<#
.SYNOPSIS
  828D 一期首轮限定采样 —— 独立 watchdog（G6-ARMED 的执行体，也是 §5.1 实现要点的落地）。

.DESCRIPTION
  设计契约（先读，再改本文件）：
    docs/validation/828d-phase1-first-limited-sampling-application.md
      §4.6     G6-ARMED 七条判据（本脚本必须能逐条自证）
      §4.6.1   M1–M3 离线故障模拟（本脚本的 -BaseUrl 指向本地假端点即可复现）
      §5.1     实现要点 1–7（独立进程 / T0 自生成 / 独占 Start / 截止 Stop / 观测 / 双向解耦 / 退出码）
      §5.3     Stop 时间约束与"停干净"六项判据、Start/Stop 竞态处置

  本脚本只做四件事，别的都不做：
    1) 由自己**独占**发出 Start，并在**同一时刻**用自身单调时钟生成 T0（写 t0.stamp）；
       **不接受**任何外部传入的 T0 或截止时间——没有这样的参数，也不读任何外部文件里的 T0。
    2) 在 T0 + 30 s **无条件**发出 Stop；30 s 是脚本内常量（$script:DEADLINE_OFFSET_MS），无参数入口。
    3) 每 200 ms 观测 GET /api/v1/endpoints/{id}，逐行追加 watchdog-states.jsonl。
    4) 以"**最终观测到** STOPPED + desired_running=false"（而不是"Stop 已发出"）给出退出码。

  ── "计时/Stop 与观测解耦"的落地方式（G6-ARMED-3，本文件的核心）─────────────────
  四个**独立执行流**（同一进程内的独立 runspace，各自独立线程，互不等待、互不 join）：

      OBS   观测流：GET 端点、写 jsonl、判定 S1/S2/S3 触发条件。可以被 HTTP 阻塞任意久。
      TIMER 计时流：**只读单调时钟**，从不读端点状态、从不发 HTTP；到 T0+30 s 置信号。
      STOP  停止流：等信号后**发出** Stop 请求；Stop 自身可能挂 75–80 s，不阻塞任何人。
      START 启动流：等放行信号后发 Start；Start 可能 10 s 超时，不阻塞主流程。

  主流程（协调者）只做"读共享样本 → 判定触发 → 入队"，**自己从不发 HTTP**。
  因此：任一条 HTTP 请求（Start / Stop / GET）挂起，都不会推迟 T0+30 s 的 Stop 发出；
  反过来 Stop 挂住时观测也照常进行（双向解耦，§5.1-6）。

  关键点：**TIMER 流不读端点状态**，所以"何时 Stop"不依赖被测流程的循环或状态——
  这是 §4.6 ARMED-2/3 与 §5.1-1 的硬性要求，改动本文件时不得破坏。

.PARAMETER BaseUrl
  Core 的 REST 根地址。正式执行指向 http://127.0.0.1:8132；
  §4.6.1 离线模拟指向本地假端点（如 http://127.0.0.1:18081）或不可达地址（http://127.0.0.1:1）。

.PARAMETER ObserveTimeoutSec
  观测 GET 的**客户端**超时，生产默认 5 s。
  M1（同步阻塞）必须放宽到大于阻塞时长（如 45），否则只是"超时兜底"、不构成解耦证明（§4.6.1）。

.PARAMETER ObserveMaxSec
  观测总上限，默认 120 s（§5.1-5）。到点即停观测，不延长。

.PARAMETER OutDir
  证据目录。正式执行必须显式给 target\validation\828d-r1\round1-<UTC 时间戳>（§6.1）。

.PARAMETER SelfTest
  G6-ARMED-4 通路自检：对**当前已 STOPPED** 的端点调用一次 POST .../stop，
  要求 200 且 was_running=false，写 stop-selftest.json。**只能在 Start 之前做**（§4.6.1 纪律）。
  自检不生成 t0.stamp、不发 Start。

.PARAMETER HostChecks
  启用"停干净"六项里的 ④驱动进程空 / ⑤到机床无 Established 两项**本机**检查。
  离线模拟（假端点）不要加：那两项对假端点无意义，会记为 N/A（并在 summary.json 注明原因）。

.EXAMPLE
  # 正式执行（Start 之前先自检）
  pwsh -File scripts/stop-watchdog.ps1 -EndpointId lx-nck-tags-190-connection `
       -OutDir target\validation\828d-r1\round1-20261021T010203Z -SelfTest

.EXAMPLE
  # 离线 M1：假端点 GET 挂起 36 s，观测超时放宽到 45 s
  pwsh -File scripts/stop-watchdog.ps1 -BaseUrl http://127.0.0.1:18081 `
       -ObserveTimeoutSec 45 -OutDir target\validation\stop-watchdog-m1

.NOTES
  严禁指向真实机床：离线模拟只用 127.0.0.1 的假端点或不可达地址。
  本脚本**不 import** 任何驱动/Core 代码，只经 REST 交互（§5.1-1）。
  退出码：0 = 六项判据（必选项）全过；1 = 未达成（已打印 §5.3 隔离处置流程）。
#>
[CmdletBinding()]
param(
    [string]$BaseUrl = 'http://127.0.0.1:8132',
    [string]$EndpointId = 'lx-nck-tags-190-connection',
    [string]$OutDir,
    [ValidateRange(1, 600)][int]$ObserveTimeoutSec = 5,
    [ValidateRange(10, 3600)][int]$ObserveMaxSec = 120,
    [ValidateRange(1, 600)][int]$StopTimeoutSec = 120,
    [ValidateRange(1, 120)][int]$StartTimeoutSec = 10,
    [switch]$SelfTest,
    [switch]$HostChecks,
    [string]$Tag = 'run'
)

$ErrorActionPreference = 'Stop'

#region ── 常量（硬性：30 s 截止为脚本内常量，不提供参数入口）─────────────────────
# §4.6-G6-ARMED-2：截止偏移 30 s 以常量固化在脚本内；§4.6："截止时刻 = watchdog 自己记录的
# T0 + 30 s，不接受外部（人工、主流程、抓证脚本）指定或修正"。故此处**故意**不是参数。
$script:DEADLINE_OFFSET_MS = 30000
# §5.1-5：每 200 ms 观测一次。
$script:OBSERVE_INTERVAL_MS = 200
# §5.3-⑥：自首次观测到 STOPPED 起连续 10 s 无新 epoch、无 RECONNECTING。
$script:SETTLE_MS = 10000
# §5.3：Stop 发出后若见 RUNNING / epoch 变化，再 Stop，最多 3 次。
$script:MAX_STOPS = 3
# §7-S1/S2：端点 detail 里的通信失败特征（首次确认即停，不设"连续 N 次"宽限）。
$script:COMM_FAIL_PATTERN = '(?i)(S7_JOB_0x8201|S7_JOB_[0-9A-Fa-f]{4}|COTP_RECV_FAIL|COTP_SEND_FAIL|early eof|CONNECT_TIMEOUT|POLL_FAIL|RECV_FAIL|SEND_FAIL|CONNECT_FAIL)'
#endregion

#region ── 跨执行流共享的辅助函数 ─────────────────────────────────────────────────
# 这些函数在"主流程"和"子 runspace"里都要用。子 runspace 有独立的函数表（不能直接调父作用域的
# 函数），因此把函数体放在一个字符串里：主流程 Invoke-Expression 一次，子流把同一段文本拼在
# 自己脚本前面编译。这样只有一份实现，不会出现"日志格式两处不一致"。
$script:CommonPrelude = @'
function Add-WdLog {
    param($Ctx, [string]$Level, [string]$Message)
    $ts = (Get-Date).ToString('o')
    $line = "- ``{0}`` [{1}] {2}" -f $ts, $Level, $Message
    [System.Threading.Monitor]::Enter($Ctx.logLock)
    try {
        [System.IO.File]::AppendAllText($Ctx.logPath, $line + "`n", [System.Text.UTF8Encoding]::new($false))
    }
    finally { [System.Threading.Monitor]::Exit($Ctx.logLock) }
}

