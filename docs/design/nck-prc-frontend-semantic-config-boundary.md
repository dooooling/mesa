# PR-C 实施边界与验收清单：828D 语义点位前端配置

- 状态：**边界与验收设计稿（待评审批准后开工）**
- 结论口径：本文件是**实施边界与验收清单**，不是实现。§1–§9 的事实均以只读命令在
  **`docs/nck-prc-boundary`**（PR-B 合并基线 HEAD `152a3e2`，`feat/nck-prb-compiler` → PR #111）
  的工作树上核实，引用一律给 `文件:行号`；无法确证的一律标注**待确认**，**不编造**。
- **本轮新增实测（评审要求，见 §9.1 / §9.3-1 / §2.5）**：在
  **`docs/nck-prc-boundary` / HEAD `871fd11`**（PR #112 待修订分支）上执行了
  `pnpm typecheck` 与 `pnpm test`（基线 **33 文件 / 383 用例**，两者均退出码 `0`，见 §9.1），
  并在**独立临时副本**内完成「仓库级 golden fixture 可导入性」最小探针（§9.3-1 / §2.5）。
  除该探针所在的临时副本外，**未修改任何生产代码 / 配置 / 其它文档**；临时副本已删除。
- **本轮（第二轮定向修订，按评审最终裁决）**：对本文档做**小范围定向修订**（修订而非重写），
  共 **4 项修订 + 2 条纪律记录**：① **I-2 升级为「完整语义配置的 P0 功能依赖」【已裁决修正】**（§1.4.3、§1.7、§10.1、§12-2）；
  ② 新增 **Target 元数据降级规则状态表**（§1.4.4，并在 §6、§7 引用，矩阵补齐 **A42–A44**）；
  ③ 新增三项验收要求（非 NCK Browse 回归、Native 不自动追加 `~<n>`、生产构建验证，见 §7.8、§6.2、§11 G11）；
  ④ **PR-C / PR-D 文件边界不变**（§2.2、§8.3、§12-9）。两条纪律记录见 §9.4 与 §11（**G0**）。
  **上一轮被认可的全部内容（§0 四路证据、C10 十步链、七项交付物、§10 登记、§2.5 探针实测结论、E2E 边界等）逐字保留。**
- 上位文档（**本轮不得修改**，只引用）：
  `docs/design/nck-semantic-point-refactor.md`（1387 行，设计冻结稿）、
  `docs/design/nck-prb-driver-compiler-boundary.md`（1167 行，PR-B 已裁决）。
- 术语沿用 PR-B：**Task Binding Kind**（任务层路由，`mesa.resources.v1` / `nck.points.v1`）与
  **Point Kind**（点位层，`nck_variable` / `nck_native`）是两个不同层次，**不得**混用。

## 0. 本轮授权范围（硬约束，先声明）

| 项 | 内容 |
|---|---|
| 允许（首轮） | **只新建本文件** `docs/design/nck-prc-frontend-semantic-config-boundary.md` |
| 允许（**本轮定向修订**） | **只修改本文件**（修订而非重写）；除本文件外**不新增、不修改、不删除**任何代码 / 测试 / 配置 / 其它文档。**第二轮修订同样只改本文件，且不执行任何安装类操作**（§9.4 禁止性纪律） |
| 允许（**本轮实测**） | ① 在 `apps/mesa-web` 下执行 `pnpm typecheck` 与 `pnpm test`（**未改任何配置**）；② 在**系统临时目录的独立副本**内构造最小探针并运行（**副本已删除**、**未改动本分支工作树**） |
| 禁止 | 修改任何代码 / 测试 / 配置 / 其它文档（含两份冻结设计文档） |
| 禁止 | 修改前端代码、Core（`crates/**`）、NCK Compiler/Driver（`drivers/sinumerik-nck/**`）、原生协议、其它驱动、生产后端 |
| 禁止 | 任何 git 写操作（commit/push/checkout/stash/reset/PR/tag）；只读 git 可用 |
| 禁止 | 启动 `mesad`、启动 Vite **dev server**、连接任何真实设备或机床 |

**因此**：本文件中凡涉及后端改动的部分（§1.4 的 I-2），一律**只作为「接口需求 / 依赖」登记**
（**已正式登记于 §10.1 / §10.2**），标明归属 PR，**不属于 PR-C 的施工范围**。

**【已裁决修正】（评审最终裁决，推翻此前的旧口径，保留可追溯记录）**：

> **旧口径（仅适用于前端基础模型与交互组件）**：「PR-C 仅用 I-1 推进即可」/「I-2 不作为 PR-C 基础开发前置」。
> **修正后口径**：该旧口径**仅适用于前端基础模型与交互组件**（§1.7 的 **L1**），
> **不适用于可实际提交的完整语义点位配置**——对**完整语义点位配置**而言，**I-2 是 P0 功能依赖**。
> 推理链、三层实施分层与「禁止绕过」条款见 **§1.4.3**；两级验收口径见 **§1.7**；正式登记见 **§10.1**。

## 0.1 七项交付物索引（评审给定）

| # | 交付物 | 本文件章节 |
|---|---|---|
| 1 | 文件范围（新增/修改/冻结 + 不改目录） | §2 |
| 2 | 数据流 `VariableDefinition → 表单 → PointSelection → Task Binding` | §3 |
| 3 | 接口契约（变量目录来源 / Target 约束 / 身份生成与校验责任） | §4 |
| 4 | C10 处置（旧 `variable` 资源入口） | §5 |
| 5 | UI 交互（语义模式 / 高级原生模式 / 编辑与错误反馈） | §6 |
| 6 | 验收矩阵 | §7 |
| 7 | E2E 边界（PR-C vs PR-D） | §8 |

---

# 1. §0【最高优先级】身份接口的存在性调查

> 本节回答的是 **PR-C 设计审查的第一个问题**：前端到底能不能拿到 Driver 计算好的身份？
> 结论直接决定 PR-C 的接口方案与整个数据流设计。

## 1.1 调查路径（四路并行核实）

| 路 | 问题 | 结论 | 关键证据 |
|---|---|---|---|
| A | Driver 是否真的在算身份？ | **是，已实现** | `drivers/sinumerik-nck/src/definition.rs:1091`；被 `src/compiler.rs:398`、`src/presets.rs:250` 调用 |
| B | 浏览载荷里的 `identity` 块是否真的落地？ | **是，已实现且有测试** | `src/presets.rs:262-274`；`tests/identity_browse.rs:179-192`、`:222-225` |
| C | 前端是否**消费**了它？ | **否，完全不消费** | `apps/mesa-web/src/browseModel.ts:20-23, 41-54`；全仓前端 grep `nck_variable|nck_native|preview_point_key` **零命中** |
| D | REST/管理面是否暴露定义目录或身份预览？ | **不存在** | 路由表 `crates/core-api/src/lib.rs:3238-3340` 无任何 definitions/preview 端点；`crates/**` 对 `definitions/828d`、`DefinitionCatalog`、`semantics_status` **零引用** |

## 1.2 现状盘点（逐项给证据）

### 1.2.1 Browse 载荷的 `identity` 块 —— **代码已落地**

**产出点**：`drivers/sinumerik-nck/src/presets.rs:242-275` 的 `semantic_binding()`：

```rust
let preview = def.canonical_identity(&target, &arguments).ok().map(Value::from).unwrap_or(Value::Null);
...
let identity = serde_json::json!({
    "template": def.identity_template,
    "keys": def.identity_keys,
    "preview_point_key": preview,
});
```

- 字段与语义（核实自 `presets.rs:262-266` 与 `definition.rs:1091`）：
  - `template`：该变量的**规范身份模板字面量**（Driver 数据，来自 `definitions/828d/r1.json`），
    例：`channel.program.status@channel={channel}`、`axis.position.commanded_mcs@axis={axis},channel={channel}`、
    `channel.r_parameter#index={index}@channel={channel}`（实测：42 条定义中 40 条 `no#args +@target`、2 条 `has#args +@target`）。
  - `keys`：**身份所需参数键集合**（`identity_keys`，`{path, required}` 数组）。前端**不得**自行决定该集合。
  - `preview_point_key`：**Driver 计算好的预览 Key**；`target`/`arguments` 不完整时**显式为 `null`**
    （`presets.rs:247-253`，注释明写「**不猜测**缺失实例，也不物化可选默认值」）。
- `identity` 块**只出现在浏览/预填载荷**，`presets.rs:168-169` 明写「**不得**进入持久化信封」；
  Core 侧有反向断言：`crates/core-types/src/point.rs:1730` 把带 `identity` 的信封判为**拒绝**（`deny_unknown_fields`）。
- **两条浏览树共用同一产出**：`nck://presets/...` 与 `nck://identity/...` 的叶都由 `leaf_node_with` 构造
  （`drivers/sinumerik-nck/src/identity.rs:10` 导入、`:247` 调用），因此**两条树都带 `identity`**。
- **退化分支实测不会触发**：`presets.rs:193-204` 在「内嵌定义目录缺少该变量」时退回旧原生信封
  `{"resource_id":"variable",...}` 并标注「仅可作原生高级配置」。本次**逐条交叉核对** 62 个模板
  （`presets/828d.json` 的 62 个 `templates[].id` → `GoldenRef` 四元组）与 `definitions/828d/r1.json` 的
  `golden_ref`（`nc_variable` 42 条 + `drive_object` 20 条 = 62）：**62/62 全部命中，退化分支 0 次触发**。
  即：**今天所有浏览叶都是语义信封**。

**可达性（wire 层）**：该 `binding_json` 字符串**逐字**到达浏览器 ——
`crates/core-api/src/lib.rs:864`（`"binding_json": n.binding_json`），路由注册于 `crates/core-api/src/lib.rs:3271`
（`POST /api/v1/endpoints/{id}/browse`），无任何裁剪或白名单。

### 1.2.2 前端消费 —— **不存在（关键缺口）**

`apps/mesa-web/src/browseModel.ts:26-56` 的 `selectableFromNode()` 只理解两种信封，返回类型**只有两个字段**：

- `browseModel.ts:20-23`：`interface BrowseSelection { resource_id: string; parameters: Record<string, unknown>; }`
  → **没有 `identity` 字段**。
- `browseModel.ts:41-45`：`resource_id` 缺失时**回落到 `node.kind`**；语义叶的 `kind` 被
  `presets.rs:194` 设为 `POINT_KIND_VARIABLE`，而 `POINT_KIND_VARIABLE = "nck_variable"`
  （`compiler.rs:36`）→ 前端拿到的 `resource_id` 是 **`"nck_variable"`**。
- `browseModel.ts:48-54`：因为语义信封**含** `parameters` 键，走第一分支，`parameters` 恰为
  `{variable_id, definition_revision, target, arguments}`；**`identity` 键被整体丢弃**
  （它既不是 `resource_id`，也不在 `parameters` 里）。

丢弃之后**没有任何下游**能把它找回来：

- `apps/mesa-web/src/components/ResourcePickerAntd.tsx:91-103`（`applyBrowseFill`）：
  `resources.find(r => r.id === fill.resource_id)` 找不到 `"nck_variable"`（Descriptor 只声明 `variable`，见 §5.1）
  → `message.warning("浏览节点指向未知资源 nck_variable，无法回填")`（`:96`），**直接放弃回填**。
- 全仓前端检索：`apps/mesa-web/src` 对 `nck_variable|nck_native|nck.points.v1|preview_point_key`
  **零命中**（`grep` 实测）。前端**完全不知道** PR-A/PR-B 引入的新模型存在。

**结论（C 路）**：`identity` 块**到达了浏览器**，但**没有任何正式接口把它交到业务层**，
而且在到达业务层之前就被解析层丢掉了。这一条是「存在数据、不存在接口」的精确形态。

### 1.2.3 REST / 管理面 —— **不存在**

- 完整路由表（`crates/core-api/src/lib.rs:3238-3340`）逐条核对，与变量定义 / 身份相关的**只有**：
  - `GET /api/v1/drivers/{id}/descriptor`（`:3241-3244`，实现在 `:3242`）→ 返回 Driver Descriptor，
    其中 NCK 的 `variable` 资源是**12 个原生字段**（`drivers/sinumerik-nck/src/lib.rs:185-337`），
    **不含** `variable_id` / `semantics_status` / `targets` / `identity_template` 等语义信息。
  - `POST /api/v1/endpoints/{id}/browse`（`:3271`）→ 见 §1.2.1/§1.2.2。
- **不存在**：任何形式的
  `definitions` / `variables` / `catalog` / `identity-preview` / `point-key` 预览端点。
- `crates/**` 对 `definitions/828d`、`r1.json`、`DefinitionCatalog`、`semantics_status`
  **零引用**（grep 实测）→ Core **完全不认识**定义目录，符合硬性约束「Core 不懂协议」，
  但也意味着**Core 不可能替前端提供目录**。

### 1.2.4 保存链路：`point_key` 由谁产生？ —— **今天由前端凭空产生**

| 环节 | 事实 | 证据 |
|---|---|---|
| 前端产生 | `suggestPointKey(resourceId, outputId, taken) = ${resourceId}.${outputId}`，冲突时 `.2/.3/...` 递增 | `apps/mesa-web/src/components/ResourcePickerAntd.tsx:41-51`，调用点 `:145-146` |
| 用户可随意改 | 每个 output 一个**自由文本** `Input`，只做「是否与已有 key 重复」的标红 | `ResourcePickerAntd.tsx:224-232` |
| 实际取值形态 | 对 NCK 即 `variable.value`（`resource_id="variable"` + `output="value"`）—— **与 canonical identity 毫无关系** | 由 `ResourcePickerAntd.tsx:46` + `drivers/sinumerik-nck/src/lib.rs:186,329` 推出 |
| 前端冲突预检 | `reconcileEditableSelection()` 第 3 步「`point_key` 在任意 task 已存在 → 拒绝」 | `apps/mesa-web/src/resourceSelectionModel.ts:223-231` |
| Core 校验 | **仅通用格式 + 唯一性**，`point_key` 非空、`validate_point_key` 语法、Endpoint 内跨 Task/跨 kind 唯一 | `crates/core-types/src/point.rs:888-912`（信封层）、`:413`（`validate_point_key`）、`:954`（唯一性）、`crates/core-api/src/binding_gate.rs:112`（跨 kind） |
| Core **不**推导身份 | 模块头明写「**无身份推导**：本模块**不提供** `canonical_identity` 之类的 API」；`parameters` 完全不透明，不与 `point_key` 交叉校验 | `crates/core-types/src/point.rs:13-16`、`:1613` |
| Driver 唯一真值 | `configure` 重算 `canonical_identity` 并与持久化 `point_key` **逐字比对**，不一致即 `POINT_KEY_MISMATCH` 且**整体失败** | `drivers/sinumerik-nck/src/compiler.rs:489-520`、`src/lib.rs:678-679` |

**⇒ 今天的状态是：前端自造 `point_key`（`variable.value`），Core 只查格式，Driver 在 Configure 时才用
重算身份拒绝它。** 也就是说，若 PR-C 不落地身份来源，语义点位**必然**在启动时被 `POINT_KEY_MISMATCH` 拒。

## 1.3 结论（二选一）：**(B) 不存在可直接消费的正式接口**

**判定**：**(B)**。前端**不存在**任何可直接获取 Driver 身份计算结果的正式接口。

**依据（四条，缺一不可）**：

1. **解析层丢弃**：唯一承载身份的载体（浏览叶 `binding_json.identity`）在
   `browseModel.ts:20-23/41-54` 被结构性丢弃，`BrowseSelection` 类型里根本没有这个字段。
2. **业务层无入口**：`ResourcePickerAntd.tsx:93-97` 连语义叶的 `resource_id="nck_variable"`
   都无法映射到任何 Descriptor 资源，直接告警放弃。
3. **无管理面接口**：路由表（`core-api/src/lib.rs:3238-3340`）无定义目录 / 身份预览端点；
   `crates/**` 零引用定义目录。
4. **`point_key` 现状是前端凭空产生**（`ResourcePickerAntd.tsx:41-51`），与 canonical identity 无任何关系。

**必须同时指出的「近似 (A) 的部分」**（如实记录，不抹平）：
Driver 侧的身份编译（`definition.rs:1091`）、浏览载荷的 `identity` 块（`presets.rs:262-274`）
**都已经实现并被测试覆盖**（`tests/identity_browse.rs:179-192`、`:222-225`），
且**逐字到达浏览器**（`core-api/src/lib.rs:864`）。
因此 (B) 的性质不是「后端没做」，而是**「后端做了、线级也有，但前端一侧没有把它变成接口」**。
这个区别很重要：它意味着**最小方案可以基本不碰后端**（见 §1.4 I-1）。

## 1.4 最小接口方案

### I-1（**PR-C 范围内，零后端改动**）：把既有浏览叶 `identity` 块**正式化为只读契约**并由前端消费

| 项 | 内容 |
|---|---|
| **接口输入** | `POST /api/v1/endpoints/{id}/browse`，body `{parent, filter, cursor, limit}`（既有，`core-api/src/lib.rs:806-811`） |
| **接口输出** | 叶节点 `binding_json`（**JSON 字符串**，需前端二次解析）：<br>`{"kind":"nck_variable","parameters":{variable_id,definition_revision,target,arguments},"identity":{"template":string,"keys":[{path,required}],"preview_point_key":string\|null}}` |
| **责任归属** | **产出**：NCK Driver（`presets.rs:242-275`，已实现，PR-C **不得**改）<br>**透传**：Core（`core-api/src/lib.rs:864`，逐字，已实现）<br>**消费**：前端（PR-C 新增：`browseModel.ts` 透出 `identity`；`nckPointModel.ts` 做通用替换） |
| **是否需要后端补充** | **不需要**。这是本方案「最小」的核心理由：身份模板、身份键集合、预览 Key 三者**都已经在线上了**。 |
| **覆盖缺口（必须写明）** | `identity` 块**只有** `template`/`keys`/`preview_point_key` 三项。§1 功能要求里的**「参数来源 / 支持状态 / 无法读取的原因」**（`semantics_status` + `source[]`，见 `definition.rs:59-69, 79-97`）、以及 `arguments` 的**类型/范围/必填**元数据（`r1.json` 的 `arguments[]`：`{key,label,value_type,required,min,max}`）**都不在** `identity` 块内，**Browse 拿不到**。 |

#### 1.4.1 I-1 的能力边界（**评审第一条：必须明确写出，不得含糊**）

**A. I-1 可独立交付的能力（PR-C 范围内，零后端改动，可单独验收；即 §1.7 的 **L1**：
前端基础模型与交互组件）**：

| # | 能力 | 边界说明 |
|---|---|---|
| 1 | **语义点位与原生点位的「数据模型」** | `PointSelection{kind, point_key, parameters}` 判别联合；`nck_variable` / `nck_native` 分流；`identity` **不得**折进 `parameters`（§2.2、§5.3-3） |
| 2 | **C10 旧配置入口隔离** | 普通采集界面**不再**通向必败的旧 `variable` 手填流程；旧入口显式降级为「高级原生配置」（§5.4 六条措施） |
| 3 | **已经由 Driver 返回的完整 `preview_point_key` 的展示与复用** | 在用户**未修改** `target`/`arguments` 时**逐字展示**并可直接复用；为 `null` 时禁止提交（§1.5） |
| 4 | **前端 PointSelection / Task Binding 的正确构造** | 装配收敛为**唯一入口**，kind 按目标 Driver 判定（§3.2 ⑥、§5.3-2、§5.4 C10-4/C10-6） |
| 5 | **高级原生模式与错误反馈** | `nck_native` 显式高级入口、`identity_mode=derived/user`、以及 Core/Driver 错误码**原样透传**（§6.2、§6.3） |

**B. I-1 不能独立支撑的部分（必须显式声明，不得用 UI 兜底掩盖）**：

现有 Browse 载荷**缺少「变量定义的完整元数据」**。除 `identity{template, keys, preview_point_key}` 与
`parameters{variable_id, definition_revision, target, arguments}` 之外，以下**全部不在** Browse 载荷内：

- **`scope`（适用范围）**——§4.1 登记的实测缺口，Browse **不做 `scope` 过滤**；
- **Target 约束**——`targets`（可选集）、`target_required`（原始约束声明）；
- **Arguments 约束**——`arguments[]` 的 `value_type` / `required` / `min` / `max`；
- **支持状态**——`semantics_status`（`verified` / `candidate` / `parameter_identity`）；
- **来源**——`source[]`（`manual` / `measurement` / `identity_probe` / `frozen_vector`）；
- **展示名**——`label{zh,en}`；以及 `golden_ref[]`。

