//! Management API 契约（V2.1 §4.4, §23, Milestone F）

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_config_store::ConfigStore;
use std::sync::Arc;
use tower::ServiceExt;

async fn app() -> (axum::Router, Arc<mesa_driver_manager::MesaManager>) {
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let drivers_dir = common::repo_root().join("drivers");
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    #[allow(deprecated)]
    let state = mesa_core_api::AppState::new(
        mgr.clone(),
        store,
        drivers_dir.to_string_lossy().to_string(),
    );
    let router = mesa_core_api::router(state);
    (router, mgr)
}

#[tokio::test]
async fn validate_connection_ok_and_field_error() {
    let (app, _) = app().await;
    // 正确连接：simulator seed
    let req = Request::builder()
        .uri("/api/v1/drivers/simulator/validate-connection")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"connection":{"seed":1}}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 错误：类型错误（host 应为字符串，传数字）
    let req2 = Request::builder()
        .uri("/api/v1/drivers/s7/validate-connection")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"connection":{"host":123,"port":102}}"#))
        .unwrap();
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    let status2 = resp2.status();
    let body_bytes = axum::body::to_bytes(resp2.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(status2, StatusCode::BAD_REQUEST);
    assert_eq!(v["valid"], false);
    assert!(!v["issues"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn descriptor_and_unknown_driver() {
    let (app, _) = app().await;
    // 已知驱动
    let req = Request::builder()
        .uri("/api/v1/drivers/simulator/descriptor")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 未知驱动
    let req2 = Request::builder()
        .uri("/api/v1/drivers/unknown/descriptor")
        .body(Body::empty())
        .unwrap();
    let resp2 = app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::SERVICE_UNAVAILABLE);
}

async fn post_json(app: axum::Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn probe_does_not_create_endpoint() {
    let (app, _) = app().await;
    // probe simulator with dummy connection (simulator always reachable via Fake)
    let req = Request::builder()
        .uri("/api/v1/drivers/simulator/probe")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"connection":{"seed":1}}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // probe 必须 reachable，且不创建 Endpoint（endpoint 列表为空）
    assert_eq!(resp.status(), StatusCode::OK);
}

/// §8 REST 冻结形状：device/capabilities/profile_hints/warnings。
#[tokio::test]
async fn probe_simulator_returns_frozen_shape() {
    let (app, _) = app().await;
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/simulator/probe",
        r#"{"connection":{}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["reachable"], true);
    assert_eq!(v["device"]["vendor"], "Mesa");
    assert_eq!(v["device"]["family"], "Simulator");
    assert_eq!(v["device"]["model"], "Basic");
    assert_eq!(v["capabilities"]["read"], true);
    assert_eq!(v["capabilities"]["subscribe"], true);
    assert_eq!(v["capabilities"]["browse"], true);
    assert!(v["warnings"].as_array().unwrap().is_empty());
    let hints = v["profile_hints"].as_array().unwrap();
    assert!(
        hints.iter().any(|h| h["profile_id"] == "simulator-basic"),
        "hints 必须含 simulator-basic，实际: {hints:?}"
    );
}

#[tokio::test]
async fn probe_unknown_driver_is_404() {
    let (app, _) = app().await;
    let (status, v) = post_json(app, "/api/v1/drivers/nope/probe", r#"{"connection":{}}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn probe_non_object_connection_is_400() {
    let (app, _) = app().await;
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/simulator/probe",
        r#"{"connection":42}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "VALIDATION_ERROR");
}

/// 设备不可达是 200 + reachable:false（不是 5xx）：s7 连关闭端口。
#[tokio::test]
async fn probe_s7_closed_port_is_unreachable_200() {
    let (app, _) = app().await;
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/s7/probe",
        r#"{"connection":{"host":"127.0.0.1","port":9}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["reachable"], false);
    // driver_id 规则仍满足，驱动级 hint 照常给出（无 probe 事实可证伪它）
    let hints = v["profile_hints"].as_array().unwrap();
    assert!(
        hints.iter().any(|h| h["profile_id"] == "s7-1200"),
        "实际: {hints:?}"
    );
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0]["code"], "CONNECTION_FAILED");
}
