//! G7 生命周期门：Start 两阶段提交 + Stop 合流 + 停止判据观察器。
//!
//! 分层（设计 §6.1，**唯一真值**原则）：
//! - `driver-manager` 的 attempt 表 = "有没有 attempt、到哪一步"的唯一真值；
//! - 本模块的 [`LifecycleGate`] = **排序器与票据池**，持有 `stop_gen`、
//!   在途 Start 票据数、**全部在途 Stop 落盘代次集合**（P0-A）、
//!   `desired_running` 缓存与写入序号；"有没有 attempt、到哪一步"仍只由
//!   `driver-manager` 回答。
//!
//! 锁序纪律：**persist_lock → gate 锁 → manager 锁**，永不反向。manager 的任何
//! 路径都不回调 gate（结构上保证）；`commit_start` / `admit_stop` 的同步临界区
//! 都只在这条锁序下完成"代次比对 + 生命周期调用"，且**临界区内没有 DB 写**。
//!
//! 取消语义（设计 §6.7）：状态迁移仍在同步临界区内完成；`await` 只用于取得
//! 落盘串行器或等待落盘结果。客户端断开/超时只丢失应答，不丢状态：
//! - Start 构造期被取消 ⇒ 票据 RAII 释放，不触碰 desired/attempt；
//! - Start 落盘期被取消 ⇒ 落盘任务持有串行器并自行完成门状态归并；
//! - Stop 被取消 ⇒ `desired=false` 已在受理时写入，teardown 继续跑，
//!   条目由 attempt 任务自己移除 ⇒ 保护不会提前释放。
//!
//! S1/P0-B（评审追加的硬约束）：**Stop 的持久化尝试不得串在 teardown 之前
//! 阻塞它，Start 的慢落盘也不得阻塞 Stop 的 `begin_stop`**。
//! `admit_stop` 先同步 `begin_stop`（拆解立即开始），再在独立任务里等待
//! per-endpoint 落盘串行器并 `spawn_blocking` 写库；Start 的 `true` 落盘同样
//! 在门锁之外，只持有串行器——DB busy 不会推迟停止动作，落盘结果事后反映到
//! 响应码（失败 ⇒ 500 `STOP_INTENT_NOT_PERSISTED`）。

use std::collections::{BTreeSet, HashMap};
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
        write!(
            f,
            "{}/{}: {}",
            self.status.as_u16(),
            self.code,
            self.message
        )
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
    /// 每次**成功**写入 +1（诊断用；证明"读到的不是旧值"）。
    desired_seq: u64,
    /// 最近一次**已确认落盘**的 Stop 的 `stop_gen`（R-2 收敛判据）。
    ///
    /// 只有 `finish_stop_persist(Ok)` 且代次仍是最新时才推进。修复前用的是
    /// "admit 时的 `desired_seq`"，那个值在落盘开始**之前**就已满足 ⇒ 慢写
    /// 条件下 `await_settled` 会在 DB 仍为旧值时误判成功。
    stop_persist_confirmed_gen: Option<u64>,
    /// **全部**正在落盘、尚未确认的 Stop 代次（P0-A 启动闸门）。
    ///
    /// 为什么是集合而不是"最新代次"：Stop-A 受理后写库被阻塞，Stop-B 受理并
    /// 先完成（旧实现只存最新代次，B 一完成就把标记清空），随后新 Start 写入
    /// `desired=true`；Stop-A 的迟到 SQL 完成后又写 `desired=false` ⇒ 实际
    /// RUNNING、DB 却是 false。只忽略 Stop-A 的**结果**救不了它已经发生的写入，
    /// 因此必须跟踪所有在途代次：只要集合非空，新 Start 一律不得提交。
    stop_persist_inflight: BTreeSet<u64>,
    /// Stop 意图落盘失败 ⇒ 禁止 Start，直到补写成功（§6.10.3）。
    intent_unpersisted: bool,
    /// 每 endpoint 的运行意图落盘串行器（P0-A/P0-B）。
    ///
    /// 作用（两个方向都必须）：
    /// 1. Start 的 `desired=true` 与随后受理的 Stop 的 `desired=false` 不会反序
    ///    ——Stop 的落盘必须排在它受理时已经开始的 Start 落盘之后；
    /// 2. Start 落盘期间不持有生命周期门锁（P0-B），因此 Stop 的 `begin_stop`
    ///    不会被慢 DB 拖住。
    ///
    /// 锁序：`persist_lock → gate`（Start/Stop 的落盘任务）；门内**不等待**
    /// 这把锁（`commit_start` 在门外 await 取得它），故不会与 `gate → manager`
    /// 形成环。
    persist_lock: Arc<tokio::sync::Mutex<()>>,
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
    /// 落盘尝试（独立任务：先等串行器，再 `spawn_blocking` 写库；不阻塞
    /// teardown）。`Err` ⇒ 意图未持久化。
    pub persist: tokio::task::JoinHandle<Result<(), String>>,
    /// 本次受理的 Stop 代次（落盘任务异常终止时用于归还门状态）。
    pub stop_gen: u64,
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
    /// 当前最新 Stop 代次（`admit_stop` 每受理一次 +1）。
    pub stop_gen: u64,
    /// 已确认落盘的 Stop 代次（R-2：收敛必须以此为准，不能只看内存意图）。
    pub stop_persist_confirmed_gen: Option<u64>,
    /// 仍在落盘中的 Stop 代次（取集合中最大者；`None` = 集合为空）。
    ///
    /// 兼容既有判据的形态：`is_none()` 等价于"没有任何在途 Stop 落盘"。
    pub stop_persist_inflight: Option<u64>,
    /// 仍在落盘中的 Stop 总数（P0-A：用于证明跟踪的是**全部**代次，
    /// 而不只是最新一个）。`stop_persist_inflight.is_none()` ⇔ 本值为 0。
    pub stop_persist_inflight_count: usize,
    pub intent_unpersisted: bool,
    pub inflight_starts: u32,
}

