//! Dynamic Probe 数据契约（V1.2.1 §8，feat/dynamic-probe）。
//!
//! 边界：本模块只描述"本次探测到的设备事实"，不做匹配、不做持久化、不碰
//! Data Plane。Driver 只返回 facts，facts→profile 的解释权只在 Core。
//!
//! 能力模型（冻结）：动态字符串 ID + 四态，不是固定三 bool——
//! 后者无法区分 AccessDenied/NotPresent，更承载不了 alarm/tool/drive/gud
//! 这类动态 capability ID（P0-1）。
//! - `CapabilityItem.detail`：只写该 capability 自己的局部原因；
//! - `ProbeWarning`：只写跨 capability / 全局性问题；同一问题禁止两处重复。

use serde::{Deserialize, Serialize};

/// ProbeReport JSON 上限 64 KiB（Management Plane 低频调用，绰绰有余；
/// 超限视为非法报告，fail-closed）。
pub const PROBE_REPORT_MAX_BYTES: usize = 64 * 1024;

/// 能力状态（冻结四态，序列化为 snake_case）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    AccessDenied,
    NotPresent,
    Unknown,
}

/// 单个动态能力：ID 为动态字符串（如 `alarm`/`tool`/`drive`/`gud`），
/// Core 不解释 ID 含义，只做透传与 profile 匹配输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityItem {
    pub id: String,
    pub state: CapabilityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 结构化 Warning：只承载跨 capability / 全局性问题（如 MODEL_UNDETECTED、
/// NAMESPACE_PARTIAL、OPTIONAL_PROBE_TIMEOUT、SECURITY_DOWNGRADE）。
/// 不设 severity/category——按需再评审。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeWarning {
    pub code: String,
    pub message: String,
}

