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
use crate::session::{
    DRIVER_STARTUP_TIMEOUT, PROBE_RPC_TIMEOUT, Session, SessionError, StartupStage,
};

/// 临时进程清理预算（Close RPC + invalidate + graceful terminate + 强杀兜底）。
const PROBE_CLEANUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(6);
/// 未配置 `timeout_ms` 时假定的协议超时（与四驱动默认值同量级）。
const PROBE_ASSUMED_TIMEOUT_MS: u64 = 5_000;
/// `OpenConnection` RPC 预算。
///
/// **四个驱动的 `open_connection` 都不做协议 I/O**，只做配置解析与对象构造
/// （S7/FOCAS2/NCK 解析 JSON 后直接返回连接对象；OPC UA 只构造 transport），
/// 真正的 TCP/COTP/Setup/OPEN 握手全部发生在随后的 `Probe` RPC 内。
/// 因此这里是**本地解析预算**，不是协议预算。
const PROBE_OPEN_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
/// `CloseConnection` RPC 预算（best-effort：失败只诊断，不掩盖探测结论）。
const PROBE_CLOSE_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// 一次 Probe 的分阶段预算（C1 §6.3）。
///
/// 阶段模型：`startup(spawn + IPC 握手)` → `OpenConnection`（本地解析）→
/// `Probe`（**协议握手 + 探测查询**）→ `CloseConnection` → `cleanup(terminate)`。
///
/// 为什么不再用固定 12s：合法的慢设备会被外层预算提前掐断，表现为不可诊断的
/// `ProbeError::Timeout`——用户看到的超时与实际配置对不上。
///
/// 为什么 Query 才是大预算：探测路径上，协议建连发生在 **`Probe` RPC 内**，
/// 且是串行的多阶段（S7 实测路径：TCP connect → COTP CC → S7 Setup → SZL；
/// NCK 为前三段；OPC UA 为 connect → 读命名空间 → 读节点）。每个响应阶段各自
/// 受 `timeout_ms` 约束，因此 `Probe` RPC 的预算必须按
/// `阶段数 × timeout_ms` 估算，而不是 1×。
///
/// 阶段数取 4 而不是 5：S7 的 COTP CR 与 Setup 请求是「发送」阶段，不等待响应；
/// 等待响应的串行阶段为 TCP connect / COTP CC / Setup resp / SZL resp。
/// 更重要的是**任一等待阶段失败即整链中止**（`?` 提前返回），
/// 所以「每个阶段都耗尽自己的 timeout」只可能发生在部分阶段上；
/// 4× 是对「慢但成功」路径的保守上界。
///
/// 有界性：预算由 `timeout_ms` 推导并受 `PROBE_RPC_TIMEOUT` 封顶，
/// `timeout_ms` 又由 Descriptor 与各 Driver parser 双侧限幅，
/// 因此总预算有确定上界。正常连接仍在毫秒级返回，本预算是**上限**而非等待时长。
///
/// NOTE: 各阶段的真实耗时分布仍需真机分阶段量测（A22）；本函数给出的是
/// 由阶段数与 `timeout_ms` 推出的工程上界，不是实测结论。
#[derive(Debug, Clone, Copy)]
struct ProbeBudgets {
    open: std::time::Duration,
    query: std::time::Duration,
    close: std::time::Duration,
}

/// `Probe` RPC 内可能串行等待响应的协议阶段数（见 [`ProbeBudgets`] 文档）。
const PROBE_PROTOCOL_PHASES: u64 = 4;

impl ProbeBudgets {
    fn from_connection_json(connection_json: &str) -> Self {
        let configured_ms = serde_json::from_str::<serde_json::Value>(connection_json)
            .ok()
            .and_then(|v| v.get("timeout_ms").and_then(|t| t.as_u64()))
            .unwrap_or(PROBE_ASSUMED_TIMEOUT_MS);
        // Query（= 协议握手 + 探测查询）：阶段数 × timeout + 3s 余量。
        let query = std::time::Duration::from_millis(
            configured_ms
                .saturating_mul(PROBE_PROTOCOL_PHASES)
                .saturating_add(3_000),
        )
        .min(PROBE_RPC_TIMEOUT)
        .max(std::time::Duration::from_secs(5));
        Self {
            open: PROBE_OPEN_BUDGET,
            query,
            close: PROBE_CLOSE_BUDGET,
        }
    }

