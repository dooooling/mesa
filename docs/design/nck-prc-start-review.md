# PR-C 实施启动审查（I-2 已落地后）

- 审查对象：`docs/design/nck-prc-frontend-semantic-config-boundary.md`（1362 行，冻结）＋ I-2 实际实现 ＋ `apps/mesa-web` 现状。
- 审查基线：`main` = `2261178130ac686f6e14a9b879044b36e58979ef`（PR #114，I-2 已合并）；工作树内容与该 commit 树一致（`git diff HEAD 2261178 --stat` 为空）。
- 性质：**只读核查**。本文只给「实施启动时要改什么、按什么断言验收」，**不是第二份设计文档**；不新增验收编号（一律复用既有 A1–A44），只把 `scope=identity` 相关三条（A13/A40/A43）合并提升为 **P0-1**。
- 所有事实给 `文件:行号`；凭代码推断而非直接断言的，标「（推断）」或**待确认**。

## 0. 结论速览（给评审）

1. **I-2 不是「按设计原样实现」**：路由与字段名对得上，但响应是 **10 字段分页信封 + 每条第 14 字段白名单投影**，设计里「`r1.json` 内容原样」这句**已不成立**（§2.5 D2/D3）。
2. **I-2 额外提供了权威预览端点**，而 PR-C 设计只把它写成「待核对的能力缺口」（§1.4.2 的「首选方案」、§1.5 的 (a)）。⇒ A39 必须从「首选/降级二选一」收紧为「**唯一路径**」，前端模板替换 (b) 不再允许作为可提交来源（§2.5 D7）。
3. **Browse 的 `preview_point_key` 不是权威身份**：它只调 `canonical_identity`（`presets.rs:249-253`），没有走 addressing→原生解析；权威预览走完整 D1–D10（`compiler.rs:406-465`）。⇒ 「未修改 target/arguments 时可逐字复用预览 Key」这条（§1.4.2 第 1 行 / A2）**必须改为「提交前一律调服务端 Preview」**。
4. **`scope=identity` 的 6 条今天确实可被 Browse 当采集项暴露**（`presets.rs` 全文无 `scope` 引用；`compiler.rs` 亦不校验 scope）。I-2 目录如实返回全部 42 条（`definition_api.rs:18-22`），**准入判定明确划给前端**。⇒ P0-1 的 6 条断言必须可执行，且必须**双重执行**（UI 层 + 提交模型层）。
5. **前端仍然零引用** `nck_variable` / `nck_native` / `nck.points.v1` / `preview_point_key`（grep 实测），双真值仍在原处：`deviceModel.ts:210/301` 与 `bootstrap.ts:138`。
6. **C10 处置不变**：`variable` 资源面逐字未动（I-2 对 `drivers/sinumerik-nck/src/lib.rs` 只**新增** 32 行 RPC 适配，见 `git diff e250600 2261178`），但**设计文档里引用该文件的行号已整体下移**（§2.5 D13）。

## 1. 主线 1：I-2 API ↔ 前端类型契约

### 1.1 目录端点（真实）

- 路由：`GET /api/v1/drivers/{id}/definitions`（`crates/core-api/src/lib.rs:3251-3254`）；**driver 作用域**，无 Endpoint、无连接、无持久化副作用。
- Core 只做信封形状判定（正整数/互斥/夹取）后把参数组装成 JSON **逐字**转发（`crates/core-api/src/driver_query.rs:57-118`，`:106-110`）；驱动返回体**零 serde 往返**直接回填（`:113-115`，`:257-259`）。
- 响应顶层 **10 字段**（`drivers/sinumerik-nck/src/definition_api.rs:260-282`）：

| 字段 | 语义 | 证据 |
|---|---|---|
| `format_version` | 定义数据格式版本（当前 1） | `definition_api.rs:261-264`；`definition.rs:41` |
| `family` | 请求给了就回显；未给且只发布一个系列则报该系列；多系列则为 `null` | `:243-250`, `:265-268` |
| `revision` | **文件级** revision（装载期强制同文件同 revision）；空目录回落最小值 | `:252-258`, `:269` |
| `driver_id` | `sinumerik-nck` | `:270` |
| `driver_version` | 编译期 `CARGO_PKG_VERSION` | `:271-274` |
| `note` | 仅 E9（已声明但无定义）非 null，用于区分「空目录」与「驱动故障」 | `:275`, `:519-525` |
| `total` | **过滤后、分页前**的总数（跨页恒定） | `:276`, `:227-232` 邻域 |
| `limit` | 实际生效页大小（夹取后的值） | `:277` |
| `next_cursor` | 无下一页为 `null` | `:278-281` |
| `definitions` | 当前页条目数组 | `:282` |

- 条目是 **14 字段白名单投影**（`definition_api.rs:56-71`，投影实现 `:534-558`）：
  `variable_id, revision, family, scope, semantics_status, label, source, targets, target_required, arguments, identity_template, identity_keys, golden_ref, digest`。
  **丢弃**定义数据中的 5 个原生字段：`addressing, decode, transport_size, count, unit_mode`（`definitions/828d/r1.json` 条目实测含这些键）；**新增**装载期计算的 `digest`（`definition.rs:247`）。

### 1.2 分页 / 查询严格性

