# 828D 一期首轮限定采样执行申请（真机验收标准模板 v1）

- **文档性质：申请，尚未获批。** 本文档不构成执行授权；只有 §10 裁决为 **GO** 且四方签字齐全后，才可按 §5 执行。**未获批准不得 Start。**
- **模板用法**：每轮复制本文件 → 改写 §1 空白、§3 点集、§5 时间窗 → **重跑 §4 六道现场门（用本轮新证据）** → §10 重新裁决。**上一轮的 GO 不延续到下一轮。**
- 实施基线：**`main = 1a382ee58428c472cc936d2340b21deee7a2f070`**（G7 代码、G7 设计、watchdog 全部已合并；编制时工作树干净，本文件为唯一新增）；本轮 **R1**；版本 **v1.5**；编制日期 2026-10-20（v1.3 对齐修订 2026-10-23；**v1.5 状态同步 2026-10-23（v1.4 勘误同日）**）。
- **v1.4 勘误声明（必须先读；只追加标注，不改写 v1.3 原文）**：v1.3 有两处技术口径与实现不符，本版据代码与 PR 更正。原文**保留**并就地加注（`~~删除线~~` 表示被更正的原句），因为 v1.3 是已签署的申请记录，**不得静默改写历史**：
  1. **事实 A：`ApplyPointMap` 不写 `point_registry`**。`point_registry` 的**唯一写点**是 `assign_point_ids`（`crates/config-store/src/lib.rs:2344-2456`），经 `StorePointIdSource::assign`（`crates/driver-manager/src/endpoint.rs:206-224`，调用点 `:213`）在 `run_config_flow`（`endpoint.rs:943-945`）中调用，**位于 `ConfigureTasks`（`endpoint.rs:914`）之后、`ApplyPointMap`（`endpoint.rs:968-985`）之前**；`ApplyPointMap` 只把 `point_key → point_id` 映射下发给 Driver，**本身不落库**。凡 v1.3 中"`point_map` 由 Start 时的 `ApplyPointMap` 写入"一类表述，一律按此更正（见 §4.6 末条、§11-2）。
  2. **事实 B：`-ExpectedPointCount` 的语义已由 PR #136 变更**（BLOCKED-01；`dfc81e0`，**已合入 main（merge `f5f9f56a35a8d3c457a139f8904fcbaf4372e119`）**）。旧语义：比对 `diagnostics.point_count`（持久化 `point_registry` 的残留行数）；新语义：比对**当前任务点数**——由 `Get-WdTaskPointCount`（**PR #136 后** `scripts/stop-watchdog.ps1:1293`）从**实时任务快照** `tasks[].binding.config.points[]` 总数计算，判据在 **PR #136 后** `watchdog:1509-1518`。`diagnostics.point_count` 降级为 `diagnostics_point_count_observed`（**observed-only，不参与配置门判定**；证据块 `task_config_observation`：拒绝记录 `:614` / arm-report `:1756` / fire summary `:3045`）。**#136 已合入 main（merge `f5f9f56a35a8d3c457a139f8904fcbaf4372e119`），main 上的 `watchdog:156-158` 现为新语义**；本文凡标"PR #136 后"的行号即 main 现状。详见 §3.3、§11-2。
- **v1.5 状态同步声明（#136 已合入）**：PR #136 已合并进 main（**merge `f5f9f56a35a8d3c457a139f8904fcbaf4372e119`**，git merge-base --is-ancestor a11e270 HEAD → 0），**BLOCKED-01 正式关闭**。本版只更新"#136 是否已合入"这一**状态事实**与行号复核，**不改写 v1.3/v1.4 原文**（删除线原句全部保留）。
  - **行号复核（实测 main 5f9f56）**：v1.4 记录的"判据 `watchdog:1509-1518`"实测为**调用点 `:1486`**（`:1512-1537` 是注释块）；证据块 `:614/:1756/:3045` 实测为 **`:618/:1760/:3049`**（+4）。v1.4 原句保留不改，以本注为准。
  - **基线更新**：`main = f5f9f56a35a8d3c457a139f8904fcbaf4372e119`（v1.3 编制基线 `1a382ee`、v1.4 基线 `125d00b` 作为历史记录保留）。
  - **边界**：BLOCKED-01 关闭**不等于**签署执行包已生成，**也不等于**现场执行已获批。
- **v1.3 对齐声明（必须先读）**：本文档**已对齐 `main = 1a382ee` 上 G6/G7 的实际已实现能力**，不再按"设计意图"承诺。三处实质性修订：
  1. **G6 由"设计要点"改为"已实现脚本的实测门"**：`scripts/stop-watchdog.ps1`（3096 行）已实现 `-Mode arm|fire`（**缺省 arm**）、ARMED **零副作用**、FIRED **必须显式授权**、**四项凭证全部为校验**（含两个"被批准对象自身"的文件摘要）、**完整配置比对**、**离线硬门**、**`exit 40 = 授权边界拒绝`**。
  2. **G7 由"待修复前置"改为"已合入 main 的生命周期门"**：`crates/core-api/src/lifecycle.rs` 已落地 attempt 表唯一真值、**六条完整停止判据**（`lifecycle.rs:986-1046`）、并发 Stop 合流（`lib.rs:2262`）、删除屏障与有界等待 `DELETE_PERSIST_WAIT = 10s`（`lifecycle.rs:55`）、**NCK 开机一律不自动恢复**（`recovery.rs:29-37`、`apps/mesad/src/main.rs:184-198`）。因此现场不再"核验修复 PR"，改为**核验部署二进制身份**（`-G7DeployVersion`）+ **外部复核六条判据**。
  3. **证据与判据改为按实现口径**：`watchdog-states.jsonl` / `watchdog-stops.jsonl` / `run-log.md` / `t0.stamp` / `summary.json` / `arm-report.json` **由 watchdog 自动产出**；`summary.json` 把 **调度（schedule）/ 采集（sampling）/ 收敛（convergence）拆成三条独立轴**，并把 **`verdict`（进程处置）与 `trial`（试验结论）分开**。
- 编制声明与口径：本次**只新增/改写本文件**，未改代码/测试/配置/既有文档，未做 git 写操作，未连设备、未启服务、未跑测试或构建；事实给 `文件:行号` 或场景号（`tests/watchdog/run-watchdog-scenarios.ps1` 的 M0–M18），无法由代码/产物确定的标 **待确认**（汇总见 §11）；**不夸大**——30 秒 ≠ Stop 完成时间，读到值 ≠ 业务可用，rev15/rev17 可用是**已证明**的而非假定的。**v1.0 → v1.1 强化执行安全与证据约束；v1.1 → v1.2 拆 G6 两阶段、调度解耦与竞态前置；v1.3 按 main 实际实现逐条对齐；v1.4 仅勘误两处口径（`ApplyPointMap` 不写 registry、`-ExpectedPointCount` 语义）——四版均不新增点位、不扩大采集范围、不改任何代码。**
- 简称：**诊断方案** = `docs/design/nck-s7-job-0x8201-diagnosis-plan.md`；**清单** = `docs/design/nck-828d-phase1-p0-points.md`；**质量调查** = `docs/design/quality-code-observability-investigation.md`；**D1** = `docs/design/pr-d1-quality-code-visibility-design.md`；**watchdog** = `scripts/stop-watchdog.ps1`；**G7 设计** = `docs/design/core-start-stop-race-fix-design.md`；**离线套件** = `tests/watchdog/`（`run-watchdog-scenarios.ps1` + `stub_endpoint.py` + `README.md`）；**生命周期门** = `crates/core-api/src/lifecycle.rs`。

## 1. 申请信息

| 项 | 值 | 来源 / 说明 |
|---|---|---|
| 机床铭牌型号 / 序列号 / 工位编号 | **待现场填写**（型号须与铭牌逐字一致） | 铭牌 + 工位牌照片（§4-G1） |
| 目标 Endpoint | `lx-nck-tags-190-connection`（`828D NCK`，`device_id=lx-nck-tags-190`，driver `sinumerik-nck`，`family=828d`） | `target/validation/828d-r1/rev17-endpoint.raw.json` |
| 当前端点状态 | `desired_running=false`、`state=STOPPED`、`runtime.revision=16`、`runtime.points=4` | 同上；**16/4 是"上次运行视图"**（§2.1-④） |
| 任务存储版本 | `revision=17`、1 个任务（`t1`）、**238 点**、`interval_ms=1000`、`kind=nck.points.v1` | `rev17-tasks.raw.json` |
| 申请人 / 执行人 / 监护人 / 审批人 | **均待填**（四方签字见 §10.2） | — |
| 申请状态 | **未批准（待 §10）** | — |

## 2. 试验目的

**唯一目的**：**在本次现场窗口内复现 T2b 已成功过的同一组 4 点采集**——同一 4 点、同一 `interval_ms=5000`、同一 `max_read_items=1`、运行观察窗口 ≤30 秒，确认"这 4 点在**本次**窗口内可重复读到 GOOD 且新鲜的值"。T2b 原始证据：`target/validation/qc-capture/20261020-093420-338-early-…-points-latest.raw.json`、`…093441-039-prestop-…-points-latest.raw.json`（两次均 4/4 `GOOD`、`value_origin=CURRENT`）。

**明确不做（做了即越权）**：

1. **不扩点、不验证 238 点**：不加第 5 点，不改任何 target（含把 `axis=2` 改成 1/3/4）；不启用 `rev17` 的 238 点配置，不跑全量，不验证 `interval_ms=1000`。
2. **不做性能评估**：不测吞吐/延迟/p95/p99/50K；速率字段即使出现也只能写"推算"（诊断方案 `:448-452`）。
3. **不做语义确认，也不扩到其它轴**：不解释 `5`、`1`、`0.0`、`_N_MPF0` 的业务含义，不做单位判定（清单 `:235-237`）；`axis.position.actual_bcs` 本轮只有 `axis=2`，不得推及 `axis=1/3..8`（清单 `:185`）。

### 2.1 本申请不消除的已知风险（如实登记）

| # | 风险 | 依据 | 本轮处置 |
|---|---|---|---|
| ① | **Stop 非即时**：**典型最坏 ≈75–80 s**，且该值**不是严格上界**（强杀后的第二次 `child.wait()` 无超时） | 诊断方案 `:395`、`:398`；`crates/driver-manager/src/process.rs:279`（宽限）与 `:284`（**无界等待**）；G7 设计 §6.3.1（尾部无界，fail-closed 不加强杀） | 不假设"Stop 立即生效"；Stop 由 watchdog 在 **`T0+30 s`** 无条件发出（`T0` 由 watchdog 自行记录，§5.1、§5.3） |
| ② | **机型身份无法由软件证明**：`family`/`model` 恒 `None`，`NCK_ANCHOR_PENDING` 恒在 | `drivers/sinumerik-nck/src/probe.rs:26`、`:49-64`；`GATE.md:91-92` | 只能靠铭牌 + HMI + X130 + 工位（§4-G1） |
| ③ | **Core/驱动日志不落盘——既定环境事实**（驱动进程无 tracing subscriber；mesad 由外层 `pwsh … 2>&1` 启动，`stdout/stderr` 未重定向） | 诊断方案 `:171`、`:425`；`process.rs:218-219`（`stdout`/`stderr` 均 `Stdio::inherit()`，未重定向到文件） | **不以任何进程日志作为判据**，改用可独立获取的证据（watchdog 的 `watchdog-states.jsonl`/`summary.json`、`points/latest` 原始快照、Stop 请求/响应、`epoch`）；日志缺失本身**不**构成 INVALID、**也不产生 NOTES**（§6.3、§9.3），但**观测不到端点状态即 INVALID**（§6.3） |
| ④ | **`runtime.revision=16` / `runtime.points=4` 是上次运行快照**，恢复 238 点后也不变 | `rev17-endpoint.raw.json`；`endpoint.rs:490-512`（`set_status` 写入 `EndpointStatus{revision, points}`）、`:227-228`（`revision()` 只读 `current_revision`） | 恢复对账只认 `GET /tasks`（§8.3） |
| ⑤ | **值语义未确认**（状态 `5`、参数 `1`、位置 `0.0`） | 清单 `:42`、`:73`、`:235` | 只判 GOOD/新鲜/窗口内推进，**不判语义**；与 ② 一样，软件侧无法自证 |
| ⑥ | **驱动层仍会自行重连**（"不允许重试"只约束人工，不约束驱动/Endpoint 的重连循环） | `crates/driver-manager/src/endpoint.rs:48`（`RECONNECT_BACKOFF_SECS = [1,2,5,10,30]`）、`:458`、`:464-466`；诊断方案 `:610-614`、`:643` | **注意口径变化（v1.3）**：v1.2 把"Stop 之后 Start 迟到生效"列为不消除的 Core 竞态；该竞态的三个窗口（W1/W2/W3）**已由 G7 关闭**（`lifecycle.rs:100`（`stop_gen`）、`:1100-1113`（`ATTEMPT_FAILED_UNKNOWN`/`STOP_IN_PROGRESS`/`CONFLICT`）、G7 设计 `:207`）。**仍然存在的是驱动/端点自身的重连退避循环**，因此判据仍须含"Stop 后无新 `epoch` / 无 `RECONNECTING`"（§3.5、§5.3-⑥） |

## 3. 精确配置

### 3.1 四点完整参数（逐字段）

| # | `point_key` | kind | 逐字段参数 | wire 地址 | dtype（decode） | 真机证据 |
|---|---|---|---|---|---|---|
| 1 | `native.C.1.122.5.1` | `nck_native` | `area=C` · `area_no=1` · `module=122` · `column=5` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `C.1.122.5.1` | `UTF8_NUL_LAST_32` | T2a + T2b |
| 2 | `native.A.1.26.30110.1` | `nck_native` | `area=A` · `area_no=1` · `module=26` · `column=30110` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `A.1.26.30110.1` | `U8` | T2b |
| 3 | `native.C.1.127.13.1` | `nck_native` | `area=C` · `area_no=1` · `module=127` · `column=13` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `C.1.127.13.1` | `U16_LE` | T2b |
| 4 | `axis.position.actual_bcs@axis=i:2,channel=i:1` | `nck_variable` | `variable_id=axis.position.actual_bcs` · `definition_revision=1` · `target={channel:1, axis:2}` · `arguments={}` | `C.1.112.33.2`（定义编译，非手写） | `F64_LE` | T2b |

