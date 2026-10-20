# 828D 一期首轮限定采样执行申请（真机验收标准模板 v1）

- **文档性质：申请，尚未获批。** 本文档不构成执行授权；只有 §10 裁决为 **GO** 且四方签字齐全后，才可按 §5 执行。**未获批准不得 Start。**
- **模板用法**：每轮复制本文件 → 改写 §1 空白、§3 点集、§5 时间窗 → **重跑 §4 五道门（用本轮新证据）** → §10 重新裁决。**上一轮的 GO 不延续到下一轮。**
- 实施基线：`main = fda7633a6d675c43f5bab58f662667aac4a54daf`（编制前工作树干净；本文件为唯一新增）；本轮 **R1**；版本 v1.0；编制日期 2026-10-20。
- 编制声明与口径：本次**只新增本文件**，未改代码/测试/配置/既有文档，未做 git 写操作，未连设备、未启服务、未跑测试或构建；事实给 `文件:行号`，无法由代码/产物确定的标 **待确认**；**不夸大**——30 秒 ≠ Stop 完成时间，读到值 ≠ 业务可用，rev15 可用是**已证明**的而非假定的。
- 简称：**诊断方案** = `docs/design/nck-s7-job-0x8201-diagnosis-plan.md`；**清单** = `docs/design/nck-828d-phase1-p0-points.md`；**质量调查** = `docs/design/quality-code-observability-investigation.md`；**D1** = `docs/design/pr-d1-quality-code-visibility-design.md`。

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
| ① | **Stop 非即时**：最坏 ≈75–80 s，期间可能仍完成一次 Start | 诊断方案 `:395`、`:398` | 不假设"Stop 立即生效"；见 §5.3、§7 |
| ② | **机型身份无法由软件证明**：`family`/`model` 恒 `None`，`NCK_ANCHOR_PENDING` 恒在 | `drivers/sinumerik-nck/src/probe.rs:26`、`:49-64`；`GATE.md:91-92` | 只能靠铭牌 + HMI + X130 + 工位（§4-G1） |
| ③ | **Core/驱动日志当前未落盘** | 诊断方案 `:171`、`:425` | 执行前须解决；未解决按 §6.3 记证据缺口 |
| ④ | **`runtime.revision=16` / `runtime.points=4` 是上次运行快照**，恢复 238 点后也不变 | `rev17-endpoint.raw.json`；`endpoint.rs:230-231`、`:431` | 恢复对账只认 `GET /tasks`（§8.3） |
| ⑤ | **值语义未确认**（状态 `5`、参数 `1`、位置 `0.0`） | 清单 `:42`、`:73`、`:235` | 只判 GOOD/新鲜/窗口内推进，**不判语义**；与 ② 一样，软件侧无法自证 |

## 3. 精确配置

### 3.1 四点完整参数（逐字段）

| # | `point_key` | kind | 逐字段参数 | wire 地址 | dtype（decode） | 真机证据 |
|---|---|---|---|---|---|---|
| 1 | `native.C.1.122.5.1` | `nck_native` | `area=C` · `area_no=1` · `module=122` · `column=5` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `C.1.122.5.1` | `UTF8_NUL_LAST_32` | T2a + T2b |
| 2 | `native.A.1.26.30110.1` | `nck_native` | `area=A` · `area_no=1` · `module=26` · `column=30110` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `A.1.26.30110.1` | `U8` | T2b |
| 3 | `native.C.1.127.13.1` | `nck_native` | `area=C` · `area_no=1` · `module=127` · `column=13` · `line=1` · `count=1` · `unit_mode=current` · `transport_size=0x09` | `C.1.127.13.1` | `U16_LE` | T2b |
| 4 | `axis.position.actual_bcs@axis=i:2,channel=i:1` | `nck_variable` | `variable_id=axis.position.actual_bcs` · `definition_revision=1` · `target={channel:1, axis:2}` · `arguments={}` | `C.1.112.33.2`（定义编译，非手写） | `F64_LE` | T2b |

- 四个 wire 地址**互不相同**（静态核查）：`C.1.122.5.1` / `A.1.26.30110.1` / `C.1.127.13.1` / `C.1.112.33.2`。`decode` 即 dtype；本轮发送 9 个字段（8 参数 + `decode`），全部落在白名单 `ALLOWED`（12 项，`drivers/sinumerik-nck/src/native.rs:147-160`）内，**不发送**可选的 `invalid_value`/`invalid_tolerance`/`unit`，也不许新增字段；点 4 的 `arguments` 必须为空对象 `{}`，不得写 `null` 或省略。

