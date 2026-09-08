//! S7 ReadVar 批量读（请求/响应帧 + PDU 感知分片）。
//!
//! 本层只认识不透明变量规范字节（`var_spec`）：S7ANY 由 `s7` Driver 编码
//! 12 字节（含 `0x12 0x0A 0x10` 头），NCK 由 `sinumerik-nck` Driver 编码
//! `0x82/83/84` 规范。分片只看字节开销，与语法无关。
//!
//! 错误隔离口径（与抽取前一致）：单项返回码非 0xFF → 该项 BAD（`return_code`
//! 原样带回，调用方发 BAD 点），不整体失败；只有整包 ROSCTR/长度/基数
//! 错位才是连接级 fatal。

use std::ops::Range;

use crate::error::{S7_ITEM_OK, S7TransportError, s7_cpu_error};
use crate::pdu::{S7_FUNC_READ, S7_ROSCTR_ACK, S7_ROSCTR_JOB, check_lengths, s7_header, wrap_s7};

// ---------------------------------------------------------------------------
// 协议常量（解释“为什么”）
// ---------------------------------------------------------------------------

/// S7 单次 PDU 可携带 item 上限：PDU 480 下按 12 字节/item + 头估算的经验值 19。
pub const S7_MAX_ITEMS_PER_PDU: usize = 19;
/// S7 传输尺寸：0x03=Bit（响应按位长回传，解析时按 1 字节消费）。
pub const S7_TRANSPORT_BIT: u8 = 0x03;
/// PDU 分片安全余量：响应头/填充/取整的保守扣除（与抽取前 `-32` 一致）。
pub const S7_CHUNK_SAFETY_MARGIN: usize = 32;

/// 单个读项：不透明变量规范 + 期望返回字节数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S7ReadVarItem {
    /// 变量规范字节（S7ANY 为 12 字节 `0x12 0x0A 0x10 …`）。
    pub var_spec: Vec<u8>,
    /// 期望返回字节数（S7ANY 由类型字节长决定；实际返回更长时截断）。
    pub expected_data_len: usize,
}

/// 单项读结果：返回码 + 传输尺寸 + 数据。
///
/// `return_code != 0xFF` 表示该项 BAD（`data` 为空，调用方按项隔离）；
/// 整包级错误以 `Err` 返回（ROSCTR 拒绝/长度错位/基数错位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S7ReadVarResult {
    pub return_code: u8,
    pub transport_size: u8,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// PDU 感知分片（纯函数，行为与抽取前两处分片循环逐项一致）
// ---------------------------------------------------------------------------

/// 按 PDU 长度把读项切成若干批（返回索引区间）。
///
/// 开销口径：每项请求 `var_spec.len()` + 响应 `expected + 4`（4 为单项返回头）。
/// `first_resp_header` 复刻历史差异：`read_vars` 路首项不计 4 字节响应头，
/// bulk 路首项计入——两处循环原样保留，避免分片边界漂移。
pub fn plan_chunks(
    items: &[S7ReadVarItem],
    pdu_length: u16,
    first_resp_header: bool,
) -> Vec<Range<usize>> {
    let budget = (pdu_length as usize).saturating_sub(S7_CHUNK_SAFETY_MARGIN);
    let mut out = Vec::new();
    let mut start = 0;
    while start < items.len() {
        let mut end = start + 1;
        let mut bytes = items[start].var_spec.len() + items[start].expected_data_len;
        if first_resp_header {
            bytes += 4;
        }
        while end < items.len() && end - start < S7_MAX_ITEMS_PER_PDU {
            let next = items[end].var_spec.len() + items[end].expected_data_len + 4;
            if bytes + next > budget {
                break;
            }
            bytes += next;
            end += 1;
        }
        out.push(start..end);
        start = end;
    }
    out
}

// ---------------------------------------------------------------------------
// 请求构造
// ---------------------------------------------------------------------------

/// 构造 ReadVar 请求（功能码 0x04 + item 数 + 拼接 var_spec），含 TPKT/COTP 头。
pub fn build_read_request(pdu_ref: u16, items: &[S7ReadVarItem]) -> Vec<u8> {
    let mut param = Vec::with_capacity(2 + items.len() * 12);
    param.push(S7_FUNC_READ);
    param.push(items.len() as u8);
    for it in items {
        param.extend_from_slice(&it.var_spec);
    }
    let param_len = param.len() as u16;
    let mut s7 = Vec::with_capacity(10 + param.len());
    s7.extend_from_slice(&s7_header(S7_ROSCTR_JOB, pdu_ref, param_len, 0));
    s7.extend_from_slice(&param);
    wrap_s7(&s7)
}

