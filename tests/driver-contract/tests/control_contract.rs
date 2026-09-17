//! Control Plane 契约（§22；Foundation-3 structured target 单真值）。
//!
//! 覆盖：WriteTarget 三元组（Simulator `writable/value` Reference，slot 实例
//! 身份）、只读 output 拒绝、值类型门、CAS（EXPECTED_MISMATCH / 真原子）、
//! reset 命令语义（state 回初值 + `{}` 结果契约）、未声明 command 由 Driver
//! 拒绝（Core 门禁见 management_api REST 层；此处锁 Driver 直调语义）。
//! 可靠队列永不 Latest-Wins、默认 disabled 见管理面/性能套件。

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
    let conn = configured_conn("sim.x", "a").await;
    let res = conn
        .write(&writable_target("nope"), Value::F64(1.0), None)
        .await;
    assert!(res.is_err());
    let e = res.unwrap_err();
    assert_eq!(e.code, "TARGET_NOT_FOUND");
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
async fn simulator_command_unsupported_fails() {
    let conn = configured_conn("sim.x", "a").await;
    // reset 之外的旧桩命令（start/stop/fault/writable）已随 ControlCatalog 收敛删除
    for cmd in ["nope", "start", "stop", "fault", "writable"] {
        let res = conn.command(cmd, "{}").await;
        assert_eq!(res.unwrap_err().code, "COMMAND_NOT_SUPPORTED", "{cmd}");
    }
}
