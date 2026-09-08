//! Writer Actor（v1.1 §5）：独占写连接 + 独立 blocking 线程。
//!
//! - 整批一事务：全部成功则 COMMIT，任一硬失败则 rollback（半批绝不入库）；
//! - 正常重放（同 id 同 payload）→ `DO NOTHING` + `duplicate_total++`，
//!   不视为失败、不回滚；
//! - 同 id 异 payload → [`EventStoreError::IdCollision`] + 整批回滚；
//! - 不用泛化 `INSERT OR IGNORE`：它会吞掉其他 constraint 错误，冲突必须
//!   精确到 `(endpoint_id, event_id)`（`ON CONFLICT … DO NOTHING` + `changes()`）。

use mesa_core_types::EventBatch;
use rusqlite::{Connection, params};
use std::sync::Arc;
#[cfg(feature = "test-hooks")]
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};

use crate::schema::{self, StoredEvent};
use crate::{EventStoreError, event_payload_hash};

/// Writer 命令队列容量：commit 确认本身很快，64 足够吸收突发；
/// 持续满说明磁盘跟不上，ingress 背压等待（fail-closed 链的一部分）。
pub const WRITER_QUEUE: usize = 64;

/// 提交请求。`received_at_ns` 由 EventIngress 按 batch 取一次后传入
/// （v1.1 §3：同批共用同一时刻）。
pub struct CommitRequest {
    pub endpoint_id: String,
    pub batch: EventBatch,
    pub received_at_ns: i64,
}

/// 提交结果：新插入行（含分配的 `seq`，按 `event_index` 序）与去重计数。
/// EventHub 只发布 `inserted`（重放的旧行不再发布）。
#[derive(Debug)]
pub struct CommitResult {
    pub inserted: Vec<StoredEvent>,
    pub duplicates: u64,
}

/// Writer 线程统计（诊断用，v1.1 §21 的 store 侧）。
#[derive(Debug, Default)]
pub struct EventStoreStats {
    pub rows: u64,
    pub size_bytes: u64,
}

pub enum WriteCommand {
    Commit(
        CommitRequest,
        oneshot::Sender<Result<CommitResult, EventStoreError>>,
    ),
    /// Retention purge（⑧b）：删除 `seq < before_seq` 最多 `batch_limit` 行。
    /// 走写通道而非读连接——与 commit 同一串行队列，天然互斥：
    /// purge 与 commit 永不并发，且 purge 发出时已排队的 commit 先执行
    /// （顺序 = 发送顺序，无需额外 flush；P0 定序保证）。
    Purge {
        before_seq: i64,
        batch_limit: u64,
        reply: oneshot::Sender<Result<usize, EventStoreError>>,
    },
    /// 测试故障器装配（`test-hooks`）：writer 线程收到后持有，后续 Commit
    /// 按其规则注入 SQLITE_FULL。首命令语义（open_with_faults 同步发送，
    /// 通道初始为空，必排在一切 Commit 之前），故装配无竞态。
    #[cfg(feature = "test-hooks")]
    SetFaults(std::sync::Arc<StoreFaults>),
}

/// 测试专用故障注入（PR10 hardening，`test-hooks` feature 门控）：
/// 默认构建中本类型与装配 API 均不存在，writer 热路径零检查。
/// 开启后 writer 线程在每次 Commit 前检查：计数到达 `fail_commit_at`
///（1-based；0 = 不注入）时返回 SQLITE_FULL 风格的 `Fatal` 错误，走与真实
/// 磁盘故障完全相同的回滚 + 归一路径（ingress → `EVENT_STORE_UNAVAILABLE`）。
/// `fail_sticky` 为 true 时第 N 个起每个 commit 都失败（持续故障 + 恢复验证）。
#[cfg(feature = "test-hooks")]
#[derive(Debug, Default)]
pub struct StoreFaults {
    /// 第几个 commit 开始失败（1-based；0 = 永不失败）。
    pub fail_commit_at: AtomicU64,
    /// true = 到达后持续失败直到测试改回 false（恢复验证用，可运行时切换）。
    pub fail_sticky: std::sync::atomic::AtomicBool,
    commits: AtomicU64,
    /// 提交栅栏（Stop-drain 确定性 Gate 用）：`Some` 时 writer 在执行下一个
    /// commit 之前阻塞，直到测试放行。One-shot：仅暂停紧随其后的第一个
    /// commit，取走后后续 commit 不受影响。
    commit_gate: Mutex<Option<Arc<CommitGate>>>,
}

