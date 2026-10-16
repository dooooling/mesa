# PR-B 实施边界与验收清单：SINUMERIK NCK 语义点位与 Address Compiler

- 状态：**边界与验收草案（供评审裁决是否批准开工）**。本文件**不是**设计定稿，**不是**代码。
- 上位设计：`docs/design/nck-semantic-point-refactor.md`（1387 行，已冻结，本文件**逐字不改**它）
- 本文件核实基线：分支 `docs/nck-prb-boundary`，HEAD `d17b0dd`（= `main`，PR-A「Core 契约」已由 PR #109 合入）
- 本次交付方式：**只新建本文件**；未改任何代码 / 测试 / 配置 / 其它文档；**无任何 git 写操作**；未启动 `mesad`、未启动 Vite、未连接任何真实设备或机床。
- 阅读口径：本文件所有**行号**均为本次在 `d17b0dd` 上实际读取所得；与上位设计 §0 的转述不一致处，一律以本文件的实测值为准并在 §C 显式列出。

---

## 0. 本次核实的冻结指纹（供「逐字不变 / 逐字不动」验收直接比对）

下表 SHA-256 由 `Get-FileHash -Algorithm SHA256` 本次本地计算，行数由 `(Get-Content <f>).Count` 取得。
**PR-B 提交后必须由评审重新计算同名文件的摘要并逐条比对**：判定为「逐字不动」的文件摘要**必须完全相等**；判定为「修改」的文件摘要**必然变化**（因此摘要同时是「有没有偷偷改到冻结面」的证据）。

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
| `drivers/sinumerik-nck/presets/828d.json` | 1256 | `e910b0558dea80ca1f2b2d8bcdd566c3bc4cd725abe71544d34f9ede0380ebfa` | 见 §C2：**建议逐字不动** |
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
| `drivers/sinumerik-nck/definitions/`（目录） | §1.1 / §3.1「新增」 | **不存在** | `Test-Path drivers\sinumerik-nck\definitions` → `False` |
| `definitions/828d/DIGESTS.lock` | §1.1 / §3.1 / §6.2「新增」 | **不存在** | 全仓 `-Filter DIGESTS.lock` 无命中 |
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
| 契约套件 29 套 | `scripts/contract_suites.py:16-52`（BASELINE 14 + EVENT 12 + LIFECYCLE 3 = 29） | ✅ |

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
| N4 | `drivers/sinumerik-nck/definitions/828d/r1.json` | 42 条定义 × `revision = 1` 的定义目录数据（含 `identity_template`/`identity_keys`/`golden_ref`/`semantics_status`/`source`） | §2.5.1、§3.1、§3.4 | 42 条 = A 16 + B 20 + C 6（§2.5.1 计数校验） |
| N5 | `drivers/sinumerik-nck/definitions/828d/DIGESTS.lock` | 已发布 `(variable_id, revision)` 的 canonical-JSON SHA-256 清单（含发行文件路径） | §3.1、§3.2、附加 2 | 只允许**新增**条目；由 T6 对照基线分支校验 |
| N6 | `drivers/sinumerik-nck/tests/definition_catalog.rs` | 定义目录完整性 / revision 不可变性 / digest / 语义状态 / `golden_ref` 双射 / A 项**静态**判据 | §6.2 | 见 §6.1 的 `catalog_*` 行 |
| N7 | `drivers/sinumerik-nck/tests/compiler_identity.rs` | Driver 侧身份契约与重算拒绝：T3b / T4 / T5(Driver) / **A 项反例** / **B 项命名空间** | §6.2、§6.6 T3b/T4/T5、本文件 §6.2/§6.3 | 见 §6.1 |
| N8 | `drivers/sinumerik-nck/tests/semantic_equivalence.rs` | 语义路径逐条复现 62 条冻结字节；G4 双入口不折叠 | §3.4、§6.2、§6.5 | `checked == 62` |
| N9 | `tools/ci/nck_definition_lock.py` | **对照基线分支**校验 `DIGESTS.lock`（附加 2）：基线已发布项不可变、只允许新增、不允删除 | §3.1、§6.2、§6.6 T6 | 三个场景可执行；离线；无第三方依赖 |
| N10 | `tools/ci/test_nck_definition_lock.py` | 上述工具的**自测**（临时 git 仓库构造三场景），沿用 `tools/ci/test_focas_diff.py` 的「PR-A 工具门」模式 | `_rust-quality.yml:37-41` 既有先例 | 三场景 + 当前树自洽，共 ≥4 断言组 |
| N11 | `docs/adr/0005-nck-semantic-point-definition.md` | 本次破坏性重构的 ADR 留痕（含 PR-A 已落地的 Core 侧契约追溯） | §1.1、§5 | 逐条覆盖 §5 的 9 项破坏 |

**相对上位设计的偏差（须评审确认）**：
- §1.1 只列了 `definition.rs`（单数）；本轮任务书写 `definitions.rs`（复数）。**本文件统一采用 `definition.rs`**（与 §7 PR-B 范围、§7.0-4 的独占文件清单一致）。见 §C8。
- §1.1 只列了 `definition_catalog.rs` 与 `semantic_equivalence.rs` 两个新测试目标；本文件**增加 `compiler_identity.rs`**（N7）。理由：T3b/T4/A/B 的信号必须能在**单独一个测试目标**里定位失败原因，混入 `definition_catalog.rs`（目录数据断言）会让「数据错误」与「编译器逻辑错误」在同一目标里不可区分。
- N9/N10 是 §6.2/附加 2 要求的**可执行**落点，但 §1.1 未列文件，属本文件补充。
- §1.1 列了 `apps/mesa-web/src/nckPointModel.ts` 等前端产物——**那是 PR-C 的范围**，不在本清单（§7.0-4、§8）。

### 2.2 【修改】

