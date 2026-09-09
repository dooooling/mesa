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

use crate::address::NckVariableRef;
use crate::catalog::{NckShape, NckVariableDefinition};
use crate::value::NckDataKind;

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
    #[error("变量 `{variable}` shape {shape:?} 与行列参数不匹配: {reason}")]
    BadShape {
        variable: String,
        shape: NckShape,
        reason: String,
    },
    #[error("count `{count}` 非法，需 1..=255（linecount 单字节）")]
    CountOutOfRange { count: u16 },
    #[error("变量 `{variable}` catalog 类型 `{data_type}` 与线缆元素 {element_size} 字节不一致")]
    TypeWireMismatch {
        variable: String,
        data_type: String,
        element_size: usize,
    },
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

/// 语义 → 线缆（组装规则 V1，NOTE 待真机确认的假设）：
///
/// - `syntax`：`unit_mode` 直映（0x82/83/84）；
/// - `area`：字母索引直映；
/// - `unit`：`area_no` 直填（缺省 0；>31 拒绝）。NOTE：unit 字段的确切语义
///   （通道/轴号直填 vs 固定 0）待真机抓包确认，当前取最直接的解释；
/// - `column`：用户 `column` 优先，缺省用 catalog 默认（字段选择 vs 实例选择
///   的二分待确认，覆盖规则显式可测）；
/// - `line`：用户 `line`，缺省 0；
/// - `line_count`：用户 `count`（1..=255；NCK linecount 单字节，>255 拒绝）；
/// - `module`：catalog；
/// - shape 校验：Scalar 禁 line/column；Lines 必须 line；LinesAndColumns
///   必须 line + column（任一来自用户显式值；catalog 默认 column 计入）。
///   NOTE：严格性待真机放宽/收紧，只调此处。
/// - 类型-线缆一致性：`kind` 字节长必须等于 `element_size`（防 catalog 笔误）；
/// - 响应期望：`expected_transport_size` 取 catalog `wire.transport_size`
///   （P1-4：响应 transport/长度逐项校验，不符即 BAD，不猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedVariable {
    pub wire: NckWireAddress,
    pub kind: NckDataKind,
    pub expected_data_len: usize,
    pub expected_transport_size: u8,
}

pub fn resolve(
    r: &NckVariableRef,
    def: &NckVariableDefinition,
) -> Result<ResolvedVariable, CodecError> {
    let kind = NckDataKind::parse(&def.data_type).map_err(|_| CodecError::TypeWireMismatch {
        variable: def.variable.clone(),
        data_type: def.data_type.clone(),
        element_size: def.wire.element_size,
    })?;
    if kind.byte_len() != def.wire.element_size {
        return Err(CodecError::TypeWireMismatch {
            variable: def.variable.clone(),
            data_type: def.data_type.clone(),
            element_size: def.wire.element_size,
        });
    }
    // shape 校验（column 缺省计入：catalog 默认 column 视为已提供）。
    let column = r.column.unwrap_or(def.wire.column);
    let column_given = r.column.is_some() || def.wire.column != 0;
    match def.shape {
        NckShape::Scalar => {
            if r.line.is_some() || r.column.is_some() {
                return Err(bad_shape(def, "标量变量不应带 line/column"));
            }
        }
        NckShape::Lines => {
            if r.line.is_none() {
                return Err(bad_shape(def, "行数组变量必须带 line"));
            }
        }
        NckShape::LinesAndColumns => {
            if r.line.is_none() || !column_given {
                return Err(bad_shape(def, "矩阵变量必须带 line + column"));
            }
        }
    }
    let unit = r.area_no.unwrap_or(0);
    let area_unit = NckWireAddress::area_unit(r.area.index(), unit)?;
    if r.count > 255 {
        return Err(CodecError::CountOutOfRange { count: r.count });
    }
    let wire = NckWireAddress {
        syntax_id: r.unit_mode.syntax_id(),
        area_unit,
        column,
        line: r.line.unwrap_or(0),
        module: def.wire.module,
        line_count: r.count as u8,
    };
    let expected_data_len = (r.count as usize)
        .checked_mul(def.wire.element_size)
        .expect("count×element_size 上溢（count≤255，element 有界）");
    Ok(ResolvedVariable {
        wire,
        kind,
        expected_data_len,
        expected_transport_size: def.wire.transport_size,
    })
}

