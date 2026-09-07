//! Event Persistence 契约测试（PR7 v1.1 §23）。
//!
//! 本文件在 Checkpoint A 覆盖生产路径：
//! `MesaManager → 真 simulator 子进程 → EventReceiver → EventIngress →
//!  events.db`。完整 Gate 电池（regression/collision/restart/SSE）在
//! step ⑨补齐（`event_sse.rs` 另起）。

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_core_types::{DriverBinding, EventBatch, EventRecord, EventTask, TaskMode, Value};
use mesa_core_types::{GENERIC_EVENT_BINDING_KIND, GenericEventBinding};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_driver_simulator::{EVENT_BINDING_KIND, SIM_EVENT_STREAM_ALARM, SIM_EVENT_STREAM_COUNTER};
use mesa_event_store::{
    CommitRequest, EVENT_HUB_CAPACITY, EventFilter, EventHub, EventServices, EventStore,
};
use tower::ServiceExt;

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

/// PR8 P1-5：与 `alarm_task` 同语义的标准 `mesa.events.v1` 形态。
/// 生产路径 Gate 必须走 generic 信封（legacy 兼容由 Simulator 单测保住）。
fn generic_alarm_task() -> EventTask {
    let binding = GenericEventBinding {
        stream_id: SIM_EVENT_STREAM_ALARM.into(),
        parameters: serde_json::json!({}),
    };
    EventTask {
        id: "al".into(),
        mode: TaskMode::Subscribe,
        interval_ms: None,
        binding: DriverBinding {
            kind: GENERIC_EVENT_BINDING_KIND.into(),
            config: serde_json::to_value(&binding).unwrap(),
        },
    }
}

