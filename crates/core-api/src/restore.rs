//! 开机恢复的统一入口（Issue #102 §9）。
//!
//! # 为什么恢复要单独有一层
//!
//! 修复前 `mesad` 的开机恢复直接调 `MesaManager::start_endpoint()`：它既绕过
//! `LifecycleGate`（因此与并发的 REST Start 构成 W1 类竞态），又不留任何审计。
//! 现场症状正好落在这一点上——"端点被停掉后自己又跑起来"，而平台无法回答
//! "这是开机恢复干的还是有人点了按钮"。
//!
//! 本模块把恢复收成一条路径：审计意图先行 → 生命周期门（与 REST Start 完全同源）
//! → 真实连接由 `endpoint.runtime_epoch` 子记录证明。策略跳过（围栏、无正向凭证、
//! 驱动不允许自动恢复）也写 `SKIPPED` 决策记录：**"为什么没恢复"同样必须可查**，
//! 否则下次现场只能看到一个安静地不动的端点。

use std::sync::Arc;

use super::{AppState, lifecycle_audit};

/// 恢复尝试的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// Manager 已接受启动（真实 RUNNING 由 epoch 子记录证明）。
    Starting,
    /// 构造或提交失败（已写 FAILED 审计）。
    Failed(String),
}

impl RestoreOutcome {
    pub fn is_starting(&self) -> bool {
        matches!(self, RestoreOutcome::Starting)
    }
}

/// 恢复一个端点。
///
/// 审计失败时**跳过该端点**而不是硬失败整个开机流程：一个端点记不下来不该
/// 阻止其它端点恢复（与 `main.rs` 既有的"单端点失败只跳过"语义一致），
/// 但绝不"没审计也照常拉起"。
pub async fn restore_endpoint(state: &Arc<AppState>, endpoint_id: &str) -> RestoreOutcome {
    let audit = state.audit();
    let request_id = lifecycle_audit::new_request_id(lifecycle_audit::OP_RESTORE, endpoint_id);
    // 与 REST Start 同一顺序约束：先取**内存票据**（让并发的 Stop 能看见"有 Start 在途"），
    // 再写审计意图。反序会在"开始处理"与"已登记"之间插入一次 DB 写，制造 Stop 看不见
    // Start 的窗口（W1 竞态）。
    let ticket = state.lifecycle.admit_start(endpoint_id);
    if let Err(e) = audit.started(
        &request_id,
        endpoint_id,
        lifecycle_audit::OP_RESTORE,
        lifecycle_audit::ACTOR_SYSTEM,
        serde_json::json!({
            "action": "restore",
            "endpoint_id": endpoint_id,
            "source": lifecycle_audit::SRC_SYSTEM_RESTORE,
            // 恢复是 Core 内部动作，没有人坐在前面按按钮
            "trigger": "mesad_startup",
        }),
    ) {
        tracing::error!(
            request_id = %request_id, endpoint = %endpoint_id, error = %e,
            "恢复审计意图写入失败：跳过该端点的自动恢复（fail-closed）"
        );
        drop(ticket);
        return RestoreOutcome::Failed(format!("AUDIT_UNAVAILABLE: {e}"));
    }
    // 与 REST Start 同一条两阶段路径：admit → build → commit。
    let cfg = match super::runtime::build_runtime_endpoint(state, endpoint_id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            audit.finish_or_warn(
                &request_id,
                lifecycle_audit::ST_FAILED,
                serde_json::json!({
                    "error_code": e.code,
                    "phase": "build_runtime_endpoint",
                    "source": lifecycle_audit::SRC_SYSTEM_RESTORE,
                }),
            );
            return RestoreOutcome::Failed(e.to_string());
        }
    };
    let ctx = mesa_driver_manager::attempt_audit::AttemptContext::audited(
        request_id.clone(),
        lifecycle_audit::ACTOR_SYSTEM,
        state.audit_sink(),
    );
    match state.lifecycle.commit_start(&ticket, cfg, ctx).await {
        Ok(()) => {
            audit.finish_or_warn(
                &request_id,
                lifecycle_audit::ST_ACCEPTED,
                serde_json::json!({
                    "manager_accepted": true,
                    "state": "accepted_not_running",
                    "source": lifecycle_audit::SRC_SYSTEM_RESTORE,
                }),
            );
            RestoreOutcome::Starting
        }
        Err(e) => {
            audit.finish_or_warn(
                &request_id,
                lifecycle_audit::ST_FAILED,
                serde_json::json!({
                    "error_code": e.code,
                    "message": &e.message,
                    "source": lifecycle_audit::SRC_SYSTEM_RESTORE,
                }),
            );
            RestoreOutcome::Failed(format!("{}: {}", e.code, e.message))
        }
    }
}

/// 记录"策略性跳过"：`SKIPPED` 是决策，不是失败。
///
/// `reason_code` 必须是稳定码（`STOP_INTENT_FENCED`/`INTENT_PROOF_MISSING`/
/// `DRIVER_POLICY_NO_AUTO_RESTORE`/`RECOVERY_DENIED_ALL`），便于事后统计与告警。
pub fn record_skipped(state: &Arc<AppState>, endpoint_id: &str, reason_code: &str, detail: &str) {
    let audit = state.audit();
    let request_id = lifecycle_audit::new_request_id(lifecycle_audit::OP_RESTORE, endpoint_id);
    let request = serde_json::json!({
        "action": "restore",
        "endpoint_id": endpoint_id,
        "source": lifecycle_audit::SRC_SYSTEM_RESTORE,
        "trigger": "mesad_startup",
        "reason_code": reason_code,
    });
    let result = serde_json::json!({"skipped": true, "reason_code": reason_code, "detail": detail});
    if let Err(e) = audit.record_terminal(
        &request_id,
        endpoint_id,
        lifecycle_audit::OP_RESTORE,
        lifecycle_audit::ACTOR_SYSTEM,
        lifecycle_audit::ST_SKIPPED,
        request,
        result,
    ) {
        // 跳过记录写不进去不改变决策本身（端点本来就不恢复），只告警。
        tracing::warn!(
            endpoint = %endpoint_id, reason_code, error = %e,
            "跳过记录的审计写入失败（不改变'不恢复'的决策）"
        );
    }
}
