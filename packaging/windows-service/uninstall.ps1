param([string]$svcName="ForgeLink")
if (Get-Service $svcName -ErrorAction SilentlyContinue) {
  Stop-Service $svcName -Force -ErrorAction SilentlyContinue
  sc.exe delete $svcName | Out-Null
  Write-Host "Service $svcName removed"
} else { Write-Host "Service $svcName not found" }