| 项 | 真实契约 | 证据 |
|---|---|---|
| `limit` 缺省 | **100** | `definition_api.rs:37`；`driver_query.rs:35` |
| `limit` 上限 | **500**，超出**夹取**（不是错误） | `definition_api.rs:39, 411-413`；`driver_query.rs:37, 66` |
| `limit` 非法 | `0` / 负数 / 小数 / 字符串 ⇒ 400 `INVALID_QUERY` | `definition_api.rs:468-483` |
| `total` | 过滤后、分页前；翻页请求仍返回同一总数 | `:276`；测试 `drivers/sinumerik-nck/tests/definition_api.rs:243` |
| `next_cursor` | `null` ⇒ 末页；否则**逐字回传**，客户端不得解析/构造 | `:227-232`, `:298-347` |
| 游标编码 | `v1-<variable_id>-<revision>`（复合排序键；`variable_id` 不含 `-`） | `:316-347` |
| 游标非法/旧式/锚点不存在 | 400 `INVALID_QUERY`（**不静默兼容**旧式「仅 variable_id」游标） | `:354-384`, `:198-209` |
| `cursor` × `variable_id` | **互斥** ⇒ 400 | `:436-440`；`driver_query.rs:83-85` |
| `variable_id` 与 `revision` | **必须成对**出现（拒绝「全目录过滤 revision」与「缺省回落最新 revision」） | `:418-433`；`driver_query.rs:73-80` |
| 未知查询键 | 400（仅允许 `family/variable_id/revision/limit/cursor`） | `:42`, `:400-406` |
| 未声明 family | 501 `UNSUPPORTED_FAMILY`（**勘误：设计写 400，实现按 kind 落 501**） | `:186-194`, `:108-118` |
| 已声明但无定义 | **200 + `definitions: []` + `note`** | `:519-525` |

> 前端必须按**契约**翻页，不能依赖「今天 42 < 100 所以一页」：一旦新增定义或 `limit` 被显式传小，单页假设立刻失效。断言 `total` 恒为过滤后总数，不代表本页条数。

### 1.3 权威身份预览端点（真实，设计里没有）

- 路由：`POST /api/v1/drivers/{id}/definitions/identity-preview`（`crates/core-api/src/lib.rs:3255-3266`）。
- **64 KiB body 门只挂在本路由**（`lib.rs:3260-3262`；`driver_query.rs:32`），框架级 413 被改写成 `{error:{code:"REQUEST_TOO_LARGE", kind, message}}`（`driver_query.rs:196-211`）。
- 请求顶层**恰 3 键**：`{family, kind, parameters}`（`driver_query.rs:146-165`；`definition_api.rs:45, 583-597`）。
  `family` **必填无缺省**（`:598-605`）；`kind` 非空字符串（`:606-609`）；`parameters` 必须对象且与将来持久化的 `PointSelection.parameters` **逐字同构**（`:560-563, 610-612`）；`point_key` 传进来即 400（`:587`）。
- 计算链与 Configure **共用** D1–D10 + 身份重算，**只跳过 D11**（持久化 Key 比对）：`compiler.rs:350-386, 393-401, 406-465`。
  - `nck_native` + `identity_mode="user"` **拒绝**（Driver 不重算、不静默改名）：`compiler.rs:371-380`（400 `INVALID_BINDING_CONFIG`）。
  - 未知 Point Kind ⇒ 501 `UNSUPPORTED_POINT_KIND`（`compiler.rs:344-346, 382-384` + `:185-187`）。
- 响应 6 字段（`definition_api.rs:649-660`）：`point_key`（恒非空）、`identity_mode`、`scope`、`variable_id`、`definition_revision`、`definition_digest`。
  - 语义点位→有定义坐标；原生点位→后四项全 `null`（`:623-647`）。
  - **`scope` 是每条定义的原始 scope，不是准入结论**：`compile_semantic_checked` 全程不校验 scope（`compiler.rs:406-465` 无 scope 分支）⇒ 对 `scope="identity"` 的变量，预览**照样返回合法 Key**。这正是 P0-1 第 6 条断言要钉死的事实。

### 1.4 错误结构与 HTTP 映射

- 统一信封 `{error:{code, kind, message}}`（`driver_query.rs:217-230`）；形状错误同形（`:242-254`）；`kind` 为 `ErrorKind` 的 `as_str()`。
- **状态码只由 `ErrorKind` 决定**（`driver_query.rs:179-190`，唯一映射点）：`Configuration→400`、`Address→404`、`Unsupported→501`、`Timeout→504`、`Connection|Device|Protocol|Decode|Internal→503`。

| HTTP | code（来源） |
|---|---|
| 400 | `INVALID_QUERY`、`INVALID_REQUEST`（Core 形状层，`driver_query.rs:233-240`）；`INVALID_BINDING_CONFIG`、`MISSING_TARGET`、`UNSUPPORTED_TARGET`、`INVALID_TARGET`、`INVALID_ARGUMENTS`、`MISSING_IDENTITY_KEY`、`UNSUPPORTED_DEFINITION_REVISION`、`INVALID_USER_IDENTITY`、`POINT_KEY_MISMATCH`（编译器 `Configuration`，`compiler.rs:162-194`） |
| 404 | `UNKNOWN_VARIABLE`、`INVALID_VARIABLE`（`Address`，`compiler.rs:188-190`） |
| 501 | `UNSUPPORTED_FAMILY`、`UNSUPPORTED_POINT_KIND`、`IDENTITY_NAMESPACE_COLLISION`（`Unsupported`）；`DEFINITIONS_UNSUPPORTED`（Manager，`manager.rs:150-159`） |
| 503 | `DEFINITION_DIGEST_MISMATCH`、`DEFINITION_RESPONSE_INVALID`（`Internal`）；`DRIVER_UNAVAILABLE`、`DEFINITION_QUERY_BUSY`、`DEFINITION_QUERY_TIMEOUT`、`DEFINITION_QUERY_TOO_LARGE`（`manager.rs:80-144`） |
| 413 | `REQUEST_TOO_LARGE`（`driver_query.rs:196-211`） |