# 追加一行 JSON（jsonl）。多线程写同一文件，用 logLock 串行化，避免行被截断。
function Write-WdJsonl {
    param($Ctx, [string]$Path, $Record)
    $line = ($Record | ConvertTo-Json -Compress -Depth 8)
    [System.Threading.Monitor]::Enter($Ctx.logLock)
    try {
        [System.IO.File]::AppendAllText($Path, $line + "`n", [System.Text.UTF8Encoding]::new($false))
    }
    finally { [System.Threading.Monitor]::Exit($Ctx.logLock) }
}

# 单调时钟（毫秒）。整个脚本只有一个 Stopwatch 实例，所有执行流读同一个时间基准。
function Get-WdMono {
    param($Ctx)
    return [double]$Ctx.sw.Elapsed.TotalMilliseconds
}
'@
Invoke-Expression $script:CommonPrelude
#endregion

#region ── 运行目录与日志骨架 ─────────────────────────────────────────────────────
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path 'target\validation' ('watchdog-' + (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ'))
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path

$runLogPath = Join-Path $OutDir 'run-log.md'
$t0Path = Join-Path $OutDir 't0.stamp'
$statesPath = Join-Path $OutDir 'watchdog-states.jsonl'
$stopsPath = Join-Path $OutDir 'watchdog-stops.jsonl'
$summaryPath = Join-Path $OutDir 'summary.json'
$selfTestPath = Join-Path $OutDir 'stop-selftest.json'

$procStart = Get-Date
$hostName = [System.Net.Dns]::GetHostName()

$header = @(
    '# stop-watchdog 运行日志'
    ''
    "- 脚本：``scripts/stop-watchdog.ps1``"
    "- PID：$PID（独立进程；本行即 G6-ARMED-1 的 PID 证据）"
    "- 进程启动时刻（本地）：$($procStart.ToString('o'))"
    "- PowerShell：$($PSVersionTable.PSVersion)"
    "- 主机：$hostName"
    "- 参数：BaseUrl=$BaseUrl EndpointId=$EndpointId ObserveTimeoutSec=$ObserveTimeoutSec ObserveMaxSec=$ObserveMaxSec StopTimeoutSec=$StopTimeoutSec StartTimeoutSec=$StartTimeoutSec SelfTest=$SelfTest HostChecks=$HostChecks Tag=$Tag"
    "- 截止偏移：$($script:DEADLINE_OFFSET_MS) ms（脚本内常量，不接受外部指定）"
    '- 说明：T0 与截止时刻**只**由本进程在 FIRED 时刻生成；人工/主流程/抓证脚本不得预先填写或修正。'
    ''
    '## 事件'
    ''
) -join "`n"
[System.IO.File]::WriteAllText($runLogPath, $header + "`n", [System.Text.UTF8Encoding]::new($false))

# 共享上下文：所有字段在子流启动前一次性建好（子流只读键名、不新增键）。
$ctx = @{
    baseUrl            = $BaseUrl.TrimEnd('/')
    endpointId         = $EndpointId
    observeTimeoutSec  = $ObserveTimeoutSec
    observeMaxSec      = $ObserveMaxSec
    stopTimeoutSec     = $StopTimeoutSec
    startTimeoutSec    = $StartTimeoutSec
    observeIntervalMs  = $script:OBSERVE_INTERVAL_MS
    settleMs           = $script:SETTLE_MS
    maxStops           = $script:MAX_STOPS
    sw                 = [System.Diagnostics.Stopwatch]::StartNew()
    logLock            = [object]::new()
    logPath            = $runLogPath
    statesPath         = $statesPath
    stopsPath          = $stopsPath
    armed              = [System.Threading.ManualResetEventSlim]::new($false)
    startGo            = [System.Threading.ManualResetEventSlim]::new($false)
    obsDone            = [System.Threading.ManualResetEventSlim]::new($false)
    shutdown           = [System.Threading.ManualResetEventSlim]::new($false)
    wake               = [System.Threading.ManualResetEventSlim]::new($false)
    stopSignal         = [System.Threading.SemaphoreSlim]::new(0)
    stopQueue          = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()
    history            = [System.Collections.Concurrent.ConcurrentQueue[object]]::new()
    stopRecords        = [System.Collections.Concurrent.ConcurrentQueue[object]]::new()
    d                  = [System.Collections.Concurrent.ConcurrentDictionary[string, object]]::new()
}

# 晚发布的共享标量一律放 $ctx.d（并发字典），避免多线程写普通哈希表。
foreach ($k in @('t0Mono', 'deadlineMono', 't0UnixNs', 'deadlineFireMono', 'stopFirstMono',
        'stopCount', 'stopInFlight', 'startDispatchMono', 'startStatus',
        'startElapsedMs', 'settleOk', 'settleSamples', 'settleResets', 'obsSamples',
        'obsFailures', 'obsMaxReqMs', 'obsFirstStoppedMono', 'devicePort', 'raceCount',
        'postStopEpoch', 'stopLastMono')) {
    $ctx.d[$k] = 0
}
$ctx.d['startOutcome'] = 'pending'
$ctx.d['startErr'] = ''
$ctx.d['startBody'] = ''
$ctx.d['startDispatchLocal'] = ''
$ctx.d['deviceHost'] = ''
$ctx.d['driverId'] = ''
$ctx.d['postStopEpoch'] = ''
$ctx.d['settleOk'] = $false

Add-WdLog $ctx 'INFO' "启动：PID=$PID BaseUrl=$($ctx.baseUrl) EndpointId=$EndpointId OutDir=$OutDir"
Add-WdLog $ctx 'INFO' "常量：截止偏移=$($script:DEADLINE_OFFSET_MS) ms 观测间隔=$($script:OBSERVE_INTERVAL_MS) ms 稳定窗口=$($script:SETTLE_MS) ms Stop 上限=$($script:MAX_STOPS) 次"
Write-Host ("[start] PID={0}  BaseUrl={1}  Endpoint={2}  OutDir={3}" -f $PID, $ctx.baseUrl, $EndpointId, $OutDir)
#endregion

#region ── G6-ARMED-4：通路自检（只在 Start 之前；不生成 t0.stamp、不发 Start）──────
if ($SelfTest) {
    $uri = "$($ctx.baseUrl)/api/v1/endpoints/$EndpointId"
    $stopUri = "$uri/stop"
    $rec = [ordered]@{
        t_local      = (Get-Date).ToString('o')
        t_unix_ns    = ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000)
        pid          = $PID
        base_url     = $ctx.baseUrl
        endpoint_id  = $EndpointId
        pre_state    = $null
        pre_desired  = $null
        http_status  = 0
        body         = $null
        ok           = $false
        reason       = $null
    }
    try {
        $pre = Invoke-WebRequest -Uri $uri -Method Get -TimeoutSec $ObserveTimeoutSec -SkipHttpErrorCheck -ErrorAction Stop
        $preObj = $pre.Content | ConvertFrom-Json
        $rec.pre_state = [string]$preObj.runtime.state
        $rec.pre_desired = $preObj.desired_running
        if ($rec.pre_state -ne 'STOPPED') {
            # 纪律：自检只能在 Start 之前做（此时端点本已 STOPPED）；运行中自检会制造"已验证"错觉。
            $rec.reason = "端点当前 state=$($rec.pre_state)，不是 STOPPED；自检拒绝执行（§4.6.1 纪律）"
        }
        else {
            $r = Invoke-WebRequest -Uri $stopUri -Method Post -TimeoutSec $StopTimeoutSec -SkipHttpErrorCheck -ErrorAction Stop
            $rec.http_status = [int]$r.StatusCode
            $rec.body = [string]$r.Content
            $bodyObj = $null
            try { $bodyObj = $rec.body | ConvertFrom-Json } catch { }
            $wasRunning = $null
            if ($bodyObj -and $bodyObj.PSObject.Properties['was_running']) { $wasRunning = $bodyObj.was_running }
            if ($rec.http_status -eq 200 -and $wasRunning -eq $false) {
                $rec.ok = $true
                $rec.reason = 'POST .../stop 返回 200 且 was_running=false（幂等，core-api:2169-2175）'
            }
            else {
                $rec.reason = "自检失败：http=$($rec.http_status) was_running=$wasRunning（期望 200/false）"
            }
        }
    }
    catch {
        $rec.reason = "自检异常：$($_.Exception.Message)"
    }
    [System.IO.File]::WriteAllText($selfTestPath, ($rec | ConvertTo-Json -Depth 6), [System.Text.UTF8Encoding]::new($false))
    Add-WdLog $ctx ($(if ($rec.ok) { 'INFO' } else { 'ERROR' })) "自检：$($rec.reason)"
    if ($rec.ok) {
        Write-Host "[SelfTest] OK  http=$($rec.http_status) was_running=false  -> $selfTestPath"
        exit 0
    }
    Write-Host "[SelfTest] FAIL  $($rec.reason)  -> $selfTestPath"
    exit 1
}
#endregion

#region ── 四个独立执行流的脚本体 ─────────────────────────────────────────────────
# 说明：每个脚本体都以 param($Ctx) 开头，经 $Body.ToString() 取出源码，在**新的 runspace**里
# 与 $CommonPrelude 拼接后编译执行。runspace 之间只通过 $Ctx 里的并发对象通信。

# ① OBS：观测流。可被 HTTP 阻塞任意久；它的阻塞不影响 TIMER / STOP。
$ObserveBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    $interval = [int]$Ctx.observeIntervalMs
    $maxMs = [int]$Ctx.observeMaxSec * 1000
    $settleMs = [int]$Ctx.settleMs
    $uri = "$($Ctx.baseUrl)/api/v1/endpoints/$($Ctx.endpointId)"
    $obsStart = Get-WdMono $Ctx
    $seq = 0
    $fail = 0
    $maxReq = 0.0
    $firstStopped = 0.0
    $lastStopMono = 0.0
    $settleStart = 0.0
    $settleEpoch = $null
    $settleSamples = 0
    $settleResets = 0
    $settleOk = $false
    $nextTick = $obsStart
    Add-WdLog $Ctx 'INFO' "OBS 启动：GET $uri；间隔 $interval ms；观测超时 $($Ctx.observeTimeoutSec) s；总上限 $($Ctx.observeMaxSec) s"
    try {
        while (-not $Ctx.shutdown.Wait(0)) {
            if ((Get-WdMono $Ctx) - $obsStart -ge $maxMs) {
                Add-WdLog $Ctx 'WARN' "OBS 达到观测总上限 $($Ctx.observeMaxSec) s，停止观测"
                break
            }
            $reqSw = [System.Diagnostics.Stopwatch]::StartNew()
            $status = 0
            $err = $null
            $content = $null
            try {
                $resp = Invoke-WebRequest -Uri $uri -Method Get -TimeoutSec ([int]$Ctx.observeTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
                $status = [int]$resp.StatusCode
                $content = [string]$resp.Content
            }
            catch {
                $err = $_.Exception.Message
            }
            $reqMs = [math]::Round($reqSw.Elapsed.TotalMilliseconds, 1)
            $mono = Get-WdMono $Ctx
            $seq++

            $state = $null
            $desired = $null
            $epoch = $null
            $detail = $null
            if ($content) {
                try {
                    $o = $content | ConvertFrom-Json -ErrorAction Stop
                    if ($o.PSObject.Properties['runtime']) {
                        if ($o.runtime.PSObject.Properties['state']) { $state = [string]$o.runtime.state }
                        if ($null -ne $o.runtime.epoch) { $epoch = [string]$o.runtime.epoch }
                        if ($null -ne $o.runtime.detail) { $detail = [string]$o.runtime.detail }
                    }
                    if ($o.PSObject.Properties['desired_running']) { $desired = [bool]$o.desired_running }
                    # 端点 connection / driver_id 只在首条成功样本里取一次（供"停干净"④⑤本机核对用）。
                    if ($o.PSObject.Properties['driver_id'] -and -not $Ctx.d['driverId']) {
                        $Ctx.d['driverId'] = [string]$o.driver_id
                    }
                    if ($o.PSObject.Properties['connection'] -and [int]$Ctx.d['devicePort'] -eq 0) {
                        if ($o.connection.PSObject.Properties['host']) { $Ctx.d['deviceHost'] = [string]$o.connection.host }
                        if ($o.connection.PSObject.Properties['port']) { $Ctx.d['devicePort'] = [int]$o.connection.port }
                    }
                }
                catch {
                    if (-not $err) { $err = "JSON 解析失败：$($_.Exception.Message)" }
                }
            }

            $t0 = [double]$Ctx.d['t0Mono']
            $rel = $null
            if ($t0 -gt 0) { $rel = [math]::Round($mono - $t0, 1) }
            $rec = [ordered]@{
                seq             = $seq
                t_local         = (Get-Date).ToString('o')
                t_unix_ns       = ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000)
                t_mono_ms       = [math]::Round($mono, 1)
                t_rel_ms        = $rel
                state           = $state
                desired_running = $desired
                epoch           = $epoch
                detail          = $detail
                http_status     = $status
                req_ms          = $reqMs
                err             = $err
            }
            Write-WdJsonl $Ctx $Ctx.statesPath $rec
            $Ctx.history.Enqueue($rec)
            if ($err) { $fail++ }
            if ($reqMs -gt $maxReq) { $maxReq = $reqMs }
            # 观测请求被长时间阻塞本身就是 M1 的证据；记一条（不刷屏：只记 >2 s 的）。
            if ($reqMs -gt 2000) {
                Add-WdLog $Ctx 'WARN' "OBS 单次 GET 耗时 $reqMs ms（观测被阻塞；计时与 Stop 走独立执行流，不受影响）"
            }
            if ($err -and ($fail -eq 1 -or ($fail % 50) -eq 0)) {
                Add-WdLog $Ctx 'WARN' "OBS 第 $fail 次观测失败：$err"
            }

            # ── ⑥ 稳定窗口：只在"Stop 已发出"之后计时，避免把 Start 之前的 STOPPED 当成"停干净"。
            # 每发出一次新的 Stop（竞态补发、或提前触发之后的截止 Stop），窗口都必须**重新计时**：
            # "停干净"只能相对**最后一次** Stop 判定，否则会拿"第一次停住"去担保"后来又被 Start 拉起"。
            $stopLast = [double]$Ctx.d['stopLastMono']
            if ($stopLast -gt $lastStopMono) {
                $lastStopMono = $stopLast
                if ($firstStopped -gt 0) {
                    Add-WdLog $Ctx 'INFO' 'OBS 观测到新的 Stop 已发出，稳定窗口重新计时（相对最后一次 Stop 判定）'
                }
                $firstStopped = 0.0
                $settleStart = 0.0
                $settleEpoch = $null
                $settleSamples = 0
                $settleOk = $false
            }
            $stopFirst = [double]$Ctx.d['stopFirstMono']
            if ($stopFirst -gt 0 -and $mono -ge $stopFirst) {
                if ($state -eq 'STOPPED' -and $desired -eq $false) {
                    if ($firstStopped -le 0) {
                        $firstStopped = $mono
                        $settleStart = $mono
                        $settleEpoch = $epoch
                        $settleSamples = 1
                        $Ctx.d['obsFirstStoppedMono'] = $mono
                        Add-WdLog $Ctx 'INFO' ("OBS 首次观测到 STOPPED + desired_running=false（mono={0:N1} ms，相对 T0={1:N1} ms），开始 {2} s 稳定窗口" -f $mono, ($mono - $t0), ($settleMs / 1000))
                    }
                    else {
                        if ($settleEpoch -and $epoch -and $epoch -ne $settleEpoch) {
                            $settleResets++
                            $settleStart = $mono
                            $settleEpoch = $epoch
                            $settleSamples = 1
                            Add-WdLog $Ctx 'WARN' "OBS 稳定窗口内 epoch 变化（$settleEpoch -> $epoch），窗口重新计时（第 $settleResets 次重置）"
                        }
                        else {
                            $settleSamples++
                        }
                    }
                }
                elseif ($state) {
                    # 已进入稳定窗口却又看到非 STOPPED / RECONNECTING ⇒ 窗口作废，重新计时。
                    if ($firstStopped -gt 0) {
                        $settleResets++
                        $settleStart = $mono
                        $settleSamples = 0
                        Add-WdLog $Ctx 'WARN' "OBS 稳定窗口被打破：state=$state（第 $settleResets 次重置）"
                    }
                    if ($state -eq 'RECONNECTING') {
                        Add-WdLog $Ctx 'WARN' "OBS 观测到 RECONNECTING（相对 T0=$([math]::Round($mono - $t0, 1)) ms）"
                    }
                }
                # 观测不早于截止结束：即使提前触发（S1/S2/S3）已停住，也要等到 T0+30 s 的截止 Stop
                # 发出并复核完才收工——否则"提前停住后又在 30 s 前被迟到 Start 拉起"会被漏掉。
                $deadlineMonoObs = [double]$Ctx.d['deadlineMono']
                if ($firstStopped -gt 0 -and ($mono - $settleStart) -ge $settleMs -and $mono -ge $deadlineMonoObs) {
                    $settleOk = $true
                    Add-WdLog $Ctx 'INFO' ("OBS 稳定窗口完成：{0} s 内无新 epoch、无 RECONNECTING（窗口内样本 {1} 条，重置 {2} 次）" -f ($settleMs / 1000), $settleSamples, $settleResets)
                    break
                }
            }

            # 节拍：按固定 200 ms 对齐；若本次请求已经超时很久，则不再补睡。
            $nextTick += $interval
            $now = Get-WdMono $Ctx
            if ($now -lt $nextTick) {
                [void]$Ctx.wake.Wait([int][math]::Ceiling($nextTick - $now))
            }
            else {
                $nextTick = $now
            }
        }
    }
    finally {
        $Ctx.d['settleOk'] = $settleOk
        $Ctx.d['settleSamples'] = $settleSamples
        $Ctx.d['settleResets'] = $settleResets
        $Ctx.d['obsSamples'] = $seq
        $Ctx.d['obsFailures'] = $fail
        $Ctx.d['obsMaxReqMs'] = $maxReq
        $Ctx.obsDone.Set()
        Add-WdLog $Ctx 'INFO' "OBS 退出：样本 $seq 条，失败 $fail 次，单次最长 $maxReq ms，settleOk=$settleOk"
    }
}

