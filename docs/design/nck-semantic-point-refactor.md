# NCK 语义化 Point Definition 重构（Address Compiler 单一入口）

- 状态：**设计冻结稿**（评审结论 = **有条件通过**；三项修订已落实，见「修订记录」）
- 基线：`main = 0d686b4`（工作树干净）；`sinumerik-nck 0.5.0` 保持 experimental
- 上位依据：ADR 0001（协议划分）、ADR 0002（NCK 交叉验证）、ADR 0003（legacy binding 不作永久兼容承诺）、ADR 0004（原生寻址，Catalog 退出采集准入）
- 目标模型：**语义化 Point Definition 为用户配置模型** → **NCK Driver 内 Address Compiler 为唯一协议地址转换入口** → 现有 NCK ReadVar/Codec 为稳定执行层
- 硬性约束：**不迁移历史 NCK 点位配置与历史数据**；允许**明确声明**的破坏性重构（见 §5）
- 非目标：不改 Protobuf/IPC wire；不引入 Core 变量知识库；不做用户导入；不引入 Core 编译快照（v1）；不动 `src/connection/**`、`DescriptorFields.tsx`、`ApplyWithRestart.tsx` 的既有语义

---

## 修订记录（本次三项修订，对照评审结论）

> **评审结论：有条件通过。** 评审已正式采用「**点位层 Point Kind + 单一 NCK 任务信封**」，允许同一任务混合 `nck_variable` 与 `nck_native`；**不采用**按点位类型拆分两个任务的方案。理由：两者只是**地址表达方式不同**，不是不同采集协议或调度模型——最终都进入**同一个 NCK ReadVar**。
>
> 任务信封：`DriverBinding.kind = "nck.points.v1"`；每个点位**单独声明** Point Kind。
>
> 以下三项修订为通过条件，本次已全部落实。修订仅触及接口 Schema、术语、Point Key 规则、校验责任与测试矩阵；**评审已批准的其它部分（总体分层、Definition Revision 策略、62 条冻结 Golden、测试护栏、PR-A→PR-D 顺序）一律保留不变**。

### 修订 1（最重要）：Core 不得承载 Siemens 专用 `TargetRef`

| 项 | 修订前（本文档初稿） | 修订后（本次） |
|---|---|---|
| Core 侧类型 | `crates/core-types/src/point.rs` 内含 `TargetRef`/`DriveObjectRef`/`DriveEntry`，与「Core 不懂协议」冲突 | Core **只有**通用信封 `PointSelection`/`PointBinding`（`deny_unknown_fields` + `parameters` 不透明对象）；**零** Siemens 类型 |
| Target 归属 | 初稿未明确，实际落在 Core | `TargetRef`（`Channel`/`ModeGroup`/`Axis`/`Spindle`/`DriveObject`）与 `DriveEntry`（module **26**/**130** 入口身份）定义在 **NCK Driver 侧**：`drivers/sinumerik-nck/src/targets.rs` |
| 校验分工 | Core 校验 target 结构（`INVALID_TARGET`） | Core 只校验**信封**；`parameters` 内部（含 `target`）由 **Driver 严格校验**；Core **不应知道 Module 26/130**，也**不需要知道** `DriveEntry` 与 NCK 地址的对应关系 |
| 静默丢弃 | 靠 Core 理解结构才能拦住 | 靠「**信封层严格 + 参数层不透明**」：信封层未知字段即拒（终止 B7 的静默丢弃），参数层由 Driver 拒绝未知字段 |
| Point Kind 白名单 | Core 维护 `{nck_variable, nck_native}` 闭集 | **Core 侧不得建立 NCK Point Kind 白名单**（不枚举取值），只要求 `kind` 是**非空字符串**；语义校验归 Driver |

**核心口径**：**Core 负责阻止未知结构被静默丢弃，Driver 负责判断 Target 是否合法。**

### 修订 2：两个层级必须命名清楚（术语统一）

| 字段 | 名称 | 用途 |
|---|---|---|
| `DriverBinding.kind` | **Task Binding Kind** | `nck.points.v1` |
| `PointSelection.kind` | **Point Kind** | `nck_variable` / `nck_native` |

- Core 按 **Task Binding Kind** 路由；
- NCK Driver 对 **Point Kind** 做最终语义校验；
- 全文（§0/§1/§2/§3/§5/§6/§7/§8）已统一替换术语，§2 开头新增**术语表**（§2.0）。**禁止**把 `nck_variable`/`nck_native` 叫「Binding Kind」，也**禁止**把 `nck.points.v1` 叫「Point Kind」。

### 修订 3：Point Key 生成规则收紧

评审**批准**原 §4 的基本方向（同一语义变量 + Target + Arguments → 同一默认身份；Revision 变化 → **不**自动产生新 Point ID；Target 或 Arguments 变化 → 产生新的默认身份；Native 与 Semantic **不能互相顶替**）。本次补充两项：

1. **Canonical Key 必须无歧义**（§4.1.1）：对分隔符做**规范编码**（`#`/`=`/`@`/`,`/`/`/`%` 等在值中出现时按字节百分号编码），明确 **UTF-8 长度上限 = 256 字节**（依据见 §4.1.1 第 8 条）与**字符串规范化 = Unicode NFC**，并给出完整编码算法 + **5 个含歧义风险的示例**（值含分隔符、值含字面 `%`、非 ASCII/NFC、空值/缺省参数、可解析性歧义），逐条说明「编码后不同输入必得不同 Key」。
2. **Point Key 不是物理地址**（§4.1.2）：同一原生地址可能被用户用于不同逻辑采集用途，因此 **Native Point 不强制以地址本身作为唯一身份**；允许生成**稳定的逻辑 Key** 并**允许用户修改显示名称**；**点位身份不得依赖前端展示文案**。

同时把评审要求的三条合同测试写入测试矩阵（§6.6）：**同一任务混合点位**、**跨任务 Point Key 唯一性**、**Revision 更新后身份稳定性**。

### 本次保留不变（评审已批准）

总体分层（语义 Definition → Driver 内 Address Compiler → 既有 ReadVar/Codec 执行层）、Definition Revision 策略（§3）、62 条冻结 Wire Golden 的**完整覆盖**、Module 26/130 **不允许折叠**、不迁移历史 NCK 点位配置、非 NCK 功能与连接回归护栏不得退化、PR-A→PR-D 顺序（并强化：**PR-A 完成后必须独立验收才能进入 PR-B**）。

---

## 0. 已核对的事实基线（现查现核，非转述）

本节所有行号均为本次在 `0d686b4` 上实际读取所得。后续各节只引用本节结论。

### 0.1 后端事实

| # | 事实 | 位置 |
|---|---|---|
| B1 | `DriverBinding{kind: String, config: serde_json::Value}`，`config` 对 Core 不透明 | `crates/core-types/src/lib.rs:491-497` |
| B2 | `AcquisitionTask{id, schedule, binding}`；`schedule` 是调度唯一真值 | `crates/core-types/src/lib.rs:499-506` |
| B3 | proto 层 `binding_kind` / `binding_config_json` 双向透传，新 kind 字符串**不需要改 proto/IPC** | `crates/driver-protocol/src/lib.rs:317-318`、`:333-334`、`:507-508`、`:565-566` |
| B4 | Core `FieldType` 恰好 12 种扁平标量，无 object/array | `crates/core-types/src/schema.rs:40-57` |
| B5 | `validate_instance` 对未知字段报 `UNKNOWN_FIELD`（无法表达嵌套 `target`/`arguments`） | `crates/core-types/src/schema.rs:405-414` |
| B6 | `Descriptor` 契约 Major/Minor = 2/0（冻结面，本次**不得**扩 `SchemaDescriptor`） | `crates/core-types/src/descriptor.rs:9-13` |
| B7 | `ResourceSelection{resource_id, parameters, outputs}` **无 `deny_unknown_fields`** ⇒ 顶层新字段会被静默丢弃 | `crates/core-types/src/resource.rs:176-188` |
| B8 | `GenericBinding{selections}`；唯一通用 kind 常量 `mesa.resources.v1` | `crates/core-types/src/resource.rs:190-200`、`:228-229` |
| B9 | Task 集合级校验唯一入口 `validate_task_set_against`，含 Endpoint-wide `point_key` 唯一（`DUPLICATE_POINT_KEY`） | `crates/core-types/src/resource.rs:239-364`（:349-360） |
| B10 | Data Task 保存门禁是**单 kind 白名单**：非 `mesa.resources.v1` 即 `UNSUPPORTED_BINDING` | `crates/core-api/src/lib.rs:228-258`（:241、:244） |
| B11 | **同一白名单在 bootstrap 路径被复制了第二份**（本任务书未提及） | `crates/core-api/src/bootstrap.rs:218-251`（:225） |
| B12 | 事件 Task 另有独立门禁 `gate_event_tasks`（`mesa.events.v1`） | `crates/core-api/src/lib.rs:330-361`（:344） |
| B13 | 启动门禁复用 `gate_data_tasks` | `crates/core-api/src/runtime.rs:178` |
| B14 | `point_id` 由 `assign_point_ids` 稳定分配：已有 key（含 tombstone）复用原 id，新 key 取 `max+1`，缺席 key 置 `deleted=1` | `crates/config-store/src/lib.rs:1655-1770`（tombstone :1750-1766） |
| B15 | `tasks.binding_kind` / `binding_config_json` 为持久化列 | `crates/config-store/src/lib.rs:420-421`、`crates/config-store/migrations/001_initial.sql:27-28` |
| B16 | `ErrorKind` 9 类（Configuration/Connection/Timeout/Address/Protocol/Decode/Device/Unsupported/Internal） | `crates/core-types/src/lib.rs:616-628` |
| B17 | `UiHints.advanced` 存在且是唯一折叠机制 | `crates/core-types/src/schema.rs:76-89`（:86） |
| B18 | 所有 5 个驱动（s7/focas2/opcua/sinumerik-nck）在 configure 内**各自**硬校验 `kind == GENERIC_BINDING_KIND` | `drivers/s7/src/lib.rs:479`、`drivers/focas2/src/lib.rs:1279`、`drivers/opcua/src/lib.rs:944`、`drivers/sinumerik-nck/src/lib.rs:613` |

### 0.2 NCK Driver 事实

| # | 事实 | 位置 |
|---|---|---|
| N1 | `DRIVER_ID="sinumerik-nck"`；唯一资源 id `GENERIC_RESOURCE_ID="variable"`；`DECLARED_FAMILIES=["828d","840d-sl"]` | `drivers/sinumerik-nck/src/lib.rs:50-58` |
| N2 | `variable` 资源参数是**12 个原生协议字段**（area/area_no/module/column/line/count/unit_mode/decode/transport_size/invalid_value/invalid_tolerance/unit），其中 8 个 `required`；**没有任何字段带 `ui.advanced`** | `drivers/sinumerik-nck/src/lib.rs:178-311` |
| N3 | `variable` 只有一个 output `value`，`type_spec = DriverResolved` | `drivers/sinumerik-nck/src/lib.rs:313-319` |
| N4 | NCK 声明 `resource_selection_methods = [Manual, Browse]` | `drivers/sinumerik-nck/src/lib.rs:324-327` |
| N5 | configure 内唯一解析链：`NativeNckAddress::from_parameters` → `resolve_native` | `drivers/sinumerik-nck/src/lib.rs:670-683` |
| N6 | configure 还硬校验 `resource_id == "variable"`（:644）与 `output == "value"`（:653） | `drivers/sinumerik-nck/src/lib.rs:644`、`:653` |
| N7 | 地址字段允许列表恰好 12 项，未知字段 `UnknownField` | `drivers/sinumerik-nck/src/native.rs:146-160`、`:231-235` |
| N8 | `NativeNckAddress` 12 字段 + `ResolvedPoint{address, wire, kind, expected_data_len, expected_transport_size, data_type, invalid_rule}` | `drivers/sinumerik-nck/src/native.rs:115-144` |
| N9 | 无效值规则必须成对、容差 0..=1、仅数值型解码可用 | `drivers/sinumerik-nck/src/native.rs:285-318` |
| N10 | `source_label` 稳定格式 `nck://read/{area}/{area_no}/{module}/{column}/{line}/{count}?mode=&decode=&transport=` | `drivers/sinumerik-nck/src/native.rs:347-361` |
| N11 | `NckArea` 7 个（N/B/C/A/T/V/H）；`M`/MMC 明确拒绝（待真机确认） | `drivers/sinumerik-nck/src/address.rs:20-82`（:66-81） |
| N12 | `NckUnitMode` current/metric/inch → syntax `0x82/0x83/0x84` | `drivers/sinumerik-nck/src/address.rs:88-121` |
| N13 | 线缆 10 字节 `0x12 0x08 syntax area_unit column(2) line(2) module line_count` | `drivers/sinumerik-nck/src/codec.rs:25-36`、`:72-84` |
| N14 | `NckDataKind` 15 个规范 decode 名（`DECODE_NAMES`），别名（REAL/DINT）一律拒绝 | `drivers/sinumerik-nck/src/native.rs:41-58`、`drivers/sinumerik-nck/src/value.rs:97-140` |
| N15 | decode → byte_len / core_type / core_array_type 映射完整可引用 | `drivers/sinumerik-nck/src/value.rs:42-95` |
| N16 | 身份探针内建常量：module **26** 与 **130** 是两条**不可互换**入口（实体机实测同 `V/n` 返回不同对象名） | `drivers/sinumerik-nck/src/identity_spec.rs:176-204` |
| N17 | 预填模板 62 个（= 42 条候选，含驱动对象双入口拆分），`validation_status` 恒 `candidate`，且**禁止**预填 `area_no`/`line` | `drivers/sinumerik-nck/presets/828d.json`、`drivers/sinumerik-nck/src/presets.rs:117-149` |
| N18 | 模板叶 `binding_json` 形态为 `{"resource_id":"variable","parameters":{...}}` | `drivers/sinumerik-nck/src/presets.rs:151-186`（:167-171） |
| N19 | NCK 驱动 Cargo 依赖**不含 `sha2`**（workspace 已有 `sha2 = "0.10"`） | `drivers/sinumerik-nck/Cargo.toml:16-27`、`Cargo.toml:53` |
| N20 | 摘要先例：`lowercase hex SHA-256(canonical JSON)`，且明确「不依赖派生 Serialize 的偶然形状」 | `crates/event-store/src/lib.rs:235-249`（canonical 见 :149-233） |

### 0.3 冻结向量与语义索引事实

| # | 事实 | 位置 |
|---|---|---|
| G1 | Wire Golden 恰 **62** 条 = 42（NC 变量入口）+ 20（驱动对象入口） | `drivers/sinumerik-nck/tests/legacy_equivalence.rs:48-52`、`drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` |
| G2 | 62 条 golden 的 `area_no` 与 `line` **全部为 1**（本次逐条 dump 核实） | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` |
| G3 | 62 条 golden 的 syntax 字节全部为 `0x82`（unit_mode=current） | 同上（`wire_hex` 第 3 字节） |
| G4 | 20 条驱动对象条目必须与同参数 NC 变量条目**同 column、不同 module、不同 wire** | `drivers/sinumerik-nck/tests/legacy_equivalence.rs:110-133` |
| G5 | 语义索引（人工确认参考）：`C/S`=通道实例 line=1；`B/S`=BAG 实例 line=1；`C/RP` line=R参数号+1；`C/SGA` line=轴索引=MCS 轴设定位置；`C/SEGA` line=轴索引=BCS 实际位置；`C/SSP` line=主轴索引；`C/SPARPP` line=程序层级；`V/M` area_no=驱动访问编号 | `drivers/sinumerik-nck/presets/README.md:35-52` |
| G6 | 42 条候选的 decode/transport/元素字节/module 全表 | `drivers/sinumerik-nck/presets/PROVENANCE.md:9-54` |
| G7 | `r0035` 无效温度哨兵 `-200`（容差 `0.001`），依据 SINAMICS S120/S150 参数手册 11/2023 p.71 | `drivers/sinumerik-nck/presets/README.md:18`、`:83-88` |
| G8 | 驱动对象**轴归属未证明**（只读途径已穷尽）；`r0031` 复测被 `0x85` 拒绝、`r0033` 可读 | `docs/evidence/nck-remaining-20-reconciliation.md:76-104`、`:156-178` |
| G9 | 「保留为模板」**不等于**工程语义已验证（PROVENANCE 明文） | `drivers/sinumerik-nck/presets/PROVENANCE.md:1-7` |
| G10 | `SPIND_ASSIGN_TO_MACHAX` 用于取轴的主轴编号，避免把机床轴号当主轴索引 | `drivers/sinumerik-nck/presets/README.md:110` |
| G11 | 7 项（设备逻辑状态/系统信息/程序注释/报警汇总）**不是单个 NC 变量**，不得填假地址 | `drivers/sinumerik-nck/presets/PROVENANCE.md:70-82`、`docs/evidence/nck-remaining-20-reconciliation.md:117-139` |

### 0.4 契约套件事实

| # | 事实 | 位置 |
|---|---|---|
| C1 | 契约套件名单 = BASELINE 14 + EVENT 12 + LIFECYCLE 3 = **29** 套 | `scripts/contract_suites.py:13-52` |
| C2 | `AGENTS.md` 写的「27 suites」**已过期**（ADR 0004 写的「28/28」同样过期） | `AGENTS.md`、`docs/adr/0004-nck-native-addressing.md:99` |
| C3 | 直接引用 NCK 的契约套件：`descriptor_contract`、`discovery_contract`、`resource_contract` | `tests/driver-contract/tests/descriptor_contract.rs:391-396`、`:593-603`、`:728-731`；`tests/driver-contract/tests/discovery_contract.rs:224-269`；`tests/driver-contract/tests/resource_contract.rs:216` |
| C4 | NCK 驱动自有测试目标与用例数：`native_address` 8 / `presets_828d` 6 / `examples_828d` 3 / `drive_access` 6 / `identity_browse` 8 / `legacy_equivalence` 2 / `sharp7_vectors` 6 / `live_*` 2（`#[ignore]`） | `drivers/sinumerik-nck/tests/*.rs` |
| C5 | 三条冻结断言：`:59` 同 key 恢复同 id、`:92` 删除 id 永不复用、`:153` 重新加入恢复原 id | `tests/driver-contract/tests/data_semantics.rs:57-88`、`:90-149`、`:151-183` |

### 0.5 前端事实

| # | 事实 | 位置 |
|---|---|---|
| F1 | 生产代码中**零**硬编码 NCK 原生协议字段；`area_no/transport_size/unit_mode/F32_BE` 等只在 `browseModel.test.ts` 出现 | 全仓 grep 核实（见 §1.5） |
| F2 | `missingRequiredParams` 按**整份 schema** 判必填（缺席=key 不存在或 `undefined`） | `apps/mesa-web/src/resourceSelectionModel.ts:44-71` |
| F3 | `variable` 资源无任何 `ui.advanced` ⇒ 现有折叠机制对 NCK 今天完全用不上 | N2 + `apps/mesa-web/src/components/DescriptorFields.tsx:336-349` |
| F4 | Binding 装配**双真值**：`deviceModel.ts` 与 `addDevice/bootstrap.ts` 各自硬编码字面量 kind | `apps/mesa-web/src/deviceModel.ts:301`、`apps/mesa-web/src/addDevice/bootstrap.ts:138` |
| F5 | 前端只编辑 canonical kind 任务，其余原样保留（全量替换安全） | `apps/mesa-web/src/deviceModel.ts:195-252`（:210、:239、:249） |
| F6 | `point_key` 自动命名 = `resource.output`（即今天的 `variable.value`），冲突递增 `.2/.3` | `apps/mesa-web/src/components/ResourcePickerAntd.tsx:41-51`、`:143-150` |
| F7 | 加入按钮前置门：`outputs.length>0` 且 `missingRequired.length===0`，onClick 再判一次 | `apps/mesa-web/src/components/ResourcePickerAntd.tsx:241-286` |
| F8 | browse 需要 `endpointId`；缺失时浏览 tab 直接 disabled 并提示 | `apps/mesa-web/src/components/ResourcePickerAntd.tsx:160-171` |
| F9 | AddDeviceFlow 数据步**端点尚不存在**，因此今天只能手动添加 | `apps/mesa-web/src/addDevice/AddDeviceFlow.tsx:6`、`:291` |
| F10 | browse 节点 → Selection 映射只理解两种信封（显式 `resource_id` / OPC UA 裸参数） | `apps/mesa-web/src/browseModel.ts:1-56` |
| F11 | 共享 onAdd 实现 `applySelectionAdd` 已统一（reconciliation 单真值），但**只统一裁决，不统一装配** | `apps/mesa-web/src/resourceSelectionModel.ts:319-377` |
| F12 | `EndpointAcquisitionPane` 走 `mergeAcquisitionTasks`（即 F4 的第一处），无第三处装配 | `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:123` |
| F13 | `src/connection/**` 共 9 个文件（3 个测试） | `apps/mesa-web/src/connection/*` |
| F14 | 前端实测基线：**33 文件 / 383 用例全过**（本次实跑） | `pnpm test` @ `0d686b4` |
| F15 | 受影响的 7 个文件合计 **119 用例**（本次实跑）；连接组 3 文件 **134 用例** | 见 §6.0 |

---

## 1. 逐文件修改清单

判定口径：**新增** = 本次新建；**重构** = 语义/形状变更；**保留（冻结）** = 本次不得改动；**删除** = 本次移除。

### 1.1 新增

| 路径 | 现状职责 | 判定 | 理由 | 影响面 |
|---|---|---|---|---|
| `crates/core-types/src/point.rs` | 不存在（`crates/core-types/src/` 现有 7 个模块，无 point） | 新增 | Core 需要显式**通用点位信封**承载「一个任务多点位」；`FieldType` 12 扁平标量（B4）与 `UNKNOWN_FIELD`（B5）表达不了嵌套对象 | 新增公开类型**仅** `PointSelection`/`PointBinding`（**信封层严格 + `parameters` 不透明**）；**不含任何 Siemens 类型**（修订 1）；不改 `SchemaDescriptor`（B6） |
| `crates/core-types/src/point.rs` 内的 `NCK_POINTS_BINDING_KIND`（**Task Binding Kind**）与 `point_key` 规范编码纯函数 | 不存在 | 新增 | 与 `GENERIC_BINDING_KIND`（B8）同构的常量，禁止魔数；`point_key` 规范编码必须是 Core 侧可复用的纯函数（§4.1.1） | Core 门禁与前端常量同源；**Core 侧不建立 Point Kind 白名单** |
| `drivers/sinumerik-nck/src/definition.rs` | 不存在 | 新增 | 变量定义目录（`VariableDefinition`、revision、digest、来源、验证状态）与解析 | 驱动新增模块；`native.rs` 不动 |
| `drivers/sinumerik-nck/src/compiler.rs` | 不存在（今天转换逻辑内联在 configure :670-683） | 新增 | Address Compiler 唯一入口：`PointSelection + VariableDefinition → ResolvedPoint`；含**参数层严格校验**（§2.4.2 的 D1–D9） | configure 改为调用它；`resolve_native` 仍为下游 |
| `drivers/sinumerik-nck/src/targets.rs` | 不存在 | 新增 | **Siemens 专用**：`TargetRef`（Channel/ModeGroup/Axis/Spindle/DriveObject）、`DriveObjectRef`、`DriveEntry`（module **26**/**130** 入口身份）的**定义与语义校验**；Core **不得**知道这些类型（修订 1） | 与 `identity_spec.rs` 的入口常量对齐（N16）；PR-B 首次定义 |
| `drivers/sinumerik-nck/src/definitions/828d.json` | 不存在（今天只有 `presets/828d.json`） | 新增 | 定义目录数据文件，随驱动版本发布（决策 3） | 编译期内嵌 `include_str!`，与 `presets.rs:23` 同模式 |
| `drivers/sinumerik-nck/tests/definition_catalog.rs` | 不存在 | 新增 | 定义目录完整性/revision/digest 断言 | 新测试目标，需登记 `scripts/contract_suites.py` 之外（驱动自有目标，不进契约名单） |
| `drivers/sinumerik-nck/tests/semantic_equivalence.rs` | 不存在（`legacy_equivalence.rs` 是模板路径版） | 新增 | 语义路径逐条复现 62 条冻结字节 | 62 条 golden 的**新**等价门；旧门保留（见 1.3） |
| `apps/mesa-web/src/nckPointModel.ts` | 不存在 | 新增 | NCK 语义点位的纯函数层：payload 构造、`point_key` 推导、target/arguments 默认值、必填预检 | 纯函数，不 import React/AntD（与 `resourceSelectionModel.ts:18` 同纪律） |
| `apps/mesa-web/src/components/NckPointFields.tsx` | 不存在 | 新增 | NCK 专用语义表单（变量选择 + target + arguments），**不改** `DescriptorFields.tsx` | 新组件；`DescriptorFields` 语义冻结 |
| `apps/mesa-web/src/nckPointModel.test.ts` | 不存在 | 新增 | 上述纯函数单测 | 新用例，计入新增 |
| `apps/mesa-web/src/components/NckPointFields.test.tsx` | 不存在 | 新增 | 语义表单渲染/必填/target 校验 | 新用例，计入新增 |
| `docs/adr/0005-nck-semantic-point-definition.md` | 不存在（`docs/adr/` 现有 0001–0004） | 新增 | 本次破坏性重构必须留 ADR（§5 逐条判定） | 决策留痕；ADR 0004 不修改 |

