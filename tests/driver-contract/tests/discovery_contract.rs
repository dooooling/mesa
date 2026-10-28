//! Discovery / Browse 契约（V2.1 §20, Milestone H）

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_config_store::ConfigStore;
use std::sync::Arc;
use tower::ServiceExt;

fn fake_opcua_connection() -> serde_json::Value {
    // 允许测试显式使用 Fake 驱动
    unsafe {
        std::env::set_var("MESA_ALLOW_FAKE_NATIVE", "1");
    }
    serde_json::json!({"endpoint_url":"opc.tcp://127.0.0.1:4840","use_native":false})
}

/// suite 内 process-heavy Browse 测试串行锁：同文件测试并行拉起多个真实
/// driver subprocess，spawn/session 建连存在启动瞬态（曾在 Ubuntu ARM 以
/// 偶发 503 现形）。串行只收敛本 suite，不动 workspace 并行度——禁止
/// `--test-threads=1` 一刀切掩盖真正该承受并发的测试。
static BROWSE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 200 断言带 body 取证：非 200 时只看状态码无法区分 DRIVER_UNAVAILABLE /
/// BROWSE_FAILED，必须把 Mesa error body 打出来再判。
async fn assert_browse_ok(resp: axum::response::Response, what: &str) -> serde_json::Value {
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{what} browse failed: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap()
}

async fn app_with_endpoint(
    driver_id: &str,
    connection: serde_json::Value,
) -> (axum::Router, String) {
    app_with_endpoint_in_dir(&common::repo_root().join("drivers"), driver_id, connection).await
}

/// 指定 drivers 目录的 endpoint app（test-driver 用 staged 目录；
/// 正式 drivers/ 不再含 test-driver）。
async fn app_with_endpoint_in_dir(
    drivers_dir: &std::path::Path,
    driver_id: &str,
    connection: serde_json::Value,
) -> (axum::Router, String) {
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
        })
        .unwrap();
    let ep_id = format!("ep-{driver_id}");
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: ep_id.clone(),
            name: ep_id.clone(),
            device_id: "d1".into(),
            driver_id: driver_id.into(),
            connection_json: serde_json::to_string(&connection).unwrap(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(drivers_dir));
    #[allow(deprecated)]
    let state =
        common::isolated_app_state(mgr, store, drivers_dir.to_string_lossy().to_string(), false)
            .expect("isolated AppState");
    let router = state.router();
    (router, ep_id)
}

#[tokio::test]
async fn browse_opcua_pagination_and_filter() {
    let _guard = BROWSE_SERIAL.lock().await;
    // OPC UA Fake 支持 browse（显式 use_native:false，避免默认 Native 去连 127.0.0.1:4840）
    let (app, ep_id) = app_with_endpoint("opcua", fake_opcua_connection()).await;
    // 未过滤，limit 2
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"","limit":2}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let v = assert_browse_ok(resp, "opcua p1").await;
    assert!(v["nodes"].as_array().unwrap().len() <= 2);
    // 若有下一页，next_cursor 非空
    if let Some(next) = v["next_cursor"].as_str()
        && !next.is_empty()
    {
        // 拉下一页
        let req2 = Request::builder()
            .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(format!(
                r#"{{"parent":"","cursor":"{next}","limit":2}}"#
            )))
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_browse_ok(resp2, "opcua p2").await;
    }
    // 过滤
    let req3 = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"","filter":"Fake","limit":10}"#))
        .unwrap();
    let resp3 = app_with_endpoint("opcua", fake_opcua_connection())
        .await
        .0
        .oneshot(req3)
        .await
        .unwrap();
    assert_browse_ok(resp3, "opcua filter").await;
}

