//! I-2b：定义目录的**权威投影**与离线**身份预览**（Driver 进程级、零连接）。
//!
//! ## 为什么投影在 Driver 侧
//!
//! 定义目录与身份模板都是**驱动数据**（`identity_template` 参与摘要，见
//! `definition.rs:8-25`）。Core 的纪律是「零 Siemens 知识、只做形状校验与逐字
//! 透传」（P9），因此「哪些字段可以给前端看」必须由数据所有者（本模块）决定。
//! 投影白名单见 [`PROJECTED_KEYS`]：语义可见面，**不含**原生寻址/解码/传输
//! 属性——把它们交给前端等于邀请前端重实现 `native.rs` 的解析链。
//!
//! ## 只读性（I21 / G7）
//!
//! 本模块是纯计算：不引用设备访问模块、不建连接、不发 wire、不读机床实时值、
//! 不写文件与存储。因此它可以在**零 Connection 条目**下被进程级 `Driver` 调用
//! （Manager 的定义查询只做 spawn + 握手 + 单 RPC，随后即终止临时进程）。
//! 文件内**不允许**出现设备触点或异步原语——CI 的只读红线门会逐字扫描。
//!
//! ## 目录真实性（I44：禁止静默删六条）
//!
//! `scope="identity"` 的 6 条定义**原样返回**：目录是「目录」，不是「可采集点位
//! 清单」；准入判定归前端（PR-C 的 A40/A13）。少 6 条 + 浏览树仍 62 叶会形成
//! 两份真值，是被明令禁止的组合（DC §9.3-C′）。

use std::collections::BTreeSet;

use mesa_core_types::ErrorKind;
use serde_json::{Map, Value};

use crate::compiler::{AddressCompiler, CompileError};
use crate::definition::{
    CatalogError, DEFINITION_FORMAT_VERSION, DefinitionCatalog, DefinitionScope,
    MIN_DEFINITION_REVISION, VariableDefinition,
};
use crate::{DECLARED_FAMILIES, DRIVER_ID};

/// 缺省页大小（DC §6.4-①-③；与 Core 的 `driver_query.rs` 同值）。
pub const DEFAULT_LIMIT: u32 = 100;
/// 单页上限：超出即**夹取**（不是错误，与 Browse 的 `min(1000)` 同口径）。
pub const MAX_LIMIT: u32 = 500;

/// 目录查询参数允许的键（DC §4.1 的五个参数）。
const QUERY_KEYS: [&str; 5] = ["family", "variable_id", "revision", "limit", "cursor"];

/// 预览请求信封允许的键（DC §5.1：只有这三个；`point_key` 正是要被算出来的东西）。
const PREVIEW_KEYS: [&str; 3] = ["family", "kind", "parameters"];

/// 目录投影的**字段白名单**（与 DC §4.2/§4.3 逐字一致）。
///
/// 为什么用白名单而不是「序列化后删 6 个原生字段」：白名单对**未来新增**的
/// 定义字段默认拒绝，避免新字段顺着 `serde` 泄漏到对外契约；删除法只对今天
/// 已知的 6 个字段有效。
///
/// 为什么不含 `unit`：它属原生字段白名单（`compiler.rs:45-58`）的一员，是原生
/// 侧展示注解而非语义可见面；当前 42 条定义均未声明它（`r1.json` 实测 0 条），
/// 因此排除它**不改变**今天的投影结果。
const PROJECTED_KEYS: [&str; 14] = [
    "variable_id",
    "revision",
    "family",
    "scope",
    "semantics_status",
    "label",
    "source",
    "targets",
    "target_required",
    "arguments",
    "identity_template",
    "identity_keys",
    "golden_ref",
    "digest",
];

/// 定义 API 的错误：**码 + `ErrorKind` + 消息**三件套。
///
/// 为什么自带 kind：Core 只按 `ErrorKind` 映射 HTTP（DC §3.3/§6.2），
/// 因此每一条错误在产生处就必须有确定归属（§6.2.1 的确定性表），
/// 不允许「同一个码在不同路径下有不同 kind」。
#[derive(Debug, Clone)]
pub struct DefinitionApiError {
    kind: ErrorKind,
    code: &'static str,
    message: String,
}

impl DefinitionApiError {
    fn new(kind: ErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }

