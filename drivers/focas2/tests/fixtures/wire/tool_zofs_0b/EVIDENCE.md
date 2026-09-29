# Gate 3-D3 Evidence — zofs `0x0B` request/response differential ✅ CLOSED

冻结日期：2026-09-29（UTC+8 现场窗口；D1-R2 + D2 同窗取证）
状态：command + selector 差分 + value slot + controlled 三点闭合；
不含 codec/production fix/address 决策。

## 1. 采集与配对口径

- CNC：NCGuide `192.168.15.165:8193`，MDI / STOP（全程）。
- dumper：同一 handle 顺序执行（TCP stream order = run order；
  3-C2 跨流错位教训已吸收）。D1-R2 单流 8 调用；D2 每窗单流 3 调用。
- 配对权威：**TCP flow 内 request/response pairing is authoritative**。
- D2-A 首跑 INPUT 未落定作废窗：INVALID，不纳入 frozen evidence
  （本归档的 `g54x-12345` 来自 D1-R2 有效 12.345 窗，非作废窗）。

## 2. 原始证据（provenance，外部保存，不进 main 历史）

- `zofs-d1-g54-12345.etl`（55529B）
  `SHA256 4F7A56EDB18D65B8C5D9D0E4CCD3E20CD10434E5F3978E00CAF83F56E98D2881`
- `zofs-d1r2-abi.etl`（58444B）
  `SHA256 50809CEA45918913EB74ABD93B1C39E0EAB8F37E5F69B86A8A6D9F9822089025`
  → `zofs-d1r2-abi.pcapng`（57800B，432 包；dumper 流 `192_168_15_97_56244`）
- `zofs-d2-g54-23456.etl`（73969B）
  `SHA256 E34063F66C5634748DF3E8A2B2357DF8936A1BDB12B33AF77ED6D56CD11EFD94`
  → `zofs-d2-g54-23456.pcapng`（148184B，1092 包；dumper 流 `58563/63181/64663`）
- fixture（逐帧完整 FOCAS frame，含 10B header；SHA256 见 `SHA256SUMS`）：
  `drivers/focas2/tests/fixtures/wire/tool_zofs_0b/`（8 组 req/res，
  req 40B / res 36B，`axis0-status4.res` 28B 除外）

## 3. framing（与 transport 同源锁定，避免抄串）

```text
C→S request:  a0a0a0a0 0001 21 02 ...
S→C response: a0a0a0a0 0003 21 02 ...
（request spec=0x0001 / response spec=0x0003；既有 transport 结论不变）
```

## 4. request selector ✅

```text
command = 0x000B ✅（12.345 / 23.456 / restore 三窗 + 对照调用同 family）

single-point request:
  count = 1 / size = 28 / dev = 1 / path = 1 / cmd = 0x000B
  aux = 0 / dlen = 0 / data = (none)

  G54/X:   args = [1,1,1,0]
  G54/a3=2: args = [1,1,2,0]
  G54/a3=3: args = [1,1,3,0]
  G55/X:   args = [2,2,1,0]

Observed differential:
  work-number change 1→2:
    arg0 + arg1 change together 1→2 ✅
  Native axis selector change:
    only arg2 changes ✅
  arg3 = 0 in all observed single-point windows ✅

Do NOT assign separate semantics to arg0 vs arg1.
```

## 5. response value slot ✅

```text
status = 0 / dlen = 8

data[0:4] = BE i32 value ✅（Native LE32 ≠ Wire BE32；endian 已重证）
data[4:8] = 00 0a 00 03 observed constant，semantic unnamed
```

## 6. controlled closure ✅

```text
G54 X panel 12.345 → 00003039 → BE32 12345 ✅（D1-R2 有效窗）
G54 X panel 23.456 → 00005ba0 → BE32 23456 ✅
G54 X panel  0.000 → 00000000 →     0 ✅

scale = value / 1000 ✅
```

## 7. 负向 evidence ✅

```text
Native a3=0 → Wire status=4 / Native rc=4 ✅
只冻结该条件下 error-code parity；不泛化所有 0x0B errors。
```

`len=7` 调用语义（收紧措辞，不宣称内部阶段）：

```text
len=7:
  Wire request emitted ✅（[1,1,1,0] 同形）
  CNC response status=0 + value=12345 ✅
  Native API returns EW_LENGTH ✅

→ length capacity policy is host/DLL-side semantics
→ length is not encoded in the observed single-point Wire request
→ 不宣称该检查发生在 send 前还是 response 后的具体内部阶段
```

## 8. 边界（明确 NOT-PROVEN / 不命名 / 不含）

- `a3=2 ↔ Y / a3=3 ↔ Z` ⏳ NOT-PROVEN。
- `all-axis a3=-1` ⏳ NOT-PROVEN。
- `arg0/arg1` 独立字段语义不命名。
- tail `000a0003` 不命名。
- `cap8` no-second-request 原因不解释（OBSERVED，不命名 caching 等）。
- `all15/all16` no-request 原因不泛化（OBSERVED）。
- 不含 codec / production fix / Mesa zofs axis address contract 决策。
