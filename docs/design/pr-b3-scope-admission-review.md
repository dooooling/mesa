# PR-B3 审查：Driver `scope=identity` 采集准入（只读 + 最小修复方案）

- 审查基线：`main` = `71d3528bd72c06b175b1933a1147c7fc51a4a729`（I-2 与 PR-C 均已合并）。
- 审查分支：`docs/pr-b3-scope-admission-review`。**只读**：未修改生产代码/测试/配置；未执行 commit/push/checkout/stash/reset/PR/tag，仅按要求创建审查分支；未启动 mesad/Vite，未连接设备，未安装，未运行测试或构建。
- 证据口径：实现给 `文件:行号`（基线工作树静态定位）；无法证实的一律标 **待确认**。
- 相关冻结文档：`docs/design/nck-prb-driver-compiler-boundary.md`（下称 PR-B 边界，§3.5 错误码表）、`docs/design/nck-i2-definition-catalog-api-boundary.md`（下称 I-2 边界，§9.3）。

## 0. 结论速览

1. **最小拒绝点 = `NckConnection::configure` 的逐点循环内、`compile_point` 成功之后**（`drivers/sinumerik-nck/src/lib.rs:709-715` 之间）。不要放进 `compile_semantic_checked`（与 Preview 共用）、`compile_semantic` / `check_identity`（被 `compile_point` 调用，会打断 62 条 wire golden 与 I12）。
2. **覆盖性**：`ConfigureTasks` 是 NCK 唯一能创建采集计划的入口（`crates/driver-sdk/src/lib.rs:1191-1193,1277`），且 NCK 的 `PlanSnapshot` 只有一个构造点（`drivers/sinumerik-nck/src/lib.rs:801`）。native 分支在同一逐点循环内；Core 的 REST Start / 开机重放 / Bootstrap 全部收敛到同一条 `run_config_flow`（`crates/driver-manager/src/endpoint.rs:703-758`）。
3. **新码** `DEFINITION_SCOPE_NOT_COLLECTABLE`，`ErrorKind::Configuration`。不复用 `INVALID_BINDING_CONFIG`（那是形状/类型错），更不复用 `Unsupported` 系（会变成 501，且 Core 的 `config_fail` 会把它当 `Lost` 进入重连循环，`endpoint.rs:1022-1031`）。
4. **零影响**：Catalog（42/6）、Preview（42/42 身份一致性）、Browse（62 叶）、Native、62 条 wire golden、55/5/77 身份 golden、非 NCK 驱动。**必须调整**：`drivers/sinumerik-nck/tests/definition_api.rs` 的 I32（`:686`）与 I45（`:1194`）——它们断言的正是本缺陷（Configure 接受 identity Key），属需求变更而非放宽。
5. **旧数据**：不做自动迁移/清理。修复后旧任务在 Start/重放时 fail-closed：端点 `FAILED`、`detail` 带新码、无重试、不写计划。建议前端加载已有任务时用目录做一次 preflight 提示（后续 PR，本轮不实现）。

## 1. Q1：六条 identity 如何进入 Configure（完整链）

