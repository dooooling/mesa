# 828D 定义 ↔ 07/2023 手册逐条交叉引用（Phase 0 阶段 A）

本文件逐条回答一个问题：**r1.json 的 42 条定义，能否在三份 07/2023 手册 PDF 中找到对应条目，找到在第几页、结论是什么。**

一句话结论：**35 条有 manual 来源的定义中，19 条在 07/2023 手册中找到同名条目且语义/类型不冲突（属较早版本），1 条存在实质差异，13 条在三份 PDF 中完全没有该标识符，2 条只在正文被提及而无条目定义；`exact_match` 为 0，因为三份 PDF 都是 07/2023，早于所引的 03/2025 与 11/2023。**

> 本文件**不把 07/2023 页码写成现有定义的原始来源**。原始 manual reference 原样照抄，07/2023 只是“较早版本比对结果”。
> 机器可读同源数据：`docs/reference/828d-manual-cross-reference.json`；机械校验：`python docs/reference/check-manual-metadata.py`。

## 0. 基线

| 项 | 值 |
|---|---|
| 分支 / HEAD | `docs/phase0-828d-catalog-audit` / `c3945847dcc196e8cb969f08fe71b7b397edfd8d` |
| 基线 main | `047e2064bd233eafc508ca9fe5329e93a96ee6e2` |
| r1.json | `drivers/sinumerik-nck/definitions/828d/r1.json`，Blob SHA-1 `3c0abe67cbcb64b525cddec1584e6e1747e0f062`（**本轮未修改**） |
| 定义 / source 条目 | 42 / 90（frozen_vector 42、manual 35、measurement 7、identity_probe 6） |
| semantics_status | verified 24 / parameter_identity 16 / candidate 2 |
| scope | collect 36 / identity 6 |
| 手册 PDF | 三份 07/2023，共 1236 + 690 + 1414 = 3340 页，本地 `docs/reference/manuals/`（gitignore:76-77，不入库） |
| 抽取工具 | pypdf（本机 6.6.0）逐页 `extract_text()` |

## 1. 本轮纪律（评审可直接用于否决）

1. **不改 `r1.json`**、不改已发布定义、不改代码/测试 proto/crates/drivers；本轮只新增 `docs/reference/` 下的文档与校验脚本。
2. **不把 07/2023 追认成来源**：三份 PDF 均早于所引 03/2025 与 11/2023，任何页码都只能出现在本文件与 `sinumerik-828d-manuals.json` 中，不得回填 r1.json。
3. **找不到就写 `not_found`**：不猜页码、不用相近条目顶替、不把“语义近邻”算作匹配（近邻只写在 notes）。
4. **不确定就写 `pending_review`**：抽取或语义把握不足的，宁可挂起。
5. **不虚构**：页码、章节、条目原文均来自逐页抽取；章节名取自 PDF 书签（outline）或抽取到的标题行。

## 2. 定位方法与判据（可复核）

### 2.1 用哪个标识去定位（任务要求说明）

| 定义分组 | 实际使用的标识 | 为什么它，而不是别的 |
|---|---|---|
| 19 条 `drive.parameter.*` / `drive.motor.temperature` | `golden_ref[].variable`（即参数号 `r0021`/`r0035`/`p0105`…） | 手册条目头就是“参数号 + 参数名”，参数号在 PDF 中唯一且可直接按行首匹配 |
| 16 条 NC 变量 | `golden_ref[].variable`（驼峰名 `progStatus`/`opMode`/`actFeedRateIpo`…）+ 中文 label 交叉验证 | 这是西门子 NC 变量（OPI 变量）的对外名；`addressing`（C/S/col 3）对手册无用（手册不印地址表） |
| `channel.r_parameter` | `label`（“通道 R 参数”）+ `golden_ref.variable = rpa`（R-Parameter） | 该条的 `rpa` 是缩写而非手册条目名，只能靠中文条目名“通道专用计算参数（R / RG）”定位 |
| 7 条无 manual 来源 | `golden_ref[].variable`（`MDSA_AXCONF_MACHAX_NAME_TAB` 等）+ `addressing.column` 的 MD 编号/参数号 | 机床数据手册按 MD 编号（30110/30220/…）排序，符号名与编号成对出现 |

**明确排除**：`variable_id`（如 `drive.parameter.r0021`）是本仓库的命名，西门子手册里从不出现，因此从不作为检索串。

### 2.2 检索方式

- 三份 PDF 逐页抽取到临时缓存（不入库），对每个标识符做两轮检索：**(a) 行首条目头**（`^r0021 ...` / `^30110 CTRLOUT_MODULE_NR`），**(b) 任意位置命中**（用于发现正文提及）。
- 补做**不区分大小写**检索与**中文关键词**检索（“程序状态/运行方式/倍率/刀具号/程序名/计算参数”），确保不是大小写或译名导致的漏检。
- 命中页码同时记录 **PDF 页码**与**页脚印刷页码**，见 §4（结论：偏移 0）。
- 章节名来源：s120 与 md 用 PDF 书签 outline 的层级定位（`PdfReader.outline`），sysvar 用抽取到的目录页（p3–p4）。

### 2.3 五类结论判据（判据写死，便于复算）

| 结论 | 判据 |
|---|---|
| `exact_match` | 找到同名条目 + 语义一致 + **PDF 版本与所引来源版本一致**。本轮三份 PDF 全为 07/2023，故 **0 条** |
| `matching_older_version` | 找到同名条目 + 名称/数据类型/单位与定义不冲突 + 所在 PDF 是**更早版本**（07/2023）。只证明“较早版本中存在该条目” |
| `version_difference` | 找到条目，但定义的表述（语义/取值含义/单位/条件）与手册存在实质差异 |
| `not_found` | 该标识符在三份 PDF 中完全未出现，且没有名称可机械对应的条目（语义近邻只记 notes，不计入结论） |
| `pending_review` | 标识符确实出现在 PDF 正文里，但不构成条目定义；或语义需人工确认才能定性 |

> 判据的取舍说明：把“较早版本中同名同义”记为 `matching_older_version` 而不是 `exact_match`，是因为 07/2023 与 03/2025/11/2023 之间无法用本套 PDF 证明条目文本未变。若评审要求改为“同名同义即 exact_match”，只需改这一条判据，分布可一键复算（见 §10）。