- 四个 wire 地址**互不相同**（静态核查）：`C.1.122.5.1` / `A.1.26.30110.1` / `C.1.127.13.1` / `C.1.112.33.2`。`decode` 即 dtype；本轮发送 9 个字段（8 参数 + `decode`），全部落在白名单 `ALLOWED`（12 项，`drivers/sinumerik-nck/src/native.rs:147-160`）内，**不发送**可选的 `invalid_value`/`invalid_tolerance`/`unit`，也不许新增字段；点 4 的 `arguments` 必须为空对象 `{}`，不得写 `null` 或省略。
- **v1.3 补充**：这 4 点同时是 watchdog 逐点核对的**被批准对象**——`-ApprovedTaskConfig` 里的 `tasks[].binding.config.points[]` 会按 `point_key` 逐个取出、**整点规范化后逐点比对**（`watchdog:1218-1250`），因此"点位参数被悄悄改一个字段"会在 Start 之前被拒绝（§4.6）。

### 3.2 Connection 参数（来源 `rev17-endpoint.raw.json` 的 `connection`，658 B；**本轮禁止修改任一字段**，需改即作废本申请）

| 字段 | 值 | 字段 | 值 |
|---|---|---|---|
| `host` | `192.168.15.60` | `remote_tsap` | `3332` |
| `port` | `102` | `pdu_length` | `480` |
| `local_tsap` | `1024` | `max_read_items` | **`1`** |
| `timeout_ms` | `1000` | `family` | `828d` |

- **v1.3 补充（口径修正）**：`host` / `port` / `local_tsap` / `remote_tsap` / `pdu_length` **五字段**是 watchdog 的**自动**连接配置比对项（`watchdog:1176-1216`，逐字段、任一不一致即拒绝）。
- **`max_read_items` / `timeout_ms` / `family` 三字段不在 watchdog 自动比对集合内 —— 不得声称它们已被自动验证。** 三者必须按 **§4.2 的人工逐字段核验**处理（对本次窗口的 Endpoint 原始 GET 响应与签署的 `approved-task-config.json` 逐字段比较），**且 §3.4 的历史配置对照不能替代现场实时校验**（历史对照只说明"编制时一致"，不证明"本次窗口一致"）。

### 3.3 Task 参数

`endpoint_id=lx-nck-tags-190-connection`；单任务 `id=t1`；`schedule={interval_ms:5000, mode:"poll"}`；`binding.kind=nck.points.v1`；`binding.config.points` = §3.1 的 4 点，**顺序不变**。

- **v1.3 补充（与 watchdog 凭证的对应关系）**：本轮四点配置的 PUT 发生在 **Start 之前**（§5.2-3），预期 `revision=18`。watchdog 的 `-ExpectedTaskRevision` 与 `GET /api/v1/tasks?endpoint=` 的 `revision` 严格比对（`watchdog:1252-1286`、`:151-154`），`-ExpectedPointCount` 与 `GET /api/v1/endpoints/{id}/diagnostics` 的 `point_count` 比对（`watchdog:156-158`；来源 `crates/core-api/src/lib.rs:1499`）。**这两个值必须取自签署的 G6/G7 执行包，不得在现场按当前状态现填**（§4.6-④、§11-2）。**v1.4 勘误**：`-ExpectedPointCount` 的语义见 §3.3 与 §11-2——**当前任务点数取自实时任务快照**（`GET /api/v1/tasks?endpoint=` 的 `tasks[].binding.config.points[]` 总数），**不是 `diagnostics.point_count`**（后者是 `point_registry` 的残留行数，已由 PR #136 降级为 observed-only，**已合入 main**）。上句 `watchdog:156-158` 描述的是 **main 现状（旧语义）**；合入 #136 后，该判据改由 `Get-WdTaskPointCount`（PR #136 后 `:1293`）给出、判据在 `:1509-1518`。

### 3.4 与 T2b 逐字段对照（同一配置，不是"相似"）

| 对照项 | 本申请 | T2b 证据（`target/validation/t2b-task.json`） | 一致 |
|---|---|---|---|
| 任务骨架（数/id、interval/mode、kind） | 1 / `t1`、5000 / `poll`、`nck.points.v1` | 1 / `t1`、5000 / `poll`、`nck.points.v1` | ✅ |
| 点数 | 4 | 4 | ✅ |
| 点 1–3 各 9 字段 | 见 §3.1 | 三点的 kind/point_key/8 参数 + decode **逐字相同** | ✅ |
| 点 4 的 4 字段 | 见 §3.1 | `variable_id`/`definition_revision`/`target`/`arguments` 逐字相同 | ✅ |
| Connection | §3.2 | `max_read_items=1`、`pdu_length=480`、`remote_tsap=3332` 一致 | ✅ |

**对照方法（可复核）**：本次编制时已以 `t2b-task.json` 为基准逐字段人工核对；执行前须再跑一次机械比对（JSON 规范化后 diff），结果落盘到 §6.1。**任一字段不一致 ⇒ 本轮不是"复现"，停止并重新审批。**

### 3.5 范围冻结（本轮不可协商）

| 项 | 冻结值 |
|---|---|
| 设备 | 指定 SINUMERIK 828D（§4-G1 确认的那一台） |
| 点位 | **T2b 同一组 4 点**（不增不减、不改 target、不改顺序） |
| Poll Interval | `5000 ms` |
| `max_read_items` | `1` |
| 运行观察窗口 | **≤30 秒**（`watchdog:354` 的 `DEADLINE_OFFSET_MS = 30000` 是**脚本内常量，无参数入口**） |
| 读取模式 | **只读**（不 Write、不 Command，Control 保持 `disabled`） |
| 自动重试 | **人工不允许**（任何失败一律 Stop + 记录）；但驱动/端点仍可能自行重连，见下方定义 |
| 扩点 / 自动进入第二轮 | **均不允许** |
| 完成后状态 | **STOPPED**（`desired_running=false`） |

**"不允许重试"的准确含义（三句，不得只引其中一句）**：

1. **人工不主动重试**：不点第二次 Start、不"再试一次看看"、不自动进入下一轮（§7 统一流程、§10.4）。
2. **异常时立即 Stop**（§7 任一触发即停）。
3. **但"不允许重试"≠"不会重连"**：`RECONNECT_BACKOFF_SECS = [1, 2, 5, 10, 30]`（`crates/driver-manager/src/endpoint.rs:48`）**只对"连续失败且从未进入 `RUNNING`"的尝试递增**；一旦某次尝试短暂进入 `RUNNING`，退避下标归零（`endpoint.rs:458`、`:464-466`），实测节奏约 **3.58 s/次且可无限持续**（诊断方案 `:610-614`、`:643`）。因此"不允许重试"只约束**人**，**不构成"Stop 后不会再有连接"的保证**；判据必须包含 **Stop 后核验无新的 `epoch` 变化、无 `RECONNECTING`**（§5.3-⑥），否则判为**未停干净**。**v1.3 澄清**：这与 v1.2 记录过的"Start 迟到生效"竞态**不是同一件事**——后者已由 G7 关闭（§2.1-⑥）。

**变更控制**：冻结表内任一项变更（含"只多读 1 点""5000 改 4000""只延长 10 秒"）都使本申请**立即失效**，须重新编制、重过 §4、重新裁决；执行中发现配置与本文档不符 ⇒ 按 §7-S6 立即 Stop。

## 4. 现场前置门（**G1–G6 六道**；G7 已合入 main，转为**部署身份校验**并入 G6-④；**必须使用本次执行窗口的新证据**）

> **授权不延续**：T2b 的通过结果、上一轮门禁记录、旧截图、**离线套件的历史运行结果**一律不得复用。每道门都须在**本次窗口内**重新取证、重新签字。任一门不通过 ⇒ **不 Start**（记 NO-GO，不得"先跑再说"）。
>
> **六道门的编号（v1.3 定稿）**：**G1 机床身份 / G2 网络参数 / G3 机床状态 / G4 现场监护 / G5 备份与并发 / G6 就绪与凭证（watchdog ARMED）**。第六道门是 **G6**，其判定工具是**已实现**的 `scripts/stop-watchdog.ps1 -Mode arm`（缺省模式），通过判据是**退出码 0（`ARMED_READY`）**，见 §4.6。
>
> **G7 的位置变了（重要）**：v1.2 把 G7 写成"Core Start/Stop 竞态**已由独立代码修复**"这一待核验前置。该修复**已合入 `main = 1a382ee`**（`crates/core-api/src/lifecycle.rs` 1649 行 + `docs/design/core-start-stop-race-fix-design.md`）。因此 G7 不再是"现场要核验的修复"，而拆成两件事：**(a) 部署身份校验**——watchdog 的 `-G7DeployVersion` 对**实际部署的 mesad 二进制**实算 SHA-256（或按受信构建清单核对），并入 §4.6 的四项凭证；**(b) 停止判据由 Core 内置**——六条判据在 `lifecycle.rs:986-1046` 落地，现场只做**外部复核**（§5.3、§8.2）。详见 §4.7。

| 门 | 要求（本次新证据） | 证据形式 | 为什么必须 | 不满足时 |
|---|---|---|---|---|
| **G1 机床身份** | ① 铭牌确认 **SINUMERIK 828D**（型号 + 序列号逐字抄录）② HMI 机床/系统信息页 ③ X130 网络口现场确认（指示灯、端口标签）④ 工位信息与申请一致 | 铭牌照片 + HMI 截图 + X130 照片 + 工位牌照片，**均带拍摄时间** | 软件侧 `family`/`model` **恒为 `None`**（`probe.rs:49-64`）、`NCK_ANCHOR_PENDING` 恒在（`probe.rs:26`、`GATE.md:91-92`）⇒ **无法由软件证明机型** | 停止，NO-GO |
| **G2 网络参数** | ① HMI 实际 IP 仍为 `192.168.15.60`（网络设置页）② `remote_tsap=3332`、`pdu_length=480` 与目标机床一致（**仅在探测已获单独授权时**才可用 `connection-check` 的 `mode=probe` 核对协商 PDU，`core-api:3396`（路由））③ **该地址无其他设备占用** | HMI 网络页截图 +（**已获授权时**）`10-probe.json`（`reachable`、协商 PDU）+ 交换机/网口核对记录（`arp -a` 的 MAC 仅作辅助对照） | 历史上**同 IP 曾换过设备**（诊断方案 `:18`、`:541`）；probe 只证"连到 S7Comm 对端"，**不证身份**（`:185`）；且 probe 是**真实机床 I/O**（见下方"探测授权"） | 未获探测授权 ⇒ 只用 HMI + ARP/MAC + 目视，**不得**由软件主动连机床；仍不符 ⇒ 停止 |
| **G3 机床状态** | 无报警、无急停、安全静止；只读采集不影响当前加工安排（负责人确认） | 机床状态表（上电/急停/驱动使能/参考完成/程序运行/轴是否运动/报警号/操作员口述，诊断方案 `:159`）+ 签字 | 安全前提；状态变化会让"值是否合理"无法判读 | 记录状态后停止 |
| **G4 现场监护** | 有权限监护人在场，具备 **人工 Stop** 与 **网络隔离**（拔**测试主机**网线）能力，并明确窗口时长 | 监护人姓名/电话 + 能力确认签字（§10.2） | **Stop 可能不及时**（**典型**最坏 ≈75–80 s，**非严格上界**，§5.3）；远程 Stop 不是安全保证 | 不开始 |
| **G5 备份与并发** | ① `rev17` 备份已在**本次窗口内重新获取并重算 SHA-256**，且与本文档基线逐字符一致（§8.1"执行窗口内的重新验证"；**不得**直接引用本文档旧哈希当作"已验证"）② **无其他控制端**同时操作该 Endpoint（另一 Mesa 控制台、HMI 侧采集工具、他人 REST 调用） | **本次窗口内**重新 `GET` 的端点/任务原始响应 + 其 `Get-FileHash` 输出 + 与 §6.4 基线的比对记录（两值并列）+ 并发确认（`GET /endpoints` 观察 + 现场唯一操作员） | 防止并发写入把"复现"变成"改配置"；恢复依赖**当时**的备份，不是"上次的备份"；**v1.3 补充**：并发写还会让 `-ExpectedTaskRevision` 失配 ⇒ fire 阶段 `REFUSED_TASKCONFIG_MISMATCH`（exit 40），本轮直接作废 | 哈希不一致或无法重验 ⇒ **NO-GO**（重新评估回退目标）；并发不确认 ⇒ 不开始 |
| **G6 就绪与凭证**（第六道） | 见 §4.6：**`-Mode arm` 退出码 0（`ARMED_READY`）+ `arm-report.json` 落盘**，且凭证汇总 `overall = verified`（arm 模式下 `authorized`/`authz_ticket` 记 `not_checked` 属正常；fire 模式下四项**全部** `verified`） | `arm-report.json` + `run-log.md` + 离线套件 M0–M18 汇总 + 签署的 G6/G7 执行包（两个文件摘要的来源） | 没有独立看门机制，30 秒截止就只是"希望主流程走到那一步"；没有凭证校验，Start 的对象无法证明就是被批准的对象 | 任一项不满足 ⇒ **NO-GO**，不得 Start |

**G2 的探测授权（单独授权，不得默认包含）**：`POST /api/v1/endpoints/{id}/connection-check` 的 `mode=probe` 会启动临时驱动进程并发起**真实机床 I/O**——TCP 连接 `192.168.15.60:102` → COTP → S7 Setup 握手（`drivers/sinumerik-nck/src/probe.rs:3`、`:38-48`；路由 `core-api:3396`，`mode=probe` 语义见 `core-api:727`），**不是**纯本地校验（`mode=validate` 才零 I/O，`connection_check.rs:75`、`:640`）。因此：

