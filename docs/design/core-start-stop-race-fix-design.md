# Core Start/Stop 生命周期竞态修复设计（G7）

- **版本**：**v3**（v1 = `c3d8918` 初稿；v2 = `eb0a0c8` + `f65a71c` 冻结契约稿/PR #132 Draft；**v3 = 已对齐 main `9611255de75962fcf61874a84befcd59651cb608`（G7 实现已合并，PR #133）的实际代码**）
- **状态**：设计**已与实现一致**，不再含"未实施"的表述。G7 实现过程中新增/纠正了五块内容，v3 已全部补齐：
  1. **P0-D：删除与在途运行意图写入的竞态**（初稿完全没有）→ §6.12；
  2. **P0-E-1：删除屏障期 Stop 必须照常落盘**（初稿/早期实现写错）→ §6.13；
  3. **P0-E-2：`DeleteGuard` RAII（未提交即撤销屏障）**（初稿没有）→ §6.14；
  4. **828D 一期开机不自动恢复**（初稿把"重启误拉起"列为**不消除**的残余风险，现由持久化围栏 + 正向凭证 + 一期产品约束消除，P0-C）→ §6.10.3、§6.15；
  5. **编号口径重排**（实现侧 `P0-A/B/C/D/E` 与原稿的编号**不是同一组含义**）→ §0.2 对照表。
- **基线**：`main = 9611255de75962fcf61874a84befcd59651cb608`。**本文所有 `文件:行号` 均按该提交逐一核对**；`crates/`、`apps/`、`tests/` 与本文同源，无待合并差异。
- **上游依据**：`docs/validation/828d-phase1-first-limited-sampling-application.md` 的 文档 §4.7（G7-1/2/3）、文档 §5.3（竞态机制、Stop 时间约束、六项判据）；828D 一期产品约束见 `docs/design/nck-828d-phase1-boot-recovery-policy.md`（独立文档，本文只引用、不复述）。
- **修订依据**：两轮独立复审（第一轮冻结 G7-2 契约 §5.1 与 P0-A~E / P1-F；第二轮冻结并发 Stop 合流与 `desired_running` 前移 §5.2/§5.3）+ **G7 实现过程中追加并落地的 S1/S2/R-1/R-2/P0-7 与 P0-D/P0-E-1/P0-E-2**（§5.5、§6.12~§6.14）。
- **约束**：不改 wire/IPC；不改 A1 退避语义；不重构生命周期；**v3 不改任何代码/测试/配置**——只把文档对齐已合并的实现；历史约束"不启动服务/设备、不跑测试与构建"对本文仍然成立。
- **引用约定**：`§x.y` 指**本文档**章节；指向上游 828D 文档时一律写作 `文档 §x.y`（即 `docs/validation/828d-phase1-first-limited-sampling-application.md`）。本文的 §5.2/§5.3 与上游 §5.3 **不是同一节**，请勿混读。**`P0-x` 编号一律以 §0.2 对照表为准。** 行号也有两条口径：描述"修复前 / 原稿 / 初稿 / 旧代码"时用 `fda7633` 的行号，描述"现状 / 已落地"时用 `9611255` 的行号；每处上下文都会标明属于哪一种。

---

## 0. 结论摘要

1. （**修复前**，`fda7633`）竞态**真实存在**，且不止一处：Stop 的幂等分支（`crates/core-api/src/lib.rs:2169-2175`）与 Start 成功路径（`lib.rs:2153-2156`）之间没有任何"意图排序"保护，两个 handler 并发执行时**后提交者胜**。
2. （**修复前**，`fda7633`）最关键、也最容易被现场触发的窗口不是文档 §5.3 描述的那一个，而是 **Stop 拆解期间**：`MesaManager::stop_endpoint` 在 await 之前就把运行条目从 map 中移除（`crates/driver-manager/src/manager.rs:535`），因此整个拆解期（典型 ≈35 s，文档实测 75–80 s，尾部无界）内 `is_running()` 恒为假（`manager.rs:459-466`），任何此刻到达的 Start 都能通过检查并**并行拉起第二个 attempt**。
3. 修复方向不变：**Stop 生成号 + Stop 在途保护 + Start 两阶段提交（admit/commit）**。但本轮修订纠正了两个根本性错误：
   - **Stop 在途保护必须归属于"实际生命周期操作"（attempt 任务本身），不能归属于 HTTP 请求**。原稿 §5 让 handler 用 RAII guard 减计数，一旦客户端断开/超时导致 handler 被取消、而 teardown 仍在后台跑，guard 会提前释放保护、**重新开放 W2**（§6.3）。
   - **`is_running()` 不能作为"停干净"的完成判据**：它在拆解开始前就变假（`manager.rs:535`），且 W3 孤儿根本不在运行表里（`manager.rs:513-516`）。判据必须换成"代次化的 attempt 表 + 代次化快照"（§1.2、§6.5、§7）。
4. 变化仅限 Core 内部：`driver-manager` 新增代次化 attempt 表与快照代次门（`snapshot.rs`/`endpoint.rs`/`manager.rs`/`process.rs`），`core-api` 新增薄门 `LifecycleGate`，`config-store` 收缩 `desired_running` 的写路径（§6.10，**不改表结构、不加列**）；**不碰 wire/IPC、不改 REST 字段**。对外的行为变化共**六处**：竞态分支下 Start 的状态码 200 → 409（带可机器识别的 code）；并发 Stop 由"立即 200 `was_running=false`"改为**合流并返回同一次结果**（§5.2）；`desired_running=false` 由"teardown 之后"**前移到 Stop 受理时**（§5.3）；**Stop 意图落盘失败由静默 200 改为显式 500**（§6.10.3）；**Stop 意图落盘未确认期间新 Start 一律 409 `STOP_PERSIST_IN_PROGRESS`**（P0-A，§6.12.1）；**DELETE 在"仍有运行意图在落盘"时返回 409 `DELETE_PERSIST_IN_PROGRESS` 而不是立即删行**（P0-D，§6.12）。
5. 第二轮复审补齐的四项**不是可选优化，而是前述方案的成立条件**（详见对应小节；下述"现状"均指**修复前** `fda7633`，现在的代码见 §1.1.1）：
   - **P0-1**：`AttemptGuard::Drop` 在 panic 展开时同样执行 ⇒ 若照原稿无条件删条目，新 Start 会在 teardown 未证明完成时被放行，**与 fail-closed 直接矛盾**。必须区分"正常完成"与"异常终止"，后者进入 `FAILED_UNKNOWN` 隔离态并禁止新 Start（§6.3.2、§6.3.3）。
   - **P0-2**：`desired_running` 还有两条**不经过生命周期门**的写路径（`update_endpoint()` / `update_endpoint_with_secrets()` 的 UPDATE 都带该列，`config-store/src/lib.rs:1091-1094`、`:1165-1168`），且 `core-api` 的 PUT 是"先读 → 长 await 校验 → 把旧对象写回"（`lib.rs:1979`、`:1998`、`:2096-2099`）⇒ **"门是唯一写者"在现有代码中不成立**。必须做写路径拆分（§6.10）。
   - **P0-3**：完成代次不能随 attempt 条目一起消失，否则 `await_settled()` 无代次可比，**正常停止反而永远无法满足判据**。必须保留 `last_completed_gen`（§6.11、§7.4）。
   - **P0-4**：墓碑检查与快照写入必须**同一线性化临界区**，否则"检查完墓碑、尚未落盘时 DELETE 刚好执行"仍可复活已删端点（§6.5）。
   - **P1-5**：Core 内部 500 ms 快速收敛检查**不等于**现场 10 s 连续稳定观察，前者不能替代后者（§7.5）。
6. **v3 追加（实现过程中新增，初稿没有）**：**删除也是一条会与"运行意图落盘"竞争的写路径**。Stop-A 的 teardown 已完成、`desired=false` 仍在写库时，DELETE 只看 attempt 表就会"看到没有活跃 attempt"⇒ 删行 + 清空在途标记 ⇒ 同 ID 重建 + Start 写 `true` ⇒ Stop-A 的迟到写入把新实例污染成 `desired=false` 而实际在跑。修法不是"清标记"，而是**屏障 + 有限等待 + 条件复位 + 删行前串行器复查**（P0-D，§6.12）。
7. **删除屏障期 Stop 必须照常落盘**：早期实现"屏障期跳过落盘但仍返回 200"，一旦 DELETE 随后失败（409/404/5xx），端点仍在而 DB `desired_running` 可能仍是 `true` ⇒ **Stop 虚假成功**。停止是安全方向，**不得把 Stop 变成可失败的 409**；200 必须等价于「意图已落盘 **或** 行已不存在」（P0-E-1，§6.13）。
8. **屏障必须由 RAII 拥有**：`begin_delete` 返回 `DeleteGuard`，未 `commit()` 即撤销（Drop 覆盖提前 return / panic 展开 / future 被丢弃），且 Drop 内用 `let Ok(..) else return` 避免 panic 期间二次 panic。否则一次取消就把端点永久锁进 409 `ENDPOINT_DELETED`（P0-E-2，§6.14）。
9. **828D 一期：NCK 端点开机一律不自动恢复**（`NCK_DRIVER_ID` + `boot_auto_restore_allowed()`，精确匹配 `driver_id="sinumerik-nck"`）。这是**一期产品约束，不是通用生命周期语义**；其他驱动行为不变（P0-C，§6.15）。

### 0.1 v3 的五处新增/纠正（一句话版）

| # | 主题 | 初稿状态 | 现在（`9611255`） |
|---|---|---|---|
| 1 | P0-D 删除与在途意图写入竞态 | **没有** | 屏障 + 有限等待 + 条件复位 + 串行器复查（§6.12） |
| 2 | P0-E-1 屏障期 Stop 的落盘 | **写错**（屏障期跳过落盘仍 200） | 与正常路径同源、照常落盘（§6.13） |
| 3 | P0-E-2 `DeleteGuard` RAII | **没有**（靠调用方显式 `abort_delete`） | guard 未提交即撤销，覆盖取消/panic（§6.14） |
| 4 | 828D 重启恢复残余风险 | 列为**不消除** | **已消除**（S2 围栏 + 正向凭证 + P0-C 产品约束，§6.10.3、§6.15） |
| 5 | 编号口径 | 原稿 `P0-A~E` / `P0-1~4` 混用 | 实现侧重排，见 §0.2 |

### 0.2 编号口径对照（**实现侧为准**）

代码注释里出现的编号是 G7 落地时重新排定的，与本文件 v2 的编号**不是同一组含义**。查代码证据时一律按右列。

| 编号 | 实现侧含义 | 主要代码落点 | 本文对照 |
|---|---|---|---|
| `P0-A` | **跟踪全部**在途 Stop 落盘代次（集合而非"最新代次"），集合非空 ⇒ 禁止新 Start | `crates/core-api/src/lifecycle.rs:131-138`（字段）、`:378-408`（闸门）、`:1064-1072`（`STOP_PERSIST_IN_PROGRESS`） | §6.12.1 |
| `P0-B` | **拆锁**：Start 的 `desired=true` 落盘不得阻塞 Stop 的 `begin_stop`；落盘串行器在门锁之外 await | `lifecycle.rs:141-152`、`:351-453`；测试 `stop_lifecycle.rs:1886` | §6.4、§5.5 |
| `P0-C` | **828D 一期产品约束**：`sinumerik-nck` 开机一律不自动恢复 | `crates/core-api/src/recovery.rs:29-37`；`apps/mesad/src/main.rs:184-198` | §6.15 |
| `P0-D` | **删除屏障**：删除必须等在途意图写入收敛，且删行前复查串行器 | `lifecycle.rs:49-58`、`:579-731`；`lib.rs:2139-2208` | §6.12 |
| `P0-E-1` | **屏障期 Stop 照常落盘**（屏障只挡 Start） | `lifecycle.rs:737-767`（`admit_stop`） | §6.13 |
| `P0-E-2` | **`DeleteGuard` RAII**：未提交即撤销屏障 | `lifecycle.rs:221-263` | §6.14 |
| `P0-1` | attempt **异常终止隔离态**（`FailedUnknown` 保留条目、禁止新 Start） | `manager.rs:494-563`、`:901-978` | §6.3.2、§6.3.3 |
| `P0-2` | `desired_running` 的**写路径拆分**（两条 UPDATE 去掉该列） | `config-store/src/lib.rs:1517`、`:1597-1603` | §6.10 |
| `P0-3` | **完成代次凭证** `last_completed_gen`（与移除条目同一临界区） | `manager.rs:482-492`、`:533-537`；`lifecycle.rs:125-130` | §6.11 |
| `P0-4` | **代次门 + 单锁墓碑**（快照写入按代次过滤、DELETE 立墓碑）＋ 正向凭证默认不恢复 | `snapshot.rs:153-175`、`:238-260`、`:495-525`；`config-store` 凭证 `:708`、`recovery_plan` `:739-767` | §6.5 |
| `P0-7` | Start/DELETE 竞态：`set_desired_running` 返回 `Ok(false)` ⇒ 认领停止，不留孤儿 | `lifecycle.rs:541-557`；测试 `stop_lifecycle.rs:2234`、`:2273` | §6.4 |
| `R-1` | Bootstrap"已启动却删配置"：启动失败分类，先停再补偿 | `bootstrap.rs:104-133`、`:168-194`；测试 `stop_lifecycle.rs:2100` | §6.10.2 |
| `R-2` | 收敛判据必须包含"**本次** Stop 落盘已确认"（慢写下不得提前判成功） | `lifecycle.rs:125-130`、`:983-1046`；测试 `stop_lifecycle.rs:2013` | §7.3、§7.4 |
| `S1` | **停止优先**：`admit_stop` 先 `begin_stop` 再落盘，DB busy 不得推迟停止 | `lifecycle.rs:737-743`、`:781-783`；`lib.rs:2252-2262` | §5.5 |
| `S2` | 落盘失败写**持久化围栏**（DB meta + sidecar 双通道），崩溃重启也不得自动拉起 | `lifecycle.rs:881-906`；`config-store/src/lib.rs:1694-1795` | §5.5、§6.10.3 |

---

## 1. 事实基线（历史基线 = `fda7633`；G7 合并后的现状见 §1.1.1）

### 1.1 代码事实（**G7 之前的基线**，`fda7633`）

> **阅读须知**：本表记录的是竞态**成因**的历史事实基线（G7 实现之前）。G7 合并后哪些行不再成立、现在的代码是什么，见 §1.1.1（同样是逐条 `文件:行号`，按 `9611255` 核对）。表中的行号指向 `fda7633`，**不得**当作当前代码的证据。

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
| F13 | 无任何"期望态 vs 运行态"对账/看门狗；`desired_running` 只在开机恢复与 API 输出被读 | `apps/mesad/src/main.rs:140-161`、`lib.rs:1508`、`:1576` |
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

### 1.1.1 已被 G7 取代的事实（按 `9611255` 逐条核对）

| 原事实 | 现状（`9611255`） | 证据 |
|---|---|---|
| F1/F2/F3：三个 handler 直接 `is_running` 判态并各自写 `desired_running` | **handler 不再写 `desired`、不再查 `is_running`**：`start_endpoint` = `admit_start` → `build_runtime_endpoint`（长 await）→ `commit_start`；`stop_endpoint` = `admit_stop` → `join!(handle, persist)` | `crates/core-api/src/lib.rs:2223-2241`、`:2256-2300` |
| F5/F6：`stop_endpoint` 先 `remove` 条目再 await，拆解期 `is_running` 恒假 | **条目不再由 Stop 移除**：`begin_stop` 只置 `Stopping` + cancel；条目由 attempt 任务自持的 `AttemptGuard::drop` 在 `committed==true` 时移除。`is_running` 退化为兼容投影（只算 `Starting/Running`） | `crates/driver-manager/src/manager.rs:996-1012`、`:521-563`、`:399-402`、`:775-784` |
| F10：`upsert_endpoint` 最后写入者胜 | **代次门**：`upsert_endpoint(generation, status)`，旧代次与"≤ 墓碑"的写入一律丢弃 | `crates/driver-manager/src/snapshot.rs:238-260` |
| F21：`remove_endpoint` 无墓碑 | **单锁墓碑**：`remove_endpoint(id, gen_floor)` 抬高 `retired` 后再删，检查与写入同临界区 | `snapshot.rs:495-525` |
| F25/F26：两条 UPDATE 都写 `desired_running` | **两条 UPDATE 已去掉该列**（函数注释明写"本函数永不写运行意图"）；唯一写点是 `set_desired_running`，且成功后记正向凭证 | `crates/config-store/src/lib.rs:1517`、`:1597-1603`、`:1617-1670` |
| F28/F29：bootstrap 建档即写 `true`、启动成功后再写真 | **建档一律写 `false`**；`start=true` 的意图改由 `commit_start` 在启动成功后写入，失败按 R-1"先停再补偿"分类 | `crates/core-api/src/bootstrap.rs:150-166`、`:360-372`、`:104-133` |
| F13：无对账/看门狗，`desired_running` 只在开机恢复与 API 输出被读 | **开机恢复改为"能证明才恢复"**：`recovery_plan()` 需 DB 可写 + 围栏可读 + 正向凭证命中；NCK 另有产品约束 | `apps/mesad/src/main.rs:140-208`；`config-store/src/lib.rs:739-767`；`core-api/src/recovery.rs:29-37` |
| F15：`desired_running` 落盘是单条 UPDATE | 仍是单条 UPDATE，列清单为 `desired_running, updated_at_ns`；额外写正向凭证（`record_intent_proof`，`n>0` 才写） | `config-store/src/lib.rs:1655-1669`、`:708` |
| F31：`Snapshot` 6 把独立锁，墓碑需另起一把 | **`endpoints` 一把锁内即含墓碑**：`EndpointTable { live, retired }` | `snapshot.rs:165-182` |

**仍然成立的部分**（未被 G7 改动，可继续引用）：F8（配置构造是长 `await`，`runtime.rs:165-199`）、F9（退避档位语义）、F11/F12（会话表以 `endpoint_id` 为键；RUNNING 只在进入时写一次）、F16~F20（teardown 的完成边界与三重进程兜底）、F22/F23（测试夹具可绕过 Manager 直调 `run_endpoint_with_heartbeat`）、F24（`EndpointStatus` 是 REST 响应元素，未加字段）、F32（早退路径同样 `wait`）。**待确认**：F8/F9/F11/F12/F16~F20/F22/F23/F24/F32 的行号未逐行重核（G7 未触碰这些函数体，但 `manager.rs`/`endpoint.rs` 因新增结构发生位移）。

### 1.2 为什么 `is_running()` 不能作为"停干净"的完成判据（设计原编号 P0-B；实现侧 `P0-B` 指"拆锁"，见 §0.2）

