# ForgeLink 统一工业设备采集平台 Driver MVP 开发实施方案

**版本：V1.4**  
**范围：S7 / FOCAS2 / OPC UA**

## 1. 目标与边界

ForgeLink V1 只解决一个问题：

> **通过统一驱动框架接入多品牌、多型号、多设备，并持续、稳定地获得实时设备数据。**

V1 实现三类驱动：

| Driver | 目标设备 | 访问模型 |
|---|---|---|
| S7 | Siemens PLC | 地址 / 内存型 |
| FOCAS2 | FANUC CNC | Function / API 型 |
| OPC UA | 通用 OPC UA 设备 | Node / Object 型 |

V1 **只读，不支持任何写入、控制、Method 调用或 CNC 控制操作**。

V1 定位为**实时状态采集**，不承诺每一次采样均无损保存。MQTT、Kafka、历史数据库、WAL、规则引擎、HA、集群、云管理、协议自动切换均不在当前范围。

---

## 2. 核心设计原则

1. Driver 按通信协议/API 开发，不按具体设备型号开发。
2. DeviceProfile 按品牌、系列、型号维护。
3. Device 与 Driver 通过 Endpoint 建立 N:N 关系。
4. 一个 Driver Process 管理 N 个独立 Connection。
5. 协议级使用进程隔离；设备级使用 async Actor；阻塞 SDK 使用专用线程。
6. Core 不解析 S7 DB、FOCAS Function、OPC UA NodeId 等协议细节。
7. Driver 负责连接、采集调度、批量优化、协议解析和数据转换。
8. Control/Metadata Plane 与高频 Data Plane 分离。
9. Point 元数据只注册/同步一次；高频数据只传 ID + Typed Value。
10. 所有队列必须有界，禁止无界缓存。
11. 配置由 Core 持久化；Driver 进程不保存业务配置真值。
12. Driver Protocol 保持语言无关；V1 官方 SDK 仅提供 Rust 实现。

---

## 3. 总体架构

```text
                         External User
                              │
                       REST API / CLI
                              │
┌──────────────────────────────────────────────────────────┐
│                     ForgeLink Core                       │
│                                                          │
│ ConfigStore(SQLite)    DeviceManager                     │
│ DriverManager          EndpointManager                   │
│ PointRegistry          DataIngress                       │
│ LatestValueCache       Diagnostics                       │
└─────────────┬─────────────────┬─────────────────┬────────┘
              │ IPC             │ IPC             │ IPC
              ▼                 ▼                 ▼
      ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
      │ S7 Driver    │  │ FOCAS2 Driver│  │ OPC UA Driver│
      │ Process      │  │ Process      │  │ Process      │
      │ Tokio Runtime│  │ Tokio +      │  │ Tokio Runtime│
      │ N Conn Actor │  │ Blocking     │  │ N Sessions   │
      └──────────────┘  └──────────────┘  └──────────────┘
```

运行模型：

```text
协议/API级      -> 独立进程
同协议多设备    -> Tokio async Connection Actor
阻塞厂商 SDK    -> Blocking Worker / 专用线程池
```

---

## 4. Core 对外接口与配置闭环

### 4.1 V1 管理入口

V1 以 **REST API** 作为唯一正式管理边界；CLI 仅作为 REST API 的客户端，不直接修改配置文件或数据库。

最低 API：

```text
GET/POST/PUT/DELETE  /api/v1/devices
GET/POST/PUT/DELETE  /api/v1/endpoints
GET/POST/PUT/DELETE  /api/v1/tasks
GET                  /api/v1/drivers
POST                 /api/v1/drivers/rescan
POST                 /api/v1/endpoints/{id}/start
POST                 /api/v1/endpoints/{id}/stop
GET                  /api/v1/endpoints/{id}/state
GET                  /api/v1/points/latest
GET                  /api/v1/diagnostics

GET                  /api/v1/certificates/opcua/{store}
POST                 /api/v1/certificates/opcua/trusted
DELETE               /api/v1/certificates/opcua/trusted/{thumbprint}
POST                 /api/v1/certificates/opcua/rejected/{thumbprint}/trust
```

开发/验证阶段可提供 WebSocket 实时数据流：

```text
/api/v1/stream
```

该接口属于调试与验证能力，不作为 Driver IPC。

`POST /api/v1/drivers/rescan` 仅重新扫描 Driver Manifest 并刷新可用驱动目录；V1 **不热替换正在运行的 Driver Process**。已运行 Driver 的二进制升级仍要求停止相关 Runtime 或重启 Core。

OPC UA 证书 API 仅提供第 8 节定义的最小 TrustStore 运维能力；CLI 通过同一 REST API 暴露等价命令，不直接操作证书目录。

### 4.2 REST 安全边界

V1 REST API **默认且仅允许绑定 loopback**：

```text
127.0.0.1 / ::1
```

V1 不提供远程管理认证协议，也不允许默认监听 `0.0.0.0`。如未来需要远程管理，必须单独增加 TLS、身份认证和授权后再开放非 loopback 地址。

CLI 仅通过本机 REST API 管理 Core，不直接访问 SQLite。

### 4.3 配置持久化

Core 使用 SQLite 保存：

- Device
- Endpoint
- AcquisitionTask
- PointRegistry
- 配置 Revision
- Driver 启停状态