    /// 外层总预算 = 各阶段预算之和（不再有“外层比内层短”的截断）。
    fn total(self) -> std::time::Duration {
        DRIVER_STARTUP_TIMEOUT + self.open + self.query + self.close + PROBE_CLEANUP_BUDGET
    }
}

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
    /// 握手失败（带启动阶段）：transport 瞬态（Connect/Hello/Welcome 阶段的
    /// Io/Timeout/Protocol(Io)，如 accept 后对端启动期死亡导致的 reset）由
    /// 调用方 kill 后整个 attempt 重建；语义失败（token/版本/解码）一次判死。
    /// REST 映射仍为 DRIVER_UNAVAILABLE，但 body 自带 stage 可直接定位。
    #[error("handshake failed at {stage}: {message}")]
    Handshake {
        stage: StartupStage,
        message: String,
    },
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
    /// 外层总预算耗尽（含 spawn/handshake/RPC/cleanup）；携带本次预算便于对账。
    #[error("probe timed out after {budget_ms} ms")]
    Timeout { budget_ms: u64 },
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

/// Probe RPC 版本门控（纯函数，可单测）：协商 Minor < 2 的旧 Driver
/// 不识别 ProbeRequest（会静默忽略），必须直接 Unsupported，不得发 RPC 干等。
pub(crate) fn probe_supported(negotiated_minor: u32) -> bool {
    negotiated_minor >= mesa_driver_protocol::PROBE_RPC_MIN_MINOR
}

impl MesaManager {
    /// 动态探测：返回设备事实报告。临时进程生命周期与本调用严格绑定。
    /// 没探测到 ≠ 猜型号：unreachable 时 probe.* 全空，不做任何型号推断。
    pub async fn probe(
        &self,
        driver_id: &str,
        connection_json: &str,
    ) -> Result<ProbeReport, ProbeError> {
        let disc = self
            .find_driver(driver_id)
            .ok_or_else(|| ProbeError::DriverNotFound(driver_id.to_string()))?;
        // 分阶段预算由本次配置的 `timeout_ms` 推导；外层总预算 = 各阶段之和，
        // 内层不再被外层提前截断（C1 §6.3）。
        let budgets = ProbeBudgets::from_connection_json(connection_json);
        let total = budgets.total();
        let report =
            match tokio::time::timeout(total, Self::probe_inner(disc, connection_json, budgets))
                .await
            {
                Ok(r) => r?,
                Err(_) => {
                    return Err(ProbeError::Timeout {
                        budget_ms: total.as_millis().min(u128::from(u64::MAX)) as u64,
                    });
                }
            };
        Ok(report)
    }

    /// 临时探测连接句柄（Core 分配；临时会话内唯一即可，0 有特殊含义禁用）。
    const PROBE_HANDLE: u32 = 999;

    async fn probe_inner(
        disc: crate::manifest::DiscoveredDriver,
        connection_json: &str,
        budgets: ProbeBudgets,
    ) -> Result<ProbeReport, ProbeError> {
        // startup transport 瞬态由原语重建（kill 半活进程 → 新 port → 新 spawn，
        // 最多一次）；语义失败与 RPC 错误原样带出。外层总预算兜底全阶段。
        // 同一 Driver 进程只 accept 一次管理连接（§14.2），重试只能重建。
        crate::temp_op::temp_driver_attempt(&disc, |td| async {
            let r = Self::probe_rpc(&td.session, &disc.manifest.id, connection_json, budgets).await;
            (td, r)
        })
        .await
        .map_err(|e| match e {
            crate::temp_op::TempOpError::Startup(crate::temp_op::TempStartupError::Connect(c)) => {
                ProbeError::Handshake {
                    stage: c.stage,
                    message: c.source.to_string(),
                }
            }
            crate::temp_op::TempOpError::Startup(crate::temp_op::TempStartupError::Spawn {
                message,
                ..
            }) => ProbeError::Spawn(message),
            crate::temp_op::TempOpError::Rpc(e) => e,
        })
    }

