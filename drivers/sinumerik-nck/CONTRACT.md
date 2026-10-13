# SINUMERIK NCK Driver 契约（V1 只读 · 原生寻址）

## 身份

- `driver_id = "sinumerik-nck"`（`sinumerik` 已退役，见 ADR 0001）。
- 传输：`mesa-s7-transport`（Classic S7Comm ReadVar），NCK Syntax
  `0x82`（current）/`0x83`（metric）/`0x84`（inch），线缆布局以 Wireshark
  `packet-s7comm.c s7comm_syntaxid_nck` 为准，真机 Gate 确认。

## 资源

- `resource_id = "variable"`，**唯一**资源、唯一输出 `value`、`Read` + `Poll`、
  `OutputTypeSpec::DriverResolved`。
- 参数是**协议字段本身**，不存在变量名目录、不存在访问路径开关：
  `area`（N/B/C/A/T/V/H）、`area_no`（0..31）、`module`（0..255）、
  `column`（0..65535）、`line`（0..65535）、`count`（1..255，是线缆
  line_count 即元素个数，不是字节数）、`unit_mode`（current/metric/inch）、
  `decode`（规范词表，见下）、`transport_size`（响应枚举 `0x05/0x06/0x07/0x09`）、
  `invalid_value` + `invalid_tolerance`（可选，必须成对）、`unit`（可选展示注释）。
- **必填由 Driver 独立实施**：不依赖 UI 自动填充，也不假设 Core 已前置校验。
  字符串整数、小数、布尔、null、负数、越界一律拒绝；未知字段立即拒绝。
- **旧模型字段（`block`/`variable`/`access_path`/`shape`/`supported_families`/
  `catalog_file`/`retain_unresolved_points`）不被接受，也不做自动迁移**。
  旧 NCK Task 必须清理重建，不允许“保存正常但运行读了另一物理地址”。
- `PointDescriptor.data_type` 由 `decode + count` 唯一决定；无第二类型来源。
  稳定诊断标识：`nck://read/<AREA>/<area_no>/<module>/<column>/<line>/<count>?mode=&decode=&transport=`
  （不含连接信息、业务名称、无效值规则；`unit` 也不进标识）。

### `decode` 规范词表（15 项，拒绝别名）

`F64_BE` `F64_LE` `F32_BE` `I32_BE` `I32_LE` `U32_BE` `U16_BE` `U16_LE`
`I16_BE` `U8` `BOOL` `UTF8_FIXED_16` `UTF8_NUL_LAST_32` `UTF8_NUL_FIRST_66`
`UTF16_BE`

- 小端变体必须显式声明，**不根据数值大小猜字节序**；`REAL`/`DINT`/`LREAL`
  等历史别名不通过新配置。
- 通用数值/BOOL/`UTF8_FIXED_16`：`count=1` 标量，`>1` 同口径数组。
  `UTF16_BE`/`UTF8_NUL_LAST_32`/`UTF8_NUL_FIRST_66` 的多行组成**一个字符串**，
  即使 `count>1` 仍是标量 String（如 `p0199` 读 25×U16BE = 50 字节 → 一个对象名）。
- `expected_data_len = count × decode.byte_len()`，用 checked 乘法；
  用户不填字节数，也不允许驱动按类型猜长度。

## 能力边界（V1 冻结）

- Probe / Browse（**模板预填 + 只读身份核对**）/ Poll / MultiRead（含 PDU
  自动分片、逐项 BAD 隔离）：做。
- Subscribe（不伪造）、Write、Command、Event：不做。
- `source_timestamp_ns` 恒为 None（S7 ReadVar 无此语义）。
- `Transport Security: None`（依赖 OT 网络隔离，UI/diagnostics 明示）。

## 采集准入 = 原生字段校验（无目录白名单）

任何结构合法、`decode` 受支持且设备实际接受的地址都能 Configure 并只读
ReadVar——**与是否存在模板无关**。模板缺失、为空、甚至整个文件解析失败，
都不影响 Configure/Poll；模板错误只出现在浏览诊断里。

失败矩阵（不伪造 GOOD）：

| 失败类型 | 层级 | 结果 |
|---|---|---|
| 缺参/越界/未知字段/非法 decoder 或 transport/规则不成对 | Configure | 拒绝整个新计划，旧有效快照不被覆盖 |
| 单项 `return_code != 0xFF` | 单点 | BAD；`quality_code` 为设备原始码 |
| `return_code=0xFF` 但 transport、长度或解码不匹配 | 单点 | BAD；`quality_code=None`（不伪造设备诊断码），原因见 warn 日志 |
| NaN/Inf、已声明无效工程值（含数组任一元素） | 单点 | BAD |
| 作业拒绝、PDU Ref 异常、TPKT/COTP 错误、超时 | 本轮尝试/会话 | 失败并按既有恢复逻辑处理 |
| BAD 且有 last-known / 无 last-known | 单点 | LastKnown / 类型正确的 Placeholder，均标 BAD |

## Preset 铁律（配置素材，不是准入）

- `presets/*.json` 只提供**结构固定字段**预填；`area_no` 与 `line` 一律留给
  使用者确认（不猜轴/通道/行索引语义）。
- 模板装载时用**同一套原生解析规则**校验固定字段；不合法的模板被拒绝并
  出现在浏览诊断里，绝不静默采纳。
