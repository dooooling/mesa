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
| opmsg/value | yes (`0x34` type=4) | pass | ready |
| alarm/value | yes (`0x23`) | pass (empty=[]) | ready |
| servo/load | identity only (`0xA4/0x89/0x56/0xA4` + name) | wait (need >0%) | hold |
| spindle/load | identity only (`0xA4/0x8A/0x40[4,-1]+0xA4`) | wait (need scale) | hold |
| tool/offset | deferred (`cnc_rdtofs` family) | hold (need type map) | hold |
| tool/length | deferred (type=3 single point) | hold | hold |
| tool/zofs | deferred (work zero, semantic corrected) | hold | hold |

`12/17 ready`，`2/17 identity-only hold`，`3/17 deferred hold`。

下一步（按顺序）：自然窗口补 `servo/spindle load > 0`（→14/17）；
再开 `driver/focas-wire-shadow`（Native primary + Wire compare log，不改生产值）。
