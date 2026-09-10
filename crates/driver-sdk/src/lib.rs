//! Mesa Driver SDK（方案 §16）。
//!
//! SDK 承担 Driver 进程的全部通用职责：IPC、session token 认证、心跳应答、
//! DataBatch 序列化与背压合并、Shutdown、父进程 liveness 防护。协议开发者只需
//! 实现 [`Driver`] / [`DriverConnection`] 两个 trait，不接触任何 IPC 细节。
//!
//! 背压模型（方案 §12）：SDK 内部出站队列有界；数据批次在队列满时执行
//! Latest-Wins Coalescing（同 point_id 仅保留最新值）；控制类消息走阻塞式入队，
//! 永不丢弃。事件批次（Event Plane V1）走第三条独立可靠队列：禁止合并、
//! 禁止 Latest-Wins、满时显式报错永不静默丢弃。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mesa_core_types::{
    ConnectionState, DriverMetadata, ErrorKind, EventRecord, PointDescriptor, PointMap,
};
use mesa_driver_protocol::{
    ConvertError, PROTOCOL_MAJOR, PROTOCOL_MINOR, ProtocolError, batch_to_pb, err_result,
    error_detail, event_batch_to_pb, event_task_from_pb, ok_result, pb, read_envelope,
    tasks_from_pb, write_envelope,
};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 出站队列容量。当前取保守值 256；性能预算阶段（§22）按 50K updates/s 压测结果调整。
#[allow(dead_code)]
const OUTBOUND_CAPACITY: usize = 256;
const CONTROL_CAPACITY: usize = 32;
const DATA_CAPACITY: usize = 256;
/// 事件队列容量 128（Event Plane V1 §12）：事件是低频 occurrence，128 足够吸收
/// 瞬时抖动；持续满队列时 publish **等待**（背压），而不是扩容、合并或丢弃——
/// 事件 occurrence 丢一条就是流完整性 broken，连接还 RUNNING 属于"假装流完整"。
const EVENT_CAPACITY: usize = 128;

/// 握手阶段读超时。超时视为对端异常，直接断开。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Stop 时等待采集循环收尾的宽限。当前采集循环仅做 tick 级清理，50ms 足够；
/// 真实协议驱动若需关闭 socket/session，应在其 run() 内响应取消并自行限时。
const RUN_DRAIN_GRACE: Duration = Duration::from_millis(500);

/// writer drain 上限（P0-2）：writer_shutdown 触发时队列里至多
/// 32+256+128 帧，loopback 下毫秒级写完；5s 是顽固生产者（cancel 后仍
/// publish 的驱动 bug）场景的兜底，超时即大声记录后退出（Core 侧
/// terminate 强杀是最后一道门）。正常路径永不触及该上限。
const WRITER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

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
        Self {
            kind,
            code: code.into(),
            message: message.into(),
        }
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

    /// 驱动自描述（V2.1 §13）：连接参数、资源目录、控制目录等。
    /// 默认实现基于 metadata 生成最小可用描述符，避免简单 Driver 被迫实现复杂契约。
    fn descriptor(&self) -> mesa_core_types::DriverDescriptor {
        let m = self.metadata();
        mesa_core_types::DriverDescriptor {
            contract_major: mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR,
            contract_minor: mesa_core_types::DESCRIPTOR_CONTRACT_MINOR,
            identity: mesa_core_types::DriverIdentity {
                driver_id: m.driver_id,
                name: m.name,
                version: m.version,
            },
            connection: mesa_core_types::SchemaDescriptor::default(),
            resources: vec![],
            controls: mesa_core_types::ControlCatalog::default(),
            resource_selection_methods: vec![mesa_core_types::ResourceSelectionMethod::Manual],
            capabilities: mesa_core_types::DriverCapabilities::default(),
            // 老 Driver 无事件目录即 empty（Event §5 向后兼容，Major 不升级）
            events: Default::default(),
        }
    }

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
/// 之后才允许 `run`。
///
/// 生命周期语义（§21 Start/Stop 与 Runtime Reconfigure 行）：连接对象在
/// run 结束后归还给会话，**同一连接可反复 Stop → Configure → Start**；
/// 驱动应在 run 内部响应 shutdown 并保证设备资源不随单次运行泄漏
/// （跨运行持有的资源由进程退出统一回收）。
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

    /// 采集主循环。实现方必须响应 shutdown 并保证退出时释放本次运行占用的资源；
    /// 数据一律经 sink 发布，不得自行写 IPC。
    async fn run(
        &mut self,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError>;

    /// 动态探测（§8）：复用本连接已建立的协议会话做纯查询，返回设备事实。
    /// 禁止在此走 Configure/ApplyPointMap/Start；Secret/PKI/会话建立全部
    /// 复用 OpenConnection 已完成的成果，不得另建第二套连接逻辑。
    /// 默认返回 Unsupported。
    async fn probe(&mut self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        Err(SdkDriverError::new(
            mesa_core_types::ErrorKind::Unsupported,
            "PROBE_UNSUPPORTED",
            "dynamic probe not supported",
        ))
    }

    /// 浏览（§20）：仅 OPC UA 等支持，默认返回 Unsupported。
    async fn browse(
        &mut self,
        _parent: &str,
        _filter: &str,
        _cursor: &str,
        _limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), SdkDriverError> {
        Err(SdkDriverError::new(
            mesa_core_types::ErrorKind::Unsupported,
            "UNSUPPORTED",
            "browse not supported",
        ))
    }

    /// 事件任务配置（Event Plane V1 §6 / PR6）：默认空任务直接接受（老 Driver
    /// 零改动即可通过 Minor Gate 的"无 EventTask"路径）；非空任务返回
    /// Unsupported（`EVENT_NOT_SUPPORTED`）。支持事件的 Driver 覆盖本方法，
    /// 校验 binding 语义（Core 永不解析 binding）并记住订阅计划供 run() 使用。
    async fn configure_events(
        &mut self,
        revision: u64,
        tasks: Vec<EventTask>,
    ) -> Result<(), SdkDriverError> {
        let _ = revision;
        if tasks.is_empty() {
            Ok(())
        } else {
            Err(SdkDriverError::new(
                ErrorKind::Unsupported,
                "EVENT_NOT_SUPPORTED",
                "event tasks not supported by this driver",
            ))
        }
    }

    /// 控制面写入（J §11）：默认 Unsupported，OPC UA 先行实现。
    async fn write(
        &mut self,
        _target: &str,
        _value: mesa_core_types::Value,
        _expected: Option<mesa_core_types::Value>,
    ) -> Result<(), SdkDriverError> {
        Err(SdkDriverError::new(
            mesa_core_types::ErrorKind::Unsupported,
            "UNSUPPORTED",
            "write not supported",
        ))
    }

    /// 控制面命令（J §11）：默认 Unsupported，预留统一入口。
    async fn command(
        &mut self,
        _command: &str,
        _args_json: &str,
    ) -> Result<serde_json::Value, SdkDriverError> {
        Err(SdkDriverError::new(
            mesa_core_types::ErrorKind::Unsupported,
            "UNSUPPORTED",
            "command not supported",
        ))
    }
}

// 别名：AcquisitionTask/EventTask 在 trait 签名中出现，保持与方案 §16 一致的命名可见性；
// EventBatch/DataBatch 供驱动运行时测试构造通道。
pub use mesa_core_types::{AcquisitionTask, DataBatch, EventBatch, EventTask};

// ---------------------------------------------------------------------------
// DataSink：带 Latest-Wins 合并的发布端
// ---------------------------------------------------------------------------

/// 出站控制/数据分流：Control 32 容量可靠队列（永不合并/永不 Latest-Wins），
/// Data 256 容量 Latest-Wins 合并队列（§12）。writer 侧 biased select! Control 优先。
struct CoalescerState {
    pending: Option<DataBatch>,
    coalesced_points: u64,
}

/// 事件发布失败（Event Plane V1 §12 fail-closed）：入队前校验失败（未绑定/
/// 空批/坏记录/超大）当场拒绝；入队本身永不丢弃——队列满时 **等待**
/// （背压），只在通道关闭（会话 teardown）时返回 [`EventPublishError::Closed`]。
/// 注意没有 `QueueFull` 变体：满队列就拒绝并让驱动 warn 后继续，会造成
/// "seq 100 → seq 102 但连接仍 RUNNING"的残缺流，违反 Event Plane 完整性。
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EventPublishError {
    #[error("event sink not bound to a connection")]
    Unbound,
    #[error("event batch is empty")]
    Empty,
    #[error("EVENT_RECORD_INVALID: {0}")]
    InvalidRecord(String),
    #[error("event batch too large: {0} bytes > 256 KiB")]
    TooLarge(usize),
    #[error("event channel closed")]
    Closed,
}

