# Core Start/Stop 生命周期竞态修复设计（G7 前置）

- **状态**：设计稿（未实施，不含代码改动）
- **基线**：`main = fda7633a6d675c43f5bab58f662667aac4a54daf`；本文件位于 `docs/core-start-stop-race-fix-design`（PR #132，HEAD `c3d8918`），`git diff fda7633 -- crates apps tests` 为空 ⇒ 本文引用的代码与 `main` 完全一致
- **上游依据**：`docs/validation/828d-phase1-first-limited-sampling-application.md` §4.7（G7-1/2/3）、§5.3（竞态机制、Stop 时间约束、六项判据）
- **本轮修订依据**：独立复审冻结的 G7-2 契约（见 §5）与 P0-A~E / P1-F 六项设计漏洞
- **约束**：不改 wire/IPC；不改 A1 退避语义；不重构生命周期；不启动服务/设备、不跑测试与构建

---

## 0. 结论摘要

1. 竞态**真实存在**，且不止一处：Stop 的幂等分支（`crates/core-api/src/lib.rs:2169-2175`）与 Start 成功路径（`lib.rs:2153-2156`）之间没有任何"意图排序"保护，两个 handler 并发执行时**后提交者胜**。
2. 最关键、也最容易被现场触发的窗口不是文档 §5.3 描述的那一个，而是 **Stop 拆解期间**：`MesaManager::stop_endpoint` 在 await 之前就把运行条目从 map 中移除（`crates/driver-manager/src/manager.rs:535`），因此整个拆解期（典型 ≈35 s，文档实测 75–80 s，尾部无界）内 `is_running()` 恒为假（`manager.rs:459-466`），任何此刻到达的 Start 都能通过检查并**并行拉起第二个 attempt**。
3. 修复方向不变：**Stop 生成号 + Stop 在途保护 + Start 两阶段提交（admit/commit）**。但本轮修订纠正了两个根本性错误：
   - **Stop 在途保护必须归属于"实际生命周期操作"（attempt 任务本身），不能归属于 HTTP 请求**。原稿 §5 让 handler 用 RAII guard 减计数，一旦客户端断开/超时导致 handler 被取消、而 teardown 仍在后台跑，guard 会提前释放保护、**重新开放 W2**（§6.3）。
   - **`is_running()` 不能作为"停干净"的完成判据**：它在拆解开始前就变假（`manager.rs:535`），且 W3 孤儿根本不在运行表里（`manager.rs:513-516`）。判据必须换成"代次化的 attempt 表 + 代次化快照"（§1.2、§6.5、§7）。
4. 变化仅限 Core 内部：`driver-manager` 新增代次化 attempt 表与快照代次门（`snapshot.rs`/`endpoint.rs`/`manager.rs`/`process.rs`），`core-api` 新增薄门 `LifecycleGate`；**不碰 wire/IPC、不加持久化列、不改 REST 字段**。唯一对外的行为变化是竞态分支下 Start 的状态码 200 → 409（带两个可机器识别的 code）。

---

## 1. 事实基线（均已逐行核对）

### 1.1 代码事实

| # | 事实 | 位置 |
|---|---|---|
| F1 | Start handler 先查 `is_running`，再 `await` 构造配置，再 start、再置 `desired_running=true` | `crates/core-api/src/lib.rs:2140`、`:2149-2152`、`:2153-2156` |
| F2 | Stop handler 在未运行时**直接**置 `desired_running=false` 并返回 200（幂等分支） | `lib.rs:2169-2175`（置假在 `:2171`） |
| F3 | Stop handler 在运行中时 `await` 拆解，**之后**置假（成功/失败都置假） | `lib.rs:2179-2180`、`:2182-2190` |
| F4 | `is_running` = "map 里有条目且 JoinHandle 未结束" | `crates/driver-manager/src/manager.rs:459-466` |
| F5 | `stop_endpoint` **先 remove 条目**，再 cancel，再 await 任务 | `manager.rs:534-545`（remove 在 `:535`，cancel 在 `:539`，await 在 `:540`） |
| F6 | `stop_endpoint` 注释声称"返回后新旧 attempt 不重叠"——只对**返回后**的 Start 成立，对**拆解期间**的 Start 不成立 | `manager.rs:530-533` |
| F7 | `start_endpoint` 的"检查—spawn—insert"跨两次加锁，insert 为覆盖写 | `manager.rs:484`、`:503`、`:513-516` |
| F8 | 配置构造是长 `await`：读库 → 任务门禁 → **懒加载 Descriptor（可能起临时驱动进程）** → Event 门禁 | `crates/core-api/src/runtime.rs:165-199`、`:137`；`manager.rs:579-599` |
| F9 | 退避表 `[1,2,5,10,30]` 只对"本次 attempt 连续失败"递增；每个新 attempt 的 `backoff_idx` 从 0 起，且"收到过任一批次"即归零 | `crates/driver-manager/src/endpoint.rs:48`、`:315`、`:416-418`、`:59-61` |
| F10 | `Snapshot::upsert_endpoint` 是**最后写入者胜**，无 attempt/代次保护 | `crates/driver-manager/src/snapshot.rs:208-213` |
| F11 | 活跃会话表以 `endpoint_id` 为键：attempt 建立时 insert，拆解时 remove | `endpoint.rs:562-567`、`:728`；表定义 `manager.rs:364-366` |
| F12 | 一个 attempt 进入 RUNNING 后**只在进入时写一次**状态，事件循环期间不再刷新 | `endpoint.rs:597-605` |
| F13 | 无任何"期望态 vs 运行态"对账/看门狗；`desired_running` 只在开机恢复与 API 输出被读 | `apps/mesad/src/main.rs:140-161`、`lib.rs:1495`、`:1563` |
| F14 | Bootstrap 建档即启动也走"start 成功后置 `desired_running=true`"，与 F1 同构 | `crates/core-api/src/bootstrap.rs:110-125`（置真在 `:117`） |
| F15 | `desired_running` 落盘是单条 UPDATE（全局 conn 锁，但不是业务排序） | `crates/config-store/src/lib.rs:1178-1189` |
| F16 | **`run_endpoint` 的 STOPPED 写入发生在 teardown 之后**：`attempt_session` 先 `teardown_attempt`（`:638-647`）再返回，`run_endpoint` 才在 Shutdown 分支写 STOPPED（`:357-368`）。这是现有代码里**唯一可信的"停止完成"写序** | `endpoint.rs:638-647` → `:357-368` |
| F17 | `teardown_attempt` 的完成边界是**有明确末步**的：post Shutdown（`SHUTDOWN_POST_TIMEOUT`）→ `terminate_after_shutdown` → reader barrier（5 s）→ ingress drain → `sess.invalidate()` → `registry.remove` → `drop(events)` | `endpoint.rs:696-731`（`:709-711`、`:713`、`:716`、`:723`、`:726`、`:728`、`:729`） |
| F18 | `terminate_after_shutdown` **返回即子进程已被回收**（`child.wait()` 成功，或强杀后 `child.wait().await`） | `crates/driver-manager/src/process.rs:143-154`；强杀路径 `:122-129`（无 timeout 的 `wait` 在 `:127`） |
| F19 | 进程收敛有三重兜底：`kill_on_drop(true)`（`:74`）、stdin EOF 防护（`:37`、`:45`）、Windows Job Object `KILL_ON_JOB_CLOSE`（`:7`、`:375`） | `process.rs:74`、`:37`、`:45`、`:7`、`:375` |
| F20 | `wait_reader_done` 的**结果被丢弃**，只用于一条 warn 日志，未上报给 Stop 调用方 | `endpoint.rs:716-721` |
| F21 | `Snapshot::remove_endpoint` 直接删 `endpoints` 条目（DELETE 端点路径），**无墓碑** | `snapshot.rs:426-443`（`:440`）；调用点 `lib.rs:2125` |
| F22 | 存在绕过 Manager 直接 spawn attempt 的路径（测试夹具），该 attempt 不在任何运行表内 | `tests/driver-contract/tests/subprocess_recovery.rs:186` |
| F23 | `run_endpoint_with_heartbeat` 是 `pub`，被测试直接调用；`run_endpoint` 的唯一生产调用点是 Manager | `endpoint.rs:262`、`:298`；`manager.rs:503`；`subprocess_recovery.rs:186` |
| F24 | `EndpointStatus` 是 `Serialize`，直接作为 `/api/v1/endpoints` 的响应元素 | `snapshot.rs:83-95` |

### 1.2 为什么 `is_running()` 不能作为"停干净"的完成判据（P0-B）

复审指出的三点在代码中全部成立，逐条给出证据：

