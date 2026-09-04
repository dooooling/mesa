//! 基于 `async-opcua 0.19` 的原生传输实现。
//!
//! 职责：Session / Read / Browse / NamespaceArray / Subscription+MonitoredItems 的
//! 分裂生命周期 + Revised 参数保留 + 幂等 cleanup。数据语义（Quality / ValueOrigin /
//! LastKnown）不在此处理，由上层 Driver Adapter 负责。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use opcua_client::{ClientBuilder, IdentityToken, Session};
use opcua_types::{
    ByteString, DataValue, ExtensionObject, Guid, MonitoredItemCreateRequest, MonitoringMode,
    MonitoringParameters, NodeId as OpcNodeId, ReadValueId, ReferenceTypeId, StatusCode,
    TimestampsToReturn, UAString,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    OpcUaConnectOptions, OpcUaTransport, UaBrowseNode, UaBrowsePage, UaBrowseRequest, UaDataValue,
    UaIdentifier, UaMonitoredItemResult, UaMonitoredItemSpec, UaNodeClass, UaNodeRef, UaOperation,
    UaSubscription, UaSubscriptionId, UaSubscriptionSpec, UaTransportError,
    error::map_service_error,
};

/// OPC UA 默认端口 4840。
pub const DEFAULT_OPCUA_PORT: u16 = 4840;

/// NamespaceArray 的固定节点（ns=0;i=2255，String[]）。
fn namespace_array_node() -> UaNodeRef {
    UaNodeRef::numeric(0, 2255)
}

pub(crate) fn to_opc_node_id(node: &UaNodeRef) -> Result<OpcNodeId, UaTransportError> {
    match &node.identifier {
        UaIdentifier::Numeric(n) => Ok(OpcNodeId::new(node.namespace, *n)),
        UaIdentifier::String(s) => Ok(OpcNodeId::new(node.namespace, UAString::from(s.as_str()))),
        UaIdentifier::Guid(g) => {
            let guid = g.parse::<Guid>().map_err(|e| {
                UaTransportError::protocol(UaOperation::Read, format!("GUID 解析失败 {g}: {e:?}"))
            })?;
            Ok(OpcNodeId::new(node.namespace, guid))
        }
        UaIdentifier::Opaque(b64) => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    UaTransportError::protocol(
                        UaOperation::Read,
                        format!("Opaque Base64 解码失败: {e}"),
                    )
                })?;
            Ok(OpcNodeId::new(node.namespace, ByteString::from(bytes)))
        }
    }
}

fn from_opc_node_id(nid: &OpcNodeId) -> UaNodeRef {
    use opcua_types::node_id::Identifier as OpcId;
    match &nid.identifier {
        OpcId::Numeric(n) => UaNodeRef::numeric(nid.namespace, *n),
        OpcId::String(s) => UaNodeRef::string(nid.namespace, s.as_ref().to_string()),
        OpcId::Guid(g) => UaNodeRef {
            namespace: nid.namespace,
            identifier: UaIdentifier::Guid(g.to_string()),
        },
        OpcId::ByteString(bs) => {
            use base64::Engine as _;
            let b64 = bs
                .value
                .as_ref()
                .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
                .unwrap_or_default();
            UaNodeRef {
                namespace: nid.namespace,
                identifier: UaIdentifier::Opaque(b64),
            }
        }
    }
}

struct NativeInner {
    endpoint_url: String,
    session: Option<Arc<Session>>,
    handle: Option<tokio::task::JoinHandle<StatusCode>>,
}

/// 协议层数量不变式（P1-3）：请求 N 个必须返回 N 个结果，静默截断即违约。
pub(crate) fn check_cardinality(
    operation: UaOperation,
    what: &str,
    expected: usize,
    actual: usize,
) -> Result<(), UaTransportError> {
    if expected != actual {
        return Err(UaTransportError::service(
            operation,
            None,
            false,
            format!("{what} 数量不变式破坏：请求 {expected} 返回 {actual}"),
        ));
    }
    Ok(())
}

/// cleanup 逐项状态判定（P0-B3.2）：仅明确幂等状态可吞，其他 BAD 必须 Err。
pub(crate) fn check_delete_item_status(
    operation: UaOperation,
    monitored_item_id: u32,
    status: StatusCode,
) -> Result<(), UaTransportError> {
    if status.is_good()
        || status == StatusCode::BadMonitoredItemIdInvalid
        || status == StatusCode::BadSubscriptionIdInvalid
    {
        Ok(())
    } else {
        Err(UaTransportError::service(
            operation,
            Some(status),
            false,
            format!("delete monitored item {monitored_item_id} 失败: {status:?}"),
        ))
    }
}