/// 提交栅栏（`test-hooks`）：writer 线程侧状态。测试侧句柄见 [`CommitGateHandle`]。
#[cfg(feature = "test-hooks")]
#[derive(Debug)]
struct CommitGate {
    /// writer 到达栅栏的累计次数（单调；测试据此确认 commit 已被暂停，
    /// 而不是"还没发出来"，栅栏等待才是确定性的）。
    entered: AtomicU64,
    /// 放行通道接收端（writer 侧阻塞等待；测试 abandon 时发送端释放，
    /// `recv` 返回 Err 即直接放行，writer 永不因测试悬挂）。
    release_rx: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

/// 提交栅栏测试句柄（`test-hooks`）：由 [`StoreFaults::arm_commit_gate`] 创建。
#[cfg(feature = "test-hooks")]
#[derive(Debug)]
pub struct CommitGateHandle {
    gate: Arc<CommitGate>,
    release_tx: std::sync::mpsc::Sender<()>,
}

#[cfg(feature = "test-hooks")]
impl CommitGateHandle {
    /// writer 到达栅栏的累计次数（`>= 1` 即当前 commit 已被暂停，
    /// 此时并发 Stop 必定落在"in-flight commit 未完成"窗口内）。
    pub fn entered(&self) -> u64 {
        self.gate.entered.load(Ordering::SeqCst)
    }

    /// 放行被暂停的 commit（消费句柄；放行后 writer 继续执行该 commit，
    /// commit-then-publish 原子性不受影响）。
    pub fn release(self) {
        let _ = self.release_tx.send(());
    }
}

#[cfg(feature = "test-hooks")]
impl StoreFaults {
    /// 构造故障器：第 `at` 个 commit 起失败（1-based；0 = 永不失败）；
    /// `sticky` 为 true 则持续失败直到测试改回（`fail_sticky` 是公开原子量）。
    pub fn new(at: u64, sticky: bool) -> Self {
        Self {
            fail_commit_at: AtomicU64::new(at),
            fail_sticky: std::sync::atomic::AtomicBool::new(sticky),
            commits: AtomicU64::new(0),
            commit_gate: Mutex::new(None),
        }
    }

    /// 布防提交栅栏：writer 收到的下一个 Commit 在执行前阻塞，
    /// 直到句柄 [`CommitGateHandle::release`] 放行（或句柄释放，
    /// 此时 writer 直接放行，永不悬挂）。测试流程：
    /// 布防 → 触发 commit → 等 `entered() >= 1` → 并发 Stop →
    /// 断言 Stop 未返回（正在等 in-flight commit）→ 放行 →
    /// Stop 成功 + DB/Hub 精确 + Stop 后无新写。
    /// 注意：暂停的是共享 writer 线程，布防期间同库其他 commit 同样等待；
    /// Gate 测试内只跑单个 endpoint，隔离由测试保证。
    pub fn arm_commit_gate(self: &Arc<Self>) -> CommitGateHandle {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let gate = Arc::new(CommitGate {
            entered: AtomicU64::new(0),
            release_rx: Mutex::new(Some(release_rx)),
        });
        *self.commit_gate.lock().unwrap() = Some(Arc::clone(&gate));
        CommitGateHandle { gate, release_tx }
    }

    /// 取走已布防的栅栏（writer 线程调用，one-shot）。
    fn take_commit_gate(&self) -> Option<Arc<CommitGate>> {
        self.commit_gate.lock().unwrap().take()
    }

