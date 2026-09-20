//! FOCAS Ethernet Wire（PR1）：`FocasClient` + typed operations + Mesa adapter。
//!
//! - `FocasClient` 持有 `Mutex<Option<WireSession>>`：Operation 全程持
//!   session guard（多步如 statinfo 的 2 次 exchange 不可 interleaving）。
//! - Typed results 忠于 Wire（`SystemInfo` 7 字段 / `StatusInfo` 7×u16），
//!   不直接等于 Mesa `Value`；Mesa 映射由 `WireFocasApi` 做。
//! - Gate 0（165）冻结：`status_info` 请求序列 = frame#1(`0x18` count=1) +
//!   frame#2(`0x19+0xe1+0x98` count=3)，忠于已捕获组合，不做 `0x19-only`
//!   优化；响应只消费 `0x19` subpacket，`0xe1/0x98` 验证 framing 后跳过
//!   （unknown by design，不命名、不映射）。
//! - V1 只读：本文件无任何 write/program/control 路径。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use super::WireError;
use super::frame::{
    FocasFrame, GenericSubpacket, PacketType, REQUEST_ORIGIN, decode_generic_payload,
    encode_generic_request, request_subpacket,
};
use super::session::WireSession;
use crate::address::FocasAddress;
use crate::focas_api::{FocasApi, FocasSysInfo};

use mesa_core_types::Value;

// ---------------------------------------------------------------------------
// Typed results（忠于 Wire，不等于 Mesa Value）
// ---------------------------------------------------------------------------

/// FOCAS `system_info`（`0x18`）typed 结果。Gate 0：18B payload
/// `addinfo/max_axis/cnc_type/mt_type/series/version/axes`。
/// Adapter 只取 `series/version` 进 `FocasSysInfo`，其余保留供诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    /// 附加信息（165 实测 514）。
    pub addinfo: i16,
    /// 最大轴数（165 实测 32；FWLIB 上限语义，非实际轴数）。
    pub max_axis: i16,
    /// CNC 类型原文（如 `"30"`）。
    pub cnc_type: String,
    /// 机床类型原文（如 `"M"`）。
    pub mt_type: String,
    /// 系列原文（如 `"G31Z"`）。
    pub series: String,
    /// 版本原文（如 `"10.0"`）。
    pub version: String,
    /// 轴数原文（如 `"03"`）。
    pub axes: String,
}

/// FOCAS `status_info`（`0x19`）typed 结果。Gate 0：14B = 7×u16 BE
/// （`aut/run/motion/mstb/emergency/alarm/edit`）；Native 独有的
/// `hdck/tmmode` 不在 Wire 上，绝不硬凑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusInfo {
    /// 操作模式选择（`machine/status` 即此字段；MEM=1/MDI=0 已验证）。
    pub aut: u16,
    /// 运行状态。
    pub run: u16,
    /// 轴/dwell 状态。
    pub motion: u16,
    /// M/S/T/B 状态。
    pub mstb: u16,
    /// 急停状态。
    pub emergency: u16,
    /// 报警状态。
    pub alarm: u16,
    /// 编辑状态。
    pub edit: u16,
}

/// FOCAS `feed_rate`（`0x24`）typed 结果。feed 证据 PASS：
/// 8B = `mantissa(i32 BE) + meta0 + base + meta1 + exponent`。
/// 已知字段 typed；`meta0/meta1`（byte4/byte6）语义未知 → `raw` 保留，
/// 绝不命名（pyfanuc 只用 `[0..4]/[5]/[7]`，`FF FF` sentinel 在 165
/// 未见过，见过再加，不断言）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedRate {
    /// 原始 8B（未知字节保留，供未来机型差异对照）。
    pub raw: [u8; 8],
    /// 定点尾数（i32 BE；165 实测 `100`）。
    pub mantissa: i32,
    /// 缩放基（165 实测 `10`；`2` 有外部佐证但 165 未见）。
    pub base: u8,
    /// 缩放指数（165 实测 `0`；非 0 已在公共 sample 见过，不假设）。
    pub exponent: u8,
}

impl FeedRate {
    /// 数值语义：`mantissa / base^exponent`（`base` 仅接受真机实证值）。
    /// 当前 165 只实证 `base == 10`；`2` 有外部实现佐证但 165 未见，
    /// PR2 极度保守：非 `10` 即 `Unsupported`（见过再放开，不猜）。
    /// 返回 `(numer, denom)`（不做除法、不丢精度，由 adapter 判无损）。
    pub fn scaled(&self) -> Result<(i64, i64), WireError> {
        if self.base != 10 {
            return Err(WireError::Unsupported("feed base != 10"));
        }
        if self.exponent > 9 {
            return Err(WireError::Unsupported("feed exponent > 9"));
        }
        let denom = 10i64.pow(self.exponent as u32);
        Ok((self.mantissa as i64, denom))
    }
}

/// FOCAS `axis_absolute`（`0x26`）typed 结果。axis 证据 PASS：
/// 8B = `mantissa(i32 BE) + meta0 + base + meta1 + exponent`
/// （与 feed 同构，但独立类型，不抽公共 `ScaledValue8`——门槛是 spindle
/// 独立证明后再议；此处宁愿重复 15 行 decoder）。
/// `meta0/meta1`（byte4/byte6）语义未知 → `raw` 保留，绝不命名。
/// signed i32 完全合法（-2880/-2227/-3160/-10 均有真机证据），
/// 与 feed 的 `mantissa < 0 → fail-closed` 无关，各自独立规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisPosition {
    /// 原始 8B（未知字节保留，供未来机型差异对照）。
    pub raw: [u8; 8],
    /// 定点尾数（i32 BE；165 实测 `-2880` 等；Mesa 取此值）。
    pub mantissa: i32,
    /// 缩放基（165 实测 `10`）。
    pub base: u8,
    /// 缩放指数（165 实测 `3`）。
    pub exponent: u8,
}

