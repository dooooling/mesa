# FOCAS Load DLL 证据索引（PR-A）

初始基线：`main@248773e`；最新复核基线：`main@a9b3690` + 未提交的研究入口。
研究入口只在测试构建中存在，15/17 READY、Load HOLD 未变。精确采样二进制哈希见新归档索引。

## 状态词典

| 标记 | 含义 |
| --- | --- |
| `observed` | 真机/DLL 直接观测的原始字节 |
| `inferred` | 由观测推导、待复核的解释 |
| `synthetic-derived` | 离线合成、明确非真机的派生样本 |
| `static-verified` | 已核对指定哈希 DLL 的反编译、汇编或常量；不代表运行时执行过 |
| `unknown` | 尚未取得证据 |

## 证据清单

| # | 证据 | 状态 | 位置 |
| --- | --- | --- | --- |
| E1 | 165 spindle type=0/1/-1 历史误请求/响应原始帧 | `observed`（A4[2]，不作为正确主轴请求证据） | `drivers/focas2/tests/fixtures/wire/spindle_load_165/` |
| E2 | Native `num_out=1` + 24B 单 record（NCGuide 零负载） | `observed` | 新归档 `load_pair_165_20261010/type0/expected.json`；完整缓冲在本地 samples |
| E3 | DLL 完整文件身份（SHA-256 + 版本） | `observed`（本地文件，非运行时加载证明） | `target/focas-load-research/dll-manifest.csv` |
| E4 | 运行时实际加载模块（当前进程模块快照） | `observed`（调用后快照，非 Load Image 事件全历史） | `pair-20261010-002/samples/modules-*.json`；归档 index 保留模块名与哈希 |
| E5 | Native 完整网络会话（OPEN/GENERIC/CLOSE） | `observed`（测试 PID 的 TCP 四元组确认归属） | `target/focas-load-research/pair-20261010-002/capture.pcapng`（本地暂存，不进仓） |
| E6 | Native↔Wire 四种请求差分与 ABI 对账 | `observed`（零负载、顺序采样） | `drivers/focas2/tests/fixtures/wire/load_pair_165_20261010/` |

## 抓包操作流程（Windows 管理员）

```powershell
pktmon status
pktmon filter list
# 确认可清理后：
pktmon filter remove
pktmon filter add FOCAS165 -i 192.168.15.165 -p 8193 -t tcp

$outDir = "D:\Mine\Project\mesa\target\focas-load-research"
New-Item -ItemType Directory -Force $outDir | Out-Null
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

## pcapng → 可比较帧（离线，不连 CNC）

`pktmon etl2pcap` 只产 pcapng，不做 TCP 重组/FOCAS 拆帧。按以下步骤离线提取：

```text
1. Wireshark 打开 pcapng，按 `ip.addr==192.168.15.165 && tcp.port==8193`
   过滤，确认唯一 TCP 会话（client 端口 + server 8193；多会话即分流，
   不得混流——TCP 流归属以 (client_ip:port ↔ server_ip:port) 四元组为准）。
2. 选中会话 → Follow → TCP Stream，分别导出 client→server、
   server→client 两个方向的原始 payload（Raw 字节）。
3. 按 10B 帧头切分：`magic a0a0a0a0 + origin(2) + type(2) + len(2)`，
   `len` 即后续 payload 字节数；切分后必须恰好耗尽（多余尾部即会话污染）。
4. 帧文件命名：`open_req.bin/open_resp.bin/generic_req_N.bin/...`，
   存入独立 frames 目录（本地暂存，不进仓）。
5. 差分（逐字节 + 退出码；一致 0，不一致/损坏非零）：
   python3 tools/ci/focas_diff.py scan <frames-dir>
   python3 tools/ci/focas_diff.py compare \
     --native <frames-dir>/generic_req_1.bin \
     --wire drivers/focas2/tests/fixtures/wire/spindle_load_165/type0/request_frame.bin
