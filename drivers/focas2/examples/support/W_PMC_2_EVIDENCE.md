# W-PMC-2 Evidence — controlled ladder signal `R100.0 ──( R101.0 )`

冻结日期：2026-09-27（UTC+8 现场窗口）
状态：`W-PMC-2 controlled ladder signal ✅ CLOSED`（本文件为 evidence closure，
只证 controlled-signal semantics，不碰 W-PMC-6 production control）。

## 1. 现场条件

- CNC：NCGuide `192.168.15.165:8193`，GLOBAL = 186 NET，PMC RUN。
- Ladder 已 UPDATE：`R100.0 ─────────────( R101.0 )`（最小链）。
- Address Map：R100 bit0~7 / R101 bit0~7 均为空闲（直接引用搜索无命中；
  R100 绝对未被系统使用 ❌ NOT-PROVEN，不声明）。
- harness：`drivers/focas2/examples/support/ncguide_write.rs`
  （`--test-r100-bit0`；R100 BYTE RMW `original | 0x01`；R101 全程只读；
  100ms ×2 scan wait；finally restore 完整 original BYTE）。

## 2. 执行路径（Native `pmc_wrpmcrng` controlled harness）

> 本轮实际执行写路径是 Native `pmc_wrpmcrng` controlled harness，
> 不是 Pure Rust Wire write。Pure Rust `0x8002` codec 由 W-PMC-5
> （PR #68）单独证明。二者组合后才进入 W-PMC-6。

## 3. stdout（完整，10/10 gate PASS）

```text
受控写测试 192.168.15.165:8193，仅 R100.0，BYTE length=9
pmc_rdpmcrng R100 BYTE start=100 end=100 length=9 rc=0 value=0x00
pmc_rdpmcrng R101 BYTE start=101 end=101 length=9 rc=0 value=0x00
before: R100 raw = 0x00 / bit0 = 0；R101 raw = 0x00 / bit0 = 0
pmc_wrpmcrng R100 BYTE start=100 end=100 length=9 rc=0 value=0x01
pmc_rdpmcrng R100 BYTE start=100 end=100 length=9 rc=0 value=0x01
after set write rc=0；R100 raw = 0x01 / bit0 = 1
pmc_rdpmcrng R101 BYTE start=101 end=101 length=9 rc=0 value=0x01
after set: R101 raw = 0x01 / bit0 = 1
pmc_wrpmcrng R100 BYTE start=100 end=100 length=9 rc=0 value=0x00
pmc_rdpmcrng R100 BYTE start=100 end=100 length=9 rc=0 value=0x00
pmc_rdpmcrng R101 BYTE start=101 end=101 length=9 rc=0 value=0x00
restore: write rc=0；R100 raw = 0x00 / bit0 = 0；R101 raw = 0x00 / bit0 = 0（期望 R101.0=0）
```

## 4. 通过门（10/10）

```text
before 双端快照 R100.0==0 && R101.0==0（fail-closed，不写 PMC） ✅
RMW 只置位 R100.0（new = original | 0x01）        ✅
R100 回读 bit0==1 + 其他 bit 与 original 一致    ✅
≥1 scan 等待（100ms）后 R101.0==1                ✅（核心 propagation 证据）
restore 完整 original BYTE（禁硬编码 0x00）       ✅
restore 后双端 readback（R100==original）         ✅ 0x00 / 0x00
original bit0==0 → R101.0==0                     ✅
finally restore（本次一次通过，未触发失败路径）   ✅
FOCAS 调用：connect 成功 + 8 次 PMC rc=0 + close 无错误 observed ✅
R101 全程只读（无 R101 写调用）                   ✅
```

## 5. 口径

```text
Native pmc_wrpmcrng
→ R100 BYTE
→ R100.0
→ Ladder scan
→ R101.0
→ readback
→ restore original BYTE
→ R100/R101 both return 0
```

- W-PMC-2（ladder consumption semantics）与 W-PMC-5（Pure Rust `0x8002`
  codec）为两条独立证据；组合后进入 W-PMC-6 production control integration。
- 不做 WORD/DWORD/range write；不写 R101；不改 ladder；不进生产控制面。
