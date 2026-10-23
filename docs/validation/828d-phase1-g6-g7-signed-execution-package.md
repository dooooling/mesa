# G6/G7 签署执行包模板（828D 一期首轮限定采样）

> ## ⚠️ 本文件是模板。
>
> **本文件是模板；未填写、未签署、未复核的模板不构成任何执行授权。**
>
> - 本文件**不是**已获批的签署包，**不是**放行单，**不是**执行授权，**不构成**任何 `-Mode fire` 的依据。
> - 只有同时满足 ①本文件全部必填栏位已填写 ②§5.7 签署栏四方齐全 ③§4 三个摘要均来自受信产物 ④§5 六道现场门全部通过 ⑤§7 全部阻塞项已关闭（或经审批人书面降级），才**可能**被视为一份可用的签署包；缺任一条即为 **NO-GO**（§5.6）。
> - **填写者不得为了让测试通过而调整任何预期值**（§5.6-N9）。任何"先跑再说、边跑边改"的做法一律作废。
> - 本文件**不含**任何已批准的具体值：所有 `______` / **待填** 都是待签署内容，不是默认值。

**编制信息**：实施基线 `main = 125d00bc840470858f9ced9cf81e313d452a1dae`；模板版本 **t1**；编制日期 2026-10-23；编制范围**仅新建本文件**（未改代码/测试/配置/其它文档，未做 git 写操作，未启服务、未连设备、未跑测试或构建）。

**本模板引用的已合并契约（不得重新设计，只能引用）**：

| # | 契约 | 合并来源 | 实测规模 | 本模板引用的部分 |
|---|---|---|---|---|
| 1 | `docs/validation/828d-phase1-first-limited-sampling-application.md` | PR #130 | 621 行，v1.3 | §3 精确配置、§4 G1–G6 六道门与 §4.2 人工核验、§5 时间窗、§7 立即停止、§8 回退、§9 验收、§11 待确认 |
| 2 | `scripts/stop-watchdog.ps1` | PR #131 | **3096 行**（任务书写"2809+"，合并后经后续提交增长） | `-Mode arm\|fire`、四项凭证校验、退出码语义、三轴与 `verdict`/`trial` |
| 3 | `docs/design/core-start-stop-race-fix-design.md` | PR #132 | 1456 行 | §5.3.1 第一层六条判据的设计依据、§6.3.1 尾部无界、§6.15 恢复策略边界 |
| 4 | `crates/core-api/src/lifecycle.rs` | PR #133 | 1649 行 | `await_settled` 六条判据（`:986-1046`）、`SETTLE_TICKS`（`:42-43`）、`DELETE_PERSIST_WAIT`（`:55`） |
| 5 | `tests/driver-contract/tests/stop_lifecycle.rs` | PR #134 | 2882 行 | 同步屏障/栅栏用例（`:181`、`:2661`、`:2788`）、冻结 Stop 错误码（`:68-83`） |

**术语**：**watchdog** = `scripts/stop-watchdog.ps1`；**#130** = 上表第 1 项；**G7 设计** = 上表第 3 项；**生命周期门** = 上表第 4 项；**ARMED / FIRED** = watchdog 的两个阶段（`watchdog:13-32`）；**三轴** = `summary.json` 的 `axes.schedule` / `axes.sampling` / `axes.convergence`（`watchdog:2966-2990`）。

**本模板不承诺的事（写在这里，避免签署时误解）**：30 秒是**调度目标**而非物理停止保证；读到的值 ≠ 业务可用；点位数量门禁（`-ExpectedPointCount`）**已由 B-1 代码修复关闭**（PR #136，**待合入 main**）：配置门核验的是**当前任务点数**（取自实时任务快照 `tasks[].binding.config.points[]`），`diagnostics.point_count` 降级为运行状态观测值 `diagnostics_point_count_observed`、**不参与判定**（§2.5）；G6 的 ARMED **不能**对生产 `127.0.0.1:8132` 执行（`watchdog:611-628`）。

---

## 1. 执行身份

### 1.1 身份栏位（逐项填写，缺一不得签署）

| # | 栏位 | 填写栏位 | 来源要求（必须留证） | 校验方式 |
|---|---|---|---|---|
| 1.1 | **Endpoint ID** | ______ | 与 `GET /api/v1/endpoints` 的 `id` 逐字符一致（预期 `lx-nck-tags-190-connection`） | 字符串全等；`driver_id` 必须为 `sinumerik-nck`（`recovery.rs:29`） |
| 1.2 | **机床型号（铭牌抄录）** | ______（逐字符抄铭牌，不写"828D 系列"这类概括） | **铭牌照片**（含拍摄时间） | 与 1.3 铭牌序列号同框；照片文件名记入 §6.6 |
| 1.3 | **机床序列号（铭牌抄录）** | ______ | 同上（同一张铭牌照片） | 抄录值与照片逐字符比对，由复核人签字确认 |
| 1.4 | **工位编号** | ______ | **工位牌照片** | 与 #130 §1 的工位一致 |
| 1.5 | **执行窗口（计划起止）** | 起 ______ / 止 ______（本地时间，含 Stop 与恢复，留 ≥5 min 余量） | 审批人批准的窗口 | 实际执行不得越窗；越窗即作废（#130 §5.4） |
| 1.6 | **执行人** | ______ | 本人签署（§5.7） | 与 `-AuthorizedBy` 记录一致 |
| 1.7 | **复核人** | ______ | 本人签署（§5.7） | 与执行人**不得为同一人** |
| 1.8 | **现场监护人** | ______（姓名 + 电话） | 本人签署（§5.7） | 必须现场在场，具备人工 Stop 与拔网线能力（#130 §4-G4） |
| 1.9 | **审批人** | ______ | 本人签署（§5.7） | 审批人不得同时是执行人 |

### 1.2 机型身份**无法由软件证明**——必须四证（不得省略任一项）

- `drivers/sinumerik-nck/src/probe.rs:49-64`：`ProbeReport` 的 `vendor` / `family` / `model` / `firmware` / `model_confidence` **恒为 `None`**——probe 只能证明"TCP → COTP → S7 Setup 建连成功、对端说 S7Comm"（`probe.rs:3`），**不能**证明对端是 SINUMERIK 828D。
- `probe.rs:26`：`NCK_ANCHOR_PENDING` 恒在（anchor 变量未固化为内建常量前不会消失）。
- 因此 §1.1 的机型/序列号只能由**四证**支撑，且四证必须**同属本次窗口**：

| 证 | 内容 | 形式 | 缺一即 |
|---|---|---|---|
| 证① 铭牌 | 型号 + 序列号逐字抄录 | 铭牌照片（带拍摄时间） | G1 不通过 |
| 证② HMI | 机床/系统信息页显示同一型号与序列号 | HMI 截图（带时间） | G1 不通过 |
| 证③ X130 | 网络口现场确认（指示灯、端口标签） | 照片 | G1 不通过 |
| 证④ 工位 | 工位信息与 §1.1-1.4 一致 | 工位牌照片 | G1 不通过 |

> **禁止**把 MAC 或 IP 当作身份证据：`98-fa-9b-59-35-53` 仅来自现场记录、仓库内无留存（#130 §4.2 末），且 MAC 可换网卡、IP 可复用（诊断方案 `:215-217`）。MAC 只能作为**辅助对照**，**不得**单独作为 G1 通过依据。

---

## 2. 获批配置

### 2.1 四点完整参数（逐字段；**本轮禁止增删改**）

| # | `point_key` | kind | 逐字段参数（填写栏位） | wire 地址 | dtype（decode） |
|---|---|---|---|---|---|
| 1 | `native.C.1.122.5.1` | `nck_native` | `area=C` · `area_no=1` · `module=122` · `column=5` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `C.1.122.5.1` | `UTF8_NUL_LAST_32` |
| 2 | `native.A.1.26.30110.1` | `nck_native` | `area=A` · `area_no=1` · `module=26` · `column=30110` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `A.1.26.30110.1` | `U8` |
| 3 | `native.C.1.127.13.1` | `nck_native` | `area=C` · `area_no=1` · `module=127` · `column=13` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `C.1.127.13.1` | `U16_LE` |
| 4 | `axis.position.actual_bcs@axis=i:2,channel=i:1` | `nck_variable` | `variable_id=axis.position.actual_bcs` · `definition_revision=1` · `target={channel:1, axis:2}` · `arguments={}`（必须空对象，不得 `null`/省略） | `C.1.112.33.2`（定义编译，非手写） | `F64_LE` |

- 以上取值逐字来自 #130 §3.1（同一配置，**不是"相似"**）；9 个发送字段全部落在白名单 `ALLOWED`（12 项，`drivers/sinumerik-nck/src/native.rs:147-160`）内。
- **签署包必须携带** `approved-task-config.json`（含 `connection` + `tasks[].binding.config.points[]` 全量），其**原始字节**摘要写入 §4-①。
- 校验方式：`watchdog:1218-1250` 按 `"<task id>/<point_key>"` 建映射，比对**该点位元素的整体规范化 JSON**（比"只比 parameters"更严）；任一字段差异 ⇒ `REFUSED_TASKCONFIG_MISMATCH`（exit 40，`watchdog:285`）。

