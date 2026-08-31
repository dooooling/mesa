# Mesa Driver Descriptor / Resource / Control 可验收契约与完整开发实施方案 V2

> 仓库：`dooooling/mesa`  
> 基线：当前 `master`  
> 文档性质：**施工契约 + 测试契约 + 验收契约**  
> 核心原则：不推翻现有 Runtime/Data Plane；先冻结契约，再实现 Descriptor、动态 UI、Profile、Discovery 和 Control Plane。

---

# 0. 文档目的

上一版已经确定了总体方向：

- Driver 自描述；
- UI 不理解具体协议；
- Core 不理解具体协议；
- Resource 与 Physical Operation 分离；
- Profile 降低用户心智负担；
- Write / Command 分离；
- Descriptor 不进入 Data Plane 热路径。

但要真正进入施工，还必须把上述方向冻结为**可以直接写测试断言、DDL、错误码、性能 Gate 和迁移脚本的契约**。

本版重点补齐：

1. Data Plane 精确语义；
2. Descriptor / IPC / Driver 三类版本兼容规则；
3. Secret / Certificate / Control 安全边界；
4. ConfigStore V1 → V2 增量迁移；
5. Management / Data Plane 队列隔离和性能预算；
6. Diagnostics / Runbook；
7. DeviceProfile 匹配、Preset 展开、i18n、ImportIssue；
8. 每个阶段明确的 Definition of Done。

---

# 1. 当前仓库真实基线

本方案不重新发明已经存在的能力。

## 1.1 当前主链路

```text
Device
  ↓
Endpoint
  ↓
AcquisitionTask
  ↓
DriverBinding
  ↓
Driver.configure()
  ↓
PointDescriptor[]
  ↓
Core 分配稳定 Point ID
  ↓
ApplyPointMap
  ↓
Driver.run()
  ↓
PointValue[]
  ↓
DataBatch
  ↓
Driver SDK / IPC
  ↓
Snapshot
```

当前这一主链继续保留。

## 1.2 当前 Core 数据契约

`crates/core-types/src/lib.rs` 已存在：

```text
TimestampNs = i64
DataType
Value
Quality
PointValue
DataBatch
TaskMode
DriverBinding
AcquisitionTask
PointDescriptor
PointDefinition
PointMap
```

当前：

```text
Quality:
GOOD
UNCERTAIN
BAD
```

业务时间：

```text
UTC Unix ns
```

性能时间：

```text
host_mono_ns()
```

仅用于：

```text
IPC / E2E latency
```

禁止作为业务时间、设备时间或持久化时间。

## 1.3 当前 DriverBinding

当前核心原则继续冻结：

```text
Core 只保存 DriverBinding.kind + config
Core 不解释协议语义
Driver 自己解释配置
```

后续 Descriptor / ResourceSelection 不能破坏这一边界。

## 1.4 当前 Point Registry

ConfigStore 当前已经实现：

```text
point_key -> stable point_id
```

唯一域：

```text
(endpoint_id, point_key)
```

删除点位：

```text
deleted = 1
```

不会删除记录，也不会复用原 `point_id`。

相同 `point_key` 重新出现：

```text
恢复原 point_id
deleted -> 0
```

因此本方案不是重新设计 Point Registry，而是把当前行为升级为正式契约。

## 1.5 当前 Config Revision

当前 `replace_tasks()`：

```text
BEGIN transaction
↓
replace task snapshot
↓
revision + 1
↓
COMMIT
```

失败：

```text
ROLLBACK
revision 不变化
```

这一语义继续冻结。

## 1.6 当前 Driver Runtime

现有 Endpoint 生命周期：

```text
spawn
 ↓
handshake
 ↓
OpenConnection
 ↓
ConfigureTasks
 ↓
PointDescriptors
 ↓
ApplyPointMap
 ↓
StartConnection
 ↓
DataBatch / State / Error
```

当前已经具备：

```text
Driver Process isolation
TCP + Protobuf IPC
session token
heartbeat
reconnect/backoff
configuration failure isolation
Point ID persistence
revision
stream_epoch
Latest-Wins
Snapshot
```

后续动态管理面不得破坏这些能力。

## 1.7 当前 Driver SDK

当前 Driver 开发者主要实现：

```rust
trait Driver {
    fn metadata(&self) -> DriverMetadata;

    async fn open_connection(
        &self,
        endpoint_id: &str,
        config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError>;
}
```

以及：

```rust
trait DriverConnection {
    async fn configure(...);
    async fn apply_point_map(...);
    async fn run(...);
}
```

后续继续坚持：

> 一个主要 Driver Trait + 一个 Connection Trait + 少量带默认 `Unsupported` 的可选方法。

禁止 Trait Explosion。

---

# 2. 开工 Gate 与优先级

任何 Descriptor / Generic UI 大规模开发之前，必须依次完成：

## P0 — 阻塞施工

```text
P0.1 Point / Data Plane 语义冻结
P0.2 Descriptor / IPC / Driver 版本契约
P0.3 Secret / Certificate / Control 安全契约
```

P0 未完成时，不允许宣布：

```text
Descriptor Contract Frozen
Runtime Baseline Frozen
Control Contract Ready
```

## P1 — 阻塞阶段验收

```text
P1.1 ConfigStore Migration V2
P1.2 Management / Data IPC Queue Isolation
P1.3 Configure / Runtime 性能预算
```

## P2 — 工程完备

```text
P2.1 Diagnostics / Runbook
P2.2 DeviceProfile / Preset / i18n / ImportIssue
```

---

# 3. P0.1：Point / Data Plane 冻结契约

这是后续所有 Descriptor Output、Resource 和控制逻辑的基础。

## 3.1 DataType

继续使用现有 `DataType`。

V1 冻结：

```text
bool
i32
u32
i64
u64
f32
f64
string
bytes
datetime

bool[]
i32[]
u32[]
i64[]
u64[]
f32[]
f64[]
string[]
datetime[]
```

Descriptor Output 的 `data_type` 必须来自这一枚举。

禁止 Driver Descriptor 自定义任意数据类型字符串。

未来新增 DataType 必须同时触发：

```text
Core DataType review
IPC compatibility review
Descriptor contract review
REST/Frontend rendering review
```

## 3.2 OutputDescriptor

Resource Output 最小契约：

```rust
pub struct OutputDescriptor {
    pub id: String,
    pub label: LocalizedText,

    pub data_type: DataType,
    pub unit: Option<String>,

    pub access: AccessMode,

    /// 仅用于诊断/UI 说明，不作为运行期表达式。
    pub quality_codes: Vec<QualityCodeDescriptor>,
}
```

