//! S7 ISO-on-TCP 最小客户端（方案 §7.1）。
//!
//! 实现足够通过 127.0.0.1:102 动态 DB10 采集的路径：
//! `TCP -> COTP CR/CC -> S7 Setup -> ReadVar`（支持多 item）。
//! 仅依赖 `tokio` 与 `bytes`，不引入额外 snap7 绑定。

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};

use crate::address::{Area, S7Address};
use crate::codec::S7Kind;
use mesa_core_types::ErrorKind;
use mesa_driver_sdk::SdkDriverError;

// ---------------------------------------------------------------------------
// 协议常量（中文注释解释“为什么”）
// ---------------------------------------------------------------------------

/// S7 默认端口：ISO-on-TCP 固定 102
const S7_DEFAULT_PORT: u16 = 102;
/// COTP/TSAP 基址：0x0100 为 S7 侧固定前缀，rack/slot 在此基础上编码
const S7_TSAP_BASE: u16 = 0x0100;
/// TSAP 中 rack 占 3 位，左移 5 位后与 slot 合并（与 snap7/TIA 兼容）
const S7_TSAP_RACK_SHIFT: u16 = 5;
/// 硬件限制：S7-300/400 rack 0..7，slot 0..31
const S7_MAX_RACK: u8 = 7;
const S7_MAX_SLOT: u8 = 31;
/// 超时下限：避免过小导致局域网抖动误超时
const S7_MIN_TIMEOUT_MS: u64 = 500;
/// PDU 协商区间：240 为最小可用，960 为上位机常用上限，480 为兼容默认值
const S7_PDU_MIN: u16 = 240;
const S7_PDU_MAX: u16 = 960;
const S7_PDU_DEFAULT: u16 = 480;
/// TPKT 固定：版本 0x03、长度校验上下限
const TPKT_VERSION: u8 = 0x03;
const TPKT_MIN_LEN: usize = 4;
const TPKT_MAX_LEN: usize = 8192;
/// COTP：CR 0xE0 / CC 0xD0，DT 0xF0
const COTP_CR: u8 = 0xE0;
const COTP_CC: u8 = 0xD0;
const COTP_DT: u8 = 0xF0;
/// S7：ROSCTR 0x01=Job 0x03=Ack，功能码 0x04=ReadVar
const S7_ROSCTR_JOB: u8 = 0x01;
const S7_ROSCTR_ACK: u8 = 0x03;
const S7_FUNC_READ: u8 = 0x04;
/// S7 单次 PDU 可携带 item 上限，受 480 字节 PDU 限制（12字节/item + 头），经验值 19
const S7_MAX_ITEMS_PER_PDU: usize = 19;
/// S7 错误码（CPU 侧返回，见 §7.1 诊断要求）
const S7_ERR_ADDRESS: u8 = 0x05;
const S7_ERR_CONTEXT: u8 = 0x04;
const S7_ERR_ACCESS: u8 = 0x03;
// TODO: S7 错误码预留，0x06 类型不匹配用于诊断，不直接抛错但需保留映射
#[allow(dead_code)]
const S7_ERR_TYPE_MISMATCH: u8 = 0x06;
const S7_ITEM_OK: u8 = 0xFF;
/// S7 传输层：0x04=Byte 0x03=Bit 0x10=S7 变量规范
// TODO: S7 传输类型预留，当前批量读仅用 0x12 简化，需保留以备按类型分流
#[allow(dead_code)]
const S7_TRANSPORT_BYTE: u8 = 0x04;
const S7_TRANSPORT_BIT: u8 = 0x03;
const S7_VAR_SPEC: u8 = 0x12;
const S7_VAR_SPEC_LEN: u8 = 0x0A;
const S7_SYNTAX_ID_S7ANY: u8 = 0x10;

