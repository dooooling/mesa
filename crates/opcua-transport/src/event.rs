//! OPC UA Event 订阅契约（PR9 Stage ①）：只描述 OPC UA wire 语义。
//!
//! 与 DataChange 路径（Latest-Wins slot + `send().await`）严格平行、绝不复用：
//! Event callback 产出的是 ordered occurrence（`Vec<Variant>` 位置数组，顺序严格
//! 对应 select clauses），必须 FIFO + fail-closed，禁止 coalesce / drop-oldest。
//! 本模块只做"服务器给了什么→原样结构化交出去"；Mesa 映射（EventRecord /
//! Condition / transition 推断）由上层 Driver（`drivers/opcua/src/event.rs`）负责。

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use opcua_types::{
    AttributeId, ContentFilter, ContentFilterElement, EventFilter, EventFilterResult,
    ExtensionObject, FilterOperator, LiteralOperand, MonitoredItemCreateRequest, MonitoringMode,
    MonitoringParameters, NumericRange, QualifiedName, ReadValueId, SimpleAttributeOperand,
    StatusCode, Variant,
};

use crate::{UaNodeRef, UaOperation, UaSubscriptionId, UaTransportError};

// ---------------------------------------------------------------------------
// 过滤器规约：调用方（Driver）按标准字段表组装，transport 只做结构校验与编码
// ---------------------------------------------------------------------------

/// browse 路径单段：`namespace + name`（QualifiedName 的 transport 形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UaQualifiedNameRef {
    pub namespace: u16,
    pub name: String,
}

/// 单个 select clause：`type_definition_id + browse_path + attribute`。
/// 位置即契约——callback 返回的 fields 数组顺序严格对应本数组顺序（§7 golden test 锁定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UaEventSelectClause {
    pub type_definition_id: UaNodeRef,
    pub browse_path: Vec<UaQualifiedNameRef>,
    pub attribute_id: u32,
}

/// EventFilter 规约：select clauses 必填；`of_type` 为 Some 时自动追加
/// OfType where 子句（LiteralOperand 携带类型 NodeId），None 时 where 为空。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UaEventFilterSpec {
    pub select_clauses: Vec<UaEventSelectClause>,
    pub of_type: Option<UaNodeRef>,
}

/// 事件监控项规约：notifier 为事件源节点；`queue_size` 由调用方按冻结策略
/// （default 1000，1..=10000，`discard_oldest=false`）传入，本层不静默改写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UaEventMonitoredItemSpec {
    pub notifier: UaNodeRef,
    pub client_handle: u32,
    pub queue_size: u32,
    pub filter: UaEventFilterSpec,
}

/// 规约 → wire `EventFilter`：逐 clause 校验 attribute 合法性并保持顺序；
/// 空 clauses 直接 configuration 失败（发线前即可判定，不可重试）。
pub fn build_event_filter(spec: &UaEventFilterSpec) -> Result<EventFilter, UaTransportError> {
    if spec.select_clauses.is_empty() {
        return Err(UaTransportError::configuration(
            UaOperation::CreateEventMonitoredItems,
            "EventFilter select_clauses 不能为空",
        ));
    }
    let mut clauses = Vec::with_capacity(spec.select_clauses.len());
    for c in &spec.select_clauses {
        // attribute_id 必须是协议已知属性，否则发线后全是 Empty，提前失败。
        AttributeId::from_u32(c.attribute_id).map_err(|_| {
            UaTransportError::protocol(
                UaOperation::CreateEventMonitoredItems,
                format!("EventFilter attribute_id 非法: {}", c.attribute_id),
            )
        })?;
        clauses.push(SimpleAttributeOperand {
            type_definition_id: crate::native::to_opc_node_id(&c.type_definition_id)?,
            browse_path: Some(
                c.browse_path
                    .iter()
                    .map(|q| opcua_types::QualifiedName::new(q.namespace, q.name.as_str()))
                    .collect(),
            ),
            attribute_id: c.attribute_id,
            index_range: NumericRange::default(),
        });
    }
    let where_clause = match &spec.of_type {
        // NOTE: OfType 操作数是类型 NodeId 的字面量；单元素 where 子句即"仅该类型"。
        Some(t) => ContentFilter {
            elements: Some(vec![ContentFilterElement {
                filter_operator: FilterOperator::OfType,
                filter_operands: Some(vec![ExtensionObject::from_message(LiteralOperand {
                    value: Variant::from(crate::native::to_opc_node_id(t)?),
                })]),
            }]),
        },
        None => ContentFilter { elements: None },
    };
    Ok(EventFilter {
        select_clauses: Some(clauses),
        where_clause,
    })
}

