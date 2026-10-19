# T1 只读现场诊断方案：`S7_JOB_0x8201`（端点 `lx-nck-tags-190-connection` / SINUMERIK 828D）

- 状态：**T0 离线只读准备已完成**（见 §3.1 与本轮 T0 报告 `target/validation/t0-report.md`）；T1a 起需现场授权窗口 + 人在机床旁。本方案不含自动化真机测试。
- 基线：`main = 2a29777b643c674557d975cdf93998645cbe10d6`；本文件所在分支 `docs/nck-s7-0x8201-diagnosis-plan`，`HEAD = 1a54c88`（本修订为工作树改动，未提交）。
- 本轮修订依据：评审五项意见（Stop 响应速度 / T4 批读 19 的字节依据 / H1–H3 判据改弱 / 请求速率口径 / 机床状态对照）+ 两项（身份指纹不足 / 官方负载上限未知），逐条落点见 §14。
- 只读声明：不改生产代码/测试/配置；不启动端点（除 §6 明确步骤）；不写机床参数；不跑测试/构建。
- 口径：结论给 `文件:行号`；代码无法确定的标 **待确认** 并写"需要什么观测"，**不用估算填坑**。

## 0. 现场事实与六问索引

### 0.1 现场事实（已确认，不再推测）

| 项 | 值 |
|---|---|
| 端点 / 驱动 / 系列 | `lx-nck-tags-190-connection` / `sinumerik-nck` / `family=828d` |
| 连接 | `192.168.15.60:102`，`local_tsap=1024`，`remote_tsap=3332`，`pdu_length=480`，`max_read_items=1`，`timeout_ms=1000` |
| 配置 / 轮询 | revision **13**，**238** 点（235 `nck_native` derived + 3 `nck_variable` 语义）；`interval_ms=1000` |
| 网络 | 事发期实测 `PING=True`、`TCP 102=True`（**T0 未复核**；搬迁后 IP 是否仍为 `192.168.15.60` **需现场确认**） |
| 历史错误（两种，不可混用） | ① `driver reported Failed ProtocolError/S7_JOB_0x8201: Read 请求被拒绝，原始头部码0x8201`（作业级拒绝）<br>② `driver reported Failed ConnectionError/COTP_RECV_FAIL: early eof`（COTP 层对端关连接，最近一次） |
| 端点状态 | **`STOPPED`，`desired_running=false`**（2026-10-19 18:04:49.304 落库；见 §3.1）。T0 开始时曾观测到 `desired_running=true` + 反复重连（§3.1），已由上级/现场置停 |
| 现场事件 | **整机搬迁 / 断电**（用户答复）；搬迁后机床是否上电、网络是否恢复、IP 是否变化 **需现场** |
| 时间线（Mesa 侧可见部分） | 235 点稳定（155 GOOD）→ 加 3 语义点 −238 点正常约 20 s → 机床搬动后 0 GOOD / 238 BAD → `RECONNECTING` → 最后成功批次时间戳 `2026-10-19 17:46:49.645`（§3.1） |

**路径纠正**：`pdu.rs` 实际在 `crates/s7-transport/src/pdu.rs`（不在 `drivers/sinumerik-nck/src/`），任务书引的 `:165`、`:225` 正是该文件。

### 0.2 评审六问 → 证据索引（先证据、后结论）

| # | 问题 | 证据位置（本方案章节） |
|---|---|---|
| 1 | `0x8201` 的真实来源与分类 | §1.1（`pdu.rs:154-168`、`:181-195`、`:222-228`；`read_var.rs:174`） |
| 2 | 一次作业拒绝如何影响 Quality / Endpoint State / 重连 | §1.2（7 跳链路）、§1.3（放大）、§1.4（项级 vs 作业级） |
| 3 | 真实读取节奏：`max_read_items`、PDU 分组、poll 周期、**实际发送频率** | §1.5（代码能确定的部分）、§1.6（**不能确定的部分 + 测量方法**） |
| 4 | H1 验证：降频前后可比的错误率/请求数/延迟 | §6（T3/T4/T4b）、§9（每次运行必测项） |
| 5 | H2 验证：机床状态与失败时间的对应 | §6.3、§3 机床状态表 |
| 6 | H3 验证：能否定位固定地址/固定类别 | §6（T5/T6）、§1.4（当前无 point_key 归因的局限）、§12.3 |

### 0.3 三处必须先纠正的口径

1. **"238 点 × 1 item × 1 s ⇒ ≈238 请求/秒"是未验证的推断，本方案不采用**。代码只能确定"**每轮 238 个串行请求**"，不能确定每秒多少请求（取决于 RTT、单轮耗时、tick 补偿策略）。见 §1.5–1.6。
2. **"0 GOOD / 238 BAD"不是"238 个地址都被拒"**：那是断线后的统一标记（`snapshot.rs:399-411`），不含任何地址信息。把它当"全部地址协议层拒绝"会误导定位。
3. **设备身份不能来自 Core**：`devices` 表只有 `id/name` 两列（`mesa.db` 只读副本实测；`GET /api/v1/devices` 也只回 `id/name`），**没有** vendor/model/serial。身份最多只能来自 NCK 只读身份探针（`docs/evidence/nck-live-identity-probe.txt:6-69`），且见 §5 的能力边界——它也不能证明唯一身份。

## 1. 故障链（代码核实）

### 1.1 `0x8201` 的产生与分类

| 事实 | 证据 |
|---|---|
| S7 `Ack_Data` 头 12 字节：`s7[10]=error class`、`s7[11]=error code` | `pdu.rs:14-23`、`:181-195` |
| **纯拒绝 ACK**（ROSCTR=`0x02`）要求 `param_len=data_len=0`，否则 `S7_ACK_SHAPE`；两个字节**合并**成 16 位作业码 `S7_JOB_0x{error:04X}`，消息 `"{ctx} 请求被拒绝，原始头部码0x…"` | `pdu.rs:154-168` |
| 读路径 `ctx` 固定 `"Read"`（现场文案与代码逐字一致） | `read_var.rs:174` |
| 单测：`[0x32,2,…,0x82,1]` → `S7_JOB_0x8201`；多带 1 字节 → `S7_LEN_MISMATCH` | `pdu.rs:222-228` |

`0x82` 是 error class、`0x01` 是 code（S7Comm 惯例"对象定义类错误"，**具体语义待官方错误表核对**，本方案不依赖该解释）。**拒绝 ACK 里没有 item 索引，也没有返回码** —— 驱动无法从报文本身知道是哪个地址触发。

### 1.2 一次作业拒绝如何升级为 `RECONNECTING`（数据面）

| # | 环节 | 证据 |
|---|---|---|
| 1 | `read_vars` 顺序分块；**作业级错误立即返回、不按项隔离**；transport 错误 → `kind=Protocol`、`code=S7_JOB_0x8201`（原样透传） | `client.rs:90-99`、`:135-145`；`core-types:643` |
| 2 | 采集循环 `return Err(e)`，**整个 poll task 结束** | `lib.rs:920-928` |
| 3 | 任一 task `Err` → cancel 全体 → `run()` 返回 `Err` | `lib.rs:959-985`；单测 `lib.rs:2108-2152` |
| 4 | SDK 把 `run()` 的 `Err` 变成 `State(Failed, err.to_string())` | `driver-sdk:1536-1544` |
| 5 | Core 判 `Lost`（reason 即现场 detail）→ `Reconnecting` + `mark_communication_lost` | `endpoint.rs:908-914`、`:336-356` |
| 6 | 该端点**全部点**置 `BAD` + `quality_code="COMMUNICATION_LOST"`（保留最后值与旧时间戳） | `snapshot.rs:396-411` |

**对 Point Quality 的语义影响**：断线 BAD 与"该点被设备拒绝"在契约上是**同一字段的不同质量码**——断线是 `COMMUNICATION_LOST`（字符串），项级拒绝是数字设备码（`lib.rs:556-564`）。因此事后看 `points/latest` 只能区分"连接级 vs 项级"，不能区分"哪个地址导致连接级"。

### 1.3 放大效应（"地雷"实锤）

- **一次作业拒绝 = 整端掉线**（`client.rs:90-99` + `lib.rs:926` + `endpoint.rs:910`）；**重连即复发**：`had_running_session=true` 时退避下标归零（`endpoint.rs:357-360`），序列 `[1,2,5,10,30]s`（`endpoint.rs:32-38`）。
- **每次重连重放 238 点**：attempt 固定 `OpenConnection → ConfigureTasks → ApplyPointMap → Start`（`endpoint.rs:716-839`）；Configure 是**纯本地编译、无设备 I/O**（`lib.rs:654-816`），所以重连不做地址预检，必然在第一个 Read 再死。
- 结论：故障期间端点在"连上→读→被杀→~1 s 后重连"的循环里自我放大。这也是 §7 Stop 必须秒级的原因。

### 1.4 作业级 vs 项级：分层判定，以及三个"不区分"的后果

| 层 | 判定点 | 依据 | 结果 |
|---|---|---|---|
| 传输层 | ROSCTR=`0x02` 且 error≠0 | `pdu.rs:154-168` | **作业级** `S7_JOB_0x8201`，fatal |
| 传输层 | ROSCTR=`0x03` 且 `[10]≠0` / `[11]≠0` | `pdu.rs:181-195` | `S7_HEADER_ERR` / `S7_0xNN`（按 0x03/04/05 映射 Configuration/Address/Protocol，`error.rs:78-99`） |
| 应用层 | 响应数据区逐项 `return_code != 0xFF` | `client.rs:104-129` | **项级**：该点 BAD，会话存活 |

**没有混为一谈**（两条路径分开实现），但有三处"信息丢失"，直接决定了本方案能做什么、不能做什么：

1. 作业级一律 `kind=Protocol`，**不走** `s7_cpu_error` 的 Address 映射（`error.rs:78-99`）→ 无法从错误类别判断"是不是地址问题"。
2. 拒绝 ACK 无 item 索引；`max_read_items>1` 时一次拒绝会**连带同批好点**（整批只返回一个 `Err`）。当前 `max_read_items=1` 恰好"一次拒绝 = 一个点"，但驱动仍不告诉你是哪个点。
3. 身份路径有 `device_rejected()`（`identity.rs:74-76`）把 `S7_JOB_*`/`S7_ITEM_*` **降级成诊断节点**（`:347-361`、`:454-462`、`:479-489`）；**数据面无等价物**。注意 `S7_ITEM_*` 只在身份路径生成（`identity.rs:98-107`），数据面的项级码是数字。

