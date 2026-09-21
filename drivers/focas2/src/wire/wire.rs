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
    FocasFrame, GenericSubpacket, PacketType, REQUEST_ORIGIN, ReplySubpacket, decode_reply_payload,
    encode_generic_request, match_slot, request_subpacket,
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

/// 8B 数值单元真实布局（PR53）：`mantissa(i32 BE) + base(u16 BE) +
/// exponent(i16 BE)`。旧 `FeedRate/AxisPosition{base: u8, exponent: u8}`
/// 在已验证值（`0x000A/0x0003` 等）下结果相同，保留为兼容视图；
/// 新 `RawNumeric8` 为真实协议模型（axis4 `30 33` 即 `i16 12339`，
/// 不是 `u8 51`——fail-closed 结论不变，文档修正见 PR53）。
/// `engineering = mantissa / base^exponent`（有理数，不转 f64）；
/// `native_value = mantissa`（`cnc_actf/cnc_absolute/cnc_acts` 只取前 4B）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawNumeric8 {
    /// 原始 8B。
    pub raw: [u8; 8],
    /// 定点尾数（i32 BE）。
    pub mantissa: i32,
    /// 缩放基（u16 BE；165 实测 `10`）。
    pub base: u16,
    /// 缩放指数（i16 BE，有符号；165 实测 `0/3`，axis4 `12339`）。
    pub exponent: i16,
}

impl RawNumeric8 {
    /// 解码 8B（精确 8B，不多不少）。
    pub fn decode(raw: &[u8]) -> Result<Self, WireError> {
        if raw.len() != 8 {
            return Err(WireError::MalformedPayload);
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(raw);
        Ok(Self {
            raw: arr,
            mantissa: i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]),
            base: u16::from_be_bytes([raw[4], raw[5]]),
            exponent: i16::from_be_bytes([raw[6], raw[7]]),
        })
    }

    /// 有效性门（165 实证范围）：`base == 10` 且 `0 <= exponent <= 9`。
    /// N4（`exp=12339`）、负指数、非 10 基即 `Unsupported`（见过再放开）。
    /// B4 后生产 adapter（`feed_to_value`/`axis_to_value`）均以 raw 重建的
    /// 本门/本值为权威，不再走兼容视图旧门。
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<(), WireError> {
        if self.base != 10 {
            return Err(WireError::Unsupported("numeric base != 10"));
        }
        if !(0..=9).contains(&self.exponent) {
            return Err(WireError::Unsupported("numeric exponent out of range"));
        }
        Ok(())
    }

    /// Native 值（B1 冻结）：`mantissa`（`cnc_actf/cnc_absolute/cnc_acts`
    /// 只复制前 4B；与 `base/exponent` 无关，不因 `exp != 0` 拒绝）。
    /// 单测直断（生产 adapter 经 `feed_to_value` 取 `mantissa` 同源语义）。
    #[allow(dead_code)]
    pub fn native_value(&self) -> i32 {
        self.mantissa
    }

    /// 工程量有理数（`mantissa / base^exponent`，不转 f64、不丢精度）。
    /// `base != 10` / 负指数 / `exponent > 18` 即 `Unsupported`（见过再放开；
    /// `18` 为工程保护范围，不是 FANUC 协议限值）。
    pub fn engineering_value(&self) -> Result<(i64, i64), WireError> {
        if self.base != 10 {
            return Err(WireError::Unsupported("numeric base != 10"));
        }
        if self.exponent < 0 || self.exponent > 18 {
            return Err(WireError::Unsupported("numeric exponent out of range"));
        }
        let denom = 10i64.pow(self.exponent as u32);
        Ok((self.mantissa as i64, denom))
    }

    /// 兼容别名（旧名单测引用；与 `engineering_value` 同语义）。
    /// PR53 约束：已验证 bytes 不变；旧名单测逐步迁移到新名后删除。
    #[allow(dead_code)]
    pub fn engineering(&self) -> Result<(i64, i64), WireError> {
        self.engineering_value()
    }
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
    /// 数值语义（兼容视图；新代码走 `RawNumeric8::engineering`）。
    /// 当前 165 只实证 `base == 10`；非 `10` 即 `Unsupported`。
    /// 返回 `(numer, denom)`（不做除法、不丢精度，由 adapter 判无损）。
    #[allow(dead_code)]
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

/// FOCAS `axis_absolute`（`0x26`）typed 结果。axis 证据 PASS。
/// 旧 `{mantissa, base: u8, exponent: u8}` 在已验证值下与 `RawNumeric8`
/// 等价，保留为兼容视图；真实布局见 `RawNumeric8`。
/// signed i32 完全合法（-2880/-2227/-3160/-10 均有真机证据），
/// 与 feed 的 `mantissa < 0 → fail-closed` 无关，各自独立规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisPosition {
    /// 原始 8B（未知字节保留，供未来机型差异对照）。
    pub raw: [u8; 8],
    /// 定点尾数（i32 BE；165 实测 `-2880` 等；Mesa 取此值）。
    pub mantissa: i32,
    /// 缩放基（165 实测 `10`；真实为 u16，兼容视图截断）。
    pub base: u8,
    /// 缩放指数（165 实测 `3`；真实为 i16，N4 `12339` 在此截断为 51）。
    pub exponent: u8,
}

impl AxisPosition {
    /// 有效性门（兼容视图；生产 adapter 已改走 `RawNumeric8::validate`，
    /// 见 `axis_to_value` B4）。保留供旧单测引用，未使用告警允许。
    #[allow(dead_code)]
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

/// FOCAS `macro_value`（`0x15`）typed 结果。macro Evidence PASS。
/// 旧 `{mantissa, base: u8, exponent: u8}` 在 M0~M3（`0x000A` + `0/7/8`）下
/// 与 `RawNumeric8` 等价，保留为兼容视图；真实布局见 `RawNumeric8`。
/// 与 feed/spindle/axis 的关键区别（证据结论，非猜测）：Mesa 取
/// engineering/scaled F64（`mantissa / 10^exponent`），不是 native mantissa。
/// signed mantissa 合法（M3 `-750000000` 真机证据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacroValue {
    /// 原始 8B。
    pub raw: [u8; 8],
    /// 定点尾数（i32 BE；M0~M3 实测 `0/250000000/123450000/-750000000`）。
    pub mantissa: i32,
    /// 缩放基（M0~M3 实测 `10`；真实为 u16，兼容视图截断）。
    pub base: u8,
    /// 缩放指数（M0~M3 实测 `0/7/7/8`；真实为 i16）。
    pub exponent: u8,
}