### 2.4 五类分布（35 条 manual 来源）

| 结论 | 条数 | 条目 |
|---|---|---|
| `exact_match` | 0 | — |
| `matching_older_version` | 19 | `channel.r_parameter`, `drive.parameter.r0021`, `drive.parameter.r0027`, `drive.motor.temperature`, `drive.parameter.r0031`, `drive.parameter.r0062`… |
| `version_difference` | 1 | `drive.parameter.r0033` |
| `not_found` | 13 | `mode_group.op_mode`, `channel.program.name.main`, `channel.program.name.active`, `channel.program.line_number`, `channel.program.block`, `axis.position.commanded_mcs`… |
| `pending_review` | 2 | `channel.program.status`, `spindle.speed.commanded` |
| **合计** | **35** | |

7 条无 manual 来源定义的独立交叉引用结论分布（不计入上面的 35）：

| 结论 | 条数 |
|---|---|
| `exact_match` | 0 |
| `matching_older_version` | 6 |
| `version_difference` | 1 |
| `not_found` | 0 |
| `pending_review` | 0 |

按命中所在的 PDF 分组（35 条）：`828D_sysvar_lists` 1 条、`828D_md_lists` 2 条、`828D_S120_para_lists` 19 条、三份皆无命中（`not_found`）13 条。

## 3. 35 条逐条交叉引用

### 3.1 828D NC variables 03/2025 组（16 条，所引文档号 A5E53370586B AC）

原始 reference 照抄：`Siemens 828D NC variables 03/2025 A5E53370586B AC`（16 条全部相同）。

| 定义 | scope / semantics | 07/2023 中实际匹配到的串 | PDF | 页 | 章节 | 结论 |
|---|---|---|---|---|---|---|
| `channel.program.status` | collect / verified | `progStatus` | md_lists_man_0723 | p222, p223 | 5.2 通道专用 NC 机床数据（正文提及） | `pending_review` |
| `mode_group.op_mode` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.program.name.main` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.program.name.active` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.program.line_number` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.program.block` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `axis.position.commanded_mcs` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `axis.position.actual_bcs` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.feed.actual` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.feed.commanded` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.feed.override` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `spindle.speed.actual` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `spindle.speed.commanded` | collect / verified | `cmdSpeed` | md_lists_man_0723 | p39 | 4 显示机床数据（正文提及） | `pending_review` |
| `spindle.speed.override` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.tool.number_active` | collect / verified | `—` | —（三份均 0 命中） | — | — | `not_found` |
| `channel.r_parameter` | collect / verified | `R[ <索引 1> ]　系统变量 Rn 或 R[n] 是 Real 类型的通道专用计算参数` | sysvar_lists_man_0723 | p15 | 3.3 通道专用和 NC 全局计算参数（R / RG） | `matching_older_version` |

逐条说明：

- **`channel.program.status`**（pending_review）：三份 PDF 中唯一出现该标识符的位置是 md p222–223 的正文（异步子程序执行时不通过 OPI 变量“progStatus”和“chanStatus”显示）。该处只以字符串提及 OPI 变量名，未给出该变量的数据类型/单位/下标/读取方式，不构成条目定义，故不能作为 42 条定义的来源页码；需人工核对是否存在独立的 NC 变量（OPI 变量）清单文档。抽取文本可读，无 CID 乱码。
- **`mode_group.op_mode`**（not_found）：标识符 opMode 在三份 PDF 中均未出现（区分大小写检索为 0 命中）。语义近邻：sysvar p265–266 的 $P_OPMODE「选择的运行方式」（$P_ 前缀编程系统变量，且说明为“通过 PLC 选择的运行方式”），但其名称、访问入口与地址形态（B/S/col 3）均与本定义不同，按纪律不得顶替为匹配。
- **`channel.program.name.main`**（not_found）：progName 未出现。同页语境：md p39 出现另一个 OPI 变量名 lastBlockNoStr（功能块 SPARP/SPARPP），与本定义 golden_ref 的 progName 不是同一标识符，不顶替。
- **`channel.program.name.active`**（not_found）：progName（block=SPARP）未出现；与上一条同因，md p39 的 lastBlockNoStr 不足以定位“当前活动程序名”。
- **`channel.program.line_number`**（not_found）：actLineNumber 未出现。手册内“程序段号”语义由 lastBlockNoStr（md p39）等其它 OPI 名承载，名称不匹配，不顶替。
- **`channel.program.block`**（not_found）：actBlock 未出现。md p39 的 lastBlockNoStr 为不同标识符，按纪律不顶替。
- **`axis.position.commanded_mcs`**（not_found）：actToolBasePos 未出现；手册的轴位置类条目均为 $AA_/$VA_ 前缀编程系统变量，无 C/SGA 访问入口的 NC 变量名。
- **`axis.position.actual_bcs`**（not_found）：actProgPosBKS 未出现（含不区分大小写检索）。不顶替。
- **`channel.feed.actual`**（not_found）：actFeedRateIpo 未出现。语义近邻 $AC_FEED 类编程变量未与本定义地址对应，不顶替。
- **`channel.feed.commanded`**（not_found）：cmdFeedRateIpo 未出现。手册未提供 NC 变量接口名与 C/S 地址的映射表。
- **`channel.feed.override`**（not_found）：feedRateIpoOvr 未出现。语义近邻：sysvar p143 $AC_OVR「轨迹倍率」、p144 $AC_TOTAL_OVR「总轨迹倍率」；二者是该手册记录的编程系统变量，不是本定义所指 C/S/col 3 的 NC 变量接口项，按纪律不顶替。
- **`spindle.speed.actual`**（not_found）：actSpeed 未出现。手册的主轴转速条目为 $AA_/  $AC_ 类编程变量，非 SSP 块 NC 变量名。
- **`spindle.speed.commanded`**（pending_review）：md p39 出现「该位涉及功能块 SPARPP 中的 OPI 变量 cmdSpeed」，说明该标识符确属本手册体系的 OPI 变量名，但（a）无独立条目给出类型/单位/下标，（b）手册把它归到功能块 SPARPP，而 r1 定义把 cmdSpeed 落在 block=SSP，二者归属不一致，需人工核对后方可采用。
- **`spindle.speed.override`**（not_found）：speedOvr 未出现。语义近邻：sysvar p1143 $AA_SPEED_OVR「主轴倍率」、p989 $AA_OVR「轴的倍率」；不同访问入口，不顶替。
- **`channel.tool.number_active`**（not_found）：actTNumber 未出现；中文关键词“刀具号”在三份 PDF 的条目头中亦无命中（命中项均为刀具数据表，不含该 NC 变量名）。
- **`channel.r_parameter`**（matching_older_version）：按中文标签“通道 R 参数”＋ golden_ref.variable=rpa（R-Parameter）定位到 sysvar §3.3（p15）：手册明确 R[n] 为通道专用、Real/DOUBLE、可掉电保持、数量由 MD28050 $MC_MM_NUM_R_PARAM 定义，且“NC 变量”读取属性为 X（可通过 NC 变量/OPI 读取），与定义 decode=F64_LE（8 字节 DOUBLE）一致。但手册未给出 C/RP/col 1 的地址形态与 line=index+1 的映射，该部分仍只能由 03/2025 版 NC 变量文档支撑。本节为 07/2023 较早版本，故记为 matching_older_version。

