# C1 连接字段契约与 Secret 安全恢复：实施与证据记录

- 依据：`docs/design/connection-setup-v1.md` 对应的 V1.1 评审修正稿（§6.2 四 Driver 契约、
  §6.3 Probe 分层预算、§9.4 Secret-safe、§10.1 最小后端变更清单、§11 A07/A21/A28/A29/A31/A32）
- 代码基线：本仓库 checkout 的 `HEAD`（见下方「执行 SHA」）
- 范围：**仅 C1**。C2（共享前端状态机）、C3（离线保存 / 设备归属 / connection-check）、
  C4（用户级自动重连）不在本次交付内。

## 1. 本轮冻结的契约

### 1.1 Host / URL 必须显式，无隐式默认目标

| Driver | 字段 | 变更 |
|---|---|---|
| `s7` | `host` | Descriptor 删除 `192.168.0.1` 默认值，改 placeholder；Parser 缺席/空/类型非法一律 `BAD_CONFIG` |
| `focas2` | `host` | 同上（原实现缺席回落 `127.0.0.1`） |
| `sinumerik-nck` | `host` | 同上（原实现缺席回落 `192.168.0.1`） |
| `opcua` | `endpoint_url` | Descriptor 删除 `opc.tcp://127.0.0.1:4840` 默认值，改 placeholder；Parser 缺席/空/类型非法一律 `BAD_CONFIG` |

理由：Descriptor 的默认值会被前端 `materializeSchemaDefaults` 回填成**有效连接目标**，
用户不填就会去连一个真实存在的地址（本机或出厂占位 IP）。「缺字段即拒绝」比
「缺字段就连别的东西」安全，且失败原因可诊断。

不变量：**已存储的、明确写了 Host/URL 的连接不受影响**（不迁移、不改写）。

### 1.2 「字段缺席」与「字段存在但非法」必须区分

四驱动统一：

1. `required` 字段缺席 → 拒绝；
2. 字段存在但类型不符（`null`、字符串数字、布尔、小数、负数、数组/对象）→ 拒绝，
   **不得**用 `.and_then(...).unwrap_or(...)` 把它当成缺席后回落默认值；
3. 数值先判范围再窄化（禁止 `port as u16` 之类截断：`70000` 不得变成 `4464`）；
4. Descriptor 的 `validation.min/max` 与 Driver Parser 使用**同一套**范围常量。

### 1.3 legacy 别名有明确边界

- `focas2`：`ip` = `host`，`timeout` = `timeout_ms`；
- `opcua`：`endpoint` / `url` / `host`+`port` 均可表达目标，`timeout` = `timeout_ms`。

规则：**主字段缺席**时才允许读别名；主字段存在但类型非法 → 直接拒绝（不退回别名）；
主字段与别名同时存在且取值冲突 → 拒绝并说明冲突（不做无说明的任选）；
取值一致 → 接受（不因重复书写而拒绝）。

### 1.4 `timeout_ms` 有显式上下限

新增 Core 级常量 `mesa_core_types::CONNECTION_TIMEOUT_MIN_MS = 500`、
`CONNECTION_TIMEOUT_MAX_MS = 10_000`（`crates/core-types/src/descriptor.rs`）。

- Descriptor 的 `validation.min/max` 与四个 Driver Parser 双重执行；
- 超限值在**访问网络之前**被拒绝（Core Schema 400 / Driver `BAD_CONFIG`），
  而不是被 Probe 外层预算静默截断；
- 取消「静默抬升到下限」的旧行为：低于下限即拒绝，避免「填 10、实际按 500 跑」。

NOTE：上限是**由阶段模型推出的工程上界**，不是真机实测结论；真机分阶段量测后应复核。

### 1.5 Secret 只有一条服务端构造路径（P0）

新增 `mesa_core_api::runtime::build_runtime_endpoint()`：

```
读 Endpoint → 读 Data Tasks → Data 门禁 → Secret 物化 → 读 Event Tasks → Event 门禁 → BuiltinEndpoint
```

三条入口全部改为调用它：