/// FOCAS `spindle_speed`（`0x25`）typed 结果。spindle Evidence PASS。
/// 旧 `{mantissa, base: u8, exponent: u8}` 在 S0~S4（`0x000A/0x0000`）下
/// 与 `RawNumeric8` 等价，保留为兼容视图；真实布局见 `RawNumeric8`。
/// `cnc_acts` 无 spindle 实例语义（只有 ActiveSpindleSpeed 可调，
/// indexed speed fail-closed，与 PR52 Native 门同口径）。
/// mantissa 可为 0（S0 停转）；负值未观测，adapter 按 feed 同口径拒绝
/// （产品合同非负 U32/I32？见 `spindle_to_value`——spindle 取 I32，
/// 负值语义未闭合前 fail-closed，不猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpindleSpeed {
    /// 原始 8B。
    pub raw: [u8; 8],
    /// 定点尾数（i32 BE；S1~S4 实测 `500/1002/1500/800`；Mesa 取此值）。
    pub mantissa: i32,
    /// 缩放基（S0~S4 实测 `10`；真实为 u16，兼容视图截断）。
    pub base: u8,
    /// 缩放指数（S0~S4 实测 `0`；真实为 i16；非零 exp 自然观测，不造数据）。
    pub exponent: u8,
}

// ---------------------------------------------------------------------------
// Wire layout 常量（PR1 review + PR53 真实模型）：
// `RESPONSE_PREFIX_LEN/DATA_LEN_FIELD_LEN` 为旧成功路径视图（6×00 前缀），
// 新 decoder 走 `ReplySubpacket{status, data}` 真实模型；旧常量保留供
// 兼容单测引用（PR53 约束：已验证 bytes 不变），未使用告警允许。
// ---------------------------------------------------------------------------

/// GENERIC 响应成功前缀（旧视图；新代码走 `reply_success_data`）。
#[allow(dead_code)]
pub const RESPONSE_PREFIX_LEN: usize = 6;
/// GENERIC 响应 `data_len` 字段（旧视图；新代码走 `ReplySubpacket.data`）。
#[allow(dead_code)]
pub const DATA_LEN_FIELD_LEN: usize = 2;
/// SYSINFO 数据体（18B ODBSYS）。
pub const SYSINFO_DATA_LEN: usize = 18;
/// STATINFO 数据体（14B = 7×u16）。
pub const STATINFO_DATA_LEN: usize = 14;
/// FEED 数据体（8B scaled value）。
pub const FEED_DATA_LEN: usize = 8;
/// AXIS 数据体（8B scaled value，与 feed 同构但独立常量，不共享语义）。
pub const AXIS_DATA_LEN: usize = 8;
/// SPINDLE 数据体（8B scaled value；spindle Evidence PASS：
/// S0~S4 `data_len=8` 稳定，`RawNumeric8` 第三个独立 operation；
/// 与 feed/axis 同构但独立常量，不抽公共语义）。
pub const SPINDLE_DATA_LEN: usize = 8;
/// MACRO 数据体（8B scaled value；macro Evidence PASS：
/// M0~M3 `data_len=8` 稳定，`RawNumeric8` 第四个独立 operation；
/// 与 feed/axis/spindle 同构但独立常量，不抽公共语义）。
pub const MACRO_DATA_LEN: usize = 8;

// ---------------------------------------------------------------------------
// 路径/命令 id（PR53 真实模型）：`function u32 = path<<16|command`。
// 旧 `FUNC_*` 兼容视图保留（已验证 bytes 100% 不变）；新 decoder 用
// `(device, path, command)` 三元组 + slot 匹配。
// ---------------------------------------------------------------------------