    /// 调用方查询形状非法（E12）。
    fn invalid_query(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Configuration, "INVALID_QUERY", message)
    }

    /// 预览信封形状非法（E13）。
    ///
    /// NOTE：该码在 DC §6.2.1 的归属层是 Core（形状层）。Driver 侧再判定一次
    /// 是**防御纵深**：直接调 RPC 的调用方不能绕过形状门。码与 kind 与 Core
    /// 完全一致，**没有引入新码**，因此不破坏「码 → kind 唯一」。
    fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Configuration, "INVALID_REQUEST", message)
    }

    /// 系列不在驱动声明内（E8；`Unsupported` ⇒ 501，见 §6.2.1）。
    fn unsupported_family(family: &str) -> Self {
        Self::new(
            ErrorKind::Unsupported,
            "UNSUPPORTED_FAMILY",
            format!(
                "系列 `{family}` 未在驱动声明内（已声明：{DECLARED_FAMILIES:?}）；\
                 已声明但无定义的系列返回 200 + 空目录，两者必须可区分"
            ),
        )
    }

    /// 构建期/发布数据问题（E7/E28）——**绝不**「跳过坏定义继续响应」。
    fn internal(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, code, message)
    }

    /// 编译器的错误**逐字保留**码与 kind（E19/E20/E16/E18/E22–E27 的唯一出处）。
    fn from_compile(e: CompileError) -> Self {
        Self::new(e.kind(), e.code(), e.to_string())
    }

    /// 目录自检失败：全部变体共用 `DEFINITION_DIGEST_MISMATCH`（`CatalogError::code`）。
    fn from_catalog(e: CatalogError) -> Self {
        Self::new(ErrorKind::Internal, e.code(), e.to_string())
    }

    /// 对外稳定码。
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// 交给 `ErrorKind → HTTP` 映射的确定性归属。
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// 诊断消息（含变量 / revision / 期望值等要素）。
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for DefinitionApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}: {}", self.kind.as_str(), self.code, self.message)
    }
}

impl std::error::Error for DefinitionApiError {}

/// 目录端点：查询参数 JSON → 目录响应 JSON（DC §4.2）。
///
/// 入参是 Core 按 §4.1 组装并**逐字透传**的查询对象；本函数是查询语义的
/// 唯一权威（Core 只判正整数、互斥与夹取，见 `core-api/src/driver_query.rs`）。
pub fn catalog_json(request_json: &str) -> Result<String, DefinitionApiError> {
    catalog_json_with_loader(request_json, DefinitionCatalog::embedded)
}

