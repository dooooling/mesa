//! §21 心跳判死与优雅停机（会话级）。
//! 子进程级的 Graceful Shutdown / 孤儿防护见 subprocess_recovery.rs。

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use mesa_driver_manager::session::{HeartbeatParams, Session};
use mesa_driver_sdk::SdkFaults;

use common::*;

/// Heartbeat / Hang（§21 行 14）：驱动停止回 Pong 后，Core 在
/// ping_period × max_missed 量级的时间内将连接判为 unresponsive。
#[tokio::test]
async fn heartbeat_flags_unresponsive_driver() {
    let faults = SdkFaults::new();
    let (port, cancel) = start_sim_server_with_faults(faults.clone()).await;
    let (mut session, _events, unresponsive) = Session::connect_with_heartbeat(
        port,
        TOKEN,
        // 测试用短周期：100ms 周期 × 2 次丢失 ≈ 最快 ~200ms 判死
        HeartbeatParams {
            ping_period: Duration::from_millis(100),
            pong_deadline: Duration::from_millis(80),
            max_missed: 2,
        },
    )
    .await
    .unwrap();

    // 正常阶段不应误判
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !unresponsive.load(Ordering::Relaxed),
        "healthy driver must not be flagged"
    );

    // 注入 hang：请求循环停止处理任何入站帧（不回 Pong）
    faults.set_hang(true);
    wait_until(5, || unresponsive.load(Ordering::Relaxed)).await;

    session.invalidate();
    cancel.cancel();
}

/// Graceful Shutdown（§21 行 20，会话级）：Core 发送 Shutdown 消息后，
/// serve 循环干净退出（Ok）。NOTE: 不用事件通道关闭作判据——心跳任务持有
/// 事件发送端直至其 5s 周期结束，通道关闭滞后于 serve 实际退出。
#[tokio::test]
async fn shutdown_message_ends_server_cleanly() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let cancel = tokio_util::sync::CancellationToken::new();
    let c = cancel.clone();
    let server = tokio::spawn(async move {
        mesa_driver_sdk::serve(
            mesa_driver_simulator::SimulatorDriver,
            listener,
            TOKEN.into(),
            c,
        )
        .await
    });

    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();
    // 会话可用性预检
    let _ = session.metadata().await.expect("metadata before shutdown");

    use mesa_driver_protocol::pb::envelope::Body;
    session
        .post(Body::Shutdown(mesa_driver_protocol::pb::Shutdown {}))
        .await
        .unwrap();

    // serve 必须在宽限内干净返回
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), server)
        .await
        .expect("serve must finish promptly after Shutdown")
        .expect("join serve task");
    assert!(result.is_ok(), "shutdown path must not error: {result:?}");

    // Shutdown 后会话不可再用
    let next = session.metadata().await;
    assert!(next.is_err(), "session must be dead after driver exit");

    teardown(&mut session, Some(cancel));
}
