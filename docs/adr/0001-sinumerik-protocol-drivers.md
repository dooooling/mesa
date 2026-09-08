# ADR 0001：按实际通信协议划分 SINUMERIK 相关 Driver

- 状态：已冻结（不考虑历史兼容，边界一次切干净）
- 日期：2026-09-08
- 关联：PR1（`s7-transport` 抽取）→ PR2（OPC UA `nsu=` canonical）→ PR3（删 `sinumerik` 建 `sinumerik-nck` 空壳）→ PR4–PR7（NCK 读/topology/真机）

## 决策

Mesa 按实际通信协议划分 Driver，不按设备品牌划分：

| Driver | ID | 职责 |
|---|---|---|
| OPC UA | `opcua` | 所有标准 OPC UA Server，含 SINUMERIK OPC UA |
| Siemens S7 | `s7` | SIMATIC PLC Classic S7Comm / S7ANY（Syntax `0x10`） |
| SINUMERIK NCK | `sinumerik-nck` | 原生 NCK S7Comm HMI access（Syntax `0x82/83/84`） |

含义模糊的 `sinumerik` ID 退役（`SINUMERIK != 一种协议`，经 OPC UA / NCK native / PLC S7 三种路径可达）。

## 原则

1. OPC UA 就是 OPC UA：`drivers/sinumerik` 的 OPC UA 实现删除，`canonical.rs`
   的 `nsu=` 解析 + NamespaceArray 换算下沉到公共层（`mesa-opcua-transport` 侧），
   通用 `opcua` Driver 只接受 canonical `nsu=` 身份（`ns=<index>` 拒绝）。
2. `s7` 只负责 PLC S7ANY；NCK 是独立驱动。两者只共享 `mesa-s7-transport`
  （TCP/TPKT/COTP/Setup/ReadVar 不透明字节），不共享地址语义。
3. `ResourceSelection` 统一 Envelope、不统一协议参数：OPC UA 用 `nsu=`，
   NCK 用 `variable` + `Area/AreaNo/Block/Variable/Line/Column` 结构化参数，
   类型与 wire mapping 由版本化 NCK Catalog（`drivers/sinumerik-nck/catalog/*.json`）决定。

## NCK V1 边界

- 能力：Probe / Catalog Browse / Poll / MultiRead（含 PDU 自动分片、逐项 BAD 隔离）。
- 不做：Subscribe（ReadVar 本质是请求/响应，不伪造）、Write、Command、Event。
- 硬约束：`source_timestamp_ns = None`（S7 ReadVar 无此语义，不伪造）；
  `Transport Security: None`（依赖 OT 网络隔离，UI/diagnostics 明示）；
  所有 wire mapping（module/column、TSAP 默认、probe anchor）必须由官方变量定义 +
  确定性协议测试 + 真机三方确认后才能进 `supported` catalog，记忆数字不进仓。
- 测试门：S7 零回归（PR1）→ exact-bytes golden + 回环 fixture（PR4）
  → driver contract + exact-count stress（PR5）→ 840D 真机证据（PR7，828D 独立 Gate）。

## Core 契约

不动 Core Profile 契约；提供 `sinumerik-840d-opcua.json` /
`sinumerik-840d-nck.json` 两套 profile，point key 语义对齐，底层 mapping 各走各的。
Core 不懂协议的硬约束保持：transport crate 只被 Driver 依赖，Core 零依赖。