- **该探测必须单独授权**，**不得**默认并入"尚未批准的前置检查"，也不得因"只是探一下"而免授权；一次探测 = 一次对机床的真实连接（诊断方案 `:185`、`:572`）。
- **未获授权时，G2 只能依赖 HMI 侧信息 + `arp -a`/MAC 对照 + 现场目视**（X130 指示灯、端口标签），**不得**由软件主动连机床；此时 G2 的"协商 PDU"一项记为"未测（无授权）"，不得写成"已核对"。
- 即便获授权，probe 结果也**只证"连到 S7Comm 对端"**，不证机型身份，不得替代 G1。

### 4.2 人工逐字段核验（`max_read_items` / `timeout_ms` / `family`）—— **不是 watchdog 自动门禁**

watchdog 的连接配置**自动**比对**只覆盖五个字段**（`host` / `port` / `local_tsap` / `remote_tsap` / `pdu_length`，`watchdog:1176-1216`）。**`max_read_items` / `timeout_ms` / `family` 三个字段不在该集合内**，因此**不得声称它们已被自动验证**，必须由现场**人工**核验：

1. 在**本次窗口内**取 **Endpoint 原始 GET 响应**（不裁剪、不摘要化），与签署的 `approved-task-config.json` **逐字段**比较这三个字段；
2. 保存 **原始响应**、**规范化差异**（JSON 规范化后 diff）与**复核签名**（§10.2）；
3. **任一字段缺失、不一致或无法取证 ⇒ NO-GO，不得进入 FIRED**；
4. **§3.4 的历史配置对照不能替代本项** —— 历史对照只证明"编制时一致"，**不证明"本次窗口一致"**。

> ⚠️ **这是人工核验项，不是 watchdog 自动门禁。** 证据清单（§6.4）与待确认清单（§11）均已登记该项；判 NO-GO 时须在 §10.3 记录具体缺失字段。

**关于 MAC `98-fa-9b-59-35-53`（仅辅助对照）**：该值目前**仅来自现场记录，仓库内无留存**（全仓检索无匹配，**待现场补充截图或书面记录**）。即便一致，**单靠 MAC 或 IP 不足以证明一定连接的是指定 828D**（MAC 可换网卡、IP 可复用；诊断方案 `:215-217` 明确"名称/ID 是人工标签，不是身份证据"）。MAC **只能**作为与 HMI + X130 + 工位信息相互印证的一环，**不得单独作为 G1 通过依据**。六道门的签字另附页，格式同 §10.2（确认人 / 确认时间 / 证据文件名；缺一即未通过）。

### 4.6 第六道门（G6）：watchdog **就绪（ARMED）** —— 已实现脚本的**实测门**，不是设计承诺

> **一句话**：**`pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode arm …` 退出码不是 0，就不得 Start。** 第六道门的判据是**一条命令的退出码 + 落盘报告**，不再是"逐条说明脚本应该具备什么"。

**脚本现状（`scripts/stop-watchdog.ps1`，3096 行，已合入 main）**：

| 事实 | 位置 |
|---|---|
| `-Mode arm\|fire`，**缺省 `arm`**；**没有任何参数能表达"arm 之后自动 fire"** | `:309`、`:13-18`、`:107` |
| **ARMED 零副作用**：不启动任何执行流、**不写 `t0.stamp`**、不发 Start、不改端点状态 | `:15-18`、`:58`、`:1511-1513` |
| **FIRED 必须显式**：`-Mode fire -Authorized -AuthorizedBy <人> -AuthzTicket <号>`，缺一项即 `exit 40` 且不进入 FIRED | `:19-32`、`:1466-1483`、`:277` |
| **30 s 截止为脚本内常量**（无参数入口，不接受外部传入的 T0/截止） | `:351-354` |
| **观测 200 ms / 稳定窗口 10 s / 最多 3 次 Stop / 采样盲区阈值 2 s** | `:356`、`:358`、`:360`、`:366` |
| 五个独立执行流（OBS/TIMER/STOP/START/MON），**任一 HTTP 挂起都不推迟 Stop** | `:76-93`、`:2240` |
| `t0.stamp` 在 **FIRED 同一段代码内**由 watchdog 自写（取单调时刻 → 武装 TIMER → 写文件 → 放行 START） | `:2248-2299` |
| 自动产出 `run-log.md` / `t0.stamp` / `watchdog-states.jsonl` / `watchdog-stops.jsonl` / `summary.json` / `arm-report.json` / `stop-selftest.json` | `:418-424` |

**G6-ARMED 的通过判据（全部满足；**Start 之前**判定）**：

| # | 判据 | 证据（本次窗口） |
|---|---|---|
| G6-1 | **`-Mode arm` 退出码 = 0（`ARMED_READY`）**：自检通过 **且**凭证汇总 `overall = verified`（`failing` 为空）。**arm 模式下 `authorized`/`authz_ticket` 记 `not_checked` 属正常**——显式授权是 FIRED 的门，不是 ARMED 的门（`:1458-1460`、`:1463-1476`） | `arm-report.json`（`:424`、`:1624-1628`） |
| G6-2 | 退出码 **1/2/3 任一**都算**不通过**：`ARMED_INPUTS_MISSING` / `ARMED_SELFTEST_FAIL` / `ARMED_ENDPOINT_UNREACHABLE` | 同上（`:268-272`、`:1624-1628`） |
| G6-3 | **离线硬门未触发**：`arm` 只允许**回环地址且端口 ≠ 8132**，否则 `exit 40 REFUSED_NON_OFFLINE`（无参数可绕过） | `:611-628`、`:45-46`、`:274` |
| G6-4 | **测试注入参数未携带**：`-TestHostProbeError` / `-TestAbortFlow` 在非回环或 8132 ⇒ **任何网络动作之前** `exit 40`，`network_actions = 0` | `:654-702`（B1）、`:275-276`；场景 **M13** |
| G6-5 | **离线套件 M0–M18 全部通过**（离线、不连真实机床；`stub_endpoint.py` 假端点） | `tests/watchdog/run-watchdog-scenarios.ps1:14-36`、`tests/watchdog/README.md` |
| G6-6 | `run-log.md` 已含 watchdog PID 与启动时刻、参数快照、截止偏移（脚本 `:433` 即以该行作为"独立进程"的 PID 证据） | `run-log.md` 头部（`:429-448`） |
| G6-7 | 监护人已知晓"Stop 由 watchdog 发出，不由执行人手动决定"；执行人不得关停 watchdog | §10.2 签字备注 |

**四项凭证——全部是"校验"，不是"记录"（`watchdog:705-1506`）**：

| # | 凭证 | 参数 | 校验内容（不是"非空即可"） |
|---|---|---|---|
| ① | 显式授权 | `-Authorized`、`-AuthorizedBy`、`-AuthzTicket` | fire 必填；arm 记 `not_checked`（`:1463-1483`） |
| ② | G6 现场签署 | `-G6Signoff`、`-G6SignedAt` | 非空 **且时间可解析、不在未来**（`:725-740`） |
| ③ | G7 部署身份 | `-G7DeployVersion`、`-G7BinaryPath`、`-ExpectedManifestSha256` | **只接受** `sha256:<64hex>` 或 `manifest:<path>`；与部署二进制**实算摘要**比对；`manifest:` 形式**必填**清单**自身**摘要，且在**解析清单之前**对**原始字节**计算；**git HEAD/describe 仅辅助记录、不参与放行**（`:125-149`、`:882`、`:1120`） |
| ④ | 任务配置 | `-ExpectedTaskRevision`、`-ExpectedPointCount`、`-ApprovedTaskConfig`、`-ExpectedTaskFingerprint`、`-ExpectedApprovedTaskConfigSha256` | 获批配置**文件自身**原始字节摘要（**先摘要、后解析，同一份字节**）→ 只读 `GET` 快照 → **完整配置比对**（`:34-42`、`:1120`、`:1288-1456`）。**`-ExpectedPointCount` 的语义 = 当前任务点数（取自实时任务快照 `tasks[].binding.config.points[]`，见 §11-2）**；main 现状仍比对 `diagnostics.point_count`（`:156-158`），语义变更见 PR #136（**已合入 main**） |

**"完整配置比对"到底比什么（`watchdog:1176-1216`、`:1218-1250`、`:1056-1075`）**：

1. **连接配置逐字段**：`host` / `port` / `local_tsap` / `remote_tsap` / `pdu_length`（任一不一致即拒绝）；
2. **逐点 Key 与参数**：`tasks[].binding.config.points[]` 按 `"<task id>/<point_key>"` 建映射，比对的是**该点位元素的整体规范化 JSON**（比"只比 parameters"更严）；
3. **规范化 tasks 数组的 SHA-256**：键递归排序 + 按 `task.id` 排序 + 紧凑 JSON，与 `-ExpectedTaskFingerprint` 比对（与 python `json.dumps(sort_keys=True, separators=(',',':'))` 逐字节一致）。

**为什么必须有两个"被批准对象自身"的摘要（`watchdog:34-42`，G1/G2）**：只比"清单里的摘要 ↔ 实际二进制"时，**清单与二进制一起被替换**仍互相匹配；而任务配置指纹**只覆盖规范化 tasks 数组、不覆盖连接配置**。因此：清单自身摘要（`-ExpectedManifestSha256`）与获批配置文件自身摘要（`-ExpectedApprovedTaskConfigSha256`）**都必须来自签署的 G6/G7 执行包**，**禁止在现场用 `Get-FileHash` 现算**（现算等于自己给自己授权）。负向场景见 **M17**（G1）/ **M18**（G2）。

**两条必须写明的能力边界（v1.3 新增；**第二条已于 v1.4 勘误**，避免承诺代码做不到的事）**：

