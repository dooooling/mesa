//! FOCAS Ethernet Wire Frame（PR1 + PR53）：application frame 编解码。
//!
//! - 只解决字节 ↔ 帧的映射，不懂 TCP、不懂 Operation、不懂 Mesa Resource。
//! - Gate 0（165 真机）锁定的协议事实：magic / 00 02 OPEN / GENERIC
//!   `count + subpacket[]` / `10B header + payload_len` 精确切帧。
//! - PR53（逆向证据 `docs/focas-reimplementation/`）：10B header 实际为
//!   `spec(u16) + kind(u8) + direction(u8) + len(u16)`；GENERIC subpacket
//!   实际为 `device/path/command + args/aux/data`（请求）与
//!   `device/path/command + status/detail1/detail2 + data`（响应）。
//!   旧 `origin: u16` / `packet_type: u16` / `function: u32` / `5×i32`
//!   视图保留为兼容构造（已验证 bytes 100% 不变），新模型为真实协议模型。
//! - `PacketType` 保持开放 `u16`（程序传输等未来只加常量）。

use std::fmt;

// ---------------------------------------------------------------------------
// 常量：Gate 0 真机证据（165）
// ---------------------------------------------------------------------------

/// 每帧同步前缀（请求/响应相同）。
pub const SYNC_PREFIX: [u8; 4] = [0xA0, 0xA0, 0xA0, 0xA0];

/// 10B frame header 长度：magic(4) + origin(2) + type(2) + payload_len(2)。
pub const FRAME_HEADER_LEN: usize = 10;

/// 协议自然上限：`payload_len` 为 u16，`<= 65535`。MTU 1500 不是协议上限
/// （TCP 分段由 `read_exact` 处理，此处不设 `MAX_PACKET=1500` 之类约束）。
pub const MAX_PAYLOAD_LEN: usize = u16::MAX as usize;

// ---------------------------------------------------------------------------
// PacketType：开放 u16（不做封闭 enum）
// ---------------------------------------------------------------------------

/// FOCAS packet 类型。开放 `u16`：未来发现新类型只加常量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PacketType(pub u16);

impl PacketType {
    /// OPEN request packet type (`0x0101`).
    pub const OPEN_REQUEST: Self = Self(0x0101);
    /// OPEN response packet type (`0x0102`, Gate 0: 370B).
    pub const OPEN_RESPONSE: Self = Self(0x0102);
    /// CLOSE request packet type (`0x0201`, Gate 0: 10B empty payload).
    pub const CLOSE_REQUEST: Self = Self(0x0201);
    /// CLOSE response packet type (`0x0202`, Gate 0: 10B empty payload).
    pub const CLOSE_RESPONSE: Self = Self(0x0202);
    /// GENERIC request packet type (`0x2101`, carries subpacket layer).
    pub const GENERIC_REQUEST: Self = Self(0x2101);
    /// GENERIC response packet type (`0x2102`, carries subpacket layer).
    pub const GENERIC_RESPONSE: Self = Self(0x2102);

    /// 是否 GENERIC 族（`0x2101/0x2102` 才有 subpacket 层）。
    /// PR2+ 的 operation 分发用；PR1 的 session 按显式 expected type 校验，
    /// 此 helper 保留供未来分发（未使用告警允许）。
    #[allow(dead_code)]
    pub fn is_generic(self) -> bool {
        self == Self::GENERIC_REQUEST || self == Self::GENERIC_RESPONSE
    }
}

impl fmt::Display for PacketType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:04x}", self.0)
    }
}

// ---------------------------------------------------------------------------
// FocasFrame：基础帧（magic/origin/type/payload）
// ---------------------------------------------------------------------------

/// FOCAS 基础帧。不放 `FocasAddress/Value/Resource`（协议层不知道 Mesa）。
/// `origin` 为 `spec` 兼容别名（历史代码用 `origin` 读写，字节相同）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocasFrame {
    /// 规格字（请求写 1；响应透传，不校验具体值）。
    pub origin: u16,
    /// 开放 packet 类型。
    pub packet_type: PacketType,
    /// type 之后 `payload_len` 字节（OPEN/CLOSE 即全部；GENERIC 内再解）。
    pub payload: Vec<u8>,
}

