# NCK 目录导航分类（驱动侧临时规则）

> **本文档描述的是「导航规则」，不是手册分类。**
> 828D 的四份手册（NC 变量清单 / 系统变量 / 机床数据 / S120 参数手册）都**不提供**
> 「这条变量属于哪一类」的字段；`r1.json` 也没有 `category` 且已冻结
> （blob `3c0abe67cbcb64b525cddec1584e6e1747e0f062`，本轮一字节未动）。
>
> 因此本轮的 `category` 只是**驱动侧为了前端建树而临时制定的导航规则**——它回答
> 「用户沿哪条路径能找到它」，**不回答**「手册说它是什么」。任何地方都**不得**把它
> 写成「手册分类」「已确认分类」或「权威分类」。
>
> Revision 1 的兼容投影使用 `category_basis="navigation_rule"`。方案 B 已在 revision 2 落地：最新定义的 category 已进入 digest 与 `DIGESTS.lock`，对应 `category_basis="definition"`。

## 1. 输出字段

`GET /api/v1/drivers/{id}/definitions` 的每条 `definitions[]` 新增两个字段
（其余既有字段的字义与形状**不变**）：

```json
{
  "variable_id": "channel.feed.actual",
  "…（既有 14 个字段逐字不变）",
  "category": "cnc_variable",
  "category_basis": "navigation_rule"
}
```

- `category`：`cnc_variable` / `system_variable` / `machine_data` /
  `drive_parameter` / `unclassified` 五码之一；
- `category_basis`：分类依据的**自证**标记，Revision 1 兼容投影取 `"navigation_rule"`；revision 2 正式定义取 `"definition"`。

两个字段都是**投影期计算**的附加字段：不参与 `digest`、不写回定义、不出现在
`DIGESTS.lock`；`identity-preview` 端点不受影响。

实现位置：

| 内容 | 位置 |
|---|---|
| 输出字段插入 | `drivers/sinumerik-nck/src/definition_api.rs:571-576` |
| 投影白名单外的说明 | `drivers/sinumerik-nck/src/definition_api.rs:59-62` |
| 分类词表 / 依据常量 | `drivers/sinumerik-nck/src/catalog_category.rs:48-84` |
| 显式映射表（42 条） | `drivers/sinumerik-nck/src/catalog_category.rs:153-456` |
| 证据锚点判据 | `drivers/sinumerik-nck/src/catalog_category.rs:92-110`、`:467-489` |
| 失败关闭入口 | `drivers/sinumerik-nck/src/catalog_category.rs:493-501` |
| 公开导出 | `drivers/sinumerik-nck/src/lib.rs:34-37` |

## 2. 分桶口径（导航层，不是手册层）

| 分类码 | 含义 | 判定证据 |
|---|---|---|
| `cnc_variable` | NC 变量清单（OPI 入口）里的通道 / 轴 / 主轴 / 方式组运行数据 | `source` 引 NC 变量清单（03/2025），寻址靠「块 + 列 + 行」 |
| `system_variable` | 用户可编程的系统变量（本轮仅 R 参数） | `golden_ref` 是 RP 块 `rpa`；R 参数在系统变量手册里定义为「系统变量 Rn」 |
| `machine_data` | 按 MD 编号定位的 NC 机床数据 | `golden_ref.variable` 为 MD 符号 **且** `addressing.column` 等于该 MD 编号 |
| `drive_parameter` | SINAMICS 驱动参数 `r*/p*` | `source` 引 S120/S150 参数手册，或 `golden_ref` 含 `drive_object` 入口 |
| `unclassified` | 其他 / 待分类 | **失败关闭**：未登记 ID 或证据锚点不成立时落这里，绝不猜 |

## 3. 42 条映射（逐条）

