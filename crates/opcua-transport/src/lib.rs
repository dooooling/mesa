//! Mesa OPC UA 公共协议传输层（Stage 2 P0-B）。
//!
//! 架构：
//! ```text
//! async-opcua
//!      ↓
//! mesa-opcua-transport（本 crate：Session/Read/Browse/Namespace/Subscription）
//!      ↓
//! ┌────────────┴────────────┐
//! generic opcua          sinumerik
//! ```
//!
//! 边界冻结：本 crate 只知道 OPC UA，不知道 Mesa Point / DriverBinding /
//! ResourceSelection / SINUMERIK / ConfigStore / CertStore。数据语义
//! （Quality / ValueOrigin / LastKnown / typed placeholder）由上层 Driver
//! Adapter（`decode_data_value()`）负责，本层只做"服务器给了什么→原样结构化交出去"。
//! PKI 由调用方经 [`OpcUaConnectOptions::pki_dir`] 注入，本 crate 绝不读取
//! `MESA_OPCUA_PKI_DIR` 环境变量。

pub mod error;
pub mod fake;
pub mod native;
pub mod types;

pub use error::{UaOperation, UaTransportError, UaTransportErrorKind, map_service_error};
pub use fake::{FakeLiveBatch, FakeOpcUaTransport, fake_browse_node};
pub use native::{DEFAULT_OPCUA_PORT, NativeOpcUaTransport};
pub use types::{
    OpcUaConnectOptions, SubscriptionStats, UaBrowseNode, UaBrowsePage, UaBrowseRequest,
    UaDataChange, UaDataValue, UaIdentifier, UaMonitoredItemId, UaMonitoredItemResult,
    UaMonitoredItemSpec, UaNodeClass, UaNodeRef, UaSubscription, UaSubscriptionId,
    UaSubscriptionSpec,
};

/// 公共 OPC UA 传输抽象（V1.2.1 一次冻结）。
///
/// 生命周期分裂：`create_subscription` 只建订阅，`create_monitored_items` 独立建项，
/// 禁止旧 `subscribe(addrs...)` 一步包办。单点 BAD 以 [`UaDataValue`] 的 status
/// 逐点返回，只有整包 Service / Session 失败才 `Err`。
#[async_trait::async_trait]
pub trait OpcUaTransport: Send + Sync {
    async fn connect(&self) -> Result<(), UaTransportError>;
    async fn disconnect(&self) -> Result<(), UaTransportError>;

    /// 批量读：按传入顺序返回等长 [`UaDataValue`]，保留原生 StatusCode/SourceTimestamp。
    async fn read(&self, nodes: &[UaNodeRef]) -> Result<Vec<UaDataValue>, UaTransportError>;

    /// 单层浏览：一次调用只展开一层并返回本页；若 `page.continuation_point` 为 Some，
    /// 调用方必须用 [`OpcUaTransport::browse_next`] 继续取页（OPC UA 分页是 opaque
    /// token 接力，不是重复 Browse）。`has_children` 恒为 None（禁 N+1）。
    async fn browse(&self, request: UaBrowseRequest) -> Result<UaBrowsePage, UaTransportError>;

    /// 用 continuation point 取下一页；取完（返回 None）后建议调
    /// [`OpcUaTransport::release_continuation`]（已取完时服务端多已自动释放，幂等）。
    async fn browse_next(
        &self,
        continuation_point: Vec<u8>,
    ) -> Result<UaBrowsePage, UaTransportError>;

    /// 释放服务端 continuation 资源（`BadContinuationPointInvalid` 等幂等 Ok）。
    async fn release_continuation(
        &self,
        continuation_point: Vec<u8>,
    ) -> Result<(), UaTransportError>;

    /// 读取 NamespaceArray（ns=0;i=2255），供 URI→index 运行时解析。
    async fn read_namespace_array(&self) -> Result<Vec<String>, UaTransportError>;

    /// 仅建订阅：返回 Server Revised 的 publishing/lifetime/keep-alive。
    async fn create_subscription(
        &self,
        spec: UaSubscriptionSpec,
    ) -> Result<UaSubscription, UaTransportError>;

    /// 独立建监控项：逐项返回 status + revised sampling/queue，部分失败不整体 Err。
    async fn create_monitored_items(
        &self,
        subscription_id: UaSubscriptionId,
        items: &[UaMonitoredItemSpec],
    ) -> Result<Vec<UaMonitoredItemResult>, UaTransportError>;

    /// 删监控项：`BadSubscriptionIdInvalid/BadMonitoredItemIdInvalid/会话已关闭` 幂等 Ok。
    async fn delete_monitored_items(
        &self,
        subscription_id: UaSubscriptionId,
        ids: &[UaMonitoredItemId],
    ) -> Result<(), UaTransportError>;

    /// 删订阅：本地无该订阅 / 对端已无 / 会话已关闭均幂等 Ok（仅 cleanup 路径）。
    async fn delete_subscription(&self, id: UaSubscriptionId) -> Result<(), UaTransportError>;
}
