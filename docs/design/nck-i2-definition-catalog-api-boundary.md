# I-2 实施边界与验收清单：SINUMERIK 828D 变量定义只读权威接口（定义目录 + 权威身份预览）

- 状态：**实施边界设计稿（第 2 轮：评审裁决定向修订稿；待评审复核差异后开工）**
- 定位：为 **PR-C 的「完整语义点位配置」**（PR-C 文档 §1.4.3 的 **L2**）提供必要的
  **服务端权威元数据**（定义目录）与**身份确认能力**（权威 Preview）。
- 结论口径：本文件是**实施边界与验收清单**，不是实现。文中事实均以**只读命令**在
  **`docs/nck-i2-boundary` / HEAD `a810c42`** 的工作树上核实，
  引用一律给 `文件:行号`；无法确证的一律标注**待确认**，**不编造**。
- **轮次与基线（可追溯）**：
  - **第 1 轮（新建本文件）**：基线 `docs/nck-i2-boundary` / HEAD `d99a34a`
    （PR-C 设计文档合并后的最新 main）；该轮的全部结论与 §12.1 命令清单**保留**。
  - **第 2 轮（本轮，评审裁决定向修订）**：基线 `docs/nck-i2-boundary` / HEAD `a810c42`。
    本轮**只修订本文件**（`docs/design/nck-i2-definition-catalog-api-boundary.md`），
    **未改任何代码 / 测试 / 配置 / 其它文档**，**`proto/driver.proto` 一个字节未改**，
    **未运行任何测试 / 构建 / 安装**，**未启动 `mesad` / Vite**，**未连接任何设备或机床**，
    **未执行任何 git 写操作**（只读 git 例外，见 §12.2）。
  - 本轮修订的**四项约束**与**逐条裁决**落点见 **§14**（含全部【已裁决】/【已裁决修正】追溯）；
    评审可**只复核 §14 的差异表**。
- **协议增补的当前状态（关键，避免误读）**：§7.2 的 wire additive 增补（tag 45–48 + `PROTOCOL_MINOR 6→7`）
  **已获原则批准**，但**生产代码仍 HOLD**：本文件本轮**不修改 `proto/driver.proto`**，
  也未修改任何 `crates/**`、`drivers/**`。**批准 ≠ 已实现**。
- 上位/同级文档（**本次一个字节都不改**，只引用）：
  `docs/design/nck-semantic-point-refactor.md`（1387 行，设计冻结稿，本次未通读全文）、
  `docs/design/nck-prb-driver-compiler-boundary.md`（1167 行，PR-B 已裁决）、
  `docs/design/nck-prc-frontend-semantic-config-boundary.md`（1362 行，PR-C 已合并且已裁决）。
- 术语沿用 PR-B：**Task Binding Kind**（任务层路由 `mesa.resources.v1` / `nck.points.v1`）
  与 **Point Kind**（点位层 `nck_variable` / `nck_native`）是两个不同层次，**不得**混用。

---

## 0. 本次授权范围（硬约束，先声明）

| 项 | 第 1 轮（新建本文件） | 第 2 轮（本轮修订） |
|---|---|---|
| 允许 | **只新建本文件** `docs/design/nck-i2-definition-catalog-api-boundary.md` | **只修改本文件** `docs/design/nck-i2-definition-catalog-api-boundary.md` |
| 禁止 | 修改任何代码 / 测试 / 配置 / 其它文档（**尤其**三份冻结设计文档） | 同左；**另加**：`proto/driver.proto` **不得修改**（增补已获原则批准，但生产代码仍 HOLD） |
| 禁止 | 任何 git 写操作（commit/push/checkout/stash/reset/PR/tag）；**只读 git 可用** | 同左 |
| 禁止 | 启动 `mesad`、启动 Vite、连接任何真实设备或机床 | 同左 |
| 禁止 | 运行任何测试 / 构建 / 安装 | 同左 |
| 提交推送 | 由本人执行 | **由上级执行**（本轮**不做任何 git 写操作**） |

**因此**：本文件全部内容都是**设计登记**，**不含任何已实施的改动**；
本文件自身的核实动作**只用了只读命令**（第 1 轮清单见 §12.1，第 2 轮清单见 §12.2）。
**本轮「修订」不是「重写」**：第 1 轮的全部已认可内容（特别要求 A 三段式论证、特别要求 B 离线可算判定、
六项问题、错误码表、验收矩阵 I1–I31、CI 门禁 G0–G11）**逐字保留**，本轮只做**定向修正与增补**。

**结论先行（给评审的 5 句话）**：

1. **特别要求 A**：现有管理面**确实已经承载了大部分能力**（浏览叶已带 `identity` 块、descriptor 通道已能
   「无 Endpoint、无连接」地拿驱动侧 JSON），但**缺一个带用户输入参数的只读管理 RPC** ——
   这一点是**结构性的、无法靠扩展既有载荷绕过**（依据：`GetDescriptor` 无任何字段、
   `BrowseRequest` 只有 `parent/filter/cursor/limit`、`ProbeRequest` 需要已打开的连接）。⇒ **必须新增只读路由**（§1.3）。
2. 该新增路由**不是新框架**：它与既有 `GetDescriptor` **逐字同构**（临时进程 + 握手 + 一条 JSON 透传 RPC + 单出口回收），
   改动面 **10 个文件、其中 3 个新增**（§8.1）。
3. **特别要求 B**：**权威 Preview 完全离线可算，不需要 Endpoint、不需要连接、不需要真机**；
   它只需要一个**被 Manager 临时 spawn 的 Driver 进程**，而该进程**不需要任何 Connection 条目**
   （既有 `GetDescriptor` 就是这条路径的现成先例）。证据链见 §2。
4. **wire additive 增补**（第 1 轮登记为「唯一需要评审裁决的点」）：新增 RPC 需要对 `proto/driver.proto` 做**additive 增补**
   （新 tag 45–48）+ `PROTOCOL_MINOR 6→7`。**既有消息、既有字段编号与语义、`PROTOCOL_MAJOR=1` 全部逐字不动**；
   按 PR-B §7.2（`:936`）与 PR-C §1.4 I-2（`:263`、`:1212`）的既有条款，这类增补**必须以独立「契约变更」PR 形态提交**
   并单列影响面。【已裁决】**已获原则批准**（仍以独立「契约变更」PR 形态提交）；**生产代码仍 HOLD**（本轮不改 `proto/**`）。
   降级路径（零 wire 的 I-2a + PR-C 既有的 A39 降级分支）见 §1.3-③，**保留备查**。
   **本轮同时修正**：第 1 轮把 `oneof body` 的既有 tag 数写成「44 个」属**计数错误** ——
   实测 **35 个**（编号 10–44，连续无空洞），逐字段核对见 **§7.2.1**。
5. **PR-C / PR-D 边界、`scope=identity` 缺陷归属、Core 零语义知识、62 条 wire golden、Descriptor 2/0
   均不变**（§7、§9、§10）。

---

## 1. 【特别要求 A】是否必须新增 HTTP API —— 三段式论证

> 评审要求：**先检查现有 Driver 管理面、Browse 与 Core API 是否能够通过最小扩展满足需求；
> 只有确认现有路径不足时，才设计新路由。**

### 1.1 （i）现有可用能力盘点（逐条给证据）

#### 1.1.1 今天**已经能拿到**的东西

| # | 能力 | 今天的状态 | 证据（`文件:行号`） |
|---|---|---|---|
| P1 | **Driver 侧已持有完整定义目录数据** | `VariableDefinition` 22 个字段（含 `scope`/`semantics_status`/`source[]`/`targets`/`target_required`/`arguments[]`/`identity_template`/`identity_keys[]`/`golden_ref[]`） | `drivers/sinumerik-nck/src/definition.rs:200-248` |
| P2 | **目录随二进制发布，装载即全量自检** | `include_str!` 内嵌 `r1.json` + `DIGESTS.lock`；装载期校验 ID 形状 / 身份覆盖闭包 / 摘要一致，任一不成立即整体不可用（fail-closed） | `definition.rs:47`、`:49`、`:376-378`、`:381-433`、`:546-590` |
| P3 | **浏览叶已携带 `identity` 块** | `{"template", "keys", "preview_point_key"}` 旁挂在语义信封上；`preview_point_key` 在 target/arguments 不完整时**显式为 `null`** | `drivers/sinumerik-nck/src/presets.rs:242-275`（`identity` 构造 `:262-266`；`null` 分支 `:249-253`） |
| P4 | **该 `binding_json` 逐字到浏览器** | Core 只做字符串透传，无裁剪、无白名单 | `crates/core-api/src/lib.rs:864`；路由注册 `:3271` |
| P5 | **descriptor 通道：driver 作用域、无 Endpoint、无连接** | `GET /api/v1/drivers/{id}/descriptor`；Manager 走「临时进程 + 握手 + `GetDescriptor` + 缓存 + 回收」，**从不发 `OpenConnection`** | 路由 `crates/core-api/src/lib.rs:3241-3244`；handler `:638-664`；`crates/driver-manager/src/manager.rs:308-370`（临时进程 `:328`、`get_descriptor` RPC `:330`、缓存 `:322-325`/`:351-357`） |
| P6 | **Driver 进程可在「零 Connection 条目」下响应管理面 RPC** | SDK 的 `GetDescriptor` 分发**不查 `session.entries`**，直接由 `session.driver.descriptor()` 应答 | `crates/driver-sdk/src/lib.rs:1126-1144` |
| P7 | **临时进程原语已通用化** | `temp_driver_attempt`：spawn → Hello/Welcome → 调用方 RPC → invalidate → terminate，单出口回收，不留孤儿 | `crates/driver-manager/src/temp_op.rs:138-166`；`cleanup` `:40-43`；spawn+握手 `:96-125` |
| P8 | **Browse 通道：endpoint 作用域、分页、`binding_json` 透传** | `POST /api/v1/endpoints/{id}/browse`；Manager **先开连接**（handle=1）再 browse | 路由 `core-api/src/lib.rs:3271`；handler `:813-885`（Endpoints 查询 `:818-830`、Secret 物化 `:837-840`、limit 缺省 50/上限 1000 `:834`）；`manager.rs:383-457`（`OpenConnection` `:402-410`、`session.browse` `:440-443`） |
| P9 | **Browse 树的语义叶由同一产出构造** | `nck://presets/...` 与 `nck://identity/...` 的叶都经 `leaf_node_with` | `presets.rs:179-214`；`drivers/sinumerik-nck/src/identity.rs:10`、`:247` |
| P10 | **目录 ↔ 模板 ↔ 冻结向量三方覆盖完整** | 62 个 `golden_ref` ↔ 62 个模板 id **一对一**；**本次独立复核命中 62/62、miss 0** | 复核见 §1.1.3；模板 id 形态 → 四元组的索引实现 `presets.rs:228-239` |
| P11 | **既有错误通道已能携带结构化 Driver 错误** | `GenericResult` + `error_detail(kind, code, message)`；`ErrorDetailBox::is_configuration_error()` | `crates/driver-protocol/src/lib.rs:618`、`:625`、`:660-670` |
| P12 | **Core 侧已有「driver 作用域只读 POST」先例** | `POST /api/v1/drivers/{id}/probe`、`POST /api/v1/drivers/{id}/validate-connection` 都是带 body 的只读查询 | 路由 `core-api/src/lib.rs:3246-3249`；`probe_driver` `:782-803` |
| P13 | **Core 侧已有「只读操作不得创建 Endpoint」的现成契约断言** | `probe_does_not_create_endpoint` | `tests/driver-contract/tests/management_api.rs:676-699` |
| P14 | **Core 零 Siemens 标识是**可执行护栏**（不是口号） | 扫描 `crates/core-types/src` 与 `crates/core-api/src` 的**生产代码**，禁令词表含 `VariableDefinition`/`DefinitionCatalog`/`canonical_identity`/`identity_template`/`identity_mode`/`UNKNOWN_VARIABLE`/`UNSUPPORTED_DEFINITION_REVISION`/`MISSING_TARGET`/`INVALID_TARGET`/`INVALID_ARGUMENTS`/`nck_variable`/`nck_native`/`drive_object` … | `tests/driver-contract/tests/resource_contract.rs:396-461`（词表 `:398-428`；扫描目录 `:432-434`；只扫首个 `#[cfg(test)]` 之前 `:445-446`） |

**⇒ P14 是本设计最重要的「可行域」约束**：任何把目录、身份、`scope`、NCK 错误码**放进 Core** 的方案
都会被该护栏**直接判红**。I-2 的落点因此被唯一确定：**Driver 出语义、Manager 出编排、Core 只做驱动无关透传**。

#### 1.1.2 今天**缺**的东西（正是 §1.4.4 状态表要求、而 Browse 拿不到的那一批）

| # | 缺口 | 是否在 `identity` 块内 | 是否在 `parameters` 内 | 依据 |
|---|---|---|---|---|
| G1 | **`scope`（适用范围）** | ✗ | ✗ | `presets.rs:262-266` 只给 `template`/`keys`/`preview_point_key` |
| G2 | **`targets` / `target_required`（合法组合）** | ✗（`identity_keys` 只声明「身份需要哪些键」，**不**声明合法集合） | ✗ | 同上；`definition.rs:214-217` vs `:170-176` |
| G3 | **`arguments[]` 的 `value_type`/`required`/`min`/`max`/`enum_options`/`default`** | ✗ | ✗（`parameters.arguments` 只是**当前实例的值**） | `definition.rs:107-125` |
| G4 | **`semantics_status`** | ✗ | ✗ | `definition.rs:59-69` |
| G5 | **`source[]`（来源证据）** | ✗ | ✗ | `definition.rs:89-97` |
| G6 | **`label{zh,en}`（定义展示名）** | ✗ | ✗ | `definition.rs:72-77` |
| G7 | **`golden_ref[]`（含 26/130 入口去重集合）** | ✗ | ✗ | `definition.rs:178-198` |
| G8 | **按 `(variable_id, revision)` 权威可查的「目录」语义** | ✗ | ✗ | 目录查询是 `DefinitionCatalog::lookup`（`definition.rs:489-505`），**未对外** |
| G9 | **带用户输入参数的权威身份计算入口** | ✗ | ✗ | 见 §1.2 的决定性证据 |
| G10 | **修订标识（`revision` 之外的摘要锚点）** | ✗ | 仅有 `definition_revision` | `definition.rs:206`；摘要 `:245-247`、`:593-609` |

**注意 G9 与 G1–G8 的性质不同**：G1–G8 是**数据缺失**（可以靠「把已有数据多带出来一点」解决），
G9 是**通道缺失**（现有 RPC 家族里**没有任何一条能承载调用方参数**）。这是 §1.3 结论的分水岭。

#### 1.1.3 本次独立复核的数字（不引用文档数字）

| 项 | 实测值 | 复核方式 |
|---|---|---|
| `definitions/828d/r1.json` | **1331 行**；`format_version=1`、`family=828d`、`revision=1`、`definitions=42` | `(Get-Content …).Count` + `ConvertFrom-Json` |
| `golden_ref` 总数 | **62**（20 条定义 × 2 + 22 条定义 × 1） | 同上 |
| `scope` 分布 | `collect=36` / `identity=6` | 同上 |
| `semantics_status` 分布 | `verified=24` / `parameter_identity=16` / `candidate=2` | 同上 |
| `target_required` 组合分布 | `{drive_object}`=20、`{channel}`=11、`{axis}`=5、`{channel,spindle}`=3、`{channel,axis}`=2、`{mode_group}`=1 | 同上 |
| `arguments` 非空的定义 | **恰 2 条**：`channel.r_parameter`（`index` 0..65534，`required=true`）、`channel.machine_axis_assignment`（`position` 0..65535，`required=true`） | 同上 |
| `targets` 含 `drive_object` ↔ `target_required` 含 `drive_object` | 均 **20**，**集合完全相同** | 同上 |
| `unit` / `invalid_rule` 出现次数 | `unit` 0 次；`invalid_rule` 1 次 | 同上 |
| `addressing.area` 分布 | `A=4`/`B=1`/`C=16`/`N=1`/`V=20` | 同上 |
| `addressing.module` 来源 | `drive_entry_module`=20 / `fixed`=22 | 同上 |
| `definitions/828d/DIGESTS.lock` | **258 行**；`format_version=1`、`family=828d`、`entries=42` | 同上 |
| `presets/828d.json` | `templates=62` | 同上 |
| `tests/fixtures/legacy-828d-wire.json` | **`entries=62`**（62 条 wire golden） | 同上 |
| `tests/fixtures/nck-point-key-golden.json` | `value_cases=55` / `value_reject_cases=5` / `key_cases=77` | 同上 |
| **62 个 `golden_ref` ↔ 62 个模板 id** | **hit=62 / miss=0**（模板 id 由四元组无损重建：`828d.<area>.<block>.<variable>.<entry>`） | 本次逐条交叉核对 |
| **`scope=identity` 的 6 条定义是否出现在模板中** | **6/6 全部出现**（即 PR-C §4.1 的缺陷在本次独立复核下**复现**） | 本次逐条交叉核对（见 §9.1 明细） |
| 合同测试套件数 | **29**（基线 14 + 事件面 12 + 生命周期 3） | `scripts/contract_suites.py:13-52` |

### 1.2 （ii）最小扩展方案候选与比较

> 比较维度（评审给定）：**改动面、复用度、Core 零语义知识、对既有契约的影响、可测试性**。

#### 决定性证据（先摆出，后面所有候选都受它约束）

**现有 Core→Driver 管理面 RPC 中，没有任何一条能承载「调用方传入的参数对象」**：

| RPC | 现有请求字段 | 能否承载 `{variable_id, revision, target, arguments}` | 证据 |
|---|---|---|---|
| `GetDescriptor` | **无字段**（`message GetDescriptor {}`） | **不能** | `proto/driver.proto`（`message GetDescriptor {}`）；handler `driver-sdk/src/lib.rs:1126-1144`；Manager `manager.rs:330` |
| `BrowseRequest` | `connection_handle` / `parent` / `filter` / `cursor` / `limit` | **不能**（且语义已被占用：路径选择、过滤、分页） | `proto/driver.proto`（`BrowseRequest` 字段 1–5）；`driver-sdk/src/lib.rs:1656-1719`；`core-api/src/lib.rs:805-811` |
| `ProbeRequest` | `connection_handle`（**且必须已 OpenConnection**） | **不能**（且会建立设备会话） | `proto/driver.proto`（`ProbeRequest`）；`driver-sdk/src/lib.rs` 的 `on_probe`（`session.entries` 未命中 ⇒ `NO_CONNECTION`）；`crates/driver-manager/src/probe.rs:156-180` |
| `ConfigureTasks` / `ApplyPointMap` / `StartConnection` / `WriteRequest` / `CommandRequest` | 各类 | **能，但被禁用**：它们是**状态变更/写路径**（配置计划、点位映射、启停、写值、命令），与「只读接口」绝对冲突，且需要已打开连接 | `driver-sdk/src/lib.rs:1148-1173`；`proto/driver.proto` 的控制面注释 |

**⇒ 结论（结构性）**：I-2 的「权威 Preview」需要一条**带参数的只读 RPC**，
而这条 RPC **在现有协议里不存在，也无法由既有字段表达**。

#### 候选方案对照

| 维度 | **C1：扩展 Browse 叶载荷** | **C2：新增只读管理面 RPC（推荐）** | **C3：扩展 Descriptor 承载目录** | **C4：复用 Probe / connection-check** | **C5：用 Browse 的 `filter` 承载预览参数** |
|---|---|---|---|---|---|
| **形态** | 每个语义叶的 `binding_json` 多带 `definition{scope,targets,target_required,arguments[],semantics_status,source[],label,golden_ref[]}` | Driver 新增 `GetDefinitions` / `PreviewPointKey` 两条只读管理 RPC；Core 新增 2 条 driver 作用域只读路由 | `DriverDescriptor` 增一个可选字段承载目录 | 把预览塞进 `ProbeResponse.report_json` | `parent="nck://preview"` + `filter` = 请求 JSON |
| **改动面** | **极小**：`presets.rs` 1 处 + 前端；**零 proto / 零 Core / 零 Descriptor** | **中**：10 个文件（3 新增），含 1 处 **additive** wire 增补（tag 45–48 + Minor 6→7） | 小：`core-types/descriptor.rs` + NCK `descriptor()` | 小（复用字段） | 极小 |
| **复用度** | 高（复用 Browse RPC 与管理面链路） | **很高**：与 `GetDescriptor` 路径逐字同构（临时进程 + 握手 + JSON 透传 + 单例缓存式回收） | 高（复用 descriptor 的临时进程链路） | 中（probe 链路**需要 `OpenConnection`**） | 高 |
| **Core 零语义知识** | 保持（`binding_json` 逐字透传，`core-api:864`） | 保持（JSON 逐字透传 + 只按 `ErrorKind` 做 HTTP 映射，**按 G14/P14 护栏零 NCK 标识**） | 保持（若字段类型为不透明 `Value`） | 保持 | 保持 |
| **对既有契约的影响** | 浏览叶载荷 additive；但前端 A16/A44 语义需同步（PR-C 自己的范围） | **既有消息/字段/语义零改动**；新增 tag additive + Minor 6→7；**须以独立契约变更 PR 提交**（PR-B §7.2 `:936` 已授权该形态） | **与 PR-C §1.4 I-2「不改 Descriptor 契约」条款直接冲突**（PR-C `:262`）；且 Descriptor 是**声明面**（PR-B `:980-982`），塞入 58KB 目录会污染每次 descriptor 取用与 256KiB 上限（`session.rs:677`） | **语义污染**：把「设备探测/连通性检查」变成「定义编译」；且 `ProbeRequest` 需已开连接 ⇒ 与「不连真机」冲突 | **语义滥用**：`filter` 已有「过滤」语义；分页/缓存假设不成立；错误出口是浏览失败码（`core-api:873-878` 只映射 `BROWSE_FAILED`/`DRIVER_UNAVAILABLE`），无法表达精确的配置错误 |
| **能否覆盖 G1–G8（目录/元数据）** | **能**（62 个叶 × 每个叶重复一份定义） | **能**（42 行目录，权威、可查、不重复） | 能 | 不能（无目录语义） | 不能 |
| **能否覆盖 G9（权威 Preview）** | **不能**（Browse 无参数通道） | **能**（唯一可行） | **不能**（`GetDescriptor` 无字段） | 能但**需开连接** ⇒ 违反特别要求 B | 能但**属语义滥用** |
| **是否依赖 Endpoint / Secret** | **是**（`core-api:818`/`:837`） | **否**（与 descriptor 同族） | **否** | 是（需连接配置） | 是 |
| **可测试性** | 高（既有 `identity_browse.rs` 可扩展） | 高（Driver 单测 + 既有 `management_api` 套件内新增用例 + 只读断言，见 §8.3） | 中（需改动 Descriptor 契约测试的既有断言 ⇒ 触冻结面） | 低（需设备/连接前置） | 低（错误语义不可断言） |

**一句话比较**：

- **C1 是「最省事」，但它只能解决目录，解决不了 Preview**；
- **C3 把目录塞进声明面**，与 PR-C 已写明的「不改 Descriptor 契约」直接冲突，且仍解决不了 Preview；
- **C4/C5 都是把只读查询伪装成别的语义**，会被 PR-C §1.6 红线与「接口只读」追问击穿；
- **C2 是唯一同时满足「统一形态 / 无 Endpoint / 离线可算 / Core 零语义 / 可断言」的方案**，
  且它是**唯一**能让「用户改了 Target/Arguments 后拿到服务端权威新 Key」的形态（PR-C §1.4.2 的**首选**方案）。

### 1.3 （iii）最终选择

#### ① 判定：**现有路径不足，必须新增一条只读路由**（决定性理由）

> **决定性理由（一句话）**：**权威 Preview 必须接收用户填写的 `target`/`arguments`，
> 而现有 Core→Driver 管理面 RPC 家族中没有任何一条具备参数通道**
> （`GetDescriptor` 无字段、`BrowseRequest` 只有 `parent/filter/cursor/limit`、`ProbeRequest` 需已开连接且语义是设备探测）——
> 这是**通道缺失**，不是数据缺失，**扩展任何既有载荷都无法补上**。

补充的**不可回避性**论证（三条，缺一不可）：

1. **不能由 Core 代算**：`crates/core-types/src/point.rs:16-19` 明写 Core **不提供** `canonical_identity`
   之类的 API、不读 `identity_template`、不按 `variable_id` 拼装；PR-B 文档 `:229` 裁决 Core 侧身份推导
   **「不需要（且禁止）」**；且 P9 护栏（`resource_contract.rs:396-461`）会把
   `canonical_identity` / `identity_template` / `variable_id` 相关标识在 Core 生产代码中出现**直接判红**。
2. **不能由前端代算并持久化**：PR-C §1.6 红线（`nck-prc-...:362-380`）禁止前端发明第二套身份算法；
   `tests/fixtures/nck-point-key-golden.json` 只能证明**编码规则**层一致（PR-C §4.3、A41），
   **不能**证明「该实例的身份语义与 Driver 当前定义完全一致」。
3. **不能借 `ConfigureTasks` 做「dry-run 编译」**：它是**状态变更**路径（写采集计划），
   违反「接口只读」，且需要已打开的连接；会让「只读性」不再可断言。

#### ② 决定的范围形态：**I-2b（主体）+ I-2a（可选先行项）**

| 项 | 内容 | 是否本文件的验收依赖 |
|---|---|---|
| **I-2b（主体，必须）** | 新增两条 driver 作用域只读管理 RPC：`GetDefinitions`（定义目录）与 `PreviewPointKey`（权威身份预览）；Core 侧对应两条只读路由 | **是**（L2 的「服务端权威 Preview」只能由它满足） |
| **I-2a（可选先行项，零 wire 增补）** | 在既有浏览叶的 `binding_json` 上**追加**完整定义元数据（`scope`/`targets`/`target_required`/`arguments[]`/`semantics_status`/`source[]`/`label`/`golden_ref[]`），**复用既有 Browse RPC + 既有 Core 路由 + 既有 `binding_json` 透传** | **否**。它能在 I-2b 获准前**先解除 PR-C 的 A40 fail-closed**（让 `scope` 可得），代价是**同一份 Driver 数据出现两个投影**，因此**必须**有交叉一致性断言（矩阵 **I31**），且**不得**据此声称「已有服务端权威 Preview」 |

> **为什么把 I-2a 单列而不并入主体**：I-2a 与 I-2b 解决的是**两件事**
> （元数据可得性 / 身份权威性）。把两者混为一谈，会让「零 wire 改动」这个优点
> 变成「以为什么都解决了」的错觉 —— 而 `scope` 可得**并不**等于「改了参数后的身份可信」。

#### ③ **唯一需要评审裁决的点：wire 的 additive 增补**

| 问题 | 事实 | 处置 |
|---|---|---|
| 新增 RPC 是否要动 wire？ | **要**。`proto/driver.proto` 的 `oneof body`（起于 `:22`）现有 **35 个 tag**（编号 **10–44** 连续、无空洞；末项 `EventBatchMsg event_batch = 44;` 在 `:76`；逐字段核对见 **§7.2.1**），新增消息对必须占用 **45–48** 四个新 tag | 见 §7.2 的逐条不动声明 + §7.2.1 的逐字段核对 |
| 既有协议是否被修改？ | **否**。既有 **35** 个 oneof tag 的编号、类型、语义**逐字不动**；`PROTOCOL_MAJOR` 保持 **1**（`crates/driver-protocol/src/lib.rs:23`）；`PROTOCOL_MINOR` **6 → 7**（`:29`） | 这是**增补**，不是**修改** |
| 这算不算「已获授权」？ | **PR-C 文档早已预告**：I-2「需要 **Driver SDK 新增能力 + Core 透传 + 可能触及 `proto/**`**」⇒ 属后端变更、**不在 PR-C 授权内**，建议独立 PR（`nck-prc-...:263`、`:1212`）；PR-B §7.2 `:936` 亦规定：确需改动 wire 时**必须作为独立的「契约变更」PR**，并列出影响的其它驱动、需同步改写的契约套件、握手影响面与回滚方案 | **本文件按该条款登记，并把影响面逐条列出（§7.2）**；**是否启动该 PR 由评审裁决** |
| **若不批准** | 降级路径：**只做 I-2a**（零 wire 增补，`scope`/元数据可得）+ PR-C **A39 降级分支**（前端按 Driver 下发的 `template`+`keys` 通用替换，标注「前端渲染、未经 Driver 校验」，身份一致性仍由 Configure 的 D11 兜底）。此时 **I-2 无「服务端权威 Preview」**，PR-C 的 **L2 只能部分达成**（`scope` 过滤 ✅ / 支持状态展示 ✅ / 权威 Preview ❌） | 该降级**必须**在验收结论里显式写明，**不得**用 I-2a 冒充 L2 |

#### ④ 「不引入新的通用框架」的落地保证

新增的 RPC **不是**通用 RPC 框架，而是**与既有 `GetDescriptor` 逐字同构的两条具名消息**：

| 通用框架的特征 | 本设计是否具备 | 说明 |
|---|---|---|
| 可任意扩展的 op 分发器（`{op, args}`） | **否** | 两条**具名**消息，各自有确定的请求/响应类型；**明确拒绝**把它做成 `DefinitionQuery{request_json}` + `op` 字段（那才是通用框架） |
| 反射/插件式注册 | **否** | SDK 侧 `Driver` trait 新增两个**带默认实现**的方法（默认 `Unsupported`），与既有 `descriptor()`（`driver-sdk:105-123`）、`DriverConnection::browse()`（`:184-196`）、`probe()`（`:175-181`）**同一模式** |
| 新增进程/线程/队列 | **否** | 完全复用 `temp_op::temp_driver_attempt`（`temp_op.rs:138-166`），无新并发原语 |
| 新配置面 / 新持久化 | **否** | 目录是编译期内嵌数据（`definition.rs:47`）；**无任何持久化写入** |

---

## 2. 【特别要求 B】权威 Preview 是否需要真机 / 连接 —— 判定与依赖链

### 2.1 判定

> **判定：权威 Preview 完全离线可算。它不需要 Endpoint、不需要连接、不需要真机、不需要 Secret。**
> 它只需要**一个被 Driver Manager 临时 spawn 的 Driver 进程**，而该进程**不需要任何 Connection 条目**
> （即 `session.entries` 为空即可响应）。
>
> **并且**：预览**只能在 Driver 侧计算**，**不得**由 Core 复用、**不得**由前端实现第二套。

### 2.2 `canonical_identity` 的依赖链（逐层给 `文件:行号`）

**入口**：`drivers/sinumerik-nck/src/definition.rs:1091-1133`

