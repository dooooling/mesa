//! S7 ISO-on-TCP 最小客户端（方案 §7.1）。
//!
//! 实现足够通过 127.0.0.1:102 动态 DB10 采集的路径：
//! `TCP -> COTP CR/CC -> S7 Setup -> ReadVar`（支持多 item）。
//! 仅依赖 `tokio` 与 `bytes`，不引入额外 snap7 绑定。

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

use crate::address::{Area, S7Address};
use crate::codec::S7Kind;
use forgelink_driver_sdk::SdkDriverError;
use forgelink_core_types::ErrorKind;

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
        Self { host: "127.0.0.1".into(), port: 102, rack: 0, slot: 1, timeout_ms: 3000, pdu_length: 480 }
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
                return Err(SdkDriverError::configuration("BAD_CONFIG", format!("port {} 非法", p)));
            }
            cfg.port = p as u16;
        }
        if let Some(r) = v.get("rack").and_then(|x| x.as_u64()) {
            if r > 7 { return Err(SdkDriverError::configuration("BAD_CONFIG", format!("rack {} 非法", r))); }
            cfg.rack = r as u8;
        }
        if let Some(s) = v.get("slot").and_then(|x| x.as_u64()) {
            if s > 31 { return Err(SdkDriverError::configuration("BAD_CONFIG", format!("slot {} 非法", s))); }
            cfg.slot = s as u8;
        }
        if let Some(t) = v.get("timeout_ms").and_then(|x| x.as_u64()) {
            cfg.timeout_ms = t.max(500);
        }
        if let Some(pdu) = v.get("pdu_length").and_then(|x| x.as_u64()) {
            cfg.pdu_length = (pdu as u16).clamp(240, 960);
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

    fn timeout(&self) -> Duration { Duration::from_millis(self.timeout_ms) }
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
        let stream = timeout(cfg.timeout(), TcpStream::connect(&addr)).await
            .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "CONNECT_TIMEOUT", format!("连接 {addr} 超时（{}ms），检查 PLC 是否可达、端口 102 是否放通", cfg.timeout_ms)))?
            .map_err(|e| map_connect_error(e, &addr, &cfg))?;

        let mut client = Self { stream, pdu_ref: 1, pdu_length: cfg.pdu_length, cfg };
        client.iso_connect().await?;
        client.s7_setup().await?;
        tracing::info!(host=%client.cfg.host, port=client.cfg.port, rack=client.cfg.rack, slot=client.cfg.slot, pdu=client.pdu_length, "S7 连接建立");
        Ok(client)
    }

    async fn iso_connect(&mut self) -> Result<(), SdkDriverError> {
        let src_tsap = 0x0100u16;
        // 经典推导：dst = 0x0100 | (rack<<5 | slot)，与 snap7 兼容
        let dst_tsap = 0x0100u16 | ((self.cfg.rack as u16) << 5) | (self.cfg.slot as u16);
        let pkt = build_cotp_cr(src_tsap, dst_tsap);
        timeout(self.cfg.timeout(), self.send_raw(&pkt)).await
            .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "COTP_TIMEOUT", "COTP CR 超时"))?
            .map_err(|e| SdkDriverError::new(ErrorKind::Connection, "COTP_SEND_FAIL", e.to_string()))?;
        let resp = timeout(self.cfg.timeout(), self.recv_packet()).await
            .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "COTP_TIMEOUT", "COTP CC 超时"))?
            .map_err(|e| SdkDriverError::new(ErrorKind::Connection, "COTP_RECV_FAIL", e.to_string()))?;
        // COTP CC PDU type 0xD0
        if resp.len() < 6 || resp[5] != 0xD0 {
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
        timeout(self.cfg.timeout(), self.send_raw(&pkt)).await
            .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "S7_SETUP_TIMEOUT", "S7 Setup 超时"))?
            .map_err(|e| SdkDriverError::new(ErrorKind::Connection, "S7_SETUP_SEND_FAIL", e.to_string()))?;
        let resp = timeout(self.cfg.timeout(), self.recv_packet()).await
            .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "S7_SETUP_TIMEOUT", "S7 Setup 响应超时"))?
            .map_err(|e| SdkDriverError::new(ErrorKind::Connection, "S7_SETUP_RECV_FAIL", e.to_string()))?;
        // 解析 S7 payload
        let payload = &resp[7..]; // 跳过 TPKT 4 + COTP 3
        if payload.len() < 12 {
            return Err(SdkDriverError::new(ErrorKind::Protocol, "S7_SETUP_SHORT", format!("Setup 响应过短 {}", payload.len())));
        }
        // rosctr 0x03 = Ack
        if payload[1] != 0x03 {
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

    /// 批量读取。返回与 items 等长的原始字节向量（按 S7Kind 截好）。任一 item 的 S7 返回码非 0xFF 即视为错误并返回整体失败（由调用方决定重连或标记 BAD）。
    pub async fn read_vars(&mut self, items: &[ReadItem]) -> Result<Vec<Vec<u8>>, SdkDriverError> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        // S7 PDU 对单次 item 数的限制约为 19，超出需分批
        let mut all: Vec<Vec<u8>> = Vec::with_capacity(items.len());
        for chunk in items.chunks(19) {
            let pkt = build_read_req(self.pdu_ref, chunk);
            self.pdu_ref = self.pdu_ref.wrapping_add(1).max(1);
            timeout(self.cfg.timeout(), self.send_raw(&pkt)).await
                .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "READ_TIMEOUT", "Read 请求超时"))?
                .map_err(|e| SdkDriverError::new(ErrorKind::Connection, "READ_SEND_FAIL", e.to_string()))?;
            let resp = timeout(self.cfg.timeout(), self.recv_packet()).await
                .map_err(|_| SdkDriverError::new(ErrorKind::Timeout, "READ_TIMEOUT", "Read 响应超时"))?
                .map_err(|e| SdkDriverError::new(ErrorKind::Connection, "READ_RECV_FAIL", e.to_string()))?;
            let mut part = parse_read_resp(&resp, chunk)?;
            all.append(&mut part);
        }
        Ok(all)
    }

    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(data).await?;
        self.stream.flush().await
    }

    /// 读取一个完整 TPKT 包。
    async fn recv_packet(&mut self) -> std::io::Result<Vec<u8>> {
        let mut hdr = [0u8; 4];
        self.stream.read_exact(&mut hdr).await?;
        if hdr[0] != 0x03 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("TPKT 版本异常 {:02x}", hdr[0])));
        }
        let len = u16::from_be_bytes([hdr[2], hdr[3]]) as usize;
        if len < 4 || len > 8192 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("TPKT 长度非法 {}", len)));
        }
        let mut buf = vec![0u8; len];
        buf[0..4].copy_from_slice(&hdr);
        self.stream.read_exact(&mut buf[4..]).await?;
        Ok(buf)
    }
}

