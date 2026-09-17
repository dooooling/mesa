//! Control Plane 契约（§22；Foundation-3 structured target 单真值）。
//!
//! 覆盖：WriteTarget 三元组（Simulator `writable/value` Reference）、
//! 只读 output 拒绝、值类型门、CAS（EXPECTED_MISMATCH / 真原子）、
//! reset 命令语义（清 overlay）、未声明 command 由 Driver 拒绝
//!（Core 门禁见 management_api REST 层；此处锁 Driver 直调语义）。
//! 可靠队列永不 Latest-Wins、默认 disabled 见管理面/性能套件。

use mesa_core_types::{
    AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, TaskSchedule, Value, WriteTarget,
};
use mesa_driver_sdk::Driver;

fn poll_task_with_key(key: &str) -> AcquisitionTask {
    AcquisitionTask {
        id: "t".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({"selections": [{"resource_id":"writable","parameters":{"initial":1},"outputs":[{"output":"value","point_key": key}]}]}),
        },
    }
}

fn writable_target(point_key: &str) -> WriteTarget {
    WriteTarget {
        resource_id: "writable".into(),
        parameters: serde_json::json!({"point_key": point_key}),
        output: "value".into(),
    }
}

async fn configured_conn(key: &str) -> Box<dyn mesa_driver_sdk::DriverConnection> {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task_with_key(key)])
        .await
        .unwrap();
    conn.apply_point_map([(key.to_string(), 1)].into_iter().collect())
        .await
        .unwrap();
    conn
}

#[tokio::test]
async fn simulator_write_known_target_succeeds() {
    let mut conn = configured_conn("sim.x").await;
    // known target（三元组 + point_key 实例身份）
    let res = conn
        .write(&writable_target("sim.x"), Value::F64(42.0), None)
        .await;
    assert!(res.is_ok(), "write known target must succeed: {res:?}");
}

#[tokio::test]
async fn simulator_write_unknown_target_fails() {
    let mut conn = configured_conn("sim.x").await;
    let res = conn
        .write(&writable_target("sim.unknown"), Value::F64(1.0), None)
        .await;
    assert!(res.is_err());
    let e = res.unwrap_err();
    assert_eq!(e.code, "TARGET_NOT_FOUND");
}

#[tokio::test]
async fn simulator_write_readonly_output_rejected() {
    let mut conn = configured_conn("sim.x").await;
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
    let mut conn = configured_conn("sim.x").await;
    // writable/value 恒 F64：Bool 即拒绝
    let res = conn
        .write(&writable_target("sim.x"), Value::Bool(true), None)
        .await;
    assert_eq!(res.unwrap_err().code, "VALUE_TYPE_MISMATCH");
}

#[tokio::test]
async fn simulator_write_cas_mismatch_and_success() {
    let mut conn = configured_conn("sim.x").await;
    // 初值 initial=1：expected=2 即 CAS 失败
    let res = conn
        .write(
            &writable_target("sim.x"),
            Value::F64(9.0),
            Some(Value::F64(2.0)),
        )
        .await;
    assert_eq!(res.unwrap_err().code, "EXPECTED_MISMATCH");
    // expected=1 命中：写入 9.0 成功；再以 9.0 为期望写入 10.0（真 CAS 链）
    conn.write(
        &writable_target("sim.x"),
        Value::F64(9.0),
        Some(Value::F64(1.0)),
    )
    .await
    .unwrap();
    conn.write(
        &writable_target("sim.x"),
        Value::F64(10.0),
        Some(Value::F64(9.0)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn simulator_write_missing_point_key_rejected() {
    let mut conn = configured_conn("sim.x").await;
    // parameters 无 point_key 即实例身份缺失
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
    let mut conn = configured_conn("sim.x").await;
    // 先写入偏离初值，再 reset 回到快照初值（overlay 清空可观测）
    conn.write(&writable_target("sim.x"), Value::F64(42.0), None)
        .await
        .unwrap();
    let res = conn.command("reset", "{}").await;
    assert!(res.is_ok(), "reset command must succeed");
    let v = res.unwrap();
    assert_eq!(v["command"], "reset");
    // reset 后 CAS 期望回到初值 1.0 才能成功（overlay 已清空）
    conn.write(
        &writable_target("sim.x"),
        Value::F64(7.0),
        Some(Value::F64(1.0)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn simulator_command_unsupported_fails() {
    let mut conn = configured_conn("sim.x").await;
    // reset 之外的旧桩命令（start/stop/fault/writable）已随 ControlCatalog 收敛删除
    for cmd in ["nope", "start", "stop", "fault", "writable"] {
        let res = conn.command(cmd, "{}").await;
        assert_eq!(res.unwrap_err().code, "COMMAND_NOT_SUPPORTED", "{cmd}");
    }
}
