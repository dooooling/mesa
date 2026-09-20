//! Fixture 回归（PR1 Level 2 + PR2 feed）：真机捕获 → 生产 codec 直测。
//!
//! - 数据：165 定向抓包 → `10B header + payload_len` 精确切帧 → 去重传/
//!   拼接残留（`tests/fixtures/wire/{sysinfo,statinfo_mem,statinfo_mdi,feed}/`）。
//! - Gate 0 证据：sysinfo 7/7 / statinfo MEM 7/7 / statinfo MDI 7/7。
//! - feed 证据：`0x24` mantissa=100 ↔ Native actf=100（同次）；`0x24-only`
//!   生产形态真机冻结。
//! - 本模块 `#[cfg(test)]` 且 crate 内部：直接调生产 decoder，无复刻。

use std::path::PathBuf;

use super::frame::{FRAME_HEADER_LEN, FocasFrame, PacketType, decode_header};
use super::wire::{decode_feed_rate, decode_status_info, decode_system_info};
use super::{cut_fixture_frames, fixture_dir, read_fixture_bytes};

fn dir(group: &str) -> PathBuf {
    fixture_dir().join(group)
}

fn read(group: &str, name: &str) -> Vec<u8> {
    read_fixture_bytes(&dir(group).join(name))
}

fn expected(group: &str) -> serde_json::Value {
    let raw = read(group, "expected.json");
    serde_json::from_slice(&raw).expect("expected.json 非法")
}

/// fixture 单帧装配：1 个 `.bin` 切出恰好 1 帧 → 生产 `assemble` 同源路径。
fn assemble_frame(raw: &[u8]) -> FocasFrame {
    let frames = cut_fixture_frames(raw);
    assert_eq!(frames.len(), 1, "fixture 必须恰好 1 帧（已清洗）");
    let f = &frames[0];
    let mut head = [0u8; FRAME_HEADER_LEN];
    head.copy_from_slice(&f[..FRAME_HEADER_LEN]);
    let hdr = decode_header(&head).expect("fixture header 必须合法");
    super::frame::assemble(hdr, f[FRAME_HEADER_LEN..].to_vec()).expect("fixture assemble 必须成功")
}

/// 全量回归入口（`wire.rs` 单测转调；失败即 production codec 与真机偏离）。
pub(crate) fn run_all() {
    sysinfo_decodes();
    sysinfo_request_locked();
    statinfo_mem_decodes();
    statinfo_mdi_decodes();
    statinfo_request_stable();
    feed_9pack_decodes();
    feed_single_decodes();
    feed_request_locked();
}

/// sysinfo：`sysinfo_response.bin` 经生产 codec 解码 == expected 7 字段。
fn sysinfo_decodes() {
    let frame = assemble_frame(&read("sysinfo", "sysinfo_response.bin"));
    assert_eq!(frame.packet_type, PacketType::GENERIC_RESPONSE);
    let info = decode_system_info(&frame).expect("生产 decode_system_info 必须成功");
    let n = &expected("sysinfo")["native"];
    assert_eq!(info.addinfo, n["addinfo"].as_i64().unwrap() as i16);
    assert_eq!(info.max_axis, n["max_axis"].as_i64().unwrap() as i16);
    assert_eq!(info.cnc_type, n["cnc_type"].as_str().unwrap());
    assert_eq!(info.mt_type, n["mt_type"].as_str().unwrap());
    assert_eq!(info.series, n["series"].as_str().unwrap());
    assert_eq!(info.version, n["version"].as_str().unwrap());
    assert_eq!(info.axes, n["axes"].as_str().unwrap());
}

/// sysinfo 请求：生产编码 == 捕获 bytes（40B 逐字节）。
fn sysinfo_request_locked() {
    let raw = read("sysinfo", "sysinfo_request.bin");
    let frames = cut_fixture_frames(&raw);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), 40, "SYSINFO request 必须 40B");
    assert_eq!(
        &frames[0][..20],
        &[
            0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x01, 0x21, 0x01, 0x00, 0x1e, 0x00, 0x01, 0x00, 0x1c,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x18,
        ]
    );
}

