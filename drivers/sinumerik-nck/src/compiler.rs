//! Address Compiler：NCK 协议地址的**唯一**转换入口。
//!
//! 设计依据：上位设计 §2.6 / §2.4.2 D1–D11 / §4.1.1 A–D，实施边界文档 §3.3
//! （含 C3 的 7 步 `identity_mode` 剥离）与 §3.5（唯一权威错误码表）。
//!
//! ## 三条硬约束
//!
//! 1. **零新增字节组装**：语义分支只负责「按 `addressing` 组装 12 个原生字段」，
//!    随后**完全复用** `NativeNckAddress::from_parameters` + `resolve_native`；
//!    wire 字节、期望长度、数据类型、无效值规则的唯一产出仍在 `native.rs`。
//! 2. **语义失败绝不退化到原生**：`CompileInput::Native` 不会被语义分支内部调用
//!    （`compile_semantic` 与 `compile_native` 互不调用），失败即 fail-closed。
//! 3. **`identity_mode` 只由本模块处理**：`native.rs` 的 12 字段白名单**不得放宽**；
//!    剥离**仅针对已明确验证的 Compiler 元数据**（本 PR 只有 `identity_mode`），
//!    **不是**「忽略未知字段」的通用机制——非白名单内的其它未知键仍必须被拒绝。
//!
//! ## 顺序不可交换
//!
//! D1–D10（结构 / 类型 / 存在性 / Target / arguments / 原生地址）**先**，
//! 身份计算与比对**后**（§4.1.1 D-1）：未知键与非法类型永远不进入身份计算。

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;

use mesa_core_types::{ErrorKind, validate_point_key};

use crate::definition::{
    DefinitionCatalog, NativeFieldSource, VariableDefinition, native_identity_key,
};
use crate::native::{NativeNckAddress, resolve_native, transport_size_name, unit_mode_name};
use crate::targets::TargetRef;

/// Point Kind：语义变量。
pub const POINT_KIND_VARIABLE: &str = "nck_variable";
/// Point Kind：高级原生地址。
pub const POINT_KIND_NATIVE: &str = "nck_native";

/// `nck_native` 允许的**原生字段**全集（12 项，与 Descriptor 的 `variable` 资源逐字一致）。
///
/// 为什么在 Driver 侧再声明一份：`native.rs` 的 `ALLOWED` 是**私有**且被冻结，
/// 而 Compiler 需要在调用它之前就拒绝未知键（C3 第 2 步）。两份的一致性由
/// `compiler_identity.rs` 的断言（与本常量 ↔ Descriptor 字段表逐项相等）保证。
pub const NATIVE_PARAMETER_KEYS: [&str; 12] = [
    "area",
    "area_no",
    "module",
    "column",
    "line",
    "count",
    "unit_mode",
    "decode",
    "transport_size",
    "invalid_value",
    "invalid_tolerance",
    "unit",
];

/// Compiler 元数据键（**唯一**一个；剥离只针对它）。
pub const NATIVE_IDENTITY_MODE_KEY: &str = "identity_mode";

/// 语义点位参数层允许的键。
const SEMANTIC_PARAMETER_KEYS: [&str; 4] =
    ["variable_id", "definition_revision", "target", "arguments"];

/// `native.` 保留前缀（`identity_mode="user"` 的 Key 必须留在此命名空间）。
const NATIVE_KEY_PREFIX: &str = "native.";

