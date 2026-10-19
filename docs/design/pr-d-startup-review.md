# PR-D 实施启动审查：新建设备向导（`AddDeviceFlow`）NCK 语义点位接线

- 审查基线：`main` = `3cd33b1583e7773a2c3a3bf3947207ddd9525ad8`（I-2、PR-C、PR-B3 均已合并）。
- 审查性质：**只读**。未修改生产代码/测试/配置；未执行任何 git 写操作（含建分支、commit、push、checkout、stash、reset、PR、tag）；未启动 mesad/Vite，未连接设备，未安装，未运行测试或构建。
- 分支状态：审查时 HEAD 在 `main`，`docs/pr-d-startup-review` 尚不存在；因“禁止 git 写操作”的硬约束，本会话**未创建分支**，本文件落在工作区，待上级建分支后提交。
- 判据来源：`docs/design/nck-prc-frontend-semantic-config-boundary.md`（下称“边界文档”，A1–A44）、`docs/design/nck-prc-acceptance-review.md`（下称“收口审查”）、`docs/design/nck-prc-implementation-checklist.md`（下称“实施清单”）、`docs/design/nck-prc-start-review.md`（下称“启动审查”）。
- 证据口径：实现给 `文件:行号`（基线工作树静态定位）；无法证实的一律标 **待确认**。

## 0. 结论速览

1. **根因不是“少一个按钮”，而是三处缺口叠加**：① 向导未向 `ResourcePickerAntd` 传 `semantic`（`AddDeviceFlow.tsx:254-257`），因此第③步仍是旧通用 Picker；② 向导从不加载 I-2 目录、也不持有 `nckPoints`；③ 提交时 `bootstrap.ts` 只装配 `mesa.resources.v1`，NCK 被 fail-closed 拒绝（`bootstrap.ts:139-155`）。三者任一单独修都不够。
2. **family 与 catalog 都能在“Endpoint 尚未创建”时取得**：目录与身份预览都是 **driver 作用域**接口，Manager 用临时驱动进程执行且**不发 `OpenConnection`**（`crates/core-api/src/driver_query.rs:13-15`；`crates/driver-manager/src/manager.rs:730-753,786-805`）；`family` 直接取连接步已归一化的草稿 `connection.connection.family`（`ConnectionSetupPanel.tsx:378-384`），它与随后落库的值同源。
3. **保存不必等 Endpoint 创建**：`POST /api/v1/device-bootstrap` 在**单事务**内落 Device + Endpoint + Tasks（`crates/core-api/src/bootstrap.rs:300-313`），且 Core 门禁已接受 `nck.points.v1`（`crates/core-api/src/binding_gate.rs:51-80`）。语义点位在向导内即可完成权威预览，暂存在 `AcquisitionDraft` 里，随 bootstrap 一次提交。
4. **向导今天只有一条写路径**（`POST /device-bootstrap`，无 `PUT /tasks`），不存在第二条 HTTP 保存；需要消除的是 `bootstrap.ts:156-162` 的**手工任务装配**与“NCK 点位不进保存计划”的旁路风险，不是再写一套保存。
5. **PR-D 的验收 = A8/A21/A22 的“向导半边” + 端到端全链**；真实端到端（Driver 接受 + 回读一致）需要 mesad + 软件 emulator，**须单独授权**，mock 全链测试不能替代。
6. **接线面应严格限定**：`AddDeviceFlow.tsx`（+测试）、`addDevice/bootstrap.ts`（+测试）；PR-C 已交付的模型/组件文件**只调用、不修改**。

## 1. 现状：向导为什么今天配不了 NCK 语义点位

### 1.1 完整链路（今天）

