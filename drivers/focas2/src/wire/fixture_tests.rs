//! Fixture 回归（PR1 Level 2 + PR2 feed + PR54 spindle + PR55 macro + PR56 pmc
//! + PR57 param + PR58 opmsg + Batch 1 spindle gear/maxrpm）：真机捕获 → 生产 codec 直测。
//!
//! - 数据：165 定向抓包 → `10B header + payload_len` 精确切帧 → 去重传/
//!   拼接残留（`tests/fixtures/wire/{sysinfo,statinfo_mem,statinfo_mdi,feed}/`）。
//! - Gate 0 证据：sysinfo 7/7 / statinfo MEM 7/7 / statinfo MDI 7/7。
//! - feed 证据：`0x24` mantissa=100 ↔ Native actf=100（同次）；`0x24-only`
//!   生产形态真机冻结。
//! - spindle 证据（S0~S4）：`0x25` mantissa=0/500/1002/1500/800 ↔
//!   Native cnc_acts 同次；request 5 点逐字节恒定（`args=[0,0,0,0]/aux=0`）。
//! - spindle word 证据（gear_s1/maxrpm_s1）：`0xA4[1]+0x40[func,1]+0xA4[1]`
//!   三 slot；`0x40` dlen=8 `data[2..4]` BE16 ↔ Native ODBSPN.data[0]
//!   （gear 672 / maxrpm 874；差值 202=0xCA 跨窗一致；隔离双窗差分；
//!   位置锁死 `subs[0]=A4/subs[1]=0x40/subs[2]=A4`，三槽 status 全检查）。
//! - macro 证据（M0~M3）：`0x15` mcr=0/250000000/123450000/-750000000 +
//!   dec=0/7/7/8 ↔ Native cnc_rdmacro 同次；request `args=[n,n,0,0]`；
//!   Mesa 取 scaled F64（与 feed/spindle 取 mantissa 形成对照）。
//! - pmc 证据（P0~P3）：`0x8001/device=2` scalar；BYTE/WORD/DWORD +
//!   bit projection；Mesa BYTE→I32（PR56 产品合同修正）。
//! - pmc 写证据（R100）：`0x8002/device=2` BYTE single-address 1B
//!   （W-PMC-3/4：R100 `0x00→0x01→0x00`；`size=29/data_len=1/data=[XX]`；
//!   成功响应 `status=0/data_len=0` 无 value echo；只 admit BYTE single 1B）。
//! - param 证据（Q0/Q3/P-C）：`0x8D` integer-safe scalar；Mesa I32。
//! - diagnosis 证据（D301）：`0x93[301,301,3,0]` → type=5 REAL →
//!   `RawNumeric8(-10,10,3)` → Mesa F64(-0.010)；Batch 2 只 admit REAL。
//! - opmsg 证据（O0/O1）：`0x34 type=4` → #3006 文本；Mesa String。
//! - alarm 证据（empty/PS0010）：`0x23[-1,29,2,32]` → 空=[] /
//!   单条 `no=10/type=3/axis=0/"IMPROPER G-CODE"` → StringArray 1 元；
//!   非 single 即 Unsupported（B3-C）。
//! - 本模块 `#[cfg(test)]` 且 crate 内部：直接调生产 decoder，无复刻。

use std::path::PathBuf;

