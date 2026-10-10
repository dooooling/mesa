# Wire Cutover Matrix（收尾 tracking v1）

> 2026-10-10 校核：下文 v1 是历史基线，FOCAS 0.4.0 已实现 **17/17 类声明读取输出**，负载软件验收见末节；设备验收另行保留。
> 当前声明范围、DLL 直接对照与剩余边界见本文件末尾的审计记录。历史 12/17 和 OPMSG OPEN 不代表当前代码。

基线：`main@6253df9` + parity baseline v1（`wire-parity-sweep.md`）。
日期：2026-09-23
规则：`ready` 才能进 shadow 全等比较；`hold` 只记 documented，不计 mismatch。

| point | Wire | Native parity | status |
|-------|------|---------------|--------|
| machine/status | yes (`0x19`) | pass | ready |
| machine/feed | yes (`0x24`) | pass | ready |
| axis/absolute `1..3` | yes (`0x18+0x26`) | pass (drift=B expected) | ready |
| machine/spindle_speed | yes (`0x25`) | pass | ready |
| spindle/gear | yes (`0xA4+0x40[2]+0xA4`) | pass (672) | ready |
| spindle/maxrpm | yes (`0xA4+0x40[1]+0xA4`) | pass (874) | ready |
| macro/value | yes (`0x15`) | pass (engineering F64) | ready |
| pmc/value | yes (`0x8001`) | pass | ready |
| param/value | yes (`0x8D`) | pass (6711=10030) | ready |
| diagnosis/value REAL | yes (`0x93` type=5) | pass (-0.010, drift=B expected) | ready |
| opmsg/value | yes (`0x34` type=4) | ⚠️ Native debt OPEN（见下） | ready (wire) |
| alarm/value | yes (`0x23`) | pass (empty=[]) | ready |
| servo/load | identity only (`0xA4/0x89/0x56/0xA4` + name) | wait (need >0%) | hold |
| spindle/load | identity only (`0xA4/0x8A/0x40[4,-1]+0xA4`) | wait (need scale) | hold |
| tool/offset | deferred (`cnc_rdtofs` family) | hold (need type map) | hold |
| tool/length | deferred (type=3 single point) | hold | hold |
| tool/zofs | deferred (work zero, semantic corrected) | hold | hold |

`12/17 ready`，`2/17 identity-only hold`，`3/17 deferred hold`。

## Native OPMSG ABI debt（OPEN，Shadow Phase 1 round #3 发现）

- Symptom：Native `cnc_rdopmsg` 返回 `EW_Length(2)`（两轮一致），
  Wire 同窗返回 `String("OP:empty")`（有效结果）。
- Observed cause boundary：当前 Native buffer/wrapper 疑用不足的历史占位，
  而 Wire evidence 显示更大的真实响应布局（`data_len=268` 全量）。
  不断言“64B 一定是根因”（Native wrapper 实际 ABI/长度参数未独立证明）。
- Impact：Native 不能作为 opmsg 的 parity oracle。
- Cutover implication：opmsg parity 必须依赖独立 evidence/fixtures，
  不依赖 `Native == Wire` 全等。
- Shadow 分类：`KNOWN_NATIVE_DEBT`（与 `KNOWN_NATIVE_UNSUPPORTED`
  gear/maxrpm/diagnosis/alarm 分开；后者为 PR52 fail-closed，前者为实现 debt）。
- Status：OPEN（不阻塞 Shadow Phase 1；`mismatch=0` 保持）。

下一步（按顺序）：自然窗口补 `servo/spindle load > 0`（→14/17）；
`driver/focas-wire-shadow` Phase 1 基线 commit（Native primary + compare log，
不改生产值；本文件 debt 项同步进分支）。

## 2026-10-10 当前声明功能与现有 DLL 对照

本节初次复核结果保留作为历史记录；随后固定报文差分发现并修正了报警网络布局、
有符号转速和进给最高位处理。当前结论以文末“软件验收门槛”及其机器证据为准。

范围：仅 FOCAS descriptor 的 17 类读取输出。以 `src/lib.rs::wire_ready_gate`
和实际适配器为支持边界；不把历史地址解析器的 44 项或 DLL 全部导出函数视为已实现。
默认生产使用 Wire。当前 DLL 身份：FWLIB64 7.3.0.1，SHA-256
`7b3837e3925902d1e06c5c7e4f9ce39fe7f39aa9caed029096187f5641ec04ab`；
FWLIBE64 5.3.0.1，SHA-256
`d3341b43bf3945bdb49d03d1d96436a75911bf3c8d1cdf4f30b9167ef935bfd6`。
这是已加载模块身份，不是仅检查仓库 DLL 文件名。

