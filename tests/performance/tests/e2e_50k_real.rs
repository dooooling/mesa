//! 真正的 end-to-end 50K throughput benchmark（跨进程 TCP + Protobuf + 单调时钟）
//! 本用例验证：跨进程 TCP + Protobuf 的实际 point throughput + Core Snapshot apply latency + IPC E2E latency（mono_ns）
//! §22 完整预算：≥50K Point Updates/s 持续 60min，IPC p95≤20ms p99≤50ms（单调时钟），RSS 有界

use std::time::{Duration, Instant};

use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode};
use mesa_driver_manager::MesaManager;

fn percentile(mut v: Vec<u64>, p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let idx = ((p / 100.0) * (v.len() as f64 - 1.0)).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn current_rss_bytes() -> Option<u64> {
    // Linux: /proc/self/status VmRSS
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
            for line in s.lines() {
                if line.starts_with("VmRSS:") {
                    // VmRSS:   12345 kB
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            return Some(kb * 1024);
                        }
                    }
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Windows/Mac：尝试 sysinfo 兜底
        let mut sys = sysinfo::System::new_all();
        sys.refresh_memory();
        let pid = sysinfo::Pid::from(std::process::id() as usize);
        if let Some(p) = sys.process(pid) {
            return Some(p.memory());
        }
        None
    }
}