### 2.2 连接配置（8 字段全列；**本轮禁止修改任一字段**）

| 字段 | 值（填写栏位） | 归属 | 依据 |
|---|---|---|---|
| `host` | ______（预期 `192.168.15.60`） | **watchdog 自动比对** | `watchdog:1180-1186`、`:1190-1214` |
| `port` | ______（预期 `102`） | **watchdog 自动比对** | 同上 |
| `local_tsap` | ______（预期 `1024`） | **watchdog 自动比对** | 同上 |
| `remote_tsap` | ______（预期 `3332`） | **watchdog 自动比对** | 同上 |
| `pdu_length` | ______（预期 `480`） | **watchdog 自动比对** | 同上 |
| `max_read_items` | ______（预期 `1`） | **人工核验**（§5.5） | #130 §4.2；不在 `watchdog:1180-1186` 集合内 |
| `timeout_ms` | ______（预期 `1000`） | **人工核验**（§5.5） | 同上 |
| `family` | ______（预期 `828d`） | **人工核验**（§5.5） | 同上 |

- 8 字段集合来自 NCK 连接 schema（`drivers/sinumerik-nck/src/config.rs:26-43`，JSON 字段名见 `config.rs:67-76`）。
- **自动比对只覆盖 5 个字段**（`watchdog:1180-1186`）；`max_read_items` / `timeout_ms` / `family` **不在该集合内**，**不得**在任何签字文件里写成"已被 watchdog 自动验证"（#130 §3.2、§11-11）。
- 原始来源：`target/validation/828d-r1/rev17-endpoint.raw.json`（658 B，编制时快照；该文件在 `target/` 下、不进仓）。

### 2.3 预期 task revision / 点位数量 / 规范化 tasks 指纹

| # | 项 | 填写栏位 | 来源要求 | 校验方式 |
|---|---|---|---|---|
| 2.3-1 | **预期 task revision** | ______（PUT 四点后预期 `18`；#130 §3.3） | 审批时一并签署；窗口内禁止任何其它写入（G5-②） | `watchdog:1405-1406` 与 `GET /api/v1/tasks?endpoint=` 的 `revision` 严格比对；不一致 ⇒ exit 40 |
| 2.3-2 | **点位数量** | ______（获批配置点位总数；**= 当前任务点数**：实时任务快照 `tasks[].binding.config.points[]` 总数）；`diagnostics.point_count` 是持久化残留值，**不参与判定**（§2.5） | 由 `approved-task-config.json` 的 `tasks[].binding.config.points[]` 计数 | `watchdog:1536-1538`：获批配置点位总数必须等于 `-ExpectedPointCount`；现场实时值由 `watchdog:1515-1518` 比对 |
| 2.3-3 | **规范化 tasks 指纹** | `sha256:______` | 对 `approved-task-config.json` 的 `tasks` 数组按"键递归排序 + 按 `task.id` 排序 + 紧凑 JSON"计算（`watchdog:1056-1075`） | `watchdog:1339-1342`（获批文件自证）+ `:1440-1443`（实际 GET 一致）；两处都必须相等 |
| 2.3-4 | `-ExpectedApprovedTaskConfigSha256` | `sha256:______` | **只能来自本签署包**（§4-①） | `watchdog:1120-1174`：**解析前**对原始字节比对；不一致 ⇒ `REFUSED_TASKCONFIG_INTEGRITY` |

> **指纹 ≠ 文件摘要**：指纹只覆盖规范化 `tasks` 数组，**不覆盖 `connection`**（`watchdog:170-171`、`:1123-1125`）。只签指纹不签文件摘要 ⇒ `connection` 被改而指纹不变，必然被 `watchdog:1326-1330` 拒绝。
>
> **行号口径**：2.3-2 的 `watchdog:1536-1538` / `:1515-1518` 属 **PR #136 版本**（3241 行，尚未合入 main）；本表其余行号对应**已合入基线**（PR #131 的 3096 行版本）。两者不可混用；本次仅重排"点数门"相关行号，`#136` 合入后需对全表整体重排。

### 2.4 「批准值来源」栏（硬约束 1 的可检查项）

**规则**：下表每一行都必须能回答"**由谁、依据什么、何时批准**"。缺任一项 ⇒ 该值视为**未批准** ⇒ NO-GO（§5.6-N1）。

| 批准值 | 值 | 由谁批准 | 依据（可追溯对象） | 何时批准 |
|---|---|---|---|---|
| 四点参数集（§2.1） | 见 §2.1 | ______ | ______（如 `approved-task-config.json` 的文件摘要 + 审批单号） | ______ |
| 连接配置 8 字段（§2.2） | 见 §2.2 | ______ | ______ | ______ |
| 预期 task revision（§2.3-1） | ______ | ______ | ______ | ______ |
| 规范化 tasks 指纹（§2.3-3） | ______ | ______ | ______ | ______ |
| 部署二进制 SHA-256（§3.1） | ______ | ______（受信构建方） | ______（CI run / 产物名） | ______ |
| 构建清单自身 SHA-256（§3.3） | ______ | ______ | ______ | ______ |
| `approved-task-config.json` 自身 SHA-256（§4-①） | ______ | ______ | ______ | ______ |

**禁止（硬约束 1 的红线）**：
1. **禁止**在现场读取端点当前值（`GET /tasks`、`GET /endpoints/{id}`、`GET …/diagnostics`）后**反过来**把这些值填成批准值——现算等于自己给自己授权（`watchdog:140-149`、`:173-181`）。
2. **禁止**把"上次跑过、看起来一样"当作批准依据；#130 §3.4 的历史对照只证明"编制时一致"，**不证明本次窗口一致**（#130 §4.2-4）。
3. 批准值必须**绑定到同一份获批配置**：tasks 指纹、连接参数、task revision、点位数量四者若来自不同来源，视为未批准。

### 2.5 `-ExpectedPointCount` 时序核实（硬约束 4，**本模板的技术核心**）

**结论：原为 B（无法确认）；已由 B-1 代码修复关闭（PR #136，HEAD `dfc81e04a9d3683dd06d6e19426012d9dcdab5a2`，CI 4 PASS / 0 FAIL，**尚未合入 main**）。** 修复前该门禁**不能**用"由获批配置推导的值"可靠判定；修复后 `-ExpectedPointCount` 比对的是**当前任务点数**（实时任务快照 `tasks[].binding.config.points[]` 总数），`diagnostics.point_count` 降级为运行状态观测、**不参与配置门判定**。签署前仍须**确认 #136 已合入 main**（本模板不得先于 #136 合并）。

> **行号口径**：本节引用的 `watchdog:<行号>` 以 **PR #136 的 `scripts/stop-watchdog.ps1`（3241 行，HEAD `dfc81e0`）** 为准。该版本尚未合入 `main`，故本模板中"已合并契约"表（§开头第 2 项，PR #131 的 3096 行版本）仍是**已合入基线**，两者行号不可混用。

**修复后的实际语义（#136 已实现，本节即最终口径）**：

| 项 | 修复前（**已废弃**） | 修复后（#136 已实现） |
|---|---|---|
| 配置门判据 | `-ExpectedPointCount` 比对 `diagnostics.point_count` | **比对"当前任务点数"**，由新增 `Get-WdTaskPointCount`（`watchdog:1293`）从**实时任务快照** `tasks[].binding.config.points[]` 总数计算 |
| 判据位置 | `watchdog:1407-1408`（旧点数门）、`:1426-1428`（获批点数自洽） | **`watchdog:1509-1518`**（新判据块；点数判据本体 `:1515-1518`），失败原因明写"取自实时任务快照 `tasks[].binding.config.points[]`，**非** `diagnostics.point_count`" |
| `diagnostics.point_count` | 被当作"当前任务点数" | 降级为 **`diagnostics_point_count_observed`**（角色串见 `watchdog:1395`），**observed-only**，**不参与配置门判定** |
| 证据字段（三处独立块 `task_config_observation`） | — | 拒绝记录 `watchdog:614`、arm-report `:1756`、fire summary `:3045`；每块含 `expected_task_point_count` / `task_point_count_live` / `task_count_live` / `diagnostics_point_count_observed` / `diagnostics_point_count_role='observed-only…'` / `task_config_state` |
| 缺失 / 不可核实 | fail-closed | **仍然 fail-closed**：`task_point_count` 读不到（快照无 `tasks[]`）⇒ `unverified`，拒绝进入 FIRED（`watchdog:1515`、`:1554-1558`） |
| Core / DB | — | **零改动、无新增 API**（只改 watchdog 与离线场景） |

**写入时序（更正任何"`ApplyPointMap` 写 `point_registry`"的说法）**：

```
ConfigureTasks → GetDescriptors → assign_point_ids 写 point_registry
→ ApplyPointMap（只把 point_id 映射下发给 Driver，本身不写 registry） → StartConnection
```

`point_registry` 的唯一写点是 `assign_point_ids`（`crates/config-store/src/lib.rs:2344-2456`）← `crates/driver-manager/src/endpoint.rs:206-224`（调用点 `:213`）← `run_config_flow:943-945`。

**调查证据（逐条给位置；这些事实在修复后依然成立，正是 #136 的修法依据）**：