```rust
pub fn canonical_identity(
    &self,                              // ← VariableDefinition（内嵌目录数据，纯内存）
    target: &TargetRef,                 // ← 调用方参数
    arguments: &Map<String, Value>,     // ← 调用方参数
) -> Result<String, CompileError>
```

**逐层依赖（全部为纯函数 / 纯内存，无 I/O）**：

| 层 | 调用 | 位置 | 是否触碰连接/会话 |
|---|---|---|---|
| 1 | `target.value_for_path(&spec.path)` | `drivers/sinumerik-nck/src/targets.rs:289-298`（纯 `match`；`drive_object` 返回递归 JSON，26/130 由此进入身份） | **否** |
| 2 | `arguments.get(&spec.path)` | `definition.rs:1100-1108` | **否** |
| 3 | `mesa_core_types::encode_typed_value(&value)` | `crates/core-types/src/point.rs:207`（类型标签编码器，**与 Core/前端同一函数**） | **否** |
| 4 | `render_identity_template(&self.identity_template, &encoded)` | `definition.rs:1137-1161`（纯字符串替换；未闭合 `{`、占位符不在 `identity_keys` 内均报错） | **否** |

**整条「编译 + 重算身份」链（预览要复用的确切范围）**：`drivers/sinumerik-nck/src/compiler.rs:351-413`（`compile_semantic`）

| 步骤 | 调用 | 位置 | 是否触碰连接/会话 |
|---|---|---|---|
| D2/D3 | `parameters` 键集合与类型校验 | `compiler.rs:356-371` | 否 |
| D5/D6 | `catalog.lookup(variable_id, revision)` | `compiler.rs:374` → `definition.rs:489-505` | 否（纯 `BTreeMap`） |
| family | `def.family != self.family` | `compiler.rs:376-381` | 否 |
| D7 | `target.validate(def)` | `compiler.rs:383` → `targets.rs:303-346` | 否 |
| D8/A9 | `def.validate_arguments(arguments)` | `compiler.rs:385` → `definition.rs:997-1084` | 否 |
| D9 | `assemble_native_fields` → `NativeNckAddress::from_parameters` → `resolve_native` | `compiler.rs:387-396` → `native.rs:230`、`native.rs:365` | 否 |
| **身份重算** | `def.canonical_identity(&target, arguments)` | `compiler.rs:398` → `definition.rs:1091` | 否 |
| **D11（唯一执行真值）** | `self.check_identity(persisted_point_key, &compiled)` | `compiler.rs:411` → `:489-520` | 否（但**它是比对，不是计算**） |

**源码级证据（本次 grep 实测，模块粒度的「无设备」证明）**：
`definition.rs`、`targets.rs`、`compiler.rs`、`native.rs`、`value.rs`、`address.rs`、`codec.rs`
**七个文件对 `NckClient` / `tokio` / `async fn` / `.await` / `session` / `connect(` 的匹配数均为 0**。

**唯一设备触点**：`drivers/sinumerik-nck/src/client.rs:55` 的 `NckClient::connect`。
它只被两处使用：`identity.rs:434`（`nck://identity` 身份探针树）与采集执行路径。

**`configure` 路径的对照证据（同一条链、同样无连接）**：
`drivers/sinumerik-nck/src/lib.rs:633-639` 装载 `DefinitionCatalog::embedded()`，
`:640` 构造 `AddressCompiler::new(&catalog, &self.cfg.family)` —— 都在 `configure` 内、
**在任何设备 I/O 之前**，且 `open_connection`（`lib.rs:354-377`）本身**只做 JSON 解析与 family 校验、不建任何连接**
（注释 `:363-365` 明写「连接只决定连哪台设备」）。

### 2.3 「纯定义编译」与「真实设备读取」的分界（可核对的清单）

| 能力 | 需要 Endpoint 记录？ | 需要连接/会话？ | 需要真机？ | 证据 |
|---|---|---|---|---|
| **定义目录**（`r1.json` 内容） | **否** | **否** | **否** | `definition.rs:47`（`include_str!` 编译进二进制）、`:381-433` + `:546-590`（装载即自检） |
| **`canonical_identity` / 权威 Preview** | **否** | **否** | **否** | §2.2 全链 |
| **`nck://presets` 浏览树**（语义叶 + `identity` 块） | **是** | **是**（但 `open_connection` 对 NCK 是纯本地，`lib.rs:354-377`） | **否**（`presets.rs:242-275` 不触点） | `core-api:818`/`:837`；`manager.rs:402-410` |
| **`nck://identity` 身份探针树** | 是 | 是 | **是** | `identity.rs:409`→`:434` `NckClient::connect(cfg).await` |
| **采集 `run` / Control `write`/`command`** | 是 | 是 | **是** | `client.rs:55`；`lib.rs:792` 起的数据面 |

> **这条分界直接决定了 I-2 的落点**：I-2 的两个端点**在表格的前两行**，
> 与表格后三行**没有任何共享执行路径**。

### 2.4 架构落点：为什么**只能**在 Driver 侧（无连接状态下计算）

| 候选落点 | 判定 | 依据 |
|---|---|---|
| **Driver 进程内、无连接状态下计算** | **✅ 选定** | 目录与身份模板是 Driver 的**驱动数据**（`definition.rs:11-12` 注释：「身份是 Driver 的产出」）；`canonical_identity` 是 Driver 的 API（`definition.rs:1091`）；`native_identity_key` 是 Driver 的固定规则（`definition.rs:1167-1176`） |
| Core 复用 | **❌ 禁止** | `core-types/src/point.rs:16-19`（Core 无身份推导）；PR-B `:229`（禁止）；P9 护栏 `resource_contract.rs:396-461`（`canonical_identity`/`identity_template` 在禁令词表内） |
| 前端计算 | **❌ 禁止（作为权威值）** | PR-C §1.6 红线 `:362-380`；前端可做的是**消费 Driver 下发的模板**（PR-C §1.6 边界澄清 1） |
| Manager 计算 | **❌ 禁止** | Manager 是驱动无关编排层，它**不认识** `variable_id`/`target`/`identity_template`（事实上 `crates/driver-manager/**` 对定义目录零引用） |

**「无连接时 Driver 是否能只加载定义目录即响应？」—— 能，且已有现成先例**：

| 证据 | 位置 | 说明 |
|---|---|---|
| SDK 的 `GetDescriptor` 分发**不查 `session.entries`** | `crates/driver-sdk/src/lib.rs:1126-1144` | 直接 `session.driver.descriptor()` 应答 ⇒ **零 Connection 条目下也能响应** |
| 对照：`BrowseRequest` 分发**必须先有连接** | `crates/driver-sdk/src/lib.rs:1656-1686` | 未命中即回 `NO_CONNECTION`（`InternalError`） |
| 对照：`ProbeRequest` 分发**必须先有连接** | `crates/driver-sdk/src/lib.rs` 的 `on_probe`（`session.entries.get(&req.connection_handle)`，未命中即 `NO_CONNECTION`） | 同上 |
| Manager 的 descriptor 路径**从不发 `OpenConnection`** | `crates/driver-manager/src/manager.rs:328-361` | 只有 spawn + 握手 + `get_descriptor()` + 缓存 |
| 对照：Manager 的 browse 路径**先发 `OpenConnection`** | `crates/driver-manager/src/manager.rs:399-449`（`OpenConnection` `:402-410`） | 这就是两条路径的**唯一差别** |
| 临时进程原语**只做 spawn + 握手** | `crates/driver-manager/src/temp_op.rs:96-125` | 不含任何 OpenConnection |

**⇒ I-2 的落点即「与 descriptor 完全同一形态」**：
Manager 临时 spawn Driver → 握手 → 直接发新 RPC → Driver 用编译进二进制的 `DefinitionCatalog::embedded()`
（`definition.rs:376-378`）与纯函数编译链（`compiler.rs:351-413`）应答 → 单出口回收（`temp_op.rs:40-43`）。

### 2.5 适用范围的边界情况（逐条给结论）

| # | 边界情况 | 结论 | 依据 |
|---|---|---|---|
| B1 | **`nck_variable`（语义点位）** | **离线可算**，且这是 I-2 的主用例 | §2.2 |
| B2 | **`nck_native` + `identity_mode="derived"`（缺省）** | **离线可算**，且**应当由服务端算**（前端若按固定规则自算，等于独立实现一条依赖 12 字段解析链的规则；`native.rs:230`/`:365` 的解析链**不得**在前端复现） | `compiler.rs:416-444`（`:433` 调 `native_identity_key`）→ `definition.rs:1167-1176` |
| B3 | **`nck_native` + `identity_mode="user"`** | **不受「离线可算」影响**，且**不提供预览重算**：Key 由用户显式指定，Driver **不重算、不静默改名**（`compiler.rs:485-488` 的 `user` 分支只复核命名空间与通用语法）。因此预览请求携带 `identity_mode:"user"` ⇒ **400 `INVALID_BINDING_CONFIG`**（**不新增码**），消息说明「`user` 模式身份由用户指定，服务端无权威身份可重算」 | `compiler.rs:462-481`（取值校验）、`:485-488`、`:512-521`（命名空间复核）；PR-C §1.5 的「身份保持原值」 |
| B4 | **`scope=identity` 的 6 条定义** | **目录必须如实返回 `scope`**（否则 PR-C 的 A40 fail-closed 无法判定）；**预览对其返回可算的 `point_key`**（与 Configure 行为**一致**，否则预览会「比真值更严」而说谎）；**前端**按 PR-C §1.4.4 第 4 行 / A13 / A40 **禁止其进入可采集列表**。**是否把 `scope` 校验下沉到 Configure ⇒ 不在 I-2 范围**（§9） | `definition.rs:51-57`；PR-C §10.2.3（权威修复点在 Driver 的 Browse/目录层） |
| B5 | **`nck://identity` 身份探针树（轴名 / 驱动对象名 / 编码器模块号…）** | **仍然需要真机**（`identity.rs:434`）。I-2 **不得**被实现为它的替代，也不得把「离线身份预览」与「在线身份探测」在接口层混同（否则会诱导前端把探针结果当成身份真值） | `identity.rs:409`/`:434`；`identity.rs:21-28`（页上限与「每项最多 5 次身份读取」） |
| B6 | **840d-sl** | 目录端点 `family=840d-sl` ⇒ **200 + `definitions: []` + `note`**（该系列**声明存在但无任何定义**）；`family=<未声明>` ⇒ **400 `UNSUPPORTED_FAMILY`**。两者**必须可区分**，否则「无定义」会被误读为「驱动故障」 | `drivers/sinumerik-nck/src/lib.rs:73`（`DECLARED_FAMILIES = ["828d","840d-sl"]`）；PR-B §6 `:916`（840D sl 无定义、不得套用 828D） |
| B7 | **目录装载/自检失败（摘要不符、ID 形状非法…）** | **fail-closed**：整个目录不可用 ⇒ **503 `DEFINITION_DIGEST_MISMATCH`**（`CatalogError::code()` 对全部变体返回同一 code）。**绝不**「跳过坏定义继续响应」 | `definition.rs:299-335`（`code()` `:332-334`）；同 `configure` 的处置 `lib.rs:633-639` |
| B8 | **Endpoint 尚不存在时能否预览** | **能**（driver 作用域，不查 Endpoint）。这正是把路由做成 **driver 作用域**而不是 endpoint 作用域的主要动机之一 | §3.1 |
| B9 | **Device 断电 / 网线拔出 / host 不可达** | **对 I-2 无影响**：I-2 的实现链**没有任何 socket 调用**（§2.2 的 grep 证据；唯一触点在 `client.rs:55`） | §2.2 |
| B10 | **适用范围的上界（必须写明）** | 本节的「不需要真机」结论**只覆盖「目录 + `canonical_identity`」**。**不包括**设备事实（轴名、驱动对象名、模块号、编码器类型…），那些来自 `nck://identity`（PR-C 的 `leaf_node_with`+`area_no` 预填就是这条路径，`identity.rs:247`），**必须真机** | `identity.rs:247`；`presets.rs:174-178` |

---

## 3. 问题 1：接口入口 —— Driver → Driver Manager → Core API 的调用路径

### 3.1 调用路径（逐层，附既有先例）

```
① 浏览器 / 前端（只读消费）
   GET  /api/v1/drivers/{id}/definitions?family=828d[&variable_id=…][&revision=…]
   POST /api/v1/drivers/{id}/definitions/identity-preview     （body = 纯查询 JSON，无副作用）
        │
        │  先例：GET /api/v1/drivers/{id}/descriptor          core-api/src/lib.rs:3241-3244
        │        POST /api/v1/drivers/{id}/probe（只读 POST）  core-api/src/lib.rs:3249 / :782-803
        ▼
② Core API 层（crates/core-api）
   · 路由表新增 2 条（:3235-3342 内，descriptor 邻域）
   · handler：只做「驱动 id 传递 + 请求体形状校验 + 结果逐字透传 + 按 ErrorKind 映射 HTTP 状态」
   · 【硬约束】本层新增代码**零 Siemens 标识**（P9 护栏 resource_contract.rs:396-461 扫描本目录）
        │
        ▼
③ Manager 层（crates/driver-manager）
   MesaManager::definitions(driver_id, query_json)
   MesaManager::preview_point_key(driver_id, request_json)
   · 形态与 get_descriptor 逐字同构：manager.rs:308-370
   · 走 temp_op::temp_driver_attempt（temp_op.rs:138-166）：
        spawn → Hello/Welcome 握手 → 发 RPC → invalidate → terminate（单出口回收 :40-43）
   · **不发 OpenConnection**（对照 browse 的 manager.rs:399-410）
        │
        ▼
④ IPC（既有 Envelope / oneof body）
   新增两个消息对（additive tag 45–48）：
        GetDefinitions {} + DefinitionsReport { result, definitions_json }
        PreviewPointKey { request_json } + PreviewIdentityReport { result, result_json }
   · 既有 35 个 tag（10–44）与全部既有 message 逐字不动（§7.2 / §7.2.1）
        │
        ▼
⑤ Driver SDK（crates/driver-sdk）
   Driver trait 新增两个**带默认实现**的方法（默认 Unsupported）
   · 先例：descriptor() 默认实现 driver-sdk:105-123；DriverConnection::browse() 默认 :184-196；probe() 默认 :175-181
   · 控制面分发新增 2 个 match 分支（:1126 / :1169 邻域）；兜底分支 :1178-1179 保持
   · 分发**不查 session.entries**（对照 GetDescriptor :1126-1144）⇒ 零 Connection 条目可响应
        │
        ▼
⑥ NCK Driver（drivers/sinumerik-nck）
   定义目录：DefinitionCatalog::embedded()                          definition.rs:376-378
   权威预览：compiler.rs 的「校验 + 重算身份」链（D2–D9 + A9 + canonical_identity）
             **跳过 D11 比对**（compiler.rs:411 / :489-520 仍是唯一执行真值）
        │
        ▼
⑦ 返回：JSON 字符串 + GenericResult(ok | error_detail(kind, code, message))
   driver-protocol/src/lib.rs:618 / :625 → Core 按 kind 映射 HTTP → 浏览器
```

### 3.2 「扩展点应落在哪一层」的理由（逐层）

| 层 | 本设计加什么 | 为什么是这一层 | 不该落在别处的理由 |
|---|---|---|---|
| **Driver（NCK）** | 两个只读方法的实现（目录投影 + 权威预览） | 它是**唯一的 NCK 语义所有者**：目录数据（`definition.rs`）、身份模板（`identity_template`）、重算入口（`canonical_identity`）、原生解析链（`native.rs`）都在这里 | 放到 Core ⇒ 违反 `point.rs:16-19` / PR-B `:229` / **且被 P9 护栏判红** |
| **Driver SDK** | `Driver` trait 两个**带默认实现**的方法（默认 `Unsupported`） | 保持「其它 4 个驱动零改动」：默认实现让它们**编译期与运行期都不变**，且能在**被误调时快速报错而不是静默超时** | 若不给默认实现 ⇒ 4 个其它驱动被迫改动（违反 P1 护栏「非 NCK 功能不得退化」） |
| **IPC / driver-protocol** | 两个具名消息对（additive tag 45–48）+ `PROTOCOL_MINOR 6→7` + 一个新门控常量 | 这是**唯一**能让「用户参数从 Core 传到 Driver」的通道；且必须是 additive，才能让既有 35 个 tag 与 4 个其它驱动**逐字不动** | 复用 `BrowseRequest.filter` / `ProbeResponse` ⇒ 语义滥用（§1.2 C4/C5） |
| **Manager** | 两个编排方法（复用 `temp_driver_attempt`，**不发 OpenConnection**） | Manager 是「driver 进程生命周期 + 管理面 RPC 编排」的既有归属地；descriptor 已在此层（`manager.rs:308-370`）。Manager **不认识任何 NCK 名词** | 把编排放到 Core ⇒ Core 需要知道「临时进程」的机器细节，破坏分层（`core-api` 从不 spawn 驱动） |
| **Core API** | 2 条只读路由 + 2 个 handler + 1 个**驱动无关**的 `ErrorKind → StatusCode` 映射 | Core 的既有职责是「驱动作用域只读端点 + JSON 透传」（descriptor 就是如此） | 让 Core 认识 `variable_id`/`scope`/`UNKNOWN_VARIABLE` ⇒ **P9 护栏直接判红**（`resource_contract.rs:398-428` 词表已含这些 token） |
| **前端** | 只读客户端（PR-C 的范围，`apps/mesa-web/src/nckCatalog.ts`，见 PR-C `:428`） | 前端**不得**发明第二套身份算法（PR-C §1.6 红线） | 前端自算身份 ⇒ PR-C §1.6 违规 |

### 3.3 Core 侧「按 `ErrorKind` 映射 HTTP」是**必须**的做法（不是可选优化）

- **不能**按 Driver 的 `code` 字符串映射：`UNKNOWN_VARIABLE` / `UNSUPPORTED_DEFINITION_REVISION` /
  `MISSING_TARGET` / `INVALID_TARGET` / `INVALID_ARGUMENTS` / `POINT_KEY_MISMATCH` 等
  **全部在 P9 禁令词表内**（`resource_contract.rs:415-421`），一旦出现在 `crates/core-api/src`
  的生产代码里，**`core_has_zero_siemens_knowledge` 会直接失败**。
- **只能**按 `mesa_core_types::ErrorKind`（`crates/core-types/src/lib.rs:624-634`，9 个变体）
  映射，且该映射函数**只含 `ErrorKind` 变体字面量**（矩阵 **I26** 用 grep 门断言）。
  每一条错误响应的 `ErrorKind` 归属见 **§6.2.1**（**确定性归属表**）；
  **「协议不支持」必须 `Unsupported`（501），严禁误报为「设备离线」**（矩阵 **I9**）。
- `ErrorKind` 早已随 Driver 错误一起过线（`driver-protocol` 的 `error_detail(kind, code, message)`，
  `:625`；`ErrorDetailBox::is_configuration_error()` `:669`），因此这条映射**不需要新增任何 wire 字段**。

---

## 4. 问题 2：定义目录 —— 请求 / 响应与字段语义

### 4.1 请求

```http
GET /api/v1/drivers/{id}/definitions?family=828d HTTP/1.1
Accept: application/json
```

| 参数 | 位置 | 必填 | 语义 | 校验失败 |
|---|---|---|---|---|
| `id` | path | **是** | 驱动清单 id（`driver.toml` 的 `id`，实测 NCK 为 `sinumerik-nck`） | 不在清单 ⇒ **503 `DRIVER_UNAVAILABLE`**（对齐 descriptor handler `core-api:646`） |
| `family` | query | 否 | 系列过滤。**缺省 = 目录中全部已发布 family**（当前只有 `828d`）。**Core 不解释它**，逐字透传。【已裁决】目录查询**可省略** `family`；省略即返回**全部已发布 family** | 未声明系列 ⇒ **400 `UNSUPPORTED_FAMILY`**；已声明但无定义（`840d-sl`）⇒ **200 + `[]` + `note`** |
| `variable_id` | query | 否 | 按变量精确查询（`DefinitionCatalog::lookup` 的查询侧） | 不存在 ⇒ **404 `UNKNOWN_VARIABLE`** |
| `revision` | query | 否 | 与 `variable_id` 联用；**不得缺省回落** | 变量存在但 revision 不存在 ⇒ **404 `UNSUPPORTED_DEFINITION_REVISION`**（消息含 `available[]`） |
| `revision` 缺 `variable_id` | — | — | **400 `INVALID_QUERY`**（拒绝「全目录过滤 revision」这种语义不明确的查询） | — |
| `limit` | query | 否 | **结果数量上限（分页）**。缺省 **100**、上限 **500**（超出即夹到 500，与 Browse 的 `limit` 同口径，`core-api:834`） | 非正整数 ⇒ **400 `INVALID_QUERY`** |
| `cursor` | query | 否 | **不透明游标**：语义 = 「上一页最后一条 `variable_id`」（按 `variable_id` 字典序，稳定性由「已发布 `(variable_id, revision)` 不可原地修改」保证，`definition.rs:8-10`）。**与 `variable_id` 互斥** | 与 `variable_id` 同时出现 ⇒ **400 `INVALID_QUERY`**；游标不在当前结果集 ⇒ **400 `INVALID_QUERY`** |

> **【已裁决】`family` 缺省的落点**：目录查询**可省略** `family`，省略时返回**全部已发布 family**；
> **Preview 则必须显式指定 `family`**（§5.1，无缺省）。此前 §11-3 把该差异登记为「待评审确认」，
> 现**已裁决**，登记改为「已裁决、保留差异」。
>
> **`family` 为什么不能像 PR-C 措辞那样「缺省用 Endpoint/驱动配置」**：本设计的路由是
> **driver 作用域**（不查 Endpoint，见 §3.1 的动机），因此**没有 Endpoint 连接配置可依据**。
> 这是与 PR-C `:259` 措辞的一处**显式差异**，已登记于 §11-3（【已裁决】）。
>
> **分页的诚实说明（不夸大）**：当前目录只有 **42 条**（本次实测，§1.1.3），单页即可装下，
> 因此 `next_cursor` 今天恒为 `null`。**但它不是可选装饰** —— 它是 §6.4 执行边界里
> 「结果数量上限」的**可验收载体**：一旦目录条目数量级增长（例如新增 family / 扩展 840D sl），
> 契约**不需再改**。这与 Browse 的「分页、懒加载，禁一次返全空间」
> （`proto/driver.proto:55`）是**同一纪律**。

### 4.2 响应（200）

> **`definitions` 的字段集合 = PR-C §1.4 I-2 已枚举的白名单**（`nck-prc-...:260`）。
> 逐字取自 `r1.json`，**不重命名、不重排、不重算**（唯一例外：`digest` —— 它是装载期计算值
> `definition.rs:478-484`/`:593-609`，见 §4.4）。
> **`addressing` / `decode` / `transport_size` / `count` / `unit_mode` / `invalid_rule` 一律不返回**
> （理由见 §4.4）。

```json
{
  "format_version": 1,
  "family": "828d",
  "revision": 1,
  "driver_id": "sinumerik-nck",
  "driver_version": "0.5.0",
  "note": null,
  "total": 42,
  "limit": 100,
  "next_cursor": null,
  "definitions": [
    {
      "variable_id": "channel.r_parameter",
      "revision": 1,
      "family": "828d",
      "scope": "collect",
      "semantics_status": "verified",
      "label": { "zh": "通道 R 参数", "en": "Channel R parameter" },
      "source": [
        { "kind": "manual", "reference": "Siemens 828D NC variables 03/2025 A5E53370586B AC" },
        { "kind": "frozen_vector",
          "reference": "drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json",
          "note": "C/RP/rpa（index=0 → line=1）" }
      ],
      "targets": ["channel"],
      "target_required": ["channel"],
      "arguments": [
        { "key": "index",
          "label": { "zh": "R 参数号", "en": "R parameter index" },
          "value_type": "integer",
          "required": true,
          "min": 0,
          "max": 65534 }
      ],
      "identity_template": "channel.r_parameter#index={index}@channel={channel}",
      "identity_keys": [
        { "path": "index", "required": true },
        { "path": "channel", "required": true }
      ],
      "golden_ref": [
        { "area": "C", "block": "RP", "variable": "rpa", "entry": "nc_variable" }
      ],
      "digest": "eba34bc560283a2acc66fa7956c5add6a1289390c0ed489122f88e8032286a6d"
    },
    {
      "variable_id": "drive.parameter.r0021",
      "revision": 1,
      "family": "828d",
      "scope": "collect",
      "semantics_status": "parameter_identity",
      "label": { "zh": "驱动参数 r0021（仅参数身份，非工程语义）",
                 "en": "Drive parameter r0021 (parameter identity only)" },
      "source": [
        { "kind": "manual", "reference": "SINAMICS S120/S150 参数手册 11/2023" },
        { "kind": "frozen_vector",
          "reference": "drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json",
          "note": "V/M/r0021 × 2（nc_variable / drive_object）" }
      ],
      "targets": ["drive_object"],
      "target_required": ["drive_object"],
      "arguments": [],
      "identity_template": "drive.parameter.r0021@drive_object={drive_object}",
      "identity_keys": [ { "path": "drive_object", "required": true } ],
      "golden_ref": [
        { "area": "V", "block": "M", "variable": "r0021", "entry": "nc_variable" },
        { "area": "V", "block": "M", "variable": "r0021", "entry": "drive_object" }
      ],
      "digest": "80eee8ba825cb44fe51828e22ca97ebba8016b2126530f47361abad26f686288"
    },
    {
      "variable_id": "channel.machine_axis_assignment",
      "revision": 1,
      "family": "828d",
      "scope": "identity",
      "semantics_status": "verified",
      "label": { "zh": "通道位置→机床轴号（身份探针）",
                 "en": "Channel position to machine axis number (identity probe)" },
      "source": [
        { "kind": "identity_probe", "reference": "drivers/sinumerik-nck/src/identity_spec.rs",
          "note": "CHANNEL_MACHAX_USED" },
        { "kind": "measurement", "reference": "docs/evidence/nck-live-identity-probe.txt" },
        { "kind": "frozen_vector",
          "reference": "drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json",
          "note": "C/M/AXCONF_MACHAX_USED" }
      ],
      "targets": ["channel"],
      "target_required": ["channel"],
      "arguments": [
        { "key": "position",
          "label": { "zh": "通道位置（机床轴序号）", "en": "Channel position (machine axis index)" },
          "value_type": "integer", "required": true, "min": 0, "max": 65535 }
      ],
      "identity_template": "channel.machine_axis_assignment#position={position}@channel={channel}",
      "identity_keys": [
        { "path": "channel", "required": true },
        { "path": "position", "required": true }
      ],
      "golden_ref": [
        { "area": "C", "block": "M", "variable": "AXCONF_MACHAX_USED", "entry": "nc_variable" }
      ],
      "digest": "bcbfe145b413195de70e4dd7c1238fd8518af14e78979f1c9f38cd2ea6a1d5de"
    }
  ]
}
```

> 上面三段 JSON 的**值**全部逐字取自 `r1.json` 与 `DIGESTS.lock` 的实测内容
> （`channel.r_parameter`、`drive.parameter.r0021`、`channel.machine_axis_assignment` 三条定义
> 由本次解析真实文件得到），**不是编造的示例**。完整响应应为 `definitions` 长度 **42**。
> `total` / `limit` / `next_cursor` 是**分页信封**（§4.1）：今天 `total = 42 = definitions.len()`、
> `next_cursor = null`；`total` 是**过滤后、分页前**的条目总数。
> **【已裁决】`digest` 与分页信封均为允许的返回元数据**，不再登记为「超出 PR-C 枚举」（见 §4.4-③、§11-4）。

### 4.3 字段语义表（逐字段「谁产出 / 谁消费 / 失败面」）

| 字段 | 取值域 | 产出层 | 消费层 | 若缺失/非法时的行为 |
|---|---|---|---|---|
| `format_version` | `1`（`DEFINITION_FORMAT_VERSION`，`definition.rs:41`） | Driver（内嵌文件） | 前端（只做展示/兼容判断） | 装载期即拒（`definition.rs:384-386`）⇒ 503 |
| `family`（顶层） | 请求的 family，或**全部已发布 family**（请求缺省时） | Driver | 前端 | — |
| `revision`（顶层） | `1`（文件级 revision，`definition.rs:362`） | Driver | 前端 | 装载期即校验（`definition.rs:422-428`） |
| `total`（分页信封） | 过滤后、分页前的条目总数（今天恒 = `42`） | Driver | 前端分页控件 / 断言 | — |
| `limit`（分页信封） | 实际生效的页大小（缺省 100、上限 500） | Core（夹取）+ Driver（回显） | 前端分页控件 | 非正整数 ⇒ 400 `INVALID_QUERY` |
| `next_cursor`（分页信封） | 下一页游标；无下一页为 `null`（今天恒 `null`，因 42 < 100） | Driver | 前端**逐字回传**（不得解析/构造） | 缺失视为 `null`（**已发布定义不可变**保证游标稳定） |
| `definitions[].variable_id` | `^[a-z][a-z0-9_.]*$` 形态（装载期自检） | Driver | 前端（作为 `parameters.variable_id` 唯一来源） | 非法 ⇒ 装载期 fail-closed |
| `definitions[].revision` | `>= 1` | Driver | 前端（作为 `parameters.definition_revision` 唯一来源） | 同上 |
| `definitions[].scope` | `collect` \| `identity`（`definition.rs:54-57`） | Driver | **前端 fail-closed 判定（A40/A13）** | **缺失不得默认放行**（PR-C §10.2.4） |
| `definitions[].semantics_status` | `verified` \| `candidate` \| `parameter_identity`（`definition.rs:62-69`） | Driver | 前端**如实展示** | **绝不允许**用 `verified` 兜底（PR-C §1.5） |
| `definitions[].label` | `{zh,en}` 非空 | Driver | 前端变量列表展示（PR-C §6.1） | — |
| `definitions[].source[]` | `kind ∈ {manual, measurement, identity_probe, frozen_vector}`（`definition.rs:82-87`）+ `reference` + 可选 `note` | Driver | 前端展示「参数来源」 | — |
| `definitions[].targets[]` | `TargetKind` 5 值（`targets.rs:46-52`） | Driver | 前端**动态生成** Target 控件（合法集合唯一来源） | 缺失 ⇒ 按 PR-C §1.4.4 第 2 行 ⇒ **禁止提交** |
| `definitions[].target_required[]` | `targets` 的子集 | Driver | 前端必填判定 | 同上 |
| `definitions[].arguments[]` | `{key,label,value_type,required,min?,max?,enum_options?,default?}`（`definition.rs:107-125`） | Driver | 前端控件生成 + 必填/范围校验 | 同上 |
| `definitions[].identity_template` | Driver 数据，**消费方不得重排**（`definition.rs:233-234`） | Driver | 前端**只读消费**（降级路径才做通用替换） | — |
| `definitions[].identity_keys[]` | `{path,required}`（`definition.rs:170-176`） | Driver | 前端展示「待确认字段」 | **不得**由它反推合法 Target 集合（PR-C §1.4.4 第 2 行 / A42） |
| `definitions[].golden_ref[]` | `{area,block,variable,entry}`，`entry ∈ {nc_variable, drive_object}`（`definition.rs:178-198`） | Driver | 前端**入口选项集合的唯一来源**（见 §4.4） | — |
| `definitions[].digest` | 小写 hex SHA-256（64 字符） | Driver（装载期计算） | 前端/运维（修订锚点、诊断） | 与 `DIGESTS.lock` 不符 ⇒ 装载期 fail-closed（`definition.rs:546-590`）；【已裁决】**允许**作为定义版本与完整性元数据返回 |