- **ARMED 不能对生产实例（`127.0.0.1:8132`）执行**：`arm` 冻结为"回环且端口 ≠ 8132"（`:611-628`），指向 8132 直接 `exit 40`。因此 **v1.2 的 `G6-ARMED-4"对当前已 STOPPED 的端点做一次幂等 stop 自检"只能对离线桩做**（`-SelfTest`，`:209-212`、`:1566-1576`，产出 `stop-selftest.json`）。**生产侧的可达性与配置一致性由 fire 阶段的只读 `GET` 证明**（`:1252-1286`），任何不一致在**发出 Start 之前**以 `exit 40` 拒绝（`:285`）。**不得**把"离线桩自检通过"写成"生产通路已验证"。
- **`-ExpectedPointCount` 的语义（v1.4 勘误）**：**现行判据 = 当前任务点数**，取自**实时任务快照** `GET /api/v1/tasks?endpoint=` 的 `tasks[].binding.config.points[]` 总数（`Get-WdTaskPointCount`，**PR #136 后** `watchdog:1293`；判据 **PR #136 后** `watchdog:1509-1518`），**不是** `diagnostics.point_count`。
  - `diagnostics.point_count` 的真实来源是**持久化 `point_registry` 的活跃行数（`deleted=0`）**（`crates/core-api/src/lib.rs:1499` → `config-store/src/lib.rs:2459-2473`），其**唯一写点**是 `assign_point_ids`（`config-store:2344-2456`），由 `StorePointIdSource::assign`（`crates/driver-manager/src/endpoint.rs:206-224`，调用点 `:213`）在 `run_config_flow`（`endpoint.rs:943-945`）中调用——**写入时机在 `ConfigureTasks`（`:914`）之后、`ApplyPointMap`（`:968-985`）之前**；`ApplyPointMap` 只下发映射，**不写 registry**。故该计数只反映"上次走完 ConfigureTasks 的那次 Start"的残留。
  - **v1.3 原文（勘误前，保留以备追溯）**：~~`-ExpectedPointCount` 比对的是 `point_count`，其来源是持久化 `point_map`（`crates/core-api/src/lib.rs:1499` → `config-store/src/lib.rs:2459`），不是任务配置里的点数；`point_map` 由 Start 时的 `ApplyPointMap` 写入。~~ **该理解已被 PR #136 修正**（`dfc81e0`，**已合入 main，merge `f5f9f56a35a8d3c457a139f8904fcbaf4372e119`**）：现行判据取自实时任务快照，`point_map` / `point_registry` 的残留值**降级为观测**（`diagnostics_point_count_observed`，observed-only，不参与配置门判定）。该值仍必须由签署包给定，并在 fire 的只读阶段实测确认（§11-2）。

**离线套件 M0–M18（`tests/watchdog/run-watchdog-scenarios.ps1:14-36`）—— 证明工具行为，不构成现场授权**：

| 场景 | 覆盖的判定 | 与本轮的关系 |
|---|---|---|
| **M0** 正常路径（Start → T0+30 s Stop → 停干净） | 调度 PASS + 采集 VALID + 收敛 CONVERGED | 正式执行的**形状**参照 |
| **M1** 观测阻塞 36 s（Stop 仍准时；采集判 INVALID） | **"调度 PASS + 采集 INVALID" 可同时成立**（F3 的核心证明） | §9.1 两轴独立的依据 |
| **M2** Start 超时 20 s | 不等待 Start 返回，仍按截止 Stop（`exit 3`） | §5.3 在途 Start |
| **M3** 端点运行中失联（Stop 发不出去） | 不静默退出，收敛判"无法确定" | §5.3 隔离流程 |
| **M4** Stop 自身挂起 15 s | 观测不被 Stop 阻塞（双向解耦） | §5.1-6 |
| **M5** 迟到 Start 竞态（Stop 后翻回 RUNNING） | `RACE_START_AFTER_STOP`（`exit 4`） | 回归防线 |
| **M6** 首次通信失败即停（0x8201/early eof） | 首次即停，不设宽限 | §7-S1/S2 |
| **M7** ARMED 误启动防护（等待 >1.5 s 也绝不 Start） | **ARMED 零副作用** | §4.6-G6-1 |
| **M8** OBS 心跳失效 | 提前 Stop + 采集 INVALID + 截止调度仍工作（`exit 2`） | §9.1 采集轴 |
| **M9** HostChecks 查询失败 | 判"无法确定"，不得判停净、不得 `exit 0`（`exit 9`） | §5.3-④⑤ 三态 |
| **M10** 工作线程异常退出 | 不静默继续，MON 接管截止调度（`exit 8`） | §5.3 兜底 |
| **M11/M12** 授权门正向/负向 | 四项凭证齐全且校验通过 ⇒ 放行；缺一项/配置不一致 ⇒ `exit 40` 且不发 Start | §4.6-①②③④ |
| **M13** 测试注入参数的离线边界（B1 负向） | 非回环/8132 + 注入参数 ⇒ `exit 40`、`network_actions = 0` | §4.6-G6-4 |
| **M14** G7 部署身份不可验证（B2 fail-closed） | 只给 git 版本标识/二进制缺失/清单缺失 ⇒ 拒绝，**不降级为 git HEAD** | §4.6-③ |
| **M15/M16** 完整任务配置不匹配/匹配（B3） | 连接配置（PDU）/点位参数/指纹任一不一致 ⇒ 拒绝；全部一致 ⇒ 放行 | §4.6 完整配置比对 |
| **M17/M18** G1/G2 负向 | 清单/获批配置**自身**未被独立验证（缺失、字节被改、清单与二进制一起被换）⇒ 拒绝 | §4.6 两个摘要 |

- **纪律**：**离线套件通过 ≠ 现场授权**。M0–M18 全部在 `127.0.0.1` 的假端点上跑（`stub_endpoint.py`），**不连接、不证明任何真实机床行为**；它证明的是"工具在已知故障下不撒谎"，**不证明机床会停**。真机停止时刻仍由 §5.3 的外部复核另行观测。
- **不满足任一项 ⇒ NO-GO**，记入 §10.1；**不得**"先 Start 再说，watchdog 边跑边调"。

### 4.7 G7：已合入 main 的**生命周期门**（不再是"待修复前置"）+ **部署身份校验**

> **口径变化（v1.3）**：v1.2 的 §4.7 要求现场核验"修复 PR/commit、修复说明、对应测试"，并声明"本文档只能兜底，不能消除竞态"。该修复**已合入 `main = 1a382ee`**，因此本节改为两件事：**(a) 证明"跑的是含 G7 的那一版二进制"**（watchdog 凭证 ③）；**(b) 说明现场需要复核的外部面**（因为 Core 内部判据不对外暴露）。

**(a) 部署身份校验（并入 §4.6 凭证 ③）**：

| 判据 | 说明 | 位置 |
|---|---|---|
| G7-1 | `-G7DeployVersion` **只接受** `sha256:<64hex>`（与 `-G7BinaryPath` 实算摘要比对）或 `manifest:<path>`（从受信清单读期望摘要再比对） | `watchdog:125-138`、`:882` |
| G7-2 | 走 `manifest:` 时**必填** `-ExpectedManifestSha256`（清单**自身**原始字节摘要，解析前计算） | `watchdog:140-149` |
| G7-3 | 两种形式都不可用 ⇒ `unverified` ⇒ **拒绝进入 FIRED**（fail-closed，**不得**降级为"用 git HEAD 代替"）；git HEAD/describe 只记入 `summary.json`/`t0.stamp` 作辅助 | `watchdog:129-132`、`:802-827` |
| G7-4 | 给出 `-G7BinaryPath` 时**只**比对该文件，不存在即 `unverified`（不回退到其它二进制"凑出通过"） | `watchdog:134-138` |

**(b) Core 已具备的能力（现场只做外部复核，不重复实现）**：

| 能力 | 实现 | 位置 |
|---|---|---|
| **attempt 表 = 生命周期唯一真值**；`TeardownDone(gen)` 有因果链证据（条目只在 spawn 前创建、只由 attempt 任务自身按 gen 匹配移除） | 模块分层声明与锁序纪律 | `lifecycle.rs:1-27`；G7 设计 §6.3.1 |
| **六条完整停止判据**（同一 tick 内跨来源取数，连续 2 个 250 ms tick 全过） | `await_settled` | `lifecycle.rs:986-1046`（判据表达式 `:1011-1017`） |
| **并发 Stop 合流**（同一次 teardown，多个请求拿到同一结果） | `join!(handle.await_result(), persist)` | `crates/core-api/src/lib.rs:2262`；G7 设计 §5.2 |
| **删除屏障 + 有界等待** | `DELETE_PERSIST_WAIT = 10s`（超时 ⇒ 409 `DELETE_PERSIST_IN_PROGRESS`，不删行） | `lifecycle.rs:55`、`:643-681` |
| **屏障期 Stop 照常落盘**（200 ⇔ 意图已落盘，或行已不存在） | P0-E-1；确定性用例断言"200 之前 DB 已为 false" | `lifecycle.rs:1461-1523`；G7 设计 §6.13 |
| **NCK 端点开机一律不自动恢复**（精确匹配 `driver_id="sinumerik-nck"`） | `boot_auto_restore_allowed()`；判定在正向凭证**之前** `continue` | `crates/core-api/src/recovery.rs:29-37`；`apps/mesad/src/main.rs:184-198` |
| **异常 attempt 隔离** `ATTEMPT_FAILED_UNKNOWN`（Start 409 / Stop 500） | 前缀映射到状态码与 code | `lifecycle.rs:1100-1102`；`tests/driver-contract/tests/stop_lifecycle.rs:68-83` |
| **冻结 Stop 错误码 8 个**（原 7 个 `EVENT_*` + `ATTEMPT_FAILED_UNKNOWN`） | `assert_frozen_stop_code` | `stop_lifecycle.rs:68-83` |
| **`desired=false` 落盘失败不再静默成功**：500 `STOP_INTENT_NOT_PERSISTED`（**优先于 teardown 结果**） | 冻结文案常量 | `crates/core-api/src/lib.rs:2285-2286`；`lifecycle.rs:46-47` |

**(c) 在途 Start 规则（不因修复而删除）**：**执行前**存在**遗留的在途 Start**（已发出、未收到明确结果）⇒ **禁止现场执行**；**执行中/裁决时** Start 结果仍不确定 ⇒ **不得**认为已安全进入采集、**不得**判 PASS（watchdog 侧对应 `startUncertain` → `exit 3`，`watchdog:2784`、`:2859`）。

**(d) 保留的回归防线**：即便 G7 已合入，本轮仍保留 §5.3 的竞态观测与 `RACE_START_AFTER_STOP` 记录（watchdog 的 `race.count`，`:2947`、`:2988`；场景 **M5**）。

## 5. 执行时间窗

### 5.1 窗口定义（**ARMED 与 FIRED 是两个独立阶段**）

| 项 | 值 |
|---|---|
| 计划开始 / 结束（本地时间） | **待填** / **待填**（结束须含 Stop 与恢复，留 ≥5 min 余量） |
| `interval_ms` | `5000` |
| **阶段 A：ARMED（就绪）** | `-Mode arm`（缺省）——只自检、只报告、**零副作用**（`:13-18`）。**判定时点：Start 之前**。通过 = `exit 0` |
| **阶段 B：FIRED（实际启动）** | `-Mode fire -Authorized …` —— 四项凭证**校验通过** + 两个文件摘要**通过** + 只读 GET 的**完整配置比对通过**，才取 `T0` 并放行 Start（`:19-32`、`:2248-2299`） |
| 运行观察窗口 / 截止 | **≤30 秒**（硬上限）；截止 = **watchdog 自记的 `T0` + 30 s**（`DEADLINE_OFFSET_MS = 30000`，脚本内常量、无参数入口，`:354`），到点由 **watchdog 无条件**发 Stop（**不等主流程、不等抓证完成**）。**该 30 s 是"Stop 指令发出时刻的调度目标"，不是"设备已停止"的保证，也不是"Stop 完成时间"**（§5.3、`:63-66`、`:2269`） |
| 计时起点 `T0` | watchdog **发出 Start 的同一段代码内**、以**自身单调时钟**记录（含建连时间，不因"还没 RUNNING"而延后截止）；**只能由 watchdog 在 FIRED 时生成**，人工/主流程/抓证脚本**不得预先填写或修正**；Start 是否返回、何时返回都不影响截止（`:2248-2299`） |
| Stop 发出者 | **watchdog（独立进程）**；执行人可另发人工 Stop（幂等），但**不得**替代或关停 watchdog |
| 预期批次 | 5–6 轮（30 s ÷ 5 s；首轮相位取决于建连耗时，**不承诺**恰好 6 轮） |

**watchdog 实现要点（已落地，现场只需按此核对命令与产物；`watchdog:68-104`）**：

1. **独立进程**：`pwsh -NoProfile -File scripts/stop-watchdog.ps1`，**不 import** 驱动/Core 代码，只经 REST 交互；`-BaseUrl`/`-EndpointId`/`-OutDir` 显式给。
2. **计时源与 T0 生成**：`[System.Diagnostics.Stopwatch]` 单调时钟（`:403-406`）；`T0` 由 watchdog 在 FIRED 时自写 `t0.stamp`（`:2251-2293`）。
3. **职责**：watchdog **独占** Start 与截止 Stop；Start **只发一次、不重发**，超时/异常只记录、**不阻塞** 30 s 计时（`:82`）。
4. **截止动作**：到达 `T0+30 s` **立即**发 `POST …/stop`（`-StopTimeoutSec 120`），**不等待** Start 响应、**不等待** `capture-points.ps1`。**Stop 的发出不依赖任何 HTTP 请求完成**（`:76-93`）。
5. **观测**：每 **200 ms** `GET /api/v1/endpoints/{id}`，逐行追加 `watchdog-states.jsonl`（`t_local`、`state`、`desired_running`、`epoch`、`detail`、`http_status` 等），覆盖 `T0` 前预热至最终 `STOPPED` 后 **10 s**；总上限 **120 s**（`:196-199`、`:356-358`）。观测在**独立执行流**，其阻塞/失败不得影响计时与 Stop。
6. **双向解耦（硬性）**：计时/Stop 调度与状态观测、Stop 请求三者互不阻塞；已由 **M1**（观测阻塞 36 s 而 Stop 准时）与 **M4**（Stop 挂 15 s 而观测照常）验证。
7. **退出**：判据满足 ⇒ `exit 0`；否则按 `:267-305` 的退出码语义处置，并打印隔离流程由监护人执行。

### 5.2 执行顺序（只读，逐步留证）

1. **阶段 A（ARMED）**：`-Mode arm` 对**离线回环桩**执行（`-BaseUrl http://127.0.0.1:<离线桩端口>`），要求 **`exit 0` + `arm-report.json`**；如需通路自检再显式 `-SelfTest`（产出 `stop-selftest.json`）。**不得**指向 8132（会 `exit 40`，`:611-628`）。
2. §4 **G1–G5 签字**；抓**运行前**基线：`GET /endpoints/{id}`（完整保存）、`GET /tasks?endpoint=`、`GET /points/latest`（**完整原始响应**，§6.3）。
3. `PUT /api/v1/tasks/lx-nck-tags-190-connection`，body = `{"tasks":[ …§3.1 四点… ]}`（结构与 `t2b-task.json` 同形，预期新 revision **18**）。保存会触发一次 Descriptor 校验（临时驱动进程，仅 handshake + GetDescriptor），**不建立到机床的 NCK 连接**；运行中保存被 `CONFLICT` 拒绝。
4. **阶段 B（FIRED）**：`-Mode fire -Authorized -AuthorizedBy … -AuthzTicket …`，携带 §4.6 的**四项凭证**与**两个文件摘要**。脚本按顺序：**静态凭证（本地，`network_actions = 0`）→ 只读 GET 快照 + 完整配置比对 → 取 `T0` 写 `t0.stamp` → 放行 Start**（`:705-1506`、`:2248-2299`）。**任一凭证/摘要/配置不一致 ⇒ `exit 40`，不发 Start**。执行人**不得**另发 Start，也**不得**预填 `T0` 或截止时间。
5. `T0+10 s±2 s` 抓 **`early`**；`T0+25~28 s` 抓 **`prestop`**（**必须在 Stop 之前**）。抓证进程从 watchdog 输出的 `t0.stamp` **只读**取 `T0` 以对齐时刻（**不参与** Stop 决策）。**抓不完也不得推迟 Stop**——抓证失败只影响证据，不影响截止。
6. `T0+30 s` **watchdog 无条件** `POST /endpoints/{id}/stop`；执行人**不得**以"prestop 还没抓完"为由推迟或取消。
7. **按 §5.3 复核"停干净"**：watchdog 自行产出 `six_items` 三态 + `convergence` + `axes` + `verdict` + `trial` + `exit_code`（`summary.json`，`:2938-3000`）。**只有 `exit_code = 0` 才可判 PASS**（`:300`）。
8. 恢复 `rev17`（§8.2）→ 对账（§8.3）→ 保持 STOPPED（**NCK 重启也不会自动拉起，见 §8.2-6**）。

### 5.3 Stop 时间约束（**本轮最关键的一条，不得改写**）

> **30 秒是"停止指令的发出时刻保证"，不是"物理停止保证"，也不是"Stop 完成时间"。** watchdog 在 `T0+30 s` 无条件发出 Stop，不依赖主流程是否走到那一步；但**watchdog 能按时发出 Stop ≠ 设备读取在 30 秒时立即结束**——Core 的 Stop 路径仍可能有长时间等待（**典型 75–80 s，尾部无界**，见下），**实际停止时刻必须由"停干净"判据另行观测**。**不得**把 30 秒写成"30 秒内一定停住"或"30 秒后设备已安全"。watchdog 自身也把该值一律写作 `deadline`（调度目标），并另列 `stops[].t_dispatch` 与 `convergence`（`:63-66`、`:2915`）。

