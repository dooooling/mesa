//! Profile 加载器 + 确定性匹配器（V1.2.1 §8/§10，feat/dynamic-probe 阶段 4）。
//!
//! 规则（用户冻结）：Driver 只返回 facts，facts→profile 的解释权只在 Core。
//! 匹配语义：Profile 的全部 match_rules 都满足才命中；按 specificity 降序、
//! profile.id 升序排列，保证任意输入顺序下结果确定。

use std::path::Path;

use mesa_core_types::{DeviceProfile, ProbeReport};

/// 扫描 `drivers_dir` 下所有 `profiles/*.json`，返回校验通过的 Profile 列表。
pub fn load_profiles(drivers_dir: &Path) -> Vec<DeviceProfile> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(drivers_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let driver_dir = entry.path();
        if !driver_dir.is_dir() {
            continue;
        }
        let profiles_dir = driver_dir.join("profiles");
        if !profiles_dir.is_dir() {
            continue;
        }
        let Ok(profile_entries) = std::fs::read_dir(&profiles_dir) else {
            continue;
        };
        for pe in profile_entries.flatten() {
            let path = pe.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                tracing::warn!(path=%path.display(), "profile read failed");
                continue;
            };
            match serde_json::from_str::<DeviceProfile>(&content) {
                Ok(p) => match p.validate() {
                    Ok(()) => out.push(p),
                    Err(e) => {
                        tracing::warn!(path=%path.display(), error=%e, "profile validate failed")
                    }
                },
                Err(e) => tracing::warn!(path=%path.display(), error=%e, "profile parse failed"),
            }
        }
    }
    out
}

/// Profile 命中（第一版只返回 id，不引入模糊分数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMatch {
    pub profile_id: String,
}

/// 取 facts 字段值：`driver_id` 来自调用参数，`probe.*` 来自报告；
/// 报告中为 None 的事实永不满足任何规则（"没探测到" ≠ "值为空字符串"）。
fn probe_fact<'a>(driver_id: &'a str, report: &'a ProbeReport, field: &str) -> Option<&'a str> {
    match field {
        "driver_id" => Some(driver_id),
        "probe.vendor" => report.vendor.as_deref(),
        "probe.family" => report.family.as_deref(),
        "probe.model" => report.model.as_deref(),
        "probe.firmware" => report.firmware.as_deref(),
        _ => None,
    }
}

fn rule_matches(driver_id: &str, report: &ProbeReport, rule: &mesa_core_types::MatchRule) -> bool {
    let Some(fact) = probe_fact(driver_id, report, rule.field.as_str()) else {
        return false;
    };
    match rule.op.as_str() {
        "eq" => rule.value.as_str().is_some_and(|v| v == fact),
        "in" => rule
            .value
            .as_array()
            .is_some_and(|arr| arr.iter().any(|v| v.as_str().is_some_and(|s| s == fact))),
        "prefix" => rule.value.as_str().is_some_and(|p| fact.starts_with(p)),
        _ => false,
    }
}

/// 规则权重（specificity）：model 4 > family/firmware 3 > vendor 2 > driver 1。
/// firmware 与 family 同级——单凭固件版本号不足以超越 model 精度。
fn rule_weight(field: &str) -> u32 {
    match field {
        "probe.model" => 4,
        "probe.family" | "probe.firmware" => 3,
        "probe.vendor" => 2,
        "driver_id" => 1,
        _ => 0,
    }
}