```

不自己实现 PCAP 解析器；会话归属错误、方向混淆、跨连接拼接一律视为证据污染。

## 24B 说明

Native `num_out=1` 对应一条 `OdbSpLoad` 的 24B ABI record span
（结构跨度；type=0 的 speed 半区 sentinel 未写，不代表 DLL 改了全部字节）。

## DLL 身份冻结

```powershell
pwsh tools/ci/dll-manifest.ps1
```

静态清单 + ProcMon 实际加载模块双轨，SHA-256 对照，不标未观察模块为已加载。

## 2026-10-10：DLL 反编译与已有抓包交叉复核

本轮代码基线为 `main@a9b3690`。范围为当前 DLL 的 `cnc_rdspmeter` /
`cnc_rdsvmeter` 请求与数据转换路径，不是全部 FOCAS 函数、版本或设备型号的协议证明。
使用本机 IDA 9.2 / Hex-Rays 导出伪代码及汇编；伪代码类型、变量名不是原始源码，
分支、偏移、常量需与汇编对照。没有修改 DLL，没有连接设备执行新的读写调用。

### 文件身份与静态入口

DLL 来自 `drivers/focas2/libs/win/`，完整清单在本地 `target/focas-load-research/dll-manifest.csv`。

| DLL | SHA-256 | `rdspmeter` RVA | `rdsvmeter` RVA |
| --- | --- | --- | --- |
| FWLIB64.dll | `7B3837E3925902D1E06C5C7E4F9CE39FE7F39AA9CAED029096187F5641EC04AB` | `0x22970` | `0x22860` |
| fwlibe64.dll | `D3341B43BF3945BDB49D03D1D96436A75911BF3C8D1CDF4F30B9167EF935BFD6` | `0x5CB50` | `0x5CA98` |
| fwlibNCG64.dll | `2A15EF18AD1823D7F5FA6C1566654CCD0FAB6C72D0706449E355231E5809967F` | `0x1D500` | `0x1D4C0` |

`FWLIB64` 按句柄标志和模块表分发，并通过 `GetProcAddress` 取得对应函数。
这证明存在模块分派，不能单凭静态结果证明进程实际加载了哪一个模块。
此静态复核阶段 E4 尚未取得；后续直接采集结果见下节。

`fwlibe64` 的两个入口检查连接上下文 `DWORD[context+12] & 4`，分别进入通用或传统分支。
该标志初始化及完整协商过程尚未追踪，不能把这个位直接命名为某个设备能力。

### 已确认的通用请求构造（static-verified）

下表省略为零的后续参数；命令均为十六进制。`type=-1` 同时读负载和速度。

| 路径 | 单个 GENERIC 批次内的子包顺序 | 核对位置（fwlibe64 RVA） |
| --- | --- | --- |
| 主轴负载 `type=0` | `A4[1] → 8A[0] → 40[4,-1] → A4[1]` | `0x58824`，分支参数 `a6=0` |
| 主轴速度 `type=1` | `A4[1] → 8A[0] → 40[5,-1] → A4[1]` | 同上 |
| 主轴两项 `type=-1` | `A4[1] → 8A[0] → 40[4,-1] → 40[5,-1] → A4[1]` | `0x5B010 → 0x5A3E4 → 0x5932C → 0x58824` |
| 伺服负载 | `A4[2] → 89[0] → 56[1,0] → A4[2]` | `0x5AFD8 → 0x5A3E4 → 0x5932C → 0x58424` |

主轴双 `A4` 参数 `1` 固定来自表，不是调用者 `num_in`。
`num_in` 用于响应后限制返回数量；主轴通用分支取调用者数量与前后两个计数的最小值。
本轮直接读取二进制常量表：

| RVA | 元素类型 | 数值 |
| --- | --- | --- |
| `0xFA268` | DWORD | `[1,1,3]` |
| `0xFA278` | DWORD | `[4,5,3,7,6,8,9,10]` |
| `0xFA298` | DWORD（有符号解释） | `[-1,0]` |
| `0xFA2A0` | DWORD | `[1,3]` |
| `0xFA2CC` | WORD | `[0,1]` |

网络数值记录跨度为 8B，主轴通用分支读取 `raw@0`（网络序 DWORD）和
`decimal@6`（网络序 WORD）；`aux@4` 不能直接当作输出 ABI 单位。
内部记录为 16B，`0x5A3E4` 转换成对外 12B `LOADELM`，
其中内部单位 `8 → ABI 0`，其他单位 `→ ABI 1`（此处限定类别 2/3）。
通用主轴负载设置内部单位 8，速度设置 5。伺服负载对原始值取绝对值。
`OdbSpLoad` 两半合计 24B，但单选 type 只写选中的半区；未写半区不能当有效数据。

### 传统与本地 NCGuide 路径（static-verified，缺动态验证）

- 传统主轴 `0x5B048` 构造 `40[4,-1] / 40[5,-1] / 8A[0]`，没有双 `A4`；
  第一子包返回状态 4 时记录上下文标志并转旧缩放路径 `0x56A34`。
- 传统伺服 `0x57C1C` 构造 `56[1,0] / 89[0]`，没有双 `A4`，取绝对值后设置单位 0。
- 旧主轴缩放路径读取 `40[0,-1] / 40[1,-1] / 8A[0]`，以及命令 `0E`
  的参数 **4127、4274、4020、4196**。这些数是参数编号，实际乘数来自参数响应值。
  名称第三字节为 `'2'` 时选择 4274/4196，否则选择 4127/4020；
  负载除数 32767、速度除数 16383，输出小数位 0。
  汇编乘法为 32 位；Rust 的宽整数乘法不能宣称在溢出输入上与 DLL 等价。
- `fwlibNCG64` 的两个入口经过 `0x1CE80 / 0x1CC40` 调用本地
  `cnc_rdaxisdata@0x19C10`。这是另一静态路径，不能用它证明 TCP 服务端运行同一模块。

### 旧抓包与仓库 fixture 的实际差异

原始抓包为 `evidence/captures/spm3a.etl`，SHA-256：
`85ACD9F75E801DA6C67F38D2267217C46E51FDDA296834FEAA28F6648C2E9F42`。
使用 PktMon 离线转换，再由 Scapy 解析；TCP 负载严格按 IP/TCP 声明长度提取，
排除链路层 Padding。按连接四元组及 SYN 会话区分，不跨连接拼接。

共提取四个连接，客户端均为 `192.168.15.97`，服务端为 `192.168.15.165:8193`。
客户端端口 52635/54953 的连接仅包含非 GENERIC 帧；52636/54954 各有 7 对
GENERIC 请求响应，共 14 对（含 2 对系统信息、12 对主轴批次）。
提取流无序列号空洞或重传字节冲突，FOCAS 拆帧恰好耗尽，子包 device/path/command 回显匹配。
转换报告另有 **4 个丢弃包**：以上结论仅证明已提取流闭合，不证明全部调用均被捕获。

旧抓包的主轴批次均为双 `A4[1]`，对应计数为 1；计数范围内的第一记录负载/速度原始值为 0。
缓冲区其余物理记录不能据此解释为有效负载，调用端 `num_in` 未归档。
旧抓包调用程序身份未确认，因此本轮不将 E5/E6 标记为 Native 完整采集或 Native 对账完成。

`spm3a-flows/flow-001/c2s-004.bin` 与现有 `spindle_load_165/type0/request_frame.bin`
都是 124B，但在从零开始的偏移 **23、107** 两处不同：旧抓包 `01`、fixture `02`。
两处均为 `A4` 第一参数末字节。现有 `fixture_tests.rs` 用 `n=2` 同时作为这个参数，
使该 fixture 回归只能证明与探针同源，不能证明符合 DLL 主轴构造。

现有主轴 fixture 的 `A4[2]` 与上述 DLL 主轴路径不一致。
由伺服路径固定使用 `A4[2]` 推断，现有响应中的计数 3 **不能作为正确主轴请求的数量证据**。
原始 fixture 必须保留，修正请求后重新采集，不能改原始字节伪装为重新观测。

将现有 type0 请求仅改这两字节的离线 `synthetic-derived` 样本，与旧抓包逐字节一致。
这只验证该请求的字节构造，不验证当前会话协商、错误分支或非零负载。

### 本地可复核产物与验证边界

产物位于被忽略的 `target/focas-load-research/`，不会随 Git 文档自动分发：

- `ida-FWLIB64/`、`ida-fwlibe64-extra/`、`ida-fwlibNCG64/`：`.c` / `.asm.txt` / `index.json`。
- `spm3a-flows/`：独立连接、方向流、逐帧二进制、SHA-256 与请求解析。
- `load-protocol-review.json`：DLL 身份、常量表、14 对子包对账及候选数据边界。
- `request-delta.json`：原始差异和派生样本对比；其工具 `native` 参数名称不证明旧抓包调用者身份。
- `dump_load_ida.py`、`analyze_capture.py`、`summarize_evidence.py`、`check_request_delta.py`：复核脚本。

验证：`python tools/ci/test_focas_diff.py` 实际执行 15 项，全部通过；
原始请求差分返回不一致，派生请求差分返回一致。没有运行 Cargo 全量测试、
Native DLL 合成响应动态差分或设备调用；本轮未改变生产代码及 Load HOLD。

下一轮验证入口：记录实际加载模块，按正确双 `A4[1]` 与 Native 同窗口采集，
保留 `type / num_in / num_out / rc / 完整输出缓冲区`，再验证非零负载、多主轴、
传统分支、状态 4 回退和整数边界。静态证据足以指导这些用例，不能替代设备兼容性验收。

## 2026-10-10：修正请求后直接采集与回放

已在 `192.168.15.165:8193` 的 NCGuide 执行只读采集，没有操作面板、写 PMC 或改变生产门禁。
新入口为测试模块 `src/wire/load_research.rs`；默认 CI 不执行其 ignored live 测试。
主轴 A4 选择参数固定为 1，构造函数不接收 `num_in`，避免再次混淆计数种类和返回数量上限。
历史请求二进制及哈希全部保留，旧回归明确标记为误请求归档身份检查。

### 同轮采集结果

两轮现场目录分别为 `target/focas-load-research/pair-20261010-001/002/`。
第一轮伺服名称出现差异；第二轮增加测试 PID 的 TCP 连接快照，用于排除背景采集程序的流量。
第二轮 PID 为 28248：Native 连接端口 55594/55595，Wire 连接端口 55596；
捕获中另一个连接不属于该 PID，未纳入对账。
所属流重组无缺口、无冲突重传，按帧头严格拆分，OPEN/GENERIC/CLOSE 均闭合。

| 操作 | Native num_in → num_out | 请求字节 | 数值/小数位/单位/名称 |
| --- | --- | --- | --- |
| spindle type=0 | 2 → 1 | 与 Wire 相同 | 相同，负载 raw=0 / dec=0 / unit=0 |
| spindle type=1 | 2 → 1 | 与 Wire 相同 | 相同，速度 raw=0 / dec=0 / unit=1 |
| spindle type=-1 | 2 → 1 | 与 Wire 相同，5 槽同批次 | 两个半区均相同 |
| servo | 4 → 3 | 与 Wire 相同，双 A4[2] | 三轴数值均为 0，字段相同 |

每次 Native 返回 `rc=0`，前后 guard、512B 外尾部、返回 record 外尾部、未选择半区的 sentinel
检查均通过。只按 selector 读取选中半区；没有把保留字节或未写半区当有效值。
`focas_load_review.py` 同时核对 Native 网络响应与对应 Native ABI，避免拿另一条 Wire 响应冒充 Native 输入。

运行时模块快照观察到临时解压目录中的 `FWLIB64.dll` 与 `FWLIBE64.DLL`；
SHA-256 分别与上节冻结文件一致，未观察到 `fwlibNCG64.dll`。
快照不代表全过程 Load Image 事件，也不覆盖调用中加载后卸载的模块。

第二轮捕获 SHA-256：
`a676038fa1e907b94c231b66f9cb836f05a074165691630eaebec30cf2314651`。
原始 pcapng 留在本地；Npcap 内核丢包统计未取得，不宣称 dropped=0。

### 伺服名称差异的结论

第一轮 Native 收到 `89` 名称项 `58 00 00 00`，Wire 收到 `58 00 44 05`；
对应 ABI / Wire 候选的第三字节不同。第二轮 Native 和 Wire 都收到非零第三字节，候选均保留 `44`。
这说明差异来自不同响应，不是 DLL 总会清零第三字节。
`fwlibe64@0x58424` 复制 4B 名称，`0x5A3E4` 再复制前三字节到 ABI，与捕获一致。
第三/第四字节的业务含义仍未确认：保留原始字节，不根据零窗擅自归一化。

### 可复用入口与新归档

- `tools/ci/focas_load_capture.py`：Npcap 定向捕获 + 只读 Native/Wire 测试；输出目录必须全新。
- `tools/ci/focas_capture_frames.py`：Scapy 解析、按 TCP 四元组及会话重组、严格拆帧；不跨连接拼接。
- `tools/ci/focas_load_review.py`：按测试 PID 确认流归属，核对四种请求、缓冲检查、ABI 字段后归档。
- `tests/fixtures/wire/load_pair_165_20261010/`：每种操作的 Native/Wire 请求响应、候选字段、SHA-256。
  `index.json` 保留捕获/测试二进制哈希与边界。基线 git_sha 不等于干净提交证明，采样时含研究代码。
- `paired_capture_replays_against_native_abi`：调用现有解码器回放全部新响应，对照独立采集的 Native ABI 字段。

复采示例（Windows + Npcap + Scapy；先构建最新 lib 测试二进制）：

```powershell
$artifacts = cargo test --locked -p mesa-driver-focas2 --lib --no-run --message-format=json |
  ForEach-Object { if ($_.StartsWith('{')) { $_ | ConvertFrom-Json } }