impl AxisPosition {
    /// 有效性门：`base == 10` 且 `exponent <= 9`（165 实证范围；
    /// N4 `exp=51` 即 `Unsupported` → ERR/BAD，codec 照常解出字段）。
    pub fn validate(&self) -> Result<(), WireError> {
        if self.base != 10 {
            return Err(WireError::Unsupported("axis base != 10"));
        }
        if self.exponent > 9 {
            return Err(WireError::Unsupported("axis exponent > 9"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wire layout 常量（PR1 review：反复出现的 offset/length 命名；
// 不引入 BinaryReader/CodecBuilder）。
// ---------------------------------------------------------------------------

/// GENERIC 响应 subpacket 前缀（`6×00`，B1 实测）。
pub const RESPONSE_PREFIX_LEN: usize = 6;
/// GENERIC 响应 `data_len` 字段（u16 BE）。
pub const DATA_LEN_FIELD_LEN: usize = 2;
/// SYSINFO 数据体（18B ODBSYS）。
pub const SYSINFO_DATA_LEN: usize = 18;
/// STATINFO 数据体（14B = 7×u16）。
pub const STATINFO_DATA_LEN: usize = 14;
/// FEED 数据体（8B scaled value）。
pub const FEED_DATA_LEN: usize = 8;
/// AXIS 数据体（8B scaled value，与 feed 同构但独立常量，不共享语义）。
pub const AXIS_DATA_LEN: usize = 8;

// ---------------------------------------------------------------------------
// Function id（Gate 0 实测 lead；response codec 以真机为准）
// ---------------------------------------------------------------------------

/// CNC 设备（Gate 0：`0x0001`；PMC=`0x0002`，PR3 axis 未用）。
pub(super) const DEV_CNC: u16 = 0x0001;
/// `system_info`（Gate 0：`00 01 00 18`）。
const FUNC_SYSINFO: u32 = 0x0001_0018;
/// `status_info`（Gate 0：`00 01 00 19`）。
const FUNC_STATINFO: u32 = 0x0001_0019;
/// statinfo 序列伴随 function（未知语义；只验 framing 后跳过）。
const FUNC_UNKNOWN_E1: u32 = 0x0001_00e1;
/// statinfo 序列伴随 function（未知语义；只验 framing 后跳过）。
const FUNC_UNKNOWN_98: u32 = 0x0001_0098;
/// `feed_rate`（feed 证据 PASS：`00 01 00 24`，`0x24-only` 真机冻结）。
pub(super) const FUNC_FEED: u32 = 0x0001_0024;
/// `axis_absolute`（axis 证据 PASS：`00 01 00 26`，`v0=4/v1=ordinal` 真机冻结；
/// A0 `[2,3,1]` 为 FWLIB-local 历史异常，不复刻，fresh Wire 恒直透）。
pub(super) const FUNC_AXIS_ABSOLUTE: u32 = 0x0001_0026;
/// axis `0x26` 请求 kind（165 实测恒 `4`；语义未知，不命名）。
pub(super) const AXIS_KIND_ABSOLUTE: i32 = 4;

// ---------------------------------------------------------------------------
// FocasClient：typed operations（串行，session guard 覆盖完整 operation）
// ---------------------------------------------------------------------------

/// FOCAS Wire 客户端。`session: Mutex<Option<WireSession>>`：
/// 每个 Operation 持 guard 做完 N 次 exchange 再放（statinfo 的 2 次
/// exchange 中间不可插入别的 request），错误致命即 `None`（由上层重连）。
pub struct FocasClient {
    session: Mutex<Option<WireSession>>,
    timeout: Duration,
}

impl FocasClient {
    /// 新建未连接客户端（连接参数在 `ensure_connected` 时传入）。
    pub fn new(timeout: Duration) -> Self {
        Self {
            session: Mutex::new(None),
            timeout,
        }
    }

    /// 建连（OPEN）。已连接则复用；失败即 `None`（调用方重连）。
    pub async fn ensure_connected(&self, host: &str, port: u16) -> Result<(), WireError> {
        let mut guard = self.session.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let session = WireSession::connect(host, port, self.timeout).await?;
        *guard = Some(session);
        Ok(())
    }

    /// 断开（CLOSE，best-effort；无论成败 session 清空）。
    pub async fn disconnect(&self) {
        let mut guard = self.session.lock().await;
        if let Some(session) = guard.take() {
            let _ = session.close().await;
        }
    }

    /// session 致命错误后失效（`None`），下次 operation 重连。
    async fn invalidate(&self) {
        *self.session.lock().await = None;
    }

    /// `system_info`（`0x18`，单次 exchange）。返回完整 7 字段。
    pub async fn system_info(&self) -> Result<SystemInfo, WireError> {
        let req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_SYSINFO,
                [0, 0, 0, 0, 0],
            )]),
        };
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or(WireError::Closed)?;
        let resp = session.exchange(&req, PacketType::GENERIC_RESPONSE).await;
        let resp = match resp {
            Ok(v) => v,
            Err(e) => {
                let fatal = e.is_session_fatal();
                drop(guard);
                if fatal {
                    self.invalidate().await;
                }
                return Err(e);
            }
        };
        drop(guard);
        match decode_system_info(&resp) {
            Ok(v) => Ok(v),
            Err(e) => {
                if e.is_session_fatal() {
                    self.invalidate().await;
                }
                Err(e)
            }
        }
    }

    /// `status_info`（Gate 0 序列：frame#1 `0x18` + frame#2
    /// `0x19+0xe1+0x98`，两次 exchange 同一 guard 内完成）。
    /// 只消费 `0x19` 的 7×u16；`0xe1/0x98` 验 framing 后跳过。
    pub async fn status_info(&self) -> Result<StatusInfo, WireError> {
        // frame#1 的请求在 guard 外构造（纯字节，不碰 session）。
        let pre_req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_SYSINFO,
                [0, 0, 0, 0, 0],
            )]),
        };
        let req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[
                request_subpacket(DEV_CNC, FUNC_STATINFO, [0, 0, 0, 0, 0]),
                request_subpacket(DEV_CNC, FUNC_UNKNOWN_E1, [0, 0, 0, 0, 0]),
                request_subpacket(DEV_CNC, FUNC_UNKNOWN_98, [0, 0, 0, 0, 0]),
            ]),
        };
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or(WireError::Closed)?;
        // frame#1：sysinfo 自查（FWLIB 序列忠实复刻；响应只验 type）。
        let r = session
            .exchange(&pre_req, PacketType::GENERIC_RESPONSE)
            .await;
        if let Err(e) = r {
            let fatal = e.is_session_fatal();
            drop(guard);
            if fatal {
                self.invalidate().await;
            }
            return Err(e);
        }
        // frame#2：0x19 + 0xe1 + 0x98（count=3，忠于捕获）。
        let resp = session.exchange(&req, PacketType::GENERIC_RESPONSE).await;
        let resp = match resp {
            Ok(v) => v,
            Err(e) => {
                let fatal = e.is_session_fatal();
                drop(guard);
                if fatal {
                    self.invalidate().await;
                }
                return Err(e);
            }
        };
        drop(guard);
        match decode_status_info(&resp) {
            Ok(v) => Ok(v),
            Err(e) => {
                if e.is_session_fatal() {
                    self.invalidate().await;
                }
                Err(e)
            }
        }
    }

    /// `feed_rate`（`0x24-only`：count=1，`0x24-only` Gate 真机冻结形态）。
    /// 单次 exchange；响应 `count=1 + 0x24 / 6×00 / dlen=8 / 8B`。
    pub async fn feed_rate(&self) -> Result<FeedRate, WireError> {
        let req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_FEED,
                [0, 0, 0, 0, 0],
            )]),
        };
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or(WireError::Closed)?;
        let resp = session.exchange(&req, PacketType::GENERIC_RESPONSE).await;
        let resp = match resp {
            Ok(v) => v,
            Err(e) => {
                let fatal = e.is_session_fatal();
                drop(guard);
                if fatal {
                    self.invalidate().await;
                }
                return Err(e);
            }
        };
        drop(guard);
        match decode_feed_rate(&resp) {
            Ok(v) => Ok(v),
            Err(e) => {
                if e.is_session_fatal() {
                    self.invalidate().await;
                }
                Err(e)
            }
        }
    }

    /// `axis_absolute(axis)`（axis 证据 PASS：`0x18` preflight + `0x26`
    /// count=1，`v0=4/v1=ordinal`；A0 `[2,3,1]` 为 FWLIB-local 历史异常，
    /// fresh Wire 恒直透，不复刻）。响应单 8B position value。
    pub async fn axis_absolute(&self, axis: u8) -> Result<AxisPosition, WireError> {
        // `v1 = ordinal`（165 observed mapping；A0 乱序已定性为陈旧状态）。
        // 产品上限外（0 或 >8）fail-closed，不发包（与 Native `Param` 同语义）。
        if axis == 0 || axis > 8 {
            return Err(WireError::Unsupported("axis ordinal 1..8"));
        }
        let pre_req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_SYSINFO,
                [0, 0, 0, 0, 0],
            )]),
        };
        let req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_AXIS_ABSOLUTE,
                [AXIS_KIND_ABSOLUTE, axis as i32, 0, 0, 0],
            )]),
        };
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or(WireError::Closed)?;
        // frame#1：sysinfo 自查（direct cnc_absolute 捕获形态，忠实复刻）。
        let r = session
            .exchange(&pre_req, PacketType::GENERIC_RESPONSE)
            .await;
        if let Err(e) = r {
            let fatal = e.is_session_fatal();
            drop(guard);
            if fatal {
                self.invalidate().await;
            }
            return Err(e);
        }
        // frame#2：0x26 count=1。
        let resp = session.exchange(&req, PacketType::GENERIC_RESPONSE).await;
        let resp = match resp {
            Ok(v) => v,
            Err(e) => {
                let fatal = e.is_session_fatal();
                drop(guard);
                if fatal {
                    self.invalidate().await;
                }
                return Err(e);
            }
        };
        drop(guard);
        match decode_axis_position(&resp) {
            Ok(v) => Ok(v),
            Err(e) => {
                if e.is_session_fatal() {
                    self.invalidate().await;
                }
                Err(e)
            }
        }
    }
}

