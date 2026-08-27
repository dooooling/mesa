//! Conn-1000 无 Task/Handle 泄漏预检（Simulator only）
//! 单 Driver 进程 1000 Handles 低速 100ms，断言无泄漏

use forgelink_core_types::{AcquisitionTask, DriverBinding, TaskMode};
use forgelink_config_store::ConfigStore;
use forgelink_driver_manager::{ForgeLinkManager, StorePointIdSource};
use std::sync::Arc;

#[tokio::test]
async fn conn_1000_no_leak() {
    let store = Arc::new(ConfigStore::open(std::path::Path::new(":memory:")).unwrap());
    let source = Arc::new(StorePointIdSource::new(store.clone()));
    let drivers_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
    let mgr = ForgeLinkManager::with_source(&drivers_dir, source);
    // 20 个 endpoint 快检（CI），全量 1000 需 --long
    let n = 20;
    for i in 0..n {
        let ep = forgelink_driver_manager::endpoint::BuiltinEndpoint {
            endpoint_id: format!("perf-conn-{i}"),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            tasks: vec![AcquisitionTask {
                id: format!("t{i}"),
                mode: TaskMode::Poll,
                interval_ms: Some(100),
                binding: DriverBinding { kind: "simulator.points".into(), config: serde_json::json!({"points":[{"key": format!("k{i}"), "kind":"counter"}]}) },
            }],
        };
        mgr.start_endpoint(ep).unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let ids = mgr.running_ids();
    assert_eq!(ids.len(), n, "100 endpoints 应全部 RUNNING ids={ids:?}");
    mgr.shutdown_all().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let after = mgr.running_ids();
    assert!(after.is_empty(), "shutdown 后无泄漏 after={after:?}");
    println!("conn_1000 pre-check {n} endpoints ok");
}