1. **`false` 出现在 teardown 之前。** `stop_endpoint` 的第一条语句就是 `remove`（`manager.rs:535`），而 await 在 `:540`。remove 之后、await 返回之前的整段（35 s 典型 / 75–80 s 实测 / 尾部无界）里，`is_running()` 必然返回 `false`（`manager.rs:459-466`：map 无条目 ⇒ `false`）。也就是说 `is_running()==false` 有两种互斥含义——"没有 attempt" 与 "attempt 正在拆解"——**不可区分**。
2. **W3 的孤儿根本不在运行表里。** 覆盖写（`manager.rs:513-516`）丢弃前一个 `JoinHandle` 与 cancel token，孤儿 attempt 从此不出现在 `running` 表中：`is_running()` 返回 `false`，`running_ids()` 也不列出它（`manager.rs:468-476`）。用运行表判断"无未完成 attempt"会**漏掉孤儿**。
3. **快照写入无代次保护。** `upsert_endpoint` 是最后写入者胜（`snapshot.rs:208-213`）：老 attempt 拆解时写的 STOPPED（`endpoint.rs:357-368`）可以覆盖新 attempt 的 RUNNING（`endpoint.rs:597-605`）。因此即使 `is_running()` 说"停了"，快照也可能说"停"，而实际仍在采集——**判据与被判据对象共享同一个不可信来源**。

**结论（写进设计纪律）**：`is_running()==false` 是"停干净"的**必要不充分条件**，只能作为 UI/兼容投影，**禁止**出现在任何完成判据、门控判定或回归断言的核心断言里。可信证据必须来自 §6 的**代次化 attempt 表**与**代次化快照**，判定方式见 §7。

### 1.3 Handler 阶段模型（下文用 S/T 引用）

- Start：`S0` 准入（`is_running` 检查）→ `S1` 构造（长 `await`）→ `S2` 提交（start + 置真）→ `S3` 应答
- Stop：`T0` 准入（`is_running` 检查）→ `T1a` 幂等置假 / `T1b` 拆解 `await` → `T2` 置假 → `T3` 应答

修订后（§6）：Start 的 `S0/S2` 合并为"admit（同步）+ commit（同步临界区）"，Stop 的 `T0/T1a/T1b` 合并为"admit（同步：代次 +1 + 置假 + 认领 attempt）"，`T2` 前移到 admit，`T3` 只等结果。

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
- **窗口宽度**：整个 teardown。注释给出的预算是 post 5 s + terminate 10 s + reader 5 s + drain 15 s ≈ 35 s（`endpoint.rs:634-637`），文档实测典型 75–80 s（§5.3），且强杀后第二次 `child.wait()` 无 timeout（`process.rs:122-127`、`:143-154`）⇒ **尾部无界**。
- **后果比 W1 更重**：W1 只是"Stop 空转"，W2 会**同时存在两个 attempt / 两个驱动进程 / 两条到机床的会话**，并互相破坏状态（见 §4）。
- **注意（本轮修订）**：W2 的危险不只在"多一个 attempt"，还在"**保护随请求消失**"。原稿让 handler 持有 Stop 在途计数，客户端一断线（`-TimeoutSec 10` 的现场常态）保护就没了——**修好 W2 的排队逻辑，却把 W2 的窗口按请求取消重新打开**。这正是 §6.3 要解决的问题。

### W3：双 Start 并发（同族，但会造成"Stop 只停掉一个"）

- 两个 Start 都通过 `S0`，都到 `S2`；`start_endpoint` 的检查（`manager.rs:484`）与 insert（`:513-516`）跨两次加锁，第二次 `insert` **覆盖**第一个 `RunningEntry`：第一个 attempt 的 `JoinHandle` 被丢弃（任务被 detach，继续跑）、cancel token 被丢弃（不再可取消），`stop_endpoint` 只能移除并停止**后一个**。
- 于是 Stop 返回 200 时，孤儿 attempt 仍在按任务周期打机床，直到进程退出（`shutdown` 父 token 取消才连带取消，`manager.rs:497`、`:555-577`）。
- 该窗口需要真正的多线程并发（生产 `#[tokio::main]` 默认多线程，`apps/mesad/src/main.rs:17`）；单线程测试 harness 复现不到——**待确认**其现场可达性（两个客户端/两个浏览器标签/超时重试均可制造）。**本轮修订后不再依赖"可达性"论证**：insert-before-spawn 使覆盖写在结构上不可能发生（§6.6）。

---

## 3. 必答 1：触发条件、必然性、跨请求还是同请求

| 问题 | 结论 |
|---|---|
| 到达顺序 | 只要 `T0` 落在 `S1`（W1）、或 `S0` 落在 `T1b`（W2），就发生。**两种到达顺序都能出问题**，不是"先 Start 后 Stop 才安全"。 |
| 必然还是偶发 | **偶发（交错依赖），不是必然**。它不依赖"Stop 失败"——Stop 每次都返回 200。 |
| 概率取决于 | ① Stop 到达时刻相对 Start 的延迟；② `S1` 宽度（Descriptor 冷/热、主机负载、SQLite 争用）；③ 拆解时长（对端是否响应 Shutdown、是否走到强杀）；④ 是否有第二个客户端在此期间发 Start。W2 的窗口是整段拆解（数十秒），**只要有人在 Stop 期间点 Start 就必然命中**；W1 需要慢构造，概率低但非零。 |
| 跨请求 / 同请求重试 | 竞态只存在于**两个 handler 执行之间**，单个 handler 不会自竞态。但"同一逻辑请求"的**超时重试**会产生第二个 handler 执行，因此**同一请求重试同样能触发**（W3，或重试的 Start 撞上并发 Stop 形成 W2）。828D 场景甚至不需要重试：客户端 10 s 超时后服务端仍在跑，即已构成 W1 的在途 Start。 |
| 与 `is_running` 的关系 | `is_running` 是**瞬时**判断，不是"意图"判断；它既不能表达"Stop 已受理"，也不能表达"有 Start 在途"。这是全部三个窗口的根因。 |
| 与请求生命周期的关系（新增） | 竞态的**保护期**同样不能挂在请求上：客户端断开即 handler future 被 drop，任何"在 handler 里减计数/释放锁"的写法都会把保护期截短到"客户端还在"为止（§6.3、§6.7）。 |

---

## 4. 必答 2：影响面

**① 现场安全（最重）**
- W2/W3 会在 Stop 已返回 200 之后继续与机床通信（S7 102 / NCK 连接），监护人依据"Stop 200 + `was_running=true`"判断"已停"会**误判**。
- 两个 attempt 并存时，两份任务表会同时驱动同一点位；事件面两个 ingress 写同一 endpoint 的事件流（可能触发 sequence/幂等类失败）。
- 更隐蔽：F12 表明 RUNNING 只在 attempt 进入时写一次，而老 attempt 的拆解会写 STOPPED（`endpoint.rs:357-366`）。若老 attempt 的 STOPPED 晚于新 attempt 的 RUNNING 落盘（F10 最后写入者胜），**快照会长期显示 STOPPED 而实际仍在采集**——直到新 attempt 下一次状态变更。这直接击穿 §5.3 判据 ②③ 的可信度。

**② 状态一致性**
- `desired_running=false` 与 `runtime.state=RUNNING` 可长期共存（F13：无对账、无看门狗）。重启后因 `desired_running=false` 不再恢复，所以"自愈"只发生在重启，当前会话内不会。
- 活跃会话表（F11）以 `endpoint_id` 为键：老 attempt 拆解时 `remove` 会删掉新 attempt 刚注册的会话（或反之），Control/Browse 可能指向已失效会话——**间歇性**、难复现。（本轮修订后此风险被结构性消除：并发 attempt 被拒绝 ⇒ 同一 `endpoint_id` 不会有两个会话同时注册；见 §6.6。）
- `desired_running` 写失败被 `let _` 吞掉（`lib.rs:2155`、`:2171`、`:2180`），DB 异常时同样产生矛盾（现状，非本次竞态）。

**③ 其他同样能覆盖 Stop 的路径**

| 路径 | 是否覆盖 Stop | 位置 | 处理 |
|---|---|---|---|
| 自动重连 | **不是**覆盖 Stop 的"意图"，但会让 Stop 之后仍有连接：attempt 内部循环只在循环顶与退避 sleep 处检查取消；配置闭环 `run_config_flow` **没有 shutdown 参数、无取消点**（4 个 RPC × `REQUEST_TIMEOUT` 10 s，`session.rs:25`），Stop 后仍可能走到 `StartConnection(new epoch)` | `endpoint.rs:329-341`、`:428-434`、`:775-782`、`:903-917` | 属文档 §5.3-⑥ 已登记的相邻缺陷，**本设计不修**（见 §10），但门不得宣称能消除它 |
| 心跳 | 否（心跳只影响会话判死，不启停） | `endpoint.rs:258-260` | 无需处理 |
| `desired_running` 恢复 | 开机恢复在 REST 起来之前串行执行，**不与 Stop 并发**；但 bootstrap 的"建档即启动"与 REST Start 同构，窗口一致 | `apps/mesad/src/main.rs:140-161`（serve 在 `:170`）；`bootstrap.rs:110-125` | bootstrap 必须一并接入门 |
| 双 Start（W3） | **是**：Stop 只停掉 map 里的一个，孤儿继续跑 | `manager.rs:513-516` | 由 insert-before-spawn + 拒绝并发 attempt 结构性封死（§6.6），**不再依赖"提交时复核"这种事后检查** |
| `DELETE /endpoints/{id}`、`PUT /tasks`、`POST /tasks` 的 `is_running` 门 | 非覆盖，但同族 TOCTOU：拆解期 `is_running` 为假，允许删端点/改任务，而旧 attempt 仍在用旧任务跑；且 `remove_endpoint` 无墓碑（F21），旧 attempt 随后的状态写入会**复活已删除端点** | `lib.rs:2113`、`:2125`、`:2240`、`:2297`；`snapshot.rs:440` | 删端点的 TOCTOU 本设计**不修**；但"旧代次写入不得复活已删端点"由代次门覆盖（§6.5） |

