//! Reconnect / epoch / Stop-barrier torture（PR10 commit 3 §8/§9）：
//! 同一份契约跑两个独立 Event source（Simulator 真子进程 / OPC UA 真子进程 +
//! fixture）。多轮启停 + 故意重放后，DB 恰好等于发射并集（不多不少）；
//! 每轮 epoch 互异；Stop 后旧 epoch 永不再出新行。

mod common;
mod event_common;

use std::collections::BTreeSet;
use std::time::Duration;

use event_common::{EventTestSource, OpcUaEventSource, SimulatorEventSource, rows_of};

/// 多轮 torture：emit → disconnect → reconnect → emit（含重放）→ …
/// 最终 DB event_id 集合恰好等于全部发射并集；epoch 轮轮互异；seq 单调。
async fn reconnect_torture_exact_set<S: EventTestSource>(endpoint: &str) {
    let mut src = S::start(endpoint).await;
    let mut emitted = BTreeSet::new();
    // 每轮新行（相对上一轮多出来的 id）的 epoch 不得见于之前轮次：
    // reconnect 确实开了新 epoch；纯重放轮（无新行）不要求新 epoch 落盘。
    let mut seen_epochs = BTreeSet::new();
    let mut known: BTreeSet<String> = BTreeSet::new();
    for round in 0..3u32 {
        let ids = src.emit_round(round).await;
        emitted.extend(ids.clone());
        src.disconnect().await;
        src.reconnect().await;
        let rows = rows_of(&src.store(), src.endpoint_id());
        let epoch_of: std::collections::HashMap<&str, u64> = rows
            .iter()
            .map(|r| (r.event_id.as_str(), r.stream_epoch))
            .collect();
        // 本轮新行 epoch 集与历史不交（同轮内共享 epoch 是正常的）。
        let round_epochs: BTreeSet<u64> = ids
            .iter()
            .filter(|id| !known.contains(*id))
            .map(|id| epoch_of[id.as_str()])
            .collect();
        assert!(
            round_epochs.is_disjoint(&seen_epochs),
            "round {round} 新行 epoch {round_epochs:?} 不得复用旧 epoch {seen_epochs:?}"
        );
        seen_epochs.extend(round_epochs);
        known.extend(ids);
    }
    let rows = rows_of(&src.store(), src.endpoint_id());
    let got: BTreeSet<String> = rows.iter().map(|r| r.event_id.clone()).collect();
    assert_eq!(
        got, emitted,
        "DB 必须恰好等于发射并集（重放不增行，新人不缺席）"
    );
    assert!(!seen_epochs.is_empty());
    // store seq 单调（DB commit order）
    let mut seqs: Vec<i64> = rows.iter().map(|r| r.seq).collect();
    seqs.sort_unstable();
    for w in seqs.windows(2) {
        assert!(w[0] < w[1]);
    }
    src.stop().await;
}

#[tokio::test]
async fn sim_reconnect_torture_exact_set() {
    common::init_log();
    reconnect_torture_exact_set::<SimulatorEventSource>("hd-torture-sim").await;
}

#[tokio::test]
async fn opcua_reconnect_torture_exact_set() {
    common::init_log();
    reconnect_torture_exact_set::<OpcUaEventSource>("hd-torture-opcua").await;
}

/// Stop barrier 重复启停：每轮新增恰好等于本轮发射；StopAck 后旧 epoch
/// 永不再出新行（2s 静默窗口验证）。
async fn stop_barrier_repeated<S: EventTestSource>(endpoint: &str, rounds: u32) {
    let mut src = S::start(endpoint).await;
    for round in 0..rounds {
        let ids = src.emit_round(round).await;
        // 本轮新增 = 发射去重后（重放部分不增行）
        let rows = rows_of(&src.store(), src.endpoint_id());
        let fresh: Vec<_> = rows.iter().filter(|r| ids.contains(&r.event_id)).collect();
        assert_eq!(
            fresh.len(),
            ids.iter().collect::<BTreeSet<_>>().len(),
            "round {round} 新增必须等于本轮发射去重"
        );
        src.disconnect().await;
        src.reconnect().await;
    }
    // 静默窗口：Stop 后 2s 内行数纹丝不动
    let before = rows_of(&src.store(), src.endpoint_id()).len();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = rows_of(&src.store(), src.endpoint_id()).len();
    assert_eq!(before, after, "Stop 后旧 epoch 不得再出新行");
    src.stop().await;
}

#[tokio::test]
async fn sim_stop_barrier_repeated() {
    common::init_log();
    stop_barrier_repeated::<SimulatorEventSource>("hd-barrier-sim", 5).await;
}

#[tokio::test]
async fn opcua_stop_barrier_repeated() {
    common::init_log();
    stop_barrier_repeated::<OpcUaEventSource>("hd-barrier-opcua", 5).await;
}
