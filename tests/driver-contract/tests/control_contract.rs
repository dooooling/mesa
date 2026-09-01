//! Control Plane 契约（§22）：可靠队列永不 Latest-Wins、默认 disabled、S7/OPC UA 能力分级。

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode, Value};
use mesa_driver_sdk::Driver;

fn poll_task_with_key(key: &str) -> AcquisitionTask {
    AcquisitionTask {
        id: "t".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: mesa_driver_simulator::BINDING_KIND.into(),
            config: serde_json::json!({"points": [{"key": key, "kind": "counter"}]}),
        },
    }
}

#[tokio::test]
async fn simulator_write_known_target_succeeds() {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task_with_key("sim.x")]).await.unwrap();
    conn.apply_point_map([("sim.x".to_string(), 1)].into_iter().collect()).await.unwrap();
    // known target
    let res = conn.write("sim.x", Value::F64(42.0), None).await;
    assert!(res.is_ok(), "write known target must succeed: {res:?}");
}

#[tokio::test]
async fn simulator_write_unknown_target_fails() {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task_with_key("sim.x")]).await.unwrap();
    conn.apply_point_map([("sim.x".to_string(), 1)].into_iter().collect()).await.unwrap();
    let res = conn.write("sim.unknown", Value::F64(1.0), None).await;
    assert!(res.is_err());
    let e = res.unwrap_err();
    assert_eq!(e.code, "TARGET_NOT_FOUND");
}

#[tokio::test]
async fn simulator_command_reset_succeeds() {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task_with_key("sim.x")]).await.unwrap();
    conn.apply_point_map([("sim.x".to_string(), 1)].into_iter().collect()).await.unwrap();
    let res = conn.command("reset", "{}").await;
    assert!(res.is_ok(), "reset command must succeed");
    let v = res.unwrap();
    assert_eq!(v["command"], "reset");
}

#[tokio::test]
async fn simulator_command_unsupported_fails() {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task_with_key("sim.x")]).await.unwrap();
    conn.apply_point_map([("sim.x".to_string(), 1)].into_iter().collect()).await.unwrap();
    let res = conn.command("nope", "{}").await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().code, "COMMAND_NOT_SUPPORTED");
}
