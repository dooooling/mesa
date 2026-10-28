# 828D 预设模板映射表（62 个原生预设 → 42 条定义）

> **Phase 0 子集**只读盘点产物：只读 `drivers/sinumerik-nck/presets/828d.json` 与 `drivers/sinumerik-nck/definitions/828d/r1.json`，
> **未改动**任何代码/定义/测试/配置，未连设备。本表不做任何"升级"或准入判定。

## 0. 基线

| 项 | 值 |
|---|---|
| 仓库 HEAD | `047e2064bd233eafc508ca9fe5329e93a96ee6e2` |
| `drivers/sinumerik-nck/definitions/828d/r1.json` Blob SHA-1 | `3c0abe67cbcb64b525cddec1584e6e1747e0f062`（`git ls-tree` = `git hash-object` = 远端 HEAD 同 commit） |
| 预设数据源 | `drivers/sinumerik-nck/presets/828d.json`（`version=1`，`scope` = "配置预填模板；不参与 Configure/Poll 准入，不构成硬件验证结论"） |
| 预设总数 | 62 |
| `validation_status` | {"candidate": 62}（**62 条全部 candidate**） |
| 定义总数 | 42 |

## 1. 匹配依据（唯一判据：`golden_ref` 四元组）

预设 id 的形态是 `828d.<area>.<block>.<variable>.<entry>`，与定义里
`golden_ref[]` 的 `(area, block, variable, entry)` 四元组**同构**。
这条链路不是本盘点发明的，而是仓库里已经在跑的运行时索引：

- `drivers/sinumerik-nck/src/presets.rs:230-239` —— `golden_ref_of_template()`：
  把模板 id 按 `.` 切成 5 段，第 5 段经 `DriveEntry::parse` 转成 `DriveEntry`，
  组成 `GoldenRef`；
- `drivers/sinumerik-nck/src/presets.rs:242-245` —— `semantic_binding()`：
  `catalog.find_by_golden_ref(&want).first()`，即"模板 id → GoldenRef → 定义目录"；
- `drivers/sinumerik-nck/src/definition.rs:552-556` —— `find_by_golden_ref()`：
  按 `GoldenRef::key()` 四元组过滤定义；
- `drivers/sinumerik-nck/src/definition.rs:252-258` —— 注释明确写：
  「42 条定义共产生 **62** 个 GoldenRef（A 16×1 + B 20×2 + C 6×1）」，
  `drive.*` 每条定义覆盖 `nc_variable` 与 `drive_object` **两条**条目。

因此本表**不按变量名、不按参数号做模糊匹配，也不用地址猜测**：
以 `golden_ref` 四元组命中为准。辅助判据（仅用于交叉一致性核对，不用于判定映射）
是原生地址 `(area, module, column)`；模块按入口换算
（`nc_variable → 26`、`drive_object → 130`，见 `drivers/sinumerik-nck/src/targets.rs:88`
注释「驱动对象入口身份：`nc_variable` → module 26，`drive_object` → module 130」）。

## 2. 结论（一句话）

**62 个预设全部能映射到 `r1.json` 的定义：映射成功 62 条，无法映射 0 条；
但只落到 42 个不同的 `variable_id` 上（`drive.*` 的 20 条参数各被 2 个预设共用）。**

双向一一对应校验（脚本可复现）：

| 校验 | 结果 |
|---|---|
| 定义侧 `golden_ref` 条目总数 | 62 |
| 预设 id 集合 − `golden_ref` 集合 | `[]`（空） |
| `golden_ref` 集合 − 预设 id 集合 | `[]`（空） |
| 是否为**双射**（62 = 62） | **是** |
| 被判 `mapped` 的预设数 | 62 / 62 |
| 被判 `unmapped` 的预设数 | 0 |
| 命中不同 `variable_id` 数 | 42 |
| 未被任何预设命中的定义 | 0（`[]`） |
| 原生字段一致性核对对数 | 62 |
| 一致性失败字段计数 | {"module_ok": 0, "column_ok": 0, "decode_ok": 0, "transport_ok": 0, "count_ok": 0, "unit_mode_ok": 0} |

