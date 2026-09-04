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

use mesa_core_types::{ConnectionState as CoreState, DataBatch};
use mesa_driver_protocol::{
    ProtocolError, batch_from_pb, connection_state_from_pb, negotiate, pb, read_envelope,
    write_envelope,
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
        if std::env::var("MESA_HEARTBEAT_FAST").ok().as_deref() == Some("1") {
            return Self {
                ping_period: Duration::from_secs(1),
                pong_deadline: Duration::from_secs(1),
                max_missed: 2,
            };
        }
        Self {
            ping_period: PING_PERIOD,
            pong_deadline: PONG_DEADLINE,
            max_missed: MAX_MISSED_PONGS,
        }
    }
}

/// 上行事件容量。控制类事件不允许静默丢弃，消费端必须活跃；
/// 容量仅作瞬时洪峰缓冲，溢出计入诊断计数。
pub const EVENT_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Batch(DataBatch),
    State {
        handle: u32,
        state: CoreState,
        detail: String,
    },
    DriverError {
        handle: Option<u32>,
        kind: String,
        code: String,
        message: String,
    },
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
    /// 握手协商出的 Minor（双方较小值）。Probe RPC 要求 >= 2，
    /// 旧 Driver 直接返回 Unsupported，不发 RPC 干等超时。
    negotiated_minor: u32,
}

/// 驱动启动窗口：从 spawn 到 listen 的容忍期（§14，Probe 外层需更大 budget）
pub const DRIVER_STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
/// Probe 整体超时需覆盖 startup + handshake + OpenConnection
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

