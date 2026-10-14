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