if ($LASTEXITCODE -ne 0) { throw '测试入口构建失败' }
$testExe = @($artifacts | Where-Object {
  $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable
}).executable
if (@($testExe).Count -ne 1) { throw '测试二进制不唯一' }
python tools/ci/focas_load_capture.py --test-exe $testExe --host 192.168.15.165 --port 8193 `
  --out target/focas-load-research/pair-new
python tools/ci/focas_capture_frames.py target/focas-load-research/pair-new/capture.pcapng `
  target/focas-load-research/pair-new/flows
python tools/ci/focas_load_review.py target/focas-load-research/pair-new
```

`focas_load_capture.py` 成功表示采集入口完成，不表示所有值相同或具备生产准入；
必须再由 review 核对，否则同轮采样变化会被误报成协议一致。

验证：FOCAS lib 191 项通过、12 项默认忽略；本轮 live 入口实际执行两次各 1 项；
FOCAS all-targets Clippy 通过；既有帧差分 15 项通过。
全量合同证据入口在 workspace 构建时因运行中的 `mesad.exe` / `mesa-driver-focas2.exe`
锁定可执行文件而失败，未产出新的 `contract.json`；不能宣称本轮全量准入通过。

剩余门禁：当前全部有效数值为零；非零负载、多主轴、传统路径、状态 4 回退、整数边界与
真实设备兼容性仍待验证。NCGuide 对账不能替代真机负载验收，Load HOLD 保持不变。

## 2026-10-10：本机派生非零报文与实际 DLL 对照

按用户要求停止本机 Mesa 服务；其 FOCAS 子进程随后退出。实验不启动业务服务，
不接入 NCGuide，不修改原始捕获，仅监听 `127.0.0.1` 的系统分配端口。
入口 `tools/ci/focas_load_synthetic.py` 回放归档 OPEN/SYSINFO/CLOSE，
只接受冻结的四种负载读请求，对未知请求立即失败，不提供写入或远端透传。
服务端有三连接、每连接 16 帧及读超时上限，报文按帧头拆分，并分段发送响应。

响应派生自 `load_pair_165_20261010`，替换数值与小数位；多主轴样本另调整首尾计数、
名称与各层长度。测试名称显式使用 S1/S2；这不是设备观测出的名称。
原始 fixture 与哈希保留，派生报文全部标记 `synthetic-derived`。

| 场景 | 主轴负载 raw / dec → 值 | 主轴速度 raw / dec → 值 | 三轴伺服网络 raw / dec → ABI 值 |
| --- | --- | --- | --- |
| single | 270 / 1 → 27.0% | 1500 / 0 → 1500 r/min | -270 / 1 → 27.0%，560 / 1 → 56.0%，-123 / 2 → 1.23% |
| multi_signed | -125 / 1 → -12.5%，875 / 1 → 87.5% | 12345 / 1 → 1234.5 r/min，6789 / 1 → 678.9 r/min | 7 / 2 → 0.07%，-999 / 1 → 99.9%，1001 / 2 → 10.01% |

主轴负数仅验证协议字段保留符号，不表示有效物理负载；生产适配器仍执行自己的范围与质量判定。
伺服负数在 DLL 与现有解码器中均取绝对值。小数位按 `raw / 10^dec` 解释，
不能把 DLL ABI 原始整数直接显示为百分比。

两个场景各执行 type=0、type=1、type=-1、servo，共八种操作；
Native 返回码均为 0，主轴数量分别 1/2，伺服数量为 3。
DLL ABI 字段、既有 Wire 解码结果与独立预先设定的原始值/小数位/单位/名称逐项一致，
guard、返回 record 外尾部、未选择半区检查通过。Native 使用实际加载的
`FWLIB64.dll` / `FWLIBE64.DLL`，模块 SHA-256 与上节冻结文件一致。

可复核产物：

- `target/focas-load-research/synthetic-20261010-001/002/`：两轮本机实验，包含完整服务端帧记录、
  Native 原始 ABI 缓冲、Wire 帧、模块身份、测试日志与独立预期。002 使用新增回归后的测试二进制。
- `tests/fixtures/wire/load_synthetic_handshake/`：原始握手/系统信息帧及来源捕获哈希。
- `tests/fixtures/wire/load_synthetic_nonzero/`：001 的派生响应、请求、Native 对照后的预期字段与哈希。
- `synthetic_nonzero_replays_against_native_and_expected`：默认 CI 回放八种派生响应，
  不加载 DLL、不连接设备；实际 DLL 对照需显式执行下述工具。

```powershell
python -B tools/ci/focas_load_synthetic.py --test-exe $testExe `
  --out target/focas-load-research/synthetic-new
```

