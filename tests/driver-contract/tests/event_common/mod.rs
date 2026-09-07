//! Event Plane V1 contract harness（PR10）：source-neutral 测试基建。
//!
//! 设计：Store 层 gate（identity/collision/atomicity/sequence）直接用
//! record/batch 构造器 + `EventStore::commit_batch`，天然 source-neutral；
//! Runtime 层 gate（reconnect/epoch/stop/backpressure/fairness）经
//! [`EventTestSource`] trait 分别跑 `SimulatorEventSource` / `OpcUaEventSource`
//!（后者在 commit 3 落地）。测试里出现 `if opcua` 即异味——先检查它是不是
//! generic contract。
//!
//! 纪律：只测冻结语义，不碰 `event-store` / Core / Web / 驱动生产代码。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{EventBatch, EventCondition, EventRecord, Value};
use mesa_event_store::{CommitRequest, CommitResult, EventStore, StoredEvent};

// ---------------------------------------------------------------------------
// record 构造器：source 形状 + 定点变异（collision 矩阵用）
// ---------------------------------------------------------------------------

/// 最小合法记录（字段约束见 `EventRecord::validate`）。
pub fn base_record(event_id: &str) -> EventRecord {
    EventRecord {
        event_id: event_id.into(),
        category: "message".into(),
        kind: "test.ping".into(),
        source: "harness".into(),
        severity: 100,
        code: None,
        message: Some("ping".into()),
        message_locale: None,
        occurred_at_ns: None,
        condition: None,
        correlation_id: None,
        attributes: BTreeMap::new(),
    }
}

/// Simulator 形状：瞬时 counter.tick（`occurred_at=None`，禁止伪造设备时间）。
pub fn sim_record(event_id: &str, n: u64) -> EventRecord {
    let mut r = base_record(event_id);
    r.category = "message".into();
    r.kind = "counter.tick".into();
    r.source = "sim".into();
    r.attributes.insert("value".into(), Value::U64(n));
    r
}

/// OPC UA 形状：condition occurrence（TransitionTime 纯函数语义由驱动保证，
/// 此处只定形状）。
pub fn opcua_record(event_id: &str, condition_id: &str) -> EventRecord {
    let mut r = base_record(event_id);
    r.category = "alarm".into();
    r.kind = "opcua.condition".into();
    r.source = "opcua".into();
    r.severity = 800;
    r.message = Some("condition".into());
    r.occurred_at_ns = Some(1_700_000_000_000_000_000);
    r.condition = Some(EventCondition {
        condition_id: condition_id.into(),
        transition: mesa_core_types::ConditionTransition::Raised,
        active: Some(true),
        acknowledged: Some(false),
        confirmed: None,
        retain: Some(true),
    });
    r
}

pub fn with_message(mut r: EventRecord, m: &str) -> EventRecord {
    r.message = Some(m.into());
    r
}

pub fn with_severity(mut r: EventRecord, s: u16) -> EventRecord {
    r.severity = s;
    r
}

pub fn with_occurred(mut r: EventRecord, ns: i64) -> EventRecord {
    r.occurred_at_ns = Some(ns);
    r
}

pub fn with_attr(mut r: EventRecord, k: &str, v: Value) -> EventRecord {
    r.attributes.insert(k.into(), v);
    r
}

pub fn with_transition(mut r: EventRecord, t: mesa_core_types::ConditionTransition) -> EventRecord {
    if let Some(c) = r.condition.as_mut() {
        c.transition = t;
    }
    r
}

/// batch 构造器：transport 元数据（handle/epoch/sequence）由调用方指定，
/// record 本体不动——metadata 变化不得改变 payload hash（commit 2 锁死）。
pub fn event_batch(
    connection_handle: u32,
    stream_epoch: u64,
    sequence: u64,
    events: Vec<EventRecord>,
) -> EventBatch {
    EventBatch {
        connection_handle,
        stream_epoch,
        sequence,
        timestamp_ns: 1_700_000_000_000_000_000,
        events,
        mono_ns: None,
    }
}

// ---------------------------------------------------------------------------
// store 直写 helpers（绕过 driver/manager，直达唯一写 API）
// ---------------------------------------------------------------------------

pub fn tmp_db(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "mesa-event-harden-{}-{}-{tag}.db",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    p
}

pub fn open_store(path: &std::path::Path) -> Arc<EventStore> {
    Arc::new(EventStore::open(path).unwrap())
}