| 当前输出 | DLL 对照入口 | 本次复核 | 已声明范围的边界 |
|---|---|---|---|
| machine/status | cnc_statinfo | aut=1 全等 | 输出运行模式 aut，不是整份 ODBST |
| machine/feed | cnc_rddynamic2 actf | 105 全等 | 当前原始整数口径，不输出整个动态结构 |
| machine/spindle_speed | cnc_acts | 0 全等；历史非零 fixtures | 当前活动主轴，不是任意主轴实例 |
| axis/absolute | cnc_absolute | 轴 2=-5000、轴 3=-1000 全等；轴 1 顺序取样漂移 | 只支持 absolute，返回原始 I32；其他坐标类别未开放 |
| spindle/gear | cnc_rdspgear | 带 guard 原始 DLL=Wire=672 | 单主轴选择；极限与其他固件仍待覆盖 |
| spindle/maxrpm | cnc_rdspmaxrpm | 带 guard 原始 DLL=Wire=0；历史 874 样本 | 非零历史证据保留，本次零值不证明完整量程 |
| macro/value | cnc_rdmacro | #501=25.0 全等；历史多组受控样本 | 标量工程量 F64，按 decimal 换算 |
| pmc/value | pmc_rdpmcrng | R100=0、Y0=1、F0=224、D0=1 全等 | BYTE/WORD/DWORD/bit 单点；范围读取不是声明功能 |
| param/value | cnc_rdparam | #6711=10030 全等；历史受控非零样本 | integer-safe tail；REAL/未知缩放拒绝，不能宣称全参数类型等价 |
| diagnosis/value | cnc_diagnoss（正确五参） | 带 guard #301 axis3：DLL=Wire=-1.0 | 仅 REAL/type5；非 REAL 没有支持承诺 |
| opmsg/value | cnc_rdopmsg3 type4 num=1 | 当前空消息全等；本机 6 场景对照全部通过 | #3006 单条 256B 文本；不声明全消息类型/多条枚举 |
| alarm/value | cnc_rdalmmsg | 带 guard 原始 DLL=Wire=[]；历史 PS0010 fixture | 空/单条；多条仍拒绝，历史非空 fixture 不等于本次非空 DLL 差分 |
| tool/offset | cnc_rdtofs type1 length8 | #16=0.0 全等；历史受控 5/8 非零样本 | 单点 RADIUS GEOM，raw/1000；不支持全刀补布局 |
| tool/length | cnc_rdtofs type3 length8 | #16=0.0 全等；历史受控 10/20 非零样本 | 单点 LENGTH GEOM，raw/1000 |
| tool/zofs | cnc_rdzofs axis1 | #1=0.0 全等；历史 12.345/23.456 非零样本 | 单点工件零偏 X，raw/1000，不是 Z 轴刀补 |
| spindle/load | cnc_rdspmeter | 零值实测与非零派生报文 ABI 对照 | HOLD；未接生产，旧缩放/状态回退/极限整数未闭合 |
| servo/load | cnc_rdsvmeter | 零值实测与非零派生报文 ABI 对照 | HOLD；未接生产，真实负载与数值输出合同未闭合 |

### 本次修正及可复核证据

- OPMSG：旧 Native 调用 `cnc_rdopmsg(type0,length64)`，既不对应 #3006，也没有正确解析 ABI。
  改为 `cnc_rdopmsg3(type4,num=1)`，三个 short 元数据 + 256B 文本，单条 ABI 大小 262B。
  按首 NUL 截断、lossy UTF-8、trim、空值 `OP:empty` 与 Wire 对齐。
  依据实际 FWLIBE64 RVA `0x6964C`，旧入口 RVA `0x76148` 的机型/长度分支不能作为统一接口。
- 返回码：17 现在保留为 `Passwd/EW_PASSWD`，不再落入 System。其他未知返回码仍按历史 System 兜底；
  未证明整个 DLL 返回码全集等价，不能将这一项修正扩大成全错误路径证明。
- 移除报警/诊断/gear/maxrpm 的旧 safe helper；报警单对象 64B、gear/maxrpm 单 short 容量不足，
  诊断旧四参声明缺 length。原始符号改为正确签名，仅由测试入口以带 guard 大缓冲调用，
  Native 生产门禁继续关闭；不把删除危险包装等同于恢复 Native 功能。
