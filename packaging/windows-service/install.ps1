# Mesa Windows Service 安装（§25 KILL_ON_JOB_CLOSE）
# 管理员 PowerShell 运行
$ErrorActionPreference = "Stop"
$svcName = "Mesa"
$bin = Join-Path $PSScriptRoot "..\..\target\release\mesad.exe"
if (-not (Test-Path $bin)) { throw "未找到 $bin，请先 cargo build --release" }
# 使用 nssm 或 sc create（示例 sc）
$args = "--db `"$env:ProgramData\mesa\mesa.db`" --http-port 8132 --drivers-dir `"$PSScriptRoot\..\..\drivers`""
if (Get-Service $svcName -ErrorAction SilentlyContinue) { sc.exe delete $svcName | Out-Null; Start-Sleep 2 }
sc.exe create $svcName binPath= "`"$bin`" $args" start= auto DisplayName= "Mesa Driver MVP" | Out-Null
# Job Object KILL_ON_JOB_CLOSE 由 mesad 进程内创建（crates/driver-manager/src/process.rs:job）无需额外配置
Write-Host "Service $svcName installed. Start with: sc start $svcName  or  Start-Service $svcName"
