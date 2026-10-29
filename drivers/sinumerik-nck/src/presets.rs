//! 配置预填模板（Preset）：**可选**的配置素材，不参与采集准入。
//!
//! 设计边界（V2.0 §4，PR-B 起叠加语义边界）：
//! - Preset 只提供**结构固定字段**的预填（area/module/column/count/unit_mode/
//!   decode/transport_size/unit）；`area_no` 与 `line` 一律留给使用者确认，
//!   不猜轴/通道/行索引语义；
//! - **模板数据逐字不动**（C2=P1）：`PresetTemplate.parameters` 保持**原生**字段，
//!   它同时是旧等价门 `tests/legacy_equivalence.rs` 的真值来源；语义索引走定义
//!   目录的 `golden_ref`，**不由模板反向生成定义**（禁止自证循环）；
//! - **浏览叶产出语义载荷**（M4）：叶的 `binding_json` 是
//!   `{"kind":"nck_variable","parameters":{variable_id,definition_revision,target,arguments}}`
//!   并旁挂 `identity` 块。语义载荷由「模板 id → `GoldenRef` → 定义目录」这条
//!   **只读索引**链构造，原生字段不再出现在载荷里；
//! - 模板是否可用**不影响**采集准入：加载失败只让模板浏览为空并给出诊断，
//!   数据面照常工作；
//! - 含 `drive_object_module` 的条目拆成两个独立模板（module 26 / module 130），
//!   因为两条入口实例编号不同，不得互相换算。
//!
//! 本模块不做：地址准入、设备探测、自动填充缺失必填项。

use mesa_driver_protocol::browse_selection as wire_selection;
use mesa_driver_protocol::pb::{BrowseDiagnosticSeverity, BrowseNode, BrowseSelectionDiagnostic};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::browse_selection as admission;
use crate::compiler::{CompileError, POINT_KIND_VARIABLE};
use crate::definition::{
    DefinitionCatalog, DefinitionScope, GoldenRef, NativeFieldSource, VariableDefinition,
};
use crate::native::{NativeError, NativeNckAddress};
use crate::targets::{DriveEntry, DriveObjectRef, TargetKind, TargetRef};

/// 内嵌模板（随驱动发布；解析失败只影响浏览）。
pub const EMBEDDED_828D: &str = include_str!("../presets/828d.json");

/// 单个预填模板。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PresetTemplate {
    pub id: String,
    pub family: String,
    pub label: String,
    /// 候选状态（当前恒 `candidate`；不得据此宣称硬件已验证）。
    pub validation_status: String,
    /// 结构固定字段的预填（**不含** area_no/line）。
    pub parameters: Map<String, Value>,
    /// 仍需使用者确认的字段（保持缺失，由通用必填校验阻止保存）。
    pub required_user_inputs: Vec<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct PresetDoc {
    #[serde(default)]
    templates: Vec<PresetTemplate>,
}

/// 只读模板集合；加载失败退化为空集并保留原因（不 panic、不阻塞采集）。
#[derive(Debug, Clone, Default)]
pub struct PresetRegistry {
    templates: Vec<PresetTemplate>,
    error: Option<String>,
}

impl PresetRegistry {
    /// 从 JSON 文本装载；非法即空集 + 诊断（**绝不**影响采集路径）。
    pub fn load(json: &str) -> Self {
        let doc: PresetDoc = match serde_json::from_str(json) {
            Ok(d) => d,
            Err(e) => {
                return Self {
                    templates: Vec::new(),
                    error: Some(format!("preset JSON 非法：{e}")),
                };
            }
        };
        let mut kept = Vec::new();
        let mut rejects = Vec::new();
        for t in doc.templates {
            match validate_template(&t) {
                Ok(()) => kept.push(t),
                Err(reason) => rejects.push(format!("{}: {reason}", t.id)),
            }
        }
        let error = (!rejects.is_empty()).then(|| {
            format!(
                "{} 个模板被拒绝（仅影响模板浏览）：{}",
                rejects.len(),
                rejects.join("; ")
            )
        });
        Self {
            templates: kept,
            error,
        }
    }

    /// 随驱动内嵌的模板集合。
    pub fn embedded() -> Self {
        Self::load(EMBEDDED_828D)
    }