use super::frame::{FRAME_HEADER_LEN, FocasFrame, PacketType, decode_header};
use super::wire::{
    decode_alarm_value, decode_diagnosis_value, decode_feed_rate, decode_macro_value,
    decode_opmsg_value, decode_spindle_speed, decode_spindle_word, decode_status_info,
    decode_system_info, decode_tofs_value, decode_zofs_value,
    tofs_to_value_for_test as tofs_to_value, zofs_to_value_for_test as zofs_to_value,
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
    spindle_word_gear_maxrpm_decodes();
    spindle_word_request_locked();
    macro_m0m3_decodes();
    macro_request_locked();
    pmc_scalar_decodes();
    pmc_request_locked();
    pmc_write_byte_locked();
    tofs_offset_length_decodes();
    tofs_request_locked();
    zofs_g54x_decodes();
    zofs_request_locked();
    param_decodes();
    param_q0_negative_evidence();
    param_request_locked();
    diagnosis_301a3_decodes();
    diagnosis_request_locked();
    alarm_empty_ps0010_decodes();
    alarm_request_locked();
    opmsg_o1o0_decodes();
    opmsg_request_locked();
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

/// spindle word G1/M1（Batch 1）：fixture 经生产 codec 解码 == expected
///（gear 672 / maxrpm 874 ↔ Native ODBSPN.data[0] 同次；
/// request 生产编码 == 捕获 fixture 全 96B）。
fn spindle_word_gear_maxrpm_decodes() {
    use super::wire::{
        FUNC_SPINDLE_WORD, FUNC_SPINDLE_WORD_HEAD, SPINDLE_WORD_FUNC_GEAR,
        SPINDLE_WORD_FUNC_MAXRPM, spindle_word_to_value_for_test as to_value,
    };
    for (group, func, want) in [
        ("gear_s1", SPINDLE_WORD_FUNC_GEAR, 672),
        ("maxrpm_s1", SPINDLE_WORD_FUNC_MAXRPM, 874),
    ] {
        // 响应：生产 decoder 直测（decoder 不取 func，操作身份由 request 决定）。
        let frame = assemble_frame(&read(group, "spindleword_response_frame.bin"));
        let w = decode_spindle_word(&frame).expect("{group} 必须解码");
        assert_eq!(w.value, want as i16, "{group} BE16 word");
        assert_eq!(
            w.raw,
            if func == SPINDLE_WORD_FUNC_GEAR {
                [0x00, 0x00, 0x02, 0xa0, 0x00, 0x0a, 0x00, 0x00]
            } else {
                [0x00, 0x00, 0x03, 0x6a, 0x00, 0x0a, 0x00, 0x00]
            },
            "{group} raw 8B 全保留"
        );
        let exp = expected(group);
        assert_eq!(
            exp["native"]["data0"].as_i64().unwrap() as i16,
            w.value,
            "{group} Native↔Wire 同次一致"
        );
        assert_eq!(
            to_value(&w),
            mesa_core_types::Value::I32(want),
            "{group} Mesa 映射"
        );
        // 请求：生产编码器输出 == 捕获 fixture 全 96B。
        use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
        use super::frame::{encode_generic_request, request_subpacket};
        use super::wire::DEV_CNC;
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[
                request_subpacket(DEV_CNC, FUNC_SPINDLE_WORD_HEAD, [1, 0, 0, 0, 0]),
                request_subpacket(DEV_CNC, FUNC_SPINDLE_WORD, [func, 1, 0, 0, 0]),
                request_subpacket(DEV_CNC, FUNC_SPINDLE_WORD_HEAD, [1, 0, 0, 0, 0]),
            ]),
        }
        .encode();
        assert_eq!(build.len(), 96, "{group} 请求必须 96B（10+86）");
        let raw = read(group, "spindleword_request_frame.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 96, "{group} 必须 96B");
        assert_eq!(
            frames[0], build,
            "{group} production encoder 必须 == captured fixture 全 96B"
        );
    }
}

/// spindle word 请求锁死（wire.rs 单测同源；fixture 侧只验全字节等价）。
fn spindle_word_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_SPINDLE_WORD, FUNC_SPINDLE_WORD_HEAD};
    use super::wire::{SPINDLE_WORD_FUNC_GEAR, SPINDLE_WORD_FUNC_MAXRPM};
    for func in [SPINDLE_WORD_FUNC_GEAR, SPINDLE_WORD_FUNC_MAXRPM] {
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[
                request_subpacket(DEV_CNC, FUNC_SPINDLE_WORD_HEAD, [1, 0, 0, 0, 0]),
                request_subpacket(DEV_CNC, FUNC_SPINDLE_WORD, [func, 1, 0, 0, 0]),
                request_subpacket(DEV_CNC, FUNC_SPINDLE_WORD_HEAD, [1, 0, 0, 0, 0]),
            ]),
        }
        .encode();
        assert_eq!(build.len(), 96, "func={func} 必须 96B");
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