| # | 路径 | 改动点（本次实测行号） | 说明 |
|---|---|---|---|
| M1 | `drivers/sinumerik-nck/src/lib.rs` | ① `configure` **整体重写**：`:596-767`（今日为 `GenericBinding` 单一路径）；② Descriptor 的 `variable` 资源面：`:170-331`（`resources: vec![ResourceDescriptor{…}]` 起 `:170`、12 字段 `:178-311`、`outputs` `:313-319`、`resource_selection_methods` `:324-327`）；③ 内嵌单测 `:1379-1814` 的 10 个用例迁移（见 §1.3 末段） | **不得触碰**：`decode_point` `:430-523`、`pack_array` `:526-586`、`probe` `:592-594`、`apply_point_map` `:769-788`、`run`/poll 数据面 `:792-1003`、`browse` `:943-1003`、`paginate` `:1010-1031`、`GENERIC_RESOURCE_ID` 常量 `:52` |
| M2 | `drivers/sinumerik-nck/src/lib.rs:599-767` 的**语义变化**（单列，便于评审逐条核对） | 入口从「读 `GenericBinding` → 逐 output → `from_parameters`」改为「读 `PointBinding` → **逐 point 按 Point Kind 路由到 Address Compiler** → 逐点 `CompileError` → 任一点失败即**整体失败**」 | `:644`（`resource_id`）与 `:653`（`output`）两条硬校验**删除**；`:613` 的 kind 判定改为 `NCK_POINTS_BINDING_KIND` |
| M3 | `drivers/sinumerik-nck/src/lib.rs:740-752` | Descriptor 构造与 `ensure_unique_point_keys` 保留 | `DUPLICATE_POINT_KEY`（`:750-752`）行为不变（驱动侧同任务判重） |
| M4 | `drivers/sinumerik-nck/src/presets.rs:151-186`（`leaf_node` / `leaf_node_with`） | 浏览叶 `binding_json` 从 `{"resource_id":"variable","parameters":{…}}`（`:167-171`）改为**语义信封** `{"kind":"nck_variable","parameters":{variable_id,definition_revision,target,arguments}}`，并**旁挂** `identity` 块（`template`/`keys`/`preview_point_key`） | `identity` **只出现在浏览/预填载荷**，不得进入持久化信封（§4.1.1 C）。`PresetTemplate.parameters`（`:27` 起）**保持原生字段不变**——见 §C2 |
| M5 | `drivers/sinumerik-nck/Cargo.toml:16-27` | 新增依赖 `sha2 = { workspace = true }`（workspace 已有 `sha2 = "0.10"`，`Cargo.toml:68`） | 摘要算法用 |
| M6 | `drivers/sinumerik-nck/presets/README.md` | `:33-52`（索引语义表）后补「语义 `variable_id` ↔ 原生身份 ↔ 62 条 golden」对应章节；`:83-88`（`r0035` 哨兵）后补 `semantics_status` 说明；`:110` 后补 26/130 入口与 `identity_template` 的 `drive_object` 递归编码说明 | 文档；`PROVENANCE.md:9-54` 的 42 条清单**不删** |
| M7 | `tests/driver-contract/tests/descriptor_contract.rs` | `:605`（`d.resources.iter().any(\|r\| r.id == "variable")`）与 `:729-731`（`find(\|r\| r.id == "variable")` + `outputs[0].type_spec == DriverResolved`）随资源面改写 | **不得放宽**：`:600-604` 的 poll/无 subscribe/无 write/无 events/无 commands，以及 `:391-399` 的「生产 Write 未开发前不得声明 write」 |
| M8 | `tests/driver-contract/tests/discovery_contract.rs:224-274` | **不改写**既有断言；**只新增**一条浏览叶形态断言（`nck://presets` 下取一叶，断言其 `binding_json` 为语义信封且**不含**原生字段） | 实测该套件当前**根本没有叶形态断言**（`:255-260`、`:270-273` 只断言根 2 节点 + 诊断节点 + 未知 parent 空页）→ 见 §C6 |
| M9 | `.github/workflows/_rust-quality.yml` | 增加 `fetch-depth: 0`（当前 `:21-22` 为默认浅克隆）并把 PR base SHA 传入；新增一步 `python3 tools/ci/nck_definition_lock.py --base "$BASE"` | `ci.yml:53-60` 已有 `fetch-depth: 0` + `BASE: ${{ github.event.pull_request.base.sha \|\| github.event.before }}` 的**先例**，但那是 `plan` job；quality job 目前没有 base 信息 |

**不修改**（明确声明，防止范围蔓延）：`drivers/sinumerik-nck/presets/828d.json`（§C2 建议）、`drivers/sinumerik-nck/tests/legacy_equivalence.rs`、`drivers/sinumerik-nck/tests/presets_828d.rs`、`crates/**`、`proto/**`、`apps/mesa-web/**`、`drivers/s7|focas2|opcua/**`。

### 2.3 【逐字不动】

