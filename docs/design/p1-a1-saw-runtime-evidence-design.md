# P1-A1 详细实现方案：`saw_runtime_evidence` 与 L0–L6 测试设计

- 审查基线：`main = 2a29777b643c674557d975cdf93998645cbe10d6`；本分支 `docs/p1-nck-running-backoff`。
- 性质：**只读设计**。未修改生产代码/测试/配置；未启动 mesad/Vite/emulator，未连设备，未安装，未运行测试或构建。
- 前置依据：`docs/design/endpoint-running-backoff-p1-review.md`（根因审查，下称“审查”）、`docs/design/nck-s7-job-0x8201-diagnosis-plan.md` §16（退避语义更正与假 RUNNING 实测）。
- 事实口径：一律给 `文件:行号`；无法证实的标 **待确认**，不猜测。
- 本节**不决定**“稳定运行”的时长/证据阈值（属生命周期设计审查），也**不改变**任何 Wire 协议、Driver 行为、配置格式或 RUNNING 的 UI 语义。

### 修订记录

- **v2（2026-10-19，设计审查裁决后）**：方向 PASS、编码条件 GO。按裁决收紧四处——① 新增“标记 × Event Ingress 生命周期契约”与“上一 attempt 残留不得计入”的论证（§1.4）；② “teardown 后读取”改为**结构化 Join 顺序**证明，明确内存序不解决任务生命周期竞争（§1.5）；③ L2 改为以 `retry_in_secs` 计划值为权威、attempt 次数降为辅助指标（§6.2）；④ Event-only 专项升级为 **P0**，断言改为“故障后下一次计划退避严格 = 1s”，不再以存储统计量增长或时间间隔充当结论（§6.3）。
- v1：初版 335 行方案（已交付审查）。

## 0. 结论摘要

| 项 | 结论 |
|---|---|
| 判定改动 | `if had_running_session && saw_runtime_evidence { backoff_idx = 0 }`（替换 `endpoint.rs:357-360`） |
| `saw_runtime_evidence` 语义 | **本次 attempt 的消费路径至少从 Driver 收到过 1 个批次**（`DataBatch` 或 `EventBatch`），质量/有效性/重复/缺口一律无关 |
| 证据作用域 | attempt 局部（`attempt_session` 内新建、随 attempt 丢弃）；**不**进 `Snapshot`/`EventServices`/任何跨 attempt、跨 Endpoint 结构 |
| 缺口 1（Event-only） | `EventBatch` 由 ingress（`event_ingress.rs:123-143`）消费，`event_loop` 完全看不到；现有唯一计数是**全局** `services.diagnostics.ingress_batches_total`（`event_ingress.rs:171-172` → `crates/event-store/src/lib.rs:540-544`），**不可**用于 per-attempt 判定。方案：attempt 局部 `Arc<AtomicBool>` 传入 ingress，在 `BatchProcessor::process` 入口置真 |
| 缺口 2（慢首采） | A1 的适用边界 = “**反复丢失**且每次都在首个批次前丢失”。单次健康会话（不丢）完全不受影响；不加阈值，误差如实记为“重连节奏变慢”，上限 = 当前退避档（≤30s） |
| 缺口 3（零任务） | 零任务 Start **合法且必须继续成功**（`core-api/lib.rs:242-251` 明确“空快照 = 清空，直接放行”）。A1 只把它的重连节奏从 ~3.6s/次 收敛到 ≤30s/次，不改变“永不进入稳定 RUNNING”这一既有状态机缺口；本轮**不动 Start 契约** |
| 默认统计方式 | 合并为**单布尔**（不按 Data/Event 分开计数），理由见 §3 |
| 附带决策 | C1 新增 `fail_before_first_batch_ms`（Start Ack 之后、零数据批零事件批即失败；只增不改旧键）；C2 新测试**不读不设** `MESA_RECONNECT_FAST`，独立测试二进制 + 计划值/时间线断言；C3 本轮**不实现** `reconnect_attempt_total`/`current_backoff_ms`，测试只用既有日志 |
| 测试口径（v2） | **权威判据一律是 `retry_in_secs` 计划值序列**（L1 `1,2,5,10,30,30`、L2 `1,2,5,10,30`、Event-only 恒 `1`）；attempt 次数/相邻间隔只作辅助观测，不作硬门禁 |
| 优先级 | Event-only 专项 = **P0**；L0–L6 全部 P0；T2a 真实 828D、P2、Release 维持 HOLD |

---

## 1. 实现方案（A1 本体）

### 1.1 判定点：改动前后

```text
现状（endpoint.rs:356-361）
    if had_running_session { backoff_idx = 0; }

目标（同一处）
    // A1：Start Ack 只证明“驱动接受了 Start”，不证明数据面通过。
    // 只有本次 attempt 真的收到过驱动上行批次，才认为运行证据成立并归零。
    if had_running_session && saw_runtime_evidence { backoff_idx = 0; }
```

`backoff_idx` 的读取/递增/日志/Stop 取消**全部不动**：`endpoint.rs:364-369`、`endpoint.rs:370-376`。

### 1.2 新增 attempt 局部观测器（`endpoint.rs`，约 14 行）

放在 `reconnect_backoff_secs`（`endpoint.rs:32-38`）之后：

```rust
/// 单次连接尝试的“运行证据”观测器（attempt 局部，不跨 attempt / Endpoint）。
/// 语义：本次 attempt 的消费路径（数据面 event_loop / 事件面 ingress）至少从
/// Driver 收到过 **一个** 批次；质量、有效性、重复、缺口一律无关。
/// 生命周期：每次 `attempt_session` 新建、随 attempt 丢弃 —— “每次 attempt 开始时
/// 重置”因此是结构性保证，而不是需要记得清零的字段。
struct AttemptEvidence(Arc<AtomicBool>);
impl AttemptEvidence {
    fn new() -> Self { Self(Arc::new(AtomicBool::new(false))) }
    fn handle(&self) -> Arc<AtomicBool> { Arc::clone(&self.0) } // 交给 ingress 任务
    fn mark(&self) { self.0.store(true, Ordering::Relaxed); }
    fn saw(&self) -> bool { self.0.load(Ordering::Relaxed) }
}
```

内存序用 `Relaxed` 的理由（写进注释，避免后人误改）：数据面置真与读取在同一任务内顺序执行；事件面置真与读取之间有 `JoinHandle` 完成（teardown 里 `shutdown_ingress` await 该 handle，`endpoint.rs:684-699`）建立的 happens-before，无需更强序。

### 1.3 采集点 1：`DataBatch`（`event_loop`）

- 现状：`SessionEvent::Batch` 只承载 `DataBatch`（`session.rs:109-122`；`SessionEvent::Batch(batch)` 由 `session.rs:1207` 构造），在 `endpoint.rs:904-907` 被 `apply_batch_logged` 消费。
- 改动：`event_loop` 增加形参 `evidence: &AttemptEvidence`；在 `Batch` 分支首行 `evidence.mark();`。
- 为什么“收到即真”：A1 的判据是**数据面通过**，不是数据质量（审查 §5.2 已冻结）。因此空值批次、全 BAD 批次、重复批次都算。

