# S7 Gate 闭环文档（Siemens PLC）

## 1. 访问范式

- **绑定** `s7.address-group` `TaskMode Poll` `interval_ms`
- **地址** `DB10.DBD20/DBX24.0/MB10/IB0/QB0/C0/T0/PIW0/PQW0/VB0/VW0/V0.0/SM0.0/SMB0/AIW0/AQW0/L0.0/LB0` `area DB 0x84/M 0x83/I 0x81/Q 0x82/C 0x1C/T 0x1D/PI 0x80/L 0x86` `V→DB1 SM→M AI→PI AQ→PQ L→0x86` `bit 0..7` `C/T 不×8`
- **类型** `BOOL/BYTE/WORD/DWORD/INT/DINT/REAL/LREAL/STRING(256)/WSTRING(516)/S5TIME(T/C)/TIME/DATE/DT` 大端 `codec` `STRING cur截断 WSTRING UTF-16BE` `12 型+V/SM/AI/AQ/L别名`
- **诊断** `SZL 0x0011 106B/0x0131` `CLOCK` `build_szl_req 0x07 UserData 0xFF09` `read_szl 97 106B GOOD`

## 2. 连接诊断

- **COTP** `CR 0xE0 → CC 0xD0` `TSAP 0x0100 rack<<5` `S7 Setup PDU480` `TPKT 0x03 FUNC_READ 0x04` `PDU 480 动态分片 19→(PDU-32)/20 LREAL 8字节重估` `连续同 DB 合并排序`
- **错误** `0x04 PUT/GET 未启用 → TIA Portal 勾选` `0x05 越界` `0x03 保护`
- **真机** `192.168.15.97:102 DB10.DBD0 7340032 MW0 0 VB0 0 SM0.0 false 4/4 GOOD PIW0 0x06/AQ/L 0x06(无硬件)` `SZL 0x0011 106B` `ARM QEMU aarch64 2.1M`

## 3. Common 补全

- [x] `C 0x1C` `T 0x1D` `bit_address 不×8 transport 0x1C/0x1D len1`
- [x] `PI/PE/AI 0x80` `PQ/PA/AQ 0x80` `AIW0/AQW0→PI/PQ`
- [x] `V→DB1 VB0/VW0/V0.0` `SM→M SMB0/SM0.0` `L 0x86 LB0/L0.0`
- [x] `SZL 0x07 0x0011 106B` `read_szl 97 GOOD` `12 单测`

## 4. 发布 Gate

- [x] `DB/M/I/Q/C/T/PI/PQ/V/SM/AI/AQ/L` 全量解析与 `BOOL` 按 `BYTE` 取位
- [x] `COTP/S7` 握手与 `READ 19` 排序不丢位分片+奇偶填充 + `SZL UserData`
- [x] `97 4/4 GOOD + SZL 106B` `ARM QEMU` `12 单测` 通
