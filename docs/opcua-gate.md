# OPC UA Gate 闭环文档（§19.3 §7.3）

> 依据 `ForgeLink_Driver_MVP_实施方案.md §19.3 §7.3`，OPC UA 生产发布前必须闭环证书信任、SecurityPolicy、Poll/Subscribe 双路径与真机兼容矩阵。V1 严格只读，不引入写路径。

## 1. 访问范式

| 绑定 | 任务模式 | 配置示例 | 语义 |
|---|---|---|---|
| `opcua.node-group` | `poll` | `{"nodes":[{"key":"counter","node_id":"ns=2;i=1","data_type":"I64"}]}` | `interval_ms` 轮询 `Session::read` 批量读，恒定速率，适用于低频/全量 |
| `opcua.subscription` | `subscribe` | `{"publishing_interval_ms":500,"sampling_interval_ms":250,"queue_size":10,"discard_oldest":true,"nodes":[...]}` | `publishing_interval` 服务端 Publish，`DataChangeCallback → mpsc 256 → Latest-Wins`，仅值变更时产 `DataBatch`，`KeepAlive` 不产批不递增 `sequence` |

`NodeId` 解析见 `drivers/opcua/src/address.rs`：`ns=2;i=1234 ns=2;s=Motor.Speed ns=2;g=GUID ns=2;b=Base64` 省略 `ns` 默认 0。前者 `Core` 禁止解析（硬约束）。

值映射 `Variant→Value` `§9.2`：`BOOL/I32/U32/I64/U64/F32/F64/STRING/Bytes`；`StatusCode Good→GOOD Bad→BAD` `quality_code = StatusCode.bits()` 单点 `Bad` 隔离不丢整批。

## 2. 证书与安全（§19.3）

- **目录** `data/certificates/opcua/{own,trusted,issuers,rejected,private}` 与 `async-opcua pki_dir` 兼容：
  - `own/own.der + own.pem + own.key` 双写 `own/cert.der + private/private.pem` 供 `ClientBuilder::certificate_path/private_key_path` 复用
  - `own.key 0o600`
- **pki_dir 解析** `connection_json.pki_dir > env FORGELINK_OPCUA_PKI_DIR > data/certificates/opcua`，`forgelinkd` 启动时若 env 未设则注入默认（子进程继承）
- **SecurityPolicy** `None/Basic128Rsa15/Basic256/Basic256Sha256/Aes128_Sha256_RsaOaep/Aes256_Sha256_RsaPss`，`MessageSecurityMode None/Sign/SignAndEncrypt` 均透传校验，非法直接 `BAD_CONFIG`
- **禁止默认忽略校验**：`ClientBuilder::trust_server_certs(false) verify_server_certs(true) create_sample_keypair(false)`。`None` 安全策略下自签 `python` 仍可 `connect OK`；`Sign/SignAndEncrypt` 未知证书首次落 `rejected/`，需 `POST /api/v1/certificates/opcua/rejected/{thumb}/trust` 人工迁移至 `trusted/` 后重连

REST：`GET /certificates/opcua/{own,trusted,issuers,rejected} /diagnostics` `POST /trusted {pem}` `DELETE /trusted/{thumb}` `POST /rejected/{thumb}/trust` `GET /diagnostics` 含 `certificates`。

## 3. 兼容矩阵

| Server | 地址 | Security | 路径 | 结果 | 备注 |
|---|---|---|---|---|---|
| **python asyncua** `0.19` | `opc.tcp://127.0.0.1:4840/freeopcua/server/` `ns=2` | `None` | `Poll Native BulkRead` + `Subscribe DataChangeCallback` | ✅ `8190 Poll 1193→1207 RUNNING` `Sub 1193→1207 KeepAlive不产批` `c2e6a7a` | 本地 `C:\Users\34268\AppData\Local\Temp\opencode\opcua_test_server2.py` `Counter ns=2;i=1 Sine ns=2;s=Sine Numeric1001 ns=2;i=1001`；`Fake` 同步 `0.05s` |
| **ProSys SimulationServer** | `opc.tcp://uademo.prosysopc.com:53530/OPCUA/SimulationServer` | `None` | `Native connect` | `Tcp True` 但 `5s` 握手超时（需 `15s+证书`），`test_native_opcua --timeout 5000` 等待超时 | `rejected/` 预期，需 `15s` 与 `Sign` 证书后补 |
| **open62541 1.4** | `opc.tcp://127.0.0.1:4840` Docker `open62541/open62541:1.4` | `None` | 预留 | 未验 | 可用 `docker run -p 4840:4840` 替代 python |
| **硬件 S7-1500/TwinCAT/iQ-R** | — | — | — | 未启 | V1 无硬件，仅 Simulator 预检；真机后补 `§7.3` 采样/队列/丢弃语义 |

最大采样/并发/RSS 归 `§22` Soak，V1 `Simulator 50K` 预检后补真机免责。

## 4. 真回调与背压

- `Poll`：`interval tick Skip → read_batch → coerce → DataBatch sequence++ → DataSink Latest-Wins (256+1024) → IPC`
- `Subscribe`：`create_subscription(publishing 500ms/30/10) → create_monitored_items(sampling 250 queue10) → DataChangeCallback try_send(mpsc 256) → 批量 drain 64 + Bad隔离 → DataBatch`；`KeepAlive` 空 `NotificationMessage` 自然无回调，不产批不递增 `seq`
- `source_timestamp_ns` 取 `DataValue.source_timestamp` 否则 `now_unix_ns()`，业务时间 UTC ns，性能用单调时钟禁 UTC 相减 `§9-11`
- 配置变更全量快照 `Stop→Configure→Apply→Start(new stream_epoch)` `point_id` 稳定 `tombstone`

## 5. 发布 Gate

- [x] `Poll/Subscribe` 双路径与 `NodeId` 解析
- [x] `证书 pki_dir` 与 `trust false` 闭环
- [x] `python 127.0.0.1:4840` 真回调联通
- [ ] `ProSys 15s+Sign` 与硬件矩阵（后续补）
- [ ] `§22 50K` Soak（后置）

> 本文档随 `c2e6a7a/7fd55ae` 入仓，满足 `§19.3` 证书不自动信任要求。