# ② TIMER：计时流。**只读单调时钟**，不读端点状态、不发 HTTP、不等任何请求。
$TimerBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    if (-not $Ctx.armed.Wait(180000)) {
        Add-WdLog $Ctx 'ERROR' 'TIMER 未等到 T0（armed 超时），不发出截止 Stop'
        return
    }
    $deadline = [double]$Ctx.d['deadlineMono']
    Add-WdLog $Ctx 'INFO' ("TIMER 已武装：deadline_mono_ms={0:N1}（= T0 + {1} ms）" -f $deadline, [int]$Ctx.deadlineOffsetMs)
    while (-not $Ctx.shutdown.Wait(0)) {
        $remain = $deadline - (Get-WdMono $Ctx)
        if ($remain -le 0) { break }
        if ($remain -le 25) {
            # 最后 25 ms 自旋：ManualResetEventSlim.Wait 受系统时钟粒度（约 15.6 ms）影响，
            # 自旋把"到达截止 → 入队 Stop"的偏差压到 1 ms 量级（§4.6.1 要求偏差 ≤1 s）。
            while ((Get-WdMono $Ctx) -lt $deadline) { [void][System.Threading.Thread]::Yield() }
            break
        }
        # 分片等待（≤200 ms）以便 shutdown 时能及时退出；并且**留出 20 ms 余量**，
        # 保证任何一次 Wait 的粒度抖动都不会跨过截止点（跨过就轮不到上面的自旋了）。
        $chunk = [math]::Min($remain - 20.0, 200.0)
        if ($chunk -lt 1) { $chunk = 1 }
        [void]$Ctx.wake.Wait([int][math]::Ceiling($chunk))
    }
    $fire = Get-WdMono $Ctx
    $Ctx.d['deadlineFireMono'] = $fire
    Add-WdLog $Ctx 'WARN' ("TIMER 到达截止：mono={0:N1} ms，deadline={1:N1} ms，偏差 {2:N1} ms → 入队 DEADLINE（不等任何 HTTP 完成）" -f $fire, $deadline, ($fire - $deadline))
    $Ctx.stopQueue.Enqueue('DEADLINE')
    [void]$Ctx.stopSignal.Release()
}

