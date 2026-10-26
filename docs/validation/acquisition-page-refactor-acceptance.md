# 采集页面重构 · 第 3 步统一验收报告

> 分支 `feat/acquisition-page-refactor`，验收起点 `HEAD = f995a90`（第 1、2 步已提交）。
> 本文件是**第 3 步（统一验收）唯一新增的文档**；代码改动仅限 `apps/mesa-web/`（见 §7）。
> 实施依据：`docs/design/acquisition-page-refactor-review.md`（§4.7 保存区状态机、§5 238 点验收 11 项、§7.2 实施期待定）。
> 结论：**11 项验收中 10 项通过、1 项未通过（§5.2-5「操作失败后能直接找到处理位置」未实现，见 §2 与 §8）**；请求顺序 8 条全部符合预期；窄屏 4 项以 jsdom 断言通过，像素级遮挡仍需人工目视（§4）。

---

## 1. 验收环境

| 项 | 事实 |
|---|---|
| 仓库/分支 | `D:\Mine\Project\mesa`，`feat/acquisition-page-refactor`，验收前 `HEAD = f995a90` |
| 前端 | `apps/mesa-web`（React 18.3 + antd 6.6 + Vite 5.4 + vitest 1.6 + jsdom 24.1） |
| 场景夹具 | `apps/mesa-web/src/test/acquisitionFixture238.ts`（第 2 步建）：235 原生 + 3 语义 = **238 点**，1 个 `poll` 任务（`interval_ms = 1000`）；派生自 `target/validation/t0-rev13-tasks.json` 的**形状**（区域/模块分布逐项对齐） |
| 正常夹具 | 上述夹具 + 一个**保留任务**（订阅型，见 §6-②） |
| 问题夹具 | 在正常夹具上**显式追加**一个定义目录里不存在的点位（`missing.variable`）→ 身份无法重新核验（`DEFINITION_MISSING`）；两者不混用（§5.1） |
| 服务端 | **不启服务**：全部 `fetch` 由测试桩替换（`installProbedFetch`，`apps/mesa-web/src/test/requestProbe.ts`） |
| 真机 / 生产库 | **未连接、未读取、未改动**；无 `crates/**`、`proto/**`、`drivers/**`、契约或版本改动 |
| 浏览器自动化 | 无（无 Playwright/Puppeteer 依赖）。窄屏以 jsdom + 显式 `window.innerWidth` 断言，像素级结果列人工目视（§4） |
| 机械判定口径 | Alert 计数、行数、页码、`scrollTop`、请求序列、文案字符串，全部可由断言判定 |

命令（均在工作树内执行，未做任何 git 写操作）：

```
pnpm --dir apps/mesa-web test        # 70 文件 / 796 用例全绿
pnpm --dir apps/mesa-web typecheck   # tsc --noEmit 通过
pnpm --dir apps/mesa-web lint        # eslint . 通过
pnpm --dir apps/mesa-web build       # tsc -b && vite build 通过（chunk 体积告警为既有）
```

---

## 2. 11 项验收逐条结果

判定依据：`docs/design/acquisition-page-refactor-review.md:545-559`。