fn bad_shape(def: &NckVariableDefinition, reason: &str) -> CodecError {
    CodecError::BadShape {
        variable: def.variable.clone(),
        shape: def.shape,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::NckArea;

    /// 合成 catalog 条目（脚手架数值，非 Siemens 语义）。
    fn def(shape: NckShape, column: u16) -> NckVariableDefinition {
        NckVariableDefinition {
            area: NckArea::Channel,
            block: "SEMA".into(),
            variable: "actFeedRate".into(),
            data_type: "F64".into(),
            shape,
            unit: None,
            wire: crate::catalog::NckWireDefinition {
                module: 0x12,
                column,
                transport_size: 0x04,
                element_size: 8,
            },
            supported_families: vec![],
        }
    }

    fn r(json: serde_json::Value) -> NckVariableRef {
        NckVariableRef::from_parameters(&json.as_object().cloned().unwrap()).unwrap()
    }

    #[test]
    fn encode_exact_bytes_all_syntaxes() {
        // area C=2, unit 1 → areaunit = 2<<5|1 = 0x41；column 0x002A；
        // line 3；module 0x12；linecount 1。
        let wire = NckWireAddress {
            syntax_id: 0x82,
            area_unit: 0x41,
            column: 42,
            line: 3,
            module: 0x12,
            line_count: 1,
        };
        assert_eq!(
            encode_var_spec(&wire),
            vec![0x12, 0x08, 0x82, 0x41, 0x00, 0x2A, 0x00, 0x03, 0x12, 0x01]
        );
        for (syntax, mode) in [(0x82, "current"), (0x83, "metric"), (0x84, "inch")] {
            let rr = r(serde_json::json!({
                "area": "C", "area_no": 1, "block": "SEMA",
                "variable": "actFeedRate", "line": 3, "unit_mode": mode,
            }));
            let d = def(NckShape::Lines, 42);
            let rv = resolve(&rr, &d).unwrap();
            let w = rv.wire;
            assert_eq!(w.syntax_id, syntax, "{mode}");
            assert_eq!(
                encode_var_spec(&w),
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

    #[test]
    fn column_default_and_override_rule() {
        // 缺省用 catalog；用户显式覆盖。
        let d = def(NckShape::LinesAndColumns, 42);
        let rr = r(serde_json::json!({"area": "C", "block": "S", "variable": "v", "line": 1}));
        let rv = resolve(&rr, &d).unwrap();
        let w = rv.wire;
        assert_eq!((w.column, w.line), (42, 1));
        assert_eq!(rv.expected_transport_size, d.wire.transport_size);
        let rr2 = r(
            serde_json::json!({"area": "C", "block": "S", "variable": "v", "line": 1, "column": 7}),
        );
        let w2 = resolve(&rr2, &d).unwrap().wire;
        assert_eq!((w2.column, w2.line), (7, 1));
    }

    #[test]
    fn shape_validation_is_strict_v1() {
        let scalar = def(NckShape::Scalar, 0);
        // 标量带 line 拒绝。
        let bad = r(serde_json::json!({"area": "N", "block": "N", "variable": "v", "line": 1}));
        assert!(resolve(&bad, &scalar).is_err());
        // 行数组缺 line 拒绝。
        let lines = def(NckShape::Lines, 0);
        let bad2 = r(serde_json::json!({"area": "C", "block": "S", "variable": "v"}));
        assert!(resolve(&bad2, &lines).is_err());
        // 标量无行列通过，line 缺省 0。
        let ok = r(serde_json::json!({"area": "N", "block": "N", "variable": "v"}));
        let rv = resolve(&ok, &scalar).unwrap();
        assert_eq!(
            (rv.wire.line, rv.wire.column, rv.expected_data_len),
            (0, 0, 8)
        );
    }

    #[test]
    fn type_wire_mismatch_fails_closed() {
        // catalog 称 F64（8 字节）但 element_size 填 4 → 拒绝（防笔误）。
        let mut d = def(NckShape::Scalar, 0);
        d.wire.element_size = 4;
        let ok = r(serde_json::json!({"area": "N", "block": "N", "variable": "v"}));
        assert!(resolve(&ok, &d).is_err());
        // 未知 data_type 拒绝。
        let mut d2 = def(NckShape::Scalar, 0);
        d2.data_type = "STRUCT".into();
        assert!(resolve(&ok, &d2).is_err());
    }

    #[test]
    fn count_scales_expected_len() {
        let d = def(NckShape::Lines, 0);
        let rr = r(
            serde_json::json!({"area": "C", "block": "S", "variable": "v", "line": 1, "count": 3}),
        );
        let rv = resolve(&rr, &d).unwrap();
        assert_eq!(rv.wire.line_count, 3);
        assert_eq!(rv.expected_data_len, 24);
    }
}
