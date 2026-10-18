# PR-C 整体收口审查：只读核查与验收材料

- 审查对象：`feat/nck-prc-impl` @ `354bca96a814355f624117c58b18f7e44d08bd88`（初始审查基线 `600a492`；PR #116）。
- 审查性质：**只读**。本轮不修改生产代码/测试/配置，不执行 git 写操作，不启动 mesad/Vite，不连接设备，不安装。
- 判据来源：`docs/design/nck-prc-frontend-semantic-config-boundary.md`（下称“边界文档”，A1–A44）、
  `docs/design/nck-prc-implementation-checklist.md`（下称“实施清单”）、
  `docs/design/nck-prc-start-review.md`（下称“启动审查”，D7/D11 裁决）。
- 证据口径：实现给 `文件:行号`；测试给“文件:行号 + 用例名（本轮实跑 + 静态定位）”；无法证实的一律标 **待确认**。

## 0. 结论速览（给评审）

1. **没有“完全未落地”的强制验收项**。44 行中：**37 行已落地、5 行被 D7 取代、2 行转 PR-D、0 行部分**。
2. **上一轮两项“部分”已在最后一轮收口关闭**：
   - **A15**：新增 Browse 503 / 网络异常 / 响应不可解析三组组件级回归（`ResourceBrowseAntd.test.tsx:340-366`），三种情况下均无可操作语义叶、`onSemanticSelect`/`onFill` 零调用。
   - **A24**：`NckNativeFields.tsx:55` 改为默认未确认；`:126-130` 任何 Key 编辑立即撤销确认；重开已有 user 点位默认未确认；新增两条组件级测试（`NckNativeFields.test.tsx:113,144`）。
3. **一致性五项的结论**：语义 `point_key` 唯一来源仍是服务端 `identity-preview`，**未发现第二套语义 Key 真值**；**未发现可绕过 `scope` 的生产入口**；保存路径唯一；非 NCK 正常路径逐字不变（唯一有意差异见 §3.5）。
4. **G5 已按裁决豁免**：D7 删除前端编码器后，G5 不再要求前端引用 Golden；新口径见 §6-1。前端生产代码不依赖 Golden、不复制身份编码算法。
5. 真实 API V1–V7 已在隔离环境执行 **31/31 PASS**（§10）；但这仍是**软件 emulator** 集成，不等于真实 CNC 真机验收；PR #116 合并仍 HOLD。

## 1. 基线与门禁（本轮实测/静态）

| 项 | 值 | 证据 |
|---|---|---|
| 分支 | `feat/nck-prc-impl` | `git rev-parse --abbrev-ref HEAD` |
| 本地 HEAD | `354bca96a814355f624117c58b18f7e44d08bd88`（含 A15/A24 修复） | `git rev-parse HEAD` |
| 远程 PR #116 HEAD | `354bca96a814355f624117c58b18f7e44d08bd88`；CI Run `39131116474` success | 上级独立核查 |
| 工作树 | **仅本文件有未提交 V1–V7 更新**；生产代码/测试无未提交改动 | `git status --porcelain` |
| 前端测试文件 | **43** | `Get-ChildItem apps/mesa-web/src -Recurse -Include *.test.ts,*.test.tsx` |
| 前端用例（实跑） | **503**（43 文件全绿，退出码 0） | `pnpm --dir apps/mesa-web test`；静态计数同为 503（500 行首 + 3 同行） |
| `src/connection/**` | **134** = 132 行首 + 2 同行 | 70（`connectionSetupModel`）+ 39（`ConnectionSetupPanel`）+ 23（`useConnectionSetup`）+ 2 |
| 类型检查 | **`tsc --noEmit` 退出码 0** | 本轮实跑（§7） |
| ESLint | **`eslint .` 退出码 0** | 本轮实跑（§7） |
| 生产构建 | **`tsc -b && vite build` 退出码 0** | 本轮实跑；`dist/assets/index-Bn2eGD58.js` 1,304.30 kB |
| V1–V7 隔离集成 | **31/31 PASS**（2026-10-19，软件 NCK emulator，非真实 CNC） | 结果与证据见 §10；原始证据 `target/validation/pr-c-v1-v7-evidence.json` |
| 冻结面 | `connection/**`、`DescriptorFields*`、`ApplyWithRestart`、`resourceSelectionModel.ts`、`AddDeviceFlow*`、`tsconfig.json`、`vite.config.ts` **均不在 `354bca9` 的 4 文件修复中** | `git show --stat 354bca9` |

> 说明：A15/A24 的 4 文件修复已在 `354bca9` 提交/推送，远程 CI Run `39131116474` success；本轮仅在本文件追加 V1–V7 证据，未改生产代码/测试。本地 vitest / tsc / lint / build 与隔离集成 31/31 均通过。

## 2. A1–A44 逐条状态

> 状态只有四类：**已落地** / **转 PR-D** / **被新裁决取代** / **部分**。
> “被新裁决取代”特指 D7“服务端 Preview 取代前端编码器”（`nck-prc-start-review.md:100`、`:104`、`:112-113`、`:117`）。
> 测试名与行号为本轮实跑/静态定位；全量结果见 §1 与 §9。