| # | 验收项 | 期望 | 实际 | 结果 | 证据（测试名） |
|---|---|---|---|---|---|
| 1 | 正常进入页面（**仅正常夹具**） | 首屏 Alert = 0、绿色成功标签 = 0、教程/流程文案 = 0；238 行不同时渲染（只 25 行） | Alert 数 `0`；全文不含「校验通过」、不含「选择变量 → 填写必要参数」；渲染 **25** 行、`共 238 项`、`第 1 / 10 页`；保存区不渲染 | **通过** | `full_path_on_238_toasts_nothing_and_saves_full_snapshot`、`first_screen_is_quiet_and_only_renders_one_page` |
| 2 | 238 点搜索直达 | 「轴实际位置」（名称）、「区域 C · 区域号 1 · 模块 122」（地址片段）、`native.C.1.122`（标识片段）各命中 1 行且落在第 1 页；不存在串给空态 | 三路各命中 1 行、页码回到 `1 / 1`；另验证**配置内容**（参数值 `F64_LE`）可搜（238 行全中）；`不存在的点位` → `没有匹配的点位，可改用标识搜索` + `清除搜索` | **通过** | `full_path_on_238_toasts_nothing_and_saves_full_snapshot`、`search_by_name_address_and_identifier_lands_on_page_one` |
| 3 | 编辑最后一页 | 第 10 页编辑 → 确认修改 → 仍第 10 页、`scrollTop` 一致（±0） | 切换后仍 `第 10 / 10 页`，`scrollTop` 保持 `240`；行标「已修改」；摘要 `1 项点位变更（修改 1）` | **通过** | `full_path_on_238_toasts_nothing_and_saves_full_snapshot`、`editing_on_last_page_keeps_page_and_scroll` |
| 4 | 未操作时不报输入错误 | 首屏字段级错误 = 0；抽屉未打开时无字段错误；未选变量时无 `FORM_MODEL_INVALID` | `.ant-form-item-explain-error` = 0、`nck-form-issues` = 0、全文无 `FORM_MODEL_INVALID`；抽屉未打开时无变量选择器 | **通过** | `no_field_errors_when_nothing_is_touched` |
| 5 | **操作失败后能直接找到处理位置** | `issues[0].path` 返回后**自动跳到该行所在页 + 打开抽屉 + 字段高亮**，无需手工搜索 | **未实现**：issues 只做「原样展示」（`EndpointAcquisitionPane.tsx:2079-2096`）；`issue.path` 唯一用途是拼文案（`:1380`、`:2090`），全仓无任何按 path 定位页面/行/字段的代码 | **未通过** | 反证：`save_failure_keeps_draft_and_row_marker`（issues 显示正确，但页面不动）+ 全仓 grep（见 §8-1） |
| 6 | 随时知道哪些修改没保存 | 计数 = 新增+修改+移除，周期**单独表述**；增删改后同步；新增后移除**抵消**、恢复原值**取消标记** | 摘要 `1 项点位变更（修改 1）` / `1 项点位变更（新增 1）` / `1 项点位变更（移除 1）；周期另有修改：1000 → 500 ms`；撤销后只剩周期项；周期改回原值后保存区整体收起 | **通过** | `full_path_on_238_…`、`summary_separates_point_changes_from_interval_change`、`add_then_remove_cancels_out`、`add_via_drawer_returns_to_same_page_and_scroll` |
| 7 | 知道保存会不会影响采集 | 运行中 + 有修改 ⇒ 中断说明「保存会暂时中断当前连接的采集」；已停止/无修改不出现 | 运行中：`draft-save-note` = 该句；主按钮 `保存并恢复采集`、次级 `保存后保持停止`；切换后 `保存并停止采集` / `保存后恢复采集`；已停止：不出现中断说明（改述「保存只写入配置…」）；无修改时保存区不渲染；全文不含「约 1–2 秒」 | **通过** | `save_labels_follow_run_state_and_restart_choice` |
| 8 | 保存失败保留修改 | 计数不变、保存区仍在、行标记仍在 | PUT 400 + 1 条 issue 后：`data-kind="failed"`，`draft-save-summary` 仍是 `1 项点位变更（修改 1）`，行仍标「已修改」，无成功结果条 | **通过** | `save_failure_keeps_draft_and_row_marker` |
| 9 | 反馈与服务端真实结果一致 | PUT 2xx 但回读不一致 ⇒「待核对」/失败，**不报成功**；NCK 与非 NCK 都回读；成功通知只 1 次 | 回读不一致（点位相同、周期没保存）→ `data-kind="pending-verify"`、无 `apply-result`、基线不刷新；回读请求失败同样；成功后 `apply-result` 恰 1 条且 `.ant-message-notice` = 0 | **通过** | `mismatched_readback_never_repeats_put`、`回读不一致（点位相同、周期没保存）⇒ 待核对…`、`回读请求失败 ⇒ 待核对（不是成功），且不重新 PUT`、`full_path_on_238_…` |
| 10 | 不丢点位、不覆盖其他任务 | 保存仍是全量替换；238 点保存后仍 238；`preservedTasks` 逐字保留 | PUT 体 `tasks[0]` 含 **238** 点（235 原生 + 3 语义），未编辑的 235 点逐字等于加载值；保留任务 `keep-sub-task` 深度相等（含 4 个 subscribe 缺省字段） | **通过** | `full_path_on_238_toasts_nothing_and_saves_full_snapshot` |
| 11 | 不削弱身份与范围校验 | §1.3 六项守卫逐项有测试锚点，重构前后断言不变 | 五处锚点测试**未改一行断言**且全绿（行号为当前工作树实测，替代评审稿 §5.3 的旧行号）：`EndpointAcquisitionPane.nckB.test.tsx:614`「门禁 6：已有点位预览失败 ⇒ 不得按旧 Key 继续提交」（身份与 Key）、`:777`「P1：Endpoint 层最终保存守卫（完整 Schema）」（未知必填字段在任何请求前拦截）、`:319`「门禁 7」+`:392`「门禁 8」（保存与回读）、`:531`「门禁 9」（存量问题点位只能显式移除）、`EndpointAcquisitionPane.ux.test.tsx:405`「P2-2（状态区）」+`:430`「P2-1（已保存状态）」（状态与草稿区分） | **通过** | 上述 5 个 describe 内的全部用例（本轮全绿） |

