//! Mesa EventStore（PR7 v1.1）：运行期事件历史的持久事实库。
//!
//! 写入模型：持续高频 append，独占 `events.db`（与配置库 `mesa.db` 分离）。
//! 写路径是专用 Writer Actor（独立 blocking 线程 + 独占写连接），Tokio
//! runtime 永不直接碰 SQLite；读路径是独立读连接（WAL 下不阻塞 writer）。
//!
//! 冻结语义（v1.1）：整批一事务；canonical payload 哈希去重；同 id 异内容
//! 整批回滚（`EVENT_ID_COLLISION`）；先 COMMIT 后外部可见。

mod hub;
mod query;
mod schema;
mod writer;

pub use hub::{EVENT_HUB_CAPACITY, EventHub};
pub use query::{EventFilter, max_seq, query_by_seq, query_history, query_range_asc};
pub use schema::{EVENT_SCHEMA_VERSION, StoredEvent, transition_str};
pub use writer::{CommitRequest, CommitResult, EventStoreStats};

use std::path::Path;
use std::sync::{Arc, Mutex};

use mesa_core_types::{EventRecord, Value};
use rusqlite::Connection;

/// EventStore 错误。`IdCollision` 与 `InvalidRecord` 是精确业务失败
/// （调用方按变体路由）；`Unavailable`（writer 线程已死）与 `Fatal`
/// 走 fail-closed（`EVENT_STORE_UNAVAILABLE`）。
#[derive(Debug, thiserror::Error)]
pub enum EventStoreError {
    #[error("EVENT_RECORD_INVALID: {0}")]
    InvalidRecord(String),
    #[error("EVENT_ID_COLLISION: endpoint {endpoint_id} event {event_id}")]
    IdCollision {
        endpoint_id: String,
        event_id: String,
    },
    #[error("EVENT_STORE_UNAVAILABLE: {0}")]
    Unavailable(String),
    #[error("event store fatal: {0}")]
    Fatal(#[from] rusqlite::Error),
    #[error("event store encode: {0}")]
    Encode(String),
}

// ---------------------------------------------------------------------------
// Canonical payload hash（v1.1 §1 冻结）
// ---------------------------------------------------------------------------

/// occurrence 语义的 canonical JSON（v1.1 冻结字段集）。
///
/// 只覆盖"发生了什么"，**明确排除**传输/存储元数据：`endpoint_id`（UNIQUE
/// 已作用域化）、`connection_handle`、`stream_epoch`、`batch_sequence`、
/// `event_index`、`published_at_ns`、`received_at_ns`、store `seq`。
/// 否则重连补发（epoch/seq/时间变化）会被误判为 `EVENT_ID_COLLISION`。
pub fn canonical_event_payload(event: &EventRecord) -> Result<serde_json::Value, EventStoreError> {
    let mut cond = serde_json::Map::new();
    if let Some(c) = &event.condition {
        cond.insert("acknowledged".into(), opt_bool_json(c.acknowledged));
        cond.insert("active".into(), opt_bool_json(c.active));
        cond.insert("condition_id".into(), str_json(&c.condition_id));
        cond.insert("confirmed".into(), opt_bool_json(c.confirmed));
        cond.insert("retain".into(), opt_bool_json(c.retain));
        cond.insert(
            "transition".into(),
            str_json(schema::transition_str(c.transition)),
        );
    }
    let mut attrs = serde_json::Map::new();
    // BTreeMap 迭代本就有序；这里仍显式逐项插入并经 sort_json 统一排序，
    // 不依赖任何"当前实现恰好有序"的偶然行为。
    let mut keys: Vec<&String> = event.attributes.keys().collect();
    keys.sort();
    for k in keys {
        attrs.insert(k.clone(), canonical_value_json(&event.attributes[k])?);
    }
    let mut o = serde_json::Map::new();
    o.insert("attributes".into(), serde_json::Value::Object(attrs));
    o.insert("category".into(), str_json(&event.category));
    o.insert(
        "code".into(),
        event
            .code
            .as_ref()
            .map(|s| str_json(s))
            .unwrap_or(serde_json::Value::Null),
    );
    o.insert(
        "condition".into(),
        if cond.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::Object(cond)
        },
    );
    o.insert(
        "correlation_id".into(),
        event
            .correlation_id
            .as_ref()
            .map(|s| str_json(s))
            .unwrap_or(serde_json::Value::Null),
    );
    o.insert("event_id".into(), str_json(&event.event_id));
    o.insert("kind".into(), str_json(&event.kind));
    o.insert(
        "message".into(),
        event
            .message
            .as_ref()
            .map(|s| str_json(s))
            .unwrap_or(serde_json::Value::Null),
    );
    o.insert(
        "message_locale".into(),
        event
            .message_locale
            .as_ref()
            .map(|s| str_json(s))
            .unwrap_or(serde_json::Value::Null),
    );
    o.insert(
        "occurred_at_ns".into(),
        event
            .occurred_at_ns
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
    );
    o.insert("severity".into(), serde_json::Value::from(event.severity));
    o.insert("source".into(), str_json(&event.source));
    Ok(serde_json::Value::Object(o))
}

