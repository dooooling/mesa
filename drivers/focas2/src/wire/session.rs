//! `WireSession`（PR1）：单 TCP 连接上的串行 FOCAS 会话。
//!
//! - 职责只有：`connect / exchange / close` + 超时 + `read_exact` 组帧。
//!   不懂 Operation、不懂 Mesa（FOCAS-local）。
//! - 串行独占：一次只处理一个 request→response（无 multiplex、无后台
//!   receiver）。多步 Operation（如 statinfo 的 2 次 exchange）由调用方
//!  （`FocasClient`）持有 session 锁完成，`exchange` 内部绝不加锁。
//! - 错误即失效：`Io/Timeout/BadMagic/BadLength/UnexpectedPacket` 等帧层
//!  错误意味着 TCP 字节流边界已不可信，调用方必须丢弃 session（`None`），
//!   由上层重连。合法 FOCAS 业务错误（`Remote`）不走此处。

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::WireError;
use super::frame::{
    FRAME_HEADER_LEN, FocasFrame, FrameError, FrameHeader, PacketType, assemble, decode_header,
};

// ---------------------------------------------------------------------------
// WireSession
// ---------------------------------------------------------------------------

/// 单连接 FOCAS 会话。普通可实例化类型（非全局单例）：未来如需
/// `Polling Session + Transfer Session` 双连接，直接再建一个即可。
pub struct WireSession {
    stream: TcpStream,
    timeout: Duration,
}

impl WireSession {
    /// 建连 + OPEN 握手（Gate 0 修正：req payload `00 02`；单流复现证实
    /// `00 01` 的流上发 GENERIC 必 RST，只有 `00 02` 的流接受后续请求）。
    /// OPEN 响应按 PR53 `validate_open_response` 校验长度闭合
    /// （165 `spec=3/count=10 → 360B`；未知布局即 Malformed，不当正常）。
    pub async fn connect(host: &str, port: u16, timeout: Duration) -> Result<Self, WireError> {
        let addr = format!("{host}:{port}");
        let stream = tokio::time::timeout(timeout, TcpStream::connect(&addr))
            .await
            .map_err(|_| WireError::Timeout)?
            .map_err(WireError::Io)?;
        let mut me = Self { stream, timeout };
        let resp = me
            .exchange(&FocasFrame::open_request(), PacketType::OPEN_RESPONSE)
            .await?;
        // PR53：OPEN 能力表长度闭合（spec/count → payload_len 公式）；
        // 失败即 session 致命（上层重连），不猜字段内容。
        super::frame::validate_open_response(&resp.payload, resp.origin)
            .map_err(|_| WireError::MalformedPayload)?;
        Ok(me)
    }

    /// 单次 request→response。调用方保证串行（持有 client 锁）。
    /// 写/读均施加 `timeout`；读严格 `read_exact(header)+read_exact(payload)`，
    /// 天然处理 TCP 分片/粘包（多余字节留 socket buffer 给下次）。
    pub async fn exchange(
        &mut self,
        request: &FocasFrame,
        expected: PacketType,
    ) -> Result<FocasFrame, WireError> {
        let raw = request.encode();
        tokio::time::timeout(self.timeout, self.stream.write_all(&raw))
            .await
            .map_err(|_| WireError::Timeout)?
            .map_err(WireError::Io)?;

        let mut head = [0u8; FRAME_HEADER_LEN];
        tokio::time::timeout(self.timeout, self.stream.read_exact(&mut head))
            .await
            .map_err(|_| WireError::Timeout)?
            .map_err(map_io)?;
        let header: FrameHeader = decode_header(&head).map_err(map_frame)?;
        if header.packet_type != expected {
            return Err(WireError::UnexpectedPacket {
                expected,
                got: header.packet_type,
            });
        }
        let mut payload = vec![0u8; header.payload_len];
        if !payload.is_empty() {
            tokio::time::timeout(self.timeout, self.stream.read_exact(&mut payload))
                .await
                .map_err(|_| WireError::Timeout)?
                .map_err(map_io)?;
        }
        assemble(header, payload).map_err(map_frame)
    }

    /// CLOSE 握手（best-effort：失败不掩盖上层结论，Gate 0：10B 空 payload）。
    pub async fn close(mut self) -> Result<(), WireError> {
        let _ = self
            .exchange(&FocasFrame::close_request(), PacketType::CLOSE_RESPONSE)
            .await?;
        Ok(())
    }
}

/// `read_exact` 的 EOF/中断统一判连接失效（`Closed` 视为致命，由上层重连）。
fn map_io(e: std::io::Error) -> WireError {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::UnexpectedEof => WireError::Closed,
        _ => WireError::Io(e),
    }
}

