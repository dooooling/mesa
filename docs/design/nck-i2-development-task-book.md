# I-2 后端开发任务书（可执行工作分解 · 文件边界 · 验收对应 · 阻塞条件）

- 状态：**可开工任务书**。它是**执行索引**，不是设计文档：凡设计已定之事，本文件**只引用章节号 + 验收编号 + 文件范围**，不复制正文。
- 基线：分支 `feat/nck-i2-backend` / HEAD `ff56855`（= `main`，PR #113 已合并）；工作树干净。
- 性质声明：本文件**只新建自身**；未改任何代码 / 测试 / 配置 / 其它文档；**未执行任何 git 写操作**；未启动 `mesad` / Vite；未连接任何设备；未做任何安装。
- 规模口径（评审确认 1）：**以准确覆盖工作范围为优先**；`250–400 行` 仅为**建议规模**。本文件**共 384 行**（在建议区间内），构成与增量理由见 **§7**。
- 阅读顺序建议：三项 P0（§1）→ 开工前提（§5.1）→ 阶段 1–3（§2）→ 对应表与缺口（§4）→ §11 处置表（§3）→ 基线（§6）。

## 0. 引用表与简写（下文只用简写，不再重复全路径）

| 简写 | 指向 |
|---|---|
| **DC §n** | `docs/design/nck-i2-definition-catalog-api-boundary.md`（1802 行，已合并）第 n 节 = **唯一设计权威** |
| **PR-B** | `docs/design/nck-prb-driver-compiler-boundary.md`（1167 行，已裁决） |
| **PR-C** | `docs/design/nck-prc-frontend-semantic-config-boundary.md`（1362 行，已合并且已裁决） |
| **I1–I45** | DC §8.3 验收矩阵（**本文件不新增、不改号**） |
| **E1–E31** | DC §6.1 错误码表（**本文件不新增、不改号**） |
| **G0–G11** | DC §8.4 CI 门禁 |
| **R1–R11 / V1–V6** | DC §6.3 只读性证据 / DC §7.7 版本兼容矩阵 |
| **D1–D11** | PR-B §3.4、§3.6 的参数层校验与身份一致性编号（D11 = 身份一致性） |
| **P9** | `crates/driver-contract/tests/resource_contract.rs:396-461` 的 Core 零 Siemens 语义护栏（实测行号见 §4.4） |

**本任务书的定位（三句话）**：① 设计内容**一律引用 DC**（尤其 §3 接口入口、§4 定义目录、§5 权威 Preview、§6 错误与安全、§6.4 执行边界、§7 兼容性、§8 验收、§9 `scope`、§11 遗留、§14 差异表）；② 本文补的是 DC 未给的**工作分解、阶段顺序、文件↔验收对应、开工前提**；③ DC 与本文冲突时，**以 DC 为准**（唯一例外见 §1 的三项 P0 裁决与 §3 的 §11 处置）。

---

## 1. 三项 P0（评审指定；未闭环则不得进入对应阶段）

### 1.1 P0-1 Manager 并发预算（**阻塞阶段 2**）

**冻结口径（不得实现成别的样子）**：

| 项 | 口径 | DC 落点 |
|---|---|---|
| 预算值 | 每 `driver_id` 最多 **4** 个并发 I-2 管理请求（in-flight 临时进程数） | §6.4-①-⑤ |
| 超限行为 | **立即拒绝、不排队、不等待** ⇒ 503 `DEFINITION_QUERY_BUSY`（E31，`ErrorKind::Internal`） | §6.1.3、§6.2.1 |
| 计数与许可释放位置 | **Manager**（`crates/driver-manager/src/manager.rs`）⇒ DC §11-13 的「载体未定」据此**冻结为 Manager** | §11-13、§8.1.2-M5 |
| `driver-protocol` 职责 | **只负责版本与消息契约**（`PROTOCOL_MINOR`、门控常量）；**不承担运行时并发计数**。M11 若落地，**只能是纯门控函数**（形态同 `probe.rs:147-151` 的 `probe_supported`），**不得持有计数器或 in-flight 状态** | §8.1.2-M11、§6.4-② |
| 四条路径 | 许可释放必须覆盖**成功 / 失败 / 超时 / 取消**，走单一出口（RAII guard / `Drop`），与既有 `temp_op.rs:40-43` 的「单出口回收」同构 | §6.4-②、DC §2.2 |
| PR-C 侧约束 | **PR-C 不得高频轮询 Preview**：仅在参数（Target/Arguments）稳定后请求一次，并**取消过期请求**（对应「取消路径」必须释放许可） | PR-C §1.5（`:351` 的合法路径 (a)） |

**前提事实（本次独立只读复核，非引用 DC）**：

| 复核项 | 命令 / 范围 | 实测结果 |
|---|---|---|
| 是否存在并发闸门 | `Select-String 'Semaphore\|acquire(\|rate_limit\|RateLimit\|concurrency'` over `crates/**/*.rs` + `apps/mesad/**/*.rs` | **零命中**（无输出） |
| 全仓扩大范围 | 同模式 over `crates, apps, drivers, tests, scripts`（`*.rs/*.py/*.ts/*.toml`） | 仅 `apps/mesa-web/node_modules/**`（第三方 `std-env`/`eslint` 的**环境探测枚举值**与类型声明）与 `tests/driver-contract/tests/management_api.rs:530` 的**测试函数名** `control_running_e2e_broadcast_isolation_and_write_without_acquire` ⇒ **均非并发闸门** |
| `in-flight` 词面 | 同范围 | 命中**均为注释**：`crates/driver-manager/src/endpoint.rs:586`、`:613`、`:618`（drain 顺序契约）、`crates/opcua-transport/src/native.rs:1198`、`crates/event-store/src/writer.rs:112`、`:141`、`:203` ⇒ **无任何运行时计数实现** |
| 超时层是否顺带提供闸门 | `crates/core-api/src/lib.rs`、`crates/driver-manager/src/*.rs` 对 `TimeoutLayer`/`Semaphore` | **零命中** |

⇒ **复核结论：仓库确实没有可复用的并发闸门，P0-1 是必须新增的唯一资源控制机制**（与 DC §6.4-② 末行「如实登记为新增项，不冒充复用」一致）。

**可执行验收（I39 扩充为 5 条，全部必须过）**：

| # | 场景 | 操作 | 期望观测 | 失败判据 |
|---|---|---|---|---|
| A1 | 立即拒绝 | 对同一 `driver_id` 并发发起 **5** 个定义查询（预算 4） | 第 5 个**立即**返回 503 `DEFINITION_QUERY_BUSY`；消息含当前 in-flight 数 + 预算 | 第 5 个被排队/等待；或错误码不是 E31 |
| A2 | 无无限等待 | 同上，测第 5 个请求的墙钟耗时 | 耗时 ≪ 5s（RPC 预算），不出现「等到超时」 | 耗时接近/超过 5s（等于变成了排队） |
| A3 | 无许可泄漏 | 成功 / 失败（非法参数）/ 超时（注入不响应）/ 取消（客户端中途断开）四条路径各跑 ≥ 4 轮后 | in-flight 计数**回落到 0**；随后 4 个并发请求**仍全部被接受** | 任一路径后出现「永久 503」（= guard 未释放） |
| A4 | 跨 driver 隔离 | 驱动 A 占满 4 个 in-flight 时对驱动 B 发 1 个请求 | B **正常返回**（预算是 per-driver 的） | B 被 A 的预算拖累 |
| A5 | 不影响正常采集 | 某 Endpoint 已 Start 并在采集；同时占满 4 个定义查询 | 采集**不中断**、无 `stream_epoch` 变化、`GET /api/v1/tasks` 不变、`points/latest` 持续更新 | 采集停摆 / 任务被改动 / 采样丢失 |

