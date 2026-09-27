# W-PMC-3A Evidence Archive — R100 BYTE write `0x00 → 0x01 → 0x00`

冻结日期：2026-09-27（UTC+8 现场窗口）
状态：`W-PMC-3 capture ✅ CLOSED`；本文件为 `W-PMC-3A archive`（只冻结证据，不写 codec）。

## 1. 采集环境

- CNC：NCGuide `192.168.15.165:8193`，STOP 状态（面板确认）。
- 采集主机：`192.168.15.97`，TCP 源端口 `65283`（target165:8193 对端）。
- 抓包：管理员 PowerShell `pktmon`（filter `192.168.15.165 TCP 8193`，
  `--pkt-size 0` 全包）→ `wpmc3.etl` → `wpmc3.pcapng`。
- 写 harness：`drivers/focas2/examples/support/ncguide_write.rs`
  （`pmc_rdpmcrng` BYTE `length=9` + `pmc_wrpmcrng` BYTE `length=9`，
  不用 WORD，restore 放 finally）。

## 2. 原始证据（provenance，外部保存，不进 main 历史）

- `wpmc3.pcapng`（56312B）
  `SHA256 DA67C9445A9A233D7F0B1941878331E9AECC73E44F25A965BA35E13A149FF35C`
- `wpmc3.etl`（61176B）
  `SHA256 A562B448A0325EC20C5999E627D962DA91587C578B7BE4A2212B1AD1B27CA5AA`
- `wpmc3_flows/192_168_15_97_65283/c2s.bin`（268B）/ `s2c.bin`（590B）
  （`tools/gate0_extract.py` 按四元组重组；`65282` 流为 16B/394B 背景流，
  与 S0~S4 无关；流提取物 SHA256：见下 §2.1）。
- 采集端点：`192.168.15.97:65283 → 192.168.15.165:8193`（TCP）。

### 2.1 流提取物 SHA256（`wpmc3_flows/192_168_15_97_65283/`）

- `c2s.bin`（268B）
  `SHA256 4BC474646A00630094038BFB09114746DC95D9CC0B2C632BB2D3CE8A234AD4D0`
- `s2c.bin`（590B）
  `SHA256 FC3ED165E83071789113386705A502C5E4ED0FDDA09B4A883288162CE8EAF0DB`
- fixture frame SHA256（逐帧完整 FOCAS frame，含 10B header）：
  `req_read_s0/s2/s4`（40B）`fefc8f98e0280dc0…`（三帧同字节：同形读请求）；
  `req_write_s1_01`（41B）`099d6fb92679b0ec…`；
  `req_write_s3_00`（41B）`1fb6687ad37d0d1c…`；
  `resp_read_s0/s4`（29B）`4625bf60cff884a4…`（同字节：同值 00 回执）；
  `resp_read_s2`（29B）`2882aed0ad5e2f97…`（值 01 回执）；
  `resp_write_s1/s3`（28B）`9cf2d3d99ce9e3fd…`（同字节：全零 status）；
  `resp_selfcheck_c0`（46B）`d085af3edb01ecae…`
  （provenance-only，非 S0~S4 合同；见 §2.2）。
  （上为 SHA256 前 16 hex 作索引；全值以文件为准。）

### 2.2 `resp_selfcheck_c0` 口径

provenance-only：C#0 `0x00010001` 自查回执（46B frame），非 S0~S4
写/读合同一部分。保留原因：首次抽取曾将其误标为 `resp_read_s0`，
靠 func echo 发现并纠正——保留作配对审计证据。不参与 W-PMC-5 codec 测试。
- fixture（逐帧完整 FOCAS frame，含 10B header）：
  `drivers/focas2/tests/fixtures/wire/pmc_write_byte/`
  `req_read_s0/read_s2/read_s4`（40B frame / 30B GENERIC）
  `req_write_s1_01/write_s3_00`（41B frame / 31B GENERIC）
  `resp_read_s0/read_s2/read_s4`（29B frame / 19B GENERIC）
  `resp_write_s1/write_s3`（28B frame / 16B GENERIC）
  `resp_selfcheck_c0`（46B frame，C#0 `0x00010001` 自查回执，非 S0~S4）。