DeviceProfile 和 Driver Manifest 为版本化静态文件。

Core 重启后按数据库状态恢复 Endpoint、Task 和 Point 映射。

V1 按**单管理员/单写者**模型实现 REST 配置修改，不提供通用资源级乐观锁；该限制必须记录为已知取舍。Task 配置本身仍使用第 6 节的 `revision` 保证 Driver 侧原子应用。

---

## 5. 核心对象模型

### 5.1 DeviceProfile

描述品牌、系列、型号和推荐 Driver。

```yaml
id: siemens.s7-1500
vendor: siemens
family: s7
model: s7-1500

drivers:
  - id: s7
    recommended: true
  - id: opcua
    conditional: true
```

### 5.2 Device

表示现场物理设备。

```yaml
id: plc001
name: Line1-PLC
profile: siemens.s7-1500
```

### 5.3 Endpoint

表示 Device 使用某个 Driver 的连接配置。

```yaml
id: plc001-s7
device: plc001
driver: s7

connection:
  host: 192.168.1.10
  port: 102
  rack: 0
  slot: 1
```

同一 Device 可拥有多个 Endpoint，V1 不做自动协议切换。

### 5.4 Connection

Endpoint 的运行时实例，每个 Connection 独立拥有：

- Socket / SDK Handle / OPC UA Session
- Connection State
- Timeout / Reconnect
- Acquisition Tasks
- Poll Scheduler 或 Subscription Manager
- Outbound bounded queue

推荐一个 Connection 一个 Actor，不共享可变连接状态。

### 5.5 AcquisitionTask

统一 Schema：

```rust
struct AcquisitionTask {
    id: String,
    mode: TaskMode,             // Poll | Subscribe
    interval_ms: Option<u64>,   // Poll 必填
    binding: DriverBinding,
}

struct DriverBinding {
    kind: String,
    config: JsonValue,
}
```

Core 不解析 `binding.config`，只做 Schema 校验和持久化；Driver 负责协议语义。

V1 中：

- S7 / FOCAS2：`Poll`
- OPC UA：`Poll` 或 `Subscribe`

---

## 6. Point ID 与配置生命周期

### 6.1 ID 所有权

- `endpoint_id`：Core 创建并持久化，稳定不变。
- `point_key`：Driver 根据 Task 生成的稳定逻辑键，例如 `motor.speed`、`axis.X.position`。
- `point_id`：**Core 分配并持久化**，在同一 Endpoint 内稳定；分配后不复用。
- `connection_handle`：Core/Driver 会话内临时 `u32`，仅用于紧凑 IPC，不作为持久标识。

当 Point 被删除时，Core 保留 `(endpoint_id, point_key) -> point_id` 的 tombstone 映射；同一 `point_key` 后续重新加入时恢复原 `point_id`，避免数据身份漂移。

稳定身份为：

```text
(endpoint_id, point_id)
```

Driver 重启或 Core 重启后必须恢复原有 `point_id`。

### 6.2 配置流程

```text
Core -> ConfigureTasks(revision, full task snapshot)
Driver -> PointDescriptors(point_key, type, unit, ...)
Core -> upsert PointRegistry / assign stable point_id
Core -> ApplyPointMap(revision, point_key -> point_id)
Driver -> ConfigApplied(revision)
Core -> StartConnection
```

`PointDescriptor` 与 `PointDefinition` 的关系：

- `PointDescriptor`：Driver 在 `ConfigureTasks` 后返回，包含 `point_key/type/unit/...`，**不含 `point_id`**。
- `PointDefinition`：Core 为 Descriptor 分配稳定 `point_id` 后形成并持久化的正式定义。

V1 的 `ConfigureTasks` 为**全量快照替换**，不做增量 Patch。

同一 Endpoint 的全量配置中，`point_key` 必须全局唯一。Driver 在 configure 阶段负责检测跨 Task 冲突并返回 `ConfigurationError/DUPLICATE_POINT_KEY`；Core 在写入 PointRegistry 前再次校验，形成双重保护。

运行中的 Endpoint 不允许热 Apply 配置。用户增删 Task 时统一执行：

```text
RUNNING
 -> StopConnection
 -> ConfigureTasks(new revision, full snapshot)
 -> PointDescriptors
 -> ApplyPointMap
 -> ConfigApplied
 -> StartConnection(new stream_epoch)
 -> RUNNING
```

新 Revision 必须完整构建成功后才替换旧 Revision；任一步失败则保持 Endpoint 为 STOPPED/FAILED，并保留上一版已持久化配置和 Point 映射，禁止半配置运行。

---

## 7. 三类 Driver 的统一方式

原则：

> **不统一底层访问方式，只统一 PointDefinition 和 DataBatch。**

### 7.1 S7：地址型

```yaml
id: fast-data
mode: poll
interval_ms: 100
binding:
  kind: s7.address-group
  config:
    items:
      - key: motor.speed
        address: DB10.DBD20
        data_type: REAL
      - key: motor.running
        address: DB10.DBX24.0
        data_type: BOOL
```

Driver 内部负责：

```text
地址解析 -> 合并连续区域 -> PDU 分包 -> Read -> Decode -> PointValue
```

V1：

