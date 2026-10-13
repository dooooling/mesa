//! 原生 NCK 只读地址（唯一采集地址模型）。
//!
//! 设计要点（V2.0 §2/§3）：
//! - 采集地址由用户显式给出的协议字段组成（area/area_no/module/column/line/
//!   count/unit_mode/decode/transport_size），**不经过任何变量名目录**；
//! - 解析是唯一的：字段允许列表、必填、字面类型、精确转换、枚举、数值规则
//!   与跨字段约束全部集中在本模块，不散落到 configure/codec/identity；
//! - `decode` 采用规范词表（如 `F32_BE`），拒绝历史别名（`REAL`/`DINT`），
//!   不允许按数值大小猜字节序；
//! - `transport_size` 是**响应检查门**（设备返回的 transport 字段），
//!   不是请求操作码，故用枚举字符串让十六进制语义在 UI 显式可见；
//! - 无效工程值规则（`invalid_value`/`invalid_tolerance`）必须成对出现，
//!   且只允许用于可投影为数值的解码；
//! - `source_label` 是稳定的原生地址标识（诊断用），不含连接信息、
//!   不含人工可变的业务名称，也不含无效值规则（后者只影响质量）。
//!
//! 本模块不做：TCP/PDU/重连（transport）、字节解码（`value`）、
//! 身份浏览（`identity_spec`）、配置预填（`presets`）。

use serde_json::{Map, Value};
use thiserror::Error;

use crate::address::{NckArea, NckUnitMode};
use crate::codec::{CodecError, NckWireAddress};
use crate::value::{NckDataKind, ValueError};

/// `area_no` 上限（NCK VarSpec 低 5 位）。
pub const MAX_AREA_NO: u8 = 31;
/// `count` 上限（NCK linecount 单字节）。
pub const MAX_COUNT: u8 = 255;
/// `unit` 展示字段长度上限。
pub const MAX_UNIT_LEN: usize = 32;

/// 允许的响应 transport 枚举（仅收录已有测试覆盖的四类）。
///
/// 未来出现新的 NCK 传输类型时，应扩展协议解码器与独立测试，
/// 而不是给单个变量加白名单；`0x05` 的 NCK 字节长度规则不得套用到
/// PLC S7ANY 的同名字段。
pub const TRANSPORT_SIZES: [u8; 4] = [0x05, 0x06, 0x07, 0x09];

/// 规范 `decode` 词表（Descriptor Enum 与严格解析共用同一来源）。
pub const DECODE_NAMES: [&str; 15] = [
    "F64_BE",
    "F64_LE",
    "F32_BE",
    "I32_BE",
    "I32_LE",
    "U32_BE",
    "U16_BE",
    "U16_LE",
    "I16_BE",
    "U8",
    "BOOL",
    "UTF8_FIXED_16",
    "UTF8_NUL_LAST_32",
    "UTF8_NUL_FIRST_66",
    "UTF16_BE",
];

/// 无效工程值规则（显式配置，不靠目录隐式继承）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvalidRule {
    pub value: f64,
    pub tolerance: f64,
}

