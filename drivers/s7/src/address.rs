//! S7 地址解析（方案 §7.1 地址型）。
//!
//! V1 支持的地址族：
//! - `DB<db>.DB<X><offset>[.bit]` 如 `DB10.DBD20`、`DB10.DBX24.0`、`DB10.DBW0`、`DB10.DBB0`
//! - 简写 `DB<db>.<byte>[.bit]` 如 `DB10.0` 视为 `DB10.DBB0` 兼容用户"DB10.0"的表述
//! - `M/I/Q` 家族：位 `M0.0`/`I0.0`/`Q0.0`，字节 `MB10`/`IB0`/`QB0`，字 `MW10`/`IW0`，双字 `MD10`/`ID0`/`QD0`
//!   以及 `MX0.0` 等显式前缀变体。
//!
//! 解析只做语法与边界校验；Core 不触及此文件（硬性约束）。

use thiserror::Error;

// ---------------------------------------------------------------------------
// 常量：S7 区域与位边界（中文解释“为什么”）
// ---------------------------------------------------------------------------
/// S7 ANY 结构的 area 字段：DB 0x84 / M 0x83 / I 0x81 / Q 0x82 / C 0x1C / T 0x1D / L 0x86（S7Comm 固定）
const S7_AREA_DB: u8 = 0x84;
const S7_AREA_MERKER: u8 = 0x83;
const S7_AREA_INPUT: u8 = 0x81;
const S7_AREA_OUTPUT: u8 = 0x82;
const S7_AREA_COUNTER: u8 = 0x1C;
const S7_AREA_TIMER: u8 = 0x1D;
const S7_AREA_PERIPHERAL: u8 = 0x80; // PI/PE/PQ/PA/AI/AQ：V1 统一按外设区 0x80，兼容 PIW/PQW/PEW/AIW
const S7_AREA_LOCAL: u8 = 0x86; // L 局部栈（S7-200/300 L 栈）
/// 1 字节内位偏移上限 0..7
const S7_MAX_BIT: u8 = 7;
/// 字节到位换算：S7 ANY 中位地址 = byte*8 + bit
const S7_BITS_PER_BYTE: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Db,
    Merker,
    Input,
    Output,
    Counter,
    Timer,
    PeripheralInput,
    PeripheralOutput,
    Local,
}

impl Area {
    /// S7 协议的 area code（ANY 结构的 area 字段）。
    pub fn code(self) -> u8 {
        match self {
            Area::Db => S7_AREA_DB,
            Area::Merker => S7_AREA_MERKER,
            Area::Input => S7_AREA_INPUT,
            Area::Output => S7_AREA_OUTPUT,
            Area::Counter => S7_AREA_COUNTER,
            Area::Timer => S7_AREA_TIMER,
            Area::PeripheralInput => S7_AREA_PERIPHERAL,
            Area::PeripheralOutput => S7_AREA_PERIPHERAL,
            Area::Local => S7_AREA_LOCAL,
        }
    }