/// 编译错误（**§3.5 A 段 14 个 code** + 1 个 configure 层码，见 `IdentityNamespaceCollision`）。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CompileError {
    #[error("Point Kind `{kind}` 不受支持（仅 {POINT_KIND_VARIABLE} / {POINT_KIND_NATIVE}）")]
    UnsupportedPointKind { kind: String },

    #[error("未知变量 `{variable_id}`（不在内嵌定义目录中）")]
    UnknownVariable { variable_id: String },

    #[error(
        "`{variable_id}` 无 revision {revision}（可用 revision：{available:?}）；绝不回落到最新、也不回落到其它 revision"
    )]
    UnsupportedDefinitionRevision {
        variable_id: String,
        revision: u32,
        available: Vec<u32>,
    },

    #[error("系列不匹配：定义 family=`{definition_family}`，设备 family=`{device_family}`")]
    UnsupportedFamily {
        definition_family: String,
        device_family: String,
    },

    #[error("`{variable_id}` 不接受 Target 变体 `{kind}`（该变量声明：{targets:?}）")]
    UnsupportedTarget {
        variable_id: String,
        kind: String,
        targets: Vec<&'static str>,
    },

    #[error(
        "`{variable_id}` 缺少必填 Target 变体（必填：{required:?}）：{reason}；不做任何默认值猜测"
    )]
    MissingTarget {
        variable_id: String,
        required: Vec<&'static str>,
        reason: String,
    },

    #[error("非法 Target：{reason}")]
    InvalidTarget { reason: String },

    #[error("非法 arguments：{reason}")]
    InvalidArguments { reason: String },

    #[error(
        "身份键 `{path}` 不存在（variable_id=`{variable_id}`, revision={revision}）；缺失即拒，定义声明的 default 绝不物化"
    )]
    MissingIdentityKey {
        variable_id: String,
        revision: u32,
        path: String,
    },

    #[error(
        "point_key 与 Driver 重算的规范身份不一致：持久化 Key=`{persisted}`，重算 Key=`{recomputed}`，variable_id=`{variable_id}`，revision={revision}；configure 整体失败，任务不得启动"
    )]
    PointKeyMismatch {
        persisted: String,
        recomputed: String,
        variable_id: String,
        revision: u32,
    },

    #[error("`identity_mode=\"user\"` 的 Key `{point_key}` 不符 native 命名空间：{reason}")]
    InvalidUserIdentity { point_key: String, reason: String },

    #[error("绑定配置非法：{reason}")]
    InvalidBindingConfig { reason: String },

    #[error("原生地址非法（复用 native.rs 解析链）：{reason}")]
    InvalidVariable { reason: String },

    #[error("内嵌定义自检失败：{reason}")]
    DefinitionDigestMismatch { reason: String },

    /// **configure 层码（B5）**：只由 [`ensure_batch_identity_namespace_unique`] 产生，
    /// `compile_point` 永不返回它。
    ///
    /// 效力边界（§5.3）：仅覆盖**该次 configure 可见的点位集合**；Driver 在
    /// configure 时通常只看到本次传入的 Task，因此这**不是** Endpoint-wide
    /// 唯一性保证——跨 Task / 跨 kind 的唯一性由 Core 的 `DUPLICATE_POINT_KEY`
    /// 唯一裁决，本码既**不替代**也**不扩大** Core 规则。
    #[error(
        "批次内身份冲突：`{point_key}` 在该次 configure 可见的点位集合中出现 {count} 次（仅可见集合内提前反馈，非 Endpoint 级保证）"
    )]
    IdentityNamespaceCollision { point_key: String, count: usize },
}

