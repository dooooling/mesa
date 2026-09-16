//! Configure 编译性能（§13.1）：ResourceSelection→AcquisitionPlan
//! 预算：1K≤100ms /10K≤1s /50K≤5s p95（3 warmup +20 测）

use std::time::Instant;

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskSchedule, GENERIC_BINDING_KIND};
use mesa_driver_sdk::Driver;

fn make_task(n: usize, id: &str) -> AcquisitionTask {
    let sels: Vec<serde_json::Value> = (0..n)
        .map(|i| serde_json::json!({"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key": format!("p{i}")}]}))
        .collect();
    AcquisitionTask {
        id: id.into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({"selections": sels}),
        },
    }
}

async fn bench(n: usize) -> u128 {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let mut conn = driver.open_connection("ep", "{}").await.unwrap();
    // warmup
    for _ in 0..3 {
        let _ = conn.configure(1, vec![make_task(n, "t")]).await;
    }
    let mut samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let start = Instant::now();
        let _ = conn.configure(1, vec![make_task(n, "t")]).await.unwrap();
        samples.push(start.elapsed().as_millis());
    }
    samples.sort_unstable();
    let p95 = samples[(0.95 * (samples.len() as f64 - 1.0)).round() as usize];
    println!("configure {n} p95={p95}ms samples={samples:?}");
    p95
}

#[tokio::test]
async fn configure_1k_within_100ms() {
    let p95 = bench(1_000).await;
    assert!(p95 <= 100, "1K p95 {p95}ms >100ms");
}

#[tokio::test]
async fn configure_10k_within_1s() {
    let p95 = bench(10_000).await;
    assert!(p95 <= 1_000, "10K p95 {p95}ms >1s");
}

#[tokio::test]
async fn configure_50k_within_5s() {
    // 50K 资源较重，单生成 50K points 的 JSON 已占一定开销，预算放宽至 5s
    let p95 = bench(50_000).await;
    assert!(p95 <= 5_000, "50K p95 {p95}ms >5s");
}