### 1.4 采集点 2：`EventBatch`（缺口 1 的核心，`event_ingress.rs`）

**事实链（必须按这条链取值，禁止用全局快照推断）**：

| 环节 | 事实 | 证据 |
|---|---|---|
| ① 谁从 wire 收到 | Session reader 是唯一 socket 读者；`Body::EventBatch` 走**独立可靠流**，`continue` 掉，绝不进 `SessionEvent` 队列 | `session.rs:1112-1203`，关键分支 `1149-1152`、入队 `1157-1202` |
| ② 谁把接收端交出去 | `Session::take_event_batches()` 返回带 epoch 门的 `EventReceiver`；`attempt_session` 仅在 `services.is_some() && !event_tasks.is_empty()` 时取走 | `session.rs:575-580`；`endpoint.rs:465-473` |
| ③ 谁消费 | spawn 的 ingress 任务 → `run_event_ingress` 的 `rx.recv()`（cancel 后 `try_recv` 排空同一函数） | `endpoint.rs:483-492`；`event_ingress.rs:123-143`（recv `131-141`、drain `134-139`） |
| ④ 现在记录在哪 | **只有**全局累计 `services.diagnostics.ingress_batches_total`（以及 gap/dup/regression 等） | `event_ingress.rs:167-173`；字段定义 `crates/event-store/src/lib.rs:540-544` |
| ⑤ 失败出口能读到什么 | `event_loop` 只拿 `JoinHandle` 的结果（`Ok/Err/panic`），**拿不到“收到过几批”**；`Lost` 构造点没有任何事件证据 | `endpoint.rs:869-903`（尤其 `886-901` 的 `ingress.take()`） |
| ⑥ epoch 门 | `EventReceiver::recv/try_recv` 内部丢弃非当前 epoch 批次 ⇒ ingress 看到的一定是**本次 Start** 的批次 | `session.rs:1305-1346`（过滤 `1317-1334`） |

**结论**：④ 是跨 Endpoint、跨 attempt 的全局量（评审明令禁止据此判定）；⑤ 证明现有失败出口**没有**可用取值路径。因此必须在 ③ 处新增 attempt 局部标记。

**改动形状（`event_ingress.rs`，约 8 行）**：

```rust
// event_ingress.rs：签名新增 attempt 局部证据句柄（None = 无需观测，例如单测直连）
pub async fn run_event_ingress(
    mut rx: EventReceiver, endpoint_id: String, services: Arc<EventServices>,
    shutdown: CancellationToken, evidence: Option<Arc<AtomicBool>>,
) -> Result<(), IngressFatal> { let mut proc = BatchProcessor::new(endpoint_id, services, evidence); ... }

// BatchProcessor::process 入口：先于任何校验/入库
if let Some(e) = &self.evidence { e.store(true, Ordering::Relaxed); }
self.diag(1, |d| &d.ingress_batches_total);
```

- 置真点在 `process` **入口**而不是 `recv` 返回处：主循环与 cancel 后的 final drain 共用同一函数，两条路径天然一致；且 duplicate/gap/invalid 批次同样计入（它们都证明数据面通过）。
- 置真点必须在 `SequenceGate` 之前：`Duplicate` 会整批跳过并 `return Ok(())`（`event_ingress.rs:189-199`），放在其后会漏掉这一类。
- 置真点必须在 commit **之前**：`StoreUnavailable` 时该批仍证明“驱动→Core 链路通”，入库失败属于另一故障域（见 §8 局限 2）。
- 调用点影响：`run_event_ingress` 是 `pub`，有 3 个既有调用点需补 `None`：`session.rs:1847`、`session.rs:2018`、`tests/driver-contract/tests/event_runtime.rs:953`。选择“改签名 + 补 3 处”而不是“新增 `_observed` 并行函数”：避免出现两条入口、避免有人在新代码里误用未观测版本。

#### 标记 × Event Ingress 生命周期契约（审查要求逐条固定）

| 阶段 | 事实/要求 | 位置 |
|---|---|---|
| **创建** | `AttemptEvidence::new()` 在 `attempt_session` **最开头**（任何 task 被 spawn 之前）创建，先于 ingress、先于配置闭环 | `endpoint.rs:433` 起 |
| **唯一持有者** | 只把 `handle()` 的 clone 交给**本次 attempt 自己 spawn 的** ingress；`Snapshot`/`EventServices`/`active_sessions` registry/`static` 一概不持有 | `endpoint.rs:483-492` |
| **交付时机** | ingress 在 `run_config_flow`（→ Start）**之前** spawn，因此不存在“Start 后批次先到、消费者后到”的窗口；Start 前无批次的保证来自 SDK release gate | `endpoint.rs:474-492`；`driver-sdk:1510-1533` |
| **置真** | `BatchProcessor::process` 入口，主循环（`event_ingress.rs:131-141`）与 cancel 后的 final drain（`:134-139`）共用，**两条路径都被覆盖** | `event_ingress.rs:171` 之前 |
| **停止** | `shutdown_ingress`：`cancel.cancel()` → 当前 commit+publish 完整执行 → 排空 channel → 退出循环 | `endpoint.rs:681-689`；`event_ingress.rs:133-139` |
| **Join（关键）** | 正常路径 `r = &mut h` 等到任务结束；超时路径 `h.abort()` **后仍 `let _ = h.await`** —— 两条路径都保证 ingress 任务已终止 | `endpoint.rs:684-699` |
| **读取** | 仅在 `teardown_attempt` 返回之后（它内部 await 了 `shutdown_ingress`），且 attempt 内**唯一另一个写者**（`event_loop`）早已返回 | `endpoint.rs:568-578` |
| **销毁** | `AttemptEvidence` 随 `attempt_session` 返回而 drop；ingress 的 Arc clone 已随任务终止释放 | — |

**为什么“上一 attempt 的残留事件”不可能计入本次证据**（失败 attempt 的数据隔离）：

1. **通道是新的**：每次 attempt 重新 `DriverProcess::spawn` + `Session::connect_retry_with_heartbeat`（`endpoint.rs:433-464`），`take_event_batches()` 取的是**本次 Session 自己**的接收端（`endpoint.rs:469-473`）。上一 attempt 的 `EventReceiver` 已随其 Session 销毁，且其 ingress 已在上一次 teardown 中被 join/abort+await（`endpoint.rs:684-699`）。
2. **epoch 门**：即使旧 epoch 批次仍在 socket 缓冲，`EventReceiver::recv/try_recv` 会按 `active_event_epochs` 丢弃（`session.rs:1317-1345`）；本次 Start 之前门未激活，StartAck 后才激活（`session.rs:1258-1285`）。
3. **对象是新的**：证据对象不是在旧对象上清零，而是新建（§2），因此不存在“上一次为 true、本次未重置”的路径。

**边界（明确不做）**：证据**不**跨 attempt 累计。一次 attempt 的 drain 批次只归该 attempt；若某次 attempt 在 drain 中收到批次，它也只影响该 attempt 自己的 `Lost` 判定（读点在 teardown 之后）。

### 1.5 读取点：teardown 之后读一次（不改 `AttemptOutcome`）

