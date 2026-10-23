# 828D 一期开发环境与离线验收 SOP（可复现）

- 状态：一期（phase 1）开发/离线验收标准作业程序（SOP）。
- 基线：`main = bf4d3f6079b83c791daa6853cc58a7bfb04bf6de`（本次核实时的 HEAD，工作树干净）。
- 目的：让**其他开发人员**按本文件从零搭起环境、跑完三条离线验收主线，并清楚知道**哪些结论不能下**。
- 边界：**不连真机、不做硬件验收、不启动生产实例**。离线全绿 ≠ 硬件已验证（见 §6）。
- 事实口径：本文所有版本、命令、路径、行号均从仓库实际文件核实；不能由仓库确定的标 **待确认**。若文档与代码冲突，**以代码为准**（冲突清单见附录 A）。

---

## 1. 环境准备

### 1.1 版本矩阵

| 组件 | 精确版本 | 仓库依据 | 验证命令 | 期望输出 |
|---|---|---|---|---|
| Rust toolchain | **1.95.0** | `rust-toolchain.toml:3`；`Cargo.toml:26-27`（edition 2024 / rust-version 1.95） | `rustc --version` | `rustc 1.95.0 (...)` |
| Cargo | 随 toolchain（1.95.0） | 同上 | `cargo --version` | `cargo 1.95.0 (...)` |
| Node.js | **22.18.0** | `.github/workflows/_web.yml:23` | `node --version` | `v22.18.0` |
| pnpm | **10.28.0** | `.github/workflows/_web.yml:19` | `pnpm --version` | `10.28.0` |
| Python | **3.x（次版本未 pin）** | `_watchdog.yml:51-52`、`_e2e.yml:65-67` 只校验 `major >= 3` | `python --version`（或 `python3 --version`） | `Python 3.x.y` |
| PowerShell | **7+（pwsh）** | `_watchdog.yml:43-44`、`_e2e.yml:57-58` 断言 `Major >= 7` | `$PSVersionTable.PSVersion` | `Major` ≥ 7 |
| cargo-nextest | **0.9.146（精确）** | `.github/actions/rust-ci-setup/action.yml:74`；`scripts/write-contract-evidence.py:39` | `cargo nextest --version` | `cargo-nextest 0.9.146` |
| Vite（前端构建/开发） | 5.4.21（lock 解析值） | `apps/mesa-web/package.json:32`（`^5.4.0`）；`apps/mesa-web/pnpm-lock.yaml:1900` | `pnpm --dir apps/mesa-web exec vite --version` | 含 `5.4.21`（可能带 `vite/` 前缀） |

### 1.2 版本纪律（不是建议）

1. **cargo-nextest 版本是硬门**：证据脚本要求本机版本与 CI pin 完全一致，不一致直接 `exit 6` 且**不写** `contract.json`（`scripts/write-contract-evidence.py:235-241`）。不要用“更新一点也能跑”的版本产出证据。
2. **Rust 版本以 `rust-toolchain.toml` 为唯一真值**：CI 也是解析该文件的 `channel`（`.github/actions/rust-ci-setup/action.yml:38-50`），升级只改这一处。
3. **Python 只承诺 3.x**：CI 仅断言 `major >= 3`，没有 pin 次版本（`_e2e.yml:66`、`_watchdog.yml:52`）。注意 `tools/ci/plan.py:51` 使用 `tomllib`（3.11+），那是 **CI planner** 的要求，不是三条离线主线的硬依赖。
4. `rust-toolchain.toml:4-5` 声明 `profile = "minimal"`、组件 `rustfmt`/`clippy`；nextest 不是 rustup 组件，需另行安装，但**版本必须 0.9.146**。安装方式不在本仓库约束内，本文不提供未经验证的安装命令。

---

## 2. 从零到可运行

### 2.1 获取代码并确认基线

```bash
git clone <仓库地址> mesa
cd mesa
git rev-parse HEAD        # 期望 bf4d3f6079b83c791daa6853cc58a7bfb04bf6de
git status --porcelain    # 期望：无输出（干净）
```

工作树必须干净：证据脚本把 `git status --porcelain --untracked-files=normal` 的任何非空输出都记为 `dirty=true`（`scripts/write-contract-evidence.py:68-76`、`:336`）。

### 2.2 前端依赖

```bash
pnpm --dir apps/mesa-web install --frozen-lockfile
```

依据：CI 用同一命令（`.github/workflows/_web.yml:27-28`）。`--frozen-lockfile` 保证解析结果与 `apps/mesa-web/pnpm-lock.yaml` 一致，不在本地漂移。