impl CompileError {
    /// 稳定 code（边界文档 §3.5 唯一权威表）。
    pub fn code(&self) -> &'static str {
        match self {
            CompileError::UnsupportedPointKind { .. } => "UNSUPPORTED_POINT_KIND",
            CompileError::UnknownVariable { .. } => "UNKNOWN_VARIABLE",
            CompileError::UnsupportedDefinitionRevision { .. } => "UNSUPPORTED_DEFINITION_REVISION",
            CompileError::UnsupportedFamily { .. } => "UNSUPPORTED_FAMILY",
            CompileError::UnsupportedTarget { .. } => "UNSUPPORTED_TARGET",
            CompileError::MissingTarget { .. } => "MISSING_TARGET",
            CompileError::InvalidTarget { .. } => "INVALID_TARGET",
            CompileError::InvalidArguments { .. } => "INVALID_ARGUMENTS",
            CompileError::MissingIdentityKey { .. } => "MISSING_IDENTITY_KEY",
            CompileError::PointKeyMismatch { .. } => "POINT_KEY_MISMATCH",
            CompileError::InvalidUserIdentity { .. } => "INVALID_USER_IDENTITY",
            CompileError::InvalidBindingConfig { .. } => "INVALID_BINDING_CONFIG",
            CompileError::InvalidVariable { .. } => "INVALID_VARIABLE",
            CompileError::DefinitionDigestMismatch { .. } => "DEFINITION_DIGEST_MISMATCH",
            CompileError::IdentityNamespaceCollision { .. } => "IDENTITY_NAMESPACE_COLLISION",
        }
    }

    /// 错误分类（映射到 `SdkDriverError`）。
    pub fn kind(&self) -> ErrorKind {
        match self {
            CompileError::UnsupportedPointKind { .. }
            | CompileError::UnsupportedFamily { .. }
            | CompileError::IdentityNamespaceCollision { .. } => ErrorKind::Unsupported,
            CompileError::UnknownVariable { .. } | CompileError::InvalidVariable { .. } => {
                ErrorKind::Address
            }
            CompileError::DefinitionDigestMismatch { .. } => ErrorKind::Internal,
            _ => ErrorKind::Configuration,
        }
    }

    pub(crate) fn unknown_variable(variable_id: &str) -> Self {
        CompileError::UnknownVariable {
            variable_id: variable_id.to_string(),
        }
    }

    pub(crate) fn unsupported_definition_revision(
        variable_id: &str,
        revision: u32,
        available: Vec<u32>,
    ) -> Self {
        CompileError::UnsupportedDefinitionRevision {
            variable_id: variable_id.to_string(),
            revision,
            available,
        }
    }

    pub(crate) fn unsupported_target(
        variable_id: &str,
        kind: &str,
        targets: &[crate::targets::TargetKind],
    ) -> Self {
        CompileError::UnsupportedTarget {
            variable_id: variable_id.to_string(),
            kind: kind.to_string(),
            targets: targets.iter().map(|k| k.as_str()).collect(),
        }
    }

    pub(crate) fn missing_target(
        variable_id: &str,
        required: &[crate::targets::TargetKind],
        reason: &str,
    ) -> Self {
        CompileError::MissingTarget {
            variable_id: variable_id.to_string(),
            required: required.iter().map(|k| k.as_str()).collect(),
            reason: reason.to_string(),
        }
    }

    pub(crate) fn invalid_target(reason: impl Into<String>) -> Self {
        CompileError::InvalidTarget {
            reason: reason.into(),
        }
    }

    pub(crate) fn invalid_arguments(reason: impl Into<String>) -> Self {
        CompileError::InvalidArguments {
            reason: reason.into(),
        }
    }

    pub(crate) fn invalid_binding_config(reason: impl Into<String>) -> Self {
        CompileError::InvalidBindingConfig {
            reason: reason.into(),
        }
    }

    pub(crate) fn missing_identity_key(variable_id: &str, revision: u32, path: &str) -> Self {
        CompileError::MissingIdentityKey {
            variable_id: variable_id.to_string(),
            revision,
            path: path.to_string(),
        }
    }
}

/// 身份模式。**仅 `nck_native` 允许 `User`**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMode {
    /// 默认：`point_key` 必须逐字等于 Driver 重算的规范身份。
    Derived,
    /// 高级原生模式：`point_key` 是用户**显式选择**的逻辑身份；Driver 不重算、不改名。
    User,
}

impl IdentityMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            IdentityMode::Derived => "derived",
            IdentityMode::User => "user",
        }
    }
}

/// 编译结果引用的定义（语义点位才有）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionRef {
    pub variable_id: String,
    pub revision: u32,
    pub digest: String,
}

/// 编译产物（configure 的唯一输入）。
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledPoint {
    /// 复用 `native.rs` 的原生地址类型。
    pub address: NativeNckAddress,
    /// 复用 `native.rs` 的读取计划（wire / 长度 / 类型 / 规则）。
    pub resolved: crate::native::ResolvedPoint,
    /// 稳定诊断标识（`nck://read/...`，**不拼接** `variable_id`）。
    pub source_label: String,
    /// 语义点位给出定义引用；原生点位为 `None`。
    pub definition: Option<DefinitionRef>,
    /// Driver 重算的规范身份（`identity_mode="user"` 时**不参与**比对）。
    pub canonical_identity: String,
    pub identity_mode: IdentityMode,
}