| # | 问题 | 核实结果 | 代码位置 |
|---|---|---|---|
| 1 | `diagnostics.point_count` 的确切来源 | `state.store.point_map(&id).map(\|m\| m.len()).unwrap_or(0)`，即"该端点在 `point_registry` 表中 `deleted=0` 的行数"；SQL：`SELECT point_key, point_id FROM point_registry WHERE endpoint_id=?1 AND deleted=0` | `crates/core-api/src/lib.rs:1499`（响应字段 `:1510`）→ `crates/config-store/src/lib.rs:2459-2473`；表定义 `:919-931` |
| 2 | `ApplyPointMap` / `point_registry` 的写入时机 | 持久化写入**只**发生在 `assign_point_ids`（唯一写点，upsert `:2407-2422`、墓碑 `:2436-2451`）；其唯一调用链是 `StorePointIdSource::assign`（`crates/driver-manager/src/endpoint.rs:206-224`，调用点 `:213`）← `run_config_flow`（`:943-945`）。`run_config_flow` 的顺序是 **OpenConnection（`:882`）→ ConfigureTasks（`:914`）→ 拿到 PointDescriptors（`:925-938`）→ assign/写 registry（`:943-945`）→ ApplyPointMap（`:968-985`）→ ConfigureEventTasks（`:990`）→ StartConnection（`:997-1009`）** | `crates/driver-manager/src/endpoint.rs:868`、`:914-945`、`:968-1009` |
| 3 | **在 FIRED 前（尚未 Start）**该字段返回什么 | 返回**上一次走完 `ConfigureTasks` 的那次 Start 留下的活跃行数**；从未成功配置过 ⇒ `0`（`unwrap_or(0)`）。**与当前任务配置无关** | 同 #1 + #2 |
| 4 | 四点配置 PUT 之后、Start 之前 `point_registry` 的内容 | **完全不变**。`PUT /tasks/{id}` 只做 `gate_data_tasks`（descriptor 集合级/信封层校验，`crates/core-api/src/lib.rs:2415-2418`）→ `store.replace_tasks`（`:2419`），**不触碰 `point_registry`**；descriptor 校验走 `manager.get_descriptor`（临时进程 handshake + GetDescriptor，`crates/driver-manager/src/manager.rs:1076-1115`），也不写 registry | `crates/core-api/src/lib.rs:255-342`、`:2371-2425`；`manager.rs:1077-1115` |
| 5 | 首轮与后续轮的预期值是否不同 | **不同，且差异来自运行历史而非配置**：①首轮（该端点上次配置闭环的点集 ≠ 本次获批点集）⇒ 残留值 = 上次点集大小（本机编制时快照为 `runtime.points=4`，见下）；②本轮跑过一次 4 点配置之后 ⇒ 残留值 = 4，与获批配置点数相同。因此**同一份获批配置在不同轮次的期望值可能不同** | 残留值来源 `endpoint.rs:351`（`id_map` 初值 = `known_map()` = 持久化 `point_map`）→ `:354-363`（首个 `set_status` 用 `id_map.len()`）→ `crates/driver-manager/src/endpoint.rs:497`/`:509`（`EndpointStatus.points`） |

**为什么最初判为 B，以及修复如何解决它**：旧门禁要求 `-ExpectedPointCount` **同时**等于 ①获批配置的点位总数（旧 `watchdog:1426-1428`）与 ②现场实时 `diagnostics.point_count`（旧 `watchdog:1407-1408`）。①可由签署包给出；②是**运行残留**，只能由"上一次 Start 的点集"决定，而"上一次 Start 用过哪些点"在签署时**无法从仓库证明**（DB 在 `target/` 下、不进仓，且任何人都可能在审批与执行之间 Start 过该端点）。因此不存在一个"既由获批配置推导、又必然等于现场残留"的值——**旧实现里两个被要求相等的量中，有一个（`diagnostics.point_count`）与本次任务配置无关**。#136 的修法是：**删掉这个无关量作为判据**，改为只比对"当前任务点数"（`watchdog:1293` 计算、`:1515-1518` 判定），并把 `diagnostics.point_count` 改名为 `diagnostics_point_count_observed`、只作观测输出。于是判据重新变成"可由获批配置推导"，`BLOCKED-01` 关闭。

**唯一可见的旁证（只能当假设，不得写死）**：编制机本地快照 `target/validation/828d-r1/rev17-endpoint.raw.json`（658 B）中 `runtime.points = 4`、`runtime.revision = 16`。由于 `runtime.points` 在 Start 起点取自持久化 `point_map`（`endpoint.rs:351` → `:497`/`:509`），它**与 `diagnostics.point_count` 是同一残留量的两个视图**，故"编制时残留 = 4"有证据支持。但它**不能**证明执行窗口内的值仍为 4（期间任何一次 Start 都会改变它）。**修复后该残留量只影响观测值 `diagnostics_point_count_observed`，不再是任何判据**，因此也不必（且不得）把它写成批准值。

**为什么不能只写"任务有 4 个点，所以 `point_count` 必然为 4"**：这是评审明确要求排查的错误假设——`point_count` 来自 `point_registry`，**不是**任务配置里的点数（`crates/core-api/src/lib.rs:1499`）；四点 PUT 后 `point_registry` 一字未变（见 #4）。若上次跑的是 238 点配置，此时 `point_count` 仍是 238，即使任务配置里只有 4 点——这正是 #136 不再用它作判据、改为从实时任务快照计算的原因（`watchdog:1293`）。

**补充事实（影响"首轮 = 0"的假设）**：`point_registry` 表**没有外键**（`crates/config-store/src/lib.rs:919-929`），`delete_endpoint` 只执行 `DELETE FROM endpoints`（`:1676-1680`），handler 也只清 Snapshot 与停止意图围栏（`crates/core-api/src/lib.rs:2182-2196`），**都不清理 `point_registry`**。因此"从未 Start 过 ⇒ `point_count = 0`"只对**从未使用过该 `id`** 的端点成立；同一 `id` 删除后重建会继承旧行。

**`BLOCKED-01` 的关闭方式：已选定 B-1（代码修复）并已实现（PR #136，待合入 main）**。B-2 / B-3 保留为**历史备选，本轮均未采用**：

| 选项 | 做法 | 状态 |
|---|---|---|
| **B-1 代码修复（已选定、已实现）** | 让配置门比对"当前任务点数"（实时任务快照 `tasks[].binding.config.points[]` 总数，`watchdog:1293`），并把 `diagnostics.point_count` 降级为 `diagnostics_point_count_observed`（observed-only、不参与判定） | **已实现于 PR #136**（`dfc81e0`，CI 4 PASS / 0 FAIL）；**尚未合入 main**，本模板不得先于 #136 合并 |
| **B-2 语义改写并显式签署** | 由审批人签署"`-ExpectedPointCount` 是**状态断言**（残留 registry 行数）"并承担风险 | **未采用**（与硬约束 1 精神冲突；#136 已从根上消除该需要） |
| **B-3 本轮不执行** | 以 `BLOCKED-01` 未关闭为由判 NO-GO，等待 B-1 | **未采用**（B-1 已落地） |

**已验证的判别性场景（M19（单场景 54 项断言；全量 M0–M19 共 20 场景 / 282 项断言）；`tests/watchdog/run-watchdog-scenarios.ps1`）**：

- **① 残留 238 / 0，当前任务 12 点 ⇒ 放行**（`exit=0`、`task_config_state=verified`、`START_begin=1`），证据为 `diagnostics_point_count_observed=238` 而 `task_point_count_live=12`（旧判据在此必然误拒）；
- **② 实时 3 / 5 点（`diagnostics` 仍报 12，旧判据看着"一致"）⇒ 拒绝** `REFUSED_TASKCONFIG_MISMATCH`（旧判据在此必然漏放）；
- **③ 点数相同但某点位参数被改 ⇒ 拒绝，且点数门不触发**（点数门与逐点参数门是两条独立判据）；
- **④ 快照不可读 / 无 `tasks` 数组 ⇒ fail-closed**（`unverified`，拒绝进入 FIRED）。

**已选定 B-1 后，以下现场步骤仍然必须执行并留证（门禁前置诊断，写进执行顺序）**：

1. 在 #130 §5.2-3 的 `PUT /api/v1/tasks/{endpoint}` **之后**、`-Mode fire` **之前**（本模板 §5.8 第 4 步），做一次**只读** `GET /api/v1/tasks?endpoint=…` 与 `GET /api/v1/endpoints/lx-nck-tags-190-connection/diagnostics`，原样保存两份响应；
2. 在 `site-log.md` 记录**六列**：**命令 / 时刻 / 当前任务点数（实时快照 `tasks[].binding.config.points[]` 计数）/ `diagnostics_point_count_observed`（观测值）/ 签署值 / 结论**；其中**只有"当前任务点数"参与判定**，观测值**不得**当作判据；
3. 当前任务点数 ≠ 签署值 ⇒ **立即 NO-GO，不得进入 FIRED**（否则 fire 必然 `exit 40`，`watchdog:1515-1518`）；`diagnostics_point_count_observed` 与签署值不等**不构成**拒绝理由（正常现象）；
4. 上述任何实测值**只能**用于"确认门禁是否会被拒绝"，**不得**反过来填入 §2.3 的批准栏位（硬约束 1）。

---

## 3. G7 部署身份

