# Core Start/Stop 生命周期竞态修复设计（G7 前置）

- **状态**：设计稿（未实施，不含代码改动）
- **基线**：`main = fda7633a6d675c43f5bab58f662667aac4a54daf`；本文件位于 `docs/core-start-stop-race-fix-design`（PR #132，HEAD `eb0a0c8`），`git diff fda7633 -- crates apps tests` 为空 ⇒ 本文引用的代码与 `main` 完全一致
- **上游依据**：`docs/validation/828d-phase1-first-limited-sampling-application.md` 的 文档 §4.7（G7-1/2/3）、文档 §5.3（竞态机制、Stop 时间约束、六项判据）
- **本轮修订依据**：两轮独立复审。第一轮冻结 G7-2 契约（§5.1）与 P0-A~E / P1-F 六项漏洞；第二轮冻结**并发 Stop 合流**与 **`desired_running` 前移**两项语义（§5.2、§5.3），并追加 P0-1~P0-4 / P1-5 四项必须补齐的设计（§6.3.2、§6.10、§6.11、§6.5、§7.5）
- **约束**：不改 wire/IPC；不改 A1 退避语义；不重构生命周期；不启动服务/设备、不跑测试与构建
- **引用约定**：`§x.y` 指**本文档**章节；指向上游 828D 文档时一律写作 `文档 §x.y`（即 `docs/validation/828d-phase1-first-limited-sampling-application.md`）。本文新增的 §5.2/§5.3 与上游 §5.3 **不是同一节**，请勿混读

---

## 0. 结论摘要

1. 竞态**真实存在**，且不止一处：Stop 的幂等分支（`crates/core-api/src/lib.rs:2169-2175`）与 Start 成功路径（`lib.rs:2153-2156`）之间没有任何"意图排序"保护，两个 handler 并发执行时**后提交者胜**。
2. 最关键、也最容易被现场触发的窗口不是文档 §5.3 描述的那一个，而是 **Stop 拆解期间**：`MesaManager::stop_endpoint` 在 await 之前就把运行条目从 map 中移除（`crates/driver-manager/src/manager.rs:535`），因此整个拆解期（典型 ≈35 s，文档实测 75–80 s，尾部无界）内 `is_running()` 恒为假（`manager.rs:459-466`），任何此刻到达的 Start 都能通过检查并**并行拉起第二个 attempt**。
3. 修复方向不变：**Stop 生成号 + Stop 在途保护 + Start 两阶段提交（admit/commit）**。但本轮修订纠正了两个根本性错误：
   - **Stop 在途保护必须归属于"实际生命周期操作"（attempt 任务本身），不能归属于 HTTP 请求**。原稿 §5 让 handler 用 RAII guard 减计数，一旦客户端断开/超时导致 handler 被取消、而 teardown 仍在后台跑，guard 会提前释放保护、**重新开放 W2**（§6.3）。
   - **`is_running()` 不能作为"停干净"的完成判据**：它在拆解开始前就变假（`manager.rs:535`），且 W3 孤儿根本不在运行表里（`manager.rs:513-516`）。判据必须换成"代次化的 attempt 表 + 代次化快照"（§1.2、§6.5、§7）。
4. 变化仅限 Core 内部：`driver-manager` 新增代次化 attempt 表与快照代次门（`snapshot.rs`/`endpoint.rs`/`manager.rs`/`process.rs`），`core-api` 新增薄门 `LifecycleGate`，`config-store` 收缩 `desired_running` 的写路径（§6.10，**不改表结构、不加列**）；**不碰 wire/IPC、不改 REST 字段**。对外的行为变化共四处：竞态分支下 Start 的状态码 200 → 409（带可机器识别的 code）；并发 Stop 由"立即 200 `was_running=false`"改为**合流并返回同一次结果**（§5.2）；`desired_running=false` 由"teardown 之后"**前移到 Stop 受理时**（§5.3）；**Stop 意图落盘失败由静默 200 改为显式 500**（§6.10.3）。
5. 第二轮复审补齐的四项**不是可选优化，而是前述方案的成立条件**（详见对应小节）：
   - **P0-1**：`AttemptGuard::Drop` 在 panic 展开时同样执行 ⇒ 若照原稿无条件删条目，新 Start 会在 teardown 未证明完成时被放行，**与 fail-closed 直接矛盾**。必须区分"正常完成"与"异常终止"，后者进入 `FAILED_UNKNOWN` 隔离态并禁止新 Start（§6.3.2、§6.3.3）。
   - **P0-2**：`desired_running` 还有两条**不经过生命周期门**的写路径（`update_endpoint()` / `update_endpoint_with_secrets()` 的 UPDATE 都带该列，`config-store/src/lib.rs:1091-1094`、`:1165-1168`），且 `core-api` 的 PUT 是"先读 → 长 await 校验 → 把旧对象写回"（`lib.rs:1979`、`:1998`、`:2096-2099`）⇒ **"门是唯一写者"在现有代码中不成立**。必须做写路径拆分（§6.10）。
   - **P0-3**：完成代次不能随 attempt 条目一起消失，否则 `await_settled()` 无代次可比，**正常停止反而永远无法满足判据**。必须保留 `last_completed_gen`（§6.11、§7.4）。
   - **P0-4**：墓碑检查与快照写入必须**同一线性化临界区**，否则"检查完墓碑、尚未落盘时 DELETE 刚好执行"仍可复活已删端点（§6.5）。
   - **P1-5**：Core 内部 500 ms 快速收敛检查**不等于**现场 10 s 连续稳定观察，前者不能替代后者（§7.5）。

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
| F25 | `update_endpoint_with_secrets()` 的 UPDATE **同时写 `desired_running`**（值来自调用方传入的 `rec`） | `crates/config-store/src/lib.rs:1067`；SQL `:1091-1094`（`desired_running=?4` 在 `:1092`，参数在 `:1093`） |
| F26 | `update_endpoint()` 的 UPDATE **同样写 `desired_running`**；该函数当前**无生产调用方**（仅单测 `:2535`、`:2538`），但仍是公开 API，语义上仍可覆盖运行意图 | `config-store/src/lib.rs:1156`；SQL `:1165-1168`（`desired_running=?4` 在 `:1166`） |
| F27 | `core-api` 的 PUT 是**"先读 Endpoint → 可能较长的 Descriptor 异步校验 → 把先前读到的对象整体写回"**：`rec` 在 `:1979-1987` 读出，`:1998` 起 `await get_descriptor`（懒加载，可能起临时驱动进程，见 F8），`:2096-2099` 才落盘 ⇒ `rec.desired_running` 是**读时的旧值** | `crates/core-api/src/lib.rs:1979-1987`、`:1990-1991`、`:1998`、`:2094-2099` |
| F28 | Bootstrap **在建档事务里就以 `desired_running=true` 插入**（`body.start` 缺省 true） | `crates/core-api/src/bootstrap.rs:297`；INSERT 在 `config-store/src/lib.rs:1259-1261` |
| F29 | Bootstrap 启动成功后再写一次真；写失败**只记 error、不翻转 start 结果** | `bootstrap.rs:110-125`（`:117`） |
| F30 | `Snapshot::remove_endpoint` 有两个生产调用点：DELETE 端点、Bootstrap 失败补偿删除 | `core-api/src/lib.rs:2125`、`bootstrap.rs:348` |
| F31 | `Snapshot` 现有 **6 把彼此独立的锁**（`endpoints` 只是其中之一）⇒ 若墓碑另起一把锁，"检查墓碑 + 写入"必然跨两次加锁 | `snapshot.rs:155-176`（`endpoints` 在 `:157`） |
| F32 | 早退路径同样收敛子进程：`terminate()` 两个分支都 `child.wait()`（宽限内自然退出，或强杀后 `:127` 再 wait） | `process.rs:119-130`（`:122`、`:127`）；正常 Stop 走 `:143-154` |

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
- **窗口宽度**：整个 teardown。注释给出的预算是 post 5 s + terminate 10 s + reader 5 s + drain 15 s ≈ 35 s（`endpoint.rs:634-637`），文档实测典型 75–80 s（文档 §5.3），且强杀后第二次 `child.wait()` 无 timeout（`process.rs:122-127`、`:143-154`）⇒ **尾部无界**。
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
- 更隐蔽：F12 表明 RUNNING 只在 attempt 进入时写一次，而老 attempt 的拆解会写 STOPPED（`endpoint.rs:357-366`）。若老 attempt 的 STOPPED 晚于新 attempt 的 RUNNING 落盘（F10 最后写入者胜），**快照会长期显示 STOPPED 而实际仍在采集**——直到新 attempt 下一次状态变更。这直接击穿文档 §5.3 判据 ②③ 的可信度。

**② 状态一致性**
- `desired_running=false` 与 `runtime.state=RUNNING` 可长期共存（F13：无对账、无看门狗）。重启后因 `desired_running=false` 不再恢复，所以"自愈"只发生在重启，当前会话内不会。
- 活跃会话表（F11）以 `endpoint_id` 为键：老 attempt 拆解时 `remove` 会删掉新 attempt 刚注册的会话（或反之），Control/Browse 可能指向已失效会话——**间歇性**、难复现。（本轮修订后此风险被结构性消除：并发 attempt 被拒绝 ⇒ 同一 `endpoint_id` 不会有两个会话同时注册；见 §6.6。）
- `desired_running` 写失败被 `let _` 吞掉（`lib.rs:2155`、`:2171`、`:2180`），DB 异常时同样产生矛盾（现状，非本次竞态）。
- **`desired_running` 不止一个写者（P0-2，本轮新增）**：除三个 handler 写点外，`config-store` 的 `update_endpoint_with_secrets()`（`config-store/src/lib.rs:1091-1094`，生产调用点 `core-api/src/lib.rs:2098`）与 `update_endpoint()`（`:1165-1168`）的 UPDATE **都带 `desired_running` 列**；而 PUT 是"读 → 长 `await` 校验 → 回写旧对象"（F27），所以**一次普通的改名/改连接就能把 Stop 刚写入的 `false` 覆盖回 `true`**——这不是竞态的衍生品，是独立的覆盖路径。修法见 §6.10。

**③ 其他同样能覆盖 Stop 的路径**

| 路径 | 是否覆盖 Stop | 位置 | 处理 |
|---|---|---|---|
| 自动重连 | **不是**覆盖 Stop 的"意图"，但会让 Stop 之后仍有连接：attempt 内部循环只在循环顶与退避 sleep 处检查取消；配置闭环 `run_config_flow` **没有 shutdown 参数、无取消点**（4 个 RPC × `REQUEST_TIMEOUT` 10 s，`session.rs:25`），Stop 后仍可能走到 `StartConnection(new epoch)` | `endpoint.rs:329-341`、`:428-434`、`:775-782`、`:903-917` | 属文档 §5.3-⑥ 已登记的相邻缺陷，**本设计不修**（见 §10），但门不得宣称能消除它 |
| 心跳 | 否（心跳只影响会话判死，不启停） | `endpoint.rs:258-260` | 无需处理 |
| `desired_running` 恢复 | 开机恢复在 REST 起来之前串行执行，**不与 Stop 并发**；但 bootstrap 的"建档即启动"与 REST Start 同构，窗口一致 | `apps/mesad/src/main.rs:140-161`（serve 在 `:170`）；`bootstrap.rs:110-125` | bootstrap 必须一并接入门 |
| 双 Start（W3） | **是**：Stop 只停掉 map 里的一个，孤儿继续跑 | `manager.rs:513-516` | 由 insert-before-spawn + 拒绝并发 attempt 结构性封死（§6.6），**不再依赖"提交时复核"这种事后检查** |
| `DELETE /endpoints/{id}`、`PUT /tasks`、`POST /tasks` 的 `is_running` 门 | 非覆盖，但同族 TOCTOU：拆解期 `is_running` 为假，允许删端点/改任务，而旧 attempt 仍在用旧任务跑；且 `remove_endpoint` 无墓碑（F21），旧 attempt 随后的状态写入会**复活已删除端点** | `lib.rs:2113`、`:2125`、`:2240`、`:2297`；`snapshot.rs:440` | 删端点的 TOCTOU 本设计**不修**；但"旧代次写入不得复活已删端点"由代次门 + 单锁墓碑覆盖（§6.5，含原子性证明） |

---

## 5. 冻结契约（评审给定，逐字采用，作为设计基准）

> 本节三小节全部为**冻结语义**。本文不再讨论其取舍（尤其不再讨论"并发 Stop 要不要 409""`desired_running` 要不要前移"），只负责把落地机制写清楚：实现见 §6，判据见 §7。

### 5.1 Start 侧三行（第一轮冻结，逐字采用）

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
| 行 2：teardown 期 Start 冲突、不排队 | attempt 表条目仍存在（`phase=Stopping`；异常终止时为隔离态 `FailedUnknown`）⇒ `start_endpoint` 同步拒绝 ⇒ `STOP_IN_PROGRESS` / `ATTEMPT_FAILED_UNKNOWN`，**没有任何队列** | §6.4 步骤 2、§6.6、§6.3.3 |
| 行 3：完成后允许全新 Start | 条目由 attempt 任务自身在 teardown 完成后移除（异常终止时由 `attempt_reaper` 复核后移除）⇒ 新 Start 走全新 admit + 全新 `build_runtime_endpoint`（新 `stream_epoch`）+ 既有授权门 | §6.4、§6.7、§6.3.3 |