> **⇒【已裁决修正】完整语义化配置依赖 I-2 或等价的服务端接口，且该依赖是「完整语义点位配置」的 P0 功能依赖，
> 不是「可选增强」**（§1.4 I-2、§10.1）。
> 在 I-2（或等价接口）落地前，PR-C 交付的是**以 Driver 预览为真值的最小闭环**，
> **不得**声称已交付「完整的语义化配置体验」——两种完成度的口径见 §1.7。
> **注意**：按 §1.4.3 的推理链，缺少 I-2 时**语义点位的可提交路径实际上会被整体禁用**，
> 因此**不得**用「L1 已交付」掩盖「可实际提交的语义点位为零」这一终态。

#### 1.4.2 实例身份规则（**评审第一条的最高优先级收紧项，原样生效**）

> **Browse 返回的 `preview_point_key` 只是「特定实例」的身份预览，
> 不等于任意 Target / Arguments 修改后的服务端实时编译结果。**
>
> **用户一旦修改 Target 或 Arguments，不得直接沿用旧 `preview_point_key`。**
>
> 仅靠**前端模板替换 + 共享 Golden** 虽可验证**编码规则**，
> 但**不能证明身份语义与 Driver 当前定义完全一致**。
>
> **首选方案是服务端权威 Preview**；若采用前端通用模板渲染，
> **必须有明确的适用范围与跨层契约验证**，
> 且**不得视为「已由 Driver 验证的新实例身份」**。

**这条规则对 §1.5 / §4.3 / §7 的强制后果（逐条落地，不得只在正文写口号）**：

| 触发条件 | 必须的行为 | 禁止的行为 |
|---|---|---|
| 用户**未**修改 `target`/`arguments`（或修改后又**完整改回**同一实例） | 允许**逐字复用** `identity.preview_point_key` 提交 | — |
| 用户**修改了** `target` 或 `arguments` | **(首选)** 请求**服务端权威 Preview** 取回新 Key；<br>**(降级且仅当有明确适用范围时)** 用 Driver 下发的 `template` + `keys` 做**通用替换**，并把该 Key 标注为**「前端渲染、未经 Driver 校验」** | 静默沿用旧 `preview_point_key`；把前端自算结果当作「已由 Driver 验证的新实例身份」持久化 |
| 服务端 Preview **不可得**（无该接口 / Driver 不可达） | 该实例标记为**待确认**并**禁止提交**（§1.5） | 用前端自算值**冒充**已验证身份后提交 |

> **可靠性边界（如实登记）**：前端通用替换只能证明「**编码规则**与共享 Golden 一致」
> （§4.3、矩阵 A3/A4/A5），**不能**证明「该实例的身份语义与 Driver 当前定义完全一致」。
> 因此矩阵新增 **A39**（改 Target/Arguments 后不得沿用旧 Key，**含反例**）与
> **A41**（共享 Golden 可不复制地直读，仅证明编码规则层）两项可验收断言。

### I-2（**超出 PR-C 授权**，仅作接口需求登记；【已裁决修正】**是「完整语义配置」的 P0 功能依赖**）：新增只读「变量定义目录」端点

| 项 | 内容 |
|---|---|
| **接口输入** | `GET /api/v1/drivers/{driver_id}/definitions?family=828d`（无 body；`family` 可选，缺省用 Endpoint/驱动配置） |
| **接口输出** | `{format_version, family, revision, definitions:[{variable_id, revision, scope, semantics_status, label{zh,en}, source[{kind,reference,note?}], targets[], target_required[], arguments[{key,label,value_type,required,min?,max?}], identity_template, identity_keys[], golden_ref[]}]}` —— 即 `definitions/828d/r1.json` 的内容**原样**（**不得**在 Core 侧裁剪、改名或重排） |
| **责任归属** | **数据所有者**：NCK Driver（目录是 Driver 内嵌数据，`definition.rs` 的 `DefinitionCatalog`）<br>**传输/缓存**：Core 仅做**逐字透传**，**不得**解释 `variable_id`/`target`/`semantics_status`（否则违反「Core 不懂协议」）<br>**消费**：前端只读 |
| **为什么是「最小」** | ① **只读**、无参数依赖、无写入路径；② **不改** `PointSelection`/`point_key`/Task Binding 任何语义；③ **不改** Descriptor 契约（`contract_major/minor` 保持 2/0，`crates/core-types/src/descriptor.rs:9-13` 冻结面不动）；④ **不改** 持久化面与 `mesa.resources.v1` 路径；⑤ 是唯一能满足「展示参数来源与支持状态」的形态。 |
| **实现代价与归属（如实记录）** | 它需要 **Driver SDK 新增能力 + Core 透传 + 可能触及 `proto/**`** ⇒ 属**后端变更**，**不在 PR-C 授权内**。建议归属：**独立后端 PR（建议命名 `PR-B2-nck-definition-catalog-api`）**。 |
| **正式登记（评审第二条，见 §10.1）**【已裁决修正】 | **I-2 登记为「独立后端接口工作」**：它是**完整语义点位配置的 P0 功能依赖**（§1.4.3）。**前端基础模型与交互组件（L1）仍可先做**、不以 I-2 为前置条件；但**可实际提交的完整语义点位配置**依赖它或等价权威接口（§1.7）。 |
| **PR-C 的应对** | 见 §1.5 降级路径：**I-2 未落地时，PR-C 必须把「支持状态不可得」如实显示为「未提供」，不得用 `verified` 兜底**。 |

> **为什么不选「Core 侧兜底」**：`crates/core-types/src/point.rs:13-16` 明写 Core 无身份推导、
> `parameters` 不透明；PR-B 文档 §4（`:229`）亦裁决 Core「身份推导 **不需要（且禁止）**」。
> 把目录或身份推导下沉到 Core 会**直接违反已冻结的架构约束**。

### 1.4.3 【已裁决修正】I-2 是「完整语义配置」的 **P0 功能依赖**（推理链 + 三层实施分层）

> **本条推翻此前的「PR-C 仅用 I-1 推进即可」口径。** 该旧口径**仅适用于前端基础模型与交互组件**
> （§1.4.1 A 的五项能力、§1.7 的 **L1**），**不适用于可实际提交的完整语义点位配置**。
> 保留此条即为**可追溯记录**（旧口径 → 新口径的修正过程不得抹去）。

**推理链（评审原文要点，逐步落地，不得省略任一环）**：

1. **I-1 只能提供**既有 Browse 叶的 `identity` **预览**及**部分参数**
   （`parameters{variable_id, definition_revision, target, arguments}` ＋
   `identity{template, keys, preview_point_key}`，§1.2.1、§1.4 I-1）；
2. **`identity.keys` 不能替代 `targets`、`target_required`、`arguments[]` 的完整定义** ——
   `keys` 只声明「身份需要哪些键」，**不**声明「哪些 Target 组合合法」「哪些键必填」
   「`arguments` 的类型 / 范围 / 必填」（§1.4.1 B 已列出全部缺口字段）；
3. **A40 规定：缺失 `scope` 时不得默认允许采集**（§10.2.4、矩阵 **A40**）；
   而 `scope` **只能通过 I-2 或等价权威接口**获取（Browse 载荷**不含** `scope`，
   §4.1 实测：`PresetRegistry`/`semantic_binding` **不按 `scope` 过滤**）；
4. **⇒ 没有 I-2，按当前文档约束，语义点位的可提交路径实际上会被整体禁用**：
   前端既不能证明某变量 `scope === "collect"`，也不能按权威 `targets`/`arguments[]` 生成合法控件
   ⇒ 在 A40 的 fail-closed 约束下**全部语义点位都必须保持禁用 / 不可确认**。

**⇒ 明确禁止两条捷径（评审原话）**：

> **不建议通过硬编码 828D Target 组合或放宽 A40 来绕过这个问题。**
>
> - **禁止硬编码 828D Target 组合**：那等于把 Driver 的定义数据搬进前端，
>   违反 §1.6 红线与 §4.2「PR-C 的表单必须按每个变量的 `target_required` 动态生成控件」；
> - **禁止放宽 A40**：放宽即等于「缺 `scope` 也默认允许采集」，
>   直接违反 §10.2.4 的 fail-closed 硬性约束，会把「只服务身份探针」的变量放给用户采集。

**调整后的三层实施分层表**：

| 层 | 内容 |
|---|---|
| **PR-C 基础开发** | Browse 解析、PointSelection、Binding、C10 隔离、高级原生模式、基本表单 |
| **独立后端接口 PR（I-2）** | 提供定义目录、`scope`、支持状态、Target/Arguments 约束；**核对服务端 Preview 能力缺口** |
| **PR-C 完整功能验收** | 服务端权威元数据可用，Target 动态生成、身份正确、采集提交可用 |

**评审的实施建议（原样登记）**：

> **推荐优先完成最小 I-2 接口，再完成 PR-C 的最终功能集成，
> 而不是交付一个所有语义点位都无法提交的界面。**

**与 §1.7 两级验收的对应关系**：第 1 层 ＝ **L1**；第 2 层 ＝ 独立后端 PR（I-2）；
第 3 层 ＝ **L2**。**L2 不再可无限期降级**（§1.7 验收纪律 1 已按本条收紧）。

### 1.4.4 Target 元数据降级规则（状态表，**逐行照录**，评审给定）

> **定位说明（评审原话）**：**这套规则既满足 A39/A40，也不会把前端变成第二个 NCK Compiler。**
>
> 适用面：**所有**语义点位表单（§6.1）、所有降级分支（§1.5）、所有提交前判定。
> 本表是**规范状态表**：矩阵 §7.2–§7.8 与 §6 的控件行为**逐行引用**本表（覆盖映射见 §7.8.1）。

| 元数据情况 | 前端行为 | 允许提交 |
|---|---|---|
| `targets`、`target_required`、`arguments[]`、`scope` 完整且有效 | 动态生成合法控件，执行校验 | 满足身份条件后允许 |
| 只有 `identity.keys` | 展示已知身份字段及待确认信息，**不推断完整合法集合** | 否 |
| `scope` 缺失 | 显示「适用范围未提供」 | 否 |
| `scope=identity` | **不作为可采集点位提供** | 否 |
| Target 或 Arguments 已修改，但无权威身份校验路径 | 旧 Key 作废，提示需要重新确认 | 否 |
| Driver 不可达 | 禁止新增语义点位 | 否 |

**逐行要点（与既有条文的对应，确保不引入新语义）**：

1. **第 1 行**是**唯一**「允许提交」的行，且**仍以「满足身份条件」为前提**（§1.4.2：身份未确认仍禁止）；
2. **第 2 行**＝ §1.4.1 B 的缺口清单：**`identity.keys` 不是 Target/Arguments 的完整定义**，
   前端**不得**由 `keys` 反推合法集合（§1.6 红线第 3 条）；
3. **第 3 行**＝ §4.1 / §10.2.4 的 **A40** fail-closed 口径；
4. **第 4 行**＝ §4.1 实测缺口＋§10.2.2 的产品语义（`identity` **不得**作为可采集项出现在普通采集界面）；
5. **第 5 行**＝ §1.4.2 / §1.5「用户改了 `target`/`arguments`」的收紧规则，对应 **A39**；
6. **第 6 行**＝ §1.5 第 1 行（Driver 不可达 ⇒ 禁止提交语义点位）。

## 1.5 失败与降级行为（**必须可验收**）

| 情形 | 前端行为（硬性） |
|---|---|
| Driver 不可达 / Browse 返回 503 / `DRIVER_UNAVAILABLE` | **禁止提交语义点位**。变量列表显示「定义不可得（驱动不可达）」，`加入` 按钮 disabled，且**不得**回落到猜测 Key。既有错误提示保留（`ResourceBrowseAntd.tsx:34-37, 42-44`、`:98`） |
| 已浏览过并**已缓存**过 `identity.template`/`keys` | 允许离线编辑与预览（缓存的是**驱动产出的模板**，不是前端算法）；缓存必须显式标注来源与时间 |
| 收到的叶**没有 `identity` 块**（旧驱动 / 未来降级） | 该叶**只能作为原生高级配置**入口，**不得**进入语义模式；UI 必须显示原因（对齐 `presets.rs:200-203` 的既有口径） |
| `preview_point_key` 为 `null`（`target`/`arguments` 不完整） | 显示「身份未确定：待确认字段 …」（对齐 `presets.rs:186-190` 的 `pending` 文案），**禁止**提交 |
| 用户改了 `target`/`arguments` 导致需要重算 | **禁止沿用旧 `preview_point_key`**（§1.4.2）。合法路径按优先级：**(a) 首选**——请求**服务端权威 Preview** 取回该实例的新 Key；**(b) 降级**——仅在已声明适用范围时，用 Driver 下发的 `template` + `keys` 做**通用替换**（值编码遵守 `tests/fixtures/nck-point-key-golden.json`），并把结果标注为「前端渲染、未经 Driver 校验」；**(c)** 重新浏览取回新的 `preview_point_key`。**不得**有第四条（前端估算后冒充已验证身份）；`(b)` 的 Key **不得**标记为「已由 Driver 验证」 |
| `identity_mode="user"`（高级原生模式指定 Key） | **允许用户明确指定合法的 Native 自定义 Key**；**本阶段不实现自动生成 `~<n>` 后缀**、**禁止自动改名**；冲突时**拒绝**并提示用户自行改名；**身份保持原值**（`resourceSelectionModel`/前端不得自动加后缀，也**不依赖**任何建议值算法；矩阵 **A25**） |
| 若 I-2 未落地 | 「参数来源 / 支持状态」显示为 **「未提供（需定义目录接口）」**，**绝不允许**把 `candidate` / 访问能力 HOLD 的条目显示为 `verified` |
| 元数据**只有 `identity.keys`**（无 `targets`/`target_required`/`arguments[]`/`scope`） | 只展示**已知身份字段**与「**待确认**」信息，**不推断完整合法集合**；**禁止提交**（§1.4.4 第 2 行、矩阵 A42） |

> **本表与 §1.4.4「Target 元数据降级规则」状态表逐行一致**：其中「**只有 `identity.keys`**」一行的
> 前端行为与「**允许提交 ＝ 否**」的约束由 §1.4.4 与矩阵 **A42** 承担；
> 「`scope` 缺失」「`scope=identity`」「Driver 不可达」「Target/Arguments 已修改但无权威校验路径」
> 四行分别由矩阵 **A40 / A13 / A15 / A39** 承担（覆盖映射见 §7.8.1）。
> **表内任何一行都不允许放宽**：**不得**由 `keys` 推断合法集合，**不得**因缺 `scope` 而默认放行。

## 1.6 红线（**原样写入，不得改写**）

> **前端不得重新实现 Driver 的地址解析器，也不得自行发明另一套 Canonical Identity 算法。**
>
> **不得自行估算身份后持久化。**
>
> 若接口不足，必须**在设计文档中明确缺口并提出最小接口方案**，而不是用前端近似算法兜底。

**与红线的边界澄清（必须显式写明，否则会被误判违规）**：

1. **允许**：按照 Driver 下发的 `identity.template` + `identity.keys` 做**通用替换**（§1.4 I-1），
   并把值按**冻结的通用类型标签编码**（`crates/core-types/src/point.rs:402` 起、§4.1.1 E–H）编码。
   这是**消费驱动模板**，不是「自己发明算法」；模板与键集合的**决定权在 Driver**。
2. **允许（唯一例外，且有冻结依据）**：`nck_native` ＋ `identity_mode="derived"` 的 Key 是
   **固定规则** `native.<area>.<area_no>.<module>.<column>.<line>`（`definition.rs:1163-1165`、
   PR-B 文档 `:573`），**不是**由变量模板派生的。前端按该固定规则生成属**实现已冻结规范**，
   且 Driver 会用同一规则重算并拒绝不一致（`compiler.rs:433`、`lib.rs:611-621`）⇒ **fail-closed**。
3. **禁止**：由 `variable_id` 反推 `target`/`arguments`；自行决定哪些键进入身份；**自行排序身份键**；
   自行生成 `~<n>` 后缀；在无模板时猜测 Key。

## 1.7 两级验收口径（**评审第一条：必须区分，不得混为一谈**；**I-2 定性已按【已裁决修正】升级**）

PR-C 存在**两种完成度**，验收时必须分别声明，**不得**用其中一级的完成去充当另一级：

| 级别 | 名称 | 依赖 | 可验收内容 | 不得声称 |
|---|---|---|---|---|
| **L1** | **前端基础模型与交互组件完成**（基础配置能力） | **I-1**（零后端改动）—— 该级别**不**要求 I-2 | §1.4.1 A 的五项能力：数据模型、C10 入口隔离、**Driver 已返回的完整 `preview_point_key` 的展示与复用**、PointSelection / Task Binding 正确构造、高级原生模式与错误反馈 | 不得声称「所有可见语义变量均可安全配置」（缺 `scope` 元数据，见 §10.2）；**更不得**据此声称「语义点位已可实际提交」 |
| **L2** | **完整语义配置闭环完成** | **I-2 或等价服务端权威接口 ——【已裁决修正】这是「完整语义点位配置」的 P0 功能依赖，不是「可选增强」** | L1 全部 ＋ 参数来源 / 支持状态 / 无法读取原因的真实展示；按 `scope` 正确过滤可采集变量；按 `targets`/`target_required`/`arguments[]` 约束渲染表单；**服务端权威 Preview**（改 Target/Arguments 后由服务端给出新实例身份） | — |

**【已裁决修正】I-2 的定性（替代此前的「可选增强 / 不作前置」口径，保留可追溯记录）**：

> I-2 **不是** L2 的「可选增强」，而是**完整语义点位配置的 P0 功能依赖**（推理链见 §1.4.3）。
> 因此 **L2 不再可无限期降级**：缺少 I-2（或等价权威接口）时，
> **语义点位的可提交路径实际上会被整体禁用**（`scope` 不可得 ⇒ A40 fail-closed），
> 交付物将是「**一个所有语义点位都无法提交的界面**」。
> 评审建议：**推荐优先完成最小 I-2 接口，再完成 PR-C 的最终功能集成**（§1.4.3、§10.1）。

**验收纪律**：

1. PR-C 的**开发前置** ＝ **L1** 的依赖项（I-1，前端基础模型与交互组件）；
   **PR-C 的「完整功能验收」** ＝ **L2** 的依赖项（**I-2 或等价接口，P0**；
   建议独立成 PR：`PR-B2-nck-definition-catalog-api`，§10.1）。
2. L2 未达成时，PR-C 的验收结论**必须**写明「**L1 达成、L2 未达成**」，并列出未达成的具体条目
   （至少：`scope` 过滤、`semantics_status`/`source[]` 展示、服务端权威 Preview），
   且**必须显式写明「完整语义点位配置当前不可提交」**这一后果；
   **L2 不得**被标记为「可选 / 可无限期推迟」（【已裁决修正】）。
3. **禁止**用 L1 的绿灯掩盖 L2 的缺口，也**禁止**用前端近似算法把 L2 的缺口「补绿」（§1.6 红线）。
4. **禁止**用下列两条捷径绕过（评审原话，详见 §1.4.3）：
   **硬编码 828D Target 组合**、**放宽 A40**。

---

# 2. 交付物一：文件范围

> 判定口径沿用 PR-B 文档 §1：**新增** = 本次新建；**修改** = 语义/形状变更；
> **冻结** = 本次**不得**改动（只允许「附加可选属性」式演进，**不得**改语义）；
> **不改** = 明确排除的目录。

## 2.1 新增（PR-C）

| 路径 | 职责 | 关键内容 | 依据 |
|---|---|---|---|
| `apps/mesa-web/src/nckPointModel.ts` | NCK 语义点位**纯函数层**（**不得** import React/AntD，纪律同 `resourceSelectionModel.ts:18`） | ① 解析浏览叶 `identity` 块；② 按 Driver 模板做**通用替换** + 类型标签编码（对齐 `tests/fixtures/nck-point-key-golden.json`）；③ `PointSelection` payload 构造（`{kind, point_key, parameters}`）；④ `target`/`arguments` 默认值与必填预检；⑤ `nck_variable` ↔ `nck_native` 判别 | 上位设计 `:195`、`:1203` |
| `apps/mesa-web/src/nckPointModel.test.ts` | 上述纯函数单测 | 见 §7 矩阵 A1–A12 | 上位设计 `:197`、`:1203` |
| `apps/mesa-web/src/components/NckPointFields.tsx` | NCK 专用语义表单组件（变量选择 + Target + Arguments + 预览 Key + 高级原生切换）；**不改** `DescriptorFields.tsx` | 按 `targets`/`target_required` **动态生成** Target 控件；按 `arguments[]` 生成 Arguments 控件 | 上位设计 `:196`、`:1204` |
| `apps/mesa-web/src/components/NckPointFields.test.tsx` | 表单渲染 / 必填 / 高级警告单测 | 见 §7 矩阵 A13–A20 | 上位设计 `:198`、`:1204` |
| `apps/mesa-web/src/nckCatalog.ts` + `.test.ts`（**仅当 I-2 落地**） | I-2 目录的类型与只读客户端 | **若 I-2 未落地，本文件不新增**，改由 §1.5 的「未提供」降级承担 | §1.4 I-2 |

