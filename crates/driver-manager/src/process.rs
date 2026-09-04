//! Driver 子进程生命周期管理（方案 §14.2 / §14.5 / §17）。
//!
//! 关键点：
//! - token 经 stdin 注入（非命令行，避免进程列表泄露）；
//! - Core 持有 child stdin 不关闭——这是 liveness pipe，Core 死亡时 OS 关闭管道，
//!   Driver 侧读到 EOF 自行退出（孤儿防护第一层）；
//! - Windows 上所有子进程加入启用 `KILL_ON_JOB_CLOSE` 的 Job Object
//!   （孤儿防护第二层）；Linux 对应 PR_SET_PDEATHSIG 由 Driver 自己设置。

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};

use crate::manifest::DiscoveredDriver;

/// Shutdown 后等待进程退出的宽限；超时强制终止（§14.4）。
pub const TERMINATE_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("driver executable missing: {0}")]
    MissingBinary(String),
    #[error("no free local port")]
    NoPort,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("job object error: code {0}")]
    Job(u32),
}

/// 一个已启动的 Driver 子进程。
///
/// stdin 被刻意保存在结构体中不关闭；Drop 时随结构体一起关闭触发 EOF 防护。
pub struct DriverProcess {
    pub port: u16,
    pub pid: u32,
    /// 本次启动生成的 session token（已注入子进程 stdin）。
    pub token: String,
    child: Child,
    #[allow(dead_code)]
    stdin: Option<ChildStdin>,
    _job: job::JobAttachment,
    /// 端口租约：与子进程生命周期绑定，drop 时释放。
    /// 禁止移除——移除即重开 Core 内并发 spawn 抢同一端口的 TOCTOU（P1-A）。
    _port_lease: PortLease,
}

impl DriverProcess {
    /// 启动 Driver 进程并等待其 IPC 端口可连接。
    pub async fn spawn(disc: &DiscoveredDriver) -> Result<Self, SpawnError> {
        let exe = disc
            .executable_path
            .as_ref()
            .ok_or_else(|| SpawnError::MissingBinary(disc.manifest.executable.clone()))?;

        // 端口租约必须在 spawn 之前持有、随结构体释放：
        // 同一 Core 并发 spawn 时，OS 循环复用端口号，裸 free_port 会让
        // 两个 child 拿到同一端口（后 bind 失败，或更糟：Core 连错进程，
        // token mismatch → Handshake 失败 → 偶发 503）。
        let port_lease = lease_port().ok_or(SpawnError::NoPort)?;
        let port = port_lease.port;
        let token = generate_session_token();

        let mut cmd = tokio::process::Command::new(exe);
        cmd.args(["--port", &port.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            // 进程对象被 drop 时兜底杀掉，防止异常路径泄漏
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        #[cfg(windows)]
        let job = job::attach_current_child(pid, child.raw_handle())?;
        #[cfg(not(windows))]
        let job = job::attach_current_child(pid, None)?;

        // 写半部必须在结构体中持续持有：一旦关闭，Driver 侧 EOF 防护会立刻退出
        let mut stdin = child
            .stdin
            .take()
            .ok_or(SpawnError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "child stdin not piped",
            )))?;
        stdin.write_all(token.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        // NOTE: 不做 TCP 探活——SDK 只 accept 一条管理连接，任何探测性连接
        // 都会抢占该名额导致握手永远失败。端口未就绪由 Session::connect_retry 处理。
        Ok(Self {
            port,
            pid,
            token,
            child,
            stdin: Some(stdin),
            _job: job,
            _port_lease: port_lease,
        })
    }

    /// 兜底强杀（正常路径应先经 Session 发送 Shutdown 消息）。
    pub fn force_kill(&mut self) {
        let _ = self.child.start_kill();
    }

    /// 优雅退出：关闭 stdin 触发 EOF 防护 → 等待宽限 → 强杀。
    /// IPC 层的 Shutdown 消息由上层在调用本方法前发送。
    pub async fn terminate(&mut self) {
        // 关闭 liveness 管道是第一信号
        drop(self.stdin.take());
        match tokio::time::timeout(TERMINATE_GRACE, self.child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                tracing::warn!(pid = self.pid, "grace exceeded, killing driver");
                self.force_kill();
                let _ = self.child.wait().await;
            }
        }
    }
}

