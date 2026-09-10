//! Resource / Output 契约（V2.1 §14）。

use serde::{Deserialize, Serialize};

use crate::descriptor::DriverDescriptor;
use crate::schema::{FieldType, LocalizedText, SchemaDescriptor, ValidationIssue};
use crate::{DataType, TaskMode};

/// 访问模式（§3.2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    Read,
    Write,
    ReadWrite,
}

/// Output 类型规格（Descriptor V2）：配置阶段的类型能力声明。
///
/// - `Fixed`：类型恒定（如 FOCAS machine status → U32）；
/// - `FromParameter`：类型由某个资源参数决定（如 S7 memory.value ← data_type 参数），
///   mapping 必须覆盖该参数的全部 enum 选项（静态校验，防无意遗漏）；
/// - `DriverResolved`：类型只能由 Driver 在 Configure 时确定
///   （如 NCK catalog 变量，类型来自 catalog 条目）。
///
/// 运行时的最终类型真值仍是 `PointDescriptor.data_type`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputTypeSpec {
    Fixed {
        data_type: DataType,
    },
    FromParameter {
        parameter: String,
        mapping: std::collections::BTreeMap<String, DataType>,
    },
    DriverResolved,
}

impl OutputTypeSpec {
    /// 按给定参数表解析出具体类型（validate_instance 通过后的参数）。
    /// `Fixed` 直接返回；`FromParameter` 查 mapping（缺键返回 None）；
    /// `DriverResolved` 恒返回 None（须由 Driver 在 Configure 时确定）。
    pub fn resolve(&self, params: &serde_json::Map<String, serde_json::Value>) -> Option<DataType> {
        match self {
            OutputTypeSpec::Fixed { data_type } => Some(*data_type),
            OutputTypeSpec::FromParameter { parameter, mapping } => {
                let v = params.get(parameter)?.as_str()?;
                mapping.get(v).copied()
            }
            OutputTypeSpec::DriverResolved => None,
        }
    }
}