- DB / M / I / Q 读取
- BOOL / BYTE / WORD / DWORD / INT / DINT / REAL / LREAL
- STRING 基础解析
- Batch Read
- Timeout / Reconnect

部署约束：S7-1200/1500 使用传统绝对地址访问时，应检查 CPU 的保护设置、PUT/GET 访问许可及 DB 是否为可绝对寻址的标准访问布局。此类限制必须在连接测试中给出明确诊断，而不是仅返回“连接失败”。

### 7.2 FOCAS2：Function/API 型

```yaml
id: axis-position
mode: poll
interval_ms: 100
binding:
  kind: focas.resource
  config:
    resource: axis.position
```

Driver 内部：

```text
Resource -> FOCAS Adapter -> FOCAS Function -> SDK Struct -> Engineering Value -> Points
```

例如：

```text
axis.X.position
axis.Y.position
axis.Z.position
```

V1 优先资源：

- CNC 状态
- 轴位置
- 主轴速度 / 负载
- 当前程序
- 当前报警
- 当前刀具基础信息

FOCAS2 属于外部厂商 SDK 依赖。开发前必须完成：

1. SDK 获取与授权/再分发条件确认；
2. Windows/Linux 目标平台库文件和调用差异确认；
3. 目标 CNC 系列支持函数与 Ethernet 连接限制确认；
4. SDK 版本写入 Driver Metadata 和兼容矩阵。

同步阻塞调用放入 Blocking Worker，不得阻塞 Tokio I/O Worker。

### 7.3 OPC UA：Node/Object 型

Poll 示例：

```yaml
id: machine-data
mode: poll
interval_ms: 500
binding:
  kind: opcua.node-group
  config:
    nodes:
      - key: motor.speed
        node_id: ns=2;s=Motor.Speed
```

Subscription 示例：

```yaml
id: machine-subscription
mode: subscribe
binding:
  kind: opcua.subscription
  config:
    publishing_interval_ms: 250
    sampling_interval_ms: 100
    queue_size: 1
    discard_oldest: true
    nodes:
      - key: motor.speed
        node_id: ns=2;s=Motor.Speed
```

V1 支持：

- Endpoint 连接
- Node Read / Batch Read
- Browse
- Subscription
- SourceTimestamp
- Session Reconnect
- OPC UA Application Certificate / Trust Store 基础管理

不支持 Method、History Access、Write。

OPC UA Poll 与 Subscription 都输出 DataBatch：

- Poll：按采集完成顺序生成 Batch。
- Subscription：按 Notification 到达顺序生成 Batch。
- `sequence` 表示 Driver 输出顺序，不代表设备原始时间顺序。
- OPC UA 原始 `SourceTimestamp` 保留到 PointValue。
- Subscription 收到 KeepAlive 但没有 DataChange Notification 时，仅刷新 Session/Subscription 健康状态和 `last_success_timestamp`，**不生成 DataBatch、不递增数据 `sequence`**，避免静默稳定值被误判为失联。

---

## 8. OPC UA 证书与信任管理

证书管理属于 V1 必需能力，而非后续附加项。

建议目录：

```text
data/certificates/opcua/
├── own/
├── trusted/
├── issuers/
└── rejected/
```

最低能力：

- 首次运行生成 Application Instance Certificate；
- 导入/删除 Trusted Certificate；
- 查看 Rejected Certificate；
- 信任 Rejected Certificate；
- 证书到期时间诊断；
- Endpoint SecurityPolicy / MessageSecurityMode 配置；
- 私钥文件权限限制。

证书信任和连接失败必须返回可诊断错误，不允许自动信任未知生产证书。

管理入口统一走 Core REST API：

```text
GET    /api/v1/certificates/opcua/own
GET    /api/v1/certificates/opcua/trusted
GET    /api/v1/certificates/opcua/issuers
GET    /api/v1/certificates/opcua/rejected
POST   /api/v1/certificates/opcua/trusted
DELETE /api/v1/certificates/opcua/trusted/{thumbprint}
POST   /api/v1/certificates/opcua/rejected/{thumbprint}/trust
```

REST 负责校验证书格式、去重和原子落盘；Driver 只读取 Core 下发或挂载的已生效 TrustStore，不自行修改信任关系。CLI 仅作为上述 REST API 的客户端。

---

## 9. 数据模型

### 9.1 PointDefinition

```rust
struct PointDefinition {
    point_id: u32,
    point_key: String,
    data_type: DataType,
    unit: Option<String>,
}
```

### 9.2 Value

V1 不使用通用 JSON Object，但必须支持时间和数组：

```rust
enum Value {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    DateTime(i64),          // Unix time ns, UTC

    BoolArray(Vec<bool>),
    I32Array(Vec<i32>),
    U32Array(Vec<u32>),
    I64Array(Vec<i64>),
    U64Array(Vec<u64>),
    F32Array(Vec<f32>),
    F64Array(Vec<f64>),
    StringArray(Vec<String>),
    DateTimeArray(Vec<i64>),
}
```

FOCAS2 结构体优先拆成稳定 Point；OPC UA Array 保留为 Typed Array。

### 9.3 Quality

```text
GOOD
UNCERTAIN
BAD
```

同时允许附加可选 `quality_code` 保存协议原生或 ForgeLink 原因码。

默认映射：