AccessMode：

```text
Read
Write
ReadWrite
```

Command 不通过 AccessMode 表达。

## 3.3 不引入任意 Quality DSL

不允许：

```text
JavaScript quality rule
自定义表达式脚本
复杂 DSL
```

Quality 使用 Mesa 全局契约：

```text
GOOD
UNCERTAIN
BAD
```

Driver 只负责：

```text
Protocol status
→ Mesa Quality + quality_code
```

## 3.4 GOOD

GOOD 时：

```text
value 必须存在
value 类型必须 == OutputDescriptor.data_type
value 具有当前业务有效性
```

示例：

```text
Output = I32
GOOD + I32(100)       ✅
GOOD + String("100") ❌ Contract Violation
```

## 3.5 UNCERTAIN

只允许在协议原生语义明确提供 uncertain 时使用。

Driver 不允许因为：

```text
“我不确定”
“重试过一次”
“数值看起来异常”
```

自行生成 UNCERTAIN。

## 3.6 BAD

BAD 表示：

```text
当前 Point 不可作为有效当前测量值使用
```

但是为了保持 TypedValue 契约：

```text
BAD Point 仍必须携带与 data_type 匹配的 Value
```

规则：

1. 优先保留该 Point 的 last-known typed value；
2. 没有 last-known value 时，允许使用 type-compatible neutral value；
3. `quality=BAD` 时消费者不得把 `value` 当有效当前测量；
4. UI 应显示 `--` / Error 状态，而不是 neutral value；
5. `quality_code` 必须表达错误原因。

禁止：

```text
I32 Point
→ BAD
→ Value::String("ERR:EW_xxx")
```

正确：

```text
value        I32 typed value
quality      BAD
quality_code EW_xxx
```

## 3.7 QualityCode

Mesa Core 预留公共原因码：

```text
COMMUNICATION_LOST
TIMEOUT
ADDRESS_INVALID
DECODE_FAILED
TYPE_MISMATCH
UNSUPPORTED
DEVICE_ERROR
NO_CURRENT_VALUE
```

协议错误：

```text
FOCAS EW_*
OPC UA StatusCode
Modbus exception
S7 error
```

可以映射为：

```text
quality = BAD
quality_code = protocol/native code
```

详细文本进入 Diagnostics/Event，不塞入 Typed Value。

## 3.8 时间戳契约

Mesa 明确区分三个时间。

### DataBatch.timestamp_ns

语义：

```text
Driver 完成本批采集，
或收到该批 Subscription Notification 的宿主 UTC Unix ns
```

单位：

```text
ns
```

时区：

```text
UTC
```

### PointValue.source_timestamp_ns

语义：

```text
设备或协议真正提供的源时间
```

只有协议真实提供时填写。

没有设备时间：

```text
None
```

禁止：

```text
用 DataBatch.timestamp_ns 冒充 source timestamp
```

### DataBatch.mono_ns

仅用于：

```text
Driver publish
→ Core receive
→ IPC/E2E latency
```

禁止：

```text
持久化为业务时间
UI 作为采样时间显示
跨宿主比较
与 UTC 相减
```

Windows QPC 文档统一描述为：

```text
宿主公共 QPC counter domain
仅用于同宿主差值
不代表 UTC 或业务时间
```

## 3.9 Point Key 唯一域

正式冻结：

```text
point_key unique within endpoint_id
```

完整唯一键：

```text
(endpoint_id, point_key)
```

不同 Endpoint 允许拥有相同 point_key。

## 3.10 Point ID 稳定性

冻结当前行为：

```text
已有 point_key
→ 始终复用原 point_id
```

删除：

```text
deleted = 1
```

禁止：

```text
删除 registry row
复用被删除 ID
```

重新加入：

```text
same point_key
→ restore original point_id
```

## 3.11 断线语义

正式契约：

```text
Endpoint communication lost
        ↓
所有已知 Latest Point
        ↓
quality = BAD
quality_code = COMMUNICATION_LOST
        ↓
保留最后一个 typed value
保留最后一次真实采样 timestamp
```

断线本身：

```text
不是一次新采样
```

因此禁止：

```text
生成虚假 timestamp
把 value 设置为 null
把 timestamp 更新为断线时间
```

### 当前代码阻塞项

当前 `Snapshot::mark_communication_lost()` 实际会将 value 改成 `null`。

必须修正为：

```text
保留 typed last value
quality = BAD
quality_code = COMMUNICATION_LOST
原 timestamp 不变
```

此项修复后，才能标记：

```text
Data Plane Semantics Frozen
```

## 3.12 同一 Operation 点级 BAD

例如：

```text
FOCAS Dynamic
├── Feed GOOD
├── Spindle GOOD
├── Program BAD
└── Position GOOD
```

这是允许且推荐的。

契约：

```text
一个 Output BAD
≠ 整个 Operation BAD
```

只有底层 Operation 完全失败且无法提取任何有效结果时，才允许全部 outputs BAD。

## 3.13 Backpressure

继续冻结：

```text
Data Plane = Latest-Wins
```

同一个 point_id 积压时：

```text
只保留最新值
```

sequence：

```text
允许缺口
```

缺口：

```text
表示背压合并
不表示设备时间倒退
```

Management / Control Plane 禁止使用 Latest-Wins。

## 3.14 P0.1 自动化测试

新增：

```text
tests/driver-contract/tests/data_semantics.rs
```

必须断言：

```text
GOOD value type matches descriptor
BAD value type still matches descriptor
point_key duplicate rejected
same point_key restores same point_id
deleted point ID never reused
re-add tombstone restores same ID
revision success +1
revision failure unchanged
disconnect preserves typed last value
disconnect keeps original timestamp
disconnect sets BAD/COMMUNICATION_LOST
one output BAD does not poison sibling outputs
```

---

# 4. P0.2：版本与兼容性契约

Mesa 存在三个独立版本：

```text
Driver Package Version
Descriptor Contract Version
Driver IPC Protocol Version
```

禁止混为一个版本。

## 4.1 Driver Package Version

继续使用：

```text
driver.toml.version
```

SemVer：

```text
MAJOR.MINOR.PATCH
```

Driver 二进制、Descriptor、Profile 发生正式对外行为变化时必须 bump version。

正式发布禁止：

```text
同 version 替换为不同 descriptor 语义
```

## 4.2 Descriptor Contract

```text
contract_major
contract_minor
```