# ③ STOP：停止流。**发出** Stop 请求；Stop 自身可能挂 75–80 s，不阻塞观测与计时。
$StopBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    $uri = "$($Ctx.baseUrl)/api/v1/endpoints/$($Ctx.endpointId)/stop"
    $n = 0
    $lastReasonAt = @{}
    Add-WdLog $Ctx 'INFO' 'STOP 执行流就绪（等信号；不轮询端点状态、不参与计时）'
    while (-not $Ctx.shutdown.Wait(0)) {
        if (-not $Ctx.stopSignal.Wait(200)) { continue }
        # 把当前队列里的原因合并成**一次** Stop；DEADLINE 永不因去重被丢掉。
        $reasons = [System.Collections.Generic.List[string]]::new()
        $r = $null
        while ($Ctx.stopQueue.TryDequeue([ref]$r)) {
            $now = Get-WdMono $Ctx
            $prev = 0.0
            if ($lastReasonAt.ContainsKey($r)) { $prev = [double]$lastReasonAt[$r] }
            if ($r -ne 'DEADLINE' -and $prev -gt 0 -and ($now - $prev) -lt 2000) {
                Add-WdLog $Ctx 'INFO' "STOP 去重：原因 $r 在 2 s 内已触发过，忽略本次重复"
                $r = $null
                continue
            }
            $lastReasonAt[$r] = $now
            if (-not $reasons.Contains($r)) { $reasons.Add($r) }
            $r = $null
        }
        if ($reasons.Count -eq 0) { continue }
        if ($n -ge [int]$Ctx.maxStops) {
            Add-WdLog $Ctx 'WARN' "STOP 次数已达上限 $($Ctx.maxStops) 次，忽略原因：$($reasons -join ',')"
            continue
        }

        $n++
        $tDispatch = Get-Date
        $mono = Get-WdMono $Ctx
        $deadline = [double]$Ctx.d['deadlineMono']
        $deviation = $null
        if ($deadline -gt 0) { $deviation = [math]::Round($mono - $deadline, 1) }
        $Ctx.d['stopInFlight'] = $true
        $Ctx.d['stopCount'] = $n
        if ($n -eq 1) { $Ctx.d['stopFirstMono'] = $mono }
        $Ctx.d['stopLastMono'] = $mono

        $swReq = [System.Diagnostics.Stopwatch]::StartNew()
        $status = 0
        $err = $null
        $body = ''
        try {
            $resp = Invoke-WebRequest -Uri $uri -Method Post -TimeoutSec ([int]$Ctx.stopTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
            $status = [int]$resp.StatusCode
            $body = [string]$resp.Content
        }
        catch {
            $err = $_.Exception.Message
        }
        $elapsed = [math]::Round($swReq.Elapsed.TotalMilliseconds, 1)
        $Ctx.d['stopInFlight'] = $false

        $rec = [ordered]@{
            n                   = $n
            reasons             = @($reasons)
            t_dispatch_local    = $tDispatch.ToString('o')
            t_dispatch_unix_ns  = ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000)
            t_dispatch_mono_ms  = [math]::Round($mono, 1)
            t0_mono_ms          = [double]$Ctx.d['t0Mono']
            deadline_mono_ms    = $deadline
            deviation_ms        = $deviation
            http_status         = $status
            elapsed_ms          = $elapsed
            body                = $body
            err                 = $err
        }
        Write-WdJsonl $Ctx $Ctx.stopsPath $rec
        $Ctx.stopRecords.Enqueue($rec)
        Add-WdLog $Ctx 'WARN' ("STOP #{0} 发出：原因={1}；t_mono={2:N1} ms；deadline={3:N1} ms；偏差={4:N1} ms；http={5}；请求耗时={6} ms{7}" -f $n, ($reasons -join '+'), $mono, $deadline, $deviation, $status, $elapsed, $(if ($err) { "；错误=$err" } else { '' }))
        if ($reasons -contains 'RACE_START_AFTER_STOP') {
            Add-WdLog $Ctx 'WARN' 'RACE_START_AFTER_STOP：Stop 之后观测到 RUNNING 或 epoch 变化，已再次 Stop（§5.3）'
        }
    }
}

