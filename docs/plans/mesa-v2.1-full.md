# Mesa V2.1 全量精密实施计划

> 基线：`master 50a9bfb` / `docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md V2.1（36章/3553行）`
> 分支：`feat/v2.1-impl`（本计划所在分支，**未合并到 master**，需显式指令才合并）
> 性质：施工契约 + 测试契约 + 验收契约的任务化落地；不推翻现有 Runtime/DataPlane
> 约束：`AGENTS.md` 硬性约束（Core不懂协议/有界队列Latest-Wins/point_id稳定+tombstone/UTC ns与单调时钟分离/独立进程孤儿防护/V1只读/全量快照替换等）继续冻结

---

## 0. 已确认决策（用户 2026-08-31 确认）

| 项 | 决策 | 依据 |
|---|---|---|
| **Web 并行度** | Milestone F 前仅 `Vite骨架+SchemaForm纯静态fixture` 并行，F 后全量联调；`openapi.json` 先冻 `GET descriptor / POST validate / POST probe` 的 `503+error.code` 形态，UI 用 `msw` mock | 避免契约未稳返工，贴合 §21.4 零修改 KPI |
| **Secret master key** | 默认 `文件 $DATA/master.key 0600 + key_id版本化 + XChaCha20-Poly1305/AES-256-GCM`，`MESA_MASTER_KEY` 环境变量覆盖；OS keychain 列为 P2 可选 | 与现有 `cert_store` 文件模式一致，适配离线工控机，便于 §6.4 Backup API 恢复 |
| **真机窗口** | 三窗口：`B后烟雾(probe+单点GOOD)` / `D后call-count复核(FOCAS 4→1/PMC 10→1, S7 PDU不退化)` / `J前低风险Control(OPC UA Write先行)` | 兼顾进度与 §29 Release Gate real-device matrix 硬门槛 |

---

## 1. 总览与依赖

### 1.1 四轮推进（§26 + §78-81）

```text
Round 1: Milestone 0 → A → B(入门) → E/F(骨架)   —— 暂停验收：四Driver能否自然表达 (§26 B Gate)
Round 2: C → D → G/H 部分                        —— 暂停验收：通用UI零改 (§21.4)
Round 3: E完整 → 25 Release Artifact → 8 性能/30 Soak
Round 4: J → K 真机控制（逐协议，默认disabled）
```

### 1.2 关键路径

```
P0 语义冻结 (§3) ─┬─→ A Descriptor Foundation ─→ B 四Driver ─→ C ResourceSelection ─→ D FOCAS Planner
                  ├─→ E Store V2/Security ─→ F Management API ─→ G Web UI ─→ H Discovery/Profile
                  └─→ 25 Release Artifact + 8 性能预算 + 30 Soak ─→ J/K Control
```

P0 未完成不得宣布 `Descriptor Contract Frozen / Runtime Baseline Frozen`（§2）。

### 1.3 分支与合并纪律

* 本分支 `feat/v2.1-impl` 持续开发，PR 粒度按 Milestone 切分；
* **禁止直接 push master**，合并需用户显式 `approve + git merge --no-ff`；
* 每个 Milestone 结束打 `tag v2.1-m0 / mA ...` 并生成 `release-validation.json`（§25）。

---

## 2. Milestone 0 — Contract Freeze（§1-3.14, §32）

**目标**：DataPlane 语义可测试冻结，为后续所有 Output/Resource/控制提供锚点。

### 2.1 范围

* §1 基线重申：主链 `Device→Endpoint→AcquisitionTask→Binding→PointDescriptor→PointID→DataBatch→Snapshot` 不变
* §3 P0.1 全部：DataType(§3.1)/OutputDescriptor(§3.2)/Quality DSL禁止(§3.3)/GOOD(§3.4)/UNCERTAIN(§3.5)/BAD typed(§3.6)/QualityCode(§3.7)/时间戳三类(§3.8)/point_key域(§3.9)/point_id+tombstone永不自动GC(§3.10-3.10.1)/断线语义(§3.11)/同Operation点级BAD隔离(§3.12)/背压Latest-Wins(§3.13)

