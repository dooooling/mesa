//! Stop 生命周期 Gate（有界/显式 + G7 启停竞态 + 停止判据）。
//!
//! **Part 1（既有）**：Stop-during-reconnect 有界显式。
//! 本测试观察的是 crash 之后、teardown 已结束、snapshot 处于 RECONNECTING
//!（退避间隙）的窗口，此时 Stop 取消退避 sleep 并等待 endpoint 任务结束。
//! 它锁的是“重连中 Stop 有界显式”，**不是**“teardown 进行中与 failure 竞争
//! 的真 race”——后者需要进程活着但 stalled 的确定性复现手段（不造平台相关的
//! suspend 原语、不新增驱动故障种类），故不在此冒充覆盖。真 race 的三段式
//! 已有各自 Gate：unresponsive 标记（fault_tolerance 会话级）、有界 drain +
//! fail-closed 超时码（endpoint in-crate）、本测试的有界显式 Stop。
//!
//! 触发器用 simulator 现有 `crash_after_batches` 故障（确定性进程死亡），
//! 不新增驱动故障种类。不改生产语义、不放宽 timeout/断言。
//!
//! 第二个测试（`stop_waits_for_inflight_commit_then_drains_exact`）是真
//! in-flight race 的确定性 Gate：test-only writer 栅栏暂停 commit，并发
//! Stop 必须等待（不得提前返回、不得超时误判），放行后成功 + DB/Hub 精确 +
//! Stop 后无新写。它冻结的是 `reader_done → 无新生产 → in-flight 完成 →
//! 队列排空 → drain ACK → Stop 返回` 这条关系。
//!
//! **Part 2（G7 竞态修复，设计 §9）**：同一套 commit 栅栏 + `StoreFaults`
//! 故障注入，确定性覆盖三个竞态窗口与两条失败语义：
//! - W1 `w1_stop_during_start_construction_leaves_nothing_running`（不变量门，
//!   用冷 Descriptor 放大构造窗口，实测 8/8 命中）；
//! - W2 `w2_start_during_stop_teardown_is_rejected_and_does_not_resurrect`
//!   （栅栏挂住 teardown ⇒ 拆解期 Start 必须 409 `STOP_IN_PROGRESS`）；
//! - W3 `w3_double_start_only_one_attempt`（并发两 Start ⇒ 恰一个 200/一个 409，
//!   Stop 后无孤儿继续采集）；
//! - 异常 teardown（panic 注入）⇒ 隔离态 fail-closed；handler 取消 ⇒ 保护不释放；
//! - DB 落盘失败 ⇒ 500 `STOP_INTENT_NOT_PERSISTED` + 禁止新 Start（S1/S2）；
//! - 白盒：`await_settled` 用完成代次凭证通过、拆解期 `is_running=false` 但
//!   attempt 仍在、删除端点后迟到写入不复活。
//!
//! 这些用例只用 HTTP + 持久化面 + 既有 manager API，因此**在修复前的代码上
//! 同样可编译并真实失败**（旧红证据），不是“没撞到就不红”的采样。

mod common;
mod event_common;

use std::time::Duration;

use event_common::{rows_of, tmp_db};
use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_event_store::{EVENT_HUB_CAPACITY, EventHub, EventServices, EventStore, StoreFaults};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_config_store::{ConfigStore, DeviceRecord, EndpointRecord};
use tower::ServiceExt;

/// staged drivers 目录（含 test-driver；正式 drivers/ 已无 test-driver）。
/// 返回 guard 必须由调用方持有（drop 即删目录）。
fn drivers_dir() -> common::StagedDriversDir {
    common::staged_drivers_with_test_driver()
}

/// Stop 错误码冻结集合：`stop_endpoint` 的 `Err(String)` 必须是以下精确码之一
/// 开头（`EventDrainError::code` / manager 的精确码前缀），禁止空消息或未定义码。
///
/// G7（设计 §6.8）：**精确扩一项** `ATTEMPT_FAILED_UNKNOWN`——teardown 异常终止
/// （panic/abort）时该 attempt 未证明收敛，`stop_endpoint` 必须返回可机器识别的
/// 同一失败。原有七个 `EVENT_*` 码**一字未改**，集合仍是白名单（不是"任意字符串"）。
/// `STOP_INTENT_NOT_PERSISTED` **不在此集合**：它由 core-api 的门产生（落盘失败），
/// 不经过 `stop_endpoint` 的 `Err`。
fn assert_frozen_stop_code(msg: &str) {
    const FROZEN: [&str; 8] = [
        "EVENT_DRAIN_TIMEOUT",
        "EVENT_DRAIN_FAILED",
        "EVENT_SEQUENCE_REGRESSION",
        "EVENT_ID_COLLISION",
        "EVENT_STORE_UNAVAILABLE",
        "EVENT_RECORD_INVALID",
        "EVENT_STREAM_CLOSED",
        "ATTEMPT_FAILED_UNKNOWN",
    ];
    assert!(
        FROZEN.iter().any(|c| msg.starts_with(c)),
        "Stop Err 必须是冻结码集合成员，实际: {msg:?}",
    );
}