# ④ START：启动流。发 Start 一次、不重发；超时只记录，不影响计时。
$StartBody = {
    param($Ctx)
    $ErrorActionPreference = 'Stop'
    if (-not $Ctx.startGo.Wait(180000)) {
        Add-WdLog $Ctx 'ERROR' 'START 未收到放行信号，未发出 Start'
        return
    }
    $uri = "$($Ctx.baseUrl)/api/v1/endpoints/$($Ctx.endpointId)/start"
    $tDispatch = Get-Date
    $mono = Get-WdMono $Ctx
    $Ctx.d['startDispatchMono'] = $mono
    $Ctx.d['startDispatchLocal'] = $tDispatch.ToString('o')
    $swReq = [System.Diagnostics.Stopwatch]::StartNew()
    $status = 0
    $err = ''
    $body = ''
    $outcome = 'unknown'
    try {
        $resp = Invoke-WebRequest -Uri $uri -Method Post -TimeoutSec ([int]$Ctx.startTimeoutSec) -SkipHttpErrorCheck -ErrorAction Stop
        $status = [int]$resp.StatusCode
        $body = [string]$resp.Content
        $outcome = 'returned'
    }
    catch {
        $err = $_.Exception.Message
        if ($err -match '(?i)timeout|timed out|cancel|abort') { $outcome = 'timeout' } else { $outcome = 'error' }
    }
    $elapsed = [math]::Round($swReq.Elapsed.TotalMilliseconds, 1)
    $Ctx.d['startOutcome'] = $outcome
    $Ctx.d['startStatus'] = $status
    $Ctx.d['startErr'] = $err
    $Ctx.d['startElapsedMs'] = $elapsed
    $Ctx.d['startBody'] = $body
    Add-WdLog $Ctx 'WARN' ("START 结果={0}（http={1}，耗时={2} ms）{3}；截止计时与 Start 返回值无关（§5.1-3/4）" -f $outcome, $status, $elapsed, $(if ($err) { "；错误=$err" } else { '' }))
    if ($outcome -ne 'returned' -or $status -ne 200) {
        Add-WdLog $Ctx 'WARN' 'START 结果不确定（在途/失败）：按 §5.3 在途 Start 规则，不得据此判 PASS；截止 Stop 仍按 T0+30 s 发出'
    }
}
#endregion

#region ── 启动执行流（全部在 FIRED 之前就绪）────────────────────────────────────
function Start-WdFlow {
    param([string]$Name, [string]$Body, [hashtable]$Ctx)
    # 注意：param() 必须是脚本的第一条语句，所以共享函数放在 param 之后；
    # 脚本体本身（含自己的 param($Ctx)）作为内层 scriptblock 调用，$Ctx 由外层传入。
    $code = "param(`$Ctx)`n" + $script:CommonPrelude + "`n& {`n" + $Body + "`n} `$Ctx"
    $rs = [runspacefactory]::CreateRunspace()
    $rs.Name = "wd-$Name"
    $rs.Open()
    $ps = [powershell]::Create()
    $ps.Runspace = $rs
    [void]$ps.AddScript($code)
    [void]$ps.AddArgument($Ctx)
    $handle = $ps.BeginInvoke()
    return [pscustomobject]@{ Name = $Name; PS = $ps; RS = $rs; Handle = $handle }
}

$ctx.deadlineOffsetMs = $script:DEADLINE_OFFSET_MS
$flows = @()
$flows += Start-WdFlow -Name 'obs' -Body $ObserveBody.ToString() -Ctx $ctx
$flows += Start-WdFlow -Name 'timer' -Body $TimerBody.ToString() -Ctx $ctx
$flows += Start-WdFlow -Name 'stop' -Body $StopBody.ToString() -Ctx $ctx
$flows += Start-WdFlow -Name 'start' -Body $StartBody.ToString() -Ctx $ctx
Add-WdLog $ctx 'INFO' "四个独立执行流已启动：$((($flows | ForEach-Object { $_.Name }) -join ', '))"

# 观测预热：让 OBS 先跑起来，使 watchdog-states.jsonl 覆盖 T0-2 s（§5.1-5）。
# 注意：这只是"观测先启动"，T0 仍由随后的 FIRED 步骤生成，预热不参与任何计时。
Start-Sleep -Milliseconds 1500
#endregion

#region ── FIRED：由 watchdog 自己取 T0 并立即发出 Start（G6-FIRED / §5.1-2/3）────
# 顺序（同一段代码内完成，中间不插入任何等待）：
#   取单调时刻 → 发布 T0/截止（武装 TIMER）→ 写 t0.stamp → 放行 START 流。
$t0Mono = Get-WdMono $ctx
$t0Dto = [DateTimeOffset]::Now
$t0UnixNs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000
$deadlineMono = $t0Mono + $script:DEADLINE_OFFSET_MS
$deadlineDto = $t0Dto.AddMilliseconds($script:DEADLINE_OFFSET_MS)
$ctx.d['t0Mono'] = $t0Mono
$ctx.d['deadlineMono'] = $deadlineMono
$ctx.d['t0UnixNs'] = $t0UnixNs
$ctx.armed.Set()

