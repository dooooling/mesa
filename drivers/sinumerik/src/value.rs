//! SINUMERIK 只读值语义（Checkpoint C：复用 Data Plane 冻结语义）。
//!
//! 与通用 OPC UA 驱动同一套规则（V1.2.1 冻结，Poll 与 Subscribe 共享解码，
//! 避免双实现分叉）：
//! - GOOD + 有效 typed 值 → CURRENT，并更新 last_known；
//! - UNCERTAIN + 有效 typed 值 → CURRENT（质量 Uncertain），不更新 last_known；
//! - BAD / 无值 / 类型不匹配 → LAST_KNOWN（有缓存）或 PLACEHOLDER（无缓存），
//!   Placeholder 时 source_timestamp=None（绝不用 0/"" 静默填充）；
//! - 不支持的数据类型在 `parse_data_type` 配置期即拒绝，不进运行期；
//! - SourceTimestamp（1601 ticks）→ Unix ns 精确保留。
//!
//! transport 只交出原生 DataValue，Quality / ValueOrigin / LastKnown 等 Mesa
//! 数据语义全部在本文件（driver 侧），不进公共 transport。

use std::collections::HashMap;

use mesa_core_types::{DataType, PointValue, Quality, Value, ValueOrigin};

use crate::canonical::SinumerikNodeId;

// ---------------------------------------------------------------------------
// 采集点规约（configure 产物，canonical 身份 + 期望类型）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PointSpec {
    pub key: String,
    pub node: SinumerikNodeId,
    pub data_type: DataType,
}

/// 期望数据类型解析（配置期；非法直接拒绝，不拖到运行期）。
pub fn parse_data_type(s: &str) -> Result<DataType, String> {
    match s.trim().to_ascii_uppercase().as_str() {
        "BOOL" | "BOOLEAN" => Ok(DataType::Bool),
        "I32" | "INT32" | "INT" => Ok(DataType::I32),
        "U32" | "UINT32" | "DWORD" => Ok(DataType::U32),
        "I64" | "INT64" => Ok(DataType::I64),
        "U64" | "UINT64" => Ok(DataType::U64),
        "F32" | "FLOAT" | "REAL" => Ok(DataType::F32),
        "F64" | "DOUBLE" | "LREAL" => Ok(DataType::F64),
        "STRING" | "STR" => Ok(DataType::String),
        "BYTES" => Ok(DataType::Bytes),
        "DATETIME" => Ok(DataType::DateTime),
        _ => Err(format!(
            "data_type `{s}` 非法，期望 BOOL/I32/U32/I64/U64/F32/F64/STRING/BYTES/DATETIME"
        )),
    }
}

fn value_fits_data_type(v: &Value, dt: DataType) -> bool {
    match (v, dt) {
        (Value::Bool(_), DataType::Bool) => true,
        (Value::I32(_), DataType::I32) => true,
        (Value::U32(_), DataType::U32) => true,
        (Value::I64(_), DataType::I64) => true,
        (Value::U64(_), DataType::U64) => true,
        (Value::F32(_), DataType::F32) => true,
        (Value::F64(_), DataType::F64) => true,
        (Value::String(_), DataType::String) => true,
        (Value::Bytes(_), DataType::Bytes) => true,
        (Value::DateTime(_), DataType::DateTime) => true,
        (Value::BoolArray(_), DataType::Bool) => true,
        (Value::I32Array(_), DataType::I32) => true,
        (Value::U32Array(_), DataType::U32) => true,
        (Value::I64Array(_), DataType::I64) => true,
        (Value::U64Array(_), DataType::U64) => true,
        (Value::F32Array(_), DataType::F32) => true,
        (Value::F64Array(_), DataType::F64) => true,
        (Value::StringArray(_), DataType::String) => true,
        (Value::DateTimeArray(_), DataType::DateTime) => true,
        // 宽容互通：U32/I32、F32/F64、I32→I64、U32→U64
        (Value::U32(_), DataType::I32) => true,
        (Value::I32(_), DataType::U32) => true,
        (Value::F32(_), DataType::F64) => true,
        (Value::F64(_), DataType::F32) => true,
        (Value::I32(_), DataType::I64) => true,
        (Value::U32(_), DataType::U64) => true,
        _ => false,
    }
}