> 说明：仓库根目录**没有** `package.json`/`pnpm-workspace.yaml`；唯一的 `pnpm-workspace.yaml` 在 `apps/mesa-web/` 且只声明 `allowBuilds`（`apps/mesa-web/pnpm-workspace.yaml:1-2`）。因此本地命令统一用 `pnpm --dir apps/mesa-web ...`。CI 另用了 `pnpm --filter mesa-web`（`_web.yml:31,34`），该写法能否在根目录解析到 workspace **待确认**（附录 A-5）。

### 2.3 构建 Rust（必须 `--locked`）

```bash
cargo build --locked --workspace
```

期望：`Finished` + `dev profile`，产物在 `target/debug/`（Windows 带 `.exe`），至少包含 `mesad`、`mesa-driver-sinumerik-nck`、`mesa-nck-emulator` 等。

**为什么必须 `--locked`**：

- `Cargo.lock` 是依赖解析的唯一真值；`--locked` 禁止 Cargo 改写锁文件，保证“我构建的依赖树 = 仓库提交的依赖树”。
- CI 与两个证据入口都使用 `--locked`：`.github/workflows/_rust-canonical.yml:70,74,84`、`scripts/write-contract-evidence.py:244`、`scripts/nck-native-rest-e2e.py:46`。本地不带 `--locked` 可能静默升级依赖，产出与 CI 不同源的结论。
- 构建必须覆盖 **全部 workspace**（含驱动 bin）。`cargo test -p mesa-contract-tests` 只构建依赖包 lib，不重编驱动 bin；改过驱动后旧二进制会让故障注入**静默失效**（`AGENTS.md:21-22`）。

### 2.4 启动 `mesad`（Core）

```bash
# 必须在仓库根目录执行
./target/debug/mesad          # Windows: .\target\debug\mesad.exe
```

默认值（全部来自代码，不是经验值）：

| 项 | 默认值 | 依据 |
|---|---|---|
| HTTP 端口 | **8132** | `apps/mesad/src/main.rs:13` |
| 驱动目录 | **`drivers`**（相对当前工作目录） | `apps/mesad/src/main.rs:267` |
| 配置库 | `mesa.db`（相对当前工作目录） | `apps/mesad/src/main.rs:15,269` |
| 事件库 | 与 `--db` 同目录的 `events.db` | `apps/mesad/src/main.rs:57-60,310-316` |
| 绑定地址 | **仅 `127.0.0.1`** | `crates/core-api/src/lib.rs:3509-3510` |
| Control 面 | 默认关闭，需 `--enable-control` | `apps/mesad/src/main.rs:229-233,295-298` |

启动后期望看到（`apps/mesad/src/main.rs:318-327`）：

```text
  Mesa Core is running
  REST  : http://127.0.0.1:8132/api/v1/drivers
          http://127.0.0.1:8132/api/v1/endpoints
          http://127.0.0.1:8132/api/v1/points/latest
          http://127.0.0.1:8132/api/v1/diagnostics
  DB    : mesa.db
  Stop  : Ctrl+C
```

注意：

- **必须从仓库根启动**：`drivers` 与 `mesa.db` 都是相对路径（`main.rs:267,269`）。驱动发现读取每个子目录的 `driver.toml`（`crates/driver-manager/src/manifest.rs:3,60-110`），二进制可从 `target/{debug,release}` 回退解析（`manifest.rs:146-192`），所以构建后无需手工拷贝。
- **NCK 端点开机不会自动恢复**：`sinumerik-nck` 端点无论 `desired_running` 为何，都必须现场显式 `POST /start`（`apps/mesad/src/main.rs:184-198`；`docs/design/nck-828d-phase1-boot-recovery-policy.md:10-17`）。这是产品策略，不是故障。

### 2.5 启动前端 dev server

```bash
pnpm --dir apps/mesa-web dev
```

`dev` 脚本为 `vite --port 5173`（`apps/mesa-web/package.json:7`）。期望输出含 `Local: http://localhost:5173/`。

**IPv6 绑定问题：仍然成立（在未显式配置 host 的前提下）**。核实依据：

- `apps/mesa-web/vite.config.ts:6-23` 只配置了 `server.proxy`/`watch`，**没有** `server.host`，因此走 Vite 5 的默认值 `localhost`。
- 本机安装的 Vite 5.4.21 对 `localhost` 先用 Node 默认解析、再与 `{ verbatim: true }` 结果比较；两者不一致时按 Node 默认地址绑定（`apps/mesa-web/node_modules/vite/dist/node/chunks/dep-BK3b2jBa.js:17300-17324`，该文件未入库，仅作实现核对）。
- 在 Windows 上 Node 的解析顺序可能先返回 `::1`，于是 dev server 只监听 IPv6，`http://127.0.0.1:5173` 打不开。