### 3.2 Connection 参数（来源 `rev17-endpoint.raw.json` 的 `connection`，658 B；**本轮禁止修改任一字段**，需改即作废本申请）

| 字段 | 值 | 字段 | 值 |
|---|---|---|---|
| `host` | `192.168.15.60` | `remote_tsap` | `3332` |
| `port` | `102` | `pdu_length` | `480` |
| `local_tsap` | `1024` | `max_read_items` | **`1`** |
| `timeout_ms` | `1000` | `family` | `828d` |

### 3.3 Task 参数

`endpoint_id=lx-nck-tags-190-connection`；单任务 `id=t1`；`schedule={interval_ms:5000, mode:"poll"}`；`binding.kind=nck.points.v1`；`binding.config.points` = §3.1 的 4 点，**顺序不变**。

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
| 运行观察窗口 | **≤30 秒** |
| 读取模式 | **只读**（不 Write、不 Command，Control 保持 `disabled`） |
| 自动重试 | **不允许**（任何失败一律 Stop + 记录） |
| 扩点 / 自动进入第二轮 | **均不允许** |
| 完成后状态 | **STOPPED**（`desired_running=false`） |

**变更控制**：冻结表内任一项变更（含"只多读 1 点""5000 改 4000""只延长 10 秒"）都使本申请**立即失效**，须重新编制、重过 §4、重新裁决；执行中发现配置与本文档不符 ⇒ 按 §7-S6 立即 Stop。

## 4. 现场五道前置门（**必须使用本次执行窗口的新证据**）

> **授权不延续**：T2b 的通过结果、上一轮门禁记录、旧截图**一律不得复用**。每道门都须在**本次窗口内**重新取证、重新签字。任一门不通过 ⇒ **不 Start**（记 NO-GO，不得"先跑再说"）。

| 门 | 要求（本次新证据） | 证据形式 | 为什么必须 | 不满足时 |
|---|---|---|---|---|
| **G1 机床身份** | ① 铭牌确认 **SINUMERIK 828D**（型号 + 序列号逐字抄录）② HMI 机床/系统信息页 ③ X130 网络口现场确认（指示灯、端口标签）④ 工位信息与申请一致 | 铭牌照片 + HMI 截图 + X130 照片 + 工位牌照片，**均带拍摄时间** | 软件侧 `family`/`model` **恒为 `None`**（`probe.rs:49-64`）、`NCK_ANCHOR_PENDING` 恒在（`probe.rs:26`、`GATE.md:91-92`）⇒ **无法由软件证明机型** | 停止，NO-GO |
| **G2 网络参数** | ① HMI 实际 IP 仍为 `192.168.15.60`（网络设置页）② `remote_tsap=3332`、`pdu_length=480` 与目标机床一致（`connection-check` 的 `mode=probe` 协商 PDU，`core-api:3286`）③ **该地址无其他设备占用** | HMI 网络页截图 + `10-probe.json`（`reachable`、协商 PDU）+ 交换机/网口核对记录（`arp -a` 的 MAC 仅作辅助对照） | 历史上**同 IP 曾换过设备**（诊断方案 `:18`、`:541`）；probe 只证"连到 S7Comm 对端"，**不证身份**（`:185`） | 先做单次 probe 核对；仍不符 ⇒ 停止 |
| **G3 机床状态** | 无报警、无急停、安全静止；只读采集不影响当前加工安排（负责人确认） | 机床状态表（上电/急停/驱动使能/参考完成/程序运行/轴是否运动/报警号/操作员口述，诊断方案 `:159`）+ 签字 | 安全前提；状态变化会让"值是否合理"无法判读 | 记录状态后停止 |
| **G4 现场监护** | 有权限监护人在场，具备 **人工 Stop** 与 **网络隔离**（拔**测试主机**网线）能力，并明确窗口时长 | 监护人姓名/电话 + 能力确认签字（§10.2） | **Stop 可能不及时**（最坏 ≈75–80 s，§5.3）；远程 Stop 不是安全保证 | 不开始 |
| **G5 备份与并发** | ① `rev17` 备份已完成且 SHA 已核（**§8.1 已给出**）② **无其他控制端**同时操作该 Endpoint（另一 Mesa 控制台、HMI 侧采集工具、他人 REST 调用） | 备份三件套的 `Get-FileHash` 输出 + 并发确认（`GET /endpoints` 观察 + 现场唯一操作员） | 防止并发写入把"复现"变成"改配置"；恢复依赖备份 | 不开始 |

