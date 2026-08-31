# ForgeLink 架构设计（V1 MVP 实现）

> 本文档为 `README` 的详细展开，同步自 `ForgeLink_Driver_MVP_实施方案.md` V1.4 与 `docs/flowchart.md`。`README` 仅保留快速开始，本文件承载完整设计、数据流、驱动实现、前端、性能与部署的段落式说明，避免关键字堆砌。

## 1. 总体架构

ForgeLink 采用单进程 Core 与多独立进程 Driver 的边缘网关形态。Core 负责配置的持久化、设备的管理、点位的分配、数据的汇聚以及北向 REST API 的提供；Driver 则专注于协议的解析与数据的采集。两者通过本机回环的可靠 IPC 通道解耦，实现了故障隔离与独立升级的能力，满足 V1 版本严格只读、单机稳定运行的核心目标。

在部署形态上，Core 以 `forgelinkd` 单二进制运行，内置 `ConfigStore`（SQLite）、`DriverManager`、`DeviceManager`、`PointRegistry`（支持 tombstone 复用）以及 `DataIngress` 与 `LatestValueCache`；而每一个 Driver（S7、FOCAS2、OPC UA、Simulator）则作为独立的 OS 进程存在，通过 `127.0.0.1` 的 TCP 通道以长度前缀的 Protobuf 协议与 Core 通信。这种形态既保证了驱动崩溃不会影响 Core，也允许驱动独立编译与发现。

设计上有三个硬约束需要特别说明。其一是进程隔离与孤儿防护：每个 Driver 在启动时由 Core 通过 `stdin` 注入一次性 `token`，Core 持有 `ChildStdin` 不关闭作为 liveness 管道，一旦 `close` 即 `EOF` 触发 Driver 自杀；Windows 侧通过 `Job Object` 的 `KILL_ON_JOB_CLOSE`、Linux 侧通过 `PR_SET_PDEATHSIG` 保证 Core 异常退出时无孤儿，详见 `crates/driver-manager/src/process.rs:54`。其二是 Core 不懂协议：无论是 S7 的 `DB10.DBD0`，还是 FOCAS 的 `status`，亦或是 OPC UA 的 `ns=2;i=1`，其文本解析仅存在于对应 Driver 进程内的 `address.rs`，Core 仅透传 `binding.config` 的 JSON 并做 Schema 校验，满足 `AGENTS.md:39`。其三是有界背压：所有 Data Plane 队列上界为 256，满时对同一 `point_id` 执行最新值覆盖的合并策略，控制面消息则走独立队列不丢弃，详见 `crates/driver-sdk/src/lib.rs:195`。

## 2. 数据流与时序

### 2.1 配置闭环（全量快照）

ForgeLink 的配置遵循全量快照的语义。用户通过 REST 接口提交设备、端点与任务的完整快照，Core 将其写入 SQLite 并为每一个逻辑点位分配稳定的 `point_id`。该 ID 通过 `tombstone` 机制保证删除后再次添加同一 `point_key` 时复用原 ID，避免数据身份的漂移。随后驱动侧会走严格串行的闭环：`DriverManager` 拉起子进程并传入 `token`，双方完成 `Hello` 与 `Welcome` 的握手与协议协商，再依次执行 `OpenConnection`、`ConfigureTasks`（在此阶段完成 `parse_address` 的全量校验与 `point_key` 唯一性检查）、`PointDescriptors` 上报、`ApplyPointMap` 以及最终的 `StartConnection` 并生成新的 `stream_epoch` 进入 `RUNNING` 状态。

需要强调的是，运行中对点位的任何增删改都必须走 `Stop → Configure → Apply → Start` 的完整路径并产生新的 `stream_epoch`，禁止热 Apply。若新 `revision` 的构建在任何一步失败，系统将保持 `STOPPED` 或 `FAILED` 状态并保留旧库，避免半配置状态下的运行风险。这一语义在 `§6.2` 中有明确约束。

### 2.2 运行时采集