### 1.2 重构

| 路径 | 现状职责 | 判定 | 理由 | 影响面 |
|---|---|---|---|---|
| `crates/core-api/src/lib.rs:228-258` | `gate_data_tasks` 单 kind 白名单（:241） | 重构 | 改为按 **Task Binding Kind** 路由（`{mesa.resources.v1, nck.points.v1}`）并对 `nck.points.v1` 做**信封层**结构校验；**不得**在此处校验 `parameters` 内部或建立 Point Kind 白名单（修订 1） | 门禁语义；错误码新增/复用见 §2.7、§2.8 |
| `crates/core-api/src/bootstrap.rs:218-251` | 同一白名单的**第二份复制**（:225） | 重构 | B11：两份必须收敛为**同一函数**，否则出现「保存通过、bootstrap 拒绝」分叉 | 与上条必须同一 PR 内完成 |
| `crates/core-api/src/runtime.rs:178` | 启动门禁复用 `gate_data_tasks` | 重构（仅随上游变化） | 启动门禁必须与保存门禁同源，不得新增第二套判定 | 无需独立逻辑改动 |
| `drivers/sinumerik-nck/src/lib.rs:596-739` | configure：读 `GenericBinding` → 逐 output → 原生解析 | 重构 | 改为按 **Point Kind** 路由到 Address Compiler（Point Kind 语义校验与参数层校验均在 Driver 侧，§2.4.2）；删掉 `resource_id`/`output` 硬校验（:644、:653） | 驱动 configure 契约；`resolve_native` 与 PDU 预检（:691-714）保留 |
| `drivers/sinumerik-nck/src/lib.rs:170-330` | Descriptor 的 `variable` 资源 = 12 原生字段 | 重构 | 语义模式下用户不得面对原生字段；`nck_native` 走独立高级路径 | Descriptor 资源面变化（§5-4）；`descriptor_contract.rs:728-731` 需改 |
| `drivers/sinumerik-nck/src/lib.rs:324-327` | `resource_selection_methods=[Manual,Browse]` | 重构 | 语义表单不走 Resource 参数渲染；browse 语义重做（§5-5） | `discovery_contract.rs:224-269` 需改 |
| `drivers/sinumerik-nck/src/presets.rs:151-186` | 模板叶 `binding_json = {resource_id:"variable",parameters:{原生字段}}` | 重构 | 浏览叶必须产出**语义载荷**（`nck_variable` + target/arguments），否则前端仍被迫构造原生字段 | `browseModel.ts` 映射需扩展（F10） |
| `apps/mesa-web/src/deviceModel.ts:203-210` | `ResourceSelection` 形状 + `CANONICAL_RESOURCES_KIND` | 重构 | 需新增 NCK canonical 形态与 kind 常量 | 影响 `deviceModel.test.ts`、`EndpointAcquisitionPane` |
| `apps/mesa-web/src/deviceModel.ts:231-252` | `splitAcquisitionTasks`/`selectionsOf` 只认 `mesa.resources.v1` | 重构 | 必须同时识别 NCK kind，否则编辑 NCK Endpoint 时任务被当成 preserved 或被丢弃 | 数据安全关键路径（F5） |
| `apps/mesa-web/src/deviceModel.ts:280-304`（:301） | Binding 装配**真值 1** | 重构 → **收敛** | 与 `bootstrap.ts:138` 合并为唯一装配入口（决策 7） | 双真值消除；两处调用方同步 |
| `apps/mesa-web/src/addDevice/bootstrap.ts:124-155`（:138） | Binding 装配**真值 2** | 重构 → **收敛** | 同上 | 同上 |
| `apps/mesa-web/src/resourceSelectionModel.ts:20-25` | `ResourceSelectionLike` 仅通用形态 | 重构 | 需扩展为「通用形态 + NCK 语义形态」的判别联合，identity 扩展（§4） | reconciliation 算法本体（:169-377）保持不变 |
| `apps/mesa-web/src/browseModel.ts:1-56` | 只理解两种信封 | 重构 | 增加 `nck_variable`/`nck_native` 信封，且**不得**把原生字段塞进语义信封 | `browseModel.test.ts:109-135` 必须改写 |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx:241-286` | 按整份 schema 判必填（F2） | 重构 | 语义模式若仍传原生 schema，`module/column/...` 的 required 会**永久阻止添加** | 必须按 Kind 分流必填来源 |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx:41-51` | `suggestPointKey = resource.output` | 重构 | 改为语义 `point_key` 推导（§4） | `deviceModel.test.ts`、`ResourcePickerAntd.test.tsx` |
| `apps/mesa-web/src/addDevice/AddDeviceFlow.tsx:1-11, 240-292` | 数据步只 manual（F9） | 重构 | 改为「先建 Endpoint 再选点位与 Target」（决策 6） | 步骤顺序变化；离线允许手工选实例 |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:123` | 走 `mergeAcquisitionTasks` | 重构（随上游） | 装配收敛后此处只调用共享入口 | 无独立语义 |
| `tests/driver-contract/tests/data_semantics.rs:57-183` | 三条 point_key/point_id 冻结断言 | 重构（**必须显式重写**） | §4.3 逐条给出改写后的断言 | 契约语义；下界不得减少 |
| `tests/driver-contract/tests/descriptor_contract.rs:391-396, 593-603, 728-731` | NCK Descriptor 只读 + `variable` resource 断言 | 重构 | 资源面变化后断言必须跟随，但**只读/无 subscribe/无 events 三条不得放宽** | 契约套件 |
| `tests/driver-contract/tests/discovery_contract.rs:224-269` | NCK browse 树 e2e（`nck://presets`） | 重构 | 浏览叶形态变化 | 契约套件 |
| `drivers/sinumerik-nck/presets/828d.json` | 62 个预填模板（原生字段） | 重构 | 必须改为承载语义 target/arguments，否则与新模型矛盾 | 62 个模板全部重写；**golden 字节不得变** |
| `drivers/sinumerik-nck/presets/README.md` | 索引语义与入口说明 | 重构 | 需说明语义 ID ↔ 原生身份对应与 `semantics_status` | 文档；PROVENANCE 的 42 条清单不删 |
| `drivers/sinumerik-nck/Cargo.toml:16-27` | 依赖表（无 `sha2`，N19） | 重构 | 定义摘要需要 SHA-256 | 新增 `sha2 = { workspace = true }` |
| `AGENTS.md` | 写「27 suites」（C2） | 重构 | 与 `scripts/contract_suites.py` 的 29 套对齐 | 仅文档纠错，独立小 PR 或随 PR-A |

### 1.3 保留（冻结，本次**不得**改动）

