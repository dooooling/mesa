//! ForgeLink Driver SDK（方案 §16）。
//!
//! SDK 承担 Driver 进程的全部通用职责：IPC、session token 认证、心跳应答、
//! DataBatch 序列化与背压合并、Shutdown、父进程 liveness 防护。协议开发者只需
//! 实现 [`Driver`] / [`DriverConnection`] 两个 trait，不接触任何 IPC 细节。
//!
//! 背压模型（方案 §12）：SDK 内部出站队列有界；数据批次在队列满时执行
//! Latest-Wins Coalescing（同 point_id 仅保留最新值）；控制类消息走阻塞式入队，
//! 永不丢弃。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forgelink_core_types::{
    ConnectionState, DataBatch, DriverMetadata, ErrorKind, PointDescriptor, PointMap,
};
use forgelink_driver_protocol::{
    batch_to_pb, err_result, error_detail, ok_result, pb, read_envelope, tasks_from_pb,
    write_envelope, ConvertError, ProtocolError, PROTOCOL_MAJOR, PROTOCOL_MINOR,
};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 出站队列容量。M0 取保守值；性能预算阶段（§22）按 50K updates/s 压测结果调整。
const OUTBOUND_CAPACITY: usize = 256;

/// 握手阶段读超时。超时视为对端异常，直接断开。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Stop 时等待采集循环收尾的宽限。M0 阶段采集循环只做 tick 级清理，50ms 足够；
/// 真实协议驱动若需关闭 socket/session，应在其 run() 内响应取消并自行限时。
const RUN_DRAIN_GRACE: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// 驱动开发者面对的 trait 与错误类型
// ---------------------------------------------------------------------------

/// Driver 侧业务错误：统一错误类别 + 原因码 + 可读信息（方案 §13）。
#[derive(Debug, Clone)]
pub struct SdkDriverError {
    pub kind: ErrorKind,
    pub code: String,
    pub message: String,
}

impl SdkDriverError {
    pub fn new(kind: ErrorKind, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { kind, code: code.into(), message: message.into() }
    }

    pub fn configuration(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Configuration, code, message)
    }
}

impl std::fmt::Display for SdkDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 展示形态：<类别>/<原因码>: <信息>，如 ConfigurationError/DUPLICATE_POINT_KEY: ...
        if self.code.is_empty() {
            write!(f, "{}: {}", self.kind.as_str(), self.message)
        } else {
            write!(f, "{}/{}: {}", self.kind.as_str(), self.code, self.message)
        }
    }
}

impl std::error::Error for SdkDriverError {}

impl From<ConvertError> for SdkDriverError {
    fn from(e: ConvertError) -> Self {
        Self::new(ErrorKind::Decode, "", e.to_string())
    }
}

/// Driver 进程入口：一个 Driver 实现管理 N 个 Connection。
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    fn metadata(&self) -> DriverMetadata;

    /// 打开一个运行时连接实例。`config_json` 是 Endpoint.connection 的 JSON，
    /// 语义完全由 Driver 解释（Core 不懂协议）。
    async fn open_connection(
        &self,
        endpoint_id: &str,
        config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError>;
}

