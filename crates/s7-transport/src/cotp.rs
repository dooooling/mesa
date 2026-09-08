//! COTP 连接管理（ISO 8073）：`CR -> CC`。
//!
//! 只负责连接请求/确认帧的编解码与校验；TSAP 取值由调用方决定
//! （SIMATIC 用 rack/slot 推导，NCK 用显式 TSAP）。

use crate::error::S7TransportError;

// ---------------------------------------------------------------------------
// COTP 常量（解释“为什么”）
// ---------------------------------------------------------------------------

/// COTP PDU 类型：CR 0xE0（连接请求）。
pub const COTP_CR: u8 = 0xE0;
/// COTP PDU 类型：CC 0xD0（连接确认）。
pub const COTP_CC: u8 = 0xD0;
/// COTP PDU 类型：DT 0xF0（数据）。
pub const COTP_DT: u8 = 0xF0;
/// 数据帧 COTP 头（LI=2 / DT / EOT）：后续所有 S7 报文共用。
pub const COTP_DATA_HEADER: [u8; 3] = [0x02, COTP_DT, 0x80];

/// 构造 COTP CR 包（含 TPKT 头），exact-bytes 与抽取前一致。
///
/// 布局：LI 0x11 / CR 0xE0 / TPDU size 0x0A=1024 / src/dst TSAP。
pub fn build_cotp_cr(src_tsap: u16, dst_tsap: u16) -> Vec<u8> {
    let mut cotp = vec![
        0x11,
        COTP_CR,
        0x00,
        0x00,
        0x00,
        0x01,
        0x00,
        0xC0,
        0x01,
        0x0A,
        0xC1,
        0x02,
        ((src_tsap >> 8) as u8),
        (src_tsap as u8),
        0xC2,
        0x02,
        ((dst_tsap >> 8) as u8),
        (dst_tsap as u8),
    ];
    let tpkt_len = (4 + cotp.len()) as u16;
    let mut pkt = vec![
        crate::tpkt::TPKT_VERSION,
        0x00,
        (tpkt_len >> 8) as u8,
        (tpkt_len & 0xFF) as u8,
    ];
    pkt.append(&mut cotp);
    pkt
}

/// 校验 COTP CC 响应（期望第 6 字节为 0xD0）。
///
/// 被拒常见于 rack/slot 或 PLC TSAP 配置错误，提示保留抽取前文案。
pub fn check_cc(resp: &[u8]) -> Result<(), S7TransportError> {
    if resp.len() < 6 || resp[5] != COTP_CC {
        return Err(S7TransportError::connection(
            "COTP_REJECTED",
            format!("COTP CC 异常，响应 {resp:02x?}，检查 rack/slot 或 PLC TSAP 配置"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cr_exact_bytes_match_legacy() {
        // src=0x0100 dst=0x0101（rack0/slot1）：与抽取前 build_cotp_cr 逐字节一致。
        let pkt = build_cotp_cr(0x0100, 0x0101);
        assert_eq!(
            pkt,
            vec![
                0x03, 0x00, 0x00, 0x16, 0x11, 0xE0, 0x00, 0x00, 0x00, 0x01, 0x00, 0xC0, 0x01,
                0x0A, 0xC1, 0x02, 0x01, 0x00, 0xC2, 0x02, 0x01, 0x01,
            ]
        );
    }

    #[test]
    fn cc_rejected_without_confirm() {
        assert!(check_cc(&[0x03, 0x00, 0x00, 0x16, 0x11, 0xE0]).is_err());
        assert!(check_cc(&[0x03]).is_err());
        let mut ok = vec![0u8; 7];
        ok[5] = COTP_CC;
        assert!(check_cc(&ok).is_ok());
    }
}