/// Driver 发布事件的句柄（由 [`DataSink::events`] 派生，与派生源共享
/// handle/epoch 绑定）。Clone 廉价。
///
/// 与 DataSink 的根本区别：publish 永不合并、永不 Latest-Wins；队列满时
/// **等待**而不是覆盖旧数据或报错丢弃。batch header
/// （connection_handle/stream_epoch/sequence/timestamp_ns/mono_ns）全部由
/// SDK 自动填充，驱动只管交 `Vec<EventRecord>`。
/// 单连接事件序号器（P0-2）：sequence 分配 → enqueue 必须原子化。
/// 同一 connection 的多个 EventTask 会并发 publish；若只锁"分配序号"、
/// 解锁后再 send，wire 顺序就会与序号错开（11 先于 10 到达），违反§11。
/// 因此同一 bound sink 派生的所有 EventSink 共享这一个 async 锁，
/// publish 全程持有直到 send().await 成功——wire 顺序 == sequence 顺序。
/// 每个 epoch 全新实例、从 1 开始；无全局表、无需 Stop 清理、天然有界。
#[derive(Debug)]
struct EventSequencer {
    next: u64,
}

#[derive(Clone, Debug)]
pub struct EventSink {
    event_tx: mpsc::Sender<EventBatch>,
    seq: Arc<tokio::sync::Mutex<EventSequencer>>,
    handle: u32,
    epoch: u64,
}

impl EventSink {
    /// 集成测试专用构造：直连给定 event 通道（handle/epoch 由调用方指定）。
    /// 生产路径必须经 [`DataSink::events`] 派生；本函数仅供驱动运行时测试
    /// （Fake/Native fixture），不得在生产代码中使用。
    pub fn for_test(event_tx: mpsc::Sender<EventBatch>, handle: u32, stream_epoch: u64) -> Self {
        Self {
            event_tx,
            seq: Arc::new(tokio::sync::Mutex::new(EventSequencer { next: 1 })),
            handle,
            epoch: stream_epoch,
        }
    }

    /// 本次 run 的 stream_epoch 只读视图（PR7：synthetic occurrence 的 run
    /// 作用域——Simulator 用它构造跨进程重启唯一的 event_id；Core epoch
    /// 是稳定的 run scope，比进程级计数更适合做 ID 作用域）。
    pub fn stream_epoch(&self) -> u64 {
        self.epoch
    }

    /// 发布一批事件。sequence 由 SDK 按 (handle, epoch) 自动分配并递增；
    /// epoch 切换（Stop → Start）后自动从 1 重新开始，驱动无需感知。
    ///
    /// 顺序语义（P0-2）：全程持有 per-connection async 锁——序号分配与
    /// send().await 在同一临界区内完成。多 publisher 并发时 wire 到达顺序
    /// 与序号顺序严格一致，不可能出现"11 先于 10"。
    ///
    /// 背压语义（P0）：队列满时 `send().await` 等待，不合并、不覆盖、不拒绝——
    /// occurrence 要么完整入队，要么会话已死（Closed）。等待期间不监听 shutdown：
    /// writer 持续排空（Core 活着）则自然疏通；会话 teardown 时发送端随 sink
    /// 一起释放，等待以 Closed 结束。驱动主循环在两次 publish 之间照常响应
    /// shutdown 即可，不会卡死。
    pub async fn publish(&self, events: Vec<EventRecord>) -> Result<u64, EventPublishError> {
        if self.handle == 0 {
            return Err(EventPublishError::Unbound);
        }
        if events.is_empty() {
            return Err(EventPublishError::Empty);
        }
        // 驱动侧前置校验：坏记录在 publish 当场拒绝，不占用队列、不污染 wire。
        // （锁外执行：纯计算，不触及序号状态）
        for e in &events {
            e.validate()
                .map_err(|e| EventPublishError::InvalidRecord(e.to_string()))?;
        }
        // 序号分配 → wire 入队原子化：锁住直到 send 成功才递增。
        let mut seq = self.seq.lock().await;
        let sequence = seq.next;
        let batch = EventBatch {
            connection_handle: self.handle,
            stream_epoch: self.epoch,
            sequence,
            timestamp_ns: mesa_core_types::now_unix_ns(),
            events,
            mono_ns: Some(mesa_core_types::host_mono_ns()),
        };
        // 粗粒度上限预检（JSON 体积与 proto 同量级；精确 256 KiB 由 writer 侧
        // event_batch_to_pb 最终强制执行）。超大批次在 publish 当场拒绝，
        // 且不消耗序号（序号只在成功入队后递增）。
        let approx = serde_json::to_string(&batch)
            .map(|s| s.len())
            .unwrap_or(usize::MAX);
        if approx > mesa_core_types::EVENT_BATCH_MAX_BYTES {
            return Err(EventPublishError::TooLarge(approx));
        }
        // 有界可靠入队：满则等待（背压），只在会话 teardown 时失败。
        // 同步 callback 不能 await 的驱动（未来 OPC UA/SINUMERIK）不得直调此处，
        // 应自建 bounded raw-ingress + async forwarder，overflow 时按订阅失败处理。
        match self.event_tx.send(batch).await {
            Ok(()) => {
                // u64 耗尽属不可达（每秒百万批也需数十万年）；saturating 兜底，
                // 语义是"序号空间实现上限"，不是静默回绕。
                seq.next = sequence.saturating_add(1);
                Ok(sequence)
            }
            Err(_) => Err(EventPublishError::Closed),
        }
    }
}

/// Driver 发布数据的句柄。Clone 廉价；内部共享有界通道与合并缓冲。
#[derive(Clone)]
pub struct DataSink {
    control_tx: mpsc::Sender<pb::Envelope>,
    data_tx: mpsc::Sender<DataBatch>,
    event_tx: mpsc::Sender<EventBatch>,
    state: Arc<Mutex<CoalescerState>>,
    /// 全局 pending 注册表：handle -> CoalescerState，用于 writer 统一 flush
    pending_registry: Arc<Mutex<HashMap<u32, Arc<Mutex<CoalescerState>>>>>,
    /// 事件序号器：同一 bound sink 派生的所有 EventSink 共享（见 [`EventSequencer`]）。
    event_seq: Arc<tokio::sync::Mutex<EventSequencer>>,
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
    /// 集成测试专用构造：直连给定三通道（未绑定 handle/epoch，行为同会话占位）。
    /// 生产路径由 `serve()` 构造；本函数仅供驱动运行时测试，不得在生产代码中使用。
    pub fn for_test(
        control_tx: mpsc::Sender<pb::Envelope>,
        data_tx: mpsc::Sender<DataBatch>,
        event_tx: mpsc::Sender<EventBatch>,
    ) -> Self {
        Self::new(control_tx, data_tx, event_tx)
    }

    fn new(
        control_tx: mpsc::Sender<pb::Envelope>,
        data_tx: mpsc::Sender<DataBatch>,
        event_tx: mpsc::Sender<EventBatch>,
    ) -> Self {
        Self {
            control_tx,
            data_tx,
            event_tx,
            state: Arc::new(Mutex::new(CoalescerState {
                pending: None,
                coalesced_points: 0,
            })),
            pending_registry: Arc::new(Mutex::new(HashMap::new())),
            // 会话级占位（handle 0 永不发事件）；for_connection 会换成新实例
            event_seq: Arc::new(tokio::sync::Mutex::new(EventSequencer { next: 1 })),
            handle: 0,
            epoch: 0,
        }
    }

    /// 派生绑定到特定 Connection 的发布端：独立合并缓冲（不同连接不互相合并），
    /// 共享有界通道。Start 时由 SDK 调用；驱动在 run() 中拿到的即为此实例，
    /// 无需关心 handle/epoch 的注入细节。
    pub fn for_connection(&self, handle: u32, stream_epoch: u64) -> Self {
        let entry = {
            let mut reg = self.pending_registry.lock().unwrap();
            reg.entry(handle)
                .or_insert_with(|| {
                    Arc::new(Mutex::new(CoalescerState {
                        pending: None,
                        coalesced_points: 0,
                    }))
                })
                .clone()
        };
        Self {
            control_tx: self.control_tx.clone(),
            data_tx: self.data_tx.clone(),
            event_tx: self.event_tx.clone(),
            state: entry,
            pending_registry: Arc::clone(&self.pending_registry),
            // 新 epoch 即新序号器、从 1 开始（§11 新流语义）；旧实例随旧
            // sink 一起释放，无全局表、无需 Stop 清理、天然有界
            event_seq: Arc::new(tokio::sync::Mutex::new(EventSequencer { next: 1 })),
            handle,
            epoch: stream_epoch,
        }
    }

    /// 派生事件发布端：与本 sink 共享 handle/epoch 绑定与 sequence 注册表。
    /// 支持事件的驱动在 run() 中 `let events = sink.events();` 后发布；
    /// 不支持事件的驱动忽略本方法即可（零成本）。
    pub fn events(&self) -> EventSink {
        EventSink {
            event_tx: self.event_tx.clone(),
            seq: Arc::clone(&self.event_seq),
            handle: self.handle,
            epoch: self.epoch,
        }
    }

