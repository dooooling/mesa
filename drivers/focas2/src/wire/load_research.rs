//! 负载差分取证入口：实际 DLL ABI、原始字段与正式采集百分比比较。

use super::WireError;
use super::frame::{
    FocasFrame, PacketType, REQUEST_ORIGIN, encode_generic_request, request_subpacket,
};
use super::wire::{
    CMD_LOAD_HEAD, CMD_SPINDLE_METER, CMD_SPINDLE_NAMES, DEV_CNC, SPINDLE_METER_FUNC_LOAD,
    SPINDLE_METER_FUNC_SPEED,
};
use super::wire::{CMD_SERVO_LOAD, CMD_SERVO_LOAD_NAMES};

/// A4 的第一参数选择计数种类，独立于 Native num_in；这里故意不接收数量参数。
/// fwlibe64 RVA 0x58824 / 表 0xFA2A0 确认主轴选择 1；type=-1 在同批次放两个 40。
pub(super) fn spindle_request(selector: i32) -> Result<FocasFrame, WireError> {
    if ![-1, 0, 1].contains(&selector) {
        return Err(WireError::Unsupported("spindle meter type 0/1/-1"));
    }
    let f = |command| (1u32 << 16) | command as u32;
    let mut subs = vec![
        request_subpacket(DEV_CNC, f(CMD_LOAD_HEAD), [1, 0, 0, 0, 0]),
        request_subpacket(DEV_CNC, f(CMD_SPINDLE_NAMES), [0; 5]),
    ];
    for (selected, function) in [(0, SPINDLE_METER_FUNC_LOAD), (1, SPINDLE_METER_FUNC_SPEED)] {
        if selector == -1 || selector == selected {
            subs.push(request_subpacket(
                DEV_CNC,
                f(CMD_SPINDLE_METER),
                [function, -1, 0, 0, 0],
            ));
        }
    }
    subs.push(request_subpacket(
        DEV_CNC,
        f(CMD_LOAD_HEAD),
        [1, 0, 0, 0, 0],
    ));
    Ok(FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&subs),
    })
}

/// 伺服 A4 固定选择 2，负载 selector 为 1；调用者数量仅限制响应解码。
fn servo_request() -> FocasFrame {
    let f = |command| (1u32 << 16) | command as u32;
    let subs = [
        request_subpacket(DEV_CNC, f(CMD_LOAD_HEAD), [2, 0, 0, 0, 0]),
        request_subpacket(DEV_CNC, f(CMD_SERVO_LOAD_NAMES), [0; 5]),
        request_subpacket(DEV_CNC, f(CMD_SERVO_LOAD), [1, 0, 0, 0, 0]),
        request_subpacket(DEV_CNC, f(CMD_LOAD_HEAD), [2, 0, 0, 0, 0]),
    ];
    FocasFrame {
        origin: REQUEST_ORIGIN,
        packet_type: PacketType::GENERIC_REQUEST,
        payload: encode_generic_request(&subs),
    }
}

/// 直接调用现有解码器，将已选择的半区转换成可与 Native ABI 字段比较的研究记录。
/// 网络原始帧另存，不在这里归一化轴名或把辅助字节解释成单位。
fn wire_candidates(
    response: &FocasFrame,
    selector: i32,
    servo: bool,
) -> Result<Vec<serde_json::Value>, WireError> {
    use serde_json::json;
    let mut candidates = Vec::new();
    if servo {
        let decoded = super::wire::decode_servo_load_new(response, 4)?;
        for (i, r) in decoded.records.iter().enumerate() {
            candidates.push(json!({"slot":i,"data":r.numeric.raw,
                "decimal":r.numeric.dec_bits,"unit":0,"name":r.axis_raw}));
        }
    } else {
        let decoded = super::wire::decode_spindle_meter(response, selector, 4)?;
        for (i, r) in decoded.records.iter().enumerate() {
            for (side, n) in [(0, r.load), (1, r.speed)] {
                if let Some(n) = n {
                    let (data, _) = n.to_spindle_abi()?;
                    candidates.push(json!({"slot":i*2+side,"data":data,
                        "decimal":n.dec_bits,"unit":side,"name":r.name_raw}));
                }
            }
        }
    }
    Ok(candidates)
}

