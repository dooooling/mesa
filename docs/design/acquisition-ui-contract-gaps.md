# 采集 UI 契约缺口（现状 / 归属 / 拟改契约 / 验收条件）

> 状态：**分析文档**。本批只记录缺口，**不**修改 `proto` / `crates/**` / `drivers/**`，
> 也**不**做 scope 收敛、恢复策略表、B 组契约变更。
>
> 基线：分支 `fix/acquisition-page-ux`，HEAD `8543139`。文中行号以该提交为准。
>
> 目的：把「Web 为了跑通 NCK 而写死/复制的驱动知识」逐条列清，给出归属与可检验的
> 契约改法，避免下一次驱动升级再出现「表单已禁用、保存却放行」这类同源缺陷。

---

## 一、现状：当前有哪些耦合

### G1. `local_tsap` / `remote_tsap` 字段名特化

- **位置**
  - `apps/mesa-web/src/components/DescriptorFields.tsx:234-249`：按 **字段 key 字面量**
    （`local_tsap` / `remote_tsap`）切换到 `TsapField`，其它 `integer` 走普通数字框。
  - `apps/mesa-web/src/connection/connectionSetupModel.ts:296-299`：`isTsapKey()` 用
    正则 `/(^|_)tsap$/i` 判定「允许十六进制」，与字段类型无关。
  - `apps/mesa-web/src/connection/connectionProfiles.ts:40-47`：828D 预设把
    `local_tsap=1024` / `remote_tsap=3332` 写进前端常量。
  - 测试把同一份假设再固化一遍：`apps/mesa-web/src/connection/ConnectionSetupPanel.test.tsx:33-34`。
- **为什么是耦合**：`local_tsap` 是 S7/NCK 协议地址参数名，不是通用 UI 语义。
  另一个驱动如果也叫 `local_tsap` 但语义是十进制、或叫 `tsap_local` 却同样支持
  `0x` 写法，Web 会分别「错误接受」与「错误拒绝」。字段类型 `integer` 本身完全
  无法表达「允许十六进制输入」。
- **风险**：新增/改名字段时，Web 需要改代码才能正确渲染；而 Core 仍按整数校验，
  于是「界面能输、后端能收、语义却不同」。

### G2. `scope` 的前端解释

- **位置**
  - `apps/mesa-web/src/nckCollectability.ts:41-50`：唯一准入判据是白名单字面量
    `scope === "collect"`；`:57-75` 把 `identity` / 缺失 / 其它值分别归为不可采集。
  - `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:222-227`（UI 执行点①：
    过滤可配置变量）与 `:655-676`（UI 执行点②：提交前对将提交点位再查一次）。
  - `apps/mesa-web/src/nckPointModel.ts:696-717`：把服务端 preview 返回的 `scope`
    与目录条目的 `scope` 逐字比对，不一致即拒绝。
- **为什么是耦合**：`scope` 是 **Driver 定义目录**里的字段，但「什么值可采集」
  由 Web 用字符串白名单二次解释。Core/Driver 并不校验 `scope`，所以 Web 的
  白名单实际上成了唯一执行点。
- **风险**：驱动新增 `scope` 取值（例如 `"probe"`）时，Web 的未知值处理是
  fail-closed（正确），但**语义完全由 Web 猜测**；反过来若某驱动把可采集标成
  `"Collect"`（大小写），Web 会静默拒绝，而驱动认为合法。

### G3. 按 `driver_id` 选 binding kind

- **位置**
  - `apps/mesa-web/src/deviceModel.ts:249-255`：`isNckDriver()` 用 `driver_id`
    字符串包含 `"nck"` 判定，`bindingKindForDriver()` 据此在
    `mesa.resources.v1` / `nck.points.v1` 之间二选一。
  - `apps/mesa-web/src/endpointSavePlan.ts:97-109`：`planEndpointTaskSave` 以
    `isNckDriver(input.driverId)` 决定走哪条保存计划。
  - `apps/mesa-web/src/nckTaskBinding.ts:286-291`：非 NCK 驱动写 `nck.points.v1`
    直接拒绝。
  - `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:208`、`:479`、
    `:523`：组件内多处按 `driverId` 分支（加载、文案、任务 kind）。
- **为什么是耦合**：任务 binding kind 是 **驱动能力**，不是驱动名字的属性。
  第二个需要 `nck.points.v1` 的驱动（或 NCK 的另一种命名）必须改 Web 代码。
- **风险**：新增驱动若 id 不含 `nck`，Web 会写出 `mesa.resources.v1`，Driver
  `configure` 直接拒绝；错误信息是「不支持的 binding」，排查方向被误导。

### G4. 质量码 switch

- **位置**
  - `apps/mesa-web/src/qualityCode.ts:92-111`：`classifyCode()` 按 `driver_id`
    分支（`sinumerik-nck` → S7 item 字节；`opcua` → StatusCode；`focas2` → Mesa 分类码）。
  - `apps/mesa-web/src/qualityCode.ts:46-61`：S7 参考码表与 FOCAS2 分类码表硬编码在 Web。
  - `apps/mesa-web/src/qualityCode.ts:234-274`：`samplingBadge()` 再按
    `quality` 字符串（`"BAD"` / `"UNCERTAIN"`）分支决定文案与颜色。
  - `apps/mesa-web/src/qualityCode.ts:120-126`：`shouldShowCodeSection()` 用
    `"GOOD"` 字面量决定是否渲染码区。
