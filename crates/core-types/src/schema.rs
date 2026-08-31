//! Schema / Field 契约（V2.1 §12）。
//!
//! 受控子集：不实现完整 JSON Schema，仅 12 种 FieldType + 3 种 ConditionOp。
//! 所有校验在 `descriptor()` 返回后可静态断言，不进入 DataPlane 热路径。

use serde::{Deserialize, Serialize};

/// 本地化文本：稳定 ID 永远不翻译，展示文本支持多语言。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalizedText {
    pub default: String,
    #[serde(rename = "zh-CN", skip_serializing_if = "Option::is_none")]
    pub zh_cn: Option<String>,
}

impl LocalizedText {
    pub fn new(default: impl Into<String>) -> Self {
        Self {
            default: default.into(),
            zh_cn: None,
        }
    }
    pub fn with_zh(mut self, zh: impl Into<String>) -> Self {
        self.zh_cn = Some(zh.into());
        self
    }
}

impl From<String> for LocalizedText {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}
impl From<&str> for LocalizedText {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// V1 支持的字段类型（§12），禁止任意字符串类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    String,
    Integer,
    Number,
    Boolean,
    Enum,
    Secret,
    Duration,
    Host,
    Port,
    Url,
    File,
    #[serde(rename = "certificate_ref")]
    CertificateRef,
}

/// Condition 操作符（§12），仅 eq/neq/in。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConditionOp {
    Eq,
    Neq,
    In,
}

/// UI 可见性条件：field 必须引用同一 Schema 已存在字段（§12）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,
    pub op: ConditionOp,
    pub value: serde_json::Value,
}

/// UI 提示（§12），仅允许受控字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UiHints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advanced: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_if: Option<Condition>,
}

/// 字段校验（§12）：范围 / 正则 / 枚举选项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FieldValidation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_options: Option<Vec<String>>,
}

/// 单个字段描述（§12）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDescriptor {
    pub key: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub field_type: FieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub validation: FieldValidation,
    #[serde(default)]
    pub ui: UiHints,
}

/// Schema：字段集合（§12）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SchemaDescriptor {
    #[serde(default)]
    pub fields: Vec<FieldDescriptor>,
}

impl SchemaDescriptor {
    pub fn new(fields: Vec<FieldDescriptor>) -> Self {
        Self { fields }
    }

    /// 校验 Schema 的静态契约：key 唯一、enum 唯一、visible_if 引用存在等。
    pub fn validate(&self) -> Result<(), String> {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for f in &self.fields {
            if f.key.trim().is_empty() {
                return Err("field key 不能为空".to_string());
            }
            if !seen.insert(&f.key) {
                return Err(format!("field key 重复: {}", f.key));
            }
            if let Some(opts) = &f.validation.enum_options {
                let mut es = HashSet::new();
                for o in opts {
                    if !es.insert(o) {
                        return Err(format!("field {} enum option 重复: {}", f.key, o));
                    }
                }
                if f.field_type != FieldType::Enum && !opts.is_empty() {
                    // 允许非 Enum 也携带选项，但通常仅 Enum 需要
                }
            }
            // default 类型与 field_type 的轻量一致性（不做完整 JSON Schema 推导）
            if let Some(def) = &f.default {
                let ok = match f.field_type {
                    FieldType::String
                    | FieldType::Host
                    | FieldType::Url
                    | FieldType::File
                    | FieldType::CertificateRef
                    | FieldType::Secret => def.is_string(),
                    FieldType::Integer | FieldType::Port => {
                        def.is_number() && def.as_i64().is_some()
                    }
                    FieldType::Number | FieldType::Duration => def.is_number(),
                    FieldType::Boolean => def.is_boolean(),
                    FieldType::Enum => def.is_string(),
                };
                if !ok {
                    return Err(format!(
                        "field {} default 类型与 field_type {:?} 不匹配: {}",
                        f.key, f.field_type, def
                    ));
                }
            }
        }
        // visible_if 引用字段存在
        for f in &self.fields {
            if let Some(cond) = &f.ui.visible_if
                && !seen.contains(&cond.field)
            {
                return Err(format!(
                    "field {} visible_if 引用不存在的字段: {}",
                    f.key, cond.field
                ));
            }
        }
        Ok(())
    }
}

/// 便捷构造
impl FieldDescriptor {
    pub fn new(key: impl Into<String>, label: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            description: None,
            field_type,
            required: false,
            default: None,
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        }
    }
    pub fn required(mut self, v: bool) -> Self {
        self.required = v;
        self
    }
    pub fn default_value(mut self, v: serde_json::Value) -> Self {
        self.default = Some(v);
        self
    }
}