/// 订阅事件 Latest-Wins 槽（P0-B1）：按 client_handle 只保留最新采样。
/// 回调线程（同步上下文）仅做 slot 覆盖 + 唤醒，永不阻塞；转发任务 drain 最新快照。
pub(crate) struct SlotState {
    pub slots: std::sync::Mutex<HashMap<u32, DataValue>>,
    pub notify: tokio::sync::Notify,
    pub stats: Arc<crate::SubscriptionStats>,
}

impl SlotState {
    pub fn new(stats: Arc<crate::SubscriptionStats>) -> Self {
        Self {
            slots: std::sync::Mutex::new(HashMap::new()),
            notify: tokio::sync::Notify::new(),
            stats,
        }
    }

    /// 同步压入（DataChangeCallback 上下文）：覆盖旧槽并计数 coalesced。
    pub fn push(&self, client_handle: u32, dv: DataValue) {
        use std::sync::atomic::Ordering;
        self.stats.events_received.fetch_add(1, Ordering::Relaxed);
        let mut slots = self.slots.lock().unwrap();
        if slots.insert(client_handle, dv).is_some() {
            self.stats.events_coalesced.fetch_add(1, Ordering::Relaxed);
        }
        self.notify.notify_one();
    }

    /// 取走全部最新快照（转发任务上下文）。
    pub fn drain(&self) -> Vec<crate::UaDataChange> {
        let mut slots = self.slots.lock().unwrap();
        slots
            .drain()
            .map(|(client_handle, data_value)| crate::UaDataChange {
                client_handle,
                data_value,
            })
            .collect()
    }
}

/// 单个 BrowseResult → 单页：整包 BAD 上抛 Err；引用逐条映射；
/// `continuation_point` 原样透传（opaque，调用方不得解析）。
pub(crate) fn browse_result_to_page(
    r: opcua_types::BrowseResult,
) -> Result<UaBrowsePage, UaTransportError> {
    if r.status_code.is_bad() {
        let status = r.status_code;
        return Err(UaTransportError::service(
            UaOperation::Browse,
            Some(status),
            false,
            format!("browse 整包失败: {status:?}"),
        ));
    }
    let mut nodes = Vec::new();
    if let Some(refs) = r.references {
        for rf in refs {
            let node_class = match rf.node_class {
                opcua_types::NodeClass::Object => UaNodeClass::Object,
                opcua_types::NodeClass::Variable => UaNodeClass::Variable,
                opcua_types::NodeClass::Method => UaNodeClass::Method,
                _ => UaNodeClass::Unknown,
            };
            nodes.push(UaBrowseNode {
                node_id: from_opc_node_id(&rf.node_id.node_id),
                browse_name: rf.browse_name.name.as_ref().to_string(),
                display_name: {
                    let s = rf.display_name.text.as_ref().to_string();
                    if s.is_empty() { None } else { Some(s) }
                },
                node_class,
                // 单次 Browse 不做 N+1 探测，一律 None（调用方按需再 Browse）
                has_children: None,
            });
        }
    }
    Ok(UaBrowsePage {
        nodes,
        continuation_point: r.continuation_point.value.clone(),
    })
}

/// 订阅转发循环（P0-B1/P1-B5）：drain slot 最新快照后 `send().await`。
/// 抽取为独立函数以便慢消费者饱和测试直接驱动；生产路径由
/// `create_subscription` 在 Revised 校验通过后 spawn。
pub(crate) async fn forwarder_loop(
    slots: Arc<SlotState>,
    tx: tokio::sync::mpsc::Sender<crate::UaDataChange>,
) {
    loop {
        // P1-B5 防御：receiver 被直接丢弃时，不必等下一次 Notify 即可退出。
        tokio::select! {
            _ = tx.closed() => break,
            _ = slots.notify.notified() => {}
        }
        if tx.is_closed() {
            break;
        }
        let batch = slots.drain();
        if batch.is_empty() {
            continue;
        }
        for ev in batch {
            // 背压时等待：旧槽可被新采样覆盖（Latest-Wins），最新采样不丢。
            if tx.send(ev).await.is_err() {
                return;
            }
        }
    }
}

/// 原生传输：持有 Session + event-loop JoinHandle；`disconnect()` 释放会话。
///
/// P1-B5：每个订阅的 forwarder task 由 `forwarders[sub_id]` 跟踪，生命周期与
/// subscription_id 绑定——创建成功且 Revised 校验通过后才 spawn，cleanup 成功
/// 路径必 abort。否则正常 unsubscribe/shutdown 也会残留睡死在 Notify 上的 task。
pub struct NativeOpcUaTransport {
    options: OpcUaConnectOptions,
    inner: Arc<AsyncMutex<Option<NativeInner>>>,
    forwarders: Arc<AsyncMutex<HashMap<UaSubscriptionId, tokio::task::JoinHandle<()>>>>,
}

