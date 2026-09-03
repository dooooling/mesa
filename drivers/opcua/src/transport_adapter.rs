//! Transport 适配器（Stage 2 P0-B 迁移期）：把公共 [`OpcUaTransport`] 桥接到旧 [`OpcUaApi`] trait。
//!
//! 作用：`run()` / `decode_data_value()` / Fake 路径零改动，只有 Native 会话实现被替换。
//! 数据语义仍在 `decode_data_value()`（本 driver），transport 只交出原生 DataValue。
//! 订阅分裂生命周期在此收敛：`subscribe()` 内依次 `create_subscription` +
//! `create_monitored_items`；`unsubscribe()` 内依次删项 + 删订阅（均幂等）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mesa_opcua_transport::{
    NativeOpcUaTransport, OpcUaConnectOptions, OpcUaTransport, UaBrowseRequest, UaIdentifier,
    UaMonitoredItemSpec, UaNodeRef, UaSubscriptionSpec,
};

use super::opcua_api::{DataChangeEvent, OpcUaApi};
use crate::address::{Identifier, OpcUaAddress};

fn addr_to_node(addr: &OpcUaAddress) -> UaNodeRef {
    let identifier = match &addr.identifier {
        Identifier::Numeric(n) => UaIdentifier::Numeric(*n),
        Identifier::String(s) => UaIdentifier::String(s.clone()),
        Identifier::Guid(g) => UaIdentifier::Guid(g.clone()),
        Identifier::Opaque(b) => UaIdentifier::Opaque(b.clone()),
    };
    UaNodeRef {
        namespace: addr.namespace,
        identifier,
    }
}

/// Native 会话的 transport 实现（PKI 已由上层经 options 注入，本层不读环境变量）。
pub struct TransportApiAdapter {
    transport: Arc<NativeOpcUaTransport>,
    /// sub_id -> 已建成功的 monitored_item_id（供 unsubscribe 按序清理）。
    items: Mutex<HashMap<u32, Vec<u32>>>,
}

impl TransportApiAdapter {
    pub fn new(options: OpcUaConnectOptions) -> Self {
        Self {
            transport: Arc::new(NativeOpcUaTransport::new(options)),
            items: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl OpcUaApi for TransportApiAdapter {
    async fn connect(&self, _endpoint_url: &str, _timeout_ms: u64) -> Result<(), String> {
        // 会话参数已在构造时经 options 固定；此处直接建连（与 cfg 一致，由上层保证）。
        self.transport.connect().await.map_err(|e| e.to_string())
    }

    async fn read_batch(
        &self,
        addrs: &[OpcUaAddress],
    ) -> Result<Vec<opcua_types::DataValue>, String> {
        let nodes: Vec<UaNodeRef> = addrs.iter().map(addr_to_node).collect();
        self.transport.read(&nodes).await.map_err(|e| {
            // 正常路径的 SessionClosed 必须上抛为 Err（由 Manager 退避重建），禁止吞掉
            e.to_string()
        })
    }

    async fn disconnect(&self) -> Result<(), String> {
        self.transport.disconnect().await.map_err(|e| e.to_string())
    }

    async fn subscribe(
        &self,
        addrs: &[OpcUaAddress],
        publishing_interval_ms: u64,
        sampling_interval_ms: u64,
        queue_size: u32,
        discard_oldest: bool,
    ) -> Result<(u32, tokio::sync::mpsc::Receiver<DataChangeEvent>), String> {
        // 分裂生命周期第一步：仅建订阅（保留 Server Revised 参数供诊断）
        let spec = UaSubscriptionSpec {
            publishing_interval_ms,
            lifetime_count: 30,
            max_keep_alive_count: 10,
            max_notifications_per_publish: 0,
            priority: 0,
            publishing_enabled: true,
        };
        let sub = self
            .transport
            .create_subscription(spec)
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!(
            sub_id = sub.id,
            requested = sub.requested_publishing_interval_ms,
            revised = sub.revised_publishing_interval_ms,
            revised_lifetime = sub.revised_lifetime_count,
            revised_keep_alive = sub.revised_max_keep_alive_count,
            "OPC UA 订阅已建立（revised 参数由 Server 协商）"
        );
        // 分裂生命周期第二步：独立建监控项（逐项状态保留，部分失败不整体 Err）
        let mi_specs: Vec<UaMonitoredItemSpec> = addrs
            .iter()
            .enumerate()
            .map(|(idx, addr)| UaMonitoredItemSpec {
                node: addr_to_node(addr),
                client_handle: (idx as u32) + 1,
                sampling_interval_ms,
                queue_size,
                discard_oldest,
            })
            .collect();
        let results = self
            .transport
            .create_monitored_items(sub.id, &mi_specs)
            .await
            .map_err(|e| e.to_string())?;
        let mut ok_ids = Vec::with_capacity(results.len());
        for r in &results {
            if r.status_code == 0 {
                ok_ids.push(r.monitored_item_id);
            } else {
                // 单项失败按单点 BAD 隔离语义记录，不整体失败（运行时该 handle 无事件即保持 LastKnown/Placeholder）
                tracing::warn!(
                    client_handle = r.client_handle,
                    status_code = r.status_code,
                    "监控项创建单项失败，不影响同订阅其他项"
                );
            }
        }
        self.items.lock().unwrap().insert(sub.id, ok_ids);
        // 转发：UaDataChange → DataChangeEvent（同构，try_send 背压语义与旧实现一致）
        let mut sub_rx = sub.receiver;
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            while let Some(ev) = sub_rx.recv().await {
                if tx.is_closed() {
                    break;
                }
                let out = DataChangeEvent {
                    client_handle: ev.client_handle,
                    data_value: ev.data_value,
                };
                let _ = tx.try_send(out);
            }
        });
        Ok((sub.id, rx))
    }

    async fn unsubscribe(&self, subscription_id: u32) -> Result<(), String> {
        // 按序清理：先删项再删订阅，均为幂等 Ok（BadSubscriptionIdInvalid/会话关闭即成功）
        let ids = self
            .items
            .lock()
            .unwrap()
            .remove(&subscription_id)
            .unwrap_or_default();
        if !ids.is_empty() {
            self.transport
                .delete_monitored_items(subscription_id, &ids)
                .await
                .map_err(|e| e.to_string())?;
        }
        self.transport
            .delete_subscription(subscription_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn browse(&self, node: &OpcUaAddress) -> Result<Vec<String>, String> {
        let page = self
            .transport
            .browse(UaBrowseRequest {
                node: addr_to_node(node),
                max_refs: 100,
            })
            .await
            .map_err(|e| e.to_string())?;
        // 保持旧字符串形态 `"<browse_name> <node_id>"`，上层过滤/分页逻辑不变
        Ok(page
            .nodes
            .into_iter()
            .map(|n| format!("{} {}", n.browse_name, n.node_id))
            .collect())
    }
}