/// 连接参数（来自 Endpoint.connection JSON）。
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
            port: S7_DEFAULT_PORT,
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
                    format!("port {} 非法", p),
                ));
            }
            cfg.port = p as u16;
        }
        if let Some(r) = v.get("rack").and_then(|x| x.as_u64()) {
            if r > S7_MAX_RACK as u64 {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("rack {} 非法，允许 0..{}", r, S7_MAX_RACK),
                ));
            }
            cfg.rack = r as u8;
        }
        if let Some(s) = v.get("slot").and_then(|x| x.as_u64()) {
            if s > S7_MAX_SLOT as u64 {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    format!("slot {} 非法，允许 0..{}", s, S7_MAX_SLOT),
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

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

/// 单个读项（地址 + 类型）。
#[derive(Debug, Clone)]
pub struct ReadItem {
    pub addr: S7Address,
    pub kind: S7Kind,
}

/// S7 客户端：持有已建立的 ISO/S7 会话。
pub struct S7Client {
    stream: TcpStream,
    pdu_ref: u16,
    pdu_length: u16,
    cfg: S7ConnConfig,
}

impl S7Client {
    /// 建立连接并完成握手。失败返回带诊断的 SdkDriverError。
    pub async fn connect(cfg: S7ConnConfig) -> Result<Self, SdkDriverError> {
        let addr = format!("{}:{}", cfg.host, cfg.port);
        let stream = timeout(cfg.timeout(), TcpStream::connect(&addr))
            .await
            .map_err(|_| {
                SdkDriverError::new(
                    ErrorKind::Timeout,
                    "CONNECT_TIMEOUT",
                    format!(
                        "连接 {addr} 超时（{}ms），检查 PLC 是否可达、端口 102 是否放通",
                        cfg.timeout_ms
                    ),
                )
            })?
            .map_err(|e| map_connect_error(e, &addr, &cfg))?;

        let mut client = Self {
            stream,
            pdu_ref: 1,
            pdu_length: cfg.pdu_length,
            cfg,
        };
        client.iso_connect().await?;
        client.s7_setup().await?;
        tracing::info!(host=%client.cfg.host, port=client.cfg.port, rack=client.cfg.rack, slot=client.cfg.slot, pdu=client.pdu_length, "S7 连接建立");
        Ok(client)
    }

    async fn iso_connect(&mut self) -> Result<(), SdkDriverError> {
        let src_tsap = S7_TSAP_BASE;
        // 经典推导：dst = 0x0100 | (rack<<5 | slot)，与 snap7/TIA Portal 兼容
        let dst_tsap =
            S7_TSAP_BASE | ((self.cfg.rack as u16) << S7_TSAP_RACK_SHIFT) | (self.cfg.slot as u16);
        let pkt = build_cotp_cr(src_tsap, dst_tsap);
        timeout(self.cfg.timeout(), self.send_raw(&pkt))
            .await
            .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "COTP_TIMEOUT", "COTP CR 超时"))?
            .map_err(|e| {
                SdkDriverError::new(ErrorKind::Connection, "COTP_SEND_FAIL", e.to_string())
            })?;
        let resp = timeout(self.cfg.timeout(), self.recv_packet())
            .await
            .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "COTP_TIMEOUT", "COTP CC 超时"))?
            .map_err(|e| {
                SdkDriverError::new(ErrorKind::Connection, "COTP_RECV_FAIL", e.to_string())
            })?;
        // COTP CC 期望 0xD0（Connection Confirm）
        if resp.len() < 6 || resp[5] != COTP_CC {
            return Err(SdkDriverError::new(
                ErrorKind::Connection,
                "COTP_REJECTED",
                format!("COTP CC 异常，响应 {resp:02x?}，检查 rack/slot 或 PLC TSAP 配置"),
            ));
        }
        Ok(())
    }

    async fn s7_setup(&mut self) -> Result<(), SdkDriverError> {
        let pkt = build_s7_setup(self.pdu_ref, self.pdu_length);
        self.pdu_ref = self.pdu_ref.wrapping_add(1).max(1);
        timeout(self.cfg.timeout(), self.send_raw(&pkt))
            .await
            .map_err(|_| {
                SdkDriverError::new(ErrorKind::Timeout, "S7_SETUP_TIMEOUT", "S7 Setup 超时")
            })?
            .map_err(|e| {
                SdkDriverError::new(ErrorKind::Connection, "S7_SETUP_SEND_FAIL", e.to_string())
            })?;
        let resp = timeout(self.cfg.timeout(), self.recv_packet())
            .await
            .map_err(|_| {
                SdkDriverError::new(ErrorKind::Timeout, "S7_SETUP_TIMEOUT", "S7 Setup 响应超时")
            })?
            .map_err(|e| {
                SdkDriverError::new(ErrorKind::Connection, "S7_SETUP_RECV_FAIL", e.to_string())
            })?;
        // 解析 S7 payload
        let payload = &resp[7..]; // 跳过 TPKT 4 + COTP 3
        if payload.len() < 12 {
            return Err(SdkDriverError::new(
                ErrorKind::Protocol,
                "S7_SETUP_SHORT",
                format!("Setup 响应过短 {}", payload.len()),
            ));
        }
        // S7 ROSCTR 0x03 = Ack（0x01 为 Job）
        if payload[1] != S7_ROSCTR_ACK {
            let err = payload.get(17).copied().unwrap_or(0);
            return Err(map_s7_error(err, "S7 Setup 被拒绝"));
        }
        // 协商 PDU 长度位于 param 偏移 6..8
        if payload.len() >= 25 {
            let negotiated = u16::from_be_bytes([payload[23], payload[24]]);
            if negotiated != 0 && negotiated < self.pdu_length {
                self.pdu_length = negotiated;
                tracing::info!(negotiated, "S7 PDU 已协商");
            }
        }
        Ok(())
    }

    /// 批量读取。返回与 items 等长的 `Option<原始字节>`，`None` 表示该 item 的 S7 返回码非 0xFF（按项 BAD 隔离，不整体失败）。
    pub async fn read_vars(
        &mut self,
        items: &[ReadItem],
    ) -> Result<Vec<Option<Vec<u8>>>, SdkDriverError> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        // PDU 480 字节时单次最多约 19 项（12字节/item），超出分批；同时按字节长度动态限流，避免 LREAL/STRING 大项撑爆 PDU
        let mut all: Vec<Option<Vec<u8>>> = Vec::with_capacity(items.len());
        let mut start = 0;
        while start < items.len() {
            let mut end = start + 1;
            let mut bytes = items[start].kind.byte_len() + 12;
            while end < items.len() && end - start < S7_MAX_ITEMS_PER_PDU {
                let next_len = items[end].kind.byte_len() + 12 + 4; // 12 请求头 + 4 返回头
                if bytes + next_len > self.pdu_length as usize - 32 {
                    break;
                }
                bytes += next_len;
                end += 1;
            }
            let chunk = &items[start..end];
            let pkt = build_read_req(self.pdu_ref, chunk);
            self.pdu_ref = self.pdu_ref.wrapping_add(1).max(1);
            timeout(self.cfg.timeout(), self.send_raw(&pkt))
                .await
                .map_err(|_| {
                    SdkDriverError::new(ErrorKind::Timeout, "READ_TIMEOUT", "Read 请求超时")
                })?
                .map_err(|e| {
                    SdkDriverError::new(ErrorKind::Connection, "READ_SEND_FAIL", e.to_string())
                })?;
            let resp = timeout(self.cfg.timeout(), self.recv_packet())
                .await
                .map_err(|_| {
                    SdkDriverError::new(ErrorKind::Timeout, "READ_TIMEOUT", "Read 响应超时")
                })?
                .map_err(|e| {
                    SdkDriverError::new(ErrorKind::Connection, "READ_RECV_FAIL", e.to_string())
                })?;
            let mut part = parse_read_resp(&resp, chunk)?;
            all.append(&mut part);
            start = end;
        }
        Ok(all)
    }

    /// 连续区合并后的批量 BYTE 读：每个 range 为 (起始地址, 字节长度) 的连续内存区，单次 PDU 同时按项数与总字节数限流。
    /// 用于 S7 continuous-range merge，减少 PDU 往返（§7.1 合并连续区域）。
    pub async fn read_byte_ranges(
        &mut self,
        ranges: &[(S7Address, usize)],
    ) -> Result<Vec<Option<Vec<u8>>>, SdkDriverError> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }
        let mut all = Vec::with_capacity(ranges.len());
        let mut start = 0;
        while start < ranges.len() {
            let mut end = start + 1;
            let mut bytes = ranges[start].1 + 12 + 4;
            while end < ranges.len() && end - start < S7_MAX_ITEMS_PER_PDU {
                let next_len = ranges[end].1 + 12 + 4;
                if bytes + next_len > self.pdu_length as usize - 32 {
                    break;
                }
                bytes += next_len;
                end += 1;
            }
            let chunk = &ranges[start..end];
            let pkt = build_bulk_read_req(self.pdu_ref, chunk);
            self.pdu_ref = self.pdu_ref.wrapping_add(1).max(1);
            timeout(self.cfg.timeout(), self.send_raw(&pkt))
                .await
                .map_err(|_| {
                    SdkDriverError::new(ErrorKind::Timeout, "READ_TIMEOUT", "Bulk Read 请求超时")
                })?
                .map_err(|e| {
                    SdkDriverError::new(ErrorKind::Connection, "READ_SEND_FAIL", e.to_string())
                })?;
            let resp = timeout(self.cfg.timeout(), self.recv_packet())
                .await
                .map_err(|_| {
                    SdkDriverError::new(ErrorKind::Timeout, "READ_TIMEOUT", "Bulk Read 响应超时")
                })?
                .map_err(|e| {
                    SdkDriverError::new(ErrorKind::Connection, "READ_RECV_FAIL", e.to_string())
                })?;
            let part = parse_bulk_resp(&resp, chunk)?;
            all.extend(part);
            start = end;
        }
        Ok(all)
    }

    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(data).await?;
        self.stream.flush().await
    }

    /// 读取一个完整 TPKT 包（ISO 8073）。
    async fn recv_packet(&mut self) -> std::io::Result<Vec<u8>> {
        let mut hdr = [0u8; 4];
        self.stream.read_exact(&mut hdr).await?;
        if hdr[0] != TPKT_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("TPKT 版本异常 {:02x}，期望 {:02x}", hdr[0], TPKT_VERSION),
            ));
        }
        let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
        if !(TPKT_MIN_LEN..=TPKT_MAX_LEN).contains(&len) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "TPKT 长度非法 {}，允许 {}..{}",
                    len, TPKT_MIN_LEN, TPKT_MAX_LEN
                ),
            ));
        }
        let mut buf = vec![0u8; len];
        buf[0..4].copy_from_slice(&hdr);
        self.stream.read_exact(&mut buf[4..]).await?;
        Ok(buf)
    }
}

