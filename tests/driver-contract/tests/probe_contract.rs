//! Dynamic Probe 合同（V1.2.1 §8，feat/dynamic-probe 阶段 8）。
//!
//! - 会话级：`Session::probe` 成功身份 / 挂起超时映射（in-process，无二进制依赖）；
//! - 编排级：`MesaManager::probe` 成功 + hints + 无孤儿（真子进程，唯一命名防串扰）；
//! - 故障级：BAD_CONFIG / RPC 超时两条路同样无孤儿。
//!
//! 孤儿判定：测试前后按可执行名快照 PID 集合，最终必须回落到基线子集
//! （15s 轮询；唯一命名保证不受并行 suite 干扰）。

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mesa_driver_manager::MesaManager;
use mesa_driver_manager::probe::ProbeError;
use mesa_driver_manager::session::Session;

use common::{TOKEN, repo_root, sim_exe, start_sim_server};

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
fn live_pids(name: &str) -> HashSet<u32> {
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
            if let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) {
                if comm.trim() == name {
                    set.insert(pid);
                }
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

// ---- 会话级（in-process）----

#[tokio::test]
async fn session_probe_simulator_returns_identity() {
    let (port, cancel) = start_sim_server().await;
    let (session, _events, _) = Session::connect(port, TOKEN).await.unwrap();
    assert_eq!(
        session.negotiated_minor(),
        mesa_driver_protocol::PROTOCOL_MINOR
    );
    let r = session.probe("{}").await.expect("probe ok");
    assert!(r.reachable);
    assert_eq!(r.vendor.as_deref(), Some("Mesa"));
    assert_eq!(r.family.as_deref(), Some("Simulator"));
    assert_eq!(r.model.as_deref(), Some("Basic"));
    assert_eq!(r.firmware.as_deref(), Some("1.0"));
    assert_eq!(r.capabilities.read, Some(true));
    assert_eq!(r.capabilities.subscribe, Some(true));
    assert_eq!(r.capabilities.browse, Some(true));
    assert!(r.warnings.is_empty());
    cancel.cancel();
}

#[tokio::test]
async fn session_probe_hanging_server_maps_to_timeout() {
    // 假驱动：走完 Hello/Welcome 后永远沉默 → RPC 超时（REQUEST_TIMEOUT 10s）。
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use mesa_driver_protocol::{pb, read_envelope, write_envelope};
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (mut rd, mut wr) = stream.into_split();
        // 驱动先发言 Hello，Core 回 Welcome（§14.3 握手方向）
        let Ok(_) = write_envelope(
            &mut wr,
            &pb::Envelope {
                msg_id: 1,
                body: Some(pb::envelope::Body::Hello(pb::Hello {
                    driver_id: "hang".into(),
                    driver_version: "0.0.0".into(),
                    protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
                    protocol_minor: mesa_driver_protocol::PROTOCOL_MINOR,
                    sdk_version: "test".into(),
                    platform: std::env::consts::OS.into(),
                    instance_id: "hang-1".into(),
                    session_token: TOKEN.into(),
                })),
            },
        )
        .await
        else {
            return;
        };
        let Ok(_) = read_envelope(&mut rd).await else {
            return;
        };
        // 读掉 ProbeRequest 然后沉默
        let _ = read_envelope(&mut rd).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let (session, _events, _) = Session::connect(port, TOKEN).await.unwrap();
    let err = session.probe("{}").await.expect_err("必须超时");
    assert!(
        matches!(err, mesa_driver_manager::session::SessionError::Timeout),
        "实际: {err}"
    );
}

// ---- 编排级（真子进程）----

#[tokio::test]
async fn manager_probe_success_reports_hints_and_cleans_child() {
    let (root, unique) = stage_drivers_dir("ok", &sim_exe(), "probe-sim-guard");
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
    let (root, unique) = stage_drivers_dir("bad", &sim_exe(), "probe-sim-guard");
    let before = live_pids(&exe_name(&unique));
    let mgr = MesaManager::discover(&root);
    let err = mgr
        .probe("simulator", "not-json")
        .await
        .expect_err("非法 JSON 必须 Err");
    assert!(
        matches!(err, ProbeError::Rpc(_)),
        "simulator BAD_CONFIG 经 DriverError 回传，实际: {err}"
    );
    assert_no_orphan(&before, &exe_name(&unique));
    cleanup_dir(&root);
}

#[tokio::test]
async fn manager_probe_timeout_cleans_hang_child() {
    // hang 桩：握手成功但永不回包 → 内层 RPC 10s 超时 → 同一清理尾回收子进程。
    let (root, unique) = stage_drivers_dir("hang", &hang_exe(), "probe-hang-guard");
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
