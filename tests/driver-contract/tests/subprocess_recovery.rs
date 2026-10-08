//! 子进程级行为（§14/§17）：Manifest Discovery、token 注入、孤儿防护（EOF 层）、
//! Driver Crash Restore 端到端、子进程 Graceful Shutdown。
//!
//! Windows Job Object 的"Core 死亡连带清理"无法在测试进程内自证（进程已死），
//! 该层由 M0 现场验收人工覆盖；此处覆盖 liveness EOF 与恢复闭环。

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mesa_core_types::ConnectionState;
use mesa_driver_manager::endpoint::{
    BuiltinEndpoint, PointIdAllocator, PointIdSource, run_endpoint_with_heartbeat,
};
use mesa_driver_manager::manifest::{DiscoveredDriver, scan_drivers};
use mesa_driver_manager::process::TERMINATE_GRACE;
use mesa_driver_manager::session::{HeartbeatParams, Session};
use mesa_driver_manager::snapshot::Snapshot;
use tokio_util::sync::CancellationToken;

/// staged test-driver 目录：从已构建 test-driver 二进制拷贝 + canonical
/// driver.toml（单真值，不手写 version/protocol），供 discovery/launch
/// 测试（不依赖正式 drivers/ 目录）。
fn staged_test_driver_dir() -> PathBuf {
    let exe = common::test_driver_exe();
    let root = std::env::temp_dir().join(format!(
        "mesa-test-driver-discover-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let dir = root.join("test-driver");
    std::fs::create_dir_all(&dir).unwrap();
    let name = exe.file_name().unwrap().to_string_lossy().to_string();
    std::fs::copy(&exe, dir.join(&name)).unwrap();
    std::fs::copy(
        common::repo_root().join("tests/support/test-driver/driver.toml"),
        dir.join("driver.toml"),
    )
    .unwrap();
    root
}

use common::*;

/// Manifest Discovery（§21 行 1）：staged test-driver 目录能发现 test-driver，
/// 字段合法且可启动（不再扫描正式 drivers/ 目录——test-driver 已退役产品身份）。
#[test]
fn manifest_discovery_finds_test_driver() {
    let root = staged_test_driver_dir();
    let found = scan_drivers(&root);
    let sim = found
        .iter()
        .find(|d| d.manifest.id == "test-driver")
        .expect("test-driver must be discovered");
    assert_eq!(sim.manifest.name, "Mesa Test Driver");
    assert!(
        sim.manifest.version.split('.').count() == 3,
        "version must be x.y.z shaped"
    );
    assert!(sim.protocol_ok, "protocol major must match core");
    assert!(
        sim.platform_ok,
        "host platform must match manifest constraints"
    );
    assert!(
        sim.launchable(),
        "simulator binary must resolve after build"
    );
}

/// 手工构造指向已构建二进制的 DiscoveredDriver（跳过目录扫描，聚焦行为本身）。
fn sim_discovered() -> DiscoveredDriver {
    let exe = test_driver_exe();
    DiscoveredDriver {
        manifest: mesa_driver_manager::manifest::DriverManifest {
            id: "test-driver".into(),
            name: "Mesa Test Driver".into(),
            version: "0.0.0".into(), // 测试桩版本，仅用于 Hello 展示
            executable: exe.file_name().unwrap().to_string_lossy().to_string(),
            protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
            protocol_minor: mesa_driver_protocol::PROTOCOL_MINOR,
            sdk: None,
            os: None,
            arch: None,
        },
        manifest_dir: PathBuf::new(),
        executable_path: Some(exe),
        platform_ok: true,
        platform_reason: None,
        protocol_ok: true,
    }
}

/// 孤儿防护 EOF 层（§21 行 15）：terminate 先关 stdin 触发驱动 EOF 自杀，
/// 进程必须在远小于强杀宽限的时间内自然退出——否则说明 EOF 防护失效。
#[tokio::test]
async fn orphan_guard_stdin_eof_exits_child_quickly() {
    let process = mesa_driver_manager::process::DriverProcess::spawn(&sim_discovered())
        .await
        .expect("spawn test-driver");
    let mut process = process;
    let pid = process.pid;

    let started = Instant::now();
    process.terminate().await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < TERMINATE_GRACE - Duration::from_secs(1),
        "child {pid} must exit via stdin EOF well before force-kill grace, took {elapsed:?}"
    );
}

/// token 注入端到端：真实子进程 + 正确 token 可握手；错误 token 被拒。
#[tokio::test]
async fn subprocess_token_handshake_paths() {
    // 错误 token：握手必须失败且为握手层错误（spawn 后需 connect_retry 处理 listen 窗口竞态）
    let mut p = mesa_driver_manager::process::DriverProcess::spawn(&sim_discovered())
        .await
        .expect("spawn");
    let port = p.port;
    let res = Session::connect_retry(port, "definitely-wrong").await;
    match res {
        Ok(_) => panic!("wrong token against real binary must be rejected"),
        Err(e) => assert_handshake_error(e),
    }
    p.terminate().await;

    // 正确 token：完整会话可用
    let mut p = mesa_driver_manager::process::DriverProcess::spawn(&sim_discovered())
        .await
        .expect("spawn");
    let (mut session, _events, _) = Session::connect_retry(p.port, &p.token)
        .await
        .expect("handshake with injected token");
    let (driver_id, _, _) = session.metadata().await.expect("metadata");
    assert_eq!(driver_id, "test-driver");
    teardown(&mut session, None);
    p.terminate().await;
}

/// Driver Crash Restore（§21 行 16）+ Reconnect（§21 行 13）端到端：
/// 通过 faults.crash_after_batches 让真实驱动进程自杀，验证 Endpoint 运行时
/// 完成退避重连 + 配置重放 + Point ID 稳定 + 新 epoch。
#[tokio::test]
async fn driver_crash_restore_via_endpoint_runtime() {
    common::init_log();
    // 本测试需要快速判死：20s 恢复预算按 ~2s 判死标定。心跳参数显式传入
    // endpoint runtime（1s/1s×2），不读进程 env、不污染同进程其他测试。
    let heartbeat = HeartbeatParams {
        ping_period: Duration::from_secs(1),
        pong_deadline: Duration::from_secs(1),
        max_missed: 2,
    };
    let snapshot = std::sync::Arc::new(Snapshot::new());
    let allocator: std::sync::Arc<dyn PointIdSource> =
        std::sync::Arc::new(PointIdAllocator::default());
    let shutdown = CancellationToken::new();

    // 第 3 批后进程退出（interval 40ms ⇒ ~120ms 时崩溃）
    let cfg = BuiltinEndpoint {
        endpoint_id: "ct-crash".into(),
        driver_id: "test-driver".into(),
        connection_json: r#"{"crash_after_batches":3}"#.into(),
        tasks: vec![poll_task(
            "t",
            40,
            serde_json::json!([
                {"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"c.a"}]},
                {"resource_id":"constant","parameters":{"value":7},"outputs":[{"output":"value","point_key":"c.b"}]}
            ]),
        )],
        event_tasks: vec![],
    };

    let disc = sim_discovered();
    let snap = snapshot.clone();
    let registry: std::sync::Arc<
        std::sync::RwLock<
            std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<Session>>>,
        >,
    > = std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
    let rt = tokio::spawn(run_endpoint_with_heartbeat(
        disc,
        cfg.clone(),
        snap,
        allocator,
        shutdown.clone(),
        registry,
        // Data-only（无 EventServices）：复用生产 Data 路径做 crash 恢复验证
        None,
        heartbeat,
    ));

    // 第一次运行：RUNNING 且 epoch != 0，记录点集与 epoch
    wait_until(10, || {
        matches!(
            snapshot.endpoint("ct-crash"),
            Some(ref s) if s.state == ConnectionState::Running.as_str() && s.epoch != 0
        )
    })
    .await;
    let epoch1 = snapshot.endpoint("ct-crash").unwrap().epoch;
    wait_until(10, || snapshot.latest_all().len() == 2).await;
    let points_before: Vec<u32> = snapshot.latest_all().iter().map(|e| e.point_id).collect();
    assert_eq!(points_before.len(), 2, "two points registered");
    let ts_before = snapshot
        .latest_all()
        .iter()
        .map(|e| e.timestamp_ns)
        .collect::<Vec<_>>();

    // 崩溃后：状态离开 RUNNING（RECONNECTING），随后自动恢复到 RUNNING 且新 epoch
    wait_until(20, || {
        matches!(
            snapshot.endpoint("ct-crash"),
            Some(ref s) if s.epoch != epoch1 && s.state == ConnectionState::Running.as_str()
        )
    })
    .await;
    let epoch2 = snapshot.endpoint("ct-crash").unwrap().epoch;
    assert_ne!(epoch1, epoch2, "restore must establish a new stream_epoch");

    // Point ID 稳定性：重启后仍是同一组 id，且数据在更新（时间戳前进）
    let points_after: Vec<u32> = snapshot.latest_all().iter().map(|e| e.point_id).collect();
    assert_eq!(
        points_before, points_after,
        "point ids must survive driver restart"
    );

    let ts_stable_start = Instant::now();
    wait_until(10, || {
        snapshot
            .latest_all()
            .iter()
            .any(|e| e.timestamp_ns > ts_before[0])
            || snapshot
                .latest_all()
                .iter()
                .enumerate()
                .any(|(i, e)| e.timestamp_ns > ts_before[i])
    })
    .await;
    assert!(
        ts_stable_start.elapsed() < Duration::from_secs(10),
        "data must resume flowing after restore"
    );
    // 恢复后质量回到 GOOD（断线窗口曾被置 BAD）
    wait_until(5, || {
        snapshot.latest_all().iter().all(|e| e.quality == "GOOD")
    })
    .await;

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(35), rt).await; // 退避 sleep 最长 30s
}

/// 子进程 Graceful Shutdown（§21 行 20）：Shutdown 消息后进程在宽限内自然退出。
#[tokio::test]
async fn graceful_shutdown_terminates_subprocess_promptly() {
    let mut p = mesa_driver_manager::process::DriverProcess::spawn(&sim_discovered())
        .await
        .expect("spawn");
    let (session, _events, _) = Session::connect_retry(p.port, &p.token).await.unwrap();

    use mesa_driver_protocol::pb::envelope::Body;
    session
        .post(Body::Shutdown(mesa_driver_protocol::pb::Shutdown {}))
        .await
        .unwrap();
    drop(session);

    let started = Instant::now();
    p.terminate().await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "driver must exit promptly on Shutdown message, took {:?}",
        started.elapsed()
    );
}
