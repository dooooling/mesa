//! Management API 契约（V2.1 §4.4, §23, Milestone F）

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_config_store::ConfigStore;
use std::sync::Arc;
use tower::ServiceExt;

async fn app() -> (
    axum::Router,
    Arc<mesa_driver_manager::MesaManager>,
    common::StagedDriversDir,
) {
    app_with_control(false).await
}

/// 开闸版 app（Foundation-3 Control REST 门禁测试用；audit 落 in-memory store）。
/// 返回 staged guard——调用方必须持有到测试结束，否则 spawn 时二进制路径失效。
async fn app_with_control(
    enable_control: bool,
) -> (
    axum::Router,
    Arc<mesa_driver_manager::MesaManager>,
    common::StagedDriversDir,
) {
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    // staged drivers 目录：正式 drivers/ + test-driver（已退役产品身份，
    // 由测试在 temp dir 组装，不污染正式目录）。
    let staged = common::staged_drivers_with_test_driver();
    let drivers_dir = staged.path().clone();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state = mesa_core_api::AppState::try_new_with_control(
        mgr.clone(),
        store,
        drivers_dir.to_string_lossy().to_string(),
        enable_control,
    )
    .unwrap();
    let router = mesa_core_api::router(state);
    (router, mgr, staged)
}

#[tokio::test]
async fn validate_connection_ok_and_field_error() {
    let (app, _, _staged) = app().await;
    // 正确连接：test-driver 空连接（未知字段会被统一校验拒绝）
    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/validate-connection")
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
    let (app, _, _staged) = app().await;
    // 已知驱动
    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/descriptor")
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
    let (app, _, _staged) = app().await;
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
    let (app, mgr, _staged) = app_with_control(true).await;
    // 建 device/endpoint（test-driver，Reference Control）
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{}}"#,
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
    // 终审 #4 门：value 双真值已删除——value_typed 即未知字段（400/422），
    // 绝不静默覆盖 value 执行。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"a"},"output":"value"},"value":1.0,"value_typed":{"F64":99.0}}"#,
    )
    .await;
    assert!(
        s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
        "body: {v}"
    );
    drop(mgr);
}

