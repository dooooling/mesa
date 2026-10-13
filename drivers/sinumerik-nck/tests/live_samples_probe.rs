//! 实体机代表性采样对照探针（`#[ignore]`，只读，默认 CI 不跑）。
//!
//! 目的（V2.0 §9 PR-C 第 3 项）：用**新的原生绑定**（模板 + 使用者确认的
//! `area_no`/`line`）在真实 828D 上读取代表性点位，并记录
//! **请求包字节 / 设备返回 / 质量**，与既有证据对照。覆盖：
//!
//! - R 参数（`C/RP/rpa`，line = R 号 + 1）
//! - 轴位置（`C/SGA/actToolBasePos` 设定位置、`C/SEGA/actProgPosBKS` 实际位置）
//! - 进给与程序状态（`C/S/actFeedRateIpo`、`C/S/progStatus`）
//! - 主轴指令转速（`C/SSP/cmdSpeed`）
//! - SINAMICS 只读驱动参数（`V/M/r0027`、`r0031`、`r0035` 含无效值规则）
//!
//! 边界：**只读**、单会话、每次请求 1 项；不写参数、不发控制、不改协议实现。
//! 与 `live_identity_probe` 的分工：那个核对**身份**（轴名/驱动号/对象名），
//! 这个核对**采样值**。两者都不构成工程语义已确认——只证明该地址在该设备上
//! 可读，且质量判定与既有证据一致。
//!
//! 用法：
//! `MESA_NCK_HOST=192.168.15.60 cargo test -p mesa-driver-sinumerik-nck
//!  --test live_samples_probe -- --ignored --nocapture`