进入运行态后，采集行为按任务类型分化。`Poll` 模式通过 `tokio::time::interval` 以 `MissedTickBehavior::Skip` 的策略周期性触发 `read_batch`，FOCAS 默认 500 毫秒、S7 为 100 毫秒、OPC UA 的 Poll 为 500 毫秒，而 Simulator 在压测时可低至 10 毫秒。`Subscribe` 模式专属于 OPC UA，它通过客户端创建订阅与受监控项，利用 `DataChangeCallback` 将数据变更经由 `mpsc 256` 通道以 `drain 64` 的批量方式合并为 `DataBatch`，而服务端 7 次空的 `KeepAlive` 通知则自然无回调，不递增 `sequence` 也不产生空批次，这正是 `§7.3` 所要求的静默语义。`Browse` 模式则周期性调用 `Session::browse` 对 `Objects 85` 这类根节点进行引用展开，并将结果以分号拼接为字符串点位。

无论哪种模式，最终都会通过 `DataSink.publish` 将 `DataBatch`（包含 `handle`、`epoch`、`seq`、`timestamp_ns` 以及点位数组）经由 `Session` 的写入端发送至 Core，再由 `DataIngress` 落入 `LatestValueCache`，最终通过 `GET /points/latest` 对外提供。时间语义上，业务时间统一为 `UTC Unix 纳秒`，而性能测量则使用宿主机的单调时钟，禁止跨进程直接相减 `§10`。对于 OPC UA，`SourceTimestamp` 会以 `1601` 年为起点的 `ticks` 精确换算为 `Unix` 纳秒予以保留，而 FOCAS 与 S7 则以当前时间近似处理。质量方面则按 `Good/Uncertain/Bad` 的三态模型，依据 `StatusCode` 的位标识进行单点隔离，不会因单点错误而丢弃整批数据。

## 3. 驱动详解

### 3.1 S7（西门子 PLC，地址型）

S7 驱动的核心在于地址解析与 PDU 编解码。地址解析覆盖了 `DB/M/I/Q/C/T` 的全量族及其别名，例如 `DB10.DBX24.0` 与简写 `DB10.0`、`M0.0` 与 `MB10`、`I0.0`、`Q0.0`、`C0/T0`（其位地址不乘以 8，且传输类型为 `0x1C/0x1D`，长度为 1）、`V` 映射为 `DB1`、`SM` 映射为 `Merker`、`AIW0` 映射为 `PIW0 0x80`、`L0.0` 映射为 `0x86` 等，共计 13 种区域类型，对应 `Area` 编码 `0x84/0x83/0x81/0x82/0x1C/0x1D/0x80/0x86`，为 S7Comm 协议的固定值。解析逻辑位于 `drivers/s7/src/address.rs:99`，并通过 `bit_address = byte*8+bit` 的规则统一处理。

编解码层面，`codec.rs` 支持 13 种 `S7Kind`，涵盖布尔、字节、字、双字、整数、双整数、浮点、长浮点、字符串等，且均为大端解码。`client.rs` 则按 `S7 ANY` 的三字节位偏移构建读取请求，能够自动合并连续区域，并通过 `read_vars` 返回按项隔离的结果，`C/T` 类型长度固定为 1 且对 `ret != 0xFF` 的项进行单点 `BAD` 跳过。诊断功能通过 `SZL` 的 `0x07 UserData 0xFF09` 透传实现，能够读取 `0x0131/0x0011` 等系统状态。真机验证在 `192.168.15.97:102` 上通过 `COTP → S7 Setup PDU480` 完成了 `DB10.DBD0 11468800` 的读取，并在 `aarch64 QEMU 7.8M` 环境下验证了批量合并与分片的稳定性。

### 3.2 FOCAS2（发那科 CNC，函数型，44/44）

FOCAS2 驱动是典型的函数型驱动，其地址族在 `drivers/focas2/src/address.rs:1` 中以 44 项清单的形式冻结，涵盖 `status`、轴的 `abs/machine/relative/distance/data/srvdelay/accdecdly` 七种、`feed`、主轴的 `speed/load/gear/maxrpm` 四种、`servo`、`macro`、`pmc` 的 `R/D/G/X/Y/F` 十四种、`diagnosis`、`tool` 的 `number/offset/zofs/length` 四种、`param`、`opmsg` 以及程序的 `number/main/name/dir/info/upload` 六种。