**"新的操作授权"的实现解释（需 owner 确认，见 §11-1）**：不复用任何 Stop 之前的票据或配置快照——新 Start 必须（a）重新 `admit_start` 取得当前代次的票据，（b）重新执行 `build_runtime_endpoint`（`lib.rs:2149`，含 Secret 物化与任务门禁），（c）继续受既有授权门约束（`CONTROL_DISABLED` 等不变）。**不是**新增鉴权机制。

### 5.2 并发 Stop：合流（第二轮冻结，六行逐字采用）

| 场景 | 预期行为 |
|---|---|
| 第一次 Stop | 受理停止，触发一次 teardown |
| teardown 过程中第二次 Stop | **合流等待，不重复 teardown** |
| teardown 成功 | 两个请求均返回 200，`was_running=true` |
| teardown 失败 | 两个请求获得**同一次明确失败结果**，不伪装成功 |
| teardown 完成之后再次 Stop | 保持原有幂等语义：200，`was_running=false` |
| 客户端断开或超时 | 只丢失当前请求的应答，**不取消后台停止操作** |

**必须同时纠正的一个认识（逐字写入）**：**Stop 不应因"已经有人在停止"而失败，但真正停止失败时仍必须能够返回错误。合流不意味着 Stop 永远成功。**

落地含义（与 §6.4 的 `begin_stop` 一一对应，本节只做映射，不重新论证）：

1. **不采用 409**：第二个 Stop 不走"冲突"分支，而是 `done_tx.subscribe()` 拿到同一个结果通道并 `await` 同一结论（§6.4 `Join` 分支）。这是"合流"的唯一实现方式——**合流点必须是"结果"，不是"请求"**。
2. **"同一次明确失败结果"**：`AttemptCtx.outcome`（§6.2）是单次 attempt 的唯一结论载体，`OnceLock` 只允许写入一次 ⇒ 两个请求**不可能**看到两次不同的失败描述。失败以 `500 STOP_FAILED` + 精确 detail 返回；`stop_endpoint` 的 `Err` 仍须以精确码开头，冻结集合因此需要**扩一项** `ATTEMPT_FAILED_UNKNOWN`（§6.8，这是唯一一处对既有冻结断言的扩写）。
3. **第六行（客户端断开不取消后台操作）是 §6.3 的全部意义**：保护与执行都挂在 attempt 任务上，HTTP 请求只持有一个 `watch::Receiver`。丢弃 receiver 不触碰任何状态。
4. **与 P0-1 的关系**：teardown **异常终止**（panic/abort）同样属于"失败"——此时两个请求都必须拿到明确失败，且**条目不得被释放**（§6.3.2）。合流语义不允许把"异常终止"翻译成 200。

### 5.3 `desired_running` 前移到 Stop 受理时（第二轮冻结）

- Stop 一经受理，**先把 `desired_running=false` 写入持久化存储**
- teardown 期间，`desired_running=false` + 实际状态 `STOPPING` 是**合法组合**
- 即使客户端超时，**后台 teardown 仍须继续**
- 后续 Start **只有在旧 attempt 确认结束后**才允许进入新的生命周期
- **必需条件**：**数据库写入失败不能再被 `let _` 静默忽略** —— 若停止意图没成功持久化却返回正常停止成功，重启后仍可能意外自动拉起端点

落地含义：

1. 写入点在 `admit_stop`（§6.4，同步临界区，**先落盘再 `begin_stop`**），不在 handler 的 `await` 之后（现状 `lib.rs:2171`、`:2180` 都在 await 之后，取消即丢）。
2. "合法组合"要求**判据侧不得把 `desired=false` + `state!=STOPPED` 当作矛盾**：§7.1 判据 2 与判据 1 是**独立**的，`desired=false` 不蕴含 `state==STOPPED`，反之亦然。
3. "写入失败不得静默"的具体行为（**显式失败 + 安全方向 + 禁止不受控重启**）见 §6.10.3。注意它**不是**"Stop 返回 500 就完事"：teardown 仍必须执行，且必须防止"DB 说 true、内存说 false"在重启后变成自动拉起。
4. "后续 Start 只在旧 attempt 确认结束后" = §6.3.1 的 `TeardownDone(gen)`，**不是** `is_running()==false`（§1.2）。

### 5.4 三者的交叉约束（本节唯一新增推理）

| 约束 | 为什么必须写下来 |
|---|---|
| **合流 ≠ Stop 永远成功** | 否则实现者会把"有人正在停"当成"必定成功"，从而在 `outcome=Err` 或 `phase=FailedUnknown` 时返回 200——这正是 §5.2 第四行与 P0-1 要禁止的"伪装成功" |
| **前移 ≠ 意图已持久化** | 前移只是把写入点搬到同步临界区；写失败仍可能发生。**"已前移"与"已落盘"是两件事**，后者失败必须显式（§6.10.3） |
| **合流 + 前移 ≠ 可以放宽 Start 门** | `desired=false` 是意图，不是"旧 attempt 已结束"的证明；Start 门只看 attempt 表 + 隔离态（§6.4 步骤 2），不看 `desired` |

---

## 6. 方案 A：Stop 生成号 + Stop 在途保护 + Start 两阶段提交

### 6.1 唯一真值与三层职责

本轮修订最重要的结构决定：**"有没有 attempt、attempt 到哪一步了"只有一个真值来源——`driver-manager` 的 attempt 表；`core-api` 的 `LifecycleGate` 只是排序器与票据池，不持有任何生命周期真值。**

| 层 | 组件 | 持有什么 | 不持有什么 |
|---|---|---|---|
| `driver-manager` | `AttemptTable`（替换现 `running`，`manager.rs:360`） | `live` 条目（attempt 是否存在、代次、phase、pid、teardown 证据、结果）+ `last_completed_gen`（完成代次凭证，§6.11）+ 隔离态（§6.3.2） | 不碰 DB、不碰 HTTP、不知道 Stop 票据 |
| `driver-manager` | `Snapshot`（`snapshot.rs`） | `(gen, EndpointStatus)` 原子对 + 墓碑，写入按代次过滤（**单锁**，§6.5） | 不判断"是否停干净" |
| `core-api` | `LifecycleGate`（新 `crates/core-api/src/lifecycle.rs`） | 每 endpoint 的 `stop_gen`、在途 **Start** 票据数、`desired_running` 缓存 + 写入序号 | **不持有** Stop 在途计数（原稿的 `inflight_stops` 删除） |

**为什么删掉 `inflight_stops`**：计数器的"减一"必须发生在"teardown 真正完成"这个事件上，而该事件只发生在 attempt 任务内部。若计数器在 core-api，就必然要么由 handler 减（取消即丢，P0-A 的原始漏洞），要么由 manager 反向回调 core-api（层次倒置）。**把保护直接表达为"attempt 表条目存在"，减一就自动等价于"条目被 teardown 完成者移除"**——不需要任何跨层通知，也不存在"忘了减"的可能。**且"移除"只发生在正常完成路径**（§6.3.2）：异常终止**不**移除、转隔离态 ⇒ 保护不会因为 panic 而消失，这是 P0-1 对"保护载体 = 条目"这一决定的补强。

### 6.2 数据结构

```rust
// driver-manager：一次 attempt 的全部可判定证据（无锁读，供观察与诊断）
pub struct AttemptCtx {
    pub gen: u64,                       // 单调代次，Manager 分配（§6.5：per endpoint 计数器，永不回收）
    phase: AtomicU8,                    // Starting → Running → Stopping → TeardownDone | FailedUnknown
    pid: AtomicU32,                     // 驱动子进程 pid（spawn 后回填；0 = 尚未 spawn）
    session_released: AtomicBool,       // teardown 末尾：registry 已摘除本 attempt 的 session
    remote_eof: AtomicBool,             // teardown 末尾：wait_reader_done 观测到对端 EOF
    outcome: OnceLock<Result<bool, String>>,  // teardown 结论（OnceLock ⇒ 一次 attempt 只有一个结论）
    committed: AtomicBool,              // P0-1：正常完成标记。**只有 commit() 能置 true**（§6.3.2）
    done_tx: tokio::sync::watch::Sender<Option<Result<bool, String>>>,  // 完成通知
}

struct AttemptEntry {
    ctx: Arc<AttemptCtx>,
    cancel: CancellationToken,
    // 注意：这里**没有** JoinHandle（见 §6.6）
}

// P0-1 / P0-3：attempt 表与"完成代次"必须同锁，否则"移除条目"与"记录完成代次"之间会出现空窗
struct AttemptTable {
    live: HashMap<String, AttemptEntry>,
    /// 只由 commit() 路径写入（与移除条目同一临界区）；永不回退、永不清理（§6.11）
    last_completed_gen: HashMap<String, u64>,
    // 隔离态**不另设 map**：它就是 live 里 phase == FailedUnknown 的条目（避免两处真值）
}
```

```rust
// core-api：只做排序与票据
struct EpGate {
    stop_gen: u64,               // Stop 受理即 +1（W1 的唯一判据）
    inflight_starts: u32,        // 已 admit、未 commit/未放弃的 Start 票据数
    desired: Option<bool>,       // 懒初始化自 DB；此后由门写入——**该结论以 §6.10 的写路径拆分为前提**
    desired_seq: u64,            // 每次成功写入 +1（证明"不是旧值"）
    last_stop_seq: Option<u64>,  // 最近一次 admit_stop 时的 desired_seq（§7.3 判据 2 的比对基准）
    intent_unpersisted: bool,    // P0-2：Stop 意图落盘失败 ⇒ 禁止 Start，直到补写成功（§6.10.3）
}
```

**"门是唯一写者"这句话的正确性前提（P0-2 的结论，必须显式）**：现有代码里 `desired_running` 还有两条 UPDATE 路径（F25/F26）与一条建档 INSERT 路径（F28），所以**该命题在当前代码中不成立**；它只有在 §6.10 的写路径拆分落地后才成立。因此：**判据 2 与 `desired_seq` 的抗旧值论证，依赖 §6.10 与本节的实现同时到位**——两者不可分开验收。

**锁序纪律（必须写进代码注释）**：`gate` 锁 → `manager` 锁，**永不反向**。`manager` 的任何路径都不调用 gate（结构上保证），故无死锁。`commit_start` 与 `admit_stop` 都在这条锁序下完成"代次比对 + 生命周期调用"。

### 6.3 Stop 在途保护归属于"生命周期操作"（P0-A）

**作废原稿写法**：上一版 §5「关键实现纪律」曾建议"`admit_stop` 用 RAII guard 兜底减计数（handler panic 时不能永久卡死 Start）"。该写法**必须废弃**，理由即复审所述：

> handler 中途被取消（客户端断开 / 网关超时 / `-TimeoutSec 10` 到点），而实际 teardown 仍在后台执行 ⇒ guard 随 future drop 而提前释放 ⇒ `inflight_stops` 归零 ⇒ **Start 重新放行** ⇒ W2 并发窗口原样重现，而且此时现场更容易触发（客户端超时是常态）。

**新规则（三条，逐条对应复审要求）**：

1. **保护载体 = attempt 表条目**，而不是任何计数器或 handler 栈帧。条目在 `start_endpoint` 里 **spawn 之前**创建（insert-before-spawn），**正常完成时**由 attempt 任务自己在返回前移除、**异常终止时保留**（§6.3.2）。因此"保护存在" ⟺ "该 attempt 尚未确认 teardown 完成"，二者**是同一件事**，不存在需要同步的副本；也不存在"panic 就把保护丢了"的缺口。
2. **释放条件 = 确认旧 attempt 已完成 teardown**，且释放动作**只由 attempt 任务自身执行**（`AttemptGuard::drop`，见下）。`stop_endpoint` 不释放、handler 不释放、超时不释放。
3. **handler 取消 / 超时 / 连接断开时保护不得释放**：handler 在该路径上只持有一个 `watch::Receiver`（等待结果用）。丢弃它只丢失"应答"，不触碰任何状态；teardown 任务继续跑，条目继续存在，Start 继续得到 409。

**RAII 不是不能用，而是不能用错宿主**：`AttemptGuard` 放在 **attempt 任务内部** 是正确的——它随任务结束而 drop，drop 里只做同步的簿记；若放在 handler 里就是灾难。**但"宿主正确"还不够**：`Drop` 在 panic 展开时**同样执行**，而 panic 那一刻 `child.wait()`、Session 清理、事件排空未必完成。因此 `Drop` 必须能区分"正常完成"与"异常终止"，判据**不是** `std::thread::panicking()`，而是**显式提交标记**（详见 §6.3.2）。