/// 确定性匹配：全部规则 AND，specificity（权重和）降序，同分按 id 升序。
pub fn match_profiles(
    driver_id: &str,
    report: &ProbeReport,
    profiles: &[DeviceProfile],
) -> Vec<ProfileMatch> {
    let mut hits: Vec<(u32, &str)> = Vec::new();
    for p in profiles {
        if !p
            .match_rules
            .iter()
            .all(|r| rule_matches(driver_id, report, r))
        {
            continue;
        }
        let specificity: u32 = p.match_rules.iter().map(|r| rule_weight(&r.field)).sum();
        hits.push((specificity, p.id.as_str()));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    hits.into_iter()
        .map(|(_, id)| ProfileMatch {
            profile_id: id.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{MatchRule, ProbeCapabilities};

    fn rule(field: &str, op: &str, value: serde_json::Value) -> MatchRule {
        MatchRule {
            field: field.into(),
            op: op.into(),
            value,
        }
    }

    fn profile(id: &str, rules: Vec<MatchRule>) -> DeviceProfile {
        DeviceProfile {
            profile_version: 1,
            id: id.into(),
            version: "1.0.0".into(),
            vendor: String::new(),
            family: String::new(),
            model: String::new(),
            driver_id: String::new(),
            match_rules: rules,
            connection_defaults: serde_json::Value::Null,
            rate_classes: Default::default(),
            presets: vec![],
        }
    }

    fn report(model: Option<&str>) -> ProbeReport {
        ProbeReport {
            reachable: true,
            vendor: Some("FANUC".into()),
            family: Some("0i".into()),
            model: model.map(str::to_string),
            firmware: None,
            capabilities: ProbeCapabilities::default(),
            warnings: vec![],
        }
    }

    #[test]
    fn load_profiles_finds_at_least_simulator() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
        let profiles = load_profiles(&dir);
        assert!(profiles.iter().any(|p| p.id == "simulator-basic"));
        assert!(profiles.iter().any(|p| p.id == "fanuc-0i-f-plus"));
    }

    #[test]
    fn eq_in_prefix_ops() {
        let r = report(Some("0i-F Plus"));
        assert!(rule_matches(
            "focas2",
            &r,
            &rule("probe.model", "eq", "0i-F Plus".into())
        ));
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.model", "eq", "0i-F".into())
        ));
        assert!(rule_matches(
            "focas2",
            &r,
            &rule(
                "probe.vendor",
                "in",
                serde_json::json!(["SIEMENS", "FANUC"])
            )
        ));
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.vendor", "in", serde_json::json!(["SIEMENS"]))
        ));
        assert!(rule_matches(
            "focas2",
            &r,
            &rule("probe.model", "prefix", "0i-F".into())
        ));
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.model", "prefix", "16i".into())
        ));
        // 非法 op 与类型错配一律 false，不 panic
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.model", "regex", ".*".into())
        ));
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.model", "eq", serde_json::json!(42))
        ));
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.model", "in", serde_json::json!("0i-F Plus"))
        ));
    }

    #[test]
    fn none_fact_never_matches() {
        // firmware 为 None：任何针对它的规则都不满足
        let r = report(Some("0i-F Plus"));
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.firmware", "eq", "".into())
        ));
        assert!(!rule_matches(
            "focas2",
            &r,
            &rule("probe.firmware", "prefix", "".into())
        ));
    }

    #[test]
    fn all_rules_must_match_and_specificity_orders() {
        let profiles = vec![
            profile(
                "driver-only",
                vec![rule("driver_id", "eq", "focas2".into())],
            ),
            profile(
                "model-exact",
                vec![
                    rule("driver_id", "eq", "focas2".into()),
                    rule("probe.model", "eq", "0i-F Plus".into()),
                ],
            ),
            profile(
                "family-wide",
                vec![
                    rule("driver_id", "eq", "focas2".into()),
                    rule("probe.family", "eq", "0i".into()),
                ],
            ),
            profile("other-driver", vec![rule("driver_id", "eq", "s7".into())]),
        ];
        let hits = match_profiles("focas2", &report(Some("0i-F Plus")), &profiles);
        let ids: Vec<&str> = hits.iter().map(|h| h.profile_id.as_str()).collect();
        // model(4+1) > family(3+1) > driver-only(1)；other-driver 被 AND 滤掉
        assert_eq!(ids, vec!["model-exact", "family-wide", "driver-only"]);
    }

    #[test]
    fn tie_breaks_by_profile_id_for_determinism() {
        let profiles = vec![
            profile("zzz", vec![rule("driver_id", "eq", "s".into())]),
            profile("aaa", vec![rule("driver_id", "eq", "s".into())]),
        ];
        let hits = match_profiles("s", &report(None), &profiles);
        let ids: Vec<&str> = hits.iter().map(|h| h.profile_id.as_str()).collect();
        assert_eq!(ids, vec!["aaa", "zzz"]);
    }

    #[test]
    fn no_match_returns_empty() {
        let profiles = vec![profile(
            "model-exact",
            vec![rule("probe.model", "eq", "0i-F Plus".into())],
        )];
        assert!(match_profiles("focas2", &report(Some("other")), &profiles).is_empty());
        assert!(match_profiles("focas2", &report(None), &profiles).is_empty());
    }

    #[test]
    fn real_assets_fanuc_report_hits_fanuc_profile_first() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
        let profiles = load_profiles(&dir);
        let r = ProbeReport {
            reachable: true,
            vendor: Some("FANUC".into()),
            family: Some("0i".into()),
            model: Some("0i-F Plus".into()),
            firmware: Some("V10".into()),
            capabilities: ProbeCapabilities::default(),
            warnings: vec![],
        };
        let hits = match_profiles("focas2", &r, &profiles);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].profile_id, "fanuc-0i-f-plus");
    }
}