### Major 必须增加

以下任意变化：

```text
删除已有 Field
删除已有 Resource
删除已有 Output
删除已有 Command

修改已有 Field Type
修改已有 Output DataType

Optional -> Required
缩小已有 Enum 合法范围

修改已有 Resource ID 的含义
修改已有 Output ID 的含义
修改已有 Command ID 的含义

修改 ResourceSelection 解释后导致旧配置产生不同 Point
```

### Minor 可以增加

```text
新增 optional Field
新增 Resource
新增 Output
新增 Command
新增 Enum Option
新增 UI Hint
新增 optional metadata
新增 optional quality code description
```

旧消费者忽略未知 optional 字段后必须继续工作。

## 4.3 IPC Protocol

沿用当前规则：

```text
protocol_major 不一致
→ Handshake reject
```

同 Major：

```text
新增 optional field / message
→ protocol_minor 演进
```

旧端无法安全解释新行为时：

```text
必须提升 protocol_major
```

## 4.4 GetDescriptor 契约

流程：

```text
Handshake
 ↓
GetDescriptor
 ↓
DescriptorReport
```

限制：

```text
timeout = 5s
max descriptor_json = 256 KiB UTF-8
```

失败码：

```text
DRIVER_DESCRIPTOR_TIMEOUT
DRIVER_DESCRIPTOR_TOO_LARGE
DRIVER_DESCRIPTOR_INVALID_JSON
DESCRIPTOR_CONTRACT_UNSUPPORTED
DRIVER_DESCRIPTOR_VALIDATION_FAILED
```

Management API：

```text
HTTP 503 DriverUnavailable
```

并返回结构化 DriverIssue / ValidationIssue。

## 4.5 Descriptor Cache

Milestone A 不将 Descriptor Cache 作为 ConfigStore 真值。

V1 使用：

```text
Core in-memory cache
```

Cache Key：

```text
(driver_id, driver_version)
```

精确失效：

1. `DriverManager.rescan()`：清空全部 Descriptor Cache；
2. manifest version 变化：新 Cache Key；
3. Core restart：重新获取；
4. 同 version 本地二进制被替换：必须 rescan；
5. 正式发布禁止同 version 修改 Descriptor。

因此 V2 初期不增加：

```text
descriptor_cache SQLite table
```

避免 Derived State 与 Package State 形成双真值。

未来如果确有：

```text
Driver 不可启动时仍显示 last-known Descriptor
```

再单独设计 persistent stale cache。

## 4.6 Legacy Binding 兼容

至少并存：

```text
legacy:
s7.data-block
focas.data-block
opcua.*

new:
mesa.resources.v1
```

Driver 内部负责 Legacy Parser。

迁移方式：

```text
Legacy Binding
      ↓
normalize
      ↓
internal ResourceSelection / Plan representation
```

Core 禁止理解 FOCAS/S7 Legacy 语义。

## 4.7 Binding Migration Tool

建议新增：

```text
apps/mesa-cli
```

命令：

```bash
mesa migrate --db mesa.db --dry-run

mesa bindings migrate --endpoint <id> --dry-run
mesa bindings migrate --endpoint <id> --apply
```

`--dry-run` 输出：

```text
旧 binding
目标 resource
point_key 是否变化
data_type 是否变化
预计 revision
warnings
```

如果：

```text
point_key 变化
data_type 变化
resource 无法确定
```

默认拒绝自动 Apply。

## 4.8 P0.2 自动化测试

```text
descriptor minor additive compatibility
descriptor major reject
required field addition -> incompatible
output data_type change -> incompatible
GetDescriptor timeout
GetDescriptor >256KiB
invalid JSON
invalid field reference
rescan cache invalidation
driver version cache invalidation
legacy binding normalize
migration dry-run no DB mutation
migration failed apply rollback
```

---

# 5. P0.3：Secret / Certificate / Control 安全契约

当前 REST 绑定 loopback 是重要安全边界，但不能因此允许 Secret 明文持久化或设备控制无审计。

## 5.1 Secret Field

Descriptor：

```text
FieldType::Secret
```

Secret 值禁止直接作为普通：

```text
Endpoint.connection_json
```

明文持久化。

## 5.2 SecretStore

新增：

```rust
trait SecretStore {
    fn put(...);
    fn get(...);
    fn delete(...);
    fn list_refs(...);
}
```

Core 持久化：

```text
SecretRef
```

而不是 Secret Plaintext。

## 5.3 Secret 加密要求

契约不强绑某个密码库名称。

必须满足：

```text
AEAD encryption at rest
unique nonce per secret
master key not stored in same DB row
master key filesystem/OS permission protected
key_id recorded
support future key rotation
```

首个实现可选：

```text
XChaCha20-Poly1305
AES-256-GCM
```

## 5.4 Secret REST 规则

REST GET 永远不得返回明文。

返回示例：

```json
{
  "password": {
    "secret_set": true
  }
}
```

禁止使用：

```text
"******"
```

作为持久化 sentinel。

Update 规则：

```text
字段缺失
→ 保留旧 Secret

提供新 plaintext
→ 更新 Secret

显式 clear_secret
→ 删除 Secret
```

## 5.5 Driver 获取 Secret

运行 Endpoint：

```text
Endpoint stored config
      ↓
resolve SecretRef
      ↓
temporary in-memory runtime config
      ↓
OpenConnection IPC
```

Driver 可以在本地子进程内使用连接凭据。

禁止：

```text
日志打印
REST 回显
Diagnostics 返回
Audit 明文记录
```

## 5.6 CertificateRef

当前 OPC UA CertStore 为共享 PKI，而非 endpoint 私有目录。

因此 V1：

```text
CertificateRef = opaque certificate ID / thumbprint
```

UI/Core 不暴露真实文件路径。

## 5.7 Certificate 生命周期

必须支持：

```text
import
list
trust
rotate
delete
diagnostics
```

证书被 Endpoint/Profile 引用时：

```text
delete -> CONFLICT
```

除非显式解除引用。

## 5.8 Control Authorization

不立即建设完整多用户 RBAC。

先冻结授权接口：

```text
scope = control:execute
```

V1 本地实现：

```text
API 仅 loopback
Control 默认 disabled
启动时显式 --enable-control
本地 PolicyProvider 授予 control:execute
```

未来加入用户系统时替换 PolicyProvider，不修改 Driver Control Contract。

## 5.9 Actor

所有 Write / Command 审计必须包含：

```text
actor
```

V1：

```text
local-console
local-api
system
```