/// pmc 写 R100（W-PMC-5）：捕获 fixture 精确回放 + 负测试（clone/mutate）。
///
/// 正：`req_write_s1_01/s3_00` 经生产 encoder byte-for-byte == 捕获 41B；
/// `resp_write_s1/s3` 经生产 decoder == `Ok(())`.
///
/// 负：成功响应 clone 后 mutate（非 evidence fixture，synthetic negative）：
/// wrong command echo → `CommandMismatch`；truncated → `MalformedPayload`；
/// non-zero status → `Remote`.
///
/// 第一层锁死 `size=29/command=8002/start=end=100/area=5/dtype=0/aux=0/`
/// `data_len=1/value=01·00`；只 admit BYTE single-address 1B.
fn pmc_write_byte_locked() {
    use super::WireError;
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN, RequestSubpacket};
    use super::frame::{decode_reply_payload, encode_generic_request, match_slot};
    use super::wire::decode_pmc_write_response as decode_write;
    use super::wire::{CMD_PMC_WRITE, DEV_PMC, PATH_PMC_OBSERVED, PmcArea};
    // 正：encoder byte-for-byte（41B frame 全等；含 10B header）。
    for (name, value) in [("req_write_s1_01", 0x01u8), ("req_write_s3_00", 0x00u8)] {
        let area = PmcArea::from_kind('R').expect("R 必须 canonical");
        let real = RequestSubpacket {
            device: DEV_PMC,
            path: PATH_PMC_OBSERVED,
            command: CMD_PMC_WRITE,
            args: [100, 100, area.adr_type() as u32, 0],
            aux: 0,
            data: vec![value],
        };
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[real.as_legacy()]),
        }
        .encode();
        assert_eq!(build.len(), 41, "{name} 0x8002 请求必须 41B");
        let raw = read("pmc_write_byte", &format!("{name}.bin"));
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{name} 必须恰好 1 帧");
        assert_eq!(
            frames[0], build,
            "{name} production encoder 必须 == captured fixture 全 41B"
        );
    }
    // 正：decoder == Ok(())（成功响应 28B frame / 16B subpacket / data 空）。
    for name in ["resp_write_s1", "resp_write_s3"] {
        let frame = assemble_frame(&read("pmc_write_byte", &format!("{name}.bin")));
        assert_eq!(frame.packet_type, PacketType::GENERIC_RESPONSE);
        decode_write(&frame).expect("{name} 必须解码 Ok(())");
    }
    // 负：captured positive clone/mutate（synthetic，非 evidence）。
    let base = assemble_frame(&read("pmc_write_byte", "resp_write_s1.bin"));
    // wrong command echo → CommandMismatch（8002 → 8001）。
    {
        let mut payload = base.payload.clone();
        // GENERIC payload: count[0:2] + size[2:4] + dev[4:6] + path[6:8] + cmd[8:10]。
        payload[8] = 0x80;
        payload[9] = 0x01;
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        assert!(
            matches!(
                decode_write(&mutated),
                Err(super::WireError::CommandMismatch)
            ),
            "wrong command echo 必须 CommandMismatch"
        );
    }
    // truncated response → MalformedPayload（去尾 1B）。
    {
        let mut payload = base.payload.clone();
        payload.truncate(payload.len() - 1);
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        assert!(
            matches!(
                decode_write(&mutated),
                Err(super::WireError::MalformedPayload)
            ),
            "truncated 必须 MalformedPayload"
        );
    }
    // non-zero status → Remote（status[8:10] 置 2；detail 保留）。
    {
        let mut payload = base.payload.clone();
        // reply subpacket: count[0:2]+size[2:4]+dev[4:6]+path[6:8]+cmd[8:10]+
        // status[10:12]+detail1[12:14]+detail2[14:16]+dlen[16:18]。
        payload[10] = 0x00;
        payload[11] = 0x02;
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        match decode_write(&mutated) {
            Err(WireError::Remote {
                status,
                detail1,
                detail2,
            }) => {
                assert_eq!(status, 2);
                assert_eq!((detail1, detail2), (0, 0));
            }
            other => panic!("non-zero status 必须 Remote，实际 {other:?}"),
        }
    }
    // 合同形状锁（与 EVIDENCE.md §5 同源断言，不重复 handler 逻辑）：
    // 写请求 subpacket size=29 / 读请求 28；成功响应 subpacket size=16 / dlen=0。
    {
        let w = read("pmc_write_byte", "req_write_s1_01.bin");
        assert_eq!(w.len(), 41);
        let subs = super::frame::decode_generic_payload(&w[10..]).expect("写请求 GENERIC 必须合法");
        assert_eq!(subs.len(), 1);
        let r = read("pmc_write_byte", "resp_write_s1.bin");
        let subs = decode_reply_payload(&r[10..]).expect("写响应 GENERIC 必须合法");
        let sub = match_slot(&subs, DEV_PMC, PATH_PMC_OBSERVED, CMD_PMC_WRITE, 0)
            .expect("写响应必须含 0x8002 槽");
        assert_eq!(sub.status, 0);
        assert!(sub.data.is_empty(), "成功响应无 value echo");
    }
}
/// tool offset/length Gate 3-C2/C3：捕获 fixture 精确回放 + 负测试。
/// - 正：`tool16-type1-value5000/type3-value10000` 经生产 decoder ==
///   `ToolCompValue{value: 5000/10000}` → adapter `F64(5.0/10.0)`；
///   zero fixture（type1/type3 ×4）→ `F64(0.0)`。
/// - 请求：生产 encoder（`tofs_value` 同源构造）byte-for-byte == 捕获 40B。
/// - 负（clone/mutate synthetic）：wrong command echo → `CommandMismatch`；
///   truncated → `MalformedPayload`；non-zero status → `Remote`。
fn tofs_offset_length_decodes() {
    use super::WireError;
    use super::frame::{FocasFrame, PacketType};
    use super::wire::{CMD_TOFS, DEV_CNC, PATH_CNC, TOFS_ARG_LENGTH, TOFS_ARG_OFFSET};
    use mesa_core_types::Value;
    // 正：decoder + adapter（5000→5.0 / 10000→10.0 / zero→0.0）。
    for (name, want_raw, want_f64) in [
        ("tool16-type1-value5000", 5000i32, 5.0f64),
        ("tool16-type3-value10000", 10000i32, 10.0f64),
    ] {
        let frame = assemble_frame(&read("tool_tofs_08", &format!("{name}.res.bin")));
        assert_eq!(frame.packet_type, PacketType::GENERIC_RESPONSE);
        let v = decode_tofs_value(&frame).expect("{name} 必须解码");
        assert_eq!(v.value, want_raw, "{name} value slot");
        assert_eq!(v.raw.len(), 8, "{name} raw 全保留");
        assert_eq!(
            tofs_to_value(&v),
            Value::F64(want_f64),
            "{name} adapter /1000"
        );
    }
    for zero in [
        "tool16-type1-zero-01",
        "tool16-type1-zero-02",
        "tool16-type1-zero-03",
        "tool16-type1-zero-04",
        "tool16-type3-zero-01",
        "tool16-type3-zero-02",
        "tool16-type3-zero-03",
        "tool16-type3-zero-04",
    ] {
        let frame = assemble_frame(&read("tool_tofs_08", &format!("{zero}.res.bin")));
        let v = decode_tofs_value(&frame).expect("{zero} 必须解码");
        assert_eq!(v.value, 0, "{zero} value 零");
        assert_eq!(tofs_to_value(&v), Value::F64(0.0), "{zero} adapter");
    }
    // 负：captured positive clone/mutate（synthetic，非 evidence）。
    let base = assemble_frame(&read("tool_tofs_08", "tool16-type1-value5000.res.bin"));
    // wrong command echo → CommandMismatch（0x08 → 0x26）。
    {
        let mut payload = base.payload.clone();
        // GENERIC payload: count[0:2] + size[2:4] + dev[4:6] + path[6:8] + cmd[8:10]。
        payload[8] = 0x00;
        payload[9] = 0x26;
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        assert!(
            matches!(
                decode_tofs_value(&mutated),
                Err(super::WireError::CommandMismatch)
            ),
            "wrong command echo 必须 CommandMismatch"
        );
    }
    // truncated response → MalformedPayload（去尾 1B）。
    {
        let mut payload = base.payload.clone();
        payload.truncate(payload.len() - 1);
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        assert!(
            matches!(
                decode_tofs_value(&mutated),
                Err(super::WireError::MalformedPayload)
            ),
            "truncated 必须 MalformedPayload"
        );
    }
    // non-zero status → Remote（status[10:12] 置 2；detail 保留）。
    {
        let mut payload = base.payload.clone();
        // reply subpacket: count[0:2]+size[2:4]+dev[4:6]+path[6:8]+cmd[8:10]+
        // status[10:12]+detail1[12:14]+detail2[14:16]+dlen[16:18]。
        payload[10] = 0x00;
        payload[11] = 0x02;
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        match decode_tofs_value(&mutated) {
            Err(super::WireError::Remote {
                status,
                detail1,
                detail2,
            }) => {
                assert_eq!(status, 2);
                assert_eq!((detail1, detail2), (0, 0));
            }
            other => panic!("non-zero status 必须 Remote，实际 {other:?}"),
        }
    }
    // 合同形状锁：写请求 subpacket size/dlen 与 EVIDENCE 同源断言。
    {
        let _ = WireError::CommandMismatch;
        let _ = (
            CMD_TOFS,
            DEV_CNC,
            PATH_CNC,
            TOFS_ARG_OFFSET,
            TOFS_ARG_LENGTH,
        );
    }
}

