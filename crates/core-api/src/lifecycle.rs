//! G7 生命周期门：Start 两阶段提交 + Stop 合流 + 停止判据观察器。
//!
//! 分层（设计 §6.1，**唯一真值**原则）：
//! - `driver-manager` 的 attempt 表 = "有没有 attempt、到哪一步"的唯一真值；
//! - 本模块的 [`LifecycleGate`] = **排序器与票据池**，只持有 `stop_gen`、
//!   在途 Start 票据数、`desired_running` 缓存与写入序号，**不持有**任何
//!   生命周期事实（没有 Stop 在途计数器——那是 P0-A 的原始漏洞）。
//!
//! 锁序纪律：**gate 锁 → manager 锁**，永不反向。manager 的任何路径都不回调
//! gate（结构上保证），故无死锁。`commit_start` / `admit_stop` 都在这条锁序下
//! 完成"代次比对 + 生命周期调用"。
//!
//! 取消语义（设计 §6.7）：**所有状态迁移都在同步函数内完成，所有 `await`
//! 只用于"等待结果"**。因此客户端断开/超时只丢失应答，不丢状态：
//! - Start 构造期被取消 ⇒ 票据 RAII 释放，不触碰 desired/attempt；
//! - Stop 被取消 ⇒ `desired=false` 已在受理时写入，teardown 继续跑，
//!   条目由 attempt 任务自己移除 ⇒ 保护不会提前释放。
//!
//! S1（评审追加的硬约束）：**Stop 的持久化尝试不得串在 teardown 之前阻塞它**。
//! `admit_stop` 先同步 `begin_stop`（拆解立即开始），再把 `set_desired_running`
//! 丢到 `spawn_blocking`——DB busy/失败都不会推迟停止动作，落盘结果事后反映到
//! 响应码（失败 ⇒ 500 `STOP_INTENT_NOT_PERSISTED`）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::http::StatusCode;
use mesa_config_store::ConfigStore;
use mesa_driver_manager::{MesaManager, StopHandle};

/// 内部快速收敛窗口（设计 §7.3）：250ms × 2 个采样 = 500ms。
///
/// **只用于 Core 内部状态收敛判定**（测试断言/日志/诊断），**不是**现场
/// "停干净"判据，也不能替代现场 10s 连续稳定观察（§7.5）。
pub const SETTLE_TICKS: u32 = 2;
pub const SETTLE_TICK: Duration = Duration::from_millis(250);

/// Stop 意图落盘失败的**冻结响应体**（评审给定，逐字采用）。
pub const STOP_INTENT_NOT_PERSISTED_MESSAGE: &str =
    "停止已执行，但停止意图未能持久化；禁止重新启动，需人工核验";

/// 生命周期错误（结构化：状态码 + 可机器识别 code + 文案）。
#[derive(Debug, Clone)]
pub struct LifecycleError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl LifecycleError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn to_response(&self) -> (StatusCode, Json<serde_json::Value>) {
        (
            self.status,
            Json(crate::json_error(self.code, &self.message)),
        )
    }
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}: {}", self.status.as_u16(), self.code, self.message)
    }
}

/// 每 endpoint 的门状态（设计 §6.2 `EpGate`）。
#[derive(Debug, Default)]
struct EpGate {
    /// Stop 受理即 +1：作废所有"在 Stop 之前 admit、之后 commit"的 Start 票据（W1）。
    stop_gen: u64,
    /// 已 admit、未 commit/未放弃的 Start 票据数（判据 3 / 6）。
    inflight_starts: u32,
    /// `desired_running` 的内存权威（懒初始化自 DB；此后由门写入）。
    desired: Option<bool>,
    /// 每次**成功**写入 +1（证明"读到的不是旧值"）。
    desired_seq: u64,
    /// 最近一次 `admit_stop` 时的 `desired_seq`（判据 2 的比对基准）。
    last_stop_seq: Option<u64>,
    /// Stop 意图落盘失败 ⇒ 禁止 Start，直到补写成功（§6.10.3）。
    intent_unpersisted: bool,
}