fn coerce_value(v: Value, dt: DataType) -> Value {
    match (v, dt) {
        (Value::U32(x), DataType::I32) => Value::I32(x as i32),
        (Value::I32(x), DataType::U32) => Value::U32(x as u32),
        (Value::U32(x), DataType::F64) => Value::F64(x as f64),
        (Value::I32(x), DataType::F64) => Value::F64(x as f64),
        (Value::U32(x), DataType::F32) => Value::F32(x as f32),
        (Value::I32(x), DataType::F32) => Value::F32(x as f32),
        (Value::F32(x), DataType::F64) => Value::F64(x as f64),
        (Value::F64(x), DataType::F32) => Value::F32(x as f32),
        (Value::I32(x), DataType::I64) => Value::I64(x as i64),
        (Value::U32(x), DataType::U64) => Value::U64(x as u64),
        (other, _) => other,
    }
}

/// OPC UA DateTime ticks (1601-01-01, 100ns) → Unix ns（精确保留）。
pub(crate) fn ticks_to_unix_ns(ticks: i64) -> i64 {
    const TICKS_PER_SEC: i64 = 10_000_000;
    const UNIX_TICKS_OFFSET: i64 = 11644473600 * TICKS_PER_SEC;
    (ticks - UNIX_TICKS_OFFSET) * 100
}

fn source_timestamp_ns_from_dv(dv: &opcua_types::DataValue) -> Option<i64> {
    dv.source_timestamp.map(|dt| ticks_to_unix_ns(dt.ticks()))
}

/// 最后一次 GOOD 的缓存：值 + 其 SourceTimestamp，避免 LastKnown 携带 BAD 时的伪时间。
#[derive(Debug, Clone)]
pub struct LastKnownSample {
    pub value: Value,
    pub source_timestamp_ns: Option<i64>,
}