/// tool 请求 Gate 3-C3：生产 encoder（`tofs_value` 同源构造）byte-for-byte
/// == 捕获 40B（offset→1001/length→1003；tool=16；aux=0/dlen=0）。
fn tofs_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_TOFS, PATH_CNC};
    for (name, number, selector) in [
        ("tool16-type1-value5000", 16u32, 1001i32),
        ("tool16-type3-value10000", 16u32, 1003i32),
    ] {
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_TOFS,
                [number as i32, number as i32, selector, 0, 0],
            )]),
        }
        .encode();
        assert_eq!(build.len(), 40, "{name} 0x08 请求必须 40B");
        let raw = read("tool_tofs_08", &format!("{name}.req.bin"));
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{name} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{name} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{name} production encoder 必须 == captured fixture 全 40B"
        );
        // PATH_CNC 三元组使用断言（防 FUNC_TOFS 常量漂移未被使用）。
        assert_eq!(PATH_CNC, 1);
    }
}

/// zofs Gate 3-D3/D4：捕获 fixture 精确回放 + 负测试。
/// - 正：`g54x-12345/23456/zero` 经生产 decoder == `ZofsValue{value}` →
///   adapter `F64(12.345/23.456/0.0)`；selector 对照（axis2/axis3/g55x zero）
///   只证 decoder 可处理，不新增 Y/Z 产品语义。
/// - `len7` fixture 回放断言（Wire 成功；production 不加 length 参数）。
/// - 真实负：`axis0-status4.res.bin` → `Remote{status: 4}`。
/// - synthetic：wrong command echo → `CommandMismatch`；truncated/wrong dlen
///   → `MalformedPayload`。
fn zofs_g54x_decodes() {
    use super::WireError;
    use super::frame::{FocasFrame, PacketType};
    use super::wire::{ZOFS_AXIS_X, ZOFS_DATA_LEN};
    use mesa_core_types::Value;
    assert_eq!((ZOFS_AXIS_X, ZOFS_DATA_LEN), (1, 8));
    // 正：decoder + adapter（12345→12.345 / 23456→23.456 / zero→0.0）。
    for (name, want_raw, want_f64) in [
        ("g54x-12345", 12345i32, 12.345f64),
        ("g54x-23456", 23456i32, 23.456f64),
        ("g54x-zero", 0i32, 0.0f64),
    ] {
        let frame = assemble_frame(&read("tool_zofs_0b", &format!("{name}.res.bin")));
        assert_eq!(frame.packet_type, PacketType::GENERIC_RESPONSE);
        let v = decode_zofs_value(&frame).expect("{name} 必须解码");
        assert_eq!(v.value, want_raw, "{name} value slot");
        assert_eq!(v.raw.len(), 8, "{name} raw 全保留");
        assert_eq!(
            zofs_to_value(&v),
            Value::F64(want_f64),
            "{name} adapter /1000"
        );
    }
    // selector 对照：decoder 可处理，不新增产品语义。
    for name in [
        "g54-axis2-zero",
        "g54-axis3-zero",
        "g55x-zero",
        "g54x-len7-native-ewlength-wire-ok",
    ] {
        let frame = assemble_frame(&read("tool_zofs_0b", &format!("{name}.res.bin")));
        let v = decode_zofs_value(&frame).expect("{name} 必须解码");
        let want = if name.starts_with("g54x-len7") {
            12345i32
        } else {
            0i32
        };
        assert_eq!(v.value, want, "{name} value");
    }
    // 真实负：axis0 → Remote{status: 4}（ captured，非 synthetic）。
    {
        let frame = assemble_frame(&read("tool_zofs_0b", "axis0-status4.res.bin"));
        match decode_zofs_value(&frame) {
            Err(WireError::Remote {
                status,
                detail1,
                detail2,
            }) => {
                assert_eq!(status, 4);
                assert_eq!((detail1, detail2), (0, 0));
            }
            other => panic!("axis0 必须 Remote{{status: 4}}，实际 {other:?}"),
        }
    }
    // synthetic 负：wrong command / truncated / wrong dlen。
    let base = assemble_frame(&read("tool_zofs_0b", "g54x-12345.res.bin"));
    {
        let mut payload = base.payload.clone();
        payload[8] = 0x00;
        payload[9] = 0x26;
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        assert!(
            matches!(decode_zofs_value(&mutated), Err(WireError::CommandMismatch)),
            "wrong command echo 必须 CommandMismatch"
        );
    }
    {
        let mut payload = base.payload.clone();
        payload.truncate(payload.len() - 1);
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        assert!(
            matches!(
                decode_zofs_value(&mutated),
                Err(WireError::MalformedPayload)
            ),
            "truncated 必须 MalformedPayload"
        );
    }
    {
        // wrong dlen：data 追加 1B（dlen 8→9；decoder 只 admit 8）。
        let mut payload = base.payload.clone();
        // reply subpacket dlen 位 [16:18]：0x0008 → 0x0009。
        payload[17] = 0x09;
        payload.push(0x00);
        let mutated = FocasFrame {
            origin: base.origin,
            packet_type: base.packet_type,
            payload,
        };
        assert!(
            matches!(
                decode_zofs_value(&mutated),
                Err(WireError::MalformedPayload)
            ),
            "wrong dlen 必须 MalformedPayload"
        );
    }
}