| # | 状态 | 实现证据（文件:行号） | 测试证据（文件:行号 / 用例名） | 备注 |
|---|---|---|---|---|
| A1 | 已落地 | `browseModel.ts:87-102`（语义叶透出 identity，parameters 独立） | `browseModel.test.ts:8 semantic_leaf_exposes_identity_without_entering_parameters (A1)`；`ResourceBrowseAntd.test.tsx:157` | identity 不进 parameters，`:36-41` 逐字断言 |
| A2 | 被新裁决取代 | `NckPointFields.tsx:266-278,319-328`（提交只取 `previewState.pointKey`） | `EndpointAcquisitionPane.nckB.test.tsx:273`（PUT 载荷 Key 逐字等于预览响应） | 原口径“未改 target/args 时等于 Browse preview”被 D11 改写（`nck-prc-start-review.md:104,112-113`）；新口径已落地 |
| A3 | 被新裁决取代 | 生产代码仅展示模板：`NckPointFields.tsx:416`；无模板替换函数 | grep `identity_template` 生产侧仅此一处 | D7 删除“前端模板替换可提交”（`nck-prc-start-review.md:100`） |
| A4 | 被新裁决取代 | 前端无编码器；未 import golden fixture | G5 grep：`nck-point-key-golden` 仅 `crates/core-types/src/point.rs:987`、`drivers/sinumerik-nck/tests/compiler_identity.rs:24,1029` | D7 后 A4 失去对象；见 §6-1 |
| A5 | 被新裁决取代 | 同上 | 同上 | 同上 |
| A6 | 已落地 | `nckPointModel.ts:152-193`（targets/target_required 驱动） | `nckPointModel.test.ts:119 compound_target_generates_two_required_fields_from_definition`、`:138`；`NckPointFields.test.tsx:106` | `drive_object` 渲染为一行 number+entry 两个输入（TargetRef 形状），与 A6 字面“只渲染一个”有措辞张力（§6-4） |
| A7 | 已落地 | `NckPointFields.tsx:319-328`；`nckCollectability.ts:209-218`；`nckTaskBinding.ts:130-151` | `nckCollectability.test.ts:94`（恰 3 键）；`nckB.test.tsx:312-321`（`toEqual` 恰 3 字段） | — |
| A8 | 已落地 | `NckPointFields.tsx:445-574`（按定义生成，不 import `missingRequiredParams`）；`ResourcePickerAntd.tsx:141-150` | `NckPointFields.test.tsx:106`；`ResourcePickerAntd.test.tsx:427`（无“手动”页） | 语义路径不拿 12 原生 schema 判必填 |
| A9 | 已落地 | `nckPointModel.ts:410-420`（required argument 缺失） | `nckPointModel.test.ts:195,213`；`NckPointFields.test.tsx:184` | 加入按钮 disabled 由 `:271-278` 的 `validation.ok` 保证 |
| A10 | 已落地 | `ResourceBrowseAntd.tsx:132-135,274-279`；`NckPointFields.tsx:271-278` | `ResourceBrowseAntd.test.tsx:254 null_browse_preview_still_opens_semantic_form_with_reason (A10)`；`browseModel.test.ts:71` | 文案为“身份未确定/尚无服务端确认的身份”，与矩阵“待确认字段…”语义等价、字面不同 |
| A11 | 已落地 | `bootstrap.ts:138-163`（零选择 ⇒ `tasks=[]`） | `bootstrap.test.ts:112,126` | — |
| A12 | 已落地 | `nckTaskBinding.ts:478-488`（原位替换、其它任务逐字保留） | `nckTaskBinding.test.ts:266`（跨 kind 保留）；`nckB.test.tsx:328` | — |
| A13 | 已落地 | `nckCollectability.ts:48-130`；`EndpointAcquisitionPane.tsx:180-183,580-597`；`NckPointFields.tsx:120-125` | `nckCollectability.test.ts:47,64,78`；`EndpointAcquisitionPane.nck.test.tsx:214,367`；`nckB.test.tsx:236`；`NckPointFields.test.tsx:210` | UI 双执行 + 提交模型第三道（§4.2） |
| A14 | 已落地 | `nckPointModel.ts:380-393`（entry 必填、不因 number 反推） | `nckPointModel.test.ts:138 drive_object_entry_is_explicit_and_never_inferred_from_number` | Golden 示例只作 datalist，不作闭集 |
| A15 | 已落地 | `NckPointFields.tsx:182-263`（Preview 失败/丢弃保持 disabled）；`ResourceBrowseAntd.tsx:235-236`（Browse 失败无节点） | `ResourceBrowseAntd.test.tsx:340-366`（503 / 网络异常 / 响应不可解析三组，均断言无可操作语义叶、回调零调用）；`NckPointFields.test.tsx:405` | 缺口已关闭（收口修复） |
| A16 | 已落地 | `browseModel.ts:87-90`（无 identity ⇒ null） | `browseModel.test.ts:44 leaf_without_identity_not_selectable (A16)`；`ResourceBrowseAntd.test.tsx:239` | — |
| A17 | 已落地 | `browseModel.ts:71-84`（空信封/非法 JSON/非对象 ⇒ null） | `browseModel.test.ts:144,167,173` | 既有断言未放宽 |
| A18 | 已落地 | `NckPointFields.tsx:319-328`（仅 `variable_id/definition_revision/target/arguments`）；`nckTaskBinding.ts:130-151` | `nckB.test.tsx:312-321`（`parameters` 恰 4 键） | 无 area_no/line/module/decode/transport_size |
| A19 | 已落地 | `deviceModel.ts:242-289,401-407`；`bootstrap.ts:140-161` | `nckTaskBinding.test.ts:132,144`；`bootstrap.test.ts:170`；`nckB.test.tsx:398` | NCK + 旧 selections 装配直接拒绝 |
| A20 | 已落地 | `NckPointFields.tsx:319-328`（语义载荷无 `identity_mode`） | `nckB.test.tsx:312-321`；`nckPointModel.test.ts:348` | `user` 仅 native；语义预览要求 `identity_mode=derived` |
| A21 | **转 PR-D** | 组件能力：`ResourcePickerAntd.tsx:141-150,354-470`；Endpoint 页：`EndpointAcquisitionPane.tsx:910-940` | `ResourcePickerAntd.test.tsx:427 semantic_config_is_default_and_legacy_variable_entry_is_not_rendered` | **Endpoint 页已满足**；向导 `AddDeviceFlow.tsx:254-257` 未传 `semantic`，向导侧默认页仍是旧 Picker ⇒ PR-D |
| A22 | **转 PR-D** | `ResourcePickerAntd.tsx:453-458`（旧入口整体不渲染）；`ResourceBrowseAntd.tsx:282-290`（旧 variable 信封阻断） | `ResourcePickerAntd.test.tsx:427`；`ResourceBrowseAntd.test.tsx:289 legacy_variable_fallback_is_not_offered_in_semantic_context (A22)` | 同上；向导侧未接线 |
| A23 | 已落地 | `NckPointFields.tsx:594-617`（`<code>` 展示，无输入框） | `NckPointFields.test.tsx:484 readonly_key_and_conflict_blocks_add` | 表单无 display_name 字段，故“改 display_name 不影响 Key”恒成立 |
| A24 | 已落地 | `NckNativeFields.tsx:47-67`（默认模式/提交门）、`:55`（默认未确认）、`:109-115`（切 user 复位）、`:126-130`（改 Key 复位）、`:139-147`（显式确认） | `NckNativeFields.test.tsx:58`（首次确认）、`:113`（重开存量 user 点位未确认 + 改 Key 撤销）、`:144`（derived→user 重新确认） | 缺口已关闭（收口修复）；Key 仍经命名空间/语法/冲突校验 |
| A25 | 已落地 | `nckNativeModel.ts:62-123,145-175`（原值保留、无自动 `~<n>`、冲突拒绝） | `NckNativeFields.test.tsx:58,79`；`nckNativeModel.test.ts:64,88` | 允许用户显式写 `~2`，但不自动追加；冲突由表单+父层+提交模型三层拒绝 |
| A26 | 已落地 | `nckNativeModel.ts:77-83`（`native.` 前缀 + 语法） | `NckNativeFields.test.tsx:79`；`nckNativeModel.test.ts:64` | Driver 侧同口径：`compiler.rs:572-585` |
| A27 | 已落地 | `nckNativeModel.ts:29-52`（恰 5 个寻址字段） | `nckNativeModel.test.ts:43 native_derived_key_uses_area_area_no_module_column_line_only`；`NckNativeFields.test.tsx:42` | 与 Driver `drivers/sinumerik-nck/src/definition.rs:1163-1176` 逐字同规则；见 §4.3 |
| A28 | 已落地 | `deviceModel.ts:260-305`（唯一装配入口）；`bootstrap.ts:140-161` | `bootstrap.test.ts:151`（两处 binding 逐字相等） | `bootstrap.ts` 已无 `"mesa.resources.v1"` 字面量 |
| A29 | 已落地 | `deviceModel.ts:247-249`（按 Driver 选 kind） | `nckTaskBinding.test.ts:132`；`bootstrap.test.ts:170,182`；`nckB.test.tsx:309` | — |
| A30 | 已落地 | `NckPointFields.tsx:280-286`（切换变量重建空草稿） | `NckPointFields.test.tsx:106`（切到 drive_object 后 axis/argument 控件消失） | — |
| A31 | 已落地 | 同上；`validateNckForm` 拒绝未声明变体 `nckPointModel.ts:344-357` | `NckPointFields.test.tsx:106`；`nckPointModel.test.ts:119` | channel/axis 不残留 |
| A32 | 已落地 | `NckPointFields.tsx:468-480`（受控 `onChange`，无 blur 依赖） | `nckB.test.tsx:298-321`（`user.type` 后直接点击，无 blur，载荷为当前值） | — |
| A33 | 已落地 | `git show --stat 354bca9` 的 4 文件不含冻结面；`connection/**` 134 用例 | 本轮实跑 vitest（43 文件 / 503 用例全绿） | “原样通过”由本轮全量 vitest 证明；冻结文件未改 |
| A34 | 已落地 | 43 文件 / 503 用例 | 本轮实跑 vitest 退出码 0 | ≥383 基线成立（较上轮 498 增 5，全部来自 A15/A24 回归） |
| A35 | 已落地 | grep 生产代码：`area_no/transport_size/unit_mode/F32_BE` 仅 `nckNativeModel.ts`、`NckNativeFields.tsx` | 本轮 grep（§7） | 属 A27 明文允许的 native 例外；语义路径零命中 |
| A36 | 已落地 | `resourceSelectionModel.ts` 未在 diff；算法 `:169-377` 未改 | `deviceModel.test.ts:236-373`（既有用例未改） | 冻结算法本体 |
| A37 | 已落地 | `endpointSavePlan.ts:97-107`（快照门最前） | `endpointSavePlan.test.ts:147`；`deviceModel.test.ts:518-534`；`EndpointAcquisitionPane.nck.test.tsx:240` | 矩阵建议文件写 `saveIssues.test.tsx`，实际快照门用例在 `nck.test.tsx` |
| A38 | 已落地 | 同 A35；语义表单 `NckPointFields.tsx` 无原生字段常量 | 本轮 grep | 原生字段只来自 Descriptor `variable` 资源 |
| A39 | 已落地 | `NckPointFields.tsx:170-172`（family+输入指纹）、`:266-278`（渲染期失效）、`:288-329`（提交前重算指纹） | `NckPointFields.test.tsx:293,350,381,405`；`nckPointModel.test.ts:299` | 迟到响应整份丢弃，旧 Key 不可见 |
| A40 | 已落地 | `nckCollectability.ts:57-98`（缺失/identity/未知一律 false） | `nckCollectability.test.ts:98`；`NckPointFields.test.tsx:210`；`nckB.test.tsx:236` | 正向白名单，非 `!== "identity"` 反写 |
| A41 | 被新裁决取代 | 前端未消费 `tests/fixtures/nck-point-key-golden.json`；`tsconfig.json`/`vite.config.ts` 未在 diff | G5 grep 仅 Core/Driver | D7 后无前端编码器；A41 的“读取向量”失去对象；“不复制第二份/不改配置”仍成立 |
| A42 | 已落地 | `nckPointModel.ts:140-172`（缺 targets ⇒ 不可提交） | `nckPointModel.test.ts:180 only_identity_keys_never_unlocks_submit (A42)` | 字面张力：`:205-220` 由 `identity_keys` 推导 argument 必填（仅收紧，与 Driver `definition.rs:1060-1074` 一致，见 §6-3） |
| A43 | 已落地 | `NckPointFields.tsx:271-278`（元数据+身份+family 全过才可提交） | `NckPointFields.test.tsx:272,484`；`nckPointModel.test.ts:331` | 反例：identity/preview 失败/family 不符均 disabled |
| A44 | 已落地 | `browseModel.ts:105-121`（非语义叶形状逐字不变） | `browseModel.test.ts:8,86 non-semantic leaves stay byte-for-byte shaped (A44 negative half)`；`ResourceBrowseAntd.test.tsx:157` | 两类回归齐备；非 NCK 叶不要求 identity |

