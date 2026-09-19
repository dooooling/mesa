//! FOCAS Ethernet Wire（PR1）：`WireError` + 模块装配。
//!
//! - FOCAS 私有错误（不改 Mesa 公共 Error 模型）。
//! - 致命性二分：`is_session_fatal() == true`（Io/Timeout/Closed/
//!   BadMagic/BadLength/UnexpectedPacket/Malformed/CommandMismatch…）
//!   意味着字节流边界不可信，调用方必须 `session = None`；
//!   `Remote/Unsupported` 为合法业务失败，session 保留。
//!   （`CommandMismatch` 保守判致命：响应与请求对不上时，
//!   无法证明流还在边界上；point-local 的失败走 `Remote`。）

use std::fmt;

/// frame 编解码（见 `frame.rs`）。
pub mod frame;
/// TCP 会话（见 `session.rs`）。
pub mod session;
/// client + typed ops + adapter（见 `wire.rs`；与本模块同名是历史命名，
/// 未来 operation 增多拆分子模块时再改名，现在不动）。
#[allow(clippy::module_inception)]
pub mod wire;

pub use frame::PacketType;
#[allow(unused_imports)]
pub use frame::{FocasFrame, GenericSubpacket};
pub use session::WireSession;
pub use wire::{FocasClient, StatusInfo, SystemInfo, WireFocasApi};

// ---------------------------------------------------------------------------
// Fixture 回归 helpers（tests + wire_probe 共用；生产路径不用）
// ---------------------------------------------------------------------------

/// fixture 根（`drivers/focas2/tests/fixtures/wire/`，相对 workspace root）。
/// 调用方（integration test / example）按需拼接组名。
pub fn fixture_dir() -> std::path::PathBuf {
    // cargo test 的 CWD = package 根（drivers/focas2），example 同理。
    std::path::PathBuf::from("tests/fixtures/wire")
}

/// 读 fixture 二进制（`*.bin`）。
pub fn read_fixture_bytes(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("读 fixture {path:?} 失败：{e}"))
}

/// fixture 帧切分：按 `10B header + payload_len` 精确切（Gate 0 裁决：
/// 绝不在拼接流里搜 `00 1c` 定界）。fixture 已清洗，应恰好 N 帧。
pub fn cut_fixture_frames(raw: &[u8]) -> Vec<Vec<u8>> {
    let first = raw
        .windows(4)
        .position(|w| w == [0xA0; 4])
        .unwrap_or_else(|| panic!("fixture 无 magic"));
    // 跳过前导拼接残留（`00 00…`），从首个 magic 起切。
    let mut pos = first;
    let mut out = Vec::new();
    while pos + 10 <= raw.len() {
        if raw[pos..pos + 4] != [0xA0; 4] {
            break;
        }
        let ln = u16::from_be_bytes([raw[pos + 8], raw[pos + 9]]) as usize;
        if pos + 10 + ln > raw.len() {
            break;
        }
        out.push(raw[pos..pos + 10 + ln].to_vec());
        pos += 10 + ln;
    }
    out
}

// ---------------------------------------------------------------------------
// WireError
// ---------------------------------------------------------------------------

/// FOCAS Wire 私有错误。
#[derive(Debug)]
pub enum WireError {
    /// 传输 IO（`read_exact/write_all` 非 EOF 失败）。
    Io(std::io::Error),
    /// 读写超时（含建连超时）。
    Timeout,
    /// 对端关闭（`read_exact` 遇 EOF）。
    Closed,
    /// magic 非 `a0 a0 a0 a0`。
    BadMagic,
    /// `payload_len` 与实收不符。
    BadLength,
    /// 期望 packet 类型不符。
    UnexpectedPacket {
        /// 期望值。
        expected: PacketType,
        /// 实际值。
        got: PacketType,
    },
    /// GENERIC count/length 不自洽，或 typed 解码结构不符。
    MalformedPayload,
    /// 请求↔响应 function 对不上（保守致命）。
    CommandMismatch,
    /// CNC 返回业务错误（`cmd + i16 != 0`，如 `EW_NOOPT`；session 保留）。
    Remote(i16),
    /// 未实现能力（fail-closed，如 PR1 非 Status 地址）。
    Unsupported(&'static str),
}

impl WireError {
    /// 是否必须使 session 失效（`session = None`，由上层重连）。
    /// `Remote/Unsupported` 为合法语义，返回 false。
    pub fn is_session_fatal(&self) -> bool {
        match self {
            Self::Io(_)
            | Self::Timeout
            | Self::Closed
            | Self::BadMagic
            | Self::BadLength
            | Self::UnexpectedPacket { .. }
            | Self::MalformedPayload
            | Self::CommandMismatch => true,
            Self::Remote(_) | Self::Unsupported(_) => false,
        }
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "FOCAS wire io: {e}"),
            Self::Timeout => write!(f, "FOCAS wire timeout"),
            Self::Closed => write!(f, "FOCAS wire closed"),
            Self::BadMagic => write!(f, "FOCAS wire bad magic"),
            Self::BadLength => write!(f, "FOCAS wire bad length"),
            Self::UnexpectedPacket { expected, got } => {
                write!(
                    f,
                    "FOCAS wire unexpected packet (expected {expected}, got {got})"
                )
            }
            Self::MalformedPayload => write!(f, "FOCAS wire malformed payload"),
            Self::CommandMismatch => write!(f, "FOCAS wire command mismatch"),
            Self::Remote(code) => write!(f, "FOCAS remote error {code}"),
            Self::Unsupported(what) => write!(f, "FOCAS unsupported: {what}"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