    /// 加载诊断（模板不可用时给出原因；`None` 表示全部合法）。
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// 指定系列的模板（稳定按 id 排序）。
    pub fn for_family(&self, family: &str) -> Vec<&PresetTemplate> {
        let mut v: Vec<&PresetTemplate> = self
            .templates
            .iter()
            .filter(|t| t.family == family)
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
}

/// 模板结构校验：固定字段必须能通过**同一套**原生解析规则。
///
/// 做法：把固定字段与占位的最小合法 `area_no`/`line` 组合后跑一次
/// `NativeNckAddress::from_parameters`，任何不合法（枚举/范围/类型/
/// 无效值规则配对）都在装载期暴露，而不是等使用者填完才发现。
fn validate_template(t: &PresetTemplate) -> Result<(), String> {
    if t.id.trim().is_empty() || t.label.trim().is_empty() {
        return Err("id/label 不得为空".into());
    }
    if t.validation_status != "candidate" {
        return Err(format!(
            "validation_status 必须为 candidate（实际 `{}`）",
            t.validation_status
        ));
    }
    // 模板不得预填 area_no/line（必须由使用者确认）。
    for key in ["area_no", "line"] {
        if t.parameters.contains_key(key) {
            return Err(format!("模板不得预填 `{key}`（须使用者确认）"));
        }
    }
    if !t.required_user_inputs.iter().any(|k| k == "area_no")
        || !t.required_user_inputs.iter().any(|k| k == "line")
    {
        return Err("required_user_inputs 必须包含 area_no 与 line".into());
    }
    let mut probe = t.parameters.clone();
    probe.insert("area_no".into(), Value::from(0));
    probe.insert("line".into(), Value::from(0));
    NativeNckAddress::from_parameters(&probe)
        .map(|_| ())
        .map_err(|e: NativeError| e.to_string())
}

/// 语义载荷不可用时的标签注记（**文案逐字沿用** IPC 1.7 的既有口径）。
const UNAVAILABLE_LABEL_NOTE: &str =
    " · 语义载荷不可用（内嵌定义目录缺少该变量；仅可作原生高级配置）";

/// 语义载荷不可用时的诊断文案（与前端历史展示文案保持一致）。
const UNAVAILABLE_DIAGNOSTIC: &str = "语义载荷不可用（内嵌定义目录缺少该变量）：只能使用「高级原生配置」，不得走旧 variable 采集入口";

/// 模板 → 浏览叶。
///
/// **语义化（M4）**：叶的 `binding_json` 从原生信封
/// `{"resource_id":"variable","parameters":{原生字段}}` 改为**语义信封**
/// `{"kind":"nck_variable","parameters":{variable_id,definition_revision,target,arguments}}`，
/// 并**旁挂** `identity` 块（`template`/`keys`/`preview_point_key`）。
///
/// 为什么可以这样改：模板本身承载的仍是**原生**字段（C2=P1：模板逐字不动），
/// 但浏览叶是**载荷构造**层面——它把「已确认的原生实例」反推成用户要填的语义
/// 坐标，从而让前端不必面对原生字段。`identity` **只出现在浏览/预填载荷**，
/// **不得**进入持久化信封（信封是 `deny_unknown_fields` 的三字段结构）。
///
/// **IPC 1.8 选择契约**：`kind` / `label` / `binding_json` 与 1.7 逐字一致
/// （旧客户端零影响）；可选性、载荷与诊断改由 additive 字段
/// `selection_mode` / `selection_payload_json` / `diagnostics` 表达，
/// 判定全部来自内嵌定义目录（见 [`crate::browse_selection`]）。
pub fn leaf_node(t: &PresetTemplate) -> BrowseNode {
    leaf_node_with(t, &[])
}

/// 模板 → 浏览叶，并填入**已确认**的字段（例如身份核对出的实例编号）。
///
/// 只允许填 `area_no`/`line` 这类使用者本来要填的实例字段；填入后会反映到语义
/// `target`/`arguments`，其余缺失项保持缺失（**不猜测、不给默认值**），
/// 由通用必填校验阻止保存。
pub fn leaf_node_with(t: &PresetTemplate, filled: &[(&str, Value)]) -> BrowseNode {
    leaf_node_in(embedded_catalog(), t, filled)
}

/// 浏览叶装配（目录可注入：生产走内嵌目录，测试用内存目录验证准入分支）。
fn leaf_node_in(
    catalog: Option<&DefinitionCatalog>,
    t: &PresetTemplate,
    filled: &[(&str, Value)],
) -> BrowseNode {
    let mut parameters = t.parameters.clone();
    let mut pending: Vec<&str> = t.required_user_inputs.iter().map(|s| s.as_str()).collect();
    for (key, value) in filled {
        parameters.insert((*key).into(), value.clone());
        pending.retain(|k| k != key);
    }
    let suffix = if pending.is_empty() {
        "字段完整".to_string()
    } else {
        format!("待确认：{}", pending.join("/"))
    };
    // 语义载荷优先；内嵌定义目录缺失该变量时退回原生信封并显式标注
    // （绝不把「找不到定义」伪装成语义可用）。
    let (kind, binding, note, selection) = match semantic_leaf(catalog, t, &parameters) {
        SemanticLeaf::Selectable {
            binding,
            diagnostics,
        } => (
            POINT_KIND_VARIABLE.to_string(),
            binding.clone(),
            String::new(),
            wire_selection::semantic(binding, diagnostics),
        ),
        // 目录命中但准入拒绝：载荷照旧给出（旧客户端按 1.7 形态消费），
        // 可选性由 `NOT_SELECTABLE` + ERROR 诊断明确否认。
        SemanticLeaf::Rejected { binding, err } => (
            POINT_KIND_VARIABLE.to_string(),
            binding.clone(),
            String::new(),
            admission::rejected(binding, &err),
        ),
        SemanticLeaf::Unavailable { binding, note } => (
            "variable".to_string(),
            binding.clone(),
            note,
            wire_selection::rejected(
                binding,
                admission::CODE_SEMANTIC_PAYLOAD_UNAVAILABLE,
                UNAVAILABLE_DIAGNOSTIC,
            ),
        ),
    };
    BrowseNode {
        id: format!("nck://presets/{}", t.id),
        label: format!("{} · {} · {}{}", t.label, t.validation_status, suffix, note),
        kind,
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: binding,
        selection_mode: selection.mode as i32,
        selection_payload_json: selection.payload_json,
        diagnostics: selection.diagnostics,
    }
}

/// 单个浏览叶的语义构造结果。
///
/// 三种状态的差别就是「用户能不能选用」的全部真相，因此**不允许**出现
/// 第四种（例如「有载荷但没说能不能选」）——那会把猜测重新推回前端。
enum SemanticLeaf {
    /// 定义命中且准入通过：可选用的语义叶（提示级诊断不改变可选性）。
    Selectable {
        binding: String,
        diagnostics: Vec<BrowseSelectionDiagnostic>,
    },
    /// 定义命中但准入拒绝（family 不一致 / scope 不可采集）：不可选，理由在诊断里。
    Rejected { binding: String, err: CompileError },
    /// 连语义载荷都构造不出来（目录缺失 / 模板无对应定义）：不可选，退回原生信封。
    Unavailable { binding: String, note: String },
}

/// 语义载荷的构造结果（含仅供展示的预览 Key）。
struct SemanticBinding {
    /// 语义信封 JSON（`kind` + `parameters` + `identity`）。
    json: String,
    /// 预览身份 Key；target/arguments 不完整时为 `None`（**不猜测**）。
    preview_point_key: Option<String>,
}

/// 模板 → 语义叶（**含准入判定**）。
///
/// 判定顺序刻意固定：先问「载荷能不能构造」，再问「能不能采集」。
/// 载荷构造失败时必须退回旧原生信封并显式标注（不能伪装成语义可用）；
/// 准入失败时载荷照旧给出，只把可选性置否——两者对用户的意义完全不同。
fn semantic_leaf(
    catalog: Option<&DefinitionCatalog>,
    t: &PresetTemplate,
    native: &Map<String, Value>,
) -> SemanticLeaf {
    // ① 目录缺失 / 模板 id 无法映射 / 定义查不到 ⇒ 语义载荷不可用。
    let Some((def, binding)) = semantic_payload(catalog, t, native) else {
        return SemanticLeaf::Unavailable {
            binding: legacy_variable_envelope(native),
            note: UNAVAILABLE_LABEL_NOTE.to_string(),
        };
    };

    // ② 准入判定（a）：定义 family 必须与本模板系列一致。
    //    不跨系列套用定义——那会把别的机床系列的点位当成本系列可用。
    if def.family != t.family {
        return SemanticLeaf::Rejected {
            binding: binding.json,
            err: CompileError::UnsupportedFamily {
                definition_family: def.family.clone(),
                device_family: t.family.clone(),
            },
        };
    }

    // ③ 准入判定（b）：只有 `scope=collect` 才允许作为采集点位。
    //    这里的判据与 Configure（`ensure_collectable_scope`）**同一份**定义数据，
    //    理由是「能不能采集」是驱动事实，不该由每个前端各写一份白名单。
    if def.scope != DefinitionScope::Collect {
        return SemanticLeaf::Rejected {
            binding: binding.json,
            err: CompileError::DefinitionScopeNotCollectable {
                variable_id: def.variable_id.clone(),
                revision: def.revision,
                scope: def.scope.as_str(),
            },
        };
    }

    // ④ 可选用：诊断只承载展示性提示（预览 Key / 身份未确定）。
    //    提示级诊断**永不**改变可选性——可选性只由 `selection_mode` 说了算。
    let diagnostics = vec![match &binding.preview_point_key {
        Some(key) => wire_selection::diagnostic(
            admission::CODE_IDENTITY_PREVIEW,
            format!("Browse 预览：{key}（仅展示；提交前仍需服务端确认）"),
            BrowseDiagnosticSeverity::Info,
        ),
        None => wire_selection::diagnostic(
            admission::CODE_IDENTITY_UNRESOLVED,
            "Browse 预览：身份未确定（target/arguments 不完整）——可选用后到语义表单补全，提交前必须由服务端 Preview 确认",
            BrowseDiagnosticSeverity::Warning,
        ),
    }];
    SemanticLeaf::Selectable {
        binding: binding.json,
        diagnostics,
    }
}

/// 内嵌定义目录（进程内惰性装载；装载失败只让**语义叶**退化为原生信封）。
fn embedded_catalog() -> Option<&'static DefinitionCatalog> {
    static CATALOG: std::sync::OnceLock<Option<DefinitionCatalog>> = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(|| DefinitionCatalog::embedded().ok())
        .as_ref()
}