| 入口 | 改动 |
|---|---|
| `POST /endpoints/{id}/start` | 原内联实现删除，改调共享函数 |
| `POST /device-bootstrap`（`start=true`） | 原重复实现删除，改调共享函数 |
| `mesad` 开机恢复 | **新增** Descriptor 查询 + Secret 物化 + 任务门禁（此前直接传持久化 marker） |

修复的缺陷：

- 开机恢复把 `{"secret_set": true}` marker 当密码交给 Driver → 带认证的 OPC UA 连接
  重启后必然 `BAD_CONFIG`，而配置类失败是终态，端点永久 FAILED；
- `materialize_connection` 对坏 JSON 用 `unwrap_or(json!({}))`，把「配置损坏」
  静默变成「没有连接参数」，等于掩盖存储损坏并可能让 Driver 回落默认目标；
  现改为 fail-closed（非法 JSON / 非对象 → 明确错误）。

失败语义（保持既有 REST 口径）：Endpoint 不存在 404；配置非法 400；
Descriptor 不可用且配置含 marker 503；marker 缺对应 Secret → 明确错误，不匿名降级。

明文只存在于内存中的返回值，不落库、不回 HTTP、不进日志；物化后仍写回 marker 的
行为由既有 Update 逻辑保持不变。

### 1.6 Probe 分层预算（不再固定 12s 截断）

`crates/driver-manager/src/probe.rs` 的 `ProbeBudgets`：

```
startup  ≤ 6s      （DRIVER_STARTUP_TIMEOUT）
open     = min(15s, max(3s, 2×timeout_ms + 2s))
query    = min(15s, max(2s, timeout_ms + 2s))
close    = min(15s, 5s)
cleanup  ≤ 6s
total    = startup + open + query + close + cleanup      （上界约 44s）
```

- 通用 `Session::call()` 仍为 `REQUEST_TIMEOUT = 10s`（**不全局拉长**）；
- Probe 专用 `Session::call_with_timeout()` / `probe_with_timeout()` 使用上述分阶段预算，
  上限 `PROBE_RPC_TIMEOUT = 15s`；
- `ProbeError::Timeout` 现在携带本次预算（`budget_ms`），超时可对账而不是黑盒；
- 外层总预算 ≥ 内层各阶段之和，不再出现「外层抢先截断内层」。

## 2. 改动文件

| 文件 | 内容 |
|---|---|
| `crates/core-types/src/descriptor.rs`、`lib.rs` | 新增并导出 `CONNECTION_TIMEOUT_MIN_MS/MAX_MS` |
| `crates/core-api/src/runtime.rs`（新增） | 共享运行期 Endpoint 构造 + Secret 物化 + 单测 |
| `crates/core-api/src/lib.rs` | `materialize_connection` fail-closed；`start_endpoint`/`browse` 改走共享路径；`ProbeError::Timeout { .. }` 映射 |
| `crates/core-api/src/bootstrap.rs` | start side effect 改调共享路径 |
| `apps/mesad/src/main.rs` | 开机恢复改调共享路径；列表读取失败不再伪装成空库 |
| `crates/driver-manager/src/session.rs` | `call_with_timeout` / `probe_with_timeout` / `PROBE_RPC_TIMEOUT`；删除固定 `PROBE_TIMEOUT` |
| `crates/driver-manager/src/probe.rs` | `ProbeBudgets` 分阶段预算；`ProbeError::Timeout { budget_ms }` |
| `drivers/s7/src/{client.rs,lib.rs}` | Host 必填、类型/范围严格、timeout 上下限、Descriptor 无默认 Host |
| `drivers/focas2/src/lib.rs` | 同上 + legacy 别名冲突规则 + 端口先判范围 |
| `drivers/opcua/src/lib.rs` | URL 必填、别名冲突、`use_native` 严格、Secret marker 明确诊断、timeout 上下限 |
| `drivers/sinumerik-nck/src/{config.rs,lib.rs}` | Host 必填、类型严格、timeout 上下限、Descriptor 无默认 Host |

## 3. 测试证据