一致性核对把预设 `parameters`（module/column/decode/transport_size/count/unit_mode）
与命中定义的 `addressing`/`decode`/`transport_size`/`count`/`unit_mode` 逐字段比对，
**62 对全部通过**（module 按入口换算后比对）。

## 3. ⚠️ 62 个候选预设 ≠ 62 个常用且已验证变量

这是本表最重要的一句话，逐层拆开：

### 3.1 数量层

62 个预设 → **42** 个 `variable_id`（20 条 `drive.*` 定义各占 2 个预设，
分别是 `nc_variable` 入口与 `drive_object` 入口）。

### 3.2 状态层

42 条定义里 `verified` 24 / `parameter_identity` 16 / `candidate` 2。
按预设计数（一个预设算一条）：

| 命中定义的 semantics_status | 预设数 |
|---|---|
| `candidate` | 4 |
| `parameter_identity` | 32 |
| `verified` | 26 |

按（状态 × 入口）拆开：

| semantics_status | entry | 预设数 |
|---|---|---|
| `candidate` | `drive_object` | 2 |
| `candidate` | `nc_variable` | 2 |
| `parameter_identity` | `drive_object` | 16 |
| `parameter_identity` | `nc_variable` | 16 |
| `verified` | `drive_object` | 2 |
| `verified` | `nc_variable` | 24 |

### 3.3 预设自身状态层

**62 条 `validation_status` 全部是 `candidate`**
（`drivers/sinumerik-nck/src/presets.rs:39-40` 注释也写明「候选状态（当前恒
`candidate`；不得据此宣称硬件已验证）」）。预设表里没有任何一条是"已验证"。

### 3.4 准入层

`presets/828d.json` 顶层 `scope` 自述
「配置预填模板；不参与 Configure/Poll 准入，不构成硬件验证结论」；
`drivers/sinumerik-nck/src/presets.rs:1-19` 也明确模板"只提供结构固定字段的预填"、
`area_no` 与 `line` 一律留给使用者确认。
每个预设的 `required_user_inputs` 实际值均为 `["area_no", "line"]`
（62/62，脚本可复现）。

**所以：映射成功只证明"模板 id 能在定义目录里找到同一四元组"，
不证明该预设的实例/行索引正确，也不证明设备可读，更不能把这些预设改标成常用/已验证。**

## 4. 62 条映射明细

