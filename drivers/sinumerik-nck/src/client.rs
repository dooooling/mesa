//! NCK 会话客户端（Commit C：经 `mesa-s7-transport` 的 ReadVar 直读）。
//!
//! 分层（与 s7 对称）：
//! ```text
//! 本模块：NckConnConfig → S7Session 建连 / NckWireAddress → var_spec（codec）/
//!     传输错误 → SDK 错误映射
//!       ↓ 不透明 var_spec + 期望长度
//! mesa-s7-transport（S7Session）：TCP/TPKT/COTP/Setup/ReadVar/分片
//! ```
//!
//! 逐项错误隔离：单项 return code 非 0xFF → 该项 `None`（调用方发 BAD），
//! 不整体失败；整包 ROSCTR/长度/基数错位才是连接级 fatal（transport 侧判定）。

use mesa_driver_sdk::SdkDriverError;

use crate::codec::{NckWireAddress, encode_var_spec};
use crate::config::NckConnConfig;
use mesa_core_types::ErrorKind;
use mesa_s7_transport::{S7ReadVarItem, S7Session, S7TransportError, S7TransportErrorKind};

/// 单个 NCK 读项（线缆地址 + 期望返回字节数，见 `codec::resolve`）。
#[derive(Debug, Clone)]
pub struct NckReadItem {
    pub wire: NckWireAddress,
    pub expected_data_len: usize,
}

/// 单项读结果：返回码 + 数据（BAD 项 `data` 为空，调用方按项隔离发 BAD，
/// `return_code` 留作 quality_code 诊断）。
#[derive(Debug, Clone)]
pub struct NckReadResult {
    pub return_code: u8,
    pub data: Option<Vec<u8>>,
}

/// NCK 客户端：持有已建立的 S7Comm 会话。
pub struct NckClient {
    session: S7Session,
}

impl NckClient {
    pub fn negotiated_pdu_length(&self) -> u16 {
        self.session.negotiated_pdu_length()
    }

    /// 建立连接并完成握手（TSAP 直通，无 rack/slot 推导）。
    pub async fn connect(cfg: &NckConnConfig) -> Result<Self, SdkDriverError> {
        let session = S7Session::connect(cfg.to_transport())
            .await
            .map_err(map_transport_error)?;
        tracing::info!(
            host = %cfg.host,
            port = cfg.port,
            pdu = session.negotiated_pdu_length(),
            "NCK 会话建立"
        );
        Ok(Self { session })
    }

    /// 批量读。返回与 items 等长的逐项结果，单项 return code 非 0xFF 即 BAD
    /// （`data` 为空，不整体失败）；整包 ROSCTR/长度/基数错位才是连接级 fatal。
    pub async fn read_vars(
        &mut self,
        items: &[NckReadItem],
    ) -> Result<Vec<NckReadResult>, SdkDriverError> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        let encoded: Vec<S7ReadVarItem> = items
            .iter()
            .map(|it| S7ReadVarItem {
                var_spec: encode_var_spec(&it.wire),
                expected_data_len: it.expected_data_len,
            })
            .collect();
        let results = self
            .session
            .read_var(&encoded)
            .await
            .map_err(map_transport_error)?;
        Ok(results
            .into_iter()
            .map(|r| {
                if r.return_code != mesa_s7_transport::S7_ITEM_OK {
                    tracing::warn!(return_code = r.return_code, "NCK item 按项 BAD",);
                    NckReadResult {
                        return_code: r.return_code,
                        data: None,
                    }
                } else {
                    NckReadResult {
                        return_code: r.return_code,
                        data: Some(r.data),
                    }
                }
            })
            .collect())
    }
}

/// 传输错误 → SDK 错误（kind 一一映射，code/message 原样透传；与 s7 同口径）。
fn map_transport_error(e: S7TransportError) -> SdkDriverError {
    let kind = match e.kind {
        S7TransportErrorKind::Timeout => ErrorKind::Timeout,
        S7TransportErrorKind::Connection => ErrorKind::Connection,
        S7TransportErrorKind::Protocol => ErrorKind::Protocol,
        S7TransportErrorKind::Configuration => ErrorKind::Configuration,
        S7TransportErrorKind::Address => ErrorKind::Address,
    };
    SdkDriverError::new(kind, e.code, e.message)
}
