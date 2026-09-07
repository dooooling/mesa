//! Backpressure / fairness pressure gates（PR10 commit 4 §5/§6）：
//! 生产路径（真 simulator 子进程 → manager → ingress → DB）证明 Event Plane
//! 无 silent drop：burst 全落盘保序；数据负载下事件不饿死；事件 burst 下
//! endpoint 健康。SDK 满等待语义另由 driver-sdk in-crate 测试锁定。

mod common;
mod event_common;

use std::time::Duration;

use event_common::{rows_of, tmp_db};
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
