//! Retention sweeper（⑧b，v1.1 §17 冻结默认值）。
//!
//! - 时间窗 `retention_days`（默认 30 天）：按 `received_at_ns` 界定超窗行。
//! - 数量上限 `max_records`（默认 1_000_000 行）：超量删最老的行。
//! - 节奏 `interval_secs`（默认 600s）：purge 按 `purge_batch` 分批走 writer 通道。
//! - sweep 尾调 `checkpoint_passive` 收 WAL，单次失败只 warn 下次 tick 重试。
//!
//! 无 CLI flag（冻结默认；调参需求出现时再加 flag，不改变本语义）。

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::{EventDiagnostics, EventStore, query};

/// Retention 配置（v1.1 §17 冻结默认）。
#[derive(Debug, Clone)]
pub struct RetentionConfig {
    /// 时间窗（天）。0 = 不按时间删（单测/特殊场景用）。
    pub retention_days: u32,
    /// 总行数上限。0 = 不按数量删。
    pub max_records: u64,
    /// sweep 间隔（秒）。
    pub interval_secs: u64,
    /// 单次 purge 上限（行）。
    pub purge_batch: u64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            retention_days: 30,
            max_records: 1_000_000,
            interval_secs: 600,
            purge_batch: 1000,
        }
    }
}

/// Retention 主循环（mesad spawn，`shutdown` 触发即退）。
/// P1-1：每次 sweep 的删除数累积进 `diagnostics.retention_purged_total`
///（`GET /events/stats` 可见）。
pub async fn run_retention_loop(
    store: Arc<EventStore>,
    diagnostics: Arc<EventDiagnostics>,
    config: RetentionConfig,
    shutdown: CancellationToken,
) {
    tracing::info!(
        days = config.retention_days,
        max_records = config.max_records,
        interval_secs = config.interval_secs,
        "event retention sweeper started"
    );
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(config.interval_secs)) => {}
        }
        match sweep_once(&store, &config).await {
            Ok(n) => {
                diagnostics
                    .retention_purged_total
                    .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) => {
                // maintenance 失败不升级：下次 tick 重试（fail-open 只影响磁盘占用，
                // 不影响写入可用性；磁盘打满由部署层监控告警）。
                tracing::warn!("event retention sweep failed: {e}");
            }
        }
    }
    tracing::info!("event retention sweeper stopped");
}

/// 单次 sweep：时间窗 → 数量上限 → WAL checkpoint。返回累计删除行数。
pub async fn sweep_once(
    store: &Arc<EventStore>,
    config: &RetentionConfig,
) -> Result<u64, crate::EventStoreError> {
    let mut deleted = 0u64;
    // ① 时间窗：cutoff 之前收到的行可删（按 received_at_ns，见 query 注释）
    if config.retention_days > 0 {
        let cutoff_ns = mesa_core_types::now_unix_ns()
            .saturating_sub(config.retention_days as i64 * 86_400_000_000_000);
        let keep_from = blocking(store, move |s| {
            let conn = s.reader_conn();
            query::min_seq_received_since(&conn, cutoff_ns)
        })
        .await?;
        let before_seq = match keep_from {
            Some(s) => s,
            // 全表都比 cutoff 旧：before = max+1，全清
            None => blocking(store, |s| s.max_seq()).await?.saturating_add(1),
        };
        deleted += purge_until_empty(store, before_seq, config.purge_batch).await?;
    }
    // ② 数量上限：超量部分删最老的
    if config.max_records > 0 {
        let rows = blocking(store, |s| s.stats().map(|st| st.rows)).await?;
        if rows > config.max_records {
            let excess = rows - config.max_records;
            let keep_from = blocking(store, move |s| {
                let conn = s.reader_conn();
                query::seq_by_asc_offset(&conn, excess as i64)
            })
            .await?;
            if let Some(keep_from) = keep_from {
                deleted += purge_until_empty(store, keep_from, config.purge_batch).await?;
            }
        }
    }
    if deleted > 0 {
        tracing::info!(deleted, "event retention sweep purged rows");
    }
    // ③ WAL 收敛（purge 留下的 WAL 帧被动回收；失败不算 sweep 失败）
    if let Err(e) = blocking(store, |s| s.checkpoint_passive()).await {
        tracing::debug!("retention checkpoint_passive failed: {e}");
    }
    Ok(deleted)
}

/// 读连接上的阻塞查询搬到 blocking 线程（sweeper 跑在 async 上下文，
/// 不直接碰 `Mutex<Connection>`；单次查询都是索引点查，spawn 开销可忽略）。
async fn blocking<F, T>(store: &Arc<EventStore>, f: F) -> Result<T, crate::EventStoreError>
where
    F: FnOnce(&EventStore) -> Result<T, crate::EventStoreError> + Send + 'static,
    T: Send + 'static,
{
    let s = store.clone();
    tokio::task::spawn_blocking(move || f(&s))
        .await
        .map_err(|e| crate::EventStoreError::Unavailable(format!("retention blocking task: {e}")))?
}

/// 分批删到 `before_seq` 之前无行为止（单次上限 `batch`，防长写事务）。
async fn purge_until_empty(
    store: &EventStore,
    before_seq: i64,
    batch: u64,
) -> Result<u64, crate::EventStoreError> {
    let mut total = 0u64;
    loop {
        let n = store.purge_before_seq(before_seq, batch).await?;
        total += n as u64;
        if n == 0 {
            break;
        }
    }
    Ok(total)
}
