//! OPC UA 稳定资源身份：canonical `nsu=<uri>;<i|s|g|b>=<id>`。
//!
//! 背景：`ns=<index>` 是会话局部的命名空间索引——设备重启、配置变化后同一
//! URI 可能落到不同 index。若把 `ns=2;s=X` 直接当资源身份持久化，重启后同一
//! 物理量会被误认成新点（point_id 漂移），历史、UI 配置、Event 全被拖累。
//!
//! 因此持久化身份一律 canonical：
//! ```text
//! nsu=<namespace-uri>;s=<id>   字符串型
//! nsu=<namespace-uri>;i=<id>   数值型
//! nsu=<namespace-uri>;g=<guid> GUID 型
//! nsu=<namespace-uri>;b=<b64>  Opaque 型
//! ```
//! - 配置边界只接受 `nsu=`（`ns=` 一律拒绝并指引去 browse 拿 canonical）；
//! - 运行期每次建会话后用新鲜 NamespaceArray 把 URI 换算回当前 index（漂移自愈，
//!   canonical 不变 → Core 的 point_id 不漂）；
//! - browse 只产出 `nsu=`（子节点 index 经当次快照换算，越界项跳过并告警，不杀整页）。
//!
//! 命名空间 URI 含 `;` 的设备不被支持（Siemens 系 URI 无此形态，拒绝时明示）。

use crate::types::{UaIdentifier, UaNodeRef};

/// OPC UA 基础命名空间 URI（NamespaceArray[0] 恒为此值，Part 6 固定，不可漂移）。
pub const OPC_BASE_NAMESPACE_URI: &str = "http://opcfoundation.org/UA/";

/// Canonical 资源身份：命名空间 URI + 标识符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpcUaNodeId {
    /// 命名空间 URI（`nsu=` 的值，原样保留大小写）。
    pub namespace_uri: String,
    pub identifier: UaIdentifier,
    /// 用户输入原文（诊断回显）。
    pub raw: String,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CanonicalError {
    #[error("空地址")]
    Empty,
    #[error("非法地址 `{input}`: {reason}")]
    Invalid { input: String, reason: String },
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum ResolveError {
    /// URI 不在当前 NamespaceArray 中（设备侧命名空间变化，fail-closed）。
    #[error("命名空间 URI `{uri}` 不在设备 NamespaceArray（{count} 项）中")]
    UnknownNamespace { uri: String, count: usize },
    /// NamespaceArray 项数超出 u16（理论上不可能；静默 wrap 会指向错误命名空间）。
    #[error("NamespaceArray 过大（{count} 项），无法换算 index")]
    TooManyNamespaces { count: usize },
}

/// 解析 canonical 节点身份（`nsu=<uri>;<i|s|g|b>=<v>`）。
///
/// 大小写容忍（`NSU=`/`S=` 亦可）、首尾空格容忍；`ns=<n>;...` legacy 形态一律
/// 拒绝（reason 指引改用 browse 产出的 canonical 身份）。
pub fn parse_canonical(input: &str) -> Result<OpcUaNodeId, CanonicalError> {
    let raw_input = input.trim();
    if raw_input.is_empty() {
        return Err(CanonicalError::Empty);
    }
    // 先判断是否为 legacy ns= 形态，给出针对性指引。
    let lowered = raw_input.to_ascii_lowercase();
    if lowered.starts_with("ns=") || lowered.starts_with("ns =") {
        return Err(CanonicalError::Invalid {
            input: raw_input.to_string(),
            reason: "须用 canonical 身份 `nsu=<namespace-uri>;<i|s|g|b>=<id>`（经 browse 获取）；`ns=<index>` 索引重启后可能漂移，不接受".into(),
        });
    }
    // nsu= 前缀（大小写不敏感），URI 体内不允许 ';'（与 identifier 分隔符冲突）。
    let rest = if raw_input.len() > 4 && raw_input[..4].eq_ignore_ascii_case("nsu=") {
        &raw_input[4..]
    } else {
        return Err(CanonicalError::Invalid {
            input: raw_input.to_string(),
            reason: "须以 `nsu=<namespace-uri>;` 开头，如 nsu=http://www.siemens.com/sinumerik;s=Channel.State".into(),
        });
    };
    let (uri_part, id_part) = rest
        .split_once(';')
        .ok_or_else(|| CanonicalError::Invalid {
            input: raw_input.to_string(),
            reason: "缺少 `;<i|s|g|b>=<id>` 部分，如 nsu=<uri>;s=MyVar".into(),
        })?;
    let uri = uri_part.trim();
    if uri.is_empty() {
        return Err(CanonicalError::Invalid {
            input: raw_input.to_string(),
            reason: "nsu= 的命名空间 URI 不能为空".into(),
        });
    }
    let (kind, val) = id_part
        .split_once('=')
        .ok_or_else(|| CanonicalError::Invalid {
            input: raw_input.to_string(),
            reason: format!("标识符 `{id_part}` 需含 =，如 i=42 或 s=MyVar"),
        })?;
    let val = val.trim();
    if val.is_empty() {
        return Err(CanonicalError::Invalid {
            input: raw_input.to_string(),
            reason: format!("`{}` 的值不能为空", kind.trim()),
        });
    }
    let identifier = match kind.trim().to_ascii_lowercase().as_str() {
        "i" => {
            let n: u32 = val.parse().map_err(|_| CanonicalError::Invalid {
                input: raw_input.to_string(),
                reason: format!("i `{val}` 非法，需无符号整数"),
            })?;
            UaIdentifier::Numeric(n)
        }
        "s" => UaIdentifier::String(val.to_string()),
        "g" => {
            // 真 GUID 校验（失败即配置期错误）。
            let guid: opcua_types::Guid = val.parse().map_err(|_| CanonicalError::Invalid {
                input: raw_input.to_string(),
                reason: format!("g GUID `{val}` 非法，需 8-4-4-4-12 十六进制"),
            })?;
            UaIdentifier::Guid(guid.to_string())
        }
        "b" => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(val)
                .map_err(|_| CanonicalError::Invalid {
                    input: raw_input.to_string(),
                    reason: format!("b Opaque `{val}` 非法，需 Base64 字符"),
                })?;
            UaIdentifier::Opaque(base64::engine::general_purpose::STANDARD.encode(&bytes))
        }
        _ => {
            return Err(CanonicalError::Invalid {
                input: raw_input.to_string(),
                reason: format!("未知标识符类型 `{}`，期望 i/s/g/b", kind.trim()),
            });
        }
    };
    Ok(OpcUaNodeId {
        namespace_uri: uri.to_string(),
        identifier,
        raw: raw_input.to_string(),
    })
}

