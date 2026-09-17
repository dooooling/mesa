//! Capability / Discovery / Control 契约（V2.1 §13, §20, §22）。

use serde::{Deserialize, Serialize};

use crate::schema::SchemaDescriptor;

/// Driver 能力（§13 capabilities）：仅 Runtime 能力，资源配置方式见
/// `ResourceSelectionMethod`（唯一真值，不在此重复）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DriverCapabilities {
    #[serde(default)]
    pub poll: bool,
    #[serde(default)]
    pub subscribe: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub method: bool,
    /// 是否支持事件（Event Plane §5）：老 Driver 缺字段即 false，正常工作。
    #[serde(default)]
    pub events: bool,
}

/// 资源配置方式（§20.1）：可组合的资源选择方式（Manual/Browse/Import，可多选），
/// 未来 UploadProject/Catalog/Template 加变体，不加 bool。
/// `DriverDescriptor.resource_selection_methods` 为唯一真值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSelectionMethod {
    Manual,
    Browse,
    Import,
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

/// 结构化写入目标（Foundation-3 单真值，ADR 0003 §37.3）：
/// `resource_id + parameters + output` 三元组，回归 Resource 模型。
///
/// - Core 理解 Resource/Output/Command，但永远不理解 `DB10.DBW2`、
///   `ns=2;i=1001`、`macro.100` 这类协议私有地址（Foundation 收口边界）；
/// - `point_key` 不得作为 WriteTarget：允许"只写、不采集"的工业参数，
///   不强迫控制操作先创建 Point；
/// - `parameters` 与 ResourceSelection.parameters 同语义（canonical 参数，
///   过 Resource.parameters schema 校验）；
/// - `output` 必须存在且 `access == Write | ReadWrite`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteTarget {
    pub resource_id: String,
    pub parameters: serde_json::Value,
    pub output: String,
}

impl WriteTarget {
    /// 审计身份：`resource_id/output`（parameters 摘要进 request_json，
    /// operation_id 只记稳定身份；Driver 私有地址字符串彻底退出审计）。
    pub fn audit_id(&self) -> String {
        format!("{}/{}", self.resource_id, self.output)
    }
}

/// 结构化写入请求（Foundation-3）：target + value + CAS 期望。
///
/// - `value` 类型必须与 `OutputDescriptor.type_spec.resolve(parameters)`
///   一致；`DriverResolved` 由 Driver 终裁，Core 放行；
/// - `expected` 为 Some 即真正的 conditional write / CAS 要求：协议无法
///   保证原子语义时 Driver 必须返回 `EXPECTED_VALUE_UNSUPPORTED`，
///   禁止 Core 或 Driver 用 read → compare → write 假装原子 CAS。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlWrite {
    pub target: WriteTarget,
    pub value: crate::Value,
    pub expected: Option<crate::Value>,
}

/// Control 门禁错误（Core Descriptor 门禁的精确码，fail-closed）。
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ControlGateError {
    #[error("driver 不支持写入 (capabilities.write=false)")]
    WriteNotSupported,
    #[error("driver 不支持命令 (capabilities.method=false)")]
    MethodNotSupported,
    #[error("resource `{0}` 未声明")]
    UnknownResource(String),
    #[error("resource `{0}` 无 output `{1}`")]
    UnknownOutput(String, String),
    #[error("resource `{0}` output `{1}` 只读，不可写入")]
    OutputNotWritable(String, String),
    #[error("command `{0}` 未声明")]
    CommandNotDeclared(String),
    #[error("command `{0}` 输入非法: {1}")]
    InvalidCommandInput(String, String),
    #[error("command `{0}` 结果违反 result_schema: {1}")]
    InvalidCommandResult(String, String),
    #[error("写入值类型与 output 类型不一致: {0}")]
    ValueTypeMismatch(String),
}

