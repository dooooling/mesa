//! 工程配置示例的机器复核（方案 §14 第 3 项）。
//!
//! `presets/examples-828d.json` 是给使用者**直接复制**的
//! `ResourceSelection.parameters`（已含确认过的 `area_no`/`line`）。
//! 本测试逐条复核其中的 `wire_hex` 与 `source_label`，使示例文档不可能与
//! 代码漂移；`EXAMPLES.md` 由同一份数据生成，不是另抄一遍。
//!
//! 边界：示例只保证**地址与请求字节**正确，不保证工程语义已验证。
use mesa_driver_sinumerik_nck::{NativeNckAddress, encode_var_spec, resolve_native};
use serde_json::Value;

const EXAMPLES: &str = include_str!("../presets/examples-828d.json");

fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|p| u8::from_str_radix(p, 16).unwrap())
        .collect()
}

#[test]
fn every_documented_example_resolves_to_its_recorded_wire_and_label() {
    let doc: Value = serde_json::from_str(EXAMPLES).expect("示例必须是合法 JSON");
    let list = doc["examples"].as_array().expect("examples 数组");
    assert_eq!(list.len(), 13, "示例条数变化必须显式确认");

    for e in list {
        let id = e["id"].as_str().unwrap();
        let params = e["parameters"].as_object().expect("parameters 对象");
        let addr = NativeNckAddress::from_parameters(params)
            .unwrap_or_else(|err| panic!("示例 {id} 参数非法: {err}"));
        let label = addr.source_label();
        let rp = resolve_native(addr).unwrap();
        assert_eq!(
            encode_var_spec(&rp.wire),
            bytes(e["wire_hex"].as_str().unwrap()),
            "示例 {id} 请求字节与文档不符"
        );
        assert_eq!(
            label,
            e["source_label"].as_str().unwrap(),
            "示例 {id} 地址标识与文档不符"
        );
    }
}

/// 示例必须覆盖方案 §14 要求的三类（普通 NC 变量 / SINAMICS 只读参数 / p0199 字符串），
/// 且两条驱动入口都在。
#[test]
fn examples_cover_the_required_categories() {
    let doc: Value = serde_json::from_str(EXAMPLES).unwrap();
    let ids: Vec<&str> = doc["examples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();

    assert!(
        ids.iter().any(|i| i.starts_with("828d.C.")),
        "缺通道 NC 变量"
    );
    assert!(ids.iter().any(|i| i.contains(".rpa.")), "缺 R 参数");
    assert!(
        ids.iter().any(|i| i.contains(".actToolBasePos.")),
        "缺轴变量"
    );
    assert!(
        ids.iter().any(|i| i.contains(".r0035.")),
        "缺 SINAMICS 只读参数"
    );
    assert!(ids.iter().any(|i| i.contains(".p0199.")), "缺驱动对象名");
    assert!(
        ids.iter().any(|i| i.ends_with(".drive_object")),
        "缺显式驱动对象入口示例"
    );
    assert!(
        ids.iter().any(|i| i.ends_with(".nc_variable")),
        "缺 NC 变量入口示例"
    );
}

/// 无效工程值规则必须是**示例里显式写出**的普通字段，而不是靠模板隐式提供。
#[test]
fn invalid_rule_is_visible_in_the_example_parameters() {
    let doc: Value = serde_json::from_str(EXAMPLES).unwrap();
    for e in doc["examples"].as_array().unwrap() {
        let id = e["id"].as_str().unwrap();
        if id.contains(".r0035.") {
            let p = e["parameters"].as_object().unwrap();
            assert_eq!(
                p.get("invalid_value").and_then(|v| v.as_f64()),
                Some(-200.0),
                "{id} 必须显式带无效值"
            );
            assert_eq!(
                p.get("invalid_tolerance").and_then(|v| v.as_f64()),
                Some(0.001),
                "{id} 必须显式带容差"
            );
        }
    }
}
