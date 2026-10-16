# PR-B 实施边界与验收清单：SINUMERIK NCK 语义点位与 Address Compiler

- 状态：**已按 PR #110 评审裁决完成定向修订——C1–C10 全部收敛（逐条见 §8.1 修订记录）**。本文件**不是**设计定稿，**不是**代码。
- 上位设计：`docs/design/nck-semantic-point-refactor.md`（1387 行，已冻结，本文件**逐字不改**它）；与本次裁决冲突之处**一律在 §8 / §8.1 如实记录，并以本次裁决为准**，**不反向修改上位设计**。
- 本文件核实基线：分支 `docs/nck-prb-boundary`，原核实 HEAD `d17b0dd`（= `main`，PR-A「Core 契约」已由 PR #109 合入）；**本次修订基线 HEAD `2977ca0`**。实测 `git diff --stat d17b0dd 2977ca0` 只有本文件一行（`1 file changed, 846 insertions(+)`），即**代码 / 测试 / 配置面与 `d17b0dd` 完全一致**，§0 的全部摘要与 §1 的全部行号继续有效。
- 本次交付方式：**只修改本文件**；未改任何代码 / 测试 / 配置 / 其它文档；**无任何 git 写操作**；未启动 `mesad`、未启动 Vite、未连接任何真实设备或机床。
- 阅读口径：本文件所有**行号**均为实际只读命令读取所得；与上位设计 §0 的转述不一致处，一律以本文件的实测值为准并在 §8 显式列出。
- **修订标注约定**：本次修订新增或变更的表述一律带 **【已裁决修正】** 前缀；**凡被本次裁决推翻的旧表述，一律不删除**，其「原表述 → 修正后」对照保留在 **§8.1 修订记录**，便于评审**只复核差异、不重新开展完整设计审查**。文中 `§C1`–`§C10` 指 §8 表中同名的裁决项。
- **【复核补充】优先级条款（正式批准，范围严格受限）**：**当上位设计 `docs/design/nck-semantic-point-refactor.md` 与本文件（PR-B 实施细化）在已正式裁决的 C1–C10 范围内发生冲突时，以本文件所述、经评审正式批准的实施裁决为准。**
  该优先级**仅适用于下表 / §8.1 明确列出的冲突项**；**不得**据此改变上位设计的**任何其它**已冻结契约（含未列入 C1–C10 的类型形状、Core/Driver 校验责任分工、身份契约、Wire 黄金、性能与只读基线等），**也不得**推广为「PR-B 优先于上位设计」的一般规则。上位设计**逐字不改**。**明确列出的冲突项 = C1、C2、C3、C4、C5、C6、C7、C9、C10（9 项）**；**C8 与上位设计无实质冲突**（上位设计本就写 `definition.rs`，仅命名口径统一）。逐条锚点见 §8 表「证据」列。

---

## 0. 本次核实的冻结指纹（供「逐字不变 / 逐字不动」验收直接比对）

下表 SHA-256 由 `Get-FileHash -Algorithm SHA256` 本地计算，行数由 `(Get-Content <f>).Count` 取得。**本表共 18 行**（原 13 行 + 本次新增 5 行）。
**PR-B 提交后必须由评审重新计算同名文件的摘要并逐条比对**：判定为「逐字不动」的文件摘要**必须完全相等**；判定为「修改」的文件摘要**必然变化**（因此摘要同时是「有没有偷偷改到冻结面」的证据）。【已裁决修正】本次新增的 5 行分别是 C2=P1 冻结的 `legacy_equivalence.rs` / `presets_828d.rs`、C10=D-A 冻结的 `descriptor_contract.rs`，以及为使「§2.3【逐字不动】的文件级条目都可在 §0 比对」而补齐的 `data_semantics.rs` / `resource_contract.rs`。