复审指出的三点在代码中全部成立，逐条给出证据：

1. **`false` 出现在 teardown 之前。** `stop_endpoint` 的第一条语句就是 `remove`（`manager.rs:535`），而 await 在 `:540`。remove 之后、await 返回之前的整段（35 s 典型 / 75–80 s 实测 / 尾部无界）里，`is_running()` 必然返回 `false`（`manager.rs:459-466`：map 无条目 ⇒ `false`）。也就是说 `is_running()==false` 有两种互斥含义——"没有 attempt" 与 "attempt 正在拆解"——**不可区分**。
2. **W3 的孤儿根本不在运行表里。** 覆盖写（`manager.rs:513-516`）丢弃前一个 `JoinHandle` 与 cancel token，孤儿 attempt 从此不出现在 `running` 表中：`is_running()` 返回 `false`，`running_ids()` 也不列出它（`manager.rs:468-476`）。用运行表判断"无未完成 attempt"会**漏掉孤儿**。
3. **快照写入无代次保护。** `upsert_endpoint` 是最后写入者胜（`snapshot.rs:208-213`）：老 attempt 拆解时写的 STOPPED（`endpoint.rs:357-368`）可以覆盖新 attempt 的 RUNNING（`endpoint.rs:597-605`）。因此即使 `is_running()` 说"停了"，快照也可能说"停"，而实际仍在采集——**判据与被判据对象共享同一个不可信来源**。

**结论（写进设计纪律）**：`is_running()==false` 是"停干净"的**必要不充分条件**，只能作为 UI/兼容投影，**禁止**出现在任何完成判据、门控判定或回归断言的核心断言里。可信证据必须来自 §6 的**代次化 attempt 表**与**代次化快照**，判定方式见 §7。

### 1.3 Handler 阶段模型（下文用 S/T 引用）

**修复前（`fda7633`）**：

- Start：`S0` 准入（`is_running` 检查）→ `S1` 构造（长 `await`）→ `S2` 提交（start + 置真）→ `S3` 应答
- Stop：`T0` 准入（`is_running` 检查）→ `T1a` 幂等置假 / `T1b` 拆解 `await` → `T2` 置假 → `T3` 应答

**现在（`9611255`，与实际代码一一对应）**：

| 阶段 | Start（`lib.rs:2223-2241` → `lifecycle.rs:364-453`） | Stop（`lib.rs:2256-2300` → `lifecycle.rs:744-816`） |
|---|---|---|
| `S0`/`T0` | `admit_start`：同步登记票据（**不查运行态、不碰 DB、不起进程**） | `admit_stop`：同步 `stop_gen+1` → 内存置假 → 集合登记本次代次 → `begin_stop`（拆解**立即**开始）→ 派发落盘任务 |
| `S1` | `build_runtime_endpoint`（长 `await`：Descriptor/Secret/任务门禁） | ——（落盘任务在门外 await 串行器，不阻塞 `T0`，P0-B） |
| `S2` | `commit_start`：门外取串行器 → 同步临界区（**前置判定 + `start_endpoint`**）→ 临界区外 `spawn_blocking` 写 `true` 并 `await` 结果 | `T1` `join!(handle.await_result(), persist)`：合流等 teardown 结论 + 等自己的落盘结论 |
| `S3` | 应答：落盘成功 ⇒ 200；`START_INTENT_NOT_PERSISTED`/`ENDPOINT_DELETED` 等 ⇒ 显式错误 | 应答：落盘失败 ⇒ **500 `STOP_INTENT_NOT_PERSISTED`（优先于 teardown 结果）**；否则 `Ok(was)` ⇒ 200、`Err` ⇒ 500 `STOP_FAILED` |

**关键差别（v3 必须强调）**：`S2`/`T1` 里各有一段 `await`，但都**不改变门状态**（等串行器、等落盘结果）；状态迁移全部在同步临界区内完成，因此取消只丢应答（§6.7）。`T0` 的落盘**与 teardown 并行**（S1），不是"先落盘再停"。

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

### W4（v3 追加）：删除落在"Stop 已停完、意图仍在落盘"的窗口里

前三个窗口都是 Start/Stop 之间的排序问题；**第四个窗口的写者是 DELETE**，它与"运行意图落盘"竞争：

```text
Stop-A : [T0 受理]--[teardown 完成 → 条目移除]--[desired=false 仍在写库 ...]--[200]
DELETE :                        [看不到活跃 attempt ⇒ 删行 + 清在途标记]
CREATE :                                    [同 ID 重建] [Start 写 desired=true]
Stop-A :                                                              [迟到的 false 落到新记录]  ← 污染
```

- **为什么它不是 W1/W2 的重复**：这里的"迟到写"来自一个**已经正常结束**的生命周期操作，attempt 表（生命周期真值）此时**正确地**说"没有在途 attempt"。判据没错，错在把"门禁记录已清空"当成"后台写库已完成"。
- **为什么墓碑代次不够**：窗口内新受理的 Stop 会拿到**更大的**新代次，墓碑挡不住它（§6.12.1）。
- 关闭方式：屏障 + 有限等待 + 条件复位 + 删行前串行器复查（§6.12）；以及屏障期 Stop 照常落盘所要求的**再复查**（§6.13.3）。
- 已落地用例：`delete_waits_for_inflight_stop_persist_and_rebuild_is_not_polluted`（`stop_lifecycle.rs:2335`）、`barrier_stop_confirms_intent_before_returning_success_even_if_delete_fails`（`:2588`）、`cancelled_delete_request_does_not_leave_a_permanent_barrier`（`:2715`）。

**三个窗口的修复状态（v3 小结）**：W1 由票据代次 + `SUPERSEDED_BY_STOP` 关闭；W2 由"条目存在即保护" + `STOP_IN_PROGRESS` 关闭（`begin_stop` 不再移除条目，`manager.rs:996-1012`）；W3 由 insert-before-spawn（`Entry::Vacant`）结构性关闭；W4 由删除屏障关闭。

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

> **v3 说明**：本节记录的是**修复前**（`fda7633`）的影响面推演，用于解释为何必须修。G7 已关闭 W1~W4（见 §2 小结与 §6.3~§6.14），下述后果**不再随时可达**；仍留存的相邻缺陷在每条末尾标注（§10 汇总）。

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
| `DELETE /endpoints/{id}`、`PUT /tasks`、`POST /tasks` 的 `is_running` 门 | 非覆盖，但同族 TOCTOU：拆解期 `is_running` 为假，允许删端点/改任务，而旧 attempt 仍在用旧任务跑；且 `remove_endpoint` 无墓碑（F21），旧 attempt 随后的状态写入会**复活已删除端点** | 修复前：`lib.rs:2113`、`:2125`、`:2240`、`:2297`；`snapshot.rs:440` | **DELETE 侧已修**（G7）：判据换成 `has_live_attempt`（`lib.rs:2130`）+ 删除屏障与在途写入等待（P0-D，§6.12）+ 代次门/单锁墓碑（§6.5）。`PUT /tasks`、`POST /tasks` 仍用 `is_running`（`lib.rs:2349`、`:2406`、`:2432`），**未修**（§10-6） |

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

**"新的操作授权"的实现解释（需 owner 确认，见 §11-1）**：不复用任何 Stop 之前的票据或配置快照——新 Start 必须（a）重新 `admit_start` 取得当前代次的票据，（b）重新执行 `build_runtime_endpoint`（`lib.rs:2233`，含 Secret 物化与任务门禁），（c）继续受既有授权门约束（`CONTROL_DISABLED` 等不变）。**不是**新增鉴权机制。

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

1. 写入点是 `admit_stop`（§6.4）：**同步临界区内置内存意图**（`lifecycle.rs:770-777`），落盘在独立任务里与 teardown 并行（`lifecycle.rs:793-810`）；**不是** handler 的 `await` 之后（修复前在 `fda7633` 的 `lib.rs:2171`、`:2180`，取消即丢）。
2. "合法组合"要求**判据侧不得把 `desired=false` + `state!=STOPPED` 当作矛盾**：§7.1 判据 2 与判据 1 是**独立**的，`desired=false` 不蕴含 `state==STOPPED`，反之亦然。
3. "写入失败不得静默"的具体行为（**显式失败 + 安全方向 + 禁止不受控重启**）见 §6.10.3。注意它**不是**"Stop 返回 500 就完事"：teardown 仍必须执行，且必须防止"DB 说 true、内存说 false"在重启后变成自动拉起。
4. "后续 Start 只在旧 attempt 确认结束后" = §6.3.1 的 `TeardownDone(gen)`，**不是** `is_running()==false`（§1.2）。

### 5.4 三者的交叉约束（本节唯一新增推理）

| 约束 | 为什么必须写下来 |
|---|---|
| **合流 ≠ Stop 永远成功** | 否则实现者会把"有人正在停"当成"必定成功"，从而在 `outcome=Err` 或 `phase=FailedUnknown` 时返回 200——这正是 §5.2 第四行与 P0-1 要禁止的"伪装成功" |
| **前移 ≠ 意图已持久化** | 前移只是把写入点搬到同步临界区；写失败仍可能发生。**"已前移"与"已落盘"是两件事**，后者失败必须显式（§6.10.3） |
| **合流 + 前移 ≠ 可以放宽 Start 门** | `desired=false` 是意图，不是"旧 attempt 已结束"的证明；Start 门只看 attempt 表 + 隔离态（§6.4 步骤 2），不看 `desired` |

### 5.5 实现过程中追加并落地的语义（S1 / S2 / R-1 / R-2）

§5.1~§5.3 是评审冻结的**需求**语义；下面四条是 G7 实现时为了让冻结语义在"DB 慢/DB 挂/进程崩"下仍然成立而追加的**实现级契约**，代码已按此落地，本文后续章节与测试清单都以此为基准。

| 编号 | 语义（一句话） | 代码证据 |
|---|---|---|
| **S1** | **停止优先于持久化**：`admit_stop` 先 `begin_stop`（拆解立即开始），再把 `desired=false` 的写库放到独立任务里跑 ⇒ DB busy 不推迟机床停下来 | `lifecycle.rs:737-743`（顺序不可交换的注释）、`:781-783`（同步 `begin_stop`）、`:793-810`（并行落盘任务）；`lib.rs:2252-2262` |
| **S2** | **落盘失败必须留下持久事实**：写"停止意图未持久化"围栏，**双通道**（DB `meta` 表 + DB 同目录 `<db>.stop-fence.json` sidecar），任一通道成功即可跨崩溃；显式 `POST /start` 成功即视为人工确认并清围栏 | `lifecycle.rs:881-906`；`config-store/src/lib.rs:1694-1742`（mark）、`:1748-1779`（clear）、`:1786-1795`（读，两通道并集）、`:342`/`:349`（键名与 sidecar 后缀）；冻结响应体文案 `lifecycle.rs:46-47` |
| **R-1** | **Bootstrap"启动已发生但意图未落盘"不是普通失败**：attempt 已在跑，必须先停再补偿删除，且只有"确认条目已移除"才算停住 | `bootstrap.rs:104-133`（`StartFailure` 两分类）、`:150-166`（走门）、`:168-194`（先停再补偿，30 s 有界） |
| **R-2** | **收敛判据必须包含"本次 Stop 的落盘已确认"**：只看内存意图会在慢写下提前判成功（DB 仍是 `true`） | `lifecycle.rs:125-130`（`stop_persist_confirmed_gen` 字段及其理由）、`:983-1046`（`await_settled` 的 `intent_confirmed` 判定） |

**S2 与 P0-C 的关系**：S2 解决"**能不能证明**这是用户意图"（负向围栏 + 正向凭证），P0-C 解决"**即使能证明**，该驱动也不许在开机时自动拉起"。两者是不同层，缺一不可（§6.15）。

---

## 6. 方案 A：Stop 生成号 + Stop 在途保护 + Start 两阶段提交

### 6.1 唯一真值与三层职责

本轮修订最重要的结构决定：**"有没有 attempt、attempt 到哪一步了"只有一个真值来源——`driver-manager` 的 attempt 表；`core-api` 的 `LifecycleGate` 只是排序器与票据池，不持有任何生命周期真值。**

| 层 | 组件 | 持有什么 | 不持有什么 |
|---|---|---|---|
| `driver-manager` | `AttemptTable`（替换现 `running`，`manager.rs:360`） | `live` 条目（attempt 是否存在、代次、phase、pid、teardown 证据、结果）+ `last_completed_gen`（完成代次凭证，§6.11）+ 隔离态（§6.3.2） | 不碰 DB、不碰 HTTP、不知道 Stop 票据 |
| `driver-manager` | `Snapshot`（`snapshot.rs`） | `(gen, EndpointStatus)` 原子对 + 墓碑，写入按代次过滤（**单锁**，§6.5） | 不判断"是否停干净" |
| `core-api` | `LifecycleGate`（`crates/core-api/src/lifecycle.rs:319-1061`） | 每 endpoint 的 `stop_gen`、在途 **Start** 票据数 `inflight_starts`、`desired` 缓存 + `desired_seq`、`stop_persist_confirmed_gen`、**全部在途 Stop 落盘代次集合** `stop_persist_inflight`、**在途 Start 落盘计数** `start_persist_inflight`、`deleting` 屏障、`intent_unpersisted`、per-endpoint `persist_lock` 串行器 | **不持有** Stop 在途**生命周期**计数（原稿的 `inflight_stops` 已删）；"有没有 attempt、到哪一步"仍只由 `driver-manager` 回答 |

**为什么删掉 `inflight_stops`**：计数器的"减一"必须发生在"teardown 真正完成"这个事件上，而该事件只发生在 attempt 任务内部。若计数器在 core-api，就必然要么由 handler 减（取消即丢，P0-A 的原始漏洞），要么由 manager 反向回调 core-api（层次倒置）。**把保护直接表达为"attempt 表条目存在"，减一就自动等价于"条目被 teardown 完成者移除"**——不需要任何跨层通知，也不存在"忘了减"的可能。**且"移除"只发生在正常完成路径**（§6.3.2）：异常终止**不**移除、转隔离态 ⇒ 保护不会因为 panic 而消失，这是 P0-1 对"保护载体 = 条目"这一决定的补强。

**但门仍然要持有另一种"在途"（P0-A / P0-D，实现侧新增）**：**运行意图的写库在途**。它与上面那个"生命周期在途"是**两件事**：

| | 生命周期在途 | 意图落盘在途 |
|---|---|---|
| 问题 | attempt 是否还在跑/拆解 | `desired_running` 的 SQL 是否已确认 |
| 载体 | attempt 表条目（`driver-manager`） | 门里的两个计数器/集合（`stop_persist_inflight` 集合、`start_persist_inflight` 计数） |
| 何时结束 | teardown 完成并由任务自身移除条目 | `spawn_blocking` 里的 UPDATE 返回（成功/失败都算结束） |
| 谁需要它 | Start 门（契约行 2）与停止判据 | **Start 门（P0-A）与 DELETE（P0-D）** |

混淆两者会同时踩两个坑：把"集合清空"当"任务完成"（P0-D 的原始缺陷），或把"条目存在"当"写入已确认"（P0-A 的反序污染）。

### 6.2 数据结构

```rust
// driver-manager（manager.rs:409-492）—— 一次 attempt 的全部可判定证据（无锁读，供观察与诊断）
pub struct AttemptCtx {
    pub generation: u64,              // 单调代次，per endpoint 分配，永不回收（墓碑以下界，§6.5）
    phase: AtomicU8,                  // Starting → Running → Stopping → TeardownDone | FailedUnknown
    pub pid: AtomicU32,               // 驱动子进程 pid（spawn 后回填；0 = 尚未 spawn）
    pub session_released: AtomicBool, // teardown 末尾：registry 已摘除本 attempt 的 session（E4）
    pub remote_eof: AtomicBool,       // teardown 末尾：reader 观测到对端 EOF
    pub outcome: OnceLock<Result<bool, String>>,  // teardown 结论（OnceLock ⇒ 一次 attempt 只有一个结论）
    committed: AtomicBool,            // **唯一正常完成标记**：只有 AttemptGuard::commit 能置 true（§6.3.2）
    done_tx: tokio::sync::watch::Sender<Option<Result<bool, String>>>,  // 完成通知（合流点）
    #[cfg(feature = "test-hooks")]
    pub test_panic_before_teardown: AtomicBool,     // 只在 test-hooks 构建存在
}

struct AttemptEntry {
    ctx: Arc<AttemptCtx>,
    cancel: CancellationToken,
    // 注意：这里**没有** JoinHandle（见 §6.6）：条目存在 ⇔ 生命周期操作未被证明结束
}

/// attempt 表：`live` + `last_completed_gen` **同锁**（P0-3 的原子性来源）
struct AttemptTable {
    live: HashMap<String, AttemptEntry>,
    /// 只由 commit 路径（与移除条目同一临界区）与 reaper 复核通过后写入；永不回退、永不清理
    last_completed_gen: HashMap<String, u64>,
    // 隔离态**不另设 map**：它就是 live 里 phase == FailedUnknown 的条目（避免两处真值）
}
```

> **字段命名**：v2 用 `gen`，实现里一律是 `generation`（`AttemptCtx.generation`、`AttemptGuard.generation`、`gen_floor`）。本文正文用 `gen` 作概念名，读代码时按 `generation` 理解。

```rust
// core-api（lifecycle.rs:97-274）—— 只做排序、票据与"意图落盘在途"
struct EpGate {
    stop_gen: u64,                             // Stop 受理即 +1（W1 的唯一判据；DELETE 完成再 +1 立墓碑）
    inflight_starts: u32,                      // 已 admit、未 commit/未放弃的 Start 票据数
    start_persist_inflight: u32,               // **已提交**、desired=true 仍在落盘的 Start 数（P0-D）
    deleting: bool,                            // DELETE 屏障（P0-D）：置位期间不接受**新 Start 提交**
    desired: Option<bool>,                     // 内存权威（懒初始化自 DB；此后由门写入）
    desired_seq: u64,                          // 每次成功写入 +1（诊断：证明"读到的不是旧值"）
    stop_persist_confirmed_gen: Option<u64>,   // 最近一次**已确认落盘**的 Stop 代次（R-2，收敛判据凭证）
    stop_persist_inflight: BTreeSet<u64>,      // **全部**在途 Stop 落盘代次（P0-A：集合，不是"最新一个"）
    intent_unpersisted: bool,                  // 落盘失败 ⇒ 禁止 Start，直到补写成功（§6.10.3）
    persist_lock: Arc<tokio::sync::Mutex<()>>, // per-endpoint 意图写库串行器（P0-A / P0-B）
}

pub struct StartTicket { /* gate, id, stop_gen, released */ }        // RAII：取消只减 inflight_starts
struct StartPersistGuard { /* gate, id */ }                          // RAII：归还 start_persist_inflight（P0-D）
pub struct DeleteGuard { /* gate, id, committed */ }                 // RAII：未提交即撤屏障（P0-E-2，§6.14）
pub struct StopAdmission { pub handle: StopHandle, pub persist: JoinHandle<Result<(), String>>, pub stop_gen: u64 }
```