/// `0x18` 响应解码：sub.payload = 6×00 + u16 data_len + 18B ODBSYS
/// （B1 实测，不假设 `5×i32`）。字符区非可打印即 `Malformed`（绝不猜）。
pub(super) fn decode_system_info(resp: &FocasFrame) -> Result<SystemInfo, WireError> {
    let subs = decode_generic_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub = find_function(&subs, DEV_CNC, FUNC_SYSINFO).ok_or(WireError::CommandMismatch)?;
    let p = &sub.payload;
    // B1 实测：p = 6×00 + 00 12 + 18B。
    if p.len() < RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + SYSINFO_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    if p[0..RESPONSE_PREFIX_LEN] != [0u8; RESPONSE_PREFIX_LEN] {
        return Err(WireError::MalformedPayload);
    }
    let data_len =
        u16::from_be_bytes([p[RESPONSE_PREFIX_LEN], p[RESPONSE_PREFIX_LEN + 1]]) as usize;
    if data_len != SYSINFO_DATA_LEN
        || p.len() < RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + SYSINFO_DATA_LEN
    {
        return Err(WireError::MalformedPayload);
    }
    let d = &p[RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN
        ..RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + SYSINFO_DATA_LEN];
    let ascii = |b: &[u8]| -> Result<String, WireError> {
        let s = String::from_utf8_lossy(b)
            .trim_matches('\0')
            .trim()
            .to_string();
        if s.is_empty() || !s.bytes().all(|c| c.is_ascii_graphic() || c == b' ') {
            return Err(WireError::MalformedPayload);
        }
        Ok(s)
    };
    Ok(SystemInfo {
        addinfo: i16::from_be_bytes([d[0], d[1]]),
        max_axis: i16::from_be_bytes([d[2], d[3]]),
        cnc_type: ascii(&d[4..6])?,
        mt_type: ascii(&d[6..8])?,
        series: ascii(&d[8..12])?,
        version: ascii(&d[12..16])?,
        axes: ascii(&d[16..18])?,
    })
}

