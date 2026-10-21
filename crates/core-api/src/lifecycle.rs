//! G7 生命周期门：Start 两阶段提交 + Stop 合流 + 停止判据观察器。
//!
//! 分层（设计 §6.1，**唯一真值**原则）：
//! - `driver-manager` 的 attempt 表 = "有没有 attempt、到哪一步"的唯一真值；
//! - 本模块的 [`LifecycleGate`] = **排序器与票据池**，持有 `stop_gen`、
//!   在途 Start 票据数、**全部在途 Stop 落盘代次集合**（P0-A）、
//!   **在途 Start 落盘计数与 DELETE 屏障**（P0-D）、
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

/// DELETE 等待在途意图写入收敛的预算（P0-D）。
///
/// 取值依据：Stop 的 `desired=false` 落盘在 DB busy 场景下是毫秒到秒级（验收用的
/// 慢写注入是 2.5–9s），10s 覆盖最坏预算且不会让正常删除变慢——**正常路径下
/// 两个计数器都是 0，`await_intent_quiescent` 第一次观测就返回，无任何 sleep**。
/// 超时不是"硬删"，而是 409 拒绝，让客户端重试（删除不可逆，等不到就别做）。
pub const DELETE_PERSIST_WAIT: Duration = Duration::from_secs(10);

/// 上述等待的采样间隔（只在确实有在途写入时才 sleep，故不影响正常路径）。
const DELETE_PERSIST_POLL: Duration = Duration::from_millis(5);

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
    /// **已提交**、`desired=true` 仍在落盘的 Start 数（P0-D）。
    ///
    /// 与 `inflight_starts` 的区别是"有没有外部副作用"：票据只代表"有人想启动"，
    /// 尚未拉起任何东西，DELETE 的代次 bump 足以作废它；本计数代表"驱动已经起来、
    /// 运行意图正在写库"，删除前必须等它真正收敛，否则迟到的 `true` 会落到删除后
    /// 重建的同 ID 记录上（与 P0-A 同类的持久化反序）。
    ///
    /// 登记点在 `commit_start`（与 `start_endpoint` 同一临界区），归还点由
    /// [`StartPersistGuard`] 的 Drop 负责——含 blocking 任务 panic/取消路径。
    start_persist_inflight: u32,
    /// DELETE 屏障（P0-D）：置位期间**不接受新的写入**。
    ///
    /// 为什么必须有它：只"等待在途写入收敛"是不够的——"观察到集合为空"与
    /// "真正删掉记录行"之间仍会受理新的 Stop 并派生出新的落盘任务，那个任务
    /// 完全可能在本端点被删除、又被同 ID 重建**之后**才执行。屏障把这段窗口
    /// 里的新写入直接掐掉，于是"一次观测为空"等价于"删行时一定为空"。
    deleting: bool,
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

/// Start 运行意图落盘的 RAII 计数（P0-D）。
///
/// 为什么用 RAII 而不是"每个返回分支各减一次"：落盘发生在 `spawn_blocking`
/// 里，写入成功、写入失败、blocking 任务 panic 三条路径都要归还；只要漏掉
/// 一条，DELETE 的等待就会永久超时（比漏减更糟的是"错误地减掉"——那会让
/// 删除在写入未完成时放行）。Drop 在**展开路径**上同样执行，因此无需重复
/// 记账。注意计数归还发生在本任务结束时，比 DB 写入本身略晚——这只会让
/// DELETE 多等一拍，方向是安全的。
struct StartPersistGuard {
    gate: Arc<LifecycleGate>,
    id: String,
}

impl StartPersistGuard {
    /// 只在 `commit_start` 的同步临界区内构造。
    ///
    /// 注意签名接收 `&mut EpGate` 而不是自己再加锁：调用方此时**已经持有**门锁，
    /// `std::sync::Mutex` 不可重入，二次加锁会死锁。
    fn register(g: &mut EpGate, gate: &Arc<LifecycleGate>, id: &str) -> Self {
        g.start_persist_inflight = g.start_persist_inflight.saturating_add(1);
        Self {
            gate: Arc::clone(gate),
            id: id.to_string(),
        }
    }
}

impl Drop for StartPersistGuard {
    fn drop(&mut self) {
        let mut m = self.gate.inner.lock().unwrap();
        if let Some(g) = m.get_mut(&self.id) {
            g.start_persist_inflight = g.start_persist_inflight.saturating_sub(1);
        }
    }
}

