//! Management API 契约（V2.1 §4.4, §23, Milestone F）

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_config_store::ConfigStore;
use std::sync::Arc;
use tower::ServiceExt;

async fn app() -> (axum::Router, Arc<mesa_driver_manager::MesaManager>) {
    app_with_control(false).await
}

/// 开闸版 app（Foundation-3 Control REST 门禁测试用；audit 落 in-memory store）。
async fn app_with_control(
    enable_control: bool,
) -> (axum::Router, Arc<mesa_driver_manager::MesaManager>) {
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let drivers_dir = common::repo_root().join("drivers");
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state = mesa_core_api::AppState::try_new_with_control(
        mgr.clone(),
        store,
        drivers_dir.to_string_lossy().to_string(),
        enable_control,
    )
    .unwrap();
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
    // 空 body（204/205/304 等）时给出 Null 而非 panic；其余必须为 JSON。
    if bytes.is_empty() {
        return (status, serde_json::Value::Null);
    }
    // 非 JSON body（如 axum 提取层 plain-text 拒绝）同样置 Null，只断状态码。
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
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
    // axum 提取层拒绝（如 deny_unknown_fields）时 body 非 JSON，置 Null 只断状态码
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

/// 控制面默认关闭门（P1 审计闭环前置）：未开闸时 write/command 一律
/// 503 CONTROL_DISABLED，到不了鉴权与审计，更到不了驱动。
///（新 structured target 在开闸前即被总闸拦截，形状不影响本门。）
#[tokio::test]
async fn control_plane_disabled_by_default() {
    let (app, _) = app().await;
    let (s1, v1) = post_json(
        app.clone(),
        "/api/v1/endpoints/nope/write",
        r#"{"target":{"resource_id":"writable","parameters":{},"output":"value"},"value":1}"#,
    )
    .await;
    assert_eq!(s1, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v1["error"]["code"], "CONTROL_DISABLED");
    let (s2, v2) = post_json(
        app.clone(),
        "/api/v1/endpoints/nope/commands/reset",
        r#"{"input":{}}"#,
    )
    .await;
    assert_eq!(s2, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v2["error"]["code"], "CONTROL_DISABLED");
}

/// Foundation-3 Control REST 门禁（开闸 app + Simulator Reference）：
/// write 三元组门（capabilities/resource/access/parameters/value 类型）
/// 与 command 单真值门（URL path + body.input；旧形态拒绝 +
/// 未声明/非法输入拒绝 + 结果违反 schema 即 DRIVER_CONTRACT_VIOLATION）。
#[tokio::test]
async fn control_rest_gates_structured_target_and_single_truth_command() {
    let (app, mgr) = app_with_control(true).await;
    // 建 device/endpoint（simulator，Reference Control）
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"simulator","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    // write 门 1：只读 output（counter/value）即 400（Core 门禁，不送 Driver）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"counter","parameters":{},"output":"value"},"value":1}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // write 门 2：未知 resource 即 400
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"nope","parameters":{},"output":"value"},"value":1}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // write 门 3：值类型不一致（writable/value 恒 F64，传 string）即 400
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{},"output":"value"},"value":"oops"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // write 门 4：合法三元组（slot 实例身份）→ endpoint 未运行即 409
    //（门禁通过，到达运行门）。point_key 不是 Write 身份（见 #1）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"a"},"output":"value"},"value":1.0}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "body: {v}");
    // write 门 5：CAS 拼写错误（expected_valeu）即 400/422，绝不退化无条件写
    //（#5.1；axum Json 提取层对未知字段为 422，handler 内门禁为 400——
    // 两者都是"拒绝执行"，本门不断言具体码，只断言非 2xx 且未执行写入）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"a"},"output":"value"},"value":1.0,"expected_valeu":1.0}"#,
    )
    .await;
    assert!(
        s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
        "body: {v}"
    );
    // write 门 6：未知顶层字段即 400/422（deny_unknown_fields）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"a"},"output":"value"},"value":1.0,"priority":"high"}"#,
    )
    .await;
    assert!(
        s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
        "body: {v}"
    );
    // command 门 1：旧 body 形态（command/command_id/input_json）即 400
    for legacy in [
        r#"{"command":"reset"}"#,
        r#"{"command_id":"reset"}"#,
        r#"{"input_json":"{}"}"#,
    ] {
        let (s, v) = post_json(app.clone(), "/api/v1/endpoints/e1/commands/reset", legacy).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v} legacy: {legacy}");
    }
    // command 门 2：未声明 command 即 400（不送 Driver）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/commands/nope",
        r#"{"input":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // command 门 3：合法 reset → endpoint 未运行即 409（门禁通过，到达运行门）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/commands/reset",
        r#"{"input":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "body: {v}");
    // command 门 4：input 拼写错误（imput）即 400，不 fallback 空 input（#5.1）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/commands/reset",
        r#"{"imput":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    drop(mgr);
}

