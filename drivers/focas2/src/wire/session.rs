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

    /// W01 regression (retest fix): true external cancel + next request refuses reuse.
    /// Operation aborted mid-half-packet, then next operation on the same
    /// FocasClient must rebuild (OPEN count 2). Internal timeout alone
    /// does not prove cancel safety (retest item 4).
    #[tokio::test]
    async fn cancelled_exchange_must_not_be_reused() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let opens = Arc::new(AtomicUsize::new(0));
        let opens_srv = Arc::clone(&opens);
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            // Accept at most 2 connections (initial + rebuild after cancel).
            for _ in 0..2 {
                let accept_r = listener.accept().await;
                let (mut sock, _) = match accept_r {
                    Ok(x) => x,
                    Err(_) => break,
                };
                let mut open_req = vec![0u8; 12];
                if sock.read_exact(&mut open_req).await.is_err() {
                    break;
                }
                sock.write_all(&fake_open_response()).await.unwrap();
                opens_srv.fetch_add(1, Ordering::SeqCst);
                // First connection: half header then stall (cancel trigger).
                let mut req = vec![0u8; 40];
                if sock.read_exact(&mut req).await.is_err() {
                    break;
                }
                sock.write_all(&[0xA0, 0xA0, 0xA0]).await.unwrap();
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        let host = addr.split(':').next().unwrap();
        let port: u16 = addr.split(':').nth(1).unwrap().parse().unwrap();
        let client = super::super::FocasClient::new(Duration::from_secs(5));
        client.ensure_connected(host, port).await.unwrap();
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        // First operation hangs on half packet: abort from outside.
        // FocasClient is not Clone; wrap in Arc like production WireFocasApi.
        let client = Arc::new(client);
        let c2 = Arc::clone(&client);
        let op = tokio::spawn(async move { c2.system_info().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        op.abort();
        let _ = op.await;
        // Next operation must not reuse old socket: poisoned guard clears on
        // next acquire, caller reconnects, OPEN count becomes 2.
        client.ensure_connected(host, port).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            opens.load(Ordering::SeqCst),
            2,
            "must rebuild after cancel, no reuse of half-packet session"
        );
        h.abort();
    }

    /// W01/W02 调度回归（复测第三轮：真实回环——首个 fatal 持锁 poison，
    /// 第二个等待者不得抢旧连接）。
    ///
    /// 本测试只验未连接调度形状（两并发均 Closed），不声称覆盖 fatal 窗口；
    /// 真正的并发 fatal 窗口由下 `fatal_waiters_rebuild_after_fatal` 覆盖。
    #[tokio::test]
    async fn fatal_waiters_do_not_reuse_damaged_session() {
        let client = super::super::FocasClient::new(Duration::from_millis(10));
        let (r1, r2) = tokio::join!(client.system_info(), client.system_info());
        assert!(r1.is_err() && r2.is_err());
    }

    /// W01/W02 真实并发回环（复测第五轮验收形状；单一超时语义）：
    /// 1. 服务端确认首个 GENERIC 已收到（`got_first`），回半包并挂起；
    ///    此后服务端进入**持续读取循环**：若旧实现让第二个复用损坏连接并发包，
    ///    计数器会变为 2（复测第五轮修正：旧代码读完首包即 sleep，不再计数，
    ///    `generics == 1` 无证明力——此处改为循环读取+计数）。
    /// 2. 首个 operation 尚未结束时启动第二个，确保它正在等待会话锁；
    /// 3. 首个因 exchange 超时 fatal（持锁 poison，不释放锁给后来者）；
    /// 4. 断言第二个返回 `Closed`（只见 `None`），且旧连接 GENERIC 计数仍为 1；
    /// 5. 显式重建（新 listener 连接），确认一次正常读取成功。
    ///
    /// 旧实现（先 complete 释放锁再 invalidate）下第二个会抢到旧连接并发出
    /// 第二个 GENERIC（`generic_count == 2`），本测试锁该窗口已关闭。
    /// 超时说明（单一语义，复测第五轮修正）：client 超时 `T=1500ms` 同时是
    /// OPEN 握手与 exchange 超时（`WireSession{timeout}` 由建连超时传入）；
    /// 回环 OPEN 远快于 T，首个在半包读上等待整整 T 后 fatal——测试约运行
    /// T（~1.5s）而非 200ms，不断言“200ms 内失败”，只断言窗口语义。
    #[tokio::test]
    async fn fatal_waiters_rebuild_after_fatal() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;
        let opens = Arc::new(AtomicUsize::new(0));
        let generics = Arc::new(AtomicUsize::new(0));
        let opens_srv = Arc::clone(&opens);
        let generics_srv = Arc::clone(&generics);
        let got_first = Arc::new(Notify::new());
        let got_first_srv = Arc::clone(&got_first);
        // 连接1 listener：OPEN → 首个 GENERIC → 半包后进入持续读取循环。
        // 循环读取是关键：旧实现复用损坏连接发第二个 GENERIC 时计数变 2；
        // 若读完首包即 sleep，计数恒 1，无证明力（复测第五轮第 2 项）。
        let listener1 = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr1 = listener1.local_addr().unwrap().to_string();
        let h1 = tokio::spawn(async move {
            let (mut sock, _) = listener1.accept().await.unwrap();
            let mut open_req = vec![0u8; 12];
            sock.read_exact(&mut open_req).await.unwrap();
            sock.write_all(&fake_open_response()).await.unwrap();
            opens_srv.fetch_add(1, Ordering::SeqCst);
            // 首个 GENERIC：确认收到，回半包。
            let mut req = vec![0u8; 40];
            sock.read_exact(&mut req).await.unwrap();
            generics_srv.fetch_add(1, Ordering::SeqCst);
            got_first_srv.notify_one();
            sock.write_all(&[0xA0, 0xA0, 0xA0]).await.unwrap();
            // 持续读取循环：旧连接上任何后续字节（第二个 GENERIC 头 40B）
            // 都会被计数——旧实现此处计数变 2，新实现保持 1 后超时退出。
            loop {
                let mut more = vec![0u8; 40];
                let got =
                    tokio::time::timeout(Duration::from_secs(10), sock.read_exact(&mut more)).await;
                match got {
                    Ok(Ok(_)) => {
                        generics_srv.fetch_add(1, Ordering::SeqCst);
                    }
                    _ => break,
                }
            }
        });
        let host1 = addr1.split(':').next().unwrap();
        let port1: u16 = addr1.split(':').nth(1).unwrap().parse().unwrap();
        // 单一超时 T=1500ms（OPEN 与 exchange 同值；回环 OPEN 远快于 T）。
        let client = super::super::FocasClient::new(Duration::from_millis(1500));
        client
            .ensure_connected_with_timeout(host1, port1, Duration::from_millis(1500))
            .await
            .unwrap();
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        // 首个 operation（持锁 exchange，半包处挂起→超时 fatal）。
        let client = Arc::new(client);
        let c1 = Arc::clone(&client);
        let first = tokio::spawn(async move { c1.system_info().await });
        // 等服务端确认首个 GENERIC 已收到（首个已持锁进入 exchange）。
        tokio::time::timeout(Duration::from_secs(5), got_first.notified())
            .await
            .expect("服务端必须收到首个 GENERIC");
        // 再启动第二个：此时首个仍挂起（T=1500ms 超时未到），第二个阻塞在 guard()。
        let c2 = Arc::clone(&client);
        let waiter_started = Arc::new(Notify::new());
        let waiter_started_srv = Arc::clone(&waiter_started);
        let second = tokio::spawn(async move {
            waiter_started_srv.notify_one();
            c2.system_info().await
        });
        tokio::time::timeout(Duration::from_secs(5), waiter_started.notified())
            .await
            .expect("第二个必须已启动");
        // 给第二个留足时间阻塞在会话锁上（首个超时 T=1500ms，第二个此时必等待）。
        tokio::time::sleep(Duration::from_millis(100)).await;
        // 首个超时 fatal（持锁 poison，约 T 后）；第二个随后只见 None → Closed。
        let r1 = first.await.expect("首个 JoinHandle 不 panic");
        assert!(r1.is_err(), "首个半包必须失败");
        let r2 = tokio::time::timeout(Duration::from_secs(5), second)
            .await
            .expect("第二个必须在首个失败后及时返回")
            .expect("第二个 JoinHandle 不 panic");
        // 关键断言：第二个是 Closed（poison 后只见 None），不是复用旧连接的
        // 超时/成功；且旧连接 GENERIC 计数仍为 1（服务端持续读取循环已验证
        // 无第二个 40B 请求；旧实现此处计数为 2）。
        assert!(
            matches!(r2, Err(super::super::WireError::Closed)),
            "第二个必须 Closed（poison 后只见 None），实际：{r2:?}"
        );
        // 给服务端循环留时间消费任何迟到字节（若旧实现复用，40B 必达）。
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            generics.load(Ordering::SeqCst),
            1,
            "旧连接不得收到第二个 GENERIC（旧实现此处为 2）"
        );
        h1.abort();
        // 显式重建：新 listener + 正常 sysinfo 响应，确认一次读取成功。
        let listener2 = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr2 = listener2.local_addr().unwrap().to_string();
        let h2 = tokio::spawn(async move {
            let (mut sock, _) = listener2.accept().await.unwrap();
            let mut open_req = vec![0u8; 12];
            sock.read_exact(&mut open_req).await.unwrap();
            sock.write_all(&fake_open_response()).await.unwrap();
            // 正常 sysinfo 响应：真机 sysinfo_response.bin 46B 全帧逐字节
            //（不手造 framing；与 fixture_tests::sysinfo_decodes 同源）。
            let mut req = vec![0u8; 40];
            sock.read_exact(&mut req).await.unwrap();
            let frame: &[u8] = &[
                0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x03, 0x21, 0x02, 0x00, 0x24, 0x00, 0x01, 0x00, 0x22,
                0x00, 0x01, 0x00, 0x01, 0x00, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x12,
                0x02, 0x02, 0x00, 0x20, 0x33, 0x30, 0x20, 0x4D, 0x47, 0x33, 0x31, 0x5A, 0x31, 0x30,
                0x2E, 0x30, 0x30, 0x33,
            ];
            sock.write_all(frame).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let host2 = addr2.split(':').next().unwrap();
        let port2: u16 = addr2.split(':').nth(1).unwrap().parse().unwrap();
        client
            .ensure_connected_with_timeout(host2, port2, Duration::from_secs(5))
            .await
            .unwrap();
        let ok = client.system_info().await.expect("重建后必须读取成功");
        assert_eq!(ok.series, "G31Z");
        h2.abort();
    }
}