1. 客户端构造 `nck.points.v1`：`PointSelection{kind:"nck_variable", point_key:"axis.name@axis=i:1", parameters:{variable_id:"axis.name", definition_revision:1, target:{axis:1}, arguments:{}}}`。Core **只认识** `kind/point_key/parameters` 三字段，`parameters` 完全不透明（`crates/core-types/src/point.rs:100-122`）。
2. **保存**：`PUT /api/v1/endpoints/{id}/tasks` → `put_tasks_for_endpoint`（`crates/core-api/src/lib.rs:2262`）→ `gate_data_tasks`（调用点 `:2307`，实现 `:242-329`）→ `binding_gate::route_data_task_kinds`（`crates/core-api/src/binding_gate.rs:51-79`）→ `parse_point_binding`（`crates/core-types/src/point.rs:934-946`）。只校验：kind 非空、point_key 通用语法、parameters 是对象；**不读定义目录、不读 scope**。
3. **启动**：`POST /api/v1/endpoints/{id}/start`（`crates/core-api/src/lib.rs:2136-2163`）→ `build_runtime_endpoint`（`crates/core-api/src/runtime.rs:165-178`，重跑同一个 `gate_data_tasks`）→ `manager.start_endpoint`（`crates/driver-manager/src/manager.rs:483-517`，spawn 后立即返回）→ `run_config_flow`（`endpoint.rs:703`）。
4. OpenConnection（`endpoint.rs:716-728`）→ **ConfigureTasks**（`endpoint.rs:748-758`）。
5. `driver-sdk` 的 `on_configure`（`crates/driver-sdk/src/lib.rs:1277-1350`）→ `conn.configure`（`:1307`）。
6. `NckConnection::configure`（`drivers/sinumerik-nck/src/lib.rs:654-808`）：`PointBinding::from_json`（`:702`）→ 逐点 `AddressCompiler::compile_point`（`:711-715`）→ `compile_semantic`（`compiler.rs:393-401`）= `compile_semantic_checked`（`:406-465`，D1–D10 + 身份重算）+ `check_identity`（`:551-588`，只比 point_key 与 canonical_identity）。
7. 成功后才写计划（`lib.rs:801-806`）；`run` 按 `PlanSnapshot.points` 全量读（`:877-910`），**不区分 scope**。

**为什么今天没有任何一层拦住**：

| 层 | 事实 | 证据 |
|---|---|---|
| Core 保存门禁 | 只看信封形状，不建 Point Kind 白名单、不读目录 | `point.rs:105-111`、`:934-946`；`binding_gate.rs:51-79` |
| Core Start / 重放 | 同一条门禁，无 scope 判定 | `runtime.rs:177-178`；`apps/mesad/src/main.rs:149-157` |
| 前端 | 有 `scope` 准入（P0-1/A13/A40），但只是客户端约束，可绕过 | `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:179`；`apps/mesa-web/src/nckCollectability.ts:48-130` |
| 定义装载 | 只校验数据形状/摘要，**没有** scope 分支 | `definition.rs:741+`（`validate_definition` 全文无 `scope`） |
| Address Compiler | 读 `def.addressing` / `identity_template`，**从不读 `def.scope`** | `compiler.rs:406-465`（`compiler.rs` 全文无 `scope`） |
| Configure | 逐点编译后直接入计划 | `lib.rs:709-767` |

## 2. Q2：最小拒绝点与架构裁决

### 2.1 候选位置的爆炸半径

| 位置 | 覆盖 Configure | 影响 Preview（6 条 identity） | 影响 62 wire golden / I12 | 结论 |
|---|---|---|---|---|
| `compile_semantic_checked`（`compiler.rs:406`） | 是 | **破坏**：`preview_point_key` 直接调用它（`:370`） | 破坏 | 否决 |
| `compile_semantic`（`compiler.rs:393`） | 是 | 不破坏 | **破坏**：`compile_point` 调用它，`semantic_equivalence.rs:99-131` 与 I12 都走 `compile_point` | 否决 |
| `check_identity`（`compiler.rs:551`） | 是 | 不破坏（Preview 不走 D11） | **破坏**：同上（`compile_point` 调用） | 否决 |
| `NckConnection::configure`（`lib.rs:709-715` 之后） | 是 | 不破坏 | 不破坏 | **采纳** |

> 关键点：评审“不要把拒绝加进公共 Compiler 路径”的判断成立，而且比“Configure 专属的 `compile_semantic`”还要再退一层——因为 `compile_semantic` 也是 `compile_point` 的公共前半段，62 条 golden 的 6 条 identity 向量与 I12 都经由它。

### 2.2 选定方案（函数名 / 插入位置）

**插入点**：`drivers/sinumerik-nck/src/lib.rs:715` 的 `})?;` 之后、`:716` 的 `let rp = &compiled.resolved;` 之前：

```rust
// PR-B3：Configure 专属准入。只允许此处调用；preview_point_key 不得调用。
compiler.ensure_collectable_scope(&compiled).map_err(|e| {
    SdkDriverError::new(e.kind(), e.code(), format!("task `{}`: {e}", task.id))
})?;
```

**实现位置**：`drivers/sinumerik-nck/src/compiler.rs`，紧随 `check_identity`（`:551-588`）之后、`impl AddressCompiler` 块内（`:589` 之前）：

