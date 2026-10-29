//! I-2b 定义目录 / 权威身份预览的 Driver 侧验收（DC §8.3 的 I1–I22 / I32 / I33 /
//! I37 / I44 / I45，以及缺口 G-a/E16、G-d、G-e/E21、G-f/E17）。
//!
//! ## 为什么这些断言放在 Driver 单测而不是只放 REST
//!
//! 「Preview 与 Configure 共用同一条权威验证路径」是**进程内**性质：REST 层
//! 只能看到 HTTP 状态，看不到 `K_p == K_c` 的字符串相等，也看不到 D1–D10 的
//! 每条拒绝面。因此 I12/I32 的主体在这里；`management_api` 的 REST 侧覆盖
//! 路由/信封/错误映射（阶段 2 已完成）。
//!
//! ## 只读性
//!
//! 全部用例**不建连接、不连机床、不启动 mesad**：目录与预览是纯计算路径，
//! `configure` 只写进程内计划（不碰 wire）。因此本文件可在 CI 里裸跑。

use std::collections::{BTreeMap, BTreeSet};

use mesa_core_types::{
    AcquisitionTask, DriverBinding, NCK_POINTS_BINDING_KIND, PointDescriptor, TaskSchedule,
};
use mesa_driver_sdk::{Driver, DriverConnection, SdkDriverError};
use mesa_driver_sinumerik_nck::{
    AddressCompiler, ArgType, DEFINITION_FORMAT_VERSION, DefinitionCatalog, DefinitionScope,
    DriveEntry, NckConnConfig, NckConnection, POINT_KIND_NATIVE, POINT_KIND_VARIABLE,
    SinumerikNckDriver, TargetKind, VariableDefinition,
};
use serde_json::{Map, Value, json};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 进程级驱动：`SinumerikNckDriver` 是**无状态单元结构体**。
///
/// 这一点本身就是 I22/R4 的结构性证据：它不可能持有 `NckConnection`，
/// 因此两个管理 RPC 天然「在有零个 Connection 条目时也能应答」。
fn driver() -> SinumerikNckDriver {
    SinumerikNckDriver
}

fn catalog() -> DefinitionCatalog {
    DefinitionCatalog::embedded().expect("内嵌定义目录必须通过装载期自检")
}

fn all_definitions(catalog: &DefinitionCatalog) -> Vec<&VariableDefinition> {
    catalog.iter().collect()
}

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_repo_file(rel: &str) -> String {
    std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("读取 `{rel}` 失败：{e}"))
}

/// `r1.json` 的原始定义（**逐字投影**的比对基准；只读，绝不改写）。
fn raw_definitions() -> BTreeMap<String, Value> {
    let doc: Value = serde_json::from_str(&read_repo_file(
        "drivers/sinumerik-nck/definitions/828d/r1.json",
    ))
    .expect("r1.json 必须是合法 JSON");
    doc["definitions"]
        .as_array()
        .expect("r1.json 必须含 definitions 数组")
        .iter()
        .map(|d| {
            (
                d["variable_id"].as_str().expect("variable_id").to_string(),
                d.clone(),
            )
        })
        .collect()
}

/// `DIGESTS.lock` 的已发布摘要（**唯一权威摘要来源**）。
fn lock_digests() -> BTreeMap<String, String> {
    let doc: Value = serde_json::from_str(&read_repo_file(
        "drivers/sinumerik-nck/definitions/828d/DIGESTS.lock",
    ))
    .expect("DIGESTS.lock 必须是合法 JSON");
    doc["entries"]
        .as_array()
        .expect("DIGESTS.lock 必须含 entries 数组")
        .iter()
        .map(|e| {
            (
                e["variable_id"].as_str().expect("variable_id").to_string(),
                e["digest"].as_str().expect("digest").to_string(),
            )
        })
        .collect()
}

async fn catalog_json(query: Value) -> Value {
    let raw = driver()
        .definitions(&query.to_string())
        .await
        .unwrap_or_else(|e| panic!("目录查询应成功，实际 {e}：query={query}"));
    serde_json::from_str(&raw).expect("目录响应必须是合法 JSON")
}

async fn catalog_err(query: Value) -> SdkDriverError {
    match driver().definitions(&query.to_string()).await {
        Ok(raw) => panic!("期望目录查询失败，实际成功：query={query} body={raw}"),
        Err(e) => e,
    }
}

fn preview_request(kind: &str, parameters: Value) -> Value {
    json!({ "family": "828d", "kind": kind, "parameters": parameters })
}

async fn preview(request: Value) -> Value {
    let raw = driver()
        .preview_point_key(&request.to_string())
        .await
        .unwrap_or_else(|e| panic!("预览应成功，实际 {e}：request={request}"));
    serde_json::from_str(&raw).expect("预览响应必须是合法 JSON")
}

async fn preview_err(request: Value) -> SdkDriverError {
    match driver().preview_point_key(&request.to_string()).await {
        Ok(raw) => panic!("期望预览失败，实际成功：request={request} body={raw}"),
        Err(e) => e,
    }
}

/// 逐条构造**合法** Target：按定义声明的 `target_required` 给实例号。
///
/// 取值刻意选小（1 / 3）：`drive_object.number` 会进 `area_no`（协议上界 31），
/// 用 3 既覆盖双入口定义又不越界；`entry` 由调用方指定，用于 26/130 的
/// 「不可折叠」断言。
fn legal_target(def: &VariableDefinition, entry: DriveEntry) -> Value {
    let mut target = Map::new();
    for kind in &def.target_required {
        let value = match kind {
            TargetKind::DriveObject => json!({ "number": 3, "entry": entry.as_str() }),
            _ => json!(1),
        };
        target.insert(kind.as_str().to_string(), value);
    }
    Value::Object(target)
}

/// 逐条构造**合法** arguments：身份键必须显式给出（缺省**绝不**物化 default）。
fn legal_arguments(def: &VariableDefinition) -> Value {
    let mut arguments = Map::new();
    for spec in &def.arguments {
        let value = match spec.value_type {
            ArgType::Integer => {
                let min = spec.min.unwrap_or(0);
                let max = spec.max.unwrap_or(i64::MAX);
                json!(min.clamp(1, max))
            }
            ArgType::Enum => {
                json!(
                    spec.enum_options
                        .as_ref()
                        .and_then(|o| o.first())
                        .cloned()
                        .unwrap_or_default()
                )
            }
        };
        arguments.insert(spec.key.clone(), value);
    }
    Value::Object(arguments)
}

fn legal_parameters(def: &VariableDefinition, entry: DriveEntry) -> Value {
    json!({
        "variable_id": def.variable_id,
        "definition_revision": def.revision,
        "target": legal_target(def, entry),
        "arguments": legal_arguments(def),
    })
}

fn conn_828d() -> NckConnection {
    NckConnection::new(
        NckConnConfig::from_json(&json!({
            "host": "127.0.0.1",
            "family": "828d",
            "local_tsap": 256,
            "remote_tsap": 258,
        }))
        .expect("连接配置必须合法"),
    )
}

/// 走**真实 Configure**（`ConfigureTasks` 的驱动入口）——这是 I32 的核心：
/// 不是「再算一遍」，而是把预览 Key 逐字交给 `configure` 的 D11 比对。
async fn configure_one(
    conn: &NckConnection,
    kind: &str,
    parameters: Value,
    point_key: &str,
) -> Result<Vec<PointDescriptor>, SdkDriverError> {
    configure_points(
        conn,
        "t-i32",
        json!([{ "kind": kind, "point_key": point_key, "parameters": parameters }]),
    )
    .await
}

