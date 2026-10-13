//! 身份浏览的内建只读探针（`IdentityProbeSpec`）。
//!
//! 为什么需要独立定义：身份核对（轴名、驱动号、输出类型、主轴归属、驱动对象名）
//! 是驱动的**内建诊断程序**，不是需要用户安装的变量目录，也不构成
//! “允许读取哪些采集地址”的白名单。把探针参数内建后，删除 Catalog
//! 不会连带删掉身份浏览能力。
//!
//! 数值来源与核对方式：
//! - 参数取自已冻结的 `catalog/828d.json` 中经真机/证据核对过的条目
//!   （module/column/decode/transport），**不按变量名重新推导数字**；
//! - `p0199` 的请求字节有**捕获证据**：
//!   `docs/evidence/nck-drive-object-readback.json` 中
//!   `module26_unit1_object_name` = `12 08 82 a1 00 c7 00 01 1a 19`、
//!   `module130_unit1_object_name` = `12 08 82 a1 00 c7 00 01 82 19`；
//! - A/C 区探针没有留存抓包，其期望字节由已 golden 覆盖的 10 字节
//!   布局推导（`codec::encode_var_spec` 有独立 exact-bytes 测试），
//!   本模块测试明确区分“捕获”与“布局推导”两类，不把后者冒充为抓包证据；
//! - 实体机行为另有只读实测记录：`docs/evidence/nck-live-identity-probe.txt`
//!   （轴名 `MX1..MQ1`、`MD30130` 轴 6 = 0、`MD35000` 轴 4 = 1 等）。
//!
//! 本模块不做：Catalog 查询、用户寻址、采集准入判断。

use crate::address::{NckArea, NckUnitMode};
use crate::client::NckReadItem;
use crate::codec::{CodecError, NckWireAddress};
use crate::value::NckDataKind;

/// 探针中的可变槽位：固定值或“本次选定的实例/索引”。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeSlot {
    /// 固定值（如通道号 1、驱动对象行 1）。
    Fixed(u16),
    /// 展开轴/对象时才填入本次选定的编号。
    TargetIndex,
}

impl ProbeSlot {
    fn value(self, target: u16) -> u16 {
        match self {
            ProbeSlot::Fixed(v) => v,
            ProbeSlot::TargetIndex => target,
        }
    }
}

/// 单个内建只读探针。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityProbeSpec {
    /// 诊断标签（不是用户寻址用的变量名）。
    pub name: &'static str,
    pub area: NckArea,
    pub area_no: ProbeSlot,
    pub module: u8,
    pub column: u16,
    pub line: ProbeSlot,
    pub count: u8,
    pub unit_mode: NckUnitMode,
    pub decode: NckDataKind,
    /// 期望设备响应 transport 字段。
    pub transport_size: u8,
}

impl IdentityProbeSpec {
    /// 构造线缆地址 + 读取项（严格校验实例槽位范围）。
    pub fn resolve(&self, target: u16) -> Result<(NckWireAddress, NckReadItem), CodecError> {
        let area_no = self.area_no.value(target);
        let line = self.line.value(target);
        let wire = NckWireAddress {
            syntax_id: self.unit_mode.syntax_id(),
            area_unit: NckWireAddress::area_unit(self.area.index(), area_no)?,
            column: self.column,
            line,
            module: self.module,
            line_count: self.count,
        };
        let expected_data_len = usize::from(self.count) * self.decode.byte_len();
        Ok((
            wire,
            NckReadItem {
                wire,
                expected_data_len,
                expected_transport_size: self.transport_size,
            },
        ))
    }
}

// ---------------------------------------------------------------------------
// 828D 已核对探针集合（参数与 catalog 已冻结条目一致）
// ---------------------------------------------------------------------------

/// 机床轴名表（`N/1/MDSA_AXCONF_MACHAX_NAME_TAB`，line = 机床轴号）。
pub const AXIS_NAME: IdentityProbeSpec = IdentityProbeSpec {
    name: "machine_axis_name",
    area: NckArea::Nck,
    area_no: ProbeSlot::Fixed(1),
    module: 26,
    column: 10000,
    line: ProbeSlot::TargetIndex,
    count: 1,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::Utf8Fixed16,
    transport_size: 0x09,
};

/// 轴驱动号（`A/<axis>/MDCA_CTRLOUT_MODULE_NR`）。
pub const AXIS_CTRLOUT_MODULE_NR: IdentityProbeSpec = IdentityProbeSpec {
    name: "axis_ctrlout_module_nr",
    area: NckArea::Axis,
    area_no: ProbeSlot::TargetIndex,
    module: 26,
    column: 30110,
    line: ProbeSlot::Fixed(1),
    count: 1,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::U8,
    transport_size: 0x09,
};

