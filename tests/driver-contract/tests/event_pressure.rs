//! Backpressure / fairness pressure gates（PR10 commit 4 §5/§6）：
//! 生产路径（真 simulator 子进程 → manager → ingress → DB）证明 Event Plane
//! 无 silent drop：burst 全落盘保序；数据负载下事件不饿死；事件 burst 下
//! endpoint 健康。SDK 满等待语义另由 driver-sdk in-crate 测试锁定。

mod common;
mod event_common;

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

use event_common::{rows_of, tmp_db};
use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_event_store::{EVENT_HUB_CAPACITY, EventHub, EventServices, EventStore, StoreFaults};

fn drivers_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("drivers")
}

fn generic_counter_task() -> EventTask {
    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    EventTask {
        id: "cnt".into(),
        mode: TaskMode::Poll,
        // 50ms（~20/s）：可持续速率。100/s 在 debug 构建下超过 ingress
        // 单批 txn 吞吐，会正确触发 fail-closed Lost + 重连（已由 session 级
        // overflow Gate 与 lifecycle torture 分别覆盖），不是本测试的断言对象。
        interval_ms: Some(50),
        binding: DriverBinding {
            kind: GENERIC_EVENT_BINDING_KIND.into(),
            config: serde_json::to_value(&binding).unwrap(),
        },
    }
}

fn data_task_20points() -> serde_json::Value {
    let points: Vec<serde_json::Value> = (0..20)
        .map(|i| serde_json::json!({"key": format!("k.{i}"), "kind": "counter"}))
        .collect();
    serde_json::json!({"points": points})
}

struct Rig {
    mgr: std::sync::Arc<MesaManager>,
    store: std::sync::Arc<EventStore>,
    db: std::path::PathBuf,
    endpoint_id: String,
}

impl Rig {
    fn start(endpoint_id: &str, with_data: bool) -> Self {
        let db = tmp_db("pressure");
        let _ = std::fs::remove_file(&db);
        let store = std::sync::Arc::new(EventStore::open(&db).unwrap());
        let mgr = std::sync::Arc::new(MesaManager::discover(&drivers_dir()));
        mgr.set_event_services(EventServices::new(
            store.clone(),
            EventHub::new(EVENT_HUB_CAPACITY),
        ));
        let mut tasks = vec![];
        if with_data {
            tasks.push(common::poll_task("d", 10, data_task_20points()));
        }
        mgr.start_endpoint(BuiltinEndpoint {
            endpoint_id: endpoint_id.into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            tasks,
            event_tasks: vec![generic_counter_task()],
        })
        .unwrap();
        Self {
            mgr,
            store,
            db,
            endpoint_id: endpoint_id.into(),
        }
    }

    /// counter `n` 序列（attributes.value.U64）：必须 1..=max 连续无缺口。
    /// 附带单 epoch 断言：若中途发生 fail-closed 重连，n 会按新 epoch 重起，
    /// 连续性断言即失效——两者共同证明"吸收了负载且未 failover"。
    fn counter_ns(&self) -> Vec<u64> {
        let rows = rows_of(&self.store, &self.endpoint_id);
        let mut ns: Vec<u64> = rows
            .iter()
            .map(|r| {
                let v: serde_json::Value = serde_json::from_str(&r.attributes_json).unwrap();
                v["value"]["U64"].as_u64().unwrap()
            })
            .collect();
        ns.sort_unstable();
        ns
    }

    async fn stop(self) {
        assert_eq!(self.mgr.stop_endpoint(&self.endpoint_id).await, Ok(true));
        let _ = std::fs::remove_file(&self.db);
    }
}

/// burst 全落盘保序：manager 路径约 20/s，跑 8s 取 ~150 行；n 必须
/// 1..=max 连续（计数 == max，无缺口无重复——静默丢失在此必现形）。
/// 锁的是"不丢+保序"，不是速率（速率由 soak 档覆盖）。
#[tokio::test]
async fn sim_counter_burst_all_persisted_in_order() {
    common::init_log();
    let rig = Rig::start("hd-pressure-burst", false);
    tokio::time::sleep(Duration::from_secs(8)).await;
    let ns = rig.counter_ns();
    assert!(ns.len() >= 100, "8s 应有规模，got {}", ns.len());
    let max = *ns.last().unwrap();
    assert_eq!(ns.len() as u64, max, "计数必须等于 max（无丢失无重复）");
    for w in ns.windows(2) {
        assert_eq!(w[1], w[0] + 1, "counter n 必须连续");
    }
    // 单 epoch（无 failover）：全行同一 stream_epoch
    let rows = rows_of(&rig.store, &rig.endpoint_id);
    let epochs: std::collections::BTreeSet<u64> = rows.iter().map(|r| r.stream_epoch).collect();
    assert_eq!(epochs.len(), 1, "可持续负载下不得触发重连");
    rig.stop().await;
}

