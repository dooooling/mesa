//! COTP 连接管理（ISO 8073）：`CR -> CC`。
//!
//! 负责连接管理与数据 TPDU 重组；TSAP 取值由调用方决定
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

/// 收齐一个 COTP 数据消息后恢复现有 S7 解析器接受的单帧形状。
///
/// 对端可先发送空的非 EOT 数据帧，或跨 TPKT 拆分 S7 载荷；不能将
/// 第一个 TPKT 直接当作完整 S7 响应。调用方对整次重组设置超时，
/// 此处限制帧数与总字节数，避免连续空帧或超大消息造成无界接收。
pub async fn recv_data(stream: &mut tokio::net::TcpStream) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    for _ in 0..16 {
        let packet = crate::tpkt::recv_packet(stream).await?;
        if packet.len() < 7 || packet[4..6] != [0x02, COTP_DT] {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "S7 响应必须是 COTP DT，且 LI=2",
            ));
        }
        if payload.len() + packet.len() - 7 > crate::tpkt::TPKT_MAX_LEN - 7 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "COTP 重组消息超过长度上限",
            ));
        }
        payload.extend_from_slice(&packet[7..]);
        if packet[6] & 0x80 != 0 {
            let mut data = COTP_DATA_HEADER.to_vec();
            data.extend_from_slice(&payload);
            return Ok(crate::tpkt::encode_tpkt(&data));
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "COTP 消息在16帧内没有结束标志",
    ))
}

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

    async fn receive_packets(packets: Vec<Vec<u8>>) -> std::io::Result<Vec<u8>> {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let writer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for packet in packets {
                if stream.write_all(&packet).await.is_err() {
                    break;
                }
            }
        });
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(1), recv_data(&mut client))
                .await
                .unwrap();
        writer.await.unwrap();
        result
    }

    fn fragment(payload: &[u8], end: bool) -> Vec<u8> {
        let mut data = vec![2, COTP_DT, if end { 0x80 } else { 0 }];
        data.extend_from_slice(payload);
        crate::tpkt::encode_tpkt(&data)
    }

    #[tokio::test]
    async fn data_reassembles_empty_prefix_and_split_s7_payload() {
        let s7 = [
            0x32, 3, 0, 0, 0, 1, 0, 2, 0, 8, 0, 0, 4, 1, 0xff, 7, 0, 4, 0x42, 4, 0, 0,
        ];
        let result = receive_packets(vec![
            fragment(&[], false),
            fragment(&s7[..11], false),
            fragment(&s7[11..], true),
        ])
        .await
        .unwrap();
        assert_eq!(result, fragment(&s7, true));
    }

    #[tokio::test]
    async fn data_rejects_non_dt_and_bounded_fragment_overflow() {
        for packets in [
            vec![crate::tpkt::encode_tpkt(&[2, COTP_CC, 0x80])],
            vec![fragment(&[], false); 16],
            vec![
                fragment(&vec![0; 5000], false),
                fragment(&vec![0; 5000], true),
            ],
        ] {
            assert_eq!(
                receive_packets(packets).await.unwrap_err().kind(),
                std::io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn cr_exact_bytes_match_legacy() {
        // src=0x0100 dst=0x0101（rack0/slot1）：与抽取前 build_cotp_cr 逐字节一致。
        let pkt = build_cotp_cr(0x0100, 0x0101);
        assert_eq!(
            pkt,
            vec![
                0x03, 0x00, 0x00, 0x16, 0x11, 0xE0, 0x00, 0x00, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A,
                0xC1, 0x02, 0x01, 0x00, 0xC2, 0x02, 0x01, 0x01,
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
