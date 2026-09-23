# S4-B2 NCGuide 无负载结论（冻结）

日期：2026-09-23（UTC+8 现场窗口）
对象：servo/load 自然非零证据（S4-B2 attempt #1）
分支基线：`main@6253df9`（不依赖任何证据分支；`PktMon.etl` HOLD）

## 现场输入

- Motion：`O0001` 运行中，位置变化 `X = 3.267`（motion=yes）
- Panel `SERVO LOAD METER`：`X = 0% / Y = 0% / Z = 0%`（variation 不存在）

## 结论（冻结）

```text
轴运动 ≠ servo load variation
servo load 反映负载/扭矩需求，不是位置变化
NCGuide 空走/轻载运动无负载模型（no load model）
```

## 纪律（冻结）

```text
Panel first：servo load > 0 才采 Native + 抓 Wire
本次：variation 不存在 → 不采 Native、不抓 Wire、不写 codec、不改分支
原因：零窗口只能得到零数据，无覆盖率价值；不为覆盖率制造无意义数据
```

## 状态机

```text
S4-A servo contract（typed + U32 + raw12 保留） ✅ CLOSED
S4-B1 servo Wire identity（0xA4/0x89/0x56/0xA4 + name parity） ✅ OBSERVED
S4-B2 natural non-zero ⏳ WAITING REAL LOAD WINDOW
  attempt #1：motion=yes / load=0 → NCGuide no load model ✅ RECORDED
production codec ⛔ HOLD（0 时正确 ≠ 非零正确）
```

## 下一次窗口条件（按优先级）

1. 真实切削（吃刀量 > 0）：`G01/G02/G03` 实际吃刀 → 面板 `SERVO LOAD > 0%`
2. 真实机床重载（Z 轴重力/大惯量加减速，面板 gate 为准）
3. NCGuide 不再投入（no load model 已证；逼不出负载）

触发后流程：`Native cnc_rdsvmeter` + 同窗 Wire capture + `LOADELM.data` diff，
三方闭合后才写 production codec。
