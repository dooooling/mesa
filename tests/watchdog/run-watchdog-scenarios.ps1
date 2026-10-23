<#
.SYNOPSIS
  stop-watchdog 离线场景驱动（M0–M16）—— 一条命令跑完全部离线故障模拟并给出断言汇总。

.DESCRIPTION
  每个场景：起本地假端点桩（stub_endpoint.py）→ 以独立 pwsh 进程跑 scripts/stop-watchdog.ps1
  → 收证据 → 跑断言 → 汇总。**不连真实设备、不启动 mesad、不依赖 8132、不依赖 Rust 代码。**

  断言不是"跑通就算过"：每一项都对应任务书里的一条硬性约束（调度/采集/收敛三条轴分开、
  ARMED 零副作用、授权门四项凭证必须**校验**、HostChecks 三态、工作流异常不静默……），
  任何一条失败即整体退出码 1。

  场景一览（详细断言见下面的 switch）：
    M0  正常路径（Start → T0+30 s Stop → 停干净）
    M1  观测阻塞 36 s（Stop 仍准时；采集判 INVALID）
    M2  Start 超时（20 s > 10 s）
    M3  端点无响应（不可达地址）
    M4  Stop 自身挂起 15 s（观测不被 Stop 阻塞）
    M5  迟到 Start 竞态（Stop 后翻回 RUNNING）
    M6  首次通信失败即停（0x8201/early eof）
    M7  ARMED 误启动防护（等待 >1.5 s 也绝不 Start）
    M8  OBS 心跳失效（提前 Stop + 采集 INVALID + 截止调度仍工作）
    M9  HostChecks 查询失败（判"无法确定"，不得判停净、不得 exit 0）
    M10 watchdog 工作线程异常退出（不静默继续，MON 接管截止调度）
    M11 授权门正向：四项凭证齐全且校验通过 ⇒ 放行并正常执行
    M12 授权门负向：缺一项凭证 / 任务配置与只读 GET 不一致 ⇒ exit 40 且不发 Start
    M13 测试注入参数的离线边界（B1 负向）：非回环地址 / 生产端口 8132 + 注入参数
        ⇒ exit 40、网络动作数 = 0、假端点未收到任何请求（更不会有 Start/Stop）
    M14 G7 部署身份不可验证（B2 fail-closed）：只给 git 版本标识 / 二进制缺失 / 清单缺失或
        摘要不符 ⇒ 一律 exit 40 且不进入 FIRED（**不得**用 git HEAD 降级通过）
    M15 完整任务配置不匹配（B3 负向）：连接配置（PDU）/ 点位参数 / 配置指纹任一不一致
        ⇒ exit 40 且不发 Start
    M16 完整任务配置匹配（B3 正向）：连接配置 + 全部点位 Key/参数 + 指纹全部一致 ⇒ 放行并正常执行
    M17 G1 负向：构建清单**自身**未被独立验证（缺 -ExpectedManifestSha256 / 清单字节被改 /
        清单+二进制一起被换且互相自洽）⇒ exit 40、网络动作数 0、不发 Start
    M18 G2 负向：获批任务配置**文件自身**未被独立验证（缺 -ExpectedApprovedTaskConfigSha256 /
        文件字节被改而 tasks 指纹不变 / 改文件后用改后文件自身摘要）⇒ exit 40、不发 Start

  关于 G7：本驱动**不再**用 git HEAD 充当 G7 凭证（那只能证明"与仓库版本匹配"，不能证明部署物
  身份）。它离线构造两份 fixture 并交给被测脚本实算比对：
    · `_fixtures/mesad-fixture.bin`：固定内容的"部署二进制"（不启动 mesad）；
    · `_fixtures/mesad-build-manifest.json`：受信构建清单（含该文件的 sha256）。
  获批任务配置（`_fixtures/approved-task-config.json`）与桩共用同一份 tasks 定义
  （`_fixtures/tasks.json`），指纹由 python 侧**独立实现**算出，与 PowerShell 侧互为交叉核对。

  关于 G1/G2（"被批准对象自身必须与签署值比对"）：正向用例把本轮 fixture 的**真实摘要**当作
  "签署包里批准的值"传入（清单自身 ⇒ -ExpectedManifestSha256；获批配置文件自身 ⇒
  -ExpectedApprovedTaskConfigSha256）；M17/M18 则构造"缺失 / 字节被改 / 清单与二进制一起被换"
  三类形态，证明这两层校验确实拦得住 —— 尤其证明"tasks 指纹不变也挡不住 connection 篡改"。

.PARAMETER Scenario
  只跑指定场景，例如 -Scenario M7,M13。缺省 = M0–M16 全部。

.PARAMETER EvidenceRoot
  证据根目录，缺省 target/validation/stop-watchdog（已被 .gitignore 忽略）。

.PARAMETER KeepEvidence
  保留上一轮证据目录（默认每次重建，避免旧证据冒充新结论）。

.PARAMETER PythonExe
  指定 python 解释器（缺省自动探测 python3 / python / $env:PYTHON）。

.PARAMETER DeployBinaryPath
  指定"部署二进制"（G7 实算比对目标）。缺省用本轮生成的 `_fixtures/mesad-fixture.bin`。
  指向真实 `target/release/mesad` 时同样可用（仍不会启动它）。

.PARAMETER List
  只列出场景，不执行。

.EXAMPLE
  pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1

.EXAMPLE
  pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1 -Scenario M7,M11,M13,M14,M15,M16

.NOTES
  Windows / Linux 均可运行（pwsh 7+ 与 python3）。CI 用法见 README.md。
#>
[CmdletBinding()]
param(
    [string[]]$Scenario = @(),
    [string]$EvidenceRoot,
    [switch]$KeepEvidence,
    [string]$PythonExe,
    [string]$DeployBinaryPath,
    [switch]$List,
    [int]$StubReadyTimeoutSec = 20
)

$ErrorActionPreference = 'Stop'

$here = $PSScriptRoot
$repo = Split-Path -Parent (Split-Path -Parent $here)
$watchdog = Join-Path $repo 'scripts/stop-watchdog.ps1'
$stubPy = Join-Path $here 'stub_endpoint.py'
if (-not $EvidenceRoot) { $EvidenceRoot = Join-Path $repo 'target/validation/stop-watchdog' }
$endpoint = 'lx-nck-tags-190-connection'

# 任务配置核验的期望值（与桩的默认 --task-revision / --point-count 对齐）
$expectedRevision = 'rev13'
$expectedPoints = 12
$expectedTaskCount = 3
$pointsPerTask = 4

# 连接配置期望值（B3：host / port / TSAP / PDU）。桩按同一组参数暴露，获批配置按同一组参数声明。
$conn = [ordered]@{ host = '127.0.0.1'; port = 18099; local_tsap = 256; remote_tsap = 256; pdu_length = 240 }
$connStubCfg = @('--device-host', $conn.host, '--device-port', [string]$conn.port,
    '--local-tsap', [string]$conn.local_tsap, '--remote-tsap', [string]$conn.remote_tsap,
    '--pdu-length', [string]$conn.pdu_length)

# G6 签署时间（可解析且不在未来）
$g6SignedAt = (Get-Date).AddMinutes(-15).ToString('o')

# 用"当前正在运行的 pwsh"跑被测脚本，保证子进程与本驱动同一版本、同一平台。
$pwshExe = $null
try {
    $p = Get-Process -Id $PID -ErrorAction Stop
    if ($p.Path) { $pwshExe = $p.Path }
}
catch { }
if (-not $pwshExe) {
    $pwshExe = Join-Path $PSHOME $(if ($IsWindows) { 'pwsh.exe' } else { 'pwsh' })
}

# ── 场景定义：stub = 假端点桩参数；wd = watchdog 额外参数；mode = arm/fire ────────
$commonStubCfg = @('--task-revision', $expectedRevision, '--point-count', [string]$expectedPoints,
    '--task-count', [string]$expectedTaskCount, '--points-per-task', [string]$pointsPerTask) + $connStubCfg
$SCEN = [ordered]@{
    M0  = @{ title = '正常路径'; stub = @('--port', '18080') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18080 }
    M1  = @{ title = '观测阻塞 36s'; stub = @('--port', '18081', '--get-delay-running', '36') + $commonStubCfg; wd = @('-ObserveTimeoutSec', '45', '-ObserveStallSec', '0'); mode = 'fire'; port = 18081 }
    M2  = @{ title = 'Start 超时 20s'; stub = @('--port', '18082', '--start-delay', '20') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18082 }
    M3  = @{ title = '端点运行中失联（Stop 发不出去）'; stub = @('--port', '18083', '--shutdown-after', '4') + $commonStubCfg; wd = @('-ObserveMaxSec', '45', '-ObserveTimeoutSec', '2'); mode = 'fire'; port = 18083 }
    M4  = @{ title = 'Stop 自身挂起 15s'; stub = @('--port', '18084', '--stop-delay', '15') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18084 }
    M5  = @{ title = '迟到 Start 竞态'; stub = @('--port', '18085', '--race-after-stop', '5') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18085 }
    M6  = @{ title = '首次通信失败即停'; stub = @('--port', '18086', '--fail-after', '6') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18086 }
    M7  = @{ title = 'ARMED 误启动防护'; stub = @('--port', '18087') + $commonStubCfg; wd = @(); mode = 'arm'; armHold = 3; port = 18087 }
    # M8 用 --get-delay-running：Start 之后（desired_running=true）的 GET 全部挂 60 s —— 观测窗口
    # 完全无响应，心跳必然失效。改用该旋钮是为了让**凭证核验阶段**（FIRED 之前的只读 GET，此时
    # desired=false）仍能完成：B3 之后凭证核验必须读端点 connection，若全量 GET 都挂住，
    # 那轮会在授权门就被正确拒绝（40），M8 想验证的"FIRED 之后心跳失效"就永远走不到。
    # -ObserveMaxSec 从 40 放宽到 45 的**原因**（不是放宽断言，是补足稳定窗口所需时长）：
    # 观测流比 T0 早 1.5 s 启动，截止 Stop 在 T0+30 s，而"停干净"⑥要求 Stop 之后连续 10 s 无新
    # epoch/无 RECONNECTING —— 需要观测到 T0+40 s，即观测流需存活 ≥41.5 s；40 s 会差 1.5 s
    # 判成 not_converged（那是窗口被截断，不是没停稳）。断言本身（exit 2 / INVALID_OBSERVATION_GAP /
    # stall_fired / OBS_STALL ≤ T0+8 s / 截止 Stop 准时）一条都没改。
    M8  = @{ title = 'OBS 心跳失效'; stub = @('--port', '18088', '--get-delay-running', '60') + $commonStubCfg; wd = @('-ObserveTimeoutSec', '5', '-ObserveMaxSec', '45'); mode = 'fire'; port = 18088 }
    M9  = @{ title = 'HostChecks 查询失败'; stub = @('--port', '18089') + $commonStubCfg; wd = @('-TestHostProbeError', 'both'); mode = 'fire'; port = 18089 }
    M10 = @{ title = '工作线程异常退出'; stub = @('--port', '18090') + $commonStubCfg; wd = @('-TestAbortFlow', 'timer', '-TestAbortAfterSec', '3'); mode = 'fire'; port = 18090 }
    M11 = @{ title = '授权门正向（四项凭证校验通过）'; stub = @('--port', '18091') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18091 }
    M12 = @{ title = '授权门负向（缺凭证/任务配置不匹配）'; stub = @('--port', '18092') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18092; phases = $true }
    # M13：桩绑 0.0.0.0，好让"非回环字面量但确实是本机"的子用例**在守卫失效时真能收到请求**
    # （否则"桩没收到"就成了空断言）。8132 子用例不启桩——生产端口必须保持空闲。
    M13 = @{ title = '测试注入参数离线边界（B1 负向）'; stub = @('--port', '18093', '--bind', '0.0.0.0') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18093; phases = 'm13' }
    M14 = @{ title = 'G7 部署身份不可验证（B2 fail-closed）'; stub = @('--port', '18094') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18094; phases = 'm14' }
    M15 = @{ title = '完整任务配置不匹配（B3 负向）'; stub = @('--port', '18095') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18095; phases = 'm15' }
    M16 = @{ title = '完整任务配置匹配（B3 正向）'; stub = @('--port', '18096') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18096; g7 = 'manifest' }
    # M17/M18：G1/G2 —— "被批准对象自身必须与签署值比对"。
    # 桩仍然起在回环端口上：这样"桩收到 0 条请求"才是有效断言（守卫/校验失效时请求会真的到达）。
    M17 = @{ title = 'G1 负向：构建清单自身未被独立验证'; stub = @('--port', '18097') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18097; phases = 'm17' }
    M18 = @{ title = 'G2 负向：获批配置文件自身未被独立验证'; stub = @('--port', '18098') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18098; phases = 'm18' }
}

