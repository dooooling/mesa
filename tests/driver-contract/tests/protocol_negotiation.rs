//! §21 Incompatible Version：协议 Major 不兼容时双方都必须明确拒绝（§14.3）。
//!
//! 使用裸帧客户端/服务端直接操纵 Hello/Welcome，验证协商规则本身。

mod common;

use std::time::Duration;

use mesa_driver_protocol::{pb, read_envelope, write_envelope, PROTOCOL_MAJOR, PROTOCOL_MINOR};
use mesa_driver_manager::session::Session;
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
            let env =
                pb::Envelope { msg_id: hello.msg_id, body: Some(pb::envelope::Body::Hello(h)) };
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
    let sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
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

    let joined =
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("serve must finish");
    let result = joined.expect("join serve task");
    let err = result.expect_err("serve must return Err on incompatible major");
    let msg = err.to_string();
    assert!(
        msg.contains("major") || msg.contains("rejected"),
        "error must mention version rejection: {msg}"
    );
}
