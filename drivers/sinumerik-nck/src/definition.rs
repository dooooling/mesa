//! 变量定义目录（`VariableDefinition` / revision / digest / 身份模板）。
//!
//! 设计依据：上位设计 §2.5 / §3.1–§3.5 / §4.1.1 C，实施边界文档 §3.1（含 C5 的
//! 存放形态裁决）与 §5.2 A-1。
//!
//! ## 为什么定义目录是「数据文件 + 装载期自检」
//!
//! 1. **已发布的 `(variable_id, revision)` 不可原地修改**（§3.1 硬约束）。
//!    这只能由「按 revision 分文件 + canonical JSON 摘要清单」落地：
//!    `definitions/828d/r1.json` + `definitions/828d/DIGESTS.lock`。
//! 2. **身份是 Driver 的产出**（修订 4）：`identity_template` / `identity_keys`
//!    是**驱动数据**，随定义目录一起发布、参与摘要；Core 不读模板、不推导身份。
//! 3. 装载即自检（ID 形状、保留前缀、身份覆盖闭包 A-1a/A-1b/A-1d、摘要一致），
//!    任何一条不成立都让目录整体不可用（fail-closed），绝不「跳过坏定义继续跑」。
//!
//! ## 摘要的 canonical 规则（§3.2，不依赖 `serde` 默认）
//!
//! 顶层与嵌套对象键按字节序升序、数组保序、`digest` 自身不参与、
//! 缺省可选字段**不出现**（不写 `null`）。同构先例：`crates/event-store/src/lib.rs`。
//!
//! ## 本模块的边界
//!
//! 不读连接配置、不发网络请求、不认识 `target` 的语义（那是 `targets.rs`）；
//! 也不读任何**预填模板**文件——定义目录与模板集合是两条独立数据来源，
//! 只共享同一份冻结 wire 向量（见 `tests/semantic_equivalence.rs`）。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::address::{NckArea, NckUnitMode};
use crate::compiler::CompileError;
use crate::native::{TRANSPORT_SIZES, unit_mode_name};
use crate::targets::{DriveEntry, TargetKind, TargetRef};
use crate::value::NckDataKind;

/// 定义载体格式版本（数据文件格式，与驱动语义版本解耦；进摘要）。
pub const DEFINITION_FORMAT_VERSION: u32 = 1;
/// 最小合法 `definition_revision`（从 1 开始，单调递增、不跳号不复用）。
pub const MIN_DEFINITION_REVISION: u32 = 1;

/// 内嵌定义数据（`include_str!` 的相对深度与既有内嵌数据文件同模式：数据目录
/// 与 `src/` 平级，见边界文档 §3.1 C5）。
pub const EMBEDDED_DEFINITIONS_828D_R1: &str = include_str!("../definitions/828d/r1.json");
/// 内嵌摘要清单（已发布 `(variable_id, revision)` 的 canonical-JSON SHA-256）。
pub const EMBEDDED_DEFINITIONS_828D_LOCK: &str = include_str!("../definitions/828d/DIGESTS.lock");
/// 内嵌 sidecar 元数据（单位 / 手册位置 / 索引规则 / 可用性判定）。
///
/// 为什么内嵌而不是运行期读文件：与上面两份同源数据保持**同一个失败时刻**。
/// `include_str!` 让「文件缺失」变成编译错误；而「文件存在但内容与 r1.json 不匹配」
/// 由装载期摘要绑定检查兜住（见 `definition_api::DefinitionMeta::load`）。
/// 二者合起来满足 fail-closed：任何一条不成立，目录整体不可用。
pub const EMBEDDED_DEFINITIONS_828D_R1_META: &str =
    include_str!("../definitions/828d/r1.meta.json");

/// 定义适用范围：`Collect` 出现在用户点位面；`Identity` 只服务身份探针。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionScope {
    Collect,
    Identity,
}

impl DefinitionScope {
    /// 投影字符串（与定义数据文件逐字一致）。
    ///
    /// PR-B3 起这是**唯一词表来源**：目录投影（`definition_api::scope_name`）
    /// 与 Configure 拒绝消息都复用它，避免「前端看到的 scope」与「后端拒绝
    /// 依据的 scope」各维护一份字符串而漂移。
    pub const fn as_str(self) -> &'static str {
        match self {
            DefinitionScope::Collect => "collect",
            DefinitionScope::Identity => "identity",
        }
    }
}

/// 语义状态（§2.5.2，防语义虚构；状态升级必须发布新 revision）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticsStatus {
    /// 仓库内可追溯证据（手册 / 实测 / 身份探针 / 冻结向量）。
    Verified,
    /// 语义仅来自导入点位标签，可靠性已登记为风险。
    Candidate,
    /// 仓库内无任何语义证据，ID 必须是 `drive.parameter.<rNNNN>`。
    ParameterIdentity,
}

/// 展示名（**纯展示元数据**：不参与身份、不参与寻址，但**参与**摘要）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalizedText {
    pub zh: String,
    pub en: String,
}

/// 定义来源类别（§4.2 第 5 条检测②：来源必须是手册 / 实测 / 身份探针 / 冻结向量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Manual,
    Measurement,
    IdentityProbe,
    FrozenVector,
}

/// 定义来源（仓库相对路径或文档标识；**不含** commit hash，见 §3.2 稳定性要求）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionSource {
    pub kind: SourceKind,
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// argument 取值类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgType {
    Integer,
    Enum,
}

/// 用户需输入的参数规格。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgumentSpec {
    /// `^[a-z][a-z0-9_]{0,63}$`（§4.1.1 H-4）。
    pub key: String,
    pub label: LocalizedText,
    pub value_type: ArgType,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enum_options: Option<Vec<String>>,
    /// 缺省值**仅用于展示**；身份与寻址**绝不**物化它（§4.1.1 H-6 三态可区分）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// 原生字段来源：**唯一定义处**（禁止在代码里散落 `if`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeFieldSource {
    /// 固定常量。
    Fixed { value: u32 },
    /// 取该 target 变体的实例号。
    Target { kind: TargetKind },
    /// 取 `arguments.<key>`。
    Argument { key: String },
    /// 取 `arguments.<key>` + 偏移（R 参数 `line = index + 1`）。
    ArgumentOffset { key: String, offset: i32 },
    /// 入口身份 → module（26 / 130），唯一实现是 [`DriveEntry::module`]。
    DriveEntryModule,
}

/// 原生寻址规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressingRule {
    #[serde(with = "area_serde")]
    pub area: NckArea,
    pub area_no: NativeFieldSource,
    pub module: NativeFieldSource,
    pub column: u16,
    pub line: NativeFieldSource,
}

impl AddressingRule {
    /// 规则里出现的全部字段来源（供装载期 A-1 覆盖闭包计算与测试复用）。
    pub fn sources(&self) -> [&NativeFieldSource; 3] {
        [&self.area_no, &self.module, &self.line]
    }
}