/// 以**任意点位集合**走真实 Configure（混合批次 / 原生回归用）。
async fn configure_points(
    conn: &NckConnection,
    task_id: &str,
    points: Value,
) -> Result<Vec<PointDescriptor>, SdkDriverError> {
    let task = AcquisitionTask {
        id: task_id.into(),
        schedule: TaskSchedule::Poll { interval_ms: 50 },
        binding: DriverBinding {
            kind: NCK_POINTS_BINDING_KIND.into(),
            config: json!({ "points": points }),
        },
    };
    conn.configure(1, vec![task]).await
}

/// 原生 12 字段的合法取值（与 DC §5.2 用例 D 逐字一致）。
fn native_derived_parameters() -> Value {
    json!({
        "area": "C", "area_no": 1, "module": 21, "column": 1, "line": 396,
        "count": 1, "unit_mode": "current", "decode": "F64_LE", "transport_size": "0x09",
    })
}

// ---------------------------------------------------------------------------
// I1 / I2 / I3 / I4 / I44：目录投影真实性
// ---------------------------------------------------------------------------

/// I1：42 条 + 每行 `digest` 与 `DIGESTS.lock` 逐字一致。
#[tokio::test]
async fn definitions_returns_42_rows_with_digests() {
    let v = catalog_json(json!({})).await;
    assert_eq!(
        v["format_version"], 1,
        "格式版本必须回显 {DEFINITION_FORMAT_VERSION}"
    );
    assert_eq!(
        v["family"], "828d",
        "省略 family ⇒ 全部已发布系列（今天只有 828d）"
    );
    assert_eq!(v["revision"], 1);
    assert_eq!(v["driver_id"], "sinumerik-nck");
    assert_eq!(v["driver_version"], env!("CARGO_PKG_VERSION"));
    assert!(v["note"].is_null(), "正常目录不得带 note");
    assert_eq!(v["total"], 42);
    assert_eq!(v["limit"], 100, "缺省页大小必须为 100");
    assert!(v["next_cursor"].is_null(), "42 < 100 ⇒ 无下一页");

    let rows = v["definitions"].as_array().expect("definitions 必须是数组");
    assert_eq!(rows.len(), 42, "目录必须如实返回全部 42 条");

    let lock = lock_digests();
    assert_eq!(lock.len(), 42, "DIGESTS.lock 必须恰含 42 条已发布摘要");
    for row in rows {
        let id = row["variable_id"].as_str().expect("variable_id");
        let digest = row["digest"].as_str().expect("digest");
        assert_eq!(digest.len(), 64, "{id} 的 digest 必须是 64 位小写 hex");
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{id} 的 digest 必须是小写 hex：{digest}"
        );
        assert_eq!(
            lock.get(id).map(String::as_str),
            Some(digest),
            "{id} 的 digest 必须与 DIGESTS.lock 逐字一致"
        );
    }
}

/// I2：每行是 `r1.json` 的**逐字投影**，且键集合恰为白名单（无原生字段）。
///
/// NOTE(导航字段)：键集合比 `r1.json` 多 `category` / `category_basis` 两个**投影期
/// 计算**的导航字段（见 `catalog_category.rs`）。它们不是定义字段，因此只出现在
/// 键集合与下方"逐字一致"循环之外——后者仍然逐条比对 13 个 `r1.json` 字段。
#[tokio::test]
async fn definitions_payload_is_verbatim_projection() {
    let whitelist: BTreeSet<&str> = [
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
        "category",
        "category_basis",
    ]
    .into_iter()
    .collect();
    let forbidden = [
        "addressing",
        "decode",
        "transport_size",
        "count",
        "unit_mode",
        "invalid_rule",
    ];

    let v = catalog_json(json!({ "limit": 500 })).await;
    let rows = v["definitions"].as_array().expect("definitions 必须是数组");
    let raw = raw_definitions();
    assert_eq!(rows.len(), raw.len());

    for row in rows {
        let id = row["variable_id"].as_str().expect("variable_id");
        let src = raw
            .get(id)
            .unwrap_or_else(|| panic!("{id} 不在 r1.json 中"));
        let obj = row.as_object().expect("每条定义必须是对象");
        let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(keys, whitelist, "{id} 的键集合必须恰为投影白名单");
        for field in forbidden {
            assert!(
                !obj.contains_key(field),
                "{id} 不得返回原生字段 `{field}`（语义可见面不含寻址/解码属性）"
            );
        }
        // 逐字一致：不重命名、不重排、不重算（digest 是装载期计算值，见 I1）。
        for field in [
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
        ] {
            assert_eq!(row[field], src[field], "{id} 的 `{field}` 必须逐字投影");
        }
    }
}

/// I3：26/130 的入口区别由 `golden_ref[].entry` 保留，且**不暴露原始 Module 数字**。
#[tokio::test]
async fn definitions_preserves_two_drive_entries_without_module_numbers() {
    let raw_text = driver()
        .definitions(&json!({}).to_string())
        .await
        .expect("目录查询应成功");
    let v: Value = serde_json::from_str(&raw_text).expect("目录响应必须是合法 JSON");
    let rows = v["definitions"].as_array().expect("definitions 必须是数组");

    let both: BTreeSet<&str> = ["nc_variable", "drive_object"].into_iter().collect();
    let mut two_entry = 0;
    for row in rows {
        let id = row["variable_id"].as_str().expect("variable_id");
        let refs = row["golden_ref"].as_array().expect("golden_ref 必须是数组");
        for r in refs {
            let keys: BTreeSet<&str> = r
                .as_object()
                .expect("golden_ref 元素必须是对象")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                keys,
                ["area", "block", "variable", "entry"].into_iter().collect(),
                "{id} 的 golden_ref 只允许四元组（entry 是入口唯一标识）"
            );
            let entry = r["entry"].as_str().expect("entry 必须是字符串");
            assert!(
                both.contains(entry),
                "{id} 的 entry 只能是 nc_variable / drive_object，实际 `{entry}`"
            );
        }
        let entries: BTreeSet<&str> = refs
            .iter()
            .map(|r| r["entry"].as_str().expect("entry"))
            .collect();
        if id.starts_with("drive.") {
            assert_eq!(
                entries, both,
                "{id} 必须同时保留两条入口（26/130 不可折叠）"
            );
            two_entry += 1;
        } else {
            assert_eq!(entries.len(), 1, "{id} 只应有一条入口");
        }
    }
    assert_eq!(
        two_entry, 20,
        "20 条 drive.* 各含两条入口（20×2 + 22×1 = 62）"
    );

    // A14：入口只能由 `entry` 表达，响应里不得出现原生 Module 字段——
    // 否则前端有机会「从 26/130 反推 entry」（PR-C A14 明令禁止）。
    assert!(
        !raw_text.contains("\"module\":"),
        "响应不得包含原生 `module` 字段：{raw_text}"
    );
    assert!(
        !raw_text.contains("\"module_number\":"),
        "响应不得包含原生 `module_number` 字段"
    );
}