**未通过项（#5）说明**：这条不是断言问题，而是**功能缺口**——后端 `issues[]` 目前只被展示，未被用作导航输入。修它需要先确认 `path` 的稳定形态（后端至少存在 `tasks[0].binding.kind`、`tasks[i].binding`、`<root>.<field>` 三类形态，见 `crates/core-api/src/binding_gate.rs:60`、`:97`、`crates/core-types/src/capability.rs:275` 等），否则只能靠猜。按「本步只验收、不重做功能」的约束，本轮**未实现**，作为缺陷记录（§8-1）与待确认项（§9）。

---

## 3. 请求顺序证据

探针：`apps/mesa-web/src/test/requestProbe.ts`（`classifyRequest` 把每次 fetch 归成 `GET:endpoints / GET:descriptor / GET:tasks / PUT:tasks / STOP / START / PREVIEW`，按发生顺序记录）。`writeSequence()` = 仅保留 `STOP / PUT:tasks / GET:tasks / START` 四个标签，**保留顺序**。
夹具规模：**2–4 个点位的小夹具**（顺序语义与点数无关；238 点规模的同一路径已由 §2 的完整路径用例覆盖）。

| # | 场景 | 期望序列 | 实际序列（断言值） | 结果 | 测试名 |
|---|---|---|---|---|---|
| 1 | 无修改 | 零 PUT / stop / start | `[]`（另：无修改时保存区不渲染；改周期再改回原值后仍 `[]`） | 通过 | `no_changes_sends_no_write_request` |
| 2 | 已停止保存 | 仅 PUT（无 stop/start） | `PUT:tasks → GET:tasks`；`GET:endpoints`（保存前复核）在 `PUT:tasks` 之前 | 通过 | `stopped_save_is_put_then_readback_only` |
| 3 | 运行中保存并恢复 | stop → PUT → 回读 GET → start | `STOP → PUT:tasks → GET:tasks → START`；且 `GET:endpoints`（复核）与 `PREVIEW`（提交前核验）均早于 `STOP` | 通过 | `running_save_with_restart_is_stop_put_get_start` |
| 4 | 运行中保存并停止 | stop → PUT → 回读 GET（无 start） | `STOP → PUT:tasks → GET:tasks`；`startCalls = 0` | 通过 | `running_save_keep_stopped_omits_start` |
| 5 | 回读不一致 | stop → PUT → 回读 GET，**不得出现第二个 PUT** | 保存：`STOP → PUT:tasks → GET:tasks`（无 START）；点「重新核对」后增量仅 `GET:tasks`，`PUT:tasks` 计数 = 0 | 通过 | `mismatched_readback_never_repeats_put` |
| 6 | 启动失败后重试 | 仅 start（无 stop、无 PUT） | 首轮保存：`STOP → PUT:tasks → GET:tasks → START`；重试增量（去掉复核用 GET）= `START`；`stopCalls / putBodies` 不增 | 通过 | `retry_start_sends_start_only` |
| 7 | 状态未知 | **零请求** | 绕过 `disabled` 直接调用提交函数后，探针记录 **`[]`**（连 `GET:endpoints` 复核与 `PREVIEW` 都没有） | 通过 | `unknown_state_sends_zero_requests_even_if_disabled_is_bypassed` |
| 8 | 保存前复核发现状态变化 | 零 stop/PUT/start | `writeSequence() = []`；`PREVIEW = 0`（复核在提交前核验之前）；`putBodies = 0` | 通过 | `state_changed_during_recheck_aborts_before_any_write` |