## 2.2 修改（PR-C）

| 路径 | 现状 | 改动点（行号区间） | 依据 |
|---|---|---|---|
| `apps/mesa-web/src/browseModel.ts` | 只理解两种信封，丢弃 `identity`（实测 71 行） | `:1-56`：新增 `nck_variable`/`nck_native` 信封判别；`BrowseSelection` 增加**可选** `identity`（**不得**把 `identity` 折进 `parameters`）；保留 `:39` 空信封不可选、`:41-46` 回落逻辑。**【本轮新增验收要求 3-1】批准添加可选的 NCK 专属元数据，但必须保证其他驱动的 Browse 叶节点仍按既有行为处理**（非 NCK 叶**不得**被判为语义叶、**不得**改变可选性）；两类回归测试见矩阵 **A44** | 上位设计 `:217`、PR-B `:499`；**本文件 §7.8 A44、§9.3-5** |
| `apps/mesa-web/src/browseModel.test.ts` | 现有 8 用例（含 `:109-135` 语义必填） | `:109-135` 起改写：语义信封识别、`identity` 透出、**`identity` 不出现在 params** | 上位设计 `:1208` |
| `apps/mesa-web/src/resourceSelectionModel.ts` | `ResourceSelectionLike` 只有通用形态（`:20-25`）；`missingRequiredParams`（`:55-71`） | `:20-25` 扩为**判别联合**（通用形态 + NCK 语义形态，携带 `identity`）；**`:169-377` reconciliation 算法本体逐字不动**；`:55-71` 保持「只回答缺不缺」语义不变 | 上位设计 `:216`、`:255` |
| `apps/mesa-web/src/deviceModel.ts` | `ResourceSelection` + `CANONICAL_RESOURCES_KIND`（`:203-210`）；`splitAcquisitionTasks`/`selectionsOf` 只认 `mesa.resources.v1`（`:235-252`）；装配真值 1（`:280-304`，`:301`） | `:203-210` 新增 `NCK_POINTS_BINDING_KIND = "nck.points.v1"` 常量与 NCK canonical 形态；`:235-252` 同时识别两种 kind（否则编辑 NCK Endpoint 时任务被判为 preserved 或丢弃 —— **数据安全关键路径**）；`:280-304` 收敛为**唯一装配入口**（与 `bootstrap.ts` 共用） | 上位设计 `:212-215` |
| `apps/mesa-web/src/deviceModel.test.ts` | 现有 71 用例（含 2 个与 `describe` 同行的 `it(`） | 新增：唯一装配入口、双 kind 识别、任务保全语义不退化 | 上位设计 `:1205` |
| `apps/mesa-web/src/addDevice/bootstrap.ts` | **硬编码字面量** `"mesa.resources.v1"`（`:138`） | `:131-140`：改为引用**共享装配入口**，消除与 `deviceModel.ts:210/301` 的重复（双真值收敛）；`tasks=[]` 零采集点语义**逐字不变**（`:130-133, 153`） | 上位设计 `:215`、`:1206` |
| `apps/mesa-web/src/addDevice/bootstrap.test.ts` | 现有 10 用例（`:76` 断言硬编码 kind） | 装配收敛后断言改指向共享入口；「`tasks=[]` 不启动」不得退化 | 上位设计 `:1206` |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx` | 必填按**整份原生 schema** 判（`:108-114`）；`suggestPointKey`（`:41-51`）；`:241-286` 加入按钮与二次校验 | `:108-114` **按 Point Kind 分流必填来源**（语义模式**不得**用原生 schema，见 §5.3）；`:41-51` 语义 `point_key` 推导（消费 `identity`）；`:241-286` 语义模式的禁用/错误反馈 | 上位设计 `:218-219` |
| `apps/mesa-web/src/components/ResourcePickerAntd.test.tsx` | 现有 16 用例 | 新增：按 Kind 的必填门、语义分支不再使用原生 schema、`point_key` 不可自由改 | 上位设计 `:1209` |
| `apps/mesa-web/src/components/ResourceBrowseAntd.tsx` | 只用 `selectableFromNode` 判定可选用（`:102`、`:121-123`） | `:102-123`：语义叶 → 语义 Selection（携带 `identity`），并把「无 identity / 预览为 null」的原因显示出来 | 上位设计 `:1210` |
| `apps/mesa-web/src/components/ResourceBrowseAntd.test.tsx` | 现有 2 用例 | 新增：浏览叶 → 语义 Selection | 上位设计 `:1210` |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx` | 装配真值 1 的调用点（`:123`、`:146`） | `:123` 邻域：只调用共享装配入口，无独立语义；`:218-247` 传 `endpointId`（Browse 语义路径的**唯一**可用调用方） | 上位设计 `:221` |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.saveIssues.test.tsx` | 现有 2 用例 | 装配收敛后的绑定形态 | 上位设计 `:1211` |
| `apps/mesa-web/tsconfig.json` | `include: ["src","vite.config.ts"]`（实测）；**未显式设** `resolveJsonModule` —— 但 `tsc --showConfig` 实测其**生效值**为 `true`（§2.5） | **条件修改项，默认不改**：实测 `import ... from "../../../tests/fixtures/nck-point-key-golden.json"` 可解析、且 `pnpm typecheck`（`tsc --noEmit`）**零错误** ⇒ **本文件无需修改**（`include` 也**无需**扩展：被 import 的文件自动进入编译单元）。仅当未来 TS 版本 / `moduleResolution` 变更导致 `TS2732` 时，才按 §2.5 的最小改动处理 | **§2.5 本轮实测**（上位设计未列此文件） |
| `apps/mesa-web/vite.config.ts` | 无 `server.fs.allow`（实测）；`test.poolOptions` 等不动 | **条件修改项，默认不改**：`pnpm test` **不启动 dev server**，实测 Vitest 导入该 fixture **直接通过**（1 文件 / 1 用例）⇒ **测试路径无需修改**。仅当**生产代码**（非测试文件）import 该 fixture 时，dev server 才会以 `403 outside of Vite serving allow list` 拒绝（§2.5 实测 `fs.allow` 不含仓库根）；此时**只改本文件这一处**，`build`/`test` 段一字不动 | **§2.5 本轮实测**（上位设计未列此文件） |

> **明确不改（评审已裁决，见 §8.3）**：`apps/mesa-web/src/addDevice/AddDeviceFlow.tsx` 与
> `apps/mesa-web/src/addDevice/AddDeviceFlow.test.tsx`（**包括对应测试**）**均由 PR-D 统一处理**，
> **PR-C 不修改这两个文件**，本阶段也**不要求**完成新增设备向导的完整语义点位流程。
> 上位设计 `:1207` / `:1223` / `:1264` 的**内部冲突标记为「已裁决」**，裁决结论见 §8.3。
>
> **【本轮定向修订再次确认】该 PR-C / PR-D 文件边界不变**（修订 4）：上述两个文件
> —— **包括 `AddDeviceFlow.test.tsx` 这个对应测试** —— **继续由 PR-D 独占**，PR-C **不修改**。
> 本轮四项修订中的任何一项（含 §1.4.3 的 I-2 P0 升级、§1.4.4 的降级规则表、§7.8 的新增验收）
> **均不改变**该边界，也**不新增**对这两个文件的任何改动点。

## 2.3 冻结（**PR-C 不得改语义**；只允许「附加可选属性」式演进）

| 路径 | 规模（实测） | 冻结口径 | 证据 |
|---|---|---|---|
| `apps/mesa-web/src/connection/**` | **9 文件**（`ConnectionFields.tsx`、`connectionProfiles.ts`、`connectionSetupModel.{ts,test.ts}`、`ConnectionSetupPanel.{tsx,test.tsx}`、`ConnectionTestResult.tsx`、`useConnectionSetup.{ts,test.ts}`） | 指令 8；**134 用例**必须原样通过 | 上位设计 `:252`、`:1212` |
| `apps/mesa-web/src/components/DescriptorFields.tsx` | 401 行 | 受控描述字段语义冻结；**新语义层不得改它** | 上位设计 `:253` |
| `apps/mesa-web/src/components/ApplyWithRestart.tsx` | 50 行 | 统一生命周期条冻结 | 上位设计 `:254` |
| `apps/mesa-web/src/resourceSelectionModel.ts:169-377` | reconciliation 冻结算法 | 只扩 identity 输入，**不改裁决** | 上位设计 `:255` |
| `drivers/sinumerik-nck/src/native.rs`、`codec.rs`、`identity_spec.rs`、`presets/828d.json`、`tests/legacy_equivalence.rs`、`tests/fixtures/legacy-828d-wire.json` | — | PR-B 裁决为「逐字不动」 | PR-B `:181`、`:196` |
| `crates/core-types/src/schema.rs`、`descriptor.rs`、`crates/config-store/src/lib.rs:1655-1770` | — | Descriptor 2.0 冻结面 / point_id 映射 | 上位设计 `:245-249` |
| `tests/driver-contract/tests/descriptor_contract.rs` | 1094 行 | PR-B 裁决 **C10=D-A：整文件逐字不动** | PR-B `:33`、`:987` |

### 2.3.1 PR-C 是否需要触碰冻结文件？—— **逐项核实结论**

| 冻结文件 | PR-C 是否需要触碰 | 判定依据 |
|---|---|---|
| `DescriptorFields.tsx` | **不需要触碰语义**，但**需要复用**其渲染能力 | 语义表单是**新组件** `NckPointFields.tsx`（上位设计 `:196` 明写「**不改** `DescriptorFields.tsx`」）。`DescriptorFields` 的 `FieldDescriptor` 驱动渲染（`types.ts:31-44`）正是高级原生模式 12 字段的现成渲染器，**调用**它不等于修改它。 |
| `ApplyWithRestart.tsx` | **不需要** | 它只是 `{running, applying, canApply, applyLabel, onApply}` 的展示组件（`ApplyWithRestart.tsx:11-23`）。PR-C 只可能改 `canApply` 的**传入值**（调用方 `EndpointAcquisitionPane.tsx:284`），组件本体不动。 |
| `apps/mesa-web/src/connection/**` | **不需要** | 属连接配置面；PR-C 只动采集点面。 |
| `resourceSelectionModel.ts:169-377` | **不需要改算法**；`:20-25` **需要扩类型** | 冻结范围**明确限于 `:169-377`**（上位设计 `:255`）；`:20-25` 的 `ResourceSelectionLike` 在 `:216` 被显式列为**需扩展**。⇒ 二者不冲突。 |

**关于「附加可选属性」式演进的实测依据**：`DescriptorFields.tsx:9-18, 31, 57-59` 已有先例 ——
C2 为「未失焦就保存」问题**附加了可选属性 `onDraftText`**，并保留 `onBlur` 提交，
语义未变。PR-C 若需要在冻结组件上传递上下文，**只允许**照此模式加**可选**属性。

## 2.4 明确「不改」的目录（PR-C 零改动）

| 目录 | 理由 |
|---|---|
| `crates/**` | Core 只做通用校验 + 唯一性；本 PR-C 不改 Core（上位设计 `:229`、PR-B `:181`） |
| `drivers/sinumerik-nck/**` | PR-C 不改 Driver：Descriptor 面按 **C10=D-A 逐字不动**（PR-B `:980-988`），`native.rs`/`codec.rs` 冻结（PR-B `:27`） |
| `drivers/s7/**`、`drivers/focas2/**`、`drivers/opcua/**`、`drivers/<其它>/**` | 「哪都不许碰」——否则违反「非 NCK 功能不得退化」 |
| `proto/**` | 不改 IPC wire（上位设计非目标 `:8`） |
| `apps/mesad/**` | 生产后端入口；PR-C 不需要 |
| `docs/design/nck-semantic-point-refactor.md`、`docs/design/nck-prb-driver-compiler-boundary.md`、`docs/adr/**` | 已冻结设计文档，逐字不动 |
| `tests/driver-contract/**`、`scripts/**`、`.github/**` | 契约与 CI；PR-C 不改后端即无需触碰 |

## 2.5 条件修改项：`tsconfig.json` / `vite.config.ts`（**实测结论：按 PR-C 计划形态，两个都无需修改**）

评审给定的**正确顺序**（本文件严格按此顺序执行，未跳步）：

1. 用**最小测试**尝试引用仓库级 fixture；
2. 运行 `pnpm typecheck` 与 Vitest；
3. **若能直接访问 ⇒ 两个配置文件都不修改**；
4. 若存在**确定**的 TS 或 Vite 访问失败 ⇒ **仅修改导致失败的那一个**配置，保留**错误日志与修复前后对照**；
5. **不复制第二份 Golden**；**不扩大 Vite 文件系统访问范围**到不必要目录。

> **这两个文件是「条件修改项」，不是无条件修改授权**：默认处置是**不改**；
> 只有在出现**确定的**、可复现的访问失败时才允许改，且**只改导致失败的那一个**。

### 2.5.1 探针构造（系统临时目录内的**独立副本**，未触碰本分支工作树）

| 项 | 内容 |
|---|---|
| 副本根 | `%TEMP%\mesa-prc-probe-871fd11`（**执行完毕后已删除**，删除结果见 §9.3-1） |
| 副本内容 | `apps/mesa-web/**`（完整可运行，含 `node_modules`）＋ `tests/fixtures/nck-point-key-golden.json`，**相对层级与真实仓库一致**（副本内同样有 `tests/fixtures/`） |
| 探针文件 | `apps/mesa-web/src/__fixture_probe__.test.ts` |
| 导入语句 | `import golden from "../../../tests/fixtures/nck-point-key-golden.json";`（`src → mesa-web → apps → <root>` 共 **3 层**，按真实目录层级核准） |
| 断言 | `value_cases.length === 55`、`value_reject_cases.length === 5`、`key_cases.length === 77`，并打印实际数量 |
| **未做** | **未**修改 `tsconfig.json`、**未**修改 `vite.config.ts`、**未**复制第二份 Golden 进 `apps/mesa-web` |

### 2.5.2 实测结果（**修复前**，即两个配置一个都没改）

| 检查 | 命令 / 方法 | 退出码 | 结果 |
|---|---|---|---|
| **TS 侧** | `pnpm typecheck`（即 `tsc --noEmit`） | **0** | **零错误**。`--listFiles` 证据：编译单元**同时包含** `src/__fixture_probe__.test.ts` 与 `tests/fixtures/nck-point-key-golden.json` ⇒ JSON **被真正解析并纳入编译**，不是被当成 `any` 跳过 |
| **TS 侧（对照实验）** | 在探针内植入 `const x: number = "str"` 后重跑 | 2 | 精确报出 `src/__fixture_probe__.test.ts(30,7): error TS2322` ⇒ 探针**确实在被类型检查**（排除「文件根本没进编译」导致的假通过） |
| **Vitest 侧** | `vitest run src/__fixture_probe__.test.ts` | **0** | `Test Files 1 passed (1)` / `Tests 1 passed (1)`；`stdout` 打印 `[PROBE] value_cases=55 value_reject_cases=5 key_cases=77` |
| **dev server 侧**（**与 test 不等价，必须分开结论**） | 用 Vite 自身的 `resolveConfig` + `searchForWorkspaceRoot` 求值（**不 listen、不启动任何 server**） | 0 | `server.fs.strict = true`；`server.fs.allow = [<副本>/apps/mesa-web, <vite>/dist/client]` ⇒ **不含仓库根**；`fixture 在 fs.allow 内？ = false` |

### 2.5.3 结论（**TS 侧与 Vite 侧分别下结论，不含糊**）

- **TS 侧：无需修改（确定）**。`tsc --showConfig` 显示**生效配置**里 `resolveJsonModule: true`
  （尽管 `tsconfig.json` **未显式声明**它）—— 成因是 `moduleResolution: "bundler"`；
  **反事实证据**：强制 `--moduleResolution node10` 后**立刻复现**
  `TS2732: Cannot find module '...nck-point-key-golden.json'. Consider using '--resolveJsonModule' to import module with '.json' extension.`
  ⇒ 现配置下 JSON import **可用**；`include` 也**无需**扩展（被 import 的文件自动进入编译单元）。
- **Vitest 侧：无需修改（确定，实测通过）**。`pnpm test` **不启动 dev server**，
  `server.fs.allow` **不参与**测试路径 ⇒ 「TS 通过、而 dev server 会拒绝」这一分叉**确实存在**，
  但**不影响 `pnpm test`**，因此**不能**用「dev server 会拒绝」当作改 `vite.config.ts` 的理由。
- **dev server 侧：确定会被拒绝**（但**不是** PR-C 计划形态的路径问题）。
  `searchForWorkspaceRoot()` 因 `apps/mesa-web/pnpm-workspace.yaml` 存在而**停在 `apps/mesa-web`**，
  导致 `fs.allow` 不含仓库根；Vite 的拒绝点是 `respondWithAccessDenied`
  （`vite/dist/node/chunks/*.js`，文案 `The request url "..." is outside of Vite serving allow list.`，HTTP **403**）。

**对 §2.2 两个条件文件的最终处置**：

| 场景 | `tsconfig.json` | `vite.config.ts` |
|---|---|---|
| **fixture 仅被测试文件 import**（**PR-C 计划形态**：§2.1 的 `nckPointModel.test.ts` / 矩阵 A4 的向量一致性断言） | **不修改** | **不修改** |
| fixture 被**生产代码**（例如 `nckPointModel.ts` 本体）import | **不修改** | **仅此一处需要修改**（最小改动见下） |

**（仅第二种场景）`vite.config.ts` 的最小修改**——只追加**一个** `fs` 项，**只放行 fixture 所在目录**，
不扩大到任何不必要目录，`build` / `test` 段一字不动：

```ts
// 顶部新增两个 import
import path from "node:path";
import { searchForWorkspaceRoot } from "vite";

// server 段新增（其余配置保持不变）
server: {
  fs: {
    allow: [
      // 必须显式带回工作区根：一旦设置了 fs.allow，Vite 不再自动加入它
      searchForWorkspaceRoot(process.cwd()),
      // 只放行 fixture 目录，不放行整个仓库根
      path.resolve(process.cwd(), "../../tests/fixtures"),
    ],
  },
  // ...原有 proxy / watch 保持不变
},
```

**修复前 / 修复后对照（均在副本内验证，真实仓库未改任何配置）**：

```
修复前：server.fs.allow = [ "<copy>/apps/mesa-web",
                            "<vite>/dist/client" ]            fixture 在 fs.allow 内？= false
修复后：server.fs.allow = [ "<copy>/apps/mesa-web",
                            "<copy>/tests/fixtures",
                            "<vite>/dist/client" ]            fixture 在 fs.allow 内？= true
```

**反事实证据（为什么不能只写 fixtures 目录）**：若 `allow` 只写
`[path.resolve(process.cwd(), "../../tests/fixtures")]`，实测得到
`server.fs.allow = ["<copy>/tests/fixtures", "<vite>/dist/client"]` —— **工作区根被丢弃**
（Vite 只自动补回自己的 client 目录）⇒ dev server 将**无法服务 `src/**`**。
故最小**且正确**的改动必须显式带回 `searchForWorkspaceRoot(process.cwd())`。

> **未实测项（如实登记，不把推断写成实测）**：`pnpm build` / `vite build`（Rollup 生产构建）路径**本轮未实测**；
> `server.fs.*` 按 Vite 设计是 dev server 专用配置、**不应**影响 build，但本文件不声称已实测。

### 2.5.4 【本轮新增验收要求】生产构建验证与「Golden 只在测试中使用」的设计约束（修订 3-3）

| 项 | 内容 |
|---|---|
| **本轮已证明** | **TypeScript / Vitest 测试可使用共享 Fixture**（`tests/fixtures/nck-point-key-golden.json`），且**无需提前修改 `tsconfig.json` 或 `vite.config.ts`**（§2.5.2、§2.5.3；矩阵 A41） |
| **本轮不能证明** | **测试成功不能证明生产构建也可访问仓库外部路径** —— `pnpm build` 的模块解析 / Rollup 打包路径与 `vitest` 的测试路径**不等价**（dev server 侧已实测 `fs.allow` 不含仓库根，§2.5.2） |
| **新增验收要求（将来）** | **正式实现时需增加 `pnpm build` 验证**（门禁 **G11**，见 §11）；**若生产代码确实需要直接引用外部 JSON，再按实际失败决定是否调整 Vite 配置**（最小改动与修复前后对照见 §2.5.3） |
| **设计约束（更优先的方案，评审给定）** | **让 Golden 只在测试中使用，生产身份生成逻辑不要依赖 Fixture 文件。**<br>即：`nckPointModel.test.ts`（测试）可直读仓库级 Golden；**生产代码**（`nckPointModel.ts` 本体等）**不得** import 该 Fixture —— 生产身份生成只依赖 **Driver 下发的 `identity.template` / `identity.keys`**（§1.4 I-1、§4.3），从而**根本不触发** `vite.config.ts` 的 `fs.allow` 问题 |

> **处置变化**：按该设计约束，§2.2 中 `vite.config.ts` 的「条件修改项」在**正常实现路径下永不需要启用**；
> 只有在评审另行批准「生产代码直接引用外部 JSON」时，才按 §2.5.3 的最小改动处理，
> 且**必须先过 `pnpm build` 验证**（G11）。

---

# 3. 交付物二：数据流

`VariableDefinition → 表单 → PointSelection → Task Binding` 的完整链路。
**每一段都必须标明输入 / 输出 / 责任方**，凡是「谁生成、谁校验」含糊的地方即为设计缺陷。

## 3.1 链路总览

```
[Driver 内嵌数据]                        ① 目录（定义）
definitions/828d/r1.json ──────────────► DefinitionCatalog
  42 条定义（24 verified / 16 parameter_identity / 2 candidate；36 collect + 6 identity）
        │
        │ ② 浏览载荷（presets.rs:242-275，语义信封 + identity 块）
        ▼
POST /api/v1/endpoints/{id}/browse ─────► Core 逐字透传（core-api/src/lib.rs:864）
        │
        │ ③ 前端解析（PR-C 新增：browseModel.ts 透出 identity）
        ▼
nckPointModel.ts（消费 template/keys，通用替换；**不发明算法**）
        │
        │ ④ 表单（NckPointFields.tsx：Target / Arguments / 预览 Key / 高级原生）
        ▼
PointSelection{kind, point_key, parameters}   ← ⑤ ⑤
        │
        │ ⑥ 装配（唯一入口：deviceModel 共享函数）
        ▼
Task Binding{kind:"nck.points.v1", config:{points:[…]}}
        │
        │ ⑦ Core 门禁（格式 + Endpoint-wide 唯一性；**不推导身份**）
        ▼
PUT /api/v1/tasks/{endpoint_id}  →  Start  →  ⑧ Driver configure：重算 canonical_identity 并比对
```

## 3.2 分段契约（输入 / 输出 / 责任方）

| # | 段 | 输入 | 输出 | 责任方 | 证据 |
|---|---|---|---|---|---|
| ① | **目录** | `definitions/828d/r1.json`（42 条，含 `scope`/`semantics_status`/`source[]`/`targets`/`target_required`/`arguments[]`/`identity_template`/`identity_keys[]`/`golden_ref[]`） | `DefinitionCatalog`（进程内惰性装载，`presets.rs:217-222`）；装载失败只让**语义叶**退化 | **NCK Driver**（唯一所有者） | `definition.rs:50-97, 210-211`；`presets.rs:216-222` |
| ② | **浏览载荷** | 模板 id → `GoldenRef` → 目录定义；模板固定字段 + 已确认实例 | `binding_json`：语义信封 `{kind:"nck_variable", parameters:{variable_id,definition_revision,target,arguments}, identity:{template,keys,preview_point_key}}`；**未确认字段不出现**（不猜测、不给默认值） | **NCK Driver**（`presets.rs:242-275`） | `presets.rs:159-214`、`:241-275`、`:277-309`；实测 `tests/identity_browse.rs:148-197` |
| ③ | **前端解析** | `binding_json` 字符串 | `BrowseSelection{resource_id, parameters, identity?}`；空信封/非法 JSON → `null`（不可选） | **前端**（PR-C 新增） | 现状 `browseModel.ts:26-56`（丢弃 identity）；`:39` 空信封、`:31-34` 非法 JSON |
| ④ | **表单** | `identity{template,keys}` + 目标变量的 `targets/target_required/arguments` | 用户填写的 `target` / `arguments` / `display_name`；**预览 `point_key`**（通用替换结果，只读展示；语义模式下**不许用户随意改**） | **前端**（PR-C） | §6.1、§6.2；红线 §1.6 |
| ⑤ | **PointSelection** | 表单状态 | `{kind, point_key, parameters}`；`kind ∈ {nck_variable, nck_native}`；`parameters` 对 Core **完全不透明** | **前端构造**；`point_key` 的**内容**由 Driver 模板决定，前端只做通用替换 | `crates/core-types/src/point.rs:103-112`、`:861-865`（恰 3 字段 + `deny_unknown_fields`） |
| ⑥ | **装配** | `PointSelection[]` + 周期 | `Task Binding{kind:"nck.points.v1", config:{points:[…]}}`；**唯一装配入口**（消除 `deviceModel.ts:301` 与 `bootstrap.ts:138` 双真值） | **前端**（PR-C 收敛） | `resourceSelectionModel` 装配收敛（上位设计 `:214-215`）；`point.rs:82` |
| ⑦ | **Core 门禁** | Task Binding | 通过 / `UNSUPPORTED_BINDING` / `INVALID_BINDING_CONFIG` / `DUPLICATE_POINT_KEY`；**只做通用格式校验 + Endpoint 内跨 Task 跨 kind 唯一性**；**不解释 `parameters`、不推导 `point_key`** | **Core**（PR-A 已实现，PR-C 不改） | `core-api/src/binding_gate.rs:30`（kind 白名单）、`:58-67`、`:112`；`point.rs:888-912`、`:954`、`:13-16` |
| ⑧ | **Driver 校验（唯一执行真值）** | 已验证的语义字段 | 重算 `canonical_identity`；`identity_mode="derived"` 时与持久化 `point_key` **逐字比对**，不一致 ⇒ `POINT_KEY_MISMATCH` 且 **`configure` 整体失败、任务不得启动** | **NCK Driver**（PR-B 已实现，PR-C 不改） | `compiler.rs:489-520`、`lib.rs:611-621`、`:678-679` |

## 3.3 关键判据：为什么 ⑧ 决定了 ④ 不能偷懒

因为 ⑧ 是 **fail-closed 的唯一执行真值**（`compiler.rs:496`：`persisted != compiled.canonical_identity` ⇒ 拒绝），
前端在 ④ 生成的 `point_key` **必须**与 Driver 重算结果逐字相等。
任何「先估算、后纠偏」的路径都不成立：Core（⑦）**不**比对身份，所以错误**不会在保存时被发现**，
只会推迟到 **Start/Configure 时整体失败**（`lib.rs:617`「任一点失败即整体失败」）。
⇒ **前端必须在提交前就拿到正确身份**，这正是 §1 的 (B) 结论为何是 P0 阻塞项。

---

# 4. 交付物三：接口契约

## 4.1 变量目录来源

| 来源 | 内容 | 可用性 | 责任方 |
|---|---|---|---|
| **主来源（PR-C 采用）**：Browse 载荷 | `identity{template, keys, preview_point_key}` + `parameters{variable_id, definition_revision, target, arguments}` | **今天可用**（`presets.rs:262-274`，线级验证 `tests/identity_browse.rs:179-192`） | Driver 产出 / Core 透传 |
| **缺口来源（需求登记）**：I-2 定义目录端点 | `semantics_status`、`source[]`、`arguments[]` 类型/范围/必填、`label`、`scope`、`targets`/`target_required` | **今天不存在** | 见 §1.4 I-2 |
| 不使用 | `GET /api/v1/drivers/{id}/descriptor` 的 `variable` 资源 | 它是 **12 个原生字段**（`lib.rs:185-337`），**只**服务 `nck_native` 高级模式；**不得**作为语义模式的变量列表 | Driver 声明面（C10=D-A 保留） |

**变量覆盖度实测**：Browse 树（`nck://presets`）暴露 **62 个模板**；与定义目录交叉核对后
**62/62 命中**、退化 0 次 ⇒ 语义模式下 Browse **足以发现全部 62 个可采集入口**。

**但必须登记一个实测缺口（见 §9.3 待确认项 2）**：`PresetRegistry`/`semantic_binding` **不按 `scope` 过滤**。
`scope=identity` 的 **6 条定义**（`axis.name`、`axis.ctrlout.module_number`、`axis.encoder.module_number`、
`axis.ctrlout.type`、`channel.machine_axis_assignment`、`axis.spindle_assignment`）的 `golden_ref`
**全部出现在 preset 模板中**（实测 6/6），而 `definition.rs:51` 声明「`Identity` 只服务身份探针」。
生产代码中 `DefinitionScope` **仅在测试与类型定义里出现**（grep 实测：`definition_catalog.rs:70, 412, 434`），
**compiler / presets / configure 均不校验 `scope`** ⇒ **语义 Browse 今天会把「只服务身份探针」的变量
当作可采集点位提供给用户**。

> **PR-C 的处置（在授权范围内）**：前端**不得**自行用变量名或 `variable_id` 前缀猜测 `scope`
> （那属于「自行发明语义」）。因此：
> - **若 I-2 落地**：按 `scope === "collect"` 过滤，`identity` 条目**不进入**可采集列表（并显示原因）。
> - **若 I-2 未落地**：**必须**在 UI 上对所有变量统一显示「适用范围未提供（需定义目录接口）」，
>   **不得**默认它们都是可采集的；并把该缺口作为**独立 Driver 缺陷**上报（**已正式登记于 §10.2**，
>   建议后续 **PR-B2** 处理）。
> - **fail-closed 口径（§10.2）**：**PR-C 不得把缺失 `scope` 元数据的变量默认认定为可采集**；
>   若无法可靠区分，**保持禁用或不可确认状态，不得静默放行**。
> - **该缺口的权威修复点**在 Driver（`presets.rs` 或目录查询层加 `scope` 过滤），**不属 PR-C**。
>
> **【本轮补充】该缺口与 I-2 的 P0 关系（§1.4.3、§1.4.4）**：`scope` **只能**通过 I-2 或等价权威接口获取，
> 因此「缺 `scope` ⇒ 一律禁止提交」与「没有 I-2 ⇒ **语义点位的可提交路径实际上会被整体禁用**」
> 是**同一条约束的两面**；**禁止**用「硬编码 828D Target 组合」或「放宽 A40」绕过（§1.4.3）。
> 逐行降级行为以 **§1.4.4「Target 元数据降级规则」状态表**为准。

## 4.2 Target 约束

**Target 类型的权威定义在 Driver**（`drivers/sinumerik-nck/src/targets.rs`），Core 不得知道：

| 类型 | 位置 | 形态 |
|---|---|---|
| `TargetKind` | `targets.rs:46` | `channel` / `mode_group` / `axis` / `spindle` / `drive_object`（`parse` 于 `:67`） |
| `DriveEntry` | `targets.rs:96` | module **26** → `nc_variable`、module **130** → `drive_object`（`parse` 于 `:119`）—— **不允许折叠** |
| `DriveObjectRef` | `targets.rs:135-137` | `{number: u32, entry: DriveEntry}` |
| `TargetRef` | `targets.rs:208-218` | `{channel?, mode_group?, axis?, spindle?, drive_object?}` **全可选** |
| `TargetRef::validate` | `targets.rs:303-345` | ① 空 target ⇒ `MISSING_TARGET`；② 缺 `target_required` 任一 ⇒ `MISSING_TARGET`；③ 提供了 `targets` 未声明的变体 ⇒ `UNSUPPORTED_TARGET`；④ 实例号 `> MAX_TARGET_INSTANCE` ⇒ `INVALID_TARGET` |

**合法 Target 组合（实测 42 条定义的 `target_required` 分布）**：

| 组合 | 条数 | 例（`identity_template`） |
|---|---|---|
| `{drive_object}` | 20 | `drive.parameter.r0080@drive_object={drive_object}` |
| `{channel}` | 11 | `channel.program.status@channel={channel}` |
| `{axis}` | 5 | `axis.name@axis={axis}` |
| `{channel, spindle}` | 3 | `spindle.speed.actual@channel={channel},spindle={spindle}` |
| `{channel, axis}` | 2 | `axis.position.commanded_mcs@axis={axis},channel={channel}` |
| `{mode_group}` | 1 | （`B/S/opMode`，BAG 实例） |

> **PR-B 已裁决：`{channel, axis}`、`{channel, spindle}` 是合法复合 Target**
> （`targets.rs:303-322` 的校验表；组合**不在代码里硬编码**，换定义即换合法集）。
> ⇒ **PR-C 的表单必须按每个变量的 `target_required` 动态生成控件**，
> **禁止**在 TS 里硬编码「channel 必填」或「axis 与 channel 互斥」之类的规则。

## 4.3 身份生成与校验责任（**谁生成、谁校验、前端只做什么**）

| 环节 | 生成者 | 校验者 | 前端只做什么 |
|---|---|---|---|
| **身份模板 `identity_template`** | **Driver**（定义数据，`definition.rs`） | — | **只读消费**，**不得**自行拼装模板 |
| **身份键集合 `identity_keys`** | **Driver** | — | **只读消费**，**不得**自行决定/排序 |
| **预览 Key `preview_point_key`** | **Driver**（`presets.rs:249-253`） | — | **直接展示**；为 `null` 时禁止提交 |
| **用户改 target/arguments 后的 Key（§1.4.2 收紧）** | **首选：服务端权威 Preview**（该实例的新 Key 由 Driver/服务端编译给出）。降级路径才允许前端按 **Driver 模板**做通用替换 + 冻结的类型标签编码 | Driver 在 ⑧ 重算比对 | **不得沿用旧 `preview_point_key`**；降级路径产出的 Key **必须**标注「前端渲染、未经 Driver 校验」，**不得**视为「已由 Driver 验证的新实例身份」；**不得**反推/猜测；服务端 Preview 不可得时**标记待确认并禁止提交** |
| **「前端通用替换」能证明什么 / 不能证明什么** | — | — | **能证明**：编码规则与共享 Golden 一致（矩阵 A3/A4/A5）。**不能证明**：身份语义与 Driver 当前定义完全一致 ⇒ 需**明确的适用范围 + 跨层契约验证**（矩阵 A39/A41） |
| **`nck_native` + `identity_mode="derived"` 的 Key** | 前端按**固定规则** `native.<area>.<area_no>.<module>.<column>.<line>` | Driver 用同一规则重算（`compiler.rs:433`） | 实现已冻结规范；**不得**扩展该规则（不含 `count`/`decode`/`transport_size`/`invalid_*`/`identity_mode`，`definition.rs:1163-1165`） |
| **`nck_native` + `identity_mode="user"` 的 Key** | **用户显式指定**（**允许用户明确指定合法的 Native 自定义 Key**；本阶段**不实现自动生成 `~<n>` 后缀**） | Driver **不重算**，但复核命名空间 `native.` 与语法 | **不得自动改名**；Key 冲突时**拒绝**并提示用户自行改名；**身份保持原值**；**不依赖**任何不稳定的建议值算法（§9.3-7、矩阵 A25） |
| **持久化 `point_key`** | 前端写入（来自 ④ 的产出） | **Core**：通用格式 + Endpoint-wide 唯一性（`point.rs:413/954`、`binding_gate.rs:112`） | 提交前用**实际载荷**校验（§7.3） |
| **身份一致性（唯一执行真值）** | — | **Driver** `configure`（`compiler.rs:489-520`） | **不参与**；不得试图在 Core/前端复现该拒绝 |

**`identity_mode` 契约**：`derived`（默认）/ `user`；**`user` 仅 `nck_native` 允许**，
用于 `nck_variable` ⇒ `INVALID_BINDING_CONFIG`（`compiler.rs:356`、`:462-480`、PR-B `:866`）。
⇒ 语义模式的表单里**不得出现** `identity_mode` 控件。

**Core/Driver 双份格式校验的分工**（避免重复实现被误判）：
Core 的 `validate_point_key`（`point.rs:413`）校验**通用格式**；Driver 侧在
`nck_native` + `identity_mode="user"` 时**复核**同一通用格式（`compiler.rs:519`）。
前端若也实现类型标签编码（§1.6 允许），**必须**与 `tests/fixtures/nck-point-key-golden.json`
逐字一致 —— 该文件是**三方共用**的**同一份**真值：
`{value_cases: 55, value_reject_cases: 5, key_cases: 77}`（实测），
被 `crates/core-types/src/point.rs:985-988`（Core）与
`drivers/sinumerik-nck/tests/compiler_identity.rs:24, 1029`（Driver）引用，
PR-B 文档 `:654` 明确要求「**同一份复用**，PR-B **不得复制第二份**」，且断言 ③ 要求
grep 命中集合恰为 {Core, Driver, **前端(PR-C 时)**}，**不得**出现 `*copy*.json`。

---

# 5. 交付物四：C10 处置

> **P0 验收项**。目标**不是删除 Descriptor**，而是：保留兼容的资源元数据，
> 同时确保**用户不会从普通采集界面进入一个必然遭到 Driver 拒绝的旧配置流程**。

## 5.1 事实基座（PR-B 的 C10 裁决 = D-A）

| 事实 | 证据 |
|---|---|
| Descriptor 的 `variable` 资源**仍然存在**：12 个原生字段、`outputs=[value(DriverResolved)]` | `drivers/sinumerik-nck/src/lib.rs:185-337`（字段 `:193-326`、output `:328-335`） |
| `resource_selection_methods = [Manual, Browse]` **不收缩** | `lib.rs:339-342` |
| 该资源是**原生字段目录**（`nck_native` 的 12 字段可读来源），**声明面 ≠ 配置面** | PR-B `:980-982` |
| `configure` 只接受 `nck.points.v1`，旧 `mesa.resources.v1` **一律** `UNSUPPORTED_BINDING` | `lib.rs:648-657`；测试 `lib.rs:1499-1507`（`configure_rejects_legacy_resource_selection`） |
| Core 侧 `mesa.resources.v1` 仍是**合法** Task Binding Kind（S7/OPC UA 等仍用它） | `crates/core-api/src/binding_gate.rs:30`（`DATA_BINDING_KINDS = [GENERIC_BINDING_KIND, NCK_POINTS_BINDING_KIND]`） |
| ⇒ 残留风险：Descriptor 仍公告 `variable` + `Manual`，而该路径在 NCK 上**必被拒** | PR-B `:988`（**如实记录**）、`:1065` |

**⇒ 处置原则**：**Descriptor 逐字不动**（PR-C 零 Driver 改动）；**收敛发生在 UI 侧**。

## 5.2 旧 `variable` 资源进入 UI 的**实际路径**（逐步给证据）

```
① 前端取 Descriptor
   api.ts:92                        getJson(`/api/v1/drivers/${id}/descriptor`)
   EndpointAcquisitionPane.tsx:64   fetch(`/api/v1/drivers/${driverId}/descriptor`)
        │  （Core 路由：core-api/src/lib.rs:3241-3244）
        ▼
② Descriptor 里 variable 资源仍在（12 原生字段 + [Manual, Browse]）
   drivers/sinumerik-nck/src/lib.rs:185-337 / :339-342
        ▼
③ 资源选型器收到 resources + selectionMethods
   AddDeviceFlow.tsx:254-257   resources / existingSelections / selectionMethods / onAdd（**不传 endpointId**）
   EndpointAcquisitionPane.tsx:218-223  同上 + endpointId
        ▼
④ 默认进入「手动」页，rid = resources[0].id = "variable"
   ResourcePickerAntd.tsx:79-84   mode = methods.includes("manual") ? "manual" : "browse"
   ResourcePickerAntd.tsx:81      useState(resources[0]?.id ?? "")
   ResourcePickerAntd.tsx:188     资源下拉（NCK 只有 "variable" 一项）
        ▼
⑤ 按原生 schema 渲染 12 字段（area/area_no/module/column/line/count/unit_mode/decode/
   transport_size/invalid_value/invalid_tolerance/unit）
   ResourcePickerAntd.tsx:192-196 → DescriptorFields.tsx（冻结组件）
        ▼
⑥ point_key 由前端自动命名并可自由改写
   ResourcePickerAntd.tsx:41-51   suggestPointKey → "variable.value"（冲突则 .2/.3…）
   ResourcePickerAntd.tsx:224-232 自由文本 Input
        ▼
⑦ 加入 → 父层 reconciliation → sels
   ResourcePickerAntd.tsx:250-283 → applySelectionAdd（resourceSelectionModel.ts:329-377）
        ▼
⑧ 保存装配为 mesa.resources.v1 任务（**两条真值**）
   端点页：EndpointAcquisitionPane.tsx:123 mergeAcquisitionTasks
           → deviceModel.ts:280-304（:301 kind = CANONICAL_RESOURCES_KIND = "mesa.resources.v1"）
           → EndpointAcquisitionPane.tsx:146 PUT /api/v1/tasks/{endpointId}
   向导页：addDevice/bootstrap.ts:131-140（:138 **硬编码** "mesa.resources.v1"）
        ▼
⑨ Core 放行（mesa.resources.v1 是合法 kind）→ 任务入库
   binding_gate.rs:30
        ▼
⑩ **Driver configure 整体拒绝**
   drivers/sinumerik-nck/src/lib.rs:648-657 → UNSUPPORTED_BINDING
```

**这条链的终局**：用户**完整走完**「选资源 → 填 12 字段 → 设 point_key → 加入 → 保存」后，
任务在 **Start / Configure** 时被**整体拒绝**。这就是 C10 要阻断的「必然遭到拒绝的旧配置流程」。

## 5.3 三个已知嫌疑点的核实结果

### 嫌疑点 1：`resourceSelectionModel.ts` 的 `missingRequiredParams` —— **核实成立，且后果确认**

| 核实项 | 结果 | 证据 |
|---|---|---|
| 逻辑 | **对整个 schema 判定必填**：遍历 `schema.fields`，`f.required` 为真且参数中**键缺席**（或值为 `undefined`）即计入 | `resourceSelectionModel.ts:55-71` |
| 语义正确性 | `0`/`false`/`""` 视为**在场**（与 Core `obj.get` 对齐）；`null` 算在场；**只回答缺不缺**，不判类型/范围 | `:44-54`（注释）、`:61-69` |
| 调用点 | ① `ResourcePickerAntd.tsx:111-114`（渲染门）；② `:253-260`（onClick 二次防御） | 实测 |
| 禁用点 | `disabled={!outputs.length \|\| missingRequired.length > 0}` | `ResourcePickerAntd.tsx:249` |
| NCK `variable` 资源的 `required` 字段 | **9 个**：`area`、`area_no`、`module`、`column`、`line`、`decode`、`transport_size`（**无 default**）＋ `count`（default 1）、`unit_mode`（default `current`） | `lib.rs:195-325`（`required(true)` 处：`:196, 212, 221, 230, 238, 247, 257, 267, 285`） |
| 默认物化 | 仅 `count`/`unit_mode` 有 default，被 `materializeSchemaDefaults` 物化 | `ResourcePickerAntd.tsx:82-84, 139`；`resourceSelectionModel.ts:77-85` |

> **⇒ 语义模式下必然「永久阻止加点」**：语义信封的 `parameters` 是
> `{variable_id, definition_revision, target, arguments}`（`presets.rs:254-261`），
> 与 12 个原生字段**无一重合** ⇒ `missingRequired` 恒为 **7 个键**
> （`area`/`area_no`/`module`/`column`/`line`/`decode`/`transport_size`）
> ⇒ `加入` 按钮**恒 disabled**、onClick 也会再拦一次。**核实成立。**

### 嫌疑点 2：`bootstrap.ts` 硬编码字面量 vs `deviceModel.ts` 绑定组装 —— **核实成立（双真值）**

| 项 | 事实 |
|---|---|
| 装配真值 1 | `apps/mesa-web/src/deviceModel.ts:210` 定义 `CANONICAL_RESOURCES_KIND = "mesa.resources.v1"`；`:301` 用它组装 `binding: { kind: CANONICAL_RESOURCES_KIND, config: { selections } }` |
| 装配真值 2 | `apps/mesa-web/src/addDevice/bootstrap.ts:138` **硬编码字面量** `binding: { kind: "mesa.resources.v1", config: { selections } }`；该文件**未** import 任何 kind 常量（`:7` 只 import 类型 `ResourceSelection`） |
| 后果 | 任何 kind 变更（例如引入 `nck.points.v1`）**必须改两处**，否则出现「端点页保存通过、向导页保存失败」的分叉 |
| 上位裁决 | 两处**必须收敛为唯一装配入口**（上位设计 `:214-215`：「Binding 装配**真值 1**/**真值 2** → 重构 → **收敛**」） |

