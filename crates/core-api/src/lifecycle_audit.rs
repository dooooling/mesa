//! 端点生命周期审计（Issue #102）。
//!
//! # 为什么要这一层
//!
//! 现场问题不是"某个 bug 让端点自己跑起来了"，而是**"端点自己跑起来了，但没人能说清是谁
//! 干的"**：Control 写/命令有审计，Start/Stop/Bootstrap/开机恢复一条都没有，HTTP 层也没有
//! 任何请求日志。于是"人工操作""其他 API 客户端""开机恢复""隐藏的第四条路径"四种可能
//! 全部无法排除。
//!
//! 本模块把四件事变成可查事实（复用 §6.6 的 `control_audit` 表，`operation_type='lifecycle'`）：
//! 1. **意图先落盘**：Start/Bootstrap/Restore 在产生任何运行副作用之前先写 `STARTED`；
//! 2. **结果如实记录**：`ACCEPTED`（Manager 接受 ≠ 已 RUNNING）/`COMPLETED`/`FAILED`/`REJECTED`/`SKIPPED`；
//! 3. **epoch 可归因**：每个真实 `stream_epoch` 一条子记录，挂在触发它的那次请求下；
//! 4. **来源可区分**：`http_start`/`http_stop`/`http_bootstrap`/`system_restore`/
//!    `initial_connection`/`internal_reconnect`。
//!
//! # 边界（写进审计的与不写进的）
//!
//! 只写"能回答谁/何时/结果"的元数据：endpoint、操作、来源、状态、错误码、epoch（十进制
//! 字符串，避免 u64 超出 SQLite INTEGER 时精度损失）、客户端 User-Agent/Origin/Referer。
//! **绝不写**请求体、密码、Token、完整连接 JSON、原始幂等键——审计表不该成为第二个密文库。
//!
//! # 当前能力上限（诚实标注）
//!
//! REST 没有可验证的用户身份，`actor` 只能是服务入口（`local-api`）或系统（`system`）。
//! 具体自然人记为 `unknown`：`actor=local-api` 只说明"从本机 API 进来的"，不是身份认证。

use std::sync::Arc;

use axum::http::HeaderMap;
use mesa_config_store::{ConfigStore, ControlAuditRecord, StoreError};
use mesa_driver_manager::attempt_audit::{
    AttemptAuditSink, EpochIntent, EpochOutcome, RuntimeLostEvent,
};

// ---------------------------------------------------------------------------
// 稳定词表（会进入审计 JSON 与查询过滤，改名等于改协议）
// ---------------------------------------------------------------------------

/// `control_audit.operation_type` 取值（v10 起允许）。
pub const OPERATION_TYPE: &str = "lifecycle";

pub const OP_START: &str = "endpoint.start";
pub const OP_STOP: &str = "endpoint.stop";
pub const OP_BOOTSTRAP: &str = "device.bootstrap";
pub const OP_RESTORE: &str = "endpoint.restore";
pub const OP_EPOCH: &str = "endpoint.runtime_epoch";
pub const OP_RUNTIME_LOST: &str = "endpoint.runtime_lost";
/// 首连即失败（与 `endpoint.runtime_lost` 区分：后者必须真的有过一次成功连接）。
pub const OP_INITIAL_CONNECTION_FAILED: &str = "endpoint.initial_connection_failed";

