//! 配置预填模板（Preset）：**可选**的配置素材，不参与采集准入。
//!
//! 设计边界（V2.0 §4）：
//! - Preset 只提供**结构固定字段**的预填（area/module/column/count/unit_mode/
//!   decode/transport_size/unit）；`area_no` 与 `line` 一律留给使用者确认，
//!   不猜轴/通道/行索引语义；
//! - 模板是否可用**不影响**原生 `variable` 的 Configure/Poll：加载失败只让
//!   模板浏览为空并给出诊断，数据面照常工作；
//! - 模板数值来自已冻结的候选目录定义，保留 `validation_status=candidate`
//!   与来源说明，绝不宣称硬件已验证；
//! - 含 `drive_object_module` 的条目拆成两个独立模板（module 26 / module 130），
//!   因为两条入口实例编号不同，不得互相换算。
//!
//! 本模块不做：地址准入、设备探测、自动填充缺失必填项。

use mesa_driver_protocol::pb::BrowseNode;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::native::{NativeError, NativeNckAddress};

/// 内嵌模板（随驱动发布；解析失败只影响浏览）。
pub const EMBEDDED_828D: &str = include_str!("../presets/828d.json");

/// 单个预填模板。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PresetTemplate {
    pub id: String,
    pub family: String,
    pub label: String,
    /// 候选状态（当前恒 `candidate`；不得据此宣称硬件已验证）。
    pub validation_status: String,
    /// 结构固定字段的预填（**不含** area_no/line）。
    pub parameters: Map<String, Value>,
    /// 仍需使用者确认的字段（保持缺失，由通用必填校验阻止保存）。
    pub required_user_inputs: Vec<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct PresetDoc {
    #[serde(default)]
    templates: Vec<PresetTemplate>,
}

/// 只读模板集合；加载失败退化为空集并保留原因（不 panic、不阻塞采集）。
#[derive(Debug, Clone, Default)]
pub struct PresetRegistry {
    templates: Vec<PresetTemplate>,
    error: Option<String>,
}

impl PresetRegistry {
    /// 从 JSON 文本装载；非法即空集 + 诊断（**绝不**影响采集路径）。
    pub fn load(json: &str) -> Self {
        let doc: PresetDoc = match serde_json::from_str(json) {
            Ok(d) => d,
            Err(e) => {
                return Self {
                    templates: Vec::new(),
                    error: Some(format!("preset JSON 非法：{e}")),
                };
            }
        };
        let mut kept = Vec::new();
        let mut rejects = Vec::new();
        for t in doc.templates {
            match validate_template(&t) {
                Ok(()) => kept.push(t),
                Err(reason) => rejects.push(format!("{}: {reason}", t.id)),
            }
        }
        let error = (!rejects.is_empty()).then(|| {
            format!(
                "{} 个模板被拒绝（仅影响模板浏览）：{}",
                rejects.len(),
                rejects.join("; ")
            )
        });
        Self {
            templates: kept,
            error,
        }
    }

    /// 随驱动内嵌的模板集合。
    pub fn embedded() -> Self {
        Self::load(EMBEDDED_828D)
    }

