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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Db,
    Merker,
    Input,
    Output,
}

impl Area {
    /// S7 协议的 area code（ANY 结构的 area 字段）。
    pub fn code(self) -> u8 {
        match self {
            Area::Db => 0x84,
            Area::Merker => 0x83,
            Area::Input => 0x81,
            Area::Output => 0x82,
        }
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
    pub fn bit_address(&self) -> u32 {
        self.byte_offset * 8 + self.bit_offset.unwrap_or(0) as u32
    }
}

/// 解析用户提供的地址字符串（大小写不敏感，允许空格）。
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
    } else if s.starts_with('M') || s.starts_with('I') || s.starts_with('Q') {
        parse_miq(&s, raw)
    } else {
        Err(AddressError::Invalid { input: raw.to_string(), reason: "必须以 DB/M/I/Q 开头".into() })
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
        if bit > 7 {
            return Err(AddressError::Invalid { input: raw.to_string(), reason: "位偏移必须 0-7".into() });
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
            if bit > 7 {
                return Err(AddressError::Invalid { input: raw.to_string(), reason: "位偏移必须 0-7".into() });
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
            if bit > 7 {
                return Err(AddressError::Invalid { input: raw.to_string(), reason: "位偏移必须 0-7".into() });
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
            if bit > 7 {
                return Err(AddressError::Invalid { input: raw.to_string(), reason: "位偏移必须 0-7".into() });
            }
            Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: Some(bit) })
        } else {
            Ok(S7Address { area, db_number: 0, byte_offset, bit_offset: None })
        }
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
    fn invalid_rejected() {
        assert!(parse_address("").is_err());
        assert!(parse_address("DB10").is_err());
        assert!(parse_address("DB10.DBX0.8").is_err());
        assert!(parse_address("XX10").is_err());
        assert!(parse_address("M").is_err());
    }
}
