//! Discovery / Browse 契约（V2.1 §20, Milestone H）

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_config_store::ConfigStore;
use std::sync::Arc;
use tower::ServiceExt;

async fn app_with_endpoint(driver_id: &str, connection: serde_json::Value) -> (axum::Router, String) {
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
            profile: None,
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
    let state = mesa_core_api::AppState::new(mgr, store, drivers_dir.to_string_lossy().to_string());
    let router = mesa_core_api::router(state);
    (router, ep_id)
}

#[tokio::test]
async fn browse_opcua_pagination_and_filter() {
    // OPC UA Fake 支持 browse
    let (app, ep_id) = app_with_endpoint("opcua", serde_json::json!({"endpoint_url":"opc.tcp://127.0.0.1:4840"})).await;
    // 未过滤，limit 2
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"","limit":2}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    if resp.status() != StatusCode::OK {
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        eprintln!("browse opcua failed: {}", String::from_utf8_lossy(&body));
        panic!("browse failed");
    }
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["nodes"].as_array().unwrap().len() <= 2);
    // 若有下一页，next_cursor 非空
    if let Some(next) = v["next_cursor"].as_str() {
        if !next.is_empty() {
            // 拉下一页
            let req2 = Request::builder()
                .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"parent":"","cursor":"{next}","limit":2}}"#)))
                .unwrap();
            let resp2 = app.oneshot(req2).await.unwrap();
            assert_eq!(resp2.status(), StatusCode::OK);
        }
    }
    // 过滤
    let req3 = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"","filter":"Fake","limit":10}"#))
        .unwrap();
    let resp3 = app_with_endpoint("opcua", serde_json::json!({"endpoint_url":"opc.tcp://127.0.0.1:4840"})).await.0.oneshot(req3).await.unwrap();
    assert_eq!(resp3.status(), StatusCode::OK);
}

#[tokio::test]
async fn browse_unsupported_for_s7_and_simulator() {
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
            resp.status() == StatusCode::SERVICE_UNAVAILABLE || resp.status() == StatusCode::BAD_REQUEST,
            "driver {driver} browse should be unsupported, got {}",
            resp.status()
        );
    }
}

#[tokio::test]
async fn browse_pagination_does_not_return_all_at_once() {
    let (app, ep_id) = app_with_endpoint("opcua", serde_json::json!({"endpoint_url":"opc.tcp://127.0.0.1:4840"})).await;
    // 请求 limit 1，应只返回 1 且有 next_cursor
    let req = Request::builder()
        .uri(format!("/api/v1/endpoints/{ep_id}/browse"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"parent":"","limit":1}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["nodes"].as_array().unwrap().len(), 1);
}