### 1.5 实际读取节奏（一）：代码能**确定**的部分

| 问题 | 答案 | 证据 |
|---|---|---|
| 串行还是并发？ | **严格串行**：一个 chunk 一次 `read_var`，`await` 完才发下一个；客户端还被 `Arc<Mutex<_>>` 串行化 | `client.rs:92-99`；`session.rs:190-236`；`lib.rs:880`、`:921` |
| 分批？238 点=238 请求？ | 先按 `max_read_items` 分行（当前=1）→ `chunks(1)` ⇒ 238 块 ⇒ **每块 1 个 ReadVar** | `client.rs:92` |
| `pdu_length=480` 参与分组吗？ | **当前不参与**：传输层 `plan_chunks` 只在块内做字节预算，块长=1 时只看 `check_single_item_fits` | `read_var.rs:103-129`、`:192-196` |
| 重试/超时？ | **无重试**；每请求**两段**超时各 `timeout_ms`（发送 1000 ms + 接收 1000 ms，单请求最坏≈2 s，不含 TCP 重传） | `client.rs:93-98`；`session.rs:200-225` |
| 扫描节拍？ | `tokio::time::interval(period)` + `MissedTickBehavior::Skip`，**首 tick 立即触发**（Start 后马上读，不等 1 s） | `lib.rs:900-911` |
| 一轮耗时 > 1 s 时会怎样？ | 按 Skip 语义**丢节拍、不补跑、不突发、不积压队列**；实际轮周期被拉长到"采集耗时"量级 | `lib.rs:903-904`（tokio 1.53.1，`Cargo.lock:2660-2661`）；节拍网格对齐的精确行为 **待确认**，用 §9 的轮周期直方图实测 |

### 1.6 实际读取节奏（二）：代码**不能确定**的部分（禁止估算填充）

- **每秒请求数不可从代码推出**。只有恒等式：`每轮请求数 = 238`；`轮周期 T_round ≈ max(1 s, Σ RTT_i)`；`请求速率 ≈ 238 / T_round`。RTT 未知 ⇒ 速率未知。
- 数值示例只说明量级、**不是结论**：RTT=2 ms ⇒ 一轮 0.48 s（速率上界 238 req/s）；RTT=10 ms ⇒ 一轮 2.4 s（~100 req/s）；RTT≥4.2 ms ⇒ 轮周期已 >1 s，1 s 周期形同虚设。因此"20 秒正常"只能说明**成功轮数 ≈ 20 / T_round**（估算 15–20 轮，不作判定依据）。
- NCK 是否有"每秒请求上限 / 累计配额 / 连接数上限"——**待确认**（无官方数据，只能靠 T3/T4 的剂量-反应曲线间接推断）。以上三项全部由**同一次运行内的时间戳差分**测得（§9），不需要额外真机操作。

**口径修正（评审意见 4，必读）**：`points/latest.timestamp_ns` **不是 S7 请求发送时刻**，也不是单点采样时刻。

| 事实 | 证据 |
|---|---|
| 驱动在一轮内把 238 个点读完后，**一次性**以 `now_unix_ns()` 生成批次时间戳并发布 | `lib.rs:946-954`（`timestamp_ns: now_unix_ns()` 在 `read_vars` 之后） |
| Core 直接把 `batch.timestamp_ns` 落为每个点的 `timestamp_ns`（同批 238 点共享同一值） | `snapshot.rs:325` |
| 断线标记**不修改** `timestamp_ns`/`value`，只改 quality（`mark_communication_lost`） | `snapshot.rs:396-423` |
| T0 实测印证：238 条 `points/latest` 只有 **1 个**不同时间戳 | `target/validation/t0-points-latest.raw.json` |

⇒ 因此：

1. 相邻 **轮**的时间戳差 ≈ 轮周期（但含"该轮最后一个请求完成"的相位偏移，且第 1 个点的请求时刻比批次时间戳早整整一轮耗时）；
2. **无抓包、无驱动级时间戳时，只能报告"推算速率"**：`推算每轮请求数 = 238`、`推算轮周期 = 相邻批次 timestamp_ns 差`、`推算速率 = 238 / 轮周期`，且必须标注"推算，非实测"；
3. 驱动**没有 tracing subscriber**（`drivers/sinumerik-nck/Cargo.toml:27` 只有 `tracing`），`client.rs:105/109/116` 的 warn 当前不输出 → 驱动侧请求级时间戳**不可得**；
4. "实测"只有两条路：宿主机抓包（本阶段不允许），或改代码加埋点（需评审，本方案不实施）。

## 2. 三种假设及其可观测差异（判据已弱化为"支持 / 削弱 / 无法区分"）

> **总原则（评审意见 3）**：本方案**不做**根因确认，只做假设的"支持 / 削弱 / 无法区分"三态判定。**只有单变量受控对照**（同一地址集合、同一机床状态，只改一个自变量，且运行内无其它变量漂移）才可能确认根因；任何"跑通了/没跑通"的观察都不足以定根因——因为失败本身是随机的、时序相关的，且当前无请求级证据。

| 假设 | 待验形式 | 观察结果 → 只能得出 |
|---|---|---|
| **H1 负载/节奏** | "每轮 238 个串行请求"在 NCK 侧造成资源/时间超限；是瞬时速率、突发还是累计，未定 | 低负载稳定但全量稳定不下来 → **支持**（仅说明"负载相关"，不区分速率/突发/累计）；低负载也死 → **削弱**（并不否定 H1 的"累计"版本）；低负载稳定、全量也稳定 → **无法区分**（未复现） |
| **H2 机床状态** | 某些地址只在特定状态可读，作业被拒 | 同一地址、同一频率、仅机床状态不同的**两次运行**结果相反 → **支持**（仍需 HMI 记录佐证状态确实不同）；两次都死 → **削弱**；只有一次运行 → **无法区分**（机床状态未被控制） |
| **H3 特定地址** | 某地址在协议层被拒（作业级，非值级 BAD） | 移除该地址后必好、加回必坏（同一频率/状态）→ **支持**；移除后仍死 → **削弱**（若移除的点集合确实包含疑似毒点）；只跑一次 → **无法区分**。**注意**：238 点曾正常约 20 s ⇒ 静态毒地址与时间线矛盾，H3 若成立只能是"状态相关地址"或"新增语义点解析到状态相关地址" |

**每条结论的边界（写报告时必须照抄）**：

- "支持" ≠ 根因确认，只表示**该解释与观察相容且未被削弱**；写"支持 Hx"，不写"就是 Hx"。
- "削弱" ≠ 排除，只能说"在该次运行条件下未观察到 Hx 预测的现象"。
- "无法区分" 是最常见结果，必须如实写，**禁止**用"大概/可能/应该是"补成结论。
- 三种假设**不互斥**（例如 H1+H3 同时成立），本方案不给出"哪个对"的排序。

**先例（只作指纹，不作结论）**：module 26 的 `V/6` 对任意列都返回 `0x8201`，module 130 的 `V/6` 正常（`docs/evidence/nck-live-native-probe.txt:51-59,78-122`；`nck-live-identity-probe.txt:52-69,79-122`）——证明"作业级地址拒绝"真实存在，且**可按入口（module）定位**。这是**历史证据**，不是本轮观察。

## 3. T0：执行前准备（不碰机床，端点 STOPPED）

```powershell
$base="http://127.0.0.1:8132"; $ep="lx-nck-tags-190-connection"
$dir="target/validation/nck-8201-$(Get-Date -Format yyyyMMddTHHmmssZ)"; New-Item -ItemType Directory -Force $dir|Out-Null
Invoke-RestMethod "$base/api/v1/endpoints/$ep"      | ConvertTo-Json -Depth 10 | Set-Content "$dir/00-endpoint.json"
Invoke-RestMethod "$base/api/v1/tasks?endpoint=$ep" | ConvertTo-Json -Depth 50 | Set-Content "$dir/01-tasks-rev13.json"
Invoke-RestMethod "$base/api/v1/points/latest"      | ConvertTo-Json -Depth 10 | Set-Content "$dir/02-points-latest.json"
```

| 步骤 | 内容 | 用途/判定 |
|---|---|---|
| T0.1 | 备份 endpoints + tasks（`GET /tasks?endpoint=` 返回 `{revision,tasks}`，`core-api:2208-2227`） | 回退依据；**revision 单调递增**（`config-store:1432-1452`），回退只能恢复"rev13 的内容" |
| T0.2 | 落盘 238 点（`source_label`/`quality`/`quality_code`/`value_origin`，`snapshot.rs:97-127`） | 唯一离线地址清单。`source_label` 形如 `nck://read/C/1/122/5/1?…`（`native.rs:347-361`）。**T0 已确认 mesad 未重启**（§3.1），当前 238 条齐全；但即便重启，`point_key→point_id` 仍可由 `point_registry` 复原（238 live，261–498），**丢的是 latest 值与质量**（`register_points` 只刷新已有条目，`snapshot.rs:237-269`） |
| T0.3 | 统计 80 个项级 BAD 的 `quality_code` 分布 | 整数设备码 ⇒ 本来就被项级拒绝的候选池（项级≠作业级） |
| T0.4 | 用 `source_label` 比对已知拒绝指纹：`area=V,module=26,column=199`、`V/6` 各列 | 命中即标注，T5 优先剔除 |
| T0.5 | 预跑 §9 的 watch 脚本（只读 state，不 Start），并记录环境与机床状态表 | 证明"命中即 stop"可用；H2 判定的唯一依据 |

**机床状态表（每次运行前必填）**：上电 / 急停 / 驱动使能 / 参考完成 / 程序运行 / 轴是否运动 / 报警号 / 操作员口述。

### 3.1 T0 本轮离线只读复核（2026-10-19 18:0x 完成，未碰机床）