/// 单个 Connection 的生命周期控制。
///
/// 配置流程严格遵循方案 §6.2：
/// `configure` 返回描述符 -> Core 回填 point_id -> `apply_point_map` 下发映射 ->
/// 之后才允许 `run`。`run` 以 Box 消耗 self，退出后该连接不可复用。
#[async_trait::async_trait]
pub trait DriverConnection: Send {
    /// 校验任务合法性（含跨任务 point_key 唯一性），构建采集计划并上报点描述。
    /// 返回 ConfigurationError/DUPLICATE_POINT_KEY 表示拒绝该快照（§6.2 双重保护）。
    async fn configure(
        &mut self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError>;

    async fn apply_point_map(&mut self, map: PointMap) -> Result<(), SdkDriverError>;

    /// 采集主循环。实现方必须响应 shutdown 并保证退出时释放设备资源；
    /// 数据一律经 sink 发布，不得自行写 IPC。
    async fn run(
        self: Box<Self>,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError>;
}

// 别名：AcquisitionTask 仅在 trait 签名中出现，保持与方案 §16 一致的命名可见性
pub use forgelink_core_types::AcquisitionTask;

// ---------------------------------------------------------------------------
// DataSink：带 Latest-Wins 合并的发布端
// ---------------------------------------------------------------------------

enum OutboundMsg {
    /// 控制类消息：不允许丢弃，满载时等待（消费端是专职 writer task，必然前进）。
    Control(pb::Envelope),
    /// 数据批次：允许被合并/丢弃。
    Data(DataBatch),
}

struct CoalescerState {
    pending: Option<DataBatch>,
    coalesced_points: u64,
}

/// Driver 发布数据的句柄。Clone 廉价；内部共享有界通道与合并缓冲。
#[derive(Clone)]
pub struct DataSink {
    tx: mpsc::Sender<OutboundMsg>,
    state: Arc<Mutex<CoalescerState>>,
    /// 本 sink 绑定的 connection_handle；0 表示会话级（不发数据）。
    handle: u32,
    /// 绑定连接的 stream_epoch，publish 时随 handle 一并盖戳（§10）。
    epoch: u64,
}

impl std::fmt::Debug for DataSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataSink").finish()
    }
}

impl DataSink {
    fn new(tx: mpsc::Sender<OutboundMsg>) -> Self {
        Self {
            tx,
            state: Arc::new(Mutex::new(CoalescerState { pending: None, coalesced_points: 0 })),
            handle: 0,
            epoch: 0,
        }
    }

    /// 派生绑定到特定 Connection 的发布端：独立合并缓冲（不同连接不互相合并），
    /// 共享有界通道。Start 时由 SDK 调用；驱动在 run() 中拿到的即为此实例，
    /// 无需关心 handle/epoch 的注入细节。
    pub fn for_connection(&self, handle: u32, stream_epoch: u64) -> Self {
        Self {
            tx: self.tx.clone(),
            state: Arc::new(Mutex::new(CoalescerState { pending: None, coalesced_points: 0 })),
            handle,
            epoch: stream_epoch,
        }
    }

    /// 发布一批数据。队列满时执行 Latest-Wins 合并：
    /// 同 point_id 只保留最新值，批次头取较新一方的 sequence/timestamp。
    ///
    /// NOTE: 合并导致 sequence 缺口——这正是 §10/§12 定义的语义（缺口可观测、
    /// 不代表设备时间顺序），下游不得假设 sequence 连续。
    pub async fn publish(&self, mut batch: DataBatch) {
        // 盖戳：高频帧只携带 handle+epoch+point_id，标识字段由 SDK 统一注入
        if self.handle != 0 {
            batch.connection_handle = self.handle;
            batch.stream_epoch = self.epoch;
        }
        match self.tx.try_send(OutboundMsg::Data(batch.clone())) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                let mut st = self.state.lock().unwrap();
                // 先 take 再合并，避免对 state 的双重可变借用
                let mut coalesced = 0u64;
                let new_pending = match st.pending.take() {
                    Some(mut pending) => {
                        let mut merged =
                            HashMap::with_capacity(pending.values.len() + batch.values.len());
                        for pv in pending.values.drain(..) {
                            merged.insert(pv.point_id, pv);
                        }
                        for pv in batch.values {
                            coalesced += 1;
                            merged.insert(pv.point_id, pv);
                        }
                        pending.sequence = pending.sequence.max(batch.sequence);
                        if batch.timestamp_ns > pending.timestamp_ns {
                            pending.timestamp_ns = batch.timestamp_ns;
                        }
                        pending.values = merged.into_values().collect();
                        // 稳定输出顺序，便于测试与排查
                        pending.values.sort_by_key(|pv| pv.point_id);
                        pending
                    }
                    None => batch,
                };
                st.coalesced_points += coalesced;
                st.pending = Some(new_pending);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Core 连接已断开：静默丢弃即可，进程级退出由 server 循环处理
            }
        }
    }

    /// 尝试把合并缓冲冲回通道；通道仍满则保留缓冲下次再试。
    /// 由 writer 在每次成功出帧后调用，保证积压最终被送出而非滞留。
    fn flush_pending(&self) {
        let mut st = self.state.lock().unwrap();
        if let Some(batch) = st.pending.take() {
            match self.tx.try_send(OutboundMsg::Data(batch)) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(OutboundMsg::Data(b))) => st.pending = Some(b),
                Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
    }

    /// 诊断计数（§17 backpressure_coalesce_total 的进程内视图）。
    pub fn coalesced_points(&self) -> u64 {
        self.state.lock().unwrap().coalesced_points
    }

    async fn send_control(&self, env: pb::Envelope) {
        if self.tx.send(OutboundMsg::Control(env)).await.is_err() {
            tracing::warn!("outbound channel closed, control message dropped");
        }
    }
}