```rust
// 只存在于 attempt 任务内部。committed 是本结构唯一的"完成"权威
struct AttemptGuard {
    table: Arc<Mutex<AttemptTable>>, id: String, gen: u64, ctx: Arc<AttemptCtx>,
    quarantine_tx: mpsc::Sender<String>,      // 交给监督清理（§6.3.3）
}
impl AttemptGuard {
    /// 唯一正常退出路径调用：必须在 run_endpoint_with_attempt 正常返回、
    /// 且 ctx.outcome 已写入之后调用（§6.3.1 的 E1~E4 由该返回点保证）
    fn commit(&self) {
        debug_assert!(self.ctx.outcome.get().is_some(), "commit 前必须已有 teardown 结论");
        self.ctx.phase.store(Phase::TeardownDone);
        self.ctx.committed.store(true, Ordering::Release);   // 先置标记，再返回
    }
}
impl Drop for AttemptGuard {
    fn drop(&mut self) {
        let mut t = self.table.lock().unwrap();
        if !t.live.get(&self.id).is_some_and(|e| e.ctx.gen == self.gen) {
            return;                                  // 防御：条目已不属于本代次，什么都不做
        }
        let committed = self.ctx.committed.load(Ordering::Acquire);
        if committed {
            // 正常完成：写完成代次 + 移除条目，**同一个临界区**（P0-3 的原子性来源）
            t.last_completed_gen.insert(self.id.clone(), self.gen);
            t.live.remove(&self.id);
        } else {
            // 异常终止：**保留条目**，转为隔离态，禁止新的 Start（P0-1）
            self.ctx.phase.store(Phase::FailedUnknown);
            if self.ctx.outcome.get().is_none() {
                let _ = self.ctx.outcome.set(Err(format!(
                    "ATTEMPT_FAILED_UNKNOWN: attempt gen {} terminated abnormally (panic/abort); \
                     teardown completion NOT proven", self.gen)));
            }
            if std::thread::panicking() {   // 只用于留证据，**不参与判定**
                tracing::error!(endpoint = %self.id, gen = self.gen,
                    "attempt task panicked; entry quarantined as FAILED_UNKNOWN");
            }
        }
        let _ = self.ctx.done_tx.send(self.ctx.outcome.get().cloned());
        drop(t);
        // 异常路径把 id 交给监督清理：try_send 非阻塞，Drop 里绝不 await
        if !committed { let _ = self.quarantine_tx.try_send(self.id.clone()); }
    }
}
```

> `Drop` 里不能 await，所以释放路径天然是同步的——这同时满足了"临界区内禁止 await"的纪律。**但也要承认 `Drop` 的能力边界：它不能做任何"证明收敛"的工作**（kill / wait / 清理全是 async），这正是异常路径只能"保留条目 + 交给监督者"而不能"顺手删掉"的原因。

#### 6.3.1 "teardown 真正完成"的可判定信号（最关键的一条）

**定义**：`TeardownDone(gen)` 当且仅当下面四件事同时成立，且它们**由同一条因果链保证，不需要额外探测**：

| # | 证据 | 为什么可信（代码依据） |
|---|---|---|
| E1 | 该 gen 的 `run_endpoint` 任务已结束 | attempt 任务就是生命周期操作本身；任务结束 ⇒ 它再也不会碰驱动进程、会话、快照 |
| E2 | 任务结束发生在 `teardown_attempt` 返回**之后** | `attempt_session` 在 `endpoint.rs:638-647` 先 `teardown_attempt`，返回后 `run_endpoint` 才写 STOPPED（`:357-368`）⇒ teardown 是 return 的前置 |
| E3 | `teardown_attempt` 返回 ⇒ 驱动子进程已被 `wait()` 回收 | `endpoint.rs:713` → `process.rs:143-154`：`child.wait()` 成功，或强杀后 `child.wait().await`（`:127`，无 timeout，**永不返回即永不"完成"**，见下） |
| E4 | 该 attempt 的 session 已从 registry 摘除 | `endpoint.rs:728` 移除；主判据是 `registry.read().get(endpoint_id).is_none()`——由于并发 attempt 被拒绝（§6.6），该键若存在只可能属于本 attempt，故"键不存在"即"本 attempt 已摘除"，不会读到上一代。辅助判据 `Arc::strong_count(&session_arc) == 1`（**不作主判据**：并发 Control 请求会临时 clone 该 Arc，`manager.rs:841`、`:890`，计数可能短暂为 2；且此时 session 已 `invalidate()`，不具操作能力） |

**信号载体**：`AttemptCtx.phase == TeardownDone`（**只能**由 `AttemptGuard::commit()` 写入，§6.3.2），**并且**该 gen 的条目已从 attempt 表移除、`last_completed_gen == gen`（§6.11）。观察者读"条目不存在 + 凭证代次匹配"即可，`phase` 只用于诊断。**注意**：`phase==TeardownDone` 但条目仍在是**不可能**的正常态（同一临界区内先写凭证再移除）；若观测到该组合，说明代码被改坏，应 `tracing::error!`。

**明确排除的伪信号**：

| 伪信号 | 为什么不可用 |
|---|---|
| `is_running()==false` | 拆解开始即假（`manager.rs:535`），且漏孤儿（§1.2） |
| `handle.is_finished()` | 它只回答"任务是否结束"，而 W3 覆盖写后旧 handle 已被丢弃、无从查询（`manager.rs:513-516`）；且在"任务已结束但条目仍在"的瞬时态下会与"停干净"混淆 |
| 快照 `state=="STOPPED"` | 可被旧代次覆盖（F10），且 `remove_endpoint` 会直接抹掉条目（F21） |
| `desired_running==false` | 是**意图**不是**事实**；旧代码还在 await 之后才写（`lib.rs:2180`），取消即丢 |
| 定时器/预算到期 | teardown 尾部无界（`process.rs:127`），"超时"不等于"已停" |

**尾部无界时的行为（必须诚实声明）**：若 `child.wait()` 因内核/驱动卡死永不返回，则 `TeardownDone` 永不成立 ⇒ 条目永不移除 ⇒ **Start 永远 409**（`STOP_IN_PROGRESS`，fail-closed，安全方向）。这不是缺陷而是设计选择：机床场景下"停不干净就不许启动"优于"以为停了又拉起来"。为此必须补一条**诊断**：`stop_endpoint` 记录 `stop_elapsed`，超过 teardown 预算（≈35 s，`endpoint.rs:634-637`）即 `tracing::warn!`，超过 3 倍预算 `tracing::error!` 并附 attempt 表快照，供现场判断是"卡在 wait"还是"卡在 drain"。**不加自动强杀**（会改变 teardown 预算，属 §10 排除项）。

#### 6.3.2 `AttemptGuard::Drop` **不等于** teardown 成功（P0-1）

**问题（复审指出，代码层面成立）**：`teardown_attempt()` 内部一旦 panic（锁中毒、切片越界、驱动 SDK 的 unwrap 等），Rust 同样会沿展开路径执行 `AttemptGuard::drop`。此刻下面三件事**都可能尚未完成**：

| 未完成的证据 | 代码依据 | 后果 |
|---|---|---|
| 子进程未 `wait()` 回收（E3） | `child.wait()` 在 `teardown_attempt` 的第 2 步之后才发生（`endpoint.rs:713` → `process.rs:143-154`）；panic 可以发生在它之前 | 驱动进程可能仍活着并与机床保持 TCP 会话 |
| Session 未失效、未从 registry 摘除（E4） | `sess.invalidate()` 在 `endpoint.rs:726`、`registry.remove` 在 `:728`，都在 teardown 末尾 | Control/Browse 可能拿到一个"看起来还活着"的旧会话 |
| ingress 未排空（E2 的 drain） | `shutdown_ingress` 在 `endpoint.rs:723` | 事件可能丢，且无人上报 |

若 `Drop` 无条件删条目 ⇒ **新 Start 立刻被放行** ⇒ 旧子进程/旧会话仍在 ⇒ **与 fail-closed 直接矛盾**（"以为停了又拉起来"）。这不是理论风险：panic 后 `kill_on_drop(true)`（`process.rs:74`）会让 `DriverProcess` 在展开时被 drop，从而**尽力**杀掉子进程；但 `child.wait()` 未执行、无回收证据，且该保证不覆盖 registry 与 ingress。**"尽力"不能当作"已收敛"**。

**要求：两条路径必须显式区分。**

| 路径 | 判定依据（唯一权威） | 动作 | 结论 |
|---|---|---|---|
| **正常完成** | `AttemptGuard::commit()` 已执行 ⇒ `ctx.committed == true` | `phase=TeardownDone`；同临界区写 `last_completed_gen` 并移除条目 | 允许新的 Start（新 gen） |
| **异常终止**（panic / task abort / 未 commit 的提前返回） | `Drop` 时 `ctx.committed == false` | `phase=FailedUnknown`；**保留条目**；`outcome=Err(ATTEMPT_FAILED_UNKNOWN…)`；交给监督清理 | **禁止新的 Start**，直到监督清理与复核完成 |

**判别机制（为什么这样设计，逐条给出理由）**：

1. **主判据 = 显式 `commit()` 标记**（`AtomicBool`，Release/Acquire）。它由**唯一正常退出点**调用，且调用点位于 `run_endpoint_with_attempt` 正常返回之后 ⇒ "已 commit" 严格蕴含 E1（任务已到结尾）、E2（teardown 在 return 之前）、E3/E4（teardown 末步）。**这是唯一被允许用于"释放保护"的依据。**
2. **`phase` 终态只用于诊断**，不作为释放依据。原因：`phase` 可被多条路径写（例如 `begin_stop` 写 `Stopping`），把"终态"当判据等于让别的路径拥有释放权——正是 P0-A 的同型错误。
3. **`std::thread::panicking()` 只写日志，不参与判定**。三条理由：(a) `panic = "abort"` 或 `std::process::abort()` 时 **`Drop` 根本不执行**，此时条目保留、`phase` 停在 `Stopping`，Start 得到 `STOP_IN_PROGRESS`——**已经是 fail-closed**，不需要额外处理；(b) 正常但**提前 return**（例如未来某次重构在 teardown 前 `return`）不是 panic，却同样没有收敛证据，必须走异常路径；(c) 反过来，panic 也可能发生在"清理其实已经做完、只是 commit 之前"——此时走隔离是**偏保守**的代价，可以接受（安全方向）。
4. **`Drop` 绝不调用 `is_finished()` / `is_running()` 来"确认已停"**：前者回答"任务是否结束"（§6.3.1 已列为伪信号），后者在拆解期恒假（§1.2）。二者都不能证明 E3/E4。
5. **禁止在 `Drop` 里把 `phase` 写成"完成"**：`Ended`/`TeardownDone` 这类终态**只能**由 `commit()` 写。代码注释必须写明"任何在 Drop 里置终态的改动都是缺陷"。

#### 6.3.3 隔离态 `FAILED_UNKNOWN` 的生命周期（谁清除、清除前 Start 返回什么）

**状态定义**：`live` 中存在该 `endpoint_id` 的条目，且 `ctx.phase == FailedUnknown`、`ctx.committed == false`。**不另设"隔离表"**——隔离就是"条目还在 + phase 是隔离值"，避免两处真值再次分叉（与 §6.1 的"唯一真值"原则一致）。

| 问题 | 答案 |
|---|---|
| 谁清除 | `MesaManager` 内的**单一监督任务** `attempt_reaper`：`start_endpoint` 创建它（进程内一次），消费 `quarantine_tx`。**不是 handler，不是 `Drop`，不是下一个 Stop** |
| 清除条件 | 有界复核通过：① `pid` 已不存在（或已被本 reaper 强杀并 `wait` 回收）；② `registry` 中该 `endpoint_id` 的键**属于本 gen**（或不存在）；③ `LIVE_CHILDREN` 回落到基线。三条全部成立才允许清除 |
| 清除动作 | 在**同一把 attempt 表锁**内：`last_completed_gen.insert(id, gen)` + `live.remove(id)`；`outcome` **保持** `Err(ATTEMPT_FAILED_UNKNOWN…)` 不被改写（**失败必须仍是失败**）；`done_tx` 已在 Drop 里发过，不重发 |
| 复核不通过 | 条目**保留**，`tracing::error!` + 周期重试；永远无法证明收敛时，只能重启 Core。这与 §6.3.1 "尾部无界 ⇒ Start 永远 409" 是同一个 fail-closed 选择 |
| 清除前 Start 返回什么 | **409**，`code = ATTEMPT_FAILED_UNKNOWN`（§6.8），message 带 `gen` 与原始异常摘要。**永不 200、永不排队、永不自动重试** |
| 清除前 Stop 返回什么 | `begin_stop` 命中隔离条目 ⇒ **不重复 teardown**，直接返回该条目已发布的 `outcome`（即同一次明确失败，符合 §5.2 第四行）；**Stop 不清除隔离**（否则等于用一次 Stop 绕过复核） |
| 清除后 | Start 允许，新 `gen = 旧 gen + 1`（§6.5 计数器单调）。注意：**隔离清除也必须写 `last_completed_gen`**，否则 P0-3 的判据在"异常结束"这条路径上又断了——异常 attempt 同样需要一个"终结凭证"，只是结论是失败 |
| 与 §10-1 的边界（必须写进代码注释） | `attempt_reaper` **只清理已死 attempt 的残留**：不启动、不重启、不写 `desired_running`、不改快照、不 cancel 任何活着的 attempt ⇒ **它不是对账器/看门狗**，不违反 §10-1 |
| 与进程重启的关系 | 隔离态只在内存。重启后按 DB 期望态恢复：若 Stop 意图已落盘 `false`，不会自动拉起（正确方向）；若**意图落盘失败**（§6.10.3），重启后可能自动拉起——这正是 §6.10.3 要堵的洞 |