| 步 | 事实 | 证据 |
|---|---|---|
| ① 设备 | 只收集 `deviceId/deviceName`，不涉及 NCK | `AddDeviceFlow.tsx:62-69` |
| ② 连接 | 面板按 Descriptor 物化连接；NCK 的 `family` 是**必填 Enum**（选项 `DECLARED_FAMILIES`）；提交前 `submitSnapshot()` 现场归一化 | `drivers/sinumerik-nck/src/lib.rs:117-125`；`ConnectionSetupPanel.tsx:370-384` |
| ② 落草稿 | 草稿保存 `endpointId`（用户填写或 `suggestEndpointId`）、`endpointName`、canonical `connection`（含 `family`） | `AddDeviceFlow.tsx:92-105` |
| ③ 数据 | 渲染 `ResourcePickerAntd`，**未传 `semantic`、未传 `endpointId`**；`resources` 是 Descriptor 的 `variable` 资源 | `AddDeviceFlow.tsx:254-257` |
| ③ 旧 Picker | 无 `semantic` ⇒ 走旧 `manualPane`：按整份 12 原生字段 schema 判必填，`point_key` 由 `resource.output` 自动生成 | `ResourcePickerAntd.tsx:141-153,158-161,193` |
| ③ 可选性 | NCK 的 `variable` 资源有 **9 个 required**（其中 2 个有默认），用户必须填 `area/area_no/module/column/line/...` 才能“加入” | `drivers/sinumerik-nck/src/lib.rs:186-338` |
| ③ Browse | `resource_selection_methods = [Manual, Browse]`，但向导不传 `endpointId` ⇒ 浏览页禁用；向导文案也自认“暂只支持手动添加” | `drivers/sinumerik-nck/src/lib.rs:340-343`；`ResourcePickerAntd.tsx:203-218`；`AddDeviceFlow.tsx:290-292` |
| ④ 提交 | `bootstrapDevice` 只装配 `{type:"resources", selections}`；`driverId=sinumerik-nck` ⇒ `assembleBinding` 返回 `DRIVER_BINDING_KIND_MISMATCH` | `bootstrap.ts:139-155`；`deviceModel.ts:260-289` |
| ④ 结果 | 返回 `failedStep:"put-tasks"`，**连 POST 都不发**（fail-closed，数据安全但功能不可用） | `bootstrap.ts:144-155` |

**结论**：即使用户忍着填完 9 个原生字段，向导也**必然**在提交前被装配层拒绝；NCK 上“语义点位可配置”的路径在向导内根本不存在。

### 1.2 三个缺口的精确定位

| # | 缺口 | 证据 | 后果 |
|---|---|---|---|
| G1 | 未传 `semantic` | `AddDeviceFlow.tsx:254-257` vs `ResourcePickerAntd.tsx:111-112,141-150` | 旧 `variable` 手动页成为默认；A21/A22 在向导内不成立 |
| G2 | 未加载目录、无 `definitions` | 向导全文件无 `fetchNckCatalog` 引用 | `NckPointFields` 无法渲染（`ResourcePickerAntd.tsx:366-371` 会 fail-closed） |
| G3 | 草稿无 `nckPoints`、提交只认 `selections` | `bootstrap.ts:22-34,139-163` | NCK 点位无处暂存、无法进入任务装配 |

### 1.3 “语义叶回填告警放弃”是否仍在？

- **在，但今天不可达**：只有 `semantic` 缺省且 Browse 可用时才会触发。`browseModel` 把语义叶映射为 `resource_id = "nck_variable"`（`browseModel.ts:94-102`），而 Descriptor 的资源 id 是 `"variable"`，于是 `ResourcePickerAntd.tsx:127-139` 命中 `message.warning("浏览节点指向未知资源 nck_variable，无法回填")` 后**丢弃**该叶。
- **PR-D 的陷阱**：如果只给向导补 `endpointId`（草稿里已有计划 id）而不补 `semantic`，浏览页会“看起来可用”，语义叶却会被上面这条告警静默吞掉；而且该 Endpoint 尚未创建，`/api/v1/endpoints/{id}/browse` 只会 404。**必须补的是 `semantic`，不是 `endpointId`。**

### 1.4 向导是否有自己的保存路径？