fn build_cotp_cr(src: u16, dst: u16) -> Vec<u8> {
    // COTP CR：LI 0x11 / PDU type CR 0xE0 / TPDU size 0x0A=1024 / src/dst TSAP
    let mut cotp = vec![
        0x11,
        COTP_CR,
        0x00,
        0x00,
        0x00,
        0x01,
        0x00,
        0xC0,
        0x01,
        0x0A,
        0xC1,
        0x02,
        ((src >> 8) as u8),
        (src as u8),
        0xC2,
        0x02,
        ((dst >> 8) as u8),
        (dst as u8),
    ];
    let tpkt_len = (4 + cotp.len()) as u16;
    let mut pkt = vec![
        TPKT_VERSION,
        0x00,
        (tpkt_len >> 8) as u8,
        (tpkt_len & 0xFF) as u8,
    ];
    pkt.append(&mut cotp);
    pkt
}

fn build_s7_setup(pdu_ref: u16, pdu_len: u16) -> Vec<u8> {
    // S7 Setup：固定头 0x32 / ROSCTR Job 0x01 / 保留 / PDU ref / param len 8 / data len 0 / F0 功能组
    let mut s7 = Vec::with_capacity(20);
    s7.extend_from_slice(&[0x32, S7_ROSCTR_JOB, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x08, 0x00, 0x00]);
    s7.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01]);
    s7.extend_from_slice(&pdu_len.to_be_bytes());
    // COTP Data
    let cotp = [0x02, COTP_DT, 0x80];
    let tpkt_len = (4 + cotp.len() + s7.len()) as u16;
    let mut pkt = vec![
        TPKT_VERSION,
        0x00,
        (tpkt_len >> 8) as u8,
        (tpkt_len & 0xFF) as u8,
    ];
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(&s7);
    pkt
}