    /// 发布一批数据。队列满时执行 Latest-Wins 合并：
    /// 同 point_id 只保留最新值，批次头取较新一方的 sequence/timestamp。
    ///
    /// NOTE: 合并导致 sequence 缺口——这正是 §10/§12 定义的语义（缺口可观测、
    /// 不代表设备时间顺序），下游不得假设 sequence 连续。
    pub async fn publish(&self, mut batch: DataBatch) {
        if self.handle != 0 {
            batch.connection_handle = self.handle;
            batch.stream_epoch = self.epoch;
        }
        if batch.mono_ns.is_none() {
            batch.mono_ns = Some(mesa_core_types::host_mono_ns());
        }
        match self.data_tx.try_send(batch) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(batch)) => {
                let mut st = self.state.lock().unwrap();
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
                        pending.mono_ns = match (pending.mono_ns, batch.mono_ns) {
                            (Some(a), Some(b)) => Some(a.max(b)),
                            (Some(a), None) => Some(a),
                            (None, Some(b)) => Some(b),
                            (None, None) => None,
                        };
                        pending.values = merged.into_values().collect();
                        pending.values.sort_by_key(|pv| pv.point_id);
                        pending
                    }
                    None => batch,
                };
                st.coalesced_points += coalesced;
                st.pending = Some(new_pending);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    /// 尝试把合并缓冲冲回通道；通道仍满则保留缓冲下次再试。
    /// 修复：遍历全局注册表，逐连接 flush，确保 per-connection pending 不会永久滞留。
    fn flush_pending(&self) {
        // 先尝试 flush 全局注册表中的所有 pending（保证 Latest-Wins 最终可达）
        let handles: Vec<u32> = {
            let reg = self.pending_registry.lock().unwrap();
            reg.keys().copied().collect()
        };
        for h in handles {
            let entry_opt = {
                let reg = self.pending_registry.lock().unwrap();
                reg.get(&h).cloned()
            };
            if let Some(entry) = entry_opt {
                let mut st = entry.lock().unwrap();
                if let Some(batch) = st.pending.take() {
                    match self.data_tx.try_send(batch) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(b)) => {
                            st.pending = Some(b);
                            // 通道仍满，后续 handle 也无法发送，提前跳出
                            break;
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {}
                    }
                }
            }
        }
        // 兼容会话级 pending（handle 0 的独立 state）
        let mut st = self.state.lock().unwrap();
        if let Some(batch) = st.pending.take() {
            match self.data_tx.try_send(batch) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(b)) => st.pending = Some(b),
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
    }

    /// 诊断计数（§17 backpressure_coalesce_total 的进程内视图）。
    pub fn coalesced_points(&self) -> u64 {
        self.state.lock().unwrap().coalesced_points
    }

    /// 控制面发送：可靠队列，不合并、满时等待（消费端必然前进），禁止 Latest-Wins。
    async fn send_control(&self, env: pb::Envelope) {
        if self.control_tx.send(env).await.is_err() {
            tracing::warn!("control channel closed, message dropped");
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
            eprintln!("Mesa driver: parent liveness lost (stdin EOF), exiting");
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
// 故障注入钩子（方案附录 A.3，仅供测试路径使用）
// ---------------------------------------------------------------------------

/// 驱动进程故障注入开关。生产入口 `serve()` 恒为禁用；
/// 合同测试通过 [`serve_with_faults`] 注入 hang 等故障模拟驱动异常。
#[derive(Clone, Default)]
pub struct SdkFaults {
    hang: Arc<std::sync::atomic::AtomicBool>,
}

impl SdkFaults {
    pub fn new() -> Self {
        Self::default()
    }

    /// 置位后请求循环停止处理任何入站帧：不回 Pong、不响应控制消息——
    /// 模拟驱动主循环死锁。数据面 writer 不受影响，批次仍会继续外发。
    pub fn set_hang(&self, on: bool) {
        self.hang.store(on, Ordering::Relaxed);
    }

    fn hanging(&self) -> bool {
        self.hang.load(Ordering::Relaxed)
    }
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

/// 等待 run task 退出；超时则 abort 并回收（P0-3 Final）。
/// 禁止 `timeout(join)` 裸用——超时会 drop JoinHandle 使 task detach：后台
/// producer 仍可 `publish` 成功，而 writer 已按"无新生产"退出，造成
/// "publish Ok 的 batch 没写 TCP"。abort 完成后再 cancel writer，
/// writer_shutdown 的"全部生产者已停"前提才是硬保证（与 writer JoinHandle
/// 同一修法，见 serve 收尾 NOTE）。
async fn stop_run_handle(rh: RunHandle, ctx: &'static str) {
    rh.cancel.cancel();
    let mut join = rh.join;
    tokio::select! {
        _ = &mut join => {}
        _ = tokio::time::sleep(RUN_DRAIN_GRACE) => {
            tracing::error!("run task did not stop in time, aborting ({ctx})");
            join.abort();
            let _ = join.await;
        }
    }
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
    /// Arc 化以便 run 任务结束时把连接对象归还回表（Stop→Start 可重复）。
    entries: Arc<Mutex<HashMap<u32, ConnEntry>>>,
    /// 当前允许发送数据的 stream_epoch（handle -> active epoch）。
    /// Start 时插入，Stop/Close 时移除；writer 在真正写 socket 前检查，丢弃已停止/过期 epoch 的旧批次。
    /// 该门控使成功的 StopConnectionAck 成为数据面 barrier：Ack 后 Core 不得再观察到该 epoch 的 DataBatch。
    active_epochs: Arc<Mutex<HashMap<u32, u64>>>,
    // TODO: PlanSnapshot 冻结字段，msg_ids 为 IPC 信封递增 ID，V1 Control 面预留，当前 Data 面未单独递增但需保留以备全量序列追踪
    #[allow(dead_code)]
    msg_ids: AtomicU64,
}

impl Session {
    // TODO: PlanSnapshot 冻结字段，msg_ids 递增器，V1 暂由 sink 内部序列保证，保留以备 Control 面独立序列
    #[allow(dead_code)]
    fn next_msg_id(&self) -> u64 {
        self.msg_ids.fetch_add(1, Ordering::Relaxed)
    }

    // TODO: 冻结字段，结构化 DriverError 上报路径，V1 已走 DataSink 控制通道，保留以备独立错误帧演进
    #[allow(dead_code)]
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

    /// 取消运行中的采集并等待其退出；超时则 abort 回收（P0-3 Final，
    /// 见 `stop_run_handle`——detach 的 producer 会破坏 writer drain 前提）。
    async fn stop_run(&self, handle: u32) {
        let rh = self
            .entries
            .lock()
            .unwrap()
            .get_mut(&handle)
            .and_then(|e| e.run.take());
        if let Some(rh) = rh {
            stop_run_handle(rh, "stop_run").await;
        }
    }
}

/// 启动 Driver 服务并阻塞至连接结束或 shutdown 触发（无故障注入）。
pub async fn serve<D: Driver>(
    driver: D,
    listener: TcpListener,
    session_token: String,
    shutdown: CancellationToken,
) -> Result<(), SdkServerError> {
    serve_with_faults(driver, listener, session_token, shutdown, None).await
}

/// [`serve`] 的故障注入变体：`faults` 为 None 时行为与 [`serve`] 完全一致。
///
/// 握手方向遵循方案 §14.3：Driver 监听并接受 Core 的唯一一条连接，
/// **先发送携带 session_token 的 Hello**，Core 校验通过后回 Welcome。
/// `session_token` 来自 [`read_session_token_from_stdin`]（生产）或静态值（测试）。
pub async fn serve_with_faults<D: Driver>(
    driver: D,
    listener: TcpListener,
    session_token: String,
    shutdown: CancellationToken,
    faults: Option<SdkFaults>,
) -> Result<(), SdkServerError> {
    let meta = driver.metadata();
    // 每个 Driver Process 默认只接受一个 Core 管理连接（§14.2）：accept 一次
    let (socket, peer) = tokio::select! {
        r = listener.accept() => r.map_err(SdkServerError::Io)?,
        _ = shutdown.cancelled() => return Ok(()),
    };
    tracing::info!(%peer, driver = %meta.driver_id, "core connected");

    let (mut rd, mut wr) = socket.into_split();
    let (control_tx, control_rx) = mpsc::channel::<pb::Envelope>(CONTROL_CAPACITY);
    let (data_tx, data_rx) = mpsc::channel::<DataBatch>(DATA_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel::<EventBatch>(EVENT_CAPACITY);
    let sink = DataSink::new(control_tx.clone(), data_tx.clone(), event_tx.clone());

    // ---- 握手：先发 Hello 再等 Welcome（§14.3）。握手期 writer 尚未启动，
    // 直接使用写半部，避免与请求循环的出站路径产生交错。----
    let instance_id = format!("{}-{}", std::process::id(), mesa_core_types::now_unix_ns());
    let hello = pb::Envelope {
        msg_id: 1,
        body: Some(pb::envelope::Body::Hello(pb::Hello {
            driver_id: meta.driver_id.clone(),
            driver_version: meta.version.clone(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            // NOTE: sdk_version 与 core_version 共用 workspace 版本，当前足以定位；TODO: 后续区分 SDK/Core 版本追踪
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
            return Err(SdkServerError::Handshake(format!(
                "rejected: {}/{}: {}",
                d.kind, d.code, d.message
            )));
        }
        _ => return Err(SdkServerError::Handshake("expected Welcome".into())),
    }

    let session = Session {
        driver: Box::new(driver),
        sink,
        entries: Arc::new(Mutex::new(HashMap::new())),
        active_epochs: Arc::new(Mutex::new(HashMap::new())),
        msg_ids: AtomicU64::new(2),
    };

    // ---- writer task：唯一拥有写半部，串行化所有出站帧 ----
    // 调度（Event Plane V1 §12）：Control 永远最高优先；Event/Data 公平交替——
    // 禁止 `Control > Event > Data` 固定偏序，否则 Alarm 风暴会让 Data 永久饥饿。
    // P0-2：writer_shutdown 是独立 token（不是全局 shutdown 的 child）——只在
    // 全部生产者停止后触发，触发后 writer 进入 drain 模式（排空已有帧再退），
    // 而不是直接 break 丢队列尾。
    let writer_shutdown = CancellationToken::new();
    let writer = tokio::spawn(writer_loop(
        wr,
        control_rx,
        data_rx,
        event_rx,
        session.sink.clone(),
        Arc::clone(&session.active_epochs),
        writer_shutdown.clone(),
    ));

    let result = request_loop(&session, rd, &shutdown, faults.as_ref()).await;

    // P0-2 两阶段 teardown（顺序即正确性）+ P0-3 Final：
    // ① request_loop 已结束 → cancel 全局 shutdown，run tasks 观察到后停止生产；
    // ② 逐个 stop_run_handle（等退出 / 超时 abort 回收）→ 此后无新生产是硬
    //    保证（abort 完成才继续；detach 残留会 publish 成功而 writer 已退）；
    // ③ 触发 writer_shutdown → writer 把队列里已有帧写完再退（drain 模式），
    //    然后 TCP FIN。active_epochs 全程不变，排队 Event 不丢。
    shutdown.cancel();
    let handles: Vec<RunHandle> = {
        let mut m = session.entries.lock().unwrap();
        m.values_mut().filter_map(|e| e.run.take()).collect()
    };
    for rh in handles {
        stop_run_handle(rh, "serve-teardown").await;
    }
    writer_shutdown.cancel();
    // NOTE: 不能用 timeout(writer)——超时会 drop JoinHandle 使任务 detach
    // 后台写半残 socket。借用 join，超时后显式 abort 再回收。
    let mut writer = writer;
    tokio::select! {
        _ = &mut writer => {}
        _ = tokio::time::sleep(WRITER_DRAIN_TIMEOUT) => {
            tracing::error!("event/data writer did not drain in time, aborting");
            writer.abort();
            let _ = writer.await;
        }
    }
    result
}

/// Event/Data 公平取帧：按 `event_turn` 交替先手，保证双路饱和时严格轮流
/// （C E D / C D E 交替，Control 由外层 biased 恒优先）；仅一侧有数据时
/// biased select 仍会取到该侧，不引入额外延迟。
enum FairMsg {
    Event(Option<EventBatch>),
    Data(Option<DataBatch>),
}

async fn recv_fair(
    event_rx: &mut mpsc::Receiver<EventBatch>,
    data_rx: &mut mpsc::Receiver<DataBatch>,
    event_turn: &mut bool,
) -> FairMsg {
    let msg = if *event_turn {
        tokio::select! {
            biased;
            e = event_rx.recv() => FairMsg::Event(e),
            d = data_rx.recv() => FairMsg::Data(d),
        }
    } else {
        tokio::select! {
            biased;
            d = data_rx.recv() => FairMsg::Data(d),
            e = event_rx.recv() => FairMsg::Event(e),
        }
    };
    *event_turn = !*event_turn;
    msg
}

async fn writer_loop(
    mut wr: OwnedWriteHalf,
    mut control_rx: mpsc::Receiver<pb::Envelope>,
    mut data_rx: mpsc::Receiver<DataBatch>,
    mut event_rx: mpsc::Receiver<EventBatch>,
    sink: DataSink,
    active_epochs: Arc<Mutex<HashMap<u32, u64>>>,
    shutdown: CancellationToken,
) {
    // 事件先手：Alarm 风暴与 Data 洪峰同时到达时，第一帧优先保证事件不被 Data 抢占
    let mut event_turn = true;
    // P0-2 drain 模式：shutdown（独立 writer_shutdown，只在全部生产者停止后
    // 由 serve 触发）不再直接 break——先把队列里已有帧按原公平顺序写完再退。
    // active_epochs 保持不变，排队 Event 不会被 epoch 门丢掉。
    // 无生产者后队列只减不增，drain 必终止（顽固生产者由 serve 侧超时兜底）。
    let mut draining = false;
    loop {
        // drain 期队列一空即退，不等待（等待会永久 hanging：发送端虽闲置但未全
        // 释放，is_closed 永假；判空用 is_empty——单消费者，无并发竞态）。
        if draining && control_rx.is_empty() && data_rx.is_empty() && event_rx.is_empty() {
            break;
        }
        // 关+空的 control 不再轮询：已关闭通道的 recv 瞬时返回 None，
        // biased 下会永远抢占 recv_fair 造成空转饿死（drain 期 control
        // 先关是常态）。条件每轮重算（phase 1 仍可能有新控制帧）。
        let control_done = control_rx.is_closed() && control_rx.is_empty();
        tokio::select! {
            biased;
            _ = shutdown.cancelled(), if !draining => {
                draining = true;
            }
            c = control_rx.recv(), if !control_done => match c {
                Some(env) => {
                    if write_envelope(&mut wr, &env).await.is_err() { break; }
                }
                None => {
                    // 控制通道关闭：继续排空数据/事件通道或退出。
                    // P0-2：关闭 ≠ 排空——发送端已释放但队列仍有帧时必须
                    // 继续，不可 break（否则 drain 期 control 先关即丢尾）。
                    if data_rx.is_closed()
                        && data_rx.is_empty()
                        && event_rx.is_closed()
                        && event_rx.is_empty()
                    {
                        break;
                    }
                    continue;
                }
            },
            msg = recv_fair(&mut event_rx, &mut data_rx, &mut event_turn) => match msg {
                FairMsg::Event(Some(batch)) => {
                    // Epoch Gate（与 Data 同门）：StopAck 为事件面 barrier，
                    // 之后旧 epoch 的排队 EventBatch 必须丢弃，Core 永不可见。
                    let allowed = {
                        let g = active_epochs.lock().unwrap();
                        g.get(&batch.connection_handle).copied() == Some(batch.stream_epoch)
                    };
                    if !allowed {
                        continue;
                    }
                    let wire = match event_batch_to_pb(&batch) {
                        Ok(w) => w,
                        // 驱动 bug（publish 已做前置校验，理论不可达）：大声记录并丢弃，
                        // 绝不能让 writer 崩溃连累 Control/Data。
                        Err(e) => {
                            tracing::error!(
                                handle = batch.connection_handle,
                                error = %e,
                                "event batch rejected at wire time (driver bug), dropped"
                            );
                            continue;
                        }
                    };
                    let env = pb::Envelope {
                        msg_id: 0,
                        body: Some(pb::envelope::Body::EventBatch(wire)),
                    };
                    if write_envelope(&mut wr, &env).await.is_err() { break; }
                }
                FairMsg::Data(Some(batch)) => {
                    // Epoch Gate：已停止/过期的 stream 不得再发送（StopAck 为数据面 barrier）。
                    // 即使 batch 已在 Data Queue 中排队，也在真正写 socket 前丢弃。
                    let allowed = {
                        let g = active_epochs.lock().unwrap();
                        g.get(&batch.connection_handle).copied() == Some(batch.stream_epoch)
                    };
                    if !allowed {
                        // 丢弃过期批次，不写出；继续处理下一个
                        continue;
                    }
                    let env = pb::Envelope {
                        msg_id: 0,
                        body: Some(pb::envelope::Body::DataBatch(batch_to_pb(&batch))),
                    };
                    if write_envelope(&mut wr, &env).await.is_err() { break; }
                    else { sink.flush_pending(); }
                }
                FairMsg::Event(None) | FairMsg::Data(None) => {
                    // 某通道关闭：其余通道全关"且排空"才退出（P0-2：关闭≠排空），
                    // 否则继续服务剩余通道
                    if control_rx.is_closed()
                        && control_rx.is_empty()
                        && data_rx.is_closed()
                        && data_rx.is_empty()
                        && event_rx.is_closed()
                        && event_rx.is_empty()
                    {
                        break;
                    }
                    continue;
                }
            },
        }
        // 三通道全关且排空则退出（P0-2：关闭≠排空）
        if control_rx.is_closed()
            && control_rx.is_empty()
            && data_rx.is_closed()
            && data_rx.is_empty()
            && event_rx.is_closed()
            && event_rx.is_empty()
        {
            break;
        }
    }
}

async fn request_loop(
    session: &Session,
    mut rd: OwnedReadHalf,
    shutdown: &CancellationToken,
    faults: Option<&SdkFaults>,
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

        // 故障注入：hang 期间继续读帧（维持 TCP 层不堆积）但不做任何处理，
        // 使 Core 心跳超时判死（§14.4）
        if faults.map(|f| f.hanging()).unwrap_or(false) {
            continue;
        }

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
            Some(pb::envelope::Body::GetDescriptor(_)) => {
                let d = session.driver.descriptor();
                // 契约校验失败则视为驱动实现错误，仍以 JSON 透出并由 Core 侧校验
                let json = serde_json::to_string(&d).unwrap_or_else(|e| {
                    format!(r#"{{"error":"descriptor serialize failed: {e}"}}"#)
                });
                // 256 KiB 上限由 Core 侧强制，此处仅透传
                session
                    .sink
                    .send_control(pb::Envelope {
                        msg_id: env.msg_id,
                        body: Some(pb::envelope::Body::DescriptorReport(pb::DescriptorReport {
                            contract_major: d.contract_major,
                            contract_minor: d.contract_minor,
                            descriptor_json: json,
                        })),
                    })
                    .await;
            }
            Some(pb::envelope::Body::OpenConnection(req)) => {
                on_open_connection(session, req, env.msg_id).await;
            }
            Some(pb::envelope::Body::ConfigureTasks(req)) => {
                on_configure(session, req, env.msg_id).await
            }
            Some(pb::envelope::Body::ApplyPointMap(req)) => {
                on_apply_point_map(session, req, env.msg_id).await;
            }
            Some(pb::envelope::Body::ConfigureEventTasks(req)) => {
                on_configure_events(session, req, env.msg_id).await
            }
            Some(pb::envelope::Body::StartConnection(req)) => {
                on_start(session, req, env.msg_id).await
            }
            Some(pb::envelope::Body::StopConnection(req)) => {
                on_stop(session, req, env.msg_id).await
            }
            Some(pb::envelope::Body::CloseConnection(req)) => {
                on_close(session, req, env.msg_id).await
            }
            Some(pb::envelope::Body::BrowseRequest(req)) => {
                on_browse(session, req, env.msg_id).await
            }
            Some(pb::envelope::Body::ProbeRequest(req)) => on_probe(session, req, env.msg_id).await,
            Some(pb::envelope::Body::WriteRequest(req)) => on_write(session, req, env.msg_id).await,
            Some(pb::envelope::Body::CommandRequest(req)) => {
                on_command(session, req, env.msg_id).await
            }
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
        match session
            .driver
            .open_connection(&req.endpoint_id, &req.config_json)
            .await
        {
            Ok(conn) => {
                session.entries.lock().unwrap().insert(
                    req.connection_handle,
                    ConnEntry {
                        conn: Some(conn),
                        run: None,
                        revision: 0,
                        epoch: 0,
                    },
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
            body: Some(pb::envelope::Body::OpenConnectionAck(
                pb::OpenConnectionAck {
                    connection_handle: req.connection_handle,
                    result: Some(result),
                },
            )),
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
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::DriverError(pb::DriverErrorReport {
                    connection_handle: Some(req.connection_handle),
                    detail: Some(error_detail(
                        ErrorKind::Internal,
                        "NO_CONNECTION",
                        "connection not open".to_string(),
                    )),
                })),
            })
            .await;
        return;
    };

    match tasks_from_pb(req.tasks) {
        Ok(tasks) => match conn.configure(req.revision, tasks).await {
            Ok(descriptors) => {
                if let Some(e) = session
                    .entries
                    .lock()
                    .unwrap()
                    .get_mut(&req.connection_handle)
                {
                    e.conn = Some(conn);
                    e.revision = req.revision;
                }
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
                if let Some(e) = session
                    .entries
                    .lock()
                    .unwrap()
                    .get_mut(&req.connection_handle)
                {
                    e.conn = Some(conn);
                }
                session
                    .sink
                    .send_control(pb::Envelope {
                        msg_id,
                        body: Some(pb::envelope::Body::DriverError(pb::DriverErrorReport {
                            connection_handle: Some(req.connection_handle),
                            detail: Some(error_detail(err.kind, &err.code, err.message)),
                        })),
                    })
                    .await;
            }
        },
        Err(e) => {
            let err: SdkDriverError = e.into();
            if let Some(e) = session
                .entries
                .lock()
                .unwrap()
                .get_mut(&req.connection_handle)
            {
                e.conn = Some(conn);
            }
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::DriverError(pb::DriverErrorReport {
                        connection_handle: Some(req.connection_handle),
                        detail: Some(error_detail(err.kind, &err.code, err.message)),
                    })),
                })
                .await;
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
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::DriverError(pb::DriverErrorReport {
                    connection_handle: Some(req.connection_handle),
                    detail: Some(error_detail(
                        ErrorKind::Internal,
                        "NO_CONNECTION",
                        "connection not open".to_string(),
                    )),
                })),
            })
            .await;
        return;
    };

    let res = conn.apply_point_map(req.key_to_point_id).await;
    if let Some(e) = session
        .entries
        .lock()
        .unwrap()
        .get_mut(&req.connection_handle)
    {
        e.conn = Some(conn);
    }
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

/// 事件任务配置（proto 42 → 43）：错误统一走 `EventConfigApplied.result`，
/// 不另造 DriverError 分流——调用方只需看一处结果（PR6 接线约定）。
async fn on_configure_events(session: &Session, req: pb::ConfigureEventTasks, msg_id: u64) {
    // 取出连接对象供 configure_events 使用；失败必须归还，避免"打开但不可用"
    let taken = session
        .entries
        .lock()
        .unwrap()
        .get_mut(&req.connection_handle)
        .and_then(|e| e.conn.take());

    let Some(mut conn) = taken else {
        session
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::EventConfigApplied(
                    pb::EventConfigApplied {
                        connection_handle: req.connection_handle,
                        revision: req.revision,
                        result: Some(err_result(
                            ErrorKind::Internal,
                            "NO_CONNECTION",
                            "connection not open",
                        )),
                    },
                )),
            })
            .await;
        return;
    };

    // wire → Core EventTask：逐条解码，任一非法即整个 revision 失败（§35）
    let tasks: Result<Vec<EventTask>, ConvertError> =
        req.tasks.into_iter().map(event_task_from_pb).collect();
    let result = match tasks {
        Err(e) => {
            let err = SdkDriverError::from(e);
            err_result(err.kind, &err.code, err.message)
        }
        Ok(tasks) => match conn.configure_events(req.revision, tasks).await {
            Ok(()) => ok_result(),
            Err(err) => err_result(err.kind, &err.code, err.message),
        },
    };
    // 归还连接（configure_events 不消耗连接对象）
    if let Some(e) = session
        .entries
        .lock()
        .unwrap()
        .get_mut(&req.connection_handle)
    {
        e.conn = Some(conn);
    }
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::EventConfigApplied(
                pb::EventConfigApplied {
                    connection_handle: req.connection_handle,
                    revision: req.revision,
                    result: Some(result),
                },
            )),
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
        // NOTE: 拒绝路径也必须回显请求 msg_id，否则 Core 侧请求无法关联回复
        session
            .send_control_ack_start(
                msg_id,
                req.connection_handle,
                false,
                "missing connection or already running",
            )
            .await;
        return;
    };

    let run_cancel = CancellationToken::new();
    let task_cancel = run_cancel.clone();
    let gate_cancel = run_cancel.clone();
    // 每个连接独立 sink：合并缓冲隔离 + 自动盖 handle/epoch 戳
    let sink_for_run = session
        .sink
        .for_connection(req.connection_handle, req.stream_epoch);
    let handle = req.connection_handle;
    let entries = Arc::clone(&session.entries);
    let mut conn = conn; // &mut 调用需要可变绑定

    // Start release gate（P0）：run 任务先 park，Ack 入队后才放行。
    // 否则首批 Data/Event 可能抢在 active_epochs.insert（SDK 门）或
    // StartConnectionAck（Core 门）之前产生，被当 stale 静默丢弃——
    // seq=1 的 occurrence 永久丢失，而连接显示 RUNNING。
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    // 采集任务独立于请求循环运行；结束（含出错）时上报终态，
    // 并把连接对象归还回表，使该连接可被再次 Configure/Start（§21 可重复启停）。
    let join = tokio::spawn(async move {
        // park 期被 Stop/Shutdown 取消：归还连接并静默退出（StopAck/Shutdown
        // 本身即确认，不再上报终态，避免 Stop 后多出一条幽灵 Stopped）。
        // release 端释放而未发送（on_start 未走完，如进程退出中）：同样归还退出。
        tokio::select! {
            r = release_rx => {
                if r.is_err() {
                    if let Some(entry) = entries.lock().unwrap().get_mut(&handle) {
                        entry.conn = Some(conn);
                    }
                    return;
                }
            }
            _ = gate_cancel.cancelled() => {
                if let Some(entry) = entries.lock().unwrap().get_mut(&handle) {
                    entry.conn = Some(conn);
                }
                return;
            }
        }
        let outcome = conn.run(sink_for_run.clone(), task_cancel).await;
        // 先归还再上报：保证 Stop ack 返回时连接已可复用
        if let Some(entry) = entries.lock().unwrap().get_mut(&handle) {
            entry.conn = Some(conn);
        }
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
            entry.run = Some(RunHandle {
                cancel: run_cancel,
                join,
            });
        }
    }
    // 激活 epoch 门控：此后该 handle 的旧 epoch 批次将被 writer 丢弃
    session
        .active_epochs
        .lock()
        .unwrap()
        .insert(req.connection_handle, req.stream_epoch);

    // Start 的 Ack 复用请求 msg_id，便于 Core 侧关联。
    // Ack 走 Control 可靠队列（writer biased 恒优先）：Ack 先入队、run 后放行，
    // wire 顺序恒为 `StartConnectionAck → first Data/EventBatch`（Data 同理）。
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::StartConnectionAck(
                pb::StartConnectionAck {
                    connection_handle: req.connection_handle,
                    result: Some(ok_result()),
                },
            )),
        })
        .await;
    let _ = release_tx.send(());
}