| 路径 | 冻结理由 | 验收方式 |
|---|---|---|
| `drivers/sinumerik-nck/src/client.rs`、`codec.rs`、`value.rs`、`address.rs`、`identity_spec.rs`、`identity.rs`、`native.rs` | 执行层 / 协议常量 / 身份探针（§1.3 冻结） | §0 摘要逐字相等 |
| `drivers/sinumerik-nck/src/lib.rs` 的 `decode_point`（`:430-523`）与 poll 执行面（`:526-586`、`:769-1003`） | 运行质量处理与读取路径（P6） | 以 `:430-523`、`:526-586`、`:769-1003` 三个行号区间为准做区间级 diff 空断言 |
| `drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json` | 62 条 wire golden 真值 | §0 摘要逐字相等 |
| `tests/fixtures/nck-point-key-golden.json` | E1–E11 **三处共用**向量 | §0 摘要逐字相等 + 断言三处引用同一路径（禁止出现第二份副本，见 §5.2） |
| `tests/driver-contract/tests/data_semantics.rs`（含 T2/T3a 的 8 个语义用例） | PR-A 已冻结契约 | 原样通过，用例数不减少 |
| `crates/core-types/src/point.rs`、`crates/core-api/src/binding_gate.rs` | PR-A 已冻结 Core 契约（§7.0-4：`point.rs` 为 PR-A 独占） | 摘要逐字相等 |
| `docs/design/nck-semantic-point-refactor.md` | 上位设计 | 摘要逐字相等 |
| `tests/driver-contract/tests/resource_contract.rs:396-460`（P9 护栏） | 修订 1 的护栏 | 原样通过 |

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
    /// 变体集合校验：见下方 D7 修正。
    pub fn validate(&self, def: &VariableDefinition) -> Result<(), CompileError>;
    /// 已提供变体的实例号（供 `NativeFieldSource::Target` 取值）。
    pub fn instance(&self, kind: TargetKind) -> Option<u32>;
}
```

**Module 26 / 130 入口身份如何区分与校验（这是本项的核心，必须可验收）**：

1. **类型层强制**：`DriveObjectRef.entry` 为**必填、非 `Option`、无 `Default`** → 缺 `entry` 在 `serde` 反序列化阶段即失败 → `INVALID_TARGET`。**不存在**「默认 module 26」的隐式回退。
2. **映射层唯一实现**：`DriveEntry::module()` 是 26/130 的**唯一**出处；`NativeFieldSource::DriveEntryModule` 只允许调用它。
3. **常量对齐（构建期自检，可执行）**：`identity_spec.rs:177-189` 的 `DRIVE_OBJECT_NAME.module == 26`、`identity_spec.rs:194-205` 的 `DRIVE_OBJECT_NAME_M130.module == 130`，且两者 `module` **必须不同**。PR-B 必须新增单测 `drive_entry_module_matches_identity_spec_constants` 直接断言这三条。
4. **不可混淆的可执行反例**：同 `number` + 同 `column` 下，`entry=nc_variable` 与 `entry=drive_object` 必须产出**不同 module、不同 `wire_hex`、不同 canonical identity**（60 个字节组合中的 20 对，全部断言）。
5. **真机证据口径**：26 与 130 是对同一 `V/n` 返回**不同对象名**的两条入口（`identity_spec.rs:190-205` 注释、`docs/evidence/nck-remaining-20-reconciliation.md:57-74`），因此**目录中同一驱动参数必须存在两条 `entry` 变体**，不得折叠成一条带默认值的定义。

**【PR-B 修正】D7 的「变体数 = 1」必须改为「变体集合合法性」**（见 §C1）：
```text
target 合法 ⟺
  (a) 提供的变体集合 S = { k | target.<k> 非 null }（drive_object 计为一个变体）
  (b) S ⊇ def.target_required
  (c) S ⊆ def.targets
  (d) S ≠ ∅
  (e) 每个 k ∈ S 的实例号 ∈ 0..=255，且 drive_object.entry 必填