> 说明：上表「HTTP」列是由 `kind` + 映射函数**推导**（代码里没有按码特判的清单，这是 P9 护栏要求的）。两条 I-2 路由今天**没有任何路径产生 `Timeout`**（预算耗尽故意取 `Internal`，`manager.rs:100-121`），故 504 在这两条路由上不可达，但前端共用客户端仍需映射。
> `POINT_KEY_MISMATCH` 只可能来自 Configure，**不会**来自预览（D11 被跳过）——前端不得为预览写该码的分支。

### 1.5 设计假设 vs 实际实现（差异表）

| # | PR-C 设计里的假设（含行号） | I-2 实际 | 判定 |
|---|---|---|---|
| D1 | 输入 `GET /api/v1/drivers/{driver_id}/definitions?family=828d`（§1.4 I-2，:259） | 同路径（`core-api/src/lib.rs:3251-3254`） | **一致** |
| D2 | 输出 = `r1.json` 内容**原样**（:260） | Driver 侧 14 字段**白名单投影**，丢 5 个原生字段、加 `digest`（`definition_api.rs:56-71, 534-558`） | **需修正** |
| D3 | 条目 13 字段（:260），无 `family`/`digest` | 14 字段（`:56-71`） | **需修正（补 2 字段）** |
| D4 | 单页一次性返回，无分页 | `total/limit/next_cursor` + 复合游标（`:276-281, 298-347`） | **缺失（必须补）** |
| D5 | 未提 `driver_id`/`driver_version`/`note` | 三者均在信封内（`:270-275`） | **缺失（类型要建全）** |
| D6 | I-2「不提供 Preview，需核对能力缺口」（§10.1 实施分层②；§1.4.2 把权威预览写成首选但无接口） | 已有 `POST .../identity-preview`，与 Configure 共用全链（`compiler.rs:350-386`） | **需修正（能力缺口已关闭）** |
| D7 | 修改 target/arguments 后：首选权威预览，**降级**允许前端模板替换 (b)（§1.5:351，A39 ④） | 权威预览可用 ⇒ 不存在「不可得」的常态 | **需修正：删掉 (b) 作为可提交路径** |
| D8 | 预览请求形状未定义 | 恰 3 键、`family` 必填、`point_key` 禁止（`driver_query.rs:146-165`） | **需修正（契约补齐）** |
| D9 | 预览响应形状未定义 | 6 字段（`definition_api.rs:649-660`） | **需修正** |
| D10 | 错误 = `code` + `message`（§6.3:929 只列了 7 个保存期码） | 信封含 `kind`；新增 10+ 个 I-2 码（§1.4） | **需修正（类型与文案表）** |
| D11 | 「未修改 target/arguments ⇒ 逐字复用 `preview_point_key`」（§1.4.2 第 1 行；A2） | Browse 预览只算 `canonical_identity`，**未**走 addressing/原生解析（`presets.rs:249-253`） | **需修正（见下）** |
| D12 | §4.1「变量目录主来源 = Browse，`scope` 不可得」（:670-671） | `scope` 现可从 I-2 目录取得；但 Browse **仍不按 scope 过滤**（`presets.rs` 无 scope 引用） | **需修正（目录为主、Browse 为树/预填）** |
| D13 | 引用 `drivers/sinumerik-nck/src/lib.rs:185-337`、`:648-657` 等（§5.1/§5.2） | I-2 后行号下移：`variable` 资源 **186-341**、configure 拒绝 **680-688** | **需修正（引用漂移）** |
| D14 | `arguments[].value_type` 只写 `integer`（§6.1:901） | `ArgType = Integer｜Enum`（`definition.rs:99-105`），但 **`Enum` 无 `options` 字段**；今天 42 条中仅 2 条有 arguments，均为 integer/required | **待确认（见 §8）** |
| D15 | 42 条 = 36 collect + 6 identity；24 verified / 16 parameter_identity / 2 candidate（§3.1:618） | 实测同值（`r1.json` 统计；断言 `tests/definition_api.rs:425-426`） | **一致** |

**D11 的后果（主线 2 的前提）**：Browse 的 `preview_point_key` 是「用已确认字段算出的 canonical identity」，**不等于**「该实例能通过 Configure」。反例路径：`channel=255` 类实例能过 Target 校验、但原生解析失败——`compiler.rs:350-357` 的注释正是为此写的。因此：

> **A2 必须改写**：「未修改 target/arguments 时，提交的 `point_key` 逐字等于 `identity.preview_point_key`」→
> **「提交前一律调 `POST .../identity-preview`；`identity.preview_point_key` 只作初始展示值。两者不一致时以 Preview 为准并提示，禁止直接提交 Browse 预览值。」**

### 1.6 「基于 I-2 未落地假设、现在必须修正」清单（按优先级）