/// 监控项请求构造：notifier 节点 + EventNotifier 属性 + EventFilter；
/// `sampling_interval` 对事件无意义，固定 0.0；`discard_oldest` 恒 false
///（§6：宁可 overflow fail-closed，不丢弃 ordered prefix）。
pub fn build_event_monitored_item_request(
    item: &UaEventMonitoredItemSpec,
) -> Result<MonitoredItemCreateRequest, UaTransportError> {
    let filter = build_event_filter(&item.filter)?;
    let params = MonitoringParameters {
        client_handle: item.client_handle,
        sampling_interval: 0.0,
        filter: ExtensionObject::from_message(filter),
        queue_size: item.queue_size,
        discard_oldest: false,
    };
    Ok(MonitoredItemCreateRequest::new(
        ReadValueId {
            node_id: crate::native::to_opc_node_id(&item.notifier)?,
            // NOTE: 事件监控项挂在 notifier 的 EventNotifier 属性上（Part 4 §5.12.2 约定）；
            // 若某 Server 拒绝，属 filter 被拒类失败，走正常回滚，不静默换属性重试。
            attribute_id: AttributeId::EventNotifier as u32,
            index_range: NumericRange::default(),
            data_encoding: QualifiedName::null(),
        },
        MonitoringMode::Reporting,
        params,
    ))
}

/// 服务端 `filter_result` 解码与校验：必须是 `EventFilterResult`，且
/// select clause 结果数严格等于请求数（数量不变式，§26）；逐 clause 的
/// Good 与否由调用方按策略判定（19 个必须全 Good），本函数只原样返回
/// 状态码数组。P0-3：where 部分不再忽略——`where_element` 原样返回，
/// 调用方按 scope 判定（all：无 BAD；conditions：恰一个 OfType 且 Good）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedEventFilterResult {
    /// 逐 select clause 状态码，顺序对应请求（数量已校验）。
    pub select: Vec<StatusCode>,
    /// 逐 where element 状态码（空 where 即空数组）。
    pub where_element: Vec<StatusCode>,
}

pub fn decode_event_filter_result(
    filter_result: &ExtensionObject,
    expected_clauses: usize,
) -> Result<DecodedEventFilterResult, UaTransportError> {
    let r = filter_result
        .inner_as::<EventFilterResult>()
        .ok_or_else(|| {
            UaTransportError::protocol(
                UaOperation::CreateEventMonitoredItems,
                "监控项 filter_result 非 EventFilterResult",
            )
        })?;
    let select = r.select_clause_results.clone().ok_or_else(|| {
        UaTransportError::protocol(
            UaOperation::CreateEventMonitoredItems,
            "EventFilterResult 缺少 select_clause_results",
        )
    })?;
    crate::native::check_cardinality(
        UaOperation::CreateEventMonitoredItems,
        "EventFilter select 结果",
        expected_clauses,
        select.len(),
    )?;
    let where_element = r
        .where_clause_result
        .element_results
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.status_code)
        .collect();
    Ok(DecodedEventFilterResult {
        select,
        where_element,
    })
}

// ---------------------------------------------------------------------------
// 订阅返回：receiver FIFO + fatal watch + 统计（与 UaSubscription 平行）
// ---------------------------------------------------------------------------

/// 回调桥不可恢复的完整性破坏：经 watch 单独传播，receiver 侧读到即 fail 当前 attempt。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UaEventStreamFatal {
    /// 本地 callback 队列 Full：已有 occurrence 可能丢失，RUNNING 已不可信。
    CallbackQueueOverflow,
    /// 通知形态非法（字段数与 clauses 对不上等）：解码契约已破坏。
    MalformedNotification,
}

impl UaEventStreamFatal {
    pub fn as_str(&self) -> &'static str {
        match self {
            UaEventStreamFatal::CallbackQueueOverflow => "CallbackQueueOverflow",
            UaEventStreamFatal::MalformedNotification => "MalformedNotification",
        }
    }
}

/// 回调队列容量（§5）：同步 callback 内只允许 `try_send`，Full 即 fatal。
pub const EVENT_CALLBACK_QUEUE_CAPACITY: usize = 1024;

/// Event 订阅统计：只计数，不解释（Latest-Wins 一类指标在此不存在）。
#[derive(Debug, Default)]
pub struct EventSubscriptionStats {
    pub events_received: AtomicU64,
    pub callback_queue_overflow: AtomicU64,
}

impl EventSubscriptionStats {
    pub fn events_received(&self) -> u64 {
        self.events_received.load(Ordering::Relaxed)
    }

