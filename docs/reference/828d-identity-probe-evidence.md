# 828D 身份探针证据表（8 个内建只读探针）

> **Phase 0 子集**只读盘点产物：只读 `drivers/sinumerik-nck/src/identity_spec.rs`、
> `drivers/sinumerik-nck/src/identity.rs` 及既有证据文件；
> **未改动**任何代码/定义/测试/配置，**未连接任何设备**，**未运行任何探针**。
> 本文只整理"仓库里已有什么证据"，不产生新证据；查不到的写「未找到」，不确定的标**待核实**。

## 0. 基线

| 项 | 值 |
|---|---|
| 仓库 HEAD | `047e2064bd233eafc508ca9fe5329e93a96ee6e2` |
| 探针定义 | `drivers/sinumerik-nck/src/identity_spec.rs`（347 行） |
| 探针执行/浏览 | `drivers/sinumerik-nck/src/identity.rs`（492 行） |
| 探针集合常量 | `identity_spec.rs:208-217` —— `PROBES_828D: [IdentityProbeSpec; 8]` |
| 探针结构体 | `identity_spec.rs:47-61` —— 字段 `name/area/area_no/module/column/line/count/unit_mode/decode/transport_size` |
| 地址合成 | `identity_spec.rs:65-85` —— `resolve(target)` 产出 `NckWireAddress` + `NckReadItem` |
| 线缆布局 | `drivers/sinumerik-nck/src/codec.rs:72-84` —— 10 字节：`12 08 <syntax> <area_unit> <column:2> <line:2> <module> <line_count>` |
| 区域索引 | `drivers/sinumerik-nck/src/address.rs:54-63` —— `N=0, B=1, C=2, A=3, T=4, V=5, H=6`；`area_unit = area<<5` 位或 `unit`（`codec.rs:31-36`） |

## 1. 8 个探针总表

`area_no` / `line` 两列用 `Fixed(n)` 或 `TargetIndex` 表示"固定值 / 本次选定的实例号"
（`identity_spec.rs:28-44` 的 `ProbeSlot`）。

| # | 常量 | 诊断名 `name` | 用途 | area | area_no | module | column | line | count | decode | transport | 期望数据长度 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `AXIS_NAME` | `machine_axis_name` | 机床轴名（`N/1/MDSA_AXCONF_MACHAX_NAME_TAB`，line = 机床轴号） | N | Fixed(1) | 26 | 10000 | TargetIndex | 1 | `UTF8_FIXED_16` | `0x09` | 16 B |
| 2 | `AXIS_CTRLOUT_MODULE_NR` | `axis_ctrlout_module_nr` | 轴驱动号（`A/<axis>/MDCA_CTRLOUT_MODULE_NR`） | A | TargetIndex | 26 | 30110 | Fixed(1) | 1 | `U8` | `0x09` | 1 B |
| 3 | `AXIS_CTRLOUT_TYPE` | `axis_ctrlout_type` | 轴输出类型（`MDCA_CTRLOUT_TYPE`；0 = 无驱动输出） | A | TargetIndex | 26 | 30130 | Fixed(1) | 1 | `U8` | `0x09` | 1 B |
| 4 | `AXIS_ENC_MODULE_NR` | `axis_enc_module_nr` | 轴编码器驱动号（`MDCA_ENC_MODULE_NR`） | A | TargetIndex | 26 | 30220 | Fixed(1) | 1 | `U8` | `0x09` | 1 B |
| 5 | `AXIS_SPIND_ASSIGN` | `axis_spind_assign` | 主轴归属（`SPIND_ASSIGN_TO_MACHAX`；非 0 = 该轴为第 n 主轴） | A | TargetIndex | 26 | 35000 | Fixed(1) | 1 | `U8` | `0x09` | 1 B |
| 6 | `CHANNEL_MACHAX_USED` | `channel_machax_used` | 通道位置→机床轴号（`C/1/AXCONF_MACHAX_USED`，line = 通道位置） | C | Fixed(1) | 26 | 20070 | TargetIndex | 1 | `U8` | `0x09` | 1 B |
| 7 | `DRIVE_OBJECT_NAME` | `drive_object_name` | 驱动对象名（NC 变量入口，`V/<object>/p0199`） | V | TargetIndex | **26** | 199 | Fixed(1) | 25 | `UTF16_BE` | `0x06` | 50 B |
| 8 | `DRIVE_OBJECT_NAME_M130` | `drive_object_name_module130` | 驱动对象名（**显式驱动对象入口**，`V/<object>/p0199`） | V | TargetIndex | **130** | 199 | Fixed(1) | 25 | `UTF16_BE` | `0x06` | 50 B |

