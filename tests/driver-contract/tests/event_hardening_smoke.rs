//! Event Hardening smoke（PR10 commit 1）：只证明 `event_common` harness
//! 可用（Simulator 真子进程 → 4 行 alarm 落盘 exact）。真正 gates 在后续 commit。

mod common;
mod event_common;

use event_common::{EventTestSource, SimulatorEventSource};

/// harness 可用性：Sim 源 emit 一轮恰 4 行（四态），ids 互异，stop 干净。
#[tokio::test]
async fn hardening_harness_sim_source_emits_alarm_cycle() {
    common::init_log();
    let mut src = SimulatorEventSource::start("hd-smoke-001").await;
    let ids = src.emit_round().await;
    assert_eq!(ids.len(), 4);
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 4, "四态 event_id 必须互异");
    let rows = event_common::rows_of(&src.store(), src.endpoint_id());
    assert_eq!(rows.len(), 4);
    src.disconnect().await;
    src.stop().await;
}
