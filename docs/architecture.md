# ForgeLink 架构设计（V1 MVP 实现）

> 本文档为 `README` 的详细展开，同步自 `ForgeLink_Driver_MVP_实施方案.md` V1.4 与 `docs/flowchart.md`。`README` 仅保留快速开始，本文件承载完整设计、数据流、驱动实现、前端、性能与部署的段落式说明，避免关键字堆砌。

## 1. 总体架构

ForgeLink 采用 **单进程 Core + 多独立进程 Driver** 的边缘网关形态。Core 以 Rust + Tokio + SQLite 为基座，负责配置持久化、设备与点位管理、数据汇聚与北向 REST；Driver 以独立 OS 进程承载协议栈，通过本机回环的可靠 IPC 与 Core 解耦，实现故障隔离与热升级。

```
                    ┌─────────────────────────────────┐
                    │          ForgeLink Core         │
                    │      forgelinkd (Tokio)         │
                    │  ┌──────────────────────────┐   │
                    │  │ ConfigStore (SQLite)     │   │
                    │  │ DriverManager            │   │
                    │  │ DeviceManager            │   │
                    │  │ PointRegistry tombstone  │   │
                    │  │ DataIngress Latest Cache │   │
                    │  │ REST API 127.0.0.1:8132   │   │
                    │  └──────────────────────────┘   │
                    └──────┬────────┬────────┬────────┘
                           │ TCP    │ TCP    │ TCP  Length-prefixed Protobuf
              ┌────────────┘        │        └────────────┐
              ▼                     ▼                      ▼
       ┌──────────┐          ┌──────────┐          ┌──────────┐
       │ S7 Driver│          │FOCAS2    │          │ OPC UA   │
       │ 独立进程 │          │ 独立进程 │          │ 独立进程 │
       │ 97:102   │          │165:8193  │          │ 4840 Ref │
       │ COTP/S7  │          │Fwlib     │          │ async-   │
       └──────────┘          └──────────┘          │ opcua    │
                                                   └──────────┘
```

**设计要点**

* **进程隔离与孤儿防护**：每个 Driver 以 `token` 经 `stdin` 注入启动，Core 持有 `ChildStdin` 不关闭作为 liveness 管道，`close` 即 `EOF` 触发 Driver 自杀；Windows 侧 `Job Object KILL_ON_JOB_CLOSE`、Linux 侧 `PR_SET_PDEATHSIG`，满足 `§14.5`。代码见 `crates/driver-manager/src/process.rs:54`。
* **Core 不懂协议**：S7 的 `DB10.DBD0`、FOCAS 的 `status`、OPC UA 的 `ns=2;i=1` 解析仅存在于对应 Driver 进程内的 `address.rs`，Core 仅透传 `binding.config` JSON 并做 Schema 校验，满足 `AGENTS.md:39` 硬约束。
* **有界背压**：所有 Data Plane 队列 `256` 上界，满时对同一 `point_id` 执行 `Latest-Wins Coalescing` 合批，`ConnectionState` 走独立控制队列不丢弃，`crates/driver-sdk/src/lib.rs:195`。

## 2. 数据流与时序

### 2.1 配置闭环（§6.2 全量快照）

用户通过 `POST /devices, /endpoints, /tasks` 提交全量快照，Core 写入 SQLite 并分配稳定 `point_id`（`tombstone` 保证删除后复用同一 ID），随后驱动侧走严格串行闭环：

```
REST 全量快照 revision++ → SQLite
  → DriverManager::spawn(--port + stdin token)
  → Hello(token) / Welcome(协议协商 Major/Minor)
  → OpenConnection(handle, endpoint_id, config_json)
  → ConfigureTasks(parse_address 全量校验 point_key 唯一)
  → PointDescriptors → Core 落库
  → ApplyPointMap(point_key→point_id)
  → StartConnection(new stream_epoch) → RUNNING
```

运行中增删点位必须 `Stop → Configure → Apply → Start` 产生新 `stream_epoch`，禁止热 Apply。新 `revision` 构建失败则保持 `STOPPED/FAILED` 并保留旧库，避免半配置运行。

### 2.2 运行时采集

* **Poll**：`tokio::time::interval(interval_ms)`  `MissedTickBehavior::Skip`  到期触发 `read_batch`。FOCAS 默认 `500ms`、S7 `100ms`、OPC UA Poll `500ms`、Simulator 可 `10ms`。所有 `Poll` 最终 `DataSink.publish(DataBatch{handle, epoch, seq, timestamp_ns, values})` 经 `Session writer` → Core。
* **Subscribe**：`OPC UA` 通过 `Client::create_subscription(publishing 500ms)` + `create_monitored_items(sampling 250ms queue10)` 建立 `DataChangeCallback → mpsc 256 → drain 64 批量`，`KeepAlive` 7 次空 `Notification` 自然无回调，不递增 `sequence`、不产 `DataBatch`，符合 `§7.3` 静默语义。
* **Browse**：`opcua.browse` 周期 `Session::browse(Objects 85, HierarchicalReferences)` 将引用 `;` 拼接为 `STRING` 点位，`§7.3 V1 支持 Browse`。