来源：`identity_spec.rs:92-205`（逐常量），期望长度 = `count × decode.byte_len()`
（`identity_spec.rs:76`），实测断言见 `identity_spec.rs:327-338`
（50 B / 0x06、16 B / 0x09、1 B / 0x09）。

**原生地址的构造方式（逐探针一致）**：
`area` 决定 `area<<5`；`area_no` 槽位填 `TargetIndex` 时用本次选定的实例号，否则 1；
`column` 与 `line` 固定或取实例号；`module` 固定（26 或 130）；`line_count` = `count`。
字节序列即 `codec.rs:72-84` 的 10 字节结构 —— 最后一字节是 `module`，
倒数第二字节是 `line_count`（`p0199` 的 `… 00 01 1a 19` = module `0x1a`=26、count `0x19`=25；
module 130 版本为 `… 00 01 82 19`，只有 module 字节不同）。

## 2. 实例范围与超时

| 项 | 值 | 出处 |
|---|---|---|
| 最大实例号 | **31** | `identity.rs:19` `const MAX_INSTANCE: u16 = 31` |
| 实例游标合法区间 | `1..=31`，越界 ⇒ `IDENTITY_CURSOR` 配置错误 | `identity.rs:295-310`；`identity.rs:301-308` |
| 单页最多核对实例数 | **8**（驱动侧硬上限 `IDENTITY_PAGE_MAX`，不随调用方 `limit` 放大） | `identity.rs:21-28` |
| `area_no` 槽位上限 | 31（5 位，`area_unit` 合成时 `unit>31` fail-closed） | `codec.rs:35-36、61-69`；测试 `codec.rs:116`、`identity_spec.rs:319-324` |
| `line` 槽位上限 | **无 5 位限制**（`line` 是 `u16`，`codec.rs:54`）；轴名/通道探针的实例号进 `line`，其 1..=31 只由 `instance_cursor` 约束 | `identity_spec.rs:99、169`；`identity.rs:295-310` |
| 单次读超时 | `cfg.timeout_ms`：默认 **5000 ms**，最小 500 ms，最大 10000 ms | `config.rs:20、22`；上限 `crates/core-types/src/descriptor.rs:27` |
| 整次身份浏览预算 | **15 s**，超时 ⇒ `IDENTITY_TIMEOUT`（`ErrorKind::Timeout`） | `lib.rs:1009-1020` |

注：身份浏览每次请求都会新建临时连接（`identity.rs:434`），无跨请求缓存；
15 s 是**整页/整个节点**的预算，页内每个读项各自受 `timeout_ms` 约束。

## 3. 结果码分类（任务指定五类的区分，另附相邻错误类）

任务要求的五类对应关系如下（表格后面的行是同一 `read()` 里会遇到的其它错误，供区分时排除）：

| 任务分类 | 本文表格中的行 |
|---|---|
| 正常空名称 | 「正常（有值）」+「正常空名称（表尾 / 未定义对象）」 |
| 设备拒绝 | 「设备拒绝（单项级）」`S7_ITEM_0xNN` + 「设备拒绝（作业级）」`S7_JOB_0xNNNN` |
| 响应校验失败 | 「响应校验失败」①②③ |
| 解码失败 | 「解码失败」`IDENTITY_DECODE`，及「类型不符」`IDENTITY_TYPE` |
| 超时 | 「超时（单次读）」`READ_TIMEOUT` + 「超时（整次浏览）」`IDENTITY_TIMEOUT` |

判定顺序在 `identity.rs:82-117` 的 `read()`：**先看 `return_code`，再看 `data` 是否存在，最后解码**。
这个顺序是分类正确的前提（`client.rs:104-131` 会把"return_code 非 0xFF / transport 不符 /
长度不符"三种情况**都**置 `data=None`，只看 `data` 无法区分）。