/// CNC 设备（Gate 0：`0x0001`；PMC=`0x0002`，PR53 未用）。
pub(super) const DEV_CNC: u16 = 0x0001;
/// CNC 路径（165 实测 1；纯网络 API 把 path 放不可变请求，不模拟可变全局）。
/// 真实模型的三元组一维；frame 层 `PATH_CNC_DEFAULT` 同值（wire 侧用此名）。
pub(super) const PATH_CNC: u16 = 0x0001;
/// `system_info` 命令（Gate 0：`path 1 + 0x18`）。
pub(super) const CMD_SYSINFO: u16 = 0x0018;
/// `status_info` 命令（`0x19`；`0xe1/0x98` 为 hdck/tmmode，已识别未暴露）。
/// PR53：decoder 已用 slot 匹配；旧单测引用保留，未使用告警允许。
#[allow(dead_code)]
pub(super) const CMD_STATINFO: u16 = 0x0019;
/// statinfo 伴随：hdck（已识别未暴露；旧名 `FUNC_UNKNOWN_E1`）。
/// PR53：decoder 不再消费（framing 由 GENERIC 层保证），保留供文档/单测引用。
#[allow(dead_code)]
pub(super) const CMD_HDCK: u16 = 0x00e1;
/// statinfo 伴随：tmmode（已识别未暴露；旧名 `FUNC_UNKNOWN_98`）。
#[allow(dead_code)]
pub(super) const CMD_TMMODE: u16 = 0x0098;
/// `feed_rate` 命令（`0x24`，`0x24-only` 真机冻结）。
pub(super) const CMD_FEED: u16 = 0x0024;
/// `macro_value` 命令（`0x15`，macro Evidence PASS：
/// M0~M3 `device=1/path=1/args=[n,n,0,0]/aux=0`；单点语义，
/// `start=end=number`，不做范围读）。
pub(super) const CMD_MACRO: u16 = 0x0015;
/// `spindle_speed` 命令（`0x25`，spindle Evidence PASS：
/// S0~S4 `device=1/path=1/args=[0,0,0,0]/aux=0` 逐字节恒定，无 selector）。
pub(super) const CMD_SPINDLE_SPEED: u16 = 0x0025;
/// `axis_absolute` 命令（`0x26`，`v0=4/v1=ordinal` 真机冻结）。
pub(super) const CMD_AXIS_ABSOLUTE: u16 = 0x0026;
/// 兼容视图：`function = path<<16|command`（旧代码用，字节等价）。
/// （`FUNC_SYSINFO` 等保留供 fixture/request builder 兼容，见下。）
pub(super) const FUNC_SYSINFO: u32 = 0x0001_0018;
/// 兼容视图（同上）。
const FUNC_STATINFO: u32 = 0x0001_0019;
/// 兼容视图（同上；新代码用 `CMD_HDCK`）。
const FUNC_UNKNOWN_E1: u32 = 0x0001_00e1;
/// 兼容视图（同上；新代码用 `CMD_TMMODE`）。
const FUNC_UNKNOWN_98: u32 = 0x0001_0098;
/// 兼容视图（同上；新代码用 `(DEV_CNC, PATH_CNC, CMD_FEED)`）。
pub(super) const FUNC_FEED: u32 = 0x0001_0024;
/// 兼容视图（同上；新代码用 `(DEV_CNC, PATH_CNC, CMD_MACRO)`）。
pub(super) const FUNC_MACRO: u32 = 0x0001_0015;
/// 兼容视图（同上；新代码用 `(DEV_CNC, PATH_CNC, CMD_SPINDLE_SPEED)`）。
pub(super) const FUNC_SPINDLE_SPEED: u32 = 0x0001_0025;
/// 兼容视图（同上；新代码用 `(DEV_CNC, PATH_CNC, CMD_AXIS_ABSOLUTE)`）。
pub(super) const FUNC_AXIS_ABSOLUTE: u32 = 0x0001_0026;
/// axis `0x26` 请求首个参数实测恒 `4`（165 observed；语义未知，不命名业务含义）。
pub(super) const AXIS_ARG0_OBSERVED: i32 = 4;

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

    /// `macro_value(number)`（macro Evidence PASS：`0x15` count=1，
    /// `args=[number,number,0,0]/aux=0`；单点语义，不做范围读）。
    /// 宏号超 `c_short` 即 `Unsupported`，不发包（与 Native `Param` 同语义，
    /// 禁止 `as` 截断配 A 读 B）。
    /// 响应单 8B macro value。
    pub async fn macro_value(&self, number: u32) -> Result<MacroValue, WireError> {
        let n = crate::native::to_c_short(number)
            .map_err(|_| WireError::Unsupported("macro number out of c_short range"))?
            as i32;
        let req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_MACRO,
                [n, n, 0, 0, 0],
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
        match decode_macro_value(&resp) {
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
    /// count=1，`v0=4/v1=ordinal`；A0 `[2,3,1]` 原因未知，不复刻）。
    /// 响应单 8B position value。
    pub async fn axis_absolute(&self, axis: u8) -> Result<AxisPosition, WireError> {
        // `v1 = ordinal`（165 observed mapping；A0 异常原因未知，不复刻）。
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
                [AXIS_ARG0_OBSERVED, axis as i32, 0, 0, 0],
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

    /// `spindle_speed`（spindle Evidence PASS：`0x25-only` count=1，
    /// `device=1/path=1/args=[0,0,0,0]/aux=0`；S0~S4 逐字节恒定，无 selector、
    /// 无 preflight——`0x25` 单次 exchange，不复刻 axis 的 `0x18` 前置）。
    /// 响应单 8B speed value。
    pub async fn spindle_speed(&self) -> Result<SpindleSpeed, WireError> {
        let req = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_SPINDLE_SPEED,
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
        match decode_spindle_speed(&resp) {
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

/// `0x18` 响应解码：slot 匹配 `0x18`，成功数据 = 18B ODBSYS
/// （B1 实测）。字符区非可打印即 `Malformed`（绝不猜）。
pub(super) fn decode_system_info(resp: &FocasFrame) -> Result<SystemInfo, WireError> {
    let subs = decode_reply_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub =
        match_slot(&subs, DEV_CNC, PATH_CNC, CMD_SYSINFO, 0).ok_or(WireError::CommandMismatch)?;
    let d = reply_success_data(sub)?;
    if d.len() != SYSINFO_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
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

/// `0x19` 响应解码：slot 匹配 `0x19`（`device/path/command`），
/// 成功数据精确 14B(7×u16)。`status != 0` 即 `Remote`（由
/// `reply_success_data` 先行返回，不到长度检查）。
/// 缺 `0x19` 即 `CommandMismatch`。`0xe1/0x98`（hdck/tmmode，已识别未暴露）
/// 不在此消费（framing 由 GENERIC 层保证）。
pub(super) fn decode_status_info(resp: &FocasFrame) -> Result<StatusInfo, WireError> {
    let subs = decode_reply_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub =
        match_slot(&subs, DEV_CNC, PATH_CNC, CMD_STATINFO, 0).ok_or(WireError::CommandMismatch)?;
    let reply = reply_success_data(sub)?;
    if reply.len() != STATINFO_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    let d = &reply[..STATINFO_DATA_LEN];
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

/// 请求槽 ↔ 响应槽匹配（PR53）：`match_slot` 按 `(device, path, command)`
/// 取第 N 个（保留重复 command）。旧 `find_function`（取第一个）保留
/// 作兼容（单 subpacket 下等价；fixture/单测引用），未使用告警允许。
#[allow(dead_code)]
fn find_function(subs: &[GenericSubpacket], dev: u16, func: u32) -> Option<&GenericSubpacket> {
    subs.iter()
        .find(|s| s.control_device == dev && s.function == func)
}

/// 响应成功数据提取（PR53 Remote 模型）：`status == 0` 即返回 `data`；
/// `status != 0` 即 `WireError::Remote{status, detail1, detail2}`
/// （合法业务失败，session 保留，单点 BAD——旧“6×00 前缀”检查只覆盖
/// 成功路径，失败路径此前误判 `MalformedPayload` 丢连接，现修正）。
fn reply_success_data(sub: &ReplySubpacket) -> Result<&[u8], WireError> {
    if sub.status != 0 {
        return Err(WireError::Remote {
            status: sub.status,
            detail1: sub.detail1,
            detail2: sub.detail2,
        });
    }
    Ok(&sub.data)
}

/// `0x24` 响应解码：slot 匹配 `0x24`，成功数据精确 8B scaled value。
/// `RawNumeric8` 真实布局（`mantissa + base(u16) + exponent(i16)`）；
/// 旧 `FeedRate{base: u8, exponent: u8}` 在已验证值下等价，保留为兼容视图。
/// 缺 `0x24` 即 `CommandMismatch`。
pub(super) fn decode_feed_rate(resp: &FocasFrame) -> Result<FeedRate, WireError> {
    let subs = decode_reply_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub =
        match_slot(&subs, DEV_CNC, PATH_CNC, CMD_FEED, 0).ok_or(WireError::CommandMismatch)?;
    let d = reply_success_data(sub)?;
    // 精确闭合：成功数据必须恰好 8B（多 1B 即错）。
    if d.len() != FEED_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    let num = RawNumeric8::decode(d)?;
    Ok(FeedRate {
        raw: num.raw,
        mantissa: num.mantissa,
        base: num.base as u8,
        exponent: num.exponent as u8,
    })
}

/// `0x26` 响应解码：slot 匹配 `0x26`，成功数据精确 8B（与 feed 同构，
/// 独立 decoder，不抽公共类型）。缺 `0x26` 即 `CommandMismatch`。
pub(super) fn decode_axis_position(resp: &FocasFrame) -> Result<AxisPosition, WireError> {
    let subs = decode_reply_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub = match_slot(&subs, DEV_CNC, PATH_CNC, CMD_AXIS_ABSOLUTE, 0)
        .ok_or(WireError::CommandMismatch)?;
    let d = reply_success_data(sub)?;
    if d.len() != AXIS_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    let num = RawNumeric8::decode(d)?;
    Ok(AxisPosition {
        raw: num.raw,
        mantissa: num.mantissa,
        base: num.base as u8,
        exponent: num.exponent as u8,
    })
}

/// `0x25` 响应解码：slot 匹配 `0x25`，成功数据精确 8B（与 feed/axis 同构，
/// 独立 decoder，不抽公共类型；`RawNumeric8` 第三个独立 operation）。
/// 缺 `0x25` 即 `CommandMismatch`。
pub(super) fn decode_spindle_speed(resp: &FocasFrame) -> Result<SpindleSpeed, WireError> {
    let subs = decode_reply_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub = match_slot(&subs, DEV_CNC, PATH_CNC, CMD_SPINDLE_SPEED, 0)
        .ok_or(WireError::CommandMismatch)?;
    let d = reply_success_data(sub)?;
    if d.len() != SPINDLE_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    let num = RawNumeric8::decode(d)?;
    Ok(SpindleSpeed {
        raw: num.raw,
        mantissa: num.mantissa,
        base: num.base as u8,
        exponent: num.exponent as u8,
    })
}

/// `0x15` 响应解码：slot 匹配 `0x15`，成功数据精确 8B（与 feed/axis/spindle
/// 同构，独立 decoder，不抽公共类型；`RawNumeric8` 第四个独立 operation）。
/// 缺 `0x15` 即 `CommandMismatch`。
pub(super) fn decode_macro_value(resp: &FocasFrame) -> Result<MacroValue, WireError> {
    let subs = decode_reply_payload(&resp.payload).map_err(|_| WireError::MalformedPayload)?;
    let sub =
        match_slot(&subs, DEV_CNC, PATH_CNC, CMD_MACRO, 0).ok_or(WireError::CommandMismatch)?;
    let d = reply_success_data(sub)?;
    if d.len() != MACRO_DATA_LEN {
        return Err(WireError::MalformedPayload);
    }
    let num = RawNumeric8::decode(d)?;
    Ok(MacroValue {
        raw: num.raw,
        mantissa: num.mantissa,
        base: num.base as u8,
        exponent: num.exponent as u8,
    })
}

/// Mesa `macro/value` 映射（macro Evidence PASS + B4 真实权威）：
/// 以 `raw` 重建 `RawNumeric8` 为权威（`u16 base/i16 exponent`），不读
/// 兼容视图截断值。**与 feed/spindle/axis 的关键区别（证据结论）**：
/// Mesa 取 engineering/scaled F64（`mantissa / 10^exponent`），不是
/// native mantissa（M1 `250000000/10^7=25.0`、M3 `-750000000/10^8=-7.5`
/// 真机证据；`Value::F64(250000000.0)` 是错的）。
/// fail-closed（不猜 scale）：走 `engineering_value()` 证据门
/// （`base == 10` + exponent 可计算范围）；无法解释的 scale 即
/// `Unsupported`（ERR → BAD），绝不输出看似正常的 F64。
fn macro_to_value(m: &MacroValue) -> Result<Value, WireError> {
    let num = RawNumeric8::decode(&m.raw)?;
    let (numer, denom) = num.engineering_value()?;
    // `denom = 10^exp > 0`（门内保证）；F64 除法（Mesa 产品合同 F64）。
    Ok(Value::F64(numer as f64 / denom as f64))
}

/// Mesa `machine/spindle_speed` 映射（spindle Evidence PASS + PR53 B4）：
/// 以 `raw` 重建 `RawNumeric8` 为权威（`u16 base/i16 exponent`），不读
/// 兼容视图截断值。`native_value = mantissa` → `Value::I32`
/// （`cnc_acts` 只取 mantissa；S1~S4 `500/1002/1500/800` 全闭合）。
/// fail-closed：`mantissa < 0` 即 `Unsupported`（ERR → BAD；S0~S4 未见负值，
/// 语义未闭合前不猜）。`base/exponent` 不设门（Native parity 目标；
/// 非零 exp 自然观测，不造规则——与 feed B1 同口径）。
fn spindle_to_value(spd: &SpindleSpeed) -> Result<Value, WireError> {
    let num = RawNumeric8::decode(&spd.raw)?;
    if num.native_value() < 0 {
        return Err(WireError::Unsupported("spindle mantissa < 0"));
    }
    Ok(Value::I32(num.native_value()))
}

/// Mesa `machine/feed` 映射（PR53 两层语义 + B1 冻结 + B4 真实权威）：
/// `native_value = mantissa` → `Value::U32`（与 `cnc_actf` 只复制前 4B
/// 同合同；不因 `exponent != 0` 拒绝——否则 `mantissa=1234/exp=1` 将在
/// Native 返回 `1234` 时 Wire 拒绝，重新产生 parity 漂移）。
/// fail-closed（不 truncate/round/clamp）：`mantissa < 0` 即 `Unsupported`
/// （ERR → BAD；产品合同非负 U32）。B4：判定以 `raw` 重建的 `RawNumeric8`
/// 完整字段为权威，不读兼容视图截断值（`base 0x010A → u8 0x0A` 逃逸类）。
/// NOTE：工程量 `mantissa/base^exponent` 为独立语义，不进此 adapter；
/// 待独立 `engineering_value` 暴露后再议（见 PR53）。
fn feed_to_value(rate: &FeedRate) -> Result<Value, WireError> {
    // PR53 B1+B4：Native contract 只依赖 mantissa；权威来自 raw 重建。
    let num = RawNumeric8::decode(&rate.raw)?;
    if num.native_value() < 0 {
        return Err(WireError::Unsupported("feed mantissa < 0"));
    }
    Ok(Value::U32(num.native_value() as u32))
}

/// Mesa `axis.absolute` 映射（PR53 B4 真实权威）：以 `raw` 重建
/// `RawNumeric8` 完整字段为权威（`u16 base/i16 exponent`），不读兼容视图
/// 截断值（`exp 0x0103 → u8 3` 逃逸类）。`validate` 通过即
/// `Value::I32(native_value)`（mantissa 可负，-2880 等均有真机证据；
/// 与 feed 的 `mantissa<0` 拒绝无关，各自独立规则）。
fn axis_to_value(pos: &AxisPosition) -> Result<Value, WireError> {
    let num = RawNumeric8::decode(&pos.raw)?;
    num.validate()?;
    Ok(Value::I32(num.native_value()))
}

/// fixture/test 专用：生产 `axis_to_value` 同源入口（`#[cfg(test)]`，
/// 不出 crate；fixture 回归与 N4 fail-closed 断言用）。
#[cfg(test)]
pub(super) fn axis_to_value_for_test(pos: &AxisPosition) -> Result<Value, WireError> {
    axis_to_value(pos)
}

/// fixture/test 专用：生产 `spindle_to_value` 同源入口（`#[cfg(test)]`，
/// 不出 crate；spindle S0~S4 fixture 回归用）。
#[cfg(test)]
pub(super) fn spindle_to_value_for_test(spd: &SpindleSpeed) -> Result<Value, WireError> {
    spindle_to_value(spd)
}

/// fixture/test 专用：生产 `macro_to_value` 同源入口（`#[cfg(test)]`，
/// 不出 crate；macro M0~M3 fixture 回归用）。
#[cfg(test)]
pub(super) fn macro_to_value_for_test(m: &MacroValue) -> Result<Value, WireError> {
    macro_to_value(m)
}

/// fixture/test 专用：`Value::I32` 构造子（断言可读性用）。
#[cfg(test)]
pub(super) fn axis_value_for_test(v: i32) -> Value {
    Value::I32(v)
}

// ---------------------------------------------------------------------------
// WireFocasApi：Mesa adapter（PR1 最小 + PR2 feed + PR3 axis + PR54 spindle
// + PR55 macro）
// ---------------------------------------------------------------------------

/// Wire 版 `FocasApi`（PR55：`system_info` + `Status` + `Feed` +
/// `Axis/absolute` + `ActiveSpindleSpeed` + `MacroVar`；其余地址 `Unsupported`，
/// fail-closed，不猜、不 fallback Native）。
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

    /// 单点错误 ↔ 连接错误的分类：point-local（`Unsupported/Remote{..}`）
    /// 即 `ERR:` 占位（上层转单点 BAD）；session 致命即整批 `Err`（重连）。
    fn point_or_fatal(e: WireError) -> Result<Value, String> {
        match e {
            WireError::Unsupported(_) | WireError::Remote { .. } => {
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
        // 同一批共享请求：Status/Feed/ActiveSpindle 各一次；Axis 按轴号各一次
        // （`0x26` 单轴语义，无多轴数组）；Macro 按宏号各一次（`0x15` 单点，
        // 不做范围读）；其余 fail-closed。
        // client 内部按 operation 持 guard。
        // 致命 session 错误在预取阶段立即短路（point-local 才进批）。
        // 非 Absolute 的 Axis kind（Machine/Relative/…）与 Native 同口径
        // fail-closed（ERR → BAD），绝不用 absolute 冒充。
        // ActiveSpindleSpeed 与 indexed Spindle::Speed 同 PR52 Native 门：
        // 只有前者可调 `0x25`，后者 fail-closed（ERR → BAD）。
        use crate::address::AxisKind;
        use std::collections::BTreeMap;
        let need_status = addresses.iter().any(|a| matches!(a, FocasAddress::Status));
        let need_feed = addresses.iter().any(|a| matches!(a, FocasAddress::Feed));
        let need_spindle = addresses
            .iter()
            .any(|a| matches!(a, FocasAddress::ActiveSpindleSpeed));
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
        // ActiveSpindleSpeed：同批一次 0x25（S0~S4 恒定请求，无 selector）。
        let spindle_r: Option<Result<Value, String>> = if need_spindle {
            match self.client.spindle_speed().await {
                Ok(spd) => match spindle_to_value(&spd) {
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
        // Macro 按宏号各一次 0x15（单点语义；超 c_short 在 operation 内 fail-closed）。
        let mut macro_map: BTreeMap<u32, Result<Value, String>> = BTreeMap::new();
        for a in addresses {
            if let FocasAddress::MacroVar { number } = a
                && !macro_map.contains_key(number)
            {
                let r: Result<Value, String> = match self.client.macro_value(*number).await {
                    Ok(m) => match macro_to_value(&m) {
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
                macro_map.insert(*number, r);
            }
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
                FocasAddress::ActiveSpindleSpeed => match spindle_r.clone().unwrap() {
                    Ok(v) => out.push(v),
                    Err(e) if e.starts_with("ERR:") => out.push(Value::String(e)),
                    Err(fatal) => return Err(fatal),
                },
                FocasAddress::MacroVar { number } => {
                    match macro_map.get(number).cloned().unwrap() {
                        Ok(v) => out.push(v),
                        Err(e) if e.starts_with("ERR:") => out.push(Value::String(e)),
                        Err(fatal) => return Err(fatal),
                    }
                }
                _ => out.push(Value::String(format!(
                    "ERR:{}",
                    WireError::Unsupported("PR55 only Status/Feed/Absolute/ActiveSpindle/Macro")
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
    use super::super::frame::{
        GenericSubpacket, decode_generic_payload, encode_generic_request, request_subpacket,
    };
    use super::*;
    use crate::address::SpindleKind;

    /// PR54 产品边界：indexed `Spindle::Speed` 在 Wire 侧 fail-closed
    /// （只有 `ActiveSpindleSpeed` 可调 `0x25`，与 PR52 Native 门同口径）。
    /// 未连接 api 上只读 indexed speed：不得发包（`need_spindle=false`），
    /// 直接 `ERR:` 单点（若误纳入预取，未连接下走 `spindle_speed()` 即
    /// fatal `Closed` 整批 Err，测试必红）。
    #[tokio::test]
    async fn indexed_spindle_speed_stays_fail_closed() {
        let api = WireFocasApi::new(std::time::Duration::from_millis(50));
        let vals = api
            .read_batch(&[FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::Speed,
            }])
            .await
            .expect("indexed speed 必须单点 ERR，不整批 Err");
        assert_eq!(vals.len(), 1);
        assert!(
            matches!(&vals[0], Value::String(s) if s.starts_with("ERR:")),
            "indexed speed 必须 ERR 单点，实际：{:?}",
            vals[0]
        );
    }

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
    /// 无法证明流还在边界上；point-local 的业务失败走 `Remote{..}`）。
    /// 注意：`0xe1` 载荷 `10×00` 在真实模型下是 `status=0` 的合法成功包，
    /// 此测试用旧兼容 `GenericSubpacket` 构造，仅验证“无 0x19 槽”路径。
    #[test]
    fn missing_statinfo_is_mismatch() {
        use super::super::frame::{ReplySubpacket, decode_reply_payload};
        // 真实模型构造：count=1 + 0xe1 成功包（status=0），无 0x19。
        let mut payload = vec![0x00, 0x01];
        payload.extend_from_slice(&[0x00, 0x12]); // size=18
        payload.extend_from_slice(&[0x00, 0x01]); // device
        payload.extend_from_slice(&[0x00, 0x01]); // path
        payload.extend_from_slice(&[0x00, 0xe1]); // command
        payload.extend_from_slice(&[0x00, 0x00]); // status=0
        payload.extend_from_slice(&[0x00, 0x00]); // detail1
        payload.extend_from_slice(&[0x00, 0x00]); // detail2
        payload.extend_from_slice(&[0x00, 0x02]); // data_len=2
        payload.extend_from_slice(&[0x00, 0x00]); // data
        let subs = decode_reply_payload(&payload).unwrap();
        assert_eq!(subs.len(), 1);
        let _: &ReplySubpacket = &subs[0];
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload,
        };
        let e = decode_status_info(&frame).unwrap_err();
        assert!(
            matches!(e, WireError::CommandMismatch),
            "缺 0x19 必须 CommandMismatch"
        );
        assert!(e.is_session_fatal(), "CommandMismatch 保守判致命");
    }

    /// Remote 模型：`status != 0` 即业务失败（session 保留，单点 BAD）。
    /// 构造 `0x19` 槽 `status=2`（size=16+14=30=0x1e；旧 `0x18` 是 6×00
    /// 模型的 2+6+2+14=24，新模型为 2+6+6+2+14=30——尺寸本身即模型证据）。
    #[test]
    fn remote_status_is_point_local() {
        let mut payload = vec![0x00, 0x01];
        payload.extend_from_slice(&[0x00, 0x1e]); // size=30
        payload.extend_from_slice(&[0x00, 0x01]); // device
        payload.extend_from_slice(&[0x00, 0x01]); // path
        payload.extend_from_slice(&[0x00, 0x19]); // command
        payload.extend_from_slice(&[0x00, 0x02]); // status=2
        payload.extend_from_slice(&[0x00, 0x01]); // detail1
        payload.extend_from_slice(&[0x00, 0x02]); // detail2
        payload.extend_from_slice(&[0x00, 0x0e]); // data_len=14
        payload.extend_from_slice(&[0x00; 14]); // data（失败时不解释）
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload,
        };
        let e = decode_status_info(&frame).unwrap_err();
        match e {
            WireError::Remote {
                status,
                detail1,
                detail2,
            } => {
                assert_eq!(status, 2);
                assert_eq!(detail1, 1);
                assert_eq!(detail2, 2);
            }
            _ => panic!("status=2 必须 Remote，实际：{e:?}"),
        }
        assert!(!e.is_session_fatal(), "Remote 不杀 session");
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

    /// feed native 语义（PR53 B1 冻结）：`exp != 0` 不拒绝 adapter——
    /// Mesa 取 `mantissa` 直透（与 `cnc_actf` 只复制前 4B 同合同）。
    /// `mantissa=12345/exp=2` 即 `Value::U32(12345)`（工程量 123.45 为
    /// 独立语义，不进此 adapter）。
    #[test]
    fn feed_fractional_is_native_value() {
        let rate = FeedRate {
            raw: [0, 0, 0x30, 0x39, 0, 10, 0, 2],
            mantissa: 12345,
            base: 10,
            exponent: 2,
        };
        // RawNumeric8 层面可解（12345/100），adapter 取 native_value。
        assert_eq!(
            RawNumeric8::decode(&rate.raw)
                .unwrap()
                .engineering_value()
                .unwrap(),
            (12345, 100)
        );
        assert_eq!(
            RawNumeric8::decode(&rate.raw).unwrap().native_value(),
            12345
        );
        assert_eq!(feed_to_value(&rate).unwrap(), Value::U32(12345));
    }

    /// feed 负值 fail-closed：`mantissa < 0` 不得进 U32。
    /// B4：判定走 `raw` 重建（此处 raw 与兼容字段一致，双重锁定）。
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

    /// feed 非实证 base：B4 Native parity 下 adapter 不再设 base 门
    /// （Native contract 只依赖 mantissa）；`scaled()` 兼容视图仍拒绝
    /// （工程量语义未暴露，见过再放开）。
    /// B4 回归：`raw base=0x010A`（`as u8 → 0x0A=10` 截断）不得影响 adapter——
    /// adapter 以 raw 重建为权威，此处 mantissa=1 ≥ 0 即通过（Native 语义）。
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

    /// B4 截断逃逸回归（axis）：`raw exp=0x0103=259` 经 `as u8` 变成 `3`
    /// （兼容视图合法），但生产 adapter 必须以 raw 重建为权威 → Unsupported。
    #[test]
    fn axis_truncated_exponent_must_fail() {
        let raw = [0x00, 0x00, 0x00, 0x64, 0x00, 0x0A, 0x01, 0x03];
        let pos = AxisPosition {
            raw,
            mantissa: 100,
            base: 10,
            exponent: 3, // 截断值（兼容视图看起来合法）
        };
        // 真实值 259 不在 [0, 9]，生产 adapter 必须拒绝。
        assert_eq!(
            RawNumeric8::decode(&raw).unwrap().exponent,
            259,
            "raw 01 03 必须解为 i16 259"
        );
        assert!(matches!(
            axis_to_value(&pos).unwrap_err(),
            WireError::Unsupported(_)
        ));
    }

    /// B4 authority 回归（feed）：raw 与兼容视图故意矛盾——
    /// 生产 adapter 必须读 raw，不读 `rate.mantissa` 截断/旧字段。
    /// 若回退成 `rate.mantissa`，此测试必红（`-1` 进 U32 即错）。
    #[test]
    fn feed_adapter_uses_raw_authority() {
        let rate = FeedRate {
            raw: [0, 0, 0, 1, 0, 10, 0, 0], // raw mantissa = 1
            mantissa: -1,                   // compat 故意错误
            base: 10,
            exponent: 0,
        };
        assert_eq!(feed_to_value(&rate).unwrap(), Value::U32(1));
    }

    /// B4 authority 回归（axis 成功路径）：raw 与兼容视图故意矛盾——
    /// 生产 adapter 必须读 raw mantissa，不读 `pos.mantissa`。
    /// 与 `axis_truncated_exponent_must_fail`（锁 validation authority）
    /// 职责互补：此测试锁 value authority。
    #[test]
    fn axis_adapter_uses_raw_value_authority() {
        let pos = AxisPosition {
            raw: [0xff, 0xff, 0xff, 0xf6, 0, 10, 0, 3], // raw = -10
            mantissa: 123456,                           // compat 故意错误
            base: 10,
            exponent: 3,
        };
        assert_eq!(axis_to_value(&pos).unwrap(), Value::I32(-10));
    }

    /// spindle `0x25` 请求：count=1（Evidence S0~S4 冻结形态：
    /// `device=1/path=1/args=[0,0,0,0]/aux=0`，逐字节恒定，无 selector）。
    #[test]
    fn spindle_request_single_locked() {
        use super::super::frame::encode_generic_request as enc;
        let payload = enc(&[request_subpacket(
            DEV_CNC,
            FUNC_SPINDLE_SPEED,
            [0, 0, 0, 0, 0],
        )]);
        // count=1 + 28B = 30 = 0x1e（与 sysinfo/feed/axis 同长，function 换 0x25）。
        assert_eq!(payload.len(), 0x1e);
        assert_eq!(&payload[0..2], &[0x00, 0x01]);
        let back = decode_generic_payload(&payload).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].function, FUNC_SPINDLE_SPEED);
    }

    /// spindle S1 响应解码：`00 00 01 f4 00 0a 00 00` → mantissa=500。
    /// production encoder == captured S1 request（见 fixture `spindle1`），
    /// 此处锁 decoder + adapter（`Value::I32(500)`）。
    #[test]
    fn decode_spindle_s1_locked() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x00, 0x00, 0x01, 0xF4, 0x00, 0x0A, 0x00, 0x00]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_SPINDLE_SPEED,
                payload: p,
            }]),
        };
        // 2 + 24 = 26 = 0x1a（S1 捕获形态，与 feed/axis 同尺寸）。
        assert_eq!(frame.payload.len(), 0x1a);
        let spd = decode_spindle_speed(&frame).unwrap();
        assert_eq!(spd.mantissa, 500);
        assert_eq!(spd.base, 10);
        assert_eq!(spd.exponent, 0);
        assert_eq!(spd.raw, [0x00, 0x00, 0x01, 0xF4, 0x00, 0x0A, 0x00, 0x00]);
        // RawNumeric8 真实值：base=10/exp=0（第三个独立 operation）。
        assert_eq!(RawNumeric8::decode(&spd.raw).unwrap().base, 10);
        assert_eq!(RawNumeric8::decode(&spd.raw).unwrap().exponent, 0);
        // adapter：native_value → I32（与 Native cnc_acts=500 同合同）。
        assert_eq!(spindle_to_value(&spd).unwrap(), Value::I32(500));
    }

    /// spindle S0 停转：mantissa=0 → I32(0)（不是 BAD，占位符也不猜）。
    #[test]
    fn decode_spindle_s0_zero() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x0A, 0x00, 0x00]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_SPINDLE_SPEED,
                payload: p,
            }]),
        };
        let spd = decode_spindle_speed(&frame).unwrap();
        assert_eq!(spd.mantissa, 0);
        assert_eq!(spindle_to_value(&spd).unwrap(), Value::I32(0));
    }

    /// spindle 负值 fail-closed：`mantissa < 0` 不得进 I32（S0~S4 未见负值，
    /// 语义未闭合前不猜；与 feed 同口径，axis 的负值合法无关）。
    #[test]
    fn spindle_negative_is_bad() {
        let spd = SpindleSpeed {
            raw: [0xFF, 0xFF, 0xFF, 0xFF, 0, 10, 0, 0],
            mantissa: -1,
            base: 10,
            exponent: 0,
        };
        assert!(matches!(
            spindle_to_value(&spd).unwrap_err(),
            WireError::Unsupported(_)
        ));
    }

    /// B4 authority 回归（spindle 成功路径）：raw 与兼容视图故意矛盾——
    /// 生产 adapter 必须读 raw mantissa，不读 `spd.mantissa`。
    #[test]
    fn spindle_adapter_uses_raw_value_authority() {
        let spd = SpindleSpeed {
            raw: [0x00, 0x00, 0x01, 0xF4, 0, 10, 0, 0], // raw = 500
            mantissa: 999999,                           // compat 故意错误
            base: 10,
            exponent: 0,
        };
        assert_eq!(spindle_to_value(&spd).unwrap(), Value::I32(500));
    }

    /// spindle 缺 `0x25` 即 `CommandMismatch`（保守致命）。
    #[test]
    fn missing_spindle_is_mismatch() {
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_FEED, // 故意放错槽
                payload: {
                    let mut p = vec![0x00; 6];
                    p.extend_from_slice(&[0x00, 0x08]);
                    p.extend_from_slice(&[0x00, 0x00, 0x00, 0x64, 0x00, 0x0A, 0x00, 0x00]);
                    p
                },
            }]),
        };
        let e = decode_spindle_speed(&frame).unwrap_err();
        assert!(matches!(e, WireError::CommandMismatch));
        assert!(e.is_session_fatal());
    }

    /// spindle typed payload 内部精确闭合（与 feed/axis 同原则）。
    #[test]
    fn spindle_trailing_payload_rejected() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x00, 0x00, 0x01, 0xF4, 0x00, 0x0A, 0x00, 0x00]);
        p.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_SPINDLE_SPEED,
                payload: p,
            }]),
        };
        assert_eq!(
            decode_spindle_speed(&frame).unwrap_err().to_string(),
            WireError::MalformedPayload.to_string(),
        );
    }

    /// axis `0x26` 请求：`v0=4/v1=ordinal`（axis 证据 PASS 冻结形态）。
    #[test]
    fn axis_request_selector_locked() {
        let payload = encode_generic_request(&[request_subpacket(
            DEV_CNC,
            FUNC_AXIS_ABSOLUTE,
            [AXIS_ARG0_OBSERVED, 2, 0, 0, 0],
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

    /// axis N4：raw 指数 `30 33` → `i16 0x3033 = 12339`（真实布局），
    /// codec 照常解出字段，但 validate fail-closed（ERR → BAD，不进 I32）。
    /// 兼容视图 `exponent: u8` 截断为 `51`（`12339 & 0xFF`），仅为旧断言保留；
    /// 真实结论以 `RawNumeric8.exponent == 12339` 为准。
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
        // 真实布局：`30 33` → `i16 12339`（不是 `u8 51`）。
        assert_eq!(
            RawNumeric8::decode(&pos.raw).unwrap().exponent,
            12339,
            "raw 30 33 必须解为 i16 12339"
        );
        assert_eq!(pos.exponent, 51, "兼容视图截断保留（旧断言）");
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

    /// macro `0x15` 请求：count=1（Evidence M0~M3 冻结形态：
    /// `args=[number,number,0,0]/aux=0`，单点语义，不做范围读）。
    #[test]
    fn macro_request_single_locked() {
        use super::super::frame::encode_generic_request as enc;
        let payload = enc(&[request_subpacket(DEV_CNC, FUNC_MACRO, [501, 501, 0, 0, 0])]);
        // count=1 + 28B = 30 = 0x1e（与 sysinfo/feed/axis/spindle 同长）。
        assert_eq!(payload.len(), 0x1e);
        assert_eq!(&payload[0..2], &[0x00, 0x01]);
        let back = decode_generic_payload(&payload).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].function, FUNC_MACRO);
    }

    /// macro M1 响应解码：`0e e6 b2 80 00 0a 00 07` → mantissa=250000000。
    /// adapter 必须得 `Value::F64(25.0)`，绝不能得 `F64(250000000.0)`
    /// （防以后误复用 feed/spindle 的 `native_value()`）。
    #[test]
    fn decode_macro_m1_scaled() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x0E, 0xE6, 0xB2, 0x80, 0x00, 0x0A, 0x00, 0x07]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_MACRO,
                payload: p,
            }]),
        };
        // 2 + 24 = 26 = 0x1a（M1 捕获形态，与 feed/axis/spindle 同尺寸）。
        assert_eq!(frame.payload.len(), 0x1a);
        let m = decode_macro_value(&frame).unwrap();
        assert_eq!(m.mantissa, 250000000);
        assert_eq!(m.base, 10);
        assert_eq!(m.exponent, 7);
        assert_eq!(m.raw, [0x0E, 0xE6, 0xB2, 0x80, 0x00, 0x0A, 0x00, 0x07]);
        // RawNumeric8 真实值（第四个独立 operation）。
        assert_eq!(RawNumeric8::decode(&m.raw).unwrap().base, 10);
        assert_eq!(RawNumeric8::decode(&m.raw).unwrap().exponent, 7);
        // adapter：scaled F64（与 Native scaled=25.0 同合同）。
        assert_eq!(macro_to_value(&m).unwrap(), Value::F64(25.0));
    }

    /// macro M3 负值：`d3 4b e8 80 00 0a 00 08` → `Value::F64(-7.5)`（锁符号）。
    #[test]
    fn decode_macro_m3_negative() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0xD3, 0x4B, 0xE8, 0x80, 0x00, 0x0A, 0x00, 0x08]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_MACRO,
                payload: p,
            }]),
        };
        let m = decode_macro_value(&frame).unwrap();
        assert_eq!(m.mantissa, -750000000);
        assert_eq!(macro_to_value(&m).unwrap(), Value::F64(-7.5));
    }

    /// macro M0 零值：mantissa=0 → F64(0.0)（不是 BAD）。
    #[test]
    fn decode_macro_m0_zero() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x0A, 0x00, 0x00]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_MACRO,
                payload: p,
            }]),
        };
        let m = decode_macro_value(&frame).unwrap();
        assert_eq!(m.mantissa, 0);
        assert_eq!(macro_to_value(&m).unwrap(), Value::F64(0.0));
    }

    /// B4 authority 回归（macro 成功路径）：raw 与兼容视图故意矛盾——
    /// 生产 adapter 必须读 raw，不读 `m.mantissa`。
    #[test]
    fn macro_adapter_uses_raw_value_authority() {
        let m = MacroValue {
            raw: [0x0E, 0xE6, 0xB2, 0x80, 0, 10, 0, 7], // raw = 250000000/10^7
            mantissa: 1,                                // compat 故意错误
            base: 10,
            exponent: 7,
        };
        assert_eq!(macro_to_value(&m).unwrap(), Value::F64(25.0));
    }

    /// macro 缺 `0x15` 即 `CommandMismatch`（保守致命）。
    #[test]
    fn missing_macro_is_mismatch() {
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_FEED, // 故意放错槽
                payload: {
                    let mut p = vec![0x00; 6];
                    p.extend_from_slice(&[0x00, 0x08]);
                    p.extend_from_slice(&[0x00, 0x00, 0x00, 0x64, 0x00, 0x0A, 0x00, 0x00]);
                    p
                },
            }]),
        };
        let e = decode_macro_value(&frame).unwrap_err();
        assert!(matches!(e, WireError::CommandMismatch));
        assert!(e.is_session_fatal());
    }

    /// macro typed payload 内部精确闭合（与 feed/axis/spindle 同原则）。
    #[test]
    fn macro_trailing_payload_rejected() {
        let mut p = vec![0x00; 6];
        p.extend_from_slice(&[0x00, 0x08]);
        p.extend_from_slice(&[0x0E, 0xE6, 0xB2, 0x80, 0x00, 0x0A, 0x00, 0x07]);
        p.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let frame = FocasFrame {
            origin: 0x0003,
            packet_type: PacketType::GENERIC_RESPONSE,
            payload: encode_generic_request(&[GenericSubpacket {
                control_device: DEV_CNC,
                function: FUNC_MACRO,
                payload: p,
            }]),
        };
        assert_eq!(
            decode_macro_value(&frame).unwrap_err().to_string(),
            WireError::MalformedPayload.to_string(),
        );
    }

    /// macro number 越界 fail-closed：超 `c_short` 不发包（与 Native Param 同语义）。
    #[tokio::test]
    async fn macro_number_out_of_range() {
        let client = FocasClient::new(std::time::Duration::from_millis(10));
        let e = client.macro_value(40000).await.unwrap_err();
        assert!(
            matches!(e, WireError::Unsupported(_)),
            "macro=40000 必须 Unsupported（不发包）"
        );
    }
}