---

## 5. G7-2 冻结契约（评审给定，逐字采用，作为设计基准）

| Start 的发生时机 | 裁决 |
|---|---|
| **Stop 受理前**已经在途、尚未提交成功 | **Stop 优先，旧 Start 必须失败** |
| **Stop 已受理、teardown 尚未完成** | **Start 必须返回冲突，不允许排队后自动启动** |
| **Stop 已真正完成**之后发起全新 Start | 允许正常启动，**但仍须满足新的操作授权** |

- 总体方向批准采用「**Stop 生成号 + Stop 在途保护 + Start 两阶段提交**」。
- **保留** `STOP_IN_PROGRESS`、`SUPERSEDED_BY_STOP` 两个可机器识别的 409 错误码。

**本设计对三行的落地映射**（实现见 §6）：

| 契约行 | 落地机制 | 代码落点 |
|---|---|---|
| 行 1：在途 Start 必须失败 | `commit_start` 比对票据里的 `stop_gen`，与当前值不等 ⇒ `SUPERSEDED_BY_STOP` | §6.4 步骤 1 |
| 行 2：teardown 期 Start 冲突、不排队 | attempt 表条目仍存在（`phase=Stopping`）⇒ `start_endpoint` 同步拒绝 ⇒ `STOP_IN_PROGRESS`，**没有任何队列** | §6.4 步骤 2、§6.6 |
| 行 3：完成后允许全新 Start | 条目由 attempt 任务自身在 teardown 完成后移除 ⇒ 新 Start 走全新 admit + 全新 `build_runtime_endpoint`（新 `stream_epoch`）+ 既有授权门 | §6.4、§6.7 |

**"新的操作授权"的实现解释（需 owner 确认，见 §11-1）**：不复用任何 Stop 之前的票据或配置快照——新 Start 必须（a）重新 `admit_start` 取得当前代次的票据，（b）重新执行 `build_runtime_endpoint`（`lib.rs:2149`，含 Secret 物化与任务门禁），（c）继续受既有授权门约束（`CONTROL_DISABLED` 等不变）。**不是**新增鉴权机制。

---

## 6. 方案 A：Stop 生成号 + Stop 在途保护 + Start 两阶段提交

### 6.1 唯一真值与三层职责

本轮修订最重要的结构决定：**"有没有 attempt、attempt 到哪一步了"只有一个真值来源——`driver-manager` 的 attempt 表；`core-api` 的 `LifecycleGate` 只是排序器与票据池，不持有任何生命周期真值。**

| 层 | 组件 | 持有什么 | 不持有什么 |
|---|---|---|---|
| `driver-manager` | `attempts: Arc<Mutex<HashMap<String, AttemptEntry>>>`（替换现 `running`，`manager.rs:360`） | attempt 是否存在、代次、phase、pid、teardown 证据、结果 | 不碰 DB、不碰 HTTP、不知道 Stop 票据 |
| `driver-manager` | `Snapshot`（`snapshot.rs`） | `(attempt_gen, EndpointStatus)` 原子对，写入按代次过滤 | 不判断"是否停干净" |
| `core-api` | `LifecycleGate`（新 `crates/core-api/src/lifecycle.rs`） | 每 endpoint 的 `stop_gen`、在途 **Start** 票据数、`desired_running` 缓存 + 写入序号 | **不持有** Stop 在途计数（原稿的 `inflight_stops` 删除） |

**为什么删掉 `inflight_stops`**：计数器的"减一"必须发生在"teardown 真正完成"这个事件上，而该事件只发生在 attempt 任务内部。若计数器在 core-api，就必然要么由 handler 减（取消即丢，P0-A 的原始漏洞），要么由 manager 反向回调 core-api（层次倒置）。**把保护直接表达为"attempt 表条目存在"，减一就自动等价于"条目被 teardown 完成者移除"**——不需要任何跨层通知，也不存在"忘了减"的可能。

### 6.2 数据结构

```rust
// driver-manager：一次 attempt 的全部可判定证据（无锁读，供观察与诊断）
pub struct AttemptCtx {
    pub gen: u64,                       // 单调代次，Manager 分配（AtomicU64 per endpoint）
    phase: AtomicU8,                    // Starting → Running → Stopping → Ended
    pid: AtomicU32,                     // 驱动子进程 pid（spawn 后回填；0 = 尚未 spawn）
    session_released: AtomicBool,       // teardown 末尾：registry 已摘除本 attempt 的 session
    remote_eof: AtomicBool,             // teardown 末尾：wait_reader_done 观测到对端 EOF
    outcome: OnceLock<Result<bool, String>>,  // teardown 结论（None 条目 = 干净）
    done_tx: tokio::sync::watch::Sender<Option<Result<bool, String>>>,  // 完成通知
}

struct AttemptEntry {
    ctx: Arc<AttemptCtx>,
    cancel: CancellationToken,
    // 注意：这里**没有** JoinHandle（见 §6.6）
}
```

```rust
// core-api：只做排序与票据
struct EpGate {
    stop_gen: u64,               // Stop 受理即 +1（W1 的唯一判据）
    inflight_starts: u32,        // 已 admit、未 commit/未放弃的 Start 票据数
    desired: Option<bool>,       // 懒初始化自 DB，之后为唯一写者
    desired_seq: u64,            // 每次成功写入 +1（证明"不是旧值"）
    last_stop_seq: Option<u64>,  // 最近一次 admit_stop 时的 desired_seq（§7.3 判据 2 的比对基准）
}
```

**锁序纪律（必须写进代码注释）**：`gate` 锁 → `manager` 锁，**永不反向**。`manager` 的任何路径都不调用 gate（结构上保证），故无死锁。`commit_start` 与 `admit_stop` 都在这条锁序下完成"代次比对 + 生命周期调用"。

### 6.3 Stop 在途保护归属于"生命周期操作"（P0-A）

**作废原稿写法**：上一版 §5「关键实现纪律」曾建议"`admit_stop` 用 RAII guard 兜底减计数（handler panic 时不能永久卡死 Start）"。该写法**必须废弃**，理由即复审所述：

> handler 中途被取消（客户端断开 / 网关超时 / `-TimeoutSec 10` 到点），而实际 teardown 仍在后台执行 ⇒ guard 随 future drop 而提前释放 ⇒ `inflight_stops` 归零 ⇒ **Start 重新放行** ⇒ W2 并发窗口原样重现，而且此时现场更容易触发（客户端超时是常态）。

**新规则（三条，逐条对应复审要求）**：

1. **保护载体 = attempt 表条目**，而不是任何计数器或 handler 栈帧。条目在 `start_endpoint` 里 **spawn 之前**创建（insert-before-spawn），在 attempt 任务 **返回之前** 由该任务自己移除。因此"保护存在" ⟺ "该 attempt 尚未确认 teardown 完成"，二者**是同一件事**，不存在需要同步的副本。
2. **释放条件 = 确认旧 attempt 已完成 teardown**，且释放动作**只由 attempt 任务自身执行**（`AttemptGuard::drop`，见下）。`stop_endpoint` 不释放、handler 不释放、超时不释放。
3. **handler 取消 / 超时 / 连接断开时保护不得释放**：handler 在该路径上只持有一个 `watch::Receiver`（等待结果用）。丢弃它只丢失"应答"，不触碰任何状态；teardown 任务继续跑，条目继续存在，Start 继续得到 409。

**RAII 不是不能用，而是不能用错宿主**：`AttemptGuard` 放在 **attempt 任务内部** 是正确的——它随任务结束（含 panic 展开）而 drop，drop 里只做两件同步事：置 `phase=Ended`、`remove_if_gen_matches(gen)`。若放在 handler 里就是灾难。这是本轮修订对原稿的核心纠正。

```rust
// 只存在于 attempt 任务内部；Drop 在 panic 展开时同样执行
struct AttemptGuard { table: Arc<Mutex<HashMap<String, AttemptEntry>>>, id: String, gen: u64,
                      ctx: Arc<AttemptCtx> }
impl Drop for AttemptGuard {
    fn drop(&mut self) {
        self.ctx.phase.store(Ended);
        // 仅在条目仍属于本代次时移除（防御性：绝不移除别人的条目）
        let mut m = self.table.lock().unwrap();
        if m.get(&self.id).is_some_and(|e| e.ctx.gen == self.gen) { m.remove(&self.id); }
        if std::thread::panicking() { tracing::error!(...); }   // panic 也要留证据
        let _ = self.ctx.done_tx.send(self.ctx.outcome.get().cloned());
    }
}
```

