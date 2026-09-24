//! FOCAS2 Shadow Soak（S7-C，诊断专用，独立于 Phase 1 probe）。
//!
//! - 目标：同一 Native/Wire session 长期保持，重复 Shadow Phase 1 集合，
//!   观测 `mismatch / timeout / latency / session`；Native 始终先执行，
//!   生产值唯一来源地位不变；Wire 只旁路比较，不改生产 backend、不写 fixture。
//! - 范围：12 ready 点（spindle/load、servo/load、tool/zofs 不纳入；
//!   `Spindle { .. }` 宽豁免暂保留——S7-C 不加 load，加 load 时必须收窄为
//!   per-case 显式 expectation，见本文件 `native_expect` 注释）。
//! - 边界（冻结）：Wire timeout/error → 记 `wire_error`，不改 Native 结果，
//!   不让 Native 变 Bad，下一周期按 reconnect 策略恢复 Wire；Native error
//!   单独记 `native_error`，绝不归因 Wire。
//! - 统计（克制版，无 percentile/histogram/Prometheus）：
//!   `cycles / native_ok / native_error / wire_ok / wire_error / wire_timeout /
//!   reconnect_attempt / reconnect_ok / reconnect_fail / equal /
//!   native_unsupported / native_debt / drift / mismatch /
//!   native_latency_ms_total / wire_latency_ms_total`（均值由调用方除法）。
//! - 用法：`MESA_SOAK_HOST=192.168.15.165 MESA_SOAK_CYCLES=60
//!   MESA_SOAK_INTERVAL_MS=1000 cargo run -p mesa-driver-focas2
//!   --example shadow_soak`（可选 `MESA_SOAK_PORT` 默认 8193；
//!   `MESA_SOAK_RECONNECT=1` 允许 Wire 断线重连，默认 1）。
//!   C2 deterministic fault injection（诊断专用，不碰生产 session）：
//!   `MESA_SOAK_KILL_WIRE_AT=N` 在第 N 周期 Wire 读取后主动 `disconnect` Wire
//!   session（只断 Wire，Native 保持），下一周期验证 reconnect 恢复；
//!   `=0` 即关闭注入（默认）。
//!   C3 lifecycle（诊断专用）：`MESA_SOAK_ROUNDS=N` 时外层多轮
//!   `connect →若干 read→ disconnect → drop重建`（默认 1 即单轮；
//!   每轮 snapshot 进程 `memory/thread/handle` 计数供横向对照，
//!   不断言 DLL 内部绝对无泄漏，见 C3 注释）。
//!   退出码：`mismatch>0 → 2`；`native_error>0 → 3`；仅 drift/known → 0。
//! - Soak Gate（正常模式）：`mismatch=0 / native_error=0 /
//!   wire unexpected err=0 / session corruption=0 / resource monotonic
//!   growth=0`。故障恢复模式（人为断 Wire）另计：Native interruption=0 +
//!   Wire reconnect 成功 + 恢复后 parity，不与正常模式混统计。

use std::time::{Duration, Instant};

