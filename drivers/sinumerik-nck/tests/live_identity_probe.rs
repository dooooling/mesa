//! 只读身份核对探针（`#[ignore]`，默认 CI 不跑；连真实机床前需显式批准）。
//!
//! 目的：为「剩余 20 点对账」补齐实体机的驱动对象身份证据——
//! `V/5`、`V/6` 是否存在及其 `p0199` 对象名，并复核机床轴名与轴号映射。
//!
//! 边界：只读；单会话；每次请求项数受 `max_read_items` 限制；
//! 不写参数、不发控制、不新增点位、不改协议实现。
//! 用法：
//! `MESA_NCK_HOST=192.168.15.60 cargo test -p mesa-driver-sinumerik-nck
//!  --test live_identity_probe -- --ignored --nocapture`

use mesa_driver_sinumerik_nck::{
    CatalogRegistry, NckClient, NckConnConfig, NckReadItem, NckVariableRef, decode_value,
    resolve_wire,
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

/// 单次只读：解析 → 读取 → 解码，返回可打印字符串。
async fn read_one(
    client: &mut NckClient,
    catalog: &mesa_driver_sinumerik_nck::NckCatalog,
    params: serde_json::Value,
) -> String {
    let reference = match NckVariableRef::from_parameters(params.as_object().unwrap()) {
        Ok(r) => r,
        Err(e) => return format!("ADDRESS_ERR {e}"),
    };
    let def = match catalog.lookup(reference.area, "M", &reference.variable) {
        Ok(d) => d,
        Err(e) => return format!("CATALOG_ERR {e}"),
    };
    let resolved = match resolve_wire(&reference, def) {
        Ok(r) => r,
        Err(e) => return format!("WIRE_ERR {e}"),
    };
    let item = NckReadItem {
        wire: resolved.wire,
        expected_data_len: resolved.expected_data_len,
        expected_transport_size: resolved.expected_transport_size,
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
                Some(bytes) => match decode_value(bytes, resolved.kind) {
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
    let catalog = CatalogRegistry::load_shipped()
        .and_then(|r| r.resolve("828d"))
        .expect("828d catalog 装载");
    let mut client = NckClient::connect(&cfg).await.expect("连接失败");
    println!(
        ">>> 已连接 {}:{} pdu={}",
        cfg.host,
        cfg.port,
        client.negotiated_pdu_length()
    );

    println!("=== 机床轴名 N/1/MDSA_AXCONF_MACHAX_NAME_TAB line=1..8 ===");
    for line in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "N", "area_no": 1, "block": "M",
                "variable": "MDSA_AXCONF_MACHAX_NAME_TAB",
                "line": line, "count": 1, "access_path": "nc_variable",
            }),
        )
        .await;
        println!("  机床轴{line}: {s}");
    }

    println!("=== 通道1 位置→机床轴号 C/1/M/AXCONF_MACHAX_USED line=1..8 ===");
    for line in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "C", "area_no": 1, "block": "M",
                "variable": "AXCONF_MACHAX_USED",
                "line": line, "count": 1, "access_path": "nc_variable",
            }),
        )
        .await;
        println!("  通道位置{line}: {s}");
    }

    println!("=== 轴驱动号 A/n/M/MDCA_CTRLOUT_MODULE_NR line=1 ===");
    for n in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "A", "area_no": n, "block": "M",
                "variable": "MDCA_CTRLOUT_MODULE_NR",
                "line": 1, "count": 1, "access_path": "nc_variable",
            }),
        )
        .await;
        println!("  机床轴{n}: {s}");
    }

    println!("=== 轴输出类型 A/n/M/MDCA_CTRLOUT_TYPE ===");
    for n in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "A", "area_no": n, "block": "M",
                "variable": "MDCA_CTRLOUT_TYPE",
                "line": 1, "count": 1, "access_path": "nc_variable",
            }),
        )
        .await;
        println!("  机床轴{n}: {s}");
    }

    println!("=== 主轴归属 A/n/M/SPIND_ASSIGN_TO_MACHAX（非0=该轴为第n主轴）===");
    for n in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "A", "area_no": n, "block": "M",
                "variable": "SPIND_ASSIGN_TO_MACHAX",
                "line": 1, "count": 1, "access_path": "nc_variable",
            }),
        )
        .await;
        println!("  机床轴{n}: {s}");
    }

    println!("=== 驱动对象名 V/n/M/p0199 count=25（nc_variable 路径 module26）===");
    for n in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "V", "area_no": n, "block": "M", "variable": "p0199",
                "line": 1, "count": 25, "access_path": "nc_variable",
            }),
        )
        .await;
        println!("  V/{n}: {s}");
    }

    println!("=== 驱动对象名 V/n/M/p0199 count=25（drive_object 路径 module130）===");
    for n in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "V", "area_no": n, "block": "M", "variable": "p0199",
                "line": 1, "count": 25, "access_path": "drive_object",
            }),
        )
        .await;
        println!("  V/{n}(130): {s}");
    }

    // 跨路径同值校验：若两条路径的 V/n 指向同一物理驱动对象，
    // 则同一参数的读回值必须相等。用于把 p0199 名称关联从假设升级为实测。
    println!("=== 跨路径同值校验 r0035（电机温度；静态值，同一对象必相等）===");
    let mut m26 = Vec::new();
    for n in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "V", "area_no": n, "block": "M", "variable": "r0035",
                "line": 1, "count": 1, "access_path": "nc_variable",
            }),
        )
        .await;
        m26.push(s);
    }
    let mut m130 = Vec::new();
    for n in 1..=8u16 {
        let s = read_one(
            &mut client,
            &catalog,
            json!({
                "area": "V", "area_no": n, "block": "M", "variable": "r0035",
                "line": 1, "count": 1, "access_path": "drive_object",
            }),
        )
        .await;
        m130.push(s);
    }
    for n in 1..=8usize {
        println!(
            "  m26 V/{n}: {:<22} | m130 V/{n}: {}",
            m26[n - 1],
            m130[n - 1]
        );
    }

    // 候选身份参数：寻找能直接编码「驱动对象↔机床轴」归属的读数。
    for var in ["r0103", "p0105", "r0106", "r0107", "r0108"] {
        println!("=== 身份候选参数 V/n/M/{var}（nc_variable module26）===");
        for n in 1..=8u16 {
            let s = read_one(
                &mut client,
                &catalog,
                json!({
                    "area": "V", "area_no": n, "block": "M", "variable": var,
                    "line": 1, "count": 1, "access_path": "nc_variable",
                }),
            )
            .await;
            println!("  V/{n}: {s}");
        }
    }

    println!("<<< 探针结束（只读，未写入任何参数）");
}