    /// 是否为外设区（PI/PQ），解析时需要区分 Input/Output 语义但线缆 code 同为 0x80。
    pub fn is_peripheral(self) -> bool {
        matches!(self, Area::PeripheralInput | Area::PeripheralOutput)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S7Address {
    /// 区域
    pub area: Area,
    /// 仅 Db 时有效，其余为 0
    pub db_number: u16,
    /// 字节偏移（0 起）
    pub byte_offset: u32,
    /// 位偏移（仅 BOOL/位地址有效，0-7）
    pub bit_offset: Option<u8>,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum AddressError {
    #[error("空地址")]
    Empty,
    #[error("非法地址 `{input}`: {reason}")]
    Invalid { input: String, reason: String },
}

impl S7Address {
    /// 将地址编码为 S7 ANY 结构中的 3 字节位偏移（byte_offset*8+bit）。
    /// Counter/Timer 例外：线缆上传递的是编号本身，不乘 8。
    pub fn bit_address(&self) -> u32 {
        match self.area {
            Area::Counter | Area::Timer => self.byte_offset,
            _ => self.byte_offset * S7_BITS_PER_BYTE + self.bit_offset.unwrap_or(0) as u32,
        }
    }
}

/// 解析用户提供的地址字符串（大小写不敏感，允许空格）。
///
/// Common 全量：`DB/M/I/Q/C/T` + `V→DB1` + `SM→M` + `AI/AQ→PI/PQ` + `L` 局部 + `PI/PQ/PE/PA`。
pub fn parse_address(input: &str) -> Result<S7Address, AddressError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(AddressError::Empty);
    }
    let s = raw.to_ascii_uppercase();
    // 统一去掉空格
    let s = s.replace(' ', "");
    if s.starts_with("DB") {
        parse_db(&s, raw)
    } else if s.starts_with("AC") {
        parse_ac(&s, raw)
    } else if s.starts_with("HC") {
        parse_hc(&s, raw)
    } else if s.starts_with("AI") || s.starts_with("AQ") {
        parse_ai_aq(&s, raw)
    } else if s.starts_with("PI") || s.starts_with("PE") || s.starts_with("PQ") || s.starts_with("PA") {
        parse_peripheral(&s, raw)
    } else if s.starts_with('V') {
        parse_v(&s, raw)
    } else if s.starts_with("SM") {
        parse_sm(&s, raw)
    } else if s.starts_with('C') && !s.starts_with("CLOCK") {
        parse_counter(&s, raw)
    } else if s.starts_with('T') {
        parse_timer(&s, raw)
    } else if s.starts_with('L') {
        parse_local(&s, raw)
    } else if s.starts_with('S') && !s.starts_with("SM") {
        parse_s(&s, raw)
    } else if s.starts_with('M') || s.starts_with('I') || s.starts_with('Q') {
        parse_miq(&s, raw)
    } else if s == "CLOCK" || s.starts_with("SZL") {
        // CLOCK/SZL 为系统功能，V1 映射为 DB0 占位，调用方走 SZL 诊断分支；保留解析兼容
        Err(AddressError::Invalid { input: raw.to_string(), reason: "CLOCK/SZL 请通过诊断接口读取，非点位地址".into() })
    } else {
        Err(AddressError::Invalid { input: raw.to_string(), reason: "必须以 DB/M/I/Q/C/T/V/SM/AI/AQ/L/PI/PQ 开头".into() })
    }
}

fn parse_db(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // s 形如 DB10.DBD20 / DB10.DBX20.0 / DB10.0 / DB10.0.1
    let dot = s.find('.').ok_or_else(|| AddressError::Invalid { input: raw.to_string(), reason: "DB 地址缺少 '.' 分隔".into() })?;
    let db_part = &s[2..dot];
    let db_number: u16 = db_part.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("DB 号 `{db_part}` 非法") })?;
    let rest = &s[dot + 1..];
    if rest.is_empty() {
        return Err(AddressError::Invalid { input: raw.to_string(), reason: "DB 偏移缺失".into() });
    }
    // 区分 DBX/DBB/DBW/DBD 前缀与裸数字
    if let Some(stripped) = rest.strip_prefix("DBX") {
        let (byte_s, bit_s) = split_byte_bit(stripped, raw)?;
        let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
        let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
        if bit > S7_MAX_BIT {
            return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") });
        }
        Ok(S7Address { area: Area::Db, db_number, byte_offset, bit_offset: Some(bit) })
    } else if let Some(stripped) = rest.strip_prefix("DBB") {
        let byte_offset: u32 = stripped.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{stripped}` 非法") })?;
        Ok(S7Address { area: Area::Db, db_number, byte_offset, bit_offset: None })
    } else if let Some(stripped) = rest.strip_prefix("DBW") {
        let byte_offset: u32 = stripped.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{stripped}` 非法") })?;
        Ok(S7Address { area: Area::Db, db_number, byte_offset, bit_offset: None })
    } else if let Some(stripped) = rest.strip_prefix("DBD") {
        let byte_offset: u32 = stripped.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{stripped}` 非法") })?;
        Ok(S7Address { area: Area::Db, db_number, byte_offset, bit_offset: None })
    } else {
        // 裸数字形式 DB10.0 或 DB10.0.1
        if rest.contains("DB") {
            return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("DB 偏移 `{rest}` 无法识别") });
        }
        let (byte_s, bit_opt) = if let Some((b, bit)) = rest.split_once('.') {
            (b, Some(bit))
        } else {
            (rest, None)
        };
        let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
        if let Some(bit_s) = bit_opt {
            let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
            if bit > S7_MAX_BIT {
                return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") });
            }
            Ok(S7Address { area: Area::Db, db_number, byte_offset, bit_offset: Some(bit) })
        } else {
            Ok(S7Address { area: Area::Db, db_number, byte_offset, bit_offset: None })
        }
    }
}

fn parse_miq(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    let prefix = s.chars().next().unwrap();
    let area = match prefix {
        'M' => Area::Merker,
        'I' => Area::Input,
        'Q' => Area::Output,
        _ => unreachable!(),
    };
    let rest = &s[1..];
    if rest.is_empty() {
        return Err(AddressError::Invalid { input: raw.to_string(), reason: "偏移缺失".into() });
    }
    // 显式宽度前缀 B/W/D/X
    if rest.starts_with('B') || rest.starts_with('W') || rest.starts_with('D') || rest.starts_with('X') {
        let after = &rest[1..];
        if after.is_empty() {
            return Err(AddressError::Invalid { input: raw.to_string(), reason: "偏移缺失".into() });
        }
        // 可能带 .bit，如 MX0.0 / MB0.1（非法但容错）
        let (byte_s, bit_opt) = if let Some((b, bit)) = after.split_once('.') {
            (b, Some(bit))
        } else {
            (after, None)
        };
        let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
        if let Some(bit_s) = bit_opt {
            let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
            if bit > S7_MAX_BIT {
                return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") });
            }
            Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: Some(bit) })
        } else {
            Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: None })
        }
    } else {
        // 隐式：如 M0.0 / I10 / Q0
        let (byte_s, bit_opt) = if let Some((b, bit)) = rest.split_once('.') {
            (b, Some(bit))
        } else {
            (rest, None)
        };
        let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
        if let Some(bit_s) = bit_opt {
            let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
            if bit > S7_MAX_BIT {
                return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") });
            }
            Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: Some(bit) })
        } else {
            Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: None })
        }
    }
}

fn parse_peripheral(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // PIW0 / PQW0 / PEW0 / PAW0 / PIB0 / PEB0 / PI0 / PQ0 支持，V1 统一按字/字节寻址
    let is_input = s.starts_with("PI") || s.starts_with("PE");
    let area = if is_input { Area::PeripheralInput } else { Area::PeripheralOutput };
    let rest = if s.starts_with("PI") || s.starts_with("PQ") || s.starts_with("PE") || s.starts_with("PA") { &s[2..] } else { s };
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "外设偏移缺失".into() }); }
    let after = if rest.starts_with('B') || rest.starts_with('W') || rest.starts_with('D') { &rest[1..] } else { rest };
    if after.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "外设偏移缺失".into() }); }
    // 兼容 PIW0 / PI0.0 混写，去掉可能的位后缀
    let byte_s = after.split('.').next().unwrap_or(after);
    let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
    Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: None })
}

fn parse_counter(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // C0 / C10 / CW0 兼容，S7 计数器为字寻址（0..2047）
    let rest = s[1..].trim_start_matches(|c| c == 'W' || c == 'B' || c == 'D');
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "计数器号缺失".into() }); }
    let num: u32 = rest.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("计数器 `{rest}` 非法") })?;
    if num > 2047 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "计数器号超出 0..2047".into() }); }
    Ok(S7Address { area: Area::Counter, db_number: 0, byte_offset: num, bit_offset: None })
}

fn parse_timer(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // T0 / T10 / TW0 兼容，定时器同计数器范围
    let rest = s[1..].trim_start_matches(|c| c == 'W' || c == 'B' || c == 'D');
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "定时器号缺失".into() }); }
    let num: u32 = rest.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("定时器 `{rest}` 非法") })?;
    if num > 2047 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "定时器号超出 0..2047".into() }); }
    Ok(S7Address { area: Area::Timer, db_number: 0, byte_offset: num, bit_offset: None })
}

fn parse_v(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // V / VB / VW / VD 均映射为 DB1（S7-200 V 变量区 ≡ DB1），兼容 V0.0 / VB0 / VW0
    let rest = s[1..].trim_start_matches(|c| c == 'B' || c == 'W' || c == 'D');
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "V 偏移缺失".into() }); }
    let (byte_s, bit_opt) = if let Some((b, bit)) = rest.split_once('.') { (b, Some(bit)) } else { (rest, None) };
    let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
    if let Some(bit_s) = bit_opt {
        let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
        if bit > S7_MAX_BIT { return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") }); }
        Ok(S7Address { area: Area::Db, db_number: 1, byte_offset, bit_offset: Some(bit) })
    } else {
        Ok(S7Address { area: Area::Db, db_number: 1, byte_offset, bit_offset: None })
    }
}

fn parse_sm(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // SM / SMB / SMW / SMD 映射为 Merker 0x83（SM0.0 特殊位同 M 寻址）
    let rest = s[2..].trim_start_matches(|c| c == 'B' || c == 'W' || c == 'D');
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "SM 偏移缺失".into() }); }
    let (byte_s, bit_opt) = if let Some((b, bit)) = rest.split_once('.') { (b, Some(bit)) } else { (rest, None) };
    let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
    if let Some(bit_s) = bit_opt {
        let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
        if bit > S7_MAX_BIT { return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") }); }
        Ok(S7Address { area: Area::Merker, db_number: 0, byte_offset, bit_offset: Some(bit) })
    } else {
        Ok(S7Address { area: Area::Merker, db_number: 0, byte_offset, bit_offset: None })
    }
}

fn parse_ai_aq(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // AIW0 / AI0 / AQW0 / AQB0 统一按外设 0x80，AI→Input AQ→Output
    let is_input = s.starts_with("AI");
    let area = if is_input { Area::PeripheralInput } else { Area::PeripheralOutput };
    let rest = &s[2..];
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "AI/AQ 偏移缺失".into() }); }
    let after = rest.trim_start_matches(|c| c == 'B' || c == 'W' || c == 'D');
    if after.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "AI/AQ 偏移缺失".into() }); }
    let byte_s = after.split('.').next().unwrap_or(after);
    let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
    Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: None })
}

fn parse_local(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // L / LB / LW / LD 局部栈 0x86，S7-200/300 L 区
    let rest = s[1..].trim_start_matches(|c| c == 'B' || c == 'W' || c == 'D');
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "L 偏移缺失".into() }); }
    let (byte_s, bit_opt) = if let Some((b, bit)) = rest.split_once('.') { (b, Some(bit)) } else { (rest, None) };
    let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
    if let Some(bit_s) = bit_opt {
        let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
        if bit > S7_MAX_BIT { return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") }); }
        Ok(S7Address { area: Area::Local, db_number: 0, byte_offset, bit_offset: Some(bit) })
    } else {
        Ok(S7Address { area: Area::Local, db_number: 0, byte_offset, bit_offset: None })
    }
}

fn parse_ac(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // AC0-AC3 累加器（S7-200），映射为 Merker 0x83 兼容，S7-200 专属
    let rest = s[2..].trim_start_matches(|c| c == 'B' || c == 'W' || c == 'D');
    if rest.is_empty() { return Ok(S7Address { area: Area::Merker, db_number: 0, byte_offset: 0, bit_offset: None }); }
    let byte_s = rest.split('.').next().unwrap_or(rest);
    let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("AC `{rest}` 非法") })?;
    if byte_offset > 3 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "AC 仅 0..3".into() }); }
    Ok(S7Address { area: Area::Merker, db_number: 0, byte_offset: byte_offset * 4, bit_offset: None })
}

fn parse_hc(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // HC0-HC5 高速计数器（S7-200），映射为 Counter 0x1C
    let rest = s[2..].trim_start_matches(|c| c == 'B' || c == 'W' || c == 'D');
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "HC 号缺失".into() }); }
    let num: u32 = rest.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("HC `{rest}` 非法") })?;
    if num > 5 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "HC 仅 0..5".into() }); }
    Ok(S7Address { area: Area::Counter, db_number: 0, byte_offset: num, bit_offset: None })
}

fn parse_s(s: &str, raw: &str) -> Result<S7Address, AddressError> {
    // S0.0 / S0 / SB0 顺控继电器（S7-200 SCR），映射为 Merker 0x83
    let rest = s[1..].trim_start_matches(|c| c == 'B' || c == 'W' || c == 'D');
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "S 偏移缺失".into() }); }
    let (byte_s, bit_opt) = if let Some((b, bit)) = rest.split_once('.') { (b, Some(bit)) } else { (rest, None) };
    let byte_offset: u32 = byte_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("字节偏移 `{byte_s}` 非法") })?;
    if let Some(bit_s) = bit_opt {
        let bit: u8 = bit_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移 `{bit_s}` 非法") })?;
        if bit > S7_MAX_BIT { return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("位偏移必须 0..{S7_MAX_BIT}") }); }
        Ok(S7Address { area: Area::Merker, db_number: 0, byte_offset, bit_offset: Some(bit) })
    } else {
        Ok(S7Address { area: Area::Merker, db_number: 0, byte_offset, bit_offset: None })
    }
}

fn split_byte_bit<'a>(s: &'a str, raw: &'a str) -> Result<(&'a str, &'a str), AddressError> {
    let (b, bit) = s.split_once('.').ok_or_else(|| AddressError::Invalid { input: raw.to_string(), reason: "DBX 必须带位偏移如 DBX0.0".into() })?;
    if b.is_empty() || bit.is_empty() {
        return Err(AddressError::Invalid { input: raw.to_string(), reason: "DBX 位地址格式非法".into() });
    }
    Ok((b, bit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str, area: Area, db: u16, byte: u32, bit: Option<u8>) {
        let a = parse_address(s).expect(&format!("parse {s} should ok"));
        assert_eq!(a.area, area);
        assert_eq!(a.db_number, db);
        assert_eq!(a.byte_offset, byte);
        assert_eq!(a.bit_offset, bit, "bit mismatch for {s}");
    }

    #[test]
    fn db_full_forms() {
        ok("DB10.DBD20", Area::Db, 10, 20, None);
        ok("DB10.DBW20", Area::Db, 10, 20, None);
        ok("DB10.DBB20", Area::Db, 10, 20, None);
        ok("DB10.DBX24.0", Area::Db, 10, 24, Some(0));
        ok("DB1.DBX0.7", Area::Db, 1, 0, Some(7));
        ok("db10.dbd20", Area::Db, 10, 20, None);
    }

    #[test]
    fn db_shorthand() {
        ok("DB10.0", Area::Db, 10, 0, None);
        ok("DB10.0.1", Area::Db, 10, 0, Some(1));
        ok("DB10.20", Area::Db, 10, 20, None);
    }

    #[test]
    fn miq_forms() {
        ok("M0.0", Area::Merker, 0, 0, Some(0));
        ok("MB10", Area::Merker, 0, 10, None);
        ok("MW10", Area::Merker, 0, 10, None);
        ok("MD10", Area::Merker, 0, 10, None);
        ok("I0.0", Area::Input, 0, 0, Some(0));
        ok("IB0", Area::Input, 0, 0, None);
        ok("Q0.0", Area::Output, 0, 0, Some(0));
    }

    #[test]
    fn counter_timer() {
        ok("C0", Area::Counter, 0, 0, None);
        ok("C10", Area::Counter, 0, 10, None);
        ok("T5", Area::Timer, 0, 5, None);
        assert_eq!(parse_address("C10").unwrap().bit_address(), 10);
        assert_eq!(parse_address("T7").unwrap().bit_address(), 7);
    }

    #[test]
    fn peripheral() {
        ok("PIW0", Area::PeripheralInput, 0, 0, None);
        ok("PQW0", Area::PeripheralOutput, 0, 0, None);
        ok("PIW256", Area::PeripheralInput, 0, 256, None);
        ok("PEW0", Area::PeripheralInput, 0, 0, None);
    }

    #[test]
    fn v_sm_ai_aq_l() {
        ok("VB0", Area::Db, 1, 0, None);
        ok("VW10", Area::Db, 1, 10, None);
        ok("V0.0", Area::Db, 1, 0, Some(0));
        ok("SM0.0", Area::Merker, 0, 0, Some(0));
        ok("SMB0", Area::Merker, 0, 0, None);
        ok("AIW0", Area::PeripheralInput, 0, 0, None);
        ok("AQW0", Area::PeripheralOutput, 0, 0, None);
        ok("L0.0", Area::Local, 0, 0, Some(0));
        ok("LB10", Area::Local, 0, 10, None);
    }

    #[test]
    fn ac_hc_s() {
        ok("AC0", Area::Merker, 0, 0, None);
        ok("AC1", Area::Merker, 0, 4, None);
        ok("HC0", Area::Counter, 0, 0, None);
        ok("HC5", Area::Counter, 0, 5, None);
        ok("S0.0", Area::Merker, 0, 0, Some(0));
        ok("SB0", Area::Merker, 0, 0, None);
    }

    #[test]
    fn invalid_rejected() {
        assert!(parse_address("").is_err());
        assert!(parse_address("DB10").is_err());
        assert!(parse_address("DB10.DBX0.8").is_err());
        assert!(parse_address("XX10").is_err());
        assert!(parse_address("M").is_err());
    }
}
