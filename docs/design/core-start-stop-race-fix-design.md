# Core Start/Stop 生命周期竞态修复设计（G7 前置）

- **状态**：设计稿（未实施，不含代码改动）
- **基线**：`main = fda7633a6d675c43f5bab58f662667aac4a54daf`（本文件编制时工作树位于 `docs/828d-phase1-first-sampling-application`，与 `main` 的差异仅为本轮 828D 文档，代码与 `main` 一致）
- **上游依据**：`docs/validation/828d-phase1-first-limited-sampling-application.md` §4.7（G7-1/2/3）、§5.3（竞态机制、Stop 时间约束、六项判据）
- **约束**：不改 wire/IPC；不改 A1 退避语义；不重构生命周期；不启动服务/设备、不跑测试与构建

---

## 0. 结论摘要

1. 竞态**真实存在**，且不止一处：Stop 的幂等分支（`crates/core-api/src/lib.rs:2169-2175`）与 Start 成功路径（`lib.rs:2153-2156`）之间没有任何"意图排序"保护，两个 handler 并发执行时**后提交者胜**。
2. 最关键、也最容易被现场触发的窗口不是文档 §5.3 描述的那一个，而是 **Stop 拆解期间**：`MesaManager::stop_endpoint` 在 await 之前就把运行条目从 map 中移除（`crates/driver-manager/src/manager.rs:535`），因此整个拆解期（典型 ≈35 s，文档实测 75–80 s，尾部无界）内 `is_running()` 恒为假（`manager.rs:459-466`），任何此刻到达的 Start 都能通过检查并**并行拉起第二个 attempt**。
3. 修复方向：**Stop 生成号（generation）+ Stop 在途计数 + Start 两阶段提交（admit/commit）**，把"改 `desired_running`"与"真正 start/stop"放进同一个同步临界区，使旧序号的 Start 自毁、Stop 在途时的新 Start 直接失败。
4. 不触碰 wire/IPC，不改驱动，不改退避表；变化仅限 REST Start 在竞态分支的状态码（200 → 409）。

---

## 1. 事实基线（均已逐行核对）

| # | 事实 | 位置 |
|---|---|---|
| F1 | Start handler 先查 `is_running`，再 `await` 构造配置，再 start、再置 `desired_running=true` | `crates/core-api/src/lib.rs:2140`、`:2149-2152`、`:2153-2156` |
| F2 | Stop handler 在未运行时**直接**置 `desired_running=false` 并返回 200（幂等分支） | `lib.rs:2169-2175`（置假在 `:2171`） |
| F3 | Stop handler 在运行中时 `await` 拆解，**之后**置假（成功/失败都置假） | `lib.rs:2179-2180`、`:2182-2190` |
| F4 | `is_running` = "map 里有条目且 JoinHandle 未结束" | `crates/driver-manager/src/manager.rs:459-466` |
| F5 | `stop_endpoint` **先 remove 条目**，再 cancel，再 await 任务 | `manager.rs:534-545`（remove 在 `:535`，await 在 `:540`） |
| F6 | `stop_endpoint` 注释声称"返回后新旧 attempt 不重叠"——只对**返回后**的 Start 成立，对**拆解期间**的 Start 不成立 | `manager.rs:530-533` |
| F7 | `start_endpoint` 的"检查—spawn—insert"跨两次加锁，insert 为覆盖写 | `manager.rs:484`、`:503`、`:513-516` |
| F8 | 配置构造是长 `await`：读库 → 任务门禁 → **懒加载 Descriptor（可能起临时驱动进程）** → Event 门禁 | `crates/core-api/src/runtime.rs:165-199`、`:137`；`manager.rs:579-599` |
| F9 | 退避表 `[1,2,5,10,30]` 只对"本次 attempt 连续失败"递增；每个新 attempt 的 `backoff_idx` 从 0 起，且"收到过任一批次"即归零 | `crates/driver-manager/src/endpoint.rs:48`、`:315`、`:416-418`、`:59-61` |
| F10 | `Snapshot::upsert_endpoint` 是**最后写入者胜**，无 attempt/代次保护 | `crates/driver-manager/src/snapshot.rs:208-213` |
| F11 | 活跃会话表以 `endpoint_id` 为键：attempt 建立时 insert，拆解时 remove | `endpoint.rs:562-567`、`:728` |
| F12 | 一个 attempt 进入 RUNNING 后**只在进入时写一次**状态，事件循环期间不再刷新 | `endpoint.rs:597-605` |
| F13 | 无任何"期望态 vs 运行态"对账/看门狗；`desired_running` 只在开机恢复与 API 输出被读 | `apps/mesad/src/main.rs:140-161`、`lib.rs:1495`、`:1563` |
| F14 | Bootstrap 建档即启动也走"start 成功后置 `desired_running=true`"，与 F1 同构 | `crates/core-api/src/bootstrap.rs:110-125`（置真在 `:117`） |
| F15 | `desired_running` 落盘是单条 UPDATE（全局 conn 锁，但不是业务排序） | `crates/config-store/src/lib.rs:1178-1189` |