**已知代价（诚实声明）**：`attempt_reaper` 是新增的常驻任务与一条通道，且"异常即隔离"会让一个 panic 变成"该端点必须等复核"的可用性事件。这个代价换的是"停不干净就不许启动"，在机床场景下方向正确。

### 6.4 Start 两阶段提交

```rust
admit_start(id) -> Ticket { stop_gen }        // 同步；inflight_starts += 1；不查 is_running、不碰 DB、不起进程
                                              // 票据由 RAII 释放：取消只减计数（此时尚未产生任何外部副作用，安全）

commit_start(t, cfg) -> Ok | Err(Superseded | StopInProgress | Quarantined | AlreadyRunning
                                 | IntentUnpersisted | StartFailed)   // 同步，单临界区
    0. if gate.intent_unpersisted                        -> IntentUnpersisted（503；§6.10.3）
    1. if gate.stop_gen != t.stop_gen                    -> Superseded        （契约行 1 / W1）
    2. mgr.start_endpoint(cfg)                            // 同步；内部 insert-before-spawn
         Err("ATTEMPT_FAILED_UNKNOWN: …")                  -> Quarantined     （P0-1 隔离态，§6.3.3）
         Err("STOP_IN_PROGRESS: …")                        -> StopInProgress  （契约行 2 / W2）
         Err("endpoint `x` already running")               -> AlreadyRunning  （W3 / 重复 Start）
         Err(其他，如 EVENT_STORE_UNAVAILABLE)             -> StartFailed     （400 START_FAILED，原样透传）
    3. gate.write_desired(id, true)                        // 仅在 2 成功后；**失败必须显式**：
                                                           // attempt 已启动、无法回滚 ⇒ 返回 500
                                                           // START_INTENT_NOT_PERSISTED + error 日志（§6.10.3）
```

- **为什么必须同步**：`commit_start` 全程无 `await` ⇒ 客户端取消**无法**落在"代次比对通过、但 start 尚未发生"之间。这是 W1/W2 在取消场景下仍然成立的前提。
- **禁止**在临界区内调 `build_runtime_endpoint`（它 await 且可能起临时进程，F8）；构造留在 `S1`，因此需要票据来检测"构造期间 Stop 已受理"。
- **禁止**在临界区内调 `manager.stop_endpoint`（async）。
- **步骤 3 不再允许 `let _`**：Start 方向写失败时 attempt 已经在跑、无法回滚，因此必须**显式失败**，并留下"`desired` 与事实不一致"的诊断（重启后该端点不会自动恢复，属安全方向）。

Stop 侧（对称设计，同样全同步，且**步骤顺序不可交换**）：

```rust
admit_stop(id) -> StopHandle          // 同步，单临界区
    1. gate.stop_gen += 1                                  // 先作废所有在途 Start 票据（W1）
    2. gate.desired = Some(false);
       match store.set_desired_running(id, false) {         // §5.3：受理即落盘
           Ok(true)  => { gate.desired_seq += 1; gate.intent_unpersisted = false; }
           Ok(false) => { /* 端点已不存在：意图无需落盘，视为已满足 */ }
           Err(e)    => { gate.intent_unpersisted = true;   // 显式失败，但**不阻止** teardown
                          tracing::error!(endpoint = %id, error = %e,
                              "STOP intent not persisted; teardown proceeds, Start blocked"); }
       }
    3. mgr.begin_stop(id) → Claimed / Join / NotRunning / Quarantined
```

`begin_stop` 是**同步**函数（`manager` 内部）：

```rust
pub fn begin_stop(&self, id: &str) -> StopHandle      // 同步：cancel + 置 phase=Stopping，无 await
    match table.live.get(id) {
        None                                        => NotRunning,                                    // was_running=false
        Some(e) if e.ctx.phase() == FailedUnknown   => Quarantined(e.ctx.done_tx.subscribe()),        // P0-1：同一次明确失败
        Some(e) if e.ctx.phase() == Stopping        => Join(e.ctx.done_tx.subscribe()),               // §5.2：合流，不重复拆解
        Some(e)                                     => { e.ctx.phase = Stopping; e.cancel.cancel(); Claimed(e.ctx.done_tx.subscribe()) }
    }
pub async fn stop_endpoint(&self, id: &str) -> Result<bool, String>   // 签名不变（既有 30+ 调用点零改动）
{ let h = self.begin_stop(id); h.await_result().await }
```

- `begin_stop` 同步 ⇒ **Stop 的"受理"与"启动拆解"不可分割**：不存在"代次已 +1、desired 已写假、但 teardown 没启动"的中间态（原稿把拆解放在 handler 的 await 之后，正好有这个洞）。
- `stop_endpoint` 保留 async 签名 ⇒ `stop_lifecycle.rs:109`、`reconnect_backoff.rs:465`、`event_persistence.rs:668` 等调用点一字不改。
- **对外结果映射（逐条对应 §5.2）**：`Claimed`/`Join` ⇒ 等同一个 `outcome`（成功 200 `was_running=true`；失败 500 `STOP_FAILED`）；`NotRunning` ⇒ 200 `was_running=false`（**除非** `intent_unpersisted`，见下条）；`Quarantined` ⇒ 500 `STOP_FAILED`（detail 为 `ATTEMPT_FAILED_UNKNOWN…`，**不重复 teardown、不清除隔离**）。
- **`intent_unpersisted == true` 时不得返回正常成功**：即使 teardown 成功，也必须返回 500 `STOP_INTENT_NOT_PERSISTED`（§6.10.3）。这是 §5.2 第三行的**前置条件**——"teardown 成功 → 200"以"停止意图已落盘"为前提；两者同时失败时，用户必须看到失败。

### 6.5 代次隔离：快照写入必须带代次（P0-D）

**问题**：`Snapshot::upsert_endpoint` 最后写入者胜（`snapshot.rs:208-213`）⇒ 旧 attempt 的 STOPPED（`endpoint.rs:357-368`）可覆盖新 attempt 的 RUNNING（`endpoint.rs:597-605`）。

**方案**：把 `endpoints` 的值从 `EndpointStatus` 换成带代次的记录，并且**把墓碑放进同一把锁**（P0-4 的关键，不是可选优化）。

```rust
// snapshot.rs：一个锁 = 一张表 = 一个线性化点
struct EndpointTable {
    live: HashMap<String, EndpointRecord>,   // 替换原 HashMap<String, EndpointStatus>
    retired: HashMap<String, u64>,           // 墓碑：DELETE 时记录"已退休的最大代次"
}
struct EndpointRecord { gen: u64, status: EndpointStatus }
endpoints: RwLock<EndpointTable>,            // 唯一锁；**不另设 retired 锁**

pub fn upsert_endpoint(&self, gen: u64, status: EndpointStatus) -> bool {
    let mut t = self.endpoints.write().unwrap();          // ← 检查与写入在同一临界区
    if t.retired.get(&status.endpoint_id).is_some_and(|&r| gen <= r) {
        return false;                                     // 墓碑门：禁止复活已删除端点
    }
    if t.live.get(&status.endpoint_id).is_some_and(|rec| gen < rec.gen) {
        return false;                                     // 旧代次写入，丢弃
    }
    t.live.insert(status.endpoint_id.clone(), EndpointRecord { gen, status });
    true
}

pub fn remove_endpoint(&self, id: &str, gen_floor: u64) {
    let mut t = self.endpoints.write().unwrap();          // ← 同一临界区
    let floor = gen_floor.max(t.live.get(id).map_or(0, |e| e.gen));
    let r = t.retired.entry(id.to_string()).or_insert(0);
    *r = (*r).max(floor);
    t.live.remove(id);
    // point_meta / name_overrides / latest 的清理保持原样（不同键空间，见 §11-10）
}
```

**为什么这是线性化的（必须能自证，不能只说"加了锁"）**：

- 线性化点 = 取得 `endpoints` 写锁的那一刻。`upsert` 的"读墓碑 → 比代次 → 插入"与 `remove` 的"算 floor → 写墓碑 → 删除"都在**同一个** `RwLock` 的写临界区内，因此任意两个操作按锁获取顺序构成全序，每个操作的效果都等价于在它的线性化点上原子执行。
- 两种交错都安全：**(a) `upsert` 先拿到锁** ⇒ 它先落盘，随后 `remove` 在同一临界区删除并抬高墓碑 ⇒ 端点最终不存在；**(b) `remove` 先拿到锁** ⇒ 墓碑已就位，`upsert` 读到 `gen <= retired` ⇒ 直接丢弃。
- **不存在第三种交错**：因为"检查墓碑"与"写入"之间**没有释放锁**。这正是 P0-4 的实质——原设计把 `endpoints` 与 `retired` 放在两把独立锁下（F31：`Snapshot` 现有 6 把锁），于是必然存在"读完 retired、尚未写 endpoints"的窗口，DELETE 落在窗口内即复活。**两把锁的"检查—写入"非原子，是可构造的交错，不是概率问题。**
- 为什么不用 CAS：`RwLock<HashMap>` 里没有可 CAS 的单元格；单个 `AtomicU64` 只能保护"代次"这一个标量，保护不了"条目是否存在"与"墓碑"的**联合状态**。单锁是该路径最简且可证明的做法，且它只被状态迁移调用（频率 = 迁移次数，不是数据面热路径），不触碰 §22 的 50K/s 预算。

- `EndpointStatus` **不加字段**（F24：它是 REST 响应元素，加字段等于改 wire）⇒ 代次只存在于 map 的键结构里，`endpoint()`/`endpoints()` 的返回类型与序列化结果**完全不变**。
- 写入门禁只在一个地方：`set_status`（`endpoint.rs:447-465`）增加 `gen` 参数；`upsert_endpoint` 的生产调用点只有它（已核实：`crates/` 内仅 `endpoint.rs:456` 一处），churn 极小。
- 代次来源：`AttemptCtx.gen`，由 `start_endpoint` 分配；**per endpoint 计数器、永不回收**（`MesaManager` 内的 `HashMap<String, AtomicU64>` 只增不删；端点 DELETE 后重建继续递增，绝不复用）。理由：墓碑需要一个"该 id 曾发放过的最大代次"作下界，计数器一旦回收或重置，`retired` 就可能低于在途写入的 `gen`，墓碑门立刻失效。
- `gen_floor` 的来源：`MesaManager::gen_floor(id)` = 该 id **已发放的最大代次**（"下一个将要发放的值 − 1"）。`remove_endpoint` 的两个生产调用点都必须传它：DELETE 端点（`core-api/src/lib.rs:2125`，可从 `state.manager` 取）与 Bootstrap 失败补偿删除（`bootstrap.rs:348`）。若传 0 或省略，"从未写过状态的端点被删后，迟到写入 `gen>0` 仍会复活"。**差一（off-by-one）是这里的致命细节**：`gen_floor` 必须是"已发放的最大值"，**不能**用"下一个将要发放的值"——否则端点被删除后重建，第一个新 attempt 的 `gen` 正好等于墓碑值，`gen <= retired` 成立，**新端点的状态写入会被永久丢弃**（表现为"重建后端点永远不显示状态"）。实现处必须写一条单测钉住：`delete_then_recreate_status_write_is_accepted`。
- `run_endpoint` 需要拿到 ctx ⇒ 新增 `run_endpoint_with_attempt(..., ctx)`，**保留** `run_endpoint_with_heartbeat` 旧签名委托（ctx=None，`gen=0`），使 `subprocess_recovery.rs:186` 零改动；直调路径的 `gen=0` 因此**永远无法覆盖**任何正式 attempt（`gen≥1`），也**永远无法复活**已删端点（`0 <= retired` 恒成立）。

**为什么在有了 attempt 表之后还需要它**（诚实论证，避免"多余设计"质疑）：

1. **DELETE 路径真实可达**：`DELETE /endpoints` 在拆解期 `is_running==false` 会放行（`lib.rs:2113`），`remove_endpoint` 直接删条目（`snapshot.rs:440`）；随后旧 attempt 写 STOPPED/RUNNING ⇒ 已删端点"复活"。这条路径**与 attempt 表无关**，只能靠墓碑 + 代次门。
2. **顺序保证是隐式跨函数不变量**："条目移除晚于 STOPPED 写入"依赖 `endpoint.rs:638` 与 `:357` 的相对位置，任何一次重构都可能静默破坏；代次门把它变成**显式可断言**的约束，并有回归测试钉住。
3. **纵深防御**：F22/F23 存在绕过 Manager 直接 spawn attempt 的路径；代次门对它们同样生效（gen=0 的直调 attempt 无法覆盖 gen≥1 的正式 attempt）。
4. **原子性本身就是被复审点名的缺陷（P0-4）**：墓碑与状态写入分属两把锁时，"检查—写入"非原子；单锁版本才使"DELETE 之后绝不复活"成为**可证明**的性质，而不是"窗口很小"的经验判断。

