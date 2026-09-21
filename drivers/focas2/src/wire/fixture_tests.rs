//! Fixture 回归（PR1 Level 2 + PR2 feed + PR54 spindle）：真机捕获 → 生产 codec 直测。
//!
//! - 数据：165 定向抓包 → `10B header + payload_len` 精确切帧 → 去重传/
//!   拼接残留（`tests/fixtures/wire/{sysinfo,statinfo_mem,statinfo_mdi,feed}/`）。
//! - Gate 0 证据：sysinfo 7/7 / statinfo MEM 7/7 / statinfo MDI 7/7。
//! - feed 证据：`0x24` mantissa=100 ↔ Native actf=100（同次）；`0x24-only`
//!   生产形态真机冻结。
//! - spindle 证据（S0~S4）：`0x25` mantissa=0/500/1002/1500/800 ↔
//!   Native cnc_acts 同次；request 5 点逐字节恒定（`args=[0,0,0,0]/aux=0`）。
//! - 本模块 `#[cfg(test)]` 且 crate 内部：直接调生产 decoder，无复刻。

use std::path::PathBuf;

use super::frame::{FRAME_HEADER_LEN, FocasFrame, PacketType, decode_header};
use super::wire::{decode_feed_rate, decode_spindle_speed, decode_status_info, decode_system_info};
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
    axis1_decodes();
    axis2_decodes();
    axis3_decodes();
    axis4_fails_closed();
    axis_request_locked();
    spindle_s0s4_decodes();
    spindle_request_locked();
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

/// feed 请求：生产编码器输出 == 捕获 fixture（`encode == request` 闭环）。
/// 未来改坏 origin/type/args 任一字节，fixture 直接红。
fn feed_request_locked() {
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_FEED};
    let raw = read("feed", "feed_request.bin");
    let frames = cut_fixture_frames(&raw);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), 40, "feed 请求必须 40B（count=1）");
    // 生产 builder 重建（与 `FocasClient::feed_rate` 同源）。
    let built = {
        use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
        FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_FEED,
                [0, 0, 0, 0, 0],
            )]),
        }
        .encode()
    };
    assert_eq!(
        frames[0], built,
        "production encoder 必须 == captured fixture 全 40B"
    );
}

/// axis1：生产 `decode_axis_position` 直解 36B 响应 → mantissa=-2880。
/// （同次 Native data0=-2880，面板 X=-2.880。）
fn axis1_decodes() {
    use super::wire::{axis_to_value_for_test, axis_value_for_test, decode_axis_position};
    let frame = assemble_frame(&read("axis1", "axis_response_frame2.bin"));
    assert_eq!(frame.packet_type, PacketType::GENERIC_RESPONSE);
    let pos = decode_axis_position(&frame).expect("生产 decode_axis_position 必须成功");
    assert_eq!(pos.mantissa, -2880, "axis1 同次 mantissa 必须 -2880");
    assert_eq!(pos.base, 10);
    assert_eq!(pos.exponent, 3);
    let exp = expected("axis1");
    assert_eq!(
        exp["native"]["data0"].as_i64().unwrap() as i32,
        pos.mantissa,
        "Wire mantissa ↔ Native data0 同次一致"
    );
    assert_eq!(
        exp["mesa"]["axis_absolute"].as_i64().unwrap() as i32,
        pos.mantissa
    );
    // adapter：负值合法 → I32（与 feed 的负值拒绝无关）。
    assert_eq!(
        axis_to_value_for_test(&pos).expect("负坐标必须 I32"),
        axis_value_for_test(-2880),
    );
}

/// axis2：mantissa=-3160（面板 Y=-3.160）。
fn axis2_decodes() {
    use super::wire::decode_axis_position;
    let frame = assemble_frame(&read("axis2", "axis_response_frame2.bin"));
    let pos = decode_axis_position(&frame).expect("axis2 必须解码");
    assert_eq!(pos.mantissa, -3160);
    assert_eq!(pos.base, 10);
    assert_eq!(pos.exponent, 3);
    let exp = expected("axis2");
    assert_eq!(
        exp["native"]["data0"].as_i64().unwrap() as i32,
        pos.mantissa
    );
}

/// axis3：mantissa=-10（面板 Z=-0.010，小值）。
fn axis3_decodes() {
    use super::wire::decode_axis_position;
    let frame = assemble_frame(&read("axis3", "axis_response_frame2.bin"));
    let pos = decode_axis_position(&frame).expect("axis3 必须解码");
    assert_eq!(pos.mantissa, -10);
    let exp = expected("axis3");
    assert_eq!(
        exp["native"]["data0"].as_i64().unwrap() as i32,
        pos.mantissa
    );
}

