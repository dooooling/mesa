//! S7Comm 连接选项（纯传输参数，无任何地址语义）。
//!
//! 本层只认识显式 `local_tsap`/`remote_tsap`：SIMATIC 的 rack/slot 推导
//! （`0x0100 | (rack<<5) | slot`）是 PLC 连接心智，归 `s7` Driver；
//! SINUMERIK NCK 直接填写 TSAP。`from_rack_slot` 仅为 SIMATIC 约定的
//! 便捷构造，校验规则与抽取前一致。

// ---------------------------------------------------------------------------
// 协议常量（解释“为什么”）
// ---------------------------------------------------------------------------

/// S7 默认端口：ISO-on-TCP 固定 102。
pub const S7_DEFAULT_PORT: u16 = 102;
/// COTP TSAP 基址：0x0100 为 S7 侧固定前缀。
pub const S7_TSAP_BASE: u16 = 0x0100;
/// TSAP 中 rack 占 3 位，左移 5 位后与 slot 合并（与 snap7/TIA 兼容）。
pub const S7_TSAP_RACK_SHIFT: u16 = 5;
/// 硬件限制：S7-300/400 rack 0..7，slot 0..31（SIMATIC 约定，NCK 不用此构造）。
pub const S7_MAX_RACK: u8 = 7;
/// 硬件限制：slot 0..31。
pub const S7_MAX_SLOT: u8 = 31;
/// 超时下限：避免过小导致局域网抖动误超时。
pub const S7_MIN_TIMEOUT_MS: u64 = 500;
/// PDU 协商区间：240 为最小可用，960 为上位机常用上限，480 为兼容默认值。
pub const S7_PDU_MIN: u16 = 240;
/// PDU 上限 960。
pub const S7_PDU_MAX: u16 = 960;
/// PDU 兼容默认值 480。
pub const S7_PDU_DEFAULT: u16 = 480;

/// S7Comm 连接选项：TCP + COTP TSAP + 超时 + 期望 PDU。
///
/// NOTE: 不含 rack/slot/db/area 等一切地址语义；NCK 与 S7ANY 调用方
/// 各自决定 TSAP，本层只负责 dial 与握手。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S7ConnectOptions {
    pub host: String,
    pub port: u16,
    /// 本地 TSAP（上位机侧，SIMATIC 约定 0x0100）。
    pub local_tsap: u16,
    /// 远端 TSAP（PLC/NCK 侧）。
    pub remote_tsap: u16,
    pub timeout_ms: u64,
    /// 期望 PDU（对方可向下协商，见 `S7Session::negotiated_pdu_length`）。
    pub requested_pdu_length: u16,
}

impl Default for S7ConnectOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: S7_DEFAULT_PORT,
            local_tsap: S7_TSAP_BASE,
            remote_tsap: S7_TSAP_BASE | 1,
            timeout_ms: 3000,
            requested_pdu_length: S7_PDU_DEFAULT,
        }
    }
}

impl S7ConnectOptions {
    /// SIMATIC rack/slot → TSAP 的约定推导（与 snap7/TIA Portal 兼容）。
    ///
    /// 校验超出硬件范围时返回错误文本（调用方决定错误类型）。
    pub fn from_rack_slot(
        host: &str,
        port: u16,
        rack: u8,
        slot: u8,
        timeout_ms: u64,
        requested_pdu_length: u16,
    ) -> Result<Self, String> {
        if host.is_empty() {
            return Err("host 不能为空".into());
        }
        if port == 0 {
            return Err(format!("port {port} 非法，需 1..=65535"));
        }
        if rack > S7_MAX_RACK {
            return Err(format!("rack {rack} 非法，允许 0..{S7_MAX_RACK}"));
        }
        if slot > S7_MAX_SLOT {
            return Err(format!("slot {slot} 非法，允许 0..{S7_MAX_SLOT}"));
        }
        Ok(Self {
            host: host.to_string(),
            port,
            local_tsap: S7_TSAP_BASE,
            remote_tsap: S7_TSAP_BASE | ((rack as u16) << S7_TSAP_RACK_SHIFT) | (slot as u16),
            timeout_ms: timeout_ms.max(S7_MIN_TIMEOUT_MS),
            requested_pdu_length: requested_pdu_length.clamp(S7_PDU_MIN, S7_PDU_MAX),
        })
    }

    /// `host:port` 拨号串。
    pub fn dial_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// 超时（已应用下限）。
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms.max(S7_MIN_TIMEOUT_MS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rack_slot_tsap_matches_legacy_formula() {
        // 抽取前公式：dst = 0x0100 | (rack<<5) | slot。rack0/slot1 → 0x0101。
        let o = S7ConnectOptions::from_rack_slot("10.0.0.1", 102, 0, 1, 3000, 480).unwrap();
        assert_eq!(o.local_tsap, 0x0100);
        assert_eq!(o.remote_tsap, 0x0101);
        let o = S7ConnectOptions::from_rack_slot("10.0.0.1", 102, 0, 2, 3000, 480).unwrap();
        assert_eq!(o.remote_tsap, 0x0102);
    }

    #[test]
    fn invalid_rack_slot_rejected() {
        assert!(S7ConnectOptions::from_rack_slot("h", 102, 8, 1, 3000, 480).is_err());
        assert!(S7ConnectOptions::from_rack_slot("h", 102, 0, 32, 3000, 480).is_err());
        assert!(S7ConnectOptions::from_rack_slot("", 102, 0, 1, 3000, 480).is_err());
        assert!(S7ConnectOptions::from_rack_slot("h", 0, 0, 1, 3000, 480).is_err());
    }

    #[test]
    fn timeout_floor_and_pdu_clamp() {
        let o = S7ConnectOptions::from_rack_slot("h", 102, 0, 1, 10, 10_000).unwrap();
        assert_eq!(o.timeout_ms, S7_MIN_TIMEOUT_MS);
        assert_eq!(o.requested_pdu_length, S7_PDU_MAX);
        let o = S7ConnectOptions::from_rack_slot("h", 102, 0, 1, 3000, 100).unwrap();
        assert_eq!(o.requested_pdu_length, S7_PDU_MIN);
    }
}
