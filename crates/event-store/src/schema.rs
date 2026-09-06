//! events.db schema（PR7 v1.1 §3 冻结）。
//!
//! 与 `mesa.db`（ConfigStore）彻底分离：配置是低频事务，事件是持续高频
//! append——锁、WAL、故障域必须隔离（§2）。
//!
//! 不变量（v1.1 §最高优先级）：
//! - 无 COMMIT 即无外部可见（REST/SSE 只读已提交行）；
//! - 同 `(endpoint_id, event_id)`：同 canonical payload → dedup，
//!   异 payload → `EVENT_ID_COLLISION` 整批回滚；
//! - 不 FK 到 ConfigStore：删设备配置不得抹掉"以前发生过什么"。

use mesa_core_types::ConditionTransition;

/// events.db schema 版本（meta 表）。V1 无迁移；未来加表/列走显式迁移。
pub const EVENT_SCHEMA_VERSION: i64 = 1;

/// 建表 DDL（幂等）。列顺序/定义按 v1.1 §3，外加 `payload_hash`（§1）与
/// `meta` 版本表。
pub const SCHEMA_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,

    endpoint_id TEXT NOT NULL,
    event_id TEXT NOT NULL,

    connection_handle INTEGER NOT NULL,
    stream_epoch INTEGER NOT NULL,
    batch_sequence INTEGER NOT NULL,
    event_index INTEGER NOT NULL,

    category TEXT NOT NULL,
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    severity INTEGER NOT NULL,

    code TEXT,
    message TEXT,
    message_locale TEXT,

    occurred_at_ns INTEGER,
    published_at_ns INTEGER NOT NULL,
    received_at_ns INTEGER NOT NULL,

    condition_id TEXT,
    transition TEXT,

    active INTEGER,
    acknowledged INTEGER,
    confirmed INTEGER,
    retain INTEGER,

    correlation_id TEXT,

    attributes_json TEXT NOT NULL,
    payload_hash TEXT NOT NULL,

    UNIQUE(endpoint_id, event_id)
);
CREATE INDEX IF NOT EXISTS idx_events_endpoint_seq
    ON events(endpoint_id, seq DESC);
CREATE INDEX IF NOT EXISTS idx_events_received
    ON events(received_at_ns DESC);
CREATE INDEX IF NOT EXISTS idx_events_category
    ON events(category, seq DESC);
CREATE INDEX IF NOT EXISTS idx_events_severity
    ON events(severity, seq DESC);
CREATE INDEX IF NOT EXISTS idx_events_condition
    ON events(condition_id, seq DESC);
"#;

/// 已持久化的事件行（`StoredEvent`，v1.1 §5）：**永远携带 DB seq**。
/// EventHub 只传播本结构，禁止传播还没有 seq 的原始 EventRecord——
/// SSE `id:` / replay 游标 / history 分页全部以 `seq` 为唯一真值。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    /// Mesa 全局持久游标（AUTOINCREMENT）。与 Driver `batch_sequence` 严格区分：
    /// 后者是流完整性语言，前者是分页/replay 语言。
    pub seq: i64,
    pub endpoint_id: String,
    pub event_id: String,
    pub connection_handle: u32,
    pub stream_epoch: u64,
    pub batch_sequence: u64,
    pub event_index: u32,
    pub category: String,
    pub kind: String,
    pub source: String,
    pub severity: u16,
    pub code: Option<String>,
    pub message: Option<String>,
    pub message_locale: Option<String>,
    pub occurred_at_ns: Option<i64>,
    pub published_at_ns: i64,
    pub received_at_ns: i64,
    pub condition_id: Option<String>,
    /// 契约固定字符串（`raised/updated/acknowledged/confirmed/cleared`），
    /// 显式映射（见 [`transition_str`]），不依赖 serde 偶然输出。
    pub transition: Option<String>,
    pub active: Option<bool>,
    pub acknowledged: Option<bool>,
    pub confirmed: Option<bool>,
    pub retain: Option<bool>,
    pub correlation_id: Option<String>,
    /// attributes 原始 typed 载荷的 compact JSON（展示用；哈希用 canonical 形式）。
    pub attributes_json: String,
    pub payload_hash: String,
}