/// OPC UA Variant → Mesa Value。
///
/// 与通用 OPC UA 驱动逐字同一映射（冻结契约，跨驱动不得分叉）：
/// - 标量按类型直转；ByteString(None) → 空 Bytes；
/// - Guid / StatusCode / 其他外来 Variant → `String(debug)` 显式回退
///   （值可见、可调试；若期望类型不是 String，上层按类型不匹配隔离为 BAD，
///   绝不静默 0/"" 冒充有效值）；
/// - LocalizedText 取文本，空文本则回退 debug 形态；
/// - 数组保留 Typed Array（元素级 filter_map，与通用驱动一致）。
pub(crate) fn variant_to_value(v: &opcua_types::Variant) -> Option<Value> {
    use opcua_types::Variant;
    Some(match v {
        Variant::Empty => return None,
        Variant::Boolean(x) => Value::Bool(*x),
        Variant::SByte(x) => Value::I32(*x as i32),
        Variant::Byte(x) => Value::U32(*x as u32),
        Variant::Int16(x) => Value::I32(*x as i32),
        Variant::UInt16(x) => Value::U32(*x as u32),
        Variant::Int32(x) => Value::I32(*x),
        Variant::UInt32(x) => Value::U32(*x),
        Variant::Int64(x) => Value::I64(*x),
        Variant::UInt64(x) => Value::U64(*x),
        Variant::Float(x) => Value::F32(*x),
        Variant::Double(x) => Value::F64(*x),
        Variant::String(s) => Value::String(s.as_ref().to_string()),
        Variant::ByteString(bs) => {
            if let Some(bytes) = &bs.value {
                Value::Bytes(bytes.clone())
            } else {
                Value::Bytes(vec![])
            }
        }
        Variant::Guid(g) => Value::String(g.to_string()),
        Variant::DateTime(dt) => Value::DateTime(ticks_to_unix_ns(dt.ticks())),
        Variant::LocalizedText(t) => {
            let txt = t.text.as_ref().to_string();
            if txt.is_empty() {
                Value::String(format!("{t:?}"))
            } else {
                Value::String(txt)
            }
        }
        Variant::Array(arr) => {
            // 保留 Typed Array：多维同样经 values 展平视角处理
            //（dimensions 仅为形状标注，不改变元素映射）。
            let vals: Vec<Value> = arr.values.iter().filter_map(variant_to_value).collect();
            if vals.is_empty() {
                return Some(Value::String(format!("{arr:?}")));
            }
            // 推断首元素类型
            match &vals[0] {
                Value::Bool(_) => Value::BoolArray(
                    vals.into_iter()
                        .filter_map(|v| {
                            if let Value::Bool(b) = v {
                                Some(b)
                            } else {
                                None
                            }
                        })
                        .collect(),
                ),
                Value::I32(_) => Value::I32Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::I32(i) = v { Some(i) } else { None })
                        .collect(),
                ),
                Value::U32(_) => Value::U32Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::U32(u) = v { Some(u) } else { None })
                        .collect(),
                ),
                Value::I64(_) => Value::I64Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::I64(i) = v { Some(i) } else { None })
                        .collect(),
                ),
                Value::U64(_) => Value::U64Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::U64(u) = v { Some(u) } else { None })
                        .collect(),
                ),
                Value::F32(_) => Value::F32Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::F32(f) = v { Some(f) } else { None })
                        .collect(),
                ),
                Value::F64(_) => Value::F64Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::F64(f) = v { Some(f) } else { None })
                        .collect(),
                ),
                Value::String(_) => Value::StringArray(
                    vals.into_iter()
                        .filter_map(|v| {
                            if let Value::String(s) = v {
                                Some(s)
                            } else {
                                None
                            }
                        })
                        .collect(),
                ),
                Value::DateTime(_) => Value::DateTimeArray(
                    vals.into_iter()
                        .filter_map(|v| {
                            if let Value::DateTime(t) = v {
                                Some(t)
                            } else {
                                None
                            }
                        })
                        .collect(),
                ),
                _ => Value::String(format!("{arr:?}")),
            }
        }
        Variant::StatusCode(sc) => Value::String(format!("{sc:?}")),
        _ => Value::String(format!("{v:?}")),
    })
}

/// 统一解码：Poll 与 Subscribe 共享，避免双实现分叉。
pub fn decode_data_value(
    spec: &PointSpec,
    point_id: u32,
    dv: opcua_types::DataValue,
    last_known: &mut HashMap<u32, LastKnownSample>,
) -> PointValue {
    use opcua_types::StatusCode;
    let status = dv.status.unwrap_or(StatusCode::Good);
    let source_ts_current = source_timestamp_ns_from_dv(&dv);

    let maybe_raw = dv.value.as_ref().and_then(variant_to_value);
    let maybe_coerced = maybe_raw.map(|v| coerce_value(v, spec.data_type));

    // GOOD 路径：必须有值且类型匹配 → CURRENT 并更新缓存
    if status.is_good() {
        if let Some(coerced) = maybe_coerced {
            if value_fits_data_type(&coerced, spec.data_type) {
                let sample = LastKnownSample {
                    value: coerced.clone(),
                    source_timestamp_ns: source_ts_current,
                };
                last_known.insert(point_id, sample);
                return PointValue {
                    point_id,
                    value: coerced,
                    quality: Quality::Good,
                    quality_code: None,
                    source_timestamp_ns: source_ts_current,
                    value_origin: ValueOrigin::Current,
                };
            }
            // GOOD 但类型不匹配 → BadTypeMismatch 隔离
            return bad_with_cache(
                spec,
                point_id,
                last_known,
                StatusCode::BadTypeMismatch.bits() as i32,
            );
        }
        // GOOD 但无值 → BadUnexpectedError 隔离
        return bad_with_cache(
            spec,
            point_id,
            last_known,
            StatusCode::BadUnexpectedError.bits() as i32,
        );
    }

    // UNCERTAIN + 有效 typed 值 → CURRENT（质量 Uncertain），不更新 last_known
    if !status.is_bad() {
        if let Some(coerced) = maybe_coerced {
            if value_fits_data_type(&coerced, spec.data_type) {
                return PointValue {
                    point_id,
                    value: coerced,
                    quality: Quality::Uncertain,
                    quality_code: Some(status.bits() as i32),
                    source_timestamp_ns: source_ts_current,
                    value_origin: ValueOrigin::Current,
                };
            }
        }
        // UNCERTAIN 但无值/类型不符 → LastKnown/Placeholder，质量保持 Uncertain
        let (val, origin, src) = cached_or_placeholder(spec, point_id, last_known);
        return PointValue {
            point_id,
            value: val,
            quality: Quality::Uncertain,
            quality_code: Some(status.bits() as i32),
            source_timestamp_ns: src,
            value_origin: origin,
        };
    }

    // BAD：LastKnown（有缓存）或 Placeholder（无缓存）
    let (val, origin, src) = cached_or_placeholder(spec, point_id, last_known);
    let q = status_to_quality(status);
    PointValue {
        point_id,
        value: val,
        quality: q,
        quality_code: Some(status.bits() as i32),
        source_timestamp_ns: src,
        value_origin: origin,
    }
}