async fn on_stop(session: &Session, req: pb::StopConnection, msg_id: u64) {
    session.stop_run(req.connection_handle).await;
    // 去活门控：StopAck 为数据面 barrier，之后旧 epoch 的排队批次必须丢弃
    session
        .active_epochs
        .lock()
        .unwrap()
        .remove(&req.connection_handle);
    // 清理 per-connection pending 合并缓冲，避免长生命周期泄漏
    session
        .sink
        .pending_registry
        .lock()
        .unwrap()
        .remove(&req.connection_handle);
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::StopConnectionAck(
                pb::StopConnectionAck {
                    connection_handle: req.connection_handle,
                    result: Some(ok_result()),
                },
            )),
        })
        .await;
}

async fn on_close(session: &Session, req: pb::CloseConnection, msg_id: u64) {
    session.stop_run(req.connection_handle).await;
    session
        .active_epochs
        .lock()
        .unwrap()
        .remove(&req.connection_handle);
    session
        .entries
        .lock()
        .unwrap()
        .remove(&req.connection_handle);
    session
        .sink
        .pending_registry
        .lock()
        .unwrap()
        .remove(&req.connection_handle);
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(pb::envelope::Body::CloseConnectionAck(
                pb::CloseConnectionAck {
                    connection_handle: req.connection_handle,
                    result: Some(ok_result()),
                },
            )),
        })
        .await;
}