fn build_read_req(pdu_ref: u16, items: &[ReadItem]) -> Vec<u8> {
    // S7 ReadVar 参数：功能码 0x04 + item数 + N×12字节 ANY 结构（0x12 0x0A 0x10 + transport + len + db + area + bit_addr）
    let mut param = Vec::with_capacity(2 + items.len() * 12);
    param.push(S7_FUNC_READ);
    param.push(items.len() as u8);
    for it in items {
        let area = it.addr.area.code();
        let db = it.addr.db_number;
        let bit_addr = it.addr.bit_address();
        // Common 修补：C/T 计数器/定时器为字寻址但 R/W 计数单位为“个数”而非字节，WORD 通用 2 会触发 0x06
        let transport = match it.addr.area {
            crate::address::Area::Counter => 0x1C,
            crate::address::Area::Timer => 0x1D,
            _ => it.kind.transport_size(),
        };
        let req_len = match it.addr.area {
            crate::address::Area::Counter | crate::address::Area::Timer => 1,
            _ => it.kind.request_len(),
        };
        param.extend_from_slice(&[S7_VAR_SPEC, S7_VAR_SPEC_LEN, S7_SYNTAX_ID_S7ANY, transport]);
        param.extend_from_slice(&req_len.to_be_bytes());
        param.extend_from_slice(&db.to_be_bytes());
        param.push(area);
        param.push(((bit_addr >> 16) & 0xFF) as u8);
        param.push(((bit_addr >> 8) & 0xFF) as u8);
        param.push((bit_addr & 0xFF) as u8);
    }
    let param_len = param.len() as u16;
    let mut s7 = Vec::with_capacity(12 + param.len());
    s7.extend_from_slice(&[0x32, S7_ROSCTR_JOB, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&param_len.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x00]);
    s7.extend_from_slice(&param);
    let cotp = [0x02, COTP_DT, 0x80];
    let tpkt_len = (4 + cotp.len() + s7.len()) as u16;
    let mut pkt = vec![
        TPKT_VERSION,
        0x00,
        (tpkt_len >> 8) as u8,
        (tpkt_len & 0xFF) as u8,
    ];
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(&s7);
    pkt
}

