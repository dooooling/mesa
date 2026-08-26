//! Endpoint 运行时（Core 侧）：配置闭环 + 数据消费 + 故障处置。
//!
//! 完整实现方案 §6.2 配置流程与 §11.1 重连语义的 M0 子集：
//! `spawn -> handshake -> Open -> Configure -> PointDescriptors -> ApplyPointMap
//!   -> Start(new epoch) -> 事件循环`；
//! 断连/无响应时按退避序列自动重建；配置类错误直接 FAILED 不自动重试。
//!
//! TODO(Phase 4): 恢复阈值可配置化、CIRCUIT_OPEN 熔断、Point ID tombstone 持久化。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use forgelink_core_types::{ensure_unique_point_keys, ConnectionState, DataBatch, PointDefinition};
use forgelink_driver_protocol::pb;
use tokio_util::sync::CancellationToken;

use crate::manifest::DiscoveredDriver;
use crate::process::DriverProcess;
use crate::session::{Session, SessionEvent};
use crate::snapshot::{EndpointStatus, Snapshot};

/// 连接级退避序列（§11.1 默认值）。M0 不做上限熔断，成功后归零。
const RECONNECT_BACKOFF_SECS: [u64; 5] = [1, 2, 5, 10, 30];

/// 内置端点配置：M0 由 forgelinkd 硬编码注入，替代尚未实现的 REST/ConfigStore。
#[derive(Debug, Clone)]
pub struct BuiltinEndpoint {
    pub endpoint_id: String,
    pub driver_id: String,
    /// Endpoint.connection 的 JSON 序列化，语义由 Driver 解释。
    pub connection_json: String,
    pub tasks: Vec<forgelink_core_types::AcquisitionTask>,
}

/// point_id 分配器：进程内稳定。同 key 复用既有 id——Driver 重启不改变映射，
/// 与"分配后不复用"的 tombstone 规则兼容（tombstone 落库在 Phase 1）。
#[derive(Default)]
pub struct PointIdAllocator {
    next: AtomicU64,
}

impl PointIdAllocator {
    fn assign(&self, used: &mut HashMap<String, u32>, keys: &[String]) -> Vec<u32> {
        keys.iter()
            .map(|k| {
                *used.entry(k.clone()).or_insert_with(|| {
                    self.next.fetch_add(1, Ordering::Relaxed) as u32 + 1 // 从 1 起
                })
            })
            .collect()
    }
}

/// 单个 Endpoint 的运行任务。返回即表示该 Endpoint 已停止且不再重试。
pub async fn run_endpoint(
    disc: DiscoveredDriver,
    cfg: BuiltinEndpoint,
    snapshot: Arc<Snapshot>,
    allocator: Arc<PointIdAllocator>,
    shutdown: CancellationToken,
) {
    let mut backoff_idx = 0usize;
    let mut id_map: HashMap<String, u32> = HashMap::new();

    set_status(&snapshot, &cfg, ConnectionState::Connecting, "", id_map.len());

    loop {
        if shutdown.is_cancelled() {
            return;
        }

        match attempt_session(&disc, &cfg, &snapshot, &allocator, &mut id_map, &shutdown).await {
            AttemptOutcome::Shutdown => return,
            AttemptOutcome::ConfigurationFailed(detail) => {
                // 非重试错误（§11.1）：保持 FAILED 直到配置 Revision 变化或显式 Start
                tracing::error!(endpoint = %cfg.endpoint_id, %detail, "configuration error, no retry");
                set_status(&snapshot, &cfg, ConnectionState::Failed, &detail, id_map.len());
                return;
            }
            AttemptOutcome::Lost(reason) => {
                // 断线语义（§11）：已知点全部转 BAD，值与时间戳保持原样
                tracing::warn!(endpoint = %cfg.endpoint_id, %reason, "connection lost");
                snapshot.mark_communication_lost(&cfg.endpoint_id);
                set_status(&snapshot, &cfg, ConnectionState::Reconnecting, &reason, id_map.len());
            }
        }

        let delay = Duration::from_secs(
            RECONNECT_BACKOFF_SECS[backoff_idx.min(RECONNECT_BACKOFF_SECS.len() - 1)],
        );
        backoff_idx += 1;
        tracing::info!(endpoint = %cfg.endpoint_id, retry_in_secs = delay.as_secs(), "reconnecting");
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.cancelled() => {
                set_status(&snapshot, &cfg, ConnectionState::Stopped, "core shutdown", id_map.len());
                return;
            }
        }
    }
}

enum AttemptOutcome {
    Shutdown,
    ConfigurationFailed(String),
    Lost(String),
}

fn set_status(snapshot: &Snapshot, cfg: &BuiltinEndpoint, state: ConnectionState, detail: &str, points: usize) {
    snapshot.upsert_endpoint(EndpointStatus {
        endpoint_id: cfg.endpoint_id.clone(),
        driver_id: cfg.driver_id.clone(),
        state: state.as_str().to_string(),
        detail: detail.to_string(),
        revision: 0, // TODO(Phase 1): 接入 ConfigStore revision
        points,
    });
}

