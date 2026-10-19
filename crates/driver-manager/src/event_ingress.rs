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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use mesa_core_types::{EventSequenceTracker, SequenceVerdict, now_unix_ns};
use mesa_event_store::{CommitRequest, EventDiagnostics, EventServices};
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

/// teardown drain 结论（P0-3）：`shutdown_ingress` 的显式结果，禁止吞错。
/// Stop 必须等待 teardown 真正完成；drain 失败是显式错误，不是 `true`。
#[derive(Debug)]
pub enum EventDrainError {
    /// drain 中精确失败（collision/unavailable/regression/stream-closed 等）：
    /// code 透出原精确码，调用方禁止 contains 回猜。
    Fatal(IngressFatal),
    /// drain 超时（`shutdown_ingress` 的 deadline，未退出则 abort 兜底已执行）：
    /// backlog 可能未 COMMIT，既无 DB 行也无 Hub 行，SSE reconcile 救不了——
    /// 必须显式报错。文本不写具体秒数（预算是可组合推导值，会变；码冻结不变）。
    Timeout,
    /// drain 任务 panic/join 失败：同 Timeout 处理（未完成即失败）。
    Join(String),
}

impl EventDrainError {
    /// wire/REST/日志用精确原因码。
    pub fn code(&self) -> &'static str {
        match self {
            EventDrainError::Fatal(f) => f.code(),
            EventDrainError::Timeout => "EVENT_DRAIN_TIMEOUT",
            EventDrainError::Join(_) => "EVENT_DRAIN_FAILED",
        }
    }
}

impl std::fmt::Display for EventDrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventDrainError::Fatal(e) => write!(f, "{}: {e}", e.code()),
            EventDrainError::Timeout => {
                write!(
                    f,
                    "EVENT_DRAIN_TIMEOUT: ingress did not finish before drain deadline"
                )
            }
            EventDrainError::Join(e) => write!(f, "EVENT_DRAIN_FAILED: {e}"),
        }
    }
}

/// 运行 ingress 直到流关闭、fatal 或优雅取消。`endpoint_id` 即去重作用域
/// 与存储分区。
///
/// `evidence`（P1-A1）：**本次 attempt 的运行证据**句柄。收到任一事件批次
/// （含 duplicate/gap 与最终 drain 的批次）即置真；`attempt_session` 在
/// teardown（Join/abort 完成）之后读取它，决定重连退避是否归零。语义是
/// “数据面通过”，与数据质量、是否入库成功无关。`None` = 无需观测
/// （单元测试直连等无重连语义的场景）。
///
/// 优雅取消（Checkpoint B 方案 A + ⑨ final drain）：cancel 只在循环头检查——
/// 当前 commit+publish 必完整执行；cancel 到达后**排空 channel 里已有批次**
/// 再退出（⑨：Stop 时已进入 Core 的旧 epoch Event 不得因 ingress 先退而被
/// 遗弃）。barrier（reader 已结束）保证 drain 期间无新到达——drain 必终止；
/// 无 barrier 时见空即停（仍严格优于直接返回）。
/// abort 只作为超时兜底（见 `attempt_session`）。
/// P1-1：运行质量计数直写 `services.diagnostics`（全局、live），不再经
/// per-attempt 本地统计中转（黑洞已除；attempt 结束后计数保留在全局）。
pub async fn run_event_ingress(
    mut rx: EventReceiver,
    endpoint_id: String,
    services: Arc<EventServices>,
    shutdown: CancellationToken,
    evidence: Option<Arc<AtomicBool>>,
) -> Result<(), IngressFatal> {
    let mut proc = BatchProcessor::new(endpoint_id, services, evidence);
    loop {
        let batch = tokio::select! {
            b = rx.recv() => b.ok_or(IngressFatal::StreamClosed)?,
            // 优雅退出：当前 commit+publish 已完成（循环尾）；先排空再返回
            _ = shutdown.cancelled() => {
                while let Ok(batch) = rx.try_recv() {
                    proc.process(batch).await?;
                }
                return Ok(());
            }
        };
        proc.process(batch).await?;
    }
}

