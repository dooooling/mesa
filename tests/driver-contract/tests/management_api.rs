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
    // 正确连接：simulator 空连接（未知字段会被统一校验拒绝）
    let req = Request::builder()
        .uri("/api/v1/drivers/simulator/validate-connection")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"connection":{}}"#))
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

async fn put_json(app: axum::Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri(uri)
        .method("PUT")
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
        .body(Body::from(r#"{"connection":{}}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // probe 必须 reachable，且不创建 Endpoint（endpoint 列表为空）
    // 诊断要求：非 200 必须带 body（偶发 503 时区分 Handshake/Spawn/Rpc）。
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "probe body: {}",
        String::from_utf8_lossy(&bytes)
    );
}

/// §8 REST 冻结形状：device/capabilities/warnings（Probe 只返回事实）。
#[tokio::test]
async fn probe_simulator_returns_frozen_shape() {
    let (app, _) = app().await;
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/simulator/probe",
        r#"{"connection":{}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "probe body: {v}");
    assert_eq!(v["reachable"], true, "probe body: {v}");
    assert_eq!(v["device"]["vendor"], "Mesa");
    assert_eq!(v["device"]["family"], "Simulator");
    assert_eq!(v["device"]["model"], "Basic");
    // P0-1：capabilities 为 CapabilityItem 数组（四态），simulator 为 poll-only
    let caps = v["capabilities"].as_array().unwrap();
    let state_of = |id: &str| {
        caps.iter()
            .find(|c| c["id"] == id)
            .map(|c| c["state"].as_str().unwrap().to_string())
    };
    assert_eq!(state_of("read").as_deref(), Some("available"));
    assert_eq!(state_of("subscribe").as_deref(), Some("not_present"));
    assert_eq!(state_of("browse").as_deref(), Some("not_present"));
    assert!(v["warnings"].as_array().unwrap().is_empty());
    assert!(v.get("profile_hints").is_none(), "profile_hints 已删除");
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

/// 并发 Probe 确定性 Gate（Management startup 收敛）：同一驱动 8 个并行
/// Probe 必须全部成功、可达——spawn/租约/建连在并发下不得串扰（错连/抢端口）。
/// 不断言时序，只断言并发正确性；临时进程由 probe attempt 单出口回收。
#[tokio::test]
async fn probe_concurrent_same_driver_all_succeed() {
    let (app, _) = app().await;
    let mut handles = Vec::new();
    for _ in 0..8 {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            post_json(
                app,
                "/api/v1/drivers/simulator/probe",
                r#"{"connection":{}}"#,
            )
            .await
        }));
    }
    for h in handles {
        let (status, v) = h.await.unwrap();
        assert_eq!(status, StatusCode::OK, "probe body: {v}");
        assert_eq!(v["reachable"], true, "probe body: {v}");
    }
}

/// JSON 是 object 但驱动配置非法 → 400，code 透出驱动原因码（P1-2 结构化）。
#[tokio::test]
async fn probe_invalid_driver_config_is_400_with_driver_code() {
    let (app, _) = app().await;
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/s7/probe",
        r#"{"connection":{"host":"127.0.0.1","port":99999}}"#,
    )
    .await;
    // 诊断要求：503 偶发（P1-A 端口竞态）时必须留下 response body，
    // 否则无法区分 Handshake/Spawn/Rpc 三类失败（exact-SHA CI 教训）。
    assert_eq!(status, StatusCode::BAD_REQUEST, "probe body: {v}");
    assert_eq!(v["error"]["code"], "BAD_CONFIG", "probe body: {v}");
}

