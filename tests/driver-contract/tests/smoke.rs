//! M0 冒烟子集（保留自首版）：Handshake 认证、Metadata、DataBatch 基础语义。
//! 全量 20 项基线见同目录其余测试文件（§21）。

mod common;

use std::time::Duration;

use mesa_core_types::Quality;
use mesa_driver_manager::session::{Session, SessionError};

use common::*;

#[tokio::test]
async fn handshake_token_mismatch_is_rejected() {
    let (port, cancel) = start_sim_server().await;
    let attempt =
        tokio::time::timeout(Duration::from_secs(5), Session::connect(port, "WRONG")).await;
    match attempt {
        Err(_) => panic!("handshake rejection should not time out"),
        Ok(Ok(_)) => panic!("wrong token must be rejected"),
        Ok(Err(e)) => assert_handshake_error(e),
    }
    cancel.cancel();
}

#[tokio::test]
async fn handshake_and_metadata_roundtrip() {
    let (port, cancel) = start_sim_server().await;
    let (session, _events, unresponsive) = Session::connect(port, TOKEN)
        .await
        .expect("handshake with correct token");
    assert!(!unresponsive.load(std::sync::atomic::Ordering::Relaxed));

    let (driver_id, name, version) = session.metadata().await.expect("metadata");
    assert_eq!(driver_id, "simulator");
    assert_eq!(name, "Mesa Simulator");
    assert!(!version.is_empty());

    teardown(&mut session_drop_guard(session), Some(cancel));
}

/// 包装以适配 teardown 的 &mut Session 签名（测试内直接 drop 即可，这里保持显式）。
fn session_drop_guard(s: Session) -> Session {
    s
}

/// DataBatch 语义（§10/§21）：epoch 回带、sequence 从 1 严格递增无缺口、
/// handle 盖戳、时间戳有效、point_id 来自映射、quality 缺省 GOOD。
#[tokio::test]
async fn databatch_epoch_sequence_semantics() {
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    const HANDLE: u32 = 7; // 非 1 值验证 handle 全链路透传而非硬编码
    const EPOCH: u64 = 0xDEAD_BEEF_1234;

    configure_and_start(
        &session,
        HANDLE,
        1,
        EPOCH,
        &[poll_task(
            "t1",
            50,
            serde_json::json!({
                "points": [
                    {"key":"k.counter","kind":"counter","step":2},
                    {"key":"k.toggle","kind":"toggle","initial":true}
                ]
            }),
        )],
        11,
    )
    .await;

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut batches: Vec<mesa_core_types::DataBatch> = Vec::new();
    while batches.len() < 5 && std::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline.into(), events.recv()).await {
            Ok(Some(mesa_driver_manager::session::SessionEvent::Batch(b))) => batches.push(b),
            Ok(other) => panic!("unexpected event: {other:?}"),
            Err(_) => break,
        }
    }
    assert!(
        batches.len() >= 3,
        "should receive several batches, got {}",
        batches.len()
    );

    let mut last_seq = 0;
    for b in &batches {
        assert_eq!(b.stream_epoch, EPOCH, "epoch must echo Start value");
        assert_eq!(b.connection_handle, HANDLE, "handle must be stamped by sdk");
        assert!(b.sequence > last_seq, "sequence strictly increasing");
        assert_eq!(
            b.sequence - last_seq,
            1,
            "no gap expected without backpressure"
        );
        last_seq = b.sequence;
        assert!(b.timestamp_ns > 0, "timestamp present");
        for pv in &b.values {
            // id_base=11 顺序分配：k.counter=11, k.toggle=12
            assert!(
                pv.point_id == 11 || pv.point_id == 12,
                "point_id must come from applied map"
            );
            assert_eq!(pv.quality, Quality::Good);
        }
    }

    teardown(&mut session, Some(server_cancel));
}

// 引用 SessionError 以确认公共断言工具与类型匹配（防止 common 演进时静默失配）
const _: fn(SessionError) = assert_handshake_error;
