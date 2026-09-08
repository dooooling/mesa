//! S7Comm 传输错误（协议层唯一错误类型）。
//!
//! 设计：本 crate 不依赖 `mesa-driver-sdk`，调用方（`s7` / `sinumerik-nck`
//! Driver）按 `kind` 一一映射为自身错误类型，`code`/`message` 原样透传，
//! 以保证抽取前后诊断文本逐字一致。S7 CPU 返回码（0x03/0x04/0x05）的语义
//! 与排障指引属于 S7Comm 协议知识，归本层；地址解析仍归 Driver。

use thiserror::Error;

// ---------------------------------------------------------------------------
// S7 CPU 返回码（协议常量，解释“为什么”）
// ---------------------------------------------------------------------------

/// S7 CPU 侧错误：0x05 地址错误（DB 不存在/非标准访问/越界）。
pub const S7_ERR_ADDRESS: u8 = 0x05;
/// S7 CPU 侧错误：0x04 上下文不支持（S7-1200/1500 未放行 PUT/GET）。
pub const S7_ERR_CONTEXT: u8 = 0x04;
/// S7 CPU 侧错误：0x03 访问拒绝（CPU 保护等级/安全策略）。
pub const S7_ERR_ACCESS: u8 = 0x03;
/// S7 item 正常返回码。
pub const S7_ITEM_OK: u8 = 0xFF;

/// 传输错误分类（调用方一一映射为自身 `ErrorKind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S7TransportErrorKind {
    /// 超时（含连接/COTP/Setup/读写各阶段，由 `code` 区分）。
    Timeout,
    /// TCP/COTP 连接类失败。
    Connection,
    /// 协议帧异常（版本/长度/ROSCTR/回显/基数错位）。
    Protocol,
    /// 对端拒绝且原因为配置类（PUT/GET 未放行、保护等级）。
    Configuration,
    /// 对端拒绝且原因为地址类（DB 不存在、越界、优化块）。
    Address,
}

/// S7Comm 传输错误：`kind` 供调用方映射，`code`/`message` 透传给诊断。
#[derive(Debug, Error, Clone, PartialEq)]
#[error("{code}: {message}")]
pub struct S7TransportError {
    pub kind: S7TransportErrorKind,
    pub code: String,
    pub message: String,
}

impl S7TransportError {
    pub fn new(
        kind: S7TransportErrorKind,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn timeout(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(S7TransportErrorKind::Timeout, code, message)
    }

    pub fn connection(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(S7TransportErrorKind::Connection, code, message)
    }

    pub fn protocol(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(S7TransportErrorKind::Protocol, code, message)
    }
}

/// S7 CPU 错误码 → 传输错误（语义与指引文本与抽取前 `map_s7_error` 逐字一致）。
///
/// - 0x04 上下文不支持：S7-1200/1500 必须在 TIA Portal 放行 PUT/GET；
/// - 0x05 地址错误：检查 DB 是否存在且为标准访问（非优化块）、地址是否越界；
/// - 0x03 拒绝：CPU 保护等级或安全策略禁止外部访问。
pub fn s7_cpu_error(code: u8, ctx: &str) -> S7TransportError {
    let (kind, help) = match code {
        S7_ERR_CONTEXT => (
            S7TransportErrorKind::Configuration,
            "（0x04 上下文不支持）S7-1200/1500 请在 TIA Portal 硬件组态 CPU 属性 -> 防护与安全 -> 连接机制 中勾选“允许来自远程对象的 PUT/GET 通信访问”",
        ),
        S7_ERR_ADDRESS => (
            S7TransportErrorKind::Address,
            "（0x05 地址错误）检查 DB 是否存在且为标准访问（非优化块），地址是否越界",
        ),
        S7_ERR_ACCESS => (
            S7TransportErrorKind::Configuration,
            "（0x03 拒绝）CPU 保护等级或安全策略禁止外部访问",
        ),
        _ => (S7TransportErrorKind::Protocol, ""),
    };
    S7TransportError::new(
        kind,
        format!("S7_0x{code:02X}"),
        format!("{ctx}: S7 错误 0x{code:02X} {help}"),
    )
}

/// TCP 建连失败 → 传输错误（提示文本与抽取前逐字一致）。
pub fn map_connect_error(e: std::io::Error, addr: &str, port: u16) -> S7TransportError {
    let hint = match e.kind() {
        std::io::ErrorKind::ConnectionRefused => {
            format!("连接被拒绝 {addr}（PLC 未开机/端口 {port} 未开放）")
        }
        std::io::ErrorKind::TimedOut => format!("连接超时 {addr}（网络不可达或 PLC 无响应）"),
        _ => format!("TCP 连接失败 {addr}: {e}"),
    };
    S7TransportError::connection("CONNECT_FAIL", hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_error_kinds_match_legacy_mapping() {
        // 与抽取前 map_s7_error 的 kind 映射一致：0x04→Configuration，
        // 0x05→Address，0x03→Configuration，其余→Protocol。
        assert_eq!(
            s7_cpu_error(0x04, "ctx").kind,
            S7TransportErrorKind::Configuration
        );
        assert_eq!(s7_cpu_error(0x05, "ctx").kind, S7TransportErrorKind::Address);
        assert_eq!(
            s7_cpu_error(0x03, "ctx").kind,
            S7TransportErrorKind::Configuration
        );
        assert_eq!(
            s7_cpu_error(0x06, "ctx").kind,
            S7TransportErrorKind::Protocol
        );
        assert_eq!(s7_cpu_error(0x04, "ctx").code, "S7_0x04");
        assert!(s7_cpu_error(0x04, "ctx").message.contains("PUT/GET"));
    }
}