fn str_json(s: &str) -> serde_json::Value {
    serde_json::Value::String(s.to_string())
}

fn opt_bool_json(b: Option<bool>) -> serde_json::Value {
    b.map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null)
}

/// `Value` → canonical JSON（冻结映射表，显式逐变体，不依赖派生 Serialize
/// 的偶然形状）。对象固定 `{"type","value"}` 两键；bytes 用小写 hex；
/// 非有限 float 直接报错（此类记录本就通不过 publish 校验，禁止静默编码）。
fn canonical_value_json(v: &Value) -> Result<serde_json::Value, CanonicalError> {
    use serde_json::Value as J;
    let mut o = serde_json::Map::new();
    let (t, val) = match v {
        Value::Bool(b) => ("bool", J::from(*b)),
        Value::I32(n) => ("i32", J::from(*n)),
        Value::U32(n) => ("u32", J::from(*n)),
        Value::I64(n) => ("i64", J::from(*n)),
        Value::U64(n) => ("u64", J::from(*n)),
        // u64 超过 JSON 安全整数范围时仍按整数编码（canonical 只要求稳定，
        // 不要求 JS 可读；SQL/REST 展示用原始 typed 载荷）。
        Value::F32(f) => ("f32", finite_json(*f as f64, "f32")?),
        Value::F64(f) => ("f64", finite_json(*f, "f64")?),
        Value::String(s) => ("string", str_json(s)),
        Value::Bytes(b) => (
            "bytes",
            str_json(&b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        ),
        Value::DateTime(ns) => ("datetime", J::from(*ns)),
        Value::BoolArray(a) => ("bool-array", J::from(a.clone())),
        Value::I32Array(a) => ("i32-array", J::from(a.clone())),
        Value::U32Array(a) => ("u32-array", J::from(a.clone())),
        Value::I64Array(a) => ("i64-array", J::from(a.clone())),
        Value::U64Array(a) => ("u64-array", J::from(a.clone())),
        Value::F32Array(a) => (
            "f32-array",
            J::from(
                a.iter()
                    .map(|f| finite_json(*f as f64, "f32"))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        ),
        Value::F64Array(a) => (
            "f64-array",
            J::from(
                a.iter()
                    .map(|f| finite_json(*f, "f64"))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        ),
        Value::StringArray(a) => ("string-array", J::from(a.clone())),
        Value::DateTimeArray(a) => ("datetime-array", J::from(a.clone())),
    };
    o.insert("type".into(), str_json(t));
    o.insert("value".into(), val);
    Ok(J::Object(o))
}

/// 小内部错误类型：canonical 编码阶段只有"非有限 float"一种失败，
/// 用字符串携带（对外统一转为 [`EventStoreError::Encode`]）。
#[derive(Debug)]
struct CanonicalError(String);

fn finite_json(f: f64, what: &str) -> Result<serde_json::Value, CanonicalError> {
    if f.is_finite() {
        Ok(serde_json::Value::from(f))
    } else {
        Err(CanonicalError(format!(
            "non-finite {what} cannot be hashed"
        )))
    }
}

/// 递归词法排序（显式执行，不依赖 serde_json::Map 后端恰好是 BTreeMap）。
fn sort_json(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<String> = m.keys().cloned().collect();
            keys.sort();
            let mut out = serde_json::Map::with_capacity(keys.len());
            for k in keys {
                // `m` 被 keys 接管前不能 move；按 key 逐个取出重排
                out.insert(k.clone(), sort_json(m[&k].clone()));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(sort_json).collect())
        }
        scalar => scalar,
    }
}

/// occurrence 语义哈希（v1.1 §1）：`lowercase hex SHA-256(canonical)`。
/// 独立函数，不依赖"当前 serde 恰好输出稳定"。
pub fn event_payload_hash(event: &EventRecord) -> Result<String, EventStoreError> {
    let sorted = sort_json(canonical_event_payload(event)?);
    let s = serde_json::to_string(&sorted)
        .map_err(|e| EventStoreError::Encode(format!("canonical stringify: {e}")))?;
    hash_str(&s)
}

fn hash_str(s: &str) -> Result<String, EventStoreError> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(s.as_bytes());
    Ok(format!("{:x}", h.finalize()))
}

