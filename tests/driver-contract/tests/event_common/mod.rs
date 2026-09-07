//! Event Plane V1 contract harness（PR10）：source-neutral 测试基建。
//!
//! 多测试 target 共享：单个 target 只用子集是常态（与 `common/mod.rs` 同例）。
#![allow(dead_code)]
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
    // 真分页：production 按 HISTORY_LIMIT_MAX=500 clamp，limit 再大也取不全；
    // 用 before_seq 翻页读完（soak/pressure 上千行必需）。取值 500 与生产
    // 常量一致（分页循环正确性不依赖该值，取满即翻页）。
    let mut all = vec![];
    let mut cursor: Option<i64> = None;
    loop {
        let (page, next) = store
            .query_history(&mesa_event_store::EventFilter {
                endpoint_id: Some(endpoint_id.into()),
                before_seq: cursor,
                limit: Some(500),
                ..Default::default()
            })
            .unwrap();
        let done = next.is_none();
        all.extend(page);
        cursor = next;
        if done {
            return all;
        }
    }
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

/// source-neutral 运行时契约。生命周期显式三段（无隐藏副作用）：
/// `start` 只建 manager/store（不起 endpoint）；`start_endpoint` 起 endpoint
///（新 Start = 新 epoch）；`emit_round(round)` 要求 endpoint 运行中，
/// 返回本轮发射的 event_id（含故意重放）；`stop_endpoint` 停 endpoint；
/// `stop` 收尾清理。kill 级 Lost 由各源已有专属 Gate 覆盖，此处不断言。
///
/// `?Send`：OPC UA fixture 的 start future 非 Send（server builder），
/// contract 测试跑 `current_thread`，与现有 opcua e2e 一致。
#[async_trait::async_trait(?Send)]
pub trait EventTestSource {
    async fn start(endpoint_id: &str) -> Self;
    async fn start_endpoint(&mut self);
    async fn emit_round(&mut self, round: u32) -> Vec<String>;
    async fn stop_endpoint(&mut self);
    async fn stop(self);
    fn store(&self) -> Arc<EventStore>;
    fn endpoint_id(&self) -> &str;
    /// ingress 诊断快照（persisted events, UNIQUE 层 event duplicates）：
    /// replay 可观测性的唯一真相来源（DB exact-set 区分不了"收到并去重"
    /// 与"中途丢失"，计数器可以）。
    fn diagnostics(&self) -> (u64, u64);
}

