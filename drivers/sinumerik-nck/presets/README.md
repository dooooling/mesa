# 828D 预填模板（Preset）与身份探针说明

`828d.json` 是从原 `catalog/828d.json` 候选定义**转换**来的预填模板集合
（42 条候选 → 62 个模板；含 `drive_object_module` 的条目拆成 module 26 与
module 130 两个独立模板）。

> **Preset 不参与采集准入。** 它只是配置预填素材：模板是否可用、是否被
> 拒绝、甚至整个文件解析失败，都**不影响**原生 `variable` 资源的
> Configure/Poll。地址能否采集只由原生字段是否合法决定。

所有条目均为 **candidate**：保留原导入名称不代表其物理含义已验证，
也不能据此认定 840D sl 适用。

## 依据

- Siemens 828D *NC variables*, 03/2025, A5E53370586B AC；
- Siemens 828D 参数手册 02/2012；
- SINAMICS S120/S150 参数手册 11/2023（`r0035` 无效值标记，p.71）；
- LX 驱动读取实现审计；NCK 报文与响应长度核对；仓库锁定的 Wireshark 来源
  （area/unit 位布局、`0x82/0x83/0x84` syntax）；
- 实体机只读实测：`docs/evidence/nck-live-identity-probe.txt`。
  虚拟控制器响应（`docs/evidence/nck-drive-object-readback.json`）只证明
  相应环境的协议行为，**不能代替 828D 硬件证据**。

## 模板只预填“结构固定字段”

每个模板只带 `area/module/column/count/unit_mode/decode/transport_size`
（以及可选 `unit`）。**`area_no` 与 `line` 一律留给使用者确认**——
它们分别是实例编号（通道/轴/驱动对象）与行索引（轴索引、R 参数号…），
不能由变量名推测。模板加载时会用**同一套原生解析规则**校验固定字段，
不合法的模板被拒绝并出现在浏览诊断里。

## 索引语义（来自原候选目录，供人工确认时参考）

| Area/Block | Variable | 类型 | 索引语义 |
|---|---|---|---|
| C/S | progStatus、actTNumber | U16 小端→U32 | 通道实例；line=1 |
| B/S | opMode | U16 小端→U32 | BAG 实例；line=1 |
| C/S | actFeedRateIpo、cmdFeedRateIpo、feedRateIpoOvr | F64 小端 | 通道实例；line=1 |
| C/RP | rpa | F64 小端 | line=R 参数号+1 |
| C/SGA | actToolBasePos | F64 小端 | line=轴索引；MCS 轴设定位置 |
| C/SEGA | actProgPosBKS | F64 小端 | line=轴索引；BCS 实际位置 |
| C/SSP | actSpeed、cmdSpeed、speedOvr | F64 小端 | line=主轴索引 |
| C/SPARPP | progName | 32 字节 NUL 文本 | line=程序层级；主程序 line=1 |
| C/SPARP | progName | 32 字节 NUL 文本 | 当前活动程序/子程序；line=1 |
| C/SPARP | actLineNumber | I32 小端 | 保留官方 -1/-2 哨兵值 |
| C/SPARP | actBlock | 66 字节 NUL 文本 | 当前执行程序块；line=1 |
| V/M | r0021、r0027、r0035 | F32 大端 | `area_no`=驱动访问编号，**不能从轴名称猜测** |
| V/M | r0033、r0031 | F32 大端候选 | r0033 有 LX 固定读取依据；r0031 在选定实体 828D 读回成功，通用适用范围仍待确认 |

> 上表是**人工确认时的参考**，不是自动填充规则；模板不会据此替你填
> `area_no`/`line`。

## 使用限制

- 保留原导入点位名称不代表其物理含义已验证。尤其“机械坐标”对应
  `actToolBasePos` 是轴设定位置，“绝对坐标”对应 `actProgPosBKS` 是 BCS 实际位置，
  不应自动当成编码器 MCS 实际位置或 WCS 值。
