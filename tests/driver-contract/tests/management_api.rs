//! Management API 契约（V2.1 §4.4, §23, Milestone F）

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_config_store::ConfigStore;
use std::sync::Arc;
use tower::ServiceExt;

/// 同进程的两个装配也不能共享证书；Router 的存活期必须覆盖证书目录的存活期。
#[tokio::test]
async fn isolated_rest_fixtures_keep_certificates_until_router_drop() {
    let staged = common::staged_drivers_with_test_driver();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(staged.path()));
    let make_state = || {
        common::isolated_app_state(
            Arc::clone(&mgr),
            Arc::new(ConfigStore::open_in_memory().unwrap()),
            staged.path().to_string_lossy().to_string(),
            false,
        )
        .unwrap()
    };
    let first = make_state();
    let second = make_state();
    let first_dir = first.cert_dir().to_path_buf();
    assert_ne!(first_dir, second.cert_dir());
    for state in [&first, &second] {
        assert!(state.cert_dir().join("own/own.der").is_file());
        assert!(state.cert_dir().join("own/own.key").is_file());
    }
    let router = first.router();
    drop(first);
    assert!(first_dir.exists(), "Router 仍在使用证书目录");
    drop(router);
    assert!(!first_dir.exists(), "最后一个 Router 释放后必须清理目录");
}

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
    let state = common::isolated_app_state(
        mgr.clone(),
        store,
        drivers_dir.to_string_lossy().to_string(),
        enable_control,
    )
    .unwrap();
    let router = state.router();
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
    let state = common::isolated_app_state(
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
    let app = state.router();
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
    let state = common::isolated_app_state(
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
    let app = state.router();
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
    let state = common::isolated_app_state(
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
    let app = state.router();
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

/// PR-A（NCK 语义点位）：Core 门禁对 `nck.points.v1` 的**信封层**判定。
///
/// 覆盖设计文档 §6.1 的 Core 门禁单测与 §6.6 的 T1（Core 侧）/T2，并同时证明
/// `mesa.resources.v1` 路径**逐字不变**（P2 护栏）。
/// 载荷统一用 `serde_json::json!` 构造，避免手写 JSON 的括号计数错误。
#[tokio::test]
async fn task_save_gate_nck_points_envelope() {
    let (app, _, _staged) = app().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED, "create device");
    let (s, _) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "create endpoint");

    /// 以 e1 为 endpoint 保存任务集（`tasks` 为 JSON 数组）。
    async fn save(app: axum::Router, tasks: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let body = serde_json::json!({ "endpoint_id": "e1", "tasks": tasks }).to_string();
        post_json(app, "/api/v1/tasks", &body).await
    }
    /// `nck.points.v1` 任务（`points` 为点位数组）。
    fn nck_task(id: &str, points: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "schedule": { "mode": "poll", "interval_ms": 100 },
            "binding": { "kind": "nck.points.v1", "config": { "points": points } }
        })
    }
    /// `mesa.resources.v1` 任务（`selections` 为选择数组）。
    fn generic_task(id: &str, selections: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "schedule": { "mode": "poll", "interval_ms": 100 },
            "binding": { "kind": "mesa.resources.v1", "config": { "selections": selections } }
        })
    }
    fn has_code(v: &serde_json::Value, code: &str) -> bool {
        v["issues"]
            .as_array()
            .map(|a| a.iter().any(|i| i["code"] == code))
            .unwrap_or(false)
    }

    // ① T1（Core 侧）：一个 nck.points.v1 任务**同时**含 nck_variable 与 nck_native
    //    ⇒ 放行（不因 Point Kind 拒绝、不要求拆成两个任务）。
    let mixed = nck_task(
        "t1",
        serde_json::json!([
            {
                "kind": "nck_variable",
                "point_key": "channel.r_parameter#index=i:100@channel=i:1",
                "parameters": {
                    "variable_id": "channel.r_parameter",
                    "definition_revision": 1,
                    "target": { "channel": 1 },
                    "arguments": { "index": 100 }
                }
            },
            {
                "kind": "nck_native",
                "point_key": "native.C.1.21.1.396",
                "parameters": {
                    "area": "C", "area_no": 1, "module": 21, "column": 1, "line": 396,
                    "count": 1, "unit_mode": "current", "decode": "F64_LE",
                    "transport_size": "0x09"
                }
            }
        ]),
    );
    let (s, v) = save(app.clone(), serde_json::json!([mixed])).await;
    assert_eq!(s, StatusCode::OK, "混合 Point Kind 必须放行: {v}");

    // ② 未知 **Point Kind** 一律放行（Core 不建立白名单，P9）。
    let weird = nck_task(
        "t1",
        serde_json::json!([{
            "kind": "weird_kind",
            "point_key": "weird.thing@whatever=i:1",
            "parameters": { "unknown": 1 }
        }]),
    );
    let (s, v) = save(app.clone(), serde_json::json!([weird])).await;
    assert_eq!(s, StatusCode::OK, "Core 不得枚举 Point Kind: {v}");

    // ③ 信封层未知字段 ⇒ UNKNOWN_FIELD（不得静默丢弃）。
    let unknown_field = nck_task(
        "t1",
        serde_json::json!([{
            "kind": "nck_variable",
            "point_key": "a.b",
            "parameters": {},
            "identity": { "template": "x" }
        }]),
    );
    let (s, v) = save(app.clone(), serde_json::json!([unknown_field])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(has_code(&v, "UNKNOWN_FIELD"), "{v}");

    // ④ parameters 非对象 ⇒ INVALID_PARAMETERS。
    let bad_params = nck_task(
        "t1",
        serde_json::json!([{
            "kind": "nck_variable",
            "point_key": "a.b",
            "parameters": [1, 2]
        }]),
    );
    let (s, v) = save(app.clone(), serde_json::json!([bad_params])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(has_code(&v, "INVALID_PARAMETERS"), "{v}");

    // ⑤ kind 为空 ⇒ EMPTY_POINT_KIND。
    let empty_kind = nck_task(
        "t1",
        serde_json::json!([{ "kind": "", "point_key": "a.b", "parameters": {} }]),
    );
    let (s, v) = save(app.clone(), serde_json::json!([empty_kind])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(has_code(&v, "EMPTY_POINT_KIND"), "{v}");

    // ⑤b point_key 为空 ⇒ EMPTY_POINT_KEY。
    let empty_key = nck_task(
        "t1",
        serde_json::json!([{ "kind": "nck_variable", "point_key": "", "parameters": {} }]),
    );
    let (s, v) = save(app.clone(), serde_json::json!([empty_key])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(has_code(&v, "EMPTY_POINT_KEY"), "{v}");

    // ⑥ point_key 通用格式非法 ⇒ INVALID_POINT_KEY（仅通用语法，不判身份）。
    for bad_key in [
        "channel.r_parameter#index=1@channel=i:1", // 值段缺类型标签
        "channel.r_parameter#b=i:1,a=i:2@channel=i:1", // 键名未升序
        "native.C.1.21.1.396~1",                   // 后缀 n < 2
        "not a key",                               // 非键字符
    ] {
        let task = nck_task(
            "t1",
            serde_json::json!([{
                "kind": "nck_variable",
                "point_key": bad_key,
                "parameters": {}
            }]),
        );
        let (s, v) = save(app.clone(), serde_json::json!([task])).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "`{bad_key}` 应被拒: {v}");
        assert!(has_code(&v, "INVALID_POINT_KEY"), "`{bad_key}`: {v}");
    }

    // ⑦ 格式合法但**身份错误**的 Key 必须放行（Core 不判定身份正确性，修订 4）。
    let identity_wrong = nck_task(
        "t1",
        serde_json::json!([{
            "kind": "nck_variable",
            "point_key": "native.C.1.21.1.396",
            "parameters": {
                "variable_id": "channel.program.status",
                "definition_revision": 99,
                "target": { "channel": 1 },
                "arguments": {}
            }
        }]),
    );
    let (s, v) = save(app.clone(), serde_json::json!([identity_wrong])).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "格式合法但身份错误必须放行（由 Driver 以 POINT_KEY_MISMATCH 拒绝）: {v}"
    );

    // ⑧ T2：**跨任务**重复 point_key ⇒ DUPLICATE_POINT_KEY（Endpoint-wide）。
    let dup_cross_task = serde_json::json!([
        nck_task(
            "t1",
            serde_json::json!([{ "kind": "nck_variable", "point_key": "dup.key@channel=i:1", "parameters": {} }])
        ),
        nck_task(
            "t2",
            serde_json::json!([{ "kind": "nck_native", "point_key": "dup.key@channel=i:1", "parameters": {} }])
        )
    ]);
    let (s, v) = save(app.clone(), dup_cross_task).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(has_code(&v, "DUPLICATE_POINT_KEY"), "{v}");

    // ⑨ 跨 Task Binding Kind 的重复 ⇒ 同样拒绝（两种 kind 共用一个命名空间）。
    let dup_cross_kind = serde_json::json!([
        generic_task(
            "t1",
            serde_json::json!([{
                "resource_id": "counter",
                "parameters": {},
                "outputs": [{ "output": "value", "point_key": "cross.kind" }]
            }])
        ),
        nck_task(
            "t2",
            serde_json::json!([{ "kind": "nck_variable", "point_key": "cross.kind", "parameters": {} }])
        )
    ]);
    let (s, v) = save(app.clone(), dup_cross_kind).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(has_code(&v, "DUPLICATE_POINT_KEY"), "{v}");

    // ⑩ 未知 **Task Binding Kind** ⇒ UNSUPPORTED_BINDING（路由仍严格，不静默放行）。
    let bad_kind = serde_json::json!([{
        "id": "t1",
        "schedule": { "mode": "poll", "interval_ms": 100 },
        "binding": { "kind": "mesa.points.v2", "config": { "points": [] } }
    }]);
    let (s, v) = save(app.clone(), bad_kind).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(has_code(&v, "UNSUPPORTED_BINDING"), "{v}");

    // ⑪ `mesa.resources.v1` 行为**零变化**：合法 200、未知 resource 仍 400。
    let generic_ok = serde_json::json!([generic_task(
        "t1",
        serde_json::json!([{
            "resource_id": "counter",
            "parameters": { "start": 1 },
            "outputs": [{ "output": "value", "point_key": "generic.k" }]
        }])
    )]);
    let (s, v) = save(app.clone(), generic_ok.clone()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let generic_bad = serde_json::json!([generic_task(
        "t1",
        serde_json::json!([{
            "resource_id": "nope",
            "parameters": {},
            "outputs": [{ "output": "value", "point_key": "generic.k" }]
        }])
    )]);
    let (s, v) = save(app.clone(), generic_bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(has_code(&v, "UNKNOWN_RESOURCE"), "{v}");

    // ⑫ 清空（tasks=[]）直接放行，不碰 Descriptor。
    let (s, v) = save(app.clone(), serde_json::json!([])).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

/// PR-B3（P0）端到端：`scope=identity` 的语义点位在**真实驱动子进程**链路上
/// 必须 Configure 失败 ⇒ 端点 `FAILED`、detail 含新码与三要素、不进入重连、
/// 不分配任何 point_id（无部分计划）。
///
/// 不需要真机：`open_connection` 只解析配置（`lib.rs:355-378`），
/// Configure 在建连之前就按 scope 拒绝。
#[tokio::test]
async fn nck_configure_rejects_identity_scope_at_start() {
    const EID: &str = "nck-b3-identity";
    let (app, mgr, _staged) = app().await;

    let (s, v) = post_json(
        app.clone(),
        "/api/v1/devices",
        r#"{"id":"nck-b3-dev","name":"NCK B3 Device"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "create device: {v}");

    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        &serde_json::json!({
            "id": EID,
            "name": "NCK B3 Identity",
            "device_id": "nck-b3-dev",
            "driver_id": "sinumerik-nck",
            "connection": {
                "host": "127.0.0.1",
                "family": "828d",
                "local_tsap": 256,
                "remote_tsap": 258,
            },
        })
        .to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "create endpoint: {v}");

    // Core 保存门禁**不下沉** scope（本 PR 不扩大 Core 职责）：identity 任务保存成功。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/tasks",
        &serde_json::json!({
            "endpoint_id": EID,
            "tasks": [{
                "id": "t-identity",
                "schedule": { "mode": "poll", "interval_ms": 100 },
                "binding": {
                    "kind": "nck.points.v1",
                    "config": { "points": [{
                        "kind": "nck_variable",
                        "point_key": "axis.name@axis=i:1",
                        "parameters": {
                            "variable_id": "axis.name",
                            "definition_revision": 1,
                            "target": { "axis": 1 },
                            "arguments": {},
                        },
                    }]},
                },
            }],
        })
        .to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "Core 保存门禁只校验信封: {v}");

    let (s, v) = post_json(app.clone(), &format!("/api/v1/endpoints/{EID}/start"), "{}").await;
    assert_eq!(
        s,
        StatusCode::OK,
        "start 先返回 200（拒绝经 /state 暴露）: {v}"
    );

    common::wait_until(20, || {
        mgr.snapshot()
            .endpoint(EID)
            .is_some_and(|s| s.state == "FAILED")
    })
    .await;

    let (s, v) = get_json(app.clone(), &format!("/api/v1/endpoints/{EID}/state")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["state"], "FAILED", "配置失败终态必须是 FAILED: {v}");
    let detail = v["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("ConfigurationError/DEFINITION_SCOPE_NOT_COLLECTABLE"),
        "detail 必须带新码: {detail}"
    );
    assert!(
        detail.contains("t-identity"),
        "detail 必须含 Task ID: {detail}"
    );
    assert!(
        detail.contains("axis.name"),
        "detail 必须含 Variable ID: {detail}"
    );
    assert!(
        detail.contains("revision=1"),
        "detail 必须含 Revision: {detail}"
    );

    // 不进入持续重连循环：配置失败是终态，等待后仍必须是 FAILED
    //（若误走 Lost，run_endpoint 会置 RECONNECTING 并反复重试）。
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let (_, v) = get_json(app.clone(), &format!("/api/v1/endpoints/{EID}/state")).await;
    assert_eq!(v["state"], "FAILED", "配置失败不得进入重连: {v}");
    assert!(
        !mgr.is_running(EID),
        "配置失败后 run 任务必须结束（无重连、无旧计划可被 Start）"
    );

    // `desired_running` 保持 Start 时置的 true（本 PR 不改变生命周期语义）。
    let (s, ep) = get_json(app.clone(), &format!("/api/v1/endpoints/{EID}")).await;
    assert_eq!(s, StatusCode::OK, "{ep}");
    assert_eq!(ep["desired_running"], true, "{ep}");
    // 无部分计划：失败批次没有 descriptors ⇒ 运行态点数恒为 0（未分配 point_id）。
    assert_eq!(ep["runtime"]["points"], 0, "失败批次不得注册任何点位: {ep}");
    assert_eq!(ep["runtime"]["state"], "FAILED", "{ep}");
    let (_, latest) = get_json(app.clone(), "/api/v1/points/latest").await;
    assert!(
        !latest["points"]
            .as_array()
            .map(|a| a.iter().any(|p| p["endpoint_id"] == EID))
            .unwrap_or(false),
        "失败端点不得产出数据: {latest}"
    );
}

/// PR-A P2 护栏：bootstrap 创建门禁与 REST 保存门禁**同源**（唯一实现）。
///
/// 设计文档 §1.2 / P2：两处此前各有一份 Task Binding Kind 白名单，必须收敛为
/// 同一函数，否则会出现「保存通过、bootstrap 拒绝」的双真值分叉。本用例证明
/// 同一载荷在两处得到**同一判定**（同状态码 + 同 issue code）。
#[tokio::test]
async fn bootstrap_gate_shares_nck_points_routing() {
    fn bootstrap_body(id: &str, tasks: serde_json::Value) -> String {
        serde_json::json!({
            "device": { "id": format!("{id}-dev"), "name": "D" },
            "endpoint": {
                "id": id,
                "name": "E1",
                "driver_id": "test-driver",
                "connection": {}
            },
            "acquisition": { "tasks": tasks },
            "start": false
        })
        .to_string()
    }
    fn nck_task(id: &str, points: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "schedule": { "mode": "poll", "interval_ms": 100 },
            "binding": { "kind": "nck.points.v1", "config": { "points": points } }
        })
    }

    // ① 合法 nck.points.v1（含 Core 不认识的 Point Kind 与不透明 parameters）⇒ 创建成功。
    let (app1, _, _staged1) = app().await;
    let ok = bootstrap_body(
        "e1",
        serde_json::json!([nck_task(
            "t1",
            serde_json::json!([
                {
                    "kind": "weird_kind",
                    "point_key": "channel.r_parameter#index=i:100@channel=i:1",
                    "parameters": { "opaque": { "any": ["thing"] } }
                },
                {
                    "kind": "nck_native",
                    "point_key": "native.C.1.21.1.396",
                    "parameters": {}
                }
            ])
        )]),
    );
    let (s, v) = post_json(app1.clone(), "/api/v1/device-bootstrap", &ok).await;
    assert_eq!(
        s,
        StatusCode::CREATED,
        "bootstrap 必须与保存门禁同判定: {v}"
    );

    // ② 信封层非法（未知字段）⇒ 与 REST 路径同一 issue code。
    let (app2, _, _staged2) = app().await;
    let bad = bootstrap_body(
        "e2",
        serde_json::json!([nck_task(
            "t1",
            serde_json::json!([{
                "kind": "nck_variable",
                "point_key": "a.b",
                "parameters": {},
                "identity": {}
            }])
        )]),
    );
    let (s, v) = post_json(app2.clone(), "/api/v1/device-bootstrap", &bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["issues"][0]["code"], "UNKNOWN_FIELD", "{v}");
    assert_eq!(
        v["issues"][0]["path"], "acquisition.tasks[0].binding.config.points[0].identity",
        "bootstrap 路径前缀必须与保存门禁同构: {v}"
    );

    // ③ 未知 Task Binding Kind ⇒ bootstrap 也拒绝（白名单唯一，无 bypass）。
    let (app3, _, _staged3) = app().await;
    let bad_kind = bootstrap_body(
        "e3",
        serde_json::json!([{
            "id": "t1",
            "schedule": { "mode": "poll", "interval_ms": 100 },
            "binding": { "kind": "nck.points.v1.legacy", "config": { "points": [] } }
        }]),
    );
    let (s, v) = post_json(app3.clone(), "/api/v1/device-bootstrap", &bad_kind).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["issues"][0]["code"], "UNSUPPORTED_BINDING", "{v}");
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
    let state = common::isolated_app_state(
        mgr.clone(),
        Arc::clone(&store),
        drivers_dir.to_string_lossy().to_string(),
        false,
    )
    .unwrap();
    (state.router(), store, mgr, staged)
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

    // ⑨ mode=probe 已接入真实探测链（C3-B）：证据是它和 validate 一样先过统一
    //    Schema 校验——非法草稿在此报 400 `{valid:false,issues}`，而不再返回旧的
    //    501 `PROBE_NOT_IMPLEMENTED`；且**不发起探测**（耗时远小于任何 Probe 路径）。
    //    真实探测的 200/400/501/503 分层由下方 C3-B 专测覆盖。
    let started = std::time::Instant::now();
    let (s, v) = post_json(
        app.clone(),
        uri_e1,
        r#"{"mode":"probe","connection":{"endpoint_url":"opc.tcp://127.0.0.1:4840","username":"u","password":{"secret_set":true},"timeout_ms":"not-an-int"}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["valid"], false, "body: {v}");
    assert!(
        v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "INVALID_TYPE"),
        "body: {v}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "Schema 不合法时必须不发起探测（实际 {:?}）",
        started.elapsed()
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
    let state =
        common::isolated_app_state(mgr, store, drivers_dir.to_string_lossy().to_string(), false)
            .unwrap();
    let app = state.router();

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

// ---------------------------------------------------------------------------
// C3-B：mode=probe 的真实探测执行链
// （契约 §3 处理链 / §5 错误边界 / §6 验收矩阵 C3-04/07/08/10）
// ---------------------------------------------------------------------------

/// 测试专属临时目录（存放 Driver 侧落盘的探测证据，Drop 时清理）。
struct ProbeTempDir(std::path::PathBuf);

impl ProbeTempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "mesa-c3-probe-{tag}-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn file(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for ProbeTempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// 构造 connection-check 的 `mode=probe` 请求体（路径转义交给 serde_json）。
fn probe_body(connection: serde_json::Value) -> String {
    serde_json::json!({ "mode": "probe", "connection": connection }).to_string()
}

/// C3-04（本轮核心证据）：实际送到临时 Driver 的是**解密后的正确明文**，
/// 而不是 `{"secret_set": true}` marker。
///
/// 证据怎么拿：HTTP 响应被 Core 脱敏（同一次请求里 Driver 把收到的凭据回灌进
/// warning，响应中必须只看到 `[redacted]`），所以明文不可能从响应里证明。唯一的
/// 直接证据是 Driver 夹具自己落盘的 `capture_path`——文件内容是临时 Driver 进程
/// **实际收到的**原始 connection JSON。两条断言同时成立才完整：
/// ① Driver 落盘的是明文（且不含任何 marker）；② 同一份明文没有出现在 HTTP 响应里。
#[tokio::test]
async fn connection_check_probe_sends_plaintext_not_marker() {
    const STORED: &str = "pw-c3-stored-plaintext";
    const INLINE: &str = "pw-c3-inline-plaintext";
    let tmp = ProbeTempDir::new("plaintext");
    let (app, store, mgr, _staged) = app_for_connection_check().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    // 已保存凭据走 SecretStore（connection_json 落盘是 marker 形态）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        &format!(
            r#"{{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{{"password":"{STORED}"}}}}"#
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "body: {v}");
    assert!(
        store
            .get_endpoint("e1")
            .unwrap()
            .unwrap()
            .connection_json
            .contains("secret_set"),
        "预置：落盘必须是 marker（明文只在 SecretStore 的密文里）"
    );

    // 预热 Descriptor（临时进程 + 缓存），把一次性 spawn 成本移出断言区。
    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/descriptor")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    let before = persisted_state(&store, "e1");

    // ① 引用已存凭据 + 让 Driver 把收到的凭据回灌进 warning（验证出口脱敏）。
    let cap_ref = tmp.file("probe-ref.json");
    let body = probe_body(serde_json::json!({
        "password": {"secret_set": true},
        "secret_echo": "warning",
        "capture_path": cap_ref.to_string_lossy(),
    }));
    let (s, v) = post_json(app.clone(), "/api/v1/endpoints/e1/connection-check", &body).await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["reachable"], true, "body: {v}");

    // 出口脱敏：warning 里必须有 [redacted]（说明 Driver 确实回灌了明文，Core 擦掉了）
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "body: {v}");
    assert_eq!(warnings[0]["code"], "SECRET_ECHO", "body: {v}");
    let msg = warnings[0]["message"].as_str().unwrap();
    assert!(
        msg.contains("[redacted]"),
        "Driver 回灌的凭据必须被 Core 擦除: {msg}"
    );
    assert_no_plaintext(&v, &[STORED]);

    // ② 核心证据：临时 Driver 实际收到的 connection 里，password 是**明文**。
    let captured = std::fs::read_to_string(&cap_ref)
        .expect("Driver 必须落盘它实际收到的 connection（否则本断言是空断言）");
    assert!(
        !captured.contains("secret_set") && !captured.contains("clear_secret"),
        "marker 绝不进入 Driver 的 OpenConnection: {captured}"
    );
    let cv: serde_json::Value = serde_json::from_str(&captured).unwrap();
    assert_eq!(
        cv["password"], STORED,
        "送到 Driver 的必须是 SecretStore 里的明文，实际: {captured}"
    );

    // ③ 草稿新明文优先于已存值（同样以 Driver 收到的内容为准）。
    let cap_inline = tmp.file("probe-inline.json");
    let body = probe_body(serde_json::json!({
        "password": INLINE,
        "capture_path": cap_inline.to_string_lossy(),
    }));
    let (s, v) = post_json(app.clone(), "/api/v1/endpoints/e1/connection-check", &body).await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    let inline_captured = std::fs::read_to_string(&cap_inline).unwrap();
    let iv: serde_json::Value = serde_json::from_str(&inline_captured).unwrap();
    assert_eq!(
        iv["password"], INLINE,
        "本次草稿明文必须优先: {inline_captured}"
    );
    assert!(
        !inline_captured.contains(STORED),
        "草稿有新明文时不得再拼上已存凭据: {inline_captured}"
    );

    // ④ 无副作用：探测前后持久化面逐字一致，且没有 Endpoint 被启动。
    assert_eq!(
        persisted_state(&store, "e1"),
        before,
        "probe 不得修改任何持久化状态"
    );
    // SecretStore 里的凭据仍是原值（草稿新明文绝不写回）。
    assert_eq!(
        store.get_secret("e1", "password").unwrap().as_deref(),
        Some(STORED)
    );
    assert!(
        mgr.snapshot()
            .endpoint("e1")
            .is_none_or(|s| s.state != "RUNNING"),
        "连接检查不得启动 Endpoint"
    );
    // 临时探测进程不得残留（详细进程/租约取证见 subprocess_orphan_guard）。
    assert!(
        mgr.snapshot().endpoints().is_empty(),
        "临时探测不得产生任何运行态 Endpoint"
    );
}

