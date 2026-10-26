# 采集页面重构 · 第 3 步统一验收报告（含第 4 轮验收遗漏修复）

> 分支 `feat/acquisition-page-refactor`，第 3 步验收起点 `HEAD = f995a90`，本轮起点 `HEAD = cfaa38d`（PR #146）。
> 本文件是**第 3 步（统一验收）+ 第 4 轮（验收遗漏修复）唯一新增/更新的文档**；代码改动仅限 `apps/mesa-web/`（见 §7、附录 A）。
> 实施依据：`docs/design/acquisition-page-refactor-review.md`（§2.5 第 8 条、§4.7 保存区状态机、§5.2 238 点验收 11 项、§7-7/§7-8 定案、§7.2 实施期待定）。
> **第 5 轮（最新，P1 连续后退丢草稿 / P2 通用资源字段定位）见 §10-§13**；下文 §0-§9 与附录 A 为第 3/4 轮记录，其中「浏览器后退用哨兵 + popstate」一段（§0.1 第 4 行、§8.2-D8、§7-11）的实现已按 §10 重写，测试也从「合成 popstate」升级为「真实 history.back()」。
> 结论（本轮更新）：**11 项验收 11 项通过**——原未通过的 §5.2-5「操作失败后能直接找到处理位置」已实现（可定位/不可定位两类都有测试与界面分层），并修复用户实测发现的 2 个回归（侧栏离开静默丢草稿、不支持事件的卡片常驻）；请求顺序 8 条全部符合预期；布局覆盖按「jsdom 可机械判定 / 必须真实浏览器」分开记录（§4）。

---

## 0. 本轮（第 4 轮）修了什么：三处遗留 + 布局补强

| # | 问题（来源） | 现象 | 修复落点 | 回归测试 |
|---|---|---|---|---|
| 1 | **P1** 应用内离开静默丢草稿（用户实测） | 周期改 2000 → 点左侧「总览」→ 无确认；返回后恢复 1000，**草稿被静默丢弃** | 统一守卫 `apps/mesa-web/src/workspace/draftNavGuard.tsx`（`DraftGuardProvider` / `useGuardedNavigate` / `GuardedLink` / `useBrowserBackGuard`）；接入侧栏 `App.tsx:65`、面包屑 `DeviceHeader.tsx:36`、设备 Tab `DeviceTabNav.tsx:28,41`、浏览器前进后退 `draftNavGuard.tsx:152-182` | `src/workspace/DraftNavGuard.test.tsx`（5 例：侧栏 / 面包屑 / Tab / popstate 各自「先确认 + 取消后草稿仍是 2000」，以及无草稿不弹框） |
| 2 | **P2** 不支持事件的卡片仍常驻（用户实测） | 采集页底部无条件渲染「事件订阅」，不支持的驱动显示「该驱动未声明事件流」及内部字段说明 | 判据改为 **`descriptor.events.streams` 是否为空**（`workspace/useDriverEventCatalog.ts:53`，与 `EventTaskEditor.tsx:129`、`:330-331` 同源；**不看 `driver_id`**）；不支持 ⇒ 整块不渲染，支持 ⇒ 独立折叠区域默认收起；`DeviceAcquisition.tsx:88-107` | `src/workspace/DeviceAcquisition.eventsRegion.test.tsx`（2 例）+ `DeviceWorkspaceFlat.test.tsx` 一条回归（未声明事件流 ⇒ 无区域/无「未声明事件流」文案） |
| 3 | **P2 / 验收第 5 项** 错误只有原始文本，没有处理入口 | 保存失败的 `issues[]` 只原样铺在页面上，`path` 仅用于拼文案 | 新增纯函数映射 `apps/mesa-web/src/issueLocation.ts`（`path` → 稳定 `rowId`（+ 字段））；页面层 `EndpointAcquisitionPane.tsx:1627`（`locateIssueRow`）执行「跳页 + 打开抽屉 + 高亮字段」；不可定位三类原因明确说明且**不跳转**；正文与技术详情分层 `EndpointAcquisitionPane.tsx:2214-2249` | `src/issueLocation.test.ts`（13 例，按真实 path 形态逐条断言）+ `src/components/EndpointAcquisitionPane.issueLocate.test.tsx`（4 例：字段级定位 / 任务级 / 保留任务 / 越界均不猜） |
| 4 | **布局测试补强** | 「布局问题不是测不到，而是当前测试缺少真实浏览器布局覆盖」 | 见 §4.3：新增 4 条可机械判定的布局断言 + 1 条**显式记录 jsdom 无布局引擎**的断言；不可覆盖项单列「需真实浏览器」 | `src/components/EndpointAcquisitionPane.layout.test.tsx`（4 例） |

### 0.1 三条离开路径为什么能「统一」

守卫**状态**从设备页提到应用外壳（`App.tsx:32-34` 的 `DraftGuardProvider` 包住侧栏 + 内容区），因为侧栏与面包屑都不在设备页里；判定与文案仍复用第 2 步已有的 `acquisitionDraftGuard.ts`（`confirmDiscardDraft`），**没有新写第二套确认逻辑**：

| 离开路径 | 接入点 | 机制 |
|---|---|---|
| 侧栏导航 | `App.tsx:65` | `useGuardedNavigate()`：有草稿先 `confirmDiscardDraft`，取消则**不调用** `navigate` |
| 面包屑（设备页头「设备」） | `DeviceHeader.tsx:36` | `GuardedLink`：点击一律 `preventDefault` 后交给同一守卫 |
| 设备 Tab | `DeviceTabNav.tsx:28,41` | 同一个 `useGuardedNavigate()`（原 `beforeNavigate` 专属通道已删除，避免出现第二套判定） |
| 浏览器前进 / 后退 | `draftNavGuard.tsx:152-182` | 有草稿时压一个**同 URL 哨兵条目**：后退先落回同一 URL（路由不变 ⇒ 组件不卸载 ⇒ 草稿不丢），此时弹确认；取消补回哨兵留在本页，确认才 `history.go(-1)` |

> 为什么不用 router 的 `useBlocker`：本项目是 `<BrowserRouter>` + `<Routes>` 声明式路由（`main.tsx:66`、`App.tsx:61-83`），`useBlocker` 只在 data router（`createBrowserRouter`）下可用；改 data router 会牵动 6 个以 `MemoryRouter` 渲染 `<App/>` 的既有测试文件。本轮的哨兵 + `popstate` 方案达到同样效果，且**取消后草稿保留**有断言（不是只看确认框）。
> 已知边界：哨兵依赖 `history.pushState`；若环境禁止它，浏览器前进/后退退化为无法拦截（应用内三条路径不受影响）。

### 0.2 「取消后草稿保留」的证据（不是只有确认框）

`src/workspace/DraftNavGuard.test.tsx` 每条用例都断言三件事：①出现确认框（文案「离开采集页会丢弃当前未保存的采集配置修改，确定继续？」）；②点「继续编辑」后确认框消失；③**周期输入框的值仍是 `2000`**（草稿内容还在）。第 5 条用例断言无修改时点侧栏**不弹框**且正常离开（`acquisition-interval-input` 消失）。


---

## 1. 验收环境