/// DELETE 屏障的 RAII 句柄（P0-E-2）：**未提交即撤销**。
///
/// 修复前的缺陷：`begin_delete()` 置 `deleting=true` 之后，handler 在
/// `await_intent_quiescent` 上等待期间若被取消（客户端断开 / 请求超时）或 panic，
/// 后面那些显式 `abort_delete()` 语句根本执行不到 ⇒ 屏障永久留在 `true`，
/// 该端点此后所有 Start 都被 409 `ENDPOINT_DELETED` 拒绝，只能重启 Core 才能好。
///
/// 用 RAII 而不是"每个返回分支补一句撤销"：Drop 在**三条**非正常退出路径上都会
/// 执行（普通提前 return、future 被丢弃时的取消、panic 展开），补语句的方式会
/// 随着分支增长不断漏。且撤销是**幂等**的：重复撤销只是把 true 写成 false。
pub struct DeleteGuard {
    gate: Arc<LifecycleGate>,
    id: String,
    /// 已提交（行已删除）：Drop 不得再撤销屏障。
    committed: bool,
}

impl DeleteGuard {
    /// 提交：记录行已删除 ⇒ 立墓碑代次并复位门状态（等价于 [`LifecycleGate::finish_delete`]）。
    ///
    /// 只应在 `store.delete_endpoint` 返回 `Ok(true)` 之后调用。
    pub fn commit(mut self) {
        self.gate.finish_delete(&self.id);
        self.committed = true;
    }
}