- 单位依赖控制器设置的变量不写死 mm/min、rpm 或 mm；需单独读单位或查机床设置。
- `r0031`、`r0033` 未列在该版 NC 变量表中。参数手册确认的是参数本身含义，
  不保证 NC 网络入口开放任意 SINAMICS 参数；已完成的选定实体机读回见
  `../GATE.md`，不由该结果推断所有 828D 都支持。
- 32/66 字节文本长度和 transport-size 属于候选协议定义；不一致时按项 BAD。
- 有定义但控制器拒绝返回有效值时仍报 BAD。完整作业拒绝使本次读取失败，
  不恢复虚拟机专属的容错分支，不捏造 GOOD 或零值。
- 设备逻辑状态、系统信息、程序注释及报警汇总不是单个 NC 变量。
  未建立相应公共资源实现前不填假地址，未知字段在 Configure 阶段拒绝。

## 访问入口与对象核对

原生模型里**没有 `access_path`**：入口就是 `module` 字段本身
（NC 变量入口 module 26、显式驱动对象入口 module 130）。
两个入口的**实例编号不同**（实体机实测：同一 `V/n` 在两条入口返回不同对象名），
必须各自读取 `p0199` 对象名核对，**禁止直接沿用编号或按名称后缀配对**。

`p0199` 以 25 个大端 U16 投影为一个 UTF-16 对象名
（`decode=UTF16_BE`、`count=25`）。`r0078[0/1]` 的网络 Line 分别为 1/2。
`r0078` 为转矩产生电流，不能冒充 `r0027`；`r0080` 未滤波，不能冒充 `r0031`；
`p0105` 仅能读取，不开放任何写入功能。

`r0035` 声明设备无效值 -200（容差 0.001）；即使协议返回 FF 也按 BAD 处理，
`quality_code` 不伪造设备错误。**该规则只在你显式配置
`invalid_value`/`invalid_tolerance` 时生效**——手填同地址而未配置哨兵时，
驱动不做工程语义判定，也不谎称具有同等保护。

## 身份浏览（内建只读探针）

浏览根的“轴与驱动对象”分为机床轴与候选驱动对象两个列表，**根目录不访问
控制器**。探针数值内建在 `src/identity_spec.rs`（不查模板、不依赖本文件）。
列表按下一实例编号分页，每页只读请求范围，每类最多 31 个实例；
展开轴仅读该轴的名称、驱动号、输出类型、主轴号和 NC 对象名（最多 5 项），
展开候选对象只读该编号的 `p0199`（1 项）。总请求时限 15 秒，无跨请求缓存。
两种入口独立展示，不扫描另一入口来推断关联。

成功的空名称表示表尾；S7 作业拒绝（含 `0x8201`）、单项拒绝和本地解码失败
分别保留，不当作普通空对象。枚举拒绝时保留已读内容并显式展示错误及未枚举
边界。过滤在本页读取后执行，无匹配节点仍返回后续游标，**诊断节点不被过滤隐藏**。

`SPIND_ASSIGN_TO_MACHAX` 读取轴的主轴编号，避免把机床轴号直接当成主轴索引。

未取得真实证据的系列（如 `840d-sl`）**不套用** 828D 探针，而是明确提示
“身份浏览待验证”；使用者仍可手动填写原生地址进行结构化测试。

## 检查

- `EXAMPLES.md`：**可直接复制**的工程配置示例（普通 NC 变量 / R 参数 / 轴变量 /
  SINAMICS 只读参数 / `p0199` 对象名，两条驱动入口都有）；其中的请求字节与
  地址标识由 `tests/examples_828d.rs` 逐条复核，数据源 `examples-828d.json`，
  文档不会与代码漂移。
- `PROVENANCE.md`：42 条候选定义的保留/弃用与来源、7 项未纳入项。
- `src/presets.rs` 单测：模板合法性、拆分入口、非法/空模板降级；
- `tests/native_address.rs`：原生地址严格解析与 golden 字节；
- `tests/legacy_equivalence.rs`：与删除前冻结的目录映射逐条对拍；
- 完整准入执行 `scripts/write-contract-evidence.py`；真实 828D Gate 见 `../GATE.md`。