fn cached_or_placeholder(
    spec: &PointSpec,
    point_id: u32,
    last_known: &HashMap<u32, LastKnownSample>,
) -> (Value, ValueOrigin, Option<i64>) {
    match last_known.get(&point_id) {
        Some(s) => (
            s.value.clone(),
            ValueOrigin::LastKnown,
            s.source_timestamp_ns,
        ),
        None => (
            Value::typed_placeholder(spec.data_type),
            ValueOrigin::Placeholder,
            None,
        ),
    }
}

fn bad_with_cache(
    spec: &PointSpec,
    point_id: u32,
    last_known: &HashMap<u32, LastKnownSample>,
    code: i32,
) -> PointValue {
    let (val, origin, src) = cached_or_placeholder(spec, point_id, last_known);
    PointValue {
        point_id,
        value: val,
        quality: Quality::Bad,
        quality_code: Some(code),
        source_timestamp_ns: src,
        value_origin: origin,
    }
}

/// StatusCode → Quality（与通用 OPC UA 驱动同一判定：Good/Uncertain 按位，
/// 其余一律 Bad）。
pub fn status_to_quality(status: opcua_types::StatusCode) -> Quality {
    if status.is_good() {
        Quality::Good
    } else if status.is_uncertain() {
        Quality::Uncertain
    } else {
        Quality::Bad
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opcua_types::{DataValue, StatusCode, Variant};

    fn spec(dt: DataType) -> PointSpec {
        PointSpec {
            key: "k".into(),
            node: crate::canonical::parse_canonical("nsu=urn:t;s=A").unwrap(),
            data_type: dt,
        }
    }

    #[test]
    fn good_typed_value_is_current_and_updates_cache() {
        let s = spec(DataType::I32);
        let mut cache = HashMap::new();
        let pv = decode_data_value(&s, 7, DataValue::new_now(42i32), &mut cache);
        assert_eq!(pv.quality, Quality::Good);
        assert_eq!(pv.value_origin, ValueOrigin::Current);
        assert_eq!(pv.value, Value::I32(42));
        assert!(cache.contains_key(&7));
    }

    #[test]
    fn bad_without_cache_is_placeholder_without_time() {
        let s = spec(DataType::F64);
        let mut cache = HashMap::new();
        let dv = DataValue::new_now_status(1.0f64, StatusCode::BadNodeIdUnknown);
        let pv = decode_data_value(&s, 7, dv, &mut cache);
        assert_eq!(pv.quality, Quality::Bad);
        assert_eq!(pv.value_origin, ValueOrigin::Placeholder);
        assert_eq!(pv.source_timestamp_ns, None);
        // 绝不静默 0：placeholder 必须显式 typed placeholder
        assert_eq!(pv.value, Value::typed_placeholder(DataType::F64));
    }

    #[test]
    fn bad_with_cache_is_last_known_with_cached_time() {
        let s = spec(DataType::I32);
        let mut cache = HashMap::new();
        decode_data_value(&s, 7, DataValue::new_now(42i32), &mut cache);
        let pv = decode_data_value(
            &s,
            7,
            DataValue::new_now_status(0i32, StatusCode::BadTimeout),
            &mut cache,
        );
        assert_eq!(pv.value_origin, ValueOrigin::LastKnown);
        assert_eq!(pv.value, Value::I32(42));
    }

    #[test]
    fn uncertain_with_value_is_current_uncertain_without_cache_update() {
        let s = spec(DataType::I32);
        let mut cache = HashMap::new();
        let mut dv = DataValue::new_now(1i32);
        dv.status = Some(StatusCode::UncertainLastUsableValue);
        let pv = decode_data_value(&s, 7, dv, &mut cache);
        assert_eq!(pv.quality, Quality::Uncertain);
        assert_eq!(pv.value_origin, ValueOrigin::Current);
        assert!(!cache.contains_key(&7));
    }

    #[test]
    fn type_mismatch_is_bad_type_mismatch_never_silent_zero() {
        let s = spec(DataType::I32);
        let mut cache = HashMap::new();
        let pv = decode_data_value(&s, 7, DataValue::new_now("oops"), &mut cache);
        assert_eq!(pv.quality, Quality::Bad);
        assert_eq!(
            pv.quality_code,
            Some(StatusCode::BadTypeMismatch.bits() as i32)
        );
        assert_eq!(pv.value_origin, ValueOrigin::Placeholder);
    }

    #[test]
    fn exotic_variant_falls_back_to_explicit_string_never_silent() {
        // Guid 不在标量映射内 → 与通用驱动一致回退为 String(debug) 显式可见；
        // String 期望下为有效值，I32 期望下按类型不匹配隔离为 BAD。
        let s_str = spec(DataType::String);
        let mut cache = HashMap::new();
        let pv = decode_data_value(
            &s_str,
            7,
            DataValue::new_now(Variant::Guid(Box::new(opcua_types::Guid::null()))),
            &mut cache,
        );
        assert_eq!(pv.quality, Quality::Good);
        assert!(matches!(pv.value, Value::String(_)));

        let s_i32 = spec(DataType::I32);
        let mut cache = HashMap::new();
        let pv = decode_data_value(
            &s_i32,
            7,
            DataValue::new_now(Variant::Guid(Box::new(opcua_types::Guid::null()))),
            &mut cache,
        );
        assert_eq!(pv.quality, Quality::Bad);
        assert_eq!(
            pv.quality_code,
            Some(StatusCode::BadTypeMismatch.bits() as i32)
        );
    }

    #[test]
    fn arrays_pass_through_within_supported_scope() {
        let s = spec(DataType::F64);
        let mut cache = HashMap::new();
        let pv = decode_data_value(&s, 7, DataValue::new_now(vec![1.0f64, 2.0]), &mut cache);
        assert_eq!(pv.quality, Quality::Good);
        assert_eq!(pv.value, Value::F64Array(vec![1.0, 2.0]));
    }

    #[test]
    fn source_timestamp_ticks_preserved() {
        // 2024-01-01T00:00:00Z 的 OPC UA ticks：校验换算精确
        let unix_ns: i64 = 1_704_067_200_000_000_000;
        let ticks = (unix_ns / 100) + 11644473600 * 10_000_000;
        assert_eq!(ticks_to_unix_ns(ticks), unix_ns);
    }

    #[test]
    fn data_type_rejects_unknown_early() {
        assert!(parse_data_type("BOOL").is_ok());
        assert!(parse_data_type("datetime").is_ok());
        assert!(parse_data_type("VARIANT").is_err());
        assert!(parse_data_type("").is_err());
    }
}