| 路径 | 行数 | SHA-256（本次） | PR-B 判定 |
|---|---:|---|---|
| `drivers/sinumerik-nck/src/client.rs` | 145 | `878aab515399ce2d44bdfdb17107ee6ef88cc8155addaf4dd44656132c2b5fc8` | **逐字不动** |
| `drivers/sinumerik-nck/src/codec.rs` | 118 | `cb8939ccfcdd90516e120f2186f3d11af95996c2da936507cbd5b3f68858c4e9` | **逐字不动** |
| `drivers/sinumerik-nck/src/value.rs` | 383 | `b564f68e017c4f932158c8861416f69f39da4ccfe8bc60c9d0bb66570c01edd6` | **逐字不动** |
| `drivers/sinumerik-nck/src/address.rs` | 170 | `23511a94af3f4bb4f3f3368226146b022f1d1e5f25e0c723afb728843003b97a` | **逐字不动** |
| `drivers/sinumerik-nck/src/identity_spec.rs` | 347 | `fca6926911eae30d222cd4a9e50d29973d80395a9f3a9678b8ffeed7ce665fd1` | **逐字不动** |
| `drivers/sinumerik-nck/src/identity.rs` | 492 | `acc2e42260788495379ca5808a451da2289a367fac726d54fcccc407310cdc25` | **逐字不动** |
| `drivers/sinumerik-nck/src/native.rs` | 673 | `8dd57a3a3872bf801ee15661725b0c8c437f7fbebabd74c709f054b57de25d8b` | **逐字不动**（见 §C3：由此推出 Compiler 必须剥离 `identity_mode`） |
| `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | 874 | `2105d74a80f6687a00a1385fa6ad873b585eeee03ff092efe7ecbf20dffa7f9f` | **逐字不动**（62 条 wire golden 真值） |
| `tests/fixtures/nck-point-key-golden.json` | 153 | `9d30f524b628a4f9224ff11b69527f2a3b48ca4d860e9014f32fd695a4b23d28` | **逐字不动**（E1–E11 三处共用向量） |
| `drivers/sinumerik-nck/presets/828d.json` | 1256 | `e910b0558dea80ca1f2b2d8bcdd566c3bc4cd725abe71544d34f9ede0380ebfa` | **逐字不动**【已裁决修正】C2=P1（原判定为「见 §C2：**建议**逐字不动」，现为**已裁决**） |
| `drivers/sinumerik-nck/tests/legacy_equivalence.rs` | 134 | `44e429bb198b7602a39bf5929149180c010274e9eab15b98f12dce3188c767a0` | **逐字不动**【已裁决修正】C2=P1（**本次新增行**：实测 2 个用例） |
| `drivers/sinumerik-nck/tests/presets_828d.rs` | 305 | `4eb747b55fe9cc4a20d7cb795ad9b4f3a1900608c6aa94e4a0ea7f50e181b6dd` | **逐字不动**【已裁决修正】C2=P1（**本次新增行**：实测 6 个用例） |
| `tests/driver-contract/tests/descriptor_contract.rs` | 1094 | `382064d346cd50c2d2b4ed7cae0cde86349c6d3af96609e0627613b3d658c15a` | **逐字不动**【已裁决修正】C10=D-A（**本次新增行**：`:605`、`:729-731` 两处断言**保留、不破坏**） |
| `tests/driver-contract/tests/data_semantics.rs` | 932 | `1663342e6e06688a4c416e3cc3f967044ebb085ecd0b68b9f7d71bf370193a4d` | **逐字不动**（**本次新增行**：补齐 §2.3 的文件级摘要，含 T2/T3a 的 8 个语义用例） |
| `tests/driver-contract/tests/resource_contract.rs` | 461 | `3936b79267c90d66329de6fa86aff12e91e96a74aac0213cd7a2275de5818460` | **逐字不动**（**本次新增行**：`resource_contract.rs` 整文件在 PR-B **零改动**，故可按文件级比对；§2.3 的验收要求仍是区间级 `:396-460` P9 护栏原样通过） |
| `crates/core-types/src/point.rs` | 1770 | `736af66b50cabf0ed23ae07aee3a199c5df84936d783f2e9011391421942aa11` | **逐字不动**（PR-A 已冻结契约） |
| `crates/core-api/src/binding_gate.rs` | 155 | `dfbf60e6829707a8e752194486ce3cac7ce51b13485efff5b76e9e8fb8832646` | **逐字不动**（PR-A 已冻结契约） |
| `docs/design/nck-semantic-point-refactor.md` | 1387 | `07b2fbf2cedf39d15163bc3fcd08208aad637184559f6aa09e1c80d537733aa8` | **逐字不动**（上位设计，已冻结） |

---

## 1. 代码现状核实（现查现核，非转述）

### 1.1 `drivers/sinumerik-nck/src/` 现有文件清单（13 个，全部为实测）

| 文件 | 字节 | 说明 |
|---|---:|---|
| `address.rs` | 5946 | `NckArea` / `NckUnitMode` 枚举 |
| `client.rs` | 5755 | S7 会话与 ReadVar |
| `codec.rs` | 4634 | 10 字节线缆编码 |
| `config.rs` | 18572 | 连接配置 |
| `fixture.rs` | 28496 | 夹具 |
| `identity.rs` | 18081 | 身份浏览 |
| `identity_spec.rs` | 12970 | 内建身份探针常量 |
| `lib.rs` | 82109 | 驱动主实现（1814 行） |
| `main.rs` | 1610 | 进程入口 |
| `native.rs` | 24100 | 原生地址解析与 `resolve_native` |
| `presets.rs` | 12637 | 模板注册表与浏览叶 |
| `probe.rs` | 4311 | 可达性探测 |
| `value.rs` | 15418 | 解码 / 类型 / 无效值规则 |

### 1.2 关键否证（这是本次最重要的核实结论）

| 名称 | 上位设计判定 | **实际** | 证据 |
|---|---|---|---|
| `drivers/sinumerik-nck/src/targets.rs` | §1.1「新增」 | **不存在** | `Get-ChildItem -Recurse drivers/sinumerik-nck/src` 无此文件 |
| `drivers/sinumerik-nck/src/definition.rs` | §1.1 / §7 PR-B「新增」（**单数**） | **不存在** | 同上 |
| `drivers/sinumerik-nck/src/definitions.rs` | 本轮任务书写作 `definitions.rs`（**复数**） | **不存在** | 同上 |
| `drivers/sinumerik-nck/src/compiler.rs` | §1.1「新增」 | **不存在** | 同上；今天地址转换内联在 `lib.rs:670-683` |
| `drivers/sinumerik-nck/definitions/`（目录） | §1.1 / §3.1「新增」 | **不存在** | `Test-Path drivers\sinumerik-nck\definitions` → `False`。**【已裁决修正】C5：目录位置固定为与 `src/` 平级**（`drivers/sinumerik-nck/definitions/`），上位设计 §1.1（`:192`）与 §7 PR-B 范围（`:1280`）写的 `drivers/sinumerik-nck/src/definitions/` **作废**；理由见 §3.1 |
| `drivers/sinumerik-nck/definitions/828d/DIGESTS.lock` | §1.1 / §3.1 / §6.2「新增」 | **不存在** | 全仓 `-Filter DIGESTS.lock` 无命中 |
| `docs/adr/0005-nck-semantic-point-definition.md` | §1.1「新增」 | **不存在**（`docs/adr/` 只有 0001–0004） | `Get-ChildItem docs\adr` |
| `crates/core-types/src/point.rs` | PR-A 交付 | **已存在**（1770 行） | 已由 `git ls-files` 确认为**已提交**文件 |
| `crates/core-api/src/binding_gate.rs` | PR-A 白名单收敛 | **已存在**（155 行） | 同上 |
| `tests/fixtures/nck-point-key-golden.json` | PR-A 交付 | **已存在**（153 行） | 同上 |

**结论**：PR-B 的全部 Driver 侧新增产物**一个都还没有**；PR-B 是从零建立定义目录、Target 类型与 Address Compiler。这一点必须在开工前被评审确认（它决定了 PR-B 的体量）。

### 1.3 与上位设计 §0.2 行号的核对

| 上位设计声称 | 实测 | 一致？ |
|---|---|---|
| `configure` 在 `lib.rs:596-739` | `lib.rs:596-767`（函数体至 `:767`，`:769` 起为 `apply_point_map`） | 偏差：末尾少报 28 行 |
| `GenericBinding` 反序列化在 `:633-641` | 一致 | ✅ |
| `kind == GENERIC_BINDING_KIND` 硬校验在 `:613` | 一致 | ✅ |
| `resource_id == "variable"` 在 `:644`、`output == "value"` 在 `:653` | 一致 | ✅ |
| 解析链 `from_parameters` → `resolve_native` 在 `:670-683` | 一致（`:670` 与 `:677`） | ✅ |
| PDU 预检 `:691-714` | 一致 | ✅ |
| `decode_point`（须逐字不动） | 实际 `lib.rs:430-523` | 上位设计未给行号，本文件补全 |
| Descriptor `variable` 资源 12 字段 `:170-311`、output `:313-319`、`selection_methods` `:324-327` | 一致（`resources: vec![` 起于 `:170`，`resource_selection_methods` 于 `:324-327`） | ✅ |
| 内嵌单测起点 | `#[cfg(test)] mod tests` 于 `lib.rs:1033-1034`，至 `:1814` | 上位设计未提，本文件补全 |
| 契约套件 29 套 | `scripts/contract_suites.py:13-52`（BASELINE 14 + EVENT 12 + LIFECYCLE 3 = 29） | ✅ |

**实测补充（上位设计未列，PR-B 必须处理）**：`lib.rs` 内嵌单测中，**大量用例把 `mesa.resources.v1` 的 `{resource_id, parameters}` 当作采集载荷**。它们全部位于 `lib.rs:1033-1814`，受 PR-B 影响的有：
`poll_task`（`:1379`）、`speed_selection`（`:1392`）、`driver_resolved_type_matches_decode_and_count`（`:1408`）、`configure_ok_and_rejects`（`:1444`）、`run_poll_e2e_ok_stop_and_restart`（`:1523`）、`run_bad_then_heal_lastknown`（`:1602`）、`run_session_loss_fails_attempt`（`:1662`）、`run_transport_mismatch_is_bad_without_fake_success_code`（`:1691`）、`oversized_count_is_rejected_at_configure`（`:1740`）、`nck_second_poll_task_failure_fails_whole_attempt`（`:1768`）。
**这 10 个用例必须迁移到新信封，但 `decode_point`（`:430-523`）与 `run`/`pack_array`（`:526-586`、`:792-1003`）的执行路径不得改动**——迁移只发生在「构造任务载荷」这一层。

### 1.4 `presets/828d.json` 实点数（自行计数，不引用文档数字）

- JSON 顶层键恰 3 个：`version` / `scope` / `templates`；`templates.Count = **62**`。
- 按 `parameters.area` 分布：`A`=4、`B`=1、`C`=16、`N`=1、`V`=40（共 62）。
- 按 `parameters.module` 分布：`21`=1、`26`=26、`112`=1、`114`=3、`115`=1、`122`=1、`125`=3、`127`=6、`130`=20（共 62）。
- 模板 id 形态实测为 `828d.<area>.<block>.<variable>.<nc_variable|drive_object>`，例如 `828d.C.S.progStatus.nc_variable`。
- `:31` 实测为 `"area": "B"`（属于 `B/S/opMode` 模板；`:30` 是 `"parameters": {`，`:32` 是 `"module": 127`）。**与任务书给出的行号一致**。
- 文件级校验（`presets.rs:122-149`）禁止模板预填 `area_no`/`line`，并要求 `required_user_inputs == ["area_no","line"]`、`validation_status == "candidate"`。

### 1.5 62 条 wire golden 实测

文件 `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json`（顶层 `source` / `note` / `entries`）：

- `entries.Count = **62**`；按 `entry` 分组：`nc_variable`=**42**、`drive_object`=**20**。→ 与 §6.5 的 42/20 完全吻合。
- `area_no` 去重后**只有 1 个值：`1`**；`line` 去重后**只有 1 个值：`1`**。
- `wire_hex` 第 3 字节（`syntax_id`）**62 条全部为 `0x82`**（unit_mode=current）。
- `area` 去重：`A`(4)、`B`(1)、`C`(16)、`N`(1)、`V`(40)。
- `module` 去重：`21, 26, 112, 114, 115, 122, 125, 127, 130`。
- 单条样本（第 1 条）：`{"area":"C","block":"S","variable":"progStatus","entry":"nc_variable","module":127,"column":13,"decode":"U16_LE","transport_size":"0x09","count":1,"area_no":1,"line":1,"wire_hex":"12 08 82 41 00 0d 00 01 7f 01"}`。
- 等价门实现：`tests/legacy_equivalence.rs`，断言 `checked == 62`（`:107`）、`entries.len() == 62`（`:48-52`）、两入口不折叠（`:112-133`）。

### 1.6 PR-A 已落地、PR-B **必须消费且不得改动**的 Core 契约

| 契约 | 位置 | PR-B 如何消费 |
|---|---|---|
| `NCK_POINTS_BINDING_KIND = "nck.points.v1"` | `crates/core-types/src/point.rs:82`（再导出 `crates/core-types/src/lib.rs:31-34`） | Driver `configure` 按此路由 |
| `PointSelection{kind, point_key, parameters}` + `deny_unknown_fields` | `point.rs:103-112` | Compiler 的输入信封 |
| `PointBinding{points}` | `point.rs:118-122` | 同上 |
| `PointKeyError` 8 变体 | `point.rs:141-158` | Driver 复核 Key 语法时的错误映射来源 |
| `encode_typed_value()` 类型标签编码器 | `point.rs:207` | **建议直接复用**（§4.4、§C 无冲突） |
| `validate_point_key()` 通用格式校验 | `point.rs:413`（`native.` 保留前缀分支 `:428-431`；`~<n>` 剥离 `:439-463`） | Driver 复核 + B 项命名空间判据 |
| `validate_point_binding()` / `parse_point_binding()` | `point.rs:818` / `point.rs:934` | 信封层校验（Driver 侧只复核） |
| `ensure_endpoint_point_keys_unique()` | `point.rs:954` | 跨 kind 唯一性（Core 侧） |
| `DATA_BINDING_KINDS = [“mesa.resources.v1”, “nck.points.v1”]` | `crates/core-api/src/binding_gate.rs:30` | — |
| 门禁三阶段 `route_data_task_kinds` / `parse_generic_bindings` / `cross_kind_point_key_issues` | `binding_gate.rs:51` / `:85` / `:112`；调用点 `crates/core-api/src/lib.rs:253`、`:281`、`:314`，`crates/core-api/src/bootstrap.rs:226`、`:234`、`:263` | — |
| P9 护栏 `core_has_zero_siemens_knowledge` | `tests/driver-contract/tests/resource_contract.rs:396-460`（禁令词表 `:398-428`） | **PR-B 不得让 Core 源码出现这些标识**；本文件在 `docs/` 下，不在扫描范围（扫描口径 `:432-434` 只扫两个 Core `src` 目录的 `.rs` 生产代码） |
| T2/T3a 语义用例（8 个） | `tests/driver-contract/tests/data_semantics.rs:742`、`:755`、`:793`、`:820`、`:838`、`:852`、`:872`、`:891`；`nck_point_key` 助手 `:82-91`（**字面量 Key，不推导身份**） | 原样通过 |
| E1–E11 golden 向量文件 | `tests/fixtures/nck-point-key-golden.json`（`spec`/`unicode_version`=`17.0.0`/`unicode_source`/`note`/`identity_boundary`/`value_cases` 55 条/`value_reject_cases` 5 条/`key_cases` 77 条） | **同一份文件**，不得复制 |
| Core 侧消费方式 | `point.rs:985-988`：`include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/nck-point-key-golden.json"))` | Driver 用**同一相对路径**：`drivers/sinumerik-nck` 的 `../..` 同样是仓库根，路径逐字可用 |

---

## 2. 交付物①：具体文件范围

判定口径：**【新增】** = PR-B 新建；**【修改】** = PR-B 改动并给出改动点与行号区间；**【逐字不动】** = PR-B 一个字节都不得改。

### 2.1 【新增】

| # | 路径 | 职责 | 依据 | 必须包含的可验收内容 |
|---|---|---|---|---|
| N1 | `drivers/sinumerik-nck/src/targets.rs` | Siemens 专用 Target 类型与语义校验：`TargetRef`、`DriveObjectRef`、`DriveEntry`、`TargetKind`；`entry → module`（26/130）映射；Target 集合合法性校验 | §1.1、§2.4.1、§2.4.2 D7/D10 | `DriveEntry` **必填、无默认值**；`TargetRef` 未知字段即拒；与 `identity_spec.rs` 常量的一致性单测 |
| N2 | `drivers/sinumerik-nck/src/definition.rs` | 定义目录类型与装载：`VariableDefinition`、`DefinitionScope`、`SemanticsStatus`、`ArgumentSpec`、`AddressingRule`、`NativeFieldSource`、`IdentityKeySpec`、`DefinitionSource`、`LocalizedText`、`DefinitionCatalog`（`lookup`/`available_revisions`/`for_family`/`verify_digests`）；canonical JSON + SHA-256 摘要；`identity_template` 渲染（canonical identity） | §1.1、§2.5、§3.2、§4.1.1 C | 定义目录装载期**自检**：ID 形状、`identity_template`/`identity_keys` 完备性、digest 一致 |
| N3 | `drivers/sinumerik-nck/src/compiler.rs` | Address Compiler 唯一入口：参数层严格校验（D1–D11）、Point Kind 路由、`identity_mode` 语义、语义→12 原生字段组装、调用**既有** `NativeNckAddress::from_parameters` + `resolve_native`、`CompileError` 全集 | §1.1、§2.6、§2.4.2 | **零新增字节组装**；`semantic` 失败**绝不**退化到 `native` |
| N4 | `drivers/sinumerik-nck/definitions/828d/r1.json` | 42 条定义 × `revision = 1` 的定义目录数据（含 `identity_template`/`identity_keys`/`golden_ref`/`semantics_status`/`source`） | §2.5.1、§3.1、§3.4 | 42 条 = A 16 + B 20 + C 6（§2.5.1 计数校验）。**【已裁决修正】C5**：定义目录**按 revision 分文件**且路径**与 `src/` 平级**（`include_str!("../definitions/828d/r1.json")`，与 `presets.rs:23` 的 `include_str!("../presets/828d.json")` 同模式）；单文件形态 `definitions/828d.json` 与 `src/definitions/**` 一律作废 |
| N5 | `drivers/sinumerik-nck/definitions/828d/DIGESTS.lock` | 已发布 `(variable_id, revision)` 的 canonical-JSON SHA-256 清单（含发行文件路径） | §3.1、§3.2、附加 2 | 只允许**新增**条目；由 T6 对照基线分支校验 |
| N6 | `drivers/sinumerik-nck/tests/definition_catalog.rs` | 定义目录完整性 / revision 不可变性 / digest / 语义状态 / `golden_ref` 双射 / A 项**静态**判据 | §6.2 | 见 **§5.1 的 #1–#12 与 #63** |
| N7 | `drivers/sinumerik-nck/tests/compiler_identity.rs` | Driver 侧身份契约与重算拒绝：T3b / T4 / T5(Driver) / **A 项反例** / **B 项命名空间** | §6.2、§6.6 T3b/T4/T5、**本文件 §5.2 / §5.3** | 见 §5.1 #13–#37 与 #57–#62 |
| N8 | `drivers/sinumerik-nck/tests/semantic_equivalence.rs` | 语义路径逐条复现 62 条冻结字节；G4 双入口不折叠 | §3.4、§6.2、§6.5 | `checked == 62` |
| N9 | `tools/ci/nck_definition_lock.py` | **对照基线分支**校验 `DIGESTS.lock`（附加 2）：基线已发布项不可变、只允许新增、不允删除 | §3.1、§6.2、§6.6 T6 | 三个场景可执行；离线；无第三方依赖 |
| N10 | `tools/ci/test_nck_definition_lock.py` | 上述工具的**自测**（临时 git 仓库构造三场景），沿用 `tools/ci/test_focas_diff.py` 的「PR-A 工具门」模式 | `_rust-quality.yml:37-41` 既有先例 | 三场景 + 当前树自洽，共 ≥4 断言组 |
| N11 | `docs/adr/0005-nck-semantic-point-definition.md` | 本次破坏性重构的 ADR 留痕（含 PR-A 已落地的 Core 侧契约追溯） | §1.1、§5 | 逐条覆盖 §5 的 9 项破坏 |

**相对上位设计的偏差（须评审确认）**：
- §1.1 只列了 `definition.rs`（单数）；本轮任务书写 `definitions.rs`（复数）。**本文件统一采用 `definition.rs`**（与 §7 PR-B 范围、§7.0-4 的独占文件清单一致）。见 §C8。**【已裁决修正】C8 已裁决**：`definition.rs`（复数 `definitions/` **只**用作数据目录名），全文已统一。
- 【已裁决修正】C1–C10 的收敛**未引入任何新的交付文件**：C1 的 6 个新用例落在 **N7 `tests/compiler_identity.rs`**，C2 的禁自证检测用例落在 **N6 `tests/definition_catalog.rs`**，C10 通过**减少**文件范围（§2.2 的 M7 撤销、`descriptor_contract.rs` 移入 §2.3）实现。
- §1.1 只列了 `definition_catalog.rs` 与 `semantic_equivalence.rs` 两个新测试目标；本文件**增加 `compiler_identity.rs`**（N7）。理由：T3b/T4/A/B 的信号必须能在**单独一个测试目标**里定位失败原因，混入 `definition_catalog.rs`（目录数据断言）会让「数据错误」与「编译器逻辑错误」在同一目标里不可区分。
- N9/N10 是 §6.2/附加 2 要求的**可执行**落点，但 §1.1 未列文件，属本文件补充。
- §1.1 列了 `apps/mesa-web/src/nckPointModel.ts` 等前端产物——**那是 PR-C 的范围**，不在本清单（§7.0-4、§8）。

### 2.2 【修改】

| # | 路径 | 改动点（本次实测行号） | 说明 |
|---|---|---|---|
| M1 | `drivers/sinumerik-nck/src/lib.rs` | ① `configure` **整体重写**：`:596-767`（今日为 `GenericBinding` 单一路径）；② 内嵌单测 `:1379-1814` 的 10 个用例迁移（见 §1.3 末段）。**【已裁决修正】C10=D-A：原「② Descriptor 的 `variable` 资源面 `:170-331` 改写」整条撤销**——Descriptor 面**逐字不动**（见 §2.3 与 §C10） | **不得触碰**：`decode_point` `:430-523`、`pack_array` `:526-586`、`probe` `:592-594`、`apply_point_map` `:769-788`、`run`/poll 数据面 `:792-1003`、`browse` `:943-1003`、`paginate` `:1010-1031`、`GENERIC_RESOURCE_ID` 常量 `:52`、**【已裁决修正】Descriptor 资源面 `:170-331`（含 `resource_selection_methods` `:324-327`）** |
| M2 | `drivers/sinumerik-nck/src/lib.rs:599-767` 的**语义变化**（单列，便于评审逐条核对） | 入口从「读 `GenericBinding` → 逐 output → `from_parameters`」改为「读 `PointBinding` → **逐 point 按 Point Kind 路由到 Address Compiler** → 逐点 `CompileError` → 任一点失败即**整体失败**」 | `:644`（`resource_id`）与 `:653`（`output`）两条硬校验**删除**；`:613` 的 kind 判定改为 `NCK_POINTS_BINDING_KIND` |
| M3 | `drivers/sinumerik-nck/src/lib.rs:740-752` | Descriptor 构造与 `ensure_unique_point_keys` 保留 | `DUPLICATE_POINT_KEY`（`:750-752`）行为不变（驱动侧同任务判重）。**【已裁决修正】C7=(a)**：该码仍是 Driver 侧唯一权威的「同任务重复」信号；B-2 的 `IDENTITY_NAMESPACE_COLLISION` 不替代它 |
| M4 | `drivers/sinumerik-nck/src/presets.rs:151-186`（`leaf_node` / `leaf_node_with`） | 浏览叶 `binding_json` 从 `{"resource_id":"variable","parameters":{…}}`（`:167-171`）改为**语义信封** `{"kind":"nck_variable","parameters":{variable_id,definition_revision,target,arguments}}`，并**旁挂** `identity` 块（`template`/`keys`/`preview_point_key`） | `identity` **只出现在浏览/预填载荷**，不得进入持久化信封（§4.1.1 C）。`PresetTemplate.parameters`（`:27` 起）**保持原生字段不变**——**【已裁决修正】C2=P1：模板逐字不动，语义索引放 `golden_ref`，仅浏览叶载荷改语义** |
| M5 | `drivers/sinumerik-nck/Cargo.toml:16-27` | 新增依赖 `sha2 = { workspace = true }`（workspace 已有 `sha2 = "0.10"`，`Cargo.toml:68`，实测） | 摘要算法用 |
| M6 | `drivers/sinumerik-nck/presets/README.md` | `:33-52`（索引语义表）后补「语义 `variable_id` ↔ 原生身份 ↔ 62 条 golden」对应章节；`:83-88`（`r0035` 哨兵）后补 `semantics_status` 说明；`:110` 后补 26/130 入口与 `identity_template` 的 `drive_object` 递归编码说明 | 文档；`PROVENANCE.md:9-54` 的 42 条清单**不删**。**【已裁决修正】C2=P1**：README 属**说明性文档**，可以补「模板保持原生字段、语义索引在定义目录」的对照说明；**不得**把 `PresetTemplate.parameters` 的语义在 README 里写成已被改写 |
| ~~M7~~ | ~~`tests/driver-contract/tests/descriptor_contract.rs`~~ | **【已裁决修正】C10=D-A：本条整条撤销。** 撤回的原改动点为 ~~`:605`（`d.resources.iter().any(|r| r.id == "variable")`）与 `:729-731`（`find(|r| r.id == "variable")` + `outputs[0].type_spec == DriverResolved`）随资源面改写~~ | 实测这两处只断言「`variable` 资源存在」与「`outputs[0].type_spec == DriverResolved`」，**与保留方案完全相容** → 该文件移入 §2.3【逐字不动】。原「**不得放宽**」条款（`:600-604` 只读能力、`:391-399` 生产 Write 未开发前不得声明 write）继续有效，改由「整文件逐字不动」保证 |
| M8 | `tests/driver-contract/tests/discovery_contract.rs:224-274` | **不改写**既有断言；**只新增**一条浏览叶形态断言（`nck://presets` 下取一叶，断言其 `binding_json` 为语义信封且**不含**原生字段） | 实测该套件当前**根本没有叶形态断言**（`:255-260`、`:270-273` 只断言根 2 节点 + 诊断节点 + 未知 parent 空页）→ **【已裁决修正】C6：采纳「只新增、不重写」**；该文件的**既有断言行**（`:255-260`、`:270-273`）属冻结面 |
| M9 | `.github/workflows/_rust-quality.yml` | 增加 `fetch-depth: 0`（当前 `:21-22` 为默认浅克隆）并把 PR base SHA 传入；新增一步 `python3 tools/ci/nck_definition_lock.py --base "$BASE"` | `ci.yml:53-60` 已有 `fetch-depth: 0` + `BASE: ${{ github.event.pull_request.base.sha \|\| github.event.before }}` 的**先例**，但那是 `plan` job；quality job 目前没有 base 信息 |

**不修改**（明确声明，防止范围蔓延）：`drivers/sinumerik-nck/presets/828d.json`、`drivers/sinumerik-nck/tests/legacy_equivalence.rs`、`drivers/sinumerik-nck/tests/presets_828d.rs`（三者**【已裁决修正】C2=P1：已正式移入 §2.3【逐字不动】，见 §0 摘要**）、`tests/driver-contract/tests/descriptor_contract.rs`（**【已裁决修正】C10=D-A：已正式移入 §2.3【逐字不动】**）、`crates/**`、`proto/**`、`apps/mesa-web/**`、`drivers/s7|focas2|opcua/**`。

**【已裁决修正】§2.2 最终计数**：【修改】= **8 条改动项**（M1–M6、M8、M9），覆盖 **6 个文件**（`src/lib.rs`、`src/presets.rs`、`Cargo.toml`、`presets/README.md`、`tests/driver-contract/tests/discovery_contract.rs`、`.github/workflows/_rust-quality.yml`）。M7 已按 C10=D-A 撤销，作为可追溯行保留（不再计入）。
**【已裁决修正】C2=P1 的范围影响**：原文档**从未**把 `presets/828d.json` / `legacy_equivalence.rs` / `presets_828d.rs` 列进 §2.2 的【修改】表（它们只出现在表下的「**不修改**」声明里）；本次进一步把它们**正式写入 §2.3【逐字不动】**并配实测 SHA-256，使「未列入【修改】」升级为「有摘要可验的冻结契约」。

### 2.3 【逐字不动】

| 路径 | 冻结理由 | 验收方式 |
|---|---|---|
| `drivers/sinumerik-nck/src/client.rs`、`codec.rs`、`value.rs`、`address.rs`、`identity_spec.rs`、`identity.rs`、`native.rs` | 执行层 / 协议常量 / 身份探针（§1.3 冻结） | §0 摘要逐字相等 |
| `drivers/sinumerik-nck/src/lib.rs` 的 `decode_point`（`:430-523`）与 poll 执行面（`:526-586`、`:769-1003`） | 运行质量处理与读取路径（P6） | 以 `:430-523`、`:526-586`、`:769-1003` 三个行号区间为准做区间级 diff 空断言 |
| `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | 62 条 wire golden 真值 | §0 摘要逐字相等 |
| `drivers/sinumerik-nck/presets/828d.json` | **【已裁决修正】C2=P1**：承载 62 个模板的**原生** `parameters`；`legacy_equivalence.rs:62-94` 与 `presets_828d.rs:210-256` 把它当原生字段读 | §0 摘要逐字相等；`git diff --exit-code -- drivers/sinumerik-nck/presets/828d.json` 为空 |
| `drivers/sinumerik-nck/tests/legacy_equivalence.rs`（2 个用例） | **【已裁决修正】C2=P1**：旧门是「用冻结的旧数据做真值」的载体（`:12` 注释）；语义化模板必然使其 8 个断言失败 | §0 摘要逐字相等 + 原样通过（`checked == 62`，`:107`） |
| `drivers/sinumerik-nck/tests/presets_828d.rs`（6 个用例） | **【已裁决修正】C2=P1**：读 `parameters.module`/`area`/`invalid_value`（`:15-26`、`:37-67`、`:210-256`、`:265-305`） | §0 摘要逐字相等 + 原样通过 |
| `tests/driver-contract/tests/descriptor_contract.rs` | **【已裁决修正】C10=D-A**：`variable` 资源面保留为「原生字段目录」、`resource_selection_methods` 不收缩 ⇒ `:605`、`:729-731` 无需改动；`:599-604` 的只读五条（descriptor < 256KB / poll / 无 subscribe / 无 write / 无 events / 无 commands）与 `:391-399` 亦原样 | §0 摘要逐字相等 + 原样通过 |
| `tests/driver-contract/tests/discovery_contract.rs:224-274` 的**既有断言**（`:255-260` 根 2 节点 / `nck://presets` / `diagnostic`，`:270-273` 未知 parent 空页） | **【已裁决修正】C6**：不重写既有断言，只在同文件**新增**一条叶断言 | 区间级 diff 空断言（新增断言必须落在既有断言行**之外**）；`git diff` 只体现新增 |
| `tests/fixtures/nck-point-key-golden.json` | E1–E11 **三处共用**向量 | §0 摘要逐字相等 + 断言三处引用同一路径（禁止出现第二份副本，见 §5.2） |
| `tests/driver-contract/tests/data_semantics.rs`（含 T2/T3a 的 8 个语义用例，`:742`、`:755`、`:793`、`:820`、`:838`、`:852`、`:872`、`:891`；`nck_point_key` 助手 `:82-91`） | PR-A 已冻结契约 | §0 摘要逐字相等 + 原样通过，用例数不减少 |
| `crates/core-types/src/point.rs`、`crates/core-api/src/binding_gate.rs` | PR-A 已冻结 Core 契约（§7.0-4：`point.rs` 为 PR-A 独占） | 摘要逐字相等 |
| `docs/design/nck-semantic-point-refactor.md` | 上位设计 | 摘要逐字相等 |
| `tests/driver-contract/tests/resource_contract.rs:396-460`（P9 护栏） | 修订 1 的护栏 | §0 摘要逐字相等（整文件零改动）+ 区间级 `:396-460` 原样通过 |

**【已裁决修正】§2.3 最终计数**：【逐字不动】= **13 行**，覆盖 **20 个文件**（第 1 行含 7 个文件）；其中 **18 个文件级条目**在 §0 有 SHA-256 可比对（**恰好等于 §0 表的 18 个数据行**）；余下 2 个是**区间级**冻结、无文件级摘要：`src/lib.rs`（`:430-523`、`:526-586`、`:769-1003`）与 `tests/driver-contract/tests/discovery_contract.rs` 的既有断言（`:255-260`、`:270-273`）。
**同步性自检（判定不冲突）**：§2.3 的 20 个文件与 **§2.4【不得触碰】的目录交集为空**；与 **§2.2 的 6 个文件**中，只有 `src/lib.rs` 与 `tests/driver-contract/tests/discovery_contract.rs` **两个文件同时出现**，且其**判定行区间互斥、已被显式写明**：
- `src/lib.rs`：§2.2 M1/M2 改 `:596-767` 与内嵌单测 `:1379-1814`；§2.3 冻结 `:430-523`、`:526-586`、`:769-1003`（另 `:170-331` 按 C10=D-A 冻结，见 M1 的「不得触碰」列）——**互不重叠**；
- `discovery_contract.rs`：§2.2 M8 **只新增**一条叶断言；§2.3 冻结既有断言行 `:255-260`、`:270-273`——**新增断言必须落在冻结行之外**。
其余 18 个文件**只出现在一个判定下**，不存在「同一文件同一区间被两个判定覆盖」的情况。

### 2.4 【不得触碰】的目录 / 文件

- `apps/mesa-web/**`（**全部**；含前端 383 用例与 `src/connection/**` 9 文件、`DescriptorFields.tsx`、`ApplyWithRestart.tsx`）——属 PR-C/PR-D。
- `drivers/s7/**`、`drivers/focas2/**`、`drivers/opcua/**`（P1 护栏：零改动）。
- `crates/core-types/src/resource.rs`、`schema.rs`、`descriptor.rs`、`event.rs`、`capability.rs`、`probe.rs`（P2/B4/B5/B6）。
- `crates/driver-protocol/**`、`proto/**`（**wire 语义冻结**，见 §7.2）。
- `crates/config-store/**`（`assign_point_ids` 与 tombstone 不复用逻辑 `crates/config-store/src/lib.rs:1655-1770` 冻结）。
- `crates/driver-sdk/**`、`apps/mesad/**`。
- 任何设备配置文件、证书目录、SQLite 数据库。

### 2.5 需要 Core 侧配合的事项（显式列出）

**结论：PR-B 不需要任何 Core 侧代码改动。** 逐条说明：

| 事项 | 是否需要 Core 改动 | 理由与影响面 |
|---|---|---|
| `nck.points.v1` 路由与信封层校验 | **不需要** | PR-A 已落地（`binding_gate.rs:30`、`point.rs:818-946`） |
| Endpoint-wide `point_key` 唯一性（跨 Task、跨 kind） | **不需要** | PR-A 已落地（`binding_gate.rs:112-155`、`point.rs:954`） |
| `point_id` 分配与 tombstone | **不需要** | 未改（`config-store` 冻结） |
| 身份推导（`canonical_identity`） | **不需要**（且**禁止**） | 修订 4：身份归 Driver；Core 侧无身份推导 API |
| `point_key` 通用格式校验 | **不需要** | PR-A 已落地（`point.rs:413`） |
| **`point_key` 的 256 字节上限是否足够覆盖 PR-B 的真实 Key** | **不需要**，但需 PR-B **实测确认** | 若 PR-B 在定义目录里引入比 §4.1.1 更长上限估算的 Key，必须在 PR 描述中给出实测最长 Key；**不得**因此要求放宽 Core 上限（§4.1.1 长度上限：超限即拒、不得截断） |

**若评审仍希望 Core 侧配合**，唯一被允许的形态是**独立「契约变更」PR**（§7.0-1/2），且必须列出：改哪个函数、影响的 4 个其它驱动、需要同步改写的契约套件、以及 `point.rs` 的 SHA-256 变化。**PR-B 内部不得夹带 Core 改动。**

---

## 3. 交付物②：接口输入输出

> 以下签名与上位设计 §2.4 / §2.4.1 / §2.5 / §2.6 **逐字对齐**；凡本文件做的收紧或修正，均以 `【PR-B 修正】` 显式标注并进入 §C 的裁决清单。

### 3.1 定义目录与 Revision / Digest（`definition.rs`）

> **【已裁决修正】C5：定义目录的存放形态在此**唯一固定**，全文（含 §1.2、§2.1、§4.3）一律以此为准：**
> - **形态 = 按 `revision` 分文件**：`drivers/sinumerik-nck/definitions/828d/r1.json`（未来 `r2.json`…）。
>   **理由（可验收）**：§3.1 的「已发布 `(variable_id, revision)` 不可原地修改」与 T6（对照基线分支）都要求「**已发布文件内容可逐字比对**」；单文件形态下新增一个 revision 必然改写同一文件，无法做「内容不变」判定，T6 的 3 条断言（改内容+同步改 Lock 仍失败 / 仅新增通过 / 删除失败）在单文件形态**无法全部落地**。
> - **位置 = 与 `src/` 平级**：`drivers/sinumerik-nck/definitions/`，**不在** `drivers/sinumerik-nck/src/definitions/` 下。
>   **理由（可验收）**：与之同构的既有先例是 `presets.rs:23` 的 `include_str!("../presets/828d.json")`（实测），因此定义目录用 `include_str!("../definitions/828d/r1.json")`；若放到 `src/definitions/`，数据文件会与 Rust 源码同目录，且相对深度与先例不一致。
> - **`DIGESTS.lock` 位置**：`drivers/sinumerik-nck/definitions/828d/DIGESTS.lock`（与 `r1.json` 同目录）。
> - **作废的旧表述**：上位设计 §1.1（`:192`）的 `drivers/sinumerik-nck/src/definitions/828d.json`（**单文件 + `src/` 下**）与 §7 PR-B 范围（`:1280`）的 `src/{…definitions/828d/*.json}` **作废**；§1.2 的「`definitions/828d.json`（单文件）」表述作废。上位设计 §3.1（`:754`）的**分文件形态被采纳**，其未写前缀的 `definitions/828d/r1.json` 补全为上述完整路径。**上位设计逐字不改**，差异记录于 §8.1 C5 行。

```rust
/// 定义载体版本（数据文件格式），与驱动语义版本解耦；进摘要。
pub const DEFINITION_FORMAT_VERSION: u32 = 1;
/// 最小合法 definition_revision（§3.1：从 1 开始，单调递增，不跳号不复用）。
pub const MIN_DEFINITION_REVISION: u32 = 1;

pub struct VariableDefinition {
    pub variable_id: String,                 // 业务语义 ID；稳定；不复用（§3.4）
    pub revision: u32,                       // >= 1；已发布不可改（§3.1）。**保持 u32**，与 §2.5 逐字一致
    pub family: String,                      // "828d"；"840d-sl" 无定义（§8.1-8）
    pub scope: DefinitionScope,              // Collect | Identity
    pub semantics_status: SemanticsStatus,   // Verified | Candidate | ParameterIdentity
    pub label: LocalizedText,                // 展示名（不含协议字段）
    pub source: Vec<DefinitionSource>,       // 手册 / 实测 / 身份探针 / 冻结向量
    pub targets: Vec<TargetKind>,            // 适用 Target 类型
    pub target_required: Vec<TargetKind>,    // 其中必须提供的
    pub arguments: Vec<ArgumentSpec>,        // 用户需输入的参数
    pub addressing: AddressingRule,          // 原生寻址规则（唯一定义处）
    pub decode: NckDataKind,                 // 固定 decode（native.rs:41-58 规范名）
    pub transport_size: u8,                  // 0x05/0x06/0x07/0x09
    pub count: u8,                           // 1..=255（线缆 line_count）
    pub unit_mode: NckUnitMode,              // 全部 Current
    pub unit: Option<String>,                // 展示单位（仅注释）
    pub invalid_rule: Option<InvalidRuleSpec>, // 如 r0035: -200 / 0.001
    pub identity_template: String,           // 规范身份模板（Driver 数据；§4.1.1 C）
    pub identity_keys: Vec<IdentityKeySpec>, // 身份所需参数键集合
    pub golden_ref: GoldenRef,               // 【PR-B 新增字段】语义 ID ↔ 62 条 golden 的绑定（§5.4）
    pub digest: String,                      // lowercase hex SHA-256(canonical JSON)（§3.2）
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DefinitionScope { Collect, Identity }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SemanticsStatus { Verified, Candidate, ParameterIdentity }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetKind { Channel, ModeGroup, Axis, Spindle, DriveObject }

pub struct ArgumentSpec {
    pub key: String,                 // ^[a-z][a-z0-9_]{0,63}$（§4.1.1 H-4）
    pub label: LocalizedText,
    pub value_type: ArgType,         // Integer | Enum
    pub required: bool,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub enum_options: Option<Vec<String>>,
    pub default: Option<serde_json::Value>,
}

/// 原生寻址规则：字段来源的唯一声明处（禁止在代码里散落 if）。
pub struct AddressingRule {
    pub area: NckArea,                    // 固定
    pub area_no: NativeFieldSource,
    pub module: NativeFieldSource,
    pub column: u16,                      // 固定
    pub line: NativeFieldSource,
}

pub enum NativeFieldSource {
    Fixed(u32),
    Target(TargetKind),
    Argument(String),
    ArgumentOffset(String, i32),
    DriveEntryModule,                     // entry=nc_variable→26；entry=drive_object→130
}

pub struct IdentityKeySpec { pub path: String, pub required: bool }

/// 【PR-B 新增】语义 ID ↔ 冻结 wire golden 的绑定（不复制 golden 内容，只存索引）。
pub struct GoldenRef {
    pub area: String,        // 冻结向量 "area"
    pub block: String,       // 冻结向量 "block"
    pub variable: String,    // 冻结向量 "variable"
    pub entry: DriveEntry,   // 冻结向量 "entry"：nc_variable | drive_object
}

pub struct DefinitionCatalog { /* BTreeMap<variable_id, BTreeMap<revision, VariableDefinition>> */ }

impl DefinitionCatalog {
    /// 编译期内嵌（include_str!，与 presets.rs:23 同模式）；装载即做全量自检。
    pub fn embedded() -> Result<Self, CatalogError>;
    /// 命中即返回；**miss 绝不回落最新**（§3.3）。
    pub fn lookup(&self, variable_id: &str, revision: u32) -> Result<&VariableDefinition, CompileError>;
    /// 错误消息三要素之一（§3.3）：该变量目录中可用的 revision 列表。
    pub fn available_revisions(&self, variable_id: &str) -> Vec<u32>;
    pub fn for_family(&self, family: &str) -> Vec<&VariableDefinition>;
    pub fn iter(&self) -> impl Iterator<Item = &VariableDefinition>;
    /// 重算全部已发布摘要并与 DIGESTS.lock 比对；不一致 → DEFINITION_DIGEST_MISMATCH。
    pub fn verify_digests(&self) -> Result<(), CatalogError>;
    /// canonical JSON（§3.2 五条规则）→ lowercase hex SHA-256。
    pub fn compute_digest(def: &VariableDefinition) -> Result<String, DigestError>;
}
```

**摘要算法（§3.2 逐条落地，不得依赖 `serde` 默认）**：顶层键**字节序升序**；数组**保序**；`digest` 字段本身不参与；浮点最短往返、非有限即报错；缺省可选字段**不出现**（不写 `null`）。先例同构于 `crates/event-store/src/lib.rs:235-249`。**PR-B 依赖**：`sha2`（`Cargo.toml:68` 已有 workspace 版本；驱动 `Cargo.toml:16-27` 需新增）。

### 3.2 Target：`TargetRef` / `DriveObjectRef` / `DriveEntry`（`targets.rs`）

```rust
/// 语义 target。**【已裁决修正】C1：这不是「恰好一个变体」的枚举**——它是
/// **按变量定义组合**的寻址坐标集合，合法字段组合由 `VariableDefinition.targets` /
/// `target_required` 声明（见下方 C1 校验表）；`{channel, axis}` 是**合法复合 Target**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetRef {
    pub channel: Option<u32>,
    pub mode_group: Option<u32>,
    pub axis: Option<u32>,
    pub spindle: Option<u32>,
    pub drive_object: Option<DriveObjectRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriveObjectRef {
    pub number: u32,       // 驱动访问编号（area_no 来源）
    pub entry: DriveEntry, // **必填、无默认值**
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveEntry { NcVariable, DriveObject }   // → module 26 / 130

impl DriveEntry {
    /// 入口身份 → module。**不允许折叠**（§2.4.2 D10）。
    pub const fn module(self) -> u8 { match self { Self::NcVariable => 26, Self::DriveObject => 130 } }
    pub const fn as_str(self) -> &'static str { match self { Self::NcVariable => "nc_variable", Self::DriveObject => "drive_object" } }
}

impl TargetRef {
    /// **【已裁决修正】C1**：按 `VariableDefinition` 声明的合法字段组合校验
    /// （不再是「变体数 = 1」）。完整规则见下方 C1 校验表 + D7。
    pub fn validate(&self, def: &VariableDefinition) -> Result<(), CompileError>;
    /// 已提供变体的实例号（供 `NativeFieldSource::Target` 取值）。
    /// **不得**在未提供时给出默认值（见 §3.2 的 C1 校验表第 5 条「不得自动猜测」）。
    pub fn instance(&self, kind: TargetKind) -> Option<u32>;
}
```

#### 【已裁决修正】C1：Target 的合法字段组合表（**唯一定义处**）

**裁决要点**：D7 原表述「`target` 变体数必须为 **1**」**作废**。Target 改为**按变量定义校验合法字段组合**：`{ "target": { "channel": 1, "axis": 3 } }` 是**合法的复合 Target**，**不得**因含两个字段而被拒绝。
**这些组合由 `VariableDefinition` 声明**（`targets` = 该变量适用的全部 Target 类型；`target_required` = 其中必须提供的类型），**不在代码里硬编码**：

| 变量类别 | 合法 Target | 由哪个定义字段声明 | 原生字段落点 |
|---|---|---|---|
| 通道变量 | `{channel}` | `targets:[Channel]`、`target_required:[Channel]` | `area_no = channel` |
| 方式组变量 | `{mode_group}` | `targets:[ModeGroup]`、`target_required:[ModeGroup]` | `area_no = mode_group` |
| 轴位置 | `{channel, axis}` | `targets:[Channel,Axis]`、`target_required:[Channel,Axis]` | `area_no = channel`、`line = axis` |
| 主轴变量 | `{channel, spindle}` | `targets:[Channel,Spindle]`、`target_required:[Channel,Spindle]` | `area_no = channel`、`line = spindle` |
| 驱动对象 | `{drive_object: {number, entry}}` | `targets:[DriveObject]`、`target_required:[DriveObject]` | `area_no = number`、`module = entry`(26/130) |

**Driver 必须拒绝**（逐条可执行；矩阵 #57–#62 逐条对应用例）：
1. **缺失字段**：`S ⊉ def.target_required` ⇒ `MISSING_TARGET`；
2. **多余字段**：`S ⊄ def.targets`（提供了该变量未声明的变体）⇒ `UNSUPPORTED_TARGET`；`target` 对象内出现 `TargetRef` 未定义的键 ⇒ serde `deny_unknown_fields` ⇒ `INVALID_BINDING_CONFIG`（与 D2 同口径）；
3. **错误字段类型**：`{"channel":"1"}` 等 JSON 类型不符 ⇒ `INVALID_BINDING_CONFIG`（与 D3 同口径）；`drive_object.entry` 缺失/类型错 ⇒ `INVALID_TARGET`；
4. **无效实例编号**：实例号 ∉ `0..=255` ⇒ `INVALID_TARGET`；
5. **不得自动猜测**：**绝不允许**自动补齐缺失的 Channel / Axis / DriveObject（无 `unwrap_or(1)`、无 `Default`、无「缺 axis 就当 axis=1」）。缺字段的唯一结果是**报错**，不是成功。

> **【已裁决修正】C1 边界（评审特别强调，必须原样写入）**：
> `{channel, axis}` 是 **NCK 读取的寻址坐标组合**，**不表示**已证明该机床轴与某个 SINAMICS 驱动对象的物理对应关系。
> 即：`axis` 只决定 NCK 读数地址的 `line` 段；`drive_object.number` 只决定驱动访问编号 `area_no`；二者在只读途径下**没有**被证明的映射关系（§6 表「驱动对象 `V/n` 的**轴归属**」一栏继续 HOLD）。

**Module 26 / 130 入口身份如何区分与校验（这是本项的核心，必须可验收）**：

1. **类型层强制**：`DriveObjectRef.entry` 为**必填、非 `Option`、无 `Default`** → 缺 `entry` 在 `serde` 反序列化阶段即失败 → `INVALID_TARGET`。**不存在**「默认 module 26」的隐式回退。
2. **映射层唯一实现**：`DriveEntry::module()` 是 26/130 的**唯一**出处；`NativeFieldSource::DriveEntryModule` 只允许调用它。
3. **常量对齐（构建期自检，可执行）**：`identity_spec.rs:177-189` 的 `DRIVE_OBJECT_NAME.module == 26`、`identity_spec.rs:194-205` 的 `DRIVE_OBJECT_NAME_M130.module == 130`，且两者 `module` **必须不同**。PR-B 必须新增单测 `drive_entry_module_matches_identity_spec_constants` 直接断言这三条。
4. **不可混淆的可执行反例**：同 `number` + 同 `column` 下，`entry=nc_variable` 与 `entry=drive_object` 必须产出**不同 module、不同 `wire_hex`、不同 canonical identity**（60 个字节组合中的 20 对，全部断言）。
5. **真机证据口径**：26 与 130 是对同一 `V/n` 返回**不同对象名**的两条入口（`identity_spec.rs:190-205` 注释、`docs/evidence/nck-remaining-20-reconciliation.md:57-74`），因此**目录中同一驱动参数必须存在两条 `entry` 变体**，不得折叠成一条带默认值的定义。

**【已裁决修正】D7：「变体数 = 1」作废，改为「按变量定义校验合法字段组合」**（C1，P0 已批准）。原表述「D7 的『变体数 = 1』必须改为『变体集合合法性』」保留于 §8.1 C1 行；**裁决后**的规则如下（与上位设计 §2.4.1 的 `TargetRef` 保持**可对应关系**：字段集合不变，只是不再限制个数）：

```text
target 合法 ⟺
  (a) 提供的变体集合 S = { k | target.<k> 非 null }（drive_object 计为一个变体）
      —— S 的**合法取值**由 def.targets 声明，例如 {channel, axis} 与 {channel, spindle} 都是合法 S
  (b) S ⊇ def.target_required                       // 缺失字段 ⇒ MISSING_TARGET
  (c) S ⊆ def.targets                               // 多余变体 ⇒ UNSUPPORTED_TARGET
  (d) S ≠ ∅                                         // 空 target ⇒ MISSING_TARGET
  (e) 每个 k ∈ S 的实例号 ∈ 0..=255（无效实例编号 ⇒ INVALID_TARGET），
      且 drive_object.entry 必填（缺失 ⇒ INVALID_TARGET）
结构/类型层（target 对象内未知键、JSON 类型错）⇒ INVALID_BINDING_CONFIG（D2/D3 同口径）
**禁止任何默认值**：不得自动猜测缺失的 Channel / Axis / DriveObject
```

**与上位设计 §2.4.1 `TargetRef` 的对应关系（不破坏上位设计的类型形状）**：上位设计 §2.4.1（`:437-462`）的 `TargetRef` 结构、`DriveObjectRef{number, entry}` 形状、`DriveEntry` 26/130 语义、以及 `:466` 的「`DriveEntry` 必填、无默认值」**全部保留**；本次裁决只废除其**文字约束中的「恰好一个变体」**（`:435` 的「Driver 负责其语义校验：变体数…」、`:438` 的注释「语义 target：恰好一个变体」）与 D7 的「变体数 = 1」（上位设计 `:482`、`:729`）。因此 Driver 侧类型与既有引用**无需改变形状**；**上位设计逐字不改**，差异记录于 §8.1 C1 行。

### 3.3 Address Compiler 的入口 / 出口（`compiler.rs`）

```rust
/// 唯一协议地址转换入口。
pub struct AddressCompiler<'a> {
    catalog: &'a DefinitionCatalog,
    family: &'a str,          // 设备 family；与 def.family 不匹配 → UNSUPPORTED_FAMILY
}

pub enum CompileInput<'a> {
    Semantic {
        variable_id: &'a str,
        definition_revision: u32,
        target: &'a TargetRef,
        arguments: &'a serde_json::Map<String, serde_json::Value>,
    },
    Native(&'a serde_json::Map<String, serde_json::Value>),
}

pub struct DefinitionRef { pub variable_id: String, pub revision: u32, pub digest: String }

pub enum IdentityMode { Derived, User }   // 仅 nck_native 可 User

pub struct CompiledPoint {
    pub address: NativeNckAddress,        // 复用 native.rs:117-132
    pub resolved: ResolvedPoint,          // 复用 native.rs:136-144（wire/kind/长度/类型/规则）
    pub source_label: String,             // 复用 native.rs:348-361 的稳定格式
    pub definition: Option<DefinitionRef>,// Semantic 时给出
    pub canonical_identity: String,       // Driver 重算的规范身份
    pub identity_mode: IdentityMode,
}

impl<'a> AddressCompiler<'a> {
    pub fn new(catalog: &'a DefinitionCatalog, family: &'a str) -> Self;

    /// 从 Core 信封点位（PR-A 类型）编译；`point_kind` = PointSelection.kind。
    /// 入参：`parameters`（不透明 JSON）+ 持久化 `point_key`（用于 D11 比对）。
    pub fn compile_point(
        &self,
        point_kind: &str,
        parameters: &serde_json::Map<String, serde_json::Value>,
        persisted_point_key: &str,
    ) -> Result<CompiledPoint, CompileError>;

    /// 语义 / 原生两条分支（§2.6 契约 3：Native **不得**被 Semantic 内部调用）。
    fn compile_semantic(&self, /* … */) -> Result<CompiledPoint, CompileError>;
    fn compile_native(&self, /* … */) -> Result<CompiledPoint, CompileError>;
}
```

**输入来源 / 输出去向（逐项）**：

| 接口 | 输入来源 | 输出去向 |
|---|---|---|
| `compile_point(kind, parameters, persisted_point_key)` | 来源 A：Core 信封 `DriverBinding.config.points[i]`（`PointSelection.kind` / `.parameters` / `.point_key`，`point.rs:105-112`） | 返回 `CompiledPoint` → `lib.rs` configure 组装 `PointSpec`（`lib.rs:721-731`）与 `PointDescriptor`（`:742-748`） |
| `compile_semantic` | 来源 A 的 `parameters`（`variable_id`/`definition_revision`/`target`/`arguments`）+ 来源 B：`DefinitionCatalog`（内嵌 `definitions/828d/r1.json`） | 12 字段 `parameters` map → **复用** `NativeNckAddress::from_parameters` |
| `compile_native` | 来源 A 的 `parameters`（12 原生字段 + 可选 `identity_mode`） | 同上（**【已裁决修正】C3：按 7 步流程剥离 `identity_mode` 后**传入 `from_parameters`，见下方「C3：`identity_mode` 的 7 步剥离流程」） |
| `DefinitionCatalog::lookup` | 来源 B（内嵌数据） | `compile_semantic` 的 `def`；miss → `UNSUPPORTED_DEFINITION_REVISION` |
| `identity_template` / `identity_keys` | 来源 B（`def`） | ① Driver 重算 `canonical_identity`；② 浏览/预填载荷（来源 C）的 `identity` 块 |
| 浏览/预填 `identity` 块 | 来源 B + 来源 C：`presets.rs` 叶（`leaf_node_with`，`presets.rs:160-186`） | 前端（PR-C）消费；**不得**进入持久化信封 |
| `CompiledPoint.resolved` | — | `lib.rs` configure → `PointSpec`；读取路径 `run`（`:792-1003`）→ `decode_point`（`:430-523`） |
| `CompiledPoint.source_label` | — | `PointDescriptor.source_label`（`:747`）→ Core 持久化（`config-store` R7 语义） |

**复用优先（禁止第二套 NCK 读取实现）——复用的具体函数清单**：

| 复用对象 | 位置 | 为什么要复用 |
|---|---|---|
| `NativeNckAddress::from_parameters` | `native.rs:230`（白名单 `:147-160`，未知字段拒绝 `:232`） | **零新增**解析代码；这是「62 条 golden 物理含义不变」的技术保证 |
| `resolve_native` | `native.rs:365-402` | wire / `expected_data_len` / `data_type` / `invalid_rule` 的唯一产出 |
| `NativeNckAddress::source_label` | `native.rs:348-361` | 稳定诊断格式（`nck://read/...`），不得拼接 `variable_id`（§4.2 末段） |
| `encode_var_spec` | `codec.rs`（经 `lib.rs:39` 再导出） | 10 字节线缆编码，62 条 golden 的字节真值 |
| `NckArea` / `NckUnitMode` / `parse_transport_size` / `NckDataKind` | `address.rs`、`native.rs:197-226`、`value.rs:97-140` | 枚举与规范名唯一词表 |
| `mesa_core_types::encode_typed_value` | `point.rs:207` | **建议复用**为 Driver 侧类型标签编码实现（Rust 侧**同一函数**，不可能漂移）。见 §C 无冲突；若评审要求 Driver 侧独立参考实现，则 PR-B 自实现并**用同一份 golden 向量**做双实现一致性断言 |
| `mesa_core_types::validate_point_key` | `point.rs:413` | Driver 侧 Key 语法复核（**不裁决身份**，§2.8 #9） |

**明确「不复用」的部分与理由**：

| 不复用 | 理由 |
|---|---|
| `GenericBinding` / `ResourceSelection` 解析路径（`lib.rs:633-641`） | PR-B 起 NCK 只接受 `nck.points.v1`；旧路径按 §5-1 明确拒绝 |
| `ResourceSelection` 的 `outputs[].point_key` 命名 | §4.1.2 第 3 条要求废弃「`variable.value` + 静默 `.2/.3`」 |
| `presets.rs` 的 `leaf_node` **绑定载荷**（`:167-171`） | 叶必须产出语义信封（M4）；但 `PresetTemplate.parameters` 与 `for_family`（`:106-114`）继续复用 |
| `identity.rs` / `identity_spec.rs` 的探针执行 | 身份浏览与采集解耦（1.3 冻结）；定义目录只**引用同值** |
| Core 的 `point.rs` 任何**身份推导** API | 不存在，且修订 4 禁止 Core 承担（§2.4 末段） |

#### 【已裁决修正】C3：`identity_mode` 的 7 步剥离流程（P0 已批准）

**裁决**：`identity_mode` 由 **Address Compiler** 负责处理；**不得**放宽 `NativeNckAddress::from_parameters` 的严格字段白名单（实测 `native.rs:147-160` 的 `ALLOWED` 恰 **12** 项、不含 `identity_mode`；`native.rs:231-235` 遇非白名单键即 `NativeError::UnknownField`）。**`native.rs` 逐字不动。**

```text
1. Driver 接收 `nck_native` 的 `parameters`（不透明 JSON 对象）。
2. Compiler 对**完整参数集合**做严格校验，**包括 `identity_mode`**（Compiler 侧 `#[serde(deny_unknown_fields)]`
   输入结构 = 12 原生字段 + 1 个可选 `identity_mode`；除此之外的键一律拒）。
3. Compiler 读取并校验 `identity_mode`：**默认 `derived`**，**仅允许 `derived` / `user`**；
   取值非法、或 `user` 用于 `nck_variable` ⇒ `INVALID_BINDING_CONFIG`。
4. Compiler 构建**只包含现有原生字段**（恰 12 项）的独立参数对象（**新 map，不就地改原 map**）。
5. 把该对象交给 `NativeNckAddress::from_parameters`（`native.rs:230`）——**零新增**解析代码。
6. 复用既有 NCK 地址解析与读取计划（`resolve_native` `native.rs:365-402`、`encode_var_spec`）。
7. 按 `identity_mode` 执行对应的身份验证：`derived` ⇒ 与重算 `canonical_identity` 逐字比对
   （不等 ⇒ `POINT_KEY_MISMATCH`）；`user` ⇒ 不重算，但复核通用语法与 `native.` 命名空间（B-1）。
```

> **【已裁决修正】C3 关键约束（必须原样保留）**：剥离**仅针对已经明确验证的 Compiler 元数据**（本 PR-B 只有 `identity_mode` 一个），**不得成为忽略未知字段的通用机制**——**非白名单内的其它未知键仍必须被拒绝**。可验收口径：`{"…12 字段…", "identity_mode":"derived", "whatever":1}` **必须失败**（`INVALID_BINDING_CONFIG`，因为第 2 步的 `deny_unknown_fields` 结构不认识 `whatever`），**不得**被「先剥离再交给原生解析器」这一手法顺手放过。

**测试 #22 必须同时证明两点（缺一不可）**：
- **(a) `identity_mode` 没有传入 Native Parser**：把含 `identity_mode` 的 13 字段 map **直接**交给 `from_parameters` ⇒ 必须返回 `UnknownField`（证明白名单未被放宽）；同时 Compiler 的 12 字段产出与「同 12 字段直接调用 `from_parameters`」逐字节相等（证明剥离结果里**没有**多出/少掉字段）。
- **(b) 最终地址与 Wire 字节与直接调用旧解析器的结果一致**：`compiled.resolved.wire` 与「同 12 字段直接 `from_parameters` + `resolve_native`」的 `wire` **逐字节相等**，`address` 全字段相等，`source_label` 逐字相等。

### 3.4 Canonical Identity 生成与 Configure 重算校验（D11）



```rust
impl VariableDefinition {
    /// canonical identity = identity_template 按 identity_keys 的叶路径做规范替换。
    /// 替换前的输入**必须**已经过 D1–D10（未知键与非法类型永远不进入身份计算，§4.1.1 D-1）。
    pub fn canonical_identity(
        &self,
        target: &TargetRef,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, CompileError>;
}

impl AddressCompiler<'_> {
    /// D11：唯一执行真值。derived ⇒ 逐字相等；user ⇒ 仅 nck_native，且必须已通过通用语法。
    pub fn check_identity(&self, persisted: &str, compiled: &CompiledPoint) -> Result<(), CompileError>;
}
```

- **`identity_mode="derived"`（默认，含全部 `nck_variable`）**：`persisted != compiled.canonical_identity` ⇒ `POINT_KEY_MISMATCH`，**configure 整体失败、任务不得启动**。
  错误消息**必须**同时含三要素（可执行断言：三个子串都在 `message` 里）：**持久化 Key**、**重算 Key**、**`variable_id` + `revision`**。
- **`identity_mode="user"`**：**仅** `nck_native` 允许；Driver **不重算、不静默改名**，但仍复核通用语法与命名空间（**本文件 §5.3 B-1**）。
- **`nck_variable` 不允许 `identity_mode`**（字段本身非法）或 `identity_mode="user"` ⇒ `INVALID_BINDING_CONFIG`。
- **`nck_native` 且 `identity_mode="derived"`** 的固定规则（§4.1.1 C 表末行）：`native.<area>.<area_no>.<module>.<column>.<line>`。**不含** `count`/`decode`/`transport_size`/`invalid_*`/`identity_mode`（§4.1.2 末段）。
- **顺序不可交换**：D1–D10 **先**、身份计算**后**（§4.1.1 D-1）。
- **`definition_revision` 不进入身份**（I3）：同一 `variable_id` 的所有 revision 身份逐字相同（T3b）。

### 3.5 【已裁决修正】C4：PR-B 唯一权威错误码表

> **本节是全文件、也是 PR-B 全范围唯一一份错误码表。** 它**取代**以下旧表述与其计数（原表述逐条保留在 §8.1 C4 行）：
> - 上位设计 §2.6 的 `CompileError` 表（`nck-semantic-point-refactor.md:677-691`，实测 **13** 行）；
> - 上位设计 §2.7 的 Driver 行（`nck-semantic-point-refactor.md:704-709`，实测**新增 8 个** Driver code）；
> - 上位设计 §5-7（`nck-semantic-point-refactor.md:1131`，写「新增 **7 个** Driver 码」）；
> - 本文件修订前的 §3.5 与 §C4 行的计数（旧文写「16 个 Driver code」）。**三个旧数字 7 / 8 / 13 互相矛盾，全部作废。**
>
> **口径**：以**实际 Compiler 入口（`compile_point`/`compile_semantic`/`compile_native`）与 §5.1 矩阵中实际使用的全集**为准，逐条核对后合并为下表。**不含**任何仅供描述、没有触发路径的码。
> **权威计数**：**21 个唯一 code 值**（全部 PR-B 可达）= **复用原 code 8 个** + **新增 13 个**；另有 **2 个**（`UNSUPPORTED_RESOURCE` / `UNSUPPORTED_OUTPUT`）NCK 侧**不再产生**，仅保留常量。**`MISSING_IDENTITY_KEY` 保留为独立码**（明确区分「`identity_keys` 声明的键不存在」这类配置错误，不与 `INVALID_ARGUMENTS` 合并）。
> **§3.6 的「失败码」列不是第二份码表**，只是 D 项 → 码的映射。

**A. `CompileError`（Address Compiler 入口 `compile_point` 返回；14 个）**

| # | code | `ErrorKind` | 触发 | fail-closed | 复用/新增 |
|---|---|---|---|---|---|
| A1 | `UNSUPPORTED_POINT_KIND` | `Unsupported` | Point Kind ∉ {`nck_variable`,`nck_native`}（D1） | 是 | 新增 |
| A2 | `UNKNOWN_VARIABLE` | `Address` | `variable_id` 不在目录（D5） | 是 | 新增 |
| A3 | `UNSUPPORTED_DEFINITION_REVISION` | `Configuration` | 目录无该 `(variable_id, revision)`（D6）；**不得回落** | 是 | 新增 |
| A4 | `UNSUPPORTED_FAMILY` | `Unsupported` | `def.family` ≠ 设备 family（如 840d-sl 却引用 828d 定义） | 是 | 新增 |
| A5 | `UNSUPPORTED_TARGET` | `Configuration` | 提供了 `def.targets` 之外的变体（D7/C1） | 是 | 新增 |
| A6 | `MISSING_TARGET` | `Configuration` | `S ⊉ def.target_required`（含 `S = ∅`）（D7/C1） | 是 | 新增 |
| A7 | `INVALID_TARGET` | `Configuration` | 实例号越界 / `drive_object.entry` 缺失或类型错（D7/D10） | 是 | 新增 |
| A8 | `INVALID_ARGUMENTS` | `Configuration` | 未知/缺失/越界/类型错的 argument；`ArgumentOffset` 上溢（D8） | 是 | 新增 |
| A9 | `MISSING_IDENTITY_KEY` | `Configuration` | `identity_keys` 声明的键**不存在**（§4.1.1 C：缺失即拒；显式 `null` 走 `INVALID_ARGUMENTS`） | 是 | 新增 |
| A10 | `POINT_KEY_MISMATCH` | `Configuration` | 持久化 `point_key` ≠ 重算身份（D11） | 是 | 新增 |
| A11 | `INVALID_USER_IDENTITY` | `Configuration` | `identity_mode="user"` 的 Key 不匹配 `native.` 命名空间（**B-1**，§5.3） | 是 | 新增 |
| A12 | `INVALID_BINDING_CONFIG` | `Configuration` | `parameters` 层结构与类型错（D2/D3，**含 `target` 对象内未知键与类型错**）、`identity_mode` 取值错或 `user` 用于 `nck_variable` | 是 | 保留原 code（`lib.rs:635-638`） |
| A13 | `INVALID_VARIABLE` | `Address` | 复用：`from_parameters` / `resolve_native` 失败（D9）、原生索引越界、`area_no:"1"` 字符串形态 | 是 | 保留原 code（`lib.rs:673`、`:681`） |
| A14 | `DEFINITION_DIGEST_MISMATCH` | `Internal` | 内嵌定义自检摘要不符（构建期问题） | 是 | 新增 |

**B. configure / 点位计划层（不属于 Compiler 入口；7 个）**

| # | code | `ErrorKind` | 触发 | fail-closed | 复用/新增 |
|---|---|---|---|---|---|
| B1 | `UNSUPPORTED_BINDING` | `Unsupported` | Task Binding Kind ≠ `nck.points.v1`（含旧 `mesa.resources.v1`） | 是 | 保留原 code（`lib.rs:614-620`） |
| B2 | `INVALID_TASK` | `Configuration` | `task.validate()` 失败（`lib.rs:611-612`） | 是 | 保留 |
| B3 | `MODE_NOT_SUPPORTED` | `Unsupported` | 非 Poll 调度（`lib.rs:624-631`） | 是 | 保留 |
| B4 | `DUPLICATE_POINT_KEY` | `Configuration` | 驱动侧同任务内 `point_key` 重复（`lib.rs:750-752`） | 是 | 保留 |
| B5 | `IDENTITY_NAMESPACE_COLLISION` | `Configuration` | **Driver 在该次 configure 可见的点位集合内**，两个点位的最终 `point_key` 相同（**B-2**，§5.3；C7=(a) 已批准）。**【复核补充】效力范围仅限 Driver 可见集合，\*\*不是\*\* Endpoint-wide 保证**——Endpoint-wide 唯一性由 Core 的 `DUPLICATE_POINT_KEY`（B4）唯一裁决 | 是 | 新增 |
| B6 | `MISSING_POINT_ID` | `Configuration` | `apply_point_map` 既有语义（`lib.rs:769-788`） | 是 | 不变 |
| B7 | `NOT_CONFIGURED` | `Configuration` | 同上（`apply_point_map`） | 是 | 不变 |

**C. NCK 侧不再产生（仅保留常量供其它驱动 / SDK 使用；0 个可达）**

| code | 说明 |
|---|---|
| ~~`UNSUPPORTED_RESOURCE`~~ / ~~`UNSUPPORTED_OUTPUT`~~ | `lib.rs:646`、`:655` 的返回点**删除**（C10=D-A 下资源面本身不变，但 NCK 不再走 `mesa.resources.v1` 路径）；常量本身保留 |

**权威计数核对（可机械复核）**：A 段 14 + B 段 7 = **21**；其中「保留原 code / 不变」= A12、A13、B1、B2、B3、B4、B6、B7 = **8 个**；「新增」= A1–A11 + A14 + B5 = **13 个**。8 + 13 = 21 ✅。


### 3.6 参数层严格校验 D1–D11 的落地位置

| # | 校验项 | 失败码 | 实现位置 |
|---|---|---|---|
| D1 | Point Kind ∈ {`nck_variable`,`nck_native`} | `UNSUPPORTED_POINT_KIND` | `compiler.rs::compile_point` |
| D2 | `parameters` 层未知字段（**【已裁决修正】C1：含 `target` 对象内部的未知键**，例如 `{"target":{"channel":1,"tool":2}}`） | `INVALID_BINDING_CONFIG` | `compiler.rs` 的两个 `#[serde(deny_unknown_fields)]` 输入结构（参数层 + `TargetRef`/`DriveObjectRef`） |
| D3 | 类型错误（**【已裁决修正】C1：含 `target` 字段的 JSON 类型错**，例如 `{"channel":"1"}`） | `INVALID_BINDING_CONFIG` | 同上（serde）+ `from_parameters`（12 字段） |
| D4 | 索引范围 | `INVALID_VARIABLE` / `INVALID_TARGET` | `from_parameters`（原生）/ `TargetRef::validate` |
| D5 | `variable_id` 存在性 | `UNKNOWN_VARIABLE` | `DefinitionCatalog::lookup` |
| D6 | `definition_revision` 可解析性（**不得回落**） | `UNSUPPORTED_DEFINITION_REVISION` | 同上 |
| D7 | **`target` 的合法字段组合**（**【已裁决修正】C1：不再是「变体数 = 1」**；见 §3.2 的 C1 校验表） | 缺失必填 ⇒ `MISSING_TARGET`；多余变体 ⇒ `UNSUPPORTED_TARGET`；实例号越界 / `entry` 缺失 ⇒ `INVALID_TARGET`；`target` 对象内未知键 / 类型错 ⇒ `INVALID_BINDING_CONFIG`（D2/D3 同口径） | `TargetRef::validate`（组合由 `VariableDefinition.targets` / `target_required` 声明） |
| D8 | `arguments` 键/类型/范围/偏移上溢 | `INVALID_ARGUMENTS` | `definition.rs` 的 `ArgumentSpec` 校验 + `NativeFieldSource::ArgumentOffset` 应用处 |
| D9 | 原生地址有效性 | `INVALID_VARIABLE` | **复用** `native.rs:230-345`（零新增） |
| D10 | Module 26/130 入口身份 | `INVALID_TARGET` + 构建期自检 | `DriveEntry::module()` + `drive_entry_module_matches_identity_spec_constants` |
| D11 | `point_key` 与重算身份一致性 | `POINT_KEY_MISMATCH` / `INVALID_BINDING_CONFIG` | `compiler.rs::check_identity` |

---

## 4. 交付物③：Golden 对应关系

### 4.1 三组 golden 的关系与不可破坏性

| 组 | 文件 | 规模（实测） | PR-B 关系 | 不可破坏性保证 |
|---|---|---|---|---|
| **G-A 既有 wire golden** | `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | **62**（42 `nc_variable` + 20 `drive_object`） | **逐字不改**；被**两道门**复用（旧模板路径 + 新语义路径） | ① §0 摘要逐字相等；② 旧门 `tests/legacy_equivalence.rs`（`checked == 62`，`:107`）**原样通过**；③ 新门 `tests/semantic_equivalence.rs` 同样 `checked == 62` |
| **G-B 身份编码 golden** | `tests/fixtures/nck-point-key-golden.json` | `value_cases` 55 + `value_reject_cases` 5 + `key_cases` 77 | **同一份复用**，PR-B **不得复制第二份** | ① §0 摘要逐字相等；② `include_str!` 路径与 PR-A 逐字相同（`point.rs:985-988` → Driver 侧 `concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/nck-point-key-golden.json")`，相对深度相同，已核实）；③ 新增断言：`grep -r "nck-point-key-golden" --include=*.rs --include=*.ts` 命中集合必须恰为 {Core(PR-A), Driver(PR-B), 前端(PR-C 时)}，且**不含**任何 `tests/fixtures/*golden*copy*.json` |
| **G-C PR-B 新增 golden** | 见 §4.3 | 见 §4.3 | 只新增**映射**与**身份**向量，不新增第二份 wire 真值 | 冻结方式：新增 fixture **带 `spec` 指向上位设计条款**，且每条用例**同时冻结字面量**（漂移即明确失败，而非静默通过） |

### 4.2 「62 条既有 wire golden 逐字保持不变」的具体做法

1. **文件级**：`drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` 的 SHA-256 必须等于 §0 值。建议在 PR 描述与 CI 里同时给出 `git diff --exit-code -- drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` 为空。
2. **旧门原样**：`tests/legacy_equivalence.rs` **一个字都不改**（实测 2 个用例，`checked == 62`）。
   ✅ **【已裁决修正】C2=P1（原为「⚠️ 前提」）：这不是「前提」，而是已裁决的冻结契约。** `presets/828d.json` 的 `parameters` **保持原生字段、逐字不动**，因为 `legacy_equivalence.rs:62-94` 把模板 `parameters` 当**原生字段**读并断言 `module/column/decode/transport_size/count` 与冻结向量一致（`:70-94` 逐条断言，实测）；`presets_828d.rs:71-227`、`:231-258`、`:265-305` 同样按原生 `parameters.module`/`area`/`invalid_value` 读。**原「两种说法不可兼得」的表述保留于 §8.1 C2 行。**
3. **复用为回归基线（新门，独立构造地址）**：`tests/semantic_equivalence.rs` 从**定义目录**出发（**而不是从模板**），经 `GoldenRef` 定位冻结条目，断言：
   - `checked == 62`（不得跳过、不得抽样）；
   - 每条 `encode_var_spec(&compiled.resolved.wire) == bytes(e["wire_hex"])`；
   - 20 条 `drive_object` 与其 `nc_variable` 孪生条目 **column 相同、module 不同、wire 不同**（G4）；
   - **语义 payload → 12 原生字段的映射与冻结条目逐字段一致**（`module`/`column`/`decode`/`transport_size`/`count`/`area`/`area_no`/`line`）。
4. **【已裁决修正】C2=P1：两条路径必须彼此独立、只共享真值**（这是本节的硬性结构要求）：
   - **旧路径**：`presets/828d.json` 原生模板 + 使用者确认的 `area_no`/`line` → `from_parameters`（即 `legacy_equivalence.rs`）；
   - **新路径**：`definitions/828d/r1.json` 的 `VariableDefinition` + `target`/`arguments` → 12 原生字段 → `from_parameters`（即 `semantic_equivalence.rs`）；
   - 两条路径**各自独立构造读取地址**，**最终与同一份冻结 Wire Golden 比较（62/62）**；
   - 两者**不得**在运行期互相调用（§3.3「明确不复用」表：`ResourceSelection` 路径不复用）。
5. **【已裁决修正】C2=P1：明确禁止「从旧模板自动反向生成新定义，再用同一数据自证等价」**（**自证循环**）。禁止的是：任何 `build.rs` / `tools/` 脚本 / 一次性 codegen 把 `presets/828d.json` 的 `parameters`（或 62 条 golden 向量）作为**输入**产出发行版 `definitions/828d/r1.json` 的 `addressing`/`targets`/`arguments`/`decode`/`transport_size`/`count`/`invalid_rule` 字段（或反过来用新定义生成旧模板），然后让两条路径读**同一份派生数据**去「证明」彼此等价——那样新门只是旧门的复读，**不构成独立证据**。
   **可执行的检测方式（4 条，全部机械可查；落地为矩阵 #63）**：
   - ① **源码级**：`compiler.rs` / `definition.rs` 对 `PresetRegistry`、`PresetTemplate`、`EMBEDDED_828D`、`presets/` 路径**零引用**（`grep -n` 断言命中集合为空）；`semantic_equivalence.rs` 的 `use` 列表**不含** `PresetRegistry`（编译期即不可能读模板）；
   - ② **来源级**：`r1.json` 每条定义的 `source` 必须指向**手册 / 实测 / 身份探针 / 冻结向量**（`legacy-828d-wire.json`），**不得**出现 `presets/828d.json` 或任何模板文件路径（逐条断言）；
   - ③ **构建无副作用**：`cargo build --locked --workspace`（G4）之后 `git status --short -- drivers/sinumerik-nck/definitions` 必须为空——发行定义目录**只能由人工编写、随 PR 提交**，不得由构建期生成或改写；
   - ④ **独立复现方向（评审验收步骤，非 CI 门禁、不改仓库文件）**：把仓库**复制到临时目录**，在副本里删除 `drivers/sinumerik-nck/presets/828d.json`，再在副本中运行 `cargo test -p mesa-driver-sinumerik-nck --test semantic_equivalence`——新门**仍然必须** `checked == 62`。这是「新门不依赖旧模板」的直接证明；若新门在模板缺失时失败或数目变化，即说明新定义是模板的派生物（自证循环），本次验收不通过。

### 4.3 PR-B 新增的 golden（若有）+ 为什么必须新增

| 新增 golden | 内容 | 为什么**必须**新增（不能靠既有向量替代） | 冻结方式 |
|---|---|---|---|
| `drivers/sinumerik-nck/definitions/828d/DIGESTS.lock` | `(variable_id, revision) → digest` + 发行文件路径 | §3.1 的「已发布不可原地修改」**只能**由摘要清单落地；它不是测试数据而是**契约数据** | 由 T6 对照基线分支校验；只允许新增条目 |
| `drivers/sinumerik-nck/tests/fixtures/semantic-identity-golden.json`（**新增**，建议） | 42 条定义的**默认 Key 字面量**（按 §2.5.1 的 target/arguments 取值） | ① §4.1.1 K 只给了 7 个示例，覆盖不了 42 条；② 身份**字面量**是「同一 `variable_id` 跨 revision 身份不变」与「A 项完整性」的唯一可判据；③ 与 wire golden 不同：wire golden 冻结**字节**，本文件冻结**身份字符串**，两者不可互推 | 每条同时冻结 `canonical_identity` 字面量；断言 `identity_template`/`identity_keys` 跨 revision 逐字相同 |
| `drivers/sinumerik-nck/tests/fixtures/semantic-golden-map.json`（**可选**） | `variable_id → GoldenRef` | 若 `GoldenRef` 已内嵌在 `r1.json` 中（本文件推荐），则**不需要**此文件——避免双份真值 | 若采纳，必须断言它与 `r1.json` 的 `golden_ref` **双射一致** |

**明确不新增**：**不新增第二份 wire goldens**（`semantic_equivalence.rs` 只能读 `legacy-828d-wire.json`）；**不复制** E1–E11 向量文件。

### 4.4 「语义 Variable ID → 既有 62 条 golden」的映射方式（可验证）

**映射载体**：每一条 `VariableDefinition` 携带 `golden_ref: GoldenRef{area, block, variable, entry}`（N4 `r1.json`），其取值直接取自冻结向量 `legacy-828d-wire.json` 的对应字段（**只存索引，不复制 wire 字节**）。

**可验证性协议（4 条断言，全部机械可查）**：

1. **满射（语义 → golden）**：42 条定义共产生 62 个 `GoldenRef`（A 16×1 + B 20×2 + C 6×1 = 62，§2.5.1 计数校验），且每一个都能在冻结向量的 62 条里**唯一命中**（用 `(area, block, variable, entry)` 四元组）。
2. **单射（golden → 语义）**：冻结向量的 **62 条每一条**都必须被**至少一个** `GoldenRef` 命中；未命中条目数必须为 **0**。
3. **双射（无重叠）**：`GoldenRef → 定义` 的映射必须恰为 62：定义 A/C 的条目各对应 1 条，定义 B 的 20 条各对应 2 条（同 `entry` 维度拆开），断言「同一 `(variables_id, entry)` 不重复绑定同一条 golden」，且断言 20 条 `drive_object` 与 20 条 `nc_variable` 绑定到**不同**的冻结条目。
4. **字节一致**：每条 `GoldenRef` 经 Compiler 编译后的 `wire` 与冻结 `wire_hex` **逐字节相等**（即 §4.2 第 3 条的 62 条断言）。

**另加「模板 id ↔ golden」的可验证交叉（不引入第二份真值）**：
`presets/828d.json` 的模板 id 实测形态为 `828d.<area>.<block>.<variable>.<nc_variable|drive_object>`（§1.4），恰好可由 `GoldenRef` 四元组**无损重建**。因此新增断言：
- 62 个 `GoldenRef` **一对一**映射到 62 个模板 id；
- **【已裁决修正】C2=P1（已采纳）**：该断言同时证明「旧模板集合」与「新定义目录」**逐个对应**，且**不需要**改动模板文件——这是本次裁决选择的方案，落地为矩阵 #7 的一部分。
- **【已裁决修正】C2 的未采纳备选 P2（保留可追溯，附理由）**：P2 = 把 `presets/828d.json` 语义化（模板 `parameters` 改为语义 `target`/`arguments`）。**未采纳**，理由（逐条、可复核）：
  1. 必须**同时**重写 `legacy_equivalence.rs`（实测 2 用例、`:62-94` 读原生 `parameters`）与 `presets_828d.rs`（实测 6 用例、`:71-227`/`:231-258`/`:265-305` 读 `parameters.module`/`area`/`invalid_value`），共 **8 个用例**必然失败 —— 直接摧毁「原样通过」的旧门。
  2. 更关键：P2 会**削弱真值来源的独立性**。`legacy_equivalence.rs:12` 的注释写明「用**冻结的旧数据**做真值，不是用新代码自证新代码」；一旦模板被改写为语义载荷、且新定义与旧模板由同一批作者按同一套映射同步改写，两条路径就退化为**同一份派生数据**的两面 —— 这正是 §4.2 第 5 条禁止的自证循环。
  3. P1 反过来还**多**提供一条独立证据：冻结的原生模板与新建的语义目录**分别**编译到同一份 62 条 wire golden（两条路径独立构造地址，见 §4.2 第 4 条）。

---

## 5. 交付物④：验收矩阵

### 5.1 测试矩阵（测试名 / 断言要点 / 所在文件与套件 / 证明什么 / 对应上位设计规则）

> 「套件」列中：**驱动自有目标** = `cargo test -p mesa-driver-sinumerik-nck --test <name>`；**契约套件** = `cargo test --locked -p mesa-contract-tests --all-features`（29 套之一，`scripts/contract_suites.py:13-52`）。
> **【已裁决修正】本矩阵最终行数 = 63 行**（原 56 行 + C1 的 6 行 #57–#62 + C2 禁自证的 1 行 #63）。**既有编号 1–56 一律不重排**（保持与上一轮评审的编号可追溯），新增用例**追加**为 #57–#63。
> **【已裁决修正】同步性口径**：矩阵中每一条涉及测试的用例，其「所在文件与套件」都必须出现在 §2 的文件范围清单里（§2.1 新增 / §2.2 修改 / §2.3 逐字不动）；反之 §2 中每个涉及测试的文件都必须在矩阵中有行。逐项对照见 §5.1 末的「矩阵 ↔ 文件范围同步性核对表」。

| # | 测试名 | 断言要点 | 所在文件与套件 | 证明什么 | 上位设计条款 |
|---|---|---|---|---|---|
| 1 | `catalog_has_42_definitions_at_revision_1` | 恰 42 条定义、revision 全为 1、`(variable_id, revision)` 唯一；A16+B20+C6 计数 | `tests/definition_catalog.rs`（驱动自有） | 定义目录完整、无重复 | §2.5.1 计数校验 |
| 2 | `variable_id_shape_and_reserved_prefix` | `^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$`；**不得等于 `native`、不得以 `native.` 开头** | 同上 | 保留前缀规则（修正原第 9 条） | §4.1.1 E |
| 3 | `semantics_status_governs_id_shape` | 19 条 `parameter_identity` 且 ID 恒为 `drive.parameter.<rNNNN>`；`r0031`/`r0033` = `candidate`；`r0035` = `verified`；**不得出现** `drive.motor.` / `drive.current.` 等编造名 | 同上 | 防语义虚构 | §2.5.2、§8.1-6、§8.1-7 |
| 4 | `identity_template_and_keys_identical_across_all_revisions` | 同一 `variable_id` 的所有 revision 的 `identity_template`/`identity_keys` **逐字相同** | 同上 | 改模板 = 新 `variable_id` | §3.2、§3.5、T3b |
| 5 | `identity_keys_cover_every_addressing_source` | **A 项静态判据**（§5.2）：`addressing` 中每个 `Target(k)`/`Argument(a)`/`ArgumentOffset(a,_)` 都被 `identity_keys` 覆盖；`DriveEntryModule` 被 `{drive_object}` 覆盖 | 同上 | **影响目标的参数没有遗漏**（可执行证明） | §4.1.1 I1/I2 |
| 6 | `identity_template_placeholders_equal_identity_keys_leaves` | 模板占位符集合 == `identity_keys` 叶路径集合；占位符**按字节序升序**、无重复 | 同上 | 模板与键集合一致、段序规范 | §4.1.1 C、§4.1.1 排序规则 |
| 7 | `golden_ref_is_a_bijection_with_62_wire_vectors` | §4.4 的满射 + 单射 + 双射三条；`GoldenRef` 四元组与 62 个模板 id 一对一 | 同上 | 62 条 golden 全覆盖、无重复绑定 | §6.5、§3.4、G1/G4 |
| 8 | `scope_identity_definitions_match_identity_spec_constants` | 6 条 `scope=Identity` 的 area/module/column/decode 与 `identity_spec.rs:93-176` 常量**逐字段相等** | 同上 | 定义目录与真机证据探针不漂移 | §2.5.1 C、N16 |
| 9 | `drive_entry_module_matches_identity_spec_constants` | `DriveEntry::NcVariable.module() == 26`、`DriveObject.module() == 130`、两者不等；且分别等于 `DRIVE_OBJECT_NAME.module` / `DRIVE_OBJECT_NAME_M130.module` | 同上 | **26/130 入口身份不可折叠**（D10） | §2.4.2 D10、N16、G8 |
| 10 | `digest_matches_lock_and_is_canonical_json_sha256` | 每条定义重算摘要 == `DIGESTS.lock`；算法为 lowercase hex SHA-256(canonical JSON) | 同上 | 摘要冻结 | §3.2、N20 |
| 11 | `digest_ignores_its_own_field_and_absent_optionals` | `digest` 字段自身不参与；缺省可选字段不写 `null`；数组保序、顶层键字节序升序 | 同上 | canonical 化五条规则 | §3.2 |
| 12 | `lookup_missing_revision_never_falls_back` | miss 返回 `Err`；`available_revisions` 列出可用列表；**绝不**返回最新或其它 revision | 同上 | 「绝不自动取最新」的**静态**证据 | §3.3 三种禁止退化 |
| 13 | `t3b_recomputed_identity_is_revision_independent` | 对 42 条定义的**每一个已发布 revision**：同 target/arguments ⇒ `canonical_identity` 逐字相等 | `tests/compiler_identity.rs`（驱动自有） | T3b：身份与 revision 无关 | §6.6 T3b、R8、I3 |
| 14 | `t3b_changed_target_or_arguments_changes_identity` | 改任一 target 实例或任一 argument ⇒ 身份**不同** | 同上 | R9/R14 反向 | §6.6 T3b、R9/R14 |
| 15 | `t4_configure_fails_whole_batch_on_point_key_mismatch` | 3 点任务中 1 点 Key 不符 ⇒ `POINT_KEY_MISMATCH`，**configure 返回 Err、无任何点位进入计划**（断言 plan 仍为 `None`）、任务**不得启动**；消息含「持久化 Key」「重算 Key」「`variable_id` + `revision`」 | 同上（+ `lib.rs` 侧 e2e） | **T4 唯一执行真值** | §2.4.2 D11、§4.1.1 D |
| 16 | `t4_matching_key_succeeds` | Key 逐字相等 ⇒ configure 成功，`PointDescriptor` 数与点位数一致 | 同上 | 正例 | §2.4.2 D11 |
| 17 | `t4_identity_mode_user_for_nck_variable_rejected` | `nck_variable` + `identity_mode="user"` ⇒ `INVALID_BINDING_CONFIG` | 同上 | user 仅原生 | §2.3、§2.4.2 D11 |
| 18 | `t4_user_mode_does_not_recompute_nor_rename` | `nck_native`+`user`：改地址字段后 `point_key` 不变（复现 R13）；Driver 不追加 `~<n>`、不改写 Key | 同上 | 不静默改名 | 附加 1、§4.2 R13、§4.1.2 第 2/3 条 |
| 19 | `identity_key_missing_is_rejected_not_defaulted` | `identity_keys` 声明的键缺失 ⇒ `MISSING_IDENTITY_KEY`；显式 `null` ⇒ `INVALID_ARGUMENTS`；定义声明了 `default` 也**不得物化** | 同上 | 缺省/`null`/空串三态在身份层被拒 | §4.1.1 C、§4.1.1 H-6、E4 |
| 20 | `point_kind_unknown_rejected` | `kind` 非两值 ⇒ `UNSUPPORTED_POINT_KIND` | 同上 | D1（Core **放行**，Driver 拒） | §2.4.2 D1、§2.8 #5 |
| 21 | `parameter_layer_unknown_field_rejected` | `parameters` 内未知键 ⇒ `INVALID_BINDING_CONFIG`（两条分支各一例） | 同上 | D2 | §2.4.2 D2 |
| 22 | `identity_mode_is_not_forwarded_to_native_parser` **【已裁决修正】C3：两条断言都必须写明并分别成立** | **(a) `identity_mode` 没有传入 Native Parser**：① 含 `identity_mode` 的 **13 字段** map **直接**交给 `from_parameters` ⇒ 必须返回 `UnknownField`（证明 `native.rs:147-160` 白名单**未被放宽**）；② Compiler 剥离后交给 `from_parameters` 的 map **恰为 12 项**、不含 `identity_mode`（逐键断言）。**(b) 结果与直接调用旧解析器一致**：`compiled.resolved.wire` 与「同 12 字段直接 `from_parameters` + `resolve_native`」的 `wire` **逐字节相等**，且 `address` 全字段、`source_label` **逐字相等** | 同上 | **复用而非复制**；`native.rs` 逐字不动；「剥离只针对已明确验证的 Compiler 元数据」 | §2.6 契约 2/3、**§3.3 C3 7 步流程**、§C3 |
| 23 | `unsupported_definition_revision_rejects_all_three_degradations` | ① 不回落最新；② 不回落其它 revision；③ 不按 `variable_id` 猜地址（三例分别断言） | 同上 | §3.3 三种禁止退化 | §3.3 |
| 24 | `argument_offset_boundaries` | `channel.r_parameter` `index=0` ⇒ `line=1`；`index=65535` ⇒ 上溢 ⇒ `INVALID_ARGUMENTS`；`index=-1` ⇒ 拒 | 同上 | `ArgumentOffset` 边界 | §2.5.1 A#16、§2.4.2 D8 |
| 25 | `a_identity_completeness_channel` | 同 `variable_id`、同其余参数，`channel=1` vs `channel=2` ⇒ **身份不同** 且 **物理地址 `area_no` 不同** | 同上 | **A 项反例：Channel** | §4.1.1 I1 |
| 26 | `a_identity_completeness_mode_group` | `mode_group=1` vs `2` ⇒ `area_no` 不同 ⇒ 身份不同 | 同上 | **A 项反例：ModeGroup** | §4.1.1 I1、§8.1-2 |
| 27 | `a_identity_completeness_axis` | `axis=1` vs `2`（`channel` 相同）⇒ `line` 不同 ⇒ 身份不同 | 同上 | **A 项反例：Axis** | §4.1.1 I1、表 A#7/#8 |
| 28 | `a_identity_completeness_spindle` | `spindle=1` vs `2`（`channel` 相同）⇒ `line` 不同 ⇒ 身份不同 | 同上 | **A 项反例：Spindle** | §4.1.1 I1、表 A#12–14、§8.1-4 |
| 29 | `a_identity_completeness_drive_object` | `drive_object.number=3` vs `4`（`entry` 相同）⇒ `area_no` 不同 ⇒ 身份不同 | 同上 | **A 项反例：DriveObject** | §4.1.1 I1、表 B |
| 30 | `a_identity_completeness_drive_entry_26_vs_130` | 同 `number`、同 `column`，`entry` 26 vs 130 ⇒ `module` 不同 ⇒ 身份不同、wire 不同 | 同上 | **A 项反例：入口身份**；26/130 不折叠 | §2.4.2 D10、G4、§6.5 |
| 31 | `a_identity_distinguishes_exactly_the_physical_target` | **机械差分（最强）**：对 42 条定义，枚举每个 `addressing` 来源的取值域样本，断言 `canonical_identity(A) == canonical_identity(B)` **当且仅当** 解析出的原生地址元组 `(area, area_no, module, column, line, count)` 相等 | 同上 | **A 项完整性判据的动态证明**：不存在「物理目标不同却同身份」的定义 | §4.1.1 I1/I2 |
| 32 | `b_user_key_must_stay_in_native_namespace` | `identity_mode="user"` 且 `point_key` 不以 `native.` 开头/不匹配 `native_key` 语法 ⇒ `INVALID_USER_IDENTITY`（含 `point_key="channel.r_parameter#index=i:100@channel=i:1"` 一例）。**【复核补充】只断言前缀命名空间的\*\*语法强制\*\*，\*\*不涉及唯一性\*\***（既不是任务内、也不是 Endpoint 级） | 同上 | **B-1**：不得冒用语义点位身份 | 本文件 §5.3、§4.1.1 E |
| 33 | `b_user_key_collision_within_batch_rejected` | **【复核补充】效力范围 = Driver 在该次 configure 可见的点位集合内**：同批次两点 A `derived`（地址 `C/1/21/1/396`）与 B `user`（`point_key="native.C.1.21.1.396"`，地址 `C/1/21/1/999`）⇒ `IDENTITY_NAMESPACE_COLLISION`，configure 整体失败。**禁止**把本用例写成「Endpoint 级 / 跨 Task 唯一性已验证」——Driver **看不到**其它 Task 的点位；**Endpoint-wide 唯一性以 Core 契约为唯一权威** | 同上 | **B-2**：不同物理目标不得共享身份（**批次内提前反馈**，**非** Endpoint 级保证） | 本文件 §5.3（含「B-2 的视野边界」行） |
| 34 | `b_user_key_cannot_shadow_semantic_identity` | B-1 + 结构论证：语义身份**恒含 `@`**（I1）且不以 `native.` 开头，`native.` 前缀在 Core 侧被强制为 `native_key` 形态（`point.rs:428-431`）⇒ 结构上不可能与语义 Key 相同；本用例用一条**试图**冒用的 Key 断言被拒。**【复核补充】这是结构性论证，\*\*不构成 Endpoint-wide 去重保证\*\*** | 同上 | **B**：命名空间隔离是**结构性**的，不靠事后判重 | §4.1.1 E、§4.1.2 |
| 35 | `t5_encoder_matches_e1_e11_golden_vectors` | 复用 `tests/fixtures/nck-point-key-golden.json` **同一份**：`value_cases`/`value_reject_cases`/`key_cases` 逐条与 Driver 编码/校验路径一致 | 同上 | T5（Driver 侧）三处同源 | §6.6 T5、§4.1.1 J |
| 36 | `t1_mixed_point_kinds_route_per_point` | 一个任务的 `points[]` 同时含 `nck_variable` 与 `nck_native` ⇒ configure **成功**，逐点路由到两条分支（用点位级断言证明路由，而不是只看总数） | 同上 | T1（Driver 侧） | §2.1 评审裁决、§6.6 T1 |
| 37 | `entry_missing_is_invalid_target` | `drive_object` 缺 `entry` ⇒ `INVALID_TARGET`；**无默认 module 26** | 同上 | `DriveEntry` 必填 | §2.4.1、§6.2、**§3.2 C1 表** |
| 38 | `semantic_path_reproduces_all_62_frozen_wire_bytes` | 从定义目录 + `GoldenRef` 出发，62 条逐条 `encode_var_spec == wire_hex`；`checked == 62` | `tests/semantic_equivalence.rs` | 62/62 字节等价（新门） | §3.4、§6.2、§6.5 |
| 39 | `semantic_path_keeps_the_two_entries_apart` | 20 对：column 相同、module 不同、wire 不同 | 同上 | G4 不折叠 | §6.5、G4 |
| 40 | `legacy_path_still_reproduces_62` | `tests/legacy_equivalence.rs` **原样**通过（`checked == 62`，实测 `:107`；共 2 用例） | `tests/legacy_equivalence.rs`（**逐字不动**，§0 摘要 `44e429bb…`） | 旧门未失效 | §6.2、1.3、**【已裁决修正】C2=P1** |
| 41 | `templates_never_prefill_instance_or_line` 等 6 个 | `tests/presets_828d.rs` **原样**通过（实测 6 用例） | `tests/presets_828d.rs`（**逐字不动**，§0 摘要 `4eb747b5…`） | 模板未被偷偷改动 | §6.2、1.3、**【已裁决修正】C2=P1（模板逐字不动，不再是「前提」）** |
| 42 | `configure_accepts_nck_points_envelope` | `nck.points.v1` 信封 ⇒ configure 成功 | `src/lib.rs` 内嵌单测（改写 `:1444`） | 新入口可用 | §7 PR-B |
| 43 | `configure_rejects_legacy_resource_selection` | 旧 `mesa.resources.v1` + `variable` 资源 ⇒ `UNSUPPORTED_BINDING` | 同上 | §5-1 破坏性变更已落实 | §5-1、§1.4 |
| 44 | `descriptor_is_valid_and_read_only` **【已裁决修正】C10=D-A：由「改写」改为「原样通过」** | 保留：driver_id、`poll`、**无 subscribe**、**无 write**、**无 events**、无 commands、descriptor < 256KB、`resource_selection_methods` 含 `Browse`（`lib.rs:1163-1174`，实测）——全部继续成立，**不改写** | `src/lib.rs` 内嵌单测（`:1163-1174`，**不改写**） | 只读能力与 Descriptor 面均不得放宽 | §6.2、**§C10** |
| 45 | 迁移后的 poll/e2e 用例（`:1523`、`:1602`、`:1662`、`:1691`、`:1740`、`:1768`，共 6 个） | 语义信封下的端到端轮询、BAD→heal、会话丢失、transport 不匹配、超 PDU 拒绝、多任务失败 | 同上 | 执行面零回归 | P5/P6、`GATE.md:211-214` |
| 46 | `t6_changed_definition_with_synced_lock_still_fails` | 临时仓库：改定义内容 + 同步改 Lock ⇒ 退出码非 0 | `tools/ci/test_nck_definition_lock.py` | **T6-①**：同步改两者也拦得住 | §3.1 附加 2 |
| 47 | `t6_added_revision_passes` | 临时仓库：仅新增 `(variable_id, revision)` ⇒ 退出码 0 | 同上 | **T6-②**：只允许新增 | §3.1 附加 2 |
| 48 | `t6_deleted_revision_fails` | 临时仓库：删除已发布 revision ⇒ 退出码非 0 | 同上 | **T6-③**：不得删除 | §3.1 附加 2 |
| 49 | `t6_current_tree_is_self_consistent` | 当前树重算摘要 == 当前 Lock | 同上 | 现网自洽 | §3.2 |
| 50 | `data_semantics` 的 8 个语义用例（`:742`、`:755`、`:793`、`:820`、`:838`、`:852`、`:872`、`:891`） | 原样通过 | 契约套件 `data_semantics` | T2/T3a/Core 身份只由 `point_key` 决定 | §6.6 T2/T3a |
| 51 | `core_has_zero_siemens_knowledge` | 原样通过 | 契约套件 `resource_contract`（`:396-460`） | P9：PR-B 未把 Siemens 知识塞进 Core | §5.1 P9 |
| 52 | ~~`descriptor_contract` 的两处改写（`:605`、`:729-731`）~~ **【已裁决修正】C10=D-A：改为「整文件原样通过」** | `descriptor_contract.rs` **逐字不动**（§0 摘要 `382064d3…`，1094 行）：`:605`（`variable` 资源存在）、`:729-731`（`outputs[0].type_spec == DriverResolved`）、`:599-604` 只读五条、`:391-399` 全部**原样通过** | 契约套件 `descriptor_contract`（**逐字不动**） | 能力边界不退化；原「两处改写」**撤销**（§2.2 M7） | §6.2、**§C10** |
| 53 | `browse_sinumerik_nck_catalog_tree_e2e`（原样）+ **新增**一条叶形态断言 | 根 2 节点 / `nck://presets` / 诊断节点 / 未知 parent 空页 **不变**（`:255-260`、`:270-273`，**不得改写**）；**新增**：叶 `binding_json` 为语义信封且不含原生字段 | 契约套件 `discovery_contract`（文件 `:224-274`；**仅新增**，既有断言行冻结） | 浏览语义重做、原有断言不退化 | §6.2、§1.2、**【已裁决修正】C6** |
| 54 | 其余 27 套契约套件 | 原样通过 | 契约套件（`scripts/contract_suites.py:13-52`） | 非 NCK 能力零回归 | §5.1 P1/P8 |
| 55 | `point.rs` 的 17 个单测（含 `e1_e11_golden`、`core_has_no_identity_derivation`） | 原样通过 | `crates/core-types` 单测 | PR-A Core 契约未被破坏 | §2.4、§6.1 |
| 56 | 前端全部用例 | 原样通过（**PR-B 零改动前端**） | `pnpm --filter mesa-web test` | 前端零回归 | §5.1 P3/P4、§7 PR-B |
| **57** | `target_composite_channel_axis_is_legal` **【已裁决修正】C1 新增** | `{"target":{"channel":1,"axis":3}}` ⇒ **编译成功**（**不得**因含两个字段被拒）；且解析结果 `area_no == channel == 1`、`line == axis == 3`（与 §2.5.1 A#7/#8 一致）；同类再断言 `{"channel":1,"spindle":2}` ⇒ `line == spindle` | `tests/compiler_identity.rs`（驱动自有，N7） | **C1 主证据**：复合 Target 合法、字段组合由定义声明 | **§3.2 C1 校验表**、§2.4.2 D7 |
| **58** | `target_missing_required_field_rejected` **【已裁决修正】C1 新增** | 逐条：`{"target":{}}` ⇒ `MISSING_TARGET`；`{channel,axis}` 定义只给 `{channel}` ⇒ `MISSING_TARGET`；`{channel,spindle}` 定义只给 `{spindle}` ⇒ `MISSING_TARGET` | 同上 | **C1-1**：缺失字段必拒 | **§3.2 C1 校验表**、D7 |
| **59** | `target_extra_field_rejected` **【已裁决修正】C1 新增** | ① 语义多余：通道变量给 `{"channel":1,"axis":3}` ⇒ `UNSUPPORTED_TARGET`；② 结构多余：`{"channel":1,"tool":2}`（`TargetRef` 无 `tool`）⇒ `INVALID_BINDING_CONFIG`（`deny_unknown_fields`） | 同上 | **C1-2**：多余字段两种形态都被拒 | **§3.2 C1 校验表**、D2、D7 |
| **60** | `target_field_type_error_rejected` **【已裁决修正】C1 新增** | `{"channel":"1"}`（字符串）⇒ `INVALID_BINDING_CONFIG`；`{"drive_object":{"number":1}}`（缺 `entry`）⇒ `INVALID_TARGET`；`{"drive_object":{"number":1,"entry":"m130"}}`（枚举值非法）⇒ `INVALID_BINDING_CONFIG` | 同上 | **C1-3**：类型错必拒，且码归属唯一（D2/D3 vs D7 不重叠） | **§3.2 C1 校验表**、D3、D7、§3.5 |
| **61** | `target_invalid_instance_number_rejected` **【已裁决修正】C1 新增** | 实例号 `256`、`-1`、`4294967296` 逐条 ⇒ `INVALID_TARGET`；`0` 与 `255` 为边界**合法**（与既有原生索引范围一致） | 同上 | **C1-4**：无效实例编号必拒，边界值明确 | **§3.2 C1 校验表**、D4、D7 |
| **62** | `target_is_never_inferred_by_compiler` **【已裁决修正】C1 新增** | 对 42 条定义各删除一个必填变体 ⇒ **逐条 Err**（`MISSING_TARGET`/`INVALID_TARGET`），**无一条成功**；并断言「缺 `channel` 的输入」与「显式补 `channel=1` 的输入」编译结果**不相等**（前者必失败），即**不存在** `unwrap_or(1)` / `Default` / `axis` 缺省等猜测路径 | 同上 | **C1-5**：**不得自动猜测** Channel / Axis / DriveObject | **§3.2 C1 校验表**、§7.1 |
| **63** | `definitions_are_not_derived_from_presets` **【已裁决修正】C2 新增（禁自证循环）** | ① 源码级：`compiler.rs`/`definition.rs`/`semantic_equivalence.rs` 对 `PresetRegistry`/`PresetTemplate`/`EMBEDDED_828D`/`presets/` **零引用**；② 来源级：`r1.json` 每条 `source` 不含 `presets/828d.json` 或任何模板路径；③ 构建后 `git status --short -- drivers/sinumerik-nck/definitions` 为空（无 codegen 写入） | `tests/definition_catalog.rs`（驱动自有，N6） | **C2=P1**：新定义不是旧模板的派生物，两条路径独立 | **§4.2 第 5 条**、§4.4、§C2 |

#### 【已裁决修正】矩阵 ↔ 文件范围同步性核对表（双向，逐项可查）

**反向（矩阵 → §2 范围）**：63 行全部能对上 §2 的文件范围清单，无「测试指向范围外文件」的行。

| 矩阵行 | 所在测试文件 | 该文件在 §2 中的归属 |
|---|---|---|
| #1–#12、#63 | `drivers/sinumerik-nck/tests/definition_catalog.rs` | §2.1 **N6【新增】** |
| #13–#37、#57–#62 | `drivers/sinumerik-nck/tests/compiler_identity.rs` | §2.1 **N7【新增】** |
| #38、#39 | `drivers/sinumerik-nck/tests/semantic_equivalence.rs` | §2.1 **N8【新增】** |
| #40 | `drivers/sinumerik-nck/tests/legacy_equivalence.rs` | §2.3**【逐字不动】**（C2=P1） |
| #41 | `drivers/sinumerik-nck/tests/presets_828d.rs` | §2.3**【逐字不动】**（C2=P1） |
| #42–#45 | `drivers/sinumerik-nck/src/lib.rs` 内嵌单测 | §2.2 **M1（修改）** |
| #46–#49 | `tools/ci/test_nck_definition_lock.py`（驱动 `tools/ci/nck_definition_lock.py`） | §2.1 **N10 / N9【新增】** |
| #50 | `tests/driver-contract/tests/data_semantics.rs` | §2.3**【逐字不动】** |
| #51 | `tests/driver-contract/tests/resource_contract.rs`（`:396-460`） | §2.3**【逐字不动】** |
| #52 | `tests/driver-contract/tests/descriptor_contract.rs` | §2.3**【逐字不动】**（C10=D-A） |
| #53 | `tests/driver-contract/tests/discovery_contract.rs` | §2.2 **M8（修改：仅新增）** + §2.3（既有断言行**逐字不动**） |
| #54 | 其余 27 套契约套件 | §2.4**【不得触碰】** |
| #55 | `crates/core-types` 单测（`point.rs`） | §2.3**【逐字不动】** |
| #56 | `apps/mesa-web`（`pnpm --filter mesa-web test`） | §2.4**【不得触碰】** |

**正向（§2 范围 → 矩阵）**：§2 中所有**涉及测试**的文件都有矩阵行（上表 + §2.1 的 N6/N7/N8/N9/N10）。**不涉及测试的文件**——§2.2 的 `Cargo.toml`（M5）、`presets/README.md`（M6）、`.github/workflows/_rust-quality.yml`（M9），以及 §2.1 的 N1–N5、N11（类型/数据/ADR 产物）——不进矩阵，由 G1–G9 门禁与 §6.2 的自检覆盖；这是**唯一**允许的「范围有、矩阵无」情形。

**最终计数（同步性结论）**：矩阵 **63 行**；§2.1【新增】 **11 项**（N1–N11，其中涉及测试的 5 项：N6/N7/N8/N9/N10）；§2.2【修改】 **8 条改动项 / 6 个文件**（M7 已撤销，保留可追溯行）；§2.3【逐字不动】 **13 行 / 20 个文件**（16 个文件级条目在 §0 有 SHA-256）。三向无交集、无遗漏。

### 5.2 验收要求 A：身份完整性（防「不同物理读取目标共享同一 Canonical Identity」）

> 评审原话：**单靠「`identity_template` 跨 revision 不变」不足以覆盖该风险。** 本节给出**独立**的机制 + 判据 + 测试。

#### A.1 机制：身份键集合的**静态覆盖判据**（装载期强制，可执行）

对每条 `VariableDefinition` 定义**覆盖闭包** `cover(def)`：

```text
cover(def) = { k | NativeFieldSource::Target(k)      ∈ reachable_sources(def.addressing) }
           ∪ { a | NativeFieldSource::Argument(a)     ∈ … }
           ∪ { a | NativeFieldSource::ArgumentOffset(a, _) ∈ … }
           ∪ ( 若 DriveEntryModule ∈ … 则 {"drive_object"} )     // 递归对象编码含 entry
```

**判据（A-1，静态，必须成立）**：

```text
A-1a  cover(def) ⊆ { key.path | key ∈ def.identity_keys }          // 影响目标的来源必须被身份覆盖
A-1b  { key.path | key ∈ def.identity_keys } 的叶路径集合
        == identity_template 的占位符集合                          // 反之不得有冗余键
A-1c  每个 NativeFieldSource::Fixed(_) 字段**不需要**身份键
        （同一 variable_id 下所有点位的固定字段相同 ⇒ 不区分点位）
A-1d  def.target_required ⊆ { 出现在 identity_keys 里的 TargetKind } // 必填目标必须进身份
```

- `A-1a` 是「**影响采集目标的 Target 与 Argument 都参与身份计算**」的**机械可查**形式；`A-1b` 排除「不影响目标却进入身份」（那不是错误，但会让等价关系变复杂，因此**不允许**，除非在定义中显式声明为 `cosmetic_keys` 并给出等价性证据——**本期不引入 `cosmetic_keys`**，即等价规则为空集）。
- `A-1c` 是**明确且经过测试的等价规则**：`Fixed` 字段确实等价（同变量内恒定），故不参与身份。该等价规则由测试 #31（机械差分）作为**可执行的等价性证据**。
- `A-1d` 保证「必填目标不会只影响校验、不影响身份」。

**判据（A-2，动态，必须成立）**：

```text
A-2  对同一定义的任意两个合法输入 A、B：
       canonical_identity(A) == canonical_identity(B)
         ⟺  resolve(addressing(A)) == resolve(addressing(B))
       其中 resolve(·) = (area, area_no, module, column, line, count)
```

`A-1`（静态）保证**没有遗漏**；`A-2`（动态）保证**没有多余**且**没有碰撞**。两者同时成立才叫「身份完整性」。测试 #5/#6 执行 `A-1`，测试 #31 执行 `A-2`，测试 #25–#30 是 `A-2` 的**具名反例**（评审要求至少 5 个）。

#### A.2 为什么「`identity_template` 跨 revision 不变」不够（必须写进测试注释）

「模板跨 revision 不变」是一条**稳定性**不变量：它只说明「同一 `variable_id` 在不同 revision 上给出同一身份」，**完全不能**说明「该模板是否把 `axis`/`spindle`/`drive_object` 这类**区分物理目标的键**纳入了身份」。
反例：若 `axis.position.commanded_mcs` 的模板误写为 `axis.position.commanded_mcs@channel={channel}`（**漏掉 `axis`**），它对 revision 1/2/3 **逐字相同**，所有既有断言（#4、#13）仍全部通过；但轴 1 与轴 2 会得到**同一身份** → 两个不同物理读取目标共享同一 `point_key` → Core 判为同一 `point_id` → 历史数据被劈成两段。
因此 A 项**必须**由 #5（静态覆盖）、#31（机械差分）、#25–#30（具名反例）独立证明，**不得**用 #4/#13 代替。

### 5.3 验收要求 B：Native 命名空间隔离（`identity_mode=user` 不得冒用语义点位身份）

**不新增 Core 规则**；全部由 Driver 负责。

| 约束 | 规则 | 违反时错误码 | 依据 |
|---|---|---|---|
| **B-1 前缀命名空间强制** | `identity_mode="user"` ⇒ `point_key` 必须匹配 `native_key [ "~" dec ]`（即必须以 `native.` 开头且第 2 段为字母、其余 4 段为十进制，可选 `~<n>`） | `INVALID_USER_IDENTITY` | §4.1.1 E 的保留前缀 + `point.rs:428-431` 对 `native.` 的语法强制。**语义身份恒含 `@`**（I1）且 `variable_id` 不得以 `native.` 开头 ⇒ **结构上**不可能与语义 Key 相同 |
| **B-2 批次内身份冲突自检** | configure 处理完一个批次后，在**该次 configure 可见的点位集合**内，任意两个点位的**最终 `point_key`** 相同 ⇒ 整体失败 | `IDENTITY_NAMESPACE_COLLISION` | §4.1.2 第 1 条（同地址不同用途才允许区分）+ A 项风险在 native 侧的镜像。**【已裁决修正】C7=(a) 已批准**（详见 §8.1 C7 行）：① B-2 的**拒绝集合是 Core 既有规则的子集**——Core 侧 `binding_gate.rs:112-155` + `point.rs:954` 的 Endpoint-wide（跨 Task、跨 kind）判重会以 `DUPLICATE_POINT_KEY` 拒绝同样的一对点位，因此 B-2 **不新增任何拒绝权限**，只把该规则在**同一批次内提前**暴露并给出批次级消息；② `DUPLICATE_POINT_KEY`（`lib.rs:750-752`）仍是 Driver 侧**同任务重复**的权威码，二者**不互相替代**；③ **B-2 不适用于 `mesa.resources.v1` 与语义点位**，只作用于 `identity_mode="user"` 的原生 Key |
| **B-2 的视野边界**（**【复核补充】必须与 B-2 同时阅读**） | Driver 在 configure 时**通常只看到本次传入的 Task**，因此 B-2 **只能对 Driver 自身可见的点位集合**执行冲突检查 | —（无独立错误码；是 B-2 的**效力范围声明**，不是新规则） | **B-2 不得被表述为「已完成 Endpoint 级唯一性验证」**；**Endpoint-wide 跨任务唯一性仍以 PR-A 已落地的 Core 契约为唯一权威**（`binding_gate.rs:112-155` + `point.rs:954`，`DUPLICATE_POINT_KEY`）。B-2 是**early feedback / 纵深防御**，既**不替代**也不**扩大** Core 规则；Driver **不得**据此宣称整个 Endpoint 已去重 |
| **B-3 `user` 仅限原生** | `nck_variable` 出现 `identity_mode` 或 `user` ⇒ 拒 | `INVALID_BINDING_CONFIG`（复用既有码，不新增） | §2.4.2 D11 |
| **B-4 不静默改名** | Driver 不得追加/改写 `~<n>`；Core 只负责唯一性 | —（无错误码；由 #18 断言「Key 逐字未变」） | 附加 1、§4.1.2 第 3 条 |
| **B-5 语义点位不可改写** | `nck_variable` 的 `point_key` **必须**逐字等于重算身份 | `POINT_KEY_MISMATCH`（复用） | §4.1.1 D-2 |

**对应测试**：#32（B-1）、#33（B-2）、#34（结构性论证 + 冒用被拒）、#17（B-3）、#18（B-4）、#15（B-5）。

> **【复核补充】C7 的测试表述约束（必须遵守，否则测试名与断言文本即为错误）**：
> - **#33 只能断言「Driver 在其可见集合（单次 configure 传入的 Task）内提前拒绝」**；**禁止**把 #33 写成「Endpoint 级唯一性已验证 / 跨 Task 唯一性已保证」——Driver **看不到**其它 Task 的点位，也无从验证。
> - **#32 / #34 不涉及唯一性**：#32 是**前缀命名空间**的语法强制（B-1），#34 是**结构性论证**（语义身份恒含 `@` 且不以 `native.` 开头），二者**均不得**被表述为 Endpoint-wide 去重保证。
> - **Endpoint-wide 唯一性若需断言，只能落在 Core 侧**（PR-A 已冻结的 `binding_gate` / `point.rs` 契约与既有契约套件）；**PR-B 不得**新增或改写任何 Core 侧唯一性测试（§2.3 冻结面）。
> - 建议在 `tests/compiler_identity.rs` 的 #33 测试注释中逐字写入本约束，避免后续实现者把效力范围写大。

### 5.4 PR-B 门禁（与 PR-A 同口径）

**本地强制门禁（离线执行，全部不得连接真机/设备）**：

| # | 命令 | 说明 |
|---|---|---|
| G1 | `cargo fmt --all -- --check` | 格式 |
| G2 | `cargo clippy --workspace --all-targets -- -D warnings` | lint（默认 feature） |
| G3 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | lint（全 feature；CI 形态为 `--locked --all-features`，`_rust-quality.yml:35`） |
| G4 | `cargo build --locked --workspace` | `sha2` 加入后锁定构建（`AGENTS.md`：改驱动后跑子进程类合同测试前必须先构建） |
| G5 | `cargo test --workspace --locked` | 全量单测 + 驱动自有测试目标 |
| G6 | `cargo test --locked -p mesa-contract-tests --all-features` | 契约 29/29（唯一准入基线，`scripts/contract_suites.py:13-52`） |
| G7 | `python3 tools/ci/test_nck_definition_lock.py` | T6 工具自测（4 组断言） |
| G8 | `python3 tools/ci/nck_definition_lock.py --base <PR base SHA>` | 真实基线分支校验（CI 内必须跑；本地可用 `main` 的 SHA） |
| G9 | 前端回归：`pnpm --filter mesa-web install --frozen-lockfile` + `pnpm --filter mesa-web test` | **PR-B 零改动前端**，因此必须**原样全过**（`_web.yml:27-31`） |

**明确不纳入本地强制门禁**：
- **`cargo test --workspace --all-features`**：不作为本地强制门禁（与 PR-A 同口径）。
- **性能**：归 CI 的 `rust-perf`（`_rust-perf.yml:4`、`:36`），本地不跑 benchmark/SLO。
- **真机探针**：`live_identity_probe`（`tests/live_identity_probe.rs:122-124`）与 `live_samples_probe`（`tests/live_samples_probe.rs:134-136`）均为 `#[ignore]`，**本期门禁不得启用 `--ignored`**。
- **【已裁决修正】C9：`docs/adr/0005-nck-semantic-point-definition.md`（N11）不进门禁、不是开工前提**。ADR 0005 的阶段归属是 **PR-B 的文档尾巴**（与 PR-B 同 PR 提交即可），但 **G1–G9 中没有任何一项检查它**，**PR-B 的代码实现与合并也不得以「ADR 0005 未写」为前提**；若评审要求追溯 PR-A 已落地的 Core 侧契约，可另开**独立文档 PR** 补齐，**均不阻塞 PR-B 核心实现**。
- **不启动 `mesad`、不启动 Vite、不连任何设备**（本文件自身的约束；本文件编写期间也未执行）。

**提交时必须在 PR 描述中给出的证据**：【已裁决修正】§0 表中 **18** 个文件的摘要（前后对比，含 C2/C10 新增的 4 行）、G1–G9 的命令与结果、62/62 的 `checked` 计数（两条独立路径各一次）、`DIGESTS.lock` 的基线对比输出、§5.1 中 **#25–#34 与新增 #57–#63** 的**逐条**通过证据、以及 §4.2 第 5 条 ④ 的「删模板副本后新门仍 62/62」离线验收结论。

---

## 6. 能力与证据边界

| 项 | PR-B 的处置 | 依据 |
|---|---|---|
| `r0031`（`drive.parameter.r0031`） | 定义**保留**（62 条 golden 要求），`semantics_status=candidate`，**访问能力 HOLD**；UI/文档**不得**宣称可用 | `GATE.md:163-180`（两条入口合计 12 次被 `0x85` 拒绝，非偶发）、`README.md:48-49`、§8.1-7 |
| `r0027` / `r0031` 的普遍可读性 | **不得**断言普遍可读或普遍不可读；`r0078`/`r0080` 可读**不替代**其验收（物理量不同） | `GATE.md:178-180` |
| 驱动对象 `V/n` 的**轴归属** | **继续 HOLD**；`TargetRef{drive_object:{number,entry}}` **只**表达「哪个驱动访问编号 + 哪条入口」，**不表达**「哪个电机/哪个机床轴」 | `docs/evidence/nck-remaining-20-reconciliation.md:78-104`（只读途径已穷尽：无轴归属参数、候选身份参数取值均匀、`p0199` 名称不编码轴名）、`GATE.md:105-108`、§8.1-5、§8.2 |
| `drive.motor.temperature` | 语义严格限定为「**该驱动对象**的电机温度」；**不得**表述为「X 轴电机温度」；`label` 与文档均按此写 | §2.5.1 B#19 注、§8.2 |
| 19 个无语义证据的驱动参数（`r0021`/`r0027`/`r0033`/`r0031`/`r0062`–`r0064`/`r0077`–`r0082`/`r0103`/`p0105`/`r0106`–`r0108`） | 一律 `drive.parameter.<rNNNN>` + `semantics_status: parameter_identity`；**不得**臆造物理含义；候选文案只能进 `label` 且带 `candidate` 标记 | `PROVENANCE.md:1-7`、`:9-54`（语义限制列「—」）、`README.md:11-12`、§2.5.1 B 命名规则、§2.5.2 |
| `r0035` | `verified`；`invalid_value=-200.0` / `invalid_tolerance=0.001` 作为**定义的一部分进 digest**；驱动**不隐式继承**、不按变量名猜哨兵 | `README.md:18`、`:83-88`、`GATE.md:205-210`、§2.5.1 B#19 |
| 7 项非单个 NC 变量的项（设备逻辑状态/系统信息/主程序注释/当前程序注释/报警号/类型/信息） | **不得**进入定义目录，**不得**填假地址；保持 HOLD，需独立资源设计 | `PROVENANCE.md:70-82`、`docs/evidence/nck-remaining-20-reconciliation.md:117-139`、G11 |
| 840D sl | `DECLARED_FAMILIES` 仍含 `840d-sl`，但**无任何定义**；被引用时必须明确报「该系列无定义可引用」（`UNSUPPORTED_FAMILY`），**不得套用 828D** | §8.1-8、`README.md:112-114`、`GATE.md:89-95` |
| 支持等级 | PR-B **不提升** `sinumerik-nck` 的支持等级；仍为 `experimental`，真机门不翻转 | `GATE.md:87-95` |

---

## 7. 其它必须写明的边界

### 7.1 复用优先（禁止第二套 NCK 读取实现）

- **必须复用**：`NativeNckAddress::from_parameters`（`native.rs:230`）、`resolve_native`（`native.rs:365-402`）、`NativeNckAddress::source_label`（`native.rs:348-361`）、`encode_var_spec`（`codec.rs`）、`NckArea`/`NckUnitMode`/`parse_transport_size`/`NckDataKind`（`address.rs`、`native.rs:197-226`、`value.rs:97-140`）、以及 `mesa_core_types::encode_typed_value` / `validate_point_key`（`point.rs:207`、`:413`）。
- **禁用**：在 `compiler.rs` 中重写字节组装、重写枚举词表、重写范围校验、重写解码/长度/transport 映射。
- **落地断言**：测试 #22（**【已裁决修正】C3 的两条断言**：(a) `identity_mode` 未传入 Native Parser（13 字段 map 直交 `from_parameters` ⇒ `UnknownField`；剥离后恰 12 项）+ (b) wire / address / source_label 与直接调用旧解析器**逐字相等**）与测试 #38（62/62 字节等价）共同保证「没有第二套实现」。
- **【已裁决修正】C1 的落地断言**：测试 #57–#62（合法复合 Target、缺失/多余/类型错/无效实例号必拒、**不得猜测**）保证「Target 组合校验由定义声明、不在代码里散落 if」。
- **【已裁决修正】C2=P1 的落地断言**：测试 #63（`definitions_are_not_derived_from_presets`）保证新定义目录**不是**旧模板的派生物，两条路径独立构造地址、只共享同一份冻结 wire golden。
- **不复用的部分与理由**：见 §3.3 末表。

### 7.2 协议兼容

- `NativeNckAddress` 的 12 字段语义、`codec.rs` 的 10 字节布局、`client.rs` 的 ReadVar 会话、`value.rs` 的解码与无效值规则、以及 62 条 wire golden：**全部保持不变**。
- **不改 `proto/driver.proto`**，不改 `crates/driver-protocol/**`：`binding_kind`/`binding_config_json` 是字符串/JSON 透传，新增 `kind = "nck.points.v1"` **不需要**任何 wire 改动。
- **若确需改动**（当前评估为**不需要**）：必须作为**独立的「契约变更」PR**，并列出影响的 4 个其它驱动、需要同步改写的契约套件、`ProtocolManifest`/握手影响面、以及回滚方案。**PR-B 内部不得夹带 wire 改动。**

### 7.3 本期不做

- **前端与设备向导**：`apps/mesa-web/**` 全部（含 `nckPointModel.ts`、`NckPointFields.tsx`、`deviceModel.ts` 装配收敛、`browseModel.ts`、`ResourcePickerAntd.tsx`、`AddDeviceFlow.tsx`）属 **PR-C / PR-D**，PR-B 零改动。
- 用户导入定义、Core 变量知识库、Core 编译快照、新增 `FieldType`/`SchemaDescriptor`、新增 NCK area 字母（`M`/MMC/`S<n>` 一律不引入）、840D sl 定义、`cosmetic_keys` 等价规则、`nck_native` 服务端限权（仅沿用只读基线 + 前端警告口径，PR-C 落地）、历史 NCK 点位配置与历史数据迁移。

---

## 8. 与上位设计的冲突 / 评审裁决（已全部收敛）

> **【已裁决修正】C1–C10 已由 PR #110 评审逐项裁决，本文件已按裁决完成定向修订。** 本表给出**最终裁决**与**证据（本次只读实测）**；**被推翻的旧表述、原方案与影响行号**逐条保留在 **§8.1 修订记录**，评审只需复核该节的「原 → 新」差异，**不需要重新开展完整设计审查**。
> 与上位设计冲突之处：**上位设计 `docs/design/nck-semantic-point-refactor.md` 逐字不改**，冲突以本次裁决为准并在此如实记录。
> **【复核补充】优先级条款（范围严格受限）**：上位设计与本文件在**已裁决的 C1–C10 范围内**冲突时，**以本次正式批准的实施裁决为准**；该优先级**只适用于本表与 §8.1 明确列出的冲突项**，**不允许**开发时借此改变**其他**已冻结契约（未列入 C1–C10 的部分一律以上位设计原文为准）。上位设计**逐字不改**。
>
> **明确列出的冲突项与其上位设计锚点（逐条，均已实测 `文件:行号`）**：
> | 冲突项 | 上位设计锚点 | 冲突内容摘要 |
> |---|---|---|
> | C1 | `:435`、`:438`、`:482`、`:729` | 「恰好一个变体」/「变体数 = 1」 ⇒ 改为按变量定义校验合法字段组合 |
> | C2 | `:225`、`:271`、`:1127`、`:1280` | `presets/828d.json` 语义化 ⇒ 改为 P1：模板与旧门逐字不动 |
> | C3 | `:375`（与 `native.rs:147-160` 的 12 字段白名单冲突） | 「完全复用 `from_parameters`」 ⇒ 由 Compiler 剥离 `identity_mode`（7 步） |
> | C4 | `:677-691`、`:704-709`、`:1131` | 三份错误码表与 7 / 8 / 13 三个矛盾计数 ⇒ 唯一权威表 21 个码 |
> | C5 | `:192`、`:754`、`:1280` | 定义目录单文件 vs 分文件、`src/definitions/**` ⇒ 分文件 + 与 `src/` 平级 |
> | C6 | `:1127` | 要求改写 `discovery_contract` 叶形态 ⇒ 只新增一条叶断言 |
> | C7 | `:726`（§2.8 #10「Driver 不裁决身份」） | B-2 批次内自检 ⇒ 批准为**仅限可见集合**的提前反馈，不替代 Core |
> | C9 | `:199`、`:1121`、`:1368` | ADR 0005 阶段归属 ⇒ 归 PR-B 文档尾巴且**不阻塞实现** |
> | C10 | `:112`、`:209`、`:210`、`:1127` | Descriptor `variable` 资源面重构/改写断言 ⇒ 保留（D-A），整文件逐字不动 |
> | ~~C8~~ | — | **无实质冲突**（上位设计本就写 `definition.rs`） |

| # | 冲突 | 证据（实测 `文件:行号`） | **评审裁决（已落实）** | 原级别 |
|---|---|---|---|---|
| **C1** | 「`target` 变体数 **= 1**」与 §2.5.1 表 A 的**双 target**定义**不可同时成立** | 上位设计 §2.4.1「恰好一个变体」（`:435`、`:438`）、§2.4.2 D7「变体数 = 1」（`:482`、`:729`）；但 §2.5.1 A#7/#8 需 `{channel, axis}`、A#12–#14 需 `{channel, spindle}`；§4.1.1 C 的模板表本身含 `@axis={axis},channel={channel}`；§2.5 的 `target_required: Vec<TargetKind>` 为复数 | **C1 已批准（P0）**：D7 改为「**按变量定义校验合法字段组合**」；`{"target":{"channel":1,"axis":3}}` 是**合法复合 Target**，不得因含两字段被拒。合法组合表见 **§3.2**；组合由 `VariableDefinition.targets`/`target_required` **声明**；Driver 必须拒绝 缺失/多余/类型错/无效实例号，**不得猜测**缺失的 Channel/Axis/DriveObject。**边界原文**：`{channel, axis}` 是 **NCK 读取的寻址坐标组合**，**不表示**已证明该机床轴与某个 SINAMICS 驱动对象的物理对应关系 | **P0** |
| **C2** | 「`presets/828d.json` 语义化」与「`legacy_equivalence.rs` / `presets_828d.rs` 原样通过」**不可同时成立** | `legacy_equivalence.rs:62-94`（实测 `:70-94` 逐条断言 `module`/`column`/`decode`/`transport_size`/`count`）把模板 `parameters` 当**原生字段**读；`presets_828d.rs:15-26`、`:71-227`、`:231-258`、`:265-305` 读 `parameters.module`/`area`/`invalid_value`。模板语义化 ⇒ 两文件共 **8 个用例**（2+6，实测）必失败 | **C2 已批准（P0）：正式采用 P1**。`presets/828d.json`（`e910b055…`）、`tests/legacy_equivalence.rs`（`44e429bb…`）、`tests/presets_828d.rs`（`4eb747b5…`）**逐字不动**；**不得**把已验证的原生地址数据改写为新的语义结构；新定义目录通过 **`golden_ref`** 关联旧冻结向量；**Browse 叶 `binding_json` 可以**转为语义载荷；旧模板原生 `parameters` **不改**；新旧两条路径**独立构造地址**，与**同一份**冻结 Wire Golden 比较（62/62）。**明确禁止**「从旧模板自动反向生成新定义、再用同一数据自证等价」——检测方式 4 条见 §4.2 第 5 条 | **P0** |
| **C3** | `identity_mode` 位于 `parameters` 与 `NativeNckAddress::from_parameters` 的 12 字段白名单**冲突** | `native.rs:147-160` 的 `ALLOWED` 恰 **12** 项、不含 `identity_mode`；`native.rs:231-235` 对未知键直接报 `UnknownField`；`native.rs` 被 §1.3 冻结 | **C3 已批准（P0）**：由 **Address Compiler** 负责 `identity_mode`；**不得放宽** `from_parameters` 的严格白名单。**7 步流程**见 **§3.3**。**关键约束**：剥离**仅针对已经明确验证的 Compiler 元数据**，**不得成为忽略未知字段的通用机制**（非白名单内的其它未知键**仍必须被拒绝**）。测试 **#22** 必须同时证明 (a) `identity_mode` 未传入 Native Parser 与 (b) 最终地址/Wire 与直接调用旧解析器一致 | **P0** |
| **C4** | 上位设计 §2.6 / §2.7 / §5-7 错误码清单**互不一致**，且缺 `MISSING_IDENTITY_KEY` | §2.6 表（`:677-691`）实测 **13** 行，其中 `UNSUPPORTED_FAMILY`、`DEFINITION_DIGEST_MISMATCH` **未出现在** §2.7 的 Driver 行（`:704-709`，实测**新增 8 个**）；§5-7（`:1131`）写「新增 **7** 个 Driver 码」；§4.1.1 C 与 §6.2 要求 `MISSING_IDENTITY_KEY`，§2.6 **没有**它 | **C4 已收敛**：以**实际 Compiler 入口与矩阵使用全集**为准，合并为**唯一一份权威错误码表 = §3.5**，**取代**上述三处旧表述与 **7 / 8 / 13** 三个互相矛盾的数字。**权威计数 = 21 个唯一 code**（复用 8 + 新增 13），**`MISSING_IDENTITY_KEY` 保留为独立码**。§3.6 的「失败码」列不是第二份码表 | **P1** |
| **C5** | 定义目录的**存放形态**自相矛盾（并附位置歧义） | 上位设计 §1.1（`:192`）写 `drivers/sinumerik-nck/src/definitions/828d.json`（**单文件 + `src/` 下**）；§3.1（`:754`）写「按 `revision` 分文件存放（`definitions/828d/r1.json`…）」；§7 PR-B 范围（`:1280`）又写 `src/{…definitions/828d/*.json}`。`presets.rs:23` 的既有同构先例是 `include_str!("../presets/828d.json")`（实测） | **C5 已收敛：采用单一约定** = **按 revision 分文件** + **与 `src/` 平级**：`drivers/sinumerik-nck/definitions/828d/r1.json`、`…/DIGESTS.lock`。理由：T6 依赖「已发布文件内容可逐字比对」，单文件形态**无法**落地 T6 的三条断言；位置与 `presets.rs:23` 同模式。详见 **§3.1**。§1.1 的单文件路径与 `src/definitions/**` 一律作废 | **P1** |
| **C6** | §6.2 要求「必须改写 `discovery_contract`（`:224-269`）浏览叶形态」，但该套件**没有**叶形态断言 | 实测 `discovery_contract.rs:255-260` 只断言根 2 节点 / `nck://presets` / `diagnostic`；`:270-273` 只断言未知 parent 空页；**全文无 `binding_json` 断言**（`grep binding_json` 零命中）；实际行区间为 `:224-274`（非 `:224-269`） | **C6 已收敛**：**不重写既有断言**（它们不因叶形态变化而失败），**只新增**一条叶形态断言（§2.2 M8 / 矩阵 #53）；既有断言行 `:255-260`、`:270-273` 列入 §2.3 冻结区间 | **P2** |
| **C7** | 「Driver **不**裁决身份唯一性」（上位设计 §2.8 #10）与本文件 B-2「Driver 批内身份冲突自检」 | 上位设计 §2.8 #10 Driver 列写 `❌ 不裁决身份`（`:726`）；§4.1.2「Core 只负责唯一性」；Core 侧已有 `binding_gate.rs:112-155` + `point.rs:954` 的 Endpoint-wide 判重（`DUPLICATE_POINT_KEY`） | **C7 选定 (a)：批准 B-2**，作为**仅限 `identity_mode="user"` 原生命名空间**的纵深防御。理由：① B-2 的**拒绝集合是 Core 既有规则的子集** ⇒ **不新增任何拒绝权限**，只把同一规则在批次内**提前**暴露；② 若选 (b)，#33 需改为 Core 侧 e2e 断言，必然要改 **PR-A 已冻结**的 Core 测试文件（`point.rs`/`binding_gate.rs`/`data_semantics.rs` 均属 §2.3【逐字不动】），与「最小变更」相悖；③ B-1 是结构性隔离，**#32/#34 无论如何都保留**。详见 **§5.3 B-2**。**【复核补充·效力边界】**：Driver 在 configure 时**通常只看到传入的 Task**，因此 B-2 **只能对 Driver 自身可见的点位集合**检查冲突，**不得**被表述为「已完成 Endpoint 级 / 跨 Task 唯一性验证」；**Endpoint-wide 唯一性仍以 PR-A 已实现的 Core 契约为唯一权威**，B-2 既**不替代**也不**扩大** Core 规则（`§5.3` 的「B-2 的视野边界」行；#32–#34 的表述约束见 `§5.3` 的「C7 的测试表述约束」）。**与上位设计 §2.8 #10 的冲突在此如实记录：本次裁决为准，上位设计逐字不改** | **P1** |
| **C8** | 新文件命名：`definition.rs`（§1.1/§7）vs `definitions.rs`（本轮任务书） | 上位设计 §1.1（`:191`）、§7 PR-B 范围（`:1280`）、§7.0-4（`:1264`）均写 `definition.rs` | **C8 已收敛：以实际采纳为准 = `definition.rs`（单数）**；`definitions/`（复数）**只**用作数据目录名 `drivers/sinumerik-nck/definitions/`。**全文已统一**（§2.1 N2、§2.2、§3.1、§7 PR-B 范围引用） | **P2** |
| **C9** | `docs/adr/0005-nck-semantic-point-definition.md` 的**阶段归属**未定；PR-A 已合并但 ADR 0005 不存在 | 上位设计 §1.1（`:191` 区块）列为新增但未指派 PR；§5 说「所有条目均由 ADR 0005 统一留痕」；实测 `docs/adr/` 只有 `0001–0004`（4 个） | **C9 已收敛：ADR 0005 不阻塞 PR-B 核心实现。** 阶段归属 = **PR-B 的文档尾巴（§2.1 N11）**；**不构成开工前提**、**不是 G1–G9 门禁项**、**不是合并前提**。若需追溯 PR-A 已落地的 Core 侧契约，另开**独立文档 PR** 补齐。详见 **§5.4** 的「不纳入本地强制门禁」清单 | **P1** |
| **C10** | Descriptor 的 `variable` 资源面：**删除** 还是 **降级保留** | 上位设计 §1.2（`:225` 邻域）「`variable` 资源 = 12 原生字段 → 重构」、§5-4「破坏性变更」、§6.2「`descriptor_contract` 必须改写」；实测受影响断言只有 `descriptor_contract.rs:605` 与 `:729-731`（两处都只断言「`variable` 资源存在」与 `outputs[0].type_spec == DriverResolved`） | **C10 选定 D-A 并写成具体契约（见下）** | **P1** |

#### 【已裁决修正】C10 的具体契约（D-A：保留、不破坏）

1. **`variable` 资源面保留什么（逐字不动）**：`lib.rs:170-331` 的 `ResourceDescriptor{ id: "variable", label, parameters: 12 个原生字段描述, outputs }`（`:178-311` 字段、`:313-319` outputs、`:324-327` `resource_selection_methods = [Manual, Browse]`）**全部保留**，**不删除、不收缩**。它是**原生字段目录**（`nck_native` 的 12 字段可读来源）。
2. **语义点位走哪条新入口**：`nck.points.v1` 信封的 `PointSelection`（PR-A 类型，`point.rs:103-112`）→ Address Compiler；浏览/预填走 `presets.rs:151-186` 的叶 `binding_json`（M4，改为语义信封）。**`variable` 资源不再是语义点位的默认配置入口**，也**不再**是 `mesa.resources.v1` 的采集入口（后者整体被 `UNSUPPORTED_BINDING` 拒绝，M2/#43）。
3. **两者关系**：同一 12 个原生字段有两个读取面——① Descriptor 的 `variable` 资源（**声明面**，供既有 Descriptor 消费者/UI 渲染原生字段）；② `nck.points.v1` 的 `nck_native` 参数（**配置面**，实际可用的入口）。**声明面保留是为了不破坏消费者；配置面以 `nck.points.v1` 为唯一真值。**
4. **受影响的既有断言 = 保留、不破坏（逐条核实结论）**：
   - `descriptor_contract.rs:605` `assert!(d.resources.iter().any(|r| r.id == "variable"))` → **保留**（资源仍存在）；
   - `descriptor_contract.rs:729-731` `let var = nck.resources.iter().find(|r| r.id == "variable").unwrap(); assert_eq!(var.outputs[0].type_spec, OutputTypeSpec::DriverResolved);` → **保留**（outputs 未变）；
   - `:599-604` 的只读五条与 `:391-399`「生产 Write 未开发前不得声明 write」→ **保留**。
   - ⇒ `tests/driver-contract/tests/descriptor_contract.rs` 整体移入 §2.3【逐字不动】（§0 摘要 `382064d3…`，1094 行）；§2.2 的 **M7 撤销**；矩阵 **#52 由「两处改写」改为「整文件原样通过」**，**#44 由「改写」改为「原样通过」**。
5. **【残留风险·如实记录】**：Descriptor 仍公告 `variable` 资源与 `Manual`/`Browse` 选择方式，而 `mesa.resources.v1` 绑定点位会被 `UNSUPPORTED_BINDING` 拒绝 ⇒ 在 **PR-C** 完成 UI 收敛前，UI 可能展示一条**不可用**的配置路径。这是「保留消费者兼容」与「立即移除过期入口」之间的**已知取舍**，必须在 PR-B 描述中显式声明；UI 侧收敛归 **PR-C/PR-D**（§7.3）。

### 8.1 【已裁决修正】修订记录（评审项 → 原表述 → 本次修正 → 影响章节）

> **用途**：评审**只复核本节的差异**，不需要重新开展完整设计审查。第 2 列是**修订前的原文关键句（逐字保留，不删除旧表述）**，第 3 列是**修订后的落地表述**，第 4 列是**受影响章节**。
> 修订后的精确行号见本节末「修订后关键锚点行号（实测）」；**修订前后总行数不同**（846 行 → 见文末），因此旧行号一律以「章节 + 原文」定位，不引用旧行号。

| 评审项 | 原表述 / 原方案（修订前，**逐字保留**） | 本次修正（已落实） | 影响章节 |
|---|---|---|---|
| **C1** | §3.2：「**【PR-B 修正】D7 的「变体数 = 1」必须改为「变体集合合法性」**」；§3.6 D7：「`target` 形态与合法性（**见 §C1 修正**）」；§C1 建议：「**采纳本文件 §3.2 的 D7 修正**（变体集合合法性…），把「变体数 = 1」作废」（**级别 P0（阻塞）**） | 改为**【已裁决修正】C1**：① §3.2 新增「**Target 的合法字段组合表**」（通道 / 方式组 / 轴位置 / 主轴 / 驱动对象 5 类 ⇒ 合法 Target），并写明**组合由 `VariableDefinition.targets` / `target_required` 声明**；② 明确 `{"target":{"channel":1,"axis":3}}` 是**合法复合 Target**；③ 新增「Driver 必须拒绝」5 条（缺失 / 多余 / 类型错 / 无效实例号 / **不得猜测**）；④ 原样写入边界句「`{channel, axis}` 是 **NCK 读取的寻址坐标组合**，**不表示**已证明该机床轴与某个 SINAMICS 驱动对象的物理对应关系」；⑤ §3.6 D2/D3/D7 明确码归属；⑥ 矩阵**新增 #57–#62**；⑦ §C1 行改为「**已批准（P0）**」 | §3.2、§3.6 D2/D3/D7、§5.1 #37/#57–#62、§7.1、§8 C1 |
| **C2** | §0：「`presets/828d.json` … 见 §C2：**建议逐字不动**」；§2.2 表下：「**不修改**（明确声明，防止范围蔓延）：`presets/828d.json`（§C2 建议）…」；§4.2 第 2 条：「⚠️ **这条成立的前提是 `presets/828d.json` 的 `parameters` 保持原生字段**…见 §C2。」；§4.2 第 3 条「**复用为回归基线**」（**未**写明两条路径独立构造地址、**未**禁止自证循环）；§4.4：「若采纳 §C2 的推荐方案（模板不动）…若评审选择 §C2 的备选方案（模板语义化）…」；§C2 建议：「**推荐方案 P1（模板逐字不动）**…**备选 P2**（模板语义化）…」（**级别 P0（阻塞）**） | 改为**【已裁决修正】C2=P1（正式采用）**：① §0 三行改判为**逐字不动**并配实测 SHA-256（`828d.json` `e910b055…`、`legacy_equivalence.rs` `44e429bb…`、`presets_828d.rs` `4eb747b5…`）；② §2.3 正式写入这 3 个文件；③ §2.2 表下「不修改」声明升级为「已正式移入 §2.3」；④ §4.2 第 2 条的「前提」改为「已裁决的冻结契约」；⑤ §4.2 **新增第 4 条**（两条路径独立构造地址、只共享冻结 golden）与**第 5 条**（禁止从旧模板反向生成、**4 条检测方式**）；⑥ §4.4 把 P2 标为**未采纳备选**并附 3 条理由；⑦ 矩阵 **#40/#41 补摘要与口径**、**新增 #63**；⑧ §C2 行改为「**已批准（P0）**」 | §0、§2.1（N4）、§2.2（M4/M6 + 表下声明）、§2.3、§4.1、§4.2、§4.3、§4.4、§5.1 #40/#41/#63、§5.4、§7.1、§8 C2 |
| **C3** | §3.3 接口表 `compile_native` 行：「同上（**剥离 `identity_mode` 后**传入，见 §C3）」；§C3 建议：「**由 Compiler 剥离** `identity_mode` 后再调用 `from_parameters`：Compiler 侧 13 字段 `deny_unknown_fields` 结构 → 取出 12 字段 map → `from_parameters`。**`native.rs` 不动**…测试 #22 固定该行为」（**级别 P0（阻塞）**）；矩阵 #22 原文只写「`nck_native` = 12 字段 + `identity_mode` ⇒ 成功，且 `resolved.wire` 与「同 12 字段直接 `from_parameters`」**逐字节相等**；把 `identity_mode` 直接交给 `from_parameters` ⇒ `UnknownField`…」（**未**分列 (a)/(b) 两条断言） | 改为**【已裁决修正】C3（P0 已批准）**：① §3.3 新增「**`identity_mode` 的 7 步剥离流程**」（1 接收 → 2 全量严格校验**含 `identity_mode`** → 3 默认 `derived`、仅允许 `derived`/`user` → 4 构建**只含 12 原生字段**的独立对象 → 5 交给 `from_parameters` → 6 复用既有解析与读取计划 → 7 按模式做身份验证）；② 原样写入关键约束「剥离**仅针对已经明确验证的 Compiler 元数据**，**不得成为忽略未知字段的通用机制**（非白名单内的其它未知键**仍必须被拒绝**）」；③ 矩阵 **#22 改写为 (a)/(b) 两条独立断言**；④ §C3 行改为「已批准（P0）」 | §3.3、§5.1 #22、§7.1、§8 C3 |
| **C4** | §3.5 标题「Driver 专属错误码全集（PR-B 部分）」（19 个数据行，含 1 行划删的 `UNSUPPORTED_RESOURCE`/`UNSUPPORTED_OUTPUT`）；§C4 建议：「以本文件 §3.5 为准（§2.6 的 13 个 + `MISSING_IDENTITY_KEY` + B 项 2 个 = **16 个** Driver code，其中 3 个复用原 code）；§5-7 的计数表述作废」（**级别 P1**） | 改为**【已裁决修正】C4**：① §3.5 重写为「**PR-B 唯一权威错误码表**」，分 A（`CompileError` 14 个）/ B（configure 层 7 个）/ C（不再产生 2 个）三段，**明确取代**上位设计 §2.6（`:677-691`，实测 13 行）、§2.7（`:704-709`，实测新增 8）、§5-7（`:1131`，写 7 个）三处旧表述与 **7 / 8 / 13 三个互相矛盾的数字**，并**作废本文件旧的「16 个」计数**；② **权威计数 = 21**（复用 8 + 新增 13），给出机械复核式；③ **`MISSING_IDENTITY_KEY` 保留为独立码**（A9）；④ 声明 §3.6 的「失败码」列**不是第二份码表** | §3.5、§3.6、§8 C4 |
| **C5** | §1.2 关键否证表 `drivers/sinumerik-nck/definitions/`（目录）行（**未**说明位置歧义）；§2.1 N4 只给路径、未定形态；§3.1 只有类型代码与「摘要算法」段，**未**固定存放形态与位置；§C5 建议：「**采纳 §3.1 的分文件形态**（`definitions/828d/r1.json`）…§1.1 的路径作废」（**级别 P1**） | 改为**【已裁决修正】C5：单一约定**：① 形态 = **按 revision 分文件**；② 位置 = **与 `src/` 平级**（`drivers/sinumerik-nck/definitions/`，理由是 `presets.rs:23` 的 `include_str!("../presets/828d.json")` 同构先例）；③ `DIGESTS.lock` 同目录；④ §3.1 新增裁决块（含理由与作废清单）；⑤ §1.2 表格行、§2.1 N4 补位置；⑥ **顺带消除上位设计 §1.1（`:192`）与 §7（`:1280`）的 `src/definitions/**` 位置歧义** | §1.2、§2.1（N4）、§3.1、§8 C5 |
| **C6** | §2.2 M8 行末「→ 见 §C6」；§C6 建议：「**不重写既有断言**（它们不因叶形态变化而失败），**改为新增**一条叶形态断言（M8）」（**级别 P2（缩小范围）**） | 改为**【已裁决修正】C6 已收敛**：① §2.2 M8 明确「既有断言行属冻结面」；② §2.3 新增「`discovery_contract.rs:224-274` 的**既有断言**」区间冻结行；③ 矩阵 #53 补「既有断言行 `:255-260`、`:270-273` **不得改写**」；④ §C6 行补实测「`grep binding_json` **零命中**」与「实际区间为 `:224-274`（上位设计写 `:224-269`）」 | §2.2（M8）、§2.3、§5.1 #53、§8 C6 |
| **C7** | §5.3 B-2 行：「**这是纵深防御**：Core 侧 `binding_gate.rs:112-155` 已做 Endpoint-wide 跨 kind 判重，但 Driver 必须**独立**可验收（见 §C7 的冲突声明）」；§C7 建议：「请评审二选一：**(a)** … **(b)** …**无论选哪个，测试 #32/#34 都必须保留**；选 (b) 时 #33 改为 Core 侧 e2e 断言」（**级别 P1**） | 改为**【已裁决修正】C7 选定 (a)**：① §5.3 B-2 行写明**批准理由**（B-2 的拒绝集合是 Core 既有规则的**子集** ⇒ **不新增任何拒绝权限**，只把同一规则在批次内提前暴露；若选 (b) 则 #33 需改为 Core 侧 e2e，必然要改 PR-A 已冻结的 Core 测试文件，与最小变更相悖）；② §3.5 B5 标注「C7=(a) 已批准」；③ **#32 / #34 保留**，#33 保留在 `compiler_identity.rs`；④ §C7 行改为「选定 (a)」并**如实记录与上位设计 §2.8 #10 的冲突：上位设计逐字不改，本次裁决为准**；⑤ **【复核补充】新增「B-2 的视野边界」行（§5.3）与「C7 的测试表述约束」（§5.3）：B-2 只对 Driver 可见集合生效，\*\*不得\*\*写成 Endpoint 级保证；#32 只断言语法、#33 效力 = 可见集合、#34 是结构性论证；Endpoint-wide 唯一性以 Core 契约为唯一权威，PR-B 不得新增/改写 Core 侧唯一性测试** | §3.5（B5）、§5.1 #32–#34、§5.3、§8 C7 |
| **C8** | §2.1「相对上位设计的偏差」第 1 条：「**本文件统一采用 `definition.rs`**…见 §C8。」；§C8 建议：「**采用 `definition.rs`（单数）**；`definitions/`（复数）**只**用于存放数据的目录 `drivers/sinumerik-nck/definitions/`」（P2） | 改为**【已裁决修正】C8 已裁决**：命名以**实际采纳**为准 = `definition.rs`（单数），`definitions/` 仅作数据目录；§2.1 该条补「全文已统一」；§3.1 位置条款与之配套 | §2.1、§3.1、§8 C8 |
| **C9** | §2.1 N11「`docs/adr/0005-nck-semantic-point-definition.md` … 逐条覆盖 §5 的 9 项破坏」；§5.4「明确不纳入本地强制门禁」清单**无** ADR 条目；§C9 建议：「**PR-B 携带 ADR 0005**，并在其中**追溯** PR-A 已落地的 Core 侧契约…若评审要求 PR-A 补，则需单独的文档 PR」（**级别 P1**） | 改为**【已裁决修正】C9：不阻塞 PR-B 核心实现**：① §5.4「不纳入本地强制门禁」新增一条，明确 ADR 0005（N11）**阶段归属 = PR-B 的文档尾巴**、**不构成开工前提 / 非 G1–G9 门禁项 / 非合并前提**；② §C9 行同步；③ N11 保留在 §2.1【新增】但不进入矩阵（非测试产物） | §2.1（N11）、§5.4、§8 C9 |
| **C10** | §2.2 **M1 ②**「Descriptor 的 `variable` 资源面：`:170-331`…」；§2.2 **M7**「`tests/driver-contract/tests/descriptor_contract.rs` … `:605` 与 `:729-731` **随资源面改写**」；矩阵 **#44**「`descriptor_is_valid_and_read_only`（**改写**）」、**#52**「`descriptor_contract` 的**两处改写**（`:605`、`:729-731`）」；§C10 建议：「**方案 D-A（推荐，churn 最小）**：保留资源 id `variable` 与 12 字段…但 `resource_selection_methods` 收缩…**方案 D-B**：删除 `variable` 资源 → 必须改写上述两处断言」（**级别 P1**） | 改为**【已裁决修正】C10 选定 D-A 并写成具体契约**（见 §8 末 5 条）：① **保留现有 `variable` Resource 的原生字段描述**（`lib.rs:170-331` **逐字不动**，含 `resource_selection_methods` **不收缩**）；② 明确它**不再是语义点位的默认配置入口**（语义走 `nck.points.v1` + 浏览叶载荷），也**不再是** `mesa.resources.v1` 采集入口；③ 写明**声明面 vs 配置面**的关系；④ 受影响断言 `:605`、`:729-731` 实测只断言「资源存在」与「`outputs[0].type_spec == DriverResolved`」⇒ **保留、不破坏**；⑤ **M7 撤销**、`descriptor_contract.rs` 移入 §2.3【逐字不动】、矩阵 #44 / #52 改为「原样通过」、`lib.rs:170-331` 移入 M1 的「不得触碰」；⑥ **如实记录残留风险**（UI 在 PR-C 前可能展示不可用路径） | §0、§2.2（M1/M3/M4/M7）、§2.3、§5.1 #44/#52、§5.4、§8 C10 契约块、§9 |

**【已裁决修正】非裁决性事实修正（一并记录，便于评审对照）**：
- `scripts/contract_suites.py:16-52` → **`:13-52`**（实测：`BASELINE = [` 在 `:13`、`SUITES` 在 `:52`；原引用少算 3 行；套件总数 14 + 12 + 3 = **29 不变**）。
- 上位设计 §3.2（`:774`）写 workspace `sha2` 在 `Cargo.toml:53`；**实测为 `Cargo.toml:68`**（`sha2 = "0.10"`）。本文件 §2.2 M5 写 `:68`（正确）；**上位设计逐字不改**，差异在此记录。
- 本次修订前的 §3.4、§5.3 与矩阵 #32/#33 用「§6.3」指代 B 项命名空间规则；**实测上位设计 §6.3 = 「PR-C（Frontend 编辑器）」**（`nck-semantic-point-refactor.md:1199`），该指代**错误**。已统一改为**本文件 §5.3**（B-1/B-2 的定义处）；上位设计引用（§6.2 / §6.6 T3b/T4/T5）**保持不变**。

**【复核补充】PR #110 复核后的两处补充（不改变 C1–C10 的任何裁决，只收紧效力范围与优先级口径）**：

| # | 复核意见 | 本次补充（已落实） | 影响章节 |
|---|---|---|---|
| R1 | **C7：提前检测不能依赖单个 Task 的视野**——Driver 通常只看到传入 Configure 的 Task，只能对自身可见的点位集合做冲突检查，**不得宣称完成 Endpoint 级跨任务唯一性验证**；Endpoint-wide 唯一性仍以 PR-A 已实现的 Core 契约为准；**不需要调整架构**，只需确保测试 #32–#34 不把任务内检测误写成 Endpoint 级保证 | ① §5.3 B-2 行把规则范围改为「**该次 configure 可见的点位集合**」；② **新增「B-2 的视野边界」行**（明确 B-2 是 early feedback / 纵深防御，**不替代、不扩大** Core 规则，Driver 不得宣称整个 Endpoint 已去重）；③ **新增「C7 的测试表述约束」**（#33 只断言可见集合内提前拒绝；#32 只断言语法；#34 是结构性论证；Endpoint-wide 断言**只能落在 Core 侧**，PR-B 不得新增/改写 Core 唯一性测试）；④ §3.5 B5 行补效力范围；⑤ §5.1 #32/#33/#34 三行各自补「不涉及唯一性 / 效力范围 / 不构成 Endpoint 级保证」；⑥ §8 C7 行与 §8.1 C7 行同步 | §3.5（B5）、§5.1 #32–#34、§5.3（两处新增行）、§8 C7、§8.1 C7 |
| R2 | **上位设计冲突的优先级**：可保留已冻结的上位设计不变；PR #110 可明确规定——当上位设计与 PR-B 细化方案在**已裁决的 C1–C10 范围内**冲突时，**以本次正式批准的实施裁决为准**；该优先级**只适用于明确列出的冲突**，不允许借此改变其他已冻结契约 | ① 文档头部新增**优先级条款**（严格限定范围 + 禁止推广 + 上位设计逐字不改）；② §8 intro 同步，并新增**「明确列出的冲突项与其上位设计锚点」逐条表**（C1–C7、C9、C10 共 **9 项**，每项给出实测 `文件:行号` 锚点；**C8 无实质冲突**）；③ 未列入 C1–C10 的部分**一律以上位设计原文为准** | 文档头部、§8 intro（含冲突锚点表）、§8.1 R2 行 |

**修订后关键锚点行号（实测，供评审直接跳转）**：

| 锚点 | 本节修订后的行号 |
|---|---|
| 文档头部 **优先级条款**（复核补充 R2：范围严格受限） | `:9` |
| §0 冻结指纹表（数据行 **18** 条，含 C2/C10 新增的 4 行） | `:21-38` |
| §2.1【新增】N6 / N7 / N8 / N9 / N10 行 | `:153` / `:154` / `:155` / `:156` / `:157` |
| §2.2【修改】M1（已撤销 M1-②） / M7（**已撤销**，可追溯行） / 计数块 | `:171` / `:177` / `:183-184` |
| §2.3【逐字不动】表 / 计数与同步性自检 | `:186-202` / `:204-209` |
| §3.1 **C5** 定义目录形态 + 位置裁决块 | `:243` |
| §3.2 **C1** Target 合法字段组合表 / C1 边界句（原样写入）/ D7 改写 | `:391` / `:411` / `:423` |
| §3.3 **C3** `identity_mode` 7 步剥离流程 / C3 关键约束 | `:525` / `:542` |
| §3.5 **C4** 唯一权威错误码表 / 权威计数核对 / **B5（效力范围）** | `:577` / `:626` / `:616` |
| §3.6 D2 / D3 / D7（码归属） | `:634` / `:635` / `:639` |
| §4.2 第 2 条（旧门原样，C2）/ 第 4 条（两条路径独立）/ 第 5 条（禁自证 + 4 条检测） | `:660` / `:667` / `:672` |
| §4.4 **C2 未采纳备选 P2**（附 3 条理由） | `:704` |
| §5.1 测试矩阵（**63** 行 `:721-783`）；#22（C3）`:742`、#32–#34 `:752-754`、新增 #57–#63 `:777-783`；同步性核对表 `:785-808` | `:719-808` |
| §5.3 **C7=(a)** B-2 行 / **「B-2 的视野边界」行（复核补充 R1）** / **「C7 的测试表述约束」（R1）** | `:864` / `:865` / `:872` |
| §5.4 **C9** ADR 0005 不进门禁 | `:898` |
| §8 **冲突锚点表（R2）** / C1–C10 裁决表 / C10 具体契约 5 条 | `:951-963` / `:965-976` / `:978-988` |
| §8.1 本修订记录（含**复核补充 R1/R2** 行 `:1017-1018`、本锚点表、「非裁决性事实修正」`:1008-1011`） | `:990-1042` |
| §9 新增待确认项 9 / 10 / 11（C10 残留风险、C2 独立复现、C5 位置） | `:1065-1067` |
| 附录 A 本次修订额外只读命令块 | `:1108` |

### 8.2 已核对、确认无冲突的点

- Core 侧 `encode_typed_value` 可被 Driver 复用，**不违反** P9 护栏（禁令词表 `resource_contract.rs:398-428` 不含它）与 §2.8 #19（Driver 仍唯一决定「哪些键进身份、以何顺序、模板形态」）。
- 本文件位于 `docs/design/`，**不在** P9 扫描范围（`resource_contract.rs:432-434` 只扫 `crates/core-types/src` 与 `crates/core-api/src`），因此文中出现 `TargetRef`/`nck_variable` 等标识不会导致该测试失败。
- E1–E11 golden 的 `include_str!` 相对路径对 Driver 同样成立（`drivers/sinumerik-nck` 的 `../..` = 仓库根），无需复制文件、无需改 `point.rs:985-988`。
- `data_semantics.rs` 的 8 个 PR-A 语义用例使用**字面量** `SEMANTIC_KEY`（`:82-91`），不依赖驱动子进程，因此 PR-B 的 Driver 改动不会使其失败。

---

## 9. 待确认清单（HOLD，不阻塞架构；PR-B 不得据此编造事实）

| # | 待确认项 | 当前处置 |
|---|---|---|
| 1 | 19 条驱动参数的**手册页码/章节**证据 | 未提供前一律 `parameter_identity`，ID 不得升级为业务语义名（§8.1-6、§8.2） |
| 2 | `r0031` 的访问能力（真机复测） | 定义保留 + `candidate` + HOLD（§8.1-7、`GATE.md:163-180`） |
| 3 | 驱动对象 `V/n` 的**轴归属** | HOLD；需外部证据或受控单轴运动测试（超只读范围，需另行批准）（§8.1-5） |
| 4 | 840D sl 的定义目录 | 无定义；引用时明确报「该系列无定义」（§8.1-8） |
| 5 | `Module 130` 的**正式入口名与硬件适用性** | 候选入口；PR-B 只按 `identity_spec.rs:194-205` 的探针常量对齐，**不**宣称硬件适用性（`GATE.md:113-119`） |
| 6 | front-end 基线 **33 文件 / 383 用例** | **本次未复跑**（未启动任何前端服务）。该数字来自上位设计 §6.0，属**转述**；PR-B 门禁 G9 必须实跑确认，**不得**用 383 作为「已核实」的事实 |
| 7 | 契约套件**用例级**总数 | 本次只核实**套件数 = 29**（`scripts/contract_suites.py:13-52`）；用例级数字（`GATE.md:61` 记 187/187 于 `d29c944`）**已过期**，PR-B 必须以实跑输出为准 |
| 8 | §4.1.1 长度上限的核心依据「42 条定义默认 Key 最长 71 字节」 | 该数字来自上位设计的推导（§4.1.1 长度上限）；PR-B 定义目录落地后必须**实测**最长 Key 并写进 PR 描述（Core 上限 256 字节**不放宽**） |
| 9 | **【已裁决修正】C10 的残留风险**：Descriptor 的 `variable` 资源仍被公告，但 `mesa.resources.v1` 绑定点位会被 `UNSUPPORTED_BINDING` 拒绝 ⇒ UI 可能展示**不可用**的配置路径 | **已知取舍，不在 PR-B 修复**：保留消费者兼容优先；UI 侧收敛归 **PR-C/PR-D**（§8 C10 契约第 5 条）。PR-B 描述必须显式声明该不一致，**不得**在 PR-B 内顺手改 Descriptor（会破坏 `descriptor_contract.rs` 冻结面） |
| 10 | **【已裁决修正】C2 的独立复现方向**：把仓库**复制到临时目录**、删掉副本中的 `presets/828d.json`、再跑 `--test semantic_equivalence`，是否**仍然** `checked == 62` | **本次未执行**（本文件编写期间**未构建、未运行任何测试**，也未启动 `mesad`/Vite）。这是「新门不依赖旧模板」的直接证明，**必须在 PR-B 门禁中实跑**并按 §4.2 第 5 条 ④ 记录结论；**不得**用「源码级零引用断言通过」代替该实跑 |
| 11 | **【已裁决修正】C5 的目录位置**：`drivers/sinumerik-nck/definitions/` 与 `src/` 平级的最终路径 | 本文件已按 `presets.rs:23` 的同构先例定为 `include_str!("../definitions/828d/r1.json")`；**上位设计 §1.1/§7 的 `src/definitions/**` 与之冲突且逐字不改** ⇒ 若评审要求与上位设计字面一致，需修订上位设计（**超出本次授权范围**），**不阻塞 PR-B 开工** |

---

## 附录 A：本次使用过的只读核实命令（可复现）

```powershell
# 仓库状态（只读）
git status --short ; git rev-parse HEAD ; git branch --show-current
git log --oneline -8 ; git merge-base HEAD main
git ls-files --error-unmatch crates/core-types/src/point.rs tests/fixtures/nck-point-key-golden.json crates/core-api/src/binding_gate.rs

# 文件存在性与行数
Get-ChildItem -Recurse drivers\sinumerik-nck\src | Select-Object FullName,Length
Get-ChildItem -Recurse drivers\sinumerik-nck | Where-Object { -not $_.PSIsContainer }
Test-Path drivers\sinumerik-nck\definitions
Get-ChildItem -Recurse -Filter 'DIGESTS.lock'
Get-ChildItem docs\adr ; Get-ChildItem docs\evidence ; Get-ChildItem docs\design
(Get-Content <file>).Count          # 行数一律用这个，不用 Measure-Object -Line

# 冻结指纹
Get-FileHash -Algorithm SHA256 <file>

# 62 条 wire golden 实测
$w = Get-Content drivers\sinumerik-nck\tests\fixtures\legacy-828d-wire.json -Raw | ConvertFrom-Json
$w.entries.Count ; $w.entries | Group-Object entry ; $w.entries | Group-Object area
($w.entries | Select-Object -ExpandProperty area_no | Sort-Object -Unique)
($w.entries | Select-Object -ExpandProperty line  | Sort-Object -Unique)
$w.entries | ForEach-Object { ($_.wire_hex -split '\s+') [2] } | Group-Object

# 62 个模板实测
$j = Get-Content drivers\sinumerik-nck\presets\828d.json -Raw | ConvertFrom-Json
$j.templates.Count ; $j.templates | Group-Object { $_.parameters.area } ; $j.templates | Group-Object { $_.parameters.module }
(Get-Content drivers\sinumerik-nck\presets\828d.json)[29..32]

# E1–E11 golden 实测
$g = Get-Content tests\fixtures\nck-point-key-golden.json -Raw | ConvertFrom-Json
$g.PSObject.Properties.Name ; $g.value_cases.Count ; $g.value_reject_cases.Count ; $g.key_cases.Count
($g.key_cases | ForEach-Object { $_.id }) -join ', '
```

**【已裁决修正】本次修订（C1–C10）额外使用过的只读核实命令（可复现）**：

```powershell
# 0) 修订基线与「代码面未变」证明（只读）
git rev-parse HEAD ; git branch --show-current
git log --oneline -5
git diff --stat d17b0dd 2977ca0        # 期望：只有本文件 1 file changed

# 1) C2/C10 新冻结文件的摘要与行数（本次实测值写入 §0）
Get-FileHash -Algorithm SHA256 drivers\sinumerik-nck\presets\828d.json
Get-FileHash -Algorithm SHA256 drivers\sinumerik-nck\tests\legacy_equivalence.rs
Get-FileHash -Algorithm SHA256 drivers\sinumerik-nck\tests\presets_828d.rs
Get-FileHash -Algorithm SHA256 tests\driver-contract\tests\descriptor_contract.rs
Get-FileHash -Algorithm SHA256 tests\driver-contract\tests\data_semantics.rs
Get-FileHash -Algorithm SHA256 tests\driver-contract\tests\resource_contract.rs
(Get-Content drivers\sinumerik-nck\tests\legacy_equivalence.rs).Count     # 134
(Get-Content drivers\sinumerik-nck\tests\presets_828d.rs).Count          # 305
(Get-Content tests\driver-contract\tests\descriptor_contract.rs).Count   # 1094

# 2) C2 证据：模板 parameters 被当「原生字段」读（用例计数）
Select-String -Path drivers\sinumerik-nck\tests\legacy_equivalence.rs -Pattern '#\[test\]'
Select-String -Path drivers\sinumerik-nck\tests\presets_828d.rs       -Pattern '#\[test\]'
(Get-Content drivers\sinumerik-nck\tests\legacy_equivalence.rs)[61..93]
(Get-Content drivers\sinumerik-nck\tests\presets_828d.rs)[14..26]

# 3) C3 证据：原生解析器白名单恰 12 项、未知键即拒
(Get-Content drivers\sinumerik-nck\src\native.rs)[145..159]
(Get-Content drivers\sinumerik-nck\src\native.rs)[227..235]

# 4) C10 证据：受影响断言的实际内容
(Get-Content tests\driver-contract\tests\descriptor_contract.rs)[598..605]
(Get-Content tests\driver-contract\tests\descriptor_contract.rs)[724..731]

# 5) C6 证据：discovery_contract 无叶形态 / 无 binding_json 断言
Select-String -Path tests\driver-contract\tests\discovery_contract.rs -Pattern 'binding_json|leaf|children'
(Get-Content tests\driver-contract\tests\discovery_contract.rs)[223..273]

# 6) C4 证据：旧错误码表的规模与矛盾数字
(Get-Content docs\design\nck-semantic-point-refactor.md)[676..690]   # §2.6 = 13 行
(Get-Content docs\design\nck-semantic-point-refactor.md)[703..708]   # §2.7 Driver 行 = 新增 8
Select-String -Path docs\design\nck-semantic-point-refactor.md -Pattern '新增 \*\*7 个 Driver 码\*\*'

# 7) C1/C5/C7 证据：上位设计的旧表述行号
Select-String -Path docs\design\nck-semantic-point-refactor.md -Pattern '变体数|恰好一个'
Select-String -Path docs\design\nck-semantic-point-refactor.md -Pattern 'definitions/|r1\.json'
(Get-Content docs\design\nck-semantic-point-refactor.md)[434..465]   # §2.4.1 TargetRef 结构
(Get-Content docs\design\nck-semantic-point-refactor.md)[694..709]   # §2.7 / §2.8 #10

# 8) C5 证据：定义目录所用 include_str! 的同构先例
(Get-Content drivers\sinumerik-nck\src\presets.rs)[17..29]

# 9) 同步性与残留冲突表述检索（本次修订后必须零命中，见 §5.1 末）
Select-String -Path docs\design\nck-prb-driver-compiler-boundary.md -Pattern '变体数 = 1|恰好一个变体'
Select-String -Path docs\design\nck-prb-driver-compiler-boundary.md -Pattern '13 个文件|16 个 Driver|见 §C2：建议|随资源面改写'
Select-String -Path docs\design\nck-prb-driver-compiler-boundary.md -Pattern 'build --workspace|mesad|vite'
git status --short      # 期望：仅 1 个文档被修改（M），无新增/删除
```


> 声明（**本次定向修订**）：本次**只修改** `docs/design/nck-prb-driver-compiler-boundary.md` 这一个文件；**未修改**任何代码、测试、配置或其它文档（尤其**未改动已冻结**的 `docs/design/nck-semantic-point-refactor.md`；与它的冲突全部在 §8 / §8.1 如实记录，并**以本次裁决为准**）；**未执行任何 git 写操作**（无 commit / push / checkout / stash / reset / PR / tag）；**未构建、未运行任何测试**；**未启动 `mesad`**、**未启动 Vite**、**未连接任何真实设备或机床**。