| 项 | 事实 |
|---|---|
| 仓库/分支 | `D:\Mine\Project\mesa`，`feat/acquisition-page-refactor`，本轮起点 `HEAD = cfaa38d`（PR #146） |
| 前端 | `apps/mesa-web`（React 18.3 + antd 6.6 + Vite 5.4 + vitest 1.6 + jsdom 24.1） |
| 场景夹具 | `apps/mesa-web/src/test/acquisitionFixture238.ts`（第 2 步建）：235 原生 + 3 语义 = **238 点**，1 个 `poll` 任务（`interval_ms = 1000`）；派生自 `target/validation/t0-rev13-tasks.json` 的**形状**（区域/模块分布逐项对齐） |
| 正常夹具 | 上述夹具 + 一个**保留任务**（订阅型，见 §6-②） |
| 问题夹具 | 在正常夹具上**显式追加**一个定义目录里不存在的点位（`missing.variable`）→ 身份无法重新核验（`DEFINITION_MISSING`）；两者不混用（§5.1）。本轮新增：保存失败用**后端真实 path 形态**的 400 桩（字段级 / 任务级 / 保留任务 / 越界） |
| 服务端 | **不启服务**：全部 `fetch` 由测试桩替换（`installProbedFetch`，`apps/mesa-web/src/test/requestProbe.ts`；页面桩 `test/pointLiveStub.ts`，本轮新增 `descriptor` / `tasks` 选项） |
| 真机 / 生产库 | **未连接、未读取、未改动**；无 `crates/**`、`proto/**`、`drivers/**`、契约或版本改动 |
| 浏览器自动化 | 仍**无**（无 Playwright/Puppeteer 依赖，且本轮不允许装依赖/联网）。jsdom 无布局引擎：本轮把它**显式断言**出来（`EndpointAcquisitionPane.layout.test.tsx`），而不是假装覆盖（§4.3） |
| 机械判定口径 | Alert 计数、行数、页码、`scrollTop`、`rowId` 锚点、字段锚点 `id` 与外框、请求序列、文案字符串，全部可由断言判定 |

命令（均在工作树内执行，未做任何 git 写操作）：