impl FocasFrame {
    /// 编码完整帧：`magic + origin + type + len(payload) + payload`（全 BE）。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FRAME_HEADER_LEN + self.payload.len());
        out.extend_from_slice(&SYNC_PREFIX);
        out.extend_from_slice(&self.origin.to_be_bytes());
        out.extend_from_slice(&self.packet_type.0.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// OPEN request payload 00 02 (verified GENERIC-capable variant).
    ///
    /// Evidence (165 / 0i-F): Gate 0 capture saw FWLIB emit both 00 01
    /// and 00 02; PR1 single-stream experiment proved 00 01 connects
    /// but RSTs on GENERIC while 00 02 serves SYSINFO/STATINFO.
    /// Purpose of 00 01 stays unknown (no control/monitor naming).
    /// OPEN has no subpacket layer.
    pub fn open_request() -> Self {
        Self {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::OPEN_REQUEST,
            payload: OPEN_GENERIC_VARIANT.to_be_bytes().to_vec(),
        }
    }

    /// CLOSE 请求帧（Gate 0：10B，payload 为空）。
    pub fn close_request() -> Self {
        Self {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::CLOSE_REQUEST,
            payload: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 10B header 真实模型（PR53）：spec + kind + direction + len。
// 旧 `origin(u16)/packet_type(u16)` 视图保留（字节等价），新字段为真实命名。
// ---------------------------------------------------------------------------

/// 10B header 解码结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// 规格字（请求构造写 1；165 响应实测 3；决定 OPEN 布局/接收预算）。
    /// 旧名 `origin`（C→S `0x0001`、S→C `0x0003` 只是本机观测值）。
    pub spec: u16,
    /// 兼容旧视图：`spec` 的别名（历史代码用 `origin`，字节相同）。
    pub origin: u16,
    /// kind（`01` OPEN、`02` CLOSE、`21` GENERIC 等；u8）。
    pub kind: u8,
    /// direction（请求 1、成功响应 2；DLL 还处理 3、4）。
    pub direction: u8,
    /// 开放 packet 类型（`kind<<8|direction` 兼容视图，如 `0x2101`）。
    pub packet_type: PacketType,
    /// 其后 payload 字节数（`<= 65535`，超限即 `BadLength`）。
    pub payload_len: usize,
}

/// 解 10B header：验 magic + 取 spec/kind/direction/payload_len（全 BE）。
/// payload 本体由调用方 `read_exact(payload_len)` 读取（防 TCP 分片/粘包）。
pub fn decode_header(raw: &[u8; FRAME_HEADER_LEN]) -> Result<FrameHeader, FrameError> {
    if raw[0..4] != SYNC_PREFIX {
        return Err(FrameError::BadMagic);
    }
    let spec = u16::from_be_bytes([raw[4], raw[5]]);
    let kind = raw[6];
    let direction = raw[7];
    let packet_type = PacketType(u16::from_be_bytes([raw[6], raw[7]]));
    let payload_len = u16::from_be_bytes([raw[8], raw[9]]) as usize;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(FrameError::BadLength);
    }
    Ok(FrameHeader {
        spec,
        origin: spec,
        kind,
        direction,
        packet_type,
        payload_len,
    })
}

/// 由 header + 已读 payload 组帧（`payload.len() == header.payload_len` 才接受）。
pub fn assemble(header: FrameHeader, payload: Vec<u8>) -> Result<FocasFrame, FrameError> {
    if payload.len() != header.payload_len {
        return Err(FrameError::BadLength);
    }
    Ok(FocasFrame {
        origin: header.origin,
        packet_type: header.packet_type,
        payload,
    })
}

// ---------------------------------------------------------------------------
// Wire layout 常量：反复出现的 byte-layout offset/length 命名，
// 避免阅读者在脑子里记 `6/2/18/14` 各自含义（PR1 review 要求）。
// 不引入 BinaryReader/CodecBuilder（过度设计，PR1 不做）。
// ---------------------------------------------------------------------------

/// 请求 origin（C→S；响应 spec 透传，见 Gate 0）。
pub const REQUEST_ORIGIN: u16 = 0x0001;
/// CNC 默认路径（165 实测 1；纯网络 API 把 path 放不可变请求）。
/// wire.rs 用 `PATH_CNC`（同值）；保留此名供 frame 层单测直引。
#[allow(dead_code)]
pub const PATH_CNC_DEFAULT: u16 = 0x0001;
/// 经直接 wire parity 验证的 GENERIC-capable OPEN variant（165 / 0i-F）。
/// `0x0001` 建连成功但随后 GENERIC 必 RST，用途未知，不命名。
pub const OPEN_GENERIC_VARIANT: u16 = 0x0002;
/// GENERIC 响应 subpacket 前导：`control_device(2) + function(4)`。
pub const SUBPACKET_HEADER_LEN: usize = 6;
/// subpacket length 字段长度（含自身 2B）。
pub const SUBPACKET_LEN_FIELD_LEN: usize = 2;
/// 单个请求 subpacket 参数个数（Gate 0 request 形态：`5×i32 BE`）。
pub const REQUEST_ARG_COUNT: usize = 5;
/// 请求参数字节数（`5×4`）。
pub const REQUEST_ARGS_LEN: usize = REQUEST_ARG_COUNT * 4;

// ---------------------------------------------------------------------------
// GENERIC subpacket 真实模型（PR53）：device/path/command + args/data（请求），
// device/path/command + status/details + data（响应）。
// 旧 `GenericSubpacket{control_device,function,payload}` 保留为兼容视图
// （`function = path<<16|command`，`payload = args + data`；已验证 bytes
// 100% 不变），新类型为真实协议模型。
// ---------------------------------------------------------------------------

/// GENERIC 请求 subpacket 真实布局（28B + data）：
/// `size(2) + device(2) + path(2) + command(2) + args[4](4×u32 BE) +
/// aux(2) + data_len(2) + data`。Gate 0 请求（path=1/无 data/aux=0）下，
/// 旧 `5×i32` 视图字节等价（第 5 个 i32 = aux+data_len 全零）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSubpacket {
    /// 设备（CNC=1、PMC=2）。
    pub device: u16,
    /// 路径（CNC/PMC 各自当前路径；165 实测 1）。
    pub path: u16,
    /// 命令（如 sysinfo `0x18`、statinfo `0x19`、feed `0x24`）。
    pub command: u16,
    /// 4 个 u32 参数槽（BE；axis selector 等在此；无参即全零）。
    pub args: [u32; 4],
    /// 命令相关辅助参数（不擅自命名为 flags；Gate 0 请求全零）。
    pub aux: u16,
    /// 附加数据（Gate 0 请求为空；PMC/程序传输等在此带数据）。
    pub data: Vec<u8>,
}

impl RequestSubpacket {
    /// 编码（含 2B size 前缀；`size = 28 + data_len`）。
    /// PR53：请求 builder 仍走旧 `request_subpacket`（已验证 bytes 不变）；
    /// 此真实模型编码由单测锁定等价，Dynamic2/PMC(data) 时启用。
    #[allow(dead_code)]
    pub fn encode(&self) -> Vec<u8> {
        let size = 28 + self.data.len();
        let mut out = Vec::with_capacity(2 + size - 2 + 2);
        out.extend_from_slice(&(size as u16).to_be_bytes());
        out.extend_from_slice(&self.device.to_be_bytes());
        out.extend_from_slice(&self.path.to_be_bytes());
        out.extend_from_slice(&self.command.to_be_bytes());
        for a in self.args {
            out.extend_from_slice(&a.to_be_bytes());
        }
        out.extend_from_slice(&self.aux.to_be_bytes());
        out.extend_from_slice(&(self.data.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// 兼容视图：旧 `GenericSubpacket{control_device,function,payload}`。
    /// Gate 0 请求下与旧 `5×i32` 字节完全等价（单测锁定）。
    #[allow(dead_code)]
    pub fn as_legacy(&self) -> GenericSubpacket {
        let mut payload = Vec::with_capacity(20 + self.data.len());
        for a in self.args {
            payload.extend_from_slice(&a.to_be_bytes());
        }
        payload.extend_from_slice(&self.aux.to_be_bytes());
        payload.extend_from_slice(&(self.data.len() as u16).to_be_bytes());
        payload.extend_from_slice(&self.data);
        GenericSubpacket {
            control_device: self.device,
            function: ((self.path as u32) << 16) | (self.command as u32),
            payload,
        }
    }
}

/// GENERIC 响应 subpacket 真实布局（16B + data）：
/// `size(2) + device(2) + path(2) + command(2) + status(i16) +
/// detail1(i16) + detail2(i16) + data_len(2) + data`。
/// `status != 0` 为合法远端业务结果（`WireError::Remote`），不是坏包；
/// 旧“6×00 前缀”检查只适用于成功路径，失败路径走 Remote（PR53 必修）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplySubpacket {
    /// 设备（请求回显）。
    pub device: u16,
    /// 路径（请求回显）。
    pub path: u16,
    /// 命令（请求回显）。
    pub command: u16,
    /// 状态（i16 返回码；0 = 成功，非零 = 远端业务错误）。
    pub status: i16,
    /// 细节 1（保留原始位型，不解释）。
    pub detail1: i16,
    /// 细节 2（保留原始位型，不解释）。
    pub detail2: i16,
    /// 数据体（成功为数据；失败为错误上下文，不猜）。
    pub data: Vec<u8>,
}

impl ReplySubpacket {
    /// 成功时数据体引用（`status == 0` 才调；失败调即逻辑错）。
    /// PR53：decoder 走 `reply_success_data`（含 Remote 转换）；此 helper
    /// 保留供单测直断，未使用告警允许。
    #[allow(dead_code)]
    pub fn success_data(&self) -> Option<&[u8]> {
        if self.status == 0 {
            Some(&self.data)
        } else {
            None
        }
    }
}

/// GENERIC 响应解码（真实模型）：`count + ReplySubpacket[]`。
/// 每包 `size == 16 + data_len` 精确闭合；count 包耗尽 payload；
/// `device/path/command` 透传（slot 匹配由调用方做，见 `match_slot`）。
pub fn decode_reply_payload(payload: &[u8]) -> Result<Vec<ReplySubpacket>, FrameError> {
    if payload.len() < 2 {
        return Err(FrameError::Malformed);
    }
    let count = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    if count == 0 {
        return Err(FrameError::Malformed);
    }
    let mut out = Vec::with_capacity(count);
    let mut off = 2;
    for _ in 0..count {
        if off + 2 > payload.len() {
            return Err(FrameError::Malformed);
        }
        let size = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
        if size < 16 || off + size > payload.len() {
            return Err(FrameError::Malformed);
        }
        let body = &payload[off + 2..off + size];
        let data_len = u16::from_be_bytes([body[12], body[13]]) as usize;
        if 16 + data_len != size {
            return Err(FrameError::Malformed);
        }
        out.push(ReplySubpacket {
            device: u16::from_be_bytes([body[0], body[1]]),
            path: u16::from_be_bytes([body[2], body[3]]),
            command: u16::from_be_bytes([body[4], body[5]]),
            status: i16::from_be_bytes([body[6], body[7]]),
            detail1: i16::from_be_bytes([body[8], body[9]]),
            detail2: i16::from_be_bytes([body[10], body[11]]),
            data: body[14..14 + data_len].to_vec(),
        });
        off += size;
    }
    if out.len() != count || off != payload.len() {
        return Err(FrameError::Malformed);
    }
    Ok(out)
}

/// 请求槽 ↔ 响应槽匹配（PR53 必修）：按 `(device, path, command)` 找第 N 个
/// 匹配响应（保留重复 command，如 Dynamic2 的 `0x26×4`），而不是
/// `find_function` 取第一个。`slot` 为请求中同 key 的第几个（0 起）。
/// 不匹配即 `None`（调用方转 `CommandMismatch`，保守致命）。
pub fn match_slot(
    resps: &[ReplySubpacket],
    device: u16,
    path: u16,
    command: u16,
    slot: usize,
) -> Option<&ReplySubpacket> {
    resps
        .iter()
        .filter(|r| r.device == device && r.path == path && r.command == command)
        .nth(slot)
}

/// OPEN 响应校验（PR53）：`spec/count → 16+8×count / 40+32×count`。
/// 偏移以完整帧开头为零（逆向证据）：`frame+18`（`<=2`）/ `frame+20`
/// （`>=3`，等价 `payload+10`）。已观测 `count=10`（Gate 0 捕获）与
/// `count=7`（当前 live）；变化原因尚未建立，只验“用通告 count + 长度公式”
/// 闭合，不断言固定 count 值。未知布局（spec 不在已实现版本）即 `Malformed`。
/// 只校验长度闭合，不解释能力表字段（只命名已由使用点证明的字段）。
pub fn validate_open_response(payload: &[u8], spec: u16) -> Result<(), FrameError> {
    if payload.len() < 12 {
        return Err(FrameError::Malformed);
    }
    let (count, expected) = if spec <= 2 {
        let c = u16::from_be_bytes([payload[8], payload[9]]) as usize;
        (c, 16usize.checked_add(8usize.saturating_mul(c)))
    } else if spec <= 4 {
        // spec=3（165）/ 4：count 在 payload+10（frame+20）。
        let c = u16::from_be_bytes([payload[10], payload[11]]) as usize;
        (c, 40usize.checked_add(32usize.saturating_mul(c)))
    } else {
        return Err(FrameError::Malformed);
    };
    let _ = count;
    match expected {
        Some(n) if n == payload.len() => Ok(()),
        _ => Err(FrameError::Malformed),
    }
}

/// 兼容视图（旧模型保留；已验证 bytes 100% 不变）：
///
/// GENERIC subpacket：`u16 BE length（含自身 2B）+ body`。
/// `body` 前 6B 为 `control_device(2) + function(4)`（Gate 0：
/// CNC=`0x0001`；`sysinfo=0x00010018`、`statinfo=0x00010019`），
/// 其后为 operation payload（变长，不假设 `5×i32`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericSubpacket {
    /// 控制设备（Gate 0 实测 CNC=`0x0001`；PMC=`0x0002`，PR1 未用）。
    pub control_device: u16,
    /// 功能号（4B BE，如 `00 01 00 18`；透传，不解释语义）。
    pub function: u32,
    /// function 之后全部字节（变长；成功为数据，失败为 `i16` 错误码起手）。
    pub payload: Vec<u8>,
}

impl GenericSubpacket {
    /// 编码单个 subpacket（含 2B length 前缀）。
    pub fn encode(&self) -> Vec<u8> {
        let body_len = SUBPACKET_HEADER_LEN + self.payload.len();
        let mut out = Vec::with_capacity(SUBPACKET_LEN_FIELD_LEN + body_len);
        out.extend_from_slice(&((body_len + SUBPACKET_LEN_FIELD_LEN) as u16).to_be_bytes());
        out.extend_from_slice(&self.control_device.to_be_bytes());
        out.extend_from_slice(&self.function.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// GENERIC 请求编码：`count + subpacket[]`（Gate 0：sysinfo `count=1`；
/// statinfo 序列 frame#2 `count=3`；多 subpacket 从第一天支持）。
pub fn encode_generic_request(subpackets: &[GenericSubpacket]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(subpackets.len() as u16).to_be_bytes());
    for s in subpackets {
        payload.extend_from_slice(&s.encode());
    }
    payload
}

/// 单个请求 subpacket 构造：`control_device + function + 5×i32 BE args`
/// （Gate 0 sysinfo/statinfo 的 request 形态；response 侧不假设此布局）。
pub fn request_subpacket(
    control_device: u16,
    function: u32,
    args: [i32; REQUEST_ARG_COUNT],
) -> GenericSubpacket {
    let mut payload = Vec::with_capacity(REQUEST_ARGS_LEN);
    for a in args {
        payload.extend_from_slice(&a.to_be_bytes());
    }
    GenericSubpacket {
        control_device,
        function,
        payload,
    }
}

/// GENERIC payload 解码（旧兼容视图；新代码走 `decode_reply_payload`）。
/// 保留供兼容单测引用（PR53 约束：已验证 bytes 不变），未使用告警允许。
#[allow(dead_code)]
pub fn decode_generic_payload(payload: &[u8]) -> Result<Vec<GenericSubpacket>, FrameError> {
    if payload.len() < 2 {
        return Err(FrameError::Malformed);
    }
    let count = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    let mut out = Vec::with_capacity(count);
    let mut off = 2;
    for _ in 0..count {
        if off + SUBPACKET_LEN_FIELD_LEN > payload.len() {
            return Err(FrameError::Malformed);
        }
        let len = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
        if len < SUBPACKET_LEN_FIELD_LEN + SUBPACKET_HEADER_LEN || off + len > payload.len() {
            return Err(FrameError::Malformed);
        }
        let body = &payload[off + SUBPACKET_LEN_FIELD_LEN..off + len];
        out.push(GenericSubpacket {
            control_device: u16::from_be_bytes([body[0], body[1]]),
            function: u32::from_be_bytes([body[2], body[3], body[4], body[5]]),
            payload: body[SUBPACKET_HEADER_LEN..].to_vec(),
        });
        off += len;
    }
    // count 个 subpacket 必须恰好耗尽 payload（frame length → count →
    // subpacket length 精确闭合；trailing bytes 说明边界已错位）。
    if out.len() != count || off != payload.len() {
        return Err(FrameError::Malformed);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// FrameError：纯帧层错误（不含 IO，IO 由 session 层报）
// ---------------------------------------------------------------------------

/// 帧层解码错误。`BadMagic/BadLength/Malformed` 意味着 TCP 字节流已不可信，
/// 调用方（session）必须使 session 失效（`session = None`），由上层重连。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// magic 非 `a0 a0 a0 a0`。
    BadMagic,
    /// `payload_len` 超 `u16` 上限或与实收不符。
    BadLength,
    /// GENERIC count/length 自描述不自洽。
    Malformed,
    /// 期望 packet 类型不符（如 OPEN 后非 `0x0102`）。
    /// PR1 的 session 层把它映射为 `WireError::UnexpectedPacket`；
    /// 此变体在 frame 层保留以备 loopback/单测直接构造。
    #[allow(dead_code)]
    UnexpectedPacket {
        /// 期望值。
        expected: PacketType,
        /// 实际值。
        got: PacketType,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "FOCAS bad magic"),
            Self::BadLength => write!(f, "FOCAS bad length"),
            Self::Malformed => write!(f, "FOCAS malformed generic payload"),
            Self::UnexpectedPacket { expected, got } => {
                write!(
                    f,
                    "FOCAS unexpected packet (expected {expected}, got {got})"
                )
            }
        }
    }
}

impl std::error::Error for FrameError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gate 0（修正后）：OPEN request 逐字节回归
    /// （`00 02` payload；单流复现证实 `00 01` 后 GENERIC 必 RST）。
    #[test]
    fn open_request_bytes_locked() {
        let raw = FocasFrame::open_request().encode();
        assert_eq!(
            raw,
            vec![
                0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x01, 0x01, 0x01, 0x00, 0x02, 0x00, 0x02
            ],
            "OPEN request 必须 12B 且逐字节一致（Gate 0 修正：payload 00 02）"
        );
        let (head, tail) = raw.split_at(FRAME_HEADER_LEN);
        let mut h = [0u8; FRAME_HEADER_LEN];
        h.copy_from_slice(head);
        let hdr = decode_header(&h).unwrap();
        assert_eq!(hdr.origin, 0x0001);
        assert_eq!(hdr.packet_type, PacketType::OPEN_REQUEST);
        assert_eq!(hdr.payload_len, 2);
        assert_eq!(tail, &[0x00, 0x02]);
    }