use mesa_driver_sinumerik_nck::{
    NativeNckAddress, NckClient, NckConnConfig, PresetRegistry, decode_value, encode_var_spec,
    resolve_native,
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

/// 从模板 + 使用者确认的 area_no/line 构造原生地址（走采集用的同一条链）。
fn from_preset(
    area: &str,
    block: &str,
    variable: &str,
    module: u64,
    area_no: u64,
    line: u64,
) -> NativeNckAddress {
    let reg = PresetRegistry::embedded();
    let prefix = format!("828d.{area}.{block}.{variable}.");
    let t = reg
        .for_family("828d")
        .into_iter()
        .find(|t| {
            t.id.starts_with(&prefix)
                && t.parameters.get("module").and_then(|v| v.as_u64()) == Some(module)
        })
        .cloned()
        .unwrap_or_else(|| panic!("缺模板 {area}/{block}/{variable}/module{module}"));
    let mut p = t.parameters.clone();
    p.insert("area_no".into(), json!(area_no));
    p.insert("line".into(), json!(line));
    NativeNckAddress::from_parameters(&p).expect("模板+确认项应组成合法地址")
}

/// 单次只读并把**请求字节**一并回显（PR-C 要求记录请求包）。
async fn read_one(client: &mut NckClient, label: &str, addr: NativeNckAddress) -> String {
    let rp = match resolve_native(addr) {
        Ok(r) => r,
        Err(e) => return format!("WIRE_ERR {e}"),
    };
    let spec = encode_var_spec(&rp.wire);
    let spec_hex = spec
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let item = mesa_driver_sinumerik_nck::NckReadItem {
        wire: rp.wire,
        expected_data_len: rp.expected_data_len,
        expected_transport_size: rp.expected_transport_size,
    };
    let outcome = match client.read_vars(&[item]).await {
        Err(e) => format!("JOB_ERR {} {}", e.code, e.message),
        Ok(results) => match results.first() {
            None => "NO_RESULT".into(),
            Some(r) if r.return_code != 0xff => {
                format!("ITEM_REJECTED 0x{:02X}", r.return_code)
            }
            Some(r) => match r.data.as_deref() {
                None => "BAD(本地校验：transport/长度不符)".into(),
                Some(bytes) => {
                    let raw = bytes
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    match decode_value(bytes, rp.kind) {
                        // 无效工程值规则随绑定：命中即按 BAD（不伪造协议码）。
                        Ok(v) => match rp.invalid_rule {
                            Some(rule) => {
                                let hit = match &v {
                                    mesa_core_types::Value::F32(x) => rule.hits(*x as f64),
                                    mesa_core_types::Value::F64(x) => rule.hits(*x),
                                    _ => false,
                                };
                                if hit {
                                    format!("BAD(命中无效值规则) value={v:?} raw={raw}")
                                } else {
                                    format!("GOOD value={v:?} raw={raw}")
                                }
                            }
                            None => format!("GOOD value={v:?} raw={raw}"),
                        },
                        Err(e) => format!("DECODE_ERR {e}"),
                    }
                }
            },
        },
    };
    format!("{label:<28} {outcome}   [req {spec_hex}]")
}

#[tokio::test]
#[ignore = "只读真实机床代表性采样；需显式 --ignored 运行"]
async fn live_samples_probe() {
    let cfg = cfg_from_env();
    let mut client = NckClient::connect(&cfg).await.expect("连接失败");
    println!(
        ">>> 已连接 {}:{} pdu={}（只读）",
        cfg.host,
        cfg.port,
        client.negotiated_pdu_length()
    );

    println!("=== 程序状态 / 进给（C/S）===");
    for (label, var) in [
        ("程序状态 progStatus", "progStatus"),
        ("进给速度 actFeedRateIpo", "actFeedRateIpo"),
    ] {
        let a = from_preset("C", "S", var, 127, 1, 1);
        println!("{}", read_one(&mut client, label, a).await);
    }

    println!("=== R 参数（C/RP/rpa，line=R号+1）===");
    for line in [1u64, 2, 100, 426] {
        let a = from_preset("C", "RP", "rpa", 21, 1, line);
        println!(
            "{}",
            read_one(&mut client, &format!("R{}", line - 1), a).await
        );
    }

    println!("=== 轴位置：MCS 设定位置（C/SGA/actToolBasePos，line=机床轴）===");
    for axis in 1..=8u64 {
        let a = from_preset("C", "SGA", "actToolBasePos", 115, 1, axis);
        println!(
            "{}",
            read_one(&mut client, &format!("轴{axis} actToolBasePos"), a).await
        );
    }

    println!("=== 轴位置：BCS 实际位置（C/SEGA/actProgPosBKS，line=机床轴）===");
    for axis in 1..=8u64 {
        let a = from_preset("C", "SEGA", "actProgPosBKS", 112, 1, axis);
        println!(
            "{}",
            read_one(&mut client, &format!("轴{axis} actProgPosBKS"), a).await
        );
    }

    println!("=== 主轴指令转速（C/SSP/cmdSpeed，line=主轴索引）===");
    for spindle in 1..=2u64 {
        let a = from_preset("C", "SSP", "cmdSpeed", 114, 1, spindle);
        println!(
            "{}",
            read_one(&mut client, &format!("主轴{spindle} cmdSpeed"), a).await
        );
    }

    println!("=== SINAMICS 只读驱动参数（V/M，module 26 NC 变量入口）===");
    for var in ["r0027", "r0031", "r0033", "r0035"] {
        for area_no in 1..=4u64 {
            let a = from_preset("V", "M", var, 26, area_no, 1);
            println!(
                "{}",
                read_one(&mut client, &format!("V/{area_no} {var}"), a).await
            );
        }
    }

    println!("=== 同一参数改走显式驱动对象入口（V/M，module 130）===");
    for var in ["r0027", "r0031", "r0033", "r0035"] {
        for area_no in 1..=2u64 {
            let a = from_preset("V", "M", var, 130, area_no, 1);
            println!(
                "{}",
                read_one(&mut client, &format!("V/{area_no}(130) {var}"), a).await
            );
        }
    }

    println!("=== 对照参数：r0078（转矩产生电流）/ r0080（未滤波转矩）===");
    for var in ["r0078", "r0080"] {
        let a = from_preset("V", "M", var, 26, 1, 1);
        println!("{}", read_one(&mut client, &format!("V/1 {var}"), a).await);
    }

    println!("=== r0035 显式无效值规则（-200±0.001；命中即 BAD）===");
    {
        let reg = PresetRegistry::embedded();
        let t = reg
            .for_family("828d")
            .into_iter()
            .find(|t| t.id.starts_with("828d.V.M.r0035.") && t.parameters["module"] == json!(26))
            .cloned()
            .expect("r0035 模板");
        let mut p = t.parameters.clone();
        p.insert("area_no".into(), json!(3));
        p.insert("line".into(), json!(1));
        p.insert("invalid_value".into(), json!(-200.0));
        p.insert("invalid_tolerance".into(), json!(0.001));
        let a = NativeNckAddress::from_parameters(&p).expect("带规则的地址合法");
        println!("{}", read_one(&mut client, "V/3 r0035 (带规则)", a).await);
    }

    println!("<<< 采样结束（只读，未写入任何参数）");
}