impl Drop for DeleteGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // panic 展开期间门锁可能已被毒化：此时不能二次 panic（`unwrap()` 会让
        // 进程 abort）。抢不到锁就只能放弃这次撤销——但毒化只可能来自持锁期间
        // panic 的代码，而那条路径本身已经把整个门搞坏了，属既有的 fail-loud 面。
        let Ok(mut m) = self.gate.inner.lock() else {
            return;
        };
        if let Some(g) = m.get_mut(&self.id) {
            g.deleting = false;
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
    /// DELETE 屏障是否已打开（P0-D，诊断面）。
    ///
    /// 只读观测点：DELETE 受理后、等到在途意图写入收敛之前本值为 true，删除撤销
    /// 或完成即复位。单测据此断言"屏障可立可撤"；端到端用例不依赖它判时序
    /// （DELETE 的等待时长取决于注入的 DB 延迟，观察窗口不保证可采样）。
    pub deleting: bool,
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

        let start_persist = {
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
            // P0-D：**在同一临界区**登记"运行意图正在落盘"。DELETE 的屏障与等待
            // 读的是同一把门锁，因此不可能出现"attempt 已起、写入已派发，但删除
            // 看不到它"的交错。
            StartPersistGuard::register(g, self, &ticket.id)
        };

        // 临界区外落盘 true（P0-B）。串行器 guard 被 move 进 blocking 任务：
        // 即使 handler 被取消，落盘与门状态归并仍会完成（只丢应答，不丢状态）。
        let gate = Arc::clone(self);
        let store = Arc::clone(&self.store);
        let id = ticket.id.clone();
        let start_gen = ticket.stop_gen;
        let rt = tokio::runtime::Handle::current();
        match tokio::task::spawn_blocking(move || {
            let _guard = guard;
            // 落盘在途计数随本任务结束归还（含 panic 展开路径）。
            let _start_persist = start_persist;
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
        // 步骤 2（P0-D）：DELETE 屏障期间一律不得提交。
        // 复用既有 `ENDPOINT_DELETED`（与 P0-7 的"配置行已删"同一语义、同一响应码），
        // 不新增码：调用方已按"该端点正在消失"处理。
        if g.deleting {
            return Err(LifecycleError::new(
                StatusCode::CONFLICT,
                "ENDPOINT_DELETED",
                format!("endpoint `{endpoint_id}`: 该端点正在被删除，启动已中止"),
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
    // Delete：删除屏障 + 在途写入收敛 + 墓碑代次（P0-7 / P0-D）
    // -----------------------------------------------------------------

    /// DELETE 第一步（同步）：打开删除屏障，**拒绝新的写入**进入本端点。
    ///
    /// 修复的竞态（P0-D，评审给的五步交错）：Stop-A 的 teardown 已完成、
    /// `desired=false` 仍在写库，此时 DELETE 只看 attempt 表 ⇒ 认为"没有活跃
    /// attempt" ⇒ 删行 + 清空在途标记；随后的 CREATE 以**同一 ID** 重建并 Start
    /// 写入 `desired=true`；最后 Stop-A 的迟到 SQL 落到新记录上，把新实例污染成
    /// `desired=false` 而实际在跑。
    ///
    /// 根因与 P0-A 同类：**清空集合 ≠ 后台持久化已完成**。
    ///
    /// 因此 DELETE 拆成三步，必须按序调用：
    /// 1. [`Self::begin_delete`]：屏障 —— 新 Start 提交一律不再进入（**Stop 的
    ///    意图持久化仍然进入**，P0-E-1）；
    /// 2. [`Self::await_intent_quiescent`]：等**已在途**的写入真正收敛（有界）；
    /// 3. 删行后 [`DeleteGuard::commit`]（成功，立墓碑代次）/ guard 被丢弃
    ///    （未删成，只撤屏障）。
    ///
    /// 为什么先开屏障再等待（而不是先等再开屏障）：否则"观测到集合为空"与
    /// "真正删行"之间还会受理新 Stop 并派生出新落盘任务，那个任务可能在本端点
    /// 被删除、又被同 ID 重建**之后**才执行——窗口只是变窄，并没有消失。
    ///
    /// P0-E-2：返回的是 RAII guard 而不是"调用方保证成对调用"。修复前，handler
    /// 在等待收敛期间被取消/panic/连接断开时，`abort_delete` 那几行根本执行不到，
    /// `deleting` 就永久为 true ⇒ 该端点此后所有 Start 都被 409 拒绝（只能重启
    /// Core 才能恢复）。现在只要 guard 离开作用域（含展开路径），屏障自动撤销。
    pub fn begin_delete(self: &Arc<Self>, endpoint_id: &str) -> DeleteGuard {
        let mut m = self.inner.lock().unwrap();
        let g = m.entry(endpoint_id.to_string()).or_default();
        g.deleting = true;

        DeleteGuard {
            gate: Arc::clone(self),
            id: endpoint_id.to_string(),
            committed: false,
        }
    }

    /// DELETE 撤销（等待超时 / 404 / 存储错误 / handler 被取消）：只撤屏障。
    ///
    /// 刻意**不动**在途计数、`intent_unpersisted` 与 `stop_persist_confirmed_gen`：
    /// 删除没有发生，端点仍然存在，这些状态对"是否允许启动 / 是否停干净"仍然
    /// 有效，清掉它们等于放宽保护。
    ///
    /// 生产路径由 [`DeleteGuard`] 的 `Drop` 调用；保留 `pub` 是为了让单测能显式
    /// 表达"未提交即撤销"这一语义。
    pub fn abort_delete(&self, endpoint_id: &str) {
        let mut m = self.inner.lock().unwrap();
        if let Some(g) = m.get_mut(endpoint_id) {
            g.deleting = false;
        }
    }

    /// 等待该端点的全部运行意图落盘收敛（P0-D，DELETE 专用，有界）。
    ///
    /// 判据是**两个真实计数器**，而不是"集合已被清空"这个动作：
    /// - `stop_persist_inflight`：已受理 Stop 的 `desired=false` 尚未确认；
    /// - `start_persist_inflight`：已提交 Start 的 `desired=true` 尚未确认。
    ///
    /// 必须在 [`Self::begin_delete`] 之后调用，且**只**能挡住 Start（P0-E-1 之后
    /// 屏障期的 Stop 照常落盘），因此"观测为空"并不严格等价于"删行时一定为空"：
    /// 观测与删行之间仍可能有一次屏障期 Stop 登记。调用方必须再用
    /// [`Self::lock_intent_persist_if_quiescent`] 做最后复查，二者合起来才成立。
    /// 超时 ⇒ 返回 409 `DELETE_PERSIST_IN_PROGRESS`（拒绝删除，而不是在没把握时
    /// 删掉配置：删除是不可逆的破坏性动作，等不到就应当让客户端重试）。
    pub async fn await_intent_quiescent(
        &self,
        endpoint_id: &str,
        timeout: Duration,
    ) -> Result<(), LifecycleError> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = {
                let m = self.inner.lock().unwrap();
                m.get(endpoint_id).map_or((0usize, 0u32), |g| {
                    (g.stop_persist_inflight.len(), g.start_persist_inflight)
                })
            };

            if now == (0, 0) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(delete_persist_in_progress(endpoint_id, now, timeout));
            }
            tokio::time::sleep(DELETE_PERSIST_POLL).await;
        }
    }

    /// 在意图落盘串行器上做**删行前的最后复查**：只有"此刻没有在途写入、且不会
    /// 有写入抢到串行器"时才返回 guard。
    ///
    /// 为什么需要它（P0-E-1 引入）：屏障期 Stop 现在会真实落盘，而它的登记
    /// （`admit_stop` 的 std 门锁）与 [`Self::await_intent_quiescent`] 的观测之间
    /// 还有一段瞬时窗口——"观测为空 → 屏障期 Stop 登记 → 删行"会让那条
    /// `desired=false` 落到删除后同 ID 重建的记录上（P0-D 的同类污染）。
    ///
    /// 串行器是所有 `desired` 写库的必经关卡：持有它就保证"既没有写正在执行，
    /// 也不会有写插到删行之前"。已登记但尚未取到串行器的写入通过 `None` 被检出，
    /// 调用方据此重试等待（预算内）。
    ///
    /// 锁序：先取串行器（`persist_lock`）、再取门锁（`inner`），与落盘任务同向，
    /// 不构成环。
    pub async fn lock_intent_persist_if_quiescent(
        &self,
        endpoint_id: &str,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let lock = {
            let mut m = self.inner.lock().unwrap();
            Arc::clone(&m.entry(endpoint_id.to_string()).or_default().persist_lock)
        };
        // 串行器在门锁之外 await：屏障期的 Stop 落盘可能正持有它数秒。
        let guard = lock.lock_owned().await;
        let clean = {
            let m = self.inner.lock().unwrap();
            m.get(endpoint_id)
                .is_none_or(|g| g.stop_persist_inflight.is_empty() && g.start_persist_inflight == 0)
        };
        if clean { Some(guard) } else { None }
    }

    /// DELETE 第三步（记录行已删除）：立墓碑代次并复位门状态。
    ///
    /// 生产路径经 [`DeleteGuard::commit`] 调用（guard 负责"未提交即撤销屏障"）。
    ///
    /// 两件事：
    /// 1. `stop_gen + 1` —— **墓碑代次**，复用既有代次机制，不新造一套：删除前
    ///    admit 的 Start 票据、以及删除前受理的 Stop 代次，永远匹配不上重建后的
    ///    实例（`finish_stop_persist` 的 `latest` 判定同样不再成立，因此旧代次
    ///    即使归并也不会改写新实例的内存意图）。
    /// 2. 复位缓存的 `desired`（重建后的端点懒加载自 DB），清掉围栏标记（记录行
    ///    已不存在，围栏没有保护对象；`store` 侧的围栏由 handler 一并清除）。
    ///
    /// **不再 `stop_persist_inflight.clear()`**：清空集合不等于任务已完成。能走到
    /// 这里的前提是 [`Self::await_intent_quiescent`] 已证明计数为 0；万一该前提
    /// 被破坏（不应发生），保留计数是 fail-closed 方向——重建后的同名端点会被
    /// 409 `STOP_PERSIST_IN_PROGRESS` 拦住，而不是让旧写入落到新记录上。
    pub fn finish_delete(&self, endpoint_id: &str) {
        let mut m = self.inner.lock().unwrap();
        let g = m.entry(endpoint_id.to_string()).or_default();
        if !g.stop_persist_inflight.is_empty() || g.start_persist_inflight != 0 {
            tracing::error!(
                endpoint = %endpoint_id,
                stop_inflight = g.stop_persist_inflight.len(),
                start_inflight = g.start_persist_inflight,
                "删除完成时仍有在途运行意图写入（不应发生）；保留计数以禁止该 id 被重新启动"
            );
        }
        g.stop_gen += 1;
        g.stop_persist_confirmed_gen = None;
        g.intent_unpersisted = false;
        g.desired = None;
        g.deleting = false;
    }

    // -----------------------------------------------------------------
    // Stop：admit（同步：代次 +1 + 置假 + 认领 attempt）→ 落盘（并行）
    // -----------------------------------------------------------------

    /// 受理 Stop（同步）：作废在途 Start、**先启动拆解**、再并行尝试落盘。
    ///
    /// 顺序不可交换（S1）：若先落盘再拆解，DB busy 会把实际停止推迟数秒；
    /// 停止是安全方向，优先级高于持久化。
    ///
    /// P0-E-1：删除屏障**不再**改变本函数的行为——屏障期同样走真实落盘，理由见
    /// 下方临界区内的注释。
    pub fn admit_stop(self: &Arc<Self>, endpoint_id: &str) -> StopAdmission {
        let (handle, stop_gen, persist_lock) = {
            let mut m = self.inner.lock().unwrap();
            let g = m.entry(endpoint_id.to_string()).or_default();
            // P0-E-1（方案 A）：**删除屏障只挡 Start，不挡 Stop 的意图持久化**。
            //
            // 修复前的行为：屏障期直接跳过落盘、`persist` 返回即成功的占位任务，
            // 于是 Stop 返回 200 而 DB 里的 `desired_running` 可能仍是 true。
            // 只要 DELETE 随后失败（5xx / 409 `DELETE_PERSIST_IN_PROGRESS` / 404），
            // 端点就仍然存在且"持久意图 = 运行"，而调用方拿到的却是"停成功了"
            // ——这直接违反 Stop 的既有契约（200 ⇒ 停止意图已确认）。
            //
            // 为什么选"照常落盘"而不是"屏障期 Stop 返回 409"：停止是安全方向，
            // 任何时候都应当被记录；把 Stop 变成可失败的 409 会把"让机器停下来"
            // 这个动作交给客户端重试，方向不安全。因此这里与正常路径**完全同源**：
            // 代次 +1、内存置假、登记在途代次、派发真实落盘任务，200 只在落盘
            // 确认之后才返回（失败则 500 `STOP_INTENT_NOT_PERSISTED`）。
            //
            // 反向风险（这条 `false` 落到"删除后同 ID 重建"的记录上，属 P0-D 的
            // 同类污染）由两处封住：屏障期 Stop 同样登记在途代次 ⇒ DELETE 的
            // `await_intent_quiescent` 不会在它落完之前删行；删行前还会在意图
            // 串行器上复查一次（`lock_intent_persist_if_quiescent`）。行已不存在
            // 时写入命中 0 行，`finish_stop_persist` 按"端点确已消失"收尾，
            // 与契约里的"或端点已确实不存在"一致。
            //
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
            deleting,
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
                g.deleting,
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
            deleting,
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

/// P0-D 的拒绝码：DELETE 等不到在途意图写入收敛。
///
/// **增量新增**（不动 wire/proto，也不动冻结的 8 个 Stop 码）：它属于删除面，
/// 与 Start 面的 `STOP_PERSIST_IN_PROGRESS` 语义不同，客户端需要区分"再试一次
/// Start"与"再试一次 DELETE"。
fn delete_persist_in_progress(
    endpoint_id: &str,
    pending: (usize, u32),
    waited: Duration,
) -> LifecycleError {
    LifecycleError::new(
        StatusCode::CONFLICT,
        "DELETE_PERSIST_IN_PROGRESS",
        format!(
            "endpoint `{endpoint_id}`: 仍有运行意图正在落盘（Stop 在途 {} 个 / Start 在途 {} 个），\
             已等待 {}ms 仍未收敛；为避免删除后被同 ID 重建的实例被旧写入污染，本次删除被拒绝，\
             请稍后重试",
            pending.0,
            pending.1,
            waited.as_millis()
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

    /// P0-A 的集合语义（**不依赖**串行器）：Stop-B 是最新代次且先完成，
    /// 不得把仍在途的 Stop-A 代次从集合里清掉；否则新 Start 会被放行，
    /// 随后 A 的迟到 false 覆盖 Start 的 true。
    #[tokio::test]
    async fn newer_stop_completion_does_not_clear_older_inflight_stop() {
        let g = gate("p0a-set");
        // 直接构造"两次 Stop 已受理、A 尚未确认、B 已确认"的门状态。
        {
            let mut m = g.inner.lock().unwrap();
            let ep = m.entry("ep-1".to_string()).or_default();
            ep.stop_gen = 2;
            ep.stop_persist_inflight.insert(1);
            ep.stop_persist_inflight.insert(2);
        }
        // Stop-B（最新代次）先完成。
        g.finish_stop_persist("ep-1", 2, Ok(true)).unwrap();
        let s = g.sample("ep-1");
        assert_eq!(
            s.stop_persist_inflight_count, 1,
            "B 完成后 A 必须仍在集合中: {s:?}"
        );
        assert_eq!(s.stop_persist_inflight, Some(1));

        // 无活 attempt ⇒ 新 Start 必须被拒，且不得写 desired=true。
        let ticket = g.admit_start("ep-1");
        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .await
            .expect_err("A 仍在途，Start 必须被拒");
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, "STOP_PERSIST_IN_PROGRESS");
        assert!(!g.manager.has_live_attempt("ep-1"), "不得产生 attempt");

        // A 完成后集合清空，Start 才允许提交。
        g.finish_stop_persist("ep-1", 1, Ok(true)).unwrap();
        assert_eq!(g.sample("ep-1").stop_persist_inflight_count, 0);
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
    /// （真正的并发 Stop 端到端在 `tests/driver-contract/tests/stop_lifecycle.rs`。）
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

    // -----------------------------------------------------------------
    // P0-D：删除与未完成 Stop 落盘的竞态
    // -----------------------------------------------------------------

    /// P0-D（门状态面）：在途落盘未收敛时 DELETE 的等待必须**失败**（409 拒绝），
    /// 收敛后才放行；`abort_delete` 不得顺手清掉任何保护状态。
    #[tokio::test]
    async fn delete_wait_rejects_unconverged_intent_persist_and_passes_after_convergence() {
        let g = gate("p0d-wait");
        // 构造"Stop-A 已受理、其 desired=false 仍在途"的门状态。
        {
            let mut m = g.inner.lock().unwrap();
            let ep = m.entry("ep-1".to_string()).or_default();
            ep.stop_gen = 1;
            ep.desired = Some(true);
            ep.stop_persist_inflight.insert(1);
        }
        let barrier = g.begin_delete("ep-1");
        let err = g
            .await_intent_quiescent("ep-1", Duration::from_millis(50))
            .await
            .expect_err("落盘未收敛时 DELETE 必须被拒绝");
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, "DELETE_PERSIST_IN_PROGRESS");

        // 撤销屏障（P0-E-2：丢弃 guard 即撤销）：端点仍在，Stop 的落盘归并照常
        // 推进（保护状态不得被清掉）。
        drop(barrier);
        assert_eq!(g.sample("ep-1").stop_persist_inflight_count, 1);
        g.finish_stop_persist("ep-1", 1, Ok(true)).unwrap();
        assert_eq!(g.sample("ep-1").desired, Some(false));
        assert_eq!(g.sample("ep-1").stop_persist_confirmed_gen, Some(1));

        // 收敛后：等待立即通过（正常路径无 sleep，一次观测即返回）。
        let barrier = g.begin_delete("ep-1");
        let t = Instant::now();
        g.await_intent_quiescent("ep-1", DELETE_PERSIST_WAIT)
            .await
            .expect("已收敛，等待必须通过");
        assert!(
            t.elapsed() < Duration::from_millis(100),
            "正常路径不得引入等待时延，实际 {:?}",
            t.elapsed()
        );

        // 立墓碑：代次 +1（旧票据/旧 Stop 代次永不匹配重建后的实例）。
        let gen_before = g.sample("ep-1").stop_gen;
        barrier.commit();
        let s = g.sample("ep-1");
        assert_eq!(s.stop_gen, gen_before + 1, "删除成功必须推进墓碑代次");
        // `desired` 复位为"无缓存"⇒ sample 懒加载自 DB（本用例的库行仍在，
        // 值为 false；真实删除后为 None）。两条都表示"不再沿用旧实例的缓存"。
        assert_eq!(s.desired, Some(false), "重建后的端点应懒加载自 DB");
        assert_eq!(s.stop_persist_confirmed_gen, None);
        assert!(!s.intent_unpersisted);
    }

    /// P0-D（屏障面）+ P0-E-1：屏障期间新 Start 一律 409 `ENDPOINT_DELETED`
    ///（且不留 attempt）；**Stop 的意图持久化照常进入**——屏障只挡 Start。
    ///
    /// 旧实现在屏障期跳过落盘、`persist` 返回一个"立即 Ok"的占位任务，于是 Stop
    /// 的 200 与 DB 里的 `desired_running` 脱钩（P0-E-1）。本用例把 DB 预置为
    /// true，断言屏障期 Stop 的落盘任务返回时 DB 已是 false。
    #[tokio::test]
    async fn delete_barrier_blocks_new_start_but_still_persists_new_stop() {
        let g = gate("p0d-barrier");
        let barrier = g.begin_delete("ep-1");
        assert!(g.sample("ep-1").deleting, "屏障必须可观测（诊断面）");
        drop(barrier);
        assert!(
            !g.sample("ep-1").deleting,
            "P0-E-2：丢弃 guard（取消/panic 路径）必须撤销屏障"
        );
        let barrier = g.begin_delete("ep-1");

        // 预置 DB 的旧值 true：否则下面的"落盘已生效"断言没有判别力。
        g.store.set_desired_running("ep-1", true).unwrap();
        assert!(
            g.store
                .get_endpoint("ep-1")
                .unwrap()
                .unwrap()
                .desired_running,
            "前置：DB 必须是 true"
        );

        // 新 Stop：受理、拆解照做、**真实落盘**（代次登记后归并归还）。
        let adm = g.admit_stop("ep-1");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), adm.persist)
                .await
                .expect("屏障期 Stop 的落盘必须有界")
                .expect("落盘任务不得 panic"),
            Ok(()),
            "屏障期 Stop 的落盘必须成功"
        );
        assert_eq!(
            g.sample("ep-1").stop_persist_inflight_count,
            0,
            "落盘确认后必须归还代次"
        );
        assert_eq!(g.sample("ep-1").desired, Some(false), "内存意图仍须置假");
        assert!(
            !g.store
                .get_endpoint("ep-1")
                .unwrap()
                .unwrap()
                .desired_running,
            "P0-E-1：屏障期 Stop 必须把 desired=false 真正落盘（否则 Stop 的 200 名不副实）"
        );

        // 新 Start：必须 409，且不得产生 attempt。
        let ticket = g.admit_start("ep-1");
        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .await
            .expect_err("删除屏障期间 Start 必须被拒");
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, "ENDPOINT_DELETED");
        assert!(!g.manager.has_live_attempt("ep-1"), "不得产生 attempt");

        // 撤销屏障后，请求重新抵达 manager（本例驱动目录为空，因此必然失败，
        // 但失败原因**不得**再是删除相关的码 ⇒ 屏障确已撤销）。
        drop(barrier);
        assert!(!g.sample("ep-1").deleting, "撤销后屏障必须复位");
        let ticket = g.admit_start("ep-1");
        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .await
            .expect_err("本例驱动目录为空，Start 必然失败");
        assert_ne!(
            err.code, "ENDPOINT_DELETED",
            "撤销屏障后不得继续以删除为由拒绝: {err}"
        );
    }

    /// P0-D（不变量）：`finish_delete` **不得**靠 `clear()` 掩盖在途写入。
    ///
    /// 修复前的 `invalidate_starts_for_delete` 无条件 `stop_persist_inflight.clear()`；
    /// 本用例把计数摆成"仍有在途"后调 `finish_delete`，集合必须原样保留 ⇒ 重建后
    /// 的同 ID 端点会被 409 `STOP_PERSIST_IN_PROGRESS` 拦住（fail-closed），
    /// 而不是让旧写入落到新记录上。
    #[tokio::test]
    async fn finish_delete_never_hides_inflight_intent_writes() {
        let g = gate("p0d-tomb");
        {
            let mut m = g.inner.lock().unwrap();
            let ep = m.entry("ep-1".to_string()).or_default();
            ep.stop_gen = 4;
            ep.stop_persist_inflight.insert(4);
            ep.start_persist_inflight = 1;
        }
        g.begin_delete("ep-1").commit();
        let s = g.sample("ep-1");
        assert_eq!(
            s.stop_persist_inflight_count, 1,
            "finish_delete 不得清空在途 Stop 集合: {s:?}"
        );
        assert_eq!(s.stop_gen, 5, "墓碑代次必须推进");

        // 重建同 ID 后的 Start 必须被拒（fail-closed），绝不写 desired=true。
        let ticket = g.admit_start("ep-1");
        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .await
            .expect_err("仍有在途写入时不得放行 Start");
        assert_eq!(err.code, "STOP_PERSIST_IN_PROGRESS");
        assert!(!g.manager.has_live_attempt("ep-1"));
    }

    // -----------------------------------------------------------------
    // P0-E-1：屏障期 Stop 必须真实落盘（Stop 的 200 ⇒ 停止意图已确认）
    // -----------------------------------------------------------------

    /// **P0-E-1（确定性 Gate）**：屏障期 Stop 的"成功"必须等价于
    /// `desired_running=false` 已落盘；因此随后 DELETE 失败（端点仍在）时，
    /// 持久意图**不得**还是 true。
    ///
    /// 旧码上确定性必红：屏障期 `admit_stop` 直接返回一个"立即 Ok"的占位落盘
    /// 任务，`persist.await == Ok(())` 但库里仍为 true；这里第二条断言即失败。
    ///
    /// DELETE 失败用"丢弃 guard（`abort` 语义）+ 库行仍在"表达：这正是
    /// `DELETE_PERSIST_IN_PROGRESS` / 404 / 存储错误三条失败路径的共同终态。
    #[tokio::test]
    async fn barrier_stop_confirms_intent_so_delete_failure_cannot_leave_desired_true() {
        let g = gate("p0e1");
        // 前置：端点正在运行（持久意图 true），DELETE 已打开屏障并在等待收敛。
        g.store.set_desired_running("ep-1", true).unwrap();
        assert!(
            g.store
                .get_endpoint("ep-1")
                .unwrap()
                .unwrap()
                .desired_running,
            "前置：DB 必须是 true"
        );
        let barrier = g.begin_delete("ep-1");
        assert!(g.sample("ep-1").deleting, "前置：屏障已立");

        // 屏障期 Stop：受理并等待它自己的落盘结论——这就是 handler 返回 200 的判据。
        let adm = g.admit_stop("ep-1");
        let confirmed_gen = adm.stop_gen;
        let persist = tokio::time::timeout(Duration::from_secs(5), adm.persist)
            .await
            .expect("屏障期 Stop 的落盘必须有界")
            .expect("落盘任务不得 panic");
        assert_eq!(persist, Ok(()), "Stop 的 200 要求落盘成功");

        // 核心断言 1：200 之前，`desired=false` 已经落到库里（不是"内存里假"）。
        assert!(
            !g.store
                .get_endpoint("ep-1")
                .unwrap()
                .unwrap()
                .desired_running,
            "P0-E-1：Stop 报告成功后 DB 仍为 true（违反 Stop 契约）"
        );
        // 核心断言 2：门状态同向——本次代次被确认为"已确认落盘"，且在途集合已归还。
        let s = g.sample("ep-1");
        assert_eq!(s.desired, Some(false));
        assert_eq!(s.stop_persist_confirmed_gen, Some(confirmed_gen));
        assert_eq!(s.stop_persist_inflight_count, 0);
        assert!(!s.intent_unpersisted);

        // DELETE 失败：屏障撤销、记录行仍在 ⇒ 持久意图必须已经是 false。
        drop(barrier);
        assert!(!g.sample("ep-1").deleting, "屏障必须撤销");
        let row = g
            .store
            .get_endpoint("ep-1")
            .unwrap()
            .expect("DELETE 失败后端点必须仍然存在");
        assert!(
            !row.desired_running,
            "P0-E-1：屏障期 Stop 返回成功 + DELETE 失败 ⇒ desired_running 不得为 true"
        );
    }

    /// P0-E-1 的另一半：屏障期 Stop 的落盘**失败**时不得返回"成功"。
    ///
    /// 与正常路径同源 ⇒ 走 `finish_stop_persist(Err)`：写围栏、禁止新 Start，
    /// handler 侧返回 500 `STOP_INTENT_NOT_PERSISTED`（此处断言门状态与围栏，
    /// 响应码由 `stop_lifecycle` 的端到端用例覆盖）。
    #[tokio::test]
    async fn barrier_stop_persist_failure_is_not_reported_as_success() {
        let g = gate("p0e1-err");
        g.store.set_desired_running("ep-1", true).unwrap();
        let _barrier = g.begin_delete("ep-1");
        let adm = g.admit_stop("ep-1");
        let stop_gen = adm.stop_gen;
        // 模拟 `set_desired_running` 失败（blocking 任务返回 Err）。
        g.finish_stop_persist("ep-1", stop_gen, Err("disk full".into()))
            .expect_err("落盘失败必须向上返回 Err（handler ⇒ 500）");
        let s = g.sample("ep-1");
        assert!(s.intent_unpersisted, "落盘失败必须置围栏标记");
        assert_eq!(s.stop_persist_inflight_count, 0, "失败也必须归还代次");
        assert_ne!(
            s.stop_persist_confirmed_gen,
            Some(stop_gen),
            "未落盘不得留下确认凭证"
        );
        assert!(
            g.store
                .get_endpoint("ep-1")
                .unwrap()
                .unwrap()
                .desired_running,
            "前置：本例刻意让 DB 仍为 true"
        );
        assert_eq!(
            g.store.stop_intent_unpersisted().unwrap(),
            vec!["ep-1".to_string()],
            "失败必须写持久化围栏（重启不得自动拉起）"
        );
    }

    // -----------------------------------------------------------------
    // P0-E-2：DELETE 屏障的 RAII 撤销（取消 / panic 不得留下永久屏障）
    // -----------------------------------------------------------------

    /// **P0-E-2（确定性 Gate）**：guard 未提交即被丢弃 ⇒ 屏障必须撤销，
    /// 且随后的 Start 重新抵达 manager（不再被 `ENDPOINT_DELETED` 拒绝）。
    #[tokio::test]
    async fn delete_guard_drop_without_commit_clears_barrier() {
        let g = gate("p0e2-drop");
        {
            let guard = g.begin_delete("ep-1");
            assert!(g.sample("ep-1").deleting, "屏障已立");
            drop(guard);
        }
        assert!(
            !g.sample("ep-1").deleting,
            "P0-E-2：guard 未提交即丢弃 ⇒ 屏障必须撤销"
        );
        let ticket = g.admit_start("ep-1");
        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .await
            .expect_err("本例驱动目录为空，Start 必然失败");
        assert_ne!(
            err.code, "ENDPOINT_DELETED",
            "屏障撤销后不得再以删除为由拒绝: {err}"
        );
    }

    /// **P0-E-2（panic 路径）**：handler panic 展开时 guard 的 Drop 仍要撤销屏障。
    ///
    /// 旧码的显式 `abort_delete` 在展开路径上永远执行不到 ⇒ 永久 409。
    #[tokio::test]
    async fn delete_guard_survives_handler_panic() {
        let g = gate("p0e2-panic");
        let g2 = Arc::clone(&g);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = g2.begin_delete("ep-1");
            assert!(g2.sample("ep-1").deleting, "屏障已立");
            panic!("模拟 handler 在等待收敛期间 panic");
        }));
        assert!(panicked.is_err(), "前置：panic 必须发生");
        assert!(
            !g.sample("ep-1").deleting,
            "P0-E-2：panic 展开后屏障必须撤销（否则该端点永久 409）"
        );
    }

    /// **P0-E-2（取消路径）**：handler future 被丢弃（客户端断开 / 请求超时）时，
    /// guard 随 future 一起 drop ⇒ 屏障撤销。
    ///
    /// 用"被 abort 的任务停在 await 上"精确表达取消：任务活着时屏障为 true，
    /// abort 之后必须立刻为 false。
    #[tokio::test]
    async fn delete_guard_is_cleared_when_handler_future_is_cancelled() {
        let g = gate("p0e2-cancel");
        let waiter = {
            let g = Arc::clone(&g);
            tokio::spawn(async move {
                let _guard = g.begin_delete("ep-1");
                // 模拟"停在 await_intent_quiescent 上"。
                std::future::pending::<()>().await
            })
        };
        // 等任务确实持有了 guard（屏障已立）再取消，避免测到"还没开始"。
        for _ in 0..200 {
            if g.sample("ep-1").deleting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(g.sample("ep-1").deleting, "前置：屏障已立");
        waiter.abort();
        let _ = waiter.await;
        assert!(
            !g.sample("ep-1").deleting,
            "P0-E-2：取消（future 被丢弃）后屏障必须撤销"
        );
        // 后续 Start 必须能正常抵达 manager（本例无驱动 ⇒ 失败原因与删除无关）。
        let ticket = g.admit_start("ep-1");
        let err = g
            .commit_start(&ticket, dummy_cfg("ep-1"))
            .await
            .expect_err("本例驱动目录为空，Start 必然失败");
        assert_ne!(err.code, "ENDPOINT_DELETED", "后续 Start 不得再被删除挡住");
    }
}
