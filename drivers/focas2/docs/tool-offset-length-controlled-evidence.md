# Gate 3-C1 Evidence — tool offset/length controlled semantics ✅ CLOSED

冻结日期：2026-09-28（UTC+8 现场窗口）
状态：Native + Panel controlled semantics 闭合；不含 Wire/dumper/pcap/codec/production fix。

## 1. 现场条件

- CNC：NCGuide `192.168.15.165:8193`，MDI / STOP（全程）。
- tool number：`#16`（同一号下 offset/length 分别闭合）。
- dumper：`tofs_dump_direct_matrix`（官方 5 参直调
  `cnc_rdtofs(hdl, tool_no, type, length=8, out)`，不经生产试探/回退；
  工作区未提交，3-C2 复用）。

## 2. tool/length — `#16 LENGTH GEOM` ✅ CLOSED

- selector：`type = 3`；scale：`raw / 1000`。

```text
T0：      panel 0.000  ↔ type=3 data=0
T1A：     panel 10.000 ↔ type=3 data=10000
T1B：     panel 20.000 ↔ type=3 data=20000
restore： panel 0.000  ↔ type=3 data=0
```

- `type=0/1/2` 全程 `data=0`（排除）。
- 其余三格（LENGTH WEAR / RADIUS GEOM / WEAR）全程 `0.000`（无联动）。
- Panel 截图三轮 + Native 矩阵三点单调 + restore 闭环（全闭合，无 debt）。

## 3. tool/offset — `#16 RADIUS GEOM` ✅ CLOSED

- selector：`type = 1`；scale：`raw / 1000`。

```text
T0：      panel 0.000 ↔ type=1 data=0
T2A：     panel 5.000  ↔ type=1 data=5000
T2B：     panel 8.000  ↔ type=1 data=8000
restore： panel 0.000 ↔ type=1 data=0
```

- `type=0/2/3` 全程 `data=0`（排除；`type=3` length 未被联动）。
- 四格确认三轮（LENGTH GEOM/WEAR / RADIUS WEAR 全程 `0.000`）。
- Panel + Native + restore 全闭环。

## 4. operation identity（同 `#16` 下已分）

```text
length = cnc_rdtofs(type=3)
offset = cnc_rdtofs(type=1)
type=0/2 excluded（两线皆零）
```

函数相同 ≠ 语义相同：`command + request selector/type + response slot +
scale + Mesa semantic` 已分一半（slot/scale 待 3-C2 Wire 差分补齐）。

## 5. production debt（3-C3 前必须修）

```text
cnc_rdtofs() 首试 type=0 → 全零即 Ok 返回（fail-open）：
length 10.000/20.000、RADIUS 5.000/8.000 时 type=0 读到 0.0 且不报错。
→ production 必须按 selector 分流：
  offset → type=1
  length → type=3
→ 禁试探（首个 rc=0 即返回）。
```

## 6. 范围外

- `tool/zofs` 独立 family，不在本轮（→ Gate 3-D）。
- Wire `0x08` request/response 差分 → Gate 3-C2。
- `native.rs` dumper / pcap-etl / codec / production fix 均不在本文件。
