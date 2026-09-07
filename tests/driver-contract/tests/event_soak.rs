//! Soak gates（PR10 commit 8 §15 + Final 分档）。
//! required fast soak（60s）：PR 门 + unified evidence 常跑档，锁 counter
//! 连续、单 epoch、diagnostics 零异常、clean stop（~1200 events 足够覆盖）。
//! extended soak（150s，`#[ignore]`）：manual/nightly 档，语义与 fast 同，
//! 跑 `cargo test -- --ignored event_soak` 手动执行，不进 required CI.
//! OPC UA 长稳态由 soak 档覆盖，此处只锁 Sim 稳态基线。

mod common;
mod event_common;

use std::time::Duration;

use event_common::rows_of;
use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_event_store::{EVENT_HUB_CAPACITY, EventHub, EventServices, EventStore};

fn drivers_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("drivers")
}

/// 稳态 soak 本体：`secs` 秒 counter（50ms）+ data（100ms）混合负载后，
/// 锁长期无损（n 全程连续）、无 epoch 漂移、诊断零异常、干净停止。
/// 任一静默丢失/漂移在此必现形。
async fn steady_state_soak(tag: &str, secs: u64, min_rows: usize) {
    common::init_log();
    let db = event_common::tmp_db(tag);
    let _ = std::fs::remove_file(&db);
    let store = std::sync::Arc::new(EventStore::open(&db).unwrap());
    let services = EventServices::new(store.clone(), EventHub::new(EVENT_HUB_CAPACITY));
    let mgr = std::sync::Arc::new(MesaManager::discover(&drivers_dir()));
    mgr.set_event_services(std::sync::Arc::clone(&services));
    let ep = format!("hd-soak-{tag}");
    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: ep.clone(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![common::poll_task(
            "d",
            100,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![EventTask {
            id: "cnt".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(50),
            binding: DriverBinding {
                kind: GENERIC_EVENT_BINDING_KIND.into(),
                config: serde_json::to_value(&binding).unwrap(),
            },
        }],
    })
    .unwrap();

    tokio::time::sleep(Duration::from_secs(secs)).await;
    assert!(mgr.is_running(&ep), "soak 全程 endpoint 必须存活");

    // 长期无损：n 连续
    let rows = rows_of(&store, &ep);
    assert!(
        rows.len() >= min_rows,
        "{secs}s 应有规模，got {}",
        rows.len()
    );
    let mut ns: Vec<u64> = rows
        .iter()
        .map(|r| {
            let v: serde_json::Value = serde_json::from_str(&r.attributes_json).unwrap();
            v["value"]["U64"].as_u64().unwrap()
        })
        .collect();
    ns.sort_unstable();
    let max = *ns.last().unwrap();
    assert_eq!(ns.len() as u64, max, "soak 全程无丢失无重复");
    for w in ns.windows(2) {
        assert_eq!(w[1], w[0] + 1, "soak 全程连续");
    }
    // 无漂移：单 epoch
    let epochs: std::collections::BTreeSet<u64> = rows.iter().map(|r| r.stream_epoch).collect();
    assert_eq!(epochs.len(), 1, "稳态下不得触发重连");
    // 诊断零异常
    use std::sync::atomic::Ordering;
    let d = &services.diagnostics;
    for (name, v) in [
        (
            "store_failures",
            d.ingress_store_failures_total.load(Ordering::Relaxed),
        ),
        ("gaps", d.ingress_gaps_total.load(Ordering::Relaxed)),
        (
            "regressions",
            d.ingress_regressions_total.load(Ordering::Relaxed),
        ),
        (
            "collisions",
            d.ingress_collisions_total.load(Ordering::Relaxed),
        ),
        ("invalid", d.ingress_invalid_total.load(Ordering::Relaxed)),
    ] {
        assert_eq!(v, 0, "soak 诊断 {name} 必须为 0");
    }
    // stats 自洽：persisted == 行数
    assert_eq!(
        d.ingress_persisted_events_total.load(Ordering::Relaxed),
        rows.len() as u64
    );
    assert_eq!(mgr.stop_endpoint(&ep).await, Ok(true));
    let _ = std::fs::remove_file(&db);
}

/// required fast soak（60s，~1200 events）：PR 门 + evidence 常跑档。
#[tokio::test]
async fn sim_steady_state_soak_60s_no_loss_no_drift() {
    steady_state_soak("60s", 60, 600).await;
}

/// extended soak（150s）：manual/nightly 档（`-- --ignored` 手动跑）。
#[tokio::test]
#[ignore]
async fn sim_extended_soak_150s_no_loss_no_drift() {
    steady_state_soak("150s", 150, 1500).await;
}