/// 数据负载下事件不饿死：20 点 @10ms 数据洪流 + 20/s 事件，10s 后事件
/// 依然连续落盘，endpoint 健康停止（无 Lost、无 fail-closed）。
#[tokio::test]
async fn data_load_does_not_starve_events() {
    common::init_log();
    let rig = Rig::start("hd-pressure-mixed", true);
    tokio::time::sleep(Duration::from_secs(10)).await;
    let ns = rig.counter_ns();
    assert!(ns.len() >= 100, "10s 混合负载应有规模，got {}", ns.len());
    let max = *ns.last().unwrap();
    assert_eq!(ns.len() as u64, max, "数据洪流下事件不得丢失");
    for w in ns.windows(2) {
        assert_eq!(w[1], w[0] + 1, "数据洪流下事件必须连续");
    }
    rig.stop().await;
}

// ---------------------------------------------------------------------------
// stall 恢复 Gate（hardening）：commit 暂停期间 Core 不判死、放行后精确恢复。
// 速率 SLO 与容量边界的严格证明在 `event_runtime` 的精确批量 Gate
//（`exact_500_events_pipeline_exact_within_budget` /
// `exact_capacity_plus_one_overflow_is_loud`）：数量代替"时间×推测速率"。
// 本文件的 stall 测试按 interval 上限天然 rate-safe（50ms tick 至多 20/s，
// 6s 至多 120 批 ≪ 512，与 runner 快慢无关），保留作 manager 级集成覆盖。
// ---------------------------------------------------------------------------
/// 容量公式测试基座：带故障器（栅栏能力）+ Hub 见证者 + event-only counter 端点。
struct StallRig {
    mgr: std::sync::Arc<MesaManager>,
    store: std::sync::Arc<EventStore>,
    services: std::sync::Arc<EventServices>,
    db: std::path::PathBuf,
    endpoint_id: String,
}

impl StallRig {
    fn start(endpoint_id: &str, faults: std::sync::Arc<StoreFaults>) -> Self {
        let db = tmp_db("capacity");
        let _ = std::fs::remove_file(&db);
        let store = std::sync::Arc::new(EventStore::open_with_faults(&db, faults).unwrap());
        let services = EventServices::new(store.clone(), EventHub::new(EVENT_HUB_CAPACITY));
        let mgr = std::sync::Arc::new(MesaManager::discover(&drivers_dir()));
        mgr.set_event_services(std::sync::Arc::clone(&services));
        Self {
            mgr,
            store,
            services,
            db,
            endpoint_id: endpoint_id.into(),
        }
    }

    fn start_counter(&self, interval_ms: u64) {
        let binding = mesa_core_types::GenericEventBinding {
            stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_COUNTER.into(),
            parameters: serde_json::json!({}),
        };
        self.mgr
            .start_endpoint(BuiltinEndpoint {
                endpoint_id: self.endpoint_id.clone(),
                driver_id: "simulator".into(),
                connection_json: "{}".into(),
                tasks: vec![],
                event_tasks: vec![EventTask {
                    id: "cnt".into(),
                    mode: TaskMode::Poll,
                    interval_ms: Some(interval_ms),
                    binding: DriverBinding {
                        kind: GENERIC_EVENT_BINDING_KIND.into(),
                        config: serde_json::to_value(&binding).unwrap(),
                    },
                }],
            })
            .unwrap();
    }

    fn counter_ns(&self) -> Vec<u64> {
        let mut ns: Vec<u64> = rows_of(&self.store, &self.endpoint_id)
            .iter()
            .map(|r| {
                let v: serde_json::Value = serde_json::from_str(&r.attributes_json).unwrap();
                v["value"]["U64"].as_u64().unwrap()
            })
            .collect();
        ns.sort_unstable();
        ns
    }

    fn epochs(&self) -> BTreeSet<u64> {
        rows_of(&self.store, &self.endpoint_id)
            .iter()
            .map(|r| r.stream_epoch)
            .collect()
    }
}

