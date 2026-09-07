//! OPC UA Event Manager 级 E2E（PR9 Stage ⑦）：fixture 服务器（测试进程内）→
//! 真 opcua 驱动子进程 → Manager → EventIngress → events.db（+ in-process
//! REST 烟雾）。
//!
//! 不是 mesad 进程级：REST/SSE 层本 PR 零改动（协议无关，由 PR7/PR8 gate 继承）；
//! 此处仅用 in-process Router 证明 native 行经 unchanged 的 HTTP 层可见。
//!
//! 纪律：trigger 循环 + `wait_until(DB 可观测态)`，不用 sleep 猜测；
//! 同一 EventId 重复 trigger 依赖 EventStore 去重（§30：新 epoch 重放不增行）。

mod common;

#[allow(dead_code)] // 各测试目标取用子集，未用 helper 属正常
#[path = "../../../drivers/opcua/tests/support/event_server.rs"]
mod event_server;

use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_event_store::{EVENT_HUB_CAPACITY, EventFilter, EventHub, EventServices, EventStore};

use common::*;

fn tmp_db(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "mesa-opcua-e2e-{}-{}-{tag}.db",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    p
}

/// PKI 目录：全进程统一（None 策略下空目录即可；统一值避免 env 并发写竞争）。
fn ensure_pki_dir() -> std::path::PathBuf {
    static ONCE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("mesa-opcua-pki-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY：测试进程内单值初始化（OnceLock），与 discovery_contract 同模式。
        unsafe {
            std::env::set_var("MESA_OPCUA_PKI_DIR", &dir);
        }
        dir.clone()
    })
    .clone()
}

fn opcua_event_task() -> EventTask {
    EventTask {
        id: "opcua-main-events".into(),
        mode: TaskMode::Subscribe,
        interval_ms: None,
        binding: DriverBinding {
            kind: GENERIC_EVENT_BINDING_KIND.into(),
            config: serde_json::json!({
                "stream_id": "opcua.events",
                "parameters": {
                    "notifier_node_id": "ns=0;i=2253",
                    "scope": "all",
                    "publishing_interval_ms": 500,
                    "queue_size": 1000,
                },
            }),
        },
    }
}

