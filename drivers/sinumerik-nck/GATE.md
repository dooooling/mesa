# SINUMERIK NCK Gate（进入 supported 的条件）

## 代码门

- Gate B（空壳）：descriptor 合法、`sinumerik-nck` 可被发现、旧 `sinumerik`
  零残留（discovery/contract/frontend）；运行期方法全部 `NOT_IMPLEMENTED`。
  ✅ 已过（Commit B）。
- Gate C（codec）：`0x82/83/84` exact-bytes golden + 回环 fixture
 （单读/多读/部分错/错基数/畸形/断开/PDU 边界）。
  ✅ 已过（Commit C）。
- Gate D（数据面）：configure/PointMap/Poll/DataBatch/LastKnown/Stop +
  driver contract + session-loss fail attempt + PDU 分片回环。
  ✅ 已过（Commit D；评审 P1-1/P1-2/P1-4/P2-1 修复后 exact gates 重验通过，
  含 JoinSet 双 task 回归 + transport/length 逐项校验 + 单 item 超 PDU 拒绝）。
- Gate E（topology+browse）：probe（会话可达 + 身份诚实待确认）/
  管理面 E2E。✅ 已过（Commit E）。probe anchor 待真机 PR7。
  （注：原“Catalog 虚拟浏览树”已由原生寻址重构移除，见 Gate G。）
- Gate G（原生寻址重构，PR-A/PR-B）：Catalog 退出采集准入，改为唯一
  `NativeNckAddress` 严格解析 + 内建身份探针 + 可选 Preset 预填。
  - G1 原生字段严格解析（未知字段/旧字段/别名/越界/非整数一律拒绝），
    `resolve_native` 不读任何文件即可产出 wire 规格。✅
  - G2 独立 golden：R394 `12 08 82 41 00 01 01 8B 15 01`；原候选目录的
    17 条 wire 向量改经「模板 + 用户输入」复现。✅
  - G3 身份浏览改用内建 `IdentityProbeSpec`；`p0199` 两条入口各有捕获
    字节 golden；分页/拒绝/表尾/过滤语义不减覆盖。✅
  - G4 模板缺失或非法**不影响** Configure/Poll（空/坏模板 + 采集回归）。✅
  - G5 旧 NCK Task JSON 被明确拒绝，无自动迁移。✅
  - G6 transport 语义按实测修正：NCK 响应为 `0x05/0x06/0x07/0x09` +
    **字节**长度；早期脚手架用的 `0x04`+位长度（PLC S7ANY 习惯）已更正。✅
- Gate G-C（独立验收与发布闭环，PR-C）：用**删除前的冻结数据**与**实体机**
  独立验证，而不是用新代码自证。
  - GC1 旧→新寻址等价门：`tests/legacy_equivalence.rs` 逐条断言原生路径复现
    62 条冻结目录地址字节（42 NC 变量 + 20 驱动对象），并断言两条入口不折叠。✅
  - GC2 实体机只读对照：`docs/evidence/nck-live-native-probe.txt` —— 用**新的**
    原生地址 + 内建探针在实体 828D 上取值，与旧目录路径一致
    （轴名/通道映射/驱动号/主轴归属/两条入口 `p0199`/候选参数）。✅
  - GC3 决策与来源留档：`docs/adr/0004-nck-native-addressing.md`（决策、取舍、
    被否决方案）、`presets/PROVENANCE.md`（42 条保留/弃用与来源 + 未纳入项）。✅
  - GC4 升级路径写实：`CONTRACT.md` §升级与迁移 记录备份→停止→重建→核对
    四步与“无自动迁移”的明确承诺。✅
  - GC5 集成回归（REST/Core 全链）：`scripts/nck-native-rest-e2e.py` +
    `docs/evidence/nck-rest-e2e.txt`（12 项断言）—— 模拟器真实 TCP +
    mesad（临时 DB/独立端口），不触碰本机配置。覆盖：
    - 手写原生字段：保存→启动→采到 **GOOD**（`type=f64`、
      `source_label=nck://read/C/1/18/42/1/1?mode=current&decode=F64_BE&transport=0x09`）；
    - **任务往返一致**：读回参数与提交参数逐字节相同，无隐藏回填或静默重解释；
    - **模板路径**：Browse 根 = `nck://presets` + `nck://identity`；模板叶只含固定字段、
      标注 `candidate`/`待确认：area_no/line`；补实例与行后可保存并采到 GOOD；
    - **端点重启 / PointMap**：两条任务同时存在，Stop→Start 后两点都在且 `point_id`
      不变（`e2e.value=1`、`e2e.preset=2`）；
    - 旧目录字段任务在**保存时**即被 Core Schema 以 `REQUIRED module/column/decode`
      拒绝（400），不会静默接受。✅
  - GC6 发布闭环：`docs/releases/nck-native-addressing-2026-10-13.md` 已起草
    （变更摘要、PR 与提交、验证清单、本机迁移、未决的驱动功能版本号），
    并在最终 SHA 上重跑全量回归 + 契约证据 + CI（含 stress/perf/web/三平台）。
    ⬜ 最终 SHA 待 #96/#97/#98/#99 合并后补入。