/// `ConditionTransition` → 契约固定字符串（PR5 §1 生命周期词汇）。
pub fn transition_str(t: ConditionTransition) -> &'static str {
    match t {
        ConditionTransition::Raised => "raised",
        ConditionTransition::Updated => "updated",
        ConditionTransition::Acknowledged => "acknowledged",
        ConditionTransition::Confirmed => "confirmed",
        ConditionTransition::Cleared => "cleared",
    }
}

/// INSERT 列（与 [`INSERT_SQL`] 占位符一一对应）。
pub const INSERT_SQL: &str = r#"
INSERT INTO events(
    endpoint_id, event_id,
    connection_handle, stream_epoch, batch_sequence, event_index,
    category, kind, source, severity,
    code, message, message_locale,
    occurred_at_ns, published_at_ns, received_at_ns,
    condition_id, transition,
    active, acknowledged, confirmed, retain,
    correlation_id,
    attributes_json, payload_hash
) VALUES(
    ?1, ?2,
    ?3, ?4, ?5, ?6,
    ?7, ?8, ?9, ?10,
    ?11, ?12, ?13,
    ?14, ?15, ?16,
    ?17, ?18,
    ?19, ?20, ?21, ?22,
    ?23,
    ?24, ?25
) ON CONFLICT(endpoint_id, event_id) DO NOTHING
"#;

/// SELECT 投影列（顺序固定，供 [`row_to_stored`] 与 query 共用）。
pub const SELECT_COLS: &str = "seq, endpoint_id, event_id, \
    connection_handle, stream_epoch, batch_sequence, event_index, \
    category, kind, source, severity, \
    code, message, message_locale, \
    occurred_at_ns, published_at_ns, received_at_ns, \
    condition_id, transition, \
    active, acknowledged, confirmed, retain, \
    correlation_id, attributes_json, payload_hash";

/// u64 stream_epoch → SQLite INTEGER（i64）。
///
/// epoch 是全范围 u64 随机值（`new_stream_epoch` XOR 混合），约一半概率超过
/// `i64::MAX`——直接存会失败。这里用**位保持重解释**（wrapping cast）：SQLite
/// INTEGER 本就是 64 bit 模式，读回时反向 cast 即精确还原（epoch 只做相等
/// 比较，从不排序/范围查，符号无意义）。往返有单测锁定。
pub fn epoch_to_sql(epoch: u64) -> i64 {
    epoch as i64
}

/// 逆变换，见 [`epoch_to_sql`]。
pub fn epoch_from_sql(v: i64) -> u64 {
    v as u64
}

fn opt_bool(v: Option<i64>) -> Option<bool> {
    v.map(|x| x != 0)
}

/// 行映射（列顺序必须与 [`SELECT_COLS`] 一致）。
pub fn row_to_stored(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    Ok(StoredEvent {
        seq: r.get(0)?,
        endpoint_id: r.get(1)?,
        event_id: r.get(2)?,
        connection_handle: r.get::<_, i64>(3)? as u32,
        stream_epoch: epoch_from_sql(r.get(4)?),
        batch_sequence: r.get::<_, i64>(5)? as u64,
        event_index: r.get::<_, i64>(6)? as u32,
        category: r.get(7)?,
        kind: r.get(8)?,
        source: r.get(9)?,
        severity: r.get::<_, i64>(10)? as u16,
        code: r.get(11)?,
        message: r.get(12)?,
        message_locale: r.get(13)?,
        occurred_at_ns: r.get(14)?,
        published_at_ns: r.get(15)?,
        received_at_ns: r.get(16)?,
        condition_id: r.get(17)?,
        transition: r.get(18)?,
        active: opt_bool(r.get(19)?),
        acknowledged: opt_bool(r.get(20)?),
        confirmed: opt_bool(r.get(21)?),
        retain: opt_bool(r.get(22)?),
        correlation_id: r.get(23)?,
        attributes_json: r.get(24)?,
        payload_hash: r.get(25)?,
    })
}