**处置**：浏览器统一用 `http://localhost:5173/`；若必须固定 IPv4，可临时给 Vite 传 `--host 127.0.0.1`（Vite CLI 参数，不是仓库脚本），但不要为此修改 `vite.config.ts`（本文档不修改任何仓库配置）。

代理链路（不受上面影响）：前端所有请求都是相对路径（`apps/mesa-web/src/api.ts:4,25`），由 dev server 把 `/api` 代理到 **`http://127.0.0.1:8132`**（`apps/mesa-web/vite.config.ts:7-12`）。`mesad` 只监听 IPv4 回环，所以**页面走 localhost、代理走 127.0.0.1** 是正常组合。

### 2.6 验证入口 URL 清单

| URL | 期望 |
|---|---|
| `http://127.0.0.1:8132/healthz` | 文本 `ok`（`crates/core-api/src/lib.rs:3471`） |
| `http://127.0.0.1:8132/api/v1/drivers` | `{"drivers":[...]}`，至少含 `sinumerik-nck`（`core-api/src/lib.rs:634-636`） |
| `http://127.0.0.1:8132/api/v1/endpoints` | `{"endpoints":[...]}`，每项含 `id/device_id/driver_id/connection/desired_running/runtime`（`core-api/src/lib.rs:1570-1581`） |
| `http://127.0.0.1:8132/api/v1/points/latest` | `{"points":[...],"count":N}`（`core-api/src/lib.rs:1584-1589`） |
| `http://127.0.0.1:8132/api/v1/diagnostics` | 含 `version`、`uptime_secs`、`drivers.count`（`core-api/src/lib.rs:1595-1609`） |

---

## 3. 离线验收三条主线

### 3.0 统一前置

```bash
git status --porcelain                 # 必须为空
cargo build --locked --workspace       # 必须最新（见 §2.3）
cargo nextest --version                # 必须是 cargo-nextest 0.9.146
```

### 3.1 主线 A：合同证据（唯一可信 Evidence）

```bash
# 在仓库根目录执行
python scripts/write-contract-evidence.py
```

脚本行为（按执行顺序）：

1. 先删除旧 `target/validation/contract.json` 与上一轮日志，避免失败后遗留旧证据（`scripts/write-contract-evidence.py:220-233`）。
2. 校验 nextest 版本，必须等于 `0.9.146`，否则 `exit 6`（`:235-241`）。
3. 执行 `cargo build --locked --workspace`（`:244`）——**证据脚本自己会重编**，但手工跑合同套件前仍必须先构建（`AGENTS.md:21-22`）。
4. 名单与磁盘双向对拍（幽灵/遗漏即 `exit 3`，`:246-251`）。
5. 按 `scripts/contract_suites.py` 的 **30 个套件**逐个执行 `cargo nextest run --locked --profile ci -p mesa-contract-tests --all-features -E 'binary(=<suite>)'`（`:105-124,264-318`）。
6. 任一失败 `exit 5`，**不写证据**（`:320-323`）；全部通过才写 `contract.json`（`:325-350`）。

套件名单唯一来源：`scripts/contract_suites.py:13-55`（BASELINE 14 + EVENT 12 + LIFECYCLE 4 = **30**）。不要用 `cargo test` 代替：`stop_lifecycle`、`reconnect_backoff` 等进程级断言在 `cargo test` 的进程内并行下会假失败，nextest 的“每测试独立进程 + test-groups”才是与门禁同源的运行器（`write-contract-evidence.py:13-27`；`.config/nextest.toml:11-14,25-27`）。

**期望结果**：退出码 `0`；每个套件打印 `suite <name>: passed=N failed=0 ignored=0`；最后一行形如

```text
wrote target/validation/contract.json suites=30 tests=<N> sha=<40 位 HEAD> dirty=False
```

`<N>` 是执行到的测试总数（随仓库演进而变，**不要**把它当固定验收值；本机现存旧证据是 279，仅作格式示例）。判定只看：`suites=30`、`failed=0`、`sha` 等于当前 HEAD、`dirty=False`。

产物：

| 文件 | 用途 |
|---|---|
| `target/validation/contract.json` | 唯一可信 Evidence（`passed/failed/total/suites/suite_results/test_counts/git_sha/dirty/runner`） |
| `target/validation/nextest-run.jsonl` | 本轮完整机器可读结果（复核用） |
| `target/validation/nextest-run.stderr.log` | 人读诊断 |
| `target/validation/contract-<suite>-failed.log` | 仅失败套件生成 |