- `validation_status` 恒为 `candidate`：保留候选名称不代表其物理含义已验证，
  也不构成硬件结论。缺必填由通用 required 前置拦住保存。
- 模板数值来自已冻结的候选定义（依据见 `presets/README.md`），
  记忆数字不进仓；`p0199` 的两条入口拆成独立模板，编号不得互相换算。

## 无效工程值规则是显式配置

- 未配置 `invalid_value`/`invalid_tolerance` 时，驱动只做协议级与通用数值
  合法性检查，**不做工程语义判定**，也不谎称具有同等保护。
- 规则可作用于数值标量、数值数组与可安全投影的整型；文本/BOOL 上配置规则
  在 Configure 拒绝。
- 例：`r0035` 设备无效值 `-200`（容差 `0.001`）——即使协议返回 FF 也判 BAD，
  `quality_code` 不伪造设备错误；传感器/PTC/参数设置需另行核对。

## 驱动对象入口与身份核对

- 入口就是 `module` 字段本身（NC 变量入口 module 26、显式驱动对象入口
  module 130）；**不再有 `access_path`**，任意 module 号原样编码，不做白名单替换。
- 两条入口的实例编号**分别填写**（实体机实测：同一 `V/n` 在两条入口返回
  不同对象名）；按 `p0199` 唯一对象名关联，不依据名称后缀、IP、机床轴序号
  或同编号推断。
- `r0078` 是转矩产生电流，`r0080` 是未滤波实际转矩；不改名充当 `r0027`/`r0031`。
- NCK 整数 transport `0x05/0x06/0x07/0x09` 的实测**字节**长度规则只对 NCK
  请求语法生效，PLC ANY 的位长度规则保持不变（两者不得互套）。
- 身份根目录纯本地；轴/对象列表分别按下一实例编号分页，最多 31 实例，
  15 秒总超时。轴节点仅读该轴（最多 5 项），对象节点仅读其 `p0199`（1 项）。
  临时 Browse 连接不宣称跨请求缓存；候选入口独立展示，按对象名人工核对。
- 身份探针是**驱动内建诊断常量**（`src/identity_spec.rs`），不查目录、
  不依赖模板文件存在；未取得真实证据的系列（如 `840d-sl`）明确提示
  “身份浏览待验证”，不套用 828D 探针。
- 成功的空名称与设备拒绝分开。`0x8201` 及单项码在诊断中原样保留，
  枚举拒绝明确提示未完成，不把拒绝或本地解码失败当作已确认的目录表尾；
  过滤不得隐藏诊断节点。

## 公共通信及 NCK 通用配置

- 连接只决定「连哪台设备」：`host`/`port`/`family`/TSAP/`timeout_ms`/
  `pdu_length`/`max_read_items`。已声明系列为 `828d` 与 `840d-sl`；
  **`family` 不是读取许可**（不限制任何地址），但未声明的系列在连接阶段拒绝，
  避免静默接受后模板与身份探针都不可用却无提示。
- `max_read_items` 限制单个 ReadVar 请求项数，范围 1..19，统一缺省 19；
  请求项数限制与公共传输层的 PDU 长度分片同时生效。
- COTP DT 按 EOT 重组；接收共享同一次超时约束，上限 16 帧、8192 字节。
- 完整作业拒绝 ACK 保留原始错误码并使本次读取失败；错误引用号、
  截断或超时同样失败，不按设备分类切换错误策略。
- 单项超出协商 PDU 必须明确失败（`READ_ITEM_TOO_LARGE`），不把一次
  line_count 静默切成多个地址拼接。
- 类型支持与候选模板均不代表通过硬件 Gate；`840d-sl` 暂无模板，
  828d 提供公共候选模板。真实系列需独立形成官方定义、报文与 HMI 对照证据。

## 升级与迁移（破坏性，无自动转换）

从目录寻址版本升级到原生寻址版本时：

1. **先备份**当前 endpoint / task 配置（REST 导出即可；本机示例见
   `target/nck-legacy-backup/`，含 endpoints/tasks/point_registry）。
2. **停止旧 NCK Task 再升级驱动**。旧配置在连接后会被 Configure 明确拒绝，
   端点停在 `RECONNECTING` 并重复失败：
   `AddressError/INVALID_VARIABLE: point <key>: 未知字段 block（新 Resource
   不接受旧 block/variable/access_path 等字段，也不做自动迁移）`。
   该拒绝是**设计行为**，不是故障；不要期待旧点位自动恢复。
3. **按新结构重建**：从 `nck://presets` 选用模板 → 确认 `area_no` 与 `line`
   → 保存。或直接手填原生字段。
4. **人工核对物理含义**：重建后 `point_key` 若沿用旧名，必须确认其**单位、
   类型与物理含义**确实未变；不得为沿用历史 `point_id` 而把不同语义的数据
   接到同一个 key 上。

不提供：旧 JSON → 新 JSON 自动迁移器、旧 `source_label` 稳定格式承诺、
旧点位恢复即有原数据类型。

地址字节层面的旧→新等价已由 `tests/legacy_equivalence.rs` 用删除前冻结的
向量逐条断言（62 条）；等价只覆盖**地址**，不覆盖工程语义。决策与取舍见
`docs/adr/0004-nck-native-addressing.md`，候选定义来源见
`presets/PROVENANCE.md`。

