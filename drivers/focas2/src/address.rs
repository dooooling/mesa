//! FOCAS2 地址解析（方案 §7.2 资源型）。
//!
//! V1 支持的地址族（全部只读，Core 不触及此文件硬约束）：
//! - `status` / `cnc.status` / `stat` → `cnc_statinfo` 报警/运行状态
//! - `alarm` / `alarm.message` → `cnc_rdalmmsg`
//! - `program.number` / `program.main` / `program.name` → `cnc_rdprgnum`/`cnc_exeprgname`
//! - `axis.abs.<n>` / `axis.machine.<n>` / `axis.relative.<n>` / `axis.distance.<n>` → `cnc_absolute/cnc_machine` 等（n=1..32）
//! - `axis.feed` → `cnc_actf`，`spindle.speed.<n>` / `spindle.load.<n>` → `cnc_acts/cnc_rdspmeter`
//! - `macro.<num>` → `cnc_rdmacro`（如 macro.100）
//! - `pmc.R100` / `pmc.D100` 等 → `pmc_rdpmcrng`（预留，Fake 阶段返回随机）
//! - `diagnosis` → `cnc_diagnoss`（预留）
//!
//! 解析只做语法与边界校验，大小写不敏感，允许空格与下划线/连字符变体。

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocasAddress {
    /// `cnc_statinfo` 报警等
    Status,
    Alarm,
    /// 程序号
    ProgramNumber,
    ProgramMain,
    ProgramName,
    /// 轴位置
    Axis {
        axis: u8,
        kind: AxisKind,
    },
    /// 实际进给
    Feed,
    /// 主轴
    Spindle {
        spindle: u8,
        kind: SpindleKind,
    },
    /// 伺服负载
    ServoLoad {
        axis: u8,
    },
    /// 宏变量
    MacroVar {
        number: u32,
    },
    /// PMC 地址（R/D 等）
    Pmc {
        kind: char,
        addr: u32,
        bit: Option<u8>,
    },
    /// 诊断（预留）
    Diagnosis {
        number: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    Absolute,
    Machine,
    Relative,
    Distance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpindleKind {
    Speed,
    Load,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum AddressError {
    #[error("空地址")]
    Empty,
    #[error("非法地址 `{input}`: {reason}")]
    Invalid { input: String, reason: String },
}

impl AxisKind {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "abs" | "absolute" => Some(Self::Absolute),
            "machine" | "mach" => Some(Self::Machine),
            "relative" | "rel" => Some(Self::Relative),
            "distance" | "dist" => Some(Self::Distance),
            _ => None,
        }
    }
}

/// 解析用户提供的 FOCAS 地址字符串
pub fn parse_address(input: &str) -> Result<FocasAddress, AddressError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(AddressError::Empty);
    }
    let s = raw.to_ascii_lowercase().replace(' ', "").replace('_', "").replace('-', "");
    // 归一化别名
    let s = s.replace("cnc.", "");
    match s.as_str() {
        "status" | "stat" | "state" => return Ok(FocasAddress::Status),
        "alarm" | "alarmmessage" => return Ok(FocasAddress::Alarm),
        "programnumber" | "prgnum" => return Ok(FocasAddress::ProgramNumber),
        "programmain" | "prgmain" => return Ok(FocasAddress::ProgramMain),
        "programname" | "prgname" => return Ok(FocasAddress::ProgramName),
        "feed" | "actf" | "axisfeed" => return Ok(FocasAddress::Feed),
        _ => {},
    }
    // 前缀分发
    if s.starts_with("axis.") {
        return parse_axis(&s, raw);
    }
    if s.starts_with("spindle.") {
        return parse_spindle(&s, raw);
    }
    if s.starts_with("servoload.") || s.starts_with("servo.") {
        return parse_servo(&s, raw);
    }
    if s.starts_with("macro.") {
        return parse_macro(&s, raw);
    }
    if s.starts_with("pmc.") {
        return parse_pmc(&s, raw);
    }
    if s.starts_with("diagnosis.") || s.starts_with("diagnosis") {
        return parse_diagnosis(&s, raw);
    }
    // 兼容简写：abs.1 等直接视为 axis.abs.1
    if s.starts_with("abs.") || s.starts_with("machine.") || s.starts_with("relative.") || s.starts_with("distance.") {
        let fake = format!("axis.{s}");
        return parse_axis(&fake, raw);
    }
    Err(AddressError::Invalid { input: raw.to_string(), reason: "无法识别的 FOCAS 地址，需如 status/axis.abs.1/spindle.load.1/macro.100/pmc.R100".into() })
}

fn parse_axis(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    // s = axis.<kind>.<n> 或 axis.<n> 默认为 absolute
    let rest = &s["axis.".len()..];
    let parts: Vec<&str> = rest.split('.').collect();
    match parts.as_slice() {
        [kind, num] => {
            let k = AxisKind::from_str(kind).ok_or_else(|| AddressError::Invalid { input: raw.to_string(), reason: format!("轴类型 `{kind}` 非法，期望 abs/machine/relative/distance") })?;
            let n: u8 = num.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("轴号 `{num}` 非法 1..32") })?;
            if n == 0 || n > 32 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "轴号必须 1..32".into() }); }
            Ok(FocasAddress::Axis { axis: n, kind: k })
        }
        [num] => {
            // axis.1 视为 axis.abs.1
            let n: u8 = num.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("轴号 `{num}` 非法") })?;
            if n == 0 || n > 32 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "轴号必须 1..32".into() }); }
            Ok(FocasAddress::Axis { axis: n, kind: AxisKind::Absolute })
        }
        _ => Err(AddressError::Invalid { input: raw.to_string(), reason: "轴地址形如 axis.abs.1 / axis.machine.2 / axis.1".into() }),
    }
}