/// `0x19` 响应解码：多 subpacket 中找 `0x19`，sub.payload =
/// 6×00 + u16 data_len + 14B(7×u16 BE)。缺 `0x19` 即 `CommandMismatch`。
pub(super) fn decode_status_info(resp: &FocasFrame) -> Result<StatusInfo, WireError> {
    let subs = decode_generic_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub = find_function(&subs, DEV_CNC, FUNC_STATINFO).ok_or(WireError::CommandMismatch)?;
    let p = &sub.payload;
    if p.len() < RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + STATINFO_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    if p[0..RESPONSE_PREFIX_LEN] != [0u8; RESPONSE_PREFIX_LEN] {
        return Err(WireError::MalformedPayload);
    }
    let data_len =
        u16::from_be_bytes([p[RESPONSE_PREFIX_LEN], p[RESPONSE_PREFIX_LEN + 1]]) as usize;
    if data_len < STATINFO_DATA_LEN || p.len() < RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + data_len
    {
        return Err(WireError::MalformedPayload);
    }
    let d = &p[RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN
        ..RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + STATINFO_DATA_LEN];
    // 7×u16 BE：aut/run/motion/mstb/emergency/alarm/edit（Gate 0 B1/B2 双闭合）。
    let u = |i: usize| u16::from_be_bytes([d[i], d[i + 1]]);
    Ok(StatusInfo {
        aut: u(0),
        run: u(2),
        motion: u(4),
        mstb: u(6),
        emergency: u(8),
        alarm: u(10),
        edit: u(12),
    })
}

/// 在多 subpacket 响应中按 `(control_device, function)` 找目标。
fn find_function(subs: &[GenericSubpacket], dev: u16, func: u32) -> Option<&GenericSubpacket> {
    subs.iter()
        .find(|s| s.control_device == dev && s.function == func)
}

/// `0x24` 响应解码：单 subpacket，sub.payload =
/// 6×00 + u16 data_len(=8) + 8B scaled value。
/// 8B = `mantissa(i32 BE) + meta0 + base + meta1 + exponent`；
/// `meta0/meta1` 不解释（进 `raw`）。缺 `0x24` 即 `CommandMismatch`。
pub(super) fn decode_feed_rate(resp: &FocasFrame) -> Result<FeedRate, WireError> {
    let subs = decode_generic_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub = find_function(&subs, DEV_CNC, FUNC_FEED).ok_or(WireError::CommandMismatch)?;
    let p = &sub.payload;
    // 精确闭合：p 必须恰好 = 6×00 + u16 data_len(=8) + 8B（多 1B 即错，
    // 与 frame length → count → subpacket length 同原则）。
    let expected_len = RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + FEED_DATA_LEN;
    if p.len() != expected_len {
        return Err(WireError::MalformedPayload);
    }
    if p[0..RESPONSE_PREFIX_LEN] != [0u8; RESPONSE_PREFIX_LEN] {
        return Err(WireError::MalformedPayload);
    }
    let data_len =
        u16::from_be_bytes([p[RESPONSE_PREFIX_LEN], p[RESPONSE_PREFIX_LEN + 1]]) as usize;
    if data_len != FEED_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    let d = &p[RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN
        ..RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + FEED_DATA_LEN];
    let mut raw = [0u8; 8];
    raw.copy_from_slice(d);
    Ok(FeedRate {
        raw,
        mantissa: i32::from_be_bytes([d[0], d[1], d[2], d[3]]),
        base: d[5],
        exponent: d[7],
    })
}

