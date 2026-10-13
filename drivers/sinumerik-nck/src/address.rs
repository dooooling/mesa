//! NCK 基础地址类型（Area 字母表 / Unit 模式）。
//!
//! 只保留**协议基础枚举**：采集地址本身的组装与校验在 `native.rs`，
//! 本模块不再承载变量引用模型。
//!
//! 线缆侧（area 索引、unit/mode→syntax）是 S7Comm 协议知识。字母↔索引
//! 映射以 Wireshark `packet-s7comm.c` `nck_area_names`
//! （0=N 1=B 2=C 3=A 4=T 5=V 6=H 7=MMC）为依据；
//! unit/mode↔syntax 以 `SYNTAXID_NCK(0x82)/_METRIC(0x83)/_INCH(0x84)` 为依据。
//!
//! NOTE: MMC（索引 7）的用户侧字母待文档/真机确认，暂不接受；线缆结构
//! 以 u8 索引承载，未来确认后只扩展字母表，不改 Resource API。

use thiserror::Error;

// ---------------------------------------------------------------------------
// NCK Area（Siemens 字母 ↔ 线缆索引）
// ---------------------------------------------------------------------------

/// NCK 区域（Siemens NC Variable List 的 N/B/C/T/A/V/H 划分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NckArea {
    /// N：NCK 系统。
    Nck,
    /// B：Mode Group（方式组）。
    ModeGroup,
    /// C：Channel（通道）。
    Channel,
    /// A：Axis（轴。注意 Siemens 字母为 A，线缆索引为 3）。
    Axis,
    /// T：Tool（刀具）。
    Tool,
    /// V：FeedDrive（进给驱动）。
    FeedDrive,
    /// H：MainDrive（主轴驱动）。
    MainDrive,
}

impl NckArea {
    /// Siemens 用户字母。
    pub fn letter(self) -> char {
        match self {
            NckArea::Nck => 'N',
            NckArea::ModeGroup => 'B',
            NckArea::Channel => 'C',
            NckArea::Axis => 'A',
            NckArea::Tool => 'T',
            NckArea::FeedDrive => 'V',
            NckArea::MainDrive => 'H',
        }
    }

    /// 线缆索引（Wireshark `nck_area_names`：N=0 B=1 C=2 A=3 T=4 V=5 H=6）。
    pub fn index(self) -> u8 {
        match self {
            NckArea::Nck => 0,
            NckArea::ModeGroup => 1,
            NckArea::Channel => 2,
            NckArea::Axis => 3,
            NckArea::Tool => 4,
            NckArea::FeedDrive => 5,
            NckArea::MainDrive => 6,
        }
    }

    /// 用户字母 → Area（大小写不敏感；`M` 归属待确认，V1 拒绝并明示）。
    pub fn parse(letter: &str) -> Result<Self, AddressError> {
        match letter.trim().to_ascii_uppercase().as_str() {
            "N" => Ok(NckArea::Nck),
            "B" => Ok(NckArea::ModeGroup),
            "C" => Ok(NckArea::Channel),
            "A" => Ok(NckArea::Axis),
            "T" => Ok(NckArea::Tool),
            "V" => Ok(NckArea::FeedDrive),
            "H" => Ok(NckArea::MainDrive),
            _ => Err(AddressError::Invalid {
                input: letter.to_string(),
                reason: "area 需为 N/B/C/A/T/V/H（M/MMC 归属待真机确认，V1 不接受）".into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Unit 模式（↔ NCK Syntax ID）
// ---------------------------------------------------------------------------

/// 单位模式 → NCK Syntax（Wireshark `SYNTAXID_NCK*`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NckUnitMode {
    /// Current（控制器当前单位）→ `0x82`。
    #[default]
    Current,
    /// Metric（公制）→ `0x83`。
    Metric,
    /// Inch（英制）→ `0x84`。
    Inch,
}

impl NckUnitMode {
    /// 线缆 Syntax ID。
    pub fn syntax_id(self) -> u8 {
        match self {
            NckUnitMode::Current => 0x82,
            NckUnitMode::Metric => 0x83,
            NckUnitMode::Inch => 0x84,
        }
    }

    pub fn parse(s: &str) -> Result<Self, AddressError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "current" => Ok(NckUnitMode::Current),
            "metric" => Ok(NckUnitMode::Metric),
            "inch" => Ok(NckUnitMode::Inch),
            _ => Err(AddressError::Invalid {
                input: s.to_string(),
                reason: "unit_mode 需为 current/metric/inch".into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// 解析错误
// ---------------------------------------------------------------------------

#[derive(Debug, Error, Clone, PartialEq)]
pub enum AddressError {
    #[error("空地址参数")]
    Empty,
    #[error("非法地址参数 `{input}`: {reason}")]
    Invalid { input: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_letter_and_index_round_trip() {
        // 线缆索引以 Wireshark nck_area_names 为依据：N=0 B=1 C=2 A=3 T=4 V=5 H=6。
        for (letter, index) in [
            ("N", 0u8),
            ("B", 1),
            ("C", 2),
            ("A", 3),
            ("T", 4),
            ("V", 5),
            ("H", 6),
        ] {
            let area = NckArea::parse(letter).unwrap();
            assert_eq!(area.letter(), letter.chars().next().unwrap());
            assert_eq!(area.index(), index);
        }
        // 大小写不敏感；M/MMC 归属未经确认，必须拒绝。
        assert_eq!(NckArea::parse("c").unwrap(), NckArea::Channel);
        for bad in ["M", "MMC", "8", ""] {
            assert!(NckArea::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unit_mode_maps_to_syntax() {
        assert_eq!(NckUnitMode::Current.syntax_id(), 0x82);
        assert_eq!(NckUnitMode::Metric.syntax_id(), 0x83);
        assert_eq!(NckUnitMode::Inch.syntax_id(), 0x84);
        assert_eq!(NckUnitMode::parse("INCH").unwrap(), NckUnitMode::Inch);
        assert!(NckUnitMode::parse("mm").is_err());
    }
}
