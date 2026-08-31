# ForgeLink 架构设计（V1 MVP 实现）

> 本文档为 `README` 的详细展开，同步自 `ForgeLink_Driver_MVP_实施方案.md` V1.4 与 `docs/flowchart.md` Mermaid 流程图。`README` 仅保留快速开始，本文件承载完整设计、数据流、驱动细节、性能与部署说明。

## 1. 总体架构

ForgeLink 采用 **单进程 Core + 多独立进程 Driver** 的经典边缘网关形态。Core 负责配置、调度、存储与北向 API，Driver 负责协议解析与采集，二者通过本机回环的可靠 IPC 解耦，满足 V1 严格只读、单机稳定运行的核心目标。

```
                    ┌─────────────────────────────┐
                    │        ForgeLink Core       │
                    │  forgelinkd (Rust/Tokio)    │
                    │  SQLite ConfigStore         │
                    │  DriverManager              │
                    │  PointRegistry (tombstone)  │
                    │  DataIngress + Latest Cache │
                    │  REST API 127.0.0.1:8132     │
                    └──────┬──────┬──────┬────────┘
                           │ TCP  │ TCP  │ TCP  Length-prefixed Protobuf
              ┌────────────┘      │      └────────────┐
              ▼                   ▼                    ▼
       ┌──────────┐       ┌──────────┐          ┌──────────┐
       │ S7 Driver│       │FOCAS2    │          │ OPC UA   │
       │ 独立进程 │       │独立进程  │          │独立进程  │
       │ Tokio    │       │Tokio+    │          │Tokio     │
       │ 97:102   │       │Blocking  │          │ 4840 Ref │
       └──────────┘       │165:8193  │          └──────────┘
                          └──────────┘
```

* **进程隔离**：每个 Driver 为独立 OS 进程，`token` 经 `stdin` 注入，`KILL_ON_JOB_CLOSE`（Windows）与 `PR_SET_PDEATHSIG`（Linux）保证 Core 崩溃时无孤儿，详见 `crates/driver-manager/src/process.rs:54`。
* **Core 不懂协议**：S7 的 `DB10.DBD0`、FOCAS 的 `status`、OPC UA 的 `ns=2;i=1` 均仅在对应 Driver 进程内解析，Core 仅透传 `binding.config` JSON，满足 `AGENTS.md:39` 硬约束。
* **有界背压**：所有 Data Plane 队列 `256 + Latest-Wins Coalescing`，Core 消费降至 25% 仍有界，`crates/driver-sdk/src/lib.rs:195`。

## 2. 数据流与时序

### 2.1 配置闭环（§6.2）

```
REST POST /devices, /endpoints, /tasks（全量快照 revision++）
  → Core SQLite 持久化
  → DriverManager spawn 子进程 --port + stdin token
  → Hello/Welcome 握手（§14.3 协议协商）
  → OpenConnection
  → ConfigureTasks(parse_address 全量校验 point_key 唯一)
  → PointDescriptors → Core 分配稳定 point_id（tombstone 复用）
  → ApplyPointMap
  → StartConnection(new stream_epoch)
  → RUNNING
```

运行中修改点位必须走 `Stop → Configure → Apply → Start` 产生新 `stream_epoch`，禁止热 Apply，`§6.2`。

### 2.2 运行采集

* **Poll**：`tokio::time::interval(interval_ms)` `MissedTick Skip` 周期触发 `read_batch`，`FOCAS 500ms` `S7 100ms` `OPC UA Poll 500ms`。
* **Subscribe**：`OPC UA` 通过 `DataChangeCallback → mpsc 256 → drain 64 批量` 合并为 `DataBatch`，`KeepAlive 7次空跳不产批不递增 sequence`，符合 `§7.3`。
* **Browse**：`opcua.browse` 周期 `Session::browse(Objects 85)` 引用展开 `;` 拼接，`§7.3 V1 支持 Browse`。

所有路径最终 `DataSink.publish(DataBatch{handle, epoch, seq, timestamp_ns, values:Vec<PointValue{point_id, Value, Quality, source_timestamp}}})` → `Session writer → Core DataIngress → LatestValueCache → GET /points/latest`。

*时间*：业务 `UTC Unix ns` `forgelink_core_types::now_unix_ns`，性能用宿主机单调时钟，禁止两进程 UTC 相减 `§10`。`SourceTimestamp` 对 OPC UA `1601 ticks → Unix ns` 精确保留 `drivers/opcua/src/opcua_api.rs:286`，FOCAS/ S7 用 `now` 近似。

## 3. 驱动详解

### 3.1 S7（Siemens PLC）