    /// Gate 0：CLOSE request 10B（payload 为空，无 subpacket 层）。
    #[test]
    fn close_request_bytes_locked() {
        let raw = FocasFrame::close_request().encode();
        assert_eq!(
            raw,
            vec![0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x01, 0x02, 0x01, 0x00, 0x00],
            "CLOSE request 必须 10B（Gate 0 165 实测）"
        );
    }

    /// Gate 0：SYSINFO request 40B 逐字节回归
    /// （`00 1e/count=1/00 1c/CNC/0x00010018/20×00`）。
    #[test]
    fn sysinfo_request_bytes_locked() {
        let sub = request_subpacket(0x0001, 0x0001_0018, [0, 0, 0, 0, 0]);
        let frame = FocasFrame {
            origin: 0x0001,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[sub]),
        };
        let raw = frame.encode();
        let mut expected = vec![
            0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x01, 0x21, 0x01, 0x00, 0x1e, 0x00, 0x01, 0x00, 0x1c,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x18,
        ];
        expected.extend_from_slice(&[0x00; 20]);
        assert_eq!(raw, expected, "SYSINFO request 必须 40B 且逐字节一致");
        assert_eq!(raw.len(), 10 + 0x1e);
    }

    /// header 解码：坏 magic 即错（session 必须失效，不猜）。
    #[test]
    fn bad_magic_rejected() {
        let bad = [0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x00, 0x02];
        assert_eq!(decode_header(&bad), Err(FrameError::BadMagic));
    }

