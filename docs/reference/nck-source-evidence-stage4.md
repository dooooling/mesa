# 第 4 步：四项来源证据补齐（记录与裁决）

> 只读/只记录类工作：**不改 `r1.json`（blob 仍为 `3c0abe67cbcb64b525cddec1584e6e1747e0f062`）**、
> 不连设备、不跑真机探针、不做 git 写操作。事实给 `文件:行号` 或页码；查不到的写「未找到」，
> 不确定的标「待核实」。本文件是第 4 步的唯一汇总记录，逐项结论同时回写进
> `828d-definition-source-audit.md` §7、`828d-manual-cross-reference.md` §13、
> `828d-identity-probe-evidence.md` §9 与 `sinumerik-828d-manuals.json` 的 `pending_documents`。

## 0. 基线

| 项 | 值 |
|---|---|
| 分支 / HEAD | `feat/nck-address-proof-ui-contract` / `c22bd52` |
| `r1.json` | `drivers/sinumerik-nck/definitions/828d/r1.json`，blob `3c0abe67cbcb64b525cddec1584e6e1747e0f062`（本轮未改动） |
| 本地手册 | 4 份（`docs/reference/manuals/`，gitignore 精确忽略）：sysvar 07/2023 1236 页、md 07/2023 690 页、S120 参数 07/2023 1414 页、NC variables 03/2025 1262 页 |
| 本地**没有**的手册 | SINAMICS S120/S150 参数手册 **11/2023**（见 §4） |
| 页码口径 | 四份 PDF 的**页脚页码 = PDF 页号，偏移 0**（既有实测，见 `sinumerik-828d-manuals.json` 各条 `page_offset_note`）；本文所有页码均指此口径 |

### 0.1 检索工具（本轮新增，可原样复跑）

| 脚本 | 作用 |
|---|---|
| `docs/reference/extract-manual-text.py` | 用 pypdf 逐页抽文本到 `docs/reference/.cache_manual_<id>.txt`（换页符 `\x0c` 分页；`.gitignore:79-80` 已排除 `.cache_*`，不入库） |
| `docs/reference/grep-manual-terms.py` | 在缓存上按关键词穷尽检索，输出「文档 + 页码 + 上下文」，`--json` 可落结构化结果 |

```bash
python docs/reference/extract-manual-text.py                    # 首次抽取（约 15 分钟）
python docs/reference/grep-manual-terms.py --preset sparp        # progName/SPARPP/SPARPI/SPARPF
python docs/reference/grep-manual-terms.py --preset md30220      # 30220 / ENC_MODULE_NR
```

> 为什么要写成脚本：本轮结论全是「某词在某页出现/不出现」，人工在 4602 页里翻不可复核；
> 把检索固定成脚本后，评审只要跑同一条命令就能推翻或复现本文的每个页码。

## 1. `channel.program.name.main`：穷尽检索结果与结论

### 1.1 `r1.json` 的实际取值（不是转述，是逐字段抄录）

`drivers/sinumerik-nck/definitions/828d/r1.json:65-92`：

| 字段 | 实际值 | 行号 |
|---|---|---|
| `variable_id` | `channel.program.name.main` | 65 |
| `label` | 主程序名 / Main program name | 70 |
| `source[0]` | `manual` → `Siemens 828D NC variables 03/2025 A5E53370586B AC` | 72 |
| `source[1]` | `frozen_vector` → `tests/fixtures/legacy-828d-wire.json`，note `C/SPARPP/progName` | 73 |
| `addressing.area` | `C` | 79 |
| `addressing.area_no` | `{source: target, kind: channel}` | 80 |
| `addressing.module` | `{source: fixed, value: 122}`（= 0x7A） | 81 |
| `addressing.column` | **5** | 82 |
| `addressing.line` | `{source: fixed, value: 1}` | 83 |
| `decode` | `UTF8_NUL_LAST_32` | 85 |
| `transport_size` | `9`（= 0x09） | 86 |
| `count` | `1` | 87 |
| `golden_ref` | `{area: C, block: SPARPP, variable: progName, entry: nc_variable}` | 91 |

浏览层同名候选模板：`drivers/sinumerik-nck/presets/828d.json:45-64`
（`828d.C.SPARPP.progName.nc_variable`，`module 122 / column 5 / count 1 / transport 0x09`，
`validation_status: "candidate"`）。**注意**：该模板已经是 `candidate`，即浏览层从未把它当作已验证。

### 1.2 四份本地手册的穷尽检索（逐处页码 + 上下文）

#### (a) 03/2025 NC variables `A5E53370586B AC`（1262 页）

`progName` 全册共 **4 个条目定义**（手册索引页 p1253 原文 `progName, 515, 522, 526, 531`），逐条：

| 页 | 条目原文标题 | Column index | 数据类型 | 上下文要点 |
|---|---|---|---|---|
| p515 | `progName progName (C, SPARP)` | **3** | `TYPE_STRING` | "Program name / Progam name of the currently active program (or subroutine)"（原文含该拼写错误 progam） |
| p522 | `progName progName (C, SPARPF)` | **7** | `TYPE_STRING` | 同一条目描述，属"程序段搜索指针"块 |
| p526 | `progName progName (C, SPARPI)` | **7** | `TYPE_STRING` | 属"中断程序指针"块（5.3.26 节内） |
| p531 | `progName progName (C, SPARPI)` | **7** | `TYPE_STRING` | 页面正文属 5.3.27 小节，但条目名仍写 `(C, SPARPI)` |

`SPARPP` 的字面出现**只有 3 处**：

| 页 | 上下文 |
|---|---|
| p4 | 目录行 `5.3.27 SPARPP: Program pointers in the automatic mode ......... 530` |
| p18 | 数据块总表行 `SPARPP  Program pointers in the automatic mode   x` |
| p530 | 小节标题 `5.3.27 SPARPP: Program pointers in the automatic mode`，**紧跟其后的正文却是 SPARPI**（原文 `SPARPI: Program pointer at interruption`、`OEM-MMC: LinkItem /Channel/InterruptionSearch/...`），且该页及后续 p531–p534 的全部条目都标注 `(C, SPARPI)` |