**阻塞条件**：并发预算载体必须先冻结在 Manager（本表已冻结）；若实现者把计数器放进 `driver-protocol`，判定为**违反 P0-1**，停止并上报。

### 1.2 P0-2 Native Preview 边界（**阻塞阶段 3**；逐行照录评审裁决）

| Point Kind / 模式 | 行为 |
|---|---|
| `nck_variable` | 服务端**重算语义身份** |
| `nck_native` + `derived` | 按 **Driver 固定规则**生成身份 |
| `nck_native` + `user` | **不重算；作为身份生成请求予以拒绝** |
| 未知 Point Kind | **明确拒绝** |

**两条硬约束**：① **不得改动 Native 自定义 Key 的既有行为**（`identity_mode="user"` 的 Key 由用户显式指定，Driver 不重算、不静默改名；PR-C §1.5 `:352` 与 DC §2.5-B3 已冻结）；② **不得自动追加 `~<n>` 后缀**（本阶段不实现自动生成，冲突时拒绝并要求用户自行改名）。

**实现与验收落点**：`compiler.rs:462-481`（取值校验）、`:485-488`（user 分支）、`:512-521`（命名空间复核）**逐字不动**；`derived` 固定规则见 `definition.rs:1163-1176`。

| 裁决行 | 验收编号 | 备注 |
|---|---|---|
| `nck_variable` 重算 | I11、I12、I32 | — |
| `nck_native` + `derived` | I15、I14 | 断言 Key 不含 `count`/`decode`/`transport_size`/`invalid_*`/`identity_mode` |
| `nck_native` + `user` 拒绝 | I16（400 `INVALID_BINDING_CONFIG`）、E18 | §11-5 的「严格最小」解释不采纳 |
| **未知 Point Kind 明确拒绝** | **无 I 行**（E16 `UNSUPPORTED_POINT_KIND` 仅在 DC `:924`/`:982` 定义，矩阵无断言） | **真实缺口**：阶段 3 必须补一条断言（见 §4.2 缺口 G-a） |

**阻塞条件**：DC §11-5 仍标「**仍待评审确认**」（可能被解释为「严格最小 ⇒ 只接受 `nck_variable`」）。本任务书采用评审的 P0-2 裁决（支持 `nck_native`+`derived`）；**开工前须把该裁决回写 DC 并冻结**，否则预览的接受/拒绝面不确定。回写需修改设计文档 ⇒ 由**上级执行**，本任务书无权改。

### 1.3 P0-3 真实超时与取消验证（**阻塞阶段 2 的完成判据**）

**三条口径**：① **22 秒只是分项预算的算术估计**（DC §6.4-①-④：启动 6s + RPC 5s，attempt ≤ 2），**不得作为对外 API 响应时间保证**；② 实际可能**先命中外层 10 秒超时**；③ 开发阶段必须用 **I38 验证完整调用链的真实超时与取消行为**，据此确定**统一的对外超时上限**，并**避免后台尝试在 HTTP 请求结束后继续占用临时进程**。

**本次只读复核（明确一处待确认）**：`crates/core-api/src/lib.rs` 的 router（`:3236` `Router::new()` → `:3341` `.with_state(state)`）**没有** `TimeoutLayer`、**没有** `DefaultBodyLimit`（复核命令见 §6）；仓库内与 I-2 相关的**唯一 10s** 是 `crates/driver-manager/src/session.rs:25` 的 `REQUEST_TIMEOUT = Duration::from_secs(10)`，它被通用路径 `call()`（`session.rs:892-895`）使用。⇒ 「外层 10 秒超时」的**确切载体待确认**：最可能是「若新 RPC 方法照抄通用 `call()` 而非照抄 descriptor 形态（`session.rs:654-690`，5s @ `:667`），则 10s 会先命中」。**该项由 I38 的实测判定，不得在设计里假设**。

**可执行验证任务（I38 扩充；命令 / 场景 → 期望观测 → 失败判据）**：

| # | 场景（注入方式） | 期望观测 | 失败判据 |
|---|---|---|---|
| T1 | 驱动**握手超时**（用可挂起的测试驱动 / test-driver，不发 `Welcome`） | 503 `DRIVER_UNAVAILABLE`（E2）；实际耗时 ≤ `DRIVER_STARTUP_TIMEOUT = 6s`（`session.rs:280`） | 超过 6s；或码不是 E2 |
| T2 | RPC **不响应**（驱动不发 `DefinitionsReport`） | 503 `DEFINITION_QUERY_TIMEOUT`（E3）；实际耗时 ≤ 5s（`session.rs:667` 形态） | 耗时命中 10s（`REQUEST_TIMEOUT`）⇒ 说明用错了形态，**阻塞** |
| T3 | attempt 计数 | 断言 attempt ≤ `MAX_TEMP_ATTEMPTS = 2`（`temp_op.rs:25`） | 出现 > 2 次 spawn |
| T4 | 全局超时未被调大 | `REQUEST_TIMEOUT` 仍 = 10s（`session.rs:25`） | 被调大 |
| T5 | **确定统一对外上限** | 以 T1–T4 实测值给出**对外 API 响应时间上限**（≤ 实测最坏 + 余量），写入实现 PR 的验收结论 | 直接把 22s 写进对外文档/前端超时 |
| T6 | **取消路径**（客户端在请求发出后立刻断开，如 curl `--max-time 1` / 测试中 drop handler future） | 必须复用下方 **R-1…R-4** 四条断言（同一组观测点），**不得只验证「连接关闭」本身** | R-1…R-4 任一不成立 ⇒ **阻塞** |

**超时 / 取消后的资源回收 —— 四条必须全部成立（评审要求；各给观测方式与失败判据）**：

| # | 断言 | 观测方式（可执行、可观测） | 失败判据 |
|---|---|---|---|
| **R-1** | **HTTP 层按预期超时返回**（不是无限等待） | 注入「RPC 不响应」驱动，记录墙钟耗时 + HTTP 状态码 + body 的 `code`/`kind` | 请求悬挂不返回；或耗时 > 已确定的对外上限；或状态码/码与 E2/E3 不符 |
| **R-2** | **Manager 不再持有该请求的并发许可**（许可计数回到基线） | ① 直接读 P0-1 的 per-driver in-flight 计数器（测试内可观测点，**不新增对外接口**）；② 间接断言：超时发生后同一 `driver_id` **连续 4 个请求全部被接受** | in-flight 未回落到基线；或后续请求被 503 `DEFINITION_QUERY_BUSY` ⇒ **许可泄漏** |
| **R-3** | **临时驱动进程被正确终止 / 回收**（无残留、无孤儿） | 进程枚举：请求返回后按驱动可执行文件名 + 父 PID 过滤（Windows `Get-Process` / 等价测试内枚举），与调用前进程快照做**差集**；同时断言临时监听端口已释放 | 差集中出现残留驱动进程；或出现孤儿（父进程已退、子进程存活）；或端口仍被占用 ⇒ 违反 `temp_op.rs:40-43` 的单出口回收 |
| **R-4** | **恢复能力**：后续请求可正常执行（超时**不把该 `driver_id` 的预算「锁死」**） | 超时后**立刻串行发 ≥ 2 个正常**定义查询 | 任一后续请求仍 503 `DEFINITION_QUERY_BUSY` / 仍超时 ⇒ 预算被锁死 |

