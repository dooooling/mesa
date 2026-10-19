# P1 缺陷审查：Endpoint 假 RUNNING 导致重连退避持续重置

- 审查基线：`main = 2a29777b643c674557d975cdf93998645cbe10d6`（工作树静态定位）。
- 审查性质：**只读**。未修改生产代码/测试/配置；未启动 mesad/Vite/emulator，未连接设备，未安装，未运行测试或构建。
- 演练证据：`target/validation/l-drill/`（演练时 HEAD 为 `0b2abd3`，相对 `main` 净变更仅一份 `docs/` 文档，`crates/`、`drivers/`、`tests/` 完全一致，故证据可直接引用）。
- 证据口径：事实一律给 `文件:行号`；无法证实的一律标 **待确认**。
- 本轮**不决定**“什么算稳定运行”（时长/证据/组合），该阈值属生命周期设计审查。

## 0. 结论速览

1. **RUNNING 的判据只有一条**：`StartConnectionAck` 的 `result.ok == true`。Core 收到 Ack 后**立即**写 `RUNNING`（`crates/driver-manager/src/endpoint.rs:528-536`），**不要求**任何采集成功证据。
2. **`StartConnectionAck` 在语义上不是“已连上设备”**：SDK 在 `on_start` 里先 `spawn(run)`、再回 Ack（`crates/driver-sdk/src/lib.rs:1520-1545,1569-1581`），而建连发生在 `run()` 内部（见 §4 表）。五个驱动**全部**如此。
3. **退避重置的门槛是“本次 attempt 是否走到事件循环”**，即 `had_running_session`，它在 16 个 `AttemptOutcome::Lost` 构造点被硬编码：能走到 `event_loop` 的 6 处为 `true`，Start 之前的 10 处为 `false`。它**不区分**“进程起来了”与“数据面通了”。
4. **因此 L1（永远连不上）也确实会让 `backoff_idx` 每次归零**，退避表 `[1,2,5,10,30]` 在该路径上**不可达**（`endpoint.rs:32-38,357-368`）。实测 17/17 次计划值恒为 1s（`target/validation/l-drill/L1-L2-止损演练报告.md:123-133`）。
5. **不是 NCK 独有**：这是 SDK “Ack 先于建连” + Core “Running 即归零”的组合性质，S7/FOCAS2/OPC UA/test-driver 走同一形状，只是 NCK 在演练里被最先暴露。
6. **推荐最小修复层 = Core（方案 A1）**：把“归零”的判据从“进入过 Running”改为“本次 attempt **收到过至少一个批次**（质量无关）”。不改 wire、不改驱动、不引入时间阈值、不动 RUNNING 展示语义。真正的 RUNNING 定义修正（Driver 上报真实连接状态）放到生命周期设计审查（方案 B/C）。

---

## 1. Q1：Core 收到什么事件把 Endpoint 标记为 RUNNING？

### 1.1 完整链路

| # | 环节 | 事实 | 证据 |
|---|---|---|---|
| ① | Core 发起 | `run_config_flow` 依序 `OpenConnection → ConfigureTasks → ApplyPointMap → ConfigureEventTasks → StartConnection(epoch)` | `crates/driver-manager/src/endpoint.rs:702-849` |
| ② | Driver 收到 Start | SDK `on_start` 校验 `entry.run.is_none()`，随后 `tokio::spawn(run)`，run 先 park 在 release gate | `crates/driver-sdk/src/lib.rs:1476-1533` |
| ③ | Driver 回 Ack | 写 `StartConnectionAck{result: ok_result()}`，**然后**才 `release_tx.send(())` 放行 run | `crates/driver-sdk/src/lib.rs:1569-1581` |
| ④ | Core 判 Ack | `expect_ack` 取 `StartConnectionAck`，`config_gate` 只看 `result.ok` | `crates/driver-manager/src/endpoint.rs:843-844,1002-1020` |
| ⑤ | 记 epoch | `*last_epoch = epoch`，日志 `connection started` | `crates/driver-manager/src/endpoint.rs:845-846` |
| ⑥ | **写 RUNNING** | `set_status(..., ConnectionState::Running, ...)`，覆盖 `state/detail/points/epoch/revision` | `crates/driver-manager/src/endpoint.rs:528-536`；写入原语 `:389-407` |
| ⑦ | 进入事件循环 | 同一 call 里紧接 `event_loop(...)` | `crates/driver-manager/src/endpoint.rs:542-550` |
| ⑧ | 后续状态变更 | 驱动 `ConnectionStateChanged{Failed}` → `SessionEvent::State` → Core 转 `Lost` | `crates/driver-manager/src/session.rs:1208-1214`；`endpoint.rs:908-916` |