fn build_cotp_cr(src: u16, dst: u16) -> Vec<u8> {
    // 固定 COTP CR：TPDU size 0x0A (1024)，src/dst TSAP
    let mut cotp = vec![
        0x11, 0xE0, 0x00, 0x00, 0x00, 0x01, 0x00,
        0xC0, 0x01, 0x0A,
        0xC1, 0x02, ((src >> 8) as u8), (src as u8),
        0xC2, 0x02, ((dst >> 8) as u8), (dst as u8),
    ];
    let tpkt_len = (4 + cotp.len()) as u16;
    let mut pkt = vec![0x03, 0x00, (tpkt_len >> 8) as u8, (tpkt_len & 0xFF) as u8];
    pkt.append(&mut cotp);
    pkt
}

fn build_s7_setup(pdu_ref: u16, pdu_len: u16) -> Vec<u8> {
    // S7: header 12 + param 8
    let mut s7 = Vec::with_capacity(20);
    s7.extend_from_slice(&[0x32, 0x01, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x08, 0x00, 0x00]); // param len 8, data len 0
    s7.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01]);
    s7.extend_from_slice(&pdu_len.to_be_bytes());
    // COTP DT
    let cotp = [0x02, 0xF0, 0x80];
    let tpkt_len = (4 + cotp.len() + s7.len()) as u16;
    let mut pkt = vec![0x03, 0x00, (tpkt_len >> 8) as u8, (tpkt_len & 0xFF) as u8];
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(&s7);
    pkt
}

