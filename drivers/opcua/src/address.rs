//! OPC UA NodeId 解析（方案 §7.3 节点型，Core 不懂协议硬约束）。
//!
//! 支持 OPC UA 标准 NodeId 文本表示（Part 4 §7.2）：
//! - `ns=2;i=1234` 数值型（Numeric）
//! - `ns=2;s=Motor.Speed` 字符串型（String）
//! - `ns=2;g=72962B91-FA75-4A99-8A64-03D6D025A2DA` GUID 型
//! - `ns=2;b=M/RbKBsRVkePCePcx24oRA==` 不透明型（Opaque，Base64）
//! - 省略 ns 默认为 `ns=0`：`i=2253` / `s=MyVar`
//! - 允许大小写不敏感、空格容忍、分号空格变体
//!
//! V1 只读，不涉及 NodeId 创建，仅解析与校验；非法一律在 `configure` 阶段拒绝。

use thiserror::Error;

// ---------------------------------------------------------------------------
// 常量（中文解释“为什么”）
// ---------------------------------------------------------------------------
/// OPC UA 命名空间索引为 u16（0..65535），0 为 OPC 基础命名空间
const MAX_NAMESPACE: u32 = 65535;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    Numeric(u32),
    String(String),
    Guid(String),
    Opaque(String), // Base64 原文
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpcUaAddress {
    /// 命名空间索引
    pub namespace: u16,
    /// 标识符
    pub identifier: Identifier,
    /// 原始字符串（用于诊断回显，保持用户输入大小写之外的规范化形式）
    pub raw: String,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum AddressError {
    #[error("空地址")]
    Empty,
    #[error("非法地址 `{input}`: {reason}")]
    Invalid { input: String, reason: String },
}

/// 解析 NodeId 字符串
pub fn parse_address(input: &str) -> Result<OpcUaAddress, AddressError> {
    let raw_input = input.trim();
    if raw_input.is_empty() {
        return Err(AddressError::Empty);
    }
    // 容忍空格：移除所有空白字符后再解析，但保留错误回显用原始 trimmed
    let s = raw_input.replace(' ', "");
    if s.is_empty() {
        return Err(AddressError::Empty);
    }
    let parts: Vec<&str> = s.split(';').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(AddressError::Invalid { input: raw_input.to_string(), reason: "格式需如 ns=2;i=1234 或 ns=2;s=MyVar".into() });
    }

    let mut namespace: Option<u16> = None;
    let mut identifier: Option<Identifier> = None;

    for part in parts {
        let (k, v) = part.split_once('=').ok_or_else(|| AddressError::Invalid { input: raw_input.to_string(), reason: format!("分段 `{part}` 需含 =，如 ns=2 或 i=42") })?;
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        if val.is_empty() {
            return Err(AddressError::Invalid { input: raw_input.to_string(), reason: format!("`{key}` 的值不能为空") });
        }
        match key.as_str() {
            "ns" => {
                if namespace.is_some() {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: "ns 重复".into() });
                }
                let n: u32 = val.parse().map_err(|_| AddressError::Invalid { input: raw_input.to_string(), reason: format!("ns `{val}` 非法，需 0..{MAX_NAMESPACE}") })?;
                if n > MAX_NAMESPACE {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: format!("ns 必须 0..{MAX_NAMESPACE}") });
                }
                namespace = Some(n as u16);
            }
            "i" => {
                if identifier.is_some() {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: "标识符重复（i/s/g/b 只能选一）".into() });
                }
                // Numeric 允许 u32，无符号
                let n: u32 = val.parse().map_err(|_| AddressError::Invalid { input: raw_input.to_string(), reason: format!("i `{val}` 非法，需无符号整数") })?;
                identifier = Some(Identifier::Numeric(n));
            }
            "s" => {
                if identifier.is_some() {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: "标识符重复（i/s/g/b 只能选一）".into() });
                }
                // String 标识符：保留原大小写（已去空格），但 OPC UA 字符串区分大小写
                // 这里 v 来自去空格后的 s，若用户原始含空格已归一化，属容忍行为
                if val.is_empty() {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: "s 字符串标识符不能为空".into() });
                }
                identifier = Some(Identifier::String(val.to_string()));
            }
            "g" => {
                if identifier.is_some() {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: "标识符重复（i/s/g/b 只能选一）".into() });
                }
                // GUID 校验：形如 72962B91-FA75-4A99-8A64-03D6D025A2DA（8-4-4-4-12 十六进制）
                if !is_valid_guid(val) {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: format!("g GUID `{val}` 非法，需 8-4-4-4-12 十六进制") });
                }
                identifier = Some(Identifier::Guid(val.to_string()));
            }
            "b" => {
                if identifier.is_some() {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: "标识符重复（i/s/g/b 只能选一）".into() });
                }
                // Opaque：要求 Base64 字符集，长度不限但不能为空
                if !is_valid_base64(val) {
                    return Err(AddressError::Invalid { input: raw_input.to_string(), reason: format!("b Opaque `{val}` 非法，需 Base64 字符") });
                }
                identifier = Some(Identifier::Opaque(val.to_string()));
            }
            _ => {
                return Err(AddressError::Invalid { input: raw_input.to_string(), reason: format!("未知分段 `{key}`，期望 ns/i/s/g/b") });
            }
        }
    }

    let ident = identifier.ok_or_else(|| AddressError::Invalid { input: raw_input.to_string(), reason: "缺少标识符（i/s/g/b 需选其一），如 ns=2;i=1234".into() })?;
    let ns = namespace.unwrap_or(0);

    Ok(OpcUaAddress { namespace: ns, identifier: ident, raw: raw_input.to_string() })
}