**"门是唯一写者"这句话的前提（P0-2）——已由实现满足**：`fda7633` 时 `desired_running` 还有两条 UPDATE 路径（F25/F26）与一条建档 INSERT 路径（F28），该命题**当时不成立**；§6.10 的写路径拆分落地后成立，现状为：

- 两条 UPDATE 的列清单已不含 `desired_running`（`config-store/src/lib.rs:1523`、`:1600-1601`，函数注释 `:1517`、`:1597`）；
- 建档 INSERT 一律写 `false`（`core-api/src/bootstrap.rs:360-372`）；
- 唯一写点 = `ConfigStore::set_desired_running`（`config-store/src/lib.rs:1617-1670`），调用方只有门（`lifecycle.rs:430-433`、`:796-801`）；
- 写成功还会记**正向凭证**（`record_intent_proof`，`n>0` 才写，`:1664-1669`），供 `recovery_plan()` 判定开机是否允许恢复。

因此判据 2 的抗旧值论证现在就成立；但**新增任何"整体回写 EndpointRecord"的路径都会再次破坏它**，§6.10.2 第 2 层（类型化入参）仍作为防复发建议保留（§11-9）。

**锁序纪律（已按此实现，代码注释见 `lifecycle.rs:11-13`、`:149-152`）**：`persist_lock`（意图写库串行器）→ `gate` 锁 → `manager` 锁，**永不反向**。`manager` 的任何路径都不回调 gate（结构上保证），`admit_stop` 的同步临界区**在门锁内不等待 `persist_lock`**（落盘任务在门外 await 它），因此不构成环。`commit_start` 的同步临界区也只做"代次比对 + `start_endpoint`"，**临界区内没有 DB 写**。

### 6.3 Stop 在途保护归属于"生命周期操作"（**设计原编号 P0-A**；实现侧 `P0-A` = "跟踪全部在途 Stop 落盘代次"，见 §6.12.3）

**作废原稿写法**：上一版 §5「关键实现纪律」曾建议"`admit_stop` 用 RAII guard 兜底减计数（handler panic 时不能永久卡死 Start）"。该写法**必须废弃**，理由即复审所述：

> handler 中途被取消（客户端断开 / 网关超时 / `-TimeoutSec 10` 到点），而实际 teardown 仍在后台执行 ⇒ guard 随 future drop 而提前释放 ⇒ `inflight_stops` 归零 ⇒ **Start 重新放行** ⇒ W2 并发窗口原样重现，而且此时现场更容易触发（客户端超时是常态）。

**新规则（三条，逐条对应复审要求）**：

1. **保护载体 = attempt 表条目**，而不是任何计数器或 handler 栈帧。条目在 `start_endpoint` 里 **spawn 之前**创建（insert-before-spawn），**正常完成时**由 attempt 任务自己在返回前移除、**异常终止时保留**（§6.3.2）。因此"保护存在" ⟺ "该 attempt 尚未确认 teardown 完成"，二者**是同一件事**，不存在需要同步的副本；也不存在"panic 就把保护丢了"的缺口。
2. **释放条件 = 确认旧 attempt 已完成 teardown**，且释放动作**只由 attempt 任务自身执行**（`AttemptGuard::drop`，见下）。`stop_endpoint` 不释放、handler 不释放、超时不释放。
3. **handler 取消 / 超时 / 连接断开时保护不得释放**：handler 在该路径上只持有一个 `watch::Receiver`（等待结果用）。丢弃它只丢失"应答"，不触碰任何状态；teardown 任务继续跑，条目继续存在，Start 继续得到 409。

**RAII 不是不能用，而是不能用错宿主**：`AttemptGuard` 放在 **attempt 任务内部** 是正确的——它随任务结束而 drop，drop 里只做同步的簿记；若放在 handler 里就是灾难。**但"宿主正确"还不够**：`Drop` 在 panic 展开时**同样执行**，而 panic 那一刻 `child.wait()`、Session 清理、事件排空未必完成。因此 `Drop` 必须能区分"正常完成"与"异常终止"，判据**不是** `std::thread::panicking()`，而是**显式提交标记**（详见 §6.3.2）。

```rust
// 只存在于 attempt 任务内部（manager.rs:494-564）。committed 是本结构唯一的"完成"权威
struct AttemptGuard {
    table: Arc<Mutex<AttemptTable>>,
    id: String,
    generation: u64,
    ctx: Arc<AttemptCtx>,
    quarantine_tx: tokio::sync::mpsc::UnboundedSender<String>,   // 交给监督清理（§6.3.3）
}

impl AttemptGuard {
    /// 唯一正常退出路径调用（`run_endpoint_with_attempt` 正常返回且 ctx.outcome 已写入之后）
    fn commit(&self) {
        debug_assert!(self.ctx.outcome.get().is_some(), "commit 前必须已有 teardown 结论");
        // 先置终态再置标记：观察者看到 committed=true 时 phase 必然已是终态
        self.ctx.set_phase(AttemptPhase::TeardownDone);
        self.ctx.committed.store(true, Ordering::Release);
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        let mut t = self.table.lock().unwrap();
        // 防御：条目已不属于本代次（例如被后续 Start 取代）⇒ 什么都不做，只补发通知
        if t.live.get(&self.id).is_none_or(|e| e.ctx.generation != self.generation) {
            drop(t);
            let _ = self.ctx.done_tx.send(self.ctx.outcome.get().cloned());
            return;
        }
        if self.ctx.committed.load(Ordering::Acquire) {
            // 正常完成：写完成代次 + 移除条目，**同一临界区**（P0-3 的原子性来源）
            t.last_completed_gen.insert(self.id.clone(), self.generation);
            t.live.remove(&self.id);
        } else {
            // 异常终止：**保留条目**、转隔离态、禁止新 Start（P0-1）
            self.ctx.set_phase(AttemptPhase::FailedUnknown);
            let _ = self.ctx.outcome.set(Err(format!(
                "ATTEMPT_FAILED_UNKNOWN: attempt generation {} terminated abnormally (panic/abort); \
                 teardown completion NOT proven", self.generation)));
            if std::thread::panicking() {
                tracing::error!(endpoint = %self.id, generation = self.generation,
                    "attempt task panicked; entry quarantined as FAILED_UNKNOWN");   // 只留证据，不参与判定
            }
        }
        let _ = self.ctx.done_tx.send(self.ctx.outcome.get().cloned());
        drop(t);
        // 异常路径把 id 交给监督清理：通道是 unbounded，`send` 在 Drop 里也不 await
        if !self.ctx.committed.load(Ordering::Acquire) {
            let _ = self.quarantine_tx.send(self.id.clone());
        }
    }
}
```

> **与 v2 的两处差异（按实现更正）**：(a) 通道是 `UnboundedSender`，因此用 `send` 而非 `try_send`（语句仍是同步、非阻塞的，Drop 里绝不 await 的纪律不变）；(b) 防御分支（条目已不属于本代次）也要**补发 `done_tx`**，否则等待方可能永远等不到结论。

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
| 清除条件 | 有界复核通过（`manager.rs:1486-1546`）：① `pid` **明确已死**——`ProcessStatus` 三态 `{Alive, Dead, Unknown}`，`Alive` 先强杀，`Unknown`（权限不足/查询异常）**必须继续隔离并重试**，只有 `Dead` 才算收敛（`pid==0` 视为 `Dead`）；② `registry` 中该 `endpoint_id` 的键已不存在（隔离条目已无活 attempt 持有它）。两条都成立才允许清除 |
| 清除动作 | 在**同一把 attempt 表锁**内：`last_completed_gen.insert(id, gen)` + `live.remove(id)`；`outcome` **保持** `Err(ATTEMPT_FAILED_UNKNOWN…)` 不被改写（**失败必须仍是失败**）；`done_tx` 已在 Drop 里发过，不重发 |
| 复核不通过 | 条目**保留**，`tracing::error!` 并重试：`RETRIES = 60` × `RETRY_DELAY = 500ms`（≈30 s，覆盖 tokio 孤儿队列的异步回收僵尸，`manager.rs:1467-1469`）。本轮仍无法证明收敛 ⇒ 保留隔离态 + 大声告警，只能重启 Core 收敛——与 §6.3.1 "尾部无界 ⇒ Start 永远 409" 是同一个 fail-closed 选择 |
| 清除前 Start 返回什么 | **409**，`code = ATTEMPT_FAILED_UNKNOWN`（§6.8），message 带 `gen` 与原始异常摘要。**永不 200、永不排队、永不自动重试** |
| 清除前 Stop 返回什么 | `begin_stop` 命中隔离条目 ⇒ **不重复 teardown**，直接返回该条目已发布的 `outcome`（即同一次明确失败，符合 §5.2 第四行）；**Stop 不清除隔离**（否则等于用一次 Stop 绕过复核） |
| 清除后 | Start 允许，新 `gen = 旧 gen + 1`（§6.5 计数器单调）。注意：**隔离清除也必须写 `last_completed_gen`**，否则 P0-3 的判据在"异常结束"这条路径上又断了——异常 attempt 同样需要一个"终结凭证"，只是结论是失败 |
| 与 §10-1 的边界（必须写进代码注释） | `attempt_reaper` **只清理已死 attempt 的残留**：不启动、不重启、不写 `desired_running`、不改快照、不 cancel 任何活着的 attempt ⇒ **它不是对账器/看门狗**，不违反 §10-1 |
| 与进程重启的关系 | 隔离态只在内存。重启后按 DB 期望态恢复：若 Stop 意图已落盘 `false`，不会自动拉起（正确方向）；若**意图落盘失败**（§6.10.3），重启后可能自动拉起——这正是 §6.10.3 要堵的洞 |

**已知代价（诚实声明）**：`attempt_reaper` 是新增的常驻任务与一条通道，且"异常即隔离"会让一个 panic 变成"该端点必须等复核"的可用性事件。这个代价换的是"停不干净就不许启动"，在机床场景下方向正确。

### 6.4 Start 两阶段提交

```rust
// lifecycle.rs:339-349 —— 同步；inflight_starts += 1；不查 is_running、不碰 DB、不起进程
admit_start(id) -> StartTicket { stop_gen }     // 票据 RAII：取消只减计数（此时无任何外部副作用）

// lifecycle.rs:364-453 —— 两段式：门外取串行器 → 同步临界区提交 → 门外落盘
async commit_start(t, cfg) -> Result<(), LifecycleError>
  { 快路径（持门锁，无 await）:                                         // :371-388
      check_start_preconditions(g, id, t.stop_gen)?                     //   → INTENT_UNPERSISTED 503
                                                                        //   → SUPERSEDED_BY_STOP 409
                                                                        //   → ENDPOINT_DELETED 409（P0-D 屏障）
      if !g.stop_persist_inflight.is_empty() && !mgr.has_live_attempt(id)
          -> STOP_PERSIST_IN_PROGRESS 409        （P0-A：任何在途 Stop 落盘都禁止提交）
      if mgr.has_live_attempt(id) -> mgr.start_endpoint(cfg)            // 让 manager 给出既有码
      persist_lock = Arc::clone(g.persist_lock) }
    persist_lock.lock_owned().await                                     // :392 门外 await（P0-B）
  { 二次判定（持门锁，无 await）:                                       // :394-417
      check_start_preconditions(...)?                                   // 等锁期间可能有新 Stop
      if !g.stop_persist_inflight.is_empty()
          -> has_live_attempt ? start_endpoint（STOP_IN_PROGRESS）
                              : STOP_PERSIST_IN_PROGRESS 409
      mgr.start_endpoint(cfg)?       // 同步，insert-before-spawn；冲突码见 §6.6
      StartPersistGuard::register(g, ...)   // P0-D：同一临界区登记"意图正在落盘" }
  spawn_blocking { set_desired_running(id, true) → finish_start_persist(...) }.await   // :426-452
```

`finish_start_persist`（`lifecycle.rs:504-573`）把落盘结果映射为响应：

| 落盘结果 | 门状态动作 | 返给 handler |
|---|---|---|
| `Ok(true)`（DB 已写 true） | 仅当 `stop_gen` 仍等于本票据代次才推进 `desired=true`/`desired_seq+1`/清 `intent_unpersisted`；否则内存意图保持 Stop 的 `false`（Stop 自己的写排在后面），只记 warn。成功且是最新代次时清**停止意图围栏**（S2 的人工确认语义） | `Ok` → 200 |
| `Ok(false)`（UPDATE 命中 0 行） | **P0-7**：端点已被并发删除而 attempt 已在跑 ⇒ 立即 `begin_stop` 认领停止（`was_running` 时在 runtime 里 await 掉结果），不留"配置已删、驱动仍跑"的孤儿 | 409 `ENDPOINT_DELETED` |
| `Err` 或 blocking 任务 panic | attempt 已启动、无法回滚 | 500 `START_INTENT_NOT_PERSISTED` |

- **为什么取消场景仍然成立**：`commit_start` 里只有两处 `await`（取串行器、等落盘结果），**都不改变门状态**；"代次比对 + `start_endpoint`"在同一个同步临界区内完成 ⇒ 客户端取消**无法**落在"代次比对通过、但 start 尚未发生"之间。这是 W1/W2 在取消场景下仍然成立的前提（`lifecycle.rs:361-363` 的注释即此语义）。
- **为什么把落盘移到临界区外（P0-B）**：若在门锁内同步写库，Stop 的 `begin_stop` 也要取同一把门锁 ⇒ DB 慢 3 s 就把停止推迟 3 s。拆开后由 `persist_lock` 串行器保证"Start 的 `true` 先于随后受理的 Stop 的 `false`"。端到端判据 `slow_start_persist_does_not_delay_stop_begin`（`stop_lifecycle.rs:1886`）：`begin_stop` 必须 <200 ms 返回，而 Start 的响应必须 ≥3 s（等自己的落盘）。
- **禁止**在临界区内调 `build_runtime_endpoint`（它 await 且可能起临时进程，F8）；构造留在 `S1`，因此需要票据来检测"构造期间 Stop 已受理"。
- **禁止**在临界区内调 `manager.stop_endpoint`（async）。
- **落盘失败不允许 `let _`**：Start 方向写失败时 attempt 已经在跑、无法回滚，因此必须**显式失败**（500 `START_INTENT_NOT_PERSISTED`），并留下"`desired` 与事实不一致"的诊断（重启后该端点不会自动恢复，属安全方向）。

Stop 侧（`lifecycle.rs:744-816`，**步骤顺序不可交换**：S1 停止优先）：

```rust
admit_stop(id) -> StopAdmission { handle, persist, stop_gen }    // 同步临界区（:745-784）
    1. g.stop_gen += 1                                  // 先作废所有在途 Start 票据（W1）；也是墓碑代次的基础
    2. g.desired = Some(false)                          // §5.3：内存意图前移（desired=false + state!=STOPPED 合法）
    3. g.stop_persist_inflight.insert(stop_gen)         // P0-A：登记**本次**代次（不是覆盖"最新代次"）
    4. g.stop_persist_confirmed_gen = None              // R-2：本次还没确认，不能复用上次的凭证
    5. handle = mgr.begin_stop(id)                      // 同步：认领并**立即开始** teardown（无 await、无 DB）
    // 注意：**没有** "屏障期跳过落盘" 的分支（P0-E-1 已删除该短路，见 §6.13）
    → 出门后在独立任务里：persist_lock.lock_owned().await → spawn_blocking(set_desired_running(id,false))
      → finish_stop_persist(id, stop_gen, res)          // 与 teardown **并行**，不阻塞停止（S1）

pub async fn stop_endpoint(&self, id: &str) -> Result<bool, String>   // manager.rs:1025-1045，签名不变
{ let h = self.begin_stop(id); h.await_result().await }              // 合流到同一个 outcome

pub fn begin_stop(&self, id: &str) -> StopHandle                       // manager.rs:996-1012
    match table.live.get(id) {
        None                                          => NotRunning,                                   // was_running=false
        Some(e) if e.ctx.is_committed()                => Join(...),   // 已完成、只差 Drop 移除条目
        Some(e) if e.ctx.phase() == FailedUnknown     => Join(...),   // P0-1：同一次明确失败，不重复拆解
        Some(e) if e.ctx.phase() == Stopping          => Join(...),   // §5.2：合流，不重复拆解
        Some(e) => { e.ctx.set_phase(Stopping); e.cancel.cancel(); Join(...) }
    }
```

`finish_stop_persist`（`lifecycle.rs:822-914`）三个动作必须一起看：

1. **无论是否最新代次，都把自己的代次从集合移除**——否则一次已完成（或已失败）的落盘会把 Start 闸门永久锁死；
2. **只有最新代次能推进门状态**（`desired=false`、`desired_seq+1`、`stop_persist_confirmed_gen=Some(gen)`、清 `intent_unpersisted`）；更早代次的迟到成功不得覆盖更新一次 Stop 的事实（P0-A）；
3. **失败**：置 `intent_unpersisted = true`（禁止新 Start，503）+ 写持久化围栏（S2，DB + sidecar 双通道）+ 返回 `Err` 让响应变 500；**陈旧代次的失败只记 warn**，不得把门锁死。

- **`begin_stop` 同步** ⇒ "受理"与"启动拆解"不可分割：不存在"代次已 +1、desired 已写假、但 teardown 没启动"的中间态。
- **`stop_endpoint` 保留 async 签名** ⇒ 既有调用点（`stop_lifecycle.rs`、`reconnect_backoff.rs`、`event_persistence.rs` 等）一字不改。
- **对外结果映射（`lib.rs:2262-2299`，逐条对应 §5.2）**：`join!` 同时等 teardown 结论与落盘结论 ⇒ **落盘失败优先**：返回 500 `STOP_INTENT_NOT_PERSISTED`（即使 teardown 成功）；否则 `Ok(was)` ⇒ 200（`was_running=true/false`），`Err(detail)` ⇒ 500 `STOP_FAILED`（detail 以冻码开头，见 §6.8）。
- **`intent_unpersisted == true` 时不得返回正常成功**：这是 §5.2 第三行的**前置条件**——"teardown 成功 → 200"以"停止意图已落盘"为前提；两者同时失败时，用户必须看到失败。

### 6.5 代次隔离：快照写入必须带代次（**P0-4**；实现侧 `P0-D` 指删除屏障，见 §6.12）

