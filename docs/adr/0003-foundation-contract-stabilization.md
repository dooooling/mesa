# ADR 0003：Foundation Contract Stabilization（基础层收口）

- 状态：accepted（Foundation-1 审计结论；删除动作放 Foundation-2，本 ADR 不改任何行为代码）
- 日期：2026-09-16
- 分支：`docs/foundation-1-contract-audit`（文档 only）
- 关联：ADR 0001（协议划分）→ ADR 0002（NCK 交叉验证）→ PR #38（evidence + control 审计闭环）
- 事实基线：`main cdadbcf`；GitHub Releases 为空（无正式 release）；workspace 版本 `0.3.0`

## 背景

项目从 MVP 经 V2、Events、Control、NCK 演进后，基础层出现"继续开发就会复制技术债"的
信号：新旧两套 Task/Binding 模型并存、Subscribe 描述能力缺口、Write Target 未冻结、
Probe/Connection 文档与事实不一致。本 ADR 是 Foundation 阶段的审计结论：
**明确什么冻结、什么删除、什么修改**，删除与修改动作分别由 Foundation-2～4 执行。

核心判断：**不是重新设计 Mesa，而是把已证明正确的设计提纯为干净的 1.0 基础契约。**
分层方向（Core 契约 → IPC → SDK → Manager → Driver）与进程隔离插件模型均不推倒。

## 决策 1：兼容边界（两种"兼容"必须区分）

前提（项目事实，需 owner 确认）：**当前没有已部署给第三方、必须原地升级读取
legacy binding 的 Mesa 实例。** 在此前提下：

```text
保留（已冻结、有价值的协议兼容）：
- Driver IPC / 版本协商 / 心跳 / 会话 token
- point_id 生命周期（Core 分配持久化 + tombstone）/ PointMap
- Data Plane：Point / Value / Quality / DataBatch / 背压 Latest-Wins
- Core 不懂协议（协议解析只存在于 Driver 进程内）

不保留（尚未正式发布的配置格式，直接删除，不做 deprecated 双轨、不做 migration shim）：
- s7.address-group
- focas.data-block
- opcua.node-group / opcua.subscription / opcua.browse
- simulator.points
- simulator.events（事件 legacy；见决策 5）
- Core API 的 legacy bypass（无 generic 即放行）
```

"V1 兼容基线严格只读"的澄清（同步更新 AGENTS.md 与 V2.1 文档，见"文档同步"节）：
该表述指 **V1 数据/IPC 只读基线兼容**（已冻结协议不破坏），**不**意味着
"所有 legacy binding 配置格式永久兼容"。两者不得混淆。

## 决策 2：FREEZE（冻结，不动）

| 项 | 冻结内容 | 依据 |
|---|---|---|
| Data Plane | Point / Value（含 typed arrays）/ Quality / ValueOrigin / DataBatch（stream_epoch/sequence/时间戳）/ Latest-Wins 背压 | 已扎实，除 correctness bug 外冻结 |
| point_id 生命周期 | Core 分配持久化、重启稳定、删除走 tombstone、永不自动 GC | §6 契约 |
| 插件模型 | Subprocess + Protobuf IPC；SDK 负责 IPC/token/心跳/shutdown/背压；Driver 只实现 Driver/DriverConnection | 进程隔离对工业 Driver 有实际价值，不搞 C ABI 热加载 |
| Core 不懂协议 | S7 DB / FOCAS Function / OPC UA NodeId 解析只在 Driver 进程内 | 硬性约束 |
| 配置真值 | 只在 Core；全量快照替换；运行中改任务走 Stop→Configure→ApplyPointMap→Start（新 stream_epoch） | 硬性约束 |

## 决策 3：REMOVE（Foundation-2 删除）

所有 legacy acquisition binding 及其 Core/Web/Test/示例入口。审计清单（main cdadbcf 实测）：

### 3.1 Driver configure 双分支（删 legacy 分支，保留 generic）

| Driver | legacy kind（删） | generic 现状（留） |
|---|---|---|
| s7 | `s7.address-group`（`drivers/s7/src/lib.rs:32` 定义，`:673` 分支） | `mesa.resources.v1`（`:459`，canonical area/db/offset/data_type） |
| focas2 | `focas.data-block`（`drivers/focas2/src/lib.rs:34` 定义，`:744` 分支） | `mesa.resources.v1`（`:709`，44 项 canonical Resource） |
| opcua | `opcua.node-group` / `opcua.subscription` / `opcua.browse`（`drivers/opcua/src/lib.rs:55-57` 定义，`:903-935` 三分支） | `mesa.resources.v1`（Poll only；Subscribe 缺口见决策 4） |
| simulator | `simulator.points`（`drivers/simulator/src/lib.rs:44` 定义，`:769` 分支） | `mesa.resources.v1`（`:724`，resource_id 即 SourceKind） |
| sinumerik-nck | 无 legacy（只接受 `GENERIC_BINDING_KIND`，`:405`） | 已是单一 generic 路径，Foundation-2 无需动 |