> **PR-C 处置**：把装配抽出为**唯一函数**（建议落在 `deviceModel.ts` 并导出，`bootstrap.ts` 引用它），
> **同时**让该函数按 `selections` 的目标 Driver 决定 kind（NCK → `nck.points.v1`）。
> 验收要求：**两处调用点的产出逐字相等**（§7 矩阵 A28）。

### 嫌疑点 3：`browseModel.ts` / `ResourcePickerAntd.tsx` / `ResourceBrowseAntd.tsx` 现状与改造点

| 文件 | 现状（实测） | 改造点 | 关键约束 |
|---|---|---|---|
| `browseModel.ts`（71 行） | 只认 `{resource_id, parameters}` 与 OPC UA 裸参数两种信封；`:39` 空信封 `{}` 永不可选；`:41-46` `resource_id` 回落到 `node.kind`；`:48-54` `identity` 被丢弃 | 增加 `nck_variable`/`nck_native` 判别；`BrowseSelection` 增可选 `identity` | **不得**把 `identity` 折进 `parameters`（它不属于点位参数，进了 `parameters` 会被 Core 视为不透明数据而**持久化**，违反 `presets.rs:168-169`）；`nck_native` 的 `parameters` **必须**是**12 原生字段**（**不得**把原生字段塞进语义信封，见 `tests/identity_browse.rs:163-177` 的反向断言） |
| `ResourcePickerAntd.tsx`（304 行） | `:108-114` 按**整份原生 schema** 判必填（嫌疑点 1）；`:41-51` `suggestPointKey = resource.output`；`:91-103` 语义叶回填失败（`nck_variable` 无对应资源）；`:136-141` 切资源时重置 params/outputs | 必填**按 Point Kind 分流**；语义 `point_key` 由 `identity` 推导且**只读**；`:91-103` 增加语义叶路由（切到语义表单而非手动表单） | `:152-154` 的 `applyBrowseFill` 目前只传 `resource_id/parameters` ⇒ 必须扩展为携带 `identity`（**类型级**修改，不是可选参数）；`:102` `setMode("manual")` 在语义路径上必须改为进入语义页 |
| `ResourceBrowseAntd.tsx`（139 行） | `:102` `selectableFromNode(n)` 判定可选用；`:121-123` 渲染「选用」；`:98` 失败文案；`:134-136` 翻页 | 语义叶 → 语义 Selection（带 `identity`）；无 `identity` / `preview` 为 `null` 时**显示原因并不给「选用」** | 现有「有 `sel` 才显示选用按钮」（`：121-123`）的机制可直接复用：**只要 `selectableFromNode` 对「缺 `identity` 的语义叶」返回 `null`，入口就自然关闭** —— 这是最小改动点 |