fn build_bulk_read_req(pdu_ref: u16, ranges: &[(S7Address, usize)]) -> Vec<u8> {
    // 连续区合并后：每 range 作为一次 BYTE 批量读，transport 固定 BYTE(0x02)，length 为 range 字节数
    let mut param = Vec::with_capacity(2 + ranges.len() * 12);
    param.push(S7_FUNC_READ);
    param.push(ranges.len() as u8);
    for (addr, len) in ranges {
        let area = addr.area.code();
        let db = addr.db_number;
        let bit_addr = addr.bit_address();
        let transport = match addr.area {
            crate::address::Area::Counter => 0x1C,
            crate::address::Area::Timer => 0x1D,
            _ => 0x02, // BYTE 批量
        };
        let req_len = match addr.area {
            crate::address::Area::Counter | crate::address::Area::Timer => 1,
            _ => *len as u16,
        };
        param.extend_from_slice(&[S7_VAR_SPEC, S7_VAR_SPEC_LEN, S7_SYNTAX_ID_S7ANY, transport]);
        param.extend_from_slice(&req_len.to_be_bytes());
        param.extend_from_slice(&db.to_be_bytes());
        param.push(area);
        param.push(((bit_addr >> 16) & 0xFF) as u8);
        param.push(((bit_addr >> 8) & 0xFF) as u8);
        param.push((bit_addr & 0xFF) as u8);
    }
    let param_len = param.len() as u16;
    let mut s7 = Vec::with_capacity(12 + param.len());
    s7.extend_from_slice(&[0x32, S7_ROSCTR_JOB, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&param_len.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x00]);
    s7.extend_from_slice(&param);
    let cotp = [0x02, COTP_DT, 0x80];
    let tpkt_len = (4 + cotp.len() + s7.len()) as u16;
    let mut pkt = vec![
        TPKT_VERSION,
        0x00,
        (tpkt_len >> 8) as u8,
        (tpkt_len & 0xFF) as u8,
    ];
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(&s7);
    pkt
}