/// 重连退避中显式 Stop：40s 内必须返回显式结论，之后端点不得再运行。
/// data 任务触发 crash（2 批即死），event 任务保证 ingress/drain 路径被执行到。
#[tokio::test]
async fn stop_while_reconnecting_is_bounded_and_explicit() {
    common::init_log();
    let db = tmp_db("stop-reconnect");
    let _ = std::fs::remove_file(&db);
    let store = std::sync::Arc::new(EventStore::open(&db).unwrap());
    let services = EventServices::new(store.clone(), EventHub::new(EVENT_HUB_CAPACITY));
    let _staged = drivers_dir();
    let mgr = std::sync::Arc::new(MesaManager::discover(_staged.path()));
    mgr.set_event_services(std::sync::Arc::clone(&services));
    let snapshot = mgr.snapshot();
    let ep = "hd-stop-reconnect";

    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_test_driver::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: ep.into(),
        driver_id: "test-driver".into(),
        // 第 2 个 data 批后进程退出（50ms poll → 约 100ms 后死亡）。
        connection_json: r#"{"crash_after_batches":2}"#.into(),
        tasks: vec![common::poll_task(
            "d",
            50,
            serde_json::json!([{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]),
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

    // 存活证据：行落盘（crash 前数据确实流动，不是起不来）。
    common::wait_until(30, || !rows_of(&store, ep).is_empty()).await;
    // 死亡证据：crash 后 snapshot 必现 RECONNECTING 窗口（Lost→退避间隙，
    // 50ms 轮询必抓到；注意是 RECONNECTING 而非 FAILED——后者只用于配置失败）。
    common::wait_until(
        60,
        || matches!(snapshot.endpoint(ep), Some(ref s) if s.state == "RECONNECTING"),
    )
    .await;

    // 重连退避中显式 Stop：40s（≈2× teardown 最坏预算）内必须返回，
    // 结论显式（Ok 布尔或冻结码 Err），不 hang、不静默。
    let res = tokio::time::timeout(Duration::from_secs(40), mgr.stop_endpoint(ep))
        .await
        .expect("重连中 Stop 必须有界返回");
    match &res {
        Ok(was_running) => eprintln!("stop outcome: Ok({was_running})"),
        Err(msg) => {
            assert_frozen_stop_code(msg);
            eprintln!("stop outcome: Err({msg})");
        }
    }
    assert!(
        !mgr.is_running(ep),
        "Stop 返回后端点不得再运行，实际 outcome={res:?}"
    );
    let _ = std::fs::remove_file(&db);
}

/// Stop 等待 in-flight commit 的确定性 Gate（main CI #103 真因冻结），
/// 且是旧实现的真回归测试（old 必红 / new 必绿），不是"没撞到就不红"的采样.
///
/// 用 test-only writer 栅栏把一个 commit 暂停在"已发送、未执行"状态，
/// 此时并发 Stop 必须**等待**（500ms 内不得返回——提前返回或超时误判都是 bug），
/// 然后继续卡住直到 Stop 发起后约 12s 才放行：
///
/// ```text
/// 旧实现（drain timer 5s，起于 shutdown_ingress ≈ Stop 后 0.5s）：
///     timer 在约 5.5s 先赢 → 确定性返回 Err(EVENT_DRAIN_TIMEOUT) → 测试红
/// 新实现（drain timer 15s，可组合预算）：
///     放行（12s）恒早于 timer（起于 ≥0s，15s 后才赢）→ drain 成功 → 测试绿
/// ```
///
/// 判别力推导：旧实现失败 ⟺ shutdown_ingress 在 Stop 后 7s 内开始。
/// 健康驱动（simulator 响应 Shutdown）约 0.5s 内到达，14× 裕度；
/// 均匀调度膨胀下 sleep 与 pre 同比拉伸，关系保持。新实现通过是无条件的
/// （放行时刻恒早于新 timer，与 pre 无关）。仅当 teardown 前置阶段全部
/// 病态拉满（pre ≥ 7s）时判别力退化——超出任何 timing Gate 的覆盖范围，
/// 届时测试按"Stop 有界"继续断言，不误报。
///
/// 放行后 Stop 成功，且 DB/Hub 精确、Stop 后无新写。冻结的关系：
/// `reader_done → 无新生产 → in-flight 完成 → 队列排空 → drain ACK → Stop 返回`。
///
/// 时序全部由栅栏握手决定，不赌调度：`entered() >= 1` 之前 Stop 尚未发起；
/// 栅栏关闭期间 Stop 不可能完成（drain 需要该 commit 的 reply）。
#[tokio::test]
async fn stop_waits_for_inflight_commit_then_drains_exact() {
    common::init_log();
    let db = tmp_db("stop-gate");
    let _ = std::fs::remove_file(&db);
    // 故障器仅提供栅栏能力，不注入失败（new(0, false) 永不失败）。
    let faults = std::sync::Arc::new(StoreFaults::new(0, false));
    let store = std::sync::Arc::new(
        EventStore::open_with_faults(&db, std::sync::Arc::clone(&faults)).unwrap(),
    );
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let services = EventServices::new(store.clone(), std::sync::Arc::clone(&hub));
    let _staged = drivers_dir();
    let mgr = std::sync::Arc::new(MesaManager::discover(_staged.path()));
    mgr.set_event_services(std::sync::Arc::clone(&services));
    let ep = "hd-stop-gate";

    // Hub 精确送达的见证者：start 之前订阅（broadcast 迟到者收不到旧消息，
    // 订阅必须在生产之前）。结尾一次性 try_recv 排空：本测试总量远小于 Hub
    // 容量（256），Lagged 不可能；比后台排空任务少一个 abort 间隙，无误红窗口。
    let mut hub_rx = hub.subscribe();

    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_test_driver::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: ep.into(),
        driver_id: "test-driver".into(),
        connection_json: "{}".into(),
        tasks: vec![],
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

    // commit 流动证据（栅栏布防前生产正常，不是起不来）。
    common::wait_until(30, || rows_of(&store, ep).len() >= 2).await;

    // 布防 → 等 writer 到达栅栏（commit 已发送、执行中被暂停；entered 计数
    // 让"暂停已生效"可观测，不赌"commit 已经发出来了"）。
    let gate = faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;

    // 并发 Stop：栅栏关闭期间必须等待，不得提前返回（成功或超时都是 bug）。
    let stop_fut = mgr.stop_endpoint(ep);
    tokio::pin!(stop_fut);
    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut stop_fut)
            .await
            .is_err(),
        "栅栏关闭时 Stop 必须等待 in-flight commit，不得提前返回",
    );

    // 判别 hold：继续卡住到 Stop 发起后约 12s。旧 5s timer（起于
    // shutdown_ingress）必在此期间先赢 → 旧代码确定性 TIMEOUT 红；
    // 新 15s timer 不可能赢 → 放行后确定性成功绿。见函数头推导。
    tokio::time::sleep(Duration::from_secs(12)).await;

    // 放行 → Stop 成功（有界）。
    gate.release();
    let res = tokio::time::timeout(Duration::from_secs(30), stop_fut)
        .await
        .expect("放行后 Stop 必须有界返回");
    assert_eq!(res, Ok(true), "放行后 Stop 必须干净成功，实际: {res:?}");

    // 精确：DB 行连续 + persisted 计数 == 行数。
    let rows = rows_of(&store, ep);
    let mut ns: Vec<u64> = rows
        .iter()
        .map(|r| {
            let v: serde_json::Value = serde_json::from_str(&r.attributes_json).unwrap();
            v["value"]["U64"].as_u64().unwrap()
        })
        .collect();
    ns.sort_unstable();
    let max = *ns.last().unwrap();
    assert_eq!(ns.len() as u64, max, "无丢失无重复");
    for w in ns.windows(2) {
        assert_eq!(w[1], w[0] + 1, "n 必须连续");
    }
    {
        use std::sync::atomic::Ordering;
        assert_eq!(
            services
                .diagnostics
                .ingress_persisted_events_total
                .load(Ordering::Relaxed),
            rows.len() as u64,
            "persisted 必须等于行数",
        );
    }

    // 精确：Hub 收到的 == DB 落盘的（commit-then-publish 原子性）。
    // Stop 后 2s 静默窗口：无新生产；然后排空 Hub 比较；同时验证 Stop 后无新写。
    tokio::time::sleep(Duration::from_secs(2)).await;
    let quiet_rows = rows_of(&store, ep).len();
    {
        use std::sync::atomic::Ordering;
        let quiet_persisted = services
            .diagnostics
            .ingress_persisted_events_total
            .load(Ordering::Relaxed);
        assert_eq!(quiet_rows, rows.len(), "Stop 后 DB 不得再出新行");
        assert_eq!(
            quiet_persisted,
            rows.len() as u64,
            "Stop 后 persisted 不得再涨"
        );
    }
    let mut got: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    loop {
        match hub_rx.try_recv() {
            Ok(ev) => {
                got.insert(ev.event_id.clone());
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(e) => panic!("Hub 排空不得失败（总量远小于容量），实际: {e:?}"),
        }
    }
    let want: std::collections::BTreeSet<String> =
        rows.iter().map(|r| r.event_id.clone()).collect();
    assert_eq!(got, want, "Hub 必须恰好收到 DB 全部行（不多不少）");
    let _ = std::fs::remove_file(&db);
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 一个完整的 Core 装配（文件库 + staged drivers + 可注入故障的事件库 + REST）。
struct Rig {
    dir: std::path::PathBuf,
    store: Arc<ConfigStore>,
    mgr: Arc<MesaManager>,
    app: axum::Router,
    faults: Arc<StoreFaults>,
    _staged: common::StagedDriversDir,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "mesa-g7-{tag}-{}-{}",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

impl Rig {
    async fn new(tag: &str, _arm_gate: bool) -> Self {
        common::init_log();
        let dir = temp_dir(tag);
        let db = dir.join("mesa.db");
        let events_db = dir.join("events.db");
        let store = Arc::new(ConfigStore::open(&db).unwrap());
        let faults = Arc::new(StoreFaults::new(0, false));
        let ev_store =
            Arc::new(EventStore::open_with_faults(&events_db, Arc::clone(&faults)).unwrap());
        let services = EventServices::new(ev_store, EventHub::new(EVENT_HUB_CAPACITY));
        let staged = common::staged_drivers_with_test_driver();
        let mgr = Arc::new(MesaManager::with_source(
            staged.path(),
            Arc::new(mesa_driver_manager::StorePointIdSource::new(Arc::clone(
                &store,
            ))),
        ));
        mgr.set_event_services(Arc::clone(&services));
        let state = mesa_core_api::AppState::try_new_with_control(
            Arc::clone(&mgr),
            Arc::clone(&store),
            staged.path().to_string_lossy().to_string(),
            false,
        )
        .unwrap();
        state.set_event_services(services);
        let app = mesa_core_api::router(state);
        Self {
            dir,
            store,
            mgr,
            app,
            faults,
            _staged: staged,
        }
    }

    /// 建 device + endpoint（默认 desired_running=false），可选 data/event 任务。
    fn create_endpoint(&self, id: &str, with_event: bool) -> EndpointRecord {
        self.store
            .create_device(&DeviceRecord {
                id: format!("dev-{id}"),
                name: "dev".into(),
            })
            .unwrap();
        let rec = EndpointRecord {
            id: id.into(),
            name: id.into(),
            device_id: format!("dev-{id}"),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: mesa_core_types::now_unix_ns(),
        };
        self.store.create_endpoint(&rec).unwrap();
        let tasks = vec![common::poll_task(
            "d",
            50,
            serde_json::json!([{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]),
        )];
        self.store.replace_tasks(id, &tasks).unwrap();
        if with_event {
            let binding = mesa_core_types::GenericEventBinding {
                stream_id: mesa_test_driver::SIM_EVENT_STREAM_COUNTER.into(),
                parameters: serde_json::json!({}),
            };
            self.store
                .replace_event_tasks(
                    id,
                    &[EventTask {
                        id: "cnt".into(),
                        mode: TaskMode::Poll,
                        interval_ms: Some(50),
                        binding: DriverBinding {
                            kind: mesa_core_types::GENERIC_EVENT_BINDING_KIND.into(),
                            config: serde_json::to_value(&binding).unwrap(),
                        },
                    }],
                )
                .unwrap();
        }
        rec
    }

    fn desired(&self, id: &str) -> bool {
        self.store
            .get_endpoint(id)
            .unwrap()
            .unwrap()
            .desired_running
    }

    fn state_of(&self, id: &str) -> Option<String> {
        self.mgr.snapshot().endpoint(id).map(|s| s.state)
    }

    async fn post(&self, path: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn start(&self, id: &str) -> (StatusCode, serde_json::Value) {
        self.post(&format!("/api/v1/endpoints/{id}/start")).await
    }

    async fn stop(&self, id: &str) -> (StatusCode, serde_json::Value) {
        self.post(&format!("/api/v1/endpoints/{id}/stop")).await
    }

    /// 冷 Descriptor 缓存：让下一次 Start 的配置构造窗口重新变宽（W1 用）。
    fn cold_descriptor(&self) {
        self.mgr.rescan(self._staged.path());
    }

    async fn wait_running(&self, id: &str, secs: u64) {
        common::wait_until(secs, || {
            matches!(self.state_of(id).as_deref(), Some("RUNNING"))
        })
        .await;
    }
}

fn code_of(v: &serde_json::Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

// ---------------------------------------------------------------------------
// W1：Stop 落进 Start 的配置构造窗口
// ---------------------------------------------------------------------------

/// 不变量门（设计 §9-2）：**Stop 返回 200 之后，端点不得处于运行态，
/// `desired_running` 不得为真**。对任意交错都必须成立。
///
/// 旧码：Stop 走幂等分支写 `false` 并返回 200，随后 Start 的提交把
/// `desired_running` 写回 true 并拉起 attempt ⇒ 断言必红。
/// 新码：Stop 受理时 `stop_gen+1`，在途 Start 的 commit 比对失败 ⇒ 409
/// `SUPERSEDED_BY_STOP`；或 Stop 认领了已提交的 attempt 并拆解干净。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w1_stop_during_start_construction_leaves_nothing_running() {
    let rig = Rig::new("w1", false).await;
    let mut hit_window = 0usize;
    for i in 0..8 {
        let id = format!("w1-{i}");
        rig.create_endpoint(&id, false);
        // 冷缓存 ⇒ 配置构造（Descriptor 临时进程）窗口为百毫秒级，Stop 才有机可乘。
        rig.cold_descriptor();

        let start_task = {
            let app = rig.app.clone();
            let uri = format!("/api/v1/endpoints/{id}/start");
            tokio::spawn(async move {
                let req = Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap();
                let resp = app.oneshot(req).await.unwrap();
                let status = resp.status();
                let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                (
                    status,
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
                )
            })
        };
        // 给 Start 时间进入 S1（构造期），再发 Stop。
        tokio::time::sleep(Duration::from_millis(15)).await;
        if !start_task.is_finished() {
            hit_window += 1;
        }
        let (stop_status, stop_body) = rig.stop(&id).await;
        assert_eq!(
            stop_status,
            StatusCode::OK,
            "Stop 必须成功受理: {stop_body}"
        );

        // Start 可能在 Stop 之前提交、也可能被作废；两种交错都不允许留下运行态。
        let (start_status, start_body) = tokio::time::timeout(Duration::from_secs(60), start_task)
            .await
            .expect("Start 必须有界返回")
            .expect("Start 任务不得 panic");
        if start_status == StatusCode::CONFLICT {
            let c = code_of(&start_body);
            assert!(
                c == "SUPERSEDED_BY_STOP" || c == "STOP_IN_PROGRESS",
                "竞态分支的 409 必须是可机器识别的 Stop 优先码，实际: {start_body}"
            );
        } else {
            assert_eq!(
                start_status,
                StatusCode::OK,
                "非冲突分支必须成功: {start_body}"
            );
        }

        // 关键不变量：Stop 之后不得仍在运行，desired 不得为真。
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !rig.mgr.is_running(&id),
            "第 {i} 轮：Stop 返回 200 之后端点仍在运行（W1 竞态）"
        );
        assert!(
            !rig.desired(&id),
            "第 {i} 轮：Stop 返回 200 之后 desired_running 仍为 true（W1 竞态）"
        );
        assert_ne!(
            rig.state_of(&id).as_deref(),
            Some("RUNNING"),
            "第 {i} 轮：快照仍显示 RUNNING"
        );

        // 清场（下一次迭代用新 id，这里只保证不残留 attempt）
        let _ = rig.stop(&id).await;
        let _ = rig.store.delete_endpoint(&id);
    }
    eprintln!("W1: 命中构造窗口 {hit_window}/8 轮");
}

// ---------------------------------------------------------------------------
// W2：Start 落进 Stop 的拆解窗口（确定性）
// ---------------------------------------------------------------------------

/// 用既有 `StoreFaults::arm_commit_gate` 把 Stop 的 teardown **确定性**挂在
/// "in-flight commit 未完成"处，然后发 Start：
/// - 旧码：`is_running()` 在 remove 之后恒假 ⇒ Start 200 并拉起第二个 attempt（红）；
/// - 新码：attempt 条目仍在（Stopping）⇒ 409 `STOP_IN_PROGRESS`（绿）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w2_start_during_stop_teardown_is_rejected_and_does_not_resurrect() {
    let rig = Rig::new("w2", true).await;
    let id = "w2-ep";
    rig.create_endpoint(id, true);

    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;
    // 数据流动证据（不是起不来）。
    common::wait_until(30, || {
        rig.mgr
            .snapshot()
            .latest_all()
            .iter()
            .any(|p| p.endpoint_id == id)
    })
    .await;

    // 布防：下一个 commit 在执行前被挂住 ⇒ Stop 的 drain 必须等它。
    let gate = rig.faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;

    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        })
    };
    // 等 Stop 真正进入拆解（旧码此刻已 remove 条目 ⇒ is_running 恒假）。
    common::wait_until(10, || !rig.mgr.is_running(id)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (s, v) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "拆解期 Start 必须冲突、不得排队后自动启动，实际: {s} {v}"
    );
    assert_eq!(
        code_of(&v),
        "STOP_IN_PROGRESS",
        "必须是可机器识别的 Stop 在途码，实际: {v}"
    );

    gate.release();
    let (s, v) = tokio::time::timeout(Duration::from_secs(40), stop_task)
        .await
        .expect("放行后 Stop 必须有界返回")
        .expect("Stop 任务不得 panic");
    assert_eq!(s, StatusCode::OK, "Stop 必须成功: {v}");
    assert_eq!(v["was_running"], true, "Stop 必须报告它真的停过一次: {v}");

    // 收敛断言：无 attempt、快照 STOPPED、desired=false。
    assert!(!rig.mgr.is_running(id), "Stop 后不得仍在运行");
    common::wait_until(15, || rig.state_of(id).as_deref() == Some("STOPPED")).await;
    assert!(!rig.desired(id), "Stop 后 desired_running 必须为 false");

    // 且 Stop 之后确实没有第二个 attempt 在采集（孤儿检测）：2s 内无新值推进。
    let before = rig
        .mgr
        .snapshot()
        .latest_all()
        .into_iter()
        .find(|p| p.endpoint_id == id)
        .map(|p| p.timestamp_ns)
        .unwrap_or(0);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = rig
        .mgr
        .snapshot()
        .latest_all()
        .into_iter()
        .find(|p| p.endpoint_id == id)
        .map(|p| p.timestamp_ns)
        .unwrap_or(0);
    assert_eq!(
        before, after,
        "Stop 完成后不得仍有 attempt 在写采集值（孤儿）"
    );
}

// ---------------------------------------------------------------------------
// W3：双 Start 并发 ⇒ 恰好一个 attempt
// ---------------------------------------------------------------------------

/// 并发两 Start：恰好一个 200、另一个 **409**（旧码在非竞态下给 400，竞态下给
/// 两个 200 并留下孤儿 ⇒ 两条断言都会红）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w3_double_start_only_one_attempt() {
    let rig = Rig::new("w3", false).await;
    // 预热 Descriptor：让两个 handler 尽量同时抵达提交点（放大竞态窗口）。
    let warm = "w3-warm";
    rig.create_endpoint(warm, false);
    let _ = rig.start(warm).await;
    let _ = rig.stop(warm).await;
    let _ = rig.store.delete_endpoint(warm);

    for i in 0..12 {
        let id = format!("w3-{i}");
        rig.create_endpoint(&id, false);
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let app = rig.app.clone();
            let uri = format!("/api/v1/endpoints/{id}/start");
            tasks.push(tokio::spawn(async move {
                let req = Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap();
                let resp = app.oneshot(req).await.unwrap();
                let status = resp.status();
                let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                (
                    status,
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
                )
            }));
        }
        let mut oks = 0;
        let mut conflicts = 0;
        for t in tasks {
            let (s, v) = t.await.unwrap();
            match s {
                StatusCode::OK => oks += 1,
                StatusCode::CONFLICT => {
                    conflicts += 1;
                    assert_eq!(
                        code_of(&v),
                        "CONFLICT",
                        "重复 Start 的冲突码必须是 CONFLICT，实际: {v}"
                    );
                }
                other => panic!("重复 Start 只允许 200/409，实际 {other}: {v}"),
            }
        }
        assert_eq!(oks, 1, "第 {i} 轮：并发两 Start 必须恰好一个成功");
        assert_eq!(conflicts, 1, "第 {i} 轮：另一个必须是 409 冲突");

        // Stop 之后不得有孤儿仍在采集。
        let (s, v) = rig.stop(&id).await;
        assert_eq!(s, StatusCode::OK, "Stop: {v}");
        tokio::time::sleep(Duration::from_millis(800)).await;
        let t1 = rig
            .mgr
            .snapshot()
            .latest_all()
            .into_iter()
            .find(|p| p.endpoint_id == id)
            .map(|p| p.timestamp_ns);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let t2 = rig
            .mgr
            .snapshot()
            .latest_all()
            .into_iter()
            .find(|p| p.endpoint_id == id)
            .map(|p| p.timestamp_ns);
        assert_eq!(t1, t2, "第 {i} 轮：Stop 后仍有孤儿在采集");
        assert!(!rig.mgr.is_running(&id));
        let _ = rig.store.delete_endpoint(&id);
    }
}

