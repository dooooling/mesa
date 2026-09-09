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
    /// 读失败注入：响应致命短包（驱动侧 fail 当前 attempt，session-loss 测试）。
    pub fail_reads: bool,
    /// P1-4 注入：响应 transport 改为 0x07（长度自洽），驱动必须按项 BAD。
    pub wrong_transport: bool,
    /// P1-4 注入：响应数据减半（长度自洽），驱动必须按项 BAD。
    pub short_payload: bool,
    /// 读 N 次后主动断开当连接（`disconnect_after_n_reads` 场景；
    /// `None` = 永不主动断开）。计数按连接独立。
    pub disconnect_after_reads: Option<usize>,
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
/// 回环脚手架句柄：地址 + 可变状态（故障注入）+ 确定性清理（drop 即 abort）。
pub struct NckFixture {
    /// 监听地址（驱动 dial 用）。
    pub addr: SocketAddr,
    /// 共享状态：测试中途可改（`fail_items`/`fail_reads` 注入）。
    pub state: Arc<Mutex<NckFixtureState>>,
    handle: tokio::task::JoinHandle<()>,
}

impl NckFixture {
    /// 启动脚手架（本机回环随机端口）+ 初始状态：握手 + NCK ReadVar 服务循环。
    pub async fn spawn(state: NckFixtureState) -> Self {
        Self::spawn_with_state("127.0.0.1:0".parse().expect("回环地址"), state).await
    }

    /// 启动脚手架（指定监听地址 + 初始状态；standalone emulator 用固定端口）。
    pub async fn spawn_with_state(addr: SocketAddr, state: NckFixtureState) -> Self {
        let listener = TcpListener::bind(addr).await.expect("fixture bind");
        let bound = listener.local_addr().expect("fixture addr");
        let shared = Arc::new(Mutex::new(state));
        let conn_shared = Arc::clone(&shared);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let shared = Arc::clone(&conn_shared);
                tokio::spawn(async move {
                    serve_conn(stream, shared).await;
                });
            }
        });
        Self {
            addr: bound,
            state: shared,
            handle,
        }
    }

    /// 收到的规范记录（exact 发送字节断言用）。
    pub fn received_specs(&self) -> Vec<Vec<u8>> {
        self.state
            .lock()
            .expect("fixture 记录")
            .received
            .lock()
            .expect("fixture 记录")
            .clone()
    }

    /// 注入当包 BAD（项索引集，每请求重新计数）。
    pub fn set_fail_items(&self, items: &[usize]) {
        self.state.lock().expect("fixture 状态").fail_items = items.iter().copied().collect();
    }

    /// 注入读失败（返回畸形/致命，驱动侧 fail 当前 attempt）。
    pub fn set_fail_reads(&self, fail: bool) {
        self.state.lock().expect("fixture 状态").fail_reads = fail;
    }
}

impl Drop for NckFixture {
    fn drop(&mut self) {
        self.handle.abort();
    }
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
    // NCK 扩展形 Setup（27 字节）：errinfo(00 00) + params(8)，协商值在
    // S7[18..20]。与读响应 param `[00 00 04 count]` 同构（NCK 方言 uniformly
    // 带 errinfo；Sharp7 硬件派生解析位一致，硬件终裁前见 ADR 0002）。
    // transport 解析器按 S7 总长判别（18 标准 / 20 扩展），两形皆吃。
    let mut ack = vec![0x32u8, 0x03, 0x00, 0x00, s7[4], s7[5]];
    ack.extend_from_slice(&[0x00, 0x0A, 0x00, 0x00]);
    ack.extend_from_slice(&[0x00, 0x00]);
    ack.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01]);
    ack.extend_from_slice(&negotiated);
    if !send_packet(&mut stream, &ack).await {
        return;
    }
    // 3) NCK ReadVar 服务循环（连接级全局序号供 pattern；读计数供断开场景）。
    let mut ordinal: u64 = 0;
    let mut reads: usize = 0;
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
        reads += 1;
        if state.disconnect_after_reads.is_some_and(|n| reads >= n) {
            break;
        }
    }
}