**耗时**：CI canonical job 预算 **60 分钟**（`_rust-canonical.yml:45`）。本机首次运行含全量编译，显著长于热态；仓库未记录本机 wall time，**具体分钟数待确认**，请用 `Measure-Command` 自测后记入现场记录。

### 3.2 主线 B：看门狗离线套件

```powershell
# Windows / Linux 同一条命令
pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1
```

可选参数：`-Scenario M7,M13` 只跑指定场景、`-List` 列场景、`-EvidenceRoot <dir>` 改证据根（默认 `target/validation/stop-watchdog`）、`-KeepEvidence` 保留上一轮（`tests/watchdog/README.md:30-37`）。

**期望结果**：退出码 `0`；汇总表 20 行全部 `PASS`；末尾打印

```text
通过 20 / 失败 0；机器可读汇总：target/validation/stop-watchdog/suite-summary.json
```

规模以 README 冻结值为准：**20 个场景（M0–M19）、282 项断言**（`tests/watchdog/README.md:7-8`）。每个场景会打印自己的 `断言：n/m PASS`（`run-watchdog-scenarios.ps1:1253`）。

**硬约束**：

- **不得指向 8132**。`arm` 模式只允许回环地址且端口 ≠ 8132；带测试注入参数时同样只允许回环且非 8132（`tests/watchdog/README.md:268-275`；B1 实现说明见 `tests/watchdog/README.md:45-64`，M13 子用例见 `run-watchdog-scenarios.ps1:691`）。套件自己用假端点桩（`tests/watchdog/stub_endpoint.py`），**不启动 `mesad`、不连设备**（`_watchdog.yml:7`）。
- 不要为了让断言通过而放宽阈值（`tests/watchdog/README.md:382-385`）。

**耗时**：全量约 **14–16 分钟**（`tests/watchdog/README.md:373-374`）；CI 单 job 预算 **25 分钟**（`_watchdog.yml:33`）。

### 3.3 主线 C：NCK 端到端（模拟器 + 真实 `mesad`）

先构建三个被测二进制（脚本前置检查缺一即 `exit 2`）：

```bash
cargo build --locked --bins -p mesad -p mesa-nck-emulator -p mesa-driver-sinumerik-nck
```

再运行（Windows/Linux 同命令）：

```bash
python scripts/nck-native-rest-e2e.py --evidence-dir target/validation/nck-rest-e2e
```

脚本行为要点：

- 启动**真实 TCP 模拟器**（`--port 0`，OS 原子分配）与**真实 `mesad`**（临时 DB、动态 HTTP 端口），全部在 `127.0.0.1` 回环（`scripts/nck-native-rest-e2e.py:227-301`）。
- **绝不占用 8132**：`PRODUCTION_PORT = 8132`，`free_port()` 取号时排除（`:69,160-172`）；绑定冲突时换端口重试，最多 3 次（`:268-297`）。
- 启动前硬检查三个二进制**不比源码旧**，否则 `exit 2` 并给出重编命令（`:189-216`）。
- 成功路径**恒为 13 项断言**（`:323`；CI 步骤名同：`_e2e.yml:107`）。

**期望结果**：退出码 `0`；末尾逐条 `PASS` 后打印

```text
13/13 PASS
```

证据目录（`--evidence-dir`）含 `summary.json`（`total/passed/failed/results/duration_s`）与 `mesad.*.log`、`emulator.stderr.log`（`write_evidence`，`:556-581`）。

**耗时**：CI job 预算 **20 分钟**（`_e2e.yml:47`）。脚本内部预算：`mesad` 就绪 60 s、模拟器就绪 20 s、采点等待 30 s（`:83-85`）。本机典型耗时未记录，**待确认**。

### 3.4 三条主线汇总

| 主线 | 入口 | 期望 | 预算/耗时 |
|---|---|---|---|
| 合同证据 | `python scripts/write-contract-evidence.py` | 30/30 套件、`failed=0`、`dirty=False`、`sha=HEAD` | CI 预算 60 min；本机待确认 |
| 看门狗离线 | `pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1` | 20 场景 / 282 断言全过，exit 0 | 实测 14–16 min；预算 25 min |
| NCK 端到端 | `python scripts/nck-native-rest-e2e.py` | 13/13 PASS，exit 0 | 预算 20 min；本机待确认 |

---

