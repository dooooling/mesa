//! 孤儿防护专测（§1.6.1 / §32 P0-D）：
//! - wrong token → handshake rejected
//! - stdin EOF → child exits before TERMINATE_GRACE
//! - helper parent 死亡 → Driver 随 Job/PDEATHSIG 被清理（Windows KILL_ON_JOB_CLOSE / Linux PR_SET_PDEATHSIG）
//!
//! 测试策略：外层测试进程 spawn helper Core，helper spawn Driver，外层杀 helper 并观察 Driver PID 消失。

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mesa_driver_manager::manifest::DiscoveredDriver;
use mesa_driver_manager::process::TERMINATE_GRACE;
use mesa_driver_manager::session::Session;

use common::{assert_handshake_error, repo_root, sim_exe};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::probe::ProbeError;

fn sim_discovered() -> DiscoveredDriver {
    let exe = sim_exe();
    DiscoveredDriver {
        manifest: mesa_driver_manager::manifest::DriverManifest {
            id: "simulator".into(),
            name: "Mesa Simulator".into(),
            version: "0.0.0".into(),
            executable: exe.file_name().unwrap().to_string_lossy().to_string(),
            protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
            protocol_minor: mesa_driver_protocol::PROTOCOL_MINOR,
            sdk: None,
            os: None,
            arch: None,
        },
        manifest_dir: PathBuf::new(),
        executable_path: Some(exe),
        platform_ok: true,
        platform_reason: None,
        protocol_ok: true,
    }
}

/// wrong token → handshake rejected
#[tokio::test]
async fn orphan_guard_wrong_token_rejected() {
    let mut p = mesa_driver_manager::process::DriverProcess::spawn(&sim_discovered())
        .await
        .expect("spawn");
    let res = Session::connect_retry(p.port, "definitely-wrong").await;
    match res {
        Ok(_) => panic!("wrong token must be rejected"),
        Err(e) => assert_handshake_error(e),
    }
    p.terminate().await;
}

/// stdin EOF → child exits quickly（第一层防护）
#[tokio::test]
async fn orphan_guard_stdin_eof_exits_quickly() {
    let mut p = mesa_driver_manager::process::DriverProcess::spawn(&sim_discovered())
        .await
        .expect("spawn");
    let pid = p.pid;
    let start = Instant::now();
    p.terminate().await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < TERMINATE_GRACE - Duration::from_secs(1),
        "child {pid} must exit via stdin EOF well before grace, took {elapsed:?}"
    );
}