**问题（修复前，`fda7633`）**：`Snapshot::upsert_endpoint` 最后写入者胜（`snapshot.rs:208-213`）⇒ 旧 attempt 的 STOPPED（`endpoint.rs:357-368`）可覆盖新 attempt 的 RUNNING（`endpoint.rs:597-605`）。

**方案**：把 `endpoints` 的值从 `EndpointStatus` 换成带代次的记录，并且**把墓碑放进同一把锁**（P0-4 的关键，不是可选优化）。

```rust
// snapshot.rs：一个锁 = 一张表 = 一个线性化点
struct EndpointTable {
    live: HashMap<String, EndpointRecord>,   // 替换原 HashMap<String, EndpointStatus>
    retired: HashMap<String, u64>,           // 墓碑：DELETE 时记录"已退休的最大代次"
}
struct EndpointRecord { generation: u64, status: EndpointStatus }
endpoints: RwLock<EndpointTable>,            // 唯一锁；**不另设 retired 锁**

pub fn upsert_endpoint(&self, generation: u64, status: EndpointStatus) -> bool {
    let mut t = self.endpoints.write().unwrap();          // ← 检查与写入在同一临界区
    if t.retired.get(&status.endpoint_id).is_some_and(|&r| generation <= r) {
        return false;                                     // 墓碑门：禁止复活已删除端点
    }
    if t.live.get(&status.endpoint_id).is_some_and(|rec| generation < rec.generation) {
        return false;                                     // 旧代次写入，丢弃
    }
    t.live.insert(status.endpoint_id.clone(), EndpointRecord { generation, status });
    true
}

pub fn remove_endpoint(&self, id: &str, gen_floor: u64) {
    let mut t = self.endpoints.write().unwrap();          // ← 同一临界区
    let floor = gen_floor.max(t.live.get(id).map_or(0, |e| e.generation));
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
- 写入门禁只在一个地方：`set_status`（`endpoint.rs:491-505`）带 `generation` 参数；`upsert_endpoint` 的生产调用点只有它（已核实：`crates/` 内仅 `endpoint.rs:501` 一处），churn 极小。
- 代次来源：`AttemptCtx.gen`，由 `start_endpoint` 分配；**per endpoint 计数器、永不回收**（`MesaManager` 内的 `HashMap<String, AtomicU64>` 只增不删；端点 DELETE 后重建继续递增，绝不复用）。理由：墓碑需要一个"该 id 曾发放过的最大代次"作下界，计数器一旦回收或重置，`retired` 就可能低于在途写入的 `gen`，墓碑门立刻失效。
- `gen_floor` 的来源：`MesaManager::gen_floor(id)`（`manager.rs:817-828`）= 该 id **已发放的最大代次**（"下一个将要发放的值 − 1"）。`remove_endpoint` 的两个生产调用点都必须传它：DELETE 端点（`core-api/src/lib.rs:2187-2189`，从 `state.manager` 取）与 Bootstrap 失败补偿删除（`bootstrap.rs:449`）。若传 0 或省略，"从未写过状态的端点被删后，迟到写入 `gen>0` 仍会复活"。**差一（off-by-one）是这里的致命细节**：`gen_floor` 必须是"已发放的最大值"，**不能**用"下一个将要发放的值"——否则端点被删除后重建，第一个新 attempt 的 `gen` 正好等于墓碑值，`gen <= retired` 成立，**新端点的状态写入会被永久丢弃**（表现为"重建后端点永远不显示状态"）。该细节已有单测钉住：`delete_then_recreate_status_write_is_accepted`（`snapshot.rs:1038`）。
- `run_endpoint` 需要拿到 ctx ⇒ 新增 `run_endpoint_with_attempt(..., ctx)`，**保留** `run_endpoint_with_heartbeat` 旧签名委托（ctx=None，`gen=0`），使 `subprocess_recovery.rs:186` 零改动；直调路径的 `gen=0` 因此**永远无法覆盖**任何正式 attempt（`gen≥1`），也**永远无法复活**已删端点（`0 <= retired` 恒成立）。

**为什么在有了 attempt 表之后还需要它**（诚实论证，避免"多余设计"质疑）：

1. **DELETE 路径仍然需要墓碑**：DELETE 现在以 **`has_live_attempt`** 为准（`lib.rs:2130`，不再是 `is_running`），所以"拆解中来删"已被拒；但仍有两条会让旧代次写入落到"已删/已重建"记录上的路径：(a) 绕过 Manager 直调 `run_endpoint_with_heartbeat` 的夹具路径（F22/F23，`gen=0`）在跑时会写状态；(b) Bootstrap 失败补偿删除（`bootstrap.rs:449`）与"同 ID 重建"之间没有别的同步机制。代次门是"DELETE 之后绝不复活"**可证明**的唯一手段（P0-D 负责的是另一件事：**在途意图写入**的收敛，见 §6.12）。
2. **顺序保证是隐式跨函数不变量**："条目移除晚于 STOPPED 写入"依赖 `endpoint.rs:638` 与 `:357` 的相对位置，任何一次重构都可能静默破坏；代次门把它变成**显式可断言**的约束，并有回归测试钉住。
3. **纵深防御**：F22/F23 存在绕过 Manager 直接 spawn attempt 的路径；代次门对它们同样生效（gen=0 的直调 attempt 无法覆盖 gen≥1 的正式 attempt）。
4. **原子性本身就是被复审点名的缺陷（P0-4）**：墓碑与状态写入分属两把锁时，"检查—写入"非原子；单锁版本才使"DELETE 之后绝不复活"成为**可证明**的性质，而不是"窗口很小"的经验判断。

**回归测试**（复审指定）：`concurrent_teardown_and_new_start_never_shows_stopped_while_collecting` —— 见 §9。补充说明：修复后 HTTP 路径上"旧 STOPPED 晚于新 RUNNING"已被 attempt 表**结构性**排除（条目移除晚于 STOPPED 写入），因此该场景的**确定性**红/绿放在 Snapshot 单测（`stale_gen_write_is_dropped`）与 DELETE 复活场景（集成）上；集成层的"不得长期显示 STOPPED 而实际在采集"以**不变量断言**表达（5 s 窗口内 `state==RUNNING` 且 `latest` 时间戳持续推进）。

**并发 DELETE + 迟到写入的专项回归（P0-4 指定）**：`concurrent_delete_and_late_status_write_never_resurrects`（单测 + 集成各一份，§9）。判据不是"跑很多次没出事"，而是**可判定的不变量**：`remove_endpoint` 返回之后，任何 `upsert_endpoint(gen<=gen_floor, …)` 的返回值必须为 `false`，且 `endpoint(id)` 恒为 `None`；把该断言放进一个 2 线程（upsert 循环 × remove）的压测里，断言在 `remove` 返回后的**每一次**读都成立。

### 6.6 防孤儿：insert-before-spawn + 拒绝并发 attempt（**设计原编号 P0-E**；实现侧 `P0-E-1/E-2` 指删除屏障两项，见 §6.13、§6.14）

**修复前**（`fda7633`，`manager.rs:513-516`）：`insert` 是覆盖写，第二个 Start 会丢弃第一个 `RunningEntry`（JoinHandle + cancel token）⇒ 孤儿打到进程退出。

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
| 进程不变量 | 全局 `LIVE_CHILDREN: AtomicUsize`（**已落地**：`process.rs:30-34`，spawn 后 +1 `:245`、`wait()` 回收后 −1 `:258-262`，强杀路径同样退出计数 `:319`）：Stop 完成后该值必须回落到"仅剩临时 Descriptor 进程"的基线（0） |
| 端到端 | `w3_double_start_only_one_attempt`（`stop_lifecycle.rs:717`）+ `double_start_only_one_attempt_at_table_level`（`:1516`）：并发两 Start，恰好一个 200/一个 409；随后 Stop，断言无条目、`LIVE_CHILDREN == 0`（`:1548-1554`）、`!has_live_attempt(id)`；平台侧 `pid` 不再存活（辅助，注意 pid 复用） |
| 基线复用 | 仓库已有 `subprocess_orphan_guard` suite（见 AGENTS.md 名单），新增场景挂在该 suite 内，不改 `scripts/contract_suites.py` |
| 运行期诊断 | `state` 与 attempt 表不一致时 `tracing::warn!`（只读告警，不自动修复） |

**已知边界（必须声明）**：F22/F23——直接调用 `run_endpoint_with_heartbeat` 的路径不经过 attempt 表，**不受本机制保护**。当前只有测试夹具这样用（`subprocess_recovery.rs:186`），生产唯一入口是 `manager.rs:503`。设计中给该函数加 `NOTE` 注释说明"绕过 Manager 即放弃 §7 的全部判据"。

### 6.7 请求取消语义（P1-F）

**统一原则**：**所有状态迁移都在同步函数内完成；所有 `await` 只用于"等待结果"，不承担迁移职责。** 这样取消只丢失应答，不丢失状态。

| 事件 | 旧行为 | 新行为 |
|---|---|---|
| Start handler 在构造期被取消 | 无副作用（尚未提交）；但构造可能已在跑 Descriptor 临时进程 | 无副作用；票据 RAII 释放，`inflight_starts` 归零；**不触碰** desired/attempt |
| Start handler 在提交后被取消 | 已 start、已置真；客户端不知结果 | 同上（提交是同步的，取消只能发生在提交之后）⇒ 状态一致；结果可查（`GET /api/v1/endpoints` 的 `state`） |
| **Stop handler 被取消（断开/超时）** | `lib.rs:2180` 的 `set_desired_running(false)` **永不执行** ⇒ `desired_running` 永久为 true（重启会自动拉起一个用户明确要停的端点） | 受理时（同步临界区）已置内存意图 `false` 并**派发独立落盘任务**；落盘任务持有串行器并在 `spawn_blocking` 里自行完成门状态归并（`lifecycle.rs:419-452`、`:788-810`）⇒ 取消只丢应答，不丢意图；teardown 继续跑完；条目由任务自删 ⇒ 保护不提前释放 |
| **DELETE handler 被取消**（客户端断开/超时/**panic 展开**） | 显式 `abort_delete()` 在提前返回与展开路径上执行不到 ⇒ `deleting` 永久为 `true` ⇒ 该端点此后所有 Start 都 409 `ENDPOINT_DELETED`，只能重启 Core（P0-E-2） | 屏障由 `DeleteGuard` 持有：未 `commit()` 即撤销（Drop 覆盖三条非正常退出路径），且 Drop 内**不 panic**（`let Ok(..) else return`）。删除本身**没有发生**，端点仍可用（§6.14、`lifecycle.rs:221-263`） |
| 客户端拿不到 Stop 结果 | 无补偿 | 结果留在 `AttemptCtx.outcome` 与 `done_tx`：再次 `POST /stop` 会**合流**到同一次 teardown 并返回同一结果（幂等 join），或读到 `was_running=false`（已完成）。同时落 `tracing` 结构化日志 |

**"返回给客户端的结果可以是未知，但服务端状态必须可查"**：可查渠道全部是既有能力——`GET /api/v1/endpoints`（`state`，已代次化）、`GET /api/v1/endpoints/{id}`（`desired_running`，`lib.rs:1508`/`:1576`）、结构化日志。**不新增 REST 路由或字段**（§10）。

### 6.8 错误码（**逐个按 `9611255` 的实际代码核对**）

body 形状不变（`json_error(code, message)`），只增加新的 `code` 字符串；不新增路由、不新增响应字段。下表每一行都给出**判定位置**，可直接对照源码。

**Start（`POST /api/v1/endpoints/{id}/start`）**

| code | HTTP | 触发条件 | 判定位置 | 相对 `fda7633` |
|---|---|---|---|---|
| `INTENT_UNPERSISTED` | 503 | 该端点上一次 Stop 的意图落盘失败（`intent_unpersisted==true`），补写成功前拒绝启动 | `lifecycle.rs:461-470`（`check_start_preconditions` 步骤 0） | **新增** |
| `SUPERSEDED_BY_STOP` | 409 | 票据 `stop_gen` 与当前不等（Stop 在 admit 之后、commit 之前受理） | `lifecycle.rs:471-481`（步骤 1） | **新增** |
| `ENDPOINT_DELETED` | 409 | (a) DELETE 屏障期间提交（`deleting==true`）；(b) `desired=true` 落盘时 UPDATE 命中 0 行（行已被并发删除，attempt 已认领停止） | `lifecycle.rs:482-491`；`:541-557` | **新增**（P0-D / P0-7） |
| `STOP_PERSIST_IN_PROGRESS` | 409 | **任一**在途 Stop 的 `desired=false` 尚未确认（P0-A），且当前无活 attempt | `lifecycle.rs:378-380`、`:398-408`；构造函数 `:1064-1072` | **新增** |
| `STOP_IN_PROGRESS` | 409 | attempt 条目存在且 `phase==Stopping`（Stop 已受理、teardown 未完成） | `lifecycle.rs:1103-1105` ← `manager.rs:912-915` | **新增** |
| `ATTEMPT_FAILED_UNKNOWN` | 409 | 条目存在且 `phase==FailedUnknown`（异常终止待复核，P0-1） | `lifecycle.rs:1100-1102` ← `manager.rs:906-911` | **新增** |
| `CONFLICT` | 409 | 条目存在且 phase 既非 `Stopping` 也非 `FailedUnknown`（真的已在运行）；文案沿用"endpoint 已在运行" | `lifecycle.rs:1106-1113` | 语义不变（基线 `lib.rs` 同码同文案） |
| `START_FAILED` | 400 | 其余启动失败原样透传（`EVENT_STORE_UNAVAILABLE`、driver not found 等） | `lifecycle.rs:1114-1115` | 不变 |
| `START_INTENT_NOT_PERSISTED` | 500 | attempt 已启动、`desired=true` 落盘失败或 blocking 任务异常终止（不可回滚） | `lifecycle.rs:443-451`、`:558-571`；bootstrap 复用同码 `bootstrap.rs:127-132` | **新增** |

**Stop（`POST /api/v1/endpoints/{id}/stop`）**

| code | HTTP | 触发条件 | 判定位置 | 相对 `fda7633` |
|---|---|---|---|---|
| `STOP_INTENT_NOT_PERSISTED` | 500 | `desired=false` 落盘失败（**优先于 teardown 结果**：teardown 成功也返回它）；响应文案是冻结常量 | `lib.rs:2277-2289`；文案 `lifecycle.rs:46-47` | **新增** |
| `STOP_FAILED` | 500 | teardown 结论为 `Err(detail)`，`detail` 以 §下方 8 个冻码之一开头 | `lib.rs:2295-2298` | 不变（基线同码） |

**DELETE（`DELETE /api/v1/endpoints/{id}`）**

| code | HTTP | 触发条件 | 判定位置 | 相对 `fda7633` |
|---|---|---|---|---|
| `CONFLICT` | 409 | 有活 attempt（含拆解中与隔离态）："请先停止后再删除"；屏障并在等待期间**再次**复查该条件 | `lib.rs:2130-2138`、`:2167-2175` | 判据由 `is_running` 换成 `has_live_attempt`（行为更严） |
| `DELETE_PERSIST_IN_PROGRESS` | 409 | 等待在途运行意图写入收敛超预算（`DELETE_PERSIST_WAIT = 10s`）⇒ 拒绝删除（不删、只撤屏障，可重试） | `lifecycle.rs:1079-1096`；预算常量 `:55`、轮询 `:58` | **新增**（P0-D） |
| `NOT_FOUND` | 404 | 行已不存在 | `lib.rs:2199-2202` | 不变 |

**`stop_endpoint` 的冻结码集合：8 个（原 7 个 `EVENT_*` 一字未改 + 新增 1 个）**

`tests/driver-contract/tests/stop_lifecycle.rs:68-83` 的 `assert_frozen_stop_code` 现为：

```
EVENT_DRAIN_TIMEOUT, EVENT_DRAIN_FAILED, EVENT_SEQUENCE_REGRESSION, EVENT_ID_COLLISION,
EVENT_STORE_UNAVAILABLE, EVENT_RECORD_INVALID, EVENT_STREAM_CLOSED,
ATTEMPT_FAILED_UNKNOWN            ← 唯一的增量（G7 §6.8，隔离态让 stop_endpoint 必须能返回可识别的同一失败）
```

- **零新增、零修改**的部分必须写清：P0-D / P0-E-1 / P0-E-2 **没有引入任何新码**，全部复用既有码——`ENDPOINT_DELETED`（屏障与"行已删"同一语义）、`DELETE_PERSIST_IN_PROGRESS`（删除面）、`STOP_PERSIST_IN_PROGRESS`（Start 面）、`STOP_INTENT_NOT_PERSISTED`（Stop 落盘失败）、`STOP_FAILED`（teardown 失败）。
- `STOP_INTENT_NOT_PERSISTED` / `START_INTENT_NOT_PERSISTED` / `INTENT_UNPERSISTED` 由 `core-api` 的门产生，**不经过** `stop_endpoint` 的 `Err`，因此**不进**上述冻结集合（`stop_lifecycle.rs:66-67` 的注释即此语义）。
- 理由：测试与现场证据需要**机器可判别**"Stop 优先拒绝"、"已在运行"、"隔离态"、"落盘在途/未落盘"、"删除被拒"五类结果；`manager` 侧沿用既有的"精确码前缀"约定（`EVENT_STORE_UNAVAILABLE: …`、`STOP_IN_PROGRESS: …`、`ATTEMPT_FAILED_UNKNOWN: …`），故 `Err(String)` 签名不变，`core-api` 按前缀映射状态码与 code（`lifecycle.rs:1098-1116`）。

### 6.9 取舍评价表

| 维度 | 评价 |
|---|---|
| 改动面 | `driver-manager`：代次化 attempt 表 + `AttemptGuard` + `attempt_reaper_loop`（`manager.rs:409-563`、`:1458-1563`）、`set_status(generation, …)`（`endpoint.rs:491-505`）、`upsert_endpoint(generation, …)` + 单锁墓碑 + `remove_endpoint(gen_floor)`（`snapshot.rs:153-260`、`:495-525`）、`LIVE_CHILDREN`（`process.rs:30-34`）；`core-api`：新增 `lifecycle.rs`（门 + 意图落盘串行器 + DELETE 屏障 + 结算观察器）、`recovery.rs`（P0-C 产品策略）、start/stop/delete handler 改写（`lib.rs:2122-2300`）、Bootstrap 三处（`bootstrap.rs:150-166`、`:360-372`、`:104-194`）；**`config-store`：两条 UPDATE 去掉 `desired_running` 列 + 唯一写点 `set_desired_running` + 停止意图围栏（双通道）+ 正向凭证 + `recovery_plan`**（`lib.rs:1517`、`:1597-1603`、`:1617-1795`、`:708-767`）。`MesaManager` 的既有公共签名不变（`start_endpoint`/`stop_endpoint` 保留），新增 `begin_stop`/`has_live_attempt`/`attempt_phase`/`current_gen`/`gen_floor` 五个同步/查询方法（`manager.rs:790-828`、`:996-1012`） |
| 跨驱动影响 | 无：门与协议、驱动、Descriptor 无关，S7/FOCAS2/OPC UA/Simulator 一致 |
| 回归风险 | 中：行为变化**六处**——竞态分支下 Start 200→409（含新 code）；并发 Stop 由"立即 200 false"改为"合流后返回同一结果"（§5.2）；`desired_running` 写入时机由"teardown 之后"前移到"受理时"（§5.3）；**Stop 意图落盘失败由静默成功改为 500**（§6.10.3）；**在途 Stop 落盘未确认期间 Start 返回 409 `STOP_PERSIST_IN_PROGRESS`**（P0-A）；**DELETE 在意图写入未收敛时返回 409 `DELETE_PERSIST_IN_PROGRESS` 而不是立即删行**（P0-D）。`mesa-web` 对 start/stop 只读 `status` 与 `error.message`，不匹配 code（`apps/mesa-web/src/api.ts:146-147`、`deviceModel.ts:446-450`），其"stop→apply→start"是**顺序 await**（`deviceModel.ts:474-494`），不会自撞；但 500 会让其提示"停止失败"，这是**期望行为** |
| 删除语义 | 删除从"看到没有活跃 attempt 就删"改为**屏障 → 等意图写入收敛（≤10s）→ 串行器复查 → 删行 → 立墓碑**；等不到就 409 让客户端重试（删除不可逆，宁可不删）。正常路径**零额外延迟**：两个计数器都是 0 时第一次观测即返回（`lifecycle.rs:49-54`） |
| A1 退避 | 不受影响：门在 attempt 之上，`backoff_idx` 的生成/归零语义（`endpoint.rs:315`、`:416-418`）一字未改；反而**减少**了一次"新 attempt 从 0 档重来"的伪重连。新增的观察器与 `attempt_reaper` 只读/只清理残留，**绝不** cancel 活着的 attempt、绝不重启（否则等于引入 §10-1 拒绝的对账器） |
| wire/IPC | 不改（`EndpointStatus` 字段不变，代次只在 map 键结构里） |
| 持久化 | 不加列：`generation`/`last_completed_gen` 只在内存；`desired_running` 仍是既有单列 UPDATE（`config-store/src/lib.rs:1655-1659`），但**只允许由 `set_desired_running` 写入**（§6.10）；S2 围栏复用既有 `meta` 键值表 + sidecar 文件、P0-4 凭证用 sidecar 文件，均**无 schema 变更** |
| 新增常驻资源 | 一个 `attempt_reaper` 任务 + 一条 `quarantine_tx` 通道（§6.3.3）；无定时轮询（只由 panic 路径触发），空载零开销 |

### 6.10 `desired_running` 的写路径拆分（P0-2）

**已核实的代码事实（按 `9611255` 逐行复核；"修复前"位置指 `fda7633`）**：

| # | 事实 | 修复前位置 | 现状（`9611255`） |
|---|---|---|---|
| W-a | `update_endpoint_with_secrets()` 的 UPDATE 带 `desired_running=?4`，值取自调用方传入的 `rec` | `config-store/src/lib.rs:1091-1094` | **该列已删除**：`UPDATE endpoints SET device_id=?1, name=?2, connection_json=?3, updated_at_ns=?4 WHERE id=?5`（`:1492` 函数、`:1600-1601` SQL、`:1517` 注释"本函数永不写运行意图"） |
| W-b | `update_endpoint()` 的 UPDATE 同样带该列（无生产调用方，但仍是公开 API） | `config-store/src/lib.rs:1165-1168` | **同样已删除**（`:1587` 函数、`:1523` SQL、`:1597` 注释） |
| W-c | `core-api` 的 PUT 是"读 → 长 `await` → 把旧对象整体回写"：`rec` 读出后 `await get_descriptor`（懒加载，可能起临时驱动进程），最后才落盘 ⇒ `rec.desired_running` 是**读时的旧值** | `core-api/src/lib.rs:1979-1987`、`:1998`、`:2094-2099` | **流程未改**（这是有意的）：`rec` 在 `:1992` 读出、`:2011` `await get_descriptor`、`:2109-2111` 回写。**旧值在 SQL 层被物理忽略**，因为列已不在列清单里 |
| W-d | Bootstrap 在建档事务里就以 `desired_running=true` 插入（`body.start` 缺省 true） | `bootstrap.rs:297` + `config-store/src/lib.rs:1259-1261` | **建档一律写 `false`**（`bootstrap.rs:360-372`；INSERT 语句见 `config-store/src/lib.rs:1865-1867`，值来自 `endpoint.desired_running`） |
| W-e | Bootstrap 启动成功后再写一次真；失败只记 error、不翻转 start 结果 | `bootstrap.rs:110-125` | **改走门**：`admit_start` → `build_runtime_endpoint` → `commit_start`（`bootstrap.rs:150-166`）；失败分类见 R-1（`:104-133`） |
| W-f | `set_desired_running()` 是唯一的"只写意图"函数（单列 UPDATE） | `config-store/src/lib.rs:1178-1189` | **唯一写点**：`:1617-1670`，SQL 为 `UPDATE endpoints SET desired_running=?1, updated_at_ns=?2 WHERE id=?3`（`:1655-1659`），写成功后记正向凭证（`:1664-1669`） |

**结论（写进设计与代码注释）**："`LifecycleGate` 是 `desired_running` 唯一写者"在 `fda7633` 时**不成立**（W-a + W-c 一次改名就能把 Stop 写入的 `false` 覆盖回读时的 `true`，W-d 建档直写 `true`）。**G7 已按 §6.10.2 落地修正，该命题现在成立**；任何新增的"整体回写 `EndpointRecord`"路径都会再次破坏它，因此 §6.10.2 第 2 层（类型化入参）作为防复发建议保留（§11-9）。

#### 6.10.1 拆分原则（一句话）

**普通 Endpoint 配置更新（名称 / 设备 / 连接 / Secret）不得再修改 `desired_running`；运行意图只能经生命周期门写入。**

#### 6.10.2 具体改法（哪个函数、哪个 SQL、以及"旧对象写回"为何不再覆盖意图）

| # | 改动 | 具体内容 |
|---|---|---|
| 1 | `update_endpoint_with_secrets()` | UPDATE **去掉 `desired_running` 列**：`UPDATE endpoints SET device_id=?1, name=?2, connection_json=?3, updated_at_ns=?4 WHERE id=?5`；占位符随之重排，`:1093` 里的 `desired_running as i32` 参数一并删除。函数注释写明"**本函数永不写运行意图**" |
| 2 | `update_endpoint()` | 同样去掉该列（`:1166-1167`）。它当前无生产调用方，但保留时必须与 (1) 同语义，否则将来被复用即复发 |
| 3 | `EndpointRecord.desired_running` | **保留字段**（它是 `get_endpoint`/`list_endpoints` 的读取结果，也是 REST 输出，`lib.rs:1508`、`:1576`），但**从"可写字段"降级为"只读快照"**：`UPDATE ... SET desired_running` 只允许出现在 `set_desired_running()` 里 |
| 4 | `core-api` PUT（`lib.rs:1955-2107`） | **流程不用改**：`rec` 仍从 `:1979` 读出并整体回写，但 UPDATE 已不含该列 ⇒ 旧对象里的 `desired_running` 在 SQL 层被**物理忽略** |
| 5 | Bootstrap 建档（`bootstrap.rs:297`） | 建档事务里一律写 `desired_running=false`（"刚建好 = 未运行"）；`body.start=true` 的启动意图改由 `commit_start` 步骤 3 在启动成功后写入。收益：DB 里不再可能出现"从未经过门却为 true"的行 |
| 6 | Bootstrap 启动（`bootstrap.rs:110-125`） | `set_desired_running(id, true)` 改为 `gate.write_desired(id, true)`（同 §6.4 步骤 3：失败显式、不再静默） |
| 7 | 三个 handler 写点（`lib.rs:2155`、`:2171`、`:2180`） | **全部删除**，改由 `commit_start` / `admit_stop` 在门内写入（§6.4） |

**落地证据（`9611255`，逐条对应上表）**：

| # | 落地位置 |
|---|---|
| 1 | 函数 `config-store/src/lib.rs:1492`；SQL 列清单 `:1600-1601`；注释 `:1517` |
| 2 | 函数 `config-store/src/lib.rs:1587`；SQL 列清单 `:1523`；注释 `:1597` |
| 3 | `EndpointRecord.desired_running` 仍是读写字段（`get_endpoint` `:1566`、`list_endpoints` `:1547`；REST 输出 `lib.rs:1508`、`:1576`），但**只有 `set_desired_running` 会写它**（`:1617-1670`） |
| 4 | PUT 流程不变（`lib.rs:1968-2120`）：靠"列不在列清单"物理保证 |
| 5 | `bootstrap.rs:360-372`（`desired_running: false` + 注释）；REST `POST /endpoints` 建档同样是 `desired_running: false`（`lib.rs:1953`） |
| 6 | `bootstrap.rs:150-166`（`admit_start`/`commit_start`），失败分类 R-1 `:104-133`、先停再补偿 `:168-194` |
| 7 | `core-api/src/lib.rs:2223-2300`（两个 handler 都不再碰 `store.set_desired_running`） |

**验收用例（已落地）**：`stale_endpoint_record_writeback_keeps_desired_running`（`config-store/src/lib.rs:4345`，单测：读出 `rec`（`desired=true`）→ `set_desired_running(false)` → 用旧 `rec` 调 `update_endpoint_with_secrets` → 断言 DB 仍为 `false`）与 `put_endpoint_does_not_clobber_desired_running`（黑盒：`stop_lifecycle.rs:1129`）。

**"旧对象写回不再覆盖意图"的保证链（三层，缺一不可）**：

1. **SQL 层（主保证）**：UPDATE 不含该列 ⇒ 写回**物理上**无法触及意图，不依赖调用方自觉。
2. **类型层（建议）**：更彻底的做法是引入 `EndpointMetaUpdate`（不含 `desired_running`）作为这两个 store 函数的入参，使"用旧对象写回"在**编译期**不可表达。**待确认**是否接受这个 churn（§11-9）。
3. **门层**：`EpGate.desired` 是内存权威；每次 `set_desired_running` 成功后 `desired_seq += 1`，观察者用它证明"读到的是门写的值"，而不是某次 PUT 回写的旧值。

**验收判据（可执行）**：`stale_endpoint_record_writeback_keeps_desired_running`（config-store 单测，§9）—— 读出 `rec`（`desired=true`）→ `set_desired_running(id,false)` → 用旧 `rec` 调 `update_endpoint_with_secrets` → 断言 DB 仍为 `false`。该用例在旧代码上**必红**（W-a），是本条的主门。

#### 6.10.3 Stop 意图落盘失败时的行为（显式失败 + 安全方向 + 禁止不受控重启）

**已消灭的旧语义（修复前 `fda7633`）**：`let _ = set_desired_running(false)`（`lib.rs:2171`、`:2180`）⇒ 落盘失败仍返回 200 ⇒ 重启后 `desired_running` 仍为 `true` ⇒ **自动拉起一个用户明确要求停止的端点**。现在落盘失败一律 500 `STOP_INTENT_NOT_PERSISTED` + 持久化围栏（S2），见上表。

| 要求 | 落地 |
|---|---|
| **显式失败** | `admit_stop` 记录 `gate.intent_unpersisted=true`；Stop 最终响应为 **500** + `code=STOP_INTENT_NOT_PERSISTED`（**即使 teardown 成功也不得返回 200**，见 §6.4 末条），并 `tracing::error!` 带 `endpoint_id` 与 DB 错误 |
| **继续执行安全方向的停止处理** | 落盘失败**不阻止** `begin_stop` 与 teardown：停下来的安全收益与 DB 无关。**禁止**因为"意图没写进去"就跳过停止 |
| **禁止不受控重新启动** | `intent_unpersisted=true` 期间 `commit_start` 一律返回 **503 `INTENT_UNPERSISTED`**（§6.4 步骤 0）——不接受新的 Start，直到意图补写成功。补写时机：teardown 结束后的 finalize（同步重试一次）与下一次 Stop（再写一次）；补写成功后清标记并 `desired_seq += 1` |

**重启恢复的残余风险：已从"不消除"改为"已消除"（v3 更正）**

初稿（v2）在此写的是"本设计不能消除它，因为 DB 是唯一持久真值"。**这个结论已被实现推翻**，现在的机制是三层叠加，且都不新增表列（§10-4 仍然成立）：

| 层 | 机制 | 代码 |
|---|---|---|
| 1. 负向证据（S2 围栏） | 落盘失败即写"停止意图未持久化"围栏，**双通道**（DB `meta` 键 `stop_intent_unpersisted` + DB 同目录 `<db>.stop-fence.json`，原子 rename + fsync）。任一通道成功即可跨崩溃 ⇒ 重启时该端点**一律不自动恢复** | `lifecycle.rs:881-906`；`config-store/src/lib.rs:342`、`:349`、`:1694-1742`、`:1748-1779`、`:1786-1795` |
| 2. 默认值翻转 + 正向凭证 | 开机恢复从"没围栏就恢复"改为"**能证明才恢复**"：需 DB 可写 + 围栏可读 + `<db>.intent-proof.json` 里该 id 被确认为 `true`；`set_desired_running` 成功且 `n>0` 才写凭证 | `config-store/src/lib.rs:359-374`、`:708`、`:739-767`；`apps/mesad/src/main.rs:140-208` |
| 3. 一期产品约束（P0-C） | `sinumerik-nck` 端点**任何情况都不自动恢复**，与凭证/围栏/`desired_running` 无关；必须显式 `POST /start` | `core-api/src/recovery.rs:29-37`；`apps/mesad/src/main.rs:184-198`（§6.15） |

**仍然诚实的边界（不是"已知无法消除"，而是"已按 fail-closed 处理"）**：

- 围栏**两条通道都写不进去**、或 sidecar 读不懂、或 DB 只读 ⇒ `recovery_plan()` 返回 `DenyAll` ⇒ **本次进程一个端点都不自动恢复**（宁可少恢复，不可误拉起机床端点）：`config-store/src/lib.rs:739-767`、`:1694-1742`（两通道都失败才 `Err`）、`:4584` 附近单测 `corrupt_sidecar_is_an_error_not_empty_fence`。
- `set_desired_running` 写成功但**凭证写失败**（文件不可写）⇒ 下次开机该端点不恢复（安全方向），需人工 `POST /start` 一次。
- 升级迁移：升级前已在运行的端点没有凭证 ⇒ 升级后**首次重启不会自动恢复**，需现场逐台显式 `POST /start`（`recovery.rs:18-26` 的迁移语义注释）。
- 真正无法在本设计内消除的只剩"两通道同时不可写 + 进程崩溃"这一种，且它导致的是**不恢复**（安全方向）而不是误拉起。

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

**落地位置（`9611255`）**：`current_gen` 见 `manager.rs:804-815`（同锁取"条目代次 or `last_completed_gen`"）；`NoGenBaseline` 判定见 `lifecycle.rs:999-1004`，R-2 的"本次落盘已确认"见 `:1005-1009`。

---

### 6.12 P0-D：删除与在途运行意图写入的竞态（**初稿完全没有，v3 新增**）

### 6.12.1 问题：**清空门禁记录 ≠ 后台任务已完成**

初稿把 DELETE 的语义写成"删行 + 清空在途标记"，实现早期也确实有一个 `invalidate_starts_for_delete()`，里面直接 `stop_persist_inflight.clear()`（该函数的痕迹见单测注释 `lifecycle.rs:1423-1428`：`finish_delete_never_hides_inflight_intent_writes` 就是为钉死这一点而写）。这个写法把"**记录已清空**"误当成"**写库已完成**"，于是能构造出五步交错：

```text
1. Stop-A 受理 → teardown 完成（attempt 条目已被移除）→ desired=false 仍在写库（DB 慢）
2. DELETE 到达：只看 attempt 表 ⇒ "没有活跃 attempt" ⇒ 删行 + 清空 stop_persist_inflight
3. CREATE 以同一 Endpoint ID 重建 → Start 写入 desired=true（新记录）
4. Stop-A 的迟到 SQL 此时才返回，命中的是**另一条记录**
5. 结果：新实例实际 RUNNING，DB 却是 false（下一次重启不会自动恢复；停止判据也失真）
```

**为什么单靠墓碑代次挡不住**（这是本条最容易写错的地方）：墓碑只挡"**写入代次 ≤ 墓碑**"。而窗口内**新受理的 Stop 会取得更大的新代次**（`admit_stop` 一律 `stop_gen += 1`），它的落盘任务完全可能排在"重建 + Start 写 `true`"**之后**执行——代次比墓碑大，墓碑门放行，`false` 落到新记录上。⇒ **必须再加一道"不允许窗口内出现新写入"的屏障**，代次与屏障缺一不可。

### 6.12.2 修法：屏障 → 有限等待 → 串行器复查 → 删行 → 立墓碑

DELETE handler 现在严格按序（`core-api/src/lib.rs:2139-2208`）：

| 步 | 动作 | 语义 | 代码 |
|---|---|---|---|
| 0 | `has_live_attempt(id)` ⇒ 409 `CONFLICT` | 有活 attempt（含拆解中/隔离态）时不允许删配置（P0-7 原判据） | `lib.rs:2130-2138` |
| 1 | `begin_delete(id)` → `DeleteGuard` | **屏障**：置 `deleting=true`，此后 `commit_start` 一律 409 `ENDPOINT_DELETED`；但 **Stop 的意图持久化照常进入**（§6.13） | `lifecycle.rs:604-614`；`lib.rs:2147` |
| 2 | `await_intent_quiescent(id, remaining)` | **有限等待**：等 `stop_persist_inflight` 为空**且** `start_persist_inflight == 0`。预算 `DELETE_PERSIST_WAIT = 10s`，采样 `DELETE_PERSIST_POLL = 5ms`；超时 ⇒ 409 `DELETE_PERSIST_IN_PROGRESS`（不删行、只撤屏障） | `lifecycle.rs:643-665`；`:55`、`:58`；`lib.rs:2157-2163` |
| 2' | 复查 `has_live_attempt(id)` | 屏障期仍可能受理 Stop（`admit_stop` 与门锁同临界区，会同步 `begin_stop`）⇒ 一旦出现活 attempt 同样不得删 | `lib.rs:2164-2175` |
| 3 | `lock_intent_persist_if_quiescent(id)` | **删行前的最后复查**：在**意图写库串行器**上确认"此刻没有写在执行、也不会有写插到删行之前"。返回 `None` ⇒ 回到步骤 2 重试（**总预算仍是 10s，未延长**） | `lifecycle.rs:681-697`；`lib.rs:2176-2180` |
| 4 | `store.delete_endpoint` + `snapshot.remove_endpoint(id, gen_floor)` + 清围栏 | 删行、立快照墓碑 | `lib.rs:2182-2192` |
| 5 | `DeleteGuard::commit()` | **立墓碑代次**：`stop_gen += 1`，复位 `desired`/`stop_persist_confirmed_gen`/`intent_unpersisted`/`deleting`；**计数非 0 时保留 + `error!`（fail-closed）** | `lifecycle.rs:715-731`；`lib.rs:2196` |
| — | `drop(intent_lock)` | 串行器一直持有到"删行 + 立墓碑"之后才释放 | `lib.rs:2205-2207` |

**`finish_delete` 为什么改成"条件复位"（fail-closed）**：`await_intent_quiescent` 已证明计数为 0，走到这里理论上不会有在途写入；万一前提被破坏（未来重构 / 统计漏减），**保留计数**会让重建后的同 ID 端点被 409 `STOP_PERSIST_IN_PROGRESS` 拦住，而不是让旧写入落到新记录上——方向是安全的。因此：

```rust
// lifecycle.rs:715-731
pub fn finish_delete(&self, endpoint_id: &str) {
    let g = ...;
    if !g.stop_persist_inflight.is_empty() || g.start_persist_inflight != 0 {
        tracing::error!(…, "删除完成时仍有在途运行意图写入（不应发生）；保留计数以禁止该 id 被重新启动");
    }                                   // ← 只告警，**不清零**
    g.stop_gen += 1;                    // 墓碑代次（复用既有代次机制，不新造一套）
    g.stop_persist_confirmed_gen = None;
    g.intent_unpersisted = false;       // 记录行已不存在，围栏没有保护对象
    g.desired = None;                   // 重建后懒加载自 DB
    g.deleting = false;
}
```

**`stop_gen += 1` 作为墓碑的复用逻辑**：删除前 admit 的 Start 票据、删除前受理的 Stop 代次，**永远**匹配不上重建后的实例；`finish_stop_persist` 的 `latest` 判定（`g.stop_gen == stop_gen`）同样不再成立，因此旧代次即使归并也不会改写新实例的内存意图。这与 §6.5 的快照墓碑是**两层**：快照 `retired` 管"状态写入不得复活已删端点"，门里的 `stop_gen` 管"意图写入/票据不得作用到新实例"。

### 6.12.3 为什么 P0-A 的集合语义是前提

如果门只记"最新在途代次"（实现早期就是这样），Stop-B 完成时会把标记清空 ⇒ 新 Start 放行 ⇒ Stop-A 的迟到 `false` 覆盖 `true`。所以 `stop_persist_inflight` 是 **`BTreeSet<u64>`**：集合非空期间新 Start 一律不得提交，每个落盘任务只移除**自己**的代次：

- 门闸：`lifecycle.rs:378-380`（快路径）、`:398-408`（二次判定）、`:1064-1072`（`STOP_PERSIST_IN_PROGRESS` 的创建）；
- 归还：`finish_stop_persist` 成功/失败**都**移除自己的代次（`:836`、`:864`），否则一次陈旧失败会永久禁止 Start；
- 只让**最新代次**推进门状态（`:839-846`、`:865-870`）。

**效果**：Stop-A 落盘未确认期间，Start 得到 409 `STOP_PERSIST_IN_PROGRESS`，且 DB 仍是旧值——这正是"不能放行"的证明（端到端用例 `late_stop_a_write_cannot_override_new_start`，`stop_lifecycle.rs:1722`）。

### 6.12.4 锁序与死锁

`persist_lock`（串行器）→ `gate` 锁 → `manager` 锁，**永不反向**：

- `lock_intent_persist_if_quiescent`：先取串行器、**再**取门锁（与落盘任务同向，`lifecycle.rs:679-697`）；
- `await_intent_quiescent` 只取门锁、`await` 期间**不持锁**（`:649-664`）；
- 删行（`store.delete_endpoint`）在串行器保护下、**不持门锁**执行，因此 DB 慢不会阻塞 Start/Stop 的门操作。

### 6.12.5 已知边界（诚实声明）

1. **DELETE 与 Stop 的队头**：`DELETE_PERSIST_WAIT` 是**总预算**。如果屏障期反复有 Stop 登记（现场疯狂重试 Stop），DELETE 会在 10 s 后返回 409，客户端需重试——这是有意的（删除不可逆）。
2. **"删除成功"不保证"没有别的写者"**：串行器只覆盖 `desired` 写库；如果将来新增别的意图列，必须同样纳入串行器与计数，否则本条的前提失效（代码注释已写明 `persist_lock` 是"所有 `desired` 写库的必经关卡"）。
3. **DELETE 期间 Start 的拒绝码是 `ENDPOINT_DELETED`（不是 409 `CONFLICT`）**：因为它等价于"该端点正在消失"，客户端应按删除语义处理（`lifecycle.rs:482-491`）。

---

### 6.13 P0-E-1：删除屏障期 Stop **必须照常落盘**（**初稿/早期实现写错，v3 更正**）

### 6.13.1 被删除的错误做法

早期实现在 `admit_stop` 里有一个"屏障短路"分支：`deleting == true` 时**跳过 `set_desired_running(false)`**，却仍然返回一个"立即 Ok"的占位落盘任务。后果是 Stop 返回 200，而 DB 里的 `desired_running` 可能仍是 `true`。只要 DELETE 随后失败（409 `DELETE_PERSIST_IN_PROGRESS` / 404 / 存储错误 / handler 取消），端点就**仍然存在**且"持久意图 = 运行"，而调用方拿到的却是"停成功了"——**Stop 虚假成功**。

### 6.13.2 现在的做法：与正常路径**完全同源**

```rust
// admit_stop（lifecycle.rs:744-816）：**没有** deleting 的短路分支
g.stop_gen += 1;                                   // 1) 作废在途 Start 票据
g.desired = Some(false);                           // 2) 内存意图前移
g.stop_persist_inflight.insert(stop_gen);           // 3) 登记在途代次（屏障期一样登记！）
g.stop_persist_confirmed_gen = None;                // 4) 本次未确认
let h = mgr.begin_stop(id);                         // 5) 同步开始拆解
// 6) 独立任务：串行器 → spawn_blocking 写 false → finish_stop_persist
```

**设计原则（写死，不允许再"优化"回去）**：

1. **停止是安全方向，不得把 Stop 变成可失败的 409**。若"有人在删这个端点"就拒绝 Stop，等于把"让机器停下来"这个动作交给客户端重试——方向不安全。
2. **200 必须等价于「停止意图已落盘 **或** 行已确实不存在」**。后者由写入命中 0 行自然满足（`finish_stop_persist` 按"端点确已消失"收尾），不需要专门分支。
3. **屏障只挡 Start**（`check_start_preconditions` 的 `deleting` 判定），**不挡 Stop 的意图持久化**。

### 6.13.3 连带要求：删行前必须**再复查一次**（否则 P0-D 的窗口重新打开）

屏障期 Stop 现在会真实落盘 ⇒ "`await_intent_quiescent` 观测为空"与"删行"之间仍可能插入一次新的登记。因此 DELETE handler 每轮都在**取得意图串行器后**再复查（`lock_intent_persist_if_quiescent`），不干净就重试：

- 串行器是所有 `desired` 写库的**必经关卡**：持有它就保证"既没有写正在执行，也不会有写插到删行之前"；
- **总预算不变**：重试共用同一个 `DELETE_PERSIST_WAIT = 10s` 截止时间（`lib.rs:2157-2181`），**没有**因为多一次复查而延长；
- 串行器持有到"删行 + 立墓碑"之后才释放（`lib.rs:2205-2207`）。

### 6.13.4 验收

| 用例 | 断言 |
|---|---|
| `barrier_stop_confirms_intent_before_returning_success_even_if_delete_fails`（`stop_lifecycle.rs:2588`，端到端） | DELETE 因注入 12s 慢写停在屏障里；屏障期 Stop-B **不得在意图落盘前返回**（旧码此处必红）；DELETE 409 `DELETE_PERSIST_IN_PROGRESS` 且行仍在；Stop-B 最终 200 且 DB `desired=false` |
| `barrier_stop_confirms_intent_so_delete_failure_cannot_leave_desired_true`（`lifecycle.rs:1471`，单测） | 屏障期 Stop 的 `persist` 返回 `Ok` 之后，DB 必须已是 `false`（P0-E-1 的确定性主门） |
| `barrier_stop_persist_failure_is_not_reported_as_success`（`lifecycle.rs:1531`） | 屏障期落盘失败 ⇒ `intent_unpersisted=true` + 写围栏 + 代次归还，handler 侧 500 `STOP_INTENT_NOT_PERSISTED` |
| `delete_barrier_blocks_new_start_but_still_persists_new_stop`（`lifecycle.rs:1351`） | 屏障期 Start 409 `ENDPOINT_DELETED` 且不留 attempt；**同一屏障期内**的 Stop 照常落盘（DB 由 `true` 变 `false`） |

---

### 6.14 P0-E-2：`DeleteGuard` RAII——**任何非正常退出都不留 `deleting=true`**（**初稿没有，v3 新增**）

### 6.14.1 问题

初稿/早期实现靠"调用方在每个失败分支显式调用 `abort_delete()`"。DELETE handler 在 `await_intent_quiescent` 上等待期间可能被**取消**（客户端断开 / 网关超时 ⇒ handler future 被丢弃）或 **panic**，此时后面那些显式 `abort_delete()` 语句根本执行不到 ⇒ `deleting` 永久为 `true` ⇒ 该端点此后**所有 Start 都被 409 `ENDPOINT_DELETED` 拒绝，只能重启 Core 才能恢复**（一次性超时把端点变成不可用）。

### 6.14.2 设计

```rust
// lifecycle.rs:221-263
pub struct DeleteGuard { gate: Arc<LifecycleGate>, id: String, committed: bool }