### 2.2 立即修复（§32 P0-A/B/C/D）

| 编号 | 任务 | 文件 | 验收 (§3.14) |
|---|---|---|---|
| P0-A | `Snapshot::mark_communication_lost` : `BAD+null → BAD typed last value + COMMUNICATION_LOST + 原timestamp不变` | `crates/driver-manager/src/snapshot.rs:apply_batch/mark_communication_lost` | `disconnect preserves typed last value / keeps original timestamp / sets BAD/COMMUNICATION_LOST` |
| P0-B | FOCAS BAD 禁 `Value::String("ERR:...")`，统一 `typed + BAD + quality_code=EW_*` | `drivers/focas2/src/lib.rs`, `focas_api.rs`, `native.rs` | `BAD value type still matches descriptor` |
| P0-C | `host_mono_ns()` 注释统一：`宿主公共QPC counter domain，仅同宿主差值，不代表UTC/业务时间` | `crates/core-types/src/lib.rs:host_mono_ns`, `crates/driver-sdk/src/lib.rs`, `proto` 注释 | 文档一致性 |
| P0-D | 补 `subprocess_orphan_guard` helper E2E：`wrong token→reject / stdin EOF→exit / helper Core被杀→Driver消失 (Linux PDEATHSIG / Win JobObject)` | 新 `tests/driver-contract/tests/subprocess_orphan_guard.rs`, `crates/driver-manager/src/process.rs:job`, `crates/driver-sdk/src/lib.rs:spawn_parent_liveness_guard` | §1.6.1 四断言 |

### 2.3 新增测试

`tests/driver-contract/tests/data_semantics.rs` 14 断言（§3.14）：
`GOOD type matches / BAD type matches / point_key duplicate rejected / same key restores same ID / deleted ID never reused / re-add restores / revision +1 / revision unchanged on failure / disconnect typed last value / timestamp unchanged / BAD/COMMUNICATION_LOST / one BAD不污染兄弟 / automatic tombstone GC does not exist`

### 2.4 Gate

`cargo test --workspace` + `data_semantics.rs` 14项全过，方可标记 `Data Plane Semantics Frozen`。

---

## 3. Milestone A — Descriptor Foundation（§4, §12-14, §23.1）

### 3.1 新增文件

```
crates/core-types/src/descriptor.rs   # FieldDescriptor{key,label,field_type,required,default,validation,ui:UiHints}
crates/core-types/src/schema.rs       # SchemaDescriptor + validate()
crates/core-types/src/resource.rs     # ResourceDescriptor{id,label,parameters:Schema,outputs,modes}
crates/core-types/src/capability.rs   # DriverCapabilities/DiscoveryCapabilities/ControlCatalog
```

§6 建议：不再塞 `lib.rs`，`lib.rs` 仅 re-export。

### 3.2 Schema V1（§12）

`FieldType=String/Integer/Number/Boolean/Enum/Secret/Duration/Host/Port/Url/File/CertificateRef`；`UiHints{group,order,placeholder,advanced,visible_if}`；`visible_if` 仅 `eq/neq/in` 且 `condition.field` 必须同Schema存在，禁跨Resource表达式。

### 3.3 DriverDescriptor（§13）

```rust
pub struct DriverDescriptor {
  pub contract_major: u32, pub contract_minor: u32,
  pub identity: DriverIdentity,
  pub connection: SchemaDescriptor,
  pub resources: Vec<ResourceDescriptor>,
  pub controls: ControlCatalog,
  pub discovery: DiscoveryCapabilities,
  pub capabilities: DriverCapabilities,
}
```

SDK：`trait Driver { fn descriptor(&self)->DriverDescriptor }` 单Trait，不增 `SchemaProvider` 等（§10）。

### 3.4 IPC（§4.3-4.4）

真值 `proto/driver.proto`（`crates/driver-protocol/build.rs` 生成 `crates/driver-protocol`）：