- **为什么是耦合**：码表与码空间是**协议知识**（Core 不懂协议），却存在 Web；
  `GOOD/BAD/UNCERTAIN` 是 Core 质量枚举的 wire 字符串，Web 用裸字符串比较。
- **风险**：驱动改码表/新增码空间，Web 不更新就只能显示 `UNKNOWN`；更严重的是
  同一数字在不同驱动语义不同，前端按 `driver_id` 猜错会把「参数错」解释成
  「地址不存在」。

### G5. derived key 的前端副本

- **位置**
  - `apps/mesa-web/src/nckNativeModel.ts:29-52`：`nativeDerivedPointKey()` 在 Web
    重实现 `native.<area>.<area_no>.<module>.<column>.<line>` 规则（文件头
    `:5-6` 明确注明该规则来自 `drivers/sinumerik-nck` 的 `definition.rs:1163-1175`）。
  - `apps/mesa-web/src/nckNativeModel.ts:236-267`：`validateNativePoint()` 用前端
    副本重算 Key 并与记录比对。
  - `apps/mesa-web/src/nckPointModel.ts:33-34`、`:208-247`：`identity_template` /
    `identity_keys` 只用于**推导必填项**与展示，Key 仍以服务端 preview 为准。
  - `apps/mesa-web/src/components/NckPointFields.tsx:485`：`identity_template` 仅展示。
- **为什么是耦合**：Key 生成规则属于 Driver（唯一知道地址语义的一方）。Web 复制
  一份后，两处规则必须永远同步；一旦驱动调整（例如增加 `~n` 后缀规则），Web
  的「重算比对」会把合法点位判成 `NATIVE_KEY_MISMATCH`。
- **风险**：这是典型的「两份真值」：表单能提交、重新打开却校验失败；或反之。

### G6. `variable` 资源 id 特化（本批 P1 直接相关）

- **位置**
  - `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:228-231`：原生字段
    Schema 固定取 `desc.resources.find((r) => r.id === "variable")`。
  - `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:690-710`：最终保存
    守卫构造「本次提交涉及资源」时，同样把 NCK 原生点位硬编码到 `resourceId: "variable"`。
- **为什么是耦合**：`variable` 是 NCK Descriptor 的资源 id；Web 用它当
  「原生地址参数 Schema」的定位键。驱动改名/新增第二个原生资源时，Web 会拿不到
  Schema，或拿错 Schema 做未知类型判定。
- **风险**：守卫与表单可能扫到不同 Schema（一个 `undefined`、一个真实 Schema），
  再次出现「表单禁用、保存放行」的不一致。

---

## 二、归属：每条应该归谁

归属口径：

- **Driver 声明**：静态能力/字段语义，写在 Descriptor / 定义目录里；
- **Driver 返回**：运行时才能确定的权威结果（如 point_key、scope 判定）；
- **Core 通用**：跨驱动、与协议无关的契约字段（Descriptor/Schema/质量枚举）；
- **Web 通用**：纯展示/交互规则，不包含任何协议常量。

| 缺口 | 归属 | 理由 | 现状代价 |
|---|---|---|---|
| G1 `local_tsap`/`remote_tsap` | **Driver 声明**（字段级 UI hint）+ **Core 通用**（Schema 承载 hint） | 「这个整数允许十六进制输入」是字段语义，不是字段名 | Web 按 key 猜；改名即失效 |
| G2 `scope` 解释 | **Driver 声明**（可采集性）+ **Driver 返回**（preview 判定） | `scope` 是驱动定义数据；Web 不应定义白名单 | 新取值只能靠 Web 猜 |
| G3 binding kind | **Driver 声明**（支持的 task binding）+ **Core 通用**（Descriptor 承载） | kind 是驱动能力，不是 id 命名 | 新驱动必须改 Web |
| G4 质量码 | **Driver 声明**（码表/码空间）+ **Core 通用**（枚举与命名空间字段） | 码表是协议知识；`GOOD/BAD` 是 Core 枚举 | 码表分散在 Web，易漂移 |
| G5 derived key | **Driver 返回**（权威 Key）+ **Driver 声明**（identity 规则用于展示） | 只有驱动知道地址语义 | 两份真值，必须手工同步 |
| G6 `variable` 资源 | **Driver 声明**（资源角色标注） | 资源 id 是驱动自定义命名 | 改名即断链，守卫与表单可能不同源 |

---

## 三、拟改契约：具体加/改什么，版本影响

> 统一原则：**只加字段、不改旧字段语义**；Web 对缺失新字段的旧驱动保留现有
> fallback（并标注 `TODO` 移除版本）。因此除特别说明外，均为 **contract_minor**
> 变更（向后兼容）；删除 fallback 才需要 **contract_major**。

