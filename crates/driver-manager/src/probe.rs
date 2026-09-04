//! MesaManager::probe() 编排（V1.2.1 §8，feat/dynamic-probe 阶段 3）。
//!
//! 唯一职责：为一次探测拉起临时 Driver 进程，走完
//! spawn → handshake → Probe RPC → invalidate → terminate，
//! 成功失败超时三条路都必须回收子进程，不留孤儿。
//!
//! 硬性不变量（违反即架构错误）：
//! - 不创建 Endpoint，不写 ConfigStore，不碰 PointRegistry；
//! - 不启动 Data Plane，不改变 stream_epoch；
//! - 不经过 OpenConnection/Configure/ApplyPointMap/Start（§8.5 禁止项）；
//! - 设备不可达是探测结果（`Ok(unreachable)`），只有基础设施失败才是 `Err`。

use mesa_core_types::ProbeReport;
use mesa_driver_protocol::PROBE_RPC_MIN_MINOR;

use crate::manager::MesaManager;
use crate::process::DriverProcess;
use crate::profile::{ProfileMatch, match_profiles};
use crate::session::{PROBE_TIMEOUT, Session, SessionError};

/// 探测基础设施失败（注意：设备不可达不是 Err，是 `Ok(ProbeReport)`）。
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("driver `{0}` not found")]
    DriverNotFound(String),
    /// 对端协商 Minor 过低，不识别 ProbeRequest（不得发 RPC 干等超时）。
    #[error("probe unsupported by driver: {0}")]
    Unsupported(String),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("handshake failed: {0}")]
    Handshake(String),
    /// Probe RPC 超时（细分出来供 REST 映射 504）。
    #[error("probe rpc timed out")]
    RpcTimeout,
    /// Probe RPC 失败：对端关闭 / 非法报告 / Driver 侧错误（供 REST 映射 503）。
    #[error("probe rpc failed: {0}")]
    Rpc(String),
    /// 12s 总预算耗尽（含 spawn/handshake/RPC/cleanup）。
    #[error("probe timed out")]
    Timeout,
}

/// 探测结果：设备事实 + Core 侧确定性 profile 提示（Driver 不参与匹配）。
#[derive(Debug)]
pub struct ProbeResult {
    pub report: ProbeReport,
    pub profile_hints: Vec<ProfileMatch>,
}

/// Probe RPC 版本门控（纯函数，可单测）：协商 Minor < 2 的旧 Driver
/// 不识别 ProbeRequest（会静默忽略），必须直接 Unsupported，不得发 RPC 干等。
pub(crate) fn probe_supported(negotiated_minor: u32) -> bool {
    negotiated_minor >= mesa_driver_protocol::PROBE_RPC_MIN_MINOR
}

impl MesaManager {
    /// 动态探测：返回设备事实报告 + profile 提示。临时进程生命周期与本调用严格绑定。
    pub async fn probe(
        &self,
        driver_id: &str,
        connection_json: &str,
    ) -> Result<ProbeResult, ProbeError> {
        let disc = self
            .find_driver(driver_id)
            .ok_or_else(|| ProbeError::DriverNotFound(driver_id.to_string()))?;
        // 总 deadline 包住全部阶段；内层各步另有独立超时，互不打架。
        let report =
            match tokio::time::timeout(PROBE_TIMEOUT, Self::probe_inner(disc, connection_json))
                .await
            {
                Ok(r) => r?,
                Err(_) => return Err(ProbeError::Timeout),
            };
        // facts→profile 解释权只在 Core：用本机加载的 profiles 做确定性匹配。
        let profiles = self.profiles.read().unwrap();
        let profile_hints = match_profiles(driver_id, &report, &profiles);
        Ok(ProbeResult {
            report,
            profile_hints,
        })
    }

    async fn probe_inner(
        disc: crate::manifest::DiscoveredDriver,
        connection_json: &str,
    ) -> Result<ProbeReport, ProbeError> {
        let mut proc = DriverProcess::spawn(&disc)
            .await
            .map_err(|e| ProbeError::Spawn(e.to_string()))?;
        // 单出口清理：无论 inner 成功失败，临时进程必须 terminate。
        // session.invalidate() 在 inner 内部、RPC 结束后执行（连接级清理），
        // 进程级 terminate 在此统一执行（RAII guard 思想的手动版）。
        let result: Result<ProbeReport, ProbeError> = async {
            let (mut session, _events, _) = Session::connect_retry(proc.port, &proc.token)
                .await
                .map_err(|e| ProbeError::Handshake(e.to_string()))?;
            let r = async {
                if !probe_supported(session.negotiated_minor()) {
                    return Err(ProbeError::Unsupported(format!(
                        "negotiated minor {} < {}",
                        session.negotiated_minor(),
                        PROBE_RPC_MIN_MINOR
                    )));
                }
                session.probe(connection_json).await.map_err(|e| match e {
                    SessionError::Timeout => ProbeError::RpcTimeout,
                    other => ProbeError::Rpc(other.to_string()),
                })
            }
            .await;
            session.invalidate();
            r
        }
        .await;
        proc.terminate().await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::MesaManager;

    #[test]
    fn old_minor_is_gated_before_any_rpc() {
        assert!(!probe_supported(0));
        assert!(!probe_supported(1));
        assert!(probe_supported(2));
        assert!(probe_supported(u32::MAX));
    }

    #[test]
    fn probe_error_display_is_stable() {
        assert_eq!(
            ProbeError::DriverNotFound("x".into()).to_string(),
            "driver `x` not found"
        );
        assert_eq!(ProbeError::Timeout.to_string(), "probe timed out");
        assert_eq!(ProbeError::RpcTimeout.to_string(), "probe rpc timed out");
    }

    fn now_ns() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[tokio::test]
    async fn probe_unknown_driver_is_not_found_without_spawn() {
        let root = std::env::temp_dir().join(format!("fl-probe-404-{}", now_ns()));
        std::fs::create_dir_all(&root).unwrap();
        let mgr = MesaManager::discover(&root);
        let err = mgr
            .probe("no-such-driver", "{}")
            .await
            .expect_err("未知 Driver 必须 Err");
        assert!(matches!(err, ProbeError::DriverNotFound(_)), "实际: {err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn probe_missing_binary_fails_fast_at_spawn() {
        // TOCTOU 形态：扫描时二进制存在（find_driver 可见），探测前被删，
        // spawn 即报 MissingBinary，不进入 6s handshake 重试，更不挂起。
        // 注意 find_driver 只返回 launchable 项，缺二进制的常态是 NotFound。
        let root = std::env::temp_dir().join(format!("fl-probe-nobin-{}", now_ns()));
        let dir = root.join("ghost");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("driver.toml"),
            format!(
                "id=\"ghost\"\nname=\"Ghost\"\nversion=\"0.1.0\"\nexecutable=\"ghost-nobin-bin\"\nprotocol_major={}\nprotocol_minor=2\n",
                mesa_driver_protocol::PROTOCOL_MAJOR
            ),
        )
        .unwrap();
        let exe = dir.join("ghost-nobin-bin");
        std::fs::write(&exe, b"placeholder").unwrap();
        let mgr = MesaManager::discover(&root);
        assert!(
            mgr.find_driver("ghost").is_some(),
            "预置：扫描时 ghost 必须 launchable"
        );
        std::fs::remove_file(&exe).unwrap();
        let err = mgr
            .probe("ghost", "{}")
            .await
            .expect_err("二进制消失必须 Err");
        assert!(matches!(err, ProbeError::Spawn(_)), "实际: {err}");
        std::fs::remove_dir_all(&root).ok();
    }
}