/// 旧 `variable` 原生信封（语义载荷不可用时的降级形态，A22 只允许提示，不得回填）。
fn legacy_variable_envelope(native: &Map<String, Value>) -> String {
    serde_json::json!({
        "resource_id": "variable",
        "parameters": native,
    })
    .to_string()
}

/// 模板 id 形态 `828d.<area>.<block>.<variable>.<entry>` → `GoldenRef` 四元组。
///
/// 这是「语义 `variable_id` ↔ 冻结 wire 向量 ↔ 模板 id」三者的**唯一**连接点：
/// 只做**索引**映射，不复制任何 wire 真值，也不从模板生成定义。
fn golden_ref_of_template(id: &str) -> Option<GoldenRef> {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() != 5 {
        return None;
    }
    Some(GoldenRef {
        area: parts[1].to_string(),
        block: parts[2].to_string(),
        variable: parts[3].to_string(),
        entry: DriveEntry::parse(parts[4])?,
    })
}

/// 语义信封 + `identity` 块（定义目录缺失 / 无对应定义时返回 `None`）。
///
/// 同时把命中的**定义**一并返回：准入判定（family / scope）必须基于同一份
/// 定义，绝不允许「载荷按 A 定义构造、准入按 B 定义判断」。
fn semantic_payload<'a>(
    catalog: Option<&'a DefinitionCatalog>,
    t: &PresetTemplate,
    native: &Map<String, Value>,
) -> Option<(&'a VariableDefinition, SemanticBinding)> {
    let catalog = catalog?;
    let want = golden_ref_of_template(&t.id)?;
    let def = catalog.find_by_golden_ref(&want).first().copied()?;
    let (target, arguments) = prefill_from_native(def, native);
    // 只有 target/arguments 完整时才能给出预览 Key；否则显式 `null`
    // （**不猜测**缺失实例，也不物化可选默认值）。
    let preview = def.canonical_identity(&target, &arguments).ok();
    let mut params = Map::new();
    params.insert("variable_id".into(), Value::from(def.variable_id.clone()));
    params.insert("definition_revision".into(), Value::from(def.revision));
    params.insert(
        "target".into(),
        serde_json::to_value(&target).unwrap_or(Value::Null),
    );
    params.insert("arguments".into(), Value::Object(arguments));
    let identity = serde_json::json!({
        "template": def.identity_template,
        "keys": def.identity_keys,
        "preview_point_key": preview,
    });
    let json = serde_json::json!({
        "kind": POINT_KIND_VARIABLE,
        "parameters": Value::Object(params),
        "identity": identity,
    })
    .to_string();
    Some((
        def,
        SemanticBinding {
            json,
            preview_point_key: preview,
        },
    ))
}