计数（对全文缓存精确计数，可复跑）：

| 标记 | 出现次数 |
|---|---|
| `(C, SPARPP)` | **0** |
| `(C, SPARPI)` | 30 |
| `(C, SPARPF)` | 16 |
| `(C, SPARP)` | 51 |

⇒ **03/2025 手册里不存在任何一条 `(C, SPARPP)` 条目，也不存在任何 `progName` 的 column 5。**
手册本身在 5.3.27 处存在**标题与正文错配**（标题写 SPARPP，正文与条目全是 SPARPI）——这是手册缺陷
（先前 `828d-manual-cross-reference.md` §12 已记录同一现象，本轮用计数把它钉死）。

与 `column 5` 有关的两处**同列巧合**（仅记录，**不作为依据**）：

1. p516 `singleBlock singleBlock (C, SPARP)` 的 `Column index: 5`，`TYPE_STRING`，3 个 line index（last/current/next）。
   即：`(C, SPARP)` 的 column 5 是 `singleBlock`，**不是 progName**。
2. p530 的 SPARPI 说明：`1 = main program level`、`2-18 = subroutine levels`。即官方唯一带"main program"
   语义的索引是 **SPARPI 的 line index = 1**，对应条目为 `(C, SPARPI) progName`，**column 7**（p526/p531）。

两处都只是"待核实的候选解释"，**没有任何一处能证明 `column 5 + SPARPP + progName` 成立**。

#### (b) 07/2023 机床数据 `6FC5397-4DP40-6RA6`（690 页）

| 页 | 标记 | 上下文 |
|---|---|---|
| p39 | `SPARPP` ×3、`SPARP` ×2 | MD `10284 DISPLAY_FUNCTION_MASK` 的位说明：位 0 `0x1`「在功能块 SPARP 和 SPARPP 中提供 OPI 变量 lastBlockNoStr」；位 1 `0x2`「涉及功能块 SPARPP 中的 OPI 变量 cmdSpeed」；位 2 `0x4`「涉及功能块 SPARPP 中的 OPI 变量 cmdSpeed（为恒定切削速度预留）」 |
| p290 | `SPARPI` ×1 | MD `TRACE_VARIABLE_NAME`/记录位说明里的「轴和 SPARPI 的不同状态」 |

`progName` **0 处**、`SPARPF` **0 处**。

读法：07/2023 机床数据手册只能证明「SPARPP 确实是一个存在的功能块，且它是若干 OPI 变量（lastBlockNoStr、cmdSpeed）的宿主」；
它**没有**给出 SPARPP 的变量清单、更没有给出 `progName` 的列号 ⇒ 不能补 col 5 的依据。

#### (c) 07/2023 系统变量 `6FC5397-5FP40-0RA6`（1236 页）

`progName` / `SPARPP` / `SPARPI` / `SPARPF` **各 0 处**（该手册条目是 `$AC_/$P_/$MC_` 前缀系统变量，不含 OPI 变量，与既有结论一致）。

#### (d) 07/2023 SINAMICS S120 参数 `A5E53014702F AA`（1414 页）

`progName` / `SPARPP` / `SPARPI` / `SPARPF` **各 0 处**（驱动参数 r*/p* 清单，与 NC 变量无关）。

### 1.3 结论（item 1）

- **`column 5` 的权威依据：未找到。** 四份本地手册里 `(C, SPARPP)` 计数为 0；`progName` 的官方列号只有 3（SPARP）与 7（SPARPF/SPARPI）。
- 因此 `channel.program.name.main` **保持 `pending_review`**（与 `828d-manual-cross-reference.md` §12.3、`docs/evidence/nck-native-address-evidence.json` 的 `manual_mapping.result.pending_review` 一致）。
- **传递链完整 ≠ 来源已确认**：下节把「驱动 → IPC → Core → 前端」的真实代码路径逐跳写明，用于说明
  *这个点位确实能一路跑到前端*，但它跑得通并不证明 `column 5` 正确；两者是不同性质的事实。

### 1.4 传递链：驱动 → IPC → Core → 前端（逐跳 `文件:行号`）