impl std::fmt::Display for OpcUaNodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.identifier {
            UaIdentifier::Numeric(n) => write!(f, "nsu={};i={n}", self.namespace_uri),
            UaIdentifier::String(s) => write!(f, "nsu={};s={s}", self.namespace_uri),
            UaIdentifier::Guid(g) => write!(f, "nsu={};g={g}", self.namespace_uri),
            UaIdentifier::Opaque(b) => write!(f, "nsu={};b={b}", self.namespace_uri),
        }
    }
}

impl OpcUaNodeId {
    /// 稳定身份键：canonical 字符串本身（Core point_key / binding 持久化用它）。
    pub fn canonical_key(&self) -> String {
        self.to_string()
    }

    /// 运行期换算：URI → 当前会话的 namespace index（每次建会话后用新鲜
    /// NamespaceArray 调用；index 漂移时换算结果自愈，canonical 不变）。
    pub fn resolve(&self, namespaces: &[String]) -> Result<UaNodeRef, ResolveError> {
        let index = namespaces
            .iter()
            .position(|u| u == &self.namespace_uri)
            .ok_or_else(|| ResolveError::UnknownNamespace {
                uri: self.namespace_uri.clone(),
                count: namespaces.len(),
            })?;
        // checked conversion：静默 wrap 会指向错误命名空间，fail-closed。
        let namespace = u16::try_from(index).map_err(|_| ResolveError::TooManyNamespaces {
            count: namespaces.len(),
        })?;
        Ok(UaNodeRef {
            namespace,
            identifier: self.identifier.clone(),
        })
    }