// ---------------------------------------------------------------------------
// 异常 teardown（panic）⇒ 隔离态，禁止新 Start
// ---------------------------------------------------------------------------

/// 在 teardown 第 2 步之前 panic：teardown **未证明完成**。
/// - 旧码：条目在 await 前就被 remove ⇒ 随后 Start 200（红）；
/// - 新码：条目保留为隔离态 ⇒ Start 409 `ATTEMPT_FAILED_UNKNOWN`（绿），
///   Stop 500 且 detail 以同一码开头；reaper 复核通过后才允许新 Start。
///
/// 为什么用 `set_test_hold_reaper`：真实收敛证明可能在微秒级完成（子进程已被
/// `kill_on_drop` 终止、会话键可摘除），隔离态窗口短到无法确定性观察。挂住
/// reaper **只是保持已有状态**，不改变生产语义（生产构建无该字段）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn teardown_panic_quarantines_and_blocks_start() {
    let rig = Rig::new("panic", false).await;
    let id = "panic-ep";
    rig.create_endpoint(id, false);
    rig.mgr.set_test_hold_reaper(true);
    rig.mgr.set_test_panic_before_teardown(true);

    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;

    // Stop 触发 panic：必须显式失败（不得 200）。
    let (s, v) = rig.stop(id).await;
    assert_eq!(
        s,
        StatusCode::INTERNAL_SERVER_ERROR,
        "teardown 异常必须显式 500: {v}"
    );
    assert_eq!(code_of(&v), "STOP_FAILED", "Stop 失败码: {v}");
    let detail = v["error"]["message"].as_str().unwrap_or("").to_string();
    assert!(
        detail.starts_with("ATTEMPT_FAILED_UNKNOWN"),
        "失败 detail 必须以冻结码开头，实际: {detail}"
    );

    // 隔离态：新 Start 必须被拒（旧码此处 200）。
    let (s, v_start) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "teardown 未证明完成时 Start 必须被拒（fail-closed），实际: {s} {v_start}"
    );
    assert_eq!(code_of(&v_start), "ATTEMPT_FAILED_UNKNOWN", "{v_start}");

    // 再次 Stop：不重复 teardown，返回同一次明确失败（不是 200）。
    let (s2, v2) = rig.stop(id).await;
    assert_eq!(s2, StatusCode::INTERNAL_SERVER_ERROR, "{v2}");
    assert_eq!(code_of(&v2), "STOP_FAILED", "{v2}");
    assert_eq!(
        v2["error"]["message"].as_str().unwrap_or(""),
        detail,
        "两个 Stop 必须拿到同一次失败描述"
    );

    // 复核通过（放行 reaper）后，条目才被移除 ⇒ 新 Start 允许（新代次）。
    rig.mgr.set_test_panic_before_teardown(false);
    rig.mgr.set_test_hold_reaper(false);
    let mut started = false;
    for _ in 0..60 {
        let (s, _) = rig.start(id).await;
        if s == StatusCode::OK {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        started,
        "reaper 复核通过后必须允许全新 Start（fail-closed 不得变成永久不可用）"
    );
    let _ = rig.stop(id).await;
}

// ---------------------------------------------------------------------------
// handler 取消：丢弃 Stop future 不得释放保护
// ---------------------------------------------------------------------------

/// 用 commit 栅栏挂住 teardown，然后**丢弃 Stop 的 future**（等价客户端断开）：
/// (a) teardown 仍在推进；(b) 保护仍在（Start 仍 409）；(c) 放行后收敛。
/// 旧码在 (b) 处必然 200（保护随 handler 消失）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_protection_survives_handler_cancel() {
    let rig = Rig::new("cancel", true).await;
    let id = "cancel-ep";
    rig.create_endpoint(id, true);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;
    common::wait_until(30, || {
        rig.mgr
            .snapshot()
            .latest_all()
            .iter()
            .any(|p| p.endpoint_id == id)
    })
    .await;

    let gate = rig.faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;

    // 客户端超时：只给 300ms，然后**丢弃 future**（等价客户端断开/网关超时，
    // 不是 abort 服务端任务）。必须真正 drop 掉 future 本体，否则取消不会发生。
    let stop_fut = Box::pin(rig.stop(id));
    let mut stop_fut = stop_fut;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut stop_fut)
            .await
            .is_err(),
        "栅栏关闭时 Stop 必须等待 in-flight commit"
    );
    drop(stop_fut);

    // (b) 保护仍在：Start 必须 409（旧码：200 并拉起第二个 attempt）。
    let (s, v) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "客户端断开后保护不得释放：Start 必须仍被拒，实际: {s} {v}"
    );
    assert_eq!(code_of(&v), "STOP_IN_PROGRESS", "{v}");

    // (c) 放行 → 后台 teardown 完成 → 条目移除、desired=false、Start 可成功。
    gate.release();
    common::wait_until(30, || !rig.mgr.is_running(id)).await;
    assert!(
        !rig.desired(id),
        "后台 teardown 完成后 desired 必须为 false"
    );
    common::wait_until(15, || rig.state_of(id).as_deref() == Some("STOPPED")).await;
    let (s, v) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "旧 attempt 确认结束后允许全新 Start: {v}"
    );
    let _ = rig.stop(id).await;
}