#[cfg(windows)]
pub(crate) mod live {
    use super::*;
    use crate::native::load_evidence::{GuardedLoadBuffer, LOAD_SENTINEL, decode_load_elems};
    use crate::native::{NativeLib, OdbSpLoad, OdbSvLoad};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use std::time::Duration;

    /// 正常和异常退出都释放 Native 句柄；不在析构中二次 panic。
    struct NativeHandle<'a>(&'a NativeLib, u16);
    impl Drop for NativeHandle<'_> {
        fn drop(&mut self) {
            let _ = self.0.cnc_freelibhndl(self.1);
        }
    }

    /// 使用当前进程模块枚举记录实际加载路径，而不是把仓库文件清单当加载证明。
    /// 固定容量并校验截断；记录的是调用后的快照，不能证明调用内已卸载的短暂模块。
    pub(crate) fn loaded_modules() -> Value {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
            fn K32EnumProcessModules(
                process: *mut c_void,
                modules: *mut *mut c_void,
                bytes: u32,
                needed: *mut u32,
            ) -> i32;
            fn K32GetModuleFileNameExW(
                process: *mut c_void,
                module: *mut c_void,
                name: *mut u16,
                size: u32,
            ) -> u32;
        }
        let mut modules = [std::ptr::null_mut(); 256];
        let mut needed = 0;
        // 缓冲容量以字节传入 Windows API，返回值必须先检查再索引。
        let process = unsafe { GetCurrentProcess() };
        let ok = unsafe {
            K32EnumProcessModules(
                process,
                modules.as_mut_ptr(),
                std::mem::size_of_val(&modules) as u32,
                &mut needed,
            )
        };
        assert!(
            ok != 0 && needed as usize <= std::mem::size_of_val(&modules),
            "模块枚举失败或截断"
        );
        let mut rows = Vec::new();
        for module in &modules[..needed as usize / std::mem::size_of::<*mut c_void>()] {
            let mut name = vec![0u16; 32768];
            let len = unsafe {
                K32GetModuleFileNameExW(process, *module, name.as_mut_ptr(), name.len() as u32)
            } as usize;
            assert!(len > 0 && len < name.len(), "模块路径获取失败或截断");
            let path = String::from_utf16(&name[..len]).expect("模块路径不是合法 UTF-16");
            if path.to_ascii_lowercase().contains("fwlib") {
                let data = std::fs::read(&path).expect("读取已加载 DLL 以计算哈希");
                rows.push(json!({"path":path,"sha256":format!("{:x}",Sha256::digest(&data))}));
            }
        }
        assert!(!rows.is_empty(), "未枚举到 FOCAS DLL");
        json!(rows)
    }

    /// 按当前进程 PID 记录 TCP 四元组，抓包对账只采用这些连接，排除旁路采集流量。
    fn connections() -> Value {
        use std::ffi::c_void;
        #[link(name = "iphlpapi")]
        unsafe extern "system" {
            fn GetExtendedTcpTable(
                table: *mut c_void,
                size: *mut u32,
                ordered: i32,
                family: u32,
                class: u32,
                reserved: u32,
            ) -> u32;
        }
        // TCP_TABLE_OWNER_PID_ALL 的每行是 6 个 DWORD；固定有界容量，拒绝静默截断。
        let mut table = vec![0u32; 16384];
        let mut bytes = std::mem::size_of_val(table.as_slice()) as u32;
        let rc = unsafe { GetExtendedTcpTable(table.as_mut_ptr().cast(), &mut bytes, 0, 2, 5, 0) };
        assert_eq!(rc, 0, "TCP 表枚举失败，不能确认流归属");
        let count = table[0] as usize;
        assert!(
            count * 6 < table.len() && (1 + count * 6) * 4 <= bytes as usize,
            "TCP 表长度不闭合"
        );
        let port = |n: u32| u16::from_be_bytes((n as u16).to_ne_bytes());
        let rows: Vec<_> = table[1..1 + count * 6]
            .chunks_exact(6)
            .filter(|r| r[5] == std::process::id())
            .map(|r| {
                json!({"pid":r[5],"state":r[0],
                "connection":[std::net::Ipv4Addr::from(r[1].to_ne_bytes()).to_string(),port(r[2]),
                    std::net::Ipv4Addr::from(r[3].to_ne_bytes()).to_string(),port(r[4])] })
            })
            .collect();
        json!(rows)
    }

    fn save(out: &Path, name: &str, value: &Value) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(out.join(name))
            .expect("证据文件已存在或无法创建，拒绝覆盖");
        file.write_all(&serde_json::to_vec_pretty(value).unwrap())
            .unwrap();
    }

    fn native_sample(
        lib: &NativeLib,
        handle: u16,
        selector: i16,
        servo: bool,
        out: &Path,
        tag: &str,
    ) -> Value {
        let num_in = 4;
        let mut num = num_in;
        let mut buf = GuardedLoadBuffer::sentinel();
        let started_ns = mesa_core_types::now_unix_ns();
        // 使用现有 ABI 与受保护缓冲；只调用读取函数，整个 Native 阶段保持同 OS 线程。
        let rc = unsafe {
            if servo {
                lib.cnc_rdsvmeter.as_ref().expect("缺少 cnc_rdsvmeter")(
                    handle,
                    &mut num,
                    buf.payload.as_mut_ptr().cast::<OdbSvLoad>(),
                )
            } else {
                lib.cnc_rdspmeter.as_ref().expect("缺少 cnc_rdspmeter")(
                    handle,
                    selector,
                    &mut num,
                    buf.payload.as_mut_ptr().cast::<OdbSpLoad>(),
                )
            }
        };
        let stride = if servo { 12 } else { 24 };
        let valid_count = rc == 0 && (0..=num_in).contains(&num);
        let elems = if valid_count {
            decode_load_elems(&buf.payload, num as usize, stride, selector)
        } else {
            vec![]
        };
        let candidates: Vec<_> = elems.iter().filter(|e| e.written).map(|e| json!({
            "slot":e.slot,"data":e.data,"decimal":e.dec,"unit":e.unit,"name":[e.name,e.suff1,e.suff2]
        })).collect();
        let span = if valid_count {
            num as usize * stride
        } else {
            0
        };
        let span_tail_ok = valid_count && buf.payload[span..].iter().all(|&b| b == LOAD_SENTINEL);
        let untouched_half_ok = elems.iter().filter(|e| !e.written).all(|e| {
            buf.payload[e.slot * 12..e.slot * 12 + 12]
                .iter()
                .all(|&b| b == LOAD_SENTINEL)
        });
        let sample = json!({"backend":"native","pid":std::process::id(),"selector":selector,
            "servo":servo,"num_in":num_in,"num_out":num,"rc":rc,
            "started_ns":started_ns,"finished_ns":mesa_core_types::now_unix_ns(),
            "raw_0_512_hex":buf.raw_hex(),"guards_ok":buf.guards_ok(),"tail_ok":buf.tail_clean(),
            "span_tail_ok":span_tail_ok,"untouched_half_ok":untouched_half_ok,"candidates":candidates});
        // 失败现场也先落盘，避免 assertion 吞掉 rc 或缓冲区证据。
        save(out, &format!("native-{tag}.json"), &sample);
        save(out, &format!("modules-{tag}.json"), &loaded_modules());
        assert!(
            valid_count && buf.guards_ok() && buf.tail_clean() && span_tail_ok && untouched_half_ok,
            "Native 采样失败，见 native-{tag}.json"
        );
        sample
    }

    /// 依次采集四个 Native 与四个 Wire 调用；两侧并非原子快照，不自动解除生产门禁。
    pub(super) fn run() {
        let host = std::env::var("MESA_FOCAS_GATE0_HOST").expect("必须显式设置设备地址");
        let port = std::env::var("MESA_FOCAS_GATE0_PORT")
            .unwrap_or_else(|_| "8193".into())
            .parse::<u16>()
            .expect("端口必须是 u16");
        let out = std::path::PathBuf::from(
            std::env::var("MESA_FOCAS_LOAD_PAIR_OUT").expect("必须设置全新证据目录"),
        );
        std::fs::create_dir(&out).expect("证据目录已存在或父目录不存在，拒绝复用");
        save(
            &out,
            "session.json",
            &json!({"host":host,"port":port,"pid":std::process::id(),
            "scope":"read-only research","panel_observed":false,"production_gate_changed":false}),
        );
        let lib = NativeLib::load().expect("无法加载 FOCAS DLL");
        let handle = NativeHandle(
            &lib,
            lib.cnc_allclibhndl3(&host, port, 5)
                .expect("Native 连接失败"),
        );
        let specs = [
            ("type0", 0, false),
            ("type1", 1, false),
            ("type_all", -1, false),
            ("servo", -1, true),
        ];
        let native: Vec<_> = specs
            .iter()
            .map(|(tag, selector, servo)| {
                native_sample(&lib, handle.1, *selector, *servo, &out, tag)
            })
            .collect();
        save(&out, "native-connections.json", &connections());
        drop(handle);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut session = super::super::session::WireSession::connect(&host, port, Duration::from_secs(5)).await.expect("Wire 连接失败");
            save(&out,"wire-connections.json",&connections());
            let mut comparisons = Vec::new();
            for ((tag, selector, servo), baseline) in specs.iter().zip(native) {
                let request = if *servo { servo_request() } else { spindle_request(*selector as i32).unwrap() };
                std::fs::write(out.join(format!("wire-{tag}.request.bin")), request.encode()).unwrap();
                let response = session.exchange(&request, PacketType::GENERIC_RESPONSE).await.expect("Wire 交换失败");
                std::fs::write(out.join(format!("wire-{tag}.response.bin")), response.encode()).unwrap();
                let candidates = wire_candidates(&response,*selector as i32,*servo).expect("Wire 解码或 ABI 转换失败");
                let exact = baseline["candidates"] == json!(candidates);
                let record = json!({"tag":tag,"candidates":candidates,"native_candidates_equal":exact,
                    "sampling_note":"顺序读取，不是原子快照；变化时需结合抓包分析，不自动判设备不兼容"});
                save(&out,&format!("wire-{tag}.json"),&record);
                comparisons.push(record);
            }
            session.close().await.expect("Wire CLOSE 失败");
            save(&out,"comparison.json",&json!(comparisons));
        });
        println!("LOAD_PAIR_EVIDENCE={}", out.display());
    }
    /// 使用受保护的实际 DLL ABI 作为基准，比较正式适配器的百分比输出。
    /// 仅可连接本机报文替身；重复批次用于验证没有跨批次残留负载缓存。
    pub(super) fn run_production() {
        use crate::address::{FocasAddress, SpindleKind};
        use crate::focas_api::FocasApi;
        use mesa_core_types::Value as MesaValue;
        let host = std::env::var("MESA_FOCAS_GATE0_HOST").unwrap();
        assert_eq!(host, "127.0.0.1", "合成报文验收只允许 loopback");
        let port = std::env::var("MESA_FOCAS_GATE0_PORT")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let out = std::path::PathBuf::from(std::env::var("MESA_FOCAS_LOAD_PAIR_OUT").unwrap());
        std::fs::create_dir(&out).unwrap();
        let lib = NativeLib::load().unwrap();
        let handle = NativeHandle(&lib, lib.cnc_allclibhndl3(&host, port, 5).unwrap());
        let spindle = native_sample(&lib, handle.1, 0, false, &out, "type0");
        let repeat = native_sample(&lib, handle.1, 0, false, &out, "type0-repeat");
        assert_eq!(
            spindle["candidates"], repeat["candidates"],
            "相同固定报文的重复 Native 调用发生变化"
        );
        let servo = native_sample(&lib, handle.1, -1, true, &out, "servo");
        drop(handle);
        let mut addresses = Vec::new();
        let mut expected = Vec::new();
        for (is_servo, baseline) in [(false, spindle), (true, servo)] {
            let rows = baseline["candidates"].as_array().unwrap();
            for index in 1..=4 {
                addresses.push(if is_servo {
                    FocasAddress::ServoLoad { axis: index }
                } else {
                    FocasAddress::Spindle {
                        spindle: index,
                        kind: SpindleKind::Load,
                    }
                });
                let value = rows.get(index as usize - 1).map(|r| {
                    let raw = r["data"].as_i64().unwrap() as f64;
                    let decimal = r["decimal"].as_i64().unwrap() as i32;
                    // 独立十进制解析作工程值基准，不调用生产换算器。
                    format!("{raw}e{}", -decimal).parse::<f64>().unwrap()
                });
                expected.push(value.filter(|v| {
                    v.is_finite() && (*v != 0.0 || rows[index as usize - 1]["data"] == 0)
                }));
            }
        }
        addresses.extend([
            addresses[0].clone(),
            addresses[4].clone(),
            FocasAddress::ServoLoad { axis: 0 },
            FocasAddress::Spindle {
                spindle: 5,
                kind: SpindleKind::Load,
            },
        ]);
        expected.extend([expected[0], expected[4], None, None]);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let api = super::super::WireFocasApi::new(Duration::from_secs(5));
            api.connect(&host, port, 5000).await.unwrap();
            let mut comparisons = Vec::new();
            for batch in 0..2 {
                let values = api.read_batch(&addresses).await.unwrap();
                assert_eq!(values.len(), expected.len());
                for (index, (actual, expected)) in values.iter().zip(&expected).enumerate() {
                    let equal = match (actual, expected) {
                        (MesaValue::F64(value), Some(number)) => (value - number).abs() <= number.abs() * 1e-14 + f64::from_bits(1),
                        (MesaValue::String(error), None) => error.starts_with("ERR:"),
                        _ => false,
                    };
                    comparisons.push(json!({"batch":batch,"index":index,"address":addresses[index].source_label(),
                        "actual":actual,"expected_percent":expected,"equal":equal}));
                }
            }
            api.disconnect().await;
            save(&out, "production-comparison.json", &json!(comparisons));
            assert!(comparisons.iter().all(|r| r["equal"] == true), "正式负载输出不等于 DLL ABI 的百分比");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 历史误请求仍完整保留；正确构造必须只改首尾 A4 的选择字节。
    #[test]
    fn spindle_request_corrects_archived_selector() {
        for (selector, tag) in [(0, "type0"), (1, "type1"), (-1, "type_all")] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "tests/fixtures/wire/spindle_load_165/{tag}/request_frame.bin"
            ));
            let old = std::fs::read(path).unwrap();
            let corrected = spindle_request(selector).unwrap().encode();
            let diffs: Vec<_> = old
                .iter()
                .zip(&corrected)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .collect();
            assert_eq!(old.len(), corrected.len());
            assert_eq!(diffs.len(), 2, "只能改变首尾 A4 的参数");
            assert_eq!(diffs[0].0, 23);
            assert_eq!(diffs[1].0, old.len() - 17);
            for (_, (before, after)) in diffs {
                assert_eq!((*before, *after), (2, 1));
            }
        }
        assert!(spindle_request(2).is_err());
    }

    /// 同轮真实采集回放：请求必须逐字节等于 DLL，响应解码必须等于独立 Native ABI。
    /// 覆盖三种主轴 selector 与三轴伺服；保留 NCGuide 响应中的非零轴名第三字节。
    #[test]
    fn paired_capture_replays_against_native_abi() {
        use sha2::{Digest, Sha256};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/wire/load_pair_165_20261010");
        for (tag, selector, servo) in [
            ("type0", 0, false),
            ("type1", 1, false),
            ("type_all", -1, false),
            ("servo", -1, true),
        ] {
            let dir = root.join(tag);
            let expected: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
            let request = if servo {
                servo_request()
            } else {
                spindle_request(selector).unwrap()
            }
            .encode();
            for name in [
                "native_request.bin",
                "wire_request.bin",
                "native_response.bin",
                "wire_response.bin",
            ] {
                let raw = std::fs::read(dir.join(name)).unwrap();
                assert_eq!(
                    format!("{:x}", Sha256::digest(&raw)),
                    expected["sha256"][name].as_str().unwrap()
                );
                if name.ends_with("request.bin") {
                    assert_eq!(request, raw, "请求偏离 Native 抓包");
                } else {
                    let mut head = [0; 10];
                    head.copy_from_slice(&raw[..10]);
                    let frame = super::super::frame::assemble(
                        super::super::frame::decode_header(&head).unwrap(),
                        raw[10..].to_vec(),
                    )
                    .unwrap();
                    assert_eq!(
                        serde_json::json!(wire_candidates(&frame, selector, servo).unwrap()),
                        expected["native_candidates"],
                        "解码偏离独立 Native ABI"
                    );
                }
            }
        }
    }

    /// 本机修改响应后实际调用 DLL 取得的非零样本；与真机采集分开归档。
    /// 检查小数位、伺服绝对值、主轴符号和双主轴配对，避免零值掩盖缩放错误。
    #[test]
    fn synthetic_nonzero_replays_against_native_and_expected() {
        use sha2::{Digest, Sha256};
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire");
        for (archive, case) in [
            ("load_synthetic_nonzero", "single"),
            ("load_synthetic_nonzero", "multi_signed"),
            ("load_synthetic_edges", "integer_edges"),
        ] {
            let root = base.join(archive);
            for (tag, selector, servo) in [
                ("type0", 0, false),
                ("type1", 1, false),
                ("type_all", -1, false),
                ("servo", -1, true),
            ] {
                let dir = root.join(case).join(tag);
                let expected: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap())
                        .unwrap();
                assert_eq!(expected["kind"], "synthetic-derived");
                assert_eq!(expected["native_wire_expected_equal"], true);
                let request = std::fs::read(dir.join("request.bin")).unwrap();
                let response = std::fs::read(dir.join("response.bin")).unwrap();
                for (key, raw) in [("request_sha256", &request), ("response_sha256", &response)] {
                    assert_eq!(
                        format!("{:x}", Sha256::digest(raw)),
                        expected[key].as_str().unwrap()
                    );
                }
                assert_eq!(
                    request,
                    if servo {
                        servo_request()
                    } else {
                        spindle_request(selector).unwrap()
                    }
                    .encode()
                );
                let head: [u8; 10] = response[..10].try_into().unwrap();
                let frame = super::super::frame::assemble(
                    super::super::frame::decode_header(&head).unwrap(),
                    response[10..].to_vec(),
                )
                .unwrap();
                assert_eq!(
                    serde_json::json!(wire_candidates(&frame, selector, servo).unwrap()),
                    expected["candidates"],
                    "{case}/{tag} 与 Native ABI / 独立预期不一致"
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "需要本机冻结报文服务与实际参考 DLL，不连接真实设备"]
    fn load_production_pair_live() {
        live::run_production();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "显式指定设备和全新目录的只读取证，默认 CI 不连接设备"]
    fn load_protocol_pair_live() {
        live::run();
    }
}