**不存在第二条写 RUNNING 的路径**：全仓 `ConnectionState::Running` 的写入点只有 `endpoint.rs:531` 一处（其余命中均为 `as_str()` 或解析）。

### 1.2 是否要求“已完成首次成功采集”？

**不要求，明确不要求。** 判据链上没有任何数据面条件：

- `run_config_flow` 的返回 `Ok(())` 只由 Start Ack 决定（`endpoint.rs:843-848`）；
- `event_loop` 收到 `Batch` 只做 `apply_batch_logged`（`endpoint.rs:905-907`），不参与状态判定；
- 驱动侧也从不发送 `Running` 状态事件：SDK 只在 run 结束时发 `Stopped`/`Failed`（`crates/driver-sdk/src/lib.rs:1537-1544`）。

**演练反证**：L1 连接 `127.0.0.1:1`（必然拒绝）时，`20:02:17.975` 出现 `state=RUNNING`、`epoch=1792131065577240024`、`points=1`，而该阶段 `points/latest` 为 **0 条**（`L1-L2-止损演练报告.md:99-103,118-120`）。

---

## 2. Q2：RUNNING 是否意味着已完成首次成功采集？

**对所有驱动都是“否”，对 NCK 尤其明确为“否”。**

- **NCK**：`open_connection` 只解析/校验配置并返回 `NckConnection`，注释即写明“连接只决定连哪台设备”（`drivers/sinumerik-nck/src/lib.rs:355-378`）；真正的 TCP/COTP/S7 Setup 发生在 `run()` 的 `NckClient::connect`（`drivers/sinumerik-nck/src/lib.rs:870-873`；`drivers/sinumerik-nck/src/client.rs:55-71`；`crates/s7-transport/src/session.rs:76-110`）。而 Start Ack 早于 `run()` 放行 ⇒ **RUNNING 时 TCP 尚未建立**。
- **其他驱动**：同一形状（建连都在 `run()` 内，见 §4 表），因此 RUNNING 同样早于建连。
- **数据面另行成立**：NCK 连接成功后才进入 ticker 循环并 `sink.publish`（`drivers/sinumerik-nck/src/lib.rs:903-954`）。所以“RUNNING + 0 条采样”是合法可观测组合，不能反推为异常。

**结论**：`RUNNING` 当前语义 = “驱动已接受 Start（会话可寻址 + 配置已装载）”，**不是**“设备已连通”，更不是“已采到数”。

---

## 3. Q3：为什么“短暂 RUNNING”就能清零失败计数？

### 3.1 `had_running_session` 的确切来源

它不是变量而是 `AttemptOutcome::Lost` 的字段（`endpoint.rs:380-387`），由 `event_loop` 的退出路径构造，`run_endpoint_with_heartbeat` 只在 `Lost` 分支读取一次：

```text
endpoint.rs:336-361   Lost{reason, had_running_session}
  → 346  日志 "connection lost ... had_running_session=?"
  → 347  snapshot.mark_communication_lost
  → 348-356 set_status(Reconnecting)
  → 357-360 if had_running_session { backoff_idx = 0 }   ← 唯一归零点
  → 365-368 delay = BACKOFF[backoff_idx]; if backoff_idx < 10 { += 1 }
```

### 3.2 全部赋值点（16 处 = 6 `true` + 10 `false`，二值即“是否走过 event_loop”）