fn endpoint(url: &str) -> BuiltinEndpoint {
    BuiltinEndpoint {
        endpoint_id: "ct-opcua-evt".into(),
        driver_id: "opcua".into(),
        connection_json: format!(r#"{{"endpoint_url":"{url}","timeout_ms":5000}}"#),
        // Event-only：零 Data 任务（§18 manager 级硬门）。
        tasks: vec![],
        event_tasks: vec![opcua_event_task()],
    }
}

fn rows_of(store: &EventStore) -> Vec<mesa_event_store::StoredEvent> {
    store
        .query_history(&EventFilter {
            endpoint_id: Some("ct-opcua-evt".into()),
            ..Default::default()
        })
        .map(|(rows, _)| rows)
        .unwrap_or_default()
}

/// §29：E1..E5 经真实协议栈落盘（exact id / transition / occurred），Hub 同步收到。
#[tokio::test]
async fn opcua_native_production_path_persists_five_events() {
    common::init_log();
    ensure_pki_dir();
    let db = tmp_db("prod");
    let _ = std::fs::remove_file(&db);
    let srv = event_server::FixtureEventServer::start().await;

    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let mut live = hub.subscribe();
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(store.clone(), hub));
    mgr.start_endpoint(endpoint(&srv.endpoint_url())).unwrap();

    // trigger 即轮询条件本身：每 4 轮（约 5/s）打一遍 E1..E5，DB 收齐即停。
    // 重复 trigger 同 payload 依赖 EventStore 去重（中间态不断言）。
    let mut polls = 0u32;
    wait_until(60, || {
        polls += 1;
        if polls % 4 == 1 {
            srv.trigger(&event_server::e1_base());
            srv.trigger(&event_server::e2_raised());
            srv.trigger(&event_server::e3_updated());
            srv.trigger(&event_server::e4_acknowledged());
            srv.trigger(&event_server::e5_cleared());
        }
        rows_of(&store).len() >= 5
    })
    .await;
    // 去重收敛：恰好 5 行（重复 trigger 不增行）。
    let rows = rows_of(&store);
    assert_eq!(rows.len(), 5, "重复 trigger 必须去重，实际 {rows:?}");

    let mut ordered = rows.clone();
    ordered.sort_by_key(|r| r.seq);
    let ids: Vec<&str> = ordered.iter().map(|r| r.event_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["opcua:4Q", "opcua:4g", "opcua:4w", "opcua:5A", "opcua:5Q"]
    );
    assert_eq!(ordered[0].transition.as_deref(), None);
    assert_eq!(ordered[0].category, "event");
    assert_eq!(ordered[0].kind, "opcua.event");
    assert_eq!(ordered[0].message.as_deref(), Some("fixture event"));
    assert_eq!(ordered[0].severity, 321);
    assert_eq!(ordered[0].occurred_at_ns, Some(event_server::t(1)));
    let transitions = ["raised", "updated", "acknowledged", "cleared"];
    for (r, want) in ordered[1..].iter().zip(transitions) {
        assert_eq!(r.transition.as_deref(), Some(want), "event {}", r.event_id);
        assert_eq!(r.category, "condition");
        assert_eq!(r.kind, "opcua.condition");
        assert!(r.condition_id.as_deref().unwrap().ends_with(";s=Alarm1"));
    }
    assert_eq!(ordered[1].occurred_at_ns, Some(event_server::t(2)));
    assert_eq!(ordered[1].active, Some(true));
    assert_eq!(ordered[2].occurred_at_ns, Some(event_server::t(3)));
    assert_eq!(ordered[3].acknowledged, Some(true));
    assert_eq!(ordered[4].active, Some(false));
    assert_eq!(ordered[4].occurred_at_ns, Some(event_server::t(5)));
    for r in &ordered {
        assert!(r.received_at_ns > 0);
        assert!(r.published_at_ns > 0);
    }

    // Hub 收到同样的已提交行（commit-then-publish）。
    let mut hub_ids = vec![];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while hub_ids.len() < 5 && std::time::Instant::now() < deadline {
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
    let mut want_sorted = ids.clone();
    want_sorted.sort_unstable();
    assert_eq!(hub_ids, want_sorted);

    assert_eq!(mgr.stop_endpoint("ct-opcua-evt").await, Ok(true));
    assert_eq!(rows_of(&store).len(), 5, "停止后历史仍在");
    drop(mgr);
    srv.stop().await;
    let _ = std::fs::remove_file(&db);
}

/// §30：stop → start（新 epoch）后重放 E1 不增行，E2 正常追加。
/// 证明 EventId 与 transport sequence 无关，去重不依赖 epoch。
#[tokio::test]
async fn opcua_reconnect_replay_dedups_and_appends() {
    common::init_log();
    ensure_pki_dir();
    let db = tmp_db("reconnect");
    let _ = std::fs::remove_file(&db);
    let srv = event_server::FixtureEventServer::start().await;

    let store = Arc::new(EventStore::open(&db).unwrap());
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));

    mgr.start_endpoint(endpoint(&srv.endpoint_url())).unwrap();
    // trigger 即轮询条件本身（50ms 一次）：E1 到达即停。
    wait_until(30, || {
        srv.trigger(&event_server::e1_base());
        !rows_of(&store).is_empty()
    })
    .await;
    assert_eq!(mgr.stop_endpoint("ct-opcua-evt").await, Ok(true));
    assert_eq!(rows_of(&store).len(), 1);

    // 新 epoch 重放同一 E1：仍 1 行（去重），且 stream_epoch 保持首轮值。
    mgr.start_endpoint(endpoint(&srv.endpoint_url())).unwrap();
    for _ in 0..15 {
        srv.trigger(&event_server::e1_base());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let rows = rows_of(&store);
    assert_eq!(rows.len(), 1, "重放必须去重，实际 {rows:?}");
    assert_eq!(rows[0].event_id, "opcua:4Q");

    // 新事件正常追加。
    srv.trigger(&event_server::e2_raised());
    wait_until(60, || rows_of(&store).len() >= 2).await;
    let rows = rows_of(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(mgr.stop_endpoint("ct-opcua-evt").await, Ok(true));
    drop(mgr);
    srv.stop().await;
    let _ = std::fs::remove_file(&db);
}

/// §31：64 个互异事件一次打足 → 运行中全部到达 → Stop 后无遗弃、无重复、
/// 同 epoch batch_sequence 连续。
#[tokio::test]
async fn opcua_stop_barrier_keeps_all_occurrences() {
    common::init_log();
    ensure_pki_dir();
    let db = tmp_db("stopgate");
    let _ = std::fs::remove_file(&db);
    let srv = event_server::FixtureEventServer::start().await;

    let store = Arc::new(EventStore::open(&db).unwrap());
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));
    mgr.start_endpoint(endpoint(&srv.endpoint_url())).unwrap();

    // 64 个互异事件打足 → 收齐（轮询条件本身：同 id 重复打足依赖去重收敛；
    // 单发 burst 不可靠——驱动子进程就绪前打出的事件必然丢失）。
    // 监控项队列 1000 / callback 1024；每 40 轮（约 2s）打一轮，远离溢出。
    let mut polls = 0u32;
    wait_until(120, || {
        polls += 1;
        if polls % 40 == 1 {
            use opcua_nodes::BaseEventType;
            for i in 0..64u8 {
                let ev = BaseEventType::new(
                    opcua_types::NodeId::new(0, opcua_types::ObjectTypeId::BaseEventType as u32),
                    opcua_types::ByteString::from(vec![0x10u8.wrapping_add(i)]),
                    opcua_types::LocalizedText::new("", "flood"),
                    event_server::ns_to_datetime(event_server::t(1)),
                );
                srv.trigger(&ev);
            }
        }
        rows_of(&store).len() >= 64
    })
    .await;
    assert_eq!(mgr.stop_endpoint("ct-opcua-evt").await, Ok(true));
    let rows = rows_of(&store);
    assert_eq!(rows.len(), 64, "64 个 occurrence 必须全在");
    let mut ids: Vec<&str> = rows.iter().map(|r| r.event_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 64, "互异 id 不得合并/丢失");
    // 屏障语义（批号语言）：64 行的 (batch_sequence, event_index) 对互异
    // （无整批重投）、每批内序号 0..n-1 连续（无批内遗弃）。
    // 注意 batch_sequence 是批号而非行号——同批多行共享批号是正常形态
    // （与 simulator 单事件一批不同，不可照搬 batch_sequence 连续断言）。
    use std::collections::{HashMap, HashSet};
    let mut pairs = HashSet::new();
    let mut by_batch: HashMap<u64, Vec<u32>> = HashMap::new();
    for r in &rows {
        assert!(
            pairs.insert((r.batch_sequence, r.event_index)),
            "重复投递：batch={} index={}",
            r.batch_sequence,
            r.event_index
        );
        by_batch
            .entry(r.batch_sequence)
            .or_default()
            .push(r.event_index);
    }
    for (batch, indices) in by_batch.iter_mut() {
        indices.sort_unstable();
        for (expect, got) in indices.iter().enumerate() {
            assert_eq!(*got as usize, expect, "batch {batch} 批内序号有缺口");
        }
    }
    drop(mgr);
    srv.stop().await;
    let _ = std::fs::remove_file(&db);
}