/// 可注入目录装载器的目录端点（**仅为测试 fail-closed 路径**而存在）。
///
/// 为什么需要这个缝：E7/E28 要求「注入损坏目录 ⇒ 503 `DEFINITION_DIGEST_MISMATCH`，
/// 且绝不返回半截结果」。内嵌目录是编译期常量，测试无法在不改冻结数据文件的
/// 前提下破坏它；因此把「装载」作为参数注入，生产路径仍只有
/// `DefinitionCatalog::embedded()` 一条。装载失败时本函数**只**返回错误，
/// 不产生任何目录字节。
pub fn catalog_json_with_loader<F>(
    request_json: &str,
    load: F,
) -> Result<String, DefinitionApiError>
where
    F: FnOnce() -> Result<DefinitionCatalog, CatalogError>,
{
    let query = parse_catalog_query(request_json)?;
    let catalog = load().map_err(DefinitionApiError::from_catalog)?;

    // 系列过滤：先判「是否已声明」，再判「是否有定义」——两者的可区分性
    // 正是 E8 与 E9 的分界（400/501 vs 200 + 空数组 + note）。
    if let Some(f) = &query.family
        && !DECLARED_FAMILIES.contains(&f.as_str())
    {
        return Err(DefinitionApiError::unsupported_family(f));
    }

    let (matched, note) = select_definitions(&catalog, &query)?;

    // 游标：语义 = 上一页最后一条 `variable_id`。它必须**存在于当前结果集**，
    // 否则调用方拿到的是一个不存在的锚点，翻页语义不成立（E12）。
    if let Some(cursor) = &query.cursor
        && !matched.iter().any(|d| &d.variable_id == cursor)
    {
        return Err(DefinitionApiError::invalid_query(format!(
            "游标 `{cursor}` 不在当前结果集内（游标必须逐字回传上一页的 next_cursor）"
        )));
    }
    // 下一页严格取 `variable_id` **大于**游标的条目：按 (variable_id, revision)
    // 升序遍历时，即使同一 variable_id 将来有多个 revision，也会整组翻过，
    // 不会在页边界重复或漏掉。
    let start = match &query.cursor {
        None => 0,
        Some(cursor) => matched.partition_point(|d| &d.variable_id <= cursor),
    };
    let end = matched
        .len()
        .min(start.saturating_add(query.limit as usize));
    let page = &matched[start..end];
    let next_cursor = if end < matched.len() {
        page.last().map(|d| d.variable_id.clone())
    } else {
        None
    };

    let definitions = page
        .iter()
        .map(|def| project(def))
        .collect::<Result<Vec<_>, _>>()?;

    // 顶层 `family`：请求给了就回显；没给时若只有**一个**已发布系列则报它
    // （今天只有 828d），多系列时无法用单值表达 ⇒ null。
    // NOTE：多系列 + 省略 family 是当前不存在、且契约未定义的组合；一旦新增
    // 第二个已发布系列，需评审决定顶层 `family` 的形态（数组 / null / 拆分响应）。
    let published = published_families(&catalog);
    let family_out = query.family.clone().or_else(|| {
        if published.len() == 1 {
            published.iter().next().cloned()
        } else {
            None
        }
    });

    // 文件级 revision：装载期强制「同一文件的全部定义 revision 相同」
    // （`definition.rs:422-428`），故取任一条即文件级值；空目录回落到最小合法值。
    let file_revision = catalog
        .iter()
        .next()
        .map(|d| d.revision)
        .unwrap_or(MIN_DEFINITION_REVISION);

    let mut out = Map::new();
    out.insert(
        "format_version".into(),
        Value::from(DEFINITION_FORMAT_VERSION),
    );
    out.insert(
        "family".into(),
        family_out.map(Value::from).unwrap_or(Value::Null),
    );
    out.insert("revision".into(), Value::from(file_revision));
    out.insert("driver_id".into(), Value::from(DRIVER_ID));
    out.insert(
        "driver_version".into(),
        Value::from(env!("CARGO_PKG_VERSION")),
    );
    out.insert("note".into(), note.map(Value::from).unwrap_or(Value::Null));
    out.insert("total".into(), Value::from(matched.len() as u64));
    out.insert("limit".into(), Value::from(query.limit));
    out.insert(
        "next_cursor".into(),
        next_cursor.map(Value::from).unwrap_or(Value::Null),
    );
    out.insert("definitions".into(), Value::Array(definitions));

    serde_json::to_string(&Value::Object(out))
        .map_err(|e| DefinitionApiError::internal("DEFINITION_RESPONSE_INVALID", e.to_string()))
}

/// 已解析且已严格校验的目录查询（保持「缺省」与「显式给出」的可区分性）。
#[derive(Debug, Clone)]
struct CatalogQuery {
    family: Option<String>,
    variable_id: Option<String>,
    revision: Option<u32>,
    limit: u32,
    cursor: Option<String>,
}

/// 查询参数的**严格**解析（E12）。
///
/// 为什么手写而不是 `serde(deny_unknown_fields)`：需要把「缺省」「显式非法」
/// 两类情况映射成**同一个** `INVALID_QUERY` 且带精确原因；`serde` 的报错文本
/// 与键名不满足「参数名 + 期望形态」的消息要素要求。
fn parse_catalog_query(request_json: &str) -> Result<CatalogQuery, DefinitionApiError> {
    let value: Value = serde_json::from_str(request_json).map_err(|e| {
        DefinitionApiError::invalid_query(format!("查询参数不是合法 JSON 对象：{e}"))
    })?;
    let obj = value.as_object().ok_or_else(|| {
        DefinitionApiError::invalid_query(
            "查询参数必须是 JSON 对象（键为 family/variable_id/revision/limit/cursor）",
        )
    })?;
    for key in obj.keys() {
        if !QUERY_KEYS.contains(&key.as_str()) {
            return Err(DefinitionApiError::invalid_query(format!(
                "未知查询参数 `{key}`（仅允许 {QUERY_KEYS:?}）"
            )));
        }
    }

    let family = optional_non_empty_string(obj, "family")?;
    let variable_id = optional_non_empty_string(obj, "variable_id")?;
    let revision = optional_positive_u32(obj, "revision")?;
    let limit = optional_positive_u32(obj, "limit")?
        .map(|v| v.min(MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT);
    let cursor = optional_non_empty_string(obj, "cursor")?;

    // revision 与 variable_id 必须成对：只给 revision 等于「全目录过滤 revision」，
    // 语义不明确；只给 variable_id 则必须回落「最新 revision」，而冻结目录的
    // `lookup` 明确拒绝任何回落（`definition.rs:488-505`）⇒ 一律 INVALID_QUERY。
    match (&variable_id, revision) {
        (None, Some(_)) => {
            return Err(DefinitionApiError::invalid_query(
                "`revision` 必须与 `variable_id` 同时出现（拒绝「全目录过滤 revision」这种语义不明确的查询）",
            ));
        }
        (Some(_), None) => {
            return Err(DefinitionApiError::invalid_query(
                "`variable_id` 必须与 `revision` 同时出现（不做「缺省回落最新 revision」——已发布定义不可变，回落会掩盖 revision 缺失）",
            ));
        }
        _ => {}
    }
    // 游标与精确变量互斥：游标是「页锚点」，再叠加 variable_id 会让「从哪开始」
    // 与「只要哪一条」两个语义互相冲突。
    if cursor.is_some() && variable_id.is_some() {
        return Err(DefinitionApiError::invalid_query(
            "`cursor` 与 `variable_id` 互斥（游标用于翻页，不能与精确过滤叠加）",
        ));
    }

    Ok(CatalogQuery {
        family,
        variable_id,
        revision,
        limit,
        cursor,
    })
}

/// 取非空字符串参数。
///
/// **显式 `null` 与缺省必须可区分**：缺省 = 不过滤；显式 `null` 是一个语义
/// 未定义的形状 ⇒ `INVALID_QUERY`（不做「null 当缺省」的静默合并）。
fn optional_non_empty_string(
    obj: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, DefinitionApiError> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(other) => Err(DefinitionApiError::invalid_query(format!(
            "`{key}` 必须是非空字符串，实际 {other}"
        ))),
    }
}