- **HTTP 层：没有。** 全流程唯一写请求是 `POST /api/v1/device-bootstrap`（`AddDeviceFlow.tsx:142-156`；测试断言 `writes == ["/api/v1/device-bootstrap"]`，`AddDeviceFlow.test.tsx:494-496`）；不存在 `PUT /api/v1/tasks/*`。
- **模型层：有“第二套任务装配”。** `bootstrap.ts:156-162` 手工拼 `{id:"t1", schedule, binding}`，与端点页的 `planEndpointTaskSave`（`endpointSavePlan.ts:97-196`）是两条构造路径；它虽把 kind 交给 `assembleBinding`（A28 已收敛），但对 NCK **只能拒绝**，无法承担“点位 → 任务”的职责。
- 因此 PR-D 要消除的是**这条手工装配在 NCK 上的存在**，并保证 NCK 点位必须经过保存计划（含 scope 准入），而不是新增一条 HTTP 保存。

## 2. 最小接线方案

### 2.1 需要给 `AddDeviceFlow` 传的 `semantic` 配置

`PickerSemanticConfig` 的字段见 `ResourcePickerAntd.tsx:45-55`，逐项来源如下：

| 字段 | 向导阶段取值 | 依据 |
|---|---|---|
| `driverId` | `connection.driverId`（`"sinumerik-nck"`） | 已在连接草稿中（`AddDeviceFlow.tsx:99-104`） |
| `definitions` | `fetchNckCatalog({driverId})` → `indexNckCatalogDefinitions(...).byVariableId` 的 `Object.values` | `nckCatalog.ts:470-551,565-578`；目录是 driver 作用域，无需 Endpoint |
| `endpointFamily` | `String(connection.connection.family ?? "")`（空则 fail-closed） | 连接步 canonical 草稿；`checkNckFamily` 只认非空字符串（`nckPointModel.ts:541-580`） |
| `catalogFamily` | `catalog.family`（多系列为 `null`，**不是拒绝理由**） | 同上第 4 条；`nckCatalog.ts:440-450` |
| `nativeSchema` | `desc.resources.find(r => r.id === "variable")?.parameters ?? null` | 与端点页同一取法（`EndpointAcquisitionPane.tsx:184-188`） |
| `takenPointKeys` | 当前 `nckPoints.map(p => p.point_key)`（新 Endpoint 无 preserved 任务） | 端点页对应实现 `EndpointAcquisitionPane.tsx:520-528` |
| `onAddPoint` | 校验 `point_key` 未占用后 `setNckPoints([...])`，返回 `true` | 复用端点页 `addPoint` 语义（`:530-538`） |
| `preview` | **不传**（生产走服务端 `identity-preview`） | `NckPointFields.tsx:55-56` |

同时：

- **不要传 `endpointId`**。Endpoint 尚不存在，Browse 必然 404；语义页本身不需要它（`ResourcePickerAntd.tsx:397-399` 会显示“浏览需要已创建的 Endpoint。”，这是诚实文案）。
- **不要传 `resources/existingSelections/onAdd` 给 NCK 分支**：语义模式在 `ResourcePickerAntd.tsx:141-150` 提前返回，旧 manual 页整体不渲染（A22），旧 `onAdd` 永不被调用。非 NCK 分支保持现状逐字不变。

### 2.2 family / catalog 的取得时机（本 PR 最大难点）

| 数据 | 何时可得 | 为什么可行（关键证据） |
|---|---|---|
| `driverId` | 连接步完成即得 | 面板提交回调携带 `driverId`（`ConnectionSetupPanel.tsx:380-384`） |
| `family` | 连接步完成即得 | 面板已做 canonical 归一化；NCK Descriptor 强制必填，缺省即无法前进（`drivers/sinumerik-nck/src/lib.rs:117-125`）。该值就是随后落库的 `connection.family`，**同源**，不存在“向导猜一个、后端存另一个” |
| 目录 | 第③步挂载即可拉取 | `GET /api/v1/drivers/{id}/definitions` 是 **driver 作用域**：不建 Endpoint、不开 CNC 连接（`driver_query.rs:13-15`）；Manager 用临时驱动进程 + 单 RPC，**不发 `OpenConnection`**（`manager.rs:730-734,786-805`） |
| 权威 Preview | 第③步即可调用 | `POST /api/v1/drivers/{id}/definitions/identity-preview` 同为 driver 作用域，`POST` 仅表示“带参只读计算”（`driver_query.rs:120-127`；`manager.rs:744-753`） |
| `nativeSchema` | 第②步结束即有 | `onDescriptorChange` 已把 Descriptor 交给向导（`AddDeviceFlow.tsx:225`），资源 `variable` 在 `lib.rs:186-338` |