/// Start 票据（RAII）：取消只减计数——此时尚未产生任何外部副作用，安全。
///
/// 注意：票据**不能**承载 Stop 在途保护（那必须归属于 attempt 条目），
/// 否则客户端一断线保护就没了（设计 §6.3）。
pub struct StartTicket {
    gate: Arc<LifecycleGate>,
    id: String,
    stop_gen: u64,
    released: bool,
}

impl StartTicket {
    /// 票据对应的 `stop_gen`（`commit_start` 比对用）。
    pub fn stop_gen(&self) -> u64 {
        self.stop_gen
    }
}

impl Drop for StartTicket {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut m = self.gate.inner.lock().unwrap();
        if let Some(g) = m.get_mut(&self.id) {
            g.inflight_starts = g.inflight_starts.saturating_sub(1);
        }
    }
}

/// Stop 受理结果：拆解**已经开始**，`persist` 是并行的落盘尝试。
pub struct StopAdmission {
    /// 合流句柄（多个 Stop 等同一个结论）。
    pub handle: StopHandle,
    /// 落盘尝试（`spawn_blocking`，不阻塞 teardown）。`Err` ⇒ 意图未持久化。
    pub persist: tokio::task::JoinHandle<Result<(), String>>,
}

/// 一次采样：6 条判据的**全部来源**（设计 §7.3）。
#[derive(Debug, Clone)]
pub struct SettleSample {
    pub snapshot_gen: Option<u64>,
    pub snapshot_state: Option<String>,
    pub current_gen: Option<u64>,
    pub entry_absent: bool,
    pub desired: Option<bool>,
    pub desired_seq: u64,
    pub intent_unpersisted: bool,
    pub inflight_starts: u32,
}

/// 收敛判定结果。
#[derive(Debug, Clone)]
pub enum Settled {
    Settled { generation: u64 },
    NotSettled { reason: String, sample: SettleSample },
}

/// 生命周期门。
pub struct LifecycleGate {
    store: Arc<ConfigStore>,
    manager: Arc<MesaManager>,
    inner: Mutex<HashMap<String, EpGate>>,
}

impl LifecycleGate {
    pub fn new(store: Arc<ConfigStore>, manager: Arc<MesaManager>) -> Self {
        Self {
            store,
            manager,
            inner: Mutex::new(HashMap::new()),
        }
    }

    // -----------------------------------------------------------------
    // Start：admit（同步）→ 构造（长 await，在 handler 里）→ commit（同步）
    // -----------------------------------------------------------------

    /// 准入（同步）：只登记票据，**不查 is_running、不碰 DB、不起进程**。
    pub fn admit_start(self: &Arc<Self>, endpoint_id: &str) -> StartTicket {
        let mut m = self.inner.lock().unwrap();
        let g = m.entry(endpoint_id.to_string()).or_default();
        g.inflight_starts += 1;
        StartTicket {
            gate: Arc::clone(self),
            id: endpoint_id.to_string(),
            stop_gen: g.stop_gen,
            released: false,
        }
    }