/// Stop barrier 计数器（⑨ Gate 用）：Poll 50ms 节奏（~20/s，可持续速率）。
/// 本测试是端到端锁（真实 Stop 路径 + 逐 epoch 连续 + 落盘），牙口不在速率：
/// 确定性遗弃证明在单测 `ingress_cancel_drains_backlog`（旧行为 73/200）。
/// 100/s 在 debug 构建下超过 ingress 单批 txn 吞吐，会正确触发 overflow
/// fail-closed 换 epoch（由 event_pressure 冻结为 overload 域行为）——
/// 此处必须用可持续速率，否则测的是"过载"而不是 Stop barrier。
fn flood_task() -> EventTask {
    EventTask {
        id: "cnt".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(50),
        binding: DriverBinding {
            kind: EVENT_BINDING_KIND.into(),
            config: serde_json::json!({"stream": SIM_EVENT_STREAM_COUNTER}),
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
        event_tasks: vec![generic_alarm_task()],
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

    assert_eq!(mgr.stop_endpoint("ct-evt-001").await, Ok(true));
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

/// P0-3：endpoint stop → start（新 Driver 进程）后 event_id 不重复。
///
/// 旧进程 run_seq 归零陷阱已除（epoch 作用域）：两轮各 4 条 alarm，
/// DB 共 8 行、8 个互异 event_id、0 collision（collision 会触发重连风暴）。
#[tokio::test]
async fn event_ids_unique_across_driver_process_restart() {
    common::init_log();
    let db = tmp_events_db("restart");
    let _ = std::fs::remove_file(&db);

    let store = Arc::new(EventStore::open(&db).unwrap());
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));
    let cfg = || BuiltinEndpoint {
        endpoint_id: "ct-evt-restart".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![poll_task(
            "t1",
            50,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![alarm_task()],
    };

    for round in 0..2 {
        mgr.start_endpoint(cfg()).unwrap();
        // 每轮等够累计 4*(round+1) 条（新进程新 epoch，ID 与上一轮互异；
        // 不能只等 >=4，否则第二轮会因首轮残留立即返回）
        let target = 4 * (round + 1);
        wait_until(15, || {
            store
                .query_history(&EventFilter {
                    endpoint_id: Some("ct-evt-restart".into()),
                    ..Default::default()
                })
                .map(|(rows, _)| rows.len() >= target)
                .unwrap_or(false)
        })
        .await;
        assert_eq!(mgr.stop_endpoint("ct-evt-restart").await, Ok(true));
    }

    let (rows, _) = store
        .query_history(&EventFilter {
            endpoint_id: Some("ct-evt-restart".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 8, "两轮 4+4 全入库，无 collision 丢失");
    let mut ids: Vec<&str> = rows.iter().map(|r| r.event_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 8, "跨进程 event_id 必须互异");
    drop(mgr);
    let _ = std::fs::remove_file(&db);
}

/// ⑤ History REST：播种 5 行后验证分页/过滤/单条/404；
/// 无 EventServices 时 503 精确码。
#[tokio::test]
async fn event_history_rest_pagination_filters_and_detail() {
    common::init_log();
    let db = tmp_events_db("rest");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());

    // 播种：5 条 alarm（severity 递增，便于过滤断言）
    let mut events = Vec::new();
    for i in 0..5 {
        events.push(EventRecord {
            event_id: format!("seed-{i}"),
            category: if i % 2 == 0 {
                "alarm".into()
            } else {
                "message".into()
            },
            kind: "alarm.condition".into(),
            source: "Channel1".into(),
            severity: (i * 200) as u16,
            code: Some("SEED".into()),
            message: Some(format!("seed event {i}")),
            message_locale: None,
            occurred_at_ns: None,
            condition: None,
            correlation_id: None,
            attributes: BTreeMap::from([("i".into(), Value::I32(i))]),
        });
    }
    let res = store
        .commit_batch(CommitRequest {
            endpoint_id: "ct-rest-001".into(),
            batch: EventBatch {
                connection_handle: 1,
                stream_epoch: 0xE100,
                sequence: 1,
                timestamp_ns: 1_700_000_000_000_000_000,
                events,
                mono_ns: None,
            },
            received_at_ns: 1_700_000_000_000_000_001,
        })
        .await
        .unwrap();
    assert_eq!(res.inserted.len(), 5);
    let first_seq = res.inserted[0].seq;

    let drivers_dir = repo_root().join("drivers");
    let cfg_store = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    let mgr = Arc::new(MesaManager::discover(&drivers_dir));
    #[allow(deprecated)]
    let state =
        mesa_core_api::AppState::new(mgr, cfg_store, drivers_dir.to_string_lossy().to_string());
    state.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));
    let app = mesa_core_api::router(state);

    async fn get(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    // 全量：5 行 DESC，第 0 行 seq 最大
    let (st, v) = get(app.clone(), "/api/v1/events?endpoint_id=ct-rest-001").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["events"].as_array().unwrap().len(), 5);
    assert!(v["events"][0]["seq"].as_i64().unwrap() > v["events"][4]["seq"].as_i64().unwrap());
    assert_eq!(v["events"][0]["event"]["event_id"], "seed-4");
    assert!(v["next_cursor"].is_null());
    // 形态：top-level + event 嵌套
    assert_eq!(v["events"][0]["endpoint_id"], "ct-rest-001");
    assert_eq!(v["events"][0]["event"]["severity"], 800);
    // attributes 保持 typed Value 派生形态（PR8 UI 按此解释类型）
    assert_eq!(
        v["events"][0]["event"]["attributes"]["i"],
        serde_json::json!({"I32": 4})
    );

    // 分页：limit=2 走三页，无重复无遗漏
    let (st, p1) = get(
        app.clone(),
        "/api/v1/events?endpoint_id=ct-rest-001&limit=2",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p1["events"].as_array().unwrap().len(), 2);
    let c1 = p1["next_cursor"].as_i64().expect("还有后页");
    let (st, p2) = get(
        app.clone(),
        &format!("/api/v1/events?endpoint_id=ct-rest-001&limit=2&before_seq={c1}"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p2["events"].as_array().unwrap().len(), 2);
    let c2 = p2["next_cursor"].as_i64().expect("还有后页");
    let (st, p3) = get(
        app.clone(),
        &format!("/api/v1/events?endpoint_id=ct-rest-001&limit=2&before_seq={c2}"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p3["events"].as_array().unwrap().len(), 1);
    assert!(p3["next_cursor"].is_null());
    let mut all: Vec<i64> = [&p1, &p2, &p3]
        .iter()
        .flat_map(|p| p["events"].as_array().unwrap().iter())
        .map(|e| e["seq"].as_i64().unwrap())
        .collect();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 5);

    // 过滤
    let (st, v) = get(app.clone(), "/api/v1/events?category=alarm").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["events"].as_array().unwrap().len(), 3);
    let (st, v) = get(app.clone(), "/api/v1/events?severity_min=400").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["events"].as_array().unwrap().len(), 3);

    // 单条 + 404
    let (st, v) = get(app.clone(), &format!("/api/v1/events/{first_seq}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["seq"], first_seq);
    assert_eq!(v["event"]["event_id"], "seed-0");
    let (st, _) = get(app.clone(), "/api/v1/events/999999999").await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // 无服务 → 503 精确码
    let mgr2 = Arc::new(MesaManager::discover(&repo_root().join("drivers")));
    let cfg2 = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    #[allow(deprecated)]
    let state2 = mesa_core_api::AppState::new(
        mgr2,
        cfg2,
        repo_root().join("drivers").to_string_lossy().to_string(),
    );
    let app2 = mesa_core_api::router(state2);
    let (st, v) = get(app2, "/api/v1/events").await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["error"]["code"], "EVENT_STORE_UNAVAILABLE");

    let _ = std::fs::remove_file(&db);
}

/// ⑤ EventTask REST：PUT/GET 全量快照；运行中 409。
#[tokio::test]
async fn event_task_rest_crud_and_running_conflict() {
    common::init_log();
    let db = tmp_events_db("taskrest");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());

    let drivers_dir = repo_root().join("drivers");
    let cfg_store = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    cfg_store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "D1".into(),
            profile: None,
        })
        .unwrap();
    cfg_store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ct-task-001".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let mgr = Arc::new(MesaManager::discover(&drivers_dir));
    #[allow(deprecated)]
    let state = mesa_core_api::AppState::new(
        mgr.clone(),
        cfg_store,
        drivers_dir.to_string_lossy().to_string(),
    );
    state.set_event_services(EventServices::new(store, EventHub::new(EVENT_HUB_CAPACITY)));
    let app = mesa_core_api::router(state);

    async fn put(
        app: axum::Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .uri(uri)
            .method("PUT")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn get(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    let body = serde_json::json!({"event_tasks": [{
        "id": "al", "mode": "subscribe", "interval_ms": null,
        "binding": {"kind": "simulator.events", "config": {"stream": "sim.events.alarm-cycle"}}
    }]});
    let (st, v) = put(
        app.clone(),
        "/api/v1/endpoints/ct-task-001/event-tasks",
        body,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["revision"], 1);
    let (st, v) = get(app.clone(), "/api/v1/endpoints/ct-task-001/event-tasks").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["event_tasks"].as_array().unwrap().len(), 1);
    assert_eq!(v["event_tasks"][0]["id"], "al");

    // 启动后 PUT → 409（需先提供 data task 才能 start）
    let _ = put(
        app.clone(),
        "/api/v1/endpoints/ct-task-001/event-tasks",
        serde_json::json!({"event_tasks": []}),
    )
    .await;
    // 直接经 manager 启动（带 data task），再 PUT 事件任务 → 409
    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: "ct-task-001".into(),
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
    let (st, v) = put(
        app.clone(),
        "/api/v1/endpoints/ct-task-001/event-tasks",
        serde_json::json!({"event_tasks": []}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(v["error"]["code"], "CONFLICT");
    assert_eq!(mgr.stop_endpoint("ct-task-001").await, Ok(true));

    let _ = std::fs::remove_file(&db);
}

/// 优雅停机证明（Checkpoint B 方案 A）：stop 时正在飞的 commit 必完整
/// publish 后 ingress 才退出——DB 有的行 Hub 全有（commit-but-not-published
/// 缺口不存在）。订阅必须在 Start 前建立（早订阅者视角）。
#[tokio::test]
async fn graceful_shutdown_publishes_every_commit() {
    common::init_log();
    let db = tmp_events_db("graceful");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    // 早订阅：Start 前即位，不漏任何已提交行
    let mut live = hub.subscribe();
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(store.clone(), hub));

    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: "ct-evt-grace".into(),
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
    wait_until(15, || {
        store
            .query_history(&EventFilter {
                endpoint_id: Some("ct-evt-grace".into()),
                ..Default::default()
            })
            .map(|(rows, _)| rows.len() >= 4)
            .unwrap_or(false)
    })
    .await;
    assert_eq!(mgr.stop_endpoint("ct-evt-grace").await, Ok(true));

    // 排空 Hub：DB 有的 event_id 必须全在 Hub 里
    let (db_rows, _) = store
        .query_history(&EventFilter {
            endpoint_id: Some("ct-evt-grace".into()),
            ..Default::default()
        })
        .unwrap();
    let mut hub_ids = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while hub_ids.len() < db_rows.len() && std::time::Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(std::time::Instant::now()),
            live.recv(),
        )
        .await
        {
            Ok(Ok(ev)) => {
                hub_ids.insert(ev.event_id);
            }
            _ => break,
        }
    }
    for r in &db_rows {
        assert!(
            hub_ids.contains(&r.event_id),
            "已提交 {} 必须已发布到 Hub",
            r.event_id
        );
    }
    drop(mgr);
    let _ = std::fs::remove_file(&db);
}