impl DeleteGuard {
    /// 只在 store.delete_endpoint 返回 Ok(true) 之后调用：立墓碑代次并复位门状态
    pub fn commit(mut self) { self.gate.finish_delete(&self.id); self.committed = true; }
}

impl Drop for DeleteGuard {
    fn drop(&mut self) {
        if self.committed { return; }
        // panic 展开期间门锁可能已被毒化：抢不到锁就放弃本次撤销，**绝不二次 panic**
        let Ok(mut m) = self.gate.inner.lock() else { return; };
        if let Some(g) = m.get_mut(&self.id) { g.deleting = false; }
    }
}
```

| 退出路径 | 谁负责撤销 | 证据 |
|---|---|---|
| 正常成功（行已删） | `commit()`：立墓碑 + 复位（不撤销"未提交"语义） | `lifecycle.rs:242-245`；`lib.rs:2196` |
| 等待超时 / 409 / 404 / 存储错误 | 提前 `return` ⇒ guard 出作用域 ⇒ `Drop` 撤销 | `lib.rs:2161-2163`、`:2167-2175`、`:2199-2203` |
| **panic 展开** | `Drop` 撤销；Drop 内用 `let Ok(..) else return` **避免 panic 期间二次 panic**（`unwrap()` 会让进程 abort） | `lifecycle.rs:248-263` |
| **future 被丢弃（取消）** | `Drop` 撤销（guard 是 future 栈帧的一部分） | 同 Drop |

**为什么用 RAII 而不是"每个分支补一句"**：Drop 在**三条**非正常退出路径上都会执行，补语句的方式会随分支增长不断漏；撤销是**幂等**的（重复撤销只是把 `true` 写成 `false`）。

**语义边界（重要）**：`abort_delete`（= Drop 路径）**刻意不动**在途计数、`intent_unpersisted`、`stop_persist_confirmed_gen`——删除没有发生，端点仍然存在，这些状态对"是否允许启动 / 是否停干净"仍然有效，清掉等于放宽保护（`lifecycle.rs:616-629` 的注释）。

### 6.14.3 验收

| 用例 | 断言 |
|---|---|
| `delete_guard_drop_without_commit_clears_barrier`（`lifecycle.rs:1570`） | 未提交即丢弃 ⇒ 屏障撤销，后续 Start 不再被 `ENDPOINT_DELETED` 拒绝 |
| `delete_guard_survives_handler_panic`（`:1596`） | `catch_unwind` 内 panic ⇒ 屏障仍被撤销（旧码的显式 `abort_delete` 在展开路径执行不到） |
| `delete_guard_is_cleared_when_handler_future_is_cancelled`（`:1617`） | 任务停在 `pending()` 时被 `abort()` ⇒ 屏障撤销 |
| `cancelled_delete_request_does_not_leave_a_permanent_barrier`（`stop_lifecycle.rs:2715`，端到端） | 真 drop 掉 DELETE future ⇒ 屏障撤销、行仍在、随后 **Start 必须能成功**（旧码永久 409） |

---

### 6.15 P0-C：828D 一期开机不自动恢复（**产品约束，不是通用生命周期语义**）

初稿把"重启后误拉起"只当成 §6.10.3 的残余风险。828D 一期评审给出了更强的产品决策：**`sinumerik-nck` 端点在 Mesad 开机时一律不自动恢复**，必须由现场显式 `POST /start`。v3 把它写成独立契约。

### 6.15.1 实现

| 组件 | 内容 | 代码 |
|---|---|---|
| 常量 | `pub const NCK_DRIVER_ID: &str = "sinumerik-nck";` | `crates/core-api/src/recovery.rs:29` |
| 判定函数 | `pub fn boot_auto_restore_allowed(driver_id: &str) -> bool { driver_id != NCK_DRIVER_ID }` ——**精确匹配**，不做前缀/大小写归一 | `recovery.rs:35-37` |
| 开机循环 | 在**正向凭证判定之前**直接 `continue` 并 `warn!`（凭证、围栏、`desired_running` 都拦不到它） | `apps/mesad/src/main.rs:184-198` |
| 单测 | `nck_is_never_auto_restored`（`recovery.rs:46`）、`other_drivers_keep_existing_recovery_behavior`（`:53`，含 `sinumerik-nck-sim` / `SINUMERIK-NCK` 必须**允许**） | 同左 |
| 端到端 | `nck_endpoint_is_never_auto_restored_at_boot`（`tests/driver-contract/tests/mesad_restart.rs:790`）：构造"`desired_running=true` + 无围栏 + 正向凭证确认"的最有利现场，同库 test-driver 对照组必须被恢复、NCK 必须**从未出现运行态条目**；DB 旧值不变；显式 `POST /start` 必须成功 | 同左 |

### 6.15.2 定位与边界（必须写清，避免被当成通用语义）

1. **这是产品约束，不是生命周期语义**：它回答"即使能证明用户意图，该驱动是否允许在开机时自动拉起"，与 §7 的"停干净判据"无关。
2. **放 Core 而不是 Driver**：开机恢复是 Core 的编排行为；Driver 只应描述"我是什么、怎么连"，不该持有"Core 该不该在启动时拉起我"的策略。策略只有一个判定位点（`recovery.rs:13-16` 的注释即此论证）。
3. **只约束开机恢复循环**：REST Start / Bootstrap 是**显式**启动，不受影响（同一注释 `:15-16`；端到端用例在重启后 `POST /start` 必须 200）。
4. **其他驱动行为不变**：判定函数对其他 `driver_id` 恒为 `true`，仍交给既有"围栏 + 正向凭证"判据。
5. **不改写用户意图**：产品策略只阻止自动恢复，不回写 `desired_running`（端到端用例断言 DB 旧值仍为 `true`）。
6. **关联文档**：`docs/design/nck-828d-phase1-boot-recovery-policy.md`（独立文档，本文**只引用不改动**）。

---

## 7. 完整停止判据六条（**设计原编号 P0-C**；实现侧 `P0-C` 是 828D 产品约束，见 §6.15）

### 7.1 判据总表

"停干净"= 下列 6 条**同时**成立，并在稳定窗口内持续成立。逐条给出可判定方式、抗旧值机制、当前代码是否具备。

| # | 判据 | 可判定信号（来源） | 如何证明不是旧值 | 当前代码 | 需新增什么 |
|---|---|---|---|---|---|
| 1 | **当前代次为 `STOPPED`** | `Snapshot` 中 `(gen, state)` 原子对（`snapshot.rs:285-292` 的 `endpoint_with_gen`），且 `gen == current_gen(id)`（条目代次优先，回退 `last_completed_gen`，§6.11） | 写入按代次过滤（旧代次被丢弃）；读取在**同一次加锁**内取 `(gen, state)`，不可能拼出"新 gen + 旧 state" | **已具备**（G7）：`EndpointRecord{generation,status}` + 代次门 `snapshot.rs:238-260` + 单锁墓碑 `:495-525`；`last_completed_gen` `manager.rs:482-492` | —— |
| 2 | **`desired_running=false` 且**本次 Stop 的落盘已确认 | `store.get_endpoint(id).desired_running`（`config-store/src/lib.rs:1566-1585`）+ 门的 `stop_persist_confirmed_gen`（R-2） | **不能只比 `desired_seq`**：`desired_seq` 在落盘**开始前**就可能满足 ⇒ 慢写时会在 DB 仍是 `true` 时误判成功。判据改为"`stop_persist_confirmed_gen == 本次 stop_gen` 且 `stop_persist_inflight` 为空"，二者只在 UPDATE 真正返回后推进（`lifecycle.rs:839-846`）；"单写者"由 §6.10 保证 | **已具备**（G7）：写点前移到 `admit_stop` 同步临界区 `lifecycle.rs:770-777`；确认凭证 `:125-130`；唯一写点 `config-store/src/lib.rs:1617-1670` | —— |
| 3 | **无在途 Start / Stop / 意图落盘** | (a) 在途 Start = 门 `inflight_starts`；(b) 在途 Stop（生命周期）= attempt 表存在且 `phase ∈ {Stopping, FailedUnknown}`；(c) 在途 Stop（**意图落盘**）= `stop_persist_inflight` 集合非空（P0-A）；(d) 在途 Start 意图落盘 = `start_persist_inflight != 0`（P0-D） | 四个来源都是"在途"的**定义本身**（不是缓存副本）；集合语义保证"任意一个代次未确认"都算在途 | **已具备**（G7）：`lifecycle.rs:99-153`；attempt 表 `manager.rs:786-792` | —— |
| 4 | **无未完成的 attempt（含孤儿）** | attempt 表：该 `endpoint_id` 无条目（条目 = 未完成；正常完成的条目在**同一临界区**内先写 `last_completed_gen` 再移除） | 条目**只**在 spawn 前创建、**只**由 attempt 任务自身按 gen 匹配移除（且异常终止**不**移除，§6.3.2）⇒ 不存在"漏登记"的孤儿，也不存在"panic 后被当作已完成"；gen 单调，读到的 gen 与当前 gen 比对 | **已具备**（G7）：`manager.rs:901-978`（`Entry::Vacant` insert-before-spawn）、`:521-563`（Drop 移除/隔离） | —— |
| 5 | **对应驱动进程与目标 TCP 会话已收敛** | (a) `TeardownDone(gen)`（§6.3.1，蕴含 `child.wait()` 已返回）；(b) `session_released == true`（teardown 末尾 registry 中该 `endpoint_id` 键不存在）；(c) 全局 `LIVE_CHILDREN`（`process.rs:30`） | (b) 因并发 attempt 被拒绝（§6.6），registry 的该键在任一时刻只可能属于当前 attempt ⇒ 不存在"读到上一代"；再叠加 gen 单调比对 | **部分具备**：进程收敛已由 teardown 保证（E3）且 `LIVE_CHILDREN` 已落地；`wait_reader_done` 的结果仍只用于一条 warn（F20，未上报给调用方）——**这一项仍是已知缺口** | `AttemptCtx.session_released/remote_eof/pid` 已存在；缺口是"把 reader 结果纳入判据"（未做，见 §11-13） |
| 6 | **经过规定的稳定观察窗口** | `gate.await_settled(id, timeout)`：`SETTLE_TICKS = 2` × `SETTLE_TICK = 250ms`（`lifecycle.rs:42-43`），每个采样同时读 1~5 条全部来源。**这是 Core 内部快速收敛检查，不是现场判据**（§7.5） | 每 tick 重读全部来源并记录 `gen`；窗口内任一条目/代次/phase 变化即**重置窗口**；旧代次的 STOPPED 写入因代次门被丢弃，即使落盘其 gen ≠ 当前 gen ⇒ 该 tick 不通过 ⇒ 窗口重置 | **已具备**（G7）：`lifecycle.rs:986-1046` | —— |

**判定式**：

```
Settled(id) ⇔ ∧_{k=1..6} Crit_k(id) 在每个采样 tick 成立，且连续 N=2 个 tick 全部成立
```

**禁止的等价替换**：不得用 `is_running()==false`、`desired_running==false`、`state=="STOPPED"` 中的任一条单独（或任意组合）代替上面 6 条——它们分别缺"代次"、"事实性"、"抗覆盖"。

### 7.2 第 4 条（孤儿）如何避免"快照被旧 attempt 覆盖"的干扰

判据 4 **完全不读快照**，只读 attempt 表。这是刻意的分层：

- 快照是"呈现层"，会被 `remove_endpoint`（`snapshot.rs:495-525`，现在立墓碑）、被旧代次写入（F10，现已按代次丢弃）、被 DELETE 抹掉；用它判"有没有未完成的 attempt"必然受污染。
- attempt 表是"生命周期层"，条目与"活着的任务"一一对应（insert-before-spawn + 自删），**没有"旧条目覆盖新条目"这种状态**——同名条目的并发创建被拒绝（§6.6），所以表里永远只有一个当前 attempt。
- 孤儿（复审特别强调的一类）在表里表现为"条目存在但对应 endpoint 已不该运行"；判定上它与普通在跑 attempt 无差别（都算"未完成"），因此**不会漏**。唯一漏网是 F22 的直调路径，已显式声明为边界。

### 7.3 第 6 条（稳定窗口）如何避免"快照被旧 attempt 覆盖"的干扰

窗口的每个 tick 是一次**跨来源的一致性快照**，规则如下：

1. 同一次加锁内读 `(snapshot.gen, snapshot.state)`（§6.5 的原子对）；
2. 读 `current_gen(id)`（条目代次优先，回退 `last_completed_gen`，§6.11）与 `phase`；
3. 要求 `snapshot.gen == current_gen(id)`，否则该 tick **直接不通过**（这正是"旧 attempt 的 STOPPED 覆盖"的检测器：被覆盖时 gen 会对不上）；`current_gen == None` ⇒ `NotSettled{NoGenBaseline}`（无基准 ≠ 成立）；
4. 要求该 `endpoint_id` 在 attempt 表中**无条目**（即 `entry_absent`）；等价地 `phase` 不得是 `Starting/Running/Stopping/FailedUnknown`；
5. 要求 `desired == Some(false)`，且 **R-2**：`stop_gen == target_gen`（进入判定时的最新 Stop 代次）、`stop_persist_confirmed_gen == Some(target_gen)`、`stop_persist_inflight.is_none()`（集合为空），且 `intent_unpersisted == false`（§6.10.3：意图没落盘就不能算"停干净"）。**不得**用 `desired_seq >= seq_at_admit` 代替——那个值在落盘开始前就可能满足（R-2 的动机）；
6. 要求 `inflight_starts == 0`；
7. 任一 tick 失败或窗口内出现 gen 变化 ⇒ 窗口从 0 重新计时。

因此"快照被旧 attempt 覆盖成 STOPPED、而实际在采集"这个具体故障，**不可能**让判据 6 通过：要么 gen 对不上（第 3 步），要么条目还在（第 4 步），要么 `inflight_starts>0`（新 Start 在途，第 6 步）。

**窗口参数（内部快速收敛检查）**：`SETTLE_TICKS = 2`、`SETTLE_TICK = 250ms`（合计 500 ms）。选值理由：需大于一次"状态写入 + 一次 REST 读"的抖动（ms 级），又必须远小于 Stop 的 teardown 预算（≈35 s），避免把内部判定拖成运维不可接受的时长。**待确认**是否需要按 endpoint 配置（§11-6）。

**这条 500 ms 检查的作用域必须写死（P1-5）**：它只用于 **Core 内部状态收敛判定**（测试断言、日志水位、诊断）。**它不是现场"停干净"判据，也不能替代现场的 10 s 连续稳定观察**——两者的采样对象、目的与失败代价都不同，见 §7.5。

**只读纪律（硬性）**：观察器只读、只记日志与水位，**绝不**写 `desired_running`、**绝不** cancel/restart attempt、**绝不**改快照。一旦它开始"纠正"，就变成 §10-1 明确拒绝的对账器/看门狗。

### 7.4 判定算法（伪代码）

```rust
// 实际实现：lifecycle.rs:986-1046（`await_settled`）
async fn await_settled(&self, id: &str, timeout: Duration) -> Settled {
    let target_gen = /* 进入判定时的最新 stop_gen */;        // :988-991（R-2：只认这一次的落盘确认）
    let deadline = Instant::now() + timeout;
    let (mut ok_ticks, mut last_gen) = (0u32, None);
    loop {
        let s = self.sample(id);                              // :922-977 一次取全部来源
        let Some(cur) = s.current_gen else {
            return Settled::NotSettled { reason: "NoGenBaseline: …".into(), sample: s };  // P0-3：无基准不猜
        };
        // R-2：本次停止意图落盘已确认，且没有更新的 Stop 在途
        let intent_confirmed = target_gen > 0
            && s.stop_gen == target_gen
            && s.stop_persist_confirmed_gen == Some(target_gen)
            && s.stop_persist_inflight.is_none();
        let same_gen = last_gen.is_none_or(|g| g == cur);
        let pass = s.snapshot_gen == Some(cur)
                && s.snapshot_state.as_deref() == Some("STOPPED")
                && s.entry_absent                              // 含隔离态：FailedUnknown 也算有条目
                && s.desired == Some(false)
                && intent_confirmed
                && !s.intent_unpersisted
                && s.inflight_starts == 0;
        ok_ticks = if pass && same_gen { ok_ticks + 1 } else { 0 };
        last_gen = Some(cur);
        if ok_ticks >= SETTLE_TICKS { return Settled::Settled { generation: cur }; }
        if Instant::now() >= deadline {
            return Settled::NotSettled { reason: /* 附全部最近观测值，供现场定位 */ …, sample: last_sample };
        }
        tokio::time::sleep(SETTLE_TICK).await;
    }
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
- 否决理由 3：持锁跨 80 s teardown，任何未来的重入都会变成死锁；且会改变 Stop 的延迟特性，冲击 `stop_lifecycle.rs:88`/`:182`（40 s 有界、栅栏期必须阻塞）与 `reconnect_backoff.rs` 的既有 Gate。
- 否决理由 4（本轮新增）：锁在 handler 栈帧上 ⇒ 客户端断开即释放 ⇒ 与 P0-A 同类漏洞。

### 方案 C：时间戳/行版本墓碑（Stop 记 `last_stop_ns` 或 `updated_at_ns`，Start 提交前比对）

- 改动面小，无需新组件，复用 `endpoints.updated_at_ns`（`config-store/src/lib.rs:1186`）。
- **否决理由 1**：判据与"spawn"无法原子化——即使 `UPDATE ... WHERE updated_at_ns=?` 的 CAS 成功，Stop 的置假仍可落在 CAS 之后、spawn 之前/之后，矛盾依旧（W1 只被"概率性"缩小）。
- 否决理由 2：无法表达"Stop 优先"（没有"Stop 在途"这一状态），拆解期到达的 Start 仍会成功（W2 完全不覆盖）。
- 否决理由 3：时间/版本是跨线程弱序证据，测试无法做"旧码必红"的确定性门；且任何无关的行更新（改名、改连接）都会误伤 Start。
- 结论：只能作为**辅助审计字段**，不能作为修复主体。
- **实现印证（v3）**：G7 最终没有在 handler 栈帧上挂任何锁或计数器——保护挂在 attempt 任务内部（§6.3），意图写入由 per-endpoint 串行器 `persist_lock` 承载且由 blocking 任务持有（§6.4、§6.7），删除侧另加屏障（§6.12）。方案 B 的四条否决理由在实现里都被回避了。

---

## 9. 回归测试（**已落地用例名，按 `9611255` 逐一核对**）

> 本节所有用例名都是**代码里真实存在的函数名**，`cargo test` 可直接运行。原稿（v2）里的"建议用例名"与实际实现的差异见 §9.6；**本节不保留任何虚构用例名**。

### 9.1 稳定复现手段（不赌调度）

1. **W2（Start 落进拆解窗口）确定性复现**：用既有 commit 栅栏 `StoreFaults::arm_commit_gate()` 把 Stop 的 teardown **确定性**挂住（`stop_lifecycle.rs:633`、`:896`、`:1147`、`:1479` 均在使用），此刻发 Start：必得 409，且不得产生 attempt。用例 `w2_start_during_stop_teardown_is_rejected_and_does_not_resurrect`（`:614`）。
2. **W1（Stop 落进构造窗口）不变量复现**：并发发起 Start 与 Stop（不指定谁先），断言不变量"Stop 返回 200 之后端点不处于运行态、`desired_running` 不为真"。用例 `w1_stop_during_start_construction_leaves_nothing_running`（`:521`）。
3. **慢 DB 的确定性交错（本设计的核心手段）**：逐调用延迟队列 `store.desired_write_faults().delay_seq`（`config-store/src/lib.rs:321-337` + `:1622-1654`）可以**精确指定第 N 次 `desired` 写入延迟多少毫秒**，从而确定性构造"Stop-A 慢 / Stop-B 快"（P0-A）、"Start 慢不阻塞 Stop"（P0-B）、"teardown 完成但落盘在途"（P0-D）等交错；`fail` 开关用于注入落盘失败（S2 围栏）。
4. **取消路径确定性复现**：把 Stop 的 future 直接丢弃（等价客户端断开），断言保护仍在（`stop_protection_survives_handler_cancel`，`:880`）；把 DELETE 的 future **真正 drop**（`drop(delete_fut)`，`:2776`）断言屏障撤销（`:2715`）。
5. **算法层确定性单测**：直接对 `LifecycleGate` / `AttemptTable` 做状态机测试（无 HTTP、无时序），见 §9.2。
6. **快照层确定性单测**：`upsert/remove` 并发压测断言不变量（§9.2 的 snapshot 四条）。

### 9.2 门与生命周期单测

> **运行前提**：隔离态与故障注入相关用例依赖 `test-hooks` feature（`MesaManager::set_test_hold_reaper` / panic 注入、`ConfigStore::desired_write_faults`），仓库验收命令本就是 `cargo test --all-features`（见 `AGENTS.md` 与 `scripts/write-contract-evidence.py`）。**待确认**：不带该 feature 时 `crates/driver-manager` 的测试模块能否编译（本轮按约束不跑构建，未实测）。

`crates/core-api/src/lifecycle.rs`（13 个）：

| 行 | 用例名 | 断言要点 |
|---|---|---|
| 1169 | `in_flight_start_superseded_by_stop_admission` | admit_start → admit_stop → commit_start ⇒ 409 `SUPERSEDED_BY_STOP`；无 attempt、`desired` 仍假 |
| 1193 | `newer_stop_completion_does_not_clear_older_inflight_stop` | **P0-A 主门**：Stop-B（最新）先完成不得清掉 Stop-A 的代次；Start 得 409 `STOP_PERSIST_IN_PROGRESS` |
| 1230 | `settled_without_generation_baseline_is_not_settled` | `current_gen==None` ⇒ `NotSettled{NoGenBaseline}`（无证据 ≠ 成立） |
| 1245 | `stop_intent_persist_failure_blocks_start_until_rewritten` | 落盘失败 ⇒ `intent_unpersisted` + 持久化围栏 + Start 503 `INTENT_UNPERSISTED`；补写成功后恢复并清围栏 |
| 1275 | `second_stop_joins_instead_of_failing` | 两次 `admit_stop` 都等同一个结论（合流，不重复拆解） |
| 1294 | `delete_wait_rejects_unconverged_intent_persist_and_passes_after_convergence` | **P0-D**：在途写入未收敛 ⇒ 409 `DELETE_PERSIST_IN_PROGRESS`；收敛后等待立即通过且不引入延迟；`commit()` 推进墓碑代次 |
| 1351 | `delete_barrier_blocks_new_start_but_still_persists_new_stop` | **P0-D + P0-E-1**（改名后的新语义，更严）：屏障期 Start 409 `ENDPOINT_DELETED` 且不留 attempt；**屏障期 Stop 照常把 DB 写成 `false`**；丢弃 guard 即撤屏障 |
| 1430 | `finish_delete_never_hides_inflight_intent_writes` | **P0-D 门级不变量**：`finish_delete` 不得 `clear()` 掩盖在途写入；保留计数 ⇒ 重建后 Start 409（fail-closed）；`stop_gen` 推进到 5 |
| 1471 | `barrier_stop_confirms_intent_so_delete_failure_cannot_leave_desired_true` | **P0-E-1 主门**：屏障期 Stop 成功 ⇒ DB 必须已是 `false`；DELETE 失败（丢弃 guard）后不得为 `true` |
| 1531 | `barrier_stop_persist_failure_is_not_reported_as_success` | 屏障期落盘失败 ⇒ 围栏 + 代次归还 + 无确认凭证（handler 侧 500 由端到端覆盖） |
| 1570 | `delete_guard_drop_without_commit_clears_barrier` | **P0-E-2**：未提交即丢弃 ⇒ 屏障撤销，后续 Start 不再被删除码拒绝 |
| 1596 | `delete_guard_survives_handler_panic` | **P0-E-2（panic）**：`catch_unwind` 内 panic ⇒ 屏障仍被撤销 |
| 1617 | `delete_guard_is_cleared_when_handler_future_is_cancelled` | **P0-E-2（取消）**：任务停在 `pending()` 时 `abort()` ⇒ 屏障撤销 |

`crates/driver-manager/src/manager.rs`（5 个）：`guard_without_commit_quarantines_entry`（:1807，未 commit 的 guard ⇒ 隔离态 + Start 拒绝）、`committed_guard_records_completed_generation`（:1838）、`start_rejected_while_stopping`（:1865）、`generation_is_monotonic_and_floor_is_last_issued`（:1883，钉住 `gen_floor` 的 off-by-one）、`pid_convergence_requires_proof_of_death`（:1898，三态 pid 只有 `Dead` 算收敛）。

`crates/driver-manager/src/snapshot.rs`（4 个）：`stale_generation_write_is_dropped`（:1000）、`deleted_endpoint_write_is_dropped_by_tombstone`（:1018）、`delete_then_recreate_status_write_is_accepted`（:1038）、`concurrent_delete_and_late_status_write_never_resurrects`（:1055，2 线程压测 + 不变量）。

### 9.3 持久化与围栏单测（`crates/config-store/src/lib.rs`，8 个）

| 行 | 用例名 | 断言要点 |
|---|---|---|
| 4345 | `stale_endpoint_record_writeback_keeps_desired_running` | **P0-2 主门**：用旧 `rec` 回写后 DB 仍为 `false` |
| 4378 | `stop_intent_fence_roundtrip` | S2 围栏写入/读取/清除 + 幂等 + 排序 |
| 4400 | `stop_intent_fence_survives_reopen` | 围栏跨进程重开仍在 |
| 4431 | `stop_intent_fence_concurrent_mark_clear_does_not_lose_updates` | 并发 mark/clear 不丢其它端点 |
| 4497 | `fence_sidecar_survives_db_unwritable` | DB 通道不可写时 sidecar 仍留下持久事实 |
| 4563 | `fence_reports_error_when_both_channels_fail` | 两通道都失败 ⇒ `Err`（调用方按"无法证明安全"处理） |
| 4587 | `corrupt_sidecar_is_an_error_not_empty_fence` | sidecar 读不懂 ⇒ 读围栏报错，不当作空集合 |
| 4612 | `recovery_plan_requires_positive_intent_proof` | **P0-4**：默认不恢复，必须有正向凭证 |

### 9.4 端到端（`tests/driver-contract/tests/stop_lifecycle.rs`，27 个）

| 行 | 用例名 | 主题 |
|---|---|---|
| 88 | `stop_while_reconnecting_is_bounded_and_explicit` | 重连退避中 Stop 40 s 内显式返回 |
| 182 | `stop_waits_for_inflight_commit_then_drains_exact` | commit 栅栏期 Stop 必须阻塞，放行后精确 drain |
| 521 | `w1_stop_during_start_construction_leaves_nothing_running` | W1 不变量 |
| 614 | `w2_start_during_stop_teardown_is_rejected_and_does_not_resurrect` | W2 确定性：409 + 不复活 |
| 717 | `w3_double_start_only_one_attempt` | W3：恰好一个 attempt |
| 809 | `teardown_panic_quarantines_and_blocks_start` | P0-1 隔离态 |
| 880 | `stop_protection_survives_handler_cancel` | 丢弃 Stop future 不释放保护 |
| 947 | `stop_intent_persist_failure_is_explicit_and_blocks_restart` | S1/S2 核心：显式 500 + teardown 照做 + 禁止新 Start |
| 1023 | `slow_desired_write_does_not_delay_teardown` | S1：慢落盘不推迟实际停止 |
| 1079 | `stop_is_idempotent_after_completion` | 完成后再次 Stop 仍 200 `was_running=false` |
| 1105 | `start_conflict_while_running_still_409` | 运行中 Start 仍 409（文案不变） |
| 1129 | `put_endpoint_does_not_clobber_desired_running` | P0-2 黑盒：拆解期 PUT 不覆盖 `desired` |
| 1416 | `settled_after_normal_stop_uses_last_completed_generation` | P0-3 主门：正常停止后 `await_settled` 必须通过 |
| 1457 | `live_attempt_outlives_is_running_during_teardown` | 拆解期存在活条目，`is_running` 已假 |
| 1516 | `double_start_only_one_attempt_at_table_level` | 表级：并发两 Start 只有一个 attempt + `LIVE_CHILDREN == 0` |
| 1559 | `deleted_endpoint_not_resurrected_by_late_status_write` | P0-4：墓碑挡迟到状态写入 |
| 1639 | `start_rejected_while_stop_intent_persist_in_flight` | P0-1/P0-A：落盘在途禁止新 Start（防 `desired` 反序） |
| 1722 | `late_stop_a_write_cannot_override_new_start` | **P0-A 确定性主门**：跟踪全部在途代次，A 未确认前 Start 409 |
| 1886 | `slow_start_persist_does_not_delay_stop_begin` | **P0-B 主门**：`begin_stop` <200 ms，Start 响应 ≥3 s |
| 2013 | `await_settled_requires_confirmed_stop_intent_under_slow_db` | **R-2 主门**：慢写下 teardown 已完、状态已 STOPPED，也不得提前判 settled |
| 2100 | `bootstrap_intent_persist_failure_stops_attempt_before_compensating` | **R-1**：先停再补偿，不留孤儿 |
| 2155 | `concurrent_delete_and_delete_during_teardown_are_safe` | 并发/拆解期 DELETE 的安全性 |
| 2234 | `start_on_deleted_endpoint_is_aborted_not_orphaned` | **P0-7**：`Ok(false)` ⇒ 认领停止 |
| 2273 | `delete_racing_start_never_leaves_orphan_attempt` | **P0-7** 并发面 |
| 2335 | `delete_waits_for_inflight_stop_persist_and_rebuild_is_not_polluted` | **P0-D 确定性主门**：DELETE 等写入收敛才删行；屏障期 Start 409；重建后不被污染 |
| 2588 | `barrier_stop_confirms_intent_before_returning_success_even_if_delete_fails` | **P0-E-1 端到端主门** |
| 2715 | `cancelled_delete_request_does_not_leave_a_permanent_barrier` | **P0-E-2 端到端主门** |

### 9.5 开机恢复（P0-C / P0-4）

| 位置 | 用例名 | 断言要点 |
|---|---|---|
| `tests/driver-contract/tests/mesad_restart.rs:790` | `nck_endpoint_is_never_auto_restored_at_boot` | NCK 端点在最有利现场（`desired=true` + 无围栏 + 凭证确认）也**永不**被自动拉起；同库 test-driver 对照组必须被恢复；显式 `POST /start` 成功 |
| `crates/core-api/src/recovery.rs:46` | `nck_is_never_auto_restored` | `boot_auto_restore_allowed(NCK_DRIVER_ID) == false` |
| `crates/core-api/src/recovery.rs:53` | `other_drivers_keep_existing_recovery_behavior` | 其他驱动仍 `true`；`sinumerik-nck-sim` / `SINUMERIK-NCK` 不受影响（精确匹配） |

**合计已登记的实际用例名：60 个**（lifecycle 13 + manager 5 + snapshot 4 + config-store 8 + stop_lifecycle 27 + 开机恢复 3）。

### 9.6 原稿"建议用例名"的去向（**改名/未落地必须显式登记**）

| 原稿建议名 | 现状 |
|---|---|
| `admit_start_rejected_while_attempt_stopping` | 实现为 manager 单测 `start_rejected_while_stopping` + 端到端 `w2_...` |
| `stop_priority_holds_until_teardown_completes` | 由 `w2_start_during_stop_teardown_is_rejected_and_does_not_resurrect` 覆盖，未单独命名 |
| `start_after_stop_completed_succeeds` | 由 `settled_after_normal_stop_uses_last_completed_generation` 的收尾断言覆盖 |
| `two_starts_only_one_attempt` | 实现为 `w3_double_start_only_one_attempt` + `double_start_only_one_attempt_at_table_level` |
| `stale_gen_write_is_dropped` | 实现为 `stale_generation_write_is_dropped` |
| `settled_after_normal_stop_uses_last_completed_gen` | 实现为 `..._generation`（单数改复数） |
| `settled_without_gen_baseline_is_not_settled` | 实现为 `settled_without_generation_baseline_is_not_settled` |
| `attempt_panic_quarantines_and_blocks_start` | 实现为端到端 `teardown_panic_quarantines_and_blocks_start` + 单测 `guard_without_commit_quarantines_entry` |
| `quarantine_cleared_only_after_convergence_proof` | **未落地**（reaper 复核路径只由 `guard_without_commit_quarantines_entry` 部分覆盖）——**待确认**是否需要补 |
| `settle_window_rejects_stale_stopped` | **未落地**（近似覆盖：`await_settled_requires_confirmed_stop_intent_under_slow_db`）——**待确认** |
| `start_during_stop_teardown_is_rejected_and_does_not_resurrect` | 实现为 `w2_start_during_stop_teardown_is_rejected_and_does_not_resurrect` |
| `second_stop_joins_inflight_teardown` | 实现为单测 `second_stop_joins_instead_of_failing` |
| `double_start_only_one_attempt` | 同 `two_starts_only_one_attempt` 一行 |
| `concurrent_teardown_and_new_start_never_shows_stopped_while_collecting` | **未落地**（由 `deleted_endpoint_not_resurrected_by_late_status_write` + snapshot 代次门单测替代）——**待确认**是否需要恢复该不变量用例 |
| `stop_then_concurrent_start_never_leaves_running` | **未落地**（近似：`w1_stop_during_start_construction_leaves_nothing_running`）——**待确认** |
| `stop_idempotent_when_never_running_still_200` | 实现为 `stop_is_idempotent_after_completion`（覆盖"完成后幂等"；"从未启动"由 `second_stop_joins_instead_of_failing` 的 `was_running=false` 覆盖） |
| `stop_500_still_sets_desired_false` | 由 `stop_intent_persist_failure_is_explicit_and_blocks_restart` + `barrier_stop_persist_failure_is_not_reported_as_success` 覆盖 |
| `bootstrap_start_does_not_override_concurrent_stop` | **未落地**（R-1 用例覆盖了 bootstrap 失败面）——**待确认** |
| `field_stop_clean_requires_continuous_10s` | **不是 cargo 测试**，仍是现场证据要求（§7.5），未执行 |

### 9.7 "没破坏"的证明清单（与修复同 PR 提交）

- 正常 Start / Stop / 幂等 / 有界：`stop_lifecycle.rs` 的 `stop_is_idempotent_after_completion`、`stop_while_reconnecting_is_bounded_and_explicit`、`stop_waits_for_inflight_commit_then_drains_exact`。
- A1 退避：`reconnect_backoff.rs` 全套（L1 `1,2,5,10,30,30`、L2、L4 `stop_cancels_escalated_backoff_and_stops_retrying`）保持绿；`endpoint.rs` 退避相关代码未改。
- 直调路径：`subprocess_recovery.rs` 直接 `run_endpoint_with_heartbeat`（旧签名保留、`gen=0`）。
- 冻结码：`stop_endpoint` 的 `Err` 仍须以精确码开头，集合为 §6.8 的 **8 个**（新增 `ATTEMPT_FAILED_UNKNOWN` 必须在 PR 描述里单独声明）。
- 证据口径：`python scripts/write-contract-evidence.py` 产出的 `target/validation/contract.json`（本文档**不代跑**；`stop_lifecycle` 属于 LIFECYCLE 4 suites 之一）。
- 现场判据：`field_stop_clean_requires_continuous_10s` 必须与 Core 侧证据**分开**提交；Core 的 500 ms 窗口通过不等于现场 10 s 通过。

## 10. 明确不做

1. **不重构生命周期**：不引入"期望态 vs 运行态"对账器/看门狗（它本身会启停端点，对现场更危险）；不改 attempt 状态机、不改 teardown 预算。§7.3 的观察器**只读**，不写 `desired_running`、不 cancel attempt。§6.3.3 的 `attempt_reaper` **只清理已死 attempt 的残留**（kill 残留 pid、摘除属于该 gen 的 registry 键），**不启停端点、不写期望态、不改快照** ⇒ 它不是对账器，不属本项排除对象；但它的实现必须守住这条边界（代码注释 + review 门）。
2. **不改 wire/IPC**：`driver-protocol`、protobuf、驱动进程协议、REST 路由与字段一律不动；`EndpointStatus`（`snapshot.rs:83-95`）**不加字段**，代次只存在于 `Snapshot` 的内部 map 键结构（`EndpointRecord{generation,status}`）里。对外行为变化仅限 §6.9 "回归风险"列出的**六处**（状态码/code、Stop 的失败可见性、`STOP_PERSIST_IN_PROGRESS`、`DELETE_PERSIST_IN_PROGRESS`），**不新增路由、不新增响应字段**。
3. **不改 A1**：`endpoint.rs:44-50`、`:315`、`:416-418`、`:59-61` 一字不改；不调退避档位、不改归零判据。
4. **不新增持久化列**：`generation`/`last_completed_gen` 只存内存，避免 `endpoints` 表迁移与 `assert_changed_within` 类红线冲突。**注意两点**：(a) §6.10 修改了 `UPDATE` 语句的列清单（去掉 `desired_running`），但**不改表结构、不加列、不迁移**；(b) S2 的停止意图围栏与 P0-4 的正向凭证**也不加列**——围栏复用既有 `meta` 键值表（`STOP_INTENT_FENCE_KEY`，`config-store/src/lib.rs:342`）+ DB 同目录 sidecar 文件（`.stop-fence.json`，`:349`），凭证用 `<db>.intent-proof.json`（`:357`）。
5. **不修** `run_config_flow` 无取消点（Stop 后仍可能 `StartConnection`）——文档 §5.3-⑥ 已作为**观测判据**登记，属独立缺陷，另立设计；本设计不宣称消除它。
6. **`DELETE` 的同族 TOCTOU 已在 G7 内顺带收紧，`PUT /tasks` 仍未修**：DELETE 的判据已由 `is_running` 换成 `has_live_attempt`（`lib.rs:2130`），并加了删除屏障（§6.12），因此"拆解期删配置"不再是可达缺陷；`PUT /tasks`（`lib.rs:2349`、`:2406`、`:2432`）仍用 `is_running`，**仍是同族 TOCTOU，本设计不修**（改它会连带改变任务编辑语义，超范围）。"旧代次写入复活已删端点"由代次门/墓碑覆盖（§6.5）。
7. **不删除** 828D 的 G7-3（无在途 Start）与文档 §5.3 的竞态观测：修复只是让竞态**显式失败**（409），现场仍按 `RACE_START_AFTER_STOP` 记录。
8. **不加自动强杀/自动恢复**：`child.wait()` 无界时保持 fail-closed（Start 一直 409）并大声告警，不引入"超时就当停了"的判定（§6.3.1）。**唯一例外**是 §6.3.3 的 `attempt_reaper`：它只对"任务已死、条目处于隔离态"的残留 pid 做强杀与回收，不改变任何活着的 attempt 的 teardown 预算。
9. **不做现场 10 s 判据的自动化**：Core 不实现"连续 10 s 观察"、不新增配置项、不写 `desired_running`（§7.5）。现场判据由证据脚本/监护流程执行，Core 只提供可观测面。
10. **不做额外的"上次干净退出"持久标记**：重启误拉起的风险已由 **S2 围栏（双通道）+ 正向凭证 + 默认不恢复 + P0-C（NCK 永不自动恢复）** 消除（§6.10.3 已改写），不需要再加"上次是否干净退出"这类新的持久状态（它与 §10-4 冲突）。剩余的唯一情形是"两通道同时不可写"，其后果是**不恢复**（安全方向），不是误拉起。

---

## 11. 待确认

1. **"新的操作授权"的确切含义**（契约行 3）：本文按"重新 admit + 重新 `build_runtime_endpoint`（新 `stream_epoch`）+ 既有授权门（`CONTROL_DISABLED` 等）"实现，**不新增鉴权机制**。若评审指的是新增授权令牌/二次确认，属另一层设计，需明确。
2. **~~`desired_running` 写入时机前移~~（第二轮已冻结，不再讨论）**：前移是既定语义（§5.3），本文不再列为待确认。仅保留一条**实现层**注意：拆解期间 DB 显示"期望停止"而实际仍在 `STOPPING`，这被 §5.3 明确为**合法组合**，UI/告警不得据此报"矛盾状态"。
3. **~~并发 Stop 合流语义~~（第二轮已冻结，不再讨论）**：合流（不采用 409）是既定语义（§5.2），备选方案不再评估。
4. **门的归属变更**：原稿建议门放 `core-api` 且由门持有 `inflight_stops`。本轮改为"真值在 `driver-manager` 的 attempt 表，`core-api` 只做排序与票据"，因此**不需要**把门搬到 manager，也不需要 manager 反向依赖 core-api。若评审坚持"门整体下沉到 manager"，则需评估 `MesaManager` 公共 API 的 churn（`begin_stop` 之外还要暴露票据模型）。
5. **`manager` 新增同步 API 的归属（已落地，供评审确认）**：`begin_stop`（同步）、`has_live_attempt`、`attempt_phase`、`current_gen`（§6.11）、`gen_floor`（§6.5）都在 `manager.rs:790-828`、`:996-1012`；`await_settled` 在门（`lifecycle.rs:986-1046`）。**仍待确认**的只是"是否接受 manager 暴露 `attempt_phase`"这一诊断口（当前仅测试与日志使用）。
6. **Core 内部稳定窗口参数**：`250 ms × 2 = 500 ms` 是否足够；是否需要按 endpoint/驱动类型可配（S7/FOCAS2 的会话关闭时延不同）。**注意：本条只讨论 Core 内部快速收敛检查；现场 10 s 判据（§7.5）不是可配参数，也不由 Core 执行，不在本条讨论范围。**
7. **~~全局 `LIVE_CHILDREN` 计数~~（已落地）**：`process.rs:30-34` 实现，`stop_lifecycle.rs:1548-1554` 断言 Stop 后回落为 0。**遗留待确认**：它把临时 Descriptor 进程也算进去，判据侧是否需要显式排除基线（当前端到端用例是在无 Descriptor 需求的场景下断言的）。
8. **W3 现场可达性**：本轮已由 insert-before-spawn **结构性**封死，不再依赖"是否真并发"的论证；但若评审仍希望保留"提交时 `is_running` 复核"作为第三道防线，需说明其与 attempt 表判定的优先级（本文认为重复，不采纳）。
9. **`EndpointMetaUpdate` 类型化改造的 churn**（§6.10.2 第 2 层保证）：是否值得把 `update_endpoint()` / `update_endpoint_with_secrets()` 的入参从 `&EndpointRecord` 换成不含 `desired_running` 的专用结构，使"用旧对象写回"在编译期不可表达。收益是类型级防复发，代价是 `config-store` 与 `core-api` 的调用点 churn（当前生产调用点 2 处 + 单测若干）。**原第 9 条（写失败是否升级为告警/返回）已由 §6.10.3 定案，不再待确认。**
10. **`latest` / `point_meta` 的迟到写入是否需要墓碑过滤**（§6.5）：墓碑门目前只覆盖 `endpoints` 表（决定"端点是否复活"）。已删除端点的迟到 `register_points`/`apply_batch` 仍可能写入 `latest` 行，影响 `/api/v1/points/latest` 的"幽灵行"。本文**未**把该路径纳入（不在 P0-4 的判据内），是否同 PR 处理待定——**待确认**。
11. **~~"上次进程是否干净退出"的持久标记~~（已由 S2 + 正向凭证 + P0-C 取代）**：见 §6.10.3 的三层机制与 §10-10。不再需要该标记；若评审仍希望对"两通道同时不可写"给出更强保证，才需要重新讨论。
12. **`attempt_reaper` 的归属与触发方式（已落地，供评审确认）**：放在 `MesaManager` 内、由 `quarantine_tx`（unbounded）事件驱动，复核为 60 × 500 ms 有界重试（`manager.rs:1458-1563`）。**遗留待确认**：约 30 s 的复核窗口是否足够覆盖 Windows 上"僵尸进程回收慢"的现场（`pid` 查询返回 `Unknown` 时会持续重试，但只要一轮 60 次用尽即放弃并保持隔离，后续只能重启 Core）。

13. **`wait_reader_done` 的结果仍未纳入判据**（判据 5 的已知缺口）：`AttemptCtx.remote_eof` 字段已存在，但 reader barrier 的结果目前只写一条 warn 日志，没有上报给 Stop 调用方（原 F20 的问题**未消除**）。是否把"对端 EOF"升级为判据 5 的必需项——**待确认**。

14. **原稿有 5 条建议用例未落地**（§9.6）：`quarantine_cleared_only_after_convergence_proof`、`settle_window_rejects_stale_stopped`、`concurrent_teardown_and_new_start_never_shows_stopped_while_collecting`、`stop_then_concurrent_start_never_leaves_running`、`bootstrap_start_does_not_override_concurrent_stop`。是否需要补齐——**待确认**（当前由邻近用例部分覆盖，已逐条注明）。

15. **`DELETE_PERSIST_WAIT = 10s` 的取值**（P0-D）：依据是"验收用慢写注入 2.5–9 s + 正常路径零等待"（`lifecycle.rs:49-54`）。现场若出现更长的事务阻塞，DELETE 会稳定返回 409 让客户端重试；是否需要一个可配置项——**待确认**（本文倾向不引入配置，避免"配置得越小越危险"）。

16. **P0-C 在产品层的可见性**：NCK 端点被开机跳过时目前只写 `warn!` 日志（`main.rs:191-197`），REST 面上没有"因产品策略跳过恢复"的状态位。是否需要可观测字段——**待确认**（`nck-828d-phase1-boot-recovery-policy.md` 属独立文档，本文不改它）。
