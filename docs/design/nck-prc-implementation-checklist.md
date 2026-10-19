# PR-C 短实施清单（顺序 / 裁决 / 范围 / 权限）

- 性质：**实施清单**，不是设计文档。设计事实与证据仍以
  `docs/design/nck-prc-start-review.md`（本分支原样引入）与
  `docs/design/nck-prc-frontend-semantic-config-boundary.md`（冻结，§7 矩阵 A1–A45）为准；
  本文件不新增验收编号，只给**执行顺序、判据、状态**与**本轮三项裁决**。
- 基线：`feat/nck-prc-impl` @ `3ace961`（前端 35 文件 / 405 用例全绿；tsc / lint 零错误）。
- 阅读方式：先看 §1 确认顺序，再看 §2 确认裁决（有争议先解决裁决），最后看 §3/§4 的文件范围与权限。

## 1. P0-1～P0-4：顺序与验收判据

顺序是**硬约束**：P0-1 → P0-2 → P0-3 → P0-4。后一项不得在前一项未接线时"顺手先做"，
否则会出现"准入没接、先接 Preview"这种把非法点位送进权威预览的倒序风险。

### P0-1 `scope=identity` 准入双保护（A13 / A40 / A43）

- 唯一判据：`isCollectableScope(scope) === (scope === "collect")`；`identity`、缺失、`null`、
  大小写/拼写变体、未来新值一律 `false`（**禁止**写成 `scope !== "identity"` 的反写）。
- 可执行判据（全部必须同时成立）：
  1. 合成目录 42 条（36 collect + 6 identity）⇒ `collectableDefinitions` 长度 **36**，6 条 id 均不在内；
  2. UI 只渲染 36 条可采集变量，identity 条目的 id **不出现**在 DOM；
  3. 强制选中 identity 定义 ⇒ UI 侧 `evaluateNckSelectionAdd` 拒绝（有原因文案）；
  4. **绕过 UI** 直接构造 identity 点位 ⇒ 提交模型 `validateNckTaskSubmission` 返回
     `SCOPE_NOT_COLLECTABLE`，**不抛异常**；
  5. 目录缺失 / 超时 / 501 ⇒ 全部不可采集（不缓存旧结论），保存被禁用而不是"默认放行"；
  6. Preview 返回合法 `point_key`（哪怕是 `scope=identity` 的变量）**不得**覆盖准入结论
     （`validateNckPointSubmission` 显式忽略 `preview`）。
- 当前状态：**已交付（纯模型）＋ 第二批 A 已接线（UI 与提交模型双执行）**。
  `nckCollectability.ts` 为第一批交付；A 批在 `EndpointAcquisitionPane` 内渲染准入清单、
  在保存计划里再执行一次（`endpointSavePlan.planEndpointTaskSave` → `validateNckTaskSubmission`）。

### P0-2 现有 NCK Task 编辑安全 + 双真值收敛（A19 / A28 / A29 / A36 / A37）

- 可执行判据：
  1. 任务读取按 Endpoint Driver 分流：NCK 读 `{points:[…]}`，其它读 `{selections:[…]}`，
     两种结构**不得**互相当成 `ResourceSelection[]`；
  2. 已有 NCK 任务后新增/删除点位 ⇒ **更新同一个任务**（任务 ID 与非 poll Schedule 不变），
     绝不追加一个 `mesa.resources.v1` 任务；
  3. 其它任务（含另一种 kind）逐字保留、顺序不变（A12/A19）；
  4. 快照非 `ready` / 归属不符 ⇒ **不发 PUT**（A37）；
  5. 多个可编辑候选、Binding 结构非法 ⇒ **拒绝保存且不覆盖原任务**，绝不"取第一个"；
  6. 装配单入口：`bootstrap.ts` 不出现 `"mesa.resources.v1"` 字面量，两处产出逐字相等（A28/A29）；
  7. 旧 `mergeAcquisitionTasks` 的 NCK no-op 安全网**必须消除**（它会把"没写"报成"已保存"）。
- 当前状态：**已交付（纯模型，第一批）＋ 第二批 A 已接线（四条路径：读取 / 编辑 / 冲突 /
  保存响应）**。no-op 安全网在本批改为显式抛错（不可能再被误读为成功）。

### P0-3 服务端 Preview 唯一权威 + 竞态（A2 / A15 / A39）