> **两条【已裁决】落点（本节）**：
> ① `definitions[].scope` **如实返回**（`collect`/`identity` 原值，**不得**在目录层过滤或删除，见 §9.3-C′）；
> ② `definitions[].digest` **允许返回** —— 第 1 轮曾把它登记为「超出 PR-C 枚举」的可选项
> （§11-4），现裁决为**允许**（定义版本与完整性元数据），因此**不再**存在「可删」这个待决状态。

### 4.4 三个必须在文档里说清楚的设计取舍

#### ① 为什么**不返回** `addressing` / `decode` / `transport_size` / `count` / `unit_mode` / `invalid_rule`

| 理由 | 依据 |
|---|---|
| PR-C §1.4 I-2 **枚举的输出字段本身就不含**这些原生字段（`nck-prc-...:260`）⇒ 本设计与 PR-C **逐字一致**，不构成偏离 | PR-C `:260` |
| 前端红线：语义路径**零**原生字段硬编码；原生字段只允许来自 Descriptor 的 `variable` 资源（矩阵 A35/A38） | PR-C `:1005`、`:1008` |
| 这些字段是**原生寻址与解码属性**，其正确使用要求理解 `NativeNckAddress` 解析链（`native.rs:230-345`）——把它交给前端等于邀请重实现 | PR-B §7.1 `:925-926` |
| 它们**仍然留在 Driver** 并**参与摘要**（`invalid_rule` 是定义的一部分、进 digest，`definition.rs:162-168`/`:232`）⇒ 不暴露**不损失**任何权威性 | 同左 |

> **对 PR-C「原样」的准确解读**：PR-C `:260` 的「**不得**在 Core 侧裁剪、改名或重排」是**对 Core 的禁令**；
> Driver 作为数据所有者，按**语义可见面**投影是本设计选择，且投影结果与 PR-C 的枚举**完全一致**。

#### ② 26 / 130 入口区别如何保留（`DriveEntry`）

| 问题 | 结论 | 依据 |
|---|---|---|
| 26/130 的**唯一实现处** | `DriveEntry::module()`：`NcVariable → 26`、`DriveObject → 130`（**不允许折叠**） | `drivers/sinumerik-nck/src/targets.rs:96-99`、`:103-108`；单一出处走 `NativeFieldSource::DriveEntryModule`（`definition.rs:139-140`） |
| 目录里**如何体现**入口区别 | 通过 `golden_ref[].entry`（**Driver 数据**）：20 条 `drive.*` 定义的 `golden_ref` **恰含两条**（`nc_variable` 与 `drive_object`），其余 22 条**恰含一条**。本次实测：任一入口集合 = `golden_ref[].entry` 去重 | 本次复核（§1.1.3）；`r1.json` 的 `drive.parameter.r0021` 两入口样本见 §4.2 |
| **是否**把 `26`/`130` 数字放进响应 | **否**。【已裁决】**保留 `entry` 的权威区别**（`nc_variable` / `drive_object` 是两个不同的语义入口，在身份层**不可折叠**）；**不需要向普通 UI 暴露原始 Module 数字**。理由：① 前端只需选 `entry`（`nc_variable`/`drive_object`），模块号由 Driver 经 `DriveEntry::module()` 派生；② 暴露 26/130 会让前端有机会「从 number 反推 entry」——这正是 PR-C **A14** 明令禁止的 | PR-C `:969`（A14）；PR-B `:568` |
| 前端如何知道要渲染入口选择器 | **规则**：`target_required` 含 `drive_object` ⇒ `drive_object.entry` 必填（`targets.rs:130-138` 的 `entry` 是**类型层必填**），其**可选值集合** = 该定义 `golden_ref[].entry` 的去重集合。两个输入都是 Driver 数据，**无需硬编码** | `targets.rs:130-138`、`:176-188`；PR-C A14 |

**⇒ 因此目录端点**足以**独立枚举全部可采集语义入口**：**42 条定义 → 62 个采集入口**
（20×2 + 22×1），与 62 个模板、62 条 wire golden **一一对应**（本次复核 62/62 hit、miss 0）
⇒ **前端不再必须依赖浏览树来发现变量**（浏览树仍是模板预填的入口，两者不冲突）。

#### ③ `digest` 字段：**【已裁决】允许返回**（第 1 轮曾登记为「超出 PR-C 枚举」）

| 项 | 说明 |
|---|---|
| 它是什么 | `(variable_id, revision)` 的 canonical-JSON SHA-256（`definition.rs:593-609`），与 `DIGESTS.lock` 的 42 条逐字对应 |
| 为什么加 | ① 让前端能把「目录里看到的 revision」与「预览时用的 revision」**绑定**（防止 UI 拿旧目录 + 新预览拼出不一致的组合）；② 它是「已发布 `(variable_id, revision)` 不可原地修改」这条硬约束的**可核对形态**（`definition.rs:8-10`）；③ 诊断价值：`POINT_KEY_MISMATCH` 排查时可直接比对摘要 |
| 是否引入第二份真值 | **否**。它由 Driver 装载期计算，`DIGESTS.lock` 是唯一权威来源，两者在装载期已被强制比对（`definition.rs:546-590`） |
| **【已裁决修正】裁决结论** | **允许**把 `digest` 作为**定义版本与完整性元数据**返回。第 1 轮的行文「若评审要求严格遵循 PR-C 枚举 ⇒ **可直接删除该字段**」**作废** —— 该字段**不再处于待决状态**，实现时**不得**以「PR-C 未枚举」为由删除它。理由（裁决口径）：它是**纯元数据**（不携带原生寻址/解码语义）、**不产生第二份真值**（装载期已与 `DIGESTS.lock` 强比对），且是 §5.4「预览与 Configure 之间定义是否被替换」的**唯一可核对锚点**。 |

### 4.5 覆盖度与「不落浏览树也能工作」的证据

| 断言 | 证据 |
|---|---|
| 42 条定义 = 全部可采集语义变量 | `r1.json` `definitions=42`（本次实测） |
| 42 条 → 62 个采集入口 | `golden_ref` 总 62（20×2 + 22×1，本次实测） |
| 62 个入口 ↔ 62 个模板 ↔ 62 条 wire golden | 本次逐条核对 hit=62/miss=0；`presets/828d.json` `templates=62`；`legacy-828d-wire.json` `entries=62` |
| 目录端点**不依赖浏览树** | 目录数据源是 `DefinitionCatalog::embedded()`（`definition.rs:376`），与 `PresetRegistry`（`presets.rs:31`）是**两条独立数据来源**（`definition.rs:23-25` 明写），只共享冻结向量 |

### 4.6 执行边界（**只读接口也必须有限**，规范条文见 §6.4）

> 【裁决要求】**即便不连接 CNC，也必须防止管理接口被无限量调用。**
> 完整规范、取值依据与复用的既有 Manager 机制**统一写在 §6.4**（请求大小上限、结果数量上限、
> 分页、超时、并发预算五项 + 四条只读性断言 R8–R11）。本章只列出**与本接口形状直接相关**的三条，
> 避免两处各写一份而漂移：

| 项 | 本接口（目录 `GET`）取值 | 与 §6.4 的关系 |
|---|---|---|
| 结果数量上限 | 单页 ≤ **500**（`limit` 上限）；单次响应 JSON ≤ **256 KiB**（对齐 `session.rs:677` 的 descriptor 门） | §6.4-② / §6.4-④ |
| 分页 | `limit`（缺省 100）+ `cursor`（不透明，= 上页末条 `variable_id`）+ `next_cursor` | §4.1 / §6.4-③ |
| 超时 | 复用 descriptor 同形态 **5s** RPC 超时 + **6s** 启动 deadline + attempt ≤ 2 | §6.4-④ |

---

## 5. 问题 3：权威 Preview —— 输入 / 输出语义与 D11 的关系

### 5.1 请求

```http
POST /api/v1/drivers/{id}/definitions/identity-preview HTTP/1.1
Content-Type: application/json

{
  "family": "828d",
  "kind": "nck_variable",
  "parameters": {
    "variable_id": "channel.r_parameter",
    "definition_revision": 1,
    "target": { "channel": 1 },
    "arguments": { "index": 5 }
  }
}
```

**输入的语义（逐字段）**：

> **【已裁决】先说清楚 `POST` 的语义（避免被误读为写操作）**：
> **本端点的 `POST` 表示「带参数的只读计算」**，**不代表写入**。
> 它**不创建** Endpoint、**不打开** CNC Connection、**不读取**机床实时值、**不修改**采集计划、
> **不产生**任何持久化副作用（四条只读性断言见 §6.3 的 **R8–R11** 与 §6.4-⑥）。
> 之所以用 `POST` 而不是 `GET`：请求需要承载 `target`/`arguments` **结构化参数对象**，
> 放进 query string 会引入编码歧义与长度上限问题。**POST-for-read 已有两个既有先例**：
> `POST /api/v1/drivers/{id}/probe`、`POST /api/v1/drivers/{id}/validate-connection`
> （`crates/core-api/src/lib.rs:3245-3249`）。**「HTTP 方法」不是「是否写入」的判据 —— 副作用才是。**

| 字段 | 语义 | 为什么这样设计 |
|---|---|---|
| `family` | 本次配置所面向的**设备系列**；**【已裁决】必须显式指定，无缺省**（与目录查询不同，目录查询可省略） | 与 `Endpoint.connection.family`（`drivers/sinumerik-nck/src/config.rs:34`、必填 `:175-179`）同义；因为路由是 driver 作用域（无 Endpoint），所以必须由调用方给出。Driver 用它执行 `def.family != family ⇒ UNSUPPORTED_FAMILY`（`compiler.rs:376-381`），**保持该检查可达**。**缺少 `family` ⇒ 400 `INVALID_REQUEST`（不是「猜一个」）** |
| `kind` | **Point Kind**：`nck_variable` \| `nck_native`（`compiler.rs:36`/`:38`） | 与 `PointSelection.kind`（`point.rs:107`）**同一个值**，前端无需做任何映射 |
| `parameters` | **与持久化 `PointSelection.parameters` 逐字同构的 JSON 对象** | 这是本设计的核心：**预览的输入 = 将来要持久化的那一段**。因此不存在「预览用一套参数、提交用另一套」的漂移面；Driver 也不需要有第二套解析器 |

**请求大小上限（与 §6.4-① 同源）**：整个 body ≤ **64 KiB**（远超合法请求所需的 ~2 KiB），
超限 ⇒ **413 `REQUEST_TOO_LARGE`**（`ErrorKind::Configuration`）。

**明确的输入禁令**：

- **不得**传 `point_key`：身份正是要**被算出来**的东西。传了也是多余字段 ⇒ 400 `INVALID_REQUEST`（信封层严格）。
- **不得**传 `identity_mode:"user"`：见 §2.5-B3 ⇒ 400 `INVALID_BINDING_CONFIG`。
- **不得**传任何原生字段（除 `kind="nck_native"` 的 12 字段白名单，`compiler.rs:45-58`）。

### 5.2 响应（200）

**用例 A：语义点位（`nck_variable`）**

```json
{
  "point_key": "channel.r_parameter#index=i:5@channel=i:1",
  "identity_mode": "derived",
  "scope": "collect",
  "variable_id": "channel.r_parameter",
  "definition_revision": 1,
  "definition_digest": "eba34bc560283a2acc66fa7956c5add6a1289390c0ed489122f88e8032286a6d"
}
```

**用例 B：复合 Target（`{channel,axis}`）**

```json
{
  "point_key": "axis.position.commanded_mcs@axis=i:3,channel=i:1",
  "identity_mode": "derived",
  "scope": "collect",
  "variable_id": "axis.position.commanded_mcs",
  "definition_revision": 1,
  "definition_digest": "64f1368787b394b477cbb8a734e2f91ea3c3820b729d66e14385f7eadaf84b4c"
}
```

**用例 C：26/130 入口（`drive_object.entry` 不可折叠）**

```json
// 请求 parameters：
// { "variable_id": "drive.parameter.r0021", "definition_revision": 1,
//   "target": { "drive_object": { "number": 3, "entry": "nc_variable" } }, "arguments": {} }
{
  "point_key": "drive.parameter.r0021@drive_object=o:{entry=s:nc_variable,number=i:3}",
  "identity_mode": "derived",
  "scope": "collect",
  "variable_id": "drive.parameter.r0021",
  "definition_revision": 1,
  "definition_digest": "80eee8ba825cb44fe51828e22ca97ebba8016b2126530f47361abad26f686288"
}
```

> `entry` 换成 `drive_object` 会得到**另一个** `point_key`（`…entry=s:drive_object…`），
> 这就是「26/130 不可折叠」在身份层的落地。该字面量形态与 `presets.rs:499-519` 的既有断言
> （`drive.parameter.r0021@drive_object=o:{entry=s:nc_variable,number=i:3}`）**同一编码**。

**用例 D：原生高级模式（`nck_native` + `derived`）**

```json
// 请求：{ "family": "828d", "kind": "nck_native",
//         "parameters": { "area": "C", "area_no": 1, "module": 21, "column": 1, "line": 396,
//                         "count": 1, "unit_mode": "current", "decode": "F64_LE",
//                         "transport_size": "0x09" } }
{
  "point_key": "native.C.1.21.1.396",
  "identity_mode": "derived",
  "scope": null,
  "variable_id": null,
  "definition_revision": null,
  "definition_digest": null
}
```

**输出的语义**：

| 字段 | 语义 | 谁消费 |
|---|---|---|
| `point_key` | **Driver 重算的规范身份**，即 `CompiledPoint.canonical_identity`（`compiler.rs:399-410` 的该字段） | 前端**逐字**写入 `PointSelection.point_key`；**不得**二次改写 |
| `identity_mode` | `derived` \| `user`（本端点只可能返回 `derived`，见 B3） | 前端展示/断言用 |
| `scope` | 该变量的适用范围（`nck_native` 为 `null`） | 前端在**提交那一刻**再断言一次 A40（纵深防御） |
| `variable_id` / `definition_revision` / `definition_digest` | 身份所用的**定义坐标**（`compiler.rs:403-407` 的 `DefinitionRef` 三字段） | 前端核对「我提交的定义版本 == 我预览的定义版本」；`nck_native` 全为 `null` |

**明确不返回的字段（含理由）**：

| 不返回 | 理由 |
|---|---|
| `source_label`（原生地址标签） | 它是原生寻址的产物；返回它等于把原生地址交到前端，与 A35/A38 的「语义路径零原生字段」冲突。诊断需求由 §6-错误消息的 `variable_id`+`revision`+**持久化/重算 Key** 三要素满足（`compiler.rs:125-133` 的 `PointKeyMismatch` 消息已含三要素） |
| `resolved`（`ResolvedPoint`：wire 布局/期望长度/transport） | 纯原生执行细节；前端**永远**不需要（`native.rs:136-160`） |
| 任何「null 表示身份不确定」的形态 | 见 §5.4 |

### 5.3 用户改了 Target / Arguments 之后如何请求服务端重新生成身份

| 步骤 | 行为 | 依据 |
|---|---|---|
| 1 | 用户改 `target`/`arguments` ⇒ **立即作废旧 `preview_point_key`**（PR-C §1.4.2 的硬性收紧，矩阵 A39） | PR-C `:246-248` |
| 2 | 前端用**当前表单值**组装 `parameters`（与将来持久化的那一段**逐字一致**），调用 `identity-preview` | §5.1 |
| 3 | 200 ⇒ 把返回的 `point_key` **逐字**作为 `PointSelection.point_key`；同时用返回的 `definition_revision`/`definition_digest` 断言「与目录里选中的那一行同版本」 | §5.2 |
| 4 | 4xx ⇒ **不得提交**；错误 `code`+`message` **原样展示**（PR-C §6.3） | PR-C `:929` |
| 5 | 503/501 ⇒ 标记为**待确认并禁止提交**（PR-C §1.5 的降级要求），**不得**回落前端自算值并冒充已验证身份 | PR-C `:248`、`:351` |

> **本端点取代的是 PR-C §1.4.2 表格里的「(a) 首选：服务端权威 Preview」**；
> **(b) 降级**（前端按 Driver 模板通用替换 + 标注「前端渲染、未经 Driver 校验」）在 I-2b 落地后**应当仅在 503/501 时启用**。

### 5.4 与「持久化 `point_key` 比对」（D11）的关系（**必须写清，否则会被误读为替代 D11**）

| 命题 | 结论 | 依据 |
|---|---|---|
| **预览是否替代 D11？** | **不替代**。D11（`check_identity`，`compiler.rs:489-520`）仍是**唯一的执行真值**，仍在 `configure` 时逐点执行，不一致仍报 `POINT_KEY_MISMATCH` 并**整体失败** | `compiler.rs:411`、`:489-520`；`drivers/sinumerik-nck/src/lib.rs:678-683`（逐点编译、任一失败即整体失败） |
| **预览与 D11 是同一个函数吗？** | **是同一个计算**。预览返回的 `point_key` **必须**等于 `compile_semantic`/`compile_native` 在 `compiler.rs:398`/`:433` 算出的 `canonical_identity`。这一条**必须**由矩阵 **I12** 对 42 条定义逐条断言（「预览不谎报」） | §8.3 |
| **数学关系** | 令 `K = canonical_identity(def, target, args)`。预览返回 `K`；前端写入 `point_key := K`；Configure 重算得 `K'`，D11 断言 `point_key == K'`。**若定义目录未变（同一 `revision`+`digest`），则 `K == K'`，D11 恒成立**。**若目录在预览与 Configure 之间被替换**（不同 `digest`），D11 会**正确拒绝** —— 这正是必须回带 `definition_digest` 的原因 | `definition.rs:452-469`（跨 revision 身份模板必须逐字相同） |
| **预览为什么必须跳过 D11 而不是「传一个自己算的 key」** | 因为先有鸡后有蛋：要传 `point_key` 才能走 D11，而 `point_key` 正是要算的东西。**因此实现上必须把「校验+重算」与「D11 比对」分成两步**（见 §8.1 的 `M10`），且**必须是同一个函数内的拆分**（不得复制一份算法） | — |
| **预览为什么必须走完整校验链（D1–D10）而不能只调 `canonical_identity`**（【已裁决修正】：第 1 轮此处写作「D2–D9 + A9」，**漏了 D1 与 D10**，权威表述见 §5.4.1） | **反例**：`channel=255` 能通过 Target 校验（`targets.rs:33-35` 注释明写），却会在原生解析阶段以 `INVALID_VARIABLE` 被拒。若预览只调 `canonical_identity`，就会给出一个**在 Configure 必然失败**的「权威 Key」⇒ 「预览通过、保存通过、启动失败」。**⇒ 预览必须复用 `compile_semantic` 的完整前半段（D1–D10），只跳过 D11** | `targets.rs:30-35`；`compiler.rs:387-396`；矩阵 **I19**、**I32** |
| **预览是否可能返回「身份无法确定」？** | **不可能，这是设计属性**。预览要么给出**非空** `point_key`，要么以精确 `code` 失败 —— **不存在**「部分/空/待确认」的成功态。PR-C §1.5 里 `preview_point_key = null` 的形态**只存在于浏览叶**（`presets.rs:249-253`，因为它只做「已确认字段」的投影，不接收用户参数） | 矩阵 **I20** |

#### 5.4.1 【裁决约束】Preview **必须**与 Configure **共用权威验证路径**（不是「调完 `canonical_identity` 就返回」）

> **规范条文（无歧义版，实现必须逐字遵守）**：
>
> 1. **允许离线 Preview**；但 Preview **必须先完成与 Configure 一致的全部校验** ——
>    **定义查找、Revision、family、Target、Arguments、以及地址可编译性** ——
>    **然后才**生成身份（`point_key`）。
> 2. Preview **应当跳过的、且只跳过 D11**。跳过的**唯一理由是请求本身尚未持有最终 Key**
>    （D11 的语义是「持久化 Key 与重算 Key 的比较」，而 Preview 阶段「持久化 Key」还不存在）。
>    **这不是跳过 D1–D10 的理由**，也**不是**「只做形状校验」的理由。
> 3. **实现形态**：**同一个内部函数**（`M10` 的拆分）承担「D1–D10 + 身份重算」，
>    `check_identity`（D11）作为**独立末步**保留在 Configure 路径上。
>    **禁止**复制第二份校验/算法；**禁止** Preview 走一条「只调 `canonical_identity`」的捷径。
> 4. **不得提供「看似有效但无法配置」的身份**：若定义已变更（`digest` 不同）或
>    Revision 不受支持，必须返回**明确错误**（不是可用的 Key），
>    由 §6.1.2 的 **E20**（`UNSUPPORTED_DEFINITION_REVISION`）与 **E29**（Configure 侧 D11 迟发拒绝）承担；
>    验收行见 **I33**。
>
> **【已裁决修正】**：第 1 轮 §5.4 第 4 行只写了「D2–D9 + A9」，**未涵盖 D1（Point Kind）与 D10（26/130 入口身份）**，
> 且**未明确 D11 是唯一被跳过项**。本小节为该约束的**权威表述**，与 §5.4 表格冲突时**以本小节为准**。

**Preview 实际执行的校验清单（D1–D11 逐条对照；D 编号取自 PR-B §3.6 的权威表，`nck-prb-driver-compiler-boundary.md:629-643`）**：

| D# | PR-B §3.6 的原始语义 | Preview 是否执行 | 执行位置（既有实现，不改） | 若不执行会怎样（反例） |
|---|---|---|---|---|
| **D1** | Point Kind ∈ {`nck_variable`, `nck_native`} | **✅ 执行** | `compiler.rs:36`/`:38` 的路由；非法 ⇒ `UNSUPPORTED_POINT_KIND`（`compiler.rs:185-187`） | 非法 `kind` 会被当成某一支处理，返回无意义的 Key |
| **D2** | `parameters` 层未知字段（含 `target` 对象内未知键） | **✅ 执行** | 两个 `#[serde(deny_unknown_fields)]` 输入结构（`compiler.rs:356-371`、`:451-472`） | 拼错字段名被静默忽略 ⇒ 用默认值算出「另一个 Key」 |
| **D3** | 类型错误（含 `target` 字段 JSON 类型错） | **✅ 执行** | 同上（serde）+ `from_parameters`（12 字段） | 类型被强制转换 ⇒ Key 与 Configure 不一致 |
| **D4** | 索引范围 | **✅ 执行** | `TargetRef::validate`（`targets.rs`）+ `from_parameters`（原生） | 越界索引进入身份模板 ⇒ 生成一个永远不能采集的 Key |
| **D5** | `variable_id` 存在性 | **✅ 执行** | `catalog.lookup`（`definition.rs:489-497`）⇒ `UNKNOWN_VARIABLE` | 拼错变量名仍返回 Key |
| **D6** | `definition_revision` 可解析性（**不得回落**） | **✅ 执行** | 同 `lookup`（`definition.rs:498-504`）⇒ `UNSUPPORTED_DEFINITION_REVISION` | 拿旧模板算新 revision ⇒ 「看似有效但无法配置」 |
| **D7** | `target` 的合法字段组合（缺必填 / 多余变体 / 越界 / 缺 `entry`） | **✅ 执行** | `target.validate(def)`（`compiler.rs:383` → `targets.rs:303-346`） | 缺 Target 也生成 Key ⇒ Configure 必拒 |
| **D8** | `arguments` 键/类型/范围/偏移上溢 | **✅ 执行** | `def.validate_arguments`（`compiler.rs:385` → `definition.rs:997-1084`） | 越界 argument 进模板 ⇒ Key 在 Configure 必拒 |
| **D9** | 原生地址有效性 | **✅ 执行** | `assemble_native_fields` → `NativeNckAddress::from_parameters` → `resolve_native`（`compiler.rs:387-396` → `native.rs:230`、`:365`） | **§5.4 第 4 行的反例**：`channel=255` 过 D7 但 D9 失败 |
| **D10** | Module 26/130 入口身份 | **✅ 执行** | `DriveEntry::module()` 经 `NativeFieldSource::DriveEntryModule` 进入身份（`targets.rs:96-108`、`definition.rs:139-140`） | `nc_variable` 与 `drive_object` 得到**同一个** Key ⇒ 入口不可区分 |
| **身份重算** | `canonical_identity(def, target, args)` | **✅ 执行**（**在 D1–D10 之后**） | `compiler.rs:398` → `definition.rs:1091-1133` | — |
| **D11** | `point_key` 与重算身份一致性 | **❌ 唯一跳过项**（请求无持久化 Key；Configure 侧仍逐点执行） | `compiler.rs:411`/`:442` → `:489-520` | —（**不得**因此跳过 D1–D10） |

**⇒ 一句话**：**Preview = `configure` 路径去掉最后一步 D11**，其余**逐条相同**。

#### 5.4.2 【裁决要求】跨接口一致性测试（**相同合法输入 ⇒ Preview 身份 == Configure 接受的身份**）

> **裁决原文要点的落地**：**相同合法输入下，Preview 返回的身份必须逐字等于随后 Configure 所接受的身份。**
>
> **计数说明**：本表是 **§8.3 验收矩阵 I32 的详细展开**（**同一编号，不重复计数**）；
> §8.3 的矩阵行只给一行摘要。**矩阵总行数仍为 I1–I45 = 45 行。**

| 编号 | 测试名（建议） | 断言要点（逐条可执行） | 所在套件 |
|---|---|---|---|
| **I32** | `preview_identity_equals_subsequently_configured_identity` | ① 对**全部 42 条定义**（含 20 条双入口、5 条复合 Target、2 条带 `arguments`）构造**合法**输入；② 先调 `identity-preview` 得 `K_p`（**必须 200 且 `K_p` 非空**）；③ 把 `K_p` **逐字**写入 `PointSelection.point_key`，走**真实 Configure**（`ConfigureTasks` + `ApplyPointMap`）；④ 断言 Configure **接受**且 `PointDescriptor` 数 == 点位数；⑤ **再加一条反向断言**：`K_p` **逐字等于** Configure 内部 `canonical_identity` 的重算值（即 `K_p == K_c`，字符串相等，**不做任何归一化**）。**任一条不成立即失败** | `definition_api`（Driver 单测）+ `management_api`（REST 侧 e2e） |

> **为什么这条测试是本轮新增的**：第 1 轮只有 **I12**（`preview == compile 的 canonical_identity`，**同一进程内的算法一致**）
> 与 **I19**（fail-closed 反例）。两者**都不能**证明「**跨接口**（Preview RPC ↔ Configure RPC）
> 在**相同输入**下给出**同一个**身份」。I32 补的正是这个缺口：它是**唯一**能同时覆盖
> 「Preview 与 Configure 共用同一条验证路径」+「身份逐字一致」的验收项。

#### 5.4.3 【裁决要求】定义已变更 / Revision 不受支持 ⇒ **明确错误**，**不得**给「看似有效」的身份

| 情形 | 期望行为 | 码 / `ErrorKind` | HTTP | 验收行 |
|---|---|---|---|---|
| `(variable_id, revision)` 不在当前内嵌目录 | **拒绝**，消息含 `available[]` | `UNSUPPORTED_DEFINITION_REVISION` / `Configuration` | **404** | **I33**（+ 既有 I17/E20） |
| `variable_id` 不在目录 | **拒绝** | `UNKNOWN_VARIABLE` / `Address` | **404** | **I33**（+ 既有 I17/E19） |
| Preview 之后、Configure 之前目录被替换（`digest` 不同） | Preview **不报错**（当时合法）；**Configure 侧 D11 拒绝**，前端**不得**把 Preview 结果标为「已确认」 | `POINT_KEY_MISMATCH` / `Configuration` | **400** | **I33**（+ **E29**） |
| 目录装载自检失败（fail-closed） | **整体不可用**，不返回任何 Key | `DEFINITION_DIGEST_MISMATCH` / `Internal` | **503** | 既有 I/B7（E7/E28） |

> **⇒ 硬性约束**：**不得**为了「体验顺畅」而在上述任一情形返回一个「看起来能用的 `point_key`」。
> **Preview 的成功态只能意味着「此刻按当前目录，这个输入可被 Configure 接受」**；它**不是**「已持久化验证通过」。

---

## 6. 问题 4：错误与安全

### 6.1 错误码表（**条件 → 码 → HTTP 状态 → 消息要素**）

> **三条总则**：
> **(1)** 所有码在 **Driver 侧**产生，经 `error_detail(kind, code, message)`（`driver-protocol:625`）
> 过线；**Core 侧不得出现任何 NCK 码字面量**（P9 护栏，`resource_contract.rs:415-421`）。
> **(2)** HTTP 状态由 Core **按 `ErrorKind`** 决定（§3.3），**不按码**。
> **(3)** 【裁决要求】**每一条错误响应都必须具有确定的 `ErrorKind`**，归属表见 **§6.2.1**；
> 其中 **「协议不支持」一律 `ErrorKind::Unsupported`（HTTP 501）**，
> **严禁**误报为 `ConnectionError` / `DeviceError`（HTTP 503，「设备离线」语义）——
> 这两者对运维的含义完全相反（「换 Core / 升级驱动」vs「查线/查机床」），**混淆即误导排障**。

#### 6.1.1 目录端点（`GET /api/v1/drivers/{id}/definitions`）

