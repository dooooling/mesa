//! Mesa-owned deterministic OPC UA Event fixture（PR9 Stage ⑥）。
//!
//! 基于 `async-opcua-server 0.19` 的 loopback 软件服务器（与 client/types 同版本）。
//! 核心纪律：事件只在 client 明确确认监控项已创建（`fixture_ready` barrier
//! 之后）由测试显式 `trigger`；禁止"启动后 sleep 猜测再发事件"的竞态写法。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use opcua_server::{ServerBuilder, ServerHandle};

static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 运行中的 fixture 服务器：handle（trigger 事件）+ 端口（client 连接）。
pub struct FixtureEventServer {
    pub handle: ServerHandle,
    pub port: u16,
    pub pki_dir: PathBuf,
    server_task: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

impl FixtureEventServer {
    /// 启动服务器并等待 TCP 就绪（listener 由调用方预绑定，端口确定性已知）。
    pub async fn start() -> Self {
        let n = FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst);
        let pki_dir = std::env::temp_dir().join(format!(
            "mesa-opcua-event-fixture-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&pki_dir).expect("fixture pki 目录必须可建");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture 端口必须可绑");
        let port = listener.local_addr().expect("端口可读").port();
        let mut builder = ServerBuilder::new_anonymous("MesaEventFixture")
            .host("127.0.0.1")
            .port(port)
            .pki_dir(pki_dir.clone())
            .create_sample_keypair(true);
        // fixture 队列上限：缺省值是测试型小数字（item 10 / subscription 20），
        // burst 会被服务端静默丢弃；提到与生产请求（1000）匹配的量级。
        // 注意：这是 fixture 侧的容量声明，不是被测行为。
        builder
            .limits_mut()
            .subscriptions
            .max_monitored_item_queue_size = 2000;
        builder.limits_mut().subscriptions.max_queued_notifications = 2000;
        let (server, handle) = builder.build().expect("fixture 服务器必须可建");
        patch_condition_self_path(&handle);
        let server_task = Some(tokio::spawn(async move { server.run_with(listener).await }));
        // 就绪定义：TCP 可建连（run_with 内 node manager 初始化完成后 accept）。
        // connect 失败即重试至 10s 上限，超时则失败而非静默通过。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                Ok(_) => break,
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::task::yield_now().await;
                }
                Err(e) => panic!("fixture 服务器 10s 内未就绪: {e:?}"),
            }
        }
        Self {
            handle,
            port,
            pki_dir,
            server_task,
        }
    }

    pub fn endpoint_url(&self) -> String {
        format!("opc.tcp://127.0.0.1:{}", self.port)
    }

    /// 停止服务器并回收任务 + PKI 临时目录（`&mut`：kill 后 harness 仍可
    /// join run 句柄做断言，见 session-loss Gate）。
    pub async fn stop(&mut self) {
        self.handle.cancel();
        if let Some(t) = self.server_task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), t).await;
        }
        let _ = std::fs::remove_dir_all(&self.pki_dir);
    }

    /// 杀服务器（abort 任务：监听 + 已建连 socket 立刻释放，模拟掉电/进程消失）。
    /// 优雅 `stop()` 要求驱动先断开（否则 server accept 循环不退出）；真断线
    /// Gate 恰恰需要"驱动还连着时服务器消失"，故用 abort。
    pub async fn kill(&mut self) {
        if let Some(t) = self.server_task.take() {
            t.abort();
            let _ = t.await;
        }
        let _ = std::fs::remove_dir_all(&self.pki_dir);
        // 确认端口已释放（connect 被拒），再返回——否则 client 的重连计时
        // 起点不确定。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::net::TcpStream::connect(("127.0.0.1", self.port))
            .await
            .is_ok()
        {
            if std::time::Instant::now() >= deadline {
                panic!("kill 后 10s 端口仍可建连");
            }
            tokio::task::yield_now().await;
        }
    }

    /// 显式 trigger 一个事件（调用方保证 monitored item 已 Good 的 barrier 之后；
    /// drop 即提交给订阅）。
    pub fn trigger(&self, event: &dyn opcua_nodes::Event) {
        trigger_event(&self.handle, event)
    }
}

/// 无 barrier 约束的 trigger（`'static` 友好，供 probe 循环持有 handle 调用）。
pub fn trigger_event(handle: &opcua_server::ServerHandle, event: &dyn opcua_nodes::Event) {
    let server_node = opcua_types::NodeId::new(0, 2253u32);
    let mut notifier = handle.subscriptions().event_notifier();
    notifier.notify(&server_node, event);
}

// ---------------------------------------------------------------------------
// §23 V1 事件集：固定 EventId / 时间 / 状态（绝不用 now()，可重复精确断言）
// ---------------------------------------------------------------------------

