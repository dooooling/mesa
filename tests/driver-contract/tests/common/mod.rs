//! Contract Test 公共基建。
//!
//! 两类测试形态（§20 四层中的两层）：
//! - 进程内：SDK `serve_with_faults` + Core `Session` 对连真实 TCP，快速覆盖协议语义；
//! - 子进程：拉起真实 test-driver 二进制 + `run_endpoint` 运行时，覆盖孤儿防护、
//!   Crash Restore 等进程级行为。

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use mesa_core_types::{
    AcquisitionTask, DriverBinding, GENERIC_BINDING_KIND, PointDescriptor, TaskSchedule,
};
use mesa_driver_manager::session::{Session, SessionError, SessionEvent};
use mesa_driver_protocol::pb;
use mesa_driver_sdk::{SdkFaults, serve_with_faults};
use mesa_test_driver::TestDriver;
use tokio_util::sync::CancellationToken;

pub const TOKEN: &str = "contract-test-token";

/// 初始化测试日志（仅输出 WARN 以上到 stderr，避免淹没断言信息；
/// 需要详细排查时设 RUST_LOG=debug 重跑）。
pub fn init_log() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_test_writer()
        .try_init();
}

/// 仓库根目录（workspace 根）。
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// 已构建的 test-driver 可执行文件路径。
///
/// NOTE(cargo 行为): `cargo test -p mesa-contract-tests` 只构建依赖包的 lib，
/// **不会**重编 test-driver 的 bin。子进程类测试前若改过驱动代码，
/// 必须先 `cargo build -p mesa-test-driver`（或 `--workspace`），
/// 否则拉起的是旧二进制、故障注入不生效。
pub fn test_driver_exe() -> PathBuf {
    find_built_binary("mesa-test-driver")
}

/// 已构建的 mesad 可执行文件路径（P1-2 进程级 restart Gate 用；
/// 同上，先 `cargo build --workspace`，否则是旧二进制）。
pub fn mesad_exe() -> PathBuf {
    find_built_binary("mesad")
}

fn find_built_binary(name: &str) -> PathBuf {
    // 允许指向**另一个 target 目录**（`MESA_TEST_BINARY_DIR`）：当默认
    // `target/debug/mesad.exe` 正被一个运行中的实例占用（Windows 无法覆盖
    // 正在运行的 exe）时，用独立 target 目录构建同一份源码并让进程级测试
    // 从那里取二进制。这只改变"从哪取二进制"，不改变任何断言。
    if let Some(dir) = std::env::var_os("MESA_TEST_BINARY_DIR") {
        let dir = PathBuf::from(dir);
        for suffix in [".exe", ""] {
            let p = dir.join(format!("{name}{suffix}"));
            if p.is_file() {
                return p;
            }
        }
    }
    let target = repo_root().join("target");
    for profile in ["debug", "release"] {
        for suffix in [".exe", ""] {
            let p = target.join(profile).join(format!("{name}{suffix}"));
            if p.is_file() {
                return p;
            }
        }
    }
    panic!("{name} binary not built; run cargo build/test first");
}