#[tokio::test]
async fn e2e_50k_real_throughput() {
    let soak = std::env::var("PERF_SOAK").is_ok();
    let long = std::env::args().any(|a| a == "--long") || std::env::var("PERF_LONG").is_ok();
    let dur = if soak {
        Duration::from_secs(3600)
    } else if long {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(10)
    };
    let drivers_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
    // 使用内存版 PointIdAllocator（BuiltinEndpoint 不落库），避免 StorePointIdSource 对 ConfigStore endpoint 的强依赖导致 ConfigurationFailed
    let mgr = MesaManager::discover(&drivers_dir);

    // 修复重复 point_key：每 task 独立前缀 p{i}_{j}，确保全量 20 个唯一 key 通过 DUPLICATE_POINT_KEY 校验
    // 4 tasks *5 points * burst 125 /20ms = 每 20ms 2500 点 → 125k/s 理论，满足 50K 基线余量
    let tasks: Vec<AcquisitionTask> = (0..4)
        .map(|i| AcquisitionTask {
            id: format!("t{i}"),
            mode: TaskMode::Poll,
            interval_ms: Some(20),
            binding: DriverBinding {
                kind: "simulator.points".into(),
                config: serde_json::json!({
                    "points": (0..5).map(|j| serde_json::json!({"key": format!("p{i}_{j}"), "kind":"counter", "start":0, "step":1})).collect::<Vec<_>>(),
                    "burst": 125
                }),
            },
        })
        .collect();
    let ep_id = "e2e-50k-real";
    let ep = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: ep_id.into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks,
    };
    mgr.start_endpoint(ep).unwrap();
    let snap = mgr.snapshot();

    // 确认 Endpoint 真正 RUNNING（非仅 running_ids 非空）：state=RUNNING 且 epoch!=0 且 points==20
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut running_ok = false;
    while Instant::now() < deadline {
        if let Some(st) = snap.endpoint(ep_id) {
            if st.state == "RUNNING" && st.epoch != 0 && st.points == 20 {
                running_ok = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        running_ok,
        "Endpoint 未进入 RUNNING/epoch!=0/points==20，当前 {:?}",
        snap.endpoint(ep_id)
    );

    let start_rss = current_rss_bytes();
    let start = Instant::now();
    let start_points = snap.point_value_total();
    let start_env = snap.envelopes_total();
    // 单调时钟下等待 dur，中途不以 latest_all 去重长度估算吞吐
    tokio::time::sleep(dur).await;
    let elapsed = start.elapsed().as_secs_f64();
    let end_rss = current_rss_bytes();
    let end_points = snap.point_value_total();
    let end_env = snap.envelopes_total();
    let delta_points = end_points.saturating_sub(start_points);
    let delta_env = end_env.saturating_sub(start_env);
    let ups = delta_points as f64 / elapsed;

    let lat = snap.snapshot_apply_latencies_snapshot();
    let p50 = percentile(lat.clone(), 50.0);
    let p95 = percentile(lat.clone(), 95.0);
    let p99 = percentile(lat.clone(), 99.0);
    let ipc_lat = snap.ipc_latencies_snapshot();
    let ipc_p50 = percentile(ipc_lat.clone(), 50.0);
    let ipc_p95 = percentile(ipc_lat.clone(), 95.0);
    let ipc_p99 = percentile(ipc_lat.clone(), 99.0);

    println!(
        "e2e_50k_real elapsed={:.2}s delta_points={} delta_env={} ups={:.0} snapshot_apply_p50={}ns p95={}ns p99={}ns ipc_p50={}ns p95={}ns p99={}ns rss_start={:?} rss_end={:?}",
        elapsed, delta_points, delta_env, ups, p50, p95, p99, ipc_p50, ipc_p95, ipc_p99, start_rss, end_rss
    );

    // 强断言：失败必须 fail，无假阳性
    assert!(elapsed >= dur.as_secs_f64() * 0.9, "elapsed {elapsed} 过短");
    assert!(
        delta_env > 0,
        "envelopes_total 必须 >0，否则 DataBatch 未到达 Core"
    );
    assert!(delta_points > 0, "point_value_total 必须 >0");
    // 10s 快检阈值 50K 的 80% 容差，60s/3600s 则严格 50K
    let threshold = if soak || long { 50_000.0 } else { 40_000.0 };
    assert!(
        ups >= threshold,
        "实际 updates/s {ups:.0} 未达阈值 {threshold:.0}（delta {delta_points} / {elapsed:.1}s），不满足 §22 50K"
    );
    assert!(!lat.is_empty(), "snapshot_apply samples must >0");
    assert!(p95 != 0, "snapshot_apply p95 must >0");
    assert!(p95 <= 20_000_000, "snapshot_apply_p95 {p95}ns >20ms");
    assert!(p99 <= 50_000_000, "snapshot_apply_p99 {p99}ns >50ms");
    assert!(
        !ipc_lat.is_empty(),
        "ipc samples must >0 (mono_ns 未埋点或跨进程时钟不可比)"
    );
    assert!(ipc_p95 != 0, "ipc p95 must >0");
    if soak {
        // Release Soak 正式 SLO：p95 ≤20ms / p99 ≤50ms
        assert!(
            ipc_p95 <= 20_000_000,
            "ipc_p95 {ipc_p95}ns >20ms (soak SLO)"
        );
        assert!(
            ipc_p99 <= 50_000_000,
            "ipc_p99 {ipc_p99}ns >50ms (soak SLO)"
        );
    } else {
        // 普通 GitHub CI（共享 Runner）仅作灾难性回归门槛：p95 <50ms / p99 <100ms
        assert!(
            ipc_p95 <= 50_000_000,
            "ipc_p95 {ipc_p95}ns >50ms (CI disaster gate)"
        );
        assert!(
            ipc_p99 <= 100_000_000,
            "ipc_p99 {ipc_p99}ns >100ms (CI disaster gate)"
        );
    }
    // Soak 模式：RSS 60min 增长 ≤10%，必须可读取否则 fail-closed
    if soak {
        let s = start_rss.expect("PERF_SOAK 必须能够读取 start RSS");
        let e = end_rss.expect("PERF_SOAK 必须能够读取 end RSS");
        let growth = if s > 0 {
            (e as f64 - s as f64) / s as f64 * 100.0
        } else {
            0.0
        };
        println!("rss soak growth {growth:.1}% start {s} end {e}");
        assert!(
            growth <= 10.0,
            "RSS 增长 {growth:.1}% >10% (start {s} end {e})，疑似泄漏"
        );
    }

    // 最终仍需 RUNNING
    let st = snap.endpoint(ep_id).expect("endpoint still present");
    assert_eq!(st.state, "RUNNING", "结束时仍应 RUNNING，实际 {:?}", st);

    mgr.shutdown_all().await;
}