| 分类 | 触发条件 | 错误码 | `ErrorKind` | 判定位置 | 浏览层表现 |
|---|---|---|---|---|---|
| **正常（有值）** | `return_code == 0xFF` 且 transport/长度匹配，解码成功且非空 | — | — | `identity.rs:115-116` | 正常节点 |
| **正常空名称**（表尾 / 未定义对象） | 同上，但解码结果是空字符串 | — | — | 轴路径 `identity.rs:141-145`（`Ok(None)`）；对象路径 `identity.rs:463-473`（空页）；`instance_page` 收到 `Ok(None)` 即收敛为表尾 `identity.rs:346` | 提前结束枚举，**不报错** |
| **设备拒绝（单项级）** | `result.return_code != 0xFF` | `S7_ITEM_0xNN`（NN = 原始单项码） | `Address` | `identity.rs:98-107` | `device_rejected()` = true（前缀 `S7_ITEM_`，`identity.rs:74-76`）⇒ 轴/对象页给出 `diagnostic` 节点而非静默表尾（`identity.rs:347-360、454-460、479-489`） |
| **设备拒绝（作业级）** | ReadVar 响应头 ack 错误（class/code 非 0） | `S7_JOB_0xNNNN` | `Address` | `crates/s7-transport/src/pdu.rs:165`，经 `client.rs:136-144` 透传 | 同上（`S7_JOB_` 前缀） |
| **响应校验失败** | ① 结果数组为空 | `IDENTITY_RESPONSE_INVALID` | `Protocol` | `identity.rs:91-97` | 整体报错 |
| | ② `data` 为 `None`（**即使 `return_code==0xFF`**：transport 类型不符或数据长度不符） | `IDENTITY_RESPONSE_INVALID` | `Protocol` | `identity.rs:108-114`；置空逻辑 `client.rs:104-131` | 整体报错，**绝不当作"未定义对象"** |
| | ③ 响应过短 / S7 头缺失 / PDU 引用号不符 | `READ_SHORT` / `S7_SHORT` / `READ_PDU_REF_MISMATCH` | `Protocol` | `crates/s7-transport/src/read_var.rs:163-172`、`session.rs:226-231` | 整体报错 |
| **解码失败** | 字节数与 `decode` 不匹配、UTF-8/UTF-16 非法等 | `IDENTITY_DECODE` | `Protocol` | `identity.rs:115-116` | 整体报错 |
| **类型不符** | 解码结果不是期望的 `String` / `U32` | `IDENTITY_TYPE` | `Protocol` | `identity.rs:119-139` | 整体报错 |
| **超时（单次读）** | 单次 ReadVar 发送/接收超 `timeout_ms` | `READ_TIMEOUT`（另有 `READ_SEND_FAIL`/`READ_RECV_FAIL`） | `Timeout` / `Connection` | `crates/s7-transport/src/session.rs:200-225` → `client.rs:136-144` | 整体报错 |
| **超时（整次浏览）** | 整页 15 s 预算耗尽 | `IDENTITY_TIMEOUT` | `Timeout` | `lib.rs:1009-1020` | 整体报错 |
| 地址/游标非法 | 实例号越界、游标非 1..31 等 | `IDENTITY_ADDRESS` / `IDENTITY_CURSOR` | `Configuration` | `identity.rs:87-89`、`284`、`301-308`、`392-396` | 配置错误 |

**关键区分点（务必保留）**：

1. **"表尾"只由"成功解码出来的空名称"代表**（`identity.rs:78-81` 注释原文：
   「只有成功解码的空名称代表表尾；作业/单项拒绝与本地解码失败均保留为错误」）。
2. 设备拒绝**不等于**表尾：枚举中断时给出 `diagnostic` 节点并明说"后续未枚举，
   目录完整性待确认"（`identity.rs:354-358`），不会把拒绝悄悄隐藏成"没有更多对象"。
3. `0x0A` 之类的单项码属于**设备拒绝**，不是"空"。
   已有证据里可看到该码的真实样本：`docs/evidence/nck-drive-object-readback.json`
   中 10 个 `ITEM_REJECTED / code 0x0A` 样本（含 `machine_axis_name_wire_line0_control`
   即 `line=0` 被拒、`drive_slot_1..9_logical_number` 被拒），这些样本的
   `transport=0 / length=0 / data_hex=''` —— 正说明"`data` 为空"本身不能当表尾用。