/// 构造 NCK 读响应：param [04, count] + 逐项 `[ret, transport, len16, data]`。
fn read_ack(req: &[u8], state: &NckFixtureState, ordinal: &mut u64) -> Vec<u8> {
    if state.fail_reads {
        // 致命短包：驱动侧整包 fatal（fail 当前 attempt，Manager 重建会话）。
        return vec![0x32, 0x03];
    }
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
        // P1-4 注入：wrong_transport 改 transport（长度按 byte 自洽）；
        // short_payload 减半数据（长度字段同步减半，自洽短包）。
        let linecount = spec[9] as usize;
        let len = linecount * state.element_size;
        let (transport, wire_len) = if state.wrong_transport {
            (0x07u8, len)
        } else if state.short_payload {
            (0x04u8, len / 2)
        } else {
            (0x04u8, len)
        };
        let len_field = if transport == 0x04 {
            (wire_len * 8) as u16
        } else {
            wire_len as u16
        };
        data.push(0xFF);
        data.push(transport);
        data.extend_from_slice(&len_field.to_be_bytes());
        data.extend_from_slice(&vec![tag; wire_len]);
        // 奇长项补齐（除末项），模仿 CPU 字对齐。
        if wire_len % 2 == 1 && k + 1 < specs.len() {
            data.push(0x00);
        }
    }
    if state.truncate {
        // 畸形模式：声明长度与实际不符的截断包。
        return vec![0x32, 0x03];
    }
    let mut s7 = vec![0x32u8, 0x03, 0x00, 0x00, req[4], req[5]];
    s7.extend_from_slice(&[0x00, 0x04]);
    s7.extend_from_slice(&(data.len() as u16).to_be_bytes());
    // NCK 扩展形 param（4 字节 [00 00 04 count]，plen=4 说真话；项仍从
    // S7[14] 起，与 Sharp7 解析位一致，位置相对上版零移动）。
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

    async fn connect(fx: &NckFixture) -> NckClient {
        let cfg = NckConnConfig {
            host: "127.0.0.1".into(),
            port: fx.addr.port(),
            family: "840d-sl".into(),
            local_tsap: 0x0100,
            remote_tsap: 0x0100,
            timeout_ms: 3000,
            requested_pdu_length: 480,
        };
        NckClient::connect(&cfg).await.expect("NCK 建连")
    }

    fn state() -> NckFixtureState {
        NckFixtureState {
            element_size: 8,
            ..Default::default()
        }
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
            expected_transport_size: 0x04,
        }
    }

    fn data_of(out: &[crate::client::NckReadResult], k: usize) -> &Vec<u8> {
        out[k]
            .data
            .as_ref()
            .unwrap_or_else(|| panic!("第 {k} 项应为 GOOD"))
    }

    #[tokio::test]
    async fn handshake_negotiates_pdu() {
        use mesa_s7_transport::{S7ConnectOptions, S7Session};
        let fx = NckFixture::spawn(NckFixtureState::with_max_pdu(240)).await;
        let opts = S7ConnectOptions {
            host: "127.0.0.1".into(),
            port: fx.addr.port(),
            local_tsap: 0x0100,
            remote_tsap: 0x0100,
            timeout_ms: 3000,
            requested_pdu_length: 480,
        };
        let session = S7Session::connect(opts).await.expect("握手成功");
        assert_eq!(session.negotiated_pdu_length(), 240);
        session.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn loopback_single_read_exact() {
        let fx = NckFixture::spawn(state()).await;
        let mut c = connect(&fx).await;
        let out = c.read_vars(&[item(0x82, 0x41, 42, 3, 1)]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(data_of(&out, 0), &vec![0x01; 8]);
        // exact 发送字节断言。
        assert_eq!(
            fx.received_specs(),
            vec![vec![
                0x12, 0x08, 0x82, 0x41, 0x00, 0x2A, 0x00, 0x03, 0x12, 0x01
            ]]
        );
    }

    #[tokio::test]
    async fn loopback_multi_read_chunks_in_order() {
        let fx = NckFixture::spawn(state()).await;
        let mut c = connect(&fx).await;
        // 10 字节规范 + 8 期望：首项 18、后续 22；预算 448 → 20+5 两包。
        let items: Vec<_> = (0..25).map(|k| item(0x82, 0x41, 42, k as u16, 1)).collect();
        let out = c.read_vars(&items).await.unwrap();
        assert_eq!(out.len(), 25);
        for (k, r) in out.iter().enumerate() {
            assert_eq!(
                r.data.as_ref().unwrap(),
                &vec![(k as u8) + 1; 8],
                "第 {k} 项"
            );
        }
        assert_eq!(fx.received_specs().len(), 25);
    }

    #[tokio::test]
    async fn loopback_partial_bad_isolated_and_truncate_fatal() {
        let fx = NckFixture::spawn(state()).await;
        fx.set_fail_items(&[1]);
        let mut c = connect(&fx).await;
        let items: Vec<_> = (0..3).map(|k| item(0x82, 0x41, 42, k, 1)).collect();
        let out = c.read_vars(&items).await.unwrap();
        assert_eq!(out.len(), 3);
        assert!(out[0].data.is_some());
        assert!(out[1].data.is_none(), "第 1 项 BAD 必须隔离");
        assert_eq!(out[1].return_code, 0x05);
        assert_eq!(data_of(&out, 2), &vec![0x03; 8]);

        let fx2 = NckFixture::spawn(NckFixtureState {
            truncate: true,
            ..Default::default()
        })
        .await;
        let mut c2 = connect(&fx2).await;
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
        let fx = NckFixture::spawn(state()).await;
        let mut c = connect(&fx).await;
        let out = c
            .read_vars(&[
                item(0x82, 0x41, 1, 1, 1),
                item(0x83, 0x41, 1, 1, 1),
                item(0x84, 0x41, 1, 1, 1),
            ])
            .await
            .unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|r| r.data.is_some()));
        let specs = fx.received_specs();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0][2], 0x82);
        assert_eq!(specs[1][2], 0x83);
        assert_eq!(specs[2][2], 0x84);
    }

    #[tokio::test]
    async fn loopback_linecount_scales_data() {
        let fx = NckFixture::spawn(state()).await;
        let mut c = connect(&fx).await;
        // linecount=3 → 24 字节。
        let out = c.read_vars(&[item(0x82, 0x41, 42, 1, 3)]).await.unwrap();
        assert_eq!(data_of(&out, 0), &vec![0x01; 24]);
    }

    #[tokio::test]
    async fn loopback_fail_reads_is_session_fatal() {
        let fx = NckFixture::spawn(state()).await;
        let mut c = connect(&fx).await;
        // 先 GOOD，确认通路。
        let ok = c.read_vars(&[item(0x82, 0x41, 42, 0, 1)]).await.unwrap();
        assert!(ok[0].data.is_some());
        // 中途注入读失败 → 整包 fatal（驱动侧 fail 当前 attempt，Manager 重建）。
        fx.set_fail_reads(true);
        let err = c
            .read_vars(&[item(0x82, 0x41, 42, 0, 1)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "READ_SHORT");
    }

    #[tokio::test]
    async fn spawn_with_state_binds_fixed_addr() {
        // Layer4：指定地址绑定（emulator 固定端口路径），端口号原样保留。
        let fx = NckFixture::spawn_with_state("127.0.0.1:0".parse().unwrap(), state()).await;
        assert_eq!(fx.addr.ip().to_string(), "127.0.0.1");
        assert!(fx.addr.port() != 0, "随机端口必须落定");
        let mut c = connect(&fx).await;
        let out = c.read_vars(&[item(0x82, 0x41, 42, 0, 1)]).await.unwrap();
        assert!(out[0].data.is_some());
    }

    #[tokio::test]
    async fn disconnect_after_reads_closes_connection() {
        // Layer4：服务 1 次读后主动断开；第 2 次读必须连接级失败。
        let fx = NckFixture::spawn(NckFixtureState {
            disconnect_after_reads: Some(1),
            ..state()
        })
        .await;
        let mut c = connect(&fx).await;
        let ok = c.read_vars(&[item(0x82, 0x41, 42, 0, 1)]).await.unwrap();
        assert!(ok[0].data.is_some());
        let err = c
            .read_vars(&[item(0x82, 0x41, 42, 0, 1)])
            .await
            .unwrap_err();
        assert!(
            err.code == "READ_TIMEOUT" || err.code == "READ_RECV_FAIL" || err.code == "READ_SHORT",
            "断开后读必须失败，实际: {}",
            err.code
        );
    }

    #[tokio::test]
    async fn nck_transport_size_mismatch_is_bad() {
        // P1-4：return_code FF 但 transport 0x07≠期望 0x04 → 按项 BAD
        // （错误类型的数据绝不能标 GOOD），不整体失败。
        let fx = NckFixture::spawn(NckFixtureState {
            wrong_transport: true,
            ..state()
        })
        .await;
        let mut c = connect(&fx).await;
        let out = c
            .read_vars(&[item(0x82, 0x41, 42, 0, 1), item(0x82, 0x41, 42, 1, 1)])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].data.is_none(), "transport 不符必须 BAD");
        assert_eq!(out[0].return_code, 0xFF);
        assert_eq!(out[0].transport_size, 0x07);
        assert!(out[1].data.is_none());
    }

    #[tokio::test]
    async fn nck_response_length_mismatch_is_bad() {
        // P1-4：自洽短包（4 字节，期望 8）→ 按项 BAD，不整体失败。
        let fx = NckFixture::spawn(NckFixtureState {
            short_payload: true,
            ..state()
        })
        .await;
        let mut c = connect(&fx).await;
        let out = c.read_vars(&[item(0x82, 0x41, 42, 0, 1)]).await.unwrap();
        assert!(out[0].data.is_none(), "长度不符必须 BAD");
        assert_eq!(out[0].return_code, 0xFF);
    }

    #[tokio::test]
    async fn nck_single_item_over_negotiated_pdu_fails_closed() {
        // P2-1：F64 count=255（期望 2040）@协商 480 → 发送前拒绝，
        // 不发超长请求（line 分段语义待真机确认）。
        let fx = NckFixture::spawn(state()).await;
        let mut c = connect(&fx).await;
        assert_eq!(c.negotiated_pdu_length(), 480);
        let huge = NckReadItem {
            wire: NckWireAddress {
                syntax_id: 0x82,
                area_unit: 0x41,
                column: 42,
                line: 1,
                module: 0x12,
                line_count: 255,
            },
            expected_data_len: 255 * 8,
            expected_transport_size: 0x04,
        };
        let err = c.read_vars(&[huge]).await.unwrap_err();
        assert_eq!(err.code, "READ_ITEM_TOO_LARGE");
        // fixture 侧未收到任何规范（拒绝发生在发送前）。
        assert!(fx.received_specs().is_empty());
    }
}