**关于 MAC `98-fa-9b-59-35-53`（仅辅助对照）**：该值目前**仅来自现场记录，仓库内无留存**（全仓检索无匹配，**待现场补充截图或书面记录**）。即便一致，**单靠 MAC 或 IP 不足以证明一定连接的是指定 828D**（MAC 可换网卡、IP 可复用；诊断方案 `:215-217` 明确"名称/ID 是人工标签，不是身份证据"）。MAC **只能**作为与 HMI + X130 + 工位信息相互印证的一环，**不得单独作为 G1 通过依据**。五门签字另附页，格式同 §10.2（确认人 / 确认时间 / 证据文件名；缺一即未通过）。

## 5. 执行时间窗

### 5.1 窗口定义

| 项 | 值 |
|---|---|
| 计划开始 / 结束（本地时间） | **待填** / **待填**（结束须含 Stop 与恢复，留 ≥5 min 余量） |
| `interval_ms` | `5000` |
| 运行观察窗口 / 截止 | **≤30 秒**（硬上限）；截止 = `T0 + 30 s`，到点**立即**发 Stop（§5.3） |
| 计时起点 `T0` | `POST /endpoints/{id}/start` **请求发出**的本地墙钟时刻（**含建连时间**，不因"还没 RUNNING"而延后截止） |
| 预期批次 | 5–6 轮（30 s ÷ 5 s；首轮相位取决于建连耗时，**不承诺**恰好 6 轮） |

### 5.2 执行顺序（只读，逐步留证）

1. §4 五门签字完成；抓**运行前**基线：`GET /endpoints/{id}`、`GET /tasks?endpoint=`、`GET /points/latest`（§6.3）。
2. `PUT /api/v1/tasks/lx-nck-tags-190-connection`，body = `{"tasks":[ …§3.1 四点… ]}`（结构与 `t2b-task.json` 同形，预期新 revision **18**）。保存会触发一次 Descriptor 校验（临时驱动进程，仅 handshake + GetDescriptor，`manager.rs:579-599`），**不建立到机床的 NCK 连接**；运行中保存被 `CONFLICT` 拒绝（`core-api:2297-2305`）。
3. `POST /endpoints/{id}/start`，记录 `T0`。
4. `T0+10 s±2 s` 抓 **`early`**；`T0+25~28 s` 抓 **`prestop`**（**必须在 Stop 之前**）。
5. `T0+30 s` **立即** `POST /endpoints/{id}/stop`（客户端 `-TimeoutSec 120`，诊断方案 `:351`）。
6. 按 §5.3 五项判据复核"停干净"；未干净 ⇒ 现场处置（§5.3）。
7. 恢复 `rev17`（§8.2）→ 对账（§8.3）→ 保持 STOPPED。

### 5.3 Stop 时间约束（**本轮最关键的一条，不得改写**）

> **30 秒是"运行观察窗口"，不是"Stop 完成时间"。**

- **Stop 完成时间最坏可能达约 75–80 秒**（诊断方案 `:357`、`:386`）：teardown 小计 ≈35 s（post 5 s + terminate 5+5 s + reader 5 s + drain 15 s，`:375-383`）+ **取消覆盖不到的**建连/配置闭环 ≈46 s（`DRIVER_STARTUP_TIMEOUT` 6 s + 4 个 RPC × 10 s，`:384`）。故 Stop 请求的**客户端超时必须 ≥120 s**（`:351`）；HTTP 层无全局超时，服务端会挂到 teardown 结束（`:386`）。
- **三条硬性纪律（逐字保留）**：
  1. **到达运行截止时间必须立即发出 Stop，不得为了等 `prestop` 快照而延长采集。**
  2. **提前触发停止条件时，立即执行停止流程**（§7），不等到截止时间。
  3. **Stop 未能及时完成时，由现场人员按批准的安全处置流程处理，不得假定远程 Stop 必然即时生效。**
