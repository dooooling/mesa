# 质量/错误码可观测性只读调查（SINUMERIK 828D 80 点项级 BAD）

- 基线：`main = cbe310f199dd6363e38f4d954501c07eeaa93916`（工作树干净）
- 性质：**只读代码调查**。未改任何生产代码/测试/配置；未启动 mesad/Vite/emulator；未连设备；未跑测试或构建；未做任何 git 写操作。
- 口径：每条结论给 `文件:行号`；代码与仓库内产物不能确定的写 **待确认**，并写明"需要什么观测"，不估算填充。
- 现场输入（用户给定，本次未复核）：80 点长期 BAD = `C.1.21` line 301..426 的 74 个 `F64_LE` + `V.{1,3,4}.26.{27,31}` 的 6 个 `F32_BE`。

## 0. 先回答"错误码到底丢没丢"

**没有在「Driver → Core → REST」主链路上丢失。** 但"点级原因"确实在三处被削弱或覆盖，且前端从不消费。

| 层 | 现状 | 是否丢 |
|---|---|---|
| S7/NCK 传输 + NCK 驱动 | 设备项级 `return_code` 被完整保留为 `quality_code`（仅设备码情形） | 设备码**不丢**；本地失败原因**丢**（只进 warn 日志，而驱动无 subscriber） |
| 契约 | `Quality` 三元枚举无 detail/reason；`PointValue.quality_code: Option<i32>` 已存在，proto 有 `optional sint32 quality_code = 4` | 字段**已存在**，无需新增 |
| Core Snapshot | `LatestEntry.quality_code: Option<String>`，`i32 → 十进制字符串` | 码**保留**，但①进制约定丢②无码的 BAD 被写成 `DEVICE_ERROR`③断线时被整体覆盖 |
| REST | `/points/latest` 与 SSE 都原样序列化 `LatestEntry` | **不丢** |
| 前端 | 类型无该字段、UI 无展示 | 展示层**丢**（运行期行对象仍带该字段） |

**仓库内硬证据（本机产物，非 git 跟踪）**：`target/validation/t0-points-latest.raw.json` 共 238 行，**238/238 都带 `quality_code`**，值全为 `COMMUNICATION_LOST`。所以上一级"`points/latest` 没有 `quality_code`"的描述与 HEAD 代码及现有产物不符；真实情况是**该字段有值，但值被连接级原因覆盖，不含任何项级信息**。

同一产物还精确复现了本次的 80 点：`value_origin = PLACEHOLDER` 恰好 80 行，其中 74 行是 `C.1.21` line 301..426 的 `F64_LE`、6 行是 `V.{1,3,4}.26.{27,31}` 的 `F32_BE`。`PLACEHOLDER` 的判定来自驱动每次 `run()` 内新建的 `last_known`（`drivers/sinumerik-nck/src/lib.rs:905-906`、`:549-555`），只能说明"本次 attempt 内从未拿到 GOOD"，**不能**说明原因。

## 1. 层 1：NCK/S7 传输与驱动（`return_code` 如何变成质量）

| 事实 | 证据 |
|---|---|
| 逐项返回码在响应数据区首字节，`ret != 0xFF` 即该项 BAD，仍跳过该数据区对齐后项 | `crates/s7-transport/src/read_var.rs:190-231`（判定 `:204`，落结果 `:225-230`） |
| 项级 BAD 只 warn，不 Err | `read_var.rs:206`（`tracing::warn!`） |
| NCK 客户端把**三类**失败统一压成"BAD、`data=None`"：① `return_code != 0xFF` ② `transport_size` 与期望不符 ③ 长度与期望不符 | `drivers/sinumerik-nck/src/client.rs:104-129` |
| 只有 `return_code` 被带到上层（`NckReadResult.return_code`） | `client.rs:36-41`、`:125-129` |
| 解码层再叠加本地失败：长度不符、元素解码失败、无效工程值 | `drivers/sinumerik-nck/src/lib.rs:485-512`、`:513-535` |
| 构造 `PointValue`：GOOD → `quality_code: None`；BAD → `quality_code = return_code.filter(!= 0xFF)` | `lib.rs:539-547`（GOOD）、`:556-567`（BAD，注释 `:472-475`） |
| NCK 无协议级 StatusCode / SourceTimestamp，故无更细的质量维度可用 | `drivers/sinumerik-nck/src/value.rs:3-6` |
| `transport_size` 是**用户声明的期望值**，不是由 decode 推导，也未与 decode 做一致性校验 | `drivers/sinumerik-nck/src/native.rs:126-127`、`:155-160`、`:197-212`、`:392`（`resolve_native` 原样拷贝） |