```protobuf
message GetDescriptor {}
message DescriptorReport { uint32 contract_major=1; uint32 contract_minor=2; string descriptor_json=3; }
```

限制 `timeout 5s / max 256 KiB UTF-8`；REST 统一 `HTTP 503` + 6码表（§4.4）：

| code | 含义 |
|---|---|
| DRIVER_DESCRIPTOR_TIMEOUT | 5s未返回 |
| DRIVER_DESCRIPTOR_TOO_LARGE | >256KiB |
| DRIVER_DESCRIPTOR_INVALID_JSON | 解析失败 |
| DESCRIPTOR_CONTRACT_UNSUPPORTED | Major不受Core支持 |
| DRIVER_DESCRIPTOR_VALIDATION_FAILED | 内容违反Contract |
| DRIVER_UNAVAILABLE | 无法启动/握手失败/可执行缺失 |

形态：`{error:{code,message,issues:[{path,code,message}]}}`，测试必须断言 `code` 精确。

### 3.5 Cache（§4.5）

`Core in-memory (driver_id,driver_version)`；失效：`rescan()/manifest version变化/Core重启/同version二进制替换`；**不建 `descriptor_cache` 表**（§6.8 防双真值，未来 stale cache 另议）。

### 3.6 REST（§23.1）

```
GET /api/v1/drivers
GET /api/v1/drivers/{id}/descriptor   # 懒加载：临时spawn→handshake→GetDescriptor→cache→shutdown
```

`driver-manager::manager.rs` 新增 `get_descriptor()`，`core-api/src/lib.rs` 路由仅 loopback。

### 3.7 测试

`descriptor_contract.rs + schema_validation.rs`：`contract version / field key唯一 / resource/output/command唯一 / enum唯一 / default类型正确 / visible_if引用存在` + synthetic全类型覆盖 + 旧DataPlane回归。

### 3.8 Gate

`GetDescriptor ≤256KiB / 5s / 503精确码 / Simulator Descriptor可序列化不参与热路径` 且 `e2e_50k` 不回归（§11）。

---

## 4. Milestone B — 四Driver Adoption（§15-19）

**顺序** `Simulator → S7 → FOCAS2 → OPC UA`（§14），同一Contract。

| Driver | Descriptor 要点 | 保留的运行时能力 |
|---|---|---|
| **Simulator** (§15) | `connection{seed,points,frequency,failure_mode} / resources{counter,sine,random,constant}/outputs{value}` | 作为 Generic UI/Contract 测试Driver |
| **S7** (§16,18) | `connection{host,port,rack,slot,timeout} / resource memory{area,db,offset,data_type,bit,length}` | `merge_paired_to_bulks_with_max / PDU negotiation / fragment/reassemble / BAD fallback` 保留，不参与Descriptor |
| **FOCAS2** (§17) | `resources ≥9: dynamic/status/axis/spindle/pmc/macro/parameter/diagnosis/alarm/program`，`dynamic(axis)→feed/spindle.speed/program.current/position.absolute` 展示 **1 Resource多Outputs** | 后续 Milestone D 的 Planner 基础 |
| **OPC UA** (§19) | `connection{endpointUrl,policy,mode,auth,CertificateRef} + resource node{NodeId,Attribute} + capabilities{poll,subscribe,browse,write,method}` | 订阅与Browse分离 |

测试：各 `descriptor()合法/可序列化/唯一性/default合法` + `Descriptor改动不影响旧binding回归`。
Gate：§19 `100%不改前端即可获取连接参数与数据能力`，否则契约不冻结。

---

## 5. Milestone C — ResourceSelection（§15-16）

```rust
pub struct ResourceSelection{ resource_id:String, parameters:Value, outputs:Vec<SelectedOutput{output,point_key}> }
```

新Binding `mesa.resources.v1{selections:[]}`；Core不解析协议含义（§21）；参数由Descriptor定义；Legacy `focas.data-block/s7.data-block/opcua.*` 至少保留1 major，Driver内 `normalize→internal Plan` 兼容；提供：

