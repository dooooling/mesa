//! DataPlane-50K Soak 预检（Simulator only，无真机）
//! - CI 默认 10s 快速预检（-- --long 跑 60s 性能门禁，PERF_3000 跑 50min soak）
//! - 指标：point_value_total / elapsed >= 40_000/s (CI) / 50_000/s (long)，且无 FAILED

use std::time::{Duration, Instant};

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode};
use mesa_driver_manager::{MesaManager, PointIdAllocator};
use std::sync::Arc;

#[tokio::test]
async fn data_plane_50k_10s_ci() {
    let long_3000 = std::env::var("PERF_3000").is_ok();
    let soak = std::env::var("PERF_SOAK").is_ok();
    let long = std::env::var("PERF_LONG").is_ok() || std::env::args().any(|a| a == "--long");
    let dur = if soak {
        Duration::from_secs(3600) // 60min Release Soak
    } else if long_3000 {
        Duration::from_secs(3000) // 50min soak
    } else if long {
        Duration::from_secs(60) // 60s 性能门禁
    } else {
        Duration::from_secs(10)
    };
    // 使用内存 PointId 分配（与 conn_1000 一致），避免 SQLite 存量校验阻塞 Data Plane
    let source = Arc::new(PointIdAllocator::default());
    let drivers_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
    let mgr = MesaManager::with_source(&drivers_dir, source);
    // 注册一个高吞吐 endpoint：Simulator burst 模拟
    let tasks: Vec<AcquisitionTask> = (0..4).map(|i| AcquisitionTask {
        id: format!("t{i}"),
        mode: TaskMode::Poll,
        interval_ms: Some(20),
        binding: DriverBinding {
            kind: "simulator.points".into(),
            config: serde_json::json!({
                "points": (0..5).map(|j| serde_json::json!({"key": format!("p{i}_{j}"), "kind":"counter", "start":0, "step":1})).collect::<Vec<_>>(),
                "burst": 125
            }),
        },
    }).collect();
    let ep = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "perf-50k".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks,
    };
    mgr.start_endpoint(ep).unwrap();
    // 等待 RUNNING
    let snap = mgr.snapshot();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(st) = snap.endpoint("perf-50k") {
            if st.state == "RUNNING" {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let start_points = snap.point_value_total();
    let start = Instant::now();
    tokio::time::sleep(dur).await;
    let elapsed = start.elapsed().as_secs_f64();
    let ids = mgr.running_ids();
    let delta_points = snap.point_value_total().saturating_sub(start_points);
    let ups = delta_points as f64 / elapsed;
    assert!(elapsed >= 9.0, "elapsed {elapsed}");
    assert!(mgr.is_running("perf-50k"), "endpoint 不应退出 ids={ids:?}");
    assert!(!ids.is_empty(), "应有运行中 endpoint");
    // CI 10s 用 40k 门禁，long 60s/50min/60min 用 50k
    let threshold = if long || long_3000 || soak {
        50_000.0
    } else {
        40_000.0
    };
    assert!(
        ups >= threshold,
        "实际吞吐 {ups:.0}/s，低于 {threshold:.0}/s delta={delta_points} elapsed={elapsed:.1}s"
    );
    mgr.shutdown_all().await;
    println!(
        "data_plane_50k_10s_ci elapsed={elapsed:.1}s running={ids:?} ups={ups:.0} delta={delta_points}"
    );
}