| 路径 | 现状职责 | 判定 | 理由 | 影响面 |
|---|---|---|---|---|
| `drivers/sinumerik-nck/src/native.rs` | 12 字段严格解析 + `resolve_native` | 保留 | 稳定执行层；Compiler 的输出下游 | Compiler 必须复用其错误类型与规则 |
| `drivers/sinumerik-nck/src/codec.rs` | 10 字节 wire 编码 | 保留 | 62 条 golden 的字节真值在此 | 不得改动字节布局 |
| `drivers/sinumerik-nck/src/value.rs` | decode/类型/无效值 | 保留 | 运行质量处理（BAD/GOOD、无效哨兵）不得退化 | 不得改动映射 |
| `drivers/sinumerik-nck/src/client.rs` | S7 会话/ReadVar | 保留 | 执行层 | — |
| `drivers/sinumerik-nck/src/address.rs` | Area/UnitMode 枚举 | 保留 | 协议基础枚举（M/MMC 仍拒绝） | 不得新增未验证字母 |
| `drivers/sinumerik-nck/src/identity_spec.rs` | 内建身份探针（module 26/130 双入口） | 保留 | 真机证据；决策 5 不得虚构地址 | 定义目录只**引用**其常量 |
| `drivers/sinumerik-nck/src/identity.rs` | 身份浏览 | 保留 | 与采集解耦 | — |
| `drivers/sinumerik-nck/src/probe.rs`、`fixture.rs`、`config.rs`、`main.rs` | 探测/夹具/连接配置/入口 | 保留 | 非本次范围 | — |
| `drivers/sinumerik-nck/tests/legacy_equivalence.rs` | 62 条冻结字节等价门 | 保留 | G1–G4：**这是历史字节的权威门**，不得删除或放宽 | 与新增 `semantic_equivalence.rs` 并存 |
| `drivers/sinumerik-nck/tests/native_address.rs`、`presets_828d.rs`、`examples_828d.rs`、`drive_access.rs`、`identity_browse.rs`、`sharp7_vectors.rs` | 原生解析/模板/示例/驱动访问/身份/协议向量 | 保留 | 非 NCK 语义重构不得使其退化（指令 9） | 用例数不得减少 |
| `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | 62 条 golden 数据 | 保留 | 冻结真值，**不得**修改 | 任何改动即等价门失效 |
| `crates/core-types/src/schema.rs` | 12 FieldType + `UNKNOWN_FIELD` | 保留 | Descriptor 2.0 冻结面（B4/B5/B6） | 不得扩 `SchemaDescriptor` |
| `crates/core-types/src/descriptor.rs` | Descriptor 契约常量 | 保留 | Major/Minor 2/0 冻结 | 不得升级 |
| `crates/core-types/src/resource.rs`（`ResourceSelection`/`GenericBinding`/`validate_task_set_against`） | 通用资源模型与集合校验 | 保留 | 其他 4 个驱动依赖（B18）；NCK 走新信封 | 不得改其形状 |
| `crates/core-types/src/event.rs`、`capability.rs`、`probe.rs` | 事件/能力/探测契约 | 保留 | 非本次范围 | — |
| `crates/config-store/src/lib.rs:1655-1770` | point_id 稳定映射 + tombstone | 保留 | 语义模型下**语义不变**（§4.2） | 不改实现，只改调用侧断言 |
| `crates/driver-protocol/**`、`proto/**` | IPC/proto | 保留 | B3：kind 字符串透传，无需改 | 不得改 wire |
| `crates/driver-sdk/**` | 驱动 SDK | 保留 | 非本次范围 | — |
| `apps/mesa-web/src/connection/**`（9 文件） | 连接配置 | **冻结** | 指令 8；134 用例必须原样通过 | 不得改语义 |
| `apps/mesa-web/src/components/DescriptorFields.tsx` | 受控描述字段 + 高级折叠 | **冻结** | 指令 8；折叠机制是通用行为（F3） | 新语义层**不得**改它 |
| `apps/mesa-web/src/components/ApplyWithRestart.tsx` | 统一生命周期条 | **冻结** | 指令 8 | — |
| `apps/mesa-web/src/resourceSelectionModel.ts:169-377` | reconciliation 冻结算法 | 保留 | 「exact duplicate/point_key 冲突/merge/append」语义与协议无关 | 只扩 identity 输入，不改裁决 |
| `drivers/s7/**`、`drivers/focas2/**`、`drivers/opcua/**` | 其他驱动 | 保留 | 指令 9：非 NCK 驱动不得退化 | 不得改 |
| `docs/adr/0001-0004` | 既有 ADR | 保留 | 历史决策不可改；本次新增 0005 | — |

### 1.4 删除

| 路径 | 现状职责 | 判定 | 理由 | 影响面 |
|---|---|---|---|---|
| `drivers/sinumerik-nck/src/lib.rs` 内 `resource_id == "variable"` 与 `output == "value"` 两条硬校验（:644、:653） | 旧资源面准入 | 删除 | 新模型无 `resource_id`/`outputs` 概念 | 错误码 `UNSUPPORTED_RESOURCE`/`UNSUPPORTED_OUTPUT` 在 NCK 侧不再产生（保留常量供其他驱动/SDK 使用） |
| `GENERIC_RESOURCE_ID` 作为**用户可选资源**的角色（`drivers/sinumerik-nck/src/lib.rs:52`、`:170-171`） | 唯一资源 id | 删除（作为用户面资源） | 语义模型下用户不再选「资源」 | 常量本身保留供 `nck_native` 高级路径与浏览诊断复用 |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx` 中「按整份原生 schema 判必填」的 NCK 分支 | 必填前置门 | 删除（仅 NCK 分支） | F2：原生 schema 的 required 会永久阻止语义点添加 | 通用驱动分支**保留** |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx:41-51` 的 `suggestPointKey` 作为 **NCK** 命名依据 | 命名 | 删除（仅 NCK 用途） | 改为语义推导（§4.1） | 函数保留给通用驱动 |

**明确声明**：除上表 4 项「局部删除」外，**本次没有可删除的文件级产物**。特别地——

- **前端无可删除项**：F1 已核实生产代码零硬编码 NCK 原生协议字段（`area_no`/`transport_size`/`unit_mode`/`F32_BE` 等只在 `apps/mesa-web/src/browseModel.test.ts` 出现）。因此本次前端工作**不是**「删除前端字段表」，而是：**换渲染入口**（新增 `NckPointFields.tsx`，`DescriptorFields.tsx` 冻结）、**扩展 Selection 身份**（`resourceSelectionModel.ts` 的 identity 联合类型）、**重做 browse 语义**（`browseModel.ts` + `presets.rs` 的叶形态）。
- **`presets/828d.json` 不删除**：它承载 62 个模板与 62 条 golden 的对应关系（G1）；本次**重构其内容**（原生字段 → 语义 target/arguments），不删文件。

---

## 2. 接口 Schema

### 2.0 术语表（全文统一，禁止混用）

| 术语 | 字段位置 | 取值 | 谁解释 |
|---|---|---|---|
| **Task Binding Kind** | `DriverBinding.kind`（**任务层**，B1） | `nck.points.v1`（NCK 任务信封）；既有 `mesa.resources.v1` / `mesa.events.v1` 不变 | **Core** 按它**路由**（§2.1） |
| **Point Kind** | `PointSelection.kind`（**点位层**，`config.points[i].kind`） | `nck_variable` / `nck_native` | **NCK Driver** 做最终语义校验；Core 只要求它是**非空字符串**，**不建白名单** |
| **点位信封** | `DriverBinding.config` 的 `{ "points": [PointSelection] }` | — | Core 校验**结构**（§2.4） |
| **参数层** | `PointSelection.parameters` | **不透明** JSON 对象 | **NCK Driver** 严格校验（§2.4.2） |
| **Target** | `parameters.target`（仅 `nck_variable`） | Siemens 专用变体（§2.4.1） | **NCK Driver**（Core 不认识其形状） |

**禁止的表述**：把 `nck_variable`/`nck_native` 叫「Binding Kind」；把 `nck.points.v1` 叫「Point Kind」。二者分别只叫 **Task Binding Kind** 与 **Point Kind**。

### 2.1 任务信封与 Task Binding Kind（评审已裁决：单一信封 + 点位层 Point Kind）

任务书给出的固定载荷形态是 `{"kind":"nck_variable","parameters":{...}}`——它是**点位**形态（`DriverBinding` 的键是 `config`，不是 `parameters`，B1）。据此本设计把 **Point Kind** 定在**点位**层，并为 NCK 增加一个**任务层**的 **Task Binding Kind**：

```jsonc
// DriverBinding.kind —— Task Binding Kind（Core 按它路由）
"nck.points.v1"
// DriverBinding.config —— 点位信封（一个任务可含多个点位）
{
  "points": [ /* PointSelection[]，见 2.4 */ ]
}
```

**评审裁决（已采纳，不得再作为待确认项）**：允许**同一任务**混合 `nck_variable` 与 `nck_native`；**不采用**「按点位类型拆分两个任务」的方案。理由：两种 Point Kind 只是**地址表达方式不同**，不是不同采集协议或调度模型——最终都进入**同一个 NCK ReadVar**；拆分会把一个用户操作变成两个任务，并破坏前端「唯一 canonical 任务」模型。

**Core 的路由与边界（修订 1 的直接后果）**：

1. Core **只按 Task Binding Kind 路由**：`DriverBinding.kind ∈ {mesa.resources.v1, nck.points.v1}`，其余仍 `UNSUPPORTED_BINDING`（复用 `crates/core-api/src/lib.rs:244`）；**不做「未知 kind 静默放行」**。
2. Core **不建立 NCK Point Kind 白名单**：不枚举 `nck_variable`/`nck_native`，只要求 `PointSelection.kind` 是**非空字符串**。
3. Core **不解释** `parameters` 内任何键（不认识 `variable_id`/`definition_revision`/`target`/`arguments`/`module`/`entry`），只要求它是 **JSON 对象**。
4. Point Kind 的语义校验、Target 合法性、地址合法性**全部归 NCK Driver**（§2.4.1、§2.4.2、§2.8）。

**为什么需要任务层信封，而不是把 Point Kind 提到任务层**：

1. 现有模型是「一个任务多个点位」（`GenericBinding.selections`，B8），且 `pdu_length`/`max_read_items` 的批读与轮询周期都在任务层（`drivers/sinumerik-nck/src/lib.rs:153-167`）。若任务层只能有一个 Point Kind，则**一个任务无法同时包含语义点与高级原生点**，且前端必须把一次用户操作拆成两个任务。
2. 前端「单任务编辑器」模型依赖「唯一 canonical 任务」判定（F5：`deviceModel.ts:239` 的 `findIndex`）。允许两个 canonical 任务会让全量替换语义变复杂（`deviceModel.ts:280-304`）。
3. 信封层让 Core 保持**协议无关**：Core 只校验 `{points:[{kind,point_key,parameters}]}` 的**结构**与 `kind` 非空，不涉及任何 Siemens 字段名，也不需要知道 Point Kind 的取值集合。

**备选方案（评审已否决，仅留痕）**：把 Point Kind 提到任务层（恰两个 kind、无信封），代价是禁止混合任务 + 前端需拆分装配。评审已裁决**不采用**（见 §8 已裁决-1）。

### 2.2 Point Kind `nck_variable`（语义变量）

```jsonc
{
  "kind": "nck_variable",                          // Point Kind
  "point_key": "channel.r_parameter#index=100@channel=1",   // 点位身份，Endpoint 内唯一，见 §4
  "parameters": {                                  // 对 Core 不透明；Driver 严格校验
    "variable_id": "channel.r_parameter",          // 必填，string，必须命中驱动定义目录
    "definition_revision": 1,                      // 必填，integer >= 1，必须能被驱动解析（§3）
    "target": { "channel": 1 },                    // 必填，object，恰好一个 Target 变体（§2.4.1）
    "arguments": { "index": 100 }                  // 必填，object（可为 {}），键必须属于该变量的 arguments 规格
  }
}
```

| 字段 | 类型 | 必填 | 取值 | Core 校验（**仅信封层**） | Driver 校验（**参数层，唯一负责人**） |
|---|---|---|---|---|---|
| `kind` | string | 是 | 非空字符串；语义取值 `nck_variable` | **仅判非空**（`EMPTY_POINT_KIND`；**无白名单**） | 取值必须为 `nck_variable`，否则 `UNSUPPORTED_POINT_KIND` |
| `point_key` | string | 是 | 规范编码（§4.1.1）、非空、Endpoint 内唯一 | 非空 + 规范编码/长度/NFC + 唯一（`DUPLICATE_POINT_KEY`，B9） | 不解释，仅回传 |
| `parameters` | object | 是 | 不透明 JSON 对象 | **只判是 object**（`INVALID_PARAMETERS`） | 反序列化到自己的强类型 |
| `parameters.variable_id` | string | 是 | `^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$` | **不校验**（Core 不知道此键存在） | 形状 + 目录命中，否则 `UNKNOWN_VARIABLE` |
| `parameters.definition_revision` | integer | 是 | `>= 1` | **不校验** | 解析不到 → `UNSUPPORTED_DEFINITION_REVISION`（**拒绝运行**，不退化） |
| `parameters.target` | object | 是 | 恰好一个 Target 变体（§2.4.1） | **不校验**（Core 不认识 Target 形状） | 变体数 ≠ 1 / 实例越界 / 不适用 → `INVALID_TARGET`/`UNSUPPORTED_TARGET`/`MISSING_TARGET` |
| `parameters.arguments` | object | 是 | 键 ∈ 该变量的 `arguments` 规格 | **不校验** | 未知/缺失/越界/类型错 → `INVALID_ARGUMENTS` |
| 信封层未知字段 | — | — | 不得出现 | `deny_unknown_fields` → `UNKNOWN_FIELD`（**不得静默丢弃**） | — |
| `parameters` 层未知字段 | — | — | 不得出现 | **不校验**（参数层不透明） | 未知键即拒 → `INVALID_BINDING_CONFIG`（§2.4.2 D2） |

**Core 禁止（修订 1，写进测试）**：不校验 `variable_id` 是否是已知变量、不校验 `definition_revision` 是否可解析、不校验 `target` 形态、不校验 `index` 范围、**不建立 `nck_variable`/`nck_native` 白名单**、不知道 `channel`/`module`/`entry` 之外任何 Siemens 语义（职责边界）。

### 2.3 Point Kind `nck_native`（高级原生地址）

```jsonc
{
  "kind": "nck_native",                          // Point Kind
  "point_key": "native.C.1.21.1.396",
  "parameters": {                                // 对 Core 不透明；Driver 严格校验
    "area": "C",                 // 必填 enum: N|B|C|A|T|V|H（M/MMC 仍拒绝，N11）
    "area_no": 1,                // 必填 integer 0..=31
    "module": 21,                // 必填 integer 0..=255
    "column": 1,                 // 必填 integer 0..=65535
    "line": 396,                 // 必填 integer 0..=65535
    "count": 1,                  // 必填 integer 1..=255
    "unit_mode": "current",      // 必填 enum: current|metric|inch
    "decode": "F64_LE",          // 必填 enum: 15 个规范名（N14）
    "transport_size": "0x09",    // 必填 enum: 0x05|0x06|0x07|0x09
    "invalid_value": -200.0,     // 可选 number（须与 invalid_tolerance 成对）
    "invalid_tolerance": 0.001,  // 可选 number 0..=1（须与 invalid_value 成对）
    "unit": "mm"                 // 可选 string，非空且 <= 32 字符
  }
}
```

**Core 侧**：**不校验上表任何字段**——`parameters` 对 Core 是**不透明对象**，Core **不知道** `area`/`module`/`decode` 这些键存在（修订 1）。Core 只保证「`parameters` 是 JSON 对象」且「信封层没有多余字段」。
**Driver 侧**：对 `parameters` 做**严格校验**（未知字段即拒、JSON 类型、整数范围、`area`/`unit_mode`/`decode`/`transport_size` 枚举、`invalid_value` 与 `invalid_tolerance` 成对且容差 `0..=1`、`unit` 非空且 ≤32 字符），然后**完全复用** `NativeNckAddress::from_parameters`（N7–N9、N13–N15）——这是**零新增**解析代码，也是「62 条 golden 物理含义不变」的技术保证。

**与「不允许前端构造原生协议字段」（决策 1）的关系**——精确定义：

- 禁止的是**前端派生/自动填充**原生字段：前端不得由 `variable_id` 反推 `module`/`column`/`transport_size`，不得对原生字段做默认值物化（`materializeSchemaDefaults` 对 `nck_native` 一律不适用）。
- `nck_native` 是**用户显式声明的高级逃生通道**：字段值由用户键入，前端只做**原样透传 + 显式警告**，不做任何转换。
- UI 必须把 `nck_native` 标为「高级：地址由你负责，语义与无效值保护不受保护」，且不得作为新建流程的默认路径。

### 2.4 Core 侧点位类型（`crates/core-types/src/point.rs`）：**信封层严格 + 参数层不透明**

**修订 1 的技术核心**：Core 既**不认识 Target 的形状**，又必须**阻止未知结构被静默丢弃**（B7 的既有缺陷）。解法是**分层**：

- **信封层严格**：`PointSelection` 使用 `deny_unknown_fields`，显式声明 `kind` / `point_key` / `parameters` **三个**字段；
- **参数层不透明**：`parameters` 的类型是 **JSON 对象**，Core **不解释**其中任何键；
- 于是 Core 能拒绝「信封层多了一个字段」（**不静默丢弃**），同时**永远不需要**知道 `target.channel`、`module`、`entry` 是什么。

```rust
/// Task Binding Kind（任务层；Core 按它路由）。
pub const NCK_POINTS_BINDING_KIND: &str = "nck.points.v1";