/// staged drivers 目录守卫（test-driver 退役产品身份后，凡需启动 test-driver
/// 的测试均走此目录：正式 drivers/ manifest 复制 + 已构建二进制 hardlink +
/// test-driver 子目录；temp dir 向上查不到 workspace target/，故必须自带
/// executable；Windows 优先 .exe）。
pub struct StagedDriversDir(PathBuf);
impl StagedDriversDir {
    pub fn path(&self) -> &PathBuf {
        &self.0
    }
}
impl Drop for StagedDriversDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// 有界重试（递增退避），供共享冲突下的文件复制使用。
///
/// 抽成独立函数是为了能被确定性测试：真实的 ERROR_SHARING_VIOLATION 需要
/// Win32 `CreateFileW(share_mode=0)` 才能稳定复现（std 在 Windows 上默认
/// 共享读写删），为它引入新依赖不划算，因此这里测“重试语义”本身：
/// 前几次失败后成功必须返回 Ok，始终失败必须返回最后一次错误。
pub fn retry_io<T, F>(attempts: u32, mut op: F) -> Result<T, std::io::Error>
where
    F: FnMut(u32) -> Result<T, std::io::Error>,
{
    let mut last = None;
    for attempt in 0..attempts {
        match op(attempt) {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = Some(e);
                // 递增退避（20ms→200ms），默认 10 次时总上限约 1.1s：
                // 足够让上一轮 driver 子进程退出、或安全软件释放句柄。
                std::thread::sleep(std::time::Duration::from_millis(
                    20 * u64::from(attempt + 1),
                ));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("retry_io: attempts 必须 ≥ 1")))
}

/// 把已构建的二进制放进 staged 目录。
///
/// 为什么需要它：staged 目录在 `temp_dir()`（Windows 通常 C:），而构建产物在
/// 仓库的 `target/`（可能 D: 等其它卷）——**跨卷无法 hardlink**，只能复制。
/// 而 `std::fs::copy` 在 Windows 上会因共享冲突失败：
/// `Os { code: 32, message: "另一个程序正在使用此文件，进程无法访问。" }`
/// （ERROR_SHARING_VIOLATION）。套件虽然串行，但上一个用例拉起的 driver 子进程
/// 可能仍在退出，或被安全软件短暂持有，命中即整条套件 panic。
///
/// 因此这里：先试 hardlink（同卷时零拷贝、且对**正在运行**的可执行文件也允许），
/// 失败再对有界重试的 copy 兜底，重试耗尽才 panic——并只在这里 panic 一次，
/// 避免调用方用 `.ok()` 静默吞掉失败（那会让 manager 起来后报无关的
/// “descriptor 不可用”，排查成本极高）。
///
/// 重试预算取 **30 次**（递增退避，总上限约 9.3s）：I-2 阶段 2 的合同测试会让
/// 「同进程并行 + 多个 test-driver 子进程同时存活」成为常态，10 次（≈1.1s）在
/// Windows 上被共享冲突打过（实测 `sim_stop_barrier_repeated` 偶发失败）。
fn stage_binary(src: &std::path::Path, dst_dir: &std::path::Path) {
    let name = src
        .file_name()
        .unwrap_or_else(|| panic!("二进制路径缺少文件名: {src:?}"));
    let dst = dst_dir.join(name);
    if std::fs::hard_link(src, &dst).is_ok() {
        return;
    }
    match retry_io(30, |_| std::fs::copy(src, &dst).map(|_| ())) {
        Ok(()) => {}
        Err(e) => panic!("复制 {name:?} 到 staged 目录失败（30 次重试后）: {e:?}"),
    }
}

/// 组装 staged drivers 目录（含 test-driver）。调用方 manager 必须
/// `MesaManager::discover(guard.path())`，AppState 的 drivers_dir 同理。
pub fn staged_drivers_with_test_driver() -> StagedDriversDir {
    let root = std::env::temp_dir().join(format!(
        "mesa-test-drivers-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for driver in ["s7", "focas2", "opcua", "sinumerik-nck"] {
        let src = repo_root().join("drivers").join(driver);
        if let Ok(raw) = std::fs::read(src.join("driver.toml")) {
            let dir = root.join(driver);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("driver.toml"), raw).unwrap();
            link_built_exe(&src, &dir);
        }
    }
    let exe = test_driver_exe();
    let dir = root.join("test-driver");
    std::fs::create_dir_all(&dir).unwrap();
    // 与正式驱动同一套 staging（hardlink → 重试 copy）：test-driver 也必须
    // 在共享冲突下重试，不能 `copy().unwrap()` 直接 panic 掉整条套件。
    stage_binary(&exe, &dir);
    // 单真值：直接复制 canonical manifest（tests/support/test-driver/driver.toml），
    // 不手写 version/protocol——可执行文件名与 canonical 一致（mesa-test-driver），
    // resolve_executable 原生支持（Windows 下 .exe 兜底）。
    let canonical = std::fs::read(repo_root().join("tests/support/test-driver/driver.toml"))
        .expect("canonical test-driver driver.toml must exist");
    std::fs::write(dir.join("driver.toml"), canonical).unwrap();
    StagedDriversDir(root)
}

