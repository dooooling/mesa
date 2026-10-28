//! NCK 值语义（ADR 0001 §25：`NckSample` + 无伪造时间戳）。
//!
//! NCK 没有 OPC UA 式的 StatusCode/SourceTimestamp：正常（return code OK +
//! 解码 OK）→ GOOD；变量错误（return BAD）→ BAD。`source_timestamp_ns` 恒为
//! None（S7 ReadVar 响应不携带值产生时刻，Mesa 收包时间另有 batch 层字段，
//! 绝不伪装成设备时间）。
//!
//! 只读解码数值及目录显式声明的小端NC数值；文本仅支持LX与虚拟机
//! 已核对的两种NUL分隔投影，其余字符串格式保持拒绝。

use mesa_core_types::{Quality, Value};
use thiserror::Error;

/// NCK 数据种类（catalog `data_type` 字符串的受控词表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NckDataKind {
    F64,
    /// NC变量返回的小端浮点；与驱动参数的大端F32分开声明。
    F64Le,
    F32,
    I32,
    I32Le,
    U32,
    /// U16按协议解码后提升到Core的U32，避免引入Core类型分支。
    U16Le,
    U8,
    U16Be,
    I16Be,
    /// 固定16字节的NC机床轴名，逐行独立解码。
    Utf8Fixed16,
    /// p0199由多个大端U16组成一个驱动对象名，不投影成数值数组。
    Utf16Be,
    /// LX读取主程序路径组成行后，取最后一个非空的NUL分隔字符串。
    Utf8NulLast32,
    /// LX读取当前执行程序块后，取第一个非空的NUL分隔字符串。
    Utf8NulFirst66,
    Bool,
}

impl NckDataKind {
    /// 线缆字节长（与 catalog `element_size` 一致性由 codec 校验）。
    pub fn byte_len(self) -> usize {
        match self {
            NckDataKind::F64 | NckDataKind::F64Le => 8,
            NckDataKind::F32 | NckDataKind::I32 | NckDataKind::I32Le | NckDataKind::U32 => 4,
            NckDataKind::U16Le | NckDataKind::U16Be | NckDataKind::I16Be | NckDataKind::Utf16Be => {
                2
            }
            NckDataKind::U8 => 1,
            NckDataKind::Utf8Fixed16 => 16,
            NckDataKind::Utf8NulLast32 => 32,
            NckDataKind::Utf8NulFirst66 => 66,
            NckDataKind::Bool => 1,
        }
    }

    /// Core 数据类型（PointDescriptor 用）。
    pub fn core_type(self) -> mesa_core_types::DataType {
        match self {
            NckDataKind::F64 | NckDataKind::F64Le => mesa_core_types::DataType::F64,
            NckDataKind::F32 => mesa_core_types::DataType::F32,
            NckDataKind::I32 | NckDataKind::I32Le | NckDataKind::I16Be => {
                mesa_core_types::DataType::I32
            }
            NckDataKind::U32 | NckDataKind::U16Le | NckDataKind::U16Be | NckDataKind::U8 => {
                mesa_core_types::DataType::U32
            }
            NckDataKind::Utf8NulLast32
            | NckDataKind::Utf8NulFirst66
            | NckDataKind::Utf8Fixed16
            | NckDataKind::Utf16Be => mesa_core_types::DataType::String,
            NckDataKind::Bool => mesa_core_types::DataType::Bool,
        }
    }

    /// Core 数组类型（count>1 时 PointDescriptor 用，与 pack_array 同口径；
    /// count==1 永不进 array 分支）。
    pub fn core_array_type(self) -> mesa_core_types::DataType {
        match self {
            NckDataKind::F64 | NckDataKind::F64Le => mesa_core_types::DataType::F64Array,
            NckDataKind::F32 => mesa_core_types::DataType::F32Array,
            NckDataKind::I32 | NckDataKind::I32Le | NckDataKind::I16Be => {
                mesa_core_types::DataType::I32Array
            }
            NckDataKind::U32 | NckDataKind::U16Le | NckDataKind::U16Be | NckDataKind::U8 => {
                mesa_core_types::DataType::U32Array
            }
            NckDataKind::Utf8Fixed16 => mesa_core_types::DataType::StringArray,
            // 这些行组成同一文本字段；投影结果是标量字符串，不是行数组。
            NckDataKind::Utf8NulLast32 | NckDataKind::Utf8NulFirst66 | NckDataKind::Utf16Be => {
                mesa_core_types::DataType::String
            }
            NckDataKind::Bool => mesa_core_types::DataType::BoolArray,
        }
    }