/// 点位信封（`config.points` 的元素）。
/// Core **只**认识这三个字段；`parameters` 对 Core 完全不透明。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]      // 关键：B7 的静默丢弃必须在此终止
pub struct PointSelection {
    pub kind: String,              // Point Kind：**只要求非空字符串**；Core 不建白名单（§2.1）
    pub point_key: String,         // 规范编码（§4.1.1）、非空、Endpoint 内唯一
    pub parameters: serde_json::Map<String, serde_json::Value>,  // 不透明 JSON 对象
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointBinding {
    pub points: Vec<PointSelection>,
}
```

**Core 不定义的类型（修订 1 的硬边界）**：`TargetRef`、`DriveObjectRef`、`DriveEntry`、`NativeAddress` 等 **Siemens 专用**类型**不得**出现在 `crates/core-types/**`（也不得出现在 `crates/core-api/**` 的校验逻辑里）；它们定义在 NCK Driver 侧（§2.4.1）。Core **不需要**知道 `DriveEntry` 与 NCK 地址的对应关系，**不应知道** Module **26**/**130**。

**Core 校验清单**（`validate_point_selection`，返回 `Vec<ValidationIssue>`；**只含信封层**）：

| 码 | 触发 | 说明 |
|---|---|---|
| `UNSUPPORTED_BINDING` | `DriverBinding.kind` 不在 `{mesa.resources.v1, nck.points.v1}` | **Task Binding Kind 路由**；与 Point Kind 无关 |
| `INVALID_STRUCTURE` | `points` 非数组 / 元素非 object / 信封缺字段 | 结构 |
| `UNKNOWN_FIELD` | **信封层** `deny_unknown_fields` 命中（复用 B5 码值，语义一致） | **不得静默丢弃** |
| `INVALID_PARAMETERS` | `parameters` 不是 JSON 对象（数组 / `null` / 标量） | 参数层的**唯一**约束 |
| `EMPTY_POINT_KIND` | `kind` 为空字符串 | 通用约束，**不是**白名单 |
| `EMPTY_POINT_KEY` | `point_key` 为空 | 通用约束 |
| `INVALID_POINT_KEY` | `point_key` 不符合规范编码 / 超过长度上限 / 未 NFC 归一 | §4.1.1（**PR-A 必须确定**） |
| `DUPLICATE_POINT_KEY` | Endpoint-wide 重复（复用 B9） | **跨 Task** 也唯一 |

**Core 明确不做（逐条写进测试）**：不校验 `variable_id` 是否存在、不校验 `definition_revision` 是否可解析、不校验 `target` 形态与实例范围、不校验任何整数范围、**不建立 `nck_variable`/`nck_native` 白名单**、不解释 `parameters` 内任何键。

**不改 `SchemaDescriptor`**（B6）：上述类型是**独立**于 Descriptor 的 Core 契约，不进入 `FieldType`，因此 Descriptor Major 保持 2。

#### 2.4.1 Siemens 专用 Target（**NCK Driver 侧**：`drivers/sinumerik-nck/src/targets.rs`）

以下类型**只属于 NCK Driver**，Core **不得**引用（修订 1）。Driver 负责其**语义校验**：变体数、实例范围、`entry` 必填与不可互换、变量是否适用该 Target。

```rust
/// 语义 target：恰好一个变体（**Driver 侧**类型；Core 不认识）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetRef {
    pub channel: Option<u32>,      // 通道号（area_no 来源）
    pub mode_group: Option<u32>,   // 方式组/BAG（`B/S/opMode` 的唯一归属，见下）
    pub axis: Option<u32>,         // 机床轴号
    pub spindle: Option<u32>,      // 主轴索引
    pub drive_object: Option<DriveObjectRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriveObjectRef {
    pub number: u32,               // 驱动访问编号（area_no 来源）
    pub entry: DriveEntry,         // 必填：入口身份，决定 module 26/130
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveEntry {
    NcVariable,     // module 26
    DriveObject,    // module 130
}
```

**`mode_group` 是对任务书 TargetRef 枚举的补充**（任务书写 `Channel`/`Axis`/`Spindle`/`DriveObject`）：冻结向量第 2 条是 `B/S/opMode`（`area=B`，G2/G6），语义索引明确 `B/S` 是 **BAG 实例**（G5）。若不引入 `ModeGroup`，该 golden 条目在新模型下**无归属**，直接违反「62 条必须建立对应关系」（决策 4）。评审已裁决 **ModeGroup 纳入 NCK Target 定义**。

**`DriveEntry` 必填、无默认值**，这是「module 26 与 130 必须作为入口身份且不可互换」（决策 5 / 指令 5）在类型层的强制：缺少 `entry` 即 `INVALID_TARGET`，**不存在**「默认 module 26」的隐式回退。理由有真机证据：同一 `V/n` 在两条入口返回**不同**对象名（N16、G8 `nck-remaining-20-reconciliation.md:57-74`）。**该裁决由 Driver 执行，Core 不参与。**

**Tool Target**：依据**实际变量需求**确定，**不为凑齐枚举提前增加**（评审裁决 3）。今天 `channel.tool.number_active` 用 `{channel:1}` 即可表达（`area=C`）；若将来出现 `T` area 的刀具变量再补。

#### 2.4.2 NCK Driver 对 `parameters` 的严格校验（参数层的唯一负责人）

Driver 把 `parameters` 反序列化到**自己的**强类型（`#[serde(deny_unknown_fields)]`），任何一项不通过即 `configure` **整体失败**、任务**不得启动**：

| # | 校验项 | 失败码 |
|---|---|---|
| D1 | **Point Kind** 取值 ∈ `{nck_variable, nck_native}` | `UNSUPPORTED_POINT_KIND` |
| D2 | **未知字段**（`parameters` 层） | `INVALID_BINDING_CONFIG` |
| D3 | **类型错误**（字符串 / 整数 / 对象 / 数组） | `INVALID_BINDING_CONFIG` |
| D4 | **索引范围**（`area_no` 0..=31、`module` 0..=255、`column`/`line` 0..=65535、`count` 1..=255、Target 实例号 0..=255） | `INVALID_VARIABLE`（原生）/ `INVALID_TARGET`（语义） |
| D5 | **`variable_id` 存在性**（定义目录命中） | `UNKNOWN_VARIABLE` |
| D6 | **`definition_revision` 可解析性**（**不得回落最新**，§3.3） | `UNSUPPORTED_DEFINITION_REVISION` |
| D7 | **`target` 形态与合法性**（变体数 = 1、变量适用性、`entry` 必填、module 26/130 不可互换、Tool 是否适用） | `INVALID_TARGET` / `UNSUPPORTED_TARGET` / `MISSING_TARGET` |
| D8 | **`arguments`** 键/类型/范围/`ArgumentOffset` 上溢 | `INVALID_ARGUMENTS` |
| D9 | **原生地址有效性**（`area`/`unit_mode`/`decode`/`transport_size` 枚举、无效值成对、容差 `0..=1`、`unit` ≤32 字符） | `INVALID_VARIABLE` |
| D10 | **Module 26/130 入口身份**（`entry` → module 映射，与 `identity_spec.rs` 常量一致，**不允许折叠**） | `INVALID_TARGET`（缺 `entry`）/ 构建期自检 |

### 2.5 Driver 侧 `VariableDefinition`（**全部为 NCK Driver 侧类型**，Core 不可见）

> 本节及 §2.6 的所有类型（含 `TargetRef`、`TargetKind`、`NativeFieldSource`）均属于 **NCK Driver**，**不得**被 `crates/core-types/**` 引用（修订 1）。

```rust
pub struct VariableDefinition {
    pub variable_id: String,                  // 业务语义 ID；稳定；不复用（§3.4）
    pub revision: u32,                        // 定义修订号；>=1；已发布不可改（§3.1）
    pub family: String,                       // 适用系列："828d"；"840d-sl" 暂无定义
    pub scope: DefinitionScope,               // Collect | Identity
    pub semantics_status: SemanticsStatus,    // Verified | Candidate | ParameterIdentity（§2.5.2）
    pub label: LocalizedText,                 // 展示名（不含协议字段）
    pub source: Vec<DefinitionSource>,        // 来源（手册/实测/身份探针/冻结向量）
    pub targets: Vec<TargetKind>,             // 适用 Target 类型（Channel|ModeGroup|Axis|Spindle|DriveObject）
    pub target_required: Vec<TargetKind>,     // 其中必须提供的（如 drive 变量必须给 DriveObject+entry）
    pub arguments: Vec<ArgumentSpec>,         // 用户需输入的参数
    pub addressing: AddressingRule,           // 原生寻址规则（唯一定义处）
    pub decode: NckDataKind,                  // 固定 decode（N14 规范名）
    pub transport_size: u8,                   // 固定期望响应 transport（0x05/06/07/09）
    pub count: u8,                            // 固定线缆 line_count（1..=255）
    pub unit_mode: NckUnitMode,               // 固定（全部定义均为 Current，G3）
    pub unit: Option<String>,                 // 展示单位（仅注释）
    pub invalid_rule: Option<InvalidRuleSpec>,// 无效值哨兵（如 r0035: -200/0.001，G7）
    pub digest: String,                       // 定义摘要，lowercase hex SHA-256（§3.2）
}

pub enum DefinitionScope { Collect, Identity }   // Identity 仅供身份探针，不出现在用户点位面

pub enum TargetKind { Channel, ModeGroup, Axis, Spindle, DriveObject }

pub struct ArgumentSpec {
    pub key: String,                          // 如 "index" / "position" / "level"
    pub label: LocalizedText,
    pub value_type: ArgType,                  // Integer | Enum
    pub required: bool,
    pub min: Option<i64>, pub max: Option<i64>,
    pub enum_options: Option<Vec<String>>,
    pub default: Option<serde_json::Value>,
}

/// 原生寻址规则：字段来源的**唯一**声明处。禁止在代码里散落 if。
pub struct AddressingRule {
    pub area: NckArea,                        // 固定
    pub area_no: NativeFieldSource,           // 实例号来源
    pub module: NativeFieldSource,            // 常量 或 入口→module（26/130）
    pub column: u16,                          // 固定
    pub line: NativeFieldSource,              // 行索引来源
}

pub enum NativeFieldSource {
    Fixed(u32),
    Target(TargetKind),                       // 取该 target 变体的实例号
    Argument(String),                         // 取 arguments.<key>
    ArgumentOffset(String, i32),              // 取 arguments.<key> + offset（R 参数 line = index + 1，G5）
    DriveEntryModule,                         // entry=nc_variable→26；entry=drive_object→130
}
```

#### 2.5.1 42 条定义与 62 条 Wire Golden 的**逐一**对应关系

`scope`：`C`=Collect，`I`=Identity。`semantics_status`：`V`=Verified，`Cand`=Candidate，`PI`=ParameterIdentity（§2.5.2）。
`area_no`/`line` 来源列给出 `NativeFieldSource`；全部 62 条 golden 的 `area_no=1, line=1`（G2），因此等价门取 `target.* = 1`、`arguments.* = 0`。

**A. 通道 / 方式组 / 轴 / 主轴 / R 参数（16 条，各覆盖 1 条 golden）**

| # | Variable ID | scope | sem | area | module | column | decode | transport | count | area_no 来源 | line 来源 | golden 条目 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `channel.program.status` | C | V | C | 127 | 13 | `U16_LE` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/S/progStatus` |
| 2 | `mode_group.op_mode` | C | V | B | 127 | 3 | `U16_LE` | `0x09` | 1 | `Target(ModeGroup)` | `Fixed(1)` | `B/S/opMode` |
| 3 | `channel.program.name.main` | C | V | C | 122 | 5 | `UTF8_NUL_LAST_32` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/SPARPP/progName` |
| 4 | `channel.program.name.active` | C | V | C | 125 | 3 | `UTF8_NUL_LAST_32` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/SPARP/progName` |
| 5 | `channel.program.line_number` | C | V | C | 125 | 9 | `I32_LE` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/SPARP/actLineNumber` |
| 6 | `channel.program.block` | C | V | C | 125 | 31 | `UTF8_NUL_FIRST_66` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/SPARP/actBlock` |
| 7 | `axis.position.commanded_mcs` | C | V | C | 115 | 2 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Target(Axis)` | `C/SGA/actToolBasePos` |
| 8 | `axis.position.actual_bcs` | C | V | C | 112 | 33 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Target(Axis)` | `C/SEGA/actProgPosBKS` |
| 9 | `channel.feed.actual` | C | V | C | 127 | 1 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/S/actFeedRateIpo` |
| 10 | `channel.feed.commanded` | C | V | C | 127 | 2 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/S/cmdFeedRateIpo` |
| 11 | `channel.feed.override` | C | V | C | 127 | 3 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/S/feedRateIpoOvr` |
| 12 | `spindle.speed.actual` | C | V | C | 114 | 2 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Target(Spindle)` | `C/SSP/actSpeed` |
| 13 | `spindle.speed.commanded` | C | V | C | 114 | 3 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Target(Spindle)` | `C/SSP/cmdSpeed` |
| 14 | `spindle.speed.override` | C | V | C | 114 | 4 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `Target(Spindle)` | `C/SSP/speedOvr` |
| 15 | `channel.tool.number_active` | C | V | C | 127 | 23 | `U16_LE` | `0x09` | 1 | `Target(Channel)` | `Fixed(1)` | `C/S/actTNumber` |
| 16 | `channel.r_parameter` | C | V | C | 21 | 1 | `F64_LE` | `0x09` | 1 | `Target(Channel)` | `ArgumentOffset("index", 1)` | `C/RP/rpa`（index=0 → line=1） |

**关键语义说明（不得含糊）**：

- **#7/#8 的 `axis.` 前缀与 `C` area 并存是刻意的**：`area_no` 取**通道号**、`line` 取**机床轴号**，两者共同决定物理地址（G5、`presets/README.md:41-42`、`EXAMPLES.md:86-122`）。这正是「语义 Target 与原生字段不是一对一」的典型：`TargetRef{channel, axis}` 映射到 `area_no=channel, line=axis`。禁止把 `axis` 当成 `area=A`。
- **#12–#14 主轴**（决策 5）：业务层是**独立类别** `spindle.*`，协议层**沿用已验证的 `C/SSP` 寻址**，`line` = 主轴索引（G5）。**不得**虚构 `S<n>` 之类未验证的原生区域——`NckArea` 只有 7 个字母，`M`/MMC 已明确拒绝（N11），本设计**不新增任何 area 字母**。
- 主轴索引**不得**由机床轴号直接推断：`A/M/SPIND_ASSIGN_TO_MACHAX`（#42）才是「轴 → 主轴编号」的已验证来源（G10）。
- **#16 R 参数**：`line = index + 1`（`EXAMPLES.md:124-141` 的 R425 → line 426）。`ArgumentOffset` 是唯一带偏移的寻址来源，偏移值写在定义里而非代码里。

**B. 驱动参数（20 条定义，各覆盖 2 条 golden = 40 条）**

公共列：`area=V`、`line=Fixed(1)`、`area_no=Target(DriveObject)`、`module=DriveEntryModule`（`entry=nc_variable`→**26**，`entry=drive_object`→**130**）、`unit_mode=Current`、`scope=C`。

| # | Variable ID | sem | column | decode | transport | count | invalid_rule | golden 条目（同 column，两条） |
|---|---|---|---|---|---|---|---|---|
| 17 | `drive.parameter.r0021` | PI | 21 | `F32_BE` | `0x07` | 1 | — | `V/M/r0021` × 2 |
| 18 | `drive.parameter.r0027` | PI | 27 | `F32_BE` | `0x07` | 1 | — | `V/M/r0027` × 2 |
| 19 | **`drive.motor.temperature`** | **V** | 35 | `F32_BE` | `0x07` | 1 | `-200.0 ± 0.001` | `V/M/r0035` × 2 |
| 20 | `drive.parameter.r0033` | Cand | 33 | `F32_BE` | `0x07` | 1 | — | `V/M/r0033` × 2 |
| 21 | `drive.parameter.r0031` | Cand | 31 | `F32_BE` | `0x07` | 1 | — | `V/M/r0031` × 2 |
| 22 | `drive.parameter.r0062` | PI | 62 | `F32_BE` | `0x07` | 1 | — | `V/M/r0062` × 2 |
| 23 | `drive.parameter.r0063` | PI | 63 | `F32_BE` | `0x07` | 1 | — | `V/M/r0063` × 2 |
| 24 | `drive.parameter.r0064` | PI | 64 | `F32_BE` | `0x07` | 1 | — | `V/M/r0064` × 2 |
| 25 | `drive.parameter.r0077` | PI | 77 | `F32_BE` | `0x07` | 1 | — | `V/M/r0077` × 2 |
| 26 | `drive.parameter.r0078` | PI | 78 | `F32_BE` | `0x07` | 1 | — | `V/M/r0078` × 2 |
| 27 | `drive.parameter.r0079` | PI | 79 | `F32_BE` | `0x07` | 1 | — | `V/M/r0079` × 2 |
| 28 | `drive.parameter.r0080` | PI | 80 | `F32_BE` | `0x07` | 1 | — | `V/M/r0080` × 2 |
| 29 | `drive.parameter.r0081` | PI | 81 | `F32_BE` | `0x07` | 1 | — | `V/M/r0081` × 2 |
| 30 | `drive.parameter.r0082` | PI | 82 | `F32_BE` | `0x07` | 1 | — | `V/M/r0082` × 2 |
| 31 | `drive.parameter.r0103` | PI | 103 | `U16_BE` | `0x06` | 1 | — | `V/M/r0103` × 2 |
| 32 | `drive.parameter.p0105` | PI | 105 | `I16_BE` | `0x05` | 1 | — | `V/M/p0105` × 2 |
| 33 | `drive.parameter.r0106` | PI | 106 | `I16_BE` | `0x05` | 1 | — | `V/M/r0106` × 2 |
| 34 | `drive.parameter.r0107` | PI | 107 | `I16_BE` | `0x05` | 1 | — | `V/M/r0107` × 2 |
| 35 | `drive.parameter.r0108` | PI | 108 | `U32_BE` | `0x06` | 1 | — | `V/M/r0108` × 2 |
| 36 | `drive.object.name` | V | 199 | `UTF16_BE` | `0x06` | **25** | — | `V/M/p0199` × 2 |

**#19 的命名与证据**：`drive.motor.temperature` 由任务书指定，且仓库有可追溯依据（G7：SINAMICS S120/S150 参数手册 11/2023 p.71 + `presets/README.md:83-88` 的 `-200` 哨兵）。**注意其 target 语义边界**：`TargetRef{drive_object:{number, entry}}` 只表达「哪个驱动访问编号、哪条入口」，**不表达**「哪个电机」——`V/n` 的轴归属在只读途径下**未证明**（G8）。因此 `drive.motor.temperature` 的语义是「该驱动对象的电机温度」，**不是**「X 轴电机温度」。

**#17–#36 的命名规则（§2.5.2 的强制后果）**：除 #19 外，其余 19 条在仓库内**没有任何**可追溯的工程语义证据（`PROVENANCE.md:9-54` 的「语义限制」列对这些参数为「—」；`README.md:11-12` 明确「保留原导入名称不代表其物理含义已验证」）。因此一律使用**参数身份 ID** `drive.parameter.<rNNNN>`，**不得**编造 `drive.motor.current...` 之类名称。可用的候选标注（来自 `docs/evidence/nck-remaining-20-reconciliation.md`：`r0021`=滤波转速实际值、`r0027`=滤波电流实际值、`r0031`=滤波转矩实际值、`r0033`=轴负载、`r0078`=转矩产生电流、`r0080`=未滤波）只能作为 `label` 的**候选**文案，且必须带 `candidate` 标记，**不得**进入 `variable_id`。

**C. 身份探针变量（6 条，各覆盖 1 条 golden，`scope=I`，不出现在用户点位面）**

| # | Variable ID | scope | sem | area | module | column | decode | transport | area_no 来源 | line 来源 | golden 条目 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 37 | `axis.name` | I | V | N | 26 | 10000 | `UTF8_FIXED_16` | `0x09` | `Fixed(1)` | `Target(Axis)` | `N/M/MDSA_AXCONF_MACHAX_NAME_TAB` |
| 38 | `axis.ctrlout.module_number` | I | V | A | 26 | 30110 | `U8` | `0x09` | `Target(Axis)` | `Fixed(1)` | `A/M/MDCA_CTRLOUT_MODULE_NR` |
| 39 | `axis.encoder.module_number` | I | V | A | 26 | 30220 | `U8` | `0x09` | `Target(Axis)` | `Fixed(1)` | `A/M/MDCA_ENC_MODULE_NR` |
| 40 | `axis.ctrlout.type` | I | V | A | 26 | 30130 | `U8` | `0x09` | `Target(Axis)` | `Fixed(1)` | `A/M/MDCA_CTRLOUT_TYPE` |
| 41 | `channel.machine_axis_assignment` | I | V | C | 26 | 20070 | `U8` | `0x09` | `Target(Channel)` | `Argument("position")` | `C/M/AXCONF_MACHAX_USED` |
| 42 | `axis.spindle_assignment` | I | V | A | 26 | 35000 | `U8` | `0x09` | `Target(Axis)` | `Fixed(1)` | `A/M/SPIND_ASSIGN_TO_MACHAX` |

**#37–#42 与 `identity_spec.rs` 的关系**：这 6 条的数值与 `identity_spec.rs:92-204` 的探针常量**逐字段一致**（本次核对）。本设计**不重构** `identity_spec.rs`（1.3 冻结），只在定义目录中**引用同值**并注明来源为 `identity_spec.rs:<行号>`。二者一致性由 `tests/definition_catalog.rs` 断言（防止将来两处漂移）。

**计数校验**：A 16 + B 20 + C 6 = **42 条定义**；A 16 + B 40 + C 6 = **62 条 golden**；B 的 40 条中 `entry=nc_variable` 20 条 + `entry=drive_object` 20 条；故 `nc_variable` 入口共 42 条、`drive_object` 入口共 20 条——与 G1 的 `42(NC变量) + 20(驱动对象)` 完全吻合。

#### 2.5.2 `semantics_status` 判定规则（防语义虚构）

| 值 | 含义 | 允许的 ID 形态 | UI 义务 |
|---|---|---|---|
| `verified` | 仓库内可追溯证据（`README`/`EXAMPLES`/`PROVENANCE`/手册页码/真机实测） | 业务语义 ID | 正常展示；单位仍标注「依赖控制器设置」（`README.md:59`） |
| `candidate` | 语义仅来自导入点位标签，可靠性已在 `nck-remaining-20-reconciliation.md:100-104` 登记为风险 | 业务语义 ID + `candidate` 标记 | **必须**显示「候选」且不得展示为已确认工程量 |
| `parameter_identity` | 仓库内**无任何**语义证据，仅有参数编号 | **必须**为 `drive.parameter.<rNNNN>` | 必须显示「仅参数身份，非工程语义」 |

`semantics_status` 是**定义的一部分**，因此进入 digest（§3.2）：状态升级（如 `candidate → verified`）**必须**发布新 revision。

### 2.6 Address Compiler 的输入 / 输出 / 错误

```rust
/// 唯一协议地址转换入口（决策 1）。
pub struct AddressCompiler<'a> {
    catalog: &'a DefinitionCatalog,
    family: &'a str,
}

