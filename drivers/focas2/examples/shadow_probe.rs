//! FOCAS2 Native/Wire 双跑影子对照（Shadow Phase 1，诊断专用）。
//!
//! - 目标：同机（`MESA_SHADOW_HOST`，默认 `192.168.15.165`）同任务先后跑
//!   Native（`NativeFocasApi`，生产值唯一来源）与 Wire（`WireFocasApi`，
//!   旁路比较），输出 12/17 parity sweep 对照；不改生产 backend、不写 fixture。
//! - 范围：`docs/wire-cutover-matrix.md` 的 12 ready 点；servo/spindle load
//!   value、tool/zofs 不纳入（HOLD，按 cutover matrix 记 documented）。
//! - 比较语义：`Value` 全等（含变体；`F64` 按位比较，不做 epsilon）；
//!   Native PR52 `ERR:` 占位（Gear/MaxRpm/Diagnosis/Alarm）与 Wire 真实值
//!   的差异为已知生产边界，记 `KnownBoundary`，不计 mismatch。
//! - 动态漂移（axis 位置、diagnosis REAL、spindle speed 等随现场变化的值）
//!   只要两侧同窗先后、差值在现场变化解释内，记 `Drift`，不计 mismatch。
//!   真正同窗同地址 `Value` 不等且非动态，记 `Mismatch`（blocker）。
//! - 用法：`MESA_SHADOW_HOST=192.168.15.165 cargo run -p mesa-driver-focas2
//!   --example shadow_probe`（可选 `MESA_SHADOW_PORT`，默认 8193）。
//!   退出码：`Mismatch>0 → 2`；仅 Drift/KnownBoundary → 0。

use std::time::Duration;

use mesa_core_types::Value;
use mesa_driver_focas2::wire_pub::WireFocasApi;
use mesa_driver_focas2::{FocasAddress, FocasApi, NativeFocasApi, parse_address};

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// 两侧全等。
    Equal,
    /// 已知生产边界（Native PR52 `ERR:` vs Wire 真实值；
    /// gear/maxrpm/diagnosis/alarm）。
    KnownNativeUnsupported { native: String },
    /// Native 实现 debt（opmsg：Native `ERR:`（当前 `EW_Length(2)`）vs Wire
    /// 有效结果；不记 mismatch，不伪造 Native 值；见 cutover matrix debt 项）。
    KnownNativeDebt { native: String },
    /// 动态值漂移（两侧都成功但值不等，且该点为已知动态点）。
    Drift { native: String, wire: String },
    /// 真 mismatch（blocker）。
    Mismatch { native: String, wire: String },
}

/// 已知动态点（随现场变化，值不等不记 mismatch）：轴位置、spindle speed、
/// diagnosis REAL（#301 随 Z 变化）、feed、macro（若现场在跑）。
fn is_dynamic(addr: &FocasAddress) -> bool {
    matches!(
        addr,
        FocasAddress::Axis { .. }
            | FocasAddress::ActiveSpindleSpeed
            | FocasAddress::Feed
            | FocasAddress::MacroVar { .. }
            | FocasAddress::Diagnosis { .. }
    )
}

fn judge(addr: &FocasAddress, native: &Value, wire: &Value) -> Verdict {
    if native == wire {
        return Verdict::Equal;
    }
    // Native PR52 占位 vs Wire 真实值：已知边界（Batch 1/2/3 豁免项）；
    // opmsg 另归 Native debt（`EW_Length(2)` vs 有效结果，不伪造 Native 值）。
    if let Value::String(s) = native
        && s.starts_with("ERR:")
    {
        if matches!(addr, FocasAddress::OpMsg) {
            return Verdict::KnownNativeDebt { native: s.clone() };
        }
        return Verdict::KnownNativeUnsupported { native: s.clone() };
    }
    if is_dynamic(addr) {
        return Verdict::Drift {
            native: format!("{native:?}"),
            wire: format!("{wire:?}"),
        };
    }
    Verdict::Mismatch {
        native: format!("{native:?}"),
        wire: format!("{wire:?}"),
    }
}

