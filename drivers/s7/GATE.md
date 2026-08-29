# S7 Gate 闭环文档（Siemens PLC）

## 1. 访问范式

- **绑定** `s7.address-group` `TaskMode Poll` `interval_ms`
- **地址** `DB10.DBD20 / DBX24.0 / MB10 / IB0 / QB0 / C0 / T0 / PIW0 / PQW0` `area DB/M/I/Q/C 0x1C/T 0x1D/PI 0x80` `bit 0..7` `parse_address` `C/T bit_address 不×8`
- **类型** `BOOL/BYTE/WORD/DWORD/INT/DINT/REAL/LREAL/STRING(256)/WSTRING(516)/S5TIME(T/C)/TIME/DATE/DT + counter/timer 别名` 大端 `codec` `STRING 按 cur_len 截断 WSTRING UTF-16BE` `S5TIME/Timer 2B BCD`
- **诊断** `SZL 0x0011/0x0131` `CLOCK` `build_szl_req 0x07 UserData 0xFF09` `read_szl` 透传（Common 只读，不入点位批次）

## 2. 连接诊断

- **COTP** `CR 0xE0 → CC 0xD0` `TSAP 0x0100 rack<<5` `S7 Setup PDU480` `TPKT 0x03 FUNC_READ 0x04` `PDU 480 动态分片 19→(PDU-32)/20 LREAL 8字节重估` `连续同 DB 合并排序`
- **错误** `0x04 PUT/GET 未启用 → TIA Portal 勾选` `0x05 越界` `0x03 保护`
- **真机** `192.168.15.97:102 DB10.DBD0 DWORD 11468800 DBW0 175 STRING 动态 97/165 双机 97 S7 165 FOCAS` `ARM QEMU aarch64 2.1M` `RUNNING 2点+4点 11点 GOOD`

## 3. Common 补全

- [x] `C 0x1C` `T 0x1D` 字寻址 `0..2047` `bit_address 不×8`
- [x] `PI/PE 0x80` `PQ/PA 0x80` 外设 `PIW256/PQW0` 字
- [x] `SZL 0x07` `build_szl_req` `parse_szl_resp` Common 诊断直通
- [x] `counter/timer` 别名 `WORD/S5TIME` `11 单测 dispatch 0 failed`

## 4. 发布 Gate

- [x] `DB/M/I/Q/C/T/PI/PQ` 解析与 `BOOL` 按 `BYTE` 读后取位
- [x] `COTP/S7` 握手与 `READ 19` 分片+奇偶填充 + `SZL UserData`
- [x] `97 DB10 动态` `ARM QEMU` `11 单测` 通