```bash
apps/mesa-cli: mesa bindings migrate --endpoint <id> --dry-run/--apply  # dry-run列 point_key/data_type变化，变化则拒Apply (§4.7)
```

Gate：`legacy regression + resource_contract + stable point_key + S7/FOCAS/OPC UA/Sim全工作`（§26 C）。

---

## 6. Milestone D — FOCAS Explicit Planner（§16-17）

**边界**（§16）：`AcquisitionPlan` 仅Driver内，禁 `GenericPhysicalOperation`；`configure() compile once → immutable Plan`；`tick → execute plan`，禁每tick重解析Schema。

```rust
enum FocasOperation{ Dynamic{axis,outputs}, Status, Axis, PmcRange{kind,start,count}, Macro... }
struct TaskPlan{ interval_ms, operations:Vec<Operation> }
```

验收：
* `4 outputs(feed/spindle/program/position) → 1 Dynamic → 1 cnc_rddynamic2`（`physical_call==1, point==4, 单output BAD不污染兄弟`）
* `R100,R102..R118 WORD 10 → 1 pmc range_call==1`（§17.1 WORD width*2: `e=119 len= (119-100+1)*2`）
* `range EW_LENGTH → range 1 + single N fallback`，单output BAD隔离
* S7 维持 `sort→range merge→PDU planning→fragment`，`ResourceSelection` 仍编译为现有PointSpec，**不重写为GenericPlanner**（§18），`range/fragmentation/BAD fallback` 不退化

Gate：`logical > physical`，call-count单测（§17, §26 D）。

---

## 7. Milestone E — Store V2 + Security（§5-6）

### 7.1 Migration（§6）

`crates/config-store/migrations/{001_initial.sql,002_management_control.sql}` + `schema_migrations{version,name,checksum,applied_at_ns}`；`BEGIN IMMEDIATE→apply→record→meta.schema_version→COMMIT/ROLLBACK`；迁移前 `SQLite Backup API → mesa.db.bak.<ts>`（禁文件拷贝）。

```sql
-- endpoint_secrets
CREATE TABLE endpoint_secrets(endpoint_id TEXT REFERENCES endpoints(id) ON DELETE CASCADE,
  field_path TEXT, ciphertext BLOB, nonce BLOB, algorithm TEXT, key_id TEXT, updated_at_ns INTEGER,
  PRIMARY KEY(endpoint_id,field_path));
-- control_audit（无CASCADE，Endpoint删审计留存）
CREATE TABLE control_audit(request_id TEXT PK, endpoint_id TEXT, actor TEXT,
  operation_type CHECK IN ('write','command'), operation_id TEXT,
  request_json TEXT, result_json TEXT, status TEXT, started_at_ns INTEGER, finished_at_ns INTEGER);
CREATE INDEX ON control_audit(endpoint_id, started_at_ns DESC);
```

`DeviceProfile` 仍 `drivers/<driver>/profiles/*.json`，`devices.profile`引ID；`descriptor_cache` 不入DB（§6.7-6.8）。

CLI：`mesa migrate --db mesa.db --dry-run` 输出 `current/target/pending/checksum/backup plan`，`--dry-run` 不改业务数据。

### 7.2 Security（§5）

* `FieldType::Secret` → `SecretStore{put/get/delete/list_refs}` 持久 `SecretRef`，`AEAD+unique nonce+master key文件权限+key_id+可轮换`（默认 `XChaCha20-Poly1305`，文件 `$DATA/master.key 0600` + env `MESA_MASTER_KEY` 覆盖，决策见 §0）
* REST `GET` 永不返明文：`{password:{secret_set:true}}`，禁 `"******"` sentinel；Update：缺失保留/新明文更新/clear_secret删除；`OpenConnection` 时内存解析，禁日志/审计明文
* `CertificateRef = opaque ID/thumbprint`（共享PKI §5.6），`import/list/trust/rotate/delete/diagnostics`，被引用时 `delete→409 CONFLICT`
* Auth：`scope=control:execute` 接口冻结，V1 `loopback + 控制默认disabled + --enable-control显式开 + PolicyProvider`，审计必含 `actor=local-console/local-api/system→future user:<id>`

