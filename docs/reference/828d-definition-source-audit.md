# 828D 定义来源盘查（42 条，r1.json）

> 本文是 **Phase 0 子集**的只读盘点产物：只读仓库既有文件、只新增文档，**未改动**任何代码、
> 定义、测试或配置，未连接任何设备/服务。
> **阶段 A 更新**：三份 07/2023 手册 PDF 已在位并经 pypdf 实测，逐条交叉引用结果见 §6 与
> `828d-manual-cross-reference.json` / `.md`；本文件 §1–§4 的统计数字不变，`r1.json` 仍未改动。

## 0. 基线对账（前置结论，已在 HEAD 复核）

| 项 | 值 | 复核方式 |
|---|---|---|
| 仓库 HEAD | `047e2064bd233eafc508ca9fe5329e93a96ee6e2` | `git rev-parse HEAD` |
| `drivers/sinumerik-nck/definitions/828d/r1.json` Blob SHA-1 | `3c0abe67cbcb64b525cddec1584e6e1747e0f062` | `git ls-tree HEAD <path>` |
| 工作区文件对象 SHA-1 | `3c0abe67cbcb64b525cddec1584e6e1747e0f062` | `git hash-object <path>` |
| 远端 `origin` HEAD | `047e2064bd233eafc508ca9fe5329e93a96ee6e2` | `git ls-remote origin HEAD` |
| 工作区状态 | clean（无输出） | `git status --porcelain` |

**结论**：`git ls-tree`（HEAD 树内 blob）、`git hash-object`（工作区实文件）、
`git ls-remote origin HEAD`（远端 HEAD 提交号相同 ⇒ 同一 commit ⇒ 同一 tree）三者一致，
**本地文件即 HEAD 即远端**，后续统计基于的正是这份 42 条定义。

## 1. 统计口径与纪律

- 唯一数据源：`drivers/sinumerik-nck/definitions/828d/r1.json`（`format_version=1`，`family=828d`，
  `revision=1`）。
- **全部数字由 Python 解析 JSON 得出**，脚本全文见附录 A，可原样复现。
  **未使用** PowerShell `Group-Object` / `Select-Object -Unique`（它们对嵌套 JSON 会把
  不存在的属性归成 `$null` 组、把对象列表塌成 1 条——上一轮错误的直接原因）。
- 逐条定义以 `variable_id` 为键；`source` 为数组，单条定义可有 1..n 个来源条目。

## 2. 汇总统计（与任务给出基准逐一核对）

| 指标 | 值 | 与基准 |
|---|---|---|
| 定义数量 | 42 | ✅ 一致 |
| `source` 条目总数 | 90 | ✅ 一致 |
| `frozen_vector` | 42 | ✅ 一致 |
| `manual` | 35 | ✅ 一致 |
| `measurement` | 7 | ✅ 一致 |
| `identity_probe` | 6 | ✅ 一致 |
| 是否有独立 `verified` 字段 | **否（0 条）** | ✅ 一致 |
| `source` 字段类型 | `list` × 42（42 条全部为 list，无对象/字符串形态） | ✅ 一致 |

### 2.1 按 `kind` 计数

| kind | 条目数 | 占比 |
|---|---|---|
| `frozen_vector` | 42 | 46.7% |
| `identity_probe` | 6 | 6.7% |
| `manual` | 35 | 38.9% |
| `measurement` | 7 | 7.8% |

合计 90 条。注意每个 kind 的**条目数 ≠ 覆盖的定义数**：
`frozen_vector` 42 条 = 42 条定义各 1 条（全覆盖）；`manual` 35 条 = 35 条定义各 1 条，
**另有 7 条定义没有 manual**（`drive.object.name` 与 6 条 `scope=identity` 定义）；
`measurement` 7 条覆盖 7 条定义；`identity_probe` 6 条覆盖 6 条定义（见 §2.6）。

### 2.2 按 manual reference 计数

| manual reference（原文照抄） | 条目数 |
|---|---|
| `SINAMICS S120/S150 参数手册 11/2023` | 18 |
| `Siemens 828D NC variables 03/2025 A5E53370586B AC` | 16 |
| `SINAMICS S120/S150 参数手册 11/2023（p.71 无效值标记）` | 1 |

合计 35 条。三份 reference 分别是两份**不同版本**的西门子文档，其中
`SINAMICS S120/S150 参数手册 11/2023` 与 `SINAMICS S120/S150 参数手册 11/2023（p.71 无效值标记）`
是同一份文档的两种写法（后者把"p.71 无效值标记"写进了 reference 字符串，
见 `drive.motor.temperature`）——**页码只以自由文本形式存在，不是结构化字段**。

> ⚠️ 版本提示：现有定义的 manual 来源是 **828D NC variables 03/2025** 与
> **S120/S150 11/2023**；用户提供的三份 PDF 是 **07/2023**，两者**版本不等同**。
> 本盘点**不**把 07/2023 追认为现有定义的来源，详见
> `docs/reference/sinumerik-828d-manuals.json`。