# -File 传参时 "M0,M1" 会作为单个字符串进来，这里统一按逗号拆分（Windows/Linux 行为一致）。
$targets = @()
if ($Scenario.Count -gt 0) {
    foreach ($s in $Scenario) {
        foreach ($part in ([string]$s).Split(',')) {
            $t = $part.Trim().ToUpperInvariant()
            if ($t) { $targets += $t }
        }
    }
}
else {
    $targets = @($SCEN.Keys)
}
foreach ($t in $targets) {
    if (-not $SCEN.Contains($t)) { throw "未知场景：$t（可选：$($SCEN.Keys -join ', ')）" }
}

if ($List) {
    foreach ($k in $SCEN.Keys) { Write-Host ("{0,-4} {1}" -f $k, $SCEN[$k].title) }
    return
}

# ── 解释器 / 前置检查 ─────────────────────────────────────────────────────────────
function Resolve-PythonExe {
    param([string]$Explicit)
    $cands = @()
    if ($Explicit) { $cands += $Explicit }
    if ($env:PYTHON) { $cands += $env:PYTHON }
    $cands += @('python3', 'python')
    foreach ($c in $cands) {
        try {
            $v = & $c --version 2>&1
            if ($LASTEXITCODE -eq 0) { return $c }
        }
        catch { }
    }
    return $null
}

if (-not (Test-Path $watchdog)) { throw "找不到被测脚本：$watchdog" }
if (-not (Test-Path $stubPy)) { throw "找不到假端点桩：$stubPy" }
$python = Resolve-PythonExe -Explicit $PythonExe
if (-not $python -and ($targets | Where-Object { $SCEN[$_].stub.Count -gt 0 })) {
    throw '未找到 python3/python：假端点桩无法启动（可用 -PythonExe 指定）'
}

New-Item -ItemType Directory -Force -Path $EvidenceRoot | Out-Null
$EvidenceRoot = (Resolve-Path -LiteralPath $EvidenceRoot).Path

# ── 凭证 fixture（B2 / B3）：部署二进制 + 受信清单 + 获批任务配置 + 指纹 ──────────────
# 设计：G7 不再接受 git HEAD（只能证明"与仓库版本匹配"）。这里离线构造一份"部署二进制"
# （固定内容，**不启动 mesad**）与一份受信构建清单，交给被测脚本实算 sha256 比对；
# 获批任务配置与桩共用同一份 tasks 定义（python 侧导出），指纹由 python 侧**独立实现**算出，
# 与 PowerShell 侧实现互为交叉核对。
$fixtureRoot = Join-Path $EvidenceRoot '_fixtures'
New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null

$deployBin = if ($DeployBinaryPath) { (Resolve-Path -LiteralPath $DeployBinaryPath).Path } else { Join-Path $fixtureRoot 'mesad-fixture.bin' }
if (-not $DeployBinaryPath) {
    # 固定内容（含说明文本）——"离线测试用的假部署二进制"，不是可执行程序，也不会被启动。
    $bytes = [System.Text.Encoding]::UTF8.GetBytes("mesa offline watchdog fixture: fake mesad binary`nrevision=$expectedRevision`n")
    [System.IO.File]::WriteAllBytes($deployBin, $bytes)
}
$deploySha = (Get-FileHash -Algorithm SHA256 -LiteralPath $deployBin).Hash.ToLowerInvariant()

$manifestPath = Join-Path $fixtureRoot 'mesad-build-manifest.json'
$manifestDoc = [ordered]@{
    schema   = 'mesa.stop-watchdog.build-manifest.v1'
    note     = '离线测试用的受信构建清单（fixture）：只含摘要，不含真实构建信息'
    binary   = (Split-Path -Leaf $deployBin)
    sha256   = $deploySha
}
[System.IO.File]::WriteAllText($manifestPath, ($manifestDoc | ConvertTo-Json -Depth 6), [System.Text.UTF8Encoding]::new($false))

$tasksFile = Join-Path $fixtureRoot 'tasks.json'
if ($python) {
    $null = & $python $stubPy --emit-tasks $tasksFile --task-count $expectedTaskCount --points-per-task $pointsPerTask 2>&1
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $tasksFile)) {
        throw "导出 tasks fixture 失败（python=$python）：$tasksFile"
    }
}
else {
    throw '未找到 python3/python：无法导出 tasks fixture（B3 需要完整点位清单）'
}
$tasksFixture = Get-Content -LiteralPath $tasksFile -Raw | ConvertFrom-Json

# 获批任务配置：连接配置 + 完整点位清单（与桩完全一致）
$approvedPath = Join-Path $fixtureRoot 'approved-task-config.json'
$approvedDoc = [ordered]@{
    schema      = 'mesa.stop-watchdog.approved-task-config.v1'
    endpoint_id = $endpoint
    revision    = $expectedRevision
    connection  = [ordered]@{
        host        = $conn.host
        port        = $conn.port
        local_tsap  = $conn.local_tsap
        remote_tsap = $conn.remote_tsap
        pdu_length  = $conn.pdu_length
    }
    tasks       = $tasksFixture
}
[System.IO.File]::WriteAllText($approvedPath, ($approvedDoc | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
# G2：获批配置**文件自身**的摘要。正向用例把它当作"签署包里批准的值"传给被测脚本；
# 负向用例（M18）故意传"缺失"或"改动前的旧值"，以证明文件字节被改时即使指纹不变也必须拒绝。
$approvedSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $approvedPath).Hash.ToLowerInvariant()

# 指纹：python 侧独立实现（sort_keys + 紧凑分隔符）
$fingerprint = (& $python $stubPy --fingerprint-file $tasksFile 2>$null | Select-Object -First 1)
$fingerprint = ([string]$fingerprint).Trim()
if ($fingerprint -notmatch '^sha256:[0-9a-f]{64}$') {
    throw "计算获批任务配置指纹失败（python 输出：$fingerprint）"
}
Write-Host ("[fixture] 部署二进制 = {0}（sha256={1}）" -f $deployBin, $deploySha) -ForegroundColor DarkGray
Write-Host ("[fixture] 受信清单   = {0}" -f $manifestPath) -ForegroundColor DarkGray
Write-Host ("[fixture] 获批配置   = {0}（指纹 {1}）" -f $approvedPath, $fingerprint) -ForegroundColor DarkGray

# 计算"期望的任务配置指纹"（用于 M15c：故意给一个与获批文件不一致的指纹）
$wrongFingerprint = 'sha256:' + ('0' * 64)

# 受信清单的负向 fixture（M14d：摘要与二进制不符 ⇒ 必须判 invalid，不得放行）
$manifestWrongPath = Join-Path $fixtureRoot 'mesad-build-manifest-wrong-digest.json'
$manifestWrongDoc = [ordered]@{
    schema = 'mesa.stop-watchdog.build-manifest.v1'
    note   = '负向 fixture：摘要被故意写错，用来证明 G7 不会"有清单就放行"'
    binary = (Split-Path -Leaf $deployBin)
    sha256 = ('b' * 64)
}
[System.IO.File]::WriteAllText($manifestWrongPath, ($manifestWrongDoc | ConvertTo-Json -Depth 6), [System.Text.UTF8Encoding]::new($false))

# ── G1 负向 fixture（M17）：清单自身未被独立验证 ───────────────────────────────────
# 受信清单自身的摘要 = "签署包里批准的那个值"（正向用例与 M17 的对照基准）。
$manifestSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $manifestPath).Hash.ToLowerInvariant()

# (b) 攻击者把"清单 + 二进制"一起换掉，且换得**自洽**：清单内部摘要 = 攻击者二进制的真实摘要。
#     于是"清单里的摘要 ↔ 二进制实算摘要"必然匹配 —— 这正是旧实现的缺口：
#     只有"清单自身与签署包批准值比对"（-ExpectedManifestSha256）才能识破。
$attackerBinPath = Join-Path $fixtureRoot 'm17-attacker-mesad.bin'
[System.IO.File]::WriteAllBytes($attackerBinPath, [System.Text.Encoding]::UTF8.GetBytes("ATTACKER mesad build: NOT the approved artifact`nrevision=attacker`n"))
$attackerSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $attackerBinPath).Hash.ToLowerInvariant()
$attackerManifestPath = Join-Path $fixtureRoot 'm17-attacker-manifest.json'
$attackerManifestDoc = [ordered]@{
    schema = 'mesa.stop-watchdog.build-manifest.v1'
    note   = '负向 fixture（M17b）：攻击者自洽清单 —— 内部摘要 = 攻击者二进制的真实摘要'
    binary = (Split-Path -Leaf $attackerBinPath)
    sha256 = $attackerSha
}
[System.IO.File]::WriteAllText($attackerManifestPath, ($attackerManifestDoc | ConvertTo-Json -Depth 6), [System.Text.UTF8Encoding]::new($false))

# (c) 只改清单**字节**、内部摘要仍指向真二进制（"清单文件字节被改动"的最小形态）。
$tamperedManifestPath = Join-Path $fixtureRoot 'm17-manifest-bytes-tampered.json'
$tamperedManifestDoc = [ordered]@{
    schema      = 'mesa.stop-watchdog.build-manifest.v1'
    note        = '负向 fixture（M17c）：清单字节被改动（多了一个字段），内部摘要仍指向真二进制'
    binary      = (Split-Path -Leaf $deployBin)
    sha256      = $deploySha
    injected_by = 'someone-who-is-not-the-signer'
}
[System.IO.File]::WriteAllText($tamperedManifestPath, ($tamperedManifestDoc | ConvertTo-Json -Depth 6), [System.Text.UTF8Encoding]::new($false))