| # | 阶段 | 事实 | 位置 |
|---|---|---|---|
| 1 | 定义装载 | r1.json 编译期内嵌进驱动 | `drivers/sinumerik-nck/src/definition.rs:47`（`include_str!("../definitions/828d/r1.json")`）、装载入口 `definition.rs:395` |
| 2 | 语义编译 | 定义 + target 组装成 12 个原生字段（`module/column/line/decode/transport_size` 原样透传） | `drivers/sinumerik-nck/src/compiler.rs:359`（`compile_point`）、`:417`/`:430`（`compile_semantic*`）、`:707`（`assemble_native_fields`，字段装配在 `:716-737`） |
| 3 | 原生地址解析 | `transport_size` 严格词表解析；`expected_data_len = count × decode.byte_len()` | `drivers/sinumerik-nck/src/native.rs:283`（解析）、`:380-400`（`resolve_native`）；32 字节来自 `drivers/sinumerik-nck/src/value.rs:51` |
| 4 | 线缆编码 | 10 字节 VarSpec：`12 08 <syntax> <area_unit> <col:2> <line:2> <module> <line_count>` | `drivers/sinumerik-nck/src/codec.rs:73-84`；布局常量 `codec.rs:26-36` |
| 5 | 设备读取 + 双闸门 | transport 或长度不符 ⇒ 该项 `data=None`（该点 BAD），不猜不补 | `drivers/sinumerik-nck/src/client.rs:107-127` |
| 6 | 解码 | 文本类整体解码（不按元素切） | `drivers/sinumerik-nck/src/lib.rs:493-505`（`if matches!(kind, Utf8NulLast32 | Utf8NulFirst66 | Utf16Be)`）；`UTF8_NUL_LAST_32` 取最后一个非空段 `value.rs:276-290` |
| 7 | 发布数据面 | 每 tick 逐点 `decode_point` 后一批 `DataBatch` | `drivers/sinumerik-nck/src/lib.rs:927`（`read_vars`）→ `:939`（`decode_point`）→ `:951-958`（`sink.publish(DataBatch{..})`）；`decode_point` 定义在 `lib.rs:482` |
| 8 | IPC 序列化 | `DataBatch` → protobuf；`Value::String` 走 `ValueMsg.string_v` | `crates/driver-sdk/src/lib.rs:535`（`publish`）、`:1078`（`Body::DataBatch(batch_to_pb(&batch))`）；`crates/driver-protocol/src/lib.rs:397`（`batch_to_pb`）、`:365`（`point_value_to_pb`）、`:282-291`（`value_to_pb`，`Value::String(x) => StringV`）；消息定义 `proto/driver.proto:266-272`（`PointValueMsg`）与 `proto/driver.proto:275-297`（`ValueMsg`，`string string_v = 8`） |
| 9 | Core 反序列化 | IPC 批次还原为 `DataBatch` 并入会话事件 | `crates/driver-manager/src/session.rs:1224`（`Body::DataBatch(b) => batch_from_pb(b)`）；`crates/driver-protocol/src/lib.rs:407`（`batch_from_pb`）、`:311`/`:322`（`value_from_pb` 的 `StringV → Value::String`） |
| 10 | Core 最新值快照 | 「Latest-Wins」快照表 | `crates/driver-manager/src/snapshot.rs:98`（`LatestEntry`）、`:557`（`latest_all`） |
| 11 | Core 出口 | REST `GET /api/v1/points/latest` 与 SSE `GET /api/v1/points/live` | `crates/core-api/src/lib.rs:1656`（`latest_points`）、路由 `:3648`；`:3183`（`points_live`，STATE STREAM）、路由 `:3649` |
| 12 | 前端消费 | EventSource 订阅 `/api/v1/points/live`，首帧 snapshot + 后续 delta | `apps/mesa-web/src/workspace/usePointLiveStream.ts:1-9`（模块注释即协议说明）、`:31-32`（事件名）、`:64`（`new EventSource("/api/v1/points/live")`）、`:107-108`（snapshot/delta 监听） |
| 13 | 前端渲染 | 逐行 `<ValueCell value={r.value}/>` → `formatPointValue` | `apps/mesa-web/src/workspace/DeviceLiveData.tsx:125`（「当前值」列）；`apps/mesa-web/src/workspace/LiveCells.tsx:59-69`（`ValueCell` → `formatPointValue`） |
| 14 | 选点入口（前端如何拿到该定义） | NCK 语义选型器 + 定义目录 RPC | `apps/mesa-web/src/components/NckSemanticPicker.tsx:52`；驱动侧 `drivers/sinumerik-nck/src/definition_api.rs:164`（`catalog_json`）/`:565`（`preview_json`）；proto `proto/driver.proto:560-583`（定义目录/preview 的 JSON 信封） |

读法：**这条链上没有任何一站会校验「col 5 是否符合手册」**——列号从 r1.json 原样穿到线缆字节第 4–5 字节。
所以「前端能看到主程序名」只能证明链路通、解码没报错（或诚实报 BAD），**不能**反过来证明列号正确。

## 2. 字符串长度：`UTF8_NUL_FIRST_66` / `UTF8_NUL_LAST_32`

### 2.1 手册里的长度依据（穷尽检索结论）

| 结论 | 证据 |
|---|---|
| **66 有手册表述，但对象不是 actBlock** | 03/2025 手册 p509（`(C, SPARP)` 的 `block` 条目）原文：*"In order to obtain consistent information, all 3 field elements must be requested simultaneously in the variable request. For this reason, the maximum string length of each field is limited to **66 characters**."* |
| 同上，另一处独立表述 | 03/2025 手册 p516（`singleBlock` 条目）原文：*"Because this variable is limited to **66 characters per string**, it might be necessary (for long blocks) to read longer strings. The variable 'singleBlock' can read complete blocks (up to strings with **198 characters**)."* |
| **actBlock 自身没有任何长度规定** | 03/2025 手册 p507 的 `actBlock (C, SPARP)`（`Column index: 31`）只写 `Data type: TYPE_STRING`，无长度、无默认值/最小值/最大值 |
| **32 未找到任何依据** | `progName` 的 4 个条目（p515/p522/p526/p531）全部只写 `Data type: TYPE_STRING`；四份手册中没有任何 "32 字符/32 字节" 与 progName 同现的表述 |
| 三份 07/2023 手册 | `actBlock` 0 处；`TYPE_STRING` 相关长度表述 0 处（sysvar/S120 与 NC 变量无关，md 手册为机床数据） |

**因此：**

- `66` 的实际手册依据是 **`(C, SPARP) block`（column 4）的 66 characters**，而 r1.json 的 `channel.program.block` 是
  **column 31 的 actBlock** ⇒ 属于「同族变量的相邻表述」，**不能**当作 actBlock 的长度确认；
  且单位是 **characters**，Mesa 的 66 是 **bytes**（多字节字符下二者不等）。
- `32` 完全**没有**手册依据，只能追到 `decode` 词表本身（`value.rs:51`）与 `frozen_vector`。
- 结论按纪律如实写：**长度依据未从手册获得**（66 有"相邻但不同变量"的表述，32 无任何表述）。
  `docs/evidence/nck-native-address-evidence.json` 的 `unresolved: string-lengths` **保持 open**（本轮不写 `authority`）。

### 2.2 `r1.json` 的 `transport_size` 实际值与 66/32 的关系