- `examples/shadow_probe.rs` 覆盖三类已开放工具输出，移除 OPMSG 的 EW_LENGTH 豁免。
  一次只读窗口为 15 点 equal、1 点动态轴位置 drift、4 点 NativeUnsupported；不能把 4 个豁免当作 DLL 全等。
  随后四类专用 guarded 对照均全等，补齐这四类当前窗口的直接 DLL 样本。

原始实验目录（工作区本地产物，不是发布件）：

- `target/focas-load-research/shadow-audit-20261010.log`：当前 NCGuide 只读对照，未启动 mesad。
- `target/focas-load-research/ready-guarded-20261010-001/`：四类原始 ABI、实际加载模块、比较 JSON、Npcap 抓包；
  `frames/report.json` 逐连接重组，三个流的 OPEN/GENERIC/CLOSE 均可复核。
  同一份数据持久归档于 `tests/fixtures/wire/dll_ready_pair_165_20261010/`，`index.json` 列出逐文件 SHA-256。
- `target/focas-load-research/opmsg-synthetic-20261010-001/002/004/`：实际 DLL 与 Wire 的长文本/截断/空值/lossy/17/2 六场景。
  全部返回码、产品文本一致，262B 外尾部及前后 guard 未改写；报文只在本机派生，不连接设备。
  004 通过后的六组报文、独立预期与加载模块身份归档于 `tests/fixtures/wire/opmsg_dll_synthetic/`。
- `docs/load-research/evidence-index.md`：两类负载的历史原始抓包与非零派生证据，以及尚未闭合的分支。

可重复入口：`tools/ci/focas_opmsg_synthetic.py --test-exe <最新 lib 测试 exe> --out <全新目录>`；
四类只读抓包使用 `tools/ci/focas_load_capture.py --test-name wire::opmsg_parity::guarded_ready_dll_pair_live`
并显式给出 `--host/--test-exe/--out`。先重编测试二进制，不能用旧 exe 验证新代码。

结论：当前 READY 范围具有 DLL 对照样本及 codec 回归，但**不构成 17 类全部功能、任意参数、任意机型的完全等价证明**。
剩余项为两类负载生产合同、未知固件/传统分支、更多范围与极限值、非空报警直接差分、全错误路径，以及真实硬件验收。
NCGuide 只能提供软件模拟层证据；真机门禁和发布门禁不因本次对照自动解除。

本次验证：FOCAS lib 193 passed / 14 ignored（两类手动 DLL 入口默认不进 CI），
shadow_probe 回归 1 passed；实际 DLL 的 OPMSG 六场景与 guarded 四类读取分别通过；
FOCAS all-targets Clippy、格式检查、帧差分工具 15 项通过。
正式合同入口 `python -B scripts/write-contract-evidence.py` 产出
`target/validation/contract.json`：28 suites、186 tests、0 failed、dirty=true，
对应未提交工作区；不是干净提交或真机验收证据。业务服务保持停止。

## 2026-10-10 软件验收门槛

用户暂时没有实际 FANUC 设备，因此软件验收和真机验收分开记录。
当前范围仍为15类 READY读取、两类负载 HOLD；不承诺整个 DLL 的全部导出函数。

### 独立 DLL 差分发现的修正

- `FocasRet` 改为保留完整有符号 short 的类型；未知数值不再归并 `System`。
  单元回归覆盖全部65536种 short 位型，实际 DLL/Wire 消息实验覆盖24组文本及返回码，
  包括1～17和未知正负状态。连接超时、DLL装载失败与所有固件错误语义不由这组报文证明。
- `cnc_rdalmmsg` 的网络条目是四个 BE32 元数据 + 32B文本，48B；
  DLL 写出的 ABI 才是44B（short type/axis/length及dummy）。
  FWLIBE64 RVA `0x68A24` 的 `ntohl` 与 `v13 += 12 / a4 += 44` 已复核，
  本机实际 DLL 的非空报警、32B满文本及嵌入NUL对照通过。
  旧 `alarm_ps0010/alarm_response_frame.bin` 将 ABI 当网络布局，已失去正确帧的证据资格；
  保留原件作为拒绝回归，不修补原件后继续称为原始设备抓包。异常消息长度返回 BAD。
- 转速输出保持 `cnc_acts` 的有符号 I32，包括 INT32_MIN；
  进给保持现有 Native `actf as u32` 的32位位型，包括最高位。
  这两项是原始值合同，不能把无效/极端原始值当实际物理速度。
