//! Contract Test 公共基建。
//!
//! 两类测试形态（§20 四层中的两层）：
//! - 进程内：SDK `serve_with_faults` + Core `Session` 对连真实 TCP，快速覆盖协议语义；
//! - 子进程：拉起真实 simulator 二进制 + `run_endpoint` 运行时，覆盖孤儿防护、
//!   Crash Restore 等进程级行为。

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use mesa_core_types::{AcquisitionTask, DriverBinding, PointDescriptor, TaskMode};
use mesa_driver_manager::session::{Session, SessionEvent, SessionError};
use mesa_driver_protocol::pb;
use mesa_driver_sdk::{SdkFaults, serve_with_faults};
use mesa_driver_simulator::SimulatorDriver;
use tokio_util::sync::CancellationToken;

pub const TOKEN: &str = "contract-test-token";

/// 初始化测试日志（仅输出 WARN 以上到 stderr，避免淹没断言信息；
/// 需要详细排查时设 RUST_LOG=debug 重跑）。
pub fn init_log() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_test_writer()
        .try_init();
}

/// 仓库根目录（workspace 根）。
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// 已构建的 simulator 可执行文件路径。
///
/// NOTE(cargo 行为): `cargo test -p mesa-contract-tests` 只构建依赖包的 lib，
/// **不会**重编 simulator 的 bin。子进程类测试前若改过驱动代码，
/// 必须先 `cargo build -p Mesa-driver-simulator`（或 `--workspace`），
/// 否则拉起的是旧二进制、故障注入不生效。
pub fn sim_exe() -> PathBuf {
    let target = repo_root().join("target");
    for profile in ["debug", "release"] {
        for name in ["Mesa-driver-simulator.exe", "Mesa-driver-simulator"] {
            let p = target.join(profile).join(&name);
            if p.is_file() {
                return p;
            }
        }
    }
    panic!("simulator binary not built; run cargo build/test first");
}

/// 启动进程内 Simulator SDK 服务（无故障注入），返回 (端口, 停机句柄)。
pub async fn start_sim_server() -> (u16, CancellationToken) {
    start_sim_server_with_faults(SdkFaults::new()).await
}

/// 启动进程内 Simulator SDK 服务并暴露故障注入开关。
/// 10055/10048 缓冲区耗尽时重试，避免并行 cargo test 抖动。
pub async fn start_sim_server_with_faults(faults: SdkFaults) -> (u16, CancellationToken) {
    let mut last_err = None;
    for i in 0..10 {
        match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
            Ok(listener) => {
                let port = listener.local_addr().unwrap().port();
                let cancel = CancellationToken::new();
                let c = cancel.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_with_faults(SimulatorDriver, listener, TOKEN.into(), c.clone(), Some(faults)).await {
                        eprintln!("sim server ended: {e}");
                    }
                });
                return (port, cancel);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse || e.raw_os_error() == Some(10055) || e.raw_os_error() == Some(10048) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(50 * (i + 1))).await;
                continue;
            }
            Err(e) => panic!("bind failed: {e}"),
        }
    }
    panic!("bind retry exhausted: {:?}", last_err);
}

/// 构造一个 Poll 任务，binding 为 simulator.points。
pub fn poll_task(id: &str, interval_ms: u64, points: serde_json::Value) -> AcquisitionTask {
    AcquisitionTask {
        id: id.into(),
        mode: TaskMode::Poll,
        interval_ms: Some(interval_ms),
        binding: DriverBinding {
            kind: mesa_driver_simulator::BINDING_KIND.into(),
            config: points,
        },
    }
}

// ---------------------------------------------------------------------------
// 协议化请求辅助：让测试以"Core 的方式"驱动完整配置闭环
// ---------------------------------------------------------------------------

/// OpenConnection 并断言成功。
pub async fn open_connection(session: &Session, handle: u32, config_json: &str) {
    let reply = session
        .call(pb::envelope::Body::OpenConnection(pb::OpenConnection {
            connection_handle: handle,
            endpoint_id: format!("ct-{handle}"),
            config_json: config_json.into(),
        }))
        .await
        .expect("open rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::OpenConnectionAck(ref a)) if ack_ok(a.result.as_ref())),
        "OpenConnection must succeed, got {reply:?}"
    );
}

/// ConfigureTasks 并返回 (point_key -> 上报描述符)。
pub async fn configure_tasks(
    session: &Session,
    handle: u32,
    revision: u64,
    tasks: &[AcquisitionTask],
) -> Vec<PointDescriptor> {
    let tasks_pb = mesa_driver_protocol::tasks_to_pb(tasks).expect("tasks to pb");
    let reply = session
        .call(pb::envelope::Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: handle,
            revision,
            tasks: tasks_pb,
        }))
        .await
        .expect("configure rpc");
    match reply.body {
        Some(pb::envelope::Body::PointDescriptors(rep)) => rep
            .descriptors
            .into_iter()
            .map(|d| PointDescriptor {
                point_key: d.point_key,
                data_type: d.data_type.parse().expect("valid data type"),
                unit: d.unit,
            })
            .collect(),
        other => panic!("expected PointDescriptors, got {other:?}"),
    }
}