```rust
/// PR-B3：Configure 专属准入——语义点位的定义 scope 必须是 collect。
/// 只由 `NckConnection::configure` 调用；`compile_point` / `preview_point_key` 永不调用，
/// 否则会分别打断 62 条 wire golden 与 6 条 identity 的权威预览。
pub fn ensure_collectable_scope(&self, compiled: &CompiledPoint) -> Result<(), CompileError> {
    let Some(d) = &compiled.definition else { return Ok(()) }; // nck_native 无定义，不受影响
    let def = self.catalog.lookup(&d.variable_id, d.revision)?; // 编译刚命中过，理论不 miss
    if def.scope != DefinitionScope::Collect {
        return Err(CompileError::DefinitionScopeNotCollectable {
            variable_id: d.variable_id.clone(),
            revision: d.revision,
            scope: def.scope.as_str(),
        });
    }
    Ok(())
}
```

配套：在 `definition.rs:54` 给 `DefinitionScope` 增加 `pub const fn as_str(self) -> &'static str`，并让 `definition_api.rs:667-672` 的私有 `scope_name` 复用它（单一词表来源）。新变体加进 `CompileError`（`compiler.rs:72-158`），`code()`（`:162-180`）与 `kind()`（`:183-194`）各加一个显式分支（不要依赖 `_` 兜底，便于码表审计）。

### 2.3 覆盖全部配置入口的论证

- **唯一计划构造点（NCK）**：`PlanSnapshot` 定义 `lib.rs:438`、字段 `:451`、构造 `:801`、读取 `:841`；在 `drivers/sinumerik-nck` 内 `git grep PlanSnapshot` 只有这四处，构造只有 `:801`。
- **NCK 唯一生产 configure 调用点**：`driver-sdk:1307`；`ConfigureTasks` 的唯一 handler 是 `on_configure`（`:1191-1193,1277`）。其它驱动的 `.configure(` 调用与 NCK 无关。
- **native 分支**：与语义点位在**同一个逐点循环**内按 Point Kind 路由（`compiler.rs:341-347`），因此同一次准入检查自然覆盖 `nck_native`；其 `definition=None`（`:497-505`）⇒ 直接放行。
- **批量提交**：Core 把全部 Task 装进一次 `ConfigureTasks`（`endpoint.rs:748-753`），循环覆盖每个 task 的每个 point；任一点失败即 `return Err`。
- **Start / 重放 / Bootstrap**：REST Start（`core-api/src/lib.rs:2149-2153`）、开机恢复（`apps/mesad/src/main.rs:149-157`）都走 `build_runtime_endpoint` → `start_endpoint` → `run_config_flow`；不存在第二条直连 `configure` 的路径。
- **`apply_point_map` 不能加点**：只校验并保存映射（`lib.rs:810-829`）；`run` 只消费计划（`:841-853`）。

### 2.4 为什么不会误伤 Preview

Preview 的路径是 `preview_point_key`（`compiler.rs:364-386`）→ `compile_semantic_checked` / `compile_native_checked`，**从不调用** `compile_semantic`、`check_identity` 或 `ensure_collectable_scope`。修复后 `ensure_collectable_scope` 的生产调用点应恰为 1 处（`lib.rs` 的 configure 循环内），可直接 grep 复核。因此 6 条 identity 的预览仍返回 200 + `scope:"identity"` + 可算 `point_key`（`definition_api.rs:619-647`）。

## 3. Q3：错误码与 HTTP / SDK 映射

| 项 | 值 |
|---|---|
| 精确码 | `DEFINITION_SCOPE_NOT_COLLECTABLE` |
| `ErrorKind` | `Configuration` |
| 产生点 | `CompileError::DefinitionScopeNotCollectable`（`compiler.rs` 新变体），仅 `ensure_collectable_scope` 返回 |
| SDK 表现 | `SdkDriverError{kind, code, message}`（`driver-sdk:59-77`）；`Display` = `ConfigurationError/DEFINITION_SCOPE_NOT_COLLECTABLE: ...`（`:79-87`） |
| Core 终态 | `config_fail` 判 `kind=="ConfigurationError"` → `ConfigurationFailed`（`endpoint.rs:1022-1031`）→ `set_status(Failed, detail)`（`:320-335`、`:389-407`） |
| HTTP | **ConfigureTasks 不是 HTTP 路由**：`POST /start` 先返回 200 `{"started":...}`（`core-api/src/lib.rs:2153-2157`；`manager.rs:503`），拒绝经 `GET /api/v1/endpoints/{id}/state` 暴露（`:1508-1516`）。若将来有同步校验路由，`error_kind_to_status(Configuration)=400`（`driver_query.rs:182`） |
| 重试 | **不重试**（配置终态）。若误用非 `Configuration` kind，`config_fail` 会走 `Lost` ⇒ 每次重连都重复 configure 失败（`endpoint.rs:1026-1029`） |