# ── G2 负向 fixture（M18）：只改 connection，tasks 一字未动 ⇒ 指纹不变 ──────────────
# 这是 G2 存在的理由：-ExpectedTaskFingerprint 只覆盖规范化 tasks 数组，
# connection（host/port/TSAP/PDU）不在其中，所以"指纹相同"完全挡不住 connection 被改。
$approvedTamperedDoc = [ordered]@{
    schema      = 'mesa.stop-watchdog.approved-task-config.v1'
    endpoint_id = $endpoint
    revision    = $expectedRevision
    connection  = [ordered]@{
        host        = $conn.host
        port        = 18100                       # ← 被改（真实桩声明的设备端口是 18099）
        local_tsap  = $conn.local_tsap
        remote_tsap = $conn.remote_tsap
        pdu_length  = $conn.pdu_length
    }
    tasks       = $tasksFixture                   # ← 与获批配置完全一致 ⇒ 指纹不变
}
$approvedTamperedPath = Join-Path $fixtureRoot 'm18-approved-connection-tampered.json'
[System.IO.File]::WriteAllText($approvedTamperedPath, ($approvedTamperedDoc | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
$approvedTamperedSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $approvedTamperedPath).Hash.ToLowerInvariant()
# 独立（python 侧）复算"改后文件里 tasks 的指纹"，用于在 M18b 断言"指纹确实没变"。
$tamperedTasksFile = Join-Path $fixtureRoot 'm18-tampered-tasks-only.json'
[System.IO.File]::WriteAllText($tamperedTasksFile, ($tasksFixture | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
$tamperedFingerprint = ([string](& $python $stubPy --fingerprint-file $tamperedTasksFile 2>$null | Select-Object -First 1)).Trim()

# ── 通用小工具 ────────────────────────────────────────────────────────────────────
$script:asserts = @()
function Assert-Wd {
    param([string]$Name, [bool]$Ok, [string]$Detail = '')
    $script:asserts += [pscustomobject]@{ name = $Name; ok = [bool]$Ok; detail = $Detail }
}
function Get-Nested {
    param($Obj, [string]$Path)
    $cur = $Obj
    foreach ($seg in $Path.Split('.')) {
        if ($null -eq $cur) { return $null }
        $prop = $cur.PSObject.Properties[$seg]
        if (-not $prop) { return $null }
        $cur = $prop.Value
    }
    return $cur
}
function Read-Jsonl {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return @() }
    $out = @()
    foreach ($ln in [System.IO.File]::ReadAllLines($Path)) {
        if ([string]::IsNullOrWhiteSpace($ln)) { continue }
        try { $out += ($ln | ConvertFrom-Json) } catch { }
    }
    return $out
}
function Get-DeadlineStops {
    param($Stops)
    return @($Stops | Where-Object { $r = @($_.reasons); ($r -contains 'DEADLINE') -or ($r -contains 'DEADLINE_FAILSAFE') })
}
function Get-EarlyStops {
    param($Stops)
    return @($Stops | Where-Object { $r = @($_.reasons); -not (($r -contains 'DEADLINE') -or ($r -contains 'DEADLINE_FAILSAFE')) })
}
function Get-MaxAbsDeviation {
    param($Stops)
    $devs = @($Stops | Where-Object { $null -ne $_.deviation_ms } | ForEach-Object { [math]::Abs([double]$_.deviation_ms) })
    if ($devs.Count -eq 0) { return $null }
    return ($devs | Measure-Object -Maximum).Maximum
}
function Get-StopReasonHit {
    param($Stops, [string]$Pattern)
    return @($Stops | Where-Object { (@($_.reasons) -join '+') -match $Pattern })
}
function Format-Stamp {
    param($v)
    if ($null -eq $v) { return 'N/A' }
    if ($v -is [datetime]) { return $v.ToString('o') }
    return [string]$v
}

# ── 假端点桩启停 ──────────────────────────────────────────────────────────────────
function Start-Stub {
    param([int]$Port, [string[]]$StubArgs, [string]$OutDir, [string]$Python)
    $log = Join-Path $OutDir 'stub-requests.jsonl'
    $argList = @($stubPy) + $StubArgs + @('--log', $log)
    $sp = @{
        FilePath               = $Python
        ArgumentList           = ($argList | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } })
        PassThru               = $true
        WorkingDirectory       = $repo
        RedirectStandardOutput = (Join-Path $OutDir 'stub-stdout.log')
        RedirectStandardError  = (Join-Path $OutDir 'stub-stderr.log')
    }
    if ($IsWindows) { $sp['WindowStyle'] = 'Hidden' }
    $proc = Start-Process @sp
    $baseUrl = "http://127.0.0.1:$Port"
    $ready = $false
    $deadline = (Get-Date).AddSeconds($StubReadyTimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $h = Invoke-WebRequest -Uri "$baseUrl/health" -TimeoutSec 2 -SkipHttpErrorCheck
            if ([int]$h.StatusCode -eq 200) { $ready = $true; break }
        }
        catch { }
        Start-Sleep -Milliseconds 100
    }
    if (-not $ready) {
        try { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue } catch { }
        throw "假端点桩未就绪：$baseUrl（见 $OutDir/stub-stderr.log）"
    }
    return $proc
}

# ── 组装 watchdog 参数 / 执行一次 / 收证据 ─────────────────────────────────────────
# G7 的两种合法形式（B2）：sha256:<hex> 或 manifest:<path>。默认用 sha256 形式，
# M16 用 manifest 形式，保证两种形式都被真实跑到。
function Get-G7Arg {
    param([string]$Form)
    if ($Form -eq 'manifest') { return "manifest:$manifestPath" }
    return "sha256:$deploySha"
}

# 统一用"参数表 → argv"的方式组装：便于负向用例按名字覆盖/去掉某一项，
# 不依赖"同名参数重复出现时谁生效"这种隐式规则。
function New-WdArgsFromSpec {
    param([string]$Name, [string]$OutDir, [string]$BaseUrl, [hashtable]$Set = @{})
    $spec = [ordered]@{
        '-Mode'                    = 'fire'
        '-Authorized'              = $true
        '-AuthorizedBy'            = 'SIM-AUTHZ'
        '-AuthzTicket'             = "SIM-$Name"
        '-G6Signoff'               = 'SIM-G6'
        '-G6SignedAt'              = $g6SignedAt
        '-G7DeployVersion'         = (Get-G7Arg 'sha256')
        '-G7BinaryPath'            = $deployBin
        '-ExpectedTaskRevision'    = $expectedRevision
        '-ExpectedPointCount'      = [string]$expectedPoints
        '-ApprovedTaskConfig'      = $approvedPath
        # G2：获批任务配置**文件自身**的摘要（正向用例一律给"本轮 fixture 的真实摘要"，
        # 等价于签署包里批准的那个值；负向用例在 -Set 里显式覆盖成缺失/过期值）。
        '-ExpectedApprovedTaskConfigSha256' = "sha256:$approvedSha"
        '-ExpectedTaskFingerprint' = $fingerprint
        '-BaseUrl'                 = $BaseUrl
        '-EndpointId'              = $endpoint
        '-OutDir'                  = $OutDir
        '-Tag'                     = $Name
    }
    foreach ($k in $Set.Keys) { $spec[$k] = $Set[$k] }
    # G1：走 manifest:<path> 形式时，脚本要求同时给"清单自身"的批准摘要。
    # 这里按**清单文件当前的真实摘要**自动填入（等价于"签署包里批准的就是这份清单"）；
    # 需要验证"缺失/被换掉"的负向用例，在 -Set 里把该键显式置为 '' 或换成旧值即可（见下方 Contains 判断）。
    $g7Value = [string]$spec['-G7DeployVersion']
    if ($g7Value -match '^(?i)manifest:(.+)$') {
        if (-not $spec.Contains('-ExpectedManifestSha256')) {
            $mPath = $Matches[1].Trim()
            if (Test-Path -LiteralPath $mPath -PathType Leaf) {
                $spec['-ExpectedManifestSha256'] = 'sha256:' + (Get-FileHash -Algorithm SHA256 -LiteralPath $mPath).Hash.ToLowerInvariant()
            }
        }
    }
    $a = @()
    foreach ($k in $spec.Keys) {
        $v = $spec[$k]
        if ($null -eq $v) { continue }
        if ($v -is [bool]) { if ($v) { $a += $k }; continue }
        if ($v -is [string] -and $v -eq '') { continue }
        $a += @($k, [string]$v)
    }
    return $a
}

function New-WdArgs {
    param([string]$Name, [string]$OutDir, [string]$BaseUrl, $Cfg)
    $set = @{}
    $g7Form = if ($Cfg.ContainsKey('g7')) { [string]$Cfg.g7 } else { 'sha256' }
    $set['-G7DeployVersion'] = Get-G7Arg $g7Form
    if ($Cfg.mode -eq 'arm') {
        # ARMED：只自检。凭证都给（用于演示"凭证齐备也不等于可以启动端点"），但**不给** -Authorized。
        $set['-Mode'] = 'arm'
        $set['-Authorized'] = $false
        $set['-ArmHoldSec'] = [string]$Cfg.armHold
    }
    $a = New-WdArgsFromSpec -Name $Name -OutDir $OutDir -BaseUrl $BaseUrl -Set $set
    if ($Cfg.ContainsKey('wd') -and $Cfg.wd) { $a += $Cfg.wd }
    return $a
}

function Invoke-WdRun {
    param([string]$OutDir, [string[]]$WdArgs)
    if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Force -Path $OutDir | Out-Null }
    $consoleLog = Join-Path $OutDir 'console.log'
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $global:LASTEXITCODE = 0
    & $pwshExe -NoProfile -File $watchdog @WdArgs 2>&1 | Tee-Object -FilePath $consoleLog | Out-Null
    $code = $LASTEXITCODE
    return [pscustomobject]@{ out = $OutDir; exit = $code; elapsed = [math]::Round($sw.Elapsed.TotalSeconds, 1) }
}

function Invoke-WdCase {
    # 跑一次子用例，并把桩日志按行号切成"本次子用例"的切片（同场景内多个子用例共用一只桩）。
    param([string]$OutDir, [string[]]$WdArgs, [string]$StubLog)
    $before = Get-StubLogLineCount -Path $StubLog
    $run = Invoke-WdRun -OutDir $OutDir -WdArgs $WdArgs
    $ev = Read-WdEvidence $run
    $all = @(Read-Jsonl $StubLog)
    $ev.events = @($all | Select-Object -Skip $before)
    $ev.ev = @{}
    foreach ($e in $ev.events) {
        $k = [string]$e.event
        if (-not $ev.ev.ContainsKey($k)) { $ev.ev[$k] = 0 }
        $ev.ev[$k]++
    }
    return $ev
}

function Read-WdEvidence {
    param([pscustomobject]$Run)
    $out = $Run.out
    $sum = $null
    if (Test-Path (Join-Path $out 'summary.json')) {
        try { $sum = Get-Content (Join-Path $out 'summary.json') -Raw | ConvertFrom-Json } catch { }
    }
    $arm = $null
    if (Test-Path (Join-Path $out 'arm-report.json')) {
        try { $arm = Get-Content (Join-Path $out 'arm-report.json') -Raw | ConvertFrom-Json } catch { }
    }
    $t0 = $null
    if (Test-Path (Join-Path $out 't0.stamp')) {
        try { $t0 = Get-Content (Join-Path $out 't0.stamp') -Raw | ConvertFrom-Json } catch { }
    }
    $stops = @(Read-Jsonl (Join-Path $out 'watchdog-stops.jsonl'))
    $states = @(Read-Jsonl (Join-Path $out 'watchdog-states.jsonl'))
    $events = @(Read-Jsonl (Join-Path $out 'stub-requests.jsonl'))
    $console = if (Test-Path (Join-Path $out 'console.log')) { [System.IO.File]::ReadAllText((Join-Path $out 'console.log')) } else { '' }
    $evCount = @{}
    foreach ($e in $events) {
        $k = [string]$e.event
        if (-not $evCount.ContainsKey($k)) { $evCount[$k] = 0 }
        $evCount[$k]++
    }
    $deadlineStops = @(Get-DeadlineStops $stops)
    return [pscustomobject]@{
        out           = $out
        exit          = $Run.exit
        elapsed       = $Run.elapsed
        summary       = $sum
        arm           = $arm
        t0            = $t0
        stops         = $stops
        states        = $states
        events        = $events
        console       = $console
        ev            = $evCount
        deadlineStops = $deadlineStops
        earlyStops    = @(Get-EarlyStops $stops)
        maxAbsDev     = (Get-MaxAbsDeviation $deadlineStops)
        t0Exists      = (Test-Path (Join-Path $out 't0.stamp'))
        summaryExists = (Test-Path (Join-Path $out 'summary.json'))
    }
}
function Count-Ev {
    param($Ev, [string]$Kind)
    if ($Ev.ev.ContainsKey($Kind)) { return [int]$Ev.ev[$Kind] }
    return 0
}
function Get-StubLogLineCount {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return 0 }
    return @([System.IO.File]::ReadAllLines($Path) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }).Count
}