/// I-2 夹具驱动的 staged 目录：`modes` 里每个短名（`ok`/`paged`/`big`/`slow`/
/// `hang`/`silent`）对应一个独立驱动目录。
///
/// 为什么要多份同一二进制：`DriverProcess::spawn` 只传 `--port`，夹具靠 `argv[0]`
/// 的 stem 判定行为；而环境变量是**进程级**共享状态，同进程并行跑多个用例会串味。
///
/// `driver_id` 固定为 `definitions-fixture-<mode>`，可执行文件名取短名
/// `def-<mode>`（Linux `/proc/<pid>/comm` 截断到 15 字符，长名会让进程枚举断言失效）。
pub fn staged_definitions_fixture(modes: &[&str]) -> StagedDriversDir {
    let root = std::env::temp_dir().join(format!(
        "mesa-def-fixture-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let src = find_built_binary("definitions_fixture");
    for mode in modes {
        let driver_id = format!("definitions-fixture-{mode}");
        let dir = root.join(&driver_id);
        std::fs::create_dir_all(&dir).unwrap();
        let exe_name = format!("def-{mode}");
        stage_binary_as(&src, &dir, &exe_name);
        let manifest = format!(
            "id = \"{driver_id}\"\nname = \"Definitions Fixture {mode}\"\nversion = \"0.0.0\"\nexecutable = \"{exe_name}\"\nprotocol_major = 1\nprotocol_minor = {}\n",
            mesa_driver_protocol::PROTOCOL_MINOR
        );
        std::fs::write(dir.join("driver.toml"), manifest).unwrap();
    }
    StagedDriversDir(root)
}

/// 在夹具目录里**追加** test-driver（A5「采集不受影响」用例需要一条真实采集链）。
///
/// 为什么单独一个函数而不是默认塞进去：多数 I-2 用例刻意只要夹具驱动，
/// 多一个可启动的 test-driver 会让「驱动清单」变大，徒增干扰。
pub fn staged_definitions_fixture_with_test_driver(modes: &[&str]) -> StagedDriversDir {
    let dir = staged_definitions_fixture(modes);
    let td_dir = dir.path().join("test-driver");
    std::fs::create_dir_all(&td_dir).unwrap();
    stage_binary(&test_driver_exe(), &td_dir);
    let canonical = std::fs::read(repo_root().join("tests/support/test-driver/driver.toml"))
        .expect("canonical test-driver driver.toml must exist");
    std::fs::write(td_dir.join("driver.toml"), canonical).unwrap();
    dir
}
/// [`stage_binary`] 的「改名」变体：同一个源二进制可被 staged 成多个名字。
fn stage_binary_as(src: &std::path::Path, dst_dir: &std::path::Path, name: &str) {
    let file_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let dst = dst_dir.join(file_name);
    if std::fs::hard_link(src, &dst).is_ok() {
        return;
    }
    match retry_io(30, |_| std::fs::copy(src, &dst).map(|_| ())) {
        Ok(()) => {}
        Err(e) => panic!("复制 {name:?} 到 staged 目录失败（30 次重试后）: {e:?}"),
    }
}

/// 从 target/ 找已构建二进制，hardlink 到 staged 子目录（Windows 优先 .exe；
/// 无扩展名 Unix 产物在 Windows 下不可执行）。
fn link_built_exe(src_manifest_dir: &std::path::Path, staged_dir: &std::path::Path) {
    let toml = std::fs::read_to_string(src_manifest_dir.join("driver.toml")).unwrap_or_default();
    let exe_base = toml
        .lines()
        .find_map(|l| l.trim().strip_prefix("executable"))
        .and_then(|v| v.split('=').nth(1))
        .map(|v| v.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default();
    if exe_base.is_empty() {
        return;
    }
    let target = repo_root().join("target");
    for profile in ["debug", "release"] {
        // Windows 必须优先 .exe（无扩展名 Unix 产物在 Windows 下不可执行）；
        // cfg! 运行时分支保证两平台都不产生 unused_mut。
        let cands = if cfg!(windows) {
            vec![
                target.join(profile).join(format!("{exe_base}.exe")),
                target.join(profile).join(&exe_base),
            ]
        } else {
            vec![
                target.join(profile).join(&exe_base),
                target.join(profile).join(format!("{exe_base}.exe")),
            ]
        };
        for cand in cands {
            if cand.is_file() {
                stage_binary(&cand, staged_dir);
                return;
            }
        }
        if let Ok(entries) = std::fs::read_dir(target.join(profile)) {
            let want = exe_base.to_ascii_lowercase();
            for e in entries.flatten() {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let n = p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                if n == want || n == format!("{want}.exe") {
                    stage_binary(&p, staged_dir);
                    return;
                }
            }
        }
    }
}

/// 找一个空闲 loopback 端口（bind :0 取号即释放；TOCTOU 下被占则调用方重试）。
pub async fn free_port() -> u16 {
    tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// 启动进程内 test-driver SDK 服务（无故障注入），返回 (端口, 停机句柄)。
pub async fn start_sim_server() -> (u16, CancellationToken) {
    start_sim_server_with_faults(SdkFaults::new()).await
}

/// 启动进程内 test-driver SDK 服务并暴露故障注入开关。
/// 10055/10048 缓冲区耗尽时重试，避免并行 cargo test 抖动。
pub async fn start_sim_server_with_faults(faults: SdkFaults) -> (u16, CancellationToken) {
    let mut last_err = None;
    for i in 0..10 {
        match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
            Ok(listener) => {
                let port = listener.local_addr().unwrap().port();
                let cancel = CancellationToken::new();
                let c = cancel.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_with_faults(
                        TestDriver,
                        listener,
                        TOKEN.into(),
                        c.clone(),
                        Some(faults),
                    )
                    .await
                    {
                        eprintln!("test-driver server ended: {e}");
                    }
                });
                return (port, cancel);
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::AddrInUse
                    || e.raw_os_error() == Some(10055)
                    || e.raw_os_error() == Some(10048) =>
            {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(50 * (i + 1))).await;
                continue;
            }
            Err(e) => panic!("bind failed: {e}"),
        }
    }
    panic!("bind retry exhausted: {:?}", last_err);
}

/// Foundation-2 单路径测试构造：`mesa.resources.v1` selections 数组形态。
pub fn poll_task(id: &str, interval_ms: u64, selections: serde_json::Value) -> AcquisitionTask {
    AcquisitionTask {
        id: id.into(),
        schedule: TaskSchedule::Poll { interval_ms },
        binding: DriverBinding {
            kind: GENERIC_BINDING_KIND.into(),
            config: serde_json::json!({"selections": selections}),
        },
    }
}

/// Foundation-3 #3 wire fail-closed 回归辅助：直发 structured WriteRequest。
/// 返回 WriteResponse 的 result（ok 即 None，失败即 (code, message)）。
/// 调用方先 open_connection(H, "{}")；不需要 configure（write 不依赖采集计划）。
pub async fn write_raw(
    session: &Session,
    resource_id: &str,
    parameters_json: &str,
    output: &str,
    value: Option<pb::ValueMsg>,
    expected_value: Option<pb::ValueMsg>,
) -> Result<(), (String, String)> {
    let reply = session
        .call(pb::envelope::Body::WriteRequest(pb::WriteRequest {
            connection_handle: 1,
            request_id: "wire-fail-closed".into(),
            #[allow(deprecated)]
            target: String::new(),
            value,
            expected_value,
            resource_id: resource_id.into(),
            parameters_json: parameters_json.into(),
            output: output.into(),
        }))
        .await
        .expect("write RPC 必须有响应");
    match reply.body {
        Some(pb::envelope::Body::WriteResponse(r)) => {
            let result = r.result.expect("result present");
            if result.ok {
                Ok(())
            } else {
                let d = result.error.unwrap_or_default();
                Err((d.code, d.message))
            }
        }
        other => panic!("unexpected reply {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 协议化请求辅助：让测试以"Core 的方式"驱动完整配置闭环
// ---------------------------------------------------------------------------

/// OpenConnection 并断言成功。
pub async fn open_connection(session: &Session, handle: u32, config_json: &str) {
    let reply = session
        .call(pb::envelope::Body::OpenConnection(pb::OpenConnection {
            connection_handle: handle,
            endpoint_id: format!("ct-{handle}"),
            config_json: config_json.into(),
        }))
        .await
        .expect("open rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::OpenConnectionAck(ref a)) if ack_ok(a.result.as_ref())),
        "OpenConnection must succeed, got {reply:?}"
    );
}

/// ConfigureTasks 并返回 (point_key -> 上报描述符)。
pub async fn configure_tasks(
    session: &Session,
    handle: u32,
    revision: u64,
    tasks: &[AcquisitionTask],
) -> Vec<PointDescriptor> {
    let tasks_pb = mesa_driver_protocol::tasks_to_pb(tasks).expect("tasks to pb");
    let reply = session
        .call(pb::envelope::Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: handle,
            revision,
            tasks: tasks_pb,
        }))
        .await
        .expect("configure rpc");
    match reply.body {
        Some(pb::envelope::Body::PointDescriptors(rep)) => rep
            .descriptors
            .into_iter()
            // P1：走统一转换（含 source_label wire 回环），不得手写构造
            .map(mesa_driver_protocol::descriptor_from_pb)
            .collect::<Result<Vec<_>, _>>()
            .expect("valid descriptors"),
        other => panic!("expected PointDescriptors, got {other:?}"),
    }
}

/// ApplyPointMap 并断言成功。
pub async fn apply_point_map(
    session: &Session,
    handle: u32,
    revision: u64,
    map: HashMap<String, u32>,
) {
    let reply = session
        .call(pb::envelope::Body::ApplyPointMap(pb::ApplyPointMap {
            connection_handle: handle,
            revision,
            key_to_point_id: map,
        }))
        .await
        .expect("apply rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::ConfigApplied(ref a)) if ack_ok(a.result.as_ref())),
        "ApplyPointMap must succeed"
    );
}

/// StartConnection 并断言成功。
pub async fn start_connection(session: &Session, handle: u32, epoch: u64) {
    let reply = session
        .call(pb::envelope::Body::StartConnection(pb::StartConnection {
            connection_handle: handle,
            stream_epoch: epoch,
        }))
        .await
        .expect("start rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::StartConnectionAck(ref a)) if ack_ok(a.result.as_ref())),
        "StartConnection must succeed"
    );
}

/// StopConnection 并断言 Ack 成功（无论此前是否在运行）。
pub async fn stop_connection(session: &Session, handle: u32) -> bool {
    let reply = session
        .call(pb::envelope::Body::StopConnection(pb::StopConnection {
            connection_handle: handle,
        }))
        .await
        .expect("stop rpc");
    match reply.body {
        Some(pb::envelope::Body::StopConnectionAck(a)) => ack_ok(a.result.as_ref()),
        _ => false,
    }
}

/// CloseConnection 并断言成功。
pub async fn close_connection(session: &Session, handle: u32) {
    let reply = session
        .call(pb::envelope::Body::CloseConnection(pb::CloseConnection {
            connection_handle: handle,
        }))
        .await
        .expect("close rpc");
    assert!(
        matches!(reply.body, Some(pb::envelope::Body::CloseConnectionAck(ref a)) if ack_ok(a.result.as_ref())),
        "CloseConnection must succeed"
    );
}

fn ack_ok(result: Option<&pb::GenericResult>) -> bool {
    result.map(|r| r.ok).unwrap_or(false)
}

/// 从 descriptors 生成分配映射（模拟 Core 分配 point_id：从 base 起顺序编号）。
pub fn sequential_ids(descriptors: &[PointDescriptor], base: u32) -> HashMap<String, u32> {
    descriptors
        .iter()
        .enumerate()
        .map(|(i, d)| (d.point_key.clone(), base + i as u32))
        .collect()
}

/// 完整配置闭环：Open -> Configure -> Apply -> Start。
pub async fn configure_and_start(
    session: &Session,
    handle: u32,
    revision: u64,
    epoch: u64,
    tasks: &[AcquisitionTask],
    id_base: u32,
) -> Vec<PointDescriptor> {
    open_connection(session, handle, "{}").await;
    let descriptors = configure_tasks(session, handle, revision, tasks).await;
    let map = sequential_ids(&descriptors, id_base);
    apply_point_map(session, handle, revision, map).await;
    start_connection(session, handle, epoch).await;
    descriptors
}

// ---------------------------------------------------------------------------
// 事件流辅助
// ---------------------------------------------------------------------------

/// 在 deadline 内收取一个批次；超时 panic。
/// State/DriverError 事件被静默跳过——断言错误事件用 [`expect_driver_error`]，
/// 这里只关心数据面。
pub async fn recv_batch(
    events: &mut tokio::sync::mpsc::Receiver<SessionEvent>,
    secs: u64,
) -> mesa_core_types::DataBatch {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let remain = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remain, events.recv()).await {
            Ok(Some(SessionEvent::Batch(b))) => return b,
            Ok(Some(_)) => continue, // State / DriverError：非本断言目标
            Ok(other) => panic!("expected batch event, got {other:?}"),
            Err(_) => panic!("timed out waiting for batch"),
        }
    }
}