/// I4：`scope` 与 `semantics_status` **原样**返回（不得兜底成 verified）。
#[tokio::test]
async fn definitions_reports_scope_and_status_verbatim() {
    let v = catalog_json(json!({})).await;
    let rows = v["definitions"].as_array().expect("definitions 必须是数组");

    let mut scope = BTreeMap::new();
    let mut status = BTreeMap::new();
    for row in rows {
        *scope
            .entry(row["scope"].as_str().expect("scope").to_string())
            .or_insert(0usize) += 1;
        *status
            .entry(
                row["semantics_status"]
                    .as_str()
                    .expect("status")
                    .to_string(),
            )
            .or_insert(0usize) += 1;
    }
    assert_eq!(scope.get("collect"), Some(&36));
    assert_eq!(scope.get("identity"), Some(&6));
    assert_eq!(status.get("verified"), Some(&24));
    assert_eq!(status.get("parameter_identity"), Some(&16));
    assert_eq!(status.get("candidate"), Some(&2));
}

/// I44：目录**如实返回**全部 42 条，6 条 `scope="identity"` 逐条在列。
///
/// 这是「禁止静默删六条」的硬性断言：少 6 条 + 浏览树仍 62 叶会形成两份真值。
#[tokio::test]
async fn catalog_returns_all_six_identity_scoped_definitions() {
    let v = catalog_json(json!({})).await;
    assert_eq!(v["total"], 42, "目录总数必须是 42（不是 36）");
    let rows = v["definitions"].as_array().expect("definitions 必须是数组");
    assert_eq!(rows.len(), 42);

    let identity: Vec<&str> = rows
        .iter()
        .filter(|r| r["scope"] == "identity")
        .map(|r| r["variable_id"].as_str().expect("variable_id"))
        .collect();
    assert_eq!(
        identity,
        vec![
            "axis.ctrlout.module_number",
            "axis.ctrlout.type",
            "axis.encoder.module_number",
            "axis.name",
            "axis.spindle_assignment",
            "channel.machine_axis_assignment",
        ],
        "6 条 identity 变量必须逐条在列、不得改名/过滤"
    );

    // 反向断言：不存在「目录少 6 条 + 浏览树仍 62 叶」的组合 —— 总数仍是 42；
    // 62 个入口由 golden_ref 计数独立复核（20×2 + 22×1）。
    let entries: usize = rows
        .iter()
        .map(|r| r["golden_ref"].as_array().expect("golden_ref").len())
        .sum();
    assert_eq!(entries, 62, "42 条定义必须仍投影出 62 个入口");
}

// ---------------------------------------------------------------------------
// I5 / I6 / E8 / E9 / E10 / E11 / E12：查询与过滤
// ---------------------------------------------------------------------------

/// I5（E8/E9）：省略 family ⇒ 42；`828d` ⇒ 42；`840d-sl` ⇒ 200 + `[]` + note；`foo` ⇒ 拒绝。
#[tokio::test]
async fn definitions_family_filter_and_undeclared_family() {
    assert_eq!(catalog_json(json!({})).await["total"], 42);
    let only = catalog_json(json!({ "family": "828d" })).await;
    assert_eq!(only["total"], 42);
    assert_eq!(only["family"], "828d");

    // E9：已声明、无定义 ⇒ **不是错误**，必须可区分于 E8/E7。
    let empty = catalog_json(json!({ "family": "840d-sl" })).await;
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["definitions"], json!([]));
    assert_eq!(empty["family"], "840d-sl");
    assert!(
        empty["note"]
            .as_str()
            .is_some_and(|n| n.contains("840d-sl")),
        "空目录必须带 note：{empty}"
    );

    // E8：未声明系列 ⇒ `Unsupported`（⇒ Core 501），绝不误报「设备离线」。
    let e = catalog_err(json!({ "family": "foo" })).await;
    assert_eq!(e.code, "UNSUPPORTED_FAMILY");
    assert_eq!(e.kind.as_str(), "Unsupported");
    assert!(
        e.message.contains("840d-sl"),
        "消息必须列出已声明系列：{}",
        e.message
    );
}

/// I6（E10/E11/E12）：精确查询、未知变量、revision 缺失与非法组合。
#[tokio::test]
async fn definitions_variable_and_revision_filters() {
    let hit = catalog_json(json!({ "variable_id": "channel.r_parameter", "revision": 1 })).await;
    assert_eq!(hit["total"], 1);
    let rows = hit["definitions"].as_array().expect("definitions");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["variable_id"], "channel.r_parameter");

    let e = catalog_err(json!({ "variable_id": "no.such.variable", "revision": 1 })).await;
    assert_eq!(e.code, "UNKNOWN_VARIABLE");
    assert_eq!(e.kind.as_str(), "AddressError");

    // revision 不存在 ⇒ 绝不回落最新版本，消息含 available[]。
    let e = catalog_err(json!({ "variable_id": "channel.r_parameter", "revision": 2 })).await;
    assert_eq!(e.code, "UNSUPPORTED_DEFINITION_REVISION");
    assert_eq!(e.kind.as_str(), "ConfigurationError");
    assert!(
        e.message.contains("[1]"),
        "消息必须含 available[]：{}",
        e.message
    );

    // E12：只给 revision / 只给 variable_id / 非法 limit / 非法游标组合。
    for bad in [
        json!({ "revision": 1 }),
        json!({ "variable_id": "channel.r_parameter" }),
        json!({ "limit": 0 }),
        json!({ "limit": "10" }),
        json!({ "limit": -1 }),
        json!({ "revision": 0, "variable_id": "channel.r_parameter" }),
        json!({ "cursor": "channel.r_parameter", "variable_id": "channel.r_parameter", "revision": 1 }),
    ] {
        let e = catalog_err(bad.clone()).await;
        assert_eq!(e.code, "INVALID_QUERY", "查询 `{bad}` 必须被拒");
        assert_eq!(e.kind.as_str(), "ConfigurationError");
    }
}

/// I37③（§6.4-①-②/③）：结果上限、夹取、分页**不重不漏**。
#[tokio::test]
async fn definition_query_result_cap_and_pagination_enforced() {
    // ① 缺省信封。
    let v = catalog_json(json!({})).await;
    assert_eq!(v["limit"], 100);
    assert_eq!(v["total"], 42);
    assert!(v["next_cursor"].is_null());

    // ② 超上限 ⇒ 夹取到 500（不是错误，与 Browse 同口径）。
    let clamped = catalog_json(json!({ "limit": 5000 })).await;
    assert_eq!(clamped["limit"], 500);
    assert_eq!(
        clamped["definitions"]
            .as_array()
            .expect("definitions")
            .len(),
        42
    );

    // ③ 小页翻页：逐字回传 next_cursor，直到 null。
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match &cursor {
            None => json!({ "limit": 10 }),
            Some(c) => json!({ "limit": 10, "cursor": c }),
        };
        let page = catalog_json(query).await;
        assert_eq!(page["total"], 42, "total 是过滤后、分页前的总数");
        assert_eq!(page["limit"], 10);
        let rows = page["definitions"].as_array().expect("definitions");
        assert!(!rows.is_empty(), "未到末页时不得返回空页");
        for row in rows {
            seen.push(
                row["variable_id"]
                    .as_str()
                    .expect("variable_id")
                    .to_string(),
            );
        }
        pages += 1;
        assert!(pages <= 10, "翻页不得死循环");
        match page["next_cursor"].as_str() {
            Some(c) => {
                let last = rows.last().expect("rows");
                // 游标是**复合**排序键（variable_id + revision），不是裸 variable_id：
                // 只记 variable_id 会在「同一变量多 revision 且页边界切在组中间」时漏项。
                assert_eq!(
                    c,
                    format!(
                        "v1-{}-{}",
                        last["variable_id"].as_str().expect("variable_id"),
                        last["revision"].as_u64().expect("revision")
                    ),
                    "next_cursor 必须是复合排序键"
                );
                cursor = Some(c.to_string());
            }
            None => break,
        }
    }
    assert_eq!(seen.len(), 42, "翻页必须取全 42 条（不重不漏）");
    let unique: BTreeSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), 42, "翻页不得重复");
    let expected: Vec<String> = all_definitions(&catalog())
        .iter()
        .map(|d| d.variable_id.clone())
        .collect();
    assert_eq!(seen, expected, "翻页顺序 = variable_id 字典序的稳定遍历");

    // ④ 游标不在结果集 / 旧式「仅 variable_id」编码 ⇒ INVALID_QUERY
    //    （不存在的锚点无法定义「下一页」；旧编码无法表达组内 revision，必须明确拒绝）。
    for bad in ["no.such.variable", "channel.r_parameter"] {
        let e = catalog_err(json!({ "cursor": bad })).await;
        assert_eq!(e.code, "INVALID_QUERY", "cursor={bad}");
    }
}