- 伺服研究路径的32位绝对值在 INT32_MIN 上与实际 DLL 一样保留该位型。
  此修正仅用于协议证据，不把负数当可发布的负载百分比，也不解除负载门禁。

### 软件基线与证据

`tools/ci/focas_ready_synthetic.py` 使用冻结读报文服务，实际 DLL 与生产
`WireFocasApi::read_batch` 的最终 TypedValue 作差分；独立预期不调用 Wire decoder。
baseline/minimum/maximum 三组各18点，覆盖15类 READY及PMC四种输出形态，总计54点。
数值边界包括 I32 最小/最大、I16 最小/最大、BYTE 0/255、bit false/true；
四类已关闭 Native 包装以大缓冲与独立 ABI oracle 取证，不能以 Unsupported 作为通过。
所有54点 DLL / Wire / 独立预期一致。只允许本机临时服务，不透传设备或实现写入。

通过的完整 Wire 会话、对照值、实际加载模块和逐文件 SHA-256 归档于
`tests/fixtures/wire/dll_ready_synthetic/{baseline,minimum,maximum}/`。
默认CI的一个 `ready_dll_evidence_replays_final_values` 回归将这些报文回放到真实生产适配器，
同时检查请求序列和最终类型/值；不为54个点各造一个琐碎单测。
负载整数极限另归档 `tests/fixtures/wire/load_synthetic_edges/`，与普通非零样本共12种操作；
传统读取/状态4回退、小数位外域、其他固件与真实负载仍未闭合。

完整入口（Windows，输出目录必须全新）：

```powershell
python -B tools/ci/focas_equivalence_gate.py --out target/validation/focas-equivalence-new
```

入口依次执行格式、Clippy、最新测试程序构建、单测、Shadow回归、帧工具、
三个实际 DLL 差分窗口、消息/返回码差分、负载研究和唯一正式合同入口。
实际加载 DLL 哈希必须匹配冻结版本；源文件集合（包含未跟踪源码）与内容在执行前后
必须一致。失败写 `software_gate_passed=false`，不接受跳过测试或旧证据参数。
`equivalence.json` 区分软件通过与生产发布，生产/硬件结论始终保持未验收。

当前软件基线不覆盖任意参数类型、多报警、DLL所有版本和固件；这些范围从未被本次门槛开放。
15类当前范围通过上述基线可继续集成验证；17类全部等价与生产交付仍需要两类负载闭环、
实际硬件、平台/固件及SDK发布条件。业务服务继续停止，分支只留本地。


## 2026-10-10 0.4.0：负载正式路径与软件验收

此前15/17、负载HOLD、旧整数缩放和首尾数量按名称缩减均为历史基线。
当前支持边界以正式驱动和 `target/validation/focas-equivalence-20261010-003/equivalence.json`
实际结果为准；合成报文证明解析/适配一致性，不能证明 NCGuide 或真实设备生成非零负载。

| 输出 | 正式合同 | DLL 软件差分覆盖 |
| --- | --- | --- |
| spindle/load | F64 `%`，实例1..4，保留符号 | 新/旧直读、状态4七槽回退、两类比例参数返回值、32位回绕、重复批次状态记忆 |
| servo/load | F64 `%`，轴1..4，DLL32位abs | 新/旧直读、旧family2全局轴数与路径轴数、INT32_MIN保留 |
| 两类公共行为 | 数量不足/远端错误为BAD，不伪造0 | 11组/264行：0、小数、四实例、负小数位、10及极限指数、subnormal、数量缩减、空数组 |

一批负载点各读一次整组；返回后按 DLL 数组槽序号选择，不依靠轴名推断索引。
两次批次重新采样，只有协议能力与旧回退标记在会话内保留，断线后清除。
错误回归覆盖新路径各槽Remote与短数值帧：Remote保留连接，坏帧使连接失效。
数值比较以 DLL ABI data/decimal/unit 与独立预期为基准；工程 F64 允许
相对1e-14加一个最小subnormal的舍入差，原始 ABI 字段仍要求精确全等。

主轴有符号值不做物理有效性猜测；伺服INT32_MIN按DLL32位abs保留负位型。
超过F64或非零下溢输出BAD；0 raw不论小数位均是0%。旧缩放乘数来自
参数数据，4127/4274/4020/4196只是参数号；乘法按DLL32位回绕。

升级0.4.0后通过Core正常重配置刷新F64和单位，同一point_key保持稳定ID。
不得把旧U32 point map直接套到新版负载。当前仍不声明 indexed spindle speed、
tool/number、非absolute轴坐标及DLL全部参数/诊断/刀补布局等价。
