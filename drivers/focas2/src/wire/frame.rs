//! FOCAS Ethernet Wire Frame（PR1）：FOCAS application frame 编解码。
//!
//! - 只解决字节 ↔ 帧的映射，不懂 TCP、不懂 Operation、不懂 Mesa Resource。
//! - Gate 0（165 真机）锁定的协议事实：
//!   `magic a0 a0 a0 a0` / origin 为 raw u16（C→S `0x0001`、S→C `0x0003`，
//!   不断言、不推广）/ OPEN(`0x0101/0x0102`) 与 CLOSE(`0x0201/0x0202`)
//!   无 GENERIC subpacket 层 / GENERIC(`0x2101/0x2102`) 内为
//!   `u16 BE count + subpacket[]`，每个 subpacket 自带 `u16 BE length`
//!   （含自身 2B），按 `10B header + payload_len` 精确切帧。
//! - `PacketType` 为开放 `u16` 新类型（程序传输 `0x15xx/0x16xx/0x17xx`
//!   等未来只加常量，不重构 parser）。

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
    /// OPEN 请求（Gate 0：`a0 a0 a0 a0 00 01 01 01 00 02 00 01`）。
    pub const OPEN_REQUEST: Self = Self(0x0101);
    /// OPEN 响应（Gate 0：`… 00 03 01 02 01 68 …`，370B）。
    pub const OPEN_RESPONSE: Self = Self(0x0102);
    /// CLOSE 请求（Gate 0：`a0 a0 a0 a0 00 01 02 01 00 00`，10B）。
    pub const CLOSE_REQUEST: Self = Self(0x0201);
    /// CLOSE 响应（Gate 0：`a0 a0 a0 a0 00 03 02 02 00 00`，10B）。
    pub const CLOSE_RESPONSE: Self = Self(0x0202);
    /// GENERIC 请求（Gate 0：`0x2101`，内含 subpacket 层）。
    pub const GENERIC_REQUEST: Self = Self(0x2101);
    /// GENERIC 响应（Gate 0：`0x2102`，内含 subpacket 层）。
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocasFrame {
    /// 原始 origin（Gate 0：C→S `0x0001`、S→C `0x0003`；透传，不校验）。
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
            origin: 0x0001,
            packet_type: PacketType::OPEN_REQUEST,
            payload: vec![0x00, 0x02],
        }
    }

    /// CLOSE 请求帧（Gate 0：10B，payload 为空）。
    pub fn close_request() -> Self {
        Self {
            origin: 0x0001,
            packet_type: PacketType::CLOSE_REQUEST,
            payload: Vec::new(),
        }
    }
}

/// 10B header 解码结果（`payload_len` 已取，payload 另读）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// 原始 origin（透传）。
    pub origin: u16,
    /// 开放 packet 类型。
    pub packet_type: PacketType,
    /// 其后 payload 字节数（`<= 65535`，超限即 `BadLength`）。
    pub payload_len: usize,
}

/// 解 10B header：验 magic + 取 origin/type/payload_len（全 BE）。
/// payload 本体由调用方 `read_exact(payload_len)` 读取（防 TCP 分片/粘包）。
pub fn decode_header(raw: &[u8; FRAME_HEADER_LEN]) -> Result<FrameHeader, FrameError> {
    if raw[0..4] != SYNC_PREFIX {
        return Err(FrameError::BadMagic);
    }
    let origin = u16::from_be_bytes([raw[4], raw[5]]);
    let packet_type = PacketType(u16::from_be_bytes([raw[6], raw[7]]));
    let payload_len = u16::from_be_bytes([raw[8], raw[9]]) as usize;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(FrameError::BadLength);
    }
    Ok(FrameHeader {
        origin,
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
// Generic subpacket 层（仅 0x2101/0x2102 内）
// ---------------------------------------------------------------------------

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
        let body_len = 2 + 4 + self.payload.len();
        let mut out = Vec::with_capacity(2 + body_len);
        out.extend_from_slice(&((body_len + 2) as u16).to_be_bytes());
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
pub fn request_subpacket(control_device: u16, function: u32, args: [i32; 5]) -> GenericSubpacket {
    let mut payload = Vec::with_capacity(20);
    for a in args {
        payload.extend_from_slice(&a.to_be_bytes());
    }
    GenericSubpacket {
        control_device,
        function,
        payload,
    }
}

/// GENERIC payload 解码：`count + length 自描述 subpacket[]`。
/// 每个 subpacket 按自身 length 切（不搜 magic、不假设等长）；
/// 消费完 count 个后 payload 必须恰好耗尽（trailing garbage 即错）。
/// 数量/边界/尾部任一不符即 `Malformed`（调用方判 session 失效）。
pub fn decode_generic_payload(payload: &[u8]) -> Result<Vec<GenericSubpacket>, FrameError> {
    if payload.len() < 2 {
        return Err(FrameError::Malformed);
    }
    let count = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    let mut out = Vec::with_capacity(count);
    let mut off = 2;
    for _ in 0..count {
        if off + 2 > payload.len() {
            return Err(FrameError::Malformed);
        }
        let len = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
        if len < 2 + 2 + 4 || off + len > payload.len() {
            return Err(FrameError::Malformed);
        }
        let body = &payload[off + 2..off + len];
        out.push(GenericSubpacket {
            control_device: u16::from_be_bytes([body[0], body[1]]),
            function: u32::from_be_bytes([body[2], body[3], body[4], body[5]]),
            payload: body[6..].to_vec(),
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
}
