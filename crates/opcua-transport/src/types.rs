//! Transport 公共类型（Stage 2 P0-B 冻结）。
//!
//! 边界：只知道 OPC UA，不知道 Mesa Point / DriverBinding / ResourceSelection /
//! SINUMERIK / ConfigStore / CertStore。`decode_data_value()`（Quality / ValueOrigin /
//! LastKnown / typed placeholder）保留在 Generic OPC UA Driver Adapter，不进本 crate。

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// 连接选项：PKI 由调用方注入，本 crate 绝不读取 MESA_OPCUA_PKI_DIR
// ---------------------------------------------------------------------------

/// 建连选项：调用方（mesa-driver-opcua / 未来 sinumerik）负责从环境或配置解析
/// `pki_dir` 后传入；transport 只使用传入值。
#[derive(Debug, Clone)]
pub struct OpcUaConnectOptions {
    pub endpoint_url: String,
    pub timeout_ms: u64,
    pub security_policy: String,
    pub security_mode: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// 客户端 PKI 目录（含 own/own.der + own/own.key + trust/ + rejected/）。
    pub pki_dir: PathBuf,
    pub application_name: String,
    pub application_uri: String,
}

impl Default for OpcUaConnectOptions {
    fn default() -> Self {
        Self {
            endpoint_url: "opc.tcp://127.0.0.1:4840".into(),
            timeout_ms: 5000,
            security_policy: "None".into(),
            security_mode: "None".into(),
            username: None,
            password: None,
            pki_dir: PathBuf::from("data/certificates/opcua"),
            application_name: "Mesa OPC UA".into(),
            application_uri: "urn:Mesa:opcua".into(),
        }
    }
}

impl OpcUaConnectOptions {
    /// 结构级校验（建连前即可判定，失败为 Configuration 且不可重试）。
    pub fn validate(&self) -> Result<(), crate::UaTransportError> {
        use crate::{UaOperation, UaTransportError};
        if self.endpoint_url.trim().is_empty() {
            return Err(UaTransportError::configuration(
                UaOperation::Connect,
                "endpoint_url 不能为空",
            ));
        }
        if !self.endpoint_url.starts_with("opc.tcp://") {
            return Err(UaTransportError::configuration(
                UaOperation::Connect,
                format!(
                    "endpoint_url `{}` 非法，需 opc.tcp://host:port",
                    self.endpoint_url
                ),
            ));
        }
        if self.timeout_ms == 0 {
            return Err(UaTransportError::configuration(
                UaOperation::Connect,
                "timeout_ms 需 >0",
            ));
        }
        const POLICIES: [&str; 6] = [
            "None",
            "Basic128Rsa15",
            "Basic256",
            "Basic256Sha256",
            "Aes128_Sha256_RsaOaep",
            "Aes256_Sha256_RsaPss",
        ];
        if !POLICIES.contains(&self.security_policy.as_str()) {
            return Err(UaTransportError::configuration(
                UaOperation::Connect,
                format!("security_policy `{}` 非法", self.security_policy),
            ));
        }
        const MODES: [&str; 3] = ["None", "Sign", "SignAndEncrypt"];
        if !MODES.contains(&self.security_mode.as_str()) {
            return Err(UaTransportError::configuration(
                UaOperation::Connect,
                format!("security_mode `{}` 非法", self.security_mode),
            ));
        }
        match (&self.username, &self.password) {
            (Some(_), None) | (None, Some(_)) => Err(UaTransportError::configuration(
                UaOperation::Connect,
                "username 与 password 需同时提供或同时为空；仅提供其一视为配置错误",
            )),
            _ => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// 节点引用：结构化 NodeId（namespace index + identifier），不承载 namespace URI
// ---------------------------------------------------------------------------

/// NodeId 标识体（与 `ns=<n>;<type>=<v>` 文本一一对应）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UaIdentifier {
    Numeric(u32),
    String(String),
    Guid(String),
    /// Base64 编码的 Opaque 字节。
    Opaque(String),
}

/// 结构化节点引用。URI→index 解析由 [`crate::OpcUaTransport::read_namespace_array`]
/// 在运行时完成；本结构只承载解析后的 index，绝不持久化 `ns=2` 假设。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UaNodeRef {
    pub namespace: u16,
    pub identifier: UaIdentifier,
}

impl UaNodeRef {
    pub fn numeric(namespace: u16, id: u32) -> Self {
        Self {
            namespace,
            identifier: UaIdentifier::Numeric(id),
        }
    }