// ---------------------------------------------------------------------------
// DB 失败：显式 500 + teardown 照做 + 禁止新 Start（S1/S2 的核心用例）
// ---------------------------------------------------------------------------

/// 注入 `set_desired_running` 失败：
/// - Stop ⇒ 500 `STOP_INTENT_NOT_PERSISTED`（冻结响应体），但 teardown 已完成；
/// - 随后 Start ⇒ 503 `INTENT_UNPERSISTED`（禁止不受控重启）；
/// - 恢复落盘后重新 Stop 可清除标记，Start 恢复可用；
/// - 持久化围栏已写入（S2：崩溃 + 重启不会自动拉起，见 `mesad_restart`）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_intent_persist_failure_is_explicit_and_blocks_restart() {
    let rig = Rig::new("dbfail", false).await;
    let id = "dbfail-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;
    assert!(rig.desired(id), "预置：启动后 desired_running 必须为 true");

    rig.store
        .desired_write_faults()
        .fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (s, v) = rig.stop(id).await;
    assert_eq!(
        s,
        StatusCode::INTERNAL_SERVER_ERROR,
        "落盘失败必须显式 500，不得静默成功: {v}"
    );
    assert_eq!(code_of(&v), "STOP_INTENT_NOT_PERSISTED", "{v}");
    assert_eq!(
        v["error"]["message"], "停止已执行，但停止意图未能持久化；禁止重新启动，需人工核验",
        "响应体冻结，逐字一致"
    );
    // teardown 照做（安全方向不受 DB 影响）。
    assert!(!rig.mgr.is_running(id), "落盘失败不得阻止实际停止");
    common::wait_until(15, || rig.state_of(id).as_deref() == Some("STOPPED")).await;
    // 持久化面仍是旧值 true（诚实声明），但围栏已写。
    assert!(
        rig.desired(id),
        "DB 仍是旧值 true（这正是围栏要覆盖的残余风险）"
    );
    assert!(
        rig.store
            .stop_intent_unpersisted()
            .unwrap()
            .contains(&id.to_string()),
        "必须写入持久化围栏，供重启时失败关闭"
    );

    // 禁止新 Start。
    let (s, v) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::SERVICE_UNAVAILABLE,
        "意图未持久化期间 Start 必须 503，实际: {s} {v}"
    );
    assert_eq!(code_of(&v), "INTENT_UNPERSISTED", "{v}");

    // 恢复落盘 + 再 Stop ⇒ 补写成功 ⇒ 清标记、清围栏，Start 恢复可用。
    rig.store
        .desired_write_faults()
        .fail
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let (s, v) = rig.stop(id).await;
    assert_eq!(s, StatusCode::OK, "补写成功后 Stop 应恢复正常: {v}");
    assert!(!rig.desired(id), "补写后 DB 必须为 false");
    assert!(
        !rig.store
            .stop_intent_unpersisted()
            .unwrap()
            .contains(&id.to_string()),
        "补写成功后围栏必须清除"
    );
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "补写成功后 Start 必须可用: {v}");
    let _ = rig.stop(id).await;
}

/// S1：**慢速**的意图落盘不得推迟实际停止。
///
/// 注入 2.5s 的写入延迟后发 Stop：在响应返回之前（约 1s 处）端点必须已经停干净。
/// 若实现把"落盘"串在 teardown 之前（设计原稿的顺序），teardown 要等到落盘
/// 结束才开始 ⇒ 该断言必红。这是对**实现约束**的守卫（旧码本来就在 teardown
/// 之后落盘，所以它同样绿——它守的是"不要写成落盘优先"）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_desired_write_does_not_delay_teardown() {
    let rig = Rig::new("slowdb", false).await;
    let id = "slowdb-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;

    rig.store
        .desired_write_faults()
        .delay_ms
        .store(2500, std::sync::atomic::Ordering::Relaxed);

    let t0 = std::time::Instant::now();
    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            (resp.status(), t0)
        })
    };
    // 落盘还卡着的时候，teardown 必须已经完成。
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        !stop_task.is_finished(),
        "响应必须等落盘结果（否则无法区分成功/未持久化）"
    );
    assert!(
        !rig.mgr.is_running(id),
        "S1：DB 慢不得阻止实际停止（teardown 必须已开始并完成）"
    );
    common::wait_until(10, || rig.state_of(id).as_deref() == Some("STOPPED")).await;

    let (status, _) = stop_task.await.unwrap();
    assert_eq!(status, StatusCode::OK, "落盘最终成功 ⇒ 200");
    assert!(t0.elapsed() >= Duration::from_millis(2500), "确实等了落盘");
    assert!(!rig.desired(id));
    rig.store
        .desired_write_faults()
        .delay_ms
        .store(0, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// 防回归
// ---------------------------------------------------------------------------

/// 幂等：完成后再 Stop ⇒ 200 `was_running=false`（既有语义冻结）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_is_idempotent_after_completion() {
    let rig = Rig::new("idem", false).await;
    let id = "idem-ep";
    rig.create_endpoint(id, false);

    // 从未启动：仍是 200 + false。
    let (s, v) = rig.stop(id).await;
    assert_eq!(s, StatusCode::OK, "从未启动的 Stop 必须幂等成功: {v}");
    assert_eq!(v["was_running"], false, "{v}");
    assert!(!rig.desired(id), "幂等分支同样要落盘 false");

    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    rig.wait_running(id, 30).await;
    let (s, v) = rig.stop(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["was_running"], true, "{v}");

    // 完成之后再来一次：必须是 200 + false（不得重复 teardown、不得报错）。
    let (s, v) = rig.stop(id).await;
    assert_eq!(s, StatusCode::OK, "完成后的 Stop 必须幂等: {v}");
    assert_eq!(v["was_running"], false, "{v}");
}

/// 运行中再 Start ⇒ 409（现有文案语义不变）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_conflict_while_running_still_409() {
    let rig = Rig::new("conflict", false).await;
    let id = "conflict-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    rig.wait_running(id, 30).await;
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::CONFLICT, "运行中再 Start 必须 409: {v}");
    assert_eq!(code_of(&v), "CONFLICT", "{v}");
    let _ = rig.stop(id).await;
}