/// Hub 见证者：start 前订阅 + 专职排空（体量超 Hub 容量时结尾排空必 Lagged，
/// 故全程跟随；2s 静默窗口后 abort——此时已无新发布，abort 间隙无在途项）。
struct HubWitness {
    collected: std::sync::Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl HubWitness {
    fn subscribe(services: &std::sync::Arc<EventServices>) -> Self {
        let mut rx = services.hub.subscribe();
        let collected = std::sync::Arc::new(Mutex::new(Vec::new()));
        let dst = std::sync::Arc::clone(&collected);
        let task = tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                dst.lock().unwrap().push(ev.event_id.clone());
            }
        });
        Self { collected, task }
    }

    async fn finish(self) -> BTreeSet<String> {
        self.task.abort();
        self.collected.lock().unwrap().iter().cloned().collect()
    }
}

/// 确定性 stall 恢复 Gate：20/s 下 commit 暂停 6s，Core 不得判死/重连
/// （单 epoch），行冻结可观测；放行后等 backlog 必空再 Stop，全精确。
/// 同时证明 control 面存活：stall 全程 snapshot 恒为 RUNNING（心跳未判死、
/// endpoint 未 Lost——判死/重连会留下新 epoch，单 epoch 断言即覆盖）。
/// 本测试只证明 A（stall→backlog→catch-up exact→正常 Stop），不测
/// Stop-under-heavy-backlog（后者另立精确 N 的 Gate，不在本测试顺带）。
#[tokio::test]
async fn event_stall_6s_recovers_exact() {
    common::init_log();
    let faults = std::sync::Arc::new(StoreFaults::new(0, false));
    let rig = StallRig::start("hd-stall-recover", faults.clone());
    let hub = HubWitness::subscribe(&rig.services);
    rig.start_counter(50);
    common::wait_until(30, || rig.counter_ns().len() >= 2).await;

    let gate = faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;
    let frozen = rig.counter_ns().len();

    // 6s stall：snapshot 必须全程 RUNNING（无 Lost/重连）。
    // interval 50ms 即发射上限 20/s（Skip 只会更少），hold 期积压天然远小于容量。
    let snapshot = rig.mgr.snapshot();
    let hold_start = std::time::Instant::now();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        rig.counter_ns().len(),
        frozen,
        "commit 暂停期间 DB 不得出新行",
    );
    assert!(
        matches!(snapshot.endpoint("hd-stall-recover"), Some(ref s) if s.state == "RUNNING"),
        "stall 期间 endpoint 必须保持 RUNNING（Core 存活，未判死重连）",
    );

    // hold 期最大发射数自标定（实测 hold 时长 / interval 上限 + 余量），
    // 不假设 runner 速度，只用 interval 的硬上限。
    let max_hold_emissions = hold_start.elapsed().as_millis() / 50 + 8;
    gate.release();
    // catch-up 必空信号（FIFO 保证）：提交按到达顺序落盘，hold 期发射至多
    // max_hold_emissions；当累计提交超过 frozen + 上限 + 20（新发射）时，
    // hold 期积压必然已全部落盘——backlog 已空，此时 Stop 的 final drain
    // 无重压可背（把 Stop-under-backlog 排除在本测试之外）。
    const CATCH_UP_MARGIN: u64 = 20;
    let drained_target = frozen as u64 + max_hold_emissions as u64 + CATCH_UP_MARGIN;
    common::wait_until(60, || rig.counter_ns().len() as u64 >= drained_target).await;
    rig.mgr.stop_endpoint(&rig.endpoint_id).await.unwrap();
    let ns = rig.counter_ns();
    let max = *ns.last().unwrap();
    assert_eq!(ns.len() as u64, max, "恢复后无丢失无重复");
    for w in ns.windows(2) {
        assert_eq!(w[1], w[0] + 1, "恢复后必须连续");
    }
    assert_eq!(rig.epochs().len(), 1, "stall 不得触发重连");
    {
        use std::sync::atomic::Ordering;
        let d = &rig.services.diagnostics;
        assert_eq!(
            d.ingress_persisted_events_total.load(Ordering::Relaxed),
            ns.len() as u64,
        );
        // stall 实证：commit 延迟最大值必须覆盖 6s 暂停（秒级余量）。
        assert!(
            d.ingress_commit_latency_max_ns.load(Ordering::Relaxed) >= 5_000_000_000,
            "commit 延迟 max 必须见证 stall，实际 {}ns",
            d.ingress_commit_latency_max_ns.load(Ordering::Relaxed),
        );
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let got = hub.finish().await;
    let want: BTreeSet<String> = rows_of(&rig.store, &rig.endpoint_id)
        .iter()
        .map(|r| r.event_id.clone())
        .collect();
    assert_eq!(got, want, "Hub 必须恰好收到 DB 全部行");
    let _ = std::fs::remove_file(&rig.db);
}
