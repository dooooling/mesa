//! S7 Setup Communication：协商 PDU 长度。
//!
//! 建连后必须先 Setup（功能组 0xF0），对端返回 Ack 并给出可接受的 PDU；
//! 若协商值小于请求值则向下取用，后续一切分片以协商值为准。

use crate::error::{S7TransportError, s7_cpu_error};

// ---------------------------------------------------------------------------
// S7 常量（解释“为什么”）
// ---------------------------------------------------------------------------

/// S7 ROSCTR：0x01=Job（请求，10 字节头）。
pub const S7_ROSCTR_JOB: u8 = 0x01;
/// S7 ROSCTR：0x03=Ack_Data（确认，12 字节头：10 + error class/code）。
///
/// Wireshark 主干口径（`hlength`）：type 2/3 → 12 字节头，
/// bytes 10-11 为 error class/code，之后才是 parameter。
/// 历史实现曾误作 10 字节头（PR20 review 纠正）。
pub const S7_ROSCTR_ACK_DATA: u8 = 0x03;
/// S7 Job 头长度（10 字节，无 error bytes）。
pub const S7_HEADER_LEN_JOB: usize = 10;
/// S7 Ack_Data 头长度（12 字节：10 + error class/code）。
pub const S7_HEADER_LEN_ACK_DATA: usize = 12;
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