> `Drop` 里不能 await，所以释放路径天然是同步的——这同时满足了"临界区内禁止 await"的纪律。

#### 6.3.1 "teardown 真正完成"的可判定信号（最关键的一条）

**定义**：`TeardownDone(gen)` 当且仅当下面四件事同时成立，且它们**由同一条因果链保证，不需要额外探测**：

| # | 证据 | 为什么可信（代码依据） |
|---|---|---|
| E1 | 该 gen 的 `run_endpoint` 任务已结束 | attempt 任务就是生命周期操作本身；任务结束 ⇒ 它再也不会碰驱动进程、会话、快照 |
| E2 | 任务结束发生在 `teardown_attempt` 返回**之后** | `attempt_session` 在 `endpoint.rs:638-647` 先 `teardown_attempt`，返回后 `run_endpoint` 才写 STOPPED（`:357-368`）⇒ teardown 是 return 的前置 |
| E3 | `teardown_attempt` 返回 ⇒ 驱动子进程已被 `wait()` 回收 | `endpoint.rs:713` → `process.rs:143-154`：`child.wait()` 成功，或强杀后 `child.wait().await`（`:127`，无 timeout，**永不返回即永不"完成"**，见下） |
| E4 | 该 attempt 的 session 已从 registry 摘除 | `endpoint.rs:728` 移除；主判据是 `registry.read().get(endpoint_id).is_none()`——由于并发 attempt 被拒绝（§6.6），该键若存在只可能属于本 attempt，故"键不存在"即"本 attempt 已摘除"，不会读到上一代。辅助判据 `Arc::strong_count(&session_arc) == 1`（**不作主判据**：并发 Control 请求会临时 clone 该 Arc，`manager.rs:841`、`:890`，计数可能短暂为 2；且此时 session 已 `invalidate()`，不具操作能力） |

**信号载体**：`AttemptCtx.phase == Ended`（由 `AttemptGuard::drop` 写入），**并且**该 gen 的条目已从 attempt 表移除。观察者读"条目不存在"即可，`phase` 只用于诊断。

**明确排除的伪信号**：

| 伪信号 | 为什么不可用 |
|---|---|
| `is_running()==false` | 拆解开始即假（`manager.rs:535`），且漏孤儿（§1.2） |
| `handle.is_finished()` | 它只回答"任务是否结束"，而 W3 覆盖写后旧 handle 已被丢弃、无从查询（`manager.rs:513-516`）；且在"任务已结束但条目仍在"的瞬时态下会与"停干净"混淆 |
| 快照 `state=="STOPPED"` | 可被旧代次覆盖（F10），且 `remove_endpoint` 会直接抹掉条目（F21） |
| `desired_running==false` | 是**意图**不是**事实**；旧代码还在 await 之后才写（`lib.rs:2180`），取消即丢 |
| 定时器/预算到期 | teardown 尾部无界（`process.rs:127`），"超时"不等于"已停" |

**尾部无界时的行为（必须诚实声明）**：若 `child.wait()` 因内核/驱动卡死永不返回，则 `TeardownDone` 永不成立 ⇒ 条目永不移除 ⇒ **Start 永远 409**（fail-closed，安全方向）。这不是缺陷而是设计选择：机床场景下"停不干净就不许启动"优于"以为停了又拉起来"。为此必须补一条**诊断**：`stop_endpoint` 记录 `stop_elapsed`，超过 teardown 预算（≈35 s，`endpoint.rs:634-637`）即 `tracing::warn!`，超过 3 倍预算 `tracing::error!` 并附 attempt 表快照，供现场判断是"卡在 wait"还是"卡在 drain"。**不加自动强杀**（会改变 teardown 预算，属 §10 排除项）。

### 6.4 Start 两阶段提交

```rust
admit_start(id) -> Ticket { stop_gen }        // 同步；inflight_starts += 1；不查 is_running、不碰 DB、不起进程
                                              // 票据由 RAII 释放：取消只减计数（此时尚未产生任何外部副作用，安全）

commit_start(t, cfg) -> Ok | Err(Superseded | StopInProgress | AlreadyRunning | StartFailed)   // 同步，单临界区
    1. if gate.stop_gen != t.stop_gen                  -> Superseded        （契约行 1 / W1）
    2. mgr.start_endpoint(cfg)                          // 同步；内部 insert-before-spawn
         Err("STOP_IN_PROGRESS: …")                      -> StopInProgress   （契约行 2 / W2）
         Err("endpoint `x` already running")             -> AlreadyRunning   （W3 / 重复 Start）
         Err(其他，如 EVENT_STORE_UNAVAILABLE)           -> StartFailed      （400 START_FAILED，原样透传）
    3. store.set_desired_running(id, true)              // 仅在 2 成功后；失败记 error（不再 let _ 静默）
```

- **为什么必须同步**：`commit_start` 全程无 `await` ⇒ 客户端取消**无法**落在"代次比对通过、但 start 尚未发生"之间。这是 W1/W2 在取消场景下仍然成立的前提。
- **禁止**在临界区内调 `build_runtime_endpoint`（它 await 且可能起临时进程，F8）；构造留在 `S1`，因此需要票据来检测"构造期间 Stop 已受理"。
- **禁止**在临界区内调 `manager.stop_endpoint`（async）。

Stop 侧（对称设计，同样全同步）：

```rust
admit_stop(id) -> StopHandle          // 同步，单临界区：gate.stop_gen += 1
                                      //                    gate.desired = Some(false) 并落盘（desired_seq += 1）
                                      //                    mgr.begin_stop(id) → 认领/合流 attempt
```

`begin_stop` 是**同步**函数（`manager` 内部）：

```rust
pub fn begin_stop(&self, id: &str) -> StopHandle      // 同步：cancel + 置 phase=Stopping，无 await
    match table.get(id) {
        None                                    => NotRunning,                 // was_running=false
        Some(e) if e.ctx.phase() == Stopping    => Join(e.ctx.done_rx.clone()), // 并发 Stop 合流，不重复拆解
        Some(e)                                  => { e.ctx.phase = Stopping; e.cancel.cancel(); Claimed(e.ctx.done_rx.clone()) }
    }
pub async fn stop_endpoint(&self, id: &str) -> Result<bool, String>   // 签名不变（既有 30+ 调用点零改动）
{ let h = self.begin_stop(id); h.await_result().await }
```

- `begin_stop` 同步 ⇒ **Stop 的"受理"与"启动拆解"不可分割**：不存在"代次已 +1、desired 已写假、但 teardown 没启动"的中间态（原稿把拆解放在 handler 的 await 之后，正好有这个洞）。
- `stop_endpoint` 保留 async 签名 ⇒ `stop_lifecycle.rs:109`、`reconnect_backoff.rs:465`、`event_persistence.rs:668` 等调用点一字不改。

### 6.5 代次隔离：快照写入必须带代次（P0-D）

**问题**：`Snapshot::upsert_endpoint` 最后写入者胜（`snapshot.rs:208-213`）⇒ 旧 attempt 的 STOPPED（`endpoint.rs:357-368`）可覆盖新 attempt 的 RUNNING（`endpoint.rs:597-605`）。

**方案**：把 `endpoints` 的值从 `EndpointStatus` 换成 `(u64, EndpointStatus)` 原子对，写入按代次过滤。

```rust
// snapshot.rs
endpoints: RwLock<HashMap<String, EndpointRecord>>,          // 替换原 HashMap<String, EndpointStatus>
struct EndpointRecord { gen: u64, status: EndpointStatus }
retired: RwLock<HashMap<String, u64>>,                        // 墓碑：DELETE 后记录已退休的最大代次

pub fn upsert_endpoint(&self, gen: u64, status: EndpointStatus) -> bool {
    // gen < 记录代次 ⇒ 丢弃（返回 false，由调用方决定是否 debug 日志）
    // 墓碑：gen <= retired[id] ⇒ 丢弃（禁止复活已删除端点）
}
```

- `EndpointStatus` **不加字段**（F24：它是 REST 响应元素，加字段等于改 wire）⇒ 代次只存在于 map 的键结构里，`endpoint()`/`endpoints()` 的返回类型与序列化结果**完全不变**。
- 写入门禁只在一个地方：`set_status`（`endpoint.rs:447-465`）增加 `gen` 参数；`upsert_endpoint` 的生产调用点只有它（已核实：`crates/` 内仅 `endpoint.rs:456` 一处），churn 极小。
- 代次来源：`AttemptCtx.gen`，由 `start_endpoint` 分配（每 endpoint 一个 `AtomicU64`，与 `ACTIVE_DRIVER_PORTS` 同风格，`process.rs:187`）。`run_endpoint` 需要拿到 ctx ⇒ 新增 `run_endpoint_with_attempt(..., ctx)`，**保留** `run_endpoint_with_heartbeat` 旧签名委托（ctx=None），使 `subprocess_recovery.rs:186` 零改动。
- `remove_endpoint`（`snapshot.rs:426-443`）在删除时写墓碑：`retired[id] = max(retired[id], 当前记录 gen)`，保证 DELETE 之后旧 attempt 的写入被丢弃（F21 的"复活已删端点"）。