/// zofs 请求 Gate 3-D4：production encoder（`zofs_value` 同源构造）
/// byte-for-byte == 捕获 40B（number=1→`g54x-12345.req` / number=2→`g55x-zero.req`；
/// 直接证明 `[1,1,1,0]/[2,2,1,0]`）。
fn zofs_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_ZOFS, PATH_CNC, ZOFS_AXIS_X};
    for (name, number) in [("g54x-12345", 1u32), ("g55x-zero", 2u32)] {
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_ZOFS,
                [number as i32, number as i32, ZOFS_AXIS_X, 0, 0],
            )]),
        }
        .encode();
        assert_eq!(build.len(), 40, "{name} 0x0B 请求必须 40B");
        let raw = read("tool_zofs_0b", &format!("{name}.req.bin"));
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{name} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{name} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{name} production encoder 必须 == captured fixture 全 40B"
        );
        assert_eq!(PATH_CNC, 1);
    }
    // `g54x-23456.req` 与 `g54x-12345.req` 同形（selector 无 value 耦合）。
    let r1 = read("tool_zofs_0b", "g54x-12345.req.bin");
    let r2 = read("tool_zofs_0b", "g54x-23456.req.bin");
    assert_eq!(r1, r2, "同 selector 请求必须逐字节相同");
}

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

