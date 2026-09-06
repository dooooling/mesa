//! EventIngress（PR7 v1.1 §7）：`EventReceiver → EventStore` 的唯一写路径。
//!
//! 链（每批按序执行）：
//! ```text
//! EventBatch
//!   → EventSequenceTracker（Accept 提交 / Gap 提交+计数 / Duplicate 整批跳过不开 txn）
//!   → received_at 取一次（同批共用）
//!   → EventStore.commit_batch（一批一事务）
//!   → 先 COMMIT，后 EventHub.publish（No COMMIT → No Visibility）
//! ```
//!
//! Fatal（v1.1 §4 hard contract）：Regression / IdCollision / Store 不可用 →
//! 终止本 task，`attempt_session` 观察到后按 `AttemptOutcome::Lost` 走现有
//! reconnect/backoff（had_running_session=true）。**只杀本 endpoint attempt，
//! 不碰其他 endpoint，更不 shutdown 整个 Mesa。**

use std::sync::{Arc, Mutex};

use mesa_core_types::{EventSequenceTracker, SequenceVerdict, now_unix_ns};
use mesa_event_store::{CommitRequest, EventServices};
use tokio_util::sync::CancellationToken;

use crate::session::EventReceiver;

/// Ingress 致命失败（精确原因码，调用方按变体路由）。
#[derive(Debug, thiserror::Error)]
pub enum IngressFatal {
    /// Driver 破坏了 ordered stream（10 → 12 → 11）：历史顺序已失去定义，
    /// 继续存就是伪造，必须重连开新 epoch。
    #[error("EVENT_SEQUENCE_REGRESSION: handle {handle} epoch {epoch} got {got} after {max}")]
    SequenceRegression {
        handle: u32,
        epoch: u64,
        got: u64,
        max: u64,
    },
    /// 同 endpoint+event_id 出现异 payload：occurrence 身份契约被破坏。
    #[error("EVENT_ID_COLLISION: endpoint {endpoint_id} event {event_id}")]
    IdCollision {
        endpoint_id: String,
        event_id: String,
    },
    /// Store 写失败（disk/busy/corruption/writer 死）：无持久化时事件使能
    /// endpoint 禁止假装 RUNNING。Data-only endpoint 不受影响（独立路径）。
    #[error("EVENT_STORE_UNAVAILABLE: {0}")]
    StoreUnavailable(String),
    /// 驱动送来连结构校验都不过的记录（理论不可达：publish 侧已校验）：
    /// fail-closed，不跳过、不入库。
    #[error("EVENT_RECORD_INVALID: {0}")]
    InvalidBatch(String),
    /// 事件流在会话层已关闭（overflow fail-closed / 会话结束）：残缺流
    /// 禁止继续消费，触发重连开新流。
    #[error("EVENT_STREAM_CLOSED")]
    StreamClosed,
}

impl IngressFatal {
    ///  wire/日志用精确原因码（禁止 contains 回猜）。
    pub fn code(&self) -> &'static str {
        match self {
            IngressFatal::SequenceRegression { .. } => "EVENT_SEQUENCE_REGRESSION",
            IngressFatal::IdCollision { .. } => "EVENT_ID_COLLISION",
            IngressFatal::StoreUnavailable(_) => "EVENT_STORE_UNAVAILABLE",
            IngressFatal::InvalidBatch(_) => "EVENT_RECORD_INVALID",
            IngressFatal::StreamClosed => "EVENT_STREAM_CLOSED",
        }
    }
}

/// Ingress 运行统计。调用方持有 [`SharedIngressStats`] 并在运行期读取
/// （step ⑧ 聚合进 Endpoint diagnostics）；本函数只写不读。
#[derive(Debug, Default)]
pub struct IngressStats {
    pub batches: u64,
    pub persisted_events: u64,
    /// batch 层整批跳过（tracker Duplicate，不开 txn）。
    pub batch_duplicates: u64,
    /// event 层逐条去重（UNIQUE 同 payload，txn 内）。
    pub event_duplicates: u64,
    pub gaps: u64,
}

/// 共享统计句柄（attempt 内创建，step ⑧ 接入快照/REST）。
pub type SharedIngressStats = Arc<Mutex<IngressStats>>;