在 FFI 层面，`native.rs:340` 以 `Pack4` 对齐定义了 `OdbTofs 8B`、`IodbZofs 36B`、`IodbPsd1 8B`、`RealPrm 8B`、`IodbTo111 28B`、`IodbTo112 46B` 等 16 个函数指针结构，并通过 `libloading` 动态加载 `FWLIB64.dll 24M`。加载时会预先将 `drivers/focas2/libs/win` 加入 `PATH` 并尝试绝对路径与临时目录的双候选，缺库时返回 `EW_NODLL` 而非直接 panic，保证了单文件分发的可行性。

逻辑上，`cnc_rdtofs` 以 `s_no=e_no=num` 的形式试读 `type 0` 再试 `1`，`cnc_rdtofsr` 先以 `IodbTo112 46B` 的 `1_2` 类型进行 9 次试探再回落至 `IodbTo111 28B`，`cnc_rdparam` 则先以 `IodbPsd1 8B` 的 `ldata` 试读，若返回 `EW_Attrib 4` 说明该参数为 `REAL` 类型，则回退至 `IodbPsd2 12B` 的 `rdata` 并按 `prm_val/dec_val` 换算。程序目录与信息则通过 `OdbNc1 6次` 与 `OdbNc2 3次` 的试探以及 `cnc_upstart3→upload3→upend3` 的循环直至 `EW_BUFFER` 来完成。所有 `EW_LENGTH/NUMBER/ATTRIB` 错误均会按项转为 `String ERR:` 并以 `Quality Bad` 隔离，而 `EW_SOCKET/NODLL` 则退避为 `RECONNECTING`。在 `165:8193` 上的 35 点全量验证显示 23 个 `GOOD` 与 12 个 `BAD` 的隔离符合预期。

### 3.3 OPC UA（通用，节点/订阅型）

OPC UA 驱动实现了三种绑定：`opcua.node-group` 的 Poll 轮询、`opcua.subscription` 的订阅以及 `opcua.browse` 的浏览。在 Poll 模式下，驱动以固定的 `interval_ms` 轮询 `Session::read` 并批量读取；在订阅模式下，则通过 `create_subscription` 的 `publishing 500ms` 与 `create_monitored_items` 的 `sampling 250ms queue10` 建立 `DataChangeCallback`，再经由 `mpsc 256` 通道以 `drain 64` 的方式合批，同样遵循 `Latest-Wins` 的背压策略；而浏览模式则周期性地对 `Objects 85` 这类根节点进行引用展开，并将结果以分号拼接。

地址解析支持 `NodeId` 的四种形式 `ns/i/s/g/b`，例如 `ns=2;i=1234` 或 `ns=2;s=Motor.Speed`，且 Core 侧被禁止解析以满足硬约束。值映射覆盖了 `BOOL/I32/U32/I64/U64/F32/F64/STRING/Bytes/DateTime/TypedArray` 等全量 `Variant`，并依据 `StatusCode` 的 `Good→GOOD Uncertain→UNCERTAIN Bad→BAD` 进行单点隔离。`SourceTimestamp` 以 `1601` 年为起点的 `ticks` 精确换算为 `Unix` 纳秒予以保留，这是 `§9.2` 所要求的语义。

安全与证书方面，`pki_dir` 的解析优先级为 `connection_json` 的 `pki_dir` 字段高于环境变量 `FORGELINK_OPCUA_PKI_DIR` 再高于默认的 `data/certificates/opcua`。`own` 私钥以 `0o600` 权限通过 `rcgen` 自签名生成，`ClientBuilder` 以 `pki_dir own.der/key trust false verify true` 的方式构建，支持 `SecurityPolicy None/Basic256Sha256/Aes128` 与 `MessageSecurityMode None/Sign/SignAndEncrypt` 的透传，`rejected` 目录下的未知证书需经 `POST /rejected/{thumb}/trust` 人工迁移至 `trusted` 后重连，符合 `§8` 的最小运维能力。`Reference Server` 通过 `ghcr.io/php-opcua/uanetstandard-test-suite` 提供了 300 节点的全功能模拟，浏览 `Objects 85` 可得到 `TestServer; Server 2253` 等引用。