| 值 | 位置 | 条件 |
|---|---|---|
| `false` | `endpoint.rs:437-441` | 子进程 spawn 失败 |
| `false` | `endpoint.rs:457-461` | 握手/Session 建连失败 |
| `false` | `endpoint.rs:722-726` | OpenConnection RPC 传输失败 |
| `false` | `endpoint.rs:754-758` | ConfigureTasks RPC 传输失败 |
| `false` | `endpoint.rs:813-817` | ApplyPointMap RPC 传输失败 |
| `false` | `endpoint.rs:838-842` | StartConnection RPC 传输失败 |
| `false` | `endpoint.rs:986-990,995-998` | 事件配置失败（非配置类） |
| `false` | `endpoint.rs:1025-1029,1033-1037` | `config_fail`/`lost` 辅助构造 |
| **`true`** | `endpoint.rs:887-902` | ingress 退出/panic |
| **`true`** | `endpoint.rs:910-915` | 驱动上报 `Stopped`/`Failed` |
| **`true`** | `endpoint.rs:923-927` | 事件通道关闭 |
| **`true`** | `endpoint.rs:930-936` | 心跳判死 |

### 3.3 精确回答“为什么不区分进程起来与数据面通”

因为它**根本没有数据面观测量**：`event_loop` 的 4 个 `true` 返回点（共 6 个构造点，其中 ingress 分支含 3 个）由“循环是否运行过”决定，而循环的启动前提只有 `StartConnectionAck`。于是**任何**在 Ack 之后 1ms 内失败的 attempt，与一条稳定运行 8 小时的连接，对 `backoff_idx` 的效果完全相同（都归零）。

这解释了两个实测现象：

1. **退避表不可达**：失败发生在 Ack 之后 ⇒ 每次都归零 ⇒ 计划值恒 1s（`L1-L2-止损演练报告.md:123-133`）；`retry_in_secs=1` 在 `mesad.out` 逐条出现（`target/validation/l-drill/run-20261019-200203/mesad.out`，12:02:19.995 / 25.416 / 28.995 …）。
2. **L2 也立即归零**：真实 RUNNING（epoch `520274126110681454`、2 点 GOOD）后杀 emulator，下一次重试仍以假 RUNNING 收场，于是 3/3 次计划值仍是 1s（`L1-L2-止损演练报告.md:181-202`）。

---

## 4. Q4：驱动差异 —— 只有 NCK 有此问题吗？

**不是。** 差异只在“建连失败能被多快观测到”，不在机制。

| 驱动 | `open_connection` 是否建连 | 建连真实位置 | Start Ack 时机 | 失败上报 |
|---|---|---|---|---|
| sinumerik-nck | **否**，仅校验 family/config（`drivers/sinumerik-nck/src/lib.rs:355-378`） | `run()` → `NckClient::connect`（`lib.rs:870`；`client.rs:55-71`） | SDK 统一：`spawn(run)` 后立即 Ack（`crates/driver-sdk/src/lib.rs:1520-1581`） | run 返回 Err → `Failed`+detail（`driver-sdk:1537-1544`） |
| s7 | **否**，仅解析 `S7ConnConfig`（`drivers/s7/src/lib.rs:221-234`） | `run()` → `S7Client::connect`（`lib.rs:777`） | 同上 | 同上 |
| focas2 | **否**，只构造 `FocasApi` 实例（`drivers/focas2/src/lib.rs:784-824`） | `run()` → `api.connect`（`lib.rs:1427-1442`） | 同上 | 同上 |
| opcua | **否**，只构造 transport（`drivers/opcua/src/lib.rs:223-276`） | `run()` → `api.connect`（`lib.rs:1269-1283`），随后还有 `read_namespace_array`（`lib.rs:1290-1296`） | 同上 | 同上 |
| test-driver | **否**，只解析 faults（`tests/support/test-driver/src/lib.rs:419-459`） | 无真实连接（纯模拟） | 同上 | 仅注入故障：`fail_after_batches`/`crash_after_batches`（`test-driver/src/lib.rs:1448-1463`） |