/// `0x26` 响应解码：单 subpacket，sub.payload =
/// 6×00 + u16 data_len(=8) + 8B scaled value（与 feed 同构，独立 decoder，
/// 不抽公共类型）。缺 `0x26` 即 `CommandMismatch`。
pub(super) fn decode_axis_position(resp: &FocasFrame) -> Result<AxisPosition, WireError> {
    let subs = decode_generic_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub =
        find_function(&subs, DEV_CNC, FUNC_AXIS_ABSOLUTE).ok_or(WireError::CommandMismatch)?;
    let p = &sub.payload;
    let expected_len = RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + AXIS_DATA_LEN;
    if p.len() != expected_len {
        return Err(WireError::MalformedPayload);
    }
    if p[0..RESPONSE_PREFIX_LEN] != [0u8; RESPONSE_PREFIX_LEN] {
        return Err(WireError::MalformedPayload);
    }
    let data_len =
        u16::from_be_bytes([p[RESPONSE_PREFIX_LEN], p[RESPONSE_PREFIX_LEN + 1]]) as usize;
    if data_len != AXIS_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    let d = &p[RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN
        ..RESPONSE_PREFIX_LEN + DATA_LEN_FIELD_LEN + AXIS_DATA_LEN];
    let mut raw = [0u8; 8];
    raw.copy_from_slice(d);
    Ok(AxisPosition {
        raw,
        mantissa: i32::from_be_bytes([d[0], d[1], d[2], d[3]]),
        base: d[5],
        exponent: d[7],
    })
}

/// Mesa `machine/feed` 无损映射：`(mantissa, denom)` →
/// `Value::U32`。任一失败即 `Err`（fail-closed，不 truncate/round/clamp）：
/// `mantissa < 0` / 分母为 0 / 不能整除 / 超 `u32::MAX`。
fn feed_to_value(rate: &FeedRate) -> Result<Value, WireError> {
    let (numer, denom) = rate.scaled()?;
    if denom <= 0 {
        return Err(WireError::Unsupported("feed denom <= 0"));
    }
    if numer < 0 {
        return Err(WireError::Unsupported("feed mantissa < 0"));
    }
    if numer % denom != 0 {
        return Err(WireError::Unsupported("feed not integral"));
    }
    let v = numer / denom;
    if v > u32::MAX as i64 {
        return Err(WireError::Unsupported("feed > u32::MAX"));
    }
    Ok(Value::U32(v as u32))
}

/// Mesa `axis.absolute` 映射：`validate(base/exp)` 通过即
/// `Value::I32(mantissa)`（mantissa 可负，-2880 等均有真机证据；
/// 与 feed 的 `mantissa<0` 拒绝无关，各自独立规则）。
fn axis_to_value(pos: &AxisPosition) -> Result<Value, WireError> {
    pos.validate()?;
    Ok(Value::I32(pos.mantissa))
}

/// fixture/test 专用：生产 `axis_to_value` 同源入口（`#[cfg(test)]`，
/// 不出 crate；fixture 回归与 N4 fail-closed 断言用）。
#[cfg(test)]
pub(super) fn axis_to_value_for_test(pos: &AxisPosition) -> Result<Value, WireError> {
    axis_to_value(pos)
}

/// fixture/test 专用：`Value::I32` 构造子（断言可读性用）。
#[cfg(test)]
pub(super) fn axis_value_for_test(v: i32) -> Value {
    Value::I32(v)
}

// ---------------------------------------------------------------------------
// WireFocasApi：Mesa adapter（PR1 最小 + PR2 feed + PR3 axis）
// ---------------------------------------------------------------------------

/// Wire 版 `FocasApi`（PR3：`system_info` + `Status` + `Feed` + `Axis/absolute`；
/// 其余地址 `Unsupported`，fail-closed，不猜、不 fallback Native）。
pub struct WireFocasApi {
    client: Arc<FocasClient>,
    host: Mutex<Option<(String, u16)>>,
}

impl WireFocasApi {
    /// 新建（timeout 由 endpoint 配置透传）。
    pub fn new(timeout: Duration) -> Self {
        Self {
            client: Arc::new(FocasClient::new(timeout)),
            host: Mutex::new(None),
        }
    }

    /// 单点错误 ↔ 连接错误的分类：point-local（`Unsupported/Remote`）
    /// 即 `ERR:` 占位（上层转单点 BAD）；session 致命即整批 `Err`（重连）。
    fn point_or_fatal(e: WireError) -> Result<Value, String> {
        match e {
            WireError::Unsupported(_) | WireError::Remote(_) => {
                Ok(Value::String(format!("ERR:{e}")))
            }
            _ => Err(e.to_string()),
        }
    }
}

#[async_trait::async_trait]
impl FocasApi for WireFocasApi {
    async fn connect(&self, host: &str, port: u16, _timeout_ms: u64) -> Result<(), String> {
        *self.host.lock().await = Some((host.to_string(), port));
        let (h, p) = self.host.lock().await.clone().unwrap();
        self.client
            .ensure_connected(&h, p)
            .await
            .map_err(|e| e.to_string())
    }