fn build_read_req(pdu_ref: u16, items: &[ReadItem]) -> Vec<u8> {
    // Param: 0x04 + num_items + N * 12 byte item
    let mut param = Vec::with_capacity(2 + items.len() * 12);
    param.push(0x04);
    param.push(items.len() as u8);
    for it in items {
        let area = it.addr.area.code();
        let db = it.addr.db_number;
        let bit_addr = it.addr.bit_address();
        let transport = it.kind.transport_size();
        let req_len = it.kind.request_len();
        param.extend_from_slice(&[0x12, 0x0A, 0x10, transport]);
        param.extend_from_slice(&req_len.to_be_bytes());
        param.extend_from_slice(&db.to_be_bytes());
        param.push(area);
        param.push(((bit_addr >> 16) & 0xFF) as u8);
        param.push(((bit_addr >> 8) & 0xFF) as u8);
        param.push((bit_addr & 0xFF) as u8);
    }
    let param_len = param.len() as u16;
    let mut s7 = Vec::with_capacity(12 + param.len());
    s7.extend_from_slice(&[0x32, 0x01, 0x00, 0x00]);
    s7.extend_from_slice(&pdu_ref.to_be_bytes());
    s7.extend_from_slice(&param_len.to_be_bytes());
    s7.extend_from_slice(&[0x00, 0x00]);
    s7.extend_from_slice(&param);
    let cotp = [0x02, 0xF0, 0x80];
    let tpkt_len = (4 + cotp.len() + s7.len()) as u16;
    let mut pkt = vec![0x03, 0x00, (tpkt_len >> 8) as u8, (tpkt_len & 0xFF) as u8];
    pkt.extend_from_slice(&cotp);
    pkt.extend_from_slice(&s7);
    pkt
}