未来：

```text
user:<id>
service:<id>
```

## 5.10 P0.3 自动化测试

```text
Secret never appears in GET endpoint
Secret never appears in diagnostics
Secret never appears in logs
runtime can resolve secret
clear secret works
wrong master key fails closed
certificate delete while referenced -> conflict
control disabled by default
control without scope -> forbidden
control audit always contains actor
```

---

# 6. P1.1：ConfigStore Migration V2

当前已有 `SCHEMA_VERSION=1` 和 `meta.schema_version`，但下一阶段需要真正的增量 migration runner。

## 6.1 Migration 目录

新增：

```text
crates/config-store/migrations/
├── 001_initial.sql
└── 002_management_control.sql
```

## 6.2 schema_migrations

```sql
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    checksum TEXT NOT NULL,
    applied_at_ns INTEGER NOT NULL
);
```

## 6.3 Migration 原子性

每个 Migration：

```text
BEGIN IMMEDIATE
 ↓
apply SQL
 ↓
validation
 ↓
record schema_migrations
 ↓
update meta.schema_version
 ↓
COMMIT
```

失败：

```text
ROLLBACK
```

## 6.4 Migration Backup

正式迁移前使用：

```text
SQLite Backup API
```

生成：

```text
mesa.db.bak.<timestamp>
```

禁止把运行中的 DB 普通文件复制作为唯一备份策略。

## 6.5 endpoint_secrets

若首个 SecretStore 使用 SQLite encrypted blob：

```sql
CREATE TABLE endpoint_secrets (
    endpoint_id TEXT NOT NULL
        REFERENCES endpoints(id) ON DELETE CASCADE,

    field_path TEXT NOT NULL,

    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,

    algorithm TEXT NOT NULL,
    key_id TEXT NOT NULL,

    updated_at_ns INTEGER NOT NULL,

    PRIMARY KEY(endpoint_id, field_path)
);

CREATE INDEX idx_endpoint_secrets_endpoint
ON endpoint_secrets(endpoint_id);
```

数据库只存 ciphertext。

## 6.6 control_audit

```sql
CREATE TABLE control_audit (
    request_id TEXT PRIMARY KEY,

    endpoint_id TEXT NOT NULL,

    actor TEXT NOT NULL,

    operation_type TEXT NOT NULL
        CHECK(operation_type IN ('write', 'command')),

    operation_id TEXT NOT NULL,

    request_json TEXT NOT NULL,
    result_json TEXT,

    status TEXT NOT NULL,

    started_at_ns INTEGER NOT NULL,
    finished_at_ns INTEGER
);

CREATE INDEX idx_control_audit_endpoint_time
ON control_audit(endpoint_id, started_at_ns DESC);

CREATE INDEX idx_control_audit_status_time
ON control_audit(status, started_at_ns DESC);
```

`control_audit` 不使用：

```text
ON DELETE CASCADE endpoint
```

原因：

```text
Endpoint 删除后历史控制审计仍然必须保留
```

## 6.7 DeviceProfile 不进 V2 DB

V1 Profile 继续作为 Driver Package 资产：

```text
drivers/<driver>/profiles/*.json
```

Device 只保存：

```text
profile_id
```

现有 `devices.profile` 可继续使用。

## 6.8 Descriptor Cache 不进 V2 DB

Milestone A 使用 in-memory derived cache。

不新增：

```text
descriptor_cache
```

避免 stale derived state 成为第二真值。

## 6.9 Migration CLI

```bash
mesa migrate --db mesa.db --dry-run
```

必须输出：

```text
current schema
target schema
pending migrations
checksum
backup plan
```

`--dry-run` 不允许修改业务数据。

---

# 7. P1.2：IPC Queue 与运行时隔离契约

当前 Driver SDK：

```text
单 outbound queue
capacity = 256
```

Data：

```text
try_send + Latest-Wins
```

Control：

```text
reliable send
```

未来加入真实 Write / Command 前，需要防止 Control 被 Data backlog 延迟。

## 7.1 队列分离

目标：

```text
Control Queue
capacity = 32

Data Queue
capacity = 256
```

保留当前 Data Capacity 256 作为性能 baseline。

不要未经 benchmark 直接改为 1024。

## 7.2 Writer 优先级

Writer：

```text
biased select

Control Queue
优先于
Data Queue
```

Control：

```text
永不 Latest-Wins
永不静默丢弃
队列满时等待
达到 timeout 后明确返回失败
```

Data：

```text
Latest-Wins
capacity = 256
```

## 7.3 Queue Metrics

增加：

```text
control_queue_depth
control_queue_capacity
control_enqueue_wait_ms

data_queue_depth
data_queue_capacity
data_coalesced_points_total
data_coalesced_batches_total
```

## 7.4 Management RPC

`GetDescriptor`、Probe、Browse、Write、Command 属于可靠控制/管理 RPC。

`GetDescriptor`：

```text
5s timeout
256KiB response cap
```

---

# 8. P1.3：性能预算

所有绝对性能门槛必须记录测试机器信息。

Benchmark 输出必须包含：

```text
OS
CPU
logical cores
RAM
Rust version
build profile
git SHA
```

## 8.1 Configure Compile Budget

测试：

```text
ResourceSelection
↓
Driver.configure()
↓
AcquisitionPlan
```

目标：

| Point 数 | p95 |
|---:|---:|
| 1,000 | ≤ 100 ms |
| 10,000 | ≤ 1 s |
| 50,000 | ≤ 5 s |

每组：

```text
3 warmup
20 measured runs
```

## 8.2 Configure Memory Budget

50K point compile 完成：

```text
Driver process RSS delta <= 256 MiB
```

若某协议由于超大 Symbol/Node metadata 合理超出：

```text
必须单独提交 Performance ADR
```

## 8.3 Runtime Budget

继续当前硬目标：

```text
>= 50K point updates/s
```

IPC：

```text
p95 <= 20 ms
p99 <= 50 ms
```

Descriptor / Resource 改造后：

```text
runtime throughput regression < 5%
```

超过 5%：

```text
Performance Review Required
```

## 8.4 Descriptor Budget

Descriptor parser/validator：

```text
warm p95 <= 50 ms
```

Cold fetch：

```text
总 RPC timeout = 5s
```

Descriptor JSON：

```text
<= 256 KiB
```

---

# 9. P2.1：Diagnostics 契约

新增：

```http
GET /api/v1/endpoints/{id}/diagnostics
```

返回至少：