/// 动态探测（§8）：从 entries 取出 `connection_handle` 对应的连接，
/// 调用 `DriverConnection::probe()`（复用其已建协议会话），用完归还。
/// 成功 → `ProbeResponse{report_json}`（64 KiB 上限内，发送前二次检查）；
/// 失败（含 handle 未打开）→ 同 msg_id 的 `DriverErrorReport`（复用等待路由）。
async fn on_probe(session: &Session, req: pb::ProbeRequest, msg_id: u64) {
    let conn_opt = {
        session
            .entries
            .lock()
            .unwrap()
            .get_mut(&req.connection_handle)
            .and_then(|e| e.conn.take())
    };
    let Some(mut conn) = conn_opt else {
        session
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::DriverError(pb::DriverErrorReport {
                    connection_handle: Some(req.connection_handle),
                    detail: Some(error_detail(
                        mesa_core_types::ErrorKind::Internal,
                        "NO_CONNECTION",
                        "probe: connection not open",
                    )),
                })),
            })
            .await;
        return;
    };
    let body = match conn.probe().await {
        Ok(report) => match report.to_report_json() {
            Ok(json) => pb::envelope::Body::ProbeResponse(pb::ProbeResponse { report_json: json }),
            Err(e) => pb::envelope::Body::DriverError(pb::DriverErrorReport {
                connection_handle: Some(req.connection_handle),
                detail: Some(error_detail(
                    mesa_core_types::ErrorKind::Internal,
                    "PROBE_SERIALIZE_FAILED",
                    e,
                )),
            }),
        },
        Err(err) => pb::envelope::Body::DriverError(pb::DriverErrorReport {
            connection_handle: Some(req.connection_handle),
            detail: Some(error_detail(err.kind, &err.code, err.message)),
        }),
    };
    // 归还连接（probe 不消耗连接对象）
    if let Some(entry) = session
        .entries
        .lock()
        .unwrap()
        .get_mut(&req.connection_handle)
    {
        entry.conn = Some(conn);
    }
    session
        .sink
        .send_control(pb::Envelope {
            msg_id,
            body: Some(body),
        })
        .await;
}