    /// 规范解码名（`decode` 字段的唯一合法词表；大小写敏感，拒绝别名）。
    ///
    /// 与 [`Self::parse`] 的区别：`parse` 兼容历史 catalog 别名
    /// （`REAL`/`DINT`/`LREAL` 等），仅用于迁移期的 catalog 装载；
    /// 新 Resource 配置一律走本函数，别名与未知值都必须失败，
    /// 避免用户用 `REAL` 猜测字节序或宽度。
    pub fn decode_name(self) -> &'static str {
        match self {
            NckDataKind::F64 => "F64_BE",
            NckDataKind::F64Le => "F64_LE",
            NckDataKind::F32 => "F32_BE",
            NckDataKind::I32 => "I32_BE",
            NckDataKind::I32Le => "I32_LE",
            NckDataKind::U32 => "U32_BE",
            NckDataKind::U16Be => "U16_BE",
            NckDataKind::U16Le => "U16_LE",
            NckDataKind::I16Be => "I16_BE",
            NckDataKind::U8 => "U8",
            NckDataKind::Bool => "BOOL",
            NckDataKind::Utf8Fixed16 => "UTF8_FIXED_16",
            NckDataKind::Utf8NulLast32 => "UTF8_NUL_LAST_32",
            NckDataKind::Utf8NulFirst66 => "UTF8_NUL_FIRST_66",
            NckDataKind::Utf16Be => "UTF16_BE",
        }
    }

    /// 严格规范解码解析（新 Resource 唯一入口）。
    pub fn parse_decode(s: &str) -> Result<Self, ValueError> {
        match s {
            "F64_BE" => Ok(NckDataKind::F64),
            "F64_LE" => Ok(NckDataKind::F64Le),
            "F32_BE" => Ok(NckDataKind::F32),
            "I32_BE" => Ok(NckDataKind::I32),
            "I32_LE" => Ok(NckDataKind::I32Le),
            "U32_BE" => Ok(NckDataKind::U32),
            "U16_BE" => Ok(NckDataKind::U16Be),
            "U16_LE" => Ok(NckDataKind::U16Le),
            "I16_BE" => Ok(NckDataKind::I16Be),
            "U8" => Ok(NckDataKind::U8),
            "BOOL" => Ok(NckDataKind::Bool),
            "UTF8_FIXED_16" => Ok(NckDataKind::Utf8Fixed16),
            "UTF8_NUL_LAST_32" => Ok(NckDataKind::Utf8NulLast32),
            "UTF8_NUL_FIRST_66" => Ok(NckDataKind::Utf8NulFirst66),
            "UTF16_BE" => Ok(NckDataKind::Utf16Be),
            _ => Err(ValueError::UnsupportedType {
                name: s.to_string(),
            }),
        }
    }

    /// 是否可投影为数值（无效工程值规则只允许用于这些解码）。
    /// 文本与 BOOL 不参与数值无效值判定。
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            NckDataKind::F64
                | NckDataKind::F64Le
                | NckDataKind::F32
                | NckDataKind::I32
                | NckDataKind::I32Le
                | NckDataKind::U32
                | NckDataKind::U16Be
                | NckDataKind::U16Le
                | NckDataKind::I16Be
                | NckDataKind::U8
        )
    }

    /// 解析catalog类型；小端变体必须显式声明，不能根据数值大小猜字节序。
    pub fn parse(s: &str) -> Result<Self, ValueError> {
        match s.trim().to_ascii_uppercase().as_str() {
            "F64" | "LREAL" | "DOUBLE" => Ok(NckDataKind::F64),
            "F64_LE" => Ok(NckDataKind::F64Le),
            "F32" | "REAL" | "FLOAT" => Ok(NckDataKind::F32),
            "I32" | "DINT" | "INT" => Ok(NckDataKind::I32),
            "I32_LE" => Ok(NckDataKind::I32Le),
            "U16_LE" => Ok(NckDataKind::U16Le),
            "U8" => Ok(NckDataKind::U8),
            "U16_BE" => Ok(NckDataKind::U16Be),
            "I16_BE" => Ok(NckDataKind::I16Be),
            "UTF8_FIXED_16" => Ok(NckDataKind::Utf8Fixed16),
            "UTF16_BE" => Ok(NckDataKind::Utf16Be),
            "UTF8_NUL_LAST_32" => Ok(NckDataKind::Utf8NulLast32),
            "UTF8_NUL_FIRST_66" => Ok(NckDataKind::Utf8NulFirst66),
            "U32" | "DWORD" | "UDINT" => Ok(NckDataKind::U32),
            "BOOL" | "BOOLEAN" => Ok(NckDataKind::Bool),
            _ => Err(ValueError::UnsupportedType {
                name: s.to_string(),
            }),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ValueError {
    #[error("NCK对象名不是有效UTF-16：{reason}")]
    InvalidUtf16 { reason: String },
    #[error("NCK文本不是有效UTF-8：{reason}")]
    InvalidUtf8 { reason: String },
    #[error("不支持的 NCK data_type `{name}`（支持数值标量，字符串尚未适配）")]
    UnsupportedType { name: String },
    #[error("数据过短：{kind:?} 需 {need} 字节，实际 {got}")]
    ShortData {
        kind: NckDataKind,
        need: usize,
        got: usize,
    },
}

/// 单采样（正常 → GOOD 值；调用方在 transport BAD 时直接产 BAD，不经此处）。
#[derive(Debug, Clone, PartialEq)]
pub struct NckSample {
    pub value: Option<Value>,
    pub quality: Quality,
    pub quality_code: Option<i32>,
}

impl NckSample {
    pub fn good(value: Value) -> Self {
        Self {
            value: Some(value),
            quality: Quality::Good,
            quality_code: None,
        }
    }

    pub fn bad() -> Self {
        Self {
            value: None,
            quality: Quality::Bad,
            quality_code: None,
        }
    }
}

/// 按目录声明解码单元素；长度不足即错，不截断或根据数值猜字节序。
pub fn decode_value(raw: &[u8], kind: NckDataKind) -> Result<Value, ValueError> {
    let need = kind.byte_len();
    if raw.len() < need {
        return Err(ValueError::ShortData {
            kind,
            need,
            got: raw.len(),
        });
    }
    let v = match kind {
        NckDataKind::F64 => Value::F64(f64::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ])),
        NckDataKind::F64Le => Value::F64(f64::from_le_bytes(raw[..8].try_into().unwrap())),
        NckDataKind::F32 => Value::F32(f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        NckDataKind::I32 => Value::I32(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        NckDataKind::I32Le => Value::I32(i32::from_le_bytes(raw[..4].try_into().unwrap())),
        NckDataKind::U32 => Value::U32(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        NckDataKind::U16Le => Value::U32(u16::from_le_bytes([raw[0], raw[1]]) as u32),
        NckDataKind::U8 => Value::U32(raw[0] as u32),
        NckDataKind::U16Be => Value::U32(u16::from_be_bytes([raw[0], raw[1]]) as u32),
        NckDataKind::I16Be => Value::I32(i16::from_be_bytes([raw[0], raw[1]]) as i32),
        NckDataKind::Utf16Be => {
            if !raw.len().is_multiple_of(2) {
                return Err(ValueError::InvalidUtf16 {
                    reason: "字节长度必须为偶数".into(),
                });
            }
            let words: Vec<u16> = raw
                .chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect();
            let text = String::from_utf16(&words).map_err(|e| ValueError::InvalidUtf16 {
                reason: e.to_string(),
            })?;
            Value::String(text.split('\0').next().unwrap_or("").to_string())
        }
        NckDataKind::Utf8Fixed16 => {
            let text = std::str::from_utf8(&raw[..16]).map_err(|e| ValueError::InvalidUtf8 {
                reason: e.to_string(),
            })?;
            Value::String(text.split('\0').next().unwrap_or("").to_string())
        }
        NckDataKind::Utf8NulLast32 | NckDataKind::Utf8NulFirst66 => {
            let text = std::str::from_utf8(raw).map_err(|e| ValueError::InvalidUtf8 {
                reason: e.to_string(),
            })?;
            let mut parts = text
                .split('\0')
                .map(|part| part.trim_matches(' '))
                .filter(|part| !part.is_empty());
            let selected = if kind == NckDataKind::Utf8NulLast32 {
                parts.next_back()
            } else {
                parts.next()
            };
            Value::String(selected.unwrap_or("").to_string())
        }
        NckDataKind::Bool => Value::Bool(raw[0] != 0),
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nc_little_endian_types_decode_without_changing_drive_endian() {
        assert_eq!(
            decode_value(&123.5f64.to_le_bytes(), NckDataKind::F64Le).unwrap(),
            Value::F64(123.5)
        );
        assert_eq!(
            decode_value(&(-17000i32).to_le_bytes(), NckDataKind::I32Le).unwrap(),
            Value::I32(-17000)
        );
        assert_eq!(
            decode_value(&[1, 128], NckDataKind::U16Le).unwrap(),
            Value::U32(32769)
        );
        assert_eq!(
            decode_value(&33f32.to_be_bytes(), NckDataKind::F32).unwrap(),
            Value::F32(33.0)
        );
        assert!(decode_value(&[1], NckDataKind::U16Le).is_err());
        assert_eq!(
            NckDataKind::U16Le.core_array_type(),
            mesa_core_types::DataType::U32Array
        );
    }

    #[test]
    fn lx_text_projection_uses_nonempty_components_and_rejects_invalid_utf8() {
        let mut path = vec![0; 160];
        let components = b"_N_MPF_DIR\0 TEST_MPF \0\0\0\0";
        path[..components.len()].copy_from_slice(components);
        assert_eq!(
            decode_value(&path, NckDataKind::Utf8NulLast32).unwrap(),
            Value::String("TEST_MPF".into())
        );
        assert_eq!(
            decode_value(&[0; 66], NckDataKind::Utf8NulFirst66).unwrap(),
            Value::String("".into())
        );
        let mut invalid = vec![0; 66];
        invalid[0] = 0xff;
        assert!(decode_value(&invalid, NckDataKind::Utf8NulFirst66).is_err());
        assert!(decode_value(&[0; 31], NckDataKind::Utf8NulLast32).is_err());
    }

    /// 32 / 66 字节文本投影的**边界与异常值**（actBlock = `UTF8_NUL_FIRST_66`，
    /// progName = `UTF8_NUL_LAST_32`；长度依据见
    /// `docs/reference/nck-source-evidence-stage4.md` §2，**手册未写长度**）。
    ///
    /// 三条语义差异（本测试同时把它们钉住，避免以后被“统一”掉）：
    /// 1. `UTF8_NUL_FIRST_66` 取**第一个非空段**（跳过前置 NUL/空格填充）；
    /// 2. `UTF8_NUL_LAST_32` 取**最后一个非空段**（跳过后置 NUL/空格填充）；
    /// 3. `UTF8_FIXED_16` / `UTF16_BE` 取**字面第一段**（不跳填充、不 trim 空格）。
    ///
    /// 长度口径：`decode_value` 只拒绝 `len < 32/66`（`ShortData`），
    /// **不截断超长输入**——超长的实际拦截点在 `lib.rs::decode_point`
    /// （`bytes.len() != spec.expected_len` ⇒ 该点 BAD）。两处行为本测试都断言，
    /// 以免有人误以为解码层会“自动截断到 66”。
    #[test]
    fn nul_text_projection_boundaries_at_32_and_66() {
        // --- 正好 32 字节：无 NUL ⇒ 整块就是唯一段 ---
        let exact32 = vec![b'A'; 32];
        assert_eq!(
            decode_value(&exact32, NckDataKind::Utf8NulLast32).unwrap(),
            Value::String("A".repeat(32))
        );
        // --- 正好 66 字节：同理 ---
        let exact66 = vec![b'B'; 66];
        assert_eq!(
            decode_value(&exact66, NckDataKind::Utf8NulFirst66).unwrap(),
            Value::String("B".repeat(66))
        );

        // --- 短一字节即 fail-closed（不补齐、不按 0 填充） ---
        assert!(decode_value(&[b'A'; 31], NckDataKind::Utf8NulLast32).is_err());
        assert!(decode_value(&[b'A'; 65], NckDataKind::Utf8NulFirst66).is_err());

        // --- 超长：解码层不截断（33 / 67 字节），由 decode_point 的 exact 校验拦截 ---
        let mut over32 = vec![b'A'; 31];
        over32.push(0);
        over32.push(b'Z'); // 33 字节，末段 "Z"
        assert_eq!(
            decode_value(&over32, NckDataKind::Utf8NulLast32).unwrap(),
            Value::String("Z".into()),
            "NUL_LAST 取最后一个非空段，33 字节输入整体参与"
        );
        let mut over66 = vec![b'C'; 66];
        over66.push(b'X');
        assert_eq!(
            decode_value(&over66, NckDataKind::Utf8NulFirst66).unwrap(),
            Value::String(format!("{}X", "C".repeat(66))),
            "解码层不按 66 截断；超长必须由调用方（decode_point）拒绝"
        );

        // --- 含 NUL：FIXST 取第一个非空段，LAST 取最后一个非空段 ---
        let mut block = Vec::new();
        block.extend_from_slice(b"N10 G0 X0\0");
        block.resize(66, 0);
        assert_eq!(
            decode_value(&block, NckDataKind::Utf8NulFirst66).unwrap(),
            Value::String("N10 G0 X0".into())
        );
        let mut path = Vec::new();
        path.extend_from_slice(b"_N_MPF_DIR\0  WORK.MPF  \0");
        path.resize(32, 0);
        assert_eq!(
            decode_value(&path, NckDataKind::Utf8NulLast32).unwrap(),
            Value::String("WORK.MPF".into()),
            "末段两侧空格被 trim；后置 NUL 填充被跳过"
        );
        // 前置 NUL 填充（FIRST 的典型形态）跳过空段，不是“返回空串”。
        let mut leading_nul = vec![0u8; 10];
        leading_nul.extend_from_slice(b"ACT.BLK");
        leading_nul.resize(66, 0);
        assert_eq!(
            decode_value(&leading_nul, NckDataKind::Utf8NulFirst66).unwrap(),
            Value::String("ACT.BLK".into())
        );

        // --- 空串：全 NUL（32 与 66） ⇒ ""，不报错 ---
        assert_eq!(
            decode_value(&[0u8; 32], NckDataKind::Utf8NulLast32).unwrap(),
            Value::String(String::new())
        );
        assert_eq!(
            decode_value(&[0u8; 66], NckDataKind::Utf8NulFirst66).unwrap(),
            Value::String(String::new())
        );

        // --- 多字节 UTF-8：恰好压到边界 → 正常；被边界切断 → 无效 UTF-8（fail-closed） ---
        let mut exact_multibyte = vec![b'D'; 63];
        exact_multibyte.extend_from_slice("中".as_bytes()); // 3 字节 ⇒ 合计 66
        assert_eq!(
            decode_value(&exact_multibyte, NckDataKind::Utf8NulFirst66).unwrap(),
            Value::String(format!("{}中", "D".repeat(63)))
        );
        let mut truncated = vec![b'D'; 65];
        truncated.push(0xE4); // "中" 的首字节，被 66 字节边界截断
        assert!(
            decode_value(&truncated, NckDataKind::Utf8NulFirst66).is_err(),
            "跨边界被截断的多字节字符必须报错，不得替换成 U+FFFD"
        );
        let mut truncated32 = vec![b'E'; 31];
        truncated32.push(0xE4);
        assert!(decode_value(&truncated32, NckDataKind::Utf8NulLast32).is_err());
    }

    #[test]
    fn decode_golden_values() {
        assert_eq!(
            decode_value(
                &[0x40, 0x5E, 0xDD, 0x2F, 0x1A, 0x9F, 0xBE, 0x77],
                NckDataKind::F64
            )
            .unwrap(),
            Value::F64(123.456)
        );
        assert_eq!(
            decode_value(&[0x3F, 0x80, 0x00, 0x00], NckDataKind::F32).unwrap(),
            Value::F32(1.0)
        );
        assert_eq!(
            decode_value(&[0xFF, 0xFF, 0xFF, 0xFE], NckDataKind::I32).unwrap(),
            Value::I32(-2)
        );
        assert_eq!(
            decode_value(&[0x00, 0x00, 0x00, 0x7B], NckDataKind::U32).unwrap(),
            Value::U32(123)
        );
        assert_eq!(
            decode_value(&[0x01], NckDataKind::Bool).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            decode_value(&[0x00], NckDataKind::Bool).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn short_and_unsupported_fail_closed() {
        assert!(decode_value(&[0x00, 0x01], NckDataKind::F64).is_err());
        assert!(NckDataKind::parse("STRING").is_err());
        assert!(NckDataKind::parse("STRUCT").is_err());
        assert_eq!(NckDataKind::parse("lreal").unwrap(), NckDataKind::F64);
    }
}