## 4. 证据矩阵（每个探针"现在有什么证据"）

证据分四类，**层级不同，不可互相替代**：
**A 字节级捕获**（与真实抓包逐字节相等的测试）＞ **B 布局推导**（按 10 字节布局推导，无抓包）
＞ **C 只读实测值**（设备上实际读数）＞ **D 仅结构断言**（长度/transport，无字节）。

| # | 探针 | A 字节级捕获 | B 布局推导 | C 只读实测（真机） | C' 只读实测（虚拟控制器） | 现有证据等级 |
|---|---|---|---|---|---|---|
| 1 | `AXIS_NAME` | 未找到 | ✅ `identity_spec.rs:294-298` | ✅ `nck-live-identity-probe.txt:7-15`（`MX1..MQ1` 8 轴） | ✅ `readback.json` `machine_axis_name_1..8`（col 10000，8 样本 RAW_ACCEPTED） | **B + C + C'** |
| 2 | `AXIS_CTRLOUT_MODULE_NR` | 未找到 | ✅ `identity_spec.rs:284-288` | ✅ `nck-live-identity-probe.txt:25-33`（轴 1..8 = 1..8） | ✅ `readback.json` `axis_N_setpoint_drive`（col 30110，8 样本） | **B + C + C'** |
| 3 | `AXIS_CTRLOUT_TYPE` | 未找到 | 未找到（仅 D） | ✅ `nck-live-identity-probe.txt:34-42`（轴 6 = 0，其余 1） | ✅ `readback.json` `axis_N_setpoint_type`（col 30130，8 样本） | **D + C + C'** |
| 4 | `AXIS_ENC_MODULE_NR` | 未找到 | 未找到（仅 D） | **未找到**（`tests/live_identity_probe.rs` 未调用该探针） | ✅ `readback.json` `axis_N_encoder_drive`（col 30220，8 样本） | **D + C'**；真机待核实 |
| 5 | `AXIS_SPIND_ASSIGN` | 未找到 | 未找到（仅 D） | ✅ `nck-live-identity-probe.txt:43-51`（轴 4 = 1，其余 0） | 未找到（col 35000 不在 `readback.json`） | **D + C** |
| 6 | `CHANNEL_MACHAX_USED` | 未找到 | ✅ `identity_spec.rs:289-293` | ✅ `nck-live-identity-probe.txt:16-24`（位置 6 = 0） | ✅ `readback.json` `channel1_machine_axis_1..8`（col 20070，8 样本） | **B + C + C'** |
| 7 | `DRIVE_OBJECT_NAME`（m26） | ✅ `identity_spec.rs:241-253`（`12 08 82 a1 00 c7 00 01 1a 19`） | —（有捕获不必推导） | ✅ `nck-live-identity-probe.txt:52-60`（V/1..V/8，V/6 被 `S7_JOB_0x8201` 拒） | ✅ `readback.json` `module26_unitN_object_name`（8 样本） | **A + C + C'** |
| 8 | `DRIVE_OBJECT_NAME_M130` | ✅ `identity_spec.rs:248-252`（`12 08 82 a1 00 c7 00 01 82 19`） | — | ✅ `nck-live-identity-probe.txt:61-69`（V/1..V/8 全部 OK） | ✅ `readback.json` `module130_unitN_object_name`（8 样本） | **A + C + C'** |

补充事实：

- **A 类只有 2 个探针**（#7、#8），且捕获字节就写在测试里（`identity_spec.rs:239-253`）；
  `identity_spec.rs:8-19` 的模块注释明确区分「有捕获证据」与「布局推导」两类，
  并声明"不把后者冒充为抓包证据"。
- **C 类（真机）** 来源 `docs/evidence/nck-live-identity-probe.txt`：连的是
  `192.168.15.60:102`、`pdu=480`（该文件第 6 行），走的是**生产探针常量**
  （`tests/live_identity_probe.rs:1-14` 注释、`GATE.md:121-124`）。
