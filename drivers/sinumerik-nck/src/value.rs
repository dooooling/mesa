//! NCK 值语义（ADR 0001 §25：`NckSample` + 无伪造时间戳）。
//!
//! NCK 没有 OPC UA 式的 StatusCode/SourceTimestamp：正常（return code OK +
//! 解码 OK）→ GOOD；变量错误（return BAD）→ BAD。`source_timestamp_ns` 恒为
//! None（S7 ReadVar 响应不携带值产生时刻，Mesa 收包时间另有 batch 层字段，
//! 绝不伪装成设备时间）。
//!
//! V1 只读解码五种标量（F64/F32/I32/U32/BOOL，大端）；STRING 等暂不支持
//! （NCK 字符串线缆形态待真机确认，fail-closed，不猜）。

use mesa_core_types::{Quality, Value};
use thiserror::Error;

/// NCK 数据种类（catalog `data_type` 字符串的受控词表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NckDataKind {
    F64,
    F32,
    I32,
    U32,
    Bool,
}

impl NckDataKind {
    /// 线缆字节长（与 catalog `element_size` 一致性由 codec 校验）。
    pub fn byte_len(self) -> usize {
        match self {
            NckDataKind::F64 => 8,
            NckDataKind::F32 | NckDataKind::I32 | NckDataKind::U32 => 4,
            NckDataKind::Bool => 1,
        }
    }

    /// Core 数据类型（PointDescriptor 用）。
    pub fn core_type(self) -> mesa_core_types::DataType {
        match self {
            NckDataKind::F64 => mesa_core_types::DataType::F64,
            NckDataKind::F32 => mesa_core_types::DataType::F32,
            NckDataKind::I32 => mesa_core_types::DataType::I32,
            NckDataKind::U32 => mesa_core_types::DataType::U32,
            NckDataKind::Bool => mesa_core_types::DataType::Bool,
        }
    }

    /// Core 数组类型（count>1 时 PointDescriptor 用，与 pack_array 同口径；
    /// count==1 永不进 array 分支）。
    pub fn core_array_type(self) -> mesa_core_types::DataType {
        match self {
            NckDataKind::F64 => mesa_core_types::DataType::F64Array,
            NckDataKind::F32 => mesa_core_types::DataType::F32Array,
            NckDataKind::I32 => mesa_core_types::DataType::I32Array,
            NckDataKind::U32 => mesa_core_types::DataType::U32Array,
            NckDataKind::Bool => mesa_core_types::DataType::BoolArray,
        }
    }

    /// 解析 catalog `data_type`（大小写不敏感；STRING 等暂不支持）。
    pub fn parse(s: &str) -> Result<Self, ValueError> {
        match s.trim().to_ascii_uppercase().as_str() {
            "F64" | "LREAL" | "DOUBLE" => Ok(NckDataKind::F64),
            "F32" | "REAL" | "FLOAT" => Ok(NckDataKind::F32),
            "I32" | "DINT" | "INT" => Ok(NckDataKind::I32),
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
    #[error("不支持的 NCK data_type `{name}`（V1 仅 F64/F32/I32/U32/BOOL）")]
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

/// 解码单元素原始字节（大端；长度不足即错，不截断猜测）。
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
        NckDataKind::F32 => Value::F32(f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        NckDataKind::I32 => Value::I32(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        NckDataKind::U32 => Value::U32(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        NckDataKind::Bool => Value::Bool(raw[0] != 0),
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

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