**回归测试**（复审指定）：`concurrent_teardown_and_new_start_never_shows_stopped_while_collecting` —— 见 §9。补充说明：修复后 HTTP 路径上"旧 STOPPED 晚于新 RUNNING"已被 attempt 表**结构性**排除（条目移除晚于 STOPPED 写入），因此该场景的**确定性**红/绿放在 Snapshot 单测（`stale_gen_write_is_dropped`）与 DELETE 复活场景（集成）上；集成层的"不得长期显示 STOPPED 而实际在采集"以**不变量断言**表达（5 s 窗口内 `state==RUNNING` 且 `latest` 时间戳持续推进）。

**并发 DELETE + 迟到写入的专项回归（P0-4 指定）**：`concurrent_delete_and_late_status_write_never_resurrects`（单测 + 集成各一份，§9）。判据不是"跑很多次没出事"，而是**可判定的不变量**：`remove_endpoint` 返回之后，任何 `upsert_endpoint(gen<=gen_floor, …)` 的返回值必须为 `false`，且 `endpoint(id)` 恒为 `None`；把该断言放进一个 2 线程（upsert 循环 × remove）的压测里，断言在 `remove` 返回后的**每一次**读都成立。

### 6.6 防孤儿：insert-before-spawn + 拒绝并发 attempt（P0-E）

**现状**（`manager.rs:513-516`）：`insert` 是覆盖写，第二个 Start 会丢弃第一个 `RunningEntry`（JoinHandle + cancel token）⇒ 孤儿打到进程退出。

**方案（三处，缺一不可）**：

1. **insert-before-spawn，且拒绝已存在**：在**同一次**加锁内完成"检查存在 + 插入占位条目"，然后才 spawn。第二个 Start 在**检查处**就被拒绝，**根本没有 spawn 发生**，因此不存在"需要覆盖/终止旧条目"的情况——这比"insert 前显式终止旧条目"更安全（终止一个正在跑机床的 attempt 是危险动作，且会引入新的竞态）。
2. **条目里不放 JoinHandle**：attempt 任务自持 `AttemptGuard`，**正常完成时**由它自己移除条目（异常终止时**保留**并转隔离态，§6.3.2）。JoinHandle 可以被 detach 丢弃而**不产生孤儿**——孤儿的前提是"有人还活着但没人管"，而现在"活着"与"有条目"是同一件事。
3. **回填必须校验代次**：`start_endpoint` 在 spawn 之后不再需要回填 handle（无 handle），但若仍需记录 `pid` 等，必须 `if let Some(e) = t.live.get_mut(&id) { if e.ctx.gen == gen { … } }`，**绝不无条件 `insert`**。条目创建一律走 `Entry::Vacant`（见下），使"覆盖写"在类型层面不可表达；任何退回 `HashMap::insert` 的改动都必须在 review 中被拒绝。

```rust
pub fn start_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {   // 签名不变
    let mut t = self.attempts.lock().unwrap();          // AttemptTable
    // 用 Entry API 而不是 insert：覆盖写在类型层面不可表达（这是 P0-E 的第一道保证）
    match t.live.entry(cfg.endpoint_id.clone()) {
        std::collections::hash_map::Entry::Occupied(e) => {
            return Err(match e.get().ctx.phase() {
                FailedUnknown => format!("ATTEMPT_FAILED_UNKNOWN: endpoint `{}` gen {} awaiting \
                                          supervisor verification", cfg.endpoint_id, e.get().ctx.gen),
                Stopping      => format!("STOP_IN_PROGRESS: endpoint `{}` is stopping", cfg.endpoint_id),
                _             => format!("endpoint `{}` already running", cfg.endpoint_id),
            });
        }
        std::collections::hash_map::Entry::Vacant(v) => {
            // 其余同步门禁（event store、find_driver）保持原位置：manager.rs:487-495
            let gen = self.next_gen(&cfg.endpoint_id);      // AtomicU64 per endpoint，单调、永不回收（§6.5）
            let ctx = Arc::new(AttemptCtx::new(gen));
            let guard = AttemptGuard::new(Arc::clone(&self.attempts), cfg.endpoint_id.clone(), gen,
                                          ctx.clone(), self.quarantine_tx.clone());
            v.insert(AttemptEntry { ctx: ctx.clone(), cancel: cancel.clone() });
            drop(t);
            tokio::spawn(async move {
                let g = guard;                                  // 任务自持，panic 时同样 drop
                let outcome = run_endpoint_with_attempt(..., ctx.clone()).await;
                ctx.outcome.set(outcome);                       // 只有正常返回才走到这里
                g.commit();                                     // P0-1：显式完成标记
            });                                                 // handle 直接丢弃
            return Ok(());
        }
    }
}
```

**"无孤儿"如何验证**：

| 层 | 验证手段 |
|---|---|
| 结构不变量 | 条目创建走 `Entry::Vacant`（`Occupied` 一律拒绝）⇒ 覆盖写在 API 层面不可达；任意时刻每 `endpoint_id` 至多一条目；条目**只**由 `AttemptGuard::drop` 在 `committed==true` 时（且 gen 匹配）移除，`committed==false` 时保留为隔离态；spawn 后的任何回填都必须 `if e.ctx.gen == gen` 校验，绝不无条件 `insert` |
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

Start 侧仍用 409（与"已在运行"同状态码），body 形状不变，只增 `code` 字符串；Stop 侧新增一个 500 code（见下）。

| code | HTTP | 触发条件 | 判定位置 |
|---|---|---|---|
| `SUPERSEDED_BY_STOP` | 409 | 票据 `stop_gen` 与当前不等（Start 在 Stop 受理前 admit、之后 commit） | `commit_start` 步骤 1（契约行 1） |
| `STOP_IN_PROGRESS` | 409 | attempt 表存在且 `phase==Stopping`（Stop 已受理、teardown 未完成） | `start_endpoint` 同步拒绝（契约行 2） |
| `ATTEMPT_FAILED_UNKNOWN` | 409（Start）/ 500（Stop） | 条目存在且 `phase==FailedUnknown`（P0-1 隔离态，§6.3.3） | `start_endpoint` 拒绝；`begin_stop` 返回 `Quarantined` |
| `INTENT_UNPERSISTED` | 503（Start） | `gate.intent_unpersisted==true`（Stop 意图落盘失败，§6.10.3） | `commit_start` 步骤 0 |
| `START_INTENT_NOT_PERSISTED` | 500（Start） | `commit_start` 步骤 3 落盘失败（attempt 已启动、不可回滚） | §6.4 步骤 3 |
| `STOP_INTENT_NOT_PERSISTED` | 500（Stop） | `admit_stop` 落盘失败；即使 teardown 成功也返回它 | §6.10.3 |
| `CONFLICT`（不变） | 409 | 条目存在且 `phase` 既非 `Stopping` 也非 `FailedUnknown`（真的已在运行） | 同上，沿用现有文案"endpoint 已在运行" |

**对既有"冻结码集合"的影响（必须同 PR 处理，否则合同测试会红）**：`stop_lifecycle.rs:41-49` 的 `assert_frozen_stop_code` 限定 `stop_endpoint` 的 `Err(String)` 只能以 7 个 `EVENT_*` 码开头。隔离态会让 `stop_endpoint` 返回 `ATTEMPT_FAILED_UNKNOWN: …`，因此**该常量必须从 7 个扩到 8 个**（新增 `ATTEMPT_FAILED_UNKNOWN`）。`STOP_INTENT_NOT_PERSISTED` **不进**该集合——它由 `core-api` 的门产生，不经过 `stop_endpoint` 的 `Err`。这是本轮修订发现的**唯一一处对既有冻结断言的扩写**，必须在 PR 描述里单独声明。

理由：测试与现场证据需要**机器可判别**"Stop 优先拒绝"、"已在运行"与"隔离态"三者；body 结构、路由、字段均不变，不构成 breaking change。`manager` 侧沿用仓库既有的"精确码前缀"约定（`EVENT_STORE_UNAVAILABLE: …` 在 `manager.rs:489`、`EVENT_DRAIN_FAILED: …` 在 `:543`），故 `Err(String)` 签名不变，core-api 按前缀映射状态码与 code。

### 6.9 取舍评价表

| 维度 | 评价 |
|---|---|
| 改动面 | `driver-manager`：attempt 表 + `AttemptGuard`（`manager.rs`）、`attempt_reaper` 监督清理（新增）、`set_status` 加 gen（`endpoint.rs`）、`upsert_endpoint` 加 gen + 单锁墓碑 + `remove_endpoint(gen_floor)`（`snapshot.rs`）、`LIVE_CHILDREN`（`process.rs`）；`core-api`：新增 `lifecycle.rs` + `AppState` 1 字段，改写 start/stop handler（`lib.rs:2136-2191`）、DELETE 传 `gen_floor`（`:2125`）、Bootstrap 两处（`bootstrap.rs:110-125`、建档记录 `:297`）；**`config-store`：两条 UPDATE 去掉 `desired_running` 列（§6.10，无表结构变更）**。`MesaManager` 的**既有公共签名不变**（`start_endpoint`/`stop_endpoint` 保留），新增 `begin_stop`/`has_live_attempt`/`current_gen`/`gen_floor` 四个查询/同步方法 |
| 跨驱动影响 | 无：门与协议、驱动、Descriptor 无关，S7/FOCAS2/OPC UA/Simulator 一致 |
| 回归风险 | 中：行为变化四处——竞态分支下 Start 200→409（含新 code）；并发 Stop 由"立即 200 false"改为"合流后返回同一结果"（§5.2）；`desired_running` 写入时机由"teardown 之后"前移到"受理时"（§5.3）；**Stop 意图落盘失败由静默成功改为 500**（§6.10.3）。`mesa-web` 对 start/stop 只读 `status` 与 `error.message`，不匹配 code（`apps/mesa-web/src/api.ts:146-147`、`deviceModel.ts:446-450`），其"stop→apply→start"是**顺序 await**（`deviceModel.ts:474-494`），不会自撞；但 500 会让其提示"停止失败"，这是**期望行为** |
| A1 退避 | 不受影响：门在 attempt 之上，`backoff_idx` 的生成/归零语义（`endpoint.rs:315`、`:416-418`）一字未改；反而**减少**了一次"新 attempt 从 0 档重来"的伪重连。新增的观察器与 `attempt_reaper` 只读/只清理残留，**绝不** cancel 活着的 attempt、绝不重启（否则等于引入 §10-1 拒绝的对账器） |
| wire/IPC | 不改（`EndpointStatus` 字段不变，代次只在 map 键结构里） |
| 持久化 | 不加列：`gen`/`last_completed_gen` 只在内存；`desired_running` 仍是既有单列 UPDATE（`config-store/src/lib.rs:1185`），但**只允许由 `set_desired_running` 写入**（§6.10） |
| 新增常驻资源 | 一个 `attempt_reaper` 任务 + 一条 `quarantine_tx` 通道（§6.3.3）；无定时轮询（只由 panic 路径触发），空载零开销 |

### 6.10 `desired_running` 的写路径拆分（P0-2）

**已核实的代码事实（本轮逐行复核）**：

| # | 事实 | 位置 |
|---|---|---|
| W-a | `update_endpoint_with_secrets()` 的 UPDATE 带 `desired_running=?4`，值取自调用方传入的 `rec` | `config-store/src/lib.rs:1091-1094` |
| W-b | `update_endpoint()` 的 UPDATE 同样带该列（当前无生产调用方，仅单测，但仍是公开 API） | `config-store/src/lib.rs:1165-1168` |
| W-c | `core-api` 的 PUT 是"读 → 长 `await` → 回写"：`rec` 在 `:1979` 读出，`:1998` 起 `await get_descriptor`（懒加载，可能起临时驱动进程，F8），`:2096-2099` 才落盘 | `core-api/src/lib.rs:1979-1987`、`:1998`、`:2094-2099` |
| W-d | Bootstrap 在建档事务里就以 `desired_running=true` 插入（`body.start` 缺省 true） | `bootstrap.rs:297` + `config-store/src/lib.rs:1259-1261` |
| W-e | Bootstrap 启动成功后再写一次真；失败只记 error，不翻转 start 结果 | `bootstrap.rs:110-125` |
| W-f | `set_desired_running()` 是唯一的"只写意图"函数（单列 UPDATE） | `config-store/src/lib.rs:1178-1189` |

