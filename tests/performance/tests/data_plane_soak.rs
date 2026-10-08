//! DataPlane-50K Soak 预检（Simulator only，无真机）
//! - CI 默认 10s 快速预检（-- --long 跑 60s 性能门禁，PERF_3000 跑 50min soak）
//! - 指标：point_value_total / elapsed >= 40_000/s (CI) / 50_000/s (long)，且无 FAILED

use std::time::{Duration, Instant};

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskSchedule, GENERIC_BINDING_KIND};
use mesa_driver_manager::{MesaManager, PointIdAllocator};
use std::sync::Arc;

/// staged drivers 目录（含 test-driver 二进制 + manifest；perf 用）。
/// guard 由调用方持有（drop 即删目录）。
struct PerfStagedDir(std::path::PathBuf);
impl PerfStagedDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for PerfStagedDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn staged_drivers_with_test_driver() -> PerfStagedDir {
    let exe_candidates = [
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/mesa-test-driver.exe"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/mesa-test-driver"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/release/mesa-test-driver.exe"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/release/mesa-test-driver"),
    ];
    let exe = exe_candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .expect("mesa-test-driver binary not built");
    let root = std::env::temp_dir().join(format!(
        "mesa-perf-drivers-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let dir = root.join("test-driver");
    std::fs::create_dir_all(&dir).unwrap();
    let name = exe.file_name().unwrap().to_string_lossy().to_string();
    std::fs::copy(&exe, dir.join(&name)).unwrap();
    // 单真值：复制 canonical manifest（tests/support/test-driver/driver.toml）。
    std::fs::copy(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../support/test-driver/driver.toml"),
        dir.join("driver.toml"),
    )
    .unwrap();
    PerfStagedDir(root)
}

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
    let _staged = staged_drivers_with_test_driver();
    let drivers_dir = _staged.path().to_path_buf();
    let mgr = MesaManager::with_source(&drivers_dir, source);
    // 注册一个高吞吐 endpoint：Simulator burst 模拟
    //（burst 为 GenericBinding 顶层参数，与 selections 同级）
    let tasks: Vec<AcquisitionTask> = (0..4).map(|i| AcquisitionTask {
        id: format!("t{i}"),
        schedule: TaskSchedule::Poll { interval_ms: 20 },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({
                "selections": (0..5).map(|j| serde_json::json!({"resource_id":"counter","parameters":{"start":0,"step":1},"outputs":[{"output":"value","point_key": format!("p{i}_{j}")}]})).collect::<Vec<_>>(),
                "burst": 125
            }),
        },
    }).collect();
    let ep = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "perf-50k".into(),
        driver_id: "test-driver".into(),
        connection_json: "{}".into(),
        tasks,
        event_tasks: vec![],
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