pub enum CompileInput<'a> {
    Semantic {
        variable_id: &'a str,
        definition_revision: u32,
        target: &'a TargetRef,                        // Driver 侧类型（§2.4.1）
        arguments: &'a serde_json::Map<String, serde_json::Value>,
    },
    Native(&'a serde_json::Map<String, serde_json::Value>),
}

pub struct CompiledPoint {
    pub address: NativeNckAddress,        // 复用现有类型（N8）
    pub resolved: ResolvedPoint,          // 复用现有类型（N8）
    pub source_label: String,             // 复用 native.rs:347-361 的稳定格式（N10）
    pub definition: Option<DefinitionRef> // Semantic 时给出 (variable_id, revision, digest)
}
```

**契约**：

1. `Semantic` 分支**唯一**合法路径是 `catalog.lookup(variable_id, revision)` → 校验 target/arguments → 按 `addressing` 组装 12 字段 → **调用既有的** `NativeNckAddress::from_parameters` → `resolve_native`。**不新增第二套字节组装**。
2. `Native` 分支直接走 `NativeNckAddress::from_parameters`，与今天**逐字相同**（N7–N9）。
3. `CompileInput::Native` 不得被 `Semantic` 路径内部调用（禁止「语义失败就退化到原生」）。
4. Compiler **不读连接配置**、**不发网络请求**、**不查目录文件**（决策 3：定义随驱动内嵌）。

**错误类型**（`CompileError`，映射到 `SdkDriverError`，N16 风格的 `ErrorKind` + 稳定 code）：

| code | ErrorKind | 触发 | 是否 fail-closed |
|---|---|---|---|
| `UNSUPPORTED_POINT_KIND` | `Unsupported` | **Point Kind** 非 `nck_variable`/`nck_native`（**修订 1：该校验从 Core 移到 Driver**） | 是 |
| `UNKNOWN_VARIABLE` | `Address` | `variable_id` 不在目录 | 是 |
| `UNSUPPORTED_DEFINITION_REVISION` | `Configuration` | 目录中无该 `revision` | 是（**明确拒绝运行**，§3.3） |
| `UNSUPPORTED_FAMILY` | `Unsupported` | 变量 `family` 与设备 family 不匹配 | 是 |
| `UNSUPPORTED_TARGET` | `Configuration` | 变量不支持该 target 类型 | 是 |
| `MISSING_TARGET` | `Configuration` | 缺少 `target_required` 中的类型 | 是 |
| `INVALID_TARGET` | `Configuration` | 实例号越界 / `entry` 缺失 / target 变体数 ≠ 1 | 是 |
| `INVALID_ARGUMENTS` | `Configuration` | 未知/缺失/越界/类型错的 argument | 是 |
| `DEFINITION_DIGEST_MISMATCH` | `Internal` | 内嵌定义自检摘要不符（构建期问题） | 是 |
| `INVALID_VARIABLE` | `Address` | 复用：`NativeNckAddress::from_parameters` 失败（N7–N9） | 是 |
| `INVALID_BINDING_CONFIG` | `Configuration` | 信封/点位结构错、`parameters` 层未知字段或类型错（§2.4.2 D2/D3） | 是 |
| `UNSUPPORTED_BINDING` | `Unsupported` | **Task Binding Kind** 非 `nck.points.v1` | 是 |

`INVALID_VARIABLE` 与 `UNSUPPORTED_BINDING` 的 code 与今天**完全一致**（`drivers/sinumerik-nck/src/lib.rs:615`、`:673`、`:681`），保证既有错误面不漂移。

### 2.7 错误码总表（Core / Driver 分工）

| 层 | code | 现状 | 本次 |
|---|---|---|---|
| Core | `UNSUPPORTED_BINDING` | `crates/core-api/src/lib.rs:244`、`bootstrap.rs:228` | 复用；触发条件改为「**Task Binding Kind** 不在 {`mesa.resources.v1`, `nck.points.v1`}」 |
| Core | `INVALID_STRUCTURE` | `crates/core-types/src/resource.rs:265` | 复用（信封结构） |
| Core | `UNKNOWN_FIELD` | `crates/core-types/src/schema.rs:410` | 复用（**信封层** `deny_unknown_fields` 命中） |
| Core | `DUPLICATE_POINT_KEY` | `crates/core-types/src/resource.rs:352` | 复用（Endpoint-wide 唯一，**跨 Task**，B9） |
| Core | `INVALID_PARAMETERS` / `EMPTY_POINT_KIND` / `EMPTY_POINT_KEY` / `INVALID_POINT_KEY` | 不存在 | **新增**（**信封层专属**；`INVALID_POINT_KEY` 见 §4.1.1） |
| Core | ~~`INVALID_TARGET`~~ | 不存在 | **不新增**：Core **不认识 Target**（修订 1 已把该校验移出 Core） |
| Driver | `UNSUPPORTED_POINT_KIND` | 不存在 | **新增**（**Point Kind** 取值校验；替代初稿放在 Core 的白名单） |
| Driver | `UNKNOWN_VARIABLE` / `UNSUPPORTED_DEFINITION_REVISION` / `UNSUPPORTED_TARGET` / `MISSING_TARGET` / `INVALID_TARGET` / `INVALID_ARGUMENTS` | 不存在 | **新增**（**参数层语义专属**，§2.4.2） |
| Driver | `INVALID_VARIABLE` / `INVALID_BINDING_CONFIG` / `UNSUPPORTED_BINDING` | `lib.rs:673`、`:681`、`:636`、`:614` | 保留原 code 与语义 |
| Driver | `UNSUPPORTED_RESOURCE` / `UNSUPPORTED_OUTPUT` | `lib.rs:646`、`:655` | NCK 侧**不再产生**（§1.4）；常量保留供其他驱动/SDK |

### 2.8 Core / Driver 校验责任表（逐项，**谁校验什么**）

口径：**Core 负责阻止未知结构被静默丢弃；Driver 负责判断 Target 是否合法**（修订 1）。「Core」列写「—」表示 Core **必须不校验**（不是「暂不实现」）。

| # | 校验项 | Core（`core-types` / `core-api`） | NCK Driver（`sinumerik-nck`） |
|---|---|---|---|
| 1 | **Task Binding Kind 路由**（`DriverBinding.kind`） | ✅ 判定 ∈ `{mesa.resources.v1, nck.points.v1}`；否则 `UNSUPPORTED_BINDING` | ✅ 复核 `nck.points.v1`，否则 `UNSUPPORTED_BINDING` |
| 2 | **信封结构**（`config.points` 是数组、元素是 object、三字段齐备） | ✅ `INVALID_STRUCTURE` | ✅ 反序列化复核 |
| 3 | **`parameters` 必须是 JSON 对象** | ✅ `INVALID_PARAMETERS` | ✅ 反序列化到强类型 |
| 4 | **`kind` 非空** | ✅ 仅非空字符串（`EMPTY_POINT_KIND`） | ✅ 取值必须 ∈ `{nck_variable, nck_native}` |
| 5 | **Point Kind 语义** | ❌ **不建立白名单**（不枚举取值） | ✅ `UNSUPPORTED_POINT_KIND` |
| 6 | **信封层未知字段拒绝** | ✅ `deny_unknown_fields` → `UNKNOWN_FIELD` | — |
| 7 | **`parameters` 层未知字段拒绝** | ❌ 不解释（参数层不透明） | ✅ `INVALID_BINDING_CONFIG` |
| 8 | **`point_key` 非空** | ✅ `EMPTY_POINT_KEY` | — |
| 9 | **`point_key` 规范编码 / NFC / 长度上限 256 字节** | ✅ `INVALID_POINT_KEY`（§4.1.1，PR-A 必须确定） | — |
| 10 | **`point_key` Endpoint 内唯一（跨 Task）** | ✅ `DUPLICATE_POINT_KEY`（B9/R5） | ❌ 不裁决身份 |
| 11 | **`variable_id` 存在性**（目录命中） | ❌ 不知道变量名清单 | ✅ `UNKNOWN_VARIABLE` |
| 12 | **`definition_revision` 可解析性** | ❌ 不解析 revision（§3.3） | ✅ `UNSUPPORTED_DEFINITION_REVISION`，**不得回落最新** |
| 13 | **`target` 形态与合法性**（变体数 = 1、变量适用性、实例范围、`entry` 必填） | ❌ **不认识 Target 形状**（`TargetRef` 属 Driver，§2.4.1） | ✅ `INVALID_TARGET`/`UNSUPPORTED_TARGET`/`MISSING_TARGET` |
| 14 | **`arguments` 键/类型/范围** | ❌ 不知道 `arguments` 规格 | ✅ `INVALID_ARGUMENTS` |
| 15 | **索引范围**（原生整数范围、Target 实例 0..=255、`ArgumentOffset` 上溢） | ❌ 不校验任何整数范围 | ✅ `INVALID_VARIABLE`/`INVALID_TARGET`/`INVALID_ARGUMENTS` |
| 16 | **原生地址有效性**（枚举、无效值成对、容差、`unit` 长度） | ❌ 不知道 `area`/`module` 等键存在 | ✅ `INVALID_VARIABLE`（复用 N7–N9） |
| 17 | **Module 26/130 入口身份**（`entry` → module，**不允许折叠**） | ❌ **不应知道 Module 26/130** | ✅ `INVALID_TARGET`（缺 `entry`）+ `identity_spec.rs` 常量对齐（N16/G8） |
| 18 | **`display_name` / `label` 等展示文案** | ❌ 不参与身份（R6/R11） | ❌ 不参与身份与寻址（§4.1.2） |

**共 18 项**。其中 Core 独立负责 8 项（#1–#4、#6、#8–#10），Driver 独立负责 8 项（#5、#7、#11–#17 中除 #1–#4 复核外的语义项），展示文案 1 项（#18）双方均不参与。

---

## 3. Definition Revision 策略

> **硬约束（评审要求，原文保留）**：**已经发布的定义不可原地修改**；**旧 Revision 缺失时必须拒绝运行，绝不能自动使用最新版本替代**。
>
> 这两条是本节所有条款的上位约束：前者由 §3.1（不可变性）+ §3.2（摘要冻结）强制；后者由 §3.3（三种禁止退化）强制。

### 3.1 不可变性

- `revision` 从 **1** 开始，单调递增，**不跳号、不复用**。
- **已发布的 (variable_id, revision) 定义不可原地修改**（评审硬约束）。任何字段变化（含 `semantics_status`、`label`、`source`、`unit`）都必须发布**新 revision**，旧 revision 原样保留在目录中。**原地修改**（含「只改一个 label」）一律视为违规，由 §3.2 的摘要清单在 CI 拦截。
- 驱动**同时携带**同一 `variable_id` 的多个 revision（历史点位引用的 revision 必须仍可解析，否则老配置会在驱动升级后无法运行）。
- 实施约束：定义目录数据文件按 `revision` 分文件存放（`definitions/828d/r1.json`、`r2.json`…），装载时合并；CI 断言「已存在的 (variable_id, revision) 文件内容不得被修改」——通过**摘要清单**（`definitions/828d/DIGESTS.lock`）冻结，改动即测试失败。

### 3.2 摘要算法与稳定性要求

- 算法：`lowercase hex SHA-256(canonical JSON)`，与仓库既有先例**同构**（N20：`crates/event-store/src/lib.rs:235-249`），并沿用其纪律「**不依赖派生 Serialize 的偶然形状**」。
- canonical 化规则（逐条固定，不得依赖 `serde` 默认）：
  1. 顶层键按**字节序升序**排序（复用 `event-store` 的 `sort_json` 思路，`crates/event-store/src/lib.rs:228-233`）；
  2. 数组**保序**，不排序（`arguments`、`source`、`targets` 的顺序是定义的一部分）；
  3. `digest` 字段本身**不参与**计算；
  4. 浮点用最短往返表示（`invalid_value=-200.0` 编码为 `-200.0`），非有限值直接报错（同 `event-store` 的 `finite_json` 纪律，`crates/event-store/src/lib.rs:149-152`）；
  5. 缺省可选字段**不出现**在 canonical 中（不得写成 `null`），否则「显式 null」与「缺省」会产生两个摘要。
- 依赖：NCK 驱动需新增 `sha2 = { workspace = true }`（N19；workspace 已有 `sha2 = "0.10"`，`Cargo.toml:53`）。
- **稳定性要求**：摘要必须只依赖**定义语义**，不得依赖编译时间、文件路径、驱动版本号。因此：
  - 驱动版本号**不进**定义摘要；
  - `source` 中的文件路径使用**仓库相对路径**且不含 commit hash（commit 记在 `DIGESTS.lock` 之外）；
  - 摘要变更即为「定义语义变更」的**唯一**判据。

### 3.3 解析不到 revision 时的明确拒绝

- 触发：点位引用的 `(variable_id, definition_revision)` 在驱动目录中**不存在**。
- 行为：`AddressCompiler` 返回 `UNSUPPORTED_DEFINITION_REVISION`（`ErrorKind::Configuration`），configure **整体失败**，任务**不得启动**。
- 消息必须含三要素：`variable_id`、请求的 `revision`、该变量目录中**可用的 revision 列表**（便于运维判断是「驱动降级」还是「配置被改」）。
- **禁止**的三种退化（逐条写进测试）：
  1. 不得回落到该变量的**最新** revision；
  2. 不得回落到**任何**其他 revision；
  3. 不得回落到「按 `variable_id` 猜原生地址」。
- 该错误在**保存门禁**的暴露方式（评审裁决 10）：Core **不解释** `parameters`（修订 1），因此 Core 的保存门禁**只做信封层校验**，**不解析 `definition_revision`**；`definition_revision` 的**最终校验责任在 Driver**。为缩短反馈环，`nck.points.v1` 的保存路径**可以**在 descriptor 可用时调用驱动的一次只读「校验」路径（复用既有 `configure` 前置校验语义），但**必须**满足三条：① 校验失败时保存被拒且错误码**原样透传**；② 驱动不可达时**不得**把保存放行为「已校验」，只能标记「**未校验**」；③ Core **不得**因此内置任何变量知识或 revision 知识。**保存反馈机制的具体形态在 PR-A 明确**（评审裁决 10，§8 已裁决-10）。

### 3.4 Wire Golden 与 revision 的绑定方式

- 62 条冻结向量（`legacy-828d-wire.json`）**绑定的是「物理地址」**，不是「某个 revision」。因此：
  - `tests/legacy_equivalence.rs`（模板路径）**原样保留**（1.3），继续断言 62 条字节；
  - 新增 `tests/semantic_equivalence.rs`：对 42 条定义的**每一个 revision**，用 §2.5.1 的对应关系构造 `nck_variable` 载荷，编译并断言 `encode_var_spec` 与冻结 `wire_hex` **逐字节相等**；
  - 断言 `checked == 62`（不得跳过），并断言 `entry=drive_object` 的 20 条与同参数 `entry=nc_variable` 条目 **module 不同、wire 不同**（G4）。
- 新增 revision 时的强制义务：若新 revision 的 `addressing` 与该变量的**既有 golden 条目**不一致，则测试必须**显式**在 `legacy-828d-wire.json` 之外新增向量并**说明理由**；不允许静默改 golden。**例外**：新 revision 若只修正非 golden 覆盖的字段（如 `label`/`unit`/`semantics_status`），字节不变，测试自然通过。

### 3.5 何时用新 Variable ID 而非新 revision

| 变化 | 处置 | 理由 |
|---|---|---|
| 物理含义实质变化（同一 `column` 实际读的是**另一个物理量**；同一语义改为读**另一地址区域**） | **新 Variable ID** | 旧点位若静默跟随会读到别的物理量（ADR 0004:76-77 的既有风险口径） |
| 仅寻址定义修正（column/line 来源修正、decode 修正、transport 修正，且**物理量不变**） | **新 revision** | 同一语义的新旧点位可并存，旧点位继续按旧 revision 运行 |
| 仅 `label`/`unit`/`source`/`semantics_status` 变化 | **新 revision** | 不影响字节；但 `semantics_status` 变化必须留痕（§2.5.2） |
| 仅 `arguments` 增加可选项（不改变已有取值的寻址结果） | **新 revision** | 向后兼容 |
| `arguments` 语义变化（同一 `index` 指向不同物理量） | **新 Variable ID** | 等价于物理含义变化 |
| `TargetRef` 结构变化导致同一 target 映射到不同 `area_no`/`line` | **新 Variable ID** | 旧点位的 target 会被重新解释 |
| **重新命名**（`variable_id` 改名但字节不变） | **新 Variable ID + 显式别名记录**，且**必须**在 `DIGESTS.lock` 与映射表中逐一说明 | 「重新命名不代表重新解释已验证的请求字节」——62 条对应关系必须可追溯 |

---

## 4. 点位身份规则

### 4.1 `point_key`：规范编码的默认身份（语义名 vs 现状 `variable.value`）

**现状**：`point_key` = `<resource_id>.<output_id>` = **`variable.value`**（F6：`ResourcePickerAntd.tsx:41-51`、`:146`），冲突时 `.2/.3` 递增。所有 NCK 点位的默认 key 都是 `variable.value`，**不含任何点位身份信息**——这是今天最弱的一环。

**评审批准的基本方向（保留不变）**：同一语义变量 + Target + Arguments → 同一默认身份；Revision 变化 → **不**自动产生新 Point ID；Target 或 Arguments 变化 → 产生新的默认身份；Native 与 Semantic **不能互相顶替**。

在此基础上，本次补充两项（修订 3）：**4.1.1 Canonical Key 必须无歧义**、**4.1.2 Point Key 不是物理地址**。

#### 4.1.1 Canonical Key 的语法与规范编码（修订 3-其一）

**目标**：**禁止不同输入生成同一个 Key**。做法是把「结构分隔符」与「值」严格分离——分隔符只由模板产出，**值一律规范编码**。

**三条不变量（保留）**：

| 不变量 | 内容 | 理由 |
|---|---|---|
| **I1 含 target** | target 变体必须出现（如 `@channel=1`、`@axis=3`） | 同一变量的不同实例必须是不同点位 |
| **I2 含 arguments** | 所有**显式提供**的 argument 按键名升序出现（如 `#index=100`） | 同一变量的不同索引必须是不同点位 |
| **I3 不含 `definition_revision`** | revision **绝不**进入 `point_key` | revision 修正不得改变点位身份（§3.5「仅寻址定义修正 → 新 revision」必须保住 point_id） |