### 3.2 SINAMICS S120/S150 参数手册 11/2023 组（19 条）

原始 reference 照抄（18 条 + 1 条 p.71 变体）：

- `SINAMICS S120/S150 参数手册 11/2023`（18 条）
- `SINAMICS S120/S150 参数手册 11/2023（p.71 无效值标记）`（1 条：`drive.motor.temperature`）

| 定义 | scope / semantics | 07/2023 中实际匹配到的串 | PDF | 页 | 章节 | 结论 |
|---|---|---|---|---|---|---|
| `drive.parameter.r0021` | collect / parameter_identity | `r0021 CO: 已滤波的转速实际值 / 已滤波的转速实际值（SERVO_828, SERVO_COM` | S120_para_lists_man_0723 | p39, p40 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0027` | collect / parameter_identity | `r0027 CO: 已滤波的电流实际值 / 滤波 I 实际值` | S120_para_lists_man_0723 | p43, p44 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.motor.temperature` | collect / verified | `r0035 CO: 电机温度 / 电机温度` | S120_para_lists_man_0723 | p50 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0033` | collect / candidate | `r0033 已滤波的转矩利用率 / 已滤波的转矩利用率` | S120_para_lists_man_0723 | p48 | 4 SINAMICS 参数 | `version_difference` |
| `drive.parameter.r0031` | collect / candidate | `r0031 已滤波的转矩实际值 / 已滤波的转矩实际值（p46 同页另有 HLA 的推力条目）` | S120_para_lists_man_0723 | p46, p47 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0062` | collect / parameter_identity | `r0062 CO: 已滤波的转速设定值 / 已滤波的转速设定值` | S120_para_lists_man_0723 | p69 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0063` | collect / parameter_identity | `r0063 CO: 已滤波的转速实际值 / 已滤波的转速实际值` | S120_para_lists_man_0723 | p69, p70 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0064` | collect / parameter_identity | `r0064 CO: 转速控制器调节差 / n 控制器控制差异` | S120_para_lists_man_0723 | p70, p71 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0077` | collect / parameter_identity | `r0077 CO: 转矩电流设定值 / Iq_设定` | S120_para_lists_man_0723 | p79 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0078` | collect / parameter_identity | `r0078[0...1] CO: 转矩电流实际值 / Iq_实际` | S120_para_lists_man_0723 | p80 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0079` | collect / parameter_identity | `r0079[0...1] CO: 总转矩设定值 / 总 M 设定值（p81 同页另有 HLA 推力条目）` | S120_para_lists_man_0723 | p81 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0080` | collect / parameter_identity | `r0080 CO: 转矩实际值 / M 实际` | S120_para_lists_man_0723 | p81, p82 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0081` | collect / parameter_identity | `r0081 CO: 转矩利用率 / M 利用率` | S120_para_lists_man_0723 | p82 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0082` | collect / parameter_identity | `r0082[0...1] CO: 有功功率实际值 / P 实际` | S120_para_lists_man_0723 | p83, p84 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0103` | collect / parameter_identity | `r0103 应用专用视图 / 应用专用视图` | S120_para_lists_man_0723 | p91 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.p0105` | collect / parameter_identity | `p0105 驱动对象激活/禁用 / DO 激活/禁用` | S120_para_lists_man_0723 | p91, p92 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0106` | collect / parameter_identity | `r0106 驱动对象有效/无效 / DO 有效/无效` | S120_para_lists_man_0723 | p93 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0107` | collect / parameter_identity | `r0107 驱动对象类型 / DO 类型` | S120_para_lists_man_0723 | p94, p95, p96, p97 | 4 SINAMICS 参数 | `matching_older_version` |
| `drive.parameter.r0108` | collect / parameter_identity | `r0108 驱动对象功能模块 / 驱动对象功能模块` | S120_para_lists_man_0723 | p98, p99, p100, p101 | 4 SINAMICS 参数 | `matching_older_version` |

逐条说明：

