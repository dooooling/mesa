# 828D 候选定义保留/弃用清单与来源

数据来源：`catalog/828d.json @ main@78e23ec`（PR-B 删除该文件前）。
每条候选**全部保留**为预填模板（`validation_status=candidate`），
但“保留”只表示协议定义与来源可追溯，**不表示该变量的工程语义已验证**。

模板 id 规则：`828d.<area>.<block>.<variable>.<nc_variable|drive_object>`。

## 定义清单（42 条）

| # | area/block/variable | decode | transport | 元素字节 | module（入口） | 语义限制 |
|---|---|---|---:|---:|---|---|
| 1 | `C/S/progStatus` | `U16_LE` | `0x09` | 2 | 127 | — |
| 2 | `B/S/opMode` | `U16_LE` | `0x09` | 2 | 127 | — |
| 3 | `C/SPARPP/progName` | `UTF8_NUL_LAST_32` | `0x09` | 32 | 122 | — |
| 4 | `C/SPARP/progName` | `UTF8_NUL_LAST_32` | `0x09` | 32 | 125 | — |
| 5 | `C/SPARP/actLineNumber` | `I32_LE` | `0x09` | 4 | 125 | — |
| 6 | `C/SPARP/actBlock` | `UTF8_NUL_FIRST_66` | `0x09` | 66 | 125 | — |
| 7 | `C/SGA/actToolBasePos` | `F64_LE` | `0x09` | 8 | 115 | LX coordinate label is preserved in imported point keys; use the official setpoint/MCS or actual/BCS meaning, not a guessed encoder/WCS value |
| 8 | `C/SEGA/actProgPosBKS` | `F64_LE` | `0x09` | 8 | 112 | LX coordinate label is preserved in imported point keys; use the official setpoint/MCS or actual/BCS meaning, not a guessed encoder/WCS value |
| 9 | `C/S/actFeedRateIpo` | `F64_LE` | `0x09` | 8 | 127 | — |
| 10 | `C/S/cmdFeedRateIpo` | `F64_LE` | `0x09` | 8 | 127 | — |
| 11 | `C/S/feedRateIpoOvr` | `F64_LE` | `0x09` | 8 | 127 | — |
| 12 | `C/SSP/actSpeed` | `F64_LE` | `0x09` | 8 | 114 | — |
| 13 | `C/SSP/cmdSpeed` | `F64_LE` | `0x09` | 8 | 114 | — |
| 14 | `C/SSP/speedOvr` | `F64_LE` | `0x09` | 8 | 114 | — |
| 15 | `C/S/actTNumber` | `U16_LE` | `0x09` | 2 | 127 | — |
| 16 | `C/RP/rpa` | `F64_LE` | `0x09` | 8 | 21 | — |
| 17 | `V/M/r0021` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 18 | `V/M/r0027` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 19 | `V/M/r0035` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 20 | `V/M/r0033` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | 828D NC variable list does not list r0033; real-controller availability remains unconfirmed |
| 21 | `V/M/r0031` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | 828D NC variable list does not list r0031; no successful network readback; VM returned item 0x85 |
| 22 | `V/M/r0062` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 23 | `V/M/r0063` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 24 | `V/M/r0064` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 25 | `V/M/r0077` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 26 | `V/M/r0078` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 27 | `V/M/r0079` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 28 | `V/M/r0080` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 29 | `V/M/r0081` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 30 | `V/M/r0082` | `F32_BE` | `0x07` | 4 | 26、130（驱动对象） | — |
| 31 | `V/M/r0103` | `U16_BE` | `0x06` | 2 | 26、130（驱动对象） | — |
| 32 | `V/M/p0105` | `I16_BE` | `0x05` | 2 | 26、130（驱动对象） | — |
| 33 | `V/M/r0106` | `I16_BE` | `0x05` | 2 | 26、130（驱动对象） | 828D SINAMICS参数手册08/2018确认Integer16活跃/不活跃状态；simDrive03/2025未单列该参数。当前软件控制器及实体828D只读响应已分别核对。 |
| 34 | `V/M/r0107` | `I16_BE` | `0x05` | 2 | 26、130（驱动对象） | — |
| 35 | `V/M/r0108` | `U32_BE` | `0x06` | 4 | 26、130（驱动对象） | — |
| 36 | `V/M/p0199` | `UTF16_BE` | `0x06` | 2 | 26、130（驱动对象） | — |
| 37 | `N/M/MDSA_AXCONF_MACHAX_NAME_TAB` | `UTF8_FIXED_16` | `0x09` | 16 | 26 | — |
| 38 | `A/M/MDCA_CTRLOUT_MODULE_NR` | `U8` | `0x09` | 1 | 26 | — |
| 39 | `A/M/MDCA_ENC_MODULE_NR` | `U8` | `0x09` | 1 | 26 | — |
| 40 | `A/M/MDCA_CTRLOUT_TYPE` | `U8` | `0x09` | 1 | 26 | — |
| 41 | `C/M/AXCONF_MACHAX_USED` | `U8` | `0x09` | 1 | 26 | — |
| 42 | `A/M/SPIND_ASSIGN_TO_MACHAX` | `U8` | `0x09` | 1 | 26 | — |

## 证据基线

全部条目的共同依据（逐条具体来源存于原 `evidence` 字段与模板 `description`）：

- Siemens 828D *NC variables*, 03/2025, A5E53370586B AC；
- Siemens 828D 参数手册 02/2012；
- SINAMICS S120/S150 参数手册 11/2023（`r0035` 无效值标记，p.71）；
- LX 驱动读取实现审计；NCK 报文与响应长度核对；
- 仓库锁定的 Wireshark 来源（area/unit 位布局、`0x82/0x83/0x84` syntax）。

虚拟控制器证据（`docs/evidence/nck-drive-object-readback.json`）只证明该环境的
协议行为，**不能代替 828D 硬件证据**。实体机只读实测见
`docs/evidence/nck-live-native-probe.txt`。

## 未纳入模板的项（不得填假地址）

原 190 点中的 7 项**不是单个 NC 变量**，不属于本清单，也没有对应模板：

| 点位 | 说明 |
|---|---|
| 设备逻辑状态 | LX 派生状态，不是可寻址 NC 变量 |
| 系统信息 | 需独立资源实现 |
| 主程序注释 / 当前程序注释 | 非单个 NC 变量 |
| 报警号 / 报警类型 / 报警信息 | 报警汇总需独立资源实现 |

详细对账（含 12 项轴归属未证明、1 项无效主轴温度）见
`docs/evidence/nck-remaining-20-reconciliation.md`。

## 等价门

每条候选的期望线缆字节已冻结在 `tests/fixtures/legacy-828d-wire.json`，
由 `tests/legacy_equivalence.rs` 断言原生寻址路径逐条复现——
删除目录不得改变任何既有地址的物理含义。