    /// 加载诊断（模板不可用时给出原因；`None` 表示全部合法）。
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// 指定系列的模板（稳定按 id 排序）。
    pub fn for_family(&self, family: &str) -> Vec<&PresetTemplate> {
        let mut v: Vec<&PresetTemplate> = self
            .templates
            .iter()
            .filter(|t| t.family == family)
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
}

/// 模板结构校验：固定字段必须能通过**同一套**原生解析规则。
///
/// 做法：把固定字段与占位的最小合法 `area_no`/`line` 组合后跑一次
/// `NativeNckAddress::from_parameters`，任何不合法（枚举/范围/类型/
/// 无效值规则配对）都在装载期暴露，而不是等使用者填完才发现。
fn validate_template(t: &PresetTemplate) -> Result<(), String> {
    if t.id.trim().is_empty() || t.label.trim().is_empty() {
        return Err("id/label 不得为空".into());
    }
    if t.validation_status != "candidate" {
        return Err(format!(
            "validation_status 必须为 candidate（实际 `{}`）",
            t.validation_status
        ));
    }
    // 模板不得预填 area_no/line（必须由使用者确认）。
    for key in ["area_no", "line"] {
        if t.parameters.contains_key(key) {
            return Err(format!("模板不得预填 `{key}`（须使用者确认）"));
        }
    }
    if !t.required_user_inputs.iter().any(|k| k == "area_no")
        || !t.required_user_inputs.iter().any(|k| k == "line")
    {
        return Err("required_user_inputs 必须包含 area_no 与 line".into());
    }
    let mut probe = t.parameters.clone();
    probe.insert("area_no".into(), Value::from(0));
    probe.insert("line".into(), Value::from(0));
    NativeNckAddress::from_parameters(&probe)
        .map(|_| ())
        .map_err(|e: NativeError| e.to_string())
}

/// 模板 → 浏览叶（`binding_json` 只含固定字段；缺必填由通用校验阻止保存）。
pub fn leaf_node(t: &PresetTemplate) -> BrowseNode {
    leaf_node_with(t, &[])
}

/// 模板 → 浏览叶，并填入**已确认**的字段（例如身份核对出的实例编号）。
///
/// 只允许填 `area_no`/`line` 这类使用者本来要填的实例字段；填入后该字段
/// 从“待确认”列表移除，其余缺失项仍由通用必填校验阻止保存。
pub fn leaf_node_with(t: &PresetTemplate, filled: &[(&str, Value)]) -> BrowseNode {
    let mut parameters = t.parameters.clone();
    let mut pending: Vec<&str> = t.required_user_inputs.iter().map(|s| s.as_str()).collect();
    for (key, value) in filled {
        parameters.insert((*key).into(), value.clone());
        pending.retain(|k| k != key);
    }
    let binding = serde_json::json!({
        "resource_id": "variable",
        "parameters": parameters,
    })
    .to_string();
    let suffix = if pending.is_empty() {
        "字段完整".to_string()
    } else {
        format!("待确认：{}", pending.join("/"))
    };
    BrowseNode {
        id: format!("nck://presets/{}", t.id),
        label: format!("{} · {} · {}", t.label, t.validation_status, suffix),
        kind: "variable".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: binding,
    }
}

/// 目录节点（模板入口）。
pub fn folder_node(id: &str, label: &str) -> BrowseNode {
    BrowseNode {
        id: id.into(),
        label: label.into(),
        kind: "folder".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: true,
        binding_json: "{}".into(),
    }
}

/// 模板目录诊断节点（模板不可用时明确提示，而不是伪装成空目录）。
pub fn diagnostic(message: String) -> BrowseNode {
    BrowseNode {
        id: "nck://presets/note".into(),
        label: message,
        kind: "diagnostic".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: "{}".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_templates_load_and_are_candidates() {
        let reg = PresetRegistry::embedded();
        assert!(
            reg.error().is_none(),
            "内嵌模板必须全部合法：{:?}",
            reg.error()
        );
        let t = reg.for_family("828d");
        assert!(!t.is_empty(), "828d 应有模板");
        assert!(t.iter().all(|x| x.validation_status == "candidate"));
        // 每个模板都不得预填 area_no/line。
        assert!(t.iter().all(|x| {
            !x.parameters.contains_key("area_no") && !x.parameters.contains_key("line")
        }));
        // 其他系列暂无模板（不得套用 828D）。
        assert!(reg.for_family("840d-sl").is_empty());
    }

    #[test]
    fn every_embedded_template_fills_into_a_valid_native_address() {
        // 模板 + 使用者确认的 area_no/line 必须能组成合法原生地址。
        let reg = PresetRegistry::embedded();
        for t in reg.for_family("828d") {
            let mut p = t.parameters.clone();
            p.insert("area_no".into(), Value::from(1));
            p.insert("line".into(), Value::from(1));
            assert!(
                NativeNckAddress::from_parameters(&p).is_ok(),
                "模板 {} 无法组成合法地址",
                t.id
            );
        }
    }

    #[test]
    fn drive_object_entries_split_into_two_modules() {
        let reg = PresetRegistry::embedded();
        let both: Vec<_> = reg
            .for_family("828d")
            .into_iter()
            .filter(|t| t.parameters.get("column").and_then(|v| v.as_u64()) == Some(199))
            .collect();
        let modules: Vec<u64> = both
            .iter()
            .map(|t| t.parameters.get("module").unwrap().as_u64().unwrap())
            .collect();
        assert!(modules.contains(&26), "应有 NC 变量入口模板");
        assert!(modules.contains(&130), "应有驱动对象入口模板");
    }

    #[test]
    fn invalid_or_empty_preset_data_degrades_to_empty_registry() {
        // 非法 JSON。
        let reg = PresetRegistry::load("{not json");
        assert!(reg.is_empty());
        assert!(reg.error().is_some());
        // 空集合合法。
        let reg = PresetRegistry::load(r#"{"templates":[]}"#);
        assert!(reg.is_empty());
        assert!(reg.error().is_none());
        // 预填 area_no 的模板被拒。
        let reg = PresetRegistry::load(
            r#"{"templates":[{"id":"x","family":"828d","label":"L",
                "validation_status":"candidate",
                "parameters":{"area":"C","area_no":1,"module":21,"column":1,
                              "count":1,"unit_mode":"current","decode":"F64_LE",
                              "transport_size":"0x09"},
                "required_user_inputs":["area_no","line"]}]}"#,
        );
        assert!(reg.is_empty());
        assert!(reg.error().unwrap().contains("不得预填"));
        // 非法 decode 的模板被拒（同一套原生规则）。
        let reg = PresetRegistry::load(
            r#"{"templates":[{"id":"x","family":"828d","label":"L",
                "validation_status":"candidate",
                "parameters":{"area":"C","module":21,"column":1,"count":1,
                              "unit_mode":"current","decode":"REAL",
                              "transport_size":"0x09"},
                "required_user_inputs":["area_no","line"]}]}"#,
        );
        assert!(reg.is_empty());
        // 非 candidate 状态被拒（不得暗示已验证）。
        let reg = PresetRegistry::load(
            r#"{"templates":[{"id":"x","family":"828d","label":"L",
                "validation_status":"supported",
                "parameters":{"area":"C","module":21,"column":1,"count":1,
                              "unit_mode":"current","decode":"F64_LE",
                              "transport_size":"0x09"},
                "required_user_inputs":["area_no","line"]}]}"#,
        );
        assert!(reg.is_empty());
    }

    #[test]
    fn leaf_binding_carries_only_fixed_fields() {
        let reg = PresetRegistry::embedded();
        let t = reg.for_family("828d")[0];
        let node = leaf_node(t);
        assert!(node.id.starts_with("nck://presets/"));
        assert!(node.label.contains("candidate"));
        assert!(node.label.contains("待确认：area_no/line"));
        let binding: Value = serde_json::from_str(&node.binding_json).unwrap();
        assert_eq!(binding["resource_id"], "variable");
        assert!(binding["parameters"].get("area_no").is_none());
        assert!(binding["parameters"].get("line").is_none());
    }
}