**Handler 阶段模型**（下文用 S/T 引用）：

- Start：`S0` 准入（`is_running` 检查）→ `S1` 构造（长 `await`）→ `S2` 提交（start + 置真）→ `S3` 应答
- Stop：`T0` 准入（`is_running` 检查）→ `T1a` 幂等置假 / `T1b` 拆解 `await` → `T2` 置假 → `T3` 应答

---

## 2. 竞态时序（三个窗口）

### W1：Stop 落进 Start 的构造窗口（文档 §5.3 描述的那一个）

```
Start : [S0 is_running=false]--[S1 构造: 读库/门禁/Descriptor ...]--[S2 start + desired=true]--[200]
Stop  :                          [T0 is_running=false]--[T1a desired=false]--[200 was_running=false]
结果  : 端点 RUNNING、desired_running=true；Stop 返回 200 且它什么也没停过
```

- **覆盖点**：`T1a`（`lib.rs:2171`）写下的 `false` 被 `S2`（`lib.rs:2155`）覆盖。
- **触发条件**：`T0` 落在 `S1` 内。窗口宽度 = 配置构造耗时。F8 里 Descriptor 冷缓存要起临时驱动进程（启动预算 6 s，`crates/driver-manager/src/session.rs:302`；GetDescriptor 自身 5 s 预算，`session.rs:689`），叠加 SQLite 与任务门禁，**毫秒级到十几秒**。
- 828D 现场对应的正是它：watchdog 的 Start 用 `-TimeoutSec 10`（文档 §5.1-3），客户端超时后**服务端 handler 仍在跑**；`T0+30 s` 的 Stop 若撞上慢构造即命中。

### W2：Start 落进 Stop 的拆解窗口（**最危险、窗口最大**）

```
Stop  : [T0 is_running=true]--[T1b: remove 条目 → is_running 立刻变假 → 拆解 35s~80s+ ...]--[T2 desired=false]--[200 was_running=true]
Start :                              [S0 is_running=false]--[S1 构造]--[S2 新 attempt 启动 + desired=true]--[200]
结果  : 两个 attempt 并存；Stop 返回后端点仍 RUNNING，且 desired_running=false
```

- **覆盖点**：F5 的"先 remove 再 await"使 `is_running()` 在整个拆解期恒假（`manager.rs:535` → `:459-466`）；`S0` 因此必然放行。
- **窗口宽度**：整个 teardown。注释给出的预算是 post 5 s + terminate 10 s + reader 5 s + drain 15 s ≈ 35 s（`endpoint.rs:634-637`），文档实测典型 75–80 s（§5.3），且强杀后第二次 `child.wait()` 无 timeout（`crates/driver-manager/src/process.rs:122-127`、`:143-154`）⇒ **尾部无界**。
- **后果比 W1 更重**：W1 只是"Stop 空转"，W2 会**同时存在两个 attempt / 两个驱动进程 / 两条到机床的会话**，并互相破坏状态（见 §4）。

### W3：双 Start 并发（同族，但会造成"Stop 只停掉一个"）

