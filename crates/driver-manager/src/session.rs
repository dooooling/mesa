//! Core 侧 Driver 会话客户端。
//!
//! 职责：完成 Hello 校验与 Welcome 应答（§14.3）、请求/响应按 msg_id 多路分发、
//! 数据面事件上行、心跳判死（§14.4）。
//!
//! NOTE：同一会话上除心跳外均为顺序请求——Runtime 的配置流程严格串行，
//! 并发请求复用同一 pending 表在语义上也正确，当前未做并发压测。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forgelink_core_types::{ConnectionState as CoreState, DataBatch};
use forgelink_driver_protocol::{
    batch_from_pb, connection_state_from_pb, negotiate, pb, read_envelope, write_envelope,
    ProtocolError,
};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// 单次请求的响应超时。当前驱动均为内存操作；真实协议驱动若接近该阈值，
/// 应拆分流程而非调大超时。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 心跳默认参数（§14.4）。
const PING_PERIOD: Duration = Duration::from_secs(5);
const PONG_DEADLINE: Duration = Duration::from_secs(3);
const MAX_MISSED_PONGS: u32 = 3;

/// 心跳参数（§14.4 允许配置覆盖——生产当前使用默认；合同测试用短周期加速判死）。
#[derive(Debug, Clone, Copy)]
pub struct HeartbeatParams {
    pub ping_period: Duration,
    pub pong_deadline: Duration,
    pub max_missed: u32,
}

impl Default for HeartbeatParams {
    fn default() -> Self {
        if std::env::var("FORGELINK_HEARTBEAT_FAST").ok().as_deref() == Some("1") {
            return Self { ping_period: Duration::from_secs(1), pong_deadline: Duration::from_secs(1), max_missed: 2 };
        }
        Self { ping_period: PING_PERIOD, pong_deadline: PONG_DEADLINE, max_missed: MAX_MISSED_PONGS }
    }
}

/// 上行事件容量。控制类事件不允许静默丢弃，消费端必须活跃；
/// 容量仅作瞬时洪峰缓冲，溢出计入诊断计数。
pub const EVENT_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Batch(DataBatch),
    State { handle: u32, state: CoreState, detail: String },
    DriverError { handle: Option<u32>, kind: String, code: String, message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("request timed out")]
    Timeout,
    #[error("session closed")]
    Closed,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
}

struct Shared {
    /// msg_id -> 等待响应的 sender。请求方登记，reader 分发。
    pending: Mutex<HashMap<u64, oneshot::Sender<pb::Envelope>>>,
    /// 写半部全局互斥：请求路径与心跳路径共享。
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    events_tx: mpsc::Sender<SessionEvent>,
    unresponsive: Arc<AtomicBool>,
    dropped_events: AtomicU64,
}

impl Shared {
    fn register(&self, id: u64) -> oneshot::Receiver<pb::Envelope> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        rx
    }

    fn unregister(&self, id: u64) {
        self.pending.lock().unwrap().remove(&id);
    }
}

pub struct Session {
    port: u16,
    shared: Arc<Shared>,
    next_msg_id: AtomicU64,
    reader_cancel: CancellationToken,
}