**守卫都在 stop/PUT/start 之前（专项验证）**

| 守卫 | 证据 | 结果 |
|---|---|---|
| 运行状态未知守卫（最外层） | 未知状态下绕过按钮直接调用提交 ⇒ 探针 **零记录**（`unknown_state_sends_zero_requests_even_if_disabled_is_bypassed`）；代码位置 `EndpointAcquisitionPane.tsx:1211-1213`（在任何请求之前 return） | 通过 |
| 完整 Schema 守卫（未知字段类型） | 原生字段含界面不认的 `matrix` ⇒ `acquisition-unsupported-fields` 出现、按钮禁用；绕过调用后探针 **零记录**（`schema_guard_rejects_before_any_request`） | 通过 |
| 快照门 | 计划层直接拒绝：`SNAPSHOT_NOT_READY`，且拒绝结果**不携带** `tasks`（`snapshot_gate_rejects_in_the_plan_before_any_request`） | 通过 |
| 提交前身份复核（NCK） | 预览失败 ⇒ 提示「保存前重新核验失败」，`PREVIEW` 已发生但 `writeSequence = []`（`pre_submit_identity_recheck_fails_closed_before_stop`） | 通过 |
| 过渡态（CONNECTING）按运行中处理 | `CONNECTING` ⇒ 状态显示「运行中」、按钮「保存并恢复采集」、序列 `STOP → PUT:tasks → GET:tasks → START`（`connecting_state_uses_stop_put_get_start`） | 通过 |

---

## 4. 窄屏结果

视口由测试显式设定（`Object.defineProperty(window, "innerWidth", …)` + `resize` 事件），桌面基准 1440、窄屏 480。

### 4.1 jsdom 可机械判定的部分（已断言）

| 检查项 | 期望 | 实测断言 | 测试名 |
|---|---|---|---|
| 抽屉桌面宽度 | 640px | `.ant-drawer-content-wrapper` 的 `width = 640px` | `desktop_keeps_640_and_narrow_follows_viewport` |
| 抽屉窄屏自适应 | 收敛到视口、不溢出 | `innerWidth = 480` ⇒ `width = 480px`；采集页真实抽屉同样 480px | 同上 + `pane_drawer_also_adapts_on_narrow_viewport` |
| 无拖拽调整 | 不提供宽度拖拽 | `.ant-drawer-resizable-handle` = `null`（`PointDrawer.tsx:43-51` 只收敛宽度） | `desktop_keeps_640_and_narrow_follows_viewport` |
| 窄屏三列表格 | 可用、不横向溢出 | `point-table[data-layout="stacked"]`；行 `gridTemplateColumns = minmax(0, 1fr)`（单列堆叠）；窄屏不渲染三列表头；名称/摘要/操作三段俱在且操作可点；三段带 `aria-label` | `stacked_layout_on_narrow_and_columns_on_desktop` |
| 视口变化即时切换 | 变宽回三列 | `innerWidth=1280` ⇒ `data-layout="columns"`，表头三项复现，行模板含 `132px` | 同上 |
| 保存区定位方式 | 固定工作区底部、预留空间 | `position: sticky`、`bottom: 0`、不透明背景、**不是 `fixed`**（在文档流内，末尾内容不会被永久盖住）；层级 `0 < z < 1000`（低于抽屉，抽屉操作不被压住） | `save_bar_is_in_flow_sticky_below_the_list_and_below_drawers` |
| 不遮挡分页 | 分页在保存区之前 | DOM 顺序断言：`point-table` 的 `compareDocumentPosition(draft-save-bar)` 含 `DOCUMENT_POSITION_FOLLOWING` | 同上 |
| 行内菜单不被保存区盖住 | 菜单层级高于保存区 | `point-row-menu` 的 `z-index(20) > draft-save-bar(5)` | 同上 |
| 搜索框不撑破窄视口 | 宽度可收缩 | `PointSearch.tsx:35`：`width: 360, maxWidth: "100%", minWidth: 0` | （样式断言随同窄屏用例渲染即生效，未单独断言） |