    /// GENERIC 解码：多 subpacket + 变长 payload（statinfo frame#2 形态）。
    #[test]
    fn generic_multi_subpacket_roundtrip() {
        let subs = vec![
            request_subpacket(0x0001, 0x0001_0019, [0, 0, 0, 0, 0]),
            request_subpacket(0x0001, 0x0001_00e1, [0, 0, 0, 0, 0]),
            request_subpacket(0x0001, 0x0001_0098, [0, 0, 0, 0, 0]),
        ];
        let payload = encode_generic_request(&subs);
        // count=3 + 3×28 = 86 = 0x56（Gate 0 B1 实测 frame#2）。
        assert_eq!(payload.len(), 0x56);
        assert_eq!(&payload[0..2], &[0x00, 0x03]);
        let back = decode_generic_payload(&payload).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].function, 0x0001_0019);
        assert_eq!(back[1].function, 0x0001_00e1);
        assert_eq!(back[2].function, 0x0001_0098);
    }

    /// GENERIC 解码：count 与实际不符即错（截断/粘包残留不得放行）。
    #[test]
    fn generic_truncated_rejected() {
        let subs = vec![request_subpacket(0x0001, 0x0001_0018, [0, 0, 0, 0, 0])];
        let mut payload = encode_generic_request(&subs);
        payload.truncate(payload.len() - 5);
        assert_eq!(decode_generic_payload(&payload), Err(FrameError::Malformed));
        // count 谎报 2、实际 1
        let mut lied = encode_generic_request(&subs);
        lied[1] = 0x02;
        assert_eq!(decode_generic_payload(&lied), Err(FrameError::Malformed));
    }

    /// GENERIC 解码：trailing garbage 即错（frame length → count →
    /// subpacket length 必须精确闭合；多余字节说明边界错位）。
    #[test]
    fn generic_trailing_bytes_rejected() {
        let subs = vec![request_subpacket(0x0001, 0x0001_0018, [0, 0, 0, 0, 0])];
        let mut payload = encode_generic_request(&subs);
        payload.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(
            decode_generic_payload(&payload),
            Err(FrameError::Malformed),
            "count=1 的合法 subpacket 后跟 4B 垃圾必须拒绝"
        );
    }

    /// `PacketType` 开放：未知类型可表示、可比较，不重构 parser。
    #[test]
    fn packet_type_open() {
        let future = PacketType(0x1501);
        assert!(!future.is_generic());
        assert_ne!(future, PacketType::GENERIC_REQUEST);
        assert_eq!(format!("{future}"), "0x1501");
    }

    /// OPEN 响应布局校验（PR53）：`spec/count → 长度公式`。
    /// count 在 payload+10（frame+20）；165 `spec=3/count=10 → 360`。
    #[test]
    fn open_response_layout_validated() {
        // 构造 360B 合法体：payload[10:12] = count=10。
        let mut payload = vec![0x00; 360];
        payload[10] = 0x00;
        payload[11] = 0x0a;
        super::validate_open_response(&payload, 3).unwrap();
        // 未知 spec 即 Malformed。
        assert_eq!(
            super::validate_open_response(&payload, 5),
            Err(FrameError::Malformed)
        );
        // 长度不闭合即 Malformed（count=10 但体只有 102B）。
        let mut short = vec![0x00; 102];
        short[10] = 0x00;
        short[11] = 0x0a;
        assert_eq!(
            super::validate_open_response(&short, 3),
            Err(FrameError::Malformed)
        );
    }

    /// 请求真实模型与旧 `5×i32` 字节等价（Gate 0 请求 path=1/aux=0/无 data）。
    #[test]
    fn request_subpacket_model_equivalent() {
        use super::{RequestSubpacket, request_subpacket};
        let legacy = request_subpacket(0x0001, 0x0001_0024, [0, 0, 0, 0, 0]);
        let real = RequestSubpacket {
            device: 0x0001,
            path: 0x0001,
            command: 0x0024,
            args: [0, 0, 0, 0],
            aux: 0,
            data: vec![],
        };
        assert_eq!(real.as_legacy(), legacy, "真实模型必须字节等价旧模型");
        assert_eq!(real.encode().len(), 28, "size 字段 = 28（含自身 2B）");
    }

    /// 响应真实模型：`status != 0` 透传 framing（旧“6×00 前缀”只覆盖成功）。
    #[test]
    fn reply_status_transparent() {
        use super::decode_reply_payload;
        // count=1 + 0x24 成功包（status=0/data 8B）。
        let mut ok = vec![0x00, 0x01, 0x00, 0x18];
        ok.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x24]);
        ok.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        ok.extend_from_slice(&[0x00, 0x08, 0x00, 0x00, 0x00, 0x64, 0x00, 0x0a, 0x00, 0x00]);
        let subs = decode_reply_payload(&ok).unwrap();
        assert_eq!(subs[0].status, 0);
        assert_eq!(subs[0].data.len(), 8);
        // count=1 + 0x24 失败包（status=2/data 4B）：framing 照常通过。
        let mut er = vec![0x00, 0x01, 0x00, 0x14];
        er.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x24]);
        er.extend_from_slice(&[0x00, 0x02, 0x00, 0x00, 0x00, 0x00]);
        er.extend_from_slice(&[0x00, 0x04, 0x00, 0x00, 0x00, 0x00]);
        let subs = decode_reply_payload(&er).unwrap();
        assert_eq!(subs[0].status, 2);
    }

    /// slot 匹配保留重复 command（Dynamic2 `0x26×4` 前置）。
    #[test]
    fn slot_matching_keeps_duplicates() {
        use super::{decode_reply_payload, match_slot};
        let mut payload = vec![0x00, 0x03];
        for v in [0xAAu8, 0xBB, 0xCC] {
            payload.extend_from_slice(&[0x00, 0x11]); // size=17
            payload.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x26]);
            payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
            payload.extend_from_slice(&[0x00, 0x01, v]);
        }
        let subs = decode_reply_payload(&payload).unwrap();
        assert_eq!(subs.len(), 3);
        assert_eq!(match_slot(&subs, 1, 1, 0x26, 0).unwrap().data, vec![0xAA]);
        assert_eq!(match_slot(&subs, 1, 1, 0x26, 1).unwrap().data, vec![0xBB]);
        assert_eq!(match_slot(&subs, 1, 1, 0x26, 2).unwrap().data, vec![0xCC]);
        assert!(match_slot(&subs, 1, 1, 0x26, 3).is_none());
    }
}
