# stop-watchdog 离线可重复测试框架（M0–M16）

本目录是 `scripts/stop-watchdog.ps1` 的**可复现离线测试**：假端点桩 + 场景驱动 + 断言。
它把原先只存在于 `target/validation/`（gitignored，别人和 CI 都拿不到）的验证手段固化进仓库，
使 **Windows / Linux 都能独立复现**全部离线场景。

**当前规模（2026-10-23 修订）**：脚本 **2809 行**、本目录 **4 个文件**、**17 个场景（M0–M16）**、
**174 项断言**（旧版本曾记录为 1054 行 / 7 场景 / 2267 行 / 13 场景 / 94 断言，均已过期）。

* 不连真实设备，不启动 `mesad`，不依赖 8132 生产实例，不依赖 Rust 代码；
* 需要：**pwsh 7+** 与 **python3**（Windows 上 `python` 亦可）；
* **G7 不再用 git HEAD 充当凭证**（那只能证明"与仓库版本匹配"，不能证明部署物身份）：
  驱动离线构造"部署二进制 fixture + 受信构建清单"，由被测脚本**实算 sha256** 比对；
* **任务配置核验核对完整配置**（连接 host/port/TSAP/PDU + 逐点 Key 与参数 + 规范化指纹），
  不再只比 revision / point_count。

## 一条命令跑完全部场景

```powershell
# Windows
pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1

# Linux
pwsh -NoProfile -File ./tests/watchdog/run-watchdog-scenarios.ps1
```

| 参数 | 说明 |
|---|---|
| `-Scenario M7,M13` | 只跑指定场景（逗号分隔；`-File` 传参按逗号拆分） |
| `-List` | 只列出场景 |
| `-EvidenceRoot <dir>` | 证据根目录（默认 `target/validation/stop-watchdog`，gitignored） |
| `-KeepEvidence` | 保留上一轮证据（默认每场景重建，避免旧证据冒充新结论） |
| `-PythonExe <path>` | 指定 python 解释器（默认自动探测 `python3` / `python` / `$env:PYTHON`） |
| `-DeployBinaryPath <path>` | G7 的"部署二进制"（默认用本轮生成的 `_fixtures/mesad-fixture.bin`；也可指向真实 `target/release/mesad`，仍不会启动它） |

退出码：**0 = 全部场景所有断言通过；1 = 至少一条断言失败**（可直接当 CI 门禁用）。
机器可读汇总写入 `<EvidenceRoot>/suite-summary.json`（含每场景 T0、Stop 发出时刻、
相对 T0 的时刻、偏差、退出码、三条判定轴、断言计数，以及本轮的 G7 fixture 与获批配置指纹）。

## 本轮新增的三项阻塞修复（B1/B2/B3）

### B1 测试注入参数必须限制在离线环境

`-TestHostProbeError` / `-TestAbortFlow` 是**离线测试专用**注入开关，但旧版只在各自使用点
（HostChecks 的 ④⑤、协调者主循环的注入中止）读取，**没有统一保护**；而 fire 模式放行任意 host。
把测试参数带进真实执行命令，会在 Start 之后**故意**中止工作流（WORKER_ABORT）或把本机核对
伪造成"无法确定"——那不是设备故障，是我们自己制造的异常。

现在的实现（`scripts/stop-watchdog.ps1` 的 `Test-WdInjectionGuard`，位于参数解析之后、
**任何 HTTP 之前**）：

| 情形 | 结果 |
|---|---|
| 不带注入参数 | 不适用（正常执行路径不受约束） |
| 带注入参数 + 回环地址 + 端口≠8132 | 允许（仅离线测试） |
| 带注入参数 + **非回环地址** | **exit 40** `REFUSED_TEST_INJECTION_ONLINE`，`network_actions = 0` |
| 带注入参数 + **生产端口 8132** | 同上 |
| 带注入参数 + BaseUrl 不是合法 http/https | 同上 |

拒绝发生在**任何连接建立之前**：`summary.json` 的 `network_actions` 为 0，
`extra.injection_guard` 记录判定依据（host/port/loopback/参数清单）。对应场景 **M13**。

### B2 G7 必须证明实际部署二进制的身份

旧实现主要比 git 仓库 HEAD / `git describe`——只能证明"输入值与仓库版本匹配"，
**不能**独立证明实际运行的 `mesad` 就是这个版本构建的。

现在 `-G7DeployVersion` **只接受**两种形式：