1. **A39 改口径**：删除「降级 (b) 前端模板替换可提交」，改为「唯一权威来源 = `identity-preview`」；A41（共享 Golden 只证明编码规则）保留，但**不得**再作为提交依据。
2. **A2 改写**（见上）。
3. **A43 前置条件收紧**：`targets/target_required/arguments[]/scope` 从目录取；「元数据完整」的定义改为「I-2 目录响应含这 4 项且本页/跨页取全」。
4. **A40 的 `scope` 来源**改为 I-2 目录（Browse 不含 scope）；补一条：**预览返回 `scope=identity` 时同样禁止提交**（不能因为「Key 合法」就放行）。
5. **A42 适用范围缩小**：目录不可用时仍 fail-closed；目录可用时不得再用 `identity.keys` 推断。
6. **§1.5 第 5 行**（「若 I-2 未落地 → 显示未提供」）保留为降级分支，但**默认路径不再走它**。
7. **前端类型契约**：新增 `DefinitionsResponse`（10 字段）、`VariableDefinition`（14 字段）、`IdentityPreviewResponse`（6 字段）、`ApiErrorEnvelope`（`error.{code,kind,message}`）。
8. **§2.1 新增文件条件解除**：`nckCatalog.ts` 从「仅当 I-2 落地」变为**必做**。
9. **行号引用**：设计文档所有 `drivers/sinumerik-nck/src/lib.rs` / `core-api/src/lib.rs` 行号需按 I-2 后重标（本报告所有引用已是新行号）。
10. **§7.8.1 覆盖映射**中「`scope` 缺失 ⇒ A40」仍需保留，但要补「`scope` 可得后的准入判定」这一正面路径。

## 2. 主线 2：身份 Preview 调用流程（含竞态）

### 2.1 硬性要求（写成可验收条款）

| # | 要求 | 验收落点 |
|---|---|---|
| R1 | **修改 Target / Arguments 的那一刻**，`point_key` 立即置为 `null`（或「校验中」），`加入` 立即 disabled；**旧 Key 不得继续显示为有效** | A39 反例① |
| R2 | 每次修改后**向服务端重新计算**：`POST /api/v1/drivers/{id}/definitions/identity-preview`，body `{family, kind:"nck_variable", parameters:{variable_id, definition_revision, target, arguments}}`（与待提交 `PointSelection.parameters` **同构**） | A39 正例；A2 改写后 |
| R3 | **只接受「当前输入对应」的最新响应**：请求带单调序号 N；响应仅在 `N === latestSeq` **且** 输入指纹（`JSON.stringify({variable_id,definition_revision,target,arguments})` 的稳定化结果）**等于**当前指纹时被采纳；否则**整份丢弃**（不改 Key、不改状态、不报成功） | A39 新增竞态子用例 |
| R4 | 丢弃/失败/中止 ⇒ 保持 disabled 且显示原因；**不得**回落到 Browse 预览值或前端模板渲染值 | A39 ③④；A15 |
| R5 | `family` 取自 I-2 目录响应的 `family`；为 `null`（多系列）时 **fail-closed**（禁止提交并提示） | A40；§8 待确认 |
| R6 | `nck_native` + `identity_mode="user"` **不调预览**（必被 400 拒），走「用户自定 Key + 冲突拒绝」路径 | A25/A26 |

实现建议（不新增文件，符合 §2.1 的文件范围）：纯判定放 `nckPointModel.ts`（`previewFingerprint(parameters)`、`shouldAcceptPreview(seq, latestSeq, fp, currentFp)`）；调用与状态机放 `NckPointFields.tsx`；`AbortController` 只用于**省流量**，**不得**把「已 abort」当作唯一防线（响应可能已 resolve）。

### 2.2 竞态测试设计（可执行）

**位置**：`apps/mesa-web/src/components/NckPointFields.test.tsx`（组件级，`globalThis.fetch` 用 deferred promise 桩，写法参照 `ResourceBrowseAntd.test.tsx:33-48`）。
**同仓先例**：`apps/mesa-web/src/connection/useConnectionSetup.test.ts:179-203` 已用「gate 放行迟到响应 + 断言整份丢弃」测过同类竞态，直接复用该模式。

用例 `stale_preview_response_never_overwrites_latest_identity`：

1. 桩 fetch 对 `identity-preview` 返回两个**手工控制**的 promise：`A`（target `{axis:1}`）、`B`（target `{axis:2}`），Key 分别 `K_A`、`K_B`。
2. 渲染表单，选变量 → 输入 `axis=1`（触发 A）；**在同一事件循环内**改成 `axis=2`（触发 B，A 的 `AbortController` 被 abort，但桩**故意**让 A 仍 resolve）。
3. 先 resolve **B**（`point_key=K_B`）；`await` 至 UI 出现 `K_B`。
4. 再 resolve **A**（`point_key=K_A`）；`await` 一个 tick。
5. **断言**：① 界面仍显示 `K_B`，`K_A` **从未出现**（`queryByText(K_A)` 为 null）；② 提交回调收到的 `point_key` 为 `K_B`；③ 只有一处「已由服务端校验」标记；④ 请求次数为 2（证明没把 A 当成功重试）。
6. 反向再跑一次（先 resolve A 后 resolve B）——断言**只**接受 B（序号更大），A 被丢弃。
7. 附加用例 `editing_target_invalidates_key_synchronously`：输入 `axis=2` 后**不等任何响应**立即断言 Key 区非 `K_B`、`加入` disabled、payload 中无旧 Key。
8. 附加用例 `preview_failure_keeps_submit_disabled`：A/B 都 reject（或 503）⇒ 保持 disabled，错误文案含 `code`，**不得**回填 Browse 预览值。

**断言要点一句话**：竞态的判定标准不是「有没有 abort」，而是「**最终展示与最终提交的 Key，必须对应最后一次输入**」，且过期响应在任何 UI 文本上**不可见**。

## 3. 主线 3：Task Binding 最终落库形态

### 3.1 硬约束（不变，且不得被前端"优化"）