```

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
| `compile_native` | 来源 A 的 `parameters`（12 原生字段 + 可选 `identity_mode`） | 同上（**剥离 `identity_mode` 后**传入，见 §C3） |
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
- **`identity_mode="user"`**：**仅** `nck_native` 允许；Driver **不重算、不静默改名**，但仍复核通用语法与命名空间（§6.3）。
- **`nck_variable` 不允许 `identity_mode`**（字段本身非法）或 `identity_mode="user"` ⇒ `INVALID_BINDING_CONFIG`。
- **`nck_native` 且 `identity_mode="derived"`** 的固定规则（§4.1.1 C 表末行）：`native.<area>.<area_no>.<module>.<column>.<line>`。**不含** `count`/`decode`/`transport_size`/`invalid_*`/`identity_mode`（§4.1.2 末段）。
- **顺序不可交换**：D1–D10 **先**、身份计算**后**（§4.1.1 D-1）。
- **`definition_revision` 不进入身份**（I3）：同一 `variable_id` 的所有 revision 身份逐字相同（T3b）。

### 3.5 Driver 专属错误码全集（PR-B 部分）

| code | `ErrorKind` | 触发 | fail-closed | 现状 |
|---|---|---|---|---|
| `UNSUPPORTED_BINDING` | `Unsupported` | Task Binding Kind ≠ `nck.points.v1` | 是 | 保留原 code（`lib.rs:614-620`） |
| `INVALID_BINDING_CONFIG` | `Configuration` | 信封/点位结构错、`parameters` 层未知字段或类型错（D2/D3）、`identity_mode` 取值错或 `user` 用于 `nck_variable` | 是 | 保留原 code（`lib.rs:635-638`） |
| `INVALID_TASK` | `Configuration` | `task.validate()` 失败 | 是 | 保留（`lib.rs:611-612`） |
| `MODE_NOT_SUPPORTED` | `Unsupported` | 非 Poll 调度 | 是 | 保留（`lib.rs:624-631`） |
| `UNSUPPORTED_POINT_KIND` | `Unsupported` | Point Kind ∉ {`nck_variable`,`nck_native`}（D1） | 是 | 新增 |
| `INVALID_VARIABLE` | `Address` | 复用：`from_parameters` / `resolve_native` 失败（D9）、原生索引越界、`area_no:"1"` 字符串形态 | 是 | 保留原 code（`lib.rs:673`、`:681`） |
| `UNKNOWN_VARIABLE` | `Address` | `variable_id` 不在目录（D5） | 是 | 新增 |
| `UNSUPPORTED_DEFINITION_REVISION` | `Configuration` | 目录无该 `(variable_id, revision)`（D6）；**不得回落** | 是 | 新增 |
| `UNSUPPORTED_FAMILY` | `Unsupported` | `def.family` ≠ 设备 family（如 840d-sl 却引用 828d 定义） | 是 | 新增（§2.6 有、§2.7 漏，见 §C4） |
| `UNSUPPORTED_TARGET` | `Configuration` | 变量不支持该 Target 类型（D7） | 是 | 新增 |
| `MISSING_TARGET` | `Configuration` | 缺 `target_required` 中的类型（D7） | 是 | 新增 |
| `INVALID_TARGET` | `Configuration` | 变体集合非法 / 实例号越界 / `entry` 缺失（D7/D10） | 是 | 新增 |
| `INVALID_ARGUMENTS` | `Configuration` | 未知/缺失/越界/类型错的 argument；`ArgumentOffset` 上溢（D8） | 是 | 新增 |
| `MISSING_IDENTITY_KEY` | `Configuration` | `identity_keys` 声明的键**不存在**（§4.1.1 C：缺失即拒；`null` 走 `INVALID_ARGUMENTS`） | 是 | 新增（§2.6 漏，见 §C4） |
| `DEFINITION_DIGEST_MISMATCH` | `Internal` | 内嵌定义自检摘要不符（构建期问题） | 是 | 新增（§2.6 有、§2.7 漏） |
| `POINT_KEY_MISMATCH` | `Configuration` | 持久化 `point_key` ≠ 重算身份（D11） | 是 | 新增 |
| `INVALID_USER_IDENTITY` | `Configuration` | `identity_mode="user"` 的 Key 不匹配 `native.` 命名空间（**B-1**，§6.3） | 是 | **新增（本文件补充，须评审批准）** |
| `IDENTITY_NAMESPACE_COLLISION` | `Configuration` | 同一 configure 批次内两个点位的最终 `point_key` 相同（**B-2**，§6.3） | 是 | **新增（本文件补充，须评审批准）** |
| ~~`UNSUPPORTED_RESOURCE`~~ / ~~`UNSUPPORTED_OUTPUT`~~ | — | NCK 侧**不再产生**（`lib.rs:646`、`:655` 删除，§1.4） | — | 常量保留供其它驱动/SDK |
| `DUPLICATE_POINT_KEY` | `Configuration` | 驱动侧同任务内 `point_key` 重复（保留 `lib.rs:750-752`） | 是 | 保留 |
| `MISSING_POINT_ID` / `NOT_CONFIGURED` | — | `apply_point_map` 既有语义（`lib.rs:769-788`） | 是 | 不变 |

### 3.6 参数层严格校验 D1–D11 的落地位置

| # | 校验项 | 失败码 | 实现位置 |
|---|---|---|---|
| D1 | Point Kind ∈ {`nck_variable`,`nck_native`} | `UNSUPPORTED_POINT_KIND` | `compiler.rs::compile_point` |
| D2 | `parameters` 层未知字段 | `INVALID_BINDING_CONFIG` | `compiler.rs` 的两个 `#[serde(deny_unknown_fields)]` 输入结构 |
| D3 | 类型错误 | `INVALID_BINDING_CONFIG` | 同上（serde）+ `from_parameters`（12 字段） |
| D4 | 索引范围 | `INVALID_VARIABLE` / `INVALID_TARGET` | `from_parameters`（原生）/ `TargetRef::validate` |
| D5 | `variable_id` 存在性 | `UNKNOWN_VARIABLE` | `DefinitionCatalog::lookup` |
| D6 | `definition_revision` 可解析性（**不得回落**） | `UNSUPPORTED_DEFINITION_REVISION` | 同上 |
| D7 | `target` 形态与合法性（**见 §C1 修正**） | `INVALID_TARGET` / `UNSUPPORTED_TARGET` / `MISSING_TARGET` | `TargetRef::validate` |
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
2. **旧门原样**：`tests/legacy_equivalence.rs` **一个字都不改**（2 个用例，`checked == 62`）。
   ⚠️ **这条成立的前提是 `presets/828d.json` 的 `parameters` 保持原生字段**——因为 `legacy_equivalence.rs:62-94` 把模板 `parameters` 当**原生字段**读并断言 `module/column/decode/transport_size/count` 与冻结向量一致。见 §C2。
3. **复用为回归基线**：`tests/semantic_equivalence.rs` 从**定义目录**出发（而不是从模板），经 `GoldenRef` 定位冻结条目，断言：
   - `checked == 62`（不得跳过、不得抽样）；
   - 每条 `encode_var_spec(&compiled.resolved.wire) == bytes(e["wire_hex"])`；
   - 20 条 `drive_object` 与其 `nc_variable` 孪生条目 **column 相同、module 不同、wire 不同**（G4）；
   - **语义 payload → 12 原生字段的映射与冻结条目逐字段一致**（`module`/`column`/`decode`/`transport_size`/`count`/`area`/`area_no`/`line`）。

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
- 若采纳 §C2 的推荐方案（模板不动），则该断言同时证明「旧模板集合」与「新定义目录」**逐个对应**，且**不需要**改动模板文件。
- 若评审选择 §C2 的备选方案（模板语义化），则必须**同时**重写 `legacy_equivalence.rs` 与 `presets_828d.rs`——这会直接违反 §1.3/§6.2 的「原样通过」，**并削弱**「用冻结的旧数据做真值、不是用新代码自证新代码」这一性质（该性质写在 `legacy_equivalence.rs:12` 的注释里）。

---

## 5. 交付物④：验收矩阵

### 5.1 测试矩阵（测试名 / 断言要点 / 所在文件与套件 / 证明什么 / 对应上位设计规则）

