# ForgeLink 架构设计（V1 MVP 实现）

> 本文档同步自 `ForgeLink_Driver_MVP_实施方案.md` V1.4 与 `docs/flowchart.md`，为 `README` 详细展开。`README` 仅保留快速开始，详细设计在此。

## 总体

`Core (forgelinkd) SQLite + DriverManager + DeviceManager + PointRegistry + DataIngress` ↔ `127.0.0.1 TCP Length-prefixed Protobuf` ↔ `Driver Process (s7/focas2/opcua/simulator) Tokio + Blocking`。详见 `docs/flowchart.md` Mermaid。

## 驱动

- **S7** `s7.address-group` `DB/M/I/Q/V/SM/AI/AQ/L/AC/HC/S` 13 Kind `Area 0x84/0x83/0x81/0x82/0x1C/0x1D/0x80/0x86` `COTP→S7 Setup PDU480` `C/T bit不×8 transport 0x1C/0x1D len1 per-BAD` `SZL 0x0131` `97:102 DB10 11468800` `WSTRING516` `LREAL`
- **FOCAS2** `focas.data-block 44/44` `status/axis 7 feed spindle 4 servo macro pmc 8 tool 4 param REAL opmsg prog 6` `Fake/Native FWLIB64 24M libloading` `IodbTo111 28B IodbTo112 46B OdbNc1 12B OdbNc2 31B OdbUp3 256B Pack4` `cnc_statinfo/rddynamic2 44/acts/rdparam REAL tofsr 1_2→1_1 rdzofs upstart3` `165:8193 35点 23 GOOD 12 BAD 8134`
- **OPC UA** `opcua.node-group Poll / opcua.subscription Sub 500/30 queue10 / opcua.browse 1000` `NodeId ns/i/s/g/b 4型` `Variant→TypedArray DateTime 1601 ticks→Unix ns Quality GOOD/UNCERTAIN/BAD StatusCode.bits()` `SourceTimestamp 保留` `ClientBuilder pki_dir own.der/key trust false verify true` `SecurityPolicy None/Basic256Sha256/Aes128 4840/4843` `Reference Server ghcr.io 300节点 Browse Objects 85`

## 证书

`data/certificates/opcua/{own,trusted,issuers,rejected,private}` `own 0o600 rcgen` `pki_dir = connection_json > FORGELINK_OPCUA_PKI_DIR > 默认` `GET /certificates/opcua/* /diagnostics` `POST /trusted {pem} DELETE /rejected/{thumb}/trust` `§8`

## 前端

`web-ui/` `Vite 8.2.2 React-TS 5173 proxy /api→8132` `Tabs drivers/devices/endpoints/points/diagnostics/certs` `Stop→PUT tasks→Start stream_epoch Bad隔离` `npm run dev 219ms build 10.29kB` 不改后端 `dist 3.5kB`

## 性能

`50K updates/s 60min` `IPC p95 13ms 1ms Poll p50 6ms Soak 15min 1.1% 60min 9.5→9.7 5.4% 5点 3600s` `p99 ≤50ms` `RSS ≤10%` `Conn-1000` `Simulator burst125` `release 8135` `FORGELINK_HEARTBEAT_FAST=1 1s×2` `common/mod.rs 10055 10次退避`

## 平台

`win64 PE 2.0/4.4/13M FWLIB64.dll` `linux x64 ELF 5m56s` `aarch64 7.8/13M QEMU 3m` `edition 2024 rust 1.85` `docker opcua-ref 4840 Up`

## 部署

`packaging/windows-service/install.ps1 sc create ForgeLink KILL_ON_JOB_CLOSE` `packaging/systemd/forgelink.service journald systemctl enable --now` `forgelinkd --db forgelink.db --http-port 8132 --drivers-dir drivers` `web-ui/dist`

## 版本

`v0.1.0-mvp` `7121998` `FOCAS 44/44 OPC UA Browse Security Soak 60min` `60 更正非CNC` `cargo build --workspace 0.37s test --lib 11/13`