fn is_valid_guid(s: &str) -> bool {
    // 8-4-4-4-12 且均为 hex
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 { return false; }
    let lens = [8, 4, 4, 4, 12];
    for (p, &exp) in parts.iter().zip(lens.iter()) {
        if p.len() != exp { return false; }
        if !p.chars().all(|c| c.is_ascii_hexdigit()) { return false; }
    }
    true
}

fn is_valid_base64(s: &str) -> bool {
    if s.is_empty() { return false; }
    // 允许 A-Za-z0-9+/= 填充
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str, ns: u16, ident: Identifier) {
        let a = parse_address(s).unwrap_or_else(|e| panic!("parse {s} failed: {e}"));
        assert_eq!(a.namespace, ns, "ns mismatch for {s}");
        assert_eq!(a.identifier, ident, "ident mismatch for {s}");
    }

    #[test]
    fn numeric_forms() {
        ok("ns=2;i=1234", 2, Identifier::Numeric(1234));
        ok("ns=0;i=2253", 0, Identifier::Numeric(2253));
        ok("i=42", 0, Identifier::Numeric(42));
        ok("NS=3;I=999", 3, Identifier::Numeric(999));
        ok(" ns=2 ; i=1 ", 2, Identifier::Numeric(1));
    }

    #[test]
    fn string_forms() {
        ok("ns=2;s=Motor.Speed", 2, Identifier::String("Motor.Speed".into()));
        ok("ns=2;s=HelloWorld", 2, Identifier::String("HelloWorld".into()));
        ok("s=MyVar", 0, Identifier::String("MyVar".into()));
    }

    #[test]
    fn guid_and_opaque() {
        ok("ns=2;g=72962B91-FA75-4A99-8A64-03D6D025A2DA", 2, Identifier::Guid("72962B91-FA75-4A99-8A64-03D6D025A2DA".into()));
        ok("ns=1;b=M/RbKBsRVkePCePcx24oRA==", 1, Identifier::Opaque("M/RbKBsRVkePCePcx24oRA==".into()));
    }

    #[test]
    fn invalid_rejected() {
        assert!(parse_address("").is_err());
        assert!(parse_address("ns=2").is_err()); // 缺 identifier
        assert!(parse_address("ns=2;x=1").is_err()); // 未知 key
        assert!(parse_address("ns=99999;i=1").is_err()); // ns 越界
        assert!(parse_address("ns=2;i=abc").is_err());
        assert!(parse_address("ns=2;i=1;s=foo").is_err()); // 重复 identifier
        assert!(parse_address("ns=2;ns=3;i=1").is_err()); // 重复 ns
        assert!(parse_address("ns=2;g=not-a-guid").is_err());
        assert!(parse_address("ns=2;b=$$$").is_err()); // 非 base64
        assert!(parse_address("ns=2;s=").is_err());
        assert!(parse_address("i=").is_err());
    }
}
