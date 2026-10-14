//! NCK 连接配置（ADR 0001 §7：NCK Connection Contract）。
//!
//! 不照搬 S7 PLC 的 rack/slot（那是 PLC 连接心智）：NCK 直接填写
//! `local_tsap`/`remote_tsap`。TSAP 默认值不凭经验写死，由真机 Gate
//! 决定；此处仅做范围校验（u16 非零由传输层兜底）。
//!
//! 安全边界：Native NCK 无证书/用户口令/安全策略，本配置绝不出现
//! username/password/certificate/security_policy（见 CONTRACT.md）。

use mesa_driver_sdk::SdkDriverError;
use mesa_s7_transport::{S7_PDU_DEFAULT, S7_PDU_MAX, S7_PDU_MIN, S7ConnectOptions};

// ---------------------------------------------------------------------------
// 常量（解释“为什么”）
// ---------------------------------------------------------------------------

/// NCK 默认端口：与 S7 同为 ISO-on-TCP 102（控制器侧同一栈）。
pub const NCK_DEFAULT_PORT: u16 = 102;
/// 超时下限：与 S7 同口径，避免局域网抖动误超时。
pub const NCK_MIN_TIMEOUT_MS: u64 = 500;
/// 默认超时。
pub const NCK_DEFAULT_TIMEOUT_MS: u64 = 5000;

/// NCK 连接配置。
#[derive(Debug, Clone)]
pub struct NckConnConfig {
    pub host: String,
    pub port: u16,
    /// 系列 family（P1-3：`840d-sl` / `828d`，显式必填）。
    ///
    /// catalog 按 `common + exactly one family` 装载，跨系列 mapping 差异
    /// 不得互相覆盖；probe 尚不能可靠识别系列时，显式配置是唯一可靠来源
    /// （未来 `family = auto` + probe 检测 + 不一致 fail-closed，见 P1-3）。
    pub family: String,
    /// 本地 TSAP（上位机侧）。
    pub local_tsap: u16,
    /// 远端 TSAP（NCK 侧；具体值由真机 Gate 冻结）。
    pub remote_tsap: u16,
    pub timeout_ms: u64,
    pub requested_pdu_length: u16,
    /// 每个ReadVar请求的项数上限，与PDU长度分片同时生效。
    pub max_read_items: usize,
}

impl Default for NckConnConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: NCK_DEFAULT_PORT,
            // NOTE: family 无默认值（显式必填，from_json 拒绝缺失）；
            // TSAP 默认值待真机 Gate 冻结（ADR 0001 §7），生产配置必须显式填写。
            family: String::new(),
            local_tsap: 0x0100,
            remote_tsap: 0x0100,
            timeout_ms: NCK_DEFAULT_TIMEOUT_MS,
            requested_pdu_length: S7_PDU_DEFAULT,
            max_read_items: mesa_s7_transport::S7_MAX_ITEMS_PER_PDU,
        }
    }
}