## 4. NCK 端点离线配置示例（模拟器）

### 4.1 启动模拟器

```bash
./target/debug/mesa-nck-emulator --port 1102 --scenario happy --element-size 8
```

期望 stderr：

```text
nck-emulator listening on 127.0.0.1:1102
MESA_NCK_EMULATOR_READY=127.0.0.1:1102
```

CLI 与场景见 `tools/nck-emulator/src/main.rs:19-30,40-76`；READY 前缀冻结为 `MESA_NCK_EMULATOR_READY=`（`:38`）。模拟器默认只绑回环（`:143`），`--port 0` 时由 OS 分配（`:192-195`）。

### 4.2 启动 `mesad`

```bash
./target/debug/mesad
```

### 4.3 建设备与端点（PowerShell，两个平台通用）

```powershell
$base = 'http://127.0.0.1:8132'

Invoke-RestMethod -Method Post -Uri "$base/api/v1/devices" `
  -ContentType 'application/json' `
  -Body '{"id":"emu-dev","name":"NCK Emulator Device"}'

$endpoint = @{
  id        = 'emu-nck'
  name      = 'NCK Emulator'
  device_id = 'emu-dev'
  driver_id = 'sinumerik-nck'
  connection = @{
    host           = '127.0.0.1'
    port           = 1102
    family         = '828d'
    local_tsap     = 256
    remote_tsap    = 258
    timeout_ms     = 3000
    max_read_items = 8
  }
} | ConvertTo-Json -Depth 5

Invoke-RestMethod -Method Post -Uri "$base/api/v1/endpoints" `
  -ContentType 'application/json' -Body $endpoint
```

连接字段集合以驱动为准：`host/port/family/local_tsap/remote_tsap/timeout_ms/pdu_length/max_read_items`，出现其它字段即 `BAD_CONFIG`（`drivers/sinumerik-nck/src/config.rs:64-80`）。`driver_id` 必须是 `sinumerik-nck`（`drivers/sinumerik-nck/driver.toml:4`）。

### 4.4 最小 4 点位任务（可直接复制）

把下面内容保存为 `target/validation/nck-emulator-task.json`（`target/` 已被 gitignore，不污染工作树）：

```json
{
  "tasks": [
    {
      "id": "t1",
      "schedule": { "mode": "poll", "interval_ms": 1000 },
      "binding": {
        "kind": "nck.points.v1",
        "config": {
          "points": [
            { "kind": "nck_native", "point_key": "native.C.1.18.42.1",
              "parameters": { "area": "C", "area_no": 1, "module": 18, "column": 42, "line": 1,
                              "count": 1, "unit_mode": "current", "decode": "F64_BE", "transport_size": "0x09" } },
            { "kind": "nck_native", "point_key": "native.C.1.18.43.1",
              "parameters": { "area": "C", "area_no": 1, "module": 18, "column": 43, "line": 1,
                              "count": 1, "unit_mode": "current", "decode": "F64_BE", "transport_size": "0x09" } },
            { "kind": "nck_native", "point_key": "native.C.1.18.44.1",
              "parameters": { "area": "C", "area_no": 1, "module": 18, "column": 44, "line": 1,
                              "count": 1, "unit_mode": "current", "decode": "F64_BE", "transport_size": "0x09" } },
            { "kind": "nck_native", "point_key": "native.C.1.18.45.1",
              "parameters": { "area": "C", "area_no": 1, "module": 18, "column": 45, "line": 1,
                              "count": 1, "unit_mode": "current", "decode": "F64_BE", "transport_size": "0x09" } }
          ]
        }
      }
    }
  ]
}
```

结构对照 `target/validation/t2b-task.json:1-76`（真实任务，4 点，`nck.points.v1`），但**地址换成模拟器回环用值**，不代表任何真实机床语义；`decode/count/element-size` 三者必须匹配（模拟器 `--element-size 8` 对应 `F64_BE` + `count=1`）。

`point_key` 不是自由文本：`identity_mode=derived`（缺省）下必须逐字等于地址推导出的 `native.<area>.<area_no>.<module>.<column>.<line>`，否则驱动报 `POINT_KEY_MISMATCH`（`drivers/sinumerik-nck/src/definition.rs:1177-1190`；`scripts/nck-native-rest-e2e.py:78-80,344-358`）。

### 4.5 提交任务、启动、验证

```powershell
$base = 'http://127.0.0.1:8132'

Invoke-RestMethod -Method Put -Uri "$base/api/v1/tasks/emu-nck" `
  -ContentType 'application/json' `
  -InFile 'target/validation/nck-emulator-task.json'