- 两个 Start 都通过 `S0`，都到 `S2`；`start_endpoint` 的检查（`manager.rs:484`）与 insert（`:513-516`）跨两次加锁，第二次 `insert` **覆盖**第一个 `RunningEntry`：第一个 attempt 的 `JoinHandle` 被丢弃（任务被 detach，继续跑）、cancel token 被丢弃（不再可取消），`stop_endpoint` 只能移除并停止**后一个**。
- 于是 Stop 返回 200 时，孤儿 attempt 仍在按任务周期打机床，直到进程退出（`shutdown` 父 token 取消才连带取消，`manager.rs:497`、`:555-577`）。
- 该窗口需要真正的多线程并发（生产 `#[tokio::main]` 默认多线程，`apps/mesad/src/main.rs:17`）；单线程测试 harness 复现不到——**待确认**其现场可达性（两个客户端/两个浏览器标签/超时重试均可制造）。

---

## 3. 必答 1：触发条件、必然性、跨请求还是同请求

| 问题 | 结论 |
|---|---|
| 到达顺序 | 只要 `T0` 落在 `S1`（W1）、或 `S0` 落在 `T1b`（W2），就发生。**两种到达顺序都能出问题**，不是"先 Start 后 Stop 才安全"。 |
| 必然还是偶发 | **偶发（交错依赖），不是必然**。它不依赖"Stop 失败"——Stop 每次都返回 200。 |
| 概率取决于 | ① Stop 到达时刻相对 Start 的延迟；② `S1` 宽度（Descriptor 冷/热、主机负载、SQLite 争用）；③ 拆解时长（对端是否响应 Shutdown、是否走到强杀）；④ 是否有第二个客户端在此期间发 Start。W2 的窗口是整段拆解（数十秒），**只要有人在 Stop 期间点 Start 就必然命中**；W1 需要慢构造，概率低但非零。 |
| 跨请求 / 同请求重试 | 竞态只存在于**两个 handler 执行之间**，单个 handler 不会自竞态。但"同一逻辑请求"的**超时重试**会产生第二个 handler 执行，因此**同一请求重试同样能触发**（W3，或重试的 Start 撞上并发 Stop 形成 W2）。828D 场景甚至不需要重试：客户端 10 s 超时后服务端仍在跑，即已构成 W1 的在途 Start。 |
| 与 `is_running` 的关系 | `is_running` 是**瞬时**判断，不是"意图"判断；它既不能表达"Stop 已受理"，也不能表达"有 Start 在途"。这是全部三个窗口的根因。 |

---

## 4. 必答 2：影响面

**① 现场安全（最重）**
- W2/W3 会在 Stop 已返回 200 之后继续与机床通信（S7 102 / NCK 连接），监护人依据"Stop 200 + `was_running=true`"判断"已停"会**误判**。
- 两个 attempt 并存时，两份任务表会同时驱动同一点位；事件面两个 ingress 写同一 endpoint 的事件流（可能触发 sequence/幂等类失败）。
- 更隐蔽：F12 表明 RUNNING 只在 attempt 进入时写一次，而老 attempt 的拆解会写 STOPPED（`endpoint.rs:357-366`）。若老 attempt 的 STOPPED 晚于新 attempt 的 RUNNING 落盘（F10 最后写入者胜），**快照会长期显示 STOPPED 而实际仍在采集**——直到新 attempt 下一次状态变更。这直接击穿 §5.3 判据 ②③ 的可信度。

**② 状态一致性**
- `desired_running=false` 与 `runtime.state=RUNNING` 可长期共存（F13：无对账、无看门狗）。重启后因 `desired_running=false` 不再恢复，所以"自愈"只发生在重启，当前会话内不会。
- 活跃会话表（F11）以 `endpoint_id` 为键：老 attempt 拆解时 `remove` 会删掉新 attempt 刚注册的会话（或反之），Control/Browse 可能指向已失效会话——**间歇性**、难复现。
- `desired_running` 写失败被 `let _` 吞掉（`lib.rs:2155`、`:2171`、`:2180`），DB 异常时同样产生矛盾（现状，非本次竞态）。

**③ 其他同样能覆盖 Stop 的路径**

