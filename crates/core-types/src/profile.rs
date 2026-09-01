//! DeviceProfile 契约（V2.1 §10）。
//!
//! Profile 为 Driver 包资产，存于 `drivers/<driver>/profiles/*.json`，Core 仅加载与校验，
//! 不参与协议底层调用。`DeviceRecord.profile` 引用 `profile.id`。

use serde::{Deserialize, Serialize};

use crate::resource::ResourceSelection;
use crate::schema::LocalizedText;

/// 匹配规则（§10.2）：仅 eq/in/prefix，白名单字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchRule {
    pub field: String,
    pub op: String, // eq | in | prefix
    pub value: serde_json::Value,
}

/// 预设（§10.4）：一键展开为 ResourceSelection[]，再按 (mode, interval) 归并为 AcquisitionTask。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub label: LocalizedText,
    pub selections: Vec<ResourceSelection>,
    /// 可选的速率类（realtime/normal/slow），由前端映射为 interval
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_class: Option<String>,
}

/// DeviceProfile（§10.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub profile_version: u32,
    pub id: String,
    pub version: String,
    pub vendor: String,
    pub family: String,
    pub model: String,
    pub driver_id: String,
    #[serde(default)]
    pub match_rules: Vec<MatchRule>,
    #[serde(default)]
    pub connection_defaults: serde_json::Value,
    #[serde(default)]
    pub rate_classes: std::collections::HashMap<String, u64>,
    #[serde(default)]
    pub presets: Vec<Preset>,
}

impl DeviceProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("profile id 不能为空".into());
        }
        if self.driver_id.trim().is_empty() {
            return Err("driver_id 不能为空".into());
        }
        // match_rules 白名单校验
        let allowed_fields = ["driver_id", "probe.vendor", "probe.family", "probe.model", "probe.firmware"];
        let allowed_ops = ["eq", "in", "prefix"];
        for r in &self.match_rules {
            if !allowed_fields.contains(&r.field.as_str()) {
                return Err(format!("match_rule field 非法: {}", r.field));
            }
            if !allowed_ops.contains(&r.op.as_str()) {
                return Err(format!("match_rule op 非法: {}", r.op));
            }
        }
        // presets 唯一与展开校验
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for p in &self.presets {
                if p.id.trim().is_empty() {
                    return Err("preset id 不能为空".into());
                }
                if !seen.insert(&p.id) {
                    return Err(format!("preset id 重复: {}", p.id));
                }
                for sel in &p.selections {
                    if sel.resource_id.trim().is_empty() {
                        return Err(format!("preset {} resource_id 不能为空", p.id));
                    }
                    for out in &sel.outputs {
                        if out.point_key.trim().is_empty() {
                            return Err(format!("preset {} point_key 不能为空", p.id));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// 展开 Preset 为 ResourceSelection 列表（§10.4）
pub fn expand_preset(preset: &Preset) -> Vec<ResourceSelection> {
    preset.selections.clone()
}