- 成功读取：GOOD
- OPC UA：按原生 StatusCode 映射 GOOD / UNCERTAIN / BAD
- 通信断开/超时：BAD
- Driver 不人工生成 UNCERTAIN，除非协议或明确转换规则给出该语义

---

## 10. 时间戳与序列号

所有 IPC 时间统一为：

```text
Unix time in nanoseconds, UTC, int64
```

### DataBatch

```rust
struct DataBatch {
    connection_handle: u32,
    stream_epoch: u64,
    sequence: u64,
    timestamp_ns: i64,
    values: Vec<PointValue>,
}
```

语义：

- `timestamp_ns`：Driver 完成本批数据采集/接收 Notification 的本机 UTC 时间。
- `source_timestamp_ns`：可选，由设备/协议提供；没有则不填。
- `stream_epoch`：每次 Connection Start/Reopen 由 Core 生成的新随机值。
- `sequence`：同一 `(connection_handle, stream_epoch)` 内从 1 递增。

Core 通过 `(endpoint_id, stream_epoch, sequence)` 识别重启、乱序与缺口。

业务时间戳统一使用 UTC Unix ns。性能测量不得通过两个进程的 UTC 时间直接相减；内部延迟/耗时指标使用**宿主机单调时钟**。Linux 使用系统级 monotonic clock，Windows 使用等价高精度单调计时源。单调时钟值仅用于进程内/同宿主性能指标，不持久化、不作为业务时间。若目标平台无法保证跨进程单调时钟可比较，则不输出跨进程 delivery latency，仅保留 Core/Driver 各自的 queue/process duration 指标。

---

## 11. 断线与数据质量语义

V1 不在断线期间为每个 Point 周期性发送 BAD 心跳帧，以避免无意义数据放大。

规则：

1. Connection 进入 `RECONNECTING/FAILED` 时立即发送 `ConnectionState`。
2. Core 将该 Endpoint 的 LatestValue 标记为 `BAD / COMMUNICATION_LOST`。
3. 断线期间不生成虚假采样值。
4. 重连后，旧缓存保持 BAD，直到各 Point 获得新值。
5. 第一次成功新值直接为 GOOD；只有协议明确返回 UNCERTAIN 时才标记 UNCERTAIN。
6. Driver 层不做插值、补点或历史回填。

### 11.1 Connection 重连与 FAILED 语义

可重试网络错误使用连接级退避：

```text
1s -> 2s -> 5s -> 10s -> 30s(max)
```

默认满足任一条件即从 `RECONNECTING` 转为 `FAILED`：

- 连续重连失败达到 10 次；或
- 自首次断线起 5 分钟仍未恢复。

进入 `FAILED` 后暂停 60 秒，再由 EndpointManager 发起一个新的自动恢复周期。配置错误、认证/证书错误、Unsupported 等**非重试错误**直接进入 `FAILED`，且不会自动重试，直到配置 Revision 变化或用户显式 Start。

以上阈值为 V1 默认值，允许通过 Core 全局配置覆盖。

---

## 12. 背压与流控

所有 Data Plane channel/queue 必须 bounded。

推荐链路：

```text
Connection Actor
  -> bounded outbound queue
  -> Driver DataPublisher
  -> IPC socket
  -> Core bounded DataIngress
  -> LatestValueCache / consumer
```

V1 背压策略：

1. 协议网络读取不得因 Core 消费慢而无限阻塞。
2. Queue 达到阈值时，对普通 Point 数据采用 **Latest-Wins Coalescing**：同一 `point_id` 只保留最新值。
3. 已过期的旧 Batch 可被丢弃/合并，不允许内存无限增长。
4. 每次合并/丢弃必须增加指标计数。
5. `ConnectionState`、DriverError 等控制消息不得与普通 Point 数据共用可被丢弃的队列。

因此 V1 明确为**实时最新状态系统**。若未来要求每次采样无损，则必须增加持久化/WAL，不通过扩大内存队列解决。

---

## 13. Driver 调度模型

Core 只负责：

```text
OpenConnection
ConfigureTasks
ApplyPointMap
StartConnection
```

### Poll Driver

S7 / FOCAS2 / OPC UA Poll：

```text
ConnectionActor
├── Poll Group A
├── Poll Group B
└── Poll Group C
```

Driver 内完成批量优化。

### Subscription Driver

OPC UA Subscription 不进入 Poll Scheduler：

```text
OPC UA Session
  -> Subscription
  -> Notification callback/task
  -> Point mapping
  -> DataBatch
```

两种模式最终进入相同 DataIngress。

---

## 14. Driver IPC V1

最低消息集合：

```text
Hello
Welcome
GetMetadata
OpenConnection
CloseConnection
ConfigureTasks
PointDescriptors
ApplyPointMap
ConfigApplied
StartConnection
StopConnection
ConnectionState
DataBatch
DriverError
Ping
Pong
Shutdown
```

### 14.1 Transport

V1：

```text
127.0.0.1 TCP + Length-prefixed Protobuf
```

### 14.2 本机认证

- 只监听 loopback。
- Core 启动 Driver 时生成 256-bit 随机 session token。
- Token 通过继承 stdin/pipe 等非命令行方式传入 Driver。
- `Hello` 必须完成 token 校验后才接受控制消息。
- 每个 Driver Process 默认只接受一个 Core 管理连接。