| 定义 | `decode` | `transport_size` | 行号 |
|---|---|---|---|
| `channel.program.block`（actBlock，column 31） | `UTF8_NUL_FIRST_66` | `9`（0x09） | `r1.json:172-173` |
| `channel.program.name.main`（progName，column 5） | `UTF8_NUL_LAST_32` | `9`（0x09） | `r1.json:85-86` |
| `channel.program.name.active`（progName，column 3） | `UTF8_NUL_LAST_32` | `9`（0x09） | `r1.json:114-115` |

三条定义的 `addressing.column` 分别是 31 / 5 / 3（`r1.json:169`、`:82`、`:111`）。

协议侧对 `0x09` 的解释（这是**协议/线缆布局层**的事实，不是手册推断）：

- `crates/s7-transport/src/read_var.rs:9-11` 与 `:41-46`：`length` 字段的单位由 `transport_size` 决定，
  **只有 `0x03/0x04/0x05` 按 bit 计**，`0x06/0x07/0x09` 及未知值一律**按 byte 计**。
- `drivers/sinumerik-nck/src/client.rs:107-127`：响应 `transport_size` 必须等于声明值（`0x09`），否则该项 BAD；
  数据长度再与 `expected_data_len`（= `count × decode.byte_len()`，`native.rs:384-389`）逐字节比对。

**一致性判定：**

- `transport_size = 9` 与 66/32 **不矛盾**：`0x09` 只声明"长度按字节计"，响应里的字节数才会是 66 或 32；
  它**不携带**元素宽度信息，因此**不能**用 `transport_size = 9` 反证长度是 66/32。
- 66/32 的**唯一**来源是 Mesa 自己的 `decode` 词表（`value.rs:42-55` 的 `byte_len()`）。
  这是"我方约定"，不是"手册事实"，也不是"设备实测事实"。

补充：**线缆布局代码本身不含数据长度信息**。`drivers/sinumerik-nck/src/codec.rs:26-36` 的常量只有
`type=0x12`、`len=0x08`、总长 10 字节、area/unit 位宽——那是**请求** VarSpec 的布局；
`codec.rs:73-84` 的 `encode_var_spec` 只拼 10 字节请求。**响应数据长度**由
`native.rs:384-389`（`count × decode.byte_len()`）+ `client.rs:107-127`（exact 比对）决定，
因此"长度依据"只能追到 `decode` 词表，而 `decode` 词表对 66/32 的选择在仓库内**没有任何来源注记**
（`frozen_vector` 只是照抄当时的 catalog，`docs/reference/828d-manual-cross-reference.md` §1 已声明
frozen_vector 只证明回归一致、不证明真机正确）。

### 2.3 `NUL_FIRST` 与 `NUL_LAST` 的语义差异（及其对解码的影响）

代码事实（`drivers/sinumerik-nck/src/value.rs:232-294`）：

| 解码 | 语义 | 对输入的裁剪 | 选择规则 | 代码 |
|---|---|---|---|---|
| `UTF8_NUL_FIRST_66`（actBlock，66 B） | **前置 NUL 填充**：字段尾部用 NUL 补齐，有效文本在前 | 要求 `len >= 66`，**不截断** | 按 `\0` 切分 → 每段 `trim(' ')` → **取第一个非空段** | `value.rs:276-290`（`parts.next()`） |
| `UTF8_NUL_LAST_32`（progName，32 B） | **后置 NUL 填充**：字段前部可能有目录等前置成分，有效名字在后 | 同上（`len >= 32`） | 同上 → **取最后一个非空段** | `value.rs:284-286`（`parts.next_back()`） |
| `UTF8_FIXED_16`（轴名）/ `UTF16_BE`（p0199 对象名） | 定长字段，未做"非空段"筛选 | 取前 16 字节 / 全串 | **字面第一段**（`split('\0').next()`，不 trim 空格） | `value.rs:268`（`Utf16Be`）、`:270-274`（`Utf8Fixed16`） |

对解码结果的实际影响（三点，均被 §2.4 的测试钉住）：

1. **`NUL_FIRST` 不做"遇到第一个 `\0` 就停"**：若报文以 NUL 填充开头，它返回的是**第一个非空段**，
   而不是空串。反过来 `NUL_LAST` 也不做"从尾部找第一个字节"，它返回**最后一个非空段**（跳过尾部 NUL 填充）。
2. **段两侧空格会被 `trim(' ')` 掉**（`value.rs:282`），只去半角空格，不去制表符/换行；
   因此 `"  WORK.MPF  "` → `"WORK.MPF"`。
3. **超长输入在解码层不被截断**：`decode_value` 只拒绝 `len < need`；真正的 exact 闸门在
   `lib.rs:482-505` 的 `decode_point`（`bytes.len() != spec.expected_len` ⇒ 该点 BAD）。
   若绕过 `decode_point` 直接调 `decode_value`，67 字节会被整体参与切分（测试已显式断言这一行为，
   防止有人误以为解码层"自动截断到 66"）。

### 2.4 边界与异常值测试（实际运行结果）

新增两处测试（**只测代码层，无任何设备读数**）：

| 测试 | 位置 | 覆盖 |
|---|---|---|
| `nul_text_projection_boundaries_at_32_and_66` | `drivers/sinumerik-nck/src/value.rs:358` | 正好 32 / 正好 66；31 / 65（短一字节 ⇒ `ShortData`）；33 / 67（超长 ⇒ 解码层不截断）；含 NUL（`NUL_FIRST` 首个非空段、`NUL_LAST` 末个非空段 + trim）；前置 NUL 填充；全 NUL 空串；多字节 UTF-8 恰压边界（63×ASCII + `中` = 66 B）；跨边界被切断（65×ASCII + `0xE4` ⇒ 无效 UTF-8 报错，不替换成 U+FFFD） |
| `text_point_requires_exact_wire_length` | `drivers/sinumerik-nck/src/lib.rs:1223` | 执行面口径：66 B ⇒ GOOD 且取首段；67 B ⇒ BAD + `LastKnown`（不杜撰新值）；65 B ⇒ BAD；32 B 名字 ⇒ GOOD；31 B / 33 B ⇒ BAD |