/// statinfo MEM：frame#2 经生产 codec 解码 == expected 7 字段。
fn statinfo_mem_decodes() {
    let frame = assemble_frame(&read("statinfo_mem", "statinfo_response_frame2.bin"));
    let st = decode_status_info(&frame).expect("生产 decode_status_info 必须成功");
    let n = &expected("statinfo_mem")["native"];
    for k in ["aut", "run", "motion", "mstb", "emergency", "alarm", "edit"] {
        let want = n[k].as_u64().unwrap() as u16;
        let got = match k {
            "aut" => st.aut,
            "run" => st.run,
            "motion" => st.motion,
            "mstb" => st.mstb,
            "emergency" => st.emergency,
            "alarm" => st.alarm,
            "edit" => st.edit,
            _ => unreachable!(),
        };
        assert_eq!(got, want, "MEM {k} 必须同次一致");
    }
    let exp = expected("statinfo_mem");
    assert_eq!(
        exp["mesa"]["machine_status"].as_u64().unwrap() as u16,
        st.aut,
        "machine_status == aut"
    );
}

/// statinfo MDI：同上（`aut=0`）。
fn statinfo_mdi_decodes() {
    let frame = assemble_frame(&read("statinfo_mdi", "statinfo_response_frame2.bin"));
    let st = decode_status_info(&frame).expect("生产 decode_status_info 必须成功");
    let exp = expected("statinfo_mdi");
    assert_eq!(st.aut, 0, "MDI aut 必须 0");
    assert_eq!(st.aut, exp["native"]["aut"].as_u64().unwrap() as u16);
    assert_eq!(st.run, exp["native"]["run"].as_u64().unwrap() as u16);
}

/// B1↔B2 请求对称：frame#2 在 MEM/MDI 下相同（请求不带状态）。
fn statinfo_request_stable() {
    let a = read("statinfo_mem", "statinfo_request_frame2.bin");
    let b = read("statinfo_mdi", "statinfo_request_frame2.bin");
    assert_eq!(a, b, "statinfo 请求与 mode 无关（状态只在响应里）");
    let fa = cut_fixture_frames(&a);
    assert_eq!(fa.len(), 1);
    assert_eq!(fa[0].len(), 10 + 0x56, "frame#2 必须 96B");
}

/// feed 9-pack：生产 `decode_feed_rate` 直解 220B 全帧 → mantissa=100。
/// （生产 `find_function` 本就支持多 subpacket；同次 Native actf=100。）
fn feed_9pack_decodes() {
    let frame = assemble_frame(&read("feed", "feed_response_9pack.bin"));
    assert_eq!(frame.packet_type, PacketType::GENERIC_RESPONSE);
    let rate = decode_feed_rate(&frame).expect("生产 decode_feed_rate 必须成功");
    assert_eq!(rate.mantissa, 100, "9-pack 同次 mantissa 必须 100");
    assert_eq!(rate.base, 10);
    assert_eq!(rate.exponent, 0);
    assert_eq!(rate.scaled().expect("100/1 无损"), (100, 1));
    let exp = expected("feed");
    assert_eq!(
        exp["native"]["actf"].as_i64().unwrap() as i32,
        rate.mantissa,
        "Wire mantissa ↔ Native actf 同次一致"
    );
    assert_eq!(
        exp["mesa"]["machine_feed"].as_u64().unwrap(),
        100,
        "mesa machine_feed == 100"
    );
}

/// feed single：`0x24-only` 探针响应 36B → mantissa=200（captured）。
fn feed_single_decodes() {
    let frame = assemble_frame(&read("feed", "feed_response_single.bin"));
    let rate = decode_feed_rate(&frame).expect("single 形态必须解码");
    assert_eq!(rate.mantissa, 200);
    assert_eq!(rate.base, 10);
    assert_eq!(rate.exponent, 0);
}

/// feed 请求：生产形态 count=1/40B（`0x24-only` Gate 冻结）。
fn feed_request_locked() {
    let raw = read("feed", "feed_request.bin");
    let frames = cut_fixture_frames(&raw);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), 40, "feed 请求必须 40B（count=1）");
    assert_eq!(
        &frames[0][..20],
        &[
            0xA0, 0xA0, 0xA0, 0xA0, 0x00, 0x01, 0x21, 0x01, 0x00, 0x1e, 0x00, 0x01, 0x00, 0x1c,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x24,
        ]
    );
}