**逐驱动判断**：

- **NCK**：最严重。建连失败延迟约 2.03s 才上报（实测 Ack→loss 2.032s，`mesad.out` 12:02:17.963→19.995），假 RUNNING 窗口最长。成因（本机 SYN 重传 vs 端口行为）**待确认**，不影响结论。
- **S7 / FOCAS2 / OPC UA**：机制相同；只要目标设备不可达/认证失败，就在 Ack 之后失败 ⇒ 同样每次归零。OPC UA 还多一个“连上了但 namespace 解析失败”的第二窗口（`opcua:1290-1296`）。
- **test-driver**：默认不会失败；但 `crash_after_batches`/`fail_after_batches` 注入出的“短 RUNNING→Lost”完全复刻该形状（`tests/driver-contract/tests/stop_lifecycle.rs:75-105` 已用 `crash_after_batches:2`）。

**另一个同源触发面（NCK）**：任务计划为空时，`run()` 的 `JoinSet` 无成员，循环立即结束并返回 `Ok(())`（`drivers/sinumerik-nck/src/lib.rs:959-986`）⇒ SDK 报 `Stopped` ⇒ Core `Lost(true)` ⇒ 立即进入 1s 循环。是否应在 Configure/Start 层拒绝空计划，**待确认**。

---

## 5. Q5：正确修复层与代价

### 5.1 候选方案对比

| # | 方案 | 改动面 | 跨驱动影响 | 回归风险 | 评价 |
|---|---|---|---|---|---|
| **A0** | 删除归零（`backoff_idx` 永不重置） | 1 行 | 无 | **高**：真恢复后也要从 30s 起步，直接违反 L3 与 `endpoint.rs:30` 注释语义 | 拒绝 |
| **A1** | 归零需“本次 attempt 收到过至少一个批次（质量无关）” | `endpoint.rs` 单文件，约 15–25 行（`AttemptOutcome` 加 `saw_batch` 字段 + 6 个 `event_loop` 出口透传 + 归零条件） | **零**（不改 wire、不改驱动、不改配置） | 中低：见 5.3 的三个边界 | **推荐（止血）** |
| **A2** | 归零需“Running 持续 ≥ T”或“证据 + 时长”组合 | 同 A1 + 引入阈值 | 零 | 中：阈值即契约，本轮明确不定；会把“慢首采”端点误判 | 推迟到生命周期设计 |
| **B** | Driver 上报真实连接状态（新事件/新状态，Driver 建连成功才发 Ready） | wire + 5 驱动 + SDK + 版本协商 + 兼容回退 | **大**：全部驱动、全部 E2E、旧版驱动兼容 | 高 | 正确方向，但非本轮 |
| **C** | A1 先落地，B 在生命周期设计里补齐 | 两阶段 | 阶段一零影响 | 阶段二同 B | **建议的路线** |

被否决的“看起来更小”的候选（记录以免重复讨论）：

- **按 `reason` 字符串识别 `CONNECT_FAIL` 再决定是否归零**：违反“Core 不懂协议”（AGENTS.md 硬约束），且码表会随驱动漂移 —— 拒绝。
- **加最小重试间隔（rate limit）而非退避**：只是把风暴换成另一种风暴上限，且新增可调参数 —— 拒绝。
- **在 `snapshot`/`mark_communication_lost` 里处理退避**：`Snapshot` 是跨 Endpoint 共享结构，会把 L5 直接弄坏 —— 拒绝。

### 5.2 A1 的精确形状（设计，不实现）

```text
run_endpoint_with_heartbeat:
    if had_running_session && saw_batch { backoff_idx = 0 }   // 替换 endpoint.rs:357-360
attempt_session:
    在 event_loop 收到 SessionEvent::Batch 时置 saw_batch = true
    Lost 出口携带 saw_batch（6 个 true 出口 + 10 个 false 出口统一成同一字段）
```