fn parse_spindle(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = &s["spindle.".len()..];
    let parts: Vec<&str> = rest.split('.').collect();
    match parts.as_slice() {
        [kind, num] => {
            let k = match *kind {
                "speed" | "acts" => SpindleKind::Speed,
                "load" | "spmeter" => SpindleKind::Load,
                _ => return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("主轴类型 `{kind}` 非法，期望 speed/load") }),
            };
            let n: u8 = num.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("主轴号 `{num}` 非法 1..4") })?;
            if n == 0 || n > 4 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "主轴号必须 1..4".into() }); }
            Ok(FocasAddress::Spindle { spindle: n, kind: k })
        }
        [kind] => {
            let k = match *kind {
                "speed" | "acts" => SpindleKind::Speed,
                "load" | "spmeter" => SpindleKind::Load,
                _ => return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("主轴类型 `{kind}` 非法") }),
            };
            Ok(FocasAddress::Spindle { spindle: 1, kind: k })
        }
        _ => Err(AddressError::Invalid { input: raw.to_string(), reason: "主轴地址形如 spindle.load.1 / spindle.speed.1".into() }),
    }
}

fn parse_servo(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let prefix_len = if s.starts_with("servoload.") { "servoload.".len() } else { "servo.".len() };
    let rest = &s[prefix_len..];
    let n: u8 = rest.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("伺服轴号 `{rest}` 非法 1..32") })?;
    if n == 0 || n > 32 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "伺服轴号必须 1..32".into() }); }
    Ok(FocasAddress::ServoLoad { axis: n })
}

fn parse_macro(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = &s["macro.".len()..];
    let n: u32 = rest.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("宏变量号 `{rest}` 非法") })?;
    Ok(FocasAddress::MacroVar { number: n })
}

fn parse_pmc(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = &s["pmc.".len()..].to_ascii_uppercase();
    if rest.is_empty() { return Err(AddressError::Invalid { input: raw.to_string(), reason: "PMC 地址缺失，如 pmc.R100".into() }); }
    let kind = rest.chars().next().unwrap();
    if !matches!(kind, 'R' | 'D' | 'G' | 'X' | 'Y' | 'F' | 'A' | 'C' | 'K' | 'T' | 'M' | 'N' | 'E') {
        return Err(AddressError::Invalid { input: raw.to_string(), reason: format!("PMC 类型 `{kind}` 非法") });
    }
    let tail = &rest[1..];
    // 支持 R100.0 位
    let (num_s, bit_opt) = if let Some((a, b)) = tail.split_once('.') { (a, Some(b)) } else { (tail, None) };
    let addr: u32 = num_s.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("PMC 地址 `{num_s}` 非法") })?;
    let bit = if let Some(b) = bit_opt {
        let v: u8 = b.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("位 `{b}` 非法 0..7") })?;
        if v > 7 { return Err(AddressError::Invalid { input: raw.to_string(), reason: "PMC 位必须 0..7".into() }); }
        Some(v)
    } else { None };
    Ok(FocasAddress::Pmc { kind, addr, bit })
}

fn parse_diagnosis(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = s.trim_start_matches("diagnosis").trim_start_matches('.');
    if rest.is_empty() { return Ok(FocasAddress::Diagnosis { number: 0 }); }
    let n: u32 = rest.parse().map_err(|_| AddressError::Invalid { input: raw.to_string(), reason: format!("诊断号 `{rest}` 非法") })?;
    Ok(FocasAddress::Diagnosis { number: n })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str, expected: FocasAddress) {
        let a = parse_address(s).unwrap_or_else(|e| panic!("parse {s} failed: {e}"));
        assert_eq!(a, expected, "mismatch for {s}");
    }

    #[test]
    fn status_forms() {
        ok("status", FocasAddress::Status);
        ok("cnc.status", FocasAddress::Status);
        ok("STAT", FocasAddress::Status);
        ok("alarm", FocasAddress::Alarm);
    }

    #[test]
    fn axis_forms() {
        ok("axis.abs.1", FocasAddress::Axis { axis: 1, kind: AxisKind::Absolute });
        ok("axis.machine.2", FocasAddress::Axis { axis: 2, kind: AxisKind::Machine });
        ok("AXIS.RELATIVE.3", FocasAddress::Axis { axis: 3, kind: AxisKind::Relative });
        ok("axis.1", FocasAddress::Axis { axis: 1, kind: AxisKind::Absolute });
        ok("abs.1", FocasAddress::Axis { axis: 1, kind: AxisKind::Absolute });
    }

    #[test]
    fn spindle_forms() {
        ok("spindle.load.1", FocasAddress::Spindle { spindle: 1, kind: SpindleKind::Load });
        ok("spindle.speed.2", FocasAddress::Spindle { spindle: 2, kind: SpindleKind::Speed });
        ok("spindle.load", FocasAddress::Spindle { spindle: 1, kind: SpindleKind::Load });
    }

    #[test]
    fn macro_pmc() {
        ok("macro.100", FocasAddress::MacroVar { number: 100 });
        ok("pmc.R100", FocasAddress::Pmc { kind: 'R', addr: 100, bit: None });
        ok("pmc.R100.0", FocasAddress::Pmc { kind: 'R', addr: 100, bit: Some(0) });
    }

    #[test]
    fn invalid_rejected() {
        assert!(parse_address("").is_err());
        assert!(parse_address("axis.abs.0").is_err());
        assert!(parse_address("axis.abs.33").is_err());
        assert!(parse_address("spindle.foo.1").is_err());
        assert!(parse_address("unknown.xyz").is_err());
    }
}
