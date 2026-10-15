# A22：Probe 阶段耗时只读量测方案（SINUMERIK 828D）

- 状态：**方案已就绪，尚未执行**（需在现场授权窗口内由人执行）
- 依据：`docs/evidence/connection-c1-contract.md` 第 4 节第 1 条（A22 真机分阶段量测
  是 V1.1 定义的 C1 合并阻断项）
- 被测对象：**仅**已有实测证据的那台实体 828D（现有成功连接参数，不猜测 TSAP）
- 操作范围：**只做连接探测**。禁止轴运动、程序启动、参数写入、批量采集、
  未经批准的扫描

## 1. 要回答的问题

当前 Probe 预算是由**阶段模型推出的工程上界**，不是实测结论：

| 阶段 | 当前预算 | 代码位置 |
|---|---|---|
| Driver 启动 + IPC 握手 | ≤ 6s | `DRIVER_STARTUP_TIMEOUT` |
| `OpenConnection`（仅配置解析） | 5s | `PROBE_OPEN_BUDGET` |
| `Probe` RPC（**含协议握手**） | `4 × timeout_ms + 3s`，封顶 45s | `ProbeBudgets::query` |
| `CloseConnection` | 5s | `PROBE_CLOSE_BUDGET` |
| 临时进程清理 | ≤ 6s | `PROBE_CLEANUP_BUDGET` |

量测要回答三件事：

1. NCK 实际路径上，TCP connect / COTP CC / S7 Setup 各占多少（是否真的接近 4 阶段模型）；
2. `timeout_ms=5000` 与 `10000` 下，**正常**响应是否会被外层预算提前截断；
3. Probe 结束后子进程与端口租约是否确实回收。

> ⚠️ 容易混淆：**45s 是单次 Probe RPC 的阶段上限，不是整个 Probe 的总上限。**
> `timeout_ms=10000` 时理论总预算 = 6 + 5 + 43 + 5 + 6 ≈ **65s**。
> 该数值需写入 A22 证据，供 C2 前端设计"测试连接"等待状态时使用。

## 2. 执行前的必要代码改动（尚未实施）

现有 Probe 路径**没有分阶段计时输出**：`S7Session::connect` 内部按顺序执行
`TcpStream::connect` → `iso_connect()`（COTP CR/CC）→ `s7_setup()`，
但只在外层记录一条 `S7 会话建立` 日志，无法区分各阶段耗时。

因此执行量测前需先加**最小侵入的阶段计时**（建议一处改动）：

- 在 `crates/s7-transport/src/session.rs` 的 `connect` / `iso_connect` / `s7_setup`
  中，用 `std::time::Instant` 记录每阶段耗时，并以 `tracing::info!` 输出
  结构化字段（`stage`、`elapsed_ms`），**不记录 host/TSAP/凭据等敏感配置**；
- 阶段命名固定为 `tcp_connect` / `cotp_cr_send` / `cotp_cc_recv` / `s7_setup_send` /
  `s7_setup_recv`，便于脚本化汇总；
- 不改任何超时语义、不改错误分类——纯观测。

该改动对 s7 驱动同样生效（同一 transport），属可复用改进；
若希望严格限定在 NCK 路径，可在 NCK 侧包一层计时，但会重复实现。

## 3. 量测步骤

### 3.1 前置

```bash
# 确认工作区干净、二进制为当前提交构建
git rev-parse HEAD && git status --porcelain=v1
cargo build --workspace
```

连接参数沿用该机**已验证**的配置（不重新猜测）：

| 项 | 值 |
|---|---|
| driver | `sinumerik-nck` |
| family | `828d` |
| host / port | 该机现有值（**不写入公开日志**） |
| local_tsap / remote_tsap | 该机现有值（**不写入公开日志**） |

### 3.2 两组探测

用 `POST /api/v1/drivers/sinumerik-nck/probe`，body 只含 `connection`：

- **第一组**：`timeout_ms = 5000`，重复 **5** 次
- **第二组**：`timeout_ms = 10000`，重复 **3** 次

每次之间间隔 ≥ 2s，避免设备侧连接资源瞬时争用。