    pub fn callback_queue_overflow(&self) -> u64 {
        self.callback_queue_overflow.load(Ordering::Relaxed)
    }
}

/// 单个原生事件通知：`client_handle` 路由，`fields` 顺序严格对应 select clauses。
#[derive(Debug, Clone)]
pub struct UaEventNotification {
    pub client_handle: u32,
    pub fields: Vec<Variant>,
}

/// Event 订阅句柄：与 [`crate::UaSubscription`] 平行（绝不复用其 Latest-Wins 通道）。
pub struct UaEventSubscription {
    pub id: UaSubscriptionId,
    pub requested_publishing_interval_ms: u64,
    pub revised_publishing_interval_ms: u64,
    pub revised_lifetime_count: u32,
    pub revised_max_keep_alive_count: u32,
    /// 有序 FIFO：生产者只 `try_send`，消费者（Driver runtime）负责排空。
    pub receiver: tokio::sync::mpsc::Receiver<UaEventNotification>,
    /// 完整性破坏信号：`Some` 即当前 attempt 已不可信，调用方必须 fail。
    pub fatal: tokio::sync::watch::Receiver<Option<UaEventStreamFatal>>,
    pub stats: Arc<EventSubscriptionStats>,
    pub(crate) producer: Arc<EventProducerShared>,
}

impl UaEventSubscription {
    /// 本地 producer-close barrier（P0-1）：关门 + 摘除发送端。此后任何
    /// callback 都无法再 publish；receiver 保持 OPEN，调用方 drain 到
    /// sender CLOSED（`recv() == None`）即证明"已接受的 occurrence 已全部
    /// 交出"。幂等，可重入。
    pub fn close_producer(&self) {
        self.producer.close();
    }
}

/// Event producer 共享态（callback 与 close 的互斥点）：门 + 发送端二合一，
/// 同一把小锁下判定，保证"关门后无新 send、先发的不丢"（drain 到 None 时
/// 通道内即全部已接受项）。同步 callback 内只做 lock + try_send，不 await。
#[derive(Debug, Default)]
pub(crate) struct EventProducerShared {
    open: AtomicBool,
    tx: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<UaEventNotification>>>,
}

/// `try_send` 结果（调用方映射到统计/fatal/静默丢弃）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProducerSend {
    /// 已入队。
    Sent,
    /// 队列满（调用方必须 fatal，绝不 drop-oldest）。
    Full,
    /// 门已关或发送端已摘除（Stop 后的 emission，静默丢弃即正确）。
    Closed,
}

impl EventProducerShared {
    pub(crate) fn new(tx: tokio::sync::mpsc::Sender<UaEventNotification>) -> Arc<Self> {
        Arc::new(Self {
            open: AtomicBool::new(true),
            tx: std::sync::Mutex::new(Some(tx)),
        })
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    pub(crate) fn try_send(&self, n: UaEventNotification) -> ProducerSend {
        if !self.open.load(Ordering::SeqCst) {
            return ProducerSend::Closed;
        }
        let guard = self.tx.lock().expect("producer 锁不中毒");
        match guard.as_ref() {
            None => ProducerSend::Closed,
            Some(tx) => match tx.try_send(n) {
                Ok(()) => ProducerSend::Sent,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => ProducerSend::Full,
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => ProducerSend::Closed,
            },
        }
    }

    pub(crate) fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
        // 摘除发送端：通道关闭（已入队项保留），drain 到 None 即可终止。
        self.tx.lock().expect("producer 锁不中毒").take();
    }
}

/// 事件监控项创建结果（逐项 status + revised queue + clause 级状态）。
#[derive(Debug, Clone)]
pub struct UaEventMonitoredItemResult {
    pub client_handle: u32,
    pub monitored_item_id: u32,
    pub status_code: u32,
    pub requested_queue_size: u32,
    pub revised_queue_size: u32,
    /// 逐 clause 状态码（bits），顺序对应 `filter.select_clauses`。
    pub select_clause_statuses: Vec<u32>,
    /// 逐 where element 状态码（bits）；空 where 即空数组（P0-3：不再忽略）。
    pub where_clause_statuses: Vec<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(type_id: u32, path: &[(&str, u16)], attr: u32) -> UaEventSelectClause {
        UaEventSelectClause {
            type_definition_id: UaNodeRef::numeric(0, type_id),
            browse_path: path
                .iter()
                .map(|(n, ns)| UaQualifiedNameRef {
                    namespace: *ns,
                    name: (*n).into(),
                })
                .collect(),
            attribute_id: attr,
        }
    }