    /// 提交（同步，单临界区）：代次比对 → `start_endpoint` → 落盘 true。
    ///
    /// 为什么必须同步：全程无 `await` ⇒ 客户端取消**无法**落在"代次比对通过、
    /// 但 start 尚未发生"之间。禁止在临界区内调 `build_runtime_endpoint`
    /// （它 await 且可能起临时进程）。
    pub fn commit_start(
        &self,
        ticket: &StartTicket,
        cfg: mesa_driver_manager::endpoint::BuiltinEndpoint,
    ) -> Result<(), LifecycleError> {
        let mut m = self.inner.lock().unwrap();
        let g = m.entry(ticket.id.clone()).or_default();

        // 步骤 0：Stop 意图未落盘 ⇒ 一律拒绝（503），直到补写成功。
        if g.intent_unpersisted {
            return Err(LifecycleError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "INTENT_UNPERSISTED",
                format!(
                    "endpoint `{}`: 停止意图未能持久化，禁止重新启动；请先重新执行 Stop 以补写意图",
                    ticket.id
                ),
            ));
        }
        // 步骤 1：票据代次与当前不符 ⇒ Stop 已受理，在途 Start 必须失败（契约行 1 / W1）。
        if g.stop_gen != ticket.stop_gen {
            return Err(LifecycleError::new(
                StatusCode::CONFLICT,
                "SUPERSEDED_BY_STOP",
                format!(
                    "endpoint `{}`: Stop 已受理，本次启动被取消（stop_gen {} -> {}）",
                    ticket.id, ticket.stop_gen, g.stop_gen
                ),
            ));
        }
        // 步骤 2：同步启动（内部 insert-before-spawn；冲突由 attempt 表给出）。
        if let Err(msg) = self.manager.start_endpoint(cfg) {
            return Err(map_start_err(&ticket.id, msg));
        }
        // 步骤 3：落盘 true（attempt 已在跑，失败无法回滚 ⇒ 显式 500）。
        match self.store.set_desired_running(&ticket.id, true) {
            Ok(true) | Ok(false) => {
                g.desired = Some(true);
                g.desired_seq += 1;
                g.intent_unpersisted = false;
                // 人工确认语义（S2）：显式 Start 成功即视为"已确认"，清除围栏，
                // 否则该端点下次开机仍会被拒绝自动恢复。
                if let Err(e) = self.store.clear_stop_intent_unpersisted(&ticket.id) {
                    tracing::error!(
                        endpoint = %ticket.id, error = %e,
                        "清除停止意图围栏失败（下次开机仍会拒绝自动恢复，属安全方向）"
                    );
                }
                Ok(())
            }
            Err(e) => {
                tracing::error!(
                    endpoint = %ticket.id, error = %e,
                    "START intent not persisted: attempt already running, cannot roll back"
                );
                Err(LifecycleError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "START_INTENT_NOT_PERSISTED",
                    format!(
                        "端点已启动，但运行意图未能持久化（{}）；重启后不会自动恢复该端点",
                        e
                    ),
                ))
            }
        }
    }

    // -----------------------------------------------------------------
    // Stop：admit（同步：代次 +1 + 置假 + 认领 attempt）→ 落盘（并行）
    // -----------------------------------------------------------------

    /// 受理 Stop（同步）：作废在途 Start、**先启动拆解**、再并行尝试落盘。
    ///
    /// 顺序不可交换（S1）：若先落盘再拆解，DB busy 会把实际停止推迟数秒；
    /// 停止是安全方向，优先级高于持久化。
    pub fn admit_stop(self: &Arc<Self>, endpoint_id: &str) -> StopAdmission {
        let (handle, seq_at_admit, stop_gen) = {
            let mut m = self.inner.lock().unwrap();
            let g = m.entry(endpoint_id.to_string()).or_default();
            // 1) 代次 +1：先作废所有在途 Start 票据（W1 的唯一判据）。
            g.stop_gen += 1;
            // 2) 内存意图立即置假（§5.3 前移）；desired=false + state!=STOPPED
            //    在拆解期是**合法组合**，判据侧不得当作矛盾。
            g.desired = Some(false);
            g.last_stop_seq = Some(g.desired_seq);
            // 3) 停止优先：同步认领并启动 teardown（无 await、无 DB）。
            let h = self.manager.begin_stop(endpoint_id);
            (h, g.desired_seq, g.stop_gen)
        };
        let store = Arc::clone(&self.store);
        let gate = Arc::clone(self);
        let id = endpoint_id.to_string();
        // 4) 落盘尝试与 teardown **并行**：spawn_blocking 不占 tokio worker，
        //    因此 DB busy 不会拖住拆解任务。
        let persist = tokio::task::spawn_blocking(move || {
            let res = store.set_desired_running(&id, false).map_err(|e| e.to_string());
            gate.finish_stop_persist(&id, seq_at_admit, stop_gen, res)
        });
        StopAdmission { handle, persist }
    }

    /// 落盘结果归并（在 blocking 线程内调用）。
    ///
    /// 失败语义（§6.10.3）：置 `intent_unpersisted`（禁止新 Start）+ 写持久化
    /// 围栏（S2：崩溃后重启也不得自动拉起该端点）+ 返回 Err 让响应变 500。
    fn finish_stop_persist(
        &self,
        endpoint_id: &str,
        seq_at_admit: u64,
        stop_gen: u64,
        res: Result<bool, String>,
    ) -> Result<(), String> {
        match res {
            Ok(_) => {
                let mut m = self.inner.lock().unwrap();
                let g = m.entry(endpoint_id.to_string()).or_default();
                // 只有"最新一次 Stop"的落盘结果才能推进门状态：更早的 Stop 若
                // 晚到成功，不得覆盖更新一次 Stop 已写入的事实。
                if g.stop_gen == stop_gen {
                    g.desired = Some(false);
                    g.desired_seq = g.desired_seq.max(seq_at_admit + 1);
                    g.intent_unpersisted = false;
                    drop(m);
                    if let Err(e) = self.store.clear_stop_intent_unpersisted(endpoint_id) {
                        tracing::error!(
                            endpoint = %endpoint_id, error = %e,
                            "清除停止意图围栏失败（下次开机仍会拒绝自动恢复，属安全方向）"
                        );
                    }
                }
                Ok(())
            }
            Err(e) => {
                {
                    let mut m = self.inner.lock().unwrap();
                    let g = m.entry(endpoint_id.to_string()).or_default();
                    g.intent_unpersisted = true;
                }
                // S2：持久化"意图未持久化"围栏。即使 Core 随后崩溃，重启也不会
                // 依据旧的 desired_running=true 自动拉起用户明确要停的端点。
                if let Err(fence_err) = self.store.mark_stop_intent_unpersisted(endpoint_id) {
                    // 围栏写失败是**最坏**情况：无法保证重启失败关闭。
                    tracing::error!(
                        endpoint = %endpoint_id, error = %fence_err,
                        "CRITICAL: 停止意图围栏写入失败；重启后该端点可能被自动拉起，需人工核验"
                    );
                }
                tracing::error!(
                    endpoint = %endpoint_id, error = %e,
                    "STOP intent not persisted; teardown proceeds, new Start blocked (503)"
                );
                Err(e)
            }
        }
    }

    // -----------------------------------------------------------------
    // 判据观察器（只读，§7.3）
    // -----------------------------------------------------------------

    /// 一次跨来源采样。只读：不写 `desired_running`、不 cancel/restart attempt、
    /// 不改快照。一旦它开始"纠正"，就变成 §10-1 明确拒绝的对账器。
    pub fn sample(&self, endpoint_id: &str) -> SettleSample {
        let (desired, desired_seq, intent_unpersisted, inflight_starts) = {
            let mut m = self.inner.lock().unwrap();
            let g = m.entry(endpoint_id.to_string()).or_default();
            if g.desired.is_none() {
                // 懒初始化自 DB（判据 2 的持久化面）。
                g.desired = self
                    .store
                    .get_endpoint(endpoint_id)
                    .ok()
                    .flatten()
                    .map(|r| r.desired_running);
            }
            (
                g.desired,
                g.desired_seq,
                g.intent_unpersisted,
                g.inflight_starts,
            )
        };
        let (snapshot_gen, snapshot_state) = self
            .manager
            .snapshot()
            .endpoint_with_gen(endpoint_id)
            .map_or((None, None), |(g, s)| (Some(g), Some(s.state)));
        SettleSample {
            snapshot_gen,
            snapshot_state,
            current_gen: self.manager.current_gen(endpoint_id),
            entry_absent: !self.manager.has_live_attempt(endpoint_id),
            desired,
            desired_seq,
            intent_unpersisted,
            inflight_starts,
        }
    }

    /// 稳定窗口判定（设计 §7.4）。`timeout` 内连续 [`SETTLE_TICKS`] 个 tick 全过才算收敛。
    ///
    /// `current_gen == None` ⇒ `NotSettled{NoGenBaseline}`（**无证据 ≠ 证据成立**）。
    pub async fn await_settled(&self, endpoint_id: &str, timeout: Duration) -> Settled {
        let seq_at_admit = {
            let m = self.inner.lock().unwrap();
            m.get(endpoint_id).and_then(|g| g.last_stop_seq)
        };
        let deadline = Instant::now() + timeout;
        let mut ok_ticks = 0u32;
        let mut last_gen: Option<u64> = None;
        let mut last_sample;
        loop {
            let s = self.sample(endpoint_id);
            last_sample = s.clone();
            let Some(cur) = s.current_gen else {
                return Settled::NotSettled {
                    reason: "NoGenBaseline: 无代次基准，不能据此声称已停干净".into(),
                    sample: s,
                };
            };
            let same_gen = last_gen.is_none_or(|g| g == cur);
            let pass = s.snapshot_gen == Some(cur)
                && s.snapshot_state.as_deref() == Some("STOPPED")
                && s.entry_absent
                && s.desired == Some(false)
                && s.desired_seq >= seq_at_admit.unwrap_or(0)
                && !s.intent_unpersisted
                && s.inflight_starts == 0;
            ok_ticks = if pass && same_gen { ok_ticks + 1 } else { 0 };
            last_gen = Some(cur);
            if ok_ticks >= SETTLE_TICKS {
                return Settled::Settled { generation: cur };
            }
            if Instant::now() >= deadline {
                return Settled::NotSettled {
                    reason: format!(
                        "窗口内未连续 {} 个 tick 全过（snapshot_gen={:?} current_gen={:?} \
                         state={:?} entry_absent={} desired={:?} seq={} intent_unpersisted={} inflight={}）",
                        SETTLE_TICKS,
                        last_sample.snapshot_gen,
                        last_sample.current_gen,
                        last_sample.snapshot_state,
                        last_sample.entry_absent,
                        last_sample.desired,
                        last_sample.desired_seq,
                        last_sample.intent_unpersisted,
                        last_sample.inflight_starts
                    ),
                    sample: last_sample,
                };
            }
            tokio::time::sleep(SETTLE_TICK).await;
        }
    }

    /// 只读查询：该 endpoint 是否处于"Stop 意图未持久化"。
    pub fn intent_unpersisted(&self, endpoint_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .get(endpoint_id)
            .is_some_and(|g| g.intent_unpersisted)
    }

    /// 只读查询：内存期望态（懒初始化自 DB）。
    pub fn desired(&self, endpoint_id: &str) -> Option<bool> {
        self.sample(endpoint_id).desired
    }
}

