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

/// fixture 回归（`#[cfg(test)]`：crate 内部直测生产 codec，不出 crate）。
/// `pub(crate)` 的 items 仅测试构建可见：`pub(crate) use` 重导出在非测试
/// 构建下未使用是预期的（fixture 回归与 loopback/单测同属测试面）。
#[cfg(test)]
pub(crate) mod fixture_tests;
/// frame 编解码（见 `frame.rs`）。
pub(crate) mod frame;
/// TCP 会话（见 `session.rs`）。
pub(crate) mod session;
/// client + typed ops + adapter（见 `wire.rs`；与本模块同名是历史命名，
/// 未来 operation 增多拆分子模块时再改名，现在不动）。
/// `#[allow]` 压 inception（改名留待拆分时）。
#[allow(clippy::module_inception)]
pub(crate) mod wire;

#[allow(unused_imports)]
pub(crate) use frame::{FocasFrame, GenericSubpacket, PacketType};
#[allow(unused_imports)]
pub(crate) use session::WireSession;
#[allow(unused_imports)]
pub(crate) use wire::{AxisPosition, FocasClient, StatusInfo, SystemInfo};
// `WireFocasApi` 是 `wire_probe` 唯一需要的诊断入口（经 `wire_pub` 窄口出 crate）。
pub use wire::WireFocasApi;

// ---------------------------------------------------------------------------
// Fixture 回归（`#[cfg(test)]`）：crate 内部直测生产 codec。
// fixture 文件仍在 `tests/fixtures/wire/**`； helpers 不出 crate。
// ---------------------------------------------------------------------------

/// fixture 根（相对 package 根 `drivers/focas2`）。
#[cfg(test)]
pub(crate) fn fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("tests/fixtures/wire")
}

/// 读 fixture 二进制（`*.bin`）。
#[cfg(test)]
pub(crate) fn read_fixture_bytes(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("读 fixture {path:?} 失败：{e}"))
}

/// fixture 帧切分：按 `10B header + payload_len` 精确切（Gate 0 裁决：
/// 绝不在拼接流里搜 `00 1c` 定界）。fixture 已清洗，应恰好 N 帧。
#[cfg(test)]
pub(crate) fn cut_fixture_frames(raw: &[u8]) -> Vec<Vec<u8>> {
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
    /// PR1 的 `0x18/0x19` 均为成功路径，此变体为 PR2+（axis/pmc/macro 等
    /// point-local 失败）预留，未使用告警允许。
    #[allow(dead_code)]
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