/// 无效工程值规则（如 `r0035`: -200 / 0.001）。它**是定义的一部分**，因此进摘要。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvalidRuleSpec {
    pub value: f64,
    pub tolerance: f64,
}

/// 身份所需参数键（点分叶路径 + 是否必填）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityKeySpec {
    pub path: String,
    pub required: bool,
}

/// 语义 ID ↔ 冻结 wire 向量的绑定（**只存索引**，不复制字节）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenRef {
    pub area: String,
    pub block: String,
    pub variable: String,
    pub entry: DriveEntry,
}

impl GoldenRef {
    /// 模板 id 可无损重建的四元组键（§4.4 的交叉校验用）。
    pub fn key(&self) -> (String, String, String, DriveEntry) {
        (
            self.area.clone(),
            self.block.clone(),
            self.variable.clone(),
            self.entry,
        )
    }
}

/// 单条变量定义（**NCK Driver 侧类型**，Core 不可见）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableDefinition {
    /// 业务语义 ID；稳定、不复用。
    pub variable_id: String,
    /// 定义修订号（>= 1；已发布不可改）。
    pub revision: u32,
    /// 适用系列（`828d`；`840d-sl` 暂无任何定义）。
    pub family: String,
    pub scope: DefinitionScope,
    pub semantics_status: SemanticsStatus,
    pub label: LocalizedText,
    pub source: Vec<DefinitionSource>,
    /// 适用 Target 类型（**合法字段组合由此声明**，不在代码里硬编码）。
    pub targets: Vec<TargetKind>,
    /// 其中必须提供的 Target 类型。
    pub target_required: Vec<TargetKind>,
    pub arguments: Vec<ArgumentSpec>,
    pub addressing: AddressingRule,
    #[serde(with = "decode_serde")]
    pub decode: NckDataKind,
    /// 固定期望响应 transport（`0x05/0x06/0x07/0x09`）。
    pub transport_size: u8,
    /// 线缆 `line_count`（1..=255）。
    pub count: u8,
    #[serde(with = "unit_mode_serde")]
    pub unit_mode: NckUnitMode,
    /// 展示单位（仅注释；本期定义一律不写死，见 `README` 的单位说明）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_rule: Option<InvalidRuleSpec>,
    /// 规范身份模板（Driver 数据；消费方**不得重排**段序）。
    pub identity_template: String,
    pub identity_keys: Vec<IdentityKeySpec>,
    /// 语义 ID ↔ 冻结向量的绑定。
    ///
    /// NOTE（与边界文档 §3.1 的**已知偏差**）：§3.1 的结构体声明写单数
    /// `golden_ref: GoldenRef`，但同一文档 §4.4 与矩阵 #7 要求「42 条定义共产生
    /// **62** 个 GoldenRef（A 16×1 + B 20×2 + C 6×1）」。`drive.*` 的每条定义
    /// 都覆盖 `nc_variable` 与 `drive_object` **两条**冻结条目，单数类型无法表达。
    /// 故取 `Vec<GoldenRef>`（字段名保持 `golden_ref`），以满足可执行判据；
    /// 该偏差已在交付报告中如实记录。
    pub golden_ref: Vec<GoldenRef>,
    /// canonical JSON 的 SHA-256（装载期计算；**不参与**自身摘要，§3.2 规则 3）。
    #[serde(default)]
    pub digest: String,
}

/// `NckArea` 的 serde：JSON 用 Siemens 字母（`"C"`），与用户面词表一致。
mod area_serde {
    use super::*;

