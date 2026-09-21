//! `wire_probe`（PR1 开发工具 + PR2 feed + PR3 axis + PR54 spindle + PR55 macro
//! + PR56 pmc scalar + PR57 param + PR58 opmsg，非生产路径）：Wire 直连真机验证。
//!
//! - 用法：`MESA_WIRE_HOST=192.168.15.165 cargo run -p mesa-driver-focas2
//!   --example wire_probe`（可选 `MESA_WIRE_PORT`，默认 8193）。
//! - 只调 `system_info` + `status/feed/axis.absolute/spindle_speed/macro/pmc/param/opmsg`，打印结果；
//!   不碰 Native、不改生产 backend、不写 fixture。
//! - Gate 0 期望（165）：series=G31Z/version=10.0，
//!   `StatusInfo.aut` 与面板 mode 一致（MEM=1/MDI=0）；feed/axis/spindle/macro/pmc/param 与 Native 一致；
//!   opmsg 与 panel #3006 一致（Native selector/ABI 待独立闭合，见 PR58）。

use std::time::Duration;

use mesa_driver_focas2::wire_pub::WireFocasApi;
use mesa_driver_focas2::{FocasAddress, FocasApi, parse_address};

#[tokio::main]
async fn main() {
    let host = std::env::var("MESA_WIRE_HOST").unwrap_or_else(|_| "192.168.15.165".into());
    let port: u16 = std::env::var("MESA_WIRE_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8193);
    let api = WireFocasApi::new(Duration::from_secs(5));
    if let Err(e) = api.connect(&host, port, 5000).await {
        eprintln!("wire connect {host}:{port} 失败：{e}");
        std::process::exit(1);
    }
    println!("wire connect {host}:{port} OK");
    match api.system_info().await {
        Ok(info) => println!(
            "system_info: series={} version={}",
            info.series, info.version
        ),
        Err(e) => eprintln!("system_info 失败：{e}"),
    }
    let addrs = vec![
        parse_address("status").expect("status 地址合法"),
        parse_address("feed").expect("feed 地址合法"),
        parse_address("axis.abs.1").expect("axis1 地址合法"),
        parse_address("axis.abs.2").expect("axis2 地址合法"),
        parse_address("axis.abs.3").expect("axis3 地址合法"),
        FocasAddress::ActiveSpindleSpeed,
        parse_address("macro.501").expect("macro 地址合法"),
        parse_address("pmc.R100").expect("pmc R100 地址合法"),
        parse_address("pmc.Y0").expect("pmc Y0 地址合法"),
        parse_address("pmc.F0").expect("pmc F0 地址合法"),
        parse_address("pmc.D0").expect("pmc D0 地址合法"),
        parse_address("param.6711").expect("param 地址合法"),
        parse_address("opmsg").expect("opmsg 地址合法"),
    ];
    match api.read_batch(&addrs).await {
        Ok(vals) => {
            println!("status+feed+axis123+spindle+macro501+pmc+param6711+opmsg -> {vals:?}")
        }
        Err(e) => eprintln!("read_batch 失败：{e}"),
    }
    api.disconnect().await;
    println!("done");
}