| 路径 | 是否覆盖 Stop | 位置 | 处理 |
|---|---|---|---|
| 自动重连 | **不是**覆盖 Stop 的"意图"，但会让 Stop 之后仍有连接：attempt 内部循环只在循环顶与退避 sleep 处检查取消；配置闭环 `run_config_flow` **没有 shutdown 参数、无取消点**（4 个 RPC × `REQUEST_TIMEOUT` 10 s，`session.rs:25`），Stop 后仍可能走到 `StartConnection(new epoch)` | `endpoint.rs:329-341`、`:428-434`、`:775-782`、`:903-917` | 属文档 §5.3-⑥ 已登记的相邻缺陷，**本设计不修**（见 §7），但门不得宣称能消除它 |
| 心跳 | 否（心跳只影响会话判死，不启停） | `endpoint.rs:258-260` | 无需处理 |
| `desired_running` 恢复 | 开机恢复在 REST 起来之前串行执行，**不与 Stop 并发**；但 bootstrap 的"建档即启动"与 REST Start 同构，窗口一致 | `apps/mesad/src/main.rs:140-161`（serve 在 `:170`）；`bootstrap.rs:110-125` | bootstrap 必须一并接入门 |
| 双 Start（W3） | **是**：Stop 只停掉 map 里的一个，孤儿继续跑 | `manager.rs:513-516` | 由门的"提交时 `is_running` 复核"顺带封死 |
| `DELETE /endpoints/{id}`、`PUT /tasks`、`POST /tasks` 的 `is_running` 门 | 非覆盖，但同族 TOCTOU：拆解期 `is_running` 为假，允许删端点/改任务，而旧 attempt 仍在用旧任务跑 | `lib.rs:2113`、`:2240`、`:2297` | 本设计**不修**，登记为后续独立项 |

---

## 5. 必答 3：候选方案

### 方案 A（推荐）：Stop 生成号 + Stop 在途计数 + Start 两阶段提交

**契约**（新组件 `LifecycleGate`，建议落在 `crates/core-api/src/lifecycle.rs`；`AppState` 增一个 `Arc<LifecycleGate>` 字段）：

```
Entry { generation: u64, inflight_stops: u32 }     // per endpoint_id，std Mutex 保护
admit_start(id)  -> Ticket{gen} | Err(StopInProgress)      // inflight_stops>0 ⇒ 立即失败
commit_start(t, cfg) -> Ok | Err(Superseded|StopInProgress|AlreadyRunning)
                   // 临界区内依次：gen 比对 → inflight_stops==0 → manager.is_running 复核
                   //               → manager.start_endpoint(cfg) → set_desired_running(true)
admit_stop(id)   -> Ticket                      // generation += 1；inflight_stops += 1
finish_stop(t)                                  // set_desired_running(false)；inflight_stops -= 1
```

**为什么这样就闭合**：
- Start 的票据带"准入时的 generation"；Stop 准入即 `generation += 1` ⇒ 任何在 Stop 之前准入、之后提交的 Start **必然**自毁（W1 封死）。
- Stop 在途期间 `inflight_stops > 0` ⇒ 拆解期到达的 Start **在准入处**就失败，不必等 80 s（W2 封死）。
- 提交与 `is_running` 复核、`start_endpoint`、置真在**同一临界区** ⇒ 双 Start 只有一个能提交（W3 封死）。
- 置假（Stop）与置真（Start）都在同一把锁下 ⇒ `desired_running` 的写入顺序与真实启停顺序一致，不再有"后提交者盲胜"。
- Stop 用**计数**而非布尔：并发 Stop 时，只有最后一个 `finish_stop` 才把计数清零，避免"先返回的 Stop 提前解除 Stop 优先"。
- 幂等语义不变：未运行时仍走"置假 + 200 `was_running=false`"，只是改在 `finish_stop` 里落盘。
- 500 语义不变：拆解 `Err` 仍返回 500 `STOP_FAILED`，`desired_running` 仍落假（`lib.rs:2180` 的现有行为保留）。

