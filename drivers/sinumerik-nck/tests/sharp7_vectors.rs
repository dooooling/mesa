//! Sharp7 reference vectors 回放（Gate F3 第一阶段，纯离线，CI 可跑）。
//!
//! 向量来源：pinned Sharp7（soft79@eac1e728，未改源码）经 tap 对打 Mesa
//! emulator（happy，element_size 8）的真实抓包，见
//! `tools/sharp7-harness/README.md` 再生流程与 manifest.json。
//! 约束：Area 恒为 0（N）——`0<<4 == 0<<5`，刻意避开 area 合成未决分歧；
//! 本测试不断言任何非零 Area 的 areaunit 字节。
//!
//! 三重断言（证据等级见 ADR 0002：请求为独立 wire evidence，
//! 响应为 compatibility evidence——rsp 是 emulator 生成、Sharp7 接受，
//! 不是独立 server framing evidence）：
//! 1. Sharp7 请求 item == Mesa `encode_var_spec` 同逻辑地址输出（exact differential）；
//! 2. 同一抓包的响应经 Mesa transport 解析为 GOOD + pattern 数据
//!    （Sharp7 当时解码 rc=0；两实现对同一 wire 一致解码）；
//! 3. manifest 的 pin（commit/area/rc）与文件集一致，防向量漂移。

use mesa_driver_sinumerik_nck::{NckWireAddress, encode_var_spec};
use mesa_s7_transport::pdu::parse_setup_ack;
use mesa_s7_transport::read_var::{S7ReadVarItem, parse_read_response};

fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reference/sharp7")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|_| panic!("缺向量 {name}"))
}

fn manifest() -> serde_json::Value {
    let text = std::fs::read_to_string(dir().join("manifest.json")).expect("缺 manifest");
    serde_json::from_str(&text).expect("manifest 非法")
}

fn wire(
    syntax: u8,
    area_unit: u8,
    column: u16,
    line: u16,
    module: u8,
    count: u8,
) -> NckWireAddress {
    NckWireAddress {
        syntax_id: syntax,
        area_unit,
        column,
        line,
        module,
        line_count: count,
    }
}

#[test]
fn manifest_pins_source_and_outcome() {
    let m = manifest();
    assert_eq!(
        m["sharp7_commit"], "eac1e728f8523278564e83c276fa6b8d281e6ba0",
        "Sharp7 pin 漂移即失效"
    );
    assert_eq!(m["area"], 0, "只允许 Area=0（分歧规避）");
    assert_eq!(m["events"][0]["rc"], 0);
    assert_eq!(m["events"][1]["rc"], 0);
    for f in [
        "req-01.bin",
        "rsp-02.bin",
        "req-03.bin",
        "rsp-04.bin",
        "req-05.bin",
        "rsp-06.bin",
        "req-07.bin",
        "rsp-08.bin",
    ] {
        assert!(dir().join(f).exists(), "缺 {f}");
    }
}

#[test]
fn single_read_request_matches_mesa_encoder_exact() {
    // req-05：TPKT(4)+COTP(3)+S7(10)+param[04 01](2)+item(10)。
    let req = read("req-05.bin");
    assert_eq!(req.len(), 29);
    assert_eq!(req[0], 0x03, "TPKT");
    assert_eq!(&req[7..10], &[0x32, 0x01, 0x00], "S7 Job");
    assert_eq!(&req[17..19], &[0x04, 0x01], "param func+count");
    let item = &req[19..29];
    // Sharp7 原始字节：12 08 82 01 | 00 2A | 00 00 | 12 | 01。
    assert_eq!(
        item,
        &[0x12, 0x08, 0x82, 0x01, 0x00, 0x2A, 0x00, 0x00, 0x12, 0x01]
    );
    // Mesa 同逻辑地址编码必须逐字节一致（Area=0 时 <<4/<<5 无差）。
    let me = encode_var_spec(&wire(0x82, 0x01, 42, 0, 0x12, 1));
    assert_eq!(me, item, "Mesa encoder vs Sharp7 differential");
}

#[test]
fn multi_read_requests_match_mesa_encoder_exact() {
    // req-07：两项（param 42/43），其余同单读。
    let req = read("req-07.bin");
    assert_eq!(req.len(), 39);
    assert_eq!(&req[17..19], &[0x04, 0x02]);
    let item0 = &req[19..29];
    let item1 = &req[29..39];
    assert_eq!(&item0[4..6], &[0x00, 0x2A], "首项 column 42");
    assert_eq!(&item1[4..6], &[0x00, 0x2B], "次项 column 43");
    assert_eq!(encode_var_spec(&wire(0x82, 0x01, 42, 0, 0x12, 1)), item0);
    assert_eq!(encode_var_spec(&wire(0x82, 0x01, 43, 0, 0x12, 1)), item1);
}

fn parse_item() -> S7ReadVarItem {
    S7ReadVarItem {
        var_spec: vec![0x12],
        expected_data_len: 8,
    }
}

#[test]
fn responses_parse_good_in_mesa_too() {
    // rsp-06：Sharp7 当时解码 rc=0 取 1 字节 0x01；Mesa 解析同包必须 GOOD
    // 且 8 字节 pattern 全 0x01（连接内首项序号 tag）。
    let rsp = read("rsp-06.bin");
    let out = parse_read_response(&rsp, &[parse_item()]).expect("Mesa 解析");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].return_code, 0xFF);
    assert_eq!(out[0].transport_size, 0x04);
    assert_eq!(out[0].data, vec![0x01; 8]);
    // rsp-08：两项 tags 0x02/0x03（序号跨请求连续），Mesa 同样 GOOD。
    let rsp = read("rsp-08.bin");
    let out = parse_read_response(&rsp, &[parse_item(), parse_item()]).expect("Mesa 解析");
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].data, vec![0x02; 8]);
    assert_eq!(out[1].data, vec![0x03; 8]);
}

#[test]
fn setup_ack_shape_is_standard_ack_data() {
    // rsp-04：标准 Ack_Data Setup（27 字节）：12 字节头 + plen=8 +
    // PDU@S7[18]=480。Sharp7 当时以此完成协商（NckConnectTo rc=0）；
    // Mesa 解析器必须同样接受（`00 00` 是 header error bytes，无 NCK 方言）。
    let rsp = read("rsp-04.bin");
    assert_eq!(rsp.len(), 27);
    let s7 = &rsp[7..];
    assert_eq!(s7.len(), 20);
    assert_eq!(&s7[6..8], &[0x00, 0x08], "plen=8（说真话）");
    assert_eq!(&s7[10..12], &[0x00, 0x00], "header error bytes");
    assert_eq!(u16::from_be_bytes([s7[18], s7[19]]), 480);
    let pdu = parse_setup_ack(&rsp, 480).expect("Mesa Setup 解析");
    assert_eq!(pdu, 480);
}
