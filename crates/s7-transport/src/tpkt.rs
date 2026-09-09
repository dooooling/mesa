//! TPKT 成帧（ISO 8073 / RFC 1006）：`TCP -> TPKT`。
//!
//! TPKT 头固定 4 字节：版本 0x03、保留、总长度（含头）。本层只做成帧，
//! 不解释载荷（COTP/S7 由上层模块负责）。

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::{S7TransportError, S7TransportErrorKind};

/// TPKT 固定版本。
pub const TPKT_VERSION: u8 = 0x03;
/// TPKT 长度校验下限（含 4 字节头）。
pub const TPKT_MIN_LEN: usize = 4;
/// TPKT 长度校验上限（S7 PDU 960 + 头远小于此，8192 为防御性上限）。
pub const TPKT_MAX_LEN: usize = 8192;

/// 为载荷加上 TPKT 头（纯函数，便于 exact-bytes 单测）。
pub fn encode_tpkt(payload: &[u8]) -> Vec<u8> {
    let len = (4 + payload.len()) as u16;
    let mut pkt = Vec::with_capacity(4 + payload.len());
    pkt.push(TPKT_VERSION);
    pkt.push(0x00);
    pkt.extend_from_slice(&len.to_be_bytes());
    pkt.extend_from_slice(payload);
    pkt
}

/// 发送一个完整 TPKT 包。
pub async fn send_packet(stream: &mut TcpStream, payload_with_tpkt: &[u8]) -> std::io::Result<()> {
    stream.write_all(payload_with_tpkt).await?;
    stream.flush().await
}

/// 读取一个完整 TPKT 包（含 4 字节头）。
///
/// 版本/长度非法即 `InvalidData`（调用方映射为协议错误）；与抽取前
/// `recv_packet` 逐字节一致。
pub async fn recv_packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).await?;
    if hdr[0] != TPKT_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("TPKT 版本异常 {:02x}，期望 {:02x}", hdr[0], TPKT_VERSION),
        ));
    }
    let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
    if !(TPKT_MIN_LEN..=TPKT_MAX_LEN).contains(&len) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("TPKT 长度非法 {len}，允许 {TPKT_MIN_LEN}..{TPKT_MAX_LEN}"),
        ));
    }
    let mut buf = vec![0u8; len];
    buf[0..4].copy_from_slice(&hdr);
    stream.read_exact(&mut buf[4..]).await?;
    Ok(buf)
}

/// `std::io::Error` → 传输错误（收发阶段由调用方传入 `code` 区分）。
pub fn map_io_error(e: std::io::Error, code: &'static str) -> S7TransportError {
    S7TransportError::new(S7TransportErrorKind::Connection, code, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tpkt_header_is_version_03_with_total_len() {
        let pkt = encode_tpkt(&[0xAA, 0xBB]);
        assert_eq!(pkt, vec![0x03, 0x00, 0x00, 0x06, 0xAA, 0xBB]);
    }
}