*时间与质量*：业务 `timestamp_ns` 为 `UTC Unix ns` `now_unix_ns()`，`source_timestamp_ns` 对 OPC UA `1601 ticks → Unix ns` 精确换算 `drivers/opcua/src/opcua_api.rs:286`，FOCAS/S7 用 `now` 近似。性能测量用宿主机单调时钟，禁止跨进程 UTC 相减 `§10`。质量按 `Good/Uncertain/Bad` `StatusCode.bits()` 单点 `Bad` 隔离不丢整批。

## 3. 驱动详解

### 3.1 S7（Siemens PLC，§7.1 地址型）

**地址解析** `drivers/s7/src/address.rs:99` 覆盖 `DB/M/I/Q/C/T` 全量及别名：`DB10.DBX24.0` `DB10.0` 简写、`M0.0/MB10`、`I0.0`、`Q0.0`、`C0/T0`（`bit_address` 不 `*8` `transport 0x1C/0x1D len1`）、`V→DB1`（S7-200）、`SM→M`、`AIW0→PIW0 0x80`/`AQW0→PQW0`、`L0.0 0x86`、`AC0-3→M*4`、`HC0-5→C`、`S0.0→M`。`Area` 编码 `0x84/0x83/0x81/0x82/0x1C/0x1D/0x80/0x86` 为 S7Comm 固定，`bit_address = byte*8+bit`（C/T 例外）。

**编解码与 PDU** `codec.rs` 支持 `13 种 S7Kind BOOL/BYTE/WORD/DWORD/INT/DINT/REAL/LREAL/STRING256/WSTRING512/S5Time/Time/Date/Dt` 大端解码。`client.rs` 按 `S7 ANY` 三字节位偏移 `build_read_req` 合并连续区 `read_vars → Vec<Option<Vec<u8>>>`，`C/T len1` 且 `ret!=0xFF` 按项 `BAD` 跳过，`paired` 排序不丢位。`SZL` 走 `0x07 UserData 0xFF09 7+12` 透传 `0x0131/0x0011`。

**真机** `192.168.15.97:102` `COTP→S7 Setup PDU480` `DB10.DBD0 11468800` `DBW0 175` `PUT/GET 0x04` 诊断，`ARM QEMU aarch64 7.8M` 验证批量合并与分片。

### 3.2 FOCAS2（FANUC CNC，§7.2 Function 型，44/44）

**地址族** `drivers/focas2/src/address.rs:1 44清单`：`status`、`axis abs/machine/relative/distance/data/srvdelay/accdecdly 7种`、`axis.feed`、`spindle speed/load/gear/maxrpm 4种`、`servo`、`macro 100`、`pmc R/D/G/X/Y/F 14种`、`diagnosis`、`tool number/offset/zofs/length 4种`、`param`、`opmsg`、`program number/main/name/dir/info/upload 6种`。

**结构与 FFI** `native.rs:340 Pack4`：`OdbTofs 8B` `IodbZofs 36B` `IodbPsd1 8B` `RealPrm 8B IodbPsd2 12B` `IodbTo111 28B` `IodbTo112 46B` `PrgDir 256` `OdbNc1 12B OdbNc2 31B` `OdbUp3 256B`。16 个 `Fn*` 经 `libloading` 动态加载 `FWLIB64.dll 24M` `NativeLib::load  prepend PATH + 绝对路径 + TEMP`，缺库返回 `EW_NODLL` 重试而非 panic。

**逻辑与容错**：`cnc_rdtofs s_no=e_no=num f64/1000` 试 `type 0→1`；`cnc_rdtofsr` 先 `IodbTo112 46B 1_2` `9 trial` 再回落 `IodbTo111 28B`；`cnc_rdparam` 先 `IodbPsd1 8B ldata` 试 `len8`，`EW_Attrib 4` 说明 `REAL` 则回退 `IodbPsd2 12B rdata prm_val/dec_val 10^dec`；`cnc_rdprogdir/info` `OdbNc1 6 trial` `OdbNc2 3 trial` `cnc_upstart3→upload3→upend3` 循环至 `EW_BUFFER`。所有 `EW_LENGTH/NUMBER/ATTRIB` 按项 `String ERR:` 转 `Quality Bad` 隔离，`EW_SOCKET/NODLL` 退避 `RECONNECTING`，`165:8193 35点 23 GOOD 12 BAD 8134` `pts44_final3.json` 验证 `Fake 10 passed`。

### 3.3 OPC UA（通用，§7.3 节点/订阅型）