见本文末「最终提交与干净工作树证据」一节——那里记录的是**最终提交 SHA 上
干净工作树**重跑的结果，替换了早先在 `5d4ce284 + dirty` 工作树上取得的数字。

命令（本地 Windows / rust 1.95.0）：

```text
cargo fmt --all -- --check                                          通过
cargo clippy --workspace --all-targets --all-features -- -D warnings 通过
cargo build --workspace                                            通过
cargo test -p mesa-core-api -p mesa-driver-s7 -p mesa-driver-focas2 \
           -p mesa-driver-opcua -p mesa-driver-sinumerik-nck \
           -p mesa-driver-manager -p mesa-config-store             全部通过（无 FAILED）
scripts/write-contract-evidence.py                                 见末节
```

契约基线由 **29** 套件组成（本轮新增 `opcua_auth_restart_e2e`，并同步登记到
`scripts/contract_suites.py`；该脚本对磁盘文件与名单做双向核对，
新增套件不登记会让证据产出直接失败）。

**因契约变更而同步修正的既有契约用例**：`tests/driver-contract/tests/resource_contract.rs`
中直接调用真实 Driver 的 6 处 `open_connection("ep1", "{}")` 改为显式给出连接目标
（focas2 补 `host`、opcua 补 `endpoint_url`）。这些用例的断言对象是
**ResourceSelection 契约**，不是连接解析；给出显式目标既保持其原意，
又与新契约一致（旧写法现在必须被拒绝）。未放宽任何断言。
另把 `subprocess_orphan_guard::manager_probe_timeout_cleans_hang_child` 改为显式
`timeout_ms=500`，避免它在新的阶段预算下白等默认长度。

新增的关键回归（对应验收编号）：

| 验收 | 测试 |
|---|---|
| A07 / A28 | `s7::client::tests::{host_is_required_and_never_falls_back_to_default_target, numeric_fields_reject_ill_typed_and_out_of_range}` |
| A07 / A28 | `focas2::tests::{host_is_required_..., legacy_aliases_do_not_mask_ill_typed_primary_fields, port_range_is_checked_before_narrowing, timeout_bounds_are_enforced_not_clamped}` |
| A07 / A28 | `opcua::tests::{endpoint_url_is_explicit_and_never_defaults_to_loopback, conflicting_target_fields_are_rejected, host_port_form_validates_port_before_narrowing, ill_typed_credential_and_use_native_are_rejected}` |
| A29 | `sinumerik_nck::config::tests::{host_must_be_explicit_and_well_typed, timeout_floor_and_pdu_range}` |
| A28 | 四驱动 `descriptor_timeout_bounds_match_parser_contract` / `timeout_bounds_match_descriptor`（Descriptor 与 Parser 范围同源） |
| A31 / P0 | `core_api::runtime::tests::{missing_endpoint_is_not_found, secret_marker_without_descriptor_is_fail_closed, plain_connection_survives_descriptor_unavailable, malformed_connection_json_is_rejected, marker_is_materialized_in_memory_only, store_rejects_malformed_connection_json_at_write_time}` |
| A17 / P0 | `opcua::tests::secret_marker_reaching_driver_is_fail_closed_with_diagnosis`（marker 到 Driver 必须明确失败，不匿名降级） |
| **A21** | `opcua_auth_restart_e2e::authenticated_opcua_endpoint_recovers_secret_and_new_data_after_restart`（**进程级**：要求 UserNameIdentityToken 的 fixture → 明文口令入库为 marker → 采到 GOOD/CURRENT → 非优雅 kill mesad → 同库重启 → 断言 RUNNING + `epoch` 增长 + `point_id` 不变 + **`timestamp_ns` 晚于重启前**；并含错误口令负例与「不得回显明文」断言） |
| **A22**（可控模拟） | `subprocess_orphan_guard::slow_but_successful_probe_is_not_truncated_by_management_budget`（`probe()` 内 11s > 旧 10s IPC / 12s 外层，必须等满并返回可达） |
| **A22**（预算模型） | `driver_manager::probe::tests::{query_budget_carries_the_protocol_handshake_not_open, max_supported_timeout_is_fully_covered_by_query_budget, slow_but_successful_multi_phase_probe_fits_the_budget, probe_budget_tracks_configured_timeout_and_stays_bounded}` |
| **A32** | `subprocess_orphan_guard::cancelled_probe_releases_child_process_and_port_lease`（abort 整个 probe future 后：PID 回落基线 + 端口租约文件删除 + 该端口不再被监听 + 随后 Probe 成功） |
| A22（IPC 层） | `driver_manager::session::tests::{call_with_timeout_is_bounded_and_cleans_pending, generic_call_timeout_is_not_widened_for_probe}` |
| **A31**（密钥不可用） | `config_store::tests::secret_decrypt_failure_is_fail_closed_and_does_not_leak`（密钥被替换 / 损坏 → 读取必须 Err，且错误信息不含明文） |
| 慢设备注入 | `test_driver` 新增 `probe_delay_ms`（connection 顶层可选整数），供 A22/A32 使用 |