实测结果（`cargo test -p mesa-driver-sinumerik-nck --lib`）：

```
test value::tests::nul_text_projection_boundaries_at_32_and_66 ... ok
test tests::text_point_requires_exact_wire_length ... ok
test result: ok. 107 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## 3. 《SINAMICS S120/S150 参数手册 11/2023》：登记为来源缺口

### 3.1 登记位置与状态

| 位置 | 内容 |
|---|---|
| `docs/reference/sinumerik-828d-manuals.json` 顶层新增 `pending_documents` | `id = s120-s150-param-manual-112023`，`status = pending_document`，`file_in_local_workspace = false`，无 `sha256`/`pages`/`page` 任何字段（避免编造） |
| `docs/reference/manuals/README.md` | 「待获取清单」表内新增一行，写明版本、被引用条数与缺它挡住的核对类型 |
| `docs/reference/828d-definition-source-audit.md` §7 | 缺口登记 + 受影响定义逐条清单 |
| `docs/evidence/nck-native-address-evidence.json` | 既有 `unresolved: drive-parameter-manual-112023` 保持 `open`，`note` 指向本节 |

> 纪律：本文与上述登记**不写任何该版本的页码范围，也不写「已核对」**——该文件不在本地工作区，
> 任何页码都只可能来自捏造或来自 07/2023 的冒充，两者都禁止。
> 下表中 `drive.motor.temperature` 出现的 `p.71` 是 `r1.json` **既有**的行内注记（**逐字引用**，
> 只为让读者能定位到那条 source 串），**不是**本登记给出的页码，也未核实。

### 3.2 受影响定义（从 `r1.json` 的 `source[].reference` 精确筛出，共 **19** 条）

筛选命令（可复跑）：

```bash
python -c "import json;d=json.load(open('drivers/sinumerik-nck/definitions/828d/r1.json',encoding='utf-8'));[print(x['variable_id'],'|',s['reference']) for x in d['definitions'] for s in x['source'] if s['kind']=='manual' and '11/2023' in s['reference']]"
```

18 条引用串为 `SINAMICS S120/S150 参数手册 11/2023`：

| # | `variable_id` |
|---|---|
| 1 | `drive.parameter.r0021` |
| 2 | `drive.parameter.r0027` |
| 3 | `drive.parameter.r0031` |
| 4 | `drive.parameter.r0033` |
| 5 | `drive.parameter.r0062` |
| 6 | `drive.parameter.r0063` |
| 7 | `drive.parameter.r0064` |
| 8 | `drive.parameter.r0077` |
| 9 | `drive.parameter.r0078` |
| 10 | `drive.parameter.r0079` |
| 11 | `drive.parameter.r0080` |
| 12 | `drive.parameter.r0081` |
| 13 | `drive.parameter.r0082` |
| 14 | `drive.parameter.r0103` |
| 15 | `drive.parameter.p0105` |
| 16 | `drive.parameter.r0106` |
| 17 | `drive.parameter.r0107` |
| 18 | `drive.parameter.r0108` |

另有 1 条引用串带行内页码注记 `SINAMICS S120/S150 参数手册 11/2023（p.71 无效值标记）`：

| # | `variable_id` | 说明 |
|---|---|---|
| 19 | `drive.motor.temperature` | 该 `p.71` 属未取得的 11/2023 版本；07/2023 版《参数描述》实测把同一无效值标记放在 **p50**（见 `828d-manual-cross-reference.md` §3.2），**两版页码不得互换** |

合计 18 + 1 = **19 条**（与 `sinumerik-828d-manuals.json` 的 `entry_count_check`：35 条 manual 来源 = 16 条 03/2025 + 19 条 11/2023 一致）。

### 3.3 缺它导致哪类核对无法完成

| 无法完成的核对 | 为什么 |
|---|---|
| 11/2023 版**页码级**溯源 | r1.json 的 19 条 manual 来源里，只有 `drive.motor.temperature` 带行内页码（`p.71`），其余 18 条只有自由文本文档名；要给出"第几页第几条"，必须有同版本 PDF |
| `exact_match` 级别结论 | 本地 S120 手册是 07/2023（`A5E53014702F AA`），版本不同，只能给 `matching_older_version`；`exact_match` 对 11/2023 组恒为 0 |
| `r0035` 无效值标记（`-200.0 ± 0.001`）的**同版本**背书 | 07/2023 版 p50 有同义表述，但版本不等同；`p.71` 无法验证 |
| 参数**默认值/最小值/最大值**逐条比对（若后续需要冻结） | 同上，需同版本条目页 |
| 19 条定义的 `document_id/section/page` 结构化回填（后续评审项） | 同上 |

### 3.4 取得该版本后可直接执行的核对步骤（可操作）

1. 把文件放入 `docs/reference/manuals/`（命名建议 `828D_S120_S150_para_man_1123_*.pdf`），
   在 `sinumerik-828d-manuals.json` 的 `documents[]` 增一条：`file_present=true`、`sha256`/`size_bytes`/
   `verified.pages` 全部**对实际文件计算**（`Get-FileHash … -Algorithm SHA256` / `pypdf` 页数），
   并从 `pending_documents` 移除该条（同一提交内完成，避免"既 pending 又 document"两份真值）。
2. `python docs/reference/extract-manual-text.py --force`（脚本里已登记四份；新增文档需在 `DOCS`
   追加一行 `(id, 文件名)`）。
3. 对 19 条逐个定位：`python docs/reference/grep-manual-terms.py --terms <参数号> <助记名>`，
   把「文档号 + 版本 + 页码 + 条目原文片段」写进 `828d-manual-cross-reference.json` 的
   新容器 `cross_reference_202311`（**不要**覆盖 `cross_reference_202307` 字段）。
4. 对 `drive.motor.temperature` 单独核对 `p.71` 是否为该版真实页码；若实际页码不同，
   在审计里记为「原文引用页码与实际版次不符」，**不改 r1.json 的自由文本**，留待定义修订评审。
5. 跑 `python docs/reference/check-manual-metadata.py`：新增容器需同步 `match_conclusion_distribution`
   等统计字段，脚本的 C/D/E 组会机械校验一致性（本轮已把该脚本纳入必跑命令）。
6. 只有在 19 条全部得到同版本页码后，才允许把 `docs/evidence/nck-native-address-evidence.json`
   的 `unresolved.drive-parameter-manual-112023` 关闭；关闭动作需评审（该校验器 R6 会拒绝"已解决却仍挂着"）。

## 4. `axis.encoder.module_number`：按证据裁决（选择路线 2）

### 4.1 先查证的四件事（逐项给结论）

#### (1) 四份手册里有无 MD30220 / 等价"编码器驱动号"定义？

**有，但只在 07/2023 机床数据手册**（版本早于 r1.json 所引的 11/2023）：

| 事实 | 位置 |
|---|---|
| `30220 ENC_MODULE_NR A01, A02, A11` / 说明 `实际值传送：驱动器编号/测量回路编号` | `828D_md_lists_man_0723_zh-CHS.pdf` **p365**（条目正文），说明**延续到 p366** |
| p366 原文：`如果轴在 PROFIBUS/PROFINET 总线上，则机床数据 30220 $MA_ENC_MODULE_NR[下标+1] 中输入机床数据 13050 $MN_DRIVE_LOGIC_ADDRESS[下标] 的逻辑输入/输出地址`；`机床数据下标[n] 有如下编码：[编码器编号]：0 或 1`；`关联数据：机床数据 30110 $MA_CTRLOUT_MODULE_NR[n]` | 同上 p366 |
| 索引页 `ENC_MODULE_NR  30220, 365`（页码自证） | 同手册 **p665** |
| 03/2025 NC 变量手册也出现 `30220`/`ENC_MODULE_NR` | p63、p1246（为**索引/交叉引用**性质，非条目定义） |
| 07/2023 系统变量手册、07/2023 S120 参数手册 | **未找到**（各 0 处） |

⇒ 手册能证明的是：**MD30220 是"轴专用机床数据"，语义为"实际值传送的驱动器编号/测量回路编号"，
按编码器编号（0 或 1）索引，且与 MD30110 关联**。
它**不能**证明：Mesa 原生地址里该机床数据的 `module` 字节应取 26（模块字节的权威来源仍是
`unresolved: module-value-authority`），也**不能**证明 `line` 固定为 1 时读到的是哪一个编码器下标。

#### (2) `identity_spec.rs` 该常量的定义注释/参数说明了什么用途？

- 注释原文（本轮补写前）：`/// 轴编码器驱动号（A/<axis>/MDCA_ENC_MODULE_NR）。`
- 参数：`area=A`、`area_no=TargetIndex`（实例号进 axis 槽）、`module=26`、`column=30220`、`line=Fixed(1)`、
  `count=1`、`unit_mode=current`、`decode=U8`、`transport_size=0x09`
  （`drivers/sinumerik-nck/src/identity_spec.rs:134-154` 注释 + `:155-166` 常量体）。
