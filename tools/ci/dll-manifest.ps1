#!/usr/bin/env pwsh
# PR-A DLL 身份冻结：仓库全部 Windows DLL 清单（SHA-256 + 版本 + 架构）。
# 用法：pwsh tools/ci/dll-manifest.ps1
# 产物：target/focas-load-research/{dll-manifest.csv,mesa-commit.txt}
# 注意：静态清单 ≠ 运行时加载；实际加载模块另见 loaded-modules 记录流程
#（drivers/focas2/docs/load-research/evidence-index.md）。
$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$dllDir = Join-Path $root 'drivers\focas2\libs\win'
$outDir = Join-Path $root 'target\focas-load-research'
New-Item -ItemType Directory -Force $outDir | Out-Null
$rows = Get-ChildItem $dllDir -Filter *.dll | Sort-Object Name
if (-not $rows -or $rows.Count -eq 0) {
    throw "no DLLs found in $dllDir（路径错误或仓库未检出？）"
}
$rows | ForEach-Object {
    # Machine 架构：PE 头 Machine 字段（0x8664=x64 / 0x14c=x86），只读诊断。
    $machine = ''
    try {
        $fs = [System.IO.File]::OpenRead($_.FullName)
        try {
            $br = New-Object System.IO.BinaryReader($fs)
            $fs.Seek(0x3C, [System.IO.SeekOrigin]::Begin) | Out-Null
            $peOff = $br.ReadInt32()
            $fs.Seek($peOff + 4, [System.IO.SeekOrigin]::Begin) | Out-Null
            $machine = ('0x{0:x}' -f $br.ReadUInt16())
        } finally { $br.Close() }
    } catch { $machine = 'unknown' }
    [pscustomobject]@{
        Name           = $_.Name
        Path           = $_.FullName
        SizeBytes      = $_.Length
        SHA256         = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
        FileVersion    = $_.VersionInfo.FileVersion
        ProductVersion = $_.VersionInfo.ProductVersion
        Machine        = $machine
    }
} | Export-Csv "$outDir\dll-manifest.csv" -NoTypeInformation -Encoding utf8
git -C $root rev-parse HEAD | Set-Content "$outDir\mesa-commit.txt"
Write-Output "wrote $outDir\dll-manifest.csv"
