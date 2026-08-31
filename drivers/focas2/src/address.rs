//! FOCAS2 地址解析（方案 §7.2 资源型，P0 44 清单已冻结）。
//!
//! V1 44 点全量（全部只读，Core 不触及此文件硬约束，2026-08-29 冻结）：
//! - `status` / `cnc.status` / `stat` → `cnc_statinfo`
//! - `alarm` → `cnc_rdalmmsg` `program.number/main/name/dir/info/upload` → `cnc_rdprgnum/dir/info/upload3`
//! - `axis.abs./machine./relative./distance./data./srvdelay./accdecdly <n>` n=1..32 → `cnc_absolute/cnc_rddynamic2`
//! - `axis.feed` `spindle.speed./load./gear./maxrpm <n>` `servo.<n>` → `cnc_acts/rdspmeter/svmeter/spgear/maxrpm`
//! - `tool.number/offset./zofs./length. <n>` → `cnc_rdtofs/r/tofsr/rdzofs` `IODBTO_1_1/1_2/1_3 Pack=4`
//! - `param.<num>` `param.axis <num>.<axis>` REAL → `cnc_rdparam IODBPSD_1/2/3 Pack=4`
//! - `macro.<num>` → `cnc_rdmacro` `pmc.G/X/Y/F/R/D...[.bit]` → `pmc_rdpmcrng` `diagnosis.<num>` `opmsg`
//!
//! 解析只做语法与边界校验，大小写不敏感，允许空格与下划线/连字符变体。

use thiserror::Error;