**关键实现纪律**：
- 临界区内**只允许同步、有界、无 await** 的操作（一次 spawn + 一次 UPDATE，`config-store/src/lib.rs:1183` 是全局 conn 锁、ms 级）。**禁止**在临界区内调 `build_runtime_endpoint`（它会 await 且可能起进程，F8）。
- `admit_stop` 用 RAII guard 兜底减计数（handler panic 时不能永久卡死 Start），guard 只减计数与记日志，不写库。
- 建议在 `finish_stop` 里加一条"`is_running` 仍为真则告警"的断言式日志（只观测，不改变行为），作为门的自检。
- `commit_start` 内 `manager.start_endpoint` 返回 `Err`（如 `EVENT_STORE_UNAVAILABLE`、驱动不可启动）时，必须原样透传为现有 400 `START_FAILED`，且**不得**置 `desired_running=true`（保持 `lib.rs:2154-2161` 的现有分支语义）。
- 入口改造点三处：`lib.rs:start_endpoint`、`lib.rs:stop_endpoint`、`bootstrap.rs:start_created_endpoint`。开机恢复（`main.rs:140-161`）在 serve 之前串行执行，可不接入，但建议接入以保持"唯一路径"。

| 维度 | 评价 |
|---|---|
| 改动面 | 新增 1 个模块 + `AppState` 1 个字段 + 3 个入口改写；`MesaManager` 签名不动（避免数十处既有测试调用点的 churn） |
| 跨驱动影响 | 无：门与协议、驱动、Descriptor 无关，S7/FOCAS2/OPC UA/Simulator 一致 |
| 回归风险 | 中低：唯一的**行为变化**是竞态分支下 Start 由 200 变 409；`mesa-web` 对 start/stop 只读 `status` 与 `error.message`，不匹配 code（`apps/mesa-web/src/api.ts:146-147`、`deviceModel.ts:446-450`），其"stop→apply→start"是**顺序 await**（`deviceModel.ts:474-494`），不会自撞 |
| A1 退避 | 不受影响：门在 manager 之上，`backoff_idx` 的生成/归零语义（`endpoint.rs:315`、`:416-418`）一字未改；反而**减少**了一次"新 attempt 从 0 档重来"的伪重连 |
| wire/IPC | 不改 |

**错误码**：HTTP 仍用 409（与"已在运行"同状态码），body 形状不变，只增两个 `code` 字符串——`STOP_IN_PROGRESS`（拆解期到达）、`SUPERSEDED_BY_STOP`（在途 Start 被 Stop 超越）。理由：测试与现场证据需要**机器可判别**"Stop 优先拒绝"与"已在运行"，而 body 结构、路由、字段均不变，不构成 breaking change。**备选**（若评审坚持零新增码）：统一复用 `CONFLICT`，仅靠 message 区分——但会让回归断言依赖文案，不推荐。

### 方案 B：整段串行化（每端点一把 async 锁，覆盖整个 handler）

- 改动面最小（锁住 `lib.rs:2136-2191` 两个函数）。
- **否决理由 1（安全）**：锁不区分方向。Stop 会排在在途 Start 的构造之后（最长十几秒），而 Stop 是安全方向，§5.3 要求 Stop 到达即受理；用普通 Mutex 无法给 Stop 优先级。
- **否决理由 2（不满足 G7-2）**：排队等待的 Start 在 Stop 完成后照样把端点拉起来——只是把"立刻覆盖"变成"延迟覆盖"，Stop 依然被覆盖。
- 否决理由 3：持锁跨 80 s teardown，任何未来的重入都会变成死锁；且会改变 Stop 的延迟特性，冲击 `stop_lifecycle.rs:109`（40 s 有界）与 `reconnect_backoff.rs:465` 的既有 Gate。

### 方案 C：时间戳/行版本墓碑（Stop 记 `last_stop_ns` 或 `updated_at_ns`，Start 提交前比对）

- 改动面小，无需新组件，复用 `endpoints.updated_at_ns`（`config-store/src/lib.rs:1186`）。
- **否决理由 1**：判据与"spawn"无法原子化——即使 `UPDATE ... WHERE updated_at_ns=?` 的 CAS 成功，Stop 的置假仍可落在 CAS 之后、spawn 之前/之后，矛盾依旧（W1 只被"概率性"缩小）。
- 否决理由 2：无法表达"Stop 优先"（没有"Stop 在途"这一状态），拆解期到达的 Start 仍会成功（W2 完全不覆盖）。
- 否决理由 3：时间/版本是跨线程弱序证据，测试无法做"旧码必红"的确定性门；且任何无关的行更新（改名、改连接）都会误伤 Start。
- 结论：只能作为**辅助审计字段**，不能作为修复主体。