| # | 条件 | 码 | HTTP | 消息要素 |
|---|---|---|---|---|
| E1 | 驱动 id 不在清单 / 不可启动 | `DRIVER_UNAVAILABLE` | **503** | `driver_id`；Manager 阶段（`spawn`/`connect`） |
| E2 | 驱动进程 spawn 或握手失败 | `DRIVER_UNAVAILABLE` | **503** | 阶段名 + 原始错误（`temp_op.rs:72-83` 的 `Display`） |
| E3 | 目录 RPC 超时 | `DEFINITION_QUERY_TIMEOUT` | **503** | `driver_id`；预算 ms（形态对齐 `DRIVER_DESCRIPTOR_TIMEOUT`，`manager.rs:331-333`） |
| E4 | **目录 JSON 超过上限**（字节 > 256 KiB）**或条目数超过上限**（过滤后条目 > 4096） | `DEFINITION_QUERY_TOO_LARGE` | **503** | 实测字节数 / 实测条目数 + 对应上限（形态对齐 `session.rs:677` 的 256 KiB 门） |
| E5 | 对端 `negotiated_minor < 7`（**旧二进制，会静默忽略未知消息**） | `DEFINITIONS_UNSUPPORTED`（**`ErrorKind::Unsupported`，不得为 `Connection`/`Device`**） | **501** | `driver_id`、`negotiated_minor`、所需 minor。**必须先门控再发帧**，不得发帧干等（同 `probe.rs:147-151` 的口径） |
| E6 | 驱动实现该 RPC 但未实现业务（如其它驱动） | `DEFINITIONS_UNSUPPORTED`（SDK 默认实现 `Unsupported`，**`ErrorKind::Unsupported`**） | **501** | 驱动 id + 「该驱动未提供定义目录」 |
| E7 | 内嵌目录装载/自检失败（格式版本、ID 形状、身份覆盖闭包、摘要不符、lock 多余项…） | `DEFINITION_DIGEST_MISMATCH`（`CatalogError::code()` 对**全部**变体返回同一 code，`definition.rs:330-334`） | **503** | `CatalogError` 的完整 `Display`（含 `variable_id`/`revision`/`expected`/`actual`） |
| E8 | `family` 未在驱动声明系列内 | `UNSUPPORTED_FAMILY` | **400** | 请求的 `family` + `DECLARED_FAMILIES`（`lib.rs:73`） |
| E9 | `family` 已声明但该系列无任何定义（`840d-sl`） | —（**不是错误**） | **200** | `definitions: []` + `note`（必须可区分于 E8 与 E7） |
| E10 | `variable_id` 指定但不在目录中 | `UNKNOWN_VARIABLE` | **404** | `variable_id` |
| E11 | `variable_id` 在目录但 `revision` 不存在 | `UNSUPPORTED_DEFINITION_REVISION` | **404** | `variable_id` + 请求 revision + `available[]`（`definition.rs:498-504`） |
| E12 | 查询形状非法（`revision` 非正整数 / `revision` 缺 `variable_id` / `limit` 非正整数 / `cursor` 与 `variable_id` 同时出现 / 游标不在结果集） | `INVALID_QUERY` | **400** | 参数名 + 期望形态 |

#### 6.1.2 预览端点（`POST /api/v1/drivers/{id}/definitions/identity-preview`）

