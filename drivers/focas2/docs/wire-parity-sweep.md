# FOCAS Wire 12/17 生产输出清单 + Native/Wire 地址映射（收尾基线 v1）

基线：`main@6253df9`（PR #60/#61/#62 已合入）
日期：2026-09-23
用途：Wire parity sweep → dual-run shadow → backend cutover 的唯一对照表。
HOLD 项（servo/spindle load value、tool/zofs）不在本表 production 范围内；
Native 侧 PR52 fail-closed 变体在对账时记为 `ERR:` 占位，不视为 parity 失败。

## Native 后端（`NativeFocasApi`，`focas_api.rs`）

- 入口：`Arc<dyn FocasApiTrait>`（`lib.rs open_connection use_native=true` 默认）。
- Native worker：固定 OS 线程（PR52 线程亲和）+ 有界队列 `WORKER_QUEUE_MAX=16`。
- PR52 门：`Alarm / Diagnosis / Spindle{Speed,Load,Gear,MaxRpm} / ServoLoad /
  非 Absolute Axis` 在任何 FFI 前即 `Noopt`（单点 BAD，不进 FFI）。
- Tool 可信路径：`Tool{Number→U32(1), Offset/Length→cnc_rdtofs→F64/1000,
  Zofs→cnc_rdzofs→F64/1000}`（B6 语义债：offset/length 同源待拆）。

## Wire 后端（`WireFocasApi`，`wire/wire.rs read_batch`）

- 会话：`FocasClient` + `SessionSlot`（generation/poison/endpoint 绑定）。
- 错误二分：`Unsupported/Remote → ERR:` 单点；其余致命即整批 `Err`（重连）。
- 去重：Status/Feed/ActiveSpindle/OpMsg/Alarm 各一次；Axis/Absolute 按轴；
  Gear/MaxRpm 按 `(kind,spindle)`；Macro/Param 按号；Diagnosis 按 `(number,axis)`；
  PMC 按 `(kind,addr,dt)`（bit 走 BYTE+本地 mask）。

## 12/17 对账表（`FocasAddress → Native 值 → Wire 值`）

| # | 地址 | Native（`focas_api.rs`） | Wire（`wire.rs`） | 备注 |
|---|------|--------------------------|-------------------|------|
| 1 | `Status` | `cnc_statinfo.aut → U32` | `0x19 → U32(aut)` | B1/B2 双闭合 |
| 2 | `Feed` | `rddynamic2.actf → U32` | `0x24 → U32(mantissa)` | B1 |
| 3 | `Axis{1..8, Absolute}` | `cnc_absolute → I32` | `0x18+0x26 → I32(mantissa)` | 轴号各一次 |
| 4 | `ActiveSpindleSpeed` | `cnc_acts → I32` | `0x25 → I32(mantissa)` | 无 spindle 实例语义 |
| 5 | `Spindle{1..4, Gear}` | PR52 Noopt | `0xA4+0x40[2]+0xA4 → I32(BE16)` | Batch 1（672） |
| 6 | `Spindle{1..4, MaxRpm}` | PR52 Noopt | `0xA4+0x40[1]+0xA4 → I32(BE16)` | Batch 1（874） |
| 7 | `MacroVar{number}` | `cnc_rdmacro → F64` | `0x15 → F64(scaled)` | engineering 口径 |
| 8 | `Pmc{kind,addr,bit?}` | `pmc_rdpmcrng → I32/Bool` | `0x8001 → I32/Bool` | BYTE→I32（PR56） |
| 9 | `Param{number}` | `cnc_rdparam → I32` | `0x8D → I32` | tail gate |
| 10 | `Diagnosis{number,axis}` | PR52 Noopt | `0x93 type=5 → F64(-0.010)` | Batch 2（REAL only） |
| 11 | `OpMsg` | `cnc_rdopmsg → String` | `0x34 type=4 → String` | 空=`OP:empty` |
| 12 | `Alarm` | PR52 Noopt | `0x23 → StringArray` | Batch 3（空=[]） |

HOLD（不在 sweep production 范围；sweep 时 Native 侧记 `ERR:`）：
`Spindle{Load,Speed}`、`ServoLoad{1..4}`（PR52 Noopt；S4/S5 等非零）、
`Tool{Number/Offset/Zofs/Length}`（Native 可信但 Wire 未实现；B6）、
`Axis{非 Absolute}`、`Pmc{非法 kind/bit}`（双边 fail-closed 对齐项）。

## Parity sweep 方法（只读，不改生产 backend）

1. 同机同窗：`MESA_FOCAS_GATE0_HOST` 与 `MESA_WIRE_HOST` 同一 target
  （165：`192.168.15.165:8193`），同任务单先后跑 Native dumper 与 `wire_probe`。
2. 同地址：上表 12 项逐项构造 `FocasAddress`（Diagnosis 固定 `(301,3)`；
   Gear/MaxRpm 固定 `spindle=1`；Macro=`501`；PMC=`R100/Y0/F0/D0`；
   Param=`6711`；Axis=`1..3 Absolute`）。
3. 输出比较：`Value` 全等（含变体；`F64(-0.01)` 按位比较，不做 epsilon）。
   Native PR52 项（Gear/MaxRpm/Diagnosis/Alarm）期望 `ERR:`，Wire 侧期望
   真实值——此差异为已知生产边界，不记 mismatch（见 dual-run 计划）。
4. 记录：mismatch / latency / unsupported 三列；只记录，不改 backend。

## Mismatch 分类（冻结：drift 不记 mismatch）

- A `codec mismatch` ← blocker（同窗同地址 `Value` 不等且非动态值）。
- B `dynamic value drift` ← expected（轴位置 `axis.abs.*`、diagnosis REAL 等
  随现场变化的值；如 `axis3 -10 → -1000`、`diag301a3 -0.01 → -1.0`）。
- C `unsupported/deferred` ← documented（PR52 vs Wire Batch 豁免项 +
  servo/spindle load value、tool/zofs HOLD 项）。

## Dual-run shadow 计划（下一步，不在本文件实现）

- 采集请求同时走 Native + Wire，生产值仍取 Native，Wire 只做 compare。
- 上表“备注”列的已知边界（PR52 vs Wire Batch 1/2/3）在 shadow 比较器中
  显式豁免，不计 mismatch；其余 8 项（Status/Feed/Axis/ActiveSpindle/
  Macro/Pmc/Param/OpMsg）必须全等。
- Cutover Gate（冻结）：Gate1 production codec parity；Gate2 CNC 长时间运行；
  Gate3 mismatch=0；Gate4 HOLD 项明确 unsupported/deferred。
