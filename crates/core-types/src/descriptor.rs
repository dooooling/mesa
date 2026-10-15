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

/// 连接 `timeout_ms` 的支持上限（C1 §6.2 / §6.3）。
///
/// 为什么需要 Core 级常量：Probe 的外层总预算按「阶段数 × `timeout_ms`」推导，
/// 没有统一上界时用户填一个极大值会让探测预算无界；同时 Descriptor
/// （UI / Core Schema 校验）与各 Driver parser 必须给出**同一套**范围，
/// 否则会出现「保存通过、运行才失败」或「UI 显示的值实际不生效」。
///
/// 语义：超过本值的配置在**访问网络之前**被拒绝（Core Schema 400 /
/// Driver `BAD_CONFIG`），而不是被外层预算静默截断。
///
/// NOTE: 该值是「由阶段模型推出的工程上界」，不是真机实测结论；
/// 真机分阶段量测（A22）后应复核并在此处更新。
pub const CONNECTION_TIMEOUT_MAX_MS: u64 = 10_000;
/// 连接 `timeout_ms` 的支持下限（低于此值的局域网抖动误超时概率显著上升）。
pub const CONNECTION_TIMEOUT_MIN_MS: u64 = 500;

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
        // NOTE：resources modes ↔ capabilities 的交叉检查故意不在此做——
        // Descriptor 2.0 definition validity 已在 PR22–PR23 冻结，事后收紧即
        // Major 变更；执行能力检查归 `validate_task_set_against`（配置能否被
        // 该 Driver 执行），见 resource.rs。
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