/// C3-07（HTTP 层）：同一驱动、同一 Secret 字段的两个 Endpoint **不得交叉读取**
/// 凭据。以「Driver 实际收到的明文」为判据，而不是响应文本（响应已被脱敏）。
#[tokio::test]
async fn connection_check_probe_secret_is_endpoint_bound() {
    const PW1: &str = "pw-e1-isolation";
    const PW2: &str = "pw-e2-isolation";
    let tmp = ProbeTempDir::new("isolation");
    let (app, _store, _mgr, _staged) = app_for_connection_check().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    // e1/e2：同驱动、同 Secret 字段，各存各的凭据；e3：同驱动但没有凭据。
    for (id, pw) in [("e1", Some(PW1)), ("e2", Some(PW2)), ("e3", None)] {
        let conn = match pw {
            Some(pw) => format!(r#"{{"password":"{pw}"}}"#),
            None => "{}".to_string(),
        };
        let (s, v) = post_json(
            app.clone(),
            "/api/v1/endpoints",
            &format!(
                r#"{{"id":"{id}","name":"{id}","device_id":"d1","driver_id":"test-driver","connection":{conn}}}"#
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "body: {v}");
    }

    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/descriptor")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    // 各 Endpoint 使用**不同的** capture 文件：交叉读取会在文件内容里直接暴露。
    for (uri, cap_name, expect, other) in [
        ("/api/v1/endpoints/e1/connection-check", "e1.json", PW1, PW2),
        ("/api/v1/endpoints/e2/connection-check", "e2.json", PW2, PW1),
    ] {
        let cap = tmp.file(cap_name);
        let body = probe_body(serde_json::json!({
            "password": {"secret_set": true},
            "capture_path": cap.to_string_lossy(),
        }));
        let (s, v) = post_json(app.clone(), uri, &body).await;
        assert_eq!(s, StatusCode::OK, "{uri} body: {v}");
        let captured = std::fs::read_to_string(&cap).unwrap();
        let cv: serde_json::Value = serde_json::from_str(&captured).unwrap();
        assert_eq!(
            cv["password"], expect,
            "{uri}: 必须是本 Endpoint 的凭据，实际: {captured}"
        );
        assert!(
            !captured.contains(other),
            "{uri}: 绝不能读到另一个 Endpoint 的凭据: {captured}"
        );
        assert_no_plaintext(&v, &[PW1, PW2]);
    }

    // e3 没有该 Secret：marker 引用无效 → 400，且**不发起探测**
    //（capture 文件不存在即证明临时 Driver 根本没起来）。
    let cap3 = tmp.file("e3-must-not-exist.json");
    let body = probe_body(serde_json::json!({
        "password": {"secret_set": true},
        "capture_path": cap3.to_string_lossy(),
    }));
    let (s, v) = post_json(app.clone(), "/api/v1/endpoints/e3/connection-check", &body).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["error"]["code"], "SECRET_NOT_FOUND", "body: {v}");
    assert!(!cap3.exists(), "凭据引用无效必须在发起探测之前被拒绝");
    assert_no_plaintext(&v, &[PW1, PW2]);
}

/// C3-10（HTTP 分层）：`mode=probe` 与既有 `/drivers/{id}/probe` 共用同一映射
/// （`probe_result_to_response`），因此分层口径必须逐条正确：
/// 400 输入（Schema / Driver ConfigurationError）、404 Endpoint 不存在、
/// 501 Driver 不支持探测、503 探测基础设施失败、200 可达与不可达。
///
/// NOTE（504）：真实超时需要耗满 Driver 侧预算（Query 下限 5s + Close 5s + 启动
/// 与清理预算，单例 ≥ 20s），放进合同套件不划算；超时分类由 `mesa-core-api`
/// 单测在**同一个映射函数**上逐分支锁定（`probe_http_layering_classification`），
/// 再加上既有 A32 用例对真实 RpcTimeout 的回收验证——两者合起来覆盖该分支。
#[tokio::test]
async fn connection_check_probe_http_layering_classification() {
    const PW: &str = "pw-c3-layering";
    let (app, _store, _mgr, _staged) = app_for_connection_check().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        &format!(
            r#"{{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{{"password":"{PW}"}}}}"#
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "body: {v}");
    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/descriptor")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    let uri = "/api/v1/endpoints/e1/connection-check";

    // ① 400（Core Schema 门）：类型非法 → `{valid:false,issues}`，且不发起探测。
    let started = std::time::Instant::now();
    let (s, v) = post_json(
        app.clone(),
        uri,
        &probe_body(serde_json::json!({"probe_delay_ms": "not-an-int"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["valid"], false, "body: {v}");
    assert!(
        v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "INVALID_TYPE"),
        "body: {v}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "Schema 不合法时不得发起探测（实际 {:?}）",
        started.elapsed()
    );

    // ② 200 可达：默认行为。
    let (s, v) = post_json(
        app.clone(),
        uri,
        &probe_body(serde_json::json!({"password": {"secret_set": true}})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["reachable"], true, "body: {v}");
    assert_eq!(v["device"]["vendor"], "Mesa", "body: {v}");
    assert_no_plaintext(&v, &[PW]);

    // ③ 200 不可达：**探测结果**，不是错误（不得是 5xx）。
    let (s, v) = post_json(
        app.clone(),
        uri,
        &probe_body(serde_json::json!({"probe_mode": "unreachable"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["reachable"], false, "body: {v}");
    assert_eq!(v["warnings"][0]["code"], "CONNECTION_FAILED", "body: {v}");

    // ④ 501：Driver 声明不支持探测。
    let (s, v) = post_json(
        app.clone(),
        uri,
        &probe_body(serde_json::json!({"probe_mode": "unsupported"})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_IMPLEMENTED, "body: {v}");
    assert_eq!(v["error"]["code"], "PROBE_UNSUPPORTED", "body: {v}");

    // ⑤ 503：探测基础设施/链路失败（不是参数错误，也不是「设备不可达」）。
    let (s, v) = post_json(
        app.clone(),
        uri,
        &probe_body(serde_json::json!({"probe_mode": "connection_error"})),
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "body: {v}");
    assert_eq!(v["error"]["code"], "PROBE_FAILED", "body: {v}");

    // ⑥ 400（Driver ConfigurationError）：Driver 返回的错误文本里带着收到的凭据
    //    → 必须原样透出**分类**，但凭据必须被 Core 擦除。
    let (s, v) = post_json(
        app.clone(),
        uri,
        &probe_body(serde_json::json!({
            "password": {"secret_set": true},
            "secret_echo": "error",
        })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["error"]["code"], "PROBE_REJECTED_BY_DRIVER", "body: {v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("[redacted]"),
        "Driver 错误文本里的凭据必须被擦除: {v}"
    );
    assert_no_plaintext(&v, &[PW]);

    // ⑦ 404：Endpoint 不存在（URL 是唯一 Endpoint 来源）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/nope/connection-check",
        &probe_body(serde_json::json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "body: {v}");
    assert_eq!(v["error"]["code"], "NOT_FOUND", "body: {v}");
}

/// 评审追加边界：**Probe 返回的错误文本与 warnings 也必须脱敏**，且日志同样不得
/// 出现明文。
///
/// 为什么单独一条：进入 Driver 前不泄漏只解决了半条边界——Driver 会把收到的凭据
/// 写进 warning / 认证失败消息，而这两条路都能回到 HTTP 响应与 Core 日志。
/// 这里让夹具把凭据原样回灌（`secret_echo`），再在 **trace 级**全量日志下断言：
/// 响应体与日志里都只有 `[redacted]`。
#[tokio::test]
async fn connection_check_probe_never_leaks_secret_in_response_or_logs() {
    const STORED: &str = "pw-probe-never-leak";
    let (app, _store, _mgr, _staged) = app_for_connection_check().await;
    let (s, _) = post_json(app.clone(), "/api/v1/devices", r#"{"id":"d1","name":"D"}"#).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        &format!(
            r#"{{"id":"e1","name":"E1","device_id":"d1","driver_id":"test-driver","connection":{{"password":"{STORED}"}}}}"#
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "body: {v}");
    let req = Request::builder()
        .uri("/api/v1/drivers/test-driver/descriptor")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    let buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("trace"))
        .with_writer({
            let b = Arc::clone(&buf);
            move || CaptureWriter(Arc::clone(&b))
        })
        .finish();
    // `set_default` 是线程级：`#[tokio::test]` 默认单线程运行时，请求处理与断言
    // 在同一线程上，因此捕获范围正好覆盖被测路径。
    let guard = tracing::subscriber::set_default(subscriber);

    // ① warning 回灌：200 + 全量 warnings 里只能看到 [redacted]。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/connection-check",
        &probe_body(serde_json::json!({
            "password": {"secret_set": true},
            "secret_echo": "warning",
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {v}");
    assert_eq!(v["warnings"][0]["code"], "SECRET_ECHO", "body: {v}");
    assert_no_plaintext(&v, &[STORED]);
    // 预置：擦除必须真的发生过（否则这条断言在「驱动没回灌」时也会绿）。
    assert!(
        v["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("[redacted]"),
        "body: {v}"
    );

    // ② 错误文本回灌：400 + 驱动原因码透出，凭据被擦除。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/connection-check",
        &probe_body(serde_json::json!({
            "password": {"secret_set": true},
            "secret_echo": "error",
        })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body: {v}");
    assert_eq!(v["error"]["code"], "PROBE_REJECTED_BY_DRIVER", "body: {v}");
    assert_no_plaintext(&v, &[STORED]);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("[redacted]"),
        "body: {v}"
    );
    drop(guard);

    let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(
        !logged.is_empty(),
        "预置：trace 级必须实际产出日志（否则本测试是空断言）"
    );
    assert!(
        !logged.contains(STORED),
        "probe 日志不得出现明文凭据: {logged}"
    );
}

/// 可选用例（任务书 §测试要求）：HTTP 层 `PERSISTED_CONNECTION_INVALID`。
///
/// 用 `rusqlite`（dev-dep）绕过写时校验直接改行，模拟**历史遗留/带外修改**的
/// 损坏 `connection_json`——这正是 C1/C3 在坏 JSON 上 fail-closed 的适用场景。
/// 两半都要成立：需要继承凭据时 503 且不探测；草稿自己给全凭据时损坏记录不得拦下
/// 探测（否则「只在确实需要继承时才解析已保存记录」的设计就退化了）。
#[tokio::test]
async fn connection_check_probe_persisted_connection_invalid_is_503() {
    use mesa_config_store::{DeviceRecord, EndpointRecord};
    let tmp = ProbeTempDir::new("persisted-invalid");
    let db = tmp.file("mesa.db");
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
            driver_id: "test-driver".into(),
            connection_json: r#"{"password":{"secret_set":true}}"#.into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
        s.put_secret("e1", "password", "pw-persisted-broken", "master")
            .unwrap();
    }
    // 带外改行：`{oops` 连合法 JSON 都不是（写时校验存在时不可能落库）。
    {
        let raw = rusqlite::Connection::open(&db).unwrap();
        let n = raw
            .execute(
                "UPDATE endpoints SET connection_json = ?1 WHERE id = 'e1'",
                ["{oops"],
            )
            .unwrap();
        assert_eq!(n, 1, "预置：必须真的改动了那一行");
    }
    let store = Arc::new(ConfigStore::open(&db).unwrap());
    let staged = common::staged_drivers_with_test_driver();
    let drivers_dir = staged.path().clone();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state =
        common::isolated_app_state(mgr, store, drivers_dir.to_string_lossy().to_string(), false)
            .unwrap();
    let app = state.router();

    // ① 草稿省略 password ⇒ 需要继承 ⇒ 已保存记录损坏必须 fail-closed（503）。
    let cap = tmp.file("must-not-exist.json");
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/connection-check",
        &probe_body(serde_json::json!({"capture_path": cap.to_string_lossy()})),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::SERVICE_UNAVAILABLE,
        "需要继承而记录损坏必须 fail-closed: {v}"
    );
    assert_eq!(
        v["error"]["code"], "PERSISTED_CONNECTION_INVALID",
        "body: {v}"
    );
    assert_no_plaintext(&v, &["pw-persisted-broken"]);
    assert!(!cap.exists(), "fail-closed 路径不得发起探测");
    // 损坏行不得被「顺手修好」。
    let raw = rusqlite::Connection::open(&db).unwrap();
    let stored: String = raw
        .query_row(
            "SELECT connection_json FROM endpoints WHERE id='e1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, "{oops", "检查失败也不得写回 connection_json");

    // ② 草稿显式清除凭据 ⇒ 不需要继承 ⇒ 同一份损坏记录**不得**拦下探测。
    let cap2 = tmp.file("cleared.json");
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/e1/connection-check",
        &probe_body(serde_json::json!({
            "password": {"clear_secret": true},
            "capture_path": cap2.to_string_lossy(),
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "不需要继承时损坏记录不得拦下探测: {v}");
    assert_eq!(v["reachable"], true, "body: {v}");
    let captured = std::fs::read_to_string(&cap2).unwrap();
    assert!(
        !captured.contains("password"),
        "明确清除后送到 Driver 的连接不得带该字段: {captured}"
    );
}

// ---------------------------------------------------------------------------
// C3-E：全链路验收
//
// 链路：配置设备 → 零采集点离线保存 → 后续补采集点 → 复用已存凭据做
//       Endpoint-bound 连接检查（mode=probe）→ 手动启动 → 产出数据 → 停止。
//
// 跨层（刻意不用 mock 短路任何一层）：
// - Core API：真实 HTTP 路由（`mesa_core_api::router` + `tower::ServiceExt::oneshot`）；
// - config-store：**文件型** SQLite（真实落盘），既走 Store API 也直查磁盘行；
// - driver-manager：临时探测子进程与常驻采集子进程都是真实 test-driver 进程。
//
// 契约：docs/design/c3-endpoint-connection-check.md §6（C3-01/02/03/04/08）。
// ---------------------------------------------------------------------------

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

/// 链路快照：只取「连接检查 / 启动 / 停止不得修改」的持久化面 + 运行态判据字段。
///
/// 为什么用快照逐字比对而不是逐字段断言：逐字段断言最容易「看漏了某一列」；
/// 快照 + [`c3e_changed_keys`] 能同时给出「红线字段一字未改」与「到底哪一列被
/// 合法改变」两个结论。`updated_at_ns` 一并纳入（不隐藏任何列），但不作为差异
/// 的**必需**项——系统时钟粒度可能让两次写落在同一纳秒。
fn c3e_state(store: &ConfigStore, endpoint_id: &str) -> serde_json::Value {
    let rec = store.get_endpoint(endpoint_id).unwrap().unwrap();
    serde_json::json!({
        "connection_json": rec.connection_json,
        "device_id": rec.device_id,
        "driver_id": rec.driver_id,
        "name": rec.name,
        "desired_running": rec.desired_running,
        "updated_at_ns": rec.updated_at_ns,
        "secret_fields": store.list_secret_fields(endpoint_id).unwrap(),
        "password_secret": store.get_secret(endpoint_id, "password").unwrap(),
        "tasks": store.list_tasks(endpoint_id).unwrap(),
        "event_tasks": store.list_event_tasks(endpoint_id).unwrap(),
        "point_map": store.point_map(endpoint_id).unwrap(),
        "revision": store.current_revision(endpoint_id).unwrap(),
    })
}

/// 两个快照之间发生变化的键（已排序）。
fn c3e_changed_keys(before: &serde_json::Value, after: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in before.as_object().unwrap() {
        if after.get(k.as_str()) != Some(v) {
            out.push(k.clone());
        }
    }
    out.sort();
    out
}

/// 断言两快照的差异**只**出现在 `allowed` 内：既钉住红线字段不变，又不放过任何
/// 意料之外的列（例如 `connection_json` 被顺手重写）。
fn assert_changed_within(
    before: &serde_json::Value,
    after: &serde_json::Value,
    allowed: &[&str],
    ctx: &str,
) {
    let changed = c3e_changed_keys(before, after);
    let unexpected: Vec<&String> = changed
        .iter()
        .filter(|k| !allowed.contains(&k.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "{ctx}: 出现意料之外的持久化变更 {unexpected:?}（允许的合法变化只有 {allowed:?}，实际变化 {changed:?}）"
    );
}

/// 直读磁盘上的 `endpoint_secrets` 行（密文 / nonce / 时间戳）。
///
/// 为什么绕过 ConfigStore 直查：AEAD 每次写入都换新随机 nonce，因此**任何**一次
/// Secret 重写（哪怕明文一模一样）都会让本函数的结果变化——这是「连接检查不得
/// 改动 `endpoint_secrets`」最严格的观测口径；Store API 只能证明解密后的明文相同。
fn c3e_raw_secret_rows(db: &std::path::Path) -> serde_json::Value {
    let raw = rusqlite::Connection::open(db).unwrap();
    let mut stmt = raw
        .prepare(
            "SELECT endpoint_id, field_path, ciphertext, nonce, algorithm, key_id, updated_at_ns
             FROM endpoint_secrets ORDER BY endpoint_id, field_path",
        )
        .unwrap();
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            Ok(serde_json::json!({
                "endpoint_id": r.get::<_, String>(0)?,
                "field_path": r.get::<_, String>(1)?,
                "ciphertext": r.get::<_, Vec<u8>>(2)?,
                "nonce": r.get::<_, Vec<u8>>(3)?,
                "algorithm": r.get::<_, String>(4)?,
                "key_id": r.get::<_, String>(5)?,
                "updated_at_ns": r.get::<_, i64>(6)?,
            }))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    serde_json::Value::Array(rows)
}

/// 该点的最新采集值（只接受 f64；用于证明数值在推进，而不是一次性静态样本）。
fn c3e_latest(mgr: &mesa_driver_manager::MesaManager, point_key: &str) -> Option<f64> {
    mgr.snapshot()
        .latest_all()
        .into_iter()
        .find(|e| e.point_key == point_key)
        .and_then(|e| e.value.value.as_f64())
}

/// 预热 Descriptor（临时进程 + 缓存）：把一次性 spawn 成本移出「保存不探测」的
/// 计时区（既有 C3-A 用例同一手法，否则耗时断言会被 descriptor 冷启动污染）。
async fn warm_descriptor(app: &axum::Router, driver_id: &str) {
    let req = Request::builder()
        .uri(format!("/api/v1/drivers/{driver_id}/descriptor"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK,
        "预置：{driver_id} descriptor 必须可用"
    );
}

/// C3-E 主链路：离线保存 → 补采集点 → 复用已存凭据探测 → 手动启动出数 → 停止。
#[tokio::test]
async fn c3e_full_chain_offline_save_probe_start_data_stop() {
    const PW: &str = "pw-c3e-chain-plaintext";
    const EID: &str = "c3e-e1";
    let tmp = ProbeTempDir::new("c3e-chain");
    let db = tmp.file("mesa.db");
    let start_cap = tmp.file("start-open.json");
    let probe_cap = tmp.file("probe-open.json");
    let running_probe_cap = tmp.file("probe-running-open.json");
    let uri_check = format!("/api/v1/endpoints/{EID}/connection-check");

    // 文件型 ConfigStore：本用例要证明的是**真实持久化**面（含磁盘上的密文），
    // 后面的 raw SQL 断言也直接读这份 DB。
    let store = Arc::new(ConfigStore::open(&db).unwrap());
    let staged = common::staged_drivers_with_test_driver();
    let drivers_dir = staged.path().clone();
    // 与 `apps/mesad/src/main.rs:47-51` **同一条装配**：point_id 由持久版
    // `StorePointIdSource` 分配并落库。`MesaManager::discover()` 用的是内存版
    // 分配器（Contract Test 兼容用），那样 `point_map` 根本不会落盘，「point_id
    // 持久化」也就无从验证——链路验收必须复刻生产装配。
    let mgr = Arc::new(mesa_driver_manager::MesaManager::with_source(
        &drivers_dir,
        Arc::new(mesa_driver_manager::StorePointIdSource::new(Arc::clone(
            &store,
        ))),
    ));
    let state = common::isolated_app_state(
        Arc::clone(&mgr),
        Arc::clone(&store),
        drivers_dir.to_string_lossy().to_string(),
        false,
    )
    .unwrap();
    let app = state.router();
    warm_descriptor(&app, "test-driver").await;

    // === 段 1：零采集点离线保存（C3-02 创建半条 + C3-01 的零 Probe/零 Start） ===
    let boot_started = std::time::Instant::now();
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/device-bootstrap",
        &serde_json::json!({
            "device": { "id": "c3e-d1", "name": "C3E Device" },
            "endpoint": {
                "id": EID,
                "name": "C3E Endpoint",
                "driver_id": "test-driver",
                "connection": {
                    "password": PW,
                    "capture_path": start_cap.to_string_lossy(),
                },
            },
            "acquisition": { "tasks": [] },
            "start": false,
            "idempotency_key": "c3e-chain-1",
        })
        .to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "段1 离线保存: {v}");
    assert_eq!(v["started"], false, "段1 必须不启动: {v}");
    assert_eq!(v["endpoint_id"], EID, "段1: {v}");
    assert!(v["revision"].as_u64().is_some(), "段1 revision: {v}");

    let st1 = c3e_state(&store, EID);
    assert_eq!(st1["tasks"], serde_json::json!([]), "段1 tasks 必须为空");
    assert_eq!(
        st1["point_map"],
        serde_json::json!({}),
        "段1 未启动不得分配 point_id"
    );
    assert_eq!(st1["desired_running"], false, "段1 期望态必须为 false");
    assert_eq!(st1["secret_fields"], serde_json::json!(["password"]));
    assert_eq!(st1["password_secret"], PW, "段1 凭据必须已入 SecretStore");
    // 落盘只有 marker；磁盘行与 Store API 一致，且密文里不得出现明文。
    let conn_json = st1["connection_json"].as_str().unwrap();
    assert!(
        conn_json.contains("\"secret_set\":true"),
        "段1 落盘必须是 marker: {conn_json}"
    );
    assert!(
        !conn_json.contains(PW),
        "段1 connection_json 不得含明文: {conn_json}"
    );
    let (db_conn_json, ct): (String, Vec<u8>) = {
        let raw = rusqlite::Connection::open(&db).unwrap();
        raw.query_row(
            "SELECT e.connection_json, s.ciphertext FROM endpoints e
             JOIN endpoint_secrets s ON s.endpoint_id = e.id
             WHERE e.id = ?1 AND s.field_path = 'password'",
            [EID],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    assert_eq!(db_conn_json, conn_json, "段1 磁盘行必须与 Store API 一致");
    assert!(
        !ct.windows(PW.len()).any(|w| w == PW.as_bytes()),
        "段1 磁盘密文不得包含明文"
    );
    // Endpoint 状态：从未启动 ⇒ 没有运行态对象、不在运行、列表里 runtime 为 null
    //（这正是 UI「非运行 / STOPPED」的判据：`isRunningState(runtime?.state)` 为假）。
    assert!(
        mgr.snapshot().endpoint(EID).is_none(),
        "段1 不得有运行态对象"
    );
    assert!(!mgr.is_running(EID), "段1 不得在运行");
    let (s, v) = get_json(app.clone(), "/api/v1/endpoints").await;
    assert_eq!(s, StatusCode::OK, "段1 列表: {v}");
    let listed = v["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == EID)
        .cloned()
        .unwrap_or_else(|| panic!("段1 endpoint 必须在列表里: {v}"));
    assert!(
        listed["runtime"].is_null(),
        "段1 runtime 必须为空（保持 STOPPED）: {listed}"
    );
    assert_eq!(listed["desired_running"], false, "段1: {listed}");
    // 零 Probe / 零 Start 的结构证据：保存路径若发起过任何 OpenConnection，
    // 夹具（test-driver）必然把收到的连接落盘到 capture_path。
    assert!(
        !start_cap.exists(),
        "段1 保存路径不得接触设备（零 Probe / 零 Start）"
    );
    assert!(
        boot_started.elapsed() < std::time::Duration::from_secs(3),
        "段1 保存不得探测设备（耗时 {:?}；探测路径 Open 预算 5s 起）",
        boot_started.elapsed()
    );

    // === 段 2：后续为该 Endpoint 补采集点（既有任务 API） ===
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/tasks",
        &serde_json::json!({
            "endpoint_id": EID,
            "tasks": [{
                "id": "t1",
                "schedule": { "mode": "poll", "interval_ms": 50 },
                "binding": {
                    "kind": "mesa.resources.v1",
                    "config": { "selections": [{
                        "resource_id": "counter",
                        "parameters": { "start": 0.0, "step": 1.0 },
                        "outputs": [{ "output": "value", "point_key": "c3e.counter" }],
                    }]},
                },
            }],
        })
        .to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "段2 补采集点: {v}");
    let st2 = c3e_state(&store, EID);
    assert_eq!(
        st2["tasks"].as_array().unwrap().len(),
        1,
        "段2 任务必须落库"
    );
    assert_eq!(st2["tasks"][0]["id"], "t1", "段2: {st2}");
    let (s, v) = get_json(app.clone(), &format!("/api/v1/tasks?endpoint={EID}")).await;
    assert_eq!(s, StatusCode::OK, "段2 GET tasks: {v}");
    assert_eq!(v["tasks"], st2["tasks"], "段2 GET 面必须与落库一致");
    // 合法变化只有任务面（附带 revision / updated_at_ns 两个记账列）；
    // **Secret 与 connection_json 一字未改**（这是「补任务不该动凭据」的逐字证据）。
    assert_changed_within(
        &st1,
        &st2,
        &["tasks", "revision", "updated_at_ns"],
        "段2 补采集点",
    );
    assert!(
        st2["revision"].as_u64().unwrap() > st1["revision"].as_u64().unwrap(),
        "段2 revision 必须自增: {} → {}",
        st1["revision"],
        st2["revision"]
    );
    assert_eq!(
        st2["password_secret"], st1["password_secret"],
        "段2 不得改动 Secret"
    );
    assert_eq!(
        st2["point_map"],
        serde_json::json!({}),
        "段2 未启动不得分配 point_id"
    );
    assert!(!start_cap.exists(), "段2 只改配置，仍不得接触设备");

    // === 段 3：复用已存凭据做 Endpoint-bound 连接检查（mode=probe） ===
    let before_probe = c3e_state(&store, EID);
    let raw_secrets_before = c3e_raw_secret_rows(&db);
    // 日志捕获：trace 级全量，断言链路上没有任何明文凭据进入日志。
    let log_buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("trace"))
        .with_writer({
            let b = Arc::clone(&log_buf);
            move || CaptureWriter(Arc::clone(&b))
        })
        .finish();
    // `set_default` 是线程级：`#[tokio::test]` 默认单线程运行时，请求处理与被测
    // 的常驻 Endpoint 任务都在同一线程上，因此捕获范围正好覆盖被测链路。
    let guard = tracing::subscriber::set_default(subscriber);

    // 草稿只带「引用已存凭据」的 marker（= 编辑态不回显密码时的线上形态）。
    let (s, v) = post_json(
        app.clone(),
        &uri_check,
        &probe_body(serde_json::json!({
            "password": { "secret_set": true },
            "capture_path": probe_cap.to_string_lossy(),
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "段3 探测: {v}");
    assert_eq!(v["reachable"], true, "段3 探测必须可达: {v}");
    assert_eq!(v["device"]["vendor"], "Mesa", "段3: {v}");
    assert_no_plaintext(&v, &[PW]);
    // C3-03 / C3-04：临时 Driver **实际收到**的连接里，password 是 SecretStore 明文
    // 且不含任何 marker（响应已被 Core 脱敏，明文不可能从响应里证明）。
    let received: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&probe_cap).expect("临时 Driver 必须落盘它实际收到的连接"),
    )
    .unwrap();
    assert_eq!(
        received["password"], PW,
        "段3 送到临时 Driver 的必须是已存明文: {received}"
    );
    assert!(
        !received.to_string().contains("secret_set"),
        "段3 marker 绝不进入 Driver: {received}"
    );
    // C3-08 零副作用：持久化面（含磁盘 Secret 行）与运行态都必须一字不变。
    assert_eq!(
        c3e_state(&store, EID),
        before_probe,
        "段3 连接检查不得修改任何持久化状态"
    );
    assert_eq!(
        c3e_raw_secret_rows(&db),
        raw_secrets_before,
        "段3 连接检查不得重写 endpoint_secrets（含 nonce）"
    );
    assert!(
        mgr.snapshot().endpoint(EID).is_none(),
        "段3 连接检查不得产生运行态"
    );

    // === 段 4：手动启动（复用已存凭据） ===
    let (s, v) = post_json(app.clone(), &format!("/api/v1/endpoints/{EID}/start"), "{}").await;
    assert_eq!(s, StatusCode::OK, "段4 手动启动: {v}");
    assert_eq!(v["started"], EID, "段4: {v}");
    common::wait_until(15, || {
        mgr.snapshot()
            .endpoint(EID)
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;
    // 常驻 Driver 同样是真实子进程：它的 OpenConnection 也落盘 ⇒ 启动复用的仍是明文。
    let started_recv: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&start_cap).expect("常驻 Driver 的 OpenConnection 必须落盘"),
    )
    .unwrap();
    assert_eq!(
        started_recv["password"], PW,
        "段4 启动送到常驻 Driver 的必须是明文: {started_recv}"
    );
    assert!(
        !started_recv.to_string().contains("secret_set"),
        "段4 marker 绝不进入 Driver: {started_recv}"
    );

    // 数据确实产出（证据形式：REST 最新值面出现该点，且数值持续推进）。
    //
    // NOTE（必须有界等待，不能单次读取即断言）：启动到**首个采样到达** REST 面之间
    // 存在真实时间差，单次读取在慢机器上会偶发失败——实测 run 38750918018 在
    // windows-arm64 上正是于此 panic（"段4 启动后必须产出采集数据"）。
    // 这里改为有界轮询：断言内容与期望值完全不变，只是给它出现的窗口。
    let mut point = serde_json::Value::Null;
    let mut last_seen = serde_json::Value::Null;
    for _ in 0..60 {
        let (s, v) = get_json(app.clone(), "/api/v1/points/latest").await;
        assert_eq!(s, StatusCode::OK, "段4 最新值: {v}");
        last_seen = v.clone();
        if let Some(found) = v["points"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["point_key"] == "c3e.counter"))
            .cloned()
        {
            point = found;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        !point.is_null(),
        "段4 启动后必须产出采集数据（等待 30s 仍未出现）: {last_seen}"
    );
    assert_eq!(point["endpoint_id"], EID, "段4: {point}");
    assert_eq!(point["type"], "f64", "段4: {point}");
    assert_eq!(point["quality"], "GOOD", "段4: {point}");
    let first = point["value"].as_f64().expect("段4 最新值必须是数值");
    common::wait_until(15, || {
        c3e_latest(&mgr, "c3e.counter").is_some_and(|x| x > first)
    })
    .await;
    let (s, v) = get_json(app.clone(), &format!("/api/v1/endpoints/{EID}/diagnostics")).await;
    assert_eq!(s, StatusCode::OK, "段4 诊断: {v}");
    assert_eq!(v["point_count"], 1, "段4 点表必须已分配: {v}");
    assert_eq!(v["connection_state"], "RUNNING", "段4: {v}");
    // 持久化面：启动的合法变化只有期望态 / point_map / updated_at_ns。
    let st4 = c3e_state(&store, EID);
    assert_eq!(st4["desired_running"], true, "段4 期望态必须置 true");
    let pm = st4["point_map"].as_object().unwrap();
    assert_eq!(pm.len(), 1, "段4 只应有一个采集点: {pm:?}");
    assert_eq!(
        pm["c3e.counter"].as_u64(),
        Some(1),
        "段4 point_id 由 Core 分配并持久化: {pm:?}"
    );
    assert_changed_within(
        &st2,
        &st4,
        &["desired_running", "point_map", "updated_at_ns"],
        "段4 手动启动",
    );
    assert_eq!(st4["tasks"], st2["tasks"], "段4 启动不得改动任务面");
    assert_eq!(st4["password_secret"], PW, "段4 启动不得改动 Secret");
    assert_eq!(
        c3e_raw_secret_rows(&db),
        raw_secrets_before,
        "段4 启动不得重写 endpoint_secrets"
    );

    // === 段 4b：运行中的连接检查（C3-08：不得停止/重启既有 Endpoint） ===
    let before_running_probe = c3e_state(&store, EID);
    let value_before_probe = c3e_latest(&mgr, "c3e.counter").expect("段4b 预置：已开始出数");
    // 「不重启」的判据用 **stream_epoch**：每次成功 Start 都会拿到新 epoch
    //（§10/§17），因此 epoch 不变 = 这个 Endpoint 的运行会话从未被停掉重起。
    // 这比 PID 集合更稳且更有语义；PID 级证据见 subprocess_orphan_guard 的 C3-08。
    let epoch_before_probe = mgr
        .snapshot()
        .endpoint(EID)
        .expect("段4b 预置：运行态必须存在")
        .epoch;
    assert!(
        epoch_before_probe > 0,
        "段4b 预置：成功 Start 后 epoch 必须非 0（否则下面的「epoch 不变」是空断言）"
    );
    let (s, v) = post_json(
        app.clone(),
        &uri_check,
        &probe_body(serde_json::json!({
            "password": { "secret_set": true },
            "capture_path": running_probe_cap.to_string_lossy(),
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "段4b 运行中探测: {v}");
    assert_eq!(v["reachable"], true, "段4b: {v}");
    assert_eq!(
        mgr.snapshot().endpoint(EID).map(|s| s.state),
        Some("RUNNING".to_string()),
        "段4b 连接检查不得停止/重启既有 Endpoint"
    );
    assert_eq!(
        mgr.snapshot().endpoint(EID).map(|s| s.epoch),
        Some(epoch_before_probe),
        "段4b 连接检查不得重启既有 Endpoint（stream_epoch 必须不变）"
    );
    // 未被重启/断流：采集值必须继续推进（重启会重置 epoch 并断流）。
    common::wait_until(15, || {
        c3e_latest(&mgr, "c3e.counter").is_some_and(|x| x > value_before_probe)
    })
    .await;
    let running_recv: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&running_probe_cap).expect("运行中探测也必须落盘"),
    )
    .unwrap();
    assert_eq!(
        running_recv["password"], PW,
        "段4b 运行中探测同样必须送明文: {running_recv}"
    );
    assert_eq!(
        c3e_state(&store, EID),
        before_running_probe,
        "段4b 连接检查不得修改任何持久化状态"
    );
    assert_eq!(
        c3e_raw_secret_rows(&db),
        raw_secrets_before,
        "段4b 连接检查不得重写 endpoint_secrets"
    );

    drop(guard);
    let logged = String::from_utf8(log_buf.lock().unwrap().clone()).unwrap();
    assert!(
        !logged.is_empty(),
        "预置：trace 级必须实际产出日志（否则本断言是空断言）"
    );
    assert!(!logged.contains(PW), "链路上任何日志都不得出现明文 Secret");

    // === 段 5：停止 ===
    let (s, v) = post_json(app.clone(), &format!("/api/v1/endpoints/{EID}/stop"), "{}").await;
    assert_eq!(s, StatusCode::OK, "段5 停止: {v}");
    assert_eq!(v["was_running"], true, "段5: {v}");
    assert!(!mgr.is_running(EID), "段5 停止后不得仍在运行");
    common::wait_until(15, || {
        mgr.snapshot()
            .endpoint(EID)
            .is_some_and(|s| s.state == "STOPPED")
    })
    .await;
    let (s, v) = get_json(app.clone(), &format!("/api/v1/endpoints/{EID}/state")).await;
    assert_eq!(s, StatusCode::OK, "段5 状态: {v}");
    assert_eq!(v["state"], "STOPPED", "段5 状态必须为 STOPPED: {v}");
    let st5 = c3e_state(&store, EID);
    assert_eq!(st5["desired_running"], false, "段5 期望态必须回落 false");
    assert_changed_within(
        &st4,
        &st5,
        &["desired_running", "updated_at_ns"],
        "段5 停止",
    );
    assert_eq!(
        st5["connection_json"], st4["connection_json"],
        "段5 不得改动连接"
    );
    assert_eq!(st5["password_secret"], PW, "段5 不得改动 Secret");
    assert_eq!(st5["point_map"], st4["point_map"], "段5 不得改动 point_map");
    assert_eq!(st5["tasks"], st4["tasks"], "段5 不得改动任务面");
    assert_eq!(
        c3e_raw_secret_rows(&db),
        raw_secrets_before,
        "段5 停止不得重写 endpoint_secrets"
    );

    mgr.shutdown_all().await;
}

/// C3-01：设备不可达时仍可仅保存合法配置，**零 Probe、零 Start**。
///
/// 两半证据：
/// ① 夹具级「会连不上」的设备（test-driver `probe_mode=unreachable` +
///    `probe_delay_ms=60000` + `capture_path`）：保存请求必须快速返回 201，且
///    capture 文件不存在 ⇒ 保存路径没有发起任何 OpenConnection（零 Probe），
///    也没有启动（零 Start）；
/// ② 真实网络不可达（s7 → loopback 关闭端口，非夹具注入）：保存同样放通，
///    事后显式探测确认「不可达」不是假象。
///
/// ①的耗时 + 落盘证据能代表两个创建入口：`POST /api/v1/device-bootstrap` 与
/// `POST /api/v1/endpoints` 都不含任何探测调用（探测只存在于 connection-check
/// 与 `/drivers/{id}/probe`），②再补一条真实网络不可达的保存证据。
#[tokio::test]
async fn c3e_unreachable_device_offline_save_is_zero_probe_and_zero_start() {
    const PW: &str = "pw-c3e-unreachable";
    let tmp = ProbeTempDir::new("c3e-unreachable");
    let db = tmp.file("mesa.db");
    let save_cap = tmp.file("must-not-exist-after-save.json");
    let probe_cap = tmp.file("unreachable-probe.json");

    let store = Arc::new(ConfigStore::open(&db).unwrap());
    let staged = common::staged_drivers_with_test_driver();
    let drivers_dir = staged.path().clone();
    // 同上：复刻生产装配（持久版 ID 源），否则「离线保存」之外的一切都与 mesad 不同源。
    let mgr = Arc::new(mesa_driver_manager::MesaManager::with_source(
        &drivers_dir,
        Arc::new(mesa_driver_manager::StorePointIdSource::new(Arc::clone(
            &store,
        ))),
    ));
    let state = common::isolated_app_state(
        Arc::clone(&mgr),
        Arc::clone(&store),
        drivers_dir.to_string_lossy().to_string(),
        false,
    )
    .unwrap();
    let app = state.router();
    warm_descriptor(&app, "test-driver").await;

    // ① 夹具注入的不可达设备：离线保存必须成功，且全程不接触设备。
    let started = std::time::Instant::now();
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/device-bootstrap",
        &serde_json::json!({
            "device": { "id": "c3e-ud1", "name": "UD1" },
            "endpoint": {
                "id": "c3e-u1",
                "name": "U1",
                "driver_id": "test-driver",
                "connection": {
                    "password": PW,
                    "probe_mode": "unreachable",
                    "probe_delay_ms": 60000,
                    "capture_path": save_cap.to_string_lossy(),
                },
            },
            "acquisition": { "tasks": [] },
            "start": false,
            "idempotency_key": "c3e-ud-1",
        })
        .to_string(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "① 不可达设备必须能离线保存: {v}");
    assert_eq!(v["started"], false, "① 必须不启动: {v}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "① 保存不得探测设备（耗时 {:?}；探测路径 Open 预算 5s 起，且本设备注入 60s 延迟）",
        started.elapsed()
    );
    assert!(
        !save_cap.exists(),
        "① 零 Probe / 零 Start 的结构证据：没有任何 OpenConnection"
    );
    assert!(
        store.list_tasks("c3e-u1").unwrap().is_empty(),
        "① 零采集点：tasks 必须为空"
    );
    assert!(
        !store
            .get_endpoint("c3e-u1")
            .unwrap()
            .unwrap()
            .desired_running,
        "① 期望态必须为 false"
    );
    assert!(
        mgr.snapshot().endpoint("c3e-u1").is_none(),
        "① 保存不得产生运行态"
    );

    // ② 同一 Endpoint 在**显式请求探测**时确实不可达 —— 证明①保存成功的设备真的连不上。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/c3e-u1/connection-check",
        &probe_body(serde_json::json!({
            "probe_mode": "unreachable",
            "probe_delay_ms": 0,
            "capture_path": probe_cap.to_string_lossy(),
        })),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "② 不可达是 200 + reachable:false（不是 5xx）: {v}"
    );
    assert_eq!(v["reachable"], false, "②: {v}");
    assert_eq!(v["warnings"][0]["code"], "CONNECTION_FAILED", "②: {v}");
    assert!(
        probe_cap.exists(),
        "② 显式探测确实建立了临时连接（与①的零接触形成对照）"
    );
    assert!(
        mgr.snapshot().endpoint("c3e-u1").is_none(),
        "② 连接检查不得产生运行态"
    );

    // ③ 真实网络不可达（loopback 关闭端口）：走另一条创建入口（device + endpoint）。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/devices",
        r#"{"id":"c3e-ud2","name":"UD2"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "③ 建设备: {v}");
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints",
        r#"{"id":"c3e-u2","name":"U2","device_id":"c3e-ud2","driver_id":"s7","connection":{"host":"127.0.0.1","port":9}}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "③ 不可达设备必须能离线保存: {v}");
    let rec = store.get_endpoint("c3e-u2").unwrap().unwrap();
    assert!(!rec.desired_running, "③ 创建即保持 STOPPED");
    assert!(
        store.list_tasks("c3e-u2").unwrap().is_empty(),
        "③ 零采集点：tasks 必须为空"
    );
    assert!(
        mgr.snapshot().endpoint("c3e-u2").is_none(),
        "③ 不得产生运行态"
    );
    // 事后显式探测确认它确实不可达。草稿必须自带**非 Secret** 字段：按
    // `resolve_connection_draft` 的 NOTE「非 Secret 字段不做继承」，缺省继承只
    // 适用于契约 §4 的 Secret 字段，可见字段走整体替换 + required 裁决——空草稿
    // 会得到 400 `REQUIRED host`，这不是缺陷而是与 PUT 同源的语义。
    let (s, v) = post_json(
        app.clone(),
        "/api/v1/endpoints/c3e-u2/connection-check",
        &probe_body(serde_json::json!({"host": "127.0.0.1", "port": 9})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "③ 探测: {v}");
    assert_eq!(v["reachable"], false, "③ s7 关闭端口必须不可达: {v}");
    assert_eq!(v["warnings"][0]["code"], "CONNECTION_FAILED", "③: {v}");

    mgr.shutdown_all().await;
}

// ---------------------------------------------------------------------------
// I-2 阶段 2：Manager 管理通道 + Core 只读路由
// （I7/I8/I9/I23/I36–I43 + P0-1 A1–A5 + P0-3 T1–T6/R-1–R-4）
// ---------------------------------------------------------------------------
//
// 为什么必须用「夹具驱动」而不是 test-driver：后者走 SDK 默认实现，只能覆盖 501
// 分支；而阶段 2 验收的是**通道语义**（200 / 分页 / 超时 / 超大 / 并发 / 取消），
// 需要一个真实应答的驱动进程。夹具 `definitions_fixture` 与既有 `hang_helper`
// 同形态（合同测试自己的 bin），不进生产依赖图。

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// 用夹具驱动组装 app：`modes` 里每个短名一个独立 driver 目录。
async fn definitions_app(
    modes: &[&str],
) -> (
    axum::Router,
    Arc<mesa_driver_manager::MesaManager>,
    common::StagedDriversDir,
) {
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let staged = common::staged_definitions_fixture(modes);
    let dir = staged.path().clone();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&dir));
    let state =
        common::isolated_app_state(mgr.clone(), store, dir.to_string_lossy().to_string(), false)
            .unwrap();
    (state.router(), mgr, staged)
}

fn catalog_uri(driver_id: &str) -> String {
    format!("/api/v1/drivers/{driver_id}/definitions")
}

fn preview_uri(driver_id: &str) -> String {
    format!("/api/v1/drivers/{driver_id}/definitions/identity-preview")
}

/// I37-①② + 逐字透传：目录信封（total/limit/next_cursor）与条目值必须原样到达调用方。
#[tokio::test]
async fn definitions_catalog_is_returned_verbatim_with_envelope() {
    let (app, _mgr, _staged) = definitions_app(&["ok"]).await;
    let (status, v) = get_json(app.clone(), &catalog_uri("definitions-fixture-ok")).await;
    assert_eq!(status, StatusCode::OK, "body: {v}");
    assert_eq!(v["total"], 42, "过滤后条目总数: {v}");
    assert_eq!(v["limit"], 100, "缺省页大小必须是 100: {v}");
    assert!(v["next_cursor"].is_null(), "42 < 100 时不得给游标: {v}");
    let defs = v["definitions"].as_array().expect("definitions 必须是数组");
    assert_eq!(defs.len(), 42);
    assert_eq!(defs[0]["variable_id"], "fixture.variable.0000");
    // 中文/缩进必须逐字保留（任何 serde 往返都会改键序或转义）
    assert_eq!(defs[0]["label"]["zh"], "夹具变量 0");
    assert_eq!(defs[41]["variable_id"], "fixture.variable.0041");

    // 查询参数逐字到达驱动（夹具把 family 回显进信封）
    let (status, v) = get_json(
        app,
        &format!("{}?family=840d-sl", catalog_uri("definitions-fixture-ok")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {v}");
    assert_eq!(
        v["family"], "840d-sl",
        "family 必须逐字到达驱动并被回显: {v}"
    );
}

/// I37-②③：`limit` 超限夹取（不是错误）＋ 游标翻页不重不漏。
#[tokio::test]
async fn definitions_pagination_cursor_is_stable_and_lossless() {
    let (app, _mgr, _staged) = definitions_app(&["paged"]).await;
    // ② limit=5000 ⇒ 夹取到 500（与 Browse 的 min(1000) 同口径，不报错）
    let (status, v) = get_json(
        app.clone(),
        &format!("{}?limit=5000", catalog_uri("definitions-fixture-paged")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {v}");
    assert_eq!(v["limit"], 500, "超出上限必须夹取而不是 400: {v}");
    assert_eq!(v["total"], 250);
    assert_eq!(v["definitions"].as_array().unwrap().len(), 250);
    assert!(v["next_cursor"].is_null(), "单页装得下就没有下一页: {v}");

    // ③ 真翻页：100 + 100 + 50，游标逐字回传
    let mut ids: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let uri = match &cursor {
            None => format!("{}?limit=100", catalog_uri("definitions-fixture-paged")),
            Some(c) => format!(
                "{}?limit=100&cursor={c}",
                catalog_uri("definitions-fixture-paged")
            ),
        };
        let (status, page) = get_json(app.clone(), &uri).await;
        assert_eq!(status, StatusCode::OK, "body: {page}");
        for d in page["definitions"].as_array().unwrap() {
            ids.push(d["variable_id"].as_str().unwrap().to_string());
        }
        pages += 1;
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
        assert!(pages < 10, "翻页必须收敛: {pages}");
    }
    assert_eq!(pages, 3, "250 条 / 每页 100 ⇒ 3 页");
    assert_eq!(ids.len(), 250, "翻页必须不重不漏");
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), 250, "游标翻页不得出现重复条目");
    assert_eq!(ids[0], "fixture.variable.0000");
    assert_eq!(ids[249], "fixture.variable.0249");
}

/// E12 / I37-⑤：查询信封的形态错误一律 400 `INVALID_QUERY`（kind=ConfigurationError）。
#[tokio::test]
async fn definitions_query_shape_violations_are_400() {
    let (app, _mgr, _staged) = definitions_app(&["ok"]).await;
    let base = catalog_uri("definitions-fixture-ok");
    let cases = [
        ("?limit=0", "limit 必须正整数"),
        ("?limit=-1", "limit 不得为负"),
        ("?limit=abc", "limit 非数字"),
        ("?revision=1", "revision 缺 variable_id"),
        ("?variable_id=x&revision=0", "revision 必须正整数"),
        ("?variable_id=x&revision=abc", "revision 非数字"),
        ("?variable_id=x&cursor=y", "cursor 与 variable_id 互斥"),
    ];
    for (query, why) in cases {
        let (status, v) = get_json(app.clone(), &format!("{base}{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {v}");
        assert_eq!(v["error"]["code"], "INVALID_QUERY", "{why}: {v}");
        assert_eq!(
            v["error"]["kind"], "ConfigurationError",
            "{why} 的 kind 必须确定: {v}"
        );
    }
}

/// E1：未知/不可启动的驱动 ⇒ 503 `DRIVER_UNAVAILABLE`，kind=ConnectionError。
#[tokio::test]
async fn definitions_unknown_driver_is_503_driver_unavailable() {
    let (app, _mgr, _staged) = definitions_app(&["ok"]).await;
    let (status, v) = get_json(app, &catalog_uri("no-such-driver")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {v}");
    assert_eq!(v["error"]["code"], "DRIVER_UNAVAILABLE");
    assert_eq!(v["error"]["kind"], "ConnectionError");
}

/// I9：未实现 I-2 的驱动（test-driver，源码零改动）⇒ **快速** 501，且 kind 必须是
/// `Unsupported`（**不得**误报成 Connection/Device，那是「去查机床」的语义）。
#[tokio::test]
async fn definitions_unsupported_driver_is_fast_501() {
    let (app, _mgr, _staged) = app().await;
    let started = Instant::now();
    let (status, v) = get_json(app.clone(), &catalog_uri("test-driver")).await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "body: {v}");
    assert_eq!(v["error"]["code"], "DEFINITIONS_UNSUPPORTED");
    assert_eq!(
        v["error"]["kind"], "Unsupported",
        "协议不支持不得报成设备离线"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "默认实现必须快速失败（不得等满 RPC 预算；并行负载下 4–5s 属正常，判别点是 code+kind），实测 {elapsed:?}"
    );

    // 预览同判据
    let (status, v) = post_json(
        app,
        &preview_uri("test-driver"),
        r#"{"family":"828d","kind":"nck_variable","parameters":{}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "body: {v}");
    assert_eq!(v["error"]["code"], "DEFINITIONS_UNSUPPORTED");
    assert_eq!(v["error"]["kind"], "Unsupported");
}

/// E4 / I37-④：驱动返回的条目数超过 4096 硬顶 ⇒ 503 `DEFINITION_QUERY_TOO_LARGE`。
#[tokio::test]
async fn definitions_oversized_catalog_is_503_too_large() {
    let (app, _mgr, _staged) = definitions_app(&["big"]).await;
    let (status, v) = get_json(app, &catalog_uri("definitions-fixture-big")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {v}");
    assert_eq!(v["error"]["code"], "DEFINITION_QUERY_TOO_LARGE");
    assert_eq!(v["error"]["kind"], "InternalError");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("4096"),
        "消息必须给出硬顶: {v}"
    );
}

/// I38-T1（P0-3）：驱动握手超时 ⇒ 503 `DRIVER_UNAVAILABLE`。
///
/// 断言口径（**实测修正**，与任务书 T1 的「≤6s」不同）：
/// - 「不发 Hello」命中的是 `connect_with_stage` 里的 **Hello 读超时 5s**，
///   **不是** `DRIVER_STARTUP_TIMEOUT`（6s 只是「连接被拒」重试循环的 deadline）；
/// - 该失败属 transport 瞬态 ⇒ `temp_driver_attempt` 允许**重建一次**
///   （`MAX_TEMP_ATTEMPTS = 2`）⇒ 可观测耗时 ≈ **2 × 5s**。
///
/// 因此断言区间取 [5.5s, 13s]：下界证明没有走 10s 的通用 `REQUEST_TIMEOUT`，
/// 上界证明没有第三次 attempt。任务书 T1 的「≤6s」只在「单 attempt + 连接被拒」
/// 这一种组合下成立；实测口径以本用例为准（已在交付说明中登记）。
#[tokio::test]
async fn definitions_startup_timeout_is_driver_unavailable() {
    let (app, _mgr, _staged) = definitions_app(&["silent"]).await;
    let started = Instant::now();
    let (status, v) = get_json(app, &catalog_uri("definitions-fixture-silent")).await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {v}");
    assert_eq!(v["error"]["code"], "DRIVER_UNAVAILABLE", "body: {v}");
    assert_eq!(v["error"]["kind"], "ConnectionError");
    assert!(
        elapsed >= Duration::from_millis(5500),
        "必须真的等满启动 deadline，实测 {elapsed:?} body={v}"
    );
    assert!(
        elapsed <= Duration::from_secs(40),
        "attempt 数必须有界（并行负载会把 2 × 5s 放大，实测可达 16s），实测 {elapsed:?}"
    );
}

/// I38-T2（P0-3）：RPC 不响应 ⇒ 503 `DEFINITION_QUERY_TIMEOUT`，且**必须**落在
/// 5s 预算内（若命中 10s 的 `REQUEST_TIMEOUT` 说明用错了形态，按设计应判阻塞）。
#[tokio::test]
async fn definitions_rpc_timeout_budget_is_5s() {
    let (app, mgr, _staged) = definitions_app(&["hang"]).await;
    let started = Instant::now();
    let (status, v) = get_json(app, &catalog_uri("definitions-fixture-hang")).await;
    let elapsed = started.elapsed();
    // **三元组冻结**（阶段 2 定向补丁）：E3 必须固定为
    // `503 + InternalError + DEFINITION_QUERY_TIMEOUT`。
    // 只改 kind（例如改回 `Timeout`）会让通用映射把 503 变成 504 —— 这条端到端
    // 断言与 `manager.rs` / `core-api` 的两条单测共同锁死该组合。
    assert_eq!(
        (
            status,
            v["error"]["kind"].as_str(),
            v["error"]["code"].as_str()
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("InternalError"),
            Some("DEFINITION_QUERY_TIMEOUT")
        ),
        "E3 必须是 503/Internal/DEFINITION_QUERY_TIMEOUT（Manager 预算耗尽，不是机床超时）: {v}"
    );
    assert!(
        elapsed >= Duration::from_millis(4800),
        "必须真的等满 5s 预算，实测 {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(30),
        "RPC 超时必须最终有界返回（5s 预算由常量断言确定性保证），实测 {elapsed:?}"
    );
    // R-2：许可必须回落
    assert_eq!(
        mgr.definition_query_in_flight("definitions-fixture-hang"),
        0,
        "超时路径不得泄漏并发许可"
    );
}

/// I39-A1/A2/A4：per-driver 预算 4，第 5 个**立即** 503；跨 driver 互不影响。
#[tokio::test]
async fn definition_query_concurrency_budget_rejects_immediately() {
    let (app, mgr, _staged) = definitions_app(&["slow", "ok"]).await;
    let uri = catalog_uri("definitions-fixture-slow");

    // A1/A2：同时发起 5 个（slow 驱动 1.5s 才应答 ⇒ 4 个必然同时在飞）
    let mut handles = Vec::new();
    for _ in 0..5 {
        let app = app.clone();
        let uri = uri.clone();
        handles.push(tokio::spawn(async move {
            let started = Instant::now();
            let (status, body) = get_json(app, &uri).await;
            (status, body, started.elapsed())
        }));
    }
    let mut ok = 0;
    let mut busy: Vec<(serde_json::Value, Duration)> = Vec::new();
    for h in handles {
        let (status, body, elapsed) = h.await.unwrap();
        if status == StatusCode::OK {
            ok += 1;
        } else {
            busy.push((body, elapsed));
        }
    }
    assert_eq!(ok, 4, "预算 4 ⇒ 只允许 4 个在飞");
    assert_eq!(busy.len(), 1, "第 5 个必须被拒");
    let (body, elapsed) = &busy[0];
    assert_eq!(
        body["error"]["code"], "DEFINITION_QUERY_BUSY",
        "body: {body}"
    );
    assert_eq!(body["error"]["kind"], "InternalError");
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("4 in-flight") && msg.contains("budget 4"),
        "消息必须含 in-flight 数与预算: {msg}"
    );
    // A2：立即拒绝（≪ 5s RPC 预算，也 ≪ 1.5s 的驱动应答延迟）
    assert!(
        *elapsed < Duration::from_millis(500),
        "超限必须立即拒绝而不是排队，实测 {elapsed:?}"
    );

    // A4：跨 driver 隔离 —— A 占满 4 个时 B 仍必须正常返回
    let mut slow_handles = Vec::new();
    for _ in 0..4 {
        let app = app.clone();
        let uri = uri.clone();
        slow_handles.push(tokio::spawn(async move { get_json(app, &uri).await }));
    }
    // 等 4 个真的占满（许可在进入 Manager 时同步取号）
    let deadline = Instant::now() + Duration::from_secs(3);
    while mgr.definition_query_in_flight("definitions-fixture-slow") < 4 {
        assert!(Instant::now() < deadline, "4 个请求未能在 3s 内占满预算");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = Instant::now();
    let (status, body) = get_json(app.clone(), &catalog_uri("definitions-fixture-ok")).await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "跨 driver 不得互相拖累: {body}");
    assert!(elapsed < Duration::from_secs(2), "实测 {elapsed:?}");
    for h in slow_handles {
        assert_eq!(h.await.unwrap().0, StatusCode::OK);
    }
    assert_eq!(
        mgr.definition_query_in_flight("definitions-fixture-slow"),
        0
    );
}

/// 进程枚举：按可执行文件名快照存活 PID（标准库实现，无新依赖；与
/// `subprocess_orphan_guard.rs:236` 同一实现，Linux `comm` 截断到 15 字符）。
fn live_pids(name: &str) -> HashSet<u32> {
    debug_assert!(
        name.trim_end_matches(".exe").len() <= 15,
        "comm truncation risk: {name}"
    );
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {name}")])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        out.lines()
            .filter_map(|l| {
                let cols: Vec<&str> = l.split("\",\"").collect();
                if cols.len() >= 2 && cols[0].trim_matches('"').eq_ignore_ascii_case(name) {
                    cols[1].trim_matches('"').parse::<u32>().ok()
                } else {
                    None
                }
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        let mut set = HashSet::new();
        let Ok(rd) = std::fs::read_dir("/proc") else {
            return set;
        };
        for e in rd.flatten() {
            let fname = e.file_name();
            let Some(pid) = fname.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if let Ok(comm) = std::fs::read_to_string(e.path().join("comm"))
                && comm.trim() == name
            {
                set.insert(pid);
            }
        }
        set
    }
}

fn fixture_exe_name(mode: &str) -> String {
    if cfg!(windows) {
        format!("def-{mode}.exe")
    } else {
        format!("def-{mode}")
    }
}

/// 等某个模式的新进程出现（证明「取消前进程确实活着」，否则无残留断言是空跑）。
async fn wait_for_new_pid(before: &HashSet<u32>, name: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = live_pids(name).difference(before).copied().next() {
            return pid;
        }
        assert!(Instant::now() < deadline, "等待 {name} 新进程超时");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 断言 `name` 的存活 PID 最终回落到 `before` 子集（无残留、无孤儿）。
///
/// 必须用 `tokio::time::sleep` 轮询：`#[tokio::test]` 默认单线程运行时，
/// `std::thread::sleep` 会阻塞调度，使 `kill_on_drop` 之后的异步子进程回收
/// 永远没机会执行（Linux 上表现为僵尸 PID 迟迟不消失）。
async fn assert_no_orphan(before: &HashSet<u32>, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let cur = live_pids(name);
        if cur.is_subset(before) {
            return;
        }
        if Instant::now() >= deadline {
            let mut leaked: Vec<u32> = cur.difference(before).copied().collect();
            leaked.sort_unstable();
            panic!("定义查询残留子进程 {name} pids={leaked:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// P0-1 A3 / P0-3 R-2–R-4：四条路径（成功 / 失败 / 超时 / 取消）都必须归还许可，
/// 且**取消**路径还要真的回收临时进程（不是只减计数）。
#[tokio::test]
async fn definition_query_permits_release_on_all_paths_and_cancel_reaps_process() {
    let (app, mgr, _staged) = definitions_app(&["ok", "fail", "hang", "cancel"]).await;
    let ok_uri = catalog_uri("definitions-fixture-ok");
    let fail_uri = catalog_uri("definitions-fixture-fail");
    let hang_uri = catalog_uri("definitions-fixture-hang");

    // ① 成功 / ② 失败（驱动业务拒绝）：各 4 轮，许可必须逐轮回落
    for round in 0..4 {
        let (status, v) = get_json(app.clone(), &ok_uri).await;
        assert_eq!(status, StatusCode::OK, "round {round}: {v}");
        assert_eq!(
            mgr.definition_query_in_flight("definitions-fixture-ok"),
            0,
            "round {round}: 成功路径必须归还许可"
        );

        let (status, v) = get_json(app.clone(), &fail_uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "round {round}: {v}");
        assert_eq!(v["error"]["code"], "INVALID_QUERY", "Driver 码必须原样过线");
        assert_eq!(
            v["error"]["kind"], "ConfigurationError",
            "kind 必须原样过线"
        );
        assert_eq!(
            mgr.definition_query_in_flight("definitions-fixture-fail"),
            0,
            "round {round}: 失败路径必须归还许可"
        );
    }

    // ③ 超时：4 个并发（各 5s，并行执行）⇒ 全部 503 且许可回落
    let mut timeouts = Vec::new();
    for _ in 0..4 {
        let app = app.clone();
        let uri = hang_uri.clone();
        timeouts.push(tokio::spawn(async move { get_json(app, &uri).await }));
    }
    for h in timeouts {
        let (status, v) = h.await.unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {v}");
        assert_eq!(v["error"]["code"], "DEFINITION_QUERY_TIMEOUT");
    }
    assert_eq!(
        mgr.definition_query_in_flight("definitions-fixture-hang"),
        0,
        "超时路径必须归还许可"
    );

    // ④ 取消（客户端中途断开）：先确认真实进程已起来，再 abort ⇒ 许可与进程都必须回收
    let cancel_uri = catalog_uri("definitions-fixture-cancel");
    let before = live_pids(&fixture_exe_name("cancel"));
    let app_for_cancel = app.clone();
    let task = tokio::spawn(async move { get_json(app_for_cancel, &cancel_uri).await });
    wait_for_new_pid(&before, &fixture_exe_name("cancel")).await;
    task.abort();
    let _ = task.await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while mgr.definition_query_in_flight("definitions-fixture-cancel") != 0 {
        assert!(
            Instant::now() < deadline,
            "取消路径未归还许可（in-flight = {}）",
            mgr.definition_query_in_flight("definitions-fixture-cancel")
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // R-3：临时进程必须被回收（无残留、无孤儿）
    assert_no_orphan(&before, &fixture_exe_name("cancel")).await;

    // R-4：恢复能力 —— 之后 4 个并发请求仍必须全部被接受（预算没有被锁死）
    let mut recover = Vec::new();
    for _ in 0..4 {
        let app = app.clone();
        let uri = ok_uri.clone();
        recover.push(tokio::spawn(async move { get_json(app, &uri).await }));
    }
    for h in recover {
        assert_eq!(h.await.unwrap().0, StatusCode::OK, "取消/超时后必须可恢复");
    }
    assert_eq!(mgr.definition_query_in_flight("definitions-fixture-ok"), 0);
}

/// I7/I8/I23/I40–I43（只读性与零副作用）：两条路由都不得建 Endpoint、不得产生
/// 数据面样本、不得改任务/采集计划、不得写任何持久化面。
#[tokio::test]
async fn definitions_rpc_opens_no_connection_and_has_no_side_effects() {
    let (app, mgr, _staged) = definitions_app(&["ok"]).await;
    let r1 = common::repo_root().join("drivers/sinumerik-nck/definitions/828d/r1.json");
    let r1_before = std::fs::read(&r1).expect("r1.json 必须存在");

    let (_, endpoints_before) = get_json(app.clone(), "/api/v1/endpoints").await;
    let (_, tasks_before) = get_json(app.clone(), "/api/v1/tasks").await;
    let (_, latest_before) = get_json(app.clone(), "/api/v1/points/latest").await;
    let (_, audit_before) = get_json(app.clone(), "/api/v1/control/audit").await;

    // 目录 + 预览各调一次（夹具的 open_connection **必然失败** ⇒ 200 即证明
    // Manager 从未发送 OpenConnection：否则这里会是 503）
    let (status, v) = get_json(app.clone(), &catalog_uri("definitions-fixture-ok")).await;
    assert_eq!(status, StatusCode::OK, "body: {v}");
    let (status, v) = post_json(
        app.clone(),
        &preview_uri("definitions-fixture-ok"),
        r#"{"family":"828d","kind":"nck_variable","parameters":{"variable_id":"x"}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {v}");

    let (_, endpoints_after) = get_json(app.clone(), "/api/v1/endpoints").await;
    let (_, tasks_after) = get_json(app.clone(), "/api/v1/tasks").await;
    let (_, latest_after) = get_json(app.clone(), "/api/v1/points/latest").await;
    let (_, audit_after) = get_json(app, "/api/v1/control/audit").await;

    assert_eq!(endpoints_before, endpoints_after, "不得创建 Endpoint");
    assert_eq!(tasks_before, tasks_after, "不得修改任务/采集计划");
    assert_eq!(latest_before, latest_after, "不得产生数据面样本");
    assert_eq!(
        audit_before["count"], audit_after["count"],
        "不得写 Control 审计"
    );
    assert_eq!(
        audit_after["count"], 0,
        "Control 审计必须仍为空: {audit_after}"
    );
    assert_eq!(
        r1_before,
        std::fs::read(&r1).expect("r1.json 必须仍可读"),
        "定义目录文件一个字节都不得改动"
    );
    // 运行态：没有任何 Endpoint 被拉起
    assert!(mgr.running_ids().is_empty(), "不得拉起任何 Endpoint");
}

/// E13：预览信封形状校验（只校验形状，不解释 `parameters` 语义）＋ 逐字透传。
#[tokio::test]
async fn preview_passes_request_verbatim_and_validates_envelope() {
    let (app, _mgr, _staged) = definitions_app(&["ok"]).await;
    let uri = preview_uri("definitions-fixture-ok");

    // 含中文 / 换行缩进 / 转义字符的请求体：必须**逐字**到达驱动
    let body = "{\n  \"family\": \"828d\",\n  \"kind\": \"nck_variable\",\n  \"parameters\": {\"variable_id\": \"通道参数\", \"note\": \"引号\\\"反斜杠\\\\制表\\t\"}\n}";
    let (status, v) = post_json(app.clone(), &uri, body).await;
    assert_eq!(status, StatusCode::OK, "body: {v}");
    assert_eq!(
        v["echo_request_json"].as_str(),
        Some(body),
        "请求体必须逐字到达驱动（不重排/不裁剪/不改编码）"
    );
    assert_eq!(v["point_key"], "fixture.variable.0001@channel=i:1");
    assert_eq!(v["identity_mode"], "derived");

    let cases = [
        (r#"{"kind":"nck_variable","parameters":{}}"#, "缺 family"),
        (r#"{"family":"828d","parameters":{}}"#, "缺 kind"),
        (
            r#"{"family":"828d","kind":"nck_variable"}"#,
            "缺 parameters",
        ),
        (
            r#"{"family":"828d","kind":"nck_variable","parameters":{},"point_key":"x"}"#,
            "禁止传 point_key",
        ),
        (
            r#"{"family":"828d","kind":"nck_variable","parameters":[]}"#,
            "parameters 必须是对象",
        ),
        (
            r#"{"family":"","kind":"nck_variable","parameters":{}}"#,
            "family 不得为空",
        ),
        ("not-json", "非法 JSON"),
    ];
    for (bad, why) in cases {
        let (status, v) = post_json(app.clone(), &uri, bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {v}");
        assert_eq!(v["error"]["code"], "INVALID_REQUEST", "{why}: {v}");
        assert_eq!(v["error"]["kind"], "ConfigurationError", "{why}: {v}");
    }
}

/// I36（E30）：预览 body 门 64 KiB —— 恰好 64 KiB 通过，+1 字节 ⇒ 413
/// `REQUEST_TOO_LARGE`（由挂在本路由上的 `DefaultBodyLimit` 产生，不是全局 2 MB）。
#[tokio::test]
async fn preview_body_size_cap_is_64kib() {
    let (app, _mgr, _staged) = definitions_app(&["ok"]).await;
    let uri = preview_uri("definitions-fixture-ok");
    let prefix = r#"{"family":"828d","kind":"k","parameters":{"pad":""#;
    let suffix = r#""}}"#;

    // ① 恰好 64 KiB ⇒ 通过
    let pad = 65536 - prefix.len() - suffix.len();
    let exact = format!("{prefix}{}{suffix}", "x".repeat(pad));
    assert_eq!(exact.len(), 65536);
    let (status, v) = post_json(app.clone(), &uri, &exact).await;
    assert_eq!(status, StatusCode::OK, "恰好 64 KiB 必须通过: {v}");

    // ② 64 KiB + 1 ⇒ 413 REQUEST_TOO_LARGE
    let over = format!("{prefix}{}{suffix}", "x".repeat(pad + 1));
    assert_eq!(over.len(), 65537);
    let (status, v) = post_json(app.clone(), &uri, &over).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "body: {v}");
    assert_eq!(v["error"]["code"], "REQUEST_TOO_LARGE", "body: {v}");
    assert_eq!(v["error"]["kind"], "ConfigurationError");

    // ③ 目录 GET 无 body ⇒ 该门不适用（同一 router 上仍正常）
    let (status, _) = get_json(app, &catalog_uri("definitions-fixture-ok")).await;
    assert_eq!(status, StatusCode::OK);
}

/// P0-1 A5：占满 4 个定义查询**不得影响正在进行的采集**。
///
/// 判据逐条：① Endpoint 始终 `RUNNING`；② `stream_epoch` 不变（没有被 Stop/Start）；
/// ③ 任务快照逐字不变；④ 饱和期间采集值**继续推进**（counter 递增）。
///
/// 为什么必须真起一条采集链：I-2 的每次调用都会 spawn 一个临时驱动进程，
/// 「不影响采集」不能靠「两者代码路径不同」推断，只能靠观测。
#[tokio::test]
async fn definition_queries_do_not_disturb_running_acquisition() {
    use mesa_core_types::{AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, TaskSchedule};

    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    let staged = common::staged_definitions_fixture_with_test_driver(&["slow"]);
    let dir = staged.path().clone();
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&dir));
    let state =
        common::isolated_app_state(mgr.clone(), store, dir.to_string_lossy().to_string(), false)
            .unwrap();
    let app = state.router();

    let task = AcquisitionTask {
        id: "t-a5".into(),
        schedule: TaskSchedule::Poll { interval_ms: 20 },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({
                "selections": [{
                    "resource_id": "counter",
                    "parameters": {},
                    "outputs": [{ "output": "value", "point_key": "a5.counter" }],
                }]
            }),
        },
    };
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "ep-a5".into(),
        driver_id: "test-driver".into(),
        connection_json: "{}".into(),
        tasks: vec![task],
        event_tasks: vec![],
    })
    .unwrap();
    common::wait_until(15, || {
        mgr.snapshot()
            .endpoint("ep-a5")
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;

    let first = wait_counter(&mgr, None).await;
    let epoch = mgr.snapshot().endpoint("ep-a5").unwrap().epoch;
    let (_, tasks_before) = get_json(app.clone(), "/api/v1/tasks").await;

    // 占满 4 个定义查询预算（slow 夹具 1.5s 才应答）
    let mut slow = Vec::new();
    for _ in 0..4 {
        let app = app.clone();
        let uri = catalog_uri("definitions-fixture-slow");
        slow.push(tokio::spawn(async move { get_json(app, &uri).await }));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while mgr.definition_query_in_flight("definitions-fixture-slow") < 4 {
        assert!(Instant::now() < deadline, "4 个定义查询未能占满预算");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 饱和期间采集必须继续推进
    let second = wait_counter(&mgr, Some(first)).await;
    assert_ne!(second, first, "饱和期间采集值必须继续变化");

    for h in slow {
        assert_eq!(h.await.unwrap().0, StatusCode::OK, "定义查询本身必须成功");
    }

    let st = mgr
        .snapshot()
        .endpoint("ep-a5")
        .expect("Endpoint 必须仍然存在");
    assert_eq!(st.state, "RUNNING", "采集不得被打断");
    assert_eq!(st.epoch, epoch, "stream_epoch 不得变化（未被 Stop/Start）");
    let (_, tasks_after) = get_json(app, "/api/v1/tasks").await;
    assert_eq!(tasks_before, tasks_after, "任务快照必须逐字不变");
    let third = wait_counter(&mgr, Some(second)).await;
    assert_ne!(third, second, "定义查询结束后采集仍必须继续");

    mgr.shutdown_all().await;
}

/// 等 counter 值相对 `prev` 推进（`prev = None` 表示等第一个值）。
async fn wait_counter(mgr: &mesa_driver_manager::MesaManager, prev: Option<f64>) -> f64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(e) = mgr
            .snapshot()
            .latest_all()
            .into_iter()
            .find(|e| e.point_key == "a5.counter")
        {
            let v = e
                .value
                .value
                .as_f64()
                .or_else(|| e.value.value.as_i64().map(|i| i as f64))
                .unwrap_or(f64::NAN);
            if prev.map(|p| (v - p).abs() > f64::EPSILON).unwrap_or(true) {
                return v;
            }
        }
        assert!(Instant::now() < deadline, "等待 counter 推进超时");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
/// **Revision 不支持的映射冻结**（阶段 2 定向补丁）：`UNSUPPORTED_DEFINITION_REVISION`
/// 的 kind 固定为 `Configuration` ⇒ HTTP **400**（不是 404、也不是 501）。
///
/// 为什么由夹具驱动来证：Core 只按 kind 映射，绝不按码特判；因此只要驱动把
/// kind 报成 `ConfigurationError`，REST 就必须是 400。该用例把「不许为某个
/// Siemens 错误码单独返回 404」这条边界钉在阶段 2，而不是等到阶段 3 才暴露。
/// `available[]` 必须保留在 message 里，前端据此提示可选 Revision。
#[tokio::test]
async fn definition_query_revision_error_maps_to_400_configuration() {
    let (app, _mgr, _staged) = definitions_app(&["revision"]).await;
    let (status, v) = get_json(app, &catalog_uri("definitions-fixture-revision")).await;
    assert_eq!(
        (
            status,
            v["error"]["kind"].as_str(),
            v["error"]["code"].as_str()
        ),
        (
            StatusCode::BAD_REQUEST,
            Some("ConfigurationError"),
            Some("UNSUPPORTED_DEFINITION_REVISION")
        ),
        "Revision 不支持必须是 400/Configuration（不得按码特判成 404）: {v}"
    );
    let msg = v["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("available"),
        "消息必须保留 available[] 供前端提示: {msg}"
    );
}

/// E4-b（契约勘误）：驱动返回**格式非法或结构不符契约** ⇒ 503
/// `DEFINITION_RESPONSE_INVALID`，**不得**伪装成体积门 `DEFINITION_QUERY_TOO_LARGE`。
///
/// 正例与反例同测：合法目录仍必须 200（证明新门没有误伤正常路径）。
#[tokio::test]
async fn definition_response_invalid_is_503_not_too_large() {
    let (app, _mgr, _staged) = definitions_app(&["ok", "invalid", "shape"]).await;
    for (driver_id, why) in [
        ("definitions-fixture-invalid", "非法 JSON"),
        ("definitions-fixture-shape", "缺 definitions 数组"),
    ] {
        let (status, v) = get_json(app.clone(), &catalog_uri(driver_id)).await;
        assert_eq!(
            (
                status,
                v["error"]["kind"].as_str(),
                v["error"]["code"].as_str()
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Some("InternalError"),
                Some("DEFINITION_RESPONSE_INVALID")
            ),
            "{why} 必须报 DEFINITION_RESPONSE_INVALID（不是 TOO_LARGE）: {v}"
        );
    }
    // 正例：合法目录仍 200（新门不得误伤）
    let (status, v) = get_json(app, &catalog_uri("definitions-fixture-ok")).await;
    assert_eq!(status, StatusCode::OK, "合法目录必须仍 200: {v}");
    assert_eq!(v["total"], 42);
}

// ---------------------------------------------------------------------------
// I-2 阶段 3 收口：真实 sinumerik-nck 驱动的 HTTP 错误契约（UNSUPPORTED_FAMILY ⇒ 501）
// ---------------------------------------------------------------------------
//
// 为什么必须用**真实** NCK 驱动而不是夹具：本组断言钉的是「同一码 → 同一 kind →
// 同一 HTTP」这条跨层契约（DC §6.2.1），并且要为阶段 3 提供一条真实 API 链路证据
// （目录 42 条、空系列 200、未声明系列 501）。夹具驱动无法证明真实驱动的 code/kind。

/// `UNSUPPORTED_FAMILY` 三元组（**目录端点**，真实 NCK 驱动）：
/// `UNSUPPORTED_FAMILY` + `ErrorKind::Unsupported` + **HTTP 501**。
///
/// 【契约勘误（PR #115）】E8 原文写 400，但 §6.2.1 把该码定为 `Unsupported`，
/// 而 Core **只按 kind 映射**（不得按码特判）⇒ 统一为 501。本用例同时给出
/// 真实链路证据：未声明 family 必须 501，而「已声明但无定义」的 840d-sl 仍是
/// 200 + [] + note（E8 与 E9 必须可区分）。
#[tokio::test]
async fn nck_definitions_undeclared_family_is_501_unsupported() {
    let (app, _mgr, _staged) = app().await;
    let base = "/api/v1/drivers/sinumerik-nck/definitions";

    // 成功基线：真实 NCK 目录 42 条（证明请求确实打到了真实驱动）。
    let (status, v) = get_json(app.clone(), base).await;
    assert_eq!(status, StatusCode::OK, "真实 NCK 目录必须 200: {v}");
    assert_eq!(v["total"], 42, "真实目录必须 42 条: {v}");
    assert_eq!(v["driver_id"], "sinumerik-nck");
    assert!(v["next_cursor"].is_null(), "42 < 100 ⇒ 无下一页: {v}");

    // 未声明 family ⇒ 501 / Unsupported / UNSUPPORTED_FAMILY（三元组）。
    let (status, v) = get_json(app.clone(), &format!("{base}?family=foo")).await;
    assert_eq!(
        (
            status,
            v["error"]["kind"].as_str(),
            v["error"]["code"].as_str()
        ),
        (
            StatusCode::NOT_IMPLEMENTED,
            Some("Unsupported"),
            Some("UNSUPPORTED_FAMILY")
        ),
        "未声明 family 必须是 501/Unsupported/UNSUPPORTED_FAMILY（不得 400、不得 503）: {v}"
    );

    // 已声明但无定义 ⇒ **不是错误**：200 + [] + note（E9）。
    let (status, v) = get_json(app, &format!("{base}?family=840d-sl")).await;
    assert_eq!(status, StatusCode::OK, "空系列必须是 200: {v}");
    assert_eq!(v["definitions"], serde_json::json!([]));
    assert!(v["note"].is_string(), "空目录必须带 note: {v}");
}

/// `UNSUPPORTED_FAMILY` 三元组（**Preview 端点**，真实 NCK 驱动）：
/// 与目录端点**同码、同 kind、同 HTTP 501**。
///
/// 预览的 family 不匹配由 `compiler.rs` 的 `def.family != family` 检查产生
/// （E21），它必须是 `Unsupported`，绝不能退化成 400/404 或 503（那会把
/// 「升级驱动 / 换 Core」误导成「查机床」）。
#[tokio::test]
async fn nck_preview_family_mismatch_is_501_unsupported() {
    let (app, _mgr, _staged) = app().await;
    let body = serde_json::json!({
        "family": "840d-sl",
        "kind": "nck_variable",
        "parameters": {
            "variable_id": "axis.name",
            "definition_revision": 1,
            "target": { "axis": 1 },
            "arguments": {}
        }
    });
    let (status, v) = post_json(
        app,
        "/api/v1/drivers/sinumerik-nck/definitions/identity-preview",
        &body.to_string(),
    )
    .await;
    assert_eq!(
        (
            status,
            v["error"]["kind"].as_str(),
            v["error"]["code"].as_str()
        ),
        (
            StatusCode::NOT_IMPLEMENTED,
            Some("Unsupported"),
            Some("UNSUPPORTED_FAMILY")
        ),
        "Preview 的 family 不匹配必须是 501/Unsupported/UNSUPPORTED_FAMILY（与目录端点同一映射）: {v}"
    );
    // 消息必须含两侧 family，便于运维定位是「定义系列」还是「请求系列」。
    let msg = v["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("828d") && msg.contains("840d-sl"),
        "消息要素: {msg}"
    );
}