---

## 2. 三阶段实施顺序（评审建议顺序；**最终文件范围以 PR #113 已批准设计为准**，DC §8.1）

### 2.1 阶段 1 · 协议与基础通道

| 项 | 内容 |
|---|---|
| **目标** | additive wire 增补 + Minor 版本门控 + SDK 默认实现 + 非 NCK 驱动兼容回归 |
| **涉及文件（新增）** | 无（本阶段不改 NCK 实现文件） |
| **涉及文件（修改）** | `proto/driver.proto`（`oneof body` `:22`–`:77` 内、末项 `:76` 之后追加 **tag 45–48** + 文件末尾 4 个 `message`）＝ M1；`crates/driver-protocol/src/lib.rs`（`:29` `PROTOCOL_MINOR 6→7`；`:34` 邻域新增 `DEFINITIONS_RPC_MIN_MINOR = 7`）＝ M2；`crates/driver-sdk/src/lib.rs`（`Driver` trait `:100-132` 末尾新增 2 个**带默认实现**方法；控制面 `match` `:1126`/`:1169` 邻域新增 2 个分支；**兜底分支 `:1178-1179` 不动**）＝ M3；**5 个 `driver.toml`**（`drivers/sinumerik-nck`、`drivers/s7`、`drivers/focas2`、`drivers/opcua`、`tests/support/test-driver`：`protocol_minor = 6 → 7`，本次实测 5 个文件当前**均为 6**）＝ M8；可选 M11（纯门控函数，**不得含计数状态**） |
| **冻结（不改）** | 既有 35 个 tag（`{10..44}` 连续无空洞）与全部既有 `message` 字段编号/类型（DC §7.2.1 逐字段表）；`PROTOCOL_MAJOR = 1`（`driver-protocol/src/lib.rs:23`）；`negotiate()`（`:100`）；`descriptor()` 默认实现（`driver-sdk:105-123`）；`MAX_FRAME_LEN`（`:52`） |
| **完成判据** | G4 `cargo build --locked --workspace` 成功；G5 `cargo test --workspace --locked` 全绿，**其中 `in_tree_manifest_minor_matches_protocol`（`crates/driver-protocol/src/lib.rs:683`，实测存在）必须过**；G6 合同测试 29/29；**4 个非 NCK 驱动源码零改动**（`git diff --stat` 只体现 5 个 `driver.toml` 的 minor 行 + `proto`/`driver-protocol`/`driver-sdk`） |
| **对应验收** | I9、I10、I27、I28、I30、I34、I35（+ G0、G5、G6） |
| **非 NCK 兼容测试设计（就位后才开工）** | ① 新 SDK 重建 4 个驱动、源码不变、既有套件原样通过（I30）；② 注入未知 oneof 变体 ⇒ 进程存活且后续 `Ping`/`GetDescriptor` 正常（I35）；③ 旧 Manager（minor 6）↔ 新驱动 ⇒ 协商 6、**永不发 tag 45–48**、驱动行为逐字一致（I34）；④ `negotiated_minor < 7` ⇒ Manager **门控先于发帧**、直接 501（I10）。**全部使用 `tests/support/test-driver` 与子进程注入，不连接任何真实设备** |
| **阻塞条件** | ① **生产代码 HOLD**：DC §0 `:19-21`、§11-1 明写 wire 增补「已获原则批准，但生产代码仍 HOLD，须待上级启动独立『契约变更』PR」⇒ **未获上级显式解除 HOLD 前不得改 `proto/**`**；② 本阶段的 wire 增补**不得夹带 PR-C**（DC §8.5）；③ 若发现既有 tag 编号/类型**不得不**破坏 ⇒ 停止上报 |

### 2.2 阶段 2 · Manager 与 Core API

| 项 | 内容 |
|---|---|
| **目标** | 临时驱动调用通道 + 只读路由 + 每 Driver 并发 4 + 资源限制 + 错误映射 + 超时与取消 |
| **涉及文件（新增）** | `crates/core-api/src/driver_query.rs`＝N3（**零 Siemens 标识**，只做形状校验 + 逐字透传 + `ErrorKind → StatusCode`） |
| **涉及文件（修改）** | `crates/driver-manager/src/session.rs`（`:654-690` 邻域新增 2 个 RPC 方法，同形态：5s 独立超时 @`:667` + 256 KiB 门 @`:677`）＝M4；`crates/driver-manager/src/manager.rs`（`:308-370` 邻域新增 2 个编排方法，走 `temp_driver_attempt`（`temp_op.rs:138`），**不发 `OpenConnection`**；**新增 per-driver 并发预算（P0-1）**；`:383-457` 的 `browse` 与 `:402-410` 的 `OpenConnection` 一字不动）＝M5；`crates/core-api/src/lib.rs`（路由表 `:3235-3342` 内、descriptor 路由 `:3241-3244` 邻域追加 2 条只读路由 + `mod driver_query;` + **新路由挂 `DefaultBodyLimit::max(64 * 1024)`**）＝M7；`tests/driver-contract/tests/management_api.rs`（**只新增用例，不删改既有断言**，如 `:676-699`）＝M9 |
| **冻结（不改）** | `session.rs:25` `REQUEST_TIMEOUT = 10s`（**不得调大**）、`:280` `DRIVER_STARTUP_TIMEOUT = 6s`、`temp_op.rs:25` `MAX_TEMP_ATTEMPTS = 2`、`manager.rs:57-62` 第二道 256 KiB 门、`core-api/src/lib.rs:834` Browse 分页（`:834` 实测 `unwrap_or(50).min(1000)`）、既有 40+ 条路由、`crates/core-types/**`、`crates/core-api/src/binding_gate.rs` |
| **完成判据** | G2/G3 clippy 零警告；**G7 grep 红线门**零命中；I36/I37/I38/I39 全过；I40–I43 四条只读性断言全过；I7/I8/I23 无副作用断言全过；I25/I26 Core 零语义与映射纯净度过；P0-3 的 T1–T6 全部实测通过并给出**对外超时上限** |
| **对应验收** | I7、I8、I23、I25、I26、I36、I37、I38、I39、I40、I41、I42、I43（+ I1/I5–I9 的 REST 侧） |
| **阻塞条件** | ① P0-1 载体未冻结 ⇒ 不写并发预算；② P0-3 未测出对外上限 ⇒ 不宣布阶段完成；③ **§11-14 的分页字段集合未冻结** ⇒ 不写响应信封（见 §3-14）；④ 若必须新建合同 suite 文件 ⇒ 停止上报（`scripts/contract_suites.py:8` 明写增删 suite 必须显式评审） |

### 2.3 阶段 3 · NCK Definition API