### 3.2 Core API bypass（删）

- `crates/core-api/src/lib.rs:232`：无 generic 任务即直接放行（"legacy true bypass"）。
  Foundation-2 改为：**所有 Data Task 必须走 Descriptor 统一校验**，无 generic 即无校验对象的时代结束。
- `gate_event_tasks`（`:310`）同理：`mesa.events.v1` 全量校验，见决策 5。

### 3.3 Web（删 legacy 识别/显示分支，保留 generic 生成路径）

- Web 当前**只生成** `mesa.resources.v1` / `mesa.events.v1`（`bootstrap.ts:124`、
  `taskBinding.ts`），无需改生成逻辑。
- 待清理的 legacy 痕迹：`deviceModel.ts` 对非 canonical kind 任务的"保留原样"逻辑
  （`:222` `driver.native.v1`、`:256-266` 自定义 binding 逐字保留）；
  `EndpointAcquisitionPane.tsx:220` 按 `binding.kind` 原样展示 Tag；
  `events/taskBinding.test.ts` 与 `EventTaskEditor.test.tsx` 中的 `legacy.private`
  测试桩。Foundation-2 删除或改写为"未知 kind 即拒绝"的 fail-closed 语义。

### 3.4 Contract suites 与性能测试（改写为 generic）

- `tests/driver-contract/tests/common/mod.rs:126`：Poll 任务构造 helper 用 `simulator.points`。
- `tests/driver-contract/tests/session_lifecycle.rs:79`：用 `s7.address-group` 表达"simulator 不支持"。
- `tests/driver-contract/tests/resource_contract.rs`：已有 generic 覆盖 + Legacy 兼容断言（`:2` 注释），Legacy 部分随删除而移除。
- `tests/driver-contract/tests/mesad_restart.rs:151`：内联 `simulator.points` 任务 JSON。
- `tests/performance/tests/`（`conn_1000.rs:25`、`data_plane_soak.rs:35`、
  `e2e_50k_real.rs:114`、`soak_short.rs:24,49`）：全部经 `simulator.points` 构造负载，
  需迁移到 generic 等价构造。
- `crates/config-store/src/lib.rs` 测试迁移脚本（`:2184`、`:2394`）中的
  `simulator.points` fixture。

### 3.5 文档（Foundation-1 已同步，见"文档同步"节）

- `docs/architecture.md:45`、`docs/flowchart.md:24-26`、`docs/flowchart.txt:16`：
  描述三 binding 现状，Foundation-2 删除后重写。
- `docs/plans/mesa-v2.1-full.md:178`（"Legacy 至少保留 1 major"）与本 ADR 冲突，
  以本 ADR 为准（该 plans 文件为历史施工计划，不再更新）。
- 各 Driver `GATE.md`/`README.md` 中的 binding 描述随代码删除同步更新。

## 决策 4：CHANGE（Foundation-2/3/4 修改）

### 4.1 Acquisition Model 2.0（Foundation-2，P0）

问题：`AcquisitionTask{ id, mode, interval_ms, binding }` 对 Subscribe 无描述能力，
导致 OPC UA `capabilities.subscribe=true` 但 canonical Resource `modes=[Poll]`
（`drivers/opcua/src/lib.rs:194-198` 自认"Descriptor 不报执行不了的 mode"，
Subscribe 只存在于 legacy runtime）——即 Descriptor lie 现场。

方向（细节由 Foundation-2 设计冻结）：

```text
AcquisitionTask
├─ id
├─ mode            # Poll | Subscribe
├─ options         # 按 mode 区分的调度参数（枚举，非松散字段）
│   ├─ Poll:      interval_ms
│   └─ Subscribe: publishing_interval_ms / sampling_interval_ms /
│                 queue_size / discard_oldest
└─ binding         # 仅 mesa.resources.v1
```

要求：Task Mode 自身有 Descriptor/Schema（Resource.modes 声明 + capabilities
交叉校验保留并补齐 Subscribe 语义）；`options` Core 做结构校验、Driver 做语义校验；
Poll 的 `interval_ms` 语义平滑迁移。

### 4.2 Control Write Target（Foundation-3，P0）

问题：SDK `write(target: &str, value, expected)`（`crates/driver-sdk/src/lib.rs:213`）
会退化为各 Driver 自发明 target 字符串（`DB10.DBW2` / `nsu=...;i=123` /
`macro.100`），与 Resource/Parameters/Output 模型冲突；Core 无法做统一权限校验
与审计身份。

方向：Write 回归 Resource 模型——`WriteTarget{ resource_id, parameters, output }`
（倾向此方案：覆盖"写未采集点"场景）或限定已配置 `point_key`（备选）。
`command` 同理走 `command_id + args schema` 落在 ControlCatalog 内。
**在 OPC UA Write、S7 Write、FOCAS Control 三个 Driver 同时开工前冻结。**