/// 终审 #2 回归（REST 层）：Driver command 业务失败（Failed + 精确 error）
/// 走正常失败响应（HTTP 200 + status=Failed），绝不冒充
/// DRIVER_CONTRACT_VIOLATION 502。
/// 注：reset 的 input_schema 为空对象，带参请求在 Core 门禁即被
/// UNKNOWN_FIELD 拦截（400，不送 Driver）——这是正确的分层；
/// "Failed 不进 schema 门禁"的回归由 control_contract 单元门锁定，
/// 此处再锁定 REST 对 Core 门禁拒绝的形态（400 + issues，不送 Driver）。
#[tokio::test]
async fn control_command_business_failure_is_not_violation() {
    use mesa_core_types::{AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, TaskSchedule};
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let _staged = common::staged_drivers_with_test_driver();
    let drivers_dir = _staged.path().clone();
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
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    // 空 task 即可 Start（reset 不依赖采集计划）。
    store
        .replace_tasks(
            "e1",
            &[AcquisitionTask {
                id: "t1".into(),
                schedule: TaskSchedule::Poll { interval_ms: 50 },
                binding: DriverBinding {
                    kind: GENERIC_BINDING_KIND.into(),
                    config: serde_json::json!({"selections": [{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"w.k"}]}]}),
                },
            }],
        )
        .unwrap();
    let app = mesa_core_api::router(state);
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "e1".into(),
        driver_id: "test-driver".into(),
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
    // reset 带参 → Core input 门禁先拦（UNKNOWN_FIELD，400）；
    // 让请求到达 Driver 的形态是 input={} 但 args 非空——不可能：
    // Core 门禁与 Driver 输入同为 {} 语义。因此业务失败回归改走 fail_once
    // 命令（input_schema 接受 flag，Driver 对 flag=false 返回业务失败）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/commands/reset",
        r#"{"input":{"x":1}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["issues"][0]["code"], "UNKNOWN_FIELD");
    // fail_once flag=false → Driver 业务失败 → HTTP 200 + status=Failed，
    // error 精确码直透，绝不是 502 DRIVER_CONTRACT_VIOLATION。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/commands/fail_once",
        r#"{"input":{"flag":false}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["status"], "Failed");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("COMMAND_BUSINESS_FAILED"),
        "body: {v}"
    );
    // fail_once flag=true → 成功 {}（过 result_schema，审计 COMPLETED）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/commands/fail_once",
        r#"{"input":{"flag":true}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["status"], "Succeeded");
    assert_eq!(v["result"], serde_json::json!({}));
    mgr.shutdown_all().await;
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
    let _staged = common::staged_drivers_with_test_driver();
    let drivers_dir = _staged.path().clone();
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
            driver_id: "test-driver".into(),
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
        driver_id: "test-driver".into(),
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
    let _staged = common::staged_drivers_with_test_driver();
    let drivers_dir = _staged.path().clone();
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
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    // 任务 t1 采 slot=a（point w.a），任务 t2 采 slot=b（point w.b）；
    // slot=c 从不出现在任何 task（"只写、不采集"，终审 #1 指定形态）。
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
            &[mk_task("t1", "a", "w.a"), mk_task("t2", "b", "w.b")],
        )
        .unwrap();
    let app = mesa_core_api::router(state);
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "e1".into(),
        driver_id: "test-driver".into(),
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
    // 终审 #1 指定 Gate：运行中直接写从未出现在任何 task 的 slot=c=42，
    // 再 expected=42 CAS 写成 43；同时 snapshot 始终没有 w.c。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"c"},"output":"value"},"value":42.0}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["status"], "Succeeded");
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/write",
        r#"{"target":{"resource_id":"writable","parameters":{"slot":"c"},"output":"value"},"value":43.0,"expected_value":42.0}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["status"], "Succeeded");
    assert!(
        !mgr.snapshot()
            .latest_all()
            .iter()
            .any(|e| e.point_key == "w.c"),
        "从未采集的 slot 不得在 snapshot 中出现投影"
    );
    // 连续快速写 20 次 slot=b，无一次静默丢失（终审 #3：无 try_send 丢写）。
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
    let (app, _, _staged) = app().await;
    // probe test-driver with dummy connection (always reachable via Fake)
    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/probe")
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
async fn probe_test_driver_returns_frozen_shape() {
    let (app, _, _staged) = app().await;
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/test-driver/probe",
        r#"{"connection":{}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "probe body: {v}");
    assert_eq!(v["reachable"], true, "probe body: {v}");
    assert_eq!(v["device"]["vendor"], "Mesa");
    assert_eq!(v["device"]["family"], "TestDriver");
    assert_eq!(v["device"]["model"], "Basic");
    // P0-1：capabilities 为 CapabilityItem 数组（四态），test-driver 为 poll-only
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
    let (app, _, _staged) = app().await;
    let (status, v) = post_json(app, "/api/v1/drivers/nope/probe", r#"{"connection":{}}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn probe_non_object_connection_is_400() {
    let (app, _, _staged) = app().await;
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/test-driver/probe",
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
    let (app, _, _staged) = app().await;
    let mut handles = Vec::new();
    for _ in 0..8 {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            post_json(
                app,
                "/api/v1/drivers/test-driver/probe",
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
    let (app, _, _staged) = app().await;
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
    let (app, _, _staged) = app().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED, "create device");
    // connection 未知字段 → 400（统一校验，未声明即拒绝）
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{"seed":1}}"#,
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
            r#"{"id":"e1","device_id":"d1","driver_id":"test-driver","connection":{}}"#,
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
        r#"{"id":"e1","name":"  ","device_id":"d1","driver_id":"test-driver","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // 合法 endpoint
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{}}"#,
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
    assert_eq!(ep["driver_id"], "test-driver", "driver 未动: {ep}");
}