- **现场安全处置（批准后按序）**：① 继续等 Stop（≤120 s，期间只读状态，不重复点 Stop）；② REST 不可用或超时 ⇒ **先阻断出站**（防火墙 Block `192.168.15.60:102`，对机床零风险，诊断方案 `:405`）；③ 仍不收敛 ⇒ 杀驱动进程（`:404`，会触发重连）；④ 最后才杀 mesad（`:406`，须先确认 `desired_running=false`）；⑤ **物理隔离只拔测试主机网线**，不得擅自断产线交换机口（`:407` 的 E4 会影响整线）。
- **"停干净"五项判据**（缺一不算）：HTTP 200/500 + `state=STOPPED` + `desired_running=false` + 驱动进程空 + 到 `192.168.15.60:102` 无 Established（`:398`）。**看到 500 不等于没停住**（drain 失败也 500，但 `desired_running` 仍落 false，`:396`）。

### 5.4 冷却与总量

单次 ≤5 min、单日累计 ≤30 min、**≤5 次启停**、两次运行间隔 **≥30 s**（等旧连接彻底关闭）——全部出自诊断方案 `:440`。本轮实际 ≤30 s + 1 次启停，远低于上限。**超窗**（计划结束仍未恢复完成）⇒ 本轮作废，记 NO-GO/INVALID，**不得**"延长窗口再跑一次"。

## 6. 证据保存

### 6.1 目录与命名

`target/validation/828d-r1/round1-<UTC 时间戳>/`（`/target/validation/` 已被 `.gitignore:55` 忽略，不进仓）。文件名前缀 `<UTC>-<label>-<endpoint>`，`label ∈ {early, prestop, pre, post, probe, log}`；目录内必须有 `run-log.md`（字段沿用诊断方案 `:478`）。

### 6.2 运行中原始快照（两次，缺一即 INVALID）

| 标签 | 时机 | 内容 | 工具 |
|---|---|---|---|
| `early` | `T0+10 s±2 s`（**尽早**，不要等最后） | `when.json`（本地墙钟 + Unix ns）、`state.json`、`points-latest.raw.json`（**逐字节完整响应，不裁剪不摘要**）、`state-after.json` | `target/validation/capture-points.ps1 -Label early`（`:15-31`） |
| `prestop` | `T0+25~28 s`，**Stop 之前** | 同上四件套 | 同上 `-Label prestop` |

- 两次抓取之间**必须**看到批次推进（T2b 实测 25.001 s = 5×5000 ms）；两次 `timestamp_ns` 相同 ⇒ 采集停滞，按 §7-S6 停。抓取期间状态变化（`state.json` 与 `state-after.json` 不一致）⇒ 该样本**作废**（`capture-points.ps1:36-38`）。

### 6.3 前后端点状态与日志

`GET /endpoints/{id}`（运行前/后）记录 `state`、`desired_running`、`runtime.epoch`、`runtime.revision`、`runtime.points`、`connection` 全字段；`GET /tasks?endpoint=`（运行前/恢复后）记录 `{endpoint_id, revision, tasks}`（`core-api:2223`）；`GET /points/latest` 只取该端点 4 条，记 `quality`/`quality_code`/`value_origin`/`timestamp_ns`/`value`。

**日志当前状态**：Core 日志**未落盘**（mesad 由外层 `pwsh … 2>&1` 启动，`stdout/stderr` 未重定向，诊断方案 `:171`、`:425`）；驱动 `stdout/stderr` 被 `inherit`（`process.rs:70-72`）。**执行前必须解决**（重定向到 §6.1 的 `mesad-console.log`）。未解决时本轮**仍可执行**，但须在 §10 记**证据缺口**——失败时可能缺时序证据，判定只能落 INVALID，不得凭"没看到错误"判 PASS。

### 6.4 SHA-256 清单（执行时逐文件填写；基线值本次编制时已实测）