/// 编译输入的两种形态（声明式表达；两条分支互不调用，禁止互相回退）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompileInput<'a> {
    Semantic {
        variable_id: &'a str,
        definition_revision: u32,
        target: &'a TargetRef,
        arguments: &'a Map<String, Value>,
    },
    Native(&'a Map<String, Value>),
}

/// 唯一协议地址转换入口。
pub struct AddressCompiler<'a> {
    catalog: &'a DefinitionCatalog,
    /// 设备 family；与 `def.family` 不匹配 ⇒ `UNSUPPORTED_FAMILY`。
    family: &'a str,
}

impl<'a> AddressCompiler<'a> {
    pub fn new(catalog: &'a DefinitionCatalog, family: &'a str) -> Self {
        Self { catalog, family }
    }

    /// 从 Core 信封点位编译；`point_kind` = `PointSelection.kind`。
    ///
    /// `persisted_point_key` 是 Core 持久化的点位身份，用于 D11 比对
    /// （`nck_variable` 恒 `derived`；`nck_native` 取决于 `identity_mode`）。
    pub fn compile_point(
        &self,
        point_kind: &str,
        parameters: &Map<String, Value>,
        persisted_point_key: &str,
    ) -> Result<CompiledPoint, CompileError> {
        match point_kind {
            POINT_KIND_VARIABLE => self.compile_semantic(parameters, persisted_point_key),
            POINT_KIND_NATIVE => self.compile_native(parameters, persisted_point_key),
            other => Err(CompileError::UnsupportedPointKind {
                kind: other.to_string(),
            }),
        }
    }

    /// 语义分支：定义目录 → `addressing` 组装 12 原生字段 → 复用原生解析链。
    fn compile_semantic(
        &self,
        parameters: &Map<String, Value>,
        persisted_point_key: &str,
    ) -> Result<CompiledPoint, CompileError> {
        // D2：参数层未知键即拒（`identity_mode` 亦不允许用于语义点位，B-3）。
        for key in parameters.keys() {
            if !SEMANTIC_PARAMETER_KEYS.contains(&key.as_str()) {
                return Err(CompileError::invalid_binding_config(format!(
                    "`{POINT_KIND_VARIABLE}` 参数层未知字段 `{key}`（`{NATIVE_IDENTITY_MODE_KEY}` 不允许用于语义点位）"
                )));
            }
        }
        // D3：类型错即拒。
        let variable_id = require_str(parameters, "variable_id")?;
        let revision = require_revision(parameters)?;
        let target_value = parameters.get("target").ok_or_else(|| {
            CompileError::invalid_binding_config("缺少必填字段 `target`（语义点位必须声明 Target）")
        })?;
        let target = TargetRef::from_json(target_value)?;
        let arguments = require_object(parameters, "arguments")?;

        // D5 / D6：目录命中 + revision 可解析性（miss 绝不回落）。
        let def = self.catalog.lookup(variable_id, revision)?;
        // 系列必须匹配：840d-sl 不得套用 828d 定义。
        if def.family != self.family {
            return Err(CompileError::UnsupportedFamily {
                definition_family: def.family.clone(),
                device_family: self.family.to_string(),
            });
        }
        // D7：Target 合法字段组合（由定义声明）。
        target.validate(def)?;
        // D8 + A9：arguments 键/类型/范围 + 身份键存在性。
        def.validate_arguments(arguments)?;
        // D9：按 addressing 组装 12 字段 → **复用**既有原生解析链。
        let native = assemble_native_fields(def, &target, arguments)?;
        let address = NativeNckAddress::from_parameters(&native).map_err(|e| {
            CompileError::InvalidVariable {
                reason: format!("`{variable_id}` 的语义寻址无法构成合法原生地址：{e}"),
            }
        })?;
        let resolved =
            resolve_native(address.clone()).map_err(|e| CompileError::InvalidVariable {
                reason: format!("`{variable_id}` resolve_native 失败：{e}"),
            })?;
        // D11：先用**已验证的语义字段**重算身份，再与持久化 Key 比对。
        let canonical_identity = def.canonical_identity(&target, arguments)?;
        let compiled = CompiledPoint {
            source_label: address.source_label(),
            address,
            resolved,
            definition: Some(DefinitionRef {
                variable_id: def.variable_id.clone(),
                revision: def.revision,
                digest: def.digest.clone(),
            }),
            canonical_identity,
            identity_mode: IdentityMode::Derived,
        };
        self.check_identity(persisted_point_key, &compiled)?;
        Ok(compiled)
    }

