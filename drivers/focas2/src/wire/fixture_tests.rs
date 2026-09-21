//! Fixture 回归（PR1 Level 2 + PR2 feed + PR54 spindle + PR55 macro + PR56 pmc）：
//! 真机捕获 → 生产 codec 直测。
//!
//! - 数据：165 定向抓包 → `10B header + payload_len` 精确切帧 → 去重传/
//!   拼接残留（`tests/fixtures/wire/{sysinfo,statinfo_mem,statinfo_mdi,feed}/`）。
//! - Gate 0 证据：sysinfo 7/7 / statinfo MEM 7/7 / statinfo MDI 7/7。
//! - feed 证据：`0x24` mantissa=100 ↔ Native actf=100（同次）；`0x24-only`
//!   生产形态真机冻结。
//! - spindle 证据（S0~S4）：`0x25` mantissa=0/500/1002/1500/800 ↔
//!   Native cnc_acts 同次；request 5 点逐字节恒定（`args=[0,0,0,0]/aux=0`）。
//! - macro 证据（M0~M3）：`0x15` mcr=0/250000000/123450000/-750000000 +
//!   dec=0/7/7/8 ↔ Native cnc_rdmacro 同次；request `args=[n,n,0,0]`；
//!   Mesa 取 scaled F64（与 feed/spindle 取 mantissa 形成对照）。
//! - pmc 证据（P0~P3）：`0x8001/device=2` scalar；BYTE/WORD/DWORD +
//!   bit projection；Mesa BYTE→I32（PR56 产品合同修正）。
//! - 本模块 `#[cfg(test)]` 且 crate 内部：直接调生产 decoder，无复刻。

use std::path::PathBuf;

use super::frame::{FRAME_HEADER_LEN, FocasFrame, PacketType, decode_header};
use super::wire::{
    decode_feed_rate, decode_macro_value, decode_spindle_speed, decode_status_info,
    decode_system_info,
};
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

/// param 单测装配 helper（`#[cfg(test)]`；wire.rs param 单测共用）：
/// 按 identity 形态构造 264B data（datano/attr/value + `00 0a 00 00`）。
/// Q0 `00 0a 00 03` 变体与 unknown-tail 变体走
/// `assemble_param_frame_for_test_raw_tail`（显式传 tail，不由 attr 推断——
/// Evidence 只证明 Q0 同时出现 attr=4/tail=...03，未证明 attr 决定 tail）。
/// unknown-tail 变体走 `assemble_param_frame_for_test_raw_tail`。
#[cfg(test)]
pub(super) fn assemble_param_frame_for_test(number: u32, attr: u32, value: i32) -> FocasFrame {
    assemble_param_frame_for_test_raw_tail(number, attr, value, super::wire::PARAM_TAIL_IDENTITY)
}

/// param 未知 tail 变体（`#[cfg(test)]`；unknown-scale fail-closed 回归用）。
#[cfg(test)]
pub(super) fn assemble_param_frame_for_test_raw_tail(
    number: u32,
    attr: u32,
    value: i32,
    tail: [u8; 4],
) -> FocasFrame {
    let mut data = Vec::with_capacity(264);
    data.extend_from_slice(&number.to_be_bytes());
    data.extend_from_slice(&attr.to_be_bytes());
    data.extend_from_slice(&value.to_be_bytes());
    data.extend_from_slice(&tail);
    while data.len() < 264 {
        data.extend_from_slice(&[0x00, 0x0a, 0x00, 0x00]);
    }
    data.truncate(264);
    // 真实模型构造：count=1 + size=280 + dev/path/cmd + status/details + dlen + data。
    let mut payload = vec![0x00, 0x01];
    payload.extend_from_slice(&[0x01, 0x18]); // size=280
    payload.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x8D]);
    payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    payload.extend_from_slice(&[0x01, 0x08]); // dlen=264
    payload.extend_from_slice(&data);
    FocasFrame {
        origin: 0x0003,
        packet_type: PacketType::GENERIC_RESPONSE,
        payload,
    }
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
    macro_m0m3_decodes();
    macro_request_locked();
    pmc_scalar_decodes();
    pmc_request_locked();
    param_decodes();
    param_q0_negative_evidence();
    param_request_locked();
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

