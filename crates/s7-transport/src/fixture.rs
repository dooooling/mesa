//! 回环假 S7 服务器（单测脚手架，非传输语义）。
//!
//! 用途：为 `S7Session` 提供确定性端到端验证（握手/分片/逐项 BAD/SZL/写），
//! 不依赖真机，不依赖现场抓包。行为约定：
//!
//! - `COTP CR → CC`；`Setup → Ack`（协商值取 `min(请求, max_pdu)`，放在 S7 区
//!   `[23..25]`，与 `parse_setup_ack` 的读取位一致）；
//! - `Read` 按 S7ANY 规范长度字段（`spec[4..6]`）返回确定性 pattern（连接级全局
//!   序号：第 n 个项全字节为 `n+1`，跨分片可断言顺序），奇长项后补 `0x00`（除末项）；
//! - `fail_item` 指定的当包第 k 项返回 `0x05` 无数据（逐项 BAD 注入）；
//! - `Write → Ack 0xFF`（`fail_write` 时返回 `0x05`）；
//! - `SZL →` 原样回传 `szl_payload`。
//!
//! NOTE: 读取规范长度字段是脚手架为构造响应而做的测试约定，不代表传输层
//! 理解 S7ANY；生产编码永远由调用方（Driver）负责。

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;

use crate::tpkt::{recv_packet, send_packet};

/// 假服务器可变状态（多连接共享，测试按需配置）。
#[derive(Debug, Clone)]
pub struct FixtureState {
    /// 对端宣告的最大 PDU（协商上限）。
    pub max_pdu: u16,
    /// 当包内注入 BAD 的项索引（0 起，每请求重新计数）。
    pub fail_items: HashSet<usize>,
    /// 写失败注入。
    pub fail_write: bool,
    /// SZL 回传负载。
    pub szl_payload: Vec<u8>,
}

impl Default for FixtureState {
    fn default() -> Self {
        Self {
            max_pdu: 480,
            fail_items: HashSet::new(),
            fail_write: false,
            szl_payload: vec![0xFF, 0x09, 0x00, 0x24, 0x00, 0x11, 0x00, 0x01],
        }
    }
}

/// 启动假服务器：返回监听地址与 accept 任务句柄（测试结束时 `abort`）。
pub async fn spawn_fake_s7(state: FixtureState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("fixture bind");
    let addr = listener.local_addr().expect("fixture addr");
    let shared = Arc::new(Mutex::new(state));
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                serve_conn(stream, shared).await;
            });
        }
    });
    (addr, handle)
}

async fn serve_conn(mut stream: tokio::net::TcpStream, shared: Arc<Mutex<FixtureState>>) {
    // 连接级全局项序号（跨请求递增）：data pattern 用它，保证多包分片顺序可断言。
    let mut ordinal: u64 = 0;
    loop {
        let pkt = match recv_packet(&mut stream).await {
            Ok(p) => p,
            Err(_) => break, // 对端关闭/畸形即结束本连接
        };
        if pkt.len() < 7 {
            break;
        }
        // COTP CR：第 6 字节 0xE0。
        if pkt.len() >= 6 && pkt[5] == crate::cotp::COTP_CR {
            let mut cc = vec![0x03, 0x00, 0x00, 0x16, 0x11, crate::cotp::COTP_CC];
            cc.extend([0u8; 16]);
            if send_packet(&mut stream, &cc).await.is_err() {
                break;
            }
            continue;
        }
        let s7 = &pkt[7..];
        if s7.len() < 12 {
            break;
        }
        let state = shared.lock().expect("fixture state").clone();
        // S7 头 10 字节，param 从 s7[10] 起（功能码位）；响应解析的 `12+param_len`
        // 是另一口径，原样保留，此处只做请求分发。
        let func = s7.get(10).copied().unwrap_or(0);
        let resp_s7: Vec<u8> = if s7[1] == 0x01 && func == 0xF0 {
            setup_ack(s7, state.max_pdu)
        } else if s7[1] == 0x01 && func == 0x04 {
            read_ack(s7, &state, &mut ordinal)
        } else if s7[1] == 0x01 && func == 0x05 {
            write_ack(s7, state.fail_write)
        } else if s7[1] == 0x07 {
            szl_ack(s7, &state.szl_payload)
        } else {
            break;
        };
        let mut pkt = vec![0x03, 0x00, 0x00, 0x00, 0x02, crate::cotp::COTP_DT, 0x80];
        pkt.extend_from_slice(&resp_s7);
        let len = pkt.len() as u16;
        pkt[2..4].copy_from_slice(&len.to_be_bytes());
        if send_packet(&mut stream, &pkt).await.is_err() {
            break;
        }
    }
}

