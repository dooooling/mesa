//! 语义路径等价门（N8）：从**定义目录**出发逐条复现 62 条冻结 wire 字节。
//!
//! 覆盖边界文档 §5.1 矩阵的 **#38 / #39**。与旧门 `legacy_equivalence.rs` 的关系：
//!
//! - **旧门**（逐字不动）：原生预填模板 + 使用者确认的 `area_no`/`line` → `from_parameters`；
//! - **新门**（本文件）：`definitions/828d/r1.json` 的 `VariableDefinition` +
//!   `target`/`arguments` → 12 原生字段 → `from_parameters`。
//!
//! 两条路径**各自独立构造地址**，只与**同一份**冻结 wire 向量比较（62/62）；
//! 本文件**不引用**模板注册表（编译期即不可能读模板），因此它不可能退化为
//! 「旧门的复读」（§4.2 第 5 条禁止自证循环）。
//!
//! 本文件同样**不新增第二份 wire 真值**：只读 `tests/fixtures/legacy-828d-wire.json`。

use mesa_driver_sinumerik_nck::{
    AddressCompiler, DefinitionCatalog, GoldenRef, NativeFieldSource, POINT_KIND_VARIABLE,
    TargetKind, TargetRef, VariableDefinition, encode_var_spec,
};
use serde_json::{Map, Value, json};

/// 62 条冻结 wire 向量（**唯一**一份真值）。
const FIXTURE: &str = include_str!("fixtures/legacy-828d-wire.json");

fn catalog() -> DefinitionCatalog {
    DefinitionCatalog::embedded().expect("内嵌定义目录必须自检通过")
}

fn entries() -> Vec<Value> {
    let doc: Value = serde_json::from_str(FIXTURE).unwrap();
    doc["entries"].as_array().unwrap().clone()
}

fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|p| u8::from_str_radix(p, 16).unwrap())
        .collect()
}

/// 由「定义 + 冻结条目」独立构造语义输入。
///
/// `area_no` / `line` 的取值**只**来自冻结向量（真值方向：向量 → 输入），
/// 再按定义声明的 `addressing` 反向求出 `target` / `arguments`；
/// 这与「用模板参数拼地址」是两条互不依赖的路径。
fn semantic_input(d: &VariableDefinition, entry: &Value) -> (Value, Value) {
    let area_no = entry["area_no"].as_u64().unwrap();
    let line = entry["line"].as_u64().unwrap();
    let module = entry["module"].as_u64().unwrap();
    let mut target = Map::new();
    let mut arguments = Map::new();
    for (source, value) in [(&d.addressing.area_no, area_no), (&d.addressing.line, line)] {
        match source {
            NativeFieldSource::Target { kind } => match kind {
                TargetKind::Channel => {
                    target.insert("channel".into(), json!(value));
                }
                TargetKind::ModeGroup => {
                    target.insert("mode_group".into(), json!(value));
                }
                TargetKind::Axis => {
                    target.insert("axis".into(), json!(value));
                }
                TargetKind::Spindle => {
                    target.insert("spindle".into(), json!(value));
                }
                TargetKind::DriveObject => {
                    // 入口身份由冻结条目的 `entry` 字段给出（26/130 不可换算）。
                    let entry_name = entry["entry"].as_str().unwrap();
                    assert_eq!(
                        u64::from(
                            mesa_driver_sinumerik_nck::DriveEntry::parse(entry_name)
                                .unwrap()
                                .module()
                        ),
                        module,
                        "冻结条目的 entry 与 module 必须自洽"
                    );
                    let mut obj = Map::new();
                    obj.insert("number".into(), json!(value));
                    obj.insert("entry".into(), json!(entry_name));
                    target.insert("drive_object".into(), Value::Object(obj));
                }
            },
            NativeFieldSource::Argument { key } => {
                arguments.insert(key.clone(), json!(value));
            }
            NativeFieldSource::ArgumentOffset { key, offset } => {
                let base = value as i64 - i64::from(*offset);
                assert!(base >= 0, "冻结向量与偏移定义不自洽");
                arguments.insert(key.clone(), json!(base));
            }
            NativeFieldSource::Fixed { .. } | NativeFieldSource::DriveEntryModule => {}
        }
    }
    (Value::Object(target), Value::Object(arguments))
}

