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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mesa_core_types::{
    ConnectionState, DataBatch, EventTask, PointDefinition, PointDescriptor,
    ensure_unique_point_keys,
};
use mesa_driver_protocol::pb;
use mesa_event_store::EventServices;
use tokio_util::sync::CancellationToken;

use crate::event_ingress::{IngressFatal, IngressStats, run_event_ingress};
use crate::manifest::DiscoveredDriver;
use crate::process::DriverProcess;
use crate::session::{Session, SessionEvent};
use crate::snapshot::{EndpointStatus, Snapshot};

/// 连接级退避序列（§11.1 默认值）。当前不做上限熔断，成功后归零。
/// 测试时 `MESA_RECONNECT_FAST=1` 缩短为 1s 固定，避免 30s/60s 阻塞。
fn reconnect_backoff_secs(idx: usize) -> u64 {
    if std::env::var("MESA_RECONNECT_FAST").ok().as_deref() == Some("1") {
        return 1;
    }
    const RECONNECT_BACKOFF_SECS: [u64; 5] = [1, 2, 5, 10, 30];
    RECONNECT_BACKOFF_SECS[idx.min(RECONNECT_BACKOFF_SECS.len() - 1)]
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
}

// ---------------------------------------------------------------------------
// 运行时
// ---------------------------------------------------------------------------