### 2.3 按 `semantics_status` × 是否有 `frozen_vector` 交叉表

| semantics_status | frozen_vector 情况 | 定义数 |
|---|---|---|
| candidate | has_frozen_vector | 2 |
| parameter_identity | has_frozen_vector | 16 |
| verified | has_frozen_vector | 24 |

**42 条定义全部带 `frozen_vector`**（`source` 数组里各 1 条，reference 恒为
`drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json`）。该 fixture 自述来源为
`catalog/828d.json @ main@78e23ec（PR-B 删除前冻结）`，共 62 个 entries
（`source`/`note`/`entries` 三键），其 `note` 明确写的是"每条候选定义的**期望线缆字节**；
原生寻址路径必须逐条复现"——因此 `frozen_vector` 证明的是**地址/字节相对旧目录未被改动**
（回归基线），**不是**设备语义证据，也不构成章节级可追溯性。

### 2.4 按 `scope`

| scope | 定义数 |
|---|---|
| `collect` | 36 |
| `identity` | 6 |

### 2.5 按 `semantics_status`

| semantics_status | 定义数 |
|---|---|
| `candidate` | 2 |
| `parameter_identity` | 16 |
| `verified` | 24 |

### 2.6 每条定义的 `source` 条目数分布

| source 条数 | 定义数 |
|---|---|
| 2 | 36 |
| 3 | 6 |

（36 条定义 2 个来源 + 6 条定义 3 个来源 = 72 + 18 = 90 条，
与总数吻合。3 个来源的 6 条正是 `scope=identity` 的 6 条身份定义。）

### 2.7 可追溯性缺口（**关键**）

`source` 条目可用的键只有：`kind`（90 次）、`reference`（90 次）、`note`（49 次）。<br>
**没有** `document_id` / `section` / `page` 这三个键中的任何一个（0 次出现）。

| 缺什么 | 定义数 / 42 |
|---|---|
| 缺结构化 `document_id` | 42 |
| 缺结构化 `section`（章节） | 42 |
| 缺结构化 `page`（页码） | 42 |

即 **42/42 条定义都没有可机器复核的章节/页码**，只有一条自由文本 `reference`
（1 条例外：`drive.motor.temperature` 的 reference 文本内含 `p.71`，但仍是自由文本，
无法用于自动定位与校验）。

### 2.8 **24 条 `verified` 中只有 manual 来源的情况**（任务指定必答项）

先明确"只有 manual 来源"的两种读法，两种结果都给出：

| 口径 | 定义 | 条数 |
|---|---|---|
| 严格：manual 是**唯一**来源条目（既无 measurement/identity_probe，也无 frozen_vector） | **空集（无任何一条）** | **0** |
| 实务：有 manual 背书，但**无任何实测证据**（measurement / identity_probe 均为 0），仅有 frozen_vector 回归基线 | 见下表 | **17** |

**结论：24 条 `verified` 中，没有一条是"只挂 manual 一个来源"的（0 条）——
但其中 17 条（24 − 7 有 measurement 的）除 manual 之外，
唯一证据是 `frozen_vector` 回归基线，既无 measurement 抓包，也无 identity_probe 探针。**

这 17 条清单：

| # | variable_id | semantics_status | 实际证据 |
|---|---|---|---|
| 01 | `channel.program.status` | verified | manual + frozen_vector |
| 02 | `mode_group.op_mode` | verified | manual + frozen_vector |
| 03 | `channel.program.name.main` | verified | manual + frozen_vector |
| 04 | `channel.program.name.active` | verified | manual + frozen_vector |
| 05 | `channel.program.line_number` | verified | manual + frozen_vector |
| 06 | `channel.program.block` | verified | manual + frozen_vector |
| 07 | `axis.position.commanded_mcs` | verified | manual + frozen_vector |
| 08 | `axis.position.actual_bcs` | verified | manual + frozen_vector |
| 09 | `channel.feed.actual` | verified | manual + frozen_vector |
| 10 | `channel.feed.commanded` | verified | manual + frozen_vector |
| 11 | `channel.feed.override` | verified | manual + frozen_vector |
| 12 | `spindle.speed.actual` | verified | manual + frozen_vector |
| 13 | `spindle.speed.commanded` | verified | manual + frozen_vector |
| 14 | `spindle.speed.override` | verified | manual + frozen_vector |
| 15 | `channel.tool.number_active` | verified | manual + frozen_vector |
| 16 | `channel.r_parameter` | verified | manual + frozen_vector |
| 17 | `drive.motor.temperature` | verified | manual + frozen_vector |

另 7 条 `verified` 有 `measurement`（`drive.object.name` 1 条 + 6 条身份定义共用
`docs/evidence/nck-live-identity-probe.txt`），其中 6 条另有 `identity_probe`。

## 3. 42 条逐条明细（总表）