后续可切换 UDS / Named Pipe，Protobuf 协议保持不变。

### 14.3 Hello / Welcome 与版本协商

`Hello` 至少携带：

```text
driver_id
driver_version
protocol_major
protocol_minor
sdk_version
platform
instance_id
session_token
```

`Welcome` 至少携带：

```text
core_version
accepted_protocol_major
accepted_protocol_minor
connection/session parameters
```

协商规则：

- `protocol_major` 不一致：拒绝启动 Driver，并返回明确的 IncompatibleProtocol 错误；
- Major 相同：协商双方支持的最低兼容 Minor；
- 新增可选消息/字段必须保持同 Major 的向后兼容；
- DriverManager 必须在 `OpenConnection` 前完成版本协商。

### 14.4 Heartbeat 与进程终止

V1 默认：

- Core 每 5 秒发送一次 `Ping`；
- Driver 应在 3 秒内返回 `Pong`；
- 连续 3 次 Heartbeat 失败则 Driver 被判定为 `UNRESPONSIVE`；
- DriverManager 立即停止向该进程发送业务请求，并进入第 17 节恢复流程。

正常退出：

```text
Shutdown -> 最多等待 5s -> 仍未退出则强制终止
```

POSIX 可在宽限后使用 `SIGTERM`/`SIGKILL`；Windows 在宽限结束后使用服务/进程管理 API 强制终止。Heartbeat 周期与宽限时间允许配置，但必须有上限，禁止无限等待卡死 Driver。

### 14.5 父进程存活与孤儿 Driver 防护

Core 非优雅退出（崩溃、强杀、服务异常终止）时，不允许遗留继续持有设备连接的孤儿 Driver。

V1 必须同时实现两层保护：

1. **父进程存活管道**：Core 为每个 Driver 保留专用 stdin/liveness pipe。Driver 在读取启动 token 后仍必须持续监控该管道；收到 EOF 视为 Core 已死亡，立即停止采集、关闭设备连接并退出。Driver 不得 daemonize、detach 或把该管道复制给无关子进程。
2. **OS 级父死联动**：
   - Linux：Driver 启动最早阶段设置 `PR_SET_PDEATHSIG`，推荐 `SIGKILL`；设置后再次校验父 PID，消除父进程在初始化窗口内死亡的竞态。
   - Windows：Core 创建 Job Object，并启用 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，所有 Driver Process 必须加入该 Job；Core 进程终止后由 OS 强制清理 Driver。

父死联动优先保证“不会继续占用设备连接”，不要求在 Core 已崩溃场景完成完整优雅清理。新 Core 启动前不得依赖“旧 Driver 自己最终会超时”作为恢复策略。

---

## 15. Driver Manifest 与发现机制

每个 Driver 必须以独立目录发布：

```text
drivers/s7/
├── driver.toml
└── forgelink-driver-s7[.exe]
```

最小 `driver.toml`：

```toml
id = "s7"
name = "Siemens S7"
version = "0.1.0"
executable = "forgelink-driver-s7"
protocol_major = 1
protocol_minor = 0
sdk = "rust"

# 可选；存在时 DriverManager 必须做宿主平台预检查
os = ["windows", "linux"]
arch = ["x86_64"]
```

DriverManager 启动时扫描 `drivers/*/driver.toml`，并执行：

1. `id` 唯一性校验；
2. Manifest 必填字段和 SemVer 格式校验；
3. 可执行文件存在性/可执行性校验；
4. 若声明 `os/arch`，必须与当前宿主匹配，否则标记为 `PLATFORM_MISMATCH` 且禁止启动；
5. `protocol_major` 兼容性预检查；
6. 启动后再通过 `Hello/Welcome` 完成实际版本协商。

V1 不实现在线安装、签名市场或热替换；新增 Driver 的标准流程为“部署目录 -> `POST /api/v1/drivers/rescan` 或重启 Core -> 发现并启动”。Rescan 只更新发现目录，不替换正在运行的 Driver Process。

---

## 16. Driver SDK

V1 官方 SDK 为 Rust：

```rust
trait Driver {
    fn metadata(&self) -> DriverMetadata;
    async fn open_connection(&self, config: JsonValue)
        -> Result<Box<dyn DriverConnection>>;
}
```

```rust
trait DriverConnection {
    async fn configure(
        &mut self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>>;

    async fn apply_point_map(&mut self, map: PointMap) -> Result<()>;

    async fn run(
        self,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<()>;
}
```

SDK 负责 IPC、认证、Heartbeat、序列化、bounded queue、Shutdown 和基础 metrics。

**已知取舍：**V1 SDK 仅支持 Rust，但 Driver Protocol 使用 Protobuf 且不暴露 Rust ABI，未来可以增加 C++/C#/Go SDK，而无需改变 Core 的驱动模型。

---

## 17. Driver 进程恢复

DriverManager 负责进程生命周期。

默认恢复策略：

```text
crash -> 1s -> 2s -> 5s -> 10s -> 30s(max)
```

若 5 分钟内连续异常退出达到 5 次：

```text
Driver -> CIRCUIT_OPEN 60s
```

之后允许再次尝试启动。参数应可配置。

Driver 重启后：

