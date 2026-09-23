# Wire Cutover Matrix（收尾 tracking v1）

基线：`main@6253df9` + parity baseline v1（`wire-parity-sweep.md`）。
日期：2026-09-23
规则：`ready` 才能进 shadow 全等比较；`hold` 只记 documented，不计 mismatch。

| point | Wire | Native parity | status |
|-------|------|---------------|--------|
| machine/status | yes (`0x19`) | pass | ready |
| machine/feed | yes (`0x24`) | pass | ready |
| axis/absolute `1..3` | yes (`0x18+0x26`) | pass (drift=B expected) | ready |
| machine/spindle_speed | yes (`0x25`) | pass | ready |
| spindle/gear | yes (`0xA4+0x40[2]+0xA4`) | pass (672) | ready |
| spindle/maxrpm | yes (`0xA4+0x40[1]+0xA4`) | pass (874) | ready |
| macro/value | yes (`0x15`) | pass (engineering F64) | ready |
| pmc/value | yes (`0x8001`) | pass | ready |
| param/value | yes (`0x8D`) | pass (6711=10030) | ready |
| diagnosis/value REAL | yes (`0x93` type=5) | pass (-0.010, drift=B expected) | ready |
| opmsg/value | yes (`0x34` type=4) | ⚠️ Native debt OPEN（见下） | ready (wire) |
| alarm/value | yes (`0x23`) | pass (empty=[]) | ready |
| servo/load | identity only (`0xA4/0x89/0x56/0xA4` + name) | wait (need >0%) | hold |
| spindle/load | identity only (`0xA4/0x8A/0x40[4,-1]+0xA4`) | wait (need scale) | hold |
| tool/offset | deferred (`cnc_rdtofs` family) | hold (need type map) | hold |
| tool/length | deferred (type=3 single point) | hold | hold |
| tool/zofs | deferred (work zero, semantic corrected) | hold | hold |

`12/17 ready`，`2/17 identity-only hold`，`3/17 deferred hold`。

## Native OPMSG ABI debt（OPEN，Shadow Phase 1 round #3 发现）

- Symptom：Native `cnc_rdopmsg` 返回 `EW_Length(2)`（两轮一致），
  Wire 同窗返回 `String("OP:empty")`（有效结果）。
- Observed cause boundary：当前 Native buffer/wrapper 疑用不足的历史占位，
  而 Wire evidence 显示更大的真实响应布局（`data_len=268` 全量）。
  不断言“64B 一定是根因”（Native wrapper 实际 ABI/长度参数未独立证明）。
- Impact：Native 不能作为 opmsg 的 parity oracle。
- Cutover implication：opmsg parity 必须依赖独立 evidence/fixtures，
  不依赖 `Native == Wire` 全等。
- Shadow 分类：`KNOWN_NATIVE_DEBT`（与 `KNOWN_NATIVE_UNSUPPORTED`
  gear/maxrpm/diagnosis/alarm 分开；后者为 PR52 fail-closed，前者为实现 debt）。
- Status：OPEN（不阻塞 Shadow Phase 1；`mismatch=0` 保持）。

下一步（按顺序）：自然窗口补 `servo/spindle load > 0`（→14/17）；
`driver/focas-wire-shadow` Phase 1 基线 commit（Native primary + compare log，
不改生产值；本文件 debt 项同步进分支）。