/// fixture 侧补偿：async-opcua 0.19 类型树没有 ConditionType 的空路径（self）
/// 注册，导致标准 Table 10 clause（ConditionType，[]，NodeId）在 validation
/// 即判 BadNodeIdUnknown。注意方向——这是在补 server 对标准 clause 的接受能力，
/// 不是在给非标准 clause 开绿灯（旧 patch 恰恰反了，已删）。
/// 注册空路径 → Object 类：validation 的 attribute 检查本就规定
/// “Object 节点取 NodeId 属性”（instance 自身即 object），求值侧见 lookup。
/// 上游类型树补齐 self 路径后删除本函数。
fn patch_condition_self_path(handle: &opcua_server::ServerHandle) {
    use opcua_types::{NodeClass, NodeId, ObjectTypeId};
    let prop_id = NodeId::new(1, opcua_types::UAString::from("FixtureConditionSelf"));
    let cond_type = NodeId::new(0, ObjectTypeId::ConditionType as u32);
    handle
        .type_tree()
        .write()
        .add_type_property(&prop_id, &cond_type, &[], NodeClass::Object);
}

/// Unix ns → OPC UA ticks（与生产换算互逆）。
pub fn ns_to_ticks(ns: i64) -> i64 {
    ns / 100 + 11644473600 * 10_000_000
}

pub fn ns_to_datetime(ns: i64) -> opcua_types::DateTime {
    opcua_types::DateTime::from(ns_to_ticks(ns))
}

/// 固定时间基 T0..T6（10s 间隔）。
pub fn t(n: u8) -> i64 {
    1_700_000_000_000_000_000 + i64::from(n) * 10_000_000_000
}

/// 手工 Event 实现（不用 derive）：按路径精确应答 19 标准字段，
/// `event_type` 取标准 AlarmConditionType（OfType 子类型判定可过）。
pub struct FixtureConditionEvent {
    pub base: opcua_nodes::BaseEventType,
    pub event_type: opcua_types::NodeId,
    pub condition_id: opcua_types::NodeId,
    pub condition_name: opcua_types::UAString,
    pub branch_id: Option<opcua_types::NodeId>,
    pub retain: bool,
    pub enabled: bool,
    pub active: Option<bool>,
    pub active_tt: Option<opcua_types::DateTime>,
    pub acked: Option<bool>,
    pub acked_tt: Option<opcua_types::DateTime>,
    pub confirmed: Option<bool>,
    pub confirmed_tt: Option<opcua_types::DateTime>,
}

impl FixtureConditionEvent {
    /// 按路径精确应答 19 标准字段（fixture 简化：类型 id 不再二次判定，
    /// 类型过滤由 OfType where 子句独立证明）。
    fn lookup(
        &self,
        attribute_id: opcua_types::AttributeId,
        index_range: &opcua_types::NumericRange,
        browse_path: &[opcua_types::QualifiedName],
    ) -> opcua_types::Variant {
        use opcua_types::Variant;
        use opcua_types::event_field::EventField as _;
        // Part 9 Table 10：ConditionId 即 Condition instance 自身的 NodeId，
        // clause 为（ConditionType，空路径，NodeId 属性）。["ConditionId"]
        // 伪字段故意不回答（非标准，合规 server 无此组件）。
        if browse_path.is_empty() {
            if attribute_id == opcua_types::AttributeId::NodeId {
                return self.condition_id.clone().into();
            }
            return Variant::Empty;
        }
        // 其余字段只接受 Value 属性（与服务端校验一致）。
        if attribute_id != opcua_types::AttributeId::Value {
            return Variant::Empty;
        }
        let seg: Vec<&str> = browse_path.iter().map(|q| q.name.as_ref()).collect();
        match seg.as_slice() {
            [single]
                if [
                    "EventId",
                    "EventType",
                    "SourceNode",
                    "SourceName",
                    "Time",
                    "ReceiveTime",
                    "Message",
                    "Severity",
                ]
                .contains(single) =>
            {
                self.base.get_value(attribute_id, index_range, browse_path)
            }
            ["ConditionName"] => self.condition_name.clone().into(),
            ["BranchId"] => self
                .branch_id
                .clone()
                .map(Variant::from)
                .unwrap_or(Variant::Empty),
            ["Retain"] => self.retain.into(),
            ["EnabledState", "Id"] => self.enabled.into(),
            ["ActiveState", "Id"] => self.active.map(Variant::from).unwrap_or(Variant::Empty),
            ["ActiveState", "TransitionTime"] => {
                self.active_tt.map(Variant::from).unwrap_or(Variant::Empty)
            }
            ["AckedState", "Id"] => self.acked.map(Variant::from).unwrap_or(Variant::Empty),
            ["AckedState", "TransitionTime"] => {
                self.acked_tt.map(Variant::from).unwrap_or(Variant::Empty)
            }
            ["ConfirmedState", "Id"] => self.confirmed.map(Variant::from).unwrap_or(Variant::Empty),
            ["ConfirmedState", "TransitionTime"] => self
                .confirmed_tt
                .map(Variant::from)
                .unwrap_or(Variant::Empty),
            _ => Variant::Empty,
        }
    }
}