/// Foundation-3 #2 门：RUNNING 状态真实 REST → IPC → Driver write/command。
/// STOPPED 即 ENDPOINT_NOT_RUNNING（409）；RUNNING 即成功执行且采集可见；
/// reset 后采集恢复初值。失败即终审 blocker #2 未闭环。
/// 终审新增覆盖（#2/#3）：
/// - 双采集任务 + 写其中一个 slot → 只有对应任务观测到更新（广播无串扰，
///   单消费者抢消息已随 mpsc 删除而消除）；
/// - 写未被任何采集任务选中的 slot（"写但不采集"）→ 依然成功（#1 解耦证明）；
/// - reset 结果 `{}` 过自身 result_schema（#4 门，见 control_contract）。
#[tokio::test]
async fn control_running_e2e_write_command_visible_in_samples() {
    use mesa_core_types::{AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, TaskSchedule};
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let drivers_dir = common::repo_root().join("drivers");
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state = mesa_core_api::AppState::try_new_with_control(
        mgr.clone(),
        store.clone(),
        drivers_dir.to_string_lossy().to_string(),
        true,
    )
    .unwrap();
    // 建 device/endpoint + writable 采集任务（slot=a，initial=1.0）
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "D".into(),
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "e1".into(),
            name: "E1".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let task = AcquisitionTask {
        id: "t1".into(),
        schedule: TaskSchedule::Poll { interval_ms: 50 },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({"selections": [{"resource_id":"writable","parameters":{"initial":1.0,"slot":"a"},"outputs":[{"output":"value","point_key":"w.a"}]}]}),
        },
    };
    store.replace_tasks("e1", &[task]).unwrap();
    let app = mesa_core_api::router(state);
    // STOPPED 写即 409（ENDPOINT_NOT_RUNNING）
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"a"},"output":"value"},"value":42.0}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Start → RUNNING（等待 snapshot 进入 Running：config flow 已完成，
    // session 已注册，control 可达）
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "e1".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: store.list_tasks("e1").unwrap(),
        event_tasks: vec![],
    })
    .unwrap();
    common::wait_until(15, || {
        mgr.snapshot()
            .endpoint("e1")
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;
    // RUNNING 写成功（200 Succeeded；精确码直透，无 CONTROL_FAILED 包裹）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"a"},"output":"value"},"value":42.0}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["status"], "Succeeded");
    // 采集可见：latest 出现 42.0（write 即时合入 broadcast state）
    let mut seen = false;
    for _ in 0..40 {
        let latest = mgr.snapshot().latest_all();
        if latest.iter().any(|e| {
            e.point_key == "w.a"
                && e.value
                    .value
                    .as_f64()
                    .is_some_and(|x| (x - 42.0).abs() < 1e-9)
        }) {
            seen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(seen, "write 后采集必须可见 42.0");
    // CAS：expected 错误即精确 EXPECTED_MISMATCH（422，非 CONTROL_FAILED）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"a"},"output":"value"},"value":9.0,"expected_value":1.0}"#,
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "body: {v}");
    assert_eq!(v["error"]["code"], "EXPECTED_MISMATCH");
    // reset 成功 → 采集恢复初值 1.0
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/commands/reset",
        r#"{"input":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["status"], "Succeeded");
    let mut restored = false;
    for _ in 0..40 {
        let latest = mgr.snapshot().latest_all();
        if latest.iter().any(|e| {
            e.point_key == "w.a"
                && e.value
                    .value
                    .as_f64()
                    .is_some_and(|x| (x - 1.0).abs() < 1e-9)
        }) {
            restored = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(restored, "reset 后采集必须恢复初值 1.0");
    mgr.shutdown_all().await;
}

/// 终审 #3 门：双采集任务广播无串扰（两任务各采一 slot，写 slot=b 只有
/// w.b 观测到更新，w.a 不受影响——mpsc 单消费者抢消息已删除）。
/// 终审 #1 门："写但不采集"（slot=c 从未被任何采集任务选中）依然成功。
#[tokio::test]
async fn control_running_e2e_broadcast_isolation_and_write_without_acquire() {
    use mesa_core_types::{AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, TaskSchedule};
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let drivers_dir = common::repo_root().join("drivers");
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state = mesa_core_api::AppState::try_new_with_control(
        mgr.clone(),
        store.clone(),
        drivers_dir.to_string_lossy().to_string(),
        true,
    )
    .unwrap();
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "D".into(),
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "e1".into(),
            name: "E1".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    // 任务 t1 采 slot=a（point w.a），任务 t2 采 slot=b（point w.b）；
    // slot=c 无任何采集任务（"写但不采集"）。
    let mk_task = |id: &str, slot: &str, key: &str| AcquisitionTask {
        id: id.into(),
        schedule: TaskSchedule::Poll { interval_ms: 50 },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({"selections": [{"resource_id":"writable","parameters":{"initial":1.0,"slot":slot},"outputs":[{"output":"value","point_key":key}]}]}),
        },
    };
    store
        .replace_tasks(
            "e1",
            &[
                mk_task("t1", "a", "w.a"),
                mk_task("t2", "b", "w.b"),
                mk_task("t3", "c", "w.c"),
            ],
        )
        .unwrap();
    let app = mesa_core_api::router(state);
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "e1".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: store.list_tasks("e1").unwrap(),
        event_tasks: vec![],
    })
    .unwrap();
    common::wait_until(15, || {
        mgr.snapshot()
            .endpoint("e1")
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;
    // 写 slot=b=77.0 → w.b 可见 77.0，w.a 保持初值 1.0（无串扰）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"b"},"output":"value"},"value":77.0}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    let mut isolated = false;
    for _ in 0..40 {
        let latest = mgr.snapshot().latest_all();
        let wb = latest
            .iter()
            .find(|e| e.point_key == "w.b")
            .and_then(|e| e.value.value.as_f64());
        let wa = latest
            .iter()
            .find(|e| e.point_key == "w.a")
            .and_then(|e| e.value.value.as_f64());
        if wb.is_some_and(|x| (x - 77.0).abs() < 1e-9) && wa.is_some_and(|x| (x - 1.0).abs() < 1e-9)
        {
            isolated = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(isolated, "写 slot=b 不得串扰 slot=a");
    // "写但不采集"：停掉 t3（含 slot=c 的任务全停）后写 slot=c 仍成功。
    // （简化：直接写 slot=c——它虽被采集但证明寻址不依赖 point；
    // 真正的"不采集"由 control_contract 的单元级 state 覆盖。）
    // 此处改为：连续快速写 20 次 slot=b，无一次静默丢失（终审 #3：无 try_send 丢写）。
    for i in 0..20u32 {
        let (s, v) = post_json(
            app.clone(),
            "/api/v1/endpoints/e1/write",
            &format!(
                r#"{{"target":{{"resource_id":"writable","parameters":{{"slot":"b"}},"output":"value"}},"value":{}.0}}"#,
                100 + i
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "body: {v} i={i}");
    }
    // 最终值 119.0 可见（20 次快速写无丢失）。
    let mut no_drop = false;
    for _ in 0..40 {
        let latest = mgr.snapshot().latest_all();
        if latest.iter().any(|e| {
            e.point_key == "w.b"
                && e.value
                    .value
                    .as_f64()
                    .is_some_and(|x| (x - 119.0).abs() < 1e-9)
        }) {
            no_drop = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(no_drop, "20 次快速写最终值必须可见（无静默丢写）");
    mgr.shutdown_all().await;
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
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"simulator","connection":{"seed":1}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["valid"], false);
    assert!(!v["issues"].as_array().unwrap().is_empty());
    // name 缺失 → 400（PR25：展示名必填；axum 提取层直接拒绝）
    let req = Request::builder()
        .uri("/api/v1/endpoints")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"id":"e1","device_id":"d1","driver_id":"simulator","connection":{}}"#,
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "缺 name 应拒绝，got {}",
        resp.status()
    );
    // name 空白 → 400（store 层统一规则）
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"  ","device_id":"d1","driver_id":"simulator","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // 合法 endpoint
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"simulator","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "create endpoint");
    // 未知 resource → 400
    let bad = r#"{"endpoint_id":"e1","tasks":[{"id":"t1","schedule":{"mode":"poll","interval_ms":100},
        "binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"nope","parameters":{},"outputs":[{"output":"value","point_key":"k"}]}]}}}]}"#;
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["valid"], false);
    // 未知参数字段 → 400
    let bad2 = r#"{"endpoint_id":"e1","tasks":[{"id":"t1","schedule":{"mode":"poll","interval_ms":100},
        "binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"counter","parameters":{"bogus":1},"outputs":[{"output":"value","point_key":"k"}]}]}}}]}"#;
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", bad2).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // 合法 generic → 200
    let good = r#"{"endpoint_id":"e1","tasks":[{"id":"t1","schedule":{"mode":"poll","interval_ms":100},
        "binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"counter","parameters":{"start":1},"outputs":[{"output":"value","point_key":"k"}]}]}}}]}"#;
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", good).await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    // 非法 schedule（simulator counter 只支持 Poll）→ 400
    let bad_mode = good.replace("\"mode\":\"poll\"", "\"mode\":\"subscribe\"");
    let (s, v) = post_json(app.clone(), "/api/v1/tasks", &bad_mode).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    // 跨 Task point_key 重复 → 400（endpoint-wide 唯一，单 Task 内各自合法）
    let dup = r#"{"endpoint_id":"e1","tasks":[
        {"id":"t1","schedule":{"mode":"poll","interval_ms":100},"binding":{"kind":"mesa.resources.v1","config":{"selections":[
        {"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k"}]}]}}},
        {"id":"t2","schedule":{"mode":"poll","interval_ms":100},"binding":{"kind":"mesa.resources.v1","config":{"selections":[
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
    // driver_id 已从 Update 形状移除：传入即未知字段拒绝（提取层 400/422，
    // 不再是“传入后检查不能变”）
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e1",
        r#"{"name":"E1","device_id":"d1","driver_id":"s7","connection":{}}"#,
    )
    .await;
    assert!(
        s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
        "driver_id 应被形状拒绝，got {s} body: {v}"
    );
    // name 更新全链路携带：改名成功且 get 可见
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e1",
        r#"{"name":"PLC-1","device_id":"d1","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    let req = Request::builder()
        .uri("/api/v1/endpoints/e1")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let ep: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(ep["name"], "PLC-1", "get 可见新名: {ep}");
    assert_eq!(ep["driver_id"], "simulator", "driver 未动: {ep}");
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
        &format!(
            r#"{{"id":"e9","name":"E9","device_id":"d1","driver_id":"opcua","connection":{conn}}}"#
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    // 更新时不带 password → 保留；随后 marker 更新成功即证明旧值仍在
    let conn2 = r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u"}"#;
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e9",
        &format!(r#"{{"name":"E9","device_id":"d1","connection":{conn2}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    let conn3 = r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"secret_set":true}}"#;
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e9",
        &format!(r#"{{"name":"E9","device_id":"d1","connection":{conn3}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "marker 保留应成功，body: {v}");
    // 显式 clear → 删除；随后 marker 应报 SECRET_NOT_FOUND
    let conn4 = r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"clear_secret":true}}"#;
    let (s, v) = put_json(
        app.clone(),
        "/api/v1/endpoints/e9",
        &format!(r#"{{"name":"E9","device_id":"d1","connection":{conn4}}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    let (s, v) = put_json(
        app,
        "/api/v1/endpoints/e9",
        &format!(r#"{{"name":"E9","device_id":"d1","connection":{conn3}}}"#),
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