#[tokio::test]
async fn browse_unsupported_for_s7_and_simulator() {
    let _guard = BROWSE_SERIAL.lock().await;
    // 防假绿：s7 必须在正式 drivers/ 可发现；test-driver 走 staged 目录
    //（已退役产品身份，不再要求正式目录可发现）。
    let drivers_dir = common::repo_root().join("drivers");
    let mgr = mesa_driver_manager::MesaManager::discover(&drivers_dir);
    assert!(
        mgr.find_driver("s7").is_some(),
        "driver `s7` 必须可被发现（否则 503 假绿）"
    );
    assert!(
        mgr.find_driver("test-driver").is_none(),
        "test-driver 不得出现在正式 drivers/ discovery"
    );
    // staged test-driver：从已构建二进制拷贝 + canonical driver.toml（单真值）。
    let exe = common::test_driver_exe();
    let root = std::env::temp_dir().join(format!(
        "mesa-test-driver-browse-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let dir = root.join("test-driver");
    std::fs::create_dir_all(&dir).unwrap();
    let name = exe.file_name().unwrap().to_string_lossy().to_string();
    std::fs::copy(&exe, dir.join(&name)).unwrap();
    std::fs::copy(
        common::repo_root().join("tests/support/test-driver/driver.toml"),
        dir.join("driver.toml"),
    )
    .unwrap();
    let staged = mesa_driver_manager::MesaManager::discover(&root);
    assert!(
        staged.find_driver("test-driver").is_some(),
        "staged test-driver 必须可被发现"
    );
    for driver in ["s7", "test-driver"] {
        let conn = if driver == "s7" {
            serde_json::json!({"host":"127.0.0.1","port":102})
        } else {
            serde_json::json!({})
        };
        // test-driver 走 staged 目录；s7 走正式 drivers/。
        let (app, ep_id) = if driver == "test-driver" {
            app_with_endpoint_in_dir(&root, driver, conn).await
        } else {
            app_with_endpoint(driver, conn).await
        };
        let req = Request::builder()
            .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"parent":"","limit":5}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // S7/test-driver 不支持 browse，应返回 503 或 400；且 body 不得是
        // DRIVER_UNAVAILABLE（binary 缺失的假绿出口，前置 find_driver 已先拦一道）。
        assert!(
            resp.status() == StatusCode::SERVICE_UNAVAILABLE
                || resp.status() == StatusCode::BAD_REQUEST,
            "driver {driver} browse should be unsupported, got {}",
            resp.status()
        );
        if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
            let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
                .await
                .unwrap();
            let text = String::from_utf8_lossy(&body);
            assert!(
                !text.contains("DRIVER_UNAVAILABLE"),
                "driver {driver} browse 503 必须是不支持语义，不得是驱动缺失：{text}"
            );
        }
    }
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn browse_pagination_does_not_return_all_at_once() {
    let _guard = BROWSE_SERIAL.lock().await;
    let (app, ep_id) = app_with_endpoint("opcua", fake_opcua_connection()).await;
    // 请求 limit 1，应只返回 1 且有 next_cursor
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"","limit":1}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let v = assert_browse_ok(resp, "opcua limit1").await;
    assert_eq!(v["nodes"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn browse_sinumerik_nck_catalog_tree_e2e() {
    // 原生寻址重构后：browse 只有两类入口——可选模板预填 与 只读身份核对。
    // 子进程 E2E：驱动被发现、二进制可拉起、browse 200。
    // `840d-sl` 暂无模板且未取得身份探针证据，因此根 = 模板目录 + 明确诊断
    // （不得伪装成空目录，也不得套用 828D 探针）。
    // 注意：改过驱动代码后须先 cargo build --workspace（旧二进制静默失效）。
    let _guard = BROWSE_SERIAL.lock().await;
    let drivers_dir = common::repo_root().join("drivers");
    let mgr = mesa_driver_manager::MesaManager::discover(&drivers_dir);
    assert!(
        mgr.find_driver("sinumerik-nck").is_some(),
        "sinumerik-nck 必须可被发现"
    );
    assert!(
        mgr.find_driver("sinumerik").is_none(),
        "旧 sinumerik 必须零残留"
    );
    let (app, ep_id) = app_with_endpoint(
        "sinumerik-nck",
        serde_json::json!({"host":"127.0.0.1","family":"840d-sl","local_tsap":256,"remote_tsap":258}),
    )
    .await;
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"","limit":10}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let v = assert_browse_ok(resp, "sinumerik-nck").await;
    let nodes = v["nodes"].as_array().expect("nodes 数组");
    assert_eq!(nodes.len(), 2, "模板入口 + 系列提示，实际: {v}");
    assert_eq!(nodes[0]["id"], "nck://presets");
    assert_eq!(
        nodes[1]["kind"], "diagnostic",
        "无探针系列必须明确提示身份浏览待验证"
    );
    // 根目录不访问控制器；未知 parent 同样空页（不伪造内容）。
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"nck://unknown","limit":10}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let v = assert_browse_ok(resp, "sinumerik-nck").await;
    assert!(
        v["nodes"].as_array().expect("nodes 数组").is_empty(),
        "未知 parent 必须为空页，实际: {v}"
    );

    // ---- 【PR-B 新增，不改写上面的既有断言】-------------------------------
    // 828d 的预填模板叶必须产出**语义载荷**：`{"kind":"nck_variable",
    // "parameters":{variable_id,definition_revision,target,arguments}}`，
    // 且**不含**任何原生协议字段（语义模式下用户不面对 `module`/`column`/…）。
    // 既有的 `840d-sl` 断言（根 2 节点、诊断节点、未知 parent 空页）保持不变：
    // 该系列没有模板，正是上面那三条断言成立的原因。
    let (app828, ep828) = app_with_endpoint(
        "sinumerik-nck",
        serde_json::json!({"host":"127.0.0.1","family":"828d","local_tsap":256,"remote_tsap":258}),
    )
    .await;
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep828}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"nck://presets","limit":1}"#))
        .unwrap();
    let resp = app828.oneshot(req).await.unwrap();
    let v = assert_browse_ok(resp, "sinumerik-nck 828d presets").await;
    let leaf = &v["nodes"].as_array().expect("nodes 数组")[0];
    assert_eq!(leaf["kind"], "nck_variable", "叶必须是语义载荷: {leaf}");
    let binding: serde_json::Value =
        serde_json::from_str(leaf["binding_json"].as_str().unwrap()).unwrap();
    assert_eq!(binding["kind"], "nck_variable", "{binding}");
    assert!(
        binding.get("resource_id").is_none(),
        "旧资源信封不得再出现: {binding}"
    );
    let params = binding["parameters"].as_object().expect("parameters 对象");
    for key in params.keys() {
        assert!(
            ["variable_id", "definition_revision", "target", "arguments"].contains(&key.as_str()),
            "语义载荷不得含 `{key}`: {binding}"
        );
    }
    assert!(
        params["variable_id"].as_str().unwrap().contains('.'),
        "variable_id 必须是定义目录里的语义 ID: {binding}"
    );
    assert_eq!(params["definition_revision"], 1);
    for key in params["target"].as_object().unwrap().keys() {
        assert!(
            ["channel", "mode_group", "axis", "spindle", "drive_object"].contains(&key.as_str()),
            "target 只允许语义变体，实际 `{key}`: {binding}"
        );
    }
    // 身份块旁挂在**浏览载荷**里（模板/键集合/预览 Key），供前端做通用替换。
    assert!(
        binding["identity"]["template"]
            .as_str()
            .unwrap()
            .contains('{'),
        "必须给出身份模板: {binding}"
    );
    assert!(
        !binding["identity"]["keys"].as_array().unwrap().is_empty(),
        "必须给出身份键集合: {binding}"
    );
}