/// P1-3 REST 烟雾：native 行经 unchanged 的 HTTP 层可见（in-process Router；
/// 不是 mesad 进程——REST/SSE 本 PR 零改动，分页/过滤语义由 PR7 gate 继承）。
#[tokio::test]
async fn opcua_native_rows_visible_via_rest() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    common::init_log();
    ensure_pki_dir();
    let db = tmp_db("rest");
    let _ = std::fs::remove_file(&db);
    let srv = event_server::FixtureEventServer::start().await;

    let store = Arc::new(EventStore::open(&db).unwrap());
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));
    mgr.start_endpoint(endpoint(&srv.endpoint_url())).unwrap();
    let mut polls = 0u32;
    wait_until(60, || {
        polls += 1;
        if polls % 4 == 1 {
            srv.trigger(&event_server::e2_raised());
        }
        !rows_of(&store).is_empty()
    })
    .await;

    let drivers_dir = repo_root().join("drivers");
    let cfg_store = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    let mgr = Arc::new(mgr);
    #[allow(deprecated)]
    let state = mesa_core_api::AppState::new(
        mgr.clone(),
        cfg_store,
        drivers_dir.to_string_lossy().to_string(),
    );
    state.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));
    let app = mesa_core_api::router(state);
    let req = Request::builder()
        .uri("/api/v1/events?endpoint_id=ct-opcua-evt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"]["event_id"], "opcua:4g");
    assert_eq!(events[0]["event"]["kind"], "opcua.condition");

    assert_eq!(mgr.stop_endpoint("ct-opcua-evt").await, Ok(true));
    drop(mgr);
    srv.stop().await;
    let _ = std::fs::remove_file(&db);
}
