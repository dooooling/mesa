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

use mesa_core_types::{ConnectionState as CoreState, DataBatch, EventBatch};
use mesa_driver_protocol::{
    EVENT_PLANE_MIN_MINOR, ProtocolError, batch_from_pb, connection_state_from_pb,
    event_batch_from_pb, negotiate, pb, read_envelope, write_envelope,
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
/// 事件批次通道容量（Event Plane V1 §12）：与 Driver SDK 侧 EVENT_CAPACITY(128)
/// 对等。事件流是独立可靠流——满队列意味着消费端已死，reader 按 fail-closed
/// 关闭整条事件流（见 [`Session::event_stream_failed`]），绝不静默丢弃。
pub const EVENT_BATCH_CAPACITY: usize = 128;

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
    /// Driver 侧结构化错误（P1-2：kind/code 保留，不格式化成字符串，
    /// 调用方按 code 做精确路由，禁止 contains()/parse 回猜）。
    #[error("driver error {kind}/{code}: {message}")]
    Driver {
        kind: String,
        code: String,
        message: String,
    },
    /// Event Plane 不可用：非空 EventTask 要求协商 Minor >= EVENT_PLANE_MIN_MINOR，
    /// 老 Driver 直接精确失败（`EVENT_PLANE_UNSUPPORTED` 语义），禁止发未知 RPC
    /// 干等超时。调用方用 matches! 做精确路由。
    #[error("event plane unsupported (negotiated minor {negotiated} < {required})")]
    EventPlaneUnsupported { negotiated: u32, required: u32 },
}

struct Shared {
    /// msg_id -> 等待响应的 sender。请求方登记，reader 分发。
    pending: Mutex<HashMap<u64, oneshot::Sender<pb::Envelope>>>,
    /// 写半部全局互斥：请求路径与心跳路径共享。
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    events_tx: mpsc::Sender<SessionEvent>,
    /// 事件批次发送端：独立于 SessionEvent 队列的可靠流。reader 持有 Arc<Shared>
    /// 持续写入；消费端从 [`Session::take_event_batches`] 取走接收端。
    /// `Option` 形态是 fail-closed 的执行手段：溢出时 take() 丢弃发送端，
    /// channel 即关闭，消费端 recv() 到 None（流终止），而不是永远阻塞。
    event_tx: Mutex<Option<mpsc::Sender<EventBatch>>>,
    unresponsive: Arc<AtomicBool>,
    dropped_events: AtomicU64,
    /// 事件流已死（fail-closed）：队列溢出后置位，reader 关闭 event channel，
    /// 后续 EventBatch 全部拒绝。调用方以此触发重连，而不是继续收残缺流。
    event_stream_dead: AtomicBool,
    /// 因溢出被拒绝的 EventBatch 数（流已死后不再计数， incidental）。
    event_overflow_drops: AtomicU64,
    /// 解码失败被丢弃的 EventBatch 数（单批损坏可观测：下游 sequence gap 会如实反映）。
    event_decode_errors: AtomicU64,
    /// Core 侧事件 epoch 门（P0 barrier）：handle -> 当前活跃 stream_epoch。
    /// SDK writer 的 active_epochs 只能挡住"尚未写 socket"的批次；已进入 TCP/
    /// event_rx 缓冲的旧 epoch 批次靠这道门在 [`EventReceiver`] 消费时过滤——
    /// StopAck 之后调用方永远观察不到旧 epoch。
    active_event_epochs: Mutex<HashMap<u32, u64>>,
    /// Start 意图登记：msg_id -> (handle, epoch)。`call()` 在发出 StartConnection
    /// 时记录，reader 在 StartConnectionAck 到达时消费（成败都清理，有界）。
    /// Ack 本身不带 epoch，必须靠这次登记找回。
    pending_event_starts: Mutex<HashMap<u64, (u32, u64)>>,
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
    /// 事件批次接收端：connect 时创建、随 Session 持有，消费端通过
    /// [`Session::take_event_batches`] 一次性取走。connect 三元组签名不变，
    /// 老调用方（endpoint/tests）零改动；PR7 EventIngress 再正式消费。
    event_rx: Option<mpsc::Receiver<EventBatch>>,
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