Invoke-RestMethod -Method Post -Uri "$base/api/v1/endpoints/emu-nck/start"
Start-Sleep -Seconds 2

Invoke-RestMethod "$base/api/v1/points/latest" | ConvertTo-Json -Depth 6
```

`PUT /api/v1/tasks/{endpoint_id}` 同时接受 `{"tasks":[...]}` 与裸数组 `[...]`（`crates/core-api/src/lib.rs:2371-2402`）。

**期望**：`start` 返回 `{"started":"emu-nck"}`；`points/latest` 的 `points[]` 出现 4 个 `point_key`，`quality` 为 `GOOD`。

### 4.5.1 `mesa.resources.v1` 的现状（重要，勿混）

- **对 NCK 驱动：已废弃**。`sinumerik-nck` 的 `configure` 只接受 `nck.points.v1`；其它 kind 一律 `UNSUPPORTED_BINDING`，且**不做隐式迁移**（`drivers/sinumerik-nck/src/lib.rs:643-650,680-688`）。
- **对 Core：仍是合法的通用任务 kind**。Core 的数据任务 kind 白名单是 `[mesa.resources.v1, nck.points.v1]`（`crates/core-api/src/binding_gate.rs:30`），因为 S7/OPC UA 等通用驱动仍使用它（`crates/core-types/src/resource.rs:229`）。
- 因此“旧配置被拒”**发生在驱动 Configure / Start 阶段**，保存可能先通过（`scripts/nck-native-rest-e2e.py:516-551` 专门覆盖这一负例）。看到 `UNSUPPORTED_BINDING` 时，先查任务 `binding.kind`，不要怀疑设备。
- `nck_variable`（语义点位）的 `point_key` **必须**由驱动的 `identity-preview` 计算，客户端不得自行拼接（`scripts/nck-native-rest-e2e.py:14-16,445-462`）；本文示例只用 `nck_native`，就是为了保证可复制。

---

## 5. 常见失败与排查

| 现象 | 原因 | 处置 |
|---|---|---|
| `contract.json` 看起来是旧的 | 本轮脚本失败或根本没跑；旧文件未被删除只可能是**不是本脚本写的**。核实当时本机现存文件 `git_sha=609d204...`，而 HEAD 是 `bf4d3f6...`，即典型陈旧证据 | 以 `git_sha == git rev-parse HEAD`、`dirty=false`、`generated_at_ns` 三者判断新鲜度；重跑 `python scripts/write-contract-evidence.py`（它启动即删旧文件，`:220-233`） |
| `contract.json` 里 `dirty=true` | 工作树有未提交/未跟踪文件（脚本把 untracked 也算进去，`:68-76`） | 提交或清理后再跑；**不要手改** `dirty` 字段。注意 `target/` 被 gitignore，正常不触发 |
| 故障注入“没反应”、测试莫名通过 | 驱动/夹具改了但二进制没重编，子进程跑的是旧 bin（`AGENTS.md:21-22`） | 先 `cargo build --locked --workspace`；证据脚本自己会构建（`:244`），手工跑 contract 套件时不会 |
| `mesad` 启动即退出 / `Address already in use` | 8132 被占（本机已有生产实例或上次未退干净） | 先停占用进程；`mesad` 只绑 `127.0.0.1:8132`（`core-api/src/lib.rs:3509`），不会自动换端口。诊断端口占用用系统工具（`Get-NetTCPConnection -LocalPort 8132` / `ss -ltnp`），仓库未提供脚本 |
| E2E 偶发端口冲突 | 动态端口存在 TOCTOU；脚本会换端口重试 3 次（`nck-native-rest-e2e.py:268-297`） | 3 次都失败再查：端口耗尽/防火墙/其它进程；不要改脚本硬编码端口 |
| 保存任务成功，Start 后端点报 `UNSUPPORTED_BINDING` | 任务用了旧 `mesa.resources.v1`（NCK 只接受 `nck.points.v1`） | 改 `binding.kind` 为 `nck.points.v1` 并按信封重构 `config.points`（§4.5.1） |
| 浏览器打开 `127.0.0.1:5173` 连不上 | Vite 按 `localhost` 解析可能只绑了 `::1`（§2.5） | 改用 `http://localhost:5173/`；或临时 `--host 127.0.0.1` |
| 前端页面能开但接口全 404/跨域 | 没走 dev server（直接开 `dist`）；Core 无 CORS 支持，前端依赖 `/api` 代理（`vite.config.ts:7-12`；`api.ts:4,25`） | 用 `pnpm --dir apps/mesa-web dev`；确认 `mesad` 在 8132；不要把 API 写成绝对地址 |
| 看门狗套件 `exit 40` | **授权边界拒绝，不是设备故障**：指向非回环/8132、缺授权、凭证不符、G1/G2 摘要不符等（`tests/watchdog/README.md:291-317`） | 按 `summary.json` 的 `refused` 标签定位；`exit 40` 时**未发 Start、未进入 FIRED**，不要按设备故障处置 |
| 证据脚本 `exit 6` | nextest 版本 ≠ 0.9.146（`:235-241`） | 安装精确版本，不要绕过 |
| `mesad` 重启后 NCK 端点不跑 | 产品策略：NCK 永不自动恢复（`docs/design/nck-828d-phase1-boot-recovery-policy.md:10-17`） | 显式 `POST /api/v1/endpoints/{id}/start` |