/// canonical 编码失败转为 [`EventStoreError::Encode`]（`?` 在返回
/// `Result<_, EventStoreError>` 的调用方直接可用）。
impl From<CanonicalError> for EventStoreError {
    fn from(e: CanonicalError) -> Self {
        EventStoreError::Encode(e.0)
    }
}

// ---------------------------------------------------------------------------
// EventStore
// ---------------------------------------------------------------------------

/// PRAGMA 与连接调优（v1.1 冻结常量）。
const BUSY_TIMEOUT_MS: i64 = 5000;

fn tune_connection(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA journal_mode=WAL;\n\
         PRAGMA synchronous=NORMAL;\n\
         PRAGMA foreign_keys=ON;\n\
         PRAGMA busy_timeout={BUSY_TIMEOUT_MS};"
    ))
}

fn apply_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(schema::SCHEMA_DDL)?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version',?1)",
        [EVENT_SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// 运行期事件历史存储。
///
/// - 写：有界 mpsc → 专用 blocking 线程（独占写连接），`commit_batch().await`
///   在 COMMIT 落盘后才返回（commit-then-publish 的执行点）；
/// - 读：独立读连接 + `Arc<Mutex<…>>`（V1 读并发足够；未来查询负载上升后可
///   替换为读连接池，不改变本 API）。
pub struct EventStore {
    writer_tx: tokio::sync::mpsc::Sender<writer::WriteCommand>,
    reader: Arc<Mutex<Connection>>,
    writer_join: Option<std::thread::JoinHandle<()>>,
}

impl EventStore {
    fn spawn(path: Option<std::path::PathBuf>, in_memory: bool) -> Result<Self, EventStoreError> {
        // 内存库 URI 必须实例唯一：`cache=shared` 按名称共享，同名即同库——
        // 并行单测若同名会互相污染 UNIQUE 约束。
        static MEM_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mem_name = format!(
            "file:mesa-events-mem-{}-{}?mode=memory&cache=shared",
            std::process::id(),
            MEM_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        // 写连接（writer 线程独占）
        let write_conn = if in_memory {
            // 内存库：读写共享同一底层（`cache=shared`），否则 writer 的表
            // 对 reader 不可见。单测/临时使用。
            Connection::open(&mem_name)?
        } else {
            let p = path.expect("file path required");
            if let Some(parent) = p.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)
                    .map_err(|e| EventStoreError::Unavailable(format!("create events dir: {e}")))?;
            }
            Connection::open(&p)?
        };
        tune_connection(&write_conn)?;
        apply_schema(&write_conn)?;

        // 读连接（独立；WAL 下不阻塞 writer）
        let read_conn = if in_memory {
            let c = Connection::open(&mem_name)?;
            tune_connection(&c)?;
            c
        } else {
            let p = write_conn
                .path()
                .map(std::path::PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty());
            // 文件库：读连接重开同一文件（WAL 读己之写无需额外同步，
            // COMMIT 可见性由 SQLite 保证）
            match p {
                Some(p) => {
                    let c = Connection::open(&p)?;
                    tune_connection(&c)?;
                    c
                }
                None => {
                    // 内存库以外的无路径连接理论不可达；fail-closed
                    return Err(EventStoreError::Unavailable(
                        "events db has no path for reader connection".into(),
                    ));
                }
            }
        };

        let (tx, rx) = tokio::sync::mpsc::channel(writer::WRITER_QUEUE);
        let join = std::thread::Builder::new()
            .name("event-store-writer".into())
            .spawn(move || writer::writer_loop(write_conn, rx))
            .map_err(|e| EventStoreError::Unavailable(format!("spawn writer: {e}")))?;
        Ok(Self {
            writer_tx: tx,
            reader: Arc::new(Mutex::new(read_conn)),
            writer_join: Some(join),
        })
    }

    /// 打开（不存在则创建）文件库。
    pub fn open(path: &Path) -> Result<Self, EventStoreError> {
        Self::spawn(Some(path.to_path_buf()), false)
    }

    /// 内存库（单测用；读写共享 `cache=shared` 同一底层）。
    pub fn open_in_memory() -> Result<Self, EventStoreError> {
        Self::spawn(None, true)
    }

    /// 提交一批（整批一事务）。`received_at_ns` 由调用方（EventIngress）
    /// 按 batch 取一次后传入，本函数不自己取时间。
    ///
    /// 成功返回新插入行（含 seq）+ 去重计数；同 id 异内容则整批回滚并报
    /// [`EventStoreError::IdCollision`]；writer 线程已死报 `Unavailable`
    /// （调用方走 `EVENT_STORE_UNAVAILABLE` fail-closed）。
    pub async fn commit_batch(&self, req: CommitRequest) -> Result<CommitResult, EventStoreError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.writer_tx
            .send(writer::WriteCommand::Commit(req, reply_tx))
            .await
            .map_err(|_| EventStoreError::Unavailable("event writer queue closed".into()))?;
        reply_rx
            .await
            .map_err(|_| EventStoreError::Unavailable("event writer died before reply".into()))?
    }

    /// 历史查询（阻塞式；REST 层用 `spawn_blocking` 包裹，见 step ⑤）。
    pub fn query_history(
        &self,
        filter: &EventFilter,
    ) -> Result<(Vec<schema::StoredEvent>, Option<i64>), EventStoreError> {
        let conn = self.reader.lock().unwrap();
        query::query_history(&conn, filter)
    }

    /// 按 seq 取单行（阻塞式；同上 spawn_blocking 包裹）。
    pub fn query_by_seq(&self, seq: i64) -> Result<Option<schema::StoredEvent>, EventStoreError> {
        let conn = self.reader.lock().unwrap();
        query::query_by_seq(&conn, seq)
    }

    /// SSE replay 页（阻塞式；见上）。
    pub fn replay_range(
        &self,
        after_seq: i64,
        limit: u32,
    ) -> Result<Vec<schema::StoredEvent>, EventStoreError> {
        let conn = self.reader.lock().unwrap();
        query::query_range_asc(&conn, after_seq, limit)
    }

    /// 当前最大 seq（阻塞式；见上）。
    pub fn max_seq(&self) -> Result<i64, EventStoreError> {
        let conn = self.reader.lock().unwrap();
        query::max_seq(&conn)
    }

    /// 小批量 purge（retention 用）：删除 `seq < cutoff` 最多 `limit` 行，
    /// 返回实际删除数。热路径永不调用（maintenance task 见 step ⑧）。
    pub fn purge_before_seq(&self, cutoff_seq: i64, limit: u64) -> Result<usize, EventStoreError> {
        let conn = self.reader.lock().unwrap();
        // 用读连接执行 purge（WAL 下读写并发安全；maintenance 低频，
        // 不值得为此唤醒 writer 线程）。
        let n = conn.execute(
            "DELETE FROM events WHERE seq IN (\
                 SELECT seq FROM events WHERE seq < ?1 ORDER BY seq LIMIT ?2)",
            rusqlite::params![cutoff_seq, limit as i64],
        )?;
        Ok(n)
    }

    /// WAL 被动 checkpoint（maintenance 尾调用，不在热路径）。
    pub fn checkpoint_passive(&self) -> Result<(), EventStoreError> {
        let conn = self.reader.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }

    /// 诊断：行数 + 文件字节数（`page_count × page_size`）。
    pub fn stats(&self) -> Result<EventStoreStats, EventStoreError> {
        let conn = self.reader.lock().unwrap();
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(EventStoreStats {
            rows: rows.max(0) as u64,
            size_bytes: (pages.max(0) as u64).saturating_mul(size.max(0) as u64),
        })
    }
}

impl Drop for EventStore {
    fn drop(&mut self) {
        // 关闭命令通道（writer 循环结束）后 join：写操作有 busy_timeout 上限，
        // 不会无限 hanging；退出时不再写库，保证"最后一次 commit 已落盘"。
        let tx = std::mem::replace(&mut self.writer_tx, tokio::sync::mpsc::channel(1).0);
        drop(tx);
        if let Some(join) = self.writer_join.take() {
            let _ = join.join();
        }
    }
}

/// 事件面服务束（v1.1 §20）：store + hub 打包，避免 AppState/Manager
/// 堆十几个 Event 字段。诊断计数见 step ⑧（`EventDiagnostics`）。
pub struct EventServices {
    pub store: Arc<EventStore>,
    pub hub: Arc<EventHub>,
}

impl EventServices {
    pub fn new(store: Arc<EventStore>, hub: Arc<EventHub>) -> Arc<Self> {
        Arc::new(Self { store, hub })
    }
}

#[cfg(test)]
mod tests;