// ---------------------------------------------------------------------------
// I11–I20：权威预览
// ---------------------------------------------------------------------------

/// I11：请求/输出语义（DC §5.2 用例 A）。
#[tokio::test]
async fn preview_returns_authoritative_point_key() {
    let v = preview(preview_request(
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "channel.r_parameter",
            "definition_revision": 1,
            "target": { "channel": 1 },
            "arguments": { "index": 5 },
        }),
    ))
    .await;
    assert_eq!(v["point_key"], "channel.r_parameter#index=i:5@channel=i:1");
    assert_eq!(v["identity_mode"], "derived");
    assert_eq!(v["scope"], "collect");
    assert_eq!(v["variable_id"], "channel.r_parameter");
    assert_eq!(v["definition_revision"], 1);
    assert_eq!(
        v["definition_digest"],
        lock_digests()["channel.r_parameter"].as_str()
    );
}

/// I12：42 条逐条 `preview == 编译链重算的 canonical_identity`（预览不谎报）。
#[test]
fn preview_equals_configure_recomputed_identity_for_all_42() {
    let catalog = catalog();
    let compiler = AddressCompiler::new(&catalog, "828d");
    let mut checked = 0;
    for def in all_definitions(&catalog) {
        let parameters = legal_parameters(def, DriveEntry::NcVariable);
        let map = parameters
            .as_object()
            .expect("parameters 必须是对象")
            .clone();
        let previewed = compiler
            .preview_point_key(POINT_KIND_VARIABLE, &map)
            .unwrap_or_else(|e| panic!("{} 预览失败：{e}", def.variable_id));
        // 用预览 Key 走 Configure 的同一编译入口：D11 接受 ⇒ 逐字相等。
        let compiled = compiler
            .compile_point(POINT_KIND_VARIABLE, &map, &previewed.canonical_identity)
            .unwrap_or_else(|e| panic!("{} 的预览 Key 不被编译链接受：{e}", def.variable_id));
        assert_eq!(
            previewed.canonical_identity, compiled.canonical_identity,
            "{} 的预览身份必须等于编译链重算值",
            def.variable_id
        );
        checked += 1;
    }
    assert_eq!(checked, 42, "必须逐条覆盖 42 条定义");
}