`$testExe` 的构建与定位方式见上节。输出目录必须全新；Windows 本机运行不需要 Npcap 或 Scapy。
可选 `--archive <全新目录>` 仅在全部字段与预期一致后归档；不得覆盖原始捕获。

本轮补上的是通用负载路径的非零与双主轴 **合成报文证据**，没有新增真机证据。
不覆盖传统读取分支、状态 4 回退、整数极限、未知固件或完整 FOCAS 协议，
也不证明 NCGuide 内部会产生真实负载。Load HOLD 与生产发布门禁保持不变。

验证结果：FOCAS lib 192 项通过、12 项默认忽略；两个本机合成场景各四种操作，
用实际 DLL 对照执行两轮全部通过；FOCAS all-targets Clippy、格式检查与帧差分 15 项通过。
停止服务后 workspace 构建通过。合同入口先发现已存在的 `point_live.rs` 未登记，
随后在 Windows 默认 GBK 解码 Rust UTF-8 日志时失败；分别补齐 `scripts/contract_suites.py`
的名单与 `write-contract-evidence.py` 的显式 UTF-8 读取后，正式入口全部通过。
`target/validation/contract.json` 记录 28 suites、186 tests、0 failed、`dirty=true`；
证据对应当前未提交工作区，不冒充干净提交的发布验收。测试结束后再次确认业务服务未运行。