- **`drive.parameter.r0021`**（matching_older_version）：数据类型 FloatingPoint32 与定义 decode=F32_BE 一致。该参数按驱动对象类型分列多条：p39 为 HLA_828（活塞速度）与 SERVO（转速），p40 为 SERVO 线性（速度 m/min）。定义只断言“参数身份”，未断言工程语义，故不构成冲突；工程语义随对象类型不同，采集侧不得假定单位。
- **`drive.parameter.r0027`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致。p43 覆盖 A_INF/S_INF/SERVO（单位 Arms），p44 覆盖 B_INF（单位 A，且手册注明该机型显示值无效）。
- **`drive.motor.temperature`**（matching_older_version）：关键核对：手册 p50 明确写出「当 r0035 等于 -200.0 °C 时」的无效值分支，与定义 invalid_rule{-200.0, ±0.001} 一致；FloatingPoint32 与 F32_BE 一致。原 manual 来源写「11/2023 p.71 无效值标记」——07/2023 版该条目实测在 p50，说明所引 p.71 是 11/2023 版的页码，两版分页不同（20 个月版本差）。
- **`drive.parameter.r0033`**（version_difference）：定义 label 记为「候选语义：轴负载；未确认」，而 07/2023 手册 p48 明确：r0033 为“经过滤波的转矩利用率”，是所需转矩与转矩极限（按 p2196 缩放）之比，单位 %。“轴负载”与“转矩利用率(%)”不是同一表述，候选语义应据此修正或继续标 candidate；其余（FloatingPoint32 / F32_BE）一致。
- **`drive.parameter.r0031`**（matching_older_version）：定义 label 的候选语义“滤波转矩实际值”与手册一致（伺服为转矩 Nm，HLA 为推力 N）；FloatingPoint32 与 F32_BE 一致；p47 另有 SERVO 线性（推力）条目。label 提到的“访问能力 HOLD”属驱动侧访问能力分级，手册未以同名词表述，未作核对。
- **`drive.parameter.r0062`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致；p69 同页另有 HLA 的速度设定值条目（m/min）。
- **`drive.parameter.r0063`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致；p69 为 HLA（速度），p70 为 SERVO（转速 rpm）。
- **`drive.parameter.r0064`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致；p70 为 HLA，p71 为 SERVO 线性。
- **`drive.parameter.r0077`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致。手册注明「在 V/f 控制中，该值没有含义」，属适用范围限制，定义未断言语义故不冲突。
- **`drive.parameter.r0078`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致。手册给出双下标 [0]=未滤波、[1]=经 p0045 滤波；定义 count=1、addressing.line=1，按 r1 中 line=index+1 的用法（见 channel.r_parameter）推断读取的是下标 0（未滤波），该推断未获手册明示，标为待确认。
- **`drive.parameter.r0079`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致；双下标同上，count=1 对应下标 0（推断，待确认）。
- **`drive.parameter.r0080`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致；p81 为 HLA 推力（N）、p82 为 SERVO 转矩（Nm）；双下标同上，count=1 对应下标 0（推断，待确认）。
- **`drive.parameter.r0081`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致；单位为 %（定标 PERCENT），定义未断言语义故不冲突。
- **`drive.parameter.r0082`**（matching_older_version）：FloatingPoint32 与 F32_BE 一致；p83 为 HLA、p84 为 A_INF/S_INF；双下标同上，count=1 对应下标 0（推断，待确认）。
- **`drive.parameter.r0103`**（matching_older_version）：手册数据类型 Unsigned16 与定义 decode=U16_BE 一致；访问等级 2；适用范围列出 A_INF/B_INF/HLA/S_INF/SERVO 全部对象。
- **`drive.parameter.p0105`**（matching_older_version）：数据类型 Integer16 与 decode=I16_BE 一致；p91 为 CU_I/CU_NX（值域 0..1），p92 为 TM54F（值域 0..2，含“2: 驱动对象禁用并且不存在”）。definition label 标注“只读”，而手册该参数写入能力为 T、U（可调试/可上传），二者不矛盾但语义侧重不同，未据此改动定义。
- **`drive.parameter.r0106`**（matching_older_version）：数据类型 Integer16 与 decode=I16_BE 一致。
- **`drive.parameter.r0107`**（matching_older_version）：数据类型 Integer16 与 decode=I16_BE 一致；该参数按驱动对象类型分列多页（94 HLA=70、95 A_INF=10、96 TM120=207、97 CU_LINK=254），定义只断言参数身份，故按对象类型取值不同不构成冲突。
- **`drive.parameter.r0108`**（matching_older_version）：数据类型 Unsigned32 与 decode=U32_BE 一致；位域含义随对象类型不同（98 HLA、99 SERVO、100 A_INF、101 S_INF_COMBI）。

## 4. PDF 页码 vs 印刷页码：偏移结论 = 0

做法：对三份 PDF 的**每一页**取页脚文本，用页脚里的文档号定位印刷页码，再与该页 PDF 页号相减，统计偏移分布。

| 文件 | 总页数 | 成功取到印刷页码 | 偏移 `PDF页 − 印刷页` 分布 | 取不到的页 |
|---|---|---|---|---|
| `828D_sysvar_lists_man_0723_zh-CHS.pdf` | 1236 | 1234 | `{0: 1234}` | p1 封面、p2 法律资讯 |
| `828D_md_lists_man_0723_zh-CHS.pdf` | 690 | 688 | `{0: 688}` | p1 封面、p2 法律资讯 |
| `828D_S120_para_lists_man_0723_zh-CHS.pdf` | 1414 | 1411 | `{0: 1411}` | p1 封面、p2 法律资讯、p3 空白/目录前页 |

**结论：三份 PDF 的 PDF 页号与印刷页码完全一致（偏移 0，共 3333 页验证，0 例外）。**
因此本文件与 JSON 里的页码对两种读法都成立；正文中仍统一标注为“PDF 页码（= 印刷页）”。

页脚两种版式（同一文件奇偶页不同）：偶数页 `46 参数手册, 07/2023, A5E53014702F AA`，奇数页 `参数手册, 07/2023, A5E53014702F AA 47`。

## 5. 抽取质量抽查（中文 CID 未乱码，原文片段逐字如下）

抽取由 pypdf `extract_text()` 完成，中文可读。下列片段为**缓存文本原样**（换行以 ` / ` 表示），写入前由脚本断言其确实存在于对应页的抽取结果中：