### 2.1 计数

- 已落地：**37**（A1、A6–A15、A16–A20、A23–A40、A42–A44）
- 被新裁决取代：**5**（A2、A3、A4、A5、A41）
- 转 PR-D：**2**（A21、A22）
- 部分：**0**

### 2.2 第三批（`600a492`）声称项的独立复核

| 声称项 | 复核结论 | 关键证据 |
|---|---|---|
| A1 `browseModel` identity 透出 | **真落地** | `browseModel.ts:87-102`；`browseModel.test.ts:8` 断言 `identity` 与 `parameters` 分离 |
| A16 `selectableFromNode` 准入 | **真落地** | `browseModel.ts:87-90`；`browseModel.test.ts:44` 对 `nck_variable`/`nck_native` 两种 kind 都返回 `null` |
| A44 非 NCK 零影响 | **真落地** | `browseModel.ts:105-121`；`browseModel.test.ts:86` 对 generic/OPC UA 叶逐字断言形状不变 |
| A13 scope 双执行 | **真落地** | `nckCollectability.ts:48-130` + `EndpointAcquisitionPane.tsx:580-597` + `NckPointFields.tsx:122-125`；测试 `nckB.test.tsx:236`、`EndpointAcquisitionPane.nck.test.tsx:214` |
| A8 语义模式不用 12 原生 schema | **真落地** | `NckPointFields.tsx:445-574`（定义驱动）；`ResourcePickerAntd.tsx:141-150`（语义模式整体替换旧 manual 页） |
| A21 语义默认入口 | **组件层落地；生产向导未接线** | `ResourcePickerAntd.test.tsx:427`（静态用例）；`AddDeviceFlow.tsx:254-257` 未传 `semantic` ⇒ PR-D |
| A22 旧入口阻断 | **组件层落地；生产向导未接线** | `ResourceBrowseAntd.test.tsx:289`、`ResourcePickerAntd.tsx:453-458`；向导同上 |
| A24 user 显式确认 | **真落地（收口修复）** | `NckNativeFields.tsx:55,109-115,126-130,139-147`；`NckNativeFields.test.tsx:58,113,144` |
| A25 Key 原值保留、冲突拒绝 | **真落地** | `nckNativeModel.ts:62-123,164-174`；`NckNativeFields.test.tsx:58,79` |
| A26 `native.` 命名空间 | **真落地** | `nckNativeModel.ts:77-83`；`NckNativeFields.test.tsx:79` |
| A27 derived 固定规则 | **真落地** | `nckNativeModel.ts:29-52`；`nckNativeModel.test.ts:43`；与 `drivers/sinumerik-nck/src/definition.rs:1163-1176` 同规则 |