| 文件 | 字节 | SHA-256 |
|---|---|---|
| `828d-r1/rev17-endpoint.raw.json` | 658 | `B3D2D3F0AC0D438E0B1DB66FCDA6D0564DDB620EDE7E3F359309E561B83BB659` |
| `828d-r1/rev17-tasks.raw.json` | 111,992 | `CE1EEEB28C1BC2A54B3A0A0781E2F966C495516749B74782A444D06AEE5E96F1` |
| `828d-r1/rev17-rollback-put.json` / `t2b-rollback-rev15.json` | 111,925 / 111,925 | `10515C712FC9AF27D8B6FDF229764A88FE3E4E7DAC3B7FAC7F01637F21DFDF42`（两文件同值） |
| 本轮文件（`early`/`prestop` 四件套、`pre`/`post` 快照、`mesad-console.log`） | 待填 | 待填（每个文件一行） |

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
| **S1** | `detail` 含 **`S7_JOB_0x8201`**（作业级拒绝） | 诊断方案 `:338` | **首次即 Stop**（比其 S2"第 2 次"更严；本轮不承担复现归因） |
| **S2** | 其它通信失败：`COTP_RECV_FAIL`/`early eof`、`CONNECT_TIMEOUT`、连续 3 次 `POLL_FAIL` | 诊断方案 `:19`、`:344` | 立即 Stop |
| **S3** | 异常重连：除 Start 那一次外 `runtime.epoch` 再变 ≥1 次；或 `RECONNECTING` ≥10 s | 诊断方案 `:343`（其 30 s 阈值对本轮 30 s 窗口无效，收紧到 10 s） | 立即 Stop |
| **S4** | 机床异常：报警、急停、伺服掉使能、异响；或操作员要求停 | 诊断方案 `:340` | 立即 Stop（**人工优先于脚本**） |
| **S5** | 监护人要求停止（任何理由，无需解释） | 人工 | 立即 Stop |
| **S6** | 状态/数据不自洽：非预期 `STOPPED`/`FAILED`；`count≠4`；任一点 `timestamp_ns` 停滞 ≥3×interval；`quality≠GOOD` | 诊断方案 `:341`；清单 `:153`、`:178` | 立即 Stop |

**统一流程**：Stop（`-TimeoutSec 120`）→ 五项复核（§5.3）→ 落盘（时刻/触发条款/`detail`/证据文件）→ 判 **NO-GO 或 INVALID** → **不自动重试**、**不自动进入下一轮**。"再试一次看看"属新的启停，计入 §5.4 的 5 次上限，且须人工批准。

## 8. 恢复与回退

### 8.1 回退基线 `rev17` 及其**证明过程**（不是"假定一致"）

**已捕获的三件套**（`target/validation/828d-r1/`）：`rev17-endpoint.raw.json`（658 B，端点快照：`connection` 8 字段、`desired_running=false`、`state=STOPPED`、`runtime.revision=16`、`runtime.points=4`）、`rev17-tasks.raw.json`（111,992 B，`GET /tasks` 原始响应 `{endpoint_id, revision:17, tasks:[…]}`，238 点）、`rev17-rollback-put.json`（111,925 B，**可直接作为 `PUT /tasks/{ep}` 请求体** `{"tasks":[…238 点…]}`）。

**证明步骤（本次编制时逐条实测，可复核）**：

1. **哈希比对**：`rev17-rollback-put.json` 的 SHA-256 = `10515C712FC9AF27D8B6FDF229764A88FE3E4E7DAC3B7FAC7F01637F21DFDF42`；既有 `target/validation/t2b-rollback-rev15.json` 的 SHA-256 **完全相同**（`Get-FileHash -Algorithm SHA256` 分别计算，两值逐字符一致）。
2. **字节级一致**：哈希相同 ⇒ 两文件**内容逐字节一致**（SHA-256 抗碰撞性是该结论的唯一依据；本次另以原始文本逐字节比较复核，结果相同）。
3. **逐点核对**：解析两份 PUT 体比较 `point_key` 序列 —— **238 / 238 完全一致**（含顺序）；`interval_ms` 均为 `1000`，`kind` 均为 `nck.points.v1`，任务 `id` 均为 `t1`，`mode` 均为 `poll`。据此**结论**：`t2b-rollback-rev15.json` **可以**作为 `rev17` 的回退目标。
4. **依据边界**：依据是**哈希相等 + 238 点逐点比对**，**不是**"rev15 与 rev17 应该一样"的假定；也**不覆盖**端点 `connection`（以 `rev17-endpoint.raw.json` 为准，本轮不改动）。若未来某轮的 `rev17` 备份与 rev15 哈希不同，则 rev15 **不能**再用，必须按当时的 `rev17` 备份恢复。

