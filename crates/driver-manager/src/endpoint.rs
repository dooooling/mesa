//! Endpoint 运行时（Core 侧）：配置闭环 + 数据消费 + 故障处置。
//!
//! 完整实现方案 §6.2 配置流程与 §11.1 重连语义：
//! `spawn -> handshake -> Open -> Configure -> PointDescriptors -> ApplyPointMap
//!   -> Start(new epoch) -> 事件循环`；
//! 断连/无响应时按退避序列自动重建；配置类错误直接 FAILED 不自动重试。
//!
//! Point ID 分配通过 [`PointIdSource`] 抽象：内存版用于 Contract Test，
//! 持久版 [`StorePointIdSource`] 委托 `ConfigStore`（带墓碑语义）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mesa_core_types::{
    ConnectionState, DataBatch, EventTask, PointDefinition, PointDescriptor,
    ensure_unique_point_keys,
};
use mesa_driver_protocol::pb;
use mesa_event_store::EventServices;
use tokio_util::sync::CancellationToken;

use crate::event_ingress::{EventDrainError, IngressFatal, run_event_ingress};
use crate::manifest::DiscoveredDriver;
use crate::process::DriverProcess;
use crate::session::{HeartbeatParams, Session, SessionEvent};
use crate::snapshot::{EndpointStatus, Snapshot};

/// 连接级退避序列（§11.1 默认值）。当前不做上限熔断，成功后归零。
/// 测试时 `MESA_RECONNECT_FAST=1` 缩短为 1s 固定，避免 30s/60s 阻塞。
///
/// 拆成两层是为了让表与边界可被纯函数单测覆盖：`reconnect_backoff_secs` 仍是
/// 唯一读取进程 env 的入口（语义不变），`backoff_delay` 不含副作用。
fn reconnect_backoff_secs(idx: usize) -> u64 {
    backoff_delay(idx, reconnect_fast_from_env())
}

fn reconnect_fast_from_env() -> bool {
    std::env::var("MESA_RECONNECT_FAST").ok().as_deref() == Some("1")
}

/// 退避表与 `idx.min(4)` 边界的纯函数（P1-A1 单测锚点，不得改变语义）。
fn backoff_delay(idx: usize, fast: bool) -> u64 {
    if fast {
        return 1;
    }
    const RECONNECT_BACKOFF_SECS: [u64; 5] = [1, 2, 5, 10, 30];
    RECONNECT_BACKOFF_SECS[idx.min(RECONNECT_BACKOFF_SECS.len() - 1)]
}

/// P1-A1 归零判据：只有“本次 attempt 走到了事件循环”**且**“本次 attempt 的
/// 消费路径收到过至少一个批次”才把退避复位。
///
/// 为什么不是单个 `saw_runtime_evidence`：两者正交——`had_running_session`
/// 表示进程起来且配置闭环走通（Start Ack 被接受），`saw_runtime_evidence`
/// 表示数据面真的通过。缺前者说明没起来，缺后者说明起来了但一个批次都没有
/// （假 RUNNING / 慢首采 / 空计划），两种情况都不该快速重连。
fn should_reset_backoff(had_running_session: bool, saw_runtime_evidence: bool) -> bool {
    had_running_session && saw_runtime_evidence
}

/// 单次连接尝试的“运行证据”观测器（attempt 局部，不跨 attempt / Endpoint）。
///
/// 语义：本次 attempt 的消费路径（数据面 `event_loop` / 事件面 ingress）至少从
/// Driver 收到过 **一个** 批次；质量、有效性、重复、缺口一律无关。
/// 生命周期：每次 `attempt_session` 新建、随 attempt 丢弃 —— “每次 attempt 开始时
/// 重置”因此是结构性保证，而不是需要记得清零的字段。
/// 读取时机：必须等 `teardown_attempt` 返回（`shutdown_ingress` 已 Join 或
/// abort+await）之后。`Relaxed` 的安全性来自这次任务终止的 happens-before，
/// 而不是原子序本身。
struct AttemptEvidence(Arc<AtomicBool>);

