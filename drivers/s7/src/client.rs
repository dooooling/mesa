//! S7 Driver 连接适配（方案 §7.1，PR1 起经公共 `mesa-s7-transport` 会话）。
//!
//! 分层：
//! ```text
//! 本模块（Driver 侧）：S7ConnConfig 解析 / S7ANY 编码（s7any）/
//!     连续区合并（fragment/reassemble）/ 错误映射 / BOOL 取位前的字节读
//!       ↓ 不透明 var_spec + 期望长度
//! mesa-s7-transport（S7Session）：TCP/TPKT/COTP/Setup/ReadVar/分片
//! ```
//!
//! 对外 API（`S7ConnConfig` / `ReadItem` / `S7Client` 方法签名）与抽取前一致；
//! Common SZL 直连诊断继续经本模块暴露（V1 只读，不影响 Core 隔离）。

use mesa_driver_sdk::SdkDriverError;

use crate::address::S7Address;
use crate::codec::S7Kind;
use crate::s7any::{encode_bulk_item, encode_read_item, encode_write_spec};
use mesa_core_types::ErrorKind;
use mesa_s7_transport::{
    S7ConnectOptions, S7Session, S7TransportError, S7TransportErrorKind,
    S7_MAX_RACK, S7_MAX_SLOT, S7_MIN_TIMEOUT_MS, S7_PDU_DEFAULT, S7_PDU_MAX, S7_PDU_MIN,
};

/// 连接参数（来自 Endpoint.connection JSON，契约与抽取前一致）。
#[derive(Debug, Clone)]
pub struct S7ConnConfig {
    pub host: String,
    pub port: u16,
    pub rack: u8,
    pub slot: u8,
    pub timeout_ms: u64,
    pub pdu_length: u16,
}

impl Default for S7ConnConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: mesa_s7_transport::S7_DEFAULT_PORT,
            rack: 0,
            slot: 1,
            timeout_ms: 3000,
            pdu_length: S7_PDU_DEFAULT,
        }
    }
}

impl S7ConnConfig {
    /// 从 JSON 解析，缺省字段使用默认值。非法字段返回 ConfigurationError。
    pub fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        let mut cfg = Self::default();
        if let Some(h) = v.get("host").and_then(|x| x.as_str()) {
            cfg.host = h.to_string();
        }
        if let Some(p) = v.get("port").and_then(|x| x.as_u64()) {
            if p == 0 || p > 65535 {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("port {p} 非法"),
                ));
            }
            cfg.port = p as u16;
        }
        if let Some(r) = v.get("rack").and_then(|x| x.as_u64()) {
            if r > S7_MAX_RACK as u64 {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("rack {r} 非法，允许 0..{S7_MAX_RACK}"),
                ));
            }
            cfg.rack = r as u8;
        }
        if let Some(s) = v.get("slot").and_then(|x| x.as_u64()) {
            if s > S7_MAX_SLOT as u64 {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("slot {s} 非法，允许 0..{S7_MAX_SLOT}"),
                ));
            }
            cfg.slot = s as u8;
        }
        if let Some(t) = v.get("timeout_ms").and_then(|x| x.as_u64()) {
            cfg.timeout_ms = t.max(S7_MIN_TIMEOUT_MS);
        }
        if let Some(pdu) = v.get("pdu_length").and_then(|x| x.as_u64()) {
            cfg.pdu_length = (pdu as u16).clamp(S7_PDU_MIN, S7_PDU_MAX);
        }
        // 兼容 tsap 直接指定（可选）
        if let Some(tsap) = v.get("remote_tsap").and_then(|x| x.as_u64()) {
            // 覆盖 rack/slot 推导：高字节为 rack<<5|slot 的 TSAP 方案
            // 这里仅存档，connect 时覆写计算
            let _ = tsap;
        }
        if cfg.host.is_empty() {
            return Err(SdkDriverError::configuration("BAD_CONFIG", "host 不能为空"));
        }
        Ok(cfg)
    }

    /// 转传输层连接选项（rack/slot → SIMATIC 约定 TSAP）。
    fn to_transport(&self) -> Result<S7ConnectOptions, SdkDriverError> {
        S7ConnectOptions::from_rack_slot(
            &self.host,
            self.port,
            self.rack,
            self.slot,
            self.timeout_ms,
            self.pdu_length,
        )
        .map_err(|e| SdkDriverError::configuration("BAD_CONFIG", e))
    }
}