fn parse_read_resp(resp: &[u8], sent_items: &[ReadItem]) -> Result<Vec<Vec<u8>>, SdkDriverError> {
    if resp.len() < 7 + 12 {
        return Err(SdkDriverError::new(ErrorKind::Protocol, "READ_SHORT", format!("响应过短 {}", resp.len())));
    }
    let s7 = &resp[7..];
    if s7.len() < 12 {
        return Err(SdkDriverError::new(ErrorKind::Protocol, "S7_SHORT", "S7 头部缺失"));
    }
    let rosctr = s7[1];
    if rosctr != 0x03 {
        // 0x02 = Ack Data should also be 0x03 for read
        // 检查 S7 错误类
        let err_class = s7.get(17).copied().unwrap_or(0);
        return Err(map_s7_error(err_class, &format!("Read 被拒绝 rosctr={:02x}", rosctr)));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    if s7.len() < 12 + param_len + data_len {
        return Err(SdkDriverError::new(ErrorKind::Protocol, "S7_LEN_MISMATCH", "S7 长度与实际不符"));
    }
    let data = &s7[12 + param_len.. 12 + param_len + data_len];
    // param 预期 0x04 00? 对于成功读取，param 首字节 0x04，次字节 item 数
    // data 结构：每 item 4 字节头 + 数据（若奇数长度则填充 1 字节）
    let mut out = Vec::with_capacity(sent_items.len());
    let mut off = 0;
    for (idx, it) in sent_items.iter().enumerate() {
        if off + 4 > data.len() {
            return Err(SdkDriverError::new(ErrorKind::Protocol, "READ_ITEM_SHORT", format!("item {idx} 头部缺失")));
        }
        let ret = data[off];
        let transport = data[off + 1];
        let len_bits = u16::from_be_bytes([data[off + 2], data[off + 3]]) as usize;
        off += 4;
        if ret != 0xFF {
            let msg = match ret {
                0x05 => format!("地址不存在或越界（S7 0x05）——检查 DB{} 是否为标准访问、地址 {} 是否正确", it.addr.db_number, format_addr(&it.addr)),
                0x04 => "上下文不支持（S7 0x04）——S7-1200/1500 请在 PLC 硬件组态中启用 PUT/GET 通信访问许可".to_string(),
                0x03 => "拒绝访问（S7 0x03）——CPU 保护/安全等级禁止外部读".to_string(),
                0x06 => "数据类型不匹配（S7 0x06）".to_string(),
                _ => format!("S7 item 错误 0x{ret:02x}"),
            };
            let kind = match ret {
                0x05 => ErrorKind::Address,
                0x04 => ErrorKind::Configuration,
                0x03 => ErrorKind::Configuration,
                _ => ErrorKind::Device,
            };
            return Err(SdkDriverError::new(kind, format!("S7_ITEM_0x{ret:02X}"), format!("{msg}（data_type 期望 {kind:?}）")));
        }
        // 数据字节数：若 transport 表明 BIT，则长度为 bits；BYTE 时长度亦为 bits（需 /8）
        let byte_len = if transport == 0x03 || it.kind == S7Kind::Bool {
            // BIT: 1 bit => 1 byte
            1
        } else {
            (len_bits + 7) / 8
        };
        // 但为防御实现差异，回退到 kind 预期长度
        let expect = it.kind.byte_len();
        let take = byte_len.min(expect).max(1);
        if off + take > data.len() {
            return Err(SdkDriverError::new(ErrorKind::Protocol, "READ_DATA_SHORT", format!("item {idx} 数据缺失")));
        }
        let mut bytes = data[off..off + take].to_vec();
        // 对于 BIT，S7 返回的当字节最低位为值，需保留 0/1 映射
        if it.kind == S7Kind::Bool {
            // snap7 行为：BIT 读取返回 1 字节，值为 0x00/0x01
            bytes = vec![if bytes[0] != 0 { 1 } else { 0 }];
        }
        out.push(bytes);
        off += take;
        // S7 对奇数长度填充对齐到偶数
        if take % 2 == 1 && off < data.len() {
            // 填充字节为 0，若下一个 item 头部恰为 0xFF 则不是填充
            // 仅当剩余字节足够且下一字节不是 0xFF 才跳过
            if data.len() - off >= 1 && off + 1 < data.len() && data[off] == 0x00 {
                // 预测下一个头部 ret 应为 0xFF，若下一字节是 0xFF 则不是填充；此处保守处理：若下一字节是 0xFF 且再下一字节 transport 合理，则为新头部而非填充
                // 简化：若长度为奇数且下一个字节为 0x00 填充则跳过
                // 实测读 20 字节偶数不会触发；读 1 字节会触发填充
                if off + 4 <= data.len() && data[off] == 0x00 && data[off+1] != 0x04 {
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
            if let Some(bit) = a.bit_offset { format!("M{}.{}", a.byte_offset, bit) } else { format!("MB{}", a.byte_offset) }
        }
        Area::Input => {
            if let Some(bit) = a.bit_offset { format!("I{}.{}", a.byte_offset, bit) } else { format!("IB{}", a.byte_offset) }
        }
        Area::Output => {
            if let Some(bit) = a.bit_offset { format!("Q{}.{}", a.byte_offset, bit) } else { format!("QB{}", a.byte_offset) }
        }
    }
}

fn map_connect_error(e: std::io::Error, addr: &str, cfg: &S7ConnConfig) -> SdkDriverError {
    let kind = e.kind();
    let hint = match kind {
        std::io::ErrorKind::ConnectionRefused => format!("连接被拒绝 {}（PLC 未开机/端口 {} 未开放）", addr, cfg.port),
        std::io::ErrorKind::TimedOut => format!("连接超时 {}（网络不可达或 PLC 无响应）", addr),
        _ => format!("TCP 连接失败 {}: {e}", addr),
    };
    SdkDriverError::new(ErrorKind::Connection, "CONNECT_FAIL", hint)
}

fn map_s7_error(code: u8, ctx: &str) -> SdkDriverError {
    let (kind, help) = match code {
        0x04 => (ErrorKind::Configuration, "（0x04 上下文不支持）S7-1200/1500 请在 TIA Portal 硬件组态 CPU 属性 -> 防护与安全 -> 连接机制 中勾选“允许来自远程对象的 PUT/GET 通信访问”"),
        0x05 => (ErrorKind::Address, "（0x05 地址错误）检查 DB 是否存在且为标准访问（非优化块），地址是否越界"),
        0x03 => (ErrorKind::Configuration, "（0x03 拒绝）CPU 保护等级或安全策略禁止外部访问"),
        _ => (ErrorKind::Protocol, ""),
    };
    SdkDriverError::new(kind, format!("S7_0x{code:02X}"), format!("{ctx}: S7 错误 0x{code:02X} {help}"))
}
