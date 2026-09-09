# ADR 0002：NCK 外部交叉验证（无真机阶段的三源比对结论）

> 状态：accepted（验证方法） / open（两处分歧待真机或上游澄清后仲裁）。
> 验证日期：2026-09-09；对照源均取当日 mainline（Wireshark / Sharp7 soft79 master /
> Softing datafeed-edge-connector master）。

## 背景

`sinumerik-nck` 地基（PR #17）合并后，真机（PR7）之前需要无真机验证。
本 ADR 记录三个独立源与 Mesa 实现的逐项比对结论：**7 处一致，2 处分歧**。
一致项进入回归门；分歧项冻结为 open 决策，任何一方不得静默“修掉”。

## 核对矩阵

| # | 断言 | Wireshark 主干 | Sharp7 | Softing 文档 | Mesa | 结论 |
|---|------|----------------|--------|--------------|------|------|
| 1 | item 布局 `12 08 syntax areaunit col:u16 line:u16 module count`（10B） | `s7comm_syntaxid_nck`（type`0x12`+len 8 分发） | `S7Nck_MRD_ITEM` 模板 + 单读填充偏移一致 | AWL 字段顺序一致 | `codec::encode_var_spec` | ✅ 四源一致 |
| 2 | syntax `0x82/83/84` = current/metric/inch | `#define` + 名表 | 仅实现 `0x82` | AWL `SYNTAX_ID := B#16#82` | 三档全实现 | ✅ 一致（Sharp7 无 83/84，不做其差分） |
| 3 | Area 索引 N=0 B=1 C=2 A=3 T=4 V=5 H=6 MMC=7 | `nck_area_names` | 无命名常量（见分歧 A） | 字母分区一致 | `NckArea::index` | ✅ 三源一致 |
| 4 | area/unit 合成 `area<<5\|unit` | `area>>5` / `&0x1f`，掩码 `0xe0`，注释 `aaauuuuu` | ❌ `NckArea<<4`（见分歧 A） | ✅ 例证：N/unit1=`0x01`，C/unit1=`0x41` | `area<<5\|unit` | ✅ 站 Wireshark+Softing 多数派 |
| 5 | 响应 length 单位：`0x03/04/05` 按 bit，`0x06/07/09` 按 byte | `#define` 注释 + 三处解码数学一致 | 部分一致（见分歧 B） | — | `wire_data_len` | ✅ 站 Wireshark |
| 6 | 奇长 payload 补 1 字节对齐（除末项） | `len%2 && 非末项 → len2=len+1` | `ItemSize%2 → ++`（末项也加，仅越过包尾，无害） | — | 填充启发式 | ✅ 一致 |
| 7 | BAD 项返回码非 FF 按项隔离 | `ret_val` 分支 | `Result=CpuError`，但**只跳 4 字节头**（BAD 载荷会带偏后项） | — | 跳过完整载荷 | ✅ Mesa 比 Sharp7 更严格 |
| 8 | NCK/PLC TSAP 必须区分 | — | — | NCK `03 03` vs PLC `03 02`（powerline 语境） | 连接显式 TSAP | ✅ 方向一致（具体值仍待真机，不抄 powerline 值） |
| 9 | module 语义（Y/FU/TO/RP…） | `nck_module_names`（`0x10` Y…） | `NckModule` 不透明透传 | GUD `BlockType` 值表 | Catalog 持有、不猜 | ✅ 字段语义一致；具体变量 mapping 仍空表待真机 |

## 分歧 A：`<<4` vs `<<5`（open，真机仲裁）

- Sharp7 三处（单读/多读/多写）均为 `NckArea << 4 | NckUnit`，且库内无 Area
  命名常量、无取值文档。
- Wireshark（`>>5`/`&0x1f`/`0xe0`）与 Softing 例证值（`0x01`、`0x41`）
  互相印证 `<<5`（取 area 索引 0–7、unit=1 代入，两例全合）。
- 反例权重：Sharp7 是经真机使用的 HMI 库，若 `<<4` 纯错则 NCK 功能全不可用，
  故不排除其调用方传入的是预移位值（如 `0,2,4,…`）——但**无证据**，
  不得作为立论依据。
- 冻结决策：Mesa 跟随 Wireshark+Softing（`<<5`）；Sharp7 只做布局/响应差分，
  **不做 areaunit 字节值差分**，直到真机或 Sharp7 上游澄清。

## 分歧 B：`0x06` 长度单位（open，影响面极小）

- Wireshark：`0x06` 按 byte；Sharp7：`>>3`（按 bit）。
- Mesa 跟随 Wireshark（`0x06/0x07/0x09` 按 byte；`0x03` 取 `div_ceil`，
  与 Wireshark 数学逐字一致；未知 transport 按 byte，见下）。
- 影响面：真机 Read 响应实际只出现 `0x03/0x04`（`0x07/0x09` 罕见，
  `0x06` 未见实证），且未知值按 byte 偏向 fail-loud（多消费→`READ_DATA_SHORT`），
  而非静默错位。真机若出现 `0x06` 即按本 ADR 重审。

## Sharp7 单变量 PDU 拆分（未来 P2-1 的第二证据）

Sharp7 `ReadNckArea` 已实现同变量跨 PDU 拆分，语义为：同
（area, module, parameter）+ `Start += NumElements` + `Amount` 分块，
预算 `MaxElements = (PDU-18)/WordSize`。Mesa P2-1 曾拒绝猜测 line 分段语义；
本条作为第二独立证据记录，待真机确认后实现（届时与 Sharp7 差分拆分点）。

## 方法冻结

- Sharp7 = 布局/响应/拆分行为的**一票**，不是 oracle；
- Wireshark dissector 源码 = 协议 oracle（静态核对已入库，零 CI 成本）；
- Softing/官方文档 = 字段语义与例证值来源；
- 真机 = 最终仲裁（TSAP、wire address、catalog mapping 三项只能真机证明）。
- 下一步：Layer4 standalone emulator（本分支后半部分），Sharp7 互操作待
  emulator 就绪后接入；tshark PCAP 门暂缓（runner 无 tshark，静态核对已覆盖九成价值）。