| 形式 | 校验方式 |
|---|---|
| `sha256:<64 hex>` | 与 `-G7BinaryPath`（缺省探测 `target/{release,debug}/mesad[.exe]`）**实算**摘要比对 |
| `manifest:<path>` | 从**受信构建清单**读期望摘要（`{"binary","sha256"}` 或 `{"artifacts":{...}}`），再与部署二进制实算摘要比对；清单自身摘要也记入证据供审计 |

* git HEAD / `git describe` **只作辅助信息**记录（`actual.git`，标注 `auxiliary-only`），**不作为通过依据**；
* 两者都不可用（未给摘要/清单、清单读不到或格式错、定位不到二进制、摘要算不出）⇒ `unverified`
  ⇒ **拒绝进入 FIRED**（fail-closed），**不得**降级为"用 git HEAD 代替"；
* 显式给了 `-G7BinaryPath` 却不存在 ⇒ 直接 `unverified`，**不回退**到 `target/` 下别的二进制。

对应场景 **M14**（5 个子用例：只给 git HEAD / 二进制缺失 / 清单缺失 / 清单摘要不符 / 完全未提供）。
正向的两种形式分别在 **M11**（sha256）与 **M16**（manifest）里真实跑通。

### B3 任务预检必须核对完整配置

旧实现只校验 revision 与 point_count，不足以证明任务与获批配置一致。现在 FIRED 前还必须核对：

| 项 | 参数 | 校验方式 |
|---|---|---|
| 连接配置 | `-ApprovedTaskConfig` 的 `connection` | `host` / `port` / `local_tsap` / `remote_tsap` / `pdu_length` 逐字段比对（字段名与 `drivers/sinumerik-nck/src/config.rs:26-41` 一致） |
| 获批点位完整 Key 与参数 | `-ApprovedTaskConfig` 的 `tasks` | 逐点比对 `tasks[].binding.config.points[]` 的 `point_key` 与**全部参数**（缺失 / 多出 / 参数被改都会点名具体 Key） |
| 任务配置指纹 | `-ExpectedTaskFingerprint`（`sha256:<64 hex>`） | ① 获批配置文件自身的指纹必须等于该值（防止拿错/被改过的批准文件）；② 实际只读 GET 到的 tasks 指纹必须等于该值 |

指纹定义：**SHA-256(规范化 tasks 数组)**，规范化 = 对象键递归排序 + 按 `task.id` 排序 + 紧凑 JSON（UTF-8、无空白）。
PowerShell 侧（`ConvertTo-WdCanonicalNode` / `Get-WdTasksFingerprint`）与 python 侧
（`stub_endpoint.py --fingerprint-file`）是**两套独立实现**，逐字节等价、互为交叉核对。

只读 GET 用 Core 的真实形态 `GET /api/v1/tasks?endpoint=<id>`（`crates/core-api/src/lib.rs:2317-2337`），
404/405 时回退到 `GET /api/v1/tasks/<id>`（兼容旧桩）。对应场景 **M15**（负向 3 子用例：
连接 PDU / 点位参数 / 指纹）与 **M16**（正向：全部一致 ⇒ 放行并正常执行）。

