//! 原生 NCK 地址的独立集成测试（PR-A）。
//!
//! 只使用驱动公开导出，验证：严格字段解析、规范解码词表、响应 transport 枚举、
//! 无效工程值规则、长度/类型投影、稳定 `source_label`，以及 R394 的
//! 独立手工 golden 字节。不连机床、不读 Catalog 文件。

use mesa_driver_sinumerik_nck::{
    MAX_AREA_NO, MAX_COUNT, NativeError, NativeNckAddress, NckDataKind, TRANSPORT_SIZES,
    encode_var_spec, resolve_native,
};
use serde_json::{Map, Value, json};

fn params(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// R394：C/1/module21/column1/line395/count1/F64_LE/0x09。
fn r394() -> Map<String, Value> {
    params(json!({
        "area": "C", "area_no": 1, "module": 21, "column": 1, "line": 395,
        "count": 1, "unit_mode": "current", "decode": "F64_LE",
        "transport_size": "0x09",
    }))
}

#[test]
fn r394_manual_golden_bytes() {
    let a = NativeNckAddress::from_parameters(&r394()).expect("解析");
    let r = resolve_native(a).expect("resolve");
    assert_eq!(
        encode_var_spec(&r.wire),
        [0x12, 0x08, 0x82, 0x41, 0x00, 0x01, 0x01, 0x8B, 0x15, 0x01]
    );
    assert_eq!(r.expected_data_len, 8);
    assert_eq!(r.expected_transport_size, 0x09);
    assert_eq!(r.data_type, mesa_core_types::DataType::F64);
    assert!(r.invalid_rule.is_none());
}

#[test]
fn canonical_decode_vocabulary_is_closed() {
    // 15 个规范名全部可解析，且 decode_name 往返一致。
    let canonical = [
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
    for name in canonical {
        let k = NckDataKind::parse_decode(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(k.decode_name(), name, "{name} 往返");
    }
    // 别名与未知值一律拒绝（不得按名字猜字节序/宽度）。
    for alias in [
        "REAL", "DINT", "LREAL", "DOUBLE", "FLOAT", "INT", "DWORD", "UDINT", "BOOLEAN", "F64",
        "F32", "I32", "U32", "U16_BE ", "f64_be", "",
    ] {
        assert!(
            NckDataKind::parse_decode(alias).is_err(),
            "{alias} 必须拒绝"
        );
    }
}

#[test]
fn transport_enum_matches_declared_set() {
    assert_eq!(TRANSPORT_SIZES, [0x05, 0x06, 0x07, 0x09]);
    for ts in TRANSPORT_SIZES {
        let mut p = r394();
        p.insert("transport_size".into(), json!(format!("0x{ts:02x}")));
        let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
        assert_eq!(r.expected_transport_size, ts);
    }
    // 整数、单数字、越界集合、空串全部拒绝。
    for bad in [
        json!(9),
        json!("0x5"),
        json!("0x04"),
        json!("0x0a"),
        json!(""),
    ] {
        let mut p = r394();
        p.insert("transport_size".into(), bad.clone());
        assert!(
            NativeNckAddress::from_parameters(&p).is_err(),
            "transport_size={bad}"
        );
    }
}

#[test]
fn legacy_task_json_is_rejected_not_migrated() {
    // 旧 NCK 配置形态：必须显式失败，且不隐式推导为原生地址。
    let legacy = json!({
        "area": "C", "area_no": 1, "block": "S", "variable": "progStatus",
        "line": 1, "column": 13, "count": 1, "unit_mode": "current",
    });
    let e = NativeNckAddress::from_parameters(&params(legacy)).unwrap_err();
    assert!(matches!(e, NativeError::UnknownField { .. }), "实际 {e:?}");
    let legacy2 = json!({
        "area": "V", "area_no": 1, "block": "M", "variable": "p0199",
        "access_path": "drive_object", "line": 1, "count": 25,
    });
    assert!(NativeNckAddress::from_parameters(&params(legacy2)).is_err());
}

#[test]
fn count_projects_types_and_checked_length() {
    // 数值 count=1 → 标量；count>1 → 数组。
    for (decode, ts, elem, scalar, array) in [
        (
            "F64_LE",
            "0x09",
            8usize,
            mesa_core_types::DataType::F64,
            mesa_core_types::DataType::F64Array,
        ),
        (
            "F32_BE",
            "0x07",
            4,
            mesa_core_types::DataType::F32,
            mesa_core_types::DataType::F32Array,
        ),
        (
            "U16_BE",
            "0x05",
            2,
            mesa_core_types::DataType::U32,
            mesa_core_types::DataType::U32Array,
        ),
        (
            "I16_BE",
            "0x05",
            2,
            mesa_core_types::DataType::I32,
            mesa_core_types::DataType::I32Array,
        ),
    ] {
        let mut p = r394();
        p.insert("decode".into(), json!(decode));
        p.insert("transport_size".into(), json!(ts));
        let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
        assert_eq!(r.expected_data_len, elem, "{decode} count=1");
        assert_eq!(r.data_type, scalar, "{decode} count=1 类型");
        p.insert("count".into(), json!(4));
        let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
        assert_eq!(r.expected_data_len, elem * 4, "{decode} count=4");
        assert_eq!(r.data_type, array, "{decode} count=4 类型");
    }
    // 组成单一文本的解码：count>1 仍是标量 String（p0199 形态）。
    let p = params(json!({
        "area": "V", "area_no": 2, "module": 26, "column": 199, "line": 1,
        "count": 25, "unit_mode": "current", "decode": "UTF16_BE",
        "transport_size": "0x06",
    }));
    let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
    assert_eq!(r.expected_data_len, 50);
    assert_eq!(r.data_type, mesa_core_types::DataType::String);
    // 最大 count 的 checked 长度（255×66 = 16830，不得溢出）。
    let p = params(json!({
        "area": "C", "area_no": 1, "module": 21, "column": 1, "line": 1,
        "count": MAX_COUNT, "unit_mode": "current", "decode": "UTF8_NUL_FIRST_66",
        "transport_size": "0x09",
    }));
    let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
    assert_eq!(r.expected_data_len, 255 * 66);
}

#[test]
fn invalid_rule_travels_with_the_binding() {
    let mut p = r394();
    p.insert("decode".into(), json!("F32_BE"));
    p.insert("transport_size".into(), json!("0x07"));
    p.insert("invalid_value".into(), json!(-200.0));
    p.insert("invalid_tolerance".into(), json!(0.001));
    let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
    let rule = r.invalid_rule.expect("规则随绑定");
    assert!(rule.hits(-200.0) && rule.hits(-199.9995));
    assert!(!rule.hits(-199.5) && !rule.hits(33.0));
    // 容差 0 合法（精确匹配）。
    p.insert("invalid_tolerance".into(), json!(0));
    let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
    let rule = r.invalid_rule.unwrap();
    assert!(rule.hits(-200.0) && !rule.hits(-200.1));
}

#[test]
fn boundary_values_are_accepted_at_limits() {
    let p = params(json!({
        "area": "H", "area_no": MAX_AREA_NO, "module": 255, "column": 65535,
        "line": 65535, "count": 1, "unit_mode": "inch", "decode": "U8",
        "transport_size": "0x05",
    }));
    let r = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
    assert_eq!(r.wire.module, 255);
    assert_eq!(r.wire.column, 65535);
    assert_eq!(r.wire.line, 65535);
    assert_eq!(r.wire.syntax_id, 0x84, "inch → 0x84");
    assert_eq!(r.wire.area_unit, (6 << 5) | MAX_AREA_NO);
}

#[test]
fn source_label_is_unique_per_address_and_decoder() {
    let a = NativeNckAddress::from_parameters(&r394()).unwrap();
    let label = a.source_label();
    assert_eq!(
        label,
        "nck://read/C/1/21/1/395/1?mode=current&decode=F64_LE&transport=0x09"
    );
    // 同地址不同解码/transport 必须产生不同标签（防止同地址不同解释混淆）。
    let mut p = r394();
    p.insert("decode".into(), json!("F32_BE"));
    p.insert("transport_size".into(), json!("0x07"));
    let other = NativeNckAddress::from_parameters(&p)
        .unwrap()
        .source_label();
    assert_ne!(label, other);
    // unit（展示注释）与 invalid_value（质量规则）都不属于地址身份：
    // 三者标签必须一致，避免同地址因元数据不同被当成不同点位。
    let mut p = r394();
    p.insert("unit".into(), json!("mm"));
    let unit_only = NativeNckAddress::from_parameters(&p)
        .unwrap()
        .source_label();
    assert_eq!(label, unit_only, "unit 不进入地址标签");
    let mut p = r394();
    p.insert("unit".into(), json!("mm"));
    p.insert("invalid_value".into(), json!(-200.0));
    p.insert("invalid_tolerance".into(), json!(0.001));
    let with_meta = NativeNckAddress::from_parameters(&p)
        .unwrap()
        .source_label();
    assert_eq!(label, with_meta, "unit 与无效值规则都不进入地址标签");
}