- Gate F（外部无真机验证，ADR 0002）：真机到手前可证明项的上限。
  - F1 source provenance pinned（`docs/evidence/nck-external-sources.lock.json`，
    三源 commit 锁定）。✅
  - F2 Wireshark assertions pinned（核对矩阵 #1–#6 入库；`wire_data_len`
    与主干解码数学一致）。✅
  - F3 Sharp7 reference vectors / interoperability：pinned vectors 入库 +
    Rust 离线回放（请求 exact differential；S7WLDouble 下双方对同一 wire
    解出相同语义数据——单读 8 字节 exact，多读双 buffer exact）✅；
    live 互操作（setup/单读/多读 rc=0）本地实证 ✅；
    非零 Area 差分与 `<<4` 仲裁仍 open ❌（Area=0 刻意规避）。
  - F4 Softing semantic evidence pinned（AWL 字段顺序 + 例证值入库）。✅
  - F5 standalone emulator process E2E（9 场景独立进程服务 + 握手验证）。✅
  - F6 disagreements explicitly unresolved（`<<4`/`0x06` 保持 open，
    不得静默修掉）。✅（保持 open 即通过项）

## 真机门（840D sl，PR7）

connect / probe / channel+axis topology / 轴实际值与速度 / R 参数 /
程序与通道变量 / multi-read / 断线重连，逐代表变量形成
`官方定义 + Mesa 参数 + wire bytes + 设备返回 + HMI 对照` 证据行，
最小必要包做成回归 fixture。828D 需独立 Gate，不得由 840D 推断。

## 状态

- 当前：experimental（828d 随仓提供公共候选**模板**，840d-sl 暂无模板；
  模板可预填与只读验证，不等于通过硬件 Gate；生产 probe 身份恒待确认）。
- 真机前恒为 ❌（Gate F/G 通过也不翻转）：`NCK_ANCHOR_PENDING` / TSAP confirmed /
  地址语义已确认 / 840D supported。
- `supported` 需全部真机门通过后翻转。
- 原生寻址重构**不提升**支持等级：能读一个结构合法的地址，只证明该地址
  在该目标上可读，不等于该变量的工程语义已确认（见 CONTRACT「三种真实性」）。

## 剩余 20 点对账（2026-10-13 基线，全部 HOLD）

见 `docs/evidence/nck-remaining-20-reconciliation.md`。要点：

- 实体机 8 轴实测名为 `MX1/MY1/MZ1/MSP1/MA1/MB1/MC1/MQ1`；**Y 轴存在**，
  先前依据点位键名推断的“无 Y 轴”不成立（已撤销）。
- 轴 4 为唯一主轴（`MD35000=1`）；轴 6 无驱动输出（`MD30130=0`），
  与 module-26 `V/6` 恰为唯一被拒对象吻合。
- **驱动对象↔机床轴归属仍无法用只读手段建立**：catalog 参数集内无轴归属
  读数，`r0103/p0105/r0106/r0107/r0108` 在各 `V/n` 取值均匀，
  跨路径 `r0035` 全为同一值，`p0199` 名称不编码轴名。
  故 `V/2`、`V/5`、`V/6` 相关点位保持 HOLD。
- 重构后“旧 catalog 白名单”不再存在：这些结论只影响**语义标签可信度**，
  不阻塞原生地址的结构化只读测试。


