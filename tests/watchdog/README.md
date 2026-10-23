# stop-watchdog 离线可重复测试框架（M0–M12）

本目录是 `scripts/stop-watchdog.ps1` 的**可复现离线测试**：假端点桩 + 场景驱动 + 断言。
它把原先只存在于 `target/validation/`（gitignored，别人和 CI 都拿不到）的验证手段固化进仓库，
使 **Windows / Linux 都能独立复现**全部离线场景。

* 不连真实设备，不启动 `mesad`，不依赖 8132 生产实例，不依赖 Rust 代码；
* 需要：**pwsh 7+** 与 **python3**（Windows 上 `python` 亦可）；
* 驱动会**现场计算** G7 部署版本凭证（`git rev-parse HEAD`，无 git 时退回部署二进制 sha256），
  所以脚本里的 G7 校验是真比对，不是"随便填个值就过"。

## 一条命令跑完全部场景

```powershell
# Windows
pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1

# Linux
pwsh -NoProfile -File ./tests/watchdog/run-watchdog-scenarios.ps1
```

| 参数 | 说明 |
|---|---|
| `-Scenario M7,M12` | 只跑指定场景（逗号分隔；`-File` 传参按逗号拆分） |
| `-List` | 只列出场景 |
| `-EvidenceRoot <dir>` | 证据根目录（默认 `target/validation/stop-watchdog`，gitignored） |
| `-KeepEvidence` | 保留上一轮证据（默认每场景重建，避免旧证据冒充新结论） |
| `-PythonExe <path>` | 指定 python 解释器（默认自动探测 `python3` / `python` / `$env:PYTHON`） |

退出码：**0 = 全部场景所有断言通过；1 = 至少一条断言失败**（可直接当 CI 门禁用）。
机器可读汇总写入 `<EvidenceRoot>/suite-summary.json`（含每场景 T0、Stop 发出时刻、
相对 T0 的时刻、偏差、退出码、三条判定轴、断言计数）。

## 场景与断言

| 场景 | 桩的故障旋钮 | 关键断言 |
|---|---|---|
| **M0** 正常路径 | 无 | exit 0 / STOPPED_CLEAN；调度 PASS、采集 VALID、收敛 CONVERGED；截止 Stop \|dev\| ≤ 1000 ms |
| **M1** 观测阻塞 36 s | `--get-delay-running 36` | exit 2；**调度 PASS 但采集 INVALID_OBSERVATION_GAP**；单次 GET ≥ 30 s（真解耦，不是超时兜底）；收敛 CONVERGED |
| **M2** Start 超时 | `--start-delay 20`（> StartTimeoutSec 10） | exit 3；Start uncertain=true；截止 Stop 仍准时；采集 VALID |
| **M3** 端点运行中失联 | `--shutdown-after 4`（桩到点自杀，之后连不上） | 退出码非 0；调度 PASS（截止 Stop 仍按 T0+30 s **发出**）；采集 INVALID_*；收敛 ∈ {NOT_CONVERGED, INDETERMINATE}；**Stop 请求确实失败（http=0）**，不假装停住了 |
| **M4** Stop 自身挂起 | `--stop-delay 15` | exit 0；Stop 请求耗时 ≥ 14 s；**Stop 挂住期间观测仍 ≥ 20 条样本**（双向解耦） |
| **M5** 迟到 Start 竞态 | `--race-after-stop 5` | exit 4；记录 RACE；出现 `RACE_START_AFTER_STOP` 的再次 Stop；最终仍 STOPPED |
| **M6** 首次通信失败即停 | `--fail-after 6` | exit 5；`S2_COMM_FAIL` 首次确认即停（≤ T0+15 s）；采集 VALID；截止 Stop 仍发出 |
| **M7** ARMED 误启动防护 | 无 | exit 0；保持 ≥ 1500 ms；`start_dispatched=false`、`flows_started=0`、无 `t0.stamp`；**桩未收到任何 START/STOP**；ARMED 也逐项校验凭证 |
| **M8** OBS 心跳失效 | `--get-delay 60`（GET 全无响应） | exit 2；`OBS_STALL` 提前 Stop（≤ T0+8 s）；采集 INVALID_OBSERVATION_GAP；**截止 Stop 仍在 T0+30 s 准时发出** |
| **M9** HostChecks 查询失败 | `-TestHostProbeError both`（注入权限/查询错误） | exit 9；④⑤ 判 `indeterminate`；收敛 INDETERMINATE；**不得 exit 0、不得判停净**；采集仍 VALID |
| **M10** 工作线程异常退出 | `-TestAbortFlow timer` | exit 8；`WORKER_ABORTED`；立即 Stop（reason=WORKER_ABORT）+ **MON 接管截止调度**（DEADLINE_FAILSAFE）；控制台给出处置提示 |
| **M11** 授权门正向 | 无（凭证齐全且校验通过） | exit 0；`authorization.overall=verified`；G7 与仓库 HEAD 实际一致；任务配置 revision/point_count 实读一致；边界放行；正常 Start 与截止 Stop |
| **M12** 授权门负向 | 两段：(a) 缺 `-Authorized/-AuthorizedBy`；(b) `-ExpectedTaskRevision rev99` 与桩的 `rev13` 不一致 | 两段都 **exit 40**、**未写 t0.stamp**、**桩未收到任何 START**；拒绝标签分别为 `REFUSED_NOT_AUTHORIZED` / `REFUSED_TASKCONFIG_MISMATCH`；(b) 确实做过只读核验 GET |