| 项 | 内容 |
|---|---|
| **目标** | 42 条权威定义目录 + 离线 Preview + **D1–D10 校验复用** + **与 Configure 的身份一致性** + 26/130 与 `scope` 证据 |
| **涉及文件（新增）** | `drivers/sinumerik-nck/src/definition_api.rs`＝N1（目录投影白名单 + 权威预览；**禁止** `use crate::client`；文件内**零** `async`/`.await`/socket）；`drivers/sinumerik-nck/tests/definition_api.rs`＝N2（不改既有 `tests/*.rs`）；`crates/core-api/src/driver_query.rs` 已由阶段 2 新建（本阶段只消费） |
| **涉及文件（修改）** | `drivers/sinumerik-nck/src/lib.rs`（`impl Driver` `:88-378` 内新增 2 个方法实现 + 顶部 `mod definition_api;`；**冻结区间**：`descriptor()` `:90-352`、`open_connection` `:354-377`、`configure` `:622` 起、`browse` `:952` 起、run/poll 面）＝M6；`drivers/sinumerik-nck/src/compiler.rs`（把 `:351-413` `compile_semantic` / `:416-444` `compile_native` 的「**D1–D10 校验 + 重算身份**」抽为**一个内部函数**，`check_identity`（`:489-520`，调用点 `:411`/`:442`）作为**独立末步（D11）**保留；新增 `preview_point_key(...)` 调用**同一内部函数**并**跳过 D11**；`code()` `:162-180` / `kind()` `:183-194` 不动）＝M10；`tests/driver-contract/tests/management_api.rs` 的 REST 侧补充 |
| **冻结（不改）** | `definition.rs`（含 `canonical_identity` `:1091-1133`）＝I-2 的**唯一**身份权威来源；`targets.rs`/`native.rs`/`codec.rs`/`value.rs`/`address.rs`/`client.rs`/`identity.rs`/`identity_spec.rs`/`presets.rs`；`definitions/828d/r1.json` + `DIGESTS.lock`；`presets/828d.json`（62 模板）、`tests/fixtures/legacy-828d-wire.json`（62 条）、`tests/fixtures/nck-point-key-golden.json`（55/5/77）；既有 NCK 测试（`legacy_equivalence.rs`/`semantic_equivalence.rs`/`presets_828d.rs`/`definition_catalog.rs`/`compiler_identity.rs`） |
| **完成判据** | G1 fmt 零差异；G5 全绿且既有 5 个 NCK 测试套件**原样通过、`checked` 计数不降**；G8 定义锁通过；I1–I6/I11–I22/I44/I45 全过；**I12（42 条逐条）与 I32（42 条 × Preview→Configure 全链）必须逐条通过，任一条不成立即失败**；`definition_api.rs` 只读 grep 门（I21/G7）零命中 |
| **对应验收** | I1、I2、I3、I4、I5、I6、I11–I22、I31（仅当 I-2a 落地）、I32、I33、I44、I45（+ I24 原样通过） |
| **阻塞条件** | ① P0-2 裁决未回写 DC 并冻结（尤其 §11-5）⇒ 不写预览的 kind 分支；② 若 I12/I32 要求改 `canonical_identity` 或新增第二套身份算法 ⇒ **停止上报**（违反 DC §8.1.3-F1）；③ 若目录自检失败无法 fail-closed（E7/E28）⇒ 停止上报；④ 若为了「UI 好看」出现「目录少 6 条 + 浏览树仍 62 叶」的组合 ⇒ **硬性违规**（DC §9.3-C′、I44） |

---

## 3. §11 十六条遗留事项处置表（逐条读 DC §11 `:1571-1591`；分类见列头）

> 分类：**(A) 阻塞实现** = 必须在开发前解决；**(B) 仅需测试验证** = 决策已定，只需断言锚定；**(C) 可以保留** = 接受现状，不需行动。
> **强制口径（评审确认 2）**：**A 类必须在修改相关生产代码之前解决**（不得"边写边定"）；**B 类必须明确对应到具体测试/断言**（表中"覆盖"列即该对应）；**C 类必须说明保留的理由与影响面**（表中"理由"列）。
> 汇总：**A 5 条 / B 6 条 / C 5 条**。A 类为 {1, 5, 9, 13, 14}。

| # | 原表述摘要（DC §11 原文口径） | 分类 | 理由（一句话） | 不阻塞时的覆盖（阶段 / 验收） |
|---|---|---|---|---|
| **1** | wire 的 additive 增补是否获准 —— 【已裁决】已获原则批准，但**生产代码仍 HOLD**，须待上级启动独立「契约变更」PR | **A** | 未解除 HOLD 前**无法改 `proto/**`**，阶段 1 整体不可开工 | — （动作：上级解除 HOLD / 以独立契约变更 PR 形态承载；验收 I27/I28） |
| **2** | Preview 端点路由命名与 `POST` 语义 —— 【已裁决】已批准 `POST …/definitions/identity-preview`，`POST` = 带参数的只读计算 | **C** | 已裁决且落地为设计条文，无残余决策 | 阶段 2/3；I40–I43（R8–R11 为「POST 非写入」的最终判据） |
| **3** | `family` 缺省语义与 PR-C 措辞不一致 —— 【已裁决】保留差异：目录**可省略**（= 全部已发布 family）；Preview **必须显式指定** | **B** | 口径已定，只需断言两个分支 | 阶段 3；**目录侧** I5（缺省 ⇒ 42 条；`840d-sl` ⇒ 200+`[]`+`note`；`foo` ⇒ 400 `UNSUPPORTED_FAMILY`）；**Preview 侧**「缺 `family` ⇒ 400 `INVALID_REQUEST`」**目前无 I 行**，属 §4.2.4 的 **G-c 缺口**，须补断言 |
| **4** | `digest` 字段超出 PR-C 枚举 —— 【已裁决修正】**允许**作为定义版本与完整性元数据返回 | **B** | 已裁决为允许；**实现时不得以「PR-C 未枚举」为由删除** | 阶段 3；I1（与 `DIGESTS.lock` 42 条逐字一致）、I2（白名单字段） |
| **5** | 预览是否支持 `nck_native` —— **范围解释（保留待确认）**：本文件把预览定义为对 `kind` 通用（含 `nck_native`+`derived`），**属范围扩大，仍待评审确认** | **A** | 裁决直接决定预览的接受/拒绝面；**P0-2 已裁决支持 `nck_native`+`derived`**，但 DC 未回写 ⇒ 开工会落在未冻结面上 | — （动作：回写 DC §11-5 + §2.5-B2/B3 后冻结；验收 I15/I16、E16） |
| **6** | `label` 有两个来源（目录 `definition.rs:72-77` vs 模板 `presets.rs:38`）—— 已确证的双真值事实；建议 I-2 落地后以目录 `label` 为准 | **C** | I-2 **不合并两者、不改模板**，其行为不受该差异影响；展示优先来源属 PR-C 决策 | 阶段 3；I2（目录 `label` 逐字投影）。残留：评审确认「展示优先来源」（PR-C 侧） |
| **7** | `ErrorKind → HTTP` 的已知不直观映射（`UNSUPPORTED_*` ⇒ 501、`INVALID_VARIABLE` ⇒ 404） | **C** | 替代方案（Core 按码映射）会**违反 P9 护栏**，故该取舍是唯一可行解，无可行动项 | 阶段 2/3；I26（映射函数只含 `ErrorKind` 变体）、I9（`Unsupported` 断言）、DC §6.2.1 全表 |
| **8** | `driver.toml` 的 `protocol_minor` 与能力协商 —— 【已裁决修正】M8 **必需且涉及 5 个文件**；**禁止**把 Manifest 字段当成实际能力协商的证明 | **B** | 结论已定且已有**既有门**强制同步；只需跑门 | 阶段 1；`driver-protocol/src/lib.rs:683` 的 P1 invariant 单测（属 **G5**）、I30；本次实测 5 个 `driver.toml` 均为 `protocol_minor = 6` |
| **9** | 未通读上位设计 `nck-semantic-point-refactor.md`（1387 行）全文；若存在与 I-2 冲突的条款，本次**可能未覆盖** | **A** | 上位设计是仓库「唯一事实来源」，其接口形态既有规定可能与 I-2 冲突 ⇒ 返工成本远高于复核成本 | — （动作：开工前通读全文并逐条比对 I-2 接口形态 / Point Key / task binding 条款，产出「无冲突」结论或差异清单） |
| **10** | 未运行任何测试 / 构建 —— 本次硬约束 | **B** | 只是「设计轮次未跑」，开工后按 DC §8.4 执行即可；本任务书已补跑基线（见 §6） | 阶段 1 起；G0/G4/G5/G6（本文件 §6 已实测 `cargo build --workspace` = 0） |
| **11** | `definition_api.rs` 的具体函数签名与目录投影的字段顺序 —— **待确认** | **C** | 属实现自由（DC 明写「实现 PR 内确定」），且**不得**改变字段语义与码表 | 阶段 3；I2（键集合恰为白名单，与签名无关） |
| **12** | 预览对 `scope=identity` 的返回值 —— 【已裁决】已定：**与 Configure 一致** | **B** | 口径已定，只需断言「Preview 不比 Configure 更严」 | 阶段 3；I45（对 6 条专门断言）、I44（目录如实返回 42 条 / `identity` 恰 6 条） |
| **13** | 并发预算的具体载体与落点（Manager `manager.rs` vs `driver-protocol`）—— **待确认**；预算值 4 已定 | **A** | 载体不冻结就无法写实现，且两种落点的**验收面不同**（P0-1 明确要求计数与释放在 Manager） | — （P0-1 已裁决：**Manager**；验收 I39 的 A1–A5） |
| **14** | `total` / `limit` / `next_cursor` 三个分页字段属本文件新增（PR-C 未枚举）—— 差异已登记；建议按「允许的元数据」处理，**若评审要求严格遵循 PR-C 枚举可删三字段而保留请求侧夹取** | **A** | 响应字段集合是**对外契约的一部分**；不冻结则 I37 的断言结构（①`limit==100`/`total==42`/`next_cursor==null`、③游标翻页）与前端消费都要返工 | — （动作：评审二选一并冻结；保留三字段 ⇒ I37 全条成立；删三字段 ⇒ I37-③ 改为「按 `limit` 多次请求不重不漏」，①/②/④/⑤ 不变） |
| **15** | 请求大小 64 KiB / 结果上限 500 条 & 256 KiB / 条目硬顶 4096 的取值 —— **本文档自定，无既有先例可直接照抄**；评审确认或给出替代数值 | **B** | 取值已给依据，且**断言结构不随数值变化**（只替换常量）⇒ 不阻塞实现 | 阶段 2；I36（64 KiB / +1 ⇒ 413）、I37④（>256 KiB 或 >4096 ⇒ 503 `DEFINITION_QUERY_TOO_LARGE`）、G7 |
| **16** | `REQUEST_TOO_LARGE`（413）不经 `ErrorKind` 映射 —— 设计取舍：由 HTTP 层 body 门在**进入 handler 之前**产生，是**唯一**不满足「HTTP 由 `ErrorKind` 决定」的路径 | **C** | 替代方案（handler 内手动读 body）会**放弃框架级门**、更易漏；现状可接受 | 阶段 2；I36（断言门挂在**新路由**上、不是全局 2 MB 默认值）、E30 |

