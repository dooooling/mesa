<#
.SYNOPSIS
  stop-watchdog 离线场景驱动（M0–M12）—— 一条命令跑完全部离线故障模拟并给出断言汇总。

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

.PARAMETER Scenario
  只跑指定场景，例如 -Scenario M7,M12。缺省 = M0–M12 全部。

.PARAMETER EvidenceRoot
  证据根目录，缺省 target/validation/stop-watchdog（已被 .gitignore 忽略）。

.PARAMETER KeepEvidence
  保留上一轮证据目录（默认每次重建，避免旧证据冒充新结论）。

.PARAMETER PythonExe
  指定 python 解释器（缺省自动探测 python3 / python / $env:PYTHON）。

.PARAMETER List
  只列出场景，不执行。

.EXAMPLE
  pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1

.EXAMPLE
  pwsh -NoProfile -File tests/watchdog/run-watchdog-scenarios.ps1 -Scenario M7,M11,M12

.NOTES
  Windows / Linux 均可运行（pwsh 7+ 与 python3）。CI 用法见 README.md。
  G7 部署版本凭证由驱动**现场计算**（git rev-parse HEAD；无 git 时退回目标二进制 sha256），
  所以脚本里的 G7 校验是真比对，而不是"随便填个值就过"。
#>
[CmdletBinding()]
param(
    [string[]]$Scenario = @(),
    [string]$EvidenceRoot,
    [switch]$KeepEvidence,
    [string]$PythonExe,
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

# ── 凭证取值：G7 必须与**真实**仓库 HEAD（或部署二进制指纹）一致，否则脚本会正确拒绝 ──
$g7Value = $null
try {
    # 注意：&(...) 是子作用域，$LASTEXITCODE 不会回传，这里直接校验取值形态（7–40 位十六进制）。
    $h = (& git -C $repo rev-parse HEAD 2>$null | Select-Object -First 1)
    if (-not [string]::IsNullOrWhiteSpace($h) -and (([string]$h).Trim() -match '^[0-9a-fA-F]{7,40}$')) { $g7Value = ([string]$h).Trim() }
}
catch { }
if (-not $g7Value) {
    foreach ($rel in @('target/release/mesad', 'target/release/mesad.exe', 'target/debug/mesad', 'target/debug/mesad.exe')) {
        $bin = Join-Path $repo $rel
        if (Test-Path $bin) {
            $g7Value = 'sha256:' + (Get-FileHash -Algorithm SHA256 -LiteralPath $bin).Hash.ToLowerInvariant()
            break
        }
    }
}
if (-not $g7Value) {
    Write-Host '[warn] 既读不到 git HEAD 也找不到部署二进制：G7 校验将无法通过，fire 场景会被脚本以 40 拒绝' -ForegroundColor Yellow
    $g7Value = 'UNVERIFIED'
}
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
$commonStubCfg = @('--task-revision', $expectedRevision, '--point-count', [string]$expectedPoints)
$SCEN = [ordered]@{
    M0  = @{ title = '正常路径'; stub = @('--port', '18080') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18080 }
    M1  = @{ title = '观测阻塞 36s'; stub = @('--port', '18081', '--get-delay-running', '36') + $commonStubCfg; wd = @('-ObserveTimeoutSec', '45', '-ObserveStallSec', '0'); mode = 'fire'; port = 18081 }
    M2  = @{ title = 'Start 超时 20s'; stub = @('--port', '18082', '--start-delay', '20') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18082 }
    M3  = @{ title = '端点运行中失联（Stop 发不出去）'; stub = @('--port', '18083', '--shutdown-after', '4') + $commonStubCfg; wd = @('-ObserveMaxSec', '45', '-ObserveTimeoutSec', '2'); mode = 'fire'; port = 18083 }
    M4  = @{ title = 'Stop 自身挂起 15s'; stub = @('--port', '18084', '--stop-delay', '15') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18084 }
    M5  = @{ title = '迟到 Start 竞态'; stub = @('--port', '18085', '--race-after-stop', '5') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18085 }
    M6  = @{ title = '首次通信失败即停'; stub = @('--port', '18086', '--fail-after', '6') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18086 }
    M7  = @{ title = 'ARMED 误启动防护'; stub = @('--port', '18087') + $commonStubCfg; wd = @(); mode = 'arm'; armHold = 3; port = 18087 }
    M8  = @{ title = 'OBS 心跳失效'; stub = @('--port', '18088', '--get-delay', '60') + $commonStubCfg; wd = @('-ObserveTimeoutSec', '5', '-ObserveMaxSec', '40'); mode = 'fire'; port = 18088 }
    M9  = @{ title = 'HostChecks 查询失败'; stub = @('--port', '18089') + $commonStubCfg; wd = @('-TestHostProbeError', 'both'); mode = 'fire'; port = 18089 }
    M10 = @{ title = '工作线程异常退出'; stub = @('--port', '18090') + $commonStubCfg; wd = @('-TestAbortFlow', 'timer', '-TestAbortAfterSec', '3'); mode = 'fire'; port = 18090 }
    M11 = @{ title = '授权门正向（四项凭证校验通过）'; stub = @('--port', '18091') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18091 }
    M12 = @{ title = '授权门负向（缺凭证/任务配置不匹配）'; stub = @('--port', '18092') + $commonStubCfg; wd = @(); mode = 'fire'; port = 18092; phases = $true }
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
function New-WdArgs {
    param([string]$Name, [string]$OutDir, [string]$BaseUrl, $Cfg)
    $a = @()
    if ($Cfg.mode -eq 'arm') {
        # ARMED：只自检。凭证都给（用于演示"凭证齐备也不等于可以启动端点"），但**不给** -Authorized。
        $a += @('-Mode', 'arm', '-ArmHoldSec', [string]$Cfg.armHold,
            '-G6Signoff', 'SIM-G6', '-G6SignedAt', $g6SignedAt,
            '-G7DeployVersion', $g7Value,
            '-ExpectedTaskRevision', $expectedRevision, '-ExpectedPointCount', [string]$expectedPoints)
    }
    else {
        $a += @('-Mode', 'fire', '-Authorized', '-AuthorizedBy', 'SIM-AUTHZ',
            '-AuthzTicket', "SIM-$Name",
            '-G6Signoff', 'SIM-G6', '-G6SignedAt', $g6SignedAt,
            '-G7DeployVersion', $g7Value,
            '-ExpectedTaskRevision', $expectedRevision, '-ExpectedPointCount', [string]$expectedPoints)
    }
    $a += @('-BaseUrl', $BaseUrl, '-EndpointId', $endpoint, '-OutDir', $OutDir, '-Tag', $Name)
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

    if ($cfg.ContainsKey('phases') -and $cfg.phases) {
        # ── M12：授权门负向，两段子用例（都必须 exit 40 且不发 Start）───────────────
        $outA = Join-Path $out 'a-missing-authorization'
        $outB = Join-Path $out 'b-taskconfig-mismatch'
        $argsA = @('-Mode', 'fire',
            '-AuthzTicket', 'SIM-M12a',
            '-G6Signoff', 'SIM-G6', '-G6SignedAt', $g6SignedAt,
            '-G7DeployVersion', $g7Value,
            '-ExpectedTaskRevision', $expectedRevision, '-ExpectedPointCount', [string]$expectedPoints,
            '-BaseUrl', $baseUrl, '-EndpointId', $endpoint, '-OutDir', $outA, '-Tag', 'M12a')
        $argsB = @('-Mode', 'fire', '-Authorized', '-AuthorizedBy', 'SIM-AUTHZ',
            '-AuthzTicket', 'SIM-M12b',
            '-G6Signoff', 'SIM-G6', '-G6SignedAt', $g6SignedAt,
            '-G7DeployVersion', $g7Value,
            '-ExpectedTaskRevision', 'rev99-DOES-NOT-MATCH', '-ExpectedPointCount', [string]$expectedPoints,
            '-BaseUrl', $baseUrl, '-EndpointId', $endpoint, '-OutDir', $outB, '-Tag', 'M12b')
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
    else {
        $wdArgs = New-WdArgs -Name $name -OutDir $out -BaseUrl $baseUrl -Cfg $cfg
        $wdArgs += $cfg.wd
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
                Assert-Wd 'G7 与仓库 HEAD/二进制指纹**实际比对**通过' ((Get-Nested $sum 'authorization.items.g7_deploy_version.state') -eq 'verified') "g7=$(Get-Nested $sum 'authorization.items.g7_deploy_version.detail')"
                Assert-Wd '任务配置核验通过（revision + point_count 实读比对）' ((Get-Nested $sum 'authorization.items.task_config.state') -eq 'verified') "task_config=$(Get-Nested $sum 'authorization.items.task_config.detail')"
                Assert-Wd 'G6 签署时间可解析' ((Get-Nested $sum 'authorization.items.g6_signoff.state') -eq 'verified') "g6=$(Get-Nested $sum 'authorization.items.g6_signoff.state')"
                Assert-Wd '授权边界放行' ((Get-Nested $sum 'boundary.allowed') -eq $true) "boundary=$(Get-Nested $sum 'boundary.reason')"
                Assert-Wd 'Start 已发出并确认' ((Get-Nested $sum 'start.outcome') -eq 'returned') "start=$(Get-Nested $sum 'start.outcome')"
                Assert-Wd '完整生命周期正常（STOPPED_CLEAN）' ((Get-Nested $sum 'verdict') -eq 'STOPPED_CLEAN') "verdict=$(Get-Nested $sum 'verdict')"
                Assert-Wd '截止 Stop 准时' (($deadlineStops.Count -gt 0) -and ($maxAbsDev -le 1000)) "maxAbsDev=$maxAbsDev"
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
    g7_credential   = $g7Value
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
