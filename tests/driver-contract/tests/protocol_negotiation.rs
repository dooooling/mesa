//! §21 Incompatible Version：协议 Major 不兼容时双方都必须明确拒绝（§14.3）。
//!
//! 使用裸帧客户端/服务端直接操纵 Hello/Welcome，验证协商规则本身。

mod common;

use std::time::Duration;

use mesa_driver_manager::session::Session;
use mesa_driver_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR, pb, read_envelope, write_envelope};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use common::*;

/// 驱动声明更高 Major → Core 必须拒绝握手（Session 侧 negotiate 失败）。
#[tokio::test]
async fn core_rejects_incompatible_driver_major() {
    // 假驱动：接受连接后发送 protocol_major + 1 的 Hello
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (mut rd, mut wr) = sock.into_split();
        if let Ok(hello) = read_envelope(&mut rd).await {
            let mut h = match hello.body {
                Some(pb::envelope::Body::Hello(h)) => h,
                _ => return,
            };
            h.protocol_major += 1; // 制造 Major 不兼容
            let env = pb::Envelope {
                msg_id: hello.msg_id,
                body: Some(pb::envelope::Body::Hello(h)),
            };
            let _ = write_envelope(&mut wr, &env).await;
            // 保持连接片刻，观察 Core 是否主动断开
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    let result = Session::connect(port, TOKEN).await;
    match result {
        Ok(_) => panic!("incompatible major must be rejected"),
        Err(e) => assert_handshake_error(e),
    }
}

/// Core 回 Welcome 声明更低 accepted_major → 驱动侧 serve 必须报错退出，
/// 而不是带着不兼容协议继续运行。
#[tokio::test]
async fn driver_rejects_core_major_downgrade() {
    // 进程内启动真实 SDK server
    let sim_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = sim_listener.local_addr().unwrap().port();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let server = tokio::spawn(async move {
        mesa_driver_sdk::serve(
            mesa_driver_simulator::SimulatorDriver,
            sim_listener,
            TOKEN.into(),
            c,
        )
        .await
    });

    // 裸 Core 客户端：读 Hello 后回一个 Major 不一致的 Welcome
    let sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (mut rd, mut wr) = sock.into_split();
    let hello = read_envelope(&mut rd).await.expect("sim sends hello first");
    let welcome = pb::Envelope {
        msg_id: hello.msg_id,
        body: Some(pb::envelope::Body::Welcome(pb::Welcome {
            core_version: "test-core".into(),
            accepted_protocol_major: PROTOCOL_MAJOR - 1, // 强制不兼容
            accepted_protocol_minor: PROTOCOL_MINOR,
        })),
    };
    write_envelope(&mut wr, &welcome).await.unwrap();

    let joined = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("serve must finish");
    let result = joined.expect("join serve task");
    let err = result.expect_err("serve must return Err on incompatible major");
    let msg = err.to_string();
    assert!(
        msg.contains("major") || msg.contains("rejected"),
        "error must mention version rejection: {msg}"
    );
}

/// 真 1.2 驱动（裸帧 Hello{1,2}，不经当前 SDK）：
/// Welcome 必须如实回 accepted_minor=2（协商值，不是 Core 自身 3）；
/// 该会话的 Event Gate 必须以协商值 2 执行（非空任务秒级精确拒绝）。
#[tokio::test]
async fn minor_negotiation_with_real_1_2_driver_is_honest() {
    use mesa_core_types::{DriverBinding, EventTask, TaskMode};
    use mesa_driver_manager::session::SessionError;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // 假 1.2 驱动：发 Hello{1,2} → 校验 Welcome → 保持连接供后续 RPC 门控验证
    let stub = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (mut rd, mut wr) = sock.into_split();
        let hello = pb::Envelope {
            msg_id: 1,
            body: Some(pb::envelope::Body::Hello(pb::Hello {
                driver_id: "legacy-1.2".into(),
                driver_version: "1.2.0".into(),
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: 2,
                sdk_version: "test".into(),
                platform: "test".into(),
                instance_id: "test-1".into(),
                session_token: TOKEN.into(),
            })),
        };
        write_envelope(&mut wr, &hello).await.unwrap();
        let welcome = read_envelope(&mut rd)
            .await
            .expect("core must answer Welcome");
        let w = match welcome.body {
            Some(pb::envelope::Body::Welcome(w)) => w,
            other => panic!("expected Welcome, got {other:?}"),
        };
        assert_eq!(w.accepted_protocol_major, PROTOCOL_MAJOR);
        assert_eq!(
            w.accepted_protocol_minor, 2,
            "Welcome 必须回协商 Minor=2，不能回 Core 自身 Minor"
        );
        // 保持连接：心跳 Ping 到达即回 Pong（让会话在断言期间存活）
        loop {
            let env = match read_envelope(&mut rd).await {
                Ok(e) => e,
                Err(_) => break,
            };
            if matches!(env.body, Some(pb::envelope::Body::Ping(_))) {
                let pong = pb::Envelope {
                    msg_id: env.msg_id,
                    body: Some(pb::envelope::Body::Pong(pb::Pong {})),
                };
                if write_envelope(&mut wr, &pong).await.is_err() {
                    break;
                }
            }
        }
    });

    let (mut session, _events, _) = Session::connect(port, TOKEN)
        .await
        .expect("1.2 handshake ok");
    assert_eq!(session.negotiated_minor(), 2, "协商值必须为双方较小值 2");
    // wire 级门控：该会话发非空事件任务必须秒级精确拒绝（不发未知 RPC 干等）
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        session.configure_events(
            7,
            1,
            &[EventTask {
                id: "e1".into(),
                mode: TaskMode::Subscribe,
                interval_ms: None,
                binding: DriverBinding {
                    kind: "k".into(),
                    config: serde_json::Value::Null,
                },
            }],
        ),
    )
    .await
    .expect("1.2 gate must answer fast")
    .expect_err("1.2 driver must reject event tasks");
    assert!(
        matches!(
            err,
            SessionError::EventPlaneUnsupported {
                negotiated: 2,
                required: 3
            }
        ),
        "必须精确报 EventPlaneUnsupported，got {err}"
    );

    teardown(&mut session, None);
    stub.abort();
}