/// 一次完整连接尝试：成功则阻塞在事件循环直到断开/判死/停机。
async fn attempt_session(
    disc: &DiscoveredDriver,
    cfg: &BuiltinEndpoint,
    snapshot: &Arc<Snapshot>,
    allocator: &Arc<PointIdAllocator>,
    id_map: &mut HashMap<String, u32>,
    shutdown: &CancellationToken,
) -> AttemptOutcome {
    let mut process = match DriverProcess::spawn(disc).await {
        Ok(p) => p,
        Err(e) => return AttemptOutcome::Lost(format!("spawn failed: {e}")),
    };

    let (session, mut events, unresponsive_flag) =
        match Session::connect_retry(process.port, &process.token).await {
            Ok((s, ev, flag)) => (s, ev, flag),
            Err(e) => {
                process.terminate().await;
                return AttemptOutcome::Lost(format!("connect failed: {e}"));
            }
        };
    let mut session = session; // invalidate(&mut) 需要

    match run_config_flow(&session, cfg, allocator, id_map, snapshot).await {
        Ok(()) => {}
        Err(outcome) => {
            session.invalidate();
            process.terminate().await;
            return outcome;
        }
    }

    set_status(snapshot, cfg, ConnectionState::Running, "", id_map.len());

    let outcome =
        event_loop(cfg, snapshot, &session, unresponsive_flag.as_ref(), &mut events, shutdown).await;

    // 收尾：先发 Shutdown 消息再终止进程；EOF 防护作为兜底第二信号
    if !session.is_unresponsive() {
        let _ = session.post(pb_shutdown_body()).await;
        tokio::time::sleep(Duration::from_millis(50)).await; // 给 Driver 一个出帧窗口
    }
    session.invalidate();
    drop(events);
    process.terminate().await;
    outcome
}

fn pb_shutdown_body() -> pb::envelope::Body {
    pb::envelope::Body::Shutdown(pb::Shutdown {})
}

/// 配置闭环（§6.2）：Open -> Configure -> PointDescriptors -> ApplyPointMap -> Start。
async fn run_config_flow(
    session: &Session,
    cfg: &BuiltinEndpoint,
    allocator: &Arc<PointIdAllocator>,
    id_map: &mut HashMap<String, u32>,
    snapshot: &Arc<Snapshot>,
) -> Result<(), AttemptOutcome> {
    use pb::envelope::Body;

    const HANDLE: u32 = 1; // M0 每驱动进程单连接，handle 固定为 1

    // OpenConnection
    let reply = session
        .call(Body::OpenConnection(pb::OpenConnection {
            connection_handle: HANDLE,
            endpoint_id: cfg.endpoint_id.clone(),
            config_json: cfg.connection_json.clone(),
        }))
        .await
        .map_err(|e| AttemptOutcome::Lost(format!("open rpc: {e}")))?;
    let result = expect_ack(reply.body).ok_or_else(|| lost("OpenConnection"))?;
    config_gate(result, "OpenConnection")?;

    // ConfigureTasks(revision=1 全量快照)
    let tasks_pb = tasks_to_pb_checked(cfg)?;
    let reply = session
        .call(Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: HANDLE,
            revision: 1,
            tasks: tasks_pb,
        }))
        .await
        .map_err(|e| AttemptOutcome::Lost(format!("configure rpc: {e}")))?;
    // configure 的响应是主动上报的 PointDescriptors 或错误帧
    let descriptors_pb = match reply.body {
        Some(Body::PointDescriptors(rep)) => rep.descriptors,
        Some(Body::DriverError(err)) => {
            let d = err.detail.unwrap_or_default();
            return Err(config_fail(d.kind, d.code, d.message));
        }
        other => return Err(lost(format!("unexpected configure reply: {other:?}"))),
    };

    // Core 侧二次校验（§6.2 双重保护之 Core 侧）
    let parsed: Vec<forgelink_core_types::PointDescriptor> = descriptors_pb
        .into_iter()
        .map(forgelink_driver_protocol::descriptor_from_pb)
        .collect::<Result<_, _>>()
        .map_err(|e| AttemptOutcome::ConfigurationFailed(e.to_string()))?;
    ensure_unique_point_keys(&parsed).map_err(|e| AttemptOutcome::ConfigurationFailed(e.to_string()))?;

    // 分配稳定 point_id + 登记点元数据
    let keys: Vec<String> = parsed.iter().map(|d| d.point_key.clone()).collect();
    let ids = allocator.assign(id_map, &keys);
    let defs: Vec<PointDefinition> = parsed
        .iter()
        .zip(&ids)
        .map(|(d, &id)| PointDefinition { point_id: id, point_key: d.point_key.clone(), data_type: d.data_type, unit: d.unit.clone() })
        .collect();
    snapshot.register_points(&cfg.endpoint_id, &defs);

    // ApplyPointMap
    let map: HashMap<String, u32> = defs.iter().map(|d| (d.point_key.clone(), d.point_id)).collect();
    let reply = session
        .call(Body::ApplyPointMap(pb::ApplyPointMap {
            connection_handle: HANDLE,
            revision: 1,
            key_to_point_id: map,
        }))
        .await
        .map_err(|e| AttemptOutcome::Lost(format!("apply rpc: {e}")))?;
    let result = expect_ack(reply.body).ok_or_else(|| lost("ApplyPointMap"))?;
    config_gate(result, "ApplyPointMap")?;

    // StartConnection(new stream_epoch)
    let epoch = new_stream_epoch();
    let reply = session
        .call(Body::StartConnection(pb::StartConnection { connection_handle: HANDLE, stream_epoch: epoch }))
        .await
        .map_err(|e| AttemptOutcome::Lost(format!("start rpc: {e}")))?;
    let result = expect_ack(reply.body).ok_or_else(|| lost("StartConnection"))?;
    config_gate(result, "StartConnection")?;
    tracing::info!(endpoint = %cfg.endpoint_id, epoch, points = defs.len(), "connection started");

    Ok(())
}