    /// 原生分支：按 C3 的 7 步流程处理 `identity_mode` 后**完全复用**既有解析。
    fn compile_native(
        &self,
        parameters: &Map<String, Value>,
        persisted_point_key: &str,
    ) -> Result<CompiledPoint, CompileError> {
        // 第 2–4 步：全量严格校验（含 identity_mode）+ 构建**只含 12 原生字段**的独立 map。
        let (native, identity_mode) = Self::split_native_parameters(parameters)?;
        // 第 5–6 步：交给既有解析链（`native.rs` 逐字未改）。
        let address = NativeNckAddress::from_parameters(&native).map_err(|e| {
            CompileError::InvalidVariable {
                reason: e.to_string(),
            }
        })?;
        let resolved =
            resolve_native(address.clone()).map_err(|e| CompileError::InvalidVariable {
                reason: e.to_string(),
            })?;
        let canonical_identity = native_identity_key(&address);
        let compiled = CompiledPoint {
            source_label: address.source_label(),
            address,
            resolved,
            definition: None,
            canonical_identity,
            identity_mode,
        };
        self.check_identity(persisted_point_key, &compiled)?;
        Ok(compiled)
    }

    /// **C3 步骤 2–4**：严格校验完整参数集合（12 原生字段 + 可选 `identity_mode`），
    /// 并产出**只含 12 原生字段**的独立 map（新 map，**不就地改**原 map）。
    ///
    /// 公开是为了让矩阵 #22(a) 能**逐键断言**剥离结果（恰 12 项、不含 `identity_mode`），
    /// 而不是只从最终 wire 反推。它是唯一权威的剥离实现，`compile_native` 也走它。
    pub fn split_native_parameters(
        parameters: &Map<String, Value>,
    ) -> Result<(Map<String, Value>, IdentityMode), CompileError> {
        // 第 2 步：**完整**集合的严格校验——剥离不是「忽略未知字段」的通用机制。
        for key in parameters.keys() {
            if !NATIVE_PARAMETER_KEYS.contains(&key.as_str()) && key != NATIVE_IDENTITY_MODE_KEY {
                return Err(CompileError::invalid_binding_config(format!(
                    "`{POINT_KIND_NATIVE}` 参数层未知字段 `{key}`（剥离仅针对已明确验证的 Compiler 元数据 `{NATIVE_IDENTITY_MODE_KEY}`）"
                )));
            }
        }
        // 第 3 步：读取并校验 `identity_mode`（默认 `derived`，仅允许 derived/user）。
        let identity_mode = match parameters.get(NATIVE_IDENTITY_MODE_KEY) {
            None => IdentityMode::Derived,
            Some(Value::String(s)) if s == "derived" => IdentityMode::Derived,
            Some(Value::String(s)) if s == "user" => IdentityMode::User,
            Some(other) => {
                return Err(CompileError::invalid_binding_config(format!(
                    "`{NATIVE_IDENTITY_MODE_KEY}` 仅允许 \"derived\"/\"user\"，实际 {other}"
                )));
            }
        };
        // 第 4 步：构建只包含现有原生字段的**独立**参数对象。
        let mut native = Map::new();
        for key in NATIVE_PARAMETER_KEYS {
            if let Some(v) = parameters.get(key) {
                native.insert(key.to_string(), v.clone());
            }
        }
        Ok((native, identity_mode))
    }