// ---------------------------------------------------------------------------
// 进程辅助：stdin token 注入与父进程 liveness 防护（方案 §14.2 / §14.5）
// ---------------------------------------------------------------------------

/// 从 stdin 读取首行作为 session token（Core spawn 时写入）。
/// 阻塞实现，须在主线程启动早期调用；stdin 关闭而未提供 token 属致命错误，
/// 直接 panic——没有 token 的 Driver 不允许继续运行。
pub fn read_session_token_from_stdin() -> String {
    use std::io::BufRead;
    let mut line = String::new();
    let n = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .expect("read session token from stdin");
    if n == 0 {
        panic!("stdin closed before session token was provided");
    }
    line.trim_end_matches(['\r', '\n']).to_string()
}

/// 孤儿防护第一层（§14.5）：持续读取 stdin 直到 EOF。EOF 即父进程死亡，
/// 立即终止进程——不要求优雅清理，只保证"不再占用设备连接"。
///
/// 必须用阻塞线程而非 tokio stdin：Windows 上异步 stdin 在管道关闭时不保证唤醒，
/// "阻塞线程读到 EOF 后 process::exit" 是双平台唯一可靠的检测路径。
pub fn spawn_parent_liveness_guard() {
    let result = std::thread::Builder::new()
        .name("parent-liveness".into())
        .spawn(|| {
            use std::io::Read;
            let mut stdin = std::io::stdin().lock();
            // 首行 token 已由 read_session_token_from_stdin 消费，这里吞掉剩余字节直到 EOF
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            eprintln!("forgelink driver: parent liveness lost (stdin EOF), exiting");
            std::process::exit(0);
        });
    if result.is_err() {
        // liveness 线程起不来属致命问题：宁可快速失败也不能裸奔成孤儿
        eprintln!("failed to spawn parent liveness guard");
        std::process::exit(1);
    }
    // JoinHandle 有意 drop：线程必须与进程同寿命
}