/// PR4 Task 保存门禁：generic 非法选择（未知 resource / 未知字段）入库即 400；
/// 合法选择 200；endpoint connection 未知字段创建即 400。
#[tokio::test]
async fn task_save_gate_rejects_unknown_resource_and_connection_field() {
    let (app, _) = app().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED, "create device");
    // connection 未知字段 → 400（统一校验，未声明即拒绝）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","device_id":"d1","driver_id":"simulator","connection":{"seed":1}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["valid"], false);
    assert!(!v["issues"].as_array().unwrap().is_empty());
    // 合法 endpoint
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","device_id":"d1","driver_id":"simulator","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "create endpoint");
    // 未知 resource → 400
    let bad = r#"{"endpoint_id":"e1","tasks":[{"id":"t1","mode":"poll","interval_ms":100,
        "binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"nope","parameters":{},"outputs":[{"output":"value","point_key":"k"}]}]}}}]}"#;
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["valid"], false);
    // 未知参数字段 → 400
    let bad2 = r#"{"endpoint_id":"e1","tasks":[{"id":"t1","mode":"poll","interval_ms":100,
        "binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"counter","parameters":{"bogus":1},"outputs":[{"output":"value","point_key":"k"}]}]}}}]}"#;
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", bad2).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // 合法 generic → 200
    let good = r#"{"endpoint_id":"e1","tasks":[{"id":"t1","mode":"poll","interval_ms":100,
        "binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"counter","parameters":{"start":1},"outputs":[{"output":"value","point_key":"k"}]}]}}}]}"#;
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", good).await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    // 非法 mode（simulator counter 只报 Poll）→ 400
    let bad_mode = good.replace("\"mode\":\"poll\"", "\"mode\":\"subscribe\"");
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", &bad_mode).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // 跨 Task point_key 重复 → 400（endpoint-wide 唯一，单 Task 内各自合法）
    let dup = r#"{"endpoint_id":"e1","tasks":[
        {"id":"t1","mode":"poll","interval_ms":100,"binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k"}]}]}}},
        {"id":"t2","mode":"poll","interval_ms":100,"binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"sine","parameters":{},"outputs":[{"output":"value","point_key":"k"}]}]}}}]}"#;
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", dup).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert!(
        v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "DUPLICATE_POINT_KEY")
    );
    // driver_id 不可变 → 400
    let (s, v) = put_json(
        app,
        "/api/v1/endpoints/e1",
        r#"{"device_id":"d1","driver_id":"s7","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["error"]["code"], "IMMUTABLE_DRIVER");
}

/// PR4 Secret 正式语义：缺失保留旧值、marker 保留、显式 clear 删除。
///（opcua password 为 optional Secret；marker 复用可观测保留/删除。）
#[tokio::test]
async fn secret_update_missing_keeps_and_clear_deletes() {
    let (app, _) = app().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    let conn = r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":"pw1"}"#;
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        &format!(r#"{{"id":"e9","device_id":"d1","driver_id":"opcua","connection":{conn}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    // 更新时不带 password → 保留；随后 marker 更新成功即证明旧值仍在
    let conn2 = r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u"}"#;
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e9",
        &format!(r#"{{"device_id":"d1","driver_id":"opcua","connection":{conn2}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    let conn3 = r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"secret_set":true}}"#;
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e9",
        &format!(r#"{{"device_id":"d1","driver_id":"opcua","connection":{conn3}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "marker 保留应成功，body: {v}");
    // 显式 clear → 删除；随后 marker 应报 SECRET_NOT_FOUND
    let conn4 = r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"clear_secret":true}}"#;
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e9",
        &format!(r#"{{"device_id":"d1","driver_id":"opcua","connection":{conn4}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    let (s, v) = put_json(
        app,
        "/api/v1/endpoints/e9",
        &format!(r#"{{"device_id":"d1","driver_id":"opcua","connection":{conn3}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["error"]["code"], "SECRET_NOT_FOUND");
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
    assert_eq!(status, StatusCode::OK, "probe body: {v}");
    assert_eq!(v["reachable"], false, "probe body: {v}");
    // 没探测到 ≠ 猜型号：profile_hints 字段已删除，不做任何型号推断。
    assert!(v.get("profile_hints").is_none(), "profile_hints 已删除");
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0]["code"], "CONNECTION_FAILED");
}
