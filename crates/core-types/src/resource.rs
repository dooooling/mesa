//! Resource / Output 契约（V2.1 §14）。

use serde::{Deserialize, Serialize};

use crate::schema::{LocalizedText, SchemaDescriptor};
use crate::{DataType, TaskMode};

/// 访问模式（§3.2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    Read,
    Write,
    ReadWrite,
}

/// Output 契约（§3.2, §14）：逻辑数据能力的单个输出。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputDescriptor {
    pub id: String,
    pub label: LocalizedText,
    pub data_type: DataType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default = "default_access")]
    pub access: AccessMode,
}

fn default_access() -> AccessMode {
    AccessMode::Read
}

/// Resource 契约（§14）：用户/管理面的逻辑数据能力，非物理 Function。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceDescriptor {
    pub id: String,
    pub label: LocalizedText,
    /// 资源参数（§14 parameters Schema）
    #[serde(default)]
    pub parameters: SchemaDescriptor,
    #[serde(default)]
    pub outputs: Vec<OutputDescriptor>,
    /// 支持的采集模式（§14 modes），为空表示默认 poll。
    #[serde(default)]
    pub modes: Vec<TaskMode>,
}

impl ResourceDescriptor {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("resource id 不能为空".into());
        }
        self.parameters.validate()?;
        // outputs 唯一
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for o in &self.outputs {
                if o.id.trim().is_empty() {
                    return Err(format!("resource {} output id 不能为空", self.id));
                }
                if !seen.insert(&o.id) {
                    return Err(format!("resource {} output id 重复: {}", self.id, o.id));
                }
            }
        }
        // outputs 至少一个（管理面无输出的 resource 无意义）
        if self.outputs.is_empty() {
            return Err(format!("resource {} outputs 不能为空", self.id));
        }
        Ok(())
    }
}