`attempt_session` 返回值从 `(AttemptOutcome, Option<EventDrainError>)` 扩为三元组 `(AttemptOutcome, Option<EventDrainError>, bool)`：

```text
endpoint.rs:414-432   签名扩为 -> (AttemptOutcome, Option<EventDrainError>, bool)
endpoint.rs:433-444   let evidence = AttemptEvidence::new();        // 先于 spawn
endpoint.rs:483-492   Some(tokio::spawn(run_event_ingress(..., Some(evidence.handle()))))
endpoint.rs:542-550   let outcome = event_loop(..., &evidence).await;
endpoint.rs:568-578   let drain_err = teardown_attempt(...).await;
                      (outcome, drain_err, evidence.saw())           // ← 在 teardown 之后读
endpoint.rs:293-305   let (outcome, drain_err, saw_runtime_evidence) = attempt_session(...).await;
endpoint.rs:336-361   Lost 分支使用 saw_runtime_evidence（判定；日志字段可选、非测试判据）
```

设计取舍：**不**把 `saw_runtime_evidence` 加进 `AttemptOutcome::Lost`（审查 §5.2 草案形状）。原因：

1. `Lost` 有 **16 个构造点**（6 个 `true` / 10 个 `false`，审查 §3.2），加字段要全改；而 `false` 侧永远为假（§2 不变式），是纯噪音。
2. 值只有在 attempt 收尾时才完整：teardown 的 ingress final drain（`event_ingress.rs:134-139`）会处理“已进 channel 未处理”的批次，它们同样是本次 attempt 的证据；在 `event_loop` 内构造 `Lost` 时读取会漏掉（误差保守，但没必要）。
3. 语义上它不属于“失败分类”，只属于“本次 attempt 的观测量”；归零决策点唯一（`endpoint.rs:358`），所有 `Lost` 都经过同一 `match` 臂，不存在“某条路径忘了带证据”。

#### 读取时机由任务生命周期保证，不由内存序保证（审查要求）

**此读点是“先读 false、随后 ingress 处理尾批置 true”这类竞争的唯一防线，必须靠结构化终止顺序，而不是 `Relaxed`/`Acquire`。**

证明（写者集合只有两个）：

| 写者 | 何时停止 | 保证 |
|---|---|---|
| `event_loop`（`SessionEvent::Batch` 置真） | 在 `attempt_session` 调 `teardown_attempt` **之前**已返回（`endpoint.rs:542-550`） | 同一任务顺序，读完即止 |
| ingress 任务（`process` 置真） | `teardown_attempt` → `shutdown_ingress`：正常路径 await JoinHandle；超时路径 `abort()` **再 await**（`endpoint.rs:684-699`） | `JoinHandle::await` 完成后任务已终止（abort 亦在 await 中被回收），此后不可能再有 `store(true)` |
| 读取 | `teardown_attempt` 返回**之后**、`attempt_session` 返回**之前**（`endpoint.rs:568-578`） | 两个写者均已终止 ⇒ 读到的是终值 |

因此各状态 `AtomicBool` 用 `Relaxed` 是安全的：**同步来自 Join 的完成边，不来自原子序**。实现时禁止以下两种“看起来更早”的读法：

- ❌ 在 `event_loop` 返回处读：此时 ingress 可能仍在处理尾部批次，会把本来成立的证据读成 false（正是审查指出的那个竞争）。
- ❌ 在 `event_loop` 里等一个“ingress 已停”的信号再读：等于把 teardown 提前，会破坏 `reader_done → 无新生产 → in-flight 完成 → 队列排空 → drain ACK` 的既有顺序契约（`endpoint.rs:613-619`）。

残余（可接受且方向保守）：超时 `abort` 路径下，尚未执行到 `process` 的尾批会丢失证据 ⇒ 读到 false ⇒ 退避多升一档。**只可能少算，不可能多算**。

### 1.6 与 `had_running_session` 的关系：**与**，不是替代

- `had_running_session` 保留，含义不变：“本次 attempt 走到了 `event_loop`”（`endpoint.rs:884-936` 的 6 个构造点）。
- 新增条件是它的**加强**，不是替换。保留它的三个理由：
  1. 它是“进程起来了 / 配置闭环走通了”的唯一指示；与“数据面通过”正交，日志里两者并列才能区分“没起来”“起来了没数据”“有数据”。
  2. 今天 `saw_runtime_evidence ⇒ had_running_session` 是**构造性不变式**（见 §2），但这是实现细节，不是契约；把两者都写进判定式，未来任何新增 Lost 路径都不会意外放宽。
  3. 删除它会让 `AttemptOutcome::Lost` 少一个语义字段，属于审查明确不批准的“顺带重构”。

### 1.7 不变量（实现时必须逐条核验）

| 不变量 | 现状证据 |
|---|---|
| 不新增 Wire 字段 / 不改 IPC / 不改 Driver 与 SDK | 改动只在 Core 进程内 `endpoint.rs`、`event_ingress.rs` |
| 不改配置语义，不改 RUNNING 写入口与 UI | `endpoint.rs:528-536` 原样保留 |
| 不改退避表值 / `idx.min(4)` / `backoff_idx < 10` | `endpoint.rs:36-37,366-368` 原样保留 |
| 不改 Stop 取消退避与 `desired_running` 落盘 | `endpoint.rs:370-376`；`core-api/lib.rs:2165-2191` 原样保留 |
| 证据不进跨 Endpoint 结构 | `AttemptEvidence` 在 `attempt_session` 内新建 |

### 1.8 改动清单与规模

| 文件 | 改动 | 约行数 |
|---|---|---|
| `crates/driver-manager/src/endpoint.rs` | `AttemptEvidence`；`event_loop` 形参 + `mark`；`attempt_session` 三元组返回；归零条件；日志补 `saw_runtime_evidence`（可选，不作测试判据） | +30 / -6 |
| `crates/driver-manager/src/event_ingress.rs` | `run_event_ingress` 形参；`BatchProcessor` 字段；`process` 置真 | +8 / -3 |
| 3 处既有调用点 | `session.rs:1847`、`session.rs:2018`、`event_runtime.rs:953` 补 `None` | +4 / -4 |
| `tests/support/test-driver/src/lib.rs` | 新故障键（`ConnFaults`/`Debug`/`parse_conn_faults`/Schema/`run()` 早退）+ 解析单测 | +25 |
| 新套件 `reconnect_backoff.rs` + `stop_lifecycle.rs` 补 L4 | L0 在库内单测；其余见 §6 | ≈ +480 |
| `scripts/contract_suites.py` | 注册新 suite（**该文件改动必须显式评审**，`contract_suites.py:7-9`） | +1 |

生产代码净增 ≈ 40 行，全部在 `driver-manager` 内。

---

## 2. `saw_runtime_evidence` 的精确语义