### 4.2 必须人工目视的部分（jsdom 无布局引擎，无法机械判定）

1. 480px 视口下**抽屉占满宽度后**，内部字段（变量选择器 320px 最小宽、原生字段两列 `minmax(240px,1fr)` 自适应）是否仍然不出现横向滚动条、按钮是否都在可视区。
2. 保存区搭在页面底部时，滚动到底部**分页是否完整可见**（"sticky + 文档流内"保证无永久遮挡，但滚动中间态的视觉重叠需肉眼确认）。
3. 窄屏下行内「⋯」菜单是否被容器裁切（菜单 `top: 36px`、`right: 8px`）。
4. 抽屉遮罩与保存区的视觉层叠顺序（断言只证明 `z-index` 数值关系）。
5. 真实窄屏设备（如 360×640）上的字号与触摸目标大小。

> 说明：§4.1 的 9 条是"组件声明的布局"，§4.2 的 5 条是"浏览器实际排版"。本报告**不**把前者当作后者通过的证据。

---

## 5. 页面效果描述（便于对照）

### 5.1 首屏（无修改、已停止、238 点、正常夹具）

渲染结构（自上而下，`EndpointAcquisitionPane.tsx:1676-1997`）：

1. **一行运行摘要**（`acquisition-status`）：`采集状态：已停止 · 点位：238 个（已保存） · 周期：1000 ms（已保存）`。
2. **采集周期**：一个数字输入框（`10–60000`，单位 `ms`）+ 一个帮助图标（压测说明只在 Tooltip 里）+ 一行小字「周期为配置值，实际采集表现以运行结果为准」。
3. **列表标题行**：`已配置点位（238）`，右侧一个主按钮 `+ 添加点位`。
4. **搜索行**：输入框（占位「搜索名称、标识或配置内容……」）；**没有筛选项**——没问题行、没修改行时不渲染下拉。
5. **列表**（最大高度 520px 的滚动区）：三列表头 `名称或标识 / 配置摘要 / 操作`；每行 `名称`（如「轴实际位置」）+ 必要标识、`配置摘要`（如「通道 1 · 轴 1」、「区域 C · 区域号 1 · 模块 122 · 列 1 · 行 1」）、操作 `编辑 · ⋯`。
6. **分页行**：`共 238 项｜每页 [25][50][100] 项｜< 1 2 3 … 10 >｜第 1 / 10 页`。
7. **无保存区、无 Alert、无教程、无逐行绿标**；若有保留任务，列表下方一行静态说明「另有 N 个任务不在此编辑，保存时原样保留：…」。

### 5.2 有未保存修改（已停止）

- 顶部摘要相应变化：`点位：238 个（已保存） → 239 个（待保存）`、`周期：1000 ms（已保存） → 500 ms（待保存）`。
- 被修改的行多一条短文案 `已修改`（新增行是 `新增`；有问题时合并成 `已修改 · 有问题`）。
- 底部出现**固定保存区**（`draft-save-bar`）：第一行摘要 `1 项点位变更（修改 1）；周期另有修改：1000 → 500 ms`；第二行说明「保存只写入配置，不会启动采集」；右侧 `放弃修改` + 主按钮 `保存配置`。
- 关键元素清单：`draft-save-summary`、`draft-save-note`、`discard-draft`、`apply-button`。

### 5.3 运行中 + 有修改

- 顶部：`采集状态：运行中 …`。
- 保存区第一行摘要同上；第二行为**中断说明**「保存会暂时中断当前连接的采集」（不含时长承诺）。
- 动作：主按钮 `保存并恢复采集`（默认）+ 次级文字链接 `保存后保持停止`；点击次级后主按钮变 `保存并停止采集`、链接变 `保存后恢复采集`。
- 保存过程：按钮 loading + 文案「处理中…」；成功后保存区只剩一条结果 `配置已保存，采集已启动`（或 `配置已保存，当前停止`），动作区收起，约 5s 后整条结果自动消失。
- 失败/待核对：保存区**保留**并显示持续结果块（`save-outcome`；`data-kind = failed / state-changed / pending-verify`），待核对时提供 `重新核对`。

---

## 6. 两条服务端归一化豁免（精确到字段名）