/// 起点来源：谁发起了这次运行态变更。
pub const SRC_HTTP_START: &str = "http_start";
pub const SRC_HTTP_STOP: &str = "http_stop";
pub const SRC_HTTP_BOOTSTRAP: &str = "http_bootstrap";
pub const SRC_SYSTEM_RESTORE: &str = "system_restore";
pub const SRC_INITIAL_CONNECTION: &str = "initial_connection";
/// 首连没成功、还在重试（**从未**成功建连）：与 `internal_reconnect` 语义不同，
/// 后者要求真的连上过再断（评审修正）。
pub const SRC_INITIAL_CONNECTION_RETRY: &str = "initial_connection_retry";
pub const SRC_INTERNAL_RECONNECT: &str = "internal_reconnect";
/// 首连就失败（从未成功建立过连接）：此时不存在"运行中断线"，不能标成重连。
pub const SRC_INITIAL_CONNECTION_FAILED: &str = "initial_connection_failed";
/// 已建立连接之后丢失（随后进入重连）。
pub const SRC_RUNTIME_LOST: &str = "runtime_lost";
/// 证据不足以判定（`last_epoch`/会话证据互相矛盾）：明确标未知，不猜测。
pub const SRC_UNKNOWN: &str = "unknown";

pub const ST_STARTED: &str = "STARTED";
/// Manager 已接受启动（attempt 已登记），**不等于** RUNNING。
pub const ST_ACCEPTED: &str = "ACCEPTED";
pub const ST_COMPLETED: &str = "COMPLETED";
pub const ST_FAILED: &str = "FAILED";
/// 门/校验在产生任何副作用之前拒绝（配置非法、状态冲突、驱动缺失等）。
pub const ST_REJECTED: &str = "REJECTED";
/// 决策跳过（开机恢复被围栏/凭证/产品策略拦下），不是失败。
pub const ST_SKIPPED: &str = "SKIPPED";

pub const ACTOR_LOCAL_API: &str = "local-api";
pub const ACTOR_SYSTEM: &str = "system";
/// 未认证的具体操作者：当前 REST 没有身份校验，只能到这个粒度。
pub const ACTOR_UNKNOWN: &str = "unknown";

/// 生成服务端请求 ID（不信任调用方 `X-Request-ID`）。
///
/// 形状 `<前缀>-<操作>-<主体>-<ns>-<seq>`：时间戳在前保证按字典序基本有序（同一毫秒内
/// 也大致有序），尾部追加**进程内单调序号**保证唯一。
///
/// 为什么必须有那个序号（CI 实测教训）：并发两次 `POST /start` 在同一纳秒内取
/// `now_unix_ns()` 是完全可能的（`w3_double_start_only_one_attempt` 就是这样撞上的），
/// 而 `request_id` 是 `control_audit` 主键——重号会让第二个请求在**写审计**这一步
/// 撞 UNIQUE 约束，于是本该返回 `409 CONFLICT` 的并发 Start 变成了 `503 AUDIT_UNAVAILABLE`。
/// 审计主键唯一性是本设计的前提，不能依赖时钟精度。
pub fn new_request_id(operation_id: &str, subject: &str) -> String {
    format!("lc-{}-{}-{}", operation_id, subject, unique_suffix())
}

/// 唯一后缀 `<ns>-<seq>`：纳秒时间戳 + 进程内单调序号。
///
/// 单独抽出来是因为**所有**审计主键都要唯一：`request_id` 与 `<父请求>:runtime_lost:<后缀>`
/// 都是主键，重号的后果是写审计失败 → 把业务上本该 `409 CONFLICT` 的并发请求变成
/// `503 AUDIT_UNAVAILABLE`（CI 上真实发生过）。
fn unique_suffix() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{}", mesa_core_types::now_unix_ns(), seq)
}

/// 提取**允许入审计**的客户端元数据。
///
/// 只取 User-Agent / Origin / Referer 与调用方自带 request id（仅作关联线索，不作为
/// 本系统的请求 ID）。刻意不读 body、不读 cookie、不读 Authorization。
///
/// **脱敏（评审要求）**：`Referer` 只保留 origin + path，**丢掉 query 与 fragment**
/// ——前端路由常把令牌、工单号、筛选条件放在查询串里，它们没有诊断价值却有泄露风险。
pub fn client_meta(headers: &HeaderMap) -> serde_json::Value {
    fn field(headers: &HeaderMap, name: &str) -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            // 截断：Header 是不可信输入，审计行不该被单个超长头撑爆
            .map(|s| s.chars().take(256).collect::<String>())
    }
    serde_json::json!({
        "user_agent": field(headers, "user-agent"),
        "origin": field(headers, "origin"),
        "referer": field(headers, "referer").map(|r| strip_url_sensitive(&r)),
        "client_request_id": field(headers, "x-request-id"),
    })
}

