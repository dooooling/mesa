//! S7Comm 会话：`TCP -> COTP CR/CC -> S7 Setup -> ReadVar/WriteVar/SZL`。
//!
//! 会话拥有 TCP 流与 PDU 引用号，按协商 PDU 自动分片；变量规范的编码
//! 与解码由调用方负责（S7ANY 见 `s7` Driver，NCK 见 `sinumerik-nck` Driver）。
//! 超时与诊断口径与抽取前 `S7Client` 一致。

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::config::S7ConnectOptions;
use crate::cotp::{build_cotp_cr, check_cc};
use crate::error::{S7TransportError, map_connect_error};
use crate::pdu::{build_s7_setup, parse_setup_ack};
use crate::read_var::{
    S7ReadVarItem, S7ReadVarResult, build_read_request, parse_bulk_response, parse_read_response,
    plan_chunks,
};
use crate::szl::{build_szl_request, parse_szl_response};
use crate::tpkt::{map_io_error, recv_packet, send_packet};
use crate::write_var::{build_write_request, parse_write_response};

/// S7Comm 会话：已建立 ISO/S7 握手的连接。
pub struct S7Session {
    stream: TcpStream,
    pdu_ref: u16,
    negotiated_pdu: u16,
    options: S7ConnectOptions,
}

impl S7Session {
    /// 协商后的 PDU（后续分片以上限为准）。
    pub fn negotiated_pdu_length(&self) -> u16 {
        self.negotiated_pdu
    }

    /// 连接选项回显（诊断用）。
    pub fn options(&self) -> &S7ConnectOptions {
        &self.options
    }

    /// 建立连接并完成 `COTP + S7 Setup` 握手。
    pub async fn connect(options: S7ConnectOptions) -> Result<Self, S7TransportError> {
        let addr = options.dial_addr();
        let stream = timeout(options.timeout(), TcpStream::connect(&addr))
            .await
            .map_err(|_| {
                S7TransportError::timeout(
                    "CONNECT_TIMEOUT",
                    format!(
                        "连接 {addr} 超时（{}ms），检查 PLC 是否可达、端口 102 是否放通",
                        options.timeout_ms
                    ),
                )
            })?
            .map_err(|e| map_connect_error(e, &addr, options.port))?;

        let mut session = Self {
            stream,
            pdu_ref: 1,
            negotiated_pdu: options.requested_pdu_length,
            options,
        };
        session.iso_connect().await?;
        session.s7_setup().await?;
        tracing::info!(
            host = %session.options.host,
            port = session.options.port,
            pdu = session.negotiated_pdu,
            "S7 会话建立"
        );
        Ok(session)
    }

    /// 显式关闭（TCP 流随 drop 关闭；提供方法以便调用方表达意图）。
    pub async fn disconnect(self) -> Result<(), S7TransportError> {
        drop(self);
        Ok(())
    }

    async fn iso_connect(&mut self) -> Result<(), S7TransportError> {
        let pkt = build_cotp_cr(self.options.local_tsap, self.options.remote_tsap);
        timeout(self.options.timeout(), send_packet(&mut self.stream, &pkt))
            .await
            .map_err(|_| S7TransportError::timeout("COTP_TIMEOUT", "COTP CR 超时"))?
            .map_err(|e| map_io_error(e, "COTP_SEND_FAIL"))?;
        let resp = timeout(self.options.timeout(), recv_packet(&mut self.stream))
            .await
            .map_err(|_| S7TransportError::timeout("COTP_TIMEOUT", "COTP CC 超时"))?
            .map_err(|e| map_io_error(e, "COTP_RECV_FAIL"))?;
        check_cc(&resp)
    }

    async fn s7_setup(&mut self) -> Result<(), S7TransportError> {
        let pkt = build_s7_setup(self.pdu_ref, self.options.requested_pdu_length);
        self.bump_pdu_ref();
        timeout(self.options.timeout(), send_packet(&mut self.stream, &pkt))
            .await
            .map_err(|_| S7TransportError::timeout("S7_SETUP_TIMEOUT", "S7 Setup 超时"))?
            .map_err(|e| map_io_error(e, "S7_SETUP_SEND_FAIL"))?;
        let resp = timeout(self.options.timeout(), recv_packet(&mut self.stream))
            .await
            .map_err(|_| S7TransportError::timeout("S7_SETUP_TIMEOUT", "S7 Setup 响应超时"))?
            .map_err(|e| map_io_error(e, "S7_SETUP_RECV_FAIL"))?;
        let negotiated = parse_setup_ack(&resp, self.options.requested_pdu_length)?;
        if negotiated != self.negotiated_pdu {
            tracing::info!(negotiated, "S7 PDU 已协商");
        }
        self.negotiated_pdu = negotiated;
        Ok(())
    }