fn setup_ack(req: &[u8], max_pdu: u16) -> Vec<u8> {
    let requested = u16::from_be_bytes([req[req.len() - 2], req[req.len() - 1]]);
    let negotiated = requested.min(max_pdu);
    let mut s7 = vec![0x32, 0x03, 0x00, 0x00];
    s7.extend_from_slice(&[req[4], req[5]]); // PDU ref 回显
    s7.extend_from_slice(&[0x00, 0x08, 0x00, 0x00]);
    s7.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    // 补到 26 字节，协商值放在 s7[23..25]（parse_setup_ack 读取位）。
    let nb = negotiated.to_be_bytes();
    s7.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, nb[0], nb[1], 0x00]);
    s7
}

fn read_ack(req: &[u8], state: &FixtureState, ordinal: &mut u64) -> Vec<u8> {
    // 请求 param [0x04, count] 位于 s7[10..12]，规范从 s7[12] 起每 12 字节一项。
    let count = req.get(11).copied().unwrap_or(0) as usize;
    let mut specs = Vec::with_capacity(count);
    let mut off = 12;
    for _ in 0..count {
        if off + 12 <= req.len() {
            specs.push(req[off..off + 12].to_vec());
        }
        off += 12;
    }
    let mut data = Vec::new();
    for (k, spec) in specs.iter().enumerate() {
        let tag = (*ordinal as u8).wrapping_add(1);
        *ordinal += 1;
        if state.fail_items.contains(&k) {
            data.extend_from_slice(&[0x05, 0x04, 0x00, 0x00]);
            continue;
        }
        let req_len = u16::from_be_bytes([spec[4], spec[5]]) as usize;
        if spec[3] == 0x01 {
            // BIT 请求：回传 1 bit。
            data.extend_from_slice(&[0xFF, 0x03, 0x00, 0x01, 0x01]);
            continue;
        }
        let len_bits = (req_len * 8) as u16;
        data.push(0xFF);
        data.push(0x04);
        data.extend_from_slice(&len_bits.to_be_bytes());
        data.extend_from_slice(&vec![tag; req_len]);
        // 奇长项补齐（除末项），模仿真实 CPU 字对齐。
        if req_len % 2 == 1 && k + 1 < specs.len() {
            data.push(0x00);
        }
    }
    let mut s7 = vec![0x32, 0x03, 0x00, 0x00];
    s7.extend_from_slice(&[req[4], req[5]]);
    s7.extend_from_slice(&[0x00, 0x02]);
    s7.extend_from_slice(&(data.len() as u16).to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x00, 0x04, count as u8]);
    s7.extend_from_slice(&data);
    s7
}

fn write_ack(req: &[u8], fail: bool) -> Vec<u8> {
    let mut s7 = vec![0x32, 0x03, 0x00, 0x00];
    s7.extend_from_slice(&[req[4], req[5]]);
    s7.extend_from_slice(&[0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x05, 0x01]);
    s7.push(if fail { 0x05 } else { 0xFF });
    s7
}

fn szl_ack(req: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut s7 = vec![0x32, 0x07, 0x00, 0x00];
    s7.extend_from_slice(&[req[4], req[5]]);
    let data_len = (12 + payload.len()) as u16;
    s7.extend_from_slice(&[0x00, 0x08]);
    s7.extend_from_slice(&data_len.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x01, 0x12, 0x04, 0x11, 0x44, 0x01, 0x00]);
    s7.extend_from_slice(payload);
    s7
}