impl NativeOpcUaTransport {
    pub fn new(options: OpcUaConnectOptions) -> Self {
        Self {
            options,
            inner: Arc::new(AsyncMutex::new(None)),
            forwarders: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    pub fn options(&self) -> &OpcUaConnectOptions {
        &self.options
    }

    /// P1-B5：按 sub_id 摘除并 abort forwarder（幂等，无 handle 即空操作）。
    /// abort 后 transport tx 被 drop，adapter 侧 sub_rx 得 None，其 forwarder
    /// 自然退出，整条链闭环——unsubscribe/shutdown 不再残留 detached task。
    async fn abort_forwarder(&self, id: UaSubscriptionId) {
        if let Some(h) = self.forwarders.lock().await.remove(&id) {
            tracing::debug!(sub_id = id, "abort 订阅 forwarder");
            h.abort();
        }
    }

    /// 直达服务端的删订阅（跳过本地存在性检查）。
    /// 仅供 Revised 缺席回滚使用：此时本地状态恰恰缺席，但服务端订阅已存在。
    async fn delete_subscription_rpc(
        &self,
        sess: &Session,
        id: UaSubscriptionId,
    ) -> Result<(), UaTransportError> {
        match sess.delete_subscription(id).await {
            Ok(status) => {
                if status.is_good() || status == StatusCode::BadSubscriptionIdInvalid {
                    self.abort_forwarder(id).await;
                    Ok(())
                } else if status == StatusCode::BadSessionClosed
                    || status == StatusCode::BadSessionIdInvalid
                {
                    tracing::debug!(?status, "delete_subscription 会话已关闭，幂等 Ok");
                    self.abort_forwarder(id).await;
                    Ok(())
                } else {
                    Err(UaTransportError::service(
                        UaOperation::DeleteSubscription,
                        Some(status),
                        false,
                        format!("delete_subscription 失败: {status:?}"),
                    ))
                }
            }
            Err(e) => {
                let ue = map_service_error(UaOperation::DeleteSubscription, e);
                if ue.is_idempotent_cleanup_ok() {
                    tracing::debug!(?ue, "delete_subscription cleanup 幂等 Ok");
                    self.abort_forwarder(id).await;
                    return Ok(());
                }
                Err(ue)
            }
        }
    }

    async fn ensure_session(&self) -> Result<Arc<Session>, UaTransportError> {
        {
            let guard = self.inner.lock().await;
            if let Some(inner) = guard.as_ref()
                && inner.endpoint_url == self.options.endpoint_url
                && let Some(sess) = &inner.session
            {
                return Ok(sess.clone());
            }
        }
        self.connect_inner().await
    }

    async fn connect_inner(&self) -> Result<Arc<Session>, UaTransportError> {
        self.options.validate()?;
        let timeout = Duration::from_millis(self.options.timeout_ms);
        let mut client = ClientBuilder::new()
            .application_name(self.options.application_name.as_str())
            .application_uri(self.options.application_uri.as_str())
            .pki_dir(self.options.pki_dir.clone())
            .certificate_path("own/own.der")
            .private_key_path("own/own.key")
            .trust_server_certs(false)
            .verify_server_certs(true)
            .create_sample_keypair(false)
            .session_retry_limit(1)
            .client()
            .map_err(|e| {
                UaTransportError::new(
                    crate::UaTransportErrorKind::Connect,
                    UaOperation::Connect,
                    None,
                    false,
                    format!("ClientBuilder 失败: {e:?}"),
                )
            })?;

        use opcua_types::{EndpointDescription, MessageSecurityMode, UserTokenPolicy};
        let mode = match self.options.security_mode.as_str() {
            "Sign" => MessageSecurityMode::Sign,
            "SignAndEncrypt" => MessageSecurityMode::SignAndEncrypt,
            _ => MessageSecurityMode::None,
        };
        let (user_policy, identity) =
            if let (Some(u), Some(p)) = (&self.options.username, &self.options.password) {
                let pol = UserTokenPolicy {
                    policy_id: UAString::from("username"),
                    token_type: opcua_types::UserTokenType::UserName,
                    issued_token_type: UAString::null(),
                    issuer_endpoint_url: UAString::null(),
                    security_policy_uri: UAString::from(self.options.security_policy.as_str()),
                };
                (
                    pol,
                    IdentityToken::UserName(u.clone(), opcua_client::Password(p.clone())),
                )
            } else {
                (UserTokenPolicy::anonymous(), IdentityToken::Anonymous)
            };
        let endpoint: EndpointDescription = (
            self.options.endpoint_url.as_str(),
            self.options.security_policy.as_str(),
            mode,
            user_policy,
        )
            .into();

        let connect_fut = client.connect_to_matching_endpoint(endpoint, identity);
        let (session, event_loop) = tokio::time::timeout(timeout, connect_fut)
            .await
            .map_err(|_| {
                UaTransportError::timeout(
                    UaOperation::Connect,
                    format!(
                        "连接超时 {}ms {}",
                        self.options.timeout_ms, self.options.endpoint_url
                    ),
                )
            })?
            .map_err(|e| map_service_error(UaOperation::Connect, e))?;

        let handle = event_loop.spawn();
        let wait_fut = session.wait_for_connection();
        tokio::time::timeout(timeout, wait_fut).await.map_err(|_| {
            UaTransportError::timeout(
                UaOperation::Connect,
                format!("等待连接超时 {}", self.options.endpoint_url),
            )
        })?;

        let sess_clone = session.clone();
        let mut guard = self.inner.lock().await;
        *guard = Some(NativeInner {
            endpoint_url: self.options.endpoint_url.clone(),
            session: Some(session.clone()),
            handle: Some(handle),
        });
        Ok(sess_clone)
    }
}

#[async_trait]
impl OpcUaTransport for NativeOpcUaTransport {
    async fn connect(&self) -> Result<(), UaTransportError> {
        self.ensure_session().await.map(|_| ())
    }

    async fn disconnect(&self) -> Result<(), UaTransportError> {
        // P1-1：证明性关闭——先请会话正常断开，再 abort event-loop task，最后清空槽位。
        // 仅丢 JoinHandle 是 detach 而非退出，必须显式 abort。
        // P1-B5：同时 abort 全部订阅 forwarder（会话已死，它们只会睡死在 Notify 上）。
        let taken = {
            let mut guard = self.inner.lock().await;
            guard.take()
        };
        if let Some(inner) = taken {
            if let Some(sess) = inner.session
                && let Err(e) = sess.disconnect().await
            {
                tracing::debug!("session disconnect 返回错误（忽略，继续清理）: {e:?}");
            }
            if let Some(handle) = inner.handle {
                handle.abort();
            }
        }
        let mut fwd = self.forwarders.lock().await;
        for (id, h) in fwd.drain() {
            tracing::debug!(sub_id = id, "disconnect abort 订阅 forwarder");
            h.abort();
        }
        Ok(())
    }

    async fn read(&self, nodes: &[UaNodeRef]) -> Result<Vec<UaDataValue>, UaTransportError> {
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| {
                    UaTransportError::session(UaOperation::Read, None, "尚未连接，请先 connect")
                })?
        };
        let mut to_read = Vec::with_capacity(nodes.len());
        for n in nodes {
            to_read.push(ReadValueId::from(to_opc_node_id(n)?));
        }
        let values: Vec<DataValue> = session
            .read(&to_read, TimestampsToReturn::Both, 0.0)
            .await
            .map_err(|e| map_service_error(UaOperation::Read, e))?;
        // P1-3：数量不变式——请求 N 个必须返回 N 个，缺项静默对齐即违约。
        check_cardinality(UaOperation::Read, "Read 结果", nodes.len(), values.len())?;
        // 单点 BAD 以 DataValue.status 逐点返回，不整体 Err（P0-B 冻结语义）
        Ok(values)
    }