**语法**：

```text
point_key    := semantic_key | native_key
semantic_key := variable_id [ "#" arglist ] [ "@" targetlist ]
arglist      := arg ("," arg)*        arg := arg_key "=" enc(value)
targetlist   := tgt ("," tgt)*        tgt := target_key "=" enc(value)
native_key   := "native." area "." dec(area_no) "." dec(module) "." dec(column) "." dec(line)
```

**编码函数 `enc`（逐步固定，不得依赖任何库的默认行为）**：

1. **Unicode 规范化**：先对值做 **NFC**；规范化失败 → 拒绝该点位（`INVALID_POINT_KEY`），**不得**原样透传。
2. **逐字节百分号编码**：对 NFC 后的 **UTF-8 字节**逐个处理——
   - 字节 ∈ 未保留集 `A-Z a-z 0-9 - . _ ~`（RFC 3986 unreserved）→ **原样输出**；
   - 否则输出 `%` + **两位大写十六进制**（`,` → `%2C`、`#` → `%23`、`=` → `%3D`、`@` → `%40`、`/` → `%2F`、`%` → `%25`、`轴` → `%E8%BD%B4`）。
   - **`%` 本身必须被编码**（第 2 步对原始字节一次性完成，不允许「先转义非 ASCII、再补 `%`」的两遍做法，否则会与字面 `%XX` 文本碰撞，见 E2）。
3. **键名（`arg_key`/`target_key`）不编码**：它们来自定义目录，必须匹配 `^[a-z][a-z0-9_]{0,63}$`；不匹配 → `INVALID_ARGUMENTS`/`INVALID_TARGET`（**绝不**静默编码）。
4. **排序**：`arglist` 与 `targetlist` **各自**按**键名字节序升序**排列（同一语义载荷只有一种 Key）。
5. **不物化缺省**：只有**显式提供**的 argument 进入 Key；缺省值**不得**物化（否则「前端是否填默认值」会改变身份）。
6. **空字符串是显式值**：`{"index":""}` → `#index=`，与「不提供 `index`」（无 `#` 段）是**两个不同 Key**。
7. **`native_key` 的数值段**用**十进制 ASCII**（无前导零）；`area` 用 `NckArea` 的规范字母（N11 的 7 个之一）。
8. **长度上限**：`point_key` 的**规范编码后** UTF-8 字节数 **≤ 256**。依据：① 全部 42 条定义的默认 Key 实测最长 **≤ 64 字节**（如 `drive.motor.temperature@drive_object=130,entry=nc_variable` = 59 字节），256 字节留 ≥4 倍余量；② `point_key` 是 SQLite 索引列（`config-store` 的 Endpoint 内唯一约束），超长键恶化 B-tree 扇出与索引记录大小，而唯一性**只在 Endpoint 内**，不需要跨 Endpoint 前缀；③ 256 字节保证 Key 在日志、表格、UI 中单行可完整展示（可运维性）。超限 → `INVALID_POINT_KEY`，**不得截断**（截断会制造碰撞）。
9. **可解析性前提**：`variable_id` 的字符集（`^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$`）不含 `#`/`@`/`,`/`=`；值经 `enc` 后也不含这些字符；因此**首个 `#`/`@` 的位置无歧义**，Key 可被唯一反解。`native` 是保留前缀（`variable_id` 必须含 `.`，故不可能等于 `native`）。
10. **实现归属**：`enc` + 模板拼装 + 排序 + 长度/NFC 校验是 **Core 侧纯函数**（`crates/core-types/src/point.rs`），前端 `nckPointModel.ts` **复用同一规则**（PR-A 冻结，PR-C 消费）；**Driver 不重算 Key**（只回传）。

**歧义示例（有风险的输入 → 规范编码后必得不同 Key）**：

| # | 风险类别 | 输入 A | 输入 B | 不做规范编码（错误） | 规范编码后（正确） |
|---|---|---|---|---|---|
| **E1** | 值里含**分隔符** | `channel.r_parameter`，args `{index:"1,position=2"}`，target `{channel:1}` | 同变量，args `{index:"1",position:"2"}`，target `{channel:1}` | 两者都是 `… #index=1,position=2@channel=1` ⇒ **碰撞** | A：`channel.r_parameter#index=1%2Cposition%3D2@channel=1`；B：`channel.r_parameter#index=1,position=2@channel=1` |
| **E2** | 值里含**字面 `%`**（与百分号编码互撞） | args `{name:"轴"}` | args `{name:"%E8%BD%B4"}` | 若只转义非 ASCII 而不转义 `%`：两者都是 `#name=%E8%BD%B4` ⇒ **碰撞** | A：`#name=%E8%BD%B4`；B：`#name=%25E8%25BD%25B4` |
| **E3** | **非 ASCII / Unicode 规范化** | args `{name:"é"}`（U+00E9，NFC） | args `{name:"é"}`（`e`+U+0301，NFD） | 字节不同 ⇒ 同一**逻辑文本**得到**两个** Key（身份不稳定） | 两者 NFC 后同一码点 ⇒ 同一 Key `#name=%C3%A9`；而 `%C3%A9`（NFC 后）与 `e%CC%81`（若不做 NFC）**必不相同** |
| **E4** | **空值 / 缺省参数** | args `{index:""}`（显式空串） | args `{}`（未提供，即使定义有 `default:0`） | 若把空串当「未提供」而省略整段（或物化缺省为 `#index=0`）⇒ **碰撞/身份漂移** | A：`channel.r_parameter#index=@channel=1`；B：`channel.r_parameter@channel=1` |
| **E5** | 值里含 **`@`/`#`（可解析性歧义）** | args `{note:"x@channel=2"}`，target `{channel:1}` | args `{note:"x"}`，target `{channel:2}` | A 变成 `#note=x@channel=2@channel=1`：解析器**无法判断**第一个 `@` 属于值还是 target 段 | A：`#note=x%40channel%3D2@channel=1`（值内的 `@`/`=` 全部编码）；B：`#note=x@channel=2`。两者可唯一反解 |

**默认 Key 示例**（**默认值**，用户可改名；Endpoint 内唯一）：

| 载荷 | 默认 `point_key` |
|---|---|
| `channel.r_parameter`, target `{channel:1}`, args `{index:100}` | `channel.r_parameter#index=100@channel=1` |
| `channel.program.status`, target `{channel:1}`, args `{}` | `channel.program.status@channel=1` |
| `axis.position.commanded_mcs`, target `{channel:1, axis:3}` | `axis.position.commanded_mcs@axis=3,channel=1` |
| `spindle.speed.actual`, target `{channel:1, spindle:1}` | `spindle.speed.actual@channel=1,spindle=1` |
| `drive.motor.temperature`, target `{drive_object:{number:3, entry:"nc_variable"}}` | `drive.motor.temperature@drive_object=3,entry=nc_variable` |
| `nck_native`, params `area=C,area_no=1,module=21,column=1,line=396` | `native.C.1.21.1.396` |
| `nck_native`, 同上地址但**逻辑用途不同**（§4.1.2） | `native.C.1.21.1.396~2`（默认建议值，用户可改） |

**排序规则**：target 与 arguments 的键**一律按键名字节序升序**，保证同一语义载荷只有一种 `point_key`（否则前端两次装配会产生两个身份 → 两个 point_id，直接触发 `DUPLICATE_POINT_KEY` 的漏判）。

#### 4.1.2 Point Key **不是物理地址**（修订 3-其二）

- **同一个原生地址可能被用户用于不同的逻辑采集用途**（例如同一 `V/M/r0035` 既用于「电机温度趋势」又用于「报警阈值监视」）。因此 **Native Point 不强制以地址本身作为唯一身份**。
- 处置（四条，逐条写进测试）：
  1. **Native 的默认 Key 只是建议值**（§4.1.1 的 `native.<area>.<area_no>.<module>.<column>.<line>`）。当默认值已被同一 Endpoint 的另一点位占用时，前端建议追加 `~<n>`（`n≥2`，如 `native.C.1.21.1.396~2`）。`~` 属未保留字符但**不出现**在原生字段的规范取值中（`area` 是字母、其余是十进制数字），故 `~<n>` 段可被唯一识别与剥离。
  2. **用户可修改 `point_key`**（以及纯展示用的 `display_name`）；修改后的 `point_key` 即该点位的**稳定逻辑身份**。
  3. **禁止自动递增后缀**：旧行为 `variable.value` → `variable.value.2`/`.3`（F6）**必须废弃**。语义点位的默认 Key 由规范算法**唯一确定**；若两个点位的语义身份完全相同，则它们是**同一个点位**，必须报 `DUPLICATE_POINT_KEY`，**不得**用后缀「造出」第二个身份（否则同一逻辑点产生两个 `point_id`，历史数据被劈成两段）。Native 的 `~<n>` 只用于「**地址相同但逻辑用途不同**」的显式区分，且**必须由用户可见地确认**，不得静默生成。
  4. **点位身份不得依赖前端展示文案**：身份 = 持久化的 `point_key`；`display_name` / 定义的 `label`（`LocalizedText`）是**纯展示元数据**，其变化**不得**影响 `point_key`/`point_id`（R6/R11）。
- **`nck_native` 默认 Key 不含** `decode`/`transport_size`/`invalid_*`——因为那三项是**质量与解码**属性，不是身份。若同一地址需要不同 decode，按第 1 条由用户**显式区分命名**，UI 必须提示「这将是两个不同点位」（否则用户会以为改了 decode 还是同一个点）。

**迁移**：不迁移历史 key（指令 9）。旧 key（`variable.value`、`variable.value.2`…）在新模型下不再产生；重建点位会得到新 key → 新 point_id。

### 4.2 `point_id` 稳定性与 tombstone 语义（新模型下的具体表述）

实现**不变**（`crates/config-store/src/lib.rs:1655-1770`，1.3 冻结），语义表述如下：

| 规则 | 表述 | 依据 |
|---|---|---|
| R1 | `point_id` 由 Core 分配并持久化，取值 `max(该 Endpoint 已有 point_id) + 1`（含 tombstone） | `config-store:1699-1700` |
| R2 | 同一 `(endpoint_id, point_key)` 再次出现（含 `deleted=1` 的墓碑行）→ **复用原 `point_id`**，并清 `deleted` 标记 | `config-store:1708-1715`、`:1721-1736` |
| R3 | 本轮未出现的活跃 key → 置 `deleted=1`（**保留行与 id**，不删除） | `config-store:1750-1766` |
| R4 | 墓碑行的 `point_id` **永不复用**（因为 `max+1` 计入墓碑） | `config-store:1699-1700` |
| R5 | `point_key` 在 Endpoint 内**必须唯一**（跨 Task 也唯一） | `crates/core-types/src/resource.rs:349-360` |
| R6 | `display_name` 是用户元数据，configure **不得**覆盖或清空 | `config-store:1719-1727`、`:1743-1746` |
| R7 | `source_label` 是 configure 真值快照：`Some` 覆盖、`None` 清空 | `config-store:1716-1718` |

**新模型下的新增语义表述**（**五条**，必须写进测试）：

| 规则 | 表述 | 理由 |
|---|---|---|
| **R8** | 仅 `definition_revision` 变化（`variable_id`/`target`/`arguments` 不变）⇒ `point_key` 不变 ⇒ `point_id` **不变** | §4.1 I3；revision 是「定义修正」而非「新点位」 |
| **R9** | `variable_id` / 任一 target 实例 / 任一 argument 变化 ⇒ `point_key` 变化 ⇒ **新 `point_id`**；旧 key 走墓碑，其 `point_id` 永不复用 | §4.1 I1/I2 + R3/R4 |
| **R10** | **Point Kind** 在 `nck_variable` ↔ `nck_native` 之间切换**必然**改变 `point_key`（前缀不同）⇒ 新 `point_id` | 语义点与原生点是不同身份，不得互相「顶替」 |
| **R11** | `point_key` 一旦持久化即为身份；`display_name` / 定义 `label` 变化**不得**改变 `point_key` 或 `point_id` | 修订 3-其二：**身份不得依赖前端展示文案** |
| **R12** | Native Point 的默认 Key 由地址派生，但**允许用户改名**；同一地址用于不同逻辑采集用途 ⇒ **不同点位、不同 `point_id`** | 修订 3-其二：**地址不是唯一身份**（§4.1.2） |

**`source_label` 在新模型下的取值**：`Semantic` 编译成功时 = 现有 `NativeNckAddress::source_label()` 的稳定格式（N10），**不额外拼接** `variable_id`（`source_label` 是**原生地址标识**，语义名属于配置而非地址）。语义名通过 `display_name`（用户命名）与前端展示表达。这一点必须写死，否则 `source_label` 的稳定性会被破坏。

### 4.3 三条 `data_semantics.rs` 断言必须显式改写的内容

三条断言的**位置与保护对象不变**（`point_id` 稳定性契约），但必须**显式重写**为语义模型下的表述。逐条给出：

#### （1）`tests/driver-contract/tests/data_semantics.rs:57-88`「同 key 恢复同 id」

**现状**：用字面量 key `"k1"` 断言两次 `assign_point_ids` 返回同一 `point_id`（:87）。

**改为**：
1. 保留原字面量 key 用例（**不得删除**，它是 ConfigStore 层的冻结契约）；
2. **新增**语义 key 用例：构造 §4.1 的默认 `point_key`（如 `channel.r_parameter#index=100@channel=1`），断言两次分配同 id；
3. **新增 R8 断言**：同一 `variable_id`/`target`/`arguments`、仅 `definition_revision` 从 1 改为 2 时，**推导出的 `point_key` 逐字相等** ⇒ 第二次分配恢复同一 `point_id`。

> 该用例必须**只依赖 `point_key` 推导函数**（`crates/core-types/src/point.rs` 内的纯函数），不得依赖驱动，以便契约套件保持「无驱动子进程」的轻量属性。

#### （2）`tests/driver-contract/tests/data_semantics.rs:90-149`「删除 id 永不复用」

**现状问题（本次核对发现）**：`:148` 的断言是 `assert!(k3_id > k1_id || k3_id != k1_id)`——由于 `>` 蕴含 `!=`，整个表达式**等价于** `k3_id != k1_id`，`>` 分支是**死代码**，该行实际上没有验证「单调递增」。这是一个**弱断言**，必须修正。

**改为**：
1. `assert_ne!(k3_id, k1_id, ...)` 保留（:146）；
2. `:148` 改为**强断言**：`assert!(k3_id > k1_id, "新 id 必须严格大于墓碑 id（max+1 计入墓碑）")`；
3. **新增**语义模型用例：删除一个语义点（其 key 缺席）后，其 `point_key` 的墓碑行 `deleted=1` 且 `point_id` 保留；随后分配**不同**语义 key 时，新 `point_id` 严格大于该墓碑 id；
4. **新增 R9 断言**：改 `variable_id`（或改 `arguments.index`）后旧 key 转墓碑、新 key 得新 id，且旧 id 不再出现。

#### （3）`tests/driver-contract/tests/data_semantics.rs:151-183`「重新加入恢复原 id」

**现状**：断言重新 assign 同一 key 恢复原 id（:182）。

**改为**：
1. 保留原用例（:182 的断言不变）；
2. **新增 R8 用例**：以 `definition_revision` 不同的**同一语义点位**重新加入，断言 `point_key` 与 `point_id` **均**不变（这是「revision 修正不改变点位身份」的**唯一**契约证据）；
3. **新增 R10 用例**：同一物理地址分别以 `nck_variable` 与 `nck_native` 表达，断言两者 `point_key` **不同**、`point_id` **不同**，且互不顶替。

