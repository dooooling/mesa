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
            device_id: "d1".into(),
            driver_id: driver_id.into(),
            connection_json: serde_json::to_string(&connection).unwrap(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let drivers_dir = common::repo_root().join("drivers");
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    #[allow(deprecated)]
    let state = mesa_core_api::AppState::new(mgr, store, drivers_dir.to_string_lossy().to_string());
    let router = mesa_core_api::router(state);
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
    for driver in ["s7", "simulator"] {
        let conn = if driver == "s7" {
            serde_json::json!({"host":"127.0.0.1","port":102})
        } else {
            serde_json::json!({})
        };
        let (app, ep_id) = app_with_endpoint(driver, conn).await;
        let req = Request::builder()
            .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"parent":"","limit":5}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // S7/Simulator 不支持 browse，应返回 503 或 400
        assert!(
            resp.status() == StatusCode::SERVICE_UNAVAILABLE
                || resp.status() == StatusCode::BAD_REQUEST,
            "driver {driver} browse should be unsupported, got {}",
            resp.status()
        );
    }
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
    // ADR 0001 Commit E：`sinumerik-nck` browse 是 Catalog 虚拟树（纯函数，
    // 无需会话）——子进程 E2E：驱动被发现、二进制可拉起、browse 200。
    // 随仓 catalog 为空（真机回填前），根即空页；身份形态为 `nck://` canonical。
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
    let resp = app.oneshot(req).await.unwrap();
    let v = assert_browse_ok(resp, "sinumerik-nck").await;
    let nodes = v["nodes"].as_array().expect("nodes 数组");
    // 空 catalog → 空根（不伪造内容）；未知 parent 同样空页。
    assert!(nodes.is_empty(), "空 catalog 根必须为空，实际: {v}");
}
