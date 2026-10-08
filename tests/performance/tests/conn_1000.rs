//! Conn-1000 无 Task/Handle 泄漏预检（Simulator only）
//! 20 Endpoints × 20 Driver Processes 低速 100ms，断言无泄漏（非单进程 1000 Handles）

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
async fn conn_1000_no_leak() {
    let source = Arc::new(PointIdAllocator::default());
    let _staged = staged_drivers_with_test_driver();
    let drivers_dir = _staged.path().to_path_buf();
    let mgr = MesaManager::with_source(&drivers_dir, source);
    // 20 个 endpoint 快检（CI），全量 1000 需 --long
    let n = 20;
    for i in 0..n {
        let ep = mesa_driver_manager::endpoint::BuiltinEndpoint {
            endpoint_id: format!("perf-conn-{i}"),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            tasks: vec![AcquisitionTask {
                id: format!("t{i}"),
                schedule: TaskSchedule::Poll { interval_ms: 100 },
                binding: DriverBinding {
                    kind: GENERIC_BINDING_KIND.into(),
                    config: serde_json::json!({"selections": [{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key": format!("k{i}")}]}]}),
                },
            }],
            event_tasks: vec![],
        };
        mgr.start_endpoint(ep).unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let ids = mgr.running_ids();
    assert_eq!(ids.len(), n, "{n} endpoints 应全部 RUNNING ids={ids:?}");
    mgr.shutdown_all().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let after = mgr.running_ids();
    assert!(after.is_empty(), "shutdown 后无泄漏 after={after:?}");
    println!("conn_1000 pre-check {n} endpoints ok");
}
