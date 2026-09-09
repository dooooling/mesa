//! SZL（System Status List）读取：只读诊断（CPU 标识/模块信息）。
//!
//! S7 功能 0x07 UserData 请求，返回原始 SZL 负载（已去 TPKT/COTP/S7 头），
//! 上层诊断做二次解析。`s7` Driver 的 probe 用 `SZL 0x0011` 取 CPU 订货号。

use crate::error::S7TransportError;
use crate::pdu::{S7_ROSCTR_ACK, wrap_s7};

/// 构造 SZL 请求（UserData 固定头 + SZL ID/Index），与抽取前逐字节一致。
///
/// 模板依据与 snap7 兼容的抓包提炼：TPKT 4 + COTP 3 + S7 32 07 ……
pub fn build_szl_request(pdu_ref: u16, szl_id: u16, szl_index: u16) -> Vec<u8> {
    let mut s7 = Vec::with_capacity(32);
    s7.extend_from_slice(&[0x32, 0x07, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x08, 0x00, 0x04]);
    // UserData 头：0x00 0x01 0x12 0x04 0x11 0x44 0x01 0x00 0xFF 0x09 0x00 0x04。
    s7.extend_from_slice(&[
        0x00, 0x01, 0x12, 0x04, 0x11, 0x44, 0x01, 0x00, 0xFF, 0x09, 0x00, 0x04,
    ]);
    s7.extend_from_slice(&szl_id.to_be_bytes());
    s7.extend_from_slice(&szl_index.to_be_bytes());
    wrap_s7(&s7)
}

/// 解析 SZL 响应：透传 S7 头之后全部 UserData（调用方二次解析）。
///
/// ROSCTR 接受 Ack（0x03）与 0x07（UserData 回执），与抽取前一致。
pub fn parse_szl_response(resp: &[u8], szl_id: u16) -> Result<Vec<u8>, S7TransportError> {
    if resp.len() < 7 + 12 {
        return Err(S7TransportError::protocol(
            "SZL_SHORT",
            format!("SZL 0x{szl_id:04X} 响应过短 {}", resp.len()),
        ));
    }
    let s7 = &resp[7..];
    if s7.len() < 12 {
        return Err(S7TransportError::protocol("SZL_S7_SHORT", "SZL S7 头缺失"));
    }
    if s7[1] != S7_ROSCTR_ACK && s7[1] != 0x07 {
        // NOTE: 下标用 get（抽取前为直接下标，畸形短包会 panic；此处 fail-closed）。
        let code = s7.get(17).copied().unwrap_or(0);
        return Err(crate::error::s7_cpu_error(
            code,
            &format!("SZL 0x{szl_id:04X} 被拒绝"),
        ));
    }
    Ok(s7[12..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn szl_request_shape_matches_legacy() {
        let pkt = build_szl_request(2, 0x0011, 1);
        // S7 载荷 4+2+4+12+4=26，整包 4+3+26=33=0x21。
        assert_eq!(&pkt[0..4], &[0x03, 0x00, 0x00, 0x21]);
        assert_eq!(&pkt[7..11], &[0x32, 0x07, 0x00, 0x00]);
        assert_eq!(&pkt[11..13], &[0x00, 0x02]);
        // 尾部 SZL ID/Index。
        assert_eq!(&pkt[pkt.len() - 4..], &[0x00, 0x11, 0x00, 0x01]);
    }

    #[test]
    fn szl_ack_passthrough_and_reject() {
        // S7(10 头) + 12 字节 UserData 头 + 4 字节负载：s7[12..] 原样透传 16 字节。
        let mut s7 = vec![0x32, 0x07, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x10];
        s7.extend_from_slice(&[
            0x00, 0x01, 0x12, 0x04, 0x11, 0x44, 0x01, 0x00, 0xFF, 0x09, 0x00, 0x04,
        ]);
        s7.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let mut resp = vec![0x03, 0x00, 0x00, 0x00, 0x02, 0xF0, 0x80];
        resp.extend_from_slice(&s7);
        let out = parse_szl_response(&resp, 0x0011).unwrap();
        assert_eq!(out.len(), 14);
        assert_eq!(&out[out.len() - 4..], &[0xDE, 0xAD, 0xBE, 0xEF]);
        // 非 Ack/0x07 → 拒绝（错误码位 s7[17] 置 0x05）。
        let mut s7bad = vec![0x32, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x04];
        s7bad.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05]);
        let mut respbad = vec![0x03, 0x00, 0x00, 0x00, 0x02, 0xF0, 0x80];
        respbad.extend_from_slice(&s7bad);
        assert_eq!(
            parse_szl_response(&respbad, 0x0011).unwrap_err().code,
            "S7_0x05"
        );
    }
}
