//! 只读身份核对探针（`#[ignore]`，默认 CI 不跑；连真实机床前需显式批准）。
//!
//! 目的：为「剩余 20 点对账」补齐实体机的驱动对象身份证据——
//! `V/5`、`V/6` 是否存在及其 `p0199` 对象名，并复核机床轴名与轴号映射。
//!
//! 本探针**直接使用驱动内建的 `IdentityProbeSpec`**（`src/identity_spec.rs`），
//! 因此同时验证生产身份浏览用的探针数值与实体机一致；其余临时读数走
//! 原生地址（`NativeNckAddress`），不再依赖任何变量目录。
//!
//! 边界：只读；单会话；每次请求项数受 `max_read_items` 限制；
//! 不写参数、不发控制、不新增点位、不改协议实现。
//! 用法：
//! `MESA_NCK_HOST=192.168.15.60 cargo test -p mesa-driver-sinumerik-nck
//!  --test live_identity_probe -- --ignored --nocapture`

use mesa_driver_sinumerik_nck::{
    AXIS_CTRLOUT_MODULE_NR, AXIS_CTRLOUT_TYPE, AXIS_NAME, AXIS_SPIND_ASSIGN, CHANNEL_MACHAX_USED,
    DRIVE_OBJECT_NAME, DRIVE_OBJECT_NAME_M130, IdentityProbeSpec, NativeNckAddress, NckClient,
    NckConnConfig, decode_value, resolve_native,
};
use serde_json::json;

