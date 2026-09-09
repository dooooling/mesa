//! Standalone process E2E（Gate F5）：二进制子进程 × 真实 TCP × `NckClient`。
//!
//! 每用例：spawn binary → 等监听 → 建连（COTP/Setup）→ NCK ReadVar →
//! 按场景断言 → kill/reap（守卫 drop 兜底，杜绝孤儿）。
//! 对端用 `NckClient`：F5 验证的是**进程边界与服务语义**，不是 codec
//! 独立正确性（后者归 F3 Sharp7 互操作；见 emulator 头能力矩阵）。

use std::net::SocketAddr;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use mesa_driver_sinumerik_nck::{NckClient, NckConnConfig, NckReadItem, NckWireAddress};

/// 子进程守卫：drop 即 kill + reap。
struct ChildGuard {
    inner: Option<Child>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.inner.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// 空闲端口（bind 即关的经典 race，回环测试可接受；监听等待另有 15s 上限）。
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("回环 bind")
        .local_addr()
        .expect("本机地址")
        .port()
}

fn spawn_emulator(port: u16, args: &[&str]) -> ChildGuard {
    let bin = env!("CARGO_BIN_EXE_mesa-nck-emulator");
    let mut cmd = Command::new(bin);
    cmd.arg("--port")
        .arg(port.to_string())
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let child = cmd.spawn().expect("emulator 子进程启动");
    ChildGuard { inner: Some(child) }
}

/// 等待监听（15s 上限；探测连接无数据即关，不影响后继会话）。
async fn wait_listen(port: u16) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return;
        }
        assert!(Instant::now() < deadline, "emulator 端口 {port} 15s 未监听");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn cfg(port: u16) -> NckConnConfig {
    NckConnConfig {
        host: "127.0.0.1".into(),
        port,
        family: "840d-sl".into(),
        local_tsap: 0x0100,
        remote_tsap: 0x0100,
        timeout_ms: 5000,
        requested_pdu_length: 480,
    }
}

fn item(linecount: u8) -> NckReadItem {
    NckReadItem {
        wire: NckWireAddress {
            syntax_id: 0x82,
            area_unit: 0x41,
            column: 42,
            line: 0,
            module: 0x12,
            line_count: linecount,
        },
        expected_data_len: linecount as usize * 8,
        expected_transport_size: 0x04,
    }
}

async fn connect(port: u16) -> NckClient {
    NckClient::connect(&cfg(port))
        .await
        .expect("跨进程 NCK 建连")
}

#[tokio::test]
async fn e2e_happy() {
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "happy", "--element-size", "8"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    assert_eq!(c.negotiated_pdu_length(), 480);
    let out = c.read_vars(&[item(1)]).await.expect("happy 读");
    let data = out[0].data.as_ref().expect("happy 必须 GOOD");
    assert_eq!(data.len(), 8);
    assert!(
        data.iter().all(|&b| b == 0x01),
        "序号 pattern，实际 {data:?}"
    );
}

#[tokio::test]
async fn e2e_partial_bad() {
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "partial_bad", "--fail-items", "0"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    let out = c
        .read_vars(&[item(1), item(1)])
        .await
        .expect("partial_bad 读");
    assert!(out[0].data.is_none(), "第 0 项（0-based）必须 BAD");
    assert_eq!(out[0].return_code, 0x05);
    assert!(out[1].data.is_some(), "第 1 项必须 GOOD");
}

#[tokio::test]
async fn e2e_wrong_transport() {
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "wrong_transport"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    let out = c.read_vars(&[item(1)]).await.expect("读返回");
    assert!(out[0].data.is_none(), "transport 不符必须 BAD");
    assert_eq!(out[0].return_code, 0xFF);
    assert_eq!(out[0].transport_size, 0x07);
}

#[tokio::test]
async fn e2e_short_payload() {
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "short_payload"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    let out = c.read_vars(&[item(1)]).await.expect("读返回");
    assert!(out[0].data.is_none(), "短包必须 BAD");
}

#[tokio::test]
async fn e2e_malformed() {
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "malformed"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    assert!(
        c.read_vars(&[item(1)]).await.is_err(),
        "截断包必须连接级失败"
    );
}

#[tokio::test]
async fn e2e_disconnect_after_n() {
    let port = free_port();
    let _guard = spawn_emulator(
        port,
        &[
            "--scenario",
            "disconnect_after_n",
            "--disconnect-after",
            "1",
        ],
    );
    wait_listen(port).await;
    let mut c = connect(port).await;
    let ok = c.read_vars(&[item(1)]).await.expect("首次读");
    assert!(ok[0].data.is_some());
    assert!(c.read_vars(&[item(1)]).await.is_err(), "断开后读必须失败");
}

#[tokio::test]
async fn e2e_pdu_240() {
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "pdu_240"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    assert_eq!(c.negotiated_pdu_length(), 240);
    let out = c.read_vars(&[item(1)]).await.expect("小 PDU 读");
    assert!(out[0].data.is_some());
}

#[tokio::test]
async fn e2e_pdu_480() {
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "pdu_480"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    assert_eq!(c.negotiated_pdu_length(), 480);
    let out = c.read_vars(&[item(1)]).await.expect("读");
    assert!(out[0].data.is_some());
}

#[tokio::test]
async fn e2e_pdu_960() {
    // 客户端请求 480 → min(480,960)=480：上限透传不断言 960 本身。
    let port = free_port();
    let _guard = spawn_emulator(port, &["--scenario", "pdu_960"]);
    wait_listen(port).await;
    let mut c = connect(port).await;
    assert_eq!(c.negotiated_pdu_length(), 480);
    let out = c.read_vars(&[item(1)]).await.expect("读");
    assert!(out[0].data.is_some());
}