### 3.1 实际部署 `mesad` 二进制的 SHA-256

| # | 项 | 填写栏位 | 来源要求 | 校验方式 |
|---|---|---|---|---|
| 3.1-1 | `-G7DeployVersion` 形式 | ☐ `sha256:______` ☐ `manifest:______`（路径） | 二选一；**不接受** git commit / `git describe` | `watchdog:901-905`（两种形式）、`:992-996`（其它形式一律 `unverified`） |
| 3.1-2 | `-G7BinaryPath`（部署二进制路径） | ______ | 现场实际运行的那一个文件 | `watchdog:828-844`：显式给出却不存在 ⇒ 直接 `unverified`，**不回退**到 `target/` 下其它二进制 |
| 3.1-3 | 二进制实算 SHA-256（现场由 watchdog 计算，仅作核对记录） | ______（`summary.authorization.items.g7_deploy_version.actual.sha256`） | watchdog 实算 | 必须等于 3.1-1/§3.3 给出的期望值（`watchdog:1025-1028`） |

### 3.2 受信构建来源（谁产出了这个二进制）

| # | 项 | 填写栏位 | 来源要求 |
|---|---|---|---|
| 3.2-1 | CI run / 流水线标识 | ______ | 可公开检索的 run 链接或编号 |
| 3.2-2 | 构建产物名（artifact） | ______ | 与 3.1-2 的路径对应 |
| 3.2-3 | 产出人 / 产出时间 | ______ / ______ | 构建方签署 |
| 3.2-4 | 该产物是否来自 `main = 125d00b` 之后的含 G7 版本 | ☐ 是 ☐ 否（否 ⇒ NO-GO） | 仅作**辅助记录**，不作为通过依据（见 §3.4） |

### 3.3 构建清单文件与其自身 SHA-256

| # | 项 | 填写栏位 | 校验方式 |
|---|---|---|---|
| 3.3-1 | 受信构建清单路径 | ______ | 走 `manifest:` 形式时必填（`watchdog:905-911`） |
| 3.3-2 | 清单**自身**原始字节 SHA-256 | `sha256:______` | `watchdog:935-962`：**解析之前**对原始字节比对；不一致 ⇒ `REFUSED_MANIFEST_INTEGRITY`（exit 40） |
| 3.3-3 | 清单内的二进制摘要字段形态 | ☐ `{"binary","sha256"}` ☐ `{"artifacts":{...}}` | `watchdog:846-880`（多产物时必须按文件名命中） |
| 3.3-4 | `-ExpectedManifestSha256` 与 `-G7DeployVersion` 是否配套 | ☐ 配套（manifest 形式） ☐ 均未使用（`sha256:` 形式） | `watchdog:997-1001`：给了清单摘要却不走 `manifest:` ⇒ **拒绝**（不静默忽略） |

> **为什么清单自身必须有独立摘要**：只比"清单里的二进制摘要 ↔ 实际二进制"时，**清单与二进制一起被替换**仍互相匹配（`watchdog:34-42`）。因此清单自身摘要必须来自本签署包（§4-②），**禁止**现场 `Get-FileHash` 现算（`watchdog:140-149`）。

### 3.4 git HEAD / `git describe` **仅作辅助记录，不作为通过依据**

- `watchdog:125-138`：`-G7DeployVersion` **只接受** `sha256:<64 hex>` 或 `manifest:<path>`；git 版本标识"只能证明输入值与仓库版本匹配，不能证明部署物就是该版本构建的"。
- `watchdog:802-827`：`Get-WdGitAux` 采集 `git_head` / `git_describe` 时显式标注 `role = 'auxiliary-only（B2：仅记录，不作为 G7 通过依据）'`，只写入 `summary.json` / `t0.stamp`。
- `watchdog:992-996`：若把 commit SHA / describe 填进 `-G7DeployVersion`，一律 `unverified` ⇒ 拒绝进入 FIRED（fail-closed，**不降级**）。

| 辅助记录项 | 值（填写栏位） | 说明 |
|---|---|---|
| git HEAD（辅助） | ______ | 只作追溯，**不参与放行判定** |
| `git describe`（辅助） | ______ | 同上 |
| 是否有人以 git 标识充当通过依据 | ☐ 否（必须为"否"） | 若为"是" ⇒ NO-GO（§5.6-N3） |

---

## 4. 文件完整性：三个摘要 + 各自的独立批准来源

| # | 摘要 | 对应参数 | 批准来源要求 | 填写栏位 |
|---|---|---|---|---|
| ① | `approved-task-config.json` **原始字节** SHA-256 | `-ExpectedApprovedTaskConfigSha256` | **本文件 §5.7 签署栏**（签署包） | `sha256:______` |
| ② | 构建清单**原始字节** SHA-256 | `-ExpectedManifestSha256` | **本文件 §5.7 签署栏**（签署包） | `sha256:______` |
| ③ | 部署二进制 SHA-256 | `-G7DeployVersion` | **受信构建产物**（§3.2 的 CI run / 产物） | `sha256:______` |

**必须写明（硬约束 1 的直接落地）**：

1. 三者**不得**在现场从"当前端点状态"或"未经审核的本地文件"临时生成。现场 `Get-FileHash` 现算 = 自己给自己授权（`watchdog:140-149`、`:173-181`、`:1120-1174`）。
2. ①与②的比对都发生在**解析内容之前**，对**同一份原始字节**进行（一次 `ReadAllBytes`，摘要与解析共用同一份字节，排除 TOCTOU；`watchdog:758-784`、`:786-800`、`:1120-1174`、`:935-965`）。
3. ①的作用不可由指纹替代：指纹不覆盖 `connection`（`watchdog:1123-1125`）；②的作用不可由清单内摘要替代：清单可被整体替换（`watchdog:913-915`）。
4. 三者中任一缺失/不匹配 ⇒ `exit 40`，**不发 Start、不写 `t0.stamp`**（`watchdog:42`、`:273-288`）。

| 归档项 | 内容 | 填写栏位 |
|---|---|---|
| `approved-task-config.json` 文件路径 | ______ | 与 ① 摘要对应的**那一份**文件（路径 + 摘要必须配对记录） |
| 构建清单文件路径 | ______ | 与 ② 摘要对应的那一份 |
| 部署二进制路径 | ______ | 与 ③ 摘要对应的那一份 |
| 三者的字节数 | ______ / ______ / ______ | 便于事后核对是否被替换 |

---

## 5. G6/G7 现场门禁

### 5.1 六道现场门（编号按 #130 §4 的实际编号，**必须用本次窗口的新证据**）

> **授权不延续**：上一轮的门禁记录、旧截图、T2b 的通过结果、离线套件的历史运行结果一律不得复用（#130 §4 开头）。任一门不通过 ⇒ **不 Start**。

| 门 | 本次新证据（填写栏位） | 证据形式 | 不满足时处置 |
|---|---|---|---|
| **G1 机床身份** | 铭牌型号 ______ / 序列号 ______ / HMI 页 ______ / X130 照片 ______ / 工位 ______ | 铭牌照片 + HMI 截图 + X130 照片 + 工位牌照片（**均带拍摄时间**） | 停止，**NO-GO**（软件侧 `family`/`model` 恒 `None`：`probe.rs:49-64`） |
| **G2 网络参数** | HMI IP ______ / 协商 PDU ______（**未授权探测则记"未测（无授权）"**）/ 占用核对 ______ | HMI 网络页截图 +（获授权时）`10-probe.json` + 交换机/网口核对记录 | 未获探测授权 ⇒ 只用 HMI + ARP/MAC + 目视，**不得**由软件主动连机床；仍不符 ⇒ 停止 |
| **G3 机床状态** | 上电 ______ / 急停 ______ / 驱动使能 ______ / 参考完成 ______ / 程序运行 ______ / 轴运动 ______ / 报警号 ______ / 操作员口述 ______ | 机床状态表 + 签字 | 记录状态后停止 |
| **G4 现场监护** | 监护人 ______ / 电话 ______ / 人工 Stop 能力 ☐ / 拔测试主机网线能力 ☐ / 窗口时长 ______ | 监护人签字（§5.7） | 不开始 |
| **G5 备份与并发** | 窗口内重新 GET 的端点/任务原始响应 ______ / 其 SHA-256 ______ / 与 #130 §6.4 基线比对结论 ______ / 并发确认 ______ | 本次窗口内的原始响应 + `Get-FileHash` 输出 + 两值并列比对记录 + 并发确认 | 哈希不一致或无法重验 ⇒ **NO-GO**；并发不确认 ⇒ 不开始 |
| **G6 就绪与凭证** | `-Mode arm` 退出码 ______（须 0）/ `arm-report.json` ______ / 凭证汇总 `overall` ______（须 `verified`） | `arm-report.json` + `run-log.md` + 离线套件 M0–M19 汇总 + **本签署包**（两个文件摘要的来源） | 任一项不满足 ⇒ **NO-GO**，不得 Start |

> **G2 的探测需单独授权**：`mode=probe` 会发起真实机床 I/O（`drivers/sinumerik-nck/src/probe.rs:3`、`:38-48`；路由 `crates/core-api/src/lib.rs:3396`），**不得**默认并入前置检查；未授权时 G2 的"协商 PDU"只能记"未测"。