/// macro M0~M3：`macro_response_frame.bin` 经生产 codec 解码 ==
/// expected（mcr=0/250000000/123450000/-750000000 + dec=0/7/7/8 ↔
/// Native cnc_rdmacro 同次；Mesa 取 scaled F64）。
fn macro_m0m3_decodes() {
    use super::wire::macro_to_value_for_test as to_value;
    for (group, mcr, dec, scaled) in [
        ("macro0", 0, 0, 0.0),
        ("macro1", 250000000, 7, 25.0),
        ("macro2", 123450000, 7, 12.345),
        ("macro3", -750000000, 8, -7.5),
    ] {
        let frame = assemble_frame(&read(group, "macro_response_frame.bin"));
        let m = decode_macro_value(&frame).expect("{group} 必须解码");
        assert_eq!(m.mantissa, mcr, "{group} mantissa==mcr_val");
        assert_eq!(m.base, 10, "{group} base");
        assert_eq!(m.exponent, dec as u8, "{group} exp==dec_val");
        let exp = expected(group);
        assert_eq!(
            exp["native"]["mcr_val"].as_i64().unwrap() as i32,
            m.mantissa,
            "{group} Native↔Wire mantissa 同次一致"
        );
        assert_eq!(
            exp["native"]["dec_val"].as_i64().unwrap() as i16,
            super::wire::RawNumeric8::decode(&m.raw)
                .expect("{group} raw 必须 8B")
                .exponent,
            "{group} Native↔Wire exponent 同次一致"
        );
        let got = to_value(&m).expect("{group} adapter 必须通过");
        match got {
            mesa_core_types::Value::F64(v) => assert!(
                (v - scaled).abs() < 1e-9,
                "{group} Mesa F64 {v} != {scaled}"
            ),
            _ => panic!("{group} 必须 F64"),
        }
    }
}

/// macro 请求：生产编码器输出 == 捕获 fixture（`encode == request` 闭环）。
/// M0~M3 四组 request 各 40B（`args=[n,n,0,0]/aux=0`，单点语义）。
fn macro_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_MACRO};
    for (group, num) in [
        ("macro0", 500),
        ("macro1", 501),
        ("macro2", 502),
        ("macro3", 503),
    ] {
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_MACRO,
                [num, num, 0, 0, 0],
            )]),
        }
        .encode();
        assert_eq!(build.len(), 40, "{group} 0x15 请求必须 40B");
        let raw = read(group, "macro_request_frame.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{group} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{group} production encoder 必须 == captured fixture 全 40B"
        );
    }
}

/// pmc scalar P0~P3：`pmc_response_frame.bin` 经生产 codec 解码 ==
/// expected（BYTE/WORD/DWORD + bit projection；Mesa BYTE→I32）。
fn pmc_scalar_decodes() {
    use super::wire::{PmcArea, PmcScalarValue, decode_pmc_scalar, pmc_scalar_to_value};
    for (group, kind, addr, want) in [
        ("pmc_r100", 'R', 100u32, mesa_core_types::Value::I32(0)),
        ("pmc_r110", 'R', 110, mesa_core_types::Value::I32(0)),
        ("pmc_x0", 'X', 0, mesa_core_types::Value::I32(0)),
        ("pmc_y0", 'Y', 0, mesa_core_types::Value::I32(4)),
        ("pmc_f0", 'F', 0, mesa_core_types::Value::I32(192)),
        ("pmc_g0", 'G', 0, mesa_core_types::Value::I32(0)),
        ("pmc_a0", 'A', 0, mesa_core_types::Value::I32(0)),
        ("pmc_t0", 'T', 0, mesa_core_types::Value::I32(0)),
        ("pmc_c0", 'C', 0, mesa_core_types::Value::I32(0)),
        ("pmc_k0", 'K', 0, mesa_core_types::Value::I32(0)),
        ("pmc_d0", 'D', 0, mesa_core_types::Value::I32(4)),
    ] {
        let area = PmcArea::from_kind(kind).expect("{group} kind 必须 canonical");
        let frame = assemble_frame(&read(group, "pmc_response_frame.bin"));
        let v = decode_pmc_scalar(&frame, area).expect("{group} 必须解码");
        assert_eq!(
            pmc_scalar_to_value(&v),
            want,
            "{group} Mesa 映射（BYTE→I32 合同）"
        );
        let exp = expected(group);
        assert_eq!(
            exp["kind"].as_str().unwrap(),
            kind.to_string(),
            "{group} kind"
        );
        assert_eq!(exp["addr"].as_u64().unwrap() as u32, addr, "{group} addr");
        // 非零点额外锁原始位型（Y0=0x04/F0=0xC0/D0=4）。
        match (group, v) {
            ("pmc_y0", PmcScalarValue::Byte(0x04))
            | ("pmc_f0", PmcScalarValue::Byte(0xC0))
            | ("pmc_d0", PmcScalarValue::Dword(4)) => {}
            ("pmc_y0" | "pmc_f0" | "pmc_d0", _) => {
                panic!("{group} 原始位型不符")
            }
            _ => {}
        }
    }
    // P1b bit projection：X0=0x00 → bit3=false（本地 mask，不进 Wire）。
    let frame = assemble_frame(&read("pmc_x0b3", "pmc_response_frame.bin"));
    let v = decode_pmc_scalar(&frame, PmcArea::from_kind('X').unwrap()).expect("x0b3 必须解码");
    assert_eq!(v, PmcScalarValue::Byte(0x00));
    let byte = 0x00u8;
    assert_eq!((byte >> 3) & 1, 0, "X0.3 必须 false");
}