* **地址族**：`DB/M/I/Q/C/T` 全量 + 别名 `V→DB1` `SM→M` `AI/AQ→PI/PQ 0x80` `L 0x86` `AC0-3→M*4` `HC0-5→C` `S0.0→M`，`drivers/s7/src/address.rs:99` 13 Kind `Area 0x84/0x83/0x81/0x82/0x1C/0x1D/0x80/0x86`。
* **编解码**：`codec.rs 13种 BOOL/BYTE/WORD/DWORD/INT/DINT/REAL/LREAL/STRING256/WSTRING512/S5Time` 大端；`client.rs C/T bit_address不×8 transport 0x1C/0x1D len1 per-BAD隔离 read_vars` `19`；`SZL 0x0131` `read_szl 0x07 UserData 0xFF09`。
* **真机**：`192.168.15.97:102 DB10.DBD0 11468800 DBW0 175` `S7Comm COTP→S7 Setup PDU480` `PUT/GET 0x04` 诊断，`ARM QEMU aarch64 7.8M` 验证。

### 3.2 FOCAS2（FANUC CNC）

* **44/44 全量**：`status` `axis abs/machine/relative/distance/data/srvdelay/accdecdly 7种` `axis.feed` `spindle speed/load/gear/maxrpm 4种` `servo` `macro` `pmc R/D/G/X/Y/F 14种` `diagnosis` `tool number/offset/zofs/length 4种` `param` `opmsg` `program number/main/name/dir/info/upload 6种`，`drivers/focas2/src/address.rs:1 44清单`。
* **结构**：`OdbTofs 8B Pack4` `IodbZofs 36B` `IodbPsd1 8B` `RealPrm 8B IodbPsd2 12B` `IodbTo111 28B IodbTo112 46B` `PrgDir 256` `OdbNc1 12B OdbNc2 31B` `OdbUp3 256B` `drivers/focas2/src/native.rs:340 Pack4` `libloading FWLIB64 24M`。
* **逻辑**：`cnc_rdtofs s_no=e_no=num f64/1000` `cnc_rdtofsr 1_2→1_1 9 trial` `cnc_rdparam len8→Attri→len12 REAL prm_val/dec_val` `cnc_rdprogdir/info 6+3 trial` `cnc_upstart3→upload3→upend3` `EW_LENGTH/NUMBER/ATTRIB` 单点 `BAD` 隔离 `RUNNING`，`165:8193 35点 23 GOOD 12 BAD 8134` `pts44_final3.json`。

### 3.3 OPC UA（通用）

* **三绑定**：`opcua.node-group Poll` `opcua.subscription Sub publishing 500 sampling 30 queue10 discard_oldest` `opcua.browse 1000` `drivers/opcua/src/lib.rs:35`。
* **地址**：`NodeId ns/i/s/g/b 4型` `ns=2;i=1234 s=Motor.Speed g=GUID b=Base64` `Core禁解析` `address.rs:199`。
* **映射**：`Variant→Value BOOL/I32/U32/I64/U64/F32/F64/STRING/Bytes/DateTime/TypedArray` `StatusCode Good→GOOD Uncertain→UNCERTAIN Bad→BAD bits()` `SourceTimestamp 1601 ticks→Unix ns` `§9.2`。
* **安全**：`pki_dir = connection_json > FORGELINK_OPCUA_PKI_DIR > data/certificates/opcua` `own 0o600 rcgen` `ClientBuilder pki_dir own.der/key trust false verify true` `SecurityPolicy None/Basic256Sha256/Aes128 4840/4843` `rejected→trusted 15s` `§8` `Reference Server ghcr.io 300节点 4840 ghcr 10808` `Browse Objects 85`。

## 4. 前端

`web-ui/` 独立 Vite 8.2.2 项目，不修改后端。`5173 proxy /api→8132` 避 CORS，`Tabs drivers/devices/endpoints/points/diagnostics/certs`，`Stop→PUT tasks→Start stream_epoch` `Bad隔离` `npm run dev 219ms build 10.29kB gzip 3.5kB` `dist` 可 `packaging` 集成。

## 5. 性能与平台

* **预算**：`§22 50K updates/s 60min` `IPC p95 13ms 1ms Poll p50 6ms` `Soak 15min 1.1% 60min 9.5→9.7 5.4% 5点 3600s release 8135` `RSS ≤10%` `Conn-1000` `Simulator burst125` `FORGELINK_HEARTBEAT_FAST=1 1s×2` `common/mod.rs 10055 10次退避`。
* **平台**：`win64 PE 2.0/4.4/13M FWLIB64.dll` `linux x64 ELF 5m56s` `aarch64 7.8/13M QEMU 3m` `edition 2024 rust 1.85` `docker opcua-ref 4840 Up 55min`。
* **部署**：`packaging/windows-service/install.ps1 sc create ForgeLink KILL_ON_JOB_CLOSE` `packaging/systemd/forgelink.service journald systemctl enable --now` `forgelinkd --db forgelink.db --http-port 8132 --drivers-dir drivers`。

## 6. 版本与门禁

`v0.1.0-mvp` `7121998 FOCAS 44/44 OPC UA Browse` `60 更正非CNC` `cargo build --workspace 0.37s cargo test --lib 11/13 contract 9/5/3/2 data_plane 2 passed` `§26 23条 20/23 Done` 余 `OPC UA硬件 97:4840 + 8C 60min正式 Soak + packaging` 即 `Done`。