impl InvalidRule {
    /// 命中规则（含容差）即该样本不可作为正常测量。
    pub fn hits(self, sample: f64) -> bool {
        (sample - self.value).abs() <= self.tolerance
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum NativeError {
    #[error(
        "未知字段 `{key}`（新 Resource 不接受旧 block/variable/access_path 等字段，也不做自动迁移）"
    )]
    UnknownField { key: String },
    #[error("缺必填字段 `{key}`")]
    MissingField { key: &'static str },
    #[error("字段 `{key}` 需为整数（不接受字符串、小数、布尔或 null），实际 {got}")]
    NotInteger { key: &'static str, got: String },
    #[error("字段 `{key}` = {value} 越界，需 {min}..={max}")]
    OutOfRange {
        key: &'static str,
        value: i64,
        min: i64,
        max: i64,
    },
    #[error("字段 `{key}` 取值 `{value}` 非法：{reason}")]
    BadValue {
        key: &'static str,
        value: String,
        reason: String,
    },
    #[error("invalid_value 与 invalid_tolerance 必须同时提供或同时省略")]
    InvalidRulePairing,
    #[error("invalid_tolerance 需为 0..=1 的有限数值，实际 {value}")]
    InvalidTolerance { value: f64 },
    #[error("invalid_value 需为有限数值，实际 {value}")]
    InvalidValueNotFinite { value: f64 },
    #[error("decoder `{name}` 不支持无效工程值规则（仅数值型可用）")]
    InvalidRuleOnNonNumeric { name: String },
    #[error("unit 需为非空且不超过 {MAX_UNIT_LEN} 字符")]
    BadUnit,
    #[error("expected_data_len 上溢（count={count}, byte_len={byte_len}）")]
    LengthOverflow { count: u8, byte_len: usize },
    #[error(transparent)]
    Wire(#[from] CodecError),
    #[error(transparent)]
    Decode(#[from] ValueError),
}

/// 原生只读地址（用户显式给出的协议字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct NativeNckAddress {
    pub area: NckArea,
    pub area_no: u8,
    pub module: u8,
    pub column: u16,
    pub line: u16,
    pub count: u8,
    pub unit_mode: NckUnitMode,
    pub decode: NckDataKind,
    /// 期望设备响应中的 transport 字段。
    pub transport_size: u8,
    pub invalid_value: Option<f64>,
    pub invalid_tolerance: Option<f64>,
    /// 展示单位，仅用户注释；不作为工程语义已核验的证据。
    pub unit: Option<String>,
}

/// 解析并校验后的采集点（唯一执行面输入）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPoint {
    pub address: NativeNckAddress,
    pub wire: NckWireAddress,
    pub kind: NckDataKind,
    pub expected_data_len: usize,
    pub expected_transport_size: u8,
    pub data_type: mesa_core_types::DataType,
    pub invalid_rule: Option<InvalidRule>,
}

/// 允许的字段全集（其余一律 `UnknownField`，含旧模型字段）。
const ALLOWED: [&str; 12] = [
    "area",
    "area_no",
    "module",
    "column",
    "line",
    "count",
    "unit_mode",
    "decode",
    "transport_size",
    "invalid_value",
    "invalid_tolerance",
    "unit",
];

fn require_int(map: &Map<String, Value>, key: &'static str) -> Result<i64, NativeError> {
    let raw = map.get(key).ok_or(NativeError::MissingField { key })?;
    raw.as_i64().ok_or_else(|| NativeError::NotInteger {
        key,
        got: raw.to_string(),
    })
}

fn require_ranged(
    map: &Map<String, Value>,
    key: &'static str,
    max: i64,
) -> Result<i64, NativeError> {
    let v = require_int(map, key)?;
    if v < 0 || v > max {
        return Err(NativeError::OutOfRange {
            key,
            value: v,
            min: 0,
            max,
        });
    }
    Ok(v)
}

fn require_str<'a>(map: &'a Map<String, Value>, key: &'static str) -> Result<&'a str, NativeError> {
    let raw = map.get(key).ok_or(NativeError::MissingField { key })?;
    raw.as_str().ok_or_else(|| NativeError::BadValue {
        key,
        value: raw.to_string(),
        reason: "需为字符串".into(),
    })
}

/// 响应 transport 枚举解析（仅规范字符串，拒绝整数与未知值）。
pub fn parse_transport_size(s: &str) -> Result<u8, NativeError> {
    let v = match s {
        "0x05" => 0x05,
        "0x06" => 0x06,
        "0x07" => 0x07,
        "0x09" => 0x09,
        _ => {
            return Err(NativeError::BadValue {
                key: "transport_size",
                value: s.to_string(),
                reason: "仅允许 0x05/0x06/0x07/0x09（响应 transport 枚举）".into(),
            });
        }
    };
    Ok(v)
}

/// 规范 transport 字符串（source_label 用）。
pub fn transport_size_name(v: u8) -> String {
    format!("0x{v:02x}")
}

/// 单位模式规范名。
pub fn unit_mode_name(m: NckUnitMode) -> &'static str {
    match m {
        NckUnitMode::Current => "current",
        NckUnitMode::Metric => "metric",
        NckUnitMode::Inch => "inch",
    }
}

impl NativeNckAddress {
    /// 从 ResourceSelection 的扁平 `parameters` 严格解析。
    pub fn from_parameters(map: &Map<String, Value>) -> Result<Self, NativeError> {
        for key in map.keys() {
            if !ALLOWED.contains(&key.as_str()) {
                return Err(NativeError::UnknownField { key: key.clone() });
            }
        }

        let area = match require_str(map, "area")? {
            "N" => NckArea::Nck,
            "B" => NckArea::ModeGroup,
            "C" => NckArea::Channel,
            "A" => NckArea::Axis,
            "T" => NckArea::Tool,
            "V" => NckArea::FeedDrive,
            "H" => NckArea::MainDrive,
            other => {
                return Err(NativeError::BadValue {
                    key: "area",
                    value: other.to_string(),
                    reason: "仅允许 N/B/C/A/T/V/H（M/MMC 尚未确认，不开放）".into(),
                });
            }
        };
        let area_no = require_ranged(map, "area_no", MAX_AREA_NO as i64)? as u8;
        let module = require_ranged(map, "module", u8::MAX as i64)? as u8;
        let column = require_ranged(map, "column", u16::MAX as i64)? as u16;
        let line = require_ranged(map, "line", u16::MAX as i64)? as u16;
        let count = require_ranged(map, "count", MAX_COUNT as i64)? as u8;
        if count == 0 {
            return Err(NativeError::OutOfRange {
                key: "count",
                value: 0,
                min: 1,
                max: MAX_COUNT as i64,
            });
        }

        let unit_mode = match require_str(map, "unit_mode")? {
            "current" => NckUnitMode::Current,
            "metric" => NckUnitMode::Metric,
            "inch" => NckUnitMode::Inch,
            other => {
                return Err(NativeError::BadValue {
                    key: "unit_mode",
                    value: other.to_string(),
                    reason: "仅允许 current/metric/inch".into(),
                });
            }
        };

        let decode_raw = require_str(map, "decode")?;
        let decode = NckDataKind::parse_decode(decode_raw)?;

        let transport_size = parse_transport_size(require_str(map, "transport_size")?)?;

        // 无效工程值规则：成对出现、有限、容差 0..=1、仅数值型。
        let invalid_value = match map.get("invalid_value") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_f64().ok_or_else(|| NativeError::BadValue {
                key: "invalid_value",
                value: v.to_string(),
                reason: "需为数值".into(),
            })?),
        };
        let invalid_tolerance = match map.get("invalid_tolerance") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_f64().ok_or_else(|| NativeError::BadValue {
                key: "invalid_tolerance",
                value: v.to_string(),
                reason: "需为数值".into(),
            })?),
        };
        match (invalid_value, invalid_tolerance) {
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => return Err(NativeError::InvalidRulePairing),
            (Some(value), Some(tolerance)) => {
                if !value.is_finite() {
                    return Err(NativeError::InvalidValueNotFinite { value });
                }
                if !tolerance.is_finite() || !(0.0..=1.0).contains(&tolerance) {
                    return Err(NativeError::InvalidTolerance { value: tolerance });
                }
                if !decode.is_numeric() {
                    return Err(NativeError::InvalidRuleOnNonNumeric {
                        name: decode_raw.to_string(),
                    });
                }
            }
        }

        let unit = match map.get("unit") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let s = v.as_str().ok_or(NativeError::BadUnit)?.trim();
                if s.is_empty() || s.chars().count() > MAX_UNIT_LEN {
                    return Err(NativeError::BadUnit);
                }
                Some(s.to_string())
            }
        };

        Ok(Self {
            area,
            area_no,
            module,
            column,
            line,
            count,
            unit_mode,
            decode,
            transport_size,
            invalid_value,
            invalid_tolerance,
            unit,
        })
    }

    /// 稳定原生地址标识（诊断用；字段顺序/进制/大小写固定）。
    pub fn source_label(&self) -> String {
        format!(
            "nck://read/{}/{}/{}/{}/{}/{}?mode={}&decode={}&transport={}",
            self.area.letter(),
            self.area_no,
            self.module,
            self.column,
            self.line,
            self.count,
            unit_mode_name(self.unit_mode),
            self.decode.decode_name(),
            transport_size_name(self.transport_size),
        )
    }
}