    pub fn string(namespace: u16, id: impl Into<String>) -> Self {
        Self {
            namespace,
            identifier: UaIdentifier::String(id.into()),
        }
    }

    /// 解析 `ns=<n>;<i|s|g|b>=<v>` 文本（大小写/空格容忍，与 driver 侧保持一致）。
    pub fn parse(s: &str) -> Result<Self, String> {
        let t = s.trim();
        let (ns_part, id_part) = t
            .split_once(';')
            .ok_or_else(|| format!("NodeId `{s}` 非法，期望 ns=<n>;<i|s|g|b>=<v>"))?;
        let ns_str = ns_part
            .trim()
            .strip_prefix("ns=")
            .or_else(|| ns_part.trim().strip_prefix("NS="))
            .ok_or_else(|| format!("NodeId `{s}` 非法，缺少 ns="))?;
        let namespace: u16 = ns_str
            .trim()
            .parse()
            .map_err(|_| format!("NodeId `{s}` 非法，namespace 非法"))?;
        let id_part = id_part.trim();
        let (kind, val) = id_part
            .split_once('=')
            .ok_or_else(|| format!("NodeId `{s}` 非法，缺少 <type>=<v>"))?;
        match kind.trim().to_ascii_lowercase().as_str() {
            "i" => {
                let n: u32 = val
                    .trim()
                    .parse()
                    .map_err(|_| format!("NodeId `{s}` 非法，numeric 非法"))?;
                Ok(Self::numeric(namespace, n))
            }
            "s" => {
                let v = val.trim();
                if v.is_empty() {
                    return Err(format!("NodeId `{s}` 非法，string 为空"));
                }
                Ok(Self::string(namespace, v))
            }
            "g" => {
                let v = val.trim();
                if v.len() != 36 {
                    return Err(format!("NodeId `{s}` 非法，guid 长度非法"));
                }
                Ok(Self {
                    namespace,
                    identifier: UaIdentifier::Guid(v.to_string()),
                })
            }
            "b" => {
                let v = val.trim();
                if v.is_empty() {
                    return Err(format!("NodeId `{s}` 非法，opaque 为空"));
                }
                Ok(Self {
                    namespace,
                    identifier: UaIdentifier::Opaque(v.to_string()),
                })
            }
            _ => Err(format!("NodeId `{s}` 非法，未知类型 `{kind}`")),
        }
    }
}

impl std::fmt::Display for UaNodeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.identifier {
            UaIdentifier::Numeric(n) => write!(f, "ns={};i={n}", self.namespace),
            UaIdentifier::String(s) => write!(f, "ns={};s={s}", self.namespace),
            UaIdentifier::Guid(g) => write!(f, "ns={};g={g}", self.namespace),
            UaIdentifier::Opaque(b) => write!(f, "ns={};b={b}", self.namespace),
        }
    }
}

// ---------------------------------------------------------------------------
// 读 / 浏览
// ---------------------------------------------------------------------------

/// 单次 Read 的返回：直接透传原生 DataValue，保留 StatusCode / SourceTimestamp。
/// 单点 BAD 以 `status != Good` 的 DataValue 表达，绝不整体 Err。
pub type UaDataValue = opcua_types::DataValue;

/// Browse 请求：单次调用只做一层展开，禁止为每个子节点再 Browse（禁 N+1）。
#[derive(Debug, Clone)]
pub struct UaBrowseRequest {
    pub node: UaNodeRef,
    /// 本次最多返回引用数（0 表示 transport 默认，上限由实现截断）。
    pub max_refs: u32,
}

/// 节点类别（browse result_mask 的最小可用子集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UaNodeClass {
    Object,
    Variable,
    Method,
    Unknown,
}

/// Browse 返回的单个引用。
#[derive(Debug, Clone)]
pub struct UaBrowseNode {
    pub node_id: UaNodeRef,
    pub browse_name: String,
    pub display_name: Option<String>,
    pub node_class: UaNodeClass,
    /// 三态：Some(true) 确认有孩子 / Some(false) 确认无孩子 / None 未探测。
    /// 未探测时必须为 None，禁止为 UI 好看而发起 N+1 探测。
    pub has_children: Option<bool>,
}