| # | variable_id | rev | scope | semantics_status | source 条数 | 证据类型 | module | 章节/页码 |
|---|---|---|---|---|---|---|---|---|
| 1 | `channel.program.status` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 127 | 缺（无 document_id / section / page） |
| 2 | `mode_group.op_mode` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 127 | 缺（无 document_id / section / page） |
| 3 | `channel.program.name.main` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 122 | 缺（无 document_id / section / page） |
| 4 | `channel.program.name.active` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 125 | 缺（无 document_id / section / page） |
| 5 | `channel.program.line_number` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 125 | 缺（无 document_id / section / page） |
| 6 | `channel.program.block` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 125 | 缺（无 document_id / section / page） |
| 7 | `axis.position.commanded_mcs` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 115 | 缺（无 document_id / section / page） |
| 8 | `axis.position.actual_bcs` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 112 | 缺（无 document_id / section / page） |
| 9 | `channel.feed.actual` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 127 | 缺（无 document_id / section / page） |
| 10 | `channel.feed.commanded` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 127 | 缺（无 document_id / section / page） |
| 11 | `channel.feed.override` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 127 | 缺（无 document_id / section / page） |
| 12 | `spindle.speed.actual` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 114 | 缺（无 document_id / section / page） |
| 13 | `spindle.speed.commanded` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 114 | 缺（无 document_id / section / page） |
| 14 | `spindle.speed.override` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 114 | 缺（无 document_id / section / page） |
| 15 | `channel.tool.number_active` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 127 | 缺（无 document_id / section / page） |
| 16 | `channel.r_parameter` | 1 | collect | verified | 2 | manual + frozen_vector | fixed 21 | 缺（无 document_id / section / page） |
| 17 | `drive.parameter.r0021` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 18 | `drive.parameter.r0027` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 19 | `drive.motor.temperature` | 1 | collect | verified | 2 | manual + frozen_vector | drive_entry_module | 仅 reference 字符串内含 `p.71`（非结构化字段） |
| 20 | `drive.parameter.r0033` | 1 | collect | candidate | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 21 | `drive.parameter.r0031` | 1 | collect | candidate | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 22 | `drive.parameter.r0062` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 23 | `drive.parameter.r0063` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 24 | `drive.parameter.r0064` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 25 | `drive.parameter.r0077` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 26 | `drive.parameter.r0078` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 27 | `drive.parameter.r0079` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 28 | `drive.parameter.r0080` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 29 | `drive.parameter.r0081` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 30 | `drive.parameter.r0082` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 31 | `drive.parameter.r0103` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 32 | `drive.parameter.p0105` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 33 | `drive.parameter.r0106` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 34 | `drive.parameter.r0107` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 35 | `drive.parameter.r0108` | 1 | collect | parameter_identity | 2 | manual + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 36 | `drive.object.name` | 1 | collect | verified | 2 | measurement + frozen_vector | drive_entry_module | 缺（无 document_id / section / page） |
| 37 | `axis.name` | 1 | identity | verified | 3 | measurement + identity_probe + frozen_vector | fixed 26 | 缺（无 document_id / section / page） |
| 38 | `axis.ctrlout.module_number` | 1 | identity | verified | 3 | measurement + identity_probe + frozen_vector | fixed 26 | 缺（无 document_id / section / page） |
| 39 | `axis.encoder.module_number` | 1 | identity | verified | 3 | measurement + identity_probe + frozen_vector | fixed 26 | 缺（无 document_id / section / page） |
| 40 | `axis.ctrlout.type` | 1 | identity | verified | 3 | measurement + identity_probe + frozen_vector | fixed 26 | 缺（无 document_id / section / page） |
| 41 | `channel.machine_axis_assignment` | 1 | identity | verified | 3 | measurement + identity_probe + frozen_vector | fixed 26 | 缺（无 document_id / section / page） |
| 42 | `axis.spindle_assignment` | 1 | identity | verified | 3 | measurement + identity_probe + frozen_vector | fixed 26 | 缺（无 document_id / section / page） |

## 4. 逐条 `source` 条目展开