> 说明：为了核对连接配置，凭证核验阶段会额外读一次 `GET /api/v1/endpoints/{id}`（Core 只在这条路径
> 暴露 connection）。因此 M8 的桩旋钮从"所有 GET 挂起"改为"`desired_running=true` 时 GET 挂起"
> （`--get-delay-running`）——凭证核验发生在 Start 之前（desired=false），必须能完成；
> M8 要验证的"FIRED 之后观测通道无响应 ⇒ 心跳失效"语义不变，断言未放宽。

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
| **M8** OBS 心跳失效 | `--get-delay-running 60`（运行中 GET 全无响应）；`-ObserveMaxSec 45` | exit 2；`OBS_STALL` 提前 Stop（≤ T0+8 s）；采集 INVALID_OBSERVATION_GAP；**截止 Stop 仍在 T0+30 s 准时发出** |
| **M9** HostChecks 查询失败 | `-TestHostProbeError both`（注入权限/查询错误） | exit 9；④⑤ 判 `indeterminate`；收敛 INDETERMINATE；**不得 exit 0、不得判停净**；采集仍 VALID |
| **M10** 工作线程异常退出 | `-TestAbortFlow timer` | exit 8；`WORKER_ABORTED`；立即 Stop（reason=WORKER_ABORT）+ **MON 接管截止调度**（DEADLINE_FAILSAFE）；控制台给出处置提示 |
| **M11** 授权门正向 | 无（凭证齐全且校验通过；G7 = `sha256:` 形式） | exit 0；`authorization.overall=verified`；G7 与**指定的部署二进制实算摘要**一致；任务配置（revision + point_count + 连接 + 点位 + 指纹）verified；边界放行；正常 Start 与截止 Stop |
| **M12** 授权门负向 | 两段：(a) 缺 `-Authorized/-AuthorizedBy`；(b) `-ExpectedTaskRevision rev99` 与桩的 `rev13` 不一致 | 两段都 **exit 40**、**未写 t0.stamp**、**桩未收到任何 START**；拒绝标签分别为 `REFUSED_NOT_AUTHORIZED` / `REFUSED_TASKCONFIG_MISMATCH`；(b) 确实做过只读核验 GET |
| **M13** 测试注入参数离线边界（B1 负向） | 三个子用例**都带完整有效凭证**：① `http://192.0.2.10:18093` + `-TestAbortFlow`；② `http://127.0.0.1:8132`（生产端口）+ `-TestHostProbeError`；③ `http://<本机主机名>:18093`（非回环字面量）+ `-TestAbortFlow`（桩绑 `0.0.0.0`，守卫失效时请求会真的到达） | 三者都 **exit 40**、标签 `REFUSED_TEST_INJECTION_ONLINE`、**`network_actions = 0`**、无 `t0.stamp`、**桩收到 0 条请求**（更不会有 Start/Stop）；守卫如实记录参数与字面 host |
| **M14** G7 部署身份不可验证（B2 fail-closed） | 5 子用例：只给 git HEAD / `sha256:` 但二进制不存在 / `manifest:` 清单不存在 / 清单摘要不符 / 完全未提供 | 全部 **exit 40**；G7 `state` 分别为 `unverified`/`unverified`/`unverified`/`invalid`/`missing`（**绝不为 verified**）；无 `t0.stamp`；桩未收到 START；子用例 ① 额外证明 git HEAD 被**记录为辅助信息**但**未用于放行** |
| **M15** 完整任务配置不匹配（B3 负向） | 3 子用例：① 获批 PDU=480（桩 240）；② 获批某点位参数 offset 0→99；③ `-ExpectedTaskFingerprint` 与获批文件不符 | 全部 **exit 40**、标签 `REFUSED_TASKCONFIG_MISMATCH`、无 `t0.stamp`、桩未收到 START；拒绝原因**点名具体差异**（`pdu_length` / `点位参数不一致：<key>` / 指纹） |
| **M16** 完整任务配置匹配（B3 正向） | 无（连接 + 全部 12 个点位 + 指纹全一致；G7 = `manifest:` 形式） | exit 0 / STOPPED_CLEAN；G7 `source=manifest` 且清单自身摘要可审计；`task_config=verified`；实际指纹 = 获批指纹（两套独立实现一致）；**逐点核对 12 个点位**；连接 host/port/TSAP/PDU 全一致；Start 确认 + 截止 Stop 准时 |

> M9 / M10 用到的 `-TestHostProbeError` / `-TestAbortFlow` 是**离线测试专用注入开关**；
> 正常执行时保持缺省值，注入值会写进 `summary.json` 的 `test_hooks`（含 `injection_guard` 判定），
> 不会伪装成真实结论。B1 之后它们**只能**配合回环非 8132 端点使用（见 M13）。

## 被测脚本的命令行契约（ARMED / FIRED）

```powershell
# ARMED（缺省）：只自检 + 校验凭证，零副作用
pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode arm `
     -BaseUrl http://127.0.0.1:18080 -OutDir <dir> `
     -G6Signoff "<签署人/编号>" -G6SignedAt "2026-10-24T09:00:00+08:00" `
     -G7DeployVersion "sha256:<部署 mesad 的 sha256>" -G7BinaryPath "<部署 mesad 路径>" `
     -ExpectedTaskRevision rev13 -ExpectedPointCount 12 `
     -ApprovedTaskConfig "<获批任务配置.json>" -ExpectedTaskFingerprint "sha256:<指纹>"

# FIRED（正式执行）：必须 -Mode fire + 显式授权 + 授权人 + 凭证 + 完整任务配置期望值
pwsh -NoProfile -File scripts/stop-watchdog.ps1 -Mode fire `
     -Authorized -AuthorizedBy "李工" -AuthzTicket "<工单号>" `
     -G6Signoff "<签署人/编号>" -G6SignedAt "2026-10-24T09:00:00+08:00" `
     -G7DeployVersion "manifest:<受信构建清单.json>" -G7BinaryPath "<部署 mesad 路径>" `
     -ExpectedTaskRevision rev13 -ExpectedPointCount 12 `
     -ApprovedTaskConfig "<获批任务配置.json>" -ExpectedTaskFingerprint "sha256:<指纹>" `
     -BaseUrl http://127.0.0.1:8132 -OutDir <dir>
```