/// Browse 单页结果：OPC UA 分页以服务端 opaque continuation point 推进，
/// 调用方必须循环 `browse` → `browse_next` 直至 `continuation_point` 为 None，
/// 结束（或放弃）后调用 `release_continuation` 释放服务端资源。
#[derive(Debug, Clone)]
pub struct UaBrowsePage {
    pub nodes: Vec<UaBrowseNode>,
    /// 服务端 opaque 翻页令牌；None 表示已取完。内容不得解析，仅透传。
    pub continuation_point: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// 订阅：Subscription 与 MonitoredItem 生命周期分裂，保留 Server Revised 参数
// ---------------------------------------------------------------------------

pub type UaSubscriptionId = u32;
pub type UaMonitoredItemId = u32;

/// 创建订阅的请求参数（均为"请求值"，实际以返回的 revised 为准）。
#[derive(Debug, Clone)]
pub struct UaSubscriptionSpec {
    pub publishing_interval_ms: u64,
    pub lifetime_count: u32,
    pub max_keep_alive_count: u32,
    pub max_notifications_per_publish: u32,
    pub priority: u8,
    pub publishing_enabled: bool,
}

impl Default for UaSubscriptionSpec {
    fn default() -> Self {
        Self {
            publishing_interval_ms: 500,
            lifetime_count: 30,
            max_keep_alive_count: 10,
            max_notifications_per_publish: 0,
            priority: 0,
            publishing_enabled: true,
        }
    }
}

/// 数据变更事件（Latest-Wins 语义：拥塞时旧采样可丢，最新采样永不被旧采样挤掉）。
#[derive(Debug)]
pub struct UaDataChange {
    pub client_handle: u32,
    pub data_value: UaDataValue,
}

/// 订阅事件诊断计数（P0-B1 Gate：队列可丢采样，但必须可观测丢了多少旧采样）。
#[derive(Debug, Default)]
pub struct SubscriptionStats {
    /// 回调收到的原始事件总数。
    pub events_received: std::sync::atomic::AtomicU64,
    /// 因同 handle 新值覆盖旧值而合并掉的旧采样数。
    pub events_coalesced: std::sync::atomic::AtomicU64,
}

/// 已创建订阅：保留 Server Revised 参数，供 Planner 做 grouping 诊断。
pub struct UaSubscription {
    pub id: UaSubscriptionId,
    pub requested_publishing_interval_ms: u64,
    pub revised_publishing_interval_ms: u64,
    pub revised_lifetime_count: u32,
    pub revised_max_keep_alive_count: u32,
    pub receiver: tokio::sync::mpsc::Receiver<UaDataChange>,
    pub stats: std::sync::Arc<SubscriptionStats>,
}

/// 单个受监控项的创建请求。
#[derive(Debug, Clone)]
pub struct UaMonitoredItemSpec {
    pub node: UaNodeRef,
    pub client_handle: u32,
    pub sampling_interval_ms: u64,
    pub queue_size: u32,
    pub discard_oldest: bool,
}

/// 单个受监控项的创建结果：逐项状态 + Revised 参数（部分失败不整体 Err）。
#[derive(Debug, Clone)]
pub struct UaMonitoredItemResult {
    pub client_handle: u32,
    pub monitored_item_id: UaMonitoredItemId,
    /// 逐项状态码（Good 方可用；BAD 表示该项未建成功，调用方按单点 BAD 隔离）。
    pub status_code: u32,
    pub requested_sampling_interval_ms: u64,
    pub revised_sampling_interval_ms: u64,
    pub requested_queue_size: u32,
    pub revised_queue_size: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ref_parse_roundtrip() {
        let n = UaNodeRef::parse("ns=2;i=85").unwrap();
        assert_eq!(n, UaNodeRef::numeric(2, 85));
        assert_eq!(n.to_string(), "ns=2;i=85");
        let s = UaNodeRef::parse("ns=2;s=Motor.Speed").unwrap();
        assert_eq!(s.to_string(), "ns=2;s=Motor.Speed");
        assert!(UaNodeRef::parse("ns=2;x=1").is_err());
        assert!(UaNodeRef::parse("bad").is_err());
    }

    #[test]
    fn connect_options_rejects_partial_credentials_and_bad_url() {
        let o = OpcUaConnectOptions {
            username: Some("u".into()),
            ..Default::default()
        };
        assert!(o.validate().is_err());
        let o = OpcUaConnectOptions {
            username: Some("u".into()),
            password: Some("p".into()),
            ..Default::default()
        };
        assert!(o.validate().is_ok());
        let o = OpcUaConnectOptions {
            endpoint_url: "http://x".into(),
            ..Default::default()
        };
        assert!(o.validate().is_err());
    }
}