| 项 | 结论 | 证据 |
|---|---|---|
| mesad 是否重启 | **未重启**。PID 5652，`StartTime=2026/10/19 14:08:31`；`GET /diagnostics.uptime_secs=14400.12`（18:08:31 采样，进程内 `start_time.elapsed()`） | `core-api:1583`；进程表 |
| 配置真值 | `config_revision.revision=13`；`tasks` 表 1 行、`binding_config_json` 48275 字节 | `mesa.db` 只读副本（`target/validation/t0-db-snapshot/`） |
| 点 ID 快照 | `point_registry` 该端点 **238 live + 260 tombstone**，当前 238 个 `point_key` 全部命中，`point_id ∈ [261,498]`，与 `points/latest` 完全一致 | 同上 + `GET /points/latest` |
| 运行期点值 | 238 条全部 `BAD/COMMUNICATION_LOST`；`value_origin` = 158 `LAST_KNOWN` + 80 `PLACEHOLDER`；**238 条共享 1 个时间戳** `1792403209645804200` = `2026-10-19 17:46:49.645 +08:00` | `target/validation/t0-points-latest.raw.json` |
| 端点状态变化 | T0 开始时（18:04:37–18:04:47）为 `desired_running=true` / `RECONNECTING`，**epoch 在每个采样点都变**（3005249133282388839 → 8432389577162556645 → 12868702587871863799 → 18331871838326995825），并短暂观测到驱动子进程 `mesa-driver-sinumerik-nck.exe`（PID 64504）出现又消失；18:04:49.304 变为 `STOPPED` / `desired_running=false` | `target/validation/t0-endpoint-sampling.json`、`t0-endpoints-snapshot.json` |
| 事件面 | `GET /events` 返回 `{"events":[]}`；`GET /events/stats` 全部计数 0（`stored_rows=0`），`event_tasks` 表 0 行 → **Mesa 侧无端点状态历史、无报警历史** | `core-api:3331-3334`；`mesa.db`/`events.db` |
| Core 日志 | **未落盘**：mesad 由外层 `pwsh ... 2>&1` 启动，`stdout/stderr` 未重定向到文件；驱动 `stdout/stderr` 被 `inherit`（`process.rs:70-72`）→ 驱动/Core 日志只存在于该控制台 | 进程命令行；`process.rs:70-72` |

**关于 epoch 的诚实边界**：epoch 每次变化只证明 Core 收到了新的 `StartConnectionAck`（`endpoint.rs:832-845`；`driver-sdk:1569-1581`），**不能**证明 TCP/COPT 已连到机床——驱动的 `NckClient::connect` 发生在 `run()` 内部（`lib.rs:870-873`），而 Ack 在 run 放行之前就已发出（`driver-sdk:1510-1581`）。因此"机床在线"**不能**由端点状态推出，必须靠 T1a。


## 4. T1a：连接对象确认（1 次探测，不读工艺数据）

> **前置（硬性）**：必须先由现场人员确认"机床已上电 + 网络已恢复 + IP 仍为 `192.168.15.60`（或给出新 IP）+ 无报警/急停"，见 §5 能力边界第 4 条。**未确认不得执行本步。**

| 问题 | 答案来源 |
|---|---|
| 参数从哪来 | `GET /api/v1/endpoints/{id}` 的 `connection`（`core-api:1831-1866`；NCK 无 Secret，原样返回） |
| 谁能改、何时生效 | `PUT /endpoints/{id}`（运行中 `CONFLICT`，`core-api:1961-1969`）→ Stop/Start 后生效 |
| 合法性谁裁决 | 驱动 `config.rs:62-196` fail-closed：字段白名单、TSAP 双格式、`timeout_ms≥500`（`:20`、`:150-155`）、`pdu_length 240..960`（`:198-210`）、`family` 白名单（`lib.rs:367-376`） |
| 是否同一台设备 | **本步不能证明**：探针只返回 `reachable` + 警告文本，`vendor/family/model` 恒 `None`（`probe.rs:4-18`、`:49-64`）→ 交给 T1b |

```powershell
$conn=(Invoke-RestMethod "$base/api/v1/endpoints/$ep").connection
Invoke-RestMethod -Method Post "$base/api/v1/endpoints/$ep/connection-check" -ContentType "application/json" `
  -Body (@{mode="probe";connection=$conn}|ConvertTo-Json -Depth 10) | ConvertTo-Json -Depth 10 | Set-Content "$dir/10-probe.json"
