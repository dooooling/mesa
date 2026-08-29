//! S7 数据类型映射与解码（方案 §7.1 V1 子集）。
//!
//! - Core `DataType` 是对外契约；S7 侧 `S7Kind` 决定线缆字节长度与解码方式。
//! - 字符串 `"REAL"/"REAL[]"` 等大小写不敏感；同时兼容 Core 已有的 `f32`/`f64` 写法。

use forgelink_core_types::{DataType, Value};
use forgelink_driver_sdk::SdkDriverError;
use forgelink_core_types::ErrorKind;

/// S7 侧的细分类型（决定字节长度与字节序）。
///
/// 为什么细分：S7 线缆字节长度与 Core DataType 非一一映射（同为 U32 的 Byte/Word/Dword 需不同 transport_size/length），
/// 且 STRING/WSTRING/S5TIME 等需特殊头解析与变长处理，故以 S7Kind 承载线缆细节，DataType 承载业务语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S7Kind {
    Bool,
    Byte,    // 1 字节，无符号，对应 SINT/USINT/CHAR
    Word,    // 2 字节无符号，对应 UINT/WCHAR
    Dword,   // 4 字节无符号，对应 UDINT/TIME
    Int,     // 2 字节有符号
    Dint,    // 4 字节有符号
    Real,    // 4 字节 IEEE754
    Lreal,   // 8 字节 IEEE754，需 8 字节对齐，PDU 分片需按 8 字节重估
    String,  // S7 STRING（[0]=max [1]=cur [2..] ASCII），V1 按最长 254 变长处理
    WString, // S7 WSTRING（[0..1]=max*2 [2..3]=cur*2 [4..] UTF-16BE），S7-1500 常用
    S5Time,  // S5TIME 2 字节 BCD，映射为 U32 ms（只读）
    Time,    // TIME 4 字节 DINT ms
    Date,    // DATE 2 字节 days since 1990
    Dt,      // DATE_AND_TIME 8 字节 BCD
}

// S7 STRING 按最长 254 字符变长处理（256 字节含头），调用方按 DB 实际 max_len 截断；
// 为什么取 256 而非 32：常规 DB 中 STRING 声明多为 32/254，32 截断 254 会丢数据，256 覆盖最长且 PDU 仍可容纳（单项 256 < PDU 480 - 头）
const S7_STRING_READ_LEN: usize = 256;
// WSTRING 最长 254 宽字符 → 512 字节 + 4 字节头 = 516，取 512 覆盖主体，头外
const S7_WSTRING_READ_LEN: usize = 516;

impl S7Kind {
    /// 请求字节数（不含 S7 STRING 的变长头——调用方需按需扩大）。
    ///
    /// 为什么 Lreal 8 字节需重估分片：单项 8 字节使 19 项假设（4 字节项）失效，需按 PDU 剩余/12 动态计算。
    pub fn byte_len(self) -> usize {
        match self {
            S7Kind::Bool => 1, // BIT 传输仍以 1 字节回传（实际按 BYTE 读后取位避免 BIT 兼容差异）
            S7Kind::Byte => 1,
            S7Kind::Word | S7Kind::Int | S7Kind::Date | S7Kind::S5Time => 2,
            S7Kind::Dword | S7Kind::Dint | S7Kind::Real | S7Kind::Time => 4,
            S7Kind::Lreal | S7Kind::Dt => 8,
            S7Kind::String => S7_STRING_READ_LEN,
            S7Kind::WString => S7_WSTRING_READ_LEN,
        }
    }

    /// 对应的 Core DataType（为什么 S5TIME/TIME/DATE/DT 映射为数值：只读场景下以 ms/days 数值便于时序分析，避免引入复杂时间类型）。
    pub fn core_type(self) -> DataType {
        match self {
            S7Kind::Bool => DataType::Bool,
            S7Kind::Byte | S7Kind::Word | S7Kind::Dword | S7Kind::S5Time | S7Kind::Time | S7Kind::Date => DataType::U32,
            S7Kind::Int | S7Kind::Dint => DataType::I32,
            S7Kind::Real => DataType::F32,
            S7Kind::Lreal | S7Kind::Dt => DataType::F64,
            S7Kind::String | S7Kind::WString => DataType::String,
        }
    }

