//! OPC UA 地址（Commit A：canonical `nsu=` 契约，方案 §7.3 节点型）。
//!
//! - 配置边界（`configure`/`browse parent`/`event notifier`）只接受 canonical
//!   `nsu=<uri>;<i|s|g|b>=<id>`，`ns=<index>` 一律拒绝（索引漂移导致 point_id
//!   漂移，见 transport `node_ref`）；
//! - 运行期 index 在每次建会话后用新鲜 NamespaceArray 解析（`resolve`），
//!   漂移自愈，canonical 不变；未知 URI 即 fail-closed，不静默回退；
//! - Fake 路径不消费 index（`namespace: None` 照常工作），Native 路径必须先
//!   `resolve`（adapter 内 `None` 即编程错误，fail-closed）。
//!
//! Core 不触及此文件（硬性约束）。

pub use mesa_opcua_transport::{OpcUaNodeId, ResolveError, UaIdentifier as Identifier, UaNodeRef};

/// 驱动侧地址：canonical 身份 + 当前会话解析出的 index。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpcUaAddress {
    /// canonical 身份（configure 期确定，持久稳定）。
    pub node: OpcUaNodeId,
    /// 当前会话的 namespace index（run 期 `resolve` 后为 `Some`）。
    pub namespace: Option<u16>,
    /// 用户输入原文（诊断回显）。
    pub raw: String,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum AddressError {
    #[error("空地址")]
    Empty,
    #[error("非法地址 `{input}`: {reason}")]
    Invalid { input: String, reason: String },
}

impl From<mesa_opcua_transport::CanonicalError> for AddressError {
    fn from(e: mesa_opcua_transport::CanonicalError) -> Self {
        match e {
            mesa_opcua_transport::CanonicalError::Empty => AddressError::Empty,
            mesa_opcua_transport::CanonicalError::Invalid { input, reason } => {
                AddressError::Invalid { input, reason }
            }
        }
    }
}

/// 解析 canonical 地址（只接受 `nsu=`；`ns=` legacy 一律拒绝并指引 browse）。
pub fn parse_address(input: &str) -> Result<OpcUaAddress, AddressError> {
    let node = mesa_opcua_transport::parse_canonical(input)?;
    Ok(OpcUaAddress {
        raw: node.raw.clone(),
        node,
        namespace: None,
    })
}

impl OpcUaAddress {
    /// 运行期换算：canonical → 当前会话 index（成功后 `namespace` 置 `Some`）。
    pub fn resolve(&mut self, namespaces: &[String]) -> Result<UaNodeRef, ResolveError> {
        let r = self.node.resolve(namespaces)?;
        self.namespace = Some(r.namespace);
        Ok(r)
    }

    /// 取当前会话 index（未 `resolve` 即编程错误，fail-closed）。
    pub fn to_node_ref(&self) -> Result<UaNodeRef, String> {
        let namespace = self.namespace.ok_or_else(|| {
            format!(
                "节点 `{}` 未解析 namespace index（run 期必须先 resolve）",
                self.node.canonical_key()
            )
        })?;
        Ok(UaNodeRef {
            namespace,
            identifier: self.node.identifier.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URI: &str = "http://example.com/MyModel/";

    #[test]
    fn canonical_accepted_unresolved_by_default() {
        let a = parse_address(&format!("nsu={URI};s=Motor.Speed")).expect("canonical Ok");
        assert_eq!(a.namespace, None);
        assert_eq!(a.node.namespace_uri, URI);
        assert!(a.to_node_ref().is_err(), "未 resolve 即 fail-closed");
    }

    #[test]
    fn resolve_fills_index_and_shifts_heal() {
        let mut a = parse_address(&format!("nsu={URI};i=2")).expect("parse ok");
        let ns = vec![
            mesa_opcua_transport::OPC_BASE_NAMESPACE_URI.to_string(),
            URI.to_string(),
        ];
        let r = a.resolve(&ns).expect("resolve ok");
        assert_eq!(r.namespace, 1);
        assert_eq!(a.namespace, Some(1));
        assert_eq!(a.to_node_ref().unwrap(), r);
        // 未知 URI fail-closed，不污染已解析值
        assert!(a.resolve(&["http://other/".to_string()]).is_err());
        assert_eq!(a.namespace, Some(1));
    }

    #[test]
    fn legacy_ns_rejected_with_guidance() {
        for s in ["ns=2;s=X", "ns=2;i=1", "NS=0;I=85", "i=42", ""] {
            let err = parse_address(s).expect_err(&format!("必须拒绝: {s:?}"));
            match err {
                AddressError::Empty => assert!(s.trim().is_empty()),
                AddressError::Invalid { reason, .. } => {
                    assert!(
                        reason.contains("nsu=") || reason.contains("canonical"),
                        "须指引 canonical，实际: {reason}"
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_shapes_rejected() {
        assert!(parse_address("nsu=;s=X").is_err());
        assert!(parse_address("nsu=uri;x=1").is_err());
        assert!(parse_address("nsu=uri;i=abc").is_err());
        assert!(parse_address("nsu=uri;g=not-a-guid").is_err());
        assert!(parse_address("nsu=uri;b=$$$").is_err());
    }
}