/// #38：**62/62 字节等价**（不得抽样、不得跳过）。
#[test]
fn semantic_path_reproduces_all_62_frozen_wire_bytes() {
    let cat = catalog();
    let all = entries();
    assert_eq!(all.len(), 62, "冻结向量条目数应为 42 + 20");
    let compiler = AddressCompiler::new(&cat, "828d");

    let mut checked = 0usize;
    for d in cat.iter() {
        for g in &d.golden_ref {
            let key = g.key();
            let entry = all
                .iter()
                .find(|e| {
                    e["area"].as_str().unwrap() == key.0
                        && e["block"].as_str().unwrap() == key.1
                        && e["variable"].as_str().unwrap() == key.2
                        && e["entry"].as_str().unwrap() == key.3.as_str()
                })
                .unwrap_or_else(|| panic!("{}：冻结向量缺条目 {key:?}", d.variable_id));
            let (target_json, args_json) = semantic_input(d, entry);
            let target = TargetRef::from_json(&target_json).expect("target 必须合法");
            let arguments = args_json.as_object().unwrap().clone();
            // 持久化 Key = Driver 重算的规范身份（先算出来，才能满足 D11）。
            let persisted = d.canonical_identity(&target, &arguments).unwrap();
            let params = json!({
                "variable_id": d.variable_id,
                "definition_revision": d.revision,
                "target": target_json,
                "arguments": args_json,
            });
            let compiled = compiler
                .compile_point(POINT_KIND_VARIABLE, params.as_object().unwrap(), &persisted)
                .unwrap_or_else(|e| panic!("{}@{key:?} 必须可编译：{e}", d.variable_id));

            // ① 线缆字节逐字节相等。
            assert_eq!(
                encode_var_spec(&compiled.resolved.wire),
                bytes(entry["wire_hex"].as_str().unwrap()),
                "{}@{key:?} 线缆字节与冻结向量不一致",
                d.variable_id
            );
            // ② 语义载荷 → 12 原生字段的映射与冻结条目**逐字段**一致。
            assert_eq!(
                compiled.address.module,
                entry["module"].as_u64().unwrap() as u8,
                "{}@{key:?} module",
                d.variable_id
            );
            assert_eq!(
                compiled.address.column,
                entry["column"].as_u64().unwrap() as u16,
                "{}@{key:?} column",
                d.variable_id
            );
            assert_eq!(
                compiled.address.decode.decode_name(),
                entry["decode"].as_str().unwrap(),
                "{}@{key:?} decode",
                d.variable_id
            );
            assert_eq!(
                format!("0x{:02x}", compiled.address.transport_size),
                entry["transport_size"].as_str().unwrap(),
                "{}@{key:?} transport",
                d.variable_id
            );
            assert_eq!(
                u64::from(compiled.address.count),
                entry["count"].as_u64().unwrap(),
                "{}@{key:?} count",
                d.variable_id
            );
            assert_eq!(
                compiled.address.area.letter().to_string(),
                entry["area"].as_str().unwrap(),
                "{}@{key:?} area",
                d.variable_id
            );
            assert_eq!(
                u64::from(compiled.address.area_no),
                entry["area_no"].as_u64().unwrap(),
                "{}@{key:?} area_no",
                d.variable_id
            );
            assert_eq!(
                u64::from(compiled.address.line),
                entry["line"].as_u64().unwrap(),
                "{}@{key:?} line",
                d.variable_id
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 62, "必须逐条核对，不得跳过");
}

/// #39：**G4 双入口不折叠**——20 对同 column、module 不同、wire 不同。
#[test]
fn semantic_path_keeps_the_two_entries_apart() {
    let cat = catalog();
    let compiler = AddressCompiler::new(&cat, "828d");
    let all = entries();
    let mut pairs = 0usize;
    for d in cat.iter() {
        if !d.targets.contains(&TargetKind::DriveObject) {
            continue;
        }
        let mut compiled_wire: Vec<(String, u8, Vec<u8>)> = Vec::new();
        for g in &d.golden_ref {
            let key = g.key();
            let entry = all
                .iter()
                .find(|e| {
                    e["area"].as_str().unwrap() == key.0
                        && e["block"].as_str().unwrap() == key.1
                        && e["variable"].as_str().unwrap() == key.2
                        && e["entry"].as_str().unwrap() == key.3.as_str()
                })
                .unwrap();
            let (target_json, args_json) = semantic_input(d, entry);
            let target = TargetRef::from_json(&target_json).unwrap();
            let arguments = args_json.as_object().unwrap().clone();
            let persisted = d.canonical_identity(&target, &arguments).unwrap();
            let params = json!({
                "variable_id": d.variable_id,
                "definition_revision": d.revision,
                "target": target_json,
                "arguments": args_json,
            });
            let compiled = compiler
                .compile_point(POINT_KIND_VARIABLE, params.as_object().unwrap(), &persisted)
                .unwrap();
            compiled_wire.push((
                key.3.as_str().to_string(),
                compiled.address.module,
                encode_var_spec(&compiled.resolved.wire),
            ));
            // 同一变量同 column。
            assert_eq!(
                compiled.address.column,
                entry["column"].as_u64().unwrap() as u16
            );
        }
        assert_eq!(compiled_wire.len(), 2, "{}：必须有两条入口", d.variable_id);
        assert_eq!(compiled_wire[0].1, 26);
        assert_eq!(compiled_wire[1].1, 130);
        assert_ne!(
            compiled_wire[0].2, compiled_wire[1].2,
            "两条入口不得产生同一请求"
        );
        pairs += 1;
    }
    assert_eq!(pairs, 20, "20 个驱动参数各覆盖两条入口");
}

/// 语义身份的可追溯性：每个 `GoldenRef` 都能唯一命中定义（与 `definition_catalog` 互证）。
#[test]
fn golden_ref_lookup_is_unique_for_every_definition() {
    let cat = catalog();
    for d in cat.iter() {
        for g in &d.golden_ref {
            let hits = cat.find_by_golden_ref(&GoldenRef {
                area: g.area.clone(),
                block: g.block.clone(),
                variable: g.variable.clone(),
                entry: g.entry,
            });
            assert_eq!(hits.len(), 1, "{}：{g:?}", d.variable_id);
        }
    }
}