- **Stop 完成时间：典型最坏 ≈75–80 秒，但这不是严格上界**（诊断方案 `:357`、`:386`）：teardown 小计 ≈35 s（post 5 s + terminate 5+5 s + reader 5 s + drain 15 s）+ **取消覆盖不到的**建连/配置闭环 ≈46 s。**该估算不含"强杀后等待子进程回收"的无界时间**：`DriverProcess::terminate` 的宽限只包住第一次 `child.wait()`（`process.rs:279`），**强杀后的第二次 `child.wait()` 没有 timeout**（`process.rs:284`）；`terminate_after_shutdown` 同样（`:301-311`）。G7 设计 §6.3.1 明确保留这一尾部无界，并以 **fail-closed**（条目永不移除 ⇒ Start 一直 409）作为安全方向，**不引入自动强杀**。因此 **75–80 s 只能写作"典型最坏估计"，不得作为超时判据、不得当作安全承诺**。
- **客户端超时 ≥120 s 只是"客户端愿意等"**（诊断方案 `:351`），**不等于服务端保证 120 s 内停住**；**"≥120 s" 是等待上限，不是完成保证。**
- **超过 120 s 仍未 `STOPPED` ⇒ 按隔离流程处理，不再等**：① 阻断出站（防火墙 Block `192.168.15.60:102`，对机床零风险）→ ② 杀驱动进程（会触发重连，故**必须先阻断**）→ ③ 杀 mesad（须先确认 `desired_running=false`）→ ④ **物理隔离只拔测试主机网线**，不得擅自断产线交换机口；**每一步后复核状态**（`state`/`desired_running`/驱动进程/到 `:102` 的连接），全过程记入 §10.3。
- **三条硬性纪律（逐字保留，第 1 条已指向 watchdog）**：
  1. **到达运行截止时间必须立即发出 Stop（由 watchdog 无条件发出），不得为了等 `prestop` 快照而延长采集。**
  2. **提前触发停止条件时，立即执行停止流程**（§7），不等到截止时间。
  3. **Stop 未能及时完成时，由现场人员按批准的安全处置流程处理，不得假定远程 Stop 必然即时生效。**

#### 5.3.1 "停干净"的两层判据（**v1.3 必须分开写，不得混为一谈**）

**第一层：Core 内部六条判据（进程内、内存视图，`lifecycle.rs:986-1046`）** —— 只证明"**Core 认为自己停了**"，**不是**现场判据：

| # | 判据 | 实现信号 | 位置 |
|---|---|---|---|
| 1 | **当前代次为 `STOPPED`** | 同一加锁内取 `(gen, state)` 原子对，且 `snapshot_gen == current_gen` | `lifecycle.rs:1011-1012`；G7 设计 §7.1-1 |
| 2 | **`desired_running=false` 且本次 Stop 的落盘已确认** | `stop_gen == target_gen` && `stop_persist_confirmed_gen == Some(target_gen)` && `stop_persist_inflight.is_none()`（**不得**用 `desired_seq` 代替） | `lifecycle.rs:1006-1009` |
| 3 | **无在途 Start / Stop / 意图落盘** | `inflight_starts == 0`（Start 票据）+ attempt 表 `phase ∈ {Stopping, FailedUnknown}` + `stop_persist_inflight` 集合 + `start_persist_inflight` | `lifecycle.rs:1017`、`:99-153` |
| 4 | **无未完成的 attempt（含孤儿）** | `entry_absent`（该 `endpoint_id` 在 attempt 表无条目；隔离态 `FailedUnknown` 也算有条目 ⇒ 不通过） | `lifecycle.rs:1013`、`:966` |
| 5 | **驱动进程与 TCP 会话收敛** | `TeardownDone(gen)`（蕴含 `child.wait()` 已返回）+ `LIVE_CHILDREN` | `process.rs:30`；G7 设计 §7.1-5（**reader 结果尚未纳入判据，属已知缺口**） |
| 6 | **经过规定的稳定观察窗口** | `SETTLE_TICKS = 2 × SETTLE_TICK = 250 ms`（合计 500 ms），任一 tick 失败或 gen 变化即窗口重置 | `lifecycle.rs:42-43`、`:1018-1022` |

- **禁止的等价替换**：不得用 `is_running()==false`、`desired_running==false`、`state=="STOPPED"` 中的任一条单独（或任意组合）代替上面 6 条（G7 设计 `:1184`）。
- **`current_gen == None`（从未有 attempt / 重启后）⇒ `NotSettled{NoGenBaseline}`**：**无基准 ≠ 成立**，不得当作"已停干净"（`lifecycle.rs:999-1004`）。
- **作用域写死**：这 500 ms 检查**只用于 Core 内部状态收敛**（测试断言、日志水位、诊断），**不能替代现场 10 s 连续稳定观察**（G7 设计 §7.5、`:41`）。

**第二层：现场"停干净"六项（外部可观测面，watchdog 自动复核，三态判定，`watchdog:2537-2776`）** —— 这才是**现场判据**：

| # | 项 | 三态判定（`converged` / `not_converged` / `indeterminate`） | 位置 |
|---|---|---|---|
| ① | **Stop HTTP 结果** | 拿到 200/500 ⇒ `converged`（500 是 drain 失败但 `desired_running` 仍落 false）；一个响应都没拿到 ⇒ `indeterminate`（**不是"没停"**）；拿到其它明确响应 ⇒ `not_converged` | `watchdog:2713-2725` |
| ② | **`state=STOPPED`** | 最终观测值；无任何有效样本 ⇒ `indeterminate` | `:2732-2739` |
| ③ | **`desired_running=false`** | 同上 | `:2732-2739` |
| ④ | **驱动进程为空** | **只认目标驱动**（按 `driver_id` 精确匹配 `mesa-driver-<id>` / `--driver-id <id>`），其它 mesa-driver 进程单列 `excluded`；**查询失败/权限不足/工具缺失 ⇒ `indeterminate`** | `:2580-2615` |
| ⑤ | **到设备 TCP 无 ESTABLISHED** | 只统计到 `device host:port` 的连接；**3389/RDP 单列并排除**；无法枚举 ⇒ `indeterminate` | `:2617-2707` |
| ⑥ | **稳定窗口 10 s** | 首次 STOPPED 后连续 10 s 无新 `epoch`、无 `RECONNECTING`；无样本 ⇒ `indeterminate` | `:358`、`:2759-2770` |

- **三态优先级**：`not_converged > indeterminate > not_verified > converged`（`:2772-2776`）。**`indeterminate` 绝不等于"已停净"**（`:2538-2539`）。
- **`-SkipHostChecks` 会把 ④⑤ 记为 `not_verified`**（`required=false`），整体判 `NOT_VERIFIED`、`exit 7`，**永远不可能被判成"停干净"**（`:217-219`、`:2741-2753`）。**本轮不得使用 `-SkipHostChecks`。**
- **看到 500 不等于没停住**（drain 失败也 500，但 `desired_running` 仍落 false）；**看到 200 也不等于停干净**（仍有驱动进程/连接/稳定窗口三项）。

#### 5.3.2 竞态与在途 Start 的现场口径（v1.3 修订）

- **Core 侧竞态已关闭**：W1（Stop 落进 Start 构造窗口）由票据代次 + `SUPERSEDED_BY_STOP` 关闭；W2（Start 落进 Stop 拆解窗口）由"条目存在即保护" + `STOP_IN_PROGRESS` 关闭；W3（双 Start 并发）由 insert-before-spawn 结构性关闭；W4（删除落在意图落盘窗口）由删除屏障关闭（G7 设计 `:207`）。**因此 v1.2 记录的"Stop 之后 Start 迟到生效"不再是本轮的主要风险来源**。
- **仍在的风险是驱动/端点自身的重连退避**（§2.1-⑥），判据必须含"Stop 后无新 `epoch` / 无 `RECONNECTING`"（即第二层判据 ⑥）。
- **watchdog 侧的竞态处置**：Stop 发出后若观测到 `RUNNING` 或 `epoch` 变化 ⇒ **再次 Stop**（最多 `MAX_STOPS = 3`，`:360`），并记 `RACE_START_AFTER_STOP`（`summary.race`，`:2947`），最终 `exit 4`。
- **在途 Start**：① **执行前**存在遗留的在途 Start ⇒ 禁止执行（§4.7-c）；② **执行中** watchdog 按设计**不等** Start 响应，因此"Start 已发出"**不得**当作"已安全进入采集"；③ **裁决时** Start 仍无明确结果（`startUncertain`）⇒ **不得判 PASS**（`exit 3`，`:2784`、`:2859`）。
- **判据以"最终观测到 `STOPPED` + `desired_running=false` + 六项全 `converged`"为准**，**不是**"Stop 请求已发出"。

### 5.4 冷却与总量

单次 ≤5 min、单日累计 ≤30 min、**≤5 次启停**、两次运行间隔 **≥30 s**（等旧连接彻底关闭）——全部出自诊断方案 `:440`。本轮实际 ≤30 s + 1 次启停，远低于上限。**超窗**（计划结束仍未恢复完成）⇒ 本轮作废，记 NO-GO/INVALID，**不得**"延长窗口再跑一次"。

## 6. 证据保存

### 6.1 目录与命名（**watchdog 自动产出，人工不得预填**）

`target/validation/828d-r1/round1-<UTC 时间戳>/`（`/target/validation/` 已被 `.gitignore:55` 忽略，不进仓）。文件名前缀 `<UTC>-<label>-<endpoint>`，`label ∈ {early, prestop, pre, post, probe, log(可选)}`。

**watchdog 在该目录自动产出的文件（`watchdog:418-424`，均为脚本写入，非人工填写）**：

| 文件 | 内容 | 谁写 |
|---|---|---|
| `t0.stamp` | `T0`（本地/Unix ns/单调 ms）、`deadline`、偏移、时钟源、`generated_by`、凭证快照、`deadline_semantics`（明确"不保证第 30 s 已物理终止"） | **仅 watchdog 在 FIRED 时刻**（`:2261-2293`）。**人工不得预填、不得事后修改** |
| `run-log.md` | watchdog 运行日志：PID、启动时刻、全部参数、凭证校验逐项结果、事件流（含 FIRED/Stop/竞态） | **watchdog**（`:380-389`、`:429-448`）。**⚠️ 启动即以 `WriteAllText` 覆盖**（`:448`），所以**人工现场记录不得写在同名文件里** |
| `watchdog-states.jsonl` | 每 200 ms 一条端点快照（`t_local`、`state`、`desired_running`、`epoch`、`detail`、`http_status`） | watchdog（`:392-400`、`:356`） |
| `watchdog-stops.jsonl` | 每次 Stop 的发出时刻、`http_status`、`deviation_ms`、`reasons` | watchdog（`:421`、`:2317-2326`） |
| `summary.json` | 凭证汇总、`t0`、`start`、`stops`、`six_items`、`convergence`、`observation`、**`axes`（schedule/sampling/convergence）**、`trial`、`verdict`、`exit_code` | watchdog（`:2875-3004`） |
| `arm-report.json` | ARMED 自检报告（探活结果 + 四项凭证逐项 state + 边界判定 + `verdict`） | watchdog（`:424`、`:1653`） |
| `stop-selftest.json` | 通路自检结果（仅 `-SelfTest` 时，仅 arm 模式） | watchdog（`:423`、`:1566-1576`） |

- **人工现场记录必须另起文件名**（建议 `site-log.md`），字段沿用诊断方案 `:478`（谁在何时做了什么、发了什么 Stop、异常处置）。**它不是进程日志**，见 §6.3。**不得**把人工记录写入 `run-log.md`（会被 watchdog 覆盖）。
- `log` 标签仅在已把 mesad 控制台重定向到文件时使用，**可选、非判据**。

### 6.2 运行中原始快照（两次，缺一即 INVALID）

| 标签 | 时机 | 内容 | 工具 |
|---|---|---|---|
| `early` | `T0+10 s±2 s`（**尽早**，不要等最后） | `when.json`（本地墙钟 + Unix ns）、`state.json`、`points-latest.raw.json`（**逐字节完整响应，不裁剪不摘要**，含**全部端点**）、`state-after.json` | `target/validation/capture-points.ps1 -Label early`（`:15-31`） |
| `prestop` | `T0+25~28 s`，**Stop 之前** | 同上四件套 | 同上 `-Label prestop` |

- **保存顺序（强制）**：先**原样落盘完整响应**（`/points/latest` 是全端点响应，`core-api:1584-1588`），**再**在分析阶段按 `endpoint_id` 筛出目标端点 4 条（§6.3）；**不得**只保存筛选后的 4 条——原始全量是"没读错端点、没悄悄扩点"的唯一凭证。
- **目录参数必须显式给**：脚本默认输出到 `target\validation\qc-capture`（`capture-points.ps1:8`），与 §6.1 的 `828d-r1/round1-<ts>/` 不同；执行时必须显式 `-OutDir target\validation\828d-r1\round1-<UTC 时间戳>`，否则证据不落在本轮目录（**且会与 watchdog 的 `-OutDir` 分叉**）。
- 两次抓取之间**必须**看到批次推进（T2b 实测 25.001 s = 5×5000 ms）；两次 `timestamp_ns` 相同 ⇒ 采集停滞，按 §7-S6 停。抓取期间状态变化（`state.json` 与 `state-after.json` 不一致）⇒ 该样本**作废**（`capture-points.ps1:36-38`）。
- **抓证失败不推迟 Stop**：`early`/`prestop` 任一次失败只记证据缺口（该样本缺失 ⇒ 按 §9 判 INVALID），**截止时刻仍由 watchdog 无条件 Stop**。

### 6.3 端点状态、快照与日志口径

**端点状态（独立证据，判据的唯一来源）**：