**下界约束**：三条断言的**用例数不得减少**（原 3 个 → 改写后 ≥ 3，实际按上述展开为 9 个），且原字面量 key 的用例必须**原样通过**（它们是 ConfigStore 层契约，与语义模型正交）。

---

## 5. 破坏性变更说明

逐条列出会被破坏的内容、影响面与是否需 ADR。**所有条目均由 ADR 0005 统一留痕**（个别条目已在上位 ADR 有先例）。

| # | 破坏内容 | 具体表现 | 影响面 | 是否需 ADR |
|---|---|---|---|---|
| 1 | **旧 NCK 点位配置格式**（`variable` 资源 + 12 原生字段） | 旧 `ResourceSelection{resource_id:"variable", parameters:{area,module,...}}` 在新驱动下被 `UNSUPPORTED_BINDING`/`INVALID_BINDING_CONFIG` 拒绝 | 所有现存 NCK Endpoint 的点位配置需重建；ADR 0004:74-75 已有同类先例（本机 240 点被拒并备份） | **是**（ADR 0005 主体） |
| 2 | **旧 `point_key`**（`variable.value`、`variable.value.2`…） | 新 key 为语义形态（§4.1）⇒ 旧 key 全部转墓碑；重建得新 `point_id` | 依赖 `point_id` 的历史数据（`point_live`、事件绑定、下游消费）在新点位上**不连续** | **是** |
| 3 | **旧预填模板**（`presets/828d.json` 62 个原生字段模板） | 模板内容重构为语义 target/arguments；模板 id 规则变化 | 浏览树叶节点形态变化；`discovery_contract.rs:224-269`、`browseModel.ts` 需同步 | **是**（可与 1 合并） |
| 4 | **旧 Descriptor 资源面**（NCK `resources[0].id == "variable"` 的 12 字段 schema） | 语义模式下用户不再看到原生字段；`descriptor_contract.rs:728-731` 的断言失效 | 任何按「NCK 有 `variable` 资源」编写的客户端/脚本失效；Descriptor Major **不需要**升级（B6：本次不扩 `SchemaDescriptor`，但资源集合内容变化属于 Minor 内的行为变化） | **是**（须说明为何不升 Major） |
| 5 | **browse 语义**（模板叶 `binding_json = {resource_id,parameters}`） | 叶形态改为语义载荷；`browseModel.selectableFromNode` 的信封识别扩展 | 前端浏览回填路径、`browseModel.test.ts:109-135` 必须改写 | **是**（可与 3 合并） |
| 6 | **Task Binding Kind 面** | Core 白名单从单 kind 变为 `{mesa.resources.v1, nck.points.v1}`（两处：`core-api/src/lib.rs:241`、`bootstrap.rs:225`）；Core **不承载**任何 Siemens 类型，**不建 Point Kind 白名单**（修订 1） | 仅影响 NCK；其他 4 驱动不变（B18） | 否（属 1 的实施细节，记入 ADR 0005） |
| 7 | **错误码面** | NCK 不再产生 `UNSUPPORTED_RESOURCE`/`UNSUPPORTED_OUTPUT`；新增 **7 个 Driver 码**（含 `UNSUPPORTED_POINT_KIND`）+ **4 个 Core 码**（`INVALID_PARAMETERS`/`EMPTY_POINT_KIND`/`EMPTY_POINT_KEY`/`INVALID_POINT_KEY`，§2.7）；初稿的 Core 侧 `INVALID_TARGET` **取消**（移入 Driver） | 运维/告警规则若按旧码匹配需更新 | 否（记入 ADR 0005） |
| 8 | **`nck_native` 的语义保护缺失** | 用户显式声明原生地址时**没有** `r0035=-200` 之类哨兵保护（除非自己填） | 与 ADR 0004:72-73 的既有口径一致（「驱动不隐式继承、不按变量名猜哨兵」） | 否（沿用既有口径） |
| 9 | **`r0031` 访问能力 HOLD** | `r0031` 定义仍存在（golden 要求），但真机复测被 `0x85` 拒绝（G8） | 该变量必须在 UI/定义中标注 `candidate` + 访问能力 HOLD，**不得**宣称可用 | 否（沿用 G8） |

### 5.1 「非 NCK 驱动 / 连接 / 协议编解码 / 运行质量处理不得退化」的具体护栏

| 护栏 | 具体措施 | 验证 |
|---|---|---|
| **P1 非 NCK 驱动零改动** | `drivers/s7/**`、`drivers/focas2/**`、`drivers/opcua/**` 与 `crates/core-types/src/resource.rs` 的 `ResourceSelection`/`GenericBinding`/`validate_task_set_against` **形状不变** | 4 个驱动的契约套件 + `resource_contract` 原样通过；`git diff --stat` 断言零改动 |
| **P2 Core 门禁双份收敛而非放宽** | `core-api/src/lib.rs:241` 与 `bootstrap.rs:225` 收敛为**同一函数**；`mesa.resources.v1` 的判定**逐字不变** | 新增单测：`mesa.resources.v1` 走原路径；未知 kind 仍 `UNSUPPORTED_BINDING` |
| **P3 连接面冻结** | `apps/mesa-web/src/connection/**` 9 文件（F13）**零改动**；`ConnectionSetupPanel`/`useConnectionSetup`/`connectionSetupModel` 的 134 用例原样通过 | `pnpm test src/connection` = 134 passed |
| **P4 连接面板语义冻结** | `DescriptorFields.tsx`（含 `ui.advanced` 折叠，:336-349）与 `ApplyWithRestart.tsx` **零改动**；S7 TSAP 的 `advanced` 行为不变 | `DescriptorFields.advanced.test.tsx`、`DescriptorFields.test.ts`、`DescriptorFields.secret.test.tsx` 原样通过 |
| **P5 协议编解码冻结** | `codec.rs`/`value.rs`/`native.rs`/`client.rs` 零改动；62 条 golden 字节不变 | `legacy_equivalence`、`native_address`、`sharp7_vectors`、`examples_828d` 原样通过（`checked == 62`） |
| **P6 运行质量处理冻结** | 无效值哨兵 → BAD 的路径不变（`value.rs` + `native.rs:60-72`）；`Quality::Good/Bad` 与 `value_origin` 语义不变；`source_timestamp_ns` 恒 `None`（`value.rs:1-9`） | `data_plane`、`data_semantics`、`point_live` 原样通过；`drive_access` 6 用例原样通过 |
| **P7 身份能力冻结** | `identity_spec.rs`/`identity.rs` 零改动；module 26/130 双入口不折叠 | `identity_browse` 8 用例 + `discovery_contract` 的 NCK 分支原样通过 |
| **P8 事件面零影响** | `gate_event_tasks`（`core-api/src/lib.rs:330-361`）与 `mesa.events.v1` 零改动 | 12 个事件面套件原样通过 |
| **P9 Core 零 Siemens 知识（修订 1 护栏）** | `crates/core-types/**`、`crates/core-api/**` 不得出现 `TargetRef`/`DriveObjectRef`/`DriveEntry`/`nck_variable`/`nck_native` 等 Siemens 专有标识（`nck.points.v1` 是**任务信封常量**，允许出现）；Core **不得**枚举 Point Kind，**不得**知道 Module 26/130 | 新增断言/CI 检查：`crates/core-*` 对上述标识零命中；单测证明「未知 Point Kind（如 `"weird_kind"`）在 Core 信封层**被放行**（只判非空），由 Driver 以 `UNSUPPORTED_POINT_KIND` 拒绝」 |
| **P10 62 条 Golden 完整覆盖（评审重申）** | 62 条 Wire Golden 必须**逐条**建立语义对应并被断言（§3.4、§6.5），**Module 26/130 不允许折叠**（`entry` 必填、双入口同 column 不同 module/wire） | `semantic_equivalence` + `legacy_equivalence` 双门 `checked == 62`；G4 双入口断言 |

---

## 6. 测试矩阵

### 6.0 基线（本次在 `0d686b4` 实跑，作为「不得减少」的下界）

| 项 | 数值 | 实测命令 |
|---|---|---|
| 前端文件 / 用例 | **33 / 383** 全过 | `pnpm test`（`apps/mesa-web`） |
| 受影响的 7 文件 | **119** 用例 | `pnpm test src/deviceModel.test.ts src/addDevice/bootstrap.test.ts src/addDevice/AddDeviceFlow.test.tsx src/browseModel.test.ts src/components/ResourcePickerAntd.test.tsx src/components/ResourceBrowseAntd.test.tsx src/components/EndpointAcquisitionPane.saveIssues.test.tsx` |
| 连接组 3 文件 | **134** 用例 | `pnpm test src/connection` |
| 必须继续通过（非受影响） | **383 − 119 = 264** | 由上述两式相减得出；**其中含连接 134** |
| 契约套件 | **29** 套（C1） | `scripts/contract_suites.py:13-52` |
| NCK Wire Golden | **62** 条（42 NC 变量入口 + 20 驱动对象入口） | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` |

**下界声明**：`383 / 264 / 134 / 119 / 29 / 62` 是**不得减少**的下界。受影响文件的用例数**可以增加**（新增语义用例），但**不得**以「删除旧用例」的方式通过。

### 6.1 PR-A（Core 契约）

| 类型 | 项 | 内容 |
|---|---|---|
| **新增** | `crates/core-types` 单测 | `point.rs`：`PointSelection` 反序列化、**信封层** `deny_unknown_fields`（未知字段即拒）、`parameters` 必须为 JSON 对象、`kind` **非空**（`EMPTY_POINT_KIND`）、`EMPTY_POINT_KEY`、`INVALID_PARAMETERS`；**不含任何 Target/NCK 字段校验**（修订 1） |
| **新增** | `point_key` 规范编码单测 | §4.1.1 的 `enc`：分隔符百分号编码、`%` 本身编码、NFC 归一、空串 vs 缺省、键名排序、缺省不物化、长度上限 **256 字节**；并逐条断言 **E1–E5 歧义示例「不同输入 ⇒ 不同 Key」**、同一逻辑文本 NFC 后 ⇒ 同一 Key |
| **新增** | Core 门禁单测 | **Task Binding Kind** 白名单（`{mesa.resources.v1, nck.points.v1}`）；`mesa.resources.v1` 原路径逐字不变；未知 **Task Binding Kind** → `UNSUPPORTED_BINDING`；**未知 Point Kind（如 `"weird_kind"`）在信封层被放行**（P9，证明 Core 无 NCK 白名单） |
| **新增** | 混合点位合同测试（§6.6 **T1**，Core 侧） | 一个 `nck.points.v1` 任务的 `points[]` **同时**含 `nck_variable` 与 `nck_native` → Core 门禁**放行**（不得因 Point Kind 拒绝、不得要求拆成两个任务） |
| **必须改写** | `data_semantics` | §4.3 的三条断言（原 3 → ≥9 用例，原字面量 key 用例原样保留）；含 §6.6 的 **T2**（跨任务唯一）与 **T3**（Revision 稳定性） |
| **必须原样通过** | `resource_contract`、`descriptor_contract`（PR-A 阶段不含 NCK 资源面改动）、`management_api`、`smoke`、`protocol_negotiation`、`session_lifecycle`、`data_plane`、`point_live`、`fault_tolerance`、`subprocess_recovery`、`discovery_contract`、`control_contract`、`subprocess_orphan_guard` + 事件 12 套 + `mesad_restart`/`stop_lifecycle`/`opcua_auth_restart_e2e` | 全部 |
| **必须原样通过** | 前端全部 **383** | PR-A 不改前端 |
| 验收 | `cargo test --workspace` 全绿；契约 29/29；前端 383/383；`point_key` 规范编码的 E1–E5 逐条通过；**Core 无 NCK 知识（P9）断言通过**；**PR-A 完成后必须独立验收，才能进入 PR-B**（§7.1） | — |

### 6.2 PR-B（Driver Compiler）

| 类型 | 项 | 内容 |
|---|---|---|
| **新增** | `drivers/sinumerik-nck/tests/definition_catalog.rs` | 42 条定义完整性；`(variable_id, revision)` 唯一；digest 与 `DIGESTS.lock` 一致；`semantics_status` 与 ID 形态规则（§2.5.2）；`scope=I` 的 6 条与 `identity_spec.rs` 同值 |
| **新增** | `drivers/sinumerik-nck/tests/semantic_equivalence.rs` | §3.4：42 条定义逐条编译 → `encode_var_spec` == 冻结 `wire_hex`；`checked == 62`；20 条驱动对象入口 module/wire 必须不同（G4） |
| **新增** | Compiler 单测 | `UNKNOWN_VARIABLE`、`UNSUPPORTED_DEFINITION_REVISION`（三种禁止退化逐条断言）、`MISSING_TARGET`、`INVALID_TARGET`（`entry` 缺失必拒）、`INVALID_ARGUMENTS`、`ArgumentOffset` 边界（R 参数 index=0 → line=1；上溢 → 拒） |
| **新增** | 参数层严格校验单测（§2.4.2 **D1–D10**） | `UNSUPPORTED_POINT_KIND`（Point Kind 非两值）；`parameters` 层**未知字段即拒**；类型错；索引范围；原生地址枚举与无效值成对；`unit` ≤32 字符 |
| **新增** | 混合点位合同测试（§6.6 **T1**，Driver 侧） | 同一任务内 `nck_variable` 与 `nck_native` 混排 → `configure` 成功，**逐点**路由到 Address Compiler 两条分支；**Module 26/130 不折叠**（`entry` 缺失 → `INVALID_TARGET`） |
| **必须改写** | `descriptor_contract`（`:391-396`、`:593-603`、`:728-731`） | 资源面断言跟随新形态；**只读 / 无 subscribe / 无 events 三条不得放宽** |
| **必须改写** | `discovery_contract`（`:224-269`） | 浏览叶形态；`nck://presets` 根与诊断节点语义不变 |
| **必须原样通过** | `legacy_equivalence`（2 用例，`checked == 62`）、`native_address`（8）、`presets_828d`（6，若模板未在 PR-B 重构则原样）、`examples_828d`（3）、`drive_access`（6）、`identity_browse`（8）、`sharp7_vectors`（6） | 62 条 golden 字节**必须不变** |
| **必须原样通过** | 契约 29/29；前端 **383**（PR-B 不改前端） | — |
| 验收 | 62/62 字节等价；`sha2` 依赖加入后 `cargo build --locked --workspace` 通过；无 `UNSUPPORTED_DEFINITION_REVISION` 的静默退化路径 | — |

### 6.3 PR-C（Frontend 编辑器）

| 类型 | 项 | 内容 |
|---|---|---|
| **新增** | `nckPointModel.test.ts` | `point_key` 推导（I1/I2/I3）、**规范编码与 Core 纯函数逐字一致**（§4.1.1，含 E1–E5）、payload 构造、默认值、按 Point Kind 的必填预检、`nck_variable` ↔ `nck_native` 判别；**用户改名后身份稳定**、**禁止自动递增后缀**、`display_name` 变化不影响 `point_key`（§4.1.2） |
| **新增** | `NckPointFields.test.tsx` | 语义表单渲染；`nck_native` 高级警告；必填缺失提示；**不出现**原生字段（语义模式） |
| **必须改写** | `deviceModel.test.ts` | 双真值收敛后的唯一装配入口；`splitAcquisitionTasks`/`selectionsOf` 识别 NCK kind；保留原有任务保全语义 |
| **必须改写** | `addDevice/bootstrap.test.ts` | 装配入口收敛；`tasks=[]` 零采集点语义不变 |
| **必须改写** | `AddDeviceFlow.test.tsx` | 「先建 Endpoint 再选点位」的步骤语义；离线手工选实例；**不得**伪造浏览/读取成功 |
| **必须改写** | `browseModel.test.ts`（`:109-135` 起） | 语义信封识别；`missingRequiredParams` 的必填来源按 Kind 分流 |
| **必须改写** | `ResourcePickerAntd.test.tsx` | 按 Kind 的必填门；NCK 语义分支不再使用原生 schema |
| **必须改写** | `ResourceBrowseAntd.test.tsx` | 浏览叶 → 语义 Selection |
| **必须改写** | `EndpointAcquisitionPane.saveIssues.test.tsx` | 装配收敛后的绑定形态 |
| **必须原样通过** | `src/connection/**` 3 文件 = **134** | P3 护栏 |
| **必须原样通过** | `DescriptorFields.test.ts`、`DescriptorFields.advanced.test.tsx`、`DescriptorFields.secret.test.tsx` | P4 护栏（`DescriptorFields.tsx` 冻结） |
| **必须原样通过** | 其余非受影响文件 | 合计 **264**（含 134） |
| 验收 | 前端 **≥383** 全过；`264` 与 `134` 两个下界**不得下降**；`pnpm typecheck` 与 `pnpm lint` 通过 | — |

### 6.4 PR-D（AddDeviceFlow + E2E）

| 类型 | 项 | 内容 |
|---|---|---|
| **新增** | 端到端用例（前端） | 「建设备 → 建 Endpoint → 选点位与 Target → 启动」全链；离线分支（设备不可达）**不得**标记浏览/读取成功 |
| **新增** | 端到端用例（契约侧） | NCK 语义点位从 REST 保存 → 启动 → `points/latest` 出现值的链路（可用 Simulator 语义，**不得**连真机） |
| **必须改写** | `AddDeviceFlow.test.tsx`（步骤语义最终形态） | 与 PR-C 的差异必须显式说明，禁止「同一文件在两阶段被不同语义改两次」而无验收 |
| **必须原样通过** | 全部契约 29 套 + 前端 ≥383 | — |
| 验收 | 全链 e2e 通过；失败路径明确区分「未执行」与「执行失败」（指令 6） | — |

### 6.5 Wire Golden 清单（62 条，PR-B 必须逐条断言）