/// I32：**跨接口一致性** —— 42 条 × Preview → **真实 Configure**。
///
/// 判据（DC §5.4.2）：① 预览 200 且非空；② Key 逐字写入 `PointSelection.point_key`
/// 后 Configure 按 **scope** 分流——36 条 `collect` 接受、6 条 `identity` 拒绝
/// （PR-B3 采集准入）；③ 接受的点位描述符数 == 点位数；④ 接受即意味着 D11 的
/// 「持久化 Key == 重算 Key」成立（**无归一化**）。
///
/// 为什么这不是「放宽」：覆盖数仍为 42；旧断言「42/42 全接受」固化了
/// 「identity 定义可被采集」这一缺陷，新断言额外要求 6 条必须被拒绝，且
/// 码/kind/消息三要素精确——判定方向更多、更严。
#[tokio::test]
async fn preview_identity_equals_subsequently_configured_identity() {
    let conn = conn_828d();
    let catalog = catalog();
    let mut checked = 0;
    let mut accepted = 0;
    let mut rejected = 0;
    for def in all_definitions(&catalog) {
        let parameters = legal_parameters(def, DriveEntry::NcVariable);
        let previewed = preview(preview_request(POINT_KIND_VARIABLE, parameters.clone())).await;
        let previewed_key = previewed["point_key"]
            .as_str()
            .unwrap_or_else(|| panic!("{} 的预览 Key 必须是字符串", def.variable_id))
            .to_string();
        assert!(!previewed_key.is_empty());

        match def.scope {
            DefinitionScope::Collect => {
                let descriptors = configure_one(
                    &conn,
                    POINT_KIND_VARIABLE,
                    parameters.clone(),
                    &previewed_key,
                )
                .await
                .unwrap_or_else(|e| {
                    panic!("{} 的预览 Key 未被 Configure 接受：{e}", def.variable_id)
                });
                assert_eq!(descriptors.len(), 1);
                assert_eq!(
                    descriptors[0].point_key, previewed_key,
                    "{}：Configure 接受的身份必须与预览逐字一致",
                    def.variable_id
                );
                accepted += 1;
            }
            DefinitionScope::Identity => {
                let e = configure_one(
                    &conn,
                    POINT_KIND_VARIABLE,
                    parameters.clone(),
                    &previewed_key,
                )
                .await
                .expect_err("identity 定义不得被 Configure 接受");
                assert_eq!(
                    e.code, "DEFINITION_SCOPE_NOT_COLLECTABLE",
                    "{}",
                    def.variable_id
                );
                assert_eq!(e.kind.as_str(), "ConfigurationError");
                assert!(e.message.contains(&def.variable_id), "{}", e.message);
                assert!(
                    e.message.contains(&format!("revision={}", def.revision)),
                    "{}",
                    e.message
                );
                assert!(e.message.contains("t-i32"), "{}", e.message);
                rejected += 1;
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 42, "I32 必须逐条覆盖 42 条定义（42/42）");
    assert_eq!(accepted, 36, "36 条 collect 必须全部 Configure 成功");
    assert_eq!(rejected, 6, "6 条 identity 必须全部 Configure 拒绝");
}

/// I13：复合 Target `{channel,axis}` / `{channel,spindle}` 预览成功且两个键都进身份。
#[tokio::test]
async fn preview_supports_compound_targets() {
    let catalog = catalog();
    let mut axis_pair = 0;
    let mut spindle_pair = 0;
    for def in all_definitions(&catalog) {
        let decl: BTreeSet<TargetKind> = def.target_required.iter().copied().collect();
        let is_axis = decl.contains(&TargetKind::Channel) && decl.contains(&TargetKind::Axis);
        let is_spindle = decl.contains(&TargetKind::Channel) && decl.contains(&TargetKind::Spindle);
        if !is_axis && !is_spindle {
            continue;
        }
        let v = preview(preview_request(
            POINT_KIND_VARIABLE,
            legal_parameters(def, DriveEntry::NcVariable),
        ))
        .await;
        let key = v["point_key"].as_str().expect("point_key");
        assert!(
            key.contains("channel=i:1"),
            "{} 缺 channel 段：{key}",
            def.variable_id
        );
        if is_axis {
            assert!(
                key.contains("axis=i:1"),
                "{} 缺 axis 段：{key}",
                def.variable_id
            );
            axis_pair += 1;
        }
        if is_spindle {
            assert!(
                key.contains("spindle=i:1"),
                "{} 缺 spindle 段：{key}",
                def.variable_id
            );
            spindle_pair += 1;
        }
    }
    assert_eq!(axis_pair, 2, "{{channel,axis}} 定义应为 2 条");
    assert_eq!(spindle_pair, 3, "{{channel,spindle}} 定义应为 3 条");
}

/// I14：`drive_object.entry` **不可折叠**（26/130 在身份层是两个入口）。
#[tokio::test]
async fn preview_drive_object_entry_is_not_collapsible() {
    let parameters = |entry: &str| {
        json!({
            "variable_id": "drive.parameter.r0021",
            "definition_revision": 1,
            "target": { "drive_object": { "number": 3, "entry": entry } },
            "arguments": {},
        })
    };
    let nc = preview(preview_request(
        POINT_KIND_VARIABLE,
        parameters("nc_variable"),
    ))
    .await;
    let drive = preview(preview_request(
        POINT_KIND_VARIABLE,
        parameters("drive_object"),
    ))
    .await;
    assert_eq!(
        nc["point_key"],
        "drive.parameter.r0021@drive_object=o:{entry=s:nc_variable,number=i:3}"
    );
    assert_eq!(
        drive["point_key"],
        "drive.parameter.r0021@drive_object=o:{entry=s:drive_object,number=i:3}"
    );
    assert_ne!(
        nc["point_key"], drive["point_key"],
        "两条入口必须给出不同的权威身份"
    );
}

/// I15：`nck_native` + `derived` 使用**冻结规则**，Key 不含质量/解码属性。
#[tokio::test]
async fn preview_native_derived_uses_frozen_rule() {
    let v = preview(preview_request(
        POINT_KIND_NATIVE,
        native_derived_parameters(),
    ))
    .await;
    assert_eq!(v["point_key"], "native.C.1.21.1.396");
    assert_eq!(v["identity_mode"], "derived");
    assert!(v["scope"].is_null(), "原生点位没有 scope");
    assert!(v["variable_id"].is_null(), "原生点位没有定义坐标");
    assert!(v["definition_revision"].is_null());
    assert!(v["definition_digest"].is_null());

    let key = v["point_key"].as_str().expect("point_key");
    for token in [
        "count",
        "decode",
        "transport_size",
        "invalid",
        "identity_mode",
    ] {
        assert!(!key.contains(token), "Key 不得含 `{token}`：{key}");
    }

    // 显式 `identity_mode="derived"` 与缺省等价。
    let mut explicit = native_derived_parameters();
    explicit["identity_mode"] = json!("derived");
    let v = preview(preview_request(POINT_KIND_NATIVE, explicit)).await;
    assert_eq!(v["point_key"], "native.C.1.21.1.396");
}

/// I16（E18）：`nck_native` + `identity_mode="user"` 明确拒绝；
/// 且**既有 Native 自定义 Key 行为不变**（Configure 仍接受，绝不自动追加 `~<n>`）。
#[tokio::test]
async fn preview_rejects_native_user_mode() {
    let mut parameters = native_derived_parameters();
    parameters["identity_mode"] = json!("user");

    let e = preview_err(preview_request(POINT_KIND_NATIVE, parameters.clone())).await;
    assert_eq!(e.code, "INVALID_BINDING_CONFIG");
    assert_eq!(e.kind.as_str(), "ConfigurationError");
    assert!(
        e.message.contains("user"),
        "消息必须写明 user 模式无权威身份可重算：{}",
        e.message
    );

    // 回归：user Key 的既有语义**一个字都没变**——Configure 接受用户显式 Key，
    // 不改名、不追加后缀（P0-2 第二条硬约束）。
    let conn = conn_828d();
    let user_key = "native.C.1.21.1.396";
    let descriptors = configure_one(&conn, POINT_KIND_NATIVE, parameters, user_key)
        .await
        .expect("既有 user 模式必须仍被 Configure 接受");
    assert_eq!(descriptors[0].point_key, user_key);
}

/// I17（E19/E20）：未知变量 / 不存在的 revision —— 明确错误，**不给** Key。
#[tokio::test]
async fn preview_rejects_unknown_variable_and_revision() {
    let e = preview_err(preview_request(
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "no.such.variable",
            "definition_revision": 1,
            "target": { "channel": 1 },
            "arguments": {},
        }),
    ))
    .await;
    assert_eq!(e.code, "UNKNOWN_VARIABLE");
    assert_eq!(e.kind.as_str(), "AddressError");

    let e = preview_err(preview_request(
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "channel.r_parameter",
            "definition_revision": 2,
            "target": { "channel": 1 },
            "arguments": { "index": 5 },
        }),
    ))
    .await;
    assert_eq!(e.code, "UNSUPPORTED_DEFINITION_REVISION");
    assert!(
        e.message.contains("[1]"),
        "消息必须含 available[]：{}",
        e.message
    );
}

/// I18（E22–E26）：非法 Target / arguments 的**精确码**逐条可达。
#[tokio::test]
async fn preview_rejects_illegal_target_and_arguments_with_precise_codes() {
    let base = |target: Value, arguments: Value| {
        json!({
            "variable_id": "channel.r_parameter",
            "definition_revision": 1,
            "target": target,
            "arguments": arguments,
        })
    };
    let cases: [(&str, Value); 6] = [
        // E22：空 target ⇒ MISSING_TARGET
        ("MISSING_TARGET", base(json!({}), json!({ "index": 5 }))),
        // E23：提供了未声明的变体 ⇒ UNSUPPORTED_TARGET
        (
            "UNSUPPORTED_TARGET",
            base(json!({ "channel": 1, "axis": 25 }), json!({ "index": 5 })),
        ),
        // E24：实例号越界 ⇒ INVALID_TARGET
        (
            "INVALID_TARGET",
            base(json!({ "channel": 256 }), json!({ "index": 5 })),
        ),
        // E25：arguments 超上界 ⇒ INVALID_ARGUMENTS
        (
            "INVALID_ARGUMENTS",
            base(json!({ "channel": 1 }), json!({ "index": 65535 })),
        ),
        // E26：身份键缺失 ⇒ MISSING_IDENTITY_KEY
        (
            "MISSING_IDENTITY_KEY",
            base(json!({ "channel": 1 }), json!({})),
        ),
        // E17：参数层未知键 ⇒ INVALID_BINDING_CONFIG（同一层 D2）
        (
            "INVALID_BINDING_CONFIG",
            json!({
                "variable_id": "channel.r_parameter",
                "definition_revision": 1,
                "target": { "channel": 1 },
                "arguments": { "index": 5 },
                "extra": true,
            }),
        ),
    ];
    for (code, parameters) in cases {
        let e = preview_err(preview_request(POINT_KIND_VARIABLE, parameters.clone())).await;
        assert_eq!(e.code, code, "parameters={parameters} 必须给出 `{code}`");
    }

    // `drive_object.entry` 缺失 ⇒ INVALID_TARGET（入口必填、无默认 module 26）。
    let e = preview_err(preview_request(
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "drive.parameter.r0021",
            "definition_revision": 1,
            "target": { "drive_object": { "number": 3 } },
            "arguments": {},
        }),
    ))
    .await;
    assert_eq!(e.code, "INVALID_TARGET");
    assert!(
        e.message.contains("entry"),
        "消息须指明缺失 entry：{}",
        e.message
    );

    // E17：同一层的**类型错**（D3）与「语义点位不接受 `identity_mode`」（B-3）
    // 都必须落 `INVALID_BINDING_CONFIG`——类型被静默强转会算出「另一个 Key」。
    for parameters in [
        json!({
            "variable_id": "channel.r_parameter",
            "definition_revision": "1",
            "target": { "channel": 1 },
            "arguments": { "index": 5 },
        }),
        json!({
            "variable_id": "channel.r_parameter",
            "definition_revision": 1,
            "target": [],
            "arguments": { "index": 5 },
        }),
        json!({
            "variable_id": "channel.r_parameter",
            "definition_revision": 1,
            "target": { "channel": 1 },
            "arguments": [],
        }),
        json!({
            "variable_id": "channel.r_parameter",
            "definition_revision": 1,
            "target": { "channel": 1 },
            "arguments": { "index": 5 },
            "identity_mode": "derived",
        }),
    ] {
        let e = preview_err(preview_request(POINT_KIND_VARIABLE, parameters.clone())).await;
        assert_eq!(
            e.code, "INVALID_BINDING_CONFIG",
            "parameters={parameters} 必须给出 INVALID_BINDING_CONFIG"
        );
        assert_eq!(e.kind.as_str(), "ConfigurationError");
    }

    // E17（原生分支）：未知键与非法 `identity_mode` 取值。
    for parameters in [
        json!({ "area": "C", "whatever": 1 }),
        json!({ "area": "C", "identity_mode": "Derived" }),
    ] {
        let e = preview_err(preview_request(POINT_KIND_NATIVE, parameters.clone())).await;
        assert_eq!(
            e.code, "INVALID_BINDING_CONFIG",
            "parameters={parameters} 必须给出 INVALID_BINDING_CONFIG"
        );
    }
}