- `GET /endpoints/{id}`：运行前/后各一次，**完整保存**（含 `state`、`desired_running`、`runtime.epoch`、`runtime.revision`、`runtime.points`、`connection` 全字段）。
- **watchdog 状态时间序列**：`watchdog-states.jsonl`，每 200 ms 一次。**这是"Stop 是否生效""是否又重连"的唯一判据来源**（§5.1-5）。
- `GET /tasks?endpoint=`：运行前/恢复后各一次，记录 `{endpoint_id, revision, tasks}`。
- `GET /points/latest`：**先完整保存原始响应（全端点、不裁剪、不摘要）**，**再**在分析阶段按 `endpoint_id` 筛出目标端点 4 条（每条 latest 点自带 `endpoint_id`，`snapshot.rs:99`），记 `quality`/`quality_code`/`value_origin`/`timestamp_ns`/`value`。响应顶层 `count` 是**全端点**总数，须原样记录、**不得**当作目标端点条数；目标端点条数由筛选结果单独统计。
- **Stop 请求/响应**：`watchdog-stops.jsonl`（watchdog 侧）+ 人工记录一份；竞态重发时逐次记录（§5.3）。

**`summary.json` 的三条判定轴与两个结论字段（`watchdog:2966-3000`）—— 本轮的判定必须按此引用，不得自造口径**：

| 字段 | 含义 | 取值 |
|---|---|---|
| `axes.schedule.verdict` | **调度轴**：Stop 请求是否在 `T0+30 s` 准时发出（只看截止 Stop 的 `|deviation|`，不看采集、不看收敛） | `PASS`（≤1000 ms）/ `FAIL` / `UNKNOWN`（`:2805`） |
| `axes.sampling.verdict` | **采集轴**：观测证据是否完整（心跳失效 / 盲区 > 2000 ms / 截断 / 无样本 一律 INVALID） | `VALID` / `INVALID_OBSERVATION_GAP` / `INVALID_OBS_TRUNCATED` / `INVALID_NO_SAMPLES`（`:2819-2822`） |
| `axes.convergence.state` | **收敛轴**：设备/进程是否真的停净 | `CONVERGED` / `NOT_CONVERGED` / `INDETERMINATE` / `NOT_VERIFIED`（`:2776`） |
| `verdict` | **进程处置结论**（决定退出码 0–9） | `STOPPED_CLEAN` / `NOT_STOPPED` / `STOPPED_OBS_INVALID` / `STOPPED_START_UNCERTAIN` / `STOPPED_WITH_RACE` / `STOPPED_BY_EARLY_TRIGGER` / `STOPPED_WITH_ANOMALIES` / `HOSTCHECKS_UNVERIFIED` / `WORKER_ABORTED` / `CONVERGENCE_INDETERMINATE` |
| `trial` | **试验结论**（"这轮证据能不能用于验收"） | `PASS` / `NO_GO` / `INVALID_SAMPLING` / `UNVERIFIED` / `ABORTED` / `NO_GO_ANOMALY`（`:2866-2871`） |

- **`verdict` 与 `trial` 必须分开写**：`verdict` 回答"进程该怎么处置"（退出码），`trial` 回答"这轮数据能不能用"。**不得**用退出码代替试验结论，也**不得**用 `trial` 反推"设备已停净"（`watchdog:61-62`、`:304-305`）。

**日志口径（唯一口径，全文一致）**：

- **事实**：驱动进程无 tracing subscriber、Core 日志不落盘，是**既定环境事实**（诊断方案 `:171`、`:425`），**不因本轮执行而改变**。
- **结论**：本轮**不以任何进程日志作为判据**，也**不要求**执行前解决日志落盘；日志即使存在也只作旁证。
- **不得**用"没看到错误"推断 PASS；**不得**因日志缺失把 PASS 降为 NOTES，也**不得**因日志缺失直接判 INVALID——**日志缺失对判定中性**（§9.3）。
- **红线**：**若无法持续观测端点状态（watchdog 未就绪、观测心跳失效、`GET /endpoints/{id}` 持续失败），判 INVALID**——watchdog 侧对应 `OBS_STALL` 提前 Stop + `axes.sampling = INVALID_OBSERVATION_GAP` + `exit 2`（`:2813`、`:2842`，场景 **M8**）；**不得启动**；已启动则按 §5.3 处置。
- `mesad-console.log` 若已重定向则一并保存（§6.1 的 `log` 标签），**可选**；`site-log.md`（人工执行记录）**仍是必需证据**。

### 6.4 SHA-256 清单（执行时逐文件填写；基线值本次编制时已实测）

> **时效声明**：下表哈希是**编制时（2026-10-20）的基线参照，不是执行时的验证结果**。执行前必须在本次窗口内重新 `GET` 并重算（§8.1），**不得**引用本表充当"已验证"。

| 文件 | 字节 | SHA-256 |
|---|---|---|
| `828d-r1/rev17-endpoint.raw.json` | 658 | `B3D2D3F0AC0D438E0B1DB66FCDA6D0564DDB620EDE7E3F359309E561B83BB659` |
| `828d-r1/rev17-tasks.raw.json` | 111,992 | `CE1EEEB28C1BC2A54B3A0A0781E2F966C495516749B74782A444D06AEE5E96F1` |
| `828d-r1/rev17-rollback-put.json` / `t2b-rollback-rev15.json` | 111,925 / 111,925 | `10515C712FC9AF27D8B6FDF229764A88FE3E4E7DAC3B7FAC7F01637F21DFDF42`（两文件同值） |
| **签署的 G6/G7 执行包**（含 `approved-task-config.json`、构建清单、两个"自身"摘要） | 待填 | 待填（**三个摘要**：获批配置自身 / 清单自身 / 部署 mesad 二进制；均**取自签署包**，不得现场现算） |
| 本轮 watchdog 产物（`t0.stamp`、`watchdog-states.jsonl`、`watchdog-stops.jsonl`、`summary.json`、`arm-report.json`、`run-log.md`） | 待填 | 待填（每个文件一行；`t0.stamp` 另需记录其 `t0_local` 与 `deadline_local` 两字段原值） |
| 本轮抓证产物（`early`/`prestop` 四件套、`pre`/`post` 快照、`site-log.md`；`mesad-console.log` 若存在） | 待填 | 待填（每个文件一行） |

**纪律**：只保存"原始响应文本 + SHA-256"，**不得只存统计摘要**（质量调查 `:151`；D1 `:18`：`timestamp_ns` ≈1.8e18 超出 JS 安全整数，二次解析会丢精度）。

### 6.5 HMI 对照记录（人工，非仪器）

| 点 | 对照对象 | 记录要求 |
|---|---|---|
| 1 `native.C.1.122.5.1` | HMI 主程序名 | 逐字符抄录（允许 `_N_` 前缀差异，须注明） |
| 2 `native.A.1.26.30110.1` | 现场约定参数（地址语义**待确认**） | 抄录 HMI 对应显示；无对应项写"无对应显示" |
| 3 `native.C.1.127.13.1` | HMI 通道状态 | 抄录状态文字；**不得**把 `5` 解释成某个状态（清单 `:42`） |
| 4 `axis.position.actual_bcs@axis=2` | HMI 该轴坐标 | 抄录数值 + 现场轴名（轴名非本轮采集） |

**禁止**为对照而操作机床（启停程序、切方式、移轴、复位报警、插拔使能，诊断方案 `:312`）；只取"同一时刻已存在"的显示，允许采样相位差并注明。

## 7. 立即停止条件（**首次出现即停，不等第二次**）

| 编号 | 触发（可判定） | 依据 | 动作 |
|---|---|---|---|
| **S1** | `detail` 含 **`S7_JOB_0x8201`**（作业级拒绝） | 诊断方案 `:338` | **首次即 Stop**（比诊断方案的 S2"第 2 次"更严；本轮不承担复现归因） |
| **S2** | 其它通信失败：`COTP_RECV_FAIL`/`early eof`、`CONNECT_TIMEOUT`、**首次确认的 `POLL_FAIL`**（**不设任何"连续 N 次"宽限**） | 诊断方案 `:19`、`:344` | 立即 Stop |
| **S3** | 异常重连：除 Start 那一次外 `runtime.epoch` 再变 ≥1 次；或 `RECONNECTING` ≥10 s | 诊断方案 `:343`（其 30 s 阈值对本轮 30 s 窗口无效，收紧到 10 s） | 立即 Stop |
| **S4** | 机床异常：报警、急停、伺服掉使能、异响；或操作员要求停 | 诊断方案 `:340` | 立即 Stop（**人工优先于脚本**） |
| **S5** | 监护人要求停止（任何理由，无需解释） | 人工 | 立即 Stop |
| **S6** | 状态/数据不自洽：非预期 `STOPPED`/`FAILED`；**筛选后目标端点条数 ≠ 4**（顶层 `count` 是全端点总数，**不作此判据**）；任一点 `timestamp_ns` 停滞 ≥3×interval；`quality≠GOOD` | 诊断方案 `:341`；清单 `:153`、`:178` | 立即 Stop |

**监控来源（两类，不得互相推断）**：

- **端点状态**（`state`/`desired_running`/`epoch`/`RECONNECTING`）**只由 watchdog 每 200 ms 轮询**得到（§5.1-5、§6.3）；S1/S2/S3 的判定用这一类。watchdog 内部判定这些条件并自动入队 Stop，命中即记 `EARLY_TRIGGER`（`exit 5`，`watchdog:2321-2326`、`:2861`）。
- **实时点位质量异常**（`quality`/`quality_code`/`value_origin`/`timestamp_ns` 停滞）**只能由 `points/latest` 快照比对**（`early` vs `prestop`）判定；**不得**由端点状态"看起来正常"推断点位正常，也**不得**用端点状态代替 S6 的点位判据。

**统一流程**：Stop（watchdog 或人工，幂等）→ **六项**复核（§5.3.1 第二层）→ 落盘（时刻/触发条款/`detail`/证据文件）→ 判 **NO-GO 或 INVALID** → **不自动重试**、**不自动进入下一轮**。"再试一次看看"属新的启停，计入 §5.4 的 5 次上限，且须人工批准。**注意**：§3.5 已明确"不自动重试"只约束人工，**驱动/端点仍可能重连**，故复核必须含 §5.3.1-⑥。

## 8. 恢复与回退

### 8.1 回退基线 `rev17` 及其**证明过程**（不是"假定一致"）

> **执行窗口内的重新验证（强制；不得以本节旧哈希代替）**：本节与 §6.4 记录的 SHA-256 全部是**编制时（2026-10-20）的本地结果，只是基准参照，不是执行时的验证结果**。执行前**必须**在**本次窗口内**：
>
> 1. 重新 `GET /api/v1/endpoints/lx-nck-tags-190-connection` 与 `GET /api/v1/tasks?endpoint=lx-nck-tags-190-connection`，**原样保存**响应文本（不重排 JSON）；
> 2. 对本次重新获取的文件与 §6.4 基线同法计算 SHA-256（`Get-FileHash -Algorithm SHA256`）；
> 3. 与本文档记录的基线值**逐字符比对**，把"命令、时刻、本次值、基线值、结论"**五列**写入 `site-log.md`；
> 4. 同步核对 `connection` **全部 8 字段**与 §3.2：其中 **5 个**（`host`/`port`/`local_tsap`/`remote_tsap`/`pdu_length`）由 watchdog **自动**比对，另 **3 个**（`max_read_items`/`timeout_ms`/`family`）按 **§4.2 人工核验**；并核对任务 `revision` 与 §1 表一致。
>
> **不一致 ⇒ NO-GO**：须重新评估回退目标（本文档的 `rev17` 备份可能已不是当前配置，恢复路径失效），**不得**"按文档哈希认定一致"后继续；**也不得**把本文档旧哈希写成"执行时已验证"。

**已捕获的三件套**（`target/validation/828d-r1/`，**编制时**本地结果）：`rev17-endpoint.raw.json`（658 B，端点快照：`connection` 8 字段、`desired_running=false`、`state=STOPPED`、`runtime.revision=16`、`runtime.points=4`）、`rev17-tasks.raw.json`（111,992 B，`GET /tasks` 原始响应 `{endpoint_id, revision:17, tasks:[…]}`，238 点）、`rev17-rollback-put.json`（111,925 B，**可直接作为 `PUT /tasks/{ep}` 请求体** `{"tasks":[…238 点…]}`）。

**证明步骤（本次编制时逐条实测，可复核）**：

1. **哈希比对**：`rev17-rollback-put.json` 的 SHA-256 = `10515C712FC9AF27D8B6FDF229764A88FE3E4E7DAC3B7FAC7F01637F21DFDF42`；既有 `target/validation/t2b-rollback-rev15.json` 的 SHA-256 **完全相同**（`Get-FileHash -Algorithm SHA256` 分别计算，两值逐字符一致）。
2. **字节级一致**：哈希相同 ⇒ 两文件**内容逐字节一致**（SHA-256 抗碰撞性是该结论的唯一依据；本次另以原始文本逐字节比较复核，结果相同）。
3. **逐点核对**：解析两份 PUT 体比较 `point_key` 序列 —— **238 / 238 完全一致**（含顺序）；`interval_ms` 均为 `1000`，`kind` 均为 `nck.points.v1`，任务 `id` 均为 `t1`，`mode` 均为 `poll`。据此**结论**：`t2b-rollback-rev15.json` **可以**作为 `rev17` 的回退目标。
4. **依据边界**：依据是**哈希相等 + 238 点逐点比对**，**不是**"rev15 与 rev17 应该一样"的假定；也**不覆盖**端点 `connection`（以 `rev17-endpoint.raw.json` 为准，本轮不改动）。若未来某轮的 `rev17` 备份与 rev15 哈希不同，则 rev15 **不能**再用，必须按当时的 `rev17` 备份恢复。
5. **时效边界（v1.1 新增）**：以上 1–4 只证明"**编制时刻**这两份本地文件一致"，**不证明执行时刻的 `rev17` 仍是同一内容**（配置可能已被他人修改）。执行时的有效性**只能**由 §8.1 顶部的"执行窗口内的重新验证"给出；**未重验即视为未验证**。

### 8.2 恢复操作步骤（含 G7 生命周期门带来的**行为变化**）