/// P0-2 黑盒：**拆解期** PUT（改名/改连接）不得把 `desired_running` 覆盖回 true。
///
/// 复现的是真实 TOCTOU 链（设计 F25/F27）：
/// 1. Endpoint 运行中，DB 里 `desired_running=true`；
/// 2. Stop 受理 ⇒ 拆解开始，`is_running()` 立刻变假（旧码 remove 在 await 之前）；
/// 3. PUT 因此在拆解期被放行：它**先读** `rec`（desired=true）→ 冷 Descriptor
///    异步校验（百毫秒级）→ 再把旧对象整体写回；
/// 4. Stop 的 teardown 结束并写入 false，随后 PUT 把读到的旧值 true 写回。
///
/// 旧码：最终 DB 为 true（红）；新码：UPDATE 不含该列 ⇒ 物理上无法覆盖（绿）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn put_endpoint_does_not_clobber_desired_running() {
    let rig = Rig::new("put", true).await;
    let id = "put-ep";
    rig.create_endpoint(id, true);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    rig.wait_running(id, 30).await;
    assert!(rig.desired(id), "预置：运行中 desired_running 必须为 true");
    common::wait_until(30, || {
        rig.mgr
            .snapshot()
            .latest_all()
            .iter()
            .any(|p| p.endpoint_id == id)
    })
    .await;

    // 用 commit 栅栏把 Stop 的 teardown 确定性挂住，制造"拆解期"窗口。
    let gate = rig.faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;
    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            resp.status()
        })
    };
    common::wait_until(10, || !rig.mgr.is_running(id)).await;

    // 冷 Descriptor：让 PUT 的"读 → 异步校验 → 写回"窗口足够长，
    // 使它的写回确定晚于 Stop 的 false 落盘。
    rig.cold_descriptor();
    let put_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}");
        let body = serde_json::json!({
            "name": "renamed-after-stop",
            "device_id": format!("dev-{id}"),
            "connection": {},
        })
        .to_string();
        tokio::spawn(async move {
            let req = Request::builder()
                .method("PUT")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            resp.status()
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    gate.release();

    let stop_status = tokio::time::timeout(Duration::from_secs(40), stop_task)
        .await
        .expect("Stop 有界")
        .unwrap();
    assert_eq!(stop_status, StatusCode::OK);
    let put_status = tokio::time::timeout(Duration::from_secs(60), put_task)
        .await
        .expect("PUT 有界")
        .unwrap();
    assert_eq!(put_status, StatusCode::OK, "PUT 本身必须成功");

    assert!(
        !rig.desired(id),
        "拆解期 PUT 不得把 Stop 写入的 desired_running=false 覆盖回 true（P0-2）"
    );
    assert_eq!(
        rig.store.get_endpoint(id).unwrap().unwrap().name,
        "renamed-after-stop",
        "PUT 的普通字段必须仍然生效"
    );
}

struct SettleRig {
    dir: std::path::PathBuf,
    store: Arc<ConfigStore>,
    mgr: Arc<MesaManager>,
    app: axum::Router,
    faults: Arc<StoreFaults>,
    state: Arc<mesa_core_api::AppState>,
    _staged: common::StagedDriversDir,
}

impl Drop for SettleRig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl SettleRig {
    async fn new(tag: &str) -> Self {
        common::init_log();
        let dir = std::env::temp_dir().join(format!(
            "mesa-g7w-{tag}-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(ConfigStore::open(&dir.join("mesa.db")).unwrap());
        let faults = Arc::new(StoreFaults::new(0, false));
        let ev_store = Arc::new(
            EventStore::open_with_faults(&dir.join("events.db"), Arc::clone(&faults)).unwrap(),
        );
        let services = EventServices::new(ev_store, EventHub::new(EVENT_HUB_CAPACITY));
        let staged = common::staged_drivers_with_test_driver();
        let mgr = Arc::new(MesaManager::with_source(
            staged.path(),
            Arc::new(mesa_driver_manager::StorePointIdSource::new(Arc::clone(
                &store,
            ))),
        ));
        mgr.set_event_services(Arc::clone(&services));
        let state = mesa_core_api::AppState::try_new_with_control(
            Arc::clone(&mgr),
            Arc::clone(&store),
            staged.path().to_string_lossy().to_string(),
            false,
        )
        .unwrap();
        state.set_event_services(services);
        let app = mesa_core_api::router(Arc::clone(&state));
        Self {
            dir,
            store,
            mgr,
            app,
            faults,
            state,
            _staged: staged,
        }
    }

    fn create_endpoint(&self, id: &str, with_event: bool) {
        self.store
            .create_device(&DeviceRecord {
                id: format!("dev-{id}"),
                name: "dev".into(),
            })
            .unwrap();
        self.create_endpoint_row(id);
        if with_event {
            let binding = mesa_core_types::GenericEventBinding {
                stream_id: mesa_test_driver::SIM_EVENT_STREAM_COUNTER.into(),
                parameters: serde_json::json!({}),
            };
            self.store
                .replace_event_tasks(
                    id,
                    &[mesa_core_types::EventTask {
                        id: "cnt".into(),
                        mode: mesa_core_types::TaskMode::Poll,
                        interval_ms: Some(50),
                        binding: mesa_core_types::DriverBinding {
                            kind: mesa_core_types::GENERIC_EVENT_BINDING_KIND.into(),
                            config: serde_json::to_value(&binding).unwrap(),
                        },
                    }],
                )
                .unwrap();
        }
    }

    /// 只写 endpoint 行 + data 任务（device 复用）。P0-D 的"同 ID 重建"用它：
    /// device 删不掉也无需重建，重建的正是 endpoint 行本身。
    fn create_endpoint_row(&self, id: &str) {
        self.store
            .create_endpoint(&EndpointRecord {
                id: id.into(),
                name: id.into(),
                device_id: format!("dev-{id}"),
                driver_id: "test-driver".into(),
                connection_json: "{}".into(),
                desired_running: false,
                updated_at_ns: mesa_core_types::now_unix_ns(),
            })
            .unwrap();
        let tasks = vec![common::poll_task(
            "d",
            50,
            serde_json::json!([{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]),
        )];
        self.store.replace_tasks(id, &tasks).unwrap();
    }

    async fn post(&self, path: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn start(&self, id: &str) -> (StatusCode, serde_json::Value) {
        self.post(&format!("/api/v1/endpoints/{id}/start")).await
    }

    async fn stop(&self, id: &str) -> (StatusCode, serde_json::Value) {
        self.post(&format!("/api/v1/endpoints/{id}/stop")).await
    }

    /// 通用请求（DELETE / 带 body 的 POST 用）。
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn delete(&self, id: &str) -> (StatusCode, serde_json::Value) {
        self.send("DELETE", &format!("/api/v1/endpoints/{id}"), None)
            .await
    }

    async fn post_json(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.send("POST", path, Some(body)).await
    }

    fn desired(&self, id: &str) -> bool {
        self.store
            .get_endpoint(id)
            .unwrap()
            .unwrap()
            .desired_running
    }

    fn state_of(&self, id: &str) -> Option<String> {
        self.mgr.snapshot().endpoint(id).map(|s| s.state)
    }

    async fn wait_running(&self, id: &str, secs: u64) {
        common::wait_until(secs, || {
            self.mgr
                .snapshot()
                .endpoint(id)
                .is_some_and(|s| s.state == "RUNNING")
        })
        .await;
    }
}

/// P0-3 主门：**正常 Stop 完成后** `await_settled` 必须通过，且返回被移除条目的代次。
///
/// 旧设计（完成代次随条目一起消失）在这一步永远 `NotSettled`——正常停止反而
/// 无法满足判据。`last_completed_gen` 是这条路径唯一的基准。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settled_after_normal_stop_uses_last_completed_generation() {
    let rig = SettleRig::new("settled").await;
    let id = "settled-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    common::wait_until(30, || {
        rig.mgr
            .snapshot()
            .endpoint(id)
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;
    let generation_running = rig.mgr.current_gen(id).expect("运行中必须有代次");
    assert_eq!(generation_running, 1);

    let (s, v) = rig.stop(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(!rig.mgr.has_live_attempt(id), "正常完成后条目必须移除");
    assert_eq!(
        rig.mgr.current_gen(id),
        Some(generation_running),
        "完成代次凭证必须保留（否则判据没有基准）"
    );

    match rig
        .state
        .lifecycle
        .await_settled(id, Duration::from_secs(3))
        .await
    {
        mesa_core_api::lifecycle::Settled::Settled { generation } => {
            assert_eq!(generation, generation_running)
        }
        other => panic!("正常 Stop 完成后必须 settled: {other:?}"),
    }
}

/// `is_running()==false` **不是**"停干净"：拆解期它恒假，而 attempt 条目仍在。
/// 这条差异正是 W2 的根因，也是"保护载体 = 条目"的直接证据。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_attempt_outlives_is_running_during_teardown() {
    let rig = SettleRig::new("teardown").await;
    let id = "teardown-ep";
    rig.create_endpoint(id, true);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    common::wait_until(30, || {
        rig.mgr
            .snapshot()
            .endpoint(id)
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;
    common::wait_until(30, || {
        rig.mgr
            .snapshot()
            .latest_all()
            .iter()
            .any(|p| p.endpoint_id == id)
    })
    .await;

    let gate = rig.faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;
    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        })
    };
    common::wait_until(10, || !rig.mgr.is_running(id)).await;
    assert!(
        rig.mgr.has_live_attempt(id),
        "拆解期 is_running=false 但 attempt 必须仍在（保护不得释放）"
    );
    assert_eq!(
        rig.mgr.attempt_phase(id),
        Some(mesa_driver_manager::AttemptPhase::Stopping)
    );

    gate.release();
    let status = tokio::time::timeout(Duration::from_secs(40), stop_task)
        .await
        .expect("Stop 有界")
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(!rig.mgr.has_live_attempt(id));
}

/// W3 的结构不变量（表级）：同名并发/重复创建被拒绝，**从不覆盖**，
/// 任意时刻每 endpoint 至多一条目；Stop 后表为空。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn double_start_only_one_attempt_at_table_level() {
    let rig = SettleRig::new("double").await;
    let id = "double-ep";
    rig.create_endpoint(id, false);
    // 直接走 manager（绕过 REST 的票据）也必须是结构性的"一个条目"。
    let cfg = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: id.into(),
        driver_id: "test-driver".into(),
        connection_json: "{}".into(),
        tasks: vec![common::poll_task(
            "d",
            50,
            serde_json::json!([{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]),
        )],
        event_tasks: vec![],
    };
    rig.mgr.start_endpoint(cfg.clone()).unwrap();
    let err = rig.mgr.start_endpoint(cfg).unwrap_err();
    assert!(err.contains("already running"), "{err}");
    assert_eq!(
        rig.mgr.running_ids().iter().filter(|x| *x == id).count(),
        1,
        "任意时刻每 endpoint 至多一个 attempt"
    );
    let _ = rig.mgr.stop_endpoint(id).await;
    assert!(!rig.mgr.has_live_attempt(id));
    assert_eq!(rig.mgr.current_gen(id), Some(1));
    assert_eq!(rig.mgr.gen_floor(id), 1, "已发放的最大代次");
    // 进程不变量（设计 §6.6）：Stop 完成后不得残留任何驱动子进程。
    // 允许极短收敛窗口（tokio 孤儿队列异步回收），因此有界轮询而非单次断言。
    let mut live = usize::MAX;
    for _ in 0..40 {
        live = mesa_driver_manager::process::live_children();
        if live == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(live, 0, "Stop 完成后 LIVE_CHILDREN 必须回落到 0（无孤儿）");
}

/// 删除端点后，迟到状态写入不得复活它（REST DELETE 传 `gen_floor` 的端到端证据）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleted_endpoint_not_resurrected_by_late_status_write() {
    let rig = SettleRig::new("delete").await;
    let id = "delete-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    common::wait_until(30, || {
        rig.mgr
            .snapshot()
            .endpoint(id)
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;
    let (s, v) = rig.stop(id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let generation = rig.mgr.current_gen(id).unwrap();

    // DELETE 必须立墓碑（gen_floor = 已发放最大代次）。
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/endpoints/{id}"))
        .body(Body::empty())
        .unwrap();
    let resp = rig.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(rig.mgr.snapshot().endpoint(id).is_none());

    // 迟到写入（旧代次）必须被丢弃，且不得复活。
    let late = mesa_driver_manager::EndpointStatus {
        endpoint_id: id.into(),
        driver_id: "test-driver".into(),
        state: "STOPPED".into(),
        detail: String::new(),
        revision: 0,
        points: 0,
        epoch: 0,
    };
    assert!(
        !rig.mgr.snapshot().upsert_endpoint(generation, late),
        "已删除端点的迟到写入必须被墓碑丢弃"
    );
    assert!(rig.mgr.snapshot().endpoint(id).is_none(), "不得复活");
    let (_, list) = {
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/endpoints")
            .body(Body::empty())
            .unwrap();
        let resp = rig.app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            (),
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        )
    };
    assert!(
        !list["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == id),
        "已删除端点不得出现在 /endpoints: {list}"
    );
}

// ---------------------------------------------------------------------------
// P0-1：Stop 落盘在途期间禁止新 Start（防止 desired 反序）
// ---------------------------------------------------------------------------

/// P0-1 确定性 Gate：**慢 DB**（2.5s）下 Stop 的 teardown 已结束、但
/// `desired_running=false` 仍在落盘途中，此时新 Start 必须被拒（409），
/// 不得出现"实际 RUNNING、DB 却是 false"的反序。
///
/// 旧码：`commit_start` 只看 `stop_gen` 与 attempt 表，teardown 完成后即放行
/// ⇒ Start 写 `true`，2.5s 后旧 Stop 的 `false` 落库覆盖它 ⇒ DB=false 而
/// attempt 在跑。本用例最后一条断言（`desired` 必须为 false 且无 attempt）
/// 在旧码上必红。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_rejected_while_stop_intent_persist_in_flight() {
    let rig = SettleRig::new("p01").await;
    let id = "p01-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;
    assert!(rig.desired(id));

    rig.store
        .desired_write_faults()
        .delay_ms
        .store(4000, std::sync::atomic::Ordering::Relaxed);
    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        })
    };
    // teardown 完成（条目已移除）但落盘仍在途。
    common::wait_until(10, || !rig.mgr.has_live_attempt(id)).await;
    common::wait_until(10, || rig.state_of(id).as_deref() == Some("STOPPED")).await;
    assert!(
        !stop_task.is_finished(),
        "前置：落盘必须仍在途（否则测不到本窗口）"
    );
    assert!(rig.desired(id), "前置：DB 仍是旧值 true");

    // 窗口内 Start：必须 409，且不得产生 attempt、不得写 desired=true。
    let (s, v) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "落盘未确认期间 Start 必须 409（否则反序）：{s} {v}"
    );
    assert_eq!(code_of(&v), "STOP_PERSIST_IN_PROGRESS", "{v}");
    assert!(
        !rig.mgr.has_live_attempt(id),
        "被拒的 Start 不得留下 attempt"
    );

    let status = tokio::time::timeout(Duration::from_secs(40), stop_task)
        .await
        .expect("Stop 有界")
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    // 反序断言：落盘完成后 DB 必须是 false，且没有 attempt 在跑。
    assert!(!rig.desired(id), "落盘完成后 DB 必须为 false");
    assert!(!rig.mgr.has_live_attempt(id), "不得有 attempt 残留");

    rig.store
        .desired_write_faults()
        .delay_ms
        .store(0, std::sync::atomic::Ordering::Relaxed);
    // 落盘确认后 Start 恢复可用。
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "落盘确认后 Start 必须可用: {v}");
    let _ = rig.stop(id).await;
}

