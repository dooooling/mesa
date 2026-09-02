# Mesa（V2.1）

工业设备统一采集平台，Rust + Tokio + Protobuf IPC + SQLite，单进程 Core + 独立进程 Driver（S7 / FOCAS2 / OPC UA / Simulator），V2.1 在 V1 只读基线上扩展动态 UI / Secret / Browse / Control / 原子事务。

## 快速开始

```bash
cargo build --workspace          # 含全部驱动 bin，edition 2024 rust 1.95.0
cargo test --workspace -- --test-threads=1
./target/release/mesad --http-port 8132 --drivers-dir drivers  # 默认 8132 loopback
```

验收入口 `http://127.0.0.1:8132/api/v1/drivers` `.../endpoints` `.../points/latest` `.../diagnostics` `.../certificates/opcua/diagnostics`

## 架构要点

- **Core 不懂协议**：S7 DB / FOCAS Function / OPC UA NodeId 解析仅在对应 Driver 进程
- **有界背压**：Data Plane 队列 256 + Latest-Wins Coalescing，`Core 25%` 仍有界
- **点位稳定**：`point_id` 由 Core 分配持久化，`tombstone` 复用，`stream_epoch` 随 `Stop→Configure→Apply→Start` 递增
- **时间**：业务 `UTC Unix ns`，性能 `单调时钟` 禁跨进程相减
- **孤儿防护**：`stdin token` + `KILL_ON_JOB_CLOSE / PR_SET_PDEATHSIG`，`token` 经 `stdin` 注入
- **多连接**：单 Driver 进程多 `handle` 复用 `HashMap<u32,ConnEntry>` `DataSink for_connection` 隔离

实施方案：`docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md` V2.1（唯一事实来源）；`Mesa_Driver_MVP_实施方案.md` V1.4 为历史基线

## 前端

`apps/mesa-web/` `Vite 5173 proxy /api→8132` `pnpm --dir apps/mesa-web install && pnpm --dir apps/mesa-web build`

## 版本

`feat/v2.1-impl` `rust 1.95.0` `Node 22.18 pnpm 10.28`