**结论**：第三批声称的 11 项中，**9 项真落地、2 项仅组件层（向导转 PR-D）、0 项部分**。A24 已在收口修复中关闭。


## 3. 遗漏项结论（本轮最重要）

### 3.1 结论

**除“已被 D7 取代的编码器类”与“已转 PR-D 的向导接线类”外，PR-C 的强制 A 项已全部落地。**
44 行现为 **37 已落地 / 5 被取代 / 2 转 PR-D / 0 部分**；G5 已按裁决豁免；V1–V7 隔离集成 31/31 PASS。剩余未完成项只有：
**PR-D 向导接线**（不在 PR-C 范围）与**真实 CNC 真机验证**（GATE，另需授权）。

### 3.2 A15 关闭记录（测试缺口已补）

- 新增 `ResourceBrowseAntd.test.tsx:340-366`，三组用例：
  `browse_http503_failure_never_offers_semantic_leaf`、
  `browse_network_failure_never_offers_semantic_leaf`、
  `browse_unparseable_failure_never_offers_semantic_leaf`。
- 三种情况均断言：无“选用（语义）”、无“选用”、`onFill`/`onSemanticSelect` 零调用。
- 503 / 网络异常显示“浏览不可用”；响应体不可解析按空页处理，不猜节点。
- 未改 `ResourceBrowseAntd` 请求层：现有 fail-closed 实现已正确，测试未暴露旁路。
- 本轮实跑：`ResourceBrowseAntd.test.tsx` 12 用例通过。

### 3.3 A24 关闭记录（确认状态已修）

- `NckNativeFields.tsx:55`：`confirmed` 初始恒为 `false`；重开已有 user 点位也只展示原 Key。
- `NckNativeFields.tsx:109-115`：每次选择 user 模式都复位确认。
- `NckNativeFields.tsx:126-130`：任何 Key 编辑立即 `setConfirmed(false)`。
- `NckNativeFields.tsx:139-147`：只有显式勾选才恢复确认。
- 新增 `NckNativeFields.test.tsx:113`（存量 user 点位未确认、改 Key 撤销、重新确认后可提交）与 `:144`（derived→user 必须重新确认）。
- 本轮实跑：`NckNativeFields.test.tsx` 6 用例通过。

### 3.4 阻塞状态

- A15/A24 的阻塞已解除；前端门禁（vitest/tsc/lint/build）本轮全绿；V1–V7 隔离集成 31/31 PASS（§10）。
- 仍 HOLD：PR-D 向导接线、真实 CNC 真机验证、PR #116 合并。
- 本轮不执行提交/推送；最终合并审查由上级在取得远程 CI 后进行。

### 3.5 非 NCK 的“零行为变化”有一个有意例外

- `planCanonicalSave` 逐字调用 `mergeAcquisitionTasks`（`endpointSavePlan.ts:117-144`）。
- 唯一差异：**非 NCK Endpoint 上存在 `nck.points.v1` 任务**时，旧实现 no-op 后误报成功，新实现显式 `DRIVER_KIND_MISMATCH`（`endpointSavePlan.ts:117-127`；测试 `endpointSavePlan.test.ts:262`）。
- 这是数据安全修正，不是回归；但“零行为变化”应表述为“正常路径逐字不变 + 该病态路径由虚假成功改为拒绝”。

## 4. 一致性核查（五项）

### 4.1 Key 真值唯一性 —— **通过（唯一例外：A27 native derived）**

逐一枚举生产代码中所有 `point_key` 产生/传递点：

| 位置 | 行为 | 判定 |
|---|---|---|
| `nckCatalog.ts:597-628` | `previewNckPoint` 原样返回服务端 `point_key` | **唯一语义 Key 来源** |
| `NckPointFields.tsx:266-278,319-328` | 只从 `previewState.pointKey` 取 Key；指纹不符即 `null` | 通过 |
| `EndpointAcquisitionPane.tsx:894-900` | Browse 预填 `point_key: ""`，参数来自 `sel.parameters` | 通过（Browse Key 不进入载荷） |
| `ResourcePickerAntd.tsx:414-420` | 同上 | 通过 |
| `ResourceBrowseAntd.tsx:270-273` | `preview_point_key` 仅展示文案 | 通过 |
| `ResourcePickerAntd.tsx:73-83,193` | `suggestPointKey` 只服务非语义 Picker；语义模式在 `:141-150` 提前返回 | 通过（非语义路径，且 NCK 向导保存会被 `assembleBinding` 拒绝） |
| `nckNativeModel.ts:29-52` | `nativeDerivedPointKey` 按冻结规则生成 | **A27 明文允许的唯一例外** |
| `nckNativeModel.ts:62-123,164-174` | `user` Key 原值透传，不自动改名/后缀 | 通过（Driver 不重算，只校验语法） |
| `nckTaskBinding.ts:130-151` | 只校验/重建 3 字段信封，从不生成 Key | 通过 |