async fn on_browse(session: &Session, req: pb::BrowseRequest, msg_id: u64) {
    // 取对应连接，若 handle 为 0 则取任意已打开连接（兼容 probe 场景）
    let conn_opt = {
        let mut m = session.entries.lock().unwrap();
        if let Some(entry) = m.get_mut(&req.connection_handle) {
            entry.conn.take()
        } else if req.connection_handle == 0 {
            // 取第一个可用连接
            let mut found = None;
            for (_, e) in m.iter_mut() {
                if e.conn.is_some() {
                    found = e.conn.take();
                    break;
                }
            }
            found
        } else {
            None
        }
    };
    let Some(mut conn) = conn_opt else {
        session
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::BrowseResponse(pb::BrowseResponse {
                    nodes: vec![],
                    next_cursor: "".into(),
                    result: Some(err_result(
                        ErrorKind::Internal,
                        "NO_CONNECTION",
                        "browse: connection not open",
                    )),
                })),
            })
            .await;
        return;
    };
    let res = conn
        .browse(&req.parent, &req.filter, &req.cursor, req.limit)
        .await;
    // 归还连接
    {
        let mut m = session.entries.lock().unwrap();
        if let Some(entry) = m.get_mut(&req.connection_handle) {
            entry.conn = Some(conn);
        } else {
            // 若原 handle 为 0，归还到任意
            for (_, e) in m.iter_mut() {
                if e.conn.is_none() {
                    e.conn = Some(conn);
                    break;
                }
            }
        }
    }
    match res {
        Ok((nodes, next)) => {
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::BrowseResponse(pb::BrowseResponse {
                        nodes,
                        next_cursor: next.unwrap_or_default(),
                        result: Some(ok_result()),
                    })),
                })
                .await;
        }
        Err(e) => {
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::BrowseResponse(pb::BrowseResponse {
                        nodes: vec![],
                        next_cursor: "".into(),
                        result: Some(err_result(e.kind, &e.code, e.message)),
                    })),
                })
                .await;
        }
    }
}

async fn on_write(session: &Session, req: pb::WriteRequest, msg_id: u64) {
    let conn_opt = {
        let mut m = session.entries.lock().unwrap();
        if let Some(entry) = m.get_mut(&req.connection_handle) {
            entry.conn.take()
        } else {
            None
        }
    };
    let exists = {
        let m = session.entries.lock().unwrap();
        m.contains_key(&req.connection_handle)
    };
    if conn_opt.is_none() && !exists {
        session
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::WriteResponse(pb::WriteResponse {
                    request_id: req.request_id,
                    result: Some(err_result(
                        ErrorKind::Internal,
                        "NO_CONNECTION",
                        "write: connection not open",
                    )),
                    readback: None,
                })),
            })
            .await;
        return;
    }
    let Some(mut conn) = conn_opt else {
        session
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::WriteResponse(pb::WriteResponse {
                    request_id: req.request_id,
                    result: Some(err_result(
                        ErrorKind::Internal,
                        "BUSY",
                        "write: connection busy",
                    )),
                    readback: None,
                })),
            })
            .await;
        return;
    };
    let value = match req.value {
        Some(v) => match mesa_driver_protocol::value_from_pb(v) {
            Ok(v) => v,
            Err(e) => {
                {
                    let mut m = session.entries.lock().unwrap();
                    if let Some(e2) = m.get_mut(&req.connection_handle) {
                        e2.conn = Some(conn);
                    }
                }
                session
                    .sink
                    .send_control(pb::Envelope {
                        msg_id,
                        body: Some(pb::envelope::Body::WriteResponse(pb::WriteResponse {
                            request_id: req.request_id,
                            result: Some(err_result(
                                ErrorKind::Internal,
                                "BAD_VALUE",
                                format!("decode value: {e}"),
                            )),
                            readback: None,
                        })),
                    })
                    .await;
                return;
            }
        },
        None => {
            {
                let mut m = session.entries.lock().unwrap();
                if let Some(e2) = m.get_mut(&req.connection_handle) {
                    e2.conn = Some(conn);
                }
            }
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::WriteResponse(pb::WriteResponse {
                        request_id: req.request_id,
                        result: Some(err_result(
                            ErrorKind::Internal,
                            "MISSING_VALUE",
                            "write: missing value",
                        )),
                        readback: None,
                    })),
                })
                .await;
            return;
        }
    };
    let expected = req
        .expected_value
        .and_then(|v| mesa_driver_protocol::value_from_pb(v).ok());
    let res = conn.write(&req.target, value, expected).await;
    {
        let mut m = session.entries.lock().unwrap();
        if let Some(e2) = m.get_mut(&req.connection_handle) {
            e2.conn = Some(conn);
        }
    }
    match res {
        Ok(()) => {
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::WriteResponse(pb::WriteResponse {
                        request_id: req.request_id,
                        result: Some(mesa_driver_protocol::ok_result()),
                        readback: None,
                    })),
                })
                .await;
        }
        Err(e) => {
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::WriteResponse(pb::WriteResponse {
                        request_id: req.request_id,
                        result: Some(err_result(e.kind, &e.code, e.message)),
                        readback: None,
                    })),
                })
                .await;
        }
    }
}

