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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    /// 校验 Schema 定义本身是否合法（Descriptor 静态契约）：
    /// key 唯一/非空、enum 选项唯一、default 类型一致、visible_if 引用存在。
    pub fn validate_definition(&self) -> Result<(), String> {
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
            // pattern 必须为合法 regex（定义期拦截，instance 期不再容忍）
            if let Some(pat) = &f.validation.pattern
                && regex::Regex::new(pat).is_err()
            {
                return Err(format!("field {} pattern 非法 regex: {pat}", f.key));
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

    /// 校验用户输入实例是否合法（Core 唯一 Schema Validator）：
    /// required、JSON 类型、enum、min/max、pattern（真 regex）、未知字段。
    /// 路径以 `root` 为前缀（如 `connection.host`）；返回全部问题（不短路），
    /// 空 Vec 表示通过。调用方（Validate Connection / Create / Update Endpoint /
    /// ResourceSelection / Event 参数）必须全部走本函数，不得各写一套。
    pub fn validate_instance(&self, root: &str, value: &serde_json::Value) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let Some(obj) = value.as_object() else {
            issues.push(ValidationIssue {
                path: root.into(),
                code: "INVALID_TYPE".into(),
                message: format!("{root} must be an object"),
            });
            return issues;
        };
        for field in &self.fields {
            let path = format!("{root}.{}", field.key);
            let Some(val) = obj.get(&field.key) else {
                if field.required {
                    issues.push(ValidationIssue {
                        path,
                        code: "REQUIRED".into(),
                        message: format!("field `{}` is required", field.key),
                    });
                }
                continue;
            };
            // Secret 持久化标记 {"secret_set": true} 视为已提供，不校验内容
            if field.field_type == FieldType::Secret && is_secret_marker(val) {
                continue;
            }
            if !field_type_matches(field.field_type, val) {
                issues.push(ValidationIssue {
                    path,
                    code: "INVALID_TYPE".into(),
                    message: format!(
                        "field `{}` expected {:?}, got {}",
                        field.key, field.field_type, val
                    ),
                });
                continue;
            }
            if let Some(opts) = &field.validation.enum_options
                && let Some(s) = val.as_str()
                && !opts.iter().any(|o| o == s)
            {
                issues.push(ValidationIssue {
                    path: path.clone(),
                    code: "INVALID_ENUM".into(),
                    message: format!("field `{}` value `{s}` not in {opts:?}", field.key),
                });
            }
            if let Some(num) = val.as_f64() {
                if let Some(min) = field.validation.min
                    && num < min
                {
                    issues.push(ValidationIssue {
                        path: path.clone(),
                        code: "OUT_OF_RANGE".into(),
                        message: format!("field `{}` {num} < min {min}", field.key),
                    });
                }
                if let Some(max) = field.validation.max
                    && num > max
                {
                    issues.push(ValidationIssue {
                        path: path.clone(),
                        code: "OUT_OF_RANGE".into(),
                        message: format!("field `{}` {num} > max {max}", field.key),
                    });
                }
            }
            // 真 regex（fail-closed：pattern 非法即配置错误，由定义校验拦截；
            // 此处若编译失败视为不匹配并报告）。
            if let Some(pat) = &field.validation.pattern
                && let Some(s) = val.as_str()
            {
                match regex::Regex::new(pat) {
                    Ok(re) => {
                        if !re.is_match(s) {
                            issues.push(ValidationIssue {
                                path: path.clone(),
                                code: "PATTERN_MISMATCH".into(),
                                message: format!("field `{}` value `{s}` 不匹配 {pat}", field.key),
                            });
                        }
                    }
                    Err(e) => issues.push(ValidationIssue {
                        path: path.clone(),
                        code: "INVALID_PATTERN".into(),
                        message: format!("field `{}` pattern 非法: {e}", field.key),
                    }),
                }
            }
        }
        // 未知字段（拼写错误早发现，不静默吞掉）
        for key in obj.keys() {
            if !self.fields.iter().any(|f| &f.key == key) {
                issues.push(ValidationIssue {
                    path: format!("{root}.{key}"),
                    code: "UNKNOWN_FIELD".into(),
                    message: format!("unknown field `{key}`"),
                });
            }
        }
        issues
    }
}

/// 单个校验问题（Core 唯一形状；`path` 含调用方前缀）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// Secret 持久化标记 {"secret_set": true}：已存值，不校验内容。
fn is_secret_marker(v: &serde_json::Value) -> bool {
    v.is_object()
        && v.as_object()
            .map(|m| m.get("secret_set") == Some(&serde_json::Value::Bool(true)))
            .unwrap_or(false)
}

/// JSON 值与 FieldType 一致性（与 definition 校验同口径）。
fn field_type_matches(t: FieldType, val: &serde_json::Value) -> bool {
    match t {
        FieldType::String
        | FieldType::Host
        | FieldType::Url
        | FieldType::File
        | FieldType::CertificateRef
        | FieldType::Secret => val.is_string(),
        FieldType::Integer | FieldType::Port => val.is_number() && val.as_i64().is_some(),
        FieldType::Number | FieldType::Duration => val.is_number(),
        FieldType::Boolean => val.is_boolean(),
        FieldType::Enum => val.is_string(),
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