## 3. S0~S4 操作/包对应关系

```text
S0 read      req  C#1 0x8001 → resp S#2 0x8001 data=00   (R100 == 0x00)
S1 write     req  C#2 0x8002 → resp S#3 0x8002 status区全零 (R100 := 0x01)
S2 readback  req  C#3 0x8001 → resp S#4 0x8001 data=01   (R100 == 0x01 ✅)
S3 restore   req  C#4 0x8002 → resp S#5 0x8002 status区全零 (R100 := 0x00)
S4 readback  req  C#5 0x8001 → resp S#6 0x8001 data=00   (R100 == 0x00 ✅)
```

Native 五步（`rc=0` 全程）：
`rd 0x00 → wr 0x01 → rd 0x01 → wr 0x00 → rd 0x00`，恢复已确认。

## 4. OBSERVED（冻结，不解释）

```text
write command      = 0x8002
read command       = 0x8001
start = end        = 0x0064 (100, BE16)
area R             = 0x0005 (BE16, u32 arg2 高位语义见下 layout)
BYTE dtype         = 0x0000

WRITE-1 vs WRITE-0：仅 1 字节差异（GENERIC payload byte 30：01 ↔ 00），
其余 request bytes 全相同。

write request GENERIC payload = 31B（subpacket size = 29）
read  request GENERIC payload = 30B（subpacket size = 28）

write response GENERIC payload = 18B（subpacket size = 16）：
command echo 0x8002 + 12B zero/status region，无 value echo。
```

`000001XX` 尾部当前只能叫 `constant byte/field = 0x01` +
`payload value = XX`（W-PMC-4 对齐真实模型后：`aux=0x0000` +
`data_len=0x0001` + `data=[XX]`；命名以 §5 layout 为准）。

## 5. W-PMC-4 layout（按真实模型 offset，GENERIC payload 相对偏移）

```text
PMC WRITE / 0x8002 request (31B: count + subpacket[29]):

offset  size  observed                 semantic
0       2     00 01                    count = 1
2       2     00 1d                    subpacket size = 29 (28 + data_len)
4       2     00 02                    device = 2 (PMC)
6       2     00 01                    path = 1
8       2     80 02                    command = 0x8002 (write)
10      4     00 00 00 64              arg0 = start = 100
14      4     00 00 00 64              arg1 = end = 100
18      4     00 00 00 05              arg2 = area (R = 5)
22      4     00 00 00 00              arg3 = dtype (BYTE = 0)
26      2     00 00                    aux (frozen, observed zero)
28      2     00 01                    data_len = 1
30      1     XX                       payload value (S1: 01 / S3: 00)

PMC WRITE response (18B: count + subpacket[16]):

offset  size  observed                 semantic
0       2     00 01                    count = 1
2       2     00 10                    subpacket size = 16 (16 + data_len)
4       2     00 02                    device = 2 (echo)
6       2     00 01                    path = 1 (echo)
8       2     80 02                    command echo = 0x8002
10      2     00 00                    status = 0 (i16)
12      2     00 00                    detail1 (frozen)
14      2     00 00                    detail2 (frozen)
16      2     00 00                    data_len = 0 (无 value echo)
```

请求/响应读写对照（GENERIC payload byte 级 diff）：
读请求 30B vs 写请求 31B 差异仅 4 处：
`byte2-3 size 001c→001d` / `byte8-9 command 8001→8002` /
`byte28-29 0000→0001`（aux 尾/data_len 语义见上）/ `byte30 新增 XX`。
WRITE-1 vs WRITE-0 仅 `byte30: 01 ↔ 00`。

## 6. 口径

- 本文件冻结后，W-PMC-5 codec 不得反过来重新定义本文件字节
  （代码服从证据；fixture 为准）。
- W-PMC-5 第一版只 admit：PMC BYTE + single address + `start == end` +
  1-byte payload；不扩 WORD/DWORD/range write（无写证据）。
- 负测试（synthetic，非 evidence fixture；W-PMC-5 已冻结）：
  captured positive clone/mutate → wrong command echo / truncated
  response / non-zero remote status（见 `pmc_write_byte_locked`）。
