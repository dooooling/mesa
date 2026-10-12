//! 本机报文派生实验：真实 DLL 与生产操作消息解码对照，不连接设备或写入 PMC。

use crate::native::{NativeLib, OpMsg, load_evidence::GuardedLoadBuffer};
use serde_json::json;
use std::time::Duration;

/// 用正确的原始签名和大缓冲旁路旧 Native 门禁，仅采集证据，不恢复产品调用。
#[test]
#[ignore = "需要显式的只读 NCGuide/设备窗口与全新证据目录"]
fn guarded_ready_dll_pair_live() {
    let host = std::env::var("MESA_FOCAS_GATE0_HOST").unwrap();
    let port: u16 = std::env::var("MESA_FOCAS_GATE0_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let out = std::path::PathBuf::from(std::env::var("MESA_FOCAS_LOAD_PAIR_OUT").unwrap());
    std::fs::create_dir(&out).expect("证据目录必须全新");
    let lib = NativeLib::load().unwrap();
    let handle = lib.cnc_allclibhndl3(&host, port, 5).unwrap();
    let mut native = Vec::new();
    for tag in ["gear", "maxrpm", "diagnosis", "alarm"] {
        let mut b = GuardedLoadBuffer::sentinel();
        let mut count: i16 = 1;
        let rc = unsafe {
            let ptr = b.payload.as_mut_ptr();
            match tag {
                "gear" => lib.cnc_rdspgear.as_ref().unwrap()(handle, 1, ptr),
                "maxrpm" => lib.cnc_rdspmaxrpm.as_ref().unwrap()(handle, 1, ptr),
                "diagnosis" => lib.cnc_diagnoss.as_ref().unwrap()(handle, 301, 3, 12, ptr),
                _ => lib.cnc_rdalmmsg.as_ref().unwrap()(handle, -1, &mut count, ptr),
            }
        };
        assert!(b.guards_ok());
        assert!(b.payload[64..].iter().all(|&v| v == 0xCC));
        std::fs::write(out.join(format!("native-{tag}.abi.bin")), &b.payload[..64]).unwrap();
        assert_eq!(rc, 0, "{tag} 原始 DLL 失败");
        let value = match tag {
            "gear" | "maxrpm" => json!(i16::from_le_bytes(b.payload[4..6].try_into().unwrap())),
            "diagnosis" => {
                assert_eq!(i16::from_le_bytes(b.payload[..2].try_into().unwrap()), 301);
                let raw = i32::from_le_bytes(b.payload[4..8].try_into().unwrap());
                let dec = i32::from_le_bytes(b.payload[8..12].try_into().unwrap());
                json!(raw as f64 / 10f64.powi(dec))
            }
            _ => {
                assert!((0..=1).contains(&count));
                if count == 0 {
                    json!([])
                } else {
                    let len = i16::from_le_bytes(b.payload[10..12].try_into().unwrap());
                    assert!((0..=32).contains(&len));
                    let text = &b.payload[12..12 + len as usize];
                    let end = text.iter().position(|&v| v == 0).unwrap_or(text.len());
                    json!([String::from_utf8_lossy(&text[..end]).trim()])
                }
            }
        };
        native.push(json!({"tag":tag,"value":value,"rc":rc,"count":count,"guards_ok":true}));
    }
    std::fs::write(
        out.join("modules.json"),
        serde_json::to_vec_pretty(&super::load_research::live::loaded_modules()).unwrap(),
    )
    .unwrap();
    lib.cnc_freelibhndl(handle).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let wire = rt.block_on(async {
        let client = super::FocasClient::new(Duration::from_secs(5));
        client.ensure_connected(&host, port).await.unwrap();
        let gear = client.spindle_gear(1).await.unwrap().value;
        let maxrpm = client.spindle_maxrpm(1).await.unwrap().value;
        let diagnosis = client.diagnosis_value(301, 3).await.unwrap();
        let mesa_core_types::Value::F64(diagnosis) =
            super::wire::diagnosis_to_value_for_test(&diagnosis).unwrap()
        else {
            panic!("诊断类型错误")
        };
        let alarm = client.alarm_value().await.unwrap();
        client.disconnect().await;
        vec![
            json!(gear),
            json!(maxrpm),
            json!(diagnosis),
            json!(alarm.alarms.iter().map(|a| &a.text).collect::<Vec<_>>()),
        ]
    });
    let comparisons: Vec<_> = native
        .iter()
        .zip(wire)
        .map(|(n, w)| json!({"tag":n["tag"],"native":n,"wire":w,"equal":n["value"] == w}))
        .collect();
    std::fs::write(
        out.join("comparison.json"),
        serde_json::to_vec_pretty(&comparisons).unwrap(),
    )
    .unwrap();
    // 顺序取样会有动态漂移，任何差异必须人工核对捕获，不能被 unsupported 豁免。
    assert!(
        comparisons.iter().all(|c| c["equal"] == true),
        "存在差异，见 comparison.json"
    );
}

#[test]
#[ignore = "需要显式启动本机冻结报文服务，并指定全新证据路径"]
fn opmsg_dll_pair_local() {
    let host = "127.0.0.1";
    let port: u16 = std::env::var("MESA_FOCAS_OPMSG_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let out = std::path::PathBuf::from(std::env::var("MESA_FOCAS_OPMSG_OUT").unwrap());
    let cases: usize = std::env::var("MESA_FOCAS_OPMSG_COUNT")
        .unwrap_or_else(|_| "6".into())
        .parse()
        .unwrap();
    assert!((1..=64).contains(&cases), "案例数量必须有界");
    std::fs::create_dir(&out).expect("证据目录必须全新");
    let lib = NativeLib::load().unwrap();
    let handle = lib.cnc_allclibhndl3(host, port, 5).unwrap();
    let mut native = Vec::new();
    for _ in 0..cases {
        // 先以大缓冲确定真实 DLL 的写入范围，再调用实际产品包装；两者必须一致。
        let mut guarded = GuardedLoadBuffer::sentinel();
        let mut count: i16 = 1;
        let rc = unsafe {
            lib.cnc_rdopmsg3.as_ref().unwrap()(
                handle,
                4,
                &mut count,
                guarded.payload.as_mut_ptr().cast(),
            )
        };
        assert!(guarded.guards_ok());
        let written = if rc == 0 && count == 1 { 262 } else { 0 };
        assert!(guarded.payload[written..].iter().all(|&b| b == 0xCC));
        let wrapped = lib.cnc_rdopmsg(handle);
        let record = match wrapped {
            Ok(msg) => {
                assert_eq!(rc, 0);
                let raw: OpMsg =
                    unsafe { std::ptr::read_unaligned(guarded.payload.as_ptr().cast()) };
                assert_eq!(msg.value_text(), raw.value_text());
                json!({"rc":rc,"count":count,"text":msg.value_text(),"guards_ok":true,"tail_clean":true})
            }
            Err(e) => {
                assert_eq!(e.code(), rc);
                json!({"rc":rc,"guards_ok":true,"tail_clean":true})
            }
        };
        native.push(record);
    }
    lib.cnc_freelibhndl(handle).unwrap();
    std::fs::write(
        out.join("modules.json"),
        serde_json::to_vec_pretty(&super::load_research::live::loaded_modules()).unwrap(),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let wire = rt.block_on(async {
        let client = super::FocasClient::new(Duration::from_secs(5));
        client.ensure_connected(host, port).await.unwrap();
        let mut records = Vec::new();
        for _ in 0..cases {
            records.push(match client.opmsg_value().await {
                Ok(msg) => json!({"rc":0,"text":msg.text}),
                Err(super::WireError::Remote { status, .. }) => json!({"rc":status}),
                Err(error) => panic!("意外 Wire 失败：{error}"),
            });
        }
        client.disconnect().await;
        records
    });
    for (n, w) in native.iter().zip(&wire) {
        assert_eq!(n["rc"], w["rc"]);
        assert_eq!(n.get("text"), w.get("text"));
    }
    std::fs::write(
        out.join("comparison.json"),
        serde_json::to_vec_pretty(&json!({
            "kind":"synthetic-derived","native":native,"wire":wire,"all_equal":true,
            "production_load_gate_changed":false
        }))
        .unwrap(),
    )
    .unwrap();
}