### 5.2 自动门禁 vs 人工核验（**两列必须分开，不得混写**）

| 类别 | 覆盖字段 / 项目 | 判定者 | 代码位置 | 签字要求 |
|---|---|---|---|---|
| **watchdog 自动** | `host` / `port` / `local_tsap` / `remote_tsap` / `pdu_length` | `scripts/stop-watchdog.ps1` | `watchdog:1176-1216`（逐字段，任一不一致即拒绝）；`:1420-1421`（并入 problems） | 无需单独签字；结果见 `summary.authorization.items.task_config` |
| **人工核验** | `family` / `max_read_items` / `timeout_ms` | **执行人 + 复核人**（现场逐字段比对） | **不在** watchdog 自动集合内（`watchdog:1180-1186`）；依据 #130 §4.2 | **必须有独立签字**（§5.5 签名栏） |

**人工项的操作口径（照抄 #130 §4.2，不得改写）**：

1. 在**本次窗口内**取 **Endpoint 原始 GET 响应**（不裁剪、不摘要化），与签署的 `approved-task-config.json` **逐字段**比较这三个字段；
2. 保存**原始响应**、**规范化差异**（JSON 规范化后 diff）与**复核签名**；
3. 任一字段缺失、不一致或无法取证 ⇒ **NO-GO，不得进入 FIRED**；
4. 历史配置对照（#130 §3.4）**不能**替代本项。

### 5.3 ARMED 结果（第六道门 G6 的判据）

| # | 判据 | 填写栏位 | 依据 |
|---|---|---|---|
| 5.3-1 | `-Mode arm` **退出码 = 0（`ARMED_READY`）** | ______ | `watchdog:1623-1628`、`:269` |
| 5.3-2 | `arm-report.json` 已落盘，含 `verdict` / `exit_code` / `credentials` / `endpoint_probe` / `arm.hold_ms` | 文件名 ______ | `watchdog:1630-1657` |
| 5.3-3 | 凭证汇总 `overall = verified`（`failing` 为空） | ______ | `watchdog:1500-1505`；arm 下 `authorized`/`authz_ticket` 记 `not_checked` 属正常（`watchdog:1458-1476`） |
| 5.3-4 | 退出码 1/2/3 任一 ⇒ 不通过（`ARMED_INPUTS_MISSING` / `ARMED_SELFTEST_FAIL` / `ARMED_ENDPOINT_UNREACHABLE`） | ______ | `watchdog:268-272`、`:1625-1627` |
| 5.3-5 | 离线硬门未触发（arm 只允许回环且端口 ≠ 8132） | 实际 `-BaseUrl` ______ | `watchdog:611-628`（**注意**：`-BaseUrl` 缺省值是 `http://127.0.0.1:8132`，`watchdog:326`，因此 arm **必须显式**给离线桩地址） |
| 5.3-6 | 测试注入参数未携带（`-TestHostProbeError` / `-TestAbortFlow`） | ☐ 未携带（必须） | `watchdog:654-702`（B1：任何网络动作之前 exit 40，`network_actions = 0`） |
| 5.3-7 | 离线套件 M0–M19 全部通过（**证明工具行为，不构成现场授权**） | 汇总文件 ______ | `tests/watchdog/run-watchdog-scenarios.ps1:14-36` |

> **能力边界（不得夸大）**：ARMED **不能**对生产实例 `127.0.0.1:8132` 执行（`watchdog:611-628` 直接 exit 40）。生产侧的可达性与配置一致性由 **fire 阶段的只读 GET** 证明（`watchdog:1344-1399`），任何不一致在**发出 Start 之前**以 exit 40 拒绝（`watchdog:1721-1726`）。**不得**把"离线桩自检通过"写成"生产通路已验证"。

### 5.4 FIRED 授权信息

| # | 项 | 填写栏位 | 依据 |
|---|---|---|---|
| 5.4-1 | `-Authorized`（开关） | ☐ 已显式给出 | `watchdog:1466-1474`（缺 ⇒ `REFUSED_NOT_AUTHORIZED`） |
| 5.4-2 | `-AuthorizedBy`（授权人） | ______ | 同上；必须可追溯到人 |
| 5.4-3 | `-AuthzTicket`（授权凭证编号） | ______ | `watchdog:1478-1483` |
| 5.4-4 | `-G6Signoff` + `-G6SignedAt`（G6 现场签署） | ______ / ______ | `watchdog:725-739`：非空、时间可解析、不在未来 |
| 5.4-5 | `-G7DeployVersion`（+`-G7BinaryPath` / `-ExpectedManifestSha256`） | 见 §3.1 | `watchdog:882-1029` |
| 5.4-6 | `-ExpectedTaskRevision` / `-ExpectedPointCount` / `-ApprovedTaskConfig` / `-ExpectedTaskFingerprint` / `-ExpectedApprovedTaskConfigSha256` | 见 §2.3、§4 | `watchdog:1368-1565`（`Test-WdCredTaskConfig`，**#136 版本**；#131 基线对应 `:1288-1455`） |
| 5.4-7 | 全部凭证 `overall = verified`（fire 下四项**全部** `verified`，含 `task_config`） | ______ | `watchdog:1692-1726`；`task_config` 非 `verified` ⇒ exit 40 |
| 5.4-8 | `-SkipHostChecks` **未使用**（否则 ④⑤ 记 `not_verified`，`exit 7`） | ☐ 未使用（必须） | `watchdog:1731-1733`、`:217-219` |

> `exit 40` 一律表示**授权边界拒绝，非设备故障**：未发 Start、未进入 FIRED（`watchdog:54-55`、`:288`）。现场**不得**按"设备没连上"处置，也不得改参数再试。

### 5.5 现场人工核验记录（`family` / `max_read_items` / `timeout_ms`）

> ⚠️ **这是人工项，不是 watchdog 自动门禁**（#130 §4.2）。以下三行必须逐字段填写并由**复核人独立签名**；任一缺失/不一致/无法取证 ⇒ NO-GO，不得进入 FIRED。

| 字段 | 签署值（§2.2） | 本次窗口实测值 | 来源文件 | 一致 |
|---|---|---|---|---|
| `family` | ______ | ______ | 端点原始 GET 响应：______ | ☐ 是 ☐ 否 |
| `max_read_items` | ______ | ______ | 同上 | ☐ 是 ☐ 否 |
| `timeout_ms` | ______ | ______ | 同上 | ☐ 是 ☐ 否 |

| 核验项 | 填写栏位 |
|---|---|
| 原始响应文件名（不裁剪、不摘要化） | ______ |
| 规范化差异（JSON 规范化后 diff）文件名 | ______ |
| 比对执行人 / 时刻 | ______ / ______ |
| **复核人签名 / 日期** | ______ / ______ |
| 结论 | ☐ 三字段全一致（可继续） ☐ 存在不一致（**NO-GO**，缺失字段：______） |

### 5.6 NO-GO 判定表（硬约束 3：逐条列出触发条件）

**任一行为"是" ⇒ 一律 NO-GO，不得进入 FIRED。不得为了让测试通过而调整预期值。**

| # | 触发条件（可判定） | 检查方式 | 本次是否触发 |
|---|---|---|---|
| N1 | §2.4「批准值来源」任一行缺少"由谁/依据什么/何时批准" | 逐行核对 §2.4 | ☐ 是 ☐ 否 |
| N2 | 三个摘要中任一缺失，或与现场实算不一致 | `watchdog:1120-1174`、`:935-962`、`:1025-1028` | ☐ 是 ☐ 否 |
| N3 | 部署身份不明（`-G7DeployVersion` 非 `sha256:`/`manifest:`；二进制定位不到；清单读不到） | `watchdog:992-1009`、`:828-844` | ☐ 是 ☐ 否 |
| N4 | 任务配置不一致（revision / 连接 5 字段 / 逐点 Key 与参数 / 指纹 / **当前任务点数（实时快照）**） | `watchdog:1368-1565`（`Test-WdCredTaskConfig`，**#136 版本**行号；点数判据 `:1515-1518`；#131 基线对应 `:1288-1456`） | ☐ 是 ☐ 否 |
| N5 | 现场门禁任一门（G1–G6）不通过 | §5.1 逐门核对 | ☐ 是 ☐ 否 |
| N6 | 人工核验三字段任一缺失/不一致/无法取证 | §5.5 | ☐ 是 ☐ 否 |
| N7 | 签署栏（§5.7）四方任缺签名、日期或角色重复 | 逐栏核对 | ☐ 是 ☐ 否 |
| N8 | **`BLOCKED-01` 的依赖未满足**：PR #136（B-1 修复）**未合入 `main`** ⇒ 修复不在基线中，仍判 NO-GO（`BLOCKED-01` 本身已由 B-1 关闭） | 核对 PR #136 是否已合入 `main`（`git log main` / PR 状态），并确认部署的 watchdog 为含 `Get-WdTaskPointCount`（`watchdog:1293`）的版本 | ☐ 是 ☐ 否 |
| N9 | 有人修改过任何预期值以"让门禁通过" | 对照 §2.4 批准记录与最终命令行参数 | ☐ 是 ☐ 否 |
| N10 | 使用了 `-SkipHostChecks` 或携带测试注入参数 | `watchdog:1731-1733`、`:654-702` | ☐ 是 ☐ 否 |
| N11 | 存在**遗留的在途 Start**（已发出、未收到明确结果） | #130 §4.7-c；`watchdog:2784`、`:2859` | ☐ 是 ☐ 否 |
| N12 | 窗口内发生过其它写入（并发控制端），导致 `-ExpectedTaskRevision` 可能失配 | G5-②；`watchdog:285` | ☐ 是 ☐ 否 |