| # | 文件 / 页 | 抽取原文片段 |
|---|---|---|
| 1 | `S120_para` p50 | r0035 CO: 电机温度 / 电机温度 / SERVO_828,  / SERVO_COMBI / 可在以下运行状态下更改： - 计算方式： - 访问等级： 2 / 数据类型： FloatingPoint32 动态下标：  - 功能图： 8016, 8017 / 参数组： 显示 |
| 2 | `S120_para` p50 | 1 = 0）。 / 当 r0035 等于 -200.0 °C 时： / － 该温度显示无效（温度传感器故障）。 / － 已连接了一个 PTC 传感器或双金属常闭触点。 / - 同步电机的温度传感器被禁用（p0600 = 0 或 p0601 = 0）。 / r0035 CO: 温度输入 / 温度输入 / A |
| 3 | `S120_para` p91 | 对象视图的编号 / r0103 应用专用视图 / 应用专用视图 / A_INF_828,  / B_INF_828, HLA_828,  / S_INF_828,  / S_INF_COMBI,  / SERVO_828,  / SERVO_COMBI / 可在以下运行状态下更改： - 计算方式： - |
| 4 | `md_lists` p364 | 30110 CTRLOUT_MODULE_NR A01, A11   / - 设定值传送：模块编号 BY T E 上电 / - / me42 1 2, 3, 4, 1, 5 1 31 2/2 M / me62 1 2, 3, 4, 1, 5 1 31 2/2 M / me82 1 2, 3, 4, 1, |
| 5 | `md_lists` p31 |  通用机床数据 / 10000 AXCONF_MACHAX_NAME_TAB N01, N11   / - 机床轴名称 STRING 上电 / - / me42 8 MX1, MY1, MZ1, MSP1,  / MA1 / --2 / 2 M / me62 11 MX1, MY1, MZ1, MSP1,  / MA1, MC1,  |
| 6 | `md_lists` p31 | - 2/2 M / me82 16 MX1, MY1, MZ1, MSP1,  / MA1, MB1, MC1, MQ1 / --2 / 2 M / te42 8 MX1, MZ1, MC1, MSP1,  / MQ1 / --2 / 2 M / te62 11 MX1, MZ1, MC1, MSP1,  / MQ |
| 7 | `sysvar` p15 | 算参数（R）  / R[ <索引 1> ] Real 类型的通道专用计算参数 DOUBLE / 说明： / 系统变量 Rn 或 R[n] 是 Real 类型的通道专用计算参数，并且可供用户自由使用。  / 在零件程序中编程：R<n> 或 R[<n>] / 在同步动作中编程：$Rn 或 $R[<n>] / 这些通道专用计算参数被以 |
| 8 | `sysvar` p15 | E / 说明： / 系统变量 Rn 或 R[n] 是 Real 类型的通道专用计算参数，并且可供用户自由使用。  / 在零件程序中编程：R<n> 或 R[<n>] / 在同步动作中编程：$Rn 或 $R[<n>] / 这些通道专用计算参数被以掉电保持方式存储，并可通过数据备份读入和读出。 / 索引 1： R 参数的最大数量通 |
| 9 | `sysvar` p265 | nk: 无限制 / $P_OPMODE 选择的运行方式 INT / Description: / 变量$P_OPMODE 确定通过 PLC 选择的运行方式。 / 此变量返回以下值： / 0：JOG（手动运行） / 1：MDA（手动数据自动运行） / 2：自动 / Unit Init value Min |
| 10 | `md_lists` p185 | [通道轴编号] / 20070 AXCONF_MACHAX_USED C01, C10   / - 通道轴对应的机床轴 BY T E 上电 / - / me42 5 1, 2, 3, 4, 0 0 31 2/2 M / me62 8 1, 2, 3, 4, 0, 0, 0, 0 0 31 2/2 M / me82 10 1, |

5 条人工可判读记录（抽样判读结论，对应上表 #1、#3、#4/#5、#7）：

1. `r0035` 条目头与 `-200.0 °C` 分支可读 → 与 `invalid_rule{-200.0, ±0.001}` 对得上（p50）。
2. `r0103 应用专用视图`、`数据类型 Unsigned16` 可读 → 与 `decode=U16_BE` 对得上（p91）。
3. `30110 CTRLOUT_MODULE_NR`、`说明： 在该机床数据中输入总线段内的模块编号` 可读 → MD30110 定义页确定（md p364）。
4. `10000 AXCONF_MACHAX_NAME_TAB`、`机床轴名称 STRING`、缺省值 `MX1, MY1, …` 可读 → MD10000 定义页确定（md p31）。
5. `R[ <索引 1> ] Real 类型的通道专用计算参数 DOUBLE` 可读 → `channel.r_parameter` 的语义对应条目确定（sysvar p15）。

未出现乱码/丢字导致的误判：另有 2 处不区分大小写命中（`$P_OPMODE`、`OPI 变量"progStatus"`）也逐一人工看过原文，未被当成匹配。

抽取覆盖率（如实记录）：三份共 3340 页，**3339 页抽取成功**；`828D_S120_para_lists_man_0723_zh-CHS.pdf` 的 **PDF p3** 抽取抛 `KeyError: /DescendantFonts`（该页为封面之后的空白/目录前页，不含参数条目），按纪律**留空**、不做任何猜测。该失败页不在本文件引用的任何页码中。

## 6. not_found / pending_review 逐条说明

本节是校验脚本的锚点：JSON 中每个 `not_found` / `pending_review` 条目都必须在本节出现。

### 6.1 pending_review（2 条）

#### `channel.program.status`

- 标识符：`progStatus`；在 PDF 中出现位置：`828D_md_lists_man_0723_zh-CHS.pdf` p222, p223；章节：5.2 通道专用 NC 机床数据（正文提及）
- 位于 PDF 中的原文串：`progStatus`
- 为何不是匹配：三份 PDF 中唯一出现该标识符的位置是 md p222–223 的正文（异步子程序执行时不通过 OPI 变量“progStatus”和“chanStatus”显示）。该处只以字符串提及 OPI 变量名，未给出该变量的数据类型/单位/下标/读取方式，不构成条目定义，故不能作为 42 条定义的来源页码；需人工核对是否存在独立的 NC 变量（OPI 变量）清单文档。抽取文本可读，无 CID 乱码。

#### `spindle.speed.commanded`

- 标识符：`cmdSpeed`；在 PDF 中出现位置：`828D_md_lists_man_0723_zh-CHS.pdf` p39；章节：4 显示机床数据（正文提及）
- 位于 PDF 中的原文串：`cmdSpeed`
- 为何不是匹配：md p39 出现「该位涉及功能块 SPARPP 中的 OPI 变量 cmdSpeed」，说明该标识符确属本手册体系的 OPI 变量名，但（a）无独立条目给出类型/单位/下标，（b）手册把它归到功能块 SPARPP，而 r1 定义把 cmdSpeed 落在 block=SSP，二者归属不一致，需人工核对后方可采用。