fn cfg_from_env() -> NckConnConfig {
    let host = std::env::var("MESA_NCK_HOST").unwrap_or_else(|_| "192.168.15.60".into());
    let port: u16 = std::env::var("MESA_NCK_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(102);
    let local_tsap: u16 = std::env::var("MESA_NCK_LOCAL_TSAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024);
    let remote_tsap: u16 = std::env::var("MESA_NCK_REMOTE_TSAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3332);
    NckConnConfig::from_json(&json!({
        "host": host, "port": port, "family": "828d",
        "local_tsap": local_tsap, "remote_tsap": remote_tsap,
        "timeout_ms": 2000, "pdu_length": 480, "max_read_items": 1,
    }))
    .expect("连接参数合法")
}

/// V 区原生地址（缺省 NC 变量入口 module 26；`module130=true` 走驱动对象入口）。
fn v_address(
    area_no: u16,
    column: u16,
    decode: &str,
    transport: &str,
    count: u8,
) -> NativeNckAddress {
    NativeNckAddress::from_parameters(
        json!({
            "area": "V", "area_no": area_no, "module": 26, "column": column,
            "line": 1, "count": count, "unit_mode": "current",
            "decode": decode, "transport_size": transport,
        })
        .as_object()
        .unwrap(),
    )
    .expect("V 区原生地址合法")
}

/// 单次只读：解析 → 读取 → 解码，返回可打印字符串。
async fn read_native(client: &mut NckClient, addr: NativeNckAddress) -> String {
    let rp = match resolve_native(addr) {
        Ok(r) => r,
        Err(e) => return format!("WIRE_ERR {e}"),
    };
    let item = mesa_driver_sinumerik_nck::NckReadItem {
        wire: rp.wire,
        expected_data_len: rp.expected_data_len,
        expected_transport_size: rp.expected_transport_size,
    };
    match client.read_vars(&[item]).await {
        Err(e) => format!("JOB_ERR {} {}", e.code, e.message),
        Ok(results) => {
            let Some(r) = results.first() else {
                return "NO_RESULT".into();
            };
            if r.return_code != 0xff {
                return format!("ITEM_REJECTED 0x{:02X}", r.return_code);
            }
            match r.data.as_deref() {
                None => "NO_DATA".into(),
                Some(bytes) => match decode_value(bytes, rp.kind) {
                    Ok(value) => format!("OK {value:?}"),
                    Err(e) => format!("DECODE_ERR {e}"),
                },
            }
        }
    }
}

/// 单次只读（内建身份探针路径）。
async fn read_probe(client: &mut NckClient, spec: &IdentityProbeSpec, target: u16) -> String {
    let (_, item) = match spec.resolve(target) {
        Ok(v) => v,
        Err(e) => return format!("PROBE_ERR {e}"),
    };
    match client.read_vars(&[item]).await {
        Err(e) => format!("JOB_ERR {} {}", e.code, e.message),
        Ok(results) => {
            let Some(r) = results.first() else {
                return "NO_RESULT".into();
            };
            if r.return_code != 0xff {
                return format!("ITEM_REJECTED 0x{:02X}", r.return_code);
            }
            match r.data.as_deref() {
                None => "NO_DATA".into(),
                Some(bytes) => match decode_value(bytes, spec.decode) {
                    Ok(value) => format!("OK {value:?}"),
                    Err(e) => format!("DECODE_ERR {e}"),
                },
            }
        }
    }
}

#[tokio::test]
#[ignore = "只读真实机床身份核对；需显式 --ignored 运行"]
async fn live_identity_probe() {
    let cfg = cfg_from_env();
    let mut client = NckClient::connect(&cfg).await.expect("连接失败");
    println!(
        ">>> 已连接 {}:{} pdu={}",
        cfg.host,
        cfg.port,
        client.negotiated_pdu_length()
    );

    println!("=== 机床轴名（内建探针 AXIS_NAME）line=1..8 ===");
    for line in 1..=8u16 {
        let s = read_probe(&mut client, &AXIS_NAME, line).await;
        println!("  机床轴{line}: {s}");
    }

    println!("=== 通道1 位置→机床轴号（内建探针 CHANNEL_MACHAX_USED）line=1..8 ===");
    for line in 1..=8u16 {
        let s = read_probe(&mut client, &CHANNEL_MACHAX_USED, line).await;
        println!("  通道位置{line}: {s}");
    }

    println!("=== 轴驱动号（内建探针 AXIS_CTRLOUT_MODULE_NR）轴=1..8 ===");
    for n in 1..=8u16 {
        let s = read_probe(&mut client, &AXIS_CTRLOUT_MODULE_NR, n).await;
        println!("  机床轴{n}: {s}");
    }

    println!("=== 轴输出类型（内建探针 AXIS_CTRLOUT_TYPE）轴=1..8 ===");
    for n in 1..=8u16 {
        let s = read_probe(&mut client, &AXIS_CTRLOUT_TYPE, n).await;
        println!("  机床轴{n}: {s}");
    }

    println!("=== 主轴归属（内建探针 AXIS_SPIND_ASSIGN；非0=该轴为第n主轴）===");
    for n in 1..=8u16 {
        let s = read_probe(&mut client, &AXIS_SPIND_ASSIGN, n).await;
        println!("  机床轴{n}: {s}");
    }

    println!("=== 驱动对象名（内建探针 DRIVE_OBJECT_NAME，NC变量入口 module26）===");
    for n in 1..=8u16 {
        let s = read_probe(&mut client, &DRIVE_OBJECT_NAME, n).await;
        println!("  V/{n}: {s}");
    }

    println!("=== 驱动对象名（内建探针 DRIVE_OBJECT_NAME_M130，驱动对象入口 module130）===");
    for n in 1..=8u16 {
        let s = read_probe(&mut client, &DRIVE_OBJECT_NAME_M130, n).await;
        println!("  V/{n}(130): {s}");
    }

    // 跨路径同值校验：同一参数的读回值必须相等，否则两条入口的 V/n
    // 不是同一物理对象（用于检验“编号可否沿用”的假设）。
    println!("=== 跨路径同值校验 r0035（电机温度；静态值，同一对象必相等）===");
    let mut m26 = Vec::new();
    for n in 1..=8u16 {
        m26.push(read_native(&mut client, v_address(n, 35, "F32_BE", "0x07", 1)).await);
    }
    let mut m130 = Vec::new();
    for n in 1..=8u16 {
        // module 130：驱动对象入口（同一列，不同 module）。
        let addr = NativeNckAddress::from_parameters(
            json!({
                "area": "V", "area_no": n, "module": 130, "column": 35,
                "line": 1, "count": 1, "unit_mode": "current",
                "decode": "F32_BE", "transport_size": "0x07",
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        m130.push(read_native(&mut client, addr).await);
    }
    for n in 1..=8usize {
        println!(
            "  m26 V/{n}: {:<22} | m130 V/{n}: {}",
            m26[n - 1],
            m130[n - 1]
        );
    }

    // 候选身份参数：寻找能直接编码「驱动对象↔机床轴」归属的读数。
    for (var, decode, transport) in [
        ("r0103", "U16_BE", "0x06"),
        ("p0105", "I16_BE", "0x05"),
        ("r0106", "I16_BE", "0x05"),
        ("r0107", "I16_BE", "0x05"),
        ("r0108", "U32_BE", "0x06"),
    ] {
        println!("=== 身份候选参数 V/n/M/{var}（NC变量入口 module26）===");
        for n in 1..=8u16 {
            let column: u16 = match var {
                "r0103" => 103,
                "p0105" => 105,
                "r0106" => 106,
                "r0107" => 107,
                "r0108" => 108,
                _ => unreachable!(),
            };
            let s = read_native(&mut client, v_address(n, column, decode, transport, 1)).await;
            println!("  V/{n}: {s}");
        }
    }

    println!("<<< 探针结束（只读，未写入任何参数）");
}