**结论栏**：☐ **GO**（仅当 N1–N12 全为"否"且 §5.7 四方签齐） ☐ **NO-GO**（触发编号：______） ☐ **HOLD**（待补：______）

### 5.7 签署栏（四方齐全才生效）

| 角色 | 姓名 | 单位/部门 | 日期时间 | 签字 |
|---|---|---|---|---|
| 执行人 | ______ | ______ | ______ | |
| 复核人 | ______ | ______ | ______ | |
| 现场监护人 | ______ | ______ | ______ | |
| 审批人 | ______ | ______ | ______ | |

**签署时必须同时勾选/填写**：

- ☐ §5.6 的 N1–N12 全部为"否"；NO-GO 结论栏已填 **GO**。
- ☐ 三个摘要（§4-①②③）已填入且来源符合 §4 的批准来源要求。
- ☐ §5.5 人工核验三字段已逐字段比对并由复核人独立签名。
- ☐ `-Mode arm` 退出码 ______（须 0）、`arm-report.json` ______、凭证汇总 ______（须 `verified`）。
- ☐ `BLOCKED-01` **已选 B-1 并实现（PR #136，待合入 main）**；已确认 **#136 已合入 main** ☐；审批人签字 ______。
- ☐ 本人已知晓：**30 秒是调度目标，不是物理停止保证**；`T0` 由 watchdog 在 FIRED 时刻自生成，**签字时不得填写任何具体 `T0` 或截止时刻**（`watchdog:2248-2293`）。

### 5.8 执行顺序（本模板固定顺序；**含门禁前置诊断步骤**）

| 步 | 动作 | 前置条件 | 留证 |
|---|---|---|---|
| 1 | `-Mode arm` 对**离线回环桩**执行（不得指向 8132） | §2–§4 已填；§5.7 已签（`-G6Signoff`/`-G6SignedAt` 本身是 arm 的凭证项，`watchdog:1484`） | `arm-report.json`、`run-log.md` |
| 2 | 抓运行前基线：`GET /endpoints/{id}`、`GET /tasks?endpoint=`、`GET /points/latest`（完整原始响应） | G1–G5 已签字 | 原始响应文件 + SHA-256 |
| 3 | `PUT /api/v1/tasks/{endpoint}`（body = 获批 4 点，预期 revision 18） | 端点已 STOPPED（运行中保存被 `CONFLICT` 拒绝，`core-api:2406-2414`） | PUT 响应 + 读回 `GET /tasks` |
| 4 | **门禁前置诊断（只读）**：记录 `task_point_count_live`（当前任务点数，实时快照，**判据**）与 `diagnostics_point_count_observed`（**观测值**），**两者都记录但只有前者参与判定**；不再要求 `point_count` 与签署值相等 | 第 3 步完成 | `site-log.md` 六列（§2.5） |
| 5 | §5.5 人工核验三字段（`family` / `max_read_items` / `timeout_ms`）+ 复核签名 | 第 2 步的原始响应在案 | 原始响应 + 规范化 diff + 签名 |
| 6 | `-Mode fire -Authorized …`（携带 §4 三个摘要与全部凭证） | 第 1、4、5 步全部通过 | `t0.stamp`、`summary.json` |
| 7 | `T0+10 s±2 s` 抓 `early`；`T0+25~28 s` 抓 `prestop`（**必须在 Stop 之前**） | 第 6 步已进入 FIRED | 两次四件套 |
| 8 | `T0+30 s` watchdog **无条件**发 Stop（执行人不得推迟） | — | `watchdog-stops.jsonl` |
| 9 | 按 §6.6 复核"停干净" → 恢复 `rev17`（§6.4）→ 对账 → 保持 STOPPED | 第 8 步完成 | `summary.json`、恢复读回 |

- 第 4 步是**门禁前置诊断**：`task_point_count_live`（当前任务点数）≠ 签署值 ⇒ **不得进入第 6 步**（否则 fire 必然 `exit 40`，`watchdog:1515-1518`）。`diagnostics_point_count_observed` 只作观测，与签署值不等**不构成**拒绝理由，**也不是**批准值（§2.5 末尾第 4 条）。
- 抓证失败（第 7 步）**不推迟** Stop（#130 §6.2）；抓证进程从 `t0.stamp` **只读**取 `T0`，**不参与** Stop 决策。

---

## 6. 结果及回退

### 6.1 30 秒截止：**调度目标，不是物理停止保证**

| 项 | 事实 | 依据 |
|---|---|---|
| 截止偏移 | `DEADLINE_OFFSET_MS = 30000`，**脚本内常量、无参数入口** | `watchdog:354` |
| 语义 | `t0.stamp` 内写死 `deadline_semantics = 'Stop 请求的截止调度目标（不保证驱动进程/设备通信已在第 30 s 物理终止）'` | `watchdog:2269`、`:63-66` |
| 物理停止 | 由"停干净"六项另行判定（见 §6.6） | `watchdog:2537-2776` |
| Stop 完成时间 | **典型最坏 ≈75–80 s，且不是严格上界**（强杀后第二次 `child.wait()` 无超时） | 诊断方案 `:357`、`:386`；`crates/driver-manager/src/process.rs:279`、`:284` |

**签署栏（承诺项）**：执行人 ☐ 已知晓并同意"30 s 不得读作'第 30 秒已停住'"；监护人 ☐ 已知晓隔离流程（阻断出站 → 杀驱动 → 杀 mesad → 拔测试主机网线）。

### 6.2 三轴验收 + `verdict` / `trial` 分离

| 轴 / 字段 | 含义 | 通过取值 | 填写栏位 | 依据 |
|---|---|---|---|---|
| `axes.schedule.verdict` | 截止 Stop 是否在 `T0+30 s` 准时**发出** | `PASS`（`|deviation| ≤ 1000 ms`） | ______ | `watchdog:2968-2974` |
| `axes.sampling.verdict` | 观测证据是否完整 | `VALID`（无心跳失效/无 > 2000 ms 盲区/无截断/有样本） | ______ | `watchdog:2975-2982`、`:366` |
| `axes.convergence.state` | 设备/进程是否真的停净 | `CONVERGED`（六项全 `converged`） | ______ | `watchdog:2983-2986`、`:2772-2776` |
| `verdict` | **进程处置结论**（决定退出码） | `STOPPED_CLEAN` 且 `exit_code = 0` | ______ | `watchdog:2998-3000`、`:289-300` |
| `trial` | **试验结论**（这轮证据能不能用） | `PASS` | ______ | `watchdog:2866-2871` |

- **三轴不得互相替代**："调度 PASS + 采集 INVALID"是合法组合（场景 M1，`tests/watchdog/run-watchdog-scenarios.ps1`）。
- **`verdict` 与 `trial` 必须分开引用**：不得只引 `trial=PASS` 而隐去 `exit_code`，也不得用 `exit_code=0` 反推"设备侧已安全"（`watchdog:61-62`、`:304-305`）。
- **只有 `exit_code = 0` 可被当作"本轮可判 PASS"**（`watchdog:300`）。

### 6.3 立即停止条件（**首次出现即停，不等第二次**）

| 编号 | 触发（可判定） | 依据 | 动作 |
|---|---|---|---|
| S1 | `detail` 含 `S7_JOB_0x8201`（作业级拒绝） | 诊断方案 `:338` | **首次即 Stop** |
| S2 | 其它通信失败（`COTP_RECV_FAIL`/`early eof`、`CONNECT_TIMEOUT`、首次确认的 `POLL_FAIL`） | 诊断方案 `:19`、`:344` | 立即 Stop（**不设"连续 N 次"宽限**） |
| S3 | 异常重连：除 Start 那一次外 `runtime.epoch` 再变 ≥1 次；或 `RECONNECTING` ≥10 s | 诊断方案 `:343`（收紧到 10 s） | 立即 Stop |
| S4 | 机床异常：报警、急停、伺服掉使能、异响；或操作员要求停 | 诊断方案 `:340` | 立即 Stop（**人工优先于脚本**） |
| S5 | 监护人叫停（任何理由，无需解释） | 人工 | 立即 Stop |
| S6 | 状态/数据不自洽（非预期 `STOPPED`/`FAILED`；筛选后目标端点条数 ≠ 4；任一点 `timestamp_ns` 停滞 ≥3×interval；`quality ≠ GOOD`） | 诊断方案 `:341`；清单 `:153`、`:178` | 立即 Stop |

- watchdog 侧对应 `EARLY_TRIGGER` → `exit 5`（`watchdog:2321-2326`、`:2861`）。
- **统一流程**：Stop（幂等）→ 六项复核（§6.6）→ 落盘（时刻/触发条款/`detail`/证据文件）→ 判 **NO-GO 或 INVALID** → **不自动重试、不自动进入下一轮**。