> 「套件」列中：**驱动自有目标** = `cargo test -p mesa-driver-sinumerik-nck --test <name>`；**契约套件** = `cargo test --locked -p mesa-contract-tests --all-features`（29 套之一，`scripts/contract_suites.py:16-52`）。

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
| 22 | `identity_mode_is_not_forwarded_to_native_parser` | `nck_native` = 12 字段 + `identity_mode` ⇒ 成功，且 `resolved.wire` 与「同 12 字段直接 `from_parameters`」**逐字节相等**；把 `identity_mode` 直接交给 `from_parameters` ⇒ `UnknownField`（证明 `native.rs:147-160` 未被放宽） | 同上 | **复用而非复制**；`native.rs` 逐字不动 | §2.6 契约 2/3、§C3 |
| 23 | `unsupported_definition_revision_rejects_all_three_degradations` | ① 不回落最新；② 不回落其它 revision；③ 不按 `variable_id` 猜地址（三例分别断言） | 同上 | §3.3 三种禁止退化 | §3.3 |
| 24 | `argument_offset_boundaries` | `channel.r_parameter` `index=0` ⇒ `line=1`；`index=65535` ⇒ 上溢 ⇒ `INVALID_ARGUMENTS`；`index=-1` ⇒ 拒 | 同上 | `ArgumentOffset` 边界 | §2.5.1 A#16、§2.4.2 D8 |
| 25 | `a_identity_completeness_channel` | 同 `variable_id`、同其余参数，`channel=1` vs `channel=2` ⇒ **身份不同** 且 **物理地址 `area_no` 不同** | 同上 | **A 项反例：Channel** | §4.1.1 I1 |
| 26 | `a_identity_completeness_mode_group` | `mode_group=1` vs `2` ⇒ `area_no` 不同 ⇒ 身份不同 | 同上 | **A 项反例：ModeGroup** | §4.1.1 I1、§8.1-2 |
| 27 | `a_identity_completeness_axis` | `axis=1` vs `2`（`channel` 相同）⇒ `line` 不同 ⇒ 身份不同 | 同上 | **A 项反例：Axis** | §4.1.1 I1、表 A#7/#8 |
| 28 | `a_identity_completeness_spindle` | `spindle=1` vs `2`（`channel` 相同）⇒ `line` 不同 ⇒ 身份不同 | 同上 | **A 项反例：Spindle** | §4.1.1 I1、表 A#12–14、§8.1-4 |
| 29 | `a_identity_completeness_drive_object` | `drive_object.number=3` vs `4`（`entry` 相同）⇒ `area_no` 不同 ⇒ 身份不同 | 同上 | **A 项反例：DriveObject** | §4.1.1 I1、表 B |
| 30 | `a_identity_completeness_drive_entry_26_vs_130` | 同 `number`、同 `column`，`entry` 26 vs 130 ⇒ `module` 不同 ⇒ 身份不同、wire 不同 | 同上 | **A 项反例：入口身份**；26/130 不折叠 | §2.4.2 D10、G4、§6.5 |
| 31 | `a_identity_distinguishes_exactly_the_physical_target` | **机械差分（最强）**：对 42 条定义，枚举每个 `addressing` 来源的取值域样本，断言 `canonical_identity(A) == canonical_identity(B)` **当且仅当** 解析出的原生地址元组 `(area, area_no, module, column, line, count)` 相等 | 同上 | **A 项完整性判据的动态证明**：不存在「物理目标不同却同身份」的定义 | §4.1.1 I1/I2 |
| 32 | `b_user_key_must_stay_in_native_namespace` | `identity_mode="user"` 且 `point_key` 不以 `native.` 开头/不匹配 `native_key` 语法 ⇒ `INVALID_USER_IDENTITY`（含 `point_key="channel.r_parameter#index=i:100@channel=i:1"` 一例） | 同上 | **B-1**：不得冒用语义点位身份 | 本文件 §6.3、§4.1.1 E |
| 33 | `b_user_key_collision_within_batch_rejected` | 同批次两点：A `derived`（地址 `C/1/21/1/396`）与 B `user`（`point_key="native.C.1.21.1.396"`，地址 `C/1/21/1/999`）⇒ `IDENTITY_NAMESPACE_COLLISION`，configure 整体失败 | 同上 | **B-2**：不同物理目标不得共享身份 | 本文件 §6.3 |
| 34 | `b_user_key_cannot_shadow_semantic_identity` | B-1 + 结构论证：语义身份**恒含 `@`**（I1）且不以 `native.` 开头，`native.` 前缀在 Core 侧被强制为 `native_key` 形态（`point.rs:428-431`）⇒ 结构上不可能与语义 Key 相同；本用例用一条**试图**冒用的 Key 断言被拒 | 同上 | **B**：命名空间隔离是**结构性**的，不靠事后判重 | §4.1.1 E、§4.1.2 |
| 35 | `t5_encoder_matches_e1_e11_golden_vectors` | 复用 `tests/fixtures/nck-point-key-golden.json` **同一份**：`value_cases`/`value_reject_cases`/`key_cases` 逐条与 Driver 编码/校验路径一致 | 同上 | T5（Driver 侧）三处同源 | §6.6 T5、§4.1.1 J |
| 36 | `t1_mixed_point_kinds_route_per_point` | 一个任务的 `points[]` 同时含 `nck_variable` 与 `nck_native` ⇒ configure **成功**，逐点路由到两条分支（用点位级断言证明路由，而不是只看总数） | 同上 | T1（Driver 侧） | §2.1 评审裁决、§6.6 T1 |
| 37 | `entry_missing_is_invalid_target` | `drive_object` 缺 `entry` ⇒ `INVALID_TARGET`；**无默认 module 26** | 同上 | `DriveEntry` 必填 | §2.4.1、§6.2 |
| 38 | `semantic_path_reproduces_all_62_frozen_wire_bytes` | 从定义目录 + `GoldenRef` 出发，62 条逐条 `encode_var_spec == wire_hex`；`checked == 62` | `tests/semantic_equivalence.rs` | 62/62 字节等价（新门） | §3.4、§6.2、§6.5 |
| 39 | `semantic_path_keeps_the_two_entries_apart` | 20 对：column 相同、module 不同、wire 不同 | 同上 | G4 不折叠 | §6.5、G4 |
| 40 | `legacy_path_still_reproduces_62` | `tests/legacy_equivalence.rs` **原样**通过（`checked == 62`） | `tests/legacy_equivalence.rs`（**逐字不动**） | 旧门未失效 | §6.2、1.3 |
| 41 | `templates_never_prefill_instance_or_line` 等 6 个 | `tests/presets_828d.rs` **原样**通过 | 同上 | 模板未被偷偷改动 | §6.2、1.3（前提见 §C2） |
| 42 | `configure_accepts_nck_points_envelope` | `nck.points.v1` 信封 ⇒ configure 成功 | `src/lib.rs` 内嵌单测（改写 `:1444`） | 新入口可用 | §7 PR-B |
| 43 | `configure_rejects_legacy_resource_selection` | 旧 `mesa.resources.v1` + `variable` 资源 ⇒ `UNSUPPORTED_BINDING` | 同上 | §5-1 破坏性变更已落实 | §5-1、§1.4 |
| 44 | `descriptor_is_valid_and_read_only`（改写） | 保留：driver_id、`poll`、**无 subscribe**、**无 write**、**无 events**、无 commands、descriptor < 256KB | 同上（今天在 `:1164`） | 只读能力不得放宽 | §6.2 |
| 45 | 迁移后的 poll/e2e 用例（`:1523`、`:1602`、`:1662`、`:1691`、`:1740`、`:1768`，共 6 个） | 语义信封下的端到端轮询、BAD→heal、会话丢失、transport 不匹配、超 PDU 拒绝、多任务失败 | 同上 | 执行面零回归 | P5/P6、`GATE.md:211-214` |
| 46 | `t6_changed_definition_with_synced_lock_still_fails` | 临时仓库：改定义内容 + 同步改 Lock ⇒ 退出码非 0 | `tools/ci/test_nck_definition_lock.py` | **T6-①**：同步改两者也拦得住 | §3.1 附加 2 |
| 47 | `t6_added_revision_passes` | 临时仓库：仅新增 `(variable_id, revision)` ⇒ 退出码 0 | 同上 | **T6-②**：只允许新增 | §3.1 附加 2 |
| 48 | `t6_deleted_revision_fails` | 临时仓库：删除已发布 revision ⇒ 退出码非 0 | 同上 | **T6-③**：不得删除 | §3.1 附加 2 |
| 49 | `t6_current_tree_is_self_consistent` | 当前树重算摘要 == 当前 Lock | 同上 | 现网自洽 | §3.2 |
| 50 | `data_semantics` 的 8 个语义用例（`:742`、`:755`、`:793`、`:820`、`:838`、`:852`、`:872`、`:891`） | 原样通过 | 契约套件 `data_semantics` | T2/T3a/Core 身份只由 `point_key` 决定 | §6.6 T2/T3a |
| 51 | `core_has_zero_siemens_knowledge` | 原样通过 | 契约套件 `resource_contract`（`:396-460`） | P9：PR-B 未把 Siemens 知识塞进 Core | §5.1 P9 |
| 52 | `descriptor_contract` 的两处改写（`:605`、`:729-731`） | 资源面断言跟随；**只读三条不放宽** | 契约套件 `descriptor_contract` | 能力边界不退化 | §6.2 |
| 53 | `browse_sinumerik_nck_catalog_tree_e2e`（原样）+ 新增叶形态断言 | 根 2 节点 / `nck://presets` / 诊断节点 / 未知 parent 空页 **不变**；**新增**：叶 `binding_json` 为语义信封且不含原生字段 | 契约套件 `discovery_contract`（`:224-274`） | 浏览语义重做、原有断言不退化 | §6.2、§1.2 |
| 54 | 其余 27 套契约套件 | 原样通过 | 契约套件（`scripts/contract_suites.py:16-52`） | 非 NCK 能力零回归 | §5.1 P1/P8 |
| 55 | `point.rs` 的 17 个单测（含 `e1_e11_golden`、`core_has_no_identity_derivation`） | 原样通过 | `crates/core-types` 单测 | PR-A Core 契约未被破坏 | §2.4、§6.1 |
| 56 | 前端全部用例 | 原样通过（**PR-B 零改动前端**） | `pnpm --filter mesa-web test` | 前端零回归 | §5.1 P3/P4、§7 PR-B |

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
| **B-2 批次内身份冲突自检** | configure 处理完一个批次后，任意两个点位的**最终 `point_key`** 相同 ⇒ 整体失败 | `IDENTITY_NAMESPACE_COLLISION` | §4.1.2 第 1 条（同地址不同用途才允许区分）+ A 项风险在 native 侧的镜像。**这是纵深防御**：Core 侧 `binding_gate.rs:112-155` 已做 Endpoint-wide 跨 kind 判重，但 Driver 必须**独立**可验收（见 §C7 的冲突声明） |
| **B-3 `user` 仅限原生** | `nck_variable` 出现 `identity_mode` 或 `user` ⇒ 拒 | `INVALID_BINDING_CONFIG`（复用既有码，不新增） | §2.4.2 D11 |
| **B-4 不静默改名** | Driver 不得追加/改写 `~<n>`；Core 只负责唯一性 | —（无错误码；由 #18 断言「Key 逐字未变」） | 附加 1、§4.1.2 第 3 条 |
| **B-5 语义点位不可改写** | `nck_variable` 的 `point_key` **必须**逐字等于重算身份 | `POINT_KEY_MISMATCH`（复用） | §4.1.1 D-2 |