    async fn read_batch(&self, addresses: &[FocasAddress]) -> Result<Vec<Value>, String> {
        if addresses.is_empty() {
            return Ok(Vec::new());
        }
        // 同一批共享请求：Status/Feed 各一次；Axis 按轴号各一次
        // （`0x26` 单轴语义，无多轴数组）；其余 fail-closed。
        // client 内部按 operation 持 guard。
        // 致命 session 错误在预取阶段立即短路（point-local 才进批）。
        // 非 Absolute 的 Axis kind（Machine/Relative/…）与 Native 同口径
        // fail-closed（ERR → BAD），绝不用 absolute 冒充。
        use crate::address::AxisKind;
        use std::collections::BTreeMap;
        let need_status = addresses.iter().any(|a| matches!(a, FocasAddress::Status));
        let need_feed = addresses.iter().any(|a| matches!(a, FocasAddress::Feed));
        // 去重后的 Absolute 轴号（保序；axis 0/超限在 operation 内 fail-closed）。
        let mut axis_order: Vec<u8> = Vec::new();
        for a in addresses {
            if let FocasAddress::Axis { axis, kind } = a
                && *kind == AxisKind::Absolute
                && !axis_order.contains(axis)
            {
                axis_order.push(*axis);
            }
        }
        let status_r: Option<Result<u32, String>> = if need_status {
            match self.client.status_info().await {
                Ok(st) => Some(Ok(st.aut as u32)),
                Err(e) => match Self::point_or_fatal(e) {
                    Ok(Value::String(s)) => Some(Err(s)),
                    Ok(_) => unreachable!("point_or_fatal Ok 必为 String"),
                    Err(fatal) => return Err(fatal),
                },
            }
        } else {
            None
        };
        let feed_r: Option<Result<Value, String>> = if need_feed {
            match self.client.feed_rate().await {
                Ok(rate) => match feed_to_value(&rate) {
                    Ok(v) => Some(Ok(v)),
                    Err(e) => match Self::point_or_fatal(e) {
                        Ok(Value::String(s)) => Some(Err(s)),
                        Ok(_) => unreachable!("point_or_fatal Ok 必为 String"),
                        Err(fatal) => return Err(fatal),
                    },
                },
                Err(e) => match Self::point_or_fatal(e) {
                    Ok(v) => Some(Ok(v)),
                    Err(fatal) => return Err(fatal),
                },
            }
        } else {
            None
        };
        // Axis 按轴号各一次 0x26（单轴语义；axis4 等无效轴 fail-closed 进批）。
        let mut axis_map: BTreeMap<u8, Result<Value, String>> = BTreeMap::new();
        for axis in &axis_order {
            let r: Result<Value, String> = match self.client.axis_absolute(*axis).await {
                Ok(pos) => match axis_to_value(&pos) {
                    Ok(v) => Ok(v),
                    Err(e) => match Self::point_or_fatal(e) {
                        Ok(Value::String(s)) => Err(s),
                        Ok(_) => unreachable!("point_or_fatal Ok 必为 String"),
                        Err(fatal) => return Err(fatal),
                    },
                },
                Err(e) => match Self::point_or_fatal(e) {
                    Ok(v) => Ok(v),
                    Err(fatal) => return Err(fatal),
                },
            };
            axis_map.insert(*axis, r);
        }
        let mut out = Vec::with_capacity(addresses.len());
        for addr in addresses {
            match addr {
                FocasAddress::Status => match status_r.clone().unwrap() {
                    Ok(aut) => out.push(Value::U32(aut)),
                    Err(e) if e.starts_with("ERR:") => out.push(Value::String(e)),
                    Err(fatal) => return Err(fatal),
                },
                FocasAddress::Feed => match feed_r.clone().unwrap() {
                    Ok(v) => out.push(v),
                    Err(e) if e.starts_with("ERR:") => out.push(Value::String(e)),
                    Err(fatal) => return Err(fatal),
                },
                FocasAddress::Axis { axis, kind } if *kind == AxisKind::Absolute => {
                    match axis_map.get(axis).cloned().unwrap() {
                        Ok(v) => out.push(v),
                        Err(e) if e.starts_with("ERR:") => out.push(Value::String(e)),
                        Err(fatal) => return Err(fatal),
                    }
                }
                _ => out.push(Value::String(format!(
                    "ERR:{}",
                    WireError::Unsupported("PR3 only Status/Feed/Absolute")
                ))),
            }
        }
        Ok(out)
    }

    async fn system_info(&self) -> Result<FocasSysInfo, String> {
        let info = self.client.system_info().await.map_err(|e| e.to_string())?;
        Ok(FocasSysInfo {
            series: info.series,
            version: info.version,
        })
    }