/// 动态探测报告：DriverConnection::probe() 的返回契约。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub reachable: bool,
    /// 扩展事实（不参与匹配白名单校验之外的任何解释）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    /// 模型识别置信度（如 high/low），识别失败时为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_confidence: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<CapabilityItem>,
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
            model_confidence: None,
            capabilities: vec![],
            warnings: vec![ProbeWarning {
                code: code.into(),
                message: message.into(),
            }],
        }
    }

    /// 最小合法性校验（大小/UTF-8 由调用方在 IPC 边界保证）。
    ///
    /// P2 语义冻结（Driver→Core 契约，`Session::probe` 接收边界执行）：
    /// - capability id 唯一：同一 ID 双态是语义冲突，直接拒收；
    /// - unreachable 规范形态：身份全 None、无 capabilities、至少一个 warning
    ///   道明原因（自相矛盾的报告拒收，不靠上层个案兜底）。
    pub fn validate(&self) -> Result<(), String> {
        {
            use std::collections::HashSet;
            let mut seen = HashSet::new();
            for c in &self.capabilities {
                if c.id.trim().is_empty() {
                    return Err("capability id 不能为空".into());
                }
                if !seen.insert(c.id.as_str()) {
                    return Err(format!("capability id 重复: {}", c.id));
                }
            }
        }
        for w in &self.warnings {
            if w.code.trim().is_empty() {
                return Err("probe warning code 不能为空".into());
            }
        }
        if !self.reachable {
            if self.vendor.is_some()
                || self.family.is_some()
                || self.model.is_some()
                || self.firmware.is_some()
                || self.model_confidence.is_some()
            {
                return Err("unreachable 报告不得携带设备身份".into());
            }
            if !self.capabilities.is_empty() {
                return Err("unreachable 报告不得携带 capabilities".into());
            }
            if self.warnings.is_empty() {
                return Err("unreachable 报告必须至少一个 warning 道明原因".into());
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
            model_confidence: Some("high".into()),
            capabilities: vec![
                CapabilityItem {
                    id: "tool".into(),
                    state: CapabilityState::Available,
                    detail: None,
                },
                CapabilityItem {
                    id: "alarm".into(),
                    state: CapabilityState::AccessDenied,
                    detail: Some("BadUserAccessDenied".into()),
                },
                CapabilityItem {
                    id: "drive".into(),
                    state: CapabilityState::NotPresent,
                    detail: None,
                },
                CapabilityItem {
                    id: "gud".into(),
                    state: CapabilityState::Unknown,
                    detail: None,
                },
            ],
            warnings: vec![ProbeWarning {
                code: "NAMESPACE_PARTIAL".into(),
                message: "partial".into(),
            }],
        };
        let s = r.to_report_json().expect("序列化 Ok");
        let back = ProbeReport::from_report_json(&s).expect("回读 Ok");
        assert_eq!(r, back);
        // 四态序列化形态冻结
        assert!(s.contains("\"access_denied\""));
        assert!(s.contains("\"not_present\""));
    }

    #[test]
    fn unknown_device_allows_all_none_and_empty_lists() {
        let r = ProbeReport {
            reachable: true,
            vendor: None,
            family: None,
            model: None,
            firmware: None,
            model_confidence: None,
            capabilities: vec![],
            warnings: vec![],
        };
        let back = ProbeReport::from_report_json(&r.to_report_json().unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn unreachable_helper_carries_code_only_in_warnings() {
        let r = ProbeReport::unreachable("CONNECTION_FAILED", "timeout");
        assert!(!r.reachable);
        assert!(r.capabilities.is_empty());
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].code, "CONNECTION_FAILED");
    }

    #[test]
    fn missing_optional_fields_stay_compatible() {
        // 旧/最小报告缺字段时按 None/空处理，不报错
        let back = ProbeReport::from_report_json(r#"{"reachable":true}"#).unwrap();
        assert!(back.reachable);
        assert!(back.capabilities.is_empty());
        assert!(back.warnings.is_empty());
    }

    #[test]
    fn oversized_report_rejected() {
        let big = "x".repeat(PROBE_REPORT_MAX_BYTES + 1);
        assert!(check_report_size(&big).is_err());
    }

    #[test]
    fn empty_ids_rejected() {
        let mut r = ProbeReport::unreachable("C", "m");
        r.capabilities.push(CapabilityItem {
            id: "  ".into(),
            state: CapabilityState::Unknown,
            detail: None,
        });
        assert!(r.validate().is_err());
        r.capabilities.clear();
        r.warnings[0].code = String::new();
        assert!(r.validate().is_err());
    }

    #[test]
    fn duplicate_capability_id_rejected() {
        // P2：同一 ID 双态是语义冲突，fail-closed。
        let r = ProbeReport::unreachable("C", "m");
        let ok_report = ProbeReport {
            reachable: true,
            vendor: None,
            family: None,
            model: None,
            firmware: None,
            model_confidence: None,
            capabilities: vec![
                CapabilityItem {
                    id: "read".into(),
                    state: CapabilityState::Available,
                    detail: None,
                },
                CapabilityItem {
                    id: "read".into(),
                    state: CapabilityState::NotPresent,
                    detail: None,
                },
            ],
            warnings: vec![],
        };
        assert!(ok_report.validate().is_err());
        assert!(r.validate().is_ok());
    }

    #[test]
    fn unreachable_canonical_form_enforced() {
        // P2：unreachable 自带身份/capabilities/无 warning 一律拒收。
        let mut r = ProbeReport::unreachable("C", "m");
        r.vendor = Some("Siemens".into());
        assert!(r.validate().is_err());
        let mut r = ProbeReport::unreachable("C", "m");
        r.capabilities.push(CapabilityItem {
            id: "read".into(),
            state: CapabilityState::Available,
            detail: None,
        });
        assert!(r.validate().is_err());
        let mut r = ProbeReport::unreachable("C", "m");
        r.warnings.clear();
        assert!(r.validate().is_err());
    }
}