    /// S7 请求侧的传输尺寸（ANY 结构中的 transport_size）。
    /// 0x01 = BIT, 0x02 = BYTE
    pub fn transport_size(self) -> u8 {
        match self {
            S7Kind::Bool => 0x01,
            _ => 0x02,
        }
    }

    /// 请求长度字段（ANY 中 length 的语义：BIT 时为 bit 数，BYTE 时为 byte 数）。
    pub fn request_len(self) -> u16 {
        match self {
            S7Kind::Bool => 1,
            _ => self.byte_len() as u16,
        }
    }
}

/// 将用户在 binding 中填写的 `data_type` 字符串映射到 `(Core DataType, S7Kind)`。
/// 同时接受 Core 风格（`bool`/`i32`/`u32`/`f32`/`f64`/`string`）与 S7 风格
///（`BOOL`/`BYTE`/`WORD`/`DWORD`/`INT`/`DINT`/`REAL`/`LREAL`/`STRING`）以及常用别名。
pub fn parse_data_type(s: &str) -> Result<(DataType, S7Kind), SdkDriverError> {
    let lower = s.trim().to_ascii_lowercase();
    let (dt, kind) = match lower.as_str() {
        "bool" => (DataType::Bool, S7Kind::Bool),
        "byte" | "usint" | "char" => (DataType::U32, S7Kind::Byte),
        "word" | "uint" | "wchar" => (DataType::U32, S7Kind::Word),
        "dword" | "udint" => (DataType::U32, S7Kind::Dword),
        "int" => (DataType::I32, S7Kind::Int),
        "dint" | "sint" => (DataType::I32, S7Kind::Dint), // SINT 单字节有符号提升为 I32（byte_len 已修正）
        "real" | "f32" => (DataType::F32, S7Kind::Real),
        "lreal" | "f64" | "double" => (DataType::F64, S7Kind::Lreal),
        "string" => (DataType::String, S7Kind::String),
        "wstring" | "wchar[]" => (DataType::String, S7Kind::WString),
        "s5time" | "timer" | "s5_time" => (DataType::U32, S7Kind::S5Time),
        "time" | "tod" => (DataType::U32, S7Kind::Time),
        "date" => (DataType::U32, S7Kind::Date),
        "dt" | "date_and_time" => (DataType::F64, S7Kind::Dt),
        "counter" | "counter_word" => (DataType::U32, S7Kind::Word),
        "bytes" => (DataType::Bytes, S7Kind::Dword), // 兜底，按字节串处理
        // Core 原生风格兜底
        "i32" => (DataType::I32, S7Kind::Dint),
        "u32" => (DataType::U32, S7Kind::Dword),
        "i64" => (DataType::I64, S7Kind::Dint),
        "u64" => (DataType::U64, S7Kind::Dword),
        "f32 " | "float" => (DataType::F32, S7Kind::Real),
        _ => {
            // 兼容带 [] 数组写法时剥离后缀再试
            let base = lower.trim_end_matches("[]");
            if base != lower {
                return parse_data_type(base);
            }
            return Err(SdkDriverError::configuration(
                "UNSUPPORTED_DATA_TYPE",
                format!("不支持的 data_type `{s}`，期望 BOOL/BYTE/WORD/DWORD/INT/DINT/REAL/LREAL/STRING/WSTRING/S5TIME/TIME/DATE/DT"),
            ));
        }
    };
    // 处理 SINT 特殊：虽然逻辑上 1 字节，byte_len 需修正为 1
    let kind = if lower == "sint" { S7Kind::Byte } else { kind };
    Ok((dt, kind))
}