**为什么在有了 attempt 表之后还需要它**（诚实论证，避免"多余设计"质疑）：

1. **DELETE 路径真实可达**：`DELETE /endpoints` 在拆解期 `is_running==false` 会放行（`lib.rs:2113`），`remove_endpoint` 直接删条目（`snapshot.rs:440`）；随后旧 attempt 写 STOPPED/RUNNING ⇒ 已删端点"复活"。这条路径**与 attempt 表无关**，只能靠墓碑 + 代次门。
2. **顺序保证是隐式跨函数不变量**："条目移除晚于 STOPPED 写入"依赖 `endpoint.rs:638` 与 `:357` 的相对位置，任何一次重构都可能静默破坏；代次门把它变成**显式可断言**的约束，并有回归测试钉住。
3. **纵深防御**：F22/F23 存在绕过 Manager 直接 spawn attempt 的路径；代次门对它们同样生效（gen=0 的直调 attempt 无法覆盖 gen≥1 的正式 attempt）。

**回归测试**（复审指定）：`concurrent_teardown_and_new_start_never_shows_stopped_while_collecting` —— 见 §9。补充说明：修复后 HTTP 路径上"旧 STOPPED 晚于新 RUNNING"已被 attempt 表**结构性**排除（条目移除晚于 STOPPED 写入），因此该场景的**确定性**红/绿放在 Snapshot 单测（`stale_gen_write_is_dropped`）与 DELETE 复活场景（集成）上；集成层的"不得长期显示 STOPPED 而实际在采集"以**不变量断言**表达（5 s 窗口内 `state==RUNNING` 且 `latest` 时间戳持续推进）。

### 6.6 防孤儿：insert-before-spawn + 拒绝并发 attempt（P0-E）

**现状**（`manager.rs:513-516`）：`insert` 是覆盖写，第二个 Start 会丢弃第一个 `RunningEntry`（JoinHandle + cancel token）⇒ 孤儿打到进程退出。

**方案（三处，缺一不可）**：

1. **insert-before-spawn，且拒绝已存在**：在**同一次**加锁内完成"检查存在 + 插入占位条目"，然后才 spawn。第二个 Start 在**检查处**就被拒绝，**根本没有 spawn 发生**，因此不存在"需要覆盖/终止旧条目"的情况——这比"insert 前显式终止旧条目"更安全（终止一个正在跑机床的 attempt 是危险动作，且会引入新的竞态）。
2. **条目里不放 JoinHandle**：attempt 任务自持 `AttemptGuard`，任务结束时自己移除条目。JoinHandle 可以被 detach 丢弃而**不产生孤儿**——孤儿的前提是"有人还活着但没人管"，而现在"活着"与"有条目"是同一件事。
3. **回填必须校验代次**：`start_endpoint` 在 spawn 之后不再需要回填 handle（无 handle），但若仍需记录 `pid` 等，必须 `if let Some(e) = table.get_mut(&id) { if e.ctx.gen == gen { … } }`，**绝不无条件 `insert`**。条目创建一律走 `Entry::Vacant`（见下），使"覆盖写"在类型层面不可表达；任何退回 `HashMap::insert` 的改动都必须在 review 中被拒绝。

```rust
pub fn start_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {   // 签名不变
    let mut m = self.attempts.lock().unwrap();
    // 用 Entry API 而不是 insert：覆盖写在类型层面不可表达（这是 P0-E 的第一道保证）
    match m.entry(cfg.endpoint_id.clone()) {
        std::collections::hash_map::Entry::Occupied(e) => {
            return Err(match e.get().ctx.phase() {
                Stopping => format!("STOP_IN_PROGRESS: endpoint `{}` is stopping", cfg.endpoint_id),
                _        => format!("endpoint `{}` already running", cfg.endpoint_id),
            });
        }
        std::collections::hash_map::Entry::Vacant(v) => {
            // 其余同步门禁（event store、find_driver）保持原位置：manager.rs:487-495
            let gen = self.next_gen(&cfg.endpoint_id);          // AtomicU64 per endpoint，单调
            let ctx = Arc::new(AttemptCtx::new(gen));
            let guard = AttemptGuard::new(Arc::clone(&self.attempts), cfg.endpoint_id.clone(), gen, ctx.clone());
            v.insert(AttemptEntry { ctx: ctx.clone(), cancel: cancel.clone() });
            drop(m);
            tokio::spawn(async move { let _g = guard; run_endpoint_with_attempt(..., ctx).await; });  // handle 直接丢弃
            return Ok(());
        }
    }
}
```

**"无孤儿"如何验证**：

| 层 | 验证手段 |
|---|---|
| 结构不变量 | 条目创建走 `Entry::Vacant`（`Occupied` 一律拒绝）⇒ 覆盖写在 API 层面不可达；任意时刻每 `endpoint_id` 至多一条目；条目只由 `AttemptGuard::drop`（gen 匹配）移除；spawn 后的任何回填都必须 `if e.ctx.gen == gen` 校验，绝不无条件 `insert` |
| 进程不变量 | 新增全局 `LIVE_CHILDREN: AtomicUsize`（`process.rs`，与 `ACTIVE_DRIVER_PORTS:187` 同风格）：spawn 后 +1、`child.wait()` 返回后 −1。Stop 完成后该值必须回落到"仅剩临时 Descriptor 进程"的基线（0） |
| 端到端 | §9 `double_start_only_one_attempt`：并发两 Start，恰好一个 200/一个 409；随后 Stop，断言无条目、`LIVE_CHILDREN==0`、`!has_live_attempt(id)`；平台侧 `pid` 不再存活（辅助，注意 pid 复用） |
| 基线复用 | 仓库已有 `subprocess_orphan_guard` suite（见 AGENTS.md 名单），新增场景挂在该 suite 内，不改 `scripts/contract_suites.py` |
| 运行期诊断 | `state` 与 attempt 表不一致时 `tracing::warn!`（只读告警，不自动修复） |

**已知边界（必须声明）**：F22/F23——直接调用 `run_endpoint_with_heartbeat` 的路径不经过 attempt 表，**不受本机制保护**。当前只有测试夹具这样用（`subprocess_recovery.rs:186`），生产唯一入口是 `manager.rs:503`。设计中给该函数加 `NOTE` 注释说明"绕过 Manager 即放弃 §7 的全部判据"。

### 6.7 请求取消语义（P1-F）

**统一原则**：**所有状态迁移都在同步函数内完成；所有 `await` 只用于"等待结果"，不承担迁移职责。** 这样取消只丢失应答，不丢失状态。

| 事件 | 旧行为 | 新行为 |
|---|---|---|
| Start handler 在构造期被取消 | 无副作用（尚未提交）；但构造可能已在跑 Descriptor 临时进程 | 无副作用；票据 RAII 释放，`inflight_starts` 归零；**不触碰** desired/attempt |
| Start handler 在提交后被取消 | 已 start、已置真；客户端不知结果 | 同上（提交是同步的，取消只能发生在提交之后）⇒ 状态一致；结果可查（`GET /api/v1/endpoints` 的 `state`） |
| **Stop handler 被取消（断开/超时）** | `lib.rs:2180` 的 `set_desired_running(false)` **永不执行** ⇒ `desired_running` 永久为 true（重启会自动拉起一个用户明确要停的端点） | `desired=false` 已在 `admit_stop`（同步）写入 ⇒ 不受取消影响；teardown 继续跑完；条目由任务自删 ⇒ 保护不提前释放 |
| 客户端拿不到 Stop 结果 | 无补偿 | 结果留在 `AttemptCtx.outcome` 与 `done_tx`：再次 `POST /stop` 会**合流**到同一次 teardown 并返回同一结果（幂等 join），或读到 `was_running=false`（已完成）。同时落 `tracing` 结构化日志 |

**"返回给客户端的结果可以是未知，但服务端状态必须可查"**：可查渠道全部是既有能力——`GET /api/v1/endpoints`（`state`，已代次化）、`GET /api/v1/endpoints/{id}`（`desired_running`，`lib.rs:1495`/`:1563`）、结构化日志。**不新增 REST 路由或字段**（§10）。

### 6.8 错误码

HTTP 仍用 409（与"已在运行"同状态码），body 形状不变，只增两个 `code` 字符串：