```
pnpm --dir apps/mesa-web test        # 75 文件 / 825 用例全绿
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
| 5 | **操作失败后能直接找到处理位置** | `issues[0].path` 返回后**自动跳到该行所在页 + 打开抽屉 + 字段高亮**，无需手工搜索；定位不了的要明确说、不得猜 | **通过（本轮实现）**。**可定位**（字段级 `...points[j].parameters.<field>`）：自动跳到该行所在页 + 打开编辑抽屉 + 展开/高亮/滚动到字段（`native-field-<key>` 外框 + `scrollIntoView`），行上带 `data-issue-located="true"`；**不可定位**（任务级 `tasks[i].binding.kind`/`binding`/`binding.config`/`selections`、保留任务 `tasks[其它下标]`、无法识别/越界）：正文写「该问题无法自动定位（原因）」，**页码不动、抽屉不开**，原始 `path`/`code` 只在技术详情 | **通过** | `locates_field_level_issue_to_row_page_and_highlights_field`、`task_level_path_is_reported_as_unlocatable_without_navigation`、`preserved_task_path_is_reported_as_unlocatable_without_navigation`、`unrecognized_path_is_reported_as_unlocatable_without_navigation`；映射层 13 例见 `issueLocation.test.ts` |
| 6 | 随时知道哪些修改没保存 | 计数 = 新增+修改+移除，周期**单独表述**；增删改后同步；新增后移除**抵消**、恢复原值**取消标记** | 摘要 `1 项点位变更（修改 1）` / `1 项点位变更（新增 1）` / `1 项点位变更（移除 1）；周期另有修改：1000 → 500 ms`；撤销后只剩周期项；周期改回原值后保存区整体收起 | **通过** | `full_path_on_238_…`、`summary_separates_point_changes_from_interval_change`、`add_then_remove_cancels_out`、`add_via_drawer_returns_to_same_page_and_scroll` |
| 7 | 知道保存会不会影响采集 | 运行中 + 有修改 ⇒ 中断说明「保存会暂时中断当前连接的采集」；已停止/无修改不出现 | 运行中：`draft-save-note` = 该句；主按钮 `保存并恢复采集`、次级 `保存后保持停止`；切换后 `保存并停止采集` / `保存后恢复采集`；已停止：不出现中断说明（改述「保存只写入配置…」）；无修改时保存区不渲染；全文不含「约 1–2 秒」 | **通过** | `save_labels_follow_run_state_and_restart_choice` |
| 8 | 保存失败保留修改 | 计数不变、保存区仍在、行标记仍在 | PUT 400 + 1 条 issue 后：`data-kind="failed"`，`draft-save-summary` 仍是 `1 项点位变更（修改 1）`，行仍标「已修改」，无成功结果条 | **通过** | `save_failure_keeps_draft_and_row_marker` |
| 9 | 反馈与服务端真实结果一致 | PUT 2xx 但回读不一致 ⇒「待核对」/失败，**不报成功**；NCK 与非 NCK 都回读；成功通知只 1 次 | 回读不一致（点位相同、周期没保存）→ `data-kind="pending-verify"`、无 `apply-result`、基线不刷新；回读请求失败同样；成功后 `apply-result` 恰 1 条且 `.ant-message-notice` = 0 | **通过** | `mismatched_readback_never_repeats_put`、`回读不一致（点位相同、周期没保存）⇒ 待核对…`、`回读请求失败 ⇒ 待核对（不是成功），且不重新 PUT`、`full_path_on_238_…` |
| 10 | 不丢点位、不覆盖其他任务 | 保存仍是全量替换；238 点保存后仍 238；`preservedTasks` 逐字保留 | PUT 体 `tasks[0]` 含 **238** 点（235 原生 + 3 语义），未编辑的 235 点逐字等于加载值；保留任务 `keep-sub-task` 深度相等（含 4 个 subscribe 缺省字段） | **通过** | `full_path_on_238_toasts_nothing_and_saves_full_snapshot` |
| 11 | 不削弱身份与范围校验 | §1.3 六项守卫逐项有测试锚点，重构前后断言不变 | 五处锚点测试**未改一行断言**且全绿（行号为当前工作树实测，替代评审稿 §5.3 的旧行号）：`EndpointAcquisitionPane.nckB.test.tsx:614`「门禁 6：已有点位预览失败 ⇒ 不得按旧 Key 继续提交」（身份与 Key）、`:777`「P1：Endpoint 层最终保存守卫（完整 Schema）」（未知必填字段在任何请求前拦截）、`:319`「门禁 7」+`:392`「门禁 8」（保存与回读）、`:531`「门禁 9」（存量问题点位只能显式移除）、`EndpointAcquisitionPane.ux.test.tsx:405`「P2-2（状态区）」+`:430`「P2-1（已保存状态）」（状态与草稿区分） | **通过** | 上述 5 个 describe 内的全部用例（本轮全绿） |

**第 5 项的实现要点（本轮补，替代上一版的「未通过」说明）**：

1. **不改后端契约**：`issues[].path` 就是「提交快照的 JSON 路径」，现有资源校验路径**已经包含任务、行和字段位置**——因此前端只需要一次「快照下标 → 稳定 rowId」的换算，不需要新的后端字段或端点。用户判断成立：**存在多种 path 形态 ≠ 必须先改后端契约**。
2. **先核实真实形态再写映射**（不是猜）。核实的真实形态（`issueLocation.ts` 顶部逐条引用）：
   - 任务级：`tasks[i].binding.kind`（`crates/core-api/src/binding_gate.rs:60-67`）、`tasks[i].binding`（`:97-102`）、`tasks[i].selections`（`crates/core-types/src/resource.rs:262-268`）、`tasks[i].binding.config`（`crates/core-types/src/point.rs:818-845`）；
   - 点位级（NCK）：`tasks[i].binding.config.points[j]` 及子键 `kind`/`point_key`/`parameters`（`point.rs:854-928`）；
   - 点位级（通用）：`tasks[i].selections[j]` 及子键 `resource_id`/`outputs`/`parameters`（`resource.rs:270-359`）；
   - 字段级：`...parameters.<field>`（`crates/core-types/src/schema.rs:293-301`，`validate_instance` 用 `{root}.{field.key}` 拼路径）。
   - 附带确认（写进映射注释，避免过度承诺）：Core 对 **NCK 点位 `parameters` 完全不透明**（`point.rs:915-928` 只判 JSON 对象），所以真实后端里字段级 path 只会出现在通用资源选择下；NCK 侧按同一 `...points[j].parameters.<field>` 规则处理，前端不做 kind 特判。
3. **映射依据是「本次提交快照」**：保存时固化 `{ taskIndex, taskCount, rowIds[] }`（`EndpointAcquisitionPane.tsx:1306`），被编辑任务下标由计划给出的 `taskId` 回查（`endpointSavePlan.ts:211`）/ 追加顺序（`deviceModel.ts:460`）确定；**快照缺失或下标对不齐时一律不可定位**（不拿实时列表下标去猜）。
4. **不可定位的三类分支**（每类都有独立文案与断言）：任务级 → 「该问题定位到整个任务，无法对应到某个点位」；保留任务 → 「该问题属于保留任务（不在此页编辑），无法定位到点位」；无法识别（含越界/非 `tasks[...]` 前缀/空路径）→ 「该问题的路径无法识别，无法定位到点位」。三类都**不跳转**（页码不动、抽屉不开、无行标记）。
5. **正文与详情分离**：正文 `save-issues-lead`（`EndpointAcquisitionPane.tsx:2218`）只写「N 个问题需要处理，点此查看第一个」或「该问题无法自动定位（原因）」；原始 `path`/`code`/`message` 只在 `save-issues-details`（`:2237`）里；保存区结果文案（`:1416-1421`）也不再拼 `path`。
6. **打开哪个抽屉**：字段级 ⇒ 编辑抽屉（能就地改，复用 `NckNativeFields.tsx:91-118` 的「先展开高级区、再定位」机制 + `DescriptorFields` 的 `highlightKeys` 外框）；行级（或该行不可就地编辑）⇒ 该行「技术详情」抽屉，并在顶部标出「该行存在服务端校验问题（字段：…）」（`:2481-2490`），避免在用户没准备编辑时摊开可编辑表单。
7. **不含驱动特判**：映射模块只解析后端 path 的**契约段**（`tasks[i]` / `binding.config.points[j]` / `selections[j]` / `parameters.<field>`），`points` 与 `selections` 两种写法共用同一实现；文件内无 `driver_id`、无资源 ID、无协议字段名判断（`issueLocation.ts` 全文可 grep 核对）。页面层也没有按驱动分支：`locateIssueRow` 只用「该行是否可编辑」决定开哪个抽屉。

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

### 4.2 必须真实浏览器的部分（jsdom 无布局引擎，无法机械判定）

1. 480px 视口下**抽屉占满宽度后**，内部字段（变量选择器 320px 最小宽、原生字段两列 `minmax(240px,1fr)` 自适应）是否仍然不出现横向滚动条、按钮是否都在可视区。
2. 保存区搭在页面底部时，滚动到底部**分页是否完整可见**（"sticky + 文档流内"保证无永久遮挡，但滚动中间态的视觉重叠需肉眼确认）。
3. 窄屏下行内「⋯」菜单是否被容器裁切（菜单 `top: 36px`、`right: 8px`）。
4. 抽屉遮罩与保存区的视觉层叠顺序（断言只证明 `z-index` 数值关系）。
5. 真实窄屏设备（如 360×640）上的字号与触摸目标大小。

> 说明：§4.1 的 9 条是"组件声明的布局"，§4.2 的 5 条是"浏览器实际排版"。本报告**不**把前者当作后者通过的证据。

### 4.3 本轮补的布局自动化覆盖（新增）与仍缺的部分

用户指出「布局问题不是自动化必然测不到，而是当前测试缺少**真实浏览器布局覆盖**」。核实结论：项目**没有真实浏览器测试环境**（`package.json` 无 Playwright/Puppeteer，本轮不允许装依赖/联网），jsdom 24.1 没有布局引擎。因此本轮做两件事——把**能机械判定的**补上断言，把**测不到的**在报告里点名，不用声明层面的断言冒充真实排版。

**已自动化（`src/components/EndpointAcquisitionPane.layout.test.tsx`，4 例）**

| # | 断言 | 可机械判定的原因 |
|---|---|---|
| L1 | 列表容器声明的 `max-height` 是 `calc(100vh - 360px)`，按视口换算：1080p ⇒ **720px ≥ 600**；720p ⇒ **360px ≥ 300**；`min-height ≥ 240` | 逐项算出具体像素，与阈值比较（不是"看起来够高"） |
| L2 | **分页在滚动容器之外**：`point-list-scroll.contains(point-table-pagination) === false`、`point-table.contains(pagination) === false`，且 `compareDocumentPosition` 为 `FOLLOWING` | DOM 层级关系与滚动位置无关，jsdom 可精确判定 |
| L3 | 列表滚动时分页不跟着滚：`point-list-scroll` 是**唯一** `overflow-y: auto` 的区域，`point-table` 自身不滚动 | 同上（滚动容器归属是结构事实） |
| L4 | 保存区同样在滚动容器之外；一页只渲染 25 行且都在滚动容器内 | 结构 + 计数，可机械判定 |
| L5 | **显式记录限制**：`list.getBoundingClientRect().height === 0`、行高 `0`、`clientHeight === 0` | 用断言把"jsdom 没有布局引擎"钉住：将来若引入真实浏览器，这条会失败，提醒维护者把 L1–L4 升级成真实布局断言，而不是继续以为覆盖了 |

**仍需真实浏览器（本轮**未**自动化，不做任何"已覆盖"声明）**

| # | 项 | 为什么 jsdom 不行 |
|---|---|---|
| R1 | 列表容器**实际渲染高度**是否 > 某阈值（真实溢出/收缩） | `getBoundingClientRect()` 恒为 0，`calc()` 不会被求值 |
| R2 | 480px 视口下抽屉内容横向溢出、右侧按钮可达性 | 需要真实排版与 `scrollWidth > clientWidth` |
| R3 | 保存区与分页/最后一行的**视觉重叠**（滚动中间态） | 需要合成叠加与像素结果 |
| R4 | 行内「⋯」菜单被容器裁切 | 需要 overflow 裁切计算 |
| R5 | 层叠顺序的真实呈现（`z-index` 数值关系已断言，但视觉遮挡不可判） | 需要绘制/层叠上下文 |
| R6 | 字号、触摸目标大小（真实窄屏设备） | 需要字体度量与设备像素比 |

> 建议（本轮不实施，需产品/CI 决策）：引入 Playwright + 一条 1440×900 / 480×800 的冒烟用例，专门覆盖 R1–R5；在此之前，本报告把 R1–R5 视为**人工目视项**，不含自动化证据。

---

## 5. 页面效果描述（便于对照）

### 5.1 首屏（无修改、已停止、238 点、正常夹具）

渲染结构（自上而下，`EndpointAcquisitionPane.tsx:1676-1997`）：

1. **一行运行摘要**（`acquisition-status`）：`采集状态：已停止 · 点位：238 个（已保存） · 周期：1000 ms（已保存）`。
2. **采集周期**：一个数字输入框（`10–60000`，单位 `ms`）+ 一个帮助图标（压测说明只在 Tooltip 里）+ 一行小字「周期为配置值，实际采集表现以运行结果为准」。
3. **列表标题行**：`已配置点位（238）`，右侧一个主按钮 `+ 添加点位`。
4. **搜索行**：输入框（占位「搜索名称、标识或配置内容……」）；**没有筛选项**——没问题行、没修改行时不渲染下拉。
5. **列表**（`point-list-scroll`：`max-height: calc(100vh - 360px)`、`min-height: 240`、`overflow-y: auto`——唯一的滚动区）：三列表头 `名称或标识 / 配置摘要 / 操作`；每行 `名称`（如「轴实际位置」）+ 必要标识、`配置摘要`（如「通道 1 · 轴 1」、「区域 C · 区域号 1 · 模块 122 · 列 1 · 行 1」）、操作 `编辑 · ⋯`。
6. **分页行**（在滚动容器**之外**，`point-table-pagination`）：`共 238 项｜每页 [25][50][100] 项｜< 1 2 3 … 10 >｜第 1 / 10 页`。
7. **无保存区、无 Alert、无教程、无逐行绿标**；若有保留任务，列表下方一行静态说明「另有 N 个任务不在此编辑，保存时原样保留：…」。
8. **事件订阅**：仅当该连接的驱动 `descriptor.events.streams` 非空时，出现独立折叠区域（默认**收起**，`data-testid="event-subscription-region"`）；未声明事件流时**整块不存在**（不再有「该驱动未声明事件流」的常驻卡片）。

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

### 5.4 保存失败（后端 400 + `issues[]`，本轮实现定位）

- 保存区出现**持续**失败条（`save-outcome[data-kind="failed"]`），文案不含原始 path（「点位保存失败（N 项）：已跳到第一个可定位的问题，其余见「技术详情」」）。
- 新增问题块 `save-issues`：
  - 标题 `保存被后端拒绝：N 个问题需要处理`；
  - **正文**（`save-issues-lead`）：`N 个问题需要处理，点此查看第一个（跳到该行所在页并高亮字段）。` 或 `N 个问题需要处理；该问题无法自动定位（原因），请按技术详情里的原始路径排查。`；
  - 可定位时给出按钮 `点此查看第一个`（`locate-first-issue`）；
  - **技术详情**（`save-issues-details`，折叠）：逐条原始 `path · code · message`（`issue-raw`）。
- 定位效果：字段级问题 → 跳到该行所在页 + 打开编辑抽屉 + 字段带红色外框并滚动到中间；行级问题 → 打开该行「技术详情」并在顶部标注「该行存在服务端校验问题（字段：…）」。行上另有 `data-issue-located="true"` 锚点（仅一行）。
- 不可定位（任务级 / 保留任务 / 无法识别）：**不跳页、不开抽屉、不标行**——只在正文说清原因，详情保留原始 path。

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
| 1 | 无浏览器自动化：真实布局、遮挡、字号、触摸目标未做像素级验证 | jsdom 限制（无布局引擎） | §4.1 的 9 条 + §4.3 的 L1–L4（声明/结构层面）已自动化；§4.3 的 R1–R6 明确标注**需真实浏览器**，本轮未覆盖 |
| 2 | 服务端为 fetch 桩：`GET /tasks`、`PUT /tasks`、`stop`、`start`、`identity-preview` 的响应形状由测试构造 | stub | 验证的是**前端逻辑与请求序列**，不是后端行为；后端契约正确性由 Rust 侧 Contract Test 负责 |
| 3 | 238 点夹具是「形状派生」而非快照逐字复制（无真机/无生产库） | 夹具 | 点数、区域分布、字段值与快照同形；逐点业务含义不代表现场 |
| 4 | 回读一致性判定为**逐字相等**，不含任何归一化放宽 | 设计（§4.4.3） | 若后端将来对 schedule 等字段做新归一化，会退化为「待核对」而非误报成功（fail-closed） |
| 5 | 「保存结果待核对 → 核对一致后继续恢复采集」依赖 `pendingVerify` 内存态：页面刷新即丢失 | 现状 | 刷新后用户需重新核对/重新保存（不会自动重复写入） |
| 6 | 无并发原子保证（后端 PUT 无条件写、无 `If-Match`） | 后端限制（§4.4.4） | 两个客户端同时编辑仍可能互相覆盖；本步不消除 |
| 7 | 窄屏阈值 768px 为布局取值（非契约），抽屉/表格共用同一阈值 | 实现细节 | 与 CSS 断点表无联动；调整需同时改 `useViewportWidth.ts:15` |
| 8 | 保存区「约 5s 后自动收起」由定时器驱动，测试只断言"动作区已收起"，未等满 5s | 测试取舍 | 自动消失的时序未做端到端断言 |
| 9 | `apply-result` 成功条在同一会话中只可能出现一条，但历史会话（多次保存）会依次替换 | 设计 | 「成功只出现一次」指单次保存内 |
| 10 | ~~既有 flake~~ **本轮已收敛**：`GlobalDataPage.test.tsx`「质量状态筛选与搜索只影响表格…」用例内部已有多个 10s/30s 等待，默认 5s 超时会在整文件并行时偶发失败 | 既有测试 | 只给该用例加**显式 60s 超时**（`GlobalDataPage.test.tsx:220-222`），**断言一字未改**；全量 75 文件/825 用例连跑通过 |
| 11 | 浏览器前进/后退守卫依赖同 URL 哨兵条目（`history.pushState`） | 实现边界 | 若运行环境禁止 `pushState`，前进/后退退化为无法拦截；侧栏/面包屑/Tab 三条仍受保护（`draftNavGuard.tsx:145-182` 注释已说明） |
| 12 | 事件区域需要额外读一次 `descriptor`（`useDriverEventCatalog`），与 `EventTaskEditor` 内部各读一次 | 实现取舍 | 同一驱动描述被读两次（都走 GET，无写入）；未做跨组件缓存，避免改动订阅流程（§7-8「不改订阅流程」） |

---

## 8. 缺陷记录

### 8.1 原「未修」项：已在第 4 轮实现（转 §8.2-D7）

~~§5.2-5「操作失败后能直接找到处理位置」未实现~~ —— 本轮已实现，见 §2 第 5 项的「实现要点」与 §8.2-D7。上一版认为「必须先定 path 契约」，经核实**该判断过强**：现有资源校验路径本身就带任务/行/字段位置（见 §2 第 5 项第 2 点的逐条引用），前端做一次快照下标换算即可，无需改后端契约。

### 8.2 已修（均带回归测试）

| # | 缺陷 | 影响 | 修复 | 证据 |
|---|---|---|---|---|
| D1 | 抽屉宽度固定 640px | 480px 级窄屏抽屉比视口宽 → 横向溢出、右侧按钮不可达（违反 §4.1-1 / §7-5「窄屏自适应」） | 新增 `useViewportWidth.ts`（阈值 + `fitDrawerWidth` + 视口 hook）；`PointDrawer.tsx:43-51` 按视口收敛宽度，桌面仍为 640px，不做拖拽 | `desktop_keeps_640_and_narrow_follows_viewport`、`pane_drawer_also_adapts_on_narrow_viewport` |
| D2 | 三列表格最小宽度约 586px（`170 + 260 + 132 + 24`） | 480px 视口横向溢出，必须横拖才能点到「编辑/更多」（违反「窄屏下三列表格可用」） | `PointTable.tsx:28-32、69-77`：窄屏改单列堆叠（`minmax(0,1fr)`）、不渲染三列表头、三段带 `aria-label`、行与单元 `minWidth: 0` + `overflowWrap: anywhere` | `stacked_layout_on_narrow_and_columns_on_desktop` |
| D3 | 高级必填缺失时**只展开不定位** | `NckNativeFields.tsx` 的 effect 在折叠态下调用 `getElementById`，此时高级区尚未渲染 ⇒ 永远取不到锚点，`scrollIntoView` 静默失效；用户看到高亮却仍停在表单顶部（违反 §3.1「自动展开并定位到第一个缺失字段」） | `NckNativeFields.tsx`：先展开，再在下一轮渲染（依赖 `advancedOpen`）定位 | `missing_required_advanced_field_expands_and_scrolls_to_anchor` |
| D4 | 行内「更多」菜单 `z-index: 5` 与固定保存区同层 | 同层时后渲染的保存区会盖住最后一行菜单的下半部分，「移除/技术详情」可能点不到 | `PointTable.tsx:190-196`：菜单 `z-index: 20`（仍低于抽屉 1000），并加注释说明原因 | `save_bar_is_in_flow_sticky_below_the_list_and_below_drawers` |
| D5 | 搜索框固定 `width: 360px` | 360px 以下视口会把页面撑出横向滚动条 | `PointSearch.tsx:35`：`maxWidth: "100%"`、`minWidth: 0` | 随窄屏用例渲染生效（`stacked_layout…` 同一挂载） |
| D6 | antd v6 弃用 `Drawer.width`（运行期告警） | 仅告警，无功能影响；下一大版本会移除 | `PointDrawer.tsx:49-50` 改用 `size={resolvedWidth}`（数值语义与 px 等值） | 全量测试无该告警 |
| **D7** | **（第 4 轮）保存失败的 `issues[].path` 无处理入口** | 用户只能看到一坨原始文本，得自己搜索定位；验收第 5 项不通过 | 新增 `issueLocation.ts`（纯映射 + 三类不可定位原因）；`EndpointAcquisitionPane.tsx:1306`（提交快照）、`:1627`（`locateIssueRow`：跳页 + 抽屉 + 字段高亮）、`:2214-2249`（正文/技术详情分层）；`NckNativeFields.tsx:91-118` 复用「展开+定位」并支持外部 `focusFieldKey`；`NckPointFields.tsx:465` 透传 | `issueLocation.test.ts`（13）、`EndpointAcquisitionPane.issueLocate.test.tsx`（4）、`save_failure_keeps_draft_and_row_marker`（已更新为「自动定位 + 草稿不丢」双向断言） |
| **D8** | **（第 4 轮）侧栏/面包屑/浏览器后退静默丢草稿**（用户实测） | 周期改 2000 → 点侧栏「总览」→ 无确认 → 回来变回 1000，改动白做 | 统一守卫 `draftNavGuard.tsx`（Provider + `useGuardedNavigate` + `GuardedLink` + 哨兵/popstate）；接入 `App.tsx:65`、`DeviceHeader.tsx:36`、`DeviceTabNav.tsx:28,41`；`DeviceWorkspacePage` 删除 Tab 专属通道，避免两套判定 | `DraftNavGuard.test.tsx`（5 例，均断言「确认框出现 + 取消后周期仍是 2000」；无草稿不弹框） |
| **D9** | **（第 4 轮）不支持事件的卡片常驻主页**（用户实测） | 不支持事件流的驱动也常驻一块卡片，底部写「该驱动未声明事件流」+ 内部字段说明（违反 §2.5-8 / §7-8） | 判据改 `descriptor.events.streams`（`useDriverEventCatalog.ts:53`，不看 `driver_id`）；空 ⇒ 整块不渲染；非空 ⇒ 折叠区域默认收起（`DeviceAcquisition.tsx:88-107`）；描述读取失败按「真实问题」给说明 + 重试（`:110-124`） | `DeviceAcquisition.eventsRegion.test.tsx`（2）、`DeviceWorkspaceFlat.test.tsx`「驱动未声明事件流 ⇒ 采集页不渲染事件区域」 |

---

## 9. 待确认

1. **字段级定位的展示形态（本轮实现后的取舍，需产品确认）**：字段级 path ⇒ 打开**编辑抽屉**并高亮字段；行级 path（或该行不可就地编辑，如非 NCK 资源选择）⇒ 打开该行「技术详情」并标注问题。若产品希望行级也直接进编辑态，只需改 `EndpointAcquisitionPane.tsx:1644-1655` 的一处分支。
2. **保存失败是否要「自动」跳页**：本轮按 §5.2-5 实现为**自动**定位第一个可定位问题（并保留「点此查看第一个」按钮复跳）。若现场反馈「自动跳走打断排查」，可改为只给按钮、不自动跳（同样一处开关）。
3. **窄屏阈值 768px**：仍为前端布局取值（`useViewportWidth.ts:15`）。若产品有既定断点（或与全局数据页一致），需要统一后我再调整。
4. **订阅型 schedule 的四个缺省字段是否要在前端显式补齐**：现状是「不补、逐字保留、缺失即不一致」。若后端将来改为「缺省字段不落库」，回读会永久不一致（每次都进「待核对」），届时需要重新评审 §4.4.3 的判据。
5. **保存区「预留空间」的实现口径**：仍以「sticky + 位于文档流末尾」实现（`DraftSaveBar.tsx:45-58`），未额外插入空白占位块；真实浏览器目视（§4.3-R3）后再决定是否补 `padding-bottom`。
6. **事件区域是否要缓存 descriptor**：当前 `useDriverEventCatalog` 与 `EventTaskEditor` **各读一次**驱动描述（都是 GET）。若认为多一次请求不可接受，需要把描述提到外壳层共享（会动到事件订阅的数据流，故本轮未做，§7-8「不改订阅流程」）。
7. **浏览器前进/后退守卫的哨兵条目**：会在有草稿期间向 history 多压一条**同 URL** 记录（取消时补回）。若担心历史栈语义，可改 data router + `useBlocker`，代价是 6 个以 `MemoryRouter` 渲染 `<App/>` 的测试文件需要改造（本轮未做，见 §0.1 说明）。

---

## 附录 A：新增/改动文件与用例

### A.1 第 3 步（上一轮，保持不变）

**新增测试（3 文件 / 24 用例）**

| 文件 | 用例数 | 覆盖 |
|---|---|---|
| `apps/mesa-web/src/components/EndpointAcquisitionPane.acceptance.test.tsx` | 7 | §5.2 的 1/2/3/6/7/8/9/10 + 错误恢复（状态未知、保存失败、必填缺失定位） |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.requestOrder.test.tsx` | 12 | §3 的 8 条请求顺序 + 守卫顺序 + 过渡态 |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.narrow.test.tsx` | 5 | 窄屏（抽屉宽度、单列堆叠、保存区层级与 DOM 顺序） |

- `apps/mesa-web/src/test/requestProbe.ts`：请求记录探针（标签化 + 顺序序列 + 计数）。
- `apps/mesa-web/src/useViewportWidth.ts`：视口宽度 hook + 两个纯函数（有单测 `narrow_threshold_and_drawer_fit`）。
- 改动：`PointDrawer.tsx`（窄屏收敛宽度、`size` 替代弃用的 `width`）、`PointTable.tsx`（单列堆叠、菜单层级、可收缩单元）、`PointSearch.tsx`（可收缩）、`NckNativeFields.tsx`（必填缺失时先展开再定位）。

### A.2 第 4 轮（本轮：三处验收遗漏修复 + 布局补强）

**新增测试（5 文件 / 28 用例）**

| 文件 | 用例数 | 覆盖 |
|---|---|---|
| `apps/mesa-web/src/issueLocation.test.ts` | 13 | `issue.path` → `rowId`（+字段）映射：字段级 `points[j].parameters.<f>` / `selections[j].parameters.<f>`；点位级 `points[j]`、`.parameters`、`.point_key`、`selections[j].outputs`；任务级 5 种；保留任务；越界/非 `tasks[...]`/空路径；无快照；`describeUnlocatable` 三类文案；`firstLocatableIssue` 跳过不可定位 |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.issueLocate.test.tsx` | 4 | 字段级 ⇒ 跳页(1→2) + 抽屉打开 + `native-field-module` 高亮 + `scrollIntoView` + 行锚点唯一；任务级 / 保留任务 / 越界 ⇒ 「无法自动定位」且页码不动、抽屉不开、无行标记；原始 path 只在技术详情 |
| `apps/mesa-web/src/workspace/DraftNavGuard.test.tsx` | 5 | 侧栏 / 面包屑 / 设备 Tab / `popstate`（后退）四条路径：先确认、取消后**周期仍是 2000**；无草稿导航**不弹框** |
| `apps/mesa-web/src/workspace/DeviceAcquisition.eventsRegion.test.tsx` | 2 | `events.streams` 为空 ⇒ 区域与「未声明事件流」文案都查不到；非空 ⇒ 折叠区域存在、默认收起（`aria-expanded=false`、编辑器未渲染），展开后出现自己的保存按钮 |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.layout.test.tsx` | 4 | L1 列表高度按视口换算 ≥ 阈值；L2/L3 分页在滚动容器**之外**且容器是唯一滚动区；L4 保存区也在滚动容器外 + 一页 25 行；L5 显式记录 jsdom 无布局引擎（`getBoundingClientRect` 恒 0） |

**更新测试（不减少用例、不放宽断言）**

| 文件 | 变化 | 原因 |
|---|---|---|
| `apps/mesa-web/src/components/EndpointAcquisitionPane.acceptance.test.tsx` | 「验收 8：保存失败保留修改」改为**更强**的断言：原来只断言 issues 文本 + 行标记（在同一页），现增加「自动跳到第 2 类页位/抽屉打开/字段高亮/行锚点/正文不含原始 path」，并在关闭抽屉后回第 10 页继续断言行标记 | 第 5 项实现后，保存失败会自动定位；原断言依赖「页面不动」，必须随之更新（断言只增不减） |
| `apps/mesa-web/src/workspace/DeviceWorkspaceFlat.test.tsx` | 「采集页含两个 Section」改为用**声明了事件流**的 descriptor 桩；新增 1 例「未声明事件流 ⇒ 不渲染事件区域」 | 事件区域改为按能力出现（D9 回归项） |
| `apps/mesa-web/src/pages/GlobalDataPage.test.tsx` | 给既有 flake 用例加显式 60s 超时 | §7-10 建议的小项；**断言未改** |

**新增实现文件**

- `apps/mesa-web/src/issueLocation.ts`：`path` → `rowId`/字段的纯映射（含三类不可定位原因与文案）。
- `apps/mesa-web/src/workspace/draftNavGuard.tsx`：应用内离开守卫（Provider / `useGuardedNavigate` / `GuardedLink` / `useBrowserBackGuard`）。
- `apps/mesa-web/src/workspace/useDriverEventCatalog.ts`：事件能力目录（唯一判据 `descriptor.events.streams`）。

**改动实现文件（仅 `apps/mesa-web/src`）**

- `App.tsx`（守卫 Provider + `AppShell`；侧栏点击走守卫）
- `workspace/DeviceHeader.tsx`（面包屑 `GuardedLink`）
- `workspace/DeviceTabNav.tsx`（Tab 走同一守卫；删除专属 `beforeNavigate` 通道）
- `workspace/DeviceWorkspacePage.tsx`（移除 Tab 专属草稿状态，交给统一守卫）
- `workspace/DeviceAcquisition.tsx`（上报草稿给守卫；事件区域按能力出现/隐藏）
- `components/EndpointAcquisitionPane.tsx`（提交行快照、`locateIssueRow`、问题正文/详情分层、行锚点、抽屉字段定位、详情抽屉问题标注）
- `components/NckNativeFields.tsx`（`focusFieldKey`：展开 + 高亮 + 有限次重试定位）
- `components/NckPointFields.tsx`（透传 `focusField`）
- `test/pointLiveStub.ts`（新增 `descriptor` / `tasks` 选项）

**测试总量**：`70 文件 / 796 用例` → **`75 文件 / 825 用例`**（+5 文件 / +29 用例；无删除、无跳过、无放宽）。

**命令结果（本轮实跑）**

| 命令 | 结果 |
|---|---|
| `pnpm --dir apps/mesa-web test` | **75 文件 / 825 用例全部通过**（含本轮新增 28 例 + 更新后的验收 8、扁平设备详情 +1 例） |
| `pnpm --dir apps/mesa-web typecheck` | `tsc --noEmit` 通过（无输出） |
| `pnpm --dir apps/mesa-web lint` | `eslint .` 通过（无输出） |
| `pnpm --dir apps/mesa-web build` | `tsc -b && vite build` 通过（仅既有 chunk >500kB 告警） |
| git 写操作 | **无**（提交由父代理执行） |

---

# 第 5 轮（P1 连续后退丢草稿 / P2 通用资源字段定位）

> 本轮起点 `HEAD = 7c860c5`（PR #146，分支 `feat/acquisition-page-refactor`）。改动仅限 `apps/mesa-web/**` 与本文件；**未**改 `proto/`、`crates/**`、`drivers/**`、契约、版本，未连真机、未读写生产库，未做任何 git 写操作。

## 10. P1：连续后退仍丢草稿，且地址与页面内容不一致

### 10.1 先核实的根因（实际行为，不是推断）

| # | 事实（代码位置） | 后果 |
|---|---|---|
| 1 | 本项目是 `<BrowserRouter>` + `<Routes>` 声明式路由（`main.tsx:66`、`App.tsx:74-92`）。`popstate` 时 `BrowserRouter` 在 `useLayoutEffect` 订阅的 `history.listen` 里**先**更新 `location`（`react-router-dom@6.30.6 dist/index.js:634`；`@remix-run/router@1.23.4 dist/router.js:350-362` 的 `handlePop`） | 只要 URL 真的变了，采集页**已经**被卸载 |
| 2 | 草稿本来是采集页组件的 `useState`（`EndpointAcquisitionPane.tsx:325` `intervalMs` 等） | 卸载即丢，事后没有任何"取消"能把草稿变回来 |
| 3 | 旧实现 `draftNavGuard.tsx:169`（本轮起点版本）：`if (window.location.href !== href) return;` —— **URL 一变就直接放行** | 第二次后退（穿透哨兵、真正离开采集页 URL）完全不拦；同一次交互里第一次弹的确认框还开着 |
| 4 | 旧实现取消分支只有 `history.pushState(..., href)` | 只改地址栏，**不通知路由** ⇒ 地址栏是采集页、路由内容停在概览页（用户实测到的"地址与内容不一致"） |
| 5 | 旧实现的监听挂在 `[dirty]` 上，且"待确认"没有任何状态 | 待确认期间再后退不会被中和（只防第一次） |

复现链（与用户实测逐字对应）：改周期 2000 → 后退（落在哨兵/锚点，弹确认）→ 不选 → 再后退（旧实现走到第 3 条：放行 ⇒ 路由切概览、采集页卸载、草稿丢）→ 点「继续编辑」（旧实现走到第 4 条：只推 URL ⇒ 地址采集页/内容概览页）。

### 10.2 采用的方案：锚点条目 + 同 URL 哨兵（每次 popstate 逐个中和）+ 路由级 repair + 草稿副本

`apps/mesa-web/src/workspace/draftNavGuard.tsx`（`useBrowserBackGuard`，`:225-381`）现在的语义：

1. **武装**：草稿出现时记下「锚点」（采集页条目 URL + 路由落点），并在其后压一条**同 URL 哨兵**；
2. **每一次** `popstate`（不论第几次）：
   - 落点 URL == 锚点 URL ⇒ 说明只落在锚点/哨兵上（路由没变、组件没卸载）：**先补回哨兵**再询问 ⇒ 连续后退每一次都只落到同一 URL，逐个被中和，不会穿透；
   - 落点 URL != 锚点 URL（浏览器把连点合并成一次跨步遍历）⇒ 记下用户原本想去的位置，**当场 repair**：用路由自己的 `navigate(anchorRoute, { replace: true })` 把**地址与内容一起**拉回采集页（不用裸 `pushState`——那正是"地址是采集页、内容是概览页"的成因），再补哨兵；
   - 待确认期间（`asking`）不弹第二个框；
3. **取消**：常路径下什么都没被动过（URL/内容/草稿都在原位），只需幂等复核；跨步遍历时已在第 2 步当场中和；
4. **确认**：当前条目 = 哨兵、锚点在它前一条 ⇒ `history.go(-2)` **一次**落到用户原本想去的位置（只走一步，不多跳不漏跳）；跨步遍历过则改用记下的 `targetPath` 导航一次。哨兵没压上（环境禁 `pushState`）时退化为 `go(-1)`；若那次后退根本不可能发生（采集页就是历史第一条），400ms 兜底把守卫恢复成"仍武装"，**不会**把之后的后退永久放行；
5. **纵深防御**：草稿内容另有副本（`apps/mesa-web/src/draftStash.tsx`，`DraftStashProvider` 挂在 `DraftGuardProvider` 内、包住整个路由内容）——卸载前写入、重新挂载并加载完服务端快照后取回，恢复成**未保存草稿**（基线仍是服务端真值，差异判定照常把它算成"有修改"）。因此即使某次中间态真的把采集页卸载了，草稿也不会消失。

**为什么这套能防住连续后退**：`popstate` 的落点被"哨兵"约束在同一 URL 上，而每一个 `popstate` 都会**同步**补回哨兵，所以第 N 次后退与第 1 次等价（都落在锚点 URL，都被询问）；跨步遍历（一次后退多格）由"落点 URL != 锚点 URL"识别并当场中和，因此"漏掉的第二次"在两种情形下都被覆盖。**没有**改成 data router（`useBlocker`）：那会牵动 6 个以 `MemoryRouter` 渲染 `<App/>` 的既有测试文件，收益不抵代价。

### 10.3 P1 测试：从"合成事件"变成"真实历史跳转"

新增 `apps/mesa-web/src/workspace/DraftNavGuard.history.test.tsx`（6 例，`BrowserRouter` + 真实 `window.history.back()`；jsdom 会真移动条目并异步派发 `popstate`）：

| 用例 | 断言（地址 / 内容 / 草稿 三者） |
|---|---|
| `real_back_asks_and_keeps_draft_when_cancelled` | 后退 ⇒ 弹确认（`pops == [采集页]`）；点「继续编辑」⇒ `location.pathname` 仍是采集页、**页面内容是采集页**、周期仍是 2000；**取消后再后退一次仍会弹框**（哨兵已补回） |
| `every_back_during_pending_confirmation_is_neutralised` | 待确认期间**连续后退三次**：`pops == [采集页, 采集页, 采集页]`（三次真实跳转全被逐次中和）、页面仍是采集页、只有 1 个确认框、周期仍未 2000 |
| `user_reported_sequence_back_twice_then_continue_editing_keeps_draft` | 用户实测顺序：后退 → 不选 → 再后退 → 点「继续编辑」⇒ 地址、内容都是采集页，周期 2000，且保存区仍在（不是靠"重新加载"骗过断言） |
| `confirmed_leave_navigates_exactly_once_to_the_previous_page` | 后退 → 确认 ⇒ `pops == [采集页, 概览页]`（**只跳一次**，不多跳不漏跳），`location.pathname` 是概览页且页面内容是概览页 |
| `cross_step_traversal_is_neutralised_immediately_and_keeps_draft` | 用 `history.go(-2)` 驱动"连点被合并成跨步遍历"的真实跳转（jsdom 会把同 tick 的多次 `back()` 折叠，无法用连点复现）：落点先到概览页 ⇒ **立即**被中和回采集页、内容也是采集页、周期 2000；取消后仍一致 |
| `confirmed_leave_after_cross_step_traversal_goes_to_the_recorded_target` | 跨步遍历后确认 ⇒ 一次导航落到记下的目标（概览页），内容一致 |

另有 `apps/mesa-web/src/components/EndpointAcquisitionPane.draftStash.test.tsx`（2 例）用**真实卸载再挂载**（换 key 强制重建，等价于路由把采集页卸载掉）证明纵深防御：有未保存修改 ⇒ 重挂载后周期仍是 2000、保存区仍在、`onDraftStateChange` 重新上报 `dirty: true`；无修改 ⇒ 不留残留（回到服务端值 1000、无保存区）。

保留 `DraftNavGuard.test.tsx`（5 例，`MemoryRouter` + 侧栏/面包屑/Tab/合成 `popstate`）不动：它证明四条**应用内**路径的确认文案与"取消后草稿仍在"，与真实历史跳转用例互补（断言只增不减）。

### 10.4 只能人工验证的部分（如实说明）

- **真实浏览器的历史栈语义**：jsdom 的 `history` 是内存实现，`back()` 为异步任务、同 tick 多次 `back()` 会被折叠，因此"连点两次后退"在真实浏览器里是否会被合并成一次多步遍历、合并后落点如何，**只能在真实浏览器确认**（本轮的代码路径已用 `go(-2)` 等价驱动并自动化）。
- **地址栏瞬时可见性**：跨步遍历时地址栏会极短暂地显示目标地址（React 18 批处理下通常不会完成一次提交，但这是渲染时序，jsdom 无法观测）。
- **iOS 上 `pushState` 的次数限制**（history API 的已知平台限制）与"确认后 `go(-2)` 是否一定产生 `popstate`"的实际时序。
- 上述之外的部分（URL 与路由内容一致性、草稿保留、只跳一次、跨步中和）**全部已自动化**。

## 11. P2：通用资源的字段错误不能定位到可编辑字段

### 11.1 改在哪

| 文件 | 改动 |
|---|---|
| `components/EndpointAcquisitionPane.tsx` | `locateIssueRow`（`:1783-1831`）不再写死「只有 NCK + fieldKey 才开编辑抽屉」：改为 `canLocateFieldInPlace(rowId, fieldKey)` —— NCK 保持既有行为，通用资源按**该行资源 Schema** 判定；可编辑 ⇒ 打开**编辑表单**并把字段交给表单高亮，不可编辑 ⇒ 打开**只读技术详情**并写明原因（`data-in-place-editable="false"`）；新增 `editingSelIndex` 状态、`openEditSelectionDrawer`、`replaceSelection`（编辑不改 rowId）；通用行也有「编辑」入口（`editableRowIds` 按 Schema 判定，不看驱动/资源名）；正文文案区分「高亮字段 / 打开技术详情（该字段无法就地编辑）」 |
| `components/ResourcePickerAntd.tsx` | 新增可选 `initialSelection`（就地编辑：按该行预填资源+参数+输出，主按钮「确认修改」）与 `focusFieldKey`（高级区先展开 → 高亮 → 有限次重试滚动）；`DescriptorFields` 加 `idPrefix="selection-field"`、`highlightKeys`、受控 `advancedOpen`；编辑态的 `dirty` 改为「与预填值比对」，打开表单本身不算修改（否则关抽屉会无端弹「放弃未确认的修改？」） |
| `components/fieldFocus.ts`（新增） | 共用定位动作 `locateFieldAnchor` / `useFieldAnchorFocus`：折叠区/抽屉里的字段晚一次渲染才存在 ⇒ 有限次（20×16ms）重试，拿不到就放弃（不猜元素、不抛错） |
| `components/NckNativeFields.tsx` | 原 `:91-118` 的重试定位逻辑改为调用 `fieldFocus.locateFieldAnchor` —— **同一套机制**，NCK 与通用表单不再各写一遍 |
| `pointRows.ts` | `createRowIdAllocator(prefix, startAt)`（`:37`）+ `maxRowIdSeq(ids, prefix)`（`:53`）：恢复草稿副本后分配器续号，新增行不会撞上已恢复的 rowId |

### 11.2 字段 key 怎么对上

1. `issueLocation.fieldKeyOf` 只认 `...parameters.<field>`（`issueLocation.ts:173-178`），`<field>` 就是 Core `validate_instance` 拼路径时用的 `field.key`（`crates/core-types/src/schema.rs:293-301`，只引用不改）；
2. 页面层拿 `<field>` 去**该行资源**的 `parameters.fields` 里查：存在 ⇒ 该字段有对应控件；`DescriptorFields` 的字段锚点 id 就是 `selection-field-<field.key>`（`DescriptorFields.tsx:469-475`，`highlightKeys` 见 `:452`），高亮集合也是同一 key ⇒ **同一个 key 贯穿 path → 判定 → 锚点 → 高亮**；
3. NCK 侧沿用既有 `native-field-<key>` 锚点与「展开高级区再定位」的顺序。

### 11.3 不可就地编辑时（如实说明，不假装高亮）

判据只有两条，全部来自 Descriptor，**没有** `driver_id` / 协议字段名 / 资源 ID 特判：① 该行资源在 `desc.resources` 里存在；② 该资源参数 Schema **没有界面不认识的字段类型**（`unsupportedFields` 为空，A3 口径）。任一不满足，或 `<field>` 不在 Schema 里：

- 打开**只读技术详情**，顶部写「该行存在服务端校验问题（字段：X）· 该字段无法就地编辑（资源参数 Schema 里没有这个字段，或含界面不支持的类型），请按下面的原始配置排查」；
- 属性 `data-in-place-editable="false"` 可供机械断言；
- 正文（`save-issues-lead`）同步改口为「打开技术详情（该字段无法就地编辑）」，**不说**"高亮字段"；
- 断言里显式检查：编辑表单没打开、没有任何 `selection-field-*` 锚点被滚动。

### 11.4 P2 测试

新增 `apps/mesa-web/src/components/EndpointAcquisitionPane.issueLocateGeneral.test.tsx`（6 例，通用资源 `mesa.resources.v1` 主路径）：

| 用例 | 断言 |
|---|---|
| `generic_field_level_issue_opens_the_edit_form_and_highlights_the_field` | `tasks[1].selections[27].parameters.count` ⇒ 跳页（1→2）+ 抽屉标题「编辑点位」+ `resource-select` 在（**不是**只读详情）+ 表单按该行预填（`#selection-field-count input` 值 = 27）+ `selection-field-count` 外框 + `scrollIntoView` 命中该锚点 + 行锚点唯一 + 正文不含原始 path |
| `generic_advanced_field_is_expanded_before_highlighting` | `parameters.decode`（`ui.advanced`）⇒ **先展开高级区**，锚点随后存在且高亮（与 NCK 同一顺序） |
| `generic_field_missing_from_schema_opens_read_only_detail_and_says_so` | `parameters.not_in_schema` ⇒ 只读技术详情 + `data-in-place-editable="false"` + 「无法就地编辑」文案 + 编辑表单未打开 + 无字段锚点被滚动 + 正文改口 |
| `task_level_path_is_still_unlocatable` | 任务级 ⇒ 「无法自动定位」，页码不动、无定位按钮、不开抽屉、无行标记 |
| `row_level_path_opens_read_only_detail_not_the_edit_form` | 行级（`selections[0].resource_id`）⇒ 技术详情、不摊开编辑表单（保持既有行为） |
| `preserved_task_path_is_still_unlocatable` | 保留任务 ⇒ 不跳转、不开抽屉 |

既有 NCK 字段路径用例（`EndpointAcquisitionPane.issueLocate.test.tsx` 4 例）**未改一行**并保持全绿。

## 12. 本轮命令结果（实跑）

| 命令 | 结果 |
|---|---|
| `pnpm --dir apps/mesa-web test` | **78 文件 / 841 用例全部通过**（起点 75 / 825；+3 文件 / +16 用例，无删除、无跳过、无放宽） |
| `pnpm --dir apps/mesa-web typecheck` | `tsc --noEmit` 通过（无输出） |
| `pnpm --dir apps/mesa-web lint` | `eslint .` 通过（无输出） |
| `pnpm --dir apps/mesa-web build` | `tsc -b && vite build` 通过（仅既有 chunk >500kB 告警） |

新增用例清单：`DraftNavGuard.history.test.tsx`（6）、`EndpointAcquisitionPane.draftStash.test.tsx`（2）、`EndpointAcquisitionPane.issueLocateGeneral.test.tsx`（6）、`pointRows.test.ts` 的新增 2 例（`maxRowIdSeq` 只认 `<prefix>-<n>`；`createRowIdAllocator` 带 `startAt` 续号）。

## 13. 本轮已知限制 / 待确认

1. **真实浏览器历史栈**、**地址栏瞬时可见性**、**iOS `pushState` 次数限制**：只能在真实浏览器确认（见 §10.4）。
2. **采集页是历史第一条**（深链接/新标签页直达）时，确认后无处可退：守卫会恢复成"仍武装"（不会把后续后退永久放行），但**不会**替用户离开站点——浏览器也没有可去之处。
3. **哨兵条目的历史栈副作用**（既有）：有草稿期间会多一条同 URL 记录；用户放弃/保存后该条目仍在，后退会先经过它（视觉无变化）。要彻底消除需要 data router + `useBlocker`（代价见 §10.2）。
4. **行级问题**仍打开只读技术详情（保持既有行为）；若产品希望行级也直接进编辑态，只需改 `locateIssueRow` 一处分支。
5. **通用资源的就地编辑**是本轮新增能力（此前通用行只有「添加/移除」）：`replaceSelection` 只改草稿，不发写请求；保存仍走同一 `planEndpointTaskSave` 全量替换路径，`rowId` 不变。
