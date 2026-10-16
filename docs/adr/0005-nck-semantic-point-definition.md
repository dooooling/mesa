# ADR 0005：SINUMERIK NCK 语义点位与 Address Compiler

- 状态：accepted（PR-B 实施；`sinumerik-nck` 支持等级仍为 **experimental**，真机门不翻转）
- 日期：2026-10-15
- 分支：`feat/nck-prb-compiler`
- 关联：ADR 0001（协议划分）→ ADR 0002（NCK 交叉验证）→ ADR 0003（legacy binding 不作永久兼容承诺）→ **ADR 0004（原生寻址，Catalog 退出采集准入）** → 本 ADR
- 上游依据：`docs/design/nck-semantic-point-refactor.md`（上位设计，**逐字不改**）
- 实施边界：`docs/design/nck-prb-driver-compiler-boundary.md`（PR-B 细化 + C1–C10 裁决）
- PR-A 已落地契约：`crates/core-types/src/point.rs`（`PointSelection`/`PointBinding`/`NCK_POINTS_BINDING_KIND`/`point_key` 通用校验 + 类型标签编码器）、`crates/core-api/src/binding_gate.rs`（按 Task Binding Kind 路由 + Endpoint-wide 判重）

## 背景

ADR 0004 把「原生字段」确立为唯一采集地址模型，但因此用户必须**直接面对协议字段**
（`area`/`module`/`column`/`decode`/`transport_size`…），且点位身份退化为
`variable.value`：同一 NC 变量的不同轴/通道/驱动实例无法区分，历史数据会被劈成两段。

上位设计与 PR #110 评审（C1–C10）因此确立「语义点位」：用户在**语义层**声明
「读哪个变量的哪个实例」，协议字段由驱动按**定义目录**推导；身份由驱动重算并在
Configure 时与持久化 `point_key` **逐字比对**（唯一执行真值）。

## 决策

1. **新增定义目录**：`drivers/sinumerik-nck/definitions/828d/r1.json`（**按 revision 分文件**、
   与 `src/` 平级）+ `DIGESTS.lock`（canonical JSON 的 SHA-256 清单）。
   42 条定义 = A 16（通道/方式组/轴/主轴/R 参数）+ B 20（驱动参数）+ C 6（身份探针），
   经 `golden_ref{area,block,variable,entry}` 与 62 条冻结 wire 向量建立**双射**。
2. **Address Compiler 是唯一转换入口**（`src/compiler.rs`）：`nck_variable` → 定义目录 →
   按 `addressing` 组装 12 原生字段 → **复用** `NativeNckAddress::from_parameters` +
   `resolve_native`；`nck_native` → 直接复用同一条解析链。**零新增字节组装**，
   `semantic` 失败**绝不**退化到 `native`。
3. **Target 的合法字段组合由定义声明**（C1）：`{channel,axis}` / `{channel,spindle}` 是
   **合法复合 Target**；缺失 ⇒ `MISSING_TARGET`、多余 ⇒ `UNSUPPORTED_TARGET`、
   实例越界或 `drive_object.entry` 缺失 ⇒ `INVALID_TARGET`、结构/类型错 ⇒
   `INVALID_BINDING_CONFIG`；**禁止任何默认值猜测**。
   `{channel, axis}` 只表达「NCK 读取的寻址坐标组合」，**不表示**已证明该机床轴与某个
   SINAMICS 驱动对象的物理对应关系（该归属在只读途径下仍 HOLD）。
4. **入口身份不可折叠**（26/130）：`DriveEntry` 必填、无默认值；`DriveEntry::module()` 是
   26/130 的唯一出处，并与 `identity_spec.rs` 的探针常量做**构建期自检**。
   目录中同一驱动参数保留两条 `entry` 变体，各自绑定不同的冻结向量。
5. **`identity_mode` 只由 Compiler 处理**（C3 的 7 步剥离）：完整参数集合（12 原生字段 +
   可选 `identity_mode`）先严格校验，再构建**只含 12 原生字段**的独立 map 交给既有解析器。
   **不得**放宽 `native.rs` 的白名单；剥离**不是**忽略未知字段的通用机制。
6. **身份契约归 Driver**：`identity_template` / `identity_keys` 随定义目录发布并**参与摘要**；
   同一 `variable_id` 的所有 revision 必须给出逐字相同的身份契约（改契约 = 新 `variable_id`）；
   身份值的编码复用 Core 的 `encode_typed_value`（与 Core/前端**同一函数**）。