    async fn read_namespace_array(&self) -> Result<Vec<String>, UaTransportError> {
        let values = self
            .read(std::slice::from_ref(&namespace_array_node()))
            .await?;
        let dv = values.into_iter().next().ok_or_else(|| {
            UaTransportError::service(
                UaOperation::ReadNamespaceArray,
                None,
                false,
                "NamespaceArray 返回为空",
            )
        })?;
        if let Some(status) = dv.status
            && !status.is_good()
        {
            return Err(UaTransportError::service(
                UaOperation::ReadNamespaceArray,
                Some(status),
                true,
                format!("NamespaceArray 读取失败: {status:?}"),
            ));
        }
        let variant = dv.value.ok_or_else(|| {
            UaTransportError::service(
                UaOperation::ReadNamespaceArray,
                None,
                false,
                "NamespaceArray 无 value",
            )
        })?;
        match variant {
            opcua_types::Variant::Array(arr) => {
                let mut out = Vec::with_capacity(arr.values.len());
                for v in arr.values.iter() {
                    match v {
                        opcua_types::Variant::String(s) => out.push(s.as_ref().to_string()),
                        other => {
                            return Err(UaTransportError::service(
                                UaOperation::ReadNamespaceArray,
                                None,
                                false,
                                format!("NamespaceArray 元素非 String: {other:?}"),
                            ));
                        }
                    }
                }
                Ok(out)
            }
            other => Err(UaTransportError::service(
                UaOperation::ReadNamespaceArray,
                None,
                false,
                format!("NamespaceArray 非 Array: {other:?}"),
            )),
        }
    }