---

## 6. 必答 4：回归测试设计

**稳定复现手段**（不赌调度）：
1. **W2 确定性复现**：复用仓库既有的 commit 栅栏 `StoreFaults::arm_commit_gate()`（`tests/driver-contract/tests/stop_lifecycle.rs:200-211` 已在用）。布防后 Stop 被**确定性**挂住 ≥12 s，而 `is_running` 从 remove 那一刻起恒假（F5）⇒ 此刻发 Start：旧码必 200 且拉起新 attempt（红），新码必 409（绿）。这是本设计的**红/绿主门**。夹具说明：该文件已有 EventServices 与栅栏，需再补 `ConfigStore` + `mesa_core_api::router`（`management_api.rs` 已有 router/`oneshot` 夹具，同一 integration crate 共享 dev-deps），不改 `scripts/contract_suites.py`。
2. **W1 不变量复现**：并发发起 Start 与 Stop（不指定谁先），断言**不变量**：*Stop 返回 200 之后，端点不得处于运行态、`desired_running` 不得为真*。循环 N=20；对任意交错都必须成立。旧码在构造较慢的机器上会命中，新码恒成立——定位为"不变量门"，不声称确定性红。
3. **算法层确定性单测**：直接对 `LifecycleGate` 做状态机测试（无 HTTP、无时序）。

**用例清单**

| 层 | 用例名（建议） | 断言点 | 旧码/新码 |
|---|---|---|---|
| 单测 | `admit_start_rejected_while_stop_inflight` | `inflight_stops>0` 时 `admit_start` 返回 `StopInProgress`，且**未**触碰 `desired_running` | 新码绿（旧码无此 API） |
| 单测 | `in_flight_start_superseded_by_stop_admission` | admit_start → admit_stop → commit_start 返回 `Superseded`，`manager.is_running` 仍假、`desired_running` 仍假 | 同上 |
| 单测 | `stop_priority_holds_until_last_teardown` | 两次 admit_stop、finish 其一后 Start 仍被拒；finish 第二个后 Start 放行 | 同上 |
| 单测 | `start_after_stop_completed_succeeds` | Stop 全部完成后 admit/commit 成功，`desired_running=true` | 同上 |
| 单测 | `two_starts_only_one_commits` | 两个 Start 同代次准入，第一个 commit 成功，第二个返回 `AlreadyRunning`（保持现有 409 文案语义） | 同上 |
| 集成 | `start_during_stop_teardown_is_rejected_and_does_not_resurrect`（`stop_lifecycle.rs`） | 栅栏挂住 Stop 期间 `POST /start` → 409 + `code=STOP_IN_PROGRESS`；`gate.release()` 后 Stop 返回 200 `was_running=true`；随后 `mgr.is_running==false`、`snapshot.state==STOPPED`、`desired_running==false` | **旧红 / 新绿** |
| 集成 | `stop_then_concurrent_start_never_leaves_running`（`management_api.rs`） | 并发 Start/Stop 循环 20 次；每次收敛后 `!is_running && !desired_running`；若 Start 得到 409 则其 code ∈ {`STOP_IN_PROGRESS`,`SUPERSEDED_BY_STOP`} | 旧偶红 / 新恒绿 |
| 集成 | `stop_idempotent_when_never_running_still_200` | 从未启动时 `POST /stop` → 200、`was_running=false`、`desired_running=false`（**现有语义冻结**） | 两码皆绿（防回归） |
| 集成 | `stop_500_still_sets_desired_false` | 注入 drain 失败使 Stop 返回 500 `STOP_FAILED` 时，`desired_running` 仍为 false（`lib.rs:2180` 语义不得被门改掉） | 两码皆绿 |
| 集成 | `start_conflict_while_running_still_409` | 运行中再 Start → 409（现有文案语义不变） | 两码皆绿 |
| 集成 | `bootstrap_start_does_not_override_concurrent_stop` | 建档即启动与 Stop 并发时，最终 `desired_running=false` 且未运行 | 旧红/新绿 |