/// 去掉 URL 的 query 与 fragment（保留 scheme/host/path）。
///
/// 用最小字符串处理而不是引入 `url` 依赖：这里只做"砍掉敏感部分"，不需要解析语义；
/// 解析失败也不影响结果——任何情况下 query/fragment 都不会被留下。
pub fn strip_url_sensitive(raw: &str) -> String {
    let cut = raw.find(['?', '#']).map(|i| &raw[..i]).unwrap_or(raw);
    cut.trim().to_string()
}

/// 生命周期审计写入器：把 Core 的副作用事实落到 `control_audit`。
///
/// 只持有 `Arc<ConfigStore>`（克隆廉价）：审计是"顺手记录"，不该引入新的状态机。
#[derive(Clone)]
pub struct LifecycleAudit {
    store: Arc<ConfigStore>,
}

impl LifecycleAudit {
    pub fn new(store: Arc<ConfigStore>) -> Self {
        Self { store }
    }

    /// 写意图（`STARTED`）。**必须在任何运行副作用之前调用**；返回 `Err` 表示"记不下来"，
    /// 调用方对 Start/Bootstrap 必须据此拒绝执行（fail-closed）。
    pub fn started(
        &self,
        request_id: &str,
        endpoint_id: &str,
        operation_id: &str,
        actor: &str,
        request: serde_json::Value,
    ) -> Result<(), StoreError> {
        let rec = ControlAuditRecord {
            request_id: request_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            actor: actor.to_string(),
            operation_type: OPERATION_TYPE.to_string(),
            operation_id: operation_id.to_string(),
            request_json: request.to_string(),
            result_json: None,
            status: ST_STARTED.to_string(),
            started_at_ns: mesa_core_types::now_unix_ns(),
            finished_at_ns: None,
        };
        self.store.insert_control_audit(&rec)
    }

    /// 写结论。用于**主操作已经结束**之后：失败只告警（`finish_or_warn`），
    /// 因为此时改主操作结果只会让事实更糟。
    pub fn finish(
        &self,
        request_id: &str,
        status: &str,
        result: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.store.update_control_audit(
            request_id,
            status,
            Some(&result.to_string()),
            mesa_core_types::now_unix_ns(),
        )
    }

    /// 一次性写入终态记录（没有后续状态更新）。
    ///
    /// 用于"决策型"记录（`SKIPPED`）与"已经发生的事实"（`runtime_lost`）：它们没有
    /// STARTED → 终态的两阶段语义，硬拆成两次写只会制造一堆"永远停在 STARTED"的行。
    #[allow(clippy::too_many_arguments)]
    pub fn record_terminal(
        &self,
        request_id: &str,
        endpoint_id: &str,
        operation_id: &str,
        actor: &str,
        status: &str,
        request: serde_json::Value,
        result: serde_json::Value,
    ) -> Result<(), StoreError> {
        let now = mesa_core_types::now_unix_ns();
        let rec = ControlAuditRecord {
            request_id: request_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            actor: actor.to_string(),
            operation_type: OPERATION_TYPE.to_string(),
            operation_id: operation_id.to_string(),
            request_json: request.to_string(),
            result_json: Some(result.to_string()),
            status: status.to_string(),
            started_at_ns: now,
            finished_at_ns: Some(now),
        };
        self.store.insert_control_audit(&rec)
    }

    /// 结论写入失败只告警：保留已持久化的 `STARTED`（查询侧能识别为"未完成"），
    /// 绝不伪造成功。
    pub fn finish_or_warn(&self, request_id: &str, status: &str, result: serde_json::Value) {
        if let Err(e) = self.finish(request_id, status, result) {
            tracing::warn!(
                request_id = %request_id, status, error = %e,
                "生命周期审计结论写入失败（保留 STARTED 记录，主操作结果不受影响）"
            );
        }
    }
}