    async fn browse(&self, request: UaBrowseRequest) -> Result<UaBrowsePage, UaTransportError> {
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| {
                    UaTransportError::session(UaOperation::Browse, None, "尚未连接，请先 connect")
                })?
        };
        let nid = to_opc_node_id(&request.node)?;
        use opcua_types::{BrowseDescription, BrowseDirection};
        let bd = BrowseDescription {
            node_id: nid,
            browse_direction: BrowseDirection::Forward,
            reference_type_id: ReferenceTypeId::HierarchicalReferences.into(),
            include_subtypes: true,
            node_class_mask: 0,
            result_mask: 0x3F,
        };
        let results = session
            .browse(&[bd], request.max_refs, None)
            .await
            .map_err(|e| map_service_error(UaOperation::Browse, e))?;
        // 单个 BrowseDescription → 单个 BrowseResult，数量不变式同样适用。
        check_cardinality(UaOperation::Browse, "Browse 结果", 1, results.len())?;
        let page = browse_result_to_page(results.into_iter().next().ok_or_else(|| {
            UaTransportError::service(
                UaOperation::Browse,
                None,
                false,
                "Browse 结果为空（已通过数量检查，不可达）",
            )
        })?)?;
        Ok(page)
    }

    async fn browse_next(
        &self,
        continuation_point: Vec<u8>,
    ) -> Result<UaBrowsePage, UaTransportError> {
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| {
                    UaTransportError::session(UaOperation::Browse, None, "尚未连接，请先 connect")
                })?
        };
        // 单 token 接力 → 单结果页；release=false 表示继续取页。
        let results = session
            .browse_next(false, &[ByteString::from(continuation_point)])
            .await
            .map_err(|e| map_service_error(UaOperation::Browse, e))?;
        check_cardinality(UaOperation::Browse, "BrowseNext 结果", 1, results.len())?;
        browse_result_to_page(results.into_iter().next().ok_or_else(|| {
            UaTransportError::service(
                UaOperation::Browse,
                None,
                false,
                "BrowseNext 结果为空（已通过数量检查，不可达）",
            )
        })?)
    }

    async fn release_continuation(
        &self,
        continuation_point: Vec<u8>,
    ) -> Result<(), UaTransportError> {
        let session = {
            let guard = self.inner.lock().await;
            guard.as_ref().and_then(|i| i.session.clone())
        };
        let Some(sess) = session else {
            // 会话已释放：服务端 continuation 随会话失效，幂等 Ok。
            return Ok(());
        };
        match sess
            .browse_next(true, &[ByteString::from(continuation_point)])
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let ue = map_service_error(UaOperation::Browse, e);
                // 释放语义幂等：会话级失败 / continuation 已失效或耗尽均视为 Ok。
                let gone = ue.status_code == Some(StatusCode::BadContinuationPointInvalid.bits())
                    || ue.status_code == Some(StatusCode::BadNoContinuationPoints.bits());
                if ue.is_idempotent_cleanup_ok() || gone {
                    tracing::debug!(?ue, "release_continuation 幂等 Ok");
                    Ok(())
                } else {
                    Err(ue)
                }
            }
        }
    }

    async fn create_subscription(
        &self,
        spec: UaSubscriptionSpec,
    ) -> Result<UaSubscription, UaTransportError> {
        use opcua_client::DataChangeCallback;
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| {
                    UaTransportError::session(
                        UaOperation::CreateSubscription,
                        None,
                        "尚未连接，请先 connect",
                    )
                })?
        };
        // P0-B1：Latest-Wins 事件路径——回调（同步上下文）只做 slot 覆盖，
        // 转发任务 drain 最新快照后 `send().await`（背压等待，永不丢最新）。
        // P1-B5：forwarder 在 CreateSubscription 成功且 Revised 校验通过后才 spawn，
        // 创建失败路径根本无 task 可泄漏；Handle 按 sub_id 登记，cleanup 时 abort。
        let stats = Arc::new(crate::SubscriptionStats::default());
        let slots = Arc::new(SlotState::new(stats.clone()));
        let cb_slots = slots.clone();
        let callback =
            DataChangeCallback::new(move |dv: DataValue, item: &opcua_client::MonitoredItem| {
                cb_slots.push(item.client_handle(), dv);
            });
        // P1-B6：请求间隔原样上送，不做 silent clamp——Server 自会 revised，
        // requested 字段必须等于实际发线值，否则诊断失真。
        let pub_interval = Duration::from_millis(spec.publishing_interval_ms);
        let sub_id = session
            .create_subscription(
                pub_interval,
                spec.lifetime_count,
                spec.max_keep_alive_count,
                spec.max_notifications_per_publish,
                spec.priority,
                spec.publishing_enabled,
                callback,
            )
            .await
            .map_err(|e| map_service_error(UaOperation::CreateSubscription, e))?;
        // Revised 参数从 session 内订阅状态回读（async-opcua 仅返回 id，不回 revised）。
        // P1-2 fail-closed：创建刚成功订阅必在本地状态中，缺席即内部不一致；
        // 此时服务端订阅已存在，必须 best-effort 删掉再 Err（不得拿着
        // subscription_state 锁去 await，先释放锁再调 delete）。
        // 锁只在块内持有：读完即释，绝不拿着 subscription_state 锁进 await。
        let revised = {
            let state = session.subscription_state();
            let guard = state.lock();
            guard.get(sub_id).map(|sub| {
                (
                    sub.publishing_interval().as_millis() as u64,
                    sub.lifetime_count(),
                    sub.max_keep_alive_count(),
                )
            })
        };
        let Some((revised_pub_ms, revised_lifetime, revised_keep_alive)) = revised else {
            // 直达服务端删除（本地状态缺席，普通 delete 会因存在性检查跳过 RPC）。
            if let Err(cleanup) = self.delete_subscription_rpc(&session, sub_id).await {
                tracing::debug!(sub_id, ?cleanup, "Revised 缺席回滚删订阅失败（仅诊断）");
            }
            return Err(UaTransportError::internal(
                UaOperation::CreateSubscription,
                format!(
                    "订阅 {sub_id} 创建成功但本地订阅状态缺席（内部不一致，已回滚，fail-closed）"
                ),
            ));
        };
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let fwd_slots = slots.clone();
        let fwd_handle = tokio::spawn(async move { forwarder_loop(fwd_slots, tx).await });
        self.forwarders.lock().await.insert(sub_id, fwd_handle);
        Ok(UaSubscription {
            id: sub_id,
            requested_publishing_interval_ms: spec.publishing_interval_ms,
            revised_publishing_interval_ms: revised_pub_ms,
            revised_lifetime_count: revised_lifetime,
            revised_max_keep_alive_count: revised_keep_alive,
            receiver: rx,
            stats,
        })
    }

    async fn create_monitored_items(
        &self,
        subscription_id: UaSubscriptionId,
        items: &[UaMonitoredItemSpec],
    ) -> Result<Vec<UaMonitoredItemResult>, UaTransportError> {
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| {
                    UaTransportError::session(
                        UaOperation::CreateMonitoredItems,
                        None,
                        "尚未连接，请先 connect",
                    )
                })?
        };
        let mut reqs = Vec::with_capacity(items.len());
        for it in items {
            let nid = to_opc_node_id(&it.node)?;
            let params = MonitoringParameters {
                client_handle: it.client_handle,
                sampling_interval: it.sampling_interval_ms as f64,
                filter: ExtensionObject::null(),
                queue_size: it.queue_size,
                discard_oldest: it.discard_oldest,
            };
            reqs.push(MonitoredItemCreateRequest::new(
                ReadValueId::from(nid),
                MonitoringMode::Reporting,
                params,
            ));
        }
        let created = session
            .create_monitored_items(subscription_id, TimestampsToReturn::Both, reqs)
            .await
            .map_err(|e| map_service_error(UaOperation::CreateMonitoredItems, e))?;
        // P1-3：数量不变式——逐项结果必须与请求一一对应，否则调用方无法归因。
        check_cardinality(
            UaOperation::CreateMonitoredItems,
            "CreateMonitoredItems 结果",
            items.len(),
            created.len(),
        )?;
        // 逐项结果：保留 status + revised sampling/queue，部分失败不整体 Err
        let mut out = Vec::with_capacity(created.len());
        for (spec, c) in items.iter().zip(created) {
            let r = &c.result;
            out.push(UaMonitoredItemResult {
                client_handle: spec.client_handle,
                monitored_item_id: r.monitored_item_id,
                status_code: r.status_code.bits(),
                requested_sampling_interval_ms: spec.sampling_interval_ms,
                revised_sampling_interval_ms: r.revised_sampling_interval as u64,
                requested_queue_size: spec.queue_size,
                revised_queue_size: r.revised_queue_size,
            });
        }
        Ok(out)
    }

    async fn delete_monitored_items(
        &self,
        subscription_id: UaSubscriptionId,
        ids: &[crate::UaMonitoredItemId],
    ) -> Result<(), UaTransportError> {
        let session = {
            let guard = self.inner.lock().await;
            guard.as_ref().and_then(|i| i.session.clone())
        };
        let Some(sess) = session else {
            // 会话已释放：cleanup 路径幂等 Ok（正常路径的 SessionClosed 由调用方在有会话时触发，此处无会话即无可清理对象）
            return Ok(());
        };
        match sess.delete_monitored_items(subscription_id, ids).await {
            Ok(results) => {
                // P0-B3.2：逐项判定——仅明确幂等状态可吞，其他 BAD 上抛；
                // 数量不变式同样适用（结果必须与被删 id 一一对应）。
                check_cardinality(
                    UaOperation::DeleteMonitoredItems,
                    "DeleteMonitoredItems 结果",
                    ids.len(),
                    results.len(),
                )?;
                for (id, st) in ids.iter().zip(results.iter()) {
                    check_delete_item_status(UaOperation::DeleteMonitoredItems, *id, *st)?;
                }
                Ok(())
            }
            Err(e) => {
                let ue = map_service_error(UaOperation::DeleteMonitoredItems, e);
                if ue.is_idempotent_cleanup_ok() {
                    tracing::debug!(?ue, "delete_monitored_items cleanup 幂等 Ok");
                    return Ok(());
                }
                Err(ue)
            }
        }
    }

    async fn delete_subscription(&self, id: UaSubscriptionId) -> Result<(), UaTransportError> {
        let session = {
            let guard = self.inner.lock().await;
            guard.as_ref().and_then(|i| i.session.clone())
        };
        let Some(sess) = session else {
            // 无会话即无服务端对象；forwarder 若在（会话丢后残留）一并 abort。
            self.abort_forwarder(id).await;
            return Ok(());
        };
        // 本地无该订阅即幂等 Ok，避免向已关闭会话发包；forwarder 照 abort
        //（防御：handle 登记与本地订阅状态不一致时也不残留 task）。
        // 锁只在块内持有，释放后才进 await。
        let exists = {
            let state = sess.subscription_state();
            let guard = state.lock();
            guard.subscription_exists(id)
        };
        if !exists {
            self.abort_forwarder(id).await;
            return Ok(());
        }
        self.delete_subscription_rpc(&sess, id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn latest_wins_slot_keeps_newest_and_counts_coalesced() {
        let stats = Arc::new(crate::SubscriptionStats::default());
        let slots = SlotState::new(stats.clone());
        slots.push(7, DataValue::new_now(1i32));
        slots.push(7, DataValue::new_now(2i32)); // 覆盖旧采样：coalesced+1
        slots.push(9, DataValue::new_now(3i32));
        assert_eq!(stats.events_received.load(Ordering::Relaxed), 3);
        assert_eq!(stats.events_coalesced.load(Ordering::Relaxed), 1);
        let mut batch = slots.drain();
        batch.sort_by_key(|e| e.client_handle);
        assert_eq!(batch.len(), 2);
        let v7 = batch[0].data_value.value.clone().expect("handle 7 有值");
        assert_eq!(v7, opcua_types::Variant::Int32(2)); // 只保留最新采样
        // drain 后槽清空
        assert!(slots.drain().is_empty());
    }

    #[test]
    fn burst_same_handle_keeps_only_latest_and_counts_all_coalesced() {
        // P0-B1 饱和语义（确定性 burst 版）：同 handle 高速压入 1000 个采样，
        // drain 只得最新一个，coalesced 精确计数被合并的 999 个旧采样。
        // 含转发循环的慢消费者版本见 forwarder_slow_consumer_ends_on_latest。
        let stats = Arc::new(crate::SubscriptionStats::default());
        let slots = SlotState::new(stats.clone());
        for v in 0..1000i32 {
            slots.push(3, DataValue::new_now(v));
        }
        assert_eq!(stats.events_received.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.events_coalesced.load(Ordering::Relaxed), 999);
        let batch = slots.drain();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].client_handle, 3);
        assert_eq!(
            batch[0].data_value.value,
            Some(opcua_types::Variant::Int32(999))
        );
    }

    #[tokio::test]
    async fn forwarder_backpressure_burst_ends_on_latest() {
        // P0-B1 full-path 确定性背压测试（P1-Gate）：channel capacity=1，
        // 全程无 sleep 猜测调度，每一步都有可观测的前置条件。
        // 生产者经 SlotState::push 注入——与真实 DataChangeCallback 同一入口。
        let stats = Arc::new(crate::SubscriptionStats::default());
        let slots = Arc::new(SlotState::new(stats.clone()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let fwd = tokio::spawn(forwarder_loop(slots.clone(), tx.clone()));

        // 步骤 1：先压入 v0，等待 forwarder 把它送进 channel（capacity 归零
        // 即 channel 真满；2s 超时则测试失败而非静默通过）。
        slots.push(7, DataValue::new_now(0i32));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while tx.capacity() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("forwarder 应把首个事件送入 channel 使其变满");

        // 步骤 2：channel 已满且无人消费——此时 burst 1..=200。forwarder
        // 至多再完成一次 send 即被阻塞，后续 push 只能在 slot 里 overwrite。
        for v in 1..=200i32 {
            slots.push(7, DataValue::new_now(v));
        }
        assert_eq!(stats.events_received.load(Ordering::Relaxed), 201);
        // channel 仍满：forwarder 没能多送（否则 permits 会回升又被占，
        // 但无人消费满 channel 不可能被排空——恒满即背压成立）。
        assert_eq!(tx.capacity(), 0, "burst 期间无人消费，channel 必须保持真满");
        assert!(
            stats.events_coalesced.load(Ordering::Relaxed) > 0,
            "背压下同 handle 旧采样必须被合并计数"
        );

        // 步骤 3：开始消费并排空。首个必为 v0（burst 前唯一送达），
        // 中间至多一个 in-flight，终值恒为最新 200。
        let mut first_value: Option<opcua_types::Variant> = None;
        let mut last_value: Option<opcua_types::Variant> = None;
        let mut count = 0u32;
        while let Ok(Some(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
        {
            assert_eq!(ev.client_handle, 7);
            if first_value.is_none() {
                first_value = ev.data_value.value.clone();
            }
            last_value = ev.data_value.value;
            count += 1;
        }
        assert_eq!(
            first_value,
            Some(opcua_types::Variant::Int32(0)),
            "首个送达的必须是 burst 前的 v0"
        );
        // 终值断言：所有 push 已完成且排空至 100ms 空闲，forwarder 必已把
        // 最新快照送出——否则就是 Latest-Wins 违约。
        assert_eq!(
            last_value,
            Some(opcua_types::Variant::Int32(200)),
            "排空终值必须等于最后压入值"
        );
        assert!(count >= 2, "至少应看到 v0 与一个后续快照，实际 {count} 个");

        // 清理：丢 receiver，forwarder 必须经 tx.closed() 自行退出。
        drop(rx);
        drop(tx);
        tokio::time::timeout(std::time::Duration::from_secs(2), fwd)
            .await
            .expect("forwarder 应在 receiver 丢弃后退出")
            .expect("forwarder 不 panic");
    }

    #[test]
    fn cardinality_mismatch_is_non_retryable_service_error() {
        let e = check_cardinality(UaOperation::Read, "Read 结果", 3, 2).expect_err("必须 Err");
        assert_eq!(e.kind, crate::UaTransportErrorKind::Service);
        assert!(!e.retryable);
        assert!(check_cardinality(UaOperation::Read, "Read 结果", 3, 3).is_ok());
    }

    #[test]
    fn delete_item_status_only_explicit_idempotent_passes() {
        let ok = |s| check_delete_item_status(UaOperation::DeleteMonitoredItems, 42, s).is_ok();
        assert!(ok(StatusCode::Good));
        assert!(ok(StatusCode::BadMonitoredItemIdInvalid));
        assert!(ok(StatusCode::BadSubscriptionIdInvalid));
        // 其他 BAD（含会话级）必须上抛，不得在逐项路径吞掉
        assert!(!ok(StatusCode::BadNodeIdUnknown));
        assert!(!ok(StatusCode::BadSessionClosed));
        assert!(!ok(StatusCode::BadNotImplemented));
    }

    #[test]
    fn browse_result_page_passes_through_continuation_opaque() {
        let r = opcua_types::BrowseResult {
            status_code: StatusCode::Good,
            continuation_point: ByteString::from(vec![9u8, 8u8]),
            references: None,
        };
        let page = browse_result_to_page(r).expect("Good 整包必须 Ok");
        assert!(page.nodes.is_empty());
        assert_eq!(page.continuation_point, Some(vec![9u8, 8u8]));
    }

    #[test]
    fn browse_result_bad_status_is_service_error() {
        let r = opcua_types::BrowseResult {
            status_code: StatusCode::Good,
            continuation_point: ByteString::null(),
            references: None,
        };
        // 先构造 Good 再改 BAD，避免构造即错
        let mut bad = r;
        bad.status_code = StatusCode::BadNodeIdUnknown;
        let e = browse_result_to_page(bad).expect_err("整包 BAD 必须 Err");
        assert_eq!(e.kind, crate::UaTransportErrorKind::Service);
        assert_eq!(e.status_code, Some(StatusCode::BadNodeIdUnknown.bits()));
    }
}
