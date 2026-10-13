//! NCK wire codec（ADR 0001 §15：`NckWireAddress ↔ S7 variable specification`）。
//!
//! 线缆布局唯一依据：Wireshark `epan/dissectors/packet-s7comm.c`
//! `s7comm_decode_param_item`（分发：`type == 0x12 && length == 8 &&
//! syntax ∈ {0x82,0x83,0x84}`）+ `s7comm_syntaxid_nck`（字段顺序与宽度）。
//! item 字节（共 10：2 头 + 8 体）：
//! ```text
//! 0x12 0x08 <syntax> <areaunit> <column:u16BE> <line:u16BE> <module:u8> <linecount:u8>
//! areaunit = area<<5 | unit（aaauuuuu；area 0=N..7=MMC，unit 5 位）
//! ```
//!
//! 本模块只负责“结构↔字节”（含 exact-bytes 可测）；“语义→结构”的组装规则
//! （area_no→unit、column 默认/覆盖、shape 校验）见 `resolve`，其 NOTE 标出
//! 待真机确认的假设——调规则不动本模块，不动传输层。
//!
//! Codec 不负责：TCP/COTP/PDU/重连（transport）、Value 解码（`value`）、
//! Catalog（`catalog`）、地址字母表（`address`）。

use thiserror::Error;

// ---------------------------------------------------------------------------
// 常量（Wireshark 实锤值，解释“为什么”）
// ---------------------------------------------------------------------------

/// 变量规范类型固定 `0x12`（与 S7ANY 同）。
pub const NCK_VAR_SPEC_TYPE: u8 = 0x12;
/// NCK 规范体长度固定 8（S7ANY 为 10；Wireshark 分发条件）。
pub const NCK_VAR_SPEC_LEN: u8 = 0x08;
/// NCK 规范总长 10 字节（2 头 + 8 体）。
pub const NCK_VAR_SPEC_TOTAL: usize = 10;
/// area 占高 3 位。
pub const NCK_AREA_SHIFT: u8 = 5;
/// area 最大值 7（MMC；用户字母 V1 只到 H=6，见 address）。
pub const NCK_AREA_MAX: u8 = 7;
/// unit 占低 5 位，最大值 31。
pub const NCK_UNIT_MAX: u8 = 31;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CodecError {
    #[error("NCK area 索引 `{index}` 非法，需 0..=7")]
    BadArea { index: u8 },
    #[error("NCK unit `{unit}` 非法，需 0..=31")]
    BadUnit { unit: u16 },
}

/// 线缆地址（字节结构体的结构化形态；`area_unit` 为合成字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NckWireAddress {
    /// Syntax ID：`0x82` current / `0x83` metric / `0x84` inch。
    pub syntax_id: u8,
    /// 合成字节：`area<<5 | unit`。
    pub area_unit: u8,
    pub column: u16,
    pub line: u16,
    pub module: u8,
    pub line_count: u8,
}

impl NckWireAddress {
    /// 合成 areaunit 字节（area ≤ 7、unit ≤ 31，超限 fail-closed）。
    pub fn area_unit(area_index: u8, unit: u16) -> Result<u8, CodecError> {
        if area_index > NCK_AREA_MAX {
            return Err(CodecError::BadArea { index: area_index });
        }
        if unit > NCK_UNIT_MAX as u16 {
            return Err(CodecError::BadUnit { unit });
        }
        Ok((area_index << NCK_AREA_SHIFT) | (unit as u8))
    }
}

/// 编码 10 字节变量规范（exact-bytes，顺序与宽度照搬 Wireshark 解码逆序）。
pub fn encode_var_spec(wire: &NckWireAddress) -> Vec<u8> {
    let mut v = Vec::with_capacity(NCK_VAR_SPEC_TOTAL);
    v.push(NCK_VAR_SPEC_TYPE);
    v.push(NCK_VAR_SPEC_LEN);
    v.push(wire.syntax_id);
    v.push(wire.area_unit);
    v.extend_from_slice(&wire.column.to_be_bytes());
    v.extend_from_slice(&wire.line.to_be_bytes());
    v.push(wire.module);
    v.push(wire.line_count);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_exact_bytes_all_syntaxes() {
        // area C=2, unit 1 → areaunit = 2<<5|1 = 0x41；column 0x002A；
        // line 3；module 0x12；linecount 1。三种 syntax 只差第 3 字节。
        for syntax in [0x82u8, 0x83, 0x84] {
            let wire = NckWireAddress {
                syntax_id: syntax,
                area_unit: 0x41,
                column: 42,
                line: 3,
                module: 0x12,
                line_count: 1,
            };
            assert_eq!(
                encode_var_spec(&wire),
                vec![0x12, 0x08, syntax, 0x41, 0x00, 0x2A, 0x00, 0x03, 0x12, 0x01]
            );
        }
    }

    #[test]
    fn area_unit_bit_math_and_limits() {
        assert_eq!(NckWireAddress::area_unit(2, 1).unwrap(), 0x41);
        assert_eq!(NckWireAddress::area_unit(0, 0).unwrap(), 0x00);
        assert_eq!(NckWireAddress::area_unit(7, 31).unwrap(), 0xFF);
        assert!(NckWireAddress::area_unit(8, 0).is_err());
        assert!(NckWireAddress::area_unit(2, 32).is_err());
    }
}