fn map_frame(e: FrameError) -> WireError {
    match e {
        FrameError::BadMagic => WireError::BadMagic,
        FrameError::BadLength => WireError::BadLength,
        FrameError::Malformed => WireError::MalformedPayload,
        FrameError::UnexpectedPacket { expected, got } => {
            WireError::UnexpectedPacket { expected, got }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// loopback OPEN 响应替身（PR53 长度闭合）：`spec=3/count=10 →
    /// 40+32×10=360B`（165 真机形态；count 在 payload+10，能力表零填充）。
    fn fake_open_response() -> Vec<u8> {
        let mut out = vec![0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x03, 0x01, 0x02, 0x01, 0x68];
        out.extend(std::iter::repeat_n(0x00, 0x168));
        // count 字段在 payload+10（frame+20），与真机一致。
        out[10 + 10] = 0x00;
        out[10 + 11] = 0x0a;
        debug_assert_eq!(out.len(), 10 + 0x168);
        out
    }

    /// 起一个 loopback 对端：按脚本收发原始字节（测试可精确控制分片）。
    async fn spawn_script(
        script: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            for (want, chunks) in script {
                let mut buf = vec![0u8; want.len()];
                sock.read_exact(&mut buf).await.unwrap();
                assert_eq!(buf, want, "server 收到的 request 不符");
                for c in chunks {
                    sock.write_all(&c).await.unwrap();
                }
            }
        });
        (addr, h)
    }

    /// OPEN/CLOSE 最小往返（Gate 0 真机字节）。
    #[tokio::test]
    async fn open_close_roundtrip() {
        let open_resp = fake_open_response();
        let close_resp = vec![0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x03, 0x02, 0x02, 0x00, 0x00];
        let (addr, h) = spawn_script(vec![
            (FocasFrame::open_request().encode(), vec![open_resp]),
            (FocasFrame::close_request().encode(), vec![close_resp]),
        ])
        .await;
        let host = addr.split(':').next().unwrap();
        let port: u16 = addr.split(':').nth(1).unwrap().parse().unwrap();
        let session = WireSession::connect(host, port, Duration::from_secs(5))
            .await
            .expect("OPEN 必须成功");
        session.close().await.expect("CLOSE 必须成功");
        h.await.unwrap();
    }

    /// TCP 分片：header 分 3 次、payload 分 2 次发送，仍须正确组帧。
    #[tokio::test]
    async fn fragmented_response_reassembled() {
        // 构造一个 GENERIC 响应（payload 8B），故意切碎。
        let full = [
            0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x03, 0x21, 0x02, 0x00, 0x08, 0x00, 0x01, 0x00, 0x06,
            0x00, 0x01, 0x00, 0x01,
        ]
        .to_vec();
        let (c1, c2, c3) = (
            full[0..3].to_vec(),
            full[3..10].to_vec(),
            full[10..].to_vec(),
        );
        let (c3a, c3b) = (c3[..4].to_vec(), c3[4..].to_vec());
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // 先消费 OPEN（12B 请求→370B 响应，保证 connect 通过）。
            let mut open_req = vec![0u8; 12];
            sock.read_exact(&mut open_req).await.unwrap();
            let open_resp = fake_open_response();
            sock.write_all(&open_resp).await.unwrap();
            // 消费一条 GENERIC 请求（40B SYSINFO），分片回响应。
            let mut req = vec![0u8; 40];
            sock.read_exact(&mut req).await.unwrap();
            for chunk in [&c1, &c2, &c3a, &c3b] {
                sock.write_all(chunk).await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        let host = addr.split(':').next().unwrap();
        let port: u16 = addr.split(':').nth(1).unwrap().parse().unwrap();
        let mut s = WireSession::connect(host, port, Duration::from_secs(5))
            .await
            .unwrap();
        let req = FocasFrame {
            origin: 0x0001,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: {
                let mut p = vec![0x00, 0x01];
                p.extend_from_slice(&[0x00, 0x1c, 0x00, 0x01]);
                p.extend_from_slice(&[0x00, 0x01, 0x00, 0x18]);
                p.extend_from_slice(&[0x00; 20]);
                p
            },
        };
        let resp = s
            .exchange(&req, PacketType::GENERIC_RESPONSE)
            .await
            .expect("分片响应必须组帧成功");
        assert_eq!(resp.payload.len(), 8);
        h.await.unwrap();
    }

    /// 坏 magic 即错（session 必须由调用方失效）。
    #[tokio::test]
    async fn bad_magic_is_fatal() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut open_req = vec![0u8; 12];
            sock.read_exact(&mut open_req).await.unwrap();
            let open_resp = fake_open_response();
            sock.write_all(&open_resp).await.unwrap();
            let mut req = vec![0u8; 10];
            sock.read_exact(&mut req).await.unwrap();
            // 回一个坏 magic 的 CLOSE 响应（10B 一次写完，避免 server
            // 先回后读的时序让 client 读到对端 FIN 判 Closed）。
            sock.write_all(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x02, 0x02, 0x00, 0x00])
                .await
                .unwrap();
            // 保持连接一会儿再关，让 client 有机会读完坏帧（否则先读到
            // FIN 会判 Closed——同样致命，但本测试要精确锁 BadMagic）。
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let host = addr.split(':').next().unwrap();
        let port: u16 = addr.split(':').nth(1).unwrap().parse().unwrap();
        let mut s = WireSession::connect(host, port, Duration::from_secs(5))
            .await
            .unwrap();
        let err = s
            .exchange(&FocasFrame::close_request(), PacketType::CLOSE_RESPONSE)
            .await
            .unwrap_err();
        assert!(
            matches!(err, WireError::BadMagic),
            "坏 magic 必须 BadMagic，实际：{err:?}"
        );
        assert!(err.is_session_fatal());
        h.abort();
    }

    /// 超时即错（对端延迟超过 timeout）。
    #[tokio::test]
    async fn timeout_is_fatal() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut open_req = vec![0u8; 12];
            sock.read_exact(&mut open_req).await.unwrap();
            let open_resp = fake_open_response();
            sock.write_all(&open_resp).await.unwrap();
            // 收到 CLOSE 请求后故意不回。
            let mut req = vec![0u8; 10];
            sock.read_exact(&mut req).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let host = addr.split(':').next().unwrap();
        let port: u16 = addr.split(':').nth(1).unwrap().parse().unwrap();
        let mut s = WireSession::connect(host, port, Duration::from_secs(5))
            .await
            .unwrap();
        // 用短 timeout 的 exchange：重建短超时 session 行为等价验证超时映射。
        s.timeout = Duration::from_millis(100);
        let err = s
            .exchange(&FocasFrame::close_request(), PacketType::CLOSE_RESPONSE)
            .await
            .unwrap_err();
        assert!(matches!(err, WireError::Timeout));
        h.abort();
    }
}
