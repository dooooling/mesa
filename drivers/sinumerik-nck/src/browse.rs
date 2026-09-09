//! NCK 虚拟浏览树（ADR 0001 §19：Catalog + Topology → `nck://` 树）。
//!
//! 层级（parent → children）：
//! ```text
//! "" → Area 组（nck://C …，仅含内容的 Area 才出现，空 Catalog 即空根）
//! nck://<A> → Block 组（nck://C/SEMA）+ 实例（nck://C/1 通道 / nck://A/3 轴，拓扑有时）
//! nck://<A>/<B> → 变量叶（canonical key 为 id，binding 可直接配任务）
//! nck://C/<n> → 该通道 Area C 变量（binding 预填 area_no=n）
//! nck://A/<i> → 该轴 Area A 变量（binding 预填 line=i）
//! ```
//!
//! 实例→字段映射（area_no/line 预填）沿用 `codec::resolve` 的 V1 假设，
//! 待真机确认；叶 binding 的回环可用性由测试锁定（binding → configure 同 wire）。

use std::collections::BTreeSet;

use mesa_driver_protocol::pb::BrowseNode;

use crate::address::NckArea;
use crate::catalog::{NckCatalog, NckVariableDefinition};
use crate::topology::{NckTopology, axis_path, channel_path};

/// Area 展示名（用户字母 + Siemens 语义，不含任何 wire 数字）。
fn area_label(area: NckArea) -> &'static str {
    match area {
        NckArea::Nck => "NCK 系统",
        NckArea::ModeGroup => "方式组 (B)",
        NckArea::Channel => "通道 (C)",
        NckArea::Axis => "轴 (A)",
        NckArea::Tool => "刀具 (T)",
        NckArea::FeedDrive => "进给驱动 (V)",
        NckArea::MainDrive => "主轴驱动 (H)",
    }
}

fn area_id(area: NckArea) -> String {
    format!("nck://{}", area.letter())
}

fn block_id(area: NckArea, block: &str) -> String {
    format!("nck://{}/{block}", area.letter())
}

/// 变量叶 binding（`ResourceSelection` 片段：resource_id + parameters；
/// point_key 由用户在加入任务时填写，不在此伪造）。
fn leaf_binding(
    def: &NckVariableDefinition,
    preset: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let mut params = serde_json::Map::new();
    params.insert(
        "area".into(),
        serde_json::json!(def.area.letter().to_string()),
    );
    params.insert("block".into(), serde_json::json!(def.block));
    params.insert("variable".into(), serde_json::json!(def.variable));
    params.insert("count".into(), serde_json::json!(1));
    params.insert("unit_mode".into(), serde_json::json!("current"));
    if def.wire.column != 0 {
        params.insert("column".into(), serde_json::json!(def.wire.column));
    }
    for (k, v) in preset {
        params.insert(k.clone(), v.clone());
    }
    serde_json::json!({"resource_id": "variable", "parameters": params}).to_string()
}

fn leaf_node(
    def: &NckVariableDefinition,
    preset: &serde_json::Map<String, serde_json::Value>,
) -> BrowseNode {
    // canonical key 需完整参数（含实例预填），与 configure 口径一致。
    let mut full = preset.clone();
    full.insert(
        "area".into(),
        serde_json::json!(def.area.letter().to_string()),
    );
    full.insert("block".into(), serde_json::json!(def.block));
    full.insert("variable".into(), serde_json::json!(def.variable));
    full.insert("count".into(), serde_json::json!(1));
    full.insert("unit_mode".into(), serde_json::json!("current"));
    if def.wire.column != 0 && !full.contains_key("column") {
        full.insert("column".into(), serde_json::json!(def.wire.column));
    }
    let id = crate::address::NckVariableRef::from_parameters(&full)
        .map(|r| r.canonical_key())
        .unwrap_or_else(|_| format!("{}/{}/{}", def.area.letter(), def.block, def.variable));
    BrowseNode {
        id,
        label: format!("{}/{} {}", def.block, def.variable, def.data_type),
        kind: "variable".into(),
        data_type: def.data_type.clone(),
        access: "read".into(),
        has_children: false,
        binding_json: leaf_binding(def, preset),
    }
}

