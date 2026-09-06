//! ① 验收：hash 冻结规则 + 存储语义（Checkpoint A 的前置）。

use std::collections::BTreeMap;

use mesa_core_types::{
    ConditionTransition, EventBatch, EventCondition, EventRecord, Value, now_unix_ns,
};

use crate::query::EventFilter;
use crate::writer::CommitRequest;
use crate::{EventStore, EventStoreError, event_payload_hash};

fn record(id: &str) -> EventRecord {
    EventRecord {
        event_id: id.into(),
        category: "alarm".into(),
        kind: "alarm.condition".into(),
        source: "Channel1".into(),
        severity: 700,
        code: Some("SIM-100".into()),
        message: Some("overtemp".into()),
        message_locale: Some("en".into()),
        occurred_at_ns: Some(1_700_000_000_000_000_000),
        condition: Some(EventCondition {
            condition_id: "SIM-ALARM-100".into(),
            transition: ConditionTransition::Raised,
            active: Some(true),
            acknowledged: None,
            confirmed: None,
            retain: Some(true),
        }),
        correlation_id: None,
        attributes: BTreeMap::from([("code".into(), Value::String("SIM-100".into()))]),
    }
}

fn batch(seq: u64, epoch: u64, events: Vec<EventRecord>) -> EventBatch {
    EventBatch {
        connection_handle: 7,
        stream_epoch: epoch,
        sequence: seq,
        timestamp_ns: 1_700_000_000_000_000_001,
        events,
        mono_ns: Some(999),
    }
}

fn tmp_path(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "mesa-event-store-test-{}-{}-{tag}.db",
        std::process::id(),
        now_unix_ns()
    ));
    p
}

/// hash 只覆盖 occurrence 语义：epoch/published/mono 变化不影响 hash；
/// 内容变化则 hash 变化；格式为 64 位小写 hex。
#[test]
fn hash_covers_occurrence_only() {
    let h1 = event_payload_hash(&record("e1")).unwrap();
    assert_eq!(h1.len(), 64);
    assert!(
        h1.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );

    // 同一 occurrence 换 epoch/batch/published（传输元数据）→ 同 hash
    let mut r2 = record("e1");
    r2.occurred_at_ns = Some(1_700_000_000_000_000_000);
    assert_eq!(event_payload_hash(&r2).unwrap(), h1);

    // 内容变化 → hash 变化
    let mut r3 = record("e1");
    r3.message = Some("different".into());
    assert_ne!(event_payload_hash(&r3).unwrap(), h1);
    let mut r4 = record("e1");
    r4.condition.as_mut().unwrap().transition = ConditionTransition::Cleared;
    assert_ne!(event_payload_hash(&r4).unwrap(), h1);

    // attributes 插入顺序不影响（BTreeMap + 显式排序双保险）
    let mut r5 = record("e1");
    r5.attributes = BTreeMap::from([
        ("z".into(), Value::I32(1)),
        ("a".into(), Value::I32(2)),
        ("code".into(), Value::String("SIM-100".into())),
    ]);
    let mut r6 = record("e1");
    r6.attributes = BTreeMap::from([
        ("code".into(), Value::String("SIM-100".into())),
        ("a".into(), Value::I32(2)),
        ("z".into(), Value::I32(1)),
    ]);
    // r5/r6 与 r1 attributes 不同故与 h1 不同，但彼此必须相同
    assert_eq!(
        event_payload_hash(&r5).unwrap(),
        event_payload_hash(&r6).unwrap()
    );
}