    /// **D11 唯一执行真值**。
    ///
    /// - `derived`：与重算身份**逐字**比对，不等 ⇒ `POINT_KEY_MISMATCH`
    ///   （消息含持久化 Key / 重算 Key / `variable_id` + `revision` 三要素）；
    /// - `user`：**仅** `nck_native` 允许（语义点位根本读不到该键，见 B-3），
    ///   Driver **不重算、不静默改名**，但仍复核通用语法与 `native.` 命名空间。
    pub fn check_identity(
        &self,
        persisted: &str,
        compiled: &CompiledPoint,
    ) -> Result<(), CompileError> {
        match compiled.identity_mode {
            IdentityMode::Derived => {
                if persisted != compiled.canonical_identity {
                    let (variable_id, revision) = match &compiled.definition {
                        Some(d) => (d.variable_id.clone(), d.revision),
                        None => (POINT_KIND_NATIVE.to_string(), 0),
                    };
                    return Err(CompileError::PointKeyMismatch {
                        persisted: persisted.to_string(),
                        recomputed: compiled.canonical_identity.clone(),
                        variable_id,
                        revision,
                    });
                }
                Ok(())
            }
            IdentityMode::User => {
                if !persisted.starts_with(NATIVE_KEY_PREFIX) {
                    return Err(CompileError::InvalidUserIdentity {
                        point_key: persisted.to_string(),
                        reason: format!(
                            "必须以 `{NATIVE_KEY_PREFIX}` 开头（语义身份恒含 `@` 且 variable_id 不得以 native. 开头 ⇒ 结构上不可能相同）"
                        ),
                    });
                }
                validate_point_key(persisted).map_err(|e| CompileError::InvalidUserIdentity {
                    point_key: persisted.to_string(),
                    reason: e.to_string(),
                })?;
                Ok(())
            }
        }
    }
}

/// B-2：**该次 configure 可见点位集合内**的最终 `point_key` 冲突自检。
///
/// 只对「至少一个成员是 `identity_mode="user"` 的原生 Key」的重复组生效：
/// 纯 `derived` / 语义点位的重复由同任务判重（`DUPLICATE_POINT_KEY`）负责，
/// 本函数**不替代**它，也**不**声称 Endpoint-wide 唯一性（见 §5.3 视野边界）。
pub fn ensure_batch_identity_namespace_unique<'a, I>(points: I) -> Result<(), CompileError>
where
    I: IntoIterator<Item = (&'a str, IdentityMode)>,
{
    let mut seen: BTreeMap<&str, Vec<IdentityMode>> = BTreeMap::new();
    for (key, mode) in points {
        seen.entry(key).or_default().push(mode);
    }
    for (key, modes) in seen {
        if modes.len() > 1 && modes.contains(&IdentityMode::User) {
            return Err(CompileError::IdentityNamespaceCollision {
                point_key: key.to_string(),
                count: modes.len(),
            });
        }
    }
    Ok(())
}

fn require_str<'a>(
    parameters: &'a Map<String, Value>,
    key: &'static str,
) -> Result<&'a str, CompileError> {
    parameters
        .get(key)
        .ok_or_else(|| CompileError::invalid_binding_config(format!("缺少必填字段 `{key}`")))?
        .as_str()
        .ok_or_else(|| CompileError::invalid_binding_config(format!("字段 `{key}` 需为字符串")))
}

fn require_revision(parameters: &Map<String, Value>) -> Result<u32, CompileError> {
    let raw = parameters.get("definition_revision").ok_or_else(|| {
        CompileError::invalid_binding_config("缺少必填字段 `definition_revision`")
    })?;
    // 必须是整数（`1.0` / `"1"` 都按类型错拒绝，见 D3）。
    let n = raw.as_u64().ok_or_else(|| {
        CompileError::invalid_binding_config(format!(
            "字段 `definition_revision` 需为非负整数，实际 {raw}"
        ))
    })?;
    u32::try_from(n).map_err(|_| {
        CompileError::invalid_binding_config(format!("字段 `definition_revision` = {n} 超过 u32"))
    })
}

fn require_object<'a>(
    parameters: &'a Map<String, Value>,
    key: &'static str,
) -> Result<&'a Map<String, Value>, CompileError> {
    parameters
        .get(key)
        .ok_or_else(|| CompileError::invalid_binding_config(format!("缺少必填字段 `{key}`")))?
        .as_object()
        .ok_or_else(|| CompileError::invalid_binding_config(format!("字段 `{key}` 需为 JSON 对象")))
}

