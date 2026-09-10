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
    /// key 唯一/非空、enum 非空且仅 Enum 可带、min/max 仅数值型且 min<=max、
    /// pattern 仅 string-like 且合法 regex、default 必须同时满足 type/enum/
    /// min-max/pattern、Port/Duration 内禀约束、visible_if 引用存在。
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
            // enum_options：仅 Enum 可带，且必须非空、选项唯一
            match &f.validation.enum_options {
                Some(opts) => {
                    if f.field_type != FieldType::Enum {
                        return Err(format!("field {} 非 Enum 不得带 enum_options", f.key));
                    }
                    if opts.is_empty() {
                        return Err(format!("field {} enum_options 不得为空", f.key));
                    }
                    let mut es = HashSet::new();
                    for o in opts {
                        if !es.insert(o) {
                            return Err(format!("field {} enum option 重复: {}", f.key, o));
                        }
                    }
                }
                None => {
                    if f.field_type == FieldType::Enum {
                        return Err(format!("field {} Enum 必须声明 enum_options", f.key));
                    }
                }
            }
            // min/max：仅数值型（Integer/Number/Port/Duration）可带，且 min<=max
            if (f.validation.min.is_some() || f.validation.max.is_some())
                && !matches!(
                    f.field_type,
                    FieldType::Integer | FieldType::Number | FieldType::Port | FieldType::Duration
                )
            {
                return Err(format!("field {} 非数值型不得带 min/max", f.key));
            }
            if let (Some(min), Some(max)) = (f.validation.min, f.validation.max)
                && min > max
            {
                return Err(format!("field {} min {min} > max {max}", f.key));
            }
            // min/max 不得放宽 Port/Duration 内禀范围（自相矛盾的约束直接非法）
            if let Some((lo, hi, name)) = intrinsic_range(f.field_type) {
                if let Some(min) = f.validation.min
                    && min < lo
                {
                    return Err(format!("field {} min {min} 放宽{name}内禀下界 {lo}", f.key));
                }
                if let Some(max) = f.validation.max
                    && max > hi
                {
                    return Err(format!("field {} max {max} 放宽{name}内禀上界 {hi}", f.key));
                }
            }
            // pattern：仅 string-like 可带，且必须为合法 regex
            if let Some(pat) = &f.validation.pattern {
                if !matches!(
                    f.field_type,
                    FieldType::String
                        | FieldType::Host
                        | FieldType::Url
                        | FieldType::File
                        | FieldType::CertificateRef
                        | FieldType::Secret
                ) {
                    return Err(format!("field {} 非字符串型不得带 pattern", f.key));
                }
                if regex::Regex::new(pat).is_err() {
                    return Err(format!("field {} pattern 非法 regex: {pat}", f.key));
                }
            }
            // default：必须同时满足 type/enum/min-max/pattern（含 Port/Duration 内禀）
            if let Some(def) = &f.default {
                if !field_type_matches(f.field_type, def) {
                    return Err(format!(
                        "field {} default 类型与 field_type {:?} 不匹配: {}",
                        f.key, f.field_type, def
                    ));
                }
                if let Some(opts) = &f.validation.enum_options
                    && let Some(s) = def.as_str()
                    && !opts.iter().any(|o| o == s)
                {
                    return Err(format!(
                        "field {} default `{s}` 不在 enum_options 中",
                        f.key
                    ));
                }
                if let Some(num) = def.as_f64() {
                    // default 同样受内禀范围约束
                    if let Some((lo, hi, name)) = intrinsic_range(f.field_type)
                        && (num < lo || num > hi)
                    {
                        return Err(format!(
                            "field {} default {num} 不在{name}内禀范围 [{lo}, {hi}]",
                            f.key
                        ));
                    }
                    if let Some(min) = f.validation.min
                        && num < min
                    {
                        return Err(format!("field {} default {num} < min {min}", f.key));
                    }
                    if let Some(max) = f.validation.max
                        && num > max
                    {
                        return Err(format!("field {} default {num} > max {max}", f.key));
                    }
                }
                if let Some(pat) = &f.validation.pattern
                    && let Some(s) = def.as_str()
                    && let Ok(re) = regex::Regex::new(pat)
                    && !re.is_match(s)
                {
                    return Err(format!(
                        "field {} default `{s}` 不匹配 pattern {pat}",
                        f.key
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
            // Secret 只有 JSON string 一种合法形态（SecretStore 的脱敏/持久化
            // 表示 {"secret_set": true} 由 core-api 在调用前 materialize，
            // 不得进 Contract 层）。
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
                // Port/Duration 内禀范围（类型对但值越界 → OUT_OF_RANGE，
                // 不得误报 INVALID_TYPE）。
                if let Some((lo, hi, name)) = intrinsic_range(field.field_type)
                    && (num < lo || num > hi)
                {
                    issues.push(ValidationIssue {
                        path: path.clone(),
                        code: "OUT_OF_RANGE".into(),
                        message: format!(
                            "field `{}` {num} 不在{name}内禀范围 [{lo}, {hi}]",
                            field.key
                        ),
                    });
                }
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

/// JSON 值与 FieldType 一致性（只回答“类型对不对”，不管值域）。
/// Port → integer，Duration → number；值域由 [`intrinsic_range`] 回答。
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

/// Port/Duration 内禀范围（Driver 可用 min/max 进一步收窄，不得放宽）：
/// Port 1..=65535，Duration >= 0。返回 (lo, hi, 名称)。
fn intrinsic_range(t: FieldType) -> Option<(f64, f64, &'static str)> {
    match t {
        FieldType::Port => Some((1.0, 65535.0, "Port")),
        FieldType::Duration => Some((0.0, f64::INFINITY, "Duration")),
        _ => None,
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