| 约束 | 证据 |
|---|---|
| 任务层 kind **恒为** `nck.points.v1` | `crates/core-types/src/point.rs:82`；`binding_gate.rs:30` |
| 点位层 kind 恒为 `nck_variable` / `nck_native`；Core **不建白名单** | `point.rs:105-107`；`binding_gate.rs:26-30` |
| 信封**恰 3 字段** `{kind, point_key, parameters}`，多一字段即 `UNKNOWN_FIELD` | `point.rs:103-112, 860-868, 934-946` |
| `point_key` 只做通用格式校验（不判身份正确性） | `point.rs:405-433` |
| Endpoint 内**跨 Task、跨 kind** 唯一 | `point.rs:954-976`；`binding_gate.rs:112-154` |
| Driver 是唯一身份真值：`configure` 重算并逐字比对，不一致整体失败 | `compiler.rs:551-570`；`drivers/sinumerik-nck/src/lib.rs:680-688` |
| 旧 `mesa.resources.v1` 在 NCK 上一律 `UNSUPPORTED_BINDING`（不迁移） | `lib.rs:680-688`；`lib.rs:1531-1545` |

⇒ **前端不得生成第二套身份真值**（禁止 `suggestPointKey` 用于语义点位、禁止自由文本改 Key），**不得退回** `mesa.resources.v1` 入口。

### 3.2 前端实际构造 payload 的位置

| 位置 | 行为 | 判定 |
|---|---|---|
| `apps/mesa-web/src/deviceModel.ts:280-304`（`:301`） | `mergeAcquisitionTasks` 组装 `{kind: CANONICAL_RESOURCES_KIND, config:{selections}}` | **装配真值 1** |
| `apps/mesa-web/src/deviceModel.ts:210` | `CANONICAL_RESOURCES_KIND = "mesa.resources.v1"` | 常量 |
| `apps/mesa-web/src/addDevice/bootstrap.ts:131-140`（`:138`） | **硬编码字面量** `kind: "mesa.resources.v1"`；文件只 import 类型（`:7`） | **装配真值 2** |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx:41-51` | `suggestPointKey = "${resource}.${output}"`（冲突 `.2/.3`） | **身份真值 3（前端自造）** |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx:224-232` | `point_key` 自由文本可改 | 同上 |
| `apps/mesa-web/src/components/ResourcePickerAntd.tsx:152-154, 282` | 回填只带 `resource_id/parameters`（**不带 identity**）；`onAdd` 透传 `outputs[].point_key` | 语义叶在此断链 |
| `apps/mesa-web/src/components/EndpointAcquisitionPane.tsx:123` | 调用 `mergeAcquisitionTasks` 后 `PUT /api/v1/tasks/{endpointId}`（`:146-150`） | 端点页保存路径 |
| `apps/mesa-web/src/resourceSelectionModel.ts:329-376` | `applySelectionAdd` 合并/追加（**冻结**） | 只改输入，不改裁决 |

### 3.3 必须改掉的双真值点

1. **T-1（收敛）**：抽 `assembleBinding(selections, driverId)` 于 `deviceModel.ts`（单入口），`bootstrap.ts` 改为 import 它；`bootstrap.ts` 中不得再出现 `"mesa.resources.v1"` 字面量（A28）。判据：两处产出**逐字相等**。
2. **T-2（kind 判定）**：kind 由「选中的资源所属 Driver」决定，NCK ⇒ `nck.points.v1`，其它 ⇒ `mesa.resources.v1`（A19/A29）。**不得**由调用方传字面量。
3. **T-3（识别两种 kind，数据安全关键）**：`splitAcquisitionTasks`（`deviceModel.ts:235-245`）与 `selectionsOf`（`:248-252`）**只认** `CANONICAL_RESOURCES_KIND`。今天若某 NCK Endpoint 已有 `nck.points.v1` 任务，它会被判为 **preserved**，用户再加点位时 `mergeAcquisitionTasks` 会**再追加一个 `mesa.resources.v1` 任务** ⇒ NCK `configure` 见非法 kind ⇒ **整体拒绝，连原有正确任务也不能启动**。⇒ 必须同时识别两种 kind，并把「已存在的 NCK 任务」作为可编辑基底。
4. **T-4（删除第二套身份）**：语义点位**不得**经过 `suggestPointKey` 与自由文本 Key；`point_key` 只来自 `identity-preview`，只读展示。`resource_id`/`outputs[]` 形态**仅**保留给 `nck_native` 与其它驱动。
5. **T-5（payload 形状）**：语义选择 → 恰一个 `PointSelection`：`{kind:"nck_variable", point_key:<preview>, parameters:{variable_id, definition_revision, target, arguments}}`，装进 `{kind:"nck.points.v1", config:{points:[…]}}`；`identity` 块**不得**进入 `parameters`（`presets.rs:166-169`；`point.rs:860-868` 会以 `UNKNOWN_FIELD` 拒绝）。

## 4. P0-1（`scope=identity`，第一条 P0）

> **定位**：本项是 **PR-C 前端采集配置的准入保护**，**不是**修复 Driver 端的 `scope` 缺陷。
> Driver 侧缺陷（`presets.rs`/目录层不按 scope 过滤）**仍 HOLD**，归属独立后端 PR（设计 §10.2，建议 PR-B2）。
> 复用的既有验收编号：**A13 / A40 / A43**（不再新增编号，本项把三者合并为首条 P0）。

### 4.1 可执行断言（逐条照录）

| 测试情形 | 必须得到的结果 |
|---|---|
| 42 条目录正常返回 | 只有 **36 条 `collect`** 可以成为采集候选 |
| 六条 `identity` 定义 | **不进入可采集列表** |
| 强行选中 `identity` 定义 | **前端仍拒绝加入** |
| 绕过 UI 直接构造提交 | **前端提交模型拒绝** |
| `scope` 缺失或未知 | **Fail-closed** |
| Preview 能返回合法 Key | **不能覆盖 `scope` 准入限制** |