#### 01. `channel.program.status`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `通道程序状态` / `Channel program status`
- module（addressing）: `fixed 127`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/S/progStatus` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 02. `mode_group.op_mode`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `方式组运行方式` / `Mode group operating mode`
- module（addressing）: `fixed 127`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `B/S/opMode` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 03. `channel.program.name.main`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `主程序名` / `Main program name`
- module（addressing）: `fixed 122`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SPARPP/progName` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 04. `channel.program.name.active`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `当前活动程序名` / `Active program name`
- module（addressing）: `fixed 125`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SPARP/progName` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 05. `channel.program.line_number`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `当前程序行号` / `Active program line number`
- module（addressing）: `fixed 125`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SPARP/actLineNumber` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 06. `channel.program.block`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `当前执行程序块` / `Active program block`
- module（addressing）: `fixed 125`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SPARP/actBlock` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 07. `axis.position.commanded_mcs`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `轴指令位置（机床坐标系）` / `Axis commanded position (MCS)`
- module（addressing）: `fixed 115`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SGA/actToolBasePos` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 08. `axis.position.actual_bcs`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `轴实际位置（基本坐标系）` / `Axis actual position (BCS)`
- module（addressing）: `fixed 112`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SEGA/actProgPosBKS` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 09. `channel.feed.actual`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `通道实际进给率（IPO）` / `Channel actual feedrate (IPO)`
- module（addressing）: `fixed 127`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/S/actFeedRateIpo` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 10. `channel.feed.commanded`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `通道指令进给率（IPO）` / `Channel commanded feedrate (IPO)`
- module（addressing）: `fixed 127`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/S/cmdFeedRateIpo` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 11. `channel.feed.override`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `进给倍率（IPO）` / `Feedrate override (IPO)`
- module（addressing）: `fixed 127`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/S/feedRateIpoOvr` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 12. `spindle.speed.actual`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `主轴实际转速` / `Spindle actual speed`
- module（addressing）: `fixed 114`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SSP/actSpeed` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 13. `spindle.speed.commanded`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `主轴指令转速` / `Spindle commanded speed`
- module（addressing）: `fixed 114`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SSP/cmdSpeed` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 14. `spindle.speed.override`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `主轴倍率` / `Spindle speed override`
- module（addressing）: `fixed 114`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/SSP/speedOvr` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 15. `channel.tool.number_active`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `当前有效刀具号` / `Active tool number`
- module（addressing）: `fixed 127`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/S/actTNumber` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 16. `channel.r_parameter`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `通道 R 参数` / `Channel R parameter`
- module（addressing）: `fixed 21`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `Siemens 828D NC variables 03/2025 A5E53370586B AC` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/RP/rpa（index=0 → line=1）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 17. `drive.parameter.r0021`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0021（仅参数身份，非工程语义）` / `Drive parameter r0021 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0021 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 18. `drive.parameter.r0027`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0027（仅参数身份，非工程语义）` / `Drive parameter r0027 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0027 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 19. `drive.motor.temperature`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `该驱动对象的电机温度（不表示某个机床轴）` / `Motor temperature of this drive object (not a machine axis)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023（p.71 无效值标记）` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0035 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **仅 reference 字符串内含 `p.71`（非结构化字段）**