```text
endpoint_id
driver_id
driver_version

descriptor_state
descriptor_contract
descriptor_last_error

profile_id
profile_state
profile_last_error

connection_state
last_connected_at_ns
last_disconnect_at_ns

reconnect_attempt_total
current_backoff_ms
last_connection_error

last_probe_at_ns
last_probe_result

point_count

data_queue_depth
data_coalesced_points_total

control_queue_depth

ipc_p50_ms
ipc_p95_ms
ipc_p99_ms

snapshot_apply_p95_ms
```

## 9.1 Descriptor State

```text
Unknown
Loading
Ready
Error
Unsupported
```

未来引入 persistent stale cache 后才考虑：

```text
Stale
```

## 9.2 Profile State

```text
None
Loaded
NotFound
Invalid
Mismatch
```

UI 必须可以区分：

```text
Driver Error
Descriptor Error
Profile Error
Connection Error
Device Error
```

## 9.3 Control Audit API

```http
GET /api/v1/control/audit
GET /api/v1/control/audit/{request_id}
```

支持：

```text
endpoint_id
status
operation_type
time range
limit/cursor
```

## 9.4 Runbook

新增：

```text
docs/runbook/
├── driver-unavailable.md
├── descriptor-invalid.md
├── endpoint-reconnect-loop.md
├── point-bad.md
├── profile-invalid.md
├── probe-failed.md
├── control-timeout.md
└── database-migration-failed.md
```

每篇包含：

```text
现象
Diagnostics 字段
可能原因
排查命令/API
恢复步骤
是否影响其它 Endpoint
```

---

# 10. P2.2：DeviceProfile 精确契约

Profile 不允许任意脚本。

## 10.1 Profile Schema 示例

```json
{
  "profile_version": 1,

  "id": "fanuc-0i-f-plus",
  "version": "1.0.0",

  "vendor": "FANUC",
  "family": "0i",
  "model": "0i-F Plus",

  "driver_id": "focas2",

  "match_rules": [
    {
      "field": "driver_id",
      "op": "eq",
      "value": "focas2"
    },
    {
      "field": "probe.model",
      "op": "eq",
      "value": "0i-F Plus"
    }
  ],

  "connection_defaults": {
    "port": 8193,
    "timeout_ms": 3000
  },

  "rate_classes": {
    "realtime": 100,
    "normal": 1000,
    "slow": 10000
  },

  "presets": [
    {
      "id": "basic",
      "label": {
        "default": "Basic",
        "zh-CN": "基础数据"
      },
      "selections": [
        {
          "resource_id": "dynamic",
          "parameters": {
            "axis": 1
          },
          "rate_class": "realtime",
          "outputs": [
            {
              "output": "feed",
              "point_key": "machine.feed"
            },
            {
              "output": "spindle.speed",
              "point_key": "machine.spindle_speed"
            }
          ]
        }
      ]
    }
  ]
}
```

## 10.2 MatchRule

V1 Field 白名单：

```text
driver_id
probe.vendor
probe.family
probe.model
probe.firmware
```

V1 Operator：

```text
eq
in
prefix
```

暂不支持：

```text
JavaScript
arbitrary JSONPath
regex
```

## 10.3 Profile 匹配

所有 rules 满足：

```text
Profile matched
```

多个 Profile 匹配：

1. model 精确匹配优先；
2. family 匹配次之；
3. 仍并列时返回候选，不自动偷偷选择。

## 10.4 Preset 展开

Preset：

```text
Selection[]
```

展开为：

```text
ResourceSelection[]
```

再根据：

```text
(mode, interval_ms)
```

稳定分组为 AcquisitionTask。

默认自动 Task ID：

```text
auto-poll-100
auto-poll-1000
auto-poll-10000
```

同 interval：

```text
只生成一个 auto Task
```

手工 Task 不与 auto Task 隐式合并。

## 10.5 Point Key 稳定

Preset 中必须明确 `point_key`。

Profile 升级不得无故修改已有 Point Key。

修改 Point Key：

```text
Profile MAJOR version change
+ Migration warning
```

## 10.6 i18n

稳定 ID 永远不翻译：

```text
dynamic
feed
machine.feed
```

展示文本：

```text
LocalizedText
```

示例：

```json
{
  "default": "Spindle Speed",
  "zh-CN": "主轴转速"
}
```

Fallback：

```text
requested locale
↓
default
↓
stable id
```

## 10.7 Unit

V1 Unit 由：

```text
OutputDescriptor
```

定义。

Profile 不允许随意修改物理单位。

未来如果需要：

```text
mm -> inch
°C -> °F
```

必须新增显式 presentation transform，不通过改字符串单位实现。

---

# 11. ImportIssue 契约

Importer 统一返回：

```rust
pub struct ImportIssue {
    pub severity: IssueSeverity,
    pub code: String,
    pub message: String,

    pub path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}
```

severity：

```text
Warning
Error
```

存在 Error：

```text
Import 不允许 Apply
```

只有 Warning：

```text
允许 Apply，但 UI 必须展示
```

---

# 12. Descriptor Schema 契约

V1 Field：

```text
String
Integer
Number
Boolean
Enum
Secret
Duration
Host
Port
Url
File
CertificateRef
```

Condition：

```json
{
  "field": "security_mode",
  "op": "neq",
  "value": "None"
}
```

支持：

```text
eq
neq
in
```

要求：

```text
condition.field 必须引用同一 Schema 已存在字段
```

禁止跨 Resource 任意依赖和任意表达式执行。

---

# 13. DriverDescriptor

```rust
pub struct DriverDescriptor {
    pub contract_major: u32,
    pub contract_minor: u32,

    pub identity: DriverIdentity,

    pub connection: SchemaDescriptor,

    pub resources: Vec<ResourceDescriptor>,

    pub controls: ControlCatalog,

    pub discovery: DiscoveryCapabilities,

    pub capabilities: DriverCapabilities,
}
```

## 13.1 Driver SDK 修改

当前：

```rust
trait Driver {
    fn metadata(&self) -> DriverMetadata;
    async fn open_connection(...);
}
```

增加：

```rust
fn descriptor(&self) -> DriverDescriptor;
```

最终仍保持一个主 Driver Trait。

---

# 14. Resource 契约

```rust
pub struct ResourceDescriptor {
    pub id: String,
    pub label: LocalizedText,
    pub parameters: SchemaDescriptor,
    pub outputs: Vec<OutputDescriptor>,
    pub modes: Vec<TaskMode>,
}
```

Resource 表示：