**为何不复用既有码**：

- `INVALID_BINDING_CONFIG`（PR-B §3.5 A12）：语义是 `parameters` 形状/类型错。复用会让客户端无法区分“参数写错”与“定义不可采集”，且 I-2 边界 §9.3-A 已明确本缺陷“需要一个新的「不可采集」码”（`:1483`）。
- `UNSUPPORTED_POINT_KIND` / `MODE_NOT_SUPPORTED`（`Unsupported`）：语义不符；Core 侧会因非 `ConfigurationError` 进入 `Lost` 重连循环，HTTP 语义也会误导为“升级驱动（501）”。
- `POINT_KEY_MISMATCH`：Key 本身是正确重算值，误报会掩盖真因。
- `UNSUPPORTED_TARGET`：`target` 合法，不成立。

**为何不破坏“Core 只按 `ErrorKind` 唯一映射”**：一个 code 在 `CompileError::code()` 只有一个字面量、在 `kind()` 只有一个分支（`compiler.rs:160-194`）；不新增 `ErrorKind` 变体（`crates/core-types/src/lib.rs:622-634`），`error_kind_to_status`（`driver_query.rs:179-190`）与其冻结测试（`:279-300`）逐字不变；Core 仍不出现任何码字面量（P9 护栏 `resource_contract.rs:389`）。

> 需由设计所有者把新码登记进 PR-B 边界 §3.5 B 表（B8）并说明“仅 configure 可达”；本报告**不修改**冻结文档。

## 4. Q4：影响面逐项结论

| 面 | 结论 | 证据 |
|---|---|---|
| Definition Catalog | **不变**：42 条，`identity` 恰 6 | `definition_api.rs:163`、`:548-557`；I44 测试 `:432-467` |
| Identity Preview | **不变**：6 条仍算出 `point_key` 并回显 `scope:"identity"` | `definition_api.rs:619-647`；`:1120`、`:1194-1215` |
| Browse | **不变**：`nck://presets` 仍 62 叶，identity 树独立 | `lib.rs:991-1015`；`discovery_contract.rs:253-293` |
| Native | **不变**：`definition=None` ⇒ 准入直接放行；derived / user 均不受影响 | `compiler.rs:497-505`、`:572-586`；`lib.rs:709-715` |
| 36 条 `scope=collect` | **不变**：`scope==Collect` 放行 | `r1.json` 36 条 collect；`definition_api.rs:425` |
| **62 条 wire golden** | **零影响** | `legacy_equivalence.rs:41-107` 直调原生解析；`semantic_equivalence.rs:99-131` 走 `compile_point`；新检查只在 configure |
| **55/5/77 身份 golden** | **零影响** | `compiler_identity.rs:1032-1064` 只测 `encode_typed_value`/`validate_point_key`；fixture 未改（本轮实测 55/5/77） |
| **42/42 Preview 一致性** | I12 零影响；I32/I45 必须调整（§5.2） | `definition_api.rs:653-678` / `:686-716` / `:1194-1224` |
| 非 NCK 驱动 | **零影响**（改动仅在 `drivers/sinumerik-nck`） | NCK 的唯一生产 configure 调用点 `driver-sdk:1307` |

**42/42 的精确拆解**（评审最关心的点）：