背景：§7.2-2 要求「确认前按不一致处理（fail-closed）」。逐字比对实现于 `apps/mesa-web/src/endpointSavePlan.ts:237-271`（`sameTaskSet` / `sameSubmittedConfig`）。

### 6.1 豁免①：任务数组按 task id 升序返回 —— **成立**

| 项 | 内容 |
|---|---|
| 归一化行为 | `GET /api/v1/tasks?endpoint=` 的返回顺序 = **task id 升序**，提交顺序不被保留 |
| 依据（代码） | `crates/config-store/src/lib.rs:2169-2173`（`list_tasks`，SQL `... FROM tasks WHERE endpoint_id=?1 ORDER BY id`，`ORDER BY id` 在 `:2172`） |
| 前端处理 | 任务数组**按 `id` 对齐**比较（`endpointSavePlan.ts:241-254`：先建 `id → task` 映射，再逐 id 比对），因此「数组顺序」不算配置差异；**任务内部的点位/选择数组顺序仍严格比较**（保序） |
| 参与对齐的字段清单 | **对齐键**：`task.id`（唯一性再校验：重复 id ⇒ 判不等）。**逐字比对字段**（每个 id 下）：`schedule`（`mode` 为 `poll` 时含 `interval_ms`；为 `subscribe` 时含 `publishing_interval_ms`、`sampling_interval_ms`、`queue_size`、`discard_oldest`）、`binding.kind`、`binding.config`（NCK：`config.points[]` 每个点位的 `kind` + `point_key` + 完整 `parameters`；通用：`config.selections[]` 的 `resource_id` + `parameters` + `outputs`）。**不参与**：`revision`（在响应外层，不在 task 内）、行 `rowId`、前端草稿指纹 |
| 对应测试 | `endpointSavePlan.test.ts` 的 `task_array_order_is_not_a_change（服务端按 id 排序返回）`、`完全一致（任务数组顺序不同也算一致）` |

### 6.2 豁免②：subscribe schedule 缺省补默认 —— **经核实不成立，如实剔除（不作为豁免）**

| 项 | 内容 |
|---|---|
| 服务端行为 | `TaskSchedule::Subscribe` 的**四个**字段都有 serde default：`publishing_interval_ms`（默认 500）、`sampling_interval_ms`（250）、`queue_size`（10）、`discard_oldest`（true）——依据 `crates/core-types/src/lib.rs:442-451`（四个 `#[serde(default = …)]`）+ `:454-468`（四个默认值函数）。`poll` 的 `interval_ms` **没有** default（缺失即反序列化失败，`:439-441`） |
| 为什么不算豁免 | 前端基线来自 `GET`（服务端已归一，四个字段必然存在），保留任务**逐字复制**，因此提交值与回读值本就带同样字段；一旦提交值真的省掉其中任一字段，`sameSubmittedConfig` **仍判不一致**（fail-closed）。即：**四个字段都不放宽，不需要任何豁免** |
| 字段清单（原样保留、逐字比对，不设默认值补齐） | `publishing_interval_ms`、`sampling_interval_ms`、`queue_size`、`discard_oldest` |
| 对应测试 | `endpointSavePlan.test.ts` 的 `subscribe_defaults_present_on_both_sides_compare_equal`（两侧都有缺省字段 ⇒ 相等）、`dropping_a_defaulted_field_is_a_mismatch_not_an_exemption`（省掉字段 ⇒ 不一致）；页面级：`full_path_on_238_toasts_nothing_and_saves_full_snapshot` 断言保留任务（含上述四个字段）在 PUT 体中**深度相等** |

> 结论：本轮**只有一条**豁免（任务数组按 id 升序）。第 2 条经核实不构成归一化豁免，按「若某字段其实不涉及归一化，如实剔除」的要求剔除，而不是凑数保留。

---

## 7. 已知限制