## 4. 明确**未**验证的部分（不得当作 PASS）

1. **A22 真机分阶段量测未做**：本轮实现的是「按阶段数与 `timeout_ms` 推导的有界预算」，
   并在受控模拟下证明「慢但成功的探测不被截断」；但各阶段**真实**耗时分布、
   以及 `CONNECTION_TIMEOUT_MAX_MS = 10_000` 与 `PROBE_RPC_TIMEOUT = 45s` 是否合适，
   仍必须在授权实体设备上分阶段量测后复核。**当前值是工程上界，不是实测结论。**
2. **A21 的负例只证明「错误凭据取不到数据」**，不声称错误码已精确映射：
   实测错误口令端点的 detail 是 `[Timeout/Connect] 等待连接超时 (status=None)`，
   而非预期的 `BadIdentityTokenRejected`。OPC UA 认证失败的 status 未透传到
   诊断串，属独立的可诊断性缺口，未在本轮改动。
3. **A32 的 HTTP 层断开未端到端复现**：本用例以 `abort()` 整个 probe future 来
   等价「客户端断开导致服务端请求被取消」（清理代码没有机会跑完，正是真实形态）；
   但「真实 HTTP 连接断开 → hyper 取消 handler future」这条链路未单独构造用例。
4. **未做 C2/C3**：前端 Host 输入体验、TSAP 未失焦提交、错误响应分类、离线保存、
   设备归属门、`connection-check` 均未实施；因此本轮的「四驱动参数可靠」只在
   API/Driver 层成立，端到端 UI 断言（A01–A06、A11、A26、A27、A30）不适用。

## 5. 升级与运维须知

- **存量审计（部署前执行，不由代码自动改写）**：经 REST 读取全部 Endpoint，
  检查其 `connection` 是否显式包含 `host`（`opcua` 为 `endpoint_url`）。
  缺失者在新版本下会在 Configure/Start 阶段被明确拒绝（`BAD_CONFIG`），
  需要人工补全真实目标后重启该 Endpoint；**不得**自动填充猜测值。
  按仓库约定，该审计经 REST 完成，不直接读写 SQLite。
- **`timeout_ms` 超出 `[500, 10000]` 的存量配置**同样会在新版本被拒绝，
  需要人工调整到范围内；调整前应先确认设备侧真实响应时间。
- 旧配置中的 `host` 若为字符串但指向出厂占位地址（如 `192.168.0.1`），
  新版本**不会**改写它，仍会按原值连接——这是有意的：不替用户改连接目标。
- **`master.key` 必须与数据库一起备份**（第 7 节）。密钥文件缺失而库中已有
  加密 Secret 时，新版本会**明确拒绝**而不是自动生成新密钥；此时唯一正确的
  动作是从备份恢复原 `master.key`，而不是让程序"重新开始"。
  由于 `.gitignore` 忽略 `**/master.key`，`git clean` / 重新克隆会删掉它——
  请把密钥文件纳入独立的密钥备份流程，不要只依赖工作区副本。

## 6. 最终提交与干净工作树证据

本分支的提交序列（`main` 未改动）：