$stamp = [ordered]@{
    t0_local            = $t0Dto.ToString('o')
    t0_unix_ns          = $t0UnixNs
    t0_mono_ms          = [math]::Round($t0Mono, 1)
    deadline_local      = $deadlineDto.ToString('o')
    deadline_unix_ns    = $t0UnixNs + ([int64]$script:DEADLINE_OFFSET_MS * 1000000)
    deadline_mono_ms    = [math]::Round($deadlineMono, 1)
    offset_sec          = $script:DEADLINE_OFFSET_MS / 1000
    clock               = 'System.Diagnostics.Stopwatch（单调）'
    generated_by        = "stop-watchdog.ps1 PID=$PID（FIRED 时刻自生成，不接受外部指定）"
    endpoint_id         = $EndpointId
    base_url            = $ctx.baseUrl
}
[System.IO.File]::WriteAllText($t0Path, ($stamp | ConvertTo-Json -Depth 4), [System.Text.UTF8Encoding]::new($false))
Add-WdLog $ctx 'WARN' ("FIRED：T0 = {0}（mono {1:N1} ms），截止 = T0+{2} ms = {3}（mono {4:N1} ms）" -f $t0Dto.ToString('o'), $t0Mono, $script:DEADLINE_OFFSET_MS, $deadlineDto.ToString('o'), $deadlineMono)

$ctx.startGo.Set()
$t0ToStart = (Get-WdMono $ctx) - $t0Mono
Add-WdLog $ctx 'INFO' ("已放行 START 流：距 T0 {0:N1} ms（该值即取 T0 到发出 Start 之间的实际间隔，供事后核对）" -f $t0ToStart)
Write-Host ("[FIRED] T0={0}  deadline={1}  (mono {2:N1} / {3:N1} ms)" -f $t0Dto.ToString('HH:mm:ss.fff'), $deadlineDto.ToString('HH:mm:ss.fff'), $t0Mono, $deadlineMono)
#endregion

#region ── 协调者主循环：读样本 → 判定触发 → 入队（自己从不发 HTTP）──────────────
$preStartDetail = ''
$preStartEpoch = ''
$sawPreStartSample = $false
$runningEpoch = $null
$reconStart = 0.0
$raceCount = 0
$lastRaceAt = 0.0
$lastValid = $null

function Send-WdStopRequest {
    param([string]$Reason)
    $ctx.stopQueue.Enqueue($Reason)
    [void]$ctx.stopSignal.Release()
}

$coordStart = Get-WdMono $ctx
$loopDone = $false
while (-not $loopDone) {
    $item = $null
    while ($ctx.history.TryDequeue([ref]$item)) {
        $s = $item
        $item = $null
        $mono = [double]$s.t_mono_ms
        $state = [string]$s.state
        $epoch = [string]$s.epoch
        $detail = [string]$s.detail
        $desired = $s.desired_running
        if ($state) { $lastValid = $s }

        $t0 = [double]$ctx.d['t0Mono']
        if ($t0 -le 0 -or $mono -lt $t0) {
            # Start 之前的样本：只用来记录"基线 detail/epoch"，不参与触发判定。
            if (-not $sawPreStartSample -and $state) {
                $sawPreStartSample = $true
                $preStartDetail = $detail
                $preStartEpoch = $epoch
                Add-WdLog $ctx 'INFO' "Start 前基线：state=$state desired=$desired epoch=$epoch detail=$(if ($detail) { $detail } else { '（空）' })"
            }
            continue
        }

        $stopFirst = [double]$ctx.d['stopFirstMono']

        # ── Stop 发出之后：竞态检测（§5.3）──────────────────────────────────────
        if ($stopFirst -gt 0 -and $mono -ge $stopFirst) {
            $postBase = [string]$ctx.d['postStopEpoch']
            if ($state -and $state -ne 'STOPPED' -and $postBase -eq '') { $ctx.d['postStopEpoch'] = $epoch }
            $isRace = $false
            if ($state -eq 'RUNNING') { $isRace = $true }
            elseif ($postBase -ne '' -and $epoch -and $epoch -ne $postBase) { $isRace = $true }
            if ($isRace -and $raceCount -lt $script:MAX_STOPS) {
                if (($mono - $lastRaceAt) -ge 2000 -or $lastRaceAt -le 0) {
                    $raceCount++
                    $lastRaceAt = $mono
                    $ctx.d['raceCount'] = $raceCount
                    Add-WdLog $ctx 'WARN' ("竞态：Stop 之后观测到 state={0} epoch={1}（相对 T0={2:N1} ms，desired={3}）→ 记录 RACE_START_AFTER_STOP 并再次 Stop（第 {4} 次）" -f $state, $epoch, ($mono - $t0), $desired, $raceCount)
                    Send-WdStopRequest -Reason 'RACE_START_AFTER_STOP'
                }
            }
            continue
        }

        # ── Stop 发出之前：S1/S2/S3/S6 触发判定（首次确认即停，无"连续 N 次"宽限）──

        if ($state -eq 'FAILED') {
            Add-WdLog $ctx 'ERROR' ("触发 S6：state=FAILED（相对 T0={0:N1} ms，detail={1}）→ 立即 Stop" -f ($mono - $t0), $detail)
            Send-WdStopRequest -Reason 'S6_FAILED'
            continue
        }

        # S1/S2：detail 里的通信失败。只认 Start 之后**新出现/变化**的 detail——
        # Start 之前端点可能残留上一轮的失败 detail，那不是"本轮首次确认的通信失败"。
        if ($detail -and $detail -ne $preStartDetail -and $detail -match $script:COMM_FAIL_PATTERN) {
            $clause = if ($detail -match '(?i)S7_JOB_') { 'S1' } else { 'S2' }
            Add-WdLog $ctx 'ERROR' ("触发 {0}：首次确认通信失败 detail={1}（相对 T0={2:N1} ms，state={3}）→ 立即 Stop（不设连续 N 次宽限）" -f $clause, $detail, ($mono - $t0), $state)
            Send-WdStopRequest -Reason "${clause}_COMM_FAIL"
            continue
        }

        # S3：除 Start 那一次外 epoch 再变；或 RECONNECTING ≥10 s。
        if ($state -eq 'RUNNING' -and $epoch) {
            if ($null -eq $runningEpoch) {
                $runningEpoch = $epoch
                Add-WdLog $ctx 'INFO' "首个 RUNNING 样本：epoch=$epoch（相对 T0=$([math]::Round($mono - $t0, 1)) ms）"
            }
            elseif ($epoch -ne $runningEpoch) {
                Add-WdLog $ctx 'ERROR' ("触发 S3：RUNNING 期间 epoch 变化（{0} -> {1}，相对 T0={2:N1} ms）→ 立即 Stop" -f $runningEpoch, $epoch, ($mono - $t0))
                Send-WdStopRequest -Reason 'S3_EPOCH_CHANGE'
                continue
            }
        }
        if ($state -eq 'RECONNECTING') {
            if ($reconStart -le 0) { $reconStart = $mono }
            elseif (($mono - $reconStart) -ge 10000) {
                Add-WdLog $ctx 'ERROR' ("触发 S3：RECONNECTING 已持续 {0:N1} ms（相对 T0={1:N1} ms）→ 立即 Stop" -f ($mono - $reconStart), ($mono - $t0))
                Send-WdStopRequest -Reason 'S3_RECONNECTING_10S'
                $reconStart = 0.0
                continue
            }
        }
        else {
            $reconStart = 0.0
        }
    }

    # 把新产生的 Stop 记录打到控制台（主流程只读记录，不参与发请求）。
    $sr = $null
    while ($ctx.stopRecords.TryDequeue([ref]$sr)) {
        $dev = $sr.deviation_ms
        $devText = if ($null -ne $dev) { ('{0:+0.0;-0.0;0.0} ms' -f [double]$dev) } else { 'N/A' }
        Write-Host ("[STOP #{0}] {1}  reasons={2}  deviation={3}  http={4}  elapsed={5} ms" -f $sr.n, $sr.t_dispatch_local, (($sr.reasons) -join '+'), $devText, $sr.http_status, $sr.elapsed_ms)
    }

    if ($ctx.obsDone.Wait(0)) { $loopDone = $true; break }
    # 观测流意外退出（例如 runspace 崩了）也要收敛，不能无限等。
    if (((Get-WdMono $ctx) - $coordStart) -gt (($ObserveMaxSec + 60) * 1000)) {
        Add-WdLog $ctx 'ERROR' '协调者等待观测超时（观测流可能已异常退出），强制收敛'
        $loopDone = $true
        break
    }
    [void]$ctx.obsDone.Wait(20)
}
# 收尾：把剩余样本判完（观测流退出后队列里可能还有几条）。
$item = $null
while ($ctx.history.TryDequeue([ref]$item)) {
    if ([string]$item.state) { $lastValid = $item }
    $item = $null
}
$sr = $null
while ($ctx.stopRecords.TryDequeue([ref]$sr)) {
    Write-Host ("[STOP #{0}] {1}  reasons={2}  deviation={3} ms  http={4}  elapsed={5} ms" -f $sr.n, $sr.t_dispatch_local, (($sr.reasons) -join '+'), $sr.deviation_ms, $sr.http_status, $sr.elapsed_ms)
}
#endregion

