//! 挂起驱动桩（probe 超时清理合同测试专用）。
//!
//! 行为：读 stdin 第一行作 token → TCP 连 Core → 发 Hello → 读 Welcome →
//! 之后永远不再应答任何请求；stdin EOF 时立即退出（模仿 SDK 的 EOF 防护）。
//! 用于证明 `MesaManager::probe()` 在 RPC 超时后仍能回收子进程（无孤儿）。

use std::time::Duration;

use mesa_driver_protocol::{pb, read_envelope, write_envelope};

fn usage_port() -> u16 {
    let args: Vec<String> = std::env::args().collect();
    let s = args
        .get(2)
        .or_else(|| args.get(1))
        .expect("usage: hang_helper --port <PORT>");
    s.parse().expect("port must be u16")
}

#[tokio::main]
async fn main() {
    let port = usage_port();
    // stdin 第一行为 Core 注入的 session token（内容不校验，照抄进 Hello）
    let token = tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut s);
        s.trim().to_string()
    })
    .await
    .unwrap();

    // Driver 侧监听：Core（Session::connect_retry）会连进来读 Hello
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind port");
    let (stream, _) = listener.accept().await.expect("accept core");
    let (mut rd, mut wr) = stream.into_split();
    write_envelope(
        &mut wr,
        &pb::Envelope {
            msg_id: 1,
            body: Some(pb::envelope::Body::Hello(pb::Hello {
                driver_id: "hang".into(),
                driver_version: "0.0.0".into(),
                protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
                protocol_minor: mesa_driver_protocol::PROTOCOL_MINOR,
                sdk_version: "contract-test".into(),
                platform: std::env::consts::OS.into(),
                instance_id: "hang-1".into(),
                session_token: token,
            })),
        },
    )
    .await
    .expect("send hello");
    let welcome = read_envelope(&mut rd).await.expect("read welcome");
    assert!(
        matches!(welcome.body, Some(pb::envelope::Body::Welcome(_))),
        "expected Welcome, got {welcome:?}"
    );

    // stdin EOF 看门狗：Core terminate() 关闭管道后立即退出
    tokio::task::spawn_blocking(|| {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        let mut buf = String::new();
        loop {
            buf.clear();
            match std::io::BufRead::read_line(&mut lock, &mut buf) {
                Ok(0) => std::process::exit(0),
                Ok(_) => continue,
                Err(_) => std::process::exit(1),
            }
        }
    });
    // 挂起：永不读请求、永不回包（ProbeRequest 石沉大海 → Core 侧 RPC 超时）
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}