/// Output 契约（§3.2, §14）：逻辑数据能力的单个输出。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputDescriptor {
    pub id: String,
    pub label: LocalizedText,
    pub type_spec: OutputTypeSpec,
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
        self.parameters.validate_definition()?;
        // 安全边界：ResourceSelection 最终明文落 Task.binding_config_json，
        // Resource 参数禁止 Secret（认证 Secret 只能走 connection + SecretStore，
        // 与 EventStream.parameters 同规则）。
        if self
            .parameters
            .fields
            .iter()
            .any(|f| f.field_type == FieldType::Secret)
        {
            return Err(format!(
                "resource {} parameters 不得含 Secret 字段",
                self.id
            ));
        }
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
        // FromParameter 静态校验：参数存在、为 Enum、mapping 与选项精确覆盖
        for o in &self.outputs {
            if let OutputTypeSpec::FromParameter { parameter, mapping } = &o.type_spec {
                let field = self
                    .parameters
                    .fields
                    .iter()
                    .find(|f| &f.key == parameter)
                    .ok_or_else(|| {
                        format!(
                            "resource {} output {} 引用不存在的参数: {}",
                            self.id, o.id, parameter
                        )
                    })?;
                if field.field_type != FieldType::Enum {
                    return Err(format!(
                        "resource {} output {} 的类型参数 {} 必须为 Enum（实际 {:?}）",
                        self.id, o.id, parameter, field.field_type
                    ));
                }
                // 类型参数必须 required：optional 会让“schema 合法但缺参”的
                // selection 通过校验，而 resolve() → None 与 configure 默认值分叉。
                // UI 预填走 default，不走 optional。
                if !field.required {
                    return Err(format!(
                        "resource {} output {} 的类型参数 {} 必须 required",
                        self.id, o.id, parameter
                    ));
                }
                let options = field.validation.enum_options.clone().unwrap_or_default();
                for opt in &options {
                    if !mapping.contains_key(opt) {
                        return Err(format!(
                            "resource {} output {} mapping 缺少选项: {}",
                            self.id, o.id, opt
                        ));
                    }
                }
                for key in mapping.keys() {
                    if !options.contains(key) {
                        return Err(format!(
                            "resource {} output {} mapping 有多余键: {}",
                            self.id, o.id, key
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// 通用 ResourceSelection 契约（V2.1 §15）：统一 Envelope，不统一协议参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedOutput {
    pub output: String,
    pub point_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceSelection {
    pub resource_id: String,
    pub parameters: serde_json::Value,
    pub outputs: Vec<SelectedOutput>,
}

/// 通用 Binding `mesa.resources.v1` 的顶层形态（§15）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericBinding {
    pub selections: Vec<ResourceSelection>,
}

impl GenericBinding {
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())
    }
}

/// 校验 ResourceSelection 合法性（§15）：resource/output 存在、point_key 唯一等，由驱动结合 Descriptor 完成，此处仅做结构级校验。
pub fn validate_selections_structure(selections: &[ResourceSelection]) -> Result<(), String> {
    use std::collections::HashSet;
    let mut seen_keys = HashSet::new();
    for sel in selections {
        if sel.resource_id.trim().is_empty() {
            return Err("resource_id 不能为空".into());
        }
        if sel.outputs.is_empty() {
            return Err(format!("resource {} outputs 不能为空", sel.resource_id));
        }
        for out in &sel.outputs {
            if out.output.trim().is_empty() || out.point_key.trim().is_empty() {
                return Err("output/point_key 不能为空".into());
            }
            if !seen_keys.insert(&out.point_key) {
                return Err(format!("point_key 重复: {}", out.point_key));
            }
        }
        if !sel.parameters.is_object() && !sel.parameters.is_null() {
            return Err(format!("resource {} parameters 需为对象", sel.resource_id));
        }
    }
    Ok(())
}

/// 通用 Binding 种别常量
pub const GENERIC_BINDING_KIND: &str = "mesa.resources.v1";

/// Core 统一 ResourceSelection 校验（§15，Task 保存门禁语义实现）：
/// 结构（沿用 `validate_selections_structure` 语义）+ Descriptor 语义
///（resource 存在、output 存在、task mode 被资源支持、parameters 过
/// `validate_instance`）。路径以 `root` 为前缀（如 `tasks[0].selections`）。
/// 空 Vec = 通过。Driver 侧 configure 是第二道门，不得替代本函数。
///
/// 注意：point_key 唯一只在本 Task 内保证；跨 Task 的 Endpoint-wide 唯一
/// 由 `validate_task_set_against` 统一执行——保存/启动门禁必须走集合入口。
pub fn validate_selections_against(
    descriptor: &DriverDescriptor,
    mode: &TaskMode,
    selections: &[ResourceSelection],
    root: &str,
) -> Vec<ValidationIssue> {
    validate_task_set_against(descriptor, &[(mode, selections, root)])
}

/// Task 集合级校验（保存/启动门禁唯一入口）：逐 Task 复用单 Task 语义，
/// 外加 **Endpoint-wide point_key 唯一**（跨所有传入 Task；冻结契约
/// `point_key endpoint unique`，ConfigStore 只保 task id 唯一，拦不住此处）。
/// `tasks` 每项为 (task mode, selections, 路径前缀如 `tasks[0].selections`)。
pub fn validate_task_set_against(
    descriptor: &DriverDescriptor,
    tasks: &[(&TaskMode, &[ResourceSelection], &str)],
) -> Vec<ValidationIssue> {
    use std::collections::HashMap;
    let mut issues = Vec::new();
    // point_key → 首次出现位置（跨 Task 全局唯一）
    let mut seen_keys: HashMap<&str, String> = HashMap::new();
    for (mode, selections, root) in tasks {
        // 结构级（同 binding 内 point_key 唯一等；结构坏即跳过本 Task，避免级联误报）
        if let Err(e) = validate_selections_structure(selections) {
            issues.push(ValidationIssue {
                path: (*root).into(),
                code: "INVALID_STRUCTURE".into(),
                message: e,
            });
            continue;
        }
        for (i, sel) in selections.iter().enumerate() {
            let base = format!("{root}[{i}]");
            let Some(res) = descriptor
                .resources
                .iter()
                .find(|r| r.id == sel.resource_id)
            else {
                issues.push(ValidationIssue {
                    path: format!("{base}.resource_id"),
                    code: "UNKNOWN_RESOURCE".into(),
                    message: format!("resource `{}` 未声明", sel.resource_id),
                });
                continue;
            };
            // mode 支持（modes 为空即默认仅 Poll）
            let effective: Vec<TaskMode> = if res.modes.is_empty() {
                vec![TaskMode::Poll]
            } else {
                res.modes.clone()
            };
            if !effective.contains(*mode) {
                issues.push(ValidationIssue {
                    path: base.clone(),
                    code: "MODE_NOT_SUPPORTED".into(),
                    message: format!("resource `{}` 不支持 {mode:?} 模式", sel.resource_id),
                });
            }
            // 执行能力：resource 允许还不够，Driver capabilities 必须对应为
            // true，否则保存门禁会放行 Runtime 实际跑不起来的 mode。
            //（故意放在任务校验层而非 Descriptor::validate：2.0 definition
            // validity 已冻结，此处只裁决“当前配置能否被该 Driver 执行”。）
            let cap_ok = match mode {
                TaskMode::Poll => descriptor.capabilities.poll,
                TaskMode::Subscribe => descriptor.capabilities.subscribe,
            };
            if !cap_ok {
                issues.push(ValidationIssue {
                    path: base.clone(),
                    code: "MODE_NOT_SUPPORTED".into(),
                    message: format!("driver capabilities 不支持 {mode:?} 模式"),
                });
            }
            // parameters（null 视为 {}，与 Driver 侧归一一致）
            let params = if sel.parameters.is_null() {
                serde_json::json!({})
            } else {
                sel.parameters.clone()
            };
            for issue in res
                .parameters
                .validate_instance(&format!("{base}.parameters"), &params)
            {
                issues.push(issue);
            }
            // outputs 存在性 + point_key Endpoint-wide 唯一 + 读访问
            for out in &sel.outputs {
                let out_desc = res.outputs.iter().find(|o| o.id == out.output);
                if out_desc.is_none() {
                    issues.push(ValidationIssue {
                        path: format!("{base}.outputs"),
                        code: "UNKNOWN_OUTPUT".into(),
                        message: format!(
                            "resource `{}` 无 output `{}`",
                            sel.resource_id, out.output
                        ),
                    });
                }
                // 数据采集任务只能读：只写 output 不可被 Poll/Subscribe 选中
                //（写走 Control 面，不进采集 PointDescriptor）。
                if out_desc.is_some_and(|o| o.access == AccessMode::Write) {
                    issues.push(ValidationIssue {
                        path: format!("{base}.outputs"),
                        code: "ACCESS_NOT_SUPPORTED".into(),
                        message: format!(
                            "resource `{}` output `{}` 只写，不可采集",
                            sel.resource_id, out.output
                        ),
                    });
                }
                if let Some(first) = seen_keys.get(out.point_key.as_str()) {
                    issues.push(ValidationIssue {
                        path: format!("{base}.outputs"),
                        code: "DUPLICATE_POINT_KEY".into(),
                        message: format!(
                            "point_key `{}` 与 {first} 重复（endpoint 内必须唯一）",
                            out.point_key
                        ),
                    });
                } else {
                    seen_keys.insert(&out.point_key, base.clone());
                }
            }
        }
    }
    issues
}