| code | 触发条件 | 判定位置 |
|---|---|---|
| `SUPERSEDED_BY_STOP` | 票据 `stop_gen` 与当前不等（Start 在 Stop 受理前 admit、之后 commit） | `commit_start` 步骤 1（契约行 1） |
| `STOP_IN_PROGRESS` | attempt 表存在且 `phase==Stopping`（Stop 已受理、teardown 未完成） | `start_endpoint` 同步拒绝（契约行 2） |
| `CONFLICT`（不变） | 条目存在且 `phase!=Stopping`（真的已在运行） | 同上，沿用现有文案"endpoint 已在运行" |

理由：测试与现场证据需要**机器可判别**"Stop 优先拒绝"与"已在运行"；body 结构、路由、字段均不变，不构成 breaking change。`manager` 侧沿用仓库既有的"精确码前缀"约定（`EVENT_STORE_UNAVAILABLE: …` 在 `manager.rs:489`、`EVENT_DRAIN_FAILED: …` 在 `:543`），故 `Err(String)` 签名不变，core-api 按前缀映射状态码与 code。

### 6.9 取舍评价表

| 维度 | 评价 |
|---|---|
| 改动面 | `driver-manager`：attempt 表 + `AttemptGuard`（`manager.rs`）、`set_status` 加 gen（`endpoint.rs`）、`upsert_endpoint` 加 gen + 墓碑（`snapshot.rs`）、`LIVE_CHILDREN`（`process.rs`）；`core-api`：新增 `lifecycle.rs` + `AppState` 1 字段 + 3 个入口改写（`lib.rs:2136-2191`、`bootstrap.rs:110-125`）。`MesaManager` 的**既有公共签名不变**（`start_endpoint`/`stop_endpoint` 保留），新增 `begin_stop`/`has_live_attempt`/`attempt_gen` 三个查询/同步方法 |
| 跨驱动影响 | 无：门与协议、驱动、Descriptor 无关，S7/FOCAS2/OPC UA/Simulator 一致 |
| 回归风险 | 中低：行为变化只有三处——竞态分支下 Start 200→409（两个新 code）；并发 Stop 由"立即 200 false"改为"合流后返回同一结果"（§11-3）；`desired_running` 的写入时机由"teardown 之后"前移到"受理时"（§11-2）。`mesa-web` 对 start/stop 只读 `status` 与 `error.message`，不匹配 code（`apps/mesa-web/src/api.ts:146-147`、`deviceModel.ts:446-450`），其"stop→apply→start"是**顺序 await**（`deviceModel.ts:474-494`），不会自撞 |
| A1 退避 | 不受影响：门在 attempt 之上，`backoff_idx` 的生成/归零语义（`endpoint.rs:315`、`:416-418`）一字未改；反而**减少**了一次"新 attempt 从 0 档重来"的伪重连。新增的观察器只读，**绝不** cancel/restart attempt（否则等于引入 §10-1 拒绝的对账器） |
| wire/IPC | 不改（`EndpointStatus` 字段不变，代次只在 map 键结构里） |
| 持久化 | 不加列：`generation` 只在内存；`desired_running` 仍是既有单条 UPDATE（`config-store/src/lib.rs:1185`） |

---

## 7. 完整停止判据六条（P0-C）

### 7.1 判据总表

"停干净"= 下列 6 条**同时**成立，并在稳定窗口内持续成立。逐条给出可判定方式、抗旧值机制、当前代码是否具备。

| # | 判据 | 可判定信号（来源） | 如何证明不是旧值 | 当前代码 | 需新增什么 |
|---|---|---|---|---|---|
| 1 | **当前代次为 `STOPPED`** | `Snapshot` 中 `(gen, state)` 原子对，且 `gen == attempt_gen(current)` | 写入按代次过滤（旧代次被丢弃）；读取在**同一次加锁**内取 `(gen, state)`，不可能拼出"新 gen + 旧 state" | **部分具备**：STOPPED 的写入时机已经正确（`endpoint.rs:638-647` → `:357-368`，teardown 之后才写），但 `upsert_endpoint` 无代次门（`snapshot.rs:208-213`） | `EndpointRecord{gen,status}` + 代次门 + 墓碑（§6.5） |
| 2 | **`desired_running=false`** | `store.get_endpoint(id).desired_running`（`config-store/src/lib.rs:1138-1147`）+ gate 的 `desired_seq` | `desired_seq` 单调：观察者记录 Stop 受理时的 seq，要求 `observed_seq >= seq_at_admit` 且值为 false；单写者（gate）保证无第三方写入 | **不具备**：写点在 handler（`lib.rs:2171`/`:2180`），**取消即丢失**；无写入序号 | 把写假前移到 `admit_stop`（同步临界区）+ `desired_seq` + gate 懒初始化自 DB |
| 3 | **无在途 Start / Stop** | 在途 Start = gate `inflight_starts`；在途 Stop = attempt 表存在且 `phase==Stopping` | 两者都是"在途"的**定义本身**，不是缓存副本（无副本可旧） | **不具备**：当前无任何表示 | `LifecycleGate` + attempt 表 |
| 4 | **无未完成的 attempt（含孤儿）** | attempt 表：该 `endpoint_id` 无条目（条目 = 未完成；`Ended` 条目在同一临界区内已被移除） | 条目**只**在 spawn 前创建、**只**由 attempt 任务自身按 gen 匹配移除 ⇒ 不存在"漏登记"的孤儿；gen 单调，读到的 gen 与当前 gen 比对 | **不具备**：`is_running` 在拆解期就假（`manager.rs:535`）、W3 孤儿不在表内（`:513-516`） | 代次化 attempt 表 + `AttemptGuard`（§6.2/§6.6） |
| 5 | **对应驱动进程与目标 TCP 会话已收敛** | (a) `TeardownDone(gen)`（§6.3.1，蕴含 `child.wait()` 已返回，`process.rs:143-154`）；(b) `session_released == true`（teardown 末尾 registry 中该 `endpoint_id` 键不存在，`endpoint.rs:728`）；(c) 全局 `LIVE_CHILDREN` 无该 attempt 的 pid | (b) 因并发 attempt 被拒绝（§6.6），registry 的该键在任一时刻只可能属于当前 attempt ⇒ 不存在"读到上一代"；再叠加 gen 单调比对 | **部分具备**：进程收敛已由 teardown 保证（E3），但 `wait_reader_done` 结果被丢弃（F20，`endpoint.rs:716-721`）、无 registry 释放证据、无全局计数 | `AttemptCtx.session_released/remote_eof/pid` + `LIVE_CHILDREN` |
| 6 | **经过规定的稳定观察窗口** | `gate.await_settled(id, timeout)`：250 ms × 2 个采样（500 ms），每个采样同时读 1~5 条全部来源 | 每 tick 重读全部来源并记录 `gen`；窗口内任一条目/代次/phase 变化即**重置窗口**；旧代次的 STOPPED 写入因代次门被丢弃，即使落盘其 gen ≠ 当前 gen ⇒ 该 tick 不通过 ⇒ 窗口重置 | **不具备**：全仓库无任何观察窗口 | 只读观察器（§7.3） |

**判定式**：

```
Settled(id) ⇔ ∧_{k=1..6} Crit_k(id) 在每个采样 tick 成立，且连续 N=2 个 tick 全部成立
```

**禁止的等价替换**：不得用 `is_running()==false`、`desired_running==false`、`state=="STOPPED"` 中的任一条单独（或任意组合）代替上面 6 条——它们分别缺"代次"、"事实性"、"抗覆盖"。

### 7.2 第 4 条（孤儿）如何避免"快照被旧 attempt 覆盖"的干扰

判据 4 **完全不读快照**，只读 attempt 表。这是刻意的分层：

- 快照是"呈现层"，会被 `remove_endpoint`（`snapshot.rs:440`）、被旧代次写入（F10）、被 DELETE 抹掉；用它判"有没有未完成的 attempt"必然受污染。
- attempt 表是"生命周期层"，条目与"活着的任务"一一对应（insert-before-spawn + 自删），**没有"旧条目覆盖新条目"这种状态**——同名条目的并发创建被拒绝（§6.6），所以表里永远只有一个当前 attempt。
- 孤儿（复审特别强调的一类）在表里表现为"条目存在但对应 endpoint 已不该运行"；判定上它与普通在跑 attempt 无差别（都算"未完成"），因此**不会漏**。唯一漏网是 F22 的直调路径，已显式声明为边界。

### 7.3 第 6 条（稳定窗口）如何避免"快照被旧 attempt 覆盖"的干扰

窗口的每个 tick 是一次**跨来源的一致性快照**，规则如下：

1. 同一次加锁内读 `(snapshot.gen, snapshot.state)`（§6.5 的原子对）；
2. 读 `attempt_gen(current)` 与 `phase`；
3. 要求 `snapshot.gen == attempt_gen(current)`，否则该 tick **直接不通过**（这正是"旧 attempt 的 STOPPED 覆盖"的检测器：被覆盖时 gen 会对不上）；
4. 要求 `phase` 不是 `Starting/Running/Stopping`（即条目已移除）；
5. 要求 `desired==false` 且 `desired_seq >= seq_at_admit`；
6. 要求 `inflight_starts == 0`；
7. 任一 tick 失败或窗口内出现 gen 变化 ⇒ 窗口从 0 重新计时。