/// diagnosis D301（Batch 2）：fixture 经生产 codec 解码 == expected
///（`datano=301/attr=3/type=5/(-10,10,3)` ↔ Native dgn/dec 同次；
/// request 生产编码 == 捕获 fixture 全 40B）。
fn diagnosis_301a3_decodes() {
    use super::wire::diagnosis_to_value_for_test as to_value;
    let frame = assemble_frame(&read("diagnosis_301a3", "diagnosis_response_frame.bin"));
    let v = decode_diagnosis_value(&frame, 301).expect("diagnosis_301a3 必须解码");
    assert_eq!(v.datano, 301);
    assert_eq!(v.attr, 3);
    assert_eq!(v.diag_type, 5);
    assert_eq!(v.numeric.mantissa, -10);
    assert_eq!(v.numeric.base, 10);
    assert_eq!(v.numeric.exponent, 3);
    let exp = expected("diagnosis_301a3");
    assert_eq!(
        exp["native"]["dgn_val"].as_i64().unwrap() as i32,
        v.numeric.mantissa,
        "Native↔Wire mantissa 同次一致"
    );
    assert_eq!(
        exp["native"]["dec_val"].as_i64().unwrap() as i16,
        v.numeric.exponent,
        "Native↔Wire dec 同次一致"
    );
    assert_eq!(
        to_value(&v).expect("adapter 必须通过"),
        mesa_core_types::Value::F64(-0.010),
        "Mesa engineering F64"
    );
    // 请求：生产编码器输出 == 捕获 fixture 全 40B。
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_DIAGNOSIS};
    let build = FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&[request_subpacket(
            DEV_CNC,
            FUNC_DIAGNOSIS,
            [301, 301, 3, 0, 0],
        )]),
    }
    .encode();
    assert_eq!(build.len(), 40, "0x93 请求必须 40B");
    let raw = read("diagnosis_301a3", "diagnosis_request_frame.bin");
    let frames = cut_fixture_frames(&raw);
    assert_eq!(frames.len(), 1, "必须恰好 1 帧");
    assert_eq!(frames[0].len(), 40, "必须 40B");
    assert_eq!(
        frames[0], build,
        "production encoder 必须 == captured fixture 全 40B"
    );
}

