//! 孤儿防护专测（§1.6.1 / §32 P0-D）：
//! - wrong token → handshake rejected
//! - stdin EOF → child exits before TERMINATE_GRACE
//! - helper parent 死亡 → Driver 随 Job/PDEATHSIG 被清理（Windows KILL_ON_JOB_CLOSE / Linux PR_SET_PDEATHSIG）
//!
//! 测试策略：外层测试进程 spawn helper Core，helper spawn Driver，外层杀 helper 并观察 Driver PID 消失。

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mesa_driver_manager::manifest::DiscoveredDriver;
use mesa_driver_manager::process::TERMINATE_GRACE;
use mesa_driver_manager::session::Session;

use common::{assert_handshake_error, repo_root, test_driver_exe};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::probe::ProbeError;

fn sim_discovered() -> DiscoveredDriver {
    let exe = test_driver_exe();
    DiscoveredDriver {
        manifest: mesa_driver_manager::manifest::DriverManifest {
            id: "test-driver".into(),
            name: "Mesa Test Driver".into(),
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

/// P1-A 并发 spawn：8 个 simulator 并发启动 → 端口/token 全不相同 →
/// 每个 handshake 成功 → 全部 terminate 后无残留。
/// 无 PortLease 时裸 free_port 高概率撞号（后 bind 失败，或连错进程
/// 触发 token mismatch → 偶发 503）；本测试失败 = 租约被旁路或破坏。
#[tokio::test]
async fn concurrent_spawn_gets_unique_ports_tokens_and_no_orphan() {
    const N: usize = 8;
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..N {
        set.spawn(async move {
            let mut p = mesa_driver_manager::process::DriverProcess::spawn(&sim_discovered())
                .await
                .expect("concurrent spawn");
            let (port, pid, token) = (p.port, p.pid, p.token.clone());
            // handshake 必须成功：连错进程会被 token mismatch fail-closed 拒绝
            let (mut session, _events, _) = Session::connect_retry(port, &token)
                .await
                .expect("concurrent handshake");
            session.invalidate();
            p.terminate().await;
            (port, pid, token)
        });
    }
    let mut ports = HashSet::new();
    let mut tokens = HashSet::new();
    let mut pids = Vec::new();
    while let Some(r) = set.join_next().await {
        let (port, pid, token) = r.expect("spawn task panicked");
        ports.insert(port);
        tokens.insert(token);
        pids.push(pid);
    }
    assert_eq!(ports.len(), N, "并发 spawn 端口必须全不相同");
    assert_eq!(tokens.len(), N, "session token 必须全不相同");
    // terminate 内部已 wait；有界轮询确认内核侧全部消失（防 zombie 误判）
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        pids.retain(|pid| is_pid_alive(*pid));
        if pids.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            panic!("并发 spawn 残留子进程 pids={pids:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
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
    let helper_exe = mesa_contract_tests::target_dir()
        .join("debug")
        .join(if cfg!(windows) {
            "orphan_helper.exe"
        } else {
            "orphan_helper"
        });
    assert!(
        helper_exe.is_file(),
        "orphan_helper 未构建；先 cargo build -p mesa-contract-tests --bins，不得跳过孤儿防护验证"
    );

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
///
/// **必须是异步轮询**：调用方是 `#[tokio::test]`（默认单线程运行时）。
/// 若在此用 `std::thread::sleep` 阻塞运行时线程，Tokio 在 `kill_on_drop` 之后
/// 的**异步子进程回收**就没有机会被调度——Linux 上表现为「进程已退出但仍是
/// 僵尸、PID 迟迟不消失」，而 Windows 侧不依赖该回收路径，于是同一用例
/// 只在 Linux 失败。改为 `tokio::time::sleep` 让回收任务得以推进。
async fn assert_no_orphan(before: &HashSet<u32>, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let cur = live_pids(name);
        if cur.is_subset(before) {
            return;
        }
        if Instant::now() >= deadline {
            let mut leaked: Vec<u32> = cur.difference(before).copied().collect();
            leaked.sort_unstable();
            dump_process_state(&leaked);
            panic!("probe 残留子进程 {name} pids={leaked:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 失败时的进程状态诊断：打印残留 PID 的 `/proc/<pid>/status` 关键行，
/// 用于区分「真存活（S/R）」「僵尸未回收（Z）」「父进程/命名空间异常（PPid/NSpid）」。
/// 非 Linux 平台为空实现（Windows 用 tasklist，无同等状态字段）。
fn dump_process_state(pids: &[u32]) {
    #[cfg(target_os = "linux")]
    for pid in pids {
        let path = format!("/proc/{pid}/status");
        match std::fs::read_to_string(&path) {
            Ok(status) => {
                for line in status.lines() {
                    if ["Name:", "State:", "PPid:", "NSpid:"]
                        .iter()
                        .any(|prefix| line.starts_with(prefix))
                    {
                        eprintln!("[orphan-diag] PID {pid}: {line}");
                    }
                }
            }
            Err(e) => eprintln!("[orphan-diag] PID {pid}: 读取 {path} 失败: {e}"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pids;
    }
}

/// 搭临时 drivers 目录：二进制按唯一名拷贝（防并行串扰）。
/// 返回 (root, exe_unique_name)。
///
/// CI ARM ETXTBSY（`Text file busy`，os error 26）修法：直接 `copy` 到最终
/// 可执行路径，会让"写者尚未完全关闭"与紧随其后的 `exec` 在慢文件系统上
/// 竞争。这里走 copy → 临时文件 → fsync/close → 显式可执行位 → 原子 rename
/// → 最终路径：rename 后目标从未以写模式打开过，exec 不可能撞上 ETXTBSY。
fn stage_drivers_dir(tag: &str, src_exe: &Path, unique_base: &str) -> (PathBuf, String) {
    use std::io::Write;
    let root = std::env::temp_dir().join(format!("fl-probe-{tag}-{}", now_ns()));
    let dir = root.join("test-driver");
    std::fs::create_dir_all(&dir).unwrap();
    let name = exe_name(unique_base);
    let final_path = dir.join(&name);
    let tmp_path = dir.join(format!("{name}.staging"));
    {
        let mut src = std::fs::File::open(src_exe).unwrap();
        let mut tmp = std::fs::File::create(&tmp_path).unwrap();
        std::io::copy(&mut src, &mut tmp).unwrap();
        tmp.flush().unwrap();
        tmp.sync_all().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    } // src + tmp 全部关闭：此后 tmp 无任何写者
    std::fs::rename(&tmp_path, &final_path).unwrap();
    // 目录项落盘（best-effort）：rename 后 crash 也不丢目标
    if let Ok(d) = std::fs::File::open(&dir) {
        let _ = d.sync_all();
    }
    // 单真值：canonical manifest 仅替换 executable 为本次唯一名
    //（version/protocol 与仓库 canonical 一致，不手写）。
    let canonical =
        std::fs::read_to_string(repo_root().join("tests/support/test-driver/driver.toml")).unwrap();
    let patched = canonical
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("executable") {
                format!("executable = \"{unique_base}\"")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(dir.join("driver.toml"), patched).unwrap();
    (root, unique_base.to_string())
}

fn hang_exe() -> PathBuf {
    let target = mesa_contract_tests::target_dir();
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
async fn manager_probe_success_reports_facts_and_cleans_child() {
    let (root, unique) = stage_drivers_dir("ok", &test_driver_exe(), "pb-sim-guard");
    let before = live_pids(&exe_name(&unique));
    let mgr = MesaManager::discover(&root);
    let res = mgr.probe("test-driver", "{}").await.expect("probe ok");
    assert!(res.reachable);
    assert_eq!(res.vendor.as_deref(), Some("Mesa"));
    assert_no_orphan(&before, &exe_name(&unique)).await;
    cleanup_dir(&root);
}

#[tokio::test]
async fn manager_probe_bad_config_fails_and_cleans_child() {
    let (root, unique) = stage_drivers_dir("bad", &test_driver_exe(), "pb-sim-guard");
    let before = live_pids(&exe_name(&unique));
    let mgr = MesaManager::discover(&root);
    let err = mgr
        .probe("test-driver", "not-json")
        .await
        .expect_err("非法 JSON 必须 Err");
    assert!(
        matches!(err, ProbeError::InvalidInput { .. }),
        "simulator BAD_CONFIG 应结构化映射 InvalidInput，实际: {err}"
    );
    assert_no_orphan(&before, &exe_name(&unique)).await;
    cleanup_dir(&root);
}

#[tokio::test]
async fn manager_probe_timeout_cleans_hang_child() {
    // hang 桩：握手成功但永不回包 → 内层 RPC 10s 超时 → 同一清理尾回收子进程。
    let (root, unique) = stage_drivers_dir("hang", &hang_exe(), "pb-hang-guard");
    // hang 桩的 manifest id 需改写为 hang（stage 函数写死 simulator，覆写 toml）
    let dir = root.join("test-driver");
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
    // 显式给一个能推出较短 Query 预算的 timeout_ms（500 → Query=5s），
    // 让「挂起 → RPC 超时」的判定保持在秒级而不是等满默认预算。
    let err = mgr
        .probe("hang", r#"{"timeout_ms":500}"#)
        .await
        .expect_err("挂起必须超时 Err");
    assert!(
        matches!(err, ProbeError::RpcTimeout),
        "Probe RPC 超时应映射 RpcTimeout，实际: {err}"
    );
    assert_no_orphan(&before, &exe_name(&unique)).await;
    cleanup_dir(&root);
}

// ---------------------------------------------------------------------------
// C1 §6.3：Probe 阶段预算（A22）与取消清理（A32）
// ---------------------------------------------------------------------------

/// 跨进程端口声明目录（与 `driver_manager::process::port_claim_dir` 同源）。
///
/// 该路径是驱动的端口租约唯一可外部观测的载体：spawn 时 `create_new` 建
/// `port-<n>.lock`，`PortLease::drop` 时删除。因此「租约是否释放」可以在
/// 测试进程里直接断言，不需要访问 manager 私有状态。
fn claim_dir() -> PathBuf {
    std::env::temp_dir().join("mesa-driver-ports")
}

/// 当前测试进程认领的端口集合；全局目录中的其它 Core 不属于本次探测。
fn claimed_ports() -> HashSet<u16> {
    claimed_ports_in(&claim_dir(), std::process::id())
}

fn claimed_ports_in(dir: &Path, owner: u32) -> HashSet<u16> {
    let mut set = HashSet::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return set;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(rest) = name.strip_prefix("port-")
            && let Some(num) = rest.strip_suffix(".lock")
            && let Ok(p) = num.parse::<u16>()
            && std::fs::read_to_string(e.path())
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                == Some(owner)
        {
            set.insert(p);
        }
    }
    set
}

/// 外部 Core 的新租约和旧版空文件都不能被认作本次探测的端口。
#[test]
fn lease_claim_observation_excludes_foreign_and_incomplete_owners() {
    let dir = std::env::temp_dir().join(format!(
        "mesa-claim-observation-{}-{}",
        std::process::id(),
        now_ns()
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("port-30001.lock"), "101\n").unwrap();
    std::fs::write(dir.join("port-30002.lock"), "202\n").unwrap();
    std::fs::write(dir.join("port-30003.lock"), "").unwrap();
    std::fs::write(dir.join("port-30004.lock"), "invalid").unwrap();
    assert_eq!(claimed_ports_in(&dir, 101), HashSet::from([30001]));
    assert_eq!(claimed_ports_in(&dir, 202), HashSet::from([30002]));
    cleanup_dir(&dir);
}

/// 轮询等待「出现一个不在 `before` 中的存活 PID」。
async fn wait_for_new_pid(before: &HashSet<u32>, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if live_pids(name).difference(before).next().is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "等待子进程 {name} 启动超时");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 轮询等待「出现一个不在 `before` 中的端口租约」，返回该端口。
async fn wait_for_new_claim(before: &HashSet<u16>) -> u16 {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(p) = claimed_ports().difference(before).next().copied() {
            return p;
        }
        assert!(Instant::now() < deadline, "等待端口租约出现超时");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A22：**慢但成功**的探测不得被管理面的固定预算提前掐断。
///
/// 用例刻意让 `probe()` 内部耗时 11s（> 旧的通用 `REQUEST_TIMEOUT` 10s 与
/// 旧外层 12s），而 `timeout_ms=5000` 推出的 Query 预算为
/// `4×5000+3000 = 23s`。旧实现必然在此失败；新实现必须真的等满延迟并返回可达。
///
/// NOTE: 这是**可控模拟**验证，不代替真机分阶段量测（A22 的实体设备部分
/// 仍需在授权设备上单独执行并另行记录）。
#[tokio::test]
async fn slow_but_successful_probe_is_not_truncated_by_management_budget() {
    let (root, unique) = stage_drivers_dir("slow", &test_driver_exe(), "pb-slow-guard");
    let name = exe_name(&unique);
    let before = live_pids(&name);
    let mgr = MesaManager::discover(&root);

    let started = Instant::now();
    let report = mgr
        .probe(
            "test-driver",
            r#"{"probe_delay_ms":11000,"timeout_ms":5000}"#,
        )
        .await
        .expect("慢但合法的探测不得被 IPC/外层预算截断");
    let elapsed = started.elapsed();

    assert!(report.reachable, "延迟后仍应判定可达");
    assert!(
        elapsed >= Duration::from_millis(11_000),
        "必须真的等满驱动侧延迟（证明没有被 10s IPC / 12s 外层截断），实际 {elapsed:?}"
    );
    assert_no_orphan(&before, &name).await;
    cleanup_dir(&root);
}

/// A32：Probe future 被取消（等价 HTTP 客户端断开 / 服务端请求取消）后，
/// **临时驱动子进程必须退出、端口租约必须释放**，随后再次 Probe 不受影响。
///
/// 与 `manager_probe_timeout_cleans_hang_child` 的区别：那条走的是「RPC 自己
/// 超时后正常回收」，本条走的是「调用方直接 drop 掉整个 probe future」——
/// 清理代码没有机会执行到尾部，只能依赖 `TempDriver`/`DriverProcess` 的 Drop
/// 语义（`kill_on_drop` + `PortLease::drop`）。这正是 HTTP 层取消的真实形态。
#[tokio::test]
async fn cancelled_probe_releases_child_process_and_port_lease() {
    let (root, unique) = stage_drivers_dir("cancel", &test_driver_exe(), "pb-cancel-guard");
    let name = exe_name(&unique);
    let before = live_pids(&name);
    let claims_before = claimed_ports();
    let mgr = Arc::new(MesaManager::discover(&root));

    // 让驱动在 probe() 内停留足够久，以便我们在 RPC 进行中取消。
    let probing = {
        let m = Arc::clone(&mgr);
        tokio::spawn(async move {
            m.probe(
                "test-driver",
                r#"{"probe_delay_ms":30000,"timeout_ms":5000}"#,
            )
            .await
        })
    };
    // 先确认临时子进程确实起来了，并抓到它认领的端口。
    wait_for_new_pid(&before, &name).await;
    let port = wait_for_new_claim(&claims_before).await;

    // 诊断留痕（A32 排查）：记录取消前后的子进程 PID 与端口租约状态，
    // 便于在 CI 日志里直接看出「是进程没死」还是「进程死了但租约没放」。
    let pids_before_abort: Vec<u32> = {
        let mut v: Vec<u32> = live_pids(&name).difference(&before).copied().collect();
        v.sort_unstable();
        v
    };
    eprintln!(
        "[a32-diag] before abort: child_pids={pids_before_abort:?} port={port} \
         lock_exists={}",
        claim_dir().join(format!("port-{port}.lock")).exists()
    );

    // 取消：drop 整个 probe future。
    probing.abort();
    let _ = probing.await;

    eprintln!(
        "[a32-diag] after abort (immediate): child_pids={:?} lock_exists={}",
        {
            let mut v: Vec<u32> = live_pids(&name).difference(&before).copied().collect();
            v.sort_unstable();
            v
        },
        claim_dir().join(format!("port-{port}.lock")).exists()
    );

    // 1) 子进程退出：唯一命名二进制的存活 PID 回落到基线。
    // 失败时会打印残留 PID 的 /proc/<pid>/status（Linux），用于区分
    // 真存活 / 僵尸未回收 / 父进程异常。
    assert_no_orphan(&before, &name).await;
    // 2) 端口租约释放：声明文件被删除（否则该端口对本进程永久不可用）。
    common::wait_until(15, || {
        !claim_dir().join(format!("port-{port}.lock")).exists()
    })
    .await;
    // 3) 该端口不再被监听 → 子进程的监听套接字确实已关闭，而不是仅对象被 drop。
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "取消后临时驱动必须已退出：端口 {port} 不应仍被监听"
    );
    // 4) 随后再次 Probe 不受影响（无端口冲突、无残留会话）。
    let report = mgr
        .probe("test-driver", "{}")
        .await
        .expect("取消后的新探测必须成功");
    assert!(report.reachable, "新探测必须可达");
    assert_no_orphan(&before, &name).await;
    cleanup_dir(&root);
}

// ---------------------------------------------------------------------------
// C3-08：Endpoint-bound 连接检查（mode=probe）的临时进程清理与旁路影响
// ---------------------------------------------------------------------------

/// C3 探测请求（与既有用例同形，但入口是 Endpoint-bound 的 connection-check）。
async fn post_connection_check(
    app: axum::Router,
    uri: &str,
    body: &str,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::body::Body;
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// 持久化面快照（`connection_json` / `endpoint_secrets` / `desired_running` /
/// 任务面 / 事件任务面 / point_map）。逐字比对而不是逐字段断言，避免「看漏了
/// 某一列改了没改」。
fn c3_persisted_snapshot(store: &mesa_config_store::ConfigStore) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for id in ["e-run", "e-probe"] {
        let rec = store.get_endpoint(id).unwrap().unwrap();
        out.insert(
            id.to_string(),
            serde_json::json!({
                "endpoint": rec,
                "secret_fields": store.list_secret_fields(id).unwrap(),
                "password": store.get_secret(id, "password").unwrap(),
                "tasks": store.list_tasks(id).unwrap(),
                "event_tasks": store.list_event_tasks(id).unwrap(),
                "point_map": store.point_map(id).unwrap(),
            }),
        );
    }
    serde_json::Value::Object(out)
}

/// 该 Endpoint 的采集当前值（用于证明「运行实例仍在出数」）。
fn latest_value(mgr: &MesaManager, point_key: &str) -> Option<f64> {
    mgr.snapshot()
        .latest_all()
        .iter()
        .find(|e| e.point_key == point_key)
        .and_then(|e| e.value.value.as_f64())
}

/// C3-08：`mode=probe` 完成后**临时探测进程必须回收**（PID 回落 + 端口租约释放 +
/// 无孤儿），且**不影响既有运行实例**（不被重启、不断流、持久化面零改动）。
///
/// 判据为什么是「唯一命名的二进制 + 精确 PID 集合」：本用例把测试驱动 stage 成
/// `pb-c3-guard`，因此按进程名看到的 PID 只可能来自本用例——不存在把别的用例
/// 的 driver 误算成自己孤儿的可能。
#[tokio::test]
async fn c3_connection_check_probe_cleans_up_and_spares_running_endpoint() {
    use mesa_config_store::{ConfigStore, DeviceRecord, EndpointRecord};
    use mesa_core_types::{AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, TaskSchedule};

    const PW: &str = "pw-c3-orphan-guard";
    let (root, unique) = stage_drivers_dir("c3check", &test_driver_exe(), "pb-c3-guard");
    let name = exe_name(&unique);
    let store = Arc::new(ConfigStore::open_in_memory().unwrap());
    store
        .create_device(&DeviceRecord {
            id: "d1".into(),
            name: "D".into(),
        })
        .unwrap();
    // e-run：真实运行中的采集实例（counter，50ms 一跳）。
    store
        .create_endpoint(&EndpointRecord {
            id: "e-run".into(),
            name: "e-run".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: true,
            updated_at_ns: 0,
        })
        .unwrap();
    store
        .replace_tasks(
            "e-run",
            &[AcquisitionTask {
                id: "t1".into(),
                schedule: TaskSchedule::Poll { interval_ms: 50 },
                binding: DriverBinding {
                    kind: GENERIC_BINDING_KIND.into(),
                    config: serde_json::json!({"selections":[{
                        "resource_id":"counter",
                        "parameters":{"start":0.0,"step":1.0},
                        "outputs":[{"output":"value","point_key":"c3.counter"}]
                    }]}),
                },
            }],
        )
        .unwrap();
    // e-probe：只有被检查的连接草稿（凭据在 SecretStore，落盘是 marker）。
    store
        .create_endpoint(&EndpointRecord {
            id: "e-probe".into(),
            name: "e-probe".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: r#"{"password":{"secret_set":true}}"#.into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    store
        .put_secret("e-probe", "password", PW, "master")
        .unwrap();

    let mgr = Arc::new(MesaManager::discover(&root));
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "e-run".into(),
        driver_id: "test-driver".into(),
        connection_json: "{}".into(),
        tasks: store.list_tasks("e-run").unwrap(),
        event_tasks: vec![],
    })
    .unwrap();
    common::wait_until(15, || {
        mgr.snapshot()
            .endpoint("e-run")
            .is_some_and(|s| s.state == "RUNNING")
    })
    .await;
    common::wait_until(15, || latest_value(&mgr, "c3.counter").is_some()).await;

    // 运行实例的 PID 集合：探测前建立基线，探测后必须**原样存活**。
    let run_pids: HashSet<u32> = {
        let p = live_pids(&name);
        assert!(
            !p.is_empty(),
            "预置：运行中的 Endpoint 必须有一个 driver 进程"
        );
        p
    };
    let claims_before = claimed_ports();
    let value_before = latest_value(&mgr, "c3.counter").expect("预置：已开始出数");
    let persisted_before = c3_persisted_snapshot(&store);

    let state = common::isolated_app_state(
        Arc::clone(&mgr),
        Arc::clone(&store),
        root.to_string_lossy().to_string(),
        false,
    )
    .unwrap();
    let app = state.router();
    let uri = "/api/v1/endpoints/e-probe/connection-check";
    let tmp = std::env::temp_dir().join(format!("mesa-c3-orphan-{}", now_ns()));
    std::fs::create_dir_all(&tmp).unwrap();
    let capture = tmp.join("probe.json");
    // probe_delay_ms：让驱动在 Probe RPC 内停留数秒，便于在**探测进行中**抓到
    // 临时子进程与被它认领的端口。
    let body = serde_json::json!({
        "mode": "probe",
        "connection": {
            "password": {"secret_set": true},
            "probe_delay_ms": 3000,
            "capture_path": capture.to_string_lossy(),
        }
    })
    .to_string();

    let probing = {
        let app = app.clone();
        let body = body.clone();
        tokio::spawn(async move { post_connection_check(app, uri, &body).await })
    };

    // ① 探测进行中：临时 driver 进程确实起来了（与运行实例并存）。
    wait_for_new_pid(&run_pids, &name).await;
    let temp_pids: Vec<u32> = {
        let mut v: Vec<u32> = live_pids(&name).difference(&run_pids).copied().collect();
        v.sort_unstable();
        v
    };
    // ② 它同时认领了一个新的端口租约。
    //
    // 只观察本测试 Core 的租约；运行实例已在基线中，新增项属于本次临时探测。
    // 其它 nextest 测试即使同时启动 Driver，也不能成为这里的清理判据。
    let temp_port = wait_for_new_claim(&claims_before).await;
    let new_claims: Vec<u16> = {
        let mut v: Vec<u16> = claimed_ports()
            .difference(&claims_before)
            .copied()
            .collect();
        v.sort_unstable();
        v
    };
    eprintln!(
        "[c3-08-diag] temp_pids={temp_pids:?} temp_port={temp_port} \
         new_claims={new_claims:?} run_pids={run_pids:?}"
    );

    let (status, v) = probing.await.expect("探测请求任务不得 panic");
    assert_eq!(status, axum::http::StatusCode::OK, "body: {v}");
    assert_eq!(v["reachable"], true, "body: {v}");

    // ③ 临时进程回收：PID 回落到基线（运行实例仍在），且端口租约释放。
    assert_no_orphan(&run_pids, &name).await;
    common::wait_until(15, || {
        !claim_dir().join(format!("port-{temp_port}.lock")).exists()
    })
    .await;
    // 运行实例的进程必须**还是原来那个**（不是被杀后重启出来的新 PID）。
    assert!(
        run_pids.is_subset(&live_pids(&name)),
        "连接检查不得重启/停止既有运行实例: {:?}",
        live_pids(&name)
    );

    // ④ 运行实例仍在出数：探测之后必须看到新的采集值（断流/重启都会失败）。
    let mut advanced = false;
    for _ in 0..100 {
        if let Some(v) = latest_value(&mgr, "c3.counter")
            && v > value_before
        {
            advanced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        advanced,
        "连接检查后既有运行实例必须继续出数（前值 {value_before}）"
    );
    assert_eq!(
        mgr.snapshot().endpoint("e-run").map(|s| s.state.clone()),
        Some("RUNNING".to_string()),
        "运行实例状态必须保持 RUNNING"
    );

    // ⑤ 无副作用：持久化面（connection_json / endpoint_secrets / desired_running /
    //    任务 / 事件任务 / point_map）逐字一致。
    assert_eq!(
        c3_persisted_snapshot(&store),
        persisted_before,
        "连接检查不得修改任何持久化状态"
    );
    // 临时 Driver 实际收到的仍是明文（顺带锁定 C3-04 在运行实例并存时同样成立）。
    let captured = std::fs::read_to_string(&capture).expect("临时 Driver 必须落盘收到的连接");
    assert!(
        !captured.contains("secret_set") && captured.contains(PW),
        "送到临时 Driver 的必须是明文: {captured}"
    );

    // ⑥ 失败路径同样必须回收：501（Driver 不支持探测）之后不得留下任何临时进程。
    let (status, v) = post_connection_check(
        app.clone(),
        uri,
        &serde_json::json!({
            "mode": "probe",
            "connection": {"password": {"secret_set": true}, "probe_mode": "unsupported"}
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_IMPLEMENTED, "body: {v}");
    assert_no_orphan(&run_pids, &name).await;
    assert!(
        run_pids.is_subset(&live_pids(&name)),
        "失败路径也不得影响既有运行实例"
    );
    assert_eq!(
        c3_persisted_snapshot(&store),
        persisted_before,
        "失败的连接检查同样不得修改任何持久化状态"
    );

    mgr.shutdown_all().await;
    std::fs::remove_dir_all(&tmp).ok();
    cleanup_dir(&root);
}