1. 确认已停：`state=STOPPED`、`desired_running=false`、且 watchdog 第二层六项全 `converged`（未停不许改任务，运行中保存被 `CONFLICT` 拒绝）。**v1.3 补充**：现在有**更强的判定依据**——Core 内部的六条判据（§5.3.1 第一层，`lifecycle.rs:986-1046`）与 attempt 表；若 `GET /tasks` 的写操作被 409 拒绝，需读回 `error.code`：`STOP_IN_PROGRESS`（拆解未完成）/ `ATTEMPT_FAILED_UNKNOWN`（异常 attempt 隔离，**必须人工复核**，不得重试绕过）。
2. `PUT /api/v1/tasks/lx-nck-tags-190-connection`，body = **`rev17-rollback-put.json` 原文**（不手工重排 JSON、不 `ConvertTo-Json` 二次序列化）。**前提**：该文件已按 §8.1 在本次窗口内重验通过；**未重验不得 PUT**。
3. 读回 `GET /api/v1/tasks?endpoint=`，记录新 `revision`（预期 **19**：17 → 四点配置 18 → 恢复 19）。
4. 按 §8.3 对账（**内容**必须与 `rev17` 一致；**revision 号会递增，不可能回退**）。保持 **STOPPED**，**不自动 Start**；不删除、不改名任何备份文件。
5. PUT 失败：只允许**重试 1 次**（同 body、同端点），再失败即升级上报，**不得**循环重试或"手工重建 238 点"。**注**：此处的"重试 1 次"是**恢复操作**的重试，与 §3.5 的"采集不得重试"不是一回事；它同样须人工决定，且**不得**借此再 Start。
6. **重启行为（v1.3 新增，必须写进现场交底）**：**`sinumerik-nck` 端点开机一律不自动恢复**（`recovery.rs:29-37` 精确匹配 `driver_id="sinumerik-nck"`；判定在正向凭证**之前**直接 `continue`，`apps/mesad/src/main.rs:184-198`）。因此：
   - 恢复完成后**即使重启 mesad，该端点也不会被自动拉起**——这是**期望行为**，不是故障；
   - 需要再次采集**必须显式 `POST /api/v1/endpoints/{id}/start`**，且属**新的申请**（§10.4）；
   - 该策略**不改写** `desired_running`（DB 旧值保持），只阻止自动恢复；`sinumerik-nck-sim` / `SINUMERIK-NCK` 不受影响（精确匹配，`recovery.rs:53-63`）。
   - **边界（不得夸大）**：这条"重启误拉起"风险的消除**只对 `sinumerik-nck` 成立**，是**一期产品约束、不是通用生命周期语义**；其它驱动仍只被"持久化围栏 + 正向凭证"收窄（G7 设计 §6.15、`:906-918`）。本轮不涉及其它驱动，**不得**把该结论外推。

### 8.3 配置对账（恢复后逐项核对；含恢复后纪律）

| 项 | 恢复前（四点配置） | 恢复后（应为 rev17 内容） | 判据 |
|---|---|---|---|
| `GET /tasks` 的 `revision` | 18（预期） | 19（预期） | **递增即正常**；数值不符只记录，不阻断（版本号不可回退，诊断方案 `:153`） |
| 任务数 / id | 1 / `t1` | 1 / `t1` | 相等 |
| 点数 | 4 | **238** | 相等 |
| `interval_ms` / `mode` / `binding.kind` | 5000 / poll / `nck.points.v1` | **1000** / poll / `nck.points.v1` | 相等 |
| `point_key` 序列 | 4 条 | **238 条，与 rev17 逐点一致** | 逐点比对（顺序敏感） |
| 任务内容哈希 | 待填 | `tasks` 数组规范化后须与 rev17 的 `tasks` 数组同法哈希一致（**不得**直接比 `rev17-tasks.raw.json` 整文件哈希：其中 `revision=17`，恢复后为 19） | 相等；不等即恢复失败 |
| `connection` 5 字段（**watchdog 自动**） | 见 §3.2 | 同 §3.2（本轮从未修改） | 逐字段相等（自动门禁） |
| `connection` 另 3 字段（**人工核验**） | `max_read_items`/`timeout_ms`/`family` | 原始 GET 响应 + 规范化 diff + 复核签名（§4.2） | 逐字段相等，缺失即 NO-GO |
| `state` / `desired_running` | RUNNING→STOPPED | **STOPPED / false** | 必须 |
| `runtime.revision` / `runtime.points` | 16 / 4 | **仍是 16 / 4** | **不作为判据**（上次运行视图，§2.1-④） |
| **重启后是否自动恢复** | — | **否**（`driver_id="sinumerik-nck"`） | 记录实测（重启一次观察）；**"未自动拉起"是期望结果** |

**恢复后纪律**：端点保持 **STOPPED**，**不自动 Start**；下一轮必须重新申请、**重过 §4 六道现场门**。备份三件套与本轮证据**全部保留**，不覆盖、不"整理"；恢复所用的 `rev17-rollback-put.json` **必须**是执行前按 §8.1 重验通过的那一份。恢复失败 ⇒ 判 **INVALID**（本轮数据不得用于任何结论），按 §10 上报。

## 9. 验收判据

### 9.1 通过条件（**全部**满足才算 PASS）

> **v1.3 口径（必读）**：**"调度 PASS"与"采集 VALID"是两个独立轴**，**收敛 CONVERGED 是第三个轴**。三者由 `summary.json` 的 `axes.schedule` / `axes.sampling` / `axes.convergence` 分别给出（`watchdog:2966-2990`），**不得**用一个"通过"概括，也**不得**用其中一个推出另一个（场景 **M1** 正是"调度 PASS + 采集 INVALID"的合法组合）。

| # | 判据 | 证据 | 备注 |
|---|---|---|---|
| 1 | 四点 **4/4 `quality=GOOD`** 且 `value_origin=CURRENT`（不是 `LAST_KNOWN`/`PLACEHOLDER`），`early` 与 `prestop` **两次**都成立 | 两次 `points-latest.raw.json`（**原始全量**）+ 筛选结果 | 只 1 次成立 ⇒ 不足以判 PASS；清单 `:153` |
| 2 | `timestamp_ns` 落在 `[T0, Stop 发出时刻]` 内（`T0` 取自 `t0.stamp`，Stop 发出时刻取自 `watchdog-stops.jsonl`，**均不接受外部指定**），且相邻批次推进 ≈ `n × 5000 ms` | 两次快照的 `timestamp_ns` + `t0.stamp` + `watchdog-stops.jsonl` | T2b 实测 25.001 s = 5×5000 ms；按"轮"判，不按点判 |
| 3 | 抓取时刻与 `timestamp_ns` 差 ≤ 1×interval（T2b `early` 4.80 s、`prestop` 0.50 s） | `when.json` + 快照 | 超过即数据不新鲜 |
| 4 | **按 `endpoint_id` 筛选后恰 4 条**，无第 5 点、无其它端点混入 | 快照（原始全量 + 筛选） | 防"悄悄扩点"；顶层 `count` 是全端点总数，**不作 4 的判据** |
| 5 | **调度轴 `axes.schedule.verdict = PASS`**：存在截止 Stop 且 `|deviation| ≤ 1000 ms` | `summary.json` 的 `axes.schedule`（`watchdog:2805`） | 无截止 Stop ⇒ `UNKNOWN`（不算 PASS） |
| 6 | **采集轴 `axes.sampling.verdict = VALID`**：无心跳失效、无 > 2000 ms 盲区、无截断、有样本 | `summary.json` 的 `axes.sampling`（`:2819-2822`、`SAMPLING_MAX_GAP_MS = 2000`） | 观测中断即 INVALID（**即使 Stop 准时**） |
| 7 | **收敛轴 `axes.convergence.state = CONVERGED`**：第二层六项**全部 `converged`**（任一 `indeterminate`/`not_verified` 都不算） | `summary.json` 的 `six_items` + `axes.convergence`（`:2938`、`:2983-2986`） | 缺一 ⇒ INVALID |
| 8 | **`verdict = STOPPED_CLEAN` 且 `exit_code = 0` 且 `trial = PASS`**（三者一致；不一致以 `exit_code` 与 `summary` 原始字段为准并记异常） | `summary.json`（`:2998-3000`） | **只有 0 可被当作"本轮可判 PASS"**（`:300`） |
| 9 | 全程未触发 §7 任一条（`EARLY_TRIGGER` 不得出现）；`epoch` 只在 Start 变化 1 次、无 `RACE` | `watchdog-states.jsonl`、`summary.race` | 无状态序列即无法判此条 |
| 10 | 证据齐全（§6 全部条目，含 watchdog 自动产物 7 件）+ SHA 清单填全 + HMI 对照在案 + 两份文件摘要与签署包一致 | §6.1 目录 | 缺一 ⇒ INVALID |
| 11 | 恢复完成且 §8.3 对账一致，端点保持 STOPPED，且重启后未被自动拉起 | 读回快照 + 重启观测 | 未恢复不得裁决 |
| 12 | **全部结论只来自独立证据**（`watchdog-states.jsonl`/`summary.json`/`points/latest` 原始快照/Stop 请求响应/`epoch`），未用进程日志推断，也未以"没看到错误"代替观测 | §6.3 | 无独立证据 ⇒ 无结论（§6.3 红线） |

**判据不是"等于历史值"**：T2b 的 `_N_MPF0`、`5`、`1`、`0.00` 只说明"量级/形态合理"。程序名、状态、参数、位置都会随机床状态变化，**禁止**把"必须等于 `_N_MPF0`/`5`/`1`/`0.0`"写成判据（清单 `:155`）。本轮判的是 **GOOD + 新鲜 + 窗口内推进**，不是数值相等。

**在途 Start 不得判 PASS**：裁决时若 Start 仍无明确结果，即便四点读数为 GOOD，也**不得**判 PASS——watchdog 会给出 `START_UNCERTAIN` / `exit 3`（`:2784`、`:2859`），按 §5.3.2 处置并记录。

**离线套件的位置（v1.3 新增，防止误读）**：M0–M18 是**离线套件**（`tests/watchdog/`，假端点），它们的通过**不构成现场授权**、**不证明机床行为**，只能作为"工具在已知故障下不撒谎"的证据引用。本轮的现场结论**只能**由 §9.1 的现场证据给出。

### 9.2 **读到值 ≠ 业务可用**（签字前必读）

**本轮 PASS 只证明**：上述 4 个点位，在本次 ≤30 秒窗口内，可被本 Mesa 实例以 `5000 ms` / `max_read_items=1` 只读方式读到质量 GOOD、时间戳新鲜的值。它**不证明**（不得据此推导）：① 其余 **234** 点可用（本轮一个都没读）；② `238 点 / interval_ms=1000` 配置可用（未启用，且该配置历史上伴随 80 BAD，清单 `:147`）；③ `axis=1/3..8` 可用（本轮只有 `axis=2`，清单 `:185`）；④ 值的**业务语义/单位**正确（`5` 是什么状态、`1` 是什么参数、位置是否 mm，均**待确认**）；⑤ 一期 P0 清单通过（10 个 P0 族里 7 个仍只有历史 GOOD，清单 `:110`）；⑥ 任何**安全判断**能力（报警/急停/驱动使能不在采集范围，清单 `:193`）；⑦ 长期稳定性/性能（30 秒、4 点、6 批不构成性能证据）；⑧ **G7 生命周期门对所有驱动都消除了"重启误拉起"**（只对 `sinumerik-nck` 成立，§8.2-6）。

### 9.3 判定分级

| 判定 | 条件 |
|---|---|
| **PASS** | §9.1 **十二条**全满足（含 `axes.schedule=PASS` + `axes.sampling=VALID` + `axes.convergence=CONVERGED` + `exit_code=0` + `trial=PASS`） |
| **PASS-WITH-NOTES** | §9.1 **十二条全满足**，且只存在**不影响结论的说明项**（如 HMI 无对应显示、值语义待确认）。**"说明项"仅限"证据已完整、结论不受影响"**；**不得**用它放行任何证据缺失、观测中断或判据不明的情形；**日志缺失不属于说明项** |
| **FAIL** | 出现 §7 触发（watchdog 侧 `STOPPED_BY_EARLY_TRIGGER`/`exit 5`），或 §9.1 第 1–9 条任一不满足（watchdog 侧对应 `exit 2`/`4`/`6`） |
| **INVALID** | 证据缺失（§6 不全）、观测中断（`axes.sampling ≠ VALID`，`exit 2`）、Stop 未确认停干净（`axes.convergence ≠ CONVERGED`，`exit 1`/`9`）、恢复未完成/对账不一致、或执行偏离冻结项（watchdog 侧 `WORKER_ABORTED`/`exit 8` 也归此类） |
| **NO-GO（授权边界）** | **`exit 40`**：凭证/摘要/配置校验未通过，**未发出 Start、未进入 FIRED**。**这是授权边界拒绝，不是设备故障、不是 Stop 失败、也不是采集异常**（`watchdog:54-55`、`:288`、`:590`）——现场**不得**按设备故障处置，也不得"改参数再试"，只能补齐签署包后重新申请 |

**判定口径（与 §6.3 一致，不得两说）**：

- **日志缺失不是 NOTES，也不是 INVALID，且不进入判定分级**：进程日志本就不是判据（§6.3）；它是既定环境事实（§2.1-③）。
- **"没看到错误"不是证据**：没有状态序列就没有"停干净"的结论；没有原始快照就没有"4/4 GOOD"的结论。
- **观测能力红线**：watchdog 不能持续观测端点状态 ⇒ **INVALID**，且**不得启动**；已启动则立即 Stop。
- **`verdict` / `trial` / `exit_code` 三者必须并列引用**：不得只引 `trial=PASS` 而隐去 `exit_code`，也不得用 `exit_code=0` 反推"设备侧已安全"（`watchdog:61-62`）。

**INVALID ≠ FAIL**：INVALID 说明"这轮什么也没证明"，**不得**用于任何结论，也不得"挑好的数据留下"。