// ---------------------------------------------------------------------------
// Server：握手认证 + 请求循环
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SdkServerError {
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

struct RunHandle {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
}

struct ConnEntry {
    /// configure 与 run 之间连接对象会被临时 take；None 表示正在运行中。
    conn: Option<Box<dyn DriverConnection>>,
    run: Option<RunHandle>,
    revision: u64,
    epoch: u64,
}

/// 一个已认证会话的全部可变状态。单会话单实例，避免全局静态以支持测试内多实例。
struct Session {
    driver: Box<dyn Driver>,
    sink: DataSink,
    entries: Mutex<HashMap<u32, ConnEntry>>,
    msg_ids: AtomicU64,
}

impl Session {
    fn next_msg_id(&self) -> u64 {
        self.msg_ids.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_driver_error(&self, handle: Option<u32>, err: &SdkDriverError) {
        self.sink
            .send_control(pb::Envelope {
                msg_id: self.next_msg_id(),
                body: Some(pb::envelope::Body::DriverError(pb::DriverErrorReport {
                    connection_handle: handle,
                    detail: Some(error_detail(err.kind, &err.code, err.message.clone())),
                })),
            })
            .await;
    }

    /// 取消运行中的采集并等待其退出；超时则放弃等待（进程退出兜底）。
    async fn stop_run(&self, handle: u32) {
        let rh = self.entries.lock().unwrap().get_mut(&handle).and_then(|e| e.run.take());
        if let Some(rh) = rh {
            rh.cancel.cancel();
            if tokio::time::timeout(RUN_DRAIN_GRACE, rh.join).await.is_err() {
                tracing::warn!(handle, "run task did not drain in time");
            }
        }
    }
}

/// 启动 Driver 服务并阻塞至连接结束或 shutdown 触发。
///
/// 握手方向遵循方案 §14.3：Driver 监听并接受 Core 的唯一一条连接，
/// **先发送携带 session_token 的 Hello**，Core 校验通过后回 Welcome。
/// `session_token` 来自 [`read_session_token_from_stdin`]（生产）或静态值（测试）。
pub async fn serve<D: Driver>(
    driver: D,
    listener: TcpListener,
    session_token: String,
    shutdown: CancellationToken,
) -> Result<(), SdkServerError> {
    let meta = driver.metadata();
    // 每个 Driver Process 默认只接受一个 Core 管理连接（§14.2）：accept 一次
    let (socket, peer) = tokio::select! {
        r = listener.accept() => r.map_err(SdkServerError::Io)?,
        _ = shutdown.cancelled() => return Ok(()),
    };
    tracing::info!(%peer, driver = %meta.driver_id, "core connected");

    let (mut rd, mut wr) = socket.into_split();
    let (tx, rx) = mpsc::channel::<OutboundMsg>(OUTBOUND_CAPACITY);
    let sink = DataSink::new(tx.clone());

    // ---- 握手：先发 Hello 再等 Welcome（§14.3）。握手期 writer 尚未启动，
    // 直接使用写半部，避免与请求循环的出站路径产生交错。----
    let instance_id = format!(
        "{}-{}",
        std::process::id(),
        forgelink_core_types::now_unix_ns()
    );
    let hello = pb::Envelope {
        msg_id: 1,
        body: Some(pb::envelope::Body::Hello(pb::Hello {
            driver_id: meta.driver_id.clone(),
            driver_version: meta.version.clone(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            // NOTE: sdk_version 与 core_version 共用 workspace 版本，M0 足够定位
            sdk_version: format!("rust-sdk v{}", env!("CARGO_PKG_VERSION")),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            instance_id,
            session_token,
        })),
    };
    tokio::select! {
        r = write_envelope(&mut wr, &hello) => r?,
        _ = shutdown.cancelled() => return Ok(()),
    }
    let welcome_env = tokio::select! {
        r = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_envelope(&mut rd)) => {
            r.map_err(|_| SdkServerError::Handshake("welcome timeout".into()))??
        }
        _ = shutdown.cancelled() => return Ok(()),
    };
    match welcome_env.body {
        Some(pb::envelope::Body::Welcome(w)) => {
            // Major 不一致即拒绝运行（协商规则见 §14.3）；Minor 记录备用
            if w.accepted_protocol_major != PROTOCOL_MAJOR {
                return Err(SdkServerError::Handshake(format!(
                    "core rejected protocol major {} != {}",
                    w.accepted_protocol_major, PROTOCOL_MAJOR
                )));
            }
            tracing::info!(accepted_major = w.accepted_protocol_major,
                accepted_minor = w.accepted_protocol_minor,
                core = %w.core_version, "handshake ok");
        }
        // Core 校验 token 失败等场景会直接断开或回错误帧
        Some(pb::envelope::Body::DriverError(err)) => {
            let d = err.detail.unwrap_or_default();
            return Err(SdkServerError::Handshake(format!("rejected: {}/{}: {}", d.kind, d.code, d.message)));
        }
        _ => return Err(SdkServerError::Handshake("expected Welcome".into())),
    }

    let session = Session {
        driver: Box::new(driver),
        sink,
        entries: Mutex::new(HashMap::new()),
        msg_ids: AtomicU64::new(2),
    };

    // ---- writer task：唯一拥有写半部，串行化所有出站帧 ----
    let writer_shutdown = shutdown.child_token();
    let writer = tokio::spawn(writer_loop(wr, rx, session.sink.clone(), writer_shutdown));

    let result = request_loop(&session, rd, &shutdown).await;

    // 会话结束：取消全部采集循环并等待 writer 排空退出
    shutdown.cancel();
    let handles: Vec<RunHandle> = {
        let mut m = session.entries.lock().unwrap();
        m.values_mut().filter_map(|e| e.run.take()).collect()
    };
    for rh in handles {
        rh.cancel.cancel();
        let _ = tokio::time::timeout(RUN_DRAIN_GRACE, rh.join).await;
    }
    drop(session.sink.clone());
    let _ = writer.await;
    result
}

async fn writer_loop(
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::Receiver<OutboundMsg>,
    sink: DataSink,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(OutboundMsg::Control(env)) => {
                    if write_envelope(&mut wr, &env).await.is_err() {
                        break;
                    }
                }
                Some(OutboundMsg::Data(batch)) => {
                    let env = pb::Envelope {
                        msg_id: 0,
                        body: Some(pb::envelope::Body::DataBatch(batch_to_pb(&batch))),
                    };
                    if write_envelope(&mut wr, &env).await.is_err() {
                        break;
                    } else {
                        // 通道腾出空间后优先补发合并积压，保证最新值最终可达
                        sink.flush_pending();
                    }
                }
                None => break,
            },
            _ = shutdown.cancelled() => break,
        }
    }
}