/// 轴输出类型（`A/<axis>/MDCA_CTRLOUT_TYPE`；0 = 无驱动输出）。
pub const AXIS_CTRLOUT_TYPE: IdentityProbeSpec = IdentityProbeSpec {
    name: "axis_ctrlout_type",
    area: NckArea::Axis,
    area_no: ProbeSlot::TargetIndex,
    module: 26,
    column: 30130,
    line: ProbeSlot::Fixed(1),
    count: 1,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::U8,
    transport_size: 0x09,
};

/// 轴编码器驱动号（`A/<axis>/MDCA_ENC_MODULE_NR`）。
pub const AXIS_ENC_MODULE_NR: IdentityProbeSpec = IdentityProbeSpec {
    name: "axis_enc_module_nr",
    area: NckArea::Axis,
    area_no: ProbeSlot::TargetIndex,
    module: 26,
    column: 30220,
    line: ProbeSlot::Fixed(1),
    count: 1,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::U8,
    transport_size: 0x09,
};

/// 主轴归属（`A/<axis>/SPIND_ASSIGN_TO_MACHAX`；非 0 = 该轴为第 n 主轴）。
pub const AXIS_SPIND_ASSIGN: IdentityProbeSpec = IdentityProbeSpec {
    name: "axis_spind_assign",
    area: NckArea::Axis,
    area_no: ProbeSlot::TargetIndex,
    module: 26,
    column: 35000,
    line: ProbeSlot::Fixed(1),
    count: 1,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::U8,
    transport_size: 0x09,
};

/// 通道位置→机床轴号（`C/1/AXCONF_MACHAX_USED`，line = 通道位置）。
pub const CHANNEL_MACHAX_USED: IdentityProbeSpec = IdentityProbeSpec {
    name: "channel_machax_used",
    area: NckArea::Channel,
    area_no: ProbeSlot::Fixed(1),
    module: 26,
    column: 20070,
    line: ProbeSlot::TargetIndex,
    count: 1,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::U8,
    transport_size: 0x09,
};

/// 驱动对象名（`V/<object>/p0199`，NC 变量入口 module 26，25×U16BE = 50B）。
pub const DRIVE_OBJECT_NAME: IdentityProbeSpec = IdentityProbeSpec {
    name: "drive_object_name",
    area: NckArea::FeedDrive,
    area_no: ProbeSlot::TargetIndex,
    module: 26,
    column: 199,
    line: ProbeSlot::Fixed(1),
    count: 25,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::Utf16Be,
    transport_size: 0x06,
};

/// 驱动对象名（`V/<object>/p0199`，显式驱动对象入口 module 130）。
///
/// 两条入口的实例编号**不同**（实体机实测：module 26 与 module 130 对同一
/// `V/n` 返回不同对象名），因此必须作为两个独立探针存在，不能互相换算。
pub const DRIVE_OBJECT_NAME_M130: IdentityProbeSpec = IdentityProbeSpec {
    name: "drive_object_name_module130",
    area: NckArea::FeedDrive,
    area_no: ProbeSlot::TargetIndex,
    module: 130,
    column: 199,
    line: ProbeSlot::Fixed(1),
    count: 25,
    unit_mode: NckUnitMode::Current,
    decode: NckDataKind::Utf16Be,
    transport_size: 0x06,
};

/// 828D 已核对的全部身份探针。
pub const PROBES_828D: [IdentityProbeSpec; 8] = [
    AXIS_NAME,
    AXIS_CTRLOUT_MODULE_NR,
    AXIS_CTRLOUT_TYPE,
    AXIS_ENC_MODULE_NR,
    AXIS_SPIND_ASSIGN,
    CHANNEL_MACHAX_USED,
    DRIVE_OBJECT_NAME,
    DRIVE_OBJECT_NAME_M130,
];