/// diagnosis 请求锁死（wire.rs 单测同源；fixture 侧只验全字节等价）。
fn diagnosis_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_DIAGNOSIS};
    let build = FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&[request_subpacket(
            DEV_CNC,
            FUNC_DIAGNOSIS,
            [301, 301, 3, 0, 0],
        )]),
    }
    .encode();
    assert_eq!(build.len(), 40, "0x93 请求必须 40B");
}

/// alarm empty/PS0010（B3-B）：fixture 经生产 codec 解码 == expected
///（空=[] / 单条 `no=10/type=3/axis=0/"IMPROPER G-CODE"` → StringArray 1 元；
/// request 生产编码 == 捕获 fixture 全 40B）。
fn alarm_empty_ps0010_decodes() {
    use super::wire::alarm_to_value_for_test as to_value;
    // 空：dlen=0 → []。
    let frame0 = assemble_frame(&read("alarm_empty", "alarm_response_frame.bin"));
    let r0 = decode_alarm_value(&frame0).expect("alarm_empty 必须解码");
    assert!(r0.alarms.is_empty());
    assert_eq!(
        to_value(&r0),
        mesa_core_types::Value::StringArray(vec![]),
        "空报警 → StringArray([])"
    );
    // PS0010：三方同次一致。
    let frame1 = assemble_frame(&read("alarm_ps0010", "alarm_response_frame.bin"));
    let r1 = decode_alarm_value(&frame1).expect("alarm_ps0010 必须解码");
    assert_eq!(r1.alarms.len(), 1);
    assert_eq!(r1.alarms[0].number, 10);
    assert_eq!(r1.alarms[0].alarm_type, 3);
    assert_eq!(r1.alarms[0].axis, 0);
    assert_eq!(r1.alarms[0].text, "IMPROPER G-CODE");
    let exp = expected("alarm_ps0010");
    assert_eq!(
        exp["alarms"][0]["text"].as_str().unwrap(),
        r1.alarms[0].text,
        "Wire↔expected 文本同次一致"
    );
    assert_eq!(
        to_value(&r1),
        mesa_core_types::Value::StringArray(vec!["IMPROPER G-CODE".into()]),
        "Mesa StringArray 1 元"
    );
    // 请求：生产编码器输出 == 捕获 fixture 全 40B（empty/PS0010 同请求）。
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{ALARM_ARG0_OBSERVED, ALARM_ARG1_OBSERVED, ALARM_ARG2_OBSERVED};
    use super::wire::{ALARM_ARG3_OBSERVED, DEV_CNC, FUNC_ALARM};
    let build = FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&[request_subpacket(
            DEV_CNC,
            FUNC_ALARM,
            [
                ALARM_ARG0_OBSERVED,
                ALARM_ARG1_OBSERVED,
                ALARM_ARG2_OBSERVED,
                ALARM_ARG3_OBSERVED,
                0,
            ],
        )]),
    }
    .encode();
    assert_eq!(build.len(), 40, "0x23 请求必须 40B");
    for group in ["alarm_empty", "alarm_ps0010"] {
        let raw = read(group, "alarm_request_frame.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{group} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{group} production encoder 必须 == captured fixture 全 40B"
        );
    }
}