### 6.2 not_found（13 条）

共同事实：以下标识符在三份 07/2023 PDF 的**全文逐页抽取结果**中均 0 命中（含不区分大小写检索）。它们的语义近邻确实存在于手册（`$AC_OVR`、`$AA_SPEED_OVR`、`$P_OPMODE`、`$AA_OVR` 等），但那些是 **NC 编程系统变量**，名称与访问入口都与 r1 定义的 NC 变量（OPI 变量）不同，按纪律不得顶替。

| 定义 | 已检索的三份 PDF | 结论 | 说明 |
|---|---|---|---|
| `mode_group.op_mode` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | 标识符 opMode 在三份 PDF 中均未出现（区分大小写检索为 0 命中）。语义近邻：sysvar p265–266 的 $P_OPMODE「选择的运行方式」（$P_ 前缀编程系统变量，且说明为“通过 PLC 选择的运行方式”），但其名称、访问入口与地址形态（B/S/col 3）均与本定义不同，按纪律不得顶替为匹配。 |
| `channel.program.name.main` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | progName 未出现。同页语境：md p39 出现另一个 OPI 变量名 lastBlockNoStr（功能块 SPARP/SPARPP），与本定义 golden_ref 的 progName 不是同一标识符，不顶替。 |
| `channel.program.name.active` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | progName（block=SPARP）未出现；与上一条同因，md p39 的 lastBlockNoStr 不足以定位“当前活动程序名”。 |
| `channel.program.line_number` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | actLineNumber 未出现。手册内“程序段号”语义由 lastBlockNoStr（md p39）等其它 OPI 名承载，名称不匹配，不顶替。 |
| `channel.program.block` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | actBlock 未出现。md p39 的 lastBlockNoStr 为不同标识符，按纪律不顶替。 |
| `axis.position.commanded_mcs` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | actToolBasePos 未出现；手册的轴位置类条目均为 $AA_/$VA_ 前缀编程系统变量，无 C/SGA 访问入口的 NC 变量名。 |
| `axis.position.actual_bcs` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | actProgPosBKS 未出现（含不区分大小写检索）。不顶替。 |
| `channel.feed.actual` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | actFeedRateIpo 未出现。语义近邻 $AC_FEED 类编程变量未与本定义地址对应，不顶替。 |
| `channel.feed.commanded` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | cmdFeedRateIpo 未出现。手册未提供 NC 变量接口名与 C/S 地址的映射表。 |
| `channel.feed.override` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | feedRateIpoOvr 未出现。语义近邻：sysvar p143 $AC_OVR「轨迹倍率」、p144 $AC_TOTAL_OVR「总轨迹倍率」；二者是该手册记录的编程系统变量，不是本定义所指 C/S/col 3 的 NC 变量接口项，按纪律不顶替。 |
| `spindle.speed.actual` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | actSpeed 未出现。手册的主轴转速条目为 $AA_/  $AC_ 类编程变量，非 SSP 块 NC 变量名。 |
| `spindle.speed.override` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | speedOvr 未出现。语义近邻：sysvar p1143 $AA_SPEED_OVR「主轴倍率」、p989 $AA_OVR「轴的倍率」；不同访问入口，不顶替。 |
| `channel.tool.number_active` | sysvar / md_lists / S120_para 全部 0 命中 | `not_found` | actTNumber 未出现；中文关键词“刀具号”在三份 PDF 的条目头中亦无命中（命中项均为刀具数据表，不含该 NC 变量名）。 |

这 13 条**不是文档写错了**，而是**文档选错了**：它们所引的是“828D NC variables（OPI 变量清单）”类文档，而本轮到手的三份是“系统变量 / 机床数据 / 参数描述”。补齐的正确路径是取得 03/2025 版 NC 变量文档（A5E53370586B AC）或任何 828D OPI 变量清单，而不是继续在三份 07/2023 PDF 里找近似条目。

## 7. 7 条无 manual 来源定义的单独核查（独立交叉引用，不追认为来源）

纪律：**不为凑 42/42 覆盖率强行添加引用**。只有确实找到的定义才新增**独立**交叉引用；`r1.json` 的 `source` 不动，因此这 7 条在 r1.json 中**仍然没有 manual 来源**，缺口仍然存在。

| 定义 | 现有证据种类 | 07/2023 中匹配到的条目 | PDF | 页 | 章节 | 结论 |
|---|---|---|---|---|---|---|
| `drive.object.name` | measurement + frozen_vector | `p0199[0...24] 驱动对象名称 / DO 名称` | S120_para_lists_man_0723 | p136 | 4 SINAMICS 参数 | `matching_older_version` |
| `axis.name` | identity_probe + measurement + frozen_vector | `10000 AXCONF_MACHAX_NAME_TAB N01, N11　机床轴名称 STRING` | md_lists_man_0723 | p31 | 5.1 NC 通用机床数据 | `matching_older_version` |
| `axis.ctrlout.module_number` | identity_probe + measurement + frozen_vector | `30110 CTRLOUT_MODULE_NR A01, A11　设定值传送：模块编号` | md_lists_man_0723 | p364 | 5.3 轴专用 NC 机床数据 | `matching_older_version` |
| `axis.encoder.module_number` | identity_probe + measurement + frozen_vector | `30220 ENC_MODULE_NR A01, A02, A11　实际值传送：驱动器编号/测量回路编号` | md_lists_man_0723 | p365 | 5.3 轴专用 NC 机床数据 | `matching_older_version` |
| `axis.ctrlout.type` | identity_probe + measurement + frozen_vector | `30130 CTRLOUT_TYPE A01, A11　设定值的输出类型` | md_lists_man_0723 | p364 | 5.3 轴专用 NC 机床数据 | `version_difference` |
| `channel.machine_axis_assignment` | identity_probe + measurement + frozen_vector | `20070 AXCONF_MACHAX_USED C01, C10　通道轴对应的机床轴` | md_lists_man_0723 | p185 | 5.2 通道专用 NC 机床数据 | `matching_older_version` |
| `axis.spindle_assignment` | identity_probe + measurement + frozen_vector | `35000 SPIND_ASSIGN_TO_MACHAX A01, A06, A11　定义机床轴为主轴` | md_lists_man_0723 | p451 | 5.3 轴专用 NC 机床数据 | `matching_older_version` |