## 5.4 C10 处置方案（**选定方案**：UI 侧收敛，Descriptor 逐字不动）

| # | 措施 | 落点 | 验收 |
|---|---|---|---|
| C10-1 | **普通采集界面不再把 NCK 的 `variable` 资源作为语义点位的配置入口** | `ResourcePickerAntd.tsx:79-84/188`：当目标 Driver 为 NCK 时，普通模式**不**默认进入 `variable` 手动页；语义路径成为默认 | 矩阵 A21 |
| C10-2 | **旧流程入口必须显式降级为「高级原生配置」**，并明确文案：走它保存的任务在 NCK 上会被 `UNSUPPORTED_BINDING` 拒绝 | `ResourcePickerAntd.tsx` / `NckPointFields.tsx` 的入口切换与提示 | 矩阵 A22 |
| C10-3 | **`missingRequiredParams` 按 Point Kind 分流**：语义模式**不得**用 12 原生 schema 判必填 | `ResourcePickerAntd.tsx:108-114`、`:253-256` | 矩阵 A8、A19 |
| C10-4 | **装配收敛**：`deviceModel.ts:301` 与 `bootstrap.ts:138` 合并为唯一入口，并按 Driver 选 kind | 见嫌疑点 2 | 矩阵 A28 |
| C10-5 | **保留兼容元数据**：`variable` 资源、`resource_selection_methods`、`DescriptorFields` 渲染能力**全部保留** | 不改 Driver / Descriptor（`lib.rs:185-342` 逐字不动） | 矩阵 A33（回归） |
| C10-6 | **禁止「无效采集配置」**：任何情况下都**不得**为 NCK 生成 `mesa.resources.v1` 的采集任务 | 唯一装配入口内含 kind 决策；`tasks=[]` 路径不变 | 矩阵 A29 |

### 5.4.1 已知取舍（**必须在 PR-C 描述与 UI 中显式声明**）

`AddDeviceFlow.tsx`（向导第③步）**不传 `endpointId`**（实测 `:254-257`），
而 Browse 页在无 `endpointId` 时**被禁用**（`ResourcePickerAntd.tsx:160-171`，`disabled: !endpointId` 在 `:163`，
页面自带说明「浏览需要已创建的 Endpoint。」；`AddDeviceFlow.tsx:291` 亦有
「浏览设备（browse）需连接已存在，M4.2 暂只支持手动添加」）。
又因 **§8.2** 的 PR-D 边界（`AddDeviceFlow.tsx` 与 `AddDeviceFlow.test.tsx`（**包括对应测试**）均由 PR-D 独占）：
**PR-C 不修改这两个文件**。

⇒ **后果**：PR-C 之后，**向导页无法添加 NCK 语义点位**（语义入口需要 Browse/目录，
而 Browse 在向导页不可用；旧 `variable` 手动路径又已被 C10 收敛）。
**处置**：UI **必须**如实提示「语义点位需在设备详情页配置（保存设备后前往）」，
**不得**伪装成可用或静默回落到旧流程。NCK 语义点位的**向导内**整合归 **PR-D**（§8.2）。

---

# 6. 交付物五：UI 交互

## 6.1 语义模式（`nck_variable`，**默认**，普通用户唯一入口）

| 区域 | 内容 | 输入控件来源（**动态**，禁止硬编码） |
|---|---|---|
| 变量选择 | 按变量定义展示可选变量；显示 `label.zh`（若 I-2 落地）或变量标识 | Browse 树（`nck://presets`）：厂房/区域 → block → 变量；或 I-2 目录列表 |
| **参数来源与支持状态** | 必须显示三个字段并**如实区分**：`semantics_status`（`verified` / `candidate` / `parameter_identity`）、`source[]`（`manual` / `measurement` / `identity_probe` / `frozen_vector`）、**适用范围**（`scope`） | `definition.rs:59-69`、`:79-97`、`:51-57`。**I-2 未落地时显示「未提供」**，**绝不**用 `verified` 兜底 |
| Target | 由 `target_required` 决定**哪些**控件必填，由 `targets` 决定**哪些**可选 | `targets.rs:46`（5 种 kind）；实测组合：`{drive_object}`×20、`{channel}`×11、`{axis}`×5、`{channel,spindle}`×3、`{channel,axis}`×2、`{mode_group}`×1 |
| `drive_object.entry` | **必须显式选择** `nc_variable`(26) / `drive_object`(130)，**不得**折叠、**不得**由 `number` 反推 | `targets.rs:96-133`；PR-B「Module 26/130 **不允许折叠**」（`nck-semantic-point-refactor.md:1235`） |
| Arguments | 按 `arguments[]` 生成：`value_type`（`integer`）、`required`、`min`/`max` | 实测仅 **2 条**定义有 arguments：`channel.r_parameter`（`index` 0..65534）、`channel.machine_axis_assignment`（`position` 0..65535）。**必填条件**：`required=true` 且缺失 ⇒ 阻止提交 |
| **`point_key`** | **自动生成并校验**，语义模式下**只读展示**（可复制） | 来源：**未修改 `target`/`arguments` 时** = `identity.preview_point_key`（**逐字展示**）；**一旦用户修改 `target` 或 `arguments`，不得沿用旧 `preview_point_key`**（§1.4.2）——首选请求**服务端权威 Preview**，降级才按 `template`+`keys` 通用替换并**标注「前端渲染、未经 Driver 校验」**；服务端 Preview 不可得 ⇒ **待确认 + 禁止提交**。**不允许用户随意修改** |
| 输出 | `value`（`DriverResolved`，`lib.rs:328-335`）—— 类型在 Configure 时才能确定，UI **不得**猜测 | `types.ts:60-63` |

> **元数据降级（强制，逐行引用 §1.4.4）**：本节表格中凡依赖 `targets` / `target_required` / `arguments[]` / `scope`
> 的控件（Target、Arguments、变量可选性、`point_key`），一律按 **§1.4.4「Target 元数据降级规则」状态表**降级：
> **元数据不足时不得推断完整合法集合，也不得允许提交**（`只有 identity.keys` / `scope 缺失` /
> `scope=identity` / `无权威身份校验路径` / `Driver 不可达` 五种情形**一律为「否」**）。
> 该定位来自评审：**这套规则既满足 A39/A40，也不会把前端变成第二个 NCK Compiler。**

## 6.2 高级原生模式（`nck_native`）

| 要求 | 落实方式 | 证据 |
|---|---|---|
| **保留 `nck_native`，但必须是显式的高级配置入口** | 语义模式内的二级折叠/切换（**不是**默认页）；需用户主动展开 | §6.1 默认语义；上位设计 `:209` |
| **`identity_mode=derived` 为默认** | 不提供可见控件时按 `derived` 处理（`compiler.rs:462-480` 的默认） | `compiler.rs:462-463` |
| **`identity_mode=user` 必须显式选择** | 必须是一个**用户主动勾选/选择**的控件 + 二次确认；未选择即 `derived` | PR-B `:571`、上位设计 `:1045` |
| **遵守 `native.` 命名空间约束** | `user` 模式下 Key 必须匹配 `native_key ["~" dec]`，否则 Driver 报 `INVALID_USER_IDENTITY`；UI 实时提示 | `compiler.rs:67`（保留前缀）、`:512-521`；`point.rs:428-431`、`:1500-1529` |
| 既有必要寻址字段 + 高级参数 | 用**冻结的** `DescriptorFields` 渲染 `variable` 资源的 12 字段（`area`/`area_no`/`module`/`column`/`line`/`count`/`unit_mode`/`decode`/`transport_size`/`invalid_value`/`invalid_tolerance`/`unit`） | `lib.rs:193-326`；`types.ts:31-44` |
| **`~<n>` 后缀（【已裁决修正】本阶段不实现自动生成）** | **本阶段不实现自动生成 `~<n>` 后缀**；**允许用户明确指定合法的 Native 自定义 Key，禁止自动改名**；Key 冲突时**拒绝**并提示用户自行改名（**不**由前端静默追加后缀） | 上位设计 `:1036`、`:1044`；**§9.3-7（已裁决）**；矩阵 **A25** |
| **不得创建无效采集配置** | 原生模式产出的 `PointSelection.kind = "nck_native"`，**必须**装入 `nck.points.v1`（**不是** `mesa.resources.v1`） | `lib.rs:648-657`；§5.4 C10-6 |
| 同一地址两种用途的提示 | 若用户把同一地址用于不同逻辑采集（改 `decode` 等），UI **必须**提示「这将是两个不同点位」（否则用户以为还是同一个点） | 上位设计 `:1048` |

## 6.3 编辑与错误反馈

| 场景 | 必须的行为 | 依据 |
|---|---|---|
| 校验失败提示来源 | **禁止**用前端局部状态自证；错误文案必须区分「**未执行**」与「**执行失败**」（PR-D 验收亦要求） | 上位设计 `:1225` 指令 6 |
| Driver 校验错误透传 | Core/Driver 的 `code` + `message` **原样展示**（`UNSUPPORTED_BINDING`、`INVALID_BINDING_CONFIG`、`POINT_KEY_MISMATCH`、`MISSING_TARGET`、`UNSUPPORTED_TARGET`、`INVALID_USER_IDENTITY`、`DUPLICATE_POINT_KEY`）；**不得**吞掉或改写成乐观文案 | `lib.rs:648-657`；`compiler.rs:174`；`point.rs:948-975` |
| `POINT_KEY_MISMATCH` 文案 | 必须含三要素：**持久化 Key / Driver 重算 Key / `variable_id` + revision** | 上位设计 `:488` |
| 保存前校验 | 用**实际提交载荷**校验（§7.3） | 任务书状态一致性第 3 条 |
| 未失焦输入 | **不得**因输入未 `blur` 就保存旧值（§7.4） | 任务书状态一致性第 2 条 |
| 浏览不可用 | 保留既有诚实文案（`ResourceBrowseAntd.tsx:98`），并**禁止**据此推断成功 | 上位设计 `:1221` |
| 「未提供」诚实显示 | `source_label`/`semantics_status` 缺失时显示「未提供」，**绝不**用其他字段反推 | 既有先例：`useDeviceWorkspaceData.ts:56-57`（「绝不拿 point_key 反推」） |

---

# 7. 交付物六：验收矩阵

> **行数：44 行**（A1–A44；原 38 行 A1–A38；第一轮按评审新增 **A39–A41** 三行，见 §7.7；
> **第二轮按评审新增 A42–A44 三行**，见 §7.8）。
> 分组：正常路径 A1–A7、边界条件 A8–A14、错误输入 A15–A20、身份 A21–A27、
> 回归与装配 A28–A38、第一轮评审新增 A39–A41、**第二轮评审新增 A42–A44**。
> 「对应规则」列引用：上位设计 `nck-semantic-point-refactor.md`、PR-B 文档 `nck-prb-driver-compiler-boundary.md`、
> 本文件 §1–§6。

## 7.1 正常路径

| # | 测试名（建议） | 断言要点 | 文件与套件 | 证明什么 | 对应规则 |
|---|---|---|---|---|---|
| A1 | `semantic_leaf_exposes_identity_without_entering_parameters` | 语义叶解析后 `identity.template/keys/preview_point_key` **可读**；且 `identity` **不在** `parameters` 里 | `apps/mesa-web/src/browseModel.test.ts`（vitest） | §1.4 I-1 的接口真的被消费；身份不会被误当点位参数持久化 | 本文件 §1.4、§5.3-3；PR-B `:499` |
| A2 | `semantic_point_key_comes_from_driver_preview` | 未改 target/arguments 时，提交的 `point_key` **逐字等于** `identity.preview_point_key` | `nckPointModel.test.ts` | 身份来源是 Driver，不是前端算法 | 本文件 §1.6、§4.3 |
| A3 | `generic_substitution_uses_driver_template_and_keys_only` | 只按 `template` 占位符 + `keys[].path` 取值替换；**不**自行排序、**不**增删键 | `nckPointModel.test.ts` | 红线：不发明第二套身份算法 | 本文件 §1.6；上位设计 `:852` |
| A4 | `encoder_matches_shared_golden_vectors` | 对 `tests/fixtures/nck-point-key-golden.json` 的 `value_cases`(55) 与 `key_cases`(77) 逐条一致 | `nckPointModel.test.ts` | T5 三处一致（Core/Driver/前端同一份真值） | 本文件 §4.3；PR-B `:654`；上位设计 `:1248` |
| A5 | `encoder_reject_cases_fail_closed` | `value_reject_cases`(5) 逐条被拒（越界数字/`~` 未编码等） | `nckPointModel.test.ts` | 编码器不静默放行 | 上位设计 `:1366` |
| A6 | `target_controls_follow_definition_target_required` | `{channel,axis}` 与 `{channel,spindle}` 变量渲染出**两个必填**控件；`{drive_object}` 只渲染一个 | `NckPointFields.test.tsx` | 复合 Target 合法（PR-B 裁决）；不在 TS 里硬编码组合 | 本文件 §4.2；`targets.rs:303-322` |
| A7 | `payload_shape_is_exactly_three_fields` | 构造的 `PointSelection` 恰为 `{kind, point_key, parameters}`；无多余键 | `nckPointModel.test.ts` | Core 信封 `deny_unknown_fields` 会被多余键拒 | `point.rs:861-865` |