/// 运行 ingress 直到流关闭、fatal 或优雅取消。`endpoint_id` 即去重作用域
/// 与存储分区。
///
/// 优雅取消（Checkpoint B 方案 A）：cancel 只在循环头检查——当前
/// commit+publish 必完整执行后才退出，杜绝"DB 已有但 Hub 未发布"的静默缺口
/// （abort 只作为超时兜底，见 `attempt_session`）。
pub async fn run_event_ingress(
    mut rx: EventReceiver,
    endpoint_id: String,
    services: Arc<EventServices>,
    stats: SharedIngressStats,
    shutdown: CancellationToken,
) -> Result<(), IngressFatal> {
    let mut tracker = EventSequenceTracker::new();
    // 已见最大序号（epoch, seq）：tracker 不暴露内部 max，ingress 本地维护，
    // 用于 Regression fatal 的精确上下文（之前最大是多少）。
    let mut max_seen: Option<(u64, u64)> = None;
    loop {
        let batch = tokio::select! {
            b = rx.recv() => b.ok_or(IngressFatal::StreamClosed)?,
            // 优雅退出：当前 commit+publish 已完成（循环尾），下次迭代不再取批
            _ = shutdown.cancelled() => return Ok(()),
        };
        stats.lock().unwrap().batches += 1;
        let (handle, epoch, seq) = (batch.connection_handle, batch.stream_epoch, batch.sequence);
        // Sequence Gate（PR5 冻结语义，PR7 第一次执法）
        match tracker.check(epoch, seq) {
            SequenceVerdict::Accept => {
                max_seen = Some((epoch, seq));
            }
            SequenceVerdict::Gap { expected, got } => {
                // 缺口可观测：入库 + 计数，不假装没发生
                stats.lock().unwrap().gaps += 1;
                max_seen = Some((epoch, got));
                tracing::warn!(
                    endpoint = %endpoint_id,
                    expected, got,
                    "event sequence gap, committed with gap accounted"
                );
            }
            SequenceVerdict::Duplicate => {
                // 整批跳过：不开 DB txn（同 seq 即同内容是 wire invariant，
                // 无需第二套 payload hash）。
                stats.lock().unwrap().batch_duplicates += 1;
                tracing::debug!(
                    endpoint = %endpoint_id,
                    seq,
                    "duplicate event batch skipped without transaction"
                );
                continue;
            }
            SequenceVerdict::Regression => {
                let prev = max_seen
                    .filter(|(e, _)| *e == epoch)
                    .map(|(_, s)| s)
                    .unwrap_or(0);
                return Err(IngressFatal::SequenceRegression {
                    handle,
                    epoch,
                    got: seq,
                    max: prev,
                });
            }
        }
        // v1.1 §3：received_at 每 batch 取一次，同批共用
        let received_at_ns = now_unix_ns();
        let res = services
            .store
            .commit_batch(CommitRequest {
                endpoint_id: endpoint_id.clone(),
                batch,
                received_at_ns,
            })
            .await;
        match res {
            Ok(outcome) => {
                stats.lock().unwrap().persisted_events += outcome.inserted.len() as u64;
                stats.lock().unwrap().event_duplicates += outcome.duplicates;
                // 先 COMMIT，后发布：Hub 只见已落盘行（重放旧行不再发布）
                for ev in &outcome.inserted {
                    services.hub.publish(ev);
                }
            }
            Err(mesa_event_store::EventStoreError::IdCollision {
                endpoint_id,
                event_id,
            }) => {
                return Err(IngressFatal::IdCollision {
                    endpoint_id,
                    event_id,
                });
            }
            Err(mesa_event_store::EventStoreError::InvalidRecord(detail)) => {
                return Err(IngressFatal::InvalidBatch(format!(
                    "handle {handle}: {detail}"
                )));
            }
            Err(mesa_event_store::EventStoreError::Unavailable(detail)) => {
                return Err(IngressFatal::StoreUnavailable(detail));
            }
            Err(mesa_event_store::EventStoreError::Fatal(e)) => {
                return Err(IngressFatal::StoreUnavailable(format!("sqlite: {e}")));
            }
            Err(mesa_event_store::EventStoreError::Encode(e)) => {
                return Err(IngressFatal::InvalidBatch(e));
            }
        }
    }
}