```text
Handshake
 -> OpenConnection
 -> ConfigureTasks(current revision)
 -> restore PointMap
 -> StartConnection(new stream_epoch)
```

Driver 不自行持久化这些配置。

---

## 18. 最小可观测性

V1 不要求完整 Prometheus/OTel 平台，但必须维护并可通过 `/api/v1/diagnostics` 查询以下指标：

### Driver / Connection

- connection_state
- reconnect_total
- connection_error_total
- last_success_timestamp
- poll_duration_ms / request_duration_ms
- subscription_notification_total

### Data Plane

- data_batch_total
- point_value_total
- ipc_queue_depth
- ingress_queue_depth
- backpressure_coalesce_total
- backpressure_drop_total
- ipc_delivery_latency_ms（仅在宿主机跨进程单调时钟可比较时提供）

日志至少包含：`driver_id`、`endpoint_id`、`stream_epoch`、`error_kind`。

---

## 19. Driver 外部约束

### 19.1 S7

对 S7-1200/1500 必须在连接自检中检查并提示：

- PUT/GET/远程访问许可可能未启用；
- CPU Protection/Security 可能拒绝访问；
- 绝对地址访问不能直接读取不兼容的 optimized DB；
- 连接成功不代表目标 DB 可读。

### 19.2 FOCAS2

开发前设置 Vendor Dependency Gate：

- FANUC SDK/Library 合法获取；
- 许可和再分发条件确认；
- Windows/Linux Library 版本确认；
- 目标 CNC 型号/系列函数支持确认；
- CNC 侧 Ethernet 连接/参数限制通过真机确认。

无法完成上述 Gate 时，不发布 FOCAS2 Driver 的生产版本。

### 19.3 OPC UA

OPC UA 安全连接必须处理 ApplicationInstanceCertificate、TrustList、Issuer、Rejected Certificate 等生命周期，不允许把“忽略证书校验”作为生产默认配置。

---

## 20. 测试与真机替代方案

测试分四层：

```text
Unit Test
 -> Protocol/Adapter Fake
 -> Software Simulator/Test Server
 -> Real Hardware
```

### S7

- Codec/地址/PDU 单元测试
- 可使用软件 S7 Server/Test Double 进行自动化协议测试
- 有条件时使用 Siemens PLCSIM Advanced 验证网络行为
- 最终必须使用目标 S7-1200/1500 真机验证

### OPC UA

- 使用标准 OPC UA Test Server/模拟 Server
- 自动测试 Read、Browse、Subscription、证书信任、断线恢复
- 最终使用至少一个真实工业 OPC UA Server 验证

### FOCAS2

FOCAS2 缺少可替代真机的高可信通用模拟器，因此必须设计 `FocasApi` 抽象：

```text
FocasApi
├── NativeFocasApi
└── FakeFocasApi
```

Fake 用于 CI、错误注入和多连接测试；协议/SDK 最终兼容性必须由 FANUC 真机完成。FOCAS2 真机资源必须在 Phase 开始前落实，否则该阶段标记为外部依赖阻塞风险。

---

## 21. Driver Contract Test

所有 Driver 必须通过同一套 `tests/driver-contract`，作为 SDK 与驱动的共同验收基线：

| 合同测试 | 最低要求 |
|---|---|
| Manifest Discovery | Manifest 可发现且字段合法 |
| Handshake | token + protocol version 协商成功 |
| Incompatible Version | Major 不兼容时明确拒绝 |
| Metadata | DriverMetadata 完整 |
| Open / Close | Connection 生命周期正常 |
| Invalid Config | 返回结构化 ConfigurationError |
| Configure Snapshot | 全量 Revision 可原子应用 |
| Duplicate point_key | configure 阶段必须拒绝 |
| Point Registration | Descriptor -> stable PointDefinition 映射正确 |
| Start / Stop | 启停可重复执行且无资源泄漏 |
| Multiple Connections | 同一 Driver 至少并发多个独立 Connection |
| Partial Failure | 单 Connection 故障不影响其他 Connection |
| Reconnect | 退避、FAILED、恢复周期符合规范 |
| Heartbeat / Hang | 无 Pong 时 Core 能判死并重启 Driver |
| Parent Death / Orphan Guard | 强杀 Core 后 Driver 必须由 liveness EOF 或 OS 父死联动自动退出，不继续占用设备连接 |
| Driver Crash Restore | 强杀 Driver 后完成 Handshake -> OpenConnection -> ConfigureTasks -> restore PointMap -> new epoch，Point ID 保持稳定 |
| Backpressure | 队列有界且 Latest-Wins/计数生效 |
| DataBatch Semantics | epoch/sequence/timestamp/quality 语义正确 |
| Runtime Reconfigure | RUNNING 修改走 Stop->Configure->Start，新 epoch 生效 |
| Graceful Shutdown | 5s 宽限后可强杀，无遗留子进程/句柄 |

Simulator Driver 必须首先完整通过 Contract Test，后续 S7/FOCAS2/OPC UA 不得绕过该基线。

---

## 22. 性能预算与压测基线

参考环境：**8 Core x86_64 / 16 GB RAM / Release Build**。以下为 V1 工程目标，不代表所有真实 PLC/CNC 的设备侧性能。

### Data Plane 基线