/// 由「模板固定字段 + 已确认实例」反推语义 `target` / `arguments`。
///
/// **仅用于浏览/预填**：采集路径永远是「语义 → 原生」单向；
/// 未确认的字段**不出现**（缺 `area_no` 就得不到 `channel`，绝不用 1 顶替）。
fn prefill_from_native(
    def: &VariableDefinition,
    native: &Map<String, Value>,
) -> (TargetRef, Map<String, Value>) {
    let mut target = TargetRef::default();
    let mut arguments = Map::new();
    let module = native.get("module").and_then(Value::as_u64);
    for (source, field_value) in [
        (&def.addressing.area_no, native.get("area_no")),
        (&def.addressing.line, native.get("line")),
    ] {
        let Some(value) = field_value.and_then(Value::as_u64) else {
            continue;
        };
        match source {
            NativeFieldSource::Target { kind } => match kind {
                TargetKind::Channel => target.channel = u32::try_from(value).ok(),
                TargetKind::ModeGroup => target.mode_group = u32::try_from(value).ok(),
                TargetKind::Axis => target.axis = u32::try_from(value).ok(),
                TargetKind::Spindle => target.spindle = u32::try_from(value).ok(),
                TargetKind::DriveObject => {
                    // `entry` 只能由模板的 module 唯一确定（26/130 不可换算）；
                    // 拿不到就**不构造** drive_object，让使用者显式确认。
                    let entry = match module {
                        Some(26) => Some(DriveEntry::NcVariable),
                        Some(130) => Some(DriveEntry::DriveObject),
                        _ => None,
                    };
                    if let (Some(number), Some(entry)) = (u32::try_from(value).ok(), entry) {
                        target.drive_object = Some(DriveObjectRef { number, entry });
                    }
                }
            },
            NativeFieldSource::Argument { key } => {
                arguments.insert(key.clone(), Value::from(value));
            }
            NativeFieldSource::ArgumentOffset { key, offset } => {
                let base = value as i64 - i64::from(*offset);
                if base >= 0 {
                    arguments.insert(key.clone(), Value::from(base));
                }
            }
            // 固定字段与入口身份不来自用户输入，不进入 target/arguments。
            NativeFieldSource::Fixed { .. } | NativeFieldSource::DriveEntryModule => {}
        }
    }
    (target, arguments)
}

