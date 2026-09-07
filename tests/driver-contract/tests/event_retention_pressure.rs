//! Retention 并发门（PR10 commit 6 §13）：sweeper 与持续写入/历史查询
//! 并发时无错误、无死锁、无损坏；数量上限最终收敛；purge 计数可观测。
//! 走生产 `run_retention_loop`（非测试替身）。

mod common;
mod event_common;

use std::sync::Arc;

use event_common::{commit, event_batch, rows_of, tmp_db};
use mesa_event_store::retention::{run_retention_loop, sweep_once};
use mesa_event_store::{EventDiagnostics, EventStore, RetentionConfig};

/// 400 行持续写入 + 历史查询并发 + sweeper（max 100，1s 间隔）：
/// 全程零错误；结束补一次 sweep 后行数 ≤100；purge 计数 >0；残留行有效。
#[tokio::test]
async fn retention_sweep_concurrent_with_writes_and_reads() {
    let db = tmp_db("retention");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let diagnostics = Arc::new(EventDiagnostics::default());

    // 生产 sweeper：只按数量删（时间窗关掉，保证确定性）
    let shutdown = tokio_util::sync::CancellationToken::new();
    let sweeper = tokio::spawn(run_retention_loop(
        Arc::clone(&store),
        Arc::clone(&diagnostics),
        RetentionConfig {
            retention_days: 0,
            max_records: 100,
            interval_secs: 1,
            purge_batch: 50,
        },
        shutdown.clone(),
    ));

    // 写：400 行，每 10 行让步（给 sweeper 交错窗口）；写到一半时等待
    // sweeper 至少跑过一次（purge 计数 >0），否则写太快、1s 首 tick 还没到
    // 就写完，测不到真并发。
    let ep = "hd-retention";
    let writer = tokio::spawn({
        let store = Arc::clone(&store);
        let diagnostics = Arc::clone(&diagnostics);
        async move {
            for i in 0..400u64 {
                commit(
                    &store,
                    ep,
                    event_batch(
                        1,
                        1,
                        i + 1,
                        vec![event_common::sim_record(&format!("rt-{i}"), i)],
                    ),
                )
                .await
                .expect("并发 purge 下写入不得失败");
                if i == 199 {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                    loop {
                        let p = diagnostics
                            .retention_purged_total
                            .load(std::sync::atomic::Ordering::Relaxed);
                        if p > 0 {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < deadline,
                            "sweeper 15s 内未跑过一次"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
                if i % 10 == 9 {
                    tokio::task::yield_now().await;
                }
            }
        }
    });
    // 读：50 次历史查询全程必须 Ok（purge 并发下无 SQLITE 错误）
    let reader = tokio::spawn({
        let store = Arc::clone(&store);
        async move {
            for _ in 0..50 {
                let rows = rows_of(&store, ep);
                assert!(rows.len() <= 400);
                tokio::task::yield_now().await;
            }
        }
    });
    writer.await.unwrap();
    reader.await.unwrap();

    // 收尾：停 sweeper + 补一次 sweep → 精确收敛到上限
    shutdown.cancel();
    sweeper.await.unwrap();
    let purged = sweep_once(
        &store,
        &RetentionConfig {
            retention_days: 0,
            max_records: 100,
            interval_secs: 1,
            purge_batch: 50,
        },
    )
    .await
    .expect("收尾 sweep 不得失败");
    let _ = purged;
    let rows = rows_of(&store, ep);
    assert!(rows.len() <= 100, "数量上限必须收敛，got {}", rows.len());
    assert!(!rows.is_empty(), "sweep 不得清空表（只删超量最老部分）");
    // 残留有效：seq 严格递增唯一（purge 按 seq 踢最老，无洞是设计，
    // 此处锁"无损坏无重复"即可——删的是前缀，残留连续）
    let mut seqs: Vec<i64> = rows.iter().map(|r| r.seq).collect();
    seqs.sort_unstable();
    for w in seqs.windows(2) {
        assert!(w[0] < w[1], "残留 seq 必须唯一递增");
    }
    let total = diagnostics
        .retention_purged_total
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(total > 0, "purge 计数必须可观测");
    let _ = std::fs::remove_file(&db);
}

/// 并发 purge 不得误删未超量表：200 行 < 上限时 sweep 全程 0 删除。
#[tokio::test]
async fn retention_sweep_never_deletes_below_limit() {
    let db = tmp_db("retention-quiet");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let ep = "hd-retention-quiet";
    for i in 0..200u64 {
        commit(
            &store,
            ep,
            event_batch(
                1,
                1,
                i + 1,
                vec![event_common::sim_record(&format!("rt-{i}"), i)],
            ),
        )
        .await
        .unwrap();
    }
    // 并发写 + sweep 交错：上限 1000，删 0 行
    let writer = tokio::spawn({
        let store = Arc::clone(&store);
        async move {
            for i in 200..300u64 {
                commit(
                    &store,
                    ep,
                    event_batch(
                        1,
                        1,
                        i + 1,
                        vec![event_common::sim_record(&format!("rt-{i}"), i)],
                    ),
                )
                .await
                .unwrap();
                tokio::task::yield_now().await;
            }
        }
    });
    for _ in 0..5 {
        let n = sweep_once(
            &store,
            &RetentionConfig {
                retention_days: 0,
                max_records: 1000,
                interval_secs: 1,
                purge_batch: 50,
            },
        )
        .await
        .unwrap();
        assert_eq!(n, 0, "未超量不得删");
    }
    writer.await.unwrap();
    assert_eq!(rows_of(&store, ep).len(), 300, "一行不得少");
    let _ = std::fs::remove_file(&db);
}

/// 慢读隔离：读连接在 purge 期间看到一致快照（不报错、不读到半删页）。
#[tokio::test]
async fn retention_reads_stay_consistent_during_purge() {
    let db = tmp_db("retention-read");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let ep = "hd-retention-read";
    for i in 0..500u64 {
        commit(
            &store,
            ep,
            event_batch(
                1,
                1,
                i + 1,
                vec![event_common::sim_record(&format!("rt-{i}"), i)],
            ),
        )
        .await
        .unwrap();
    }
    // purge 与翻页读交错 20 轮：每次读必须自洽（seq 唯一递增）
    for _ in 0..20 {
        let n = sweep_once(
            &store,
            &RetentionConfig {
                retention_days: 0,
                max_records: 100,
                interval_secs: 1,
                purge_batch: 25,
            },
        )
        .await
        .unwrap();
        let _ = n;
        let rows = rows_of(&store, ep);
        let mut seqs: Vec<i64> = rows.iter().map(|r| r.seq).collect();
        seqs.sort_unstable();
        for w in seqs.windows(2) {
            assert!(w[0] < w[1], "purge 并发读必须自洽");
        }
    }
    let rows = rows_of(&store, ep);
    assert!(rows.len() <= 100, "最终收敛，got {}", rows.len());
    let _ = std::fs::remove_file(&db);
}
