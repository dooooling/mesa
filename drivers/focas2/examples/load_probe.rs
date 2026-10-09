//! `load_probe`（阶段 A live 定标，只读，不进 production）：
//!
//! - 用法：`MESA_LOAD_HOST=192.168.15.165 cargo run -p mesa-driver-focas2
//!   --example load_probe`（可选 `MESA_LOAD_PORT`，默认 8193）。
//! - A 诊断：spindle `type=0/1/-1` 依次抓完整 REQUEST/RESPONSE frame hex +
//!   decoder 结果（只读一次，不重试，不猜参数）；servo 沿用已验证四槽探针。
//! - 输出为 Evidence Day 归档格式（含机床/时间/selector/commit/原始字节）。

use std::time::Duration;

use mesa_driver_focas2::wire_pub::WireFocasApi;
use mesa_driver_focas2::FocasApi;

fn commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

#[tokio::main]
async fn main() {
    let host = std::env::var("MESA_LOAD_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("MESA_LOAD_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8193);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ver = commit();
    let api = WireFocasApi::new(Duration::from_secs(5));
    if let Err(e) = api.connect(&host, port, 5000).await {
        eprintln!("wire connect {host}:{port} 失败：{e}");
        std::process::exit(1);
    }
    println!("wire connect {host}:{port} OK");
    match api.system_info().await {
        Ok(info) => println!("system_info: series={} version={}", info.series, info.version),
        Err(e) => eprintln!("system_info 失败：{e}"),
    }
    for t in [0, 1, -1] {
        // 每 type 独立重连（fail-closed 下 fatal 即杀 session，不连带后测）。
        if let Err(e) = api.connect(&host, port, 5000).await {
            eprintln!("reconnect 失败：{e}");
            break;
        }
        let cap = api.capture_spindle_meter(t, 2).await;
        println!("RUN=SPINDLE_TYPE_{t} host={host}:{port} unix_s={now} commit={ver}");
        println!("REQUEST:");
        println!("  origin=0x{:04x}", cap.req_origin);
        println!("  packet_type=0x{:04x}", cap.req_packet_type);
        println!("  payload_length={}", cap.req_frame_hex.len() / 2);
        println!("  raw_frame_hex={}", cap.req_frame_hex);
        println!("RESPONSE:");
        match (&cap.resp_origin, &cap.resp_frame_hex) {
            (Some(o), Some(hex)) => {
                println!("  origin=0x{o:04x}");
                println!(
                    "  packet_type=0x{:04x}",
                    cap.resp_packet_type.unwrap_or(0)
                );
                println!("  payload_length={}", hex.len() / 2);
                println!("  raw_frame_hex={hex}");
            }
            _ => println!("  <no response: exchange failed>"),
        }
        println!("DECODE:");
        println!("  result={}", cap.decode);
    }
    if let Err(e) = api.connect(&host, port, 5000).await {
        eprintln!("reconnect 失败：{e}");
    }
    match api.probe_servo_load_new(4).await {
        Ok(sv) => {
            for (i, r) in sv.records.iter().enumerate() {
                println!(
                    "servo rec{i}: axis={:02x?} raw={} dec={} aux={:02x?}",
                    r.axis_raw, r.numeric.raw, r.numeric.dec_bits, r.numeric.aux4
                );
            }
        }
        Err(e) => eprintln!("servo 失败：{e}"),
    }
    api.disconnect().await;
    println!("done");
}
