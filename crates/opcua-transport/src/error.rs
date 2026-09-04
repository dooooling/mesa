//! 结构化传输错误（Stage 2 P0-B 冻结）。
//!
//! 边界规则（V1.2.1）：
//! - 单点 `BadNodeIdUnknown / BadUserAccessDenied` 等必须以 [`opcua_types::DataValue`]
//!   的 `status` 逐点返回，绝不整体 `Err`；只有整个 Service / Session / 连接失败才是 [`UaTransportError`]。
//! - `delete / cleanup` 路径遇到 `BadSubscriptionIdInvalid` 或会话已关闭时幂等返回 `Ok`；
//!   正常 `Read / Browse / Create` 路径遇到 `SessionClosed` 必须上抛，不得吞掉。

use opcua_types::StatusCode;

/// 传输错误类别：只描述"哪一类失败"，不携带 Mesa 业务语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UaTransportErrorKind {
    /// 连接配置非法（URL / 安全策略 / 凭据 / PKI 选项等，建连前即可判定）。
    Configuration,
    /// 建连失败（TCP / Hello / OpenSecureChannel / CreateSession / ActivateSession）。
    Connect,
    /// 服务超时（含建连超时与服务调用超时，可重试）。
    Timeout,
    /// 会话失效（SessionClosed / SessionIdInvalid / SecureChannelClosed / ConnectionClosed）。
    Session,
    /// 服务级失败（Read / Browse / CreateSubscription 等整包被拒，非单点 BAD）。
    Service,
    /// 协议/编解码失败（NodeId 非法、扩展对象解析失败等）。
    Protocol,
    /// 其他内部错误（锁污染、任务 Join 失败等）。
    Internal,
}

impl UaTransportErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            UaTransportErrorKind::Configuration => "Configuration",
            UaTransportErrorKind::Connect => "Connect",
            UaTransportErrorKind::Timeout => "Timeout",
            UaTransportErrorKind::Session => "Session",
            UaTransportErrorKind::Service => "Service",
            UaTransportErrorKind::Protocol => "Protocol",
            UaTransportErrorKind::Internal => "Internal",
        }
    }
}

/// 失败发生的操作：用于日志排障与重试决策，不做业务路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UaOperation {
    Connect,
    Read,
    Browse,
    ReadNamespaceArray,
    CreateSubscription,
    CreateMonitoredItems,
    DeleteMonitoredItems,
    DeleteSubscription,
    Disconnect,
}

impl UaOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            UaOperation::Connect => "Connect",
            UaOperation::Read => "Read",
            UaOperation::Browse => "Browse",
            UaOperation::ReadNamespaceArray => "ReadNamespaceArray",
            UaOperation::CreateSubscription => "CreateSubscription",
            UaOperation::CreateMonitoredItems => "CreateMonitoredItems",
            UaOperation::DeleteMonitoredItems => "DeleteMonitoredItems",
            UaOperation::DeleteSubscription => "DeleteSubscription",
            UaOperation::Disconnect => "Disconnect",
        }
    }
}

/// 结构化传输错误：`kind + operation + status_code + retryable + message`。
#[derive(Debug, Clone, thiserror::Error)]
#[error("[{kind:?}/{operation:?}] {message} (status={status_code:?}, retryable={retryable})")]
pub struct UaTransportError {
    pub kind: UaTransportErrorKind,
    pub operation: UaOperation,
    /// 协议原生 StatusCode bits，无原生码时为 None（如配置错误、本地超时）。
    pub status_code: Option<u32>,
    /// 是否值得重试（超时/会话闪断可重试；配置/参数非法不可重试）。
    pub retryable: bool,
    pub message: String,
}