    /// 批量读（Read 路解析）：按传入顺序返回等长结果，单项 BAD 以
    /// `return_code != 0xFF` 隔离，不整体失败。
    pub async fn read_var(
        &mut self,
        items: &[S7ReadVarItem],
    ) -> Result<Vec<S7ReadVarResult>, S7TransportError> {
        self.read_chunked(items, false).await
    }

    /// 批量读（Bulk 路解析：连续区 BYTE 批量，简化填充口径）。
    pub async fn read_bulk(
        &mut self,
        items: &[S7ReadVarItem],
    ) -> Result<Vec<S7ReadVarResult>, S7TransportError> {
        self.read_chunked(items, true).await
    }

    async fn read_chunked(
        &mut self,
        items: &[S7ReadVarItem],
        bulk: bool,
    ) -> Result<Vec<S7ReadVarResult>, S7TransportError> {
        if items.is_empty() {
            return Ok(vec![]);
        }
        let mut all = Vec::with_capacity(items.len());
        for range in plan_chunks(items, self.negotiated_pdu, bulk) {
            let chunk = &items[range];
            let pkt = build_read_request(self.pdu_ref, chunk);
            self.bump_pdu_ref();
            timeout(self.options.timeout(), send_packet(&mut self.stream, &pkt))
                .await
                .map_err(|_| {
                    S7TransportError::timeout(
                        "READ_TIMEOUT",
                        if bulk { "Bulk Read 请求超时" } else { "Read 请求超时" },
                    )
                })?
                .map_err(|e| map_io_error(e, "READ_SEND_FAIL"))?;
            let resp = timeout(self.options.timeout(), recv_packet(&mut self.stream))
                .await
                .map_err(|_| {
                    S7TransportError::timeout(
                        "READ_TIMEOUT",
                        if bulk { "Bulk Read 响应超时" } else { "Read 响应超时" },
                    )
                })?
                .map_err(|e| map_io_error(e, "READ_RECV_FAIL"))?;
            let mut part = if bulk {
                parse_bulk_response(&resp, chunk)?
            } else {
                parse_read_response(&resp, chunk)?
            };
            all.append(&mut part);
        }
        Ok(all)
    }

    /// 单项写（Write 路）：`var_spec` + 值原始字节。
    pub async fn write_var(
        &mut self,
        var_spec: &[u8],
        data: &[u8],
    ) -> Result<(), S7TransportError> {
        let pkt = build_write_request(self.pdu_ref, var_spec, data);
        self.bump_pdu_ref();
        timeout(self.options.timeout(), send_packet(&mut self.stream, &pkt))
            .await
            .map_err(|_| S7TransportError::timeout("WRITE_TIMEOUT", "Write 请求超时"))?
            .map_err(|e| map_io_error(e, "WRITE_SEND_FAIL"))?;
        let resp = timeout(self.options.timeout(), recv_packet(&mut self.stream))
            .await
            .map_err(|_| S7TransportError::timeout("WRITE_TIMEOUT", "Write 响应超时"))?
            .map_err(|e| map_io_error(e, "WRITE_RECV_FAIL"))?;
        parse_write_response(&resp)
    }

    /// SZL 读取（诊断路）：返回原始 SZL 负载。
    pub async fn read_szl(
        &mut self,
        szl_id: u16,
        szl_index: u16,
    ) -> Result<Vec<u8>, S7TransportError> {
        let pkt = build_szl_request(self.pdu_ref, szl_id, szl_index);
        self.bump_pdu_ref();
        timeout(self.options.timeout(), send_packet(&mut self.stream, &pkt))
            .await
            .map_err(|_| {
                S7TransportError::timeout("SZL_TIMEOUT", format!("SZL 0x{szl_id:04X} 请求超时"))
            })?
            .map_err(|e| map_io_error(e, "SZL_SEND_FAIL"))?;
        let resp = timeout(self.options.timeout(), recv_packet(&mut self.stream))
            .await
            .map_err(|_| {
                S7TransportError::timeout("SZL_TIMEOUT", format!("SZL 0x{szl_id:04X} 响应超时"))
            })?
            .map_err(|e| map_io_error(e, "SZL_RECV_FAIL"))?;
        parse_szl_response(&resp, szl_id)
    }

    /// PDU 引用号递增（0 为保留值，wrapping 后跳过）。
    fn bump_pdu_ref(&mut self) {
        self.pdu_ref = self.pdu_ref.wrapping_add(1).max(1);
    }
}
