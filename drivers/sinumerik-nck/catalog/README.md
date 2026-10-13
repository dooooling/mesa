# 828D公共候选变量映射

`828d.json` 记录42种公共候选变量定义。它按既有 `family=828d` 加载，
不依赖设备名称、IP、虚拟机专属分类或外部文件配置。Core/UI无协议分支。
所有条目均为 **candidate**，不是已通过真实828D硬件验证的supported目录。
`supported_families` 仅声明系列装载归属；不能据此认定840D sl也适用。

依据：Siemens 828D *NC variables*, 03/2025, A5E53370586B AC；
Siemens 828D参数手册02/2012；LX驱动读取实现审计；NCK报文与响应长度核对。
变量字段、来源页面、类型和已知限制随条目存储在 `evidence` 中。
模块值来自已审计的LX枚举；area/unit位布局来自仓库锁定的Wireshark来源。
虚拟控制器响应仅证明相应环境的协议行为，不能代替828D硬件证据。

| Area/Block | Variable | 类型 | 索引语义 |
|---|---|---|---|
| C/S | progStatus、actTNumber | U16小端，提升U32 | 通道实例；line=1 |
| B/S | opMode | U16小端，提升U32 | BAG实例；line=1 |
| C/S | actFeedRateIpo、cmdFeedRateIpo、feedRateIpoOvr | F64小端 | 通道实例；line=1 |
| C/RP | rpa | F64小端 | line=R参数号+1 |
| C/SGA | actToolBasePos | F64小端 | line=轴索引；MCS轴设定位置 |
| C/SEGA | actProgPosBKS | F64小端 | line=轴索引；BCS实际位置 |
| C/SSP | actSpeed、cmdSpeed、speedOvr | F64小端 | line=主轴索引 |
| C/SPARPP | progName | 32字节NUL文本 | line=程序层级；主程序line=1、count=1 |
| C/SPARP | progName | 32字节NUL文本 | 当前活动程序/子程序；line=1、count=1 |
| C/SPARP | actLineNumber | I32小端 | 保留官方-1/-2哨兵值 |
| C/SPARP | actBlock | 66字节NUL文本 | 当前执行程序块；line=1 |
| V/M | r0021、r0027、r0035 | F32大端 | area_no=驱动访问编号，不能从轴名称猜测 |
| V/M | r0033、r0031 | F32大端候选 | r0033有LX固定读取依据；r0031访问仍待确认 |

## 使用限制

- 保留原导入点位名称不代表其物理含义已验证。尤其“机械坐标”对应
  `actToolBasePos` 是轴设定位置，“绝对坐标”对应 `actProgPosBKS` 是BCS实际位置，
  不应自动当成编码器MCS实际位置或WCS值。
- 单位依赖控制器设置的变量不写死mm/min、rpm或mm；需单独读单位或查机床设置。
- `r0031`、`r0033` 未列在该版NC变量表中。参数手册确认的是参数本身含义，
  不保证NC网络入口开放任意SINAMICS参数；r0031没有成功读取证据。
- 32/66字节文本长度和transport-size属于候选协议定义；不一致时按项BAD。
- 有定义但控制器拒绝返回有效值时仍报BAD。完整作业拒绝使本次读取失败，
  不恢复虚拟机专属的容错分支，不捏造GOOD或零值。
- 设备逻辑状态、系统信息、程序注释及报警汇总不是单个NC变量。
  未建立相应公共资源实现前不填假地址，未知变量在Configure阶段拒绝。

## 检查

`tests/828d_catalog.rs` 检查公共定义、请求字节、响应编码及Configure契约；
完整准入仍执行 `scripts/write-contract-evidence.py`。真实828D Gate见 `../GATE.md`。

## 访问入口与对象核对

`access_path=nc_variable`是缺省值，保留已有模块26地址和点位身份。
`access_path=drive_object`仅选择目录显式声明的模块130候选入口。
模块130的正式名称和真实硬件适用性尚未确认，不能从IP、型号或轴名自动启用。
这两个入口的实例编号可能不同，须读取`p0199`对象名关联，禁止直接沿用编号。

公共目录包含`r0062/r0063/r0064/r0077/r0078/r0079/r0080/r0081/r0082`、
`r0103/p0105/r0106/r0107/r0108/p0199`，以及轴名、MD30110、MD30130、
MD30220和通道轴映射。`p0199`以25个大端U16投影为一个UTF-16对象名；
`r0078[0/1]`的网络Line分别为1/2。r0078为转矩产生电流，不能冒充r0027；
r0080未滤波，不能冒充r0031。p0105仅能读取，新增目录不开放任何写入功能。

浏览根目录的“轴与驱动对象”在用户展开时执行有界只读核对，连接内快照缓存30秒，
总请求时限15秒、每类实例最多31个。同名对象须唯一才关联；未启用驱动输出的
轴及未关联对象分别展示，不根据同编号猜测配对。选择变量时自动携带该入口编号。
原始软件控制器证据见`docs/evidence/nck-drive-object-readback.json`。
管理API使用临时浏览连接，每次展开会重新核对；保存的采集地址不会自动随对象
分配变化。更换项目或修改轴分配后必须重新核对。枚举遇到设备作业拒绝时保留
已读对象并展示原始错误及未枚举边界，不将部分目录宣传为完整设备目录。

`SPIND_ASSIGN_TO_MACHAX`读取轴的主轴编号，避免把机床轴号直接当成主轴索引。
固定列条目使用`wire.fixed_column=true`，拒绝不一致Column及未声明的参数字段。
`r0035`声明设备无效值-200（容差0.001）；即使协议返回FF也按BAD处理，
quality_code不伪造设备错误。原始值保留于只读证据；传感器/PTC/参数设置需另行核对。