/// 在 deadline 内收取下一个 DriverError 事件；超时/其他事件则继续等待。
pub async fn expect_driver_error(
    session: &Session,
    events: &mut tokio::sync::mpsc::Receiver<SessionEvent>,
    secs: u64,
) -> (String, String, String) {
    let deadline = Duration::from_secs(secs);
    loop {
        match tokio::time::timeout(deadline, events.recv()).await {
            Ok(Some(SessionEvent::DriverError {
                kind,
                code,
                message,
                ..
            })) => {
                return (kind, code, message);
            }
            // 配置失败也可能以错误帧之外的路径表现：请求侧直接报错
            Ok(other) => {
                // 心跳等事件可忽略；若会话已死则直接失败
                if other.is_none() {
                    panic!("session closed before driver error arrived");
                }
                let _ = session; // 保持签名一致性
            }
            Err(_) => panic!("timed out waiting for driver error"),
        }
    }
}

/// 等待条件成立（轮询），超时 panic。
pub async fn wait_until<F>(secs: u64, mut cond: F)
where
    F: FnMut() -> bool,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not met within {secs}s");
}

/// 统一会话收尾。
pub fn teardown(session: &mut Session, server_cancel: Option<CancellationToken>) {
    session.invalidate();
    if let Some(c) = server_cancel {
        c.cancel();
    }
}

/// 断言错误为握手层拒绝。
pub fn assert_handshake_error(e: SessionError) {
    assert!(
        matches!(e, SessionError::Handshake(_)),
        "rejection must be handshake-level, got {e}"
    );
}