**A 类清单（评审最关心，共 5 条）**：#1 wire HOLD、#5 预览 `nck_native` 范围、#9 上位设计全文复核、#13 并发预算载体、#14 分页字段集合。
**其中 #13 已由 P0-1 裁决（Manager）、#5 已由 P0-2 裁决（支持 `nck_native`+`derived`）** ⇒ 这两条的剩余动作只是「回写 DC 并冻结」，但**回写需改设计文档，须由上级执行**；#1 需上级解除 HOLD；#9 与 #14 需评审/上级给出结论。

---

## 4. 文件范围 ↔ 验收编号对应表

### 4.1 正向对应（文件 / 模块 → 验收编号 → 阶段）

| 文件（或模块） | 新增/修改/冻结 | 对应验收编号 | 阶段 |
|---|---|---|---|
| `proto/driver.proto` | M1 修改（`:22-77` 追加 tag 45–48；末尾 4 个 `message`） | I27、I28、I34、I35 | 1 |
| `crates/driver-protocol/src/lib.rs` | M2（`:29` Minor→7；`:34` 门控常量）+ M11（纯门控函数，可选） | I10、I27、I28；既有 `:683` 单测 ⇒ G5 | 1 |
| `crates/driver-sdk/src/lib.rs` | M3 修改（trait `:100-132` + 分发 `:1126`/`:1169`） | I9、I30、I35 | 1 |
| 5 × `driver.toml`（`drivers/sinumerik-nck`、`drivers/s7`、`drivers/focas2`、`drivers/opcua`、`tests/support/test-driver`） | M8 修改（`protocol_minor = 6 → 7`） | I30；`driver-protocol:683` 单测（G5） | 1 |
| `crates/driver-manager/src/session.rs` | M4 修改（`:654-690` 邻域 +2 方法；`:667` 5s、`:677` 256 KiB 形态复用） | I8、I38（T1/T2/T4）、I40 | 2 |
| `crates/driver-manager/src/manager.rs` | M5 修改（`:308-370` 邻域 +2 编排；**并发预算**；`:57-62` 第二道门） | I38、I39（A1–A5）、I40、I42 | 2 |
| `crates/core-api/src/lib.rs` | M7 修改（`:3235-3342` +2 路由；`DefaultBodyLimit::max(64*1024)`） | I7、I23、I25、I26、I36（④）、I40、I41、I42、I43 | 2 |
| `crates/core-api/src/driver_query.rs` | **N3 新增** | I1、I5–I9、I23、I25、I26、I36、I37、I38、I39、I40–I43 | 2 |
| `tests/driver-contract/tests/management_api.rs` | M9 修改（**只新增用例**） | I1、I5–I9、I23、I32、I33、I36–I43 | 2/3 |
| `tests/driver-contract/tests/protocol_negotiation.rs` | **补入（DC §8.1.2 漏列，见 §4.2 缺口 G-b）** | I10、I27、I28、I34、I35 | 1 |
| `drivers/sinumerik-nck/src/definition_api.rs` | **N1 新增** | I2–I4、I11–I22、I31、I32、I33、I36–I39、I44、I45 | 3 |
| `drivers/sinumerik-nck/src/lib.rs` | M6 修改（`:88-378` 内 +2 方法；冻结区间见 §2.3） | I1–I6、I11–I22、I44 | 3 |
| `drivers/sinumerik-nck/src/compiler.rs` | M10 修改（拆分 `:351-413`/`:416-444`；`check_identity` `:489-520` 为独立末步；新增 `preview_point_key`） | I12、I13、I14、I15、I16、I18、I19、I32、I45 | 3 |
| `drivers/sinumerik-nck/tests/definition_api.rs` | **N2 新增** | I2–I4、I11–I22、I31、I32、I33、I36–I39、I44、I45 | 3 |
| `scripts/contract_suites.py` | **冻结（F9）** | G6（29/29 名单不变） | 全程 |
| `.github/workflows/**` | 冻结（本 PR 不改） | G1–G11 口径来源 | 全程 |

### 4.2 覆盖性检查（口径：每个编号 → **恰好一个明确承担者** + 阶段）

