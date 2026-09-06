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
}

/// Writer 主循环（blocking 线程内运行）。发送端全部释放即退出。
pub fn writer_loop(mut conn: Connection, mut rx: mpsc::Receiver<WriteCommand>) {
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            WriteCommand::Commit(req, reply) => {
                let _ = reply.send(commit_batch(&mut conn, &req));
            }
            WriteCommand::Purge {
                before_seq,
                batch_limit,
                reply,
            } => {
                let _ = reply.send(purge_batch(&mut conn, before_seq, batch_limit));
            }
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
