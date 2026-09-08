//! Mesa S7Comm 公共协议传输层（PR1 从 `s7` Driver 抽取）。
//!
//! 架构：
//! ```text
//! TCP / TPKT / COTP / S7Comm
//!      ↓
//! mesa-s7-transport（本 crate：建连/Setup/ReadVar/WriteVar/SZL/分片）
//!      ↓
//! ┌────────────┴────────────┐
//! s7（S7ANY 0x10）   sinumerik-nck（NCK 0x82/83/84，PR4 起）
//! ```
//!
//! 边界冻结：本 crate 只知道 S7Comm 线缆（TPKT/COTP/Setup/变量规范字节），
//! 绝对不知道 `DB10.DBD0` / S7ANY / NCK / Area / actFeedRate。变量规范是
//! 不透明字节（`S7ReadVarItem::var_spec`），由调用方编码；地址解析、类型
//! 解码、质量语义归 Driver。`mesa-core-types` / `mesa-driver-sdk` 零依赖。

pub mod config;
pub mod cotp;
pub mod error;
pub mod fixture;
pub mod pdu;
pub mod read_var;
pub mod session;
pub mod szl;
pub mod tpkt;
pub mod write_var;

pub use config::{
    S7ConnectOptions, S7_DEFAULT_PORT, S7_MAX_RACK, S7_MAX_SLOT, S7_MIN_TIMEOUT_MS, S7_PDU_DEFAULT,
    S7_PDU_MAX, S7_PDU_MIN, S7_TSAP_BASE, S7_TSAP_RACK_SHIFT,
};
pub use cotp::{COTP_CC, COTP_CR, COTP_DATA_HEADER, COTP_DT};
pub use error::{
    S7_ERR_ACCESS, S7_ERR_ADDRESS, S7_ERR_CONTEXT, S7_ITEM_OK, S7TransportError,
    S7TransportErrorKind, map_connect_error, s7_cpu_error,
};
pub use pdu::{S7_FUNC_READ, S7_FUNC_WRITE, S7_ROSCTR_ACK, S7_ROSCTR_JOB, S7_SYNTAX_ID_S7ANY};
pub use read_var::{
    S7ReadVarItem, S7ReadVarResult, S7_CHUNK_SAFETY_MARGIN, S7_MAX_ITEMS_PER_PDU,
    S7_TRANSPORT_BIT,
};
pub use session::S7Session;
pub use write_var::S7_WRITE_TRANSPORT_BYTE;
