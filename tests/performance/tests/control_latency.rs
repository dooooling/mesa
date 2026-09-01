//! Control 面 IPC 延迟（§22）：Write/Command 走可靠 Control 队列 p95≤20ms/p99≤50ms（单调时钟）
//! 30 samples 取 p95/p99，Simulator 本地环路应远低于预算

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode, Value};
use mesa_driver_sdk::Driver;

fn poll_task(key: &str) -> AcquisitionTask {
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

fn percentile(mut v: Vec<u128>, p: f64) -> u128 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let idx = ((p / 100.0) * (v.len() as f64 - 1.0)).round() as usize;
    v[idx.min(v.len() - 1)]
}

#[tokio::test]
async fn control_write_p95_within_20ms() {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task("sim.x")]).await.unwrap();
    conn.apply_point_map([("sim.x".to_string(), 1)].into_iter().collect())
        .await
        .unwrap();

    let mut samples = Vec::with_capacity(30);
    for _ in 0..30 {
        let start = std::time::Instant::now();
        conn.write("sim.x", Value::F64(1.0), None).await.unwrap();
        samples.push(start.elapsed().as_micros() / 1000);
    }
    let p95 = percentile(samples.clone(), 95.0);
    let p99 = percentile(samples, 99.0);
    println!("control_write p95={p95}ms p99={p99}ms");
    assert!(p95 <= 20, "write p95 {p95}ms >20ms");
    assert!(p99 <= 50, "write p99 {p99}ms >50ms");
}

#[tokio::test]
async fn control_command_p95_within_20ms() {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    conn.configure(1, vec![poll_task("sim.x")]).await.unwrap();
    conn.apply_point_map([("sim.x".to_string(), 1)].into_iter().collect())
        .await
        .unwrap();

    let mut samples = Vec::with_capacity(30);
    for _ in 0..30 {
        let start = std::time::Instant::now();
        conn.command("reset", "{}").await.unwrap();
        samples.push(start.elapsed().as_micros() / 1000);
    }
    let p95 = percentile(samples.clone(), 95.0);
    let p99 = percentile(samples, 99.0);
    println!("command p95={p95}ms p99={p99}ms");
    assert!(p95 <= 20, "command p95 {p95}ms >20ms");
    assert!(p99 <= 50, "command p99 {p99}ms >50ms");
}