- 用途声明只有"诊断标签"，`IdentityProbeSpec::name = "axis_enc_module_nr"`；结构体注释明确
  「诊断标签（不是用户寻址用的变量名）」（`identity_spec.rs:49-50`）。

#### (3) 它是否在 `identity.rs` 的某个未启用分支里？还是彻底死代码？

**不是"未启用分支"，是"从未被引用"**：

| 事实 | 位置 |
|---|---|
| `identity.rs` 的 `use` 清单只导入 6 个探针常量，**不含** `AXIS_ENC_MODULE_NR` | `drivers/sinumerik-nck/src/identity.rs:6-9` |
| 轴身份读取序列是 `AXIS_NAME → AXIS_CTRLOUT_MODULE_NR → AXIS_CTRLOUT_TYPE → AXIS_SPIND_ASSIGN → (DRIVE_OBJECT_NAME)`，**没有第 6 次读** | `identity.rs:152-193`（`async fn axis`），对象名读在 `:164-183` |
| 真机探针测试的 use 清单同样不含它 | `drivers/sinumerik-nck/tests/live_identity_probe.rs:16-20`；探针体跑的是 `AXIS_NAME`(134-138)、`CHANNEL_MACHAX_USED`(140-144)、`AXIS_CTRLOUT_MODULE_NR`(146-150)、`AXIS_CTRLOUT_TYPE`(152-156)、`AXIS_SPIND_ASSIGN`(158-162)、`DRIVE_OBJECT_NAME`(164-168)、`DRIVE_OBJECT_NAME_M130`(170-174) + 原生地址组（r0035 跨入口 176-204、r0103/p0105/r0106/r0107/r0108 206-227） |
| 它唯二的引用点是常量表与目录契约测试 | `identity_spec.rs:233-244`（`PROBES_828D`）、`tests/definition_catalog.rs:404`（`("axis.encoder.module_number", "axis_enc_module_nr")`） |

⇒ 判定：**定义存在、被目录测试比对、但从不进入任何执行路径**（既非 feature gate，也非条件分支）。
`lib.rs:49-51` 的 `pub use` 只是把它导出给测试/外部引用，不构成执行。

#### (4) `tests/live_identity_probe.rs` 实际跑哪些探针（列出）

见 (3) 的表格：7 个内建探针（不含 `AXIS_ENC_MODULE_NR`）+ `r0035` 的 module26/module130 跨入口同值校验
+ 5 个候选参数（`r0103`/`p0105`/`r0106`/`r0107`/`r0108`，全部走 module 26 原生地址）。

### 4.2 裁决：**选路线 2（撤销无依据的测量声明）**

依据（逐条对应裁决规则）：