/// 单个读项（地址 + 类型）。
#[derive(Debug, Clone)]
pub struct ReadItem {
    pub addr: S7Address,
    pub kind: S7Kind,
}

/// S7 客户端：持有已建立会话的薄适配（分片/编解码在传输层与 s7any）。
pub struct S7Client {
    session: S7Session,
}

impl S7Client {
    pub fn pdu_length(&self) -> u16 {
        self.session.negotiated_pdu_length()
    }

    /// 建立连接并完成握手。失败返回带诊断的 SdkDriverError。
    pub async fn connect(cfg: S7ConnConfig) -> Result<Self, SdkDriverError> {
        let options = cfg.to_transport()?;
        let session = S7Session::connect(options).await.map_err(map_transport_error)?;
        tracing::info!(
            host = %cfg.host,
            port = cfg.port,
            rack = cfg.rack,
            slot = cfg.slot,
            pdu = session.negotiated_pdu_length(),
            "S7 连接建立"
        );
        Ok(Self { session })
    }

    /// 批量读取。返回与 items 等长的 `Option<原始字节>`，`None` 表示该 item 的 S7 返回码非 0xFF（按项 BAD 隔离，不整体失败）。
    pub async fn read_vars(
        &mut self,
        items: &[ReadItem],
    ) -> Result<Vec<Option<Vec<u8>>>, SdkDriverError> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        let encoded: Vec<_> = items
            .iter()
            .map(|it| encode_read_item(&it.addr, it.kind))
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
                if r.return_code != mesa_s7_transport::S7_ITEM_OK {
                    tracing::warn!(
                        addr = %format_addr(&it.addr),
                        ret = r.return_code,
                        "S7 item 0x{:02X} 按项 BAD",
                        r.return_code
                    );
                    return None;
                }
                let mut bytes = r.data;
                // 防御：BIT 读取返回 1 字节 0x00/0x01，归一化（调用方常规经 BYTE 读后取位，此处仅兜底直传 BOOL 的情形）。
                if it.kind == S7Kind::Bool {
                    bytes = vec![if bytes.first().copied().unwrap_or(0) != 0 { 1 } else { 0 }];
                }
                Some(bytes)
            })
            .collect())
    }

    /// 连续区合并后的批量 BYTE 读：每个 range 为 (起始地址, 字节长度) 的连续内存区。
    /// 单 logical range 若超过 negotiated PDU 可承载（STRING 256 @ PDU240 / WSTRING 516 @ PDU480），
    /// 在传输层按 PDU 自动分片为多个物理读并重组，避免上层 planner 头污染与跨 chunk 覆盖。
    pub async fn read_byte_ranges(
        &mut self,
        ranges: &[(S7Address, usize)],
    ) -> Result<Vec<Option<Vec<u8>>>, SdkDriverError> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }
        let (physical, logical_to_physical) =
            fragment_ranges(ranges, self.session.negotiated_pdu_length());
        let encoded: Vec<_> = physical
            .iter()
            .map(|(addr, len)| encode_bulk_item(addr, *len))
            .collect();
        // 整批一次 bulk 读：传输层按 PDU 自动打包多项（与抽取前 physical 打包
        // 循环同口径），逐项 BAD 隔离后重组回 logical。
        let results = self
            .session
            .read_bulk(&encoded)
            .await
            .map_err(map_transport_error)?;
        let physical_results: Vec<Option<Vec<u8>>> = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                if r.return_code != mesa_s7_transport::S7_ITEM_OK {
                    tracing::warn!(idx = i, ret = r.return_code, "Bulk item BAD");
                    None
                } else {
                    Some(r.data)
                }
            })
            .collect();
        Ok(reassemble_ranges(
            &logical_to_physical,
            &physical_results,
            ranges.len(),
        ))
    }

    /// 单点写入：DB10.DBW0 INT 2字节为例，data 为大端编码
    pub async fn write_single(
        &mut self,
        addr: &S7Address,
        kind: crate::codec::S7Kind,
        data: &[u8],
    ) -> Result<(), SdkDriverError> {
        let spec = encode_write_spec(addr, kind);
        self.session
            .write_var(&spec, data)
            .await
            .map_err(map_transport_error)
    }

    /// SZL 读取（Common 诊断）：S7 功能 0x07 SZL，返回原始 SZL 负载（已去 TPKT/COTP/S7 头）。
    /// 用于 `SZL 0x0011` CPU 诊断、`0x0131` 模块标识等只读诊断，不走点位批次。
    pub async fn read_szl(
        &mut self,
        szl_id: u16,
        szl_index: u16,
    ) -> Result<Vec<u8>, SdkDriverError> {
        self.session
            .read_szl(szl_id, szl_index)
            .await
            .map_err(map_transport_error)
    }
}