async fn event_loop(
    cfg: &BuiltinEndpoint,
    snapshot: &Arc<Snapshot>,
    session: &Session,
    unresponsive_flag: &std::sync::atomic::AtomicBool,
    events: &mut tokio::sync::mpsc::Receiver<SessionEvent>,
    shutdown: &CancellationToken,
) -> AttemptOutcome {
    const WATCHDOG_TICK: Duration = Duration::from_secs(2);
    let mut watchdog = tokio::time::interval(WATCHDOG_TICK);
    watchdog.tick().await; // interval 首个 tick 立即完成，跳过

    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Some(SessionEvent::Batch(batch)) => {
                    apply_batch_logged(snapshot, cfg, batch);
                }
                Some(SessionEvent::State { state, detail, .. }) => {
                    tracing::debug!(endpoint=%cfg.endpoint_id, ?state, %detail, "state");
                    if matches!(state, ConnectionState::Stopped | ConnectionState::Failed) {
                        return AttemptOutcome::Lost(format!("driver reported {state:?} {detail}"));
                    }
                }
                Some(SessionEvent::DriverError { kind, code, message, .. }) => {
                    tracing::warn!(endpoint=%cfg.endpoint_id, %kind, %code, %message, "driver error");
                    if kind == "ConfigurationError" || code == "DUPLICATE_POINT_KEY" {
                        return AttemptOutcome::ConfigurationFailed(format!("{kind}/{code}: {message}"));
                    }
                }
                None => return AttemptOutcome::Lost("event channel closed".into()),
            },
            _ = watchdog.tick() => {
                if session.is_unresponsive() || unresponsive_flag.load(Ordering::Relaxed) {
                    return AttemptOutcome::Lost("heartbeat dead".into());
                }
            }
            _ = shutdown.cancelled() => return AttemptOutcome::Shutdown,
        }
    }
}

// ---- 小工具 ----

fn tasks_to_pb_checked(cfg: &BuiltinEndpoint) -> Result<Vec<pb::AcquisitionTaskProto>, AttemptOutcome> {
    forgelink_driver_protocol::tasks_to_pb(&cfg.tasks)
        .map_err(|e| AttemptOutcome::ConfigurationFailed(e.to_string()))
}

fn apply_batch_logged(snapshot: &Arc<Snapshot>, cfg: &BuiltinEndpoint, batch: DataBatch) {
    let n = batch.values.len();
    snapshot.apply_batch(&batch, &cfg.endpoint_id);
    tracing::trace!(endpoint=%cfg.endpoint_id, seq=batch.sequence, values=n, "batch");
}

/// Ack 类响应统一解包为 GenericResult。
fn expect_ack(body: Option<pb::envelope::Body>) -> Option<pb::GenericResult> {
    use pb::envelope::Body as B;
    match body {
        Some(B::OpenConnectionAck(a)) => a.result,
        Some(B::ConfigApplied(a)) => a.result,
        Some(B::StartConnectionAck(a)) => a.result,
        _ => None,
    }
}

/// ok 直通；ConfigurationError 归入非重试分支，其余按连接丢失处理。
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
        AttemptOutcome::Lost(format!("{kind}/{code}: {message}"))
    }
}

fn lost(reason: impl Into<String>) -> AttemptOutcome {
    AttemptOutcome::Lost(reason.into())
}

/// stream_epoch：时间+pid+自增混合。单机单 Core 下碰撞概率可忽略，
/// M0 不为此引入额外随机依赖。
static EPOCH_COUNTER: AtomicU64 = AtomicU64::new(0);
fn new_stream_epoch() -> u64 {
    let n = forgelink_core_types::now_unix_ns() as u64;
    let pid = std::process::id() as u64;
    let c = EPOCH_COUNTER.fetch_add(1, Ordering::Relaxed);
    n ^ (pid << 32) ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