impl NckConnConfig {
    /// 从 Endpoint connection JSON 解析（fail-closed：非法一律 BAD_CONFIG）。
    pub fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        let mut cfg = Self::default();
        // 删除的实验字段不得被静默忽略；直接调用配置解析也遵守同一字段集合。
        let fields = [
            "host",
            "port",
            "family",
            "local_tsap",
            "remote_tsap",
            "timeout_ms",
            "pdu_length",
            "max_read_items",
        ];
        let object = v
            .as_object()
            .ok_or_else(|| SdkDriverError::configuration("BAD_CONFIG", "connection需为对象"))?;
        if let Some(key) = object.keys().find(|key| !fields.contains(&key.as_str())) {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!("不支持的连接字段 `{key}`"),
            ));
        }
        cfg.host = object
            .get("host")
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                SdkDriverError::configuration("BAD_CONFIG", "host 必填且必须为非空字符串")
            })?
            .to_string();
        if let Some(value) = object.get("port") {
            let p = value.as_u64().ok_or_else(|| {
                SdkDriverError::configuration("BAD_CONFIG", "port 必须为 1..=65535 的整数")
            })?;
            if !(1..=65535).contains(&p) {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("port {p} 非法，需 1..=65535"),
                ));
            }
            cfg.port = p as u16;
        }
        // TSAP：显式必填（NCK 无 rack/slot 推导；远端值由真机 Gate 定）。
        // 双格式：JSON 整数（256）或十六进制字符串（"0x0100"/"0100h" 大小写不限）；
        // 均换算为同一 u16；越界/非法一律 BAD_CONFIG（fail-closed）。
        fn parse_tsap(v: &serde_json::Value, key: &str) -> Result<u16, SdkDriverError> {
            let bad = || {
                SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("{key} 必填（u16：十进制整数或十六进制如 0x0100）"),
                )
            };
            // JSON 整数直通。
            if let Some(n) = v.get(key).and_then(|x| x.as_u64()) {
                if n > 65535 {
                    return Err(SdkDriverError::configuration(
                        "BAD_CONFIG",
                        format!("{key} {n} 越界，需 0..=65535"),
                    ));
                }
                return Ok(n as u16);
            }
            // 十六进制字符串："0x0100" / "0X0100" / "0100h" / "0100H"（允许前后空白）。
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                let t = s.trim();
                let hex = t
                    .strip_prefix("0x")
                    .or_else(|| t.strip_prefix("0X"))
                    .or_else(|| t.strip_suffix('h').or_else(|| t.strip_suffix('H')))
                    .ok_or_else(bad)?;
                if hex.is_empty() || hex.len() > 4 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(bad());
                }
                let n = u16::from_str_radix(hex, 16).map_err(|_| bad())?;
                return Ok(n);
            }
            Err(bad())
        }
        let local = parse_tsap(v, "local_tsap")?;
        let remote = parse_tsap(v, "remote_tsap")?;
        cfg.local_tsap = local;
        cfg.remote_tsap = remote;
        if let Some(value) = object.get("timeout_ms") {
            let t = value.as_u64().ok_or_else(|| {
                SdkDriverError::configuration("BAD_CONFIG", "timeout_ms 必须为非负整数")
            })?;
            if t < NCK_MIN_TIMEOUT_MS {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("timeout_ms {t} 非法，最小值为 {NCK_MIN_TIMEOUT_MS}"),
                ));
            }
            // 上限与 Descriptor 的 validation.max 同源（C1 §6.2）：超限在
            // 访问网络之前拒绝，而不是被 Probe 外层预算静默截断。
            if t > mesa_core_types::CONNECTION_TIMEOUT_MAX_MS {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!(
                        "timeout_ms {t} 超出支持上限 {}（Probe 分阶段预算按该值推导）",
                        mesa_core_types::CONNECTION_TIMEOUT_MAX_MS
                    ),
                ));
            }
            cfg.timeout_ms = t;
        }
        if let Some(value) = object.get("pdu_length") {
            let pdu = value.as_u64().ok_or_else(|| {
                SdkDriverError::configuration("BAD_CONFIG", "pdu_length 必须为整数")
            })?;
            cfg.pdu_length_set(pdu)?;
        }
        // family：显式必填（common + 单 family 装载，无静默默认系列）。
        let family_value = object.get("family").ok_or_else(|| {
            SdkDriverError::configuration("BAD_CONFIG", "family 必填（如 840d-sl/828d）")
        })?;
        let family = family_value
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                SdkDriverError::configuration("BAD_CONFIG", "family 必须为非空字符串")
            })?;
        cfg.family = family.to_string();
        if let Some(value) = object.get("max_read_items") {
            cfg.max_read_items = value
                .as_u64()
                .filter(|n| (1..=mesa_s7_transport::S7_MAX_ITEMS_PER_PDU as u64).contains(n))
                .ok_or_else(|| {
                    SdkDriverError::configuration("BAD_CONFIG", "max_read_items需为1..=19的整数")
                })? as usize;
        }
        Ok(cfg)
    }

    fn pdu_length_set(&mut self, pdu: u64) -> Result<(), SdkDriverError> {
        if pdu < S7_PDU_MIN as u64 || pdu > S7_PDU_MAX as u64 {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!(
                    "pdu_length `{pdu}` 非法，需 {}..={}",
                    S7_PDU_MIN, S7_PDU_MAX
                ),
            ));
        }
        self.requested_pdu_length = pdu as u16;
        Ok(())
    }

    /// 转传输层连接选项（TSAP 直通，无推导）。
    pub fn to_transport(&self) -> S7ConnectOptions {
        S7ConnectOptions {
            host: self.host.clone(),
            port: self.port,
            local_tsap: self.local_tsap,
            remote_tsap: self.remote_tsap,
            timeout_ms: self.timeout_ms,
            requested_pdu_length: self.requested_pdu_length,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(json: serde_json::Value) -> Result<NckConnConfig, SdkDriverError> {
        NckConnConfig::from_json(&json)
    }

    #[test]
    fn tsap_required_and_validated() {
        // 缺 TSAP 即拒绝（NCK 无 rack/slot 推导可回退）。
        assert!(cfg(serde_json::json!({"host": "10.0.0.5", "family": "840d-sl"})).is_err());
        // family 显式必填（无静默默认系列）。
        assert!(
            cfg(serde_json::json!({"host": "10.0.0.5", "local_tsap": 256, "remote_tsap": 258}))
                .is_err()
        );
        let c = cfg(serde_json::json!({
            "host": "10.0.0.5",
            "family": "840d-sl",
            "local_tsap": 256,
            "remote_tsap": 258,
        }))
        .expect("显式 TSAP 通过");
        assert_eq!((c.local_tsap, c.remote_tsap), (256, 258));
        assert_eq!(c.family, "840d-sl");
        // port 越界拒绝。
        assert!(
            cfg(serde_json::json!({"host": "h", "family": "f", "port": 0, "local_tsap": 1, "remote_tsap": 1}))
                .is_err()
        );
        assert!(
            cfg(serde_json::json!({"host": "h", "family": "f", "port": 70000, "local_tsap": 1, "remote_tsap": 1}))
                .is_err()
        );
        // 空 host 拒绝。
        assert!(
            cfg(serde_json::json!({"host": "", "family": "f", "local_tsap": 1, "remote_tsap": 1}))
                .is_err()
        );
    }

    #[test]
    fn timeout_floor_and_pdu_range() {
        let base =
            serde_json::json!({"host": "h", "family": "f", "local_tsap": 1, "remote_tsap": 1});
        // 缺席 → 驱动默认；恰好等于下限 → 通过。
        assert_eq!(
            cfg(base.clone()).unwrap().timeout_ms,
            NCK_DEFAULT_TIMEOUT_MS
        );
        let mut at_floor = base.clone();
        at_floor["timeout_ms"] = serde_json::json!(NCK_MIN_TIMEOUT_MS);
        assert_eq!(cfg(at_floor).unwrap().timeout_ms, NCK_MIN_TIMEOUT_MS);
        // 存在但非法（过小/字符串/布尔/null）一律拒绝，不得静默回落默认值。
        for bad in [
            serde_json::json!(10),
            serde_json::json!("5000"),
            serde_json::json!(true),
            serde_json::json!(null),
        ] {
            let mut invalid = base.clone();
            invalid["timeout_ms"] = bad.clone();
            assert_eq!(
                cfg(invalid).unwrap_err().code,
                "BAD_CONFIG",
                "timeout_ms={bad} 必须拒绝"
            );
        }
        assert!(cfg(serde_json::json!({"host": "h", "family": "f", "local_tsap": 1, "remote_tsap": 1, "pdu_length": 100})).is_err());
        assert!(cfg(serde_json::json!({"host": "h", "family": "f", "local_tsap": 1, "remote_tsap": 1, "pdu_length": 960})).is_ok());
    }

    /// C1 §6.2：字段「缺席」与「存在但类型非法」必须区分。
    /// Host 缺席或类型非法一律拒绝，绝不落回内置默认目标。
    #[test]
    fn host_must_be_explicit_and_well_typed() {
        let tsap = serde_json::json!({"local_tsap": 256, "remote_tsap": 258});
        let with = |extra: serde_json::Value| {
            let mut v = serde_json::json!({"family": "828d"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        // 缺席 → 拒绝（无隐式默认目标）。
        assert_eq!(cfg(with(tsap.clone())).unwrap_err().code, "BAD_CONFIG");
        // 存在但类型非法 → 拒绝。
        for bad in [
            serde_json::json!(123),
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!([1]),
        ] {
            let mut v = tsap.clone();
            v["host"] = bad.clone();
            assert_eq!(
                cfg(with(v)).unwrap_err().code,
                "BAD_CONFIG",
                "host={bad} 必须拒绝"
            );
        }
        // 空/纯空白 → 拒绝。
        for bad in [serde_json::json!(""), serde_json::json!("   ")] {
            let mut v = tsap.clone();
            v["host"] = bad.clone();
            assert_eq!(cfg(with(v)).unwrap_err().code, "BAD_CONFIG", "host={bad}");
        }
        // port 存在但类型非法 → 拒绝（此前会被静默忽略并回落 102）。
        for bad in [
            serde_json::json!("102"),
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!(1.5),
        ] {
            let mut v = tsap.clone();
            v["host"] = serde_json::json!("10.0.0.5");
            v["port"] = bad.clone();
            assert_eq!(
                cfg(with(v)).unwrap_err().code,
                "BAD_CONFIG",
                "port={bad} 必须拒绝"
            );
        }
        // family 存在但类型非法 → 拒绝（此前会被当作缺失）。
        let mut v = tsap.clone();
        v["host"] = serde_json::json!("10.0.0.5");
        let mut with_bad_family = v.clone();
        with_bad_family["family"] = serde_json::json!(828);
        assert_eq!(
            cfg(with_bad_family).unwrap_err().code,
            "BAD_CONFIG",
            "family 类型非法必须拒绝"
        );
        // 合法显式 Host 仍通过（不回归）。
        v["host"] = serde_json::json!("10.0.0.5");
        assert_eq!(cfg(with(v)).unwrap().host, "10.0.0.5");
    }

    #[test]
    fn transport_passthrough_has_no_rack_slot() {
        let c = cfg(
            serde_json::json!({"host": "h", "family": "f", "local_tsap": 300, "remote_tsap": 400}),
        )
        .unwrap();
        let o = c.to_transport();
        assert_eq!((o.local_tsap, o.remote_tsap), (300, 400));
        assert_eq!(o.dial_addr(), "h:102");
    }

    #[test]
    fn read_limit_is_shared_and_obsolete_fields_are_rejected() {
        for family in ["828d", "840d-sl"] {
            let base = serde_json::json!({"host":"h", "family":family,
                "local_tsap":1024, "remote_tsap":3332});
            assert_eq!(cfg(base.clone()).unwrap().max_read_items, 19);
            let mut single = base.clone();
            single["max_read_items"] = serde_json::json!(1);
            assert_eq!(cfg(single).unwrap().max_read_items, 1);
            for value in [
                serde_json::json!(0),
                serde_json::json!(20),
                serde_json::json!(1.5),
                serde_json::json!("1"),
            ] {
                let mut bad = base.clone();
                bad["max_read_items"] = value;
                assert!(cfg(bad).is_err());
            }
            for key in ["catalog_file", "retain_unresolved_points"] {
                let mut obsolete = base.clone();
                obsolete[key] = serde_json::json!(true);
                assert_eq!(cfg(obsolete).unwrap_err().code, "BAD_CONFIG");
            }
        }
    }

    /// TSAP 双格式：十六进制字符串与十进制整数等价（u16 同值）。
    #[test]
    fn tsap_hex_string_equals_decimal() {
        let base = |local: serde_json::Value, remote: serde_json::Value| {
            cfg(serde_json::json!({
                "host": "10.0.0.5", "family": "840d-sl",
                "local_tsap": local, "remote_tsap": remote,
            }))
            .expect("双格式 TSAP 通过")
        };
        // 0x0100 == 256；0x0102 == 258（大小写/后缀/空白变体同值）。
        for (l, r) in [
            (serde_json::json!("0x0100"), serde_json::json!("0x0102")),
            (serde_json::json!("0X0100"), serde_json::json!("0102H")),
            (serde_json::json!(" 0100h "), serde_json::json!(258)),
            (serde_json::json!(256), serde_json::json!("0x102")),
        ] {
            let c = base(l, r);
            assert_eq!((c.local_tsap, c.remote_tsap), (256, 258));
        }
        // 非法一律拒绝：空串/超长/非 hex/越界/类型错。
        for bad in [
            serde_json::json!(""),
            serde_json::json!("0x"),
            serde_json::json!("0x10000"),
            serde_json::json!("0xZZ"),
            serde_json::json!("12345h"),
            serde_json::json!(70000),
            serde_json::json!(-1),
            serde_json::json!(true),
        ] {
            assert!(
                cfg(serde_json::json!({
                    "host": "h", "family": "f",
                    "local_tsap": bad, "remote_tsap": 258,
                }))
                .is_err(),
                "非法 local_tsap 必须拒绝：{bad}"
            );
        }
    }
}
