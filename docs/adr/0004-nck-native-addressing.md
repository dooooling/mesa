# ADR 0004：SINUMERIK NCK 原生寻址（Catalog 退出采集准入）

- 状态：accepted（PR-A/PR-B/PR-C 实施；`sinumerik-nck` 保持 experimental）
- 日期：2026-10-13
- 分支：`feat/nck-native-addressing-a` → `-b` → `-c`
- 关联：ADR 0001（协议划分）→ ADR 0002（NCK 交叉验证）→ ADR 0003（legacy binding 不作永久兼容承诺）
- 事实基线：`main 78e23ec`；冻结标签 `nck-v0.5.0-freeze-20261013` → `80a8c81`

## 背景

NCK 驱动的采集地址原先必须经过**随仓变量目录**（`catalog/*.json`）才能成立：
`configure()` 强制 `CatalogRegistry::load_shipped() → catalog.lookup(area, block, variable) → codec::resolve()`，
变量名不在目录里就无法配置，类型、module、column、transport 全部由目录条目给出。

这带来三个问题：

1. **协议能力被目录长度封顶**：NCK 可读的地址空间远大于随仓的 42 条候选定义，
   任何未收录的变量都必须改代码发版才能采集。
2. **目录同时充当了准入白名单与类型真值**：用户无法为“目录里没有但设备确实可读”
   的地址配置采集，也无法绕开目录给出的类型。
3. **身份核对与采集准入耦合**：身份浏览（轴名、驱动号、`p0199` 对象名）的实现
   也走目录查询，删除目录会连带删掉身份能力——形成“目录无法删除”的隐性锁定。

同时，前一轮对账暴露了目录的**语义可信度上限**：42 条候选中 20 个导入点位仍
无法建立可信映射（7 项不是单个 NC 变量、12 项轴归属无法只读证明、1 项无效温度），
说明“目录里有的名字”并不等于“已确认的工程语义”。

## 决策

**Catalog 从「采集入口许可证」退为「可选的地址模板知识」。**

1. **唯一采集地址模型**：`NativeNckAddress`（`area`/`area_no`/`module`/`column`/
   `line`/`count`/`unit_mode`/`decode`/`transport_size`，加可选
   `invalid_value`/`invalid_tolerance`/`unit`）。字段即协议字段，`decode` 用规范
   词表（显式大小端与宽度），`transport_size` 用响应枚举。Configure 只有这一条路径。
2. **删除运行时目录链路**：`catalog.rs`、`browse.rs`、`topology.rs`、
   `catalog/*.json`、`codec::resolve`、`NckVariableRef`、`NckAccessPath`、`NckShape`、
   `CatalogRegistry` 全部删除；`family` 降为设备档案（模板/探针范围），**不是读取许可**。
3. **身份能力内建**：身份浏览改用驱动内建常量 `IdentityProbeSpec`，不查目录、
   不依赖模板文件；未取得真实证据的系列明确提示“待验证”，不套用 828D 探针。
4. **配置知识转为可选模板**：原 42 条候选转成 `presets/828d.json`（62 个模板，
   含驱动对象双入口拆分）。模板只预填结构固定字段；`area_no` 与 `line`
   **留给使用者确认**——按变量名推断实例/行索引会违反“不猜”原则。
   **模板缺失、为空或非法都不影响 Configure/Poll**，错误只出现在浏览诊断里。
5. **破坏性切换，不迁移**：旧 `block`/`variable`/`access_path` 配置被 Core Schema
   与 Driver 双重拒绝，不提供自动转换，不允许“保存正常但运行读了另一物理地址”。
6. **无效工程值改为显式配置**：规则随绑定存储，覆盖数值标量/数组/整型；
   未配置时驱动不做工程语义判定，也不谎称具有同等保护。

## 理由

- **协议能力不再被目录长度封顶**，同时保住“不伪造”的底线：地址合法 ≠ 协议读取
  成功 ≠ 工程语义已确认，三者仍严格分离。
- **删除目录不等于丢失知识**：候选定义的数值、来源与限制迁到
  `presets/README.md` 与模板的 `description`；语义索引表按人工确认用途保留。
- **可对账**：删除前把每条候选的期望线缆字节冻结为测试向量
  （`tests/fixtures/legacy-828d-wire.json`，来源 `main@78e23ec`），
  逐条断言新路径复现同样字节——用旧数据做真值，而不是用新代码自证新代码。
- **身份能力与配置准入解耦后**，目录才真正可删；否则“删除 Catalog”只是把锁定
  从一个模块挪到另一个模块。

## 后果

正面：

- 任意结构合法、解码受支持、设备实际接受的地址都可配置与只读采集（与型号无关）。
- 目录不再是“能否采集”的开关，也不能再被误当硬件验证结论。
- 身份浏览在没有模板文件时依然可用；模板损坏不会波及数据面。

代价与需承担的风险：

- **失去目录自动给出的工程语义与无效值保护**：手工地址默认没有 `r0035=-200`
  这类保护，必须显式配置；文档与 UI 必须如实说明。
- **旧 NCK 点位必须重建**：本机 240 点端点在升级后即被明确拒绝并停用
  （已备份 `target/nck-legacy-backup/`），重建需人工确认实例与行索引。
- **地址写错会读到别的物理量**：这是能力带来的自由度，靠
  “显式字段 + 独立 golden + 模板来源说明 + 不与 GOOD 混淆”控制，而不是靠白名单。

## 被否决的替代方案

- **保留目录作为兜底**：`catalog.lookup()` 可选、未命中时回退到名称推导——
  等于保留隐式的“变量名 → 地址”承诺，且删除目录后行为仍然分叉。
- **双资源（`variable` + `nck_address`）**：两套 configure/resolve/read 实现，
  身份与准入仍耦合，违背“单管道”目标。
- **自动迁移旧 JSON**：`block/variable` 到原生字段不是一一映射（同一 block/variable
  可能对应两条入口与不同 column），自动转换会把不确定写成默认值。
- **按变量名推断 `area_no`/`line`**：对账已证明轴归属无法只读建立，
  推断即等于伪造地址语义。

## 验证

- 等价门：`tests/legacy_equivalence.rs` 逐条复现 62 条冻结地址字节。
- 严格性：`tests/native_address.rs` + `src/native.rs` 单测（未知/旧字段、别名、
  越界、非整数、规则配对、checked 长度）。
- 身份：`src/identity_spec.rs` 捕获字节 golden（`p0199` 两条入口）+
  `tests/identity_browse.rs` 分页/拒绝/表尾/过滤覆盖不减。
- 真机只读：`docs/evidence/nck-live-native-probe.txt`（实体 828D，内建探针取值
  与旧目录路径一致）。
- 契约：`scripts/write-contract-evidence.py` 28/28 套件、186/186 测试。
