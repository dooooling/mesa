//! MesaManager::probe() 编排（V1.2.1 §8）。
//!
//! 唯一职责：为一次探测拉起临时 Driver 进程，走完
//! spawn → handshake → OpenConnection(临时) → Probe RPC → CloseConnection
//! → invalidate → terminate，成功失败超时三条路都必须回收子进程，不留孤儿。
//!
//! 硬性不变量（违反即架构错误）：
//! - 不创建 Endpoint，不写 ConfigStore，不碰 PointRegistry；
//! - 不启动 Data Plane，不改变 stream_epoch；
//! - 禁止 Configure/ApplyPointMap/Start（§8.5）；
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
    /// 连接配置被 Driver 拒绝（供 REST 映射 400；仍是基础设施侧结论，
    /// 与 REST 层 JSON 外形校验的 VALIDATION_ERROR 区分）。
    #[error("invalid probe input {code}: {message}")]
    InvalidInput { code: String, message: String },
    /// 12s 总预算耗尽（含 spawn/handshake/RPC/cleanup）。
    #[error("probe timed out")]
    Timeout,
}

/// Driver 结构化错误 → ProbeError（P1-2：按 kind/code 精确路由，
/// 禁止字符串 contains()/parse 回猜）。
fn driver_error_to_probe_error(kind: &str, code: &str, message: String) -> ProbeError {
    if code == "PROBE_UNSUPPORTED" {
        ProbeError::Unsupported(message)
    } else if kind == mesa_core_types::ErrorKind::Configuration.as_str() {
        ProbeError::InvalidInput {
            code: code.to_string(),
            message,
        }
    } else {
        ProbeError::Rpc(format!("{kind}/{code}: {message}"))
    }
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
        // 没探测到 ≠ 猜型号：unreachable 时 probe.* 全空，driver_id-only 规则
        // 会误命中具体硬件型号（如 s7-1200/1214C），此时 hints 必须为空（P1-B）。
        let profiles = self.profiles.read().unwrap();
        let profile_hints = if report.reachable {
            match_profiles(driver_id, &report, &profiles)
        } else {
            Vec::new()
        };
        Ok(ProbeResult {
            report,
            profile_hints,
        })
    }

    /// 临时探测连接句柄（Core 分配；临时会话内唯一即可，0 有特殊含义禁用）。
    const PROBE_HANDLE: u32 = 999;

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
                // P0-2 冻结生命周期：OpenConnection → Probe → CloseConnection，
                // 与正常采集走完全相同的建连路径（Secret/PKI/会话），
                // Configure/Apply/Start 仍禁止。
                Self::open_temp(&session, &disc.manifest.id, connection_json).await?;
                let pr = session
                    .probe(Self::PROBE_HANDLE)
                    .await
                    .map_err(|e| match e {
                        SessionError::Timeout => ProbeError::RpcTimeout,
                        SessionError::Driver {
                            kind,
                            code,
                            message,
                        } => driver_error_to_probe_error(&kind, &code, message),
                        other => ProbeError::Rpc(other.to_string()),
                    });
                // close 必须执行（best-effort）：probe 成败都不留已开连接
                Self::close_temp(&session).await;
                pr
            }
            .await;
            session.invalidate();
            r
        }
        .await;
        proc.terminate().await;
        result
    }

    /// 打开临时探测连接（与正常采集相同的 OpenConnection 路径）。
    async fn open_temp(
        session: &Session,
        driver_id: &str,
        connection_json: &str,
    ) -> Result<(), ProbeError> {
        use mesa_driver_protocol::pb;
        let reply = session
            .call(pb::envelope::Body::OpenConnection(pb::OpenConnection {
                connection_handle: Self::PROBE_HANDLE,
                endpoint_id: format!("probe-{driver_id}"),
                config_json: connection_json.to_string(),
            }))
            .await
            .map_err(|e| match e {
                SessionError::Timeout => ProbeError::RpcTimeout,
                other => ProbeError::Rpc(other.to_string()),
            })?;
        match reply.body {
            Some(pb::envelope::Body::OpenConnectionAck(ack)) => match ack.result {
                Some(r) if r.ok => Ok(()),
                Some(r) => {
                    let d = r.error.unwrap_or_default();
                    Err(driver_error_to_probe_error(
                        &d.kind,
                        &d.code,
                        format!("open failed: {}", d.message),
                    ))
                }
                None => Err(ProbeError::Rpc("open failed: empty result".into())),
            },
            Some(pb::envelope::Body::DriverError(e)) => {
                let d = e.detail.unwrap_or_default();
                Err(driver_error_to_probe_error(
                    &d.kind,
                    &d.code,
                    format!("open failed: {}", d.message),
                ))
            }
            _ => Err(ProbeError::Rpc("open failed: unexpected reply".into())),
        }
    }

    /// 关闭临时探测连接（best-effort：失败只诊断，不掩盖探测结论）。
    async fn close_temp(session: &Session) {
        use mesa_driver_protocol::pb;
        if let Err(e) = session
            .call(pb::envelope::Body::CloseConnection(pb::CloseConnection {
                connection_handle: Self::PROBE_HANDLE,
            }))
            .await
        {
            tracing::debug!("probe close temp connection failed (ignored): {e}");
        }
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
    fn driver_error_routes_by_code_not_substring() {
        // PROBE_UNSUPPORTED → Unsupported（REST 501）
        assert!(matches!(
            driver_error_to_probe_error("UnsupportedError", "PROBE_UNSUPPORTED", "x".into()),
            ProbeError::Unsupported(_)
        ));
        // ConfigurationError → InvalidInput（REST 400），无论 message 写什么
        assert!(matches!(
            driver_error_to_probe_error("ConfigurationError", "BAD_CONFIG", "boom".into()),
            ProbeError::InvalidInput { .. }
        ));
        // 其他 → Rpc（REST 503）
        assert!(matches!(
            driver_error_to_probe_error("ConnectionError", "CONNECT_FAIL", "x".into()),
            ProbeError::Rpc(_)
        ));
        assert!(matches!(
            driver_error_to_probe_error("", "", "x".into()),
            ProbeError::Rpc(_)
        ));
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