// ---------------------------------------------------------------------------
// IPC 1.8 Browse 选择契约：资格判定归驱动，Core 只透传
// ---------------------------------------------------------------------------

/// 选择模式取值（与 `proto/driver.proto` 的 `BrowseSelectionMode` 逐字对应）。
///
/// 这里刻意用数值断言：Core 透传的就是数值，前端解引用同一组数值。若哪天
/// 有人调整枚举顺序，本文件会连同 `protocol_negotiation.rs` 的枚举冻结一起红。
const MODE_UNSPECIFIED: i64 = 0;
const MODE_RESOURCE: i64 = 1;
const MODE_SEMANTIC: i64 = 2;
const MODE_NOT_SELECTABLE: i64 = 3;

/// NCK 语义 Browse：**可选性、载荷、诊断全部由驱动给出**，前端不再推断。
///
/// 关键判据（对应 Workstream B 的验收项）：
/// ① 分支节点 = NOT_SELECTABLE 且无理由（不该出现红色阻断）；
/// ② `scope=identity` 的叶由驱动判为 NOT_SELECTABLE，码与 Configure 拒绝码
///    同源（`DEFINITION_SCOPE_NOT_COLLECTABLE`）——这正是"资格判定搬到
///    driver/catalog 拥有者"的直接证据；
/// ③ `scope=collect` 的叶 = SEMANTIC，载荷与 `binding_json` 逐字一致，
///    诊断只承载展示性提示（INFO/WARNING，不改变可选性）。
#[tokio::test]
async fn browse_selection_contract_is_owned_by_driver() {
    let _guard = BROWSE_SERIAL.lock().await;
    let (app, ep) = app_with_endpoint(
        "sinumerik-nck",
        serde_json::json!({"host":"127.0.0.1","family":"828d","local_tsap":256,"remote_tsap":258}),
    )
    .await;

    // ① 根：模板入口 + 身份入口（828d 有探针）都是不可选分支
    let root = browse_json(&app, &ep, r#"{"parent":"","limit":10}"#).await;
    for node in root["nodes"].as_array().unwrap() {
        assert_eq!(
            node["selection_mode"].as_i64().unwrap(),
            MODE_NOT_SELECTABLE,
            "分支节点必须由驱动声明不可选: {node}"
        );
        assert_eq!(
            node["diagnostics"].as_array().unwrap().len(),
            0,
            "分支节点不应带红色阻断理由: {node}"
        );
    }

    // ② 模板列表：逐条核对「驱动给出的模式」与「定义目录的 scope」一致
    let presets = browse_json(&app, &ep, r#"{"parent":"nck://presets","limit":50}"#).await;
    let leaves = presets["nodes"].as_array().unwrap();
    assert!(!leaves.is_empty(), "828d 应有模板叶");
    let identity_leaf = leaves
        .iter()
        .find(|n| n["id"].as_str().unwrap().contains("MDCA_CTRLOUT_TYPE"))
        .expect("identity 探针模板必须出现（默认可见）");
    assert_eq!(
        identity_leaf["selection_mode"].as_i64().unwrap(),
        MODE_NOT_SELECTABLE,
        "scope=identity 必须由驱动判为不可选: {identity_leaf}"
    );
    let diag = &identity_leaf["diagnostics"].as_array().unwrap()[0];
    assert_eq!(
        diag["code"], "DEFINITION_SCOPE_NOT_COLLECTABLE",
        "诊断码必须与 Configure 拒绝码同源: {identity_leaf}"
    );
    assert_eq!(diag["severity"].as_i64().unwrap(), 3, "拒绝必须是 ERROR 级");
    assert!(
        diag["message"].as_str().unwrap().contains("scope="),
        "诊断文案必须说清是哪条准入规则: {diag}"
    );

    let collect_leaf = leaves
        .iter()
        .find(|n| {
            n["id"]
                .as_str()
                .unwrap()
                .ends_with("828d.B.S.opMode.nc_variable")
        })
        .expect("可采集模板 opMode 必须出现");
    assert_eq!(
        collect_leaf["selection_mode"].as_i64().unwrap(),
        MODE_SEMANTIC,
        "scope=collect 的语义叶必须声明 SEMANTIC: {collect_leaf}"
    );
    assert_eq!(
        collect_leaf["selection_payload_json"], collect_leaf["binding_json"],
        "选择载荷与 binding_json 必须同内容（旧客户端继续读 binding_json）"
    );
    let notes = collect_leaf["diagnostics"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "可选叶只应有一条展示性提示: {collect_leaf}");
    assert!(
        ["IDENTITY_PREVIEW", "IDENTITY_UNRESOLVED"].contains(&notes[0]["code"].as_str().unwrap()),
        "提示码必须是驱动定义的展示级诊断: {notes:?}"
    );
    assert!(
        notes[0]["severity"].as_i64().unwrap() <= 2,
        "提示级诊断不得是 ERROR（那会把它变成阻断）: {notes:?}"
    );

    // ③ 语义载荷本身仍是驱动私有的：Core 原样搬运，不解析、不改写
    let payload = collect_leaf["selection_payload_json"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
    assert_eq!(parsed["kind"], "nck_variable");
    assert_eq!(parsed["parameters"]["definition_revision"], 1);
}

/// OPC UA：通用资源叶由驱动声明 **RESOURCE**，载荷就是 canonical selection
///（`resource_id` + `parameters`），通用前端无需任何语义层即可回填。
#[tokio::test]
async fn browse_opcua_declares_resource_selection_mode() {
    let _guard = BROWSE_SERIAL.lock().await;
    let (app, ep) = app_with_endpoint("opcua", fake_opcua_connection()).await;
    let v = browse_json(&app, &ep, r#"{"parent":"","limit":5}"#).await;
    let nodes = v["nodes"].as_array().unwrap();
    assert!(!nodes.is_empty(), "OPC UA Fake 应能浏览出节点: {v}");
    for node in nodes {
        assert_eq!(
            node["selection_mode"].as_i64().unwrap(),
            MODE_RESOURCE,
            "OPC UA 的浏览叶是通用资源叶: {node}"
        );
        assert_ne!(
            node["selection_mode"].as_i64().unwrap(),
            MODE_UNSPECIFIED,
            "in-tree 驱动必须显式声明选择契约，不得留 UNSPECIFIED: {node}"
        );
        let payload: serde_json::Value =
            serde_json::from_str(node["selection_payload_json"].as_str().unwrap()).unwrap();
        assert_eq!(payload["resource_id"], "node", "资源 id 必须显式给出");
        assert!(
            payload["parameters"]["node_id"].as_str().is_some(),
            "资源参数必须显式给出: {payload}"
        );
        assert_eq!(
            node["diagnostics"].as_array().unwrap().len(),
            0,
            "可选叶不应有阻断诊断: {node}"
        );
        // 旧客户端仍读 binding_json：两条通路必须同源（node_id 一致）。
        let legacy: serde_json::Value =
            serde_json::from_str(node["binding_json"].as_str().unwrap()).unwrap();
        assert_eq!(legacy["node_id"], payload["parameters"]["node_id"]);
    }
}

/// Core 的 Browse 透传层**不含任何品牌/协议判断**（AGENTS.md「Core 不懂协议」）。
///
/// 源码级断言：`browse_nodes_json` 只做字段搬运，一旦有人在那里加 `if` 分支，
/// 必然要写出厂商字面量或选择模式的字符串名，本断言就会红。
#[test]
fn core_browse_passthrough_has_no_vendor_literals() {
    let path = common::repo_root()
        .join("crates")
        .join("core-api")
        .join("src")
        .join("lib.rs");
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // 只看透传函数本体，避免把契约测试的注释算进来。
    let start = src
        .find("fn browse_nodes_json")
        .expect("core-api 必须提供 browse_nodes_json 透传函数");
    let body = &src[start..];
    let end = body
        .find("\nasync fn browse_endpoint")
        .unwrap_or(body.len());
    let body = &body[..end];
    for forbidden in [
        "sinumerik",
        "nck_variable",
        "nck_native",
        "focas2",
        "opcua",
        "BROWSE_SELECTION_MODE_",
        "semantic",
        "not_selectable",
    ] {
        assert!(
            !body.contains(forbidden),
            "Core 的 Browse 透传层不得出现 `{forbidden}`（Core 不懂协议）"
        );
    }
}

/// Browse 请求 → JSON（本文件内的薄封装，避免每处重写 Request 构造）。
async fn browse_json(app: &axum::Router, ep_id: &str, body: &str) -> serde_json::Value {
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_browse_ok(resp, "browse").await
}