| # | variable_id | category | 一句话依据 |
|---|---|---|---|
| 1 | `channel.program.status` | `cnc_variable` | C/S 状态块 `progStatus`（NC 变量手册 p.306），通道运行数据 |
| 2 | `mode_group.op_mode` | `cnc_variable` | B/S `opMode`（p.89–90），方式组运行方式 |
| 3 | `channel.program.name.main` | `cnc_variable` | C/SPARPP `progName`，主程序名 |
| 4 | `channel.program.name.active` | `cnc_variable` | C/SPARP `progName`，活动程序名 |
| 5 | `channel.program.line_number` | `cnc_variable` | C/SPARP `actLineNumber`，当前行号 |
| 6 | `channel.program.block` | `cnc_variable` | C/SPARP `actBlock`，当前程序段 |
| 7 | `axis.position.commanded_mcs` | `cnc_variable` | C/SGA `actToolBasePos`，指令位置（NC 入口，非 MD） |
| 8 | `axis.position.actual_bcs` | `cnc_variable` | C/SEGA `actProgPosBKS`，实际位置 |
| 9 | `channel.feed.actual` | `cnc_variable` | C/S `actFeedRateIpo`，进给实际值（状态块项，非 $A_/$V_ 系统变量） |
| 10 | `channel.feed.commanded` | `cnc_variable` | C/S `cmdFeedRateIpo`，进给指令值 |
| 11 | `channel.feed.override` | `cnc_variable` | C/S `feedRateIpoOvr`，进给倍率 |
| 12 | `spindle.speed.actual` | `cnc_variable` | C/SSP `actSpeed`，主轴实际转速 |
| 13 | `spindle.speed.commanded` | `cnc_variable` | C/SSP `cmdSpeed`，主轴指令转速 |
| 14 | `spindle.speed.override` | `cnc_variable` | C/SSP `speedOvr`，主轴倍率 |
| 15 | `channel.tool.number_active` | `cnc_variable` | C/S `actTNumber`（p.277），当前刀具号 |
| 16 | `channel.r_parameter` | `system_variable` | C/RP 块 `rpa`，即 R 参数；系统变量手册把 Rn/$R[n] 定义为「系统变量」 |
| 17 | `drive.parameter.r0021` | `drive_parameter` | S120/S150 参数手册（11/2023）r0021 |
| 18 | `drive.parameter.r0027` | `drive_parameter` | 同上，r0027 |
| 19 | `drive.motor.temperature` | `drive_parameter` | 同上，r0035（含 p.71 无效值标记） |
| 20 | `drive.parameter.r0033` | `drive_parameter` | 同上，r0033（候选语义，不影响导航归类） |
| 21 | `drive.parameter.r0031` | `drive_parameter` | 同上，r0031（候选语义） |
| 22 | `drive.parameter.r0062` | `drive_parameter` | 同上，r0062 |
| 23 | `drive.parameter.r0063` | `drive_parameter` | 同上，r0063 |
| 24 | `drive.parameter.r0064` | `drive_parameter` | 同上，r0064 |
| 25 | `drive.parameter.r0077` | `drive_parameter` | 同上，r0077 |
| 26 | `drive.parameter.r0078` | `drive_parameter` | 同上，r0078 |
| 27 | `drive.parameter.r0079` | `drive_parameter` | 同上，r0079 |
| 28 | `drive.parameter.r0080` | `drive_parameter` | 同上，r0080 |
| 29 | `drive.parameter.r0081` | `drive_parameter` | 同上，r0081 |
| 30 | `drive.parameter.r0082` | `drive_parameter` | 同上，r0082 |
| 31 | `drive.parameter.r0103` | `drive_parameter` | 同上，r0103 |
| 32 | `drive.parameter.p0105` | `drive_parameter` | 同上，p0105（只读；`p` 前缀不改变导航归类） |
| 33 | `drive.parameter.r0106` | `drive_parameter` | 同上，r0106 |
| 34 | `drive.parameter.r0107` | `drive_parameter` | 同上，r0107 |
| 35 | `drive.parameter.r0108` | `drive_parameter` | 同上，r0108 |
| 36 | `drive.object.name` | `drive_parameter` | `V/M/p0199` 驱动对象名；无 manual 来源，靠 `drive_object` 入口定案 |
| 37 | `axis.name` | `machine_data` | `MDSA_AXCONF_MACHAX_NAME_TAB` = MD10000 |
| 38 | `axis.ctrlout.module_number` | `machine_data` | `MDCA_CTRLOUT_MODULE_NR` = MD30110 |
| 39 | `axis.encoder.module_number` | `machine_data` | `MDCA_ENC_MODULE_NR` = MD30220 |
| 40 | `axis.ctrlout.type` | `machine_data` | `MDCA_CTRLOUT_TYPE` = MD30130 |
| 41 | `channel.machine_axis_assignment` | `machine_data` | `AXCONF_MACHAX_USED` = MD20070（**反例**：`channel.` 前缀却是机床数据） |
| 42 | `axis.spindle_assignment` | `machine_data` | `SPIND_ASSIGN_TO_MACHAX` = MD35000 |