    pub fn serialize<S: Serializer>(area: &NckArea, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&area.letter().to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NckArea, D::Error> {
        let raw = String::deserialize(d)?;
        NckArea::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// `NckDataKind` 的 serde：JSON 用规范 `decode` 名（`U16_LE` / `UTF8_NUL_LAST_32`…）。
mod decode_serde {
    use super::*;

    pub fn serialize<S: Serializer>(kind: &NckDataKind, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(kind.decode_name())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NckDataKind, D::Error> {
        let raw = String::deserialize(d)?;
        NckDataKind::parse_decode(&raw).map_err(serde::de::Error::custom)
    }
}

/// `NckUnitMode` 的 serde（JSON 用 `current` / `metric` / `inch`）。
mod unit_mode_serde {
    use super::*;

    pub fn serialize<S: Serializer>(mode: &NckUnitMode, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(unit_mode_name(*mode))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NckUnitMode, D::Error> {
        let raw = String::deserialize(d)?;
        NckUnitMode::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// 摘要计算失败（只发生在 JSON 序列化层面；定义已在装载期校验过）。
#[derive(Debug, Error)]
pub enum DigestError {
    #[error("定义无法序列化为 canonical JSON：{0}")]
    Serialize(String),
}

/// 目录装载 / 自检错误。**全部**映射为同一个对外 code：内嵌定义问题属**构建期**
/// 缺陷（不是用户配置问题），必须 fail-closed 且可被 CI 立刻定位。
#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("内嵌定义 JSON 非法：{0}")]
    Json(String),
    #[error("定义载荷格式版本 `{0}` 不受支持（当前 {DEFINITION_FORMAT_VERSION}）")]
    UnsupportedFormatVersion(u32),
    #[error("定义 `{variable_id}`(rev {revision}) 自检失败：{reason}")]
    InvalidDefinition {
        variable_id: String,
        revision: u32,
        reason: String,
    },
    #[error(
        "定义 `{variable_id}`(rev {revision}) 摘要与 DIGESTS.lock 不符：lock={expected}，实算={actual}"
    )]
    DigestMismatch {
        variable_id: String,
        revision: u32,
        expected: String,
        actual: String,
    },
    #[error("DIGESTS.lock 缺少 `{variable_id}`(rev {revision}) 的条目")]
    MissingLockEntry { variable_id: String, revision: u32 },
    #[error("DIGESTS.lock 含无用条目 `{variable_id}`(rev {revision})：定义目录中不存在该定义")]
    UnexpectedLockEntry { variable_id: String, revision: u32 },
    #[error("摘要清单非法：{0}")]
    LockJson(String),
    /// sidecar 元数据（`r1.meta.json`）非法：形状不对、与 `r1.json` 的绑定摘要不符、
    /// 或与定义集合不能双向对上。
    ///
    /// NOTE：这里**不**新增对外错误码——它和上面各变体同属「构建期/发布数据问题」，
    /// 统一映射为 `DEFINITION_DIGEST_MISMATCH`（`Internal` ⇒ 503）。新增码会破坏
    /// 边界文档 §3.5 的「码 → kind 唯一」冻结表，而 sidecar 不符与摘要不符对调用方
    /// 而言是同一种失败：这份目录不可用。
    #[error("定义 sidecar 元数据非法：{0}")]
    MetaInvalid(String),
}

impl CatalogError {
    /// 对外稳定 code（边界文档 §3.5 A14）。
    pub fn code(&self) -> &'static str {
        "DEFINITION_DIGEST_MISMATCH"
    }
}

/// `DIGESTS.lock` 的条目（已发布摘要）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockEntry {
    pub variable_id: String,
    pub revision: u32,
    /// 发行文件路径（**仓库相对**）——便于 CI 跨 checkout 比对。
    pub file: String,
    pub digest: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockDoc {
    format_version: u32,
    #[serde(default)]
    family: String,
    entries: Vec<LockEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefinitionDoc {
    format_version: u32,
    family: String,
    revision: u32,
    definitions: Vec<VariableDefinition>,
}

/// 定义目录：`variable_id → revision → 定义`。
#[derive(Debug, Clone, Default)]
pub struct DefinitionCatalog {
    by_id: BTreeMap<String, BTreeMap<u32, VariableDefinition>>,
    /// 已发布摘要清单；仅内存构造（测试）时为空。
    locks: BTreeMap<(String, u32), String>,
}

impl DefinitionCatalog {
    /// 编译期内嵌目录（装载即全量自检 + 摘要比对）。
    pub fn embedded() -> Result<Self, CatalogError> {
        Self::load(EMBEDDED_DEFINITIONS_828D_R1, EMBEDDED_DEFINITIONS_828D_LOCK)
    }

    /// 从「定义数据 + 摘要清单」装载（`embedded()` 与测试共用同一路径）。
    pub fn load(definitions_json: &str, digests_json: &str) -> Result<Self, CatalogError> {
        let doc: DefinitionDoc = serde_json::from_str(definitions_json)
            .map_err(|e| CatalogError::Json(e.to_string()))?;
        if doc.format_version != DEFINITION_FORMAT_VERSION {
            return Err(CatalogError::UnsupportedFormatVersion(doc.format_version));
        }
        let lock: LockDoc = serde_json::from_str(digests_json)
            .map_err(|e| CatalogError::LockJson(e.to_string()))?;
        if lock.format_version != DEFINITION_FORMAT_VERSION {
            return Err(CatalogError::UnsupportedFormatVersion(lock.format_version));
        }
        if !lock.family.is_empty() && lock.family != doc.family {
            return Err(CatalogError::LockJson(format!(
                "Lock family `{}` 与定义 family `{}` 不一致",
                lock.family, doc.family
            )));
        }
        let mut locks = BTreeMap::new();
        for e in &lock.entries {
            if locks
                .insert((e.variable_id.clone(), e.revision), e.digest.clone())
                .is_some()
            {
                return Err(CatalogError::LockJson(format!(
                    "条目重复：{}@{}",
                    e.variable_id, e.revision
                )));
            }
        }
        let mut catalog = Self {
            by_id: BTreeMap::new(),
            locks,
        };
        for def in doc.definitions {
            if def.family != doc.family {
                return Err(CatalogError::InvalidDefinition {
                    variable_id: def.variable_id.clone(),
                    revision: def.revision,
                    reason: format!("family `{}` 与文件声明 `{}` 不一致", def.family, doc.family),
                });
            }
            if def.revision != doc.revision {
                return Err(CatalogError::InvalidDefinition {
                    variable_id: def.variable_id.clone(),
                    revision: def.revision,
                    reason: format!("revision 与文件声明 `{}` 不一致", doc.revision),
                });
            }
            catalog.insert(def)?;
        }
        catalog.verify_digests()?;
        Ok(catalog)
    }

    /// 内存构造（**仅供测试与内存场景**；不做摘要比对）。
    ///
    /// 仍然执行全部装载期自检（ID 形状、身份覆盖闭包、跨 revision 一致性），
    /// 因此测试可以放心用它构造「合法的多 revision 目录」与「刻意的坏定义」。
    pub fn from_definitions(defs: Vec<VariableDefinition>) -> Result<Self, CatalogError> {
        let mut catalog = Self {
            by_id: BTreeMap::new(),
            locks: BTreeMap::new(),
        };
        for def in defs {
            catalog.insert(def)?;
        }
        Ok(catalog)
    }

    fn insert(&mut self, def: VariableDefinition) -> Result<(), CatalogError> {
        validate_definition(&def)?;
        // 跨 revision 一致性：同一 variable_id 的所有 revision 必须给出**逐字相同**的
        // 身份模板与身份键集合（否则「仅 revision 变化 ⇒ point_key 不变」失效）。
        if let Some((_, first)) = self
            .by_id
            .get(&def.variable_id)
            .and_then(|existing| existing.iter().next())
            && (first.identity_template != def.identity_template
                || first.identity_keys != def.identity_keys)
        {
            return Err(CatalogError::InvalidDefinition {
                variable_id: def.variable_id.clone(),
                revision: def.revision,
                reason: format!(
                    "identity_template/identity_keys 与 rev {} 不同——修改身份契约必须发布**新 variable_id**（§3.5）",
                    first.revision
                ),
            });
        }
        let slot = self.by_id.entry(def.variable_id.clone()).or_default();
        if slot.contains_key(&def.revision) {
            return Err(CatalogError::InvalidDefinition {
                variable_id: def.variable_id.clone(),
                revision: def.revision,
                reason: "(variable_id, revision) 重复".into(),
            });
        }
        let mut def = def;
        def.digest = Self::compute_digest(&def).map_err(|e| CatalogError::InvalidDefinition {
            variable_id: def.variable_id.clone(),
            revision: def.revision,
            reason: format!("摘要计算失败：{e}"),
        })?;
        slot.insert(def.revision, def);
        Ok(())
    }

    /// 命中即返回；**miss 绝不回落最新或其它 revision**（§3.3 三种禁止退化）。
    pub fn lookup(
        &self,
        variable_id: &str,
        revision: u32,
    ) -> Result<&VariableDefinition, CompileError> {
        let revisions = self
            .by_id
            .get(variable_id)
            .ok_or_else(|| CompileError::unknown_variable(variable_id))?;
        revisions.get(&revision).ok_or_else(|| {
            CompileError::unsupported_definition_revision(
                variable_id,
                revision,
                self.available_revisions(variable_id),
            )
        })
    }

    /// 该变量目录中可用的 revision 列表（升序；供错误消息三要素之一）。
    pub fn available_revisions(&self, variable_id: &str) -> Vec<u32> {
        self.by_id
            .get(variable_id)
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default()
    }

    /// 指定系列的全部定义（按 `variable_id` 字节序、再按 revision 升序）。
    pub fn for_family(&self, family: &str) -> Vec<&VariableDefinition> {
        self.iter().filter(|d| d.family == family).collect()
    }

    /// 全部定义的稳定遍历。
    pub fn iter(&self) -> impl Iterator<Item = &VariableDefinition> {
        self.by_id.values().flat_map(|m| m.values())
    }

    /// 定义总数。
    pub fn len(&self) -> usize {
        self.by_id.values().map(|m| m.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 按 `GoldenRef` 四元组查找唯一命中的定义（§4.4 满射/单射/双射的查询侧）。
    ///
    /// 返回 `Vec`：调用方（与测试）必须自己断言**恰一条**，本函数不替调用方
    /// 吞掉「多定义绑定同一条冻结向量」这种数据缺陷。
    pub fn find_by_golden_ref(&self, want: &GoldenRef) -> Vec<&VariableDefinition> {
        let key = want.key();
        self.iter()
            .filter(|d| d.golden_ref.iter().any(|g| g.key() == key))
            .collect()
    }

    /// 重算全部已发布摘要并与 `DIGESTS.lock` 比对（§3.2 / 矩阵 #10）。
    pub fn verify_digests(&self) -> Result<(), CatalogError> {
        let mut seen: BTreeSet<(String, u32)> = BTreeSet::new();
        for def in self.iter() {
            let key = (def.variable_id.clone(), def.revision);
            seen.insert(key.clone());
            let actual =
                Self::compute_digest(def).map_err(|e| CatalogError::InvalidDefinition {
                    variable_id: def.variable_id.clone(),
                    revision: def.revision,
                    reason: format!("摘要计算失败：{e}"),
                })?;
            if def.digest != actual {
                return Err(CatalogError::DigestMismatch {
                    variable_id: def.variable_id.clone(),
                    revision: def.revision,
                    expected: def.digest.clone(),
                    actual,
                });
            }
            match self.locks.get(&key) {
                None => {
                    return Err(CatalogError::MissingLockEntry {
                        variable_id: def.variable_id.clone(),
                        revision: def.revision,
                    });
                }
                Some(expected) if expected != &actual => {
                    return Err(CatalogError::DigestMismatch {
                        variable_id: def.variable_id.clone(),
                        revision: def.revision,
                        expected: expected.clone(),
                        actual,
                    });
                }
                Some(_) => {}
            }
        }
        if let Some((variable_id, revision)) = self.locks.keys().find(|k| !seen.contains(*k)) {
            return Err(CatalogError::UnexpectedLockEntry {
                variable_id: variable_id.clone(),
                revision: *revision,
            });
        }
        Ok(())
    }

    /// canonical JSON（§3.2 五条规则）→ lowercase hex SHA-256。
    pub fn compute_digest(def: &VariableDefinition) -> Result<String, DigestError> {
        let mut value =
            serde_json::to_value(def).map_err(|e| DigestError::Serialize(e.to_string()))?;
        let obj = value
            .as_object_mut()
            .ok_or_else(|| DigestError::Serialize("定义不是 JSON 对象".into()))?;
        // 规则 3：`digest` 字段本身不参与。
        obj.remove("digest");
        // 规则 5：缺省可选字段不出现（serde 的 skip_serializing_if 已保证；
        // 这里再兜一道 null，避免将来新增字段时静默引入「显式 null」形态）。
        let canonical = canonicalize(&value);
        let text =
            serde_json::to_string(&canonical).map_err(|e| DigestError::Serialize(e.to_string()))?;
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        Ok(format!("{:x}", hasher.finalize()))
    }
}

/// 按 **git blob** 语义计算一段文本的 SHA-1（`sha1("blob <字节数>\0" + 内容)`）。
///
/// 为什么需要它：sidecar 的 `binds_to.definition_blob_sha1` 记录的是 `r1.json`
/// 在 git 里的 blob id（`git hash-object` 的输出），这是仓库里唯一与提交内容
/// 一一对应的度量。装载期必须自己算一遍才能证明「这份 sidecar 描述的确实是当前
/// 这份定义」，否则 sidecar 与定义会在无人察觉时漂移。
///
/// 为什么先做 CRLF → LF 归一：Windows 工作树是 CRLF（`core.autocrlf=true`），而
/// git 里的 blob 是 LF。`include_str!` 嵌入的是**工作树字节**，不归一就会在
/// Windows 上算出与 Linux 不同的摘要，让同一份数据在两平台得出不同结论。
/// 这也与本仓 `tools/ci/*.py` 里「raw/lf 双候选」的判定口径一致。
pub fn git_blob_sha1(text: &str) -> String {
    use sha1::{Digest, Sha1};

    let normalized = text.replace("\r\n", "\n");
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", normalized.len()).as_bytes());
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 递归 canonical 化：对象键按**字节序**升序，数组保序，`null` 不出现。
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            let mut out = Map::new();
            for k in keys {
                if map[k].is_null() {
                    continue;
                }
                out.insert(k.clone(), canonicalize(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

/// ID 形状：`^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$`。
fn valid_variable_id(id: &str) -> bool {
    let mut segs = id.split('.');
    let head = match segs.next() {
        Some(h) => h,
        None => return false,
    };
    if head.is_empty() || !head.as_bytes()[0].is_ascii_lowercase() {
        return false;
    }
    if !head
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return false;
    }
    let mut count = 0;
    for seg in segs {
        if seg.is_empty()
            || !seg
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return false;
        }
        count += 1;
    }
    count >= 1
}

/// 键名形状：`^[a-z][a-z0-9_]{0,63}$`。
fn valid_key_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

/// 身份模板解析结果。
struct TemplateShape {
    /// `#` 段（arguments）中的键，保序。
    args: Vec<String>,
    /// `@` 段（targets）中的键，保序。
    targets: Vec<String>,
}

/// 解析并校验身份模板：必须是 `<variable_id>[#arglist][@targetlist]`，
/// 每个段是 `key={key}`，段内键名按字节序升序且不重复（§4.1.1 C/排序规则）。
fn parse_identity_template(template: &str, variable_id: &str) -> Result<TemplateShape, String> {
    if !template.starts_with(variable_id) {
        return Err(format!("模板必须以 variable_id `{variable_id}` 开头"));
    }
    let rest = &template[variable_id.len()..];
    let mut args = Vec::new();
    let mut targets = Vec::new();
    let mut cursor = rest;
    if let Some(after) = cursor.strip_prefix('#') {
        let (list, tail) = match after.find('@') {
            Some(i) => (&after[..i], &after[i..]),
            None => (after, ""),
        };
        args = parse_template_segment(list)?;
        cursor = tail;
    }
    if let Some(list) = cursor.strip_prefix('@') {
        targets = parse_template_segment(list)?;
        cursor = "";
    }
    if !cursor.is_empty() {
        return Err(format!("模板尾部无法解析：`{cursor}`"));
    }
    Ok(TemplateShape { args, targets })
}

/// 解析 `k1={k1},k2={k2}` 形式的一段，并校验键名与顺序。
fn parse_template_segment(list: &str) -> Result<Vec<String>, String> {
    let mut keys = Vec::new();
    for item in list.split(',') {
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| format!("段 `{item}` 缺少 `=`"))?;
        if !valid_key_name(key) {
            return Err(format!("键名 `{key}` 不匹配 ^[a-z][a-z0-9_]{{0,63}}$"));
        }
        if value != format!("{{{key}}}") {
            return Err(format!(
                "`{key}` 的取值必须是占位符 `{{{key}}}`，实际 `{value}`"
            ));
        }
        keys.push(key.to_string());
    }
    for pair in keys.windows(2) {
        if pair[0].as_bytes() >= pair[1].as_bytes() {
            return Err(format!(
                "段内键名必须按字节序**严格升序**且不重复：`{}` 之后是 `{}`",
                pair[0], pair[1]
            ));
        }
    }
    Ok(keys)
}

/// 定义装载期自检（**A-1 静态覆盖判据**在此强制，见 §5.2）。
fn validate_definition(def: &VariableDefinition) -> Result<(), CatalogError> {
    let fail = |reason: String| CatalogError::InvalidDefinition {
        variable_id: def.variable_id.clone(),
        revision: def.revision,
        reason,
    };

    // ---- ID 形状与保留前缀（§4.1.1 E）----
    if !valid_variable_id(&def.variable_id) {
        return Err(fail(format!(
            "variable_id `{}` 不匹配 ^[a-z][a-z0-9_]*(\\.[a-z0-9_]+)+$",
            def.variable_id
        )));
    }
    if def.variable_id == "native" || def.variable_id.starts_with("native.") {
        return Err(fail(
            "variable_id 不得等于 `native`、也不得以 `native.` 开头（保留前缀）".into(),
        ));
    }
    if def.revision < MIN_DEFINITION_REVISION {
        return Err(fail(format!("revision 必须 >= {MIN_DEFINITION_REVISION}")));
    }
    if def.family.trim().is_empty() {
        return Err(fail("family 不得为空".into()));
    }
    if def.label.zh.trim().is_empty() || def.label.en.trim().is_empty() {
        return Err(fail("label 的中/英文均不得为空".into()));
    }
    if def.source.is_empty() {
        return Err(fail(
            "source 不得为空（必须可追溯到手册/实测/身份探针/冻结向量）".into(),
        ));
    }

    // ---- Target 声明合法性 ----
    if def.targets.is_empty() {
        return Err(fail("targets 不得为空".into()));
    }
    let mut seen_targets = BTreeSet::new();
    for k in &def.targets {
        if !seen_targets.insert(*k) {
            return Err(fail(format!("targets 含重复变体 `{}`", k.as_str())));
        }
    }
    let mut required = BTreeSet::new();
    for k in &def.target_required {
        if !def.targets.contains(k) {
            return Err(fail(format!(
                "target_required 含未声明变体 `{}`（必须 ⊆ targets）",
                k.as_str()
            )));
        }
        if !required.insert(*k) {
            return Err(fail(format!("target_required 含重复变体 `{}`", k.as_str())));
        }
    }

    // ---- arguments ----
    let mut arg_keys = BTreeSet::new();
    for spec in &def.arguments {
        if !valid_key_name(&spec.key) {
            return Err(fail(format!(
                "argument key `{}` 不匹配 ^[a-z][a-z0-9_]{{0,63}}$",
                spec.key
            )));
        }
        if !arg_keys.insert(spec.key.as_str()) {
            return Err(fail(format!("argument key `{}` 重复", spec.key)));
        }
        match spec.value_type {
            ArgType::Integer => {
                if spec.enum_options.is_some() {
                    return Err(fail(format!(
                        "integer argument `{}` 不得声明 enum_options",
                        spec.key
                    )));
                }
            }
            ArgType::Enum => match &spec.enum_options {
                Some(opts) if !opts.is_empty() => {}
                _ => {
                    return Err(fail(format!(
                        "enum argument `{}` 必须声明非空 enum_options",
                        spec.key
                    )));
                }
            },
        }
        if let (Some(min), Some(max)) = (spec.min, spec.max)
            && min > max
        {
            return Err(fail(format!("argument `{}` 的 min > max", spec.key)));
        }
        if let Some(v) = &spec.default
            && v.is_null()
        {
            return Err(fail(format!(
                "argument `{}` 的 default 不得为 null（缺省不得物化，§4.1.1 H-6）",
                spec.key
            )));
        }
    }

    // ---- 寻址规则 ----
    if def.transport_size == 0 || !TRANSPORT_SIZES.contains(&def.transport_size) {
        return Err(fail(format!(
            "transport_size `{:#04x}` 不在 {TRANSPORT_SIZES:?} 内",
            def.transport_size
        )));
    }
    if def.count == 0 {
        return Err(fail("count 必须 >= 1".into()));
    }
    if def.unit_mode != NckUnitMode::Current {
        return Err(fail(
            "本期定义一律 unit_mode=current（未验证其它单位模式，不引入未验证地址）".into(),
        ));
    }
    if let Some(rule) = &def.invalid_rule {
        if !rule.value.is_finite() || !rule.tolerance.is_finite() {
            return Err(fail("invalid_rule 必须为有限数值".into()));
        }
        if !(0.0..=1.0).contains(&rule.tolerance) {
            return Err(fail("invalid_rule.tolerance 必须在 0..=1".into()));
        }
        if !def.decode.is_numeric() {
            return Err(fail("非数值 decode 不得声明 invalid_rule".into()));
        }
    }
    // 固定字段必须落在原生解析器的范围内（否则定义永远无法编译）。
    if let NativeFieldSource::Fixed { value } = &def.addressing.area_no
        && *value > u32::from(crate::native::MAX_AREA_NO)
    {
        return Err(fail(format!(
            "area_no 固定值 {value} 超过协议上界 {}",
            crate::native::MAX_AREA_NO
        )));
    }
    if let NativeFieldSource::Fixed { value } = &def.addressing.module {
        if *value > u32::from(u8::MAX) {
            return Err(fail(format!("module 固定值 {value} 超过 255")));
        }
    } else if !matches!(def.addressing.module, NativeFieldSource::DriveEntryModule) {
        return Err(fail(
            "module 只允许 Fixed(...) 或 DriveEntryModule（入口身份是唯一 26/130 出处）".into(),
        ));
    }
    if let NativeFieldSource::Fixed { value } = &def.addressing.line
        && *value > u32::from(u16::MAX)
    {
        return Err(fail(format!("line 固定值 {value} 超过 65535")));
    }

    // ---- 身份模板 / 身份键（§4.1.1 C + A-1）----
    let shape = parse_identity_template(&def.identity_template, &def.variable_id)
        .map_err(|reason| fail(format!("identity_template 非法：{reason}")))?;
    let mut template_paths: Vec<&str> = shape
        .args
        .iter()
        .chain(shape.targets.iter())
        .map(|s| s.as_str())
        .collect();
    template_paths.sort_unstable();
    if template_paths.windows(2).any(|w| w[0] == w[1]) {
        return Err(fail("identity_template 的键名跨段重复".into()));
    }
    let mut key_paths: Vec<&str> = def.identity_keys.iter().map(|k| k.path.as_str()).collect();
    let sorted_keys = {
        let mut v = key_paths.clone();
        v.sort_unstable();
        v
    };
    if sorted_keys.windows(2).any(|w| w[0] == w[1]) {
        return Err(fail("identity_keys 含重复 path".into()));
    }
    key_paths.sort_unstable();
    if key_paths != template_paths {
        return Err(fail(format!(
            "identity_keys 叶路径集合 {key_paths:?} != identity_template 占位符集合 {template_paths:?}（A-1b）"
        )));
    }
    for spec in &def.identity_keys {
        if !spec.required {
            return Err(fail(format!(
                "identity_keys `{}` 必须 required=true：本期不引入 cosmetic_keys，等价规则为空集（§5.2 A-1c）",
                spec.path
            )));
        }
        let is_target = TargetKind::parse(&spec.path).is_some();
        let is_arg = arg_keys.contains(spec.path.as_str());
        if !is_target && !is_arg {
            return Err(fail(format!(
                "identity_keys `{}` 既不是 Target 变体名也不是本定义的 argument",
                spec.path
            )));
        }
    }

    // A-1a：影响寻址的来源必须被身份覆盖（含 DriveEntryModule ⇒ drive_object）。
    let mut cover: BTreeSet<&str> = BTreeSet::new();
    for source in def.addressing.sources() {
        match source {
            NativeFieldSource::Fixed { .. } => {}
            NativeFieldSource::Target { kind } => {
                cover.insert(kind.as_str());
            }
            NativeFieldSource::Argument { key } | NativeFieldSource::ArgumentOffset { key, .. } => {
                cover.insert(key.as_str());
            }
            NativeFieldSource::DriveEntryModule => {
                cover.insert(TargetKind::DriveObject.as_str());
            }
        }
    }
    for needed in &cover {
        if !key_paths.contains(needed) {
            return Err(fail(format!(
                "身份覆盖缺失：寻址来源 `{needed}` 未被 identity_keys 覆盖（A-1a）"
            )));
        }
    }
    // A-1d：必填 target 必须进身份。
    for kind in &def.target_required {
        if !key_paths.contains(&kind.as_str()) {
            return Err(fail(format!(
                "必填 target `{}` 未出现在 identity_keys 中（A-1d）",
                kind.as_str()
            )));
        }
    }

    // ---- golden 绑定 ----
    if def.golden_ref.is_empty() {
        return Err(fail(
            "golden_ref 不得为空（语义 ID 必须绑定冻结 wire 向量）".into(),
        ));
    }
    let mut seen_refs = BTreeSet::new();
    for g in &def.golden_ref {
        if g.area.is_empty() || g.block.is_empty() || g.variable.is_empty() {
            return Err(fail("golden_ref 的 area/block/variable 均不得为空".into()));
        }
        if !seen_refs.insert(g.key()) {
            return Err(fail("golden_ref 含重复条目".into()));
        }
    }
    Ok(())
}

impl VariableDefinition {
    /// 语义参数校验（D8）+ 身份键存在性（A9）。
    ///
    /// 三态语义（§4.1.1 H-6，**逐条固定**）：
    /// - 键不存在（缺省）⇒ `MISSING_IDENTITY_KEY`（身份键）/ `INVALID_ARGUMENTS`（其它必填）；
    /// - 键存在且为 `null` ⇒ `INVALID_ARGUMENTS`（身份键不得为 null）；
    /// - 键存在且为空串/0 ⇒ 正常取值（**不**与缺省合并，缺省**绝不**物化 `default`）。
    pub fn validate_arguments(&self, arguments: &Map<String, Value>) -> Result<(), CompileError> {
        // 1) 提供的键必须已声明（未知键即拒，绝不忽略）。
        for key in arguments.keys() {
            if !self.arguments.iter().any(|a| &a.key == key) {
                return Err(CompileError::invalid_arguments(format!(
                    "`{}` 未在 `{}`(rev {}) 的 arguments 规格中声明",
                    key, self.variable_id, self.revision
                )));
            }
        }
        // 2) 类型与范围（只校验**显式提供**的值；缺省不物化 default）。
        for spec in &self.arguments {
            let Some(value) = arguments.get(&spec.key) else {
                continue;
            };
            if value.is_null() {
                return Err(CompileError::invalid_arguments(format!(
                    "argument `{}` 为显式 null（身份键不得为 null；缺省与 null 三态可区分）",
                    spec.key
                )));
            }
            match spec.value_type {
                ArgType::Integer => {
                    let n = value.as_i64().ok_or_else(|| {
                        CompileError::invalid_arguments(format!(
                            "argument `{}` 需为整数，实际 {value}",
                            spec.key
                        ))
                    })?;
                    if let Some(min) = spec.min
                        && n < min
                    {
                        return Err(CompileError::invalid_arguments(format!(
                            "argument `{}` = {n} 小于下界 {min}",
                            spec.key
                        )));
                    }
                    if let Some(max) = spec.max
                        && n > max
                    {
                        return Err(CompileError::invalid_arguments(format!(
                            "argument `{}` = {n} 超过上界 {max}",
                            spec.key
                        )));
                    }
                }
                ArgType::Enum => {
                    let s = value.as_str().ok_or_else(|| {
                        CompileError::invalid_arguments(format!(
                            "enum argument `{}` 需为字符串，实际 {value}",
                            spec.key
                        ))
                    })?;
                    let options = spec.enum_options.as_deref().unwrap_or(&[]);
                    if !options.iter().any(|o| o == s) {
                        return Err(CompileError::invalid_arguments(format!(
                            "argument `{}` = `{s}` 不在 {options:?} 内",
                            spec.key
                        )));
                    }
                }
            }
        }
        // 3) 身份键：缺失 ⇒ MISSING_IDENTITY_KEY（与「必填 argument 缺失」共用同一判据，
        //    但身份层的缺失是更精确的诊断，故优先给出）。
        let identity_paths: BTreeSet<&str> =
            self.identity_keys.iter().map(|k| k.path.as_str()).collect();
        for spec in &self.arguments {
            if arguments.contains_key(&spec.key) {
                continue;
            }
            let is_identity = identity_paths.contains(spec.key.as_str());
            if is_identity {
                return Err(CompileError::missing_identity_key(
                    &self.variable_id,
                    self.revision,
                    &spec.key,
                ));
            }
            if spec.required {
                return Err(CompileError::invalid_arguments(format!(
                    "缺少必填 argument `{}`",
                    spec.key
                )));
            }
        }
        Ok(())
    }

    /// canonical identity：`identity_template` 按 `identity_keys` 的叶路径做规范替换。
    ///
    /// 替换前的输入**必须**已经过 D1–D10（未知键与非法类型永远不进入身份计算）；
    /// 值的编码一律走 `mesa_core_types::encode_typed_value`（与 Core / 前端**同一函数**，
    /// 因此三处不可能漂移）。
    pub fn canonical_identity(
        &self,
        target: &TargetRef,
        arguments: &Map<String, Value>,
    ) -> Result<String, CompileError> {
        let mut encoded: BTreeMap<&str, String> = BTreeMap::new();
        for spec in &self.identity_keys {
            let (value, from_target) = match target.value_for_path(&spec.path) {
                Some(v) => (v, true),
                None => match arguments.get(&spec.path) {
                    Some(v) => (v.clone(), false),
                    None => {
                        return Err(CompileError::missing_identity_key(
                            &self.variable_id,
                            self.revision,
                            &spec.path,
                        ));
                    }
                },
            };
            if value.is_null() {
                return Err(CompileError::invalid_arguments(format!(
                    "身份键 `{}` 为 null（缺省与 null 三态可区分，身份层不接受 null）",
                    spec.path
                )));
            }
            let text = mesa_core_types::encode_typed_value(&value).map_err(|e| {
                if from_target {
                    CompileError::invalid_target(format!(
                        "身份键 `{}` 无法规范编码：{e}",
                        spec.path
                    ))
                } else {
                    CompileError::invalid_arguments(format!(
                        "身份键 `{}` 无法规范编码：{e}",
                        spec.path
                    ))
                }
            })?;
            encoded.insert(spec.path.as_str(), text);
        }
        render_identity_template(&self.identity_template, &encoded)
    }
}

/// 模板渲染：把每个 `{path}` 替换为已经规范编码的值。
fn render_identity_template(
    template: &str,
    encoded: &BTreeMap<&str, String>,
) -> Result<String, CompileError> {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map(|i| open + i).ok_or_else(|| {
            CompileError::invalid_binding_config(format!(
                "identity_template `{template}` 含未闭合的 `{{`"
            ))
        })?;
        let path = &rest[open + 1..close];
        let value = encoded.get(path).ok_or_else(|| {
            CompileError::invalid_binding_config(format!(
                "identity_template 占位符 `{{{path}}}` 不在 identity_keys 中"
            ))
        })?;
        out.push_str(value);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// 原生地址 → `identity_mode="derived"` 的固定 Key 规则（§4.1.1 C 表末行）。
///
/// **不含** `count`/`decode`/`transport_size`/`invalid_*`/`identity_mode`：
/// 它们是质量与解码属性、模式声明，都不是身份（§4.1.2）。
pub fn native_identity_key(address: &crate::native::NativeNckAddress) -> String {
    format!(
        "native.{}.{}.{}.{}.{}",
        address.area.letter(),
        address.area_no,
        address.module,
        address.column,
        address.line
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{NativeNckAddress, resolve_native};
    use serde_json::json;

    fn def(v: Value) -> VariableDefinition {
        serde_json::from_value(v).expect("测试定义必须可反序列化")
    }

    /// 最小合法定义（供坏定义用例照抄后改一处）。
    fn good_json() -> Value {
        json!({
            "variable_id": "channel.program.status",
            "revision": 1,
            "family": "828d",
            "scope": "collect",
            "semantics_status": "verified",
            "label": {"zh": "状态", "en": "status"},
            "source": [{"kind": "frozen_vector", "reference": "tests/fixtures/legacy-828d-wire.json"}],
            "targets": ["channel"],
            "target_required": ["channel"],
            "arguments": [],
            "addressing": {
                "area": "C",
                "area_no": {"source": "target", "kind": "channel"},
                "module": {"source": "fixed", "value": 127},
                "column": 13,
                "line": {"source": "fixed", "value": 1}
            },
            "decode": "U16_LE",
            "transport_size": 9,
            "count": 1,
            "unit_mode": "current",
            "identity_template": "channel.program.status@channel={channel}",
            "identity_keys": [{"path": "channel", "required": true}],
            "golden_ref": [{"area": "C", "block": "S", "variable": "progStatus", "entry": "nc_variable"}]
        })
    }

    #[test]
    fn embedded_catalog_loads_and_is_self_consistent() {
        let cat = DefinitionCatalog::embedded().expect("内嵌定义必须自检通过");
        assert_eq!(cat.len(), 42, "A16 + B20 + C6");
        assert_eq!(cat.for_family("828d").len(), 42);
        assert!(cat.for_family("840d-sl").is_empty(), "840d-sl 无任何定义");
    }

    #[test]
    fn reserved_prefix_and_id_shape_are_enforced_at_load() {
        let mut bad = good_json();
        bad["variable_id"] = json!("native.foo");
        bad["identity_template"] = json!("native.foo@channel={channel}");
        let err = DefinitionCatalog::from_definitions(vec![def(bad)]).unwrap_err();
        assert!(err.to_string().contains("保留前缀"), "{err}");

        let mut bad = good_json();
        bad["variable_id"] = json!("Channel.Status");
        let err = DefinitionCatalog::from_definitions(vec![def(bad)]).unwrap_err();
        assert!(err.to_string().contains("不匹配"), "{err}");
    }

    #[test]
    fn a1_static_cover_is_enforced_at_load() {
        // (i) 去掉 identity_keys ⇒ 模板占位符与键集合不等（A-1b）。
        let mut bad = good_json();
        bad["identity_keys"] = json!([]);
        let err = DefinitionCatalog::from_definitions(vec![def(bad)]).unwrap_err();
        assert!(err.to_string().contains("A-1b"), "{err}");

        // (ii) 复合 Target（channel + axis）本身**必须合法**——`{channel,axis}` 是
        // 合法复合 Target，不得因含两个字段被拒。
        let mut composite = good_json();
        composite["targets"] = json!(["channel", "axis"]);
        composite["target_required"] = json!(["channel", "axis"]);
        composite["addressing"]["line"] = json!({"source": "target", "kind": "axis"});
        composite["identity_template"] =
            json!("channel.program.status@axis={axis},channel={channel}");
        composite["identity_keys"] = json!([
            {"path": "axis", "required": true},
            {"path": "channel", "required": true}
        ]);
        let ok = DefinitionCatalog::from_definitions(vec![def(composite.clone())]);
        assert!(ok.is_ok(), "复合 Target 定义必须合法：{:?}", ok.err());

        // (iii) 把 axis 从键集合与模板中删掉、却仍让 line 取自 axis ⇒ A-1a 必须拒绝
        //       （影响寻址的来源没有进入身份）。
        let mut leak = composite.clone();
        leak["identity_template"] = json!("channel.program.status@channel={channel}");
        leak["identity_keys"] = json!([{"path": "channel", "required": true}]);
        let err = DefinitionCatalog::from_definitions(vec![def(leak)]).unwrap_err();
        assert!(err.to_string().contains("A-1a"), "{err}");

        // (iv) 必填 Target 未进身份 ⇒ A-1d 必须拒绝（先删 axis 键，再改模板以通过 A-1b）。
        let mut a1d = good_json();
        a1d["targets"] = json!(["channel", "axis"]);
        a1d["target_required"] = json!(["channel", "axis"]);
        a1d["addressing"]["line"] = json!({"source": "fixed", "value": 1});
        a1d["identity_template"] = json!("channel.program.status@channel={channel}");
        a1d["identity_keys"] = json!([{"path": "channel", "required": true}]);
        let err = DefinitionCatalog::from_definitions(vec![def(a1d)]).unwrap_err();
        assert!(err.to_string().contains("A-1d"), "{err}");
    }

    #[test]
    fn template_segment_order_is_byte_ascending() {
        let mut bad = good_json();
        bad["targets"] = json!(["channel", "axis"]);
        bad["target_required"] = json!(["channel", "axis"]);
        bad["addressing"]["line"] = json!({"source": "target", "kind": "axis"});
        // 乱序：channel 在 axis 之前（'c' > 'a'）⇒ 必须拒绝。
        bad["identity_template"] = json!("channel.program.status@channel={channel},axis={axis}");
        bad["identity_keys"] = json!([
            {"path": "channel", "required": true},
            {"path": "axis", "required": true}
        ]);
        let err = DefinitionCatalog::from_definitions(vec![def(bad)]).unwrap_err();
        assert!(err.to_string().contains("升序"), "{err}");
    }

    #[test]
    fn same_variable_id_must_keep_identical_identity_contract() {
        // 复合 Target 定义（channel + axis）：同 variable_id 的 rev1 / rev2 必须给出
        // **逐字相同**的 identity_template / identity_keys。
        let mut base = good_json();
        base["targets"] = json!(["channel", "axis"]);
        base["target_required"] = json!(["channel", "axis"]);
        base["addressing"]["line"] = json!({"source": "target", "kind": "axis"});
        base["identity_template"] = json!("channel.program.status@axis={axis},channel={channel}");
        base["identity_keys"] = json!([
            {"path": "axis", "required": true},
            {"path": "channel", "required": true}
        ]);
        let mut r2 = base.clone();
        r2["revision"] = json!(2);
        let ok = DefinitionCatalog::from_definitions(vec![def(base.clone()), def(r2.clone())]);
        assert!(ok.is_ok(), "{:?}", ok.err());

        // 同一 variable_id 的 rev2 把 identity_keys 顺序换了（集合相同、字节不同）
        // ⇒ 必须拒绝：否则「仅 revision 变化 ⇒ point_key 不变」失效（§3.5）。
        r2["identity_keys"] = json!([
            {"path": "channel", "required": true},
            {"path": "axis", "required": true}
        ]);
        let err = DefinitionCatalog::from_definitions(vec![def(base), def(r2)]).unwrap_err();
        assert!(err.to_string().contains("新 variable_id"), "{err}");
    }

    #[test]
    fn digest_ignores_its_own_field_and_absent_optionals() {
        let mut a = def(good_json());
        let base = DefinitionCatalog::compute_digest(&a).unwrap();
        a.digest = "whatever".into();
        assert_eq!(DefinitionCatalog::compute_digest(&a).unwrap(), base);
        assert_eq!(base.len(), 64);
        assert!(
            base.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        // 缺省可选字段不出现在 canonical JSON 中（不写 null）。
        let value = serde_json::to_value(&a).unwrap();
        assert!(value.get("unit").is_none());
        assert!(value.get("invalid_rule").is_none());
        // 数组保序：source 顺序变化必须改变摘要。
        let mut b = def(good_json());
        b.source.push(DefinitionSource {
            kind: SourceKind::Manual,
            reference: "Siemens 828D NC variables 03/2025".into(),
            note: None,
        });
        assert_ne!(DefinitionCatalog::compute_digest(&b).unwrap(), base);
    }

    #[test]
    fn lookup_never_falls_back_to_another_revision() {
        let mut r3 = good_json();
        r3["revision"] = json!(3);
        let cat = DefinitionCatalog::from_definitions(vec![def(good_json()), def(r3)]).unwrap();
        assert_eq!(
            cat.available_revisions("channel.program.status"),
            vec![1, 3]
        );
        assert_eq!(cat.lookup("channel.program.status", 1).unwrap().revision, 1);
        let err = cat.lookup("channel.program.status", 2).unwrap_err();
        assert_eq!(err.code(), "UNSUPPORTED_DEFINITION_REVISION");
        assert!(err.to_string().contains("[1, 3]"), "{err}");
        assert_eq!(
            cat.lookup("nope.nope", 1).unwrap_err().code(),
            "UNKNOWN_VARIABLE"
        );
    }

    #[test]
    fn canonical_identity_matches_frozen_literals() {
        let cat = DefinitionCatalog::embedded().unwrap();
        let cases = [
            (
                "channel.program.status",
                json!({"channel": 1}),
                json!({}),
                "channel.program.status@channel=i:1",
            ),
            (
                "channel.r_parameter",
                json!({"channel": 1}),
                json!({"index": 100}),
                "channel.r_parameter#index=i:100@channel=i:1",
            ),
            (
                "axis.position.commanded_mcs",
                json!({"channel": 1, "axis": 3}),
                json!({}),
                "axis.position.commanded_mcs@axis=i:3,channel=i:1",
            ),
            (
                "spindle.speed.actual",
                json!({"channel": 1, "spindle": 1}),
                json!({}),
                "spindle.speed.actual@channel=i:1,spindle=i:1",
            ),
            (
                "drive.motor.temperature",
                json!({"drive_object": {"number": 3, "entry": "nc_variable"}}),
                json!({}),
                "drive.motor.temperature@drive_object=o:{entry=s:nc_variable,number=i:3}",
            ),
        ];
        for (id, target, args, want) in cases {
            let d = cat.lookup(id, 1).unwrap();
            let t = TargetRef::from_json(&target).unwrap();
            let got = d.canonical_identity(&t, args.as_object().unwrap()).unwrap();
            assert_eq!(got, want, "{id}");
        }
    }

    #[test]
    fn native_identity_key_is_the_fixed_derived_rule() {
        let mut m = Map::new();
        m.insert("area".into(), json!("C"));
        m.insert("area_no".into(), json!(1));
        m.insert("module".into(), json!(21));
        m.insert("column".into(), json!(1));
        m.insert("line".into(), json!(396));
        m.insert("count".into(), json!(1));
        m.insert("unit_mode".into(), json!("current"));
        m.insert("decode".into(), json!("F64_LE"));
        m.insert("transport_size".into(), json!("0x09"));
        let addr = NativeNckAddress::from_parameters(&m).unwrap();
        resolve_native(addr.clone()).unwrap();
        assert_eq!(native_identity_key(&addr), "native.C.1.21.1.396");
    }
}
