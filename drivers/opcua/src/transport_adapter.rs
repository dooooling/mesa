//! Transport 适配器（Stage 2 P0-B）：把公共 [`OpcUaTransport`] 桥接到旧 [`OpcUaApi`] trait。
//!
//! 作用：`run()` / `decode_data_value()` / Fake 路径零改动，只有 Native 会话实现被替换。
//! 数据语义仍在 `decode_data_value()`（本 driver），transport 只交出原生 DataValue。
//! 订阅分裂生命周期在此收敛：`subscribe()` 内依次 `create_subscription` +
//! `create_monitored_items`（失败回滚删订阅）；`unsubscribe()` 内依次删项 + 删订阅（均幂等）。
//! Browse 在此聚合 continuation 多页，上层仍看到完整一层。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mesa_opcua_transport::{
    NativeOpcUaTransport, OpcUaConnectOptions, OpcUaTransport, UaBrowseRequest, UaIdentifier,
    UaMonitoredItemSpec, UaNodeRef, UaSubscriptionSpec,
};
use opcua_types::StatusCode;

use super::opcua_api::{DataChangeEvent, OpcUaApi};
use crate::address::{Identifier, OpcUaAddress};

/// 单页最大引用数（命名常量：服务端仍可按自身上限截断并返回 continuation，
/// 本 adapter 用 continuation 接力取全页，见 `browse()`）。
const BROWSE_MAX_REFS_PER_PAGE: u32 = 1000;

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
/// 泛型 `T` 便于测试注入 [`mesa_opcua_transport::FakeOpcUaTransport`]；生产用默认
/// [`NativeOpcUaTransport`]。
pub struct TransportApiAdapter<T: OpcUaTransport = NativeOpcUaTransport> {
    transport: Arc<T>,
    /// sub_id -> 已建成功的 monitored_item_id（供 unsubscribe 按序清理）。
    items: Mutex<HashMap<u32, Vec<u32>>>,
}

impl TransportApiAdapter<NativeOpcUaTransport> {
    pub fn new(options: OpcUaConnectOptions) -> Self {
        Self {
            transport: Arc::new(NativeOpcUaTransport::new(options)),
            items: Mutex::new(HashMap::new()),
        }
    }
}

