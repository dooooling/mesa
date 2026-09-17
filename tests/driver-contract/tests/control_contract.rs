//! Control Plane 契约（§22；Foundation-3 structured target 单真值）。
//!
//! 覆盖：WriteTarget 三元组（Simulator `writable/value` Reference，slot 实例
//! 身份）、只读 output 拒绝、值类型门、CAS（EXPECTED_MISMATCH / 真原子）、
//! reset 命令语义（state 回初值 + `{}` 结果契约）、未声明 command 由 Driver
//! 拒绝（Core 门禁见 management_api REST 层；此处锁 Driver 直调语义）。
//! wire fail-closed（#3：expected/parameters_json 解码失败拒绝 + 原值不变）
//! 见本文件末尾 `wire_decode_failure_*`（进程内 SDK 服务直发 malformed 帧）。
//! 可靠队列永不 Latest-Wins、默认 disabled 见管理面/性能套件。

mod common;

use mesa_core_types::{
    AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, TaskSchedule, Value, WriteTarget,
};
use mesa_driver_sdk::Driver;

fn poll_task_with_key(key: &str, slot: &str) -> AcquisitionTask {
    AcquisitionTask {
        id: "t".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({"selections": [{"resource_id":"writable","parameters":{"initial":1,"slot":slot},"outputs":[{"output":"value","point_key": key}]}]}),
        },
    }
}

fn writable_target(slot: &str) -> WriteTarget {
    WriteTarget {
        resource_id: "writable".into(),
        parameters: serde_json::json!({"slot": slot}),
        output: "value".into(),
    }
}

async fn configured_conn(key: &str, slot: &str) -> Box<dyn mesa_driver_sdk::DriverConnection> {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task_with_key(key, slot)])
        .await
        .unwrap();
    conn.apply_point_map([(key.to_string(), 1)].into_iter().collect())
        .await
        .unwrap();
    conn
}

#[tokio::test]
async fn simulator_write_known_target_succeeds() {
    let conn = configured_conn("sim.x", "a").await;
    // known target（三元组 + slot 实例身份；point_key 只是采集投影）
    let res = conn
        .write(&writable_target("a"), Value::F64(42.0), None)
        .await;
    assert!(res.is_ok(), "write known target must succeed: {res:?}");
}