| 提交 | 内容 |
|---|---|
| `b714a25` | 四驱动连接契约严格化 + Secret 统一构造路径 + Probe 分层超时（首轮） |
| `c41cd8f` | 补齐 A21/A22/A31/A32 验收 + **修正 Probe 阶段预算方向**（`opcua_auth_restart_e2e` 新套件） |
| `e818caf` | 忽略 `__pycache__`：证据脚本自身 import `contract_suites.py` 会弄脏工作树，导致官方证据恒为 `dirty=true`；不修则无法产出干净工作树证据 |
| `61aed30` | 修正 A21 的 epoch 断言（见下） |
| `b58be17` | 记录 `61aed30` 的干净工作树证据 |
| `43ccff4` | **`master.key` 不可逆覆盖风险修复**（第 7 节，C1 合并前 P0） |
| `6458c99` | A22 只读量测方案（`docs/design/a22-probe-timing-measurement.md`） |
| `6644477` | 记录 `6458c99` 的干净工作树证据 |
| `4b02b4f` | **密钥并发首次初始化防覆盖**（唯一临时文件 + `hard_link` 原子发布 + 跨进程测试，第 7 节补充） |
| `cea5dc5` | **A32 的 Linux CI 修复**：`assert_no_orphan` 改异步轮询 + Linux 进程状态诊断（原同步 `std::thread::sleep` 阻塞 Tokio 运行时，妨碍子进程异步回收；该改动后 Linux x64 与 ARM64 均通过） |
| `d5a95f2` | **A22 分阶段单调计时**（`StageTimer` RAII Drop 输出 5 阶段 + 捕获式回归测试；纯观测，协议与超时语义零改动） |
| `37938fd` | 同步 `Cargo.lock`（`d5a95f2` 为 s7-transport 加了 dev-dependency；`--locked` 构建必需）。**并更正 `d5a95f2` 提交信息中「不改 Cargo.lock」的错误陈述** |

### 6.1 被测试提交 SHA 与文档提交 SHA 的区分（重要）

本文档记录的证据对应**被测试提交** `37938fd`；而更新本文档本身会产生一个新提交，
两者**不是同一个 SHA**。禁止把被测试 SHA 冒充为文档提交后 HEAD 的测试结果。

- **被测试提交 SHA**：`37938fda854e6ef1b1fc766a9e6da223ab690cb9`
- **证据文档提交 SHA**：本文档更新后新产生的提交（见 `git log`）
- 约定：文档提交后**不再反复改文档制造 SHA 循环**；最终 HEAD 的机器证据
  以「在干净工作树上再跑一次脚本」的结果记录到 PR #103 的验收评论 / CI 证据中。

### 6.2 干净工作树证据（被测试提交 `37938fd`）

```text
scripts/write-contract-evidence.py
  → target/validation/contract.json
    suites=29  tests=190  sha=37938fda854e6ef1b1fc766a9e6da223ab690cb9  dirty=False
```

全部契约套件 PASS、无失败测试；`dirty=False` 表示该结果取自干净工作树。

**独立单测证据**（不混入契约套件的 190 项计数）：

```text
cargo test --locked -p mesa-s7-transport    57 passed   （含 A22 五阶段计时回归）
cargo test --locked -p mesa-config-store    31 passed   （含 master.key 保护与并发防覆盖）
```

在此之前同一命令在 `c41cd8f` 上曾两次出现**单套件偶发失败**（分别落在
`event_pressure` 与 `opcua_auth_restart_e2e`），逐项定位后确认：

- `opcua_auth_restart_e2e` 的失败是**本测试自身的缺陷**——`epoch2 > epoch1` 断言无效，
  因为 `new_stream_epoch()` 是 `now_unix_ns ^ (pid<<32) ^ (counter*黄金比)` 的 XOR
  混合值，**不是单调序列**；重启后进程 pid 变化会让大小关系随机。已改为
  `assert_ne!`，并用 4 实例并行复现验证（修正前 4/4 失败 → 修正后 4/4 通过）。
- `event_pressure` 单独重跑与序列重跑均通过，未能复现，按环境偶发处理。