逐条说明：

- **`drive.object.name`**（`matching_older_version`）：手册 p0199 下标 [0...24] 共 25 个、数据类型 Unsigned16，与定义 count=25 + decode=UTF16_BE（25×U16BE）一致；说明为“驱动对象自由命名”。该条在 r1 中仍无 manual 来源，本次仅新增独立文档依据，不改变其来源登记。
- **`axis.name`**（`matching_older_version`）：手册该条为“机床轴名称一览”，类型 STRING、上电生效，与定义 decode=UTF8_FIXED_16 的量纲不冲突（手册不描述线缆字节布局）。⚠ 需显式记录的观察：p31 各机型缺省值列出的轴名（me82 为 MX1, MY1, MZ1, MSP1, MA1, MB1, MC1, MQ1）与 docs/evidence/nck-live-identity-probe.txt:8-15 的真机读数值逐字相同；这既可能是“设备沿用出厂缺省名”，也可能是抽取自手册缺省值——仅凭该证据无法区分，故该 measurement 仍只作为“当时读数”，不作独立性证明。
- **`axis.ctrlout.module_number`**（`matching_older_version`）：手册说明：输入总线段内的模块编号；PROFIBUS/PROFINET 轴上该下标对应 MD13050 的逻辑 I/O 地址。定义 decode=U8 与之不冲突（手册不描述线缆宽度）。
- **`axis.encoder.module_number`**（`matching_older_version`）：条目标题在 p365，说明延续到 p366（PROFIBUS/PROFINET 轴上该下标对应 MD13050）。⚠ 本结论只补文档依据：不得据此把 axis.encoder.module_number 标记为“真机已验证”，见本文 §9.1 的独立缺口。
- **`axis.ctrlout.type`**（`version_difference`）：手册 p364 说明：0 = 仿真（无硬件要求）、1 = 设定值输出有效（通过硬件配置区分）、2 = 模拟模块上的设定值输出（16 位），并注明“必须使用 MD30132 $MA_IS_VIRTUAL_AX，而非数值 4”。定义 label 写“0 = 无驱动输出”，与手册“0 = 仿真（无硬件要求）”表述不同，需人工确认是否为同一含义（这也是唯一一处取值语义与手册措辞不一致的探针参数）。真机读数（轴 6 = 0、其余 = 1）在两种表述下的解释不同，不能自证。
- **`channel.machine_axis_assignment`**（`matching_older_version`）：手册说明：将机床轴设为通道轴/辅助轴，0 = 未分配给通道（通道轴间隙需 MD11640 $MN_ENABLE_CHAN_AX_GAP=1）。真机读数“位置 6 = 0”与“未分配”一致，可作为该取值的文档解释，但仍不构成真机对协议可访问性的证明。
- **`axis.spindle_assignment`**（`matching_older_version`）：手册说明：在此输入主轴号以定义主轴，非 0 即该轴为第 n 主轴（与定义 label 一致）；最低主轴号 1。

处理结果小结：7 条**全部**在 07/2023 手册中找到了对应条目（6 条机床数据 + 1 条驱动参数），但按纪律只作独立交叉引用：

- 仍标记为“无 manual 来源”的定义数保持 **7**（`drive.object.name` + 6 条 `scope=identity`），未因本轮而减少；
- 其中 `axis.ctrlout.type` 找到的条目与定义表述存在差异（0 的语义），记 `version_difference`；
- `axis.encoder.module_number` 虽然找到了 MD30220 的文档定义，但**这不改变**它的真机证据缺口（见 §9.1）。

## 8. 四类证据各自能证明什么 / 不能证明什么（保持独立）

| 证据类型 | 能证明什么 | **不能**证明什么 |
|---|---|---|
| `manual` 页码 | 手册中如何定义该条目（名称/类型/单位/适用范围） | 协议上该地址可访问、真机可读 |
| `frozen_vector` | 冻结地址/字节行为的**回归一致性** | 真实设备支持 |
| `measurement` | 当时那次测量的结果 | 原始证据是否真实存在、是否覆盖该定义（本轮确有反例，见 §9.1） |
| `identity_probe` | 探针返回过什么 | 实际调用路径与采样范围（本轮确有反例，见 §9.1） |

因此：**本轮新增的 07/2023 文档依据，不等于任何一类运行时证据升级**。尤其 `matching_older_version` 只说明“较早版本手册里有这个条目”，既不能证明协议可访问，也不能证明真机可读。

## 9. 两个必须保留为独立缺口的问题

### 9.1 `axis.encoder.module_number`：证据链断在 measurement 与探针路径，文档补不回来

| 事实 | 位置 |
|---|---|
| 该定义挂着 `measurement` 来源 `docs/evidence/nck-live-identity-probe.txt` | `r1.json` 定义 `axis.encoder.module_number` 的 `source` |
| 该证据文件共 **128 行**，13 个小节依次为：机床轴名(p7)、通道1 位置→机床轴号(p16)、轴驱动号(p25)、轴输出类型(p34)、主轴归属(p43)、驱动对象名 module26(p52)、驱动对象名 module130(p61)、跨路径 r0035(p70)、r0103(p79)、p0105(p88)、r0106(p97)、r0107(p106)、r0108(p115) —— **没有编码器驱动号一节** | `docs/evidence/nck-live-identity-probe.txt:7,16,25,34,43,52,61,70,79,88,97,106,115` |
| `AXIS_ENC_MODULE_NR` **未进入真实探测执行路径**：`live_identity_probe.rs` 的 use 清单不含该常量，测试体内也未调用 | `drivers/sinumerik-nck/tests/live_identity_probe.rs:16-20`（use 清单）、该文件 124-176 行的探针调用序列 |
| `identity.rs` 的导入清单同样不含 `AXIS_ENC_MODULE_NR`，身份浏览执行路径从不读它 | `drivers/sinumerik-nck/src/identity.rs:6-9` |
| 常量本身存在并被 catalog 测试核对，但“存在”≠“被执行” | `drivers/sinumerik-nck/src/identity_spec.rs:135`、`tests/definition_catalog.rs:405` |
| 本轮新增：07/2023 手册确有该机床数据的文档定义（MD30220，md p365，说明延续至 p366） | 本文件 §7 |

