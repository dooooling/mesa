//! S7 WriteVar 单项写（功能码 0x05）。
//!
//! Data 负载 framing（`0x00 0x04 + bit 长 + 数据 + 奇长补齐`）是 S7Comm
//! 线缆规则，归本层；变量规范与值字节的大端编码由调用方负责。
//! NOTE: V1 仅 `s7` Driver 的最小闭环用此路径（INT/WORD），NCK V1 不开放写。

use crate::error::S7TransportError;
use crate::pdu::{S7_FUNC_WRITE, S7_ROSCTR_JOB, check_ack, s7_header, wrap_s7};

/// S7 写 Data 项的传输尺寸：0x04=BYTE/WORD/DWORD（长度按 bit 计）。
pub const S7_WRITE_TRANSPORT_BYTE: u8 = 0x04;

/// 构造 WriteVar 请求：param（0x05 0x01 + var_spec）+ data（transport + bit 长 + 值）。
///
/// `data` 为值原始字节（如 INT 的 2 字节大端）；奇长 data 自动补 `0x00`
/// （Write DATA 项偶对齐），与抽取前逐字节一致。
pub fn build_write_request(pdu_ref: u16, var_spec: &[u8], data: &[u8]) -> Vec<u8> {
    let mut param = Vec::with_capacity(2 + var_spec.len());
    param.push(S7_FUNC_WRITE);
    param.push(1);
    param.extend_from_slice(var_spec);
    let param_len = param.len() as u16;
    let bit_len = (data.len() * 8) as u16;
    let mut s7_data = Vec::with_capacity(4 + data.len());
    s7_data.extend_from_slice(&[0x00, S7_WRITE_TRANSPORT_BYTE]);
    s7_data.extend_from_slice(&bit_len.to_be_bytes());
    s7_data.extend_from_slice(data);
    if s7_data.len() % 2 == 1 {
        s7_data.push(0);
    }
    let data_len = s7_data.len() as u16;
    let mut s7 = Vec::with_capacity(10 + param.len() + s7_data.len());
    s7.extend_from_slice(&s7_header(S7_ROSCTR_JOB, pdu_ref, param_len, data_len));
    s7.extend_from_slice(&param);
    s7.extend_from_slice(&s7_data);
    wrap_s7(&s7)
}

/// 解析 Write Ack：ROSCTR 确认 + 首个 data 项返回码 0xFF 即成功。
///
/// 逐项返回码的定位（`s7[12+param_len]`）与抽取前一致。
pub fn parse_write_response(resp: &[u8]) -> Result<(), S7TransportError> {
    if resp.len() < 7 + 12 {
        return Err(S7TransportError::protocol(
            "WRITE_SHORT",
            format!("Write响应过短 {}", resp.len()),
        ));
    }
    let s7 = &resp[7..];
    check_ack(s7, &format!("Write被拒绝 rosctr={:02x}", s7.get(1).copied().unwrap_or(0)))?;
    if s7.len() < 14 {
        return Err(S7TransportError::protocol(
            "WRITE_S7_SHORT",
            "S7 write ack过短",
        ));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_start = 12 + param_len;
    if s7.len() <= data_start {
        return Err(S7TransportError::protocol(
            "WRITE_NO_DATA",
            "Write ack无Data",
        ));
    }
    let ret = s7[data_start];
    if ret != crate::error::S7_ITEM_OK {
        return Err(crate::error::s7_cpu_error(
            ret,
            &format!("Write item ret {ret:02x}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_request_shape_matches_legacy() {
        // 12 字节规范 + INT 2 字节：param=14，data=2+4=6（偶数不补齐）。
        let pkt = build_write_request(3, &[0x12; 12], &[0x00, 0x7B]);
        // 总长 4+3+10+14+6 = 37 = 0x25。
        assert_eq!(&pkt[0..4], &[0x03, 0x00, 0x00, 0x25]);
        assert_eq!(
            &pkt[7..17],
            &[0x32, 0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x0E, 0x00, 0x06]
        );
        assert_eq!(&pkt[17..19], &[0x05, 0x01]);
        // data 区：00 04 + bit_len(16) + 值。
        assert_eq!(&pkt[31..37], &[0x00, 0x04, 0x00, 0x10, 0x00, 0x7B]);
    }

    #[test]
    fn write_ack_ok_and_item_bad() {
        // 成功 Ack：S7(12)+param(14)+data(1: 0xFF)。
        let mut s7 = vec![0x32, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0E, 0x00, 0x01];
        s7.extend_from_slice(&[0x00, 0x00]);
        s7.extend_from_slice(&[0x05, 0x01]);
        s7.extend_from_slice(&[0x12; 12]);
        s7.push(0xFF);
        let mut resp = vec![0x03, 0x00, 0x00, 0x00, 0x02, 0xF0, 0x80];
        resp.extend_from_slice(&s7);
        assert!(parse_write_response(&resp).is_ok());
        // 项返回 0x05 → S7_0x05。
        let mut bad = resp.clone();
        *bad.last_mut().unwrap() = 0x05;
        assert_eq!(parse_write_response(&bad).unwrap_err().code, "S7_0x05");
    }
}