impl Session {
    /// 带重试的连接：子进程从 bind 到 listen 存在窗口期，连接拒绝属预期时序，
    /// 自动退避重试；其他错误立即失败。总窗口约 3s。
    pub async fn connect_retry(
        port: u16,
        expected_token: &str,
    ) -> Result<(Self, mpsc::Receiver<SessionEvent>, Arc<AtomicBool>), SessionError> {
        let mut last: Option<SessionError> = None;
        for _ in 0..60 {
            match Self::connect(port, expected_token).await {
                Ok(v) => return Ok(v),
                Err(SessionError::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    last = Some(SessionError::Io(e));
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or(SessionError::Timeout))
    }

    /// 建立到 Driver IPC 端口的连接并完成握手（默认心跳参数）。
    pub async fn connect(
        port: u16,
        expected_token: &str,
    ) -> Result<(Self, mpsc::Receiver<SessionEvent>, Arc<AtomicBool>), SessionError> {
        Self::connect_with_heartbeat(port, expected_token, HeartbeatParams::default()).await
    }

    /// [`Session::connect`] 的心跳参数覆盖变体：合同测试以短周期验证判死路径，
    /// 避免真实 5s×3 的等待。
    ///
    /// 握手方向（§14.3）：Driver 先发 Hello 携带 token；本端校验通过后回 Welcome。
    /// token 不匹配或 Major 不兼容立即断开。
    ///
    /// 返回会话句柄与上行事件流。事件通道随会话废弃而关闭（recv 返回 None），
    /// Endpoint 运行时以此感知断连。
    pub async fn connect_with_heartbeat(
        port: u16,
        expected_token: &str,
        hb: HeartbeatParams,
    ) -> Result<(Self, mpsc::Receiver<SessionEvent>, Arc<AtomicBool>), SessionError> {
        let stream = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .map_err(|_| SessionError::Timeout)?
        .map_err(SessionError::Io)?;

        let (mut rd, mut wr) = stream.into_split();

        // ---- 读 Hello 并校验 ----
        let hello_env = tokio::time::timeout(Duration::from_secs(5), read_envelope(&mut rd))
            .await
            .map_err(|_| SessionError::Handshake("hello timeout".into()))??;
        let hello = match hello_env.body {
            Some(pb::envelope::Body::Hello(h)) => h,
            _ => return Err(SessionError::Handshake("first frame must be Hello".into())),
        };
        // 本地回环场景 token 为一次性随机值，直接比较足够（NOTE: 非常数时间比较）
        if hello.session_token != expected_token {
            return Err(SessionError::Handshake("token mismatch".into()));
        }
        negotiate(
            (hello.protocol_major, hello.protocol_minor),
            (
                forgelink_driver_protocol::PROTOCOL_MAJOR,
                forgelink_driver_protocol::PROTOCOL_MINOR,
            ),
        )
        .map_err(|e| SessionError::Handshake(e.to_string()))?;

        // ---- 回 Welcome（协商 Minor 取双方较小值）----
        let welcome = pb::Envelope {
            msg_id: hello_env.msg_id,
            body: Some(pb::envelope::Body::Welcome(pb::Welcome {
                core_version: format!("forgelinkd v{}", env!("CARGO_PKG_VERSION")),
                accepted_protocol_major: forgelink_driver_protocol::PROTOCOL_MAJOR,
                accepted_protocol_minor: forgelink_driver_protocol::PROTOCOL_MINOR
                    .min(hello.protocol_minor),
            })),
        };
        write_envelope(&mut wr, &welcome).await?;

        tracing::info!(
            driver = %hello.driver_id,
            instance = %hello.instance_id,
            version = %hello.driver_version,
            "session established"
        );

        let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
        let unresponsive_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            writer: tokio::sync::Mutex::new(wr),
            events_tx,
            unresponsive: Arc::clone(&unresponsive_flag),
            dropped_events: AtomicU64::new(0),
        });

        // ---- reader：分发响应 + 上行事件；断开时关闭事件通道通知运行时 ----
        let reader_cancel = CancellationToken::new();
        tokio::spawn(reader_loop(rd, Arc::clone(&shared), reader_cancel.clone()));

        // ---- 心跳任务：连续丢 Pong 达到阈值即判死（§14.4）----
        let hb_shared = Arc::clone(&shared);
        let hb_cancel = reader_cancel.clone();
        tokio::spawn(async move {
            static HB_ID: AtomicU64 = AtomicU64::new(u64::MAX - 1_000_000);
            let mut misses = 0u32;
            loop {
                tokio::time::sleep(hb.ping_period).await;
                if hb_cancel.is_cancelled() {
                    break;
                }
                // 心跳 id 使用独立高位段，不与请求计数器交互
                let id = HB_ID.fetch_sub(1, Ordering::Relaxed);
                let rx = hb_shared.register(id);
                let env =
                    pb::Envelope { msg_id: id, body: Some(pb::envelope::Body::Ping(pb::Ping {})) };
                let wrote = {
                    let mut w = hb_shared.writer.lock().await;
                    write_envelope(&mut *w, &env).await.is_ok()
                };
                if wrote && tokio::time::timeout(hb.pong_deadline, rx).await.is_ok() {
                    misses = 0;
                    continue;
                }
                hb_shared.unregister(id);
                misses += 1;
                tracing::warn!(misses, "pong missed");
                if misses >= hb.max_missed {
                    tracing::error!("driver unresponsive (no pong x{})", hb.max_missed);
                    hb_shared.unresponsive.store(true, Ordering::Relaxed);
                    break;
                }
            }
        });

        Ok((
            Self { port, shared, next_msg_id: AtomicU64::new(100), reader_cancel },
            events_rx,
            unresponsive_flag,
        ))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_unresponsive(&self) -> bool {
        self.shared.unresponsive.load(Ordering::Relaxed)
    }

    // ---- 协议化请求封装 ----

    /// GetMetadata：返回 (driver_id, name, version)。
    pub async fn metadata(&self) -> Result<(String, String, String), SessionError> {
        let reply = self.call(pb::envelope::Body::GetMetadata(pb::GetMetadata {})).await?;
        match reply.body {
            Some(pb::envelope::Body::MetadataReport(m)) => Ok((m.driver_id, m.name, m.version)),
            _ => Err(SessionError::Closed),
        }
    }

    /// 发送一帧并等待同 msg_id 的响应帧。超时即清理登记项，防止泄漏。
    pub async fn call(&self, body: pb::envelope::Body) -> Result<pb::Envelope, SessionError> {
        let id = self.next_msg_id.fetch_add(1, Ordering::Relaxed);
        let rx = self.shared.register(id);
        let env = pb::Envelope { msg_id: id, body: Some(body) };
        {
            let mut wr = self.shared.writer.lock().await;
            write_envelope(&mut *wr, &env).await?;
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(SessionError::Closed),
            Err(_) => {
                self.shared.unregister(id);
                Err(SessionError::Timeout)
            }
        }
    }

    /// 发送后不等响应（主动断开前的通知类消息）。
    pub async fn post(&self, body: pb::envelope::Body) -> Result<(), SessionError> {
        let id = self.next_msg_id.fetch_add(1, Ordering::Relaxed);
        let env = pb::Envelope { msg_id: id, body: Some(body) };
        let mut wr = self.shared.writer.lock().await;
        write_envelope(&mut *wr, &env).await?;
        Ok(())
    }

    /// 取消 reader 并中止心跳——连接进入废弃状态。
    pub fn invalidate(&mut self) {
        self.reader_cancel.cancel();
    }
}

async fn reader_loop(mut rd: OwnedReadHalf, shared: Arc<Shared>, cancel: CancellationToken) {
    use pb::envelope::Body;
    loop {
        let env = tokio::select! {
            r = read_envelope(&mut rd) => match r {
                Ok(e) => e,
                Err(_) => break, // 断开/解码失败都终止 reader
            },
            _ = cancel.cancelled() => break,
        };

        // 1) 已登记请求的响应原路返回（Pong 也经此路径回到心跳任务）
        // DriverError 需同时作为事件可见：contract test 通过事件断言，而 manager 通过 call 返回值断言——两者都需满足
        if env.msg_id != 0 && shared.pending.lock().unwrap().contains_key(&env.msg_id) {
            let is_driver_error = matches!(env.body, Some(Body::DriverError(_)));
            let sender = shared.pending.lock().unwrap().remove(&env.msg_id);
            if let Some(tx) = sender {
                let _ = tx.send(env.clone());
            }
            if is_driver_error {
                if let Some(Body::DriverError(e)) = env.body {
                    let d = e.detail.unwrap_or_default();
                    let ev = SessionEvent::DriverError {
                        handle: e.connection_handle,
                        kind: d.kind,
                        code: d.code,
                        message: d.message,
                    };
                    let _ = shared.events_tx.try_send(ev);
                }
            }
            continue;
        }
        // 2) 其余帧转为上行事件
        let ev = match env.body {
            Some(Body::DataBatch(b)) => batch_from_pb(b).ok().map(SessionEvent::Batch),
            Some(Body::ConnectionStateChanged(s)) => connection_state_from_pb(&s.state)
                .ok()
                .map(|state| SessionEvent::State {
                    handle: s.connection_handle,
                    state,
                    detail: s.detail,
                }),
            Some(Body::DriverError(e)) => {
                let d = e.detail.unwrap_or_default();
                Some(SessionEvent::DriverError {
                    handle: e.connection_handle,
                    kind: d.kind,
                    code: d.code,
                    message: d.message,
                })
            }
            other => {
                tracing::debug!(?other, "unsolicited envelope ignored");
                None
            }
        };
        if let Some(ev) = ev {
            if shared.events_tx.try_send(ev).is_err() {
                // 洪峰丢弃计数：诊断可见（§17），绝不阻塞 reader
                let n = shared.dropped_events.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 1000 == 1 {
                    tracing::warn!(dropped = n, "event queue overflow");
                }
            }
        }
    }
    // reader 结束 => events channel 随 Shared drop 关闭，Endpoint 以 recv()==None 感知断开。
    // NOTE: events_tx 存于 Shared，Session 全部克隆销毁后才真正关闭——Endpoint 运行时
    // 通过 is_unresponsive/请求失败等信号兜底感知断连。
}

impl Drop for Session {
    fn drop(&mut self) {
        self.reader_cancel.cancel();
    }
}