| 维度 | 定义 |
|---|---|
| 作用域/载体 | **单次 attempt**（`attempt_session` 一次调用 = 一次 spawn→teardown）；载体 `AttemptEvidence(Arc<AtomicBool>)` 在栈上创建，ingress 任务持 clone。不跨 attempt、不跨 Endpoint、不进共享结构 |
| 置真者 | ① `event_loop` 主任务收到 `SessionEvent::Batch`（`endpoint.rs:904-907`）；② ingress 任务在 `BatchProcessor::process` 入口（`event_ingress.rs:171` 之前，覆盖主循环与 final drain） |
| 语义 | “本次 attempt 的消费路径至少从 Driver 收到过 1 个上行批次”；质量无关（BAD 也算）、有效性无关（invalid/duplicate 也算）、数量无关（1 批即真） |
| 读取点 | `attempt_session` 收尾、`teardown_attempt` 返回后读 1 次（`endpoint.rs:568-578`）；唯一消费者是 `endpoint.rs:336-361` 的 `Lost` 臂 |
| 重置/内存序 | 结构性重置（每次新建，无清零代码可漏）；`Relaxed` 成立的前提是**读取晚于两个写者的任务终止**（§1.5），同步由 Join 提供，不依赖原子序 |
| 生命周期 | 创建/交付/置真/停止/Join/读取/销毁七阶段契约见 §1.4；只由本次 attempt 的 ingress 持有，不跨 attempt 累计 |
| 不包含 | 不含“提交成功/入库成功/质量 GOOD”任何含义；epoch 过滤由 `EventReceiver` 在 `session.rs:1317-1334` 完成 |
| 否定式 | `== false` **不等于**“连接失败/驱动撒谎”，只表示“本次 attempt 未观察到数据面通过”，可能是慢首采、空计划、事件空闲等合法情形（§5.1/§5.2、§8） |

不变式（实现自检）：`saw_runtime_evidence ⇒ had_running_session`。理由：两条置真路径都只存在于 Start 之后的代码里（`event_loop` 在 `endpoint.rs:542` 才被调用；ingress 虽在 `endpoint.rs:483-492` 提前 spawn，但 SDK 的 release gate 保证 Start Ack 前不产生批次，`driver-sdk/src/lib.rs:1514-1533,1566-1581`），且事件面还要过 Core 侧 epoch 门。故该布尔**不会**在“未进入 Running”的 attempt 上为真——10 个 `had_running_session: false` 的构造点无需携带它。

---

## 3. Data 与 Event 是否合并统计

**结论：合并为一个布尔。** 取舍如下：

| 方案 | 优点 | 缺点 | 采用 |
|---|---|---|---|
| 单布尔（Data 或 Event 任一命中） | 判定式就是一个合取；不诱导“按类型加权/配阈值”；代码最小 | 日志里看不出是哪种证据触发（可用 2 个字段的日志补，见 §7-C3） | **是** |
| 两个计数分开 | 归零归因更细 | 3 份额外状态；最危险的是**诱导**出“Data 权重高/Event 需 N 批”这类阈值语义，与审查“不得引入阈值”直接冲突 | 否 |

判据本质是析取（任一数据面通道通过即算运行），拆开只服务于诊断，而诊断已有独立通道（`EventDiagnostics` 全局计数 + `Snapshot` 批次延迟统计 `endpoint.rs:952-969`）。

---

## 4. 缺口 1 结论：仅 EventBatch 的 Endpoint

**结论：`EventBatch` 必须计入运行证据，且必须走 attempt 局部取值路径。**

- 若只计 `DataBatch`：事件型端点（OPC UA 事件订阅；`tasks` 为空、`event_tasks` 非空）永远 `saw_runtime_evidence == false`，每次丢失都升级退避。这会让一个**今天正常快速恢复**的场景（审查 §7 约束一）在修复 NCK 时被误伤——不可接受。
- 正确取值路径（唯一）：`attempt_session` 建 `AttemptEvidence` → `handle()` clone 进 `run_event_ingress` → `BatchProcessor::process` 入口置真 → teardown 后读。
- **禁止**的三条捷径及原因：
  1. 读 `services.diagnostics.ingress_batches_total`：全局累计、跨 Endpoint、跨 attempt，无法归因（`event_ingress.rs:171-172`）。
  2. 读 `Snapshot` 的任意计数：`Snapshot` 是跨 Endpoint 共享结构（审查 §5.1 已否决同类做法）。
  3. 让 `event_loop` 自己收事件批次：`EventBatch` 走独立可靠流，`SessionEvent` 枚举里根本没有事件变体（`session.rs:109-122`），改造它等于重做事件面，明确超范围。

---

## 5. 缺口 2 / 缺口 3

### 5.1 缺口 2：慢首采 Endpoint（A1 的适用边界）

**A1 不宣称“没有批次 = 连接失败”。** 精确定义：

- A1 只在**发生 `Lost` 时**评估证据；会话健康（不丢）时退避根本不参与，慢首采端点不受任何影响。
- 误差只出现在：“一次 attempt 在拿到首个批次之前就丢失”**且**这种短命会话**反复发生**。此时 A1 判“无证据”并升级退避。
- 保守行为定义：**重连节奏变慢，但连接与采集逻辑不变**；每次 attempt 仍然真实执行 `spawn→Open→Configure→Apply→Start`，一旦某次会话活到首个批次，**下一次**丢失即归零回 1s（归零看的是“本次 attempt 是否见到证据”，不是“历史是否见过”）。
- 误差上界：最坏多等一个退避档，即 ≤30s（`endpoint.rs:36-37`）。对“轮询间隔 > 会话寿命”的端点，其有效采样周期本来就 ≥ 该间隔，30s 的额外重连延迟不会改变其业务节奏量级。
- **明确不用阈值私了**：不引入 `T`、不引入“连续 N 批”、不引入 `running_since`（审查 §7 约束二）。该误差应由“Driver 上报权威连接状态”（方案 B）消除：那时“已连接”由驱动侧判定，与是否已采到样解耦。

### 5.2 缺口 3：零任务 Endpoint

**事实**：

- 空 `tasks` 是**合法配置**，且被显式解释为“清空”（`core-api/lib.rs:242-251`；事件任务同理 `core-api/lib.rs:331-341`）。Start 路由不因空任务拒绝（`core-api/lib.rs:2136-2163`）。
- 空计划时：NCK `run()` 的 `JoinSet` 无成员，循环立即结束并 `Ok(())`（`drivers/sinumerik-nck/src/lib.rs:885-986`）；test-driver 同理（`tests/support/test-driver/src/lib.rs:1389-1492`）。
- SDK 把 `Ok(())` 报为 `Stopped`、`Err` 报为 `Failed`（`crates/driver-sdk/src/lib.rs:1536-1544`）；Core 把两者都转 `Lost(had_running_session = true)`（`endpoint.rs:908-915`）。

**结论**：