// ---------------------------------------------------------------------------
// P0-A：Stop-A 的迟到落盘不得覆盖后续 Start（跟踪全部在途 Stop）
// ---------------------------------------------------------------------------

/// P0-A 确定性 Gate：精确构造评审给出的四步交错。
///
/// 1. Stop-A 受理，但它的 `desired=false` 写库被阻塞（6s）；
/// 2. Stop-B 受理并完成 teardown（**不得**因为 B 完成就清空 A 的在途标记）；
/// 3. 此时新 Start 必须被拒（409 `STOP_PERSIST_IN_PROGRESS`）——若放行，
///    Stop-A 的迟到 SQL 会在之后把 `desired` 写回 false（实际 RUNNING、DB false）；
/// 4. A/B 全部落盘完成后，Start 才允许提交，最终 DB 与运行态一致。
///
/// 旧实现只存**最新**代次：B 完成即清空标记 ⇒ 第 3 步放行 ⇒ 第 4 步 A 的迟到
/// 写入覆盖 true ⇒ 最终 DB=false 而 attempt 在跑。本用例的 409 与最终一致性
/// 断言在旧码上必红。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_stop_a_write_cannot_override_new_start() {
    use std::sync::atomic::Ordering;

    let rig = SettleRig::new("p0a").await;
    let id = "p0a-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;
    assert!(rig.desired(id), "预置：DB 必须为 true");

    // 逐调用延迟：Stop-A 的 false 写 6s，Stop-B 的 false 写 0ms。
    // 初始 Start 的 true 已经完成，不会消费本队列。
    {
        let faults = rig.store.desired_write_faults();
        let mut q = faults.delay_seq.lock().unwrap();
        q.clear();
        q.push_back(6000);
        q.push_back(0);
    }
    let base = rig
        .store
        .desired_write_faults()
        .calls_started
        .load(Ordering::Relaxed);

    // 步骤 1：Stop-A 受理并进入 DB 写（被 6s 延迟阻塞）。
    let stop_a = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        })
    };
    common::wait_until(10, || {
        rig.store
            .desired_write_faults()
            .calls_started
            .load(Ordering::Relaxed)
            > base
    })
    .await;
    assert!(
        !stop_a.is_finished(),
        "前置：Stop-A 的落盘必须仍在途（否则测不到本窗口）"
    );

    // 步骤 2：Stop-B 受理；teardown 很快完成，但 B 的落盘排在 A 之后。
    let stop_b = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        })
    };
    common::wait_until(10, || !rig.mgr.has_live_attempt(id)).await;
    common::wait_until(10, || rig.state_of(id).as_deref() == Some("STOPPED")).await;

    // B 完成后，A 仍在途：集合必须同时包含两代次（不是"只存最新"）。
    common::wait_until(5, || {
        rig.state.lifecycle.sample(id).stop_persist_inflight_count == 2
    })
    .await;
    let sample = rig.state.lifecycle.sample(id);
    assert_eq!(
        sample.stop_persist_inflight_count, 2,
        "P0-A：必须跟踪全部在途 Stop（A 与 B），实际: {sample:?}"
    );
    assert_eq!(
        sample.stop_persist_inflight,
        Some(2),
        "最新在途代次应为 Stop-B 的 2"
    );
    assert!(
        !stop_a.is_finished() && !stop_b.is_finished(),
        "前置：两个 Stop 的落盘都未确认"
    );

    // 步骤 3：新 Start 必须被拒（B 完成不得清空 A 的在途标记）。
    let (s, v) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "P0-A：Stop-A 落盘未确认期间 Start 必须 409: {s} {v}"
    );
    assert_eq!(code_of(&v), "STOP_PERSIST_IN_PROGRESS", "{v}");
    assert!(
        !rig.mgr.has_live_attempt(id),
        "被拒的 Start 不得留下 attempt"
    );
    assert!(
        rig.desired(id),
        "A 尚未落盘：DB 仍是旧值 true（这正是必须拦住 Start 的原因）"
    );

    // 步骤 4：等 A/B 全部落盘完成，再允许 Start；最终 DB 与运行态一致。
    let (sa, va) = tokio::time::timeout(Duration::from_secs(40), stop_a)
        .await
        .expect("Stop-A 有界")
        .unwrap();
    assert_eq!(sa, StatusCode::OK, "{va}");
    let (sb, vb) = tokio::time::timeout(Duration::from_secs(40), stop_b)
        .await
        .expect("Stop-B 有界")
        .unwrap();
    assert_eq!(sb, StatusCode::OK, "{vb}");
    assert!(!rig.desired(id), "两次 Stop 落盘完成后 DB 必须为 false");
    assert!(!rig.mgr.has_live_attempt(id), "不得有 attempt 残留");

    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "全部落盘完成后 Start 必须可用: {v}");
    rig.wait_running(id, 30).await;
    assert!(
        rig.desired(id),
        "P0-A 最终一致性：实际 RUNNING 时 DB 必须为 true"
    );
    assert!(
        rig.mgr.has_live_attempt(id),
        "P0-A 最终一致性：DB=true 时必须有活 attempt"
    );
    let _ = rig.stop(id).await;
}

// ---------------------------------------------------------------------------
// P0-B：慢 Start 落盘不得阻塞 Stop 的 begin_stop
// ---------------------------------------------------------------------------

/// P0-B 确定性 Gate：Start 的 `desired=true` 写库慢 3s，Stop 并发受理。
///
/// 旧实现 `commit_start` 在生命周期门锁内同步写库；Stop 的 `begin_stop` 要取
/// 同一把锁 ⇒ 必须等 Start 的 3s 写完才能 cancel。拆锁后：
/// - `begin_stop`（`admit_stop` 的同步部分）必须 **<200ms** 返回，且立即进入
///   拆解——此时 Start 的 3s 落盘仍未完成；
/// - Start 的响应不得早于 3s（意图落盘先于"认可完成"）；
/// - 最终 DB=false 且无 attempt（Stop 的 false 排在 Start 的 true 之后）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_start_persist_does_not_delay_stop_begin() {
    use std::sync::atomic::Ordering;

    let rig = SettleRig::new("p0b").await;
    let id = "p0b-ep";
    rig.create_endpoint(id, false);
    // Start 的 true 写 3s；Stop 的 false 写 0ms（逐调用队列）。
    {
        let faults = rig.store.desired_write_faults();
        let mut q = faults.delay_seq.lock().unwrap();
        q.clear();
        q.push_back(3000);
        q.push_back(0);
    }
    let base = rig
        .store
        .desired_write_faults()
        .calls_started
        .load(Ordering::Relaxed);

    let t_start = std::time::Instant::now();
    let start_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/start");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        })
    };
    // 等 Start 已产生 attempt 且进入 DB 写（此时它持有落盘串行器，不持有门锁）。
    common::wait_until(10, || {
        rig.store
            .desired_write_faults()
            .calls_started
            .load(Ordering::Relaxed)
            > base
    })
    .await;
    assert!(
        rig.mgr.has_live_attempt(id),
        "前置：Start 必须先启动 attempt 再落盘"
    );
    assert!(!start_task.is_finished(), "前置：Start 仍在等 3s 落盘");

    // 直接调用门的 Stop 受理（REST 的 Stop handler 就是先调它）：测的是
    // `begin_stop` 本身是否被 Start 的慢落盘拖住，不受 HTTP 调度与拆解耗时干扰。
    let t_admit = std::time::Instant::now();
    let admission = rig.state.lifecycle.admit_stop(id);
    let admit_elapsed = t_admit.elapsed();
    assert!(
        admission.handle.was_running(),
        "P0-B：Stop 必须认领 Start 刚创建的 attempt"
    );
    assert!(
        admit_elapsed < Duration::from_millis(200),
        "P0-B 时间上界：begin_stop 必须 <200ms 返回（Start 落盘需 3s），实际 {admit_elapsed:?}"
    );
    assert!(
        !start_task.is_finished(),
        "前置：begin_stop 返回时 Start 的落盘必须仍未完成（否则测不到本窗口）"
    );

    // teardown 结论；Stop 的 false 落盘排在 Start 的 true 之后。
    let was_running =
        tokio::time::timeout(Duration::from_secs(30), admission.handle.await_result())
            .await
            .expect("teardown 有界")
            .expect("teardown 必须成功");
    assert!(was_running, "Stop 必须报告 was_running=true");
    let persist = tokio::time::timeout(Duration::from_secs(30), admission.persist)
        .await
        .expect("Stop 落盘有界")
        .expect("Stop 落盘任务不得 panic");
    assert!(persist.is_ok(), "Stop 落盘必须成功: {persist:?}");

    // Start 的意图落盘必须先于响应返回（拆锁后的硬约束）。
    let (ss, sv) = tokio::time::timeout(Duration::from_secs(20), start_task)
        .await
        .expect("Start 有界")
        .unwrap();
    let start_elapsed = t_start.elapsed();
    assert_eq!(ss, StatusCode::OK, "慢 Start 最终成功: {sv}");
    assert!(
        start_elapsed >= Duration::from_millis(3000),
        "P0-B：Start 必须等自己的意图落盘完成才返回，实际 {start_elapsed:?}"
    );
    assert!(
        rig.store
            .desired_write_faults()
            .calls_completed
            .load(Ordering::Relaxed)
            > base,
        "P0-B：Start 响应返回时，意图写必须已完成"
    );

    assert!(
        !rig.desired(id),
        "P0-B 最终一致性：Stop 的 false 必须排在 Start 的 true 之后（DB=false）"
    );
    assert!(!rig.mgr.has_live_attempt(id), "不得有 attempt 残留");
    common::wait_until(10, || rig.state_of(id).as_deref() == Some("STOPPED")).await;
}

// ---------------------------------------------------------------------------
// R-2：收敛判据必须包含"本次停止意图落盘已确认"
// ---------------------------------------------------------------------------