| # | 限制 | 性质 | 影响 |
|---|---|---|---|
| 1 | 无浏览器自动化：布局、遮挡、字号、触摸目标未做像素级验证 | jsdom 限制 | §4.1 的 9 条（声明层面）已通过；§4.2 的 5 条需人工目视 |
| 2 | 服务端为 fetch 桩：`GET /tasks`、`PUT /tasks`、`stop`、`start`、`identity-preview` 的响应形状由测试构造 | stub | 验证的是**前端逻辑与请求序列**，不是后端行为；后端契约正确性由 Rust 侧 Contract Test 负责 |
| 3 | 238 点夹具是「形状派生」而非快照逐字复制（无真机/无生产库） | 夹具 | 点数、区域分布、字段值与快照同形；逐点业务含义不代表现场 |
| 4 | 回读一致性判定为**逐字相等**，不含任何归一化放宽 | 设计（§4.4.3） | 若后端将来对 schedule 等字段做新归一化，会退化为「待核对」而非误报成功（fail-closed） |
| 5 | 「保存结果待核对 → 核对一致后继续恢复采集」依赖 `pendingVerify` 内存态：页面刷新即丢失 | 现状 | 刷新后用户需重新核对/重新保存（不会自动重复写入） |
| 6 | 无并发原子保证（后端 PUT 无条件写、无 `If-Match`） | 后端限制（§4.4.4） | 两个客户端同时编辑仍可能互相覆盖；本步不消除 |
| 7 | 窄屏阈值 768px 为布局取值（非契约），抽屉/表格共用同一阈值 | 实现细节 | 与 CSS 断点表无联动；调整需同时改 `useViewportWidth.ts:15` |
| 8 | 保存区「约 5s 后自动收起」由定时器驱动，测试只断言"动作区已收起"，未等满 5s | 测试取舍 | 自动消失的时序未做端到端断言 |
| 9 | `apply-result` 成功条在同一会话中只可能出现一条，但历史会话（多次保存）会依次替换 | 设计 | 「成功只出现一次」指单次保存内 |
| 10 | **既有 flake（与本步无关）**：`apps/mesa-web/src/pages/GlobalDataPage.test.tsx` 的「质量状态筛选与搜索只影响表格…」在整文件并行执行时偶发 5s 超时（单个用例独立跑通过、整文件重跑通过）。该文件不引用本步改动的任何模块 | 既有测试 | 复现：`npx vitest run src/pages/GlobalDataPage.test.tsx`（首次失败、重跑通过）；建议另立小项给该用例加显式超时，本步不擅自改无关测试 |

---

## 8. 本轮发现的缺陷

### 8.1 未修（功能缺口，需评审决定是否立项）

**§5.2-5「操作失败后能直接找到处理位置」未实现。**
后端 400 返回的 `issues[]` 只被原样展示（`EndpointAcquisitionPane.tsx:2079-2096`），`issue.path` 仅用于拼提示文案（`:1380`、`:2090`）。全仓无任何代码按 `path` 跳到对应行/页或打开抽屉、高亮字段（grep `\.path` 命中项均与采集页无关）。
修它需要先确认 `path` 的稳定形态：后端目前至少存在三类形态——`tasks[0].binding.kind`（`crates/core-api/src/binding_gate.rs:60`）、`tasks[i].binding`（`:97`）、`<root>.<field>`（如 `crates/core-types/src/capability.rs:275`、`:262`），并非总能唯一映射到「第 N 个点位」。建议在第 4 步单独立项（先定 path 契约，再做定位），本轮按约束不改功能。

### 8.2 已修（本轮修复，均带回归测试）