1. **Start 契约不变**：零任务端点仍应 Start 成功（不得改成 fail-closed，那会破坏“清空任务后保留端点”的既有语义与 UI 流程）。
2. **当前行为**：每个 attempt 秒级返回 `Stopped` → `Lost` → 归零 → 1s 退避 → 无限循环（实测 ~3.58s/次，审查 §3.3）。
3. **A1 后的行为**：无任何批次 ⇒ 0 证据 ⇒ `1,2,5,10,30,30…` 收敛。**这是改善而不是退化**：稳态重连频率 ≤2 次/分钟（爬升期首分钟约 6 次），对“无点可采”的端点资源开销可接受。
4. 但“零任务端点永远无法稳定停在 RUNNING”是**既有状态机缺口**，A1 不修也不该修（修它要改 Start 契约或引入 IDLE 终态）。建议生命周期审查单独立项：二选一——(a) 空计划 Start 返回终态（非重连循环）；(b) Configure 层 fail-closed。二者都是契约变更，本轮不批。
5. 特例：零数据任务 + 非空事件任务 = 事件型端点，事件到达即证据（§4），不落入上述循环。

---

## 6. B. L0–L6 测试设计（逐门）

### 6.0 观测手段（三层，均不依赖 `MESA_RECONNECT_FAST`）

| 观测 | 手段 | 确定性 |
|---|---|---|
| O1 计划退避值 | 捕获本进程 tracing 日志中的 `retry_in_secs`（`endpoint.rs:369`）与 `connection lost`（`endpoint.rs:346`） | 确定性（字符串精确） |
| O2 attempt 起始时间线 | 测试侧实现 `PointIdSource` 装饰器，记录每次 `assign()` 的时间戳（每个走到 Configure 的 attempt 调用一次，`endpoint.rs:777`），经 `MesaManager::with_source` 注入（`manager.rs:383`） | 确定性（无需轮询） |
| O3 成功 attempt 次数/新流 | `Snapshot::endpoint(ep).epoch` 变化（`endpoint.rs:528-536`、`snapshot.rs:92-94`） | 轮询有漏采风险，仅作旁证 |

日志捕获 helper（约 40 行，放在新套件文件内，不进 `common/`）：`tracing_subscriber::fmt().with_writer(...)` + `Arc<Mutex<Vec<u8>>>`，按行解析 `endpoint=<id>`、`retry_in_secs=`（`endpoint.rs:369`）与 `connection started`（`endpoint.rs:846`）。**测试只依赖既有日志字段**；若实现额外补 `saw_runtime_evidence`/`backoff_idx`，只能作为诊断冗余，不得成为任何用例的通过条件。注意 `common::init_log()` 用 `try_init`（`tests/driver-contract/tests/common/mod.rs:27-35`），本套件先装自己的 subscriber 即可，互不 panic。

### 6.1 L0 —— 退避决策纯函数与边界序列

| 用例 | 构造 | 断言 | 失败暴露 |
|---|---|---|---|
| `reset_requires_runtime_evidence` | 纯函数 `should_reset_backoff(had_running_session, saw_runtime_evidence)`，归零点改为调用它 | `(F,F)=F`、`(T,F)=F`、`(F,T)=F`（防御性）、`(T,T)=T` | 条件被写成 OR、或丢失 `had_running_session` |
| `backoff_sequence_is_frozen_table` | 抽出 `backoff_delay(idx, fast)`；`reconnect_backoff_secs(idx)` = `backoff_delay(idx, fast_from_env())`（env 仍只在一处读） | `idx=0..12, fast=false` → `[1,2,5,10,30,30,…]`；`fast=true` → 全 1；`idx=usize::MAX` 不 panic | 表值或 `min(4)` 边界被改 |
| `backoff_index_progression_matches_table` | 模拟 `endpoint.rs:365-368`：连续 12 次无证据失败 | 计划值序列 = `[1,2,5,10,30,30,…]`，`idx` 终值 10，序列非递减 | 递增/封顶被误改（如把 `+=1` 写成 `=min(4)`，30s 档提前） |

位置：`endpoint.rs` 既有 `mod tests`（`endpoint.rs:1048-1107`），`cargo test -p mesa-driver-manager endpoint::`。

#### 6.1b 证据接线单元 Gate（S1–S3，补“标记覆盖正常消费 + final drain”的直接证明）

这三条在**进程内**直接驱动 `run_event_ingress`（现有测试已有同样骨架：`session.rs:1847`、`session.rs:2018`），比端到端用例更早、更确定地固定 §1.4 的生命周期契约：

| 用例 | 构造 | 断言 | 失败暴露 |
|---|---|---|---|
| `evidence_marked_on_normal_consumption` | 起 ingress（健康 Store），投 1 个合法 `EventBatch`，等其落盘 | `evidence.saw() == true` | 标记没接进 `process`，或接在 commit 之后 |
| `evidence_marked_by_final_drain_after_cancel` | 先 `cancel()`，再向 channel 投 1 批（模拟“Stop 时已进入 Core 的旧 epoch Event”） | ingress 走 `try_recv` 排空后退出，且 `evidence.saw() == true` | 只覆盖主循环、漏掉 drain 路径（审查要求“覆盖最终 Drain”） |
| `evidence_absent_when_ingress_receives_nothing` | 起 ingress 后直接 cancel，无任何批次 | 退出后 `evidence.saw() == false` | 标记被误置真（例如从全局诊断计数推导） |

三者的共同前置：`evidence` 句柄由测试创建并 clone 给 ingress，**assert 在 `h.await` 之后**——与生产读取点同序（§1.5），顺带证明“Join 之后读”这一形状可复现。

### 6.2 L1–L6（逐门用例）

