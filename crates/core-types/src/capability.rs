//! Capability / Discovery / Control 契约（V2.1 §13, §20, §22）。

use serde::{Deserialize, Serialize};

use crate::schema::SchemaDescriptor;

/// Driver 能力（§13 capabilities）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DriverCapabilities {
    #[serde(default)]
    pub poll: bool,
    #[serde(default)]
    pub subscribe: bool,
    #[serde(default)]
    pub browse: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub method: bool,
    /// 是否支持事件（Event Plane §5）：老 Driver 缺字段即 false，正常工作。
    #[serde(default)]
    pub events: bool,
}

/// Discovery 能力（§20.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DiscoveryCapabilities {
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub browse: bool,
    #[serde(default)]
    pub import: bool,
}

/// Control 风险等级（§22）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Command 描述（§22.2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub id: String,
    pub label: crate::schema::LocalizedText,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: SchemaDescriptor,
    #[serde(default)]
    pub result_schema: SchemaDescriptor,
    #[serde(default = "default_risk")]
    pub risk: RiskLevel,
    #[serde(default)]
    pub confirmation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotent: bool,
}

fn default_risk() -> RiskLevel {
    RiskLevel::Low
}

/// Control 目录（§13）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ControlCatalog {
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
}

impl ControlCatalog {
    pub fn validate(&self) -> Result<(), String> {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for c in &self.commands {
            if c.id.trim().is_empty() {
                return Err("command id 不能为空".into());
            }
            if !seen.insert(&c.id) {
                return Err(format!("command id 重复: {}", c.id));
            }
            c.input_schema.validate()?;
            c.result_schema.validate()?;
        }
        Ok(())
    }
}