/// I19：`channel=255` 过 Target 校验、在原生解析失败 ⇒ `INVALID_VARIABLE`，
/// **不得**返回 Key（这就是「预览必须走完整 D1–D10」的反例）。
#[tokio::test]
async fn preview_fails_closed_on_native_resolution() {
    let e = preview_err(preview_request(
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "channel.program.status",
            "definition_revision": 1,
            "target": { "channel": 255 },
            "arguments": {},
        }),
    ))
    .await;
    assert_eq!(e.code, "INVALID_VARIABLE");
    assert_eq!(e.kind.as_str(), "AddressError");
}

/// I20：成功态 `point_key` **非空**；不存在「身份待确认」的成功态。
#[test]
fn preview_never_returns_null_or_partial_key() {
    let catalog = catalog();
    let compiler = AddressCompiler::new(&catalog, "828d");
    for def in all_definitions(&catalog) {
        let parameters = legal_parameters(def, DriveEntry::NcVariable);
        let compiled = compiler
            .preview_point_key(
                POINT_KIND_VARIABLE,
                parameters.as_object().expect("parameters"),
            )
            .unwrap_or_else(|e| panic!("{} 预览失败：{e}", def.variable_id));
        assert!(!compiled.canonical_identity.is_empty());
        assert_ne!(compiled.canonical_identity, "null");
    }
    // 未知 kind 永远走错误分支，绝不返回 Key。
    let err = compiler
        .preview_point_key("nck_unknown", &Map::new())
        .unwrap_err();
    assert_eq!(err.code(), "UNSUPPORTED_POINT_KIND");
}

// ---------------------------------------------------------------------------
// I21 / I22 / I40：只读链与零连接
// ---------------------------------------------------------------------------

/// I21 / G7：只读链七个模块 + `definition_api` 对设备触点/异步原语**零命中**。
#[test]
fn readonly_chain_has_zero_device_touchpoints() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = [
        "definition_api.rs",
        "definition.rs",
        "targets.rs",
        "compiler.rs",
        "native.rs",
        "value.rs",
        "address.rs",
        "codec.rs",
    ];
    let tokens = [
        "NckClient",
        "client::",
        "tokio::net",
        "TcpStream",
        ".await",
        "OpenConnection",
        "WriteRequest",
        "CommandRequest",
    ];
    for file in files {
        let text = std::fs::read_to_string(src.join(file))
            .unwrap_or_else(|e| panic!("读取 {file} 失败：{e}"));
        for token in tokens {
            assert!(
                !text.contains(token),
                "{file} 不得出现只读红线词 `{token}`（设备触点/写请求/异步原语）"
            );
        }
    }
}

/// I22 / R4：**零 Connection 条目**下两个 RPC 仍成功（直接对进程级 Driver 调用）。
#[tokio::test]
async fn management_rpc_requires_no_connection_entry() {
    // 结构性证据：`SinumerikNckDriver` 是无状态单元结构体，不可能持有
    // `NckConnection`；本用例全程**不调用** `open_connection`。
    let d = SinumerikNckDriver;
    let raw = d
        .definitions(&json!({ "family": "828d" }).to_string())
        .await
        .expect("零连接下目录查询必须成功");
    let v: Value = serde_json::from_str(&raw).expect("目录响应必须是 JSON");
    assert_eq!(v["total"], 42);

    let raw = d
        .preview_point_key(
            &preview_request(
                POINT_KIND_VARIABLE,
                json!({
                    "variable_id": "axis.name",
                    "definition_revision": 1,
                    "target": { "axis": 1 },
                    "arguments": {},
                }),
            )
            .to_string(),
        )
        .await
        .expect("零连接下预览必须成功");
    let v: Value = serde_json::from_str(&raw).expect("预览响应必须是 JSON");
    assert_eq!(v["point_key"], "axis.name@axis=i:1");
}

// ---------------------------------------------------------------------------
// I33 / I45：定义变更与 scope=identity 边界
// ---------------------------------------------------------------------------

/// I33：定义已变更 / revision 不受支持 ⇒ **明确错误**，不得给「看似有效」的身份。
#[tokio::test]
async fn preview_rejects_changed_definition_or_unsupported_revision() {
    // ③ 模拟「预览之后、Configure 之前目录被替换」：把 A 的预览 Key 交给 B 的
    //    parameters —— 等价于「持久化 Key 来自旧定义、重算 Key 来自新目录」。
    let previewed = preview(preview_request(
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "channel.program.status",
            "definition_revision": 1,
            "target": { "channel": 1 },
            "arguments": {},
        }),
    ))
    .await;
    let stale_key = previewed["point_key"]
        .as_str()
        .expect("point_key")
        .to_string();

    let conn = conn_828d();
    let e = configure_one(
        &conn,
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "channel.feed.actual",
            "definition_revision": 1,
            "target": { "channel": 1 },
            "arguments": {},
        }),
        &stale_key,
    )
    .await
    .expect_err("旧定义的 Key 不得被新定义接受");
    assert_eq!(e.code, "POINT_KEY_MISMATCH");
    assert_eq!(e.kind.as_str(), "ConfigurationError");
    // 三要素：持久化 Key + 重算 Key + variable_id + revision。
    assert!(
        e.message.contains(&stale_key),
        "消息须含持久化 Key：{}",
        e.message
    );
    assert!(
        e.message.contains("channel.feed.actual@channel=i:1"),
        "消息须含重算 Key：{}",
        e.message
    );
    assert!(e.message.contains("channel.feed.actual"), "{}", e.message);
    assert!(e.message.contains("revision=1"), "{}", e.message);

    // ①② 定义/revision 不存在：预览阶段即明确拒绝（不产生 Key）。
    let e = preview_err(preview_request(
        POINT_KIND_VARIABLE,
        json!({
            "variable_id": "axis.name",
            "definition_revision": 9,
            "target": { "axis": 1 },
            "arguments": {},
        }),
    ))
    .await;
    assert_eq!(e.code, "UNSUPPORTED_DEFINITION_REVISION");
    assert!(e.message.contains("[1]"));
}

