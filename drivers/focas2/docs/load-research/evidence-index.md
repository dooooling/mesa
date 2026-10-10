# FOCAS Load DLL 证据索引（PR-A）

基线：`main@248773e`（PR #91 已合并；15/17 READY，Load HOLD）。

## 状态词典

| 标记 | 含义 |
| --- | --- |
| `observed` | 真机/DLL 直接观测的原始字节 |
| `inferred` | 由观测推导、待复核的解释 |
| `synthetic-derived` | 离线合成、明确非真机的派生样本 |
| `unknown` | 尚未取得证据 |

## 证据清单

| # | 证据 | 状态 | 位置 |
| --- | --- | --- | --- |
| E1 | 165 spindle type=0/1/-1 请求/响应原始帧 | `observed` | `drivers/focas2/tests/fixtures/wire/spindle_load_165/` |
| E2 | Native `num_out=1` + 24B 单 record（165 零负载） | `observed` | `spindle_load_zero_live` 输出（待归档 JSONL） |
| E3 | DLL 完整文件身份（SHA-256 + 版本） | 待采集 | `target/focas-load-research/dll-manifest.csv` |
| E4 | 运行时实际加载模块（ProcMon Load Image） | `unknown` | `target/focas-load-research/loaded-modules.csv` |
| E5 | Native type=0 完整网络会话（OPEN/GENERIC/CLOSE） | `unknown` | `target/focas-load-research/native-spindle-type0.pcapng`（本地暂存，不进仓） |
| E6 | Native↔Wire type=0 请求差分 | `unknown` | 待 E5 后用 `tools/ci/focas_diff.py` 输出 |

## 抓包操作流程（Windows 管理员）

```powershell
pktmon status
pktmon filter list
# 确认可清理后：
pktmon filter remove
pktmon filter add FOCAS165 -i 192.168.15.165 -p 8193 -t tcp

$outDir = "D:\Mine\Project\mesa\target\focas-load-research"
pktmon start --capture --pkt-size 0 --comp nics `
    --file-name "$outDir\native-spindle-type0.etl"
try {
    $env:MESA_FOCAS_GATE0_HOST = "192.168.15.165"
    cargo test --locked -p mesa-driver-focas2 --lib `
        spindle_load_zero_live -- `
        --ignored --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "probe failed: $LASTEXITCODE" }
} finally {
    pktmon stop
}
pktmon etl2pcap "$outDir\native-spindle-type0.etl" `
    --out "$outDir\native-spindle-type0.pcapng"
```

要求：`1 test` 实际执行；`rc=0`；TCP 流重组后按 10B 帧头拆帧；
OPEN/GENERIC/CLOSE 逐帧归档 SHA；原始 ETL/pcapng 本地暂存，不进仓。

## DLL 身份冻结

```powershell
pwsh tools/ci/dll-manifest.ps1
```

静态清单 + ProcMon 实际加载模块双轨，SHA-256 对照，不标未观察模块为已加载。