/// R-2 确定性 Gate：慢写（2.5s）下 teardown 已完成、状态 STOPPED、内存意图
/// 已为 false，但 DB 仍是旧值 true ⇒ `await_settled` **必须** NotSettled。
///
/// 旧码用 `desired_seq >= last_stop_seq`，而 `last_stop_seq` 记录的是写入**前**
/// 的旧序号，落盘一开始就满足 ⇒ 500ms 窗口后误判 Settled（DB 仍为 true，
/// 重启会把这个用户明确要停的端点自动拉起）。落盘完成后才允许 Settled。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn await_settled_requires_confirmed_stop_intent_under_slow_db() {
    use mesa_core_api::lifecycle::Settled;

    let rig = SettleRig::new("r2").await;
    let id = "r2-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;

    rig.store
        .desired_write_faults()
        .delay_ms
        .store(2500, std::sync::atomic::Ordering::Relaxed);
    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        })
    };
    common::wait_until(10, || !rig.mgr.has_live_attempt(id)).await;
    common::wait_until(10, || rig.state_of(id).as_deref() == Some("STOPPED")).await;
    assert!(
        !stop_task.is_finished(),
        "前置：落盘必须仍在途（否则测不到本窗口）"
    );
    assert!(rig.desired(id), "前置：DB 仍是旧值 true");

    // 500ms 收敛窗口 + 余量：落盘未确认 ⇒ 必须 NotSettled。
    match rig
        .state
        .lifecycle
        .await_settled(id, Duration::from_millis(700))
        .await
    {
        Settled::NotSettled { reason, sample } => {
            assert!(
                sample.stop_persist_inflight.is_some(),
                "未确认时必须报告落盘在途: {reason}"
            );
            assert_eq!(
                sample.stop_persist_confirmed_gen, None,
                "未确认时不得有确认凭证: {reason}"
            );
            assert_eq!(sample.desired, Some(false), "内存意图已是 false: {reason}");
        }
        other => panic!("落盘未确认时 await_settled 不得成功: {other:?}"),
    }

    let status = tokio::time::timeout(Duration::from_secs(40), stop_task)
        .await
        .expect("Stop 有界")
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(!rig.desired(id), "落盘完成后 DB 必须为 false");
    rig.store
        .desired_write_faults()
        .delay_ms
        .store(0, std::sync::atomic::Ordering::Relaxed);
    match rig
        .state
        .lifecycle
        .await_settled(id, Duration::from_secs(5))
        .await
    {
        Settled::Settled { .. } => {}
        other => panic!("落盘确认后必须 settled: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// R-1：bootstrap "已启动却删配置"
// ---------------------------------------------------------------------------

/// R-1 确定性 Gate：`commit_start` 已启动 attempt、随后 `desired_running=true`
/// 落盘失败 ⇒ bootstrap **必须先停止已启动的 attempt**，再补偿删除配置。
///
/// 旧码把它当普通 Start 失败，直接删 Endpoint/Device ⇒ 配置已删除、驱动任务
/// 仍在执行（孤儿）。断言 `has_live_attempt == false` 在旧码上必红。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_intent_persist_failure_stops_attempt_before_compensating() {
    let rig = SettleRig::new("r1").await;
    let ep = "r1-ep";
    let task = common::poll_task(
        "d",
        50,
        serde_json::json!([{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]),
    );
    let body = serde_json::json!({
        "device": {"id": "dev-r1", "name": "r1"},
        "endpoint": {
            "id": ep,
            "name": ep,
            "driver_id": "test-driver",
            "connection": {},
        },
        "acquisition": {"tasks": [serde_json::to_value(&task).unwrap()]},
        "start": true,
    });

    // 只让 `set_desired_running` 失败：启动本身成功，意图落盘失败。
    rig.store
        .desired_write_faults()
        .fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (s, v) = rig.post_json("/api/v1/device-bootstrap", body).await;
    assert_eq!(
        s,
        StatusCode::BAD_GATEWAY,
        "bootstrap 必须显式失败（不得谎报成功）: {v}"
    );
    assert_eq!(v["error"]["code"], "START_INTENT_NOT_PERSISTED", "{v}");
    assert_eq!(v["attempt_stopped"], true, "必须报告已先停止 attempt: {v}");
    assert_eq!(v["compensated"], true, "停止确认后才允许补偿删除: {v}");
    assert!(
        !rig.mgr.has_live_attempt(ep),
        "R-1：配置已删但驱动任务仍在执行（孤儿）"
    );
    assert!(
        rig.store.get_endpoint(ep).unwrap().is_none(),
        "补偿删除后配置不得残留"
    );
    rig.store
        .desired_write_faults()
        .fail
        .store(false, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// 并发删除
// ---------------------------------------------------------------------------

/// 并发删除：两个 DELETE 竞争 ⇒ 恰好一个 200、一个 404；端点最终不存在。
/// 拆解期 DELETE ⇒ 必须 409（`is_running()==false` 但 attempt 仍在）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_delete_and_delete_during_teardown_are_safe() {
    let rig = SettleRig::new("delete2").await;

    // (a) 并发两个 DELETE。
    let id = "del-race";
    rig.create_endpoint(id, false);
    let (a, b) = tokio::join!(rig.delete(id), rig.delete(id));
    let mut codes = [a.0, b.0];
    codes.sort();
    assert_eq!(
        codes,
        [StatusCode::OK, StatusCode::NOT_FOUND],
        "并发删除必须恰好一个成功、一个 404"
    );
    assert!(
        rig.store.get_endpoint(id).unwrap().is_none(),
        "并发删除后不得残留配置"
    );

    // (b) 拆解期 DELETE：配置不得被删除（否则留下无主驱动任务）。
    // 必须带 event 任务：commit 栅栏挂在事件提交路径上，没有事件就没有可挂住的
    // in-flight commit，也就测不到"拆解中"这个窗口。
    let id2 = "del-teardown";
    rig.create_endpoint(id2, true);
    let (s, v) = rig.start(id2).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id2, 30).await;

    let gate = rig.faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;
    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id2}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        })
    };
    common::wait_until(10, || !rig.mgr.is_running(id2)).await;
    assert!(
        rig.mgr.has_live_attempt(id2),
        "前置：拆解期 attempt 必须仍在"
    );
    let (s, v) = rig.delete(id2).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "拆解期删除配置必须 409（否则驱动任务无主）: {s} {v}"
    );
    assert!(rig.store.get_endpoint(id2).unwrap().is_some());

    gate.release();
    let status = tokio::time::timeout(Duration::from_secs(40), stop_task)
        .await
        .expect("Stop 有界")
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    common::wait_until(10, || !rig.mgr.has_live_attempt(id2)).await;
    let (s, _) = rig.delete(id2).await;
    assert_eq!(s, StatusCode::OK, "拆解完成后才允许删除");
}

// ---------------------------------------------------------------------------
// P0-7：Start / DELETE 竞态 + `set_desired_running` 的 Ok(false)
// ---------------------------------------------------------------------------

/// P0-7 确定性 Gate：`set_desired_running` 返回 `Ok(false)`（UPDATE 命中 0 行，
/// 即端点已被并发删除）时，`commit_start` **不得**当成功——attempt 已经启动，
/// 必须立刻认领停止并返回 409。
///
/// 旧码把 `Ok(true) | Ok(false)` 都当成功 ⇒ 返回 200、`desired_running` 没写成、
/// attempt 在跑、配置行却不存在（配置已删、驱动仍执行）。断言 `!has_live_attempt`
/// 在旧码上必红。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_on_deleted_endpoint_is_aborted_not_orphaned() {
    let rig = SettleRig::new("p07").await;
    let id = "p07-ep";
    rig.create_endpoint(id, false);
    // 模拟并发 DELETE 已经删掉配置行（Start 已构造完成、正准备 commit）。
    assert!(rig.store.delete_endpoint(id).unwrap());

    let ticket = rig.state.lifecycle.admit_start(id);
    let cfg = BuiltinEndpoint {
        endpoint_id: id.into(),
        driver_id: "test-driver".into(),
        connection_json: "{}".into(),
        tasks: vec![common::poll_task(
            "d",
            50,
            serde_json::json!([{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]),
        )],
        event_tasks: vec![],
    };
    let err = rig
        .state
        .lifecycle
        .commit_start(&ticket, cfg)
        .await
        .expect_err("配置行已删除，Start 必须失败");
    assert_eq!(err.code, "ENDPOINT_DELETED", "{}", err.message);
    common::wait_until(10, || !rig.mgr.has_live_attempt(id)).await;
    assert!(
        !rig.mgr.has_live_attempt(id),
        "P0-7：删除后不得留下运行中的 attempt（孤儿）"
    );
}

/// P0-7 并发 Gate：Start 与 DELETE 竞争时，**删除成功就绝不允许留下活 attempt**。
///
/// 两种合法结局：DELETE 先赢（Start 409 / `SUPERSEDED_BY_STOP` 或
/// `ENDPOINT_DELETED`），或 Start 先赢（DELETE 409，端点与 attempt 都还在）。
/// 非法结局（旧码）：DELETE 返回 200，attempt 仍在跑。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_racing_start_never_leaves_orphan_attempt() {
    let rig = SettleRig::new("delrace").await;
    for round in 0..10 {
        let id = format!("delrace-{round}");
        rig.create_endpoint(&id, false);
        let (start_res, del_res) = tokio::join!(rig.start(&id), rig.delete(&id));
        match del_res.0 {
            StatusCode::OK => {
                // 删除成功 ⇒ 端点不存在，且不允许留下任何 attempt。
                common::wait_until(10, || !rig.mgr.has_live_attempt(&id)).await;
                assert!(
                    !rig.mgr.has_live_attempt(&id),
                    "第 {round} 轮：DELETE 成功后仍有 attempt 在跑（孤儿）；start={:?}",
                    start_res.0
                );
                assert!(
                    rig.store.get_endpoint(&id).unwrap().is_none(),
                    "第 {round} 轮：DELETE 成功后配置必须不存在"
                );
            }
            StatusCode::CONFLICT => {
                // Start 先赢：端点与 attempt 都必须还在（不得半删）。
                assert!(
                    rig.store.get_endpoint(&id).unwrap().is_some(),
                    "第 {round} 轮：DELETE 被拒后配置必须仍在"
                );
                assert!(
                    rig.mgr.has_live_attempt(&id),
                    "第 {round} 轮：DELETE 被拒说明有活 attempt，实际没有"
                );
                let _ = rig.stop(&id).await;
            }
            other => panic!("第 {round} 轮：DELETE 只允许 200/409，实际 {other}"),
        }
    }
}

// ---------------------------------------------------------------------------
// P0-D：删除与"未完成的 Stop 落盘"的竞态（DELETE + 同 ID 重建）
// ---------------------------------------------------------------------------

