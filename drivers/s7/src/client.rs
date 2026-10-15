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
    S7_MAX_RACK, S7_MAX_SLOT, S7_PDU_DEFAULT, S7_PDU_MAX, S7_PDU_MIN, S7ConnectOptions, S7Session,
    S7TransportError, S7TransportErrorKind,
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
    /// 从 JSON 解析。
    ///
    /// 契约（C1 §6.2「缺席 vs 存在但非法」）：
    /// - **字段缺席**：`required` 字段（host）拒绝；真正 optional 的字段
    ///   才使用 Descriptor 同义的默认值；
    /// - **字段存在但类型/范围非法**（含 `null`、字符串数字、布尔、小数、
    ///   负数、越界）：一律 `BAD_CONFIG`，绝不 `and_then(...).unwrap_or(...)`
    ///   把非法类型当缺席后静默回落到默认连接目标。
    ///
    /// 数值一律先判范围再窄化（不允许先 `as u16` 再判 `== 0`，那会把
    /// 65536 截断成 0、把 70000 截断成 4464 而连到另一台设备）。
    pub fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        fn bad(message: impl Into<String>) -> SdkDriverError {
            SdkDriverError::configuration("BAD_CONFIG", message)
        }
        let object = v.as_object().ok_or_else(|| bad("connection 需为对象"))?;
        // host：必填、非空字符串。缺席/类型非法都不得回落到内置默认目标。
        let host = object
            .get("host")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("host 必填且必须为非空字符串（不从默认目标隐式连接）"))?
            .to_string();
        let mut cfg = Self {
            host,
            ..Self::default()
        };
        if let Some(value) = object.get("port") {
            let p = value
                .as_u64()
                .ok_or_else(|| bad("port 必须为 1..=65535 的整数"))?;
            if !(1..=65535).contains(&p) {
                return Err(bad(format!("port {p} 非法，需 1..=65535")));
            }
            cfg.port = p as u16;
        }
        if let Some(value) = object.get("rack") {
            let r = value.as_u64().ok_or_else(|| bad("rack 必须为整数"))?;
            if r > S7_MAX_RACK as u64 {
                return Err(bad(format!("rack {r} 非法，允许 0..{S7_MAX_RACK}")));
            }
            cfg.rack = r as u8;
        }
        if let Some(value) = object.get("slot") {
            let s = value.as_u64().ok_or_else(|| bad("slot 必须为整数"))?;
            if s > S7_MAX_SLOT as u64 {
                return Err(bad(format!("slot {s} 非法，允许 0..{S7_MAX_SLOT}")));
            }
            cfg.slot = s as u8;
        }
        if let Some(value) = object.get("timeout_ms") {
            let t = value
                .as_u64()
                .ok_or_else(|| bad("timeout_ms 必须为非负整数"))?;
            // 下限/上限都是**显式契约**而不是静默抬升或截断：越界即拒绝，
            // 避免“用户填 10、实际按 500 运行”或“填 60000、实际按 12s 被掐断”。
            if t < mesa_core_types::CONNECTION_TIMEOUT_MIN_MS {
                return Err(bad(format!(
                    "timeout_ms {t} 非法，最小值为 {}",
                    mesa_core_types::CONNECTION_TIMEOUT_MIN_MS
                )));
            }
            if t > mesa_core_types::CONNECTION_TIMEOUT_MAX_MS {
                return Err(bad(format!(
                    "timeout_ms {t} 超出支持上限 {}（Probe 分阶段预算按该值推导，\
                     超限配置无法被可靠验证）",
                    mesa_core_types::CONNECTION_TIMEOUT_MAX_MS
                )));
            }
            cfg.timeout_ms = t;
        }
        if let Some(value) = object.get("pdu_length") {
            let pdu = value.as_u64().ok_or_else(|| bad("pdu_length 必须为整数"))?;
            if !(S7_PDU_MIN as u64..=S7_PDU_MAX as u64).contains(&pdu) {
                return Err(bad(format!(
                    "pdu_length {pdu} 非法，需 {S7_PDU_MIN}..={S7_PDU_MAX}（不截断、不 clamp）"
                )));
            }
            cfg.pdu_length = pdu as u16;
        }
        // 兼容 tsap 直接指定（可选）：当前仅存档，connect 时仍按 rack/slot 推导。
        // NOTE: 该字段不参与连接目标计算，保留读取以避免旧配置报未知字段。
        if let Some(tsap) = object.get("remote_tsap") {
            // 存在但类型非法同样拒绝，避免“看着生效、其实被忽略”。
            tsap.as_u64().ok_or_else(|| bad("remote_tsap 必须为整数"))?;
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
        let session = S7Session::connect(options)
            .await
            .map_err(map_transport_error)?;
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
        // wire 位地址超限即整批拒绝（单点 BAD 隔离只处理设备回码，
        // 不掩盖本地编码错误）。
        let mut encoded = Vec::with_capacity(items.len());
        for it in items {
            encoded.push(encode_read_item(&it.addr, it.kind).map_err(|e| {
                SdkDriverError::configuration("INVALID_ADDRESS", format!("编码失败: {e}"))
            })?);
        }
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
                        addr = %it.addr.display_label(it.kind.display_width()),
                        ret = r.return_code,
                        "S7 item 0x{:02X} 按项 BAD",
                        r.return_code
                    );
                    return None;
                }
                let mut bytes = r.data;
                // P1-2：transport 返回完整 wire 数据；长度语义由本层决定——
                // 截断到请求长度（多余字节是设备多回，不是本点数据；不足则
                // 原样透传，由 decode 报 SHORT_DATA，不在这里伪造）。
                let want = it.kind.byte_len();
                if bytes.len() > want {
                    bytes.truncate(want);
                }
                // 防御：BIT 读取返回 1 字节 0x00/0x01，归一化（调用方常规经 BYTE 读后取位，此处仅兜底直传 BOOL 的情形）。
                if it.kind == S7Kind::Bool {
                    bytes = vec![if bytes.first().copied().unwrap_or(0) != 0 {
                        1
                    } else {
                        0
                    }];
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
        let mut encoded = Vec::with_capacity(physical.len());
        for (addr, len) in &physical {
            encoded.push(encode_bulk_item(addr, *len).map_err(|e| {
                SdkDriverError::configuration("INVALID_ADDRESS", format!("编码失败: {e}"))
            })?);
        }
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
                    // P1-2：transport 返回完整 wire 数据；本层截断到请求长度，
                    // 恢复 lib 按期望偏移切片的不变式（不足则原样透传）。
                    let mut b = r.data;
                    let want = physical[i].1;
                    if b.len() > want {
                        b.truncate(want);
                    }
                    Some(b)
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
        let spec = encode_write_spec(addr, kind).map_err(|e| {
            SdkDriverError::configuration("INVALID_ADDRESS", format!("编码失败: {e}"))
        })?;
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

    // -----------------------------------------------------------------------
    // C1 §6.2：连接字段「缺席 vs 存在但非法」契约
    // -----------------------------------------------------------------------

    /// Host 必填：缺席或类型非法都不得回落到内置默认连接目标。
    #[test]
    fn host_is_required_and_never_falls_back_to_default_target() {
        let base = || serde_json::json!({"port": 102});
        assert_eq!(
            S7ConnConfig::from_json(&base()).unwrap_err().code,
            "BAD_CONFIG",
            "缺 host 必须拒绝（不得落回 127.0.0.1/192.168.0.1）"
        );
        for bad in [
            serde_json::json!(123),
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!(""),
            serde_json::json!("   "),
        ] {
            let mut v = base();
            v["host"] = bad.clone();
            assert_eq!(
                S7ConnConfig::from_json(&v).unwrap_err().code,
                "BAD_CONFIG",
                "host={bad} 必须拒绝"
            );
        }
        // 合法显式 host 通过，且不被改写。
        let mut ok = base();
        ok["host"] = serde_json::json!("10.0.0.7");
        assert_eq!(S7ConnConfig::from_json(&ok).unwrap().host, "10.0.0.7");
    }

    /// 数值字段：类型非法 / 越界一律拒绝，且不得先窄化再判断。
    #[test]
    fn numeric_fields_reject_ill_typed_and_out_of_range() {
        let with = |key: &str, value: serde_json::Value| {
            let mut v = serde_json::json!({"host": "10.0.0.7"});
            v[key] = value;
            S7ConnConfig::from_json(&v)
        };
        // 类型非法（含字符串数字/小数/布尔/null）→ 拒绝，不回退默认值。
        for key in ["port", "rack", "slot", "timeout_ms", "pdu_length"] {
            for bad in [
                serde_json::json!("102"),
                serde_json::json!(1.5),
                serde_json::json!(true),
                serde_json::json!(null),
            ] {
                assert_eq!(
                    with(key, bad.clone()).unwrap_err().code,
                    "BAD_CONFIG",
                    "{key}={bad} 必须拒绝"
                );
            }
        }
        // 越界：port 65536 若先 `as u16` 会截断成 0（连到另一语义目标）。
        assert!(with("port", serde_json::json!(65536)).is_err());
        assert!(with("port", serde_json::json!(0)).is_err());
        assert!(with("port", serde_json::json!(-1)).is_err());
        assert!(with("rack", serde_json::json!(8)).is_err());
        assert!(with("slot", serde_json::json!(32)).is_err());
        // timeout_ms：低于下限或高于支持上限都拒绝（不静默抬升/截断）。
        assert!(with("timeout_ms", serde_json::json!(10)).is_err());
        assert!(
            with(
                "timeout_ms",
                serde_json::json!(mesa_core_types::CONNECTION_TIMEOUT_MAX_MS + 1)
            )
            .is_err()
        );
        assert!(
            with(
                "timeout_ms",
                serde_json::json!(mesa_core_types::CONNECTION_TIMEOUT_MIN_MS)
            )
            .is_ok()
        );
        assert!(
            with(
                "timeout_ms",
                serde_json::json!(mesa_core_types::CONNECTION_TIMEOUT_MAX_MS)
            )
            .is_ok()
        );
        // pdu_length：越界拒绝而不是 clamp 到边界值。
        assert!(with("pdu_length", serde_json::json!(100)).is_err());
        assert!(with("pdu_length", serde_json::json!(10_000)).is_err());
        assert!(with("pdu_length", serde_json::json!(480)).is_ok());
    }

    /// 缺席的 optional 字段仍使用与 Descriptor 一致的默认值（不回归）。
    #[test]
    fn absent_optional_fields_keep_documented_defaults() {
        let cfg = S7ConnConfig::from_json(&serde_json::json!({"host": "10.0.0.7"})).unwrap();
        assert_eq!(cfg.port, 102);
        assert_eq!(cfg.rack, 0);
        assert_eq!(cfg.slot, 1);
        assert_eq!(cfg.timeout_ms, 3000);
        assert_eq!(cfg.pdu_length, S7_PDU_DEFAULT);
    }
}