| 规则 | 判断 |
|---|---|
| 规则 1「证据充分（含义 + 适用条件 + 实际执行路径）才可改探针接线」 | **不满足**：含义有（MD30220 文档，p365/p366），但 ① **适用条件**未证明——该机床数据按编码器编号 0/1 索引，而探针把 `line` 固定为 1，`line→下标` 的对应关系无任何证据；② **module 字节 26** 对该机床数据的适用性属 `unresolved: module-value-authority`，手册不提供（`docs/evidence/nck-native-address-evidence.json` 的 `manual_mapping.result.native_module_values_provided = 0`）；③ **执行路径**无处可"启用"，只能新写，而新写就等于把一条语义未确认的读塞进用户可见的身份浏览 |
| 规则 2「证据不足 ⇒ 撤销无依据的测量声明，标注尚未验证/暂不可用」 | **照此执行**（见 §4.3） |
| 规则 3「绝对禁止编造测量、把常量伪装成实测、为让测试变绿补期望值」 | 本轮**没有**新增任何 `probe_outputs`、没有新增任何"读数"，也没有为了让测试通过而改期望值 |
| 规则 4「真实 828D 未连接 ⇒ 只可代码/模拟/契约层验证」 | 本轮只做代码层与契约层验证；`docs/evidence/nck-native-address-evidence.json` 的 `real_828d_test_in_this_change` 保持 `false` |

**为什么"接线"这条路本轮不选（而不是"做不到"）**：即使把常量接到 `identity.rs` 的轴路径里，
也没有任何手段在真实 828D 上取得结果来证明"常量参与执行且结果解码正确"——测试只能证明它被调用、
不能证明读数正确；而 `r1.json` 里那条 `measurement` 声明指向的
`docs/evidence/nck-live-identity-probe.txt`（128 行 / 13 个小节，小节头在第 7、16、25、34、43、52、61、70、79、88、97、106、115 行）
**本来就没有编码器驱动号一节**，接线也不会让那条历史声明变得有依据。先撤声明、再谈接线，顺序不可颠倒。

### 4.3 落地清单（本轮实际改动）

| # | 改动 | 位置 |
|---|---|---|
| 1 | 在常量上加 `FIXME`（证据链缺陷：不在执行路径 + measurement 无对应记录）、`TODO`（待硬件证据后才能接线）、`NOTE`（07/2023 只是较早版本的文档依据） | `drivers/sinumerik-nck/src/identity_spec.rs:134-154`（FIXME `:136`、TODO `:147`） |
| 2 | 在 `PROBES_828D` 上加 `NOTE`：**集合成员 ≠ 被执行**，当前实际执行集合是 7 个 | `drivers/sinumerik-nck/src/identity_spec.rs:228-242` |
| 3 | 新增契约层测试：证明该常量**地址自洽**（A/1、col 30220=0x760C、module 26、len 1、transport 0x09）且**明示未被执行** | `drivers/sinumerik-nck/src/identity_spec.rs:344-362`（`enc_module_nr_probe_contract_is_well_formed_but_unexercised`） |
| 4 | 在审计记录里把该 `measurement` 标记为 `retracted_pending_evidence` | `docs/reference/828d-definition-source-audit.md` §7.3；`docs/reference/828d-identity-probe-evidence.md` §9；本文 §4.4 |
| 5 | `r1.json` **未改动**（blob 复核见 §6）；实际移除留给后续经评审的定义修订 | — |

关于"是否要从 `r1.json` 的 `source` 里移除该 measurement"：
**本轮不移除，也不允许移除**（硬约束：`r1.json` 一字节不许动）。移除动作挂在 §4.4 的待办里。

### 4.4 `retracted_pending_evidence` 记录（本轮的正式标记）

```yaml
variable_id: axis.encoder.module_number        # r1.json:1199
source_entry:
  kind: measurement                            # r1.json:1207
  reference: docs/evidence/nck-live-identity-probe.txt
status: retracted_pending_evidence
reason:
  - 所引证据文件（128 行 / 13 小节）没有「编码器驱动号」一节
  - 产生该证据的探针（tests/live_identity_probe.rs）不含 AXIS_ENC_MODULE_NR，故该文件不可能含此读数
  - 常量 AXIS_ENC_MODULE_NR 未被 identity.rs / live_identity_probe.rs 引用，执行路径从不读它
effect_on_r1: 不修改（本轮硬约束）；definition.source 的移除须经评审的定义修订
still_valid_evidence:
  - identity_probe（identity_spec.rs 常量 + definition_catalog 契约测试）
  - frozen_vector（A/M/MDCA_ENC_MODULE_NR）
  - 07/2023 手册 p365/p366 的 MD30220 文档依据（较早版本，仅文档级）
cannot_claim:
  - 真机实测
  - native module 字节 26 对该机床数据的适用性
  - line=1 对应编码器下标 0 或 1
```

### 4.5 结论一句话

`axis.encoder.module_number` 现在处于「**定义存在、文档有（较早版）、执行路径没有、测量声明已撤**」的状态：
`semantics_status=verified` 缺少运行时支撑，这一点**如实保留**，不用任何"看起来绿"的方式抹平。

## 5. 必跑命令的实测结果

驱动代码改动后**先** `cargo build --workspace` 再跑合同测试（旧二进制会让故障注入静默失效）；下表为最终 HEAD 上的实测。