**结论（层 1）**：
1. 设备项级码（S7/NCK item return code）**被保真为 i32**，这是今天唯一能区分"设备拒绝"与其它原因的通道。本仓库只显式定义 0x03/0x04/0x05（`crates/s7-transport/src/error.rs:14-21`），其余码值语义**待确认**。
2. 但另外三类失败（transport 不符、长度不符、解码/工程值失败）在**驱动内部**就被折叠成 `None`，原因只存在于 `tracing::warn!`。
3. 而这些 warn **在当前进程里不会输出**：`drivers/sinumerik-nck/src/main.rs:5-46` 未初始化任何 subscriber，`drivers/sinumerik-nck/Cargo.toml:27` 只有 `tracing` 没有 `tracing-subscriber`，`crates/driver-sdk/src` 内也无 subscriber 安装点。即：**本地失败原因是"写进了一个黑洞"**。
4. 因此"80 点被拒"若属本地校验失败，`quality_code` 为 `None`，该点在 Core 侧被兜底成 `DEVICE_ERROR`，与真正的设备错误**不可区分**。

## 2. 层 2：契约（是否已有承载字段）

| 事实 | 证据 |
|---|---|
| `Quality` 只有 Good/Uncertain/Bad 三态，**没有** `detail`/`reason`/`quality_code` 字段 | `crates/core-types/src/lib.rs:196-201` |
| `PointValue` 已有 `quality_code: Option<i32>`（"协议原生或 Mesa 原因码"） | `crates/core-types/src/lib.rs:352-365`（字段 `:358-359`） |
| wire 字段齐全：`string quality = 3`、`optional sint32 quality_code = 4`、`source_timestamp_ns = 5`、`value_origin = 6` | `proto/driver.proto:266-275` |
| 编解码原样透传，无丢弃 | `crates/driver-protocol/src/lib.rs:260-271`、`:273-286` |
| 冻结方案已要求"`quality_code` 必须表达错误原因"，并预留公共码 | `docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md:553-589`（§3.6）、`:591-622`（§3.7：`COMMUNICATION_LOST`/`TIMEOUT`/`ADDRESS_INVALID`/`DECODE_FAILED`/`TYPE_MISMATCH`/`UNSUPPORTED`/`DEVICE_ERROR`/`NO_CURRENT_VALUE`） |

**关键能力错配（本次调查最容易被忽略的一条）**：§3.7 的公共原因码是**字符串语义**，但驱动→Core 的 `quality_code` 线上类型是 **`sint32`**。因此这些符号码**只可能在 Core 内部生成**（今天也确实只在 `mark_communication_lost` 里生成，见 §3），驱动无法上行符号码。任何"让 NCK 驱动上报 `TYPE_MISMATCH`/`DECODE_FAILED`"的方案，都必须先确定一个 **i32 表示**（命名空间/映射表），否则就得改 wire。OPC UA 驱动已占用负值区（`StatusCode::bits() as i32`，如 `drivers/opcua/src/lib.rs:789`、`:832`），**不能**简单把负数保留给 Mesa。

**丢失点（层 2）**：字段与 wire **都没丢**；损失在语义表达能力——"驱动侧本地原因"没有可上行的表示，且现有 `Option<i32>` 同时混装协议原生码（NCK 字节码、OPC UA u32 status）与 Core 生成的符号码（`String`），类型上不区分来源。

## 3. 层 3：Core Snapshot 与存储

| 事实 | 证据 |
|---|---|
| `LatestEntry.quality_code: Option<String>`，注释明确"协议/ Mesa 原因码" | `crates/driver-manager/src/snapshot.rs:113-127`（字段 `:116-119`） |
| 入 Snapshot 时 `i32 → to_string()`，即**十进制字符串**；无码的 BAD 兜底为 `"DEVICE_ERROR"` | `snapshot.rs:309-328`（`:318-324`） |
| 断线统一覆盖：`quality = "BAD"`、`quality_code = "COMMUNICATION_LOST"` | `snapshot.rs:399-424`（`:404-405`） |
| 断线覆盖是**冻结契约**，不是实现疏漏 | 方案 §3.11 `docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md:791-805`；单测 `tests/driver-contract/tests/data_semantics.rs:440-455` |
| `register_points` 只刷新 key/label/name，不碰 quality | `snapshot.rs:237-269` |