| # | 预设 id | entry | 名称（label） | 映射 variable_id | rev | scope | semantics_status | 匹配依据 | 字段一致性 |
|---|---|---|---|---|---|---|---|---|---|
| 1 | `828d.C.S.progStatus.nc_variable` | `nc_variable` | S/progStatus U16_LE | `channel.program.status` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 2 | `828d.B.S.opMode.nc_variable` | `nc_variable` | S/opMode U16_LE | `mode_group.op_mode` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 3 | `828d.C.SPARPP.progName.nc_variable` | `nc_variable` | SPARPP/progName UTF8_NUL_LAST_32 | `channel.program.name.main` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 4 | `828d.C.SPARP.progName.nc_variable` | `nc_variable` | SPARP/progName UTF8_NUL_LAST_32 | `channel.program.name.active` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 5 | `828d.C.SPARP.actLineNumber.nc_variable` | `nc_variable` | SPARP/actLineNumber I32_LE | `channel.program.line_number` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 6 | `828d.C.SPARP.actBlock.nc_variable` | `nc_variable` | SPARP/actBlock UTF8_NUL_FIRST_66 | `channel.program.block` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 7 | `828d.C.SGA.actToolBasePos.nc_variable` | `nc_variable` | SGA/actToolBasePos F64_LE | `axis.position.commanded_mcs` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 8 | `828d.C.SEGA.actProgPosBKS.nc_variable` | `nc_variable` | SEGA/actProgPosBKS F64_LE | `axis.position.actual_bcs` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 9 | `828d.C.S.actFeedRateIpo.nc_variable` | `nc_variable` | S/actFeedRateIpo F64_LE | `channel.feed.actual` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 10 | `828d.C.S.cmdFeedRateIpo.nc_variable` | `nc_variable` | S/cmdFeedRateIpo F64_LE | `channel.feed.commanded` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 11 | `828d.C.S.feedRateIpoOvr.nc_variable` | `nc_variable` | S/feedRateIpoOvr F64_LE | `channel.feed.override` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 12 | `828d.C.SSP.actSpeed.nc_variable` | `nc_variable` | SSP/actSpeed F64_LE | `spindle.speed.actual` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 13 | `828d.C.SSP.cmdSpeed.nc_variable` | `nc_variable` | SSP/cmdSpeed F64_LE | `spindle.speed.commanded` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 14 | `828d.C.SSP.speedOvr.nc_variable` | `nc_variable` | SSP/speedOvr F64_LE | `spindle.speed.override` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 15 | `828d.C.S.actTNumber.nc_variable` | `nc_variable` | S/actTNumber U16_LE | `channel.tool.number_active` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 16 | `828d.C.RP.rpa.nc_variable` | `nc_variable` | RP/rpa F64_LE | `channel.r_parameter` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 17 | `828d.V.M.r0021.nc_variable` | `nc_variable` | M/r0021 F32 | `drive.parameter.r0021` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 18 | `828d.V.M.r0021.drive_object` | `drive_object` | M/r0021 F32（驱动对象入口 module130） | `drive.parameter.r0021` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 19 | `828d.V.M.r0027.nc_variable` | `nc_variable` | M/r0027 F32 | `drive.parameter.r0027` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 20 | `828d.V.M.r0027.drive_object` | `drive_object` | M/r0027 F32（驱动对象入口 module130） | `drive.parameter.r0027` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 21 | `828d.V.M.r0035.nc_variable` | `nc_variable` | M/r0035 F32（含 -200 无效温度规则） | `drive.motor.temperature` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 22 | `828d.V.M.r0035.drive_object` | `drive_object` | M/r0035 F32（驱动对象入口 module130，含 -200 无效温度规则） | `drive.motor.temperature` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 23 | `828d.V.M.r0033.nc_variable` | `nc_variable` | M/r0033 F32 | `drive.parameter.r0033` | 1 | collect | candidate | golden_ref 四元组 | 6 字段全通过 |
| 24 | `828d.V.M.r0033.drive_object` | `drive_object` | M/r0033 F32（驱动对象入口 module130） | `drive.parameter.r0033` | 1 | collect | candidate | golden_ref 四元组 | 6 字段全通过 |
| 25 | `828d.V.M.r0031.nc_variable` | `nc_variable` | M/r0031 F32 | `drive.parameter.r0031` | 1 | collect | candidate | golden_ref 四元组 | 6 字段全通过 |
| 26 | `828d.V.M.r0031.drive_object` | `drive_object` | M/r0031 F32（驱动对象入口 module130） | `drive.parameter.r0031` | 1 | collect | candidate | golden_ref 四元组 | 6 字段全通过 |
| 27 | `828d.V.M.r0062.nc_variable` | `nc_variable` | M/r0062 F32 | `drive.parameter.r0062` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 28 | `828d.V.M.r0062.drive_object` | `drive_object` | M/r0062 F32（驱动对象入口 module130） | `drive.parameter.r0062` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 29 | `828d.V.M.r0063.nc_variable` | `nc_variable` | M/r0063 F32 | `drive.parameter.r0063` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 30 | `828d.V.M.r0063.drive_object` | `drive_object` | M/r0063 F32（驱动对象入口 module130） | `drive.parameter.r0063` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 31 | `828d.V.M.r0064.nc_variable` | `nc_variable` | M/r0064 F32 | `drive.parameter.r0064` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 32 | `828d.V.M.r0064.drive_object` | `drive_object` | M/r0064 F32（驱动对象入口 module130） | `drive.parameter.r0064` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 33 | `828d.V.M.r0077.nc_variable` | `nc_variable` | M/r0077 F32 | `drive.parameter.r0077` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 34 | `828d.V.M.r0077.drive_object` | `drive_object` | M/r0077 F32（驱动对象入口 module130） | `drive.parameter.r0077` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 35 | `828d.V.M.r0078.nc_variable` | `nc_variable` | M/r0078 F32 | `drive.parameter.r0078` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 36 | `828d.V.M.r0078.drive_object` | `drive_object` | M/r0078 F32（驱动对象入口 module130） | `drive.parameter.r0078` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 37 | `828d.V.M.r0079.nc_variable` | `nc_variable` | M/r0079 F32 | `drive.parameter.r0079` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 38 | `828d.V.M.r0079.drive_object` | `drive_object` | M/r0079 F32（驱动对象入口 module130） | `drive.parameter.r0079` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 39 | `828d.V.M.r0080.nc_variable` | `nc_variable` | M/r0080 F32 | `drive.parameter.r0080` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 40 | `828d.V.M.r0080.drive_object` | `drive_object` | M/r0080 F32（驱动对象入口 module130） | `drive.parameter.r0080` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 41 | `828d.V.M.r0081.nc_variable` | `nc_variable` | M/r0081 F32 | `drive.parameter.r0081` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 42 | `828d.V.M.r0081.drive_object` | `drive_object` | M/r0081 F32（驱动对象入口 module130） | `drive.parameter.r0081` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 43 | `828d.V.M.r0082.nc_variable` | `nc_variable` | M/r0082 F32 | `drive.parameter.r0082` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 44 | `828d.V.M.r0082.drive_object` | `drive_object` | M/r0082 F32（驱动对象入口 module130） | `drive.parameter.r0082` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 45 | `828d.V.M.r0103.nc_variable` | `nc_variable` | M/r0103 U16_BE | `drive.parameter.r0103` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 46 | `828d.V.M.r0103.drive_object` | `drive_object` | M/r0103 U16_BE（驱动对象入口 module130） | `drive.parameter.r0103` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 47 | `828d.V.M.p0105.nc_variable` | `nc_variable` | M/p0105 I16_BE | `drive.parameter.p0105` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 48 | `828d.V.M.p0105.drive_object` | `drive_object` | M/p0105 I16_BE（驱动对象入口 module130） | `drive.parameter.p0105` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 49 | `828d.V.M.r0106.nc_variable` | `nc_variable` | M/r0106 I16_BE | `drive.parameter.r0106` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 50 | `828d.V.M.r0106.drive_object` | `drive_object` | M/r0106 I16_BE（驱动对象入口 module130） | `drive.parameter.r0106` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 51 | `828d.V.M.r0107.nc_variable` | `nc_variable` | M/r0107 I16_BE | `drive.parameter.r0107` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 52 | `828d.V.M.r0107.drive_object` | `drive_object` | M/r0107 I16_BE（驱动对象入口 module130） | `drive.parameter.r0107` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 53 | `828d.V.M.r0108.nc_variable` | `nc_variable` | M/r0108 U32 | `drive.parameter.r0108` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 54 | `828d.V.M.r0108.drive_object` | `drive_object` | M/r0108 U32（驱动对象入口 module130） | `drive.parameter.r0108` | 1 | collect | parameter_identity | golden_ref 四元组 | 6 字段全通过 |
| 55 | `828d.V.M.p0199.nc_variable` | `nc_variable` | M/p0199 UTF16_BE | `drive.object.name` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 56 | `828d.V.M.p0199.drive_object` | `drive_object` | M/p0199 UTF16_BE（驱动对象入口 module130） | `drive.object.name` | 1 | collect | verified | golden_ref 四元组 | 6 字段全通过 |
| 57 | `828d.N.M.MDSA_AXCONF_MACHAX_NAME_TAB.nc_variable` | `nc_variable` | M/MDSA_AXCONF_MACHAX_NAME_TAB UTF8_FIXED_16 | `axis.name` | 1 | identity | verified | golden_ref 四元组 | 6 字段全通过 |
| 58 | `828d.A.M.MDCA_CTRLOUT_MODULE_NR.nc_variable` | `nc_variable` | M/MDCA_CTRLOUT_MODULE_NR U8 | `axis.ctrlout.module_number` | 1 | identity | verified | golden_ref 四元组 | 6 字段全通过 |
| 59 | `828d.A.M.MDCA_ENC_MODULE_NR.nc_variable` | `nc_variable` | M/MDCA_ENC_MODULE_NR U8 | `axis.encoder.module_number` | 1 | identity | verified | golden_ref 四元组 | 6 字段全通过 |
| 60 | `828d.A.M.MDCA_CTRLOUT_TYPE.nc_variable` | `nc_variable` | M/MDCA_CTRLOUT_TYPE U8 | `axis.ctrlout.type` | 1 | identity | verified | golden_ref 四元组 | 6 字段全通过 |
| 61 | `828d.C.M.AXCONF_MACHAX_USED.nc_variable` | `nc_variable` | M/AXCONF_MACHAX_USED U8 | `channel.machine_axis_assignment` | 1 | identity | verified | golden_ref 四元组 | 6 字段全通过 |
| 62 | `828d.A.M.SPIND_ASSIGN_TO_MACHAX.nc_variable` | `nc_variable` | M/SPIND_ASSIGN_TO_MACHAX U8 | `axis.spindle_assignment` | 1 | identity | verified | golden_ref 四元组 | 6 字段全通过 |

