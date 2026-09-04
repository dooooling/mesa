# OPC UA Gate 闭环文档（§19.3 §7.3）

> 依据 `mesa_Driver_MVP_实施方案.md §19.3 §7.3`，OPC UA 生产发布前必须闭环证书信任、SecurityPolicy、Poll/Subscribe 双路径与真机兼容矩阵。V1 严格只读，不引入写路径。

## 1. 访问范式

| 绑定 | 任务模式 | 配置示例 | 语义 |
|---|---|---|---|
| `opcua.node-group` | `poll` | `{"nodes":[{"key":"counter","node_id":"ns=2;i=1","data_type":"I64"}]}` | `interval_ms` 轮询 `Session::read` 批量读 |
| `opcua.subscription` | `subscribe` | `{"publishing_interval_ms":500,"sampling_interval_ms":250,"queue_size":10,"discard_oldest":true,"nodes":[...]}` | `DataChangeCallback → per-handle Latest-Wins slots → forwarder send().await` `KeepAlive` 不产批 |
| `opcua.browse` | `poll` | `{"nodes":[{"key":"objs","node_id":"ns=0;i=85","data_type":"STRING"}]}` | `interval_ms` 周期 `Session::browse` 引用展开 `;` 拼接 |

`NodeId` 解析见 `address.rs`：`ns=2;i/s/g/b 4型` `Core` 禁止解析。值映射 `Variant→Value` `§9.2`：`BOOL/I32/U32/I64/U64/F32/F64/STRING/Bytes/DateTime/DateTimeArray/TypedArray`；`StatusCode Good→GOOD Uncertain→UNCERTAIN Bad→BAD` `quality_code=bits()` 单点隔离；`SourceTimestamp 1601 ticks→Unix ns` 精确保留。

## 2. 证书与安全（§19.3）

- **目录** `data/certificates/opcua/{own,trusted,issuers,rejected,private}` 与 `async-opcua pki_dir` 兼容：
  - `own/own.der + own.pem + own.key` 双写 `own/cert.der + private/private.pem` 供 `ClientBuilder::certificate_path/private_key_path` 复用
  - `own.key 0o600`
- **pki_dir 注入（Stage 2 冻结）**：transport 不读取任何配置来源（不读 connection_json / 环境变量 / 默认目录）；PKI 目录由调用方经 `OpcUaConnectOptions.pki_dir` 注入。
- **SecurityPolicy** `None/Basic128Rsa15/Basic256/Basic256Sha256/Aes128_Sha256_RsaOaep/Aes256_Sha256_RsaPss`，`MessageSecurityMode None/Sign/SignAndEncrypt` 均透传校验，非法直接 `BAD_CONFIG`
- **禁止默认忽略校验**：`ClientBuilder::trust_server_certs(false) verify_server_certs(true) create_sample_keypair(false)`。`None` 安全策略下自签 `python` 仍可 `connect OK`；`Sign/SignAndEncrypt` 未知证书首次落 `rejected/`，需 `POST /api/v1/certificates/opcua/rejected/{thumb}/trust` 人工迁移至 `trusted/` 后重连

REST：`GET /certificates/opcua/{own,trusted,issuers,rejected} /diagnostics` `POST /trusted {pem}` `DELETE /trusted/{thumb}` `POST /rejected/{thumb}/trust` `GET /diagnostics` 含 `certificates`。`SecurityPolicy None/Basic256Sha256/Aes128_Sha256_RsaOaep透传至 ClientBuilder EndpointDescription 500ms→Sign/SignAndEncrypt 4842/4843 真测`

## 3. 兼容矩阵

| Server | 地址 | Security | 路径 | 结果 | 备注 |
|---|---|---|---|---|---|
| **Fake** | `opc.tcp://127.0.0.1:4840` | `None` | `Poll/Sub/Browse 8134 4点 Poll/Sub/Browse 300ms` | ✅ `8134 ep-opc 4点 objs i=85.Child1 cnt 948 speed 308.5 sub_cnt 209` `Fake 11 passed` `Browse Poll Sub KeepAlive7不产批` | `Mesad 8134 opcua_browse.db` |
| **UA-.NETStandard Reference** `ghcr.io/php-opcua/uanetstandard-test-suite` | `opc.tcp://127.0.0.1:4840/UA/TestServer` | `None` | `Poll/Sub/Browse/continuation` | ✅ `2026-09-04 实测通过：connect/read/ns/browse(4节点)/max_refs=1分页3跳/revised 250→300ms/逐项BAD/live CurrentTime/cleanup幂等` | `test_native_opcua.rs` |
| **python asyncua** `0.19` | `opc.tcp://127.0.0.1:4840/freeopcua/server/` `ns=2` | `None` | `Poll Native BulkRead` + `Subscribe` | ✅ `8190 Poll RUNNING Sub KeepAlive` `c2e6a7a` | 历史验证 |
| **ProSys/open62541/硬件** | — | — | — | 未启 | `ProSys Certified` `open62541 1.4 Docker` 可替 `Reference` |