```text
用户/管理面的逻辑数据能力
```

不是物理协议 Function。

---

# 15. ResourceSelection 契约

```rust
pub struct ResourceSelection {
    pub resource_id: String,
    pub parameters: serde_json::Value,
    pub outputs: Vec<SelectedOutput>,
}

pub struct SelectedOutput {
    pub output: String,
    pub point_key: String,
}
```

Validation：

```text
resource exists
parameters schema valid
output exists
point_key valid
point_key endpoint unique
```

通用 Binding：

```text
mesa.resources.v1
```

现有 Legacy Binding 至少保留一个 Major Release。

---

# 16. AcquisitionPlan 边界

AcquisitionPlan 必须：

```text
只存在于 Driver 内部
```

Core 不定义：

```text
GenericPhysicalOperation
UniversalIndustrialPlanner
```

Driver `configure()` 负责把 ResourceSelection / Legacy Binding 编译成内部不可变 Plan。

运行时：

```text
tick
↓
execute precompiled plan
```

而不是每 Tick 重新解析 Schema/Resource。

---

# 17. FOCAS 可验收契约

FOCAS 是 Multi-output Resource 的核心验收 Driver。

Descriptor：

```text
Resource dynamic(axis)
Outputs:
feed
spindle.speed
program.current
position.absolute
```

配置：

```text
4 Logical Points
```

configure 后：

```text
1 FocasOperation::Dynamic
```

run：

```text
1 cnc_rddynamic2
```

验收：

```text
physical_call_count == 1
point_count == 4
```

一个 output decode fail：

```text
1 BAD
3 GOOD
```

## 17.1 FOCAS PMC

例如：

```text
R100
R102
...
R118
```

WORD range：

```text
range call == 1
```

Range 失败：

```text
range_calls == 1
single_fallback_calls == N
```

## 17.2 当前 FOCAS P0 修复

任何 BAD 路径禁止：

```text
Value::String("ERR:...")
```

替代声明的数值类型。

必须统一为：

```text
Typed Value
+ BAD
+ quality_code
```

---

# 18. S7 可验收契约

Resource：

```text
memory
```

逻辑点：

```text
DB1.DBW0
DB1.DBW2
DB1.DBD4
```

继续使用当前 Planner：

```text
sort
↓
continuous range merge
↓
PDU planning
↓
fragment/reassemble
↓
read
↓
fan-out
```

验收：

```text
Descriptor/Resource 引入后
现有物理 range count 不增加
现有 fragmentation 不退化
现有 BAD fallback 不退化
```

禁止为了 FOCAS 统一而重写 S7 为 Generic Planner。

---

# 19. OPC UA 可验收契约

Resource：

```text
node
```

支持：

```text
Poll
Subscribe
Browse
```

Browse 必须：

```text
分页
懒加载
搜索
```

选择 100 Nodes：

```text
100 logical points
```

不允许新增：

```text
OPCUA-specific frontend component
```

---

# 20. Discovery / Browse / Import

工业设备配置至少存在：

```text
Manual
Browse
Import
```

三者必须并存。

## 20.1 DiscoveryCapabilities

```rust
pub struct DiscoveryCapabilities {
    pub manual: bool,
    pub browse: bool,
    pub import: bool,
}
```

## 20.2 Browse API

```http
POST /api/v1/endpoints/{id}/browse
```

```rust
BrowseRequest {
    parent,
    filter,
    cursor,
    limit,
}
```

返回：

```rust
BrowseNode {
    id,
    label,
    kind,
    data_type,
    access,
    has_children,
    binding,
    metadata,
}
```

禁止一次返回完整 OPC UA Namespace。

## 20.3 Import

Importer 由 Driver/Profile 声明。

未来包括：

```text
IODD
EDS
DCF
SCL
L5X
ACD
NodeSet
CSV
```

不设计 Universal Industrial Import Format。

---

# 21. Generic Web UI 契约

建议：

```text
apps/mesa-web/
```

技术栈：

```text
React
TypeScript
Vite
```

初期不需要：

```text
Next.js
SSR
Node BFF
Micro Frontend
Electron
```

## 21.1 通用组件

只允许通用组件：

```text
SchemaForm
DeviceWizard
ResourcePicker
OutputSelector
ResourceBrowser
ImportWizard
PointTable
CommandForm
DiagnosticsPanel
```

禁止：

```text
FocasForm.tsx
S7ConnectionForm.tsx
ModbusTagEditor.tsx
```

## 21.2 Add Device 流程

```text
Add Device
   ↓
Vendor / Profile
   ↓
Connection
   ↓
Validate
   ↓
Probe
   ↓
Recommended Data
   ↓
Browse / Import / Manual
   ↓
Update Rate
   ↓
Review
   ↓
Save
```

## 21.3 Update Rate

普通用户看到：

```text
Realtime  100 ms
Normal    1 s
Slow      10 s
Custom
```

内部生成/归并 AcquisitionTask。

## 21.4 Generic UI 零修改契约

Fixture Driver 增加：

```text
新 Connection Field
新 Resource
新 Output
新 Enum
新 optional Command
```

如果均属于 Contract V1：

```text
mesa-web source diff == 0
```

自动化 E2E 必须证明页面自动出现新字段、资源和命令。

这是整个 Descriptor/UI 架构的核心 KPI。

---

# 22. Control Plane 契约

Mesa V1 只定义：

```text
Write
Command
```

不定义：

```text
Universal Machine Operation
Universal Motion Control
```

## 22.1 Write

```rust
WriteRequest {
    request_id,
    target,
    value,
    expected_value,
}
```

适用于：

```text
PLC Variable
Modbus Register
OPC UA Variable
CANopen Parameter
IO-Link Parameter
```

## 22.2 Command

```rust
CommandDescriptor {
    id,
    label,
    input_schema,
    result_schema,
    risk,
    confirmation,
    timeout_ms,
    idempotent,
    readback,
}
```

适用于：

```text
Start Program
Stop Program
Reset Alarm
Servo Enable
Home
Change Mode
```

## 22.3 DriverConnection 扩展

保持一个 Trait：

```rust
trait DriverConnection {
    async fn configure(...);
    async fn apply_point_map(...);
    async fn run(...);

    async fn browse(...) {
        Unsupported
    }

    async fn write(...) {
        Unsupported
    }

    async fn command(...) {
        Unsupported
    }
}
```

## 22.4 CommandResult

至少：

```text
request_id
status
started_at_ns
finished_at_ns
result
error
readback
```

status：