| 门 | 用例名 | 构造 | 断言 | 失败时暴露什么 |
|---|---|---|---|---|
| **L1** | `backoff_escalates_when_every_attempt_dies_before_first_batch` | 单 Endpoint（test-driver，1 个 50ms poll 任务），`connection_json = {"fail_before_first_batch_ms":100}`（C1 新键，语义见 §7：Start Ack 之后、0 数据批 0 事件批即失败）；`with_source` 注入 O2，O1 捕获日志 | **权威断言**：前 6 次 `connection lost` 后的 `retry_in_secs` = `1,2,5,10,30,30`（精确整数）。辅助：`detail` 含 `SIMULATED_NO_FIRST_BATCH`；日志出现过 `connection started`（证明先 RUNNING 后失败）；相邻间隔 ∈ `[计划值, 计划值+3s]` | 归零条件仍只看 `had_running_session`（旧码恒 `1,1,1,1,1,1`）⇒ 红。≈50s；可拆“前四档 CI（≈20s）+ 30s 档由 L0/离线演练覆盖”，须评审明确，不得默认放弃 30s 端到端观测 |
| **L2** | `brief_false_running_does_not_loop_at_one_second` | 同 L1，`{"fail_before_first_batch_ms":300}`：每次 attempt 真实进入 `RUNNING`（`endpoint.rs:528-536`）、300ms 后失败、**0 批次** | **权威断言**：`retry_in_secs` 序列前 5 项 = `1,2,5,10,30`（与 L1 同一口径）。辅助（只记录、不作硬门禁）：60s 内 attempt 次数（旧码 ≈17、新码 ≤6，可 `eprintln!`）、第 5 次后的相邻间隔、全程该 Endpoint 无 `points/latest` 数据 | 旧码序列恒 `1,1,1,1,…` ⇒ 权威断言红。**不得**把 attempt 次数当唯一判据：它还含 spawn/握手/失败/清理耗时，跨平台会漂移（审查 §三） |
| **L3** | `recovery_after_real_batches_keeps_fast_reconnect` | `{"crash_after_batches":2}`（既有键，`test-driver:1448-1454`），每次出 2 批后 exit 101 | ① 首 attempt 崩溃前 ≥2 批（`Snapshot` 有最新值）；② 故障后**下一次 `retry_in_secs` = 1**（证据生效，不依赖新增日志字段）；③ 下一次 attempt 间隔 ≤3s（旁证）；④ ≤2 个 attempt 内重新 `RUNNING` 且 `epoch` 变化 | A1 过度收紧（要求 GOOD / 连续 N 批）⇒ 归零不生效，② 变成 2、③ 超界。旧码亦绿，它是“不许误伤”闸门而非红转绿用例 |
| **L4** | `stop_cancels_escalated_backoff_and_marks_desired_false`（放 `stop_lifecycle.rs`，与 `stop_while_reconnecting_is_bounded_and_explicit`（`stop_lifecycle.rs:56-124`）并列） | 同 L1 键；O2 等到第 4 次 attempt 起点（下一次退避 10s）再等 300ms 落进睡眠；`POST /stop` | ① Stop ≤40s 返回；② 终态 `STOPPED`；③ 之后 ≥12s 内 O2 计数不再增长；④ `GET /endpoints/{id}` 的 `desired_running == false` | 退避被改成不可取消的 sleep、或 Lost 分支提前返回跳过 `select!` ⇒ ③④ 红。`desired_running` 落盘已在 `core-api/lib.rs:2165-2191`、`management_api.rs:3040` 冻结，本用例只做联合断言 |
| **L5** | `backoff_and_evidence_are_isolated_per_endpoint` | 同一 Manager 两个 Endpoint：A=`fail_before_first_batch_ms:100`，B=健康 test-driver（50ms poll）；同时 Start | ① 15s 内 A 出现 ≥3 档（1,2,5），B 从未出现；② B 的 `epoch` 不变、`timestamp_ns` 持续推进、`quality=GOOD`；③ A 的 attempt ≥4 时 B 仍 `RUNNING`；④ `stop_endpoint(A)` 后 B 的 state/epoch/值不变 | 证据/计数被放进 `Snapshot`、`EventServices.diagnostics` 或 `static`（审查 §5.1 已否决同类）⇒ A 的失败推动 B 的退避或反之 |
| **L6** | ① 既有 27 套 suite 全绿；② 新增 `normal_start_and_recovery_not_degraded` | ① `python scripts/write-contract-evidence.py` → `target/validation/contract.json`（AGENTS.md 唯一可信证据），重点 `subprocess_recovery`（`subprocess_recovery.rs:147-225`）、`stop_lifecycle`、`fault_tolerance`、`event_runtime`、`event_store_faults`；② 健康端点 Start→首批 ≤2s→`RUNNING`，`crash_after_batches:2` 后 ≤3s 恢复且新 `epoch`，Stop 无新增错误码 | ① `contract.json` 全绿；② 恢复时延与改动前同量级；③ Stop 返回符合 `assert_frozen_stop_code`（`stop_lifecycle.rs:40-54`） | 证据接线引入死锁/额外 await（如在 `event_loop` 里锁 `session_arc`）、`run_event_ingress` 签名改动漏传导致事件面回归 |

### 6.3 专项：Event-only 回归（**P0**，必须证明“故障后计划退避重置为 1s”）

| 项 | 内容 |
|---|---|
| 用例 | `event_only_endpoint_resets_backoff_to_one_second_after_event_evidence` |
| 构造 | 单 Endpoint：`tasks = []`（零数据任务）+ 1 个事件任务（`interval_ms: 50`，generic binding）；启用 `EventServices`（`mgr.set_event_services`，`manager.rs:402-404`）；Store 用 `StoreFaults`（test-hooks，`crates/event-store/src/writer.rs:124-181`）。步骤：① 等**事件确实被 Core 消费**（`rows_of(store, ep)` 非空 **且** `ingress_persisted_events_total > 0`）——这只是前置条件，不是结论；② `fail_commit_at=1`、`fail_sticky=true` 制造 `EVENT_STORE_UNAVAILABLE`（`event_ingress.rs:278-285` → ingress fatal → `endpoint.rs:886-897` → `Lost`）；③ 等下一次 attempt 落进 `RECONNECTING`；④ 重复 ②③ 共 3 次 |
| **硬断言（唯一判据）** | 每次故障后的 `retry_in_secs` **严格 = 1**，连续 3 次（O1 日志，精确整数）；即 `saw_runtime_evidence` 在每次事件故障 attempt 上都为真 |
| 辅助断言 | ① 相邻 attempt 间隔 ≤3s（旁证，不作结论）；② 全程 `tasks=[]`，`assign()` 仍被调用（空描述符），O2 可计数；③ 每次 attempt 都先回到 `RUNNING` 再失败（证明走的是 `event_loop` 而非配置失败） |
| 判别力（红/绿） | 若实现只把 `DataBatch` 记为证据（缺口 1 未关闭），同一用例的 `retry_in_secs` 会变成 `1,2,5` ⇒ **第 2 次即红**。本用例是本设计里唯一能拦截“事件型端点被 A1 误伤”的 Gate，故列为 P0 |
| 不接受的替代断言 | ❌ “`ingress_persisted_events_total` 增长” 或 ❌ “相邻 attempt 间隔 ≤3s” 单独作为通过条件：前者只证明事件入库，不证明退避被重置；后者含启动/清理抖动（审查 §三） |
| 备注 | 该场景下“证据为真但入库持续失败”仍会 1s 级重连（§8 局限 2），属 A1 语义边界，不是本用例的失败条件 |

---

## 7. 三个附带实现决策（明确表态）

### C1 是否新增 test-driver 故障注入键？

**要，且只加一个键：`fail_before_first_batch_ms`（可选整数，毫秒；`0` = 立即失败，与“未设置”不同）。**

**语义（审查要求精确定义，实现不得偏离）**：表示 **`StartConnectionAck` 已成功返回（Core 已置 `RUNNING`）之后、`run()` 尚未发布任何 `DataBatch`/`EventBatch` 之前**的失败。三条硬要求：

1. 生效点必须在 SDK release gate 放行**之后**、`run()` 内部（`driver-sdk:1524-1536` 之后才调 `run`）。在 `open_connection`/`configure` 阶段失败**不可接受**——那样 Core 看不到 `RUNNING`，L1/L2 就没有“假 RUNNING”的证明力。
2. 早退位置：`run()` 内 `plan/map` 校验之后、数据任务 spawn 之前（`test-driver:1353-1389`）。事件任务在 `:1471-1488` 才 spawn，故该键天然产生 **0 数据批 + 0 事件批**（不会被 EventBatch 意外置真）。
3. 用例必须自证“先 RUNNING 后失败”：断言日志出现过 `connection started`（`endpoint.rs:846`）或 `Snapshot` 曾为 `RUNNING`，且 `detail`/`code` 为 `SIMULATED_NO_FIRST_BATCH`、`points/latest` 为空。

