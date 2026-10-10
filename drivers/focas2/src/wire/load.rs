//! 负载读取的正式路径；数量和 ABI 数值保留在驱动内，输出统一为百分比。

use super::WireError;
use super::frame::{
    FocasFrame, PacketType, REQUEST_ORIGIN, decode_reply_payload, encode_generic_request,
    request_subpacket,
};
use super::wire::{
    CMD_LOAD_HEAD, CMD_SERVO_LOAD, CMD_SERVO_LOAD_NAMES, CMD_SPINDLE_METER, CMD_SPINDLE_NAMES,
    DEV_CNC, LoadNumeric,
};
use mesa_core_types::Value;

/// OPEN spec 决定 DLL 的新旧负载分支；数量来自能力表，不用名称长度推断。
/// 字段偏移来自 FWLIBE64 的 0x915AC/0x91AC4；状态4记忆只属于当前会话。
#[derive(Default)]
pub(super) struct Capabilities {
    pub legacy: bool,
    pub axes: usize,
    pub spindles: usize,
    pub family: u16,
    pub scaled: bool,
    pub global_axes: Option<usize>,
}

impl Capabilities {
    pub fn from_open(response: &FocasFrame) -> Self {
        let p = &response.payload;
        let short = |at: usize| {
            p.get(at..at + 2)
                .map(|v| u16::from_be_bytes([v[0], v[1]]))
                .unwrap_or(0)
        };
        let legacy = response.origin <= 2;
        Self {
            legacy,
            // 路径1记录在旧表+16、新表+40；路径记录缺失时不擅自越界。
            axes: if legacy && short(8) > 0 {
                short(20) as usize
            } else {
                0
            },
            spindles: if legacy && short(8) > 0 {
                short(22) as usize
            } else {
                short(20) as usize
            },
            family: short(2),
            ..Self::default()
        }
    }
}

fn packet(slots: &[(u16, [i32; 5])]) -> FocasFrame {
    FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(
            &slots
                .iter()
                .map(|(command, args)| {
                    request_subpacket(DEV_CNC, (1u32 << 16) | *command as u32, *args)
                })
                .collect::<Vec<_>>(),
        ),
    }
}

/// 旧主轴分支无 A4，固定同时读取负载、速度、名称；七槽系数来自参数数据。
pub(super) fn legacy_request(servo: bool, scaled: bool) -> FocasFrame {
    if servo {
        return packet(&[(0x56, [1, 0, 0, 0, 0]), (0x89, [0; 5])]);
    }
    if !scaled {
        return packet(&[
            (0x40, [4, -1, 0, 0, 0]),
            (0x40, [5, -1, 0, 0, 0]),
            (0x8a, [0; 5]),
        ]);
    }
    packet(&[
        (0x40, [0, -1, 0, 0, 0]),
        (0x40, [1, -1, 0, 0, 0]),
        (0x8a, [0; 5]),
        (0x0e, [4127, 4127, -1, 0, 0]),
        (0x0e, [4274, 4274, -1, 0, 0]),
        (0x0e, [4020, 4020, -1, 0, 0]),
        (0x0e, [4196, 4196, -1, 0, 0]),
    ])
}

pub(super) fn global_axes_request() -> FocasFrame {
    packet(&[(0x8c, [0; 5])])
}

fn slots(
    response: &FocasFrame,
    commands: &[u16],
) -> Result<Vec<super::frame::ReplySubpacket>, WireError> {
    let slots = decode_reply_payload(&response.payload).map_err(|_| WireError::MalformedPayload)?;
    if slots.len() != commands.len()
        || slots
            .iter()
            .zip(commands)
            .any(|(s, c)| s.device != DEV_CNC || s.path != 1 || s.command != *c)
    {
        return Err(WireError::CommandMismatch);
    }
    Ok(slots)
}

pub(super) fn legacy_needs_scale(response: &FocasFrame) -> Result<bool, WireError> {
    let slots = slots(response, &[0x40, 0x40, 0x8a])?;
    super::wire::spindle_legacy_branch(slots[0].status, slots[0].detail1, slots[0].detail2)
}

pub(super) fn decode_global_axes(response: &FocasFrame) -> Result<usize, WireError> {
    let slots = slots(response, &[0x8c])?;
    let data = super::wire::reply_success_data(&slots[0])?;
    if data.len() != 4 {
        return Err(WireError::MalformedPayload);
    }
    Ok(u16::from_be_bytes([data[2], data[3]]) as usize)
}