/// 等待诊断增量达标：persisted 精确（多一行都是 exact-set 联动失败），
/// duplicates 为下界（trigger 轮询的天然冗余会贡献额外去重；下界恰好是
/// 盲区所需的方向：replay 若在传输中全丢，增量必低于故意重放数）。
pub async fn wait_diagnostics(
    src: &impl EventTestSource,
    base: (u64, u64),
    want_persisted_exact: u64,
    want_duplicates_min: u64,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let (p, d) = src.diagnostics();
        if p - base.0 >= want_persisted_exact && d - base.1 >= want_duplicates_min {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "诊断增量未达标：want +{want_persisted_exact}/≥+{want_duplicates_min}，got +{}/+{}",
            p - base.0,
            d - base.1,
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Simulator 源：manager + 真 simulator 子进程（`mesa.events.v1` alarm 流）。
/// 发射是驱动自主的（每 Start 跑一遍四态）；`emit_round` 要求已 start。
pub struct SimulatorEventSource {
    endpoint_id: String,
    mgr: Arc<mesa_driver_manager::MesaManager>,
    store: Arc<EventStore>,
    services: Arc<mesa_event_store::EventServices>,
    db: std::path::PathBuf,
    running: bool,
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

#[async_trait::async_trait(?Send)]
impl EventTestSource for SimulatorEventSource {
    async fn start(endpoint_id: &str) -> Self {
        let db = tmp_db("sim");
        let _ = std::fs::remove_file(&db);
        let store = open_store(&db);
        let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(
            &repo_drivers_dir(),
        ));
        let services = mesa_event_store::EventServices::new(
            store.clone(),
            mesa_event_store::EventHub::new(mesa_event_store::EVENT_HUB_CAPACITY),
        );
        mgr.set_event_services(std::sync::Arc::clone(&services));
        Self {
            endpoint_id: endpoint_id.into(),
            mgr,
            store,
            services,
            db,
            running: false,
        }
    }

    async fn start_endpoint(&mut self) {
        use mesa_driver_manager::endpoint::BuiltinEndpoint;
        assert!(!self.running, "endpoint 已在运行，显式 stop 后再起");
        self.mgr
            .start_endpoint(BuiltinEndpoint {
                endpoint_id: self.endpoint_id.clone(),
                driver_id: "simulator".into(),
                connection_json: "{}".into(),
                tasks: vec![],
                event_tasks: vec![sim_alarm_task()],
            })
            .unwrap();
        self.running = true;
    }

    async fn emit_round(&mut self, _round: u32) -> Vec<String> {
        // round 被忽略：Simulator 每次 Start 都是新 epoch + 新 ids（epoch 作用域）。
        assert!(self.running, "先 start_endpoint 再 emit（无隐藏启动）");
        let before = rows_of(&self.store, &self.endpoint_id).len();
        // alarm-cycle 每轮恰 4 条（Raised/Updated/Acknowledged/Cleared）。
        let rows = wait_rows(
            &self.store,
            &self.endpoint_id,
            before + 4,
            Duration::from_secs(30),
        )
        .await;
        // rows_of 按 seq DESC：take(4) 即本轮新增。
        rows.iter().take(4).map(|r| r.event_id.clone()).collect()
    }

    async fn stop_endpoint(&mut self) {
        if self.running {
            assert_eq!(self.mgr.stop_endpoint(&self.endpoint_id).await, Ok(true));
            self.running = false;
        }
    }

    async fn stop(mut self) {
        self.stop_endpoint().await;
        let _ = std::fs::remove_file(&self.db);
    }

    fn store(&self) -> Arc<EventStore> {
        self.store.clone()
    }

    fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }

    fn diagnostics(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering;
        let d = &self.services.diagnostics;
        (
            d.ingress_persisted_events_total.load(Ordering::Relaxed),
            d.ingress_event_duplicates_total.load(Ordering::Relaxed),
        )
    }
}

// ---------------------------------------------------------------------------
// OPC UA 源：manager + 真 opcua 驱动子进程 + 进程内 fixture 服务器
// ---------------------------------------------------------------------------

#[allow(dead_code)] // 各测试目标取用子集，未用 helper 属正常
#[path = "../../../../drivers/opcua/tests/support/event_server.rs"]
mod event_server;

/// round → 触发集（固定 E-id；含故意重放。id 形如 `opcua:4Q`，
/// 单字节 EventId 的 base64url，由到达断言自验证）。
fn opcua_round(round: u32) -> Vec<&'static str> {
    match round % 3 {
        0 => vec!["opcua:4Q", "opcua:4g"],
        1 => vec!["opcua:4g", "opcua:4w"],
        _ => vec!["opcua:4Q", "opcua:4g", "opcua:4w"],
    }
}

