#!/usr/bin/env pwsh
# PR-A DLL 身份冻结：仓库全部 Windows DLL 清单（SHA-256 + 版本 + 架构）。
# 用法：pwsh tools/ci/dll-manifest.ps1
# 产物：target/focas-load-research/{dll-manifest.csv,mesa-commit.txt}
# 注意：静态清单 ≠ 运行时加载；实际加载模块另见 loaded-modules 记录流程
#（docs/load-research/dll-inventory.md）。
$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$dllDir = Join-Path $root 'drivers\focas2\libs\win'
$outDir = Join-Path $root 'target\focas-load-research'
New-Item -ItemType Directory -Force $outDir | Out-Null
Get-ChildItem $dllDir -Filter *.dll |
    Sort-Object Name |
    ForEach-Object {
        [pscustomobject]@{
            Name           = $_.Name
            Path           = $_.FullName
            SizeBytes      = $_.Length
            SHA256         = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
            FileVersion    = $_.VersionInfo.FileVersion
            ProductVersion = $_.VersionInfo.ProductVersion
        }
    } |
    Export-Csv "$outDir\dll-manifest.csv" -NoTypeInformation -Encoding utf8
git rev-parse HEAD | Set-Content "$outDir\mesa-commit.txt"
Write-Output "wrote $outDir\dll-manifest.csv"