- **I12 `preview_equals_configure_recomputed_identity_for_all_42`**（`:653-678`）：`preview_point_key` + `compile_point`，两者都不变 ⇒ **仍 42/42 绿**。但其“用预览 Key 走 Configure 的同一编译入口”的注释对 6 条 identity 已不再代表 Configure 准入，建议顺手改注释（非必须、不影响通过）。
- **I32 `preview_identity_equals_subsequently_configured_identity`**（`:686-716`）：真实 `configure` 逐条 42 次并断言接受 ⇒ 6 条 identity 必然失败。**这不是“被新检查破坏”，而是它固化了缺陷**：必须改写为 36 接受 + 6 拒绝，`checked` 仍为 42。
- **I45 `preview_scope_semantics_match_configure_for_identity_scope`**（`:1194-1224`）：`:1216` 注释明写“I-2 没有把 scope 校验下沉到 Configure：同一 Key 必须被接受” ⇒ 与本次需求直接冲突，必须反转；其 Preview 半边（`:1205-1215`）逐字保留。

## 5. Q5：测试新增与调整

### 5.1 必须新增

| 文件 | 建议用例名 | 核心断言 |
|---|---|---|
| `drivers/sinumerik-nck/tests/definition_api.rs`（或新 `scope_admission.rs`） | `configure_rejects_all_six_identity_definitions` | 6 条逐条：`code=="DEFINITION_SCOPE_NOT_COLLECTABLE"`、`kind=="ConfigurationError"`、消息含 `variable_id` + `scope=identity` |
| 同上 | `configure_rejects_mixed_batch_without_partial_plan` | 合法 collect + native + identity（identity 放最后）⇒ 整体 `Err`；`conn.plan.read().unwrap().is_none()`；随后纯合法批次成功（参照 `lib.rs:1551-1573`） |
| 同上 | `configure_accepts_all_thirty_six_collect_definitions` | 36 条逐条成功、`descriptors.len()==1`（防退化） |
| 同上 | `configure_accepts_native_derived_and_user_alongside_collect` | native derived + native user + 1 条 collect 混合成功；user Key 仍走 `native.` 命名空间 |
| 同上 | `identity_preview_still_succeeds_for_all_six` | 6 条预览 200、`scope=="identity"`、Key 非空（Preview 不回退） |
| `tests/driver-contract/tests/management_api.rs` | `nck_configure_rejects_identity_scope_at_start` | 保存 identity 任务 200（Core 门禁不变）→ Start 200 → 轮询 `/state` 至 `FAILED`，`detail` 含新码；**不需要真机**（configure 在建连前失败，`open_connection` 只解析配置，`lib.rs:355-378`） |

### 5.2 确实需要调整（并说明为何不是放宽）

| 用例 | 旧断言 | 新断言 | 为什么不是放宽 |
|---|---|---|---|
| I32 `definition_api.rs:686` | 42/42 全部被 Configure 接受 | 36 collect 接受 + 6 identity 拒绝（code/kind 精确）；覆盖数仍 42 | 断言**更严**：新增“必须拒绝”方向；覆盖数不变 |
| I45 `definition_api.rs:1194` | identity Key 被 Configure 接受（`:1216`） | Preview 仍 200 / `scope=identity` / 非空 Key；Configure 必须拒绝 | 需求变更：旧断言固化缺陷；Preview 侧断言逐字保留 |
| 注释 `definition_api.rs:20-22`、`:1216` | “准入判定归前端 / 未下沉到 Configure” | 改为“目录与预览如实；Configure 准入拒绝” | 事实更新，非测试放宽 |

**不需要调整**：`semantic_equivalence`、`legacy_equivalence`、`compiler_identity`、`identity_browse`、`drive_access` 均不调用 `configure`；全仓只有 I32 与 I45 用 identity 定义走 Configure。

## 6. Q6：旧任务已保存 identity 点位的恢复问题