- 可执行判据：
  1. `Browse.identity.preview_point_key` **只**作初始展示，**不得**作为提交来源；
  2. 提交前必须有**当前实例**的 `POST /api/v1/drivers/{id}/definitions/identity-preview` 成功响应；
  3. `target` / `arguments` / `variable_id` / `definition_revision` 任一变更 ⇒ Key **立即**失效、
     `加入` 立即 disabled；
  4. 预览失败 / 被丢弃 ⇒ 保持 disabled 并给原因，**不得**回退到前端模板替换或 Browse 预览值；
  5. 竞态判定 = **请求序号 + 当前输入指纹**双条件；`AbortController` 只用于省流量，
     不得作为唯一防线（响应可能已 resolve）；
  6. 过期响应在任何 UI 文本上**不可见**。
- 当前状态：**未开始（第二批 B）**。A 批的约束：只提交"身份已确认"的点位
  （来自服务端快照的已持久化点位），**不**自行推导 Key；B 批接线时不回改 A 批的保存模型。

### P0-4 I-2 类型 / 分页 / 错误映射（A40 / A42）

- 可执行判据：
  1. 目录响应 10 字段信封、条目 14 字段、预览响应 6 字段、错误信封 `{code,kind,message}` 全部建型；
  2. `next_cursor` 逐字回传（不解析/不构造）；`limit` 缺省 100、上限 500 夹取、非法 400；
     未知查询键 400；`cursor` × `variable_id` 互斥；
  3. `total` 是"过滤后、分页前"的总数，翻页恒定；不得假设"42 < 100 所以一页"；
  4. 错误状态码只由 `kind` 决定（400/404/413/501/503/504 映射），`POINT_KEY_MISMATCH`
     **不会**来自预览；
  5. `identity.keys` 单独存在**不得**解锁提交（A42）。
- 当前状态：**未开始（第二批 B）**。A 批只以最小形状 `NckVariableDefinitionLike`
  （`variable_id` + 原始 `scope`）接收注入，不新建目录客户端、不定义分页语义。

## 2. 本轮三项正式裁决

### 2.1 `family` 来源

**裁决**：以 **Endpoint 有效连接配置 `connection.family`** 为准，并与条目
`definition.family` 做**交叉校验**。**不采纳**"目录顶层 `family=null` 一律禁止提交"。

理由：顶层 `family` 是"请求未指定且只发布一个系列时回显、多系列为 `null`"的**回显语义**，
不是"该 Endpoint 想用哪个系列"；用 Endpoint 连接配置才能把"用户意图"与"定义数据"分开校验。
`family` 不是 secret，可从 `list_endpoints` 的 `connection` 读取。

| # | 场景 | 判定 |
|---|---|---|
| 1 | Endpoint `connection.family` = 定义条目 `family` | **允许**提交 |
| 2 | Endpoint `connection.family` 缺失 / 空串 | **fail-closed**：禁止提交并提示补齐连接配置 |
| 3 | Endpoint `connection.family` ≠ 定义条目 `family` | **拒绝**提交（提示不一致的具体两值） |
| 4 | 目录顶层 `family=null`，但 Endpoint 与条目 `family` 一致 | **允许**提交（顶层 null 不是拒绝条件） |
| 5 | 条目缺 `family` 字段 | **fail-closed**：无法交叉校验 ⇒ 禁止提交 |
| 6 | Preview 返回的 `family` 与提交时使用的 `family` 不匹配 | **报错并禁止提交**（不得静默改用返回值） |

> A 批不实现 Preview，故第 6 行只登记；A 批已有 `family` 读取点（连接配置）不改任何来源语义。

### 2.2 已有 NCK Task 编辑

**裁决**：按 **Endpoint Driver** 选 kind（`bindingKindForDriver`），NCK 恒 `nck.points.v1`，
其它恒 `mesa.resources.v1`。读取严格分流：NCK 只读 `{points:[…]}`，其它只读 `{selections:[…]}`；
**不得**把两种结构都当成旧 `ResourceSelection[]`（旧 `splitAcquisitionTasks`/`selectionsOf`
只认 canonical，正是"追加第二个任务导致整体 configure 失败"的成因）。

编辑必须：保留任务 ID 与**合理** Schedule（poll 写编辑器周期，subscribe 等外来 schedule 逐字保留）；
其它任务（含跨 kind）逐字不变、顺序不变。以下任一情况**拒绝覆盖**、**不得取第一个**：
多个可编辑候选、Binding/点位结构非法、快照未就绪/归属不符。
点位身份冲突采用**前后端两层一致拒绝**：前端 `collectEndpointPointKeys` + Endpoint-wide 唯一性
（跨 Task、跨 kind），后端 Core `ensure_endpoint_point_keys_unique` 是最终裁决。

### 2.3 权威 Preview 唯一性