## 5. 无法映射的预设（单独列出）

**空集：0 条。** 62 个预设全部命中 `golden_ref` 四元组，无一落到 `unmapped`。

处置原则（即使本表为空也写明，供后续新增预设时套用）：

> **无法映射的预设一律保留为候选，不自动升级为常用变量。**
> 不允许用"名字相似""参数号相同""地址相邻"反推定义；
> 也不允许为该预设反向新增定义（`drivers/sinumerik-nck/src/presets.rs:8-9`
> 明确"语义索引走定义目录的 `golden_ref`，不由模板反向生成定义（禁止自证循环）"）。

## 6. 必须一并阅读的限制

- **module 26 与 module 130 的实例编号不可互换。** 20 条 `drive.*` 参数各有两条入口，
  同一 `V/n` 在两条入口下指向不同对象；`presets/828d.json` 也为此拆成两个独立模板
  （`drivers/sinumerik-nck/src/presets.rs:16-17`；
  `drivers/sinumerik-nck/src/identity.rs:224-227、333-335、443-447、470-472`
  均有"编号不跨入口复用"的显式过滤）。
  本表 `field_consistency` 里 `module_expected` 一列即按入口换算的期望值，
  **它是一致性核对，不是"两条入口等价"的证据**。
- 定义侧 `drive.*` 的 `addressing.module` 是 `drive_entry_module`（不是固定数字），
  具体值由 `drive_object.entry` 在运行期决定；因此这 20 条定义**不能**用
  `(area, module, column)` 唯一匹配，只能靠 `golden_ref` 四元组区分入口。