## 软件控制器的驱动对象证据

模块 130 是显式候选入口，正式入口名及硬件适用性待确认。模板、UTF-16
对象名和整数响应长度修复可由独立软件控制器向量及测试验证；不改变真机门。
对象浏览通过 MD30110/MD30130/MD35000 与 p0199 呈现身份，NC 轴入口及候选
对象入口分开展示，不因相同编号推断关联。r0027/r0031 仍可能被设备拒绝，
不能以 r0078/r0080 成功替代其验收。

实体机只读实测记录见 `docs/evidence/nck-live-identity-probe.txt`
（探针 `tests/live_identity_probe.rs`，`#[ignore]`，显式 `--ignored` 运行；
只读、单会话、不写参数）。该探针**直接使用生产身份探针常量**，
因此同时验证 `src/identity_spec.rs` 的探针数值与实体机一致。


## 实体828D的有限只读验证（2026-10-13）

用户确认的实体828D通过模块26读取r0027/r0031；X/Z/C/A的有效驱动输出
分别由MD30110、MD30130、MD35000及p0199核对。选定240个点通过Mesa
连续3轮完整采集，包含原表170点、18个独立驱动参数及52个身份核对点。
原表20点保留：7项未定义资源、12项未匹配轴、1项无效主轴温度。
r0035接近-200的无效标记按BAD处理，不以ReadVar返回FF判为有效工程值。

这些结果只证明该目标与所选地址的只读行为；HMI逐项语义对照、全部动态
工况、multi-read、断线重连等完整硬件Gate未完成。模块130仍保留候选状态，
不据部分参数或对象名成功将整个目录标成supported。

## 实体828D原生绑定采样复测（2026-10-14，只读）

用**新的原生绑定**（模板 + 使用者确认的 `area_no`/`line`）复测代表性点位，
逐条记录请求字节、设备返回与质量：`docs/evidence/nck-live-samples-probe.txt`
（探针 `tests/live_samples_probe.rs`，`#[ignore]`，只读、单会话）。

**可读（GOOD）**

| 点位 | 结果 |
|---|---|
| `C/S/progStatus` | `U32(3)` |
| `C/S/actFeedRateIpo` | `F64(0.0)` |
| `C/RP/rpa` R0 / R1 / R99（line=R号+1） | `F64(0.0)` |
| `C/SGA/actToolBasePos` 轴1/2/3 | `-1.967` / `-3.515` / `25.551` |
| `C/SGA/actToolBasePos` 轴4..8 | `0.0` |
| `C/SEGA/actProgPosBKS` 轴1..8 | 与同轴设定位置相同（静止、无偏置时可预期） |
| `C/SSP/cmdSpeed` 主轴1 / 主轴2 | `10000.0` / `0.0` |
| `V/M` `r0033`、`r0035`（module 26 与 130） | 见证据文件 |
| `V/M` `r0078`、`r0080`（module 26） | `0.0` / `-3.55e-7` |

**当前被设备拒绝**

- `V/M` `r0027`、`r0031`：在 **module 26 与 module 130 两条入口**、
  `V/1`..`V/4` 上均返回**单项码 `0x85`**（8 次尝试一致，非偶发）。
- `C/RP/rpa` 的 R425：返回单项码 `0x03`（该 R 参数在此机上不存在）。
  注意这不影响 R0/R1/R99 可读。

**与 2026-10-13 记录不一致，以本次为准**

上文「实体828D通过模块26读取r0027/r0031」是 2026-10-13 的记录；本次在同一台
实体机上复测，两条入口均被拒，**该结论不再成立**。可能原因（机床配置/选项/
固件变化，或该参数未经 NC 入口开放）尚未确认，因此：

- **不得**据此断言 `r0027`/`r0031` 在 828D 上普遍可读或普遍不可读；
- `r0078`/`r0080` 可读**不能**替代 `r0027`/`r0031` 的验收（物理量不同）；
- 相关点位维持 HOLD，直到有可复现的设备侧解释。

轴位置在轴1..3 有值、轴4..8 为零，与 `line=机床轴索引` 的说法**一致**，
但单次静止采样不足以据此确认轴身份映射（参见上一节的归属未证明结论）。

