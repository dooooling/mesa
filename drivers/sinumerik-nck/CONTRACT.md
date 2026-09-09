# SINUMERIK NCK Driver 契约（V1 只读）

## 身份

- `driver_id = "sinumerik-nck"`（`sinumerik` 已退役，见 ADR 0001）。
- 传输：`mesa-s7-transport`（Classic S7Comm ReadVar），NCK Syntax
  `0x82`（current）/`0x83`（metric）/`0x84`（inch），线缆布局以 Wireshark
  `packet-s7comm.c s7comm_syntaxid_nck` 为准，真机 Gate 确认。

## 资源

- `resource_id = "variable"`（Siemens 称 NC Variable）。
- 参数（结构化，不接受手写地址字符串）：
  `area`（N/B/C/A/T/V/H）、`area_no`、`block`、`variable`、`line`、
  `column`、`count`、`unit_mode`（current/metric/inch）。
- `data_type` 来自 NCK Catalog，不由用户填写。
- 内部 canonical 身份：`nck://<AREA>/<NO>/<BLOCK>/<VAR>?line=&column=&count=&unit=`。

## 能力边界（V1 冻结）

- Probe / Browse（Catalog + Topology 虚拟树）/ Poll / MultiRead（含 PDU
  自动分片、逐项 BAD 隔离）：做。
- Subscribe（不伪造）、Write、Command、Event：不做。
- `source_timestamp_ns` 恒为 None（S7 ReadVar 无此语义）。
- `Transport Security: None`（依赖 OT 网络隔离，UI/diagnostics 明示）。

## Catalog 铁律

`catalog/*.json` 的 wire 数值（module/column/transport_size/element_size）
必须由官方变量定义 + 确定性协议测试 + 真机三方确认后方可进入；
记忆数字一律不进仓。空 catalog + 占位 schema 先行，数据由真机证据回填。