```

| 结果 | 判定 | 下一步 |
|---|---|---|
| A：`reachable=true`，警告含 `协商 PDU 480`（`probe.rs:47,57-63`） | 连接参数与现场一致；连接层不是故障点 | T1b |
| B：`reachable=true` 但 PDU<480 | 对端压小 PDU，分片/容量行为随之改变 | 记录实测值，继续 T1b（不据此改配置） |
| C：`reachable=false` | 与**事发期**的 `TCP 102=True` 不一致（该事实 **T0 未复核**）⇒ 先按"现场网络/机床未就绪"处理，**不能**直接判定为 TSAP 配置问题，也**不是** `0x8201` | 停止本方案，请现场核对网络与上电状态 |

## 5. T1b：设备身份交叉验证（只读身份探针，端点保持 STOPPED）

身份浏览走**临时驱动进程 + 独立连接**（`manager.rs:653-730`），与端点状态无关；驱动侧整页 15 s 超时（`lib.rs:1005-1016`），每页固定 ≤8 实例（`identity.rs:21-28`），每轴最多 5 次读（`identity.rs:141-182`）——这是**每分钟几十次请求**的轻量路径，不引入轮询负载。

```powershell
foreach($p in @("nck://identity","nck://identity/axes","nck://identity/objects")){
  Invoke-RestMethod -Method Post "$base/api/v1/endpoints/$ep/browse" -ContentType "application/json" `
    -Body (@{parent=$p;cursor="1";limit=3}|ConvertTo-Json) | ConvertTo-Json -Depth 10 |
    Set-Content "$dir/11-identity-$(($p -split '/')[-1]).json"
}
```

**指纹表**（同一台 828D 历史实测，`docs/evidence/nck-live-identity-probe.txt:6-69`）：轴名 `MX1,MY1,MZ1,MSP1,MA1,MB1,MC1,MQ1`；`MACHAX_USED=1,2,3,4,5,0,7,8`；驱动号 `1..8`；输出类型 `1,1,1,1,1,0,1,1`；主轴归属仅轴 4=1；module130 对象名 `SERVO_3.3:3..8`、`SERVO_3.15:3..4`；**已知异常**：module26 的 `V/6` 任意列 `0x8201`。

**身份指纹的能力边界（评审意见，必须写在 T1b 报告里）**：

1. 这些字段是**轴配置/驱动拓扑**，同一系列、同一配置批次的机床**可以完全相同**；它们不构成唯一标识（无序列号、无机器码、无出厂编号）。
2. 因此"逐项一致"只**提高**"连到预期机床"的可信度，**不能证明设备唯一身份**；真正能定唯一身份的信息 Mesa 侧完全没有（`devices` 表只有 `id/name` 两列，无 vendor/model/serial——`mesa.db` 只读副本实测；`GET /devices` 同样只回 `id/name`）。
3. **旁证（说明名称字段不可信）**：该端点 `device_id=lx-nck-tags-190` 的 `name` 是 `"LX 190 点 · 实体828D · 192.168.1.1"`，而 connection 里是 `192.168.15.60` —— 名称里写的是**另一个网段**。名称/ID 是人工输入的标签，不是身份证据。
4. **前置条件（硬性）**：T1a/B 之前必须由**现场人员**核对 ① 机床铭牌/序列号与工位号 ② 现场网络口与 IP（是否仍为 `192.168.15.60`）③ 机床当前状态（上电/急停/报警）。三项未经现场确认，T1a 不得开始；即使指纹全对，也只记为"与现场描述相容"。

| 结果 | 判定（弱化后） | 下一步 |
|---|---|---|
| A：逐项与指纹一致 | **提高可信度**（与"同一台 828D 同配置"相容），**不能**证明唯一身份；需现场核对佐证 | §6（报告里写"指纹一致 + 现场核对通过"） |
| B：身份页正常但指纹不一致 | **强不一致信号** → 很可能连到另一台设备，后续测试全部无效 | **立即终止**，回退并上报 |
| C：HTTP 200 但出现 `kind="diagnostic"` 且含 `读取被拒绝`/`S7_JOB_0x8201` | 设备连身份探针都拒 → **削弱** H1 的"纯瞬时速率"版本（极低速率下也拒）。注意身份路径的拒绝是 HTTP 200 + 诊断节点（`identity.rs:347-361`），只看 HTTP 状态会漏判 | 记录编号，直接 T6 |
| D：HTTP 400/504（如 `IDENTITY_TIMEOUT`） | 连接/时延问题，**不是** `0x8201` 证据，也不是身份证据 | `limit=1` 重试 1 次；仍失败则停止上报 |

## 6. 实验序列（T2–T6）：用最少真机操作区分 H1/H2/H3

**点集（从 T0.1 备份裁剪，不手写地址）**：`S1`=3 个已证实 GOOD 的 native（`native.C.1.122.5.1`、`native.A.1.26.30110.1`、`native.C.1.127.13.1`）+ 1 个语义点（备份中首条 `kind="nck_variable"`）；`S2`=去掉语义点；`S3`=`S1` 中 1 个在 20 s 正常期为 GOOD 的 native 点；`S0`=全量 238。

**为什么是这 4 个**：3 个 native 事件前已证实 GOOD（wire 形态稳定），语义点覆盖"新增 delta"；4 点每轮仅 4 个请求（周期 ≥1 s ⇒ ≤4 req/s），足以暴露"毒点/状态拒绝"，又把负载压到 H1 噪声之下；`S2/S3` 是单变量对照，不需另备配置。

### 6.0.1 T4 用 `max_read_items=19` 的**字节预算依据**（评审意见 2，已用真实点表算过）

**结论先说**：19 项/批在**线缆字节**上安全（请求 209 B、响应最大 323 B，均 ≤ `pdu_length=480`），但**驱动自身的规划预算会把第 1 组拆成 17+2**，因此 T4 实跑是 **14 个请求/轮，不是 13**。本方案据此把 T4 的预期从 13 改为 14，并把"13"从文档中删除。

计算公式（全部来自代码，不是估算）：

| 量 | 公式/值 | 出处 |
|---|---|---|
| NCK 变量规范 | 固定 **10 字节**（`0x12 0x08 <syntax> <areaunit> col(2) line(2) module line_count`） | `codec.rs:73-84` |
| 每点期望返回字节 `exp` | `count × decode.byte_len()`（F64=8、U32/F32/I32=4、U16/I16/UTF16=2、U8=1、UTF8_FIXED_16=16、NUL_LAST_32=32、NUL_FIRST_66=66） | `native.rs:375-380`；`value.rs:42-55` |
| 驱动分组预算 | `budget = negotiated_pdu - 32`，`pdu=480 ⇒ 448` | `read_var.rs:33`、`:108` |
| 单项规划开销 | 首项 `10+exp`，其余 `10+exp+4`（4 = 单项响应头） | `read_var.rs:103-129` |
| 单 item 上限检查 | `10+exp+32 ≤ 480`（本点表最坏 = 108，通过） | `read_var.rs:78-96` |
| 真实请求字节（每批 n 项） | `TPKT4+COTP3+S7头10+参数2 + 10n` | `read_var.rs:136-148` |
| 真实响应字节（每批 n 项） | `TPKT4+COTP3+S7头12+参数2 + 4n + Σexp + 奇数长度补位` | `read_var.rs:159-180`、`:193-244` |

真实点表统计（238 点，来自 `GET /tasks?endpoint=`，见 `target/validation/t0-rev13-tasks.json`）：

- `Σexp = 1798 B`；`exp` 分布：8 B×146、1 B×40、4 B×29、2 B×8、16 B×8、50 B×4、32 B×2、66 B×1。
- 按 `chunks(19)` 顺序分 13 组（12 组 19 点 + 末组 10 点）后逐组算规划字节：

| 组（点序号） | 规划字节 | 真实请求 B | 真实响应 B | 是否超 448 |
|---|---|---|---|---|
| 0–18 | **488** | 209 | 323 | **是 → 拆成 17+2** |
| 19–37 | 408 | 209 | 243 | 否 |
| 38–56 / 57–75 / 76–94 / 95–113 / 114–132 / 133–151 | 414（各） | 209 | 249 | 否 |
| 152–170 | 350 | 209 | 185 | 否 |
| 171–189 | 373 | 209 | 213 | 否 |
| 190–208 | 424 | 209 | 273 | 否（余量 24） |
| 209–227 | 375 | 209 | 225 | 否 |
| 228–237（10 点） | 176 | 119 | 107 | 否 |

⇒ **每轮请求数 = 12 组未拆（12）+ 第 1 组拆成 2（17 项、2 项）+ 末组 1 = 14**。
（`max_read_items=19` 的其它副作用：实际批大小集合 = {2, 10, 17, 19}——不是整齐的 19。）

**元数据**：`S7_MAX_ITEMS_PER_PDU = 19` 是驱动内的**经验常数**（`read_var.rs:28-29`，注释写明"PDU 480 下按 12 字节/item + 头估算的经验值"），**不是西门子规范值**；见 §10 的官方上限声明。

**T4 若要"恰好 13 个请求"**：只能调整点表顺序把 66/32 B 的大点移出第 1 组——这是**生产配置变更**（`PUT /tasks`，revision 递增），不属于"只改一个变量"的受控实验，本方案**不采用**。T4 就按 14 请求跑。

### 6.1 判定矩阵（结果一律用"支持 / 削弱 / 无法区分"）

| 步 | 点集 | 连接 | 每轮请求（确定）／推算速率（标注"推算"） | 时长 | 结果 A → 判定/下一步 | 结果 B → 判定/下一步 |
|---|---|---|---|---|---|---|
| **T2** | `S1`(4) | 现参数 | **4**／推算，见 §1.6 | 3 min | **稳定**（全程 RUNNING、4/4 GOOD）→ **削弱**"低负载下必死"；**不能**据此支持 H1（只能说低负载这次没复现）。进 T3 | **`0x8201` 死亡** → 4 点内已有触发源（H3 类）或状态拒绝（H2）→ **支持 H3/H2 的"多点内即有毒元素"**；**跳过 T3/T4**，直接 T5 |
| **T3** | `S0`(238) | 现参数，`interval=5000` | **238**／推算 | 5 min | **稳定** → **支持 H1**（同地址集合低频正常），但**不能**区分速率/突发/累计。进 T4 | **死亡** → **削弱** H1 的"瞬时速率"版（低频也不行）；**不能**排除累计版。进 T5 |
| **T4** | `S0`(238) | `max_read_items=19`，`interval=1000` | **14**（§6.0.1 已算）／推算 | 5 min | **稳定** → **支持**"请求数是关键变量"，H1 细化为"请求数相关" | **死亡** → **削弱**"请求数唯一"，转 T4b |
| **T4b**（仅 T4 死亡时） | `S0`(238) | `max_read_items=1`，`interval=2000` | **238**／推算 | 3 min | **稳定** → 仅能说"2 s 档这次可跑"，**不能**定阈值；判"**无法区分**"并记录 | **死亡** → 阈值≤T3 档，**支持**"低频也不够"（对 H1 不利） |
| **T5** | `S2`(3) | 现参数，`interval=1000` | **3**／推算 | 5 min | **稳定** → **支持 H3**（新增语义点是触发源）；进 T6 点名 | **死亡** → **削弱** H3 的"新增点"版本；进 T6 |
| **T6** | `S3`(1) | 现参数，`interval=1000` | **1**／推算 | 5 min | **稳定** → 需多点或更高频才触发 → 回 T4 判定；H2 **无法区分** | **死亡** → **支持 H2 候选**（单点、每轮 1 请求仍拒）→ §6.3 状态核对 |

**再强调**：以上每一格都只判"支持/削弱/无法区分"。即使 T2→T6 全部走完且结果自洽，也**只是**"某一解释与全部观察相容"，**不等于**根因确认；根因确认需要受控对照 + 请求级证据（当前没有）。

非 `0x8201` 的死亡（`READ_TIMEOUT`、连接失败）**不算证据**：记录、允许同配置重试 1 次；连续 2 次则中止（另一类故障）。

### 6.2 每次运行的统一流程与命令

填机床状态表 → Stop → PUT 任务 → Start → 起 §9 watch → 到时或命中 Stop → Stop → 归档。

```powershell
$src=Get-Content "$dir/01-tasks-rev13.json" -Raw|ConvertFrom-Json; $task=$src.tasks[0]
$native=@("native.C.1.122.5.1","native.A.1.26.30110.1","native.C.1.127.13.1")
$pts=$task.binding.config.points; $sem=@($pts|Where-Object{$_.kind -eq "nck_variable"})[0]
$task.binding.config.points=@(@($pts|Where-Object{$native -contains $_.point_key})+@($sem))  # S2 去掉 +@($sem)
$task.schedule=@{mode="poll";interval_ms=1000}                                                # 档位：1000/2000/5000
Invoke-RestMethod -Method Put "$base/api/v1/tasks/$ep" -ContentType "application/json" `
  -Body (@{tasks=@($task)}|ConvertTo-Json -Depth 50) | ConvertTo-Json -Depth 6 | Set-Content "$dir/20-tasks.json"
Invoke-RestMethod -Method Post "$base/api/v1/endpoints/$ep/start" | Out-Null
```

T3/T4 用全量：先 `PUT` 回 `01-tasks-rev13.json` 的 `tasks`（只改 `interval_ms`），再按需改连接（**必须停机**）：

```powershell
$e=Invoke-RestMethod "$base/api/v1/endpoints/$ep"; $c=$e.connection; $c.max_read_items=19
Invoke-RestMethod -Method Put "$base/api/v1/endpoints/$ep" -ContentType "application/json" `
  -Body (@{name=$e.name;device_id=$e.device_id;connection=$c}|ConvertTo-Json -Depth 10)
```

### 6.3 H2 的状态对照（仅 T6 死亡分支；**禁止人为制造机床状态**）

**硬性禁令（评审意见 5）**：**不允许**为验证 H2 而启动程序、装刀、移动轴、切换运行模式、复位报警、插拔使能。任何"制造条件 Y"的动作都是**生产操作**，不在本方案的只读边界内。

因此 H2 的对照只能用**已经发生/已经记录**的状态：

| 可用材料 | 来源 | 能做什么 |
|---|---|---|
| HMI 报警日志、急停/断电记录（时间点） | **现场**（Mesa 无此数据） | 与运行时段对齐：失败是否总落在某类状态下 |
| 事后能拿到的"机床当时在做什么"的口述 | 现场人员 | 交叉核对，标注"口述，非仪器记录" |
| 运行期状态轮询（`points/latest` 里的 `channel.program.status`、`feed.actual` 等） | Mesa | 只反映**采集到的**状态，而采集本身可能失败 → **不能**作为状态真值（循环论证） |

判定（三态）：

- 同一地址/频率下，**历史记录的**两种状态对应的失败结果相反，且状态差异有 HMI 记录佐证 → **支持 H2**；
- 所有可得记录都处在同一状态 → **无法区分**（不是"削弱"，因为状态根本没变过）；
- 同一状态下反复死 → **削弱**"纯状态"解释。

**若现场无法提供状态记录**：本步骤直接记为"**无法区分**"并结束，**不得**为了做对照而操作机床。

### 6.4 为什么这是最少步数

每步只改**一个**变量（点数 / 频率 / 批量 / 机床状态），避免不可归因。路径 A（H1）：T2→T3 两次定性，T4 顺带给出修复方案；路径 B（H3）：T2 已把范围压到 4 点，T5 去掉语义点即定性，T6 仅在需要点名时跑；路径 C（H2）：T6 + 一次状态对照，不再增点集。任何分支 ≤ **5 次启停**、累计采集 ≤ **25 min**。

## 7. Stop 条件（可判定 + 可立刻执行）

| 编号 | 触发（可判定） | 判据来源 | 动作 |
|---|---|---|---|
| **S1** | `state ∈ {RECONNECTING, FAILED}`，或 `detail` 含 `S7_JOB_0x8201` | `GET /endpoints/{id}/state`（`core-api:1508-1516`，字段 `snapshot.rs:84-95`） | 立即 Stop，本档作废 |
| **S2** | 一次运行内**第 2 次** `0x8201`（重连后又死） | 同上 | 立即 Stop，判"可复现"，不再重试 |
| **S3** | 机床侧任一异常：报警、急停、伺服掉使能、异响、操作员要求停 | 人工 | 立即 Stop（人工优先于脚本） |
| **S4** | 非预期时刻 `STOPPED`，或该端点 `points/latest` 条目数 < 预期点数 | 同上 + `points/latest` | 立即 Stop，查 Core 日志 |
| **S5** | 达到本次时长上限 | 计时 | 正常收尾：Stop + 归档 |
| **S6** | `RECONNECTING` 持续 30 s 未恢复 | 同上 | Stop（避免无界重连） |
| **S7** | mesad 不可达 / 状态查询连续 3 次失败 | HTTP | 先排除网络抖动；由上级决定 |

**一键止损（口径已修正：`Stop` 慢，客户机必须给足超时）**：

```powershell
$ep="lx-nck-tags-190-connection"; $base="http://127.0.0.1:8132"
# Stop 可能长达数十秒（§7.1），客户端超时必须 >=90s，否则你会先超时、以为没停
Invoke-RestMethod -Method Post "$base/api/v1/endpoints/$ep/stop" -TimeoutSec 120
Invoke-RestMethod "$base/api/v1/endpoints/$ep/state" -TimeoutSec 10
Get-Process mesa-driver-sinumerik-nck -ErrorAction SilentlyContinue
Get-NetTCPConnection -RemoteAddress 192.168.15.60 -RemotePort 102 -ErrorAction SilentlyContinue
```

> **"秒级发现"≠"秒级停止"（评审意见 1）**：状态轮询 1 s 内就发现异常，是**发现**速度；Stop 的**执行**速度由 §7.1 的 teardown 预算决定，最坏 ≈35 s（含未被取消覆盖的阶段可达 ≈75 s）。因此 Stop 的验收判据**不能**写成"3 秒内进程消失"。

### 7.1 Stop 机制静态核验（只读代码，未做真机试停）

**调用链（`文件:行号`）**：

| # | 环节 | 位置 | 关键行为 |
|---|---|---|---|
| 1 | HTTP 路由 | `core-api:3323` | `POST /api/v1/endpoints/{id}/stop` → `stop_endpoint` |
| 2 | Handler | `core-api:2165-2191` | 未运行 → 直接 `set_desired_running(false)` 返回 200（幂等，`was_running=false`）；运行中 → `manager.stop_endpoint().await`，**无论成败**都 `set_desired_running(false)`（`:2180`） |
| 3 | Manager | `manager.rs:534-545` | 摘除 entry → `cancel()` → **`await` 任务结束**（无外层 timeout，不 detach） |
| 4 | Endpoint 任务 | `endpoint.rs:279-291`、`:370-376`、`:938` | 循环顶部检查、退避 `select!`、`event_loop` 的 `select!` 都能立刻看到取消 → `AttemptOutcome::Shutdown`（`:307-318`） |
| 5 | Teardown | `endpoint.rs:624-659` | post Shutdown（≤5 s）→ `terminate_after_shutdown` → reader barrier → 关 ingress → 摘 registry |
| 6 | 进程终止 | `process.rs:143-154`、`:119-130`、`:110-112` | 先等 5 s 自然退出；不行关 stdin 再等 5 s；再不行 `start_kill()` |
| 7 | 兜底 | `process.rs:74`、`:79-82`、`:365-375` | `kill_on_drop(true)` + Windows Job Object `KILL_ON_JOB_CLOSE` → mesad 退出则驱动必死 |

**最坏耗时由这些上界叠加**：

| 阶段 | 代码上界 | 出处 |
|---|---|---|
| 停产请求 `post(Shutdown)` | **5 s** | `endpoint.rs:604` |
| terminate 第 1 阶段（等自然退出） | **5 s** | `process.rs:21`（`TERMINATE_GRACE`）、`:144` |
| terminate 第 2 阶段（关 stdin 后再等） | **5 s** | `process.rs:119-129` |
| 强杀后的 `child.wait()` | **无 timeout（代码上无界）** | `process.rs:127` |
| reader barrier | **5 s** | `endpoint.rs:644` |
| ingress drain | **15 s** = `EVENT_STORE_BUSY_TIMEOUT_MS(5000)` + `INGRESS_DRAIN_MARGIN_MS(10000)` | `endpoint.rs:592-598`；`event-store:264-270` |
| **teardown 小计** | **≈35 s**（+ 强杀等待） | `endpoint.rs:621` 注释同口径 |
| **取消未覆盖的阶段**：驱动连接/握手/配置闭环 | `DRIVER_STARTUP_TIMEOUT` **6 s** + 4 个 RPC 各 `REQUEST_TIMEOUT` **10 s** = **≈46 s** | `session.rs:302`、`:25`、`:1003`；`endpoint.rs:703-849`（Open→Configure→Apply→Start 四次 `session.call`） |

⇒ **理论上界 ≈ 46 + 35 ≈ 81 s**（取整报 **≈75–80 s**）；**典型路径**（任务在 `event_loop` 或退避 sleep 里）通常 **< 1 s**（T0 实测那次 stop 落在退避分支，detail 记为 `core shutdown`，`endpoint.rs:373`）。HTTP 层**没有**全局 TimeoutLayer（`core-api` 路由未配置超时中间件），所以 Stop 请求会一直挂到 teardown 结束——**风险在客户端超时，不在服务端**。

**Stop 能否可靠切断重连？（评审问题，逐条证明）**

| 论点 | 证据 |
|---|---|
| 不再重连 | `desired_running=false` 在返回前落库（`core-api:2180`；`config-store:1178-1190`）；开机只恢复 `desired_running=true` 的端点（`main.rs:140-143`） |
| 循环退出 | 取消令牌在循环顶部/退避/event_loop 三处均可立即观察到（`endpoint.rs:280`、`:372`、`:938`） |
| 无孤儿进程 | Stop `await` 任务结束（`manager.rs:540`）后才返回；进程由 `terminate*` 收尾；Job Object 兜底（`process.rs:365-375`） |
| **但**：非即时 | Stop 期间**正在进行**的 attempt 不会被中途取消——`attempt_session` 是直接 `await`（`endpoint.rs:293-305`），只有 `event_loop` 与退避 `sleep` 用 `select!`。所以"Stop 进行中仍可能完成一次 Start"，只是 Stop 返回后不会再有新 attempt |
| **但**：drain 失败会 500 | `stop_endpoint` 把 drain 错误变 500（`core-api:2186-2190`），**但 desired_running 仍会落 false**（`:2180`）→ 看到 500 不等于没停住，必须用状态复核 |

**结论**：Stop 在"**返回后不复发**"这一意义上可靠；在"**立即切断**"意义上**不可靠**（最长≈75–80 s，且期间可能再 Start 一次）。T1 的 Stop 判据由此改为：**HTTP 200/500 + `state=STOPPED` + `desired_running=false` + 驱动进程空 + 到 `192.168.15.60:102` 无 Established**，五项齐了才算停干净。

### 7.2 应急停止方案（REST 不可达时；**只写方案，不执行**）

| 方案 | 做法 | 风险 | 验证方式 |
|---|---|---|---|
| **E1 只杀驱动进程** | `Stop-Process -Name mesa-driver-sinumerik-nck -Force` | **不足以防复发**：Core 把 EOF 当 `Lost` → 退避 1 s 重建（`endpoint.rs:336-376`），对机床仍持续重连；`kill_on_drop` 只在 Core 侧释放时生效，手动杀不影响期望态 | `Get-Process` 空 **且** 3 s 后再查仍空（否则说明在重建） |
| **E2 阻断出站**（推荐第一动作） | 防火墙阻断 `192.168.15.60:102` 出站（`New-NetFirewallRule ... -Direction Outbound -RemoteAddress 192.168.15.60 -RemotePort 102 -Action Block`） | 需管理员权限；重连循环仍在跑（每秒 SYN 被丢弃），会污染后续诊断；必须**记下规则名并在事后删除** | `Get-NetTCPConnection -RemoteAddress 192.168.15.60 -RemotePort 102` 无 Established；端点 detail 变成 `CONNECT_TIMEOUT`（`s7-transport/session.rs:83-90`） |
| **E3 杀 mesad** | 结束 mesad 进程（或 Ctrl+C 走 `wait_for_interrupt` → `shutdown_all`） | 丢内存态：`points/latest` 全丢、事件 buffer 可能丢；**重启后若 `desired_running` 仍为 true 会自动恢复**（`main.rs:140-143`）→ 杀前/杀后必须确认 false（REST 或 `mesa.db`） | 进程表无 `mesad`/`mesa-driver-*`；重启后 `GET /endpoints` 该端点为 `STOPPED` |
| **E4 拔网线/断交换机口** | 物理隔离 | 影响整条产线/其它设备；不可远程验证；只在 E2 不可用时考虑 | 现场目视 + `ping` |

**建议顺序**：能连 REST → 直接 Stop（§7.1，客户端超时≥120 s）→ 复核四项；连不上 REST → **先 E2 阻断**（对机床零风险），再视需要 E1（减少本机连接尝试），最后才 E3（并在重启前确认 desired_running=false）。

### 7.3 "1 秒轮询可能错过窗口"的评估与更可靠观测

| 会被漏掉 | 不会被漏掉 |
|---|---|
| **短命的 `RUNNING`**（detail 被清空，`endpoint.rs:528-536`）——1 s 采样可能只看到前后两次 `RECONNECTING` | `RECONNECTING` + detail 会一直保留到**下一次 attempt 有结果**（`endpoint.rs:348-356`），通常 ≥1 s，1 s 轮询大概率命中 |
| 同一采样间隔内发生的多次 attempt（T0 实测：10 s 内 ≥4 次 epoch 变化） | 失败 detail 本身（它是"最后一次结果"，不是瞬时事件） |

更可靠的手段与各自局限：

| 手段 | 做法 | 能拿到 | 局限 |
|---|---|---|---|
| 加密轮询 | 200 ms 采 `GET /endpoints/{id}/state` | 漏窗口缩小 5 倍（内存态，成本极低） | 仍无变化缓冲；短命 RUNNING 仍可能漏 |
| **数 epoch** | 每次采样记录 `runtime.epoch`，统计变化次数 | **即使状态被覆盖，epoch 变化 = 发生了新的 StartConnectionAck**（`endpoint.rs:832-845`）→ 可算"每秒 attempt 数" | 只证明 Core 侧开始过一次会话，不证明连到机床 |
| SSE `/points/live` | `core-api:2843-2902`，200 ms 合并刷新（`:2821`） | 断线时 `mark_communication_lost` 会推送全量 BAD 变更（`snapshot.rs:418-423`）→ 给出"何时置丢失" | 合并窗口内变化会被合并；无 transition 历史；慢消费者转全量 |
| Core 控制台 | `endpoint.rs:346` `connection lost`、`:369` `reconnecting` 每 attempt 一行 | 最完整的时序 | **当前未落盘**（mesad 由外层 pwsh `2>&1` 启动）；需先解决日志落盘，属环境变更 |
| 抓包 / 驱动埋点 | pcap 或加时间戳 | 唯一的"实测"级证据 | 本阶段禁止；驱动无 subscriber（`drivers/sinumerik-nck/Cargo.toml:27`） |

**结论**：1 s 轮询**不足以**支撑"第一次失败到重连之间"的精确时序；T1 期间应同时用 **200 ms 轮询 + epoch 变化计数 + `/points/live`**，并在报告里把时间线标为"观测到的时间线（采样分辨率 200 ms）"，不得写成精确时刻。

## 8. 频率/时长分档（速率列只写"每轮请求数"，速率一律标"推算"）

| 档 | interval | 点集 | 每轮请求（确定） | 每次时长 | 用途 |
|---|---|---|---|---|---|
| L0 | 1000 ms | `S1`(4) / `S2`(3) | 4 / 3 | 3–5 min | 低负载门（T2、T5） |
| L1 | 5000 ms | `S0`(238) | 238 | 5 min | 全量低频（T3） |
| L2 | 1000 ms | `S0`(238)+批读 19 | **14**（§6.0.1 逐组算过；不是 13） | 5 min | 批量读判别（T4） |
| L3 | 2000 ms | `S0`(238) | 238 | 3 min | 阈值上界（T4b，可选） |
| L4 | 1000 ms | `S3`(1) | 1 | 5 min | 单点判据（T6） |

**总上限**：单次 ≤5 min；单日累计 ≤30 min；≤5 次启停；两次运行间 ≥30 s 冷却（等旧连接彻底关闭）。

**每次运行必测（口径已修正）**：

| 指标 | 算法 | 必须标注 |
|---|---|---|
| 每轮请求数 | 代码确定（`max_read_items=1` → 238；`=19` → 14） | "确定值" |
| 轮周期 | 相邻**批次** `timestamp_ns` 差（`lib.rs:946-954`、`snapshot.rs:325`） | "推算"（含相位误差，见 §1.6） |
| 请求速率 | `每轮请求数 / 轮周期` | **"推算速率"**，禁止写"实测" |
| 单请求平均耗时 | `轮周期 / 每轮请求数` | "推算" |
| 错误率、错误码分布、GOOD/BAD 计数 | 采样计数 | 相对值可用 |

> 只有当①有抓包 或 ②有驱动级请求时间戳 时，才允许出现"实测请求速率"字段；当前两者都不具备（`drivers/sinumerik-nck/Cargo.toml:27` 无 subscriber），故本方案**全部**速率字段一律写"推算"。

## 9. 日志与报文留存

| # | 内容 | 获取 | 文件 |
|---|---|---|---|
| 1 | 端点配置 + rev13 任务 | `GET /endpoints/{id}`、`GET /tasks?endpoint=` | `00-endpoint.json`、`01-tasks-rev13.json` |
| 2 | 238 点（quality/quality_code/source_label/value_origin） | `GET /points/latest` | `02-points-latest.json` |
| 3 | 端点状态 + 点值时序（1 s，点值含 `timestamp_ns`） | `GET /state`、`GET /points/latest` 过滤端点 | `3x-state.jsonl`、`3x-points.jsonl` |
| 4 | 身份指纹 | `POST /browse` | `11-identity-*.json` |
| 5 | Core 控制台（含 `driver reported Failed …`） | 驱动 `stdout/stderr` 被 `inherit`（`process.rs:70-72`） | `mesad-console.log` |
| 6 | 报文 pcap（可选） | 宿主机抓包，过滤 `host 192.168.15.60 and tcp port 102` | `nck-102.pcapng`（不进仓） |

```powershell
$limit=(Get-Date).AddMinutes(5)
while((Get-Date) -lt $limit){
  $s=Invoke-RestMethod "$base/api/v1/endpoints/$ep/state"; ($s|ConvertTo-Json -Compress)|Add-Content "$dir/3x-state.jsonl"
  if($s.state -in @("RECONNECTING","FAILED") -or "$($s.detail)" -match "0x8201"){
    Invoke-RestMethod -Method Post "$base/api/v1/endpoints/$ep/stop"|Out-Null
    "STOP S1/S2 $(Get-Date -Format o) state=$($s.state) detail=$($s.detail)"|Add-Content "$dir/run-log.md"; break }
  ((Invoke-RestMethod "$base/api/v1/points/latest").points|Where-Object{$_.endpoint_id -eq $ep}|ConvertTo-Json -Compress)|Add-Content "$dir/3x-points.jsonl"
  Start-Sleep -Milliseconds 1000 }
```

- 目录 `target/validation/nck-8201-<UTC>/`（沿用 §3 的 `$base/$ep/$dir`），由 `.gitignore:55`（`/target/validation/`）忽略；pcap 由 `.gitignore:66` 忽略。时间戳一律 UTC（`timestamp_ns` 语义见 `snapshot.rs:120-122`）。
- 保留期：证据 1/2/4 + `run-log.md` 结案后 90 天（必要时评审后转 `docs/evidence/`）；时序文件与 pcap 30 天。
- `run-log.md` 每段固定字段：`档位 / 点集 / interval / max_read_items / 起止(UTC) / 机床状态 / 结果(RUNNING|S1|S2|S5) / 死亡 detail / 轮周期中位数 / 实测请求速率 / 单请求平均耗时 / GOOD:BAD / 判定 / 下一步`。

### 9.1 驱动日志与 hex 的可得性（诚实结论）

- **驱动进程没有 tracing subscriber**：`drivers/sinumerik-nck/Cargo.toml:27` 只有 `tracing`，无 `tracing-subscriber`；`src/main.rs`（47 行）未初始化。故 `client.rs:105,109,116` 的 `warn!` **当前不输出**，`RUST_LOG=debug` 对驱动无效（mesad 的 `EnvFilter` 只作用于 Core，`apps/mesad/src/main.rs:19-22`）。
- 所以"提高驱动日志级别"**在本方案内不可得**；本方案不依赖驱动内部日志，可观测面只有 `state/detail`、`points/latest`、Core 控制台。
- 报文 hex 只能靠宿主机抓包；是否允许 **待确认**。要"每请求 point_key"级证据必须改代码（§12.3），本方案不实施。

## 10. 对机床的影响

| 维度 | 结论 | 依据 |
|---|---|---|
| 性质 / 协议 | **严格只读**（无 `write`/`command` 能力，驱动内无 `write_var`）；动作仅 TCP+COTP+Setup+ReadVar | `lib.rs:12`、`:1260-1261`；`s7-transport/session.rs:166-238` |
| 并发 / PDU | 同一连接串行（`read_var` 一次一块、等待响应），NCK 侧最多 1 个未完成请求；单请求规划预算 `PDU-32=448`，本点表最坏单 item = 108 B | `client.rs:92-99`；`read_var.rs:78-96`、`:103-129` |
| 连接资源 | 正常 1 条 S7 连接；**风险在重连循环**（`had_running_session=true` 时退避归零 → 约 1 s 重试） | `endpoint.rs:357-360`、`:433-464` |
| 实时性影响 | 读取占用 NCK S7 服务时间片，请求越多影响越大；本方案档位（每轮 1–238 请求）不高于事发配置 | §1.6、§8 |
| 副作用 | 无写入、无程序启停、无轴动作；外部可见变化仅"多一条 S7 连接与读请求" | 同上 |

**关于"官方允许的负载上限"——明确未知（评审要求）**：

1. 本方案里出现的 **238 请求/轮**、**`pdu_length=480`**、**T4 的 19 项/批** 全部是**本仓库代码与配置的自选值**，**不是**西门子官方允许的负载上限：
   - `480` 是连接配置里申请的值（合法区间 `240..=960`，`s7-transport/config.rs:25-29`、`sinumerik-nck/config.rs:198-210`），实际由对端在 Setup 时协商（`s7-transport/session.rs:156-160`）；
   - `19` 是驱动里的**经验常数** `S7_MAX_ITEMS_PER_PDU`，注释自称"PDU 480 下按 12 字节/item + 头估算"（`read_var.rs:28-29`）；
   - `ceil(238 / 19) = 13` 只是**批数上界**；实际因首组超规划预算被驱动再拆，**每轮 14 个请求**（§6.0.1）。
2. **828D 的 S7 并发连接数上限、每秒请求上限、累计配额，均无经核实的官方资料**（本仓库内无引用、无厂商答复）。因此：
   - **禁止**在任何报告里写"480 B 是 828D 允许的单请求上限"或"19 项是官方批量上限"；
   - **禁止**写"我们远低于厂商限制"这类结论——不知道限制在哪，就不能说离它多远；
   - 只能说"**本方案自身设定的**规划预算为 448 B，且实测点表下最坏请求 209 B / 响应 323 B"。
3. 要拿到官方上限：需西门子文档或支持答复（§13 第 6 条），当前**待确认**。

## 11. 回退与止损

1. **立刻停并确认停干净**：`POST /api/v1/endpoints/{id}/stop`（`core-api:2165-2191`，幂等；客户端超时 ≥120 s）。Stop 会等 teardown 完成（**最坏 ≈35 s，含取消未覆盖阶段可达 ≈75–80 s**，§7.1），随后四项复核：`state=STOPPED`、`desired_running=false`、`Get-Process mesa-driver-sinumerik-nck` 空、`Get-NetTCPConnection -RemoteAddress 192.168.15.60 -RemotePort 102` 空。**注意**：Stop 返回 500 也可能是"已停住但 drain 报错"，必须看 `desired_running`。
2. **恢复原配置**：`PUT /tasks/{endpoint_id}` 回写 `01-tasks-rev13.json` 的 `tasks`；若 T4 改过连接，`PUT /endpoints/{id}` 把 `max_read_items` 改回 `1`。**revision 会变成 14+，不可能回到 13**（`config-store:1432-1452`）。
3. **保持停机并记录**：定性前不要为"看起来恢复"而重启（只会复现放大循环）；`run-log.md` + `02-points-latest.json` + 时序交上级；若命中 S3，先保机床后保证据。

## 12. 若某假设成立，初步修复建议（**只是建议，不实施**）

### 12.1 H1 成立

- **配置层（当天可做）**：`max_read_items=19` 把每轮 238 请求压到 **14**（§6.0.1 实测计算，不是 13——第 1 组 19 点规划字节 488 > 448，会被驱动再拆成 17+2）；`interval_ms` 提到 2000/5000。均需 Stop → PUT → Start（`core-api:1961-1969`、`:2297-2305`）。若确实需要 13，只能重排点表把大字符串点移出首组——属生产配置变更，需单独评审。
- **代码层（需评审）**：按 PDU 预算动态批量 + 令牌桶整形；把 238 点拆成多任务分频（快变 1 s / 慢变 5 s）。**不建议**只加大 `timeout_ms`（把超时推迟，仍杀会话）。
- **验收口径**：改完后以"每轮请求数 / 轮周期 / 单请求耗时 / 错误率"四项对比，而不是只报"恢复了"。

### 12.2 H2 成立

- 采集策略改"状态门控"：状态相关点独立任务，仅在允许窗口采集；状态不允许时发 BAD（需新质量码）而不是让作业拒绝杀会话。
- Configure 期做"就绪试读"，不通过直接 `FAILED` 并带 point_key，避免上线后每 1 s 重连。新增质量码要走契约评审（当前 `quality_code` 只允许设备原生码，`lib.rs:556-564`）。

### 12.3 H3 成立

- **最高性价比改动**：数据面失败时把"当前 chunk 的 point_key(s)"附进错误 detail（`lib.rs:920-928`；`client.rs` 已知 chunk，`max_read_items=1` 时恰好 1 个 key）。纯诊断增强、不改质量语义，能让下一次故障**一次定位**。
- Configure 期对已知拒绝地址做准入预检（可复用 `identity.rs:74-76` 判据，但必须在数据面语义下重定义：作业级拒绝 ≠ 对象不存在）。**禁止**把作业级拒绝降级为"该点 BAD、其余继续"——那会把地址错误伪装成设备正常，违反 CNC 采集红线（`client.rs:11-14`）。

## 13. 待确认清单（执行前必须有人回答）

**已由本轮 T0 关闭**（不再是问题）：

- ~~2. mesad 是否重启过~~ → **未重启**（PID 5652，14:08:31 启动，`uptime_secs=14400.12`）；238 条地址清单完整。
- ~~4. 3 个语义点的参数~~ → `axis.position.actual_bcs {channel:1,axis:1} rev1`、`channel.feed.actual {channel:1} rev1`、`channel.program.status {channel:1} rev1`（`target/validation/t0-rev13-tasks.json`）。

**仍然需要现场/上级回答**：

1. **搬迁后机床是否已上电、网口是否恢复、IP 是否仍为 `192.168.15.60`**？——T1a 的前置条件（现场核对，不能靠 ping 猜）。
2. 事发时报警/急停/断电记录（HMI 报警日志）？——H2 的唯一合法旁证（§6.3）。
3. 是否允许抓包（`pktmon`/`tcpdump`）并带出 `.pcapng`；谁有权在机床旁执行 Stop，窗口多长。——决定"实测速率"能否成立（§1.6）。
4. 828D 的 S7 并发连接数 / 每秒请求上限（官方文档或西门子支持答复）。**当前未知，禁止用 238/480/19 顶替**（§10）。
5. tick 补偿的精确行为（tokio 1.53.1 `MissedTickBehavior::Skip`）：以轮周期直方图确认，不引用文档推断。
6. Core/驱动日志能否落盘（当前被外层 `pwsh 2>&1` 吞在控制台）？——决定 §7.3 的时序分辨率。

## 14. 本轮修订对照（评审五项 + 两项）

| # | 评审意见 | 落点 | 关键改动 |
|---|---|---|---|
| 1 | Stop 响应速度：不能把"秒级发现"当"秒级停止"；先证明能可靠切断重连 | §7 上方、§7.1、§11.1 | 新增 §7.1：调用链 7 环节 + 最坏耗时表（post 5 s / terminate 5+5 s / reader 5 s / drain 15 s ≈35 s；取消未覆盖的 connect+config ≈46 s ⇒ 合计 ≈75–80 s）+ "可靠"的四项判据 + "返回 500 也可能已停住" |
| 2 | T4 批读 19 必须从真实点表算字节预算 | §6.0.1、§6.1（T4 行）、§8（L2）、§12.1 | 用 `transport_size`/`count`/`decode` → `Σexp=1798 B`；逐组算规划字节；结论：真实请求 209 B、响应 ≤323 B < 480，但首组 488>448 被拆 ⇒ **每轮 14 请求，不是 13** |
| 3 | H1/H2/H3 判据改弱为支持/削弱/无法区分 | §2、§6.1、§5 表 | 三态判据 + "只有受控对照才可能确认根因" + 每种结果的结论边界（支持≠确认、削弱≠排除） |
| 4 | `points/latest.timestamp_ns` 是采样时间，不是请求发送时刻 | §1.6 口径修正表、§8 必测项 | 证明是**批次**时间（`lib.rs:946-954`、`snapshot.rs:325`）、断线不改它（`:413`）；无抓包/驱动埋点 ⇒ 只能写"推算速率" |
| 5 | 不允许为验证 H2 人为启动程序/移动轴 | §6.3 | 改为硬性禁令 + 只看已有 HMI 记录；无记录即判"无法区分"，不得制造状态 |
| 6 | 身份指纹不足 | §5 能力边界 1–4、§3.1 旁证 | 指纹只提高可信度、不能证明唯一身份（`devices` 只有 id/name）；T1a 前必须现场核对；指出设备名写 `192.168.1.1` 而连接是 `192.168.15.60` |
| 7 | 不得把 238/480/19 当西门子官方负载上限 | §10 末段 | 明确"未核实=未知"，禁止"远低于厂商限制"这类表述；480/19 分别是配置申请值与仓库内经验常数 |


## 15. 结论：环境变更导致不可复现（P3 暂停）

**日期**：2026-10-19 · **结论**：`S7_JOB_0x8201` **在本环境下不可复现**，P3 真机诊断**暂停**。

### 15.1 事实

- 原目标机床为 **828D**，端点为 `lx-nck-tags-190-connection`（`host=192.168.15.60`、`remote_tsap=3332`、family `828d`）。
- 该配置（rev13，238 点）**连续正常运行 3 小时 38 分**：mesad 14:08:31 启动 → **17:46:49.645 最后一次成功批次**。
- 随后机床**整机搬迁/断电**。
- 现场确认：**828D 暂时不在现场**；`192.168.15.60` 现疑似为**一台 840D**（非本系统目标机型）。

### 15.2 探针结果（T1a，单次 `/connection-check`，未启动端点、未改配置）

```json
{"reachable": false,
 "warnings": [{"code": "COTP_RECV_FAIL", "message": "NCK 会话建立失败: early eof"}],
 "device": {"family": null, "firmware": null, "model": null, "vendor": null},
 "capabilities": [], "model_confidence": null}
```

### 15.3 推断与排除

| 假设 | 判定 | 依据 |
|---|---|---|
| **H1 速率/负载** | **已排除** | 同一 238 点配置连续稳定运行 3 小时 38 分；速率若为瓶颈不可能先稳定近 4 小时 |
| **H3 特定地址** | **无证据支持** | 无任何"某地址单独失败"的证据；238 点全 BAD 是断线后的统一质量标记 |
| **H2 机床状态** | **不可证伪，且不再是主因** | 连接层在 COTP 阶段即被关闭，未进入 S7 读作业，状态相关假设无从检验 |
| **H4 目标设备已更换** | **成立** | 现场确认 828D 不在；15.60 为另一台设备；探针 `reachable=false` 且设备身份全 `null` |

`S7_JOB_0x8201` 最可能出现在**断电/搬迁的过渡过程**（NCK 半就绪时拒绝 S7 作业），属一次性环境事件，**不应归因为本系统的采集设计缺陷**。

### 15.4 一个正面的安全属性（值得保留）

当把 `family=828d` 的端点指向非 828D 设备时，系统行为是：**`reachable=false` + `COTP_RECV_FAIL` + 设备身份全 `null`** —— 即**明确拒绝把它识别成 828D**，没有产生"看起来连上了"的假象。这是正确的 fail-closed 行为。

### 15.5 后续处置

1. **端点 `lx-nck-tags-190-connection` 保持 STOPPED**；**不得**对现址设备（840D）做任何探测或采集。
2. **T1a–T6 全部不执行**：目标设备已不在此地址，继续测试无诊断价值。
3. **不将 840D 纳入支持范围**：上位设计已明确 840D sl 定义属"范围外（明确不做）"；定义目录亦仅有 `828d`。
4. **若 828D 回归**：先由现场确认其**实际 IP**、上电状态、无报警、铭牌；再按 §6 的 T1a 起重新申请，**从连接层重新验证**，不沿用本次结论。
5. `0x8201` 的**放大链缺陷**（一次作业拒绝 → 整端重连、全点 BAD）**仍然真实存在**，与本次环境事件无关，应在有设备时作为独立健壮性改进处理（见 §7 建议 H3）。

## 16. 重要更正：退避语义与假 RUNNING（**本节覆盖前文相关描述**）

> **本节由 L1/L2 离线止损演练实测得出，优先于文档前文关于"退避封顶"的任何表述。**

### 16.1 退避语义更正

`RECONNECT_BACKOFF_SECS = [1, 2, 5, 10, 30]`（`endpoint.rs:32-37`，索引 `min(4)`）**只适用于"连续失败且每次尝试都未进入 RUNNING"**的情形。

若某次尝试曾进入 `RUNNING`，`backoff_idx` 会**归零**（`endpoint.rs:358-361`：`if had_running_session { backoff_idx = 0 }`），下一次失败重新从 **1 秒**开始。

**因此："30 秒封顶"不是自动收敛保证，而是"连续从未 RUNNING 的失败"才会达到的上限。前文把它描述成"重连会自行退避到 30 秒"，是错误且会低估风险的表述。**

### 16.2 NCK 驱动路径的特殊行为（实测）

**在该路径上 `[1,2,5,10,30]` 阶梯不可达。**

机制（已由代码与实测双向印证）：

```
NckConnection::open_connection  只校验配置，不建立 TCP
driver-sdk 的 Start             先 spawn(run)，随后即回 Ack
真正的 TCP 建连失败             发生在会话已被标记 RUNNING 之后
⇒ Core 每次都置 had_running_session = true
⇒ backoff_idx 每次失败都被归零
⇒ 退避恒为 1 秒，无限重试
```

**实测数据**（L1：连 `127.0.0.1:1`，必然拒绝）：

| 指标 | 实测 |
|---|---|
| 观察窗口 | 58 秒 |
| 失败次数 | **17 次**（初始 1 次 + 重试 16 次） |
| 计划退避 | **始终 1 秒**（从未出现 2/5/10/30 秒） |
| `loss → loss` 间隔 | **3.562 – 3.640 秒** |
| `loss → 下次驱动握手` | 1.515 – 1.577 秒 |

**L2**（emulator 真实 RUNNING 后杀进程）：`loss → loss` **3.576 / 3.581 秒**，同样恒 1 秒。

⇒ **实际节奏约为每 3.58 秒一次重连尝试，且无限持续**，不能依赖退避自行收敛。

### 16.3 假 RUNNING：`RUNNING` 不代表连接健康

**L1 连接 `127.0.0.1:1`（必然拒绝）也短暂进入了 `RUNNING`**：`epoch` 非零、注册 1 个点、`points/latest` **0 条采样**，约 **2.1 秒**后才转为 `RECONNECTING`。

**结论：`state=RUNNING` 的语义是"驱动进程已启动并回 Ack"，不等于"已连上设备"，更不等于"采到了数据"。**

因此**不得**用 `state=RUNNING` 作为采集健康判据。可用于判健康的信号应为 **`points/latest` 中是否存在新鲜数据**（时间戳前进），而非连接状态。

**注**：不能反过来规定"必须采到首个 GOOD 才算 RUNNING" —— 存在**合法地持续返回 BAD** 或**暂时没有采集点**的设备。**连接状态与数据质量必须分别定义。**

### 16.4 Stop 止损能力（实测，可靠）

| 指标 | 实测 |
|---|---|
| Watcher 硬门禁（首轮成功早于 Start） | **77.67 – 141.94 ms**（4 组全部成立） |
| Watcher 实际轮询间隔（目标 200ms） | 199.2 – 219.9 ms，p50 ≈ **213 ms** |
| 检测延迟 | L1 **92.79 ms**（后端 loss → Watcher）；L2 kill → Watcher **352.19 ms**（其中 loss → 触发 183.78 ms） |
| Stop HTTP | **200**，耗时 **1.92 – 12.54 ms** |
| 触发 → `STOPPED` | **5 – 33 ms** |
| 退避取消 | 拦截剩余计划退避 **803.36 – 970.93 ms**，Stop 后 **0 次重试** |
| Watcher 负向自检 | 连续 3 次状态 API 失败 ⇒ 退出码 3（8.69 s），并尽力 Stop |

**结论：Watcher 能可靠发现异常并快速止损；但"异常会自行收敛"不成立。**

### 16.5 T2a 安全前提（新增门禁）

在 **P1 缺陷（假 RUNNING 导致退避持续重置）修复并通过离线回归**之前，**不执行 T2a**。

即便修复后，T2a 仍须满足：

1. Watcher 健康判据为 **`points/latest` 有新鲜数据**，而非 `state=RUNNING`；
2. Stop 落在退避窗口内（或接受中断正在进行的建连）；
3. 显式核验**无孤儿驱动进程**；
4. 已确认 `MESA_RECONNECT_FAST` **未设置**（该测试开关会把退避压成固定 1s，扭曲结果）；
5. 维护窗口内预演真实负载（238 点 / 5s）另行授权。

**若沿用"仅以 `RUNNING` 判健康"，不得执行 T2a。**

### 16.6 演练证据位置

- 报告：`target/validation/l-drill/L1-L2-止损演练报告.md`
- 原始证据：`target/validation/l-drill/run-20261019-200203/evidence.json`
- 脚本：`target/validation/l-drill/watch.ps1`、`drill.py`
- 隔离性核验：生产 6 个库文件 SHA-256 前后一致；生产 mesad 仍为 PID 5652（未被触碰）

### 16.7 P1 缺陷立项

**名称**：P1 — Endpoint 假 RUNNING 导致重连退避持续重置

**性质**：生命周期缺陷（非退避数组参数问题）。退避数组本身无误，**问题在于"达到 RUNNING"的判据过于宽松，导致退避被不恰当地重置**。

**调查方向**：RUNNING 判据、Driver 真实健康、退避重置条件、是否仅 NCK、正确修复层（Core / Driver / 契约）、可复现测试。

**倾向**：先审查 Core 的退避重置条件，而非给 NCK 添加特殊逻辑 —— 若根因在 Core 对 RUNNING 的定义，其他驱动未来同样会中招。

**验收 L1–L6**：L1 从未成功时退避按预期增长；L2 短暂 RUNNING 后失败不得无限高频重连；L3 真正稳定运行后中断仍可正常恢复；L4 Stop 可打断退避并置 `desired_running=false`；L5 无跨 Endpoint 误影响；L6 不破坏正常启动与恢复。

**阈值（何为"稳定运行"）留待生命周期设计审查确定，本阶段不决定。**


## 17. 状态更新（须与 §16 分级理解，不得合并成单一结论）

> 本节记录 §16 之后的进展，用于防止把三件不同的事混为一谈。

### 17.1 三个**必须分开**的状态

| 事项 | 状态 |
|---|---|
| **P1-A1（退避持续归零）** | **已修复并合入 main**。PR #121 的 CI 十项全部通过，Merge Commit `38a2c53d55c3da9055c4d9600c552e04f75370f2`。修复内容：无运行证据的失败尝试不再重置退避。 |
| **`S7_JOB_0x8201`（真机作业级拒绝）** | **原因仍未确定**。A1 **不负责**解决该协议错误，二者无因果关系。 |
| **T2a（真机单点采集）** | **软件修复已合并，但合并后的离线复验与现场执行授权均未完成**，故仍 HOLD。 |

### 17.2 因此对 §16 的补充说明

- §16.5 的 T2a 门禁**仍然有效**：修复已合并不等于门禁已满足，**合并后离线回归**这一步尚未执行。
- §16.7 的缺陷登记状态由"待修复"更新为"**已修复并合并**"，但 §16.7 列出的六项验收 L1–L6 **需在合并后的 main 上重新确认**，不以合并前的本地结果为最终依据。
- §16.3 关于"`RUNNING` 不代表连接健康"的结论**不受 A1 影响**：A1 只改了退避归零判据，**未改变 `RUNNING` 的对外语义**。真正的"设备已连接"仍待未来由 Driver 上报权威连接状态。

### 17.3 仍然存在、且 A1 不解决的局限

- **每次产出一批后立即失败的端点仍会重置为 1 秒**；
- **本修复不证明 `S7_JOB_0x8201` 根因已消除**；
- 零任务零事件端点仍会升到 30s 周期；慢首采端点反复首批前丢失会升到 30s；
- teardown 窗口在途批次可能漏判（只会更慢，不会更快）。

### 17.4 证据边界

- `rust-canonical` 在 CI 中**已执行并通过** `mesad_restart` 与 `opcua_auth_restart_e2e` 两套重启类测试，跨 Driver 生命周期无回归；
- **本地 `target/validation/contract.json` 证据产物仍未产出**（生产实例占用调试二进制）。该缺失属**证据产物缺失**，**不是测试覆盖缺失**，不应在文档中混为一谈。

### 17.5 合并后离线复验结果（T2a L1/L2）

在合并后的 main（`38a2c53d55c3da9055c4d9600c552e04f75370f2`）上，于完全隔离环境完成复验。

**构建基线**：工作树当时停在文档分支，故用 `git archive 38a2c53` 导出基线树构建；关键文件 blob 与基线逐一相等。构建使用独立 `CARGO_TARGET_DIR`，**未替换生产 `mesad.exe`**。`MESA_RECONNECT_FAST` 未设置。

| 项 | 实测 | 判定 |
|---|---|---|
| **L1**（Ack 后立即失败） | `retry_in_secs` = **`[1, 2, 5, 10, 30, 30]`**；6/6 均 `had_running_session=true, saw_runtime_evidence=false`；采样 0 条 | **PASS** |
| **L2**（保持 RUNNING 约 2s 后失败，poll 5s） | `retry_in_secs` = **`[1, 2, 5, 10, 30]`**；5/5 均 `saw_runtime_evidence=false`；采样 0 条 | **PASS** |
| **Stop** | HTTP 200，3.5ms / 3.7ms；`STOPPED`；`desired_running=false`；**35s 内新 attempt = 0** | **PASS** |
| **隔离** | 生产 PID 5652 存活且未变；`mesa.db`/`events.db` SHA-256 前后一致；真实端点仍 `STOPPED` | **PASS** |

⇒ **§16.5 中"合并后离线回归"这一项已满足**。但 **T2a 真机执行仍需单独授权**，本次复验不构成真机放行。

**保留的边界**（不得据此扩大结论）：
- 不证明**所有故障场景**下 Stop 都能在数毫秒内完成；
- 不保证 **Event-only 端点**或**"每批成功后故障"** 不会发生快速重连；
- 不证明 `S7_JOB_0x8201` 根因。

### 17.6 稳定性观察项：windows-x64 间歇失败

main push CI（`38a2c53`）**首次运行**在 `rust-platform (windows-latest, windows-x64)` 失败：

| 用例 | 失败位置 | 性质 |
|---|---|---|
| `event_runtime::exact_500_events_pipeline_exact_within_budget` | `tests/driver-contract/tests/common/mod.rs:614` | `wait_until` 条件等待超时（50ms 轮询） |
| `event_persistence::stop_barrier_drains_inflight_epoch_events` | `tests/driver-contract/tests/event_persistence.rs:629` | `assertion left == right` |

结果 `1091 passed / 2 failed / 18 skipped`（共 1093）。**同一 Git Tree（`566b7a31…`）重跑后全绿，两个用例均再次执行并通过**。

**正式记录**：**Windows x64 首次失败、同一代码树重跑通过的间歇性测试失败，根因未确定。**

**不得**据此宣称"已证明与 A1 无关" —— 两个失败用例都涉及事件处理，而 `event_ingress.rs` 在 A1 修改范围内，**该可能性未排除**。

**升级条件**：若任一用例再次出现同类失败，**立即启动事件处理与排空时序专项调查**，不得继续归类为环境抖动。