fn opcua_event_task() -> mesa_core_types::EventTask {
    use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
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

pub struct OpcUaEventSource {
    endpoint_id: String,
    mgr: Arc<mesa_driver_manager::MesaManager>,
    store: Arc<EventStore>,
    services: Arc<mesa_event_store::EventServices>,
    db: std::path::PathBuf,
    srv: Option<event_server::FixtureEventServer>,
    running: bool,
}

#[async_trait::async_trait(?Send)]
impl EventTestSource for OpcUaEventSource {
    async fn start(endpoint_id: &str) -> Self {
        ensure_pki_dir();
        let db = tmp_db("opcua");
        let _ = std::fs::remove_file(&db);
        let store = open_store(&db);
        let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(
            &repo_drivers_dir(),
        ));
        let services = mesa_event_store::EventServices::new(
            store.clone(),
            mesa_event_store::EventHub::new(mesa_event_store::EVENT_HUB_CAPACITY),
        );
        mgr.set_event_services(std::sync::Arc::clone(&services));
        Self {
            endpoint_id: endpoint_id.into(),
            mgr,
            store,
            services,
            db,
            srv: None,
            running: false,
        }
    }

    async fn start_endpoint(&mut self) {
        use mesa_driver_manager::endpoint::BuiltinEndpoint;
        assert!(!self.running, "endpoint 已在运行，显式 stop 后再起");
        if self.srv.is_none() {
            self.srv = Some(event_server::FixtureEventServer::start().await);
        }
        let url = self.srv.as_ref().unwrap().endpoint_url();
        self.mgr
            .start_endpoint(BuiltinEndpoint {
                endpoint_id: self.endpoint_id.clone(),
                driver_id: "opcua".into(),
                connection_json: format!(r#"{{"endpoint_url":"{url}","timeout_ms":5000}}"#),
                tasks: vec![],
                event_tasks: vec![opcua_event_task()],
            })
            .unwrap();
        self.running = true;
    }

    async fn emit_round(&mut self, round: u32) -> Vec<String> {
        assert!(self.running, "先 start_endpoint 再 emit（无隐藏启动）");
        let plan = opcua_round(round);
        let want: Vec<String> = plan.iter().map(|id| id.to_string()).collect();
        // 每 burst 全量 trigger 一次；每 burst 必须被诊断吸收（persisted +
        // duplicates 共 +want.len())——否则 trigger 还在路上、presence 检查
        // 会被旧行"满足"，全重放轮直接返回，replay 真丢了也看不见（review P1
        // 盲区）。吸收等不到即大声失败，不静默。
        let total = || {
            let (p, d) = self.diagnostics();
            p + d
        };
        let base = total();
        for burst in 0..3u64 {
            // burst 内重打直到吸收：订阅就绪前的首波 trigger 合法丢失
            //（fixture 无监控项时 notify 即弃），停打即误判。
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            loop {
                {
                    let srv = self.srv.as_ref().unwrap();
                    for id in &want {
                        match id.as_str() {
                            "opcua:4Q" => srv.trigger(&event_server::e1_base()),
                            "opcua:4g" => srv.trigger(&event_server::e2_raised()),
                            "opcua:4w" => srv.trigger(&event_server::e3_updated()),
                            _ => unreachable!(),
                        }
                    }
                }
                if total() >= base + want.len() as u64 * (burst + 1) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "burst {burst} 触发未被吸收（trigger 可能丢失）"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        // 新 id 到达确认（全重放轮里此检查恒真，真正的证据是上面的吸收计数）。
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let rows = rows_of(&self.store, &self.endpoint_id);
            if want.iter().all(|id| rows.iter().any(|r| &r.event_id == id)) {
                return want;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "60s 内本轮 ids 未齐：want={want:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn stop_endpoint(&mut self) {
        // manager 级 stop（graceful，快）。kill 级 Lost 由 PR9 专属 Gate 覆盖。
        if self.running {
            assert_eq!(self.mgr.stop_endpoint(&self.endpoint_id).await, Ok(true));
            self.running = false;
        }
    }

    async fn stop(mut self) {
        self.stop_endpoint().await;
        if let Some(mut srv) = self.srv.take() {
            srv.stop().await;
        }
        let _ = std::fs::remove_file(&self.db);
    }

    fn store(&self) -> Arc<EventStore> {
        self.store.clone()
    }

    fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }

    fn diagnostics(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering;
        let d = &self.services.diagnostics;
        (
            d.ingress_persisted_events_total.load(Ordering::Relaxed),
            d.ingress_event_duplicates_total.load(Ordering::Relaxed),
        )
    }
}