**对应测试**：#32（B-1）、#33（B-2）、#34（结构性论证 + 冒用被拒）、#17（B-3）、#18（B-4）、#15（B-5）。

### 5.4 PR-B 门禁（与 PR-A 同口径）

**本地强制门禁（离线执行，全部不得连接真机/设备）**：

| # | 命令 | 说明 |
|---|---|---|
| G1 | `cargo fmt --all -- --check` | 格式 |
| G2 | `cargo clippy --workspace --all-targets -- -D warnings` | lint（默认 feature） |
| G3 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | lint（全 feature；CI 形态为 `--locked --all-features`，`_rust-quality.yml:35`） |
| G4 | `cargo build --locked --workspace` | `sha2` 加入后锁定构建（`AGENTS.md`：改驱动后跑子进程类合同测试前必须先构建） |
| G5 | `cargo test --workspace --locked` | 全量单测 + 驱动自有测试目标 |
| G6 | `cargo test --locked -p mesa-contract-tests --all-features` | 契约 29/29（唯一准入基线，`scripts/contract_suites.py:16-52`） |
| G7 | `python3 tools/ci/test_nck_definition_lock.py` | T6 工具自测（4 组断言） |
| G8 | `python3 tools/ci/nck_definition_lock.py --base <PR base SHA>` | 真实基线分支校验（CI 内必须跑；本地可用 `main` 的 SHA） |
| G9 | 前端回归：`pnpm --filter mesa-web install --frozen-lockfile` + `pnpm --filter mesa-web test` | **PR-B 零改动前端**，因此必须**原样全过**（`_web.yml:27-31`） |

