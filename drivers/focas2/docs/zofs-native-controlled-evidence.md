# Gate 3-D1/D2 Evidence — zofs Native ABI + controlled semantics ✅ CLOSED

冻结日期：2026-09-28（UTC+8 现场窗口）
状态：Native ABI 修正 + controlled 双非零 + restore 闭合；不含 Wire/codec/fix。

## 1. 现场条件

- CNC：NCGuide `192.168.15.165:8193`，MDI / STOP（全程）。
- 面板：EXT X/Y/Z=0；G54 X controlled（0→12.345→23.456→0），G54 Y/Z=0，
  G55 X/Y/Z=0（全程未联动）。
- dumper：`zofs_dump_direct_matrix`（D1-R1）→ `zofs_dump_d2_window`
  （D1-R2/D2；正确单点形态，同一 handle 顺序执行；工作区未提交，
  D3 复用）。

## 2. D1-R1 ABI misuse evidence（旧模型作废）

- 旧调用把第 4 参当 `type`：`cnc_rdzofs(hdl, s=num, e=num, type=0..3)`。
- 结果：`num=1,2 × type=0,1,2,3` 共 8 格，**8/8 `rc=2`（EW_LENGTH）**。
- 零初始化保留（全拒下 data 全零，无垃圾值）。
- 结论：旧 `s_no/e_no/type` 模型 ❌ 作废。

## 3. D1-R2 corrected ABI ✅

反编译（fwlibe64 `0x34cf8`）+ 实测双闭合：

```text
cnc_rdzofs(hdl, worknum, axis, len, out)
  worknum = 1 → G54 ✅（回显 raw[0:2]=01 00）
  axis    = 1 → X ✅ / 2 → Y / 3 → Z（回显 raw[2:4]）
  len     = 8 → 单轴容量门 ✅（公式 4*v12+4，见反编译）
```

实测（a2=1，G54 X=12.345）：

```text
R2-A len=7 → rc=2（EW_LENGTH）✅ / len=8 → rc=0 ✅
R2-B a3=0 → rc=4（EW_NUMBER；0 非法 axis）✅
R2-B a3=1 → rc=0 raw[4:8]=39300000 → LE32 12345 ✅
R2-B a3=2 → rc=0 raw=0 ✅（Y=0）
R2-B a3=3 → rc=0 raw=0 ✅（Z=0）
R2-C a3=-1 len=15/16 → rc=2 ⏳（全轴分支未触发，NOT-PROVEN，不猜）
```

- 36B `IodbZofs` 布局兼容（`4+N*4` 容量公式吻合）；field semantics 待定，
  不改 struct。
- `cnc_rdzofsr` 参数身份不命名（等反编译）。

## 4. D2 controlled semantics ✅

正确单点形态（`worknum, axis, len=8`），同一 handle 顺序 3 次
`(1,1)/(1,2)/(2,1)`（G54X/G54Y/G55X）：

```text
R2-B：   G54 X=12.345 → (1,1)=12345 / (1,2)=0 / (2,1)=0 ✅
D2-A2：  G54 X=23.456 → (1,1)=23456(a05b0000 LE) / (1,2)=0 / (2,1)=0 ✅
D2-B：   G54 X=0.000  → (1,1)=0 / (1,2)=0 / (2,1)=0 ✅
```

- number mapping：`worknum=1 → G54` ✅（回显 + G55X 全程零对照）。
- axis mapping：`axis=1 → X` ✅（Y 同槽零对照）。
- scale：`raw / 1000` ✅（12345/23456 双点单调）。
- restore ✅（三零）。

作废窗（归档标注，不纳入证据）：

```text
D2-A first run INVALID：INPUT not committed, returned previous value (12345)。
not used for scale/monotonic evidence（与 3-C2 同例）。
```

## 5. production debt（D4 前必须修）

```text
当前生产 cnc_rdzofs() 传 type=[0,1] 给第 4 参；第 4 参实际是 length。
→ production tool.zofs 当前 loud fail（全 Length；与 tofs silent
  wrong-value 不同形态，同属 selector debt）。
→ D4 必须按 worknum/axis/len 分流，禁 type 试探。
```

## 6. 边界（明确 NOT-PROVEN / 不冻结）

- `all-axis a3=-1`：`len=15/16` 皆 Length → ⏳ NOT-PROVEN。
- `out` 前 4 字节字段语义不命名；只冻结 `value @ raw[4:8] LE`。
- Wire `command=0x0B`：反编译索引仅为候选；D3 以真实抓包为准。
- 不含 codec / production fix；dumper 留工作区；ETL 不进 main。

## 7. 原始证据（provenance，外部保存，不进 main 历史）

- `zofs-d1-g54-12345.etl`（55529B）
  `SHA256 4F7A56EDB18D65B8C5D9D0E4CCD3E20CD10434E5F3978E00CAF83F56E98D2881`
- `zofs-d1r2-abi.etl`（58444B）
  `SHA256 50809CEA45918913EB74ABD93B1C39E0EAB8F37E5F69B86A8A6D9F9822089025`
- `zofs-d2-g54-23456.etl`（16MB，同窗含 D2-A/D2-A2/D2-B；pktmon 仍持有，
  SHA256 待收包后补）