因此"快照被旧 attempt 覆盖成 STOPPED、而实际在采集"这个具体故障，**不可能**让判据 6 通过：要么 gen 对不上（第 3 步），要么条目还在（第 4 步），要么 `inflight_starts>0`（新 Start 在途，第 6 步）。

**窗口参数**：`SETTLE_TICKS = 2`、`SETTLE_TICK = 250ms`（合计 500 ms）。选值理由：需大于一次"状态写入 + 一次 REST 读"的抖动（ms 级），又必须远小于 Stop 的 teardown 预算（≈35 s），避免把"停干净"的判定拖成运维不可接受的时长。**待确认**是否需要按 endpoint 配置（§11-6）。

**只读纪律（硬性）**：观察器只读、只记日志与水位，**绝不**写 `desired_running`、**绝不** cancel/restart attempt、**绝不**改快照。一旦它开始"纠正"，就变成 §10-1 明确拒绝的对账器/看门狗。

### 7.4 判定算法（伪代码）

```rust
async fn await_settled(&self, id: &str, timeout: Duration) -> Settled {
    let seq_at_admit = self.seq_at_last_stop(id);          // Stop 受理时记录
    let deadline = Instant::now() + timeout;
    let mut ok_ticks = 0;
    let mut last_gen = None;
    while Instant::now() < deadline {
        let s = self.sample(id);                            // 一次取全部来源
        let same_gen = last_gen.map_or(true, |g| g == s.attempt_gen);
        let pass = s.snapshot_gen == s.attempt_gen
                && s.snapshot_state == "STOPPED"
                && s.entry_absent
                && s.desired == Some(false)
                && s.desired_seq >= seq_at_admit
                && s.inflight_starts == 0;
        ok_ticks = if pass && same_gen { ok_ticks + 1 } else { 0 };
        last_gen = Some(s.attempt_gen);
        if ok_ticks >= SETTLE_TICKS { return Settled { gen: s.attempt_gen, .. } }
        tokio::time::sleep(SETTLE_TICK).await;
    }
    NotSettled { last_observation: … }                      // 附失败原因，供现场定位
}
```

---

## 8. 被否方案（保留原论证）

### 方案 B：整段串行化（每端点一把 async 锁，覆盖整个 handler）

- 改动面最小（锁住 `lib.rs:2136-2191` 两个函数）。
- **否决理由 1（安全）**：锁不区分方向。Stop 会排在在途 Start 的构造之后（最长十几秒），而 Stop 是安全方向，§5.3 要求 Stop 到达即受理；用普通 Mutex 无法给 Stop 优先级。
- **否决理由 2（不满足 G7-2）**：排队等待的 Start 在 Stop 完成后照样把端点拉起来——只是把"立刻覆盖"变成"延迟覆盖"，正是冻结契约行 2 明令禁止的"排队后自动启动"。Stop 依然被覆盖。
- 否决理由 3：持锁跨 80 s teardown，任何未来的重入都会变成死锁；且会改变 Stop 的延迟特性，冲击 `stop_lifecycle.rs:109`（40 s 有界）与 `reconnect_backoff.rs:465` 的既有 Gate。
- 否决理由 4（本轮新增）：锁在 handler 栈帧上 ⇒ 客户端断开即释放 ⇒ 与 P0-A 同类漏洞。

### 方案 C：时间戳/行版本墓碑（Stop 记 `last_stop_ns` 或 `updated_at_ns`，Start 提交前比对）

- 改动面小，无需新组件，复用 `endpoints.updated_at_ns`（`config-store/src/lib.rs:1186`）。
- **否决理由 1**：判据与"spawn"无法原子化——即使 `UPDATE ... WHERE updated_at_ns=?` 的 CAS 成功，Stop 的置假仍可落在 CAS 之后、spawn 之前/之后，矛盾依旧（W1 只被"概率性"缩小）。
- 否决理由 2：无法表达"Stop 优先"（没有"Stop 在途"这一状态），拆解期到达的 Start 仍会成功（W2 完全不覆盖）。
- 否决理由 3：时间/版本是跨线程弱序证据，测试无法做"旧码必红"的确定性门；且任何无关的行更新（改名、改连接）都会误伤 Start。
- 结论：只能作为**辅助审计字段**，不能作为修复主体。

---

## 9. 回归测试设计

**稳定复现手段**（不赌调度）：
1. **W2 确定性复现**：复用仓库既有的 commit 栅栏 `StoreFaults::arm_commit_gate()`（`tests/driver-contract/tests/stop_lifecycle.rs:200-211` 已在用）。布防后 Stop 被**确定性**挂住 ≥12 s，而 `is_running` 从 remove 那一刻起恒假（F5）⇒ 此刻发 Start：旧码必 200 且拉起新 attempt（红），新码必 409 `STOP_IN_PROGRESS`（绿）。这是本设计的**红/绿主门**。夹具说明：该文件已有 EventServices 与栅栏，需再补 `ConfigStore` + `mesa_core_api::router`（`management_api.rs` 已有 router/`oneshot` 夹具，同一 integration crate 共享 dev-deps），不改 `scripts/contract_suites.py`。
2. **W1 不变量复现**：并发发起 Start 与 Stop（不指定谁先），断言**不变量**：*Stop 返回 200 之后，端点不得处于运行态、`desired_running` 不得为真*。循环 N=20；对任意交错都必须成立。旧码在构造较慢的机器上会命中，新码恒成立——定位为"不变量门"，不声称确定性红。
3. **算法层确定性单测**：直接对 `LifecycleGate` 与 `AttemptCtx` 做状态机测试（无 HTTP、无时序）。
4. **取消路径确定性复现（本轮新增，P0-A 主门）**：用 commit 栅栏挂住 teardown，然后**丢弃 Stop 的 future**（等价于客户端断开），断言 (a) teardown 仍在推进、(b) 条目仍在（`has_live_attempt==true`）、(c) 此时 `POST /start` 必 409 `STOP_IN_PROGRESS`、(d) 放行栅栏后条目被移除、`desired==false`、Start 可成功。旧码在 (c) 处必然 200（保护随 handler 消失）。

**用例清单**

| 层 | 用例名（建议） | 断言点 | 旧码/新码 |
|---|---|---|---|
| 单测 | `admit_start_rejected_while_attempt_stopping` | 条目 `phase==Stopping` 时 `start_endpoint` 返回 `STOP_IN_PROGRESS`，且**未**触碰 `desired_running`、未 spawn | 新码绿（旧码无此 API） |
| 单测 | `in_flight_start_superseded_by_stop_admission` | admit_start → admit_stop → commit_start 返回 `Superseded`，无 attempt 产生、`desired` 仍假 | 同上 |
| 单测 | `stop_priority_holds_until_teardown_completes` | 挂住 teardown 期间 Start 恒被拒；完成后放行 | 同上 |
| 单测 | `start_after_stop_completed_succeeds` | Stop 全部完成后 admit/commit 成功，`desired=true`、新 gen > 旧 gen | 同上 |
| 单测 | `two_starts_only_one_attempt` | 两个 Start 同代次，恰好一个成功、一个 `AlreadyRunning`；attempt 表条目数 == 1；`Entry::Occupied` 分支被命中、从未发生覆盖 | 同上 |
| 单测 | `stale_gen_write_is_dropped`（`snapshot.rs`） | 先 `upsert(gen=2, RUNNING)` 再 `upsert(gen=1, STOPPED)` ⇒ 快照仍 `RUNNING`/gen=2 | **旧码无 gen 参数**，等价场景必红 |
| 单测 | `deleted_endpoint_write_is_dropped_by_tombstone` | `remove_endpoint` 后 `upsert(gen<=retired, RUNNING)` 被丢弃，条目不存在 | 新码绿 |
| 单测 | `settle_window_rejects_stale_stopped` | 构造 `snapshot.gen != attempt_gen` 的采样 ⇒ `await_settled` 不通过 | 新码绿 |
| 集成 | `start_during_stop_teardown_is_rejected_and_does_not_resurrect`（`stop_lifecycle.rs`） | 栅栏挂住 Stop 期间 `POST /start` → 409 + `code=STOP_IN_PROGRESS`；`gate.release()` 后 Stop 返回 200 `was_running=true`；随后 `!has_live_attempt`、`snapshot.state==STOPPED`、`desired_running==false`、`await_settled` 通过 | **旧红 / 新绿** |
| 集成 | `stop_protection_survives_handler_cancel` | 丢弃 Stop future 后保护仍在（见上"取消路径确定性复现"） | **旧红 / 新绿** |
| 集成 | `second_stop_joins_inflight_teardown` | 并发两个 Stop：两个都 200 且 `was_running=true`，teardown 只发生一次（drain 计数 1），第二次不返回 `was_running=false` | 旧码：第二个立即 200 false（红） |
| 集成 | `double_start_only_one_attempt` | 并发两 Start：一个 200、一个 409；Stop 后无条目、`LIVE_CHILDREN==0`、驱动 pid 不再存活 | **旧红（孤儿）/ 新绿** |
| 集成 | `concurrent_teardown_and_new_start_never_shows_stopped_while_collecting` | 5 s 内快照 `state==RUNNING` 且 `latest` 时间戳持续推进；`gen` 等于当前 attempt gen | 旧码可长期 STOPPED（红） |
| 集成 | `deleted_endpoint_not_resurrected_by_late_status_write` | DELETE 期间旧 attempt 写 STOPPED/RUNNING 后，`/api/v1/endpoints` 中不得重新出现该 id | 旧码红 |
| 集成 | `stop_then_concurrent_start_never_leaves_running`（`management_api.rs`） | 并发 Start/Stop 循环 20 次；每次收敛后 `!has_live_attempt && !desired_running`；若 Start 得到 409 则其 code ∈ {`STOP_IN_PROGRESS`,`SUPERSEDED_BY_STOP`} | 旧偶红 / 新恒绿 |
| 集成 | `stop_idempotent_when_never_running_still_200` | 从未启动时 `POST /stop` → 200、`was_running=false`、`desired_running=false`（**现有语义冻结**） | 两码皆绿（防回归） |
| 集成 | `stop_500_still_sets_desired_false` | 注入 drain 失败使 Stop 返回 500 `STOP_FAILED` 时，`desired_running` 仍为 false（`lib.rs:2180` 语义不得被门改掉） | 两码皆绿 |
| 集成 | `start_conflict_while_running_still_409` | 运行中再 Start → 409（现有文案语义不变） | 两码皆绿 |
| 集成 | `bootstrap_start_does_not_override_concurrent_stop` | 建档即启动与 Stop 并发时，最终 `desired_running=false` 且无 attempt | 旧红/新绿 |