/// 取正整数参数：缺省 = 用默认值；`null`、`0`、负数、小数、字符串一律 `INVALID_QUERY`。
fn optional_positive_u32(
    obj: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<u32>, DefinitionApiError> {
    let raw = match obj.get(key) {
        None => return Ok(None),
        Some(v) => v,
    };
    let n = raw.as_u64().filter(|n| *n > 0).ok_or_else(|| {
        DefinitionApiError::invalid_query(format!("`{key}` 必须是正整数，实际 {raw}"))
    })?;
    u32::try_from(n)
        .map(Some)
        .map_err(|_| DefinitionApiError::invalid_query(format!("`{key}` = {n} 超过 u32 上限")))
}

/// 选择结果集（过滤后、分页前），并给出 `note`。
///
/// `note` 只用于 **E9**（已声明但无定义的系列 ⇒ 200 + `[]` + note），
/// 它是「无定义」与「驱动故障 / 未知系列」可区分的唯一标记。
fn select_definitions<'a>(
    catalog: &'a DefinitionCatalog,
    query: &CatalogQuery,
) -> Result<(Vec<&'a VariableDefinition>, Option<String>), DefinitionApiError> {
    if let (Some(variable_id), Some(revision)) = (&query.variable_id, query.revision) {
        // D5/D6：命中即返回，miss 绝不回落（错误码由冻结的 lookup 决定）。
        let def = catalog
            .lookup(variable_id, revision)
            .map_err(DefinitionApiError::from_compile)?;
        // family 过滤与精确变量是**合取**：该变量不在请求系列内 ⇒ 对该系列而言
        // 它不存在（404 UNKNOWN_VARIABLE），而不是「返回一个别的系列的定义」。
        if let Some(f) = &query.family
            && &def.family != f
        {
            return Err(DefinitionApiError::new(
                ErrorKind::Address,
                "UNKNOWN_VARIABLE",
                format!(
                    "变量 `{variable_id}` 不在系列 `{f}` 的已发布定义中（其定义属于系列 `{}`）",
                    def.family
                ),
            ));
        }
        return Ok((vec![def], None));
    }

    let defs: Vec<&VariableDefinition> = catalog
        .iter()
        .filter(|d| query.family.as_ref().is_none_or(|f| &d.family == f))
        .collect();
    let note = match (&query.family, defs.is_empty()) {
        (Some(f), true) => Some(format!(
            "系列 `{f}` 已在驱动中声明，但当前没有任何已发布定义（这是空目录，不是驱动故障）"
        )),
        _ => None,
    };
    Ok((defs, note))
}

/// 已发布系列集合（由定义数据本身得出，不另立真值）。
fn published_families(catalog: &DefinitionCatalog) -> BTreeSet<String> {
    catalog.iter().map(|d| d.family.clone()).collect()
}