| # | 缺陷 | 影响 | 修复 | 证据 |
|---|---|---|---|---|
| D1 | 抽屉宽度固定 640px | 480px 级窄屏抽屉比视口宽 → 横向溢出、右侧按钮不可达（违反 §4.1-1 / §7-5「窄屏自适应」） | 新增 `useViewportWidth.ts`（阈值 + `fitDrawerWidth` + 视口 hook）；`PointDrawer.tsx:43-51` 按视口收敛宽度，桌面仍为 640px，不做拖拽 | `desktop_keeps_640_and_narrow_follows_viewport`、`pane_drawer_also_adapts_on_narrow_viewport` |
| D2 | 三列表格最小宽度约 586px（`170 + 260 + 132 + 24`） | 480px 视口横向溢出，必须横拖才能点到「编辑/更多」（违反「窄屏下三列表格可用」） | `PointTable.tsx:28-32、69-77`：窄屏改单列堆叠（`minmax(0,1fr)`）、不渲染三列表头、三段带 `aria-label`、行与单元 `minWidth: 0` + `overflowWrap: anywhere` | `stacked_layout_on_narrow_and_columns_on_desktop` |
| D3 | 高级必填缺失时**只展开不定位** | `NckNativeFields.tsx` 的 effect 在折叠态下调用 `getElementById`，此时高级区尚未渲染 ⇒ 永远取不到锚点，`scrollIntoView` 静默失效；用户看到高亮却仍停在表单顶部（违反 §3.1「自动展开并定位到第一个缺失字段」） | `NckNativeFields.tsx:69-80`：先展开，再在下一轮渲染（依赖 `advancedOpen`）定位 | `missing_required_advanced_field_expands_and_scrolls_to_anchor` |
| D4 | 行内「更多」菜单 `z-index: 5` 与固定保存区同层 | 同层时后渲染的保存区会盖住最后一行菜单的下半部分，「移除/技术详情」可能点不到 | `PointTable.tsx:190-196`：菜单 `z-index: 20`（仍低于抽屉 1000），并加注释说明原因 | `save_bar_is_in_flow_sticky_below_the_list_and_below_drawers` |
| D5 | 搜索框固定 `width: 360px` | 360px 以下视口会把页面撑出横向滚动条 | `PointSearch.tsx:35`：`maxWidth: "100%"`、`minWidth: 0` | 随窄屏用例渲染生效（`stacked_layout…` 同一挂载） |
| D6 | antd v6 弃用 `Drawer.width`（运行期告警） | 仅告警，无功能影响；下一大版本会移除 | `PointDrawer.tsx:49-50` 改用 `size={resolvedWidth}`（数值语义与 px 等值） | 全量测试无该告警 |

---

## 9. 待确认

1. **§5.2-5 是否纳入下一步**：需要先与后端确认 `issues[].path` 的**稳定形态**（是否保证形如 `tasks[i].binding.config.points[j].parameters.<key>`），否则前端只能猜。建议作为第 4 步独立小项。
2. **窄屏阈值 768px**：目前是前端布局取值（`useViewportWidth.ts:15`）。若产品有既定断点（或与全局数据页一致），需要统一后我再调整。
3. **订阅型 schedule 的四个缺省字段是否要在前端显式补齐**：现状是「不补、逐字保留、缺失即不一致」。若后端将来改为「缺省字段不落库」，回读会永久不一致（每次都进「待核对」），届时需要重新评审 §§4.4.3 的判据。
4. **保存区「预留空间」的实现口径**：本轮以「sticky + 位于文档流末尾」实现（`DraftSaveBar.tsx:45-58`），未额外插入空白占位块。若目视发现滚动中间态仍有遮挡，需要在页面层补 `padding-bottom`（会改变页面高度，故未擅自加）。

---

## 附录 A：本轮新增/改动文件与用例

**新增测试（3 文件 / 24 用例）**

| 文件 | 用例数 | 覆盖 |
|---|---|---|
| `apps/mesa-web/src/components/EndpointAcquisitionPane.acceptance.test.tsx` | 7 | §5.2 的 1/2/3/6/7/8/9/10 + 错误恢复（状态未知、保存失败、必填缺失定位） |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.requestOrder.test.tsx` | 12 | §3 的 8 条请求顺序 + 守卫顺序 + 过渡态 |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.narrow.test.tsx` | 5 | 窄屏（抽屉宽度、单列堆叠、保存区层级与 DOM 顺序） |

**新增测试支撑**

- `apps/mesa-web/src/test/requestProbe.ts`：请求记录探针（标签化 + 顺序序列 + 计数）。
- `apps/mesa-web/src/useViewportWidth.ts`：视口宽度 hook + 两个纯函数（有单测 `narrow_threshold_and_drawer_fit`）。

**改动（仅 `apps/mesa-web/src`）**

- `components/acquisition/PointDrawer.tsx`（窄屏收敛宽度、`size` 替代弃用的 `width`）
- `components/acquisition/PointTable.tsx`（窄屏单列堆叠、菜单层级、可收缩单元）
- `components/acquisition/PointSearch.tsx`（搜索框可收缩）
- `components/NckNativeFields.tsx`（必填缺失时先展开再定位）

**测试总量**：`67 文件 / 772 用例` → **`70 文件 / 796 用例`**（无删除、无跳过、无放宽；原断言零改动）。