- **C' 类（虚拟控制器）** 来源 `docs/evidence/nck-drive-object-readback.json`，
  其自述 `environment = "Run MyVirtual Machine 1.6 / CNC-SW 6.25 SP2; not real 828D"`、
  `hardware_validated = false`、`module130_status = "candidate; official numeric mapping
  not confirmed; no IP/model special branches"` ——
  **因此 C' 不能被当作真机证据**（本文只在没有真机证据时用它标记"存在过读数"）。
- `AXIS_ENC_MODULE_NR` 虽然被 `identity_spec.rs:208-217` 列入 `PROBES_828D`
  并被 `tests/definition_catalog.rs:397-430` 逐字段核对，
  但 `identity.rs:6-9` 的导入清单里**没有它**，即身份浏览执行路径从不读它。
  它的地址数值只能靠间接证据（虚拟控制器 30220 读数）支撑。

## 5. 与 `r1.json` 的 6 条 `identity_probe` 来源对应

`r1.json` 里 `kind = "identity_probe"` 的条目恰好 **6 条**，reference 均为
`drivers/sinumerik-nck/src/identity_spec.rs`，`note` 即探针常量名：

| `r1.json` 定义 `variable_id` | `source.note`（= 常量名） | 对应探针 # | 该定义是否另有 measurement |
|---|---|---|---|
| `axis.name` | `AXIS_NAME` | 1 | ✅ `docs/evidence/nck-live-identity-probe.txt` |
| `axis.ctrlout.module_number` | `AXIS_CTRLOUT_MODULE_NR` | 2 | ✅ 同上 |
| `axis.encoder.module_number` | `AXIS_ENC_MODULE_NR` | 4 | ✅ 同上（**但该文件实际未读此探针，见 §4**） |
| `axis.ctrlout.type` | `AXIS_CTRLOUT_TYPE` | 3 | ✅ 同上 |
| `channel.machine_axis_assignment` | `CHANNEL_MACHAX_USED` | 6 | ✅ 同上 |
| `axis.spindle_assignment` | `AXIS_SPIND_ASSIGN` | 5 | ✅ 同上 |

**8 个探针 vs 6 条 `identity_probe` 来源的差额解释**：
`DRIVE_OBJECT_NAME`（#7）与 `DRIVE_OBJECT_NAME_M130`（#8）**没有** `identity_probe` 来源条目；
它们的证据挂在定义 `drive.object.name` 上，但用的是 `kind = "measurement"`
（reference `docs/evidence/nck-drive-object-readback.json`，
note「module26/module130 两条入口的 p0199 抓包」）。
即 8 = 6（`identity_probe`）+ 2（以 `measurement` 形式挂在 `drive.object.name`）。

⚠️ **一个必须显式记录的缺口**：`axis.encoder.module_number` 的 `measurement` 来源
reference 是 `docs/evidence/nck-live-identity-probe.txt`，但该文件**没有**编码器驱动号一节
（其小节依次为：机床轴名 / 通道 1 位置→机床轴号 / 轴驱动号 / 轴输出类型 / 主轴归属 /
驱动对象名 m26 / 驱动对象名 m130 / 跨路径 r0035 / r0103 / p0105 / r0106 / r0107 / r0108，
见该文件第 7、16、25、34、43、52、61、70、79、88、97、106、115 行）。
即：**该条 `measurement` 来源在所引文件中未找到对应记录**，属**待核实**
（不排除记录了其他次运行但仍属同一 probe 脚本的可能，但就仓库内该文件而言不具备）。

该对应关系另有可执行判据：`tests/definition_catalog.rs:397-430`
（测试 `scope_identity_definitions_match_identity_spec_constants`）
逐字段断言 6 条 `scope=identity` 定义的 `area/module/column/decode/count/transport_size/unit_mode`
与该 6 个探针常量相等。

## 6. module 26 与 module 130 的实例编号不可互换（必须记录具体入口）

代码侧的硬约束（不是注释建议，是**过滤逻辑**）：

- `identity_spec.rs:190-193` 注释：「两条入口的实例编号**不同**（实体机实测：
  module 26 与 module 130 对同一 `V/n` 返回不同对象名），因此必须作为两个独立探针存在，
  不能互相换算。」
- `identity.rs:224-227`：`variable_nodes()` 按 `entry.module` 做第二重过滤，
  只列 module 与本次核对入口一致的模板；