async fn request_loop(
    session: &Session,
    mut rd: OwnedReadHalf,
    shutdown: &CancellationToken,
) -> Result<(), SdkServerError> {
    loop {
        let env = tokio::select! {
            r = read_envelope(&mut rd) => match r {
                Ok(e) => e,
                // 对端断开属正常结束
                Err(ProtocolError::Io(e)) if matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
                {
                    tracing::info!("core disconnected");
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            },
            _ = shutdown.cancelled() => return Ok(()),
        };

        match env.body {
            Some(pb::envelope::Body::Ping(_)) => {
                session
                    .sink
                    .send_control(pb::Envelope {
                        msg_id: env.msg_id,
                        body: Some(pb::envelope::Body::Pong(pb::Pong {})),
                    })
                    .await;
            }
            Some(pb::envelope::Body::GetMetadata(_)) => {
                let m = session.driver.metadata();
                session
                    .sink
                    .send_control(pb::Envelope {
                        msg_id: env.msg_id,
                        body: Some(pb::envelope::Body::MetadataReport(pb::MetadataReport {
                            driver_id: m.driver_id,
                            name: m.name,
                            version: m.version,
                        })),
                    })
                    .await;
            }
            Some(pb::envelope::Body::OpenConnection(req)) => {
                on_open_connection(session, req, env.msg_id).await;
            }
            Some(pb::envelope::Body::ConfigureTasks(req)) => on_configure(session, req, env.msg_id).await,
            Some(pb::envelope::Body::ApplyPointMap(req)) => {
                on_apply_point_map(session, req, env.msg_id).await;
            }
            Some(pb::envelope::Body::StartConnection(req)) => on_start(session, req, env.msg_id).await,
            Some(pb::envelope::Body::StopConnection(req)) => on_stop(session, req, env.msg_id).await,
            Some(pb::envelope::Body::CloseConnection(req)) => on_close(session, req, env.msg_id).await,
            Some(pb::envelope::Body::Shutdown(_)) => {
                tracing::info!("shutdown requested by core");
                return Ok(());
            }
            // Core->Driver 方向不应出现数据面消息；忽略未知消息保持向前兼容
            other => tracing::debug!(?other, "unexpected envelope ignored"),
        }
    }
}

async fn on_open_connection(session: &Session, req: pb::OpenConnection, msg_id: u64) {
    let exists = session
        .entries
        .lock()
        .unwrap()
        .contains_key(&req.connection_handle);
    let result = if exists {
        err_result(
            ErrorKind::Internal,
            "HANDLE_EXISTS",
            format!("handle {} already open", req.connection_handle),
        )
    } else {
        match session.driver.open_connection(&req.endpoint_id, &req.config_json).await {
            Ok(conn) => {
                session.entries.lock().unwrap().insert(
                    req.connection_handle,
                    ConnEntry { conn: Some(conn), run: None, revision: 0, epoch: 0 },
                );
                ok_result()
            }
            Err(err) => err_result(err.kind, &err.code, err.message),
        }
    };
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::OpenConnectionAck(pb::OpenConnectionAck {
                connection_handle: req.connection_handle,
                result: Some(result),
            })),
        })
        .await;
}