/// P0-D 确定性 Gate：评审给定的五步交错，全部经由 HTTP + 持久化面复现。
///
/// ```text
/// 1. Stop-A 已完成驱动 teardown（attempt 条目已移除），
///    但 desired=false 仍在写库（注入 9s 延迟；DELETE 的等待预算 10s）；
/// 2. DELETE 到达：旧码"看不到活跃 attempt"⇒ 立即删行 + 清空在途标记；
/// 3. CREATE 以**同一 Endpoint ID** 重建，Start 写入 desired=true；
/// 4. 旧 Stop-A 的迟到 SQL 仍会执行 —— 它命中的已经是**另一条记录**；
/// 5. 结果：新实例的运行意图可能被旧实例的异步任务改写（DB 与运行态不一致）。
/// ```
///
/// 修复后：DELETE 必须**等待**那次写入真正收敛（有界 10s）后才删行；等待期间
/// 屏障拒绝新 Start（409 `ENDPOINT_DELETED`）；删除成功才立墓碑代次（`stop_gen`
/// +1），使旧实例的票据/代次永远匹配不上重建后的记录。
///
/// 旧码上**确定性必红**的断言（在"前置：写入仍在途"成立时）：
/// - 删除窗口内的 Start 不是 409（旧码已删行、且清空了在途标记）；
/// - 旧 Stop 的 `desired=false` 未确认之前配置行已被删掉；
/// - DELETE 返回时旧 Stop 的 `desired=false` 尚未确认落库。

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_waits_for_inflight_stop_persist_and_rebuild_is_not_polluted() {
    use std::sync::atomic::Ordering;

    let rig = SettleRig::new("p0d").await;

    let id = "p0d-ep";
    rig.create_endpoint(id, false);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "预置启动: {v}");
    rig.wait_running(id, 30).await;
    assert!(rig.desired(id), "预置：DB 必须为 true");

    // 只让本次 Stop 的 false 写延迟 9s；队列消费完即回落到 0ms，因此重建实例的
    // true 写不受影响。9s 是刻意留的余量：DELETE 的等待预算是 10s（`DELETE_PERSIST_WAIT`），
    // 而 teardown 本身耗时不定（实测几十毫秒，重载时可到秒级）——"Stop 的写入仍在途"
    // 这个窗口必须比 teardown 明显宽，否则本用例会退化成时序采样而不是确定性 Gate。
    {
        let faults = rig.store.desired_write_faults();
        let mut q = faults.delay_seq.lock().unwrap();
        q.clear();
        q.push_back(9000);
    }
    let base = rig
        .store
        .desired_write_faults()
        .calls_started
        .load(Ordering::Relaxed);
    // 旧实例那次 `desired=false` 写库**已执行完毕**的判据（calls_completed 在
    // UPDATE 之后 +1）：用它替代固定 sleep，才能确定性覆盖"迟到写入落地"的时刻。
    let base_completed = rig
        .store
        .desired_write_faults()
        .calls_completed
        .load(Ordering::Relaxed);

    // ── 步骤 1：Stop 受理 → teardown 完成 → desired=false 仍在途 ──
    let stop_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}/stop");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        })
    };
    common::wait_until(10, || {
        rig.store
            .desired_write_faults()
            .calls_started
            .load(Ordering::Relaxed)
            > base
    })
    .await;
    common::wait_until(10, || !rig.mgr.has_live_attempt(id)).await;
    common::wait_until(10, || rig.state_of(id).as_deref() == Some("STOPPED")).await;
    assert!(
        !stop_task.is_finished(),
        "前置：Stop 的落盘必须仍在途（否则测不到本窗口）"
    );
    assert!(rig.desired(id), "前置：DB 仍是旧值 true");
    // 前置的量化判据：旧 Stop 的那次写库调用**尚未完成**（`calls_completed` 在
    // UPDATE 之后才 +1）。步骤 3 会用同一个计数器证明 DELETE 返回时它已完成。
    assert_eq!(
        rig.store
            .desired_write_faults()
            .calls_completed
            .load(Ordering::Relaxed),
        base_completed,
        "前置：旧 Stop 的 desired=false 必须仍在途（已进入写库但未完成）"
    );
    assert_eq!(
        rig.state.lifecycle.sample(id).stop_persist_inflight_count,
        1,
        "前置：在途 Stop 代次必须被门跟踪"
    );

    // ── 步骤 2：DELETE 到达 —— 必须等待，不得"看到没 attempt 就删" ──
    //
    // 判据全部落在可机器判定的状态上（不靠固定 sleep 猜时序）：
    // - 删除窗口内的 Start 一律不得成功（409，且不得留下 attempt / 写入）；
    // - 旧 Stop 的 `desired=false` 未确认之前，配置行不得被删掉；
    // - DELETE 返回时，那次写入必须**已确认落库**（见步骤 3）。
    let delete_task = {
        let app = rig.app.clone();
        let uri = format!("/api/v1/endpoints/{id}");
        tokio::spawn(async move {
            let req = Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        })
    };

    // 窗口内的 Start：必须被拒。两种合法拒绝码取决于 DELETE 是否已立屏障：
    // 屏障已立 ⇒ `ENDPOINT_DELETED`；尚未轮到 handler ⇒ 仍由 P0-A 的在途 Stop
    // 闸门给出 `STOP_PERSIST_IN_PROGRESS`。两者都要求"不得产生 attempt / 写入"。
    // 旧码：DELETE 已删行且清空了在途标记 ⇒ Start 会成功或被 404 挡住，本条即红。
    let (s, v) = rig.start(id).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "删除窗口内的 Start 必须 409: {s} {v}"
    );
    assert!(
        matches!(code_of(&v), "ENDPOINT_DELETED" | "STOP_PERSIST_IN_PROGRESS"),
        "删除窗口内的 Start 必须是可机器识别的拒绝码，实际: {v}"
    );
    assert!(
        !rig.mgr.has_live_attempt(id),
        "被拒的 Start 不得留下 attempt"
    );
    assert!(
        rig.store.get_endpoint(id).unwrap().is_some(),
        "P0-D：旧 Stop 的 desired=false 未确认前，DELETE 不得删行"
    );
    assert!(
        !delete_task.is_finished(),
        "P0-D：旧 Stop 的 desired=false 未确认前，DELETE 不得返回"
    );

    // ── 步骤 3：DELETE 收敛并删行（不在这里等 Stop 应答——否则旧码会因为
    //    等 Stop 而错过"重建发生在迟到写入之前"的污染窗口）──
    let (del_status, del_body) = tokio::time::timeout(Duration::from_secs(40), delete_task)
        .await
        .expect("DELETE 有界")
        .unwrap();
    assert_eq!(
        del_status,
        StatusCode::OK,
        "落盘收敛后 DELETE 必须成功: {del_body}"
    );
    assert!(
        rig.store.get_endpoint(id).unwrap().is_none(),
        "DELETE 后配置行必须不存在"
    );
    // 修复的**核心**：DELETE 返回时，旧 Stop 的 `desired=false` 必须已经
    // **确认落库**（`calls_completed` 在 UPDATE 之后才 +1），而不是"门把在途标记
    // 清掉就算完成"。修复前这里恒为 false（DELETE 在被注入的 6s 窗口内就返回了）。
    assert!(
        rig.store
            .desired_write_faults()
            .calls_completed
            .load(Ordering::Relaxed)
            > base_completed,
        "P0-D：DELETE 返回时旧 Stop 的 desired=false 必须已确认落库"
    );

    // ── 步骤 4：同一 ID 重建 + Start（写 desired=true）──
    let calls_before_rebuild = rig
        .store
        .desired_write_faults()
        .calls_started
        .load(Ordering::Relaxed);
    rig.create_endpoint_row(id);
    let (s, v) = rig.start(id).await;
    assert_eq!(s, StatusCode::OK, "同 ID 重建后 Start 必须成功: {v}");
    rig.wait_running(id, 30).await;
    assert!(rig.desired(id), "新实例：DB 必须为 true");

    // ── 步骤 5：旧实例的迟到写入不得落到新记录上 ──
    // 等**两次**写入都执行完毕再看（`calls_completed` 在 UPDATE 之后才 +1）：
    // 旧 Stop 的 `desired=false` 与新实例的 `desired=true`。只等一次不够——新实例
    // 自己的 true 会先完成，那正是"迟到写入尚未落地"的假绿窗口。
    //
    // 判别力（诚实说明，见 `persist_lock`）：同一 endpoint 的 `desired` 写库由
    // `EpGate::persist_lock` 串行化，且该串行器在 DELETE 后仍被重建实例复用，
    // 因此**仅仅"DB 慢"不会把两次写入的顺序颠倒**——旧写入排在前面时，它的
    // `false` 会被新实例的 `true` 覆盖。真正的污染窗口是：
    //   a) 旧写入还没排上串行器（调度饥饿）⇒ 它可以排在新的 `true` 之后；
    //   b) 新实例的 `true` 写入失败/未发生 ⇒ 旧的 `false` 留在新记录上。
    // 两者都不是靠"注入 DB 延迟"能确定性构造的，所以本用例把可判定性放在前面
    // 三条断言（DELETE 必须等旧写入确认后才删行、删行时行还在、屏障期 Start 409）
    // ——它们在修复前**确定性必红**；这里保留的是"最终 DB 与运行态一致"的收尾断言，
    // 连同"重建后只允许新实例自己的一次写入"一起证明旧实例没有再发出写库调用。
    common::wait_until(20, || {
        rig.store
            .desired_write_faults()
            .calls_completed
            .load(Ordering::Relaxed)
            > base_completed + 1
    })
    .await;
    assert!(
        rig.desired(id),
        "P0-D：旧 Stop 的迟到写入污染了重建后的记录（实际 RUNNING、DB=false）"
    );
    assert!(rig.mgr.has_live_attempt(id), "P0-D：新实例必须在运行");
    assert_eq!(rig.state_of(id).as_deref(), Some("RUNNING"));
    assert_eq!(
        rig.store
            .desired_write_faults()
            .calls_started
            .load(Ordering::Relaxed),
        calls_before_rebuild + 1,
        "P0-D：重建后只允许新实例自己的 true 写入，旧实例不得再发出任何写库调用"
    );

    // Stop 的应答也必须成功（它的 false 已确认落库，不会在之后覆盖新实例）。
    let (stop_status, stop_body) = tokio::time::timeout(Duration::from_secs(40), stop_task)
        .await
        .expect("Stop 有界")
        .unwrap();
    assert_eq!(stop_status, StatusCode::OK, "Stop 必须成功: {stop_body}");
    assert!(
        rig.desired(id),
        "P0-D：Stop 应答返回后新实例的 desired 仍必须为 true"
    );

    // 一致性收尾：DB 与运行态必须同向（RUNNING ⇒ desired=true）。
    let _ = rig.stop(id).await;
    assert!(!rig.desired(id), "收尾 Stop 后 DB 必须为 false");
    assert!(!rig.mgr.has_live_attempt(id));
}