// ---------------------------------------------------------------------------
// 响应解析（Read 路：复杂填充启发式，与抽取前 `parse_read_resp` 逐行一致）
// ---------------------------------------------------------------------------

/// 解析 ReadVar 响应（`read_vars` 路）。
///
/// GOOD 项按 `min(线缆长度, 期望长度)` 切片；BIT 传输固定消费 1 字节；
/// 奇长 payload 后的填充字节跳过逻辑原样保留（DB 偶数不触发、单字节触发，
/// BIT/BYTE 混批需兼顾，见内联注释）。
pub fn parse_read_response(
    resp: &[u8],
    items: &[S7ReadVarItem],
) -> Result<Vec<S7ReadVarResult>, S7TransportError> {
    if resp.len() < 7 + 12 {
        return Err(S7TransportError::protocol(
            "READ_SHORT",
            format!("响应过短 {}", resp.len()),
        ));
    }
    let s7 = &resp[7..];
    if s7.len() < 12 {
        return Err(S7TransportError::protocol("S7_SHORT", "S7 头部缺失"));
    }
    if s7[1] != S7_ROSCTR_ACK {
        let err_class = s7.get(17).copied().unwrap_or(0);
        return Err(s7_cpu_error(
            err_class,
            &format!(
                "Read 被拒绝 rosctr={:02x} 期望 {:02x}",
                s7[1], S7_ROSCTR_ACK
            ),
        ));
    }
    let (param_len, data_len) = check_lengths(s7)?;
    let data = &s7[12 + param_len..12 + param_len + data_len];
    let mut out: Vec<S7ReadVarResult> = Vec::with_capacity(items.len());
    let mut off = 0;
    for (idx, it) in items.iter().enumerate() {
        if off + 4 > data.len() {
            return Err(S7TransportError::protocol(
                "READ_ITEM_SHORT",
                format!("item {idx} 头部缺失"),
            ));
        }
        let ret = data[off];
        let transport = data[off + 1];
        let len_bits = u16::from_be_bytes([data[off + 2], data[off + 3]]) as usize;
        off += 4;
        if ret != S7_ITEM_OK {
            // 按项 BAD：跳过该项数据区以对齐下一项（错误时 len 可能为 0）。
            tracing::warn!(idx, ret, "S7 ReadVar item 按项 BAD");
            let byte_len = len_bits.div_ceil(8);
            if byte_len > 0 && off + byte_len <= data.len() {
                off += byte_len;
                if byte_len % 2 == 1
                    && off < data.len()
                    && data[off] == 0x00
                    && idx + 1 < items.len()
                {
                    off += 1;
                }
            }
            out.push(S7ReadVarResult {
                return_code: ret,
                transport_size: transport,
                data: Vec::new(),
            });
            continue;
        }
        // S7 返回长度单位为 bit，需转字节；BIT 固定 1 bit→1 byte。
        let byte_len = if transport == S7_TRANSPORT_BIT {
            1
        } else {
            len_bits.div_ceil(8)
        };
        // 防御实现差异：回退到调用方期望长度截断。
        let take = byte_len.min(it.expected_data_len).max(1);
        if off + take > data.len() {
            return Err(S7TransportError::protocol(
                "READ_DATA_SHORT",
                format!("item {idx} 数据缺失"),
            ));
        }
        let bytes = data[off..off + take].to_vec();
        out.push(S7ReadVarResult {
            return_code: ret,
            transport_size: transport,
            data: bytes,
        });
        off += take;
        // S7 协议字对齐：奇数长度 payload 后补 1 字节 0x00，解析时需跳过否则
        // 下一 item 头 0xFF 错位；实测 DB 20 字节偶数不触发、1 字节触发，需兼顾
        // BIT（1 字节）与 BYTE 混批场景。
        // NOTE: 内层冗余嵌套（`take % 2` 在外层已成立、双重 `data[off] == 0x00`
        // 检查）照搬历史实现：与抽取前逐行一致，任何化简都需重跑填充对齐用例。
        if take % 2 == 1 && off < data.len() {
            // 填充字节为 0，若下一个 item 头部恰为 0xFF 则不是填充；
            // 仅当剩余字节足够且下一字节不是 0xFF 才跳过。
            if data.len() - off >= 1 && off + 1 < data.len() && data[off] == 0x00 {
                // 预测下一个头部 ret 应为 0xFF：若下一字节是 0xFF 且再下一字节
                // transport 合理，则为新头部而非填充；此处保守处理。
                if off + 4 <= data.len() && data[off] == 0x00 && data[off + 1] != 0x04 {
                    // 可能是填充，跳过
                    off += 1;
                } else if take % 2 == 1 {
                    // 默认跳过填充字节
                    if data[off] == 0x00 {
                        // 只有在确实是填充时才跳过；为不误判，仅当还有剩余
                        // items 且 off 字节为 0 时才认为是填充。
                        if idx + 1 < items.len() {
                            off += 1;
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 响应解析（Bulk 路：简化填充逻辑，与抽取前 `parse_bulk_resp` 逐行一致）
// ---------------------------------------------------------------------------

/// 解析 Bulk 批量读响应（连续区 `read_byte_ranges` 路）。
///
/// 与 Read 路差异：无 BIT/BOOL 归一化（bulk 恒为 BYTE 批量），填充跳过为
/// 简单版（奇长 + `0x00` + 还有后项即跳过）。两路解析保持分离，避免合并
/// 启发式导致任一路行为漂移。
pub fn parse_bulk_response(
    resp: &[u8],
    items: &[S7ReadVarItem],
) -> Result<Vec<S7ReadVarResult>, S7TransportError> {
    if resp.len() < 7 + 12 {
        return Err(S7TransportError::protocol(
            "READ_SHORT",
            format!("Bulk 响应过短 {}", resp.len()),
        ));
    }
    let s7 = &resp[7..];
    if s7.len() < 12 {
        return Err(S7TransportError::protocol("S7_SHORT", "S7 头部缺失"));
    }
    if s7[1] != S7_ROSCTR_ACK {
        let err_class = s7.get(17).copied().unwrap_or(0);
        return Err(s7_cpu_error(
            err_class,
            &format!("Bulk Read 被拒绝 rosctr={:02x}", s7[1]),
        ));
    }
    let (param_len, data_len) = check_lengths(s7)?;
    let data = &s7[12 + param_len..12 + param_len + data_len];
    let mut out = Vec::with_capacity(items.len());
    let mut off = 0;
    for (idx, it) in items.iter().enumerate() {
        if off + 4 > data.len() {
            return Err(S7TransportError::protocol(
                "READ_ITEM_SHORT",
                format!("bulk {idx} 头部缺失"),
            ));
        }
        let ret = data[off];
        let transport = data[off + 1];
        let len_bits = u16::from_be_bytes([data[off + 2], data[off + 3]]) as usize;
        off += 4;
        if ret != S7_ITEM_OK {
            tracing::warn!(idx, ret, "Bulk item BAD");
            let byte_len = len_bits.div_ceil(8);
            if byte_len > 0 && off + byte_len <= data.len() {
                off += byte_len;
                if byte_len % 2 == 1
                    && off < data.len()
                    && idx + 1 < items.len()
                    && data[off] == 0x00
                {
                    off += 1;
                }
            }
            out.push(S7ReadVarResult {
                return_code: ret,
                transport_size: transport,
                data: Vec::new(),
            });
            continue;
        }
        let byte_len = len_bits.div_ceil(8);
        // 期望长度与实际返回长度取小，避免 STRING 等短回越界。
        let take = it.expected_data_len.min(byte_len).max(1);
        if off + take > data.len() {
            return Err(S7TransportError::protocol(
                "READ_DATA_SHORT",
                format!("bulk {idx} 数据缺失"),
            ));
        }
        out.push(S7ReadVarResult {
            return_code: ret,
            transport_size: transport,
            data: data[off..off + take].to_vec(),
        });
        off += byte_len;
        if byte_len % 2 == 1 && off < data.len() && idx + 1 < items.len() && data[off] == 0x00 {
            off += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(spec_len: usize, expect: usize) -> S7ReadVarItem {
        S7ReadVarItem {
            var_spec: vec![0x12; spec_len],
            expected_data_len: expect,
        }
    }

    #[test]
    fn chunking_matches_legacy_read_vars_loop() {
        // 12 字节规范 + 4 字节期望：首项 16，后续每项 20；PDU480 预算 448。
        // 1 + floor((448-16)/20) = 1+21，但上限 19 项。
        let items: Vec<_> = (0..25).map(|_| item(12, 4)).collect();
        let chunks = plan_chunks(&items, 480, false);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], 0..19);
        assert_eq!(chunks[1], 19..25);
    }

    #[test]
    fn chunking_matches_legacy_bulk_loop() {
        // bulk 路首项计入 +4：首项 12+256+4=272（STRING 256 @ PDU480 预算 448），
        // 第二项同等 272 放不下 → 每批 1 项。
        let items = vec![item(12, 256), item(12, 256)];
        let chunks = plan_chunks(&items, 480, true);
        assert_eq!(chunks, vec![0..1, 1..2]);
        // read 路首项不计 +4：首项 268，第二项 272，268+272=540>448 → 同样两批。
        let chunks = plan_chunks(&items, 480, false);
        assert_eq!(chunks, vec![0..1, 1..2]);
    }

    #[test]
    fn read_request_header_shape() {
        let items = vec![item(12, 4)];
        let pkt = build_read_request(7, &items);
        // TPKT 头 + COTP Data + S7 头：32 01(ref 7) param_len=14 data_len=0 04 01。
        assert_eq!(&pkt[0..4], &[0x03, 0x00, 0x00, 0x1F]);
        assert_eq!(&pkt[4..7], &[0x02, 0xF0, 0x80]);
        assert_eq!(
            &pkt[7..17],
            &[0x32, 0x01, 0x00, 0x00, 0x00, 0x07, 0x00, 0x0E, 0x00, 0x00]
        );
        assert_eq!(&pkt[17..19], &[0x04, 0x01]);
        assert_eq!(&pkt[19..31], &[0x12; 12]);
    }

    /// 构造最小 Read Ack 响应：TPKT+COTP+S7(12 头)+param(2)+data。
    fn ack_resp(data_items: &[u8], param: &[u8]) -> Vec<u8> {
        let param_len = param.len() as u16;
        let data_len = data_items.len() as u16;
        let mut s7 = vec![0x32, 0x03, 0x00, 0x00, 0x00, 0x01];
        s7.extend_from_slice(&param_len.to_be_bytes());
        s7.extend_from_slice(&data_len.to_be_bytes());
        s7.extend_from_slice(&[0x00, 0x00]);
        s7.extend_from_slice(param);
        s7.extend_from_slice(data_items);
        let mut pkt = vec![0x03, 0x00, 0x00, 0x00, 0x02, 0xF0, 0x80];
        pkt.extend_from_slice(&s7);
        let len = pkt.len() as u16;
        pkt[2..4].copy_from_slice(&len.to_be_bytes());
        pkt
    }

    #[test]
    fn parse_single_good_item_with_pad_skip() {
        // 单项 GOOD：ret FF / transport 04 / len 8bit（1 字节）+ 数据 0x2A；
        // 奇长 take=1 → 填充启发式在包尾无后项，不跳过越界。
        let data = [0xFF, 0x04, 0x00, 0x08, 0x2A];
        let resp = ack_resp(&data, &[0x04, 0x01]);
        let out = parse_read_response(&resp, &[item(12, 1)]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].return_code, 0xFF);
        assert_eq!(out[0].data, vec![0x2A]);
    }

    #[test]
    fn parse_bad_item_is_isolated_not_fatal() {
        // 第一项 BAD（0x05，无数据），第二项 GOOD：整包 Ok，BAD 项 data 为空。
        let data = [
            0x05, 0x04, 0x00, 0x00, // BAD，无数据
            0xFF, 0x04, 0x00, 0x08, 0x2A, // GOOD
        ];
        let resp = ack_resp(&data, &[0x04, 0x02]);
        let out = parse_read_response(&resp, &[item(12, 1), item(12, 1)]).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].return_code, 0x05);
        assert!(out[0].data.is_empty());
        assert_eq!(out[1].data, vec![0x2A]);
    }

    #[test]
    fn parse_rejected_rosctr_maps_cpu_error() {
        // S7 区需 ≥18 字节（s7[17] 为 CPU 错误码位），用 6 字节 param 补齐。
        let mut resp = ack_resp(&[], &[0x04, 0x01, 0x00, 0x00, 0x00, 0x05]);
        resp[7 + 1] = 0x01; // ROSCTR Job（非 Ack）
        let err = parse_read_response(&resp, &[item(12, 1)]).unwrap_err();
        assert_eq!(err.code, "S7_0x05");
    }

    #[test]
    fn parse_truncated_cardinality_is_fatal() {
        // 声明 2 项但 data 只够 1 项头 → 第二项头部缺失，整包 fatal。
        let data = [0xFF, 0x04, 0x00, 0x08, 0x2A];
        let resp = ack_resp(&data, &[0x04, 0x02]);
        let err = parse_read_response(&resp, &[item(12, 1), item(12, 1)]).unwrap_err();
        assert_eq!(err.code, "READ_ITEM_SHORT");
    }

    #[test]
    fn bulk_parse_truncates_to_expected() {
        // 线缆返回 4 字节，期望 2 字节 → 截断取 2。
        let data = [0xFF, 0x04, 0x00, 0x20, 0x01, 0x02, 0x03, 0x04];
        let resp = ack_resp(&data, &[0x04, 0x01]);
        let out = parse_bulk_response(&resp, &[item(12, 2)]).unwrap();
        assert_eq!(out[0].data, vec![0x01, 0x02]);
    }
}