    fn check(&self) -> Result<(), EventStoreError> {
        let at = self.fail_commit_at.load(Ordering::Relaxed);
        if at == 0 {
            return Ok(());
        }
        let n = self.commits.fetch_add(1, Ordering::Relaxed) + 1;
        let hit = if self.fail_sticky.load(Ordering::Relaxed) {
            n >= at
        } else {
            n == at
        };
        if hit {
            // SQLITE_FULL（13）：调用方（ingress）按 Fatal 归一为 StoreUnavailable，
            // 与真实磁盘满路径一致（整批回滚，不污染）。
            Err(EventStoreError::Fatal(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(13),
                Some("disk full (injected fault)".into()),
            )))
        } else {
            Ok(())
        }
    }
}

/// Writer 主循环（blocking 线程内运行）。发送端全部释放即退出。
/// 默认构建：Commit 直落 `commit_batch`，无故障检查。
/// `commit_latency_max_ns`：每次 commit 执行耗时（不含队列等待）最大值，
/// stall 位置判定用（ingress 测到的大延迟若此处很小，说明堵在队列/调度）。
pub fn writer_loop(
    mut conn: Connection,
    mut rx: mpsc::Receiver<WriteCommand>,
    commit_latency_max_ns: Arc<AtomicU64>,
) {
    #[cfg(feature = "test-hooks")]
    let mut faults: Option<Arc<StoreFaults>> = None;
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            WriteCommand::Commit(req, reply) => {
                let exec_start = std::time::Instant::now();
                #[cfg(feature = "test-hooks")]
                let res = match faults.as_ref().map(|f| f.check()).unwrap_or(Ok(())) {
                    Ok(()) => {
                        // 提交栅栏（one-shot）：暂停紧随其后的第一个 commit，
                        // 让并发 Stop 确定性落在 in-flight 窗口内。
                        if let Some(gate) = faults.as_ref().and_then(|f| f.take_commit_gate()) {
                            gate.entered.fetch_add(1, Ordering::SeqCst);
                            let _ = gate.release_rx.lock().unwrap().take().map(|rx| rx.recv());
                        }
                        commit_batch(&mut conn, &req)
                    }
                    Err(e) => Err(e),
                };
                #[cfg(not(feature = "test-hooks"))]
                let res = commit_batch(&mut conn, &req);
                record_max_ns(&commit_latency_max_ns, exec_start.elapsed().as_nanos());
                let _ = reply.send(res);
            }
            WriteCommand::Purge {
                before_seq,
                batch_limit,
                reply,
            } => {
                let _ = reply.send(purge_batch(&mut conn, before_seq, batch_limit));
            }
            #[cfg(feature = "test-hooks")]
            WriteCommand::SetFaults(f) => {
                faults = Some(f);
            }
        }
    }
}