## 7.2 边界条件

| # | 测试名（建议） | 断言要点 | 文件与套件 | 证明什么 | 对应规则 |
|---|---|---|---|---|---|
| A8 | `missing_required_partitions_by_point_kind` | 语义模式**不**使用 12 原生 schema 判必填；对语义参数不报 `area/module/...` 缺失 | `ResourcePickerAntd.test.tsx` + `deviceModel.test.ts` | 修复 C10 嫌疑点 1（永久阻止加点） | 本文件 §5.3-1 |
| A9 | `semantic_required_arguments_block_submit` | `arguments[].required=true` 缺席 ⇒ `加入` disabled 且 onClick 拒绝 | `NckPointFields.test.tsx` | Arguments 必填条件 | 本文件 §6.1 |
| A10 | `preview_null_blocks_submit_with_reason` | `preview_point_key === null` ⇒ 禁止提交并显示「待确认字段 …」 | `NckPointFields.test.tsx` | 不猜测缺失实例 | `presets.rs:247-253`、`:186-190` |
| A11 | `zero_selections_still_bootstraps_without_tasks` | `selections=[]` ⇒ `tasks=[]` 且 `start=false` | `addDevice/bootstrap.test.ts` | C3-D 零采集点语义**不得**退化 | `bootstrap.ts:130-133, 153` |
| A12 | `preserved_tasks_are_untouched_on_nck_endpoint` | 编辑 NCK Endpoint 时，非 canonical 任务逐字保留（含另一种 kind） | `deviceModel.test.ts` + `EndpointAcquisitionPane.saveIssues.test.tsx` | 数据安全：不得静默删任务 | 上位设计 `:213`；`deviceModel.ts:280-304` |
| A13 | `scope_identity_variables_are_not_offered_as_collectable` | `scope=identity` 的 6 条**不**进入可采集列表；或（I-2 未落地时）显示「适用范围未提供」而**不**默认可采集 | `nckPointModel.test.ts` | 实测缺口（§4.1）不得被前端猜测掩盖 | 本文件 §4.1、§10.2（强化断言见 A40） |
| A14 | `drive_object_entry_is_never_inferred_from_number` | `number=26` **不**自动推出 `entry=nc_variable`；必须显式给 `entry` | `nckPointModel.test.ts` | Module 26/130 不允许折叠 | PR-B `:568`、上位设计 `:1235` |

## 7.3 错误输入

| # | 测试名（建议） | 断言要点 | 文件与套件 | 证明什么 | 对应规则 |
|---|---|---|---|---|---|
| A15 | `driver_unreachable_forbids_semantic_submit` | Browse 503 / 网络失败 ⇒ 语义模式禁止提交；**不**生成任何猜测 Key | `NckPointFields.test.tsx` + `browseModel.test.ts` | §1.5 降级行为（红线兜底） | 本文件 §1.5、上位设计 `:1203` |
| A16 | `leaf_without_identity_not_selectable` | 无 `identity` 的叶 ⇒ `selectableFromNode` 返回 `null`（入口自然关闭） | `browseModel.test.ts` | 「无身份不得进入语义模式」，且改动最小 | 本文件 §5.3-3、§1.5 |
| A17 | `empty_envelope_and_invalid_json_stay_unselectable` | `"{}"` / `" { } "` / `"{oops"` / `"[1,2]"` / `'"str"'` ⇒ `null` | `browseModel.test.ts`（既有断言**不得**放宽） | 既有回归护栏保持 | `browseModel.ts:31-39`；既有测试 `:47-77` |
| A18 | `native_parameters_must_not_leak_into_semantic_envelope` | 语义载荷**不含** `area_no`/`line`/`module`/`column`/`decode`/`transport_size` | `nckPointModel.test.ts` | 用户不面对协议字段 | `tests/identity_browse.rs:163-177` |
| A19 | `nck_never_emits_generic_binding_kind` | NCK 的任一装配路径都**不**产出 `mesa.resources.v1` | `deviceModel.test.ts` + `bootstrap.test.ts` | C10-6：不生成无效采集配置 | 本文件 §5.4；`lib.rs:648-657` |
| A20 | `identity_mode_user_rejected_for_semantic_kind` | 语义模式**不出现** `identity_mode` 控件；构造出的语义 `parameters` 也无该键 | `nckPointModel.test.ts` + `NckPointFields.test.tsx` | `user` 仅限 `nck_native` | `compiler.rs:356`、`:462-480`；PR-B `:866` |

## 7.4 身份

| # | 测试名（建议） | 断言要点 | 文件与套件 | 证明什么 | 对应规则 |
|---|---|---|---|---|---|
| A21 | `semantic_mode_is_default_entry_for_nck` | 进入 NCK 采集配置时**默认**是语义模式；旧 `variable` 手动页不再是默认 | `ResourcePickerAntd.test.tsx` | C10-1 | 本文件 §5.4 |
| A22 | `legacy_variable_path_is_labeled_as_advanced_and_unsupported` | 旧路径入口显式标注「高级原生配置」+「经此保存的旧式任务会被驱动拒绝」 | `ResourcePickerAntd.test.tsx` | C10-2：不把用户送进必败流程 | 本文件 §5.4；PR-B `:988` |
| A23 | `semantic_point_key_is_readonly` | 语义模式下 `point_key` 无输入框；仅可复制；改 `display_name` 不影响 `point_key` | `NckPointFields.test.tsx` | 语义 Key 不允许用户随意修改 | 本文件 §6.1；上位设计 `:1045` |
| A24 | `native_user_mode_requires_explicit_confirmation` | `identity_mode=user` 必须用户主动选择；Key 改动需二次确认 | `NckPointFields.test.tsx` | 附加 1 | 上位设计 `:1036/1044` |
| A25 | `native_custom_key_keeps_identity_and_rejects_conflict` | **本阶段不实现自动生成 `~<n>` 后缀**（§9.3-7 已裁决）：① 用户**明确指定**合法的 Native 自定义 Key 时，`point_key` **身份保持原值** —— **不得**被静默改名、**不得**被自动追加 `~<n>`；② 指定的 Key 与既有 Key **冲突时拒绝**（提示用户自行改名），**不得**静默改成别的名字；**断言不得依赖任何「建议值算法」**（不稳定的建议值**不作为**本阶段的实现与断言对象） | `NckPointFields.test.tsx` | **禁止静默改名 / 自动后缀**（F16 路径废弃）；**身份保持原值 + 冲突时拒绝** | 上位设计 `:1036`；**§9.3-7（已裁决：本阶段不实现）**；§6.2、§4.3 |
| A26 | `user_key_must_keep_native_namespace` | `user` 模式 Key 非 `native.` 前缀或不符 `native_key` 语法 ⇒ 前端阻止提交并提示 | `NckPointFields.test.tsx` | B-1 命名空间隔离 | `compiler.rs:512-521`；PR-B `:863` |
| A27 | `native_derived_key_uses_frozen_fixed_rule` | `nck_native`+`derived` 的 Key **恰为** `native.<area>.<area_no>.<module>.<column>.<line>`，**不含** `count`/`decode`/`transport_size`/`invalid_*`/`identity_mode` | `nckPointModel.test.ts` | §1.6 唯一允许的例外，且规则冻结 | `definition.rs:1163-1165`；PR-B `:573` |

## 7.5 回归与装配

| # | 测试名（建议） | 断言要点 | 文件与套件 | 证明什么 | 对应规则 |
|---|---|---|---|---|---|
| A28 | `binding_assembly_has_single_truth` | `deviceModel` 与 `bootstrap` 两处产出的 `binding` **逐字相等**；且 `bootstrap.ts` **不再**出现 `"mesa.resources.v1"` 字面量 | `deviceModel.test.ts` + `bootstrap.test.ts` | 修复 C10 嫌疑点 2（双真值） | 本文件 §5.3-2 |
| A29 | `ncK_kind_is_nck_points_v1_everywhere` | NCK 的投入产出 kind 恒为 `nck.points.v1`（端点页与向导页一致） | 同上 | C10-6 | 本文件 §5.4 |
| A30 | `switch_variable_clears_incompatible_target_and_arguments` | 切到另一变量后，上一变量不兼容的 `target` 成员与 `arguments` **不得残留** | `NckPointFields.test.tsx` | §7.6 反例 1 | 任务书状态一致性第 1 条 |
| A31 | `switch_target_kind_clears_incompatible_members` | 从 `{channel,axis}` 变量切到 `{drive_object}` 变量后，`channel`/`axis` 值不得残留并参与提交 | `NckPointFields.test.tsx` | 反例：复合 Target 切换残留 | 本文件 §4.2 |
| A32 | `unblurred_input_is_not_lost_on_submit` | 输入未 `blur` 直接提交 ⇒ 用**当前键入值**校验与提交，**不得**用旧值 | `NckPointFields.test.tsx` | 复用 `DescriptorFields` 的 `onDraftText` 先例 | `DescriptorFields.tsx:9-18`；任务书第 2 条 |
| A33 | `frozen_files_unchanged` | `src/connection/**` 9 文件 **134** 用例、`DescriptorFields*.test.*` **3** 文件、`ApplyWithRestart` 相关用例**原样通过** | vitest 全量 | 冻结面零退化 | 本文件 §2.3；上位设计 `:1212-1213` |
| A34 | `frontend_suite_baseline_not_regressed` | 全量用例数 **≥383**（现状 33 文件 / 383 用例，见 §9.2） | vitest 全量 | 基线不降 | 上位设计 `:1215` |
| A35 | `no_frontend_reimplementation_of_address_parser` | 生产代码中**不出现** NCK 原生协议常量（`area_no`/`transport_size`/`unit_mode`/`F32_BE` 等），**除** `nck_native` 表单的显式 12 字段声明外 | `nckPointModel.test.ts` + grep 门 | 红线：不重实现地址解析器 | 本文件 §1.6；上位设计 `:1293` |
| A36 | `resource_selection_reconcile_algorithm_unchanged` | `reconcileEditableSelection`/`applySelectionAdd` 的既有裁决语义（duplicate-editable / duplicate-protected / point-key-conflict / merge / append）逐条不变 | `deviceModel.test.ts`（既有用例**不得**删改语义） | 冻结算法本体 | 上位设计 `:255` |
| A37 | `task_snapshot_fail_closed_preserved` | 快照非 `ready` / 归属不符 ⇒ **禁止保存**（不得用空快照覆盖） | `EndpointAcquisitionPane.saveIssues.test.tsx` | 既有数据安全门不退化 | `deviceModel.ts:259-272` |
| A38 | `zero_production_hardcoded_nck_native_fields_in_semantic_path` | 语义路径**零**原生字段硬编码；原生字段只来自 Descriptor 的 `variable` 资源 | 同 A35 | 「换渲染入口，不删字段表」 | 上位设计 `:270` |

## 7.6 状态一致性（任务书第 4 条的四点 + **反例**）

| 要求 | 覆盖用例 | 反例（必须被拦） |
|---|---|---|
| 切换变量后**不得**保留不兼容的 Target / Arguments | A30 | 从 `channel.r_parameter`（有 `index` argument + `{channel}`）切到 `axis.name`（无 argument + `{axis}`）：若 `index` 与 `channel` 残留并被提交 ⇒ 身份被污染（`identity_keys` 不含它，但 `parameters` 里多出未知键 ⇒ Driver `INVALID_BINDING_CONFIG`） |
| **不得**因输入未 `blur` 就保存旧值 | A32 | 用户在 `line` 输入框键入新值后直接点「加入」：若提交旧值 ⇒ 身份与用户所见不一致 |
| 保存前必须用**实际提交载荷**校验（**不是** UI 局部状态） | A8、A19、A20 | UI 局部 state 显示「必填齐了」，但实际 `parameters` 少了 `arguments.index`：必须用即将提交的 `PointSelection` 再跑一次必填/格式预检 |
| 多点位**不得**生成冲突身份 | A28、A36 + `resourceSelectionModel.ts:223-231` | 同一 Endpoint 两个点位得到同一 `point_key`（例如同一变量同一 Target 加两次）：前端必须拦（`point-key-conflict`），且 Core 侧 `ensure_endpoint_point_keys_unique`（`point.rs:954`）是**最终防御** |
| **重新打开编辑器不得静默改变身份或修改既有采集目标** | A23、A25、A36 | 打开一个已保存的语义点位：`nckPointModel` **不得**因「模板重算」而改写 `point_key`；若 Driver 重算结果与已存 `point_key` 不同，**必须**显示差异并阻止提交，**不得**静默替换（否则会触发新 `point_id`、旧 key 走墓碑 —— 违反 R3/R4） |

## 7.7 【第一轮修订新增】评审新增（**A39–A41 三条，均含反例**）

| # | 测试名（建议） | 断言要点 | 文件与套件 | 证明什么 | 对应规则 |
|---|---|---|---|---|---|
| A39 | `edited_instance_never_reuses_stale_preview_point_key` | **正例**：未修改 `target`/`arguments` 时，提交 `point_key` 逐字等于 `identity.preview_point_key`。<br>**反例（必须被拦）**：修改 `target` 或 `arguments` 后 —— ① **不得**继续沿用旧 `preview_point_key`；② **不得**把该 Key 标记为「已由 Driver 验证」；③ 首选走**服务端权威 Preview**；④ 服务端 Preview 不可得时该实例**必须**被标记为**待确认并禁止提交** | `nckPointModel.test.ts` + `NckPointFields.test.tsx` | §1.4.2 收紧：`preview_point_key` 只是**特定实例**的身份预览，不等于任意 Target/Arguments 修改后的服务端实时编译结果 | 本文件 §1.4.2、§1.5、§4.3、§6.1 |
| A40 | `missing_scope_metadata_is_never_default_allowed` | **反例（必须被拦）**：变量**缺失 `scope` 元数据**（I-2 未落地 / 目录不可得）⇒ **不得**被默认为可采集、**不得**放行提交；UI 必须显示「适用范围未提供（需定义目录接口）」，`加入` 保持 **disabled 或不可确认**状态。<br>**正例**：`scope === "collect"` 才进入可采集列表；`scope === "identity"` 不进入并显示原因 | `nckPointModel.test.ts` + `NckPointFields.test.tsx` | §10.2 缺陷的 fail-closed 处置：**无法可靠区分时保持禁用，不得静默放行** | 本文件 §4.1、§10.2 |
| A41 | `shared_golden_fixture_is_consumed_without_a_second_copy` | 前端直接从仓库级 `tests/fixtures/nck-point-key-golden.json` 读取向量（**不改** `tsconfig.json`、**不新增** `server.fs.allow`、**不复制**第二份）；断言 `value_cases=55` / `value_reject_cases=5` / `key_cases=77` | `nckPointModel.test.ts`（构建期零配置改动） | ① PR-B `:654`「同一份复用、**不得复制第二份**」在前端可达；② 本轮实测：TS 侧与 Vitest 侧**均无需修改配置**（§2.5） | 本文件 §2.5、§4.3；PR-B `:654` |

> **A41 的能力边界（必须与 A39 一起读）**：A41 只能证明**编码规则层**的一致
> （同一份 Golden、同一份向量），**不能**据此认为「新实例的身份语义已被 Driver 验证」——
> 后者只能由**服务端权威 Preview** 或 Driver 的 ⑧ 重算比对（`compiler.rs:489-520`）证明。

## 7.8 【第二轮修订新增】评审新增（**A42–A44**，均含反例）

> 本节三行分别对应：**修订 2**（§1.4.4 状态表的缺口行）与**修订 3** 的三项验收要求
> （非 NCK Browse 回归、Native 不自动追加 `~<n>`、生产构建验证）。

| # | 测试名（建议） | 断言要点 | 文件与套件 | 证明什么 | 对应规则 |
|---|---|---|---|---|---|
| A42 | `identity_keys_only_never_unlocks_submit` | **正例**：`identity.keys` 可读时，只展示**已知身份字段**与「**待确认**」信息。<br>**反例（必须被拦）**：元数据**只有 `identity.keys`**（无 `targets` / `target_required` / `arguments[]` / `scope`）时 —— ① **不得**由 `keys` 推断「完整合法 Target 集合」或「`arguments[]` 的类型 / 范围 / 必填」；② **不得**据此放行提交（`加入` 保持 disabled / 不可确认）；③ **不得**用硬编码 828D Target 组合或放宽 A40 绕过 | `nckPointModel.test.ts` + `NckPointFields.test.tsx` | §1.4.4 状态表第 **2** 行；评审定位「**既满足 A39/A40，也不会把前端变成第二个 NCK Compiler**」 | 本文件 **§1.4.4**、§1.4.1 B、§1.6 红线第 3 条、§1.4.3 |
| A43 | `complete_metadata_is_the_only_submit_enabler` | **正例**：`targets` / `target_required` / `arguments[]` / `scope` **完整且有效** ⇒ 动态生成合法控件并执行校验，**在满足身份条件后允许提交**（身份条件含 §1.4.2：未修改则逐字复用 `preview_point_key`，修改后需权威 Preview）。<br>**反例（必须被拦）**：元数据完整但**身份未确认**、或 `scope=identity`、或 **Driver 不可达** ⇒ **仍禁止提交**（控件齐备 ≠ 可提交） | `NckPointFields.test.tsx` + `nckPointModel.test.ts` | §1.4.4 状态表第 **1** 行**唯一**「允许提交」行，且**以身份条件为前提**；其余各行一律为「否」 | 本文件 **§1.4.4**、§1.4.2、§1.5；矩阵 A2 / A6 / A13 / A15 / A39 / A40 |
| A44 | `nck_identity_is_optional_and_non_nck_browse_unchanged` | **两类回归测试（至少）**：① **NCK 带 `identity`** ⇒ 被识别为语义叶，`BrowseSelection.identity` 正确透出（且**不**进入 `parameters`）；② **非 NCK 不带 `identity`**（如 OPC UA 裸参数叶 / 通用 `mesa.resources.v1` 叶）⇒ **逐字保持既有行为**（`resource_id` 回落、`parameters`、可选性判定**全部不变**）。<br>**反例（必须被拦）**：非 NCK 叶**不得**因新增的**可选** NCK 专属字段而被判为语义叶、**不得**因此变得不可选、**不得**被要求提供 `identity` | `browseModel.test.ts` + `ResourceBrowseAntd.test.tsx` | **修订 3-1**：批准添加**可选** NCK 专属元数据，但**必须保证其他驱动的 Browse 叶节点仍按既有行为处理**（防「非 NCK 功能退化」，§2.4） | 本文件 **§2.2 `browseModel.ts` 行**、§9.3-5；矩阵 A16 / A17；§2.4 |

> **A44 与 A16/A17 的关系（避免重复计数）**：A16（无 `identity` 不可选）与 A17（空信封 / 非法 JSON 不可选）
> 覆盖的是**语义叶的准入**；**A44 覆盖的是「新增可选字段对非 NCK 驱动零影响」这一回归面**
> —— 两者**角度不同**，不得相互替代。

### 7.8.1 §1.4.4 状态表 → 矩阵行**覆盖映射**（逐行核对结论）

| §1.4.4 状态表行 | 允许提交 | 承担该行的矩阵断言 | 是否需补行 |
|---|---|---|---|
| `targets`/`target_required`/`arguments[]`/`scope` 完整且有效 | 满足身份条件后允许 | **A43**（新增，正例＋反例）、A6、A2 | **本轮补（A43）** |
| 只有 `identity.keys` | 否 | **A42**（新增，含反例） | **本轮补（A42）** |
| `scope` 缺失 | 否 | A40（既有，含反例）、A13 | 无需补 |
| `scope=identity` | 否 | A13（既有）、A40 | 无需补 |
| Target 或 Arguments 已修改，但无权威身份校验路径 | 否 | A39（既有，含反例） | 无需补 |
| Driver 不可达 | 否 | A15（既有，`driver_unreachable_forbids_semantic_submit`：Browse 503 / 网络失败 ⇒ 语义模式禁止提交、不生成猜测 Key） | 无需补（既有行已完整覆盖「禁止新增语义点位」） |

**⇒ 覆盖结论**：§1.4.4 六行**全部**有矩阵断言承担；其中第 1、2 行**本轮补**为 **A43 / A42**，
其余四行由既有 A40 / A13 / A39 / A15 承担（**逐行引用已在上表给出，无遗漏、无仅为本文件正文而无验收的行**）。

---

# 8. 交付物七：E2E 边界

## 8.1 PR-C **完成**什么