/// 传输错误 → SDK 错误（kind 一一映射，code/message 原样透传，保证诊断逐字一致）。
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

/// 将 logical ranges 按 PDU 分片为物理 ranges，返回 (physical, logical_to_physical)
/// 纯函数便于单测覆盖 STRING/WSTRING 跨 PDU 的分片与重组
pub(crate) fn fragment_ranges(
    ranges: &[(S7Address, usize)],
    pdu_len: u16,
) -> (Vec<(S7Address, usize)>, Vec<Vec<usize>>) {
    let max_single = (pdu_len as usize).saturating_sub(48).max(64);
    let mut physical: Vec<(S7Address, usize)> = Vec::new();
    let mut logical_to_physical: Vec<Vec<usize>> = vec![Vec::new(); ranges.len()];
    for (li, (addr, len)) in ranges.iter().enumerate() {
        if *len <= max_single {
            let pi = physical.len();
            physical.push((addr.clone(), *len));
            logical_to_physical[li].push(pi);
        } else {
            let mut remaining = *len;
            let mut off = 0u32;
            while remaining > 0 {
                let chunk_len = remaining.min(max_single);
                let chunk_addr = S7Address {
                    area: addr.area,
                    db_number: addr.db_number,
                    byte_offset: addr.byte_offset + off,
                    bit_offset: None,
                };
                let pi = physical.len();
                physical.push((chunk_addr, chunk_len));
                logical_to_physical[li].push(pi);
                off += chunk_len as u32;
                remaining = remaining.saturating_sub(chunk_len);
            }
        }
    }
    (physical, logical_to_physical)
}