**进入 Snapshot 时丢的东西**：
1. **进制约定**：设备码 `0x05` 在 REST 里是字符串 `"5"`，而协议层/文档/日志一律写 `0x05`（`crates/s7-transport/src/error.rs:14-21`、`read_var.rs:206`）。人工比对时极易误读。
2. **语义归类**：本地校验失败（驱动给 `None`）被写成 `"DEVICE_ERROR"`——把"我们读不懂/配置声明不符"记成"设备错"，这是对 §3.7 已有码的**误用**，直接妨碍本次定位。
3. **项级证据的存活期**：一次断线就把 80 个点各自的原因抹成同一个字符串（契约要求），事后无法回溯（T0 现场就是这种状态）。

## 4. 层 4：API 序列化

| 事实 | 证据 |
|---|---|
| `GET /api/v1/points/latest` 直接返回 `snapshot.latest_all()`，无字段裁剪 | `crates/core-api/src/lib.rs:1571-1576`；路由 `:3302` |
| SSE `/api/v1/points/live` 的首帧与 delta 同样原样序列化 `LatestEntry` | `core-api/src/lib.rs:2823-2835`、`:2843-2902`；路由 `:3303` |
| 端点级 `detail` 承载驱动/作业级错误文本 | `snapshot.rs:83-95`（`EndpointStatus.detail`）；`core-api/src/lib.rs:1508-1516` |

每行实际输出的质量相关字段（`LatestEntry` 全字段序列化）：`quality`、`quality_code`、`value_origin`、`source_timestamp_ns`、`timestamp_ns`，外加身份/展示字段 `endpoint_id`/`point_id`/`point_key`/`key`/`source_label`/`display_name`（`snapshot.rs:97-127`）。其中 `quality_code` 与 `source_timestamp_ns` 标了 `skip_serializing_if = "Option::is_none"`（`:118`、`:125-126`）。

**由此可得一条可直接用于验收的判据**：在 HEAD 上 **BAD 行必然带 `quality_code`**（三选一：设备数字码 / `DEVICE_ERROR` / `COMMUNICATION_LOST`，见 `snapshot.rs:318-324`）；只有 GOOD 行才会缺该字段。所以任何"BAD 且完全没有 `quality_code`"的现场观测，只能来自更旧的构建、或中间被手工裁剪——**不能**作为"码在链路上丢失"的证据。

**与内部类型相比少了什么**：没有少字段。少的是**消费能力**——没有任何"按 quality_code 聚合/统计/翻译"的接口，码只能逐行肉眼扫；也没有任何接口把 Job 级错误挂到点上（它只在 `endpoint.detail` 里）。

## 5. 层 5：前端显示

| 事实 | 证据 |
|---|---|
| 前端类型 `WorkspacePoint` 没有 `quality_code` 字段 | `apps/mesa-web/src/workspace/useDeviceWorkspaceData.ts:20-36` |
| 映射时用对象展开 `{...p}`，运行期仍会带上未知字段，但类型与后续逻辑都不消费 | `useDeviceWorkspaceData.ts:110-131`；`apps/mesa-web/src/workspace/useLivePointsSource.ts:107-131` |
| 派生状态只看 `quality` 三态 | `apps/mesa-web/src/deviceModel.ts:543-546` |
| 状态列只有 GOOD/BAD/STALE/UNKNOWN 标签 | `apps/mesa-web/src/workspace/LiveCells.tsx:86-92`；`DeviceOverview.tsx:52-56` |
| 点详情抽屉只有"状态/数据类型/来源/时间"，无原因码 | `apps/mesa-web/src/components/PointDetailDrawer.tsx:70-78`、`:108-122` |

**结论（层 5）**：显示层确实"丢"了码——但不是序列化丢的，是**从来没人读**。所以"现场看不到原因"至少一半原因在这里，与 §0 的字段存在性无关。

## 6. Job 级 vs Item 级：现状与混同风险

**两条路径在代码上是分开实现的**，这点必须先说清楚：

| 层 | 判定点 | 依据 | 结果 |
|---|---|---|---|
| 传输（Job 级） | ROSCTR=`0x02` 且 header error≠0，另外要求 `param_len=data_len=0` | `crates/s7-transport/src/pdu.rs:154-168` | `Protocol` 错误 `S7_JOB_0x{code:04X}`，**fatal，无 item 索引** |
| 传输（Job/Header） | ROSCTR=`0x03` 且 `[10]`/`[11]` 非 0 | `pdu.rs:182-195`；映射 `error.rs:78-99` | `S7_HEADER_ERR` / `S7_0xNN` |
| 应用（Item 级） | 数据区逐项 `return_code != 0xFF` | `read_var.rs:190-231` | 该项 BAD，**会话存活** |