/// Core 进程内已租出的 Driver IPC 端口表（P1-A）。
/// OS 会循环复用刚释放的端口号；并发 spawn 时裸 bind(:0)-取号-drop
/// 必然让两个 child 撞号。租约表消除 Core 内部竞争源（生产最现实的
/// 竞争：多 Endpoint 并发启动 / reconnect 与 Probe 并发）。
/// 外部进程在 bind-drop-child_bind 极小窗口抢号理论仍存在，届时靠
/// connect_retry + token mismatch fail-closed 兜底（报错，不连错设备）。
static ACTIVE_DRIVER_PORTS: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();

fn active_driver_ports() -> &'static Mutex<HashSet<u16>> {
    ACTIVE_DRIVER_PORTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 端口租约：持有期间该端口不会被本 Core 再次分配；
/// 随 `DriverProcess` drop 自动释放（子进程已退出，端口可安全复用）。
struct PortLease {
    port: u16,
}

impl Drop for PortLease {
    fn drop(&mut self) {
        active_driver_ports().lock().unwrap().remove(&self.port);
    }
}

/// 分配 IPC 端口租约：OS bind(:0) 取号 + 进程内去重（至多 64 次重试）。
/// 锁只保护 HashSet 插查（微秒级），bind 本身在锁外，不阻塞并发 spawn。
fn lease_port() -> Option<PortLease> {
    for _ in 0..64 {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .ok()?
            .local_addr()
            .ok()?
            .port();
        // NOTE: 此处 listener 已 drop，child 尚未 bind——窗口仍在，
        // 但至少本 Core 内绝不双发同一端口（外部抢号属下一级小概率事件）。
        if active_driver_ports().lock().unwrap().insert(port) {
            return Some(PortLease { port });
        }
    }
    None
}

/// 256-bit 随机 session token 的十六进制形式（§14.2）。
fn generate_session_token() -> String {
    use std::fmt::Write;
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("os rng");
    let mut s = String::with_capacity(64);
    for b in buf {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

// ---------------------------------------------------------------------------
// Windows Job Object（孤儿防护第二层）
// ---------------------------------------------------------------------------

mod job {
    #[cfg(windows)]
    use std::sync::OnceLock;

    /// Core 进程级唯一 Job Object：Core 终止后 OS 自动清理全部成员进程。
    #[cfg(windows)]
    static CORE_JOB: OnceLock<Result<JobHandle, u32>> = OnceLock::new();

    /// 表示"该子进程已被纳入 Core Job"。非 Windows 平台为空实现。
    pub struct JobAttachment {
        _priv: (),
    }

    impl std::fmt::Debug for JobAttachment {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("JobAttachment")
        }
    }

    #[cfg(windows)]
    pub fn attach_current_child(
        _pid: u32,
        raw_handle: Option<std::os::windows::io::RawHandle>,
    ) -> Result<JobAttachment, super::SpawnError> {
        use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
        let job = CORE_JOB
            .get_or_init(create_core_job)
            .as_ref()
            .map_err(|&code| super::SpawnError::Job(code))?;
        if let Some(h) = raw_handle {
            // RawHandle 与 HANDLE 同为 *mut c_void
            let proc_handle: HANDLE = h;
            let ok = job.assign(proc_handle);
            if ok == 0 {
                return Err(super::SpawnError::Job(unsafe { GetLastError() }));
            }
        }
        Ok(JobAttachment { _priv: () })
    }

    #[cfg(not(windows))]
    pub fn attach_current_child(
        _pid: u32,
        _raw_handle: Option<core::ffi::c_void>,
    ) -> Result<JobAttachment, super::SpawnError> {
        // Linux 依赖 Driver 侧 PR_SET_PDEATHSIG（方案 §14.5），Core 侧无需动作
        Ok(JobAttachment { _priv: () })
    }

    #[cfg(windows)]
    struct JobHandle(windows_sys::Win32::Foundation::HANDLE);

    #[cfg(windows)]
    unsafe impl Send for JobHandle {}
    #[cfg(windows)]
    unsafe impl Sync for JobHandle {}

    #[cfg(windows)]
    impl JobHandle {
        fn assign(&self, proc: windows_sys::Win32::Foundation::HANDLE) -> i32 {
            unsafe {
                windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(self.0, proc)
            }
        }
    }

    #[cfg(windows)]
    impl Drop for JobHandle {
        fn drop(&mut self) {
            use windows_sys::Win32::Foundation::CloseHandle;
            unsafe { CloseHandle(self.0) };
        }
    }

    #[cfg(windows)]
    fn create_core_job() -> Result<JobHandle, u32> {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(GetLastError());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = GetLastError();
                CloseHandle(handle);
                return Err(err);
            }
            Ok(JobHandle(handle))
        }
    }
}