/// I45：6 条 `scope="identity"` 的 **Preview 仍如实返回身份**，而 Configure
/// **必须拒绝**（PR-B3 采集准入）。
///
/// Preview 半边逐字保留（这是「未破坏权威预览」的回归）；Configure 半边反转旧
/// 断言——旧断言写死「I-2 没有把 scope 校验下沉到 Configure：同一 Key 必须被
/// 接受」，它固化的正是本缺陷，属需求变更而非放宽。
#[tokio::test]
async fn preview_scope_semantics_match_configure_for_identity_scope() {
    let conn = conn_828d();
    let catalog = catalog();
    let identity: Vec<&VariableDefinition> = all_definitions(&catalog)
        .into_iter()
        .filter(|d| d.scope == DefinitionScope::Identity)
        .collect();
    assert_eq!(identity.len(), 6, "identity scope 必须恰为 6 条");

    for def in identity {
        let parameters = legal_parameters(def, DriveEntry::NcVariable);
        let previewed = preview(preview_request(POINT_KIND_VARIABLE, parameters.clone())).await;
        assert_eq!(
            previewed["scope"], "identity",
            "{} 的 scope 必须如实回显",
            def.variable_id
        );
        let key = previewed["point_key"]
            .as_str()
            .expect("point_key")
            .to_string();
        assert!(!key.is_empty());
        // PR-B3：Preview 不负责采集准入（它必须给出权威身份），Configure 才拒绝。
        let e = configure_one(&conn, POINT_KIND_VARIABLE, parameters, &key)
            .await
            .expect_err("identity 定义不得进入采集计划");
        assert_eq!(e.code, "DEFINITION_SCOPE_NOT_COLLECTABLE");
        assert_eq!(e.kind.as_str(), "ConfigurationError");
        assert!(e.message.contains(&def.variable_id), "{}", e.message);
        assert!(
            e.message.contains(&format!("revision={}", def.revision)),
            "{}",
            e.message
        );
    }
}

// ---------------------------------------------------------------------------
// PR-B3：Configure 采集准入（scope=identity 拒绝 / scope=collect 放行）
// ---------------------------------------------------------------------------

/// PR-B3 证据 1：6 条 `scope=identity` **逐条**被 Configure 拒绝，且
/// 码 / kind / 消息三要素（task id + variable_id + revision）精确。
#[tokio::test]
async fn configure_rejects_all_six_identity_definitions() {
    let conn = conn_828d();
    let catalog = catalog();
    let identity: Vec<&VariableDefinition> = all_definitions(&catalog)
        .into_iter()
        .filter(|d| d.scope == DefinitionScope::Identity)
        .collect();
    assert_eq!(identity.len(), 6, "identity scope 必须恰为 6 条");

    let mut checked = 0;
    for def in identity {
        let parameters = legal_parameters(def, DriveEntry::NcVariable);
        // 用**真实预览 Key** 走 Configure：拒绝必须发生在身份核对之后，
        // 而不是「Key 算错」的副作用。
        let previewed = preview(preview_request(POINT_KIND_VARIABLE, parameters.clone())).await;
        let key = previewed["point_key"]
            .as_str()
            .expect("point_key")
            .to_string();

        let e = configure_one(&conn, POINT_KIND_VARIABLE, parameters, &key)
            .await
            .expect_err("identity 定义必须被 Configure 拒绝");
        assert_eq!(
            e.code, "DEFINITION_SCOPE_NOT_COLLECTABLE",
            "{}",
            def.variable_id
        );
        assert_eq!(e.kind.as_str(), "ConfigurationError");
        assert!(
            e.message.contains("t-i32"),
            "消息必须含 Task ID：{}",
            e.message
        );
        assert!(
            e.message.contains(&def.variable_id),
            "消息必须含 Variable ID：{}",
            e.message
        );
        assert!(
            e.message.contains(&format!("revision={}", def.revision)),
            "消息必须含 Revision：{}",
            e.message
        );
        assert!(
            e.message.contains("scope=`identity`"),
            "消息必须点明实际 scope：{}",
            e.message
        );
        checked += 1;
    }
    assert_eq!(checked, 6, "必须逐条覆盖 6 条 identity 定义");
}

/// PR-B3 证据 2：36 条 `scope=collect` **逐条** Configure 成功，描述符不退化。
#[tokio::test]
async fn configure_accepts_all_thirty_six_collect_definitions() {
    let conn = conn_828d();
    let catalog = catalog();
    let collect: Vec<&VariableDefinition> = all_definitions(&catalog)
        .into_iter()
        .filter(|d| d.scope == DefinitionScope::Collect)
        .collect();
    assert_eq!(collect.len(), 36, "collect scope 必须恰为 36 条");

    let mut checked = 0;
    for def in collect {
        let parameters = legal_parameters(def, DriveEntry::NcVariable);
        let previewed = preview(preview_request(POINT_KIND_VARIABLE, parameters.clone())).await;
        let key = previewed["point_key"]
            .as_str()
            .expect("point_key")
            .to_string();
        let descriptors = configure_one(&conn, POINT_KIND_VARIABLE, parameters, &key)
            .await
            .unwrap_or_else(|e| panic!("{} 的 collect 定义必须被接受：{e}", def.variable_id));
        assert_eq!(descriptors.len(), 1, "{}", def.variable_id);
        assert_eq!(descriptors[0].point_key, key, "{}", def.variable_id);
        checked += 1;
    }
    assert_eq!(checked, 36, "必须逐条覆盖 36 条 collect 定义");
}

/// PR-B3 证据 3：Native derived / user 与合法语义点位混合配置成功（不误伤）。
#[tokio::test]
async fn configure_accepts_native_derived_and_user_alongside_collect() {
    let conn = conn_828d();
    let mut user_parameters = native_derived_parameters();
    user_parameters["identity_mode"] = json!("user");

    let points = json!([
        {
            "kind": POINT_KIND_NATIVE,
            "point_key": "native.C.1.21.1.396",
            "parameters": native_derived_parameters(),
        },
        {
            // user 模式：Key 由用户显式指定，Driver 不重算、不改名。
            "kind": POINT_KIND_NATIVE,
            "point_key": "native.C.1.21.1.396~2",
            "parameters": user_parameters,
        },
        {
            "kind": POINT_KIND_VARIABLE,
            "point_key": "channel.program.status@channel=i:1",
            "parameters": {
                "variable_id": "channel.program.status",
                "definition_revision": 1,
                "target": { "channel": 1 },
                "arguments": {},
            },
        },
    ]);
    let descriptors = configure_points(&conn, "t-native-mixed", points)
        .await
        .expect("Native derived/user 与合法语义点位混合必须成功");
    assert_eq!(descriptors.len(), 3);
    assert_eq!(descriptors[0].point_key, "native.C.1.21.1.396");
    assert_eq!(descriptors[1].point_key, "native.C.1.21.1.396~2");
    assert_eq!(
        descriptors[2].point_key,
        "channel.program.status@channel=i:1"
    );
}