**尚未完成的前置检查**：本证据只覆盖仓库内可自动化部分。A22 的**实体设备**
分阶段量测、以及第 4 节列出的可诊断性缺口，均不包含在上述 190 项之内。

> **门禁归属已于 2026-10-15 调整**：A22 不再是**代码合并**阻断项，改为
> **硬件支持与生产发布**阻断项。这是对既有验收约定的显式变更，**不是跳过**——
> 详见 **§8**（含变更理由、仍然独立的边界、以及"未验证"状态的具体含义）。

## 7. `master.key` 不可逆覆盖风险修复（C1 合并前 P0）

**原缺陷**：`load_or_create_master_key_file()` 对「已存在但内容非法」的
`master.key` **不报错**，而是继续走生成流程——在多数文件系统上会**替换掉**
原文件。后果不是"读不出来"，而是**原始密钥材料被销毁**：备份或人工修复
恢复原 Secret 的唯一线索被覆盖。读取路径本身是 fail-closed 的（A31 断言
成立），但"拒绝解密"不等于"保护了密钥材料"。

**修复后的规则**（`crates/config-store/src/lib.rs`）：

| 情形 | 行为 |
|---|---|
| 文件存在且合法（32 字节原始 / base64 32 字节） | 使用它；**不改写文件** |
| 文件存在但内容非法 | **报错**，不生成、不覆盖、不轮换；文件字节保持原样 |
| 文件缺失，且库中**已有**加密 Secret | **报错**，不自动生成（新密钥解不开旧密文） |
| 文件缺失，且库中**没有**加密 Secret（全新存储） | 生成新密钥（原行为） |
| `MESA_MASTER_KEY` 环境变量已设 | 按原语义使用（显式覆盖，不涉及文件） |
| in-memory 库 | 固定测试 key，不碰磁盘（原行为） |

**实现要点**：判定「库中是否已有加密 Secret」需要查询 `endpoint_secrets`，
但 `get_secret()` 原本在**持有 `conn` 锁**时调用 `master_key_bytes()`，
而 `std::sync::Mutex` 不可重入——在 `master_key_bytes()` 内部查询会自锁。
因此改为由调用方传入 `db_has_secrets`：写路径先 `count_secrets()`（短锁查询
后立即释放）再取 key；`get_secret()` 改为**先短锁取出密文行、释放锁后**
再取 key（此时已知存在 Secret 行，直接传 `true`）。

**回归测试**（`crates/config-store`）：

- `secret_decrypt_failure_is_fail_closed_and_does_not_leak`（已更新）：
  非法密钥 → 报错且错误信息指向 `master.key`，并**断言文件字节未被修改**；
- `missing_master_key_with_existing_secrets_refuses_to_generate`（新增）：
  删除密钥文件后读/写路径都必须拒绝，且**不得创建** `master.key`；
- `fresh_store_without_secrets_still_creates_master_key`（新增）：
  反向保护——全新存储仍能正常创建密钥并完成加解密；
- `base64_master_key_is_still_accepted`（新增）：
  合法 base64 形式仍被接受且**文件不被改写**。

**运维影响（重要）**：本修复把"静默换钥匙"变成"明确拒绝"。若某台机器的
数据库已有加密 Secret 而 `master.key` 丢失，升级后相关端点会**明确失败**
（而不是用一个新密钥继续跑、让所有旧凭据永久无法解密）。此时唯一正确的
动作是从备份恢复原 `master.key`。

### 7.1 补充：跨进程并发首次初始化的防覆盖（`4b02b4f`）

7 节的修复只堵住了"文件已存在但非法"的路径；**并发首次初始化**仍有独立窗口：
原实现用固定名 `master.tmp` + `rename`，而 POSIX `rename` 会**替换**已存在目标，
两个 mesad 进程同时首次初始化同一密钥目录时可能互相截断、甚至后写者覆盖先写者，
造成「内存 key ≠ 落盘 key」。