impl<T: OpcUaTransport> TransportApiAdapter<T> {
    pub fn with_transport(transport: Arc<T>) -> Self {
        Self {
            transport,
            items: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl<T: OpcUaTransport> OpcUaApi for TransportApiAdapter<T> {
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
        let results = match self
            .transport
            .create_monitored_items(sub.id, &mi_specs)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // P0-B3.1：服务级失败必须回滚——删掉刚建的空订阅，避免 Server 侧泄漏，
                // 再把原始错误上抛（cleanup 自身失败仅诊断，不掩盖原始错误）。
                if let Err(cleanup) = self.transport.delete_subscription(sub.id).await {
                    tracing::debug!(
                        sub_id = sub.id,
                        ?cleanup,
                        "订阅创建失败后回滚删订阅失败（仅诊断）"
                    );
                }
                return Err(e.to_string());
            }
        };
        let mut ok_ids = Vec::with_capacity(results.len());
        // P0-B2：单项 BAD 立即合成初始 BAD 事件——失败项永不到达 live 流，
        // 不合成则该点在首个 live 事件前处于"无值"而非 LastKnown/Placeholder。
        let mut initial_bads = Vec::new();
        for r in &results {
            let status = StatusCode::from(r.status_code);
            if status.is_good() {
                ok_ids.push(r.monitored_item_id);
            } else {
                tracing::warn!(
                    client_handle = r.client_handle,
                    status = ?status,
                    "监控项创建单项失败：合成初始 BAD 事件并隔离该项"
                );
                initial_bads.push(DataChangeEvent {
                    client_handle: r.client_handle,
                    data_value: opcua_types::DataValue {
                        value: None,
                        status: Some(status),
                        source_timestamp: None,
                        source_picoseconds: None,
                        server_timestamp: None,
                        server_picoseconds: None,
                    },
                });
            }
        }
        self.items.lock().unwrap().insert(sub.id, ok_ids);
        // 转发：UaDataChange → DataChangeEvent。先发初始 BAD（有序在前），再转 live；
        // `send().await` 背压等待——transport 侧已 Latest-Wins，adapter 不再丢最新。
        let mut sub_rx = sub.receiver;
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            for bad in initial_bads {
                if tx.send(bad).await.is_err() {
                    return;
                }
            }
            while let Some(ev) = sub_rx.recv().await {
                let out = DataChangeEvent {
                    client_handle: ev.client_handle,
                    data_value: ev.data_value,
                };
                if tx.send(out).await.is_err() {
                    return;
                }
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
        // P0-B4：continuation 接力取全页——首 browse + browse_next 直至 token 为空；
        // 消费过的旧 token  best-effort 释放（失败仅诊断，不中断翻页）。
        let mut out = Vec::new();
        let mut page = self
            .transport
            .browse(UaBrowseRequest {
                node: addr_to_node(node),
                max_refs: BROWSE_MAX_REFS_PER_PAGE,
            })
            .await
            .map_err(|e| e.to_string())?;
        loop {
            out.extend(
                page.nodes
                    .into_iter()
                    .map(|n| format!("{} {}", n.browse_name, n.node_id)),
            );
            let Some(token) = page.continuation_point else {
                break;
            };
            let next = match self.transport.browse_next(token.clone()).await {
                Ok(p) => p,
                Err(e) => {
                    if let Err(rel) = self.transport.release_continuation(token).await {
                        tracing::debug!(?rel, "翻页失败后释放 continuation 失败（仅诊断）");
                    }
                    return Err(e.to_string());
                }
            };
            if let Err(rel) = self.transport.release_continuation(token).await {
                tracing::debug!(?rel, "释放已消费 continuation 失败（仅诊断）");
            }
            page = next;
        }
        // 保持旧字符串形态 `"<browse_name> <node_id>"`，上层过滤/分页逻辑不变
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_opcua_transport::{FakeLiveBatch, FakeOpcUaTransport, UaBrowsePage, fake_browse_node};
    use opcua_types::{DataValue, Variant};

    fn addr(ns: u16, id: u32) -> OpcUaAddress {
        OpcUaAddress {
            namespace: ns,
            identifier: Identifier::Numeric(id),
            raw: format!("ns={ns};i={id}"),
        }
    }

    fn node(ns: u16, id: u32) -> UaNodeRef {
        UaNodeRef::numeric(ns, id)
    }

    #[tokio::test]
    async fn partial_bad_item_synthesizes_initial_bad_and_rolls_nothing_on_success() {
        let fake = FakeOpcUaTransport::new()
            .with_create_status(&node(2, 2), StatusCode::BadNodeIdUnknown)
            .with_live_batch(FakeLiveBatch {
                events: vec![(1, DataValue::new_now(42i32))],
            });
        let fake = Arc::new(fake);
        let adapter = TransportApiAdapter::with_transport(fake.clone());

        let (sub_id, mut rx) = adapter
            .subscribe(&[addr(2, 1), addr(2, 2)], 500, 250, 10, true)
            .await
            .expect("部分 BAD 不整体失败");
        // 首事件必须是对失败项的合成 BAD（有序在 live 之前）
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("首事件超时")
            .expect("通道不断");
        assert_eq!(first.client_handle, 2);
        assert_eq!(first.data_value.status, Some(StatusCode::BadNodeIdUnknown));
        // 次事件为 live 好值
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("次事件超时")
            .expect("通道不断");
        assert_eq!(second.client_handle, 1);
        assert_eq!(second.data_value.value, Some(Variant::Int32(42)));

        // unsubscribe 按序清理：只删建成功的项 + 删订阅
        adapter.unsubscribe(sub_id).await.expect("cleanup Ok");
        let items = fake.deleted_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, sub_id);
        assert_eq!(items[0].1.len(), 1, "仅 Good 项进入删除清单");
        assert_eq!(fake.deleted_subscriptions(), vec![sub_id]);
    }

    #[tokio::test]
    async fn browse_aggregates_continuation_pages_and_releases_token() {
        let token = vec![0xC0u8, 0xFFu8, 0xEEu8];
        let pages = vec![
            UaBrowsePage {
                nodes: vec![fake_browse_node(node(2, 10), "Alpha")],
                continuation_point: Some(token.clone()),
            },
            UaBrowsePage {
                nodes: vec![fake_browse_node(node(2, 11), "Beta")],
                continuation_point: None,
            },
        ];
        let fake = Arc::new(FakeOpcUaTransport::new().with_browse_pages(&node(1, 85), pages));
        let adapter = TransportApiAdapter::with_transport(fake.clone());

        let out = adapter.browse(&addr(1, 85)).await.expect("翻页聚合 Ok");
        assert_eq!(out.len(), 2);
        assert!(out[0].starts_with("Alpha "));
        assert!(out[1].starts_with("Beta "));
        assert_eq!(fake.released_continuations(), vec![token]);
    }

    #[tokio::test]
    async fn read_batch_passes_per_point_bad_without_err() {
        let fake =
            Arc::new(FakeOpcUaTransport::new().with_read(&node(2, 1), DataValue::new_now(7i32)));
        let adapter = TransportApiAdapter::with_transport(fake);
        let values = adapter
            .read_batch(&[addr(2, 1), addr(2, 999)])
            .await
            .expect("单点 BAD 不整体 Err");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].value, Some(Variant::Int32(7)));
        assert_eq!(values[1].status, Some(StatusCode::BadNodeIdUnknown));
    }
}