/// 单个 Endpoint 的运行任务。返回即表示该 Endpoint 已停止且不再重试。
///
/// `events` 为 `None` 时为纯 Data-only 路径（EventStore 不可用或未配置时）：
/// 不接管 EventReceiver、不起 ingress，数据面完全不受影响（v1.1 §9 隔离）。
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
) {
    let mut backoff_idx = 0usize;
    let mut id_map: HashMap<String, u32> = source.known_map(&cfg.endpoint_id);
    let mut last_epoch: u64 = 0;

    set_status(
        &snapshot,
        &cfg,
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
                ConnectionState::Stopped,
                "stopped",
                id_map.len(),
                last_epoch,
                source.revision(&cfg.endpoint_id),
            );
            return;
        }

        match attempt_session(
            &disc,
            &cfg,
            &snapshot,
            &source,
            &mut id_map,
            &mut last_epoch,
            &shutdown,
            &registry,
            events.as_ref(),
        )
        .await
        {
            AttemptOutcome::Shutdown => {
                set_status(
                    &snapshot,
                    &cfg,
                    ConnectionState::Stopped,
                    "stopped",
                    id_map.len(),
                    last_epoch,
                    source.revision(&cfg.endpoint_id),
                );
                return;
            }
            AttemptOutcome::ConfigurationFailed(detail) => {
                tracing::error!(endpoint = %cfg.endpoint_id, %detail, "configuration error, no retry");
                set_status(
                    &snapshot,
                    &cfg,
                    ConnectionState::Failed,
                    &detail,
                    id_map.len(),
                    last_epoch,
                    source.revision(&cfg.endpoint_id),
                );
                return;
            }
            AttemptOutcome::Lost {
                reason,
                had_running_session,
            } => {
                tracing::warn!(endpoint = %cfg.endpoint_id, %reason, had_running_session, "connection lost");
                snapshot.mark_communication_lost(&cfg.endpoint_id);
                set_status(
                    &snapshot,
                    &cfg,
                    ConnectionState::Reconnecting,
                    &reason,
                    id_map.len(),
                    last_epoch,
                    source.revision(&cfg.endpoint_id),
                );
                // 仅当本次 attempt 曾进入 Running（即 had_running_session）才归零，否则指数退避
                if had_running_session {
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
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.cancelled() => {
                set_status(&snapshot, &cfg, ConnectionState::Stopped, "core shutdown", id_map.len(), last_epoch, source.revision(&cfg.endpoint_id));
                return;
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

fn set_status(
    snapshot: &Snapshot,
    cfg: &BuiltinEndpoint,
    state: ConnectionState,
    detail: &str,
    points: usize,
    epoch: u64,
    revision: u64,
) {
    snapshot.upsert_endpoint(EndpointStatus {
        endpoint_id: cfg.endpoint_id.clone(),
        driver_id: cfg.driver_id.clone(),
        state: state.as_str().to_string(),
        detail: detail.to_string(),
        revision,
        points,
        epoch,
    });
}

/// 一次完整连接尝试：成功则阻塞在事件循环直到断开/判死/停机。
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
) -> AttemptOutcome {
    let mut process = match DriverProcess::spawn(disc).await {
        Ok(p) => p,
        Err(e) => {
            return AttemptOutcome::Lost {
                reason: format!("spawn failed: {e}"),
                had_running_session: false,
            };
        }
    };

    let (mut session, mut events, unresponsive_flag) =
        match Session::connect_retry(process.port, &process.token).await {
            Ok((s, ev, flag)) => (s, ev, flag),
            Err(e) => {
                process.terminate().await;
                return AttemptOutcome::Lost {
                    reason: format!("connect failed: {e}"),
                    had_running_session: false,
                };
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
    // 统计句柄 step ⑧ 接入 Endpoint diagnostics，此处仅创建持有。
    let _ingress_stats: crate::event_ingress::SharedIngressStats =
        Arc::new(Mutex::new(IngressStats::default()));
    // ingress 优雅取消令牌（Checkpoint B 方案 A）：attempt 结束时先 cancel，
    // 当前 commit+publish 完整执行后退出；超时才 abort（见下方收尾）。
    let ingress_shutdown = shutdown.child_token();
    let mut ingress: Option<tokio::task::JoinHandle<Result<(), IngressFatal>>> =
        match (event_rx, services) {
            (Some(rx), Some(svc)) => Some(tokio::spawn(run_event_ingress(
                rx,
                cfg.endpoint_id.clone(),
                Arc::clone(svc),
                Arc::clone(&_ingress_stats),
                ingress_shutdown.clone(),
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
            // 配置失败：ingress 若已 spawn（Start 前）优雅停下，避免无主消费
            shutdown_ingress(ingress_shutdown.clone(), ingress).await;
            {
                let mut sess = session_arc.lock().await;
                sess.invalidate();
            }
            registry.write().unwrap().remove(&cfg.endpoint_id);
            process.terminate().await;
            return outcome;
        }
    }

    set_status(
        snapshot,
        cfg,
        ConnectionState::Running,
        "",
        id_map.len(),
        *last_epoch,
        source.revision(&cfg.endpoint_id),
    );

    // 事件循环期间保持会话注册，Control 请求通过同一 session_arc 的 Mutex 串行化
    let outcome = event_loop(
        cfg,
        snapshot,
        unresponsive_flag.as_ref(),
        &mut events,
        &mut ingress,
        shutdown,
    )
    .await;

    // ingress 收尾：优雅取消（当前 commit+publish 必完整执行，见
    // run_event_ingress），杜绝"DB 已有但 Hub 未发布"的静默缺口。
    shutdown_ingress(ingress_shutdown, ingress).await;

    {
        let sess = session_arc.lock().await;
        if !sess.is_unresponsive() {
            let _ = sess.post(pb_shutdown_body()).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    {
        let mut sess = session_arc.lock().await;
        sess.invalidate();
    }
    registry.write().unwrap().remove(&cfg.endpoint_id);
    drop(events);
    process.terminate().await;
    outcome
}

fn pb_shutdown_body() -> pb::envelope::Body {
    pb::envelope::Body::Shutdown(pb::Shutdown {})
}

/// ingress 优雅停机（Checkpoint B 方案 A）：先 cancel 让当前 commit+publish
/// 完整执行，5s 内未退出才 abort（磁盘 hang 等极端情况，大声告警）。
/// abort 兜底仍可能跳过 hub 发布——等价于一次 broadcast lag，SSE replay
/// 按 seq 补回；DB 事实本身不受影响（writer 线程不受 abort 影响）。
async fn shutdown_ingress(
    cancel: CancellationToken,
    ingress: Option<tokio::task::JoinHandle<Result<(), IngressFatal>>>,
) {
    let Some(h) = ingress else {
        return;
    };
    cancel.cancel();
    // NOTE: 不能用 timeout(h)——超时会 drop JoinHandle 使任务 detach 继续跑。
    // select 保留所有权，超时后显式 abort。
    let mut h = h;
    tokio::select! {
        _ = &mut h => {}
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            tracing::error!("event ingress did not exit in 5s, aborting");
            h.abort();
            let _ = h.await;
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
    let defs: Vec<PointDefinition> = parsed
        .iter()
        .zip(&ids)
        .map(|(d, &id)| PointDefinition {
            point_id: id,
            point_key: d.point_key.clone(),
            data_type: d.data_type,
            unit: d.unit.clone(),
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
