//! 可脚本化的 Fake 传输（Stage 2 P0-B adapter 层测试专用）。
//!
//! 不触网：为上层 Driver Adapter 提供确定性行为——命名空间数组、多页 Browse、
//! 逐项 BAD 的 MonitoredItems、可观测的 cleanup 记录。真实协议语义仍由
//! contract 测试覆盖，本 Fake 只验证 Adapter 的编排逻辑（回滚/合成事件/翻页聚合）。

use std::collections::{HashMap, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

use async_trait::async_trait;
use opcua_types::{DataValue, StatusCode};

use crate::{
    OpcUaTransport, SubscriptionStats, UaBrowseNode, UaBrowsePage, UaBrowseRequest, UaDataChange,
    UaDataValue, UaMonitoredItemResult, UaMonitoredItemSpec, UaNodeRef, UaOperation,
    UaSubscription, UaSubscriptionId, UaSubscriptionSpec, UaTransportError,
};

fn node_key(n: &UaNodeRef) -> String {
    n.to_string()
}

/// 预置的订阅事件：发往本次 `create_subscription` 返回的 receiver。
#[derive(Debug, Clone, Default)]
pub struct FakeLiveBatch {
    pub events: Vec<(u32, DataValue)>,
}

/// Fake 传输：所有行为由预置脚本决定，调用轨迹可查询。
#[derive(Debug, Default)]
pub struct FakeOpcUaTransport {
    namespace_array: Mutex<Vec<String>>,
    /// 预置 `read_namespace_array` 整体失败（probe read 证据合并测试用）。
    namespace_error: Mutex<Option<UaTransportError>>,
    reads: Mutex<HashMap<String, DataValue>>,
    /// node key → 待返回的页队列（首个由 browse 返回，后续经 continuation 接力）。
    browse_pages: Mutex<HashMap<String, VecDeque<UaBrowsePage>>>,
    /// continuation token → 剩余页队列。
    pending: Mutex<HashMap<Vec<u8>, VecDeque<UaBrowsePage>>>,
    /// node key → create_monitored_items 逐项状态（缺省 Good）。
    create_status: Mutex<HashMap<String, StatusCode>>,
    /// 预置 create_monitored_items 服务级整体失败（P0-B3 回滚测试用）。
    create_mi_error: Mutex<Option<UaTransportError>>,
    /// 预置 delete_monitored_items 非幂等失败（P1-B7 测试用）。
    delete_mi_error: Mutex<Option<UaTransportError>>,
    /// create_subscription 返回前注入 receiver 的事件。
    live_batches: Mutex<VecDeque<FakeLiveBatch>>,
    /// 预置下一次（及以后所有）`create_event_subscription` 返回前注入的原生事件通知。
    event_batches: Mutex<VecDeque<crate::event::UaEventNotification>>,
    /// 存活的事件 sender（订阅 id → sender）：Fake 必须持有发送端，
    /// 否则 receiver 建完即见 None（与 Native“sender 随会话存活”语义一致）；
    /// `delete_subscription` 时摘除，通道关闭。
    event_senders: Mutex<
        HashMap<UaSubscriptionId, tokio::sync::mpsc::Sender<crate::event::UaEventNotification>>,
    >,
    /// 存活的 fatal sender（订阅 id → sender）：同上，否则 changed() 建完即 Err。
    event_fatal_senders: Mutex<
        HashMap<
            UaSubscriptionId,
            tokio::sync::watch::Sender<Option<crate::event::UaEventStreamFatal>>,
        >,
    >,
    /// notifier key → create_event_monitored_items 逐项状态（缺省 Good）。
    event_create_status: Mutex<HashMap<String, StatusCode>>,
    /// notifier key → 逐 clause 状态（缺省全 Good，长度自动对齐 clauses 数）。
    event_clause_statuses: Mutex<HashMap<String, Vec<StatusCode>>>,
    /// 预置 create_event_subscription 整体失败（session loss 类测试用）。
    event_sub_error: Mutex<Option<UaTransportError>>,
    /// 预置 create_event_monitored_items 服务级整体失败（回滚测试用）。
    event_mi_error: Mutex<Option<UaTransportError>>,
    next_sub_id: AtomicU32,
    next_mi_id: AtomicU32,
    created_subs: Mutex<Vec<UaSubscriptionId>>,
    deleted_subs: Mutex<Vec<UaSubscriptionId>>,
    deleted_items: Mutex<Vec<(UaSubscriptionId, Vec<u32>)>>,
    released: Mutex<Vec<Vec<u8>>>,
}

impl FakeOpcUaTransport {
    pub fn new() -> Self {
        Self {
            next_sub_id: AtomicU32::new(1),
            next_mi_id: AtomicU32::new(100),
            ..Default::default()
        }
    }

    pub fn with_namespace_array(self, uris: Vec<String>) -> Self {
        *self.namespace_array.lock().unwrap() = uris;
        self
    }

    /// 让 `read_namespace_array` 返回指定错误（缺省成功）。
    pub fn with_namespace_array_error(self, err: UaTransportError) -> Self {
        *self.namespace_error.lock().unwrap() = Some(err);
        self
    }

    pub fn with_read(self, node: &UaNodeRef, value: DataValue) -> Self {
        self.reads.lock().unwrap().insert(node_key(node), value);
        self
    }

    /// 预置多页 Browse：`pages[0]` 由 `browse` 返回；若某页 `continuation_point`
    /// 为 Some(token)，剩余页挂到该 token 下由 `browse_next` 接力。
    pub fn with_browse_pages(self, node: &UaNodeRef, pages: Vec<UaBrowsePage>) -> Self {
        self.browse_pages
            .lock()
            .unwrap()
            .insert(node_key(node), pages.into());
        self
    }

    pub fn with_create_status(self, node: &UaNodeRef, status: StatusCode) -> Self {
        self.create_status
            .lock()
            .unwrap()
            .insert(node_key(node), status);
        self
    }

    /// 让下一次（及以后所有）`create_monitored_items` 整体返回 Err，
    /// 模拟服务级失败，验证 adapter 回滚路径。
    pub fn with_create_monitored_items_error(self, err: UaTransportError) -> Self {
        *self.create_mi_error.lock().unwrap() = Some(err);
        self
    }

    /// 让 `delete_monitored_items` 返回指定错误（P1-B7：删项失败也必须删订阅）。
    pub fn with_delete_monitored_items_error(self, err: UaTransportError) -> Self {
        *self.delete_mi_error.lock().unwrap() = Some(err);
        self
    }

    /// 下一次 `create_subscription` 返回的 receiver 将先收到这些事件。
    pub fn with_live_batch(self, batch: FakeLiveBatch) -> Self {
        self.live_batches.lock().unwrap().push_back(batch);
        self
    }

    /// 预置原生事件通知：下一次 `create_event_subscription` 返回的 receiver
    /// 将按序先收到这些通知（Fake 只造 `UaEventNotification`，绝不直接造
    /// EventRecord——decoder 必须走真实路径）。
    pub fn with_event_notifications(
        self,
        notifications: Vec<crate::event::UaEventNotification>,
    ) -> Self {
        self.event_batches.lock().unwrap().extend(notifications);
        self
    }

    /// 预置某 notifier 的 `create_event_monitored_items` 逐项状态。
    pub fn with_event_item_status(self, notifier: &UaNodeRef, status: StatusCode) -> Self {
        self.event_create_status
            .lock()
            .unwrap()
            .insert(node_key(notifier), status);
        self
    }

    /// 预置某 notifier 的逐 clause 状态（bad filter 类测试用）。
    pub fn with_event_clause_statuses(
        self,
        notifier: &UaNodeRef,
        statuses: Vec<StatusCode>,
    ) -> Self {
        self.event_clause_statuses
            .lock()
            .unwrap()
            .insert(node_key(notifier), statuses);
        self
    }

    /// 让 `create_event_subscription` 整体返回 Err（session loss 类测试用）。
    pub fn with_event_subscription_error(self, err: UaTransportError) -> Self {
        *self.event_sub_error.lock().unwrap() = Some(err);
        self
    }

    /// 让 `create_event_monitored_items` 整体返回 Err（回滚测试用）。
    pub fn with_event_monitored_items_error(self, err: UaTransportError) -> Self {
        *self.event_mi_error.lock().unwrap() = Some(err);
        self
    }

    pub fn created_subscriptions(&self) -> Vec<UaSubscriptionId> {
        self.created_subs.lock().unwrap().clone()
    }

    pub fn deleted_subscriptions(&self) -> Vec<UaSubscriptionId> {
        self.deleted_subs.lock().unwrap().clone()
    }

    pub fn deleted_items(&self) -> Vec<(UaSubscriptionId, Vec<u32>)> {
        self.deleted_items.lock().unwrap().clone()
    }

    pub fn released_continuations(&self) -> Vec<Vec<u8>> {
        self.released.lock().unwrap().clone()
    }

    /// 取一页并把剩余页挂到其 continuation token 下（无剩余则不挂）。
    fn stage_page(
        &self,
        mut queue: VecDeque<UaBrowsePage>,
    ) -> Result<UaBrowsePage, UaTransportError> {
        let page = queue.pop_front().ok_or_else(|| {
            UaTransportError::service(UaOperation::Browse, None, false, "Fake 页队列为空")
        })?;
        if let Some(tok) = page.continuation_point.clone()
            && !queue.is_empty()
        {
            self.pending.lock().unwrap().insert(tok, queue);
        }
        Ok(page)
    }
}

#[async_trait]
impl OpcUaTransport for FakeOpcUaTransport {
    async fn connect(&self) -> Result<(), UaTransportError> {
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), UaTransportError> {
        Ok(())
    }

    async fn read(&self, nodes: &[UaNodeRef]) -> Result<Vec<UaDataValue>, UaTransportError> {
        let reads = self.reads.lock().unwrap();
        let mut out = Vec::with_capacity(nodes.len());
        for n in nodes {
            match reads.get(&node_key(n)) {
                Some(dv) => out.push(dv.clone()),
                None => out.push(DataValue::new_now_status(
                    0i32,
                    StatusCode::BadNodeIdUnknown,
                )),
            }
        }
        Ok(out)
    }

    async fn browse(&self, request: UaBrowseRequest) -> Result<UaBrowsePage, UaTransportError> {
        let mut pages = self.browse_pages.lock().unwrap();
        match pages.remove(&node_key(&request.node)) {
            Some(queue) => self.stage_page(queue),
            None => Ok(UaBrowsePage {
                nodes: Vec::new(),
                continuation_point: None,
            }),
        }
    }

    async fn browse_next(
        &self,
        continuation_point: Vec<u8>,
    ) -> Result<UaBrowsePage, UaTransportError> {
        let mut pending = self.pending.lock().unwrap();
        match pending.remove(&continuation_point) {
            Some(queue) => self.stage_page(queue),
            None => Err(UaTransportError::service(
                UaOperation::Browse,
                Some(StatusCode::BadContinuationPointInvalid),
                false,
                "Fake 未知 continuation token",
            )),
        }
    }

    async fn release_continuation(
        &self,
        continuation_point: Vec<u8>,
    ) -> Result<(), UaTransportError> {
        self.pending.lock().unwrap().remove(&continuation_point);
        self.released.lock().unwrap().push(continuation_point);
        Ok(())
    }

    async fn read_namespace_array(&self) -> Result<Vec<String>, UaTransportError> {
        if let Some(e) = self.namespace_error.lock().unwrap().clone() {
            return Err(e);
        }
        Ok(self.namespace_array.lock().unwrap().clone())
    }

    async fn create_subscription(
        &self,
        spec: UaSubscriptionSpec,
    ) -> Result<UaSubscription, UaTransportError> {
        let id = self.next_sub_id.fetch_add(1, Ordering::SeqCst);
        self.created_subs.lock().unwrap().push(id);
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        if let Some(batch) = self.live_batches.lock().unwrap().pop_front() {
            for (handle, dv) in batch.events {
                let _ = tx.try_send(UaDataChange {
                    client_handle: handle,
                    data_value: dv,
                });
            }
        }
        Ok(UaSubscription {
            id,
            requested_publishing_interval_ms: spec.publishing_interval_ms,
            revised_publishing_interval_ms: spec.publishing_interval_ms,
            revised_lifetime_count: spec.lifetime_count,
            revised_max_keep_alive_count: spec.max_keep_alive_count,
            receiver: rx,
            stats: Arc::new(SubscriptionStats::default()),
        })
    }

    async fn create_monitored_items(
        &self,
        _subscription_id: UaSubscriptionId,
        items: &[UaMonitoredItemSpec],
    ) -> Result<Vec<UaMonitoredItemResult>, UaTransportError> {
        if let Some(err) = self.create_mi_error.lock().unwrap().clone() {
            return Err(err);
        }
        let statuses = self.create_status.lock().unwrap();
        let mut out = Vec::with_capacity(items.len());
        for spec in items {
            let status = statuses
                .get(&node_key(&spec.node))
                .copied()
                .unwrap_or(StatusCode::Good);
            let good = status.is_good();
            out.push(UaMonitoredItemResult {
                client_handle: spec.client_handle,
                monitored_item_id: if good {
                    self.next_mi_id.fetch_add(1, Ordering::SeqCst)
                } else {
                    0
                },
                status_code: status.bits(),
                requested_sampling_interval_ms: spec.sampling_interval_ms,
                revised_sampling_interval_ms: spec.sampling_interval_ms,
                requested_queue_size: spec.queue_size,
                revised_queue_size: spec.queue_size,
            });
        }
        Ok(out)
    }

    async fn delete_monitored_items(
        &self,
        subscription_id: UaSubscriptionId,
        ids: &[u32],
    ) -> Result<(), UaTransportError> {
        self.deleted_items
            .lock()
            .unwrap()
            .push((subscription_id, ids.to_vec()));
        if let Some(err) = self.delete_mi_error.lock().unwrap().clone() {
            return Err(err);
        }
        Ok(())
    }

    async fn delete_subscription(&self, id: UaSubscriptionId) -> Result<(), UaTransportError> {
        self.deleted_subs.lock().unwrap().push(id);
        // 摘除事件发送端，receiver 侧随即见 None（会话结束语义）。
        self.event_senders.lock().unwrap().remove(&id);
        self.event_fatal_senders.lock().unwrap().remove(&id);
        Ok(())
    }

    // PR9-② Fake raw event：只造 UaEventNotification（位置数组），
    // 绝不直接造 EventRecord——decoder 必须走真实路径（§24）。
    async fn create_event_subscription(
        &self,
        spec: UaSubscriptionSpec,
    ) -> Result<crate::event::UaEventSubscription, UaTransportError> {
        if let Some(err) = self.event_sub_error.lock().unwrap().clone() {
            return Err(err);
        }
        let id = self.next_sub_id.fetch_add(1, Ordering::SeqCst);
        self.created_subs.lock().unwrap().push(id);
        let (tx, rx) = tokio::sync::mpsc::channel(crate::event::EVENT_CALLBACK_QUEUE_CAPACITY);
        let (fatal_tx, fatal_rx) = tokio::sync::watch::channel(None);
        // 预置通知按序注入（单测规模，必须能容纳；超限即脚本错误，直接 panic）。
        for n in self.event_batches.lock().unwrap().drain(..) {
            tx.try_send(n)
                .expect("fake event batch 必须能容纳（单测规模）");
        }
        // 发送端随订阅存活（delete 时摘除），receiver 不会早于会话结束见 None。
        self.event_senders.lock().unwrap().insert(id, tx);
        self.event_fatal_senders
            .lock()
            .unwrap()
            .insert(id, fatal_tx);
        Ok(crate::event::UaEventSubscription {
            id,
            requested_publishing_interval_ms: spec.publishing_interval_ms,
            revised_publishing_interval_ms: spec.publishing_interval_ms,
            revised_lifetime_count: spec.lifetime_count,
            revised_max_keep_alive_count: spec.max_keep_alive_count,
            receiver: rx,
            fatal: fatal_rx,
            stats: Arc::new(crate::event::EventSubscriptionStats::default()),
        })
    }

    async fn create_event_monitored_items(
        &self,
        _subscription_id: UaSubscriptionId,
        items: &[crate::event::UaEventMonitoredItemSpec],
    ) -> Result<Vec<crate::event::UaEventMonitoredItemResult>, UaTransportError> {
        if let Some(err) = self.event_mi_error.lock().unwrap().clone() {
            return Err(err);
        }
        let statuses = self.event_create_status.lock().unwrap();
        let clause_maps = self.event_clause_statuses.lock().unwrap();
        let mut out = Vec::with_capacity(items.len());
        for spec in items {
            let key = node_key(&spec.notifier);
            let status = statuses.get(&key).copied().unwrap_or(StatusCode::Good);
            let good = status.is_good();
            let n = spec.filter.select_clauses.len();
            let clause_statuses = clause_maps
                .get(&key)
                .map(|v| v.iter().map(|s| s.bits()).collect())
                .unwrap_or_else(|| vec![StatusCode::Good.bits(); n]);
            out.push(crate::event::UaEventMonitoredItemResult {
                client_handle: spec.client_handle,
                monitored_item_id: if good {
                    self.next_mi_id.fetch_add(1, Ordering::SeqCst)
                } else {
                    0
                },
                status_code: status.bits(),
                requested_queue_size: spec.queue_size,
                revised_queue_size: spec.queue_size,
                select_clause_statuses: clause_statuses,
            });
        }
        Ok(out)
    }
}

/// 测试用小构造：单个 Variable 引用。
pub fn fake_browse_node(node_id: UaNodeRef, browse_name: &str) -> UaBrowseNode {
    use crate::UaNodeClass;
    UaBrowseNode {
        node_id,
        browse_name: browse_name.to_string(),
        display_name: Some(browse_name.to_string()),
        node_class: UaNodeClass::Variable,
        has_children: None,
    }
}
