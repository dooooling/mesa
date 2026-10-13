//! 828D 预填模板的结构、来源与「模板+用户输入 → 线缆字节」一致性。
//!
//! 取代原 `828d_catalog.rs`：模板不再是采集准入，但**模板数值仍必须与
//! 已冻结的候选定义完全一致**，因此保留原 golden 字节向量作为回归。
//! 模板缺失/非法只影响浏览，已在 `lib.rs` 单测覆盖。
use mesa_driver_sinumerik_nck::{
    NativeNckAddress, PresetRegistry, PresetTemplate, encode_var_spec, resolve_native,
};
use serde_json::{Value, json};

/// 按 area + block + 变量名（+ 入口 module）定位模板。
///
/// block 必须给出：`progName` 等变量在多个 block 下同名，id 若不限定会歧义。
/// module 也必须显式给出：驱动对象区两条入口编号不同，不得互相换算。
fn preset(area: &str, block: &str, variable: &str, module: u64) -> PresetTemplate {
    let reg = PresetRegistry::embedded();
    let prefix = format!("828d.{area}.{block}.{variable}.");
    reg.for_family("828d")
        .into_iter()
        .find(|t| {
            t.id.starts_with(&prefix)
                && t.parameters.get("module").and_then(|v| v.as_u64()) == Some(module)
        })
        .cloned()
        .unwrap_or_else(|| panic!("缺模板 {area}/{block}/{variable}/module{module}"))
}

fn bytes(value: &str) -> Vec<u8> {
    value
        .split_whitespace()
        .map(|p| u8::from_str_radix(p, 16).unwrap())
        .collect()
}

/// 模板只带结构固定字段：area_no/line 必须由使用者确认。
#[test]
fn templates_never_prefill_instance_or_line() {
    let reg = PresetRegistry::embedded();
    assert!(
        reg.error().is_none(),
        "内嵌模板必须全部合法：{:?}",
        reg.error()
    );
    let all = reg.for_family("828d");
    assert!(all.len() >= 42, "候选条目不应在转换中丢失");
    for t in &all {
        assert_eq!(t.validation_status, "candidate");
        assert!(!t.parameters.contains_key("area_no"));
        assert!(!t.parameters.contains_key("line"));
        assert_eq!(t.required_user_inputs, vec!["area_no", "line"]);
        assert!(!t.description.trim().is_empty(), "{} 缺来源说明", t.id);
    }
}

/// 每个模板补上使用者确认的 area_no/line 后，都能组成合法原生地址。
#[test]
fn every_template_completes_into_a_valid_native_address() {
    let reg = PresetRegistry::embedded();
    for t in reg.for_family("828d") {
        let mut p = t.parameters.clone();
        p.insert("area_no".into(), Value::from(1));
        p.insert("line".into(), Value::from(1));
        let addr = NativeNckAddress::from_parameters(&p)
            .unwrap_or_else(|e| panic!("模板 {} 无法组成合法地址: {e}", t.id));
        resolve_native(addr).unwrap_or_else(|e| panic!("模板 {} 无法解析: {e}", t.id));
    }
}

