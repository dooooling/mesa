# AGENTS.md

本仓库指导文件。交流与产出统一使用中文。

## 仓库现状（先读这个）

- 设计文档：`docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md`（V2.1，36ch）是唯一事实来源，写任何代码前必须先读；`Mesa_Driver_MVP_实施方案.md`（V1.4）为历史 MVP 基线，仅作兼容参考
- **V2.1 已落地**：`Mesa_Driver_MVP_实施方案.md V1.4` 基线上的动态 UI / Secret / Browse / Control / 原子事务 / 50K 验收等扩展；`feat/v2.1-impl` 为当前开发分支
- `rust-toolchain.toml channel="1.95.0"` / `Node 22.18 pnpm 10.28` / `apps/mesa-web`（AntD 最小管理端）

## 常用命令（已验证，rust 1.95.0）

```bash
cargo build --workspace                                             # 构建（含全部驱动 bin）
cargo test --workspace                                              # 全部测试（含 §21 全量 Contract Test + V2.1 扩展）
./target/release/mesad                                         # 从 workspace 根启动（默认 drivers/ 目录 + 端口 8132）
```

注意：`cargo test -p mesa-contract-tests` 只构建依赖包 lib，不重编驱动 bin；
改过驱动代码后跑子进程类合同测试前，先 `cargo build --workspace`，否则旧二进制会让故障注入静默失效。

验收入口：
- `http://127.0.0.1:8132/api/v1/drivers` — 驱动清单
- `http://127.0.0.1:8132/api/v1/endpoints` — 端点状态
- `http://127.0.0.1:8132/api/v1/points/latest` — 最新值

Contract Test 基线（§21 全部 20 项）位于 `tests/driver-contract/tests/`：
`smoke.rs` / `protocol_negotiation.rs` / `session_lifecycle.rs` / `data_plane.rs` /
`fault_tolerance.rs` / `subprocess_recovery.rs`。任何新 Driver 必须全过该基线。

## 项目是什么

Mesa：工业设备统一采集平台。Rust + Tokio + Protobuf IPC + SQLite。独立进程 Driver：S7（Siemens PLC）、FOCAS2（FANUC CNC）、OPC UA、Simulator（V2.1 四类）。

计划目录结构见文档 §23（10 个 crate 的 cargo workspace + `drivers/` + `apps/mesad`）。

## 硬性设计约束（多轮评审的结论，违反即为架构错误）

- **Core 不懂协议**：S7 DB 地址 / FOCAS Function / OPC UA NodeId 的解析只允许存在于对应 Driver 进程内
- 所有 Data Plane 队列必须有界；背压 = Latest-Wins Coalescing，禁止靠加大队列或无界缓存解决
- `point_id` 由 **Core 分配并持久化**，Driver/Core 重启后必须保持稳定（删除走 tombstone）；高频通道只传 `(connection_handle, point_id) + TypedValue`
- 业务时间戳 = UTC Unix ns；性能测量用宿主机单调时钟，禁止两进程 UTC 相减
- Driver 为独立进程：token 经 stdin 传入并持续监控该管道（EOF 即自杀退出）；Linux `PR_SET_PDEATHSIG`、Windows Job Object `KILL_ON_JOB_CLOSE` 防孤儿
- V1 兼容基线严格只读；V2.1 Control 默认 `disabled`，仅 `mesad --enable-control` 显式开启时允许 Write/Command，否则 `CONTROL_DISABLED`
- 配置变更为全量快照替换；运行中的 Endpoint 改任务必须走 Stop → ConfigureTasks → ApplyPointMap → Start（新 `stream_epoch`），不允许热 Apply
- **配置真值只在 Core**：Driver 进程不持久化业务配置，重启后由 Core 重放（Handshake → OpenConnection → ConfigureTasks → restore PointMap → Start）
- **管理边界**：CLI 仅作为 REST API 客户端，禁止直接读写 SQLite 或证书目录；REST 默认仅绑定 loopback

## 开发顺序（勿跳步）

按文档 §24 执行：`core-types` 冻结 → ConfigStore+REST → driver-protocol/IPC 认证 → SDK+Simulator（过全部 Contract Test）→ DriverManager → 才轮到 S7/FOCAS2/OPC UA 真实驱动。

## 代码与注释规范

- 注释统一使用**中文**，做到合理详细：解释设计意图、协议背景和"为什么这样做"，不复述代码字面行为。公共 API、协议编解码、非直观的并发/时序处理必须有注释
- 注释只描述代码逻辑本身，禁止写入开发进度类叙事（如"Phase X 完成"）；但**未完成与后续调整必须显式标注**：未实现的功能用 `TODO`、需修正的缺陷用 `FIXME`、待确认的决策用 `NOTE`，均需附带简短原因或触发条件，便于全局检索和清理
- 不写冗余注释（每行都注释），也不留关键路径裸奔（复杂解析、字节序处理、锁顺序等零注释）

## 质量门槛（均为硬性要求，不是建议）

- **Contract Test 是唯一准入基线**：任何 Driver（含 Simulator）必须全过 §21 全部测试项；Simulator 必须第一个通过，S7/FOCAS2/OPC UA 不得绕过或裁剪该基线
- **测试四层递进**：Unit → Protocol/Fake → Software Simulator → Real Hardware（§20）；真机验证不可省略，FOCAS2 无可信模拟器，只能靠 `FocasApi` 抽象 + Fake 做 CI，最终兼容性必须真机确认
- **单测要合理聚焦，不琐碎也不缺位**：单元测试必须写，但写在高价值处——编解码、地址解析、边界值、错误路径属于必覆盖项；不堆砌覆盖率导向的琐碎用例，不用 mock 镜像实现细节，避免维护大量低信息量测试拖慢迭代。行为与协议语义交给 Contract Test 和集成层验证。取舍标准：一条测试只有在其失败能真实拦截缺陷时才值得存在
- **性能预算即验收条件**（§22）：≥50K Point Updates/s 持续 60 分钟；IPC delivery p95 ≤ 20ms / p99 ≤ 50ms（单调时钟测量）；RSS 禁止无界增长（60 分钟增幅 ≤10%）；Core 消费降至 25% 时背压仍有效；Conn-1000 无 Task/Handle 泄漏。真实设备不强行对齐 Simulator 指标
- **外部依赖 Gate**（§19）：FOCAS2 未完成 SDK 合法获取/许可/平台/函数支持确认前，**不发布生产版本**；S7 连接自检必须输出 PUT/GET 权限等具体诊断，禁止只报"连接失败"；OPC UA 禁止把"忽略证书校验"作为生产默认
- **Phase 门禁**：每阶段完成条件见 §24 表，未达标不得进入下一 Phase；Phase 0 的 schema 冻结尤其关键——ID/时间/Quality/Value/Task Schema 冻结后不要随意改动
- **Definition of Done**：整体以 §26 的 23 条验收标准为准，全部可测试

## 文档关键章节速查

| 主题 | 章节 |
|---|---|
| 对象模型 / Point ID 生命周期 | §5–6 |
| 三类驱动访问范式 | §7 |
| Value/Quality/时间戳/背压/断线语义 | §9–12 |
| IPC 消息集 / 版本协商 / 心跳 / 孤儿防护 | §14 |
| Manifest 与驱动发现 | §15 |
| Contract Test 清单（全部必过） | §21 |
| 性能预算（50K updates/s 等量化指标） | §22 |
| 验收标准（23 条） | §26 |