    /// transport 侧 index 身份 → canonical（browse 产出路径；index 越界返回 None）。
    pub fn from_index_ref(node: &UaNodeRef, namespaces: &[String]) -> Option<Self> {
        let uri = namespaces.get(node.namespace as usize)?.clone();
        let id = Self {
            namespace_uri: uri,
            identifier: node.identifier.clone(),
            raw: String::new(),
        };
        let raw = id.to_string();
        Some(Self { raw, ..id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_URI: &str = "http://example.com/MyModel/";
    const STD_URI: &str = OPC_BASE_NAMESPACE_URI;

    #[test]
    fn canonical_roundtrip_all_kinds() {
        for s in [
            format!("nsu={TEST_URI};s=Motor.Speed"),
            format!("nsu={TEST_URI};i=1234"),
            format!("nsu={TEST_URI};g=8A0A1B2C-3D4E-5F60-7080-90A0B0C0D0E0"),
            format!("nsu={TEST_URI};b=AQID"),
            format!("nsu={STD_URI};i=85"),
        ] {
            let id = parse_canonical(&s).unwrap_or_else(|e| panic!("parse {s} failed: {e}"));
            let back = id.canonical_key();
            let again = parse_canonical(&back).expect("canonical 必须可往返");
            assert_eq!(id.canonical_key(), again.canonical_key(), "往返不一致: {s}");
        }
    }

    #[test]
    fn legacy_ns_form_rejected_with_guidance() {
        for s in ["ns=2;s=X", "ns=2;i=1", "NS=0;I=85", "i=42", "s=MyVar", ""] {
            let err = parse_canonical(s).unwrap_err();
            match err {
                CanonicalError::Empty => assert!(s.trim().is_empty()),
                CanonicalError::Invalid { reason, .. } => {
                    assert!(
                        reason.contains("nsu=") || reason.contains("canonical"),
                        "reason 须指引 canonical，实际: {reason}"
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_shapes_rejected() {
        assert!(parse_canonical("nsu=;s=X").is_err()); // 空 URI
        assert!(parse_canonical("nsu=uri").is_err()); // 缺 identifier
        assert!(parse_canonical("nsu=uri;x=1").is_err()); // 未知类型
        assert!(parse_canonical("nsu=uri;i=abc").is_err());
        assert!(parse_canonical("nsu=uri;g=not-a-guid").is_err());
        assert!(parse_canonical("nsu=uri;b=$$$").is_err());
        assert!(parse_canonical("nsu=uri;s=").is_err());
        assert!(parse_canonical("nsu=a;buri;s=X").is_err()); // URI 含分号
    }

    #[test]
    fn index_shift_keeps_canonical_key_stable() {
        // 同一 URI 在 index 2 与 index 4 → 同一 canonical，
        // 运行期换算出不同的当前 index，但 canonical_key 不变。
        let id = parse_canonical(&format!("nsu={TEST_URI};s=Motor.Speed")).expect("parse ok");
        let ns_before = vec![
            STD_URI.to_string(),
            "urn:other".to_string(),
            TEST_URI.to_string(),
        ];
        let ns_after = vec![
            STD_URI.to_string(),
            "urn:a".to_string(),
            "urn:b".to_string(),
            "urn:c".to_string(),
            TEST_URI.to_string(),
        ];
        let ref_before = id.resolve(&ns_before).expect("resolve before");
        let ref_after = id.resolve(&ns_after).expect("resolve after");
        assert_eq!(ref_before.namespace, 2);
        assert_eq!(ref_after.namespace, 4);
        assert_ne!(ref_before, ref_after);
        assert_eq!(id.canonical_key(), format!("nsu={TEST_URI};s=Motor.Speed"));
        // browse 逆向：index 身份 → 同一 canonical
        let back = OpcUaNodeId::from_index_ref(&ref_after, &ns_after).expect("from_index_ref");
        assert_eq!(back.canonical_key(), id.canonical_key());
    }

    #[test]
    fn unknown_namespace_fails_closed() {
        let id = parse_canonical(&format!("nsu={TEST_URI};i=1")).expect("parse ok");
        let err = id
            .resolve(&[STD_URI.to_string()])
            .expect_err("未知 URI 必须 fail-closed");
        assert!(err.to_string().contains(TEST_URI));
        // index 越界的 browse 子节点换算 → None（调用方跳过并告警，不杀整页）
        let stray = UaNodeRef::numeric(9, 1);
        assert!(OpcUaNodeId::from_index_ref(&stray, &[STD_URI.to_string()]).is_none());
    }
}