### 4.2 实现判据（供 T1/T2/T9 落地）

- 唯一准入函数：`isCollectable(def) === (def.scope === "collect")`；**只认白名单值**，`undefined`/`null`/`"Identity"`/拼写变体/未来新值一律 `false`（**未知即拒**，不得 `!== "identity"` 反写）。
- 准入在**两处**执行，缺一不可：① UI（列表过滤 + 选中态禁用 + 原因文案）；② **提交模型**（`buildPointSelection` 内部再查一次 `scope`，不信任 UI 传入的 `collectable` 标志）。
- 预览结果**只**决定 `point_key`，**不**参与准入；`scope=identity` 即使预览成功也必须拒绝（代码依据：`compiler.rs:406-465` 无 scope 分支，`definition_api.rs:20-22` 明确把准入划给前端）。
- 目录不可用/超时/501 ⇒ 全部不可采集（不缓存旧的「可采集」结论）。

### 4.3 测试文件与用例名建议

| 用例名 | 文件 | 覆盖断言 |
|---|---|---|
| `scope_identity_definitions_never_enter_collect_list` | `apps/mesa-web/src/nckPointModel.test.ts` | 第 1、2 条（42 条 fixture：36 collect + 6 identity ⇒ 列表长度 36，6 个 id 均不在内） |
| `forced_identity_selection_is_rejected_in_ui` | `apps/mesa-web/src/components/NckPointFields.test.tsx` | 第 3 条（构造「已选中 identity 变量」的状态，点「加入」⇒ 不调用 onAdd，出现拒绝文案） |
| `bypass_ui_payload_is_rejected_by_submit_model` | `apps/mesa-web/src/nckPointModel.test.ts` | 第 4 条（直接以 identity 定义构造 payload ⇒ 返回错误对象，**不抛异常**，错误码含 `SCOPE_NOT_COLLECTABLE`（前端自定义码，不冒充后端码）） |
| `missing_or_unknown_scope_fails_closed` | `apps/mesa-web/src/nckPointModel.test.ts` | 第 5 条（`scope` 缺键 / `null` / `"IDENTITY"` / `"future_value"` 四组输入全拒） |
| `preview_success_never_overrides_scope_admission` | `apps/mesa-web/src/nckPointModel.test.ts` + `NckPointFields.test.tsx` | 第 6 条（桩 preview 返回 `{point_key:"channel.program.status@channel=1", scope:"identity"}` ⇒ 仍然拒绝提交，Key 不落库） |

> fixture 用**合成目录**（自造 id），**不得**把 `definitions/828d/r1.json` 的 6 个真实 id 抄进前端测试（那等于把 Driver 数据搬进前端，违反 §1.6 红线；真实 42/36/6 已由 `drivers/sinumerik-nck/tests/definition_api.rs:404-438` 锁定）。

## 5. PR-C 最小实施任务清单

> 只列可执行工作项；不改设计、不新增验收编号。依赖列写「前置任务」。

| # | 工作项 | 涉及文件 | 对应验收 | 依赖 | 完成判据 |
|---|---|---|---|---|---|
| **T1** | I-2 只读客户端与类型 | 新增 `src/nckCatalog.ts`、`src/nckCatalog.test.ts` | A40、A42 前置 | — | 10 字段信封 / 14 字段条目 / 6 字段预览 / `{code,kind,message}` 全部建型；翻页按 `next_cursor` 逐字回传；42 条取全；`family` 为 null 时 fail-closed |
| **T2** | 语义纯函数层 | 新增 `src/nckPointModel.ts`、`.test.ts` | A2–A7、A13、A14、A18、A20、A39、A40、A42、A43 | T1 | `PointSelection` 恰 3 字段；`scope` 准入 + 提交模型二次拒绝；preview 指纹/序号判定纯函数；原生 Key 固定规则；**零** import React/AntD |
| **T3** | Preview 调用与竞态 | `nckPointModel.ts`（判定）、`components/NckPointFields.tsx`（状态机） | A39（含竞态子用例）、A15 | T2 | 改 target/args ⇒ Key 立即失效、`加入` disabled；只采纳最新响应；失败/丢弃保持 disabled；`nck_native+user` 不调预览 |
| **T4** | Browse 透出 `identity` | `src/browseModel.ts`、`browseModel.test.ts` | A1、A16、A17、A44 | — | `BrowseSelection.identity?` 可选；`identity` **不**进 `parameters`；非 NCK 叶逐字不变（A44 两类回归） |
| **T5** | Picker 按 Point Kind 分流 | `components/ResourcePickerAntd.tsx`、`.test.tsx` | A8、A19、A21、A22、A23、A25 | T2、T4 | 语义模式不用 12 原生 schema 判必填；语义叶路由到语义表单（不再 `message.warning` 后放弃）；旧 `variable` 页显式标「高级原生配置，将被驱动拒绝」；**新增 props 必须可选**（`AddDeviceFlow.tsx:254-257` 为 PR-D 冻结，不得被迫改动） |
| **T6** | Browse 组件的语义叶 | `components/ResourceBrowseAntd.tsx`、`.test.tsx` | A1、A16 | T4 | 语义叶「选用」→ 语义 Selection（带 `identity`）；缺 `identity`/预览 null 显示原因且不给「选用」（复用 `:102` 的单点判定） |
| **T7** | 装配收敛（C10-4/C10-6） | `deviceModel.ts`、`deviceModel.test.ts`、`addDevice/bootstrap.ts`、`bootstrap.test.ts` | A11、A12、A19、A28、A29、A36、A37 | — | `bootstrap.ts` 无 `"mesa.resources.v1"` 字面量；两处产出逐字相等；NCK 恒 `nck.points.v1`；双 kind 识别；preserved 语义不退化 |
| **T8** | 端点页面接线 | `components/EndpointAcquisitionPane.tsx`、`.saveIssues.test.tsx` | A12、A37 | T1、T5、T7 | 向 Picker 传 `driverId`/`family`（来自目录）；语义点位入库走 `nck.points.v1`；快照门与 issues 展示不退化 |
| **T9** | 语义表单组件 | 新增 `components/NckPointFields.tsx`、`.test.tsx` | A6、A9、A10、A13、A15、A20、A23、A24、A26、A30、A31、A32、A39、A40、A42、A43 | T2、T3 | Target 按 `targets/target_required` 动态生成（**零**硬编码组合）；Arguments 按 `arguments[]`；Key 只读；`scope` 准入与原因文案；未 blur 值不丢（复用 `DescriptorFields` 的 `onDraftText` 先例） |
| **T10** | 门禁 | — | G1–G7、G11 | 全部 | `pnpm test` ≥383 且全绿；`typecheck` 零错误；`lint` 零错误；`src/connection/**` 134 不降；grep 红线门（A35/A38）；`pnpm build` 成功（G11，不得用 G1/G2 替代） |