/// 解析 Setup Ack，返回协商后 PDU（`min(请求, 对端)`，0 视为对方无表示）。
///
/// 标准 Ack_Data 形状（冻结）：S7(20) = 12 字节头（含 `00 00` error bytes）
/// 加 8 字节 parameter `[F0 00 00 01 00 01 PDU]`；`param_len` 必须为 8，
/// `data_len` 必须为 0，协商值在 S7[18..20]。
///
/// 其他形状即 `S7_SETUP_SHAPE`（fail-closed，不猜）。历史实现曾读固定
/// `payload[23..25]`（自创口径，真机漏协商），已废除；“NCK 扩展方言”一说
/// 亦已撤回——`00 00` 从来都是 Ack_Data header 的 error bytes，不是 NCK 方言
/// （ADR 0002）。
pub fn parse_setup_ack(resp: &[u8], requested_pdu: u16) -> Result<u16, S7TransportError> {
    if resp.len() < 7 + 20 {
        return Err(S7TransportError::protocol(
            "S7_SETUP_SHORT",
            format!("Setup 响应过短 {}", resp.len().saturating_sub(7)),
        ));
    }
    // 跳过 TPKT 4 + COTP 3。
    let payload = &resp[7..];
    if payload.len() != 20 {
        return Err(S7TransportError::protocol(
            "S7_SETUP_SHAPE",
            format!("Setup 形状未知 S7 长 {}", payload.len()),
        ));
    }
    if payload[1] != S7_ROSCTR_ACK_DATA {
        let err = payload.get(17).copied().unwrap_or(0);
        return Err(s7_cpu_error(err, "S7 Setup 被拒绝"));
    }
    check_ack_data_header(payload, "S7 Setup")?;
    if payload[6] != 0 || payload[7] != 8 {
        return Err(S7TransportError::protocol(
            "S7_SETUP_SHAPE",
            "Setup param_len 非 8",
        ));
    }
    if payload[8] != 0 || payload[9] != 0 {
        return Err(S7TransportError::protocol(
            "S7_SETUP_SHAPE",
            "Setup data_len 非 0",
        ));
    }
    // 只向下协商（min），0 视为对方无表示（保持请求值）。
    let n = u16::from_be_bytes([payload[18], payload[19]]);
    let mut negotiated = requested_pdu;
    if n != 0 && n < negotiated {
        negotiated = n;
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

/// 检查 S7 Ack_Data 的 ROSCTR 与 header error bytes（供 Read 共用）。
///
/// Ack_Data 头为 12 字节：`s7[10]`=error class、`s7[11]`=error code；
/// 任一非零即拒绝（code 走 CPU 错误映射，class 走协议错误）。
/// 历史实现曾从 `s7[17]` 取错误码（错位），已修正。
pub fn check_ack(s7: &[u8], ctx: &str) -> Result<(), S7TransportError> {
    if s7.len() < S7_HEADER_LEN_ACK_DATA {
        return Err(S7TransportError::protocol("S7_SHORT", "S7 头部缺失"));
    }
    if s7[1] != S7_ROSCTR_ACK_DATA {
        let err_class = s7.get(17).copied().unwrap_or(0);
        return Err(s7_cpu_error(err_class, ctx));
    }
    check_ack_data_header(s7, ctx)
}

/// 校验 Ack_Data header error bytes（bytes 10-11 必须全零）。
fn check_ack_data_header(s7: &[u8], ctx: &str) -> Result<(), S7TransportError> {
    let class = s7[10];
    let code = s7[11];
    if class != 0 {
        return Err(S7TransportError::protocol(
            "S7_HEADER_ERR",
            format!("{ctx} header error class={class:02x} code={code:02x}"),
        ));
    }
    if code != 0 {
        return Err(s7_cpu_error(code, ctx));
    }
    Ok(())
}

/// 校验 S7 Ack_Data 报文声明长度与实际一致（param_len/data_len）。
///
/// data 起点恒为 `12 + param_len`（12 字节 Ack_Data 头 + param 区，
/// 标准 Read：plen=2 → +14）。只认 header 声明，不猜。
pub fn check_lengths(s7: &[u8]) -> Result<(usize, usize), S7TransportError> {
    if s7.len() < 12 {
        return Err(S7TransportError::protocol("S7_SHORT", "S7 头部缺失"));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    if s7.len() < S7_HEADER_LEN_ACK_DATA + param_len + data_len {
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
        // 标准 Ack_Data Setup（27 字节）：TPKT(4)+COTP(3)+S7(20)，
        // S7 = 12 字节头（含 00 00 error）+ 8 字节 param，PDU 在 S7[18..20]。
        let mut resp = vec![0x03, 0x00, 0x00, 0x1B, 0x02, 0xF0, 0x80];
        let mut s7 = vec![0x32, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00];
        s7.extend_from_slice(&[0x00, 0x00, 0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0xF0]);
        resp.extend_from_slice(&s7);
        assert_eq!(resp.len(), 27);
        assert_eq!(parse_setup_ack(&resp, 480).unwrap(), 240);
        // 对端报更大（960）不向上取。
        let mut resp2 = resp.clone();
        resp2[7 + 18] = 0x03;
        resp2[7 + 19] = 0xC0;
        assert_eq!(parse_setup_ack(&resp2, 480).unwrap(), 480);
        // 对端 0 视为无表示，保持请求值。
        let mut resp3 = resp.clone();
        resp3[7 + 18] = 0x00;
        resp3[7 + 19] = 0x00;
        assert_eq!(parse_setup_ack(&resp3, 480).unwrap(), 480);
        // param_len 非 8 即拒绝。
        let mut bad_plen = resp.clone();
        bad_plen[7 + 6] = 0x00;
        bad_plen[7 + 7] = 0x0A;
        assert!(parse_setup_ack(&bad_plen, 480).is_err());
        // 非 20 字节 S7 即拒绝。
        let short = &resp[..7 + 19];
        assert!(parse_setup_ack(short, 480).is_err());
    }

    #[test]
    fn setup_ack_header_error_fails() {
        // header error class 非零 → S7_HEADER_ERR（不再从 s7[17] 取码）。
        let mut resp = vec![0x03, 0x00, 0x00, 0x1B, 0x02, 0xF0, 0x80];
        let mut s7 = vec![0x32, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00];
        s7.extend_from_slice(&[0x84, 0x00, 0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, 0x01, 0xE0]);
        resp.extend_from_slice(&s7);
        let err = parse_setup_ack(&resp, 480).unwrap_err();
        assert_eq!(err.code, "S7_HEADER_ERR");
        // error code 非零 → CPU 错误映射。
        let mut resp2 = resp.clone();
        resp2[7 + 10] = 0x00;
        resp2[7 + 11] = 0x05;
        let err = parse_setup_ack(&resp2, 480).unwrap_err();
        assert_eq!(err.code, "S7_0x05");
    }

    #[test]
    fn setup_rejected_maps_cpu_error() {
        // 20 字节 S7 + ROSCTR 非 Ack_Data → CPU 错误映射（s7[17] 为码位）。
        let mut resp = vec![0x03, 0x00, 0x00, 0x1B, 0x02, 0xF0, 0x80];
        let mut s7 = vec![0x32, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00];
        s7.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00]);
        resp.extend_from_slice(&s7);
        let err = parse_setup_ack(&resp, 480).unwrap_err();
        assert_eq!(err.code, "S7_0x05");
    }
}