async fn on_command(session: &Session, req: pb::CommandRequest, msg_id: u64) {
    let conn_opt = {
        let mut m = session.entries.lock().unwrap();
        if let Some(entry) = m.get_mut(&req.connection_handle) {
            entry.conn.take()
        } else {
            None
        }
    };
    let exists = {
        let m = session.entries.lock().unwrap();
        m.contains_key(&req.connection_handle)
    };
    if conn_opt.is_none() && !exists {
        session
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::CommandResponse(pb::CommandResponse {
                    request_id: req.request_id,
                    status: "Failed".into(),
                    result_json: "".into(),
                    error: "NO_CONNECTION: command: connection not open".into(),
                })),
            })
            .await;
        return;
    }
    let Some(mut conn) = conn_opt else {
        session
            .sink
            .send_control(pb::Envelope {
                msg_id,
                body: Some(pb::envelope::Body::CommandResponse(pb::CommandResponse {
                    request_id: req.request_id,
                    status: "Failed".into(),
                    result_json: "".into(),
                    error: "BUSY: command: connection busy".into(),
                })),
            })
            .await;
        return;
    };
    let res = conn.command(&req.command_id, &req.input_json).await;
    {
        let mut m = session.entries.lock().unwrap();
        if let Some(e2) = m.get_mut(&req.connection_handle) {
            e2.conn = Some(conn);
        }
    }
    match res {
        Ok(v) => {
            let json = serde_json::to_string(&v).unwrap_or_else(|_| "{}".into());
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::CommandResponse(pb::CommandResponse {
                        request_id: req.request_id,
                        status: "Succeeded".into(),
                        result_json: json,
                        error: "".into(),
                    })),
                })
                .await;
        }
        Err(e) => {
            session
                .sink
                .send_control(pb::Envelope {
                    msg_id,
                    body: Some(pb::envelope::Body::CommandResponse(pb::CommandResponse {
                        request_id: req.request_id,
                        status: "Failed".into(),
                        result_json: "".into(),
                        error: format!("{}/{}: {}", e.kind.as_str(), e.code, e.message),
                    })),
                })
                .await;
        }
    }
}

impl Session {
    async fn send_control_ack_start(&self, msg_id: u64, handle: u32, ok: bool, why: &str) {
        let result = if ok {
            ok_result()
        } else {
            err_result(ErrorKind::Internal, "NOT_CONFIGURED", why)
        };
        self.sink
            .send_control(pb::Envelope {
                // 响应必须回显请求的 msg_id（§14 请求/响应关联规则）
                msg_id,
                body: Some(pb::envelope::Body::StartConnectionAck(
                    pb::StartConnectionAck {
                        connection_handle: handle,
                        result: Some(result),
                    },
                )),
            })
            .await;
    }
}

