//! DriverDescriptor 契约（V2.1 §13）。

use serde::{Deserialize, Serialize};

use crate::capability::{ControlCatalog, DiscoveryCapabilities, DriverCapabilities};
use crate::resource::ResourceDescriptor;
use crate::schema::SchemaDescriptor;

/// Driver 身份（§13 identity）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverIdentity {
    pub driver_id: String,
    pub name: String,
    pub version: String,
}

/// Driver 总描述符（§13）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverDescriptor {
    pub contract_major: u32,
    pub contract_minor: u32,
    pub identity: DriverIdentity,
    /// 连接参数 Schema（§13 connection）
    pub connection: SchemaDescriptor,
    #[serde(default)]
    pub resources: Vec<ResourceDescriptor>,
    #[serde(default)]
    pub controls: ControlCatalog,
    #[serde(default)]
    pub discovery: DiscoveryCapabilities,
    #[serde(default)]
    pub capabilities: DriverCapabilities,
    /// 事件目录（Event Plane §5）：serde(default) 保证老 Driver 无该字段时
    /// 按 empty 正常工作，Descriptor Major 不升级（backwards-compatible Minor）。
    #[serde(default)]
    pub events: crate::event::EventCatalog,
}

impl DriverDescriptor {
    /// 静态契约校验：字段/资源/output/命令 唯一性、default 类型、visible_if 引用等。
    pub fn validate(&self) -> Result<(), String> {
        if self.identity.driver_id.trim().is_empty() {
            return Err("identity.driver_id 不能为空".into());
        }
        self.connection.validate()?;
        // resources 唯一
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for r in &self.resources {
                if !seen.insert(&r.id) {
                    return Err(format!("resource id 重复: {}", r.id));
                }
                r.validate()?;
            }
        }
        self.controls.validate()?;
        self.events.validate()?;
        Ok(())
    }
}

/// 便捷：从 DriverMetadata 生成最小 Identity
impl From<crate::DriverMetadata> for DriverIdentity {
    fn from(m: crate::DriverMetadata) -> Self {
        Self {
            driver_id: m.driver_id,
            name: m.name,
            version: m.version,
        }
    }
}