测试：§5.10 9项 `Secret never appears / runtime resolve / clear / wrong key fails closed / cert conflict / disabled by default / scope forbidden / actor recorded`。

---

## 8. Milestone F — Management API（§7, §23-23.4）

**Queue隔离**（§7）：`Data 256 + Control/Management 32`（保256基线，禁无评估改1024）；`writer biased select Control优先`；`Control永不Latest-Wins/静默丢弃，满则等待至timeout明确失败`；新增 `control_queue_depth/capacity/control_enqueue_wait_ms/data_coalesced_*` metrics（§7.3）。

```
GET  /api/v1/drivers /drivers/{id}/descriptor
POST /api/v1/drivers/{id}/validate-connection  # 不触设备：Schema/类型/范围/静态语义 → ValidationIssue[]
POST /api/v1/drivers/{id}/probe                # 触设备：reachable/device info/profile hints/capabilities/warnings，不建永久Endpoint
GET  /api/v1/endpoints/{id}/diagnostics        # §9 至少18字段：driver/version/descriptor_state/contract/error/connection/last_connected/reconnect/backoff/probe/point_count/queue/latency p50/95/99/snapshot p95
GET  /api/v1/control/audit{endpoint_id,status,time range,limit/cursor} + /{request_id}
```

`ValidationIssue{path,code,message}` 按path落字段；`DescriptorState∈{Unknown,Loading,Ready,Error,Unsupported,Stale(未来)}`。

Gate：`Driver增字段→API自动变，前端0改`（§34）。

---

## 9. Milestone G — Generic Web UI（§21）

`apps/mesa-web/` `React+TS+Vite`（无需Next/SSR/BFF），仅通用组件 `SchemaForm/DeviceWizard/ResourcePicker/OutputSelector/ResourceBrowser/ImportWizard/PointTable/CommandForm/DiagnosticsPanel`，禁 `FocasForm.tsx` 等。

流程 `Add Device: Vendor/Profile→Connection→Validate→Probe→Recommended Data→Browse/Import/Manual→Update Rate(Realtime100ms/Normal1s/Slow10s→auto归并AcquisitionTask)→Review→Save`（§21.2-21.3）。

**零修改KPI**（§21.4）：fixture-driver 新增 `field/resource/output/enum/command`，`mesa-web source diff==0` 且E2E自动出现；`web-e2e/add-device, configure-resources, browse, profile, control` 覆盖。

并行策略：F 前仅骨架+fixture静态（msw mock §23形态），F 后全量联调（决策 §0）。

---

## 10. Milestone H — Discovery/Import（§11, §20）

`DiscoveryCapabilities{manual,browse,import}`；

```
POST /api/v1/endpoints/{id}/browse {parent,filter,cursor,limit} → BrowseNode{id,label,kind,data_type,access,has_children,binding,metadata} # 分页懒加载，禁一次返全namespace
POST /api/v1/endpoints/{id}/import # 导入器由Driver/Profile声明：NodeSet/L5X/ACD/SCL/IODD/EDS/DCF/CSV，不设Universal格式
```

`ImportIssue{severity∈{Warning,Error},code,message,path,line,column}`：有Error禁Apply，仅Warning可Apply但UI必须展示。

Gate：`OPC UA 100 nodes→Add→ResourceSelection`，`S7 Manual仍正常`，无协议特定UI（§20）。

---

## 11. Milestone I — DeviceProfile（§10）

`DeviceProfile{id,version,vendor,family,model,driver_id,match_rules,connection_defaults,rate_classes,presets}` 存 `drivers/<driver>/profiles/*.json`，`Device.profile`引ID（例 `fanuc-0i-f-plus` §10.1）。