**但"观测后果"是混同的**，有四处可指认：

1. **Job 级一律 fatal → 整端"断线"**：`read_var.rs:174` 在任何逐项解析之前 `check_ack(...)?`；`drivers/sinumerik-nck/src/client.rs:90-99` 整块返回 `Err`；`drivers/sinumerik-nck/src/lib.rs:920-928` 直接 `return Err`；`:959-985` 取消全部 task；最终 `crates/driver-manager/src/endpoint.rs:386-412` 走 `Lost` → `snapshot.mark_communication_lost`（`:403`）→ **全部点 `COMMUNICATION_LOST`**。结果是：一次"某个地址被 NCK 拒绝"（0x8201 这类）在点上看起来与"网线掉了"完全一样。
2. **同一个设备原因会因报文形状落在不同层**：`max_read_items` 变化或 PDU 重新分组后，同一地址可能从 item 级 BAD（有序号、有码）变成 job 级拒绝（无序号、无码、整端掉线）。分类是按**协议封装**而非按**原因**做的。
3. **Job 级结构化码在 Core 边界被拍平**：`AttemptOutcome::Lost { reason: String, .. }` 只有字符串（`endpoint.rs:441-444`）；`driver-sdk` 把 `Err(e)` 变成 `State(Failed, e.to_string())`，`kind/code` 结构丢失只剩文本（`crates/driver-sdk/src/lib.rs:1536-1544`）；`endpoint.rs:985-992`、`:1103-1104` 进一步拼成 `"{kind}/{code}: {message}"`。之后没有任何代码再解析它。
4. **身份路径有、数据面没有**：身份探针把项级拒绝组织成 `S7_ITEM_0xNN`（`drivers/sinumerik-nck/src/identity.rs:98-107`），并把 `S7_JOB_*`/`S7_ITEM_*` 降级为 `kind="diagnostic"` 的浏览节点（`:74-76`、`:347-361`）。数据面没有等价物。

**是否"把未知 Job 级错误静默降级为普通点级 BAD"？** 现状**没有降级成 `BAD + 无码`**（Job 级是 fatal，不会走 `decode_point`）。但存在一个更隐蔽的等价问题：**它被降级成了"断线"**（`COMMUNICATION_LOST`），而"未知 Job 码"与"真的连不上"在点上不可区分。这正是评审要求"不得静默降级"的那条，只是表现形式不是 plain BAD，而是被误记成 `COMMUNICATION_LOST`。

## 7. 信息消失的准确位置（交付表）

| # | 层 | 消失的是什么 | 准确位置 | 现在能否恢复 |
|---|---|---|---|---|
| L1a | NCK 驱动 | 本地失败**原因**（transport 不符 / 长度不符 / 解码失败 / 无效工程值）只剩 warn | `drivers/sinumerik-nck/src/client.rs:107-124`；`lib.rs:485-535`；`main.rs:5-46` 无 subscriber | 否（日志不落地） |
| L1b | NCK 驱动 | 三类失败折叠为同一 `BAD(data=None)` 形状，只有 `return_code` 带码 | `client.rs:104-129`；`lib.rs:556-567` | 部分（可有码；本地原因不可） |
| L2 | 契约 | §3.7 符号原因码无法从驱动上行（wire 是 `sint32`） | `proto/driver.proto:271`；`crates/core-types/src/lib.rs:359` | 需契约决策 |
| L3a | Core Snapshot | 无码 BAD 被写成 `"DEVICE_ERROR"`，本地失败与设备错误混同 | `crates/driver-manager/src/snapshot.rs:318-324` | 是（改兜底策略） |
| L3b | Core Snapshot | 断线把逐点原因整体覆盖为 `COMMUNICATION_LOST` | `snapshot.rs:399-424`（`:404-405`），契约 `docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md:791-805` | 契约内不可（需保留"断线前原因"才可） |
| L3c | Core Snapshot | 设备码 `i32 → 十进制字符串`，十六进制约定丢失 | `snapshot.rs:318` | 是（格式化即可，但有兼容影响） |
| L4 | Core API | 无按码聚合/翻译接口（不是丢字段） | `crates/core-api/src/lib.rs:1571-1576` | 是（新增只读接口，不改 wire） |
| L5 | 前端 | 完全不读 `quality_code` | `useDeviceWorkspaceData.ts:20-36`；`useLivePointsSource.ts:107-131`；`PointDetailDrawer.tsx:70-78`、`:108-122`；`LiveCells.tsx:86-92` | 是（纯展示） |
| L6 | Core Manager | Job 级 `kind/code` 在 `Lost{reason:String}` 处拍平，之后只有文本 | `crates/driver-manager/src/endpoint.rs:441-444`、`:985-992`、`:1103-1104`；`crates/driver-sdk/src/lib.rs:1536-1544` | 是（内部字段，不改 wire） |