#### 20. `drive.parameter.r0033`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `candidate`
- 标签: `驱动参数 r0033（候选语义：轴负载；未确认）` / `Drive parameter r0033 (candidate: axis load)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0033 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 21. `drive.parameter.r0031`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `candidate`
- 标签: `驱动参数 r0031（候选语义：滤波转矩实际值；访问能力 HOLD）` / `Drive parameter r0031 (candidate; access capability HOLD)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0031 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 22. `drive.parameter.r0062`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0062（仅参数身份，非工程语义）` / `Drive parameter r0062 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0062 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 23. `drive.parameter.r0063`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0063（仅参数身份，非工程语义）` / `Drive parameter r0063 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0063 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 24. `drive.parameter.r0064`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0064（仅参数身份，非工程语义）` / `Drive parameter r0064 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0064 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 25. `drive.parameter.r0077`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0077（仅参数身份，非工程语义）` / `Drive parameter r0077 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0077 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 26. `drive.parameter.r0078`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0078（仅参数身份，非工程语义）` / `Drive parameter r0078 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0078 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 27. `drive.parameter.r0079`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0079（仅参数身份，非工程语义）` / `Drive parameter r0079 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0079 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 28. `drive.parameter.r0080`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0080（仅参数身份，非工程语义）` / `Drive parameter r0080 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0080 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 29. `drive.parameter.r0081`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0081（仅参数身份，非工程语义）` / `Drive parameter r0081 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0081 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 30. `drive.parameter.r0082`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0082（仅参数身份，非工程语义）` / `Drive parameter r0082 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0082 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 31. `drive.parameter.r0103`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0103（仅参数身份，非工程语义）` / `Drive parameter r0103 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0103 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 32. `drive.parameter.p0105`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 p0105（仅参数身份，非工程语义；只读）` / `Drive parameter p0105 (parameter identity only, read-only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/p0105 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 33. `drive.parameter.r0106`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0106（仅参数身份，非工程语义）` / `Drive parameter r0106 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0106 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 34. `drive.parameter.r0107`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0107（仅参数身份，非工程语义）` / `Drive parameter r0107 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0107 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 35. `drive.parameter.r0108`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `parameter_identity`
- 标签: `驱动参数 r0108（仅参数身份，非工程语义）` / `Drive parameter r0108 (parameter identity only)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `manual` | `SINAMICS S120/S150 参数手册 11/2023` | — | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/r0108 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=1 / measurement=0 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 36. `drive.object.name`

- revision: `1`｜family: `828d`｜scope: `collect`｜semantics_status: `verified`
- 标签: `该驱动对象名（25×U16BE）` / `Drive object name (25 x U16BE)`
- module（addressing）: `drive_entry_module`
- source 条目（共 2 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `measurement` | `docs/evidence/nck-drive-object-readback.json` | `module26/module130 两条入口的 p0199 抓包` | — | — | — |
  | 2 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `V/M/p0199 × 2（nc_variable / drive_object）` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=0 / measurement=1 / identity_probe=0
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 37. `axis.name`

- revision: `1`｜family: `828d`｜scope: `identity`｜semantics_status: `verified`
- 标签: `机床轴名（身份探针）` / `Machine axis name (identity probe)`
- module（addressing）: `fixed 26`
- source 条目（共 3 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `identity_probe` | `drivers/sinumerik-nck/src/identity_spec.rs` | `AXIS_NAME` | — | — | — |
  | 2 | `measurement` | `docs/evidence/nck-live-identity-probe.txt` | — | — | — | — |
  | 3 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `N/M/MDSA_AXCONF_MACHAX_NAME_TAB` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=0 / measurement=1 / identity_probe=1
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 38. `axis.ctrlout.module_number`

- revision: `1`｜family: `828d`｜scope: `identity`｜semantics_status: `verified`
- 标签: `轴驱动模块号（身份探针）` / `Axis control output module number (identity probe)`
- module（addressing）: `fixed 26`
- source 条目（共 3 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `identity_probe` | `drivers/sinumerik-nck/src/identity_spec.rs` | `AXIS_CTRLOUT_MODULE_NR` | — | — | — |
  | 2 | `measurement` | `docs/evidence/nck-live-identity-probe.txt` | — | — | — | — |
  | 3 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `A/M/MDCA_CTRLOUT_MODULE_NR` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=0 / measurement=1 / identity_probe=1
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 39. `axis.encoder.module_number`

- revision: `1`｜family: `828d`｜scope: `identity`｜semantics_status: `verified`
- 标签: `轴编码器模块号（身份探针）` / `Axis encoder module number (identity probe)`
- module（addressing）: `fixed 26`
- source 条目（共 3 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `identity_probe` | `drivers/sinumerik-nck/src/identity_spec.rs` | `AXIS_ENC_MODULE_NR` | — | — | — |
  | 2 | `measurement` | `docs/evidence/nck-live-identity-probe.txt` | — | — | — | — |
  | 3 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `A/M/MDCA_ENC_MODULE_NR` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=0 / measurement=1 / identity_probe=1
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 40. `axis.ctrlout.type`

- revision: `1`｜family: `828d`｜scope: `identity`｜semantics_status: `verified`
- 标签: `轴驱动输出类型（身份探针；0 = 无驱动输出）` / `Axis control output type (identity probe)`
- module（addressing）: `fixed 26`
- source 条目（共 3 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `identity_probe` | `drivers/sinumerik-nck/src/identity_spec.rs` | `AXIS_CTRLOUT_TYPE` | — | — | — |
  | 2 | `measurement` | `docs/evidence/nck-live-identity-probe.txt` | — | — | — | — |
  | 3 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `A/M/MDCA_CTRLOUT_TYPE` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=0 / measurement=1 / identity_probe=1
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 41. `channel.machine_axis_assignment`

- revision: `1`｜family: `828d`｜scope: `identity`｜semantics_status: `verified`
- 标签: `通道位置→机床轴号（身份探针）` / `Channel position to machine axis number (identity probe)`
- module（addressing）: `fixed 26`
- source 条目（共 3 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `identity_probe` | `drivers/sinumerik-nck/src/identity_spec.rs` | `CHANNEL_MACHAX_USED` | — | — | — |
  | 2 | `measurement` | `docs/evidence/nck-live-identity-probe.txt` | — | — | — | — |
  | 3 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `C/M/AXCONF_MACHAX_USED` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=0 / measurement=1 / identity_probe=1
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

#### 42. `axis.spindle_assignment`

- revision: `1`｜family: `828d`｜scope: `identity`｜semantics_status: `verified`
- 标签: `轴的主轴归属（身份探针；非 0 = 该轴为第 n 主轴）` / `Axis spindle assignment (identity probe)`
- module（addressing）: `fixed 26`
- source 条目（共 3 条，逐条列出）：

  | # | kind | reference | note | document_id | section | page |
  |---|---|---|---|---|---|---|
  | 1 | `identity_probe` | `drivers/sinumerik-nck/src/identity_spec.rs` | `AXIS_SPIND_ASSIGN` | — | — | — |
  | 2 | `measurement` | `docs/evidence/nck-live-identity-probe.txt` | — | — | — | — |
  | 3 | `frozen_vector` | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | `A/M/SPIND_ASSIGN_TO_MACHAX` | — | — | — |

- 证据类型标记: frozen_vector=1 / manual=0 / measurement=1 / identity_probe=1
- 是否缺可追溯章节/页码: **缺（无 document_id / section / page）**

## 5. 未找到 / 待核实

- **未找到**：任何定义上的 `document_id` / `section` / `page` 结构化字段（42/42 缺失）。
- **已解决（原结论已过期，此处更正）**：三份 07/2023 PDF 已在位并经 pypdf 实测（文档号、版本、页数、SHA-256 全部核对），
  `docs/reference/sinumerik-828d-manuals.json` 的 `file_present` 均为 `true`；本文件 §6 记录了逐条交叉引用结果。
  注意：07/2023 与定义所引的 03/2025 / 11/2023 **版本不等同**，本盘点仍不把任何 07/2023 页码写成定义来源。
- **待核实**：`frozen_vector` 唯一 reference `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` 是**回归基线**，
  其 62 条 entries 的 `wire_hex` 与真机是否一致须真机复核（该 fixture 自身不声明硬件验证）。
- **已核对（阶段 A）**：`drive.motor.temperature` 的 `p.71` 是 **11/2023 版**页码；07/2023 版《参数描述》中
  `r0035 CO: 电机温度` 的条目实测位于 **p50**，且 p50 明确写出「当 r0035 等于 -200.0 °C 时」的无效值分支，
  与 `invalid_rule = {"value": -200.0, "tolerance": 0.001}` 一致（见 `828d-manual-cross-reference.md` §3.2）。
  两版分页不同这一事实本身说明 07/2023 不能充当 11/2023 的替代来源。
- **待核实**：`measurement` 来源 `docs/evidence/nck-drive-object-readback.json` 自述
  `environment = "Run MyVirtual Machine 1.6 / CNC-SW 6.25 SP2; not real 828D"`、
  `hardware_validated = false`，因此它**不是真机证据**。
- **待核实（来源与文件内容不符）**：`axis.encoder.module_number` 的 `measurement` 来源写的是
  `docs/evidence/nck-live-identity-probe.txt`，但该文件小节依次为
  机床轴名 / 通道1 位置→机床轴号 / 轴驱动号 / 轴输出类型 / 主轴归属 / 驱动对象名 m26 /
  驱动对象名 m130 / 跨路径 r0035 / r0103 / p0105 / r0106 / r0107 / r0108（第 7、16、25、34、
  43、52、61、70、79、88、97、106、115 行），**没有编码器驱动号一节** ⇒
  该条 `measurement` 来源**在所引文件中未找到对应记录**（详见
  `docs/reference/828d-identity-probe-evidence.md` §5、§7）。

## 6. 阶段 A 增补：07/2023 交叉引用、两条独立缺口与三条禁令

本节由 Phase 0 阶段 A 增补，**不修改本文件 §1–§4 的任何统计数字**（42 / 90 / 35 / 7 / 6、24-16-2、36-6 均不变），
也**不修改 `r1.json`**（blob SHA-1 仍为 `3c0abe67cbcb64b525cddec1584e6e1747e0f062`）。

### 6.1 35 条 manual 来源 × 07/2023 手册的比对结果

逐条明细见 `docs/reference/828d-manual-cross-reference.json` / `.md`（五类结论 + 实测页码 + 抽取原文依据）。分布：

| 结论 | 条数 | 含义 |
|---|---|---|
| `exact_match` | 0 | 找到且语义一致且版本一致（三份 PDF 均为 07/2023，早于所引来源，故为 0） |
| `matching_older_version` | 19 | 找到同名条目、语义/类型不冲突，但属较早版本 |
| `version_difference` | 1 | 找到条目但表述存在实质差异 |
| `not_found` | 13 | 三份 PDF 中完全没有该标识符 |
| `pending_review` | 2 | 标识符只在正文被提及、无条目定义 |

关键结论：**13 条 `not_found` 全部集中在 NC 变量（OPI 变量）组**（16 条中的 13 条）。
它们所引的是“828D NC variables（OPI 变量清单）”类文档，而本轮到手的三份是“系统变量 / 机床数据 / 参数描述”，
这是**文档选型问题**，不是定义错误；补齐需取得 03/2025 版 NC 变量文档（A5E53370586B AC）。
19 条 S120 参数类定义全部在 07/2023《参数描述》的“4 SINAMICS 参数”章找到条目，且数据类型（FloatingPoint32 /
Unsigned16 / Integer16 / Unsigned32）与定义的 `decode` 逐条一致。

### 6.2 7 条无 manual 来源定义的处理（缺口未被抹平）

- 7 条（`drive.object.name` + 6 条 `scope=identity`）在 07/2023 手册中**都**找到了对应条目
  （6 条机床数据：MD10000 p31、MD20070 p185、MD30110/MD30130 p364、MD30220 p365、MD35000 p451；
  1 条驱动参数：p0199 p136）。
- 但按纪律**只新增独立交叉引用**，`r1.json` 的 `source` 一个字节未动 ⇒
  **“无 manual 来源的定义”仍为 7 条**，缺口仍然成立，且每条都被显式标记为 `additive_only`。
- 其中 `axis.ctrlout.type` 找到的条目与定义表述存在差异（手册 p364 写“0 = 仿真（无硬件要求）”，
  定义 label 写“0 = 无驱动输出”），记为 `version_difference`，待人工确认是否同义。

### 6.3 独立缺口 A：`axis.encoder.module_number` 的证据链断在运行时侧

| 事实 | 位置 |
|---|---|
| `measurement` 来源指向 `docs/evidence/nck-live-identity-probe.txt`，但该文件 13 个小节里**没有编码器驱动号一节** | `docs/evidence/nck-live-identity-probe.txt:7,16,25,34,43,52,61,70,79,88,97,106,115`（共 128 行） |
| `AXIS_ENC_MODULE_NR` **未进入真实探测执行路径**：测试的 use 清单不含该常量，测试体也未调用 | `drivers/sinumerik-nck/tests/live_identity_probe.rs:16-20` |
| 身份浏览执行路径的导入清单同样不含该常量 | `drivers/sinumerik-nck/src/identity.rs:6-9` |
| 常量存在并被 catalog 测试核对（存在 ≠ 被执行） | `drivers/sinumerik-nck/src/identity_spec.rs:135`、`tests/definition_catalog.rs:405` |

**即使 07/2023 PDF 已找到对应机床数据定义（MD30220，md p365，说明延续至 p366），也只能补文档依据，
不能据此标记真机已验证。** 该条必须二选一：**修复探针执行路径**（把 `AXIS_ENC_MODULE_NR` 纳入真实探测、重跑并留存证据），
或**撤销这条不受证据支持的 `measurement` 声明**。

### 6.4 独立缺口 B：module 26 与 module 130 的实例编号（三条禁令）

同一台 828D 实测（`docs/evidence/nck-live-identity-probe.txt:53-69`）：`V/1` 的 m26=`SERVO_3.3:4` 而 m130=`SERVO_3.3:3`（差 1），
`V/2`–`V/4` 亦不同（`V/4` 甚至不是 ±1），但 `V/5`、`V/7`、`V/8` 恰好同值；`V/6` 在 m26 上被拒（`S7_JOB_0x8201`）。

1. **不允许**固定 `±1` 规则转换实例编号；
2. **不允许**因某些编号恰好相同就认定两个入口等价；
3. **每个映射必须绑定**实际入口、对象类型（`nc_variable` / `drive_object`）、索引与验证证据出处。

> 同样适用于 MD30220 的“驱动器编号/测量回路编号”语义：写证据时必须注明“哪个入口 + 哪个编号”。

### 6.5 四类证据仍然互不替代

`manual` 页码只证明“手册里怎么定义”，不证明协议可访问或真机可读；`frozen_vector` 只证明回归一致性；
`measurement` 只证明“当时那次读数”，且**须先核实原始证据是否真实存在、是否覆盖该定义**（本轮已有反例，见 §6.3）；
`identity_probe` 只证明“探针返回过什么”，**须先确认实际调用路径与采样范围**（本轮已有反例，见 §6.3）。
**新增 07/2023 文档依据不等于任何一类运行时证据升级。**

## 附录 A：统计脚本全文（可原样复现）

```python
# -*- coding: utf-8 -*-
"""Phase 0 交付1：r1.json 42 条定义来源盘查（只读）

用法: python audit_sources.py <r1.json 路径> <输出 json 路径>
纪律: 全部用 Python 解析 JSON；不使用 PowerShell Group-Object / Select-Object -Unique。
"""
import json
import sys
from collections import Counter, OrderedDict

r1_path = sys.argv[1]
out_path = sys.argv[2]

with open(r1_path, encoding="utf-8") as f:
    doc = json.load(f)

defs = doc["definitions"]

KINDS = ["frozen_vector", "manual", "measurement", "identity_probe"]

rows = []
kind_counter = Counter()
manual_ref_counter = Counter()
status_counter = Counter()
scope_counter = Counter()
cross = Counter()          # (semantics_status, has_frozen_vector)
independent_verified_field = 0
source_type_counter = Counter()
multi_source_count = 0
no_source_count = 0
defs_missing_doc_id = 0
defs_missing_section = 0
defs_missing_page = 0
extra_source_keys = Counter()

for d in defs:
    vid = d.get("variable_id")
    srcs = d.get("source") or []
    if not isinstance(srcs, list):
        source_type_counter["NOT_A_LIST:%s" % type(srcs).__name__] += 1
        srcs = []
    else:
        source_type_counter["list"] += 1

    if len(srcs) == 0:
        no_source_count += 1
    if len(srcs) > 1:
        multi_source_count += 1

    kinds_present = OrderedDict((k, 0) for k in KINDS)
    src_rows = []
    has_doc_id = False
    has_section = False
    has_page = False
    for s in srcs:
        kind = s.get("kind")
        kind_counter[kind] += 1
        if kind in kinds_present:
            kinds_present[kind] += 1
        if kind == "manual":
            manual_ref_counter[s.get("reference")] += 1
        for k in s.keys():
            extra_source_keys[k] += 1
        if s.get("document_id"):
            has_doc_id = True
        if s.get("section"):
            has_section = True
        if s.get("page") is not None:
            has_page = True
        src_rows.append(OrderedDict(
            (k, s.get(k)) for k in ["kind", "reference", "note", "document_id", "section", "page"]
        ))

    if not has_doc_id:
        defs_missing_doc_id += 1
    if not has_section:
        defs_missing_section += 1
    if not has_page:
        defs_missing_page += 1

    st = d.get("semantics_status")
    status_counter[st] += 1
    scope_counter[d.get("scope")] += 1
    has_fv = kinds_present["frozen_vector"] > 0
    cross[(st, "has_frozen_vector" if has_fv else "no_frozen_vector")] += 1

    if "verified" in d and d.get("verified") is not None:
        independent_verified_field += 1

    rows.append(OrderedDict([
        ("variable_id", vid),
        ("revision", d.get("revision")),
        ("family", d.get("family")),
        ("scope", d.get("scope")),
        ("semantics_status", st),
        ("label_zh", (d.get("label") or {}).get("zh")),
        ("label_en", (d.get("label") or {}).get("en")),
        ("source_count", len(srcs)),
        ("sources", src_rows),
        ("evidence", OrderedDict([
            ("frozen_vector", kinds_present["frozen_vector"]),
            ("manual", kinds_present["manual"]),
            ("measurement", kinds_present["measurement"]),
            ("identity_probe", kinds_present["identity_probe"]),
        ])),
        ("missing_traceability", OrderedDict([
            ("document_id", not has_doc_id),
            ("section", not has_section),
            ("page", not has_page),
        ])),
        ("addressing_module", (d.get("addressing") or {}).get("module")),
        ("golden_ref", d.get("golden_ref")),
    ]))

# 24 条 verified 中只有 manual 来源（无 measurement/identity_probe/frozen_vector）的条数
verified_only_manual = [
    r["variable_id"] for r in rows
    if r["semantics_status"] == "verified"
    and r["evidence"]["manual"] > 0
    and r["evidence"]["measurement"] == 0
    and r["evidence"]["identity_probe"] == 0
    and r["evidence"]["frozen_vector"] == 0
]
# 更宽松口径：verified 且有 manual、但无 measurement 且无 identity_probe（frozen_vector 可有可无）
verified_no_meas_no_probe = [
    r["variable_id"] for r in rows
    if r["semantics_status"] == "verified"
    and r["evidence"]["manual"] > 0
    and r["evidence"]["measurement"] == 0
    and r["evidence"]["identity_probe"] == 0
]
verified_ids = [r["variable_id"] for r in rows if r["semantics_status"] == "verified"]
verified_with_fv = [
    r["variable_id"] for r in rows
    if r["semantics_status"] == "verified" and r["evidence"]["frozen_vector"] > 0
]
verified_with_meas = [
    r["variable_id"] for r in rows
    if r["semantics_status"] == "verified" and r["evidence"]["measurement"] > 0
]
verified_with_probe = [
    r["variable_id"] for r in rows
    if r["semantics_status"] == "verified" and r["evidence"]["identity_probe"] > 0
]

summary = OrderedDict([
    ("r1_path", r1_path),
    ("format_version", doc.get("format_version")),
    ("family", doc.get("family")),
    ("revision", doc.get("revision")),
    ("definition_count", len(defs)),
    ("source_entry_total", sum(kind_counter.values())),
    ("count_by_kind", OrderedDict(sorted(kind_counter.items()))),
    ("count_by_manual_reference", OrderedDict(
        sorted(manual_ref_counter.items(), key=lambda kv: (-kv[1], str(kv[0]))))),
    ("count_by_semantics_status", OrderedDict(sorted(status_counter.items()))),
    ("count_by_scope", OrderedDict(sorted(scope_counter.items()))),
    ("cross_status_x_frozen_vector", OrderedDict(
        ("%s|%s" % k, v) for k, v in sorted(cross.items()))),
    ("source_field_type", OrderedDict(sorted(source_type_counter.items()))),
    ("definitions_with_multiple_sources", multi_source_count),
    ("definitions_without_source", no_source_count),
    ("definitions_missing_document_id", defs_missing_doc_id),
    ("definitions_missing_section", defs_missing_section),
    ("definitions_missing_page", defs_missing_page),
    ("definitions_with_independent_verified_field", independent_verified_field),
    ("source_entry_key_usage", OrderedDict(
        sorted(extra_source_keys.items(), key=lambda kv: (-kv[1], kv[0])))),
    ("verified_total", len(verified_ids)),
    ("verified_with_frozen_vector", len(verified_with_fv)),
    ("verified_with_measurement", len(verified_with_meas)),
    ("verified_with_identity_probe", len(verified_with_probe)),
    ("verified_only_manual_source_strict", len(verified_only_manual)),
    ("verified_only_manual_source_strict_ids", verified_only_manual),
    ("verified_manual_no_measurement_no_probe", len(verified_no_meas_no_probe)),
    ("verified_manual_no_measurement_no_probe_ids", verified_no_meas_no_probe),
])

with open(out_path, "w", encoding="utf-8") as f:
    json.dump(OrderedDict([("summary", summary), ("definitions", rows)]),
              f, ensure_ascii=False, indent=2)

print("OK", out_path)
```

复现命令（仓库根目录）：

```bash
python audit_sources.py drivers/sinumerik-nck/definitions/828d/r1.json audit.json
```

## 附录 B：机器可读原始数据

同一份逐条数据另存为 `docs/reference/828d-definition-source-audit.json`
（`summary` + `definitions` 两个顶层键，字段与本文表格一一对应）。