/// DLL 旧路径按能力数量截取；先验证整个所需容量，再解码，拒绝短帧。
pub(super) fn decode_legacy(
    response: &FocasFrame,
    servo: bool,
    scaled: bool,
    count: usize,
) -> Result<Vec<LoadNumeric>, WireError> {
    let commands: &[u16] = if servo {
        &[0x56, 0x89]
    } else if scaled {
        &[0x40, 0x40, 0x8a, 0x0e, 0x0e, 0x0e, 0x0e]
    } else {
        &[0x40, 0x40, 0x8a]
    };
    let slots = slots(response, commands)?;
    let data = slots
        .iter()
        .map(super::wire::reply_success_data)
        .collect::<Result<Vec<_>, _>>()?;
    let count = count.min(4);
    let names = data[if servo { 1 } else { 2 }];
    if names.len() < count * 4 || data[0].len() < count * 8 || (!servo && data[1].len() < count * 8)
    {
        return Err(WireError::MalformedPayload);
    }
    if scaled && data[3..].iter().any(|d| d.len() < 8 + count * 4) {
        return Err(WireError::MalformedPayload);
    }
    (0..count)
        .map(|i| {
            let mut numeric = LoadNumeric::decode(&data[0][i * 8..i * 8 + 8])?;
            if servo {
                numeric.raw = numeric.raw.wrapping_abs();
            }
            if scaled {
                let coefficients = data[if names[i * 4 + 2] == b'2' { 4 } else { 3 }];
                let at = 8 + i * 4;
                let coefficient = i32::from_be_bytes(coefficients[at..at + 4].try_into().unwrap());
                // DLL 先取32位 abs，再32位乘法回绕，最后有符号截断除法。
                numeric.raw = super::wire::legacy_scaled(numeric.raw, coefficient, 32767)?;
                numeric.dec_bits = 0;
            }
            Ok(numeric)
        })
        .collect()
}

/// A4 的参数选择数量类别，不是调用者容量；主轴选 1，伺服选 2。
pub(super) fn request(servo: bool) -> FocasFrame {
    let f = |command| (1u32 << 16) | command as u32;
    let category = if servo { 2 } else { 1 };
    FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&[
            request_subpacket(DEV_CNC, f(CMD_LOAD_HEAD), [category, 0, 0, 0, 0]),
            request_subpacket(
                DEV_CNC,
                f(if servo {
                    CMD_SERVO_LOAD_NAMES
                } else {
                    CMD_SPINDLE_NAMES
                }),
                [0; 5],
            ),
            request_subpacket(
                DEV_CNC,
                f(if servo {
                    CMD_SERVO_LOAD
                } else {
                    CMD_SPINDLE_METER
                }),
                if servo {
                    [1, 0, 0, 0, 0]
                } else {
                    [4, -1, 0, 0, 0]
                },
            ),
            request_subpacket(DEV_CNC, f(CMD_LOAD_HEAD), [category, 0, 0, 0, 0]),
        ]),
    }
}

/// 小数位属于有符号 short。仅接受可无溢出、无非零下溢表示的工程值；
/// 超出 F64 范围时返回 BAD，绝不钳位或转换成正常的零负载。
pub(super) fn percent(n: LoadNumeric) -> Result<Value, WireError> {
    if n.raw == 0 {
        return Ok(Value::F64(0.0));
    }
    let decimal = n.dec_bits as i16 as i32;
    let mut value = n.raw as f64;
    let mut remaining = decimal.abs();
    // 10^309 本身会溢出，但 raw/10^309 可能仍是有效 subnormal；
    // 分段换算避免中间幂溢出把可表示的值误判为零。
    while remaining > 0 {
        let step = remaining.min(308);
        let scale = 10f64.powi(step);
        value = if decimal >= 0 {
            value / scale
        } else {
            value * scale
        };
        remaining -= step;
        if !value.is_finite() || value == 0.0 {
            break;
        }
    }
    if !value.is_finite() || (n.raw != 0 && value == 0.0) {
        return Err(WireError::Unsupported("load percentage outside F64 range"));
    }
    Ok(Value::F64(value))
}