判据语义是**“数据面通过”而非“数据质量良好”**：BAD 采样同样是批次，因此“合法持续返回 BAD 的设备”不会被误伤（满足两条设计约束之一）。它**不改变** RUNNING 的展示定义 —— 修复只针对退避，假 RUNNING 的显示屏问题留给 §9。

### 5.3 A1 的边界与风险（必须写进实现评审）

1. **事件型端点**：`EventBatch` 被 ingress 直接消费（`endpoint.rs:468-492`），`event_loop` 看不到，`saw_batch` 只覆盖 `DataBatch`。event-only 端点（OPC UA §18）需要额外计数（ingress 已直写 `services.diagnostics`，`endpoint.rs:477-481`）——**待确认**取值路径。
2. **慢首采端点**：若一次健康会话在丢失前尚未到达首个批次（poll 间隔长于会话寿命），会被判为“无证据”而升级退避。这是 A1 的已知误差，**不应**用阈值私了，应由 B 消除。
3. **零任务端点**：永远无批次 ⇒ 永远升级到 30s 重连。对“无点可采”的端点这是资源层面可接受的行为，但需在生命周期审查中确认（与 §4 末的空计划问题一并处理）。
4. **`MESA_RECONNECT_FAST` 仍是进程级 env**：`reconnect_backoff_secs` 每次调用读 `std::env::var`（`endpoint.rs:32-35`），而 `HeartbeatParams` 已经走“显式传参”改造（`endpoint.rs:241-263`）。若测试要断言递增序列，建议后续把退避参数同样显式化（非本轮）。

### 5.4 为什么“先修 Core”是对的

- 缺陷的**可判定位置**在 Core：驱动只是如实上报“Start 已被接受、随后失败”，没有任何驱动撒谎。让驱动去补“我到底连上没有”，等于要求 5 个驱动各自发明一套连接判据（B 的成本来源）。
- 跨驱动一致性：A1 对 s7/focas2/opcua/test-driver **零改动**，不会出现“NCK 修好了、S7 还在风暴”的分裂。
- 与既有架构约束相容：不新增协议字段、不动 IPC 版本、不动冻结契约。

---

## 6. Q6：如何稳定复现“连续短暂 RUNNING + 失败”？

### 6.1 三层复现手段

| 层 | 手段 | 确定性 | 能证明什么 | 现状 |
|---|---|---|---|---|
| L0 单测 | 把归零决策抽成纯函数（`should_reset(had_running, saw_batch)`）+ `reconnect_backoff_secs` 表驱动（0..10 → `[1,2,5,10,30,30,…]`） | 完全 | 序列与判定逻辑；**不能**证明端到端行为 | 需新增（本轮不写） |
| L1 契约测试 | 需要“**首采前就失败**”的 attempt：现有 `fail_after_batches:1` 会先发 1 批（修复后应归零，不能用作递增用例）；可选 (a) test-driver 新增“首采前失败”故障键，或 (b) 直接用 NCK→`127.0.0.1:1`。复用 `stop_lifecycle.rs:59-124` 的 `MesaManager + snapshot` 骨架 | 高（无真实设备） | 每次 attempt 都“RUNNING 后立刻 Lost 且 0 批”时，退避是否递增 | 骨架已存在，用例/故障键需新增 |
| L2 离线演练 | 复用 `target/validation/l-drill/drill.py` + `watch.ps1`（任意 `-Base`/`-Endpoint`、200ms 轮询、`-NoStop` 记录退避序列、JSONL+summary） | 中（依赖本机端口行为） | 真实驱动链路（NCK→`127.0.0.1:1`；emulator→kill） | **已跑通**，可直接复用 |

### 6.2 直接回答“演练脚本能否复用”

**能。** 两个脚本都是参数化的，不含被测端点的硬编码：