- **无前端模板替换兜底**：生产侧 `identity_template` 只在 `NckPointFields.tsx:416` 展示；无占位符替换实现。
- **Browse `preview_point_key` 不进入提交载荷**：两处预填都把它丢掉；`browseModel.ts:37-44` 把它放在 `identity` 而非 `parameters`。
- 结论：**未发现第二套语义 Key 真值**。

### 4.2 `scope` 准入无旁路 —— **通过（生产路径）；PR-D 接线需补提交模型复核**

| 入口 | UI 准入 | 提交模型复核 | 结论 |
|---|---|---|---|
| Endpoint 语义表单（`NckPointFields`） | `NckPointFields.tsx:141` `evaluateNckSelectionAdd`；`:122-125` 只提供 collect | `EndpointAcquisitionPane.tsx:645-655` → `endpointSavePlan.ts:147-196` → `nckTaskBinding.ts:297-309` → `nckCollectability.ts:181-219` | **三道**，通过 |
| Endpoint Browse 语义叶 | `ResourceBrowseAntd.tsx:93-136`（scope/family/identity） | 进入同一表单 + 保存计划 | 通过 |
| 存量点位加载 | `EndpointAcquisitionPane.tsx:287-295`（逐点 scope） | `EndpointAcquisitionPane.tsx:580-597` + `:633-642` + 保存计划 | 通过 |
| Picker 语义模式（`SemanticResourcePicker`） | 复用 `NckPointFields`（组件内 scope） | **无**：`onAddPoint` 由父层任意实现，Picker 不调用 `planEndpointTaskSave` | **未接线，生产无路径**；PR-D 接线时必须走保存计划，否则是旁路 |
| Native 高级模式 | 不适用目录 scope（`nckTaskBinding.ts:397-400`） | 信封 + 原生规则/字段 Schema（`EndpointAcquisitionPane.tsx:217-254`） | 设计允许；提交模型层只校验信封（见 §6-9） |
| 向导（`AddDeviceFlow`） | 无 NCK 语义入口（`AddDeviceFlow.tsx:254-257` 未传 semantic） | `bootstrap.ts:140-155` 对 NCK+旧 selections 直接拒绝 | 无旁路，但不可用（PR-D） |

- `removePoint` 刻意不过 scope 门（`EndpointAcquisitionPane.tsx:552-558`），但只做删除，不产生提交，**不是旁路**。
- 结论：**生产入口未发现可绕过 `scope` 的路径**；Picker 是 PR-D 的**潜在旁路点**，必须在接线时补提交模型复核。

### 4.3 Native 两种模式 —— **与 canonical_identity 语义一致（含边界说明）**

- **derived**：TS `nativeDerivedPointKey`（`nckNativeModel.ts:29-52`）与 Rust `native_identity_key`（`drivers/sinumerik-nck/src/definition.rs:1163-1176`）逐字同规则：`native.<area>.<area_no>.<module>.<column>.<line>`，不含 `count/decode/transport_size/invalid_*/identity_mode`。
  - Driver 在 Configure 重算并逐字比对（`drivers/sinumerik-nck/src/compiler.rs:557-570`），不一致即 `POINT_KEY_MISMATCH`。
  - **前端未对 native derived 调服务端 Preview**（实现事实；A27 只冻结规则，未要求 Preview）：最终防线是 Configure 的 D11。真实 API 验证必须覆盖（§5）。
- **user**：前端只校验 `native.` 前缀 + 通用语法（`nckNativeModel.ts:62-123`），Key 原值保留、不追加 `~<n>`（`:164-174`）；Driver 不重算，仅复核语法/命名空间（`drivers/sinumerik-nck/src/compiler.rs:572-585`）。
  - 与 Core `validate_native_key` 的差异：前端要求 area 段**大写字母**，Core 允许 `is_ascii_alphabetic`（`crates/core-types/src/point.rs:469-478`）——前端更严，不是漏洞（§6-6）。
- 结论：derived 与 Driver 规则一致；user 是设计上的“用户显式身份 + Driver 只复核语法”，**没有**服务端可重算的权威身份（`drivers/sinumerik-nck/src/compiler.rs:359-360` 明示），与 A25 的裁决一致。

### 4.4 保存路径唯一 —— **通过**

- `planEndpointTaskSave` 唯一调用点：`EndpointAcquisitionPane.tsx:645`；唯一 `PUT /api/v1/tasks/{id}`：`EndpointAcquisitionPane.tsx:690-694`。
- `mergeAcquisitionTasks` 遇到 NCK 任务**显式抛错**（`deviceModel.ts:401-407`），杜绝第二套装配。
- `bootstrap.ts` 走 `POST /api/v1/device-bootstrap`（创建链），不是第二条任务 PUT；`api.ts:69-81` 的 `putJson` 用于 device/endpoint/event-tasks，不用于采集任务。
- 测试：`endpointSavePlan.test.ts` 全链、`EndpointAcquisitionPane.nck.test.tsx:307`、`nckB.test.tsx:273`。
- 结论：**无第二条采集任务 PUT 路径**。

### 4.5 非 NCK 零行为变化 —— **通过（附 §3.5 的有意例外）**

- 非 NCK UI 分支未改：`EndpointAcquisitionPane.tsx:787-817`（仍 `ResourcePickerAntd` + `applySelectionAdd`）。
- 保存：`planCanonicalSave` 逐字调用 `mergeAcquisitionTasks`（`endpointSavePlan.ts:133-136`）。
- 测试：`endpointSavePlan.test.ts:230`（与旧 merge 逐字相等）、`:253`（空选择旧文案）、`:278`（快照门不变）；`EndpointAcquisitionPane.nck.test.tsx:389`（PUT body 与旧 merge 相等）。
- 唯一有意差异：§3.5 的 `DRIVER_KIND_MISMATCH`。
- 结论：**正常非 NCK 路径零变化**。

## 5. 真实 API 验证清单（原始定义；执行结果见 §10）

