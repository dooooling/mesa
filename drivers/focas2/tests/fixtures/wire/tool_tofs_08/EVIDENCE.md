# Gate 3-C2 Evidence — `0x08` request/response differential ✅ CLOSED

冻结日期：2026-09-28（UTC+8 现场窗口）
状态：Wire differential core 闭合；chronological cross-window parity 明确
NOT-PROVEN；不含 codec/production fix/dumper。

## 1. 采集与配对口径

- CNC：NCGuide `192.168.15.165:8193`，MDI / STOP（全程）。
- 抓包：管理员 pktmon（filter `192.168.15.165 TCP 8193`）→ `tofs3c2.etl`
  → `tofs3c2.pcapng`（3216 包，20 流；dumper 连接为 c2s=100B 流 ×10）。
- Native 同窗：`tofs_dump_direct_matrix`（`MESA_FOCAS_TOOLS=16`，
  `MESA_FOCAS_TOFSTYPES=1/3` 单格双抓；W0/W1/W2/W0 四窗 + 1 作废窗
  INPUT 未落定；作废窗映射见 §5 边界，不纳入差分；dumper 留工作区）。
- 配对权威：**TCP flow 内 request/response pairing is authoritative.
  Cross-flow chronological ordering is unavailable/untrusted**
  （dumper 运行顺序与端口/mtime 错位；fixture 按 observed selector/value
  identity 命名，不用推断的 W0/W1/W2 执行标签）。

## 2. 原始证据（provenance，外部保存，不进 main 历史）

- `tofs3c2.pcapng`（434240B）
  `SHA256 45C830DF2453A5F4BBCB84F937160AE59016D25E9648633960BE81C8B6CCA106`
- `tofs3c2.etl`（120129B）
  `SHA256 5C3E9FB35053CC162F95B7DB62D947761E4E57950736CD5BABF6DBA3C6C8676E`
- `tofs3c2_flows/`（20 流；100B c2s 流 ×10 为 dumper 连接）
- fixture（逐帧完整 FOCAS frame，含 10B header；SHA256 见 `SHA256SUMS`）：
  `drivers/focas2/tests/fixtures/wire/tool_tofs_08/`
  `tool16-type1-value5000.req/res`（40B/36B）
  `tool16-type3-value10000.req/res`（40B/36B）
  `tool16-type1-zero-01..04.req/res`（40B/36B ×4）
  `tool16-type3-zero-01..04.req/res`（40B/36B ×4）

## 3. request selector ✅

```text
cmd  = 0x0008 (dev=1/path=1/count=1/size=28/aux=0/dlen=0)
arg0 = 16 (0x10, tool number)
arg1 = 16 (0x10, tool number echo)
arg2 = 1001 (0x3E9) ↔ Native type=1 (offset)
     = 1003 (0x3EB) ↔ Native type=3 (length)
arg3 = 0
两请求仅 arg2 不同 ✅（observed：type=1→1001 / type=3→1003；
不命名 1000+type 语义，见 §5）。
```

## 4. response value slot ✅

```text
status = 0 / dlen = 8 / data 8B：
  data[0:4] = BE32 value（00000000 / 00001388=5000 / 00002710=10000）
  data[4:6] = 000a / data[6:8] = 0003（恒定，不命名）

observed non-zero：
  arg2=1001 → 00001388 = 5000 ✅
  arg2=1003 → 00002710 = 10000 ✅
  与 Native data（5000/10000）一致 ✅
```

旧“32B stride”口径：❌ 不适用于本次 single-point `0x08`
（本次实证 `dlen=8`；32B 疑为 `cnc_rdtofsr` area 版形态，不硬套）。

## 5. 明确 NOT-PROVEN（不写 CLOSED）

```text
Wire wrong-selector controlled-window parity ⏳ NOT-PROVEN：
8 条 zero response 证明“0x08 对这些 selector 存在 status=0/data=0 的
成功零值响应”✅，但 cross-flow 时序不可信，不能仅凭内容证明“它就是
发生在对侧非零 controlled window”。该结论由 Native 3-C1 已证明；
Wire 同构待 tagged 双窗抓包（后续小窗口，不重做四窗）。
```

存在 1 个 INPUT 未落定作废窗；因 cross-flow chronological ordering
unavailable，无法将其可靠映射到具体 zero fixture。全部 zero fixtures
仅作为 observed-zero provenance，不用于 controlled-window
parity/differential。

## 6. 口径（3-C3 codec 输入）与边界

```text
offset → arg2=1001
length → arg2=1003
response dlen=8
value = BE i32 @ data[0..4]
scale = /1000（来自 3-C1 semantic evidence）
```

- 本文件冻结后，3-C3 codec 不得反过来重新定义本文件字节。
- 3-C3 只 admit single-point `0x08`（本实证形态）；不扩 area/range。
- 3-C2 CLOSED ≠ 所有关于 `0x08` 的历史形态都 CLOSED：
  本轮只关闭本次 single-point tool offset/length 所需的 Wire identity。