impl opcua_types::event_field::EventField for FixtureConditionEvent {
    fn get_value(
        &self,
        attribute_id: opcua_types::AttributeId,
        index_range: &opcua_types::NumericRange,
        remaining_path: &[opcua_types::QualifiedName],
    ) -> opcua_types::Variant {
        self.lookup(attribute_id, index_range, remaining_path)
    }
}

impl opcua_nodes::Event for FixtureConditionEvent {
    fn get_field(
        &self,
        _type_definition_id: &opcua_types::NodeId,
        attribute_id: opcua_types::AttributeId,
        index_range: &opcua_types::NumericRange,
        browse_path: &[opcua_types::QualifiedName],
    ) -> opcua_types::Variant {
        self.lookup(attribute_id, index_range, browse_path)
    }

    fn time(&self) -> &opcua_types::DateTime {
        &self.base.time
    }

    fn event_type_id(&self) -> &opcua_types::NodeId {
        &self.event_type
    }
}

fn alarm_type() -> opcua_types::NodeId {
    opcua_types::NodeId::new(0, opcua_types::ObjectTypeId::AlarmConditionType as u32)
}

fn base_event(
    id_byte: u8,
    message: &str,
    severity: u16,
    time_ns: i64,
) -> opcua_nodes::BaseEventType {
    opcua_nodes::BaseEventType::new(
        opcua_types::NodeId::new(0, opcua_types::ObjectTypeId::BaseEventType as u32),
        opcua_types::ByteString::from(vec![id_byte]),
        opcua_types::LocalizedText::new("", message),
        ns_to_datetime(time_ns),
    )
    .set_source_name(opcua_types::UAString::from("MesaFixture"))
    .set_severity(severity)
}

/// E1：普通 BaseEvent（E1 专属：Time=T1）。
pub fn e1_base() -> opcua_nodes::BaseEventType {
    base_event(0xE1, "fixture event", 321, t(1))
}

// fixture 脚手架：9 参数仅为事件集行文方便，允许。
#[allow(clippy::too_many_arguments)]
fn condition_event(
    id_byte: u8,
    time_ns: i64,
    active: Option<bool>,
    active_tt: Option<i64>,
    acked: Option<bool>,
    acked_tt: Option<i64>,
    confirmed: Option<bool>,
    confirmed_tt: Option<i64>,
    source_name: &str,
) -> FixtureConditionEvent {
    let mut base = base_event(id_byte, "fixture condition", 500, time_ns);
    base.source_name = opcua_types::UAString::from(source_name);
    FixtureConditionEvent {
        base,
        event_type: alarm_type(),
        condition_id: opcua_types::NodeId::new(1, opcua_types::UAString::from("Alarm1")),
        condition_name: opcua_types::UAString::from("Alarm1"),
        branch_id: None,
        retain: true,
        enabled: true,
        active,
        active_tt: active_tt.map(ns_to_datetime),
        acked,
        acked_tt: acked_tt.map(ns_to_datetime),
        confirmed,
        confirmed_tt: confirmed_tt.map(ns_to_datetime),
    }
}

/// E0 probe（sacrificial：先被观测到即 barrier，后续断言跳过它）。
pub fn e0_probe() -> FixtureConditionEvent {
    condition_event(
        0xE0,
        t(0),
        Some(true),
        Some(t(0)),
        None,
        None,
        None,
        None,
        "MesaFixture",
    )
}

/// E2 Raised：Active=true 且 ActiveTransitionTime == Time。
pub fn e2_raised() -> FixtureConditionEvent {
    condition_event(
        0xE2,
        t(2),
        Some(true),
        Some(t(2)),
        None,
        None,
        None,
        None,
        "MesaFixture",
    )
}

/// E3 Updated：ActiveTransitionTime 停留 T2，Time 已到 T3。
pub fn e3_updated() -> FixtureConditionEvent {
    condition_event(
        0xE3,
        t(3),
        Some(true),
        Some(t(2)),
        None,
        None,
        None,
        None,
        "MesaFixture",
    )
}

/// E4 Acknowledged。
pub fn e4_acknowledged() -> FixtureConditionEvent {
    condition_event(
        0xE4,
        t(4),
        Some(true),
        Some(t(2)),
        Some(true),
        Some(t(4)),
        None,
        None,
        "MesaFixture",
    )
}

/// E5 Cleared（附带 source 回退链全覆盖：SourceName 空 + SourceNode null →
/// 期望 notifier canonical）。
pub fn e5_cleared() -> FixtureConditionEvent {
    let mut e = condition_event(
        0xE5,
        t(5),
        Some(false),
        Some(t(5)),
        None,
        None,
        None,
        None,
        "",
    );
    e.base.source_node = opcua_types::NodeId::null();
    e
}

/// E6 Confirmed。
pub fn e6_confirmed() -> FixtureConditionEvent {
    condition_event(
        0xE6,
        t(6),
        Some(true),
        Some(t(2)),
        None,
        None,
        Some(true),
        Some(t(6)),
        "MesaFixture",
    )
}