### 6.4 rev17 回退依据（**执行窗口内重新 GET + 重算 SHA-256**）

| # | 步骤 | 要求 | 填写栏位 |
|---|---|---|---|
| 6.4-1 | 窗口内重新 `GET /api/v1/endpoints/lx-nck-tags-190-connection` 与 `GET /api/v1/tasks?endpoint=lx-nck-tags-190-connection` | 原样保存响应文本（不重排 JSON） | 文件名 ______ / ______ |
| 6.4-2 | 对本次文件计算 SHA-256（`Get-FileHash -Algorithm SHA256`） | 与 #130 §6.4 基线**逐字符**比对 | 本次值 ______ / 基线值 ______ |
| 6.4-3 | 记录五列到 `site-log.md` | 命令 / 时刻 / 本次值 / 基线值 / 结论 | 结论 ______ |
| 6.4-4 | 核对 `connection` 全 8 字段 | 5 个自动 + 3 个人工（§5.2） | ☐ 一致 ☐ 不一致 |

> **不得引用文档旧哈希**：#130 §6.4/§8.1 记录的 SHA-256 是**编制时**（2026-10-20）的本地结果，只是基准参照。未在窗口内重验 ⇒ 视为**未验证** ⇒ **NO-GO**（不得"按文档哈希认定一致"后继续）。
>
> 回退体必须是执行前按上述步骤重验通过的那一份 `rev17-rollback-put.json`（编制时基线 SHA-256 `10515C712FC9AF27D8B6FDF229764A88FE3E4E7DAC3B7FAC7F01637F21DFDF42`，**仅作参照**）。

### 6.5 NCK 端点开机**一律不自动恢复** ⇒ 回退后重启必须**显式 Start**

- `crates/core-api/src/recovery.rs:29-37`：`boot_auto_restore_allowed(driver_id) = driver_id != "sinumerik-nck"`（精确匹配；`sinumerik-nck-sim` / `SINUMERIK-NCK` 不受影响，`recovery.rs:53-63`）。
- `apps/mesad/src/main.rs:184-198`：该判定在**正向凭证之前**直接 `continue`——即使凭证完整，NCK 也不得被自动拉起。
- **现场结论**：恢复完成后即使重启 `mesad`，该端点**也不会**被自动拉起，这是**期望行为**；需要再次采集必须**显式 `POST /api/v1/endpoints/{id}/start`**，且属**新的申请**。
- **边界（不得夸大）**：该结论**只对 `sinumerik-nck` 成立**，是**一期产品约束、不是通用生命周期语义**（G7 设计 §6.15）。

| 核验项 | 填写栏位 |
|---|---|
| 恢复后是否重启过一次 `mesad` 以观察 | ☐ 是 ☐ 否 |
| 重启后该端点是否被自动拉起 | ☐ 否（期望） ☐ 是（异常，记 §6.6 异常行） |
| 是否有人为"再采一次"而隐式 Start | ☐ 否（必须） |

### 6.6 最终 STOPPED 核验（G7 六条判据）+ 证据归档清单

**第一层：Core 内部六条判据**（`crates/core-api/src/lifecycle.rs:986-1046`，判据表达式 `:1011-1017`；只证明"Core 认为自己停了"，**不是**现场判据）：

| # | 判据 | 实现信号 |
|---|---|---|
| 1 | 当前代次为 `STOPPED` | `snapshot_gen == Some(cur)` 且 `snapshot_state == "STOPPED"`（`:1011-1012`） |
| 2 | `desired_running=false` 且本次 Stop 落盘已确认 | `stop_gen == target_gen` && `stop_persist_confirmed_gen == Some(target_gen)` && `stop_persist_inflight.is_none()`（`:1006-1009`） |
| 3 | 无在途 Start / Stop / 意图落盘 | `inflight_starts == 0`（`:1017`）+ attempt 表 `phase` + `stop_persist_inflight` |
| 4 | 无未完成的 attempt（含孤儿） | `entry_absent`（`:1013`） |
| 5 | 驱动进程与 TCP 会话收敛 | `TeardownDone(gen)` + `LIVE_CHILDREN`（G7 设计 §7.1-5；**reader 结果未纳入判据，属已知缺口**） |
| 6 | 经过规定的稳定观察窗口 | `SETTLE_TICKS = 2 × SETTLE_TICK = 250 ms`（`:42-43`、`:1018-1022`） |

**第二层：现场"停干净"六项（外部可观测，watchdog 自动复核，三态）**（`watchdog:2537-2776`）：

| # | 项 | 三态取值 | 填写栏位 |
|---|---|---|---|
| ① | Stop HTTP 结果 | 200/500 ⇒ `converged`；无响应 ⇒ `indeterminate` | ______ |
| ② | `state = STOPPED` | 最终观测值 | ______ |
| ③ | `desired_running = false` | 最终观测值 | ______ |
| ④ | 驱动进程为空（只认目标 `driver_id`） | 查询失败 ⇒ `indeterminate` | ______ |
| ⑤ | 到设备 TCP 无 ESTABLISHED（3389 单列排除） | 无法枚举 ⇒ `indeterminate` | ______ |
| ⑥ | 稳定窗口 10 s 内无新 `epoch`、无 `RECONNECTING` | 无样本 ⇒ `indeterminate` | ______ |

- 三态优先级：`not_converged > indeterminate > not_verified > converged`（`watchdog:2772-2776`）；**`indeterminate` 绝不等于"已停净"**（`watchdog:2538-2539`）。
- **禁止的等价替换**：不得用 `is_running()==false`、`desired_running==false`、`state=="STOPPED"` 单独或任意组合代替第一层六条（G7 设计 `:1184`）。

**证据归档清单（缺一即 INVALID；每行填文件名 + SHA-256）**：

| # | 证据 | 谁产出 | 文件名 | SHA-256 |
|---|---|---|---|---|
| 1 | `t0.stamp`（含 `deadline_semantics`） | watchdog（`watchdog:2261-2293`） | ______ | ______ |
| 2 | `run-log.md`（**启动即覆盖**，人工记录不得写同名文件） | watchdog（`watchdog:418`、`:448`） | ______ | ______ |
| 3 | `watchdog-states.jsonl`（每 200 ms 一条） | watchdog（`watchdog:420`） | ______ | ______ |
| 4 | `watchdog-stops.jsonl` | watchdog（`watchdog:421`） | ______ | ______ |
| 5 | `summary.json`（含 `axes` / `verdict` / `trial` / `exit_code`） | watchdog（`watchdog:422`、`:2875-3004`） | ______ | ______ |
| 6 | `arm-report.json` | watchdog（`watchdog:424`） | ______ | ______ |
| 7 | `stop-selftest.json`（仅 `-SelfTest` 时） | watchdog（`watchdog:423`） | ______ | ______ |
| 8 | `early` 四件套（`when.json`/`state.json`/`points-latest.raw.json`/`state-after.json`） | 抓证脚本（#130 §6.2） | ______ | ______ |
| 9 | `prestop` 四件套 | 同上 | ______ | ______ |
| 10 | `site-log.md`（人工记录：谁/何时/做了什么/异常处置） | 执行人 | ______ | ______ |
| 11 | 本签署包（含 §4 三个摘要） | 审批人 | ______ | ______ |
| 12 | §5.5 人工核验的原始响应 + 规范化 diff | 执行人/复核人 | ______ | ______ |

- **保存顺序（强制）**：`points/latest` 先**原样落盘完整响应**（全端点），再在分析阶段按 `endpoint_id` 筛出目标 4 条；顶层 `count` 是**全端点**总数，不得当作目标端点条数（#130 §6.2、§6.3）。
- **日志口径**：进程日志不落盘是既定环境事实，**不以任何进程日志作为判据**；日志缺失对判定中性（#130 §6.3）。**若无法持续观测端点状态 ⇒ INVALID**。
- **`diagnostics_point_count_observed` 不得用于恢复对账**：该观测值是持久化残留量（`point_registry` 中 `deleted=0` 的行数，`core-api:1499` ← `config-store:2459-2473`），恢复 `rev17`（238 点）只写 `tasks` 表，**不写** `point_registry`（§2.5-证据 #4、写入时序块），因此恢复后该观测值仍是上次运行的点数（本轮为 4），与 `GET /tasks` 的 238 点不一致**是预期现象**。恢复对账只认 `GET /tasks` 的内容与点数（#130 §8.3）。

---

## 7. 阻塞项与待确认清单