impl AttemptEvidence {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
    /// 交给本次 attempt 的 ingress 任务持有的句柄（跨任务，故用 Arc）。
    fn handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
    fn mark(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    fn saw(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// 内置端点配置：空库演示时由 Mesad 硬编码注入，当前优先 ConfigStore 构造；硬编码仅用于空库演示
#[derive(Debug, Clone)]
pub struct BuiltinEndpoint {
    pub endpoint_id: String,
    pub driver_id: String,
    /// Endpoint.connection 的 JSON 序列化，语义由 Driver 解释。
    pub connection_json: String,
    pub tasks: Vec<mesa_core_types::AcquisitionTask>,
    /// 事件订阅快照（PR7 v1.1 §11）：空 = 无事件订阅，老路径零成本。
    pub event_tasks: Vec<EventTask>,
}

// ---------------------------------------------------------------------------
// PointIdSource 抽象
// ---------------------------------------------------------------------------

/// Point ID 分配与 revision 供给的抽象。Core 负责稳定分配，Driver 只透传 id。
pub trait PointIdSource: Send + Sync {
    /// 为一批 descriptor 分配稳定 id（按序返回）。实现负责持久化与墓碑复用。
    fn assign(
        &self,
        endpoint_id: &str,
        descriptors: &[PointDescriptor],
    ) -> Result<Vec<u32>, String>;
    /// 已知映射（重启恢复时预填）。
    fn known_map(&self, endpoint_id: &str) -> HashMap<String, u32>;
    /// 当前 revision（全量快照版本号），用于 Configure/Apply 的原子性标记。
    fn revision(&self, endpoint_id: &str) -> u64;
    /// P2：已持久化的用户展示名（point_key -> display_name，未设置的不出现）。
    /// 默认空（内存版无持久化）；持久版从 registry 回填。configure 流程
    /// 用它把保留下来的命名合并进 PointDefinition，不经过 Driver。
    fn display_names(&self, _endpoint_id: &str) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// 内存版分配器：进程内稳定，同 key 复用既有 id。用于 Contract Test 与无库场景。
#[derive(Default)]
pub struct PointIdAllocator {
    next: AtomicU64,
    /// endpoint_id -> (point_key -> point_id)
    maps: Mutex<HashMap<String, HashMap<String, u32>>>,
    /// endpoint_id -> revision（内存版恒为 1，满足测试对 revision 的最小需求）
    revisions: Mutex<HashMap<String, u64>>,
}

impl PointIdAllocator {
    /// 兼容旧调用点的辅助：直接对 keys 分配（不经过 descriptor 校验）。
    /// 保留仅为向后兼容，新代码应使用 [`PointIdSource::assign`]。
    #[allow(dead_code)]
    fn assign_keys(&self, used: &mut HashMap<String, u32>, keys: &[String]) -> Vec<u32> {
        keys.iter()
            .map(|k| {
                *used
                    .entry(k.clone())
                    .or_insert_with(|| self.next.fetch_add(1, Ordering::Relaxed) as u32 + 1)
            })
            .collect()
    }
}

impl PointIdSource for PointIdAllocator {
    fn assign(
        &self,
        endpoint_id: &str,
        descriptors: &[PointDescriptor],
    ) -> Result<Vec<u32>, String> {
        ensure_unique_point_keys(descriptors).map_err(|e| e.to_string())?;
        let mut maps = self.maps.lock().unwrap();
        let map = maps.entry(endpoint_id.to_string()).or_default();
        let mut out = Vec::with_capacity(descriptors.len());
        for d in descriptors {
            let id = *map
                .entry(d.point_key.clone())
                .or_insert_with(|| self.next.fetch_add(1, Ordering::Relaxed) as u32 + 1);
            out.push(id);
        }
        // 内存版 revision：首次 assign 后记为 1，后续保持
        {
            let mut revs = self.revisions.lock().unwrap();
            revs.entry(endpoint_id.to_string()).or_insert(1);
        }
        Ok(out)
    }

    fn known_map(&self, endpoint_id: &str) -> HashMap<String, u32> {
        self.maps
            .lock()
            .unwrap()
            .get(endpoint_id)
            .cloned()
            .unwrap_or_default()
    }

    fn revision(&self, endpoint_id: &str) -> u64 {
        *self
            .revisions
            .lock()
            .unwrap()
            .get(endpoint_id)
            .unwrap_or(&1)
    }
}

/// 持久版分配器：委托 `ConfigStore`（含 tombstone 语义）。
pub struct StorePointIdSource {
    store: Arc<mesa_config_store::ConfigStore>,
}

impl StorePointIdSource {
    pub fn new(store: Arc<mesa_config_store::ConfigStore>) -> Self {
        Self { store }
    }
}

impl PointIdSource for StorePointIdSource {
    fn assign(
        &self,
        endpoint_id: &str,
        descriptors: &[PointDescriptor],
    ) -> Result<Vec<u32>, String> {
        let defs = self
            .store
            .assign_point_ids(endpoint_id, descriptors)
            .map_err(|e| e.to_string())?;
        // 保持输入顺序返回
        let by_key: HashMap<&str, u32> = defs
            .iter()
            .map(|d| (d.point_key.as_str(), d.point_id))
            .collect();
        Ok(descriptors
            .iter()
            .map(|d| by_key[d.point_key.as_str()])
            .collect())
    }

    fn known_map(&self, endpoint_id: &str) -> HashMap<String, u32> {
        self.store.point_map(endpoint_id).unwrap_or_default()
    }

    fn revision(&self, endpoint_id: &str) -> u64 {
        self.store.current_revision(endpoint_id).unwrap_or(0).max(1)
    }

    fn display_names(&self, endpoint_id: &str) -> HashMap<String, String> {
        self.store
            .point_registry_all(endpoint_id)
            .map(|rows| {
                rows.into_iter()
                    .filter(|(_, _, _, deleted, _, name)| !deleted && name.is_some())
                    .map(|(k, _, _, _, _, name)| (k, name.unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// 运行时
// ---------------------------------------------------------------------------

/// 单个 Endpoint 的运行任务。返回即表示该 Endpoint 已停止且不再重试。
///
/// 返回值（P0-3）：teardown 结论。`None` = 干净；`Some(desc)` = 收尾异常
/// （drain fatal/timeout 等精确描述），`stop_endpoint` 转为显式错误。
/// `events` 为 `None` 时为纯 Data-only 路径（EventStore 不可用或未配置时）：
/// 不接管 EventReceiver、不起 ingress，数据面完全不受影响（v1.1 §9 隔离）。
///
/// 心跳语义：本函数使用默认心跳参数（`HeartbeatParams::default()`，仍保留
/// `MESA_HEARTBEAT_FAST=1` 显式覆盖能力）；需要显式心跳的调用方请用
/// [`run_endpoint_with_heartbeat`]。旧签名保持不变，非 breaking。
#[allow(clippy::too_many_arguments)]
pub async fn run_endpoint(
    disc: DiscoveredDriver,
    cfg: BuiltinEndpoint,
    snapshot: Arc<Snapshot>,
    source: Arc<dyn PointIdSource>,
    shutdown: CancellationToken,
    registry: std::sync::Arc<
        std::sync::RwLock<
            std::collections::HashMap<
                String,
                std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
            >,
        >,
    >,
    events: Option<Arc<EventServices>>,
) -> Option<String> {
    run_endpoint_with_heartbeat(
        disc,
        cfg,
        snapshot,
        source,
        shutdown,
        registry,
        events,
        HeartbeatParams::default(),
    )
    .await
}

/// 单个 Endpoint 的运行任务（显式心跳版）。返回即表示该 Endpoint 已停止且不再重试。
///
/// 返回值（P0-3）：teardown 结论。`None` = 干净；`Some(desc)` = 收尾异常
/// （drain fatal/timeout 等精确描述），`stop_endpoint` 转为显式错误。
/// `heartbeat` 为本次运行的会话心跳参数（调用方显式给出；测试按需传 fast，
/// 不再经进程 env 隐式决定）。
///
/// G7 NOTE：本函数**绕过** Manager 的 attempt 表（测试夹具直调路径），因此
/// 不走代次化快照门（generation=0）、也不参与"停干净"判据。生产唯一入口是
/// [`crate::MesaManager::start_endpoint`]（走 [`run_endpoint_with_attempt`]）。
#[allow(clippy::too_many_arguments)]
pub async fn run_endpoint_with_heartbeat(
    disc: DiscoveredDriver,
    cfg: BuiltinEndpoint,
    snapshot: Arc<Snapshot>,
    source: Arc<dyn PointIdSource>,
    shutdown: CancellationToken,
    registry: std::sync::Arc<
        std::sync::RwLock<
            std::collections::HashMap<
                String,
                std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
            >,
        >,
    >,
    events: Option<Arc<EventServices>>,
    heartbeat: HeartbeatParams,
) -> Option<String> {
    run_endpoint_with_attempt(
        disc, cfg, snapshot, source, shutdown, registry, events, heartbeat, None,
    )
    .await
}

/// 带 attempt 上下文的运行任务（G7 生产路径）。
///
/// `ctx` 是本次 attempt 的**唯一可判定证据载体**：代次（快照写入过滤）、
/// pid（残留进程复核）、session_released / remote_eof（收敛证据）、
/// outcome（单次结论）。`None` 仅供旧直调路径使用（generation=0，永不覆盖正式 attempt）。
#[allow(clippy::too_many_arguments)]
pub async fn run_endpoint_with_attempt(
    disc: DiscoveredDriver,
    cfg: BuiltinEndpoint,
    snapshot: Arc<Snapshot>,
    source: Arc<dyn PointIdSource>,
    shutdown: CancellationToken,
    registry: std::sync::Arc<
        std::sync::RwLock<
            std::collections::HashMap<
                String,
                std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
            >,
        >,
    >,
    events: Option<Arc<EventServices>>,
    heartbeat: HeartbeatParams,
    ctx: Option<Arc<crate::manager::AttemptCtx>>,
) -> Option<String> {
    let generation = ctx.as_ref().map_or(0, |c| c.generation);
    let mut backoff_idx = 0usize;
    let mut id_map: HashMap<String, u32> = source.known_map(&cfg.endpoint_id);
    let mut last_epoch: u64 = 0;

    set_status(
        &snapshot,
        &cfg,
        generation,
        ConnectionState::Connecting,
        "",
        id_map.len(),
        last_epoch,
        source.revision(&cfg.endpoint_id),
    );

    loop {
        if shutdown.is_cancelled() {
            set_status(
                &snapshot,
                &cfg,
                generation,
                ConnectionState::Stopped,
                "stopped",
                id_map.len(),
                last_epoch,
                source.revision(&cfg.endpoint_id),
            );
            return None;
        }

        let (outcome, drain_err, saw_runtime_evidence) = attempt_session(
            &disc,
            &cfg,
            &snapshot,
            &source,
            &mut id_map,
            &mut last_epoch,
            &shutdown,
            &registry,
            events.as_ref(),
            heartbeat,
            ctx.as_ref(),
        )
        .await;
        match outcome {
            AttemptOutcome::Shutdown => {
                set_status(
                    &snapshot,
                    &cfg,
                    generation,
                    ConnectionState::Stopped,
                    "stopped",
                    id_map.len(),
                    last_epoch,
                    source.revision(&cfg.endpoint_id),
                );
                // P0-3：teardown 结论随任务返回，stop_endpoint 转显式错误
                return drain_err.map(|e| e.to_string());
            }
            AttemptOutcome::ConfigurationFailed(detail) => {
                tracing::error!(endpoint = %cfg.endpoint_id, %detail, "configuration error, no retry");
                if let Some(e) = &drain_err {
                    tracing::error!(endpoint = %cfg.endpoint_id, %e, "teardown drain failed");
                }
                set_status(
                    &snapshot,
                    &cfg,
                    generation,
                    ConnectionState::Failed,
                    &detail,
                    id_map.len(),
                    last_epoch,
                    source.revision(&cfg.endpoint_id),
                );
                return drain_err.map(|e| e.to_string());
            }
            AttemptOutcome::Lost {
                reason,
                had_running_session,
            } => {
                // P0-3：Lost 路径（重连继续）drain 失败只大声记录——没有
                // Stop 调用方可以接收它；abort 丢弃的 backlog 由重连后的
                // 新 epoch 继续（旧 epoch 行已 COMMIT 的不受影响）。
                if let Some(e) = &drain_err {
                    tracing::error!(endpoint = %cfg.endpoint_id, %reason, %e, "teardown drain failed, reconnecting");
                }
                tracing::warn!(
                    endpoint = %cfg.endpoint_id,
                    %reason,
                    had_running_session,
                    saw_runtime_evidence,
                    "connection lost"
                );
                snapshot.mark_communication_lost(&cfg.endpoint_id);
                set_status(
                    &snapshot,
                    &cfg,
                    generation,
                    ConnectionState::Reconnecting,
                    &reason,
                    id_map.len(),
                    last_epoch,
                    source.revision(&cfg.endpoint_id),
                );
                // A1：Start Ack 只证明“驱动接受了 Start”，不证明数据面通过。
                // 只有本次 attempt 真的收到过驱动上行批次（Data 或 Event），才归零
                // 重新从 1s 起步；否则按 1/2/5/10/30 指数退避（抑制假 RUNNING 风暴）。
                if should_reset_backoff(had_running_session, saw_runtime_evidence) {
                    backoff_idx = 0;
                }
            }
        }

        // 未曾 Running 的连续失败则指数退避
        let delay = Duration::from_secs(reconnect_backoff_secs(backoff_idx));
        if backoff_idx < 10 {
            backoff_idx += 1;
        }
        tracing::info!(endpoint = %cfg.endpoint_id, retry_in_secs = delay.as_secs(), "reconnecting");
        let wait = async {
            // 仅测试可以显式推进本 Endpoint 的等待；计划值、递增与重置仍走生产逻辑。
            #[cfg(feature = "test-hooks")]
            if let Some(clock) = ctx.as_ref().and_then(|c| c.test_reconnect_clock.get()) {
                clock.wait(delay).await;
                return;
            }
            tokio::time::sleep(delay).await;
        };
        tokio::select! {
            _ = wait => {}
            _ = shutdown.cancelled() => {
                set_status(&snapshot, &cfg, generation, ConnectionState::Stopped, "core shutdown", id_map.len(), last_epoch, source.revision(&cfg.endpoint_id));
                return None;
            }
        }
    }
}

enum AttemptOutcome {
    Shutdown,
    ConfigurationFailed(String),
    Lost {
        reason: String,
        had_running_session: bool,
    },
}

/// 写入运行态快照（带代次门，G7 §6.5）：`generation` 由 attempt 上下文给出，
/// 直调路径为 0（永不覆盖正式 attempt，也永不复活已删端点）。
#[allow(clippy::too_many_arguments)]
fn set_status(
    snapshot: &Snapshot,
    cfg: &BuiltinEndpoint,
    generation: u64,
    state: ConnectionState,
    detail: &str,
    points: usize,
    epoch: u64,
    revision: u64,
) {
    let written = snapshot.upsert_endpoint(
        generation,
        EndpointStatus {
            endpoint_id: cfg.endpoint_id.clone(),
            driver_id: cfg.driver_id.clone(),
            state: state.as_str().to_string(),
            detail: detail.to_string(),
            revision,
            points,
            epoch,
        },
    );
    if !written {
        // 丢弃是设计行为（旧代次/已删除端点），但静默丢弃会掩盖真实缺陷，
        // 因此留一条诊断；不得在此重试或改写。
        tracing::debug!(
            endpoint = %cfg.endpoint_id,
            generation,
            state = state.as_str(),
            "stale status write dropped by generation/tombstone gate"
        );
    }
}

/// 一次完整连接尝试：成功则阻塞在事件循环直到断开/判死/停机。
/// 返回 `(outcome, drain_err, saw_runtime_evidence)`：
/// - `drain_err`（P0-3）：teardown 的 drain 结论随 attempt 带出（`None` = 无
///   ingress 或干净），由 `run_endpoint` 路由到 stop 显式错误或大声日志；
/// - `saw_runtime_evidence`（P1-A1）：本次 attempt 是否收到过驱动上行批次。
///   **在 teardown 之后读取**——此时 `event_loop` 已返回、ingress 已被
///   `shutdown_ingress` Join（或 abort 后 await），两个写者都已终止，读到的是
///   终值；因此不需要更强的原子序，也不能提前到 `event_loop` 返回处读。
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
async fn attempt_session(
    disc: &DiscoveredDriver,
    cfg: &BuiltinEndpoint,
    snapshot: &Arc<Snapshot>,
    source: &Arc<dyn PointIdSource>,
    id_map: &mut HashMap<String, u32>,
    last_epoch: &mut u64,
    shutdown: &CancellationToken,
    registry: &std::sync::Arc<
        std::sync::RwLock<
            std::collections::HashMap<
                String,
                std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
            >,
        >,
    >,
    services: Option<&Arc<EventServices>>,
    heartbeat: HeartbeatParams,
    ctx: Option<&Arc<crate::manager::AttemptCtx>>,
) -> (AttemptOutcome, Option<EventDrainError>, bool) {
    let generation = ctx.map_or(0, |c| c.generation);
    // P1-A1：attempt 局部证据。先于任何 spawn 创建，只交给本 attempt 的 ingress；
    // 不跨 attempt、不进 Snapshot/EventServices/registry。
    let evidence = AttemptEvidence::new();
    let mut process = match DriverProcess::spawn(disc).await {
        Ok(p) => p,
        Err(e) => {
            return (
                AttemptOutcome::Lost {
                    reason: format!("spawn failed: {e}"),
                    had_running_session: false,
                },
                None,
                evidence.saw(),
            );
        }
    };
    // G7：回填 pid（隔离态复核"残留进程是否已死"的唯一依据）。
    if let Some(c) = ctx {
        c.pid
            .store(process.pid, std::sync::atomic::Ordering::Release);
    }

    let (mut session, mut events, unresponsive_flag) = match Session::connect_retry_with_heartbeat(
        process.port,
        &process.token,
        heartbeat,
    )
    .await
    {
        Ok((s, ev, flag)) => (s, ev, flag),
        Err(e) => {
            process.terminate().await;
            return (
                AttemptOutcome::Lost {
                    reason: format!("connect failed: {e}"),
                    had_running_session: false,
                },
                None,
                evidence.saw(),
            );
        }
    };
    // v1.1 §14 + P1：Event-enabled（services + 非空 event_tasks）才接管。
    // Data-only（无 services，或 tasks 为空）不 take、不 spawn——接收端随
    // Session 释放，零成本（reader 侧溢出无人消费也无人在意）。
    let want_events = services.is_some() && !cfg.event_tasks.is_empty();
    let event_rx = if want_events {
        session.take_event_batches()
    } else {
        None
    };
    // P1：ingress 在 run_config_flow（→ Start）之前 spawn。StartAck 后首批
    // 即可能到达；若等 Start 成功后再 spawn，洪峰会在消费者就位前先塞 channel。
    // （PR6 release gate 保证 Start 前无 batch，但消费者早到位消除整类窗口。）
    // P1-1：运行质量计数由 ingress 直写 `services.diagnostics`（全局 live），
    // 无 per-attempt 本地中转（黑洞已除）。
    // ingress 优雅取消令牌（Checkpoint B 方案 A）：独立 token，只在
    // shutdown_ingress() 里 cancel。不能用 child_token——父 shutdown cancel
    // 会抢先触发，与 event_loop 的正常退出竞争，把"优雅停机"误判成 Lost。
    let ingress_shutdown = CancellationToken::new();
    let mut ingress: Option<tokio::task::JoinHandle<Result<(), IngressFatal>>> =
        match (event_rx, services) {
            (Some(rx), Some(svc)) => Some(tokio::spawn(run_event_ingress(
                rx,
                cfg.endpoint_id.clone(),
                Arc::clone(svc),
                ingress_shutdown.clone(),
                // P1-A1：事件面证据句柄（本 attempt 私有）
                Some(evidence.handle()),
            ))),
            _ => None,
        };
    // 注册活跃会话供 Control 面可靠转发（§22），Control 与 Data 共用同一 TCP 但分队列
    let session_arc = std::sync::Arc::new(tokio::sync::Mutex::new(session));
    registry
        .write()
        .unwrap()
        .insert(cfg.endpoint_id.clone(), std::sync::Arc::clone(&session_arc));

    // run_config_flow 需经 Mutex 单次加锁，避免跨 await 持锁导致 !Send
    let config_res = {
        let sess = session_arc.lock().await;
        #[allow(clippy::explicit_auto_deref)]
        {
            run_config_flow(&sess, cfg, source, id_map, snapshot, last_epoch).await
        }
    };
    match config_res {
        Ok(()) => {}
        Err(outcome) => {
            // 配置失败走统一 teardown（producer-stop → terminate → reader
            // barrier → drain，与正常 Stop 同一原语；drain 结论一并带出，
            // P0-3，次级错误不覆盖主 outcome）。
            let drain_err = teardown_attempt(
                &cfg.endpoint_id,
                &session_arc,
                &mut process,
                ingress_shutdown,
                ingress,
                events,
                std::sync::Arc::clone(registry),
                ctx,
            )
            .await
            .0;
            return (outcome, drain_err, evidence.saw());
        }
    }

    // G7：进入 RUNNING（快照写入 + phase 迁移在同一处，避免两处真值分叉）。
    set_status(
        snapshot,
        cfg,
        generation,
        ConnectionState::Running,
        "",
        id_map.len(),
        *last_epoch,
        source.revision(&cfg.endpoint_id),
    );
    if let Some(c) = ctx {
        c.set_phase(crate::manager::AttemptPhase::Running);
    }

    // 事件循环期间保持会话注册。Control 与 event_loop 并发执行：
    // control_write/command 经 session 内 writer 锁 + pending 表串行，
    // 不与 event_loop 抢同一 Core 侧 Mutex（此前 session_arc 全局锁是
    // Control RUNNING 超时的根因——event_loop 持锁期间 write 排队 10s）。
    let outcome = event_loop(
        cfg,
        snapshot,
        unresponsive_flag.as_ref(),
        &mut events,
        &mut ingress,
        shutdown,
        &evidence,
    )
    .await;

    // ⑨ Stop barrier（真 barrier，不是 sleep，见 `teardown_attempt`）：
    // Shutdown-post → terminate_after_shutdown → 等 reader 结束 → 再 drain
    // ingress。P0-2 Final：terminate_after_shutdown 在 Phase 1 保持 stdin
    // OPEN——SDK 侧 Shutdown RPC → run tasks 停 → writer drain → TCP FIN →
    // 自然退出；liveness guard 不抢跑。reader 把 FIN 之前全部字节 pump 进
    // channel 后结束（TCP 有序性保证）。reader 是事件 channel 的唯一生产者
    // ⇒ reader 结束时"已进入 Core 的旧 epoch Event"已全部在 channel 里；
    // 此时再 cancel ingress 做 final drain，零窗口（旧顺序 cancel-first 下，
    // drain 与 driver 停产之间在途批次会落入无人消费的 channel 而静默丢失）。
    // Phase 1 超时（顽固对端）→ emergency EOF → reader 5s 未结束 →
    // 超时降级 drain 现有（等价旧行为）。50ms sleep 已删除：等的对象是
    // "reader 结束"这个条件，不是时长。
    // P0-3 teardown 总预算（全有界，Manager 直接 await，无外层超时）：
    // post 5s + terminate 10s + reader barrier 5s +
    // ingress drain 15s（可组合预算，见 `INGRESS_DRAIN_TIMEOUT`）≈ 35s
    // worst-case，正常路径百 ms 内。超时/失败经 drain_err 显式上报。
    let drain_err = teardown_attempt(
        &cfg.endpoint_id,
        &session_arc,
        &mut process,
        ingress_shutdown,
        ingress,
        events,
        std::sync::Arc::clone(registry),
        ctx,
    )
    .await
    .0;
    // 读点固定在 teardown 之后：event_loop 已返回、ingress 已 Join/abort+await，
    // 两个写者都终止，读到的是终值（见函数文档）。
    (outcome, drain_err, evidence.saw())
}

fn pb_shutdown_body() -> pb::envelope::Body {
    pb::envelope::Body::Shutdown(pb::Shutdown {})
}

/// ingress drain 外层预算（可组合推导，非魔法数字）：
/// 必须严格覆盖 writer 线程内最坏情况（单次 in-flight commit 的 busy 上限
/// [`mesa_event_store::EVENT_STORE_BUSY_TIMEOUT_MS`]），再加密封 backlog
/// 排空 + 调度余量。外层 timer 若与内层最坏同长，正常偏慢的成功路径会被
/// 误判为 `EVENT_DRAIN_TIMEOUT`（main CI #103 实证：在途 commit + Windows
/// 调度抖动 idle 耗尽 5s，timer 先赢）。超时分支保留为真磁盘 hang 的
/// 大声失败兜底，不代表成功。
const INGRESS_DRAIN_TIMEOUT: Duration = Duration::from_millis(
    mesa_event_store::EVENT_STORE_BUSY_TIMEOUT_MS as u64 + INGRESS_DRAIN_MARGIN_MS,
);
/// 余量：reader barrier 达成后 backlog 有限（生产者已停），覆盖其排空 +
/// Tokio/Windows 调度抖动。病态 backlog（通道上限 512 批全慢）超出本预算时
/// 走大声 `EVENT_DRAIN_TIMEOUT`，不静默——预算覆盖现实最坏，不覆盖病态。
const INGRESS_DRAIN_MARGIN_MS: u64 = 10_000;

/// producer-stop 请求有界等待：Shutdown post 只是停产请求（best-effort），
/// 超时则直接进入 terminate（terminate 兜底一切：grace → emergency EOF →
/// 强杀）；post 自身不得成为 teardown 链中的无界 await（此前形式上无界，
/// `terminate_after_shutdown` 之前存在无限等待窗口）。
const SHUTDOWN_POST_TIMEOUT: Duration = Duration::from_secs(5);

/// 统一 teardown 原语（producer-stop → bounded terminate → reader barrier → drain → cleanup）。
///
/// 所有 attempt 退出路径（正常 Stop / Lost 重连 / 配置失败 / heartbeat 判死）
/// 共用同一顺序；区别仅在于主 outcome 如何上报，drain 次级结论随返回值带出
/// （不覆盖主 outcome）。此前配置失败路径是"drain 先于 producer stop"的第二套
/// 顺序，已收敛到此：producer 未停时 drain 面对的不是密封输入，排空语义不成立。
///
/// 顺序契约（对应 `reader_done → 无新生产 → in-flight 完成 → 队列排空 → drain ACK`）：
/// 1. 能响应则 post Shutdown（生产者停产；unresponsive 时跳过；
///    post 本身 [`SHUTDOWN_POST_TIMEOUT`] 有界，超时直接进入 terminate）；
/// 2. `terminate_after_shutdown`（graceful 5s + emergency 后 terminate 5s）；
/// 3. `wait_reader_done` barrier 5s（超时降级：drain 已到达者）；
/// 4. `shutdown_ingress`（in-flight commit 完成 + 密封 backlog 排空 + Hub 发布）；
/// 5. session 失效 + 注册表摘除 + 事件接收端释放。
///
/// 总预算严格有界 ≈ post 5 + terminate 10 + reader 5 + drain 15 ≈ 35s 最坏，
/// 正常路径百 ms 内；超时/失败经返回值显式上报。
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
async fn teardown_attempt(
    endpoint_id: &str,
    session_arc: &Arc<tokio::sync::Mutex<Session>>,
    process: &mut DriverProcess,
    ingress_shutdown: CancellationToken,
    ingress: Option<tokio::task::JoinHandle<Result<(), IngressFatal>>>,
    events: tokio::sync::mpsc::Receiver<SessionEvent>,
    registry: Arc<std::sync::RwLock<HashMap<String, Arc<tokio::sync::Mutex<Session>>>>>,
    ctx: Option<&Arc<crate::manager::AttemptCtx>>,
) -> (Option<EventDrainError>, bool) {
    // 测试专用 panic 注入点（`test-hooks`）：位置在"第 2 步 terminate"之前，
    // 即子进程尚未回收、会话尚未摘除、ingress 尚未排空的时刻——这正是
    // "Drop 时未证明收敛"的真实形态，用来验证隔离态 fail-closed（G7 §9-5）。
    #[cfg(feature = "test-hooks")]
    if ctx.is_some_and(|c| {
        c.test_panic_before_teardown
            .load(std::sync::atomic::Ordering::Relaxed)
    }) {
        panic!("injected teardown panic before process termination (test-hooks)");
    }
    {
        let sess = session_arc.lock().await;
        if !sess.is_unresponsive() {
            // Best-effort 停产请求：有界等待，超时/失败都直接进入 terminate。
            let _ =
                tokio::time::timeout(SHUTDOWN_POST_TIMEOUT, sess.post(pb_shutdown_body())).await;
        }
    }
    process.terminate_after_shutdown().await;
    let mut remote_eof = false;
    {
        let sess = session_arc.lock().await;
        if !sess.wait_reader_done(Duration::from_secs(5)).await {
            tracing::warn!(
                endpoint = %endpoint_id,
                "reader did not finish after terminate, draining what arrived"
            );
        } else {
            remote_eof = true;
        }
    }
    let drain_err = shutdown_ingress(ingress_shutdown, ingress).await.err();
    {
        let mut sess = session_arc.lock().await;
        sess.invalidate();
    }
    registry.write().unwrap().remove(endpoint_id);
    if let Some(c) = ctx {
        c.remote_eof
            .store(remote_eof, std::sync::atomic::Ordering::Release);
        c.session_released
            .store(true, std::sync::atomic::Ordering::Release);
    }
    drop(events);
    (drain_err, remote_eof)
}

/// ingress 优雅停机（Checkpoint B 方案 A + ⑨ final drain + P0-3）：
/// 先 cancel（当前 commit+publish 完整执行 + final drain 排空），
/// [`INGRESS_DRAIN_TIMEOUT`] 内未退出才 abort（磁盘 hang 等极端情况）。
///
/// P0-3：返回显式结果，禁止吞错——drain 中的精确失败（collision/unavailable/
/// regression）与超时（abort 时 backlog 可能根本未 COMMIT，既无 DB 行
/// 也无 Hub 行）都必须向上传播，由 `stop_endpoint` 转为显式错误；
/// "Stop 返回成功但后台仍在跑"（detach-success）在此终结。
///
/// 残余诚实声明：abort 不能撤回已送达 writer 线程的 commit（命令一旦发送，
/// writer 侧仍会执行并 COMMIT，只是 reply 无人接收、Hub 不再发布）。
/// 因此超时后可能出现"DB 有行而 Hub 未发布"的缺口——本函数以显式
/// `EVENT_DRAIN_TIMEOUT` 大声报告，不伪装成功；调用方不得把该错误当成功吞掉。
async fn shutdown_ingress(
    cancel: CancellationToken,
    ingress: Option<tokio::task::JoinHandle<Result<(), IngressFatal>>>,
) -> Result<(), EventDrainError> {
    let Some(mut h) = ingress else {
        return Ok(());
    };
    cancel.cancel();
    // NOTE: 不能用 timeout(h)——超时会 drop JoinHandle 使任务 detach 继续跑。
    // select 保留所有权，超时后显式 abort。
    tokio::select! {
        r = &mut h => match r {
            Ok(Ok(())) => Ok(()),
            Ok(Err(fatal)) => Err(EventDrainError::Fatal(fatal)),
            Err(join) => Err(EventDrainError::Join(join.to_string())),
        },
        _ = tokio::time::sleep(INGRESS_DRAIN_TIMEOUT) => {
            tracing::error!(
                "event ingress did not exit in {:?}, aborting",
                INGRESS_DRAIN_TIMEOUT
            );
            h.abort();
            let _ = h.await;
            Err(EventDrainError::Timeout)
        }
    }
}

/// 配置闭环（§6.2）：Open -> Configure -> PointDescriptors -> ApplyPointMap -> Start。
async fn run_config_flow(
    session: &Session,
    cfg: &BuiltinEndpoint,
    source: &Arc<dyn PointIdSource>,
    id_map: &mut HashMap<String, u32>,
    snapshot: &Arc<Snapshot>,
    last_epoch: &mut u64,
) -> Result<(), AttemptOutcome> {
    use pb::envelope::Body;

    const HANDLE: u32 = 1;

    // OpenConnection
    let reply = session
        .call(Body::OpenConnection(pb::OpenConnection {
            connection_handle: HANDLE,
            endpoint_id: cfg.endpoint_id.clone(),
            config_json: cfg.connection_json.clone(),
        }))
        .await
        .map_err(|e| AttemptOutcome::Lost {
            reason: format!("open rpc: {e}"),
            had_running_session: false,
        })?;
    let result = expect_ack(reply.body).ok_or_else(|| lost("OpenConnection"))?;
    config_gate(result, "OpenConnection")?;

    // ConfigureTasks：revision 来自持久源
    let revision = source.revision(&cfg.endpoint_id);
    // Foundation-2 wire 门控（IPC 1.5）：Subscribe 任务要求协商 Minor >=
    // TASK_SCHEDULE_MIN_MINOR；旧端不识别四字段，静默降级即改变用户语义，
    // 故 fail-closed（ConfigurationFailed，不重试）。
    if cfg
        .tasks
        .iter()
        .any(|t| t.mode() == mesa_core_types::TaskMode::Subscribe)
        && session.negotiated_minor() < mesa_driver_protocol::TASK_SCHEDULE_MIN_MINOR
    {
        return Err(AttemptOutcome::ConfigurationFailed(format!(
            "TASK_SCHEDULE_UNSUPPORTED: negotiated minor {} < {} (Subscribe schedule 需要 IPC 1.5)",
            session.negotiated_minor(),
            mesa_driver_protocol::TASK_SCHEDULE_MIN_MINOR,
        )));
    }
    let tasks_pb = tasks_to_pb_checked(cfg)?;
    let reply = session
        .call(Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: HANDLE,
            revision,
            tasks: tasks_pb,
        }))
        .await
        .map_err(|e| AttemptOutcome::Lost {
            reason: format!("configure rpc: {e}"),
            had_running_session: false,
        })?;
    let descriptors_pb = match reply.body {
        Some(Body::PointDescriptors(rep)) => rep.descriptors,
        Some(Body::DriverError(err)) => {
            let d = err.detail.unwrap_or_default();
            return Err(config_fail(d.kind, d.code, d.message));
        }
        other => return Err(lost(format!("unexpected configure reply: {other:?}"))),
    };

    let parsed: Vec<PointDescriptor> = descriptors_pb
        .into_iter()
        .map(mesa_driver_protocol::descriptor_from_pb)
        .collect::<Result<_, _>>()
        .map_err(|e| AttemptOutcome::ConfigurationFailed(e.to_string()))?;
    ensure_unique_point_keys(&parsed)
        .map_err(|e| AttemptOutcome::ConfigurationFailed(e.to_string()))?;

    // 分配稳定 point_id（持久化或内存）
    let ids = source
        .assign(&cfg.endpoint_id, &parsed)
        .map_err(AttemptOutcome::ConfigurationFailed)?;
    // 同步本地 id_map（用于状态展示与断线前计数）
    for (d, &id) in parsed.iter().zip(ids.iter()) {
        id_map.insert(d.point_key.clone(), id);
    }
    // P2：回填 registry 保留的用户展示名（assign 内部 upsert 不覆盖它）。
    let names = source.display_names(&cfg.endpoint_id);
    let defs: Vec<PointDefinition> = parsed
        .iter()
        .zip(&ids)
        .map(|(d, &id)| PointDefinition {
            point_id: id,
            point_key: d.point_key.clone(),
            data_type: d.data_type,
            unit: d.unit.clone(),
            source_label: d.source_label.clone(),
            // P2：configure 不产生展示名；registry 里保留的用户命名
            // 在此处回填（assign 内部 upsert 已保留旧值）。
            display_name: names.get(d.point_key.as_str()).cloned(),
        })
        .collect();
    snapshot.register_points(&cfg.endpoint_id, &defs);

    // ApplyPointMap
    let map: HashMap<String, u32> = defs
        .iter()
        .map(|d| (d.point_key.clone(), d.point_id))
        .collect();
    let reply = session
        .call(Body::ApplyPointMap(pb::ApplyPointMap {
            connection_handle: HANDLE,
            revision,
            key_to_point_id: map,
        }))
        .await
        .map_err(|e| AttemptOutcome::Lost {
            reason: format!("apply rpc: {e}"),
            had_running_session: false,
        })?;
    let result = expect_ack(reply.body).ok_or_else(|| lost("ApplyPointMap"))?;
    config_gate(result, "ApplyPointMap")?;

    // ConfigureEventTasks（PR7 v1.1 §11）：Apply 之后、Start 之前。
    // 空任务零成本（Session 内直接成功，不发 RPC，老 Driver 路径无变化）；
    // 失败则不 Start——同一 revision 半更新（Data 新 + Event 旧）永不存在。
    if let Err(e) = session
        .configure_events(HANDLE, revision, &cfg.event_tasks)
        .await
    {
        return Err(event_config_fail(e));
    }

    // StartConnection(new stream_epoch)
    let epoch = new_stream_epoch();
    let reply = session
        .call(Body::StartConnection(pb::StartConnection {
            connection_handle: HANDLE,
            stream_epoch: epoch,
        }))
        .await
        .map_err(|e| AttemptOutcome::Lost {
            reason: format!("start rpc: {e}"),
            had_running_session: false,
        })?;
    let result = expect_ack(reply.body).ok_or_else(|| lost("StartConnection"))?;
    config_gate(result, "StartConnection")?;
    *last_epoch = epoch;
    tracing::info!(endpoint = %cfg.endpoint_id, epoch, points = defs.len(), "connection started");

    Ok(())
}

async fn event_loop(
    cfg: &BuiltinEndpoint,
    snapshot: &Arc<Snapshot>,
    unresponsive_flag: &std::sync::atomic::AtomicBool,
    events: &mut tokio::sync::mpsc::Receiver<SessionEvent>,
    ingress: &mut Option<tokio::task::JoinHandle<Result<(), IngressFatal>>>,
    shutdown: &CancellationToken,
    // P1-A1：数据面运行证据（与事件面共用同一个 attempt 局部布尔）。
    // 这里只写不读；读取发生在 `attempt_session` 的 teardown 之后。
    evidence: &AttemptEvidence,
) -> AttemptOutcome {
    const WATCHDOG_TICK: Duration = Duration::from_secs(2);
    let mut watchdog = tokio::time::interval(WATCHDOG_TICK);
    watchdog.tick().await;

    loop {
        tokio::select! {
            // EventIngress fatal（v1.1 §4）：只杀本 attempt，走 Lost 重连；
            // Data-only（ingress=None）时本分支挂起永不就绪。
            // NOTE: 不加 `if ingress.is_some()` guard——future 已持有 &mut 借用，
            // guard 的二次借用编译不过；pending 分支语义等价。
            res = async {
                match ingress {
                    Some(h) => h.await,
                    None => {
                        std::future::pending::<
                            Result<Result<(), IngressFatal>, tokio::task::JoinError>,
                        >()
                        .await
                    }
                }
            } => {
                // PR10 fault-injection 暴露：本分支返回意味着 handle 的 output
                // 已被取走；必须 take 出来，否则 teardown 的 shutdown_ingress
                // 复 poll 已完成 handle 会 panic（"JoinHandle polled after
                // completion"），直接杀死 endpoint task 使 Lost 重连失效。
                // 结论已在 res 里（下转为 Lost 原因 + 诊断计数），teardown
                // 收到 None 即无 dead-handle 可 drain，语义完整。
                let _ = ingress.take();
                return match res {
                    Ok(Ok(_stats)) => AttemptOutcome::Lost {
                        // ingress 正常返回理论不可达（无限循环直到 fatal/流关闭）；
                        // 若发生，按 Lost 重连而非静默 Running。
                        reason: "event ingress exited".into(),
                        had_running_session: true,
                    },
                    Ok(Err(fatal)) => AttemptOutcome::Lost {
                        reason: format!("{}: {fatal}", fatal.code()),
                        had_running_session: true,
                    },
                    Err(join_err) => AttemptOutcome::Lost {
                        reason: format!("event ingress panicked: {join_err}"),
                        had_running_session: true,
                    },
                };
            }
            ev = events.recv() => match ev {
                Some(SessionEvent::Batch(batch)) => {
                    // P1-A1：收到数据批次即“数据面通过”。质量无关（BAD 也算）。
                    evidence.mark();
                    apply_batch_logged(snapshot, cfg, batch);
                }
                Some(SessionEvent::State { state, detail, .. }) => {
                    tracing::debug!(endpoint=%cfg.endpoint_id, ?state, %detail, "state");
                    if matches!(state, ConnectionState::Stopped | ConnectionState::Failed) {
                        return AttemptOutcome::Lost {
                            reason: format!("driver reported {state:?} {detail}"),
                            had_running_session: true,
                        };
                    }
                }
                Some(SessionEvent::DriverError { kind, code, message, .. }) => {
                    tracing::warn!(endpoint=%cfg.endpoint_id, %kind, %code, %message, "driver error");
                    if kind == "ConfigurationError" || code == "DUPLICATE_POINT_KEY" {
                        return AttemptOutcome::ConfigurationFailed(format!("{kind}/{code}: {message}"));
                    }
                }
                None => {
                    return AttemptOutcome::Lost {
                        reason: "event channel closed".into(),
                        had_running_session: true,
                    }
                }
            },
            _ = watchdog.tick() => {
                if unresponsive_flag.load(Ordering::Relaxed) {
                    return AttemptOutcome::Lost {
                        reason: "heartbeat dead".into(),
                        had_running_session: true,
                    };
                }
            }
            _ = shutdown.cancelled() => return AttemptOutcome::Shutdown,
        }
    }
}

// ---- 小工具 ----

fn tasks_to_pb_checked(
    cfg: &BuiltinEndpoint,
) -> Result<Vec<pb::AcquisitionTaskProto>, AttemptOutcome> {
    mesa_driver_protocol::tasks_to_pb(&cfg.tasks)
        .map_err(|e| AttemptOutcome::ConfigurationFailed(e.to_string()))
}

fn apply_batch_logged(snapshot: &Arc<Snapshot>, cfg: &BuiltinEndpoint, batch: DataBatch) {
    let n = batch.values.len();
    // IPC/E2E 单调延迟：Driver host_mono_ns → Core host_mono_ns 差值，>10s 视为时钟不可比丢弃
    if let Some(mono) = batch.mono_ns {
        let now = mesa_core_types::host_mono_ns();
        if now >= mono {
            let delta = now - mono;
            if delta < 10_000_000_000 {
                snapshot.record_ipc_latency_ns(delta);
            }
        }
    }
    let start = std::time::Instant::now();
    snapshot.apply_batch(&batch, &cfg.endpoint_id);
    let ns = start.elapsed().as_nanos() as u64;
    snapshot.record_snapshot_apply_latency_ns(ns);
    tracing::trace!(endpoint=%cfg.endpoint_id, seq=batch.sequence, values=n, snapshot_apply_latency_ns=ns, "batch");
}

/// 事件配置失败路由："配错了"（老驱动无能力/老 minor/校验拒绝）→
/// ConfigurationFailed（不重试，重试也不会长出能力）；真正的传输失败 →
/// Lost（走 reconnect）。空任务永不走到这里（Session 内直接成功）。
fn event_config_fail(e: crate::session::SessionError) -> AttemptOutcome {
    use crate::session::SessionError;
    match e {
        SessionError::Driver {
            kind,
            code,
            message,
        } => {
            let detail = format!("{kind}/{code}: {message}");
            if kind == "ConfigurationError" || kind == "Unsupported" {
                AttemptOutcome::ConfigurationFailed(detail)
            } else {
                AttemptOutcome::Lost {
                    reason: format!("configure-events: {detail}"),
                    had_running_session: false,
                }
            }
        }
        SessionError::EventPlaneUnsupported { .. } => {
            AttemptOutcome::ConfigurationFailed(format!("configure-events: {e}"))
        }
        other => AttemptOutcome::Lost {
            reason: format!("configure-events rpc: {other}"),
            had_running_session: false,
        },
    }
}

fn expect_ack(body: Option<pb::envelope::Body>) -> Option<pb::GenericResult> {
    use pb::envelope::Body as B;
    match body {
        Some(B::OpenConnectionAck(a)) => a.result,
        Some(B::ConfigApplied(a)) => a.result,
        Some(B::StartConnectionAck(a)) => a.result,
        _ => None,
    }
}

fn config_gate(result: pb::GenericResult, what: &'static str) -> Result<(), AttemptOutcome> {
    if result.ok {
        return Ok(());
    }
    match result.error {
        Some(d) => Err(config_fail(d.kind, d.code, d.message)),
        None => Err(lost(format!("{what}: failed without detail"))),
    }
}

fn config_fail(kind: String, code: String, message: String) -> AttemptOutcome {
    if kind == "ConfigurationError" {
        AttemptOutcome::ConfigurationFailed(format!("{kind}/{code}: {message}"))
    } else {
        AttemptOutcome::Lost {
            reason: format!("{kind}/{code}: {message}"),
            had_running_session: false,
        }
    }
}

fn lost(reason: impl Into<String>) -> AttemptOutcome {
    AttemptOutcome::Lost {
        reason: reason.into(),
        had_running_session: false,
    }
}

static EPOCH_COUNTER: AtomicU64 = AtomicU64::new(0);
fn new_stream_epoch() -> u64 {
    let n = mesa_core_types::now_unix_ns() as u64;
    let pid = std::process::id() as u64;
    let c = EPOCH_COUNTER.fetch_add(1, Ordering::Relaxed);
    n ^ (pid << 32) ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_ingress::IngressFatal;

    /// P1-A1：归零判据必须是“走到事件循环”**且**“见到运行证据”。
    /// 任一单独成立都不足以归零（假 RUNNING / 无证据都不得快速重连）。
    #[test]
    fn reset_requires_runtime_evidence() {
        assert!(!should_reset_backoff(false, false), "没起来没数据");
        assert!(
            !should_reset_backoff(true, false),
            "假 RUNNING：起来了但零批次"
        );
        assert!(
            !should_reset_backoff(false, true),
            "防御性：证据不应独立成立（不变式被破坏时不归零）"
        );
        assert!(should_reset_backoff(true, true), "真运行：有批次");
    }

    /// P1-A1：退避表与 `min(4)` 边界冻结；fast 分支固定 1s。
    #[test]
    fn backoff_sequence_is_frozen_table() {
        let got: Vec<u64> = (0..13).map(|i| backoff_delay(i, false)).collect();
        assert_eq!(got, vec![1, 2, 5, 10, 30, 30, 30, 30, 30, 30, 30, 30, 30]);
        for i in 0..5 {
            assert_eq!(backoff_delay(i, true), 1, "MESA_RECONNECT_FAST 语义不变");
        }
        assert_eq!(backoff_delay(usize::MAX, false), 30, "越界必须夹到 30s");
    }

    /// P1-A1：递增循环（`if backoff_idx < 10 { += 1 }`）与表一致、序列非递减。
    #[test]
    fn backoff_index_progression_matches_table() {
        let mut idx = 0usize;
        let mut planned = Vec::new();
        for _ in 0..12 {
            planned.push(backoff_delay(idx, false));
            if idx < 10 {
                idx += 1;
            }
        }
        assert_eq!(idx, 10);
        assert_eq!(planned, vec![1, 2, 5, 10, 30, 30, 30, 30, 30, 30, 30, 30]);
        assert!(planned.windows(2).all(|w| w[0] <= w[1]));
    }

    /// P0-3：drain 正常 → Ok（None ingress 同样 Ok，Data-only 零成本）。
    #[tokio::test]
    async fn shutdown_ingress_clean_is_ok() {
        assert!(
            shutdown_ingress(CancellationToken::new(), None)
                .await
                .is_ok()
        );
        let h = tokio::spawn(async { Ok::<(), IngressFatal>(()) });
        assert!(
            shutdown_ingress(CancellationToken::new(), Some(h))
                .await
                .is_ok()
        );
    }

    /// P0-3：drain 中精确失败 → 原精确码透出（禁止吞错/泛化）。
    #[tokio::test]
    async fn shutdown_ingress_fatal_propagates_precise_code() {
        let h = tokio::spawn(async {
            Err::<(), IngressFatal>(IngressFatal::StoreUnavailable("disk gone".into()))
        });
        let err = shutdown_ingress(CancellationToken::new(), Some(h))
            .await
            .expect_err("fatal must propagate");
        assert_eq!(err.code(), "EVENT_STORE_UNAVAILABLE");
        assert!(err.to_string().contains("EVENT_STORE_UNAVAILABLE"));

        let h = tokio::spawn(async {
            Err::<(), IngressFatal>(IngressFatal::IdCollision {
                endpoint_id: "e".into(),
                event_id: "x".into(),
            })
        });
        let err = shutdown_ingress(CancellationToken::new(), Some(h))
            .await
            .expect_err("fatal must propagate");
        assert_eq!(err.code(), "EVENT_ID_COLLISION");
    }

    /// P0-3：drain 超时（[`INGRESS_DRAIN_TIMEOUT`]）→ EVENT_DRAIN_TIMEOUT
    ///（abort 已执行，不 detach）。保留真实计时，避免 test-util 改变工作区
    /// 的依赖特征集合，使 bins 与单独运行的合同测试重复构建依赖。
    /// 超时码本身是冻结契约，时长是可组合预算——改预算不改码。
    #[tokio::test]
    async fn shutdown_ingress_timeout_is_explicit_error() {
        struct Dropped(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let witness = Dropped(Arc::clone(&dropped));
        let start = tokio::time::Instant::now();
        let h = tokio::spawn(async move {
            let _witness = witness;
            // 无视 cancel 的顽固任务（模拟磁盘 hang）：pending 永不就绪
            std::future::pending::<Result<(), IngressFatal>>().await
        });
        let err = shutdown_ingress(CancellationToken::new(), Some(h))
            .await
            .expect_err("timeout must be explicit error, not success");
        assert_eq!(err.code(), "EVENT_DRAIN_TIMEOUT");
        assert!(tokio::time::Instant::now() - start >= INGRESS_DRAIN_TIMEOUT);
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }
}