| # | 范围 | 说明 |
|---|---|---|
| 1 | 语义点位的**纯函数层**与**表单组件** | `nckPointModel.ts`、`NckPointFields.tsx` + 单测；消费 Driver 的 `identity` 块（§1.4 I-1） |
| 2 | Browse → 语义 Selection 的**映射** | `browseModel.ts` 透出 `identity`；`ResourceBrowseAntd.tsx` 语义叶路由 |
| 3 | 必填与 `point_key` 的**按 Kind 分流** | `ResourcePickerAntd.tsx`、`resourceSelectionModel.ts`（**仅扩类型/入参**，算法不动） |
| 4 | **装配收敛** | `deviceModel.ts` 成为唯一装配入口；`bootstrap.ts` 引用它（C10-4） |
| 5 | **C10 UI 收敛** | 普通采集界面不再通向必败的旧配置流程（§5.4） |
| 6 | NCK Endpoint **详情页**内的语义点位新增/编辑 | 唯一具备 `endpointId`（Browse 可用）的调用方：`EndpointAcquisitionPane.tsx:223` |

## 8.2 PR-C **不做**（明确留给 PR-D）

| # | 明确不做 | 依据 |
|---|---|---|
| 1 | **新增设备端到端向导整合**（`AddDeviceFlow.tsx` 的步骤重排为「先建 Endpoint 再选点位与 Target」） | 任务书 PR-D 边界；上位设计 `:220`、`:1264`（`AddDeviceFlow.tsx` **PR-D 独占**） |
| 2 | 向导第③步启用 Browse / 语义点位 | 需 `endpointId`，而向导中 Endpoint 尚未创建 ⇒ 属 PR-D 的步骤语义（上位设计 `:1207`、`:1223`） |
| 3 | 前端 **E2E** 用例（「建设备 → 建 Endpoint → 选点位与 Target → 启动」全链、离线分支不得标记成功） | 上位设计 `:1221` |
| 4 | 契约侧 **E2E**（REST 保存 → 启动 → `points/latest` 出现值；用 Simulator，**不得**连真机） | 上位设计 `:1222` |
| 5 | **真机 / 真实机床**验证 | 本轮硬约束；且属 Driver Gate（`drivers/sinumerik-nck/GATE.md`） |
| 6 | 性能验收（50K updates/s 等） | 归 CI `rust-perf`（§11） |

> **【本轮定向修订再次确认（修订 4）】PR-C / PR-D 文件边界不变**：
> `apps/mesa-web/src/addDevice/AddDeviceFlow.tsx` 与 `apps/mesa-web/src/addDevice/AddDeviceFlow.test.tsx`
> （**包括对应测试**）**继续归 PR-D 独占**，**PR-C 不修改**；
> §2.2、§5.4.1、§8.3、§12-10 的表述与本条**一致**，本轮四项修订**未引入**任何新的越界改动点。

## 8.3 与上位设计的**已知冲突**（如实登记，不抹平）

| 冲突 | 上位设计原话 | 本文件处置 | 理由 |
|---|---|---|---|
| `AddDeviceFlow.tsx` / `AddDeviceFlow.test.tsx` 的归属 | `:1207`（PR-C 节）把 `AddDeviceFlow.test.tsx` 列入「**必须改写**」；`:1223`（PR-D 节）又把它列入「**必须改写**（步骤语义最终形态）」；`:1264` 裁定 `AddDeviceFlow.tsx` 属 **PR-D 独占** | **状态：已裁决（评审裁决，不再待确认）**。<br>**裁决结论：`AddDeviceFlow.tsx` 与 `AddDeviceFlow.test.tsx` 均由 PR-D 统一处理；PR-C 不修改这两个文件，也不要求本阶段完成新增设备向导的完整语义点位流程。** | ① `:1264` 明确裁定 `AddDeviceFlow.tsx` 属 **PR-D 独占**；② `:1223` 要求「与 PR-C 的差异必须显式说明，禁止同一文件在两阶段被不同语义改两次而无验收」；③ 任务书给定「PR-C 不做新增设备端到端向导整合」⇒ 三者中只有「两个文件全部归 PR-D」自洽。**⇒ 上位设计 `:1207`/`:1223`/`:1264` 的内部冲突标记为「已裁决」，PR-C 侧以本条为准。**<br>**【本轮定向修订再次确认】** 该边界**不变**：`AddDeviceFlow.tsx` 与 `AddDeviceFlow.test.tsx`（**包括对应测试**）**继续由 PR-D 独占**，PR-C **不修改**；本轮四项修订均**未**新增对这两个文件的改动点（§2.2、§8.2、§12-10）。 |

---

# 9. 前端现状关键事实（实测，供评审复核）

## 9.1 测试基线：**33 文件 / 383 用例 —— 与既有基线一致（已核实）**

| 项 | 实测值 | 核实方式 |
|---|---|---|
| 测试文件数 | **33** | `Get-ChildItem -Recurse -Include *.test.ts,*.test.tsx` 计数 |
| 用例数 | **383** | 行首 `it(`/`test(` = **380**；其中 **3** 个 `it(` 与 `describe(` **同行**（`connectionSetupModel.test.ts:251`、`:334`、`deviceModel.test.ts:155`）⇒ 380 + 3 = **383** |
| `src/connection/**` | **9 文件 / 134 用例** | 行首计数 132 + `connectionSetupModel.test.ts` 的 2 个同行 `it(` = **134**（与上位设计 `:1212` 的 134 **一致**） |
| 校验旁证 | 宽松匹配 `\b(it\|test)\s*\(` = **387**；减去 4 处正则方法调用（`ResourceBrowseAntd.test.tsx:81`、`useConnectionSetup.test.ts:86/90/95` 的 `/…/.test(`）= **383** | 两条独立路径同得 383 |
| **实跑验证（本轮，替换此前的「未运行」）** | **`pnpm test` 退出码 `0`**：原始输出 `Test Files  33 passed (33)` / `Tests  383 passed (383)`（`vitest run`，`Duration 97.07s`）；**`pnpm typecheck` 退出码 `0`**：原始输出仅 `$ tsc --noEmit`，**零错误** | 本轮在 **HEAD `871fd11`**（PR #112 分支）上**实际执行**，前端运行时为 `C:\Users\34268\.dsh\dsh-runtimes\dsh-primary-runtime\dependencies\node\bin\node.exe` + `...\pnpm\bin\pnpm.mjs`；命令、退出码与原始输出的采集方式见 §2.5 |
| **基线结论** | **33 文件 / 383 用例 ⇒ 与既有基线一致**；静态计数与实跑结果**同值** | 上一行（实跑）＋ 上表三行（静态） |

## 9.2 其余已核实事实

| 事实 | 证据 |
|---|---|
| `missingRequiredParams` 对整个 schema 判必填；`0`/`false`/`""`/`null` 均视为在场；只回答「缺不缺」 | `resourceSelectionModel.ts:44-71` |
| 其两个调用点，且 `加入` 按钮以它为 `disabled` 条件 | `ResourcePickerAntd.tsx:111-114`、`:253-260`、`:249` |
| NCK `variable` 资源有 **9** 个 `required` 字段（其中 2 个有 default） | `drivers/sinumerik-nck/src/lib.rs:195-325` |
| `bootstrap.ts:138` 硬编码 `"mesa.resources.v1"`；该文件**未** import kind 常量 | `bootstrap.ts:7, 138` |
| `deviceModel.ts:210` 定义 `CANONICAL_RESOURCES_KIND`；`:301` 使用它 | 实测 |
| 前端对 `nck_variable`/`nck_native`/`nck.points.v1`/`preview_point_key` **零引用** | `grep` 全 `apps/mesa-web/src` |
| `browseModel.ts` 丢弃 `identity`；`BrowseSelection` 只有 2 字段 | `browseModel.ts:20-23, 41-54` |
| 语义叶 `resource_id` 回落到 `node.kind = "nck_variable"`；Picker 找不到该资源并告警放弃 | `browseModel.ts:41-45` + `presets.rs:194` + `compiler.rs:36` + `ResourcePickerAntd.tsx:93-97` |
| 62/62 preset 模板均命中定义目录（退化分支 0 次） | 本次逐条交叉核对（`presets/828d.json` ↔ `definitions/828d/r1.json`） |
| 定义目录 42 条：`scope` = collect 36 / identity 6；`semantics_status` = verified 24 / parameter_identity 16 / candidate 2 | 本次统计 |
| **不存在** `semantics_status = "hold"` 这一**结构化**取值；HOLD 只是文档/标签里的**散文**注记 | `SemanticsStatus` 枚举仅 3 值（`definition.rs:59-69`）；`HOLD` 仅出现在 `presets/README.md:49/80/92`、`GATE.md:97/108/180`、`r1.json:632` 的**标签文本**中 |
| 共享 golden fixture **存在**：`tests/fixtures/nck-point-key-golden.json`，`value_cases 55 / value_reject_cases 5 / key_cases 77` | 实测；被 `crates/core-types/src/point.rs:985-988` 与 `drivers/sinumerik-nck/tests/compiler_identity.rs:24, 1029` 引用 |
| `scope` 在生产代码中**未被任何路径校验** | `DefinitionScope` 仅出现在 `definition.rs:54/210`（定义）与 `definition_catalog.rs:70/412/434`（测试）；compiler/presets/configure 无引用 |
| 无定义目录 / 身份预览 REST 端点 | 路由表 `core-api/src/lib.rs:3238-3340` 全量核对 |
| Browse 载荷 `binding_json` 逐字透传到前端 | `core-api/src/lib.rs:864` |
| Core 无身份推导 API；`parameters` 不透明；信封层拒绝 `identity` 键 | `point.rs:13-16`、`:1613`、`:1730` |
| `configure` 旧 `mesa.resources.v1` ⇒ `UNSUPPORTED_BINDING` | `drivers/sinumerik-nck/src/lib.rs:648-657` |
| **前端可直读仓库级 golden fixture（TS 侧）**：`import "../../../tests/fixtures/nck-point-key-golden.json"` 在**不改 `tsconfig.json`** 的前提下 `tsc --noEmit` **零错误**；`--listFiles` 同时列出探针与 fixture；对照实验（植入 `TS2322`）证明探针确实被检查 | **本轮隔离副本实测**（§2.5） |
| **`resolveJsonModule` 的生效值为 `true`**（`tsconfig.json` 未显式声明），成因 `moduleResolution: "bundler"`；强制 `--moduleResolution node10` 立刻复现 `TS2732` | **本轮隔离副本实测**（`tsc --showConfig` + 反事实，§2.5.3） |
| **前端可直读仓库级 golden fixture（Vitest 侧）**：**不改 `vite.config.ts`** 的前提下 `Test Files 1 passed (1)` / `Tests 1 passed (1)`，读到 `value_cases=55 / value_reject_cases=5 / key_cases=77` | **本轮隔离副本实测**（§2.5.2） |
| **Vite dev server 默认 `fs.allow` 不含仓库根**：`searchForWorkspaceRoot` 因 `apps/mesa-web/pnpm-workspace.yaml` 存在而**停在 `apps/mesa-web`** ⇒ 若**生产代码** import 该 fixture，dev server 会以 `403` 拒绝（`server.fs.strict = true`） | **本轮隔离副本实测**（`resolveConfig`，**未启动 server**，§2.5.2/§2.5.3） |
| 前端测试基线 **33 文件 / 383 用例** 与既有基线一致（**实跑**，非仅静态计数） | 本轮 `pnpm test` 退出码 `0`；见 §9.1 |

## 9.3 无法确认 / 待确认（**如实列出，不抹平**）

| # | 项 | 状态 | 影响 | 建议的确认动作（评审/后续 PR） |
|---|---|---|---|---|
| 1 | 前端能否**直接** `import` 仓库级 `tests/fixtures/nck-point-key-golden.json`（**原「待确认」，本轮已实测结案**） | **已实测结案：按 PR-C 计划形态（fixture 仅被测试文件 import）⇒ `tsconfig.json` 与 `vite.config.ts` 两个都无需修改** | ① **TS 侧**：`tsc --showConfig` 生效值 `resolveJsonModule = true`（成因 `moduleResolution: "bundler"`；强制 `node10` 复现 `TS2732`）⇒ `tsc --noEmit` 零错误，`include` 无需扩展；② **Vitest 侧**：`pnpm test` 不启动 dev server，`server.fs.allow` 不参与，实测 1 文件 / 1 用例通过；③ **dev server 侧**：实测默认 `fs.allow` 不含仓库根 ⇒ 仅当**生产代码** import fixture 时才需要改 `vite.config.ts` **一处**（最小改动与前后对照见 §2.5.3） | 已执行（两配置未改）＝ §2.5；**本轮未启动任何 dev server**，dev server 结论由 `resolveConfig` + `searchForWorkspaceRoot` 求值 + Vite 自身 403 拒绝逻辑坐实 |
| 2 | `scope=identity` 的 6 条定义经 Browse 暴露为可采集变量 | **已确证存在；评审已裁决 ⇒ 已正式登记为独立 Driver 缺陷（见 §10.2，建议 PR-B2）** | 语义模式下 UI 会把「只服务身份探针」的变量当作可采集点位（§4.1）；**PR-C 的 fail-closed 处置**：缺 `scope` 元数据 ⇒ **不得默认可采集**，保持禁用/不可确认（矩阵 A40） | 权威修复在 Driver（`presets.rs`/目录查询加 `scope` 过滤）⇒ 登记为 **PR-B2 后端缺陷**；PR-C 只如实显示「适用范围未提供」 |
| 3 | I-2（定义目录端点）是否本 PR 前置 | **已裁决（见 §10.1）；定性已按【已裁决修正】升级**：**「PR-C 仅用 I-1 推进即可」只适用于前端基础模型与交互组件（L1）**；**I-2 是「完整语义点位配置」的 P0 功能依赖**，**L2 依赖它或等价权威接口，且不再可无限期降级** | 决定 §1 功能要求「参数来源/支持状态/无法读取原因」能落到什么程度；**并决定语义点位是否「可提交」**（缺 `scope` ⇒ A40 fail-closed ⇒ **可提交路径整体禁用**，§1.4.3） | 独立后端 PR（建议 `PR-B2-nck-definition-catalog-api`）；**推荐优先完成最小 I-2 接口，再完成 PR-C 的最终功能集成**；PR-C 走 §1.5 降级路径 |
| 4 | 上位设计内部冲突：`AddDeviceFlow.tsx` / `AddDeviceFlow.test.tsx` 归 PR-C 还是 PR-D | **已裁决（见 §8.3）**：**两个文件均由 PR-D 统一处理，PR-C 不修改** | — | 评审裁决已写入 §8.3，冲突标记为「已裁决」 |
| 5 | `identity` 块进入 `BrowseSelection` 后是否会被**其他** Driver 的浏览叶误解 | **已裁决（本轮，修订 3-1）** | 批准添加**可选**的 NCK 专属元数据，但**必须保证其他驱动的 Browse 叶节点仍按既有行为处理**；**至少增加两类回归测试**：**NCK 带 `identity`** 与 **非 NCK 不带 `identity`** | 矩阵 **A44**（新增，含反例：非 NCK 叶不得被判为语义叶、不得因此变得不可选）；A16 / A17 覆盖既有准入形态，**不可替代** A44（§7.8） |
| 6 | 高级原生模式是否需要在服务端限权 | **上位设计已明确排除** | `nck_native` 服务端限权**不在**本 PR（上位设计 `:941`：仅沿用只读基线 + 前端警告口径，**PR-C 落地**） | PR-C 只做前端警告，**不得**新增服务端权限逻辑 |
| 7 | `~<n>` 建议值的具体生成规则（何时建议、如何避免与既有 key 冲突） | **已裁决：本阶段不实现**（原「待确认」，本轮按评审裁决结案） | 不再影响任何断言（A25 已改写，**不再依赖建议值算法**） | **本阶段不实现自动生成 `~<n>` 后缀**；**允许用户明确指定合法的 Native 自定义 Key，禁止自动改名**；冲突时**拒绝**并非静默改名（§6.2、矩阵 A25） |
| 8 | 是否存在 PR-C 未列出的 Browse 消费方 | **已核实：无** | `selectableFromNode` 的调用点只有 `ResourceBrowseAntd.tsx:102` | — |
| 9 | **生产构建**（`pnpm build`）能否访问仓库外部路径（`tests/fixtures/**`） | **本轮未验证 —— 明确登记为「将来验收要求」**（不是本轮结论） | 本轮只证明 **TypeScript / Vitest 测试**可使用共享 Fixture，且**无需提前修改 `tsconfig.json` / `vite.config.ts`**；**测试成功不能证明生产构建也可访问仓库外部路径** | **正式实现时增加 `pnpm build` 验证**（门禁 **G11**，§11）；**若生产代码确实需要直接引用外部 JSON，再按实际失败决定是否调整 Vite 配置**（§2.5.3 最小改动）。**设计约束（更优先的方案）**：**让 Golden 只在测试中使用，生产身份生成逻辑不要依赖 Fixture 文件**（§2.5.4、§4.3） |

---

## 9.4 本轮实测命令清单（供评审复核；**未执行任何 git 写操作**）

| # | 命令 / 方法 | 执行位置 | 结果 |
|---|---|---|---|
| 1 | `pnpm typecheck`（即 `tsc --noEmit`） | 真实仓库 `apps/mesa-web`（HEAD `871fd11`） | 退出码 **0**，零错误 |
| 2 | `pnpm test`（即 `vitest run`） | 真实仓库 `apps/mesa-web`（HEAD `871fd11`） | 退出码 **0**；`Test Files 33 passed (33)` / `Tests 383 passed (383)` |
| 3 | `pnpm typecheck`（**含探针**，配置未改） | **隔离副本** `%TEMP%\mesa-prc-probe-871fd11\apps\mesa-web` | 退出码 **0**，零错误 |
| 4 | `tsc --noEmit --listFiles`、`tsc --showConfig`、`tsc --noEmit --moduleResolution node10`（反事实） | 隔离副本 | 见 §2.5.2 / §2.5.3 |
| 5 | `vitest run src/__fixture_probe__.test.ts` | 隔离副本 | 退出码 **0**；1 文件 / 1 用例通过 |
| 6 | `node __probe_vite_allow__.mjs`（`vite.resolveConfig` + `searchForWorkspaceRoot`，**不 listen、不启动 server**） | 隔离副本 | 退出码 **0**；见 §2.5.2 |
| 7 | `git status --porcelain`、`git rev-parse HEAD`、`git rev-parse --abbrev-ref HEAD`（**只读**） | 仓库根 | 执行前后均为**干净**（空输出） |

**本轮明确未做**：未启动 Vite dev server、未启动 `mesad`、未连接任何真实设备或机床、
未执行任何 git 写操作、未修改 `apps/mesa-web` 的任何生产源码或配置文件、未修改本文件以外的任何文档。
隔离副本 `%TEMP%\mesa-prc-probe-871fd11` **已删除**（`Test-Path` 复核为 `False`）。

> **方法学副作用与事故记录（如实登记；表述已按评审修正，不再使用「逐字恢复 / 完全一致」口径）**：
> 该隔离副本的 `node_modules` 采用**链接到真实仓库 `.pnpm`** 的方式构造（避免复制 190MB / 2.6 万文件）。
> 在副本内执行 `pnpm install`（pnpm 11 的 `verify-deps-before-run` 触发）时，pnpm 以**副本为项目根**
> 重算了链接目标，**穿透写入真实仓库** `apps/mesa-web/node_modules/.pnpm`，
> 把其中 **833 个 junction 的目标改成了副本路径**（1213 个中的 833 个）—— 这是一次**真实事故**。

**【已裁决修正】事故表述的准确边界（评审第 5 项）**：

| 项 | 结论 |
|---|---|
| **可确证** | ① **功能验证通过**：真实仓库 `pnpm typecheck` 退出码 **0**、`pnpm test` **33 文件 / 383 用例**全绿（退出码 `0`）、**无悬空链接**；② **无指向副本的残留链接**（指向临时副本的链接计数为 **0**）；③ junction 总数 **1213**、真实文件数 **26718**；④ `git status --porcelain` **为空** |
| **不可确证** | **与原状态的逐字一致性** —— 事故发生前**没有快照**，因此**不应声称** junction 已「**逐字恢复**」（也不应声称与事故前**完全一致**、不应给出「1196/1213 已逐字恢复」一类的结论）。本轮**不再做任何安装类操作** |
| **阻塞关系** | 该事故**不构成设计 PR 的阻塞项**（设计交付物不受影响） |

**禁止性纪律（评审第 5 项，硬性）**：