**结论**：所谓“Endpoint 未创建 ⇒ family/catalog 拿不到”是伪难点。真正需要 Endpoint 的只有 **Browse**（`/endpoints/{id}/browse`）与 **PUT /tasks**，二者在 PR-D 中都不使用。

### 2.3 保存是否必须等 Endpoint 创建？

**不必。** 理由链：

1. bootstrap 请求体直接携带 `acquisition.tasks`，与 Device/Endpoint 在**同一事务**落库（`bootstrap.rs:300-313`）；Endpoint 不存在不构成障碍。
2. Core 门禁对 `nck.points.v1` 只做信封层校验与跨 kind 唯一性（`binding_gate.rs:51-80,107-155`），**不看 scope**（scope 是前端职责，见 `nckCollectability.ts:1-25`），因此前端必须自己跑保存计划。
3. 因此向导内的正确做法是：**点位暂存在 `AcquisitionDraft.nckPoints`，提交前调用一次 `planEndpointTaskSave`，把返回的 `plan.tasks` 交给 bootstrap**；不新建任何“向导专用保存函数”。

**“计划中的 Endpoint”快照语义**（必须在实现里写死并测试）：调用 `planEndpointTaskSave` 时传

```ts
{
  driverId, tasks: [], snapshotState: "ready",
  loadedEndpointId: connection.endpointId, currentEndpointId: connection.endpointId,
  intervalMs, nckPoints, nckDefinitions: definitions,
}
```

- 合法性：Endpoint 尚不存在 ⇒ 其服务端任务集**可证为空**，不存在“用空快照覆盖已有任务”的风险；bootstrap 是原子创建，任务集就是本次提交的这一份。
- 边界：该调用**只允许**出现在“新建设备”路径；任何“已有 Endpoint”场景必须用真实快照（`endpointSavePlan.ts:98-107` 的门保持原样）。
- 若实现时觉得这层语义需要显式命名，**也不要改 `planEndpointTaskSave` 的签名/语义**（属 PR-C 已验收文件）；在调用点用注释 + 测试固定即可。

### 2.4 复用清单（逐项，禁止复制）

| 能力 | 复用的既有导出 | 落点 |
|---|---|---|
| 目录分页/错误映射 | `fetchNckCatalog`、`indexNckCatalogDefinitions`（`nckCatalog.ts:470,565`） | 向导第③步加载 |
| 权威身份 | `previewNckPoint`（`nckCatalog.ts:597`，经 `NckPointFields` 默认实现） | 语义表单内部 |
| 语义表单 + 原生高级 | `NckPointFields` / `NckNativeFields`（经 `ResourcePickerAntd` 语义模式） | 第③步渲染 |
| scope 准入 | `collectableDefinitions` / `evaluateNckSelectionAdd` / `validateNckPointSubmission`（`nckCollectability.ts:106,123,181`） | 表单内 + 保存计划内（双执行） |
| family 校验 | `checkNckFamily`（`nckPointModel.ts:541`） | 表单内 |
| 保存计划 | `planEndpointTaskSave`（`endpointSavePlan.ts:97`） | `bootstrap.ts` 的 NCK 分支 |
| 任务装配 | `assembleBinding`（`deviceModel.ts:260`，由保存计划内部调用） | 不直接调用 |
| 任务形状/唯一性 | `mergeNckAcquisitionTasks` / `collectEndpointPointKeys`（`nckTaskBinding.ts:478,510`） | 由保存计划内部调用 |
| 零采集点 | `tasks=[]` 语义（`bootstrap.ts:131-138,176`） | 保持不变 |

### 2.5 建议改动面（最小）

