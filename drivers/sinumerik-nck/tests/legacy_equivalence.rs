//! 旧→新寻址等价门（PR-C 独立验收）。
//!
//! PR-B 删除了 `catalog/828d.json`。删除前已把每条候选定义的**期望线缆字节**
//! 冻结成 `tests/fixtures/legacy-828d-wire.json`（来源 `main@78e23ec`）。
//! 本测试用**新的**原生寻址路径（模板 + 使用者确认的 area_no/line）逐条复现
//! 这些字节：
//!
//! - 证明「删除目录」没有改变任何既有地址的物理含义（12 字节布局、module、
//!   column、area/unit 位、count 都不得漂移）；
//! - 证明两条入口（NC 变量 module 26 / 驱动对象 module 130）各自保持自己的
//!   编号，不被折叠或换算；
//! - 用**冻结的旧数据**做真值，不是用新代码自证新代码。
//!
//! 注意：等价只说明地址字节一致，**不**说明变量的工程语义已验证。
use mesa_driver_sinumerik_nck::{
    NativeNckAddress, PresetRegistry, encode_var_spec, resolve_native,
};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/legacy-828d-wire.json");

fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|p| u8::from_str_radix(p, 16).unwrap())
        .collect()
}

/// 按「系列.area.block.variable.入口」定位模板（入口由 module 唯一确定）。
fn preset_for(area: &str, block: &str, variable: &str, entry: &str) -> Value {
    let reg = PresetRegistry::embedded();
    let prefix = format!("828d.{area}.{block}.{variable}.");
    let suffix = format!(".{entry}");
    reg.for_family("828d")
        .into_iter()
        .find(|t| t.id.starts_with(&prefix) && t.id.ends_with(&suffix))
        .map(|t| serde_json::to_value(&t.parameters).expect("模板参数可序列化"))
        .unwrap_or_else(|| panic!("缺模板 {area}/{block}/{variable}/{entry}"))
}

#[test]
fn native_path_reproduces_every_frozen_catalog_address() {
    let doc: Value = serde_json::from_str(FIXTURE).expect("冻结向量必须是合法 JSON");
    assert!(
        doc["source"].as_str().unwrap().contains("78e23ec"),
        "必须记录冻结来源提交"
    );
    let entries = doc["entries"].as_array().expect("entries 数组");
    assert_eq!(
        entries.len(),
        62,
        "冻结向量条目数应为 42(NC变量) + 20(驱动对象)；条目丢失即等价门失效"
    );

    let mut checked = 0usize;
    for e in entries {
        let area = e["area"].as_str().unwrap();
        let block = e["block"].as_str().unwrap();
        let variable = e["variable"].as_str().unwrap();
        let entry = e["entry"].as_str().unwrap();

        // 模板只带结构固定字段；area_no/line 由「使用者确认」提供。
        let mut params = preset_for(area, block, variable, entry)
            .as_object()
            .expect("模板参数为对象")
            .clone();
        params.insert("area_no".into(), e["area_no"].clone());
        params.insert("line".into(), e["line"].clone());

        // 冻结向量里的固定字段必须与模板一致（防止模板数值被改过）。
        assert_eq!(
            params.get("module").and_then(|v| v.as_u64()),
            e["module"].as_u64(),
            "{area}/{block}/{variable}/{entry} module 漂移"
        );
        assert_eq!(
            params.get("column").and_then(|v| v.as_u64()),
            e["column"].as_u64(),
            "{area}/{block}/{variable}/{entry} column 漂移"
        );
        assert_eq!(
            params.get("decode").and_then(|v| v.as_str()),
            e["decode"].as_str(),
            "{area}/{block}/{variable}/{entry} decode 漂移"
        );
        assert_eq!(
            params.get("transport_size").and_then(|v| v.as_str()),
            e["transport_size"].as_str(),
            "{area}/{block}/{variable}/{entry} transport 漂移"
        );
        assert_eq!(
            params.get("count").and_then(|v| v.as_u64()),
            e["count"].as_u64(),
            "{area}/{block}/{variable}/{entry} count 漂移"
        );

        // 新路径必须产出与冻结目录映射完全相同的线缆字节。
        let addr = NativeNckAddress::from_parameters(&params)
            .unwrap_or_else(|err| panic!("{area}/{block}/{variable}/{entry}: {err}"));
        let rp = resolve_native(addr).unwrap();
        assert_eq!(
            encode_var_spec(&rp.wire),
            bytes(e["wire_hex"].as_str().unwrap()),
            "{area}/{block}/{variable}/{entry} 线缆字节与冻结目录不一致"
        );
        checked += 1;
    }
    assert_eq!(checked, 62, "必须逐条核对，不得跳过");
}

/// 冻结向量的两条入口必须各自独立（同列不同 module），不得被折叠。
#[test]
fn frozen_vectors_keep_the_two_entries_apart() {
    let doc: Value = serde_json::from_str(FIXTURE).unwrap();
    let entries = doc["entries"].as_array().unwrap();
    let drive: Vec<&Value> = entries
        .iter()
        .filter(|e| e["entry"] == "drive_object")
        .collect();
    assert_eq!(drive.len(), 20, "20 条候选声明了驱动对象入口");
    for e in drive {
        let nc = entries
            .iter()
            .find(|x| {
                x["area"] == e["area"]
                    && x["block"] == e["block"]
                    && x["variable"] == e["variable"]
                    && x["entry"] == "nc_variable"
            })
            .expect("同参数必须有 NC 变量入口");
        assert_eq!(nc["column"], e["column"], "同一参数的列必须一致");
        assert_ne!(nc["module"], e["module"], "两条入口的 module 必须不同");
        assert_ne!(nc["wire_hex"], e["wire_hex"], "两条入口不得产生同一请求");
    }
}