/// 收敛判定结果。
#[derive(Debug, Clone)]
pub enum Settled {
    Settled {
        generation: u64,
    },
    NotSettled {
        reason: String,
        sample: SettleSample,
    },
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

    /// 提交（两段式，P0-B）：先在不持门锁的等待中取得 per-endpoint 落盘
    /// 串行器，再在同步临界区内完成"代次比对 + `start_endpoint`"，最后在
    /// **临界区外**落盘 `true`。
    ///
    /// 为什么不能像旧实现那样在门锁内同步写库：Stop 的 `begin_stop` 也要取
    /// 同一把门锁；DB 慢数秒时 Stop 的 cancel 会被迫等待（P0-B）。拆开后仍保证：
    /// - Start 的意图落盘**先于**响应返回（这里 await 落盘结果）；
    /// - Start 落盘期间受理的 Stop，其 `false` 落盘排在 `true` 之后（串行器）；
    /// - 任何在途 Stop 落盘未确认时，新 Start 不得提交（P0-A 集合闸门）。
    ///
    /// 同步临界区仍然**无 await**：`start_endpoint` 与"代次比对"不可分割，
    /// 客户端取消无法落在两者之间。唯一新增的 await 在取得串行器与等待落盘
    /// 结果处，二者都不改变门状态。
    pub async fn commit_start(
        self: &Arc<Self>,
        ticket: &StartTicket,
        cfg: mesa_driver_manager::endpoint::BuiltinEndpoint,
    ) -> Result<(), LifecycleError> {
        // 快路径：已有活 attempt 时不需要等落盘串行器——`start_endpoint` 会给出
        // 既有的 STOP_IN_PROGRESS / already running / ATTEMPT_FAILED_UNKNOWN。
        let persist_lock = {
            let mut m = self.inner.lock().unwrap();
            let g = m.entry(ticket.id.clone()).or_default();
            Self::check_start_preconditions(g, &ticket.id, ticket.stop_gen)?;
            // P0-A：集合里**任何**一个在途 Stop 落盘都禁止新 Start。只在拆解
            // 已经结束（无活 attempt）时报本码；拆解仍在进行时让
            // `start_endpoint` 给出既有的 `STOP_IN_PROGRESS`（契约行 2）。
            if !g.stop_persist_inflight.is_empty() && !self.manager.has_live_attempt(&ticket.id) {
                return Err(stop_persist_in_progress(&ticket.id));
            }
            if self.manager.has_live_attempt(&ticket.id) {
                return self
                    .manager
                    .start_endpoint(cfg)
                    .map_err(|msg| map_start_err(&ticket.id, msg));
            }
            Arc::clone(&g.persist_lock)
        };

        // 串行器在门外 await：Stop 受理（gate 锁）绝不会被 Start 的慢写拖住。
        // 若等锁期间有新 Stop 受理，下面的二次判定会拒绝或交给 start_endpoint。
        let guard = persist_lock.lock_owned().await;

        {
            let mut m = self.inner.lock().unwrap();
            let g = m.entry(ticket.id.clone()).or_default();
            Self::check_start_preconditions(g, &ticket.id, ticket.stop_gen)?;
            if !g.stop_persist_inflight.is_empty() {
                // 等锁期间有 Stop 受理：拆解已开始 ⇒ 保持契约码 STOP_IN_PROGRESS；
                // 拆解已结束但落盘未确认 ⇒ STOP_PERSIST_IN_PROGRESS。
                if self.manager.has_live_attempt(&ticket.id) {
                    return self
                        .manager
                        .start_endpoint(cfg)
                        .map_err(|msg| map_start_err(&ticket.id, msg));
                }
                return Err(stop_persist_in_progress(&ticket.id));
            }
            // 同步启动（内部 insert-before-spawn；冲突由 attempt 表给出）。
            if let Err(msg) = self.manager.start_endpoint(cfg) {
                return Err(map_start_err(&ticket.id, msg));
            }
        }

        // 临界区外落盘 true（P0-B）。串行器 guard 被 move 进 blocking 任务：
        // 即使 handler 被取消，落盘与门状态归并仍会完成（只丢应答，不丢状态）。
        let gate = Arc::clone(self);
        let store = Arc::clone(&self.store);
        let id = ticket.id.clone();
        let start_gen = ticket.stop_gen;
        let rt = tokio::runtime::Handle::current();
        match tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let write = store
                .set_desired_running(&id, true)
                .map_err(|e| e.to_string());
            gate.finish_start_persist(&id, start_gen, write, &rt)
        })
        .await
        {
            Ok(res) => res,
            Err(join) => {
                tracing::error!(
                    endpoint = %ticket.id, error = %join,
                    "START persist blocking task failed; intent state unknown"
                );
                Err(LifecycleError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "START_INTENT_NOT_PERSISTED",
                    format!(
                        "端点已启动，但运行意图落盘任务异常终止（{}）；重启后是否自动恢复需人工核验",
                        join
                    ),
                ))
            }
        }
    }

    /// Start 的两条共同前置判定（快速判定与取得串行器后的二次判定同源）。
    fn check_start_preconditions(
        g: &EpGate,
        endpoint_id: &str,
        ticket_gen: u64,
    ) -> Result<(), LifecycleError> {
        // 步骤 0：Stop 意图未落盘 ⇒ 一律拒绝（503），直到补写成功。
        if g.intent_unpersisted {
            return Err(LifecycleError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "INTENT_UNPERSISTED",
                format!(
                    "endpoint `{endpoint_id}`: 停止意图未能持久化，禁止重新启动；请先重新执行 Stop 以补写意图"
                ),
            ));
        }
        // 步骤 1：票据代次与当前不符 ⇒ Stop 已受理，在途 Start 必须失败（契约行 1 / W1）。
        if g.stop_gen != ticket_gen {
            return Err(LifecycleError::new(
                StatusCode::CONFLICT,
                "SUPERSEDED_BY_STOP",
                format!(
                    "endpoint `{endpoint_id}`: Stop 已受理，本次启动被取消（stop_gen {ticket_gen} -> {}）",
                    g.stop_gen
                ),
            ));
        }
        Ok(())
    }

    /// Start 意图落盘归并（在 blocking 线程内调用；调用方持 `persist_lock`）。
    ///
    /// 语义：
    /// - `Ok(true)`：DB 已写 true。只有代次仍匹配才把内存意图推进为 true；
    ///   若落盘期间已有更新的 Stop 受理，内存意图保持 Stop 的 false，由 Stop
    ///   自己的 false 落盘收尾（串行器保证它排在本写入之后）。
    /// - `Ok(false)`：UPDATE 命中 0 行 ⇒ 端点已被并发删除；attempt 已在跑，
    ///   必须认领停止（P0-7），返回 409 `ENDPOINT_DELETED`。
    /// - `Err`：attempt 已在跑、无法回滚；返回 500 `START_INTENT_NOT_PERSISTED`。
    pub(crate) fn finish_start_persist(
        &self,
        endpoint_id: &str,
        start_gen: u64,
        res: Result<bool, String>,
        rt: &tokio::runtime::Handle,
    ) -> Result<(), LifecycleError> {
        match res {
            Ok(true) => {
                let superseded = {
                    let mut m = self.inner.lock().unwrap();
                    let g = m.entry(endpoint_id.to_string()).or_default();
                    if g.stop_gen == start_gen {
                        g.desired = Some(true);
                        g.desired_seq += 1;
                        g.intent_unpersisted = false;
                        false
                    } else {
                        true
                    }
                };
                if superseded {
                    tracing::warn!(
                        endpoint = %endpoint_id, start_gen,
                        "Start intent persisted after a newer Stop was admitted; \
                         memory intent stays false (Stop's own persist is ordered after this write)"
                    );
                } else if let Err(e) = self.store.clear_stop_intent_unpersisted(endpoint_id) {
                    // 人工确认语义（S2）：显式 Start 成功即视为"已确认"，清除围栏，
                    // 否则该端点下次开机仍会被拒绝自动恢复。
                    tracing::error!(
                        endpoint = %endpoint_id, error = %e,
                        "清除停止意图围栏失败（下次开机仍会拒绝自动恢复，属安全方向）"
                    );
                }
                Ok(())
            }
            // P0-7：`Ok(false)` = UPDATE 命中 0 行 ⇒ 该端点已被并发删除，而 attempt
            // 已经启动。若不处理就是"配置已删、驱动仍在跑"的孤儿。
            Ok(false) => {
                let handle = self.manager.begin_stop(endpoint_id);
                if handle.was_running() {
                    rt.spawn(async move {
                        let _ = handle.await_result().await;
                    });
                }
                Err(LifecycleError::new(
                    StatusCode::CONFLICT,
                    "ENDPOINT_DELETED",
                    format!(
                        "endpoint `{endpoint_id}`: 该端点已被删除，启动已中止（已启动的 attempt 已认领停止）"
                    ),
                ))
            }
            Err(e) => {
                tracing::error!(
                    endpoint = %endpoint_id, error = %e,
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
    // Delete：作废在途 Start（P0-7）
    // -----------------------------------------------------------------

    /// DELETE 前调用：`stop_gen + 1` 作废所有在途 Start 票据。
    ///
    /// 为什么必须在这里 bump：DELETE 的 `has_live_attempt` 检查与真正删除之间，
    /// 一个已 admit 的 Start 仍可能提交并拉起驱动 ⇒ "删除的端点继续启动"。
    /// 复用 Stop 的同一把锁 + 同一代次机制，提交侧无需新增分支。
    ///
    /// 不重置 `stop_gen`：端点若被同名重建，新票据拿到的是新代次，旧票据永远
    /// 匹配不上（与墓碑语义一致）。
    pub fn invalidate_starts_for_delete(&self, endpoint_id: &str) {
        let mut m = self.inner.lock().unwrap();
        let g = m.entry(endpoint_id.to_string()).or_default();
        g.stop_gen += 1;
        g.stop_persist_inflight.clear();
        g.stop_persist_confirmed_gen = None;
        // 端点即将不存在：旧的"意图未持久化"阻塞不再适用于这个 id。
        g.intent_unpersisted = false;
        g.desired = None;
    }

    // -----------------------------------------------------------------
    // Stop：admit（同步：代次 +1 + 置假 + 认领 attempt）→ 落盘（并行）
    // -----------------------------------------------------------------

    /// 受理 Stop（同步）：作废在途 Start、**先启动拆解**、再并行尝试落盘。
    ///
    /// 顺序不可交换（S1）：若先落盘再拆解，DB busy 会把实际停止推迟数秒；
    /// 停止是安全方向，优先级高于持久化。
    pub fn admit_stop(self: &Arc<Self>, endpoint_id: &str) -> StopAdmission {
        let (handle, stop_gen, persist_lock) = {
            let mut m = self.inner.lock().unwrap();
            let g = m.entry(endpoint_id.to_string()).or_default();
            // 1) 代次 +1：先作废所有在途 Start 票据（W1 的唯一判据）。
            g.stop_gen += 1;
            let stop_generation = g.stop_gen;
            // 2) 内存意图立即置假（§5.3 前移）；desired=false + state!=STOPPED
            //    在拆解期是**合法组合**，判据侧不得当作矛盾。
            g.desired = Some(false);
            // 3) P0-A：把**本次**代次加入在途集合（不是覆盖最新代次）。集合非空
            //    期间新 Start 一律不得提交；只有每次落盘各自归并后才能移除。
            g.stop_persist_inflight.insert(stop_generation);
            // R-2：新 Stop 使上一次的"已确认"失效——本次停止的落盘还没确认，
            // 收敛判据必须等这一次，而不是复用上一次的凭证。
            g.stop_persist_confirmed_gen = None;
            // 4) 停止优先：同步认领并启动 teardown（无 await、无 DB）。
            let h = self.manager.begin_stop(endpoint_id);
            (h, stop_generation, Arc::clone(&g.persist_lock))
        };
        let store = Arc::clone(&self.store);
        let gate = Arc::clone(self);
        let id = endpoint_id.to_string();
        // 5) 落盘尝试与 teardown **并行**：先 await 串行器（不占 tokio worker，
        //    也不阻塞 admit_stop），再用 spawn_blocking 做真正的 DB 写。
        //
        //    串行器保证：Start 的 true 与随后 Stop 的 false 不会反序；多个 Stop
        //    之间也按受理顺序落盘，因此旧代次的迟到写不会越过新代次。
        let persist = tokio::spawn(async move {
            let _guard = persist_lock.lock_owned().await;
            let write_id = id.clone();
            let res = tokio::task::spawn_blocking(move || {
                store
                    .set_desired_running(&write_id, false)
                    .map_err(|e| e.to_string())
            })
            .await;
            match res {
                Ok(r) => gate.finish_stop_persist(&id, stop_gen, r),
                Err(join) => gate.finish_stop_persist(
                    &id,
                    stop_gen,
                    Err(format!("persist task failed: {join}")),
                ),
            }
        });
        StopAdmission {
            handle,
            persist,
            stop_gen,
        }
    }

    /// 落盘结果归并（在 blocking 线程内调用）。
    ///
    /// 失败语义（§6.10.3）：置 `intent_unpersisted`（禁止新 Start）+ 写持久化
    /// 围栏（S2：崩溃后重启也不得自动拉起该端点）+ 返回 Err 让响应变 500。
    pub(crate) fn finish_stop_persist(
        &self,
        endpoint_id: &str,
        stop_gen: u64,
        res: Result<bool, String>,
    ) -> Result<(), String> {
        match res {
            Ok(_) => {
                let latest = {
                    let mut m = self.inner.lock().unwrap();
                    let g = m.entry(endpoint_id.to_string()).or_default();
                    // P0-A：无论是否最新代次，本次落盘都已结束，必须把自己的
                    // 代次从在途集合移除；否则 Start 闸门会被一次已完成（或已
                    // 失败）的落盘永久锁死。
                    g.stop_persist_inflight.remove(&stop_gen);
                    // 只有"最新一次 Stop"的落盘结果才能推进门状态：更早的 Stop 若
                    // 晚到成功，不得覆盖更新一次 Stop 已写入的事实。
                    let latest = g.stop_gen == stop_gen;
                    if latest {
                        g.desired = Some(false);
                        g.desired_seq += 1;
                        // R-2：本次停止意图落盘已确认——这是收敛判据的唯一凭证。
                        g.stop_persist_confirmed_gen = Some(stop_gen);
                        g.intent_unpersisted = false;
                    }
                    latest
                };
                if latest {
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
                let latest = {
                    let mut m = self.inner.lock().unwrap();
                    let g = m.entry(endpoint_id.to_string()).or_default();
                    // 失败也必须归还本代次：否则一次陈旧失败会永久禁止 Start。
                    g.stop_persist_inflight.remove(&stop_gen);
                    let latest = g.stop_gen == stop_gen;
                    if latest {
                        g.stop_persist_confirmed_gen = None;
                        g.intent_unpersisted = true;
                    }
                    latest
                };
                if !latest {
                    // 更晚的 Stop 已受理：它的落盘结果才是权威，旧代次的失败不得
                    // 把门锁死在"意图未持久化"（否则一次陈旧失败会永久禁止 Start）。
                    tracing::warn!(
                        endpoint = %endpoint_id, stop_gen, error = %e,
                        "stale STOP persist failed after a newer Stop was admitted; ignored"
                    );
                    return Err(e);
                }
                // S2/P0-3：持久化"意图未持久化"围栏。即使 Core 随后崩溃，重启也不会
                // 依据旧的 desired_running=true 自动拉起用户明确要停的端点。
                // 围栏有 DB + sidecar 两条通道（DB 整体不可写时仍可跨崩溃）。
                match self.store.mark_stop_intent_unpersisted(endpoint_id) {
                    Ok(report) if report.durable() => {
                        tracing::error!(
                            endpoint = %endpoint_id,
                            db_channel = report.db,
                            file_channel = report.file,
                            "STOP intent not persisted; 持久化围栏已写入（重启将拒绝自动恢复）"
                        );
                    }
                    Ok(_) => {
                        tracing::error!(
                            endpoint = %endpoint_id,
                            "CRITICAL: 停止意图围栏两条通道均未留下持久事实"
                        );
                    }
                    Err(fence_err) => {
                        // 围栏写失败是**最坏**情况：无法保证重启失败关闭。
                        tracing::error!(
                            endpoint = %endpoint_id, error = %fence_err,
                            "CRITICAL: 停止意图围栏写入失败；重启后该端点可能被自动拉起，需人工核验"
                        );
                    }
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
        let (
            desired,
            desired_seq,
            stop_gen,
            confirmed,
            inflight_latest,
            inflight_count,
            intent_unpersisted,
            inflight_starts,
        ) = {
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
                g.stop_gen,
                g.stop_persist_confirmed_gen,
                g.stop_persist_inflight.iter().next_back().copied(),
                g.stop_persist_inflight.len(),
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
            stop_gen,
            stop_persist_confirmed_gen: confirmed,
            stop_persist_inflight: inflight_latest,
            stop_persist_inflight_count: inflight_count,
            intent_unpersisted,
            inflight_starts,
        }
    }

    /// 稳定窗口判定（设计 §7.4）。`timeout` 内连续 [`SETTLE_TICKS`] 个 tick 全过才算收敛。
    ///
    /// `current_gen == None` ⇒ `NotSettled{NoGenBaseline}`（**无证据 ≠ 证据成立**）。
    ///
    /// R-2：收敛**必须**以"本次 Stop 的 `desired=false` 落盘已确认"为前提。
    /// 只看内存意图（`desired == Some(false)`）会在慢写条件下提前返回成功——
    /// 此时 DB 仍是旧值 `true`，重启会把这个用户明确要停的端点自动拉起来。
    pub async fn await_settled(&self, endpoint_id: &str, timeout: Duration) -> Settled {
        // 以"进入判定时的最新 Stop 代次"为目标：只认这一次的落盘确认。
        let target_gen = {
            let m = self.inner.lock().unwrap();
            m.get(endpoint_id).map_or(0, |g| g.stop_gen)
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
            // R-2：本次停止意图落盘已确认，且没有更新的 Stop 在途。
            let intent_confirmed = target_gen > 0
                && s.stop_gen == target_gen
                && s.stop_persist_confirmed_gen == Some(target_gen)
                && s.stop_persist_inflight.is_none();
            let same_gen = last_gen.is_none_or(|g| g == cur);
            let pass = s.snapshot_gen == Some(cur)
                && s.snapshot_state.as_deref() == Some("STOPPED")
                && s.entry_absent
                && s.desired == Some(false)
                && intent_confirmed
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
                         state={:?} entry_absent={} desired={:?} stop_gen={} \
                         confirmed_gen={:?} persist_inflight={:?} intent_unpersisted={} inflight={}）",
                        SETTLE_TICKS,
                        last_sample.snapshot_gen,
                        last_sample.current_gen,
                        last_sample.snapshot_state,
                        last_sample.entry_absent,
                        last_sample.desired,
                        last_sample.stop_gen,
                        last_sample.stop_persist_confirmed_gen,
                        last_sample.stop_persist_inflight,
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

/// P0-A 的统一拒绝码：**至少有一个** Stop 落盘尚未确认（不是只看最新代次）。
fn stop_persist_in_progress(endpoint_id: &str) -> LifecycleError {
    LifecycleError::new(
        StatusCode::CONFLICT,
        "STOP_PERSIST_IN_PROGRESS",
        format!(
            "endpoint `{endpoint_id}`: 上一次 Stop 的停止意图仍在落盘，尚未确认；请稍后重试 Start"
        ),
    )
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
            .await
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
        let r = g.await_settled("ep-1", Duration::from_millis(300)).await;
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
        let r = g.finish_stop_persist("ep-1", 0, Err("disk full".into()));
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
            .await
            .expect_err("意图未持久化期间必须拒绝 Start");
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "INTENT_UNPERSISTED");

        // 补写成功 ⇒ 清标记 + 清围栏。
        g.finish_stop_persist("ep-1", 0, Ok(true)).unwrap();
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
