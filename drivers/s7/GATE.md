# S7 Gate 闭环文档（Siemens PLC）

## 1. 访问范式

- **绑定** `s7.address-group` `TaskMode Poll` `interval_ms`
- **地址** `DB10.DBD20 / DBX24.0 / MB10 / IB0 / QB0` `area DB/M/I/Q` `bit 0..7` `parse_address`
- **类型** `BOOL/BYTE/WORD/DWORD/INT/DINT/REAL/LREAL/STRING(256)/WSTRING(516)/S5TIME/TIME/DATE/DT` 大端 `codec` `STRING 按 cur_len 截断 WSTRING UTF-16BE`

## 2. 连接诊断

- **COTP** `CR 0xE0 → CC 0xD0` `TSAP 0x0100 rack<<5` `S7 Setup PDU480` `TPKT 0x03 FUNC_READ 0x04` `PDU 480 动态分片 19→(PDU-32)/20 LREAL 8字节重估` `连续同 DB 合并排序`
- **错误** `0x04 PUT/GET 未启用 → TIA Portal 勾选` `0x05 越界` `0x03 保护`
- **真机** `192.168.15.97:102 DB10.DBD0 DWORD 11468800 DBW0 175 STRING 动态 97/165 双机 97 S7 165 FOCAS` `ARM QEMU aarch64 2.1M` `RUNNING 2点+4点 11点 GOOD`

## 3. 发布 Gate

- [x] `DB/M/I/Q` 解析与 `BOOL` 按 `BYTE` 读后取位
- [x] `COTP/S7` 握手与 `READ 19` 分片+奇偶填充
- [x] `97 DB10 动态` `ARM QEMU` 通