```text
Succeeded
Failed
TimedOut
Rejected
Cancelled
```

## 22.5 控制执行流程

```text
UI
 ↓
Schema Validation
 ↓
Core
 ↓
Authorization / Capability Check
 ↓
Audit STARTED
 ↓
Driver
 ↓
重新验证设备状态
 ↓
执行
 ↓
Read-back
 ↓
CommandResult
 ↓
Audit COMPLETED / FAILED
```

前端按钮状态不能成为安全边界。

## 22.6 FOCAS 控制

初期：

```text
controls = []
```

Control Contract + Simulator 完成后，再逐项启用：

```text
Reset Alarm
Select Program
Start Program
Stop Program
```

禁止直接暴露：

```text
generic write(address, value)
```

作为 CNC 控制模型。

---

# 23. Management API

基于现有 `core-api` 扩展，不增加 Node BFF。

## 23.1 Driver API

```http
GET /api/v1/drivers
GET /api/v1/drivers/{id}
GET /api/v1/drivers/{id}/descriptor
```

## 23.2 Validate

```http
POST /api/v1/drivers/{id}/validate-connection
```

不访问真实设备。

验证：

```text
Schema
required
类型
范围
Driver config parse
静态语义
```

## 23.3 Probe

```http
POST /api/v1/drivers/{id}/probe
```

访问真实设备。

返回：

```text
reachable
device info
detected profile hints
detected capabilities
warnings
```

Probe 不创建永久 Endpoint。

## 23.4 ValidationIssue

```rust
pub struct ValidationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}
```

Generic UI 根据 `path` 显示字段错误。

---

# 24. Protocol Coverage Fixture Matrix

不要求立刻开发所有协议，但必须用 Fixture 防止架构偏向 PLC。

| 模型 | 代表 | Mesa Resource | Control |
|---|---|---|---|
| Memory | S7 | memory | Write |
| Register | Modbus Fixture | register | Write |
| Symbol | EtherNet/IP Fixture | symbol | Write |
| Node | OPC UA | node | Write / Method |
| Function | FOCAS | dynamic/status/etc | Command |
| Object Dictionary | CANopen Fixture | object | Write / Command |
| Device Description | IO-Link Fixture | process/parameter | Write |
| Report/Event | IEC 61850 Fixture | dataset/report | Command |
| REST/SDK | Robot Fixture | resource | Command |

目的：

```text
防止 Mesa 逐步退化成“只适合 PLC 地址型协议”
```

---

# 25. Release Validation Artifact

每个 Release Candidate 自动生成：

```text
release-validation.json
```

至少包含：

```text
git_sha
rust_version
os
cpu
ram

contract_tests
descriptor_tests
migration_tests

configure_1k_p95_ms
configure_10k_p95_ms
configure_50k_p95_ms

throughput_updates_s

ipc_p50_ms
ipc_p95_ms
ipc_p99_ms

snapshot_apply_p95_ms

soak_duration
rss_start_mb
rss_end_mb

real_device_matrix
```

禁止 Release Note 只写：

```text
“性能良好”
“测试通过”
```

必须有机器可读结果。

---

# 26. 阶段化实施计划

## Milestone 0 — Contract Freeze

### 修改

```text
core-types
snapshot
config-store tests
docs
```

### 完成

```text
Point semantics
Quality
timestamps
disconnect semantics
point registry semantics
revision semantics
```

### Gate

```text
data_semantics.rs 全过
snapshot disconnect typed-value bug 修复
FOCAS BAD typed-value 修复
host_mono_ns 文档修正
```

---

## Milestone A — Descriptor Foundation

### 修改

```text
core-types/descriptor.rs
core-types/schema.rs
core-types/resource.rs
core-types/capability.rs

driver-sdk
driver-protocol
driver-manager
core-api

simulator
```

### 完成

```text
DriverDescriptor
SchemaDescriptor
ResourceDescriptor
GetDescriptor
Descriptor validation
Descriptor in-memory cache
REST API
```

### Gate

```text
GetDescriptor <= 256 KiB
GetDescriptor timeout 5s
Descriptor contract tests
Simulator Descriptor
旧 Data Plane regression
```

---

## Milestone B — Existing Driver Adoption

顺序：

```text
Simulator
↓
S7
↓
FOCAS2
↓
OPC UA
```

### Gate

四个 Driver 全部使用同一 Descriptor Contract。

如果无法自然表达：

```text
Descriptor Contract 不得冻结
```

---

## Milestone C — ResourceSelection

### 完成

```text
mesa.resources.v1
Legacy normalize
Resource validation
PointDescriptor generation
```

### Gate

```text
legacy regression
new resource contract
stable point_key
S7 / FOCAS / OPC UA / Simulator 全工作
```

---

## Milestone D — FOCAS Explicit Planner

### 完成

```text
Dynamic Operation
Status Operation
PMC Range
Fallback
multi-output fan-out
```

### Gate

```text
4 outputs -> 1 dynamic call
10 PMC word -> 1 range call
range failure -> point fallback
point BAD isolation
```

---

## Milestone E — Store V2 / Security

### 完成

```text
migration runner
schema_migrations
encrypted endpoint secrets
backup
dry-run
control_audit schema
```

### Gate

```text
migration rollback
backup restore
no plaintext secret
secret redaction
wrong key fails closed
```

---

## Milestone F — Management API

### 完成

```text
Descriptor
Validate
Probe
Diagnostics
ValidationIssue
```

### Gate

```text
Driver field change -> API automatic
no frontend change
```

---

## Milestone G — Generic Web UI

### 完成

```text
SchemaForm
DeviceWizard
ResourcePicker
OutputSelector
PointTable
DiagnosticsPanel
```

### Gate

Fixture Driver：

```text
frontend diff == 0
```

---

## Milestone H — Discovery / Import

### 完成

```text
Browse contract
OPC UA Browse
ImportIssue
Importer framework
```

### Gate

```text
100 OPC UA nodes
generic ResourceBrowser
no OPC UA special frontend component
```

---

## Milestone I — DeviceProfile

### 完成

```text
Profile loader
match_rules
rate_classes
preset expansion
i18n
FANUC profile
Siemens profile
```

### Gate

```text
普通用户不接触 DriverBinding
即可创建设备和选择推荐数据
```

---

## Milestone J — Control Plane

### 完成

```text
Control/Data queue split
Write
Command
PolicyProvider
Audit
Simulator Control
```

### Gate

```text
control disabled default
scope enforced
actor recorded
all error paths covered
control messages never Latest-Wins
```