/// 将物理结果重组回 logical（任一物理 BAD 则 logical BAD）
/// 复杂度 O(total logical bytes)（需复制所有 fragment 字节，256/516B 量级可忽略）
pub(crate) fn reassemble_ranges(
    logical_to_physical: &[Vec<usize>],
    physical_results: &[Option<Vec<u8>>],
    ranges_len: usize,
) -> Vec<Option<Vec<u8>>> {
    debug_assert_eq!(
        logical_to_physical.len(),
        ranges_len,
        "调用方需保证 logical_to_physical 与 ranges 同长"
    );
    let mut out = Vec::with_capacity(ranges_len);
    for pis in logical_to_physical {
        if pis.is_empty() {
            out.push(None);
            continue;
        }
        let mut buf = Vec::new();
        let mut ok = true;
        for &pi in pis {
            match &physical_results[pi] {
                Some(b) => buf.extend_from_slice(b),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            out.push(Some(buf));
        } else {
            out.push(None);
        }
    }
    out
}

fn format_addr(a: &S7Address) -> String {
    use crate::address::Area;
    match a.area {
        Area::Db => {
            if let Some(bit) = a.bit_offset {
                format!("DB{}.DBX{}.{}", a.db_number, a.byte_offset, bit)
            } else {
                format!("DB{}.{}", a.db_number, a.byte_offset)
            }
        }
        Area::Merker => {
            if let Some(bit) = a.bit_offset {
                format!("M{}.{}", a.byte_offset, bit)
            } else {
                format!("MB{}", a.byte_offset)
            }
        }
        Area::Input => {
            if let Some(bit) = a.bit_offset {
                format!("I{}.{}", a.byte_offset, bit)
            } else {
                format!("IB{}", a.byte_offset)
            }
        }
        Area::Output => {
            if let Some(bit) = a.bit_offset {
                format!("Q{}.{}", a.byte_offset, bit)
            } else {
                format!("QB{}", a.byte_offset)
            }
        }
        Area::Counter => format!("C{}", a.byte_offset),
        Area::Timer => format!("T{}", a.byte_offset),
        Area::PeripheralInput => format!("PIW{}", a.byte_offset),
        Area::PeripheralOutput => format!("PQW{}", a.byte_offset),
        Area::Local => {
            if let Some(bit) = a.bit_offset {
                format!("L{}.{}", a.byte_offset, bit)
            } else {
                format!("LB{}", a.byte_offset)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::parse_address;

    fn addr(s: &str) -> S7Address {
        parse_address(s).unwrap()
    }

    #[test]
    fn fragment_string_256_pdu240() {
        let ranges = vec![(addr("DB10.DBD0"), 256)];
        let (phys, map) = fragment_ranges(&ranges, 240);
        assert_eq!(phys.len(), 2, "256>192 应分2片");
        assert_eq!(phys[0].1, 192);
        assert_eq!(phys[1].1, 64);
        assert_eq!(map[0].len(), 2);
        // 模拟两物理 GOOD 重组
        let phys_res = vec![Some(vec![0xAA; 192]), Some(vec![0xBB; 64])];
        let reass = reassemble_ranges(&map, &phys_res, ranges.len());
        assert_eq!(reass[0].as_ref().unwrap().len(), 256);
        assert!(reass[0].as_ref().unwrap()[..192].iter().all(|&b| b == 0xAA));
        assert!(reass[0].as_ref().unwrap()[192..].iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn fragment_wstring_516_pdu480() {
        let ranges = vec![(addr("DB10.DBD0"), 516)];
        let (phys, map) = fragment_ranges(&ranges, 480);
        // PDU480 max 432 → 516→432+84
        assert_eq!(phys.len(), 2);
        assert_eq!(phys[0].1, 432);
        assert_eq!(phys[1].1, 84);
        let phys_res = vec![Some(vec![1; 432]), Some(vec![2; 84])];
        let reass = reassemble_ranges(&map, &phys_res, ranges.len());
        assert_eq!(reass[0].as_ref().unwrap().len(), 516);
    }

    #[test]
    fn fragment_wstring_516_pdu240() {
        let ranges = vec![(addr("DB10.DBD0"), 516)];
        let (phys, _map) = fragment_ranges(&ranges, 240);
        // 240→192 per chunk → 192*2+132
        assert_eq!(phys.len(), 3);
        assert_eq!(phys[0].1, 192);
        assert_eq!(phys[1].1, 192);
        assert_eq!(phys[2].1, 132);
    }

    #[test]
    fn fragment_reassemble_one_bad_all_bad() {
        let ranges = vec![(addr("DB10.DBD0"), 256)];
        let (_phys, map) = fragment_ranges(&ranges, 240);
        let phys_res = vec![Some(vec![0; 192]), None];
        let reass = reassemble_ranges(&map, &phys_res, ranges.len());
        assert!(reass[0].is_none(), "任一物理 BAD 则 logical BAD");
    }
}