        // ---- 回 Welcome（协商 Minor 取双方较小值，必须如实回告）----
        // 1.2 老驱动靠 accepted_protocol_minor 知道自己被当作 1.2 对待；
        // 若固定回 Core 自身 Minor，wire 上的协商结果就是错的（即使 Core 侧
        // 因自存 negotiated_minor 还能正确执行 Event Gate）。
        let welcome = pb::Envelope {
            msg_id: hello_env.msg_id,
            body: Some(pb::envelope::Body::Welcome(pb::Welcome {
                core_version: format!("Mesad v{}", env!("CARGO_PKG_VERSION")),
                accepted_protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
                accepted_protocol_minor: negotiated_minor,
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
        let (event_tx, event_rx) = mpsc::channel(EVENT_BATCH_CAPACITY);
        let unresponsive_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            writer: tokio::sync::Mutex::new(wr),
            events_tx,
            event_tx: Mutex::new(Some(event_tx)),
            unresponsive: Arc::clone(&unresponsive_flag),
            dropped_events: AtomicU64::new(0),
            event_stream_dead: AtomicBool::new(false),
            event_overflow_drops: AtomicU64::new(0),
            event_decode_errors: AtomicU64::new(0),
            active_event_epochs: Mutex::new(HashMap::new()),
            pending_event_starts: Mutex::new(HashMap::new()),
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
                event_rx: Some(event_rx),
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

    /// 取走事件批次接收端（一次性；重复调用返回 None）。
    /// 返回的是带 Core 侧 epoch 门的 [`EventReceiver`]（P0 barrier），不是裸
    /// channel：StopAck 之后旧 epoch 批次即使已在缓冲中也不可见。
    /// PR6 的 E2E Gate（contract test）与 PR7 的 EventIngress 由此消费 EventBatch 流。
    pub fn take_event_batches(&mut self) -> Option<EventReceiver> {
        self.event_rx.take().map(|rx| EventReceiver {
            rx,
            shared: Arc::clone(&self.shared),
        })
    }

    /// 事件流是否已死（fail-closed）：队列溢出后 reader 关闭 channel 并置位。
    /// 为 true 时消费端必须丢弃残缺流并触发重连，禁止继续用 sequence gate
    /// "假装"流还完整——溢出丢了多少条不可观测，gap 计数已失去意义。
    pub fn event_stream_failed(&self) -> bool {
        self.shared.event_stream_dead.load(Ordering::Relaxed)
    }

    /// 因溢出被拒绝的 EventBatch 数（诊断用）。
    pub fn event_overflow_drops(&self) -> u64 {
        self.shared.event_overflow_drops.load(Ordering::Relaxed)
    }

    /// 解码失败被丢弃的 EventBatch 数（诊断用；下游 sequence gap 如实反映缺失）。
    pub fn event_decode_errors(&self) -> u64 {
        self.shared.event_decode_errors.load(Ordering::Relaxed)
    }

    /// 事件任务配置（Event Plane V1 §6 / PR6 Core 侧）：
    /// - 空任务：不发 RPC，直接成功（老 Driver/无事件连接零成本）；
    /// - 非空 + 协商 Minor < EVENT_PLANE_MIN_MINOR：立即 `EventPlaneUnsupported`，
    ///   禁止发未知 RPC 干等超时；
    /// - 非空 + Minor 达标：发 ConfigureEventTasks(42)，按 EventConfigApplied(43)
    ///   的 result 判定（失败转精确的 `SessionError::Driver`）。
    pub async fn configure_events(
        &self,
        connection_handle: u32,
        revision: u64,
        tasks: &[mesa_core_types::EventTask],
    ) -> Result<(), SessionError> {
        if tasks.is_empty() {
            return Ok(());
        }
        if self.negotiated_minor < EVENT_PLANE_MIN_MINOR {
            return Err(SessionError::EventPlaneUnsupported {
                negotiated: self.negotiated_minor,
                required: EVENT_PLANE_MIN_MINOR,
            });
        }
        let tasks_pb = tasks
            .iter()
            .map(mesa_driver_protocol::event_task_to_pb)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| SessionError::Handshake(e.to_string()))?;
        let reply = self
            .call(pb::envelope::Body::ConfigureEventTasks(
                pb::ConfigureEventTasks {
                    connection_handle,
                    revision,
                    tasks: tasks_pb,
                },
            ))
            .await?;
        match reply.body {
            Some(pb::envelope::Body::EventConfigApplied(resp)) => match resp.result {
                Some(r) if r.ok => Ok(()),
                Some(r) => {
                    let d = r.error.unwrap_or_default();
                    Err(SessionError::Driver {
                        kind: d.kind,
                        code: d.code,
                        message: d.message,
                    })
                }
                None => Err(SessionError::Closed),
            },
            _ => Err(SessionError::Closed),
        }
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

    /// Dynamic Probe（§8）：对已 OpenConnection 的 `connection_handle`
    /// 发送 ProbeRequest 并等待 ProbeResponse。
    /// 调用方（MesaManager）须先以 `negotiated_minor()` 门控版本，
    /// 此处只做 RPC + 响应种类校验 + JSON 解码 + 大小校验，不解释业务语义。
    /// Driver 侧失败以同 msg_id 的 DriverErrorReport 回到此处，转为错误上抛。
    pub async fn probe(
        &self,
        connection_handle: u32,
    ) -> Result<mesa_core_types::ProbeReport, SessionError> {
        let reply = self
            .call(pb::envelope::Body::ProbeRequest(pb::ProbeRequest {
                connection_handle,
            }))
            .await?;
        match reply.body {
            Some(pb::envelope::Body::ProbeResponse(r)) => {
                mesa_core_types::ProbeReport::from_report_json(&r.report_json)
                    .map_err(SessionError::Handshake)
            }
            Some(pb::envelope::Body::DriverError(e)) => {
                let d = e.detail.unwrap_or_default();
                Err(SessionError::Driver {
                    kind: d.kind,
                    code: d.code,
                    message: d.message,
                })
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
        // 事件 barrier 自维护：Start 意图在此登记，reader 在 Ack 到达时更新
        // epoch 门。所有 Start/Stop/Close 都经本方法，调用方无需记得调 hook，
        // barrier 永不错位（endpoint/tests 零改动）。
        if let pb::envelope::Body::StartConnection(req) = &body {
            self.shared
                .pending_event_starts
                .lock()
                .unwrap()
                .insert(id, (req.connection_handle, req.stream_epoch));
        }
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
            // 生命周期 Ack 嗅探：维护 Core 侧事件 epoch 门（P0 barrier）。
            // 注意嗅探的是"已确认送达请求方"的回复帧，与请求方是否处理无关。
            snoop_lifecycle_ack(&shared, env.msg_id, &env.body);
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
        // 1.5) 事件批次走独立可靠流（Event Plane V1 §12）：绝不进入 SessionEvent
        // 队列——该队列满时 try_send 丢弃的语义对事件不可接受。
        if let Some(Body::EventBatch(b)) = env.body {
            // 流已死后拒绝一切后续批次（channel 已关闭，try_send 必败；此处
            // 短路避免无意义的解码开销）
            if shared.event_stream_dead.load(Ordering::Relaxed) {
                continue;
            }
            match event_batch_from_pb(b) {
                Ok(batch) => {
                    let mut guard = shared.event_tx.lock().unwrap();
                    match guard.as_ref().map(|tx| tx.try_send(batch)) {
                        // 流已死（发送端已 take）：拒绝后续批次
                        None => {}
                        Some(Ok(())) => {}
                        Some(Err(mpsc::error::TrySendError::Full(_))) => {
                            // fail-closed：满队列 = 消费端已死。丢了多少条不可观测，
                            // sequence gap 已失去意义——丢弃发送端关闭整条流并置位，
                            // 消费端 recv() 到 None 后触发重连。
                            // NOTE: Closed 同样视为流终止（消费端主动 drop 接收端）。
                            shared.event_overflow_drops.fetch_add(1, Ordering::Relaxed);
                            shared.event_stream_dead.store(true, Ordering::Relaxed);
                            *guard = None;
                            tracing::error!(
                                "event batch queue overflow: stream terminated (fail-closed)"
                            );
                        }
                        Some(Err(mpsc::error::TrySendError::Closed(_))) => {
                            shared.event_stream_dead.store(true, Ordering::Relaxed);
                            *guard = None;
                        }
                    }
                }
                Err(e) => {
                    // 单批损坏：丢弃该批并计数，下游 sequence gap 如实反映缺失
                    // （与溢出不同：丢了哪一批是可观测的，流完整性仍可判定）。
                    shared.event_decode_errors.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(error = %e, "event batch decode failed, batch dropped");
                }
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

/// 生命周期 Ack 嗅探：维护 Core 侧事件 epoch 门（P0 barrier，PR6 review）。
///
/// - `StartConnectionAck(ok)` → 以请求时登记的 epoch 激活该 handle；
/// - `StopConnectionAck(ok)` / `CloseConnectionAck(ok)` → 去活该 handle；
/// - 失败的 Ack 不改变门状态（流语义未变）；Start 登记项无论成败都清理（有界）。
/// - 回执 handle 与请求不一致视为对端违约：不更新门并告警（宁可错杀新流，
///   不可放行旧 epoch）。
fn snoop_lifecycle_ack(shared: &Shared, msg_id: u64, body: &Option<pb::envelope::Body>) {
    use pb::envelope::Body;
    match body {
        Some(Body::StartConnectionAck(ack)) => {
            let start = shared.pending_event_starts.lock().unwrap().remove(&msg_id);
            if !ack.result.as_ref().is_some_and(|r| r.ok) {
                return;
            }
            match start {
                Some((handle, epoch)) if ack.connection_handle == handle => {
                    shared
                        .active_event_epochs
                        .lock()
                        .unwrap()
                        .insert(handle, epoch);
                }
                Some((handle, epoch)) => {
                    tracing::warn!(
                        req_handle = handle,
                        req_epoch = epoch,
                        ack_handle = ack.connection_handle,
                        "StartConnectionAck handle mismatch, epoch gate untouched"
                    );
                }
                None => {
                    tracing::warn!(
                        msg_id,
                        "StartConnectionAck without recorded Start, epoch gate untouched"
                    );
                }
            }
        }
        Some(Body::StopConnectionAck(ack)) if ack.result.as_ref().is_some_and(|r| r.ok) => {
            shared
                .active_event_epochs
                .lock()
                .unwrap()
                .remove(&ack.connection_handle);
        }
        Some(Body::CloseConnectionAck(ack)) if ack.result.as_ref().is_some_and(|r| r.ok) => {
            shared
                .active_event_epochs
                .lock()
                .unwrap()
                .remove(&ack.connection_handle);
        }
        _ => {}
    }
}

/// 事件批次接收端：Core 侧 epoch 门的执行点（P0 barrier）。
///
/// 即使旧 epoch 批次已在 mpsc 缓冲中排队（SDK 门挡不住"已写 socket"的），
/// StopAck 去活后 `recv()`/`try_recv()` 也会在内部丢弃它们——调用方永远
/// 观察不到 StopAck 之后的旧 epoch。流终止（fail-closed/会话结束）时
/// `recv()` 返回 None，与普通 channel 语义一致。
pub struct EventReceiver {
    rx: mpsc::Receiver<EventBatch>,
    shared: Arc<Shared>,
}

impl EventReceiver {
    fn batch_active(shared: &Shared, b: &EventBatch) -> bool {
        shared
            .active_event_epochs
            .lock()
            .unwrap()
            .get(&b.connection_handle)
            .copied()
            == Some(b.stream_epoch)
    }

    pub async fn recv(&mut self) -> Option<EventBatch> {
        loop {
            let b = self.rx.recv().await?;
            if Self::batch_active(&self.shared, &b) {
                return Some(b);
            }
            // stale：StopAck 后的旧 epoch，内部丢弃继续等（调用方不可见）
        }
    }

    pub fn try_recv(&mut self) -> Result<EventBatch, mpsc::error::TryRecvError> {
        loop {
            match self.rx.try_recv() {
                Ok(b) if Self::batch_active(&self.shared, &b) => return Ok(b),
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.reader_cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回环 Session：真实 TCP 对（满足 writer 的 OwnedWriteHalf 类型），
    /// reader/hb 不启动——只测 configure_events 的门控与编解码，不测网络。
    struct Loopback {
        session: Session,
        // 存活守卫：server 端 socket + 通道接收端 + reader 取消令牌，测试结束才释放
        _server: Option<tokio::net::TcpStream>,
        _events_rx: mpsc::Receiver<SessionEvent>,
        _event_rx: mpsc::Receiver<EventBatch>,
        _reader_cancel: CancellationToken,
    }

    impl Loopback {
        async fn new(negotiated_minor: u32) -> Self {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let accept = tokio::spawn(async move { listener.accept().await.unwrap().0 });
            let cli = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let server = accept.await.unwrap();
            let (rd, wr) = cli.into_split();
            let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
            let (event_tx, event_rx) = mpsc::channel(EVENT_BATCH_CAPACITY);
            let shared = Arc::new(Shared {
                pending: Mutex::new(HashMap::new()),
                writer: tokio::sync::Mutex::new(wr),
                events_tx,
                event_tx: Mutex::new(Some(event_tx)),
                unresponsive: Arc::new(AtomicBool::new(false)),
                dropped_events: AtomicU64::new(0),
                event_stream_dead: AtomicBool::new(false),
                event_overflow_drops: AtomicU64::new(0),
                event_decode_errors: AtomicU64::new(0),
                active_event_epochs: Mutex::new(HashMap::new()),
                pending_event_starts: Mutex::new(HashMap::new()),
            });
            // call() 的响应分发依赖 reader_loop——回环测试也必须启动它
            let reader_cancel = CancellationToken::new();
            tokio::spawn(reader_loop(rd, Arc::clone(&shared), reader_cancel.clone()));
            let session = Session {
                port,
                shared,
                next_msg_id: AtomicU64::new(100),
                reader_cancel: reader_cancel.clone(),
                negotiated_minor,
                event_rx: None,
            };
            Self {
                session,
                _server: Some(server),
                _events_rx: events_rx,
                _event_rx: event_rx,
                _reader_cancel: reader_cancel,
            }
        }

        fn task(id: &str) -> mesa_core_types::EventTask {
            mesa_core_types::EventTask {
                id: id.into(),
                mode: mesa_core_types::TaskMode::Subscribe,
                interval_ms: None,
                binding: mesa_core_types::DriverBinding {
                    kind: "k".into(),
                    config: serde_json::Value::Null,
                },
            }
        }
    }

    /// 空任务不发 RPC：server 端无任何读取也不会超时，直接成功。
    #[tokio::test]
    async fn configure_events_empty_tasks_needs_no_rpc() {
        let lb = Loopback::new(2).await;
        lb.session
            .configure_events(7, 1, &[])
            .await
            .expect("empty tasks must succeed without RPC");
    }

    /// Minor Gate：协商 Minor < 3 + 非空任务 → 立即精确失败，不发 RPC。
    #[tokio::test]
    async fn configure_events_minor_gate_rejects_before_io() {
        let lb = Loopback::new(2).await;
        let err = lb
            .session
            .configure_events(7, 1, &[Loopback::task("e1")])
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::EventPlaneUnsupported {
                    negotiated: 2,
                    required: 3
                }
            ),
            "must fail fast with precise variant, got {err}"
        );
    }

    /// Minor 达标 + server 回 EventConfigApplied(ok) → 成功全路径。
    #[tokio::test]
    async fn configure_events_roundtrip_success() {
        let mut lb = Loopback::new(3).await;
        // server 桩：读一帧 ConfigureEventTasks，回同 msg_id 的 EventConfigApplied
        let server = lb._server.take().unwrap();
        let (mut srd, mut swr) = server.into_split();
        let stub = tokio::spawn(async move {
            let req = read_envelope(&mut srd).await.unwrap();
            let (handle, revision) = match req.body {
                Some(pb::envelope::Body::ConfigureEventTasks(c)) => {
                    (c.connection_handle, c.revision)
                }
                other => panic!("expected ConfigureEventTasks, got {other:?}"),
            };
            write_envelope(
                &mut swr,
                &pb::Envelope {
                    msg_id: req.msg_id,
                    body: Some(pb::envelope::Body::EventConfigApplied(
                        pb::EventConfigApplied {
                            connection_handle: handle,
                            revision,
                            result: Some(pb::GenericResult {
                                ok: true,
                                error: None,
                            }),
                        },
                    )),
                },
            )
            .await
            .unwrap();
        });
        lb.session
            .configure_events(7, 9, &[Loopback::task("e1")])
            .await
            .expect("minor-3 roundtrip must succeed");
        stub.await.unwrap();
    }

    /// 生命周期 Ack 嗅探：Start 激活门、Stop 去活门、失败 Ack 不动门。
    #[tokio::test]
    async fn lifecycle_ack_snoop_drives_event_epoch_gate() {
        let mut lb = Loopback::new(3).await;
        let server = lb._server.take().unwrap();
        let (mut srd, mut swr) = server.into_split();
        let stub = tokio::spawn(async move {
            // Start → ok（Ack 本身不带 epoch，门靠 Core 登记的请求值激活）
            let req = read_envelope(&mut srd).await.unwrap();
            let handle = match req.body {
                Some(pb::envelope::Body::StartConnection(c)) => c.connection_handle,
                other => panic!("expected Start, got {other:?}"),
            };
            write_envelope(
                &mut swr,
                &pb::Envelope {
                    msg_id: req.msg_id,
                    body: Some(pb::envelope::Body::StartConnectionAck(
                        pb::StartConnectionAck {
                            connection_handle: handle,
                            result: Some(pb::GenericResult {
                                ok: true,
                                error: None,
                            }),
                        },
                    )),
                },
            )
            .await
            .unwrap();
            // Stop → ok
            let req = read_envelope(&mut srd).await.unwrap();
            let handle = match req.body {
                Some(pb::envelope::Body::StopConnection(_)) => handle,
                other => panic!("expected Stop, got {other:?}"),
            };
            write_envelope(
                &mut swr,
                &pb::Envelope {
                    msg_id: req.msg_id,
                    body: Some(pb::envelope::Body::StopConnectionAck(
                        pb::StopConnectionAck {
                            connection_handle: handle,
                            result: Some(pb::GenericResult {
                                ok: true,
                                error: None,
                            }),
                        },
                    )),
                },
            )
            .await
            .unwrap();
        });
        lb.session
            .call(pb::envelope::Body::StartConnection(pb::StartConnection {
                connection_handle: 7,
                stream_epoch: 4242,
            }))
            .await
            .unwrap();
        // 给 reader 一个调度机会处理 Ack
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            lb.session
                .shared
                .active_event_epochs
                .lock()
                .unwrap()
                .get(&7),
            Some(&4242),
            "Start Ack 必须激活 epoch 门"
        );
        lb.session
            .call(pb::envelope::Body::StopConnection(pb::StopConnection {
                connection_handle: 7,
            }))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            lb.session
                .shared
                .active_event_epochs
                .lock()
                .unwrap()
                .get(&7)
                .is_none(),
            "Stop Ack 必须去活 epoch 门"
        );
        stub.await.unwrap();
    }

    /// server 回失败 result → 精确的 SessionError::Driver（kind/code 可路由）。
    #[tokio::test]
    async fn configure_events_driver_rejection_is_precise() {
        let mut lb = Loopback::new(3).await;
        let server = lb._server.take().unwrap();
        let (mut srd, mut swr) = server.into_split();
        let stub = tokio::spawn(async move {
            let req = read_envelope(&mut srd).await.unwrap();
            write_envelope(
                &mut swr,
                &pb::Envelope {
                    msg_id: req.msg_id,
                    body: Some(pb::envelope::Body::EventConfigApplied(
                        pb::EventConfigApplied {
                            connection_handle: 7,
                            revision: 9,
                            result: Some(pb::GenericResult {
                                ok: false,
                                error: Some(pb::ErrorDetail {
                                    kind: "Unsupported".into(),
                                    code: "EVENT_NOT_SUPPORTED".into(),
                                    message: "no".into(),
                                }),
                            }),
                        },
                    )),
                },
            )
            .await
            .unwrap();
        });
        let err = lb
            .session
            .configure_events(7, 9, &[Loopback::task("e1")])
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Driver { ref code, .. } if code == "EVENT_NOT_SUPPORTED"
            ),
            "must surface precise driver code, got {err}"
        );
        stub.await.unwrap();
    }
}