/// Write 门禁（Core 统一执行，Foundation-3）：
/// descriptor 可用 → capabilities.write → resource/output 存在 →
/// access 可写 → parameters 过 schema → value/expected 类型一致。
/// 返回解析出的期望 DataType（DriverResolved 即 None，由 Driver 终裁）。
/// issues 为空即通过（调用方直接映射为 REST 400）。
pub fn gate_write_against(
    descriptor: &crate::descriptor::DriverDescriptor,
    target: &WriteTarget,
    value: &crate::Value,
    expected: &Option<crate::Value>,
    root: &str,
) -> (Vec<crate::schema::ValidationIssue>, Option<crate::DataType>) {
    use crate::schema::ValidationIssue;
    let mut issues = Vec::new();
    let fail = |path: String, code: &str, message: String| ValidationIssue {
        path,
        code: code.into(),
        message,
    };
    // capabilities.write 门
    if !descriptor.capabilities.write {
        issues.push(fail(
            root.into(),
            "WRITE_NOT_SUPPORTED",
            "driver 不支持写入 (capabilities.write=false)".into(),
        ));
        return (issues, None);
    }
    // resource 存在
    let Some(res) = descriptor
        .resources
        .iter()
        .find(|r| r.id == target.resource_id)
    else {
        issues.push(fail(
            format!("{root}.resource_id"),
            "UNKNOWN_RESOURCE",
            format!("resource `{}` 未声明", target.resource_id),
        ));
        return (issues, None);
    };
    // parameters 过 schema（null 视为 {}，与 validate_task_set_against 同口径）
    let params = if target.parameters.is_null() {
        serde_json::json!({})
    } else {
        target.parameters.clone()
    };
    for issue in res
        .parameters
        .validate_instance(&format!("{root}.parameters"), &params)
    {
        issues.push(issue);
    }
    if !issues.is_empty() {
        return (issues, None);
    }
    // output 存在 + 可写
    let Some(out_desc) = res.outputs.iter().find(|o| o.id == target.output) else {
        issues.push(fail(
            format!("{root}.output"),
            "UNKNOWN_OUTPUT",
            format!(
                "resource `{}` 无 output `{}`",
                target.resource_id, target.output
            ),
        ));
        return (issues, None);
    };
    if out_desc.access != crate::resource::AccessMode::Write
        && out_desc.access != crate::resource::AccessMode::ReadWrite
    {
        issues.push(fail(
            format!("{root}.output"),
            "ACCESS_NOT_SUPPORTED",
            format!(
                "resource `{}` output `{}` 只读，不可写入",
                target.resource_id, target.output
            ),
        ));
        return (issues, None);
    }
    // 类型解析与 value/expected 一致性（DriverResolved 即 None，Driver 终裁）
    let params_obj = params.as_object().cloned().unwrap_or_default();
    let expected_dt = out_desc.type_spec.resolve(&params_obj);
    if let Some(dt) = expected_dt {
        if value.data_type() != dt {
            issues.push(fail(
                format!("{root}.value"),
                "VALUE_TYPE_MISMATCH",
                format!(
                    "写入值类型 {:?} 与 output 期望 {:?} 不一致",
                    value.data_type(),
                    dt
                ),
            ));
        }
        if let Some(exp) = expected
            && exp.data_type() != dt
        {
            issues.push(fail(
                format!("{root}.expected_value"),
                "VALUE_TYPE_MISMATCH",
                format!(
                    "期望值类型 {:?} 与 output 期望 {:?} 不一致",
                    exp.data_type(),
                    dt
                ),
            ));
        }
    }
    (issues, expected_dt)
}

/// Command 存在性 + input 门禁（Core 统一执行，Foundation-3）：
/// capabilities.method → command 声明 → input 过 input_schema。
/// 不存在的 command 绝不送 Driver（COMMAND_NOT_DECLARED）。
pub fn gate_command_against(
    descriptor: &crate::descriptor::DriverDescriptor,
    command_id: &str,
    input: &serde_json::Value,
    root: &str,
) -> Vec<crate::schema::ValidationIssue> {
    use crate::schema::ValidationIssue;
    let mut issues = Vec::new();
    if !descriptor.capabilities.method {
        issues.push(ValidationIssue {
            path: root.into(),
            code: "METHOD_NOT_SUPPORTED".into(),
            message: "driver 不支持命令 (capabilities.method=false)".into(),
        });
        return issues;
    }
    let Some(cmd) = descriptor
        .controls
        .commands
        .iter()
        .find(|c| c.id == command_id)
    else {
        issues.push(ValidationIssue {
            path: format!("{root}.command_id"),
            code: "COMMAND_NOT_DECLARED".into(),
            message: format!("command `{command_id}` 未声明"),
        });
        return issues;
    };
    let params = if input.is_null() {
        serde_json::json!({})
    } else {
        input.clone()
    };
    for issue in cmd
        .input_schema
        .validate_instance(&format!("{root}.input"), &params)
    {
        issues.push(issue);
    }
    issues
}

/// Command 结果门禁（Core 执行，Foundation-3）：Driver 返回违反自己
/// result_schema 即 DRIVER_CONTRACT_VIOLATION（审计记 FAILED）。
pub fn gate_command_result_against(
    descriptor: &crate::descriptor::DriverDescriptor,
    command_id: &str,
    result: &serde_json::Value,
    root: &str,
) -> Vec<crate::schema::ValidationIssue> {
    let mut issues = Vec::new();
    let Some(cmd) = descriptor
        .controls
        .commands
        .iter()
        .find(|c| c.id == command_id)
    else {
        return issues;
    };
    let params = if result.is_null() {
        serde_json::json!({})
    } else {
        result.clone()
    };
    for issue in cmd
        .result_schema
        .validate_instance(&format!("{root}.result"), &params)
    {
        let mut i = issue;
        i.code = "DRIVER_CONTRACT_VIOLATION".into();
        issues.push(i);
    }
    issues
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
            c.input_schema.validate_definition()?;
            c.result_schema.validate_definition()?;
        }
        Ok(())
    }
}
