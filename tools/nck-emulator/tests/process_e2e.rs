//! Standalone process E2E（Gate F5）：二进制子进程 × 真实 TCP × `NckClient`。
//!
//! 每用例：spawn binary（`--port 0`，OS 原子分配）→ 读子进程自报 READY
//! 地址 → 建连（COTP/Setup）→ NCK ReadVar → 按场景断言。
//! 端口约定（冻结）：**禁止预占端口**——`free_port→release→bind` 是 TOCTOU，
//! main CI #120 实证（并行测试端口复用 → 子进程 bind 失败退出 → 误连他进程
//! 端口 → kill 后 `Connection refused`）。只认目标子进程自己的 READY 行。
//! 对端用 `NckClient`：F5 验证的是**进程边界与服务语义**，不是 codec
//! 独立正确性（后者归 F3 Sharp7 互操作；见 emulator 头能力矩阵）。

use std::net::SocketAddr;
use std::time::Duration;

use mesa_driver_sinumerik_nck::{NckClient, NckConnConfig, NckReadItem, NckWireAddress};
use tokio::io::{AsyncBufReadExt, BufReader};

/// READY 行前缀（与 `main.rs::READY_PREFIX` 同值；双写冻结，改一处必改另一处，
/// schema 测试只校验格式存在，不跨 crate 引用避免测试耦合生产常量）。
const READY_PREFIX: &str = "MESA_NCK_EMULATOR_READY=";

/// 运行中的 emulator 子进程（`kill_on_drop`：drop 即杀，无孤儿；僵尸由
/// 测试进程退出时统一回收——短命测试进程可接受，见模块注释）。
struct EmulatorProcess {
    _child: tokio::process::Child,
    addr: SocketAddr,
}

/// 启动 emulator 并等待其自报 READY（15s 上限；READY 即已 bind，
/// 后继 connect 由内核 backlog 承接，无需二次等待）。
async fn spawn_emulator(args: &[&str]) -> EmulatorProcess {
    let bin = env!("CARGO_BIN_EXE_mesa-nck-emulator");
    let mut child = tokio::process::Command::new(bin)
        .arg("--port")
        .arg("0")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("emulator 子进程启动");
    let stderr = child.stderr.take().expect("stderr 已管道");
    let mut lines = BufReader::new(stderr).lines();
    let addr: SocketAddr = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match lines.next_line().await.expect("stderr 读取") {
                Some(line) => {
                    if let Some(rest) = line.strip_prefix(READY_PREFIX) {
                        return rest.parse().expect("READY 地址非法");
                    }
                }
                None => panic!("emulator 未打印 READY 即退出"),
            }
        }
    })
    .await
    .expect("15s 未收到 emulator READY");
    EmulatorProcess {
        _child: child,
        addr,
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
    let emulator = spawn_emulator(&["--scenario", "happy", "--element-size", "8"]).await;
    let mut c = connect(emulator.addr.port()).await;
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
    let emulator = spawn_emulator(&["--scenario", "partial_bad", "--fail-items", "0"]).await;
    let mut c = connect(emulator.addr.port()).await;
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
    let emulator = spawn_emulator(&["--scenario", "wrong_transport"]).await;
    let mut c = connect(emulator.addr.port()).await;
    let out = c.read_vars(&[item(1)]).await.expect("读返回");
    assert!(out[0].data.is_none(), "transport 不符必须 BAD");
    assert_eq!(out[0].return_code, 0xFF);
    assert_eq!(out[0].transport_size, 0x07);
}

#[tokio::test]
async fn e2e_short_payload() {
    let emulator = spawn_emulator(&["--scenario", "short_payload"]).await;
    let mut c = connect(emulator.addr.port()).await;
    let out = c.read_vars(&[item(1)]).await.expect("读返回");
    assert!(out[0].data.is_none(), "短包必须 BAD");
}

#[tokio::test]
async fn e2e_malformed() {
    let emulator = spawn_emulator(&["--scenario", "malformed"]).await;
    let mut c = connect(emulator.addr.port()).await;
    assert!(
        c.read_vars(&[item(1)]).await.is_err(),
        "截断包必须连接级失败"
    );
}

#[tokio::test]
async fn e2e_disconnect_after_n() {
    let emulator = spawn_emulator(&[
        "--scenario",
        "disconnect_after_n",
        "--disconnect-after",
        "1",
    ])
    .await;
    let mut c = connect(emulator.addr.port()).await;
    let ok = c.read_vars(&[item(1)]).await.expect("首次读");
    assert!(ok[0].data.is_some());
    assert!(c.read_vars(&[item(1)]).await.is_err(), "断开后读必须失败");
}

#[tokio::test]
async fn e2e_pdu_240() {
    let emulator = spawn_emulator(&["--scenario", "pdu_240"]).await;
    let mut c = connect(emulator.addr.port()).await;
    assert_eq!(c.negotiated_pdu_length(), 240);
    let out = c.read_vars(&[item(1)]).await.expect("小 PDU 读");
    assert!(out[0].data.is_some());
}

#[tokio::test]
async fn e2e_pdu_480() {
    let emulator = spawn_emulator(&["--scenario", "pdu_480"]).await;
    let mut c = connect(emulator.addr.port()).await;
    assert_eq!(c.negotiated_pdu_length(), 480);
    let out = c.read_vars(&[item(1)]).await.expect("读");
    assert!(out[0].data.is_some());
}

#[tokio::test]
async fn e2e_pdu_960() {
    // 客户端请求 480 → min(480,960)=480：上限透传不断言 960 本身。
    let emulator = spawn_emulator(&["--scenario", "pdu_960"]).await;
    let mut c = connect(emulator.addr.port()).await;
    assert_eq!(c.negotiated_pdu_length(), 480);
    let out = c.read_vars(&[item(1)]).await.expect("读");
    assert!(out[0].data.is_some());
}