---

## Milestone K — Real Device Control

按风险由低到高：

```text
OPC UA Write
Modbus Write
S7 Write
FOCAS Selected Commands
CANopen / CiA402
```

每一种能力单独真机验收。

---

# 27. 测试目录目标

```text
tests/
├── driver-contract/
│   ├── protocol_negotiation.rs
│   ├── session_lifecycle.rs
│   ├── data_plane.rs
│   ├── data_semantics.rs
│   ├── fault_tolerance.rs
│   ├── subprocess_recovery.rs
│   ├── descriptor_contract.rs
│   ├── resource_contract.rs
│   ├── discovery_contract.rs
│   └── control_contract.rs
│
├── performance/
│   ├── e2e_50k_real.rs
│   ├── configure_large.rs
│   └── control_latency.rs
│
├── fixtures/
│   ├── generic-register/
│   ├── generic-symbol/
│   ├── generic-function/
│   ├── generic-object-dictionary/
│   └── generic-report/
│
└── web-e2e/
    ├── add-device.spec.ts
    ├── configure-resources.spec.ts
    ├── browse.spec.ts
    ├── profile.spec.ts
    └── control.spec.ts
```

---

# 28. CI Gate

每个 PR：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

Web：

```bash
pnpm lint
pnpm typecheck
pnpm test
pnpm build
```

必须通过：

```text
Contract Tests
Migration Tests（涉及 Store 时）
Simulator E2E
```

---

# 29. Release Gate

Release Candidate：

```text
Windows
Linux

50K Data Plane
Configure Benchmark

Migration dry-run
Migration apply/rollback

24h Soak

Real Device Matrix
```

控制功能 Release：

```text
必须额外完成对应设备的 real-device control validation
```

---

# 30. Soak Test

至少：

```text
24h
72h（Release Candidate 推荐）
```

覆盖：

```text
多个 Endpoint
不同 Polling Interval
OPC UA Subscription
Driver Restart
设备断线
Core Restart
Config Reload
Backpressure
Probe
Descriptor Fetch
```

观察：

```text
RSS
CPU
OS handles
Tokio task count
Queue depth
Reconnect count
Coalesced points
Sequence
IPC latency
```

验收：

```text
无持续 Memory Leak
无 Driver Process Leak
无孤儿子进程
Point ID 稳定
Revision 稳定
Endpoint State 正确
Reconnect 可恢复
UI/Descriptor 不影响采集
```

---

# 31. Definition of Done

一个 Milestone 只有同时满足：

```text
实现完成
Contract Tests 完成
错误路径完成
Regression 完成
Migration 完成（若涉及 Store）
Docs 完成
Diagnostics 完成
Performance 数据完成
CI 完成
```

才能标记：

```text
DONE
```

仅：

```text
代码能编译
```

不能视为完成。

只有静态测试：

```text
不代表真实设备验证通过
```

---

# 32. 需要立即修正的现有代码项

在 Descriptor Milestone A 开始前，先完成：

## P0-A Snapshot communication lost

当前：

```text
BAD + value = null
```

目标：

```text
BAD + COMMUNICATION_LOST
保留 typed last value
保留原 timestamp
```

## P0-B FOCAS BAD TypedValue

当前部分 BAD 路径可能使用：

```text
Value::String("ERR:...")
```

目标：

```text
value 保持 Output data_type
quality = BAD
quality_code = protocol error
```

## P0-C host_mono_ns 文档

Windows QPC 注释统一修改为：

```text
宿主公共 QPC counter domain
仅用于同宿主差值计算
不代表 UTC 或业务时间
```

---

# 33. Driver 开发者体验标准

一个简单 Driver 应只需要理解：

```text
metadata
descriptor
open_connection
configure
run
```

需要时才实现：

```text
browse
write
command
```

不要强迫一个简单温度传感器 Driver 实现它不需要的复杂能力。

SDK 可以提供：

```text
Field::host()
Field::port()
Field::duration()
Field::enum_()

ResourceBuilder
OutputBuilder
CommandBuilder

SchemaValidator
ResourceSelectionParser
```

禁止提供：

```text
UniversalAddress
GenericIndustrialDriver
UniversalProtocol
GenericIndustrialPlanner<T>
```

---

# 34. 明确禁止的过度设计

除非真实需求证明必要，否则禁止：

```text
UniversalAddress
完整 JSON Schema
Generic Industrial Planner
前端 JS / React Driver Plugin
Driver 注入 HTML / CSS / JS
GraphQL
Micro Frontend
Workflow Engine
大量 Driver Traits
Descriptor Persistent Cache
每 Tick 动态构建 AcquisitionPlan
Profile 参与底层协议逻辑
Core 判断具体 Driver 类型
任意 Profile Script
任意 Quality DSL
```

---

# 35. 最终边界定义

```text
Driver
负责协议如何工作。

Descriptor
负责告诉 Mesa：
Driver 如何配置、有哪些能力。

Profile
负责告诉用户：
这是什么设备，以及推荐怎么用。

Resource
负责表达：
设备有哪些逻辑数据能力。

PointBinding
负责：
业务 Point 来自哪个 Resource Output。

AcquisitionTask
负责：
什么时候采集。

AcquisitionPlan
负责：
如何把逻辑需求优化成实际协议操作。

Point
负责：
对外提供稳定 Typed Data。

Write / Command
负责：
统一控制入口，但不统一协议控制语义。

Core
负责：
生命周期、持久化、IPC、Point ID、Revision、数据与控制契约。

UI
负责：
渲染 Descriptor / Profile，永远不解释工业协议。
```

---

# 36. 最终可验收目标

Mesa 的架构成功不是：

```text
“设计很通用”
```

而是可以持续用测试证明：

```text
新增第五个完全不同类型 Driver
→ Core 不修改

新增连接参数
→ Web 不修改

新增 Resource / Output
→ Web 不修改

FOCAS 一个 API 多输出
→ Physical Call 不增加

S7 Descriptor 接入
→ Range/PDU Planner 不退化

设备断线
→ Point 统一 BAD
→ last value/timestamp 语义稳定

Point 删除重加
→ point_id 不变化

Driver 升级/回滚
→ Descriptor cache 行为确定

Secret
→ 不落明文、不回显

Command
→ 可靠队列、权限检查、actor 审计

50K Data Plane
→ throughput 不低于基线
→ Descriptor 开销不进入 hot path
```

只有这些断言长期成立，Mesa 的 Driver 自描述、动态管理和控制架构才算真正建立完成。
