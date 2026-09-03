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

#[allow(dead_code)]
fn current_rss_bytes() -> Option<u64> {
    // Linux: /proc/self/status VmRSS（无 self-perturbation）
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
        // Windows/Mac：sysinfo 兜底（已废弃 new_all 高扰动路径，改为持久 Probe）
        // 此函数仅保留用于非 Soak 快速路径；Soak 下使用 RssProbe::sample()
        let mut sys = sysinfo::System::new();
        let pid = sysinfo::Pid::from(std::process::id() as usize);
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            false,
            sysinfo::ProcessRefreshKind::new().with_memory(),
        );
        if let Some(p) = sys.process(pid) {
            return Some(p.memory());
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
struct RssProbe {
    sys: sysinfo::System,
    pid: sysinfo::Pid,
}

#[cfg(not(target_os = "linux"))]
impl RssProbe {
    fn new() -> Self {
        let pid = sysinfo::Pid::from(std::process::id() as usize);
        Self {
            sys: sysinfo::System::new(),
            pid,
        }
    }

    fn sample(&mut self) -> Option<u64> {
        self.sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[self.pid]),
            false,
            sysinfo::ProcessRefreshKind::new().with_memory(),
        );
        self.sys.process(self.pid).map(|p| p.memory())
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
    // 测试开始即删旧 Evidence，避免失败后遗留假阳性（diagnostic 亦清理，避免旧曲线残留）
    {
        let out_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/validation");
        let _ = std::fs::remove_file(out_dir.join("performance.json"));
        let _ = std::fs::remove_file(out_dir.join("soak.json"));
        let _ = std::fs::remove_file(out_dir.join("soak-diagnostic.json"));
    }
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

    // P1-1/2：持久化 RSS Probe，避免每分钟 System::new_all() 自扰动；
    // 在 warm-up 前即创建并预热一次，使 probe 自身分配不污染基线。
    #[cfg(not(target_os = "linux"))]
    let mut rss_probe = {
        let mut p = RssProbe::new();
        let _ = p.sample();
        p
    };

    // Soak 测量模型：warm-up 5min 后取稳态基线，再正式 60min 验收；阈值仍 §22 的 10%，
    // 但 start 已为高水位后的稳态值，避免冷启动基线误判。期间每 60s 采样一次用于趋势判断。
    let warmup = if soak {
        Duration::from_secs(300)
    } else {
        Duration::from_secs(0)
    };
    if warmup.as_secs() > 0 {
        println!("warm-up {}s ...", warmup.as_secs());
        tokio::time::sleep(warmup).await;
    }
    // warm-up 后立即确认 Endpoint 仍 RUNNING，避免无效长跑
    if soak {
        let st_warm = snap
            .endpoint(ep_id)
            .expect("endpoint after warmup still present");
        assert_eq!(
            st_warm.state, "RUNNING",
            "warm-up 后仍应 RUNNING，实际 {:?}",
            st_warm
        );
    }
    // fail-closed：Soak 下 warm-up 后必须可读取稳态基线（Probe 复用）
    #[cfg(target_os = "linux")]
    let start_rss = if soak {
        Some(current_rss_bytes().expect("PERF_SOAK warm-up 后必须能够读取 RSS baseline"))
    } else {
        current_rss_bytes()
    };
    #[cfg(not(target_os = "linux"))]
    let start_rss = if soak {
        Some(
            rss_probe
                .sample()
                .expect("PERF_SOAK warm-up 后必须能够读取 RSS baseline"),
        )
    } else {
        rss_probe.sample()
    };
    let steady_rss_mib = start_rss.map(|v| v as f64 / (1024.0 * 1024.0));
    println!(
        "steady baseline rss={:?} ({:.1?} MiB) warmup={}s",
        start_rss,
        steady_rss_mib,
        warmup.as_secs()
    );
    let start = Instant::now();
    let start_points = snap.point_value_total();
    let start_env = snap.envelopes_total();
    // 周期采样 RSS（每 60s），fail-closed：Soak 下每次采样必须成功
    let mut rss_samples: Vec<u64> = Vec::new();
    if let Some(v) = start_rss {
        rss_samples.push(v);
    }
    let elapsed: f64;
    let mut end_rss = start_rss;
    if soak {
        let sample_interval = Duration::from_secs(60);
        let mut remaining = dur;
        while remaining > Duration::from_secs(0) {
            let step = remaining.min(sample_interval);
            tokio::time::sleep(step).await;
            #[cfg(target_os = "linux")]
            let v = current_rss_bytes().expect("PERF_SOAK 周期 RSS 采样失败");
            #[cfg(not(target_os = "linux"))]
            let v = rss_probe.sample().expect("PERF_SOAK 周期 RSS 采样失败");
            rss_samples.push(v);
            end_rss = Some(v);
            remaining = remaining.saturating_sub(step);
        }
        elapsed = start.elapsed().as_secs_f64();
        // Soak 强制 sample 完整性：baseline 1 + 3600/60 = 61
        let expected_samples = 1 + dur.as_secs() / sample_interval.as_secs();
        assert_eq!(
            rss_samples.len() as u64,
            expected_samples,
            "RSS sample count incomplete"
        );
    } else {
        tokio::time::sleep(dur).await;
        elapsed = start.elapsed().as_secs_f64();
        #[cfg(target_os = "linux")]
        {
            end_rss = current_rss_bytes();
        }
        #[cfg(not(target_os = "linux"))]
        {
            end_rss = rss_probe.sample();
        }
        if let Some(v) = end_rss {
            rss_samples.push(v);
        }
    }
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
    // Soak 模式：RSS 60min 增长 ≤10%（基于 warm-up 后稳态基线），必须可读取否则 fail-closed
    // 周期采样用于事后趋势分析：若前 5min 涨到高水位后稳定属正常，若持续正斜率则为泄漏。
    let rss_peak = rss_samples.iter().copied().max();
    let rss_sample_count = rss_samples.len();
    if soak {
        let s = start_rss.expect("PERF_SOAK 必须能够读取 start RSS");
        let e = end_rss.expect("PERF_SOAK 必须能够读取 end RSS");
        let growth = if s > 0 {
            (e as f64 - s as f64) / s as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "rss soak growth {growth:.1}% start {s} end {e} peak {:?} samples {} warmup 300s steady baseline",
            rss_peak, rss_sample_count
        );
        // P1-3：失败诊断文件，即使 Gate FAIL 也保留完整 61 samples（非 Release Evidence，strict 不读）
        {
            let out_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/validation");
            let _ = std::fs::create_dir_all(&out_dir);
            let git_sha = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "unknown".to_string());
            let rss_samples_mib: Vec<f64> = rss_samples
                .iter()
                .map(|v| *v as f64 / (1024.0 * 1024.0))
                .collect();
            let diag = serde_json::json!({
                "passed": growth <= 10.0,
                "failure": if growth <= 10.0 { serde_json::Value::Null } else { serde_json::Value::String("RSS_GROWTH".into()) },
                "rss_probe": if cfg!(target_os = "linux") { "proc_status" } else { "sysinfo_current_pid_reused" },
                "git_sha": git_sha,
                "warmup_seconds": warmup.as_secs(),
                "rss_start_mib": s as f64 / (1024.0*1024.0),
                "rss_end_mib": e as f64 / (1024.0*1024.0),
                "rss_peak_mib": rss_peak.map(|v| v as f64 / (1024.0*1024.0)),
                "rss_sample_count": rss_sample_count,
                "rss_samples_mib": rss_samples_mib,
                "rss_growth_percent": growth,
                "elapsed_seconds": elapsed,
            });
            let _ = std::fs::write(
                out_dir.join("soak-diagnostic.json"),
                serde_json::to_string_pretty(&diag).unwrap(),
            );
        }
        assert!(
            growth <= 10.0,
            "RSS 增长 {growth:.1}% >10% (start {s} end {e} peak {:?} samples {})，疑似泄漏或 warm-up 不足",
            rss_peak, rss_sample_count
        );
    }
    // 最终仍需 RUNNING（必须在写 Evidence 之前通过，否则不留成功证据）
    let st = snap.endpoint(ep_id).expect("endpoint still present");
    assert_eq!(st.state, "RUNNING", "结束时仍应 RUNNING，实际 {:?}", st);

    mgr.shutdown_all().await;

    // 全部断言通过后最后一步才写 Evidence，保证文件存在=跑到成功终点
    // 注意：当前 RSS 仅度量 Core/Test 进程自身（/proc/self/status 或 sysinfo 当前进程），
    // 不含独立 Driver 子进程全量，文档中应表述为 Core/Test process RSS growth。
    {
        let out_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/validation");
        let _ = std::fs::create_dir_all(&out_dir);
        let git_sha = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_string());
        let git_sha_short = git_sha.chars().take(7).collect::<String>();
        let dirty = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .ok()
            .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
            .unwrap_or(false);
        let build_profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let mode = if soak {
            "soak"
        } else if long {
            "long"
        } else {
            "quick"
        };
        let rss_samples_mib: Vec<f64> = rss_samples
            .iter()
            .map(|v| *v as f64 / (1024.0 * 1024.0))
            .collect();
        let perf_json = serde_json::json!({
            "throughput_updates_per_sec": ups.round() as u64,
            "ipc_p95_ms": (ipc_p95 as f64 / 1_000_000.0).round() as u64,
            "ipc_p99_ms": (ipc_p99 as f64 / 1_000_000.0).round() as u64,
            "configure_1k_ms": 0, "configure_10k_ms": 0, "configure_50k_ms": 0,
            "rss_delta_mib": end_rss.zip(start_rss).map(|(e,s)| ((e as i64 - s as i64) / (1024*1024)) as i32).unwrap_or(0),
            "git_sha": git_sha,
            "git_sha_short": git_sha_short,
            "generated_at_ns": mesa_core_types::now_unix_ns(),
            "mode": mode,
            "duration_seconds": elapsed,
            "build_profile": build_profile,
            "dirty": dirty,
            "rss_scope": "core_test_process",
            "rss_start_mib": start_rss.map(|v| v as f64 / (1024.0*1024.0)),
            "rss_end_mib": end_rss.map(|v| v as f64 / (1024.0*1024.0)),
            "rss_peak_mib": rss_peak.map(|v| v as f64 / (1024.0*1024.0)),
            "rss_sample_count": rss_sample_count,
            "rss_samples_mib": rss_samples_mib,
            "warmup_seconds": warmup.as_secs()
        });
        let _ = std::fs::write(
            out_dir.join("performance.json"),
            serde_json::to_string_pretty(&perf_json).unwrap(),
        );
        if soak {
            let growth = if let (Some(s), Some(e)) = (start_rss, end_rss) {
                if s > 0 {
                    (e as f64 - s as f64) / s as f64 * 100.0
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let soak_json = serde_json::json!({
                "duration_hours": elapsed / 3600.0,
                "rss_growth_percent": growth,
                "leak_detected": false,
                "git_sha": git_sha,
                "git_sha_short": git_sha_short,
                "generated_at_ns": mesa_core_types::now_unix_ns(),
                "mode": "soak",
                "duration_seconds": elapsed,
                "build_profile": build_profile,
                "dirty": dirty,
                "rss_scope": "core_test_process",
                "rss_start_mib": start_rss.map(|v| v as f64 / (1024.0*1024.0)),
                "rss_end_mib": end_rss.map(|v| v as f64 / (1024.0*1024.0)),
                "rss_peak_mib": rss_peak.map(|v| v as f64 / (1024.0*1024.0)),
                "rss_sample_count": rss_sample_count,
                "rss_samples_mib": rss_samples_mib.clone(),
                "warmup_seconds": warmup.as_secs()
            });
            let _ = std::fs::write(
                out_dir.join("soak.json"),
                serde_json::to_string_pretty(&soak_json).unwrap(),
            );
        }
    }
}