fn shadow_addrs() -> Vec<(String, FocasAddress)> {
    vec![
        ("machine.status".into(), parse_address("status").unwrap()),
        ("machine.feed".into(), parse_address("feed").unwrap()),
        ("axis.abs.1".into(), parse_address("axis.abs.1").unwrap()),
        ("axis.abs.2".into(), parse_address("axis.abs.2").unwrap()),
        ("axis.abs.3".into(), parse_address("axis.abs.3").unwrap()),
        (
            "machine.spindle_speed".into(),
            FocasAddress::ActiveSpindleSpeed,
        ),
        ("macro.501".into(), parse_address("macro.501").unwrap()),
        ("pmc.R100".into(), parse_address("pmc.R100").unwrap()),
        ("pmc.Y0".into(), parse_address("pmc.Y0").unwrap()),
        ("pmc.F0".into(), parse_address("pmc.F0").unwrap()),
        ("pmc.D0".into(), parse_address("pmc.D0").unwrap()),
        ("param.6711".into(), parse_address("param.6711").unwrap()),
        ("opmsg".into(), parse_address("opmsg").unwrap()),
        (
            "spindle.gear.1".into(),
            parse_address("spindle.gear.1").unwrap(),
        ),
        (
            "spindle.maxrpm.1".into(),
            parse_address("spindle.maxrpm.1").unwrap(),
        ),
        (
            "diagnosis.301@axis3".into(),
            FocasAddress::Diagnosis {
                number: 301,
                axis: 3,
            },
        ),
        ("alarm".into(), parse_address("alarm").unwrap()),
    ]
}

#[tokio::main]
async fn main() {
    let host = std::env::var("MESA_SHADOW_HOST").unwrap_or_else(|_| "192.168.15.165".into());
    let port: u16 = std::env::var("MESA_SHADOW_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8193);
    let native = NativeFocasApi::new();
    let wire = WireFocasApi::new(Duration::from_secs(5));
    if let Err(e) = native.connect(&host, port, 5000).await {
        eprintln!("native connect {host}:{port} 失败：{e}");
        std::process::exit(1);
    }
    if let Err(e) = wire.connect(&host, port, 5000).await {
        eprintln!("wire connect {host}:{port} 失败：{e}");
        std::process::exit(1);
    }
    println!("shadow connect {host}:{port} OK（native=生产值唯一来源，wire=旁路比较）");
    let addrs = shadow_addrs();
    let natives = native
        .read_batch(&addrs.iter().map(|(_, a)| a.clone()).collect::<Vec<_>>())
        .await
        .unwrap_or_else(|e| {
            eprintln!("native read_batch 失败：{e}");
            std::process::exit(1);
        });
    let wires = wire
        .read_batch(&addrs.iter().map(|(_, a)| a.clone()).collect::<Vec<_>>())
        .await
        .unwrap_or_else(|e| {
            eprintln!("wire read_batch 失败：{e}");
            std::process::exit(1);
        });
    let mut n_mismatch = 0;
    let mut n_drift = 0;
    let mut n_unsup = 0;
    let mut n_debt = 0;
    for (i, (label, addr)) in addrs.iter().enumerate() {
        let (nv, wv) = (&natives[i], &wires[i]);
        match judge(addr, nv, wv) {
            Verdict::Equal => println!("[EQUAL] {label} = {nv:?}"),
            Verdict::KnownNativeUnsupported { native } => {
                n_unsup += 1;
                println!("[KNOWN_NATIVE_UNSUPPORTED] {label} native={native} wire={wv:?}");
            }
            Verdict::KnownNativeDebt { native } => {
                n_debt += 1;
                println!(
                    "[KNOWN_NATIVE_DEBT] {label} native={native} wire={wv:?}（Native debt，不伪造）"
                );
            }
            Verdict::Drift { native, wire } => {
                n_drift += 1;
                println!("[DRIFT] {label} native={native} wire={wire}（动态值，同窗先后）");
            }
            Verdict::Mismatch { native, wire } => {
                n_mismatch += 1;
                println!("[MISMATCH] {label} native={native} wire={wire}（blocker）");
            }
        }
    }
    native.disconnect().await;
    wire.disconnect().await;
    println!(
        "shadow done: equal={} unsupported={} debt={} drift={} mismatch={}",
        addrs.len() - n_mismatch - n_drift - n_unsup - n_debt,
        n_unsup,
        n_debt,
        n_drift,
        n_mismatch
    );
    if n_mismatch > 0 {
        std::process::exit(2);
    }
}