**"没破坏"的证明清单**（必须与修复同 PR 提交）：
- 正常 Start：`management_api.rs` 现有"段 4 手动启动"（`:2872-2932`）全绿——启动成功、RUNNING、点表、`desired_running=true`。
- 正常 Stop：现有"段 5 停止"（`:3026-3058`）全绿——200、`was_running=true`、STOPPED、`desired_running=false`，且 `assert_changed_within` 的"只允许 desired_running/updated_at_ns 变化"仍成立（门不新增持久化列）。
- Stop 幂等：`event_persistence.rs:668`（`stop_endpoint` 二次调用 `Ok(false)`）——第二次调用发生在第一次完成之后 ⇒ 条目已移除 ⇒ 仍 `Ok(false)`。
- Stop 有界：`stop_lifecycle.rs:109`（40 s）与 `reconnect_backoff.rs:465` 保持绿；`stop_lifecycle.rs:200-223`（栅栏期必须阻塞、放行后 `Ok(true)`）保持绿。
- A1 退避递增：`reconnect_backoff.rs` 全套（L1 `1,2,5,10,30,30`、L2 `1,2,5,10,30`、L4 `stop_cancels_escalated_backoff_and_stops_retrying`）全绿；**不新增/不修改** `endpoint.rs` 退避相关代码。
- L3 稳定运行后恢复：`recovery_after_real_batches_keeps_fast_reconnect`（`reconnect_backoff.rs:326`，定义见 `docs/design/p1-a1-saw-runtime-evidence-design.md:315`）保持"下一次 `retry_in_secs=1`"——门不参与 attempt 内部，期望零差异。
- 直调路径不回归：`subprocess_recovery.rs:186`（直接 `run_endpoint_with_heartbeat`）零改动编译通过、行为不变（旧签名保留）。
- 冻结码：Stop 的 `Err` 仍属冻结集合（`stop_lifecycle.rs:41-49`）；新增码只出现在 **Start 的 HTTP 响应**，不进入该集合。
- 证据口径：`python scripts/write-contract-evidence.py` 产出的 `target/validation/contract.json`（本文档不代跑）。

---

## 10. 明确不做

1. **不重构生命周期**：不引入"期望态 vs 运行态"对账器/看门狗（它本身会启停端点，对现场更危险）；不改 attempt 状态机、不改 teardown 预算。§7.3 的观察器**只读**，不写 `desired_running`、不 cancel attempt。
2. **不改 wire/IPC**：`driver-protocol`、protobuf、驱动进程协议、REST 路由与字段一律不动；`EndpointStatus`（`snapshot.rs:83-95`）**不加字段**，代次只存在于 `Snapshot` 的内部 map 键结构里。唯一对外变化是竞态分支下 Start 的状态码 200→409。
3. **不改 A1**：`endpoint.rs:44-50`、`:315`、`:416-418`、`:59-61` 一字不改；不调退避档位、不改归零判据。
4. **不新增持久化列**：`generation`/`attempt_gen` 只存内存，避免 `endpoints` 表迁移与 `assert_changed_within` 类红线冲突；进程崩溃场景由"重启后按 DB 期望态恢复 + REST 尚未起来"覆盖。
5. **不修** `run_config_flow` 无取消点（Stop 后仍可能 `StartConnection`）——文档 §5.3-⑥ 已作为**观测判据**登记，属独立缺陷，另立设计；本设计不宣称消除它。
6. **不修** `DELETE /endpoints`、`PUT /tasks` 的 `is_running` 同族 TOCTOU（§4-③ 表末行）。注意：`is_running` 的**对外语义保持不变**（拆解期为假），本设计不"顺手"改它——改它会连带改变 DELETE/PUT 的行为，超出本竞态范围。但"旧代次写入复活已删端点"由代次门/墓碑覆盖（§6.5）。
7. **不删除** 828D 的 G7-3（无在途 Start）与 §5.3 的竞态观测：修复只是让竞态**显式失败**（409），现场仍按 `RACE_START_AFTER_STOP` 记录。
8. **不加自动强杀/自动恢复**：`child.wait()` 无界时保持 fail-closed（Start 一直 409）并大声告警，不引入"超时就当停了"的判定（§6.3.1）。

---

## 11. 待确认

1. **"新的操作授权"的确切含义**（契约行 3）：本文按"重新 admit + 重新 `build_runtime_endpoint`（新 `stream_epoch`）+ 既有授权门（`CONTROL_DISABLED` 等）"实现，**不新增鉴权机制**。若评审指的是新增授权令牌/二次确认，属另一层设计，需明确。
2. **`desired_running` 写入时机前移**：由"teardown 之后"（`lib.rs:2180`）前移到"Stop 受理时"（同步临界区）。好处是取消/崩溃都不会留下 `desired=true`（重启不会自动拉起用户已要求停的端点），代价是拆解期间 DB 显示"期望停止"而实际仍在拆解。需 owner 确认这个"意图先行"的语义可接受。
3. **并发 Stop 合流语义**：第二个 Stop 不再"立即 200 `was_running=false`"，而是合流到同一次 teardown、返回同一结果（`was_running=true`，延迟 = 剩余 teardown）。这与现有"幂等即立刻返回"的直觉不同，但更真实；需确认是否接受（若不接受，备选是第二个 Stop 返回 409 `STOP_IN_PROGRESS`，但那会让"Stop 幂等"变成"Stop 可失败"，不推荐）。
4. **门的归属变更**：原稿建议门放 `core-api` 且由门持有 `inflight_stops`。本轮改为"真值在 `driver-manager` 的 attempt 表，`core-api` 只做排序与票据"，因此**不需要**把门搬到 manager，也不需要 manager 反向依赖 core-api。若评审坚持"门整体下沉到 manager"，则需评估 `MesaManager` 公共 API 的 churn（`begin_stop` 之外还要暴露票据模型）。
5. **`manager` 新增同步 API**：`begin_stop`（同步）、`has_live_attempt`、`attempt_gen`、`await_settled` 的归属（manager vs gate）。本文把前三个放 manager、`await_settled` 放 gate。
6. **稳定窗口参数**：`250 ms × 2 = 500 ms` 是否足够；是否需要按 endpoint/驱动类型可配（S7/FOCAS2 的会话关闭时延不同）。
7. **全局 `LIVE_CHILDREN` 计数**：是否值得（它把临时 Descriptor 进程也算进去，需在判定时排除基线）。若不做，则"无孤儿"只剩结构不变量与端到端 pid 检查。
8. **W3 现场可达性**：本轮已由 insert-before-spawn **结构性**封死，不再依赖"是否真并发"的论证；但若评审仍希望保留"提交时 `is_running` 复核"作为第三道防线，需说明其与 attempt 表判定的优先级（本文认为重复，不采纳）。
9. **`desired_running` 写失败**（现被 `let _` 吞掉，`lib.rs:2155`/`:2171`/`:2180`）是否要升级为告警/返回——影响面超出本竞态，本文只把 Start/Stop 两处升级为 `tracing::error!`，是否返回错误另议。