fn parse_bulk_resp(
    resp: &[u8],
    ranges: &[(S7Address, usize)],
) -> Result<Vec<Option<Vec<u8>>>, SdkDriverError> {
    if resp.len() < 7 + 12 {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "READ_SHORT",
            format!("Bulk 响应过短 {}", resp.len()),
        ));
    }
    let s7 = &resp[7..];
    if s7.len() < 12 {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "S7_SHORT",
            "S7 头部缺失",
        ));
    }
    let rosctr = s7[1];
    if rosctr != S7_ROSCTR_ACK {
        let err_class = s7.get(17).copied().unwrap_or(0);
        return Err(map_s7_error(
            err_class,
            &format!("Bulk Read 被拒绝 rosctr={:02x}", rosctr),
        ));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    if s7.len() < 12 + param_len + data_len {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "S7_LEN_MISMATCH",
            "S7 长度与实际不符",
        ));
    }
    let data = &s7[12 + param_len..12 + param_len + data_len];
    let mut out = Vec::with_capacity(ranges.len());
    let mut off = 0;
    for (idx, (_, expect_len)) in ranges.iter().enumerate() {
        if off + 4 > data.len() {
            return Err(SdkDriverError::new(
                ErrorKind::Protocol,
                "READ_ITEM_SHORT",
                format!("bulk {idx} 头部缺失"),
            ));
        }
        let ret = data[off];
        let _transport = data[off + 1];
        let len_bits = u16::from_be_bytes([data[off + 2], data[off + 3]]) as usize;
        off += 4;
        if ret != S7_ITEM_OK {
            tracing::warn!(idx, ret, "Bulk item BAD");
            let byte_len = len_bits.div_ceil(8);
            if byte_len > 0 && off + byte_len <= data.len() {
                off += byte_len;
                if byte_len % 2 == 1 && off < data.len() && idx + 1 < ranges.len() {
                    // 字对齐填充
                    if data[off] == 0x00 {
                        off += 1;
                    }
                }
            }
            out.push(None);
            continue;
        }
        let byte_len = len_bits.div_ceil(8);
        // 期望长度与实际返回长度取小，避免 STRING 等短回
        let take = (*expect_len).min(byte_len).max(1);
        if off + take > data.len() {
            return Err(SdkDriverError::new(
                ErrorKind::Protocol,
                "READ_DATA_SHORT",
                format!("bulk {idx} 数据缺失"),
            ));
        }
        out.push(Some(data[off..off + take].to_vec()));
        off += byte_len;
        if byte_len % 2 == 1 && off < data.len() && idx + 1 < ranges.len() && data[off] == 0x00 {
            off += 1;
        }
    }
    Ok(out)
}