| # | 条件 | 码 | HTTP | 消息要素 |
|---|---|---|---|---|
| E13 | body 非法 JSON / 未知字段 / 缺 `family`/`kind`/`parameters` / 出现 `point_key` | `INVALID_REQUEST` | **400** | JSON 指针 + 期望形态（**只校验形状，不解释语义**） |
| E14 | 驱动不可达 / spawn 失败 | `DRIVER_UNAVAILABLE` | **503** | 同 E1/E2 |
| E15 | 预览 RPC 超时 / 超限 / minor 不足 | 同 E3/E4/E5 | **503**/**501** | 同 E3/E4/E5 |
| E16 | `kind` ∉ {`nck_variable`,`nck_native`} | `UNSUPPORTED_POINT_KIND` | **501**（`ErrorKind::Unsupported`，`compiler.rs:185-187`） | `kind` |
| E17 | `parameters` 层未知键 / 类型错 / `identity_mode` 取值非法 | `INVALID_BINDING_CONFIG` | **400** | 未知键名 / 期望类型（`compiler.rs:356-363`、`:451-472`） |
| E18 | `identity_mode:"user"`（无权威身份可重算） | `INVALID_BINDING_CONFIG` | **400** | 明写「`user` 模式身份由用户指定，服务端不重算」（§2.5-B3） |
| E19 | `variable_id` 不在目录 | `UNKNOWN_VARIABLE` | **404** | `variable_id` |
| E20 | `(variable_id, revision)` 不存在 | `UNSUPPORTED_DEFINITION_REVISION` | **404** | `variable_id` + revision + `available[]` |
| E21 | `def.family` ≠ 请求 `family` | `UNSUPPORTED_FAMILY` | **501** | `definition_family` + 请求 family（`compiler.rs:376-381`） |
| E22 | `target` 缺失必填变体（含空 target） | `MISSING_TARGET` | **400** | `variable_id` + `target_required[]` + 具体原因（`targets.rs:305-322`） |
| E23 | 提供了未声明的 Target 变体 | `UNSUPPORTED_TARGET` | **400** | `variable_id` + 该变体 + 该定义声明的 `targets[]`（`targets.rs:323-332`） |
| E24 | 实例号越界 / `drive_object.entry` 缺失或类型错 / `drive_object.number` 缺失 | `INVALID_TARGET` | **400** | 字段路径 + 合法域（`0..=255`，`targets.rs:36`）+ 入口必填说明（`targets.rs:163-188`） |
| E25 | arguments 未知键 / 类型错 / `min`/`max` 越界 / 显式 `null` | `INVALID_ARGUMENTS` | **400** | `key` + 实测值 + 合法域（`definition.rs:998-1058`） |
| E26 | `identity_keys` 声明的键不存在 | `MISSING_IDENTITY_KEY` | **400** | `variable_id` + `revision` + 缺失 `path`（`definition.rs:1060-1075`） |
| E27 | 原生解析链失败（D9：如 `channel=255` 通过 Target 校验但 `area_no` 越界、`resolve_native` 失败） | `INVALID_VARIABLE` | **404**（`ErrorKind::Address`，`compiler.rs:188-190`） | `variable_id` + 原生解析原因（`compiler.rs:387-396`） |
| E28 | 内嵌目录自检失败 | `DEFINITION_DIGEST_MISMATCH` | **503** | 同 E7 |
| **E29** | **Preview 与 Configure 之间定义被替换**（`digest` 不同）⇒ Configure 侧 **D11** 拒绝（【裁决要求】「不得提供看似有效但无法配置的身份」的**收口点**，见 §5.4.3） | `POINT_KEY_MISMATCH` | **400**（`ErrorKind::Configuration`） | **持久化 Key + 重算 Key + `variable_id` + `revision`**（三要素，`compiler.rs:125-133`）；Configure **整体失败**，无点位进入计划（`lib.rs:678-683`） |

#### 6.1.3 共同执行边界错误（两个端点共用，与 §6.4 同源）

| # | 条件 | 码 | HTTP | 消息要素 |
|---|---|---|---|---|
| **E30** | **请求体超过大小上限**（预览 `POST` body > **64 KiB**） | `REQUEST_TOO_LARGE` | **413**（`ErrorKind::Configuration`） | 实测字节数 + 上限；**Core 侧产生**（框架级，见 §6.4-① 与 §6.2.1 的「Core 侧驱动无关码」） |
| **E31** | **并发预算耗尽**（同一 driver 同时进行的定义查询 > **4**） | `DEFINITION_QUERY_BUSY` | **503**（`ErrorKind::Internal`） | `driver_id` + 当前 in-flight 数 + 预算；**Manager 侧产生**（驱动无关码，与 E3/E4 同族） |

**码表条数（本轮修订后）**：目录端点 **12** 条（E1–E12）+ 预览端点 **17** 条（E13–**E29**）
+ 共同执行边界 **2** 条（**E30–E31**）= **31** 条（第 1 轮为 28 条）。
其中 **新增码共 6 个**（`DEFINITION_QUERY_TIMEOUT` / `DEFINITION_QUERY_TOO_LARGE` /
`DEFINITIONS_UNSUPPORTED` / `INVALID_QUERY` / **`REQUEST_TOO_LARGE`** / **`DEFINITION_QUERY_BUSY`**），
其余 **25 条全部复用**既有码（PR-B §3.5 的 `CompileError` 表 `:589-606` 与 manager 侧既有码）。
**【已裁决修正】**：第 1 轮写「新增码仅 4 个」，本轮因**执行边界**（§6.4）新增 2 个**驱动无关**的边界码，
故为 **6 个**；这两个码**不含任何 NCK 语义**，因此**不违反** P9 护栏。

### 6.2 HTTP 状态映射规则（Core 侧，**驱动无关**）

| `ErrorKind`（`crates/core-types/src/lib.rs:624-634`，**9 个变体**：`:625-633`） | HTTP | 说明 |
|---|---|---|
| `ConfigurationError` | **400** | 用户输入问题（缺 Target、arguments 越界、未知字段…） |
| `AddressError` | **404** | 「地址/变量不存在」类（`UNKNOWN_VARIABLE`、`INVALID_VARIABLE`） |
| `Unsupported` | **501** | 沿用既有先例（`probe_result_to_response` 对 `ProbeError::Unsupported` 返回 `NOT_IMPLEMENTED`） |
| `Timeout` | **504** | RPC 超时 |
| `InternalError` / `ProtocolError` / `DecodeError` / `DeviceError` / `ConnectionError` | **503** | 基础设施/构建期问题 |
| **Manager 层基础设施码**（`DRIVER_UNAVAILABLE`、`DEFINITION_QUERY_TIMEOUT`、`DEFINITION_QUERY_TOO_LARGE`、**`DEFINITION_QUERY_BUSY`**） | **503** | 这些是**驱动无关**的 manager 码（与既有 `DRIVER_DESCRIPTOR_TIMEOUT` 同族） |
| `DEFINITIONS_UNSUPPORTED` | **501** | **必须 `Unsupported`**（见 §6.2.1 的硬性断言） |
| **Core 侧框架码**（**`REQUEST_TOO_LARGE`**，413） | **413** | 唯一不以 `ErrorKind` 表达的一条：它由 HTTP 层（body 大小门）在**进入 handler 之前**产生，见 §6.4-① 与 §6.2.1 末行 |

**已知取舍（如实登记）**：把 `UNSUPPORTED_FAMILY`（`ErrorKind::Unsupported`）映射成 **501**、
把 `INVALID_VARIABLE`（`ErrorKind::Address`）映射成 **404**，从「客户端错误分类」的角度略不直观；
但这是**唯一**能同时满足「Core 零 Siemens 标识」与「Core 不解释 NCK 语义」的映射方式。
**前端必须按 body 里的 `code` 分流展示，不得只看 HTTP 状态**（PR-C §6.3 已要求原样透传 `code`+`message`）。

#### 6.2.1 【裁决要求】码 → `ErrorKind` 的**确定性归属表**（每一条错误都必须有确定 kind）

> **裁决原文要点**：**错误响应必须具有确定的 `ErrorKind`**，且**不得把「协议不支持」误报为「设备离线」**。
> 下表是本设计的**规范归属**；实现时**不得**出现「同一个码在不同路径下有不同 kind」的情况。
> 校验方式：矩阵 **I9**（`Unsupported` 断言）+ **I26**（映射函数只含 `ErrorKind` 变体字面量）。

| 码 | `ErrorKind` | HTTP | 归属层 | 为何是这个 kind（判据） |
|---|---|---|---|---|
| `UNKNOWN_VARIABLE` | `Address` | 404 | Driver | 「地址/变量不存在」——**不是**协议不支持，**不是**设备离线 |
| `INVALID_VARIABLE` | `Address` | 404 | Driver | 同上（原生解析失败） |
| `UNSUPPORTED_POINT_KIND` | `Unsupported` | 501 | Driver | 请求的**能力**不存在 |
| `UNSUPPORTED_FAMILY` | `Unsupported` | 501 | Driver | 同上（系列不在声明内） |
| **`DEFINITIONS_UNSUPPORTED`** | **`Unsupported`** | **501** | Manager | **协议/能力不支持** —— **【硬性】严禁 `ConnectionError` / `DeviceError`**（那会被读成「去查机床/查线」，而实际要做的是「升级驱动或换 Core」） |
| `DEFINITION_QUERY_TIMEOUT` | `Timeout`（Manager 措辞）→ HTTP **503** | 503 | Manager | 预算耗尽；**与设备无关**（临时进程 RPC 超时） |
| `DEFINITION_QUERY_TOO_LARGE` | `Internal` | 503 | Manager | 契约/规模违约（驱动返回超出约定上限） |
| **`DEFINITION_QUERY_BUSY`** | **`Internal`** | **503** | Manager | **本机资源预算**耗尽，**与设备在线状态无关** |
| `DRIVER_UNAVAILABLE` | `Connection` | 503 | Manager | **真正的**「驱动进程不可用」——**只有这一条**可以表达「不可达」 |
| `INVALID_QUERY` | `Configuration` | 400 | Driver | 调用方查询形状错 |
| `INVALID_REQUEST` | `Configuration` | 400 | Core（形状层） | 调用方请求形状错 |
| `INVALID_BINDING_CONFIG` | `Configuration` | 400 | Driver | 调用方输入错 |
| `MISSING_TARGET` / `UNSUPPORTED_TARGET` / `INVALID_TARGET` / `INVALID_ARGUMENTS` / `MISSING_IDENTITY_KEY` / `POINT_KEY_MISMATCH` | `Configuration` | 400 | Driver | 均为**用户可修正的输入问题** |
| `UNSUPPORTED_DEFINITION_REVISION` | `Configuration` | 404 | Driver | PR-B §3.5 A3 的既有归属（**保留不改**） |
| `DEFINITION_DIGEST_MISMATCH` | `Internal` | 503 | Driver | 构建期/发布数据问题，**不是**用户输入、**不是**设备 |
| `REQUEST_TOO_LARGE` | `Configuration`（**在 HTTP 层以 413 表达**） | 413 | Core（框架） | 调用方请求过大；**在 handler 之前**由 body 门拒绝，**不经过 `ErrorKind` 映射**（见 §6.4-①） |

> **一句话判据（给实现者的判别口诀）**：
> **「换 Core / 升级驱动能解决」⇒ `Unsupported`（501）**；
> **「改请求能解决」⇒ `Configuration`/`Address`（400/404）**；
> **「查机床 / 查网线能解决」⇒ `Connection`/`Device`（503，且只有 `DRIVER_UNAVAILABLE` 等少数码配得上）**；
> **「Manager 自身预算/规模违约」⇒ `Timeout`/`Internal`（503）**。
> **任何「协议不支持」被写成 `Connection`/`Device` 都视为缺陷**（矩阵 **I9** 会直接判红）。

### 6.3 只读性：可验收证据（**不是口号，逐条可执行**）

| 证据 | 内容 | 验收方式 |
|---|---|---|
| **R1 结构** | 两个端点：`GET`（无 body）+ `POST`（body 是纯查询，**非**写入语义）。POST-for-read 已有两个先例：`/drivers/{id}/validate-connection`（`core-api:3245-3248`）、`/drivers/{id}/probe`（`core-api:3249`） | 路由表 diff 只体现新增；handler 不触碰任何 store 写路径 |
| **R2 代码级** | 实现链**七个模块零设备触点**：`definition.rs`/`targets.rs`/`compiler.rs`/`native.rs`/`value.rs`/`address.rs`/`codec.rs` 对 `NckClient`/`tokio`/`async fn`/`.await`/`session`/`connect(` 匹配数均为 **0**（本次 grep 实测） | CI grep 门（新增，见 §8.4 G7） |
| **R3 进程级** | Manager 走 `temp_op::temp_driver_attempt`（`temp_op.rs:138-166`）⇒ 只 spawn + 握手，**不发 `OpenConnection`**（对照 browse 的 `manager.rs:402-410`） | 契约测试断言驱动进程未收到 `OpenConnection`（矩阵 I8） |
| **R4 会话级** | SDK 分发**不查 `session.entries`**（对照 `GetDescriptor` `driver-sdk:1126-1144` 与 `on_browse` `:1656-1686` 的差别）⇒ 零连接 ⇒ 零设备会话 | 矩阵 I8、I22 |
| **R5 副作用级** | 调用后：`/api/v1/endpoints` 仍为空（对齐 `probe_does_not_create_endpoint`，`management_api.rs:676-699`）；`/api/v1/points/latest` 无新增；`/api/v1/tasks` 不变；Control 审计无新记录 | 矩阵 I7、I23、I24 |
| **R6 禁止的实现方式（显式列出）** | **不得**用 `ConfigureTasks`/`ApplyPointMap`/`StartConnection` 做「dry-run 编译」；**不得**用 `WriteRequest`/`CommandRequest`；**不得**用 `ProbeRequest` 承载（它需要已开连接）；**不得**为了算身份 `open_connection` | 设计禁令 + 矩阵 I21；`driver-sdk:1148-1173` 的写路径不被新 handler 调用 |
| **R7 Control 面零变化** | NCK 的 `controls` 是 `ControlCatalog::default()`（`lib.rs:338`），V2.1 Control 默认 `disabled`，仅 `mesad --enable-control` 时放行；I-2 **不触碰**这两条 | 既有 control 套件原样通过（矩阵 I24） |
| **【新增】R8 不创建 CNC Connection** | 两个端点的实现链**从不**调用 `NckClient::connect`（`client.rs:55`），Manager **从不发** `OpenConnection`（`temp_op.rs:96-125` 只做 spawn + 握手）；驱动侧断言「未收到 `OpenConnection`」，代码侧断言「`client::` 零引用」 | 测试名 **`definitions_rpc_opens_no_cnc_connection`**（矩阵 **I40**）+ G7 grep 门 |
| **【新增】R9 不读取机床实时值** | 端点**不**触碰 `NckClient` 的读值 API（`client.rs:55` 是唯一设备触点，两处使用者为 `identity.rs:434` 与采集路径）；调用前后 `GET /api/v1/points/latest` 无新增 | 测试名 **`definitions_rpc_reads_no_live_machine_values`**（矩阵 **I41**） |
| **【新增】R10 不修改采集计划** | 端点**不**发送 `ConfigureTasks` / `ApplyPointMap` / `StartConnection` / `StopConnection`；`GET /api/v1/tasks` 与 Endpoint `plan` 快照调用前后**逐字不变**；`GET /api/v1/endpoints` 仍为空 | 测试名 **`definitions_rpc_does_not_modify_acquisition_plan`**（矩阵 **I42**） |
| **【新增】R11 无持久化副作用** | 端点**不**写 SQLite（`config-store` / `event-store`）、**不**写 Control 审计、**不**改定义目录文件（`r1.json` / `DIGESTS.lock` 摘要不变）、**不**建缓存文件 | 测试名 **`definitions_rpc_has_no_persistent_side_effects`**（矩阵 **I43**） |

> **R1–R7 与 R8–R11 的关系**：R1–R7 是**第 1 轮的既有证据**，**逐字保留**；
> R8–R11 是本轮【裁决要求】新增的**四条只读性断言**（合并进同一张表，不另开一份），
> 它们把「不创建 CNC 连接 / 不读机床实时值 / 不改采集计划 / 无持久化副作用」从**设计声明**
> 升级为**可执行断言**（每条一个测试名 + 一个矩阵编号）。
> **R5 与 R8–R11 有重叠但不冗余**：R5 说的是「调用后外部可见状态不变」，
> R8–R11 说的是「**实现链上不出现那些动作**」——一个黑盒、一个白盒，**两者都要断言**。

### 6.4 【裁决要求】执行边界（**只读 ≠ 无限量**：请求大小 / 结果数量 / 分页 / 超时 / 并发预算）

> **裁决原文要点**：**即便不连接 CNC，也必须防止管理接口被无限量调用。**
> 原因：I-2 的每一次调用都会**真实 spawn 一个驱动子进程**（`temp_op.rs:96-125`：`DriverProcess::spawn`
> → 建连握手 → RPC → `terminate`），**这是本机资源的真实消耗**，与「是否连机床」无关。
> 因此**「只读」只保证「无副作用」，不保证「无成本」** —— 成本必须被**显式设界**。

#### ① 五项边界（规范取值 + 取值依据）

| # | 边界 | 规范取值 | 取值依据（可核对） | 超限行为 |
|---|---|---|---|---|
| **①-1** | **请求大小上限** | 预览 `POST` body ≤ **64 KiB**；目录 `GET` **无 body**（query string 由框架 URL 长度限制约束，**待确认**具体值） | 合法预览请求的最大形态是 `nck_native` 的 12 字段白名单（`compiler.rs:45-58`）+ 信封，实测量级 **< 2 KiB** ⇒ **64 KiB 给出 ≥ 30× 余量**。**框架已有更宽松的默认门**：axum `Bytes`/`Json` 默认拒绝 > **2 MB**（`axum-core-0.5.6/src/extract/default_body_limit.rs:7-8`，`axum = "0.8"` 见 `Cargo.toml:72`）⇒ 本设计只是**收紧**，不是新增机制 | **413 `REQUEST_TOO_LARGE`**（E30）；在 handler 之前由 body 门拒绝 |
| **①-2** | **结果数量上限** | 单页 `definitions` ≤ **500**；单次响应 JSON ≤ **256 KiB**；**过滤后条目总数 > 4096 视为驱动违约** | 500 = 与 Browse 的 `limit` 上限 **1000**（`core-api:834`）**同量级但更严**（目录是定义面，不是点位面）；256 KiB **逐字复用** descriptor 的既有门 `session.rs:677`（`> 256 * 1024`）与 `manager.rs:57-62` 的第二道门；4096 是「目录条目数量级增长」的硬止损位（今天 **42**，见 §1.1.3） | 500 → 夹取（**不是错误**，与 Browse 同口径）；256 KiB / 4096 → **503 `DEFINITION_QUERY_TOO_LARGE`**（E4） |
| **①-3** | **分页** | `limit` 缺省 **100**、上限 **500**；`cursor` **不透明**（= 上页末条 `variable_id`）；`next_cursor` 无下一页为 `null` | 形态**逐字对齐 Browse**（`BrowseRequest` 的 `cursor`/`limit`，`proto/driver.proto:56`；handler 夹取 `core-api:834`）。**稳定性来源**：「已发布 `(variable_id, revision)` 不可原地修改」（`definition.rs:8-10`）⇒ 按 `variable_id` 字典序遍历**不会因数据变化而跳项/重项** | `limit` 非法 / `cursor` 与 `variable_id` 并存 / 游标不在结果集 ⇒ **400 `INVALID_QUERY`**（E12） |
| **①-4** | **超时** | **RPC 5s**（同 `get_descriptor`：`session.rs:667` 的 `Duration::from_secs(5)`）+ **启动 deadline 6s**（`session.rs:280` 的 `DRIVER_STARTUP_TIMEOUT`）+ **attempt ≤ 2**（`temp_op.rs:25` 的 `MAX_TEMP_ATTEMPTS`）⇒ **单次调用最坏上界 ≤ 2 × (6s + 5s) = 22s** | **全部复用**既有常量，**不新增第二套超时栈**。对照 Probe 的独立预算法（`probe.rs:62-94` 的 `ProbeBudgets`）——I-2 **不**需要它，因为 I-2 只有**一个** RPC 阶段（不像 Probe 有 4 个串行阶段） | RPC 超时 ⇒ **503 `DEFINITION_QUERY_TIMEOUT`**（E3）；启动超时 ⇒ **503 `DRIVER_UNAVAILABLE`**（E2） |
| **①-5** | **并发预算** | 同一 `driver_id` 同时进行的定义查询 ≤ **4**（in-flight 临时进程数）；超出 ⇒ **立即拒绝**（不排队、不等待） | **如实说明：仓库当前没有任何并发闸门可复用** —— 本次全仓只读抽查 `crates/*/src/*.rs`、`apps/mesad/src/*.rs` 对 `Semaphore` / `acquire(` / `rate_limit` / `concurrency` **零命中**；`temp_op.rs` 也**没有**全局 in-flight 计数。⇒ 这是**唯一必须新增**的资源控制机制，取 **4** 的理由：① 每次调用 = 1 个驱动进程 + 1 个 TCP 监听端口，**4** 与「桌面/工控机的管理面并发人手操作数」相当；② 与 `MAX_TEMP_ATTEMPTS = 2` 相乘，最坏瞬时进程数 = 8，**远低于**任何进程数风险阈值；③ **不排队**（拒绝而非等待）以保证**响应时间可预测** —— 排队会让 §①-4 的 22s 上界失效 | **503 `DEFINITION_QUERY_BUSY`**（E31）；消息含当前 in-flight 数 + 预算 |

#### ② 复用了哪些既有 Manager 资源控制机制（`文件:行号` + 为什么它是合适载体）

> **裁决原文要点**：**复用现有 Manager 的资源控制机制**。下表**逐条给出载体的行号**，
> 并**明确区分**「已复用的既有机制」与「必须新增的机制」—— **不把不存在的东西写成存在**。

| 机制 | 位置（`文件:行号`） | 现值 | 为什么它是**合适载体** | 本设计如何复用 |
|---|---|---|---|---|
| **临时进程 attempt 预算** | `crates/driver-manager/src/temp_op.rs:25` | `MAX_TEMP_ATTEMPTS = 2` | 它就是 I-2 调用的**执行单元**：I-2 与 descriptor/probe/browse **共用同一个原语**，因此它的预算天然就是 I-2 的预算，**不需要**另立一套 | **直接继承**（不改） |
| **启动 deadline** | `crates/driver-manager/src/session.rs:280` | `DRIVER_STARTUP_TIMEOUT = 6s` | 约束的是「spawn + Hello/Welcome」这一段，对 I-2 与 descriptor **完全同构** | **直接继承** |
| **descriptor 的 5s RPC 超时 + 256 KiB 大小门** | `crates/driver-manager/src/session.rs:654-677`（超时 `:667`、门 `:677`） | 5s / 256 KiB | 它是**driver 作用域、无连接、单 RPC、JSON 透传**的**最近同构先例**；I-2 的两个 RPC 与它**逐字同形态**（§2.4）⇒ 复用同一常量与同一门是最小改动、最不易漂移的选择 | **形态复用**（新增 2 个方法，超时/门**取值相同**） |
| **descriptor 的第二道大小门（Manager 侧）** | `crates/driver-manager/src/manager.rs:57-62` | `json.len() > 256 * 1024` ⇒ `DRIVER_DESCRIPTOR_TOO_LARGE` | 说明「门设在 Manager 而非仅靠 Session」是**既有惯例**；I-2 照做（E4） | **形态复用** |
| **通用 RPC 超时** | `crates/driver-manager/src/session.rs:25`（`REQUEST_TIMEOUT = 10s`）、`:902`（`call_with_timeout`） | 10s | 是**其余**管理面 RPC 的缺省；I-2 **不调大它**（`session.rs:892-895` 明写「不得为此调大全局值」） | **显式不改**（I-2 用 5s 专用预算，不动全局） |
| **帧长度上限** | `crates/driver-protocol/src/lib.rs:52` | `MAX_FRAME_LEN = 4 MiB` | 传输层的**最后一道**物理门（比 256 KiB 更宽松，只用于兜底「不至于把进程打爆」） | **直接继承**（不改） |
| **响应条目分页的既有先例** | `crates/core-api/src/lib.rs:834`（Browse：`unwrap_or(50).min(1000)`） | 缺省 50 / 上限 1000 | 它是**同一 API 面上**唯一既有的分页实现；照它的形状做，前端与测试**无需学第二套分页语义** | **形态复用**（I-2 取缺省 100 / 上限 500） |
| **HTTP body 默认门** | `axum-core-0.5.6/src/extract/default_body_limit.rs:7-8`（框架级，`axum = "0.8"`：`Cargo.toml:72`） | 2 MB | 它是**已经在生效**的第一道门（`Json` extractor 内建）⇒ I-2 只需**收紧到 64 KiB**，机制零新增 | **收紧**（`DefaultBodyLimit::max(64 * 1024)`） |
| **并发预算** | **不存在**（本次只读抽查：`crates/*/src/*.rs` + `apps/mesad/src/*.rs` 对 `Semaphore`/`rate_limit`/`concurrency` **零命中**） | — | — | **【必须新增】**：I-2 自带一个 per-driver in-flight 计数（预算 **4**，§①-5）。**如实登记为新增项，不冒充复用** |

> **⇒ 一句话**：I-2 **复用**了 **8 项**既有机制（表中 8 行）——「临时进程 attempt 预算 / 启动 deadline /
> descriptor 的 5s 单 RPC 超时 / 256 KiB 大小门（Session 侧）/ 256 KiB 第二道门（Manager 侧）/
> 4 MiB 帧门 / Browse 分页形状 / axum body 默认门」；
> **只新增 1 项**（per-driver 并发预算）。**没有为了 I-2 发明第二套超时或第二套大小门。**
> （另有一行 `REQUEST_TIMEOUT = 10s` 属「**显式不改**」项，不计入复用计数。）

#### ③ 四条只读性断言（**与 R1–R7 合并，不另开一份**）

> 逐条给出**测试名**；与 §6.3 的 R8–R11 **一一对应**，矩阵编号 **I40–I43**。

| # | 断言（必须成立） | 测试名 | 断言要点 | 对应 R |
|---|---|---|---|---|
| **A1** | **目录与 Preview 均不得创建 CNC Connection** | `definitions_rpc_opens_no_cnc_connection` | ① 驱动进程侧：未收到 `OpenConnection`（对照 `manager.rs:402-410` 的 browse 路径）；② 代码侧：`definition_api.rs` + 七个模块对 `client::` / `NckClient` / `connect(` **零命中**；③ `/api/v1/endpoints` 调用前后**仍为空** | **R8** |
| **A2** | **目录与 Preview 均不得读取机床实时值** | `definitions_rpc_reads_no_live_machine_values` | ① `NckClient` 的读值 API 零调用（静态：`client.rs:55` 是唯一触点，I-2 链不引用）；② `GET /api/v1/points/latest` 调用前后**无新增**；③ 无 `DataBatchMsg` 产生 | **R9** |
| **A3** | **目录与 Preview 均不得修改采集计划** | `definitions_rpc_does_not_modify_acquisition_plan` | ① 未发送 `ConfigureTasks` / `ApplyPointMap` / `StartConnection` / `StopConnection`；② `GET /api/v1/tasks` 快照**逐字不变**；③ Endpoint 的 `plan` / `stream_epoch` 不变 | **R10** |
| **A4** | **目录与 Preview 均不得产生持久化副作用** | `definitions_rpc_has_no_persistent_side_effects` | ① `config-store` / `event-store` 的 SQLite 文件**摘要不变**；② Control 审计无新记录；③ `r1.json` / `DIGESTS.lock` **摘要不变**；④ 不创建任何新文件 | **R11** |

> **【裁决要求的落点】**：以上四条覆盖「创建 CNC Connection / 读取机床实时值 / 修改采集计划 / 持久化副作用」
> 四类禁止行为，**逐条可验收**。**这也是「`POST` 不等于写入」的最终判据**
> —— 判据不是动词，而是**这四条断言**。

---

## 7. 问题 5：兼容性 —— 逐条说明「如何保证」

### 7.1 Core 不解释 NCK 语义（只逐字透传）

| 保证措施 | 内容 |
|---|---|
| **数据形态** | 两个端点的新增字段一律以**不透明 JSON** 过 Core：目录 = `definitions_json` 字符串；预览 = `request_json` / `result_json` 字符串。Core 只做长度/形状校验与转发 |
| **状态映射** | Core 只按 `ErrorKind` 映射 HTTP（§6.2），**不出现任何 NCK 码字面量** |
| **可执行护栏（既有）** | `core_has_zero_siemens_knowledge`（`resource_contract.rs:396-461`）扫描 `crates/core-types/src` 与 `crates/core-api/src` 的**生产代码**，禁止词表含 `VariableDefinition`/`DefinitionCatalog`/`AddressCompiler`/`canonical_identity`/`identity_template`/`identity_mode`/`UNKNOWN_VARIABLE`/`UNSUPPORTED_DEFINITION_REVISION`/`MISSING_TARGET`/`INVALID_TARGET`/`INVALID_ARGUMENTS`/`POINT_KEY_MISMATCH`/`nck_variable`/`nck_native`/`drive_object` … |
| **路由命名安全** | 新增路由/handler 的标识符**不含**上述任何 token（本次自查：`definitions`、`identity-preview`、`preview_point_identity` 均不匹配词表任一项；词表匹配是**大小写敏感的 `contains`**，`DefinitionCatalog` 不会被 `definitions` 命中） |
| **矩阵** | **I25**（P9 护栏原样通过）、**I26**（映射表只含 `ErrorKind` 变体） |

### 7.2 既有通信协议 / `proto/**` / `crates/driver-protocol/**`

| 项 | 保证 |
|---|---|
| **既有消息** | `proto/driver.proto` 的 `oneof body`（起始 `:22`）**既有 35 个 tag（编号 10–44，连续无空洞）的编号、类型、语义逐字不动**；不重排、不复用、不删除。全部既有 `message` 定义逐字不动。**逐字段核对结果见 §7.2.1** |
| **`PROTOCOL_MAJOR`** | 保持 **1**（`crates/driver-protocol/src/lib.rs:23`）；既有 `negotiate()` 语义（Major 必须相等、Minor 取较小值，`:100`）逐字不动 |
| **`PROTOCOL_MINOR`** | **6 → 7**（`:29`）。这是**唯一的版本号变化**，且是 additive 增补的必然结果 |
| **新增内容** | 新 tag **45–48**（4 个）+ 4 个新 `message`；新门控常量 `DEFINITIONS_RPC_MIN_MINOR = 7`（与既有 `PROBE_RPC_MIN_MINOR = 2`（`:34`）、`EVENT_PLANE_MIN_MINOR = 3`（`:39`）、`TASK_SCHEDULE_MIN_MINOR = 5`（`:44`）、`CONTROL_MODEL_MIN_MINOR = 6`（`:49`）**同一机制**） |
| **旧 Core ↔ 新 Driver** | 协商取 min ⇒ 旧 Core 得 `6`，不会发新消息（`negotiate` `:100`） |
| **新 Core ↔ 旧 Driver 二进制** | 协商得 `6` ⇒ **Core 必须先门控再发帧**（E5），否则旧 SDK 的兜底分支会**静默忽略**未知消息（`driver-sdk:1178-1179`）⇒ 调用方白等一个 timeout |
| **新 Core ↔ 用新 SDK 重建但未实现该 RPC 的其它驱动** | 协商得 `7`，门控放行 ⇒ SDK **默认实现**返回 `Unsupported` ⇒ **快速、精确**的 501（E6），**不是超时** |
| **换行/生命周期影响** | 无：新消息只走**临时进程**（`temp_op`），不进入采集/事件/控制任一面；不与 `start_epoch`、事件 barrier、Control 可靠队列交互 |
| **回滚方案** | ① 路由层：删除 2 条 Core 路由 ⇒ 端点消失，其余零影响；② 版本层：`PROTOCOL_MINOR` 退回 6、删除 4 个新 tag（protobuf 允许删除未发布的 tag）；③ 驱动层：SDK 默认实现替换为删除方法（编译期暴露所有调用点）；④ **无持久化面 ⇒ 回滚不需要数据迁移** |
| **形态合规** | 按 PR-B §7.2（`:936`）「确需改动 wire ⇒ **必须作为独立的「契约变更」PR**」；本文件即该 PR 的边界文档。影响面清单见 §7.6 |
| **是否触碰 PR-B 的「不得触碰」声明** | **触碰**（PR-B §2.4 `:215` 把 `crates/driver-protocol/**`、`proto/**`、`crates/driver-sdk/**` 列为 PR-B 的【不得触碰】）。**但那是 PR-B 的自身范围约束**；PR-B §7.2 `:936` 同时给出了「确需改动时的合规路径」。⇒ 【已裁决】**已获原则批准**（仍以独立「契约变更」PR 形态提交）；**生产代码仍 HOLD**（§11-1） |

### 7.2.1 【修订 1】既有 `oneof body` 的**逐字段核对结果**（**以实际 Protobuf 声明为准**）

> **【已裁决修正】第 1 轮的错误**：本文件第 1 轮（以及 §1.3-③、§3.1-④、§3.2、§7.2、§8.1.2-M1、§8.3-I27、§13-4）
> 把 `oneof body` 的既有 tag 数写成 **「44 个」**。**该计数错误**：
> **`10–44` 是 35 个编号，不是 44 个**；「44」是**最大 tag 编号**（末项 `EventBatchMsg event_batch = 44;`），
> **不是成员个数**。本轮**不采用「只把 44 改成 35」的做法** —— 下表给出**逐字段核对**的完整结果。

**核对方法（只读，可复现）**：
① 读 `proto/driver.proto`，定位 `oneof body {`（`:22`）到其闭合 `}`（`:77`）的区间；
② 逐行用 `^\s*<Type>\s+<name>\s*=\s*(\d+)\s*;` 抽取 `(tag, 类型, 名称, 行号)`；
③ 统计成员数、tag 最小值/最大值、编号是否连续无空洞、是否有重复编号；
④ 另行统计**顶层 `message` 数**与**全文件 `oneof` 数**作为旁证。

**核对结论（本次实测）**：

| 指标 | 实测值 | 说明 |
|---|---|---|
| `oneof body` 成员数 | **35** | 与「10–44」的编号个数一致（44 − 10 + 1 = 35） |
| tag 最小值 / 最大值 | **10 / 44** | 末项是 `event_batch = 44`（`:76`） |
| 编号集合 | `{10,11,…,44}` | **连续、无空洞**（实测「是否 10..44 连续无空洞 = True」） |
| 重复 tag 数 | **0** | 无重号 |
| 顶层 `message` 数 | **56** | 含 `message Envelope`；**本次增补后为 60**（+4） |
| 全文件 `oneof` 数 | **2** | `oneof body`（`Envelope`，`:22`，35 成员）+ `oneof kind`（`ValueMsg`，`:261`，**19 成员**） |

**逐字段核对表（35 行，全部实测；`文件:行号` 为 `proto/driver.proto`）**：

| # | tag | 成员类型 | 成员名 | 行号 | 第 1 轮的「44 个」表述是否受影响 |
|---|---|---|---|---|---|
| 1 | 10 | `Hello` | `hello` | **:23** | 是（计数） |
| 2 | 11 | `Welcome` | `welcome` | **:24** | 是（计数） |
| 3 | 12 | `GetMetadata` | `get_metadata` | **:25** | 是（计数） |
| 4 | 13 | `MetadataReport` | `metadata_report` | **:26** | 是（计数） |
| 5 | 14 | `OpenConnection` | `open_connection` | **:28** | 是（计数） |
| 6 | 15 | `OpenConnectionAck` | `open_connection_ack` | **:29** | 是（计数） |
| 7 | 16 | `CloseConnection` | `close_connection` | **:30** | 是（计数） |
| 8 | 17 | `CloseConnectionAck` | `close_connection_ack` | **:31** | 是（计数） |
| 9 | 18 | `ConfigureTasks` | `configure_tasks` | **:33** | 是（计数） |
| 10 | 19 | `PointDescriptorsReport` | `point_descriptors` | **:34** | 是（计数） |
| 11 | 20 | `ApplyPointMap` | `apply_point_map` | **:35** | 是（计数） |
| 12 | 21 | `ConfigApplied` | `config_applied` | **:36** | 是（计数） |
| 13 | 22 | `StartConnection` | `start_connection` | **:38** | 是（计数） |
| 14 | 23 | `StartConnectionAck` | `start_connection_ack` | **:39** | 是（计数） |
| 15 | 24 | `StopConnection` | `stop_connection` | **:40** | 是（计数） |
| 16 | 25 | `StopConnectionAck` | `stop_connection_ack` | **:41** | 是（计数） |
| 17 | 26 | `ConnectionStateChanged` | `connection_state_changed` | **:43** | 是（计数） |
| 18 | 27 | `DataBatchMsg` | `data_batch` | **:44** | 是（计数） |
| 19 | 28 | `DriverErrorReport` | `driver_error` | **:45** | 是（计数） |
| 20 | 29 | `Ping` | `ping` | **:47** | 是（计数） |
| 21 | 30 | `Pong` | `pong` | **:48** | 是（计数） |
| 22 | 31 | `Shutdown` | `shutdown` | **:49** | 是（计数） |
| 23 | 32 | `GetDescriptor` | `get_descriptor` | **:52** | 是（计数） |
| 24 | 33 | `DescriptorReport` | `descriptor_report` | **:53** | 是（计数） |
| 25 | 34 | `BrowseRequest` | `browse_request` | **:56** | 是（计数） |
| 26 | 35 | `BrowseResponse` | `browse_response` | **:57** | 是（计数） |
| 27 | 36 | `WriteRequest` | `write_request` | **:60** | 是（计数） |
| 28 | 37 | `WriteResponse` | `write_response` | **:61** | 是（计数） |
| 29 | 38 | `CommandRequest` | `command_request` | **:62** | 是（计数） |
| 30 | 39 | `CommandResponse` | `command_response` | **:63** | 是（计数） |
| 31 | 40 | `ProbeRequest` | `probe_request` | **:67** | 是（计数） |
| 32 | 41 | `ProbeResponse` | `probe_response` | **:68** | 是（计数） |
| 33 | 42 | `ConfigureEventTasks` | `configure_event_tasks` | **:74** | 是（计数） |
| 34 | 43 | `EventConfigApplied` | `event_config_applied` | **:75** | 是（计数） |
| 35 | 44 | `EventBatchMsg` | `event_batch` | **:76** | 是（计数） |

**⇒ 三项核对结论**：
1. **既有 tag 数 = 35**（不是 44）。本文件所有「44 个 tag」的表述**已逐处改为 35**，
   并**保留一处「44」的合法用法** —— 仅当指「**最大 tag 编号**」或「**末项** `= 44`」时（如 §8.1.2-M1 的 `event_batch = 44;`）。
2. **增补后**：`oneof body` 成员 **35 → 39**（+4：tag 45–48）；顶层 `message` **56 → 60**（+4）。
   **新增 tag 必须从 45 起**（`10–44` 已连续占满，无空洞可复用）。
3. **I27 的断言口径据此升级**（§8.3）：不再断言「44 个」这一**数字**，而是断言
   **「§7.2.1 的 35 行逐字段核对表（tag → 类型 → 名称）在增补前后逐字不变，且新 tag 集合恰为 {45,46,47,48}」**
   —— 数字是**派生物**，**逐字段表才是真值**。

> **本轮修正涉及的文档位置（全部已改）**：§1.3-③（原 `:196-197`）、§3.1-④（原 `:356`）、
> §3.2 IPC 行（原 `:382`）、§7.2「既有消息」行（原 `:846`）、§8.1.2-M1（原 `:918`）、
> §8.3-I27（原 `:1005`）、§13-4（原 `:1201`）；另在 §0/§1.3-③/§7.2 新增**指向本小节**的引用。

### 7.3 62 条 wire golden 与 G-B 身份编码 golden

| Golden 组 | 是否受影响 | 保证方式 |
|---|---|---|
| **G-A 既有 wire golden**：`drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json`（**62 条**，本次实测 `entries=62`） | **零影响** | 它是 **NCK 业务帧**（`encode_var_spec` 的 10 字节布局）真值，与 IPC 帧无任何关系。I-2 不改 `codec.rs`/`native.rs`/`compiler.rs` 的地址构造（`M10` 只拆「重算身份 vs D11 比对」两步，不触碰 wire 构造）。既有门 `tests/legacy_equivalence.rs` 与 `tests/semantic_equivalence.rs` 仍必须 `checked == 62` |
| **G-B 身份编码 golden**：`tests/fixtures/nck-point-key-golden.json`（`value_cases=55`/`value_reject_cases=5`/`key_cases=77`，本次实测） | **零影响** | I-2 **不新增第二份**（PR-B `:654` 的「同一份复用」）；预览复用的是**同一个** `encode_typed_value`（`point.rs:207`），因此不可能漂移。既有 grep 断言（命中集合恰为 {Core, Driver, 前端}、无 `*copy*.json`）继续成立 |
| **`presets/828d.json`（62 模板）** | **零影响**（I-2b）；**I-2a 也不改它** | I-2a 只改**浏览叶载荷的构造**（`presets.rs:242-275` 邻域），**不动模板文件**（PR-B C2=P1 的 `:193` 冻结） |

### 7.4 其它驱动与已冻结的业务契约

| 对象 | 保证 |
|---|---|
| **S7 / FOCAS2 / OPC UA / test-driver** | SDK 的两个新方法**带默认实现**（返回 `Unsupported`）⇒ 这 4 个驱动的**源码零改动、行为零变化**；契约测试仍要求它们全过既有 29 套（`scripts/contract_suites.py`） |
| **Descriptor 契约** | **`contract_major/minor` 保持 2/0**（`crates/core-types/src/descriptor.rs:12-13`）；`DriverDescriptor` 结构体**零改动**（明确否决了 C3 路线，理由见 §1.2）。`tests/driver-contract/tests/descriptor_contract.rs` **整文件逐字不动**（PR-B C10=D-A，`:196`） |
| **`nck.points.v1` / `PointSelection` / `PointBinding` / `point_key` 通用格式 / Endpoint-wide 唯一性** | **零改动**：`crates/core-types/src/point.rs` 与 `crates/core-api/src/binding_gate.rs` **逐字不动**（PR-B `:200` 声明为 PR-A 独占冻结） |
| **`PointSelection` 的三字段形态** | 预览**不新增任何字段到 `PointSelection`**：它只是**算** `point_key`，返回值由前端按其既有形态写入（`point.rs:100-112` 的 `deny_unknown_fields` 得到保持） |
| **`scope=identity` 的既有产品语义** | 不变（`definition.rs:51-57`）；I-2 **不**把 `scope` 校验下沉到 Configure（§9） |
| **PR-C / PR-D 文件边界** | 不变（§10.1） |
| **`DIGESTS.lock` / `r1.json`** | **一个字节都不改** ⇒ CI 的 `python3 tools/ci/nck_definition_lock.py --base "$BASE"`（`.github/workflows/_rust-quality.yml`）**仍通过** |

### 7.5 版本号同步项（须与 wire 增补同一 PR）

> **【已裁决修正】（重要，纠正第 1 轮的两处错误结论）**：
> ① 第 1 轮称 `driver.toml` 的 `protocol_minor` 「**未被任何门使用**」⇒ **不准确**。
> 实测存在一条**P1 invariant 单测**：`crates/driver-protocol/src/lib.rs:679-714`
> 的 `in_tree_manifest_minor_matches_protocol`，它**遍历 5 个 in-tree `driver.toml`**
> （`drivers/s7`、`drivers/opcua`、`drivers/focas2`、`drivers/sinumerik-nck`、`tests/support/test-driver`，
> 见 `:690-696`）并断言每个 `protocol_minor` **必须等于** `PROTOCOL_MINOR`（`:709-712`）。
> ⇒ **一旦 `PROTOCOL_MINOR` 提到 7 而任一 `driver.toml` 未同步，该单测直接失败。**
> 因此 §8.1.2 的 **M8 不是「可选项」，而是必需项，且是 5 个文件、不是 1 个**。
> （「发现期只校验 `protocol_major`」这一点**仍然成立** —— `manifest.rs:95-103` 确实只比 `protocol_major`；
> 但「**运行期不参与 gating**」≠「**没有任何门使用**」。）
> ② 【已裁决】**`driver.toml` 的 Minor 可同步，但不得把 Manifest 字段当成实际能力协商的证明。**
> 实际协商值只来自 **SDK 常量**（`driver-sdk:817-818` 的 `Hello.protocol_minor = PROTOCOL_MINOR`）
> 与 **`Welcome.accepted_protocol_minor`**（`proto/driver.proto:97`）；`driver.toml` 的字段是**声明/展示值**，
> **不是能力标志**。**任何**「读 manifest minor 来判断该驱动是否实现 I-2 RPC」的实现方式**一律禁止**。

| 文件 | 现值（本次实测） | 需要 | 依据 |
|---|---|---|---|
| `crates/driver-protocol/src/lib.rs:29` | `PROTOCOL_MINOR = 6` | `7` | additive 增补 |
| `drivers/sinumerik-nck/driver.toml` | `protocol_minor = 6` | **`7`（必需，非可选）** | §8.1.2-M8；受 `driver-protocol/src/lib.rs:679-714` 的 P1 invariant 单测强制 |
| `drivers/s7/driver.toml` | `protocol_minor = 6` | **`7`（必需）** | 同上（`:690-696` 的遍历清单含它） |
| `drivers/focas2/driver.toml` | `protocol_minor = 6` | **`7`（必需）** | 同上 |
| `drivers/opcua/driver.toml` | `protocol_minor = 6` | **`7`（必需）** | 同上 |
| `tests/support/test-driver/driver.toml` | `protocol_minor = 6` | **`7`（必需）** | 同上（`:685-686` 注释明写 test-driver 的 manifest minor 不允许漂移） |

> **⇒ 新增一个必须同步的**契约**断言**：`in_tree_manifest_minor_matches_protocol`（既有，`driver-protocol:683`）
> 就是该同步项的**可执行门**；它**不需要新写**，但**必须被列为本 PR 的必过项**（§8.4-G5）。

### 7.6 影响面清单（PR-B §7.2 `:936` 要求逐条给出）

| 要求项 | 内容 |
|---|---|
| 影响的其它驱动 | **4 个**（`drivers/s7`、`drivers/focas2`、`drivers/opcua`、`tests/support/test-driver`）：**源码零改动**（SDK 默认实现）；**但各自的 `driver.toml` 必须同步 `protocol_minor = 7`**（§7.5，由 `driver-protocol:679-714` 的 P1 invariant 单测强制）。**【已裁决修正】**：第 1 轮把「其它 4 个驱动**源码零改动**」写成「**均不需改**」是不准确的 —— **源码零改动成立，配置（`driver.toml`）必须同步**。行为面仍无变化（协商取 min 且既有 4 个 minor 门都是 `>=` 语义） |
| 需同步改写的契约套件 | **不新建套件**（`scripts/contract_suites.py:8` 明写「增删 suite 必须显式评审本文件」）⇒ 断言**新增在既有 `management_api` 套件内**；另外 `protocol_negotiation` 套件需**新增**（不改既有）断言：I10（新消息对旧 Minor 的门控）、I27（逐字段核对表不变）、I34（旧 Manager ↔ 新驱动降级）、I35（未知消息不异常退出）。**29 套名单不变**（`contract_suites.py:52`） |
| `ProtocolManifest` / 握手影响面 | `Hello.protocol_minor` 由 SDK 常量驱动（`driver-sdk:817-818`）；`Welcome.accepted_protocol_minor` 为协商结果（`proto/driver.proto:97`）；`driver.toml` 的 `protocol_minor` **不参与发现期 gating**（`manifest.rs:95-103` 只比 `protocol_major`），**但受 P1 invariant 单测强制同步**（`driver-protocol:679-714`）⇒ 握手面**结构零变化**，仅版本数值 +1 |
| 回滚方案 | 见 §7.2 |

### 7.7 【修订 1】版本兼容矩阵（**组合 → 期望行为 → 返回码 / `ErrorKind` → 可验收测试**）

> **【裁决要求】**至少覆盖五类：① 新 SDK 默认实现必须让不支持 I-2 的驱动**快速**返回明确的 `Unsupported`，
> **而不是等待超时**；② **新 Manager ↔ 旧驱动**的版本能力协商；③ **旧 Manager ↔ 新驱动**的**降级行为**；
> ④ **未知消息类型不得导致进程异常退出**；⑤ **错误响应必须具有确定的 `ErrorKind`**，
> 且**不得把「协议不支持」误报为「设备离线」**。
>
> **协商基础**：`PROTOCOL_MAJOR` 必须相等、`PROTOCOL_MINOR` 取双方较小值（`crates/driver-protocol/src/lib.rs:100`）；
> 新门控常量 `DEFINITIONS_RPC_MIN_MINOR = 7`（与既有 `PROBE_RPC_MIN_MINOR = 2`（`:34`）、
> `EVENT_PLANE_MIN_MINOR = 3`（`:39`）、`TASK_SCHEDULE_MIN_MINOR = 5`（`:44`）、
> `CONTROL_MODEL_MIN_MINOR = 6`（`:49`）**同一机制**）。

| # | 组合 | 期望行为 | 返回码 / `ErrorKind` | 可验收测试 |
|---|---|---|---|---|
| **V1** | **新 Manager ↔ 新 SDK 但该驱动未实现 I-2 RPC**（如 S7 / FOCAS2 / OPC UA / test-driver 用新 SDK 重建） | 协商得 **7**，门控**放行**；SDK 的**默认实现**（`Driver` trait 新增的两个**带默认实现**方法，与 `descriptor()` 默认实现 `driver-sdk:105-123` 同一模式）**立即**返回 `Unsupported` ⇒ **快速、精确**的 501，**不等待超时** | **501** `DEFINITIONS_UNSUPPORTED` / **`Unsupported`** | **I9**（`definitions_unsupported_driver_returns_501_not_timeout`，**本轮强化**：增断言 `error.kind == "Unsupported"` 且**耗时显著早于**超时预算） |
| **V2** | **新 Manager ↔ 旧驱动二进制**（`negotiated_minor = 6 < 7`） | 协商得 **6**；Manager **必须先门控再发帧**（同 `probe.rs:149-151` 的 `probe_supported` 口径）⇒ **不发帧**，直接 501。**若发帧**，旧 SDK 的兜底分支会**静默忽略**未知消息（`driver-sdk:1178-1179`）⇒ 调用方**白等一个 timeout** | **501** `DEFINITIONS_UNSUPPORTED` / **`Unsupported`**（**不是** `Timeout`，**不是** `Connection`） | **I10**（`definitions_stale_binary_is_gated_before_send`，既有） |
| **V3** | **旧 Manager ↔ 新驱动二进制** | 旧 Manager 的 `PROTOCOL_MINOR = 6` ⇒ 协商取 min = **6**；旧 Manager **根本不知道** tag 45–48 存在 ⇒ **永不发新帧**；新驱动只需在旧消息集上继续工作（**纯 additive 的必然结果**）⇒ **降级是静默且安全的**（无错误、无告警、无行为变化） | **无新错误**（旧 Manager 对本端点无路由 ⇒ 调用方拿到 **404**，属**预期**） | **I34**（`legacy_manager_new_driver_downgrade_is_silent`，**本轮新增**） |
| **V4** | **任意一端收到未知消息类型 / 未知 oneof 变体** | **不得导致进程异常退出**。实测既有行为：SDK 的 `accept()` 兜底分支是 `other => tracing::debug!(?other, "unexpected envelope ignored")`（`driver-sdk:1178-1179`）—— **只记日志并忽略，不 panic、不断连接**；因此 `PROTOCOL_MAJOR` 不变时的 additive 增补**结构安全** | **无进程级错误**（进程存活、连接存活）；**唯一风险**是「发帧方干等到 timeout」，由 **V2 的门控**兜住 | **I35**（`unknown_envelope_variant_never_aborts_driver_process`，**本轮新增**：注入未知 tag 后断言驱动进程仍存活且后续 RPC 正常） |
| **V5** | **任意错误路径**（含 V1–V3） | 每条错误响应**必须**带确定的 `ErrorKind`（§6.2.1 的归属表）；**「协议不支持」必须 `Unsupported`（501）**，**严禁** `ConnectionError`/`DeviceError`（503「设备离线」语义）—— 两者对运维的含义**完全相反**（「升级驱动 / 换 Core」vs「查线 / 查机床」） | 归属表见 **§6.2.1** | **I9**（`Unsupported` 断言）+ **I26**（映射函数只含 `ErrorKind` 变体字面量）+ **§6.2.1 全表** |
| **V6** | **新 Manager ↔ 新驱动**（正常路径） | 协商得 **7**，门控放行；两个新 RPC 正常应答（JSON 透传）；**既有 35 个 tag 与全部既有 message 逐字不变** | **200**（正常）；错误见 §6.1 | **I27**（**本轮修正口径**：断言 §7.2.1 的 **35 行逐字段核对表**逐字不变 + 新 tag 集合恰为 `{45,46,47,48}`，**不再断言「44」这个数字**）+ **I28** |

> **⇒ 矩阵的六行覆盖了裁决点名的五类**：V1（① 快速 `Unsupported`）、V2（② 新 Manager ↔ 旧驱动协商）、
> V3（③ 旧 Manager ↔ 新驱动降级）、V4（④ 未知消息不异常退出）、V5（⑤ `ErrorKind` 确定性 + 不误报设备离线），
> V6 是正常路径对照。**每行都有可验收测试编号**，其中 **I9 与 I27 为本轮强化/修正口径**，
> **I34 与 I35 为本轮新增**。

---

## 8. 问题 6：验收与归属

### 8.1 具体文件范围（新增 / 修改 / 冻结）

> 判定口径沿用 PR-B §2：**新增** = 本次新建；**修改** = 语义/形状变更（给行号区间）；
> **冻结** = 本次**不得**改动（只允许「附加可选属性」式演进，**不得**改语义）。

#### 8.1.1 新增（**3** 项）

| # | 路径 | 职责 | 关键约束 |
|---|---|---|---|
| **N1** | `drivers/sinumerik-nck/src/definition_api.rs` | I-2b 的 Driver 侧实现：目录投影（白名单字段）+ 权威预览（调用编译链前半段） | **只允许**依赖 `definition.rs` / `targets.rs` / `compiler.rs` / `native.rs`；**禁止** `use crate::client`；文件内**零** `async`/`.await`/socket |
| **N2** | `drivers/sinumerik-nck/tests/definition_api.rs` | 目录/预览单测；**「预览 == D11 重算」逐条 42 条断言**（I12）；**【本轮新增】** 跨接口一致性（**I32**，42 条 × Preview→Configure 全链）、定义变更/Revision 不支持（**I33**）、**执行边界 I36–I39**、**`scope=identity` 边界 I44/I45**；只读链 grep 断言 | 新测试目标；**不改**既有 `tests/*.rs` |
| **N3** | `crates/core-api/src/driver_query.rs` | Core 侧**驱动无关**的只读查询 handler（2 个）+ `ErrorKind → StatusCode` 映射 | **零 Siemens 标识**（P9 扫描）；只做形状校验 + 逐字透传 |

> **本文件自身**（`docs/design/nck-i2-definition-catalog-api-boundary.md`）是本次唯一的新建文件，
> 属**文档交付**，不计入 I-2 的代码文件范围。

#### 8.1.2 修改（**11** 项，含行号区间）

| # | 路径 | 改动点（行号区间，基于本次实测） | 说明 |
|---|---|---|---|
| **M1** | `proto/driver.proto` | 在 `oneof body`（`:22`）内、末项 `EventBatchMsg event_batch = 44;`（`:76`）之后追加 **tag 45–48**；文件末尾追加 4 个 `message` | **既有 35 个 tag 一字不动**（【已裁决修正】：第 1 轮误作「44 个」；逐字段核对见 §7.2.1）。**注意**：本轮**未修改** `proto/driver.proto`（增补已获原则批准，生产代码仍 HOLD） |
| **M2** | `crates/driver-protocol/src/lib.rs` | `:29` `PROTOCOL_MINOR: u32 = 6` → `7`；`:34` 邻域新增 `DEFINITIONS_RPC_MIN_MINOR: u32 = 7` | 与既有 4 个 `*_MIN_MINOR` 常量同族；**连带强制** M8（`:679-714` 的 P1 invariant 单测） |
| **M3** | `crates/driver-sdk/src/lib.rs` | `Driver` trait（`:100-132`）**末尾新增 2 个带默认实现的方法**；控制面 `match`（`:1126` 的 `GetDescriptor` 分支 / `:1169` 的 `ProbeRequest` 分支**邻域**）新增 2 个分支 | **兜底分支 `:1178-1179` 不动**（它正是「未知消息不异常退出」的实现，§7.7-V4）；`descriptor()` 默认实现 `:105-123` **不动** |
| **M4** | `crates/driver-manager/src/session.rs` | `:654-690`（`get_descriptor`）邻域新增 2 个 RPC 方法（同形态：5s 独立超时 + 256 KiB 大小门） | `:667` 的 5s 超时与 `:677` 的 256 KiB 门**形态复用**（§6.4-②）；既有方法**不动** |
| **M5** | `crates/driver-manager/src/manager.rs` | `:308-370`（`get_descriptor`）邻域新增 2 个编排方法（走 `temp_driver_attempt`，**不发 OpenConnection**）+ **per-driver 并发预算（新增，预算 4）**（§6.4-①-⑤） | `:383-457` 的 `browse`（含 `:402-410` 的 `OpenConnection`）**一字不动** |
| **M6** | `drivers/sinumerik-nck/src/lib.rs` | `impl Driver`（`:88-378`）内新增 2 个方法实现 + 顶部 `mod definition_api;` | **冻结区间**：`descriptor()` `:90-352`、`open_connection` `:354-377`、`configure` `:622` 起、`browse` `:952` 起、run/poll 面——全部**逐字不动** |
| **M7** | `crates/core-api/src/lib.rs` | 路由表（`:3235-3342`）内、descriptor 路由（`:3241-3244`）邻域追加 **2 条只读路由**；`mod driver_query;` 与 `use`；**新增路由上挂 `DefaultBodyLimit::max(64 * 1024)`**（§6.4-①） | **零 Siemens 标识**；既有 40+ 条路由逐字不动 |
| **M8** | **5 个 `driver.toml`**：`drivers/sinumerik-nck`、`drivers/s7`、`drivers/focas2`、`drivers/opcua`、`tests/support/test-driver` | 每个的 `protocol_minor = 6` → `7`。**【已裁决修正】第 1 轮把 M8 写成「单文件 + 可选项」——两处都错**：实际是 **5 个文件**，且受 `crates/driver-protocol/src/lib.rs:679-714` 的 P1 invariant 单测**强制**，**不是可选项**（§7.5） | `id`/`protocol_major`/`executable` 不动；【已裁决】Minor「**可同步**」，但**不得把 Manifest 字段当成实际能力协商的证明**（§7.5） |
| **M9** | `tests/driver-contract/tests/management_api.rs`（3094 行） | **只在文件内新增用例**（矩阵 I1–I45 的 REST 侧）；**不删改既有断言** | `probe_does_not_create_endpoint`（`:676-699`）等既有用例**逐字不动**（对齐 PR-B M8 的「只新增」口径） |
| **M10** | `drivers/sinumerik-nck/src/compiler.rs` | `:351-413`（`compile_semantic`）与 `:416-444`（`compile_native`）：把「**D1–D10 校验 + 重算身份**」抽为**一个内部函数**，`check_identity`（`:411`/`:442`）作为**独立末步（D11）**保留；新增 `pub fn preview_point_key(...)` 调用**同一内部函数**并**跳过 D11** | **`check_identity`（`:489-520`）语义逐字不变**；`code()`（`:162-180`）/`kind()`（`:183-194`）**不动**；这是**同一实现的拆分**，不是第二套算法（矩阵 I12、**I32** 逐条证明）。**【已裁决修正】**：第 1 轮此处只写「校验 + 重算身份」，**未点明范围是 D1–D10** ⇒ 已按 §5.4.1 明确 |
| **M11** | `crates/driver-protocol/src/lib.rs`（**同一文件，第二处**） | 若纳入**并发预算**相关的纯函数（如 `definitions_query_supported(negotiated_minor)` 门控，与 `probe.rs:149-151` 的 `probe_supported` 同族），则此处同样零语义；**也可选择放在 Manager** ⇒ 见 §11-13 的**待确认项** | 【本轮新增行】；**只增不改**，既有常量与 `negotiate()` 逐字不动 |
| — | `apps/mesa-web/src/nckCatalog.ts` + `.test.ts` | **归属 PR-C**（PR-C `:428` 已登记「仅当 I-2 落地」） | **不在 I-2 文件范围内** |

#### 8.1.3 冻结（**10** 项，逐字不动）

| # | 路径 | 冻结理由 | 验收方式 |
|---|---|---|---|
| F1 | `drivers/sinumerik-nck/src/definition.rs`（含 `canonical_identity` `:1091-1133`） | I-2 的**唯一**身份权威来源；不得新增第二套算法 | 摘要逐字相等 |
| F2 | `drivers/sinumerik-nck/src/targets.rs`、`native.rs`、`codec.rs`、`value.rs`、`address.rs`、`client.rs`、`identity.rs`、`identity_spec.rs`、`presets.rs` | PR-B §2.3 `:190` 的逐字不动面（`identity.rs` 含**真机依赖**，`client.rs` 是唯一设备触点） | 摘要逐字相等 |
| F3 | `drivers/sinumerik-nck/definitions/828d/r1.json` + `DIGESTS.lock` | 「已发布 `(variable_id, revision)` 不可原地修改」（`definition.rs:8-10`） | 摘要逐字相等 + `tools/ci/nck_definition_lock.py --base` 通过 |
| F4 | `drivers/sinumerik-nck/presets/828d.json`（62 模板）、`tests/fixtures/legacy-828d-wire.json`（62 条）、`tests/fixtures/nck-point-key-golden.json`（55/5/77） | PR-B §2.3 `:192-198` | 摘要逐字相等 |
| F5 | `drivers/sinumerik-nck/tests/legacy_equivalence.rs`、`semantic_equivalence.rs`、`presets_828d.rs`、`definition_catalog.rs`、`compiler_identity.rs` | 既有门（`checked == 62`、身份字面量） | **原样通过**，`checked` 计数不降 |
| F6 | `crates/core-types/**`（尤其 `descriptor.rs` 2/0、`point.rs`、`schema.rs`、`resource.rs`、`capability.rs`、`event.rs`、`probe.rs`） | PR-A/PR-B 冻结面（PR-B `:200`、`:214`） | 摘要逐字相等 + P9 护栏通过 |
| F7 | `crates/core-api/src/binding_gate.rs` | PR-A 冻结契约（PR-B `:200`） | 摘要逐字相等 |
| F8 | `tests/driver-contract/tests/descriptor_contract.rs`（1094 行）、`resource_contract.rs`（461 行，含 P9 护栏）、`discovery_contract.rs:224-274` 既有断言 | PR-B C10=D-A `:196`、C6 `:197` | 摘要逐字相等 + 原样通过 |
| F9 | `scripts/contract_suites.py`（29 套名单） | 头部纪律 `:8`「增删 suite 必须显式评审本文件」 | 摘要逐字相等 |
| F10 | 三份设计文档（上位设计 / PR-B / PR-C）、`docs/adr/**`、`apps/mesad/**`、`drivers/s7|focas2|opcua/**`、`crates/config-store/**`、`crates/event-store/**` | 已冻结设计文档 / P1 护栏 / 明确不改目录 | 摘要逐字相等 |

**计数**：新增 **3** + 修改 **11**（【已裁决修正】：第 1 轮为「10 项」，本轮把 M8 从「单文件可选项」修正为「**5 个 `driver.toml` 的必需项**」并**新增 M11 行**；
其中 M1 本轮**未实际执行**（wire 增补 HOLD）、M8 为**必需项**、M9 为「只新增」、M10 为「同一实现拆分」）+ 冻结 **10**。

> **【已裁决修正】关于 M8 的计数口径**：M8 是**一个改动项**，但它**落在 5 个文件**上
> （`drivers/sinumerik-nck`、`drivers/s7`、`drivers/focas2`、`drivers/opcua`、`tests/support/test-driver`）。
> 按「改动项」计 **11 项**、按「受影响文件」计 **15 个文件**。**两个口径都写出来**，避免评审误算。

### 8.2 请求 / 响应（契约正文）

- **目录**：`GET /api/v1/drivers/{id}/definitions?family=…&variable_id=…&revision=…` → 见 §4.1/§4.2。
- **预览**：`POST /api/v1/drivers/{id}/definitions/identity-preview` → 见 §5.1/§5.2。
- **IPC**（4 个新 tag / 4 个新 message，JSON 承载以保持「小型外框 + 可演进的 JSON 契约」）：

```
message GetDefinitions {}                                  // tag 45
message DefinitionsReport {                                // tag 46
  GenericResult result = 1;      // ok / error_detail(kind, code, message)
  string definitions_json = 2;   // 目录 JSON（语义可见面白名单）
}

message PreviewPointKey { string request_json = 1; }        // tag 47
message PreviewIdentityReport {                             // tag 48
  GenericResult result = 1;
  string result_json = 2;        // { point_key, identity_mode, scope, variable_id, definition_revision, definition_digest }
}
```

> **为什么不把请求字段直接写进 proto**：与 `ProbeResponse.report_json`（`proto/driver.proto` 的注释
> 「Proto 只传契约外框，具体 ProbeReport 以 JSON 承载」）**同一取舍** —— 让 JSON 契约随 Minor 演进时
> **不必再动 wire**。这是本设计「最小」的一部分。

### 8.3 契约测试与验收矩阵（**45 行**：I1–I45；第 1 轮为 31 行 I1–I31）

> **分组（本轮修订后）**：目录 **I1–I10**、预览 **I11–I20**、只读与安全 **I21–I26**、兼容与回归 **I27–I31**、
> **跨接口一致性与定义变更 I32–I33（修订 2）**、**版本兼容矩阵 I34–I35（修订 1）**、
> **执行边界 I36–I39（修订 3）**、**四条只读性断言 I40–I43（修订 3）**、
> **`scope=identity` 边界 I44–I45（修订 4）**。
> 「所在套件」栏中，`management_api` = `tests/driver-contract/tests/management_api.rs`
> （**只新增用例**，不改既有），`definition_api` = 新增的 `drivers/sinumerik-nck/tests/definition_api.rs`。
>
> **本轮对既有行的改动（只 4 行，其余 27 行逐字保留）**：**I5**（补【已裁决】`family` 缺省断言）、
> **I9**（强化：快速失败 + `ErrorKind == Unsupported`）、**I27**（修正口径：断言 §7.2.1 逐字段表，
> **删除「44 个」这一错误数字**）、**I31**（保留，I-2a 仍可选）。
> **本轮新增 14 行**：I32–I45。

| # | 测试名（建议） | 断言要点 | 所在套件 | 证明什么 |
|---|---|---|---|---|
| I1 | `definitions_returns_42_rows_with_digests` | 200；`definitions.len()==42`；每行 `digest` 与 `DIGESTS.lock` 的 42 条**逐字一致** | `management_api` + `definition_api` | 目录完整性 + 发布摘要锚点 |
| I2 | `definitions_payload_is_verbatim_projection` | 每行字段值与 `r1.json` **逐字一致**（除 `digest`）；键集合**恰为**白名单（不含 `addressing`/`decode`/`transport_size`/`count`/`unit_mode`/`invalid_rule`） | `definition_api` | 「不重命名/不重排/不重算」+ 语义可见面 |
| I3 | `definitions_preserves_two_drive_entries_without_module_numbers` | 20 条 `drive.*` 的 `golden_ref[].entry` 去重 == `["nc_variable","drive_object"]`；其余 22 条 == 1 项；**响应体中不出现 `26`/`130` 作为入口标识** | `definition_api` | 26/130 区别被保留，且**不可由 number 反推 entry**（A14） |
| I4 | `definitions_reports_scope_and_status_verbatim` | `scope` = 36/6、`semantics_status` = 24/16/2 | `definition_api` | 支持状态与适用范围如实 |
| I5 | `definitions_family_filter_and_undeclared_family` | **不传 `family`（【已裁决】可省略）→ 200 且返回全部已发布 family 的 42 条**；`family=828d` → 42；`family=840d-sl` → **200 + `[]` + `note`**；`family=foo` → **400 `UNSUPPORTED_FAMILY`** | `management_api` | E8/E9 可区分（「无定义」≠「驱动故障」）+ 【已裁决】`family` 缺省语义 |
| I6 | `definitions_variable_and_revision_filters` | 命中 → 1 条；未知变量 → **404 `UNKNOWN_VARIABLE`**；`revision=2` → **404 `UNSUPPORTED_DEFINITION_REVISION`**（含 `available[]`）；`revision` 缺 `variable_id` → **400 `INVALID_QUERY`** | `management_api` | E10–E12；`lookup` 的「**绝不回落**」语义（`definition.rs:488-505`） |
| I7 | `definitions_route_does_not_create_endpoint` | 调用后 `GET /api/v1/endpoints` 仍为空 | `management_api`（对齐 `:676-699`） | 只读、无副作用 |
| I8 | `definitions_rpc_never_sends_open_connection` | 驱动侧未收到 `OpenConnection`（对照 browse 路径 `manager.rs:402-410`） | `management_api` + `definition_api` | 无连接 ⇒ 无设备会话 |
| I9 | `definitions_unsupported_driver_returns_501_not_timeout`（**【本轮强化】**） | 对未实现该 RPC 的驱动 → **501 `DEFINITIONS_UNSUPPORTED`**，且**快于** RPC 超时预算（**断言实测耗时 ≪ 5s 预算，不是「等到超时才失败」**）；**新增断言**：`error.kind == "Unsupported"`，**且 ≠ `ConnectionError` / `DeviceError`**（【裁决要求】不得把「协议不支持」误报为「设备离线」） | `management_api` | SDK 默认实现存在 + 快速失败（§7.2、§7.7-V1）+ **`ErrorKind` 确定性**（§6.2.1、§7.7-V5） |
| I10 | `definitions_stale_binary_is_gated_before_send` | `negotiated_minor < 7` ⇒ Manager **不发帧**，直接 501 | `protocol_negotiation`（**新增用例**） + `definition_api` | E5，同 `probe_supported` 先例（`probe.rs:147-151`） |
| I11 | `preview_returns_authoritative_point_key` | `channel.r_parameter{channel:1,index:5}` → `channel.r_parameter#index=i:5@channel=i:1` | `definition_api` | 输入/输出语义（§5.1/§5.2） |
| I12 | `preview_equals_configure_recomputed_identity_for_all_42` | **42 条定义逐条**：`preview(...) == compile_semantic/native 的 canonical_identity`（即 D11 的重算值） | `definition_api` | **预览不谎报**；「同一实现拆分」而非第二套算法（M10） |
| I13 | `preview_supports_compound_targets` | `{channel,axis}`（2 条）与 `{channel,spindle}`（3 条）预览成功且 Key 含两个键 | `definition_api` | PR-B C1 的复合 Target 合法 |
| I14 | `preview_drive_object_entry_is_not_collapsible` | `entry=nc_variable` 与 `entry=drive_object` 得到**不同** `point_key` | `definition_api` | 26/130 不可折叠（`targets.rs:88-108`） |
| I15 | `preview_native_derived_uses_frozen_rule` | `kind=nck_native`+缺省 → Key 恰为 `native.<area>.<area_no>.<module>.<column>.<line>`，**不含** `count`/`decode`/`transport_size`/`invalid_*`/`identity_mode` | `definition_api` | `definition.rs:1163-1176` 固定规则 |
| I16 | `preview_rejects_native_user_mode` | `identity_mode="user"` → **400 `INVALID_BINDING_CONFIG`** | `definition_api` | §2.5-B3（无权威身份可重算） |
| I17 | `preview_rejects_unknown_variable_and_revision` | **404 `UNKNOWN_VARIABLE`** / **404 `UNSUPPORTED_DEFINITION_REVISION`**（含 `available[]`） | `definition_api` | E19/E20 |
| I18 | `preview_rejects_illegal_target_and_arguments_with_precise_codes` | 缺必填 `MISSING_TARGET`；多余 `UNSUPPORTED_TARGET`；越界/缺 entry `INVALID_TARGET`；arguments `INVALID_ARGUMENTS`；身份键缺失 `MISSING_IDENTITY_KEY` | `definition_api` | E22–E26 逐条可达（PR-B §3.5 的码表被**复用而非改写**） |
| I19 | `preview_fails_closed_on_native_resolution` | `channel=255`（过 Target 校验、原生解析失败）→ **404/400 `INVALID_VARIABLE`**，**不得**返回 Key | `definition_api` | §5.4 的「预览必须复用完整链」反例 |
| I20 | `preview_never_returns_null_or_partial_key` | 任何 **200** 响应的 `point_key` **非空且非 null**；不存在「身份待确认」成功态 | `definition_api` | fail-closed 设计属性（§5.4） |
| I21 | `readonly_chain_has_zero_device_touchpoints` | grep 门：`definition_api.rs` + `definition.rs`/`targets.rs`/`compiler.rs`/`native.rs`/`value.rs`/`address.rs`/`codec.rs` 对 `NckClient`/`connect(`/`.await`/`WriteRequest`/`CommandRequest`/`OpenConnection` **零命中** | CI grep 门（§8.4 G7） | R2（代码级只读证据） |
| I22 | `management_rpc_requires_no_connection_entry` | 在**零 Connection 条目**下两个 RPC 仍成功（直接对 `NckDriver` 调用即可，不经 `open_connection`） | `definition_api` | R4；特别要求 B 的核心断言 |
| I23 | `preview_route_produces_no_samples_and_no_tasks` | 调用后 `GET /api/v1/points/latest` 无新增、`GET /api/v1/tasks` 不变 | `management_api` | 只读、无数据面副作用 |
| I24 | `control_routes_and_nck_controls_unchanged` | `POST /write`、`POST /commands/{command}` 行为不变；未 `--enable-control` 时仍 `CONTROL_DISABLED`；NCK `controls` 仍为默认空目录（`lib.rs:338`） | `control_contract` + `management_api`（原样通过） | R7；I-2 不触碰控制面 |
| I25 | `core_has_zero_siemens_knowledge_still_passes` | P9 护栏（`resource_contract.rs:396-461`）**原样通过** | `resource_contract`（原样） | Core 零语义（含新增代码） |
| I26 | `error_kind_to_status_mapping_is_driver_agnostic` | Core 侧映射函数只出现 `ErrorKind` 变体；grep 断言不含任何 NCK 码字面量 | CI grep 门 + `resource_contract` | §3.3/§6.2 |
| I27 | `protocol_additive_fields_do_not_alter_existing_messages`（**【本轮修正口径】**） | proto diff **只体现新增**。**断言对象从「数字」升级为「逐字段表」**：① **§7.2.1 的 35 行逐字段核对表**（tag → 成员类型 → 成员名）在增补前后**逐字不变**；② **既有 35 个 tag 的编号集合恰为 `{10..44}`**（连续、无空洞、无重复）；③ **新增 tag 集合恰为 `{45,46,47,48}`**（且新增 4 个 `message`）；④ 全部既有 `message` 的字段编号/类型逐字不变。**不得**把断言写成「44 个」（【已裁决修正】：第 1 轮该数字错误） | `protocol_negotiation`（新增用例） | §7.2 + **§7.2.1**（逐字段核对才是真值，计数是派生物） |
| I28 | `negotiated_minor_semantics_unchanged` | 新 Core ↔ 旧 Driver（minor 6）⇒ 协商 6；既有 4 个 minor 门行为不变 | `protocol_negotiation`（新增用例） | `negotiate` 取 min（`:100`） |
| I29 | `wire_and_identity_goldens_unchanged` | `legacy-828d-wire.json` / `presets/828d.json` / `nck-point-key-golden.json` 摘要不变；两道等价门仍 `checked == 62` | `legacy_equivalence` / `semantic_equivalence` / `compiler_identity`（原样） | §7.3 |
| I30 | `descriptor_contract_and_other_drivers_unchanged` | Descriptor 仍 2/0；`descriptor_contract.rs` 逐字不动并通过；S7/FOCAS2/OPC UA/test-driver 行为不变（SDK 默认实现） | `descriptor_contract` + 其它驱动套件（原样） | §7.4 |
| I31 | `browse_leaf_projection_matches_catalog`（**仅当 I-2a 落地**；【已裁决】I-2a **保持可选**） | 浏览叶新增的 `definition` 投影与目录端点对同一 `(variable_id, revision)` 的返回**逐字一致** | `identity_browse` / `definition_api` | I-2a 与 I-2b **不产生第二份真值**（【已裁决】I-2a **不得生成第二份变量定义真值**） |
| **I32** | `preview_identity_equals_subsequently_configured_identity`（**修订 2**） | **跨接口一致性（裁决原话的落地）**：对**全部 42 条**定义构造合法输入 ⇒ ① Preview 返回 `K_p`（200 且非空）；② `K_p` **逐字**写入 `PointSelection.point_key` 后走**真实 Configure** ⇒ **接受**；③ `K_p` **逐字等于** Configure 内部重算值 `K_c`（字符串相等，**无归一化**）。**任一条不成立即失败**（详见 §5.4.2） | `definition_api` + `management_api`（e2e） | **相同合法输入下 Preview 身份 == 随后 Configure 接受的身份**；证明 Preview 与 Configure **共用同一条权威验证路径** |
| **I33** | `preview_rejects_changed_definition_or_unsupported_revision`（**修订 2**） | ① `revision` 不存在 ⇒ **404 `UNSUPPORTED_DEFINITION_REVISION`**（含 `available[]`）**且不返回任何 Key**；② `variable_id` 不存在 ⇒ **404 `UNKNOWN_VARIABLE`**，**不返回任何 Key**；③ **模拟目录在 Preview 与 Configure 之间被替换**（`digest` 不同）⇒ Configure 侧 **400 `POINT_KEY_MISMATCH`**（E29），消息含**持久化 Key + 重算 Key + `variable_id` + `revision`** 三要素；④ 上述任一情形**不得**出现「200 + 看似可用的 Key」 | `definition_api` + `management_api` | 【裁决要求】**不得提供「看似有效但无法配置」的身份**（§5.4.3） |
| **I34** | `legacy_manager_new_driver_downgrade_is_silent`（**修订 1 / 旧 Manager ↔ 新驱动**） | 以 `PROTOCOL_MINOR = 6` 的旧 Manager 对**新驱动二进制**：① 协商结果 `accepted_protocol_minor == 6`；② 旧 Manager **永不发送** tag 45–48（无路由 ⇒ 调用方得 404）；③ 新驱动在**旧消息集**上行为与本改动前**逐字一致**（无告警、无错误、无行为变化） | `protocol_negotiation`（新增用例） | §7.7-**V3** 降级行为可验收 |
| **I35** | `unknown_envelope_variant_never_aborts_driver_process`（**修订 1 / 未知消息**） | 向驱动**注入未知 oneof 变体**（未知 tag）：① 驱动进程**不 panic、不退出**（存活断言）；② **连接不中断**（后续 `Ping`/`GetDescriptor` 正常）；③ 既有兜底分支行为保持 —— `other => tracing::debug!(?other, "unexpected envelope ignored")`（`driver-sdk:1178-1179`）**逐字不变** | `protocol_negotiation`（新增用例） + `driver-sdk` 单测 | §7.7-**V4**：未知消息**不得导致进程异常退出** |
| **I36** | `definition_query_body_size_cap_enforced`（**修订 3**） | ① 预览 `POST` body **恰好 64 KiB** → 通过；② **64 KiB + 1 字节** → **413 `REQUEST_TOO_LARGE`**（E30）；③ 目录 `GET` 无 body ⇒ 该门不适用；④ 断言该门由 `DefaultBodyLimit::max(64 * 1024)` 挂在新路由上（**不是**全局 2 MB 默认值） | `management_api` | §6.4-①-① + E30 |
| **I37** | `definition_query_result_cap_and_pagination_enforced`（**修订 3**） | ① 不传 `limit` ⇒ `limit == 100`、`total == 42`、`next_cursor == null`；② `limit=5000` ⇒ **夹取到 500**（**不是错误**，与 Browse `core-api:834` 同口径）；③ 构造 > 100 条的分页场景（或注入合成目录）⇒ `next_cursor` 非 `null`，用该游标可取到**不重不漏**的下一页；④ 目录 JSON > **256 KiB** 或过滤后条目 > **4096** ⇒ **503 `DEFINITION_QUERY_TOO_LARGE`**（E4）；⑤ `limit=0` / `cursor` 与 `variable_id` 并存 ⇒ **400 `INVALID_QUERY`**（E12） | `definition_api` + `management_api` | §6.4-①-② / ①-③ + E4/E12 |
| **I38** | `definition_query_timeout_budget_enforced`（**修订 3**） | ① 注入「驱动握手超时」⇒ **503 `DRIVER_UNAVAILABLE`**（E2），**不超过 `DRIVER_STARTUP_TIMEOUT = 6s`**（`session.rs:280`）；② 注入「RPC 不响应」⇒ **503 `DEFINITION_QUERY_TIMEOUT`**（E3），**不超过 5s**（`session.rs:667`）；③ 断言 attempt ≤ **2**（`temp_op.rs:25`）⇒ **单次调用最坏上界 ≤ 22s**；④ 断言 `REQUEST_TIMEOUT`（10s，`session.rs:25`）**未被调大** | `management_api` + `definition_api` | §6.4-①-④ + E2/E3 |
| **I39** | `definition_query_concurrency_budget_enforced`（**修订 3**） | ① 同一 `driver_id` 并发发起 **5** 个定义查询（预算 **4**）⇒ 第 5 个**立即**得到 **503 `DEFINITION_QUERY_BUSY`**（E31），**不排队、不等待**；② 消息含当前 in-flight 数 + 预算；③ 断言 in-flight 计数在调用结束后**回落到 0**（**无泄漏**）；④ **跨 driver 不互相影响**（预算是 per-driver 的） | `management_api` + `resource_contract`（不新建套件） | §6.4-①-⑤ + E31；【如实】该机制为**新增**（仓库当前无并发闸门） |
| **I40** | `definitions_rpc_opens_no_cnc_connection`（**修订 3 / R8**） | ① 驱动进程侧**未收到** `OpenConnection`（对照 `manager.rs:402-410` 的 browse）；② 代码侧：`definition_api.rs` + 七个模块对 `client::`/`NckClient`/`connect(` **零命中**；③ `/api/v1/endpoints` 调用前后**仍为空** | `management_api` + `definition_api` | **R8**：目录与 Preview **均不创建 CNC Connection** |
| **I41** | `definitions_rpc_reads_no_live_machine_values`（**修订 3 / R9**） | ① `NckClient` 读值 API **零调用**；② `GET /api/v1/points/latest` 调用前后**无新增**；③ **无** `DataBatchMsg` 产生 | `management_api` + `definition_api` | **R9**：目录与 Preview **均不读取机床实时值** |
| **I42** | `definitions_rpc_does_not_modify_acquisition_plan`（**修订 3 / R10**） | ① **未发送** `ConfigureTasks`/`ApplyPointMap`/`StartConnection`/`StopConnection`；② `GET /api/v1/tasks` 快照**逐字不变**；③ Endpoint 的 `plan`/`stream_epoch` 不变 | `management_api` | **R10**：目录与 Preview **均不修改采集计划** |
| **I43** | `definitions_rpc_has_no_persistent_side_effects`（**修订 3 / R11**） | ① `config-store`/`event-store` 的 SQLite 文件**摘要不变**；② Control 审计**无新记录**；③ `r1.json`/`DIGESTS.lock` **摘要不变**；④ **不创建**任何新文件（含缓存文件） | `management_api` + `resource_contract`（原样） | **R11**：目录与 Preview **均无持久化副作用** |
| **I44** | `catalog_returns_all_six_identity_scoped_definitions`（**修订 4 / 硬性约束**） | ① 目录端点**如实返回全部 42 条**，其中 `scope="identity"` **恰为 6 条**（逐条 id 与 §9.1 的清单**逐字一致**：`axis.name`、`axis.ctrlout.module_number`、`axis.encoder.module_number`、`axis.ctrlout.type`、`channel.machine_axis_assignment`、`axis.spindle_assignment`）；② **绝不**因 `scope="identity"` 而在目录层**过滤/删除/改名**；③ `presets/828d.json` 仍为 **62 模板**、浏览树仍为 **62 叶**（**逐字不变**）；④ **反向断言**：不存在「目录少 6 条 + 浏览树仍 62 条」这种**两份真值**的组合 | `definition_api` + `identity_browse` + `presets_828d`（原样） | **修订 4 的硬性约束**：**禁止静默删除六条身份变量而保留 62 条模板**（§9.3-C′） |
| **I45** | `preview_scope_semantics_match_configure_for_identity_scope`（**修订 4**） | ① 对 6 条 `scope="identity"` 定义，Preview **返回可算的 `point_key`**（与 Configure **行为一致**），**不是**「预览比 Configure 更严」；② Preview 响应的 `scope` 字段为 `"identity"`（**如实**）；③ 同输入下 Configure **接受**同一 Key（与 I32 同一判据，此处**专门覆盖 `scope=identity`**）；④ 断言 **I-2 未把 `scope` 校验下沉到 Configure**（`compile_semantic` 仍不读 `def.scope`，`definition.rs:51-57`；`presets.rs:242-275`） | `definition_api` | **修订 4**：**I-2 如实返回 `scope`；Preview 与当前 Configure 的计算语义保持一致**；`scope` 的准入判定归 **PR-C**（§9.3-C） |

### 8.4 CI 门禁建议

> **口径说明**：以下命令与既有口径一致 —— `cargo test --workspace --all-features`
> **不作为本地强制门禁**；性能归 CI `rust-perf`（`.github/workflows/_rust-perf.yml`，`name: rust-perf`）。

| # | 门禁 | 命令 | 通过标准 |
|---|---|---|---|
| **G0** | **前置：改驱动代码后必须先构建**（`AGENTS.md` 明写） | `cargo build --workspace` | 成功。**理由**：`cargo test -p mesa-contract-tests` 只构建依赖包 lib，不重编驱动 bin；旧二进制会让子进程类合同测试**静默失效** |
| **G1** | 格式 | `cargo fmt --all -- --check` | 零差异（CI `_rust-quality.yml` 的 `fmt` 步为 `cargo fmt --check`） |
| **G2** | Lint（默认 feature） | `cargo clippy --workspace --all-targets -- -D warnings` | 零警告 |
| **G3** | Lint（全 feature） | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 零警告（CI 形态为 `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`） |
| **G4** | 构建（锁定） | `cargo build --locked --workspace` | 成功 |
| **G5** | 全量测试 | `cargo test --workspace --locked` | 全绿；**既有 `compiler_identity.rs` / `definition_catalog.rs` / `legacy_equivalence.rs` / `semantic_equivalence.rs` 必须原样通过** |
| **G6** | 合同测试（唯一准入基线） | `cargo test --locked -p mesa-contract-tests --all-features` | **29/29**（`scripts/contract_suites.py:52` 的 `SUITES`，**名单不变**） |
| **G7** | **只读红线门（新增，grep）** | ① `definition_api.rs` 与 §8.3-I21 的七个模块对 `NckClient\|client::\|tokio::net\|TcpStream\|\.await\|OpenConnection\|WriteRequest\|CommandRequest` 零命中；② `crates/core-api/src/driver_query.rs` + `crates/core-api/src/lib.rs` 新增行对 P9 词表零命中；③ **【本轮新增】** 新增路由/handler 的标识符与新增错误码字面量（`REQUEST_TOO_LARGE`/`DEFINITION_QUERY_BUSY`）**不含** NCK 语义 token（`nck_variable`/`drive_object`/`UNKNOWN_VARIABLE`…） | 零命中 |
| **G8** | 定义锁（既有） | `python3 tools/ci/test_nck_definition_lock.py` + `python3 tools/ci/nck_definition_lock.py --base "$BASE"` | 通过（`r1.json`/`DIGESTS.lock` 未改 ⇒ 必过） |
| **G9** | 证据产出（唯一可信 Evidence） | `python scripts/write-contract-evidence.py` | 产出 `target/validation/contract.json`；**跑之前先过 G0** |
| **G10** | 前端（**仅当** PR-C 侧的 `nckCatalog.ts` 落地时） | `pnpm --dir apps/mesa-web test` + `pnpm --dir apps/mesa-web build` | 全绿；用例数 `>= 383`（PR-C 基线）；`build` 必须成功（PR-C 门禁 **G11**） |
| **G11** | 性能 | CI `rust-perf` | **不在本 PR 范围**（I-2 无热路径改动） |

**新增/同步的契约套件断言（不新建 suite 文件，29 套名单不变）**：
`management_api`（新增 I1、I5–I9、I23、**I32、I33、I36–I43**）、
`protocol_negotiation`（新增 I10、I27、I28、**I34、I35**）、
新建 `drivers/sinumerik-nck/tests/definition_api.rs`（I2–I4、I11–I22、I31、**I32、I33、I36–I39、I44、I45**）。

> **【本轮新增的必过门（既有单测，不需新写）】**：`in_tree_manifest_minor_matches_protocol`
> （`crates/driver-protocol/src/lib.rs:683`）—— 它**强制 5 个 `driver.toml` 的 `protocol_minor`
> 等于 `PROTOCOL_MINOR`**（§7.5/M8）。它属于 **G5（全量测试）**，因此**已在门禁内**，
> 但**必须在本 PR 的验收结论里显式点出**，否则 M8 的「5 个文件」容易被漏改。

### 8.5 归属建议

| 项 | 建议 |
|---|---|
| PR 命名 | `PR-B2-nck-definition-catalog-api`（与 PR-C §10.1 `:1218` 的建议一致） |
| 内容 | **I-2b 主体 + 可选 I-2a**（【已裁决】I-2a **保持可选**，且**不得生成第二份变量定义真值**）；wire 增补（M1/M2/**M8 的 5 个 `driver.toml`**）**必须在本 PR 内**（不得夹带到 PR-C）。**【已裁决】**wire 增补**已获原则批准**，但**生产代码仍 HOLD** |
| **禁止混入** | `apps/mesa-web/**`（PR-C/PR-D）、`drivers/s7|focas2|opcua/**`、`crates/core-types/**`、`crates/config-store/**`、`apps/mesad/**` |
| 与 PR-C 的关系 | PR-C **在 I-2 落地前**保持 §1.5 的降级路径：目录端口不可得 ⇒ 「适用范围未提供」⇒ **A40 fail-closed（禁用/不可确认）**，并如实显示「支持状态未提供」 |

---

## 9. `scope=identity` 缺陷的独立处理边界

### 9.1 缺陷事实（本次**独立复现**，不只引用 PR-C 结论）

| 事实 | 证据 |
|---|---|
| `DefinitionScope` 的**声明** | 「`Collect` 出现在用户点位面；`Identity` 只服务身份探针」 | `definition.rs:51-57` |
| `scope=identity` 的 **6** 条定义 | `axis.name`、`axis.ctrlout.module_number`、`axis.encoder.module_number`、`axis.ctrlout.type`、`channel.machine_axis_assignment`、`axis.spindle_assignment` | 本次解析 `r1.json` 实测 |
| 这 6 条的 `golden_ref` **全部出现在 preset 模板中** | **6/6**（本次逐条交叉核对，明细）：<br>`axis.name → 828d.N.M.MDSA_AXCONF_MACHAX_NAME_TAB.nc_variable`<br>`axis.ctrlout.module_number → 828d.A.M.MDCA_CTRLOUT_MODULE_NR.nc_variable`<br>`axis.encoder.module_number → 828d.A.M.MDCA_ENC_MODULE_NR.nc_variable`<br>`axis.ctrlout.type → 828d.A.M.MDCA_CTRLOUT_TYPE.nc_variable`<br>`channel.machine_axis_assignment → 828d.C.M.AXCONF_MACHAX_USED.nc_variable`<br>`axis.spindle_assignment → 828d.A.M.SPIND_ASSIGN_TO_MACHAX.nc_variable` | 同上 |
| 生产代码中 `scope` **从未被校验** | **本次独立 grep 实测**：`DefinitionScope` 在整个 `drivers/sinumerik-nck/src/**` 中只出现 **3 处** —— `definition.rs:54`（枚举定义）、`definition.rs:210`（`VariableDefinition.scope` 字段定义）、`lib.rs:41`（**仅 `pub use` 再导出，无任何逻辑**）；`semantic_binding`（`presets.rs:242-275`）与 `compile_semantic`（`compiler.rs:351-413`）**都不读 `def.scope`**。全部**逻辑**引用都在测试里：`tests/definition_catalog.rs:70`、`:412`、`:434`
| **⇒ 后果** | 语义 Browse **今天**会把「只服务身份探针」的变量当作可采集点位交给用户；且 **Configure 也不会拒绝**（因为编译链不看 `scope`） |

### 9.2 这是 **Driver 侧缺陷**（不是前端缺陷、不是 Core 缺陷）

| 归属 | 结论 | 依据 |
|---|---|---|
| **Driver（权威修复点）** | `presets.rs` 或目录查询层按 `scope` 过滤，或至少把 `scope` **如实体现在可采集能力上** | PR-C §10.2.3（`:1242-1246`） |
| 前端 | **不得**用变量名 / `variable_id` 前缀猜 `scope`（那属于自行发明语义）；缺失时**保持 fail-closed** | PR-C §10.2.3 `:1246`、§10.2.4 `:1248-1254` |
| Core | **不得**参与（P9 护栏；`scope` 是 NCK 语义） | `resource_contract.rs:398-428` |

### 9.3 I-2 **是否顺带修复它** —— **不顺带**（附理由）

| 选项 | 结论 | 理由 |
|---|---|---|
| **A. I-2 顺带在 Configure 路径按 `scope` 拒绝** | **❌ 不采纳** | ① 会**改变 Configure 的既有验收语义**（今天 `scope=identity` 的配置能通过编译，PR-B 的 22 条 golden/多套测试都在这个语义下冻结）；② 会**扩 PR-B §3.5 的唯一权威错误码表**（需要一个新的「不可采集」码）；③ PR-C 明确把权威修复点定为 **Browse/目录层**，而不是编译准入层（`:1243-1245`）；④ 会把「元数据可用性」与「准入策略」两件事耦合成一个 PR |
| **B. I-2 顺带在 Browse 层按 `scope` 过滤** | **❌ 不采纳（但可以是 I-2a 的**可选**附加项）** | ① 会**改变既有浏览树的内容**（`nck://presets` 从 62 叶变 56 叶），触碰 `discovery_contract.rs` 的既有断言面与 `presets_828d.rs` 的 62 模板断言（PR-B 冻结面 `:197`）；② 该过滤应**独立评审**（它改变的是「用户能看到什么」，属产品语义决定） |
| **C. I-2 只提供 `scope` 元数据，不过滤、不拒绝** | **✅ 采纳（I-2 的范围）**；【已裁决】**本项获裁决确认** | I-2 的职责是**提供权威元数据**；「缺 `scope` 不得默认放行」由 PR-C 的 **A40 fail-closed** 在前端执行；「不该出现在浏览树」由**独立 Driver 缺陷修复**执行。**【已裁决】同意继续独立登记 Driver 缺陷**（即选项 D 归属不变） |
| **D. 缺陷修复的归属** | **独立工作项**（建议 `PR-B3-nck-scope-collectability` 或并入 Driver 的下一个功能 PR）；【已裁决】**同意继续独立登记 Driver 缺陷** | 改动点在 `presets.rs`（或目录查询层）⇒ 属 Driver；**必须**与 I-2 分开，因为它的验收面是「浏览树内容 + Configure 准入」而不是「元数据接口」 |

#### 9.3-C′ 【已裁决 / 硬性约束】**禁止**「静默删六条 + 保留 62 模板」这条捷径

> **裁决原文要点（禁止的捷径）**：
> **不得为了使 UI 好看而在目录接口中静默删掉六条身份变量，同时又保留旧 Browse 中的 62 条模板。**
>
> **这被写成本设计的硬性约束，不是建议。** 具体禁止的行为：
> **在 `GET /api/v1/drivers/{id}/definitions` 的目录投影里按 `scope == "identity"` 过滤掉那 6 条**，
> 而**同时不动** `presets/828d.json`（62 模板）与 `nck://presets` 浏览树（62 叶）。

**为什么这条捷径必须被禁止（两条独立理由）**：

| # | 理由 | 展开 |
|---|---|---|
| **1** | **它会造成「第二份定义真值」** | 目录端点与 Browse 树是**两条独立数据来源**（`definition.rs:23-25` 明写：`DefinitionCatalog::embedded()` 与 `PresetRegistry` 只共享冻结向量）。若目录侧过滤、Browse 侧不过滤，则**同一时刻系统内对「这个变量是否可用」存在两个互相矛盾的答案**（目录说「没有」、Browse 说「有」）。这正是本文件与 PR-B/PR-C 反复强调要避免的**双真值**形态 —— 而 I-2a 之所以需要 **I31** 交叉一致性断言，也是同一个理由。 |
| **2** | **它让「准入策略」伪装成「数据事实」** | `scope` 的语义是**元数据**（`definition.rs:51-57`：「`Identity` 只服务身份探针」）。**「该不该进入可采集列表」是产品准入策略，属 PR-C 的 A13/A40/A43**（§9.4 六行映射）。在**目录数据层**偷偷删掉 6 条，等于**把策略判定藏进数据投影里**：调用方**无法区分**「这个定义不存在」与「这个定义存在但被过滤了」，PR-C 的 **A40 fail-closed 也就无从判定**（它需要**看到** `scope` 才能 fail-closed）。**⇒ 目录必须如实返回 6 条，让策略留在策略层。** |

**⇒ 可验收形态（矩阵 I44）**：目录端点返回 **42 条**（其中 `scope="identity"` **恰 6 条**、逐条 id 与 §9.1 一致），
`presets/828d.json` 仍 **62 模板**、浏览树仍 **62 叶**；**不存在**「目录少 6 条 + 浏览树 62 条」这一组合。
**若将来要修「Browse 暴露 identity 变量」这个 Driver 缺陷（选项 B/D）**，必须**独立 PR、独立评审**，
且**同时**处理目录与 Browse 两个投影（或**明确声明**哪个是权威、另一个如何降级），**不得只动一边**。

**⇒ 另外两条【已裁决】落点（修订 4 的完整口径）**：

| # | 裁决要点 | 落点 |
|---|---|---|
| **C1** | **I-2 如实返回 `scope`** | §4.3 字段表（`definitions[].scope` 行）+ §4.4 的【已裁决】注 + 矩阵 **I4**/**I44** |
| **C2** | **Preview 与当前 Configure 的计算语义保持一致** | §2.5-B4、§5.4、§5.4.1、矩阵 **I45**（对 6 条 `scope=identity` 专门断言「Preview 与 Configure 同行为」） |
| **C3** | **PR-C 根据 `scope` 决定是否允许进入采集配置** | §9.4 六行映射（PR-C A13/A40/A43）；I-2 **只提供元数据**，**不做**准入判定 |
| **C4** | **同意继续独立登记 Driver 缺陷** | §9.2 归属表 + §9.3-D（**独立工作项**，建议 `PR-B3-nck-scope-collectability`） |