/// helper 父进程死亡 → Driver 被 Job/PDEATHSIG 清理
#[tokio::test]
async fn orphan_guard_helper_death_cleans_driver() {
    // cargo build 需先产出 orphan_helper 二进制
    let helper_exe = repo_root()
        .join("target")
        .join("debug")
        .join(if cfg!(windows) {
            "orphan_helper.exe"
        } else {
            "orphan_helper"
        });
    if !helper_exe.is_file() {
        eprintln!(
            "orphan_helper not built, skipping helper death test (run cargo build -p mesa-contract-tests)"
        );
        return;
    }

    let mut cmd = tokio::process::Command::new(&helper_exe);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    let mut helper = cmd.spawn().expect("spawn helper");

    // 读取 helper 输出的 DRIVER_PID
    use tokio::io::{AsyncBufReadExt, BufReader};
    let stdout = helper.stdout.take().expect("helper stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut driver_pid: Option<u32> = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        line.clear();
        let n = reader.read_line(&mut line).await.unwrap_or(0);
        if n == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        if let Some(pid_str) = line.strip_prefix("DRIVER_PID=") {
            driver_pid = pid_str.trim().parse().ok();
            break;
        }
    }
    let driver_pid = driver_pid.expect("helper must print DRIVER_PID");

    // 给 Driver 一点时间进入 Job
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 杀死 helper（模拟 Core 异常死亡）
    helper.kill().await.ok();
    let _ = helper.wait().await;

    // 观察 Driver 是否在有界时间内消失
    let start = Instant::now();
    let timeout = Duration::from_secs(6);
    loop {
        if !is_pid_alive(driver_pid) {
            break;
        }
        if start.elapsed() > timeout {
            panic!("driver pid {driver_pid} must disappear after helper death within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(start.elapsed() < timeout, "orphan driver not cleaned");
}

#[cfg(windows)]
fn is_pid_alive(pid: u32) -> bool {
    // OpenProcess 查询，失败或退出即视为死亡
    unsafe {
        let handle = windows_sys::Win32::System::Threading::OpenProcess(0x0400, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
        if handle.is_null() || handle == 1 as _ {
            return false;
        }
        let mut code: u32 = 0;
        let ok = windows_sys::Win32::System::Threading::GetExitCodeProcess(handle, &mut code);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        if ok == 0 {
            return false;
        }
        code == 259 // STILL_ACTIVE
    }
}

#[cfg(not(windows))]
fn is_pid_alive(pid: u32) -> bool {
    // kill(pid,0) 探测
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

// ---- Dynamic Probe 临时进程清理（§8，P0-3 由 probe_contract 并入）----
// 孤儿判定：唯一命名二进制 + 测试前后 PID 快照，15s 轮询回落基线。

fn now_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// 按可执行文件名快照存活 PID（标准库实现，无新依赖）。
/// 注意：Linux comm 截断到 15 字符，调用方传入的唯一名必须 ≤15 字符，
/// 否则匹配永远为空、孤儿检查被静默旁路（"pb-sim-guard"=12，"pb-hang-guard"=13）。
fn live_pids(name: &str) -> HashSet<u32> {
    // Windows 按 IMAGENAME（含 .exe）匹配无截断；Linux comm 去扩展名后必须 ≤15
    debug_assert!(
        name.trim_end_matches(".exe").len() <= 15,
        "comm truncation risk: {name}"
    );
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {name}")])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        out.lines()
            .filter_map(|l| {
                let cols: Vec<&str> = l.split("\",\"").collect();
                if cols.len() >= 2 && cols[0].trim_matches('"').eq_ignore_ascii_case(name) {
                    cols[1].trim_matches('"').parse::<u32>().ok()
                } else {
                    None
                }
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        let mut set = HashSet::new();
        let Ok(rd) = std::fs::read_dir("/proc") else {
            return set;
        };
        for e in rd.flatten() {
            let fname = e.file_name();
            let Some(pid) = fname.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if let Ok(comm) = std::fs::read_to_string(e.path().join("comm"))
                && comm.trim() == name
            {
                set.insert(pid);
            }
        }
        set
    }
}

/// 断言 `name` 的存活 PID 最终回落到 `before` 子集（无孤儿）。
fn assert_no_orphan(before: &HashSet<u32>, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let cur = live_pids(name);
        if cur.is_subset(before) {
            return;
        }
        if Instant::now() >= deadline {
            let leaked: Vec<u32> = cur.difference(before).copied().collect();
            panic!("probe 残留子进程 {name} pids={leaked:?}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 搭临时 drivers 目录：二进制按唯一名拷贝（防并行串扰）+ profiles 拷贝（hints 验证）。
/// 返回 (root, exe_unique_name)。
fn stage_drivers_dir(tag: &str, src_exe: &Path, unique_base: &str) -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!("fl-probe-{tag}-{}", now_ns()));
    let dir = root.join("simulator");
    std::fs::create_dir_all(&dir).unwrap();
    let name = exe_name(unique_base);
    std::fs::copy(src_exe, dir.join(&name)).unwrap();
    std::fs::write(
        dir.join("driver.toml"),
        format!(
            "id=\"simulator\"\nname=\"Mesa Simulator\"\nversion=\"0.1.0\"\nexecutable=\"{unique_base}\"\nprotocol_major={}\nprotocol_minor=2\n",
            mesa_driver_protocol::PROTOCOL_MAJOR
        ),
    )
    .unwrap();
    // profiles 拷贝：hints 端到端验证用
    let prof_src = repo_root()
        .join("drivers")
        .join("simulator")
        .join("profiles");
    let prof_dst = dir.join("profiles");
    std::fs::create_dir_all(&prof_dst).unwrap();
    for e in std::fs::read_dir(&prof_src).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) == Some("json") {
            std::fs::copy(&p, prof_dst.join(p.file_name().unwrap())).unwrap();
        }
    }
    (root, unique_base.to_string())
}

fn hang_exe() -> PathBuf {
    let target = repo_root().join("target");
    for profile in ["debug", "release"] {
        let p = target.join(profile).join(exe_name("hang_helper"));
        if p.is_file() {
            return p;
        }
    }
    panic!("hang_helper binary not built; run cargo build --workspace first");
}

fn cleanup_dir(root: &Path) {
    std::fs::remove_dir_all(root).ok();
}

// ---- 编排级（真子进程）----

#[tokio::test]
async fn manager_probe_success_reports_hints_and_cleans_child() {
    let (root, unique) = stage_drivers_dir("ok", &sim_exe(), "pb-sim-guard");
    let before = live_pids(&exe_name(&unique));
    let mgr = MesaManager::discover(&root);
    let res = mgr.probe("simulator", "{}").await.expect("probe ok");
    assert!(res.report.reachable);
    assert_eq!(res.report.vendor.as_deref(), Some("Mesa"));
    assert!(
        res.profile_hints
            .iter()
            .any(|h| h.profile_id == "simulator-basic"),
        "hints 必须含 simulator-basic，实际: {:?}",
        res.profile_hints
    );
    assert_no_orphan(&before, &exe_name(&unique));
    cleanup_dir(&root);
}

#[tokio::test]
async fn manager_probe_bad_config_fails_and_cleans_child() {
    let (root, unique) = stage_drivers_dir("bad", &sim_exe(), "pb-sim-guard");
    let before = live_pids(&exe_name(&unique));
    let mgr = MesaManager::discover(&root);
    let err = mgr
        .probe("simulator", "not-json")
        .await
        .expect_err("非法 JSON 必须 Err");
    assert!(
        matches!(err, ProbeError::InvalidInput { .. }),
        "simulator BAD_CONFIG 应结构化映射 InvalidInput，实际: {err}"
    );
    assert_no_orphan(&before, &exe_name(&unique));
    cleanup_dir(&root);
}

#[tokio::test]
async fn manager_probe_timeout_cleans_hang_child() {
    // hang 桩：握手成功但永不回包 → 内层 RPC 10s 超时 → 同一清理尾回收子进程。
    let (root, unique) = stage_drivers_dir("hang", &hang_exe(), "pb-hang-guard");
    // hang 桩的 manifest id 需改写为 hang（stage 函数写死 simulator，覆写 toml）
    let dir = root.join("simulator");
    std::fs::write(
        dir.join("driver.toml"),
        format!(
            "id=\"hang\"\nname=\"Hang\"\nversion=\"0.1.0\"\nexecutable=\"{unique}\"\nprotocol_major={}\nprotocol_minor=2\n",
            mesa_driver_protocol::PROTOCOL_MAJOR
        ),
    )
    .unwrap();
    let before = live_pids(&exe_name(&unique));
    let mgr = MesaManager::discover(&root);
    let err = mgr.probe("hang", "{}").await.expect_err("挂起必须超时 Err");
    assert!(
        matches!(err, ProbeError::RpcTimeout),
        "内层 10s RPC 超时应映射 RpcTimeout，实际: {err}"
    );
    assert_no_orphan(&before, &exe_name(&unique));
    cleanup_dir(&root);
}
