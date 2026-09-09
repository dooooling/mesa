//! NCK 回环脚手架（骨架：Commit B 只做 COTP/Setup 握手）。
//!
//! TODO(Commit C)：NCK ReadVar 请求 exact-bytes 校验 + 确定性响应
//! （单读/多读/部分错/错基数/畸形/断开/PDU 边界/0x82/83/84）。
//! 届时复用 `mesa-s7-transport` 的分片与解析，本脚手架只负责“对端行为”。

use std::net::SocketAddr;

use tokio::net::TcpListener;

/// 脚手架状态（Commit C 扩展 NCK 脚本）。
#[derive(Debug, Clone, Default)]
pub struct NckFixtureState {
    /// Setup 协商上限。
    pub max_pdu: u16,
}

impl NckFixtureState {
    pub fn with_max_pdu(max_pdu: u16) -> Self {
        Self { max_pdu }
    }
}

/// 启动脚手架：接受连接并完成 COTP/Setup 握手，随后关闭写侧（NCK 语义待 Commit C）。
pub async fn spawn_nck_fixture(
    state: NckFixtureState,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("fixture bind");
    let addr = listener.local_addr().expect("fixture addr");
    let max_pdu = if state.max_pdu == 0 {
        480
    } else {
        state.max_pdu
    };
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                serve_handshake(stream, max_pdu).await;
            });
        }
    });
    (addr, handle)
}

async fn serve_handshake(mut stream: tokio::net::TcpStream, max_pdu: u16) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // 1) COTP CR → CC（固定确认，TSAP 不校验——握手层只保证帧形态）。
    let mut hdr = [0u8; 4];
    if stream.read_exact(&mut hdr).await.is_err() {
        return;
    }
    let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
    if !(4..=8192).contains(&len) {
        return;
    }
    let mut rest = vec![0u8; len - 4];
    if stream.read_exact(&mut rest).await.is_err() {
        return;
    }
    let mut cc = vec![0x03u8, 0x00, 0x00, 0x16, 0x11, 0xD0];
    cc.extend([0u8; 16]);
    if stream.write_all(&cc).await.is_err() {
        return;
    }
    // 2) Setup → Ack（协商 min(请求, max_pdu)，放在 S7 区 [23..25]）。
    if stream.read_exact(&mut hdr).await.is_err() {
        return;
    }
    let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
    if !(4..=8192).contains(&len) {
        return;
    }
    let mut req = vec![0u8; len - 4];
    if stream.read_exact(&mut req).await.is_err() {
        return;
    }
    // req = COTP(3) + S7(18)：请求 PDU 在末 2 字节。
    if req.len() < 21 {
        return;
    }
    let requested = u16::from_be_bytes([req[req.len() - 2], req[req.len() - 1]]);
    let negotiated = requested.min(max_pdu).to_be_bytes();
    let mut s7 = vec![0x32u8, 0x03, 0x00, 0x00, req[7], req[8]];
    s7.extend_from_slice(&[0x00, 0x08, 0x00, 0x00]);
    s7.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    s7.extend_from_slice(&[
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        negotiated[0],
        negotiated[1],
        0x00,
    ]);
    let total = (4 + 3 + s7.len()) as u16;
    let mut pkt = vec![0x03u8, 0x00];
    pkt.extend_from_slice(&total.to_be_bytes());
    pkt.extend_from_slice(&[0x02, 0xF0, 0x80]);
    pkt.extend_from_slice(&s7);
    let _ = stream.write_all(&pkt).await;
    // TODO(Commit C)：此后进入 NCK ReadVar 服务循环（当前骨架握手后即半闭，
    // 驱动侧 Commit C 前不建数据会话）。
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handshake_only_skeleton() {
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
}