---

## 6. 已知限制与不可承诺项（不得淡化）

1. **机型身份软件无法证明**：`family`/`model` 恒为 `None`，`NCK_ANCHOR_PENDING` 恒在（`drivers/sinumerik-nck/src/probe.rs:26,49-64`；`drivers/sinumerik-nck/GATE.md:91-92`；`docs/design/nck-828d-phase1-p0-points.md:137`）。只能靠现场铭牌/序列号/HMI/工位核对。
2. **真机 Gate 未通过**：NCK 状态仍是 `experimental`；真机门（connect/probe/topology/轴值/R 参数/断线重连等）未完成，828D 需独立 Gate，**不得由 840D 推断**（`GATE.md:80-95`）。
3. **P0 证据不足**：10 个 P0 点位族里只有 3 个有单独真机复验（A2/B1/D1），7 个只有 238 点跑批的历史 GOOD，9 个“目录无定义”，1 个被 80 BAD 阻塞（`docs/design/nck-828d-phase1-p0-points.md:15-20,110,118-122`）。
4. **离线通过 ≠ 硬件验证**：模拟器与被测驱动共享同一 `NckFixture`，只能证明进程/传输层，**不能证明 codec 正确性**（`tools/nck-emulator/src/main.rs:11-17`）；E2E 全绿也不构成“828D 已验收/NCK 可生产”（`docs/validation/828d-phase1-first-limited-sampling-application.md:613`）。看门狗离线套件通过也只证明校验逻辑有效，**不代表现场凭证已获授权**（`tests/watchdog/README.md:170-172`）。
5. **238 点跑批的 80 BAD 根因未定**：80 = `C.1.21.1.301..426` 的 74 个 + `V.{1,3,4}.26.{27,31}` 的 6 个；项级码在断线时被 `COMMUNICATION_LOST` 覆盖，**不得**断言“地址不存在/设备不支持”（`p0-points.md:195-199,214-215`；`docs/design/quality-code-observability-investigation.md:142-146`；`docs/design/pr-d1-quality-code-visibility-design.md:581-583`）。
6. **S7 驱动不产生设备码**：S7 的项级失败只 `warn!` 并返回 `None`，Core 兜底写成 `DEVICE_ERROR`；该标记是“驱动未提供数字码”，**不能**据此指认本地校验失败或设备错误（`drivers/s7/src/client.rs:213-221`；`crates/driver-manager/src/snapshot.rs:387-393`；`docs/design/pr-d1-quality-code-visibility-design.md:152`）。
7. **Stop 不是即时的**：典型最坏 ≈75–80 s，且尾部无严格上界；`T0+30 s` 只是 Stop 的**调度目标**，不是“物理已停”（`docs/validation/828d-phase1-first-limited-sampling-application.md:302`）。不要把它写成安全承诺。
8. **NCK 开机不自动恢复是产品策略**，不是通用生命周期语义；其它驱动行为不变（`docs/design/nck-828d-phase1-boot-recovery-policy.md:4-6,62-67`）。
9. 一期不承诺报警/程序注释/轴负载/轴温度/急停/驱动使能/刀补等能力（目录无定义，需新增资源或产品决策，`p0-points.md:124-137`）。

---

## 7. 离线验收检查清单（逐项打勾）

