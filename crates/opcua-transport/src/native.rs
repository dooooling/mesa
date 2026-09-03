//! 基于 `async-opcua 0.19` 的原生传输实现。
//!
//! 职责：Session / Read / Browse / NamespaceArray / Subscription+MonitoredItems 的
//! 分裂生命周期 + Revised 参数保留 + 幂等 cleanup。数据语义（Quality / ValueOrigin /
//! LastKnown）不在此处理，由上层 Driver Adapter 负责。

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
    OpcUaConnectOptions, OpcUaTransport, UaBrowseNode, UaBrowsePage, UaBrowseRequest, UaDataChange,
    UaDataValue, UaIdentifier, UaMonitoredItemResult, UaMonitoredItemSpec, UaNodeClass, UaNodeRef,
    UaOperation, UaSubscription, UaSubscriptionId, UaSubscriptionSpec, UaTransportError,
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
    _handle: Option<tokio::task::JoinHandle<StatusCode>>,
}

/// 原生传输：持有 Session + event-loop JoinHandle；`disconnect()` 释放会话。
pub struct NativeOpcUaTransport {
    options: OpcUaConnectOptions,
    inner: Arc<AsyncMutex<Option<NativeInner>>>,
}

impl NativeOpcUaTransport {
    pub fn new(options: OpcUaConnectOptions) -> Self {
        Self {
            options,
            inner: Arc::new(AsyncMutex::new(None)),
        }
    }

    pub fn options(&self) -> &OpcUaConnectOptions {
        &self.options
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
            _handle: Some(handle),
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
        let mut guard = self.inner.lock().await;
        *guard = None;
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
        let mut nodes = Vec::new();
        for r in results {
            // 整包 BAD → Err；单引用缺失仅跳过
            if r.status_code.is_bad() {
                let status = r.status_code;
                return Err(UaTransportError::service(
                    UaOperation::Browse,
                    Some(status),
                    false,
                    format!("browse 整包失败: {status:?}"),
                ));
            }
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
        }
        Ok(UaBrowsePage { nodes })
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
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let tx_cb = tx.clone();
        let callback =
            DataChangeCallback::new(move |dv: DataValue, item: &opcua_client::MonitoredItem| {
                let ev = UaDataChange {
                    client_handle: item.client_handle(),
                    data_value: dv,
                };
                let _ = tx_cb.try_send(ev);
            });
        let pub_interval = Duration::from_millis(spec.publishing_interval_ms.max(10));
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
        // Revised 参数从 session 内订阅状态回读（async-opcua 仅返回 id，不回 revised）
        let (revised_pub_ms, revised_lifetime, revised_keep_alive) = {
            let state = session.subscription_state();
            let guard = state.lock();
            match guard.get(sub_id) {
                Some(sub) => (
                    sub.publishing_interval().as_millis() as u64,
                    sub.lifetime_count(),
                    sub.max_keep_alive_count(),
                ),
                None => (
                    spec.publishing_interval_ms,
                    spec.lifetime_count,
                    spec.max_keep_alive_count,
                ),
            }
        };
        Ok(UaSubscription {
            id: sub_id,
            requested_publishing_interval_ms: spec.publishing_interval_ms,
            revised_publishing_interval_ms: revised_pub_ms,
            revised_lifetime_count: revised_lifetime,
            revised_max_keep_alive_count: revised_keep_alive,
            receiver: rx,
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
                // 逐项 BAD 仍整体 Ok（幂等语义：已删/无此项即成功），仅记录诊断
                for (id, st) in ids.iter().zip(results.iter()) {
                    if !st.is_good() {
                        tracing::debug!(monitored_item_id = id, status = ?st, "delete_monitored_items 单项非 Good，视为幂等成功");
                    }
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
            return Ok(());
        };
        // 本地无该订阅即幂等 Ok，避免向已关闭会话发包
        {
            let state = sess.subscription_state();
            let guard = state.lock();
            if !guard.subscription_exists(id) {
                return Ok(());
            }
        }
        match sess.delete_subscription(id).await {
            Ok(status) => {
                if status.is_good() || status == StatusCode::BadSubscriptionIdInvalid {
                    Ok(())
                } else if status == StatusCode::BadSessionClosed
                    || status == StatusCode::BadSessionIdInvalid
                {
                    tracing::debug!(?status, "delete_subscription 会话已关闭，幂等 Ok");
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
                    return Ok(());
                }
                Err(ue)
            }
        }
    }
}