/// commit → query 往返：字段全保留，occurred None 保持 None（禁止补造）。
#[tokio::test]
async fn commit_query_roundtrip() {
    let store = EventStore::open_in_memory().unwrap();
    let mut r = record("e1");
    r.occurred_at_ns = None;
    let res = store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 0xE001, vec![r]),
            received_at_ns: 1_700_000_000_000_000_002,
        })
        .await
        .unwrap();
    assert_eq!(res.inserted.len(), 1);
    assert_eq!(res.duplicates, 0);
    let s = &res.inserted[0];
    assert!(s.seq >= 1);
    assert_eq!(s.endpoint_id, "sim-001");
    assert_eq!(s.stream_epoch, 0xE001);
    assert_eq!(s.batch_sequence, 1);
    assert_eq!(s.event_index, 0);
    assert_eq!(s.published_at_ns, 1_700_000_000_000_000_001);
    assert_eq!(s.received_at_ns, 1_700_000_000_000_000_002);
    assert_eq!(s.occurred_at_ns, None);
    assert_eq!(s.transition.as_deref(), Some("raised"));
    assert_eq!(s.active, Some(true));
    assert_eq!(s.acknowledged, None);

    let (rows, next) = store.query_history(&EventFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], *s);
    assert_eq!(next, None);
}

/// 同 id 同内容重放 → dedup（不重复入库，只计数）。
#[tokio::test]
async fn replay_same_payload_dedups() {
    let store = EventStore::open_in_memory().unwrap();
    let mk = || CommitRequest {
        endpoint_id: "sim-001".into(),
        batch: batch(1, 0xE002, vec![record("e1")]),
        received_at_ns: now_unix_ns(),
    };
    let r1 = store.commit_batch(mk()).await.unwrap();
    assert_eq!(r1.inserted.len(), 1);
    // 新 epoch 重发同一 occurrence（补发语义）→ dedup
    let r2 = store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(9, 0xE003, vec![record("e1")]),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();
    assert_eq!(r2.inserted.len(), 0);
    assert_eq!(r2.duplicates, 1);
    assert_eq!(store.stats().unwrap().rows, 1);
}

/// 同 id 异内容 → 整批回滚 + 精确 IdCollision（批内新事件也不得入库）。
#[tokio::test]
async fn id_collision_rolls_back_whole_batch() {
    let store = EventStore::open_in_memory().unwrap();
    store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 0xE004, vec![record("e1")]),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();

    let mut evil = record("e1");
    evil.message = Some("forged".into());
    let err = store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(2, 0xE004, vec![record("e-new"), evil]),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, EventStoreError::IdCollision { .. }),
        "必须精确报 collision，got {err}"
    );
    // 整批回滚：e-new 也不得入库
    let (rows, _) = store.query_history(&EventFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_id, "e1");
}

/// 混装批：旧（dedup）+ 新（insert）一次 COMMIT；Hub 只会收到新行。
#[tokio::test]
async fn mixed_batch_partial_dedup() {
    let store = EventStore::open_in_memory().unwrap();
    store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 0xE005, vec![record("a")]),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();
    let res = store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(2, 0xE005, vec![record("a"), record("b"), record("c")]),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();
    assert_eq!(res.duplicates, 1);
    assert_eq!(res.inserted.len(), 2);
    assert_eq!(res.inserted[0].event_id, "b");
    assert_eq!(res.inserted[1].event_id, "c");
    // event_index 保留批内位置
    assert_eq!(res.inserted[0].event_index, 1);
    assert_eq!(res.inserted[1].event_index, 2);
}

/// u64 epoch 全范围往返（超 i64::MAX 的位保持重解释）。
#[tokio::test]
async fn epoch_full_u64_range_roundtrips() {
    let store = EventStore::open_in_memory().unwrap();
    for epoch in [1u64, 0xE001, u64::MAX, u64::MAX - 1, 1 << 63] {
        let id = format!("e-{epoch}");
        store
            .commit_batch(CommitRequest {
                endpoint_id: "ep".into(),
                batch: batch(1, epoch, vec![record(&id)]),
                received_at_ns: now_unix_ns(),
            })
            .await
            .unwrap();
        let (rows, _) = store
            .query_history(&EventFilter {
                endpoint_id: Some("ep".into()),
                ..Default::default()
            })
            .unwrap();
        let got = rows.iter().find(|r| r.event_id == id).unwrap();
        assert_eq!(got.stream_epoch, epoch, "epoch {epoch} 必须精确往返");
    }
}