pub async fn commit(
    store: &EventStore,
    endpoint_id: &str,
    batch: EventBatch,
) -> Result<CommitResult, mesa_event_store::EventStoreError> {
    store
        .commit_batch(CommitRequest {
            endpoint_id: endpoint_id.into(),
            batch,
            received_at_ns: 1_700_000_000_000_000_001,
        })
        .await
}

pub fn rows_of(store: &EventStore, endpoint_id: &str) -> Vec<StoredEvent> {
    store
        .query_history(&mesa_event_store::EventFilter {
            endpoint_id: Some(endpoint_id.into()),
            ..Default::default()
        })
        .unwrap()
        .0
}

pub async fn wait_rows(
    store: &EventStore,
    endpoint_id: &str,
    n: usize,
    timeout: Duration,
) -> Vec<StoredEvent> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let rows = rows_of(store, endpoint_id);
        if rows.len() >= n {
            return rows;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "{timeout:?} 内 {endpoint_id} 行数不足：want>={n} got={}",
                rows.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Runtime gate trait：两个独立 Event source 跑同一份契约
// ---------------------------------------------------------------------------

/// source-neutral 运行时契约。`emit_round` 语义：触发一次发射轮次并返回
/// 本轮 occurrence 的 event_id 集合（顺序无关；重复调用返回新轮次）。
/// disconnect/reconnect 模拟传输入断（kill/恢复），endpoint 配置保留，
/// reconnect 后必须是新 epoch。
#[async_trait::async_trait]
pub trait EventTestSource {
    async fn start(endpoint_id: &str) -> Self;
    async fn emit_round(&mut self) -> Vec<String>;
    async fn disconnect(&mut self);
    async fn reconnect(&mut self);
    async fn stop(self);
    fn store(&self) -> Arc<EventStore>;
    fn endpoint_id(&self) -> &str;
}

/// Simulator 源：manager + 真 simulator 子进程（`mesa.events.v1` alarm 流）。
/// 发射是驱动自主的（每 Start 跑一遍四态），`emit_round` = 起 endpoint →
/// 等 4 行 → 记 ids（调用方负责 stop/reconnect 前调 `stop_endpoint` 配对）。
pub struct SimulatorEventSource {
    endpoint_id: String,
    mgr: Arc<mesa_driver_manager::MesaManager>,
    store: Arc<EventStore>,
    db: std::path::PathBuf,
}

fn repo_drivers_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("drivers")
}

fn sim_alarm_task() -> mesa_core_types::EventTask {
    use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_ALARM.into(),
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

#[async_trait::async_trait]
impl EventTestSource for SimulatorEventSource {
    async fn start(endpoint_id: &str) -> Self {
        let db = tmp_db("sim");
        let _ = std::fs::remove_file(&db);
        let store = open_store(&db);
        let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(
            &repo_drivers_dir(),
        ));
        mgr.set_event_services(mesa_event_store::EventServices::new(
            store.clone(),
            mesa_event_store::EventHub::new(mesa_event_store::EVENT_HUB_CAPACITY),
        ));
        Self {
            endpoint_id: endpoint_id.into(),
            mgr,
            store,
            db,
        }
    }

    async fn emit_round(&mut self) -> Vec<String> {
        use mesa_driver_manager::endpoint::BuiltinEndpoint;
        self.mgr
            .start_endpoint(BuiltinEndpoint {
                endpoint_id: self.endpoint_id.clone(),
                driver_id: "simulator".into(),
                connection_json: "{}".into(),
                tasks: vec![],
                event_tasks: vec![sim_alarm_task()],
            })
            .unwrap();
        // alarm-cycle 每轮恰 4 条（Raised/Updated/Acknowledged/Cleared）。
        let rows = wait_rows(&self.store, &self.endpoint_id, 4, Duration::from_secs(30)).await;
        rows.iter().take(4).map(|r| r.event_id.clone()).collect()
    }

    async fn disconnect(&mut self) {
        assert_eq!(self.mgr.stop_endpoint(&self.endpoint_id).await, Ok(true));
    }

    async fn reconnect(&mut self) {
        // 新 Start = 新 epoch（SDK 序号器重建）；旧行保留，去重跨 epoch 有效。
        let _ = self.emit_round().await;
    }

    async fn stop(self) {
        let _ = self.mgr.stop_endpoint(&self.endpoint_id).await;
        let _ = std::fs::remove_file(&self.db);
    }

    fn store(&self) -> Arc<EventStore> {
        self.store.clone()
    }

    fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }
}