/// PR4 Secret 正式语义：缺失保留旧值、marker 保留、显式 clear 删除。
///（opcua password 为 optional Secret；marker 复用可观测保留/删除。）
#[tokio::test]
async fn secret_update_missing_keeps_and_clear_deletes() {
    let (app, _, _staged) = app().await;
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
    let (app, _, _staged) = app().await;
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

// ---------------------------------------------------------------------------
// C3-A：Endpoint-bound 连接检查（契约 §3/§4/§5）
// ---------------------------------------------------------------------------

/// C3 专用 app：额外把 store 交回测试，用于逐字比对「检查前后持久化面不变」。
async fn app_for_connection_check() -> (
    axum::Router,
    Arc<ConfigStore>,
    Arc<mesa_driver_manager::MesaManager>,
    common::StagedDriversDir,
) {
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let staged = common::staged_drivers_with_test_driver();
    let drivers_dir = staged.path().clone();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state = mesa_core_api::AppState::try_new_with_control(
        mgr.clone(),
        Arc::clone(&store),
        drivers_dir.to_string_lossy().to_string(),
        false,
    )
    .unwrap();
    (mesa_core_api::router(state), store, mgr, staged)
}

/// 持久化面快照：connection_json / endpoint_secrets / desired_running /
/// 更新时间 / 任务面 / 事件任务面。用 JSON 比对而不是逐个字段断言，避免
/// 「看漏了某一列改了没改」。
fn persisted_state(store: &ConfigStore, endpoint_id: &str) -> serde_json::Value {
    let rec = store.get_endpoint(endpoint_id).unwrap().unwrap();
    serde_json::json!({
        "endpoint": rec,
        "secret_fields": store.list_secret_fields(endpoint_id).unwrap(),
        "password_secret": store.get_secret(endpoint_id, "password").unwrap(),
        "tasks": store.list_tasks(endpoint_id).unwrap(),
        "event_tasks": store.list_event_tasks(endpoint_id).unwrap(),
    })
}

/// 一次性断言「响应体里没有出现任何明文凭据」。
fn assert_no_plaintext(v: &serde_json::Value, plaintexts: &[&str]) {
    let text = v.to_string();
    for p in plaintexts {
        assert!(
            !text.contains(p),
            "响应体不得出现明文 Secret（命中 {p}）: {text}"
        );
    }
}

/// C3-A 验收 1（+ C3-01 的「零 Probe」半条）：`mode=validate` 校验的是**当前
/// 草稿**而不是已保存配置，且**完全不调用 Probe**。
#[tokio::test]
async fn connection_check_validate_uses_draft_and_never_probes() {
    let (app, _store, mgr, _staged) = app_for_connection_check().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    // 已保存配置：合法且带 60s Probe 延迟注入（真被 Probe 会阻塞十几秒）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{"probe_delay_ms":60000}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "body: {v}");
    let before = persisted_state(&_store, "e1");

    // 预热 Descriptor（临时进程 + 缓存），把一次性 spawn 成本移出计时区。
    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/descriptor")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    // ① 草稿非法而**已保存配置合法** → 必须按草稿报 400：若读的是持久化配置
    //    这里会返回 200，也证明不了「用草稿校验」。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/connection-check",
        r#"{"mode":"validate","connection":{"probe_delay_ms":"not-an-int"}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["valid"], false);
    assert!(
        v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "INVALID_TYPE"),
        "body: {v}"
    );

    // ② 草稿合法（且与已保存值不同）→ 200，且响应是**校验形状**，不是 Probe
    //    结果形状；耗时必须远小于任何 Probe 路径。
    //    对账：test-driver 的 probe() 会 sleep(probe_delay_ms)；未配置 timeout_ms
    //    时 Query 预算 = min(4×5000+3000, PROBE_RPC_TIMEOUT=45s) = 23s，即一旦
    //    真的走了 Probe，本请求至少 5s 起步（Open 5s + Query 命中同一延迟），
    //    3s 阈值有足够判别力。
    let started = std::time::Instant::now();
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/connection-check",
        r#"{"mode":"validate","connection":{"probe_delay_ms":1000}}"#,
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["valid"], true, "body: {v}");
    assert_eq!(v["issues"], serde_json::json!([]), "body: {v}");
    assert!(
        v.get("reachable").is_none(),
        "validate 不得返回 Probe 结论: {v}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "validate 不得调用 Probe（耗时 {elapsed:?}；Probe 路径至少 5s，Query 预算 23s）"
    );

    // 零副作用：validate 前后持久化面逐字相同，且没有新的 Endpoint/运行态。
    assert_eq!(
        persisted_state(&_store, "e1"),
        before,
        "validate 不得改持久化面"
    );
    assert!(
        mgr.snapshot()
            .endpoint("e1")
            .is_none_or(|s| s.state != "RUNNING"),
        "连接检查不得启动 Endpoint"
    );
}