### 9.4 缺陷修复前，PR-C **必须保持 fail-closed**（不得放宽）

> **PR-C 不得把缺失 `scope` 元数据的变量默认认定为可采集。**
> **若无法可靠区分，则保持禁用或不可确认状态，不得静默放行。**
> （PR-C §10.2.4 `:1248-1252`，可验收断言 **A40**）

| 情形 | 允许提交？ | 承担该行的断言 |
|---|---|---|
| `scope == "collect"` 且其余元数据完整且身份条件满足 | **是** | PR-C A43（§1.4.4 第 1 行） |
| **只有 `identity.keys`**（无 `targets`/`target_required`/`arguments[]`/`scope`） | **否** | PR-C A42（§1.4.4 第 2 行） |
| **`scope` 缺失** | **否** | PR-C A40、A13（§1.4.4 第 3 行） |
| **`scope == "identity"`** | **否**（不作为可采集点位提供） | PR-C A13、A40（§1.4.4 第 4 行） |
| Target/Arguments 已改但无权威身份校验路径 | **否** | PR-C A39（§1.4.4 第 5 行） |
| Driver 不可达 | **否** | PR-C A15（§1.4.4 第 6 行） |

**⇒ I-2 在这条缺陷上的作用只有一个**：把 `scope` **变成可得**，从而让 PR-C 的
「`scope === "collect"` 才允许进入可采集列表」**从「一律禁用」升级为「可按真值区分」**。
它**不**修复「Browse 暴露 identity 变量」这一 Driver 侧缺陷，**也不**改变 Configure 的准入语义。