> 归纳：**不是"某一层把码删了"，而是"码只在设备项级这一种情形存活；其余原因分散死在驱动本地、契约类型、断线覆盖、前端四处"**。

## 8. 最小改进方案（优先复用现有契约）

顺序即优先级；**P0 不改代码，且能直接回答本次 80 点的定性问题**。

**P0（测量，零代码）**：在端点 RUNNING 时抓一次 `GET /api/v1/points/latest`，只看这 80 行的 `quality_code`：
- 数字（如 `"5"`/`"3"`/`"10"`）⇒ **设备项级拒绝**，问题在地址/状态/权限，按 `crates/s7-transport/src/error.rs:14-21` 的 0x03/0x04/0x05 语义解读；
- `"DEVICE_ERROR"` ⇒ **本地校验失败**（transport/length/decode），优先怀疑 `native.rs:126-127` 里声明的 `transport_size` 与设备实际不符，或 `decode` 与宽/字节序不符；
- `"COMMUNICATION_LOST"` ⇒ 不是项级问题，样本无效，须重测。

这条分工在**当前 HEAD 上已经成立**，无需改任何代码。仓库里 `target/validation/t2a-exec-watch.log`（2026-10-19 23:36:58–23:38:26，416 次采样全部 `RUNNING`、`detail` 全空、`epoch` 不变）说明曾存在约 90 s 的 **RUNNING 观测窗口**，但当时没有落 `points/latest`。注意按 `docs/design/nck-s7-job-0x8201-diagnosis-plan.md:173` 的既有口径，`RUNNING`/`epoch` 只能证明 Core 收到了 `StartConnectionAck`，**不等于**驱动真的连到机床、更不等于数据面已通过；但它仍是唯一能抓到项级码的机会窗 —— 这正是本次证据缺口，**待确认 → 需补一次抓取**。

**P1（Driver，无 wire 变更）**：在 `decode_point` 之前保留本地判定原因，用**既有** `quality_code` 字段上行一个 Mesa 本地码。需一次契约决策（**待评审**）：
- 选项 A：把 §3.7 的码映射成 i32 命名空间（如 `0x4D45_00xx`，避开 OPC UA 负值区）——纯复用字段、不改 wire，但要新增一张映射表并写进契约；
- 选项 B：不改 i32 语义，仅把本地失败与设备码在**同一字段内**用"负数/保留值"区分——与 OPC UA 冲突，**不建议**；
- 选项 C：维持 `None`，只把本地原因写进"驱动日志落盘"（需给驱动装 subscriber）——改动最小，但日志不是机读契约，仍无法支撑 UI。
> 若评审倾向"零契约变更"，则本次只能落地 P0+P3，P1 挂起。

**P2（Core，无 wire 变更）**：
1. `snapshot.rs:318-324` 的兜底 `DEVICE_ERROR` 改为**只有在确实没有驱动码时**才用，并且对"驱动明确表示本地失败"的情况不再冒用 `DEVICE_ERROR`（依赖 P1 或保持 `None` 时至少把文案区分为"未知（驱动未提供）"）。
2. `AttemptOutcome::Lost` 增加结构化字段（如 `code: Option<String>`，来源即 `endpoint.rs:985-992`/`:1103-1104` 已有的 `kind/code`），`mark_communication_lost` 改为接收原因码：`S7_JOB_*` ⇒ `DEVICE_ERROR`/`ADDRESS_INVALID`，纯传输失败才 ⇒ `COMMUNICATION_LOST`。**这会碰冻结语义 §3.11**，必须单列评审；不改 `proto`、不改 `Quality`。

**P3（前端，纯展示）**：`WorkspacePoint` 增加 `quality_code?: string`；行/抽屉展示"原因码"（数字原样 + 按 0x03/0x04/0x05 与 §3.7 名单给一句话解释）。零后端改动。