> **口径（评审细化 1）**：`I/E/G` 编号**不要求与单个生产函数一对一** —— 属于协议兼容、运行时行为或 CI 级综合验证的编号，**允许**映射到**测试文件 / CI 作业 / 跨模块流程**。但**每个编号都必须有明确承担者**，**不得留空、不得含糊**，也**不得为了"补全"而虚构承担者**。
> 三张表分别给**正向**映射（4.2.1–4.2.3），缺口集中在 **4.2.4 无主编号清单**。反向映射见 §4.1；冻结面见 §4.4。

#### 4.2.1 I1–I45 → 承担者（逐个列出，无遗漏）

| 编号（逐个） | 承担者（测试文件 / CI 作业 / 跨模块流程） | 阶段 |
|---|---|---|
| I1 | `tests/driver-contract/tests/management_api.rs` + `drivers/sinumerik-nck/tests/definition_api.rs` | 2/3 |
| I2、I3、I4 | `drivers/sinumerik-nck/tests/definition_api.rs`（经 N1） | 3 |
| I5、I6、I7、I8、I9、I23 | `tests/driver-contract/tests/management_api.rs`（经 N3/M7） | 2 |
| I10 | `tests/driver-contract/tests/protocol_negotiation.rs`（**补入**，见 G-b） | 1 |
| I11、I12、I13、I14、I15、I16、I17、I18、I19、I20、I21、I22 | `drivers/sinumerik-nck/tests/definition_api.rs`（I21 另由 CI grep 作业 G7 承担） | 3 |
| I24 | 既有 `control_contract` 套件（跨模块流程，**原样通过**） | 2/3 |
| I25 | 既有 `resource_contract.rs` 的 P9 护栏（跨模块流程，**原样通过**，属 G6 的一部分） | 2/3 |
| I26 | CI grep 作业（G7）+ `resource_contract.rs` | 2 |
| I27、I28 | `tests/driver-contract/tests/protocol_negotiation.rs`（新增用例） | 1 |
| I29 | 既有 `legacy_equivalence.rs` / `semantic_equivalence.rs` / `compiler_identity.rs`（跨模块流程，**原样通过**） | 3 |
| I30 | 既有 `descriptor_contract.rs` + 其它驱动套件（跨模块流程，**原样通过**） | 1 |
| I31 | **无承担者**（I-2a 不做）⇒ 见 **G-1** | — |
| I32、I33 | `definition_api.rs` + `management_api.rs`（**e2e 跨模块流程**） | 3 |
| I34、I35 | `protocol_negotiation.rs` + `crates/driver-sdk` 单测 | 1 |
| I36、I37、I38、I39、I40、I41、I42、I43 | `management_api.rs` + `definition_api.rs`（`resource_contract.rs` **原样参与、不新增用例**，见 **G-h**） | 2/3 |
| I44 | `definition_api.rs` + 既有 `identity_browse` / `presets_828d.rs`（**原样通过**） | 3 |
| I45 | `drivers/sinumerik-nck/tests/definition_api.rs` | 3 |

#### 4.2.2 E1–E31 → 实现承担者 / 验收承担者（两列分开，避免"实现有主"掩盖"验收无主"）

| 编号（逐个） | 实现承担者 | 验收承担者 | 阶段 |
|---|---|---|---|
| E1 | M4/M5（Manager 编排） | **无**（I38 覆盖的是 E2）⇒ **G-g** | 2 |
| E2、E14 | M4/M5 | I38-T1（`DRIVER_UNAVAILABLE`） | 2 |
| E3、E15 | M4（5s 形态 @`:667`）/ M5 | I38-T2（`DEFINITION_QUERY_TIMEOUT`） | 2 |
| E4 | M5（第二道门，形态同 `manager.rs:57-62`） | I37-④ | 2 |
| E5 | M2/M11（门控）+ M3 | I10 | 1 |
| E6 | M3（SDK 默认实现） | I9 | 1 |
| E7、E28 | N1/M6（`CatalogError::code()` 透传，**fail-closed**） | **无** ⇒ **G-d** | 3 |
| E8 | N1/M6 | I5 | 3 |
| E9 | N1/M6（**不是错误**：200 + `[]` + `note`） | I5 | 3 |
| E10、E19 | N1/M6 | I6、I17 | 3 |
| E11、E20 | N1/M6 | I6、I17 | 3 |
| E12 | N3（形状层）+ N1 | I6、I37-⑤ | 2/3 |
| E13 | N3（Core 形状层） | **无** ⇒ **G-c** | 2 |
| E16 | M10（kind 路由）/ N1 | **无**（= P0-2 第三行）⇒ **G-a** | 3 |
| E17 | M10 | **仅部分**（I16 只覆盖同码的 `user` 条件）⇒ **G-f** | 3 |
| E18 | M10 | I16 | 3 |
| E21 | M10（`compiler.rs:376-381`） | **无** ⇒ **G-e** | 3 |
| E22、E23、E24、E25、E26 | M10 | I18 | 3 |
| E27 | M10 | I19 | 3 |
| E29 | M10（`check_identity`，D11） | I33 | 3 |
| E30 | M7（`DefaultBodyLimit::max(64*1024)`） | I36 | 2 |
| E31 | M5（P0-1 并发预算） | I39 | 2 |

#### 4.2.3 G0–G11 → 承担者

| 编号 | 承担者（CI 作业 / 流程） | 阶段 |
|---|---|---|
| G0 | 开发流程前置步骤（`AGENTS.md` 口径；**本地实测见 §6**） | 全程 |
| G1 | CI 作业 `_rust-quality.yml` 的 `fmt` 步 / 本地 `cargo fmt --all -- --check` | 3 |
| G2、G3 | CI clippy 步 / 本地 `cargo clippy --workspace --all-targets -- -D warnings` | 2/3 |
| G4 | 构建作业 `cargo build --locked --workspace` | 1 |
| G5 | CI 全量测试作业（含 `driver-protocol:683` 的 P1 invariant 单测） | 1/3 |
| G6 | CI 合同测试作业 `cargo test --locked -p mesa-contract-tests --all-features`（29/29） | 全程 |
| G7 | **新增 CI grep 红线作业**（只读红线门，三项子项） | 2/3 |
| G8 | CI 定义锁作业（`tools/ci/nck_definition_lock.py` + 其自测） | 3 |
| G9 | 证据产出流程 `python scripts/write-contract-evidence.py` → `target/validation/contract.json` | 交付前 |
| G10 | **PR-C 前端作业**（`pnpm --dir apps/mesa-web …`）—— **不在 I-2 范围** | — |
| G11 | CI `rust-perf` 作业 —— **不在本 PR 范围**（I-2 无热路径改动） | — |

#### 4.2.4 无主编号清单（**真实缺口，逐条列出；不虚构承担者**）