### 3.3 每次记录

| 字段 | 说明 |
|---|---|
| `run` | 组内序号 |
| `timeout_ms` | 本次配置 |
| `stage_elapsed_ms` | 各阶段耗时（单调时钟，见第 2 节） |
| `total_elapsed_ms` | Probe 端到端（单调时钟） |
| `http_status` | REST 状态码（200/400/503/504） |
| `reachable` | 探测结果 |
| `pdu` | 协商 PDU（来自日志 `NCK 会话建立 pdu=`） |
| `cleanup` | 子进程是否退出、端口租约是否释放 |

**清理核验**（每次探测后）：

- 无 `mesa-driver-sinumerik-nck` 残留进程；
- `%TEMP%\mesa-driver-ports\` 中本次新增的 `port-*.lock` 已消失；
- 紧接着再做一次探测仍成功（无端口冲突）。

### 3.4 禁止事项

- 不得为量测启动正式采集（不 `start` Endpoint、不配置任务、不读点位）；
- 不得写入任何 NC 参数、不得触发程序或轴运动；
- 证据文件**不得**包含 IP、TSAP、凭据；用占位符（如 `<site-host>`）替代。

## 4. 验收判据

| 判据 | 通过条件 |
|---|---|
| 正常响应不被截断 | 两组中**全部**正常探测都返回 200 且 `reachable=true`；无 `504` / `ProbeError::Timeout` |
| 阶段可归因 | 每条记录都能给出各阶段耗时；若出现失败，失败能定位到具体阶段 |
| 预算合理性 | 实测 `total` 明显小于理论总预算（≈65s），且 `timeout_ms=10000` 组未被 Query 预算截断 |
| 清理无残留 | 第 3.3 节三项清理核验全部通过 |
| 模型校验 | 实测阶段数与 `PROBE_PROTOCOL_PHASES=4` 的偏差被明确记录（若实际阶段更少/更多，需据此复核该常量与 `CONNECTION_TIMEOUT_MAX_MS`） |

**结论边界**：单台 828D 通过只证明**该设备当前路径**的时间分布，
**不能**宣称所有 828D、840D sl 或全部驱动的真实硬件性能已验证。

## 5. 产出

1. `docs/evidence/a22-probe-timing-828d.txt`：原始记录（脱敏）+ 汇总表 + 结论；
2. 若实测与模型有偏差，更新 `ProbeBudgets` 常量与 `connection-c1-contract.md`
   第 4 节第 1 条（把「工程上界」改为「实测结论」）；
3. 把理论总预算（`timeout_ms=10000` ≈ 65s）写入证据，供 C2 前端等待状态设计引用。

## 6. 当前状态

- [x] 方案设计（本文）
- [x] **阶段计时最小改动（第 2 节）—— 已实施**
  - `crates/s7-transport/src/session.rs`：新增 `StageTimer`（RAII 守卫，**Drop 时**输出
    `stage` + `elapsed_ms`），因此成功与失败（含 `?` 提前返回）都会记录——
    手写"每步之后打日志"会恰好漏掉最需要计时的超时/错误阶段；
  - 阶段名固定为 `tcp_connect` / `cotp_cr_send` / `cotp_cc_recv` /
    `s7_setup_send` / `s7_setup_recv`，与第 2 节一致；
  - 用 `Instant`（宿主单调时钟）；**不输出 host / TSAP / 凭据**；
  - 协议行为与超时语义**零改动**（只加计时与日志）；
  - 回归保护：`crates/s7-transport/src/fixture.rs` 新增
    `handshake_emits_all_stage_timings`——捕获 tracing 事件并断言 5 个阶段
    全部出现、带 `elapsed_ms`、且不含 TSAP。若将来有人删掉某个计时点，
    真机量测会静默缺一段数据，而该测试会失败。
    NOTE: 捕获必须用**全局**订阅者（`set_global_default`，进程内装一次）：
    线程局部的 `set_default` 在 libtest 并行执行下观测不到事件
    （实测单独跑通过、全量跑捕获为空）。
- [ ] 现场只读量测执行（需授权窗口）
- [ ] 证据落盘与常量复核