## 2026-10-10 整数极限补充

本机 `load-edges-001/integer_edges` 首次实验保留失败现场：实际DLL在servo raw=INT32_MIN时
写出INT32_MIN位型，旧Wire的checked_abs却拒绝。研究转换改为32位wrapping_abs后，
`load-edges-002` 三组场景/12种操作全部与实际DLL、独立预期一致；最大值、最小值和零均覆盖。
原始ABI、前后guard、返回记录后的尾部及未选择半区都检查通过。
归档 `tests/fixtures/wire/load_synthetic_edges/`，默认回归同时检查整数极限与普通非零样本。

该结果只证明新版通用读取分支的原始字段与ABI转换；负载生产门禁不变。
传统分支与状态4回退、外域小数位、工程量有效性、其他固件和真实负载仍待验收。


## 2026-10-10 正式百分比路径：新旧分支差分

当前实现与证据 supersede 上文负载HOLD结论；历史原始档案不改写。
正式入口 `tools/ci/focas_load_production.py`，全软件验收入口
`tools/ci/focas_equivalence_gate.py`，本次完整结果目录
`target/validation/focas-equivalence-20261010-003/`；是否通过以机器结果为准。

分支依据实际加载 FWLIBE64 的反编译与本机抓帧：0x0F654/0x93DA0确认
OPEN spec<=2走旧负载、>=3走新负载；0x915AC/0x91AC4确认能力字段；
0x5B048确认旧主轴首槽状态4写入会话标志并转0x56A34七槽路径；
0x57C1C确认旧伺服全局/路径轴数；0x56A34汇编确认32位abs/imul/有符号截断除法。
反编译工作目录 `target/focas-load-research/ida-connection/`、`ida-open-legacy/`
用于本机复核；可复跑的输入帧、DLL ABI、模块身份、守护检查和输出另行正式归档。

- `load_production_dll/`：第一版11组工程输出档案，保留为历史。
- `load_production_dll_v2/`：当前11组/264行默认CI证据，追加309/317指数边界。
- `load_dll_evidence_replays_production_batches`：完整生产适配器批次/请求序列回放。
- `load_remote_errors_and_malformed_session_lifecycle`：各槽Remote及坏帧连接生命周期。

v2不同于上文研究阶段的raw ABI比较：正式 `WireFocasApi::read_batch` 返回
F64百分比，按独立DLL整数/小数位构造的工程预期核对；4个实例、重复点、
重复批次、非法索引和缺失记录均进入实际适配器。源/测试二进制/DLL身份由全验收入口冻结。
11场景为 zero、fractional、four、decimal10、decimal_extremes、shrinking、empty、
legacy_direct、legacy_scaled、legacy_global_axes、legacy_wrapping。
新分支A4计数与名称不闭合应报坏帧；历史误请求spindle_load_165保留为拒绝回归。

合成档案不属于硬件或物理负载证据。当前服务保持停止，工作区未提交、未推送；
生产设备与发布许可验收独立保留。