fn parse_read_resp(
    resp: &[u8],
    sent_items: &[ReadItem],
) -> Result<Vec<Option<Vec<u8>>>, SdkDriverError> {
    if resp.len() < 7 + 12 {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "READ_SHORT",
            format!("响应过短 {}", resp.len()),
        ));
    }
    let s7 = &resp[7..];
    if s7.len() < 12 {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "S7_SHORT",
            "S7 头部缺失",
        ));
    }
    let rosctr = s7[1];
    if rosctr != S7_ROSCTR_ACK {
        let err_class = s7.get(17).copied().unwrap_or(0);
        return Err(map_s7_error(
            err_class,
            &format!(
                "Read 被拒绝 rosctr={:02x} 期望 {:02x}",
                rosctr, S7_ROSCTR_ACK
            ),
        ));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    if s7.len() < 12 + param_len + data_len {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "S7_LEN_MISMATCH",
            "S7 长度与实际不符",
        ));
    }
    let data = &s7[12 + param_len..12 + param_len + data_len];
    // param 预期 0x04 00? 对于成功读取，param 首字节 0x04，次字节 item 数
    // data 结构：每 item 4 字节头 + 数据（若奇数长度则填充 1 字节）— 按项 BAD 隔离
    let mut out: Vec<Option<Vec<u8>>> = Vec::with_capacity(sent_items.len());
    let mut off = 0;
    for (idx, it) in sent_items.iter().enumerate() {
        if off + 4 > data.len() {
            return Err(SdkDriverError::new(
                ErrorKind::Protocol,
                "READ_ITEM_SHORT",
                format!("item {idx} 头部缺失"),
            ));
        }
        let ret = data[off];
        let transport = data[off + 1];
        let len_bits = u16::from_be_bytes([data[off + 2], data[off + 3]]) as usize;
        off += 4;
        if ret != S7_ITEM_OK {
            // 按项 BAD：记录日志但不整体失败，调用方将该点发 BAD
            tracing::warn!(addr=%format_addr(&it.addr), ret=ret, "S7 item 0x{:02X} 按项 BAD", ret);
            // 仍需跳过该 item 的数据区（若有）以对齐下一项
            let byte_len = len_bits.div_ceil(8);
            // S7 在错误时 len_bits 可能为 0，仍按 0 处理，不消费数据
            if byte_len > 0 && off + byte_len <= data.len() {
                off += byte_len;
                if byte_len % 2 == 1
                    && off < data.len()
                    && data[off] == 0x00
                    && idx + 1 < sent_items.len()
                {
                    off += 1;
                }
            }
            out.push(None);
            continue;
        }
        // S7 返回长度单位为 bit，需转字节；BIT 固定 1 bit→1 byte
        let byte_len = if transport == S7_TRANSPORT_BIT || it.kind == S7Kind::Bool {
            1
        } else {
            len_bits.div_ceil(8)
        };
        // 但为防御实现差异，回退到 kind 预期长度
        let expect = it.kind.byte_len();
        let take = byte_len.min(expect).max(1);
        if off + take > data.len() {
            return Err(SdkDriverError::new(
                ErrorKind::Protocol,
                "READ_DATA_SHORT",
                format!("item {idx} 数据缺失"),
            ));
        }
        let mut bytes = data[off..off + take].to_vec();
        // 对于 BIT，S7 返回的当字节最低位为值，需保留 0/1 映射
        if it.kind == S7Kind::Bool {
            // snap7 行为：BIT 读取返回 1 字节，值为 0x00/0x01
            bytes = vec![if bytes[0] != 0 { 1 } else { 0 }];
        }
        out.push(Some(bytes));
        off += take;
        // S7 协议字对齐：奇数长度 payload 后补 1 字节 0x00，解析时需跳过否则下一 item 头 0xFF 错位；
        // 为什么用启发式：实测 DB 20 字节偶数不触发、1 字节触发，需兼顾 BIT(1字节)与 BYTE 混批场景
        if take % 2 == 1 && off < data.len() {
            // 填充字节为 0，若下一个 item 头部恰为 0xFF 则不是填充
            // 仅当剩余字节足够且下一字节不是 0xFF 才跳过
            if data.len() - off >= 1 && off + 1 < data.len() && data[off] == 0x00 {
                // 预测下一个头部 ret 应为 0xFF，若下一字节是 0xFF 则不是填充；此处保守处理：若下一字节是 0xFF 且再下一字节 transport 合理，则为新头部而非填充
                // 简化：若长度为奇数且下一个字节为 0x00 填充则跳过
                // 实测读 20 字节偶数不会触发；读 1 字节会触发填充
                if off + 4 <= data.len() && data[off] == 0x00 && data[off + 1] != 0x04 {
                    // 可能是填充，跳过
                    off += 1;
                } else if take % 2 == 1 {
                    // 默认跳过填充字节
                    if data[off] == 0x00 {
                        // 只有在确实是填充时才跳过；为了不误判，仅当剩余 items 预期头部为 0xFF 时才认为是填充
                        // 这里简单：若还有剩余 items 且 off 字节为 0 且下一 item 的预期返回码位置对齐，则跳过
                        if idx + 1 < sent_items.len() {
                            off += 1;
                        }
                    }
                }
            }
        }
        // 若 S7 返回的长度大于预期（STRING 场景），截断已由 take 控制
    }
    Ok(out)
}

/// SZL 读取（Common 诊断）：S7 功能 0x07 SZL，返回原始 SZL 负载（已去 TPKT/COTP/S7 头）。
/// 用于 `SZL 0x0011` CPU 诊断、`0x0131` 模块标识等只读诊断，不走点位批次。
impl S7Client {
    pub async fn read_szl(
        &mut self,
        szl_id: u16,
        szl_index: u16,
    ) -> Result<Vec<u8>, SdkDriverError> {
        let pkt = build_szl_req(self.pdu_ref, szl_id, szl_index);
        self.pdu_ref = self.pdu_ref.wrapping_add(1).max(1);
        timeout(self.cfg.timeout(), self.send_raw(&pkt))
            .await
            .map_err(|_| {
                SdkDriverError::new(
                    ErrorKind::Timeout,
                    "SZL_TIMEOUT",
                    format!("SZL 0x{szl_id:04X} 请求超时"),
                )
            })?
            .map_err(|e| {
                SdkDriverError::new(ErrorKind::Connection, "SZL_SEND_FAIL", e.to_string())
            })?;
        let resp = timeout(self.cfg.timeout(), self.recv_packet())
            .await
            .map_err(|_| {
                SdkDriverError::new(
                    ErrorKind::Timeout,
                    "SZL_TIMEOUT",
                    format!("SZL 0x{szl_id:04X} 响应超时"),
                )
            })?
            .map_err(|e| {
                SdkDriverError::new(ErrorKind::Connection, "SZL_RECV_FAIL", e.to_string())
            })?;
        parse_szl_resp(&resp, szl_id)
    }
}