/// ⑨ Stop barrier/drain Gate（生产端到端锁）：洪峰中 Stop，旧 epoch 已进入
/// Core 的 Event 不得因 ingress 先退而被遗弃。
///
/// 完整链（见 `attempt_session` 收尾 + `run_event_ingress`）：Shutdown-post →
/// terminate → 等 reader 结束（TCP FIN 之前全部字节已 pump）→ cancel ingress →
/// final drain 提交 channel 全部。确定性遗弃证明在单测
/// `session::tests::ingress_cancel_drains_backlog`（旧行为 73/200）与
/// `reader_eof_barrier_means_all_batches_pumped`；本测试锁定真实 Stop 路径：
/// 断言逐 epoch `batch_sequence` 无缺口；五轮 stop/start 反复执行 barrier；
/// 末轮重开 events.db 验证 drain 的行真正落盘（不只是 WAL 可见）。
/// 无 sleep 定序：等待只用 `wait_until` 条件轮询 + `stop_endpoint` 的
/// 完成语义。
#[tokio::test]
async fn stop_barrier_drains_inflight_epoch_events() {
    common::init_log();
    let db = tmp_events_db("stopgate");
    let _ = std::fs::remove_file(&db);

    let store = Arc::new(EventStore::open(&db).unwrap());
    let mgr = MesaManager::discover(&repo_root().join("drivers"));
    mgr.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));
    let cfg = || BuiltinEndpoint {
        endpoint_id: "ct-evt-stopgate".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![poll_task(
            "t1",
            50,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![flood_task()],
    };

    // 五轮（总量 <500 行 history 上限，见下方 limit）：旧顺序每轮掉尾概率
    // 高，五轮全过的概率可忽略；新顺序恒过。
    for round in 0..5 {
        mgr.start_endpoint(cfg()).unwrap();
        // 负载形成（≥25 行/轮，producer 仍活跃时直接 Stop——证明的是
        // "accepted tail drains"，不是"永远吸收过载"）
        let base = store.stats().unwrap().rows;
        wait_until(20, || store.stats().unwrap().rows >= base + 25).await;
        assert_eq!(mgr.stop_endpoint("ct-evt-stopgate").await, Ok(true));
        // 逐 epoch 连续性：同 epoch 内 batch_sequence 无缺口、无重复
        let (rows, _) = store
            .query_history(&EventFilter {
                endpoint_id: Some("ct-evt-stopgate".into()),
                limit: Some(500),
                ..Default::default()
            })
            .unwrap();
        let mut by_epoch: std::collections::HashMap<u64, Vec<u64>> =
            std::collections::HashMap::new();
        for r in &rows {
            by_epoch
                .entry(r.stream_epoch)
                .or_default()
                .push(r.batch_sequence);
        }
        assert_eq!(
            by_epoch.len(),
            round + 1,
            "每轮新 epoch，第 {round} 轮后应有 {} 个 epoch",
            round + 1
        );
        for (epoch, mut seqs) in by_epoch {
            seqs.sort_unstable();
            let before = seqs.len();
            seqs.dedup();
            assert_eq!(seqs.len(), before, "epoch {epoch} 有重复 batch_sequence");
            let contiguous = seqs.last().unwrap() - seqs.first().unwrap() + 1 == seqs.len() as u64;
            assert!(
                contiguous,
                "epoch {epoch} 有缺口（Stop 遗弃在途 Event）：{:?}...{:?}",
                &seqs[..seqs.len().min(5)],
                &seqs[seqs.len().saturating_sub(5)..]
            );
        }
    }
    // P0-3：已停止后再停 → Ok(false) 幂等成功（Result<bool> 双变体），
    // 不是"没停掉也返回 true"的旧语义。
    assert_eq!(mgr.stop_endpoint("ct-evt-stopgate").await, Ok(false));
    // 落盘性：重开 events.db，barrier-drain 的行必须全在
    let rows_before = store.stats().unwrap().rows;
    assert!(rows_before >= 125, "五轮应 ≥125 行，实际 {rows_before}");
    drop(store);
    drop(mgr);
    let reopened = EventStore::open(&db).unwrap();
    assert_eq!(reopened.stats().unwrap().rows, rows_before);
    drop(reopened);
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
    assert_eq!(mgr.stop_endpoint("ct-data-001").await, Ok(true));
    drop(mgr);
    let _ = std::fs::remove_file(&db);
}