## 4. 前端

`web-ui/` 是一个独立的 Vite 8.2.2 前端项目，采用 `proxy /api→127.0.0.1:8132` 的方式避免跨域，且完全不修改后端。项目通过 `Tabs` 的形式组织了六大功能区：`drivers` 通过 `GET /drivers` 与 `POST /drivers/rescan` 展示驱动清单；`devices/endpoints` 通过 `POST /devices POST /endpoints GET /endpoints` 实现设备的增删与 `start/stop/delete` 控制；`points` 通过 `GET /points/latest auto 1s` 以表格形式展示 `endpoint/key/point_id/quality/type/value/time`；`diagnostics` 合并了 `GET /diagnostics` 与证书诊断；`certs` 则对四个 `store own/trusted/issuers/rejected` 提供了 `GET /certificates/opcua/{store}` `POST /trusted {pem}` `DELETE /trusted/{thumb}` 等操作。

点位的变更严格遵循 `Stop → PUT /tasks/{id} 全量快照 → Start` 的路径并产生新的 `stream_epoch`，单点 `Bad` 通过 `String ERR:` 的形式隔离而不影响同批其他点。前端构建通过 `npm run dev 219ms` 启动开发服，`npm run build` 产出 `10.29kB` 的 `dist` 产物，可随 `packaging` 一同部署。

## 5. 性能与平台

性能预算遵循 `§22` 的 `50K updates/s 持续 60分钟`、`IPC delivery p95 ≤20ms p99 ≤50ms`（基于单调时钟测量）、`RSS 60分钟增幅 ≤10%` 以及 `Conn-1000 60分钟无泄漏` 的要求。本机在 `release 8135` 上以 `sim-001 5点 100ms` 的低负载进行了 `15分钟 1.1% 9.5→9.6` 与 `60分钟 5.4% 9.5→9.7 5点 3600s` 的预检，并通过 `Simulator burst125` 验证了 `Latest-Wins` 的有效性；`1ms Poll 1pt` 的实测 `p50 6ms p95 13ms` 表明 `IPC` 本身远低于 `20ms` 门限。此外，`FORGELINK_HEARTBEAT_FAST=1` 可将心跳从 `5s/3s×3=15s` 缩短至 `1s×2=2s`，`common/mod.rs` 对 `10055` 的 `10次 50ms` 退避也已验证对 `TIME_WAIT 30s` 的缓解。

平台方面，`win64 PE 2.0/4.4/13M` 携带 `FWLIB64.dll`，`linux x64 ELF 5m56s` 与 `aarch64 7.8/13M QEMU 3m02s` 均以 `edition 2024 rust 1.85` 构建，`docker opcua-ref 4840 Up 55min` 通过 `10808` 代理拉取 `ghcr.io` 镜像。部署则通过 `packaging/windows-service/install.ps1` 的 `sc create ForgeLink` `KILL_ON_JOB_CLOSE` 与 `packaging/systemd/forgelink.service` 的 `journald` `systemctl enable --now` 实现，默认启动命令为 `forgelinkd --db forgelink.db --http-port 8132 --drivers-dir drivers`。

## 6. 版本与门禁

当前版本为 `v0.1.0-mvp` `7121998`，已实现 `FOCAS 44/44` 与 `OPC UA Browse` 等增量，并对前期误记的 `60` 进行了更正。通过 `cargo build --workspace 0.37s` 与 `cargo test --workspace --lib 11/13` 以及 `contract 9/5/3/2 data_plane 2 passed` 的验证，`§26` 的 23 条验收标准中 20 条已 `Done`，剩余 `OPC UA 硬件 97:4840 + 8C 60分钟正式 Soak + packaging` 即可完全闭环。详细门禁见 `ForgeLink_Driver_MVP_实施方案.md V1.4` 与三份 `GATE` 文档。