1. `apps/mesa-web/src/addDevice/AddDeviceFlow.tsx`
   - 新增状态：`catalog/definitions`、`nckPoints`；第③步按 `isNckDriver` 分支：NCK 拉目录并渲染 `semantic`，非 NCK 保持现状。
   - 语义点位列表用 `point_key + variable_id` 展示、可移除（不进入 `sels`）。
   - `commitAcquisition` 把 `nckPoints` 与 `definitions` 写入草稿；零采集点判定改为 `sels.length === 0 && nckPoints.length === 0`。
2. `apps/mesa-web/src/addDevice/bootstrap.ts`
   - `AcquisitionDraft` 增可选 `nckPoints` / `nckDefinitions`。
   - NCK 分支调用 `planEndpointTaskSave` 并使用 `plan.tasks`；失败映射 `put-tasks`。非 NCK 分支保持 `assembleBinding` 原样（A28/A29 回归不动）。
3. `apps/mesa-web/src/addDevice/AddDeviceFlow.test.tsx`、`bootstrap.test.ts`：按 §4 增补用例。
4. `ResourcePickerAntd.tsx` **不需要改**（`semantic` 已就绪）；若确需改动，只允许“调用点/类型”级别，并在报告中单独说明。

## 3. 必须消除的第二套逻辑

| # | 位置 | 现状 | PR-D 处理 |
|---|---|---|---|
| S1 | `bootstrap.ts:156-162` | 手工拼 `{id:"t1", schedule, binding}`，NCK 无法表达 | NCK 改走 `planEndpointTaskSave` 的 `plan.tasks`；bootstrap 内**不出现**任何 kind 字面量（保持 A28） |
| S2 | `AddDeviceFlow.tsx:262-286` | `schemaOf` + `applySelectionAdd`（旧 canonical 的 key/冲突裁决） | 仅非 NCK 使用；NCK 点位**不得**进入 `selections`，也不得调用 `applySelectionAdd` |
| S3 | `ResourcePickerAntd.tsx:73-83,193` | `suggestPointKey = resource.output` 自动命名 | 语义模式在 `:141-150` 提前返回，天然不经过；PR-D **不得**为 NCK 另写命名/加后缀逻辑 |
| S4 | `NckPointFields.tsx:266-278,319-328` | `point_key` 唯一来源 = 服务端预览 | 向导只接收 `onAddPoint(point)` 的结果，**不读取/不重算** `identity_template` |
| S5 | `AddDeviceFlow.tsx:233`、`bootstrap.ts:109-113` | `suggestEndpointId` / `newOperationKey` | **不是点位身份**，保留；前者是 Endpoint id，后者是幂等键 |
| S6 | 保存入口 | 唯一 `POST /device-bootstrap`，无 `PUT /tasks` | 保持唯一；**禁止**在向导里新增 `PUT /api/v1/tasks/*` |

**判定口径**：NCK 的 `point_key` 在向导内只能来自 `previewNckPoint` 的成功响应；NCK 的任务只能来自 `planEndpointTaskSave` 的成功结果。二者之外出现任何生成/改写，都属第二套逻辑。

## 4. PR-D 验收矩阵