/// manager 的 `Err(String)`（精确码前缀约定）→ 结构化生命周期错误。
fn map_start_err(endpoint_id: &str, msg: String) -> LifecycleError {
    if msg.starts_with("ATTEMPT_FAILED_UNKNOWN") {
        return LifecycleError::new(StatusCode::CONFLICT, "ATTEMPT_FAILED_UNKNOWN", msg);
    }
    if msg.starts_with("STOP_IN_PROGRESS") {
        return LifecycleError::new(StatusCode::CONFLICT, "STOP_IN_PROGRESS", msg);
    }
    if msg.contains("already running") {
        // 沿用既有文案语义（前端只读 message，不匹配 code）。
        return LifecycleError::new(
            StatusCode::CONFLICT,
            "CONFLICT",
            format!("endpoint `{endpoint_id}` 已在运行"),
        );
    }
    // 其余（EVENT_STORE_UNAVAILABLE / driver not found 等）原样透传为 400。
    LifecycleError::new(StatusCode::BAD_REQUEST, "START_FAILED", msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_driver_manager::endpoint::BuiltinEndpoint;

    fn temp_drivers(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mesa-gate-{tag}-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn gate(tag: &str) -> Arc<LifecycleGate> {
        let store = Arc::new(ConfigStore::open_in_memory().unwrap());
        store
            .create_device(&mesa_config_store::DeviceRecord {
                id: "d1".into(),
                name: "d".into(),
            })
            .unwrap();
        store
            .create_endpoint(&mesa_config_store::EndpointRecord {
                id: "ep-1".into(),
                name: "ep".into(),
                device_id: "d1".into(),
                driver_id: "test-driver".into(),
                connection_json: "{}".into(),
                desired_running: false,
                updated_at_ns: 0,
            })
            .unwrap();
        let mgr = Arc::new(MesaManager::discover(&temp_drivers(tag)));
        Arc::new(LifecycleGate::new(store, mgr))
    }

    fn dummy_cfg(id: &str) -> BuiltinEndpoint {
        BuiltinEndpoint {
            endpoint_id: id.into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            tasks: vec![],
            event_tasks: vec![],
        }
    }

    /// 契约行 1 / W1：在 Stop 受理前 admit、之后 commit 的 Start 必须失败，
    /// 且**不得产生任何 attempt**、不得把 desired 写回 true。
    #[tokio::test]
    async fn in_flight_start_superseded_by_stop_admission() {
        let g = gate("superseded");
        let ticket = g.admit_start("ep-1");
        assert_eq!(ticket.stop_gen(), 0);
        // Stop 受理：stop_gen +1 + 置假 + begin_stop（此时无 attempt ⇒ NotRunning）。
        let adm = g.admit_stop("ep-1");
        assert!(!adm.handle.was_running(), "此时还没有 attempt");
        let persisted = adm.persist.await.unwrap();
        assert!(persisted.is_ok(), "落盘应成功: {persisted:?}");

        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .expect_err("Stop 已受理，在途 Start 必须失败");
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, "SUPERSEDED_BY_STOP");
        assert!(!g.manager.has_live_attempt("ep-1"), "不得产生 attempt");
        assert_eq!(g.desired("ep-1"), Some(false), "desired 必须仍为假");
    }

    /// P0-3 的 fail-closed 面：没有代次基准（从未有 attempt / 刚重启）时，
    /// `await_settled` **不得**判为 settled（无证据 ≠ 证据成立）。
    #[tokio::test]
    async fn settled_without_generation_baseline_is_not_settled() {
        let g = gate("nobaseline");
        let r = g
            .await_settled("ep-1", Duration::from_millis(300))
            .await;
        match r {
            Settled::NotSettled { reason, sample } => {
                assert!(reason.contains("NoGenBaseline"), "{reason}");
                assert_eq!(sample.current_gen, None);
            }
            other => panic!("无基准时不得判为 settled: {other:?}"),
        }
    }

    /// §6.10.3：Stop 意图落盘失败 ⇒ 置 `intent_unpersisted` + 写持久化围栏，
    /// 并且**禁止**新 Start（503 `INTENT_UNPERSISTED`）；补写成功后恢复。
    #[tokio::test]
    async fn stop_intent_persist_failure_blocks_start_until_rewritten() {
        let g = gate("intent");
        // 直接驱动落盘归并（等价于 admit_stop 的 spawn_blocking 返回失败）。
        // `stop_gen` 必须与门当前值一致（生产里 admit_stop 先 +1 再落盘）。
        let r = g.finish_stop_persist("ep-1", 0, 0, Err("disk full".into()));
        assert!(r.is_err());
        assert!(g.intent_unpersisted("ep-1"));
        assert_eq!(
            g.store.stop_intent_unpersisted().unwrap(),
            vec!["ep-1".to_string()],
            "必须写持久化围栏（S2）"
        );

        let ticket = g.admit_start("ep-1");
        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .expect_err("意图未持久化期间必须拒绝 Start");
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "INTENT_UNPERSISTED");

        // 补写成功 ⇒ 清标记 + 清围栏。
        g.finish_stop_persist("ep-1", 0, 0, Ok(true)).unwrap();
        assert!(!g.intent_unpersisted("ep-1"));
        assert!(g.store.stop_intent_unpersisted().unwrap().is_empty());
    }

    /// 合流语义的纯逻辑面：第二次 Stop 不重复 cancel/teardown，只等同一个结论。
    /// （真正的并发 Stop 端到端在 `lifecycle_race.rs`。）
    #[tokio::test]
    async fn second_stop_joins_instead_of_failing() {
        let g = gate("join");
        let a = g.admit_stop("ep-1");
        let b = g.admit_stop("ep-1");
        assert!(!a.handle.was_running());
        assert!(!b.handle.was_running());
        assert_eq!(a.handle.await_result().await, Ok(false));
        assert_eq!(b.handle.await_result().await, Ok(false));
        assert_eq!(a.persist.await.unwrap(), Ok(()));
        assert_eq!(b.persist.await.unwrap(), Ok(()));
    }
}
