//! 固定报文服务上的 DLL 差分；覆盖最终 TypedValue，不恢复已关闭的 Native ABI 路径。

use crate::address::{FocasAddress, parse_address};
use crate::focas_api::{FocasApi, NativeFocasApi};
use crate::native::{NativeLib, load_evidence::GuardedLoadBuffer};
use mesa_core_types::Value;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Deserialize)]
struct Case {
    name: String,
    address: String,
    expected: Value,
    guarded: Option<String>,
}

/// 已停用包装的四类函数只用独立 ABI 解释器作 oracle；容量/写入范围同时校验。
/// 不使用 Wire decoder 计算 DLL 预期值，否则两边同一错误也会被误判全等。
fn guarded_value(lib: &NativeLib, handle: u16, tag: &str) -> (Value, Vec<u8>) {
    let mut b = GuardedLoadBuffer::sentinel();
    let mut count: i16 = 1;
    let rc = unsafe {
        let p = b.payload.as_mut_ptr();
        match tag {
            "gear" => lib.cnc_rdspgear.as_ref().unwrap()(handle, 1, p),
            "maxrpm" => lib.cnc_rdspmaxrpm.as_ref().unwrap()(handle, 1, p),
            "diagnosis" => lib.cnc_diagnoss.as_ref().unwrap()(handle, 301, 3, 12, p),
            "alarm" => lib.cnc_rdalmmsg.as_ref().unwrap()(handle, -1, &mut count, p),
            _ => panic!("未知 guarded 类型"),
        }
    };
    eprintln!(
        "guarded {tag} rc={rc} count={count} abi={:02x?}",
        &b.payload[..48]
    );
    assert_eq!(rc, 0, "{tag} DLL 调用失败");
    let span = match tag {
        "gear" | "maxrpm" => 12,
        "diagnosis" => 12,
        _ => {
            assert!((0..=1).contains(&count));
            count as usize * 44
        }
    };
    assert!(b.guards_ok() && b.payload[span..].iter().all(|&x| x == 0xCC));
    let short = |i| i16::from_le_bytes(b.payload[i..i + 2].try_into().unwrap());
    let int = |i| i32::from_le_bytes(b.payload[i..i + 4].try_into().unwrap());
    let value = match tag {
        "gear" | "maxrpm" => Value::I32(short(4) as i32),
        "diagnosis" => {
            assert_eq!(short(0), 301);
            Value::F64(int(4) as f64 / 10f64.powi(int(8)))
        }
        _ if count == 0 => Value::StringArray(vec![]),
        _ => {
            let len = short(10);
            assert!((0..=32).contains(&len));
            let text = &b.payload[12..12 + len as usize];
            let end = text.iter().position(|&x| x == 0).unwrap_or(text.len());
            Value::StringArray(vec![String::from_utf8_lossy(&text[..end]).trim().into()])
        }
    };
    (value, b.payload[..span].to_vec())
}

#[test]
#[ignore = "需本机固定报文服务、实际 DLL 与全新证据目录"]
fn ready_dll_pair_local() {
    let port = std::env::var("MESA_FOCAS_READY_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let out = std::path::PathBuf::from(std::env::var("MESA_FOCAS_READY_OUT").unwrap());
    let cases: Vec<Case> = serde_json::from_slice(
        &std::fs::read(std::env::var("MESA_FOCAS_READY_CASES").unwrap()).unwrap(),
    )
    .unwrap();
    assert!(!cases.is_empty() && cases.len() <= 128);
    std::fs::create_dir(&out).expect("证据目录必须全新");
    let addresses: Vec<_> = cases
        .iter()
        .map(|c| {
            if c.address == "active_spindle" {
                FocasAddress::ActiveSpindleSpeed
            } else if c.address == "diagnosis301axis3" {
                FocasAddress::Diagnosis {
                    number: 301,
                    axis: 3,
                }
            } else {
                parse_address(&c.address).unwrap()
            }
        })
        .collect();
    let lib = NativeLib::load().unwrap();
    let handle = lib.cnc_allclibhndl3("127.0.0.1", port, 5).unwrap();
    let mut native = Vec::new();
    // 句柄创建、读和关闭全在本测试线程；之后才进入异步 Wire 阶段。
    for (c, a) in cases.iter().zip(&addresses) {
        let v = if let Some(tag) = &c.guarded {
            let (value, bytes) = guarded_value(&lib, handle, tag);
            std::fs::write(out.join(format!("{}.abi.bin", c.name)), bytes).unwrap();
            value
        } else {
            NativeFocasApi::read_for_dll_evidence(&lib, handle, a).unwrap()
        };
        std::fs::write(
            out.join(format!("{}.native.json", c.name)),
            serde_json::to_vec_pretty(&v).unwrap(),
        )
        .unwrap();
        native.push(v);
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
        let api = super::WireFocasApi::new(Duration::from_secs(5));
        api.connect("127.0.0.1", port, 5000).await.unwrap();
        let mut values = Vec::new();
        for a in &addresses {
            // 单点批保留生产适配器输出类型；不在测试中手工模拟适配器。
            let mut batch = api.read_batch(std::slice::from_ref(a)).await.unwrap();
            assert_eq!(batch.len(), 1);
            values.push(batch.remove(0));
        }
        api.disconnect().await;
        values
    });
    let rows: Vec<_> = cases
        .iter()
        .zip(native)
        .zip(wire)
        .map(|((c, n), w)| {
            json!({"name":c.name,"address":c.address,"native":n,"wire":w,"expected":c.expected,
            "equal":n == w && n == c.expected,"guarded":c.guarded})
        })
        .collect();
    std::fs::write(
        out.join("comparison.json"),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
    assert!(
        rows.iter().all(|r| r["equal"] == true),
        "DLL/Wire/独立预期存在差异，见 comparison.json"
    );
}