async fn on_configure(session: &Session, req: pb::ConfigureTasks, msg_id: u64) {
    // 先取出连接对象供 configure 使用；失败必须归还，避免泄漏成"打开但不可用"
    let taken = session
        .entries
        .lock()
        .unwrap()
        .get_mut(&req.connection_handle)
        .and_then(|e| e.conn.take());

    let Some(mut conn) = taken else {
        session
            .send_driver_error(
                Some(req.connection_handle),
                &SdkDriverError::new(ErrorKind::Internal, "NO_CONNECTION", "connection not open"),
            )
            .await;
        return;
    };

    match tasks_from_pb(req.tasks) {
        Ok(tasks) => match conn.configure(req.revision, tasks).await {
            Ok(descriptors) => {
                session.entries.lock().unwrap().get_mut(&req.connection_handle).map(|e| {
                    e.conn = Some(conn);
                    e.revision = req.revision;
                });
                session
                    .sink
                    .send_control(pb::Envelope {
                        msg_id,
                        body: Some(pb::envelope::Body::PointDescriptors(
                            pb::PointDescriptorsReport {
                                connection_handle: req.connection_handle,
                                revision: req.revision,
                                descriptors: descriptors
                                    .into_iter()
                                    .map(|d| pb::PointDescriptorProto {
                                        point_key: d.point_key,
                                        data_type: d.data_type.as_str().to_string(),
                                        unit: d.unit,
                                    })
                                    .collect(),
                            },
                        )),
                    })
                    .await;
            }
            Err(err) => {
                session.entries.lock().unwrap().get_mut(&req.connection_handle).map(|e| {
                    e.conn = Some(conn);
                });
                session.send_driver_error(Some(req.connection_handle), &err).await;
            }
        },
        Err(e) => {
            let err: SdkDriverError = e.into();
            session.entries.lock().unwrap().get_mut(&req.connection_handle).map(|e| {
                e.conn = Some(conn);
            });
            session.send_driver_error(Some(req.connection_handle), &err).await;
        }
    }
}

async fn on_apply_point_map(session: &Session, req: pb::ApplyPointMap, msg_id: u64) {
    let taken = session
        .entries
        .lock()
        .unwrap()
        .get_mut(&req.connection_handle)
        .and_then(|e| e.conn.take());

    let Some(mut conn) = taken else {
        session
            .send_driver_error(
                Some(req.connection_handle),
                &SdkDriverError::new(ErrorKind::Internal, "NO_CONNECTION", "connection not open"),
            )
            .await;
        return;
    };

    let res = conn.apply_point_map(req.key_to_point_id).await;
    session.entries.lock().unwrap().get_mut(&req.connection_handle).map(|e| {
        e.conn = Some(conn);
    });
    let result = match res {
        Ok(()) => ok_result(),
        Err(err) => err_result(err.kind, &err.code, err.message),
    };
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::ConfigApplied(pb::ConfigApplied {
                connection_handle: req.connection_handle,
                revision: req.revision,
                result: Some(result),
            })),
        })
        .await;
}