/// 实例坐标载荷：**单一实例坐标**（通道 / 主轴）的浏览节点载荷。
///
/// 与 [`semantic_payload`] 的区别（为什么不能复用）：那里必须绑定**一条**具体
/// 定义的 `variable_id` + `definition_revision`，而实例节点服务的是**多条**
/// 定义（42 条定义里 16 条的 `targets` 含 `channel`、3 条含 `spindle`）。
/// 绑死其中任何一条，前端 `deriveObjectFill` 的变量一致性判定就会把其余变量的
/// 合法坐标判成 `OBJECT_VARIABLE_MISMATCH`——用户明明选的是正确通道，却被
/// 告知"这是另一个变量的对象"。
///
/// 因此载荷只给「坐标 + 身份键」，而这两样**取自真实定义**（不手写拼接）：
/// - **适用定义** = 所有"声明该 Target 变体、且把它列为**必填身份键**"的定义
///   （`channel` 16 条、`spindle` 3 条）；一条都没有 ⇒ 该坐标不是任何定义的身份
///   组成部分，返回 `None`（由调用方转成诊断，绝不猜）；
/// - **身份键** = 本节点**提供**的那一个坐标（`[{path: kind, required: true}]`）。
///   为什么不是某条定义的完整键集：主轴定义同时需要 `channel`，节点只负责给出
///   `spindle`；把 `channel` 也写进载荷等于替用户编一个通道号。缺的键由前端
///   `deriveObjectFill` 如实列为"仍需手动填写"，不是靠节点补默认值；
/// - **身份模板** = 适用定义中身份键最少（并列取 `variable_id` 最小）的那条的
///   `identity_template` 原文，仅用于说明"这条坐标怎么用"；
/// - **`applies_to_variables`** = 该载荷对哪些定义有效，让"载荷与需要该坐标的
///   定义匹配"成为**可机械核对**的事实而非口头约定。
///
/// 返回 `None` 的两种情况都必须由调用方转成诊断（不可选 + 稳定码）：
/// 内嵌目录不可用、该变体没有任何"把它当必填身份键"的定义。
pub fn instance_coordinate_payload(family: &str, kind: TargetKind, number: u32) -> Option<String> {
    let catalog = embedded_catalog()?;
    let path = kind.as_str();
    // 目录 `by_id` 是 BTreeMap，`for_family` 的输出顺序稳定（variable_id 升序）。
    let defs: Vec<&VariableDefinition> = catalog
        .for_family(family)
        .into_iter()
        .filter(|d| d.targets.contains(&kind))
        .filter(|d| d.identity_keys.iter().any(|k| k.path == path && k.required))
        .collect();
    if defs.is_empty() {
        return None;
    }

    // 代表定义：身份键最少（并列取 variable_id 最小，`defs` 已按 id 升序）。
    let template_def = defs.iter().min_by_key(|d| d.identity_keys.len()).copied()?;

    let mut target = Map::new();
    target.insert(path.to_string(), Value::from(number));
    let applies: Vec<&str> = defs.iter().map(|d| d.variable_id.as_str()).collect();
    Some(
        serde_json::json!({
            "kind": POINT_KIND_VARIABLE,
            "parameters": {
                "target": Value::Object(target),
                "arguments": Value::Object(Map::new()),
            },
            "identity": {
                "keys": [{ "path": path, "required": true }],
                "template": template_def.identity_template,
                // 单坐标不构成完整身份（多键定义还缺别的键），因此**不**给预览 Key：
                // 猜一个"看起来完整"的 Key 正是本轮要消除的东西。
                "preview_point_key": Value::Null,
                "applies_to_variables": applies,
            },
        })
        .to_string(),
    )
}

/// 目录节点（模板入口）。
///
/// IPC 1.8 起显式声明 `NOT_SELECTABLE`：分支节点永远不该被选用，由驱动把这件事
/// 说清楚，前端不必再靠 `has_children` / 空信封之类的形态去猜。
pub fn folder_node(id: &str, label: &str) -> BrowseNode {
    let selection = wire_selection::not_selectable();
    BrowseNode {
        id: id.into(),
        label: label.into(),
        kind: "folder".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: true,
        binding_json: "{}".into(),
        selection_mode: selection.mode as i32,
        selection_payload_json: selection.payload_json,
        diagnostics: selection.diagnostics,
    }
}