/// 按 `AddressingRule` 组装 12 个原生字段（字段来源的**唯一**解释处）。
fn assemble_native_fields(
    def: &VariableDefinition,
    target: &TargetRef,
    arguments: &Map<String, Value>,
) -> Result<Map<String, Value>, CompileError> {
    let rule = &def.addressing;
    let area_no = resolve_source(&rule.area_no, def, target, arguments)?;
    let module = resolve_source(&rule.module, def, target, arguments)?;
    let line = resolve_source(&rule.line, def, target, arguments)?;

    let mut native = Map::new();
    native.insert("area".into(), Value::from(rule.area.letter().to_string()));
    native.insert("area_no".into(), Value::from(area_no));
    native.insert("module".into(), Value::from(module));
    native.insert("column".into(), Value::from(rule.column));
    native.insert("line".into(), Value::from(line));
    native.insert("count".into(), Value::from(def.count));
    native.insert(
        "unit_mode".into(),
        Value::from(unit_mode_name(def.unit_mode)),
    );
    native.insert("decode".into(), Value::from(def.decode.decode_name()));
    native.insert(
        "transport_size".into(),
        Value::from(transport_size_name(def.transport_size)),
    );
    if let Some(rule) = &def.invalid_rule {
        native.insert("invalid_value".into(), Value::from(rule.value));
        native.insert("invalid_tolerance".into(), Value::from(rule.tolerance));
    }
    if let Some(unit) = &def.unit {
        native.insert("unit".into(), Value::from(unit.clone()));
    }
    Ok(native)
}

/// 解析单个 `NativeFieldSource`（缺失一律报错，**绝不**给默认值）。
fn resolve_source(
    source: &NativeFieldSource,
    def: &VariableDefinition,
    target: &TargetRef,
    arguments: &Map<String, Value>,
) -> Result<u32, CompileError> {
    match source {
        NativeFieldSource::Fixed { value } => Ok(*value),
        NativeFieldSource::Target { kind } => target.instance(*kind).ok_or_else(|| {
            CompileError::missing_target(
                &def.variable_id,
                &def.target_required,
                &format!("寻址需要 `{}` 但没有提供", kind.as_str()),
            )
        }),
        NativeFieldSource::Argument { key } => {
            let n = argument_int(arguments, key)?;
            u32::try_from(n).map_err(|_| {
                CompileError::invalid_arguments(format!("argument `{key}` = {n} 不能作为原生字段"))
            })
        }
        NativeFieldSource::ArgumentOffset { key, offset } => {
            let base = argument_int(arguments, key)?;
            let value = base + i64::from(*offset);
            if !(0..=i64::from(u16::MAX)).contains(&value) {
                return Err(CompileError::invalid_arguments(format!(
                    "argument `{key}` = {base} 加偏移 {offset} 后 = {value} 溢出（ArgumentOffset 上溢即拒，不夹取）"
                )));
            }
            Ok(value as u32)
        }
        NativeFieldSource::DriveEntryModule => {
            let drive = target.drive_object.as_ref().ok_or_else(|| {
                CompileError::missing_target(
                    &def.variable_id,
                    &def.target_required,
                    "入口身份需要 `drive_object`（含 number 与 entry）但没有提供",
                )
            })?;
            Ok(u32::from(drive.entry.module()))
        }
    }
}