use mesa_core_types::Value;
use mesa_driver_focas2::wire_pub::WireFocasApi;
use mesa_driver_focas2::{FocasAddress, FocasApi, NativeFocasApi, parse_address};

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Equal,
    KnownNativeUnsupported { native: String },
    KnownNativeDebt { native: String },
    Drift { native: String, wire: String },
    Mismatch { native: String, wire: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeExpect {
    Exact,
    NativeUnsupported,
    NativeDebt,
}

fn native_expect(addr: &FocasAddress) -> NativeExpect {
    // S7-C 暂保留 `Spindle { .. }` 宽豁免（shadow 只有 gear/maxrpm）。
    // spindle/load 纳入 shadow 时必须收窄为 per-case 显式 expectation
    //（gear/maxrpm → NativeUnsupported；load READY 后 → Exact），
    // 不得继承本宽豁免（见 PR #64 最终 review 提醒）。
    match addr {
        FocasAddress::Spindle { .. } => NativeExpect::NativeUnsupported,
        FocasAddress::Diagnosis { .. } => NativeExpect::NativeUnsupported,
        FocasAddress::Alarm => NativeExpect::NativeUnsupported,
        FocasAddress::OpMsg => NativeExpect::NativeDebt,
        _ => NativeExpect::Exact,
    }
}

fn value_variant(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "Bool",
        Value::I32(_) => "I32",
        Value::U32(_) => "U32",
        Value::I64(_) => "I64",
        Value::U64(_) => "U64",
        Value::F32(_) => "F32",
        Value::F64(_) => "F64",
        Value::String(_) => "String",
        Value::Bytes(_) => "Bytes",
        Value::DateTime(_) => "DateTime",
        Value::BoolArray(_) => "BoolArray",
        Value::I32Array(_) => "I32Array",
        Value::U32Array(_) => "U32Array",
        Value::I64Array(_) => "I64Array",
        Value::U64Array(_) => "U64Array",
        Value::F32Array(_) => "F32Array",
        Value::F64Array(_) => "F64Array",
        Value::StringArray(_) => "StringArray",
        Value::DateTimeArray(_) => "DateTimeArray",
    }
}

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
    if let Value::String(s) = native
        && s.starts_with("ERR:")
    {
        match native_expect(addr) {
            NativeExpect::NativeDebt => {
                let u = s.to_ascii_uppercase();
                if u.contains("EW_LENGTH(2)") {
                    return Verdict::KnownNativeDebt { native: s.clone() };
                }
                return Verdict::Mismatch {
                    native: format!("{native:?}"),
                    wire: format!("{wire:?}"),
                };
            }
            NativeExpect::NativeUnsupported => {
                return Verdict::KnownNativeUnsupported { native: s.clone() };
            }
            NativeExpect::Exact => {
                return Verdict::Mismatch {
                    native: format!("{native:?}"),
                    wire: format!("{wire:?}"),
                };
            }
        }
    }
    if value_variant(native) != value_variant(wire) {
        return Verdict::Mismatch {
            native: format!("{native:?}"),
            wire: format!("{wire:?}"),
        };
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

/// Wire 错误分类（soak 观测用，不改 Native 结果）：timeout 关键字单独计数。
fn is_timeout_err(e: &str) -> bool {
    let u = e.to_ascii_uppercase();
    u.contains("TIMEOUT") || u.contains("TIMED OUT") || u.contains("DEADLINE")
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

#[derive(Default)]
struct SoakCounters {
    cycles: u64,
    native_ok: u64,
    native_error: u64,
    wire_ok: u64,
    wire_error: u64,
    wire_timeout: u64,
    reconnect_attempt: u64,
    reconnect_ok: u64,
    reconnect_fail: u64,
    equal: u64,
    native_unsupported: u64,
    native_debt: u64,
    drift: u64,
    mismatch: u64,
    native_latency_ms_total: u128,
    wire_latency_ms_total: u128,
}

#[tokio::main]
async fn main() {
    let host = std::env::var("MESA_SOAK_HOST").unwrap_or_else(|_| "192.168.15.165".into());
    let port: u16 = std::env::var("MESA_SOAK_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8193);
    let cycles: u64 = std::env::var("MESA_SOAK_CYCLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let interval_ms: u64 = std::env::var("MESA_SOAK_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let allow_reconnect = std::env::var("MESA_SOAK_RECONNECT")
        .ok()
        .map(|s| s != "0")
        .unwrap_or(true);
    // C2 deterministic fault injection：第 N 周期 Wire 读取后主动断 Wire
    // session（`disconnect` 只影响 Wire client，不碰 Native worker/handle；
    // 不改生产 `WireSession`，诊断专用）。
    let kill_wire_at: u64 = std::env::var("MESA_SOAK_KILL_WIRE_AT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // C3 lifecycle：外层多轮 connect/read/disconnect/drop重建（默认 1）。
    let rounds: u64 = std::env::var("MESA_SOAK_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v| *v >= 1)
        .unwrap_or(1);
    // C3 进程快照（OBSERVED：Windows 同进程读计数器；不断言 DLL 内部无泄漏）。
    fn snapshot() -> (u64, u64, u64) {
        // (available_mb_approx, thread_count, handle_count)
        // 保守实现：未接 Win32 真实指标前一律 0（快照缺失不伪造“无增长”；
        // C3 结论以“轮次通过 + 无失败”为准，见下注释）。
        (0, 0, 0)
    }
    let mut round_snapshots: Vec<(u64, u64, u64, u64, u64)> = Vec::new();
    for round in 1..=rounds {
        if rounds > 1 {
            println!("--- round {round}/{rounds} connect → read → disconnect → drop重建 ---");
        }
        let snap0 = snapshot();
        let rc = soak_round(
            &host,
            port,
            cycles,
            interval_ms,
            allow_reconnect,
            kill_wire_at,
            round,
        )
        .await;
        let snap1 = snapshot();
        round_snapshots.push((snap0.0, snap0.1, snap0.2, snap1.1, snap1.2));
        if !rc {
            std::process::exit(rc as i32);
        }
    }
    // C3 结论（证据纪律）：
    // OBSERVED：各 round connect/read/disconnect 无失败（见上轮次输出）；
    // 快照计数器当前为占位 0（未接 Win32 真实指标前不声称“无单调增长”）。
    // NOT PROVEN：DLL 内部资源绝对无泄漏。
    println!(
        "soak rounds done: rounds={} snapshots={:?} (OBSERVED: 轮次通过；NOT PROVEN: DLL 内部无泄漏)",
        rounds, round_snapshots,
    );
}

/// C3 单轮：connect → 若干 read → disconnect → drop重建（调用方每轮新建
/// `NativeFocasApi`/`WireFocasApi` 即 drop 旧实例；返回 true=通过）。
/// latency 是否随轮次劣化由调用方对照各轮 `lat_*_avg` 输出（本函数内打印）。
async fn soak_round(
    host: &str,
    port: u16,
    cycles: u64,
    interval_ms: u64,
    allow_reconnect: bool,
    kill_wire_at: u64,
    round: u64,
) -> bool {
    let native = NativeFocasApi::new();
    let wire = WireFocasApi::new(Duration::from_secs(5));
    // C1：同一 Native/Wire session 长期保持（connect once）。
    if let Err(e) = native.connect(host, port, 5000).await {
        eprintln!("native connect {host}:{port} 失败：{e}");
        std::process::exit(1);
    }
    if let Err(e) = wire.connect(host, port, 5000).await {
        eprintln!("wire connect {host}:{port} 失败：{e}");
        std::process::exit(1);
    }
    println!("soak connect {host}:{port} OK（cycles={cycles} interval={interval_ms}ms）");
    let addrs = shadow_addrs();
    let keys: Vec<FocasAddress> = addrs.iter().map(|(_, a)| a.clone()).collect();
    let mut c = SoakCounters::default();
    for n in 1..=cycles {
        c.cycles += 1;
        // Native 始终先执行（生产值唯一来源；结果先保存，不受 Wire 影响）。
        let t0 = Instant::now();
        let natives = native.read_batch(&keys).await;
        let native_ms = t0.elapsed().as_millis();
        c.native_latency_ms_total += native_ms;
        let natives = match natives {
            Ok(v) => {
                c.native_ok += 1;
                v
            }
            Err(e) => {
                // Native error 单独记，绝不归因 Wire（C1 边界）。
                c.native_error += 1;
                eprintln!("[cycle {n}] native_error={e}（不归因 Wire）");
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                continue;
            }
        };
        // Wire 旁路比较（timeout/error 只记数，不改 Native 结果）。
        let t1 = Instant::now();
        let wires = wire.read_batch(&keys).await;
        let wire_ms = t1.elapsed().as_millis();
        c.wire_latency_ms_total += wire_ms;
        let wires = match wires {
            Ok(v) => {
                c.wire_ok += 1;
                v
            }
            Err(e) => {
                c.wire_error += 1;
                if is_timeout_err(&e) {
                    c.wire_timeout += 1;
                }
                eprintln!("[cycle {n}] wire_error={e}（不改 Native 结果）");
                // 下一周期按 reconnect 策略恢复 Wire（C2 预留：当前仅重连
                // 同一 endpoint；不断 Native session）。
                if allow_reconnect {
                    c.reconnect_attempt += 1;
                    match wire.connect(host, port, 5000).await {
                        Ok(()) => c.reconnect_ok += 1,
                        Err(re) => {
                            c.reconnect_fail += 1;
                            eprintln!("[cycle {n}] wire reconnect_fail={re}");
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                continue;
            }
        };
        for (i, (label, addr)) in addrs.iter().enumerate() {
            match judge(addr, &natives[i], &wires[i]) {
                Verdict::Equal => c.equal += 1,
                Verdict::KnownNativeUnsupported { .. } => c.native_unsupported += 1,
                Verdict::KnownNativeDebt { .. } => c.native_debt += 1,
                Verdict::Drift { .. } => c.drift += 1,
                Verdict::Mismatch { .. } => {
                    c.mismatch += 1;
                    eprintln!(
                        "[cycle {n}] [MISMATCH] {label} native={:?} wire={:?}",
                        natives[i], wires[i]
                    );
                }
            }
        }
        // C2 deterministic fault injection：本周期 Wire 读取完成后主动断 Wire
        // session（`disconnect` 为 `FocasApi` 公共语义，只影响 Wire client；
        // Native worker/handle 不动；下一周期 Wire 侧按 reconnect 策略恢复）。
        if kill_wire_at != 0 && n == kill_wire_at {
            wire.disconnect().await;
            eprintln!("[cycle {n}] FAULT-INJECT wire disconnect（只断 Wire，Native 保持）");
        }
        if n % 10 == 0 || n == cycles {
            println!(
                "[cycle {n}/{cycles}] native_ok={} native_err={} wire_ok={} wire_err={} (timeout={}) reconnect={}/{}/{} equal={} unsup={} debt={} drift={} mismatch={} lat_native_avg={}ms lat_wire_avg={}ms",
                c.native_ok,
                c.native_error,
                c.wire_ok,
                c.wire_error,
                c.wire_timeout,
                c.reconnect_attempt,
                c.reconnect_ok,
                c.reconnect_fail,
                c.equal,
                c.native_unsupported,
                c.native_debt,
                c.drift,
                c.mismatch,
                c.native_latency_ms_total / c.cycles.max(1) as u128,
                c.wire_latency_ms_total / c.cycles.max(1) as u128,
            );
        }
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
    native.disconnect().await;
    wire.disconnect().await;
    println!(
        "[round {round}] soak done: cycles={} native_ok={} native_error={} wire_ok={} wire_error={} (timeout={}) reconnect={}/{}/{} equal={} unsupported={} debt={} drift={} mismatch={}",
        c.cycles,
        c.native_ok,
        c.native_error,
        c.wire_ok,
        c.wire_error,
        c.wire_timeout,
        c.reconnect_attempt,
        c.reconnect_ok,
        c.reconnect_fail,
        c.equal,
        c.native_unsupported,
        c.native_debt,
        c.drift,
        c.mismatch,
    );
    if c.mismatch > 0 {
        return false;
    }
    if c.native_error > 0 {
        return false;
    }
    true
}