#region ── 等待在途 Stop 落盘（Stop 可能挂 75–80 s，不能把"已发出"当"已完成"）────
$waitStart = Get-WdMono $ctx
while ([bool]$ctx.d['stopInFlight'] -and ((Get-WdMono $ctx) - $waitStart) -lt (($StopTimeoutSec + 5) * 1000)) {
    Start-Sleep -Milliseconds 50
}
$allStops = @()
$sr = $null
# stopRecords 已被消费过，这里直接读 jsonl 兜底（保证 summary 里有完整记录）。
if (Test-Path $stopsPath) {
    foreach ($line in [System.IO.File]::ReadAllLines($stopsPath)) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        try { $allStops += ($line | ConvertFrom-Json) } catch { }
    }
}
$stopCount = $allStops.Count
#endregion

#region ── "停干净"六项复核（§5.3；缺一不算）────────────────────────────────────
$items = [ordered]@{}
$stopHttpOk = $false
$stopHttpDetail = '未发出任何 Stop'
foreach ($s in $allStops) {
    if ([int]$s.http_status -eq 200 -or [int]$s.http_status -eq 500) { $stopHttpOk = $true }
    $stopHttpDetail = "最后一次 Stop：http=$($s.http_status)（200/500 均算停住：500 是 drain 失败但 desired_running 仍落 false，core-api:2181-2190）"
}
$items['1_stop_http'] = [ordered]@{ required = $true; ok = $stopHttpOk; detail = $stopHttpDetail }

$finalState = if ($lastValid) { [string]$lastValid.state } else { $null }
$finalDesired = if ($lastValid) { $lastValid.desired_running } else { $null }
$finalEpoch = if ($lastValid) { [string]$lastValid.epoch } else { $null }
$items['2_state_stopped'] = [ordered]@{ required = $true; ok = ($finalState -eq 'STOPPED'); detail = "最终观测 state=$finalState" }
$items['3_desired_false'] = [ordered]@{ required = $true; ok = ($finalDesired -eq $false); detail = "最终观测 desired_running=$finalDesired" }

# ④⑤ 是本机环境核对，离线假端点场景无意义（-HostChecks 关闭时记 N/A，原因写明）。
if ($HostChecks) {
    $procHit = @()
    try {
        # 有 driver_id 时按它精确匹配（如 sinumerik-nck），避免误伤同机其它含 driver 字样的进程。
        $driverId = [string]$ctx.d['driverId']
        $driverKey = if ($driverId) { '(?i)' + [regex]::Escape($driverId) + '|mesa-driver' } else { '(?i)driver|mesa-driver' }
        $procHit = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -match $driverKey } | Select-Object -ExpandProperty ProcessName -Unique)
    }
    catch { $procHit = @("查询失败：$($_.Exception.Message)") }
    $items['4_no_driver_process'] = [ordered]@{ required = $true; ok = ($procHit.Count -eq 0); detail = if ($procHit.Count -eq 0) { '未发现驱动进程' } else { "仍存在：$($procHit -join ',')" } }

    $devHost = [string]$ctx.d['deviceHost']
    $devPort = [int]$ctx.d['devicePort']
    if ($devHost -and $devPort -gt 0) {
        $conns = @()
        try {
            $conns = @(Get-NetTCPConnection -RemoteAddress $devHost -RemotePort $devPort -State Established -ErrorAction Stop)
        }
        catch { $conns = @() }
        $items['5_no_established_device'] = [ordered]@{ required = $true; ok = ($conns.Count -eq 0); detail = if ($conns.Count -eq 0) { "到 $devHost`:$devPort 无 Established" } else { "仍有 $($conns.Count) 条 Established 到 $devHost`:$devPort" } }
    }
    else {
        $items['5_no_established_device'] = [ordered]@{ required = $true; ok = $false; detail = '未能从端点响应取到 connection.host/port，无法核对' }
    }
}
else {
    $items['4_no_driver_process'] = [ordered]@{ required = $false; ok = $null; detail = 'N/A：未启用 -HostChecks（离线假端点场景无驱动进程可查）' }
    $items['5_no_established_device'] = [ordered]@{ required = $false; ok = $null; detail = 'N/A：未启用 -HostChecks（离线假端点场景无机床连接可查）' }
}

$settleOk = [bool]$ctx.d['settleOk']
$items['6_settle_10s'] = [ordered]@{
    required = $true
    ok       = $settleOk
    detail   = if ($settleOk) {
        "首次 STOPPED 后连续 $($script:SETTLE_MS / 1000) s 无新 epoch、无 RECONNECTING（窗口内样本 $($ctx.d['settleSamples']) 条，重置 $($ctx.d['settleResets']) 次）"
    }
    else {
        "稳定窗口未完成（首次 STOPPED mono=$($ctx.d['obsFirstStoppedMono'])，窗口重置 $($ctx.d['settleResets']) 次）"
    }
}

$mandatoryFail = @()
foreach ($k in $items.Keys) {
    if ($items[$k].required -and -not $items[$k].ok) { $mandatoryFail += $k }
}
$raceRecorded = ($raceCount -gt 0)
$startOutcome = [string]$ctx.d['startOutcome']
$startUncertain = ($startOutcome -ne 'returned' -or [int]$ctx.d['startStatus'] -ne 200)
$verdict = if ($mandatoryFail.Count -eq 0) { 'STOPPED_CLEAN' } else { 'NOT_STOPPED' }
if ($verdict -eq 'STOPPED_CLEAN' -and $raceRecorded) { $verdict = 'STOPPED_CLEAN_WITH_RACE' }
$exitCode = if ($mandatoryFail.Count -eq 0) { 0 } else { 1 }
#endregion

#region ── 汇总输出与落盘 ─────────────────────────────────────────────────────────
# 偏差判据（§4.6.1）只针对**截止 Stop**：它衡量"调度是否被阻塞"。
# 竞态补发的 Stop 天然晚于截止（它是被观测结果触发的），其 deviation 只是相对截止的时标，
# **不得**混进 ≤1 s 判据，否则会把"竞态处置正确"误判成"调度被阻塞"。
$deadlineDevList = @()
$raceDevList = @()
foreach ($s in $allStops) {
    if ($null -eq $s.deviation_ms) { continue }
    if ((@($s.reasons)) -contains 'DEADLINE') { $deadlineDevList += [double]$s.deviation_ms }
    else { $raceDevList += [double]$s.deviation_ms }
}
$maxDev = $null
$maxAbsDev = $null
if ($deadlineDevList.Count -gt 0) {
    $maxDev = ($deadlineDevList | Measure-Object -Maximum).Maximum
    $maxAbsDev = ($deadlineDevList | ForEach-Object { [math]::Abs($_) } | Measure-Object -Maximum).Maximum
}