**明确不纳入本地强制门禁**：
- **`cargo test --workspace --all-features`**：不作为本地强制门禁（与 PR-A 同口径）。
- **性能**：归 CI 的 `rust-perf`（`_rust-perf.yml:4`、`:36`），本地不跑 benchmark/SLO。
- **真机探针**：`live_identity_probe`（`tests/live_identity_probe.rs:122-124`）与 `live_samples_probe`（`tests/live_samples_probe.rs:134-136`）均为 `#[ignore]`，**本期门禁不得启用 `--ignored`**。
- **不启动 `mesad`、不启动 Vite、不连任何设备**（本文件自身的约束；本文件编写期间也未执行）。

**提交时必须在 PR 描述中给出的证据**：§0 表中 13 个文件的摘要（前后对比）、G1–G9 的命令与结果、62/62 的 `checked` 计数、`DIGESTS.lock` 的基线对比输出、以及 §5.1 中 #25–#34 的**逐条**通过证据。

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
- **落地断言**：测试 #22（`identity_mode` 不转发 + wire 与直接调用逐字节相等）与测试 #38（62/62 字节等价）共同保证「没有第二套实现」。
- **不复用的部分与理由**：见 §3.3 末表。

### 7.2 协议兼容

- `NativeNckAddress` 的 12 字段语义、`codec.rs` 的 10 字节布局、`client.rs` 的 ReadVar 会话、`value.rs` 的解码与无效值规则、以及 62 条 wire golden：**全部保持不变**。
- **不改 `proto/driver.proto`**，不改 `crates/driver-protocol/**`：`binding_kind`/`binding_config_json` 是字符串/JSON 透传，新增 `kind = "nck.points.v1"` **不需要**任何 wire 改动。
- **若确需改动**（当前评估为**不需要**）：必须作为**独立的「契约变更」PR**，并列出影响的 4 个其它驱动、需要同步改写的契约套件、`ProtocolManifest`/握手影响面、以及回滚方案。**PR-B 内部不得夹带 wire 改动。**

### 7.3 本期不做

- **前端与设备向导**：`apps/mesa-web/**` 全部（含 `nckPointModel.ts`、`NckPointFields.tsx`、`deviceModel.ts` 装配收敛、`browseModel.ts`、`ResourcePickerAntd.tsx`、`AddDeviceFlow.tsx`）属 **PR-C / PR-D**，PR-B 零改动。
- 用户导入定义、Core 变量知识库、Core 编译快照、新增 `FieldType`/`SchemaDescriptor`、新增 NCK area 字母（`M`/MMC/`S<n>` 一律不引入）、840D sl 定义、`cosmetic_keys` 等价规则、`nck_native` 服务端限权（仅沿用只读基线 + 前端警告口径，PR-C 落地）、历史 NCK 点位配置与历史数据迁移。

---

## 8. 与上位设计的冲突 / 需评审先裁决的事项

> 这些是**开工前**必须先有结论的事项；本文件给出建议裁决，但**不代替评审**。