`42` 条 `entry=nc_variable`：`C/S/progStatus`、`B/S/opMode`、`C/SPARPP/progName`、`C/SPARP/progName`、`C/SPARP/actLineNumber`、`C/SPARP/actBlock`、`C/SGA/actToolBasePos`、`C/SEGA/actProgPosBKS`、`C/S/actFeedRateIpo`、`C/S/cmdFeedRateIpo`、`C/S/feedRateIpoOvr`、`C/SSP/actSpeed`、`C/SSP/cmdSpeed`、`C/SSP/speedOvr`、`C/S/actTNumber`、`C/RP/rpa`、`V/M/r0021`、`V/M/r0027`、`V/M/r0035`、`V/M/r0033`、`V/M/r0031`、`V/M/r0062`、`V/M/r0063`、`V/M/r0064`、`V/M/r0077`、`V/M/r0078`、`V/M/r0079`、`V/M/r0080`、`V/M/r0081`、`V/M/r0082`、`V/M/r0103`、`V/M/p0105`、`V/M/r0106`、`V/M/r0107`、`V/M/r0108`、`V/M/p0199`、`N/M/MDSA_AXCONF_MACHAX_NAME_TAB`、`A/M/MDCA_CTRLOUT_MODULE_NR`、`A/M/MDCA_ENC_MODULE_NR`、`A/M/MDCA_CTRLOUT_TYPE`、`C/M/AXCONF_MACHAX_USED`、`A/M/SPIND_ASSIGN_TO_MACHAX`

`20` 条 `entry=drive_object`：`V/M/r0021`、`r0027`、`r0035`、`r0033`、`r0031`、`r0062`、`r0063`、`r0064`、`r0077`、`r0078`、`r0079`、`r0080`、`r0081`、`r0082`、`r0103`、`p0105`、`r0106`、`r0107`、`r0108`、`p0199`（均为 module 130）

**断言口径**：`checked == 62` 且 `assert_eq!(encode_var_spec(&rp.wire), bytes(e["wire_hex"]))` 逐条成立（沿用 `legacy_equivalence.rs:100-107` 的形式）。

**评审重申**：62 条 Wire Golden 必须**完整覆盖**（不得抽样、不得跳过、不得减少）；**Module 26/130 不允许折叠**（`entry` 必填，双入口同 column 必须不同 module 且不同 wire）。

### 6.6 评审要求的三条合同测试（必须有，**不得**以单测替代）

| # | 合同测试 | 层次与位置 | 断言（失败即为架构错误） |
|---|---|---|---|
| **T1** | **同一任务混合点位** | Core 门禁（**PR-A**）+ Driver `configure`（**PR-B**） | 一个 `nck.points.v1` 任务的 `points[]` **同时**含 `nck_variable` 与 `nck_native`：Core 门禁**放行**（不因 Point Kind 拒绝）；Driver `configure` 成功且**逐点**路由到 Address Compiler 的两条分支；**不得**要求用户/前端拆成两个任务 |
| **T2** | **跨任务 Point Key 唯一性** | Core 集合校验（**PR-A**） | 同一 Endpoint 的**两个不同任务**出现同一 `point_key` → `DUPLICATE_POINT_KEY`（B9/R5）；同一 `point_key` 在同一任务内重复同样拒绝；不同 Endpoint 的同名 key **不**冲突 |
| **T3** | **Revision 更新后身份稳定性** | ConfigStore + Core（**PR-A**） | 仅 `definition_revision` 1→2（`variable_id`/`target`/`arguments` 均不变）：推导出的 `point_key` **逐字相等** ⇒ `assign_point_ids` 恢复**同一** `point_id`；反向：改 `target` / `arguments` / Point Kind ⇒ 新 Key、新 `point_id`，旧 id 走墓碑且**永不复用**（R8/R9/R10） |

三条合同测试的**下界**：T1 ≥2 用例（Core + Driver 各 1），T2 ≥3 用例（跨任务/同任务/跨 Endpoint），T3 ≥3 用例（revision 不变、target 变、Point Kind 变）。

---

## 7. PR 顺序

### 7.0 总原则：禁止多阶段交叉修改同一核心契约而缺少验收

「核心契约」= **Core 侧**：`crates/core-types/src/point.rs` 的公开类型（**仅**通用信封 `PointSelection`/`PointBinding` 与 `NCK_POINTS_BINDING_KIND`）、`point_key` 规范编码规则（§4.1.1）、`nck.points.v1` 信封形状；**Driver 侧**：`drivers/sinumerik-nck/src/targets.rs` 的 `TargetRef`/`DriveEntry`、`VariableDefinition` 的**字段集合与 digest 算法**。约束：

1. 每个 PR **只允许**在其归属阶段**首次定义**核心契约；后续 PR 只能**消费**，不得改形状。
2. 若某 PR 确需改核心契约，**必须**：先单独提交一个「契约变更」PR（含验收），再继续；**禁止**在同一 PR 内既改契约又改消费方并只跑一半验收。
3. 每个 PR 的验收必须包含「该阶段全部必须原样通过的测试」+「该阶段必须改写的测试已改写且用例数不减少」。
4. 阶段间不得并行修改同一文件：`point.rs`（PR-A 独占）、`targets.rs`/`compiler.rs`/`definition.rs`（PR-B 独占）、`nckPointModel.ts`/`NckPointFields.tsx`（PR-C 独占）、`AddDeviceFlow.tsx`（PR-D 独占）。共享文件（`deviceModel.ts`、`resourceSelectionModel.ts`、`presets.rs`、`browseModel.ts`）**每个 PR 只允许改其归属部分**，且必须在 PR 描述中列出改动的函数名。

### PR-A：Core 契约

| 项 | 内容 |
|---|---|
| 范围 | `crates/core-types/src/point.rs`（新增，**仅通用信封**）；`crates/core-api/src/lib.rs:228-258` 与 `bootstrap.rs:218-251` 的**白名单收敛**（按 Task Binding Kind 路由）；`runtime.rs:178` 随动；`data_semantics.rs` 三条断言改写；`AGENTS.md` 的 29 套纠错 |
| 依赖 | 无（最先） |
| 不改 | 前端全部；NCK 驱动；`resource.rs` 形状；`schema.rs`；`descriptor.rs` |
| 验收 | ① `cargo test --workspace` 全绿；② 契约 29/29；③ 前端 383/383（零改动）；④ 新单测覆盖 §2.4 信封层全部错误码；⑤ `mesa.resources.v1` 路径**逐字不变**（有专门用例）；⑥ **P9 断言**：Core 零 Siemens 知识、**无 Point Kind 白名单**；⑦ **`point_key` 规范编码（§4.1.1）冻结并含 E1–E5 用例**；⑧ §6.6 的 T1（Core 侧）/T2/T3 通过 |
| 交付物 | 冻结的 `point.rs` 公开类型（信封层严格 + 参数层不透明）+ `point_key` 规范编码纯函数 + 双份白名单收敛为一份 + **保存反馈机制形态明确**（§3.3 末段，评审裁决 10） |

### PR-B：Driver Compiler

| 项 | 内容 |
|---|---|
| 范围 | `drivers/sinumerik-nck/src/{definition.rs, compiler.rs, targets.rs, definitions/828d/*.json}`（新增）；`lib.rs` 的 descriptor 与 configure 重构；`presets.rs` 叶形态；`presets/828d.json` 语义化；`Cargo.toml` 加 `sha2`；`descriptor_contract`/`discovery_contract` 改写；新增 `definition_catalog.rs`/`semantic_equivalence.rs` |
| 依赖 | **PR-A 合并**（消费 `PointSelection` 的形状）；且 **PR-A 必须已独立验收**（§7.1） |
| 不改 | `native.rs`/`codec.rs`/`value.rs`/`client.rs`/`address.rs`/`identity_spec.rs`/`identity.rs`（1.3 冻结）；`legacy-828d-wire.json`（**一个字节都不许改**）；前端 |
| 验收 | ① 62/62 字节等价（`checked == 62`，G4 的双入口断言成立）；② `UNSUPPORTED_DEFINITION_REVISION` 的三种禁止退化逐条有用例；③ 42 条定义 digest 与 `DIGESTS.lock` 一致；④ `legacy_equivalence`/`native_address`/`presets_828d`/`examples_828d`/`drive_access`/`identity_browse`/`sharp7_vectors` 原样通过；⑤ 契约 29/29；⑥ 前端 383/383（零改动）；⑦ **参数层严格校验 D1–D10 逐条有用例**；⑧ **§6.6 T1（Driver 侧）通过，Module 26/130 不折叠** |
| 交付物 | Address Compiler 唯一入口 + **Driver 侧 `targets.rs`（TargetRef/DriveEntry）** + 定义目录（42 条 × revision 1）+ 语义等价门 |

### PR-C：Frontend 编辑器

| 项 | 内容 |
|---|---|
| 范围 | 新增 `nckPointModel.ts`、`NckPointFields.tsx` 及其测试；重构 `deviceModel.ts`（装配收敛 + kind 识别）、`addDevice/bootstrap.ts`（装配收敛）、`resourceSelectionModel.ts`（identity 扩展）、`browseModel.ts`（信封扩展）、`ResourcePickerAntd.tsx`（按 Kind 必填）、`AddDeviceFlow.tsx`（渲染入口） |
| 依赖 | **PR-B 合并**（需要 descriptor 的实际形态与浏览叶形态） |
| 不改 | `src/connection/**`（P3）、`DescriptorFields.tsx`（P4）、`ApplyWithRestart.tsx`（P4）、`resourceSelectionModel.ts:169-377` 的裁决算法 |
| 验收 | ① 前端 ≥383 全过；② `264` 与 `134` 下界不降；③ 7 个受影响文件的新用例覆盖 §4.1 三条不变量与 §2.4 的按 Kind 必填；④ `pnpm typecheck` + `pnpm lint` 通过；⑤ 生产代码仍**零**硬编码 NCK 原生协议字段（除 `nck_native` 表单的显式字段声明） |
| 交付物 | NCK 专用语义表单 + 唯一 Binding 装配入口 + 按 Kind 的必填校验 |

### PR-D：AddDeviceFlow + E2E

| 项 | 内容 |
|---|---|
| 范围 | `AddDeviceFlow.tsx` 步骤语义最终化（先建 Endpoint 再选点位与 Target）；端到端用例；失败路径语义（未执行 vs 执行失败） |
| 依赖 | **PR-C 合并** |
| 不改 | PR-A/B/C 已冻结的核心契约与前端纯函数层 |
| 验收 | ① 全链 e2e 通过（不连真机）；② 离线分支明确区分「未执行」与「失败」，**不得**标记浏览/读取成功；③ 契约 29/29；④ 前端 ≥383；⑤ 双真值已消除（全仓 grep 断言只有一处 Binding 装配点） |
| 交付物 | 可用的「建设备 → 建连接 → 选点位 → 启动」全流程 |

### 7.1 依赖图

```text
PR-A (Core 契约冻结)
  └─> PR-B (Driver Compiler + 定义目录 + 62 条等价门)
        └─> PR-C (前端语义表单 + 装配收敛)
              └─> PR-D (AddDeviceFlow 最终化 + E2E)
```

严格串行。任何跨阶段并行都必须先拆分出独立的「契约变更」PR。

**评审要求（门禁，不得绕过）**：**PR-A 完成后必须独立验收，才能进入 PR-B**。即：`cargo test --workspace` 全绿 + 契约 29/29 + 前端 383/383 + §7 PR-A 的 ⑥⑦⑧ 专项用例（P9、`point_key` 规范编码 E1–E5、T1 Core 侧 / T2 / T3）全部通过并留证，方可开启 PR-B。**不得**「PR-A 未验收即开工 PR-B」，**不得**用 PR-B 的验收结果回填 PR-A。

---

## 8. 已裁决事项与剩余依赖（评审后）

原 §8 的 12 项**待确认**已全部裁决（评审结论 = 有条件通过）。下表为**裁决结果 + 落实位置 + 剩余依赖**。**已裁决项不得再作为「待确认」重新提出**；「剩余依赖」栏非空者按 HOLD 处理，但**不阻塞架构**。

### 8.1 已裁决（12 项）

| # | 事项 | 裁决 | 落实位置 | 剩余依赖（HOLD） |
|---|---|---|---|---|
| 1 | **信封命名** | `nck.points.v1`（**Task Binding Kind**）；**不采用** `mesa.points.v1` | §2.0、§2.1、§2.4 | 无 |
| 2 | **ModeGroup** | **纳入** NCK Target 定义（`B/S/opMode` 的唯一归属） | §2.4.1、§2.5.1 表 A#2 | 无 |
| 3 | **Tool Target** | 依据**实际变量需求**确定，**不为凑齐枚举提前增加** | §2.4.1 | 出现 `T` area 刀具变量时再补 |
| 4 | **Spindle** | 业务**独立**类别（`spindle.*`），协议仍用**现有已验证寻址**（`C/SSP`，`line`=主轴索引）；**不虚构** `S<n>` 等未验证区域 | §2.5.1 表 A#12–#14 | 无 |
| 5 | **驱动轴归属** | 继续 **HOLD**，**不阻塞架构**（`TargetRef{drive_object}` 只表达驱动访问编号 + 入口，**不表达**「哪个电机」） | §2.5.1 B#19 注 | 外部证据：SINAMICS 拓扑/厂商标定含义，或受控单轴运动测试（超只读范围，需另行批准） |
| 6 | **19 个驱动参数语义** | 保留**参数编号型 ID** `drive.parameter.<rNNNN>`，**不编造物理意义** | §2.5.1 B、§2.5.2 | 手册**具体页码/章节**证据（可离线，无需真机） |
| 7 | **`r0031`** | **保留候选定义**（golden 要求），访问能力 **HOLD**（标 `candidate`，**不得**宣称可用） | §5-9、§2.5.1 B#21 | 真机复测（今天被 `0x85` 拒绝，G8） |
| 8 | **840D sl** | **不纳入本次**；`DECLARED_FAMILIES` 仍含 `840d-sl`（N1）但**无任何定义**，必须明确「该系列无定义可引用」，**不得**套用 828D | §2.5 `family`、§8.2 | 真机/文档证据 |
| 9 | **Native 限权** | 本期采用**只读约束**（沿用 V1 只读基线 + `nck_native` 仅前端警告、不得作为默认路径），**不额外引入复杂权限系统** | §2.3 | 产品裁决（若将来需要「仅管理员可用」） |
| 10 | **Revision 保存校验** | **Driver 负责最终校验**；Core 保存门禁只做**信封层**校验；**保存反馈机制在 PR-A 明确** | §3.3 末段、§2.8 #12、§7 PR-A 交付物 | PR-A 实施设计（descriptor 可用时是否多一次只读校验调用） |
| 11 | **Point Key 规范** | **PR-A 必须确定**（本次已给出规范：编码算法 + NFC + 256 字节上限 + E1–E5） | §4.1.1、§7 PR-A | 无 |
| 12 | **Golden 双轨** | 重构期间**保留双轨**（`legacy_equivalence` + `semantic_equivalence`），**完成独立验收后再评估精简** | §1.3、§3.4、§6.5 | 独立验收完成后评估 |

> 计数说明：评审清单表述为「11 项处理决定」，本文档按原 §8 的**12 行**逐项裁决（其中「Point Key 规范」即原 §8-11「`point_key` 字符集与长度上限」，本次已由 §4.1.1 定稿）。

### 8.2 剩余依赖（HOLD，不阻塞架构）

| 依赖项 | 性质 | 处置 |
|---|---|---|
| 驱动对象 `V/n` 的**轴归属** | 需真机/厂商证据（§8.1-5） | `drive.motor.temperature` 的语义边界写死为「该驱动对象的电机温度」，UI/告警**不得**宣称「X 轴电机温度」 |
| `r0031` 的**访问能力** | 需真机复测（§8.1-7） | 定义保留 + `candidate` + 访问能力 HOLD |
| `840d-sl` 的**定义目录** | 需真机/文档证据（§8.1-8） | 设备可选但**无可配点位**；拒绝时必须明确报「该系列无定义」 |
| 19 条参数的**手册页码** | 可离线核实（§8.1-6） | 未提供前一律 `parameter_identity`，ID 不得升级为业务语义名 |
| **`DIGESTS.lock` 的 CI 强制形式** | 实施设计（原 §8-8） | 锁文件格式、CI 步骤、与 `scripts/contract_suites.py` 的关系在 **PR-B** 明确 |
| **`nck_native` 是否服务端限权** | 产品裁决（§8.1-9） | 本期只读约束 + 前端警告 |

**范围外（明确不做）**：840D sl 定义、用户导入定义、Core 变量知识库、Core 编译快照（v1）、新增 proto/IPC 消息、新增 `FieldType`、新增 NCK area 字母、真机/网络访问（见附录 A）。

---

## 附录 A：本设计**未**引入的东西（防止范围蔓延）

| 未引入 | 原因 |
|---|---|
| Core 变量知识库 / 变量名清单 | 决策 3：定义来源是 Driver 内置，Core 不硬编码 Siemens 知识 |
| Core 侧的 Siemens Target 类型与 Point Kind 白名单 | 修订 1：`TargetRef`/`DriveEntry` 属 Driver（`targets.rs`）；Core 只要求 `kind` 非空字符串 |
| 用户导入定义 | 决策 3 |
| Core 编译快照 | 版本策略：第一版不引入；若将来驱动可能删除旧 revision 再评估，且必须明确唯一执行真值、不一致即拒绝运行 |
| 新的 proto/IPC 消息或字段 | B3：kind/config 已透传 |
| 新的 `FieldType` / `SchemaDescriptor` 扩展 | B4/B5/B6：Descriptor 2.0 冻结面 |
| 新的 NCK area 字母（如 `S<n>`） | 决策 5 + N11：不得虚构未经验证的原生地址区域 |
| 真机/网络访问 | 本设计为纯离线设计；真机验证属后续独立 Gate |
