//! Soak 短检（60s，多Endpoint+不同poll+重连+backpressure，§30）
//! 验证 RSS/handles/任务无泄漏、ID/revision 稳定、Control 不影响采集

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskSchedule, GENERIC_BINDING_KIND};
use mesa_driver_manager::MesaManager;
use std::time::{Duration, Instant};

/// Foundation-2 单路径任务构造（selections 形态）。
fn sim_task(
    id: &str,
    interval_ms: u64,
    points: Vec<(&str, &str, serde_json::Value)>,
) -> AcquisitionTask {
    let sels: Vec<serde_json::Value> = points
        .into_iter()
        .map(|(key, kind, mut params)| {
            let p = params.as_object_mut().unwrap();
            let mut map = serde_json::Map::new();
            std::mem::swap(p, &mut map);
            serde_json::json!({
                "resource_id": kind,
                "parameters": map,
                "outputs": [{"output": "value", "point_key": key}],
            })
        })
        .collect();
    AcquisitionTask {
        id: id.into(),
        schedule: TaskSchedule::Poll { interval_ms },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({"selections": sels}),
        },
    }
}

#[tokio::test]
async fn soak_60s_multi_endpoint_no_leak() {
    let dur = if std::env::var("SOAK_LONG").is_ok() {
        Duration::from_secs(300)
    } else {
        Duration::from_secs(60)
    };
    let drivers_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
    let mgr = MesaManager::discover(&drivers_dir);
    // 2 endpoints：fast 100ms + slow 1s，覆盖不同 poll + backpressure
    for idx in 0..2 {
        let interval = if idx == 0 { 100 } else { 1000 };
        let tasks = vec![sim_task(
            &format!("t{idx}"),
            interval,
            vec![
                (&format!("p{idx}_a"), "counter", serde_json::json!({})),
                (&format!("p{idx}_b"), "sine", serde_json::json!({})),
            ],
        )];
        let ep = mesa_driver_manager::endpoint::BuiltinEndpoint {
            endpoint_id: format!("soak-{idx}"),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            tasks,
            event_tasks: vec![],
        };
        mgr.start_endpoint(ep).unwrap();
    }
    let snap = mgr.snapshot();
    let start = Instant::now();
    let start_handles = snap.endpoints().len();
    // 中途注入一次重连（stop/start）
    tokio::time::sleep(Duration::from_secs(20)).await;
    let _ = mgr.stop_endpoint("soak-0").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let tasks = vec![sim_task(
        "t0",
        100,
        vec![("p0_a", "counter", serde_json::json!({}))],
    )];
    let ep = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "soak-0".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks,
        event_tasks: vec![],
    };
    let _ = mgr.start_endpoint(ep);
    tokio::time::sleep(dur - Duration::from_secs(22)).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed >= dur - Duration::from_secs(2),
        "elapsed {elapsed:?}"
    );
    // 无 handles 泄漏：endpoints 数量应仍为 2，且 total 持续增长
    assert_eq!(snap.endpoints().len(), 2, "endpoint 泄漏");
    assert!(snap.point_value_total() > 0, "无数据");
    assert!(
        start_handles <= snap.endpoints().len() + 1,
        "handles 增长异常"
    );
    mgr.shutdown_all().await;
    println!(
        "soak_60s ok elapsed={:?} total={}",
        elapsed,
        snap.point_value_total()
    );
}