/// 建树（纯函数；未知 parent → 空页，不报错——浏览是探索，不是寻址）。
pub fn build_tree(
    catalog: &NckCatalog,
    topology: Option<&NckTopology>,
    parent: &str,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> (Vec<BrowseNode>, Option<String>) {
    let vars = catalog.variables();
    let mut nodes: Vec<BrowseNode> = if parent.is_empty() {
        root_nodes(&vars, topology)
    } else if let Some(area) = parse_area_path(parent) {
        area_nodes(&vars, topology, area)
    } else if let Some(n) = parse_channel_path(parent) {
        // 实例路径优先于 block（block 名为纯数字时实例胜出；Siemens
        // Block 名恒为字母（如 SEMA），冲突仅理论存在）。
        instance_nodes(&vars, NckArea::Channel, "area_no", n.into())
    } else if let Some(i) = parse_axis_path(parent) {
        instance_nodes(&vars, NckArea::Axis, "line", i.into())
    } else if let Some((area, block)) = parse_block_path(parent) {
        block_nodes(&vars, area, &block, &serde_json::Map::new())
    } else {
        Vec::new()
    };
    // 过滤（展示名或身份命中其一即可，与 opcua 同口径）。
    if !filter.is_empty() {
        nodes.retain(|n| n.label.contains(filter) || n.id.contains(filter));
    }
    // 分页（cursor 为偏移；非法 cursor 视为 0，不炸整页）。
    let start = cursor.parse::<usize>().unwrap_or(0).min(nodes.len());
    let lim = if limit == 0 { 50 } else { limit as usize };
    let end = (start + lim).min(nodes.len());
    let next_cursor = if end < nodes.len() {
        Some(end.to_string())
    } else {
        None
    };
    (nodes[start..end].to_vec(), next_cursor)
}

/// 根：有内容的 Area 组（排序稳定：字母序）。
fn root_nodes(vars: &[&NckVariableDefinition], topology: Option<&NckTopology>) -> Vec<BrowseNode> {
    let mut areas: BTreeSet<char> = vars.iter().map(|d| d.area.letter()).collect();
    if let Some(t) = topology {
        if !t.channels.is_empty() {
            areas.insert(NckArea::Channel.letter());
        }
        if !t.axis_names().is_empty() {
            areas.insert(NckArea::Axis.letter());
        }
    }
    areas
        .into_iter()
        .filter_map(|l| {
            let area = NckArea::parse(&l.to_string()).ok()?;
            Some(BrowseNode {
                id: area_id(area),
                label: area_label(area).into(),
                kind: "area".into(),
                data_type: String::new(),
                access: "read".into(),
                has_children: true,
                binding_json: "{}".into(),
            })
        })
        .collect()
}

/// Area 下：Block 组（字母序）+ 拓扑实例（通道/轴，数字序）。
fn area_nodes(
    vars: &[&NckVariableDefinition],
    topology: Option<&NckTopology>,
    area: NckArea,
) -> Vec<BrowseNode> {
    let mut nodes = Vec::new();
    let blocks: BTreeSet<&str> = vars
        .iter()
        .filter(|d| d.area == area)
        .map(|d| d.block.as_str())
        .collect();
    for b in blocks {
        nodes.push(BrowseNode {
            id: block_id(area, b),
            label: b.into(),
            kind: "block".into(),
            data_type: String::new(),
            access: "read".into(),
            has_children: true,
            binding_json: "{}".into(),
        });
    }
    if let Some(t) = topology {
        match area {
            NckArea::Channel => {
                let mut ch: Vec<u16> = t.channels.iter().map(|c| c.number).collect();
                ch.sort_unstable();
                for n in ch {
                    let name = t.channel(n).map(|c| c.name.as_str()).unwrap_or("");
                    nodes.push(BrowseNode {
                        id: channel_path(n),
                        label: if name.is_empty() {
                            format!("Channel {n}")
                        } else {
                            format!("Channel {n} ({name})")
                        },
                        kind: "channel".into(),
                        data_type: String::new(),
                        access: "read".into(),
                        has_children: vars.iter().any(|d| d.area == area),
                        binding_json: "{}".into(),
                    });
                }
            }
            NckArea::Axis => {
                let mut idx: Vec<u16> = t.axis_names().keys().copied().collect();
                idx.sort_unstable();
                for i in idx {
                    let name = t.axis_names().get(&i).copied().unwrap_or("");
                    nodes.push(BrowseNode {
                        id: axis_path(i),
                        label: if name.is_empty() {
                            format!("Axis {i}")
                        } else {
                            format!("Axis {i} ({name})")
                        },
                        kind: "axis".into(),
                        data_type: String::new(),
                        access: "read".into(),
                        has_children: vars.iter().any(|d| d.area == area),
                        binding_json: "{}".into(),
                    });
                }
            }
            _ => {}
        }
    }
    nodes
}

/// Block 下：变量叶（block/variable 排序）。
fn block_nodes(
    vars: &[&NckVariableDefinition],
    area: NckArea,
    block: &str,
    preset: &serde_json::Map<String, serde_json::Value>,
) -> Vec<BrowseNode> {
    let mut defs: Vec<&&NckVariableDefinition> = vars
        .iter()
        .filter(|d| d.area == area && d.block == block)
        .collect();
    defs.sort_by(|a, b| a.variable.cmp(&b.variable));
    defs.into_iter().map(|d| leaf_node(d, preset)).collect()
}

/// 实例下：该 Area 全部变量，binding 预填实例字段。
fn instance_nodes(
    vars: &[&NckVariableDefinition],
    area: NckArea,
    field: &str,
    value: serde_json::Value,
) -> Vec<BrowseNode> {
    let mut preset = serde_json::Map::new();
    preset.insert(field.into(), value);
    let mut defs: Vec<&&NckVariableDefinition> = vars.iter().filter(|d| d.area == area).collect();
    defs.sort_by(|a, b| (&a.block, &a.variable).cmp(&(&b.block, &b.variable)));
    defs.into_iter().map(|d| leaf_node(d, &preset)).collect()
}

// ---------------------------------------------------------------------------
// 路径解析（变量 canonical key 恒为 4 段，实例路径恒为 2 段，不碰撞）
// ---------------------------------------------------------------------------

fn parse_area_path(parent: &str) -> Option<NckArea> {
    let rest = parent.strip_prefix("nck://")?;
    if rest.contains('/') {
        return None;
    }
    NckArea::parse(rest).ok()
}

fn parse_block_path(parent: &str) -> Option<(NckArea, String)> {
    let rest = parent.strip_prefix("nck://")?;
    let mut parts = rest.splitn(2, '/');
    let area = NckArea::parse(parts.next()?).ok()?;
    let block = parts.next()?;
    if block.is_empty() || block.contains('/') {
        return None;
    }
    Some((area, block.to_string()))
}

fn parse_channel_path(parent: &str) -> Option<u16> {
    let rest = parent.strip_prefix("nck://C/")?;
    if rest.contains('/') {
        return None;
    }
    rest.parse().ok()
}

fn parse_axis_path(parent: &str) -> Option<u16> {
    let rest = parent.strip_prefix("nck://A/")?;
    if rest.contains('/') {
        return None;
    }
    rest.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> NckCatalog {
        NckCatalog::from_json(&serde_json::json!({"variables": [
            {"area": "C", "block": "SEMA", "variable": "actFeedRate",
             "data_type": "F64", "shape": "lines", "unit": "mm/min",
             "wire": {"module": 18, "column": 42, "transport_size": 4, "element_size": 8}},
            {"area": "C", "block": "SEMA", "variable": "actSpindleSpeed",
             "data_type": "F64", "shape": "lines",
             "wire": {"module": 18, "column": 43, "transport_size": 4, "element_size": 8}},
            {"area": "N", "block": "NCK", "variable": "sysClock",
             "data_type": "U32", "shape": "scalar",
             "wire": {"module": 3, "column": 0, "transport_size": 6, "element_size": 4}},
        ]}))
        .unwrap()
    }

    #[test]
    fn empty_catalog_yields_empty_root() {
        let (nodes, next) = build_tree(&NckCatalog::empty(), None, "", "", "", 0);
        assert!(nodes.is_empty());
        assert!(next.is_none());
    }

    #[test]
    fn root_area_block_leaf_navigation() {
        let cat = catalog();
        let (root, _) = build_tree(&cat, None, "", "", "", 0);
        assert_eq!(root.len(), 2, "C 与 N 有内容才出现");
        assert_eq!(root[0].id, "nck://C");
        assert_eq!(root[0].kind, "area");
        assert!(root[0].has_children);
        assert_eq!(root[1].id, "nck://N");

        let (blocks, _) = build_tree(&cat, None, "nck://C", "", "", 0);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, "nck://C/SEMA");

        let (leaves, _) = build_tree(&cat, None, "nck://C/SEMA", "", "", 0);
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].kind, "variable");
        assert_eq!(leaves[0].data_type, "F64");
        assert!(!leaves[0].has_children);
        // 叶 id 即 canonical key（无实例预填时 column 取 catalog 默认 42）。
        assert_eq!(
            leaves[0].id, "nck://C//SEMA/actFeedRate?column=42",
            "实际: {}",
            leaves[0].id
        );
    }

    #[test]
    fn leaf_binding_roundtrips_to_same_wire() {
        // browse 输出的 binding 必须可直接配任务：参数 → resolve 与直接一致。
        let cat = catalog();
        let (leaves, _) = build_tree(&cat, None, "nck://C/SEMA", "", "", 0);
        for leaf in &leaves {
            let b: serde_json::Value = serde_json::from_str(&leaf.binding_json).unwrap();
            assert_eq!(b["resource_id"], "variable");
            let params = b["parameters"].as_object().unwrap();
            let r = crate::address::NckVariableRef::from_parameters(params).unwrap();
            let def = cat.lookup(r.area, &r.block, &r.variable).unwrap();
            // Lines 形变量缺 line 时 resolve 必须 fail-closed（browse 不伪造实例）。
            assert!(crate::codec::resolve(&r, def).is_err());
        }
        // N 标量叶：binding 可直接 resolve（无需实例字段）。
        let (nleaves, _) = build_tree(&cat, None, "nck://N/NCK", "", "", 0);
        assert_eq!(nleaves.len(), 1);
        let b: serde_json::Value = serde_json::from_str(&nleaves[0].binding_json).unwrap();
        let params = b["parameters"].as_object().unwrap();
        let r = crate::address::NckVariableRef::from_parameters(params).unwrap();
        let def = cat.lookup(r.area, &r.block, &r.variable).unwrap();
        let (wire, _, _) = crate::codec::resolve(&r, def).unwrap();
        assert_eq!(
            crate::codec::encode_var_spec(&wire),
            vec![0x12, 0x08, 0x82, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x01]
        );
    }

    #[test]
    fn channel_instance_prefills_area_no() {
        use crate::topology::{NckChannel, NckTopology};
        let cat = catalog();
        let topo = NckTopology::from_channels(vec![NckChannel {
            number: 1,
            name: "CHAN1".into(),
            axes: vec![],
        }]);
        let (areas, _) = build_tree(&cat, Some(&topo), "", "", "", 0);
        assert!(areas.iter().any(|n| n.id == "nck://C"));
        let (children, _) = build_tree(&cat, Some(&topo), "nck://C", "", "", 0);
        let ch = children
            .iter()
            .find(|n| n.id == "nck://C/1")
            .expect("通道实例");
        assert_eq!(ch.kind, "channel");
        let (vars, _) = build_tree(&cat, Some(&topo), "nck://C/1", "", "", 0);
        assert_eq!(vars.len(), 2);
        let b: serde_json::Value = serde_json::from_str(&vars[0].binding_json).unwrap();
        assert_eq!(b["parameters"]["area_no"], 1);
        assert_eq!(vars[0].id, "nck://C/1/SEMA/actFeedRate?column=42");
    }

    #[test]
    fn filter_and_pagination() {
        let cat = catalog();
        let (hits, _) = build_tree(&cat, None, "nck://C/SEMA", "Spindle", "", 0);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].id.contains("actSpindleSpeed"));
        let (p1, next) = build_tree(&cat, None, "nck://C/SEMA", "", "", 1);
        assert_eq!(p1.len(), 1);
        let next = next.expect("应有下一页");
        let (p2, next2) = build_tree(&cat, None, "nck://C/SEMA", "", &next, 1);
        assert_eq!(p2.len(), 1);
        assert!(next2.is_none());
        assert_ne!(p1[0].id, p2[0].id);
        // 未知 parent → 空页（探索语义，不报错）。
        let (none, _) = build_tree(&cat, None, "nck://X", "", "", 0);
        assert!(none.is_empty());
    }
}
