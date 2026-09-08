//! S7ANY 变量规范编码（Syntax ID 0x10，方案 §7.1 地址型）。
//!
//! 职责边界：本模块是 `s7` Driver 侧唯一的地址语义→线缆字节翻译点。
//! 编码 12 字节规范 `0x12 0x0A 0x10 transport req_len db area addr24`，
//! transport/request_len 映射（含 C/T 计数器/定时器的字寻址特殊）与抽取前
//! `build_read_req`/`build_bulk_read_req`/`build_write_req` 内联逻辑逐项一致。
//! 编出字节即为不透明 `var_spec`，交 `mesa-s7-transport` 发送；NCK 的
//! `0x82/83/84` 规范由 `sinumerik-nck` 另行编码，绝不进本文件。

use crate::address::{Area, S7Address};
use crate::codec::S7Kind;
use mesa_s7_transport::{S7_SYNTAX_ID_S7ANY, S7ReadVarItem};

/// S7ANY 规范固定长度 12 字节。
pub const S7ANY_SPEC_LEN: usize = 12;
/// 批量连续读的传输尺寸：0x02=BYTE（C/T 除外）。
pub const S7ANY_TRANSPORT_BYTE: u8 = 0x02;
/// C/T 计数器/定时器的传输尺寸（字寻址，个数单位）。
pub const S7ANY_TRANSPORT_COUNTER: u8 = 0x1C;
/// T 定时器的传输尺寸。
pub const S7ANY_TRANSPORT_TIMER: u8 = 0x1D;

/// 由区域与类型决定 transport/request_len（C/T 字寻址特殊与抽取前一致）。
///
/// C/T 为字寻址但读写计数单位为“个数”而非字节，WORD 通用 2 会触发 CPU 0x06，
/// 故 transport 取 0x1C/0x1D、长度取 1。
fn spec_params(area: Area, kind: S7Kind) -> (u8, u16) {
    match area {
        Area::Counter => (S7ANY_TRANSPORT_COUNTER, 1),
        Area::Timer => (S7ANY_TRANSPORT_TIMER, 1),
        _ => (kind.transport_size(), kind.request_len()),
    }
}

/// 编码 12 字节 S7ANY 规范（纯函数，exact-bytes 可测）。
pub fn encode_s7any(area_code: u8, db: u16, bit_addr: u32, transport: u8, req_len: u16) -> Vec<u8> {
    let mut spec = Vec::with_capacity(S7ANY_SPEC_LEN);
    spec.extend_from_slice(&[0x12, 0x0A, S7_SYNTAX_ID_S7ANY, transport]);
    spec.extend_from_slice(&req_len.to_be_bytes());
    spec.extend_from_slice(&db.to_be_bytes());
    spec.push(area_code);
    spec.push(((bit_addr >> 16) & 0xFF) as u8);
    spec.push(((bit_addr >> 8) & 0xFF) as u8);
    spec.push((bit_addr & 0xFF) as u8);
    spec
}

/// 单点读项 → 传输项（transport/request_len 按类型，期望长度按 `byte_len`）。
pub fn encode_read_item(addr: &S7Address, kind: S7Kind) -> S7ReadVarItem {
    let (transport, req_len) = spec_params(addr.area, kind);
    S7ReadVarItem {
        var_spec: encode_s7any(
            addr.area.code(),
            addr.db_number,
            addr.bit_address(),
            transport,
            req_len,
        ),
        expected_data_len: kind.byte_len(),
    }
}

/// 连续区批量项 → 传输项（恒 BYTE 批量，C/T 保持字寻址特殊）。
pub fn encode_bulk_item(addr: &S7Address, len: usize) -> S7ReadVarItem {
    let (transport, req_len) = match addr.area {
        Area::Counter => (S7ANY_TRANSPORT_COUNTER, 1),
        Area::Timer => (S7ANY_TRANSPORT_TIMER, 1),
        _ => (S7ANY_TRANSPORT_BYTE, len as u16),
    };
    S7ReadVarItem {
        var_spec: encode_s7any(
            addr.area.code(),
            addr.db_number,
            addr.bit_address(),
            transport,
            req_len,
        ),
        expected_data_len: len,
    }
}

/// 写项规范 → 12 字节（transport/request_len 与读同口径）。
pub fn encode_write_spec(addr: &S7Address, kind: S7Kind) -> Vec<u8> {
    let (transport, req_len) = spec_params(addr.area, kind);
    encode_s7any(
        addr.area.code(),
        addr.db_number,
        addr.bit_address(),
        transport,
        req_len,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::parse_address;

    #[test]
    fn db_dword_spec_matches_legacy_layout() {
        // DB10.DBD20 REAL：area 0x84，db 10，bit_addr=(20*8)=160=0xA0；
        // transport BYTE(0x02)，req_len 4。
        let addr = parse_address("DB10.DBD20").unwrap();
        let spec = encode_s7any(
            addr.area.code(),
            addr.db_number,
            addr.bit_address(),
            0x02,
            4,
        );
        assert_eq!(
            spec,
            vec![
                0x12, 0x0A, 0x10, 0x02, 0x00, 0x04, 0x00, 0x0A, 0x84, 0x00, 0x00, 0xA0
            ]
        );
    }

    #[test]
    fn counter_timer_keep_word_addressing_special() {
        let c = parse_address("C1").unwrap();
        let (t, l) = spec_params(c.area, S7Kind::Word);
        assert_eq!((t, l), (0x1C, 1));
        let tm = parse_address("T2").unwrap();
        let (t, l) = spec_params(tm.area, S7Kind::Word);
        assert_eq!((t, l), (0x1D, 1));
        // 普通 M 区不受影响。
        let m = parse_address("MB10").unwrap();
        let (t, l) = spec_params(m.area, S7Kind::Byte);
        assert_eq!((t, l), (0x02, 1));
    }

    #[test]
    fn read_item_expected_len_follows_kind() {
        let addr = parse_address("DB10.DBD0").unwrap();
        let it = encode_read_item(&addr, S7Kind::Real);
        assert_eq!(it.var_spec.len(), 12);
        assert_eq!(it.expected_data_len, 4);
        let s = parse_address("DB10.DBD0").unwrap();
        let str_item = encode_read_item(&s, S7Kind::String);
        assert_eq!(str_item.expected_data_len, 256);
    }
}