> M9 / M10 用到的 `-TestHostProbeError` / `-TestAbortFlow` 是**离线测试专用注入开关**；
> 正常执行时保持缺省值，注入值会写进 `summary.json` 的 `test_hooks`，不会伪装成真实结论。

## 被测脚本的命令行契约（ARMED / FIRED）

```powershell
# ARMED（缺省）：只自检 + 校验凭证，零副作用
pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode arm `
     -BaseUrl http://127.0.0.1:18080 -OutDir <dir> `
     -G6Signoff "<签署人/编号>" -G6SignedAt "2026-10-24T09:00:00+08:00" `
     -G7DeployVersion "<git rev-parse HEAD>" -ExpectedTaskRevision rev13 -ExpectedPointCount 12

# FIRED（正式执行）：必须 -Mode fire + 显式授权 + 授权人 + 凭证 + 任务配置期望值
pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode fire `
     -Authorized -AuthorizedBy "李工" -AuthzTicket "<工单号>" `
     -G6Signoff "<签署人/编号>" -G6SignedAt "2026-10-24T09:00:00+08:00" `
     -G7DeployVersion "<commit SHA 或 sha256:<hex>>" `
     -ExpectedTaskRevision rev13 -ExpectedPointCount 12 `
     -BaseUrl http://127.0.0.1:8132 -OutDir <dir>