**三绑定**：`opcua.node-group Poll` `opcua.subscription Sub publishing 500 sampling 30 queue10 discardOldest` `opcua.browse 1000` `drivers/opcua/src/lib.rs:35`。`Poll` 定速 `interval_ms` 轮询 `Session::read TimestampsToReturn::Both` 批量；`Sub` `create_subscription 500ms` + `create_monitored_items 250ms` `DataChangeCallback try_send mpsc 256` `drain 64` 合批 `Latest-Wins`；`Browse` `Session::browse(Objects 85, HierarchicalReferences)` `;` 拼接。

**地址与映射**：`NodeId ns/i/s/g/b 4型` `ns=2;i=1234 s=Motor.Speed g=72962B91... b=Base64` `Core禁解析` `address.rs:199`。`Variant→Value` 覆盖 `BOOL/I32/U32/I64/U64/F32/F64/STRING/Bytes/DateTime/TypedArray` `§9.2`，`StatusCode Good→GOOD Uncertain→UNCERTAIN Bad→BAD bits()` `quality_code` 单点隔离，`SourceTimestamp` 以 `1601 ticks 11644473600*10M → Unix ns` 精确 `opcua_api.rs:286`，`LocalizedText/Array` 保留。

**安全与证书**：`pki_dir = connection_json > FORGELINK_OPCUA_PKI_DIR > data/certificates/opcua` `own 0o600 rcgen 0o600` `ClientBuilder pki_dir own.der/key trust false verify true create_sample_keypair false` `SecurityPolicy None/Basic256Sha256/Aes128_Sha256_RsaOaep` `MessageSecurityMode None/Sign/SignAndEncrypt` `4840 None` `4843 SignAndEncrypt` `rejected→trusted 15s 人工` `§8` `Reference Server ghcr.io/php-opcua/uanetstandard-test-suite 300节点 12方法` `Browse Objects 85 → TestServer; Server 2253`。

## 4. 前端

`web-ui/` 为独立 Vite 8.2.2 前端项目，不修改后端。`5173` 开发服通过 `vite.config.ts proxy /api→127.0.0.1:8132` 避 `CORS`，`src/main.ts 662行` 实现 `Tabs drivers/devices/endpoints/points/diagnostics/certs`。`drivers` 走 `GET /drivers` + `POST /drivers/rescan`；`devices/endpoints` 走 `POST /devices POST /endpoints GET /endpoints` `start/stop/delete`；`points` 走 `GET /points/latest auto 1s` 表格 `endpoint/key/point_id/quality/type/value/time`；`diagnostics` 合并 `GET /diagnostics` 与 `GET /certificates/opcua/diagnostics`；`certs` 四 `store own/trusted/issuers/rejected` `GET /certificates/opcua/{store}` `POST /trusted {pem} DELETE /trusted/{thumb} POST /rejected/{thumb}/trust`。点位变更严格 `Stop → PUT /tasks/{id} 全量快照 → Start new epoch` `Bad隔离` `npm run dev 219ms build 10.29kB gzip 3.5kB` `dist` 可随 `packaging` 部署。

## 5. 性能与平台

**预算**：`§22 50K updates/s 60min` `IPC delivery p95 ≤20ms p99 ≤50ms` 单调时钟测量 `RSS 60min 增幅 ≤10%` `Conn-1000 60min无泄漏`。本机 `release 8135 sim-001 5点 100ms` `Soak 15min 1.1% 9.5→9.6 60min 5.4% 9.5→9.7 5点 3600s` `Simulator burst125` 已预检；`1ms Poll 1pt p50 6ms p95 13ms` `FOCAS 500ms 13ms` `FORGELINK_HEARTBEAT_FAST=1 1s×2` `common/mod.rs 10055 10次退避` `TIME_WAIT 30s` 已 `8134` 验证。

**平台**：`win64 PE 2.0/4.4/13M FWLIB64.dll 649KB` `linux x64 ELF 5m56s` `aarch64 7.8/13M QEMU 3m02s` `edition 2024 rust 1.85` `docker opcua-ref 4840 Up 55min` `ghcr.io 10808`。

**部署**：`packaging/windows-service/install.ps1 sc create ForgeLink binPath --db %ProgramData%\ForgeLink\forgelink.db --http-port 8132 start auto` `KILL_ON_JOB_CLOSE` 已 `process.rs:job`；`packaging/systemd/forgelink.service ExecStart /opt/forgelink/forgelinkd --db /var/lib/forgelink/forgelink.db journald` `systemctl enable --now`。`forgelinkd --db forgelink.db --http-port 8132 --drivers-dir drivers` 默认 `8132 loopback` `forgelinkd --help`。

## 6. 版本与门禁

`v0.1.0-mvp` `7121998 FOCAS 44/44 OPC UA Browse Security` `60 更正非CNC` `cargo build --workspace 0.37s cargo test --workspace --lib 11/13 contract 9/5/3/2 data_plane 2 passed --test-threads=1` `§26 23条 20/23 Done` 余 `OPC UA硬件 97:4840 + 8C 60min正式 Soak 60min + packaging` 即 `Done`。详见 `ForgeLink_Driver_MVP_实施方案.md V1.4` 与 `GATE` 三文档。