/// axis4 负控制：codec 照常解出字段（mantissa==Native），但 adapter
/// fail-closed（raw 指数 `30 33` → `i16 12339` → Unsupported → ERR/BAD，
/// 不进 I32；`.bin` 字节不动，只修正语义描述）。
fn axis4_fails_closed() {
    use super::wire::{RawNumeric8, axis_to_value_for_test, decode_axis_position};
    let frame = assemble_frame(&read("axis4_nc", "axis_response_frame2.bin"));
    let pos = decode_axis_position(&frame).expect("codec 必须解出字段");
    assert_eq!(pos.mantissa, 0x2000_0202);
    assert_eq!(
        RawNumeric8::decode(&pos.raw).expect("raw 必须 8B").exponent,
        12339,
        "raw 30 33 必须解为 i16 12339（不是 u8 51）"
    );
    assert_eq!(pos.exponent, 51, "兼容视图截断保留（旧断言）");
    let exp = expected("axis4_nc");
    assert_eq!(
        exp["native"]["data0"].as_i64().unwrap() as i32,
        pos.mantissa,
        "无效轴 Native↔Wire mantissa 仍一致（错的是 CNC 值本身）"
    );
    assert!(
        axis_to_value_for_test(&pos).is_err(),
        "i16 exp=12339 必须 fail-closed"
    );
}

/// axis 请求：生产编码器输出 == 捕获 fixture（`encode == request` 闭环）。
/// 四组（axis1/2/3/4_nc）全部锁定 selector `1→1/2→2/3→3/4→4`；
/// 每组 frame#1（`0x18` preflight）与 frame#2（`0x26`）双锁。
fn axis_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{AXIS_ARG0_OBSERVED, DEV_CNC, FUNC_AXIS_ABSOLUTE, FUNC_SYSINFO};
    let build = |func: u32, args: [i32; 5]| {
        FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(DEV_CNC, func, args)]),
        }
        .encode()
    };
    for (group, axis) in [("axis1", 1), ("axis2", 2), ("axis3", 3), ("axis4_nc", 4)] {
        // frame#2：0x26(axis) 全 40B。
        let raw = read(group, "axis_request_frame2.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} frame#2 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{group} frame#2 必须 40B");
        assert_eq!(
            frames[0],
            build(FUNC_AXIS_ABSOLUTE, [AXIS_ARG0_OBSERVED, axis, 0, 0, 0]),
            "{group} production encoder 必须 == captured fixture 全 40B"
        );
        // frame#1：0x18 preflight 全 40B（operation request evidence 闭环）。
        let raw1 = read(group, "axis_request_frame1.bin");
        let frames1 = cut_fixture_frames(&raw1);
        assert_eq!(frames1.len(), 1, "{group} frame#1 必须恰好 1 帧");
        assert_eq!(
            frames1[0],
            build(FUNC_SYSINFO, [0, 0, 0, 0, 0]),
            "{group} frame#1 必须 == 0x18 preflight 全 40B"
        );
    }
}

/// spindle S0~S4：`spindle_response_frame.bin` 经生产 codec 解码 ==
/// expected（mantissa=0/500/1002/1500/800 ↔ Native cnc_acts 同次）。
fn spindle_s0s4_decodes() {
    use super::wire::spindle_to_value_for_test as to_value;
    for (group, want) in [
        ("spindle0", 0),
        ("spindle1", 500),
        ("spindle2", 1002),
        ("spindle3", 1500),
        ("spindle4", 800),
    ] {
        let frame = assemble_frame(&read(group, "spindle_response_frame.bin"));
        let spd = decode_spindle_speed(&frame).expect("{group} 必须解码");
        assert_eq!(spd.mantissa, want, "{group} mantissa");
        assert_eq!(spd.base, 10, "{group} base");
        assert_eq!(spd.exponent, 0, "{group} exp");
        let exp = expected(group);
        assert_eq!(
            exp["native"]["data"].as_i64().unwrap() as i32,
            spd.mantissa,
            "{group} Native↔Wire mantissa 同次一致"
        );
        assert_eq!(
            to_value(&spd).expect("{group} adapter 必须通过"),
            mesa_core_types::Value::I32(want),
            "{group} Mesa 映射"
        );
    }
}

/// spindle 请求：生产编码器输出 == 捕获 fixture（`encode == request` 闭环）。
/// S0~S4 五组 request 全 40B 逐字节恒定（`args=[0,0,0,0]/aux=0`，无 selector）。
fn spindle_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_SPINDLE_SPEED};
    let build = FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&[request_subpacket(
            DEV_CNC,
            FUNC_SPINDLE_SPEED,
            [0, 0, 0, 0, 0],
        )]),
    }
    .encode();
    assert_eq!(build.len(), 40, "0x25 请求必须 40B");
    for group in ["spindle0", "spindle1", "spindle2", "spindle3", "spindle4"] {
        let raw = read(group, "spindle_request_frame.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{group} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{group} production encoder 必须 == captured fixture 全 40B"
        );
    }
}