fn build_szl_req(pdu_ref: u16, szl_id: u16, szl_index: u16) -> Vec<u8> {
    // S7 SZL (0x07 UserData) 请求模板，基于 snap7 抓包提炼：
    // TPKT 4 + COTP 3 + S7 32 07 00 00 + pdu_ref + 00 08/00 04 + UserData 固定 + SZL ID/Index
    let mut s7 = Vec::with_capacity(32);
    s7.extend_from_slice(&[0x32, 0x07, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x08, 0x00, 0x04]);
    // UserData 头：0x00 0x01 0x12 0x04 0x11 0x44 0x01 0x00 0xFF 0x09 0x00 0x04
    s7.extend_from_slice(&[
        0x00, 0x01, 0x12, 0x04, 0x11, 0x44, 0x01, 0x00, 0xFF, 0x09, 0x00, 0x04,
    ]);
    s7.extend_from_slice(&szl_id.to_be_bytes());
    s7.extend_from_slice(&szl_index.to_be_bytes());
    let cotp = [0x02, COTP_DT, 0x80];
    let tpkt_len = (4 + cotp.len() + s7.len()) as u16;
    let mut pkt = vec![
        TPKT_VERSION,
        0x00,
        (tpkt_len >> 8) as u8,
        (tpkt_len & 0xFF) as u8,
    ];
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(&s7);
    pkt
}

fn parse_szl_resp(resp: &[u8], szl_id: u16) -> Result<Vec<u8>, SdkDriverError> {
    if resp.len() < 7 + 12 {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "SZL_SHORT",
            format!("SZL 0x{szl_id:04X} 响应过短 {}", resp.len()),
        ));
    }
    let s7 = &resp[7..];
    if s7.len() < 12 {
        return Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "SZL_S7_SHORT",
            "SZL S7 头缺失",
        ));
    }
    if s7[1] != S7_ROSCTR_ACK && s7[1] != 0x07 {
        return Err(map_s7_error(s7[17], &format!("SZL 0x{szl_id:04X} 被拒绝")));
    }
    // 剩余为 UserData，透传给上层诊断做二次解析
    Ok(s7[12..].to_vec())
}

fn format_addr(a: &S7Address) -> String {
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

fn map_connect_error(e: std::io::Error, addr: &str, cfg: &S7ConnConfig) -> SdkDriverError {
    let kind = e.kind();
    let hint = match kind {
        std::io::ErrorKind::ConnectionRefused => {
            format!("连接被拒绝 {}（PLC 未开机/端口 {} 未开放）", addr, cfg.port)
        }
        std::io::ErrorKind::TimedOut => format!("连接超时 {}（网络不可达或 PLC 无响应）", addr),
        _ => format!("TCP 连接失败 {}: {e}", addr),
    };
    SdkDriverError::new(ErrorKind::Connection, "CONNECT_FAIL", hint)
}

fn map_s7_error(code: u8, ctx: &str) -> SdkDriverError {
    let (kind, help) = match code {
        S7_ERR_CONTEXT => (
            ErrorKind::Configuration,
            "（0x04 上下文不支持）S7-1200/1500 请在 TIA Portal 硬件组态 CPU 属性 -> 防护与安全 -> 连接机制 中勾选“允许来自远程对象的 PUT/GET 通信访问”",
        ),
        S7_ERR_ADDRESS => (
            ErrorKind::Address,
            "（0x05 地址错误）检查 DB 是否存在且为标准访问（非优化块），地址是否越界",
        ),
        S7_ERR_ACCESS => (
            ErrorKind::Configuration,
            "（0x03 拒绝）CPU 保护等级或安全策略禁止外部访问",
        ),
        _ => (ErrorKind::Protocol, ""),
    };
    SdkDriverError::new(
        kind,
        format!("S7_0x{code:02X}"),
        format!("{ctx}: S7 错误 0x{code:02X} {help}"),
    )
}