- 现有 `fail_after_batches:1` 必定先发 1 批（`test-driver:1437-1461`），A1 生效后该批即运行证据 ⇒ 每次归零 ⇒ **不能**用于退避递增用例（审查 §6.1 已指出）；`crash_after_batches:N` 同理需要 N≥1 批。
- 新键实现要点（全部**新增**）：`ConnFaults` 字段（`test-driver:684-713`）、`Debug`（`:717-729`）、`parse_conn_faults`（`:764-777`）、connection Schema 字段（`:102-119`，与 `fail_after_batches` 同形）、`run()` 在组装任务前早退（插在 `:1383-1389` 间，`sleep(ms)` 后返回 `SIMULATED_NO_FIRST_BATCH`）。
- 为什么不用 `fail_after_batches: 0`：该值今天永不触发（计数从 1 开始，`:1449`），赋予新含义属于**改变旧键语义**，评审已划红线；也不用 NCK→`127.0.0.1:1`（依赖本机端口行为与 ~2s 建连延迟，不适合做 CI 确定性 gate，继续作离线演练证据源）。
- 旧键语义**零改动**：解析、触发点、单测（`test-driver:2026-2050`）全部保留。

### C2 如何在不改全局环境变量语义的前提下保证退避测试确定性？

**表态：A1 本轮不动退避参数化；新测试不读、也不设 `MESA_RECONNECT_FAST`。**

四条具体做法：

1. **不再扩大进程级 env 模式**。`reconnect_backoff_secs` 每次调用读 env（`endpoint.rs:32-35`），而 `event_store_faults.rs:145-149` 用 `unsafe { env::set_var }` 设置它——在多线程测试进程里这本身就是数据竞争（Rust 2024 要求 `unsafe` 正是这个原因）。A1 的测试**禁止**沿用该模式。
2. **进程隔离即串扰隔离**：Cargo 的每个 `tests/*.rs` 是独立测试二进制/进程，`event_store_faults` 里的 env 设置不会传播到新套件。新用例集中放在**同一个新文件**（`reconnect_backoff.rs`），该文件内一次也不设该 env ⇒ 不依赖它的任何语义。
3. **断言口径**：用 O1（日志里的计划值 `retry_in_secs`）+ O2（`assign()` 时间线），不用 `loss → loss` 间隔（含 teardown ~0.1-0.3s 抖动，审查 §6.2 已警告）。
4. **容差**：时间断言写成区间 `[planned, planned+3s]` 与“第 N 次 ≥ 第 N-1 次”，不用等值比较；值断言（`retry_in_secs`）用精确整数字符串。

若将来 CI 时间不可接受，正确做法是把退避参数**显式化**（对齐 `HeartbeatParams` 的既有先例，`session.rs:64-87`：env 只在 `Default` 里读一次，测试走显式构造）。但那需要把表从 `u64 秒` 改为 `Duration`，会连带改动 `retry_in_secs` 日志口径与演练脚本的解析约定（`target/validation/l-drill/` 依赖该字段）——**明确不塞进 A1**，列为后续独立改动。

### C3 本轮是否实现 `reconnect_attempt_total` / `current_backoff_ms`？

**不实现。**

- 现状：`reconnect_attempt_total` 在 REST 响应里硬编码 `0`（`core-api/lib.rs:1499-1504`），`current_backoff_ms` 根本不存在。补上它们等于**新增对外可观测语义**（且要定义“attempt 从哪一刻算起”“reset 后归零还是累计”），与本轮“只改判定”的范围冲突，也让生命周期审查要重新冻结一份字段语义。
- 测试不依赖它们：L1/L2/Event-only 的判据由 O1（既有 `retry_in_secs` 日志）精确取得，attempt 时间线由 O2（测试侧 `PointIdSource`）取得，成功次数由 `epoch` 旁证。
- 日志字段属**可选诊断冗余**（非 API、非测试判据）：可在既有 `connection lost`（`endpoint.rs:346`）与 `reconnecting`（`endpoint.rs:369`）里补 `saw_runtime_evidence`/`backoff_idx` 便于离线排障，但**不得**有任何一个用例把它当作通过条件（裁决：使用现有日志与测试侧时间戳）。
- 结论：`reconnect_attempt_total`/`current_backoff_ms` 与方案 B（驱动上报权威连接状态）一起设计，届时 reconnect 语义本身要重定义。

---

## 8. 这个改动在什么情况下仍会失效（诚实局限）

1. **每次都能出 1 批、然后立刻死**：证据恒为真 ⇒ 恒归零 ⇒ 仍是 1s 级重连风暴。这是 A1 语义的必然（“数据面通过”不等于“连接稳定”）。只有引入稳定窗口/权威连接状态（方案 B，本轮明确不做）才能消除；**不得**用“首批后必须活 N 秒”之类阈值绕过（审查约束二）。
2. **证据为真但故障域在 Core 侧**（如 §6.3 的 Store 持续故障）：驱动→Core 数据面通，A1 会保持快速重连。重连修不了存储故障，反而放大日志/CPU。这属“重连原因分类”问题，A1 不解决。
3. **零任务且零事件的端点**：永远无证据 ⇒ 永远在 ≤30s 的循环里（§5.2）。A1 让它变安静，但端点永远不会稳定停在 RUNNING —— 既有状态机缺口，需生命周期审查决定终态或 fail-closed。
4. **慢首采 / 事件空闲端点**：反复在首批发前丢失时被判无证据而升级退避（§5.1）。误差方向是“慢”，不是“连不上”，上限 30s。
5. **保守性误差（可接受）**：teardown 窗口内刚进 socket、reader 尚未 pump 完的批次可能漏判；Start 之前的所有失败（`endpoint.rs:433-464,716-844`）永不归零（本来如此）。二者都只会让恢复更慢，不会更快。
6. **人为反复 Start/Stop**：每次 Start 是新任务、`backoff_idx` 从 0 开始（`endpoint.rs:265`）；风暴若来自编排侧重启，A1 无能为力。另：若驱动违约在 StartAck 前发批次，数据面会被记为证据（SDK release gate `driver-sdk:1514-1533` 已堵住，属实现约束而非当前风险）。

> **验收残余（必须逐字写入 Draft PR 说明与验收记录，不得只在设计文档里）**
> A1 缓解的是“**无运行证据**下的无限快速重连”；它**不是**所有重连风暴的根治。
> **每次 attempt 都成功产生一批数据、随后失败**的端点，仍会因证据为真而把退避重置为 **1 秒**（§8 局限 1）。
> 该残余只能由方案 B（Driver 上报权威连接状态）或生命周期策略（稳定窗口，本轮明确不引入阈值）消除。
> 验收不得以“重连风暴已全部消失”作为通过条件；通过条件只能是 L1/L2 的计划退避序列、L3 的不误伤、Event-only 的 P0 证明。

---

## 9. 待确认