- `drill.py` 自起 `mesad`（随机空闲端口，`env.pop("MESA_RECONNECT_FAST")`，`drill.py:277`）、REST 建 Endpoint、拉起 `watch.ps1`、汇总 `evidence.json`；
- `watch.ps1` 参数为 `Base/Endpoint/LogPath/ReadyPath/SummaryPath/TimeoutSec/PollMs/NoStop`（`watch.ps1:13-24`），已具备“先轮询后 Start”门禁与“Stop 耗时/终止态”记录；
- L1 补充观测（58s、17 次失败）就是 `-NoStop` 模式产出的（`l1-backoff/watch.summary.json`）。

复用要点：**被测 mesad 的 `MESA_RECONNECT_FAST` 必须是空**（演练报告 §1.3 已核验），否则固定 1s 会掩盖修复效果；断言应基于**计划退避值**而非 loss→loss 间隔（后者含 ~0.5s 拉起 + ~2.0s 建连）。

### 6.3 修复后必须“旧码红、新码绿”的用例

- **T-L1**：连续 6 次失败，计划退避序列 = `[1,2,5,10,30,30]`；当前实现恒 `[1,1,1,1,1,1]` ⇒ 旧红。
- **T-L2**：每次 attempt ≤2.5s 且 **0 批次**（须用“首采前失败”源，见 §6.1），5 次后相邻 attempt 间隔 ≥5s；当前恒 ≈3.58s ⇒ 旧红。
- **T-L3**：真实 RUNNING（≥1 批）后杀掉对端 ⇒ **第一次**重连间隔回到 1s 且能恢复 RUNNING（防止 A1 过度收紧）。
- **T-L6**：现有 27 套 contract suite 全绿（重点 `subprocess_recovery`、`stop_lifecycle`、`fault_tolerance`）。

---

## 7. 修复**不得**做的事（避免误伤 L3/L6）

1. **不得**把 RUNNING 定义改成“首个 GOOD 才算运行”——合法持续 BAD 的设备、暂无采集点的端点会被永久判为未运行（违反评审约束一）。
2. **不得**引入“稳定运行时长/证据阈值”（`T`、连续 N 批、`running_since` 判定），本轮明确不定（约束二）。
3. **不得**删除归零本身（A0）——真恢复后被迫从 30s 起步，直接违反 L3。
4. **不得**改 `RECONNECT_BACKOFF_SECS` 表值、`idx.min(4)` 或 `backoff_idx < 10` 的边界（`endpoint.rs:36-37,366-368`）；也不得把 `MESA_RECONNECT_FAST` 从短路逻辑里去掉（现有 gate 依赖它）。
5. **不得**让归零依赖本次 attempt 之外的状态：`id_map.len()`、`snapshot` 全局计数、跨 Endpoint 结构（会破坏 L5）。
6. **不得**在驱动侧把 `Failed` 改写为 `Stopped`、静默自连或吞掉错误——那会隐藏故障并绕过 Core teardown 顺序（`endpoint.rs:606-659` 的契约）。
7. **不得**改动 Stop 取消退避的 `select!`（`endpoint.rs:370-376`）与 `stop_endpoint` 的 `desired_running=false` 落盘（`crates/core-api/src/lib.rs:2165-2191`）——L4 的现有实现是对的。
8. **不得**改 test-driver 的故障键语义（`fail_after_batches`/`crash_after_batches`），否则 `stop_lifecycle`、`subprocess_recovery` 等既有 gate 失效。
9. **不得**只改文档/注释而不改判定（当前注释 `endpoint.rs:357` 已经写着“仅当曾进入 Running 才归零”，与缺陷同源，必须连判定一起改）。

---

## 8. L1–L6 的可验证判据（不实现，仅判据）

