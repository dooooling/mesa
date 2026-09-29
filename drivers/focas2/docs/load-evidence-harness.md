# Load Evidence Harness — Evidence Day 操作纪律（PR C）

冻结范围：test-only harness；不碰 production/pre_ffi/Wire gate/canary；
coverage 保持 15/17；Gate 3-A/3-B 仍 ⏳ WAITING REAL FANUC。

## 1. 环境变量

```powershell
$env:MESA_FOCAS_GATE0_HOST="192.168.15.165"
$env:MESA_FOCAS_GATE0_PORT="8193"
$env:MESA_FOCAS_GATE0_TIMEOUT_MS="5000"
$env:MESA_FOCAS_LOAD_FAMILY="BOTH"   # SPINDLE / SERVO / BOTH
$env:MESA_FOCAS_LOAD_RUN="001"       # 三位数字；失败即作废递增，禁复用
```

## 2. 执行命令（单线程串行；同一 handle 整场）

```powershell
# BOTH（推荐；同一负载状态下 spindle + servo 皆有效时）
cargo test -p mesa-driver-focas2 load_evidence_harness -- --ignored --nocapture --test-threads=1

# SPINDLE-only
$env:MESA_FOCAS_LOAD_FAMILY="SPINDLE"
cargo test -p mesa-driver-focas2 load_evidence_harness -- --ignored --nocapture --test-threads=1

# SERVO-only（同理把 FAMILY 换成 SERVO）
```

## 3. 同场顺序（BOTH；stream order = RUN order）

```text
SPINDLE-L0-001 → SERVO-L0-001
→ SPINDLE-L1-001 → SERVO-L1-001
→ SPINDLE-L2-001 → SERVO-L2-001
→ SPINDLE-L0R-001 → SERVO-L0R-001
```

OPEN handle once（场首）→ CLOSE once（场尾）；窗间无 reconnect。

## 4. 每窗交互（操作员输入面板当前实际值 + 可选引用）

```text
[WAIT] RUN=SPINDLE-L1-001
请输入面板 SPINDLE LOAD S1 当前值：
> 27
[WAIT] RUN=SERVO-L1-001
请输入面板 SERVO LOAD 当前值（servo 附轴标识如 `27 X`）：
> 27 X
```

- 输入形如 `"<value> [ref]"`（servo 记录轴标识原文，不解释语义；
  同 family 四窗必须同一 ref，漂移即整场作废）。
- SERVO ref 不能为空（空 ref 即使四窗一致亦无轴 provenance，FFI 前 fail-closed）；
  spindle 空输入自动记 `"S1"`（prompt 已固定 S1）。
- 值须 `is_finite`（NaN/inf 即作废，不落非法 JSONL）。
- L1/L2 必须非零且互异（FFI 前预检；无效档位不发证据窗）。
- L0/L0R 不设 tolerance（baseline/recovery 由 Panel+Native+Wire 三方 review 判定）。
- JSONL `create_new` 在 connect/FFI 前先占（误复用 seq 不得多一次 OPEN/CLOSE 污染抓包）；
  文件已存在即 fail-closed，禁复用 append。
- rc 非零即整场 INVALID（本窗落盘后立即终止，不进下一窗）。
- panic（含 panel/guard/rc 路径）经 RAII handle guard free exactly once。
- 任何失败/误操作/负载未落定：本 session 作废，下一次必须新 seq，
  **禁止复用序号补跑某窗**。

## 5. 输出

- stdout：`>>> BEGIN RUN=… HANDLE=…` / `<<< END RUN=… RC=…`（切 ETL 对账用）。
- JSONL：`target/focas-load-evidence/load-<seq>.jsonl`，每 RUN 一行
  （`run_id/family/phase/handle/panel_value/panel_ref/num_in/num_out/rc/
  raw_0_512_hex/pre_guard_ok/post_guard_ok/tail_after_512_clean/unix_ms`）。
- raw 只保留 `payload[0..512]` hex；不转 `i16/LOADELM/%`（PR52 教训）。

## 6. guard 纪律（任一违反即整场 HOLD，不推 layout）

```text
pre_guard unchanged ✅ / post_guard unchanged ✅
payload[512..4096] clean ✅ / num_out ∈ 0..=4 ✅
```

## 7. 抓包（harness 不管理 pktmon；现场外部执行）

```powershell
pktmon start --capture --pkt-size 0
# …一次跑完整个 harness…
pktmon stop
```

建议文件：`load-both-001.etl`（或分开 `load-spindle-001.etl` /
`load-servo-001.etl`）。

## 8. Evidence Day 通过门（review 时判定，不由 harness 判定）

```text
panel L1/L2 非零且互异 / L0/L0R baseline 回落可辨
Native raw 槽位随 panel 单调变化（guard 全程 clean）
Wire slot（spindle 0x40[4,-1] / servo 0x56 系）随 panel 单调变化
Native / Panel / Wire 三方可对齐
```

## 9. 本 PR 明确不做

- 不恢复 production `cnc_rdspmeter/rdsvmeter`；不改 `SpLoad` layout；
  不写 spindle/servo codec；不改 Wire gate/canary；不碰 `pre_ffi_gate`；
  不提交 ETL/pcapng；coverage 保持 15/17。

## 10. CI fast-path probe（本节仅验证 `heavy=false` 路径；无语义变更）

本节存在即证明 docs-only 改动触发 fast CI（changes → heavy=false →
heavy jobs skipped → ci-gate SUCCESS）。验证后 CI-1 正式 CLOSED。
