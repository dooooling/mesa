//! 历史查询（v1.1 §16）：只读已提交行。
//!
//! - 排序恒为 `seq DESC`（最新在前）；`after_seq`/`before_seq` 做游标分页，
//!   无重复、无遗漏；
//! - `from_ns`/`to_ns` 作用于 `received_at_ns`（恒存在、近似单调；
//!   `occurred_at_ns` 可为 NULL，不适合做分页基准）；
//! - `active` 过滤只匹配显式值（NULL 行不参与相等比较，SQL 语义）；
//! - `limit` 默认 100、上限 500（v1.1 冻结）。

use rusqlite::{Connection, ToSql};

use crate::EventStoreError;
use crate::schema::{self, StoredEvent};

/// 历史查询过滤器（v1.1 §16 全字段）。
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub endpoint_id: Option<String>,
    pub category: Option<String>,
    pub kind: Option<String>,
    pub severity_min: Option<u16>,
    pub code: Option<String>,
    pub condition_id: Option<String>,
    pub active: Option<bool>,
    pub from_ns: Option<i64>,
    pub to_ns: Option<i64>,
    pub before_seq: Option<i64>,
    pub after_seq: Option<i64>,
    pub limit: Option<u32>,
}

pub const HISTORY_LIMIT_DEFAULT: u32 = 100;
pub const HISTORY_LIMIT_MAX: u32 = 500;

/// 查询历史，返回 `(rows, next_cursor)`。`next_cursor = Some(last_seq)`
/// 当且仅当还有更多（取满一页）；调用方用它做 `before_seq` 继续翻页。
pub fn query_history(
    conn: &Connection,
    filter: &EventFilter,
) -> Result<(Vec<StoredEvent>, Option<i64>), EventStoreError> {
    let limit = filter
        .limit
        .unwrap_or(HISTORY_LIMIT_DEFAULT)
        .clamp(1, HISTORY_LIMIT_MAX) as i64;
    let mut sql = format!("SELECT {} FROM events", schema::SELECT_COLS);
    let mut conds: Vec<String> = Vec::new();
    // Box 装箱：各条件类型不同，统一为 trait object 供 rusqlite 绑定
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();

    macro_rules! cond {
        ($clause:expr, $v:expr) => {{
            conds.push($clause.to_string());
            args.push(Box::new($v));
        }};
    }
    // 裸 `?` 按位置绑定（与 args 压入顺序一致），无需显式编号
    if let Some(v) = &filter.endpoint_id {
        cond!("endpoint_id = ?", v.clone());
    }
    if let Some(v) = &filter.category {
        cond!("category = ?", v.clone());
    }
    if let Some(v) = &filter.kind {
        cond!("kind = ?", v.clone());
    }
    if let Some(v) = filter.severity_min {
        cond!("severity >= ?", v as i64);
    }
    if let Some(v) = &filter.code {
        cond!("code = ?", v.clone());
    }
    if let Some(v) = &filter.condition_id {
        cond!("condition_id = ?", v.clone());
    }
    if let Some(v) = filter.active {
        cond!("active = ?", i64::from(v));
    }
    if let Some(v) = filter.from_ns {
        cond!("received_at_ns >= ?", v);
    }
    if let Some(v) = filter.to_ns {
        cond!("received_at_ns <= ?", v);
    }
    if let Some(v) = filter.before_seq {
        cond!("seq < ?", v);
    }
    if let Some(v) = filter.after_seq {
        cond!("seq > ?", v);
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    // after_seq 语义是"游标之后的新行"：仍按 DESC 取页，调用方可反转展示；
    // SSE replay（ASC 需求）由专用查询承载，不复用本函数（见 step ⑦）。
    sql.push_str(" ORDER BY seq DESC LIMIT ?");
    args.push(Box::new(limit));

    let params: Vec<&dyn ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params.as_slice(), schema::row_to_stored)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_cursor = if rows.len() as i64 == limit {
        rows.last().map(|r| r.seq)
    } else {
        None
    };
    Ok((rows, next_cursor))
}

/// 按 seq 取单行（detail drawer 用）。无行返回 `Ok(None)`，调用方转 404。
pub fn query_by_seq(conn: &Connection, seq: i64) -> Result<Option<StoredEvent>, EventStoreError> {
    let sql = format!("SELECT {} FROM events WHERE seq = ?1", schema::SELECT_COLS);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([seq], schema::row_to_stored)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// SSE replay 页：`(after, until]` 区间按 ASC 取 `limit` 条（调用方循环翻页
/// 直到不满页；与 history 的 DESC 分页互不干扰）。
/// `until_seq = Some(h)` 把上界字面锁死在建连 high-water（初始 replay 用，
/// 区间恒为 `(cursor, high_water]`）；`None` 即 `i64::MAX` 追 DB 现状
/// （live 期 gap/Lag/reconcile 用）。单条静态 SQL，无分支拼接。
pub fn query_range_asc(
    conn: &Connection,
    after_seq: i64,
    until_seq: Option<i64>,
    limit: u32,
) -> Result<Vec<StoredEvent>, EventStoreError> {
    let sql = format!(
        "SELECT {} FROM events WHERE seq > ?1 AND seq <= ?2 ORDER BY seq ASC LIMIT ?3",
        schema::SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        (after_seq, until_seq.unwrap_or(i64::MAX), limit as i64),
        schema::row_to_stored,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(EventStoreError::Fatal)
}

/// 当前最大 seq（无行则 0；live-only 连接的 high-water mark）。
pub fn max_seq(conn: &Connection) -> Result<i64, EventStoreError> {
    let v: Option<i64> = conn.query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?;
    Ok(v.unwrap_or(0))
}

/// Retention 时间阈值（⑧b）：`received_at_ns >= since` 的最小 seq，
/// 即"应保留"的第一行；`None` = 全表都比 since 旧（调用方转 max+1 全清）。
/// 用 `received_at_ns`（Core 落盘时钟，恒存在）而非 `occurred_at_ns`
/// （可为 NULL，设备时钟还可能回拨）。
pub fn min_seq_received_since(
    conn: &Connection,
    since_ns: i64,
) -> Result<Option<i64>, EventStoreError> {
    let v: Option<i64> = conn.query_row(
        "SELECT MIN(seq) FROM events WHERE received_at_ns >= ?1",
        [since_ns],
        |r| r.get(0),
    )?;
    Ok(v)
}

/// Retention 数量阈值（⑧b）：按 seq ASC 第 `offset` 行（0-based）的 seq，
/// 即跳过最老的 `offset` 行后"应保留"的第一行；`None` = 行数不足。
pub fn seq_by_asc_offset(conn: &Connection, offset: i64) -> Result<Option<i64>, EventStoreError> {
    let mut stmt = conn.prepare("SELECT seq FROM events ORDER BY seq ASC LIMIT 1 OFFSET ?1")?;
    let mut rows = stmt.query_map([offset], |r| r.get::<_, i64>(0))?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}