1. **后续禁止使用「连接真实依赖 Store 的临时副本」执行安装** —— 这正是**穿透改写 junction** 的成因
   （命中真实仓库 `apps/mesa-web/node_modules/.pnpm`）；同类探针**一律不得**在
   「`node_modules` 链接进真实仓库」的副本内执行任何包管理器命令。
   若要复制 `node_modules`，应复制为**真实文件**（或在**完全隔离**的副本内用 `pnpm install --offline` 独立重建）。
2. **正式开发前做一次依赖完整性与构建检查**（门禁 **G0**，见 §11），并按 **G11** 增加生产构建验证。

> **第二次定向修订（本次修订动作）的执行边界（如实登记）**：本次修订**只修改本文件**；
> **未运行任何测试或构建**（§2.5.4 新增的 `pnpm build` 也只是**登记为将来的验收要求**，**本轮不执行**）、
> **未执行任何安装类操作**（严格遵循上面的禁止性纪律 1）、**未启动 Vite dev server**、**未启动 `mesad`**、
> **未连接任何真实设备或机床**、**未执行任何 git 写操作**。
> 文中 §9.1 / §2.5 的「本轮实测」数字均**沿用第一次的实跑与隔离副本探针**，本次修订**未重复执行**、其结论未被改动。

---

# 10. 后端依赖与 Driver 缺陷登记（**正式登记**）

> 本节是**登记**，不是 PR-C 的施工范围。两条登记项的共同要求是：
> **前端不得用猜测填补后端缺口**（§1.6 红线），
> 且在缺口存在时**保持 fail-closed（禁用 / 不可确认），不得静默放行**。

## 10.1 I-2：新增只读「变量定义目录」端点 —— **独立后端接口工作**【已裁决修正：**完整语义配置的 P0 功能依赖**】

| 项 | 登记内容 |
|---|---|
| **性质** | **独立的后端接口工作**（Driver SDK 新增能力 + Core 逐字透传 + 可能触及 `proto/**`），**不属于 PR-C 授权范围** |
| **是否 PR-C 基础开发前置** | **【已裁决修正】分层回答**：**前端基础模型与交互组件（L1，§1.7）不依赖它**，可先行开发（**旧口径「不作前置」仅在此范围内有效**）；但 **I-2 是「完整语义点位配置」的 P0 功能依赖** —— 没有 I-2，按当前文档约束，**语义点位的可提交路径实际上会被整体禁用**（推理链见 §1.4.3） |
| **是否阻塞完整功能验收** | **是（P0）**。PR-C 的 **L2（完整语义配置闭环）验收依赖它或等价权威接口**：参数来源 / 支持状态 / 无法读取原因、`scope` 过滤、`targets`/`target_required`/`arguments[]` 约束渲染、**服务端权威 Preview**。**L2 不再可无限期降级**（§1.7 验收纪律 2、§1.4.3） |
| **实施分层（调整后的三层，评审给定）** | ① **PR-C 基础开发**：Browse 解析、PointSelection、Binding、C10 隔离、高级原生模式、基本表单；② **独立后端接口 PR（I-2）**：定义目录、`scope`、支持状态、Target/Arguments 约束，**并核对服务端 Preview 能力缺口**；③ **PR-C 完整功能验收**：服务端权威元数据可用，Target 动态生成、身份正确、采集提交可用 |
| **评审实施建议（原样登记）** | **推荐优先完成最小 I-2 接口，再完成 PR-C 的最终功能集成，而不是交付一个所有语义点位都无法提交的界面。** |
| **禁止的绕过方式（评审原话）** | **不建议通过硬编码 828D Target 组合或放宽 A40 来绕过这个问题**（§1.4.3）；**不得**把 `identity.keys` 当作 `targets` / `target_required` / `arguments[]` 的替代（§1.4.4 第 2 行、矩阵 **A42**） |
| **归属建议** | 独立 PR（建议命名 `PR-B2-nck-definition-catalog-api`）；**不得**把后端改动混入 PR-C 的前端改动 |
| **接口形态** | 见 §1.4 I-2（`GET /api/v1/drivers/{driver_id}/definitions?family=828d`；目录内容**原样透传**，Core **不得**解释，以守住「Core 不懂协议」） |
| **I-2 未落地时 PR-C 的义务** | ① 「支持状态 / 参数来源」如实显示为 **「未提供（需定义目录接口）」**，**绝不**用 `verified` 兜底；② **不得**默认全部变量可采集（§10.2）；③ 验收结论**必须**写明 **L1 达成 / L2 未达成**（§1.7） |

## 10.2 `scope=identity` 缺陷 —— **独立 Driver 缺陷（比普通 UI 缺陷更重要）**

> **评审裁决**：本条**不是**普通 UI 缺陷，而是**独立的 Driver 缺陷**，建议由后续 **PR-B2** 处理。

### 10.2.1 缺陷事实（§4.1 实测）

- `PresetRegistry` / `semantic_binding` **不按 `scope` 过滤**（`drivers/sinumerik-nck/src/presets.rs`）；
- `scope=identity` 的 **6 条定义**（`axis.name`、`axis.ctrlout.module_number`、`axis.encoder.module_number`、
  `axis.ctrlout.type`、`channel.machine_axis_assignment`、`axis.spindle_assignment`）的 `golden_ref`
  **全部出现在 preset 模板中**（实测 **6/6**），而 `definition.rs:51` 声明「`Identity` 只服务身份探针」；
- `DefinitionScope` 在生产代码中**从未被校验**（仅出现在类型定义与测试中）
  ⇒ **语义 Browse 今天会把「只服务身份探针」的变量当作可采集点位提供给用户**。

### 10.2.2 必须写明的产品语义

| `scope` | 产品语义（Driver 目录层必须表达清楚） |
|---|---|
| `collect` | **可采集**：该变量可被用户配置为采集点位 |
| `identity` | **只服务身份探针**：用于身份判定 / 探测，**不是**面向用户的采集点位，**不得**作为可采集项出现在普通采集界面 |

### 10.2.3 责任归属（**权威修复点不在前端**）

- **由 Driver 的 Browse / 目录层负责提供正确的可采集能力**（在 `presets.rs` 或目录查询层按 `scope` 过滤，
  或至少把 `scope` 元数据如实体现在可采集能力上）——**这是 Driver 侧的责任，不属 PR-C**。
- 前端**必须**把缺失的 `scope` 当作**「不可确认」**处理，**不得**用变量名 / `variable_id` 前缀猜测。

### 10.2.4 PR-C 的硬性约束（**fail-closed，可验收**）

> **PR-C 不得把缺失 `scope` 元数据的变量默认认定为可采集。**
>
> **若无法可靠区分，则保持禁用或不可确认状态，不得静默放行。**

对应可验收断言：矩阵 **A40**（含反例「**缺失 `scope` 的变量不得被默认放行**」）。

### 10.2.5 阻塞关系（**必须分清，不得含糊**）

| 问题 | 结论 |
|---|---|
| 是否阻塞 **PR-C 基础组件开发**（L1） | **不阻塞**。数据模型、C10 入口隔离、`preview_point_key` 的展示与复用、装配收敛、高级原生模式与错误反馈均可先行完成 |
| 是否阻塞 **「所有可见语义变量均可安全配置」的功能验收声明** | **阻塞**。在该缺陷修复（或 I-2 提供 `scope` 元数据）之前，**不得**做出该声明；验收只能声明「缺失 `scope` 的变量处于**不可确认 / 禁用**状态」 |

---

# 11. 门禁清单（PR-C 完成时）

| # | 门禁 | 命令（**PR-C 实施时逐轮执行**；**G1/G2 已在本轮作为基线实跑**，见 §9.1/§9.4） | 通过标准 |
|---|---|---|---|
| **G0** | **前置：依赖完整性与构建检查（评审第 5 项纪律；正式开发前执行一次）** | ① 依赖完整性检查（工作区 / lockfile 与 `node_modules` 是否一致、是否存在**悬空链接**；只读校验或 `pnpm install --frozen-lockfile`）；② 构建检查（`pnpm --dir apps/mesa-web build`） | **无悬空链接、无指向「临时副本 / 真实依赖 Store 副本」的残留链接**；构建失败即**先修复再开工**。**前置纪律（硬性）**：**禁止使用「连接真实依赖 Store 的临时副本」执行安装**（§9.4 禁止性纪律 1，即穿透改写 junction 的成因） |
| G1 | 前端测试 | `pnpm --dir apps/mesa-web test`（即 `vitest run`） | **全绿**；用例数 **≥383**（本轮实测基线 33 文件 / 383 用例，退出码 `0`）；`src/connection/**` 的 **134** 不得下降 |
| G2 | 类型检查 | `pnpm --dir apps/mesa-web typecheck`（`tsc --noEmit`） | 零错误（本轮实测退出码 `0`）；**§9.3-1 已结案：`resolveJsonModule` 无需显式设置**（`moduleResolution: "bundler"` 下生效值为 `true`），`tsconfig.json` **不需要**修改 |
| G3 | Lint | `pnpm --dir apps/mesa-web lint`（`eslint .`） | 零错误 |
| G4 | 冻结面 | 见 §2.3 | `src/connection/**`（9 文件/134）、`DescriptorFields*`（3 文件）、`ApplyWithRestart` 相关用例**原样通过**；`resourceSelectionModel.ts:169-377` 算法语义不变 |
| G5 | **红线门（grep）** | `grep -r "nck-point-key-golden" --include=*.rs --include=*.ts,*.tsx` | 命中集合恰为 {Core, Driver, 前端}；**不得**出现 `*copy*.json`（PR-B `:654`） |
| G6 | **红线门（grep）** | 前端生产代码检索 NCK 原生协议常量 | 除 `nck_native` 表单的显式 12 字段声明外**零命中**（矩阵 A35/A38） |
| G7 | **C10 门** | 矩阵 A19/A21/A22/A28/A29 | NCK **不**产出 `mesa.resources.v1`；旧路径显式标注为高级且会被拒 |
| G8 | Rust 门禁 | `cargo build --workspace` + `cargo test --workspace` | **仅当触碰 Core/Driver 时必跑**。**PR-C 原则上不改后端**（§2.4）⇒ 默认**不需要**；若由 **§10.1 的 I-2 后端 PR** 引入后端改动，则由**该后端 PR** 承担此门禁，并需 `cargo build --locked --workspace` |
| G9 | 契约测试 | `python scripts/write-contract-evidence.py`（29 套） | 同上：**仅后端 PR 承担**；PR-C 不改 `drivers/**`、`crates/**`、`tests/driver-contract/**` ⇒ 不在 PR-C 门禁内 |
| G10 | 性能 | CI `rust-perf` | **不在 PR-C 范围**（性能预算 §22 归 CI） |
| **G11** | **生产构建验证（本轮新增验收要求 3-3；尚未执行）** | `pnpm --dir apps/mesa-web build`（= `tsc -b && vite build`，`apps/mesa-web/package.json:8`） | **必须成功**。**理由**：本轮已证明 **TypeScript / Vitest 测试可使用共享 Fixture 且无需提前改配置**，但**测试成功不能证明生产构建也可访问仓库外部路径**（§2.5.4、§9.3-9）。**若生产代码确实需要直接引用外部 JSON 而失败** ⇒ 按 §2.5.3 的**最小改动**处理 `vite.config.ts`（**只此一处**），且**必须先过本门禁**。**优先方案（设计约束）**：**让 Golden 只在测试中使用，生产身份生成逻辑不要依赖 Fixture 文件** ⇒ 正常情况下本门禁**不触发**任何配置改动 |

**PR-C 门禁的最小集合**：**G1–G7**（**正式开工前另需 G0**：依赖完整性与构建检查）。
G8/G9 仅在评审裁决要求引入 §1.4 I-2 后端改动时才启用，
且该改动**必须**独立成 PR（不得混入 PR-C 的前端改动中）。
**G11 属「正式实现阶段」的新增验收要求**（本轮未执行，见 §2.5.4、§9.3-9）：
**不得**用 G1/G2 替代 G11 —— 测试路径与生产构建路径**不等价**。

---

# 12. 结论摘要（给评审）

1. **§0 结论 = (B)**：前端**不存在**可直接获取 Driver 身份计算结果的正式接口。
   但性质是「后端已实现、线级已承载、前端未消费」——
   载体是浏览叶 `binding_json.identity{template, keys, preview_point_key}`
   （`presets.rs:242-275`，逐字透传于 `core-api/src/lib.rs:864`），
   它在 `browseModel.ts:20-23/41-54` 被**结构性丢弃**，且 `ResourcePickerAntd.tsx:93-97`
   连语义叶的 `resource_id="nck_variable"` 都无法映射。
2. **最小接口方案**：**I-1**（零后端改动，PR-C 内完成）把既有 `identity` 块正式化为只读契约，
   其**可独立交付的能力清单与不可独立支撑的部分**已在 **§1.4.1** 明确写出；
   **I-2**（定义目录端点）是唯一能满足「参数来源/支持状态/无法读取原因」的形态，
   **已正式登记为独立后端接口工作（§10.1）**。
   **【已裁决修正】I-2 定性升级（推翻此前「仅用 I-1 推进即可 / 不作基础开发前置」的旧口径）**：
   该旧口径**仅适用于前端基础模型与交互组件**（§1.7 的 L1），
   **不适用于可实际提交的完整语义点位配置**；对完整语义点位配置，**I-2 是 P0 功能依赖**（§1.4.3）：
   I-1 只能提供既有 Browse 叶的 `identity` **预览**及**部分参数**，
   **`identity.keys` 不能替代 `targets`、`target_required`、`arguments[]` 的完整定义**；
   **A40 规定缺失 `scope` 时不得默认允许采集**，而 **`scope` 只能通过 I-2 或等价权威接口获取**
   ⇒ **没有 I-2，按当前文档约束，语义点位的可提交路径实际上会被整体禁用**
   （§1.7 的 **L2 不再可无限期降级**）。
   **明确禁止两条捷径（评审原话）：不建议通过硬编码 828D Target 组合或放宽 A40 来绕过这个问题。**
   评审建议：**推荐优先完成最小 I-2 接口，再完成 PR-C 的最终功能集成，
   而不是交付一个所有语义点位都无法提交的界面**（三层实施分层见 §1.4.3、§10.1）。
   **实例身份规则已按评审收紧（§1.4.2）**：Browse 的 `preview_point_key` 只是**特定实例**的身份预览，
   **用户一旦修改 Target 或 Arguments，不得直接沿用旧 `preview_point_key`**；
   **首选服务端权威 Preview**，前端通用模板替换只能证明**编码规则**一致，
   **不得视为「已由 Driver 验证的新实例身份」**（矩阵 A39/A41）。
3. **C10 的实际路径**已完整给出（§5.2 十步链），终局是
   `drivers/sinumerik-nck/src/lib.rs:648-657` 的 `UNSUPPORTED_BINDING`；
   处置为 **UI 侧收敛 + Descriptor 逐字不动**（六条措施 §5.4），
   并显式声明一个取舍：**PR-C 后向导页无法添加 NCK 语义点位**，归 PR-D。
4. **红线与原样条款**见 §1.6；**唯一允许的例外**（`nck_native`+`derived` 的固定规则）已在
   §1.6 与 §4.3 明确边界，避免被误判违规。
5. **无法确认 / 冲突项**如实列于 §9.3：其中 **`scope` 未校验**与 **`AddDeviceFlow.tsx` 归属**两项
   已**确证并裁决**（前者登记为独立 Driver 缺陷 §10.2，后者写入 §8.3「已裁决」）；
   **前端能否直读仓库级 fixture** 一项**已由本轮实测结案（§2.5/§9.3-1）**：
   按 PR-C 计划形态（fixture 仅被测试文件 import）⇒ **`tsconfig.json` 与 `vite.config.ts` 两个都无需修改**；
   仅当**生产代码** import 时才需要改 `vite.config.ts` **一处**（最小改动与修复前后对照已给出，
   并附「只写 fixtures 会丢掉工作区根」的反事实证据）。
   **第二轮新增裁决**：§9.3-**5**（`identity` 可选字段是否影响非 NCK 浏览叶）与 §9.3-**7**（`~<n>` 生成规则）
   已由评审**裁决结案**（分别落到矩阵 **A44** 与 **A25** 的改写）；
   §9.3-**9**（**`pnpm build` 能否访问仓库外部路径**）为**将来验收要求**、**本轮未验证**（门禁 **G11**），
   并附评审优先方案作为设计约束：「**让 Golden 只在测试中使用，生产身份生成逻辑不要依赖 Fixture 文件**」（§2.5.4）。
6. 前端测试基线 **33 文件 / 383 用例**：本轮**已实际运行**（`pnpm test` 退出码 `0`，
   `Test Files 33 passed (33)` / `Tests 383 passed (383)`；`pnpm typecheck` 退出码 `0`），
   实跑数字与原有两条独立静态计数路径**同值**（§9.1、§9.4）。
7. **第一轮三级修订落点（保留，不改）**：修订 1（I-1 能力边界 + 实例身份收紧 + 两级验收）＝ §1.4.1 / §1.4.2 / §1.7，
   并落到 §1.5、§4.3、§6.1 与矩阵 **A39**；
   修订 2（I-2 与 `scope=identity` 正式登记）＝ 新增 **§10.1 / §10.2**，并落到矩阵 **A40**；
   修订 3（`tsconfig.json` / `vite.config.ts` 改为条件修改项 + 实测结论）＝ §2.2 两行、新增 **§2.5**、§9.3-1、门禁 G2。
8. **第二轮四项修订落点（本轮）**：
   ① **I-2 升级为「完整语义配置」的 P0 功能依赖【已裁决修正】**（推理链 ＋ 禁止硬编码 828D Target 组合 /
   禁止放宽 A40 ＋ 三层实施分层表 ＋ 评审实施建议）＝ 新增 **§1.4.3**，并落到 **§0**、§1.4.1 B、
   §1.4 I-2 行、**§1.7**、§9.3-3、**§10.1**、§12-2；
   ② **Target 元数据降级规则状态表（逐行照录）** ＝ 新增 **§1.4.4**（含评审定位「既满足 A39/A40，
   也不会把前端变成第二个 NCK Compiler」），并在 **§1.5**、**§6.1**、**§7.8.1** 引用，
   矩阵补齐 **A42 / A43**（`§1.4.4` 六行覆盖映射见 §7.8.1，**无遗漏**）；
   ③ **三项验收要求**：**(1)** `BrowseSelection` 非 NCK 回归（批准添加可选 NCK 专属元数据 + 两类回归测试
   「NCK 带 `identity`」/「非 NCK 不带 `identity`」）＝ §2.2 `browseModel.ts` 行、§9.3-5、矩阵 **A44**；
   **(2)** Native **不自动追加 `~<n>`**（允许用户明确指定合法 Key、禁止自动改名、**身份保持原值**、
   冲突时拒绝、**不依赖建议值算法**）＝ **A25 改写**、§6.2、§4.3、§1.5、§9.3-7（改为「已裁决：本阶段不实现」）；
   **(3)** **生产构建验证**（`pnpm build` 登记为将来验收要求）＝ **§2.5.4**、§9.3-9、门禁 **G11**，
   并写入设计约束「**让 Golden 只在测试中使用，生产身份生成逻辑不要依赖 Fixture 文件**」；
   ④ **PR-C / PR-D 文件边界不变（修订 4）**＝ §2.2 注、§5.4.1、§8.2 注、§8.3 注、§12-10，
   **`AddDeviceFlow.tsx` 与 `AddDeviceFlow.test.tsx`（包括对应测试）继续归 PR-D 独占**。
   两条纪律记录：**§9.4**（事故表述改为**不含「已逐字恢复」含义**的准确写法 ＋
   禁止使用「连接真实依赖 Store 的临时副本」执行安装）与 **§11 G0**（正式开发前依赖完整性与构建检查）。
9. **矩阵最终行数：44 行（A1–A44）**——原 38 行 A1–A38 与第一轮 **A39–A41** **全部保留**；
   第二轮新增 **A42 / A43 / A44**（§7.8），并附 **§7.8.1 的「§1.4.4 状态表 → 矩阵行」逐行覆盖映射**。
10. **`AddDeviceFlow.tsx` / `AddDeviceFlow.test.tsx` 的归属冲突已裁决**（§8.3）：
   **两个文件（包括对应测试）均由 PR-D 统一处理，PR-C 不修改**；
   上位设计 `:1207`/`:1223`/`:1264` 的内部冲突标记为「**已裁决**」；
   **本轮定向修订再次确认该边界不变**（修订 4，§8.2 注）。
