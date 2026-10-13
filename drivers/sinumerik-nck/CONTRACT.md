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
  `column`、`count`、`unit_mode`（current/metric/inch）、
  `access_path`（缺省nc_variable；drive_object需目录明确声明候选入口）。
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
必须记录可追溯依据，记忆数字不进仓。公共候选映射可用于解析与只读验证，
必须注明 `validation_status=candidate`、`hardware_validated=false`；
标记为 supported 仍须官方变量定义、确定性协议测试和真实828D设备验证。
`supported_families` 是解析时的系列归属，不代表已通过硬件Gate。
候选映射的适用范围与未确认项见 `catalog/README.md`。

## 公共通信及NCK通用配置

- 连接系列使用既有 `828d` / `840d-sl`，按统一注册表加载系列目录；
  不提供虚拟机专属系列、单文件实验目录入口或未知变量运行期保留开关。
- `max_read_items` 限制单个ReadVar请求项数，范围1..19，统一缺省19；
  请求项数限制与公共传输层的PDU长度分片同时生效。
- COTP DT按EOT重组；接收共享同一次超时约束，上限16帧、8192字节。
- 完整作业拒绝ACK保留原始错误码并使本次读取失败；错误引用号、
  截断或超时同样失败，不按设备分类切换错误策略。
- 目录可声明 `F64_LE`、`I32_LE`、`U16_LE`；U16提升为Core U32。
  未声明小端的已有类型行为不变，驱动F32仍为大端。
- `UTF8_NUL_LAST_32` 与 `UTF8_NUL_FIRST_66` 提供目录显式选择的文本
  组成行解码；输出Core String，无效UTF-8按项BAD，不扩大为通用字符串支持。
- 类型支持与候选目录均不代表通过硬件Gate；common/840d-sl目录保持空，
  828d目录提供公共候选定义。真实系列需独立形成官方定义、报文与HMI对照证据。

## 驱动对象入口与身份核对

- 默认入口地址及canonical key保持兼容；drive_object参与canonical身份，
  仅选择条目wire.drive_object_module，不接受用户任意模块号。
- 两条入口的实例编号分别填写；按p0199唯一对象名关联，不依据名称后缀、
  IP、机床轴序号或同编号推断。单位模式与入口选择互不替代。
- r0078是转矩产生电流，r0080是未滤波实际转矩；不改名充当r0027/r0031。
- 支持显式U8、I16_BE、U16_BE及UTF16_BE对象名；NCK整数transport=0x05
  的实测字节长度规则只对NCK请求语法生效，PLC ANY位长度规则保持不变。
- 身份根目录纯本地；轴/对象列表分别按下一实例编号分页，最多31实例，
  15秒总超时。轴节点仅读该轴（最多5项），对象节点仅读其p0199（1项）。
  临时Browse连接不宣称跨请求缓存；候选入口独立展示，按对象名人工核对。
- 成功的空名称与设备拒绝分开。0x8201及单项码在诊断中原样保留，
  枚举拒绝明确提示未完成，不把拒绝或本地解码失败当作已确认的目录表尾。
