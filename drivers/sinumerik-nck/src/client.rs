//! NCK 会话客户端（经 `mesa-s7-transport` 的 ReadVar 直读）。
//!
//! 分层（与 s7 对称）：
//! ```text
//! 本模块：NckConnConfig → S7Session 建连 / NckWireAddress → var_spec（codec）/
//!     逐项响应校验（P1-4）/ 传输错误 → SDK 错误映射
//!       ↓ 不透明 var_spec + 期望长度（分片 hint）
//! mesa-s7-transport（S7Session）：TCP/TPKT/COTP/Setup/ReadVar/分片
//! ```
//!
//! 逐项错误隔离（P1-4 fail-closed，不猜）：单项 `return_code != 0xFF`、
//! `transport_size` 与期望不符、wire 数据长度与期望不符，任一成立该项即 BAD
//! （`data` 为空，调用方发 BAD 点）；整包 ROSCTR/长度/基数错位才是连接级
//! fatal（transport 侧判定）。

use mesa_driver_sdk::SdkDriverError;

use crate::codec::{NckWireAddress, encode_var_spec};
use crate::config::NckConnConfig;
use mesa_core_types::ErrorKind;
use mesa_s7_transport::{S7ReadVarItem, S7Session, S7TransportError, S7TransportErrorKind};

/// 单个 NCK 读项（线缆地址 + 响应期望，见 `codec::resolve`）。
#[derive(Debug, Clone)]
pub struct NckReadItem {
    pub wire: NckWireAddress,
    pub expected_data_len: usize,
    /// 响应 `transport_size` 期望（catalog `wire.transport_size`）。
    pub expected_transport_size: u8,
}

/// 单项读结果：返回码 + 传输尺寸 + 数据（BAD 项 `data` 为空，调用方按项
/// 隔离发 BAD；`return_code`（非 FF 时）留作 quality_code 诊断，
/// transport/长度 mismatch 时 `return_code` 为 FF、`quality_code` 为 None，
/// 原因见 warn 日志，不伪造协议码）。
#[derive(Debug, Clone)]
pub struct NckReadResult {
    pub return_code: u8,
    pub transport_size: u8,
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

    /// 批量读。返回与 items 等长的逐项结果；任一 mismatch（return code /
    /// transport / 长度）该项即 BAD（`data` 为空，不整体失败）；
    /// 整包 ROSCTR/长度/基数错位才是连接级 fatal。
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
            .zip(items.iter())
            .map(|(r, it)| {
                let bad = if r.return_code != mesa_s7_transport::S7_ITEM_OK {
                    tracing::warn!(return_code = r.return_code, "NCK item 按项 BAD");
                    true
                } else if r.transport_size != it.expected_transport_size {
                    // P1-4：错误类型的数据绝不能标 GOOD（CNC 采集红线）。
                    tracing::warn!(
                        got = r.transport_size,
                        want = it.expected_transport_size,
                        "NCK transport 类型不符，按项 BAD",
                    );
                    true
                } else if r.data.len() != it.expected_data_len {
                    tracing::warn!(
                        got = r.data.len(),
                        want = it.expected_data_len,
                        "NCK 响应长度不符，按项 BAD",
                    );
                    true
                } else {
                    false
                };
                NckReadResult {
                    return_code: r.return_code,
                    transport_size: r.transport_size,
                    data: if bad { None } else { Some(r.data) },
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