### 8.2 恢复操作步骤

1. 确认已停：`state=STOPPED`、`desired_running=false`（未停不许改任务，`core-api:2297-2305` 返回 `CONFLICT`）。
2. `PUT /api/v1/tasks/lx-nck-tags-190-connection`，body = **`rev17-rollback-put.json` 原文**（不手工重排 JSON、不 `ConvertTo-Json` 二次序列化）。
3. 读回 `GET /api/v1/tasks?endpoint=`，记录新 `revision`（预期 **19**：17 → 四点配置 18 → 恢复 19）。
4. 按 §8.3 对账（**内容**必须与 `rev17` 一致；**revision 号会递增，不可能回退**，`config-store:1432-1452`）。保持 **STOPPED**，**不自动 Start**；不删除、不改名任何备份文件。
5. PUT 失败：只允许**重试 1 次**（同 body、同端点），再失败即升级上报，**不得**循环重试或"手工重建 238 点"。

### 8.3 配置对账（恢复后逐项核对；含恢复后纪律）

| 项 | 恢复前（四点配置） | 恢复后（应为 rev17 内容） | 判据 |
|---|---|---|---|
| `GET /tasks` 的 `revision` | 18（预期） | 19（预期） | **递增即正常**；数值不符只记录，不阻断（版本号不可回退，诊断方案 `:153`） |
| 任务数 / id | 1 / `t1` | 1 / `t1` | 相等 |
| 点数 | 4 | **238** | 相等 |
| `interval_ms` / `mode` / `binding.kind` | 5000 / poll / `nck.points.v1` | **1000** / poll / `nck.points.v1` | 相等 |
| `point_key` 序列 | 4 条 | **238 条，与 rev17 逐点一致** | 逐点比对（顺序敏感） |
| 任务内容哈希 | 待填 | `tasks` 数组规范化后须与 rev17 的 `tasks` 数组同法哈希一致（**不得**直接比 `rev17-tasks.raw.json` 整文件哈希：其中 `revision=17`，恢复后为 19） | 相等；不等即恢复失败 |
| `connection` 8 字段 | 见 §3.2 | 同 §3.2（本轮从未修改） | 逐字段相等 |
| `state` / `desired_running` | RUNNING→STOPPED | **STOPPED / false** | 必须 |
| `runtime.revision` / `runtime.points` | 16 / 4 | **仍是 16 / 4** | **不作为判据**（上次运行视图，§2.1-④） |

**恢复后纪律**：端点保持 **STOPPED**，**不自动 Start**；下一轮必须重新申请、重过五门（§4）。备份三件套与本轮证据**全部保留**，不覆盖、不"整理"。恢复失败 ⇒ 判 **INVALID**（本轮数据不得用于任何结论），按 §10 上报。

## 9. 验收判据

### 9.1 通过条件（**全部**满足才算 PASS）

| # | 判据 | 证据 | 备注 |
|---|---|---|---|
| 1 | 四点 **4/4 `quality=GOOD`** 且 `value_origin=CURRENT`（不是 `LAST_KNOWN`/`PLACEHOLDER`），`early` 与 `prestop` **两次**都成立 | 两次 `points-latest.raw.json` | 只 1 次成立 ⇒ 不足以判 PASS；清单 `:153` |
| 2 | `timestamp_ns` 落在 `[T0, Stop 发出时刻]` 内，且相邻批次推进 ≈ `n × 5000 ms` | 两次快照的 `timestamp_ns` | T2b 实测 25.001 s = 5×5000 ms；按"轮"判，不按点判 |
| 3 | 抓取时刻与 `timestamp_ns` 差 ≤ 1×interval（T2b `early` 4.80 s、`prestop` 0.50 s） | `when.json` + 快照 | 超过即数据不新鲜 |
| 4 | `count=4`，无第 5 点、无多余条目 | 快照 | 防"悄悄扩点" |
| 5 | 全程未触发 §7 任一条；`state` 除首末外为 `RUNNING`，`epoch` 只在 Start 变化 1 次 | `state*.json`、`run-log.md` | — |
| 6 | Stop "停干净"五项齐（§5.3） | `state`、进程表、`Get-NetTCPConnection` | 缺一 ⇒ INVALID |
| 7 | 证据齐全（§6 全部条目）+ SHA 清单填全 + HMI 对照在案 | §6.1 目录 | 缺一 ⇒ INVALID |
| 8 | 恢复完成且 §8.3 对账一致，端点保持 STOPPED | 读回快照 | 未恢复不得裁决 |