**结论（必须写进设计与代码注释）**：**"LifecycleGate 是 `desired_running` 唯一写者"在当前代码中不成立。** 一次普通的改名/改连接（W-a + W-c）就足以把 Stop 刚写入的 `false` 覆盖回**读时的旧值 `true`**，而这次覆盖完全不经过任何生命周期门；W-d 则在创建端点时写入 `true`，同样绕过门。

#### 6.10.1 拆分原则（一句话）

**普通 Endpoint 配置更新（名称 / 设备 / 连接 / Secret）不得再修改 `desired_running`；运行意图只能经生命周期门写入。**

#### 6.10.2 具体改法（哪个函数、哪个 SQL、以及"旧对象写回"为何不再覆盖意图）

| # | 改动 | 具体内容 |
|---|---|---|
| 1 | `update_endpoint_with_secrets()` | UPDATE **去掉 `desired_running` 列**：`UPDATE endpoints SET device_id=?1, name=?2, connection_json=?3, updated_at_ns=?4 WHERE id=?5`；占位符随之重排，`:1093` 里的 `desired_running as i32` 参数一并删除。函数注释写明"**本函数永不写运行意图**" |
| 2 | `update_endpoint()` | 同样去掉该列（`:1166-1167`）。它当前无生产调用方，但保留时必须与 (1) 同语义，否则将来被复用即复发 |
| 3 | `EndpointRecord.desired_running` | **保留字段**（它是 `get_endpoint`/`list_endpoints` 的读取结果，也是 REST 输出，`lib.rs:1495`、`:1563`），但**从"可写字段"降级为"只读快照"**：`UPDATE ... SET desired_running` 只允许出现在 `set_desired_running()` 里 |
| 4 | `core-api` PUT（`lib.rs:1955-2107`） | **流程不用改**：`rec` 仍从 `:1979` 读出并整体回写，但 UPDATE 已不含该列 ⇒ 旧对象里的 `desired_running` 在 SQL 层被**物理忽略** |
| 5 | Bootstrap 建档（`bootstrap.rs:297`） | 建档事务里一律写 `desired_running=false`（"刚建好 = 未运行"）；`body.start=true` 的启动意图改由 `commit_start` 步骤 3 在启动成功后写入。收益：DB 里不再可能出现"从未经过门却为 true"的行 |
| 6 | Bootstrap 启动（`bootstrap.rs:110-125`） | `set_desired_running(id, true)` 改为 `gate.write_desired(id, true)`（同 §6.4 步骤 3：失败显式、不再静默） |
| 7 | 三个 handler 写点（`lib.rs:2155`、`:2171`、`:2180`） | **全部删除**，改由 `commit_start` / `admit_stop` 在门内写入（§6.4） |

**"旧对象写回不再覆盖意图"的保证链（三层，缺一不可）**：

1. **SQL 层（主保证）**：UPDATE 不含该列 ⇒ 写回**物理上**无法触及意图，不依赖调用方自觉。
2. **类型层（建议）**：更彻底的做法是引入 `EndpointMetaUpdate`（不含 `desired_running`）作为这两个 store 函数的入参，使"用旧对象写回"在**编译期**不可表达。**待确认**是否接受这个 churn（§11-9）。
3. **门层**：`EpGate.desired` 是内存权威；每次 `set_desired_running` 成功后 `desired_seq += 1`，观察者用它证明"读到的是门写的值"，而不是某次 PUT 回写的旧值。

**验收判据（可执行）**：`stale_endpoint_record_writeback_keeps_desired_running`（config-store 单测，§9）—— 读出 `rec`（`desired=true`）→ `set_desired_running(id,false)` → 用旧 `rec` 调 `update_endpoint_with_secrets` → 断言 DB 仍为 `false`。该用例在旧代码上**必红**（W-a），是本条的主门。

#### 6.10.3 Stop 意图落盘失败时的行为（显式失败 + 安全方向 + 禁止不受控重启）

**必须消灭的现状语义**：`let _ = set_desired_running(false)`（`lib.rs:2171`、`:2180`）⇒ 落盘失败仍返回 200 ⇒ 重启后 `desired_running` 仍为 `true` ⇒ **自动拉起一个用户明确要求停止的端点**。

| 要求 | 落地 |
|---|---|
| **显式失败** | `admit_stop` 记录 `gate.intent_unpersisted=true`；Stop 最终响应为 **500** + `code=STOP_INTENT_NOT_PERSISTED`（**即使 teardown 成功也不得返回 200**，见 §6.4 末条），并 `tracing::error!` 带 `endpoint_id` 与 DB 错误 |
| **继续执行安全方向的停止处理** | 落盘失败**不阻止** `begin_stop` 与 teardown：停下来的安全收益与 DB 无关。**禁止**因为"意图没写进去"就跳过停止 |
| **禁止不受控重新启动** | `intent_unpersisted=true` 期间 `commit_start` 一律返回 **503 `INTENT_UNPERSISTED`**（§6.4 步骤 0）——不接受新的 Start，直到意图补写成功。补写时机：teardown 结束后的 finalize（同步重试一次）与下一次 Stop（再写一次）；补写成功后清标记并 `desired_seq += 1` |

**残余风险（诚实声明）**：若 Core 在"意图落盘失败 + teardown 已完成"之后崩溃，重启时 DB 仍为 `true` ⇒ 开机恢复（`main.rs:140-161`）会拉起该端点。**本设计不能消除它**（DB 是唯一持久真值），但把窗口从"静默成功"压缩为"显式 500 + 禁止新 Start + error 日志"，且不新增持久化列（§10-4）。要彻底消除需在启动时判断"上次进程是否干净退出"，属独立设计（§11-11）。

### 6.11 完成代次凭证 `last_completed_gen`（P0-3）

**问题（本轮新增，且直接导致"正常停止反而不满足判据"）**：**原设计** §7.3 第 3 步要求 `snapshot.gen == attempt_gen(current)`。若 attempt 条目在 teardown 完成时被移除，`attempt_gen(current)` 就**没有值可比** ⇒ 该 tick 永不通过 ⇒ `await_settled` 永远 `NotSettled`。**正常停止反而无法满足判据**——这是设计缺陷，不是测试缺陷。

**方案**：在 attempt 表**同一把锁**内维护 `last_completed_gen: HashMap<String, u64>`（§6.2），并提供唯一读取口：

```rust
/// 当前代次：有条目取条目代次，无条目取"最后一次已证明完成的代次"。
/// 两者取自同一把表锁 ⇒ 不可能读到"既无条目又无凭证"的组合（P0-3）
pub fn current_gen(&self, id: &str) -> Option<u64> {
    let t = self.attempts.lock().unwrap();
    t.live.get(id).map(|e| e.ctx.gen).or_else(|| t.last_completed_gen.get(id).copied())
}
```

**生命周期（何时写、何时失效、与墓碑的关系）**：

| 事件 | 对 `last_completed_gen` 的影响 |
|---|---|
| **正常完成**（`commit()` 之后的 `Drop`） | **写入** `= gen`，且与"移除条目"在**同一临界区**（§6.3.2 的 Drop 代码）⇒ 不存在"条目已删、凭证未写"的空窗 |
| **异常终止**（隔离态） | **不写**（没有完成证据）；条目仍在，`current_gen` 取条目代次 ⇒ 判据仍因"条目存在"而不通过 |
| **隔离清除**（`attempt_reaper` 复核通过） | **写入** `= gen`（异常 attempt 也需要终结凭证，但 `outcome` 保持 `Err`） |
| 新 attempt 启动 | 不写、不清除；新 `gen > last_completed_gen`，有条目时以条目为准 |
| **端点 DELETE** | **不清理**（内存项随进程结束）。它与快照墓碑是**两层**：`retired` 管"已删端点不得复活"，`last_completed_gen` 管"停止判据的代次比对"，互不替代 |
| 端点重建 | 代次计数器不复用（§6.5）⇒ 重建后 `gen` 仍大于旧值，旧凭证不会被误认作当前代次 |
| **进程重启** | 全部丢失（只在内存）。重启后表为空、`current_gen=None` ⇒ 判据 3/4 以"无在途"通过；这符合"重启即无 attempt"的事实，但**不得**据此声称"重启前已停干净"（现场判据见 §7.5） |

**`await_settled()` 如何使用它**：采样把 `attempt_gen(current)` 换成 `current_gen(id)`（条目代次优先，回退到 `last_completed_gen`），其余条件不变。

- **正常停止的完整时序**：teardown 结束 → 该 attempt 写 `STOPPED(gen)`（`endpoint.rs:357-368`）→ `Drop` 在同一临界区写 `last_completed_gen=gen` 并移除条目 → 采样读到 `snapshot.gen == gen == current_gen` 且 `entry_absent` ⇒ **通过**。这正是本条要修好的路径。
- 若采样落在"新 Start 已进"之后（`current_gen` 变成新 gen，与 `snapshot.gen` 不等）⇒ 该 tick 不通过、窗口重置。**方向正确**：后者本来就不是 settled。
- **`current_gen == None`（从未有 attempt / 重启后）**：判据 1 **没有基准**，`await_settled` 必须返回 `NotSettled{ reason: NoGenBaseline }`，**不得**当作"已停干净"。这是 fail-closed 的另一面：没有证据 ≠ 证据成立。

---

## 7. 完整停止判据六条（P0-C）

### 7.1 判据总表

"停干净"= 下列 6 条**同时**成立，并在稳定窗口内持续成立。逐条给出可判定方式、抗旧值机制、当前代码是否具备。

| # | 判据 | 可判定信号（来源） | 如何证明不是旧值 | 当前代码 | 需新增什么 |
|---|---|---|---|---|---|
| 1 | **当前代次为 `STOPPED`** | `Snapshot` 中 `(gen, state)` 原子对，且 `gen == current_gen(id)`（条目代次优先，回退 `last_completed_gen`，§6.11） | 写入按代次过滤（旧代次被丢弃）；读取在**同一次加锁**内取 `(gen, state)`，不可能拼出"新 gen + 旧 state" | **部分具备**：STOPPED 的写入时机已经正确（`endpoint.rs:638-647` → `:357-368`，teardown 之后才写），但 `upsert_endpoint` 无代次门（`snapshot.rs:208-213`） | `EndpointRecord{gen,status}` + 代次门 + 墓碑（§6.5）+ `last_completed_gen`（§6.11） |
| 2 | **`desired_running=false`** | `store.get_endpoint(id).desired_running`（`config-store/src/lib.rs:1138-1147`）+ gate 的 `desired_seq` | `desired_seq` 单调：观察者记录 Stop 受理时的 seq，要求 `observed_seq >= seq_at_admit` 且值为 false；**"单写者"只有在 §6.10 的写路径拆分落地后才成立**（现状另有两条 UPDATE 与一条建档 INSERT 会写该列） | **不具备**：写点在 handler（`lib.rs:2171`/`:2180`），**取消即丢失**；无写入序号；且存在非门写者（§6.10） | 把写假前移到 `admit_stop`（同步临界区）+ `desired_seq` + gate 懒初始化自 DB + **§6.10 写路径拆分** |
| 3 | **无在途 Start / Stop** | 在途 Start = gate `inflight_starts`；在途 Stop = attempt 表存在且 `phase ∈ {Stopping, FailedUnknown}` | 两者都是"在途"的**定义本身**，不是缓存副本（无副本可旧） | **不具备**：当前无任何表示 | `LifecycleGate` + attempt 表（含隔离态） |
| 4 | **无未完成的 attempt（含孤儿）** | attempt 表：该 `endpoint_id` 无条目（条目 = 未完成；正常完成的条目在**同一临界区**内先写 `last_completed_gen` 再移除） | 条目**只**在 spawn 前创建、**只**由 attempt 任务自身按 gen 匹配移除（且异常终止**不**移除，§6.3.2）⇒ 不存在"漏登记"的孤儿，也不存在"panic 后被当作已完成"；gen 单调，读到的 gen 与当前 gen 比对 | **不具备**：`is_running` 在拆解期就假（`manager.rs:535`）、W3 孤儿不在表内（`:513-516`） | 代次化 attempt 表 + `AttemptGuard` + 隔离态（§6.2/§6.3.2/§6.6） |
| 5 | **对应驱动进程与目标 TCP 会话已收敛** | (a) `TeardownDone(gen)`（§6.3.1，蕴含 `child.wait()` 已返回，`process.rs:143-154`）；(b) `session_released == true`（teardown 末尾 registry 中该 `endpoint_id` 键不存在，`endpoint.rs:728`）；(c) 全局 `LIVE_CHILDREN` 无该 attempt 的 pid | (b) 因并发 attempt 被拒绝（§6.6），registry 的该键在任一时刻只可能属于当前 attempt ⇒ 不存在"读到上一代"；再叠加 gen 单调比对 | **部分具备**：进程收敛已由 teardown 保证（E3），但 `wait_reader_done` 结果被丢弃（F20，`endpoint.rs:716-721`）、无 registry 释放证据、无全局计数 | `AttemptCtx.session_released/remote_eof/pid` + `LIVE_CHILDREN` |
| 6 | **经过规定的稳定观察窗口** | `gate.await_settled(id, timeout)`：250 ms × 2 个采样（500 ms），每个采样同时读 1~5 条全部来源。**这是 Core 内部快速收敛检查，不是现场判据**（§7.5） | 每 tick 重读全部来源并记录 `gen`；窗口内任一条目/代次/phase 变化即**重置窗口**；旧代次的 STOPPED 写入因代次门被丢弃，即使落盘其 gen ≠ 当前 gen ⇒ 该 tick 不通过 ⇒ 窗口重置 | **不具备**：全仓库无任何观察窗口 | 只读观察器（§7.3）+ **现场 10 s 连续稳定观察（§7.5，独立满足）** |

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
2. 读 `current_gen(id)`（条目代次优先，回退 `last_completed_gen`，§6.11）与 `phase`；
3. 要求 `snapshot.gen == current_gen(id)`，否则该 tick **直接不通过**（这正是"旧 attempt 的 STOPPED 覆盖"的检测器：被覆盖时 gen 会对不上）；`current_gen == None` ⇒ `NotSettled{NoGenBaseline}`（无基准 ≠ 成立）；
4. 要求该 `endpoint_id` 在 attempt 表中**无条目**（即 `entry_absent`）；等价地 `phase` 不得是 `Starting/Running/Stopping/FailedUnknown`；
5. 要求 `desired==false` 且 `desired_seq >= seq_at_admit`，且 `intent_unpersisted == false`（§6.10.3：意图没落盘就不能算"停干净"）；
6. 要求 `inflight_starts == 0`；
7. 任一 tick 失败或窗口内出现 gen 变化 ⇒ 窗口从 0 重新计时。