## 10. 裁决记录

### 10.1 裁决

- [ ] **GO** —— 批准按 §5 执行（仅限 §3.5 冻结范围，单次；**前提：G1–G6 六道现场门全部通过**，其中 G6 = `-Mode arm` 退出码 0 + 凭证汇总 `overall = verified`（arm 下 `authorized`/`authz_ticket` 为 `not_checked` 属正常），且 §8.1 执行前重验已完成；**`T0`/截止时刻不得预先填写**，由 watchdog 在 FIRED 时生成）
- [ ] **NO-GO** —— 不执行（原因：______________________）
- [ ] **HOLD** —— 暂缓（待补：______________________；补齐后重新提交 §10）

裁决理由（必填）：______________________________________________

**裁决时必须同时记录（v1.3 新增）**：

| 项 | 值 | 来源 |
|---|---|---|
| G6 自检退出码 | ______（**必须为 0**） | `arm-report.json` 的 `exit_code` |
| 四项凭证汇总 | ______（**必须为 `verified`**） | `arm-report.json` / `summary.authorization.overall` |
| 执行退出码 | ______（0–9；**40 表示授权边界拒绝，未发 Start**） | `summary.json` / `exit_code` |
| `verdict` | ______ | `summary.verdict` |
| `trial` | ______ | `summary.trial` |
| 三条轴 | 调度 ______ / 采集 ______ / 收敛 ______ | `summary.axes` |

**关于 `exit 40`（必须写清，不得与设备故障混淆）**：`40` 覆盖 `REFUSED_NON_OFFLINE`（arm 指向非回环/8132）、`REFUSED_TEST_INJECTION_ONLINE`（注入参数进生产）、`REFUSED_NOT_AUTHORIZED`（缺授权）、`REFUSED_CREDENTIAL_INVALID`（G6 时间不可解析 / 部署摘要不一致）、`REFUSED_MANIFEST_INTEGRITY`（G1）、`REFUSED_TASKCONFIG_INTEGRITY`（G2）、`REFUSED_TASKCONFIG_MISMATCH`（完整配置不一致）、`REFUSED_CREDENTIAL_UNVERIFIABLE`（fail-closed）（`watchdog:273-288`）。**全部发生在发出 Start 之前**，`network_actions` 至多是一次只读 GET。**现场不得按"设备没连上"处置**，也不得用 `-SkipHostChecks`/改参数等方式"绕过"。

### 10.2 签字（四方齐全才生效）

| 角色 | 姓名 | 单位/部门 | 日期时间 | 签名 |
|---|---|---|---|---|
| 申请人 | 待填 | 待填 | 待填 | |
| 执行人 | 待填 | 待填 | 待填 | |
| 现场监护人 | 待填 | 待填 | 待填 | |
| 审批人 | 待填 | 待填 | 待填 | |

**G6 就绪确认（§4.6；执行人与监护人双签）**：`-Mode arm` 退出码 ______（须为 0）；`arm-report.json` 文件名 ______；四项凭证 ①授权 ______ ②G6 签署 ______ ③G7 部署身份（形式 `sha256:`/`manifest:`）______ ④任务配置 ______（arm 下 ①（`authorized`/`authz_ticket`）为 `not_checked` 属正常；②③④ 与汇总 `overall` 须 `verified`）；两个"自身"摘要已由签署包提供（是/否）—— `-ExpectedManifestSha256` ______ / `-ExpectedApprovedTaskConfigSha256` ______；离线套件 M0–M18 全部通过（是/否，汇总文件 ______）；通路自检 `stop-selftest.json`（是/否，仅离线桩）；**`T0` 记录机制**：由 watchdog 在 FIRED 时刻自写 `t0.stamp`（**签字时不得填写任何具体 `T0` 或截止时刻**）。

**G7 部署身份与生命周期门确认（§4.7）**：部署 `mesad` 路径 ______；其 SHA-256 ______（与 `-G7DeployVersion` 比对结果 ______）；`-G7DeployVersion` 形式 ______；git HEAD/describe（**仅辅助**）______；无在途 Start（是/否）；`-SkipHostChecks` **未使用**（是/否，必须"是"）。

### 10.3 异常记录（无异常也要写"无"）

| 序号 | 时刻（本地） | 现象 | 证据文件 | 处置 | 触发条款 |
|---|---|---|---|---|---|
| 1 | 待填 | 待填 | 待填 | 待填 | S?/无 |
| 2 | 待填 | **迟到 Start 竞态**（Stop 后观测到 `RUNNING`/`epoch` 变化）须单独记一行 | `watchdog-states.jsonl`、`summary.race` | 再次 Stop | `RACE_START_AFTER_STOP` |
| 3 | 待填 | **授权边界拒绝（`exit 40`）** 须单独记一行，并注明是"未发 Start 的拒绝"而非设备故障 | `summary.json`（`refused` 字段）、`run-log.md` | 补齐签署包后重新申请 | — |
| 4 | 待填 | **收敛 `indeterminate` / 观测 `INVALID`**（`exit 2`/`9`）须单独记一行，注明"无法确定 ≠ 已停净" | `summary.six_items`、`axes` | 按 §5.3 隔离流程 | — |

### 10.4 后续纪律

1. **禁止自动进入下一轮**：本轮结束即停止；下一轮重新编制申请（复制本模板）、重跑 §4 **六道现场门**、重新裁决。
2. 本轮 **GO 不覆盖**：238 点、`1000 ms`、扩点、其它轴、性能评估、语义确认、生产接入；**PASS 也不外推**——不构成"828D 已验收"、不构成"NCK 驱动可生产"、不翻转 `GATE.md:91-92` 的任何 Gate；**也不构成"G7 生命周期门已在真机验证"**（G7 的验证是代码级 + 合同测试，本轮只做外部复核）。
3. 归档：本文件 + §6.1 证据目录（含 watchdog 7 件自动产物）+ SHA 清单 + `site-log.md`；结案后 90 天（诊断方案 `:477`），必要时转 `docs/evidence/`。

## 11. 待确认清单（**本申请明确不承诺的能力**；执行前必须逐条落实或改为"不做"）

> **v1.4 勘误**：第 **2** 项（`-ExpectedPointCount` 的语义）**已关闭**——已修复于 **PR #136（`dfc81e0`，已合入 main（merge `f5f9f56a35a8d3c457a139f8904fcbaf4372e119`））**，本表保留原条目并改为"**依赖检查项：确认 #136 已合入 main**"。其余条目不变。事实基础见 **§11.1（事实 A：`ApplyPointMap` 不写 `point_registry`）**。

| # | 待确认项 | 为什么不能承诺 | 谁来解决 |
|---|---|---|---|
| 1 | **签署的 G6/G7 执行包本身尚不存在**：本轮需要的 `approved-task-config.json`、受信构建清单、以及三个摘要（获批配置自身 / 清单自身 / 部署 `mesad` 二进制）都还没有经签署的版本 | `watchdog` 明确要求这两个摘要**只能来自签署包**，禁止现场现算（`:140-149`、`:173-181`）。**在签署包到位之前，fire 阶段必然 `exit 40`** | 审批人 + 构建方 |
| 2 | **【已关闭（v1.4）】`-ExpectedPointCount` 的语义**：~~比对 `diagnostics.point_count`（持久化 `point_map` 残留，由 Start 时的 `ApplyPointMap` 写入），不是任务配置里的点数~~ → **现行判据：比对当前任务点数**（实时任务快照 `tasks[].binding.config.points[]` 总数；`Get-WdTaskPointCount`，**PR #136 后** `watchdog:1293`，判据 `:1509-1518`）。**已修复于 PR #136（`dfc81e0`，已合入 main（merge `f5f9f56a35a8d3c457a139f8904fcbaf4372e119`））**；`diagnostics.point_count` 降级为 `diagnostics_point_count_observed`（**observed-only，不参与配置门判定**） | **改为依赖检查项**：执行前确认 **PR #136 已合入 main**（`git log --oneline -1 dfc81e0` + `git merge-base --is-ancestor dfc81e0 main`）。**若现场部署的 watchdog 早于该合并**，则仍为旧语义（`watchdog:156-158` 比对 `diagnostics.point_count`，读到的只是 `point_registry` 残留），此时须在 `site-log.md` 写明本轮所用**脚本版本与语义口径**，并按该语义取签署值——**不得**把"残留 4 行"当成"当前任务 4 点"的证据 | 审批人（确认 #136 合入状态）+ 执行人（签署值口径） |
| 3 | **`-ExpectedTaskRevision` 的时序**：PUT 四点后 revision 变为 18（预期），而 `-ExpectedTaskRevision` 必须与签署值一致 | 签署包在窗口之前准备，无法预知窗口内是否发生其它写入；一旦偏移 ⇒ `REFUSED_TASKCONFIG_MISMATCH`（`exit 40`） | 审批时把 revision 一并签署；窗口内禁止任何其它写入（G5-②） |
| 4 | **G2 的"协商 PDU 核对"**：`mode=probe` 会发起真实机床 I/O，需**单独授权** | 本申请未包含该授权；未授权时该项只能记"未测" | 审批人单独授权或明确不做 |
| 5 | **MAC `98-fa-9b-59-35-53`** 无仓库内留存 | 仅来自现场记录，全仓检索无匹配 | 现场补充截图/书面记录 |
| 6 | **G7 判据 5 的 reader 结果未纳入判定**：`wait_reader_done` 的结果目前只用于一条 `warn!`，未上报给调用方 | G7 设计 §7.1-5 明确标为**已知缺口**（§11-13） | 后续代码修复；本轮只能靠外部 TCP 会话核对（第二层判据 ⑤）兜底 |
| 7 | **Core 内部 500 ms 收敛窗口**是否需按 endpoint 配置 | G7 设计 §7.3 末标 **待确认**（§11-6） | 后续设计决策；本轮不依赖它作现场判据 |
| 8 | **`sinumerik-nck` 之外的驱动仍有"重启误拉起"路径** | G7 设计 §6.10.3 末明确**未消除**；本轮不涉及其它驱动，也不得外推 | 产品决策（是否推广"所有驱动开机一律不自动恢复"） |
| 9 | **`ARMED` 无法对生产 8132 执行**，生产侧"通路可用"只能由 fire 的只读 GET 证明 | `arm` 冻结为回环且端口 ≠ 8132（`watchdog:611-628`） | 流程层面接受（现场交底时写清）；如需"对生产端点的独立通路自检"，属**新功能**，本申请不承诺 |
| 10 | **真机停止时刻**无法由任何离线手段证明 | 30 s 是调度目标；实际停止由 §5.3.1 第二层六项另行观测，且尾部可能无界（`process.rs:284`） | 现场监护人按 §5.3 处置 |
| 11 | **`max_read_items` / `timeout_ms` / `family` 三字段不由 watchdog 自动验证** | watchdog 的连接配置自动比对只覆盖 5 个字段（`host`/`port`/`local_tsap`/`remote_tsap`/`pdu_length`，`watchdog:1176-1216`）；这三字段**不在集合内** | **现场人工核验**（§4.2）：对本次窗口的 Endpoint 原始 GET 响应与签署的 `approved-task-config.json` 逐字段比较，保存原始响应 + 规范化 diff + 复核签名；任一缺失/不一致/无法取证 ⇒ **NO-GO，不得进入 FIRED**。**这是人工项，不是自动门禁** |

### 11.1 §11-2 的事实基础（事实 A，v1.4 新增）：`ApplyPointMap` **不写** `point_registry`

> 这一条是 §11-2 与 §4.6 末条的共同前提；v1.3 把它写反了（"`point_map` 由 Start 时的 `ApplyPointMap` 写入"），故单列在此，便于后续轮次直接引用。

**正确时序**（`crates/driver-manager/src/endpoint.rs:868-999`，函数 `run_config_flow`）：

```
OpenConnection（:882）→ ConfigureTasks（:914）→ 驱动回 PointDescriptors（:925-932）
→ assign_point_ids 写 point_registry（:943-945 → StorePointIdSource::assign :206-224 → crates/config-store/src/lib.rs:2344-2456）
→ ApplyPointMap（:968-985，只把 point_key → point_id 映射下发给 Driver，本身不写 registry）
→ ConfigureEventTasks（:990-995）→ StartConnection（:997 起，新 stream_epoch）
```

| 结论 | 依据 |
|---|---|
| `point_registry` 的**唯一写点**是 `assign_point_ids`（INSERT/UPSERT + 墓碑 UPDATE，`config-store:2408-2447`） | `crates/config-store/src/lib.rs:2344-2456` |
| 调用链：`run_config_flow` → `StorePointIdSource::assign` → `assign_point_ids` | `crates/driver-manager/src/endpoint.rs:943-945` → `:206-224`（调用点 `:213`） |
| 写入时机在 **`ConfigureTasks` 之后、`ApplyPointMap` 之前** | 同上（`:914` / `:943-945` / `:968-985`） |
| `ApplyPointMap` 只下发映射（`Body::ApplyPointMap{ connection_handle, revision, key_to_point_id }`），**无任何持久化副作用** | `endpoint.rs:968-985` |
| `diagnostics.point_count` = `point_map(endpoint_id).len()` = `point_registry` 中 `deleted=0` 的活跃行数 | `crates/core-api/src/lib.rs:1499` → `crates/config-store/src/lib.rs:2459-2473` |

**推论（现场口径）**：在"四点配置已 PUT、但本轮尚未 Start 过"的时刻，`diagnostics.point_count` 读到的是**上一次 Start 走完 ConfigureTasks 留下的残留行数**（本申请 §1 记录上次为 4 点，属**历史观测**，不能当作本轮四点配置已生效的证据）。这正是 BLOCKED-01 的成因，也是 PR #136 把配置门判据改为"实时任务快照点数"的原因。**在 #136 合入 main 之前**，本申请对 `-ExpectedPointCount` 一律按 §11-2 的"依赖检查项"处理：先确认合入状态，再按所用语义取签署值并记录于 `site-log.md`。