/// 最大值收敛（诊断计数用；writer 单线程写，调用方 low contention）。
fn record_max_ns(target: &AtomicU64, elapsed: u128) {
    let v = elapsed.min(u128::from(u64::MAX)) as u64;
    let mut cur = target.load(Ordering::Relaxed);
    while v > cur {
        match target.compare_exchange_weak(cur, v, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

/// 小批量 purge（调用方需保证单线程调用——writer 线程独占连接）。
/// 单次最多删 `batch_limit` 行（长写事务拆小，避免阻塞 commit）；
/// 调用方（retention sweeper）循环调用直到返回 0。
fn purge_batch(
    conn: &mut Connection,
    before_seq: i64,
    batch_limit: u64,
) -> Result<usize, EventStoreError> {
    let n = conn.execute(
        "DELETE FROM events WHERE seq IN (\
             SELECT seq FROM events WHERE seq < ?1 ORDER BY seq LIMIT ?2)",
        rusqlite::params![before_seq, batch_limit as i64],
    )?;
    Ok(n)
}

/// 整批提交（调用方需保证单线程调用——writer 线程独占连接）。
fn commit_batch(
    conn: &mut Connection,
    req: &CommitRequest,
) -> Result<CommitResult, EventStoreError> {
    // 防御性校验：publish/SDK 侧已验过，这里是"不信任上游"的第二道门。
    // 任一坏记录 → 整批拒绝（不入库、不污染），与 collision 同属硬失败。
    for ev in &req.batch.events {
        ev.validate()
            .map_err(|e| EventStoreError::InvalidRecord(format!("{}: {e}", ev.event_id)))?;
    }

    let tx = conn.transaction()?;
    let mut inserted = Vec::new();
    let mut duplicates = 0u64;
    // 空批：开销一次空事务（语义清晰，调用方可无条件 commit；代价可忽略）
    for (index, ev) in req.batch.events.iter().enumerate() {
        let hash = event_payload_hash(ev)?;
        let attrs = serde_json::to_string(&ev.attributes)
            .map_err(|e| EventStoreError::Encode(format!("attributes: {e}")))?;
        let cond = ev.condition.as_ref();
        let changes = tx.execute(
            schema::INSERT_SQL,
            params![
                req.endpoint_id,
                ev.event_id,
                req.batch.connection_handle as i64,
                schema::epoch_to_sql(req.batch.stream_epoch),
                // batch_sequence 现实范围远小于 i64::MAX（每 epoch 从 1 起）；
                // 若未来序号空间变化，此处 cast 即需复审（单测 epoch 覆盖 u64 侧）。
                req.batch.sequence as i64,
                index as i64,
                ev.category,
                ev.kind,
                ev.source,
                ev.severity as i64,
                ev.code,
                ev.message,
                ev.message_locale,
                ev.occurred_at_ns,
                req.batch.timestamp_ns,
                req.received_at_ns,
                cond.map(|c| c.condition_id.clone()),
                cond.map(|c| schema::transition_str(c.transition)),
                cond.and_then(|c| c.active).map(i64::from),
                cond.and_then(|c| c.acknowledged).map(i64::from),
                cond.and_then(|c| c.confirmed).map(i64::from),
                cond.and_then(|c| c.retain).map(i64::from),
                ev.correlation_id,
                attrs,
                hash,
            ],
        )?;
        if changes == 1 {
            let seq = tx.last_insert_rowid();
            inserted.push(StoredEvent {
                seq,
                endpoint_id: req.endpoint_id.clone(),
                event_id: ev.event_id.clone(),
                connection_handle: req.batch.connection_handle,
                stream_epoch: req.batch.stream_epoch,
                batch_sequence: req.batch.sequence,
                event_index: index as u32,
                category: ev.category.clone(),
                kind: ev.kind.clone(),
                source: ev.source.clone(),
                severity: ev.severity,
                code: ev.code.clone(),
                message: ev.message.clone(),
                message_locale: ev.message_locale.clone(),
                occurred_at_ns: ev.occurred_at_ns,
                published_at_ns: req.batch.timestamp_ns,
                received_at_ns: req.received_at_ns,
                condition_id: cond.map(|c| c.condition_id.clone()),
                transition: cond.map(|c| schema::transition_str(c.transition).to_string()),
                active: cond.and_then(|c| c.active),
                acknowledged: cond.and_then(|c| c.acknowledged),
                confirmed: cond.and_then(|c| c.confirmed),
                retain: cond.and_then(|c| c.retain),
                correlation_id: ev.correlation_id.clone(),
                attributes_json: attrs,
                payload_hash: hash,
            });
        } else {
            // 精确冲突：查已有 hash 定性（dedup vs collision）
            let existing: String = tx.query_row(
                "SELECT payload_hash FROM events WHERE endpoint_id = ?1 AND event_id = ?2",
                params![req.endpoint_id, ev.event_id],
                |r| r.get(0),
            )?;
            if existing == hash {
                duplicates += 1;
            } else {
                // 整批回滚：tx drop 即 rollback（未 commit），错误携带精确身份
                return Err(EventStoreError::IdCollision {
                    endpoint_id: req.endpoint_id.clone(),
                    event_id: ev.event_id.clone(),
                });
            }
        }
    }
    tx.commit()?;
    Ok(CommitResult {
        inserted,
        duplicates,
    })
}