    /// OpenConnection → Probe → CloseConnection 的 RPC 段（建连之后）：
    /// 与正常采集走完全相同的建连路径（Secret/PKI/会话），
    /// Configure/Apply/Start 仍禁止。
    async fn probe_rpc(
        session: &Session,
        driver_id: &str,
        connection_json: &str,
        budgets: ProbeBudgets,
    ) -> Result<ProbeReport, ProbeError> {
        if !probe_supported(session.negotiated_minor()) {
            return Err(ProbeError::Unsupported(format!(
                "negotiated minor {} < {}",
                session.negotiated_minor(),
                PROBE_RPC_MIN_MINOR
            )));
        }
        // P0-2 冻结生命周期：OpenConnection → Probe → CloseConnection。
        Self::open_temp(session, driver_id, connection_json, budgets.open).await?;
        let pr = session
            .probe_with_timeout(Self::PROBE_HANDLE, budgets.query)
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
        Self::close_temp(session, budgets.close).await;
        pr
    }

    /// 打开临时探测连接（与正常采集相同的 OpenConnection 路径）。
    async fn open_temp(
        session: &Session,
        driver_id: &str,
        connection_json: &str,
        budget: std::time::Duration,
    ) -> Result<(), ProbeError> {
        use mesa_driver_protocol::pb;
        let reply = session
            .call_with_timeout(
                pb::envelope::Body::OpenConnection(pb::OpenConnection {
                    connection_handle: Self::PROBE_HANDLE,
                    endpoint_id: format!("probe-{driver_id}"),
                    config_json: connection_json.to_string(),
                }),
                budget,
            )
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
    async fn close_temp(session: &Session, budget: std::time::Duration) {
        use mesa_driver_protocol::pb;
        if let Err(e) = session
            .call_with_timeout(
                pb::envelope::Body::CloseConnection(pb::CloseConnection {
                    connection_handle: Self::PROBE_HANDLE,
                }),
                budget,
            )
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

    // -----------------------------------------------------------------------
    // C1 §6.3：Probe 分阶段预算
    // -----------------------------------------------------------------------

    /// 预算随配置的 `timeout_ms` 增长，但始终有界（不会被极大值放大）。
    #[test]
    fn probe_budget_tracks_configured_timeout_and_stays_bounded() {
        let small = ProbeBudgets::from_connection_json(r#"{"timeout_ms":500}"#);
        let default = ProbeBudgets::from_connection_json("{}");
        let large = ProbeBudgets::from_connection_json(r#"{"timeout_ms":10000}"#);
        assert!(
            small.total() <= default.total(),
            "更小的配置超时不应产生更大的总预算"
        );
        assert!(
            default.total() <= large.total(),
            "更大的配置超时应有更大的预算空间"
        );
        // 每个阶段都不得超过单次 RPC 上限。
        for b in [small, default, large] {
            assert!(b.open <= PROBE_RPC_TIMEOUT);
            assert!(b.query <= PROBE_RPC_TIMEOUT);
            assert!(b.close <= PROBE_RPC_TIMEOUT);
            // 外层总预算必然覆盖所有内层阶段之和（不再出现“外层抢先截断”）。
            let stages = DRIVER_STARTUP_TIMEOUT + b.open + b.query + b.close + PROBE_CLEANUP_BUDGET;
            assert_eq!(b.total(), stages);
            assert!(
                b.total() < std::time::Duration::from_secs(90),
                "总预算必须有确定上界，实际 {:?}",
                b.total()
            );
        }
        // 非法/缺失的 timeout_ms 不改变有界性（回落到假定值）。
        let bogus = ProbeBudgets::from_connection_json(r#"{"timeout_ms":"abc"}"#);
        assert_eq!(bogus.total(), default.total());
    }

    /// C1 修正点：协议握手发生在 **Probe RPC** 内，因此 Query 才是承载多阶段
    /// 握手的大预算，Open 只是本地配置解析。
    #[test]
    fn query_budget_carries_the_protocol_handshake_not_open() {
        let b = ProbeBudgets::from_connection_json(r#"{"timeout_ms":5000}"#);
        // Query 必须能覆盖多阶段串行握手（≥ 4 × timeout），而不是 1 × timeout。
        assert!(
            b.query >= std::time::Duration::from_secs(20),
            "Query 必须按阶段数 × timeout 估算，实际 {:?}",
            b.query
        );
        // Open 是本地解析，不应被协议超时放大。
        assert!(
            b.open <= std::time::Duration::from_secs(5),
            "Open 只做配置解析，不应随 timeout_ms 膨胀，实际 {:?}",
            b.open
        );
        assert!(
            b.query > b.open,
            "Query 必须显著大于 Open：握手在 Probe RPC 内"
        );
    }

    /// 用户可配置的最大 `timeout_ms` 必须能被 Query 预算完整承载，
    /// 否则就是「配置合法但永远不生效」。
    #[test]
    fn max_supported_timeout_is_fully_covered_by_query_budget() {
        let b = ProbeBudgets::from_connection_json(&format!(
            r#"{{"timeout_ms":{}}}"#,
            mesa_core_types::CONNECTION_TIMEOUT_MAX_MS
        ));
        let needed = std::time::Duration::from_millis(
            mesa_core_types::CONNECTION_TIMEOUT_MAX_MS * PROBE_PROTOCOL_PHASES,
        );
        assert!(
            b.query >= needed,
            "支持上限 {}ms 的 {PROBE_PROTOCOL_PHASES} 阶段握手需要 {needed:?}，\
             但 Query 只有 {:?}（且必须 ≤ PROBE_RPC_TIMEOUT）",
            mesa_core_types::CONNECTION_TIMEOUT_MAX_MS,
            b.query
        );
    }

    /// 一个「慢但成功」的合法探测不能被旧口径的 10s IPC / 12s 外层提前打断：
    /// 以 timeout_ms=5000 为例，两个各耗时 7s 的响应阶段合计 14s > 12s。
    #[test]
    fn slow_but_successful_multi_phase_probe_fits_the_budget() {
        let b = ProbeBudgets::from_connection_json(r#"{"timeout_ms":5000}"#);
        let slow_two_phase = std::time::Duration::from_secs(14);
        assert!(
            b.query > slow_two_phase,
            "两个 7s 响应阶段的慢路径合计 {slow_two_phase:?}，Query 必须更长，实际 {:?}",
            b.query
        );
        assert!(b.total() > slow_two_phase, "外层总预算也必须覆盖该慢路径");
    }

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
        assert_eq!(
            ProbeError::Timeout { budget_ms: 12_000 }.to_string(),
            "probe timed out after 12000 ms"
        );
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

    /// 建连失败分类（零字符串匹配）：Connect/Hello/Welcome 三阶段的
    /// Io/Timeout/Protocol(Io) 是 startup transport 瞬态 → 可重建；
    /// token/版本/关闭等语义失败一次判死。main CI 的 s7 probe 握手 reset
    /// （Hello 阶段 Protocol(Io)）即第一类，当时无此路由直接 503。
    /// 路由谓词在 [`crate::temp_op`]（三操作共用），此处锁定谓词 + Handshake
    /// 构造（stage 可观测 + 原始 message 透出）。
    #[test]
    fn classify_connect_error_routes_by_stage_and_variant() {
        use crate::session::{ConnectError, SessionError, StartupStage};
        use crate::temp_op::is_transient_connect;
        use mesa_driver_protocol::ProtocolError;

        fn reset_io() -> std::io::Error {
            // 自定义载荷：跨平台断言不依赖 OS 错误文本。
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "test-reset-marker")
        }

        // 瞬态：三阶段 × 三 transport 源 → 全部可重建。
        for stage in [
            StartupStage::Connect,
            StartupStage::Hello,
            StartupStage::Welcome,
        ] {
            for source in [
                SessionError::Io(reset_io()),
                SessionError::Timeout,
                SessionError::Protocol(ProtocolError::Io(reset_io())),
            ] {
                assert!(
                    is_transient_connect(&ConnectError { stage, source }),
                    "stage={stage:?} 必须可重建",
                );
            }
        }
        // 语义：token 不匹配/非法首帧/会话已关一次判死（同阶段也不重试）。
        for source in [
            SessionError::Handshake("token mismatch".into()),
            SessionError::Handshake("first frame must be Hello".into()),
            SessionError::Closed,
        ] {
            assert!(
                !is_transient_connect(&ConnectError {
                    stage: StartupStage::Hello,
                    source,
                }),
                "语义失败必须一次判死",
            );
        }
        // 阶段可观测：Handshake 错误自带 stage + 原始 message，下次 reset 直接定位。
        let err = ProbeError::Handshake {
            stage: StartupStage::Hello,
            message: SessionError::Protocol(ProtocolError::Io(reset_io())).to_string(),
        };
        match err {
            ProbeError::Handshake { stage, message } => {
                assert_eq!(stage, StartupStage::Hello);
                assert!(message.contains("test-reset-marker"), "实际: {message}");
            }
            other => panic!("构造错误，实际: {other:?}"),
        }
    }
}