| 场景 | 验收目标 |
|---|---|
| DataPlane-50K | 连续 60 分钟 >= 50,000 Point Updates/s |
| IPC 延迟 | IPC delivery p95 <= 20 ms，p99 <= 50 ms（使用宿主机单调时钟；不使用 UTC 时间差） |
| 内存稳定性 | 预热后持续压测 RSS 不得出现无界增长；60 分钟增长 <= 10% |
| Backpressure | Core 消费能力降至生产速率 25% 时，内存仍有界、连接 Actor 保持响应 |
| Conn-1000 | Simulator 1000 Connections 连续运行 60 分钟，无 Task/Handle 泄漏 |

真实 Driver 另外记录：

- 单 Connection RSS 增量
- CPU 使用率
- 最大稳定采样速率
- Device 侧最大并发/请求限制

真实设备能力不强行要求达到 Simulator 的 50K/1000 Connection 指标。

---

## 23. 项目目录

```text
forgelink/
├── crates/
│   ├── core-types/
│   ├── driver-protocol/
│   ├── driver-sdk/
│   ├── driver-manager/
│   ├── device-manager/
│   ├── config-store/
│   ├── profile-registry/
│   ├── point-registry/
│   ├── data-ingress/
│   └── core-api/
│
├── drivers/
│   ├── simulator/    # driver.toml + executable
│   ├── s7/           # driver.toml + executable
│   ├── focas2/       # driver.toml + executable
│   └── opcua/        # driver.toml + executable
│
├── profiles/
│   ├── siemens/
│   ├── fanuc/
│   └── opcua/
│
├── apps/
│   ├── forgelinkd/        # console/service entry
│   └── forgelink-cli/
│
├── packaging/
│   ├── windows-service/
│   └── systemd/
│
└── tests/
    ├── driver-contract/
    ├── integration/
    ├── performance/
    └── hardware/
```

---

## 24. 开发实施顺序

| Phase | 内容 | 完成条件 |
|---|---|---|
| 0 | core-types + schema | ID、时间、Quality、Value、Task Schema 冻结 |
| 1 | ConfigStore + Core REST API | Device/Endpoint/Task CRUD 闭环 |
| 2 | driver-protocol + Manifest + IPC auth | 发现、平台校验、token、版本协商、Heartbeat、父死联动可用 |
| 3 | driver-sdk + Simulator + Contract Test | 合同测试全通过，1000 Connection / Backpressure 验证 |
| 4 | DriverManager + Recovery | Driver crash/hang/restart/config restore 闭环 |
| 5 | S7 Driver | 多 PLC、Batch、权限诊断、真机验证 |
| 6 | FOCAS2 Driver | Fake API + Native SDK、多 CNC、真机验证 |
| 7 | OPC UA Driver | Poll/Subscription、KeepAlive、证书 REST 运维、Browse、真机验证 |
| 8 | Service Packaging + Performance + Soak | Windows Service/systemd 可运行，达到性能预算 |

---

## 25. 部署与服务化预留

V1 运行入口统一为 `apps/forgelinkd`。

部署目标：

- Windows：支持作为 Windows Service 运行，预留 install/start/stop/uninstall 包装脚本或 service wrapper；所有 Driver 加入启用 `KILL_ON_JOB_CLOSE` 的 Job Object；
- Linux：提供对应 systemd unit；Driver 启动时设置 `PR_SET_PDEATHSIG`；
- Driver 子进程必须由 `forgelinkd` 创建并纳入同一生命周期管理，同时持续监控父进程 liveness pipe；Core 优雅退出、崩溃或被强杀均不得遗留孤儿 Driver；
- 日志默认写 stdout/stderr，由部署层负责轮转：Linux 优先使用 journald/systemd 的保留策略；Windows Service 如落文件必须启用大小/数量受限的 rolling log，禁止无限增长单一日志文件。

V1 不要求远程编排、容器化或集群部署，但服务化运行、孤儿进程清理和日志有界增长必须纳入安装包与 Soak Test。

---

## 26. V1 验收标准

1. S7、FOCAS2、OPC UA 三个 Driver 均以独立进程运行。
2. 一个 Driver Process 可管理多个独立 Connection，单设备故障不阻塞其他设备。
3. Core 提供 REST API 完成 Device/Endpoint/Task 配置、启停和状态查询。
4. Core 重启能够从 SQLite 恢复配置、Point ID 和运行状态。
5. Driver 重启后 Point ID 保持稳定，并使用新的 `stream_epoch` 恢复采集。
6. S7 地址、FOCAS2 Resource、OPC UA Node/Subscription 均统一输出 Typed PointValue/DataBatch。
7. OPC UA Certificate/TrustList 具备最小可运维能力。
8. 所有 Data Plane 队列 bounded；背压压测不得造成无界内存增长。
9. 断线时 Point 状态转 BAD；重连后只以新数据恢复 GOOD。
10. Driver Data Plane 达到第 22 节性能预算。
11. V1 全部 Driver 均只读，不存在设备控制路径。
12. 单 Driver 崩溃不影响 Core 和其他 Driver。
13. Driver Manifest 可被发现；Hello/Welcome 能完成 Protocol Major/Minor 协商，Major 不兼容时明确拒绝启动。
14. Heartbeat 能识别 Driver hang，Shutdown 超时后可强制终止并进入恢复流程。
15. 所有 Driver 必须通过第 21 节 Driver Contract Test。
16. 运行中 Task 变更必须走 Stop->Configure->ApplyPointMap->Start，并生成新 `stream_epoch`。
17. OPC UA KeepAlive 能刷新连接健康度但不产生空 DataBatch。
18. 在 **Driver Protocol 主版本兼容且新增能力为可选扩展** 的前提下，新增 Driver 不需要修改或重新编译 Core。
19. `forgelinkd` 可作为 Windows Service/Linux systemd 服务运行；无论优雅退出还是 Core 被强杀，Driver 均不得成为孤儿进程或继续持有设备连接。
20. OPC UA Trusted/Rejected Certificate 的最小查看、导入、删除、信任操作可通过 Core REST API/CLI 完成。
21. Driver Manifest 可选 `os/arch` 能阻止不匹配平台的二进制被启动；Driver rescan 能刷新发现目录但不得热替换运行中进程。
22. Driver Crash Restore Contract Test 通过：Point ID 稳定、配置恢复完整、恢复后使用新 `stream_epoch`。
23. 三个正式 Driver 均完成目标真机兼容验证并形成 Compatibility Matrix。