**"没破坏"的证明清单**（必须与修复同 PR 提交）：
- 正常 Start：`management_api.rs` 现有"段 4 手动启动"（`:2872-2932`）全绿——启动成功、RUNNING、点表、`desired_running=true`。
- 正常 Stop：现有"段 5 停止"（`:3026-3058`）全绿——200、`was_running=true`、STOPPED、`desired_running=false`，且 `assert_changed_within` 的"只允许 desired_running/updated_at_ns 变化"仍成立（门不新增持久化列）。
- Stop 幂等：`event_persistence.rs:668`（`stop_endpoint` 二次调用 `Ok(false)`）与新增 HTTP 幂等用例。
- A1 退避递增：`reconnect_backoff.rs` 全套（L1 `1,2,5,10,30,30`、L2 `1,2,5,10,30`、L4 `stop_cancels_escalated_backoff_and_stops_retrying`）全绿；**不新增/不修改** `endpoint.rs` 退避相关代码。
- L3 稳定运行后恢复：`recovery_after_real_batches_keeps_fast_reconnect`（`reconnect_backoff.rs:326`，定义见 `docs/design/p1-a1-saw-runtime-evidence-design.md:315`）保持"下一次 `retry_in_secs=1`"——门不参与 attempt 内部，期望零差异。
- 冻结码：Stop 的 `Err` 仍属冻结集合（`stop_lifecycle.rs:41-49`）；新增码只出现在 **Start 的 HTTP 响应**，不进入该集合。
- 证据口径：`python scripts/write-contract-evidence.py` 产出的 `target/validation/contract.json`（本文档不代跑）。

---

## 7. 必答 5：明确不做

1. **不重构生命周期**：不引入"期望态 vs 运行态"对账器/看门狗（它本身会启停端点，对现场更危险）；不改 attempt 状态机、不改 teardown 预算。
2. **不改 wire/IPC**：`driver-protocol`、protobuf、驱动进程协议、REST 路由与字段一律不动；唯一变化是竞态分支下 Start 的状态码 200→409。
3. **不改 A1**：`endpoint.rs:44-50`、`:315`、`:416-418`、`:59-61` 一字不改；不调退避档位、不改归零判据。
4. **不新增持久化列**：`generation` 只存内存，避免 `endpoints` 表迁移与 `assert_changed_within` 类红线冲突；进程崩溃场景由"重启后按 DB 期望态恢复 + REST 尚未起来"覆盖。
5. **不修** `run_config_flow` 无取消点（Stop 后仍可能 `StartConnection`）——文档 §5.3-⑥ 已作为**观测判据**登记，属独立缺陷，另立设计；本设计不宣称消除它。
6. **不修** `DELETE /endpoints`、`PUT /tasks` 的 `is_running` 同族 TOCTOU（§4-③ 表末行）。
7. **不删除** 828D 的 G7-3（无在途 Start）与 §5.3 的竞态观测：修复只是让竞态**显式失败**（409），现场仍按 `RACE_START_AFTER_STOP` 记录。

---

## 8. 待确认

1. **G7-2 的边界解释**：字面"Stop 置 false 后 Start 不得再置 true"会禁止一切后续启动。本设计采用的可实现解释是"**Stop 受理前已在途的 Start 必须失败；Stop 返回之后新发起的 Start 允许成功**"。需 828D 文档 owner 确认该解释可满足 G7-2。
2. **错误码策略**：新增 `STOP_IN_PROGRESS`/`SUPERSEDED_BY_STOP`（HTTP 仍 409）vs 纯复用 `CONFLICT`。本文推荐前者（可机器判别、不破形状），需 API 契约 owner 确认。
3. **门的归属**：放 `core-api`（本文推荐，改 `AppState` 一处）vs 放 `driver-manager`（可覆盖所有调用方，但要改 manager 公共 API 与大量测试调用点）。若选后者，需先评估 `MesaManager` 的 API churn。
4. **W3 现场可达性**：双 Start 需要真正并发；是否需在 `start_endpoint` 内部补一把"检查+insert"的同锁（1 行级改动）作为纵深防御——若做，属独立小 PR，避免与门耦合。
5. **`desired_running` 写失败**（现被 `let _` 吞掉）是否要升级为告警/返回——影响面超出本竞态，另议。