**影响面（现有测试/契约）**：
- 已冻结的 wire 字符串：`tests/driver-contract/tests/protocol_negotiation.rs:311` 逐字断言 `PointValueMsg` 字段列表 —— **任何 wire 变更都会破这条**，这也支持"P1 选 A 而不是扩字段"。
- 断线语义单测：`tests/driver-contract/tests/data_semantics.rs:440-455` 断言 `COMMUNICATION_LOST`。
- NCK 项级码单测：`drivers/sinumerik-nck/src/lib.rs:1984-1985`（设备码 0x05 必须保留）。
- NCK 本地失败单测：`drivers/sinumerik-nck/src/lib.rs:2032-2063` 断言"transport 不符 ⇒ `quality_code = None`" —— P1 若改变该行为，**必须同步改这条用例**（这是一条防伪造 255 的回归，不能直接删）。

## 9. 明确不做的事（避免范围膨胀）

- 不新增 `Quality` 的 `detail`/`reason` 字段，不扩 `Quality` 三态。
- 不修改 `proto/driver.proto`（不新增字段、不把 `quality_code` 改成字符串）。
- 不把 Job 级拒绝"按项隔离"——拒绝 ACK 里没有 item 索引（`pdu.rs:154-168`），做不到，也不猜。
- 不把 Job 级错误静默降级为点级普通 BAD；同样不把未知 Job 码继续当成 `COMMUNICATION_LOST` 就完事（列入 P2，但要评审）。
- 不改断线契约本身（`COMMUNICATION_LOST` 覆盖）——若要保留"断线前原因"，须先改 ADR/契约，本轮不碰。
- 不给驱动加 `tracing-subscriber` 作为"解决方案"（属于运行环境变更，且日志不是机读契约；仅在 P1 选项 C 下作为补充讨论）。
- 不做自动地址探测、不自动剔除坏点、不自动改点表。
- 不修改任何冻结设计文档；不在本轮实现 P1/P2/P3。

## 10. 待确认（含所需观测）

| # | 待确认项 | 为什么定不了 | 需要什么观测 |
|---|---|---|---|
| 1 | 这 80 点在 **RUNNING 窗口**的真实 `quality_code` | 现有产物只有断线后的 `COMMUNICATION_LOST`（`target/validation/t0-points-latest.raw.json` 238/238）；`t2a-exec-watch.log` 的 RUNNING 窗口未落点值 | 端点 RUNNING 时抓一次 `points/latest`（只读 GET） |
| 2 | 这 80 点声明的 `transport_size` 是否与设备实际一致 | `transport_size` 是声明值且无一致性校验（`native.rs:126-127`、`:392`），也不在 `points/latest` 里 | 同一次抓取的 `source_label`（含 `decode=`/`transport=`，见 `native.rs:347-361`）+ 驱动侧失败原因（P1）或抓包 |
| 3 | NCK item return code 的厂商语义表 | 仓库内只有 S7 侧的 0x03/0x04/0x05 解释（`error.rs:14-21`、`:78-99`），无 NCK 官方码表 | 厂商文档；或按码分布 + 地址特征反推（不得当作结论） |
| 4 | 828D 是否会对特定 NC 变量做 **job 级**拒绝，且与本 80 点相关 | 历史指纹只有 module 26 的 `V/6` 任意列返回 `0x8201`（`docs/design/nck-s7-job-0x8201-diagnosis-plan.md:139`），与本 80 点的地址不同 | 一次 RUNNING 窗口内是否出现 `S7_JOB_*`（`endpoint.detail`）+ 同时刻 80 点的码 |
| 5 | `quality_code` 的十进制/十六进制约定是否已有外部消费方 | 仓库内无消费方（前端不读、脚本只落盘），但仓外脚本**未知** | 询问现场/确认无外部解析后再定 L3c 是否改格式 |
| 6 | P1 若实施，`sint32` 的 Mesa 本地码命名空间如何划 | OPC UA 已占用负值区（`drivers/opcua/src/lib.rs:789`、`:832`）；§3.7 是字符串语义 | 评审决定（本轮不实现） |

> 注：`docs/design/nck-s7-job-0x8201-diagnosis-plan.md` 的 T0.3（`:155`）早已把"统计 80 个项级 BAD 的 `quality_code` 分布"列为动作，但仓库内检索不到该统计产物——本轮把它确认为**尚未执行的观测**，而不是已完成结论。