- **现状**：DB 中可能已有 identity 点位（V1 实测 API 接受）。Core 的保存门禁不会拦（`gate_data_tasks` 无 scope），所以“保存成功”与“能启动”从此不再等价。
- **修复后 Start/重放**：`configure` 返回 `ConfigurationError/DEFINITION_SCOPE_NOT_COLLECTABLE` → `ConfigurationFailed` → 端点 `FAILED`、`detail` 带码、**无计划、无数据、不重试**（`endpoint.rs:320-335`）。开机恢复会再试一次并同样失败（`desired_running` 在 Start 请求时已置 true，`core-api/src/lib.rs:2155`），不是热循环。
- **混合任务的爆炸半径**：同一 endpoint 上原本正常的 collect 点也会一起停止——这是“整体拒绝”的预期代价，必须在错误消息里点名 `task id + variable_id + revision`。
- **建议**：**不做自动迁移/删除**（静默删除用户选择会掩盖问题，也违背“不得静默”的既有纪律）；运维路径为 Stop → 编辑 tasks 删除 identity 点位 → Start；错误消息给出“改用 `nck_native` 并显式确认身份”的提示。前端加载已有任务时可用目录做 preflight 标记（后续 PR，不在本轮）。
- **不建议** CLI 直改 SQLite（违反 AGENTS.md 的管理边界）；也不需要清理历史已采集数据。
- **待确认**：是否在 `ConfigurationFailed` 时把 `desired_running` 置 false，避免每次 mesad 重启重复失败——这是行为变更，需单独评审。

## 7. 失败原子性

- **现状已具备**：`new_points` / `new_tasks` / `visible_keys` 全是局部变量（`lib.rs:673-676`），所有校验失败都在 `:801` 之前 `return`；唯一计划写入是整体替换 `*self.plan.write().unwrap() = Some(...)`（`:801-806`）。既有测试已证明失败不留计划（`lib.rs:1564-1567`、`:1754`）。
- **新检查插入后仍保持**：检查在 `:715` 之后、`:716` 之前，仍在 `:801` 之前，且是纯读目录、无副作用。
- **Core 侧同样原子**：只有拿到 descriptors 才分配 `point_id`（`endpoint.rs:776-779`）；失败批次没有 descriptors ⇒ 无 ID、无映射、无数据。
- **注意**：若此前已有成功计划，失败的 reconfigure 不会清空旧计划（未走到 `:801`）。但 Core 的流程是 Stop → Configure → Start，失败即不 Start（`endpoint.rs:703-758`），不会用旧计划启动新 revision——这是“保留上次成功快照”，不是“部分进入”。

## 8. 回归矩阵

| # | 场景 | 期望 | 验证方式 |
|---|---|---|---|
| 1 | 36 条 collect 逐条 Configure | 全部成功，descriptor 逐字不变 | 新增用例 + 既有 `semantic_equivalence` 62/62 |
| 2 | 6 条 identity 逐条 Configure | 全部拒绝，码/kind 精确 | 新增用例 |
| 3 | 混合批次（collect + native + identity） | 整体拒绝，无计划、无 descriptor | 新增用例；`plan.is_none()` |
| 4 | native derived / user | 不受影响，仍成功 | 新增用例 + `compiler_identity` 既有用例 |
| 5 | Preview 42/42 | 42 条全部返回权威 Key；6 条 scope=identity 如实回显 | I12 原样绿；I45 Preview 半边 |
| 6 | 62 条 wire golden | 逐字节不变 | `legacy_equivalence` + `semantic_equivalence` |
| 7 | 55/5/77 身份 golden | 逐条不变 | `compiler_identity.rs:1032` |
| 8 | Browse | 62 叶不变，identity 树不变 | `discovery_contract.rs:253-293`；`identity_browse` |
| 9 | 端到端 Start（identity 任务） | 端点 `FAILED`、detail 含新码、无重试 | 新增契约用例 |
| 10 | 非 NCK 驱动 | 零影响 | 既有 contract suites（29/29 基线） |

## 9. 风险与副作用