| # | 待确认 | 影响 |
|---|---|---|
| 1 | L1 的 CI 时长取舍：完整 ~50s（1/2/5/10/30 端到端）还是“前四档 CI（≈20s）+ 30s 档由 L0 表与离线演练覆盖” | 只影响 CI 时长与证据分布；两种都必须以 `retry_in_secs` 计划值为权威断言（裁决已固定） |
| 2 | 是否允许新增 suite 文件并在 `scripts/contract_suites.py` 注册（该文件声明“增删 suite 必须显式评审”） | 决定 L1/L2/L3/L5/Event-only 能否进基线证据链 |
| 3 | `saw_runtime_evidence` 是否也写进 `AttemptOutcome::Lost`（本设计选择三元组返回值，理由是 16 构造点 + teardown 后取值） | 若评审要求枚举内携带，需改 16 处（约 +20 行），判定语义不变 |
| 4 | L2 的 attempt 次数指标是否保留为 `eprintln!` 软观测（本设计：不作为门禁） | 纯观测，无判定影响 |

已随本版裁决关闭的两项：**日志新增字段**（不强制；测试只用既有 `retry_in_secs`/`connection started`，新增字段只能作诊断冗余）、**事件面故障注入手段**（Event-only P0 用既有 `StoreFaults` 制造 ingress fatal，不新增 `crash_after_event_batches` 键）。

## 10. 实施顺序与验收

0. **先合并本文件（design-only）**，作为可独立追溯的设计基线；实现另开 Draft PR（§11）。
1. L0 纯函数 + 表（先红后绿，`cargo test -p mesa-driver-manager`）。
2. A1 接线（`endpoint.rs` + `event_ingress.rs` + 3 处调用点），先跑 S1–S3 证据接线单元 Gate（证明正常消费/final drain/无误报），再跑 `cargo test -p mesa-driver-manager` 与 `cargo test -p mesa-contract-tests --test stop_lifecycle --test event_runtime`。
3. test-driver 新键（C1，按 §7 语义）+ 解析单测。
4. 新套件 L1/L2/L3/L5/Event-only(P0)；`stop_lifecycle.rs` 补 L4；注册 suite（待确认 2 通过后）。
5. 离线演练（审查 §6.1 的 L2 层）用真实表复核 `retry_in_secs = 1/2/5/10/30` 与 NCK→`127.0.0.1:1` 路径；**核验被测 mesad 的 `MESA_RECONNECT_FAST` 为空**（审查 §6.2）。
6. 全量 `python scripts/write-contract-evidence.py` 出 `target/validation/contract.json`，作为唯一准入证据（AGENTS.md）。

## 11. 交付与合并安排（审查 §六）

- **设计与实现分离**：本文件先以 design-only 变更提交并合入可追溯主线；修复以**单独 Draft PR** 实施，PR 描述引用本文件路径与本分支的 design commit SHA，并给出实现 commit SHA，使“设计—实现—证据”三者可按 SHA 对应。
- **PR 必附四项**：① 代码差异；② **红/绿证据**（逐条列出“旧码红、新码绿”的用例：L1、L2、Event-only P0；以及“旧码亦须绿、新码不得红”的用例：L3、L4、L5、L6）；③ `target/validation/contract.json`；④ 远程 CI 状态。
- **明确不授权**：自动合并、创建 Release Tag、部署、启动真实 828D Endpoint。T2a 真实单点采集与本项 P2（`target_entry_options`）维持 **HOLD**；它们的前置条件见 `nck-s7-job-0x8201-diagnosis-plan.md` §16.5（Watcher 判据须为 `points/latest` 新鲜数据，且确认 `MESA_RECONNECT_FAST` 未设置）。
- **验收口吻**：以 §8 末尾的“验收残余”为准——A1 不等于重连风暴根治。

## 12. 编码授权范围（审查 §五）

| 允许改动 | 禁止改动 |
|---|---|
| `crates/driver-manager/src/endpoint.rs`（attempt 局部证据 + 归零条件 + 计划值日志口径不变） | RUNNING 的对外展示语义（`endpoint.rs:528-536` 与 UI） |
| `crates/driver-manager/src/event_ingress.rs`（证据句柄形参 + `process` 置真） | IPC/Wire、Driver 协议、既有错误码 |
| 3 处既有调用点补 `None` | 用 GOOD/BAD 质量作为重置依据 |
| `tests/support/test-driver` 的 `fail_before_first_batch_ms`（只增不改旧键） | 引入任何稳定运行时长/证据阈值 |
| 新增测试套件、`stop_lifecycle.rs` 补 L4、`contract_suites.py` 注册（需显式评审） | Stop / `desired_running` 语义；真实 NCK Driver 的采集行为；状态 API 两个计数器 |

## 13. 实施状态（归档说明）

> 本文档为 **A1 修复的设计归档**，记录已实施并合并的设计。后续不再修改本文档；若设计需调整，另文记录。

### 13.1 已实施并合并

| 项 | 值 |
|---|---|
| 实现 PR | **#121** |
| Merge Commit | **`38a2c53d55c3da9055c4d9600c552e04f75370f2`** |
| 代码树 | `566b7a319f32024dd9e33a6b1a63cbf501b61dc4` |
| 实施范围 | 7 文件：`endpoint.rs` / `event_ingress.rs` / `session.rs`（仅测试）/ test-driver 新故障键 / 新测试套件 `reconnect_backoff` / `event_runtime.rs` 调用点 / `contract_suites.py` 最小注册 |

实施结果与本文设计一致：`AttemptEvidence` 每次 Attempt 新建、Data/Event 任一到达消费路径即置真（早于 Sequence Gate）、teardown 后读取、不跨 Attempt/Endpoint；归零条件改为 `should_reset_backoff(had_running_session, saw_runtime_evidence)`。

### 13.2 门禁与验证

| 项 | 结果 |
|---|---|
| 本地聚焦 | `mesa-driver-manager` 49 passed；`reconnect_backoff` 6 passed；`clippy -D warnings`、`fmt --check` 通过 |
| 远程 CI（PR #121） | **10/10 全部 success**（含 `rust-canonical` 1093 项、`rust-perf`、三平台、stress） |
| **合并后离线复验（L1/L2）** | **PASS**：L1 `[1,2,5,10,30,30]`（6/6 `saw_runtime_evidence=false`）；L2 `[1,2,5,10,30]`（5/5）；Stop 3.5/3.7ms、`desired_running=false`、35s 内 0 新 attempt；生产 PID 与库哈希前后一致 |

### 13.3 稳定性观察项

main push CI 首次在 `windows-x64` 失败（`event_runtime::exact_500_events_pipeline_exact_within_budget`、`event_persistence::stop_barrier_drains_inflight_epoch_events`），**同一代码树重跑全绿**。

正式记录为**间歇性失败、根因未确定**；**不得**据此宣称与本次修复无关（两用例均涉及事件处理，而 `event_ingress.rs` 在本轮修改范围内）。同类失败再次出现 ⇒ 立即启动事件处理与排空时序专项调查。

### 13.4 本文档**不**证明的事

- **不证明 `S7_JOB_0x8201` 根因已消除** —— 那是协议层作业拒绝，与本修复无因果关系；
- **不保证"每次产出一批后立即失败"不触发快速重连** —— 这是 A1 的语义边界；
- **不改变 `RUNNING` 的对外语义** —— 真正的"设备已连接"仍待未来由 Driver 上报权威连接状态；
- **不保证所有故障场景下 Stop 都在毫秒级完成** —— 离线复验只覆盖已测路径。
