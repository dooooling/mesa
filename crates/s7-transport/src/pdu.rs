//! S7 Setup Communication：协商 PDU 长度。
//!
//! 建连后必须先 Setup（功能组 0xF0），对端返回 Ack 并给出可接受的 PDU；
//! 若协商值小于请求值则向下取用，后续一切分片以协商值为准。

use crate::error::{S7TransportError, s7_cpu_error};

// ---------------------------------------------------------------------------
// S7 常量（解释“为什么”）
// ---------------------------------------------------------------------------

/// S7 ROSCTR：0x01=Job（请求），0x03=Ack（确认）。
pub const S7_ROSCTR_JOB: u8 = 0x01;
/// S7 ROSCTR：0x03=Ack。
pub const S7_ROSCTR_ACK: u8 = 0x03;
/// S7 功能码：0x04=ReadVar。
pub const S7_FUNC_READ: u8 = 0x04;
/// S7 功能码：0x05=WriteVar。
pub const S7_FUNC_WRITE: u8 = 0x05;
/// S7 变量规范头：0x12 + 长度 0x0A。
pub const S7_VAR_SPEC: u8 = 0x12;
/// S7 变量规范长度固定 0x0A。
pub const S7_VAR_SPEC_LEN: u8 = 0x0A;
/// S7ANY 语法 ID：0x10（NCK 用 0x82/83/84，由调用方提供，见 `read_var`）。
pub const S7_SYNTAX_ID_S7ANY: u8 = 0x10;

/// 构造 S7 Setup 请求（含 TPKT/COTP 头），exact-bytes 与抽取前一致。
pub fn build_s7_setup(pdu_ref: u16, requested_pdu: u16) -> Vec<u8> {
    // 固定头 0x32 / ROSCTR Job 0x01 / 保留 / PDU ref / param len 8 /
    // data len 0 / F0 功能组。
    let mut s7 = Vec::with_capacity(20);
    s7.extend_from_slice(&[0x32, S7_ROSCTR_JOB, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x08, 0x00, 0x00]);
    s7.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01]);
    s7.extend_from_slice(&requested_pdu.to_be_bytes());
    let cotp = crate::cotp::COTP_DATA_HEADER;
    let tpkt_len = (4 + cotp.len() + s7.len()) as u16;
    let mut pkt = vec![
        crate::tpkt::TPKT_VERSION,
        0x00,
        (tpkt_len >> 8) as u8,
        (tpkt_len & 0xFF) as u8,
    ];
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(&s7);
    pkt
}

/// 解析 Setup Ack，返回协商后 PDU（`min(请求, 对端)`，0 视为不协商）。
///
/// 行为与抽取前 `s7_setup` 的解析段逐字一致：短包/非 Ack 即协议错误。
pub fn parse_setup_ack(resp: &[u8], requested_pdu: u16) -> Result<u16, S7TransportError> {
    if resp.len() < 7 + 12 {
        return Err(S7TransportError::protocol(
            "S7_SETUP_SHORT",
            format!("Setup 响应过短 {}", resp.len().saturating_sub(7)),
        ));
    }
    // 跳过 TPKT 4 + COTP 3。
    let payload = &resp[7..];
    if payload.len() < 12 {
        return Err(S7TransportError::protocol(
            "S7_SETUP_SHORT",
            format!("Setup 响应过短 {}", payload.len()),
        ));
    }
    if payload[1] != S7_ROSCTR_ACK {
        let err = payload.get(17).copied().unwrap_or(0);
        return Err(s7_cpu_error(err, "S7 Setup 被拒绝"));
    }
    let mut negotiated = requested_pdu;
    if payload.len() >= 25 {
        let n = u16::from_be_bytes([payload[23], payload[24]]);
        if n != 0 && n < negotiated {
            negotiated = n;
        }
    }
    Ok(negotiated)
}