因此"快照被旧 attempt 覆盖成 STOPPED、而实际在采集"这个具体故障，**不可能**让判据 6 通过：要么 gen 对不上（第 3 步），要么条目还在（第 4 步），要么 `inflight_starts>0`（新 Start 在途，第 6 步）。

**窗口参数（内部快速收敛检查）**：`SETTLE_TICKS = 2`、`SETTLE_TICK = 250ms`（合计 500 ms）。选值理由：需大于一次"状态写入 + 一次 REST 读"的抖动（ms 级），又必须远小于 Stop 的 teardown 预算（≈35 s），避免把内部判定拖成运维不可接受的时长。**待确认**是否需要按 endpoint 配置（§11-6）。

**这条 500 ms 检查的作用域必须写死（P1-5）**：它只用于 **Core 内部状态收敛判定**（测试断言、日志水位、诊断）。**它不是现场"停干净"判据，也不能替代现场的 10 s 连续稳定观察**——两者的采样对象、目的与失败代价都不同，见 §7.5。

**只读纪律（硬性）**：观察器只读、只记日志与水位，**绝不**写 `desired_running`、**绝不** cancel/restart attempt、**绝不**改快照。一旦它开始"纠正"，就变成 §10-1 明确拒绝的对账器/看门狗。

### 7.4 判定算法（伪代码）

```rust
async fn await_settled(&self, id: &str, timeout: Duration) -> Settled {
    let seq_at_admit = self.seq_at_last_stop(id);          // Stop 受理时记录
    let deadline = Instant::now() + timeout;
    let mut ok_ticks = 0;
    let mut last_gen = None;
    while Instant::now() < deadline {
        let s = self.sample(id);                            // 一次取全部来源（含 last_completed_gen）
        let Some(cur) = s.current_gen else {                // P0-3：无代次基准 ⇒ 明确失败，不猜
            return NotSettled { reason: NoGenBaseline, .. };
        };
        let same_gen = last_gen.map_or(true, |g| g == cur);
        let pass = s.snapshot_gen == cur
                && s.snapshot_state == "STOPPED"
                && s.entry_absent                           // 含隔离态：FailedUnknown 也算有条目
                && s.desired == Some(false)
                && s.desired_seq >= seq_at_admit
                && !s.intent_unpersisted
                && s.inflight_starts == 0;
        ok_ticks = if pass && same_gen { ok_ticks + 1 } else { 0 };
        last_gen = Some(cur);
        if ok_ticks >= SETTLE_TICKS { return Settled { gen: cur, .. } }
        tokio::time::sleep(SETTLE_TICK).await;
    }
    NotSettled { last_observation: … }                      // 附失败原因，供现场定位
}
```

### 7.5 Core 内部 500 ms 稳定窗口 **≠** 现场 10 s 判据（P1-5）

| 维度 | Core 内部快速收敛检查 | 现场"停干净"判据 |
|---|---|---|
| 时长 | `250 ms × 2 = 500 ms`（§7.3） | **连续 10 s**（现场要求，独立满足） |
| 目的 | 内部状态是否已收敛：供测试断言、日志水位、诊断与排障 | 现场监护人据此判定"可以进入下一步/可以断电/可以上人" |
| 采样对象 | 进程内 6 条判据的**内存视图**（快照 + attempt 表 + gate） | 外部可观测面：`GET /api/v1/endpoints`（`state`）、`GET /api/v1/endpoints/{id}`（`desired_running`）、驱动 pid 是否存活、目标 TCP 会话是否已断（`netstat`/驱动日志） |
| 谁能证明 | 只能证明"Core 认为自己停了" | 必须证明"从机床侧看也确实停了" |
| 失败代价 | 测试红、日志噪声 | 误判 ⇒ 带电作业/二次启动 ⇒ **安全事故** |

**硬性规则**：

1. **前者不能替代后者**：`await_settled` 通过 **不构成**现场判据；任何"500 ms 绿了就算停干净"的结论都是把内部视图当外部事实。
2. **现场判据必须独立满足 10 s**：在 10 s 窗口内**每一次**采样（建议 ≥10 次，间隔 ≤1 s）都必须满足同一组外部条件；任一采样看到 `state != STOPPED`、`desired_running != false`、pid 存活或会话未断 ⇒ **窗口从 0 重新计时**。
3. **10 s 是现场阈值，不是 Core 参数**：Core 不为它新增配置项、不做自动判定、不写 `desired_running`（§10-1/§10-2 不变）。它由现场证据脚本/监护流程执行，结果记入验证文档。
4. **两者都要留证据**：Core 侧留 `await_settled` 的 tick 记录与 `current_gen`；现场侧留 10 s 采样序列（时间戳 + 每次采样的 `state`/`desired_running`/pid/会话状态）。**只有后者能作为 G7 的现场放行证据。**
5. **不得用"Stop 返回 200 `was_running=true`"代替 10 s 观察**：它只证明"这一次 teardown 返回了"，不证明"之后 10 s 没有重新拉起"（自动重连路径仍未修，§4-③ 首行、§10-5）。

---

## 8. 被否方案（保留原论证）

### 方案 B：整段串行化（每端点一把 async 锁，覆盖整个 handler）

- 改动面最小（锁住 `lib.rs:2136-2191` 两个函数）。
- **否决理由 1（安全）**：锁不区分方向。Stop 会排在在途 Start 的构造之后（最长十几秒），而 Stop 是安全方向，文档 §5.3 要求 Stop 到达即受理；用普通 Mutex 无法给 Stop 优先级。
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
5. **P0-1 隔离路径确定性复现（本轮新增）**：在 attempt 任务内注入一次 panic（测试专用 hook，位置在 `teardown_attempt` 的第 2 步之前），断言 (a) 条目仍在且 `phase==FailedUnknown`、(b) `POST /start` → 409 `ATTEMPT_FAILED_UNKNOWN`、(c) `POST /stop` → 500 且 detail 以 `ATTEMPT_FAILED_UNKNOWN` 开头（两个并发 Stop 拿到**同一条**）、(d) 驱动 `attempt_reaper` 完成复核后条目移除、`last_completed_gen==gen`、`outcome` 仍为 `Err`、(e) 此后 Start 成功。**旧设计（Drop 无条件删条目）在 (b) 处会 200 —— 必红。**
6. **P0-2 写回覆盖确定性复现（本轮新增）**：不需要时序构造——读出 `rec`（`desired=true`）→ `set_desired_running(false)` → 用旧 `rec` 调 `update_endpoint_with_secrets` → 断言 DB 仍为 `false`。见 `stale_endpoint_record_writeback_keeps_desired_running`。

**用例清单**

| 层 | 用例名（建议） | 断言点 | 旧码/新码 |
|---|---|---|---|
| 单测 | `admit_start_rejected_while_attempt_stopping` | 条目 `phase==Stopping` 时 `start_endpoint` 返回 `STOP_IN_PROGRESS`，且**未**触碰 `desired_running`、未 spawn | 新码绿（旧码无此 API） |
| 单测 | `in_flight_start_superseded_by_stop_admission` | admit_start → admit_stop → commit_start 返回 `Superseded`，无 attempt 产生、`desired` 仍假 | 同上 |
| 单测 | `stop_priority_holds_until_teardown_completes` | 挂住 teardown 期间 Start 恒被拒；完成后放行 | 同上 |
| 单测 | `start_after_stop_completed_succeeds` | Stop 全部完成后 admit/commit 成功，`desired=true`、新 gen > 旧 gen | 同上 |
| 单测 | `two_starts_only_one_attempt` | 两个 Start 同代次，恰好一个成功、一个 `AlreadyRunning`；attempt 表条目数 == 1；`Entry::Occupied` 分支被命中、从未发生覆盖 | 同上 |
| 单测 | `stale_gen_write_is_dropped`（`snapshot.rs`） | 先 `upsert(gen=2, RUNNING)` 再 `upsert(gen=1, STOPPED)` ⇒ 快照仍 `RUNNING`/gen=2 | **旧码无 gen 参数**，等价场景必红 |
| 单测 | `settled_after_normal_stop_uses_last_completed_gen` | **正常 Stop 完成后**（条目已移除）`await_settled` **必须通过**，返回 `gen` == 被移除条目的 gen | **P0-3 主门：旧设计在此永远 `NotSettled`**（条目已删 ⇒ 无可比代次） |
| 单测 | `settled_without_gen_baseline_is_not_settled` | `current_gen==None` ⇒ `NotSettled{NoGenBaseline}`，不得判为 settled | 新码绿（防"无证据即成立"） |
| 单测 | `attempt_panic_quarantines_and_blocks_start` | panic 后条目保留、`phase==FailedUnknown`、`current_gen` 仍为该 gen、`start_endpoint` 返回 `ATTEMPT_FAILED_UNKNOWN`、`done_tx.subscribe()` 收到 `Err` | 新码绿；**旧设计必红**（Drop 无条件删条目） |
| 单测 | `quarantine_cleared_only_after_convergence_proof` | 复核未通过 ⇒ 条目保留、Start 仍被拒；复核通过 ⇒ 条目移除、`last_completed_gen==gen`、`outcome` 仍为 `Err` | 新码绿 |
| 单测 | `stale_endpoint_record_writeback_keeps_desired_running`（`config-store`） | 用旧 `rec`（`desired=true`）写回后 DB 仍为 `false` | **P0-2 主门：旧码必红**（F25） |
| 单测 | `bootstrap_create_never_writes_desired_true` | 建档事务后 DB 为 `false`；`start=true` 时由门写入 `true` | 新码绿（F28） |
| 单测 | `concurrent_delete_and_late_status_write_never_resurrects`（`snapshot.rs`） | 2 线程：`upsert` 循环 × `remove_endpoint(gen_floor)`；`remove` 返回后每次 `endpoint(id)` 均为 `None`，`upsert(gen<=floor)` 均返回 `false` | **P0-4 主门：两锁版本可构造复活（红）** |
| 单测 | `deleted_endpoint_write_is_dropped_by_tombstone` | `remove_endpoint` 后 `upsert(gen<=retired, RUNNING)` 被丢弃，条目不存在 | 新码绿 |
| 单测 | `delete_then_recreate_status_write_is_accepted` | DELETE（`gen_floor`=已发放最大值）→ 端点重建 → 新 `gen`（= floor+1）的写入**必须被接受**，不得被墓碑永久拒绝 | 新码绿；**钉住 `gen_floor` 的 off-by-one**（§6.5） |
| 单测 | `settle_window_rejects_stale_stopped` | 构造 `snapshot.gen != current_gen` 的采样 ⇒ `await_settled` 不通过 | 新码绿 |
| 集成 | `start_during_stop_teardown_is_rejected_and_does_not_resurrect`（`stop_lifecycle.rs`） | 栅栏挂住 Stop 期间 `POST /start` → 409 + `code=STOP_IN_PROGRESS`；`gate.release()` 后 Stop 返回 200 `was_running=true`；随后 `!has_live_attempt`、`snapshot.state==STOPPED`、`desired_running==false`、`await_settled` 通过 | **旧红 / 新绿** |
| 集成 | `stop_protection_survives_handler_cancel` | 丢弃 Stop future 后保护仍在（见上"取消路径确定性复现"） | **旧红 / 新绿** |
| 集成 | `second_stop_joins_inflight_teardown` | 并发两个 Stop：两个都 200 且 `was_running=true`，teardown 只发生一次（drain 计数 1），第二次不返回 `was_running=false` | 旧码：第二个立即 200 false（红） |
| 集成 | `double_start_only_one_attempt` | 并发两 Start：一个 200、一个 409；Stop 后无条目、`LIVE_CHILDREN==0`、驱动 pid 不再存活 | **旧红（孤儿）/ 新绿** |
| 集成 | `concurrent_teardown_and_new_start_never_shows_stopped_while_collecting` | 5 s 内快照 `state==RUNNING` 且 `latest` 时间戳持续推进；`gen` 等于当前 attempt gen | 旧码可长期 STOPPED（红） |
| 集成 | `deleted_endpoint_not_resurrected_by_late_status_write` | DELETE 期间旧 attempt 写 STOPPED/RUNNING 后，`/api/v1/endpoints` 中不得重新出现该 id | 旧码红 |
| 集成 | `put_endpoint_does_not_clobber_desired_running` | 先 Stop（`desired=false`），再 PUT 改名/改连接（走完 Descriptor 异步校验），完成后 `GET /endpoints/{id}` 仍为 `false` | **旧码必红**（W-a + W-c） |
| 集成 | `stop_intent_persist_failure_is_explicit_and_blocks_restart` | 注入 `set_desired_running` 失败：Stop → 500 `STOP_INTENT_NOT_PERSISTED`，但 teardown 已完成（`!has_live_attempt`）；随后 `POST /start` → 503 `INTENT_UNPERSISTED`；补写成功后 Start 可用 | 新码绿（旧码 200 + 静默） |
| 集成 | `stop_then_concurrent_start_never_leaves_running`（`management_api.rs`） | 并发 Start/Stop 循环 20 次；每次收敛后 `!has_live_attempt && !desired_running`；若 Start 得到 409 则其 code ∈ {`STOP_IN_PROGRESS`,`SUPERSEDED_BY_STOP`} | 旧偶红 / 新恒绿 |
| 集成 | `stop_idempotent_when_never_running_still_200` | 从未启动时 `POST /stop` → 200、`was_running=false`、`desired_running=false`（**现有语义冻结**） | 两码皆绿（防回归） |
| 集成 | `stop_500_still_sets_desired_false` | 注入 drain 失败使 Stop 返回 500 `STOP_FAILED` 时，`desired_running` 仍为 false（`desired=false` 已在 `admit_stop` 写入，门不得把它改回 true） | 两码皆绿 |
| 集成 | `start_conflict_while_running_still_409` | 运行中再 Start → 409（现有文案语义不变） | 两码皆绿 |
| 集成 | `bootstrap_start_does_not_override_concurrent_stop` | 建档即启动与 Stop 并发时，最终 `desired_running=false` 且无 attempt | 旧红/新绿 |
| **现场证据** | `field_stop_clean_requires_continuous_10s`（**不是 cargo 测试**） | 10 s 内 ≥10 次外部采样全部满足：`state==STOPPED`、`desired_running==false`、驱动 pid 不存在、目标 TCP 会话已断；任一采样失败即窗口重置 | 现场放行依据；**不得**用 `await_settled` 的 500 ms 结果替代（§7.5） |

