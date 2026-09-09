//! NCK 回环脚手架（Commit C：COTP/Setup 握手 + NCK ReadVar 服务）。
//!
//! 对端行为（确定性，无真机）：
//! - `COTP CR → CC`；`Setup → Ack`（协商 `min(请求, max_pdu)`）；
//! - `Read`：逐项解析 10 字节 NCK 规范（`12 08 syntax …`，syntax 非
//!   0x82/83/84 即关连接，形如真机拒收畸形）；GOOD 项返回 pattern 数据
//!   （连接级全局序号：第 n 项全字节为 `n+1`），长度 = `linecount × element_size`
//!   （`element_size` 由测试配置，与 catalog 条目一致）；`fail_items` 指定的
//!   当包第 k 项返回 `0x05` 无数据；`truncate` 模式返回截断包（畸形测试）；
//! - 收到的规范字节原样记录（`received_specs`），测试断言 exact 发送字节。
//!
//! NOTE: 响应 transport_size 取 `0x04`（NCK 真实回传值待抓包确认；传输层
//! 切片只看 len_bits，非 0x03 均同途，驱动侧 V1 不校验该字段，待 Gate 确认）。

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;

/// 脚手架状态（多连接共享，测试按需配置）。
#[derive(Debug, Clone, Default)]
pub struct NckFixtureState {
    /// Setup 协商上限（0 → 480）。
    pub max_pdu: u16,
    /// 单元素字节数（响应数据长度 = linecount × element_size）。
    pub element_size: usize,
    /// 当包内注入 BAD 的项索引（0 起，每请求重新计数）。
    pub fail_items: HashSet<usize>,
    /// 畸形模式：读响应截断（malformed 测试）。
    pub truncate: bool,
    /// 已收规范记录（测试断言 exact 发送字节用）。
    pub received: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl NckFixtureState {
    pub fn with_max_pdu(max_pdu: u16) -> Self {
        Self {
            max_pdu,
            ..Default::default()
        }
    }

    pub fn received_specs(&self) -> Vec<Vec<u8>> {
        self.received.lock().expect("fixture 记录").clone()
    }
}

/// 启动脚手架：握手 + NCK ReadVar 服务循环。
pub async fn spawn_nck_fixture(
    state: NckFixtureState,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
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

async fn read_packet(stream: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).await.ok()?;
    let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
    if !(4..=8192).contains(&len) {
        return None;
    }
    let mut pkt = vec![0u8; len];
    pkt[0..4].copy_from_slice(&hdr);
    stream.read_exact(&mut pkt[4..]).await.ok()?;
    Some(pkt)
}

async fn send_packet(stream: &mut tokio::net::TcpStream, s7: &[u8]) -> bool {
    use tokio::io::AsyncWriteExt;
    let total = (4 + 3 + s7.len()) as u16;
    let mut pkt = vec![0x03u8, 0x00];
    pkt.extend_from_slice(&total.to_be_bytes());
    pkt.extend_from_slice(&[0x02, 0xF0, 0x80]);
    pkt.extend_from_slice(s7);
    stream.write_all(&pkt).await.is_ok()
}

async fn serve_conn(mut stream: tokio::net::TcpStream, shared: Arc<Mutex<NckFixtureState>>) {
    // 1) COTP CR → CC（固定确认，TSAP 不校验——握手层只保证帧形态）。
    let pkt = match read_packet(&mut stream).await {
        Some(p) => p,
        None => return,
    };
    if pkt.len() < 6 || pkt[5] != 0xE0 {
        return;
    }
    let mut cc = vec![0x03u8, 0x00, 0x00, 0x16, 0x11, 0xD0];
    cc.extend([0u8; 16]);
    {
        use tokio::io::AsyncWriteExt;
        if stream.write_all(&cc).await.is_err() {
            return;
        }
    }
    // 2) Setup → Ack（协商 min(请求, max_pdu)，放在 S7 区 [23..25]）。
    let max_pdu = {
        let m = shared.lock().expect("fixture 状态").max_pdu;
        if m == 0 { 480 } else { m }
    };
    let pkt = match read_packet(&mut stream).await {
        Some(p) => p,
        None => return,
    };
    // pkt = TPKT(4) + COTP(3) + S7(18)：请求 PDU 在末 2 字节。
    if pkt.len() < 7 + 18 {
        return;
    }
    let s7 = &pkt[7..];
    let requested = u16::from_be_bytes([s7[s7.len() - 2], s7[s7.len() - 1]]);
    let negotiated = requested.min(max_pdu).to_be_bytes();
    let mut ack = vec![0x32u8, 0x03, 0x00, 0x00, s7[4], s7[5]];
    ack.extend_from_slice(&[0x00, 0x08, 0x00, 0x00]);
    ack.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    ack.extend_from_slice(&[
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        negotiated[0],
        negotiated[1],
        0x00,
    ]);
    if !send_packet(&mut stream, &ack).await {
        return;
    }
    // 3) NCK ReadVar 服务循环（连接级全局序号供 pattern）。
    let mut ordinal: u64 = 0;
    loop {
        let pkt = match read_packet(&mut stream).await {
            Some(p) => p,
            None => break,
        };
        if pkt.len() < 7 + 12 {
            break;
        }
        let s7 = &pkt[7..];
        // 仅服务 Read（0x01/0x04）；其余即关连接（形如真机拒收）。
        if s7[1] != 0x01 || s7.get(10).copied().unwrap_or(0) != 0x04 {
            break;
        }
        let state = shared.lock().expect("fixture 状态").clone();
        let resp = read_ack(s7, &state, &mut ordinal);
        if !send_packet(&mut stream, &resp).await {
            break;
        }
    }
}

/// 构造 NCK 读响应：param [04, count] + 逐项 `[ret, transport, len16, data]`。
fn read_ack(req: &[u8], state: &NckFixtureState, ordinal: &mut u64) -> Vec<u8> {
    // 请求 param [0x04, count] 位于 s7[10..12]，规范从 s7[12] 起每 10 字节一项。
    let count = req.get(11).copied().unwrap_or(0) as usize;
    let mut specs = Vec::with_capacity(count);
    let mut off = 12;
    for _ in 0..count {
        if off + 10 <= req.len() {
            specs.push(req[off..off + 10].to_vec());
        }
        off += 10;
    }
    let mut data = Vec::new();
    for (k, spec) in specs.iter().enumerate() {
        let tag = (*ordinal as u8).wrapping_add(1);
        *ordinal += 1;
        // 规范校验：12 08 82/83/84，否则按畸形关连接（空响应触发对端关闭）。
        let valid = spec.len() == 10
            && spec[0] == 0x12
            && spec[1] == 0x08
            && (0x82..=0x84).contains(&spec[2]);
        if !valid || state.fail_items.contains(&k) {
            if !valid {
                return Vec::new();
            }
            data.extend_from_slice(&[0x05, 0x04, 0x00, 0x00]);
            continue;
        }
        // 数据长度 = linecount × element_size；pattern 全字节为序号 tag。
        let linecount = spec[9] as usize;
        let len = linecount * state.element_size;
        let len_bits = (len * 8) as u16;
        data.push(0xFF);
        data.push(0x04);
        data.extend_from_slice(&len_bits.to_be_bytes());
        data.extend_from_slice(&vec![tag; len]);
        // 奇长项补齐（除末项），模仿 CPU 字对齐。
        if len % 2 == 1 && k + 1 < specs.len() {
            data.push(0x00);
        }
    }
    if state.truncate {
        // 畸形模式：声明长度与实际不符的截断包。
        return vec![0x32, 0x03];
    }
    let mut s7 = vec![0x32u8, 0x03, 0x00, 0x00, req[4], req[5]];
    s7.extend_from_slice(&[0x00, 0x02]);
    s7.extend_from_slice(&(data.len() as u16).to_be_bytes());
    // 响应 param 占位 4 字节（与传输层解析 `12+param_len` 口径对齐，见 s7 回环）。
    s7.extend_from_slice(&[0x00, 0x00, 0x04, count as u8]);
    s7.extend_from_slice(&data);
    // 记录收到的规范（exact 发送字节断言用）。
    state.received.lock().expect("fixture 记录").extend(specs);
    s7
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{NckClient, NckReadItem};
    use crate::codec::NckWireAddress;
    use crate::config::NckConnConfig;

    async fn connect(state: &NckFixtureState) -> (NckClient, SocketAddr) {
        let (addr, _) = spawn_nck_fixture(state.clone()).await;
        // NOTE: handle 故意不 abort（测试结束即回收；abort 竞争曾导致偶发失败）。
        let cfg = NckConnConfig {
            host: "127.0.0.1".into(),
            port: addr.port(),
            local_tsap: 0x0100,
            remote_tsap: 0x0100,
            timeout_ms: 3000,
            requested_pdu_length: 480,
        };
        let client = NckClient::connect(&cfg).await.expect("NCK 建连");
        (client, addr)
    }

    fn item(syntax: u8, area_unit: u8, column: u16, line: u16, linecount: u8) -> NckReadItem {
        NckReadItem {
            wire: NckWireAddress {
                syntax_id: syntax,
                area_unit,
                column,
                line,
                module: 0x12,
                line_count: linecount,
            },
            expected_data_len: linecount as usize * 8,
        }
    }

    #[tokio::test]
    async fn handshake_negotiates_pdu() {
        use mesa_s7_transport::{S7ConnectOptions, S7Session};
        let (addr, h) = spawn_nck_fixture(NckFixtureState::with_max_pdu(240)).await;
        let opts = S7ConnectOptions {
            host: "127.0.0.1".into(),
            port: addr.port(),
            local_tsap: 0x0100,
            remote_tsap: 0x0100,
            timeout_ms: 3000,
            requested_pdu_length: 480,
        };
        let session = S7Session::connect(opts).await.expect("握手成功");
        assert_eq!(session.negotiated_pdu_length(), 240);
        session.disconnect().await.unwrap();
        h.abort();
    }

    #[tokio::test]
    async fn loopback_single_read_exact() {
        let state = NckFixtureState {
            element_size: 8,
            ..Default::default()
        };
        let (mut c, _) = connect(&state).await;
        let out = c.read_vars(&[item(0x82, 0x41, 42, 3, 1)]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_ref().unwrap(), &vec![0x01; 8]);
        // exact 发送字节断言。
        assert_eq!(
            state.received_specs(),
            vec![vec![
                0x12, 0x08, 0x82, 0x41, 0x00, 0x2A, 0x00, 0x03, 0x12, 0x01
            ]]
        );
    }

    #[tokio::test]
    async fn loopback_multi_read_chunks_in_order() {
        let state = NckFixtureState {
            element_size: 8,
            ..Default::default()
        };
        let (mut c, _) = connect(&state).await;
        // 10 字节规范 + 8 期望：首项 18、后续 22；预算 448 → 20+5 两包。
        let items: Vec<_> = (0..25).map(|k| item(0x82, 0x41, 42, k as u16, 1)).collect();
        let out = c.read_vars(&items).await.unwrap();
        assert_eq!(out.len(), 25);
        for (k, raw) in out.iter().enumerate() {
            assert_eq!(raw.as_ref().unwrap(), &vec![(k as u8) + 1; 8], "第 {k} 项");
        }
        assert_eq!(state.received_specs().len(), 25);
    }

    #[tokio::test]
    async fn loopback_partial_bad_isolated_and_truncate_fatal() {
        let mut st = NckFixtureState {
            element_size: 8,
            ..Default::default()
        };
        st.fail_items.insert(1);
        let (mut c, _) = connect(&st).await;
        let items: Vec<_> = (0..3).map(|k| item(0x82, 0x41, 42, k, 1)).collect();
        let out = c.read_vars(&items).await.unwrap();
        assert_eq!(out.len(), 3);
        assert!(out[0].is_some());
        assert!(out[1].is_none(), "第 1 项 BAD 必须隔离");
        assert_eq!(out[2].as_ref().unwrap(), &vec![0x03; 8]);

        let trunc = NckFixtureState {
            truncate: true,
            ..Default::default()
        };
        let (mut c2, _) = connect(&trunc).await;
        let err = c2
            .read_vars(&[item(0x82, 0x41, 42, 0, 1)])
            .await
            .unwrap_err();
        assert!(
            err.code == "READ_SHORT" || err.code == "S7_SHORT" || err.code == "S7_LEN_MISMATCH",
            "截断包必须整包 fatal，实际: {}",
            err.code
        );
    }

    #[tokio::test]
    async fn loopback_all_three_syntaxes_accepted() {
        let state = NckFixtureState {
            element_size: 8,
            ..Default::default()
        };
        let (mut c, _) = connect(&state).await;
        let out = c
            .read_vars(&[
                item(0x82, 0x41, 1, 1, 1),
                item(0x83, 0x41, 1, 1, 1),
                item(0x84, 0x41, 1, 1, 1),
            ])
            .await
            .unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|r| r.is_some()));
        let specs = state.received_specs();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0][2], 0x82);
        assert_eq!(specs[1][2], 0x83);
        assert_eq!(specs[2][2], 0x84);
    }

    #[tokio::test]
    async fn loopback_linecount_scales_data() {
        let state = NckFixtureState {
            element_size: 8,
            ..Default::default()
        };
        let (mut c, _) = connect(&state).await;
        // linecount=3 → 24 字节。
        let out = c.read_vars(&[item(0x82, 0x41, 42, 1, 3)]).await.unwrap();
        assert_eq!(out[0].as_ref().unwrap(), &vec![0x01; 24]);
    }
}