/// 原候选目录的 golden 字节（独立手工向量），现经「模板 + 用户输入」生成。
#[test]
fn preset_plus_user_input_reproduces_frozen_wire_bytes() {
    // (area, block, variable, module, area_no, line, expected)
    // module 取已冻结候选定义里的真实值（非 26；驱动对象区才用 26/130）。
    let cases = [
        (
            "C",
            "S",
            "progStatus",
            127,
            1,
            1,
            "12 08 82 41 00 0d 00 01 7f 01",
        ),
        (
            "B",
            "S",
            "opMode",
            127,
            1,
            1,
            "12 08 82 21 00 03 00 01 7f 01",
        ),
        (
            "C",
            "SPARPP",
            "progName",
            122,
            1,
            1,
            "12 08 82 41 00 05 00 01 7a 01",
        ),
        (
            "C",
            "SPARP",
            "progName",
            125,
            1,
            1,
            "12 08 82 41 00 03 00 01 7d 01",
        ),
        (
            "C",
            "SPARP",
            "actLineNumber",
            125,
            1,
            1,
            "12 08 82 41 00 09 00 01 7d 01",
        ),
        (
            "C",
            "SPARP",
            "actBlock",
            125,
            1,
            1,
            "12 08 82 41 00 1f 00 01 7d 01",
        ),
        (
            "C",
            "SGA",
            "actToolBasePos",
            115,
            1,
            3,
            "12 08 82 41 00 02 00 03 73 01",
        ),
        (
            "C",
            "SEGA",
            "actProgPosBKS",
            112,
            1,
            3,
            "12 08 82 41 00 21 00 03 70 01",
        ),
        (
            "C",
            "S",
            "actFeedRateIpo",
            127,
            1,
            1,
            "12 08 82 41 00 01 00 01 7f 01",
        ),
        (
            "C",
            "S",
            "cmdFeedRateIpo",
            127,
            1,
            1,
            "12 08 82 41 00 02 00 01 7f 01",
        ),
        (
            "C",
            "S",
            "feedRateIpoOvr",
            127,
            1,
            1,
            "12 08 82 41 00 03 00 01 7f 01",
        ),
        (
            "C",
            "SSP",
            "cmdSpeed",
            114,
            1,
            2,
            "12 08 82 41 00 03 00 02 72 01",
        ),
        (
            "C",
            "SSP",
            "speedOvr",
            114,
            1,
            2,
            "12 08 82 41 00 04 00 02 72 01",
        ),
        (
            "C",
            "S",
            "actTNumber",
            127,
            1,
            1,
            "12 08 82 41 00 17 00 01 7f 01",
        ),
        (
            "C",
            "RP",
            "rpa",
            21,
            1,
            426,
            "12 08 82 41 00 01 01 aa 15 01",
        ),
        ("V", "M", "r0027", 26, 1, 1, "12 08 82 a1 00 1b 00 01 1a 01"),
        ("V", "M", "r0035", 26, 3, 1, "12 08 82 a3 00 23 00 01 1a 01"),
    ];
    for (area, block, variable, module, area_no, line, expected) in cases {
        let t = preset(area, block, variable, module);
        let mut p = t.parameters.clone();
        p.insert("area_no".into(), Value::from(area_no));
        p.insert("line".into(), Value::from(line));
        let addr = NativeNckAddress::from_parameters(&p)
            .unwrap_or_else(|e| panic!("{area}/{block}/{variable}: {e}"));
        let rp = resolve_native(addr).unwrap();
        assert_eq!(
            encode_var_spec(&rp.wire),
            bytes(expected),
            "{area}/{block}/{variable}/module{module}"
        );
    }
}

/// 驱动对象区：两条入口拆成独立模板，编号不同且各自显式。
#[test]
fn drive_object_area_splits_into_two_explicit_entries() {
    let reg = PresetRegistry::embedded();
    let v: Vec<_> = reg
        .for_family("828d")
        .into_iter()
        .filter(|t| t.parameters.get("area").and_then(|x| x.as_str()) == Some("V"))
        .collect();
    assert!(!v.is_empty());
    let modules: std::collections::BTreeSet<u64> = v
        .iter()
        .map(|t| t.parameters.get("module").unwrap().as_u64().unwrap())
        .collect();
    assert_eq!(
        modules.into_iter().collect::<Vec<_>>(),
        vec![26, 130],
        "驱动区必须同时提供 NC 变量入口与驱动对象入口"
    );
    // 两条入口的模板数相同（每条候选各生成一个）。
    let m26 = v
        .iter()
        .filter(|t| t.parameters.get("module").unwrap().as_u64() == Some(26))
        .count();
    let m130 = v
        .iter()
        .filter(|t| t.parameters.get("module").unwrap().as_u64() == Some(130))
        .count();
    assert_eq!(m26, m130);
}

/// `r0035` 的无效工程值规则**不在模板里自动生效**：它必须作为普通字段
/// 被复制进任务，缺失时不提供工程语义保护（诚实边界）。
#[test]
fn invalid_rule_is_explicit_configuration_not_a_template_privilege() {
    let t = preset("V", "M", "r0035", 26);
    assert!(
        !t.parameters.contains_key("invalid_value"),
        "模板不隐式携带无效值规则"
    );
    let mut p = t.parameters.clone();
    p.insert("area_no".into(), Value::from(1));
    p.insert("line".into(), Value::from(1));
    let rp = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
    assert!(rp.invalid_rule.is_none(), "未配置即无规则");
    // 显式配置后才生效。
    p.insert("invalid_value".into(), json!(-200.0));
    p.insert("invalid_tolerance".into(), json!(0.001));
    let rp = resolve_native(NativeNckAddress::from_parameters(&p).unwrap()).unwrap();
    let rule = rp.invalid_rule.expect("显式配置后规则随绑定");
    assert!(rule.hits(-200.0) && !rule.hits(33.0));
}