| 编号 | 变更 | 类型/默认 | 兼容策略 | 版本影响 |
|---|---|---|---|---|
| C1 | `FieldDescriptor.ui.input`: `"text" \| "decimal" \| "hex"`（或 `radix: 10 \| 16`） | 可选，缺省 `decimal`；未知值 ⇒ Web 视为不支持（fail-closed） | Web 先读新字段，缺失时**保留** key 特化 fallback | minor |
| C2 | 定义目录条目增 `collectable: bool` + `admission: {code, message}`；或 Core 通用 `admission` 结构 | `collectable` 可选；缺失 ⇒ Web 保持现有 `scope` 白名单 | 目录 `format_version` 1 → 2，旧字段保留 | minor（format_version +1） |
| C3 | `DriverDescriptor.task_bindings: [{ kind, schema, modes }]` | 可选；缺失 ⇒ Web 按 `driver_id` fallback | Web 优先声明，fallback 标注废弃 | minor |
| C4 | `DriverDescriptor.quality_codes: [{ namespace, code, class, text }]`；snapshot 增 `code_namespace` | 可选；缺失 ⇒ 现有 `driver_id` 分支 | Core 透传 namespace，Web 不再按 id 猜 | minor（snapshot 加字段） |
| C5 | 原生点位身份改由**服务端 preview** 返回权威 `point_key`；Descriptor 侧只保留 `identity` 声明用于展示 | 新端点/新字段可选 | Web 删除 `nativeDerivedPointKey` 需与 preview 接线同批；旧驱动缺 preview ⇒ 保持只读、禁止保存 | minor（加 preview 能力）；删除副本本身是 Web 内部变更 |
| C6 | `ResourceDescriptor.role: "native_address" \| "point_address" \| ...`（或 Descriptor 顶层 `native_parameters_resource`） | 可选；缺失 ⇒ 现有 `id === "variable"` fallback | Web 优先 role，fallback 标注废弃 | minor |

**不做的事（本批明确排除）**：

- 不收敛 `scope`（C2 只加声明，不删现有白名单）；
- 不引入恢复策略表；
- 不改 B 组契约（Descriptor 之外的传输/IPC 面）。

**待确认**：

- C1 的 hint 放在 `ui` 还是新增 `input_hint` 顶层字段，需要 Core Schema 冻结口径
  确认（`ui` 目前是纯展示提示，放输入格式是否越界 —— 待确认）。
- C4 的 `code_namespace` 由 Core 在 snapshot 上补，还是由 Driver 在 point 更新里
  自带，需要看 IPC 消息是否允许扩展（待确认）。
- C5 的 preview 端点目前只有 NCK 语义点位使用；原生点位复用同一端点还是新增
  `native-preview`，需要驱动侧确认（待确认）。

---

## 四、验收条件：怎样算改完（可检验）

每条都必须能在 CI/单测里机械判定，不靠人工观察。

1. **AC1（G1）**：`apps/mesa-web/src` 中 `local_tsap` / `remote_tsap` 只允许出现在
   测试夹具与文案里；控件选择只依据 `ui.input`（grep 断言 + 一个「字段名不同但
   `ui.input=hex`」的渲染单测）。
2. **AC2（G2）**：定义目录新增任意未知 `scope` 值时，Web 不依赖白名单即可 fail-closed；
   有 `collectable=false` 时即使 `scope="collect"` 也不得进入可配置集合（组件测试
   覆盖「声明优先于 scope」）。
3. **AC3（G3）**：注入 `driverId="custom-driver"` + `task_bindings` 声明
   `nck.points.v1` 的 Descriptor，保存写出正确 kind，且 Web 源码不再新增
   `driver_id` 字符串分支（`endpointSavePlan` 单测 + grep 断言）。
4. **AC4（G4）**：新驱动只声明 `quality_codes` 即可获得正确解释；
   `apps/mesa-web/src/qualityCode.ts` 中不再出现 `"sinumerik-nck"` / `"opcua"` /
   `"focas2"` 字面量（grep 断言 + 码表单测）。
5. **AC5（G5）**：删除 `nativeDerivedPointKey` 后，所有 native 用例改由 preview mock
   提供 `point_key` 仍全绿；Web 不再出现「前端重算 Key 并与记录比对」的逻辑
   （单测断言 `validateNativePoint` 只消费服务端结果）。
6. **AC6（G6）**：`apps/mesa-web/src` 中 `id === "variable"` 仅存在于 fallback 分支，
   并有注释标明移除条件；新增「资源 role 声明」后，表单与保存守卫使用**同一**
   Schema 来源（组件测试：把 variable 资源改名，守卫仍拒绝未知类型）。

---

## 附：与本批修复的关系

- 本批 P1 修的是**守卫同源**：最终保存守卫改为扫「本次提交涉及资源的完整 Schema」
  （`apps/mesa-web/src/descriptorCompatibility.ts` 的 `unsupportedFieldsInResources`），
  与编辑表单同源；G6 是它残留的最后一处「按资源 id 定位 Schema」耦合。
- 本批 P2-1/P2-2 只修「已保存状态」与「草稿显示」，不触碰上述契约。