| 编号 | 判据 | 观测点 | 通过条件 |
|---|---|---|---|
| **L1** | 连接从未成功时退避递增 | 计划退避值（`retry_in_secs` 或新增 `current_backoff_ms`） | 连续 6 次失败的计划值序列 = `[1,2,5,10,30,30]`；不得出现 1s 恒定 |
| **L2** | 短暂 RUNNING 随即失败不得无限高频重连 | 相邻 attempt 起始时间戳 | 5 次之后相邻间隔 ≥5s；60s 内 attempt 次数 ≤6（当前为 17） |
| **L3** | 真稳定运行后的中断仍按既定策略重连 | 首个 `Batch` 所在 attempt 的 loss 之后 | 下一次 attempt 延迟 = 1s（归零生效），且 ≤2 个 attempt 内重新 RUNNING 并出新 epoch 的批次 |
| **L4** | Stop 可打断退避且 `desired_running=false` | `POST /stop` 返回码/耗时 + 终态 | 退避剩余 ≥5s 时 Stop：≤40s 返回、`state=STOPPED`、`desired_running=false`、Stop 后无新驱动进程 |
| **L5** | 无跨 Endpoint 误影响 | 两 Endpoint 并行（A 恒失败、B 健康） | B 的 state/epoch/批次不受 A 影响；Stop A 不改变 B；A 的退避递增不导致 B 重启 |
| **L6** | 不破坏正常启动与恢复 | 既有 contract suite + 正常启停 | 27 套全绿；正常 Start→RUNNING→Stop→Start 路径无新增等待/无新增错误码 |

---

## 9. “连接状态 vs 数据质量”分别定义（建议）

**现状已具备分离基础**：`Quality` 有独立的 `quality_code` 体系（`docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md:591-604`），断线由 `mark_communication_lost` 统一置 `BAD/COMMUNICATION_LOST` 并保留 last value（`crates/driver-manager/src/snapshot.rs:396-424`）。缺陷在于**连接状态被塞进了“Start 被接受”这个动作语义**。

建议（供生命周期设计审查采纳，非本轮实施）：

1. **连接状态只回答“链路是否可用”**：取值域保持 `ConnectionState` 五态（`crates/core-types/src/lib.rs:599-620`），但 `RUNNING` 的判据改为“驱动上报的连接已建立”（B 方案事件），而不是“Start 已被接受”。
2. **数据质量只回答“样本是否可信”**：GOOD/BAD + `quality_code`（`COMMUNICATION_LOST/NO_CURRENT_VALUE/…`）。**不得**用“有没有 GOOD”反推连接是否建立。
3. **新增两个正交观测量（设计层面）**：
   - `last_batch_at_ns`（本 epoch 最近一次任意质量批次到达时间）——表达“有数据流”；
   - `first_batch_at_ns`（本 epoch 首个批次时间）——表达“数据面已通过”。
   二者都可为空 = “已连接但尚无样本”，这**不是**错误状态。
4. **展示层拆两列**：`连接：已建立/重连中` 与 `数据：有流/无流`。UI 不得把 `points/latest` 为空渲染成“未运行”（L1 已证 0 条仍 RUNNING）。
5. **禁止**把“无采集点”或“持续 BAD”编码成连接故障——两者都在合法语义内。

---

## 10. 仍待确认项

| # | 待确认 | 影响 |
|---|---|---|
| 1 | NCK 建连失败耗时约 2.03s 的确切成因（Windows SYN 重传 / 本机端口行为） | 只影响假 RUNNING 窗口长度，不影响结论 |
| 2 | `EventBatch` 在 ingress 内的计数如何被 `attempt_session` 读取（event-only 端点需要） | 决定 A1 实现细节 |
| 3 | Core 是否允许“零任务”端点 Start（NCK 会立即 `Ok(())` 返回并触发 1s 循环） | 可能需要在 Configure 层 fail-closed |
| 4 | 是否新增 test-driver 故障键以精确复现“RUNNING 后 0 批即失败” | 影响 T-L2 能否在纯本地确定性成立 |
| 5 | `reconnect_attempt_total`/`current_backoff_ms` 目前未实现（`crates/core-api/src/lib.rs:1499-1503` 硬编码 0） | 建议随修复补上，否则 L1/L2 只能靠日志断言 |
| 6 | 退避参数是否改为显式传参（对齐 `HeartbeatParams`），以消除进程级 env 对并行测试的串扰 | 影响 T-L1 的稳定性 |
| 7 | “稳定运行”阈值（时长/证据/组合） | 按评审要求，本轮不决定，留给生命周期设计审查 |