fn argument_int(arguments: &Map<String, Value>, key: &str) -> Result<i64, CompileError> {
    let raw = arguments.get(key).ok_or_else(|| {
        CompileError::invalid_arguments(format!("寻址需要 argument `{key}` 但未提供"))
    })?;
    raw.as_i64().ok_or_else(|| {
        CompileError::invalid_arguments(format!("argument `{key}` 需为整数，实际 {raw}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn native_params() -> Map<String, Value> {
        params(json!({
            "area": "C", "area_no": 1, "module": 21, "column": 1, "line": 396,
            "count": 1, "unit_mode": "current", "decode": "F64_LE",
            "transport_size": "0x09",
        }))
    }

    #[test]
    fn split_native_parameters_strips_only_verified_metadata() {
        // 完整 12 原生字段 + identity_mode：剥离后必须**恰 12 项**、不含 identity_mode。
        let mut p = native_params();
        p.insert("invalid_value".into(), json!(-200.0));
        p.insert("invalid_tolerance".into(), json!(0.001));
        p.insert("unit".into(), json!("mm"));
        p.insert("identity_mode".into(), json!("derived"));
        assert_eq!(p.len(), 13, "先确认输入是 13 字段");
        let (stripped, mode) = AddressCompiler::split_native_parameters(&p).unwrap();
        assert_eq!(mode, IdentityMode::Derived);
        assert_eq!(stripped.len(), 12, "剥离后恰 12 项");
        for key in NATIVE_PARAMETER_KEYS {
            assert!(stripped.contains_key(key), "剥离结果缺 `{key}`");
        }
        assert!(!stripped.contains_key("identity_mode"));
        // 未提供 identity_mode 时默认 derived，且同样只产出 12 项。
        let mut p12 = p.clone();
        p12.remove("identity_mode");
        let (stripped12, mode12) = AddressCompiler::split_native_parameters(&p12).unwrap();
        assert_eq!(mode12, IdentityMode::Derived);
        assert_eq!(stripped12.len(), 12);
        // 未知键**不得**被顺手放过。
        p.insert("whatever".into(), json!(1));
        assert_eq!(
            AddressCompiler::split_native_parameters(&p)
                .unwrap_err()
                .code(),
            "INVALID_BINDING_CONFIG"
        );
        // 取值非法。
        let mut p = native_params();
        p.insert("identity_mode".into(), json!("Derived"));
        assert_eq!(
            AddressCompiler::split_native_parameters(&p)
                .unwrap_err()
                .code(),
            "INVALID_BINDING_CONFIG"
        );
        let mut p = native_params();
        p.insert("identity_mode".into(), json!(null));
        assert_eq!(
            AddressCompiler::split_native_parameters(&p)
                .unwrap_err()
                .code(),
            "INVALID_BINDING_CONFIG"
        );
    }

    #[test]
    fn native_derived_key_must_match_exactly() {
        let cat = DefinitionCatalog::embedded().unwrap();
        let c = AddressCompiler::new(&cat, "828d");
        let p = native_params();
        let ok = c
            .compile_point(POINT_KIND_NATIVE, &p, "native.C.1.21.1.396")
            .unwrap();
        assert_eq!(ok.canonical_identity, "native.C.1.21.1.396");
        assert_eq!(ok.source_label, ok.address.source_label());
        let err = c
            .compile_point(POINT_KIND_NATIVE, &p, "native.C.1.21.1.397")
            .unwrap_err();
        assert_eq!(err.code(), "POINT_KEY_MISMATCH");
        assert!(err.to_string().contains("native.C.1.21.1.397"));
        assert!(err.to_string().contains("native.C.1.21.1.396"));
    }

    #[test]
    fn user_mode_native_requires_native_namespace() {
        let cat = DefinitionCatalog::embedded().unwrap();
        let c = AddressCompiler::new(&cat, "828d");
        let mut p = native_params();
        p.insert("identity_mode".into(), json!("user"));
        assert!(
            c.compile_point(POINT_KIND_NATIVE, &p, "native.C.1.21.1.396~2")
                .is_ok()
        );
        let err = c
            .compile_point(POINT_KIND_NATIVE, &p, "channel.program.status@channel=i:1")
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_USER_IDENTITY");
    }

    #[test]
    fn batch_collision_only_arms_for_user_keys() {
        use IdentityMode::{Derived, User};
        assert!(
            ensure_batch_identity_namespace_unique([
                ("native.C.1.21.1.396", Derived),
                ("native.C.1.21.1.396", Derived),
            ])
            .is_ok()
        );
        let err = ensure_batch_identity_namespace_unique([
            ("native.C.1.21.1.396", Derived),
            ("native.C.1.21.1.396", User),
        ])
        .unwrap_err();
        assert_eq!(err.code(), "IDENTITY_NAMESPACE_COLLISION");
    }
}