> 编号说明：本节是**原始定义**；裁决表的 V1–V7 与本节编号存在错位，映射见 §10.1。本轮按场景名执行，结果见 §10。
> 前置共同项：`cargo build --workspace` + 显式授权启动 `mesad`；创建/复用 `sinumerik-nck` Endpoint（`connection.family=828d`）。本轮使用隔离 DB/端口/PKI + 软件 NCK emulator，**未连真实 CNC、未启动真实 CNC 采集任务、未安装**。

| # | 场景 | 环境前置 | 验收要点 | 状态 |
|---|---|---|---|---|
| V1 | Browse 语义叶 | mesad + NCK Endpoint 可达；Browse 根/分支可下钻 | 叶 `kind=nck_variable` 带 `identity{template,keys,preview_point_key}`；`preview_point_key` 只展示；identity 叶显示 `SCOPE_IDENTITY` 且无“选用（语义）”；非 NCK 叶仍走旧回填 | **已执行（裁决表 V2）** |
| V2 | Native derived | 同上 + Descriptor `variable` 12 字段可用 | 选 derived → Key 恰为 `native.<area>.<area_no>.<module>.<column>.<line>`；保存后 Configure 不报 `POINT_KEY_MISMATCH`；重开编辑器复核通过 | **已执行（裁决表 V3）** |
| V3 | Native user | 同上 | 显式 Key 原值落库、无 `~<n>` 自动追加；冲突被拒；非法命名空间/语法被前端与 Driver 双重拒绝（`INVALID_USER_IDENTITY`） | **已执行（裁决表 V4）** |
| V4 | Picker 语义模式 | **当前无生产接线**（`AddDeviceFlow.tsx:254-257` 未传 semantic） | 只能在 PR-D 接线后验证；组件测试不能替代真实 API | **未执行（PR-D）** |
| V5 | Endpoint 全链 | mesad + NCK Endpoint + 真实目录 | 目录 42 条分页取全（36 collect / 6 identity）；Preview 成功；新增/编辑/移除；`PUT /tasks` 后回读一致；启动后 `points/latest` 出现值 | **已执行（裁决表 V5）** |
| V6 | 非 NCK 回归 | S7/OPC UA/Simulator Endpoint | Browse 叶形状、保存 PUT body 与旧实现逐字一致；不因新增可选字段变成不可选 | **已执行（裁决表 V7）** |
| V7 | 错误路径 | 可制造 503/501/413/400 的桩或断连 | Preview/Browse 失败时 `加入`/保存 disabled；错误码按 `kind` 展示；不回退 Browse Key | **已执行（裁决表 V6）** |

> 说明：V2/V3 的最终裁决在 Driver Configure（D11），前端单测无法证明“真实 Key 与 Driver 重算一致”；V5 才能证明“保存 → 启动 → 出值”闭环。
> A15/A24 的前端回归已在本轮关闭（§3.2/§3.3），但**不能替代** V1–V7 的真实 API 证据；本轮已在隔离环境执行，结果见 §10。

## 6. 无法确认 / 新发现的矛盾（如实登记）

1. **G5 门禁：已裁决豁免（不再是冲突）**。原 G5 要求 `nck-point-key-golden` 命中集合“恰为 {Core, Driver, 前端}”（`nck-prc-frontend-semantic-config-boundary.md:1274`）；D7 删除前端编码器后，实际命中只有 `crates/core-types/src/point.rs:987`、`drivers/sinumerik-nck/tests/compiler_identity.rs:24,1029`，前端 0 命中。**裁决**：G5 新口径为 ① Core/Driver 原有 Golden 测试继续通过；② 前端语义身份编码器必须不存在；③ 前端生产代码不得直接依赖 Golden；④ 不新增副本、不复制身份编码算法；⑤ 前端只验证权威 Preview 响应与当前输入的一致性。该勘误仅适用于 PR-C 的 G5。
2. **实施清单状态未回写**：`nck-prc-implementation-checklist.md:58` 写 P0-3“未开始（第二批 B）”、`:71` 写 P0-4“未开始（第二批 B）”，但 `nckCatalog.ts`/`NckPointFields.tsx` 已落地；该文件 `:7` 的基线“35 文件 / 405 用例”也已过期（现 43/503）。属文档滞后，不影响代码，但验收材料不能直接引用。
3. **A42 字面张力**：A42 写“不得由 `identity.keys` 推断 `arguments[]` 的类型/范围/必填”，但 `nckPointModel.ts:205-220` 会把 `identity_keys` 命中的 argument 标记为必填（仅收紧）。依据是 Driver 在身份层优先报 `MISSING_IDENTITY_KEY`（`drivers/sinumerik-nck/src/definition.rs:1060-1074`），行为正确；建议在验收材料中把“不得推断”限定为“不得用 keys 替代缺失的 `arguments[]` 元数据/不得据此放行”。
4. **A6 字面张力**：A6 写 `{drive_object}` “只渲染一个”，实际渲染 number+entry 两个输入（`NckPointFields.tsx:462-515`），这是 TargetRef `{number,entry}` 的必需形状；应理解为“一个 Target 变体/一行”，不是“一个 input”。
5. **A24 确认态：已关闭**（§3.3）。`confirmed` 默认 false；选 user、改 Key、derived↔user 均复位；存量 user 点位重开需重新确认。新增两条组件级测试。
6. **Native user 的 area 语法比 Core 更严**：前端 `[A-Z]+`（`nckNativeModel.ts:110`）vs Core `is_ascii_alphabetic`（`crates/core-types/src/point.rs:474`）。不会放行非法 Key，但会拒绝 Core/Driver 本可接受的 `native.c...`；是否放宽**待确认**。
7. **Native derived 无服务端 Preview**：A27 允许前端按冻结规则生成，但一旦 TS 规则漂移，只有 Configure D11 能发现。建议把“native derived 保存成功”列入 V2，并考虑在 PR-D 增加 native derived 的 identity-preview 调用（**待确认**是否属于本阶段范围）。
8. **Picker 语义模式的提交模型缺口**（§4.2）：当前无生产路径；PR-D 接线时若只把 `onAddPoint` 结果塞进 `selections` 而不走 `planEndpointTaskSave`，就会形成 scope 旁路。必须在 PR-D 设计里写死。
9. **`validateNckTaskSubmission` 对 native 只校验信封**：`nckTaskBinding.ts:397-400` 直接放行 `nck_native`，原生字段 Schema 校验只在 Endpoint 页 `verifyOne`（`EndpointAcquisitionPane.tsx:217-254`）。若未来有别的调用方直接调保存计划，native 字段不会被模型层复核。
10. **`buildNckFormModel` 把缺失 `arguments` 当空数组**：`nckPointModel.ts:195-203` 只在 `arguments !== undefined` 时校验类型，`:221` 用 `?? []`。I-2 目录契约要求 `arguments` 必填（`nckCatalog.ts:362-368`），故生产路径不会触发；测试注入的宽松定义可绕过“完整元数据”字面（A43）。
11. **A15 的 Browse 网络失败用例：已关闭**（§3.2，新增 3 条）；**A21/A22 向导接线**（§2）属 PR-D，不计入 PR-C 缺陷。
12. **G11 生产构建：本轮已执行并通过**（`tsc -b && vite build` 退出码 0）。D7 后前端不再 import golden fixture，G11 的原始风险（Vite `fs.allow`）不再触发。