---

## 27. 最终数据链路

配置链：

```text
REST API
 -> ConfigStore
 -> Device / Endpoint / AcquisitionTask
 -> Driver ConfigureTasks
 -> PointDescriptors
 -> Core PointRegistry / stable point_id
 -> ApplyPointMap
```

运行链：

```text
Device
 -> Connection Actor / OPC UA Subscription
 -> Protocol Native Data
 -> Driver Decode / Adapter
 -> point_id + Typed Value
 -> bounded DataBatch queue
 -> authenticated IPC
 -> Core DataIngress
 -> LatestValueCache
```

最终统一边界：

```text
S7 DB Address
FOCAS2 Function
OPC UA Node / Notification
        ↓
      Driver
        ↓
Stable Point ID + Typed Value + Quality + Timestamp
        ↓
    ForgeLink Core
```

---

## 28. 关键工程结论

ForgeLink V1 不追求协议数量，而是用三类访问范式验证驱动框架：

```text
S7      -> Address / Memory
FOCAS2  -> Function / API
OPC UA  -> Node / Subscription
```

开发前必须冻结：

```text
ID 生命周期
Task Schema / Runtime Reconfigure
Point Registration / point_key 唯一性
Timestamp / Sequence / Monotonic Metrics
Quality / Reconnect Semantics
Backpressure
Driver Manifest / Discovery / Protocol Negotiation / Platform Match / Rescan
IPC Authentication / Heartbeat / Forced Termination
Parent-Death / Orphan Driver Guard
OPC UA Certificate Management API
Config Recovery
```

只有这些基础语义稳定后，才能开始正式协议驱动开发。

---

# 附录 A. Simulator Driver 行为规范

Simulator 是 Driver Framework 的参考实现和 Contract/Performance Test 基线。它必须支持以下可配置行为，以便在没有真机时覆盖正常采集、时序异常、连接故障和数据质量场景。

## A.1 数据源

| 类型 | 行为 |
|---|---|
| Constant | 固定值 |
| Counter | 按步长递增，可配置初值、步长和回绕 |
| Random | 指定范围随机值，可选固定 seed |
| Sine | 可配置振幅、周期、偏移的正弦值 |
| Toggle | Bool 周期翻转 |
| String | 固定字符串或序号模板 |
| DateTime | 当前 UTC 时间或可配置时间推进 |
| Array | 固定长度 Typed Array，用于验证 OPC UA 类数组值 |

## A.2 时序与性能注入

- `delay_ms`：为单次采集增加固定延迟。
- `jitter_ms`：增加随机抖动。
- `burst`：按配置批量产生 Point 更新。
- `silent_interval`：保持连接健康但暂不产生 DataBatch，用于验证 OPC UA Subscription keep-alive 类语义。
- 支持不同 Connection、Task 使用不同采样周期。

## A.3 连接与故障注入

- `disconnect_after`：运行指定时间或批次数后主动断开。
- `reconnect_after`：模拟恢复可用。
- `connect_timeout`：模拟连接超时。
- `hang`：停止响应 Control/Ping，用于验证 Heartbeat、强杀和恢复。
- `crash`：主动异常退出，用于验证 Driver Crash Restore。
- 单 Connection 故障不得影响同 Driver 其他 Connection。

## A.4 数据质量与错误注入

- `GOOD`、`UNCERTAIN`、`BAD` Quality 注入。
- 配置错误：非法 binding、重复 `point_key`、不支持的数据类型。
- 采集错误：Timeout、ProtocolError、DecodeError、DeviceError。
- 支持恢复后从 BAD/UNCERTAIN 回到 GOOD 的状态转换测试。

## A.5 背压验证

Simulator 必须能持续产生高于 Core 消费能力的数据速率，用于验证：

- bounded queue 不发生无界增长；
- Latest-Wins/旧值丢弃策略生效；
- dropped/coalesced 计数可观测；
- Control/Heartbeat 不因 Data Plane 拥塞失去响应。

Simulator 的行为配置属于测试配置，不进入生产 DeviceProfile。