| # | 检查项 | 判定依据 | 完成 |
|---|---|---|---|
| 1 | HEAD 与基线一致 | `git rev-parse HEAD` = `bf4d3f6...` | ☐ |
| 2 | 工作树干净 | `git status --porcelain` 为空 | ☐ |
| 3 | Rust 1.95.0 | `rustc --version` | ☐ |
| 4 | Node 22.18.0 / pnpm 10.28.0 | `node --version` / `pnpm --version` | ☐ |
| 5 | Python 3.x / pwsh 7+ | `python --version` / `$PSVersionTable.PSVersion` | ☐ |
| 6 | cargo-nextest 0.9.146 | `cargo nextest --version` | ☐ |
| 7 | `cargo build --locked --workspace` 成功 | 输出含 `Finished` 与 `dev profile` | ☐ |
| 8 | `mesad` 起在 8132 且仅回环 | 启动 banner；`/healthz` 返回 `ok` | ☐ |
| 9 | 驱动清单含 `sinumerik-nck` | `GET /api/v1/drivers` | ☐ |
| 10 | 前端 dev server 可用 | `http://localhost:5173/` 打开 | ☐ |
| 11 | 前端能取到后端数据 | 页面列表非空或 `/api/v1/endpoints` 有响应 | ☐ |
| 12 | 合同证据 30/30 | `contract.json`：`passed=30 failed=0` | ☐ |
| 13 | 证据与 HEAD 同源 | `git_sha` = 当前 HEAD | ☐ |
| 14 | 证据工作树干净 | `dirty=false` | ☐ |
| 15 | 证据运行器同源 | `runner.version=0.9.146`、`profile=ci` | ☐ |
| 16 | 看门狗 20 场景全过 | 末尾 `通过 20 / 失败 0` | ☐ |
| 17 | 看门狗断言数 282 | 各场景 `断言：n/m PASS` 合计 282 | ☐ |
| 18 | 看门狗未触碰 8132 | 证据里无 8132 请求；`suite-summary.json` 存在 | ☐ |
| 19 | E2E 13/13 | 末尾 `13/13 PASS` | ☐ |
| 20 | E2E 未占用 8132 | 脚本日志端口 ≠ 8132；`summary.json` `failed=0` | ☐ |
| 21 | 已知限制已阅读并接受 | §6 九条逐条确认 | ☐ |
| 22 | 未把离线结论外推为硬件验收 | 记录中无“828D 已验收/可生产”表述 | ☐ |

---

## 附录 A：本次核实发现的文档与代码不一致

以下均为**文档过期**，处置原则：**以代码为准**。

| # | 文档说法 | 代码/实际 | 处置 |
|---|---|---|---|
| A-1 | `GATE.md:41-42` 称 E2E 为“12 项断言” | 脚本成功路径为 **13** 项（`scripts/nck-native-rest-e2e.py:323`）；CI 步骤名也是 13（`_e2e.yml:107`） | 以 13 为准，GATE 该行待更新 |
| A-2 | `_watchdog.yml:61` 步骤名、`ci.yml:15` 注释写“M0–M18” | README 冻结为 **20 场景（M0–M19）**（`tests/watchdog/README.md:7-8`）；脚本缺省跑全部（`run-watchdog-scenarios.ps1:56`） | 以 README/脚本为准，workflow 文案待更新 |
| A-3 | 质量调查/D1 设计引用 `snapshot.rs:318-324` 为 `DEVICE_ERROR` 兜底 | 当前兜底在 `crates/driver-manager/src/snapshot.rs:387-393`（行号漂移，语义未变） | 引用时用现行行号 |
| A-4 | `README.md:10-12` 快速开始：先 `cargo build --workspace`，再跑 `./target/release/mesad` | 该构建命令只产出 `target/debug/`；直接跑 release 会找不到文件或用到陈旧二进制（`AGENTS.md:17-18` 已警告） | 按 §2.3/§2.4 用 debug；要 release 必须先 `cargo build --release --workspace` |
| A-5 | `_web.yml:31,34` 在仓库根用 `pnpm --filter mesa-web ...` | 仓库根无 `package.json`/`pnpm-workspace.yaml`；唯一 workspace 文件在 `apps/mesa-web/` 且未声明 `packages:` | **待确认**（未在本次运行 CI）；本地统一用 `pnpm --dir apps/mesa-web ...` |
| A-6 | 编写要求称 planner flags 为“rust/web/watchdog/e2e” | `tools/ci/plan.py:1-20` 实际为 `quality/canonical/platform/stress/perf/web/watchdog/e2e`，`PLAN_VERSION=5`（`:53`），`ci.yml:191` 强校验版本 5 | 以 `plan.py` 为准 |
| A-7 | 编写要求提到“引用盘点 D 类” | 全仓检索无“D 类/D类”分类；实际证据分档是 `✅/⚠️/❓/❌`（`docs/design/nck-828d-phase1-p0-points.md:24-35`） | **待确认**指代；本文按实际四档与“真机门未过”如实汇总 |