/// 测试用 S7ANY 规范构造（脚手架约定：`0x12 0x0A 0x10 transport len db area addr24`）。
pub fn test_s7any_spec(transport: u8, req_len: u16, db: u16, area: u8, bit_addr: u32) -> Vec<u8> {
    let mut v = vec![0x12, 0x0A, 0x10, transport];
    v.extend_from_slice(&req_len.to_be_bytes());
    v.extend_from_slice(&db.to_be_bytes());
    v.push(area);
    v.push(((bit_addr >> 16) & 0xFF) as u8);
    v.push(((bit_addr >> 8) & 0xFF) as u8);
    v.push((bit_addr & 0xFF) as u8);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::S7ConnectOptions;
    use crate::read_var::S7ReadVarItem;
    use crate::session::S7Session;

    async fn connect(state: FixtureState) -> (S7Session, tokio::task::JoinHandle<()>) {
        let (addr, handle) = spawn_fake_s7(state).await;
        let opts = S7ConnectOptions {
            host: "127.0.0.1".into(),
            port: addr.port(),
            ..Default::default()
        };
        let session = S7Session::connect(opts).await.expect("回环建连");
        (session, handle)
    }

    fn byte_item(k: u32, len: usize) -> S7ReadVarItem {
        S7ReadVarItem {
            var_spec: test_s7any_spec(0x02, len as u16, 10, 0x84, k * 32),
            expected_data_len: len,
        }
    }

    #[tokio::test]
    async fn loopback_negotiates_pdu_downward() {
        let (session, h) = connect(FixtureState {
            max_pdu: 240,
            ..Default::default()
        })
        .await;
        // 请求默认 480，对端上限 240 → 协商 240。
        assert_eq!(session.negotiated_pdu_length(), 240);
        session.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_single_read_good() {
        let (mut s, h) = connect(FixtureState::default()).await;
        let out = s.read_var(&[byte_item(0, 4)]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].return_code, 0xFF);
        assert_eq!(out[0].data, vec![0x01; 4]);
        s.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_multi_read_chunks_in_order() {
        let (mut s, h) = connect(FixtureState {
            max_pdu: 960,
            ..Default::default()
        })
        .await;
        // 协商 480（请求默认 480）：预算 448，首项 16、后续 20 → 19+6。
        assert_eq!(s.negotiated_pdu_length(), 480);
        let items: Vec<_> = (0..25).map(|k| byte_item(k, 4)).collect();
        let out = s.read_var(&items).await.unwrap();
        assert_eq!(out.len(), 25);
        for (k, r) in out.iter().enumerate() {
            assert_eq!(r.return_code, 0xFF, "第 {k} 项");
            assert_eq!(r.data, vec![(k as u8) + 1; 4], "第 {k} 项");
        }
        s.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_partial_bad_isolated() {
        let mut st = FixtureState::default();
        st.fail_items.insert(1);
        let (mut s, h) = connect(st).await;
        let items: Vec<_> = (0..3).map(|k| byte_item(k, 4)).collect();
        let out = s.read_var(&items).await.unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].return_code, 0xFF);
        assert_eq!(out[1].return_code, 0x05);
        assert!(out[1].data.is_empty());
        assert_eq!(out[2].data, vec![0x03; 4]);
        s.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_odd_length_pad_alignment() {
        let (mut s, h) = connect(FixtureState::default()).await;
        // 两项各 1 字节：首项后有填充 0x00，解析必须对齐到第二项头。
        let out = s
            .read_var(&[byte_item(0, 1), byte_item(1, 1)])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].data, vec![0x01]);
        assert_eq!(out[1].data, vec![0x02]);
        s.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_bulk_path_ok() {
        let (mut s, h) = connect(FixtureState::default()).await;
        let items: Vec<_> = (0..3).map(|k| byte_item(k, 8)).collect();
        let out = s.read_bulk(&items).await.unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|r| r.return_code == 0xFF));
        assert_eq!(out[2].data, vec![0x03; 8]);
        s.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_szl_passthrough() {
        let (mut s, h) = connect(FixtureState::default()).await;
        let payload = s.read_szl(0x0011, 1).await.unwrap();
        assert!(payload.len() >= 8);
        assert_eq!(
            &payload[payload.len() - 8..],
            &[0xFF, 0x09, 0x00, 0x24, 0x00, 0x11, 0x00, 0x01]
        );
        s.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_write_ok_and_bad() {
        let (mut s, h) = connect(FixtureState::default()).await;
        let spec = test_s7any_spec(0x02, 2, 10, 0x84, 0);
        s.write_var(&spec, &[0x00, 0x7B]).await.unwrap();
        s.disconnect().await.unwrap();
        h.abort();

        let st = FixtureState {
            fail_write: true,
            ..Default::default()
        };
        let (mut s2, h2) = connect(st).await;
        let err = s2.write_var(&spec, &[0x00, 0x7B]).await.unwrap_err();
        assert_eq!(err.code, "S7_0x05");
        s2.disconnect().await.unwrap();
        h2.abort();
    }

    #[tokio::test]
    async fn loopback_unreachable_is_connection_error() {
        // 关闭端口（沿用既有约定用 9）：建连被拒，不是超时/协议错。
        let opts = S7ConnectOptions {
            host: "127.0.0.1".into(),
            port: 9,
            timeout_ms: 3000,
            ..Default::default()
        };
        let err = match S7Session::connect(opts).await {
            Ok(_) => panic!("关闭端口必须建连失败"),
            Err(e) => e,
        };
        assert_eq!(err.code, "CONNECT_FAIL");
    }
}