/// pmc 请求：生产编码器输出 == 捕获 fixture（`encode == request` 闭环）。
/// 12 组 request 各 40B（`device=2/[start,end,adr,dt]/aux=0`）。
fn pmc_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_PMC, FUNC_PMC_READ, PmcArea};
    for (group, kind, addr) in [
        ("pmc_r100", 'R', 100u32),
        ("pmc_r110", 'R', 110),
        ("pmc_x0", 'X', 0),
        ("pmc_x0b3", 'X', 0),
        ("pmc_y0", 'Y', 0),
        ("pmc_f0", 'F', 0),
        ("pmc_g0", 'G', 0),
        ("pmc_a0", 'A', 0),
        ("pmc_t0", 'T', 0),
        ("pmc_c0", 'C', 0),
        ("pmc_k0", 'K', 0),
        ("pmc_d0", 'D', 0),
    ] {
        let area = PmcArea::from_kind(kind).expect("{group} kind 必须 canonical");
        let end = addr + (area.width() as u32).saturating_sub(1);
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_PMC,
                FUNC_PMC_READ,
                [
                    addr as i32,
                    end as i32,
                    area.adr_type() as i32,
                    area.data_type() as i32,
                    0,
                ],
            )]),
        }
        .encode();
        assert_eq!(build.len(), 40, "{group} 0x8001 请求必须 40B");
        let raw = read(group, "pmc_request_frame.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{group} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{group} production encoder 必须 == captured fixture 全 40B"
        );
    }
}

/// param Q0/Q3/P-C：`param_response_frame.bin` 经生产 codec 解码 ==
/// param Q3/P-C（identity-scale GOOD）：`param_response_frame.bin` 经生产
/// codec 解码 == expected（3411=0/6711=10027/123/456/restored；Mesa I32）。
/// Q0（`00 0a 00 03`）不在此列——它走 `param_q0_negative_evidence`，
/// fixture 保留但 decoder 必须 `Unsupported`（value=0 无辨别力，不 admit）。
fn param_decodes() {
    use super::wire::{decode_param_value, param_to_value_for_test as to_value};
    for (group, number, want) in [
        ("param_3411", 3411u32, 0),
        ("param_6711_c0", 6711, 10027),
        ("param_6711_c1", 6711, 123),
        ("param_6711_c2", 6711, 456),
        ("param_6711_c3", 6711, 10027),
    ] {
        let frame = assemble_frame(&read(group, "param_response_frame.bin"));
        let p = decode_param_value(&frame, number).expect("{group} 必须解码");
        assert_eq!(p.datano, number, "{group} datano echo");
        assert_eq!(p.value, want, "{group} value slot");
        let exp = expected(group);
        assert_eq!(
            exp["native"]["decoded"].as_i64().unwrap() as i32,
            p.value,
            "{group} Native↔Wire value 同次一致"
        );
        assert_eq!(
            to_value(&p).expect("{group} adapter 必须通过"),
            mesa_core_types::Value::I32(want),
            "{group} Mesa I32"
        );
    }
}

/// param Q0 负 evidence：fixture 保留（bytes 不动），但 decoder 必须
/// `Unsupported`（`00 0a 00 03` 未闭合为 identity；value=0 碰巧解对不算证据）。
/// 防以后因“结果碰巧还是 0”误放行。
fn param_q0_negative_evidence() {
    use super::wire::decode_param_value;
    let frame = assemble_frame(&read("param_3410", "param_response_frame.bin"));
    let e = decode_param_value(&frame, 3410).unwrap_err();
    assert!(
        matches!(e, super::WireError::Unsupported(_)),
        "Q0 tail=00 0a 00 03 必须 Unsupported，实际：{e:?}"
    );
}

/// param 请求：生产编码器输出 == evidence request fixture（`encode == request`）。
/// Q0/Q3/P-C 各 40B（`args=[n,n,0,0]/aux=0`，单点语义）。
/// NOTE：request 为 hand-built per frozen contract（非 socket capture），
/// response 为 head16 live + 264B 按 framing 重建（见 expected.json source）。
/// 此处锁“生产 encoder 与 evidence fixture 一致”，不夸大为 full capture。
fn param_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_PARAM};
    for (group, num) in [
        ("param_3410", 3410),
        ("param_3411", 3411),
        ("param_6711_c0", 6711),
        ("param_6711_c1", 6711),
        ("param_6711_c2", 6711),
        ("param_6711_c3", 6711),
    ] {
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_PARAM,
                [num, num, 0, 0, 0],
            )]),
        }
        .encode();
        assert_eq!(build.len(), 40, "{group} 0x8D 请求必须 40B");
        let raw = read(group, "param_request_frame.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{group} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{group} production encoder 必须 == evidence request fixture 全 40B"
        );
    }
}