获批任务配置的形态（`_fixtures/approved-task-config.json` 即此结构）：

```json
{
  "schema": "mesa.stop-watchdog.approved-task-config.v1",
  "endpoint_id": "lx-nck-tags-190-connection",
  "revision": "rev13",
  "connection": { "host": "127.0.0.1", "port": 18099,
                  "local_tsap": 256, "remote_tsap": 256, "pdu_length": 240 },
  "tasks": [ { "id": "nck-tags-1",
               "schedule": { "mode": "poll", "interval_ms": 1000 },
               "binding": { "kind": "nck.points.v1",
                            "config": { "points": [ { "point_key": "nck.task1.p1",
                                                      "kind": "variable",
                                                      "parameters": { "...": "..." } } ] } } } ]
}
```

### 授权边界（重要）

| 模式 | 允许的 BaseUrl |
|---|---|
| `arm`（缺省） | **只允许回环地址且端口≠8132**（8132 是生产 mesad）。非回环或指向 8132 ⇒ **exit 40，无参数可绕过** |
| `fire` + 四项凭证**校验通过** | 允许任何 host（含 `127.0.0.1:8132` 生产 mesad、`192.168.15.60` 等现场地址） |
| `fire` 但任一项缺失/校验失败 | **exit 40**，不进入 FIRED、不发 Start |
| 任意模式 + **测试注入参数** | 只允许回环且端口≠8132；否则 **exit 40 `REFUSED_TEST_INJECTION_ONLINE`**，网络动作数 = 0 |

### 四项凭证是"校验"，不是"记录"

| 凭证 | 参数 | 校验方式 |
|---|---|---|
| 显式授权 | `-Authorized` + `-AuthorizedBy` | 开关必须显式给出，且授权人标识非空（可追溯）。**arm 模式不要求**（记 `not_checked`），它是 FIRED 的门 |
| 授权凭证号 | `-AuthzTicket` | 非空（arm 记 `not_checked`） |
| G6 现场签署 | `-G6Signoff` + `-G6SignedAt` | 签署标识非空；签署时间必须可解析（ISO 8601）且不在未来 |
| G7 部署身份 | `-G7DeployVersion`（+ `-G7BinaryPath`） | `sha256:<hex>` 或 `manifest:<path>`，与部署二进制**实算摘要**比对；git HEAD/describe 仅作辅助记录；不可验证 ⇒ `unverified` ⇒ 拒绝 |
| 任务配置 | `-ExpectedTaskRevision` + `-ExpectedPointCount` + `-ApprovedTaskConfig` + `-ExpectedTaskFingerprint` | **先做只读 GET**（diagnostics / tasks / 端点快照）与获批配置**逐项、逐点、指纹**比对；不一致 ⇒ `REFUSED_TASKCONFIG_MISMATCH`，读不到 ⇒ `REFUSED_CREDENTIAL_UNVERIFIABLE` |

## 退出码与试验结论（分开表达）

| 退出码 | verdict / refused | 含义 |
|---|---|---|
| 0 | `ARMED_READY` | arm：自检通过 + 四项凭证全部校验通过（仅"可进入 FIRED"的条件） |
| 1 | `ARMED_INPUTS_MISSING` | arm：凭证缺失或校验未通过 |
| 2 | `ARMED_SELFTEST_FAIL` | arm：通路自检失败 |
| 3 | `ARMED_ENDPOINT_UNREACHABLE` | arm：端点不可达 |
| **40** | `REFUSED_NON_OFFLINE` | arm 指向非回环地址或生产端口 8132 |
| **40** | `REFUSED_TEST_INJECTION_ONLINE` | 携带测试注入参数却指向非回环地址或 8132（B1；网络动作数 = 0） |
| **40** | `REFUSED_NOT_AUTHORIZED` | fire 缺 `-Authorized` / `-AuthorizedBy` / `-AuthzTicket` |
| **40** | `REFUSED_CREDENTIAL_INVALID` | G6 时间不可解析、G7 部署二进制摘要与期望不一致、任务配置指纹/字段不一致 |
| **40** | `REFUSED_TASKCONFIG_MISMATCH` | 完整任务配置与获批配置不一致（连接配置 / 点位 Key 与参数 / 指纹 / revision / point_count） |
| **40** | `REFUSED_CREDENTIAL_UNVERIFIABLE` | 凭证读不到或无法独立验证（G7 无摘要/清单、部署二进制定位不到、端点读不到、获批配置不可用）→ fail-closed |
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