| # | 编号 | 缺口性质 | 处置 |
|---|---|---|---|
| **G-1** | **I31** | **完全无承担者**（唯一一个）：I31 仅当 I-2a 落地，而【已裁决】**I-2a 保持可选**；本任务书按「不做 I-2a」组织（阶段 1–3 均不含 I-2a） | 若上级要求做 I-2a ⇒ 追加文件范围并重新评审（**不得生成第二份变量定义真值**） |
| **G-a** | **E16** | 实现**有主**（M10/N1），验收**无主**：矩阵无任何 I 行断言 `UNSUPPORTED_POINT_KIND`（DC 仅 `:924` 定义、`:982` 归属）；而它是 **P0-2「未知 Point Kind 明确拒绝」** 的码 | **阶段 3 必须补断言**：`kind=foo` ⇒ 501 `UNSUPPORTED_POINT_KIND` 且 `error.kind == "Unsupported"` |
| **G-b** | I10、I27、I28、I34、I35 的**承载文件** | 编号本身有主，但 DC §8.1.2 修改清单**漏列** `tests/driver-contract/tests/protocol_negotiation.rs`，而 §8.3/§8.4 把这 5 条放在该套件 ⇒ **文件范围与验收编号不闭合** | 本任务书 §4.1 已**显式补入**该文件（阶段 1）；实现 PR 必须把它写进文件范围 |
| **G-c** | **E13** | 实现有主（N3），验收**无主**（DC 仅 `:921` 定义、`:1762` 正文声明） | 阶段 1/2 补断言：body 非法 JSON / 未知字段 / 缺 `family`/`kind`/`parameters` / 出现 `point_key` ⇒ 400 `INVALID_REQUEST` |
| **G-d** | **E7、E28** | 实现有主（N1/M6），验收**无主**（fail-closed 的目录自检失败无任何断言） | 阶段 3 补断言：注入损坏目录 ⇒ 503 `DEFINITION_DIGEST_MISMATCH`，且**绝不**「跳过坏定义继续响应」 |
| **G-e** | **E21** | 实现有主（M10），验收**无主**（I5 只覆盖「未声明 family」） | 阶段 3 补断言：`def.family` ≠ 请求 `family` ⇒ 501 `UNSUPPORTED_FAMILY` |
| **G-f** | **E17** | 实现有主（M10），验收**部分无主**（I16 只覆盖同码的 `identity_mode="user"`） | 阶段 3 补断言：`parameters` 未知键 / 类型错 ⇒ 400 `INVALID_BINDING_CONFIG` |
| **G-g** | **E1** | 实现有主（M4/M5），验收**部分无主**（I38 覆盖的是 E2） | 阶段 2 补断言：未知 `driver_id` ⇒ 503 `DRIVER_UNAVAILABLE` |
| **G-h** | I39、I43 的「所在套件」列 | DC §8.3 把二者标注含 `resource_contract`，但 **F8 将 `resource_contract.rs` 冻结为「逐字不动」**，且 §8.4 未把它列为新增用例的套件 ⇒ **三处口径不一致** | 本任务书口径：I39/I43 的**新增用例**只落在 `management_api.rs` / `definition_api.rs`；`resource_contract.rs` **原样参与** |

> **结论**：**I1–I45 中仅 I31 完全无承担者**（属设计上的可选范围，不是遗漏）；**E1–E31 的实现承担者齐全，但 E13/E16/E7+E28/E21 验收无主、E17/E1 部分无主**；**G0–G11 承担者齐全**。
> 上述缺口**不得通过改动 DC 的编号来「补全」**（编号已冻结）—— 只能**新增断言**或**显式登记为缺口**。
> **§3 的 A 类条目**：#1/#5/#9/#13/#14 均已在阶段 1/2/3 的「阻塞条件」中显式落位（§2.1/§2.2/§2.3）。

### 4.3 反向检查（文件范围内每个文件至少一个验收编号）

| 文件 | 结果 |
|---|---|
| §4.1 表中全部 15 个文件 | **均有 ≥ 1 个验收编号**（见上表） |
| `scripts/contract_suites.py`、`.github/workflows/**` | **无直接验收**，见 **§4.4 冻结面**与 **G6/G1–G11 流程**（它们是流程/门禁载体，不是被验收对象） |
| `tests/support/test-driver/**`（若需扩展测试驱动能力） | **无直接验收**；它是阶段 1 非 NCK 兼容测试（I9/I34/I35）的**测试夹具**，其 `driver.toml` 已被 M8 覆盖 |

### 4.4 冻结面（不改的文件 / 契约，违反即为架构错误）

1. **既有 proto 消息与字段编号**：`oneof body` 的 35 个既有 tag `{10..44}`（连续无空洞）+ 全部既有 `message` 字段编号/类型 + `PROTOCOL_MAJOR = 1`（DC §7.2.1）；
2. **62 条 wire golden 与身份 golden**：`tests/fixtures/legacy-828d-wire.json`、`presets/828d.json`（62 模板）、`tests/fixtures/nck-point-key-golden.json`（55/5/77）；两道等价门仍 `checked == 62`（I29）；
3. **Descriptor 契约 2/0**：`descriptor_contract.rs` 逐字不动并通过（I30）；`crates/core-types/**`（`descriptor.rs` 2/0、`point.rs`、`schema.rs`、`resource.rs`、`capability.rs`、`event.rs`、`probe.rs`）逐字不动；
4. **其它驱动源码零改动**：`drivers/s7`、`drivers/focas2`、`drivers/opcua`、`tests/support/test-driver`（**仅 `driver.toml` 的 minor 同步**，M8；I30）；
5. **Core 语义边界**：Core **零 Siemens 标识**、只按 `ErrorKind` 映射 HTTP；P9 护栏 `crates/driver-contract/tests/resource_contract.rs:396-461` **原样通过**（I25/I26）；
6. **NCK 身份权威唯一性**：`definition.rs`（含 `canonical_identity:1091-1133`）不得新增第二套算法；`check_identity`（`compiler.rs:489-520`）语义逐字不变（F1/F2、I12/I32）；
7. **其它冻结**：`crates/core-api/src/binding_gate.rs`、`drivers/sinumerik-nck/definitions/828d/{r1.json,DIGESTS.lock}`（**一个字节不改** ⇒ G8 必过）、三份设计文档、`docs/adr/**`、`apps/mesad/**`、`crates/config-store/**`、`crates/event-store/**`、`scripts/contract_suites.py`（29 套名单不变）。

---

## 5. 阻塞条件与开工前提

### 5.1 开工前必须满足（缺一不可，可判定）

| # | 前提 | 判定方式 | 不满足的后果 |
|---|---|---|---|
| P-1 | **wire 生产代码 HOLD 被上级显式解除**，且确认以独立「契约变更」PR 形态提交 | 上级书面指令 / PR 描述 | 阶段 1 不可开工（`proto/**` 不得触碰） |
| P-2 | **P0-1 口径确认（四项）**：预算 4、立即拒绝不排队、**计数与释放在 Manager**、`driver-protocol` 不承担运行时计数 | 设计回写或评审确认 | 阶段 2 不可开工 |
| P-3 | **P0-2 四项裁决回写 DC 并冻结**（尤其 §11-5 的 `nck_native` 范围） | DC 修订版 | 阶段 3 不可开工 |
| P-4 | **§11-14 分页字段集合冻结**（保留 `total`/`limit`/`next_cursor` 或删除并改用请求侧夹取） | 评审二选一 | 阶段 2 响应信封不可写 |
| P-5 | **§11-9 上位设计全文复核完成**，产出「无冲突」或差异清单 | 复核记录 | 阶段 3 风险不可控（可能返工） |
| P-6 | **阶段 1 的非 NCK 兼容测试设计就位**：① 4 个驱动源码零改动（只改 `driver.toml` minor）；② 旧 Manager ↔ 新驱动降级；③ 未知 tag 存活；④ 门控先于发帧。**均使用 `tests/support/test-driver` + 子进程注入，不连接真实设备** | 测试设计评审通过 | 阶段 1 完成判据不可判 |
| P-7 | **G0 前置已满足**：改驱动代码后先 `cargo build --workspace`（否则子进程类合同测试静默失效） | 见 §6 实测 | 合同测试结果不可信 |
| P-8 | 基线现状确认：本次实测 5 个 `driver.toml` 均 `protocol_minor = 6`、`PROTOCOL_MINOR = 6`（`driver-protocol/src/lib.rs:29`）、既有 tag 为 `{10..44}` 共 35 个 | 见 §6 实测 | 版本同步范围会算错 |