| # | 命令 | 结果 |
|---|---|---|
| 1 | `cargo build --workspace` | **exit 0**（`Finished dev profile [unoptimized + debuginfo] target(s)`） |
| 2 | `cargo test --workspace` | **exit 0**（含 §21 合同测试全集；`cargo test` 只在全部通过时返回 0） |
| 3 | `cargo fmt --all --check` | **exit 0**（无 diff；本轮新增代码已按 rustfmt 排版） |
| 4 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | **exit 0** |
| 5 | `pnpm --dir apps/mesa-web test` | **exit 0**：`Test Files 82 passed (82)` / `Tests 864 passed (864)` |
| 6 | `pnpm --dir apps/mesa-web typecheck && pnpm --dir apps/mesa-web lint` | **exit 0 / exit 0**（`tsc --noEmit` 与 `eslint .` 均无输出即通过） |
| 7 | `pnpm --dir apps/mesa-web build` | **exit 0**（`✓ built in 11.30s`；仅有一条 chunk>500kB 的既有提示，不阻断） |
| 8 | `python tools/ci/test_nck_address_evidence.py` | **OK**：`PASS=62 FAIL=0` / `RESULT: OK` |
| 9 | `python tools/ci/nck_address_evidence.py` | **OK**：`RESULT: OK (fail=0 warn=0 info=6)`（info 为 5 条 SW 版本未记录 + 1 条 r1 blob 未变） |
| 10 | `python docs/reference/check-manual-metadata.py` | **通过**：`OK 28 / WARN 0 / FAIL 0`，exit 0 |

### 5.1 本轮附带的一处 CI 脚本健壮性修复（与四项结论无关，但影响必跑命令）

`tools/ci/test_nck_address_evidence.py` 的断言标签含 `⇒` 与中文，在 **Windows 默认 GBK 控制台**上直接运行会在
`print` 处抛 `UnicodeEncodeError`，导致这条必跑命令以退出码 1 结束（**不是**测试失败，而是编码问题）。
已在脚本开头把 `sys.stdout` 显式重配置为 UTF-8（`tools/ci/test_nck_address_evidence.py:31-37`），
断言逻辑与断言数量未变（仍为 62 条）。

### 5.2 本轮改动文件清单

| 类型 | 文件 | 改动性质 |
|---|---|---|
| 代码（注释/测试，无行为变化） | `drivers/sinumerik-nck/src/identity_spec.rs` | 常量与探针集合的 FIXME/TODO/NOTE；新增契约层测试 |
| 代码（测试） | `drivers/sinumerik-nck/src/value.rs` | 新增 32/66 边界与异常值测试 |
| 代码（测试） | `drivers/sinumerik-nck/src/lib.rs` | 新增执行面 exact 长度测试 |
| CI 脚本 | `tools/ci/test_nck_address_evidence.py` | 仅 stdout 编码修复（见 §5.1） |
| 证据矩阵 | `docs/evidence/nck-native-address-evidence.json` | 新增 `string_lengths` 块（刻意不含 `authority`）；3 条 `unresolved` 的 `note` |
| 审计记录 | `docs/reference/nck-source-evidence-stage4.md`（新） | 本文 |
| 审计记录 | `docs/reference/828d-definition-source-audit.md` §7 | 四项登记（含 `retracted_pending_evidence`、`pending_document`） |
| 审计记录 | `docs/reference/828d-manual-cross-reference.md` §13 | progName/SPARPP/SPARPI/SPARPF 逐页检索结果 |
| 审计记录 | `docs/reference/828d-identity-probe-evidence.md` §9 | encoder 探针裁决与接线前置条件 |
| 清单 | `docs/reference/sinumerik-828d-manuals.json` | 新增 `pending_documents[]`（11/2023） |
| 清单说明 | `docs/reference/manuals/README.md` | 新增「待获取」小节 |
| 检索工具（新） | `docs/reference/extract-manual-text.py`、`docs/reference/grep-manual-terms.py` | 可重复执行的手册检索脚本（不联网） |
| **未改动** | `drivers/sinumerik-nck/definitions/828d/r1.json` | **一字节未动**（blob 见 §6） |

## 6. `r1.json` blob 复核

```
git hash-object drivers/sinumerik-nck/definitions/828d/r1.json
→ 3c0abe67cbcb64b525cddec1584e6e1747e0f062   （与冻结值一致）
git ls-files -s drivers/sinumerik-nck/definitions/828d/r1.json
→ 100644 3c0abe67cbcb64b525cddec1584e6e1747e0f062 0
```

## 7. 未解决项（如实保留）

| # | 未解决项 | 状态 | 谁负责 |
|---|---|---|---|
| 1 | `channel.program.name.main` 的 `column 5` 依据 | `pending_review`（四份手册 0 命中） | 需其他版本/官方证据 |
| 2 | 66 / 32 字节长度的权威依据 | 66 有"相邻不同变量"表述（p509/p516，且单位是 characters）；32 无；`unresolved: string-lengths` 保持 open | 需官方证据或独立设备证据 |
| 3 | 11/2023 SINAMICS S120/S150 参数手册 | `pending_document`（19 条定义受影响） | 需取得同版本文件 |
| 4 | `axis.encoder.module_number` 的 `measurement` | `retracted_pending_evidence`；`r1.json` 待评审后移除 | 定义修订评审 |
| 5 | `line=1` → MD30220 编码器下标（0 或 1）的对应关系 | 待核实 | 需真机/官方说明 |
| 6 | module 字节 26 的权威来源 | `unresolved: module-value-authority`（未变） | 需规格来源 |
| 7 | 真机验证（含本项及其余点位） | 未做（`real_828d_test_in_this_change = false`） | 需显式授权 + 真机 |

## 8. 复核命令（只读，可原样复跑）

```bash
# 1) 手册穷尽检索（§1、§2、§4.1）
python docs/reference/extract-manual-text.py
python docs/reference/grep-manual-terms.py --preset sparp
python docs/reference/grep-manual-terms.py --preset md30220

# 2) 交叉引用/清单机械校验（含 r1.json blob 与统计口径）
python docs/reference/check-manual-metadata.py

# 3) 原生地址证据矩阵与其负向测试
python tools/ci/nck_address_evidence.py
python tools/ci/test_nck_address_evidence.py

# 4) 边界与执行面测试
cargo test -p mesa-driver-sinumerik-nck --lib

# 5) r1.json 未改动
git hash-object drivers/sinumerik-nck/definitions/828d/r1.json
```