**不在本清单**：任何 `crates/**`、`drivers/**`、`proto/**` 改动；`AddDeviceFlow.tsx` / `AddDeviceFlow.test.tsx`（PR-D 独占，设计 §8.3）；Drive 端 `scope` 过滤修复（HOLD）。

## 6. 文件清单与测试基线

### 6.1 改 / 冻结 / 新建

| 类别 | 文件 |
|---|---|
| **新建** | `src/nckPointModel.ts`(+test)、`src/components/NckPointFields.tsx`(+test)、`src/nckCatalog.ts`(+test)（I-2 已落地 ⇒ **必做**） |
| **修改** | `src/browseModel.ts`(+test)、`src/resourceSelectionModel.ts`（**仅** `:20-25` 扩类型，`:169-377` 冻结）、`src/deviceModel.ts`(+test)、`src/addDevice/bootstrap.ts`(+test)、`src/components/ResourcePickerAntd.tsx`(+test)、`src/components/ResourceBrowseAntd.tsx`(+test)、`src/components/EndpointAcquisitionPane.tsx`(+`.saveIssues.test.tsx`) |
| **冻结（不得改语义）** | `src/components/DescriptorFields.tsx`、`src/components/ApplyWithRestart.tsx`、`src/connection/**`（9 文件）、`resourceSelectionModel.ts:169-377`、`src/addDevice/AddDeviceFlow.tsx`(+test) |
| **零改动** | `crates/**`、`drivers/**`、`proto/**`、`apps/mesad/**`、`tests/driver-contract/**`、`tsconfig.json`、`vite.config.ts`（Golden 只在测试中用） |

### 6.2 测试基线（**静态计数，未运行**）

| 项 | 实测 | 方法 |
|---|---|---|
| 测试文件 | **33** | `Get-ChildItem apps/mesa-web/src -Recurse -Include *.test.ts,*.test.tsx` |
| 用例 | **383** | 行首 `it(`/`test(` = **380**；其中 3 处与 `describe` 同行（`connectionSetupModel.test.ts:251`、`:334`、`deviceModel.test.ts:155`）⇒ 380+3 |
| 宽松匹配旁证 | 387 = 383 + 4 处正则 `.test(`（`ResourceBrowseAntd.test.tsx:81`，`useConnectionSetup.test.ts:86/90/95`） | 两条路径同值 |
| `src/connection/**` | 3 个 test 文件 / **134** 用例（132 行首 + 2 同行） | 同法 |
| 关键单文件 | `browseModel` 8、`deviceModel` **72**、`bootstrap` 10、`ResourcePickerAntd` 16、`ResourceBrowseAntd` 2、`EndpointAcquisitionPane.saveIssues` 2、`DescriptorFields*` 4+3+7 | 同法 |