现改为：**唯一临时文件**（`pid` + 单调序号，`create_new` 不复用残留）→ `fsync`
→ **`hard_link` 原子发布**。`hard_link` 在目标已存在时必然失败，因此不存在覆盖
窗口；链接可见的一刻内容已完整（临时文件已 fsync 并关闭），读取方不可能看到
半截密钥。若发现目标已由其它进程创建，则**读取并校验对方那把**密钥，而不是把
自己这把写进去——两个进程最终必须得到同一把密钥。

回归测试 `concurrent_first_init_across_two_processes_keeps_one_key`：父进程以
`--exact` 重新执行本测试二进制两次（子进程经环境变量走"只做初始化"的分支），
断言两进程密钥完全相同、落盘与返回一致、无 `.tmp` 残留、且该密钥可正常加解密。
**必须真起进程**：进程内的 `INIT_LOCK` 只串行化同进程调用，无法覆盖该场景。

## 8. A22 门禁归属调整（2026-10-15，**显式的验收约定变更**）

### 8.1 变更内容

| 门禁 | 调整前 | 调整后 |
|---|---|---|
| A22（828D 真机 Probe 分阶段耗时量测） | **代码合并阻断项**（PR #103 不得合并） | **硬件支持与生产发布阻断项**；**不再阻断代码合并** |

三层门禁自此分离，**互不代替**：

1. **软件合并门禁**：实现 + 契约测试 + 远程 CI（含 `ci-gate`）全部通过 → C1/C2/C3 可合并；
2. **硬件验收门禁**：A22 真机阶段量测**继续 HOLD**，未取得现场授权窗口前不执行；
3. **生产发布门禁**：真实设备验证未完成，**不发布正式生产版本**（不创建 Release Tag）。

### 8.2 变更理由

- A22 要验证的是**特定设备上的实际通信耗时分布**，用来复核 Probe 分层预算这一
  **工程上界**；它**不是**"当前代码存在一个已知且未修复的 P0 缺陷"。C1 的
  `master.key` 覆盖/并发问题属于后者，**已修复**（§7、§7.1），因此不构成遗留风险。
- 让三个**软件验收已完成且远程 CI 全绿**的 PR 无限期滞留 Draft，会持续推高
  后续分支的同步成本，对主线不利。

### 8.3 明确**不因本次调整而改变**的事实

- **A22 未通过**，也**未执行**。本次调整**不产生任何真机结论**。
- `CONNECTION_TIMEOUT_MAX_MS = 10000` 与 `PROBE_RPC_TIMEOUT = 45s` 仍是
  **由阶段模型推导的工程上界，不是实测结论**，未获真机确认。
- §4 列出的可诊断性缺口（OPC UA 认证失败 status 未透传、真实 HTTP 断开链路未单独用例化）
  **仍然存在**。
- A32 的原始 Zombie 状态**未被捕获**——严格结论只能是"同步阻塞轮询是导致 Linux
  失败的高度可信原因，异步轮询已通过跨平台验证"。

### 8.4 合并后仍然互相独立的边界

C1/C2/C3 合并进 `main` **仅意味着**「连接管理的软件功能已进入主线」，**不意味着**：

- 828D / 840D sl 已全面通过真机兼容性验证；
- C1 的 A22 已通过；
- NCK 全部参数及 SINAMICS 驱动参数已完整覆盖；
- FANUC Load 已通过实体设备生产验收；
- Mesa 可以直接宣布生产发布就绪。

以上状态必须**继续保持独立**，不得因合并而合并陈述。

### 8.5 依据的覆盖矩阵结论（同期只读审计）

2026-10-15 的 NCK/FANUC 只读覆盖审计指出：**全类别均未达 `HMI 语义确认` 档**；
NCK 存在"**GOOD 但标签/归属可能错误**"的已发生实例（240 个遗留点位中，`MX*` 指向
`V/3`、`MZ*` 指向 `V/4`，而实机轴名表为 1=MX1/2=MY1/3=MZ1/4=MSP1，二者不可能同时
正确）；FANUC **无任何实机证据**（唯一连过的目标是 NCGuide 虚拟机，`GATE.md` 自己
声明其不能替代真机验收）。**这些结论与本次门禁调整无关**，恰恰说明硬件门禁必须
保持独立、不得因软件合并而下调。