$summary = [ordered]@{
    tag                  = $Tag
    pid                  = $PID
    ps_version           = [string]$PSVersionTable.PSVersion
    host                 = $hostName
    base_url             = $ctx.baseUrl
    endpoint_id          = $EndpointId
    script_started_local = $procStart.ToString('o')
    observe_timeout_sec  = $ObserveTimeoutSec
    observe_max_sec      = $ObserveMaxSec
    deadline_offset_sec  = $script:DEADLINE_OFFSET_MS / 1000
    t0                   = [ordered]@{
        local            = $t0Dto.ToString('o')
        unix_ns          = $t0UnixNs
        mono_ms          = [math]::Round($t0Mono, 1)
        deadline_local   = $deadlineDto.ToString('o')
        deadline_mono_ms = [math]::Round($deadlineMono, 1)
        t0_to_start_ms   = [math]::Round($t0ToStart, 1)
    }
    start                = [ordered]@{
        outcome    = $startOutcome
        http       = [int]$ctx.d['startStatus']
        elapsed_ms = [double]$ctx.d['startElapsedMs']
        dispatch   = [string]$ctx.d['startDispatchLocal']
        err        = [string]$ctx.d['startErr']
        uncertain  = $startUncertain
    }
    stops                     = @($allStops)
    stop_count                = $stopCount
    deadline_stop_deviation_ms = $maxDev
    max_abs_deviation_ms      = $maxAbsDev
    non_deadline_stop_deviations_ms = @($raceDevList)
    final                = [ordered]@{
        state           = $finalState
        desired_running = $finalDesired
        epoch           = $finalEpoch
        t_local         = if ($lastValid) { [string]$lastValid.t_local } else { $null }
        t_rel_ms        = if ($lastValid) { $lastValid.t_rel_ms } else { $null }
    }
    six_items            = $items
    mandatory_failures   = @($mandatoryFail)
    race                 = [ordered]@{ count = $raceCount; recorded = $raceRecorded }
    observation          = [ordered]@{
        samples        = [int]$ctx.d['obsSamples']
        failures       = [int]$ctx.d['obsFailures']
        max_req_ms     = [double]$ctx.d['obsMaxReqMs']
        settle_ok      = $settleOk
        settle_samples = [int]$ctx.d['settleSamples']
        settle_resets  = [int]$ctx.d['settleResets']
        first_stopped  = [double]$ctx.d['obsFirstStoppedMono']
    }
    verdict              = $verdict
    exit_code            = $exitCode
    out_dir              = $OutDir
}
[System.IO.File]::WriteAllText($summaryPath, ($summary | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))

Add-WdLog $ctx 'INFO' "判定：$verdict；六项=$((($items.Keys | ForEach-Object { "$_=" + $(if ($null -eq $items[$_].ok) { 'N/A' } elseif ($items[$_].ok) { 'OK' } else { 'FAIL' }) }) -join ' '))；退出码=$exitCode"

Write-Host ''
Write-Host '================ stop-watchdog 结果 ================'
Write-Host ("PID            : {0}（独立进程）" -f $PID)
Write-Host ("BaseUrl        : {0}" -f $ctx.baseUrl)
Write-Host ("Endpoint       : {0}" -f $EndpointId)
Write-Host ("T0             : {0}  (mono {1:N1} ms)" -f $t0Dto.ToString('o'), $t0Mono)
Write-Host ("Deadline       : {0}  (mono {1:N1} ms = T0+{2} s)" -f $deadlineDto.ToString('o'), $deadlineMono, ($script:DEADLINE_OFFSET_MS / 1000))
Write-Host ("T0→Start 发出  : {0:N1} ms" -f $t0ToStart)
Write-Host ("START          : outcome={0} http={1} elapsed={2} ms{3}" -f $startOutcome, $ctx.d['startStatus'], $ctx.d['startElapsedMs'], $(if ($startUncertain) { '  ** 结果不确定（在途/超时）→ 按 §5.3 不得判 PASS **' } else { '' }))
if ($allStops.Count -eq 0) {
    Write-Host 'STOP           : ** 未发出任何 Stop **'
}
foreach ($s in $allStops) {
    $d = $s.deviation_ms
    $dText = if ($null -ne $d) { ('{0:+0.0;-0.0;0.0} ms' -f [double]$d) } else { 'N/A' }
    Write-Host ("STOP #{0,-2}       : {1}  reasons={2}  deviation={3}  http={4}  elapsed={5} ms" -f $s.n, $s.t_dispatch_local, (($s.reasons) -join '+'), $dText, $s.http_status, $s.elapsed_ms)
}
if ($null -ne $maxAbsDev) {
    $devVerdict = if ($maxAbsDev -le 1000) { 'PASS（≤1000 ms，调度未被阻塞）' } else { 'FAIL（>1000 ms，G6-ARMED 不通过）' }
    Write-Host ("偏差判定        : 截止 Stop 的 |deviation| = {0:N1} ms  ⇒ {1}" -f $maxAbsDev, $devVerdict)
}
else {
    Write-Host '偏差判定        : ** 未发出截止 Stop，无法判定 **'
}
if ($raceDevList.Count -gt 0) {
    Write-Host ("非截止 Stop    : 相对截止的时标 {0}（提前触发/竞态补发，**不**作为调度判据）" -f (($raceDevList | ForEach-Object { '{0:+0.0;-0.0;0.0}' -f $_ }) -join ' / '))
}
Write-Host ("最终观测        : state={0} desired_running={1} epoch={2}（t_rel={3} ms）" -f $finalState, $finalDesired, $finalEpoch, $(if ($lastValid) { $lastValid.t_rel_ms } else { 'N/A' }))
Write-Host ("观测            : 样本 {0} 条，失败 {1} 次，单次最长 {2} ms" -f $ctx.d['obsSamples'], $ctx.d['obsFailures'], $ctx.d['obsMaxReqMs'])
foreach ($k in $items.Keys) {
    $v = $items[$k]
    $flag = if ($null -eq $v.ok) { 'N/A ' } elseif ($v.ok) { 'OK  ' } else { 'FAIL' }
    Write-Host ("  {0,-26} [{1}] {2}" -f $k, $flag, $v.detail)
}
Write-Host ("竞态            : {0}" -f $(if ($raceRecorded) { "记录 RACE_START_AFTER_STOP ×$raceCount（已再次 Stop）" } else { '未观测到' }))
Write-Host ("判定            : {0}" -f $verdict)
Write-Host ("退出码          : {0}" -f $exitCode)
Write-Host ("证据目录        : {0}" -f $OutDir)

if ($exitCode -ne 0) {
    Write-Host ''
    Write-Host '=========== 未达成"停干净"：按 §5.3 隔离处置（由监护人执行）==========='
    Write-Host '① 阻断出站：防火墙 Block <机床 IP>:102（对机床零风险，诊断方案 :405）'
    Write-Host '② 杀驱动进程（会触发重连，必须先阻断；:404）'
    Write-Host '③ 杀 mesad（须先确认 desired_running=false；:406）'
    Write-Host '④ 物理隔离只拔**测试主机**网线，不得擅自断产线交换机口（:407 的 E4 影响整线）'
    Write-Host '每一步后复核 state / desired_running / 驱动进程 / 到 :102 的连接，全过程记入 §10.3。'
    Write-Host '不得自动重试、不得自动进入下一轮（§7 统一流程）。'
}
Write-Host '==================================================='

# 收尾：通知所有执行流退出，并释放 runspace。
$ctx.shutdown.Set()
[void]$ctx.wake.Set()
try { [void]$ctx.stopSignal.Release() } catch { }
foreach ($f in $flows) {
    try { $f.PS.Stop() } catch { }
    try { $f.PS.Dispose() } catch { }
    try { $f.RS.Dispose() } catch { }
}
#endregion

exit $exitCode