/// 模板目录诊断节点（模板不可用时明确提示，而不是伪装成空目录）。
pub fn diagnostic(message: String) -> BrowseNode {
    let selection = wire_selection::not_selectable();
    BrowseNode {
        id: "nck://presets/note".into(),
        label: message,
        kind: "diagnostic".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: "{}".into(),
        selection_mode: selection.mode as i32,
        selection_payload_json: selection.payload_json,
        diagnostics: selection.diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_templates_load_and_are_candidates() {
        let reg = PresetRegistry::embedded();
        assert!(
            reg.error().is_none(),
            "内嵌模板必须全部合法：{:?}",
            reg.error()
        );
        let t = reg.for_family("828d");
        assert!(!t.is_empty(), "828d 应有模板");
        assert!(t.iter().all(|x| x.validation_status == "candidate"));
        // 每个模板都不得预填 area_no/line。
        assert!(t.iter().all(|x| {
            !x.parameters.contains_key("area_no") && !x.parameters.contains_key("line")
        }));
        // 其他系列暂无模板（不得套用 828D）。
        assert!(reg.for_family("840d-sl").is_empty());
    }

    #[test]
    fn every_embedded_template_fills_into_a_valid_native_address() {
        // 模板 + 使用者确认的 area_no/line 必须能组成合法原生地址。
        let reg = PresetRegistry::embedded();
        for t in reg.for_family("828d") {
            let mut p = t.parameters.clone();
            p.insert("area_no".into(), Value::from(1));
            p.insert("line".into(), Value::from(1));
            assert!(
                NativeNckAddress::from_parameters(&p).is_ok(),
                "模板 {} 无法组成合法地址",
                t.id
            );
        }
    }

    #[test]
    fn drive_object_entries_split_into_two_modules() {
        let reg = PresetRegistry::embedded();
        let both: Vec<_> = reg
            .for_family("828d")
            .into_iter()
            .filter(|t| t.parameters.get("column").and_then(|v| v.as_u64()) == Some(199))
            .collect();
        let modules: Vec<u64> = both
            .iter()
            .map(|t| t.parameters.get("module").unwrap().as_u64().unwrap())
            .collect();
        assert!(modules.contains(&26), "应有 NC 变量入口模板");
        assert!(modules.contains(&130), "应有驱动对象入口模板");
    }

    #[test]
    fn invalid_or_empty_preset_data_degrades_to_empty_registry() {
        // 非法 JSON。
        let reg = PresetRegistry::load("{not json");
        assert!(reg.is_empty());
        assert!(reg.error().is_some());
        // 空集合合法。
        let reg = PresetRegistry::load(r#"{"templates":[]}"#);
        assert!(reg.is_empty());
        assert!(reg.error().is_none());
        // 预填 area_no 的模板被拒。
        let reg = PresetRegistry::load(
            r#"{"templates":[{"id":"x","family":"828d","label":"L",
                "validation_status":"candidate",
                "parameters":{"area":"C","area_no":1,"module":21,"column":1,
                              "count":1,"unit_mode":"current","decode":"F64_LE",
                              "transport_size":"0x09"},
                "required_user_inputs":["area_no","line"]}]}"#,
        );
        assert!(reg.is_empty());
        assert!(reg.error().unwrap().contains("不得预填"));
        // 非法 decode 的模板被拒（同一套原生规则）。
        let reg = PresetRegistry::load(
            r#"{"templates":[{"id":"x","family":"828d","label":"L",
                "validation_status":"candidate",
                "parameters":{"area":"C","module":21,"column":1,"count":1,
                              "unit_mode":"current","decode":"REAL",
                              "transport_size":"0x09"},
                "required_user_inputs":["area_no","line"]}]}"#,
        );
        assert!(reg.is_empty());
        // 非 candidate 状态被拒（不得暗示已验证）。
        let reg = PresetRegistry::load(
            r#"{"templates":[{"id":"x","family":"828d","label":"L",
                "validation_status":"supported",
                "parameters":{"area":"C","module":21,"column":1,"count":1,
                              "unit_mode":"current","decode":"F64_LE",
                              "transport_size":"0x09"},
                "required_user_inputs":["area_no","line"]}]}"#,
        );
        assert!(reg.is_empty());
    }

    #[test]
    fn leaf_binding_is_a_semantic_envelope_without_native_fields() {
        let reg = PresetRegistry::embedded();
        let t = reg.for_family("828d")[0];
        let node = leaf_node(t);
        assert!(node.id.starts_with("nck://presets/"));
        assert!(node.label.contains("candidate"));
        assert!(node.label.contains("待确认：area_no/line"));
        assert_eq!(node.kind, "nck_variable", "叶必须是语义载荷");
        let binding: Value = serde_json::from_str(&node.binding_json).unwrap();
        assert_eq!(binding["kind"], "nck_variable");
        let params = binding["parameters"].as_object().unwrap();
        assert!(params.contains_key("variable_id"));
        assert_eq!(params["definition_revision"], 1);
        assert!(params.contains_key("target"));
        assert!(params.contains_key("arguments"));
        // 原生字段**不得**出现在语义信封里（语义模式下用户不面对协议字段）。
        for key in params.keys() {
            assert!(
                ["variable_id", "definition_revision", "target", "arguments"]
                    .contains(&key.as_str()),
                "语义信封不得含原生字段 `{key}`：{}",
                node.binding_json
            );
        }
        for key in binding["parameters"]["target"].as_object().unwrap().keys() {
            assert!(
                ["channel", "mode_group", "axis", "spindle", "drive_object"]
                    .contains(&key.as_str()),
                "target 只允许语义变体，实际 `{key}`"
            );
        }
        assert!(binding["resource_id"].is_null(), "不得再有旧资源信封");
        // identity 块旁挂在**浏览载荷**里（模板/键集合/预览 Key）。
        assert!(
            binding["identity"]["template"]
                .as_str()
                .unwrap()
                .starts_with("axis.")
        );
        assert!(!binding["identity"]["keys"].as_array().unwrap().is_empty());
        // 未确认实例 ⇒ 预览 Key 为 null（不猜测）。
        assert!(binding["identity"]["preview_point_key"].is_null());
    }

    #[test]
    fn filled_leaf_prefill_produces_a_complete_preview_key() {
        // 身份浏览会填入已核对的 area_no；驱动对象模板应能给出完整预览 Key。
        let reg = PresetRegistry::embedded();
        let t = reg
            .for_family("828d")
            .into_iter()
            .find(|t| t.id == "828d.V.M.r0021.nc_variable")
            .expect("驱动对象模板");
        let node = leaf_node_with(t, &[("area_no", Value::from(3))]);
        let binding: Value = serde_json::from_str(&node.binding_json).unwrap();
        assert_eq!(binding["parameters"]["target"]["drive_object"]["number"], 3);
        assert_eq!(
            binding["parameters"]["target"]["drive_object"]["entry"],
            "nc_variable"
        );
        assert_eq!(
            binding["identity"]["preview_point_key"],
            "drive.parameter.r0021@drive_object=o:{entry=s:nc_variable,number=i:3}"
        );
    }

    // ---------------------------------------------------------------------
    // IPC 1.8 Browse 选择契约（准入判定在驱动侧）
    // ---------------------------------------------------------------------

    use crate::definition::DefinitionScope;
    use mesa_driver_protocol::pb::BrowseSelectionMode;

    /// 用内嵌目录里的**真实定义**克隆一份并改字段，构造内存目录。
    ///
    /// 为什么克隆真定义而不是手写 JSON：手写会引入第二份"定义长什么样"的真值，
    /// 一旦 schema 演进就会变成假绿。克隆后仅改被测字段，其余（身份模板、
    /// golden_ref、addressing）都与生产数据逐字一致。
    fn catalog_with(
        mutate: impl FnOnce(&mut VariableDefinition),
    ) -> (DefinitionCatalog, PresetTemplate) {
        let embedded = DefinitionCatalog::embedded().expect("内嵌目录必须可装载");
        let reg = PresetRegistry::embedded();
        // 基线必须选一条**可采集**（scope=collect）的模板：模板顺序首项对应的是
        // 身份探针用的 identity 定义，拿它当"可选叶"基线会掩盖准入判定。
        let t = reg
            .for_family("828d")
            .into_iter()
            .find(|t| {
                golden_ref_of_template(&t.id)
                    .and_then(|w| embedded.find_by_golden_ref(&w).first().copied())
                    .is_some_and(|d| d.scope == DefinitionScope::Collect)
            })
            .expect("828D 应至少有一条可采集模板")
            .clone();
        let want = golden_ref_of_template(&t.id).expect("模板 id 必须可映射");
        let mut def = embedded
            .find_by_golden_ref(&want)
            .first()
            .copied()
            .expect("模板必须能命中定义")
            .clone();
        mutate(&mut def);
        let catalog = DefinitionCatalog::from_definitions(vec![def]).expect("内存目录必须合法");
        (catalog, t)
    }

    /// `scope=collect` 且 family 一致的叶：驱动声明 **SEMANTIC**，载荷与
    /// `binding_json` 逐字一致，诊断只放 INFO 级预览提示。
    #[test]
    fn selectable_leaf_declares_semantic_mode_with_preview_diagnostic() {
        let (catalog, t) = catalog_with(|_| {});
        // 填齐使用者本来就要确认的实例字段 ⇒ target/arguments 完整 ⇒ 有预览 Key。
        let node = leaf_node_in(
            Some(&catalog),
            &t,
            &[("area_no", Value::from(1)), ("line", Value::from(1))],
        );
        assert_eq!(
            node.selection_mode,
            BrowseSelectionMode::Semantic as i32,
            "可采集的语义叶必须声明 SEMANTIC（不是靠前端推断）"
        );
        assert_eq!(
            node.selection_payload_json, node.binding_json,
            "选择载荷与 binding_json 同内容（旧客户端继续读 binding_json）"
        );
        assert_eq!(node.diagnostics.len(), 1);
        assert_eq!(node.diagnostics[0].code, admission::CODE_IDENTITY_PREVIEW);
        assert_eq!(
            node.diagnostics[0].severity,
            mesa_driver_protocol::pb::BrowseDiagnosticSeverity::Info as i32,
            "预览 Key 只是展示性提示，不得升级成阻断"
        );
        assert!(node.diagnostics[0].message.contains("Browse 预览："));
    }

    /// 身份未确定（target/arguments 不完整）⇒ 仍可选，但诊断必须是 WARNING。
    #[test]
    fn unresolved_identity_keeps_leaf_selectable_with_warning() {
        let (catalog, t) = catalog_with(|_| {});
        // 不填 area_no/line ⇒ 无法构造 target ⇒ 预览 Key 为 None。
        let node = leaf_node_in(Some(&catalog), &t, &[]);
        assert_eq!(node.selection_mode, BrowseSelectionMode::Semantic as i32);
        assert_eq!(
            node.diagnostics[0].code,
            admission::CODE_IDENTITY_UNRESOLVED
        );
        assert_eq!(
            node.diagnostics[0].severity,
            mesa_driver_protocol::pb::BrowseDiagnosticSeverity::Warning as i32
        );
        assert!(node.diagnostics[0].message.contains("身份未确定"));
    }

    /// `scope=identity` ⇒ 驱动直接声明不可选 + `DEFINITION_SCOPE_NOT_COLLECTABLE`，
    /// 与 Configure 拒绝码**同源**（前端不再需要自己解释 scope 白名单）。
    #[test]
    fn identity_scope_leaf_is_declared_not_selectable_by_driver() {
        let (catalog, t) = catalog_with(|def| def.scope = DefinitionScope::Identity);
        let node = leaf_node_in(Some(&catalog), &t, &[]);
        assert_eq!(
            node.selection_mode,
            BrowseSelectionMode::NotSelectable as i32
        );
        assert_eq!(node.diagnostics.len(), 1);
        assert_eq!(node.diagnostics[0].code, "DEFINITION_SCOPE_NOT_COLLECTABLE");
        assert_eq!(
            node.diagnostics[0].severity,
            mesa_driver_protocol::pb::BrowseDiagnosticSeverity::Error as i32
        );
        assert!(node.diagnostics[0].message.contains("scope=`identity`"));
        // 载荷仍按 1.7 形态给出（旧客户端不受影响），但它不构成可选许可。
        assert_eq!(node.kind, POINT_KIND_VARIABLE);
        assert!(!node.selection_payload_json.is_empty());
    }

    /// 定义 family 与本模板系列不一致 ⇒ 不可选 + `UNSUPPORTED_FAMILY`
    /// （禁止跨系列套用定义）。
    #[test]
    fn family_mismatch_is_declared_not_selectable_by_driver() {
        let (catalog, t) = catalog_with(|def| def.family = "840d-sl".into());
        let node = leaf_node_in(Some(&catalog), &t, &[]);
        assert_eq!(
            node.selection_mode,
            BrowseSelectionMode::NotSelectable as i32
        );
        assert_eq!(node.diagnostics[0].code, "UNSUPPORTED_FAMILY");
    }

    /// 目录不可用（`None`）⇒ 退回旧 `variable` 原生信封，但仍声明**不可选**
    /// 且给出 `SEMANTIC_PAYLOAD_UNAVAILABLE`：绝不把"找不到定义"伪装成可选。
    #[test]
    fn missing_catalog_is_declared_not_selectable_with_code() {
        let reg = PresetRegistry::embedded();
        let t = reg.for_family("828d")[0].clone();
        let node = leaf_node_in(None, &t, &[]);
        assert_eq!(
            node.selection_mode,
            BrowseSelectionMode::NotSelectable as i32
        );
        assert_eq!(
            node.diagnostics[0].code,
            admission::CODE_SEMANTIC_PAYLOAD_UNAVAILABLE
        );
        assert_eq!(node.kind, "variable", "降级形态必须显式标注（A22）");
        let binding: Value = serde_json::from_str(&node.binding_json).unwrap();
        assert_eq!(binding["resource_id"], "variable");
        assert!(
            node.label.contains("语义载荷不可用"),
            "标签也要保留降级说明，实际：{}",
            node.label
        );
        assert!(
            !node.selection_payload_json.is_empty(),
            "降级叶的载荷必须与 binding_json 同内容（旧客户端继续读 binding_json）"
        );
    }

    /// 分支 / 诊断节点：驱动声明 **NOT_SELECTABLE** 且**不给理由**
    /// （它们本来就不该被选用，红色阻断会误导用户）。
    #[test]
    fn folder_and_diagnostic_nodes_are_not_selectable_without_reason() {
        for node in [
            folder_node("nck://presets", "常用预设"),
            diagnostic("模板不可用".into()),
        ] {
            assert_eq!(
                node.selection_mode,
                BrowseSelectionMode::NotSelectable as i32,
                "{} 必须显式声明不可选",
                node.id
            );
            assert!(
                node.diagnostics.is_empty(),
                "分支/诊断节点不应产生红色阻断理由"
            );
            assert!(node.selection_payload_json.is_empty());
        }
    }
}