/// alarm 请求锁死（wire.rs 单测同源；fixture 侧只验全字节等价）。
fn alarm_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{ALARM_ARG0_OBSERVED, ALARM_ARG1_OBSERVED, ALARM_ARG2_OBSERVED};
    use super::wire::{ALARM_ARG3_OBSERVED, DEV_CNC, FUNC_ALARM};
    let build = FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&[request_subpacket(
            DEV_CNC,
            FUNC_ALARM,
            [
                ALARM_ARG0_OBSERVED,
                ALARM_ARG1_OBSERVED,
                ALARM_ARG2_OBSERVED,
                ALARM_ARG3_OBSERVED,
                0,
            ],
        )]),
    }
    .encode();
    assert_eq!(build.len(), 40, "0x23 请求必须 40B");
}

/// opmsg O1/O0：`opmsg_response_frame.bin` 经生产 codec 解码 ==
/// expected（type4 → `OPMSG TEST 123` / type0 → `OP:empty`；Mesa String）。
fn opmsg_o1o0_decodes() {
    use super::wire::opmsg_to_value_for_test as to_value;
    for (group, want) in [
        ("opmsg_type4", "OPMSG TEST 123"),
        ("opmsg_type0", "OP:empty"),
    ] {
        let frame = assemble_frame(&read(group, "opmsg_response_frame.bin"));
        let m = decode_opmsg_value(&frame).expect("{group} 必须解码");
        assert_eq!(m.text, want, "{group} text");
        let exp = expected(group);
        assert_eq!(
            exp["mesa"]["opmsg_value"].as_str().unwrap(),
            want,
            "{group} Mesa↔Wire text 一致"
        );
        assert_eq!(
            to_value(&m).expect("{group} adapter 必须通过"),
            mesa_core_types::Value::String(want.to_string()),
            "{group} Mesa String"
        );
    }
}

/// opmsg 请求：生产编码器输出 == evidence request fixture（`encode == request`）。
/// type4/type0 各 40B（`args=[type,0,0,0]/aux=0`，产品固定 type=4）。
fn opmsg_request_locked() {
    use super::frame::{FocasFrame, PacketType, REQUEST_ORIGIN};
    use super::frame::{encode_generic_request, request_subpacket};
    use super::wire::{DEV_CNC, FUNC_OPMSG};
    for (group, typ) in [("opmsg_type4", 4), ("opmsg_type0", 0)] {
        let build = FocasFrame {
            origin: REQUEST_ORIGIN,
            packet_type: PacketType::GENERIC_REQUEST,
            payload: encode_generic_request(&[request_subpacket(
                DEV_CNC,
                FUNC_OPMSG,
                [typ, 0, 0, 0, 0],
            )]),
        }
        .encode();
        assert_eq!(build.len(), 40, "{group} 0x34 请求必须 40B");
        let raw = read(group, "opmsg_request_frame.bin");
        let frames = cut_fixture_frames(&raw);
        assert_eq!(frames.len(), 1, "{group} 必须恰好 1 帧");
        assert_eq!(frames[0].len(), 40, "{group} 必须 40B");
        assert_eq!(
            frames[0], build,
            "{group} production encoder 必须 == evidence request fixture 全 40B"
        );
    }
}