分桶统计：`cnc_variable` 15 + `system_variable` 1 + `machine_data` 6 +
`drive_parameter` 20 + `unclassified` 0 = 42。

## 4. `unclassified` 的判定

当前为 **0 条**，理由：42 条在显式表里逐条登记，且证据锚点在 `r1.json` 里逐条成立
（见 `drivers/sinumerik-nck/tests/catalog_category.rs` 的
`unclassified_bucket_is_empty_today`）。三条最容易被误判的边界条目都已由定义内证据
定案：

- `channel.r_parameter`：RP 块 `rpa` + 系统变量手册对 R 参数的措辞 ⇒ `system_variable`
  （NOTE：它同时出现在 NC 变量清单 5.3.15，两处都不构成「分类」，本轮按系统变量
  手册措辞归类，属导航选择）；
- `channel.machine_axis_assignment`：`channel.` 前缀，但符号 + MD20070 证明是机床数据；
- `drive.object.name`：无 manual 来源，但 `drive_object` 入口 + `p0199` 证明是驱动参数。

这个桶的真正用途是**失败关闭**，触发条件有三种（均有用例覆盖）：

1. 新增定义未在表里登记（哪怕 ID 形状像已有前缀）；
2. 表与定义漂移（证据锚点不成立，如手册来源被删、MD 编号与符号不一致）；
3. ID 被复用成别的对象。

## 5. Scheme B 已落地

`drivers/sinumerik-nck/definitions/828d/r2.json` 为 42 条定义发布正式 `category`，并通过 `DIGESTS.lock` 锁定摘要。此文档和 `catalog_category.rs` 只用于解释 revision 1 的历史导航规则；revision 2 以定义字段为唯一分类来源。
本轮**不改** `r1.json`、不改前端、不改 `proto/`、不改 `crates/`。

## 6. 测试

| 层 | 用例 | 覆盖 |
|---|---|---|
| 单元（`catalog_category.rs`） | `rules_cover_every_published_definition_exactly_once` | 表 ↔ 42 条一一对应、无重复 |
| 单元 | `table_never_declares_unclassified_directly` | 待分类只能由失败关闭产生 |
| 单元 | `every_embedded_definition_hits_its_anchor` | 42 条锚点全部成立 |
| 单元 | `unknown_variable_id_falls_back_to_unclassified` | 反例：未登记 ID 不猜 |
| 单元 | `anchor_mismatch_falls_back_to_unclassified` | 反例：锚点失效 / MD 编号不符不归类 |
| 单元 | `prefixes_do_not_determine_category` | 同前缀分属不同分类 |
| 集成（`tests/catalog_category.rs`） | `every_definition_declares_a_legal_navigation_category` | 最新 revision 的 42 条定义都有合法 category，`basis=definition`；revision 1 的兼容投影另行验证 `navigation_rule` |
| 集成 | `frozen_definitions_carry_no_category_field` | `r1.json` 里没有分类字段 |
| 集成 | `category_partition_is_pinned_entry_by_entry` | 每个桶逐条钉住（15/1/6/20/0） |
| 集成 | `unclassified_bucket_is_empty_today` | 待分类桶为空的断言与理由 |
| 集成 | `categories_are_backed_by_definition_evidence` | 分类跟随 `source` / `golden_ref` 证据 |
| 集成 | `unregistered_definition_cannot_be_forced_into_a_category` | 反例：无法归类者不被塞进四类 |