| # | 冲突 | 证据（文件:行号 / 实测） | 建议裁决 | 级别 |
|---|---|---|---|---|
| **C1** | 「`target` 变体数 **= 1**」与 §2.5.1 表 A 的**双 target**定义**不可同时成立** | §2.4.1「恰好一个变体」、§2.4.2 D7「变体数 = 1」；但 §2.5.1 A#7/#8 需 `{channel, axis}`（`area_no=Channel`、`line=Axis`）、A#12–#14 需 `{channel, spindle}`、A#41 需 `{channel}`+`Argument("position")`；§4.1.1 C 的模板表本身就有 `@axis={axis},channel={channel}` 与 `@channel={channel},spindle={spindle}`；§2.5 的 `target_required: Vec<TargetKind>` 也是复数 | **采纳本文件 §3.2 的 D7 修正**（变体集合合法性：`S ⊇ target_required`、`S ⊆ targets`、`S ≠ ∅`），把「变体数 = 1」作废 | **P0（阻塞）** |
| **C2** | 「`presets/828d.json` 语义化」（§1.2、§7）与「`legacy_equivalence.rs` / `presets_828d.rs` 原样通过」（§1.3、§6.2）**不可同时成立** | `legacy_equivalence.rs:62-94` 把模板 `parameters` 当**原生字段**读并断言 `module`/`column`/`decode`/`transport_size`/`count`；`presets_828d.rs:15-26`、`:37-67`、`:210-256`、`:265-305` 读 `parameters.module`/`area`/`invalid_value`。模板 `parameters` 一旦改为语义 target/arguments，两个文件共 **8 个用例**必然失败 | **推荐方案 P1（模板逐字不动）**：`presets/828d.json` 与 `PresetTemplate.parameters` 保持原生字段不变；语义索引放入**定义目录**的 `golden_ref`；`presets.rs:151-186` 的**浏览叶 `binding_json`** 单独改为语义信封——这样 `legacy_equivalence.rs`/`presets_828d.rs`（8 用例）真·原样通过，且 §1.2 的「叶必须产出语义载荷」仍被满足。**备选 P2**（模板语义化）必须同时重写这两个文件，直接违反 §1.3/§6.2，并削弱 `legacy_equivalence.rs:12` 声明的「用冻结的旧数据做真值、不是用新代码自证新代码」 | **P0（阻塞）** |
| **C3** | `identity_mode` 位于 `parameters`（§2.3）与 `NativeNckAddress::from_parameters` 的 12 字段白名单**冲突** | `native.rs:147-160` 的 `ALLOWED` 恰 12 项、不含 `identity_mode`；`native.rs:230-235` 对未知键直接报 `UnknownField`；而 §2.6 契约 2 要求 Native 分支「与今天逐字相同」；`native.rs` 又被 §1.3 冻结 | **由 Compiler 剥离** `identity_mode` 后再调用 `from_parameters`：Compiler 侧 13 字段 `deny_unknown_fields` 结构 → 取出 12 字段 map → `from_parameters`。**`native.rs` 不动**，也不新增第二套解析。测试 #22 固定该行为 | **P0（阻塞）** |
| **C4** | §2.6 与 §2.7 的错误码清单**互不一致**，且缺 `MISSING_IDENTITY_KEY` | §2.6 表含 13 个 code（其中 `UNSUPPORTED_FAMILY`、`DEFINITION_DIGEST_MISMATCH` **未出现在** §2.7 的 Driver 行）；§5-7 说「新增 **7** 个 Driver 码」但 §2.7 的 Driver 新增行实际列了 **8** 个；§4.1.1 C 与 §6.2 要求 `MISSING_IDENTITY_KEY`，但 §2.6 的 `CompileError` 表**没有**它 | 以本文件 §3.5 为准（§2.6 的 13 个 + `MISSING_IDENTITY_KEY` + B 项 2 个 = **16 个** Driver code，其中 3 个复用原 code）；§5-7 的计数表述作废 | **P1** |
| **C5** | 定义目录的**存放形态**自相矛盾 | §1.1 写 `definitions/828d.json`（单文件）；§3.1 写「按 `revision` 分文件存放（`definitions/828d/r1.json`、`r2.json`…）」 | **采纳 §3.1 的分文件形态**（`definitions/828d/r1.json`）：§3.1 的不可变性与 T6 依赖「已发布 revision 的文件内容可逐字比对」，单文件形态下新增 revision 会改写同一文件、无法做「内容不变」判定。§1.1 的路径作废 | **P1** |
| **C6** | §6.2 要求「必须改写 `discovery_contract`（`:224-269`）浏览叶形态」，但该套件**没有**叶形态断言 | 实测 `discovery_contract.rs:255-260` 只断言根 2 节点 / `nck://presets` / `diagnostic`；`:270-273` 只断言未知 parent 空页；全文无 `binding_json` 断言 | **不重写既有断言**（它们不因叶形态变化而失败），**改为新增**一条叶形态断言（M8）——这样既落实 §6.2 的意图，又不制造无谓的契约 churn | **P2（缩小范围）** |
| **C7** | 「Driver **不**裁决身份唯一性」（§2.8 #10）与本文件 B-2「Driver 批内身份冲突自检」 | §2.8 #10 的 Driver 列写 `❌ 不裁决身份`；§4.1.2「Core 只负责唯一性」 | 请评审二选一：**(a)** 批准 B-2 作为**仅限 native 用户命名空间**的纵深防御（不影响 `mesa.resources.v1` 与语义点位，Core 规则不变）；**(b)** 拒绝 B-2，则只保留 B-1——此时 B 项仍成立，因为 B-1 是**结构性**隔离（语义身份恒含 `@` 且不以 `native.` 开头），不依赖事后判重。**无论选哪个，测试 #32/#34 都必须保留**；选 (b) 时 #33 改为 Core 侧 e2e 断言（`DUPLICATE_POINT_KEY`） | **P1** |
| **C8** | 新文件命名：`definition.rs`（§1.1/§7）vs `definitions.rs`（本轮任务书） | §1.1、§7 PR-B 范围、§7.0-4 均写 `definition.rs` | **采用 `definition.rs`（单数）**；`definitions/`（复数）**只**用于存放数据的目录 `drivers/sinumerik-nck/definitions/` | P2 |
| **C9** | `docs/adr/0005-nck-semantic-point-definition.md` 的**阶段归属**未定；PR-A 已合并但 ADR 0005 不存在 | §1.1 列为新增但未指派 PR；§5 说「所有条目均由 ADR 0005 统一留痕」；实测 `docs/adr/` 只有 0001–0004 | **PR-B 携带 ADR 0005**，并在其中**追溯** PR-A 已落地的 Core 侧契约（`point.rs`/`binding_gate.rs`/golden fixture）。若评审要求 PR-A 补，则需单独的文档 PR | **P1** |
| **C10** | Descriptor 的 `variable` 资源面：**删除** 还是 **降级保留** | §1.2「Descriptor 的 `variable` 资源 = 12 原生字段 → 重构」、§5-4「破坏性变更」、§6.2「`descriptor_contract` 必须改写」；但改写的具体形态未定。实测受影响断言只有 `descriptor_contract.rs:605` 与 `:729-731` | **方案 D-A（推荐，churn 最小）**：保留资源 id `variable` 与 12 字段（作为 `nck_native` 的**字段目录**），但 `resource_selection_methods` 收缩、并在 PR 描述中明确「该资源不再作为 `mesa.resources.v1` 采集入口」→ `descriptor_contract.rs:605`/`:729-731` 可**保持不变**。**方案 D-B**：删除 `variable` 资源 → 必须改写上述两处断言（§6.2 的口径）。**两方案的 `:600-604` 只读三条都不放宽** | **P1** |

**已核对、确认无冲突的点**（避免评审重复讨论）：
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
| 7 | 契约套件**用例级**总数 | 本次只核实**套件数 = 29**（`scripts/contract_suites.py:16-52`）；用例级数字（`GATE.md:61` 记 187/187 于 `d29c944`）**已过期**，PR-B 必须以实跑输出为准 |
| 8 | §4.1.1 长度上限的核心依据「42 条定义默认 Key 最长 71 字节」 | 该数字来自上位设计的推导（§4.1.1 长度上限）；PR-B 定义目录落地后必须**实测**最长 Key 并写进 PR 描述（Core 上限 256 字节**不放宽**） |

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

> 声明：本次**只新建** `docs/design/nck-prb-driver-compiler-boundary.md` 这一个文件；**未修改**任何代码、测试、配置或其它文档（尤其未改动已冻结的 `docs/design/nck-semantic-point-refactor.md`）；**未执行任何 git 写操作**（无 commit / push / checkout / stash / reset / PR / tag）；**未启动 `mesad`**、**未启动 Vite**、**未连接任何真实设备或机床**。