`MatchRule field∈{driver_id,probe.vendor/family/model/firmware} op∈{eq,in,prefix}` 禁JS/regex；多匹配 `model>family>返回候选`；`Preset→Selection[]→ResourceSelection[]→按(mode,interval)归并auto-poll-{100,1000,10000}`，手工Task不隐式合并；`point_key` 稳定，改则 `Profile MAJOR + warning`；`LocalizedText{default,zh-CN}` fallback `requested→default→id`；`Unit` 由 `OutputDescriptor` 定，禁Profile字符串换算（未来显式transform）。

Gate：不接触Driver/Binding完成设备创建与推荐数据。

---

## 12. Milestone J/K — Control Plane & Real Device（§22, §26 J/K）

**J Control Plane**（解除 §2.1只读限制后）：
```
Write{request_id,target,value,expected_value?}  # PLC/Modbus/OPC UA等参数
Command{id,label,input_schema,result_schema,risk,confirmation,timeout_ms,idempotent,readback} # Start/Stop/Reset/Home等
trait DriverConnection{ write/command/browse → default Unsupported } 单Trait
CommandResult{request_id,status∈{Succeeded,Failed,TimedOut,Rejected,Cancelled},started/finished_at_ns,result,error,readback}
```
`Write/Command` 走可靠Control Queue（禁Latest-Wins）；`POST /endpoints/{id}/write + /commands/{command}`；流程 `UI验证→Core鉴权/能力→audit STARTED→Driver二次验证→执行→read-back→CommandResult→audit COMPLETED/FAILED`，UI禁用非安全边界；Simulator先行 `Writable Value/Reset/Start/Stop/Fault` 覆盖异常路径；FOCAS初期 `controls=[]`，待Simulator通过再逐项 `reset_alarm/select/start`，禁 `generic write(address,value)` 降级CNC（§22.6）；默认 `disabled` 需 `--enable-control`。

**K Real Device**：按 `OPC UA Write → Modbus Write → S7 Write → FOCAS Commands → CANopen/CiA402` 逐协议真机验收，每能力独立 `real-device` matrix（§26 K）。

Gate J：`disabled default / CONTROL_DISABLED精确码 / scope / actor / 异常全覆盖 / 永不Latest-Wins`；Gate K：对应设备真机通过。

---

## 13. 横切：性能/Soak/Release/CI

### 13.1 性能预算（§8）

基准输出必含 `OS/CPU/cores/RAM/Rust version/build profile/git SHA`。

| 场景 | 预算 |
|---|---|
| Configure compile `ResourceSelection→AcquisitionPlan` | `1K ≤100ms / 10K ≤1s / 50K ≤5s` p95（3 warmup+20测） |
| Configure memory | `50K Driver RSS delta ≤256MiB`，超限需单独 ADR |
| Runtime | `≥50K updates/s + IPC p95≤20ms p99≤50ms + Descriptor不进热路径，regression <5% 需Review` |
| Descriptor | `validation warm p95 ≤50ms / cold GetDescriptor 5s / JSON ≤256KiB` |

### 13.2 Soak（§30）

`24h（RC 72h）`，覆盖 `多Endpoint/不同poll/OPC UA订阅/Driver重启/断线/Core重启/reload/backpressure/Probe/DescriptorFetch`，观测 `RSS/CPU/handles/Tokio tasks/queue depth/reconnect/coalesced/sequence/ipc`，验收 `无泄漏/无孤儿/ID稳定/revision稳定/可恢复/UI不影响采集`。

### 13.3 Release Artifact（§25）

自动生成 `release-validation.json`（`schema_version/git_sha/environment{os,cpu,ram,rust,profile}/contract_tests/performance{throughput,ipc,configure_1k/10k/50k}/soak/rss/real_device_matrix`）；新增 `schemas/release-validation.schema.json (Draft2020-12, §25.2)` + `examples/release-validation.example.json`，CI `generate → validate against schema → PASS → upload`，字段删除/改义需 `schema_version` 演进。

### 13.4 测试目录目标（§27）

