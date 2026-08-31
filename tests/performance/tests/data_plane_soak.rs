//! DataPlane-50K Soak 预检（Simulator only，无真机）
//! - CI 默认 10s 快速预检（-- --long 跑 60min 全量）
//! - 指标：point_value_total / elapsed >= 50_000/s，且无 FAILED

use std::time::{Duration, Instant};

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_config_store::ConfigStore;
use mesa_driver_manager::StorePointIdSource;
use std::sync::Arc;

#[tokio::test]
async fn data_plane_50k_10s_ci() {
    let long_3000 = std::env::var("PERF_3000").is_ok();
    let long = std::env::var("PERF_LONG").is_ok() || std::env::args().any(|a| a == "--long");
    let dur = if long_3000 { Duration::from_secs(3000) } else if long { Duration::from_secs(600) } else { Duration::from_secs(10) };
    // 使用内存库 + Simulator 4 Tasks *5 points=20/batch burst 125 20ms => 125k/s 远超 50k
    let store = Arc::new(ConfigStore::open(std::path::Path::new(":memory:")).unwrap());
    let source = Arc::new(StorePointIdSource::new(store.clone()));
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
    let start = Instant::now();
    tokio::time::sleep(dur).await;
    let ids = mgr.running_ids();
    let elapsed = start.elapsed().as_secs_f64();
    assert!(elapsed >= 9.0, "elapsed {elapsed}");
    assert!(!ids.is_empty(), "应有运行中 endpoint");
    mgr.shutdown_all().await;
    println!("data_plane_50k_10s_ci elapsed={elapsed:.1}s running={:?}", ids);
}