---

## 10. PR-C / PR-D 边界与「明确不做」

### 10.1 PR-C / PR-D 边界**不变**

| 边界 | 结论 |
|---|---|
| `apps/mesa-web/src/addDevice/AddDeviceFlow.tsx` 与 `AddDeviceFlow.test.tsx`（**含对应测试**） | **继续由 PR-D 独占**，I-2 与 PR-C **都不修改**（PR-C §8.3 `:1096`、§2.2 注 `:455-458`） |
| PR-C 的文件范围、验收矩阵（44 行 A1–A44）、门禁 G0–G11 | **不变**（PR-C §2、§7、§11） |
| PR-C 的 I-1（零后端改动、把既有 `identity` 块正式化为只读契约） | **不变**；I-2 **不替代** I-1，也不修改 I-1 的任何结论 |
| PR-C 的 L1 / L2 两级验收 | **不变**；I-2 只影响 **L2** 的可达成性（`scope` 过滤、支持状态展示、服务端权威 Preview） |

### 10.2 I-2 **明确不做**（防止范围蔓延）

| # | 不做 | 理由 |
|---|---|---|
| 1 | **不新增/修改任何持久化面**（定义不进 SQLite、不做用户导入定义） | 定义是**发行数据**（`definition.rs:8-10`）；PR-B §7.3 `:941` 亦把「用户导入定义 / Core 变量知识库」列为不做 |
| 2 | **不做 Core 侧变量知识库 / 编译快照** | 违反 P9 护栏与 PR-B `:941` |
| 3 | **不改 Descriptor**（不加目录字段、不动 2/0） | §1.2 的 C3 否决理由 |
| 4 | **不改 `presets/828d.json` / 62 条 wire golden / G-B 身份 golden** | PR-B §2.3 `:192-198` |
| 5 | **不把 `scope` 过滤或拒绝下沉到 Configure / Browse** | §9.3-A/B |
| 6 | **不为 `identity_mode="user"` 提供身份重算**（只拒绝并说明） | §2.5-B3 |
| 7 | **不返回原生字段**（`addressing`/`decode`/`transport_size`/`count`/`unit_mode`/`invalid_rule`/`source_label`/`resolved`） | §4.4-①、§5.2 |
| 8 | **不做前端**（`nckCatalog.ts` 等归 PR-C） | §8.1.2 末行 |
| 9 | **不做 `nck_native` 的服务端限权** | PR-B §7.3 `:941`（仅沿用只读基线 + 前端警告口径，PR-C 落地） |
| 10 | **不做 840D sl 定义、不做新 area 字母（`M`/MMC/`S<n>`）** | PR-B §7.3 `:941` |
| 11 | **~~不做 `DefinitionCatalog` 的对外缓存/分页~~**（目录只有 42 条、约 58 KB 源文件） | **【已裁决修正】作废**：本轮【裁决要求】只读接口**必须**设执行边界，其中**含分页** ⇒ 改为「**做**最小分页（`limit` 缺省 100/上限 500 + 不透明 `cursor` + `next_cursor`，§4.1/§6.4-①-③）」，但**仍不做**对外缓存（无意义：目录是编译期内嵌数据，`definition.rs:47`）。第 1 轮「无必要」的结论**作废** | 修订 3（§6.4）；矩阵 **I37** |
| 12 | **不引入通用 RPC 框架** | §1.3-④ |
| 13 | **本轮（第 2 轮修订）不修改 `proto/driver.proto`、不修改任何代码/测试/配置** | §0；wire 增补**已获原则批准但生产代码仍 HOLD**；本轮是**纯文档修订** |
| 14 | **不做「目录侧过滤 `scope=identity`」这条捷径** | §9.3-C′（**硬性约束**）；矩阵 **I44** |

---

## 11. 无法确认 / 与既有设计冲突之处（**如实登记，不抹平**）

| # | 项 | 状态 | 影响 | 需要的动作 |
|---|---|---|---|---|
| **1** | **wire 的 additive 增补是否获准** | **【已裁决】已获原则批准**（原为「冲突/需裁决」）。PR-B §2.4 `:215` 把 `proto/**`、`crates/driver-protocol/**`、`crates/driver-sdk/**` 列为 **【不得触碰】**；但 PR-C §1.4 I-2 又明说 I-2「**可能触及 `proto/**`**」（`:263`、`:1212`），PR-B §7.2 `:936` 亦给出「确需改动 ⇒ 独立契约变更 PR」的合规路径 | **I-2b 成立**；**但生产代码仍 HOLD**（本轮不改 `proto/**`、不改任何代码），须待上级启动独立「契约变更」PR | 无需再裁决；**影响面清单见 §7.6 + §7.2.1 + §7.5（M8 的 5 个 `driver.toml`）** |
| **2** | **Preview 端点的路由命名与 `POST` 语义** | **【已裁决】已批准**：`POST /api/v1/drivers/{id}/definitions/identity-preview`。**`POST` 表示「带参数的只读计算」，不代表写入**（§5.1 显式说明）；四条只读性断言见 R8–R11 / **I40–I43** | 仅影响命名与读者理解，不影响语义 | 无需再裁决；文档已显式写明 `POST` 的只读语义（§5.1、§6.4-③） |
| **3** | **`family` 的缺省语义与 PR-C 措辞不一致** | **【已裁决】保留差异**：目录查询**可省略 `family`**（缺省 = **全部已发布 family**）；**Preview 必须显式指定 `family`**。原 PR-C `:259` 写「缺省用 Endpoint/驱动配置」，本设计因路由是 **driver 作用域**（无 Endpoint）而**无法沿用** | 前端需从 `Endpoint.connection.family` 或目录行自行带上 `family` | 无需再裁决；矩阵 **I5** 已覆盖「缺省 ⇒ 全部 family」 |
| **4** | **`digest` 字段超出 PR-C 的枚举** | **【已裁决修正】原「差异（已登记，可选）」作废** ⇒ 裁决为**允许**：`digest` 可作为**定义版本与完整性元数据**返回（§4.4-③） | 无 | 无需再裁决；**实现时不得以「PR-C 未枚举」为由删除它** |
| **5** | **预览是否支持 `nck_native`** | **范围解释（保留待确认）**。PR-C 的语义模式只管 `nck_variable`（A20 明确语义模式不出现 `identity_mode` 控件）；本文件把预览定义为**对 `kind` 通用**（含 `nck_native`+`derived`） | 属**范围扩大**（但只增不减：`nck_variable` 的行为不受影响） | **仍待评审确认**；若要求严格最小 ⇒ 预览只接受 `nck_variable`，`nck_native` 返回 501 `UNSUPPORTED_POINT_KIND`（响应形状不变） |
| **6** | **`label` 有两个来源（目录 vs 模板）** | **已确证的双真值事实**（不抹平）。目录的 `label` 在 `r1.json`（`definition.rs:72-77`），模板的 `label` 在 `presets/828d.json`（`presets.rs:38`）；两者是**两条独立数据来源**（`definition.rs:23-25` 明写） | 前端展示变量名时需指定优先来源。本文件建议：**I-2 落地后以目录 `label` 为准**，模板 `label` 仅用于浏览树节点 | 评审确认；**I-2 不合并两者、不改模板** |
| **7** | **`ErrorKind → HTTP` 的已知不直观映射** | **取舍（已登记）**。`UNSUPPORTED_FAMILY`/`UNSUPPORTED_POINT_KIND`（`Unsupported`）⇒ **501**；`INVALID_VARIABLE`（`Address`）⇒ **404** | 前端必须按 body 的 `code` 分流（PR-C §6.3 已要求） | 评审确认；替代方案（Core 按码映射）**会违反 P9 护栏**，不可选 |
| **8** | **`driver.toml` 的 `protocol_minor` 与能力协商** | **【已裁决修正】第 1 轮的结论「未被任何门使用」不准确**：① 发现期**确实**只校验 `protocol_major`（`manifest.rs:95-103`）；② **但**存在 P1 invariant 单测 `in_tree_manifest_minor_matches_protocol`（`crates/driver-protocol/src/lib.rs:679-714`）**强制 5 个 in-tree `driver.toml` 的 `protocol_minor` 等于 `PROTOCOL_MINOR`** ⇒ M8 **必需且涉及 5 个文件**（§7.5）。③ 【已裁决】**Minor 可同步，但不得把 Manifest 字段当成实际能力协商的证明**；实际协商值只来自 SDK 常量（`driver-sdk:817-818`）与 `Welcome.accepted_protocol_minor`（`proto/driver.proto:97`） | M8 从「可选一致性动作」升级为**必过门（G5）**；**禁止**任何「读 manifest minor 判断该驱动是否实现 I-2 RPC」的实现 | 无需再裁决；**实现时必须同步改 5 个 `driver.toml`** |
| **9** | **未通读 `docs/design/nck-semantic-point-refactor.md`（1387 行）全文** | **待确认**。本次只按需检索了上位设计中与 I-2 相关的条款（经 PR-B/PR-C 的引用），**未逐行通读** | 若上位设计存在与 I-2 冲突的条款（例如对接口形态的既有规定），本次**可能未覆盖** | 评审/后续 PR 复核上位设计全文；**本文件不声称已覆盖上位设计全部条款** |
| **10** | **未运行任何测试/构建** | **本次硬约束** | §8.3 的测试名均为**建议**，未执行；§1.1.3 的数字均为**静态解析**（JSON 解析 + 行数统计），**不是**测试结果 | 开工后按 §8.4 执行 |
| **11** | **`definition_api.rs` 的具体函数签名与目录投影的字段顺序** | **待确认**（本文件只定契约与白名单，不写实现签名） | 实现自由 | 实现 PR 内确定，**不得**改变本文件的字段语义与码表 |
| **12** | **预览对 `scope=identity` 的返回值** | **【已裁决】已定：与 Configure 一致**（Preview 对其返回可算的 `point_key`，如实返回 `scope`） | 若改成「预览直接拒绝」则会引入「预览比 Configure 更严」的不一致 | 无需再裁决（§2.5-B4、§9.3-C′、矩阵 **I45**） |
| **13** | **【本轮新增】并发预算的具体载体与落点** | **待确认（实现细节）**：per-driver in-flight 计数放在 **Manager（`manager.rs`，M5）** 还是 **driver-protocol（M11，纯门控函数）**；**预算值 4** 由本文件确定（取值依据见 §6.4-①-⑤），但**载体位置**未定 | 仅影响实现位置，不影响可验收性（矩阵 **I39** 对行为断言，不绑定位置） | 实现 PR 内确定；**不得**改变预算值语义与 `DEFINITION_QUERY_BUSY`（E31）的码/`ErrorKind` |
| **14** | **【本轮新增】`total` / `limit` / `next_cursor` 三个分页字段属本文件新增（PR-C 未枚举）** | **差异（已登记）**。与 **修订 4** 的 `digest` 情形同类：它们是**纯元数据**、**不引入第二份真值**、**不暴露原生语义** | 前端多消费 3 个字段；若 `/api/v1/drivers/{id}/definitions` 的消费者按「无未知字段」严格解析则需同步 | **建议与本文件的 `digest` 同样按「允许的元数据」处理**；若评审要求严格遵循 PR-C 枚举，**可删 `total`/`limit`/`next_cursor` 三个字段而保留 `limit`/`cursor` 的请求侧夹取**（此时「结果数量上限」由**服务端夹取 + 响应长度门**承担，矩阵 I37 的第 ①/②/④/⑤ 条仍成立，第 ③ 条需改为「按 `limit` 多次请求不重不漏」） |
| **15** | **【本轮新增】请求大小上限 64 KiB / 结果上限 500 条 & 256 KiB / 条目硬顶 4096 的取值** | **本文档自定（无既有先例可直接照抄）**。64 KiB：合法请求 < 2 KiB ⇒ ≥30× 余量；500：对齐 Browse 的 1000 但更严（`core-api:834`）；256 KiB：**逐字复用** `session.rs:677`；4096：止损位（今天 42 条） | 若目录将来真的大幅增长（例如新增多个 family），4096 可能需要上调 | 评审确认或给出替代数值；**矩阵 I36/I37 的断言结构不随数值变化**（只替换常量） |
| **16** | **【本轮新增】`REQUEST_TOO_LARGE`（413）不经 `ErrorKind` 映射** | **设计取舍（已登记）**：该码由 HTTP 层（body 门）在**进入 handler 之前**产生，因此 **§6.2 的 `ErrorKind → HTTP` 表覆盖不到它** —— 这是本文件里**唯一**不满足「HTTP 由 `ErrorKind` 决定」的错误路径（§6.1-总则(2)） | 前端需把 413 与 400 区分处理（`code` 仍是 `REQUEST_TOO_LARGE`，body 形状与其它错误一致） | 评审确认可接受；**替代方案**是在 handler 内手动读 body 并自抛 `ErrorKind::Configuration`，但那会**放弃框架级门**（更容易漏），故本文件选前者 |

---

## 12. 核实命令清单（**全部只读**，供评审复核）

### 12.1 第 1 轮（新建本文件）的命令清单

> 全部在 `D:\Mine\Project\mesa`、分支 `docs/nck-i2-boundary`、HEAD `d99a34a` 上执行。
> **未执行任何 git 写操作**；**未运行任何测试/构建/安装**；**未启动 `mesad`/Vite**；**未连接任何设备**。

| # | 命令 / 方法 | 用途 | 结果 |
|---|---|---|---|
| 1 | `git status --porcelain` | 工作树状态 | **空输出**（干净） |
| 2 | `git log --oneline -3`、`git rev-parse HEAD`、`git branch --show-current` | 基线与分支 | `d99a34a…`；分支 `docs/nck-i2-boundary` |
| 3 | `(Get-Content <file>).Count`（对 15+ 个文件） | 行数（**不用 `Measure-Object -Line`**） | 见 §1.1.3 |
| 4 | `ConvertFrom-Json` 解析 `definitions/828d/r1.json` + 分组/计数 | 42 定义 / 62 `golden_ref` / scope / status / target 组合 / arguments / area / module 来源 | 见 §1.1.3 |
| 5 | `ConvertFrom-Json` 解析 `definitions/828d/DIGESTS.lock` | 42 条 lock、`format_version`、`family` | 见 §1.1.3 |
| 6 | `ConvertFrom-Json` 解析 `presets/828d.json`、`tests/fixtures/legacy-828d-wire.json`、`tests/fixtures/nck-point-key-golden.json` | 62 模板 / 62 wire golden / 55-5-77 | 见 §1.1.3 |
| 7 | 交叉核对：62 个 `golden_ref` → 模板 id（`828d.<area>.<block>.<variable>.<entry>`） | 覆盖度 | **hit=62 / miss=0** |
| 8 | 交叉核对：6 条 `scope=identity` 定义的 `golden_ref` → 模板 id | 缺陷复现 | **6/6 命中**（§9.1 明细） |
| 9 | `Select-String` 全仓 grep（`crates/**`、`drivers/sinumerik-nck/src/**`、`apps/mesa-web/src/**`、`tests/driver-contract/tests/**`、`proto/driver.proto`、`.github/workflows/*.yml`、`scripts/*.py`） | 逐条取得 `文件:行号` 证据 | 见全文引用 |
| 10 | 模块级「无设备」grep：`definition.rs`/`targets.rs`/`compiler.rs`/`native.rs`/`value.rs`/`address.rs`/`codec.rs` 对 `NckClient\|client::\|tokio::net\|TcpStream\|async fn\|\.await\|session\|connect(` | 特别要求 B 的代码级证据 | **七个文件全部零命中** |
| 11 | `Get-Content` 读取 `.github/workflows/_rust-quality.yml`、`_rust-canonical.yml`、`_rust-perf.yml`、`_web.yml` | CI 门禁口径 | 见 §8.4 |
| 12 | `python -c "…"`（统计 `scripts/contract_suites.py` 名单） | 套件数 | **该命令因 PowerShell 解析失败未执行**（未产生结果）；套件数 14+12+3=**29** 由直接阅读 `contract_suites.py:13-52` 得出 |

**第 1 轮明确未做**：未修改任何生产代码 / 测试 / 配置 / 其它文档（**只新建本文件**）；
未运行 `cargo`/`pnpm`/`python` 中的任何测试或构建；未执行任何安装类操作；
未启动 `mesad`、未启动 Vite；未连接任何真实设备或机床；未执行任何 git 写操作。

### 12.2 第 2 轮（本轮评审裁决修订）的命令清单

> 全部在 `D:\Mine\Project\mesa`、分支 `docs/nck-i2-boundary`、**HEAD `a810c42`** 上执行。
> **未执行任何 git 写操作**（只读 git：`rev-parse` / `status` / `log` / `show` / `ls-files`）；
> **未运行任何测试/构建/安装**；**未启动 `mesad`/Vite**；**未连接任何设备或机床**。