**裁决**：`identity-preview` 是**唯一**权威身份来源。Browse 的 `preview_point_key` **仅初始展示、
不可提交**；提交前必须有当前实例的服务端 `identity-preview` **成功**响应；
Target / Arguments / Variable / Revision 任一变更 ⇒ 立即失效；
**不得**回退前端模板替换（旧 §1.5 的降级 (b) 已废止）。
竞态用**请求序号 + 当前输入指纹**双判定，两者都匹配才采纳；`AbortController` 仅作资源优化。

## 3. 第二批 A / B 的文件范围与测试清单

### 3.1 第二批 A（本批，生产保存安全接线）

- 文件：`apps/mesa-web/src/components/EndpointAcquisitionPane.tsx`（接线 + UI 准入）、
  `apps/mesa-web/src/endpointSavePlan.ts`（**新增**：保存计划的纯模型，无 React/AntD）、
  `apps/mesa-web/src/deviceModel.ts`（仅消除 NCK no-op 安全网）、
  `apps/mesa-web/src/nckTaskBinding.test.ts`（no-op 断言改为"显式抛错"）、
  `apps/mesa-web/src/nckCollectability.ts`（仅注释：接线状态与 B 的 TODO）、
  对应测试 `endpointSavePlan.test.ts`、`EndpointAcquisitionPane.nck.test.tsx`。
- 定义目录来源：A 批**只消费不获取**，以可选 prop `nckDefinitions` 注入（B 提供）；
  缺失 ⇒ NCK 保存保持禁用（fail-closed），不退回"无 scope 也能存"。
- 变更判定口径：**NCK** 目标任务集与当前快照逐字相等 ⇒ 不发 PUT、不报成功
  （"未检测到实际修改（未发送保存请求）"）；**非 NCK** 保持原有行为（其编辑一定
  体现在 PUT 载荷中，不存在 no-op 缺陷），因此不引入该短路。
- 必须通过的 7 条关键回归：
  1. 已有 NCK Task 后新增点位 ⇒ 更新正确任务、不追加旧 Binding；
  2. 已有 NCK Task 后编辑点位 ⇒ ID/Schedule 保留、保存后可重新读取；
  3. NCK 任务快照未就绪 ⇒ 禁止 PUT；
  4. Binding 不合法或存在歧义 ⇒ 拒绝保存、不覆盖原任务；
  5. 无实际编辑变化 ⇒ 不显示"已保存"成功（不发 PUT）；
  6. 42 条定义含 6 条 identity ⇒ UI 只提供 36 条 collect；
  7. 绕过 UI 注入 identity 点位 ⇒ 提交模型仍拒绝。
- 不做但不得冲突的三条（B 批）：连续修改 Target A→B、Preview 合法 Key 但 scope 不允许、
  Endpoint family 与定义 family 不匹配。

### 3.2 第二批 B（Catalog / Preview 接线 + 竞态）

- 文件：新增 `src/nckCatalog.ts`（+test，10/14/6 字段类型与契约翻页、错误映射）、
  新增 `src/components/NckPointFields.tsx`（+test，语义表单 + Preview 状态机 + 竞态），
  `ResourcePickerAntd.tsx` / `ResourceBrowseAntd.tsx`（+test，按 Point Kind 分流、语义叶）。
- 测试：A2 / A6 / A9 / A10 / A15 / A20 / A23–A26 / A30–A32 / A39 / A42 / A43，
  含竞态子用例（迟到响应整份丢弃）与"输入变更同步失效 Key"。
- 约束：`AddDeviceFlow.tsx`(+test) 仍由 PR-D 独占；`resourceSelectionModel.ts:169-377` 逐字不动。

## 4. 权限边界

- **Draft PR 与任何 Git 写操作（commit / push / PR / 合并 / reset / stash / checkout）须由上级
  明确批准后执行**；本会话只读 git（`git show` 等）可用。
- **未获独立合并授权不得合并**（即使 PR 已创建、CI 已绿）。
- 若必须修改 `apps/mesa-web/src/addDevice/AddDeviceFlow.tsx`（及其测试）、
  `apps/mesa-web/src/resourceSelectionModel.ts` 的**主体算法**（`:44-71`、`:169-377`）、
  `crates/**`、`drivers/**`、`proto/**`、`apps/mesad/**` ⇒ **立即停止并报告**，不得"顺手改一下"。
- 已冻结设计文档只读；启动审查文件是**原样引入**，不是修改（逐字一致，hash 可校验）。
- 不得启动 Vite/mesad、不得连接设备、不得安装依赖，不得使用"连接真实依赖 Store 的临时副本"。