/// 身份浏览是否可用（未取得真实证据的系列不得套用 828D 探针）。
///
/// 目前只有 `828d` 有已核对的探针数值；其余系列返回 `None`，
/// 由调用方明确提示“该系列身份浏览待验证”，而不是静默套用。
pub fn probes_for_family(family: &str) -> Option<&'static [IdentityProbeSpec]> {
    match family {
        "828d" => Some(&PROBES_828D),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(spec: &IdentityProbeSpec, target: u16) -> Vec<u8> {
        let (wire, _) = spec.resolve(target).unwrap();
        crate::codec::encode_var_spec(&wire)
    }

    /// 捕获证据（`docs/evidence/nck-drive-object-readback.json`）：
    /// module26_unit1_object_name 与 module130_unit1_object_name 的原始请求。
    #[test]
    fn p0199_request_bytes_match_captured_evidence() {
        assert_eq!(
            bytes(&DRIVE_OBJECT_NAME, 1),
            vec![0x12, 0x08, 0x82, 0xa1, 0x00, 0xc7, 0x00, 0x01, 0x1a, 0x19],
            "module26 捕获字节"
        );
        assert_eq!(
            bytes(&DRIVE_OBJECT_NAME_M130, 1),
            vec![0x12, 0x08, 0x82, 0xa1, 0x00, 0xc7, 0x00, 0x01, 0x82, 0x19],
            "module130 捕获字节"
        );
    }

    /// 捕获证据中的 r106/r35 探针（V/1，module 26）字节形态。
    #[test]
    fn captured_v_area_layout_matches_codec() {
        // 用同一 V/1 槽位构造 r106（col 0x6a）与 r35（col 0x23）形态，
        // 与捕获的 `12 08 82 a1 00 6a 00 01 1a 01` / `...00 23...` 对齐。
        for (column, expect) in [
            (
                106u16,
                vec![0x12, 0x08, 0x82, 0xa1, 0x00, 0x6a, 0x00, 0x01, 0x1a, 0x01],
            ),
            (
                35u16,
                vec![0x12, 0x08, 0x82, 0xa1, 0x00, 0x23, 0x00, 0x01, 0x1a, 0x01],
            ),
        ] {
            let spec = IdentityProbeSpec {
                decode: NckDataKind::F32,
                transport_size: 0x07,
                count: 1,
                ..DRIVE_OBJECT_NAME
            };
            let spec = IdentityProbeSpec { column, ..spec };
            assert_eq!(bytes(&spec, 1), expect, "column={column}");
        }
    }

    /// 布局推导（无抓包）：A/C 区探针的期望字节，标注为推导而非捕获。
    #[test]
    fn axis_and_channel_probe_layout_derived() {
        // A/1，module 26，col 30110 → 0x75 0x9e；area A(3)<<5 | 1 = 0x61。
        assert_eq!(
            bytes(&AXIS_CTRLOUT_MODULE_NR, 1),
            vec![0x12, 0x08, 0x82, 0x61, 0x75, 0x9e, 0x00, 0x01, 0x1a, 0x01]
        );
        // C/1，col 20070 → 0x4e 0x66；area C(2)<<5 | 1 = 0x41。
        assert_eq!(
            bytes(&CHANNEL_MACHAX_USED, 1),
            vec![0x12, 0x08, 0x82, 0x41, 0x4e, 0x66, 0x00, 0x01, 0x1a, 0x01]
        );
        // N/1，col 10000 → 0x27 0x10；area N(0)<<5 | 1 = 0x01。
        assert_eq!(
            bytes(&AXIS_NAME, 1),
            vec![0x12, 0x08, 0x82, 0x01, 0x27, 0x10, 0x00, 0x01, 0x1a, 0x01]
        );
    }

    /// 槽位：TargetIndex 进 area_no 或 line，随探针语义而定。
    #[test]
    fn target_index_fills_the_right_slot() {
        // 轴探针：target 进 area_no，line 恒 1。
        let (w, _) = AXIS_CTRLOUT_MODULE_NR.resolve(7).unwrap();
        assert_eq!(w.area_unit, (3 << 5) | 7);
        assert_eq!(w.line, 1);
        // 轴名/通道探针：target 进 line，area_no 恒 1。
        let (w, _) = AXIS_NAME.resolve(5).unwrap();
        assert_eq!(w.area_unit, 1, "area N(0)<<5 | unit 1");
        assert_eq!(w.line, 5);
        let (w, _) = CHANNEL_MACHAX_USED.resolve(8).unwrap();
        assert_eq!(w.line, 8);
        // 驱动对象：target 进 area_no。
        let (w, _) = DRIVE_OBJECT_NAME.resolve(6).unwrap();
        assert_eq!(w.area_unit, (5 << 5) | 6);
    }

    /// 实例槽位越界 fail-closed（unit 5 位上限 31）。
    #[test]
    fn target_index_out_of_range_is_rejected() {
        assert!(AXIS_CTRLOUT_MODULE_NR.resolve(32).is_err());
        assert!(AXIS_CTRLOUT_MODULE_NR.resolve(31).is_ok());
    }

    /// 期望长度与 transport 与 catalog 冻结值一致。
    #[test]
    fn expected_lengths_and_transports() {
        let (_, item) = DRIVE_OBJECT_NAME.resolve(1).unwrap();
        assert_eq!(item.expected_data_len, 50, "25×U16BE");
        assert_eq!(item.expected_transport_size, 0x06);
        let (_, item) = AXIS_NAME.resolve(1).unwrap();
        assert_eq!(item.expected_data_len, 16, "UTF8_FIXED_16");
        assert_eq!(item.expected_transport_size, 0x09);
        let (_, item) = AXIS_CTRLOUT_TYPE.resolve(1).unwrap();
        assert_eq!(item.expected_data_len, 1);
        assert_eq!(item.expected_transport_size, 0x09);
    }

    /// 未取得证据的系列不得套用 828D 探针。
    #[test]
    fn other_families_have_no_probe_set() {
        assert!(probes_for_family("828d").is_some());
        assert!(probes_for_family("840d-sl").is_none());
        assert!(probes_for_family("").is_none());
    }
}
