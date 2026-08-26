//! M0 合同测试冒烟子集（方案 §21 的三条核心项）。
//!
//! 采用进程内 SDK Server + Core Session 对连真实 TCP 的方式，
//! 覆盖握手认证与 DataBatch 语义两条关键链路，不依赖子进程 spawn。
//!
//! TODO(Phase 3): 补齐全量 20 项 Contract Test（§21），含真实子进程的
//! Parent-Death/Orphan Guard 与 Driver Crash Restore（当前由现场验收人工覆盖）。

use std::time::Duration;

use forgelink_core_types::{AcquisitionTask, DriverBinding, TaskMode};
use forgelink_driver_manager::session::{Session, SessionEvent};
use forgelink_driver_sdk::{serve, SdkServerError};
use forgelink_driver_simulator::SimulatorDriver;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "m0-smoke-token";

/// 启动一个进程内 Simulator SDK 服务，返回 (端口, 停机句柄)。
async fn start_sim_server() -> (u16, CancellationToken) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        // serve 出错仅记录：测试断言由客户端侧完成
        if let Err(e) = serve(SimulatorDriver, listener, TOKEN.into(), c.clone()).await {
            eprintln!("sim server ended: {e}");
        }
    });
    (port, cancel)
}

fn poll_task(points: serde_json::Value) -> AcquisitionTask {
    AcquisitionTask {
        id: "t1".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(50),
        binding: DriverBinding {
            kind: forgelink_driver_simulator::BINDING_KIND.into(),
            config: points,
        },
    }
}

#[tokio::test]
async fn handshake_token_mismatch_is_rejected() {
    let (port, _cancel) = start_sim_server().await;
    let attempt = tokio::time::timeout(Duration::from_secs(5), Session::connect(port, "WRONG")).await;
    match attempt {
        Err(_) => panic!("handshake rejection should not time out"),
        Ok(Ok(_)) => panic!("wrong token must be rejected"),
        Ok(Err(e)) => {
            assert!(
                matches!(e, forgelink_driver_manager::session::SessionError::Handshake(_)),
                "rejection reason must be handshake-level, got {e}"
            );
        }
    }
}

#[tokio::test]
async fn handshake_and_metadata_roundtrip() {
    let (port, _cancel) = start_sim_server().await;
    let (session, _events, unresponsive) =
        Session::connect(port, TOKEN).await.expect("handshake with correct token");
    assert!(!unresponsive.load(std::sync::atomic::Ordering::Relaxed));

    let (driver_id, name, version) = session.metadata().await.expect("metadata");
    assert_eq!(driver_id, "simulator");
    assert_eq!(name, "ForgeLink Simulator");
    assert!(!version.is_empty());
}

/// DataBatch 语义（§10/§21 DataBatch Semantics 行）：
/// epoch 原样回带、sequence 从 1 严格递增、handle 盖戳、时间戳有效、quality 缺省 GOOD。
#[tokio::test]
async fn databatch_epoch_sequence_semantics() {
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    const HANDLE: u32 = 7; // 非 1 值验证 handle 全链路透传而非硬编码
    const EPOCH: u64 = 0xDEAD_BEEF_1234;

    use forgelink_driver_protocol::pb::envelope::Body;

    // Open -> Configure -> ApplyPointMap -> Start 完整闭环
    let reply = session
        .call(Body::OpenConnection(forgelink_driver_protocol::pb::OpenConnection {
            connection_handle: HANDLE,
            endpoint_id: "e2e-smoke".into(),
            config_json: "{}".into(),
        }))
        .await
        .unwrap();
    assert!(matches!(reply.body, Some(Body::OpenConnectionAck(ref a)) if a.result.as_ref().unwrap().ok));

    let reply = session
        .call(Body::ConfigureTasks(forgelink_driver_protocol::pb::ConfigureTasks {
            connection_handle: HANDLE,
            revision: 1,
            tasks: forgelink_driver_protocol::tasks_to_pb(&[poll_task(serde_json::json!({
                "points": [
                    {"key":"k.counter","kind":"counter","step":2},
                    {"key":"k.toggle","kind":"toggle","initial":true}
                ]
            }))])
            .unwrap(),
        }))
        .await
        .unwrap();
    let descriptors = match reply.body {
        Some(Body::PointDescriptors(rep)) => rep.descriptors,
        other => panic!("expected PointDescriptors, got {other:?}"),
    };
    assert_eq!(descriptors.len(), 2, "two points registered");

    let map = std::collections::HashMap::from([("k.counter".to_string(), 11u32), ("k.toggle".to_string(), 22u32)]);
    let reply = session
        .call(Body::ApplyPointMap(forgelink_driver_protocol::pb::ApplyPointMap {
            connection_handle: HANDLE,
            revision: 1,
            key_to_point_id: map,
        }))
        .await
        .unwrap();
    assert!(matches!(reply.body, Some(Body::ConfigApplied(ref a)) if a.result.as_ref().unwrap().ok));

    let reply = session
        .call(Body::StartConnection(forgelink_driver_protocol::pb::StartConnection {
            connection_handle: HANDLE,
            stream_epoch: EPOCH,
        }))
        .await
        .unwrap();
    assert!(matches!(reply.body, Some(Body::StartConnectionAck(ref a)) if a.result.as_ref().unwrap().ok));

    // 收集批次并校验语义
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut batches: Vec<forgelink_core_types::DataBatch> = Vec::new();
    while batches.len() < 5 && std::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline.into(), events.recv()).await {
            Ok(Some(SessionEvent::Batch(b))) => batches.push(b),
            Ok(other) => panic!("unexpected event: {other:?}"),
            Err(_) => break,
        }
    }
    assert!(batches.len() >= 3, "should receive several batches, got {}", batches.len());

    let mut last_seq = 0;
    for b in &batches {
        assert_eq!(b.stream_epoch, EPOCH, "epoch must echo Start value");
        assert_eq!(b.connection_handle, HANDLE, "handle must be stamped by sdk");
        assert!(b.sequence > last_seq, "sequence strictly increasing");
        assert_eq!(b.sequence - last_seq, 1, "no gap expected without backpressure");
        last_seq = b.sequence;
        assert!(b.timestamp_ns > 0, "timestamp present");
        for pv in &b.values {
            assert!(
                pv.point_id == 11 || pv.point_id == 22,
                "point_id must come from applied map"
            );
            assert_eq!(pv.quality, forgelink_core_types::Quality::Good);
        }
    }

    // 收尾：停服务并等待退出
    session.invalidate();
    server_cancel.cancel();
}
