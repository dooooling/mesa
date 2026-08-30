# ForgeLink Driver MVP

工业设备统一采集平台，Rust + Tokio + Protobuf IPC + SQLite，单进程 Core + 独立进程 Driver（S7 / FOCAS2 / OPC UA），V1 严格只读。

## 快速开始

```bash
cargo build --workspace          # 含全部驱动 bin，edition 2024 rust 1.85
cargo test --workspace -- --test-threads=1  # 42+11+4+23项 0 failed（单跑 subprocess 18s）
./target/debug/forgelinkd --db forgelink.db --http-port 8132 --drivers-dir drivers  # 默认 8132 loopback
```

验收入口 `http://127.0.0.1:8132/api/v1/drivers` `.../endpoints` `.../points/latest` `.../diagnostics` `.../certificates/opcua/diagnostics`

## 架构要点

- **Core 不懂协议**：S7 DB / FOCAS Function / OPC UA NodeId 解析仅在对应 Driver 进程
- **有界背压**：Data Plane 队列 256 + Latest-Wins Coalescing，`Core 25%` 仍有界
- **点位稳定**：`point_id` 由 Core 分配持久化，`tombstone` 复用，`stream_epoch` 随 `Stop→Configure→Apply→Start` 递增
- **时间**：业务 `UTC Unix ns`，性能 `单调时钟` 禁跨进程相减
- **孤儿防护**：`stdin token` + `KILL_ON_JOB_CLOSE / PR_SET_PDEATHSIG`，`token` 经 `stdin` 注入
- **多连接**：单 Driver 进程多 `handle` 复用 `HashMap<u32,ConnEntry>` `DataSink for_connection` 隔离

## 驱动

- **S7** `s7.address-group` `DB/M/I/Q` `COTP→S7 Setup PDU480` `READ 19` `PUT/GET 0x04` 诊断
- **FOCAS2** `focas.data-block` `status/axis/spindle/macro/pmc` `Fake/Native libloading FWLIB64 24M` `cnc_statinfo/rddynamic2 44`
- **OPC UA** `opcua.node-group Poll` `opcua.subscription Subscribe` `NodeId ns/i/s/g/b` `Variant→Value` `pki_dir 10808` `trust false`

## 证书

`data/certificates/opcua/{own,trusted,issuers,rejected,private}` `own 0o600` `pki_dir = connection_json > FORGELINK_OPCUA_PKI_DIR > 默认` `POST /rejected/{thumb}/trust`

## 性能

`50K updates/s 60min 600+3000 Soak 3600s` `IPC p95 ≤20ms p99 ≤50ms` `RSS ≤10%` `Conn-1000` `Simulator burst125` `release 8135 60min 9.5→9.7 5.4% 5点` `Soak 15min 1.1%` 已预检

## 部署

`packaging/windows-service/install.ps1` `sc create ForgeLink` `KILL_ON_JOB_CLOSE` `packaging/systemd/forgelink.service` `systemctl enable --now` `journald`

## 平台

`win64 PE 2.0/4.4/13M` `linux x64 ELF 5m56s` `aarch64 7.8/13M QEMU 3m` `edition 2024`

## 版本

`v0.1.0-mvp` `1e84b1f` 前 `df7955b` 注释整理，`tag` 本地
