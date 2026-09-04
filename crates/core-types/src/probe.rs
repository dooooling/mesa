//! Dynamic Probe 数据契约（V1.2.1 §8，feat/dynamic-probe 阶段 1）。
//!
//! 边界：本模块只描述"本次探测到的设备事实"，不做匹配、不做持久化、不碰
//! Data Plane。Driver 只返回 facts，facts→profile 的解释权只在 Core。
//! 三态能力使用 `Option<bool>`：`Some(true)` 已确认支持 / `Some(false)`
//! 已确认不支持 / `None` 本次未确认——"没探测"绝不能被误读为"不支持"。

use serde::{Deserialize, Serialize};

/// ProbeReport JSON 上限 64 KiB（Management Plane 低频调用，绰绰有余；
/// 超限视为非法报告，fail-closed）。
pub const PROBE_REPORT_MAX_BYTES: usize = 64 * 1024;

/// 本次实际探测到的设备能力（与静态 `DriverCapabilities` 语义不同：
/// 后者声明 Driver 类型支持什么，前者记录这次对端实际确认了什么）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProbeCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browse: Option<bool>,
}

/// 结构化 Warning：只承载跨 capability / 全局性问题（如 MODEL_UNDETECTED、
/// NAMESPACE_PARTIAL），单个 capability 的状态原因只写它自己的去处，
/// 同一问题禁止同时写两处。不设 severity/category——按需再评审。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeWarning {
    pub code: String,
    pub message: String,
}

/// 动态探测报告：Driver::probe() 的返回契约。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default)]
    pub capabilities: ProbeCapabilities,
    #[serde(default)]
    pub warnings: Vec<ProbeWarning>,
}

impl ProbeReport {
    /// 不可达报告的规范构造：device 全 None，原因只进 warnings。
    pub fn unreachable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            reachable: false,
            vendor: None,
            family: None,
            model: None,
            firmware: None,
            capabilities: ProbeCapabilities::default(),
            warnings: vec![ProbeWarning {
                code: code.into(),
                message: message.into(),
            }],
        }
    }

    /// 最小合法性校验（大小/UTF-8 由调用方在 IPC 边界保证）。
    pub fn validate(&self) -> Result<(), String> {
        for w in &self.warnings {
            if w.code.trim().is_empty() {
                return Err("probe warning code 不能为空".into());
            }
        }
        Ok(())
    }

    /// 序列化并执行上限检查（IPC 发送前调用）。
    pub fn to_report_json(&self) -> Result<String, String> {
        let s = serde_json::to_string(self).map_err(|e| format!("ProbeReport 序列化失败: {e}"))?;
        check_report_size(&s)?;
        Ok(s)
    }

    /// 解析并校验（IPC 接收后调用；未知字段忽略保证前向兼容）。
    pub fn from_report_json(s: &str) -> Result<Self, String> {
        check_report_size(s)?;
        let r: Self = serde_json::from_str(s).map_err(|e| format!("ProbeReport 解析失败: {e}"))?;
        r.validate()?;
        Ok(r)
    }
}

/// 报告大小检查（调用方保证 UTF-8；serde_json 输出恒为合法 UTF-8）。
pub fn check_report_size(s: &str) -> Result<(), String> {
    if s.len() > PROBE_REPORT_MAX_BYTES {
        return Err(format!(
            "ProbeReport 超限 {} > {} bytes",
            s.len(),
            PROBE_REPORT_MAX_BYTES
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_report_json_roundtrip() {
        let r = ProbeReport {
            reachable: true,
            vendor: Some("FANUC".into()),
            family: Some("0i-F".into()),
            model: Some("0i-F Plus".into()),
            firmware: Some("V1.0".into()),
            capabilities: ProbeCapabilities {
                read: Some(true),
                subscribe: None,
                browse: Some(false),
            },
            warnings: vec![ProbeWarning {
                code: "NAMESPACE_PARTIAL".into(),
                message: "partial".into(),
            }],
        };
        let s = r.to_report_json().expect("序列化 Ok");
        let back = ProbeReport::from_report_json(&s).expect("回读 Ok");
        assert_eq!(r, back);
        // None 能力不序列化（压缩噪音），但回读仍为 None
        assert!(!s.contains("subscribe"));
    }

    #[test]
    fn unknown_device_allows_all_none_and_empty_warnings() {
        let r = ProbeReport {
            reachable: true,
            vendor: None,
            family: None,
            model: None,
            firmware: None,
            capabilities: ProbeCapabilities::default(),
            warnings: vec![],
        };
        let back = ProbeReport::from_report_json(&r.to_report_json().unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn unreachable_helper_carries_code_only_in_warnings() {
        let r = ProbeReport::unreachable("CONNECTION_FAILED", "timeout");
        assert!(!r.reachable);
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].code, "CONNECTION_FAILED");
    }

    #[test]
    fn missing_optional_fields_stay_compatible() {
        // 旧/最小报告缺字段时按 None/空处理，不报错
        let back = ProbeReport::from_report_json(r#"{"reachable":true}"#).unwrap();
        assert!(back.reachable);
        assert_eq!(back.capabilities, ProbeCapabilities::default());
        assert!(back.warnings.is_empty());
    }

    #[test]
    fn oversized_report_rejected() {
        let big = "x".repeat(PROBE_REPORT_MAX_BYTES + 1);
        assert!(check_report_size(&big).is_err());
    }

    #[test]
    fn empty_warning_code_rejected() {
        let mut r = ProbeReport::unreachable("", "m");
        assert!(r.validate().is_err());
        r.warnings[0].code = "OK".into();
        assert!(r.validate().is_ok());
    }
}