现状备注：PR #38 已建好平台层链路（API → enable_control → authorize →
audit STARTED → Driver → audit result）；各 Driver `write`/`command`
实现现状：simulator 有桩（`lib.rs:1121/1140`）、s7 有桩（`lib.rs:1060/1131`）、
focas2 仅 `command` 桩（`lib.rs:1043`）、opcua 尚未实现——冻结时一并对齐。

### 4.3 Probe / Connection 生命周期（Foundation-4，P1）

问题：SDK 注释要求 `probe()` 复用已建会话、"不得另建第二套连接逻辑"
（`crates/driver-sdk/src/lib.rs:164-166`），但 S7 `probe()` 实际另起短会话
（`drivers/s7/src/lib.rs:375-377`，`S7Client::connect`）——文档与事实不一致。

方向：**改契约文字，不改 S7 行为**（S7 短会话 probe 本身合理）。正式定义：

```text
open_connection() → 创建 Endpoint runtime object + 配置 fail-fast，
                    不承诺已建立物理协议 session
probe()/run()/write()/command() → 经 Driver 内部统一 session factory 懒建/复用连接
```

OPC UA 复用长连接、S7 建短会话均合法；"不得另建第二套"的本意收敛为
"不得绕过统一 factory 各自为政"。

### 4.4 Capability / Resource / Mode 一致性（Foundation-2 附带，P1）

`validate_task_set_against`（`crates/core-types/src/resource.rs:252`）已做
Resource.modes × capabilities 交叉校验——保留该机制，Foundation-2 补齐
Subscribe 语义后，OPC UA canonical Resource 恢复 `modes=[Poll, Subscribe]`，
消除 Descriptor lie。

## 决策 5：AUDIT（EventTask 双轨结论）

**结论：事件面存在与采集面同构的双轨，Foundation-2 一并删除。**

- 契约层：`mesa.events.v1`（`GENERIC_EVENT_BINDING_KIND`，`crates/core-types/src/event.rs:371`）
  与采集面 `mesa.resources.v1` 对等设计；`EventTask{ id, mode, interval_ms, binding }`
  同样允许任意 kind。
- 现状：OPC UA 事件只接受 generic（`drivers/opcua/src/event.rs:173`，fail-closed，
  可作为删除后的范本）；**simulator 事件保留 legacy `simulator.events` 双分支**
  （`drivers/simulator/src/lib.rs:912`），是事件面唯一的 legacy 分支。
- Core 侧 `gate_event_tasks` 同样存在"无 generic 即放行" bypass
  （`crates/core-api/src/lib.rs:318`）。
- Web 事件面只生成 `mesa.events.v1`（`events/taskBinding.ts`），与采集面对称。
- Foundation-2 删除范围因此包括：`simulator.events` 分支、事件 gate bypass、
  对应的 Web legacy 测试桩。

## 决策 6：SPLIT（Foundation-5，P1，纯搬移零行为变更）

三个大文件实测（main cdadbcf）：`config-store/src/lib.rs` 133KB /
`core-api/src/lib.rs` 117KB / `driver-manager/src/session.rs` 85KB
（另 `driver-sdk/src/lib.rs` 99KB 同量级，Foundation-5 一并评估是否拆分）。
原则：原 crate 内拆模块、一个 crate 一个 PR、纯搬移；必须在契约冻结之后做，
避免与 Foundation-2～4 产生合并冲突。

## 明确不做

- SQLite → PostgreSQL；C ABI 插件；统一所有工业协议 transport。
- 为 legacy binding 做 deprecated 双轨 / runtime migration shim（决策 1 已否决）。
- 重抽象 Value/Point/DataBatch（决策 2 已冻结）。

## 执行顺序

```text
Foundation-1（本分支，文档 only） 契约审计 → 本 ADR + 文档同步 + evidence 无漂移
Foundation-2  Acquisition Model 2.0（generic 单路径 + Subscribe 闭环 + 删 legacy）
Foundation-3  Control Model（structured write target + command + audit 身份）
Foundation-4  Lifecycle（改 SDK 契约文字 + 各 Driver 对齐）
Foundation-5  三个大文件拆模块（纯搬移）
```

分支策略：每阶段独立分支（`docs/foundation-1-contract-audit` /
`refactor/foundation-2-acquisition-model` / …），不使用长期承载分支。
每阶段结束跑 `python scripts/write-contract-evidence.py` 全量基线。

## 文档同步（Foundation-1 执行）

- `AGENTS.md`："V1 兼容基线严格只读"澄清为数据/IPC 基线含义；分支表述已更新
  （`feat/v2.1-impl` 已合并归档，当前以 main 为准——本次同步确认）。
- V2.1 唯一事实文档：以附录/修订形式记录本 ADR 决策（legacy 删除清单、
  Acquisition/Control/Lifecycle 变更方向），不重写已冻结章节。