每个场景目录（`<EvidenceRoot>/<场景>/`；M12/M13/M14/M15 下再分子用例目录）：

| 文件 | 内容 |
|---|---|
| `console.log` | watchdog 的标准输出/错误（含判定与处置提示） |
| `run-log.md` | 事件流水（五个执行流、FIRED、Stop、MON 事件、凭证校验、注入守卫判定） |
| `t0.stamp` | T0 与截止时刻（仅 fire 且进入 FIRED 才有；被拒绝时**不存在**——这本身是断言） |
| `watchdog-states.jsonl` | 每次观测一行（相对 T0 的时刻、state、epoch、req_ms、err） |
| `watchdog-stops.jsonl` | 每次 Stop 一行（原因、发出时刻、相对截止偏差、http、耗时） |
| `summary.json` | 判定汇总（6 项三态、三条轴、workers、credentials、test_hooks、verdict、exit_code）；被拒绝时是拒绝记录（`refused` / `note` / `network_actions` / `extra`） |
| `arm-report.json` | 仅 arm 模式：探活/凭证校验/保持时间/副作用清单 |
| `stub-requests.jsonl` | 假端点桩收到的每个请求（用于**独立**交叉核对 Stop/Start 时刻、以及"拒绝时到底有没有连接"） |
| `stub-stdout.log` / `stub-stderr.log` | 桩的原始输出 |

`<EvidenceRoot>/_fixtures/` 下是本轮凭证 fixture（部署二进制、受信清单、tasks、获批配置），
`<EvidenceRoot>/suite-summary.json` 汇总全部场景并记录本轮 G7 fixture 与获批指纹。

## CI 接入（Windows + Linux 双平台，并行）

workflow：`.github/workflows/_watchdog.yml`（`workflow_call`），由 `ci.yml` 的 planner 驱动：

| 项 | 值 |
|---|---|
| 平台 | `windows-latest` + `ubuntu-latest`（**并行**，`fail-fast: false`） |
| 单 job 超时预算 | **25 分钟**（`timeout-minutes: 25`） |
| 环境检查 | 显式校验 **pwsh 7+** 与 **python3**；缺失即 `throw`（**明确失败，不静默跳过**） |
| 执行 | `pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1 -EvidenceRoot target/validation/stop-watchdog` |
| 证据 | `actions/upload-artifact`（`if: always()`，保留 14 天，名字 `watchdog-<platform>`） |
| 触发条件 | planner 输出 `watchdog=true` 才运行（**绝不无条件跑**）：`scripts/stop-watchdog.ps1`、`tests/watchdog/**`（`.md` 除外）、`.github/workflows/_watchdog.yml`，以及 fail-closed FULL（未知路径/空 diff/`Cargo.lock`/`tools/ci/**` 等） |
| 门禁 | `ci-gate` 严格判定：`watchdog=true` ⇒ 两个平台都必须 `success`；`watchdog=false` ⇒ 必须 `skipped`（跳过不算失败，"该跑却跳过"一定失败）；`watchdog` 不是 bool ⇒ FAIL CLOSED |

`ci-gate` 自身的严格性有离线自测：`python3 tools/ci/test_ci_gate.py`（CI quality job 内执行）。

单场景耗时 5 s（M7/M12/M13/M14/M15）到 ~60 s（M3/M4/M8），全量约 **10–13 分钟**。
若 CI 机器上有其它 mesa 驱动进程或到目标端口的 Established 连接，M0/M11/M16 的 HostChecks 会正确地
判为未收敛——那是**预期行为**（说明该机器不适合做"停干净"验收），不是测试缺陷。

## 失败时怎么处置

1. 先看该场景的 `console.log` 末尾（判定 + 处置提示）与 `summary.json` 的 `axes`；
2. 断言失败会打印 `[FAIL] 断言名 — 实际值`，对照上表定位是哪一层（调度/采集/收敛/授权/注入边界/工作流）；
3. 不得为了让断言通过而放宽脚本阈值（尤其 `ObserveStallSec`、`SAMPLING_MAX_GAP_MS`、`MAX_STOPS`）——
   这些阈值本身就是被验收对象；同理不得把注入守卫、G7 摘要比对、任务配置完整核对"简化"掉。