impl Session {
    /// 带重试的连接：子进程从 bind 到 listen 存在窗口期，连接拒绝属预期时序，
    /// 自动退避重试；其他错误立即失败。基于 deadline 而非固定次数，避免与外层超时打架。
    pub async fn connect_retry(
        port: u16,
        expected_token: &str,
    ) -> Result<(Self, mpsc::Receiver<SessionEvent>, Arc<AtomicBool>), SessionError> {
        let deadline = tokio::time::Instant::now() + DRIVER_STARTUP_TIMEOUT;
        #[allow(unused_assignments)]
        let mut last: Option<SessionError> = None;
        loop {
            match Self::connect(port, expected_token).await {
                Ok(v) => return Ok(v),
                Err(SessionError::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    last = Some(SessionError::Io(e));
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
            if tokio::time::Instant::now() >= deadline {
                break;
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
        let (_, negotiated_minor) = negotiate(
            (hello.protocol_major, hello.protocol_minor),
            (
                mesa_driver_protocol::PROTOCOL_MAJOR,
                mesa_driver_protocol::PROTOCOL_MINOR,
            ),
        )
        .map_err(|e| SessionError::Handshake(e.to_string()))?;

        // ---- 回 Welcome（协商 Minor 取双方较小值）----
        let welcome = pb::Envelope {
            msg_id: hello_env.msg_id,
            body: Some(pb::envelope::Body::Welcome(pb::Welcome {
                core_version: format!("Mesad v{}", env!("CARGO_PKG_VERSION")),
                accepted_protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
                accepted_protocol_minor: mesa_driver_protocol::PROTOCOL_MINOR,
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
                let env = pb::Envelope {
                    msg_id: id,
                    body: Some(pb::envelope::Body::Ping(pb::Ping {})),
                };
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
            Self {
                port,
                shared,
                next_msg_id: AtomicU64::new(100),
                reader_cancel,
                negotiated_minor,
            },
            events_rx,
            unresponsive_flag,
        ))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// 握手协商出的 Minor（probe 门控用）。
    pub fn negotiated_minor(&self) -> u32 {
        self.negotiated_minor
    }

    pub fn is_unresponsive(&self) -> bool {
        self.shared.unresponsive.load(Ordering::Relaxed)
    }

    // ---- 协议化请求封装 ----

    /// GetMetadata：返回 (driver_id, name, version)。
    pub async fn metadata(&self) -> Result<(String, String, String), SessionError> {
        let reply = self
            .call(pb::envelope::Body::GetMetadata(pb::GetMetadata {}))
            .await?;
        match reply.body {
            Some(pb::envelope::Body::MetadataReport(m)) => Ok((m.driver_id, m.name, m.version)),
            _ => Err(SessionError::Closed),
        }
    }

    /// GetDescriptor（§4.4）：5s 超时、256KiB 上限，返回 (major, minor, json)。
    pub async fn get_descriptor(&self) -> Result<(u32, u32, String), SessionError> {
        // 使用独立超时 5s（§4.4），而非通用的 10s
        let id = self.next_msg_id.fetch_add(1, Ordering::Relaxed);
        let rx = self.shared.register(id);
        let env = pb::Envelope {
            msg_id: id,
            body: Some(pb::envelope::Body::GetDescriptor(pb::GetDescriptor {})),
        };
        {
            let mut wr = self.shared.writer.lock().await;
            write_envelope(&mut *wr, &env).await?;
        }
        let reply = match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => return Err(SessionError::Closed),
            Err(_) => {
                self.shared.unregister(id);
                return Err(SessionError::Timeout);
            }
        };
        match reply.body {
            Some(pb::envelope::Body::DescriptorReport(d)) => {
                if d.descriptor_json.len() > 256 * 1024 {
                    return Err(SessionError::Handshake("descriptor too large".into()));
                }
                // 校验 JSON 可解析性由调用方完成，此处仅透传
                Ok((d.contract_major, d.contract_minor, d.descriptor_json))
            }
            _ => Err(SessionError::Closed),
        }
    }

    /// Dynamic Probe（§8）：发送 ProbeRequest 并等待 ProbeResponse。
    /// 调用方（MesaManager）须先以 `negotiated_minor()` 门控版本，
    /// 此处只做 RPC + 响应种类校验 + JSON 解码 + 大小校验，不解释业务语义。
    /// Driver 侧失败以同 msg_id 的 DriverErrorReport 回到此处，转为错误上抛。
    pub async fn probe(
        &self,
        connection_json: &str,
    ) -> Result<mesa_core_types::ProbeReport, SessionError> {
        let reply = self
            .call(pb::envelope::Body::ProbeRequest(pb::ProbeRequest {
                connection_json: connection_json.to_string(),
            }))
            .await?;
        match reply.body {
            Some(pb::envelope::Body::ProbeResponse(r)) => {
                mesa_core_types::ProbeReport::from_report_json(&r.report_json)
                    .map_err(SessionError::Handshake)
            }
            Some(pb::envelope::Body::DriverError(e)) => {
                let d = e.detail.unwrap_or_default();
                Err(SessionError::Handshake(format!(
                    "{}/{}: {}",
                    d.kind, d.code, d.message
                )))
            }
            _ => Err(SessionError::Closed),
        }
    }

    /// Write（§22 可靠控制，无 Latest-Wins）：发送 WriteRequest 并等待 WriteResponse
    pub async fn write(
        &self,
        connection_handle: u32,
        request_id: &str,
        target: &str,
        value: mesa_core_types::Value,
        expected: Option<mesa_core_types::Value>,
    ) -> Result<Option<mesa_core_types::Value>, SessionError> {
        let id = self.next_msg_id.fetch_add(1, Ordering::Relaxed);
        let rx = self.shared.register(id);
        let env = pb::Envelope {
            msg_id: id,
            body: Some(pb::envelope::Body::WriteRequest(pb::WriteRequest {
                connection_handle,
                request_id: request_id.to_string(),
                target: target.to_string(),
                value: Some(mesa_driver_protocol::value_to_pb(&value)),
                expected_value: expected.as_ref().map(mesa_driver_protocol::value_to_pb),
            })),
        };
        {
            let mut wr = self.shared.writer.lock().await;
            write_envelope(&mut *wr, &env).await?;
        }
        let reply = match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => return Err(SessionError::Closed),
            Err(_) => {
                self.shared.unregister(id);
                return Err(SessionError::Timeout);
            }
        };
        match reply.body {
            Some(pb::envelope::Body::WriteResponse(resp)) => {
                if let Some(result) = resp.result
                    && !result.ok
                {
                    let d = result.error.unwrap_or_default();
                    return Err(SessionError::Handshake(format!(
                        "{}/{}: {}",
                        d.kind, d.code, d.message
                    )));
                }
                if let Some(v) = resp.readback {
                    Ok(Some(
                        mesa_driver_protocol::value_from_pb(v)
                            .map_err(|e| SessionError::Handshake(e.to_string()))?,
                    ))
                } else {
                    Ok(None)
                }
            }
            _ => Err(SessionError::Closed),
        }
    }

    /// Command（§22）：发送 CommandRequest 并等待 CommandResponse
    pub async fn command(
        &self,
        connection_handle: u32,
        request_id: &str,
        command_id: &str,
        input_json: &str,
    ) -> Result<(String, String, String), SessionError> {
        let id = self.next_msg_id.fetch_add(1, Ordering::Relaxed);
        let rx = self.shared.register(id);
        let env = pb::Envelope {
            msg_id: id,
            body: Some(pb::envelope::Body::CommandRequest(pb::CommandRequest {
                connection_handle,
                request_id: request_id.to_string(),
                command_id: command_id.to_string(),
                input_json: input_json.to_string(),
            })),
        };
        {
            let mut wr = self.shared.writer.lock().await;
            write_envelope(&mut *wr, &env).await?;
        }
        let reply = match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => return Err(SessionError::Closed),
            Err(_) => {
                self.shared.unregister(id);
                return Err(SessionError::Timeout);
            }
        };
        match reply.body {
            Some(pb::envelope::Body::CommandResponse(resp)) => {
                Ok((resp.status, resp.result_json, resp.error))
            }
            _ => Err(SessionError::Closed),
        }
    }

    /// Browse（§20）：分页浏览，返回 (nodes, next_cursor)
    pub async fn browse(
        &self,
        connection_handle: u32,
        parent: &str,
        filter: &str,
        cursor: &str,
        limit: u32,
    ) -> Result<(Vec<pb::BrowseNode>, Option<String>), SessionError> {
        let id = self.next_msg_id.fetch_add(1, Ordering::Relaxed);
        let rx = self.shared.register(id);
        let env = pb::Envelope {
            msg_id: id,
            body: Some(pb::envelope::Body::BrowseRequest(pb::BrowseRequest {
                connection_handle,
                parent: parent.to_string(),
                filter: filter.to_string(),
                cursor: cursor.to_string(),
                limit,
            })),
        };
        {
            let mut wr = self.shared.writer.lock().await;
            write_envelope(&mut *wr, &env).await?;
        }
        let reply = match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => return Err(SessionError::Closed),
            Err(_) => {
                self.shared.unregister(id);
                return Err(SessionError::Timeout);
            }
        };
        match reply.body {
            Some(pb::envelope::Body::BrowseResponse(resp)) => {
                if let Some(result) = resp.result
                    && !result.ok
                {
                    let d = result.error.unwrap_or_default();
                    return Err(SessionError::Handshake(format!(
                        "{}/{}: {}",
                        d.kind, d.code, d.message
                    )));
                }
                let next = if resp.next_cursor.is_empty() {
                    None
                } else {
                    Some(resp.next_cursor)
                };
                Ok((resp.nodes, next))
            }
            _ => Err(SessionError::Closed),
        }
    }

    /// 发送一帧并等待同 msg_id 的响应帧。超时即清理登记项，防止泄漏。
    pub async fn call(&self, body: pb::envelope::Body) -> Result<pb::Envelope, SessionError> {
        let id = self.next_msg_id.fetch_add(1, Ordering::Relaxed);
        let rx = self.shared.register(id);
        let env = pb::Envelope {
            msg_id: id,
            body: Some(body),
        };
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
        let env = pb::Envelope {
            msg_id: id,
            body: Some(body),
        };
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
            if is_driver_error && let Some(Body::DriverError(e)) = env.body {
                let d = e.detail.unwrap_or_default();
                let ev = SessionEvent::DriverError {
                    handle: e.connection_handle,
                    kind: d.kind,
                    code: d.code,
                    message: d.message,
                };
                let _ = shared.events_tx.try_send(ev);
            }
            continue;
        }
        // 2) 其余帧转为上行事件
        let ev =
            match env.body {
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
        if let Some(ev) = ev
            && shared.events_tx.try_send(ev).is_err()
        {
            // 洪峰丢弃计数：诊断可见（§17），绝不阻塞 reader
            let n = shared.dropped_events.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 1000 == 1 {
                tracing::warn!(dropped = n, "event queue overflow");
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