**结论写法（必须原样保留）**：即使 07/2023 PDF 找到了对应机床数据定义，**也只能补文档依据，不能据此标记真机已验证**。该条要么**修复探针执行路径**（把 `AXIS_ENC_MODULE_NR` 纳入真实探测、重跑并留存证据），要么**撤销这条不受证据支持的 measurement 声明**。在两件事之一完成前，`axis.encoder.module_number` 的 `semantics_status=verified` 缺少运行时支撑。

### 9.2 module 26 与 module 130 的实例编号不可换算（三条禁令）

同一台 828D 的实测（`docs/evidence/nck-live-identity-probe.txt:53-69`）：

| `V/n` | module 26（`nc_variable` 入口） | module 130（`drive_object` 入口） | 是否同值 |
|---|---|---|---|
| V/1 | `SERVO_3.3:4` | `SERVO_3.3:3` | ❌ 差 1 |
| V/2 | `SERVO_3.3:5` | `SERVO_3.3:4` | ❌ 差 1 |
| V/3 | `SERVO_3.3:6` | `SERVO_3.3:5` | ❌ 差 1 |
| V/4 | `SERVO_3.3:3` | `SERVO_3.3:6` | ❌ 不同（非 ±1） |
| V/5 | `SERVO_3.3:7` | `SERVO_3.3:7` | ✅ 恰好同值 |
| V/6 | **被拒** `S7_JOB_0x8201` | `SERVO_3.3:8` | — |
| V/7 | `SERVO_3.15:3` | `SERVO_3.15:3` | ✅ 恰好同值 |
| V/8 | `SERVO_3.15:4` | `SERVO_3.15:4` | ✅ 恰好同值 |

**三条禁令（同时写进 `828d-definition-source-audit.md` 与 `828d-identity-probe-evidence.md`）**：

1. **不允许**用固定 `±1` 规则把一个入口的实例编号换算成另一个入口的编号（反例：V/4 的 m26=`SERVO_3.3:3` 对 m130=`SERVO_3.3:6`，就不是 ±1）。
2. **不允许**因为某些编号恰好相同（V/5、V/7、V/8）就认定两个入口等价——V/1..V/4 已给出反例。
3. **每个映射必须绑定**：实际入口（module 26 或 module 130）、对象类型（`nc_variable` / `drive_object`）、索引（`V/n`）以及验证证据出处。

写法示例（唯一可接受的表述）：`V/1 @ module26 = SERVO_3.3:4`（证据 `nck-live-identity-probe.txt:53`）与 `V/1 @ module130 = SERVO_3.3:3`（证据 `nck-live-identity-probe.txt:62`）是两条不同事实。

## 10. 复核命令（只读，可在 CI 之外本地一键跑）

```bash
# 0) 机械校验：文件身份 / 清单自相矛盾 / 统计口径 / 覆盖率 / 缺口说明（OK-WARN-FAIL 三档）
python docs/reference/check-manual-metadata.py

# 1) 复算五类分布（与本文 §2.4 对照）
python -c "import json,collections;d=json.load(open('docs/reference/828d-manual-cross-reference.json',encoding='utf-8'));print(collections.Counter(e['match_conclusion'] for e in d['entries']))"

# 2) 复算 35 / 7 的分组（与 r1.json 对照）
python -c "import json;d=json.load(open('drivers/sinumerik-nck/definitions/828d/r1.json',encoding='utf-8'));print(sum(1 for x in d['definitions'] if any(s['kind']=='manual' for s in x['source'])), sum(1 for x in d['definitions'] if not any(s['kind']=='manual' for s in x['source'])))"

# 3) 独立复核某一条页码：逐页抽取并检索（需要本地有 PDF，PDF 不入库）
python - <<'PY'
import json, re
from pypdf import PdfReader
pdf = r'docs/reference/manuals/828D_S120_para_lists_man_0723_zh-CHS.pdf'
r = PdfReader(pdf)
for i, p in enumerate(r.pages, 1):
    try:
        t = p.extract_text() or ''
    except Exception:            # 个别页（如 s120 p3）会抛 KeyError: /DescendantFonts，留空即可
        continue
    if re.search(r'^r0035 \b', t, re.M):
        print('PDF页', i, '| 印刷页尾', t.strip().splitlines()[-1])
PY
```

（以上命令只读本地文件：不连设备、不运行探针、不做任何 git 写操作。）

## 11. 待确认 / 未完成

| # | 事项 | 现状与建议 |
|---|---|---|
| 1 | 13 条 `not_found` | 手册选型问题，非手册错误。需取 03/2025 NC 变量文档（A5E53370586B AC）或 828D OPI 变量清单后再比对 |
| 2 | 2 条 `pending_review` | `channel.program.status`（progStatus 仅正文提及）、`spindle.speed.commanded`（cmdSpeed 仅正文提及且手册归到功能块 SPARPP 而非 r1 的 SSP） |
| 3 | `drive.parameter.r0033` 语义 | 定义写“候选语义：轴负载”，手册 p48 为“转矩利用率（%）”。需评审决定改 label 还是维持 candidate |
| 4 | `axis.ctrlout.type` 的 0 值语义 | 定义写“0 = 无驱动输出”，手册 p364 写“0 = 仿真（无硬件要求）”。需人工确认是否同义 |
| 5 | 双下标参数的线缆取法 | `r0078/r0079/r0080/r0082` 手册为 `[0...1]`，定义 `count=1` + `line=1`；按下标 0（未滤波）解读属**推断**（依据是 r1 中 `channel.r_parameter` 的 `line=index+1` 用法），未获手册明示 |
| 6 | `axis.name` 的实测独立性 | md p31 的 MD10000 缺省轴名与真机读数逐字相同（MX1…MQ1），该 measurement 不能作为独立证据 |
| 7 | 42/42 的结构化 document_id/section/page | 本轮按纪律**未改 r1.json**，该缺口仍在；建议在拿到正确版本文档后单独立项 |
| 8 | 探针执行路径 | `AXIS_ENC_MODULE_NR` 需纳入真实探测或撤销其 measurement 声明（§9.1） |