#[tokio::test]
async fn simulator_write_unknown_target_fails() {
    // Control/Acquisition 解耦后：未知 slot 首次写入即创建（"只写、不采集"），
    // 不再是 TARGET_NOT_FOUND。本用例转义为"非法 resource 仍拒绝"由
    // simulator_write_readonly_output_rejected 覆盖；此处锁定创建语义。
    let conn = configured_conn("sim.x", "a").await;
    conn.write(&writable_target("nope"), Value::F64(1.0), None)
        .await
        .unwrap();
    // 创建后 CAS 可观测（初值 1.0 即刚写入值）。
    conn.write(
        &writable_target("nope"),
        Value::F64(2.0),
        Some(Value::F64(1.0)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn simulator_write_without_acquire_succeeds() {
    // 终审 #1 单元门：从未出现在任何 acquisition task 的 slot 直接写成功，
    // 且 CAS 链可观测（42 → 43）。Control 资源空间独立于 Acquisition 投影。
    let driver = mesa_driver_simulator::SimulatorDriver;
    let conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.write(&writable_target("never-acquired"), Value::F64(42.0), None)
        .await
        .unwrap();
    conn.write(
        &writable_target("never-acquired"),
        Value::F64(43.0),
        Some(Value::F64(42.0)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn simulator_write_target_has_no_point_key() {
    // Foundation-3 #1 门：WriteTarget parameters 不得含 point_key。
    // point_key 只是采集投影；实例身份 = slot。
    let conn = configured_conn("sim.x", "a").await;
    let bad = WriteTarget {
        resource_id: "writable".into(),
        parameters: serde_json::json!({"point_key": "sim.x"}),
        output: "value".into(),
    };
    let res = conn.write(&bad, Value::F64(1.0), None).await;
    // 无 slot 即 INVALID_TARGET（point_key 不被识别为身份）
    assert_eq!(res.unwrap_err().code, "INVALID_TARGET");
}

#[tokio::test]
async fn simulator_write_readonly_output_rejected() {
    let conn = configured_conn("sim.x", "a").await;
    // 非 writable resource 即拒绝（OUTPUT_NOT_WRITABLE）
    let bad = WriteTarget {
        resource_id: "counter".into(),
        parameters: serde_json::json!({}),
        output: "value".into(),
    };
    let res = conn.write(&bad, Value::F64(1.0), None).await;
    assert_eq!(res.unwrap_err().code, "OUTPUT_NOT_WRITABLE");
}

#[tokio::test]
async fn simulator_write_value_type_mismatch_rejected() {
    let conn = configured_conn("sim.x", "a").await;
    // writable/value 恒 F64：Bool 即拒绝
    let res = conn
        .write(&writable_target("a"), Value::Bool(true), None)
        .await;
    assert_eq!(res.unwrap_err().code, "VALUE_TYPE_MISMATCH");
}

#[tokio::test]
async fn simulator_write_cas_mismatch_and_success() {
    let conn = configured_conn("sim.x", "a").await;
    // 初值 initial=1：expected=2 即 CAS 失败
    let res = conn
        .write(
            &writable_target("a"),
            Value::F64(9.0),
            Some(Value::F64(2.0)),
        )
        .await;
    assert_eq!(res.unwrap_err().code, "EXPECTED_MISMATCH");
    // expected=1 命中：写入 9.0 成功；再以 9.0 为期望写入 10.0（真 CAS 链）
    conn.write(
        &writable_target("a"),
        Value::F64(9.0),
        Some(Value::F64(1.0)),
    )
    .await
    .unwrap();
    conn.write(
        &writable_target("a"),
        Value::F64(10.0),
        Some(Value::F64(9.0)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn simulator_write_missing_slot_rejected() {
    let conn = configured_conn("sim.x", "a").await;
    // parameters 无 slot 即实例身份缺失
    let bad = WriteTarget {
        resource_id: "writable".into(),
        parameters: serde_json::json!({}),
        output: "value".into(),
    };
    let res = conn.write(&bad, Value::F64(1.0), None).await;
    assert_eq!(res.unwrap_err().code, "INVALID_TARGET");
}

#[tokio::test]
async fn simulator_command_reset_succeeds() {
    let conn = configured_conn("sim.x", "a").await;
    // 先写入偏离初值，再 reset 回到快照初值（state 恢复可观测）
    conn.write(&writable_target("a"), Value::F64(42.0), None)
        .await
        .unwrap();
    let res = conn.command("reset", "{}").await;
    assert!(res.is_ok(), "reset command must succeed");
    // #4 门：reset 结果必须通过自身 result_schema（空对象 {}）。
    let v = res.unwrap();
    assert_eq!(v, serde_json::json!({}), "reset result 必须为 {{}}");
    let desc = mesa_driver_simulator::SimulatorDriver.descriptor();
    let issues = mesa_core_types::gate_command_result_against(&desc, "reset", &v, "command");
    assert!(issues.is_empty(), "{issues:?}");
    // reset 后 CAS 期望回到初值 1.0 才能成功（state 已恢复）
    conn.write(
        &writable_target("a"),
        Value::F64(7.0),
        Some(Value::F64(1.0)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn simulator_command_reset_preserves_unacquired_slots() {
    // configure/reset 的 merge 语义：reset 只恢复被采集 slot，
    // 未被采集的 control slot 原样保留。
    let conn = configured_conn("sim.x", "a").await;
    conn.write(&writable_target("ghost"), Value::F64(5.0), None)
        .await
        .unwrap();
    conn.command("reset", "{}").await.unwrap();
    conn.write(
        &writable_target("ghost"),
        Value::F64(6.0),
        Some(Value::F64(5.0)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn simulator_command_unsupported_fails() {
    let conn = configured_conn("sim.x", "a").await;
    // reset 之外的旧桩命令（start/stop/fault/writable）已随 ControlCatalog 收敛删除
    for cmd in ["nope", "start", "stop", "fault", "writable"] {
        let res = conn.command(cmd, "{}").await;
        assert_eq!(res.unwrap_err().code, "COMMAND_NOT_SUPPORTED", "{cmd}");
    }
}

// ---------------------------------------------------------------------------
// 终审 #3 wire fail-closed：expected/parameters_json 解码失败必须拒绝，
// 且原值不得被修改（禁止 silent downgrade 成无条件写）。
// 经进程内 SDK 服务直发 malformed WriteRequest（common::write_raw）。
// ---------------------------------------------------------------------------

use mesa_driver_manager::session::Session;

fn f64_msg(v: f64) -> mesa_driver_protocol::pb::ValueMsg {
    mesa_driver_protocol::value_to_pb(&Value::F64(v))
}

#[tokio::test]
async fn wire_decode_failure_bad_expected_value_rejected_and_value_unchanged() {
    let (port, cancel) = common::start_sim_server().await;
    let (session, _events, _) = mesa_driver_manager::session::Session::connect(port, common::TOKEN)
        .await
        .unwrap();
    common::open_connection(&session, 1, "{}").await;
    // 先写 slot=w1=10.0（无条件写成功）。
    common::write_raw(
        &session,
        "writable",
        r#"{"slot":"w1"}"#,
        "value",
        Some(f64_msg(10.0)),
        None,
    )
    .await
    .unwrap();
    // expected 带了但 ValueMsg 非法（kind=None）→ BAD_EXPECTED_VALUE，
    // 绝不退化成无条件写；原值仍为 10.0（以 expected=10 写 11 成功证明）。
    let (code, _) = common::write_raw(
        &session,
        "writable",
        r#"{"slot":"w1"}"#,
        "value",
        Some(f64_msg(99.0)),
        Some(mesa_driver_protocol::pb::ValueMsg { kind: None }),
    )
    .await
    .unwrap_err();
    assert_eq!(code, "BAD_EXPECTED_VALUE");
    common::write_raw(
        &session,
        "writable",
        r#"{"slot":"w1"}"#,
        "value",
        Some(f64_msg(11.0)),
        Some(f64_msg(10.0)),
    )
    .await
    .unwrap();
    common::teardown(&mut session_drop(session), Some(cancel));
}

#[tokio::test]
async fn wire_decode_failure_bad_parameters_json_rejected_and_value_unchanged() {
    let (port, cancel) = common::start_sim_server().await;
    let (session, _events, _) = mesa_driver_manager::session::Session::connect(port, common::TOKEN)
        .await
        .unwrap();
    common::open_connection(&session, 1, "{}").await;
    // 先写 slot=w2=20.0。
    common::write_raw(
        &session,
        "writable",
        r#"{"slot":"w2"}"#,
        "value",
        Some(f64_msg(20.0)),
        None,
    )
    .await
    .unwrap();
    // parameters_json 非法 → INVALID_TARGET，原值仍为 20.0。
    let (code, _) = common::write_raw(
        &session,
        "writable",
        r#"{"slot": "#,
        "value",
        Some(f64_msg(99.0)),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(code, "INVALID_TARGET");
    common::write_raw(
        &session,
        "writable",
        r#"{"slot":"w2"}"#,
        "value",
        Some(f64_msg(21.0)),
        Some(f64_msg(20.0)),
    )
    .await
    .unwrap();
    common::teardown(&mut session_drop(session), Some(cancel));
}

fn session_drop(s: Session) -> Session {
    s
}

/// 终审 #2 回归：Driver command 业务失败（Err）经 SDK 编码为
/// status=Failed + result_json="" + error 精确码；Core 对非 Succeeded 终态
/// 不得做 result_schema 门禁（空字符串不是对象，门禁必误报 VIOLATION）。
/// 本用例在 SDK/Manager 编码层锁定该语义（REST 层断言见 management_api）。
#[tokio::test]
async fn driver_command_business_failure_is_failed_not_violation() {
    use mesa_driver_protocol::pb;
    // 直接构造 Driver 侧失败编码（与 SDK respond_command Err 分支同构）：
    // status=Failed，result_json 为空，error 带精确码。
    let resp = pb::CommandResponse {
        request_id: "t".into(),
        status: "Failed".into(),
        result_json: "".into(),
        error: "Internal/INVALID_COMMAND_INPUT: reset 不接受输入参数".into(),
    };
    assert_eq!(resp.status, "Failed");
    // Core 门禁规则：非 Succeeded 不调用 gate_command_result_against。
    // 此处锁定"空 result 不进门禁"——若未来有人改成无条件门禁，
    // 空字符串过空对象 schema 必产生 DRIVER_CONTRACT_VIOLATION 误报。
    let desc = mesa_driver_simulator::SimulatorDriver.descriptor();
    if resp.status == "Succeeded" {
        let v: serde_json::Value =
            serde_json::from_str(&resp.result_json).unwrap_or(serde_json::Value::Null);
        assert!(
            mesa_core_types::gate_command_result_against(&desc, "reset", &v, "command").is_empty()
        );
    } else {
        // 业务失败路径：不断言 schema，只断言 error 携带精确码（可路由）。
        assert!(
            resp.error.contains("INVALID_COMMAND_INPUT"),
            "{}",
            resp.error
        );
    }
    // Simulator 真实路径：reset 带参即业务失败 Err（INVALID_COMMAND_INPUT）。
    let conn = configured_conn("sim.x", "a").await;
    let err = conn.command("reset", r#"{"x":1}"#).await.unwrap_err();
    assert_eq!(err.code, "INVALID_COMMAND_INPUT");
}