# ── 主循环 ────────────────────────────────────────────────────────────────────────
$rows = @()
foreach ($name in $targets) {
    $cfg = $SCEN[$name]
    $out = Join-Path $EvidenceRoot $name
    if (-not $KeepEvidence -and (Test-Path $out)) { Remove-Item -Recurse -Force $out }
    New-Item -ItemType Directory -Force -Path $out | Out-Null

    $baseUrl = "http://127.0.0.1:$($cfg.port)"

    Write-Host ''
    Write-Host ("════════ {0}  {1} ════════" -f $name, $cfg.title) -ForegroundColor Cyan

    $stubProc = $null
    if ($cfg.stub.Count -gt 0) {
        $stubProc = Start-Stub -Port $cfg.port -StubArgs $cfg.stub -OutDir $out -Python $python
        Write-Host ("[stub] 就绪 {0}（pid={1}）" -f $baseUrl, $stubProc.Id)
    }
    else {
        Write-Host ("[stub] 无桩：使用不可达地址 {0}" -f $baseUrl)
    }

    $script:asserts = @()

    if ($cfg.ContainsKey('phases') -and $cfg.phases -eq $true) {
        # ── M12：授权门负向，两段子用例（都必须 exit 40 且不发 Start）───────────────
        $outA = Join-Path $out 'a-missing-authorization'
        $outB = Join-Path $out 'b-taskconfig-mismatch'
        $argsA = New-WdArgsFromSpec -Name 'M12a' -OutDir $outA -BaseUrl $baseUrl -Set @{
            '-Authorized' = $false
            '-AuthzTicket' = 'SIM-M12a'
        }
        $argsB = New-WdArgsFromSpec -Name 'M12b' -OutDir $outB -BaseUrl $baseUrl -Set @{
            '-AuthzTicket' = 'SIM-M12b'
            '-ExpectedTaskRevision' = 'rev99-DOES-NOT-MATCH'
        }
        $runA = Invoke-WdRun -OutDir $outA -WdArgs $argsA
        $nA = Get-StubLogLineCount -Path (Join-Path $out 'stub-requests.jsonl')
        $runB = Invoke-WdRun -OutDir $outB -WdArgs $argsB
        $allEvents = @(Read-Jsonl (Join-Path $out 'stub-requests.jsonl'))
        # 桩日志在场景根目录共享：按行号切分，保证每段子用例只看到自己那段的请求。
        $evA = Read-WdEvidence $runA
        $evA.events = @($allEvents | Select-Object -First $nA)
        $evB = Read-WdEvidence $runB
        $evB.events = @($allEvents | Select-Object -Skip $nA)
        foreach ($x in @($evA, $evB)) {
            $x.ev = @{}
            foreach ($e in $x.events) {
                $k = [string]$e.event
                if (-not $x.ev.ContainsKey($k)) { $x.ev[$k] = 0 }
                $x.ev[$k]++
            }
        }

        Assert-Wd '[a] 缺 -Authorized/-AuthorizedBy ⇒ 40' ($evA.exit -eq 40) "exit=$($evA.exit)"
        Assert-Wd '[a] 拒绝标签 = REFUSED_NOT_AUTHORIZED' ((Get-Nested $evA.summary 'refused') -eq 'REFUSED_NOT_AUTHORIZED') "refused=$(Get-Nested $evA.summary 'refused')"
        Assert-Wd '[a] 拒绝说明为"授权边界拒绝，非设备故障"' ([string](Get-Nested $evA.summary 'note') -match '非设备故障') "note=$(Get-Nested $evA.summary 'note')"
        Assert-Wd '[a] 未写 t0.stamp（未进入 FIRED）' (-not $evA.t0Exists) "t0=$(Join-Path $outA 't0.stamp')"
        Assert-Wd '[a] 桩未收到任何 START' ((Count-Ev $evA 'START_begin') -eq 0) "START_begin=$(Count-Ev $evA 'START_begin')"

        Assert-Wd '[b] 任务配置 revision 不匹配 ⇒ 40' ($evB.exit -eq 40) "exit=$($evB.exit)"
        Assert-Wd '[b] 拒绝标签 = REFUSED_TASKCONFIG_MISMATCH' ((Get-Nested $evB.summary 'refused') -eq 'REFUSED_TASKCONFIG_MISMATCH') "refused=$(Get-Nested $evB.summary 'refused')"
        Assert-Wd '[b] 未写 t0.stamp（未进入 FIRED）' (-not $evB.t0Exists) "t0=$(Join-Path $outB 't0.stamp')"
        Assert-Wd '[b] 桩未收到任何 START' ((Count-Ev $evB 'START_begin') -eq 0) "START_begin=$(Count-Ev $evB 'START_begin')"
        Assert-Wd '[b] 确实做过只读核验 GET（非空谈）' (((Count-Ev $evB 'DIAG') + (Count-Ev $evB 'TASKS')) -ge 1) "DIAG=$(Count-Ev $evB 'DIAG') TASKS=$(Count-Ev $evB 'TASKS')"
        Assert-Wd '[b] 拒绝记录里如实报告网络动作次数' ([int](Get-Nested $evB.summary 'network_actions') -ge 1) "network_actions=$(Get-Nested $evB.summary 'network_actions')"

        $ev = $evA
        $rowExit = $evA.exit
        $rowVerdict = ('{0} / {1}' -f [string](Get-Nested $evA.summary 'refused'), [string](Get-Nested $evB.summary 'refused'))
        $rowTrial = ''
        $rowSched = ''
        $rowSampling = ''
        $rowConv = ''
        $rowT0 = ''
        $rowStops = @()
        $rowMaxDev = $null
        $rowElapsed = [math]::Round($evA.elapsed + $evB.elapsed, 1)
    }
    elseif ($cfg.phases -eq 'm13') {
        # ── M13：测试注入参数的离线边界（B1 负向）────────────────────────────────────
        # 三个子用例**都带完整有效凭证**：这样 40 只可能来自注入守卫（若守卫失效，脚本会真的去
        # 连接目标地址 → 子用例会以别的标签/非 40 结束，测试立刻失败）。这比"故意缺凭证"强得多。
        # 桩绑在 0.0.0.0：子用例 c 用"本机主机名"这种**非回环字面量**，若守卫没拦住，请求会真的
        # 落到桩上（因此"桩收到 0 条请求"不是空断言）。
        $stubLog = Join-Path $out 'stub-requests.jsonl'
        $hostLabel = [System.Net.Dns]::GetHostName()
        $cases = @(
            @{ tag = 'a-non-loopback-ip'; base = 'http://192.0.2.10:18093'; wd = @('-TestAbortFlow', 'timer'); why = '非回环地址 192.0.2.10 + -TestAbortFlow' }
            @{ tag = 'b-production-port-8132'; base = 'http://127.0.0.1:8132'; wd = @('-TestHostProbeError', 'both'); why = '生产端口 8132 + -TestHostProbeError' }
            @{ tag = 'c-non-loopback-hostname'; base = "http://${hostLabel}:18093"; wd = @('-TestAbortFlow', 'stop'); why = "非回环字面量（本机主机名 $hostLabel）+ -TestAbortFlow" }
        )
        $elapsed = 0.0
        foreach ($c in $cases) {
            $co = Join-Path $out $c.tag
            $a = New-WdArgsFromSpec -Name "M13$($c.tag)" -OutDir $co -BaseUrl $c.base -Set @{ '-AuthzTicket' = "SIM-M13-$($c.tag)" }
            $a += $c.wd
            $ev = Invoke-WdCase -OutDir $co -WdArgs $a -StubLog $stubLog
            $elapsed += $ev.elapsed
            $guard = Get-Nested $ev.summary 'extra.injection_guard'
            Assert-Wd ("[{0}] {1} ⇒ exit 40" -f $c.tag, $c.why) ($ev.exit -eq 40) "exit=$($ev.exit)"
            Assert-Wd ("[{0}] 拒绝标签 = REFUSED_TEST_INJECTION_ONLINE" -f $c.tag) ((Get-Nested $ev.summary 'refused') -eq 'REFUSED_TEST_INJECTION_ONLINE') "refused=$(Get-Nested $ev.summary 'refused')"
            Assert-Wd ("[{0}] 网络动作数 = 0（拒绝发生在任何连接之前）" -f $c.tag) ([int](Get-Nested $ev.summary 'network_actions') -eq 0) "network_actions=$(Get-Nested $ev.summary 'network_actions')"
            Assert-Wd ("[{0}] 未写 t0.stamp（未进入 FIRED）" -f $c.tag) (-not $ev.t0Exists) "t0=$($ev.t0Exists)"
            Assert-Wd ("[{0}] 假端点未收到任何请求（含 Start/Stop）" -f $c.tag) (@($ev.events).Count -eq 0) "events=$(@($ev.events).Count)"
            Assert-Wd ("[{0}] 守卫如实记录了注入参数" -f $c.tag) (@(Get-Nested $guard 'params').Count -ge 1) "params=$((@(Get-Nested $guard 'params')) -join ' ')"
            Assert-Wd ("[{0}] 守卫判定的 host 就是 BaseUrl 的字面 host" -f $c.tag) ([string](Get-Nested $guard 'host') -eq ([System.Uri]::new($c.base)).Host) "guard_host=$(Get-Nested $guard 'host')"
        }
        $ev = $null
        $rowExit = 40
        $rowVerdict = 'REFUSED_TEST_INJECTION_ONLINE ×3'
        $rowTrial = ''
        $rowSched = ''
        $rowSampling = ''
        $rowConv = ''
        $rowT0 = ''
        $rowStops = @()
        $rowMaxDev = $null
        $rowElapsed = [math]::Round($elapsed, 1)
    }
    elseif ($cfg.phases -eq 'm14') {
        # ── M14：G7 部署身份不可验证（B2 fail-closed）────────────────────────────────
        # 关键：**只给 git 版本标识**也必须被拒（旧实现会把它当通过依据）。同时覆盖
        # 二进制缺失 / 清单缺失 / 清单摘要不符 / 完全未提供 四种不可验证形态。
        $stubLog = Join-Path $out 'stub-requests.jsonl'
        $gitHead = ''
        try { $gitHead = ([string](& git -C $repo rev-parse HEAD 2>$null | Select-Object -First 1)).Trim() } catch { }
        if (-not $gitHead) { $gitHead = 'f803de55d319cdc24e28d069a3a40ab92d7024c7' }  # 无 git 时的固定回退（仅用于构造"像版本号"的输入）
        $cases = @(
            @{ tag = 'a-git-head-only'; set = @{ '-G7DeployVersion' = $gitHead }; expectState = 'unverified'; why = '只给 git HEAD（无摘要/无清单）' }
            @{ tag = 'b-sha256-binary-missing'; set = @{ '-G7DeployVersion' = ('sha256:' + ('a' * 64)); '-G7BinaryPath' = (Join-Path $out 'no-such-binary.bin') }; expectState = 'unverified'; why = 'sha256 + 指定的部署二进制不存在' }
            @{ tag = 'c-manifest-missing'; set = @{ '-G7DeployVersion' = ('manifest:' + (Join-Path $out 'no-such-manifest.json')) }; expectState = 'unverified'; why = '受信清单读不到' }
            @{ tag = 'd-manifest-wrong-digest'; set = @{ '-G7DeployVersion' = "manifest:$manifestWrongPath"; '-G7BinaryPath' = $deployBin }; expectState = 'invalid'; why = '受信清单摘要与部署二进制不符' }
            @{ tag = 'e-no-g7'; set = @{ '-G7DeployVersion' = '' }; expectState = 'missing'; why = '完全未提供 -G7DeployVersion' }
        )
        $elapsed = 0.0
        foreach ($c in $cases) {
            $co = Join-Path $out $c.tag
            $set = @{ '-AuthzTicket' = "SIM-M14-$($c.tag)" }
            foreach ($k in $c.set.Keys) { $set[$k] = $c.set[$k] }
            $a = New-WdArgsFromSpec -Name "M14$($c.tag)" -OutDir $co -BaseUrl $baseUrl -Set $set
            $ev = Invoke-WdCase -OutDir $co -WdArgs $a -StubLog $stubLog
            $elapsed += $ev.elapsed
            # 拒绝记录的形状：Write-WdRefusal 把凭证明细放在 extra.credentials（与 fire 的 summary 不同）
            $g7State = [string](Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.state')
            $refused = [string](Get-Nested $ev.summary 'refused')
            Assert-Wd ("[{0}] {1} ⇒ exit 40" -f $c.tag, $c.why) ($ev.exit -eq 40) "exit=$($ev.exit)"
            Assert-Wd ("[{0}] G7 不得判 verified" -f $c.tag) ($g7State -ne 'verified') "g7_state=$g7State"
            Assert-Wd ("[{0}] G7 state = {1}" -f $c.tag, $c.expectState) ($g7State -eq $c.expectState) "g7_state=$g7State"
            Assert-Wd ("[{0}] 拒绝标签属于凭证不可验证/不一致/缺失" -f $c.tag) (@('REFUSED_CREDENTIAL_UNVERIFIABLE', 'REFUSED_CREDENTIAL_INVALID', 'REFUSED_NOT_AUTHORIZED') -contains $refused) "refused=$refused"
            Assert-Wd ("[{0}] 未写 t0.stamp（未进入 FIRED）" -f $c.tag) (-not $ev.t0Exists) "t0=$($ev.t0Exists)"
            Assert-Wd ("[{0}] 桩未收到任何 START" -f $c.tag) ((Count-Ev $ev 'START_begin') -eq 0) "START_begin=$(Count-Ev $ev 'START_begin')"
            if ($c.tag -eq 'a-git-head-only') {
                # 反证：git 信息被**记录**了（辅助信息），但**没有**被当作通过依据。
                Assert-Wd '[a] 辅助信息里如实记录了 git HEAD（仅记录）' ([string](Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.actual.git.git_head') -eq $gitHead) "git_head=$(Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.actual.git.git_head')"
                Assert-Wd '[a] 拒绝说明指明"不能独立证明部署二进制身份"' ([string](Get-Nested $ev.summary 'reason') -match '不能独立证明|不是 sha256|部署二进制') "reason=$(Get-Nested $ev.summary 'reason')"
            }
        }
        $ev = $null
        $rowExit = 40
        $rowVerdict = 'G7 unverified/invalid ×5（fail-closed）'
        $rowTrial = ''
        $rowSched = ''
        $rowSampling = ''
        $rowConv = ''
        $rowT0 = ''
        $rowStops = @()
        $rowMaxDev = $null
        $rowElapsed = [math]::Round($elapsed, 1)
    }
    elseif ($cfg.phases -eq 'm15') {
        # ── M15：完整任务配置不匹配（B3 负向）────────────────────────────────────────
        # 三个子用例各只改一处：连接配置 PDU / 某点位的参数 / 期望指纹。都必须 exit 40 且不发 Start。
        $stubLog = Join-Path $out 'stub-requests.jsonl'
        # a：获批配置把 PDU 写成 480（桩是 240）——tasks 未变，所以指纹仍用同一份。
        $approvedA = [ordered]@{
            schema      = 'mesa.stop-watchdog.approved-task-config.v1'
            endpoint_id = $endpoint
            revision    = $expectedRevision
            connection  = [ordered]@{
                host        = $conn.host
                port        = $conn.port
                local_tsap  = $conn.local_tsap
                remote_tsap = $conn.remote_tsap
                pdu_length  = 480
            }
            tasks       = $tasksFixture
        }
        $approvedAPath = Join-Path $out 'a-approved-pdu-mismatch.json'
        [System.IO.File]::WriteAllText($approvedAPath, ($approvedA | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
        # b：改一个点位的参数（offset 0 → 99），并对**改后**的 tasks 重新算指纹，
        #    这样触发点只可能是"点位参数不一致"（而不是顺带触发指纹不一致）。
        $tasksB = Get-Content -LiteralPath $tasksFile -Raw | ConvertFrom-Json
        $tasksB[0].binding.config.points[0].parameters.offset = 99
        $tasksBPath = Join-Path $out 'b-tasks-point-param-changed.json'
        [System.IO.File]::WriteAllText($tasksBPath, ($tasksB | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
        $fpB = ([string](& $python $stubPy --fingerprint-file $tasksBPath 2>$null | Select-Object -First 1)).Trim()
        $approvedB = [ordered]@{
            schema      = 'mesa.stop-watchdog.approved-task-config.v1'
            endpoint_id = $endpoint
            revision    = $expectedRevision
            connection  = $approvedDoc.connection
            tasks       = $tasksB
        }
        $approvedBPath = Join-Path $out 'b-approved-point-param-changed.json'
        [System.IO.File]::WriteAllText($approvedBPath, ($approvedB | ConvertTo-Json -Depth 20), [System.Text.UTF8Encoding]::new($false))
        $cases = @(
            @{ tag = 'a-connection-pdu-mismatch'; set = @{ '-ApprovedTaskConfig' = $approvedAPath; '-ExpectedApprovedTaskConfigSha256' = ('sha256:' + (Get-FileHash -Algorithm SHA256 -LiteralPath $approvedAPath).Hash.ToLowerInvariant()); '-ExpectedTaskFingerprint' = $fingerprint }; why = '连接配置 PDU 不一致（获批 480 / 实际 240）'; expect = 'pdu_length' }
            @{ tag = 'b-point-param-mismatch'; set = @{ '-ApprovedTaskConfig' = $approvedBPath; '-ExpectedApprovedTaskConfigSha256' = ('sha256:' + (Get-FileHash -Algorithm SHA256 -LiteralPath $approvedBPath).Hash.ToLowerInvariant()); '-ExpectedTaskFingerprint' = $fpB }; why = '点位参数不一致（point 0 offset 0→99）'; expect = '点位参数不一致' }
            @{ tag = 'c-fingerprint-mismatch'; set = @{ '-ApprovedTaskConfig' = $approvedPath; '-ExpectedTaskFingerprint' = $wrongFingerprint }; why = '配置指纹与获批版本不一致'; expect = '指纹' }
        )
        $elapsed = 0.0
        foreach ($c in $cases) {
            $co = Join-Path $out $c.tag
            $set = @{ '-AuthzTicket' = "SIM-M15-$($c.tag)" }
            foreach ($k in $c.set.Keys) { $set[$k] = $c.set[$k] }
            $a = New-WdArgsFromSpec -Name "M15$($c.tag)" -OutDir $co -BaseUrl $baseUrl -Set $set
            $ev = Invoke-WdCase -OutDir $co -WdArgs $a -StubLog $stubLog
            $elapsed += $ev.elapsed
            Assert-Wd ("[{0}] {1} ⇒ exit 40" -f $c.tag, $c.why) ($ev.exit -eq 40) "exit=$($ev.exit)"
            Assert-Wd ("[{0}] 拒绝标签 = REFUSED_TASKCONFIG_MISMATCH" -f $c.tag) ((Get-Nested $ev.summary 'refused') -eq 'REFUSED_TASKCONFIG_MISMATCH') "refused=$(Get-Nested $ev.summary 'refused')"
            Assert-Wd ("[{0}] 未写 t0.stamp（未进入 FIRED）" -f $c.tag) (-not $ev.t0Exists) "t0=$($ev.t0Exists)"
            Assert-Wd ("[{0}] 桩未收到任何 START" -f $c.tag) ((Count-Ev $ev 'START_begin') -eq 0) "START_begin=$(Count-Ev $ev 'START_begin')"
            Assert-Wd ("[{0}] 拒绝原因点明具体差异（{1}）" -f $c.tag, $c.expect) ([string](Get-Nested $ev.summary 'reason') -match [regex]::Escape($c.expect)) "reason=$(Get-Nested $ev.summary 'reason')"
        }
        $ev = $null
        $rowExit = 40
        $rowVerdict = 'REFUSED_TASKCONFIG_MISMATCH ×3'
        $rowTrial = ''
        $rowSched = ''
        $rowSampling = ''
        $rowConv = ''
        $rowT0 = ''
        $rowStops = @()
        $rowMaxDev = $null
        $rowElapsed = [math]::Round($elapsed, 1)
    }
    elseif ($cfg.phases -eq 'm17') {
        # ── M17（G1 负向）：构建清单**本身**未被独立验证 ⇒ 一律拒绝 ────────────────────
        # 三个子用例都只有"清单自身"这一层不成立（其余凭证齐全），因此 40 只可能来自 G1。
        # (b) 是最关键的一条：**清单 + 二进制一起被换掉**且两者互相匹配 —— 旧实现会放行。
        $stubLog = Join-Path $out 'stub-requests.jsonl'
        $cases = @(
            @{ tag = 'a-missing-expected-manifest-sha'; g7 = "manifest:$manifestPath"; bin = $deployBin; msha = ''; expectLabel = 'REFUSED_MANIFEST_INTEGRITY'; why = '走 manifest: 形式但缺 -ExpectedManifestSha256' }
            @{ tag = 'b-manifest-and-binary-swapped'; g7 = "manifest:$attackerManifestPath"; bin = $attackerBinPath; msha = "sha256:$manifestSha"; expectLabel = 'REFUSED_MANIFEST_INTEGRITY'; why = '清单+二进制一起被换（清单内部摘要与攻击者二进制匹配，二者互相自洽）' }
            @{ tag = 'c-manifest-bytes-tampered'; g7 = "manifest:$tamperedManifestPath"; bin = $deployBin; msha = "sha256:$manifestSha"; expectLabel = 'REFUSED_MANIFEST_INTEGRITY'; why = '清单文件字节被改动（内部摘要仍指向真二进制）' }
        )
        $elapsed = 0.0
        foreach ($c in $cases) {
            $co = Join-Path $out $c.tag
            $set = @{
                '-AuthzTicket'             = "SIM-M17-$($c.tag)"
                '-G7DeployVersion'         = $c.g7
                '-G7BinaryPath'            = $c.bin
                '-ExpectedManifestSha256'  = $c.msha
            }
            $a = New-WdArgsFromSpec -Name "M17$($c.tag)" -OutDir $co -BaseUrl $baseUrl -Set $set
            $ev = Invoke-WdCase -OutDir $co -WdArgs $a -StubLog $stubLog
            $elapsed += $ev.elapsed
            $g7State = [string](Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.state')
            $refused = [string](Get-Nested $ev.summary 'refused')
            Assert-Wd ("[{0}] {1} ⇒ exit 40" -f $c.tag, $c.why) ($ev.exit -eq 40) "exit=$($ev.exit)"
            Assert-Wd ("[{0}] 拒绝标签 = {1}" -f $c.tag, $c.expectLabel) ($refused -eq $c.expectLabel) "refused=$refused"
            Assert-Wd ("[{0}] G7 不得判 verified" -f $c.tag) ($g7State -ne 'verified') "g7_state=$g7State"
            Assert-Wd ("[{0}] 未写 t0.stamp（未进入 FIRED）" -f $c.tag) (-not $ev.t0Exists) "t0=$($ev.t0Exists)"
            Assert-Wd ("[{0}] 桩未收到任何 START" -f $c.tag) ((Count-Ev $ev 'START_begin') -eq 0) "START_begin=$(Count-Ev $ev 'START_begin')"
            Assert-Wd ("[{0}] 桩未收到任何请求（清单校验是纯本地检查）" -f $c.tag) (@($ev.events).Count -eq 0) "events=$(@($ev.events).Count)"
            Assert-Wd ("[{0}] 网络动作数 = 0（拒绝发生在任何连接之前）" -f $c.tag) ([int](Get-Nested $ev.summary 'network_actions') -eq 0) "network_actions=$(Get-Nested $ev.summary 'network_actions')"
            Assert-Wd ("[{0}] 拒绝原因点明清单自身未获独立批准" -f $c.tag) ([string](Get-Nested $ev.summary 'reason') -match '清单') "reason=$(Get-Nested $ev.summary 'reason')"
            if ($c.tag -eq 'b-manifest-and-binary-swapped') {
                # 反证"这是一次自洽的替换"：清单内部摘要 == 攻击者二进制的实算摘要。
                # 也就是说，**若只看清单与二进制是否互相匹配，本子用例会通过** —— 必须靠清单自身摘要识破。
                $attManifestDoc = Get-Content -LiteralPath $attackerManifestPath -Raw | ConvertFrom-Json
                Assert-Wd '[b] 前提成立：攻击者清单内部摘要 = 攻击者二进制实算摘要（二者互相匹配）' `
                    ([string]$attManifestDoc.sha256 -eq $attackerSha) "manifest_sha=$($attManifestDoc.sha256) bin_sha=$attackerSha"
                Assert-Wd '[b] 但清单自身摘要 ≠ 签署包批准值 ⇒ 仍然拒绝' `
                    ($manifestSha -ne (Get-FileHash -Algorithm SHA256 -LiteralPath $attackerManifestPath).Hash.ToLowerInvariant()) "signed=$manifestSha attacker_manifest=$((Get-FileHash -Algorithm SHA256 -LiteralPath $attackerManifestPath).Hash.ToLowerInvariant())"
                Assert-Wd '[b] 拒绝记录里保留了清单自身摘要比对（expected/actual 都在证据里）' `
                    (([string](Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.actual.manifest.sha256_self') -match '^[0-9a-f]{64}$') -and ([string](Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.actual.manifest.integrity.expected') -eq $manifestSha)) `
                    "self=$(Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.actual.manifest.sha256_self') expected=$(Get-Nested $ev.summary 'extra.credentials.items.g7_deploy_version.actual.manifest.integrity.expected')"
            }
        }
        $ev = $null
        $rowExit = 40
        $rowVerdict = 'REFUSED_MANIFEST_INTEGRITY ×3'
        $rowTrial = ''
        $rowSched = ''
        $rowSampling = ''
        $rowConv = ''
        $rowT0 = ''
        $rowStops = @()
        $rowMaxDev = $null
        $rowElapsed = [math]::Round($elapsed, 1)
    }
    elseif ($cfg.phases -eq 'm18') {
        # ── M18（G2 负向）：获批任务配置文件**自身**未被独立验证 ⇒ 一律拒绝 ─────────────
        # (b) 是最关键的一条：**tasks 指纹保持不变、只改 connection** —— 指纹校验完全挡不住，
        #     必须靠"文件原始字节摘要"识破。
        # (c) 是"改文件 + 用改后文件自身摘要"：文件完整性因此通过，但连接配置与实时不一致 ⇒ 仍然拒绝
        #     （证明字段比对这一层没有被 G2 取代）。
        $stubLog = Join-Path $out 'stub-requests.jsonl'
        $cases = @(
            @{ tag = 'a-missing-expected-config-sha'; file = $approvedPath; sha = ''; expectLabel = 'REFUSED_TASKCONFIG_INTEGRITY'; why = '缺 -ExpectedApprovedTaskConfigSha256'; expectNet = 0 }
            @{ tag = 'b-config-bytes-tampered-fingerprint-unchanged'; file = $approvedTamperedPath; sha = "sha256:$approvedSha"; expectLabel = 'REFUSED_TASKCONFIG_INTEGRITY'; why = '批准文件字节被改（只改 connection，tasks 未动 ⇒ 指纹不变）'; expectNet = 0 }
            @{ tag = 'c-tampered-with-its-own-sha'; file = $approvedTamperedPath; sha = "sha256:$approvedTamperedSha"; expectLabel = 'REFUSED_TASKCONFIG_MISMATCH'; why = '文件被改且用改后文件自身摘要 ⇒ 文件完整性通过，但连接配置与实时不一致'; expectNet = -1 }
        )
        $elapsed = 0.0
        foreach ($c in $cases) {
            $co = Join-Path $out $c.tag
            $set = @{
                '-AuthzTicket'                        = "SIM-M18-$($c.tag)"
                '-ApprovedTaskConfig'                 = $c.file
                '-ExpectedApprovedTaskConfigSha256'   = $c.sha
            }
            $a = New-WdArgsFromSpec -Name "M18$($c.tag)" -OutDir $co -BaseUrl $baseUrl -Set $set
            $ev = Invoke-WdCase -OutDir $co -WdArgs $a -StubLog $stubLog
            $elapsed += $ev.elapsed
            $refused = [string](Get-Nested $ev.summary 'refused')
            $net = [int](Get-Nested $ev.summary 'network_actions')
            Assert-Wd ("[{0}] {1} ⇒ exit 40" -f $c.tag, $c.why) ($ev.exit -eq 40) "exit=$($ev.exit)"
            Assert-Wd ("[{0}] 拒绝标签 = {1}" -f $c.tag, $c.expectLabel) ($refused -eq $c.expectLabel) "refused=$refused"
            Assert-Wd ("[{0}] 未写 t0.stamp（未进入 FIRED）" -f $c.tag) (-not $ev.t0Exists) "t0=$($ev.t0Exists)"
            Assert-Wd ("[{0}] 桩未收到任何 START" -f $c.tag) ((Count-Ev $ev 'START_begin') -eq 0) "START_begin=$(Count-Ev $ev 'START_begin')"
            Assert-Wd ("[{0}] 没有发出 Start（t0.stamp 与 START 双证）" -f $c.tag) ((-not $ev.t0Exists) -and ((Count-Ev $ev 'START_begin') -eq 0)) "t0=$($ev.t0Exists) START_begin=$(Count-Ev $ev 'START_begin')"
            if ($c.expectNet -ge 0) {
                Assert-Wd ("[{0}] 网络动作数 = {1}（G2 是纯本地检查，先于任何连接）" -f $c.tag, $c.expectNet) ($net -eq $c.expectNet) "network_actions=$net"
                Assert-Wd ("[{0}] 桩未收到任何请求" -f $c.tag) (@($ev.events).Count -eq 0) "events=$(@($ev.events).Count)"
            }
            else {
                Assert-Wd ("[{0}] 文件完整性通过后才做只读核验（网络动作数 ≥ 1）" -f $c.tag) ($net -ge 1) "network_actions=$net"
                Assert-Wd ("[{0}] 拒绝原因点明连接配置不一致" -f $c.tag) ([string](Get-Nested $ev.summary 'reason') -match 'port') "reason=$(Get-Nested $ev.summary 'reason')"
            }
            if ($c.tag -eq 'b-config-bytes-tampered-fingerprint-unchanged') {
                # 反证：改后文件里的 tasks 指纹**与获批指纹完全相同**（python 侧独立复算）。
                # 因此"指纹校验"这一层对该文件是失效的，挡住它的只能是文件原始字节摘要。
                Assert-Wd '[b] 前提成立：改后文件的 tasks 指纹与获批指纹相同（指纹不变）' ($tamperedFingerprint -eq $fingerprint) "tampered=$tamperedFingerprint approved=$fingerprint"
                Assert-Wd '[b] 前提成立：改后文件字节确实与获批文件不同' ($approvedTamperedSha -ne $approvedSha) "tampered_sha=$approvedTamperedSha approved_sha=$approvedSha"
                Assert-Wd '[b] 但仍然拒绝（只能靠文件摘要，不能靠指纹）' ($ev.exit -eq 40) "exit=$($ev.exit)"
                Assert-Wd '[b] 拒绝记录里同时给出 expected/actual 文件摘要' `
                    ((([string](Get-Nested $ev.summary 'extra.credentials.items.approved_task_config_integrity.actual.file.actual') -eq $approvedTamperedSha) -and ([string](Get-Nested $ev.summary 'extra.credentials.items.approved_task_config_integrity.actual.file.expected') -eq $approvedSha))) `
                    "actual=$(Get-Nested $ev.summary 'extra.credentials.items.approved_task_config_integrity.actual.file.actual') expected=$(Get-Nested $ev.summary 'extra.credentials.items.approved_task_config_integrity.actual.file.expected')"
            }
        }
        $ev = $null
        $rowExit = 40
        $rowVerdict = 'REFUSED_TASKCONFIG_INTEGRITY ×2 + REFUSED_TASKCONFIG_MISMATCH ×1'
        $rowTrial = ''
        $rowSched = ''
        $rowSampling = ''
        $rowConv = ''
        $rowT0 = ''
        $rowStops = @()
        $rowMaxDev = $null
        $rowElapsed = [math]::Round($elapsed, 1)
    }
    else {
        $wdArgs = New-WdArgs -Name $name -OutDir $out -BaseUrl $baseUrl -Cfg $cfg
        $run = Invoke-WdRun -OutDir $out -WdArgs $wdArgs
        $ev = Read-WdEvidence $run
        $sum = $ev.summary
        $arm = $ev.arm
        $stops = $ev.stops
        $states = $ev.states
        $deadlineStops = $ev.deadlineStops
        $earlyStops = $ev.earlyStops
        $maxAbsDev = $ev.maxAbsDev
        $code = $ev.exit

        switch ($name) {
            'M0' {
                Assert-Wd 'exit=0（STOPPED_CLEAN）' ($code -eq 0) "exit=$code"
                Assert-Wd 'verdict=STOPPED_CLEAN' ((Get-Nested $sum 'verdict') -eq 'STOPPED_CLEAN') "verdict=$(Get-Nested $sum 'verdict')"
                Assert-Wd '调度轴 PASS' ((Get-Nested $sum 'axes.schedule.verdict') -eq 'PASS') "schedule=$(Get-Nested $sum 'axes.schedule.verdict')"
                Assert-Wd '采集轴 VALID' ((Get-Nested $sum 'axes.sampling.verdict') -eq 'VALID') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
                Assert-Wd '收敛轴 CONVERGED（HostChecks 强制生效）' ((Get-Nested $sum 'axes.convergence.state') -eq 'CONVERGED') "convergence=$(Get-Nested $sum 'axes.convergence.state')"
                Assert-Wd '截止 Stop 准时（|dev| ≤ 1000 ms）' (($deadlineStops.Count -gt 0) -and ($null -ne $maxAbsDev) -and ($maxAbsDev -le 1000)) "stops=$($deadlineStops.Count) maxAbsDev=$maxAbsDev"
                Assert-Wd 'Start 已确认（returned/200）' (((Get-Nested $sum 'start.outcome') -eq 'returned') -and ((Get-Nested $sum 'start.uncertain') -eq $false)) "start=$(Get-Nested $sum 'start.outcome')"
                Assert-Wd '工作流全部正常' ((Get-Nested $sum 'workers.abort_count') -eq 0) "aborts=$(Get-Nested $sum 'workers.abort_count')"
                Assert-Wd 'trial=PASS' ((Get-Nested $sum 'trial') -eq 'PASS') "trial=$(Get-Nested $sum 'trial')"
            }
            'M1' {
                Assert-Wd 'exit=2（STOPPED_OBS_INVALID）' ($code -eq 2) "exit=$code"
                Assert-Wd '调度轴 PASS（Stop 仍准时）' (((Get-Nested $sum 'axes.schedule.verdict') -eq 'PASS') -and ($maxAbsDev -le 1000)) "schedule=$(Get-Nested $sum 'axes.schedule.verdict') maxAbsDev=$maxAbsDev"
                Assert-Wd '采集轴 INVALID_OBSERVATION_GAP' ((Get-Nested $sum 'axes.sampling.verdict') -eq 'INVALID_OBSERVATION_GAP') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
                Assert-Wd '单次 GET 阻塞 ≥30 s（真解耦，不是超时兜底）' ([double](Get-Nested $sum 'observation.max_req_ms') -ge 30000) "max_req_ms=$(Get-Nested $sum 'observation.max_req_ms')"
                Assert-Wd '收敛轴 CONVERGED（确实停住了）' ((Get-Nested $sum 'axes.convergence.state') -eq 'CONVERGED') "convergence=$(Get-Nested $sum 'axes.convergence.state')"
                Assert-Wd 'trial=INVALID_SAMPLING（调度 PASS 但采集无效）' ((Get-Nested $sum 'trial') -eq 'INVALID_SAMPLING') "trial=$(Get-Nested $sum 'trial')"
                Assert-Wd '存在 DEADLINE Stop' ($deadlineStops.Count -gt 0) "stops=$($deadlineStops.Count)"
            }
            'M2' {
                Assert-Wd 'exit=3（STOPPED_START_UNCERTAIN）' ($code -eq 3) "exit=$code"
                Assert-Wd 'Start 结果不确定' ((Get-Nested $sum 'start.uncertain') -eq $true) "uncertain=$(Get-Nested $sum 'start.uncertain') outcome=$(Get-Nested $sum 'start.outcome')"
                Assert-Wd '调度轴 PASS（Start 超时不影响截止 Stop）' (((Get-Nested $sum 'axes.schedule.verdict') -eq 'PASS') -and ($maxAbsDev -le 1000)) "schedule=$(Get-Nested $sum 'axes.schedule.verdict') maxAbsDev=$maxAbsDev"
                Assert-Wd '采集轴 VALID' ((Get-Nested $sum 'axes.sampling.verdict') -eq 'VALID') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
                Assert-Wd '收敛轴 CONVERGED' ((Get-Nested $sum 'axes.convergence.state') -eq 'CONVERGED') "convergence=$(Get-Nested $sum 'axes.convergence.state')"
            }
            'M3' {
                # 端点运行中失联：Stop 请求发不出去（http=0）⇒ 既不能判"停净"，也不能假装停住了。
                Assert-Wd '退出码非 0（不得静默通过）' ($code -ne 0) "exit=$code"
                Assert-Wd '调度轴 PASS（Stop 请求仍按 T0+30 s 发出）' (((Get-Nested $sum 'axes.schedule.verdict') -eq 'PASS') -and ($maxAbsDev -le 1000)) "schedule=$(Get-Nested $sum 'axes.schedule.verdict') maxAbsDev=$maxAbsDev"
                Assert-Wd '采集轴 INVALID_*（观测全失败，不得判有效）' ((Get-Nested $sum 'axes.sampling.verdict') -like 'INVALID_*') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
                Assert-Wd '收敛轴 ∈ {NOT_CONVERGED, INDETERMINATE}（没看到 ≠ 已停）' (@('NOT_CONVERGED', 'INDETERMINATE') -contains [string](Get-Nested $sum 'axes.convergence.state')) "convergence=$(Get-Nested $sum 'axes.convergence.state')"
                Assert-Wd 'verdict ≠ STOPPED_CLEAN' ((Get-Nested $sum 'verdict') -ne 'STOPPED_CLEAN') "verdict=$(Get-Nested $sum 'verdict')"
                Assert-Wd 'Stop 请求确实失败（http=0，不是"假装停住了"）' ((@($stops | Where-Object { [int]$_.http_status -eq 0 }).Count) -gt 0) "stops_http0=$(@($stops | Where-Object { [int]$_.http_status -eq 0 }).Count)"
                Assert-Wd '桩确实在运行中被拔掉（SHUTDOWN_BY_STUB 事件）' ((Count-Ev $ev 'SHUTDOWN_BY_STUB') -ge 1) "shutdown=$(Count-Ev $ev 'SHUTDOWN_BY_STUB')"
            }
            'M4' {
                Assert-Wd 'exit=0（Stop 挂 15 s 也能停干净）' ($code -eq 0) "exit=$code"
                Assert-Wd 'Stop 请求本身确实挂住（elapsed ≥ 14000 ms）' (($deadlineStops.Count -gt 0) -and ([double](@($deadlineStops)[0].elapsed_ms) -ge 14000)) "elapsed=$(@($deadlineStops)[0].elapsed_ms)"
                $sd = if ($deadlineStops.Count -gt 0) { [double](@($deadlineStops)[0].t_dispatch_mono_ms) } else { 0 }
                $during = @($states | Where-Object { [double]$_.t_mono_ms -gt $sd -and [double]$_.t_mono_ms -lt ($sd + 13000) -and $_.state })
                Assert-Wd 'Stop 挂住期间观测照常（≥20 条样本）' ($during.Count -ge 20) "samples_during_stop=$($during.Count)"
                Assert-Wd '调度轴 PASS' ((Get-Nested $sum 'axes.schedule.verdict') -eq 'PASS') "schedule=$(Get-Nested $sum 'axes.schedule.verdict')"
                Assert-Wd '采集轴 VALID' ((Get-Nested $sum 'axes.sampling.verdict') -eq 'VALID') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
            }
            'M5' {
                Assert-Wd 'exit=4（STOPPED_WITH_RACE）' ($code -eq 4) "exit=$code"
                Assert-Wd '记录到竞态' ((Get-Nested $sum 'race.recorded') -eq $true) "race=$(Get-Nested $sum 'race.count')"
                Assert-Wd '存在 RACE_START_AFTER_STOP 的再次 Stop' (@(Get-StopReasonHit $stops 'RACE_START_AFTER_STOP').Count -gt 0) "race_stops=$(@(Get-StopReasonHit $stops 'RACE_START_AFTER_STOP').Count)"
                Assert-Wd '最终仍收敛（STOPPED + desired=false）' (((Get-Nested $sum 'final.state') -eq 'STOPPED') -and ((Get-Nested $sum 'final.desired_running') -eq $false)) "final=$(Get-Nested $sum 'final.state')/$(Get-Nested $sum 'final.desired_running')"
            }
            'M6' {
                $commStop = @(Get-StopReasonHit $earlyStops '^S[12]_COMM_FAIL$')
                Assert-Wd 'exit=5（STOPPED_BY_EARLY_TRIGGER）' ($code -eq 5) "exit=$code"
                Assert-Wd '触发 S1/S2 并提前 Stop' ($commStop.Count -gt 0) "comm_stops=$($commStop.Count)"
                $firstComm = if ($commStop.Count -gt 0) { @($commStop)[0] } else { $null }
                $earlyRel = if ($firstComm) { [math]::Round([double]$firstComm.t_dispatch_mono_ms - [double]$firstComm.t0_mono_ms, 1) } else { $null }
                Assert-Wd '首次确认即停（≤ T0+15 s，不设连续 N 次宽限）' (($null -ne $earlyRel) -and ($earlyRel -le 15000)) "t_rel=$earlyRel ms"
                Assert-Wd '采集轴 VALID' ((Get-Nested $sum 'axes.sampling.verdict') -eq 'VALID') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
                Assert-Wd '截止 Stop 仍然发出' ($deadlineStops.Count -gt 0) "deadline_stops=$($deadlineStops.Count)"
            }
            'M7' {
                Assert-Wd 'exit=0（ARMED_READY）' ($code -eq 0) "exit=$code"
                Assert-Wd 'ARMED 判定' ((Get-Nested $arm 'verdict') -eq 'ARMED_READY') "verdict=$(Get-Nested $arm 'verdict')"
                Assert-Wd '保持时间 ≥1500 ms（旧版此刻已 FIRED）' ([double](Get-Nested $arm 'arm.hold_ms') -ge 1500) "hold_ms=$(Get-Nested $arm 'arm.hold_ms')"
                Assert-Wd '从未发出 Start' ((Get-Nested $arm 'arm.start_dispatched') -eq $false) "start_dispatched=$(Get-Nested $arm 'arm.start_dispatched')"
                Assert-Wd '未启动任何执行流' ((Get-Nested $arm 'arm.flows_started') -eq 0) "flows=$(Get-Nested $arm 'arm.flows_started')"
                Assert-Wd '未写 t0.stamp' ((-not $ev.t0Exists) -and ((Get-Nested $arm 'arm.t0_written') -eq $false)) "t0=$($ev.t0Exists)"
                Assert-Wd '零副作用：桩未收到任何 START' ((Count-Ev $ev 'START_begin') -eq 0) "START_begin=$(Count-Ev $ev 'START_begin')"
                Assert-Wd '零副作用：桩未收到任何 STOP' ((Count-Ev $ev 'STOP_arrive') -eq 0) "STOP_arrive=$(Count-Ev $ev 'STOP_arrive')"
                Assert-Wd '确实做了只读探活（GET ≥1）' ((Count-Ev $ev 'GET_end') -ge 1) "GET_end=$(Count-Ev $ev 'GET_end')"
                Assert-Wd 'ARMED 也校验可核验项（G6/G7/任务配置 verified）' ((Get-Nested $arm 'credentials.overall') -eq 'verified') "creds=$(Get-Nested $arm 'credentials.overall')"
                Assert-Wd '无 fire 证据文件（summary.json 不应出现）' (-not $ev.summaryExists) "summary=$($ev.summaryExists)"
            }
            'M8' {
                Assert-Wd 'exit=2（STOPPED_OBS_INVALID）' ($code -eq 2) "exit=$code"
                Assert-Wd '采集轴 INVALID_OBSERVATION_GAP' ((Get-Nested $sum 'axes.sampling.verdict') -eq 'INVALID_OBSERVATION_GAP') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
                Assert-Wd '观测心跳判失效（stall_fired=true）' ((Get-Nested $sum 'observation.stall_fired') -eq $true) "stall_fired=$(Get-Nested $sum 'observation.stall_fired')"
                $stallStop = @(Get-StopReasonHit $stops 'OBS_STALL')
                Assert-Wd '提前 Stop 被触发（reason=OBS_STALL）' ($stallStop.Count -gt 0) "stall_stops=$($stallStop.Count)"
                $stallRel = if ($stallStop.Count -gt 0) { [math]::Round([double](@($stallStop)[0].t_dispatch_mono_ms) - [double](@($stallStop)[0].t0_mono_ms), 1) } else { $null }
                Assert-Wd '提前 Stop 发生在阈值后不久（≤ T0+8 s）' (($null -ne $stallRel) -and ($stallRel -le 8000)) "t_rel=$stallRel ms"
                Assert-Wd '截止调度仍工作（T0+30 s 的 DEADLINE Stop 存在且准时）' (($deadlineStops.Count -gt 0) -and ($maxAbsDev -le 1000)) "deadline_stops=$($deadlineStops.Count) maxAbsDev=$maxAbsDev"
                Assert-Wd '调度轴 PASS 与采集 INVALID 同时表达' (((Get-Nested $sum 'axes.schedule.verdict') -eq 'PASS') -and ((Get-Nested $sum 'trial') -eq 'INVALID_SAMPLING')) "schedule=$(Get-Nested $sum 'axes.schedule.verdict') trial=$(Get-Nested $sum 'trial')"
            }
            'M9' {
                Assert-Wd 'exit=9（CONVERGENCE_INDETERMINATE）' ($code -eq 9) "exit=$code"
                Assert-Wd '收敛轴 INDETERMINATE' ((Get-Nested $sum 'axes.convergence.state') -eq 'INDETERMINATE') "convergence=$(Get-Nested $sum 'axes.convergence.state')"
                Assert-Wd '④驱动进程判 indeterminate' ((Get-Nested $sum 'six_items.4_no_driver_process.state') -eq 'indeterminate') "4=$(Get-Nested $sum 'six_items.4_no_driver_process.state')"
                Assert-Wd '⑤TCP 判 indeterminate' ((Get-Nested $sum 'six_items.5_no_established_device.state') -eq 'indeterminate') "5=$(Get-Nested $sum 'six_items.5_no_established_device.state')"
                Assert-Wd '不得判停净 / 不得 exit 0' (((Get-Nested $sum 'verdict') -ne 'STOPPED_CLEAN') -and ($code -ne 0)) "verdict=$(Get-Nested $sum 'verdict')"
                Assert-Wd '采集本身仍有效（vs M8 的观测中断）' ((Get-Nested $sum 'axes.sampling.verdict') -eq 'VALID') "sampling=$(Get-Nested $sum 'axes.sampling.verdict')"
                Assert-Wd 'trial=UNVERIFIED' ((Get-Nested $sum 'trial') -eq 'UNVERIFIED') "trial=$(Get-Nested $sum 'trial')"
            }
            'M10' {
                Assert-Wd 'exit=8（WORKER_ABORTED）' ($code -eq 8) "exit=$code"
                Assert-Wd 'verdict=WORKER_ABORTED' ((Get-Nested $sum 'verdict') -eq 'WORKER_ABORTED') "verdict=$(Get-Nested $sum 'verdict')"
                Assert-Wd '记录到 timer 工作流异常退出' (@(Get-Nested $sum 'workers.aborted') -contains 'timer') "aborted=$((Get-Nested $sum 'workers.aborted') -join ',')"
                Assert-Wd '不静默继续：立即 Stop（reason=WORKER_ABORT）' (@(Get-StopReasonHit $stops 'WORKER_ABORT').Count -gt 0) "workerstops=$(@(Get-StopReasonHit $stops 'WORKER_ABORT').Count)"
                Assert-Wd 'MON 接管截止调度（DEADLINE_FAILSAFE 存在）' (@(Get-StopReasonHit $stops 'DEADLINE_FAILSAFE').Count -gt 0) "failsafe=$(@(Get-StopReasonHit $stops 'DEADLINE_FAILSAFE').Count)"
                Assert-Wd '控制台给出处置提示' ($ev.console -match '工作流异常退出') 'console 含"工作流异常退出"'
                Assert-Wd 'MON 事件里有 WORKER_ABORT' (@(Get-Nested $sum 'workers.mon_events' | Where-Object { $_.kind -eq 'WORKER_ABORT' -and $_.worker -eq 'timer' }).Count -gt 0) "mon_events=$((Get-Nested $sum 'workers.mon_events').Count)"
            }
            'M11' {
                Assert-Wd 'exit=0（四项凭证校验通过 → 放行并正常执行）' ($code -eq 0) "exit=$code"
                Assert-Wd 'authorization.overall = verified' ((Get-Nested $sum 'authorization.overall') -eq 'verified') "overall=$(Get-Nested $sum 'authorization.overall')"
                Assert-Wd 'G7 与部署二进制**实算摘要**比对通过（sha256 形式）' (((Get-Nested $sum 'authorization.items.g7_deploy_version.state') -eq 'verified') -and ((Get-Nested $sum 'authorization.items.g7_deploy_version.actual.source') -eq 'sha256')) "g7=$(Get-Nested $sum 'authorization.items.g7_deploy_version.detail')"
                Assert-Wd 'G7 比对的正是指定的部署二进制（路径与实算摘要一致）' (((Get-Nested $sum 'authorization.items.g7_deploy_version.actual.binary') -eq $deployBin) -and ((Get-Nested $sum 'authorization.items.g7_deploy_version.actual.sha256') -eq $deploySha)) "binary=$(Get-Nested $sum 'authorization.items.g7_deploy_version.actual.binary')"
                Assert-Wd '任务配置核验通过（revision + point_count + 连接 + 点位 + 指纹）' ((Get-Nested $sum 'authorization.items.task_config.state') -eq 'verified') "task_config=$(Get-Nested $sum 'authorization.items.task_config.detail')"
                Assert-Wd 'G6 签署时间可解析' ((Get-Nested $sum 'authorization.items.g6_signoff.state') -eq 'verified') "g6=$(Get-Nested $sum 'authorization.items.g6_signoff.state')"
                Assert-Wd 'G2：获批配置文件自身摘要 = 签署值（解析前比对）' `
                    (((Get-Nested $sum 'authorization.items.approved_task_config_integrity.state') -eq 'verified') -and ((Get-Nested $sum 'authorization.items.approved_task_config_integrity.actual.file.actual') -eq $approvedSha) -and ((Get-Nested $sum 'authorization.items.approved_task_config_integrity.actual.file.verified_before_parse') -eq $true)) `
                    "file_sha=$(Get-Nested $sum 'authorization.items.approved_task_config_integrity.actual.file.actual') expected=$approvedSha"
                Assert-Wd '授权边界放行' ((Get-Nested $sum 'boundary.allowed') -eq $true) "boundary=$(Get-Nested $sum 'boundary.reason')"
                Assert-Wd 'Start 已发出并确认' ((Get-Nested $sum 'start.outcome') -eq 'returned') "start=$(Get-Nested $sum 'start.outcome')"
                Assert-Wd '完整生命周期正常（STOPPED_CLEAN）' ((Get-Nested $sum 'verdict') -eq 'STOPPED_CLEAN') "verdict=$(Get-Nested $sum 'verdict')"
                Assert-Wd '截止 Stop 准时' (($deadlineStops.Count -gt 0) -and ($maxAbsDev -le 1000)) "maxAbsDev=$maxAbsDev"
            }
            'M16' {
                # ── B3 正向：连接配置 + 全部点位 Key/参数 + 指纹全部一致 ⇒ 放行并正常执行 ──
                Assert-Wd 'exit=0（完整配置一致 → 放行并正常执行）' ($code -eq 0) "exit=$code"
                Assert-Wd 'verdict=STOPPED_CLEAN（真的跑完了整个生命周期）' ((Get-Nested $sum 'verdict') -eq 'STOPPED_CLEAN') "verdict=$(Get-Nested $sum 'verdict')"
                Assert-Wd 'authorization.overall = verified' ((Get-Nested $sum 'authorization.overall') -eq 'verified') "overall=$(Get-Nested $sum 'authorization.overall')"
                Assert-Wd 'G7 用 manifest:<path> 形式也通过（受信清单 + 实算摘要）' (((Get-Nested $sum 'authorization.items.g7_deploy_version.state') -eq 'verified') -and ((Get-Nested $sum 'authorization.items.g7_deploy_version.actual.source') -eq 'manifest')) "g7=$(Get-Nested $sum 'authorization.items.g7_deploy_version.detail')"
                Assert-Wd '受信清单记录的自身摘要可审计' ([string](Get-Nested $sum 'authorization.items.g7_deploy_version.actual.manifest.sha256_self') -match '^[0-9a-f]{64}$') "manifest_sha=$(Get-Nested $sum 'authorization.items.g7_deploy_version.actual.manifest.sha256_self')"
                Assert-Wd 'G1：清单自身摘要 = 签署值（解析前比对，verified_before_parse=true）' `
                    (((Get-Nested $sum 'authorization.items.g7_deploy_version.actual.manifest.integrity.ok') -eq $true) -and ((Get-Nested $sum 'authorization.items.g7_deploy_version.actual.manifest.integrity.expected') -eq $manifestSha) -and ((Get-Nested $sum 'authorization.items.g7_deploy_version.actual.manifest.integrity.verified_before_parse') -eq $true)) `
                    "expected=$(Get-Nested $sum 'authorization.items.g7_deploy_version.actual.manifest.integrity.expected') signed=$manifestSha"
                Assert-Wd 'task_config = verified' ((Get-Nested $sum 'authorization.items.task_config.state') -eq 'verified') "task_config=$(Get-Nested $sum 'authorization.items.task_config.detail')"
                Assert-Wd '实际任务配置指纹 = 获批指纹（独立实现交叉核对）' ((Get-Nested $sum 'authorization.items.task_config.actual.tasks_fingerprint') -eq $fingerprint) "actual=$(Get-Nested $sum 'authorization.items.task_config.actual.tasks_fingerprint') expect=$fingerprint"
                Assert-Wd '逐点核对了全部 12 个点位（不只数量）' (@((Get-Nested $sum 'authorization.items.task_config.actual.point_keys').PSObject.Properties).Count -eq $expectedPoints) "point_keys=$(@((Get-Nested $sum 'authorization.items.task_config.actual.point_keys').PSObject.Properties).Count)"
                Assert-Wd '连接配置 host/port/TSAP/PDU 均与获批一致' (((Get-Nested $sum 'authorization.items.task_config.actual.connection.pdu_length') -eq $conn.pdu_length) -and ((Get-Nested $sum 'authorization.items.task_config.actual.connection.local_tsap') -eq $conn.local_tsap) -and ((Get-Nested $sum 'authorization.items.task_config.actual.connection.remote_tsap') -eq $conn.remote_tsap) -and ((Get-Nested $sum 'authorization.items.task_config.actual.connection.host') -eq $conn.host)) "conn=$(Get-Nested $sum 'authorization.items.task_config.actual.connection')"
                Assert-Wd 'Start 已确认 + 截止 Stop 准时' (((Get-Nested $sum 'start.outcome') -eq 'returned') -and ($deadlineStops.Count -gt 0) -and ($maxAbsDev -le 1000)) "start=$(Get-Nested $sum 'start.outcome') maxAbsDev=$maxAbsDev"
                Assert-Wd '工作流全部正常' ((Get-Nested $sum 'workers.abort_count') -eq 0) "aborts=$(Get-Nested $sum 'workers.abort_count')"
            }
        }

        $rowExit = $code
        $rowVerdict = [string](Get-Nested $sum 'verdict')
        if (-not $rowVerdict -and $arm) { $rowVerdict = [string](Get-Nested $arm 'verdict') }
        $rowTrial = [string](Get-Nested $sum 'trial')
        $rowSched = [string](Get-Nested $sum 'axes.schedule.verdict')
        $rowSampling = [string](Get-Nested $sum 'axes.sampling.verdict')
        $rowConv = [string](Get-Nested $sum 'axes.convergence.state')
        $rowT0 = if ($ev.t0) { Format-Stamp $ev.t0.t0_local } else { '' }
        $rowStops = @($stops | ForEach-Object {
                [pscustomobject]@{
                    n            = $_.n
                    reasons      = (@($_.reasons) -join '+')
                    at           = Format-Stamp $_.t_dispatch_local
                    t_rel_ms     = $(if ($null -ne $_.t0_mono_ms) { [math]::Round([double]$_.t_dispatch_mono_ms - [double]$_.t0_mono_ms, 1) } else { $null })
                    deviation_ms = $_.deviation_ms
                    http         = $_.http_status
                    elapsed_ms   = $_.elapsed_ms
                }
            })
        $rowMaxDev = $maxAbsDev
        $rowElapsed = $ev.elapsed
    }

    $passed = @($script:asserts | Where-Object { $_.ok }).Count
    $total = $script:asserts.Count
    $okAll = ($passed -eq $total) -and ($total -gt 0)
    Write-Host ("  断言：{0}/{1} {2}" -f $passed, $total, $(if ($okAll) { 'PASS' } else { 'FAIL' })) -ForegroundColor $(if ($okAll) { 'Green' } else { 'Red' })
    foreach ($a in $script:asserts) {
        $mark = if ($a.ok) { '[PASS]' } else { '[FAIL]' }
        Write-Host ("   {0} {1}{2}" -f $mark, $a.name, $(if ($a.detail) { "  — $($a.detail)" } else { '' })) -ForegroundColor $(if ($a.ok) { 'DarkGray' } else { 'Red' })
    }

    if ($stubProc) {
        try { Stop-Process -Id $stubProc.Id -Force -ErrorAction SilentlyContinue } catch { }
        Start-Sleep -Milliseconds 200
    }

    $rows += [pscustomobject]@{
        scenario             = $name
        title                = $cfg.title
        mode                 = $cfg.mode
        exit                 = $rowExit
        verdict              = $rowVerdict
        trial                = $rowTrial
        schedule             = $rowSched
        sampling             = $rowSampling
        convergence          = $rowConv
        t0_local             = $rowT0
        stops                = @($rowStops)
        max_abs_deviation_ms = $rowMaxDev
        asserts_passed       = $passed
        asserts_total        = $total
        result               = $(if ($okAll) { 'PASS' } else { 'FAIL' })
        wall_sec             = $rowElapsed
    }
}

# ── 汇总 ──────────────────────────────────────────────────────────────────────────
Write-Host ''
Write-Host '================ stop-watchdog 离线场景汇总 ================' -ForegroundColor Cyan
Write-Host ("{0,-4} {1,-6} {2,-30} {3,-5} {4,-22} {5,-10} {6,-25} {7}" -f 'SCN', 'MODE', 'VERDICT', 'EXIT', 'TRIAL', 'SCHEDULE', 'SAMPLING', 'CONVERGENCE / ASSERT')
foreach ($r in $rows) {
    Write-Host ("{0,-4} {1,-6} {2,-30} {3,-5} {4,-22} {5,-10} {6,-25} {7}  ({8}/{9}) {10}" -f `
            $r.scenario, $r.mode, $r.verdict, $r.exit, $r.trial, $r.schedule, $r.sampling, $r.convergence,
        $r.asserts_passed, $r.asserts_total, $r.result) -ForegroundColor $(if ($r.result -eq 'PASS') { 'Green' } else { 'Red' })
}
Write-Host ''
Write-Host '── 时刻与偏差（T0 / Stop 发出 / 偏差）────────────────────────────'
foreach ($r in $rows) {
    $t0Text = if ($r.t0_local) { $r.t0_local } else { '（无 T0：ARMED / 授权门拒绝）' }
    Write-Host ("{0,-4} T0={1}" -f $r.scenario, $t0Text)
    if (@($r.stops).Count -eq 0) {
        Write-Host ("     Stop: 未发出")
    }
    foreach ($s in $r.stops) {
        Write-Host ("     Stop #{0} {1}  reasons={2}  t_rel={3} ms  deviation={4} ms  http={5}  elapsed={6} ms" -f $s.n, $s.at, $s.reasons, $s.t_rel_ms, $s.deviation_ms, $s.http, $s.elapsed_ms)
    }
}

$suiteSummary = [ordered]@{
    generated_local = (Get-Date).ToString('o')
    host            = [System.Net.Dns]::GetHostName()
    platform        = $(if ($IsWindows) { 'windows' } else { 'linux/other' })
    ps_version      = [string]$PSVersionTable.PSVersion
    python          = $python
    evidence_root   = $EvidenceRoot
    # G7（B2）+ G1：记录本轮实算比对的部署二进制、受信清单**及其自身摘要**
    g7 = [ordered]@{
        deploy_binary      = $deployBin
        deploy_sha256      = $deploySha
        manifest           = $manifestPath
        manifest_sha256    = $manifestSha
        manifest_wrong     = $manifestWrongPath
        m17_attacker_binary   = $attackerBinPath
        m17_attacker_sha256   = $attackerSha
        m17_attacker_manifest = $attackerManifestPath
        m17_tampered_manifest = $tamperedManifestPath
        note               = 'git HEAD/describe 只作辅助信息，不作为 G7 通过依据；manifest 形式另需清单自身摘要（G1）'
    }
    # B3 + G2：获批任务配置、指纹、以及**文件自身摘要**
    task_config = [ordered]@{
        approved           = $approvedPath
        approved_sha256    = $approvedSha
        tasks_fixture      = $tasksFile
        expected_fingerprint = $fingerprint
        expected_revision  = $expectedRevision
        expected_points    = $expectedPoints
        connection         = $conn
        m18_tampered_file  = $approvedTamperedPath
        m18_tampered_sha256 = $approvedTamperedSha
        m18_tampered_tasks_fingerprint = $tamperedFingerprint
    }
    rows            = @($rows)
    passed          = @($rows | Where-Object { $_.result -eq 'PASS' }).Count
    failed          = @($rows | Where-Object { $_.result -ne 'PASS' }).Count
}
$suitePath = Join-Path $EvidenceRoot 'suite-summary.json'
[System.IO.File]::WriteAllText($suitePath, ($suiteSummary | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

Write-Host ''
Write-Host ("通过 {0} / 失败 {1}；机器可读汇总：{2}" -f $suiteSummary.passed, $suiteSummary.failed, $suitePath) -ForegroundColor $(if ($suiteSummary.failed -eq 0) { 'Green' } else { 'Red' })
Write-Host '==========================================================='
if ($suiteSummary.failed -gt 0) { exit 1 }
exit 0