7. **命名空间隔离**（B 项）：`identity_mode="user"` 的 `point_key` 必须留在 `native.` 命名空间
   （违反 ⇒ `INVALID_USER_IDENTITY`）；Driver 在**该次 configure 可见的点位集合**内提前拒绝
   最终 Key 冲突（`IDENTITY_NAMESPACE_COLLISION`）——**这不是** Endpoint-wide 唯一性保证，
   跨 Task / 跨 kind 的唯一性仍以 Core 的 `DUPLICATE_POINT_KEY` 为唯一权威。
8. **Descriptor 的 `variable` 资源面保留**（C10=D-A）：它继续作为**原生字段声明面**
   （`nck_native` 的可读来源），而**配置面**以 `nck.points.v1` 为唯一真值。

## 破坏性变更（逐条留痕，对应上位设计 §5 的 9 项）

| # | 破坏内容 | 影响面与处置 |
|---|---|---|
| 1 | 旧 NCK 点位配置格式（`mesa.resources.v1` + `variable` 资源 + 12 原生字段） | 一律 `UNSUPPORTED_BINDING`；现存 NCK Endpoint 的点位配置需重建（沿用 ADR 0004:74-75 的先例：明确拒绝、不静默迁移） |
| 2 | 旧 `point_key`（`variable.value` / `variable.value.2`…） | 新 key 为语义形态 ⇒ 旧 key 全部转墓碑；重建得新 `point_id`（迁移不在本次范围） |
| 3 | 旧预填模板的「语义」角色 | **模板数据逐字不动**（C2=P1）：`PresetTemplate.parameters` 仍是原生字段；语义化只发生在**浏览叶载荷**（`binding_json` 改为语义信封 + `identity` 块） |
| 4 | 旧 Descriptor 资源面（`variable` 的 12 字段 schema） | **保留**（C10=D-A）：`resource_selection_methods` 不收缩、`outputs[0].type_spec` 不变 ⇒ `descriptor_contract` 整文件逐字不动；Descriptor Major 不升级 |
| 5 | browse 语义（叶载荷） | 叶从原生信封改为语义信封；既有根/诊断/空页断言**不改写**，只新增一条叶形态断言 |
| 6 | Task Binding Kind 面 | Core 白名单从单 kind 变为 `{mesa.resources.v1, nck.points.v1}`（PR-A 已落地）；Core **不承载**任何 Siemens 类型、不建 Point Kind 白名单 |
| 7 | 错误码面 | NCK 侧**不再产生** `UNSUPPORTED_RESOURCE`/`UNSUPPORTED_OUTPUT`（常量保留）；唯一权威码表 = 21 个（复用 8 + 新增 13） |
| 8 | `nck_native` 的语义保护缺失 | 用户显式声明原生地址时没有 `r0035=-200` 之类哨兵保护（除非自己填）；沿用 ADR 0004:72-73 「驱动不隐式继承、不按变量名猜哨兵」 |
| 9 | `r0031` 访问能力 HOLD | 定义保留（golden 要求）但 `semantics_status=candidate` + 访问能力 HOLD，UI/文档**不得**宣称可用 |

## 明确不做（防止范围蔓延）

- 不改 Core（`crates/**`）、前端（`apps/mesa-web/**`）、其它驱动、`proto/**`、`crates/driver-protocol/**`；
- 不改冻结面：`client.rs`/`codec.rs`/`value.rs`/`address.rs`/`identity_spec.rs`/`identity.rs`/`native.rs`、
  `presets/828d.json`、`legacy_equivalence.rs`、`presets_828d.rs`、`legacy-828d-wire.json`、
  `nck-point-key-golden.json`、`descriptor_contract.rs`、`data_semantics.rs`、`resource_contract.rs`；
- 不新增 NCK area 字母、不引入 `cosmetic_keys`、不做 840D sl 定义、不做历史点位/数据迁移；
- 不启动 `mesad`/Vite、不连接任何真实设备（真机门仍归独立 Gate）。

## 后果

- **正面**：同一 NC 变量的不同通道/轴/主轴/驱动入口获得**互不相同的稳定身份**；
  协议字段不再是用户面负担；62 条既有地址的物理含义由**两条独立路径**（旧模板 / 新定义目录）
  各自证明并与同一份冻结向量比较；定义变更受「已发布不可原地修改 + 摘要清单基线校验」保护。
- **代价**：旧 NCK 点位配置需重建；Descriptor 仍公告 `variable` 资源（在 PR-C 完成 UI 收敛前，
  UI 可能展示一条**不可用**的配置路径）——这是「保留消费者兼容」与「立即移除过期入口」之间的
  **已知取舍**，在 PR-B 描述中显式声明。
- **残留 HOLD**：驱动对象 `V/n` 的轴归属（只读途径未证明）、`r0031` 访问能力、
  19 条驱动参数的手册页码证据、840D sl 定义目录。以上均**不阻塞**本 ADR 的决策落地。