/// 单批处理（主循环与 final drain 共用；语义恒一致）。
struct BatchProcessor {
    endpoint_id: String,
    services: Arc<EventServices>,
    tracker: EventSequenceTracker,
    // 已见最大序号（epoch, seq）：tracker 不暴露内部 max，本地维护，
    // 用于 Regression fatal 的精确上下文（之前最大是多少）。
    max_seen: Option<(u64, u64)>,
    /// P1-A1：本次 attempt 的运行证据（收到即置真，先于 Sequence Gate）。
    evidence: Option<Arc<AtomicBool>>,
}

impl BatchProcessor {
    fn new(
        endpoint_id: String,
        services: Arc<EventServices>,
        evidence: Option<Arc<AtomicBool>>,
    ) -> Self {
        Self {
            endpoint_id,
            services,
            tracker: EventSequenceTracker::new(),
            max_seen: None,
            evidence,
        }
    }

    /// 诊断计数（P1-1）：Relaxed 原子，直写全局 diagnostics，
    /// live 可观测（`GET /events/stats` 暴露）。
    fn diag(&self, n: u64, field: fn(&EventDiagnostics) -> &AtomicU64) {
        field(&self.services.diagnostics).fetch_add(n, Ordering::Relaxed);
    }

    async fn process(&mut self, batch: mesa_core_types::EventBatch) -> Result<(), IngressFatal> {
        // P1-A1：批次“到达 Core 并进入本 attempt 的消费路径”本身就是运行证据。
        // 必须早于 Sequence Gate（Duplicate 会整批跳过并 return）与 commit
        // （入库失败属于另一故障域，但驱动→Core 链路仍然通过）。
        if let Some(e) = &self.evidence {
            e.store(true, Ordering::Relaxed);
        }
        self.diag(1, |d| &d.ingress_batches_total);
        let (handle, epoch, seq) = (batch.connection_handle, batch.stream_epoch, batch.sequence);
        // Sequence Gate（PR5 冻结语义，PR7 第一次执法）
        match self.tracker.check(epoch, seq) {
            SequenceVerdict::Accept => {
                self.max_seen = Some((epoch, seq));
            }
            SequenceVerdict::Gap { expected, got } => {
                // 缺口可观测：入库 + 计数，不假装没发生
                self.diag(1, |d| &d.ingress_gaps_total);
                self.max_seen = Some((epoch, got));
                tracing::warn!(
                    endpoint = %self.endpoint_id,
                    expected, got,
                    "event sequence gap, committed with gap accounted"
                );
            }
            SequenceVerdict::Duplicate => {
                // 整批跳过：不开 DB txn（同 seq 即同内容是 wire invariant，
                // 无需第二套 payload hash）。
                self.diag(1, |d| &d.ingress_batch_duplicates_total);
                tracing::debug!(
                    endpoint = %self.endpoint_id,
                    seq,
                    "duplicate event batch skipped without transaction"
                );
                return Ok(());
            }
            SequenceVerdict::Regression => {
                let prev = self
                    .max_seen
                    .filter(|(e, _)| *e == epoch)
                    .map(|(_, s)| s)
                    .unwrap_or(0);
                self.diag(1, |d| &d.ingress_regressions_total);
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
        // commit 延迟测量（单调时钟）：队列等待 + writer 执行，为容量公式
        // STALL 输入与 stall 调查提供实证。
        let commit_start = std::time::Instant::now();
        let res = self
            .services
            .store
            .commit_batch(CommitRequest {
                endpoint_id: self.endpoint_id.clone(),
                batch,
                received_at_ns,
            })
            .await;
        let commit_latency_ns = commit_start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        self.services
            .diagnostics
            .ingress_commit_latency_last_ns
            .store(commit_latency_ns, Ordering::Relaxed);
        // max 语义不能用 fetch_add：CAS 循环收敛（低竞争，commit 串行）。
        {
            let max_ref = &self.services.diagnostics.ingress_commit_latency_max_ns;
            let mut cur = max_ref.load(Ordering::Relaxed);
            while commit_latency_ns > cur {
                match max_ref.compare_exchange_weak(
                    cur,
                    commit_latency_ns,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(v) => cur = v,
                }
            }
        }
        match res {
            Ok(outcome) => {
                self.diag(outcome.inserted.len() as u64, |d| {
                    &d.ingress_persisted_events_total
                });
                self.diag(outcome.duplicates, |d| &d.ingress_event_duplicates_total);
                // 先 COMMIT，后发布：Hub 只见已落盘行（重放旧行不再发布）
                for ev in &outcome.inserted {
                    self.services.hub.publish(ev);
                }
                Ok(())
            }
            Err(mesa_event_store::EventStoreError::IdCollision {
                endpoint_id,
                event_id,
            }) => {
                self.diag(1, |d| &d.ingress_collisions_total);
                Err(IngressFatal::IdCollision {
                    endpoint_id,
                    event_id,
                })
            }
            Err(mesa_event_store::EventStoreError::InvalidRecord(detail)) => {
                self.diag(1, |d| &d.ingress_invalid_total);
                Err(IngressFatal::InvalidBatch(format!(
                    "handle {handle}: {detail}"
                )))
            }
            Err(mesa_event_store::EventStoreError::Unavailable(detail)) => {
                self.diag(1, |d| &d.ingress_store_failures_total);
                Err(IngressFatal::StoreUnavailable(detail))
            }
            Err(mesa_event_store::EventStoreError::Fatal(e)) => {
                self.diag(1, |d| &d.ingress_store_failures_total);
                Err(IngressFatal::StoreUnavailable(format!("sqlite: {e}")))
            }
            Err(mesa_event_store::EventStoreError::Encode(e)) => {
                self.diag(1, |d| &d.ingress_invalid_total);
                Err(IngressFatal::InvalidBatch(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{EventBatch, EventRecord};

    fn batch(seq: u64, id: &str) -> EventBatch {
        EventBatch {
            connection_handle: 1,
            stream_epoch: 7,
            sequence: seq,
            timestamp_ns: 1_700_000_000_000_000_000,
            events: vec![EventRecord {
                event_id: id.into(),
                category: "message".into(),
                kind: "counter.tick".into(),
                source: "T".into(),
                severity: 100,
                code: None,
                message: None,
                message_locale: None,
                occurred_at_ns: None,
                condition: None,
                correlation_id: None,
                attributes: Default::default(),
            }],
            mono_ns: None,
        }
    }

    fn processor(evidence: Option<Arc<AtomicBool>>) -> BatchProcessor {
        let store = Arc::new(mesa_event_store::EventStore::open_in_memory().unwrap());
        let services =
            mesa_event_store::EventServices::new(store, mesa_event_store::EventHub::new(16));
        BatchProcessor::new("hd-evidence".into(), services, evidence)
    }

    /// P1-A1 S1（处理层）：正常批次必须置真。
    #[tokio::test]
    async fn evidence_marked_on_batch_processing() {
        let ev = Arc::new(AtomicBool::new(false));
        let mut p = processor(Some(Arc::clone(&ev)));
        assert!(!ev.load(Ordering::Relaxed));
        p.process(batch(1, "e-1")).await.expect("commit ok");
        assert!(ev.load(Ordering::Relaxed));
    }

    /// P1-A1 S2：置真必须早于 Sequence Gate —— Duplicate 会整批跳过并 return，
    /// 但“本 attempt 收到过”仍成立。final drain 走的正是同一个 `process`。
    #[tokio::test]
    async fn evidence_marked_before_sequence_gate_on_duplicate() {
        let ev = Arc::new(AtomicBool::new(false));
        let mut p = processor(Some(Arc::clone(&ev)));
        p.process(batch(1, "e-dup")).await.expect("first ok");
        assert!(ev.load(Ordering::Relaxed));
        // 仅测试用：清掉后观察“同一批再次到达”是否仍会置真。
        ev.store(false, Ordering::Relaxed);
        p.process(batch(1, "e-dup"))
            .await
            .expect("duplicate skipped");
        assert!(
            ev.load(Ordering::Relaxed),
            "Duplicate 在 gate 之前也必须置真（否则 drain/重复批会被漏算）"
        );
    }

    /// P1-A1：None 路径零成本且不得 panic（单元测试/无重连语义场景）。
    #[tokio::test]
    async fn evidence_none_path_is_safe() {
        let mut p = processor(None);
        p.process(batch(1, "e-none")).await.expect("commit ok");
    }
}
