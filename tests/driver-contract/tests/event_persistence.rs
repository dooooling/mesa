//! Event Persistence 契约测试（PR7 v1.1 §23）。
//!
//! 本文件在 Checkpoint A 覆盖生产路径：
//! `MesaManager → 真 simulator 子进程 → EventReceiver → EventIngress →
//!  events.db`。完整 Gate 电池（regression/collision/restart/SSE）在
//! step ⑨补齐（`event_sse.rs` 另起）。

mod common;

use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{DriverBinding, EventTask, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_driver_simulator::{EVENT_BINDING_KIND, SIM_EVENT_STREAM_ALARM};
use mesa_event_store::{EVENT_HUB_CAPACITY, EventFilter, EventHub, EventServices, EventStore};

use common::*;

fn tmp_events_db(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "mesa-event-e2e-{}-{}-{tag}.db",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    p
}

fn alarm_task() -> EventTask {
    EventTask {
        id: "al".into(),
        mode: TaskMode::Subscribe,
        interval_ms: None,
        binding: DriverBinding {
            kind: EVENT_BINDING_KIND.into(),
            config: serde_json::json!({"stream": SIM_EVENT_STREAM_ALARM}),
        },
    }
}

/// 生产路径：manager 启动带事件任务的 endpoint → 报警四态落入 events.db，
/// 且先 COMMIT 后可见（查到即已落盘）；Hub 同步收到已提交行。
#[tokio::test]
async fn production_path_alarm_cycle_persists_before_visible() {
    common::init_log();
    let db = tmp_events_db("prod");
    let _ = std::fs::remove_file(&db);

    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let mut live = hub.subscribe();
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(store.clone(), hub));

    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: "ct-evt-001".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![poll_task(
            "t1",
            50,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![alarm_task()],
    })
    .unwrap();

    // 等待四态全部入库（查到 = 已 COMMIT）
    wait_until(15, || {
        store
            .query_history(&EventFilter {
                endpoint_id: Some("ct-evt-001".into()),
                ..Default::default()
            })
            .map(|(rows, _)| rows.len() >= 4)
            .unwrap_or(false)
    })
    .await;
    let (rows, _) = store
        .query_history(&EventFilter {
            endpoint_id: Some("ct-evt-001".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 4);
    // 实现说明：query_history 按 seq DESC；反转后即发生序
    let mut ordered = rows.clone();
    ordered.reverse();
    let transitions: Vec<Option<String>> = ordered.iter().map(|r| r.transition.clone()).collect();
    assert_eq!(
        transitions,
        vec![
            Some("raised".to_string()),
            Some("updated".to_string()),
            Some("acknowledged".to_string()),
            Some("cleared".to_string())
        ]
    );
    let cond = ordered[0].condition_id.clone();
    assert!(cond.is_some());
    for r in &ordered {
        assert_eq!(r.condition_id, cond);
        assert!(r.received_at_ns > 0, "received_at 由 Core 生成");
        assert!(r.published_at_ns > 0);
    }
    let mut ids: Vec<&str> = ordered.iter().map(|r| r.event_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4);

    // Hub 收到了同样的已提交行（commit-then-publish）
    let mut hub_ids = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while hub_ids.len() < 4 && std::time::Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(std::time::Instant::now()),
            live.recv(),
        )
        .await
        {
            Ok(Ok(ev)) => hub_ids.push(ev.event_id),
            _ => break,
        }
    }
    hub_ids.sort();
    assert_eq!(hub_ids, ids);

    assert!(mgr.stop_endpoint("ct-evt-001").await);
    // 停止后历史仍在（重启恢复的前置：落盘即事实）
    let (rows, _) = store
        .query_history(&EventFilter {
            endpoint_id: Some("ct-evt-001".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 4);
    drop(mgr);
    let _ = std::fs::remove_file(&db);
}

/// Data-only 老路径零成本：无 EventServices 的 manager 照旧跑数据，
///
/// 且无事件任务的 endpoint 在有 EventServices 时也不发 Event RPC。
#[tokio::test]
async fn data_only_path_unaffected() {
    common::init_log();
    let db = tmp_events_db("dataonly");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));

    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: "ct-data-001".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![poll_task(
            "t1",
            50,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![],
    })
    .unwrap();

    // 数据面推进
    wait_until(10, || {
        matches!(
            mgr.snapshot().endpoint("ct-data-001"),
            Some(ref s) if s.state == mesa_core_types::ConnectionState::Running.as_str()
        )
    })
    .await;
    // 无事件入库
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(store.stats().unwrap().rows, 0);
    assert!(mgr.stop_endpoint("ct-data-001").await);
    drop(mgr);
    let _ = std::fs::remove_file(&db);
}