impl AttemptAuditSink for LifecycleAudit {
    /// epoch 意图：`<parent>:epoch:<n>` 一条记录，挂在触发本次运行的那次请求下。
    fn record_epoch_intent(&self, intent: &EpochIntent) -> Result<(), String> {
        let request = serde_json::json!({
            "parent_request_id": intent.parent_request_id,
            "source": intent.source.as_str(),
            "connection_sequence": intent.connection_sequence,
            // u64 一律转十进制字符串：SQLite INTEGER 是有符号 64 位，
            // JSON 数字在部分客户端还会退化成 f64（>2^53 精度丢失）。
            "stream_epoch": intent.stream_epoch.to_string(),
            "generation": intent.generation,
            "endpoint_id": intent.endpoint_id,
        });
        self.started(
            &intent.request_id,
            &intent.endpoint_id,
            OP_EPOCH,
            &intent.actor,
            request,
        )
        .map_err(|e| e.to_string())
    }

    fn record_epoch_outcome(&self, request_id: &str, outcome: &EpochOutcome) -> Result<(), String> {
        let (status, result) = match outcome {
            EpochOutcome::Completed { point_count } => (
                ST_COMPLETED,
                serde_json::json!({"state": "RUNNING", "point_count": point_count}),
            ),
            // 只落稳定分类与错误码；原始错误串留在日志（评审要求：审计不做日志库）。
            EpochOutcome::Failed { category, code } => (
                ST_FAILED,
                serde_json::json!({"state": "FAILED", "category": category, "code": code}),
            ),
        };
        self.finish(request_id, status, result)
            .map_err(|e| e.to_string())
    }

    /// 连接丢失：写一条轻量记录，复用父请求前缀（`<parent>:<kind>:<后缀>`）以便按前缀聚合。
    ///
    /// 来源**按证据判定**（评审修正），不再一律写 `internal_reconnect`：
    /// - `last_epoch == 0 && !had_running_session` → `initial_connection_failed`
    ///   （从未成功建连，谈不上"断线"）；
    /// - 其余有成功连接迹象 → `runtime_lost`；
    /// - 证据互相矛盾（如 `last_epoch==0` 却 `had_running_session=true`）→ `unknown`，不猜。
    fn record_runtime_lost(&self, event: &RuntimeLostEvent) -> Result<(), String> {
        let (operation_id, source) = classify_loss(event);
        let kind = if source == SRC_INITIAL_CONNECTION_FAILED {
            "initial_connection_failed"
        } else if source == SRC_RUNTIME_LOST {
            "runtime_lost"
        } else {
            "lost_unknown"
        };
        let request_id = match &event.parent_request_id {
            Some(parent) => format!("{parent}:{kind}:{}", unique_suffix()),
            None => new_request_id(operation_id, &event.endpoint_id),
        };
        let request = serde_json::json!({
            "parent_request_id": event.parent_request_id,
            "generation": event.generation,
            "last_stream_epoch": event.last_epoch.to_string(),
            "had_running_session": event.had_running_session,
            "source": source,
            "endpoint_id": event.endpoint_id,
        });
        // 一步写成终态：这是"已经发生的事实"，没有后续状态可更新。
        // `reason_code` 是白名单稳定码；原始原因串只进日志。
        let rec = ControlAuditRecord {
            request_id: request_id.clone(),
            endpoint_id: event.endpoint_id.clone(),
            actor: ACTOR_SYSTEM.to_string(),
            operation_type: OPERATION_TYPE.to_string(),
            operation_id: operation_id.to_string(),
            request_json: request.to_string(),
            result_json: Some(
                serde_json::json!({"category": event.reason_code, "source": source}).to_string(),
            ),
            status: ST_FAILED.to_string(),
            started_at_ns: mesa_core_types::now_unix_ns(),
            finished_at_ns: Some(mesa_core_types::now_unix_ns()),
        };
        self.store
            .insert_control_audit(&rec)
            .map_err(|e| e.to_string())
    }
}