/// 为 S7 报文（Setup/Read/Write/SZL 共用）拼接 TPKT + COTP Data 头。
pub fn wrap_s7(pdu_payload: &[u8]) -> Vec<u8> {
    let cotp = crate::cotp::COTP_DATA_HEADER;
    let tpkt_len = (4 + cotp.len() + pdu_payload.len()) as u16;
    let mut pkt = Vec::with_capacity(4 + cotp.len() + pdu_payload.len());
    pkt.push(crate::tpkt::TPKT_VERSION);
    pkt.push(0x00);
    pkt.extend_from_slice(&tpkt_len.to_be_bytes());
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(pdu_payload);
    pkt
}

/// S7 通用头（10 字节）：`32 ROSCTR 0000 pdu_ref param_len data_len`。
pub fn s7_header(rosctr: u8, pdu_ref: u16, param_len: u16, data_len: u16) -> [u8; 10] {
    let mut h = [0u8; 10];
    h[0] = 0x32;
    h[1] = rosctr;
    h[2] = 0x00;
    h[3] = 0x00;
    h[4..6].copy_from_slice(&pdu_ref.to_be_bytes());
    h[6..8].copy_from_slice(&param_len.to_be_bytes());
    h[8..10].copy_from_slice(&data_len.to_be_bytes());
    h
}

/// 检查 S7 Ack 的 ROSCTR，不符则按 CPU 错误码映射（供 Read/Write/SZL 共用）。
pub fn check_ack(s7: &[u8], ctx: &str) -> Result<(), S7TransportError> {
    if s7.len() < 12 {
        return Err(S7TransportError::protocol("S7_SHORT", "S7 头部缺失"));
    }
    if s7[1] != S7_ROSCTR_ACK {
        let err_class = s7.get(17).copied().unwrap_or(0);
        return Err(s7_cpu_error(err_class, ctx));
    }
    Ok(())
}

/// 校验 S7 报文声明长度与实际一致（param_len/data_len），供 Read/Write 共用。
pub fn check_lengths(s7: &[u8]) -> Result<(usize, usize), S7TransportError> {
    if s7.len() < 12 {
        return Err(S7TransportError::protocol("S7_SHORT", "S7 头部缺失"));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    if s7.len() < 12 + param_len + data_len {
        return Err(S7TransportError::protocol(
            "S7_LEN_MISMATCH",
            "S7 长度与实际不符",
        ));
    }
    Ok((param_len, data_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_exact_bytes_match_legacy() {
        // pdu_ref=1, 请求 480：S7 区 18 字节，整包 4+3+18=25。
        let pkt = build_s7_setup(1, 480);
        assert_eq!(
            pkt,
            vec![
                0x03, 0x00, 0x00, 0x19, 0x02, 0xF0, 0x80, 0x32, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
                0x08, 0x00, 0x00, 0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, 0x01, 0xE0,
            ]
        );
    }

    #[test]
    fn setup_ack_negotiates_downward_only() {
        // 构造 Ack：TPKT(4)+COTP(3)+S7(≥25 字节，payload[23..25] 为协商 PDU)。
        let mut resp = vec![0x03, 0x00, 0x00, 0x00, 0x02, 0xF0, 0x80];
        let mut s7 = vec![0x32, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00];
        s7.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
        // 再补 8 字节，使 payload 长度 ≥25；协商值放在 payload[23..25]。
        // s7[18..23] 填充，s7[23]=0x00 s7[24]=0xF0 → 240（向下协商）。
        s7.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x00, 0xF0, 0x00]);
        resp.extend_from_slice(&s7);
        assert_eq!(parse_setup_ack(&resp, 480).unwrap(), 240);
        // 对端报更大（960）不向上取。
        let mut resp2 = resp.clone();
        resp2[7 + 23] = 0x03;
        resp2[7 + 24] = 0xC0;
        assert_eq!(parse_setup_ack(&resp2, 480).unwrap(), 480);
    }

    #[test]
    fn setup_rejected_maps_cpu_error() {
        let mut resp = vec![0x03, 0x00, 0x00, 0x20, 0x02, 0xF0, 0x80];
        let mut s7 = vec![0x32, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00];
        s7.extend_from_slice(&[0u8; 8]);
        resp.extend_from_slice(&s7);
        let err = parse_setup_ack(&resp, 480).unwrap_err();
        assert_eq!(err.code, "S7_0x00");
    }
}