```

### 授权边界（重要）

| 模式 | 允许的 BaseUrl |
|---|---|
| `arm`（缺省） | **只允许回环地址且端口≠8132**（8132 是生产 mesad）。非回环或指向 8132 ⇒ **exit 40，无参数可绕过** |
| `fire` + 四项凭证**校验通过** | 允许任何 host（含 `127.0.0.1:8132` 生产 mesad、`192.168.15.60` 等现场地址） |
| `fire` 但任一项缺失/校验失败 | **exit 40**，不进入 FIRED、不发 Start |

### 四项凭证是"校验"，不是"记录"

| 凭证 | 参数 | 校验方式 |
|---|---|---|
| 显式授权 | `-Authorized` + `-AuthorizedBy` | 开关必须显式给出，且授权人标识非空（可追溯）。**arm 模式不要求**（记 `not_checked`），它是 FIRED 的门 |
| 授权凭证号 | `-AuthzTicket` | 非空（arm 记 `not_checked`） |
| G6 现场签署 | `-G6Signoff` + `-G6SignedAt` | 签署标识非空；签署时间必须可解析（ISO 8601）且不在未来 |
| G7 部署版本 | `-G7DeployVersion` | 与仓库 HEAD / `git describe` 实比对；或 `sha256:<hex>` 与 `target/{release,debug}/mesad` 实际哈希比对；**读不到 ⇒ 无法校验 ⇒ 按未通过处理** |
| 任务配置 | `-ExpectedTaskRevision` + `-ExpectedPointCount` | **先做只读 GET**：`/api/v1/endpoints/{id}`（存在性）+ `/diagnostics`（point_count）+ `/tasks/{id}`（revision），与实际值比对；不一致 ⇒ `REFUSED_TASKCONFIG_MISMATCH`，读不到 ⇒ `REFUSED_CREDENTIAL_UNVERIFIABLE` |

## 退出码与试验结论（分开表达）

| 退出码 | verdict / refused | 含义 |
|---|---|---|
| 0 | `ARMED_READY` | arm：自检通过 + 四项凭证全部校验通过（仅"可进入 FIRED"的条件） |
| 1 | `ARMED_INPUTS_MISSING` | arm：凭证缺失或校验未通过 |
| 2 | `ARMED_SELFTEST_FAIL` | arm：通路自检失败 |
| 3 | `ARMED_ENDPOINT_UNREACHABLE` | arm：端点不可达 |
| **40** | `REFUSED_NON_OFFLINE` | arm 指向非回环地址或生产端口 8132 |
| **40** | `REFUSED_NOT_AUTHORIZED` | fire 缺 `-Authorized` / `-AuthorizedBy` / `-AuthzTicket` |
| **40** | `REFUSED_CREDENTIAL_INVALID` | G6 时间不可解析、G7 与 HEAD/二进制指纹不一致 |
| **40** | `REFUSED_TASKCONFIG_MISMATCH` | 任务配置与实际只读 GET 不一致 |
| **40** | `REFUSED_CREDENTIAL_UNVERIFIABLE` | 凭证读不到（无 git、端点读不到）→ 按未通过处理 |
| 0 | `STOPPED_CLEAN` | fire：收敛 CONVERGED + 采集 VALID + Start 确认 + 无竞态 |
| 1 | `NOT_STOPPED` | 已观测到仍未收敛（最高严重级） |
| 2 | `STOPPED_OBS_INVALID` | 观测中断/盲区/截断 → 采集 INVALID（即使 Stop 准时） |
| 3 | `STOPPED_START_UNCERTAIN` | Start 在途/超时，生命周期未确认 |
| 4 | `STOPPED_WITH_RACE` | 记录到 RACE_START_AFTER_STOP |
| 5 | `STOPPED_BY_EARLY_TRIGGER` | Stop 由 S1/S2/S3/S6 提前触发 |
| 6 | `STOPPED_WITH_ANOMALIES` | 上述 2–5 命中 ≥2 项 |
| 7 | `HOSTCHECKS_UNVERIFIED` | 显式 `-SkipHostChecks` → "停干净"未验证 |
| 8 | `WORKER_ABORTED` | 监护流发现工作流异常退出，本轮作废 |
| 9 | `CONVERGENCE_INDETERMINATE` | 本机核对无法确定（查询失败/权限不足） |

> **exit 40 = "授权边界拒绝，非设备故障"**：它不是 Stop 失败、也不是采集异常；此时**未发出 Start、
> 未进入 FIRED**。现场不要按设备故障处置（`summary.json` 的 `note` 字段也会写明这一点）。

`summary.json` 里同时给出三条**互相独立**的判定轴，便于"调度 PASS + 采集 INVALID"这类组合表达：

```json
"axes": {
  "schedule":    { "verdict": "PASS", "max_abs_deviation_ms": 12.1, "threshold_ms": 1000 },
  "sampling":    { "verdict": "INVALID_OBSERVATION_GAP", "valid": false, "max_gap_ms": 36100.0 },
  "convergence": { "state": "CONVERGED" },
  "start": "returned", "race": "NONE", "workers": "OK"
},
"trial": "INVALID_SAMPLING",   // PASS / NO_GO / INVALID_SAMPLING / UNVERIFIED / ABORTED / NO_GO_ANOMALY
"verdict": "STOPPED_OBS_INVALID",
"exit_code": 2
```

**T0+30 s 的语义**：它是 **Stop 请求的截止调度目标**，不是"驱动进程或设备通信已在第 30 秒
物理终止"的保证；物理终止由 `six_items` / `axes.convergence` 另行判定。

## 证据目录内容

每个场景目录（`<EvidenceRoot>/<场景>/`，M12 下再分 `a-missing-authorization/`、`b-taskconfig-mismatch/`）：

| 文件 | 内容 |
|---|---|
| `console.log` | watchdog 的标准输出/错误（含判定与处置提示） |
| `run-log.md` | 事件流水（五个执行流、FIRED、Stop、MON 事件、凭证校验） |
| `t0.stamp` | T0 与截止时刻（仅 fire 且进入 FIRED 才有） |
| `watchdog-states.jsonl` | 每次观测一行（相对 T0 的时刻、state、epoch、req_ms、err） |
| `watchdog-stops.jsonl` | 每次 Stop 一行（原因、发出时刻、相对截止偏差、http、耗时） |
| `summary.json` | 判定汇总（6 项三态、三条轴、workers、credentials、test_hooks、verdict、exit_code）；被拒绝时是拒绝记录（`refused` / `note` / `network_actions`） |
| `arm-report.json` | 仅 arm 模式：探活/凭证校验/保持时间/副作用清单 |
| `stub-requests.jsonl` | 假端点桩收到的每个请求（用于**独立**交叉核对 Stop/Start 时刻） |
| `stub-stdout.log` / `stub-stderr.log` | 桩的原始输出 |

## CI 接入建议

```yaml
# 示例（Linux runner 需预装 pwsh 与 python3；G7 凭证自动取 git HEAD）
- run: pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1
  timeout-minutes: 25
```

单场景耗时 5 s（M7/M12）到 ~60 s（M3/M4/M8），全量约 **9–11 分钟**。
若 CI 机器上有其它 mesa 驱动进程或到目标端口的 Established 连接，M0/M11 的 HostChecks 会正确地
判为未收敛——那是**预期行为**（说明该机器不适合做"停干净"验收），不是测试缺陷。

## 失败时怎么处置

1. 先看该场景的 `console.log` 末尾（判定 + 处置提示）与 `summary.json` 的 `axes`；
2. 断言失败会打印 `[FAIL] 断言名 — 实际值`，对照上表定位是哪一层（调度/采集/收敛/授权/工作流）；
3. 不得为了让断言通过而放宽脚本阈值（尤其 `ObserveStallSec`、`SAMPLING_MAX_GAP_MS`、`MAX_STOPS`）——
   这些阈值本身就是被验收对象。
