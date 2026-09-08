//! 临时驱动操作原语：Management Plane 的 Descriptor/Probe/Browse 共用。
//!
//! 一次临时操作 = spawn 新进程 → 建连（Hello/Welcome）→ 调用方 RPC →
//! invalidate → terminate，单出口，成功失败都回收子进程，不留孤儿。
//! startup transport 瞬态（建连三阶段的 Io/Timeout/Protocol(Io)，如 accept
//! 后对端启动期死亡导致的 reset）kill 半活进程后换新 port + 新进程重建整个
//! attempt，最多重建一次；驱动已说话后的语义错误一次判死，不重试。
//! 同一进程只 accept 一次管理连接（§14.2），重试绝不能在同一进程 reconnect。

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::mpsc;

use crate::manifest::DiscoveredDriver;
use crate::process::{DriverProcess, SpawnError};
use crate::session::{
    ConnectError, HeartbeatParams, Session, SessionError, SessionEvent, StartupStage,
};
use mesa_driver_protocol::ProtocolError;

/// 启动 attempt 上限（含首次）：transport 瞬态重建一次。单 attempt 内建连
/// 另有 6s deadline；各操作外层超时（probe 12s 等）兜底总和。
pub(crate) const MAX_TEMP_ATTEMPTS: u32 = 2;

/// 存活临时驱动（进程 + 会话）：RPC 阶段持有者；`cleanup` 单出口回收。
pub(crate) struct TempDriver {
    pub proc: DriverProcess,
    pub session: Session,
    #[allow(dead_code)]
    pub events: mpsc::Receiver<SessionEvent>,
    #[allow(dead_code)]
    pub unresponsive: Arc<AtomicBool>,
}

impl TempDriver {
    /// 单出口回收：连接级 invalidate + 进程级 terminate（优于强杀；
    /// 半活进程同样走 graceful 路径，宽限后兜底）。
    pub async fn cleanup(mut self) {
        self.session.invalidate();
        self.proc.terminate().await;
    }
}

/// spawn/建连失败（结构化阶段）：调用方映射为各自的不可用错误，
/// message 自带 stage，下次 reset 直接定位死在哪。
#[derive(Debug)]
pub(crate) enum TempStartupError {
    /// spawn 阶段（含端口租约）：MissingBinary→spawn，NoPort→bind，
    /// Io/Job→spawn。从不重建（本进程/环境问题，重建无意义）。
    Spawn {
        stage: StartupStage,
        message: String,
    },
    /// 建连三阶段：transport 瞬态可重建，语义失败一次判死（见
    /// [`is_transient_connect`]）。
    Connect(ConnectError),
}

impl TempStartupError {
    /// 是否 transport 瞬态（可 kill 后重建）：仅建连三阶段的
    /// Io/Timeout/Protocol(Io)；spawn 失败与语义失败一次判死。
    pub(crate) fn transient(&self) -> bool {
        match self {
            TempStartupError::Spawn { .. } => false,
            TempStartupError::Connect(c) => is_transient_connect(c),
        }
    }
}

impl std::fmt::Display for TempStartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TempStartupError::Spawn { stage, message } => {
                write!(f, "spawn failed at {stage}: {message}")
            }
            TempStartupError::Connect(e) => {
                write!(f, "handshake failed at {}: {}", e.stage, e.source)
            }
        }
    }
}

/// 建连失败是否 transport 瞬态（零字符串匹配，按 (stage, source 变体) 路由）：
/// Connect/Hello/Welcome 三阶段的 Io/Timeout/Protocol(Io) 可 kill 后重建；
/// token/版本/解码等语义失败一次判死。
pub(crate) fn is_transient_connect(e: &ConnectError) -> bool {
    matches!(
        e.source,
        SessionError::Io(_) | SessionError::Timeout | SessionError::Protocol(ProtocolError::Io(_))
    )
}

/// 单次 spawn+建连（无重试、无清理；重试循环见 [`temp_driver_attempt`]）。
async fn spawn_and_connect(disc: &DiscoveredDriver) -> Result<TempDriver, TempStartupError> {
    let proc = DriverProcess::spawn(disc).await.map_err(|e| {
        let stage = match &e {
            SpawnError::NoPort => StartupStage::Bind,
            _ => StartupStage::Spawn,
        };
        TempStartupError::Spawn {
            stage,
            message: e.to_string(),
        }
    })?;
    let port = proc.port;
    let token = proc.token.clone();
    match Session::connect_retry_with_stage(port, &token, HeartbeatParams::default()).await {
        Ok((session, events, unresponsive)) => Ok(TempDriver {
            proc,
            session,
            events,
            unresponsive,
        }),
        Err(e) => {
            // 建连失败：半活进程必须先杀掉再返回（端口租约随 drop 释放），
            // 否则重建的新进程与旧进程抢port/留孤儿。
            // NOTE: terminate（grace+强杀兜底）优于 force_kill。
            let mut proc = proc;
            proc.terminate().await;
            Err(TempStartupError::Connect(e))
        }
    }
}

/// 临时操作统一入口：attempt 循环 + RPC + 单出口清理。
/// `rpc` 移入存活驱动做各操作 RPC，返回 `(TempDriver, 结果)` 以便原语
/// 单出口清理（`FnMut` 按值传递避开 async 闭包高阶借用问题）。
/// 返回 `Ok(T)` 成功，`Err(E)` 为调用方 RPC 错误（原样带出，不重试）。
/// startup 失败：transport 瞬态重建一次后仍败则返回 `Startup`，
/// 语义失败直接返回 `Startup`。
pub(crate) enum TempOpError<E> {
    Startup(TempStartupError),
    Rpc(E),
}

pub(crate) async fn temp_driver_attempt<T, E, F, Fut>(
    disc: &DiscoveredDriver,
    mut rpc: F,
) -> Result<T, TempOpError<E>>
where
    F: FnMut(TempDriver) -> Fut,
    Fut: Future<Output = (TempDriver, Result<T, E>)>,
{
    let mut attempt = 0u32;
    loop {
        match spawn_and_connect(disc).await {
            Ok(td) => {
                let (td, r) = rpc(td).await;
                td.cleanup().await;
                return r.map_err(TempOpError::Rpc);
            }
            Err(e) if e.transient() && attempt + 1 < MAX_TEMP_ATTEMPTS => {
                tracing::warn!(
                    driver = %disc.manifest.id,
                    attempt = attempt + 1,
                    error = %e,
                    "temp operation startup transient, respawning fresh attempt"
                );
                attempt += 1;
            }
            Err(e) => return Err(TempOpError::Startup(e)),
        }
    }
}