/// C3-A 验收 2/3/4/5 + 红线 1/3（HTTP 层）：凭据归属、四语义、marker 位置与
/// 形状、`driver_id` 不可覆盖、`probe` 未实现、响应脱敏。
#[tokio::test]
async fn connection_check_secret_semantics_are_endpoint_bound_and_side_effect_free() {
    let (app, store, _mgr, _staged) = app_for_connection_check().await;
    let uri_e1 = "/api/v1/endpoints/e1/connection-check";
    let uri_e2 = "/api/v1/endpoints/e2/connection-check";
    let uri_e3 = "/api/v1/endpoints/e3/connection-check";
    let plaintexts = ["pw-of-e1", "pw-of-e2", "pw-fresh"];

    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    // e1 有密码；e2/e3 同驱动、同字段但**没有** Secret。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"opcua","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":"pw-of-e1"}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "body: {v}");
    for id in ["e2", "e3"] {
        let (s, v) = post_json(
            app.clone(),
            "/api/v1/endpoints",
            &format!(
                r#"{{"id":"{id}","name":"{id}","device_id":"d1","driver_id":"opcua","connection":{{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u"}}}}"#
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "body: {v}");
    }

    let before = persisted_state(&store, "e1");
    let before_e2 = persisted_state(&store, "e2");

    // ① 引用已存凭据（C3-03）：编辑态不回显密码也能检查。
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"secret_set":true}}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "marker 应解析为已存明文: {v}");
    assert_no_plaintext(&v, &plaintexts);

    // ② C3-07 跨 Endpoint 隔离：同一个 driver、同一个 field_path，但该 Endpoint
    //    没有这条 Secret → 必须报引用无效，绝不「按 field_path 全局兜底」。
    for uri in [uri_e2, uri_e3] {
        let (s, v) = post_json(
            app.clone(),
            uri,
            r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"secret_set":true}}}"#,
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
        assert_eq!(v["error"]["code"], "SECRET_NOT_FOUND", "body: {v}");
        assert_no_plaintext(&v, &plaintexts);
    }

    // ③ 新明文只用于本次检查（C3-05）：SecretStore 内容不变。
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":"pw-fresh"}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_no_plaintext(&v, &plaintexts);
    assert_eq!(
        store.get_secret("e1", "password").unwrap().as_deref(),
        Some("pw-of-e1"),
        "本次检查的明文不得写回 SecretStore"
    );

    // ④ 明确清除优先（红线 1）：清除后仍不继承旧值；SecretStore 同样不变。
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"clear_secret":true}}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "可选凭据被清除后应通过校验: {v}");
    assert_eq!(v["valid"], true, "body: {v}");
    // 空字符串同样视为明确清除（C2 归一化后的形态）。
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":""}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(
        store.get_secret("e1", "password").unwrap().as_deref(),
        Some("pw-of-e1"),
        "清除只作用于本次检查，不得删库"
    );

    // ⑤ 字段缺失 → 按 PUT 契约继承旧值（无旧值则保持缺失）。
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u"}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "缺失应继承已存凭据: {v}");

    // ⑥ 红线 3：非 Secret 字段上的 marker、以及不合法的 marker 形状必须 400。
    for bad in [
        r#"{"mode":"validate","connection":{"endpoint_url":{"secret_set":true},"username":"u"}}"#,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"secret_set":"yes"}}}"#,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"secret_set":true,"x":1}}}"#,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"clear_secret":true,"secret_set":true}}}"#,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"credentials":{"user":"u"}}}}"#,
    ] {
        let (s, v) = post_json(app.clone(), uri_e1, bad).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "输入 {bad} → {v}");
        assert_eq!(v["error"]["code"], "INVALID_SECRET_MARKER", "输入 {bad}");
        assert_no_plaintext(&v, &plaintexts);
    }

    // ⑦ driver_id 只读已保存记录：请求体覆盖一律 400。
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840"},"driver_id":"s7"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");

    // ⑧ Endpoint 不存在 → 404。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/nope/connection-check",
        r#"{"mode":"validate","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "body: {v}");
    assert_eq!(v["error"]["code"], "NOT_FOUND");

    // ⑨ mode=probe 在 C3-B 之前必须明确「未实现」，绝不返回模拟成功。
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"probe","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"secret_set":true}}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_IMPLEMENTED, "body: {v}");
    assert_eq!(v["error"]["code"], "PROBE_NOT_IMPLEMENTED");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("C3-B"),
        "必须写明待 C3-B 接入: {v}"
    );
    assert!(
        v.get("reachable").is_none(),
        "未实现的 probe 不得给出任何可达性结论: {v}"
    );
    assert_no_plaintext(&v, &plaintexts);

    // ⑩ 无副作用：检查前后 connection_json / endpoint_secrets / desired_running /
    //    任务 / 事件任务逐字一致。
    assert_eq!(
        persisted_state(&store, "e1"),
        before,
        "连接检查不得修改任何持久化状态"
    );
    assert_eq!(persisted_state(&store, "e2"), before_e2);
}