/// ApplyPointMap 并断言成功。
pub async fn apply_point_map(session: &Session, handle: u32, revision: u64, map: HashMap<String, u32>) {
    let reply = session
        .call(pb::envelope::Body::ApplyPointMap(pb::ApplyPointMap {
            connection_handle: handle,
            revision,
            key_to_point_id: map,
        }))
        .await
        .expect("apply rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::ConfigApplied(ref a)) if ack_ok(a.result.as_ref())),
        "ApplyPointMap must succeed"
    );
}

/// StartConnection 并断言成功。
pub async fn start_connection(session: &Session, handle: u32, epoch: u64) {
    let reply = session
        .call(pb::envelope::Body::StartConnection(pb::StartConnection {
            connection_handle: handle,
            stream_epoch: epoch,
        }))
        .await
        .expect("start rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::StartConnectionAck(ref a)) if ack_ok(a.result.as_ref())),
        "StartConnection must succeed"
    );
}

/// StopConnection 并断言 Ack 成功（无论此前是否在运行）。
pub async fn stop_connection(session: &Session, handle: u32) -> bool {
    let reply = session
        .call(pb::envelope::Body::StopConnection(pb::StopConnection { connection_handle: handle }))
        .await
        .expect("stop rpc");
    match reply.body {
        Some(pb::envelope::Body::StopConnectionAck(a)) => ack_ok(a.result.as_ref()),
        _ => false,
    }
}

/// CloseConnection 并断言成功。
pub async fn close_connection(session: &Session, handle: u32) {
    let reply = session
        .call(pb::envelope::Body::CloseConnection(pb::CloseConnection { connection_handle: handle }))
        .await
        .expect("close rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::CloseConnectionAck(ref a)) if ack_ok(a.result.as_ref())),
        "CloseConnection must succeed"
    );
}

fn ack_ok(result: Option<&pb::GenericResult>) -> bool {
    result.map(|r| r.ok).unwrap_or(false)
}

/// 从 descriptors 生成分配映射（模拟 Core 分配 point_id：从 base 起顺序编号）。
pub fn sequential_ids(descriptors: &[PointDescriptor], base: u32) -> HashMap<String, u32> {
    descriptors
        .iter()
        .enumerate()
        .map(|(i, d)| (d.point_key.clone(), base + i as u32))
        .collect()
}

/// 完整配置闭环：Open -> Configure -> Apply -> Start。
pub async fn configure_and_start(
    session: &Session,
    handle: u32,
    revision: u64,
    epoch: u64,
    tasks: &[AcquisitionTask],
    id_base: u32,
) -> Vec<PointDescriptor> {
    open_connection(session, handle, "{}").await;
    let descriptors = configure_tasks(session, handle, revision, tasks).await;
    let map = sequential_ids(&descriptors, id_base);
    apply_point_map(session, handle, revision, map).await;
    start_connection(session, handle, epoch).await;
    descriptors
}

// ---------------------------------------------------------------------------
// 事件流辅助
// ---------------------------------------------------------------------------

/// 在 deadline 内收取一个批次；超时 panic。
/// State/DriverError 事件被静默跳过——断言错误事件用 [`expect_driver_error`]，
/// 这里只关心数据面。
pub async fn recv_batch(events: &mut tokio::sync::mpsc::Receiver<SessionEvent>, secs: u64) -> mesa_core_types::DataBatch {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let remain = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remain, events.recv()).await {
            Ok(Some(SessionEvent::Batch(b))) => return b,
            Ok(Some(_)) => continue, // State / DriverError：非本断言目标
            Ok(other) => panic!("expected batch event, got {other:?}"),
            Err(_) => panic!("timed out waiting for batch"),
        }
    }
}

/// 在 deadline 内收取下一个 DriverError 事件；超时/其他事件则继续等待。
pub async fn expect_driver_error(
    session: &Session,
    events: &mut tokio::sync::mpsc::Receiver<SessionEvent>,
    secs: u64,
) -> (String, String, String) {
    let deadline = Duration::from_secs(secs);
    loop {
        match tokio::time::timeout(deadline, events.recv()).await {
            Ok(Some(SessionEvent::DriverError { kind, code, message, .. })) => {
                return (kind, code, message);
            }
            // 配置失败也可能以错误帧之外的路径表现：请求侧直接报错
            Ok(other) => {
                // 心跳等事件可忽略；若会话已死则直接失败
                if other.is_none() {
                    panic!("session closed before driver error arrived");
                }
                let _ = session; // 保持签名一致性
            }
            Err(_) => panic!("timed out waiting for driver error"),
        }
    }
}

/// 等待条件成立（轮询），超时 panic。
pub async fn wait_until<F>(secs: u64, mut cond: F)
where
    F: FnMut() -> bool,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not met within {secs}s");
}

/// 统一会话收尾。
pub fn teardown(session: &mut Session, server_cancel: Option<CancellationToken>) {
    session.invalidate();
    if let Some(c) = server_cancel {
        c.cancel();
    }
}

/// 断言错误为握手层拒绝。
pub fn assert_handshake_error(e: SessionError) {
    assert!(
        matches!(e, SessionError::Handshake(_)),
        "rejection must be handshake-level, got {e}"
    );
}