| # | 判据（可执行） | 建议测试文件 / 用例名 |
|---|---|---|
| D1 | **A8（向导半边）**：NCK 第③步不出现“缺少必填参数：area/area_no/module/column/line…”，只填 Target 后“加入点位”可用 | `AddDeviceFlow.test.tsx`：`nck_semantic_mode_does_not_require_native_fields (A8)` |
| D2 | **A21（向导半边）**：NCK 第③步默认渲染 `nck-semantic-picker`；无“手动”页、无“无可用资源”；旧 Picker 不渲染 | 同上：`nck_semantic_picker_is_default_entry_in_wizard (A21)` |
| D3 | **A22（向导半边）**：旧 `variable` 入口整体不存在；`nativeSchema` 存在时只出现“高级原生配置”入口；不产生 `mesa.resources.v1` | 同上：`legacy_variable_entry_absent_and_native_is_labeled (A22)` |
| D4 | 目录在**无 Endpoint** 时加载：第③步发出 `GET /api/v1/drivers/sinumerik-nck/definitions`，零 `POST /endpoints`；目录失败 ⇒ 语义保存禁用且文案含原因 | 同上：`catalog_loads_before_endpoint_exists` / `catalog_failure_disables_semantic_save` |
| D5 | `family` 来自连接草稿：预览请求体 `family` 等于第②步所选；缺 family ⇒ 禁用；`family` 与定义不一致 ⇒ `FAMILY_MISMATCH` 且不可加入 | 同上 + `NckPointFields.test.tsx`（既有 family 用例不重复） |
| D6 | 提交体：`acquisition.tasks[0].binding.kind === "nck.points.v1"`、`config` 恰为 `{points:[{kind,point_key,parameters}]}`、`point_key` 逐字等于预览响应、任务 `id` 稳定、`schedule.mode="poll"` | `AddDeviceFlow.test.tsx`：`nck_bootstrap_body_is_nck_points_v1`；`bootstrap.test.ts`：`nck_points_use_save_plan` |
| D7 | **scope 无旁路**：identity 定义不进入可选列表；绕过 UI 注入 identity 点位 ⇒ 保存计划返回 `SCOPE_NOT_COLLECTABLE`，bootstrap **零 POST** | `bootstrap.test.ts`：`identity_point_is_rejected_before_post` |
| D8 | **保存路径唯一**：全流程仅 1 次 `POST /device-bootstrap`，0 次 `PUT /tasks`、0 次 `POST /endpoints`；NCK 任务只由 `planEndpointTaskSave` 产出 | `AddDeviceFlow.test.tsx`：`single_write_path_only_bootstrap` |
| D9 | **零采集点不退化**：NCK 零点位仍 `tasks=[]` + `start=false` | `bootstrap.test.ts`（既有 A29 用例）+ 向导用例 |
| D10 | **全链（mock）**：新建设备 → 配语义点位 → 确认 → 请求体正确 → 201 → 出现“进入设备” | `AddDeviceFlow.test.tsx`：`nck_end_to_end_wizard_mock` |
| D11 | **全链（真实 API，须单独授权）**：mesad + NCK 软件 emulator；创建 → 保存 → Driver `configure` 接受 → `GET /tasks` 回读点位逐字一致 → 启动后 `points/latest` 出现 GOOD | 一次性脚本（建议 `target/validation/pr-d-e2e.py`，gitignored）；**不得**提交为生产测试 |

> D11 是唯一能证明“Driver 接受 + 回读一致”的判据；D10 只能证明前端请求体正确。D11 的环境与 PR-C §10 相同（临时 DB/端口/PKI + emulator），**不连真实 CNC**。

## 5. 风险与禁止项

### 5.1 可以改

- `apps/mesa-web/src/addDevice/AddDeviceFlow.tsx` 及其测试（PR-C 已裁决归 PR-D 独占，`边界文档:1087-1090`）。
- `apps/mesa-web/src/addDevice/bootstrap.ts` 及其测试（PR-C 已把装配收敛到 `deviceModel`，此处只加 NCK 分支）。
- `ResourcePickerAntd` 的**调用点**（不是组件语义）。

### 5.2 仍然禁止

- `apps/mesa-web/src/resourceSelectionModel.ts` 主体算法（`:44-71`、`:169-377`）。
- `crates/**`、`drivers/**`、`proto/**`、`apps/mesad/**`、`tests/driver-contract/**`。
- 冻结设计文档；`AddDeviceFlow` 之外的 PR-C 已交付文件**不得重写**：`nckCatalog.ts`、`nckPointModel.ts`、`nckCollectability.ts`、`nckTaskBinding.ts`、`endpointSavePlan.ts`、`NckPointFields.tsx`、`NckNativeFields.tsx`、`ResourceBrowseAntd.tsx`、`EndpointAcquisitionPane.tsx`。
- 若实现中认为必须改上述文件，**停止并报告**，不得“顺手改一下”。

### 5.3 主要风险