async fn on_start(session: &Session, req: pb::StartConnection, msg_id: u64) {
    // 前置校验：必须已完成 configure 且未在运行
    let ready = {
        let mut m = session.entries.lock().unwrap();
        match m.get_mut(&req.connection_handle) {
            Some(entry) => {
                if entry.run.is_some() {
                    None
                } else {
                    entry.conn.take()
                }
            }
            None => None,
        }
    };

    let Some(conn) = ready else {
        session
            .send_control_ack_start(req.connection_handle, false, "missing connection or already running")
            .await;
        return;
    };

    let run_cancel = CancellationToken::new();
    let task_cancel = run_cancel.clone();
    // 每个连接独立 sink：合并缓冲隔离 + 自动盖 handle/epoch 戳
    let sink_for_run = session.sink.for_connection(req.connection_handle, req.stream_epoch);
    let handle = req.connection_handle;

    // 采集任务独立于请求循环运行；结束（含出错）时上报终态。
    let join = tokio::spawn(async move {
        let outcome = conn.run(sink_for_run.clone(), task_cancel).await;
        let (final_state, detail) = match &outcome {
            Ok(()) => (ConnectionState::Stopped, String::new()),
            Err(err) => {
                tracing::warn!(handle, err = %err, "connection run failed");
                (ConnectionState::Failed, err.to_string())
            }
        };
        send_state_direct(&sink_for_run, handle, final_state, &detail).await;
    });

    {
        let mut m = session.entries.lock().unwrap();
        if let Some(entry) = m.get_mut(&req.connection_handle) {
            entry.epoch = req.stream_epoch;
            entry.run = Some(RunHandle { cancel: run_cancel, join });
        }
    }

    // Start 的 Ack 复用请求 msg_id，便于 Core 侧关联
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::StartConnectionAck(pb::StartConnectionAck {
                connection_handle: req.connection_handle,
                result: Some(ok_result()),
            })),
        })
        .await;
}

async fn on_stop(session: &Session, req: pb::StopConnection, msg_id: u64) {
    session.stop_run(req.connection_handle).await;
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::StopConnectionAck(pb::StopConnectionAck {
                connection_handle: req.connection_handle,
                result: Some(ok_result()),
            })),
        })
        .await;
}

async fn on_close(session: &Session, req: pb::CloseConnection, msg_id: u64) {
    session.stop_run(req.connection_handle).await;
    session.entries.lock().unwrap().remove(&req.connection_handle);
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::CloseConnectionAck(pb::CloseConnectionAck {
                connection_handle: req.connection_handle,
                result: Some(ok_result()),
            })),
        })
        .await;
}

impl Session {
    async fn send_control_ack_start(&self, handle: u32, ok: bool, why: &str) {
        let result = if ok {
            ok_result()
        } else {
            err_result(ErrorKind::Internal, "NOT_CONFIGURED", why)
        };
        self.sink
            .send_control(pb::Envelope {
                msg_id: self.next_msg_id(),
                body: Some(pb::envelope::Body::StartConnectionAck(pb::StartConnectionAck {
                    connection_handle: handle,
                    result: Some(result),
                })),
            })
            .await;
    }
}

/// 独立于 Session 的状态发送（供已 spawn 的 run 任务使用）。
async fn send_state_direct(sink: &DataSink, handle: u32, state: ConnectionState, detail: &str) {
    sink.send_control(pb::Envelope {
        msg_id: 0,
        body: Some(pb::envelope::Body::ConnectionStateChanged(pb::ConnectionStateChanged {
            connection_handle: handle,
            state: state.as_str().to_string(),
            detail: detail.to_string(),
        })),
    })
    .await;
}