// ---------------------------------------------------------------------------
// 常量：FOCAS 地址边界（中文解释“为什么”）
// ---------------------------------------------------------------------------
/// FANUC 最大 32 轴（0i-F 3轴、30i 10/24轴均在此内），超出需扩展 OdbAxis
const FOCAS_MAX_AXIS: u8 = 32;
/// FANUC 最大 4 主轴（双主轴常见，4 为上位机兼容上限）
const FOCAS_MAX_SPINDLE: u8 = 4;
/// 位偏移固定 0..7（1 字节内 8 位）
const FOCAS_MAX_BIT: u8 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocasAddress {
    /// `cnc_statinfo` 报警等
    Status,
    Alarm,
    /// 程序号
    ProgramNumber,
    ProgramMain,
    ProgramName,
    ProgramDir,
    ProgramInfo,
    ProgramUpload,
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
    /// 刀具（TOOL 8：number/offset/zofs）
    Tool {
        kind: ToolKind,
        number: u32,
    },
    /// 参数
    Param {
        number: u32,
    },
    /// 操作信息 `cnc_rdopmsg` 64B
    OpMsg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    Absolute,
    Machine,
    Relative,
    Distance,
    Data,
    SrvDelay,
    AccDecDly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpindleKind {
    Speed,
    Load,
    Gear,
    MaxRpm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Number,
    Offset,
    Zofs,
    Length,
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
            "abs" | "absolute" | "pos" | "position" => Some(Self::Absolute),
            "machine" | "mach" => Some(Self::Machine),
            "relative" | "rel" => Some(Self::Relative),
            "distance" | "dist" => Some(Self::Distance),
            "data" | "axisdata" => Some(Self::Data),
            "srvdelay" | "srv" => Some(Self::SrvDelay),
            "accdecdly" | "acc" => Some(Self::AccDecDly),
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
    let s = raw.to_ascii_lowercase().replace([' ', '_', '-'], "");
    // 归一化别名
    let s = s.replace("cnc.", "");
    match s.as_str() {
        "status" | "stat" | "state" => return Ok(FocasAddress::Status),
        "alarm" | "alarmmessage" => return Ok(FocasAddress::Alarm),
        "programnumber" | "program.number" | "prgnum" => return Ok(FocasAddress::ProgramNumber),
        "programmain" | "program.main" | "prgmain" => return Ok(FocasAddress::ProgramMain),
        "programname" | "program.name" | "prgname" => return Ok(FocasAddress::ProgramName),
        "programdir" | "program.dir" | "progdir" => return Ok(FocasAddress::ProgramDir),
        "programinfo" | "program.info" | "proginfo" => return Ok(FocasAddress::ProgramInfo),
        "programupload" | "program.upload" | "upload" => return Ok(FocasAddress::ProgramUpload),
        "feed" | "actf" | "axisfeed" => return Ok(FocasAddress::Feed),
        _ => {}
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
    if s.starts_with("param.") || s.starts_with("parameter.") {
        return parse_param(&s, raw);
    }
    if s.starts_with("tool.") {
        return parse_tool(&s, raw);
    }
    if s.starts_with("macro.") || s.starts_with("variable.") || s.starts_with("var.") {
        return parse_macro(&s, raw);
    }
    if s.starts_with("pmc.") {
        return parse_pmc(&s, raw);
    }
    if s.starts_with("diagnosis.") || s.starts_with("diagnosis") {
        return parse_diagnosis(&s, raw);
    }
    if s == "opmsg" || s == "opmessage" || s == "op" {
        return Ok(FocasAddress::OpMsg);
    }
    // 兼容简写：abs.1 等直接视为 axis.abs.1
    if s.starts_with("abs.")
        || s.starts_with("machine.")
        || s.starts_with("relative.")
        || s.starts_with("distance.")
    {
        let fake = format!("axis.{s}");
        return parse_axis(&fake, raw);
    }
    Err(AddressError::Invalid {
        input: raw.to_string(),
        reason: "无法识别的 FOCAS 地址，需如 status/axis.abs.1/spindle.load.1/macro.100/pmc.R100"
            .into(),
    })
}

fn parse_axis(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    // s = axis.<kind>.<n> 或 axis.<n> 默认为 absolute
    let rest = &s["axis.".len()..];
    let parts: Vec<&str> = rest.split('.').collect();
    match parts.as_slice() {
        [kind, num] => {
            let k = AxisKind::from_str(kind).ok_or_else(|| AddressError::Invalid {
                input: raw.to_string(),
                reason: format!("轴类型 `{kind}` 非法，期望 abs/machine/relative/distance"),
            })?;
            let n: u8 = num.parse().map_err(|_| AddressError::Invalid {
                input: raw.to_string(),
                reason: format!("轴号 `{num}` 非法 1..{FOCAS_MAX_AXIS}"),
            })?;
            if n == 0 || n > FOCAS_MAX_AXIS {
                return Err(AddressError::Invalid {
                    input: raw.to_string(),
                    reason: format!("轴号必须 1..{FOCAS_MAX_AXIS}"),
                });
            }
            Ok(FocasAddress::Axis { axis: n, kind: k })
        }
        [num] => {
            // axis.1 视为 axis.abs.1
            let n: u8 = num.parse().map_err(|_| AddressError::Invalid {
                input: raw.to_string(),
                reason: format!("轴号 `{num}` 非法"),
            })?;
            if n == 0 || n > FOCAS_MAX_AXIS {
                return Err(AddressError::Invalid {
                    input: raw.to_string(),
                    reason: format!("轴号必须 1..{FOCAS_MAX_AXIS}"),
                });
            }
            Ok(FocasAddress::Axis {
                axis: n,
                kind: AxisKind::Absolute,
            })
        }
        _ => Err(AddressError::Invalid {
            input: raw.to_string(),
            reason: "轴地址形如 axis.abs.1 / axis.machine.2 / axis.1".into(),
        }),
    }
}

fn parse_spindle(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = &s["spindle.".len()..];
    let parts: Vec<&str> = rest.split('.').collect();
    match parts.as_slice() {
        [kind, num] => {
            let k = match *kind {
                "speed" | "acts" | "rpm" | "s" => SpindleKind::Speed,
                "load" | "spmeter" | "ld" => SpindleKind::Load,
                "gear" | "spgear" => SpindleKind::Gear,
                "maxrpm" | "spmaxrpm" => SpindleKind::MaxRpm,
                _ => {
                    return Err(AddressError::Invalid {
                        input: raw.to_string(),
                        reason: format!("主轴类型 `{kind}` 非法，期望 speed/load/gear/maxrpm"),
                    });
                }
            };
            let n: u8 = num.parse().map_err(|_| AddressError::Invalid {
                input: raw.to_string(),
                reason: format!("主轴号 `{num}` 非法 1..{FOCAS_MAX_SPINDLE}"),
            })?;
            if n == 0 || n > FOCAS_MAX_SPINDLE {
                return Err(AddressError::Invalid {
                    input: raw.to_string(),
                    reason: format!("主轴号必须 1..{FOCAS_MAX_SPINDLE}"),
                });
            }
            Ok(FocasAddress::Spindle {
                spindle: n,
                kind: k,
            })
        }
        [kind] => {
            let k = match *kind {
                "speed" | "acts" | "rpm" | "s" => SpindleKind::Speed,
                "load" | "spmeter" | "ld" => SpindleKind::Load,
                "gear" | "spgear" => SpindleKind::Gear,
                "maxrpm" | "spmaxrpm" => SpindleKind::MaxRpm,
                _ => {
                    return Err(AddressError::Invalid {
                        input: raw.to_string(),
                        reason: format!("主轴类型 `{kind}` 非法"),
                    });
                }
            };
            Ok(FocasAddress::Spindle {
                spindle: 1,
                kind: k,
            })
        }
        _ => Err(AddressError::Invalid {
            input: raw.to_string(),
            reason: "主轴地址形如 spindle.load.1 / spindle.speed.1".into(),
        }),
    }
}

fn parse_servo(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let prefix_len = if s.starts_with("servoload.") {
        "servoload.".len()
    } else {
        "servo.".len()
    };
    let rest = &s[prefix_len..];
    let n: u8 = rest.parse().map_err(|_| AddressError::Invalid {
        input: raw.to_string(),
        reason: format!("伺服轴号 `{rest}` 非法 1..{FOCAS_MAX_AXIS}"),
    })?;
    if n == 0 || n > FOCAS_MAX_AXIS {
        return Err(AddressError::Invalid {
            input: raw.to_string(),
            reason: format!("伺服轴号必须 1..{FOCAS_MAX_AXIS}"),
        });
    }
    Ok(FocasAddress::ServoLoad { axis: n })
}

fn parse_tool(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = &s["tool.".len()..];
    if rest == "number" || rest == "num" {
        return Ok(FocasAddress::Tool {
            kind: ToolKind::Number,
            number: 0,
        });
    }
    if let Some(num) = rest.strip_prefix("offset.") {
        let n: u32 = num.parse().map_err(|_| AddressError::Invalid {
            input: raw.to_string(),
            reason: format!("刀补号 `{num}` 非法"),
        })?;
        return Ok(FocasAddress::Tool {
            kind: ToolKind::Offset,
            number: n,
        });
    }
    if let Some(num) = rest.strip_prefix("zofs.") {
        let n: u32 = num.parse().map_err(|_| AddressError::Invalid {
            input: raw.to_string(),
            reason: format!("工件零点 `{num}` 非法"),
        })?;
        return Ok(FocasAddress::Tool {
            kind: ToolKind::Zofs,
            number: n,
        });
    }
    if let Some(num) = rest.strip_prefix("length.") {
        let n: u32 = num.parse().map_err(|_| AddressError::Invalid {
            input: raw.to_string(),
            reason: format!("刀长 `{num}` 非法"),
        })?;
        return Ok(FocasAddress::Tool {
            kind: ToolKind::Length,
            number: n,
        });
    }
    // 兼容 tool.1 视为 offset.1
    if let Ok(n) = rest.parse::<u32>() {
        return Ok(FocasAddress::Tool {
            kind: ToolKind::Offset,
            number: n,
        });
    }
    Err(AddressError::Invalid {
        input: raw.to_string(),
        reason: "tool 地址形如 tool.number / tool.offset.1 / tool.zofs.1 / tool.length.1".into(),
    })
}

fn parse_param(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = if let Some(stripped) = s.strip_prefix("param.") {
        stripped
    } else if let Some(stripped) = s.strip_prefix("parameter.") {
        stripped
    } else {
        &s["parameter.".len()..]
    };
    let n: u32 = rest.parse().map_err(|_| AddressError::Invalid {
        input: raw.to_string(),
        reason: format!("参数号 `{rest}` 非法"),
    })?;
    Ok(FocasAddress::Param { number: n })
}

fn parse_macro(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = if let Some(stripped) = s.strip_prefix("macro.") {
        stripped
    } else if let Some(stripped) = s.strip_prefix("variable.") {
        stripped
    } else if let Some(stripped) = s.strip_prefix("var.") {
        stripped
    } else {
        &s["var.".len()..]
    };
    let n: u32 = rest.parse().map_err(|_| AddressError::Invalid {
        input: raw.to_string(),
        reason: format!("宏变量号 `{rest}` 非法"),
    })?;
    Ok(FocasAddress::MacroVar { number: n })
}

fn parse_pmc(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = &s["pmc.".len()..].to_ascii_uppercase();
    if rest.is_empty() {
        return Err(AddressError::Invalid {
            input: raw.to_string(),
            reason: "PMC 地址缺失，如 pmc.R100".into(),
        });
    }
    let kind = rest.chars().next().unwrap();
    if !matches!(
        kind,
        'R' | 'D' | 'G' | 'X' | 'Y' | 'F' | 'A' | 'C' | 'K' | 'T' | 'M' | 'N' | 'E' | 'Z' | 'B'
    ) {
        return Err(AddressError::Invalid {
            input: raw.to_string(),
            reason: format!("PMC 类型 `{kind}` 非法"),
        });
    }
    let tail = &rest[1..];
    // 支持 R100.0 位
    let (num_s, bit_opt) = if let Some((a, b)) = tail.split_once('.') {
        (a, Some(b))
    } else {
        (tail, None)
    };
    let addr: u32 = num_s.parse().map_err(|_| AddressError::Invalid {
        input: raw.to_string(),
        reason: format!("PMC 地址 `{num_s}` 非法"),
    })?;
    let bit = if let Some(b) = bit_opt {
        let v: u8 = b.parse().map_err(|_| AddressError::Invalid {
            input: raw.to_string(),
            reason: format!("位 `{b}` 非法 0..{FOCAS_MAX_BIT}"),
        })?;
        if v > FOCAS_MAX_BIT {
            return Err(AddressError::Invalid {
                input: raw.to_string(),
                reason: format!("PMC 位必须 0..{FOCAS_MAX_BIT}"),
            });
        }
        Some(v)
    } else {
        None
    };
    Ok(FocasAddress::Pmc { kind, addr, bit })
}

fn parse_diagnosis(s: &str, raw: &str) -> Result<FocasAddress, AddressError> {
    let rest = s.trim_start_matches("diagnosis").trim_start_matches('.');
    if rest.is_empty() {
        return Ok(FocasAddress::Diagnosis { number: 0 });
    }
    let n: u32 = rest.parse().map_err(|_| AddressError::Invalid {
        input: raw.to_string(),
        reason: format!("诊断号 `{rest}` 非法"),
    })?;
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
        ok(
            "axis.abs.1",
            FocasAddress::Axis {
                axis: 1,
                kind: AxisKind::Absolute,
            },
        );
        ok(
            "axis.machine.2",
            FocasAddress::Axis {
                axis: 2,
                kind: AxisKind::Machine,
            },
        );
        ok(
            "AXIS.RELATIVE.3",
            FocasAddress::Axis {
                axis: 3,
                kind: AxisKind::Relative,
            },
        );
        ok(
            "axis.1",
            FocasAddress::Axis {
                axis: 1,
                kind: AxisKind::Absolute,
            },
        );
        ok(
            "abs.1",
            FocasAddress::Axis {
                axis: 1,
                kind: AxisKind::Absolute,
            },
        );
    }

    #[test]
    fn spindle_forms() {
        ok(
            "spindle.load.1",
            FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::Load,
            },
        );
        ok(
            "spindle.speed.2",
            FocasAddress::Spindle {
                spindle: 2,
                kind: SpindleKind::Speed,
            },
        );
        ok(
            "spindle.load",
            FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::Load,
            },
        );
    }

    #[test]
    fn macro_pmc() {
        ok("macro.100", FocasAddress::MacroVar { number: 100 });
        ok(
            "pmc.R100",
            FocasAddress::Pmc {
                kind: 'R',
                addr: 100,
                bit: None,
            },
        );
        ok(
            "pmc.R100.0",
            FocasAddress::Pmc {
                kind: 'R',
                addr: 100,
                bit: Some(0),
            },
        );
    }

    #[test]
    fn tool_forms() {
        ok(
            "tool.number",
            FocasAddress::Tool {
                kind: crate::address::ToolKind::Number,
                number: 0,
            },
        );
        ok(
            "tool.offset.1",
            FocasAddress::Tool {
                kind: crate::address::ToolKind::Offset,
                number: 1,
            },
        );
        ok(
            "tool.zofs.2",
            FocasAddress::Tool {
                kind: crate::address::ToolKind::Zofs,
                number: 2,
            },
        );
        ok("variable.100", FocasAddress::MacroVar { number: 100 });
        ok(
            "axis.pos.1",
            FocasAddress::Axis {
                axis: 1,
                kind: AxisKind::Absolute,
            },
        );
        ok(
            "pmc.Z0",
            FocasAddress::Pmc {
                kind: 'Z',
                addr: 0,
                bit: None,
            },
        );
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