/// 单条定义的**语义可见面**投影（白名单 + 逐字取值，唯一例外是装载期计算的 `digest`）。
fn project(def: &VariableDefinition) -> Result<Value, DefinitionApiError> {
    let full = serde_json::to_value(def).map_err(|e| {
        DefinitionApiError::internal(
            "DEFINITION_RESPONSE_INVALID",
            format!("定义 `{}` 无法序列化：{e}", def.variable_id),
        )
    })?;
    let obj = full.as_object().ok_or_else(|| {
        DefinitionApiError::internal(
            "DEFINITION_RESPONSE_INVALID",
            format!("定义 `{}` 序列化后不是 JSON 对象", def.variable_id),
        )
    })?;
    let mut out = Map::new();
    for key in PROJECTED_KEYS {
        let value = obj.get(key).ok_or_else(|| {
            DefinitionApiError::internal(
                "DEFINITION_RESPONSE_INVALID",
                format!("定义 `{}` 缺少投影字段 `{key}`", def.variable_id),
            )
        })?;
        out.insert(key.to_string(), value.clone());
    }
    Ok(Value::Object(out))
}

/// 预览端点：请求 JSON → 身份结果 JSON（DC §5.2）。
///
/// 输入语义：`parameters` 与将来持久化的 `PointSelection.parameters` **逐字同构**，
/// 因此不存在「预览一套参数、提交另一套」的漂移面。
pub fn preview_json(request_json: &str) -> Result<String, DefinitionApiError> {
    preview_json_with_loader(request_json, DefinitionCatalog::embedded)
}