| # | 项 | 状态 | 为什么不能在本模板里写死 | 谁来解决 |
|---|---|---|---|---|
| **BLOCKED-01** | **`-ExpectedPointCount` 的时序语义**（§2.5） | **已关闭（B-1，PR #136）**；**依赖 #136 合入 main**（未合入 ⇒ 仍判 NO-GO，§5.6-N8） | 关闭前的问题：`point_count` 来自持久化 `point_registry`（`crates/core-api/src/lib.rs:1499` → `crates/config-store/src/lib.rs:2459-2473`），只由 Start 时的 `assign` 写入（`crates/driver-manager/src/endpoint.rs:206-224`，调用点 `:213` ← `run_config_flow:943-945`），PUT 不写它（`crates/core-api/src/lib.rs:2415-2419`），因此不存在"由获批配置推导、又必然等于现场残留"的值。#136 改为核验**当前任务点数**（实时快照 `tasks[].binding.config.points[]`，`watchdog:1293`/`:1515-1518`），`diagnostics.point_count` 降级为 `diagnostics_point_count_observed`（observed-only） | 已由代码修复解决；**待 #136 合入 main**（合入前本模板不得先行合并） |
| 2 | `-ExpectedTaskRevision` 的时序 | **待确认** | 签署包在窗口之前准备，无法预知窗口内是否发生其它写入；一旦偏移 ⇒ `REFUSED_TASKCONFIG_MISMATCH`（`watchdog:285`） | 审批时一并签署；窗口内禁止任何其它写入（G5-②） |
| 3 | G2 的"协商 PDU 核对" | **待确认** | `mode=probe` 会发起真实机床 I/O，需**单独授权**（#130 §4-G2） | 审批人单独授权或明确"不做" |
| 4 | MAC `98-fa-9b-59-35-53` | **待确认** | 仓库内无留存（全仓检索无匹配） | 现场补充截图/书面记录 |
| 5 | G7 判据 5 的 reader 结果未纳入判定 | **待确认（已知缺口）** | `wait_reader_done` 结果目前只用于一条 `warn!`（G7 设计 §7.1-5） | 后续代码修复；本轮靠第二层判据 ⑤ 兜底 |
| 6 | Core 内部 500 ms 收敛窗口是否需按 endpoint 配置 | **待确认** | G7 设计 §7.3 末标待确认 | 后续设计决策；本轮不依赖它作现场判据 |
| 7 | `sinumerik-nck` 之外的驱动仍有"重启误拉起"路径 | **不适用（本轮不涉及）** | G7 设计 §6.10.3 明确未消除 | 产品决策；不得外推 |
| 8 | ARMED 无法对生产 8132 执行 | **接受（流程层）** | `watchdog:611-628` 冻结为回环且端口 ≠ 8132 | 现场交底写清；"对生产端点的独立通路自检"属新功能 |
| 9 | 真机停止时刻无法由离线手段证明 | **接受** | 30 s 是调度目标；尾部可能无界（`process.rs:284`） | 监护人按 #130 §5.3 处置 |
| 10 | `max_read_items`/`timeout_ms`/`family` 不由 watchdog 自动验证 | **接受（人工项）** | 自动比对只覆盖 5 字段（`watchdog:1180-1186`） | 人工核验（§5.5）+ 复核签名 |

---

## 附录 A：凭证 → 参数 → 代码位置（供复核人逐条抽查）

| 凭证 / 检查项 | 参数 | 代码位置 | 拒绝标签 / 退出码 |
|---|---|---|---|
| 显式授权 | `-Authorized`、`-AuthorizedBy` | `watchdog:1466-1474` | `REFUSED_NOT_AUTHORIZED` / 40 |
| 授权凭证编号 | `-AuthzTicket` | `watchdog:1478-1483` | 同上 |
| G6 现场签署 | `-G6Signoff`、`-G6SignedAt` | `watchdog:725-739` | `REFUSED_CREDENTIAL_INVALID` / 40 |
| G7 部署身份 | `-G7DeployVersion`、`-G7BinaryPath`、`-ExpectedManifestSha256` | `watchdog:882-1029` | `REFUSED_MANIFEST_INTEGRITY` / `REFUSED_CREDENTIAL_INVALID` / `REFUSED_CREDENTIAL_UNVERIFIABLE` / 40 |
| 获批配置自身完整性（G2） | `-ExpectedApprovedTaskConfigSha256` | `watchdog:1120-1174`、`:1326-1330` | `REFUSED_TASKCONFIG_INTEGRITY` / 40 |
| 任务 revision | `-ExpectedTaskRevision` | `watchdog:1405-1406` | `REFUSED_TASKCONFIG_MISMATCH` / 40 |
| **当前任务点数**（配置门判据） | `-ExpectedPointCount` | `watchdog:1509-1518`（判据块，点数判据本体 `:1515-1518`）、`watchdog:1293`（`Get-WdTaskPointCount` 计算） | 同上（**语义见 §2.5**；`diagnostics_point_count_observed` 为观测值，不参与判定） |
| 连接配置 5 字段（自动） | `-ApprovedTaskConfig` 的 `connection` | `watchdog:1176-1216`、`:1420-1421` | 同上 |
| 逐点 Key 与参数 | 同上（`tasks[].binding.config.points[]`） | `watchdog:1218-1250`、`:1429-1438` | 同上 |
| 规范化 tasks 指纹 | `-ExpectedTaskFingerprint` | `watchdog:1056-1075`、`:1339-1342`、`:1440-1443` | 同上 |
| 只读 GET 三源 | — | `watchdog:1344-1399`（diagnostics / tasks / endpoint） | 读不到 ⇒ `unverified` / 40 |
| 离线硬门 | `-BaseUrl` | `watchdog:611-628` | `REFUSED_NON_OFFLINE` / 40 |
| 测试注入边界 | `-TestHostProbeError`、`-TestAbortFlow` | `watchdog:654-702` | `REFUSED_TEST_INJECTION_ONLINE` / 40 |
| 截止 Stop 调度 | `DEADLINE_OFFSET_MS` | `watchdog:354`、`:2251-2299` | 调度轴 `PASS/FAIL/UNKNOWN` |
| 三轴与结论 | — | `watchdog:2966-3000` | `verdict` / `trial` / `exit_code` |
| 现场六项收敛 | — | `watchdog:2537-2776` | `CONVERGED` / `NOT_CONVERGED` / `INDETERMINATE` / `NOT_VERIFIED` |
| Core 六条判据 | — | `crates/core-api/src/lifecycle.rs:986-1046` | `Settled` / `NotSettled{...}` |
| NCK 开机不自动恢复 | — | `crates/core-api/src/recovery.rs:29-37`、`apps/mesad/src/main.rs:184-198` | — |
| 冻结 Stop 错误码（8 个） | — | `tests/driver-contract/tests/stop_lifecycle.rs:68-83` | — |

## 附录 B：命令骨架（参数名全部来自 `scripts/stop-watchdog.ps1:307-346`；值一律留空待签）

```powershell
# 阶段 A：ARMED（必须显式给离线回环桩地址；缺省 -BaseUrl 是 8132，会被 exit 40 拒绝）
pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode arm `
  -BaseUrl http://127.0.0.1:<离线桩端口> -EndpointId <endpoint-id> -OutDir <本轮证据目录> `
  -ExpectedTaskRevision <签署值> -ExpectedPointCount <签署值> -ApprovedTaskConfig <签署包路径> `
  -ExpectedTaskFingerprint sha256:<签署值> -ExpectedApprovedTaskConfigSha256 sha256:<签署值> `
  -G7DeployVersion <sha256:... | manifest:<path>> -ExpectedManifestSha256 sha256:<签署值>

# 阶段 B：FIRED（必须显式授权；四项凭证全部 verified 才会放行 Start）
pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode fire `
  -BaseUrl http://127.0.0.1:8132 -EndpointId <endpoint-id> -OutDir <本轮证据目录> `
  -Authorized -AuthorizedBy <姓名> -AuthzTicket <编号> `
  -G6Signoff <签署编号> -G6SignedAt <ISO8601> `
  -G7DeployVersion <sha256:... | manifest:<path>> -G7BinaryPath <部署二进制路径> `
  -ExpectedManifestSha256 sha256:<签署值> `
  -ExpectedTaskRevision <签署值> -ExpectedPointCount <签署值> `
  -ApprovedTaskConfig <签署包路径> -ExpectedTaskFingerprint sha256:<签署值> `
  -ExpectedApprovedTaskConfigSha256 sha256:<签署值>
```

> 骨架**不是**可直接执行的命令：所有 `<...>` 必须由 §2–§4 的签署值填入，且须先**确认 PR #136 已合入 main**（`BLOCKED-01` 已由 B-1 关闭）。**不得**携带 `-TestHostProbeError` / `-TestAbortFlow` / `-SkipHostChecks`（`watchdog:654-702`、`:1731-1733`）。

## 附录 C：填写完整性自检（提交签署前逐项打勾）

- ☐ §1.1 九项身份栏位全部填写，机型四证（§1.2）齐备
- ☐ §2.1 四点逐字段与 `approved-task-config.json` 逐字一致；§2.2 八字段齐备且标注 5 自动 / 3 人工
- ☐ §2.3 四项（revision / 点位数 / 指纹 / 文件摘要）同源；§2.4 每行都有"由谁 / 依据什么 / 何时批准"
- ☐ §2.5 已**确认 PR #136 已合入 main**（`BLOCKED-01` 已由 B-1 关闭）；§3 部署身份三件套齐备且未用 git 标识充当通过依据
- ☐ §4 三个摘要各自写明批准来源，且无现场现算；§5.1 六道门均为**本次窗口**新证据
- ☐ §5.2 自动 / 人工两列未混写；§5.5 人工三字段有独立签名；§5.6 N1–N12 全为"否"
- ☐ §5.7 四方签名齐全；§5.8 九步顺序已交底；§6 的核验项与证据清单全部填写

---

**模板结束。再次声明：本文件是模板；未填写、未签署、未复核的模板不构成任何执行授权。**
