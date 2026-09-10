//! DriverDescriptor 契约（V2.1 §13）。

use serde::{Deserialize, Serialize};

use crate::capability::{ControlCatalog, DriverCapabilities, ResourceSelectionMethod};
use crate::resource::ResourceDescriptor;
use crate::schema::SchemaDescriptor;

/// Descriptor 契约版本（§4.2）：V2 删除/改变 wire shape
///（`data_type→type_spec`、`discovery→resource_selection_methods`、
/// 删除 `capabilities.browse`），Major 必须升级。禁止魔数，全部引用本常量。
pub const DESCRIPTOR_CONTRACT_MAJOR: u32 = 2;
pub const DESCRIPTOR_CONTRACT_MINOR: u32 = 0;

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
    /// 资源配置方式（§20.1）：唯一真值，可扩展枚举。
    #[serde(default)]
    pub resource_selection_methods: Vec<ResourceSelectionMethod>,
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
        self.connection.validate_definition()?;
        // resource_selection_methods 唯一（可组合，不去重即契约非法）
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for m in &self.resource_selection_methods {
                if !seen.insert(m) {
                    return Err(format!("resource_selection_methods 重复: {m:?}"));
                }
            }
        }
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