1. **快照语义误用**：把“计划中的 Endpoint”快照写法带到已有 Endpoint 场景，会绕过 `SNAPSHOT_NOT_READY` 保护。必须用调用点隔离 + 测试钉死。
2. **native 模式的模型层缺口**：保存计划对 `nck_native` 只校验信封（`nckTaskBinding.ts:397-400`），原生字段校验在 `NckNativeFields`/端点页 `verifyOne`。若向导启用高级原生，必须确保 `NckNativeFields` 自身校验通过才可加入（`NckNativeFields.tsx:47-67,139-147`）。
3. **点位唯一性**：向导内 `takenPointKeys` 只含本批点位；跨任务唯一性在保存计划里由 `collectEndpointPointKeys` 兜底（`nckTaskBinding.ts:332-358`），新 Endpoint 无既有任务，风险低但需 D7 覆盖。
4. **目录失败即禁用**：不得用空目录/旧缓存继续（`nckCatalog.ts:480-540` 的完整性校验必须保留）。
5. **Browse 不可伪装**：向导内不传 `endpointId`，也不要给“浏览”做假入口；文案必须说明“保存后到设备详情页浏览”。
6. **幂等键**：同一 Flow 重试复用 `operationKey`；新 Flow 必须新 key（`bootstrap.ts:101-113`），PR-D 不得改动该语义。
7. **零采集点与空点位混淆**：`planEndpointTaskSave` 对空 `nckPoints` 返回 `EMPTY_POINTS`（`endpointSavePlan.ts:148-156`）；零采集点必须走 `tasks=[]` 分支，不能调用计划。

### 5.4 是否需要 mesad

- **不需要**：类型检查、lint、vitest（含 D1–D10 的 mock 全链）。
- **需要且须单独授权**：D11 真实 API 全链（mesad + `mesa-nck-emulator`）。该验证涉及启动进程/临时 DB/端口，按仓库纪律必须先取得上级明确授权；本轮审查**未启动**任何服务。

## 6. 与 PR-C 的边界（PR-D 不得回改）

| PR-C 已验收行为 | 证据 | PR-D 约束 |
|---|---|---|
| `planEndpointTaskSave` 是唯一保存决策点 | `endpointSavePlan.ts:97-196`；收口审查 §4.4 | 只新增调用点，不改签名/顺序/错误码 |
| `scope` 准入双执行（UI + 提交模型） | `nckCollectability.ts:1-25`；`nckTaskBinding.ts:297-309` | 向导必须经保存计划，不得只靠表单 |
| 权威 Preview 唯一性 | `nckPointModel.ts:594-728`；`NckPointFields.tsx:182-263` | 不读 `identity_template`、不重算 Key |
| 按 Driver 分流 `nck.points.v1` / `mesa.resources.v1` | `deviceModel.ts:242-289`；`bootstrap.ts:139-155` | 不在 bootstrap 写 kind 字面量 |
| 非 NCK 零行为变化（含 §3.5 的有意例外） | 收口审查 §3.5、§4.5 | 非 NCK 分支逐字保留 |
| 零采集点 `tasks=[]` | `bootstrap.ts:131-138,176` | 不改 |
| 组件层 A8/A21/A22 已落地 | `ResourcePickerAntd.tsx:141-150,354-470`；`ResourcePickerAntd.test.tsx:427` | 只接线，不改组件行为 |

**边界一句话**：PR-D 只允许“把已有能力接到向导”，任何“为了让向导好用而改模型/组件语义”的改动都越界。

## 7. 待确认（不编造）

1. 向导是否应开放“高级原生配置”（任务书要求复用 `NckNativeFields`，但原生字段的模型层复核缺口见 §5.3-2）；建议**开放但必须由 `NckNativeFields` 自身校验通过**，并在 D 矩阵补一条 native 用例。
2. 向导内是否需要支持“编辑已加入的语义点位”（端点页支持 `replacePoint`，向导当前只列可删）；最小方案是“删除后重加”，避免引入编辑态状态机。
3. 目录请求是否带 `family` 过滤：端点页不带（`EndpointAcquisitionPane.tsx:365`），建议保持一致，交叉校验交给 `checkNckFamily`。
4. 是否把“计划中的 Endpoint”快照封装成显式命名常量（仅调用点注释，不改 `endpointSavePlan.ts`）；**待实现阶段决定**。
