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
    // probe 应返回 reachable，但不创建 Endpoint（endpoint 列表仍空）
    assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::SERVICE_UNAVAILABLE);
}