- `identity.rs:333-335`：驱动对象列表走 `DRIVE_OBJECT_NAME_M130`（module 130）；
- `identity.rs:443-450`：轴路径的驱动编号来自 MD30110，且已用 **module 26** 核对过对象名，
  故只列 module 26 的模板；
- `identity.rs:470-472`：对象路径的编号由 **module 130** 核对，只列 module 130 的模板；
- `identity.rs:153-156`：轴身份里的对象名只走 `DRIVE_OBJECT_NAME`（module 26），
  "不做跨入口换算"。

**证据记录的是"具体入口"，这一点在数据里直接可见**
（`docs/evidence/nck-live-identity-probe.txt:52-69`，同一台 828D）：

| `V/n` | module 26（`nc_variable` 入口） | module 130（`drive_object` 入口） | 是否同值 |
|---|---|---|---|
| V/1 | `SERVO_3.3:4` | `SERVO_3.3:3` | ❌ 不同 |
| V/2 | `SERVO_3.3:5` | `SERVO_3.3:4` | ❌ 不同 |
| V/3 | `SERVO_3.3:6` | `SERVO_3.3:5` | ❌ 不同 |
| V/4 | `SERVO_3.3:3` | `SERVO_3.3:6` | ❌ 不同 |
| V/5 | `SERVO_3.3:7` | `SERVO_3.3:7` | ✅ 偶然同值 |
| V/6 | **被拒** `S7_JOB_0x8201` | `SERVO_3.3:8` | — |
| V/7 | `SERVO_3.15:3` | `SERVO_3.15:3` | ✅ 偶然同值 |
| V/8 | `SERVO_3.15:4` | `SERVO_3.15:4` | ✅ 偶然同值 |

**读法**：两条入口编号偏移并不恒定（V/1..V/4 有偏移、V/5/V/7/V/8 恰好相同），
所以"偏移 +1"或"两条入口等价"的任何换算都会产生**未经证明的地址归属**。
记录证据时必须写清"哪个 module 入口 + 哪个编号"（例如
`V/1 @ module26 = SERVO_3.3:4` 与 `V/1 @ module130 = SERVO_3.3:3` 是两条不同事实）。

同理，`docs/evidence/nck-drive-object-readback.json` 的样本标签本身就带入口
（`module26_unit1_object_name` / `module130_unit1_object_name`），且自述
`module130_status = "candidate; official numeric mapping not confirmed"`。

### 6.1 三条禁令（硬性，禁止在后续任何证据/映射里违反）

1. **不允许**用固定 `±1` 规则把一个入口的实例编号换算成另一个入口的编号
   （反例：`V/4` 的 m26=`SERVO_3.3:3`、m130=`SERVO_3.3:6`，不是 ±1）。
2. **不允许**因为某些编号恰好相同（上表 `V/5`、`V/7`、`V/8`）就认定两个入口等价。
3. **每个映射必须绑定**：实际入口（module 26 或 module 130）、对象类型（`nc_variable` / `drive_object`）、
   索引（`V/n`）以及验证证据出处（文件:行号）。

**唯一可接受的写法**：`V/1 @ module26 = SERVO_3.3:4`（证据 `nck-live-identity-probe.txt:53`）与
`V/1 @ module130 = SERVO_3.3:3`（证据 `nck-live-identity-probe.txt:62`）是两条不同事实，不得合并成一条“驱动对象 4”。

## 7. 待核实（阶段 A 后仍缺运行时证据的项）