- **旧 API 客户端**：以前能配 identity 点位的脚本/客户端会在 Start 硬失败（预期变更，需发布说明）。前端已有 scope 准入，正常 UI 路径不受影响。
- **Native 逃生通道**：`nck_native` 仍可读同一物理地址（本需求明确“不误伤 native”）。准入是**语义**的（定义 scope），不是物理地址白名单；这是设计取舍，不是漏洞。
- **错误优先级**：检查放在 PDU 检查（`lib.rs:724-747`）之前，因此“identity + 装不下 PDU”的点位会先报 scope。无既有测试依赖该优先级；若要求保留旧优先级，可把检查移到 PDU 之后（**待确认**）。
- **未来 scope 变体**：判定用 `!= Collect` 而非 `== Identity`，对未知 scope fail-closed；若将来新增“可采集”的 scope，必须显式更新。
- **未来新增计划入口**：`ensure_collectable_scope` 的覆盖依赖“所有计划都从 configure 构建”。当前由唯一构造点 `lib.rs:801` 保证；将来新增入口必须调用它（代码注释 + 评审清单约束，无法用类型系统强制）。
- **码串选择**：`DEFINITION_SCOPE_NOT_COLLECTABLE` 刻意区别于前端自定义码 `SCOPE_NOT_COLLECTABLE` / `SCOPE_IDENTITY`（`nck-prc-start-review.md:228` 明确“前端自定义码，不冒充后端码”）。若产品要求前后端同串，需另行裁决（**待确认**）。
- 每点多一次目录查找（`BTreeMap`），开销可忽略。

## 10. 与冻结设计文档的关系

- I-2 边界 §9.3-A 曾拒绝“I-2 顺带在 Configure 按 scope 拒绝”（`:1483`），理由是：① 改变 Configure 既有验收语义；② 扩错误码表；③ PR-C 把权威修复点定为 Browse/目录层；④ 与元数据 PR 耦合。那是 **I-2 的范围裁决，不是正确性裁决**。本轮 PR-B3 是独立工作项，且明确要求 Catalog/Browse 保持不变 ⇒ ③ 被新需求取代（Configure 是唯一剩下的强制点）；①/② 是已接受的成本，分别由 §5.2 的测试调整与 PR-B §3.5 新增 B8 行处理。
- 本报告不修改任何冻结设计文件；`docs/design/` 下仅新增本文件。

## 11. 交付边界

- 本轮只新增本文件；生产代码、测试、配置、冻结文档均未改；未执行 commit/push/checkout/stash/reset/PR/tag（仅创建审查分支）。
- 所有结论基于 `71d3528` 的静态定位；未运行测试/构建/服务，未连接设备。涉及“修复后行为”的部分均为方案推演，实施时需以真实测试与契约套件复核。

## 附：旧计划隔离约束（实施后确认）

**已确认的硬约束**（后续开发必须遵守，属生命周期重构时不可破坏的不变量）：

> Configure 失败后，任何生产路径都不得使用该失败实例的旧 Plan 启动采集。

**实现状态**：

- **驱动层**：`on_configure` 失败**不清空**旧 `plan`，这是**既有 SDK 契约**——`tests/driver-contract/tests/session_lifecycle.rs` 的 `failed_reconfigure_keeps_previous_snapshot` 明确要求失败 reconfigure 后旧快照仍可 Apply/Start。本轮**不修改**该契约（修改会波及所有驱动，风险远大于收益）。
- **隔离由 Core 生命周期保证**：`crates/driver-manager/src/endpoint.rs` 在收到 Configure 的 `DriverError` 后立即返回 `ConfigurationFailed`（早于 ApplyPointMap 与 StartConnection）；随后 `teardown_attempt` **终止该次尝试的驱动进程**；端点置 `FAILED` 后直接返回，**不进入重连**。每个 attempt 都是**新进程**，且 Configure **只调用一次**。
- **结论**：当前**无生产路径误用**旧快照；上述约束为后续生命周期重构必须保持的不变量。

**验证证据**：

- 单测 `reconfigure_failure_preserves_last_plan_snapshot_without_partial_commit`：旧计划 revision=1 的点位保留，失败 configure(revision=2) 不写入任何点位；
- 契约测试 `nck_configure_rejects_identity_scope_at_start`：端点 `FAILED`、detail 含 `DEFINITION_SCOPE_NOT_COLLECTABLE`、等待后仍 `FAILED`、`mgr.is_running=false`、`desired_running=true`、`runtime.points=0`、无 latest 数据。

**另记两项实施限制**：PDU 容量优先级无专用用例（identity 定义的 `count` 固定、用户不可改，无法构造该场景），仅由代码顺序证明（`lib.rs:722` 早于 `:733`）；未知 scope 的兜底属未来保护——运行时 `DefinitionScope` 只有 `Collect`/`Identity` 两值，未知 JSON 值在目录装载期即失败，`!= Collect` 的写法是为将来新增 scope 兜底。