> **与设计文档的差异**：设计 §2.2 写 `deviceModel.test.ts`「现有 **71** 用例（含 2 个与 describe 同行的 it(」；实测为 **72**（71 行首 + **1** 处同行 `:155`），且 §9.1 的 383 合计只有把该文件记为 72 才成立。归类为文档笔误（见 §8-1），**不影响** 33/383 基线。

## 7. C10 遗留（旧 `variable` 资源入口）在 I-2 落地后的处置变化

| 项 | 结论 | 证据 |
|---|---|---|
| `variable` 资源面是否仍逐字未动 | **是**。I-2 对 `drivers/sinumerik-nck/src/lib.rs` 的改动**只有** 32 行新增（`mod definition_api`、两个 `Driver` 方法、错误转换、测试），资源声明与 `resource_selection_methods` 未被触及 | `git diff e250600 2261178 -- drivers/sinumerik-nck/src/lib.rs`；资源体 `lib.rs:186-341`，methods `:340-342` |
| Descriptor 是否变化 | **否**（`contract_minor` 与字段集未变） | 同上 diff 无相关 hunk |
| `configure` 是否仍拒绝旧路径 | **是**，`UNSUPPORTED_BINDING` | `lib.rs:680-688`；测试 `:1531-1545` |
| Browse 叶 `identity` 块现状 | **未变**：仍由 `semantic_binding` 旁挂 `{template, keys, preview_point_key}`，且预览只算「身份」不算「可配置性」 | `presets.rs:242-275`，`:249-253` |
| **处置是否变化** | **方案不变**（UI 侧收敛 + Descriptor 逐字不动），但**依据变了**：① 现在可拿到 `scope` ⇒ C10-1 的「哪些不该进普通采集界面」有了权威判据，不再只能靠「缺 scope ⇒ 全禁用」；② 但 Browse **仍按 42 条暴露 6 条 identity** ⇒ **前端过滤仍是必需的**，不能因为 I-2 落地就认为 Driver 已修好 | §4；`presets.rs` 无 scope 引用 |
| 新增的 C10 风险 | `splitAcquisitionTasks` 只认一种 kind 导致的「追加第二个任务 ⇒ 整体 configure 失败」（§3.3 T-3），**必须**在 C10-4 一并修 | `deviceModel.ts:235-245, 280-304` |

## 8. 无法确认 / 新发现的矛盾（如实登记）

1. **`deviceModel.test.ts` 用例数 71 vs 72**：设计 §2.2 与实测差 1（我计 72：71 行首 + `:155` 同行）；设计 §9.1 的 383 合计却要求 72。判为文档笔误，**基线数字仍取 383**。
2. **`ArgType::Enum` 无 `options` 字段**（`definition.rs:99-105`）：当前 42 条中 arguments 只有 2 条、均为 `integer`（`channel.r_parameter.index 0..65534`、`channel.machine_axis_assignment.position 0..65535`，均 `required=true`）。若将来发布 `enum` 型 argument，前端**无法渲染**（无候选集）⇒ 后台需补 `options`，或前端对该值 **fail-closed**。**待确认**由谁补。
3. **Browse 预览与权威预览可能分叉**：`presets.rs:249-253` 只调 `canonical_identity`，而权威预览走 addressing→`NativeNckAddress`→`resolve_native`（`compiler.rs:406-465`）。理论上存在「Browse 给非 null Key、Configure/Preview 必然失败」的实例；**本轮未逐条枚举 42 条验证是否真存在**（需构造 Drive 矩阵，属验证工作）⇒ **待确认**，但结论不依赖它：提交前一律调 Preview 即可消除风险。
4. **`family` 来源**：预览要求 `family` 必填（`definition_api.rs:598-605`），而目录的 `family` 在「多系列且请求未指定」时为 `null`（`:243-250`，NOTE 明写该组合契约未定义）。当前只有 `828d` 一个已发布系列（`r1.json` 顶部 `family`），但 `DECLARED_FAMILIES` 有两个（`lib.rs:74`）。⇒ 前端取「目录 `family`」还是「Endpoint 连接配置里的 `family`」**需评审定**（本报告建议前者，null 即 fail-closed；后者需读 `list_endpoints` 的 `connection.family`，已被脱敏但 family 非 secret）。
5. **`note` 何时非 null**：仅「已声明但无定义」；今天 828d 有 42 条，故恒 `null`。前端不要据此判断驱动健康。
6. **`REQUEST_TOO_LARGE`（413）不在设计 §6.3 的错误码清单内**，但预览路由确实会产生（64 KiB 门）。前端错误表需补，且**不得**把它当作 400 的别名。
7. **预览对 `nck_native`+`user` 恒 400**：这意味着「用户自定义 Native Key」**没有**任何服务端确认路径（只有 Configure 的语法/命名空间复核，`compiler.rs:512-521`）。A25/A26 的断言只能覆盖前端语法与冲突，**不能**声称「已验证」。这是既有裁决的必然结果，非新缺陷。
8. **设计文档行号整体漂移**（`drivers/sinumerik-nck/src/lib.rs` +1 / +31；`core-api/src/lib.rs` 路由表新增 2 条）⇒ 后续引用一律以本报告的行号为准，或在 PR-C 开工时统一重标。

## 9. 实际运行过的命令（全部只读）

```
git fetch --all --prune                       # 拉到 origin/main = 2261178
git rev-parse HEAD / main                     # 10bcb80 / e250600（本地 main 落后，未更新）
git diff --stat HEAD 2261178                  # 空 ⇒ 工作树内容与目标 main 一致
git branch docs/nck-prc-start-review 2261178  # 仅建引用（未 checkout）
git status --porcelain  /  git log -1 main
git diff --stat e250600 2261178               # I-2 改动面
git diff e250600 2261178 -- drivers/sinumerik-nck/src/lib.rs
Get-ChildItem / Get-Content / Select-String   # 静态计数、行号定位、r1.json 统计
```

- **未运行**：`cargo build/test`、`pnpm test`、`pnpm typecheck`、`pnpm lint`、`pnpm build`、`mesad`、Vite、任何安装。
- **未连接**任何真实设备 / 机床；**未使用**任何「连接真实依赖 Store 的临时副本」。

## 10. 声明

- 本轮**只新建** `docs/design/nck-prc-start-review.md` 一个文件；**未修改**任何代码 / 测试 / 配置 / 其它文档（含已冻结设计文档）。
- **未执行** commit / push / checkout / stash / reset / PR / tag；**未启动** `mesad` / Vite；**未连接**设备；**未做安装**；**未运行**测试与构建。
- 唯一 git 写动作是按要求创建分支引用 `docs/nck-prc-start-review` → `2261178`（未切换工作树、未改任何 ref 之外的对象）；提交与推送由上级执行。
- 文中所有 `文件:行号` 均按 `2261178` 读取；推断性结论已标注「（推断）」，未证实项已列入 §8。