/// PR-B3 证据 4：合法与非法混合 ⇒ 整体拒绝；失败不污染后续合法 Configure。
///
/// 注：`PlanSnapshot` 是驱动私有状态，进程内 `plan.is_none()` 的权威断言在
/// `lib.rs` 的同名单测（`configure_rejects_mixed_batch_and_leaves_no_partial_plan`）；
/// 本用例从**外部可观察面**验证整包拒绝与后续可恢复。
#[tokio::test]
async fn configure_rejects_mixed_batch_without_partial_plan() {
    let conn = conn_828d();
    let identity_point = json!({
        "kind": POINT_KIND_VARIABLE,
        "point_key": "axis.name@axis=i:1",
        "parameters": {
            "variable_id": "axis.name",
            "definition_revision": 1,
            "target": { "axis": 1 },
            "arguments": {},
        },
    });
    let collect_point = json!({
        "kind": POINT_KIND_VARIABLE,
        "point_key": "channel.program.status@channel=i:1",
        "parameters": {
            "variable_id": "channel.program.status",
            "definition_revision": 1,
            "target": { "channel": 1 },
            "arguments": {},
        },
    });
    let points = json!([
        {
            "kind": POINT_KIND_NATIVE,
            "point_key": "native.C.1.21.1.396",
            "parameters": native_derived_parameters(),
        },
        collect_point.clone(),
        // identity 放最后：若实现「边编译边提交」，前两点已进入局部计划。
        identity_point,
    ]);
    let e = configure_points(&conn, "t-mixed-bad", points)
        .await
        .expect_err("批次含 identity 点位时必须整体拒绝");
    assert_eq!(e.code, "DEFINITION_SCOPE_NOT_COLLECTABLE");
    assert!(e.message.contains("t-mixed-bad"), "{}", e.message);
    assert!(e.message.contains("axis.name"), "{}", e.message);
    assert!(e.message.contains("revision=1"), "{}", e.message);

    // 后续纯合法批次仍成功：失败没有污染驱动状态。
    let descriptors = configure_points(&conn, "t-after-failure", json!([collect_point]))
        .await
        .expect("失败后合法批次必须可提交");
    assert_eq!(descriptors.len(), 1);
}

/// PR-B3 证据 5：Preview 对 6 条 identity 仍返回合法身份（未破坏权威预览）。
#[tokio::test]
async fn identity_preview_still_succeeds_for_all_six() {
    let catalog = catalog();
    let identity: Vec<&VariableDefinition> = all_definitions(&catalog)
        .into_iter()
        .filter(|d| d.scope == DefinitionScope::Identity)
        .collect();
    assert_eq!(identity.len(), 6);

    let mut checked = 0;
    for def in identity {
        let parameters = legal_parameters(def, DriveEntry::NcVariable);
        let previewed = preview(preview_request(POINT_KIND_VARIABLE, parameters)).await;
        assert_eq!(previewed["scope"], "identity", "{}", def.variable_id);
        let key = previewed["point_key"]
            .as_str()
            .unwrap_or_else(|| panic!("{} 的预览 Key 必须非空", def.variable_id));
        assert!(!key.is_empty());
        assert_eq!(previewed["identity_mode"], "derived", "{}", def.variable_id);
        checked += 1;
    }
    assert_eq!(checked, 6);
}

// ---------------------------------------------------------------------------
// 缺口关闭：E16（G-a）/ E21（G-e）/ E17（G-f）/ E13 防御纵深
// ---------------------------------------------------------------------------

/// **E16（G-a）**：未知 Point Kind 明确拒绝 ⇒ 501 `UNSUPPORTED_POINT_KIND`，
/// 且 `ErrorKind == Unsupported`（**绝不** `Connection`/`Device`）。
#[tokio::test]
async fn preview_rejects_unsupported_point_kind() {
    for kind in ["foo", "NCK_VARIABLE", "nck_native2", "nck.variable"] {
        let e = preview_err(preview_request(kind, json!({}))).await;
        assert_eq!(e.code, "UNSUPPORTED_POINT_KIND", "kind={kind}");
        assert_eq!(
            e.kind.as_str(),
            "Unsupported",
            "kind={kind}：协议不支持必须是 Unsupported（⇒ 501），不得误报设备离线"
        );
        assert!(e.message.contains(kind), "消息须含 kind：{}", e.message);
    }
    // 空 kind 属**信封形状**问题（E13 口径：非空字符串），不是 E16。
    let e = preview_err(preview_request("", json!({}))).await;
    assert_eq!(e.code, "INVALID_REQUEST");
}

/// **E21（G-e）**：`def.family != 请求 family` ⇒ `UNSUPPORTED_FAMILY`（`Unsupported`）。
#[tokio::test]
async fn preview_rejects_family_mismatch() {
    let request = json!({
        "family": "840d-sl",
        "kind": POINT_KIND_VARIABLE,
        "parameters": {
            "variable_id": "axis.name",
            "definition_revision": 1,
            "target": { "axis": 1 },
            "arguments": {},
        },
    });
    let e = preview_err(request).await;
    assert_eq!(e.code, "UNSUPPORTED_FAMILY");
    assert_eq!(e.kind.as_str(), "Unsupported");
    assert!(
        e.message.contains("828d"),
        "消息须含定义 family：{}",
        e.message
    );
    assert!(
        e.message.contains("840d-sl"),
        "消息须含请求 family：{}",
        e.message
    );
}

/// E13 的 Driver 侧防御纵深：信封形状非法（含禁止的 `point_key`）⇒ `INVALID_REQUEST`。
///
/// Core 已先判一次（`management_api` 的 `preview_passes_request_verbatim_and_validates_envelope`）；
/// 这里补的是「直接调 RPC 也不能绕过形状门」，码与 kind 与 E13 完全一致。
#[tokio::test]
async fn preview_rejects_malformed_envelope() {
    let cases = [
        (
            json!({ "kind": "nck_variable", "parameters": {} }),
            "缺 family",
        ),
        (json!({ "family": "828d", "parameters": {} }), "缺 kind"),
        (
            json!({ "family": "828d", "kind": "nck_variable" }),
            "缺 parameters",
        ),
        (
            json!({ "family": "828d", "kind": "nck_variable", "parameters": {}, "point_key": "x" }),
            "禁止传 point_key",
        ),
        (
            json!({ "family": "828d", "kind": "nck_variable", "parameters": [] }),
            "parameters 必须是对象",
        ),
    ];
    for (request, why) in cases {
        let e = preview_err(request.clone()).await;
        assert_eq!(e.code, "INVALID_REQUEST", "{why}: {request}");
        assert_eq!(e.kind.as_str(), "ConfigurationError", "{why}");
    }
    // 非法 JSON 与「顶层不是对象」同样必须 fail-closed（空 body 也是非法 JSON）。
    for raw in ["not-json", "[]", ""] {
        let e = driver()
            .preview_point_key(raw)
            .await
            .expect_err("非法请求体必须失败");
        assert_eq!(e.code, "INVALID_REQUEST", "body={raw}");
    }
}
