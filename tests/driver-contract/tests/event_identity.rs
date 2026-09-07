//! Event identity Contract Gate（PR10 commit 2 §2）：实现行为 → 正式不变量。
//!
//! 直达唯一写 API（`EventStore::commit_batch`），两种 source 形状各跑一遍：
//! 同 occurrence 重复 → 插一次；同 id 异 payload → 整批回滚；transport 元数据
//! 变化 → 身份不变。SSE/Hub 可见性由 PR7 gate 继承，此处只锁存储契约。

mod common;
mod event_common;

use event_common::*;
use mesa_core_types::Value;
use mesa_event_store::EventStoreError;

fn fresh_store(tag: &str) -> (ArcPath, mesa_event_store::EventStore) {
    let db = tmp_db(tag);
    let _ = std::fs::remove_file(&db);
    let path = db.clone();
    (
        ArcPath(path),
        mesa_event_store::EventStore::open(&db).unwrap(),
    )
}

struct ArcPath(std::path::PathBuf);
impl Drop for ArcPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 同 occurrence 重复到达（同 endpoint / 同 id / 同 payload）→ 只插一次，
/// 第二次计 duplicate，不开第二行。跨 epoch 同样 dedup（重连补发语义）。
#[tokio::test]
async fn same_occurrence_replays_once_both_sources() {
    for shape in ["sim", "opcua"] {
        let (_guard, store) = fresh_store(&format!("dup-{shape}"));
        let ep = format!("hd-dup-{shape}");
        let rec = if shape == "sim" {
            sim_record("dup-1", 42)
        } else {
            opcua_record("dup-1", "COND-1")
        };
        let r1 = commit(&store, &ep, event_batch(1, 7, 100, vec![rec.clone()]))
            .await
            .unwrap();
        assert_eq!(r1.inserted.len(), 1);
        assert_eq!(r1.duplicates, 0);
        // 同 epoch 重放
        let r2 = commit(&store, &ep, event_batch(1, 7, 101, vec![rec.clone()]))
            .await
            .unwrap();
        assert!(r2.inserted.is_empty());
        assert_eq!(r2.duplicates, 1);
        // 跨 epoch 重放（新 handle/epoch/sequence，语义相同）
        let r3 = commit(&store, &ep, event_batch(2, 8, 1, vec![rec]))
            .await
            .unwrap();
        assert!(r3.inserted.is_empty());
        assert_eq!(r3.duplicates, 1);
        assert_eq!(rows_of(&store, &ep).len(), 1, "shape={shape}");
    }
}

/// 同 event_id 但 payload 任一语义字段不同 → `EVENT_ID_COLLISION` + 整批回滚
/// （批内其他新行也不得入库）。差异矩阵：message / severity / transition /
/// attributes / occurred_at（不只测一个简单字段）。
#[tokio::test]
async fn same_id_different_payload_each_field_collides() {
    use mesa_core_types::{ConditionTransition, EventRecord};
    type Mutate = fn(EventRecord) -> EventRecord;
    let variants: Vec<(&str, Mutate)> = vec![
        ("message", |r| with_message(r, "CHANGED")),
        ("severity", |r| with_severity(r, 999)),
        ("transition", |r| {
            with_transition(r, ConditionTransition::Acknowledged)
        }),
        ("attributes", |r| with_attr(r, "extra", Value::I32(1))),
        ("occurred_at", |r| {
            with_occurred(r, 1_700_000_000_000_000_001)
        }),
    ];
    for (field, mutate) in variants {
        let (_guard, store) = fresh_store(&format!("col-{field}"));
        let ep = format!("hd-col-{field}");
        let base = opcua_record("col-1", "COND-9");
        commit(&store, &ep, event_batch(1, 1, 1, vec![base.clone()]))
            .await
            .unwrap();
        // 批内再带一条全新记录：回滚必须连它一起撤（原子性）。
        let evil = mutate(base);
        let fresh = opcua_record("col-fresh", "COND-9");
        let err = commit(&store, &ep, event_batch(1, 1, 2, vec![evil, fresh]))
            .await
            .expect_err(&format!("{field} 差异必须 collision"));
        assert!(
            matches!(err, EventStoreError::IdCollision { .. }),
            "{field} 必须精确报 collision，got {err:?}"
        );
        let rows = rows_of(&store, &ep);
        assert_eq!(rows.len(), 1, "{field}：整批回滚后只剩首条");
        assert_eq!(rows[0].event_id, "col-1");
    }
}

/// connection metadata 变化不能改变 payload hash：handle / epoch / sequence /
/// event_index 怎么跳，同一 EventRecord 仍然 dedup（PR9“不写 transport 元数据
/// 进 record”的冻结，commit 2 锁死）。
#[tokio::test]
async fn transport_metadata_does_not_change_identity() {
    let (_guard, store) = fresh_store("meta");
    let ep = "hd-meta";
    let rec = sim_record("meta-1", 7);
    commit(&store, ep, event_batch(1, 7, 100, vec![rec.clone()]))
        .await
        .unwrap();
    for (h, e, s) in [(1u32, 7u64, 101u64), (9, 7, 102), (9, 8, 1), (1, 9, 999)] {
        let r = commit(&store, ep, event_batch(h, e, s, vec![rec.clone()]))
            .await
            .unwrap();
        assert!(r.inserted.is_empty(), "metadata ({h},{e},{s}) 不得产生新行");
        assert_eq!(r.duplicates, 1);
    }
    assert_eq!(rows_of(&store, ep).len(), 1);
}

/// batch 原子性 torture：256 条中第 137 条 collision → 整批 0 行入库
///（不是前 136 条写入）。
#[tokio::test]
async fn batch_atomicity_middle_collision_rolls_back_all() {
    let (_guard, store) = fresh_store("atomic");
    let ep = "hd-atomic";
    // 预埋与 #137 同 id 异 payload 的行
    commit(
        &store,
        ep,
        event_batch(
            1,
            1,
            1,
            vec![with_message(base_record("evt-137"), "original")],
        ),
    )
    .await
    .unwrap();
    let mut batch: Vec<mesa_core_types::EventRecord> =
        (0..256).map(|i| base_record(&format!("evt-{i}"))).collect();
    batch[137] = with_message(base_record("evt-137"), "MUTATED");
    let err = commit(&store, ep, event_batch(1, 1, 2, batch))
        .await
        .expect_err("第 137 条必须引爆整批");
    assert!(matches!(err, EventStoreError::IdCollision { .. }));
    let rows = rows_of(&store, ep);
    assert_eq!(rows.len(), 1, "整批回滚：只剩预埋行，got {}", rows.len());
    assert_eq!(rows[0].message.as_deref(), Some("original"));
}

/// Store seq 与 transport sequence 彻底隔离：epoch/batch 怎么跳，
/// `events.seq` 只表示 DB commit order（连续自增，不接触 transport 语言）。
#[tokio::test]
async fn store_seq_isolated_from_transport_sequence() {
    let (_guard, store) = fresh_store("seqiso");
    let ep = "hd-seqiso";
    commit(&store, ep, event_batch(1, 7, 100, vec![base_record("s-1")]))
        .await
        .unwrap();
    commit(&store, ep, event_batch(1, 7, 200, vec![base_record("s-2")]))
        .await
        .unwrap();
    commit(&store, ep, event_batch(1, 8, 1, vec![base_record("s-3")]))
        .await
        .unwrap();
    let rows = rows_of(&store, ep);
    assert_eq!(rows.len(), 3);
    // query_history 按 seq DESC；反转后才是 commit order
    let seqs: Vec<i64> = rows.iter().rev().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3], "store seq 必须纯 commit order");
}