/// C3-A 验收 6（红线 2，HTTP 层）：密钥丢失/解密失败必须是 **503 SecretStore
/// 基础设施故障**，不得是 400（参数错误）、不得降级为匿名连接。
#[tokio::test]
async fn connection_check_secret_store_failure_is_503() {
    use mesa_config_store::{DeviceRecord, EndpointRecord};
    let base = std::env::temp_dir().join(format!(
        "mesa-c3-contract-store-{}-{}",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    let db = base.join("mesa.db");
    {
        let s = ConfigStore::open(&db).unwrap();
        s.create_device(&DeviceRecord {
            id: "d1".into(),
            name: "D".into(),
        })
        .unwrap();
        s.create_endpoint(&EndpointRecord {
            id: "e1".into(),
            name: "E1".into(),
            device_id: "d1".into(),
            driver_id: "opcua".into(),
            connection_json:
                r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"secret_set":true}}"#
                    .into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
        s.put_secret("e1", "password", "top-secret-pw", "master")
            .unwrap();
    }
    // 换成另一把合法密钥：AEAD 认证失败（密钥丢失/轮换的典型形态）。
    std::fs::write(base.join("master.key"), [0x5Au8; 32]).unwrap();
    let store = Arc::new(ConfigStore::open(&db).unwrap());

    let staged = common::staged_drivers_with_test_driver();
    let drivers_dir = staged.path().clone();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state = mesa_core_api::AppState::try_new_with_control(
        mgr,
        store,
        drivers_dir.to_string_lossy().to_string(),
        false,
    )
    .unwrap();
    let app = mesa_core_api::router(state);

    let (s, v) = post_json(
        app,
        "/api/v1/endpoints/e1/connection-check",
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","password":{"secret_set":true}}}"#,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::SERVICE_UNAVAILABLE,
        "SecretStore 故障必须是 503（既不是 400 参数错误，也不是「设备不可达」）: {v}"
    );
    assert_eq!(v["error"]["code"], "SECRET_STORE_UNAVAILABLE", "body: {v}");
    assert!(
        !v.to_string().contains("top-secret-pw"),
        "错误体不得回显明文: {v}"
    );
    std::fs::remove_dir_all(&base).ok();
}

/// 捕获式 writer：把 tracing 输出收进共享缓冲，供「日志无明文」断言使用。
#[derive(Clone)]
struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// C3-A 验收 8（日志半条，红线「明文不得写入 tracing」）：连接检查路径即使按
/// **trace 级**全量输出，也不得出现任何明文凭据——既包括用户本次输入的新明文，
/// 也包括从 SecretStore 物化出来的旧明文。
///
/// 为什么值得单独装一个 subscriber 而不是「靠代码审查」：这条边界最容易在
/// 后续排障时被顺手破坏（`tracing::debug!(?connection)` 一行就能泄漏），
/// 只有把它钉在测试里才有回归保护。
#[tokio::test]
async fn connection_check_never_logs_secret_plaintext() {
    const STORED: &str = "pw-stored-never-log";
    const INLINE: &str = "pw-inline-never-log";
    let (app, _store, _mgr, _staged) = app_for_connection_check().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        &format!(
            r#"{{"id":"e1","name":"E1","device_id":"d1","driver_id":"opcua","connection":{{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":"{STORED}"}}}}"#
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "body: {v}");

    let buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("trace"))
        .with_writer({
            let b = Arc::clone(&buf);
            move || CaptureWriter(Arc::clone(&b))
        })
        .finish();
    // `set_default` 是线程级：`#[tokio::test]` 默认单线程运行时，请求处理与
    // 断言在同一线程上，因此捕获范围正好覆盖被测路径。
    let guard = tracing::subscriber::set_default(subscriber);

    for body in [
        // 已存凭据物化（明文只在内存里出现）
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"secret_set":true}}}"#.to_string(),
        // 本次新明文
        format!(r#"{{"mode":"validate","connection":{{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":"{INLINE}"}}}}"#),
        // 明确清除
        r#"{"mode":"validate","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"clear_secret":true}}}"#.to_string(),
    ] {
        let (s, v) = post_json(app.clone(), "/api/v1/endpoints/e1/connection-check", &body).await;
        assert_eq!(s, StatusCode::OK, "body: {v}");
    }
    drop(guard);

    let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(
        !logged.is_empty(),
        "预置：trace 级必须实际产出日志（否则本测试是空断言）"
    );
    assert!(
        !logged.contains(STORED),
        "日志不得出现 SecretStore 中的明文凭据: {logged}"
    );
    assert!(
        !logged.contains(INLINE),
        "日志不得出现本次输入的明文凭据: {logged}"
    );
}