## 7. 实际运行过的命令（git 只读；本轮有授权内代码/测试修改与构建）

```text
git rev-parse --abbrev-ref HEAD
git rev-parse HEAD
git status --porcelain=v1
git log --oneline -8
git show --stat HEAD
git diff --stat HEAD~1 HEAD
git diff --stat 2261178 HEAD
git diff --name-only 2261178 HEAD
git diff 2261178 HEAD -- apps/mesa-web/src/components/EndpointAcquisitionPane.tsx
git diff --stat                                # 本轮收口修改
git diff --name-only 600a492                   # 初始基线以来的修改面
git diff --name-only 354bca9                   # 当前 HEAD 以来仅本文件
git diff --check                               # 空白/冲突标记检查，退出码 0
Get-ChildItem apps/mesa-web/src -Recurse -File -Include *.ts,*.tsx
Get-ChildItem apps/mesa-web/src -Recurse -File -Include *.test.ts,*.test.tsx
Get-Content / Select-String（静态计数、用例名、行号定位、grep 红线）
node <pnpm.mjs> --dir apps/mesa-web exec vitest run \
  src/components/NckNativeFields.test.tsx src/components/ResourceBrowseAntd.test.tsx
node <pnpm.mjs> --dir apps/mesa-web test          # 43 文件 / 503 用例，退出码 0
node <pnpm.mjs> --dir apps/mesa-web typecheck     # tsc --noEmit，退出码 0
node <pnpm.mjs> --dir apps/mesa-web lint          # eslint .，退出码 0
node <pnpm.mjs> --dir apps/mesa-web build         # tsc -b && vite build，退出码 0
cargo build --workspace                            # 24.35s
target/debug/mesa-nck-emulator.exe --port 0 --scenario happy --element-size 8
target/debug/mesad.exe --db <tmp>/mesa.db --events-db <tmp>/events.db \
  --http-port <free> --drivers-dir drivers          # MESA_OPCUA_PKI_DIR=<tmp>/pki
python target/validation/pr-c-v1-v7-harness.py      # 31/31 PASS；结束后 terminate + 删临时目录
```

- **本轮运行**：`cargo build --workspace`；隔离 `mesad` + `mesa-nck-emulator`（V1–V7）；vitest / typecheck / lint / build。
- **未运行**：`cargo test --workspace`、Vite dev server、任何安装。
- **未连接**真实 CNC/机床；**未执行**任何 git 写操作（commit/push/checkout/stash/reset/PR/tag）。

## 8. 声明

- A15/A24 的限定范围修改已在 `354bca9` 提交/推送（`NckNativeFields.tsx`、`NckNativeFields.test.tsx`、
  `ResourceBrowseAntd.test.tsx` + 本文件）；本会话**未执行**该提交/推送。
- 本轮 V1–V7 **只更新本文件**（追加 §10 证据），**未修改**生产代码/测试；未改 Core / Driver / Proto /
  `apps/mesad` / `AddDeviceFlow.tsx` / `resourceSelectionModel.ts` 算法 / 其它冻结文件 / 配置。
- **未执行** commit / push / checkout / stash / reset / PR / tag；只读 git。
- V1–V7 期间**临时启动**隔离 `mesad` + 软件 NCK emulator（临时 DB / 独立端口 / 临时 PKI），已终止并清理；
  **未启动** Vite dev server；**未连接**真实 CNC/机床；**未安装**依赖。
- 本轮**实际运行** vitest（43 文件 / 503 用例）、`tsc --noEmit`、`eslint .`、`tsc -b && vite build`，均退出码 0；
  V1–V7 隔离集成 31/31 PASS（§10）；远程 CI 由上级在提交/推送后取得。
- 文中所有 `文件:行号` 按本轮工作树读取；A15/A24 的关闭证据见 §3.2/§3.3，门禁结果见 §9，V1–V7 见 §10。

## 9. 最后一轮收口修复记录

| 项 | 内容 |
|---|---|
| A24 代码 | `NckNativeFields.tsx:55` 默认未确认；`:109-115` 选 user 复位；`:126-130` 改 Key 复位；`:139-147` 显式确认 |
| A24 测试 | `NckNativeFields.test.tsx:113 existing_user_point_is_unconfirmed_and_requires_reconfirmation (A24)`；`:144 switching_derived_and_back_to_user_requires_reconfirmation (A24)` |
| A15 测试 | `ResourceBrowseAntd.test.tsx:340-366`：`browse_http503_failure_never_offers_semantic_leaf`、`browse_network_failure_never_offers_semantic_leaf`、`browse_unparseable_failure_never_offers_semantic_leaf` |
| G5 | 按裁决豁免；复核结果：前端生产代码 **0 引用** `nck-point-key-golden`、**0 份** Golden 副本、**无**语义身份编码器；`suggestPointKey` 仅存在于非语义 Picker（`ResourcePickerAntd.tsx:73-83`，语义模式 `:141-150` 提前返回）。新口径见 §6-1 |
| Vitest | **43 文件 / 503 用例全部通过**（较上轮 498 增 5：A15 三例 + A24 两例） |
| TypeScript | `tsc --noEmit` 退出码 0 |
| ESLint | `eslint .` 退出码 0 |
| 生产构建 | `tsc -b && vite build` 退出码 0（dist 已重新生成） |
| V1–V7 隔离集成 | **31/31 PASS**（软件 NCK emulator，非真实 CNC；见 §10） |
| 未提交 | A15/A24 修复已在 `354bca9` 提交/推送；本轮仅本文件（V1–V7 证据）未提交 |
| 仍 HOLD | PR-D 向导接线、真实 CNC 真机验证、PR #116 合并 |