    async fn disconnect(&self) {
        self.client.disconnect().await;
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame::{GenericSubpacket, encode_generic_request, request_subpacket};
    use super::*;

    /// Gate 0 B1 精确帧：frame#2 请求编码必须 `0x56/count=3`。
    #[test]
    fn statinfo_frame2_request_locked() {
        let subs = vec![
            request_subpacket(DEV_CNC, FUNC_STATINFO, [0, 0, 0, 0, 0]),
            request_subpacket(DEV_CNC, FUNC_UNKNOWN_E1, [0, 0, 0, 0, 0]),
            request_subpacket(DEV_CNC, FUNC_UNKNOWN_98, [0, 0, 0, 0, 0]),
        ];
        let payload = encode_generic_request(&subs);
        assert_eq!(payload.len(), 0x56, "frame#2 必须 86B（Gate 0 B1）");
        assert_eq!(&payload[0..2], &[0x00, 0x03]);
    }

    /// Fixture 级回归入口（`tests/fixtures/wire/**`）：生产 codec 直测，
    /// 不经测试侧复刻 decoder（见 `super::super::fixture_tests`）。
    #[test]
    fn fixture_production_codec_locked() {
        super::super::fixture_tests::run_all();
    }

    /// Gate 0 B1：sysinfo 响应解码 7/7（`02 02 00 20 33 30 …`）。
    #[test]
    fn decode_sysinfo_b1_locked() {
        // sub.payload = function 之后 = 6×00 + 00 12 + 18B（B1 实测 26B）。
        let mut payload = vec![0x00; 6];
        payload.extend_from_slice(&[0x00, 0x12]);
        payload.extend_from_slice(&[
            0x02, 0x02, 0x00, 0x20, 0x33, 0x30, 0x20, 0x4d, 0x47, 0x33, 0x31, 0x5a, 0x31, 0x30,
            0x2e, 0x30, 0x30, 0x33,
        ]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_SYSINFO,
                payload,
            }]),
        };
        let info = decode_system_info(&frame).unwrap();
        assert_eq!(info.addinfo, 514);
        assert_eq!(info.max_axis, 32);
        assert_eq!(info.cnc_type, "30");
        assert_eq!(info.mt_type, "M");
        assert_eq!(info.series, "G31Z");
        assert_eq!(info.version, "10.0");
        assert_eq!(info.axes, "03");
    }

    /// Gate 0 B1：statinfo 响应解码 7/7（MEM：`00 01 00 01 …`）。
    #[test]
    fn decode_statinfo_b1_locked() {
        // sub1(0x19)：6×00 + 0x0e + 14B(MEM：aut=1/run=1/其余0)。
        let mut p19 = vec![0x00; 6];
        p19.extend_from_slice(&[0x00, 0x0e]);
        p19.extend_from_slice(&[
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        // sub2(0xe1)：6×00 + 0x04 + 4×00；sub3(0x98)：6×00 + 0x02 + 2×00。
        let mut pe1 = vec![0x00; 6];
        pe1.extend_from_slice(&[0x00, 0x04, 0x00, 0x00, 0x00, 0x00]);
        let mut p98 = vec![0x00; 6];
        p98.extend_from_slice(&[0x00, 0x02, 0x00, 0x00]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[
                GenericSubpacket {
                    control_device: DEV_CNC,
                    function: FUNC_STATINFO,
                    payload: p19,
                },
                GenericSubpacket {
                    control_device: DEV_CNC,
                    function: FUNC_UNKNOWN_E1,
                    payload: pe1,
                },
                GenericSubpacket {
                    control_device: DEV_CNC,
                    function: FUNC_UNKNOWN_98,
                    payload: p98,
                },
            ]),
        };
        // 2 + 30 + 20 + 18 = 70 = 0x46（Gate 0 B1 闭合）。
        assert_eq!(frame.payload.len(), 0x46);
        let st = decode_status_info(&frame).unwrap();
        assert_eq!(
            st,
            StatusInfo {
                aut: 1,
                run: 1,
                motion: 0,
                mstb: 0,
                emergency: 0,
                alarm: 0,
                edit: 0,
            },
            "B1 MEM 必须 7/7 同次一致"
        );
    }

    /// Gate 0 B2：MDI（`aut=0/run=1`）同样解码。
    #[test]
    fn decode_statinfo_b2_locked() {
        let mut p19 = vec![0x00; 6];
        p19.extend_from_slice(&[0x00, 0x0e]);
        p19.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_STATINFO,
                payload: p19,
            }]),
        };
        let st = decode_status_info(&frame).unwrap();
        assert_eq!(st.aut, 0);
        assert_eq!(st.run, 1);
    }

    /// 缺 `0x19` 即 `CommandMismatch`（保守致命：响应与请求对不上时
    /// 无法证明流还在边界上；point-local 的业务失败走 `Remote`）。
    #[test]
    fn missing_statinfo_is_mismatch() {
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_UNKNOWN_E1,
                payload: vec![0x00; 10],
            }]),
        };
        let e = decode_status_info(&frame).unwrap_err();
        assert!(
            matches!(e, WireError::CommandMismatch),
            "缺 0x19 必须 CommandMismatch"
        );
        assert!(e.is_session_fatal(), "CommandMismatch 保守判致命");
    }

    /// feed `0x24` 请求：count=1（`0x24-only` Gate 冻结形态）。
    #[test]
    fn feed_request_single_locked() {
        use super::super::frame::encode_generic_request as enc;
        let payload = enc(&[request_subpacket(DEV_CNC, FUNC_FEED, [0, 0, 0, 0, 0])]);
        // count=1 + 28B = 30 = 0x1e（与 sysinfo 同长，function 换 0x24）。
        assert_eq!(payload.len(), 0x1e);
        assert_eq!(&payload[0..2], &[0x00, 0x01]);
        let back = super::super::frame::decode_generic_payload(&payload).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].function, FUNC_FEED);
    }

    /// feed 响应解码：`00 00 00 64 00 0a 00 00` → mantissa=100/base=10/exp=0。
    #[test]
    fn decode_feed_locked() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x00, 0x00, 0x00, 0x64, 0x00, 0x0a, 0x00, 0x00]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_FEED,
                payload: p,
            }]),
        };
        // 2 + 24 = 26 = 0x1a（0x24-only 探针实测）。
        assert_eq!(frame.payload.len(), 0x1a);
        let rate = decode_feed_rate(&frame).unwrap();
        assert_eq!(rate.mantissa, 100);
        assert_eq!(rate.base, 10);
        assert_eq!(rate.exponent, 0);
        assert_eq!(rate.raw, [0x00, 0x00, 0x00, 0x64, 0x00, 0x0a, 0x00, 0x00]);
        // scaled 无损：100/1。
        assert_eq!(rate.scaled().unwrap(), (100, 1));
        // adapter 无损入 U32。
        assert_eq!(feed_to_value(&rate).unwrap(), Value::U32(100));
    }

    /// feed 非整数 fail-closed：`12345/10^2 = 123.45` 不得 truncate 成 123。
    #[test]
    fn feed_fractional_is_bad() {
        let rate = FeedRate {
            raw: [0, 0, 0x30, 0x39, 0, 10, 0, 2],
            mantissa: 12345,
            base: 10,
            exponent: 2,
        };
        assert_eq!(rate.scaled().unwrap(), (12345, 100));
        let e = feed_to_value(&rate).unwrap_err();
        assert!(
            matches!(e, WireError::Unsupported(_)),
            "非整数 feed 必须 fail-closed"
        );
    }

    /// feed 负值 fail-closed：`mantissa < 0` 不得进 U32。
    #[test]
    fn feed_negative_is_bad() {
        let rate = FeedRate {
            raw: [0xFF, 0xFF, 0xFF, 0xFF, 0, 10, 0, 0],
            mantissa: -1,
            base: 10,
            exponent: 0,
        };
        assert!(matches!(
            feed_to_value(&rate).unwrap_err(),
            WireError::Unsupported(_)
        ));
    }

    /// feed 非实证 base fail-closed：`base=2` 有外部佐证但 165 未见，不猜。
    #[test]
    fn feed_unverified_base_is_bad() {
        let rate = FeedRate {
            raw: [0, 0, 0, 1, 0, 2, 0, 0],
            mantissa: 1,
            base: 2,
            exponent: 0,
        };
        assert!(matches!(
            rate.scaled().unwrap_err(),
            WireError::Unsupported(_)
        ));
    }

    /// feed typed payload 内部精确闭合：trailing 4B 即使 data_len 正确也拒收
    /// （与 frame length → count → subpacket length 同原则）。
    #[test]
    fn feed_trailing_payload_rejected() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x00, 0x00, 0x00, 0x64, 0x00, 0x0a, 0x00, 0x00]);
        p.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_FEED,
                payload: p,
            }]),
        };
        assert_eq!(
            decode_feed_rate(&frame).unwrap_err().to_string(),
            WireError::MalformedPayload.to_string(),
            "8B 后跟 4B 垃圾必须 Malformed"
        );
    }

    /// axis `0x26` 请求：`v0=4/v1=ordinal`（axis 证据 PASS 冻结形态）。
    #[test]
    fn axis_request_selector_locked() {
        let payload = encode_generic_request(&[request_subpacket(
            DEV_CNC,
            FUNC_AXIS_ABSOLUTE,
            [AXIS_KIND_ABSOLUTE, 2, 0, 0, 0],
        )]);
        // count=1 + 28B = 30 = 0x1e（与 sysinfo/feed 同长）。
        assert_eq!(payload.len(), 0x1e);
        assert_eq!(&payload[0..2], &[0x00, 0x01]);
        let back = decode_generic_payload(&payload).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].function, FUNC_AXIS_ABSOLUTE);
    }

    /// axis 响应解码：A0' `ff ff f4 c0 00 0a 00 03` → mantissa=-2880。
    #[test]
    fn decode_axis_locked() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0xFF, 0xFF, 0xF4, 0xC0, 0x00, 0x0A, 0x00, 0x03]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_AXIS_ABSOLUTE,
                payload: p,
            }]),
        };
        // 2 + 24 = 26 = 0x1a（direct cnc_absolute 捕获形态）。
        assert_eq!(frame.payload.len(), 0x1a);
        let pos = decode_axis_position(&frame).unwrap();
        assert_eq!(pos.mantissa, -2880);
        assert_eq!(pos.base, 10);
        assert_eq!(pos.exponent, 3);
        // adapter：负值合法 → I32（与 feed 的负值拒绝无关）。
        assert_eq!(axis_to_value(&pos).unwrap(), Value::I32(-2880));
    }

    /// axis N4：`exp=51` codec 照常解出字段，但 validate fail-closed。
    #[test]
    fn axis_n4_exp51_fails_closed() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x20, 0x00, 0x02, 0x02, 0x00, 0x0A, 0x30, 0x33]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_AXIS_ABSOLUTE,
                payload: p,
            }]),
        };
        let pos = decode_axis_position(&frame).unwrap();
        assert_eq!(pos.mantissa, 0x2000_0202);
        assert_eq!(pos.exponent, 51);
        // codec 成功，但语义层 fail-closed（ERR → BAD，不进 I32）。
        assert!(matches!(
            axis_to_value(&pos).unwrap_err(),
            WireError::Unsupported(_)
        ));
    }

    /// axis typed payload 内部精确闭合（与 feed 同原则）。
    #[test]
    fn axis_trailing_payload_rejected() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0xFF, 0xFF, 0xF4, 0xC0, 0x00, 0x0A, 0x00, 0x03]);
        p.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_AXIS_ABSOLUTE,
                payload: p,
            }]),
        };
        assert_eq!(
            decode_axis_position(&frame).unwrap_err().to_string(),
            WireError::MalformedPayload.to_string(),
        );
    }

    /// axis ordinal 越界 fail-closed：0 与 >8 不发包（与 Native Param 同语义）。
    #[tokio::test]
    async fn axis_ordinal_out_of_range() {
        let client = FocasClient::new(std::time::Duration::from_millis(10));
        for axis in [0u8, 9u8] {
            let e = client.axis_absolute(axis).await.unwrap_err();
            assert!(
                matches!(e, WireError::Unsupported(_)),
                "axis={axis} 必须 Unsupported（不发包）"
            );
        }
    }
}