- 预设模块与入口换算对照（62 条全量，`✅` = 换算后与预设 `module` 相同）：

| 预设 id | 定义 module 类别 | 换算后期望 module | 预设 module | 一致 |
|---|---|---|---|---|
| `828d.C.S.progStatus.nc_variable` | fixed | 127 | 127 | ✅ |
| `828d.B.S.opMode.nc_variable` | fixed | 127 | 127 | ✅ |
| `828d.C.SPARPP.progName.nc_variable` | fixed | 122 | 122 | ✅ |
| `828d.C.SPARP.progName.nc_variable` | fixed | 125 | 125 | ✅ |
| `828d.C.SPARP.actLineNumber.nc_variable` | fixed | 125 | 125 | ✅ |
| `828d.C.SPARP.actBlock.nc_variable` | fixed | 125 | 125 | ✅ |
| `828d.C.SGA.actToolBasePos.nc_variable` | fixed | 115 | 115 | ✅ |
| `828d.C.SEGA.actProgPosBKS.nc_variable` | fixed | 112 | 112 | ✅ |
| `828d.C.S.actFeedRateIpo.nc_variable` | fixed | 127 | 127 | ✅ |
| `828d.C.S.cmdFeedRateIpo.nc_variable` | fixed | 127 | 127 | ✅ |
| `828d.C.S.feedRateIpoOvr.nc_variable` | fixed | 127 | 127 | ✅ |
| `828d.C.SSP.actSpeed.nc_variable` | fixed | 114 | 114 | ✅ |
| `828d.C.SSP.cmdSpeed.nc_variable` | fixed | 114 | 114 | ✅ |
| `828d.C.SSP.speedOvr.nc_variable` | fixed | 114 | 114 | ✅ |
| `828d.C.S.actTNumber.nc_variable` | fixed | 127 | 127 | ✅ |
| `828d.C.RP.rpa.nc_variable` | fixed | 21 | 21 | ✅ |
| `828d.V.M.r0021.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0021.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0027.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0027.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0035.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0035.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0033.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0033.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0031.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0031.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0062.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0062.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0063.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0063.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0064.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0064.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0077.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0077.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0078.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0078.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0079.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0079.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0080.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0080.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0081.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0081.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0082.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0082.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0103.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0103.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.p0105.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.p0105.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0106.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0106.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0107.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0107.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.r0108.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.r0108.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.V.M.p0199.nc_variable` | drive_entry_module | 26 | 26 | ✅ |
| `828d.V.M.p0199.drive_object` | drive_entry_module | 130 | 130 | ✅ |
| `828d.N.M.MDSA_AXCONF_MACHAX_NAME_TAB.nc_variable` | fixed | 26 | 26 | ✅ |
| `828d.A.M.MDCA_CTRLOUT_MODULE_NR.nc_variable` | fixed | 26 | 26 | ✅ |
| `828d.A.M.MDCA_ENC_MODULE_NR.nc_variable` | fixed | 26 | 26 | ✅ |
| `828d.A.M.MDCA_CTRLOUT_TYPE.nc_variable` | fixed | 26 | 26 | ✅ |
| `828d.C.M.AXCONF_MACHAX_USED.nc_variable` | fixed | 26 | 26 | ✅ |
| `828d.A.M.SPIND_ASSIGN_TO_MACHAX.nc_variable` | fixed | 26 | 26 | ✅ |