最大采样/并发/RSS 归 `§22` Soak，V1 `Simulator 50K` 预检后补真机免责。

## 4. 真回调与背压

- `Poll`：`interval tick Skip → read_batch → coerce → DataBatch sequence++ → DataSink Latest-Wins (256+1024) → IPC`
- `Subscribe`：`create_subscription(publishing 500ms/30/10)` → `create_monitored_items(sampling 250 queue10)` → `DataChangeCallback` 只做 per-handle Latest-Wins slot 覆盖 → forwarder `drain + send().await` → adapter `send().await` → `DataBatch`；单项 BAD 合成初始 BAD 事件；`unsubscribe` 删项失败仍 best-effort 删订阅；`SubscriptionStats{received,coalesced}` 可观测合并量。
- `source_timestamp_ns` 取 `DataValue.source_timestamp` 否则 `now_unix_ns()`，业务时间 UTC ns，性能用单调时钟禁 UTC 相减 `§9-11`
- 配置变更全量快照 `Stop→Configure→Apply→Start(new stream_epoch)` `point_id` 稳定 `tombstone`

## 5. 发布 Gate

- [x] `Poll/Subscribe/Browse` 三路径与 `NodeId 4型` `Variant→TypedArray/DateTime`
- [x] `SecurityPolicy/mode 透传` `SignAndEncrypt 4843` `pki_dir` 与 `trust false verify true`
- [x] `SourceTimestamp 1601 ticks→Unix ns` `UNCERTAIN→UNCERTAIN Bad→BAD`
- [x] `证书 pki_dir` 与 `trust false` 闭环 `8134 Browse/Poll/Sub 4点`
- [x] `Fake 11 passed` `contract 9/5/3/2 data_plane 2 passed` `lib --lib 11`
- [x] `Reference Server 4840 docker 实测通过（2026-09-04，见上表 Reference 行）`
- [ ] `ProSys/硬件` 与 `§22 50K` Soak（后置）

> 本文档随 `c2e6a7a/7fd55ae` 入仓，满足 `§19.3` 证书不自动信任要求。`2026-08-29 Browse+Security+DateTime/Arrays 已 44/44`

## 6. Stage 2 transport addendum (P0-B, ASCII-only section)

> Section 4 Subscribe 行已是 Stage 2 现状（含 P1-B7）；历史实现曾用 `try_send(mpsc 256) + drain 64`，已废止。
> New event path (Stage 2 P0-B1):
> DataChangeCallback --push--> per-handle Latest-Wins slots (overwrite + stats)
> --drain--> forwarder task --send().await--> adapter mpsc 256 --send().await--> Driver.
> No try_send anywhere on the live path: congestion coalesces OLD samples in
> slots, newest sample is never dropped by an older one. Drops are observable via
> SubscriptionStats { events_received, events_coalesced }.
>
> Other P0-B deltas:
> - P0-B2: per-item BAD from CreateMonitoredItems synthesizes an immediate initial
>   BAD DataValue event for that handle (LastKnown/Placeholder from t=0).
> - P0-B3: CreateMonitoredItems service failure rolls back via best-effort
>   DeleteSubscription; DeleteMonitoredItems results are checked per item
>   (only Good/BadMonitoredItemIdInvalid/BadSubscriptionIdInvalid pass).
> - P0-B4: Browse aggregates continuation pages (browse/browse_next/release);
>   per-page cap is BROWSE_MAX_REFS_PER_PAGE=1000, has_children stays None.
> - P1: disconnect() closes session + aborts event-loop; Revised params are
>   fail-closed; Read/Create/Delete results are cardinality-checked.
> - Tests: cargo test -p mesa-opcua-transport (slots/helpers) +
>   cargo test -p mesa-driver-opcua transport_adapter (Fake-injected adapter
>   rollback/synthetic-BAD/continuation tests).
> - Real opc.tcp:// software integration (local server, no hardware) is still
>   REQUIRED before Stage 2 Gate sign-off.