    #[test]
    fn empty_clauses_rejected_before_wire() {
        let err = build_event_filter(&UaEventFilterSpec::default()).unwrap_err();
        assert_eq!(err.operation, UaOperation::CreateEventMonitoredItems);
        assert!(!err.retryable);
    }

    #[test]
    fn unknown_attribute_rejected_before_wire() {
        let spec = UaEventFilterSpec {
            select_clauses: vec![clause(2041, &[], 0xFFFF_FFFF)],
            of_type: None,
        };
        assert!(build_event_filter(&spec).is_err());
    }

    #[test]
    fn clause_order_and_shape_preserved() {
        // 位置即契约：callback fields[i] 必须对应 clauses[i]（§7 golden）。
        let spec = UaEventFilterSpec {
            select_clauses: vec![
                clause(2041, &[], 15), // EventId: Value
                clause(2041, &[("Message", 0)], 13),
                clause(2782, &[("ConditionId", 0)], 13),
            ],
            of_type: None,
        };
        let f = build_event_filter(&spec).unwrap();
        let clauses = f.select_clauses.unwrap();
        assert_eq!(clauses.len(), 3);
        assert_eq!(clauses[0].attribute_id, 15);
        assert!(clauses[0].browse_path.as_ref().unwrap().is_empty());
        assert_eq!(clauses[1].browse_path.as_ref().unwrap()[0].name, "Message");
        assert_eq!(
            clauses[2].type_definition_id,
            opcua_types::NodeId::new(0, 2782u32)
        );
        assert!(f.where_clause.elements.is_none());
    }

    #[test]
    fn of_type_builds_single_oftype_element() {
        let spec = UaEventFilterSpec {
            select_clauses: vec![clause(2041, &[], 15)],
            of_type: Some(UaNodeRef::numeric(0, 2915)),
        };
        let f = build_event_filter(&spec).unwrap();
        let elements = f.where_clause.elements.unwrap();
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].filter_operator, FilterOperator::OfType);
        let ops = elements[0].filter_operands.clone().unwrap();
        assert_eq!(ops.len(), 1);
        let lit = ops[0].inner_as::<LiteralOperand>().unwrap();
        assert_eq!(
            lit.value,
            Variant::from(opcua_types::NodeId::new(0, 2915u32))
        );
    }

    #[test]
    fn filter_result_validates_cardinality() {
        use opcua_types::{ContentFilterElementResult, ContentFilterResult, StatusCode};
        let good = ExtensionObject::from_message(EventFilterResult {
            select_clause_results: Some(vec![StatusCode::Good, StatusCode::Good]),
            select_clause_diagnostic_infos: None,
            where_clause_result: ContentFilterResult {
                element_results: Some(vec![ContentFilterElementResult {
                    status_code: StatusCode::Good,
                    operand_status_codes: None,
                    operand_diagnostic_infos: None,
                }]),
                element_diagnostic_infos: None,
            },
        });
        let decoded = decode_event_filter_result(&good, 2).unwrap();
        assert!(decoded.select.iter().all(|c| c.is_good()));
        // P0-3：where element 结果原样返回（调用方按 scope 判定）。
        assert_eq!(decoded.where_element, vec![StatusCode::Good]);
        // 数量对不上即违约
        assert!(decode_event_filter_result(&good, 3).is_err());
        // 非 EventFilterResult 即协议错误
        assert!(decode_event_filter_result(&ExtensionObject::null(), 0).is_err());
        // 空 where 即空数组（scope=all 形态）
        let no_where = ExtensionObject::from_message(EventFilterResult {
            select_clause_results: Some(vec![StatusCode::Good]),
            select_clause_diagnostic_infos: None,
            where_clause_result: Default::default(),
        });
        let decoded = decode_event_filter_result(&no_where, 1).unwrap();
        assert!(decoded.where_element.is_empty());
    }

    #[test]
    fn monitor_request_freezes_event_semantics() {
        let item = UaEventMonitoredItemSpec {
            notifier: UaNodeRef::numeric(0, 2253),
            client_handle: 7,
            queue_size: 1000,
            filter: UaEventFilterSpec {
                select_clauses: vec![clause(2041, &[], 15)],
                of_type: None,
            },
        };
        let req = build_event_monitored_item_request(&item).unwrap();
        assert_eq!(req.item_to_monitor.attribute_id, 12); // EventNotifier
        assert!(!req.requested_parameters.discard_oldest); // §6：恒 false
        assert_eq!(req.requested_parameters.queue_size, 1000);
        assert_eq!(req.requested_parameters.client_handle, 7);
        assert!(!req.requested_parameters.filter.is_null());
    }
}