/// 按证据把一次连接丢失归类成 `(operation_id, source)`。
///
/// 判定只用两个可观测事实：`last_epoch`（是否真的成功建立过 epoch）与
/// `had_running_session`（本次 attempt 是否曾经跑起来过）。二者矛盾时不猜，标 `unknown`
/// —— 审计宁可说"不知道"，也不能编一个看起来合理的来源。
fn classify_loss(event: &RuntimeLostEvent) -> (&'static str, &'static str) {
    match (event.last_epoch, event.had_running_session) {
        (0, false) => (OP_INITIAL_CONNECTION_FAILED, SRC_INITIAL_CONNECTION_FAILED),
        (0, true) => (OP_RUNTIME_LOST, SRC_UNKNOWN),
        (_, false) => (OP_RUNTIME_LOST, SRC_UNKNOWN),
        (_, true) => (OP_RUNTIME_LOST, SRC_RUNTIME_LOST),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_driver_manager::attempt_audit::EpochSource;

    fn audit() -> (LifecycleAudit, Arc<ConfigStore>) {
        let store = Arc::new(ConfigStore::open_in_memory().unwrap());
        (LifecycleAudit::new(Arc::clone(&store)), store)
    }

    #[test]
    fn started_then_finish_round_trip() {
        let (a, store) = audit();
        let rid = new_request_id(OP_START, "ep1");
        a.started(
            &rid,
            "ep1",
            OP_START,
            ACTOR_LOCAL_API,
            serde_json::json!({"source": SRC_HTTP_START, "action": "start"}),
        )
        .unwrap();
        let rec = store.get_control_audit(&rid).unwrap().unwrap();
        assert_eq!(rec.operation_type, OPERATION_TYPE);
        assert_eq!(rec.operation_id, OP_START);
        assert_eq!(rec.status, ST_STARTED);
        assert_eq!(rec.finished_at_ns, None);

        a.finish(
            &rid,
            ST_ACCEPTED,
            serde_json::json!({"manager_accepted": true}),
        )
        .unwrap();
        let rec = store.get_control_audit(&rid).unwrap().unwrap();
        assert_eq!(rec.status, ST_ACCEPTED);
        assert!(rec.finished_at_ns.is_some());
    }

    #[test]
    fn epoch_records_hang_under_parent_and_keep_u64_epoch_as_string() {
        let (a, store) = audit();
        let parent = "lc-endpoint.start-ep1-1";
        // 超过 i64::MAX 的 epoch：必须无损保存
        let big = u64::MAX - 7;
        let intent = EpochIntent {
            request_id: format!("{parent}:epoch:2"),
            parent_request_id: parent.into(),
            endpoint_id: "ep1".into(),
            source: EpochSource::InternalReconnect,
            connection_sequence: 2,
            stream_epoch: big,
            actor: ACTOR_LOCAL_API.into(),
            generation: 9,
        };
        a.record_epoch_intent(&intent).unwrap();
        let rec = store
            .get_control_audit(&intent.request_id)
            .unwrap()
            .unwrap();
        assert_eq!(rec.operation_id, OP_EPOCH);
        let req: serde_json::Value = serde_json::from_str(&rec.request_json).unwrap();
        assert_eq!(req["stream_epoch"], serde_json::json!(big.to_string()));
        assert_eq!(req["parent_request_id"], serde_json::json!(parent));
        assert_eq!(req["source"], serde_json::json!(SRC_INTERNAL_RECONNECT));

        a.record_epoch_outcome(
            &intent.request_id,
            &EpochOutcome::Completed { point_count: 240 },
        )
        .unwrap();
        let rec = store
            .get_control_audit(&intent.request_id)
            .unwrap()
            .unwrap();
        assert_eq!(rec.status, ST_COMPLETED);
        let res: serde_json::Value =
            serde_json::from_str(rec.result_json.as_deref().unwrap()).unwrap();
        assert_eq!(res["state"], serde_json::json!("RUNNING"));
        assert_eq!(res["point_count"], serde_json::json!(240));
    }

    #[test]
    fn epoch_outcome_on_missing_row_is_error_not_silent_success() {
        let (a, _store) = audit();
        // 父记录不存在（例如被人工清理）：更新必须报错，而不是静默"成功"
        assert!(
            a.record_epoch_outcome("nope:epoch:1", &EpochOutcome::Completed { point_count: 1 })
                .is_err()
        );
    }

    #[test]
    fn runtime_lost_records_previous_epoch_and_reason() {
        let (a, store) = audit();
        a.record_runtime_lost(&RuntimeLostEvent {
            parent_request_id: Some("lc-endpoint.start-ep1-1".into()),
            endpoint_id: "ep1".into(),
            generation: 3,
            last_epoch: 42,
            reason_code: "reader_eof",
            had_running_session: true,
        })
        .unwrap();
        let rows = store
            .list_control_audit(
                Some("ep1"),
                None,
                Some(OPERATION_TYPE),
                None,
                None,
                10,
                None,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].operation_id, OP_RUNTIME_LOST);
        assert_eq!(rows[0].status, ST_FAILED);
        let req: serde_json::Value = serde_json::from_str(&rows[0].request_json).unwrap();
        assert_eq!(req["last_stream_epoch"], serde_json::json!("42"));
        assert_eq!(req["source"], serde_json::json!(SRC_RUNTIME_LOST));
        // 持久审计只留白名单稳定码
        let res: serde_json::Value =
            serde_json::from_str(rows[0].result_json.as_deref().unwrap()).unwrap();
        assert_eq!(res["category"], serde_json::json!("reader_eof"));
        // 子记录 ID 复用父请求前缀，时间线可按前缀聚合同一次启动
        assert!(
            rows[0]
                .request_id
                .starts_with("lc-endpoint.start-ep1-1:runtime_lost:")
        );
    }

    /// 评审修正：首连就失败（从未成功建连）不得被标成"运行中断线重连"。
    #[test]
    fn loss_before_any_successful_connection_is_initial_connection_failed() {
        let (a, store) = audit();
        a.record_runtime_lost(&RuntimeLostEvent {
            parent_request_id: Some("lc-endpoint.start-ep2-1".into()),
            endpoint_id: "ep2".into(),
            generation: 1,
            last_epoch: 0,
            reason_code: "connect_failed",
            had_running_session: false,
        })
        .unwrap();
        let rows = store
            .list_control_audit(
                Some("ep2"),
                None,
                Some(OPERATION_TYPE),
                None,
                None,
                10,
                None,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].operation_id, OP_INITIAL_CONNECTION_FAILED);
        let req: serde_json::Value = serde_json::from_str(&rows[0].request_json).unwrap();
        assert_eq!(
            req["source"],
            serde_json::json!(SRC_INITIAL_CONNECTION_FAILED)
        );
        assert!(
            rows[0]
                .request_id
                .starts_with("lc-endpoint.start-ep2-1:initial_connection_failed:")
        );
    }

    /// 证据矛盾时明确标 unknown，不猜一个看起来合理的来源。
    #[test]
    fn contradictory_loss_evidence_is_marked_unknown() {
        let (a, store) = audit();
        a.record_runtime_lost(&RuntimeLostEvent {
            parent_request_id: Some("lc-endpoint.start-ep3-1".into()),
            endpoint_id: "ep3".into(),
            generation: 1,
            last_epoch: 0,
            reason_code: "reader_eof",
            // 没成功建过 epoch，却声称跑过会话：矛盾
            had_running_session: true,
        })
        .unwrap();
        let rows = store
            .list_control_audit(
                Some("ep3"),
                None,
                Some(OPERATION_TYPE),
                None,
                None,
                10,
                None,
            )
            .unwrap();
        let req: serde_json::Value = serde_json::from_str(&rows[0].request_json).unwrap();
        assert_eq!(req["source"], serde_json::json!(SRC_UNKNOWN));
        assert_ne!(rows[0].operation_id, OP_INITIAL_CONNECTION_FAILED);
    }

    /// 评审要求：epoch 失败结论只落稳定分类/错误码，不落原始错误串。
    #[test]
    fn epoch_failure_persists_only_stable_category_and_code() {
        let (a, store) = audit();
        let rid = "lc-endpoint.start-ep4-1:epoch:1";
        a.record_epoch_intent(&EpochIntent {
            request_id: rid.into(),
            parent_request_id: "lc-endpoint.start-ep4-1".into(),
            endpoint_id: "ep4".into(),
            source: EpochSource::InitialConnection,
            connection_sequence: 1,
            stream_epoch: 7,
            actor: ACTOR_LOCAL_API.into(),
            generation: 1,
        })
        .unwrap();
        a.record_epoch_outcome(
            rid,
            &EpochOutcome::Failed {
                category: "configuration_failed".into(),
                code: Some("ConfigurationError/BAD_CONFIG".into()),
            },
        )
        .unwrap();
        let rec = store.get_control_audit(rid).unwrap().unwrap();
        let res = rec.result_json.unwrap();
        assert!(res.contains("configuration_failed"));
        assert!(res.contains("ConfigurationError/BAD_CONFIG"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&res).unwrap()["state"],
            serde_json::json!("FAILED")
        );
    }

    /// 评审要求：Referer 的 query/fragment 不得进审计；凭据头一律不读。
    #[test]
    fn client_meta_strips_referer_query_and_fragment() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "referer",
            "http://127.0.0.1:8132/endpoints?token=SECRET&x=1#frag"
                .parse()
                .unwrap(),
        );
        headers.insert("origin", "http://127.0.0.1:8132".parse().unwrap());
        headers.insert("cookie", "session=SECRET".parse().unwrap());
        let meta = client_meta(&headers);
        assert_eq!(
            meta["referer"],
            serde_json::json!("http://127.0.0.1:8132/endpoints")
        );
        let s = meta.to_string();
        assert!(!s.contains("SECRET"), "审计不得包含 query/凭据: {s}");
        assert!(!s.contains("frag"), "审计不得包含 fragment: {s}");
        assert!(!s.contains("cookie"), "审计不得读取 cookie: {s}");
    }

    #[test]
    fn client_meta_keeps_only_safe_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "curl/8.0".parse().unwrap());
        headers.insert("authorization", "Bearer SECRET".parse().unwrap());
        let meta = client_meta(&headers);
        assert_eq!(meta["user_agent"], serde_json::json!("curl/8.0"));
        let s = meta.to_string();
        assert!(!s.contains("SECRET"), "审计不得包含凭据: {s}");
    }

    /// 请求 ID 是 `control_audit` 的主键，必须**无条件**唯一。
    ///
    /// 回归背景：CI 上两个并发 `POST /start` 在同一纳秒内取时间戳，生成同一个
    /// `request_id`，第二个请求在写审计时撞 UNIQUE 约束 → 本该 `409 CONFLICT`
    /// 的并发 Start 变成 `503 AUDIT_UNAVAILABLE`（`w3_double_start_only_one_attempt`）。
    #[test]
    fn request_ids_are_unique_under_concurrency() {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..500)
                        .map(|_| new_request_id(OP_START, "ep1"))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        for h in handles {
            for id in h.join().unwrap() {
                assert!(seen.insert(id.clone()), "request_id 重复: {id}");
            }
        }
        assert_eq!(seen.len(), 8 * 500);
    }
}