| # | 命令 / 方法 | 用途 | 结果 |
|---|---|---|---|
| 1 | `git rev-parse --abbrev-ref HEAD`、`git rev-parse --short HEAD`、`git status --porcelain`、`git config core.autocrlf`、`git log --oneline -3` | 基线、分支、工作树、换行设置 | `docs/nck-i2-boundary` / `a810c42` / 工作树**干净** / `core.autocrlf=true` |
| 2 | `git ls-files --eol docs/design/` | **行尾实测**（本文件与同目录其它文档对比） | **修订前**：本文件 `w/lf`（唯一），同目录其它 5 个 `.md` 均 `w/crlf`；索引侧全部 `i/lf` ⇒ **本轮已把本文件归一为 CRLF**（实测见 §14.6） |
| 3 | `(Get-Content <file>).Count`（对 12 个文件） | 行数（**不用 `Measure-Object -Line`**） | `proto/driver.proto` 453、`core-api/src/lib.rs` 3397、`driver-manager/src/manager.rs` 679、`driver-sdk/src/lib.rs` 2358、`compiler.rs` 798、`definition.rs` 1429、`lib.rs` 2018、`targets.rs` 463、`presets.rs` 520、`native.rs` 673、`management_api.rs` 3094、`resource_contract.rs` 461、`descriptor_contract.rs` 1094 |
| 4 | **PowerShell 逐行解析 `proto/driver.proto` 的 `oneof body` 区间**（`:22` → `:77`），按 `^\s*<Type>\s+<name>\s*=\s*(\d+)\s*;` 抽取 `(tag, 类型, 名称)` | **修订 1 的核心：逐字段核对** | **成员数 = 35**；tag 集合 = `{10..44}`**连续无空洞**；重复 = **0**；`Select-String '^message\s'` ⇒ 顶层 message **56**；`Select-String '^\s+oneof\s'` ⇒ oneof **2**（`body` 35 成员 + `kind` **19** 成员）。**⇒ 第 1 轮的「44 个」为计数错误**（详见 §7.2.1） |
| 5 | `Select-String` 读 `crates/driver-protocol/src/lib.rs` 的版本常量 | 版本号 | `PROTOCOL_MAJOR = 1`（`:23`）、`PROTOCOL_MINOR = 6`（`:29`）、`PROBE_RPC_MIN_MINOR = 2`（`:34`）、`EVENT_PLANE_MIN_MINOR = 3`（`:39`）、`TASK_SCHEDULE_MIN_MINOR = 5`（`:44`）、`CONTROL_MODEL_MIN_MINOR = 6`（`:49`）、`MAX_FRAME_LEN = 4 MiB`（`:52`） |
| 6 | **`Get-Content` 读 `crates/driver-protocol/src/lib.rs:674-714`** | **本轮新发现（关键）** | 存在 P1 invariant 单测 `in_tree_manifest_minor_matches_protocol`（`:683`），**遍历 5 个 in-tree `driver.toml`**（`:690-696`）断言 `protocol_minor == PROTOCOL_MINOR`（`:709-712`）⇒ 第 1 轮「`protocol_minor` 未被任何门使用」**不准确**（§7.5/M8） |
| 7 | `Select-String` 全仓并行 grep：`crates/*/src/*.rs` + `apps/mesad/src/*.rs` 对 `Semaphore\|acquire(\|rate_limit\|RateLimit\|concurrency` | **修订 3 的复用性核查** | **零命中** ⇒ **仓库当前无并发闸门可复用**；I-2 的并发预算**必须新增**（§6.4-②，如实登记） |
| 8 | `Select-String` 读 `crates/driver-manager/src/session.rs`、`manager.rs`、`temp_op.rs`、`probe.rs` 的超时/大小门/预算常量 | **修订 3 的载体核查** | `REQUEST_TIMEOUT = 10s`（`session.rs:25`）、`DRIVER_STARTUP_TIMEOUT = 6s`（`:280`）、descriptor 5s 超时（`:667`）+ 256 KiB 门（`:677`）、`MAX_TEMP_ATTEMPTS = 2`（`temp_op.rs:25`）、descriptor 第二道门（`manager.rs:57-62`）、`probe_supported`（`probe.rs:149-151`） |
| 9 | `Select-String` 读 `crates/core-api/src/lib.rs` 的路由表与 Browse 限额 | **修订 3 的分页先例** | 路由表 `:3236-3342`（descriptor `:3241-3244`、probe `:3249`）；**Browse `limit` 缺省 50 / 上限 1000**（`:834`）；**router 无 `DefaultBodyLimit` / `TimeoutLayer`**（`:3341` 直接 `.with_state(state)`） |
| 10 | `Get-Content` 读 `axum-core-0.5.6/src/extract/default_body_limit.rs:7-8`（本地 cargo registry）+ `Cargo.toml:72` | **修订 3 的框架级请求大小门** | `Json`/`Bytes` **默认拒绝 > 2 MB**；`axum = "0.8"` ⇒ I-2 只需**收紧到 64 KiB**（§6.4-①-①） |
| 11 | `Get-Content` 读 `crates/core-types/src/lib.rs:622-650` | `ErrorKind` 变体数与行号 | 枚举 `:624-634`，**9 个变体在 `:625-633`**（第 1 轮写 `:624-632`，**已修正**）；`as_str()` `:637-649` |
| 12 | `Get-Content` 读 `docs/design/nck-prb-driver-compiler-boundary.md:629-643` | **修订 2 的 D1–D11 权威编号** | 取得 D1（Point Kind）/D4（索引范围）/D10（26/130 入口身份）/D11（Key 一致性）的原文语义 ⇒ §5.4.1 的逐条对照表 |
| 13 | `Get-Content` 读 `crates/driver-sdk/src/lib.rs:99-132`、`:1126-1144`、`:1178-1179` | **修订 1 的兼容性证据** | `Driver` trait `:100-132`（`descriptor()` 默认实现 `:105-123`）；`GetDescriptor` 分发**不查 `session.entries`**（`:1126-1144`）；**兜底分支 `other => tracing::debug!(…, "unexpected envelope ignored")`（`:1178-1179`）⇒ 未知消息不 panic、不断连** |
| 14 | `Get-ChildItem … -Filter driver.toml`（`drivers/` + `tests/`） | **修订 1 的 M8 影响面** | 实测 **5 个** `driver.toml`，`protocol_minor` **全部 = 6** |
| 15 | `ConvertFrom-Json` 重核 `definitions/828d/r1.json`、`DIGESTS.lock`、`presets/828d.json`、`tests/fixtures/legacy-828d-wire.json`、`tests/fixtures/nck-point-key-golden.json` + 62↔62 交叉核对 | **不沿用未核对的数字**（复核第 1 轮结论） | `definitions=42`、`golden_ref=62`（20×2 + 22×1）、`scope` `collect=36/identity=6`、`semantics_status` `verified=24/parameter_identity=16/candidate=2`、`arguments` 非空 **2** 条、`area` `A=4/B=1/C=16/N=1/V=20`、`module` `drive_entry_module=20/fixed=22`、`r1.json` **1331 行**、`DIGESTS.lock` **258 行 / entries=42**、`presets templates=62`、`wire entries=62`、`golden 55/5/77`、**hit=62 / miss=0** ⇒ **第 1 轮这些数字全部复现** |
| 16 | `Get-ChildItem tests/driver-contract/tests/*.rs`、`drivers/sinumerik-nck/tests/*.rs` | 套件清单实测 | 合同套件 **29 个文件**（与 `contract_suites.py` 一致）；NCK 侧**存在** `identity_browse.rs`（I31 引用的套件**确实存在**） |
| 17 | `Get-FileHash <file> -Algorithm SHA256` + `[System.IO.File]::ReadAllBytes` 计 CR/LF + `[regex]::Matches` 复核 | **行尾与摘要实测** | 修订前：`SHA-256 = 7E536554011CBC6CDCF7C6BC177CA755DFE1F0D3E1F03FAC55EE9F275006400C`、**CR=0 / LF=1211**（LF-only）；修订后见 §14.4 |
| 18 | **`edit` 工具定向替换**（本轮唯一写操作，**只针对本文件**） | 执行四项约束 + 逐条裁决的修订 | 见 §14 的差异表 |

**第 2 轮明确未做**：**只修改本文件**（`docs/design/nck-i2-definition-catalog-api-boundary.md`）；
**未修改 `proto/driver.proto`**、**未修改任何代码 / 测试 / 配置 / 其它文档**（含三份冻结设计文档）；
未运行任何测试 / 构建 / 安装；未启动 `mesad` / Vite；未连接任何真实设备或机床；
**未执行任何 git 写操作**（commit/push/checkout/stash/reset/PR/tag 均未执行；只用只读 git）。

---

## 13. 结论摘要（给评审）

1. **特别要求 A 的三段式结论**：
   **(i)** 现有能力已覆盖「目录数据（P1/P2）、浏览叶 `identity` 块（P3/P4）、无连接管理面通道（P5–P7）、
   结构化错误通道（P11）、Core 零语义护栏（P14）」；
   **缺口分两类**：*数据缺失*（G1–G8：`scope`/`targets`/`target_required`/`arguments[]` 元数据/
   `semantics_status`/`source[]`/`label`/`golden_ref`）与 ***通道缺失***（G9：**没有任何既有 RPC 能承载调用方参数**）。
   **(ii)** 比较 **C1 扩展 Browse 叶载荷** / **C2 新增只读管理 RPC** / **C3 扩展 Descriptor** /
   **C4 复用 Probe** / **C5 用 `filter` 承载参数**：C1 零 wire 但只能覆盖目录；
   C3 与 PR-C「不改 Descriptor 契约」直接冲突且仍覆盖不了预览；C4 需要连接（违反特别要求 B）；
   C5 属语义滥用；**C2 是唯一同时满足「无 Endpoint / 离线可算 / Core 零语义 / 可断言」的形态**。
   **(iii)** **最终选择：新增一条只读管理路由（I-2b），并明确它不是新框架**（与 `GetDescriptor` 逐字同构）；
   **决定性理由 = 权威 Preview 需要参数化输入，而参数通道在现有 RPC 家族中不存在**。
   附带 **I-2a**（零 wire 的可选先行项，只解决元数据可得性）。
   **wire additive 增补【已裁决】已获原则批准**（§11-1；**生产代码仍 HOLD**）。
2. **特别要求 B 的判定**：**Preview 完全离线可算 —— 不需要 Endpoint、不需要连接、不需要真机**；
   依赖链 = `canonical_identity`（`definition.rs:1091`）→ `TargetRef::value_for_path`（`targets.rs:289`）+
   `encode_typed_value`（`point.rs:207`）+ `render_identity_template`（`definition.rs:1137`），
   全部纯函数；**七个模块对 `NckClient`/`tokio`/`.await` 零命中**；唯一设备触点是 `client.rs:55`（只被
   `identity.rs:434` 与采集路径使用）。**架构落点 = Driver 进程内、无连接状态下计算**
   （Core 被 `point.rs:16-19` + PR-B `:229` + P9 护栏三重禁止）。
   **「无连接时 Driver 能只加载定义目录即响应」有现成先例**：`GetDescriptor`（`driver-sdk:1126-1144`）
   不查 `session.entries`，Manager 的 descriptor 路径从不发 `OpenConnection`（`manager.rs:328-361`）。
   **边界情况**：`nck_native`+`user` 不重算（B3）；`scope=identity` 目录如实返回、预览与 Configure 一致（B4）；
   `nck://identity` 探针树**仍需真机**（B5）；840d-sl 返回空列表 + 说明（B6）；目录自检失败 503（B7）。
3. **六项问题在本文档的落点**：接口入口 **§3**；定义目录 **§4**（请求/响应示例 3 条真实定义 +
   **18 字段语义表**（含分页信封 3 字段）+ 26/130 保留规则 + **§4.6 执行边界**）；
   权威 Preview **§5**（4 个请求/响应示例 + 与 D11 的 5 条关系 + **§5.4.1 的 D1–D11 逐条对照**）；
   错误与安全 **§6**（**31 条错误码表**（E1–E31）+ `ErrorKind` 映射规则 + **§6.2.1 码→kind 归属表** +
   **11 条只读证据 R1–R11** + **§6.4 执行边界**）；
   兼容性 **§7**（+ **§7.2.1 逐字段核对** + **§7.7 版本兼容矩阵**）；验收与归属 **§8**（文件范围
   **新增 3 / 修改 11（落在 15 个文件）/ 冻结 10**、
   **验收矩阵 45 行 I1–I45**、CI 门禁 **G0–G11**）。
   **§4 字段语义表共 19 行**（16 行定义字段 + 3 行分页信封 `total`/`limit`/`next_cursor`）。
4. **兼容性逐条核对结果**：Core 零语义 ✅（P9 护栏 + 只按 `ErrorKind` 映射，归属表见 §6.2.1）；
   既有协议 ✅（**【已裁决修正】既有 35 个 tag（10–44）逐字不动**、Major 仍 1；
   **仅 additive 增补 tag 45–48 + Minor 6→7**；逐字段核对见 §7.2.1）；
   62 条 wire golden ✅（NCK 业务帧，与 IPC 无关）；其它 4 个驱动 ✅（SDK 默认实现 ⇒ **源码**零改动；
   **但 `driver.toml` 的 `protocol_minor` 必须同步，5 个文件**，§7.5）；
   Descriptor 契约 ✅（2/0 不动，明确否决 C3）；PR-A 冻结的 `point.rs`/`binding_gate.rs` ✅；
   PR-C / PR-D 边界 ✅（`AddDeviceFlow.tsx`+测试仍归 PR-D）；
   **版本兼容矩阵 §7.7（V1–V6）覆盖裁决点名的五类**；**未知消息不异常退出 ✅**（`driver-sdk:1178-1179`）。
5. **`scope=identity` 缺陷边界**：**Driver 侧缺陷**（权威修复点 = `presets.rs`/目录查询层，PR-C §10.2.3）；
   **I-2 不顺带修复**（理由 §9.3：会改 Configure 准入语义、扩 PR-B 码表、触碰浏览树冻结断言）；
   **【已裁决】同意继续独立登记该 Driver 缺陷**（§9.3-D）；
   I-2 的唯一作用是**让 `scope` 可得**，从而把 PR-C 的 A40 从「一律禁用」升级为「可按真值区分」；
   **I-2 如实返回 `scope`；Preview 与当前 Configure 的计算语义保持一致；准入判定归 PR-C**（§9.3-C′）；
   **【硬性约束】禁止「目录侧静默删 6 条 + Browse 保留 62 模板」**（§9.3-C′，矩阵 **I44**）；
   **缺陷修复前 PR-C 必须保持 fail-closed**（§9.4 六行映射）。
6. **本轮（第 2 轮）修订的落点**：**§14** 给出四项约束 + 逐条裁决的**差异表**（含行号与关键原文），
   评审可**只复核 §14**。
7. **如实登记的无法确认/冲突项共 16 条**（§11；第 1 轮 12 条 + 本轮新增 4 条）：
   其中第 1 轮登记的 **4 条关键项已裁决**（wire 增补、路由命名/`POST` 语义、`family` 缺省、`digest`），
   **1 条仍未决**（预览是否支持 `nck_native`，§11-5）、**1 条为未通读上位设计全文**（§11-9）、
   其余为实现细节与本轮新增的取舍项（§11-13…16）。

---

## 14. 第 2 轮评审裁决落点（**【已裁决】/【已裁决修正】差异表**，供评审只复核差异）

> **本节是第 2 轮修订的索引**。四项约束与「五、其它具体裁决」**逐条给出落点行号与关键原文**。
> 行号为**修订后**的行号（`(Get-Content <file>).Count` 口径）。

### 14.1 修订 1：协议兼容性（**不只检查 Minor 数字**）

| 要求 | 落点（修订后行号） | 关键原文 / 结论 |
|---|---|---|
| **逐字段核对既有字段编号**、给出**真实计数**与核对方法 | **§7.2.1**（引用点：§1.3-③、§3.1-④、§3.2、§7.2） | **`oneof body` 成员数 = 35**（不是 44）；tag 集合 `{10..44}` **连续无空洞、无重复**；顶层 `message` **56**（+4 → 60）；全文件 `oneof` **2**（`body` 35 成员 + `kind` 19 成员）；**35 行逐字段核对表**（tag → 类型 → 名称 → 行号） |
| **修正文档中所有受影响的表述** | §1.3-③（2 处）、§3.1-④、§3.2、§7.2、§8.1.2-M1、§8.3-I27、§13-4 | **全部「44 个 tag」→「35 个 tag」**；**唯一保留 `44` 的合法用法** = 「最大 tag 编号」或「末项 `event_batch = 44;`」；I27 断言口径从「数字」升级为「§7.2.1 的逐字段表 + 新 tag 集合恰为 `{45,46,47,48}`」 |
| **版本兼容矩阵**（5 类 → 期望行为 → 返回码/`ErrorKind` → 可验收测试） | **§7.7（V1–V6 六行）** | V1 新 SDK 默认实现**快速 `Unsupported`（不等待超时）** → I9；V2 新 Manager ↔ 旧驱动**门控先于发帧** → I10；V3 旧 Manager ↔ 新驱动**静默降级** → **I34（新增）**；V4 **未知消息不异常退出** → **I35（新增）**；V5 **`ErrorKind` 确定性 + 不误报设备离线** → §6.2.1 + I9；V6 正常路径对照 → I27/I28 |
| **能力协商表述复核**：`driver.toml` 的 Minor **不得**当作实际能力协商的证明 | **§7.5**（【已裁决修正】，含 5 个文件表）、**§7.6**、**§11-8** | 实际协商值只来自 SDK 常量（`driver-sdk:817-818`）与 `Welcome.accepted_protocol_minor`（`proto/driver.proto:97`）；**禁止**「读 manifest minor 判断是否实现 I-2 RPC」；**另修正第 1 轮的错误**：`protocol_minor` **确实有门**（P1 invariant 单测 `driver-protocol:679-714`）⇒ **M8 必需且 5 个文件** |

### 14.2 修订 2：Preview 必须与 Configure **共用权威验证路径**

| 要求 | 落点 | 关键原文 |
|---|---|---|
| **必须先完成与 Configure 一致的校验**，然后才生成身份 | **§5.4.1**（规范条文 4 条） | 「**Preview 必须先完成与 Configure 一致的全部校验 —— 定义查找、Revision、family、Target、Arguments、以及地址可编译性 —— 然后才生成身份（`point_key`）。**」 |
| **只跳过 D11**，理由是请求本身尚未持有最终 Key；**不是**跳过 D1–D10 | **§5.4.1**（条文 2）+ §5.4 表格第 4 行的【已裁决修正】 | 「**Preview 应当跳过的、且只跳过 D11。跳过的唯一理由是请求本身尚未持有最终 Key…这不是跳过 D1–D10 的理由**」；**逐条对照表**收录 **D1/D2/D3/D4/D5/D6/D7/D8/D9/D10 + 身份重算 + D11**，D1–D10 全部「✅ 执行」，D11「❌ 唯一跳过项」 |
| **新增跨接口一致性测试**（编号 + 断言要点） | **I32**（§5.4.2 + §8.3） | 断言原文：「**相同合法输入下，Preview 返回的身份必须逐字等于随后 Configure 所接受的身份。**」五步断言（42 条 × Preview→写入 `point_key`→真实 Configure 接受→`K_p == K_c` 逐字） |
| **定义已变更 / Revision 不受支持 ⇒ 明确错误**，不得给「看似有效但无法配置」的身份 | **§5.4.3**（4 行情形表）+ **E29**（`POINT_KEY_MISMATCH`）+ **I33** | 「**不得为了「体验顺畅」而在上述任一情形返回一个「看起来能用的 `point_key`」**」；E29 消息含**持久化 Key + 重算 Key + `variable_id` + `revision`** 三要素 |

### 14.3 修订 3：只读接口也必须设**执行边界**

| 要求 | 落点 | 取值 / 关键原文 |
|---|---|---|
| **请求大小上限** | **§6.4-①-①**（+ §5.1、§4.6）；错误 **E30**（413 `REQUEST_TOO_LARGE`） | 预览 `POST` body ≤ **64 KiB**（合法请求 < 2 KiB ⇒ ≥30× 余量）；**框架级已有 2 MB 默认门**（`axum-core-0.5.6/src/extract/default_body_limit.rs:7-8`）⇒ 本设计只是**收紧** |
| **结果数量上限** | **§6.4-①-②**；§4.6；错误 **E4** | 单页 ≤ **500**；响应 JSON ≤ **256 KiB**（逐字复用 `session.rs:677`）；过滤后条目 > **4096** 视为驱动违约 ⇒ 503 `DEFINITION_QUERY_TOO_LARGE` |
| **分页** | **§4.1**（`limit`/`cursor` 行）、**§4.2**（`total`/`limit`/`next_cursor`）、**§4.3**（3 行）、**§6.4-①-③**；测试 **I37** | `limit` 缺省 **100**/上限 **500**（对齐 Browse `core-api:834` 的形状、更严的值）；`cursor` **不透明**（= 上页末条 `variable_id`，稳定性由 `definition.rs:8-10` 保证）；`next_cursor` 无下一页为 `null` |
| **超时** | **§6.4-①-④**；错误 **E2/E3**；测试 **I38** | RPC **5s**（`session.rs:667`）+ 启动 **6s**（`session.rs:280`）+ attempt ≤ **2**（`temp_op.rs:25`）⇒ **单次调用最坏 ≤ 22s**；**不新增第二套超时栈**（`REQUEST_TIMEOUT = 10s` 不动，`session.rs:25`） |
| **并发预算** | **§6.4-①-⑤**；错误 **E31**（503 `DEFINITION_QUERY_BUSY`）；测试 **I39** | 同一 `driver_id` in-flight ≤ **4**，超出**立即拒绝（不排队）**；**如实说明**：仓库**无任何并发闸门可复用**（零命中）⇒ 这是**唯一必须新增**的机制 |
| **复用现有 Manager 的资源控制机制**（给 `文件:行号`） | **§6.4-②**（9 行表 = **8 行复用 + 1 行新增**） | **复用 8 项**：`temp_op.rs:25`（attempt ≤ 2）、`session.rs:280`（6s）、`session.rs:654-677`（5s + 256 KiB 门）、`manager.rs:57-62`（**第二道** 256 KiB 门）、`driver-protocol/src/lib.rs:52`（4 MiB 帧门）、`core-api/src/lib.rs:834`（Browse 分页形状）、`axum-core-0.5.6/src/extract/default_body_limit.rs:7-8`（2 MB 默认门）；另有 `session.rs:25`/`:902`（10s `call_with_timeout`）属「**显式不改**」项；**新增 1 项**（per-driver 并发预算，预算 4） |
| **四条只读性断言**（与 R1–R7 合并，给测试名） | **§6.3 的 R8–R11** + **§6.4-③** 的 A1–A4；矩阵 **I40–I43** | 测试名：`definitions_rpc_opens_no_cnc_connection`（**R8**）、`definitions_rpc_reads_no_live_machine_values`（**R9**）、`definitions_rpc_does_not_modify_acquisition_plan`（**R10**）、`definitions_rpc_has_no_persistent_side_effects`（**R11**） |

### 14.4 修订 4：`scope=identity` **不在 I-2 中隐式修复** + 禁止捷径

| 要求 | 落点 | 关键原文 |
|---|---|---|
| **同意继续独立登记 Driver 缺陷** | **§9.3-C / §9.3-D**（【已裁决】）、§9.2 归属表 | 「**【已裁决】同意继续独立登记 Driver 缺陷**」（建议 `PR-B3-nck-scope-collectability`） |
| **I-2 如实返回 `scope`** | §4.3（`definitions[].scope` 行）、§4.4 的【已裁决】注、**§9.3-C′ 的 C1**；矩阵 **I4/I44** | 「`scope` **如实返回**（`collect`/`identity` 原值，**不得**在目录层过滤或删除）」 |
| **Preview 与当前 Configure 的计算语义保持一致** | §2.5-B4、§5.4、**§9.3-C′ 的 C2**；矩阵 **I45** | 6 条 `scope=identity` 定义：Preview **返回可算的 `point_key`**（与 Configure **行为一致**），**不是**「预览比 Configure 更严」 |
| **PR-C 根据 `scope` 决定是否允许进入采集配置** | **§9.3-C′ 的 C3** + §9.4 六行映射（A13/A40/A43） | I-2 **只提供元数据**，**不做**准入判定（准入属 PR-C 前端 fail-closed） |
| **【硬性约束】禁止「目录接口静默删掉六条身份变量，同时保留旧 Browse 中的 62 条模板」** | **§9.3-C′**（独立小节，含两条理由）+ **§10.2 第 14 条**；矩阵 **I44** | 「**不得为了使 UI 好看而在目录接口中静默删掉六条身份变量，同时又保留旧 Browse 中的 62 条模板。**」理由①：造成「**第二份定义真值**」（目录说「没有」、Browse 说「有」）；理由②：让**准入策略伪装成数据事实** —— 调用方**无法区分**「不存在」与「被过滤」，PR-C 的 **A40 fail-closed 无从判定**（它需要**看到** `scope`）。**可验收形态**：目录 42 条（`identity` **恰 6 条**）+ 模板/浏览树 **62 条**，**不存在**「目录少 6 + 浏览树 62」这一组合 |

### 14.5 「五、其它具体裁决」逐条落点（**【已裁决】标注**）

| # | 裁决 | 落点 | 关键原文 / 处理 |
|---|---|---|---|
| 1 | `POST /api/v1/drivers/{id}/definitions/identity-preview` **批准**；**POST 表示带参数的只读计算，不代表写入** | **§5.1**（显式说明块）、**§11-2**、**§6.3-R1**；§0 头部「生产代码仍 HOLD」 | 「**本端点的 `POST` 表示「带参数的只读计算」，不代表写入。**」+「**「HTTP 方法」不是「是否写入」的判据 —— 副作用才是。**」+ 先例 `core-api:3245-3249`；**最终判据 = R8–R11 / I40–I43** |
| 2 | `family` 缺省：**目录查询可省略**（返回全部已发布 family）；**Preview 必须显式指定** | **§4.1**（`family` 行 + 【已裁决】注）、**§5.1**（`family` 行）、**§11-3**；矩阵 **I5** | 「【已裁决】目录查询**可省略** `family`；省略即返回**全部已发布 family**」；「【已裁决】必须显式指定，无缺省…**缺少 `family` ⇒ 400 `INVALID_REQUEST`**」 |
| 3 | **26/130**：**保留 `entry` 的权威区别**；**不需要向普通 UI 暴露原始 Module 数字** | **§4.4-②**（`是否把 26/130 数字放进响应` 行）、§5.2 用例 C；矩阵 **I3/I14** | 「【已裁决】**保留 `entry` 的权威区别**…**不需要向普通 UI 暴露原始 Module 数字**」 |
| 4 | `digest`：**可以作为定义版本与完整性元数据返回**（此前登记为「超出 PR-C 枚举」，现裁决为允许） | **§4.4-③**（改为【已裁决】允许）、**§11-4**（【已裁决修正】） | 「**【已裁决修正】裁决结论：允许**把 `digest` 作为**定义版本与完整性元数据**返回」；原「可直接删除该字段」**作废** |
| 5 | **I-2a 保持可选**；**不得生成第二份变量定义真值** | §1.3-②（I-2a 行）、**§8.5 内容行**、§8.3-I31；矩阵 **I31** | 「【已裁决】I-2a **保持可选**，且**不得生成第二份变量定义真值**」 |
| 6 | `driver.toml` Minor：**可同步**，但**不得把 Manifest 字段当成实际能力协商的证明** | **§7.5**、**§7.6**、**§11-8**、§8.1.2-M8 | 「【已裁决】**`driver.toml` 的 Minor 可同步，但不得把 Manifest 字段当成实际能力协商的证明。**」 |

### 14.6 行尾（CRLF）实测

| 指标 | 修订前（HEAD `a810c42`） | 修订后（本轮，归一后实测） |
|---|---|---|
| `git ls-files --eol`（`docs/design/`） | 本文件 **`w/lf`**（全目录**唯一**），同目录其它 5 个 `.md` 均 `w/crlf` | 本文件 **`w/crlf`**（与同目录一致） |
| 字节级 CR / LF 计数（`[System.IO.File]::ReadAllBytes`） | **CR = 0** / **LF = 1211** | **CR = 1802** / **LF = 1802**（**CR == LF** ⇒ 每行恰一个 CRLF，**无孤立 LF**） |
| 行数（`(Get-Content <file>).Count`，**不用** `Measure-Object -Line`） | **1211** | **1802** |
| 文件字节数与 SHA-256 | 122727 / `7E536554011CBC6CDCF7C6BC177CA755DFE1F0D3E1F03FAC55EE9F275006400C` | **由交付报告给出（本文件刻意不自带该值）**。**理由（自指）**：本表自身也是文件内容，任何写进文件内的摘要都会因写入本身而失效，**无法成为不动点**；写一个「上次的」值只会误导复核。**权威取得方式**（只读）：`Get-Item docs/design/nck-i2-definition-catalog-api-boundary.md \| Select-Object Length` 与 `Get-FileHash docs/design/nck-i2-definition-catalog-api-boundary.md -Algorithm SHA256`。（**行数 1802 与 CR/LF 计数 1802/1802 不受影响** —— 本表行数固定。） |
| BOM | 无（首 3 字节 `35,32,73` = `"5 I"`） | **无**（首 3 字节 `35,32,73`，保持不变） |
| 索引侧（`git ls-files --eol` 的 `i/`） | `i/lf` | **`i/lf` 不变**（`core.autocrlf=true` 的正常形态）⇒ `git diff` 仍按**规范化文本**比较，**不会**产生整文件行尾翻转的伪差异 |

> **说明**：本文件在**修订前**是全目录**唯一**的 LF-only 文件（`git ls-files --eol` 实测 `w/lf`），
> 与本目录其它设计文档（`a22-*`、`c3-*`、`nck-prb-*`、`nck-prc-*`、`nck-semantic-*` 均 `w/crlf`）**不一致**。
> 本轮按任务要求**归一为 CRLF**（用 `[System.IO.File]::WriteAllText` + `UTF8Encoding($false)` 写出，
> 先 `\r\n → \n` 再 `\n → \r\n` 归一，**不改变任何字符内容**），
> 且因索引侧保持 `i/lf`，提交后**不会有整文件行尾翻转**。

### 14.7 本轮交付核对（只读命令实测）

| 项 | 实测值 |
|---|---|
| 修订后文档**绝对路径** | `D:\Mine\Project\mesa\docs\design\nck-i2-definition-catalog-api-boundary.md` |
| 总行数（`(Get-Content <file>).Count`，**不用** `Measure-Object -Line`） | **1802**（修订前 1211；净增 **591** 行） |
| SHA-256 | **由交付报告给出（本文件刻意不自带该值，理由见 §14.6 的自指说明）**；复核命令：`Get-FileHash docs/design/nck-i2-definition-catalog-api-boundary.md -Algorithm SHA256` |
| `git status --porcelain` | **仅 1 行**：` M docs/design/nck-i2-definition-catalog-api-boundary.md`（**无其它文件**） |
| 验收矩阵最终行数 | **45 行**（I1–I45）。第 1 轮 31 行 ⇒ **新增 14 行**（I32–I45）、**修改 4 行**（I5 补 `family` 缺省断言、I9 强化 kind 断言、I27 修正 44→逐字段表口径、I31 标注 I-2a 仍可选），其余 **27 行逐字保留** |
| 错误矩阵最终行数 | **31 行**（E1–E31）。第 1 轮 28 行 ⇒ **新增 3 行**（E29 `POINT_KEY_MISMATCH` 迟发拒绝、E30 `REQUEST_TOO_LARGE`、E31 `DEFINITION_QUERY_BUSY`）、**修改 2 行**（E4 扩充条目数门、E5/E6 明确 `ErrorKind::Unsupported`），其余 **26 行逐字保留** |
| 只读证据行数 | **11 行**（R1–R11）。第 1 轮 7 行 ⇒ **新增 4 行**（R8–R11） |
| 版本兼容矩阵行数 | **6 行**（V1–V6，§7.7），覆盖裁决点名的 **5 类** |
| 执行边界项数 | **5 项**（§6.4-①：请求大小上限 / 结果数量上限 / 分页 / 超时 / 并发预算） |
| 复用既有机制 / 新增机制 | **复用 8 项**（§6.4-② 表中 8 行；另有 `REQUEST_TIMEOUT = 10s` 一行属「显式不改」）/ **新增 1 项**（per-driver 并发预算）——§6.4-② |
| 逐字段核对表 | **35 行**（§7.2.1）+ 6 行核对结论 |
| 文件范围 | 新增 **3** / 修改 **11**（落在 **15 个文件**）/ 冻结 **10** |
| CI 门禁 | **G0–G11**（**逐字保留**，仅 G7 扩充 1 条子项） |
| 六项问题 / I1–I31 / G0–G11 / 特别要求 A・B | **全部逐字保留**（本轮只做定向修正与增补） |