/// 解析为现有 wire 结构 + 执行面所需的全部派生量（唯一函数）。
pub fn resolve_native(a: NativeNckAddress) -> Result<ResolvedPoint, NativeError> {
    let area_unit = NckWireAddress::area_unit(a.area.index(), a.area_no as u16)?;
    let wire = NckWireAddress {
        syntax_id: a.unit_mode.syntax_id(),
        area_unit,
        column: a.column,
        line: a.line,
        module: a.module,
        line_count: a.count,
    };
    let expected_data_len = usize::from(a.count)
        .checked_mul(a.decode.byte_len())
        .ok_or(NativeError::LengthOverflow {
            count: a.count,
            byte_len: a.decode.byte_len(),
        })?;
    // count>1 时数组型按 array 投影；组成单一文本的解码恒为标量 String。
    let data_type = if a.count > 1 {
        a.decode.core_array_type()
    } else {
        a.decode.core_type()
    };
    let invalid_rule = match (a.invalid_value, a.invalid_tolerance) {
        (Some(value), Some(tolerance)) => Some(InvalidRule { value, tolerance }),
        _ => None,
    };
    let kind = a.decode;
    let expected_transport_size = a.transport_size;
    Ok(ResolvedPoint {
        address: a,
        wire,
        kind,
        expected_data_len,
        expected_transport_size,
        data_type,
        invalid_rule,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(v: serde_json::Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    /// R394 规范示例（V2.0 §2.3）：C/1/module21/col1/line395/count1/F64_LE/0x09。
    fn r394() -> Map<String, Value> {
        params(json!({
            "area": "C", "area_no": 1, "module": 21, "column": 1, "line": 395,
            "count": 1, "unit_mode": "current", "decode": "F64_LE",
            "transport_size": "0x09",
        }))
    }

    #[test]
    fn r394_wire_golden_is_exact() {
        let a = NativeNckAddress::from_parameters(&r394()).expect("R394 解析");
        let r = resolve_native(a).expect("R394 resolve");
        assert_eq!(
            crate::codec::encode_var_spec(&r.wire),
            vec![0x12, 0x08, 0x82, 0x41, 0x00, 0x01, 0x01, 0x8B, 0x15, 0x01],
            "R394 独立手工 golden"
        );
        assert_eq!(r.expected_data_len, 8);
        assert_eq!(r.expected_transport_size, 0x09);
        assert_eq!(r.data_type, mesa_core_types::DataType::F64);
    }

    #[test]
    fn source_label_is_stable() {
        let a = NativeNckAddress::from_parameters(&r394()).unwrap();
        assert_eq!(
            a.source_label(),
            "nck://read/C/1/21/1/395/1?mode=current&decode=F64_LE&transport=0x09"
        );
    }

    #[test]
    fn legacy_fields_are_rejected_not_migrated() {
        for legacy in [
            "block",
            "variable",
            "access_path",
            "catalog_file",
            "retain_unresolved_points",
            "shape",
            "supported_families",
        ] {
            let mut p = r394();
            p.insert(legacy.into(), json!("M"));
            let e = NativeNckAddress::from_parameters(&p).unwrap_err();
            assert!(
                matches!(e, NativeError::UnknownField { .. }),
                "{legacy} 必须拒绝，实际 {e:?}"
            );
        }
    }

    #[test]
    fn missing_and_ill_typed_fields_rejected() {
        for key in [
            "area",
            "area_no",
            "module",
            "column",
            "line",
            "count",
            "unit_mode",
            "decode",
            "transport_size",
        ] {
            let mut p = r394();
            p.remove(key);
            assert!(
                matches!(
                    NativeNckAddress::from_parameters(&p),
                    Err(NativeError::MissingField { .. })
                ),
                "{key} 缺失必须拒绝"
            );
        }
        // 字符串整数、小数、布尔、null 均不接受。
        for bad in [json!("395"), json!(1.5), json!(true), json!(null)] {
            let mut p = r394();
            p.insert("line".into(), bad.clone());
            let e = NativeNckAddress::from_parameters(&p).unwrap_err();
            assert!(
                matches!(e, NativeError::NotInteger { .. }),
                "line={bad} 必须 NotInteger，实际 {e:?}"
            );
        }
        // 负数与越界。
        for (key, bad, max) in [
            ("area_no", json!(-1), 31),
            ("area_no", json!(32), 31),
            ("module", json!(256), 255),
            ("column", json!(65536), 65535),
            ("line", json!(-1), 65535),
            ("count", json!(0), 255),
            ("count", json!(256), 255),
        ] {
            let mut p = r394();
            p.insert(key.into(), bad.clone());
            let e = NativeNckAddress::from_parameters(&p).unwrap_err();
            assert!(
                matches!(e, NativeError::OutOfRange { .. }),
                "{key}={bad} 必须 OutOfRange（max={max}），实际 {e:?}"
            );
        }
    }

    #[test]
    fn area_and_mode_and_decode_are_strict_enums() {
        for bad in ["M", "MMC", "8", "c", ""] {
            let mut p = r394();
            p.insert("area".into(), json!(bad));
            assert!(NativeNckAddress::from_parameters(&p).is_err(), "area={bad}");
        }
        for bad in ["CURRENT", "mm", ""] {
            let mut p = r394();
            p.insert("unit_mode".into(), json!(bad));
            assert!(NativeNckAddress::from_parameters(&p).is_err(), "mode={bad}");
        }
        // 历史 catalog 别名不得通过新入口。
        for bad in ["REAL", "DINT", "LREAL", "F64", "F32", "f64_le"] {
            let mut p = r394();
            p.insert("decode".into(), json!(bad));
            assert!(
                NativeNckAddress::from_parameters(&p).is_err(),
                "decode={bad}"
            );
        }
    }

    #[test]
    fn transport_size_enum_is_strict() {
        for ok in ["0x05", "0x06", "0x07", "0x09"] {
            let mut p = r394();
            p.insert("transport_size".into(), json!(ok));
            assert!(NativeNckAddress::from_parameters(&p).is_ok(), "{ok} 应通过");
        }
        for bad in [
            json!(9),
            json!("9"),
            json!("0x04"),
            json!("0X09"),
            json!(""),
            json!("0x0A"),
            json!(0x09),
        ] {
            let mut p = r394();
            p.insert("transport_size".into(), bad.clone());
            assert!(
                NativeNckAddress::from_parameters(&p).is_err(),
                "transport_size={bad} 必须拒绝"
            );
        }
    }

    #[test]
    fn invalid_rule_pairing_and_scope() {
        // 只给其一 → 拒绝。
        let mut p = r394();
        p.insert("invalid_value".into(), json!(-200.0));
        assert_eq!(
            NativeNckAddress::from_parameters(&p).unwrap_err(),
            NativeError::InvalidRulePairing
        );
        let mut p = r394();
        p.insert("invalid_tolerance".into(), json!(0.001));
        assert_eq!(
            NativeNckAddress::from_parameters(&p).unwrap_err(),
            NativeError::InvalidRulePairing
        );
        // 成对且合法 → 通过，规则随绑定。
        let mut p = r394();
        p.insert("invalid_value".into(), json!(-200.0));
        p.insert("invalid_tolerance".into(), json!(0.001));
        let a = NativeNckAddress::from_parameters(&p).unwrap();
        let r = resolve_native(a).unwrap();
        let rule = r.invalid_rule.expect("规则随绑定");
        assert!(rule.hits(-200.0));
        assert!(rule.hits(-200.0005));
        assert!(!rule.hits(-199.0));
        // 容差越界 / 非有限。
        for bad in [json!(-0.1), json!(1.5), json!("0.1")] {
            let mut p = r394();
            p.insert("invalid_value".into(), json!(-200.0));
            p.insert("invalid_tolerance".into(), bad.clone());
            assert!(
                NativeNckAddress::from_parameters(&p).is_err(),
                "tolerance={bad}"
            );
        }
        // 文本/BOOL 解码不得携带无效值规则。
        for (decode, ts) in [
            ("UTF16_BE", "0x06"),
            ("UTF8_FIXED_16", "0x09"),
            ("BOOL", "0x09"),
        ] {
            let mut p = r394();
            p.insert("decode".into(), json!(decode));
            p.insert("transport_size".into(), json!(ts));
            p.insert("invalid_value".into(), json!(-200.0));
            p.insert("invalid_tolerance".into(), json!(0.001));
            assert!(
                matches!(
                    NativeNckAddress::from_parameters(&p),
                    Err(NativeError::InvalidRuleOnNonNumeric { .. })
                ),
                "decode={decode} 不得带无效值规则"
            );
        }
    }

    #[test]
    fn unit_field_is_bounded_optional_annotation() {
        let mut p = r394();
        p.insert("unit".into(), json!("mm/min"));
        assert_eq!(
            NativeNckAddress::from_parameters(&p)
                .unwrap()
                .unit
                .as_deref(),
            Some("mm/min")
        );
        for bad in [
            json!(""),
            json!("   "),
            json!("x".repeat(MAX_UNIT_LEN + 1)),
            json!(7),
        ] {
            let mut p = r394();
            p.insert("unit".into(), bad.clone());
            assert!(NativeNckAddress::from_parameters(&p).is_err(), "unit={bad}");
        }
    }

    #[test]
    fn count_projects_scalar_or_array_and_text_stays_scalar() {
        // 数值 count>1 → 数组类型。
        let mut p = r394();
        p.insert("count".into(), json!(4));
        let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
        assert_eq!(r.expected_data_len, 32);
        assert_eq!(r.data_type, mesa_core_types::DataType::F64Array);
        // p0199 形态：UTF16_BE count=25 → 50 字节、标量 String。
        let p = params(json!({
            "area": "V", "area_no": 1, "module": 26, "column": 199, "line": 1,
            "count": 25, "unit_mode": "current", "decode": "UTF16_BE",
            "transport_size": "0x06",
        }));
        let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
        assert_eq!(r.expected_data_len, 50);
        assert_eq!(r.data_type, mesa_core_types::DataType::String);
    }

    #[test]
    fn area_unit_overflow_is_fail_closed() {
        // area_no=31 合法；area 索引与 unit 组合由 codec 校验。
        let mut p = r394();
        p.insert("area_no".into(), json!(31));
        let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
        assert_eq!(r.wire.area_unit, (2 << 5) | 31);
    }
}