/// 文件库重启持久性：drop 后重开，历史仍在且 seq 继续增长。
#[tokio::test]
async fn file_restart_persists_and_seq_continues() {
    let path = tmp_path("restart");
    let _ = std::fs::remove_file(&path);
    let seq1 = {
        let store = EventStore::open(&path).unwrap();
        let res = store
            .commit_batch(CommitRequest {
                endpoint_id: "sim-001".into(),
                batch: batch(1, 7, vec![record("e1")]),
                received_at_ns: now_unix_ns(),
            })
            .await
            .unwrap();
        res.inserted[0].seq
    };
    {
        let store = EventStore::open(&path).unwrap();
        let (rows, _) = store.query_history(&EventFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        let res = store
            .commit_batch(CommitRequest {
                endpoint_id: "sim-001".into(),
                batch: batch(2, 7, vec![record("e2")]),
                received_at_ns: now_unix_ns(),
            })
            .await
            .unwrap();
        assert!(
            res.inserted[0].seq > seq1,
            "重启后 seq 必须继续增长（AUTOINCREMENT 持久）"
        );
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

/// 分页游标无重复无遗漏 + 过滤器。
#[tokio::test]
async fn pagination_and_filters() {
    let store = EventStore::open_in_memory().unwrap();
    let mut events = Vec::new();
    for i in 0..5 {
        let mut r = record(&format!("e{i}"));
        r.severity = (i * 200) as u16;
        r.category = if i % 2 == 0 {
            "alarm".into()
        } else {
            "message".into()
        };
        events.push(r);
    }
    store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 9, events),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();

    // 分页：limit=2 取三页，5 行全覆盖无重复
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let (rows, next) = store
            .query_history(&EventFilter {
                limit: Some(2),
                before_seq: cursor,
                ..Default::default()
            })
            .unwrap();
        for r in &rows {
            seen.push(r.seq);
        }
        cursor = next;
        if next.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 5);
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 5);
    // DESC 顺序
    assert!(seen.windows(2).all(|w| w[0] > w[1]));

    // 过滤器
    let (rows, _) = store
        .query_history(&EventFilter {
            category: Some("alarm".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 3);
    let (rows, _) = store
        .query_history(&EventFilter {
            severity_min: Some(400),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 3);
    let (rows, _) = store
        .query_history(&EventFilter {
            condition_id: Some("SIM-ALARM-100".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 5);
}

/// P1 golden vector：canonical JSON 全串 + SHA-256 精确冻结。
/// 目标是"跨版本可比"——serde_json 升级或有人"优化" canonical 映射时，
/// 此测试变红，而不是静默改变历史 payload_hash 兼容性。
#[test]
fn canonical_hash_golden_vector() {
    let mut r = record("GOLDEN-1");
    r.attributes = BTreeMap::from([
        ("z".into(), Value::I32(-7)),
        ("a".into(), Value::Bool(true)),
        ("code".into(), Value::String("G1".into())),
    ]);
    let s = serde_json::to_string(&crate::canonical_event_payload(&r).unwrap()).unwrap();
    assert_eq!(
        s,
        r#"{"attributes":{"a":{"type":"bool","value":true},"code":{"type":"string","value":"G1"},"z":{"type":"i32","value":-7}},"category":"alarm","code":"SIM-100","condition":{"acknowledged":null,"active":true,"condition_id":"SIM-ALARM-100","confirmed":null,"retain":true,"transition":"raised"},"correlation_id":null,"event_id":"GOLDEN-1","kind":"alarm.condition","message":"overtemp","message_locale":"en","occurred_at_ns":1700000000000000000,"severity":700,"source":"Channel1"}"#
    );
    assert_eq!(
        crate::event_payload_hash(&r).unwrap(),
        "d2432d51ef1908c9a4726d4d4a76e2d8aeb71cf61b687d2bcef3d8fc01b8644f"
    );
}

/// purge + stats。
#[tokio::test]
async fn purge_and_stats() {
    let store = EventStore::open_in_memory().unwrap();
    let events: Vec<EventRecord> = (0..5).map(|i| record(&format!("e{i}"))).collect();
    let res = store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 11, events),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();
    let cutoff = res.inserted[3].seq;
    assert_eq!(store.purge_before_seq(cutoff, 100).await.unwrap(), 3);
    assert_eq!(store.stats().unwrap().rows, 2);
    assert!(store.stats().unwrap().size_bytes > 0);
}

/// ⑧b retention sweep：数量上限删最老、时间窗删过期、空 sweep 无操作。
#[tokio::test]
async fn retention_sweep_count_and_time() {
    use std::sync::Arc;

    use crate::retention::{RetentionConfig, sweep_once};

    let store = Arc::new(EventStore::open_in_memory().unwrap());
    let events: Vec<EventRecord> = (0..5).map(|i| record(&format!("r{i}"))).collect();
    store
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 11, events),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();
    // 数量上限 3 → 删最老的 2 行（seq 最小的两行）
    let cfg = RetentionConfig {
        retention_days: 0, // 本用例不按时间删
        max_records: 3,
        interval_secs: 600,
        purge_batch: 1000,
    };
    assert_eq!(sweep_once(&store, &cfg).await.unwrap(), 2);
    assert_eq!(store.stats().unwrap().rows, 3);
    // 时间窗 0 天是"不按时间删"；用 retention_days=30（全部在窗内）应删 0 行
    let cfg = RetentionConfig {
        retention_days: 30,
        max_records: 0,
        interval_secs: 600,
        purge_batch: 1000,
    };
    assert_eq!(sweep_once(&store, &cfg).await.unwrap(), 0);
    assert_eq!(store.stats().unwrap().rows, 3);
    // 部分过期 → Some 臂：只删 cutoff 之前的旧行，新行保留。
    // 注意提交顺序：time purge 按 seq 前缀删（append 时序 ≈ seq 序的
    // 生产不变量），旧行必须先入库。
    let store3 = Arc::new(EventStore::open_in_memory().unwrap());
    let old3: Vec<EventRecord> = (0..2).map(|i| record(&format!("old{i}"))).collect();
    store3
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 11, old3),
            received_at_ns: now_unix_ns() - 31 * 86_400_000_000_000,
        })
        .await
        .unwrap();
    let fresh: Vec<EventRecord> = (0..3).map(|i| record(&format!("fresh{i}"))).collect();
    store3
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(2, 11, fresh),
            received_at_ns: now_unix_ns(),
        })
        .await
        .unwrap();
    let cfg = RetentionConfig {
        retention_days: 30,
        max_records: 0,
        interval_secs: 600,
        purge_batch: 1, // 小批量多轮也必须删净
    };
    assert_eq!(sweep_once(&store3, &cfg).await.unwrap(), 2);
    assert_eq!(store3.stats().unwrap().rows, 3);
    // 全表过期 → None 臂（before = max+1）全清
    let store2 = Arc::new(EventStore::open_in_memory().unwrap());
    let old2: Vec<EventRecord> = (0..2).map(|i| record(&format!("o{i}"))).collect();
    store2
        .commit_batch(CommitRequest {
            endpoint_id: "sim-001".into(),
            batch: batch(1, 11, old2),
            received_at_ns: now_unix_ns() - 31 * 86_400_000_000_000,
        })
        .await
        .unwrap();
    let cfg = RetentionConfig {
        retention_days: 30,
        max_records: 0,
        interval_secs: 600,
        purge_batch: 1000,
    };
    assert_eq!(sweep_once(&store2, &cfg).await.unwrap(), 2);
    assert_eq!(store2.stats().unwrap().rows, 0);
}
