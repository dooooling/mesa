//! 孤儿防护专测（§1.6.1 / §32 P0-D）：
//! - wrong token → handshake rejected
//! - stdin EOF → child exits before TERMINATE_GRACE
//! - helper parent 死亡 → Driver 随 Job/PDEATHSIG 被清理（Windows KILL_ON_JOB_CLOSE / Linux PR_SET_PDEATHSIG）
//!
//! 测试策略：外层测试进程 spawn helper Core，helper spawn Driver，外层杀 helper 并观察 Driver PID 消失。

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mesa_driver_manager::manifest::DiscoveredDriver;
use mesa_driver_manager::process::TERMINATE_GRACE;
use mesa_driver_manager::session::Session;

use common::{assert_handshake_error, repo_root, sim_exe};

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
    let res = Session::connect(p.port, "definitely-wrong").await;
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