**判据不是"等于历史值"**：T2b 的 `_N_MPF0`、`5`、`1`、`0.00` 只说明"量级/形态合理"。程序名、状态、参数、位置都会随机床状态变化，**禁止**把"必须等于 `_N_MPF0`/`5`/`1`/`0.0`"写成判据（清单 `:155`）。本轮判的是 **GOOD + 新鲜 + 窗口内推进**，不是数值相等。

### 9.2 **读到值 ≠ 业务可用**（签字前必读）

**本轮 PASS 只证明**：上述 4 个点位，在本次 ≤30 秒窗口内，可被本 Mesa 实例以 `5000 ms` / `max_read_items=1` 只读方式读到质量 GOOD、时间戳新鲜的值。它**不证明**（不得据此推导）：① 其余 **234** 点可用（本轮一个都没读）；② `238 点 / interval_ms=1000` 配置可用（未启用，且该配置历史上伴随 80 BAD，清单 `:147`）；③ `axis=1/3..8` 可用（本轮只有 `axis=2`，清单 `:185`）；④ 值的**业务语义/单位**正确（`5` 是什么状态、`1` 是什么参数、位置是否 mm，均**待确认**）；⑤ 一期 P0 清单通过（10 个 P0 族里 7 个仍只有历史 GOOD，清单 `:110`）；⑥ 任何**安全判断**能力（报警/急停/驱动使能不在采集范围，清单 `:193`）；⑦ 长期稳定性/性能（30 秒、4 点、6 批不构成性能证据）。

### 9.3 判定分级

| 判定 | 条件 |
|---|---|
| **PASS** | §9.1 八条全满足 |
| **PASS-WITH-NOTES** | §9.1 全满足，但有**不影响结论**的偏差（如 HMI 无对应显示、日志缺口已登记） |
| **FAIL** | 出现 §7 触发，或 §9.1 第 1–6 条任一不满足 |
| **INVALID** | 证据缺失（§6 不全）、Stop 未确认停干净、恢复未完成/对账不一致、或执行偏离冻结项 |

**INVALID ≠ FAIL**：INVALID 说明"这轮什么也没证明"，**不得**用于任何结论，也不得"挑好的数据留下"。

## 10. 裁决记录

### 10.1 裁决

- [ ] **GO** —— 批准按 §5 执行（仅限 §3.5 冻结范围，单次）
- [ ] **NO-GO** —— 不执行（原因：______________________）
- [ ] **HOLD** —— 暂缓（待补：______________________；补齐后重新提交 §10）

裁决理由（必填）：______________________________________________

### 10.2 签字（四方齐全才生效）

| 角色 | 姓名 | 单位/部门 | 日期时间 | 签名 |
|---|---|---|---|---|
| 申请人 | 待填 | 待填 | 待填 | |
| 执行人 | 待填 | 待填 | 待填 | |
| 现场监护人 | 待填 | 待填 | 待填 | |
| 审批人 | 待填 | 待填 | 待填 | |

### 10.3 异常记录（无异常也要写"无"）

| 序号 | 时刻（本地） | 现象 | 证据文件 | 处置 | 触发条款 |
|---|---|---|---|---|---|
| 1 | 待填 | 待填 | 待填 | 待填 | S?/无 |

### 10.4 后续纪律

1. **禁止自动进入下一轮**：本轮结束即停止；下一轮重新编制申请（复制本模板）、重跑 §4 五门、重新裁决。
2. 本轮 **GO 不覆盖**：238 点、`1000 ms`、扩点、其它轴、性能评估、语义确认、生产接入；**PASS 也不外推**——不构成"828D 已验收"、不构成"NCK 驱动可生产"、不翻转 `GATE.md:91-92` 的任何 Gate。
3. 归档：本文件 + §6.1 证据目录 + SHA 清单；结案后 90 天（诊断方案 `:477`），必要时转 `docs/evidence/`。