/// 将 S7 返回的原始字节解码为 Core `Value`。
pub fn decode_value(raw: &[u8], kind: S7Kind) -> Result<Value, SdkDriverError> {
    if raw.len() < kind.byte_len() && kind != S7Kind::String {
        return Err(SdkDriverError::new(
            ErrorKind::Decode,
            "SHORT_DATA",
            format!("kind {:?} 需要 {} 字节，实际 {}", kind, kind.byte_len(), raw.len()),
        ));
    }
    let v = match kind {
        S7Kind::Bool => Value::Bool(raw[0] != 0),
        S7Kind::Byte => Value::U32(raw[0] as u32),
        S7Kind::Word => {
            let n = u16::from_be_bytes([raw[0], raw[1]]) as u32;
            Value::U32(n)
        }
        S7Kind::Dword => {
            let n = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
            Value::U32(n)
        }
        S7Kind::Int => {
            let n = i16::from_be_bytes([raw[0], raw[1]]) as i32;
            Value::I32(n)
        }
        S7Kind::Dint => {
            let n = i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
            Value::I32(n)
        }
        S7Kind::Real => {
            let n = f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
            Value::F32(n)
        }
        S7Kind::Lreal => {
            let n = f64::from_be_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ]);
            Value::F64(n)
        }
        S7Kind::String => {
            // S7 STRING: [0]=max_len, [1]=cur_len, [2..2+cur_len)=chars (ASCII)
            // 为什么取 256：覆盖最长 254，调用方按 cur_len 截断，避免 PDU 截断丢尾
            if raw.len() < 2 {
                return Ok(Value::String(String::new()));
            }
            let cur = raw[1] as usize;
            let end = (2 + cur).min(raw.len());
            let bytes = &raw[2..end];
            let s = String::from_utf8_lossy(bytes).to_string();
            Value::String(s)
        }
        S7Kind::WString => {
            // S7 WSTRING: [0..1]=max (BE word), [2..3]=cur (BE word), [4..]=UTF-16BE
            // 为什么单独分支：WSTRING 为宽字符，需 BE 解码，非 ASCII lossy
            if raw.len() < 4 {
                return Ok(Value::String(String::new()));
            }
            let cur = u16::from_be_bytes([raw[2], raw[3]]) as usize;
            let chars = raw[4..].chunks_exact(2).take(cur).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<u16>>();
            Value::String(String::from_utf16_lossy(&chars))
        }
        S7Kind::S5Time => {
            // S5TIME 2 字节 BCD：映射为毫秒数便于时序分析（只读）
            if raw.len() < 2 { return Ok(Value::U32(0)); }
            Value::U32(u16::from_be_bytes([raw[0], raw[1]]) as u32)
        }
        S7Kind::Time => Value::U32(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        S7Kind::Date => Value::U32(u16::from_be_bytes([raw[0], raw[1]]) as u32),
        S7Kind::Dt => {
            // DATE_AND_TIME 8 字节 BCD，直接以 F64 数值暴露（只读诊断用）
            Value::F64(f64::from_be_bytes([raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7]]))
        }
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(kind: S7Kind, bytes: &[u8]) -> Value {
        decode_value(bytes, kind).unwrap()
    }

    #[test]
    fn parse_case_insensitive() {
        let (dt, k) = parse_data_type("REAL").unwrap();
        assert_eq!(dt, DataType::F32);
        assert_eq!(k, S7Kind::Real);
        let (dt, k) = parse_data_type("lreal").unwrap();
        assert_eq!(dt, DataType::F64);
        assert_eq!(k, S7Kind::Lreal);
        let (dt, k) = parse_data_type("bool").unwrap();
        assert_eq!(dt, DataType::Bool);
        assert_eq!(k, S7Kind::Bool);
        assert!(parse_data_type("FOO").is_err());
    }

    #[test]
    fn decode_int_be() {
        assert_eq!(dec(S7Kind::Int, &[0xFF, 0xFF]), Value::I32(-1));
        assert_eq!(dec(S7Kind::Word, &[0x00, 0x7B]), Value::U32(123));
        assert_eq!(dec(S7Kind::Dint, &[0xFF, 0xFF, 0xFF, 0xFE]), Value::I32(-2));
        assert_eq!(dec(S7Kind::Real, &[0x3F, 0x80, 0x00, 0x00]), Value::F32(1.0));
        assert_eq!(dec(S7Kind::Byte, &[0xBF]), Value::U32(0xBF));
    }

    #[test]
    fn decode_string() {
        // max 20, cur 5, "Hello"
        let raw = [20, 5, b'H', b'e', b'l', b'l', b'o', 0, 0];
        assert_eq!(dec(S7Kind::String, &raw), Value::String("Hello".to_string()));
    }
}