impl UaTransportError {
    pub fn new(
        kind: UaTransportErrorKind,
        operation: UaOperation,
        status_code: Option<u32>,
        retryable: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            operation,
            status_code,
            retryable,
            message: message.into(),
        }
    }

    pub fn configuration(operation: UaOperation, message: impl Into<String>) -> Self {
        Self::new(
            UaTransportErrorKind::Configuration,
            operation,
            None,
            false,
            message,
        )
    }

    pub fn timeout(operation: UaOperation, message: impl Into<String>) -> Self {
        Self::new(
            UaTransportErrorKind::Timeout,
            operation,
            None,
            true,
            message,
        )
    }

    pub fn session(
        operation: UaOperation,
        status: Option<StatusCode>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            UaTransportErrorKind::Session,
            operation,
            status.map(|s| s.bits()),
            true,
            message,
        )
    }

    pub fn service(
        operation: UaOperation,
        status: Option<StatusCode>,
        retryable: bool,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            UaTransportErrorKind::Service,
            operation,
            status.map(|s| s.bits()),
            retryable,
            message,
        )
    }

    pub fn protocol(operation: UaOperation, message: impl Into<String>) -> Self {
        Self::new(
            UaTransportErrorKind::Protocol,
            operation,
            None,
            false,
            message,
        )
    }

    pub fn internal(operation: UaOperation, message: impl Into<String>) -> Self {
        Self::new(
            UaTransportErrorKind::Internal,
            operation,
            None,
            false,
            message,
        )
    }

    /// 是否为"会话已关闭"类错误（正常路径必须上抛给 supervisor 重试）。
    pub fn is_session_closed(&self) -> bool {
        match self.status_code.map(StatusCode::from) {
            Some(s) => {
                s == StatusCode::BadSessionClosed
                    || s == StatusCode::BadSessionIdInvalid
                    || s == StatusCode::BadSecureChannelClosed
                    || s == StatusCode::BadConnectionClosed
            }
            None => self.kind == UaTransportErrorKind::Session,
        }
    }

    /// delete/cleanup 路径是否可视为幂等成功：
    /// `BadSubscriptionIdInvalid / BadMonitoredItemIdInvalid` 表示对端已无该对象，重删即成功；
    /// 会话已关闭时本地亦无可清理对象，同样视为成功（仅限 delete/cleanup 路径）。
    pub fn is_idempotent_cleanup_ok(&self) -> bool {
        match self.status_code.map(StatusCode::from) {
            Some(s) => {
                s == StatusCode::BadSubscriptionIdInvalid
                    || s == StatusCode::BadMonitoredItemIdInvalid
                    || s == StatusCode::BadSessionClosed
                    || s == StatusCode::BadSessionIdInvalid
                    || s == StatusCode::BadSecureChannelClosed
                    || s == StatusCode::BadConnectionClosed
            }
            None => false,
        }
    }
}

/// 将底层 `async-opcua` 服务错误映射为结构化错误：以原生 StatusCode 为准，消息仅作诊断。
pub fn map_service_error(operation: UaOperation, err: opcua_types::Error) -> UaTransportError {
    let status = err.status();
    let msg = format!("{err:?}");
    if status == StatusCode::BadTimeout {
        return UaTransportError::timeout(operation, msg);
    }
    if status == StatusCode::BadSessionClosed
        || status == StatusCode::BadSessionIdInvalid
        || status == StatusCode::BadSecureChannelClosed
        || status == StatusCode::BadConnectionClosed
    {
        return UaTransportError::session(operation, Some(status), msg);
    }
    // 其余整包失败均为 Service 级（是否可重试由调用方按操作决定，默认不可重试）
    UaTransportError::service(operation, Some(status), false, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_closed_errors_are_retryable_and_propagated() {
        let e = UaTransportError::session(
            UaOperation::Read,
            Some(StatusCode::BadSessionClosed),
            "session gone",
        );
        assert!(e.is_session_closed());
        assert!(e.retryable);
        // cleanup 路径同样幂等 Ok
        assert!(e.is_idempotent_cleanup_ok());
    }

    #[test]
    fn bad_subscription_id_invalid_is_cleanup_ok_but_not_session_closed() {
        let e = UaTransportError::service(
            UaOperation::DeleteSubscription,
            Some(StatusCode::BadSubscriptionIdInvalid),
            false,
            "already gone",
        );
        assert!(e.is_idempotent_cleanup_ok());
        assert!(!e.is_session_closed());
    }

    #[test]
    fn configuration_errors_are_not_retryable() {
        let e = UaTransportError::configuration(UaOperation::Connect, "bad url");
        assert!(!e.retryable);
        assert!(!e.is_idempotent_cleanup_ok());
    }

    #[test]
    fn timeout_errors_are_retryable() {
        let e = UaTransportError::timeout(UaOperation::Read, "elapsed");
        assert!(e.retryable);
        assert_eq!(e.kind, UaTransportErrorKind::Timeout);
    }
}