**"没破坏"的证明清单**（必须与修复同 PR 提交）：
- 正常 Start：`management_api.rs` 现有"段 4 手动启动"（`:2872-2932`）全绿——启动成功、RUNNING、点表、`desired_running=true`。
- 正常 Stop：现有"段 5 停止"（`:3026-3058`）全绿——200、`was_running=true`、STOPPED、`desired_running=false`，且 `assert_changed_within` 的"只允许 desired_running/updated_at_ns 变化"仍成立（门不新增持久化列）。
- Stop 幂等：`event_persistence.rs:668`（`stop_endpoint` 二次调用 `Ok(false)`）——第二次调用发生在第一次完成之后 ⇒ 条目已移除 ⇒ 仍 `Ok(false)`。
- Stop 有界：`stop_lifecycle.rs:109`（40 s）与 `reconnect_backoff.rs:465` 保持绿；`stop_lifecycle.rs:200-223`（栅栏期必须阻塞、放行后 `Ok(true)`）保持绿。
- A1 退避递增：`reconnect_backoff.rs` 全套（L1 `1,2,5,10,30,30`、L2 `1,2,5,10,30`、L4 `stop_cancels_escalated_backoff_and_stops_retrying`）全绿；**不新增/不修改** `endpoint.rs` 退避相关代码。
- L3 稳定运行后恢复：`recovery_after_real_batches_keeps_fast_reconnect`（`reconnect_backoff.rs:326`，定义见 `docs/design/p1-a1-saw-runtime-evidence-design.md:315`）保持"下一次 `retry_in_secs=1`"——门不参与 attempt 内部，期望零差异。
- 直调路径不回归：`subprocess_recovery.rs:186`（直接 `run_endpoint_with_heartbeat`）零改动编译通过、行为不变（旧签名保留）。
- 冻结码：`stop_endpoint` 的 `Err` 仍须以精确码开头（`stop_lifecycle.rs:41-49`），但**集合必须从 7 个扩到 8 个**（新增 `ATTEMPT_FAILED_UNKNOWN`，§6.8）。`STOP_INTENT_NOT_PERSISTED` / `START_INTENT_NOT_PERSISTED` / `INTENT_UNPERSISTED` 由 `core-api` 的门产生，不经过 `stop_endpoint` 的 `Err`，**不进**该集合。该扩写必须在 PR 描述里单独声明（这是本设计对既有冻结断言的唯一扩写）。
- 现场判据：`field_stop_clean_requires_continuous_10s`（§7.5）必须与 Core 侧证据**分开**提交；Core 的 500 ms 窗口通过不等于现场 10 s 通过。
- 证据口径：`python scripts/write-contract-evidence.py` 产出的 `target/validation/contract.json`（本文档不代跑）。

---

## 10. 明确不做

1. **不重构生命周期**：不引入"期望态 vs 运行态"对账器/看门狗（它本身会启停端点，对现场更危险）；不改 attempt 状态机、不改 teardown 预算。§7.3 的观察器**只读**，不写 `desired_running`、不 cancel attempt。§6.3.3 的 `attempt_reaper` **只清理已死 attempt 的残留**（kill 残留 pid、摘除属于该 gen 的 registry 键），**不启停端点、不写期望态、不改快照** ⇒ 它不是对账器，不属本项排除对象；但它的实现必须守住这条边界（代码注释 + review 门）。
2. **不改 wire/IPC**：`driver-protocol`、protobuf、驱动进程协议、REST 路由与字段一律不动；`EndpointStatus`（`snapshot.rs:83-95`）**不加字段**，代次只存在于 `Snapshot` 的内部 map 键结构里。对外行为变化仅限 §6.9 "回归风险"列出的四处（状态码/code 与 Stop 的失败可见性），**不新增路由、不新增响应字段**。
3. **不改 A1**：`endpoint.rs:44-50`、`:315`、`:416-418`、`:59-61` 一字不改；不调退避档位、不改归零判据。
4. **不新增持久化列**：`gen`/`last_completed_gen` 只存内存，避免 `endpoints` 表迁移与 `assert_changed_within` 类红线冲突；进程崩溃场景由"重启后按 DB 期望态恢复 + REST 尚未起来"覆盖。**注意**：§6.10 会修改 `UPDATE` 语句的列清单（去掉 `desired_running`），但**不改表结构、不加列、不迁移**。
5. **不修** `run_config_flow` 无取消点（Stop 后仍可能 `StartConnection`）——文档 §5.3-⑥ 已作为**观测判据**登记，属独立缺陷，另立设计；本设计不宣称消除它。
6. **不修** `DELETE /endpoints`、`PUT /tasks` 的 `is_running` 同族 TOCTOU（§4-③ 表末行）。注意：`is_running` 的**对外语义保持不变**（拆解期为假），本设计不"顺手"改它——改它会连带改变 DELETE/PUT 的行为，超出本竞态范围。但"旧代次写入复活已删端点"由代次门/墓碑覆盖（§6.5）。
7. **不删除** 828D 的 G7-3（无在途 Start）与文档 §5.3 的竞态观测：修复只是让竞态**显式失败**（409），现场仍按 `RACE_START_AFTER_STOP` 记录。
8. **不加自动强杀/自动恢复**：`child.wait()` 无界时保持 fail-closed（Start 一直 409）并大声告警，不引入"超时就当停了"的判定（§6.3.1）。**唯一例外**是 §6.3.3 的 `attempt_reaper`：它只对"任务已死、条目处于隔离态"的残留 pid 做强杀与回收，不改变任何活着的 attempt 的 teardown 预算。
9. **不做现场 10 s 判据的自动化**：Core 不实现"连续 10 s 观察"、不新增配置项、不写 `desired_running`（§7.5）。现场判据由证据脚本/监护流程执行，Core 只提供可观测面。
10. **不做"上次干净退出"持久标记**：§6.10.3 的残余风险不在本文消除（会引入新的持久状态，与 §10-4 冲突），见 §11-11。

---

## 11. 待确认

1. **"新的操作授权"的确切含义**（契约行 3）：本文按"重新 admit + 重新 `build_runtime_endpoint`（新 `stream_epoch`）+ 既有授权门（`CONTROL_DISABLED` 等）"实现，**不新增鉴权机制**。若评审指的是新增授权令牌/二次确认，属另一层设计，需明确。
2. **~~`desired_running` 写入时机前移~~（第二轮已冻结，不再讨论）**：前移是既定语义（§5.3），本文不再列为待确认。仅保留一条**实现层**注意：拆解期间 DB 显示"期望停止"而实际仍在 `STOPPING`，这被 §5.3 明确为**合法组合**，UI/告警不得据此报"矛盾状态"。
3. **~~并发 Stop 合流语义~~（第二轮已冻结，不再讨论）**：合流（不采用 409）是既定语义（§5.2），备选方案不再评估。
4. **门的归属变更**：原稿建议门放 `core-api` 且由门持有 `inflight_stops`。本轮改为"真值在 `driver-manager` 的 attempt 表，`core-api` 只做排序与票据"，因此**不需要**把门搬到 manager，也不需要 manager 反向依赖 core-api。若评审坚持"门整体下沉到 manager"，则需评估 `MesaManager` 公共 API 的 churn（`begin_stop` 之外还要暴露票据模型）。
5. **`manager` 新增同步 API**：`begin_stop`（同步）、`has_live_attempt`、`current_gen`（§6.11）、`gen_floor`（§6.5）、`await_settled` 的归属（manager vs gate）。本文把前四个放 manager、`await_settled` 放 gate。
6. **Core 内部稳定窗口参数**：`250 ms × 2 = 500 ms` 是否足够；是否需要按 endpoint/驱动类型可配（S7/FOCAS2 的会话关闭时延不同）。**注意：本条只讨论 Core 内部快速收敛检查；现场 10 s 判据（§7.5）不是可配参数，也不由 Core 执行，不在本条讨论范围。**
7. **全局 `LIVE_CHILDREN` 计数**：是否值得（它把临时 Descriptor 进程也算进去，需在判定时排除基线）。若不做，则"无孤儿"只剩结构不变量与端到端 pid 检查。
8. **W3 现场可达性**：本轮已由 insert-before-spawn **结构性**封死，不再依赖"是否真并发"的论证；但若评审仍希望保留"提交时 `is_running` 复核"作为第三道防线，需说明其与 attempt 表判定的优先级（本文认为重复，不采纳）。
9. **`EndpointMetaUpdate` 类型化改造的 churn**（§6.10.2 第 2 层保证）：是否值得把 `update_endpoint()` / `update_endpoint_with_secrets()` 的入参从 `&EndpointRecord` 换成不含 `desired_running` 的专用结构，使"用旧对象写回"在编译期不可表达。收益是类型级防复发，代价是 `config-store` 与 `core-api` 的调用点 churn（当前生产调用点 2 处 + 单测若干）。**原第 9 条（写失败是否升级为告警/返回）已由 §6.10.3 定案，不再待确认。**
10. **`latest` / `point_meta` 的迟到写入是否需要墓碑过滤**（§6.5）：墓碑门目前只覆盖 `endpoints` 表（决定"端点是否复活"）。已删除端点的迟到 `register_points`/`apply_batch` 仍可能写入 `latest` 行，影响 `/api/v1/points/latest` 的"幽灵行"。本文**未**把该路径纳入（不在 P0-4 的判据内），是否同 PR 处理待定——**待确认**。
11. **"上次进程是否干净退出"的持久标记**（§6.10.3 残余风险）：要彻底消除"意图落盘失败 + 崩溃 ⇒ 重启自动拉起"，需要在启动时判断上次是否干净退出。这会引入新的持久状态（文件或列），与 §10-4 冲突，**属独立设计，本文不做**。
12. **`attempt_reaper` 的归属与触发方式**（§6.3.3）：本文放在 `MesaManager` 内、由 `quarantine_tx` 事件驱动（无定时轮询）。若评审认为常驻任务不可接受，备选是"由下一次 Stop 顺带触发复核"——但那会让"无人再 Stop 时端点永久不可 Start"，需权衡。