### 5.2 任一阶段出现以下情况，**必须停下来报告**（不得自行决定）

1. **非 NCK 驱动需要改源码**（不只是 `driver.toml` 的 minor）⇒ 违反 P1 护栏与 I30；
2. **既有契约不得不破坏**：任一既有 tag 编号/类型/语义变化、`PROTOCOL_MAJOR` 需变、Descriptor 2/0 变化、62 条 wire golden 或 55/5/77 身份 golden 变化；
3. **需要新建合同 suite 文件**（`scripts/contract_suites.py:8` 明写增删 suite 必须显式评审本文件）；
4. **身份算法出现第二份实现**：为满足 I12/I32 而修改 `canonical_identity` 或复制一套算法；
5. **并发预算无法在 Manager 内落地**（例如必须引入新依赖或改动 `driver-protocol` 的运行期状态）；
6. **P0-3 测出取消路径无法及时回收临时进程**（后台 attempt 在 HTTP 结束后仍存活）；
7. **权限 / 环境阻塞**：需要连接真实机床才能验证、需要安装类操作、需要启动 `mesad`/Vite；
8. **E 码验收缺口（§4.2 的 G-a/G-c/G-d/G-e/G-f/G-g）无法在不改设计的前提下补断言**；
9. 任何「目录少 6 条 + 浏览树仍 62 叶」的迹象（DC §9.3-C′ 硬性禁止）。

---

## 6. 基线检查（本次实际执行；未跑的如实说明）

> 全部在 `D:\Mine\Project\mesa`、分支 `feat/nck-i2-backend`、HEAD `ff56855` 上执行。
> **未执行任何 git 写操作**；**未启动 `mesad`/Vite**；**未连接任何设备或机床**；**未执行任何安装类操作**。

| # | 命令 | 退出码 | 关键输出 |
|---|---|---|---|
| 1 | `git rev-parse --abbrev-ref HEAD` / `--short HEAD` / `git status --porcelain` | 0 | `feat/nck-i2-backend` / `ff56855` / **空输出（干净）** |
| 2 | `git log --oneline -1`、`git rev-parse --short main` | 0 | `ff56855 Merge pull request #113 from dooooling/docs/nck-i2-boundary`；`main = ff56855` ⇒ **HEAD == main** |
| 3 | `(Get-Content <file>).Count`（**不用** `Measure-Object -Line`） | 0 | I-2 文档 **1802**；`proto/driver.proto` 453；`driver-manager/src/manager.rs` 679；`session.rs` 2044；`temp_op.rs` 166；`core-api/src/lib.rs` 3397；`driver-sdk/src/lib.rs` 2358；`driver-protocol/src/lib.rs` 1100；`nck/src/lib.rs` 2018；`definition.rs` 1429；`compiler.rs` 798；`targets.rs` 463；`presets.rs` 520；`management_api.rs` 3094；`resource_contract.rs` 461 |
| 4 | `cargo build --workspace`（**先执行**） | **0** | 第 1 次：`Finished \`dev\` profile … in 3.69s`；第 2 次（复核）：`in 1.78s`。**驱动二进制确实被重编**（两次均编译/链接了 `mesa-driver-sinumerik-nck` 等），**不是"仅测试进程启动成功"** |
| 5 | `cargo test --locked -p mesa-contract-tests --all-features`（**在 #4 之后**） | **0** | 见 **§6.1**（33 个 `test result:` 行 / **213 passed / 0 failed** / 1 ignored / 29 个合同套件） |
| 6 | 并发闸门复核（见 §1.1 表） | 0 | **`crates/**` + `apps/mesad/**` 零命中** ⇒ 无并发闸门可复用 |
| 7 | 只读 grep 复核（`PROTOCOL_MINOR`/`DRIVER_STARTUP_TIMEOUT`/`MAX_TEMP_ATTEMPTS`/`256 * 1024`/`unwrap_or(50).min(1000)`/`in_tree_manifest_minor_matches_protocol`/`TimeoutLayer`/`oneof body`） | 0 | 逐条与 DC 引用一致（`driver-protocol:29/34/52/683`、`session.rs:25/280/667/677`、`temp_op.rs:25`、`manager.rs:57`、`core-api:834/3241-3244/3341`、`driver.proto:22/76/261`）；**额外发现：`core-api` router 无 `TimeoutLayer`/`DefaultBodyLimit`** |
| 8 | `Get-ChildItem -Recurse -Filter driver.toml`（`drivers/` + `tests/`） | 0 | **5 个**，`protocol_minor` **全部 = 6** |

### 6.1 `cargo test --locked -p mesa-contract-tests --all-features`（**实测，已取得最终结论**）

> **执行顺序说明（评审确认 3）**：`cargo build --workspace` **先行**、契约测试**后行**；两次均独立记录退出码。
> **不因"测试进程启动成功"就认定驱动构建正常** —— 上表 #4 的驱动编译/链接输出即为构建正常的证据，且 #5 的退出码为 **0**。

| 指标 | 实测值 |
|---|---|
| 命令 | `cargo test --locked -p mesa-contract-tests --all-features` |
| **退出码** | **0** |
| `test result:` 行数 | **33**（含 4 个 0 用例的 lib/bin/doc-test 目标 ⇒ **29 个合同套件有实际用例**，与 `scripts/contract_suites.py` 的 29 套名单一致） |
| **`passed` 合计** | **213** |
| **`failed` 合计** | **0** |
| `test result: FAILED` 行数 | **0** |
| `ignored` | **1**（位于耗时 60.07s 的套件内，属既有 `#[ignore]` 用例） |
| 结论 | **合同测试基线通过（29/29 套件、213 passed / 0 failed）**；本次未跑 `cargo test --workspace --locked`（G5）与 clippy/fmt（G1–G3），**未跑即未跑，不得据此声称 G1–G3/G5 通过** |

---

## 7. 规模说明（本文件 384 行的构成与增量理由）

评审确认 1 明确「**250–400 行仅为建议规模，以准确覆盖工作范围为优先**」。本文件的实际构成为：**§1 三项 P0 的可执行验收（含 P0-1 的独立复核证据、P0-2 的四行裁决、P0-3 的 T1–T6 + R-1–R-4）约 1/4**；**§4 的编号→承担者正向映射（I 45 条 + E 31 条 + G 12 条逐条列出）与 §4.2.4 无主清单约 1/4**；其余为阶段分解、§11 十六条处置与基线证据。
**增量理由**：① 评审细化 1 要求「每个编号都要有明确承担者、不得留空」⇒ 正向映射表不可压缩为区间（压缩即"含糊"）；② 评审细化 2 要求 P0-3 的四条资源回收断言各带观测方式与失败判据；③ 评审确认 2 要求 A/B/C 三类分别落到"开发前解决 / 对应测试 / 理由与影响"。**若评审要求压回 400 行以内，可压缩的只有 §4.2.1–4.2.3 的三张映射表（改为区间 + 缺口清单），但会违反细化 1 的"不得含糊"口径** ⇒ 本文件选择保持准确覆盖。