| # | 事项 | 现状 |
|---|---|---|
| 1 | 8 个探针的**手册章节/页码** | **已补 6 条（阶段 A）**：`AXIS_NAME`=md p31（MD10000）、`AXIS_CTRLOUT_MODULE_NR`=md p364（MD30110）、`AXIS_CTRLOUT_TYPE`=md p364（MD30130）、`AXIS_ENC_MODULE_NR`=md p365（MD30220）、`CHANNEL_MACHAX_USED`=md p185（MD20070）、`AXIS_SPIND_ASSIGN`=md p451（MD35000）；`DRIVE_OBJECT_NAME`/`_M130`=s120 p136（p0199）。均为 07/2023 较早版本文档依据，**不构成真机验证**（见 `828d-manual-cross-reference.md` §7）。证据文件内部仍无文档节号 |
| 2 | `AXIS_ENC_MODULE_NR`（col 30220）的真机读数 | **未找到**真机证据（`tests/live_identity_probe.rs` 未调用该常量，`identity.rs:6-9` 也未导入）；仅有虚拟控制器读数。**另**：该定义 `axis.encoder.module_number` 在 `r1.json` 中确实挂着 `measurement` 来源 `docs/evidence/nck-live-identity-probe.txt`，但该文件内**没有**编码器驱动号一节 ⇒ 该 `measurement` 来源**在所引文件中未找到对应记录**（见 §5） |
| 3 | `AXIS_SPIND_ASSIGN`（col 35000）的字节级/布局测试 | **未找到**（既无 exact-bytes 断言，也无长度断言）；仅有真机读数 |
| 4 | `AXIS_CTRLOUT_TYPE`（col 30130）的 exact-bytes 断言 | **未找到**；仅有长度/transport 断言（`identity_spec.rs:335-337`）与实测 |
| 5 | module 26 / 130 的**官方入口名与硬件适用性** | **待核实**。`GATE.md:115-116` 原文：「模块 130 是显式候选入口，正式入口名及硬件适用性待确认」；证据文件自述 `module130_status = candidate` |
| 6 | 模块 26 在 `V/6` 上被拒（`S7_JOB_0x8201`）的机理解释 | **待核实**。该现象记录在 `nck-live-identity-probe.txt:58、76、85` 等行，仓库内**未找到**成因说明；`GATE.md:118-119` 只记录"r0027/r0031 仍可能被设备拒绝" |
| 7 | 单项码 `0x0A` / `0x85` 的官方语义 | **未找到**官方定义。仓库内只作为"设备拒绝"处理（`identity.rs:98-107`），不解释码义 |
| 8 | 15 s 整页预算是否覆盖 31 实例全量枚举 | **待核实**。`IDENTITY_PAGE_MAX=8` 下每页最多 8 实例 × 最多 5 次读；31 实例需多页，跨页总耗时未在任何证据文件中实测 |
| 9 | `AXIS_NAME` 的 `line` 槽位上界 | 代码层只受 `instance_cursor` 的 1..=31 限制（`identity.rs:295-310`），但 `line` 字段本身是 `u16`（`codec.rs:54`）；真实设备的 `line` 上界**未找到**证据（证据只覆盖 1..8） |
| 10 | `axis.encoder.module_number` 的**文档依据** | **已补（阶段 A）**：07/2023 机床数据手册 md p365 有 30220 ENC_MODULE_NR 的条目定义（说明延续至 p366）。但这**只补文档依据**，该条 `measurement` 在所引证据文件中仍不存在、`AXIS_ENC_MODULE_NR` 仍未进入真实探测执行路径 ⇒ **仍不得标记真机已验证**，须修探针或撤声明（见 `828d-manual-cross-reference.md` §9.1） |

## 8. 复核命令（只读，不触发任何设备访问）

```bash
# 1) 探针常量与字段（8 个）
python - <<'PY'
import json
src = open('drivers/sinumerik-nck/src/identity_spec.rs', encoding='utf-8').read()
import re
for m in re.finditer(r'pub const (\w+): IdentityProbeSpec = IdentityProbeSpec \{(.*?)\n\};', src, re.S):
    print(m.group(1), re.sub(r'\s+', ' ', m.group(2)).strip())
PY

# 2) r1.json 中 6 条 identity_probe 来源
python -c "import json;d=json.load(open('drivers/sinumerik-nck/definitions/828d/r1.json',encoding='utf-8'));print([(x['variable_id'],s['note']) for x in d['definitions'] for s in x['source'] if s['kind']=='identity_probe'])"

# 3) 已有证据文件的入口标签（module26 / module130 分开）
python -c "import json;d=json.load(open('docs/evidence/nck-drive-object-readback.json',encoding='utf-8'));print(d['environment'],d['hardware_validated'],d['module130_status']);print([s['label'] for s in d['object_identity_samples'] if s['column']==199])"
```

（上述命令只读本地文件，不建立网络连接、不运行探针测试。
`tests/live_identity_probe.rs` 是 `#[ignore]` 的真机探针，本次**未运行**。）