```
tests/driver-contract/{protocol_negotiation,session_lifecycle,data_plane,data_semantics,fault_tolerance,subprocess_recovery,subprocess_orphan_guard,descriptor_contract,resource_contract,discovery_contract,control_contract}
tests/performance/{e2e_50k_real,configure_large,control_latency}
tests/fixtures/{generic-register,generic-symbol,generic-function,generic-object-dictionary,generic-report}
tests/web-e2e/{add-device,configure-resources,browse,profile,control}.spec.ts
schemas/release-validation.schema.json + examples/release-validation.example.json
```

### 13.5 CI/Release Gate（§28-29）

PR：`cargo fmt --check / clippy -D warnings / cargo test+build` + `pnpm lint/typecheck/test/build` + `Contract/Migration/Simulator E2E`；涉Artifact必 `schema validation PASS`；Release：`Win+Linux + 50K + Configure + migration dry-run/apply/rollback + 24h Soak + real-device matrix + schema PASS`；Control Release另需对应设备 `real-device control validation`。

---

## 14. Definition of Done（§31）

Milestone仅同时满足 `实现 + Contract测试 + 错误路径 + Regression + Migration(如涉Store) + Docs + Diagnostics + Performance数据 + CI` 才 `DONE`；仅编译/仅静态测试不算。

---

## 15. 风险与缓解

| 风险 | 缓解 |
|---|---|
| Descriptor Contract 无法自然表达FOCAS多输出 | B阶段若失败则不冻结契约，回 §13 修枚举（§26 B Gate） |
| Secret 备份恢复丢失 | §6.4 Backup API + §5.3 key_id版本化 + 演练 `wrong key fails closed` |
| Queue拆分引入回归 | 保256基线，改1024需 benchmark ADR（§7.1） |
| Web与后端并行漂移 | 决策 §0：F前仅fixture静态，openapi先冻503形态 |
| Tombstone误删致ID reuse | §3.10.1 永不自动GC + high-watermark保障 + 离线工具需Backup |

---

## 16. 关键文件清单（按 Milestone）

```
0:  crates/driver-manager/src/snapshot.rs
    drivers/focas2/src/*, crates/core-types/src/lib.rs, tests/driver-contract/tests/data_semantics.rs, subprocess_orphan_guard.rs
A:  crates/core-types/src/{descriptor,schema,resource,capability}.rs
    proto/driver.proto, crates/driver-protocol/build.rs, crates/driver-manager/src/manager.rs, crates/core-api/src/lib.rs
B:  drivers/{simulator,s7,focas2,opcua}/src/lib.rs + driver.toml + profiles/
C:  crates/core-types/src/resource.rs, drivers/*/src/lib.rs (normalize), apps/mesa-cli/src/main.rs
D:  drivers/focas2/src/focas_api.rs, address.rs, lib.rs (FocasOperation, PmcRange)
E:  crates/config-store/migrations/*.sql, crates/config-store/src/lib.rs, crates/core-api/src/lib.rs (Secret)
F:  crates/driver-sdk/src/lib.rs (queue split), crates/core-api/src/lib.rs (validate/probe/diagnostics/audit)
G:  apps/mesa-web/** (React/Vite)
H/I: drivers/opcua/src/lib.rs (browse), crates/core-types/src/capability.rs, drivers/*/profiles/*.json
J/K: crates/driver-sdk/src/lib.rs, drivers/*/src/lib.rs, crates/config-store (control_audit)
25:  schemas/release-validation.schema.json, examples/release-validation.example.json
```

---

## 17. 下一步（本分支）

1. 完成 Milestone 0 的 P0-A/B/C/D 修复并提交 `data_semantics + orphan_guard` 测试（本分支首个PR）
2. 按序 A→K 逐 Milestone 发PR，每PR附 `release-validation.json` 片段与 `cargo test --workspace` 结果
3. 每 Milestone 结束更新本计划的 `进度` 章节

> 本计划已覆盖 `V2.1` 全36章（含新增 §1.6.1 §2.1 §3.10.1 §4.4表 §6.5-6.6 §25.2-25.3 等），无遗漏；后续实施以本计划的 Gate 为唯一验收依据。