- 预设里 `unit`（6 条）、`invalid_value`/`invalid_tolerance`（2 条）等字段是**预填提示**，
  与定义侧的 `unit` / `invalid_rule` 分属两套数据（定义是本盘点的判定基准）。
  本盘点不把预设字段当作定义修正建议。

## 7. 待核实

- **待核实**：62 个预设的 `area_no` / `line` 全部留空，实际实例与行索引须按设备确认
  （`presets/828d.json` 顶层 `scope` 与每条 `required_user_inputs` 均如此声明）。
- **待核实**：预设 `module` 数值（26/127/122/125/115/112/114/21 等）的物理含义
  （NC 变量入口为 26 等）目前只有代码与证据文件支撑，**手册章节/页码未找到**
  （见 `docs/reference/828d-definition-source-audit.md` §2.7）。
- **未找到**：任何预设与"常用变量清单"的对应关系；仓库内不存在这种清单。

## 附录 A：统计脚本全文（可原样复现）

```python
# -*- coding: utf-8 -*-
"""Phase 0 交付2：62 个原生预设 → r1.json 42 条定义的映射盘查（只读）

用法: python map_presets.py <presets/828d.json> <definitions/828d/r1.json> <out.json>

匹配依据（按优先级，逐条显式记录在 match_basis 里）：
  M1 = golden_ref 四元组全匹配 (area, block, variable, entry)。
       预设 id 形如 828d.<area>.<block>.<variable>.<entry>，四元组即 GoldenRef.key()。
       这正是 presets.rs:242 semantic_binding() + definition.rs:552 find_by_golden_ref()
       的运行时索引链，是**权威依据**（代码可执行，不依赖人工判断）。
  M2 = addressing 全匹配 (area, module 固定值, column)：仅当定义 module 为 fixed 时才可能。
  M3 = addressing 弱匹配 (area, column)：定义 module 为 drive_entry_module，无法确定具体值。
  none = 无任何命中。

一致性核对（只记录、不判定）：把预设 parameters 与命中定义的 addressing/decode/
transport_size/count 逐字段比对，module 按 entry 换算（nc_variable→26，drive_object→130，
见 targets.rs:88）。
"""
import json
import sys
from collections import Counter, OrderedDict

presets_path, r1_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]

presets_doc = json.load(open(presets_path, encoding="utf-8"))
r1 = json.load(open(r1_path, encoding="utf-8"))
defs = r1["definitions"]

ENTRY_MODULE = {"nc_variable": 26, "drive_object": 130}   # targets.rs:88


def parse_preset_id(pid):
    parts = pid.split(".")
    if len(parts) == 5 and parts[0] == "828d":
        return parts[1], parts[2], parts[3], parts[4]
    return None


def addr_module_value(d):
    m = (d.get("addressing") or {}).get("module") or {}
    if m.get("source") == "fixed":
        return m.get("value"), "fixed"
    return None, m.get("source")


rows = []
for t in presets_doc["templates"]:
    pid = t["id"]
    p = t.get("parameters") or {}
    parsed = parse_preset_id(pid)
    if parsed is None:
        rows.append(OrderedDict([("preset_id", pid), ("verdict", "bad_id"),
                                 ("match_basis", "none"), ("matched_variable_ids", [])]))
        continue
    area, block, variable, entry = parsed

    # ---- M1: golden_ref 四元组 ----
    m1 = [d for d in defs
          if any(g.get("area") == area and g.get("block") == block
                 and g.get("variable") == variable and g.get("entry") == entry
                 for g in (d.get("golden_ref") or []))]

    # ---- M2/M3: addressing ----
    m2, m3 = [], []
    for d in defs:
        a = d.get("addressing") or {}
        mod_val, mod_kind = addr_module_value(d)
        if mod_kind == "fixed":
            if (a.get("area") == area and mod_val == p.get("module")
                    and a.get("column") == p.get("column")):
                m2.append(d["variable_id"])
        elif a.get("area") == area and a.get("column") == p.get("column"):
            m3.append(d["variable_id"])

    if m1:
        verdict, basis = "mapped", "M1:golden_ref(area/block/variable/entry)"
        matched = [d["variable_id"] for d in m1]
    elif m2:
        verdict, basis = "mapped_addr_only", "M2:addressing(area/module/column)"
        matched = sorted(set(m2))
    elif m3:
        verdict, basis = "mapped_addr_weak", "M3:addressing(area/column)"
        matched = sorted(set(m3))
    else:
        verdict, basis = "unmapped", "none"
        matched = []

    matched_defs = [d for d in defs if d["variable_id"] in matched]
    checks = []
    for d in matched_defs:
        a = d.get("addressing") or {}
        mod_val, mod_kind = addr_module_value(d)
        expect_module = ENTRY_MODULE.get(entry) if mod_kind == "drive_entry_module" else mod_val
        checks.append(OrderedDict([
            ("variable_id", d["variable_id"]),
            ("module_kind", mod_kind),
            ("module_expected", expect_module),
            ("module_preset", p.get("module")),
            ("module_ok", expect_module == p.get("module")),
            ("column_def", a.get("column")),
            ("column_preset", p.get("column")),
            ("column_ok", a.get("column") == p.get("column")),
            ("decode_def", d.get("decode")),
            ("decode_preset", p.get("decode")),
            ("decode_ok", d.get("decode") == p.get("decode")),
            ("transport_def", d.get("transport_size")),
            ("transport_preset", p.get("transport_size")),
            ("transport_ok", ("0x%02x" % d["transport_size"]) == str(p.get("transport_size")).lower()),
            ("count_def", d.get("count")),
            ("count_preset", p.get("count")),
            ("count_ok", d.get("count") == p.get("count")),
            ("unit_mode_def", d.get("unit_mode")),
            ("unit_mode_preset", p.get("unit_mode")),
            ("unit_mode_ok", d.get("unit_mode") == p.get("unit_mode")),
        ]))

    rows.append(OrderedDict([
        ("preset_id", pid),
        ("label", t.get("label")),
        ("family", t.get("family")),
        ("validation_status", t.get("validation_status")),
        ("parameters", p),
        ("required_user_inputs", t.get("required_user_inputs")),
        ("parsed_from_id", OrderedDict([("area", area), ("block", block),
                                        ("variable", variable), ("entry", entry)])),
        ("match_basis", basis),
        ("verdict", verdict),
        ("matched_variable_ids", matched),
        ("matched_definition_meta", [OrderedDict([
            ("variable_id", d["variable_id"]),
            ("revision", d["revision"]),
            ("scope", d["scope"]),
            ("semantics_status", d["semantics_status"]),
            ("golden_ref_entries", [g.get("entry") for g in (d.get("golden_ref") or [])]),
        ]) for d in matched_defs]),
        ("field_consistency", checks),
        ("addr_only_candidates", sorted(set(m2))),
        ("addr_weak_candidates", sorted(set(m3))),
    ]))

verdicts = Counter(r["verdict"] for r in rows)
hit_ids = set()
for r in rows:
    if r["verdict"].startswith("mapped"):
        hit_ids.update(r["matched_variable_ids"])
unhit = [d["variable_id"] for d in defs if d["variable_id"] not in hit_ids]

all_checks = [c for r in rows for c in r.get("field_consistency") or []]
check_fail = OrderedDict([
    ("module_ok", sum(1 for c in all_checks if not c["module_ok"])),
    ("column_ok", sum(1 for c in all_checks if not c["column_ok"])),
    ("decode_ok", sum(1 for c in all_checks if not c["decode_ok"])),
    ("transport_ok", sum(1 for c in all_checks if not c["transport_ok"])),
    ("count_ok", sum(1 for c in all_checks if not c["count_ok"])),
    ("unit_mode_ok", sum(1 for c in all_checks if not c["unit_mode_ok"])),
])

# 每个 variable_id 被几个预设命中（drive.* 应为 2，其余 1）
per_def = Counter()
for r in rows:
    for vid in r["matched_variable_ids"]:
        per_def[vid] += 1

# 双向一一对应校验：预设 id 集合 vs 定义 golden_ref 四元组集合
gr_ids = set()
gr_total = 0
for d in defs:
    for g in d.get("golden_ref") or []:
        gr_total += 1
        gr_ids.add("828d.%s.%s.%s.%s" % (g["area"], g["block"], g["variable"], g["entry"]))
preset_ids = set(t["id"] for t in presets_doc["templates"])

summary = OrderedDict([
    ("preset_count", len(rows)),
    ("definition_count", len(defs)),
    ("golden_ref_total", gr_total),
    ("preset_minus_golden_ref", sorted(preset_ids - gr_ids)),
    ("golden_ref_minus_preset", sorted(gr_ids - preset_ids)),
    ("bijection", (preset_ids == gr_ids)),
    ("verdict_counts", OrderedDict(sorted(verdicts.items()))),
    ("match_basis_counts", OrderedDict(sorted(Counter(r["match_basis"].split(":")[0] for r in rows).items()))),
    ("mapped_count", sum(1 for r in rows if r["verdict"].startswith("mapped"))),
    ("unmapped_count", verdicts.get("unmapped", 0) + verdicts.get("bad_id", 0)),
    ("distinct_variable_ids_hit", len(hit_ids)),
    ("definitions_hit_by_presets", len(hit_ids)),
    ("definitions_not_hit_by_any_preset", len(unhit)),
    ("definitions_not_hit_by_any_preset_ids", unhit),
    ("presets_per_definition", OrderedDict(sorted(per_def.items()))),
    ("preset_validation_status_counts",
     OrderedDict(sorted(Counter(r["validation_status"] for r in rows).items()))),
    ("field_consistency_failures", check_fail),
    ("field_consistency_checked_pairs", len(all_checks)),
])

json.dump(OrderedDict([("summary", summary), ("presets", rows)]),
          open(out_path, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
print("OK", out_path)
```

复现命令（仓库根目录）：

```bash
python map_presets.py drivers/sinumerik-nck/presets/828d.json \
       drivers/sinumerik-nck/definitions/828d/r1.json presets.json
```

## 附录 B：机器可读原始数据

`docs/reference/828d-preset-mapping.json`（`summary` + `presets` 两个顶层键，
逐条含 `parameters` 原值、`match_basis`、`matched_variable_ids`、`field_consistency`）。