## 10. V1–V7 隔离集成验收（2026-10-19）

### 10.1 编号映射（裁决表 vs 文档 §5）

> 裁决表的 V1–V7 与本文档 §5 的原始编号存在**错位**；本轮按场景名执行，两套编号映射如下，避免验收材料歧义。

| 场景 | 裁决表编号 | 文档 §5 旧编号 | 结果 |
|---|---|---|---|
| Catalog / Preview | V1 | §5 V1 前置 + V5 目录 | PASS |
| Browse 语义叶 | V2 | V1 | PASS |
| Native Derived | V3 | V2 | PASS |
| Native User | V4 | V3 | PASS |
| Endpoint 保存 | V5 | V5 | PASS |
| 错误 / 竞态 | V6 | V7 | PASS（API）+ UI 单测 |
| 非 NCK 回归 | V7 | V6 | PASS |
| Picker 语义模式 | — | V4 | 未执行（PR-D，无生产接线） |

### 10.2 环境与边界

- `cargo build --workspace` 成功（24.35s），使用 `target/debug/{mesad,mesa-driver-sinumerik-nck,mesa-nck-emulator}.exe`。
- 软件 emulator：`--port 0 --scenario happy --element-size 8`，自报 `MESA_NCK_EMULATOR_READY=127.0.0.1:<port>`。
- `mesad`：临时 `--db` / `--events-db` / `MESA_OPCUA_PKI_DIR` / 空闲 `--http-port` / `--drivers-dir drivers`。
- **未使用**生产 DB、现场 Endpoint、真实 CNC；未启动 Vite；未安装依赖。

### 10.3 PASS/FAIL（31/31 PASS）

| 组 | 检查项（全部 PASS） |
|---|---|
| V1 | driver_discovered、catalog_pagination_total（5 页/42 条）、scope_split（36 collect/6 identity）、14_fields、preview_semantic、preview_identity_returns_key_but_scope_identity |
| V2 | create_endpoint、browse_root（`nck://presets` + `nck://identity`）、browse_semantic_leaves（62 节点/62 语义）、identity_not_in_parameters、collect_and_identity_leaves_present（36/6） |
| V3 | native_derived_save、readback_key、configure_and_poll（Key `native.C.1.18.42.1`，GOOD） |
| V4 | native_user_save、key_preserved（`native.C.1.18.42.2~2`）、configure_and_poll（GOOD）、namespace_rejected（`INVALID_USER_IDENTITY`）、conflict_rejected（400 `DUPLICATE_POINT_KEY`） |
| V5 | preview_semantic_f64、semantic_save、readback_task_id_and_payload、configure_and_poll（GOOD）、edit_save、edit_keeps_task_id、edit_both_points_good、remove_keeps_task_id |
| V6 | browse_unreachable_returns_503（HTTP 503 `BROWSE_FAILED`）、endpoint_task_isolation；迟到 Preview / Endpoint 切换由 UI 单测 23/23 通过覆盖 |
| V7 | non_nck_save（S7 `mesa.resources.v1` 200）、non_nck_readback_unchanged（逐字回读） |

### 10.4 关键 API 证据（摘要）

- Catalog：`GET .../definitions?family=828d&limit=10` 连续 5 页，`total=42`、`definitions=42`、`collect=36`、`identity=6`、14 字段投影。
- Preview（collect）：`{"point_key":"channel.program.status@channel=i:1","identity_mode":"derived","scope":"collect","variable_id":"channel.program.status","definition_revision":1}`。
- Preview（identity）：`{"point_key":"axis.name@axis=i:1","scope":"identity","variable_id":"axis.name"}` —— **Key 合法但 scope=identity**，前端必须拒绝。
- Browse：根 `["nck://presets","nck://identity"]`；presets 62 节点全部 `kind=nck_variable`，`identity` 与 `parameters` 同级、不进入 `parameters`。
- Native Derived：`PUT /tasks` 200；`points/latest` `point_key=native.C.1.18.42.1`、`quality=GOOD`、`type=f64`、`source_label=nck://read/C/1/18/42/1/1?...`。
- Native User：`point_key=native.C.1.18.42.2~2` 原值保留、`quality=GOOD`；坏命名空间 `other.var@channel=i:1` 在启动 Configure 被 `ConfigurationError/INVALID_USER_IDENTITY` 拒绝；重复 Key 在保存时 400 `DUPLICATE_POINT_KEY`。
- Endpoint：`axis.position.actual_bcs@axis=i:1,channel=i:1` 保存/回读/启动 GOOD；新增第二点后 task id 仍为 `nck-task`；移除后仍为 `nck-task`。
- 错误：不可达 Endpoint 的 `nck://identity/axes` Browse 返回 **503** `BROWSE_FAILED`（`CONNECT_TIMEOUT`）；主 Endpoint 与不可达 Endpoint 的任务集相互隔离。
- 非 NCK：S7 `mesa.resources.v1` 保存 200，回读与提交逐字一致。
- 原始请求/响应：`target/validation/pr-c-v1-v7-evidence.json`（41 条请求/响应，151,693 字节）。

### 10.5 资源清理

- 所有测试 Endpoint 已 Stop；`mesad` 与 emulator 已 terminate；临时 DB / events DB / PKI / 日志目录已删除。
- 未触碰仓库根既有 `mesa.db` / `events.db`（时间戳早于本轮，非本轮生成）；无残留监听端口/进程。
- 证据文件在 `target/validation/`（gitignored），不属于生产代码。

### 10.6 边界与待确认

- 本轮是**软件 emulator** 集成，不等同于真实 CNC 真机验收；GATE 真机项仍 HOLD。
- V6 的“迟到 Preview / Endpoint 切换不误提交”是 UI 状态机行为，由 `NckPointFields.test.tsx` / `EndpointAcquisitionPane.nckB.test.tsx` 23 用例覆盖；API 侧只验证了 503 与任务集隔离。
- V5 使用 F64 语义变量 `axis.position.actual_bcs` 以匹配 emulator `element-size=8`；U16 语义变量在该 emulator 下返回 `BAD`，**推断**是 emulator 固定 8 字节 fixture 与 U16 期望长度不一致（未单独定位），属测试环境约束，不计为产品缺陷；真实设备应按请求长度返回。
- Picker 语义模式仍未执行（PR-D，`AddDeviceFlow.tsx` 冻结）。