/// 独立于 Session 的状态发送（供已 spawn 的 run 任务使用）。
async fn send_state_direct(sink: &DataSink, handle: u32, state: ConnectionState, detail: &str) {
    sink.send_control(pb::Envelope {
        msg_id: 0,
        body: Some(pb::envelope::Body::ConnectionStateChanged(
            pb::ConnectionStateChanged {
                connection_handle: handle,
                state: state.as_str().to_string(),
                detail: detail.to_string(),
            },
        )),
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(seq: u64, value: f64) -> DataBatch {
        DataBatch {
            connection_handle: 0,
            stream_epoch: 0,
            sequence: seq,
            timestamp_ns: mesa_core_types::now_unix_ns(),
            values: vec![mesa_core_types::PointValue::good(
                1,
                mesa_core_types::Value::F64(value),
            )],
            mono_ns: None,
        }
    }

    /// 背压语义（§12 / §21 Backpressure 行）的确定性验证：
    /// 通道满载时同点批次合并为最新值，coalesced 计数可观测，
    /// 腾出空间后积压的"最新值"最终可达。
    /// 会话级 sink + 存活的接收端（_erx 必须在测试作用域内存活，
    /// 否则 try_send 直接 Closed）。
    fn test_session_sink() -> (DataSink, mpsc::Receiver<EventBatch>) {
        let (control_tx, _) = mpsc::channel::<pb::Envelope>(CONTROL_CAPACITY);
        let (data_tx, _) = mpsc::channel::<DataBatch>(DATA_CAPACITY);
        let (event_tx, erx) = mpsc::channel::<EventBatch>(EVENT_CAPACITY);
        (DataSink::new(control_tx, data_tx, event_tx), erx)
    }

    #[tokio::test]
    async fn sink_coalesces_latest_wins_when_full() {
        let (control_tx, _cr) = mpsc::channel::<pb::Envelope>(CONTROL_CAPACITY);
        let (data_tx, mut rx) = mpsc::channel::<DataBatch>(1);
        let (event_tx, _er) = mpsc::channel::<EventBatch>(EVENT_CAPACITY);
        let session_sink = DataSink::new(control_tx, data_tx, event_tx);
        let sink = session_sink.for_connection(7, 99);

        // 第一批直接入队（占满容量 1）
        sink.publish(batch(1, 1.0)).await;
        // 后续批次通道满：第一批溢出的成为合并基底（不计入 coalesced），
        // 其余 48 批被合并覆盖
        for seq in 2..=50u64 {
            sink.publish(batch(seq, seq as f64)).await;
        }
        assert!(
            sink.coalesced_points() >= 48,
            "overflow batches must be accounted as coalesced, got {}",
            sink.coalesced_points()
        );

        // 消费者取走第一批后模拟 writer 出帧成功的补发路径
        match rx.recv().await {
            Some(b) => assert_eq!(b.sequence, 1),
            other => panic!("expected first batch, got {other:?}"),
        }
        sink.flush_pending();
        match rx.recv().await {
            Some(b) => {
                // Latest-Wins：只剩最新一批，值为该批的值
                assert_eq!(b.sequence, 50);
                assert_eq!(b.values.len(), 1);
                assert_eq!(b.values[0].value, mesa_core_types::Value::F64(50.0));
            }
            other => panic!("expected merged latest batch, got {other:?}"),
        }
    }

    fn event(id: &str) -> EventRecord {
        EventRecord {
            event_id: id.into(),
            category: "alarm".into(),
            kind: "alarm.condition".into(),
            source: "Channel1".into(),
            severity: 700,
            code: None,
            message: None,
            message_locale: None,
            occurred_at_ns: None,
            condition: None,
            correlation_id: None,
            attributes: Default::default(),
        }
    }

    /// Event Plane 背压冻结（PR10 §5）：满队列 publish 必须等待，
    /// 不失败、不丢弃、不合并。腾出空间后完成且序号连续、内容 intact。
    /// 与 Data Latest-Wins 的对照：事件走独立可靠队列，无 QueueFull 变体。
    #[tokio::test]
    async fn event_sink_full_waits_without_loss_or_coalesce() {
        use std::time::Duration;
        let (session_sink, mut erx) = test_session_sink();
        let conn = session_sink.for_connection(7, 99);
        let events = conn.events();
        for i in 1..=EVENT_CAPACITY {
            let seq = events
                .publish(vec![event(&format!("w-{i}"))])
                .await
                .unwrap();
            assert_eq!(seq, i as u64);
        }
        // 第 129 个 publish 必须 pending（200ms 内不得完成/失败）
        let pend = tokio::spawn(async move { events.publish(vec![event("w-last")]).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !pend.is_finished(),
            "满队列 publish 必须等待，不失败不丢不合并"
        );
        // 消费腾出空间 → 完成，序号连续
        let first = erx.recv().await.expect("首批必在");
        assert_eq!(first.events[0].event_id, "w-1");
        let seq = tokio::time::timeout(Duration::from_secs(5), pend)
            .await
            .expect("腾出空间后必须完成")
            .unwrap()
            .unwrap();
        assert_eq!(seq, EVENT_CAPACITY as u64 + 1);
        // 排空：剩余顺序 intact（w-2..w-128, w-last）
        let mut ids = vec![];
        for _ in 0..EVENT_CAPACITY {
            let b = erx.recv().await.expect("排空不得缺");
            ids.extend(b.events.iter().map(|r| r.event_id.clone()));
        }
        assert_eq!(ids.len(), EVENT_CAPACITY);
        assert_eq!(ids[0], "w-2");
        assert_eq!(ids[EVENT_CAPACITY - 1], "w-last");
        assert!(erx.try_recv().is_err());
    }

    /// EventSink 自动盖戳 + sequence 按 (handle, epoch) 独立递增；
    /// 新 epoch 从 1 重来（§11）。
    #[tokio::test]
    async fn event_sink_stamps_header_and_sequences_per_epoch() {
        let (root, _erx) = test_session_sink();
        let sink = root.for_connection(7, 100);
        let events = sink.events();
        let s1 = events.publish(vec![event("e1")]).await.unwrap();
        let s2 = events.publish(vec![event("e2")]).await.unwrap();
        assert_eq!((s1, s2), (1, 2));
        // 同一 sink 派生的第二个 EventSink 共享序号（同一连接同一 epoch）
        let s3 = sink.events().publish(vec![event("e3")]).await.unwrap();
        assert_eq!(s3, 3);
        // 新 epoch 从 1 重来；旧 epoch 序号不继承（同 registry：真实路径由
        // serve() 持有的 DataSink 派生，for_connection 共享 event_seq）
        let sink_new_epoch = sink.for_connection(7, 101);
        let r = sink_new_epoch
            .events()
            .publish(vec![event("e4")])
            .await
            .unwrap();
        assert_eq!(r, 1);
    }

    /// Event 永不合并永不丢：队列满时 publish 等待（背压），而不是合并、
    /// 覆盖或报错丢弃；腾出空间后等待的批次完整入队且序号连续。
    #[tokio::test]
    async fn event_sink_backpressures_instead_of_dropping() {
        let (control_tx, _) = mpsc::channel::<pb::Envelope>(CONTROL_CAPACITY);
        let (data_tx, _) = mpsc::channel::<DataBatch>(DATA_CAPACITY);
        // 容量 1：第一批占满
        let (event_tx, mut erx) = mpsc::channel::<EventBatch>(1);
        let sink = DataSink::new(control_tx, data_tx, event_tx).for_connection(7, 100);
        let events = sink.events();
        events.publish(vec![event("e1")]).await.unwrap();

        // 第二批在满队列上必须等待：50ms 内不得完成（没合并、没覆盖、没报错）
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let parked = events.clone();
        tokio::spawn(async move {
            let r = parked.publish(vec![event("e2")]).await;
            let _ = done_tx.send(r);
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), done_rx)
                .await
                .is_err(),
            "full queue must park the producer, not drop/merge/reject"
        );
        // 腾出空间 → 等待的批次完整入队，序号连续（1 → 2，无缺口）
        let b = erx.recv().await.unwrap();
        assert_eq!(b.sequence, 1);
        assert_eq!(b.events[0].event_id, "e1");
        let b = tokio::time::timeout(Duration::from_secs(2), erx.recv())
            .await
            .expect("parked publish must complete after drain")
            .unwrap();
        assert_eq!(b.sequence, 2);
        assert_eq!(b.events.len(), 1);
        assert_eq!(b.events[0].event_id, "e2");

        // 会话 teardown（接收端释放）→ 等待以 Closed 结束，不 hanging
        drop(erx);
        assert_eq!(
            events.publish(vec![event("e3")]).await.unwrap_err(),
            EventPublishError::Closed
        );
    }

    /// 多 publisher 并发：到达顺序必须恰为 1..=N（P0-2 分配→入队原子化）。
    #[tokio::test]
    async fn event_sink_concurrent_publishers_stay_ordered() {
        let (control_tx, _) = mpsc::channel::<pb::Envelope>(CONTROL_CAPACITY);
        let (data_tx, _) = mpsc::channel::<DataBatch>(DATA_CAPACITY);
        let (event_tx, mut erx) = mpsc::channel::<EventBatch>(64);
        let sink = DataSink::new(control_tx, data_tx, event_tx).for_connection(7, 100);
        let mut handles = Vec::new();
        for t in 0..8u64 {
            let e = sink.events();
            handles.push(tokio::spawn(async move {
                for i in 0..5u64 {
                    let id = format!("t{t}-{i}");
                    e.publish(vec![event(&id)]).await.unwrap();
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        // 40 批到达顺序必须严格 1..=40（旧 HashMap+先分配后发送必现 11→10 倒置）
        for expect in 1..=40u64 {
            let b = tokio::time::timeout(Duration::from_secs(2), erx.recv())
                .await
                .expect("ordered delivery")
                .unwrap();
            assert_eq!(b.sequence, expect, "wire 顺序必须等于序号顺序");
        }
    }

    /// P0-2 SDK outbound drain 的确定性 Gate：128 个 publish 全部 Ok 后
    /// （writer 尚未启动，backlog 确定非空），再启动 writer 且 writer_shutdown
    /// 已预取消（serve 两阶段的第③步快照）；读端（Core 侧）必须在 EOF 前收到
    /// 全部 128 个 EventBatch 且 sequence 连续，随后 writer 退出关闭写半部。
    /// 旧行为（shutdown 直接 break）下首轮 poll 即退，0 帧即 EOF，必红。
    /// 零竞态：backlog 先于 writer 存在，不依赖 socket 阻塞时序。
    #[tokio::test]
    async fn writer_shutdown_drains_queued_events_before_fin() {
        const N: u64 = EVENT_CAPACITY as u64;
        const HANDLE: u32 = 7;
        const EPOCH: u64 = 4242;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accept = tokio::spawn(async move { listener.accept().await.unwrap().0 });
        let cli = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let server = accept.await.unwrap();

        let (control_tx, control_rx) = mpsc::channel::<pb::Envelope>(CONTROL_CAPACITY);
        let (data_tx, data_rx) = mpsc::channel::<DataBatch>(DATA_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel::<EventBatch>(EVENT_CAPACITY);
        // 生产者：128 个 publish 全 Ok，writer 未启动 → 队列满且无人消费
        let producer = DataSink::new(control_tx, data_tx, event_tx).for_connection(HANDLE, EPOCH);
        let events = producer.events();
        for i in 1..=N {
            let seq = events
                .publish(vec![event(&format!("q-{i}"))])
                .await
                .unwrap();
            assert_eq!(seq, i);
        }
        drop(producer);
        // writer 自持的 sink（idle，不生产；drain 判空用 is_empty，不依赖关闭）
        let (c2, _) = mpsc::channel::<pb::Envelope>(CONTROL_CAPACITY);
        let (d2, _) = mpsc::channel::<DataBatch>(DATA_CAPACITY);
        let (e2, _) = mpsc::channel::<EventBatch>(EVENT_CAPACITY);
        let sink = DataSink::new(c2, d2, e2);
        let epochs = Arc::new(Mutex::new(HashMap::from([(HANDLE, EPOCH)])));
        // serve 第③步快照：生产者已全部停止后才触发 writer_shutdown
        let writer_shutdown = CancellationToken::new();
        writer_shutdown.cancel();
        // 方向：writer 写 accept 侧（Driver→Core），读端在 connect 侧；
        // 各自另一半保持存活 idle（避免 RST/EOF 误判），不参与读写。
        let (crd, _cwr) = cli.into_split();
        let (_srd, swr) = server.into_split();
        let mut srd = crd;
        let w = tokio::spawn(writer_loop(
            swr,
            control_rx,
            data_rx,
            event_rx,
            sink,
            epochs,
            writer_shutdown,
        ));
        // Core 侧：读完 128 帧，随后必须是 EOF（writer 退出关写半部）
        for expect in 1..=N {
            let env = tokio::time::timeout(Duration::from_secs(10), read_envelope(&mut srd))
                .await
                .expect("drain 不得超时")
                .expect("帧必须可解");
            match env.body {
                Some(pb::envelope::Body::EventBatch(b)) => {
                    assert_eq!(b.connection_handle, HANDLE);
                    assert_eq!(b.stream_epoch, EPOCH);
                    assert_eq!(b.sequence, expect, "drain 不得丢尾/乱序");
                }
                other => panic!("expect EventBatch, got {other:?}"),
            }
        }
        let tail = read_envelope(&mut srd).await;
        assert!(
            matches!(&tail, Err(ProtocolError::Io(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof),
            "128 帧后必须是 EOF（writer 已退出），got {tail:?}"
        );
        tokio::time::timeout(Duration::from_secs(5), w)
            .await
            .expect("writer 必须退出")
            .unwrap();
    }

    /// publish 前置校验：坏记录/空批/未绑定当场拒绝，不占用队列。
    #[tokio::test]
    async fn event_sink_rejects_invalid_upfront() {
        let (root, _erx) = test_session_sink();
        let sink = root.for_connection(7, 100);
        let events = sink.events();
        assert_eq!(
            events.publish(vec![]).await.unwrap_err(),
            EventPublishError::Empty
        );
        let mut bad = event("bad");
        bad.event_id = String::new();
        assert!(matches!(
            events.publish(vec![bad]).await.unwrap_err(),
            EventPublishError::InvalidRecord(_)
        ));
        let (root2, _erx2) = test_session_sink();
        let unbound = root2.events();
        assert_eq!(
            unbound.publish(vec![event("x")]).await.unwrap_err(),
            EventPublishError::Unbound
        );
    }

    /// 默认 configure_events：空任务接受，非空拒绝（老 Driver 零改动）。
    #[tokio::test]
    async fn default_configure_events_accepts_empty_rejects_nonempty() {
        struct Stub;
        #[async_trait::async_trait]
        impl DriverConnection for Stub {
            async fn configure(
                &mut self,
                _r: u64,
                _t: Vec<AcquisitionTask>,
            ) -> Result<Vec<mesa_core_types::PointDescriptor>, SdkDriverError> {
                Ok(vec![])
            }
            async fn apply_point_map(&mut self, _m: PointMap) -> Result<(), SdkDriverError> {
                Ok(())
            }
            async fn run(
                &mut self,
                _s: DataSink,
                _c: CancellationToken,
            ) -> Result<(), SdkDriverError> {
                Ok(())
            }
        }
        let mut stub = Stub;
        assert!(stub.configure_events(1, vec![]).await.is_ok());
        let task = EventTask {
            id: "e1".into(),
            mode: mesa_core_types::TaskMode::Subscribe,
            interval_ms: None,
            binding: mesa_core_types::DriverBinding {
                kind: "k".into(),
                config: serde_json::Value::Null,
            },
        };
        let err = stub.configure_events(1, vec![task]).await.unwrap_err();
        assert_eq!(err.code, "EVENT_NOT_SUPPORTED");
    }
}