/// 可注入目录装载器的预览端点（同 [`catalog_json_with_loader`]，供 E28 的
/// fail-closed 断言使用；生产路径仍只有内嵌目录一条）。
pub fn preview_json_with_loader<F>(
    request_json: &str,
    load: F,
) -> Result<String, DefinitionApiError>
where
    F: FnOnce() -> Result<DefinitionCatalog, CatalogError>,
{
    let value: Value = serde_json::from_str(request_json)
        .map_err(|e| DefinitionApiError::invalid_request(format!("请求体不是合法 JSON：{e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| DefinitionApiError::invalid_request("请求体必须是 JSON 对象"))?;

    // 信封层：只校验**形状**，不解释 parameters 的任何键（D2–D10 归编译器）。
    for key in obj.keys() {
        if !PREVIEW_KEYS.contains(&key.as_str()) {
            return Err(DefinitionApiError::invalid_request(format!(
                "未知顶层字段 `{key}`（仅允许 {PREVIEW_KEYS:?}；`point_key` 正是要被算出来的东西，不得传入）"
            )));
        }
    }
    for key in PREVIEW_KEYS {
        if !obj.contains_key(key) {
            return Err(DefinitionApiError::invalid_request(format!(
                "缺少必需字段 `{key}`"
            )));
        }
    }
    // family 必须显式指定（与目录查询「可省略」不同：预览没有 Endpoint 连接
    // 配置可依据，猜一个系列等于凭空制造语义）。
    let family = obj["family"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            DefinitionApiError::invalid_request("`family` 必须是非空字符串（本端点无缺省值）")
        })?;
    let kind = obj["kind"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| DefinitionApiError::invalid_request("`kind` 必须是非空字符串"))?;
    let parameters = obj["parameters"]
        .as_object()
        .ok_or_else(|| DefinitionApiError::invalid_request("`parameters` 必须是 JSON 对象"))?;

    let catalog = load().map_err(DefinitionApiError::from_catalog)?;
    let compiler = AddressCompiler::new(&catalog, family);
    // D1–D10 + 身份重算（D11 不适用：预览还没有持久化 Key）。
    // 拒绝面：未知 kind（E16）、user 模式（E18）、定义/Target/arguments/
    // 原生解析（E19–E27）全部由**同一个**编译器产生，预览不另写判定。
    let compiled = compiler
        .preview_point_key(kind, parameters)
        .map_err(DefinitionApiError::from_compile)?;

    // 定义坐标与 scope：语义点位有 definition 引用；原生点位全为 null。
    let (scope, variable_id, revision, digest) = match &compiled.definition {
        None => (Value::Null, Value::Null, Value::Null, Value::Null),
        Some(d) => {
            // 同一目录对象内的二次查找，只取 scope（`DefinitionRef` 不含它）。
            // 该查找不可能 miss（编译器刚用同一 catalog 命中过同一 (id, rev)）；
            // 若真 miss，说明目录被并发替换——按构建期缺陷 fail-closed，绝不
            // 返回一个「看起来有效」的身份。
            let def = catalog.lookup(&d.variable_id, d.revision).map_err(|e| {
                DefinitionApiError::internal(
                    "DEFINITION_DIGEST_MISMATCH",
                    format!(
                        "预览命中的定义 `{}`(rev {}) 在返回前无法再次命中：{e}",
                        d.variable_id, d.revision
                    ),
                )
            })?;
            (
                Value::from(scope_name(def.scope)),
                Value::from(d.variable_id.clone()),
                Value::from(d.revision),
                Value::from(d.digest.clone()),
            )
        }
    };

    let mut out = Map::new();
    // 成功态的 `point_key` 恒为非空字符串：预览**没有**「身份待确认」的中间态
    // （DC §5.4 的 fail-closed 设计属性，I20）。
    out.insert("point_key".into(), Value::from(compiled.canonical_identity));
    out.insert(
        "identity_mode".into(),
        Value::from(compiled.identity_mode.as_str()),
    );
    out.insert("scope".into(), scope);
    out.insert("variable_id".into(), variable_id);
    out.insert("definition_revision".into(), revision);
    out.insert("definition_digest".into(), digest);

    serde_json::to_string(&Value::Object(out))
        .map_err(|e| DefinitionApiError::internal("DEFINITION_RESPONSE_INVALID", e.to_string()))
}

/// `DefinitionScope` → 投影字符串（与定义数据文件逐字一致）。
fn scope_name(scope: DefinitionScope) -> &'static str {
    match scope {
        DefinitionScope::Collect => "collect",
        DefinitionScope::Identity => "identity",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **E7/E28（G-d）**：内嵌目录自检失败 ⇒ 503 `DEFINITION_DIGEST_MISMATCH`
    /// （`ErrorKind::Internal`），且**绝不**「跳过坏定义继续响应」。
    ///
    /// 为什么用注入装载器而不是改数据文件：`r1.json`/`DIGESTS.lock` 是冻结面
    /// （一个字节都不能改），内嵌目录又是编译期常量 ⇒ 只能在装载边界注入。
    #[test]
    fn catalog_and_preview_are_fail_closed_on_self_check_failure() {
        let corrupted = || Err(CatalogError::Json("injected corruption".into()));

        let err = catalog_json_with_loader(
            &serde_json::json!({ "family": "828d" }).to_string(),
            corrupted,
        )
        .expect_err("装载失败必须整体失败");
        assert_eq!(err.code(), "DEFINITION_DIGEST_MISMATCH");
        assert_eq!(err.kind().as_str(), "InternalError");
        // 消息必须保留 CatalogError 的完整 Display（含具体原因），便于 CI 定位。
        assert!(err.message().contains("injected corruption"));

        let request = serde_json::json!({
            "family": "828d",
            "kind": "nck_variable",
            "parameters": {
                "variable_id": "channel.program.status",
                "definition_revision": 1,
                "target": { "channel": 1 },
                "arguments": {},
            },
        })
        .to_string();
        let err = preview_json_with_loader(&request, corrupted)
            .expect_err("装载失败时预览不得返回任何 Key");
        assert_eq!(err.code(), "DEFINITION_DIGEST_MISMATCH");
        assert_eq!(err.kind().as_str(), "InternalError");
    }

    /// E12：查询形状在**驱动侧**也必须严格（Core 只判正整数/互斥/夹取）。
    #[test]
    fn query_shape_is_strict() {
        for bad in [
            r#"{"unknown":1}"#,
            r#"{"family":""}"#,
            r#"{"family":null}"#,
            r#"{"limit":0}"#,
            r#"{"limit":1.5}"#,
            r#"{"cursor":"x","variable_id":"y","revision":1}"#,
            "[]",
            "not-json",
        ] {
            let err = parse_catalog_query(bad).expect_err("必须拒绝");
            assert_eq!(err.code(), "INVALID_QUERY", "query={bad}");
            assert_eq!(err.kind().as_str(), "ConfigurationError");
        }
        // 缺省与显式给出可区分；limit 夹取到上限（不是错误）。
        let q = parse_catalog_query(r#"{"limit":5000}"#).expect("合法");
        assert_eq!(q.limit, MAX_LIMIT);
        assert_eq!(q.family, None);
        let q = parse_catalog_query(r#"{"variable_id":"v","revision":3}"#).expect("合法");
        assert_eq!(q.revision, Some(3));
        assert_eq!(q.limit, DEFAULT_LIMIT);
    }
}
