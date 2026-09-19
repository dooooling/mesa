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

// ---------------------------------------------------------------------------
// Function id（Gate 0 实测 lead；response codec 以真机为准）
// ---------------------------------------------------------------------------

/// CNC 设备（Gate 0：`0x0001`；PMC=`0x0002`，PR1 未用）。
const DEV_CNC: u16 = 0x0001;
/// `system_info`（Gate 0：`00 01 00 18`）。
const FUNC_SYSINFO: u32 = 0x0001_0018;
/// `status_info`（Gate 0：`00 01 00 19`）。
const FUNC_STATINFO: u32 = 0x0001_0019;
/// statinfo 序列伴随 function（未知语义；只验 framing 后跳过）。
const FUNC_UNKNOWN_E1: u32 = 0x0001_00e1;
/// statinfo 序列伴随 function（未知语义；只验 framing 后跳过）。
const FUNC_UNKNOWN_98: u32 = 0x0001_0098;

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

// ---------------------------------------------------------------------------
// WireFocasApi：Mesa adapter（PR1 最小：connect/system_info/read_batch）
// ---------------------------------------------------------------------------

/// Wire 版 `FocasApi`（PR1 最小实现：`system_info` + `Status` 单地址；
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
        // PR1：同一批只锁一次 session（client 内部按 operation 持 guard），
        // 当前仅 Status 有 typed 实现；其余 fail-closed（ERR → 单点 BAD）。
        let mut out = Vec::with_capacity(addresses.len());
        // 检查是否全为 Status：是则一次 status_info 服务整批（共享请求）。
        let all_status = addresses.iter().all(|a| matches!(a, FocasAddress::Status));
        if all_status {
            match self.client.status_info().await {
                Ok(st) => {
                    for _ in addresses {
                        out.push(Value::U32(st.aut as u32));
                    }
                    return Ok(out);
                }
                Err(e) if matches!(e, WireError::Unsupported(_) | WireError::Remote(_)) => {
                    for _ in addresses {
                        out.push(Value::String(format!("ERR:{e}")));
                    }
                    return Ok(out);
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        // 混合批：Status 走共享，其余 Unsupported（PR2+ 逐个实现）。
        // `StatusInfo: Clone` 不派生（WireError 不可 Clone），缓存 String 化错误。
        let mut status_cache: Option<Result<u32, String>> = None;
        for addr in addresses {
            match addr {
                FocasAddress::Status => {
                    let r: Result<u32, String> = match status_cache.clone() {
                        Some(cached) => cached,
                        None => {
                            let fresh: Result<u32, String> = self
                                .client
                                .status_info()
                                .await
                                .map(|st| st.aut as u32)
                                .map_err(|e| match Self::point_or_fatal(e) {
                                    Ok(v) => match v {
                                        Value::String(s) => s,
                                        _ => "ERR:unreachable".to_string(),
                                    },
                                    Err(fatal) => fatal,
                                });
                            status_cache = Some(fresh.clone());
                            fresh
                        }
                    };
                    match r {
                        Ok(aut) => out.push(Value::U32(aut)),
                        Err(e) if e.starts_with("ERR:") => out.push(Value::String(e)),
                        Err(fatal) => return Err(fatal),
                    }
                }
                _ => out.push(Value::String(format!(
                    "ERR:{}",
                    WireError::Unsupported("PR1 only Status")
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
}
