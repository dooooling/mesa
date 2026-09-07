//! SINUMERIK 连接配置（Checkpoint A：descriptor-driven，沿用 OPC UA 形态）。
//!
//! 字段与通用 OPC UA 驱动对齐（endpoint / 安全策略 / 认证 / 超时），语义完全一致，
//! 以便运维心智统一；Secret（password）只走 Endpoint connection JSON（Core Secret
//! 边界），本驱动绝不另开明文通道、不落盘、不打日志。
//!
//! PKI 目录由驱动侧从环境解析后经 options 注入 transport（transport 本体不读环境变量）：
//! - `MESA_OPCUA_PKI_DIR`（与通用 OPC UA 共用主机 PKI；未设则 `data/certificates/opcua`）。

use mesa_driver_sdk::SdkDriverError;

// ---------------------------------------------------------------------------
// 连接配置
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SinumerikConnConfig {
    pub endpoint_url: String,
    pub timeout_ms: u64,
    /// SecurityPolicy，如 "None" / "Basic256Sha256"。
    pub security_policy: String,
    /// MessageSecurityMode，如 "None" / "Sign" / "SignAndEncrypt"。
    pub security_mode: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Default for SinumerikConnConfig {
    fn default() -> Self {
        Self {
            endpoint_url: "opc.tcp://127.0.0.1:4840".into(),
            timeout_ms: 5000,
            security_policy: "None".into(),
            security_mode: "None".into(),
            username: None,
            password: None,
        }
    }
}

impl SinumerikConnConfig {
    /// PKI 目录解析（驱动侧职责）：仅允许环境变量 `MESA_OPCUA_PKI_DIR` 或默认值，
    /// 禁止 Endpoint JSON 指定路径；解析后经 options 注入 transport。
    pub fn resolve_pki_dir() -> std::path::PathBuf {
        std::env::var("MESA_OPCUA_PKI_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("data/certificates/opcua"))
    }

    /// 组装 transport 连接选项（含 PKI 注入与 SINUMERIK 应用身份）。
    pub fn connect_options(&self) -> mesa_opcua_transport::OpcUaConnectOptions {
        mesa_opcua_transport::OpcUaConnectOptions {
            endpoint_url: self.endpoint_url.clone(),
            timeout_ms: self.timeout_ms,
            security_policy: self.security_policy.clone(),
            security_mode: self.security_mode.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            pki_dir: Self::resolve_pki_dir(),
            application_name: "Mesa SINUMERIK".into(),
            application_uri: "urn:Mesa:sinumerik".into(),
        }
    }

    /// 解析 Endpoint connection JSON（fail-closed：非法一律 BAD_CONFIG）。
    pub fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        // 兼容多种写法：endpoint_url / endpoint / url / host+port
        let endpoint_url = if let Some(s) = v
            .get("endpoint_url")
            .or_else(|| v.get("endpoint"))
            .or_else(|| v.get("url"))
            .and_then(|x| x.as_str())
        {
            s.to_string()
        } else if let Some(host) = v.get("host").and_then(|x| x.as_str()) {
            let port = v
                .get("port")
                .and_then(|x| x.as_u64())
                .unwrap_or(mesa_opcua_transport::DEFAULT_OPCUA_PORT as u64)
                as u16;
            format!("opc.tcp://{host}:{port}")
        } else {
            "opc.tcp://127.0.0.1:4840".to_string()
        };
        let timeout_ms = v
            .get("timeout_ms")
            .or_else(|| v.get("timeout"))
            .and_then(|x| x.as_u64())
            .unwrap_or(5000);
        let security_policy = v
            .get("security_policy")
            .and_then(|x| x.as_str())
            .unwrap_or("None")
            .to_string();
        let security_mode = v
            .get("security_mode")
            .and_then(|x| x.as_str())
            .unwrap_or("None")
            .to_string();
        if endpoint_url.trim().is_empty() {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                "endpoint_url 不能为空",
            ));
        }
        if !endpoint_url.starts_with("opc.tcp://") {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!("endpoint_url `{endpoint_url}` 非法，需 opc.tcp://host:port"),
            ));
        }
        if timeout_ms == 0 {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                "timeout_ms 需 >0",
            ));
        }
        const VALID_POLICIES: [&str; 6] = [
            "None",
            "Basic128Rsa15",
            "Basic256",
            "Basic256Sha256",
            "Aes128_Sha256_RsaOaep",
            "Aes256_Sha256_RsaPss",
        ];
        if !VALID_POLICIES.contains(&security_policy.as_str()) {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!("security_policy `{security_policy}` 非法，期望 {VALID_POLICIES:?}"),
            ));
        }
        const VALID_MODES: [&str; 3] = ["None", "Sign", "SignAndEncrypt"];
        if !VALID_MODES.contains(&security_mode.as_str()) {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!("security_mode `{security_mode}` 非法，期望 {VALID_MODES:?}"),
            ));
        }
        let username = v
            .get("username")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let password = v
            .get("password")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        // 部分凭据必须拒绝而非静默 Anonymous（与通用 OPC UA 同规则）。
        match (&username, &password) {
            (Some(_), None) | (None, Some(_)) => {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    "username 与 password 需同时提供或同时为空；仅提供其一视为配置错误",
                ));
            }
            _ => {}
        }
        Ok(Self {
            endpoint_url,
            timeout_ms,
            security_policy,
            security_mode,
            username,
            password,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(json: serde_json::Value) -> Result<SinumerikConnConfig, SdkDriverError> {
        SinumerikConnConfig::from_json(&json)
    }

    #[test]
    fn defaults_and_host_port_form() {
        let c = cfg(serde_json::json!({})).expect("默认配置 Ok");
        assert_eq!(c.endpoint_url, "opc.tcp://127.0.0.1:4840");
        assert_eq!(c.timeout_ms, 5000);
        let c = cfg(serde_json::json!({"host": "10.0.0.5", "port": 4840})).expect("host 形态 Ok");
        assert_eq!(c.endpoint_url, "opc.tcp://10.0.0.5:4840");
        // 应用身份为 SINUMERIK（与通用 OPC UA 区分，便于服务端侧审计）。
        let opts = c.connect_options();
        assert_eq!(opts.application_name, "Mesa SINUMERIK");
        assert_eq!(opts.application_uri, "urn:Mesa:sinumerik");
    }

    #[test]
    fn invalid_configs_fail_closed() {
        assert!(cfg(serde_json::json!({"endpoint_url": ""})).is_err());
        assert!(cfg(serde_json::json!({"endpoint_url": "http://x"})).is_err());
        assert!(cfg(serde_json::json!({"timeout_ms": 0})).is_err());
        assert!(cfg(serde_json::json!({"security_policy": "Nope"})).is_err());
        assert!(cfg(serde_json::json!({"security_mode": "Encrypt"})).is_err());
        // 部分凭据：静默 Anonymous 是安全隐患，直接拒绝
        assert!(cfg(serde_json::json!({"username": "u"})).is_err());
        assert!(cfg(serde_json::json!({"password": "p"})).is_err());
        assert!(cfg(serde_json::json!({"username": "u", "password": "p"})).is_ok());
    }
}
