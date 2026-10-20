# PR-D1 质量码可见性设计（复用 `quality_code`：零新增路由、零后端改动）

- 基线：`main = 760942e8532f9990b7dd33dcc10acb40d3d201ac`（PR #127 合并后的实现基线；本文档已对齐该实现）。
- 性质：**只写设计文档**。未改任何生产代码/测试/配置；未启动 mesad/Vite/emulator；未连接设备；未运行测试或构建；未做 git 写操作。
- 上游输入（均已合并，直接引用）：`docs/design/quality-code-observability-investigation.md`（PR #123）、`docs/design/endpoint-running-backoff-p1-review.md`、`docs/design/nck-s7-job-0x8201-diagnosis-plan.md` §16/§17。
- 命名消歧：本文件是 **PR-D1 = 质量码可见性**，与 `docs/design/pr-d-startup-review.md`（PR-D = 新建设备向导接线）无关。
- 口径：事实给 `文件:行号`；无法确定的写 **待确认** 并指明"由谁、依据什么确认"（§12），不估算、不猜厂商码表。

## 0. 结论速览

1. **零新增 API 路由、零后端改动。** `quality_code` 已在 `/api/v1/points/latest` 与 SSE 中原样输出（`crates/core-api/src/lib.rs:1571-1576`、`:2823-2835`），前端经 `{...p}` 展开时**运行期已带着该字段**（`useDeviceWorkspaceData.ts:110-131`），只是类型不声明、没人读。D1 只补"类型 + 展示 + 前端聚合"。
2. 统计与导出都**在浏览器内用既有 GET 完成**（§2.3 逐条论证为什么新端点无收益），因此 **Driver、Snapshot、Wire 契约零改动**，热路径天然不受影响（§8.1）。
3. `quality_code` 是**异构的十进制字符串**：可能是 S7 item 返回码、OPC UA `StatusCode` 位模式、FOCAS2 的 Mesa 分类码，或 Core 写的符号标记。仅凭数值无法判定语义，必须与 `driver_id` 组成二元组（§4.3）。
4. 真正会被写出的符号标记只有 **2 个**：`COMMUNICATION_LOST`（`crates/driver-manager/src/snapshot.rs:405`）、`DEVICE_ERROR`（`:318-324`）。方案 §3.7 其余 6 个公共码**预留未实现**（`docs/Mesa_驱动自描述动态UI采集控制完整开发实施方案.md:591-604`），D1 不得展示成已有能力。
5. **S7 驱动不上报设备码**（`drivers/s7/src/client.rs:213-221` 丢弃 `return_code`，`drivers/s7/` 无任何 `quality_code` 赋值）；能带设备码的只有 NCK。"S7 端点上设备说 X"的文案**恒为假**。
6. **必须先修的展示层缺陷**：`pointFieldFingerprint` 不含 `quality_code`/`value_origin`（`apps/mesa-web/src/deviceModel.ts:591-603`），导致 `BAD(DEVICE_ERROR,PLACEHOLDER)` → `BAD(COMMUNICATION_LOST,PLACEHOLDER)` 这种"只改码"的覆盖在 UI 上**不刷新**（`:640-648` 保旧引用）——恰好掩盖 D1 要暴露的事实。
7. D1 只让**今后**的码可见，**不能**回溯解释那 80 个已被覆盖的点；统计只有当前快照、没有历史（§7.2、§11）。
8. **时间戳精度**：Unix ns ≈ `1.8e18` 超出 `Number.MAX_SAFE_INTEGER`，`Date.now()*1e6` 与 `JSON.parse` 都会丢精度 ⇒ 导出元数据时间戳一律**十进制字符串**（`BigInt` 计算），**原始响应文本 + SHA-256 才是权威证据**，`points[]` 只是结构化副本（§6.6-1）。**前端既有的时间戳精度问题**单列 §11.4（本轮不修，且**不得**据此断言现有界面显示错误）。
9. **`STATE_UNCHANGED` 不等于有效样本**：前后端点状态相同只说明状态没漂移；是否新鲜只能看 `freshness_summary`（`current`/`last_known`/`placeholder`/`unknown`；未知/缺失 `value_origin` **单独计 `unknown`**，不再并入 `placeholder`）。导出用四个字段分离"状态一致性（`state_consistency`）/ 请求完整性（`request_status`）/ 新鲜度分布（`freshness_summary`）/ 证据完整性（`evidence_integrity`）"，另加 `limitations`（§6.3、§6.6-3）。
10. **`request_status=COMPLETE` 不代表数据新鲜、连接健康或设备正常**；它只说明"四个请求都成功且结构符合预期"。失败即不产文件，故**不存在** `FAILED`/`INCOMPLETE` 状态。
11. `CURRENT` 只表达**值的来源**，不表达**采集成功**：`CURRENT + BAD` 必须显示"本次返回失败"，**不得**用成功绿色（§5.3、P1-1）。
12. **统计/分析范围 = 设备 + 连接双重作用域**（`qualityScope.ts` 单一真值）：全部设备×全部连接 = 全范围；选中设备×全部连接 = **该设备的全部连接**（不再跨设备）；选中设备×单连接 = 该连接；全部设备×单连接 = 该连接。**质量状态筛选（GOOD/BAD/STALE）与搜索关键字只影响表格，不进入基础统计范围**（§7.1）。导出前必须在导出按钮同区域看到范围提示（§6.7），文件名含设备维度（§6.5）。
13. **弹层可达性**：antd v6 Popover 的 `content` 传 `null` 会**永不打开**（Tooltip 侧 `noTitle` 强制关闭且不再回调 `onOpenChange`）。原实现因此让「按码分解/导出」整体不可达，既有测试只渲染内容组件、未测触发器（§7.6）。这是实现约束，且必须有**真实点击展开**的回归用例。

## 1. 现状核实：现有 API 实际输出什么

### 1.1 行结构（`LatestEntry`，`crates/driver-manager/src/snapshot.rs:97-127`）

| 字段 | 线上类型 | 语义 / 取值 | 证据 |
|---|---|---|---|
| `endpoint_id` / `point_id` | string / number(u32) | 身份（`point_id` 由 Core 分配并持久化） | `:99-100` |
| `point_key` + `key` | string | 同一值双写（兼容旧 UI） | `:101-104` |
| `source_label` | string? | Driver 人类可读来源（如 `nck://read/C/1/112/33/2/1?…&decode=F64_LE&transport=0x09`） | `:105-108` |
| `display_name` | string? | 用户展示名 | `:109-112` |
| `type` / `value` | string / any | `ValueJson` 被 `#[serde(flatten)]` 摊平，**顶层直接是 `type`+`value`** | `:14-20`、`:113-114` |
| `quality` | string | `"GOOD" \| "UNCERTAIN" \| "BAD"` | `:115`；`crates/core-types/src/lib.rs:203-211` |
| **`quality_code`** | **string** | **十进制字符串**（`i32 → to_string()`）或符号标记 `DEVICE_ERROR`/`COMMUNICATION_LOST` | `:116-119`、`:318-324`、`:405` |
| `timestamp_ns` | number(i64) | 批次时间（UTC Unix ns）；断线**不改** | `:120-121`、`:413-414` |
| `value_origin` | string | `"CURRENT" \| "LAST_KNOWN" \| "PLACEHOLDER"`（已 normalize） | `:122-123`、`:299-303` |
| `source_timestamp_ns` | number? | 协议原始源时间；NCK 恒 `None`；PLACEHOLDER 强制 `None` | `:124-126`、`:304-308` |

### 1.2 三条形态规则（必须写进契约）

1. **缺失 ≠ 空串**：`quality_code`/`source_label`/`display_name`/`source_timestamp_ns` 均 `skip_serializing_if = "Option::is_none"`（`:107`、`:111`、`:118`、`:125`），键不出现即 `None`。
2. **GOOD 行没有 `quality_code`**：GOOD 时驱动给 `None`（`drivers/sinumerik-nck/src/lib.rs:539-547`），Core 只对 `BAD` 兜底（`:318-324`）。本机 `target/validation/qc-capture/20261020-093420-338-early-…-points-latest.raw.json`（4 行、全 GOOD、`RUNNING`）**无一行带该键**。
3. **十进制不重排**：设备码 `0x05` 在 REST 里是 `"5"`（`:318`）。D1 不改格式（改格式需契约决策，见上游 §7 L3c、§10-5），只在**展示层**补 `0x05` 视图。

### 1.3 现有消费面与缺口

| 事实 | 证据 |
|---|---|
| REST 与 SSE 都整行序列化 `LatestEntry`，无裁剪 | `core-api/src/lib.rs:1571-1576`、`:2823-2835`、`:2843-2902` |
| 路由表无任何统计/导出/过滤端点（`/points/latest` 为全局全量） | `:3240-3360`（`:3302-3303` 仅 latest/live） |
| 行内**没有 `driver_id`**（在 endpoint 级） | `:1557-1566`；`snapshot.rs:83-95` |
| **D1 实施前**：前端不消费该字段；抽屉无"原因码"区；状态列只有 GOOD/BAD/STALE。**（D1 已实施后现状：前端已消费，抽屉有采样与原因码区，见 §5.3 / §8.3）** | `useDeviceWorkspaceData.ts:20-36`；`PointDetailDrawer.tsx:70-78`、`:108-122`；`LiveCells.tsx:86-93` |

## 2. 最小侵入：复用清单与"为什么零新增路由"

### 2.1 复用清单（D1 不重造任何一条）

| 能力 | 复用的现有实现 | 证据 |
|---|---|---|
| 实时点位数据 | SSE `GET /api/v1/points/live` + `usePointLiveStream`（首帧全量、delta 整行） | `apps/mesa-web/src/workspace/usePointLiveStream.ts:50-152` |
| 全局/设备点位视图 | `useLivePointsSource`（全局）、`useDeviceWorkspaceData`（设备过滤） | `useLivePointsSource.ts:107-131`；`useDeviceWorkspaceData.ts:110-131` |
| 点位列表 + 分页 | `DeviceLiveData`（现有列）、`GlobalDataPage`（`pageSize:20`） | `DeviceLiveData.tsx:69-111`；`GlobalDataPage.tsx:217-226` |
| 点详情 | `PointDetailDrawer`（三处共用，不复制） | `PointDetailDrawer.tsx:1-140`；调用点 `DeviceWorkspacePage.tsx:145`、`GlobalDataPage.tsx:229-244` |
| 状态/值/年龄单元 | `StatusCell`/`ValueCell`/`AgeCell` | `LiveCells.tsx:60-104` |
| 派生状态 | `derivePointStale`（BAD 优先于 STALE） | `deviceModel.ts:543-547` |
| 计数行（统计入口） | 设备页与全局页**已有**计数行 | `DeviceLiveData.tsx:151`；`GlobalDataPage.tsx:214-216` |
| 端点上下文（含 `driver_id`） | `GET /api/v1/endpoints`（含 `driver_id` 与 `runtime{state,detail,revision,points,epoch}`） | `core-api/src/lib.rs:1518-1569`（`:1557-1566`） |
| 单端点状态 | `GET /api/v1/endpoints/{id}/state` | `core-api/src/lib.rs:1508-1516` |
| 原始快照 | `GET /api/v1/points/latest` | `core-api/src/lib.rs:1571-1576` |
| 版本号 | `GET /api/v1/diagnostics` 的 `version` | `core-api/src/lib.rs:1582-1596`（`:1594`） |
| 采集纪律（导出对齐） | `capture-points.ps1`：时刻→状态→原始响应→事后状态 | `target/validation/capture-points.ps1:15-31` |

### 2.2 D1 唯一新增面（全部在 `apps/mesa-web`）

| # | 文件 | 改动 | 量级（**最初设计估算**） |
|---|---|---|---|
| 1 | `workspace/useDeviceWorkspaceData.ts:20-36`、`deviceModel.ts:557-568` | 类型补 `quality_code?: string`、`value_origin?: string`（**仅类型，运行期字段已存在**） | ~4 行 |
| 2 | `deviceModel.ts:40-45`、`:51-67`；两处 `toView` | `EndpointContext`/`DevicePointView` 补 `driverId`（endpoint 清单本就带 `driver_id`，`useLivePointsSource.ts:71`） | ~8 行 |
| 3 | `deviceModel.ts:591-603` | **修缺陷**：指纹补 `quality_code`、`value_origin`（§7.2 上游文档同款问题） | 2 行 |
| 4 | 新增 `src/qualityCode.ts` | 纯函数：`classifyCode(driverId, code)`、`codeText(...)`（码表，仅文案） | ~70 行 |
| 5 | `components/PointDetailDrawer.tsx:70-78` 后 | 新增"采样"与"原因码"两块 | ~45 行 |
| 6 | 新增 `src/qualityExport.ts` | 纯函数拼装 + 4 次既有 GET（§6.2） | ~90 行 |
| 7 | 新增 `src/components/QualityCodeBreakdown.tsx` | 计数行上的 Popover（按码分解），挂到 2 处既有计数行 | ~50 行 |
| 8 | `LiveCells.tsx:86-93` 旁（可选） | 状态 Tag 后加"有码才显示"的命名空间徽标（不新增列） | ~15 行 |

> **量级说明**：上表"量级"为**最初设计估算**，只表示相对规模，**不是最终行数**。实际实现因增加弹层可达性约束、导出范围对象、未知来源计数与作用域模块等，`qualityCode.ts` / `qualityExport.ts` / `QualityCodeBreakdown.tsx` 等均**显著大于**估算值。**不要用本表核对实现体量**，以代码为准。
>
> 另：第 8 项（行内命名空间徽标）标注为**可选**，本轮**未实施** —— 抽屉与 Popover 已覆盖展示需求（见 §9 U7）。

### 2.3 被否决的"更大方案"（每条给否决理由）

| 备选 | 为什么否决 |
|---|---|
| 新增 `GET /points/quality-stats` | 前端已通过 SSE 持有**全部**点位行（`usePointLiveStream.ts:50-152`），按码聚合是纯 O(N) 内存计算（238 点≈微秒级）。新端点会引入第二份聚合真值、额外鉴权面与一致性口径（服务端快照 vs 客户端快照），**零收益**。 |
| 新增 `GET /points/export`（服务端生成） | 服务端也无法原子读取（行与端点状态是**独立锁**：`snapshot.rs:475-483`、`:208-209`），其一致性并不优于前端依次发 4 个既有 GET；而后者可直接复用 `capture-points.ps1:15-31` 的纪律。服务端导出还要新增"写文件/大小限制/权限"面。 |
| 新建"诊断"独立页面 / 新状态管理子系统 | 评审明令禁止；且抽屉已覆盖单点、计数行已覆盖聚合，第三套展示必然与二者漂移。 |
| 给 `LatestEntry` 加 `driver_id` | 改既有输出，破坏逐字比对用例（`management_api.rs:3755-3777`）与"D1 不改数据面"的自证（§10-3）。`driver_id` 由端点上下文注入即可。 |
| 在服务端做"码 → 语义"翻译 | 把协议知识塞进 Core，违反 AGENTS.md 硬约束"Core 不懂协议"；且码表会随驱动漂移。 |

### 2.4 禁止项（越界即打回）

新增独立页面/路由/Store/Context；在行内发起按点请求；把 `quality_code` 复制进任何新的全局状态；改 `LatestEntry`、`Snapshot`、`proto`、任何 Driver；改 `Quality` 三态；改 `mark_communication_lost`。

## 3. 数据契约（逐字段）

### 3.1 服务端字段：复用、零变更

字段名/类型/可空性见 §1.1（全部已存在）。D1 **不新增、不改名、不改类型、不改缺失语义**。

### 3.2 前端派生字段（只在视图层，不落 API、不复制状态）

| 派生字段 | 类型 | 来源 | 规则 |
|---|---|---|---|
| `driverId` | `string` | `EndpointContext.driverId`（端点清单） | 缺失 → `""` → 码分类降级为 `UNKNOWN`（不解释） |
| `codeClass` | `"MESA_MARKER" \| "DEVICE_S7_ITEM" \| "PROTOCOL_STATUS" \| "MESA_CLASS" \| "UNKNOWN" \| "NONE"` | `(driverId, quality_code)` | 纯函数 `classifyCode`，见 §4.2 |
| `freshness` | `"CURRENT" \| "STALE_VALUE" \| "NO_VALUE" \| "UNKNOWN"` | `value_origin` | `CURRENT`→本次；`LAST_KNOWN`→旧值；`PLACEHOLDER`→无有效值；缺失/未知→`UNKNOWN`（显示"采样状态未知"，不猜来源；`qualityCode.ts:204-212`） |
| `codeText` | `string \| null` | `(codeClass, code)` | 可解释时给文案；否则 `null`（UI 只显示原值） |

**不新增第二份状态**：以上全部由现有行 + 端点清单**即时派生**（`useMemo`），不写回任何 hook/store。

### 3.3 类型与可空性规则

- `quality_code` / `value_origin` / `source_label` / `source_timestamp_ns` / `display_name`：`T | undefined`（**不是** `T | null`，缺失即 `undefined`，禁止用 `""` 兜底）。
- `quality`：必有值（后端恒发）。
- 判空一律用 `x === undefined`，不得用 `!x`（`"0"` 是合法码字符串且为真值，但 `""` 永不出现）。

### 3.4 行示例（原样来自 `/points/latest`）

```json
{ "endpoint_id": "lx-nck-tags-190-connection", "point_id": 261,
  "point_key": "native.C.1.127.13.1", "key": "native.C.1.127.13.1",
  "source_label": "nck://read/C/1/127/13/1/1?mode=current&decode=U16_LE&transport=0x09",
  "type": "u32", "value": 5, "quality": "GOOD",
  "timestamp_ns": 1792460055540482200, "value_origin": "CURRENT" }

{ "endpoint_id": "lx-nck-tags-190-connection", "point_id": 445,
  "point_key": "native.A.1.26.30110.1", "key": "native.A.1.26.30110.1",
  "type": "u32", "value": 0, "quality": "BAD", "quality_code": "5",
  "timestamp_ns": 1792460055540482200, "value_origin": "PLACEHOLDER" }
```

## 4. 质量码解释：完整映射表

### 4.1 谁在写这个字段

| `driver_id` | 语义空间 | 取值 | 证据 |
|---|---|---|---|
| `sinumerik-nck` | **S7 item 返回码**（设备原始字节） | `0..254`（`0xFF` 被过滤） | `drivers/sinumerik-nck/src/lib.rs:556-568`（`:562-564`）；`crates/s7-transport/src/read_var.rs:190-231`（`:204-206`） |
| `s7` | **不上报**（丢弃，仅 `warn!`） | 恒 `None` → Core 兜底 `DEVICE_ERROR` | `drivers/s7/src/client.rs:213-221`、`:272-273` |
| `opcua` | **OPC UA `StatusCode` 位模式**（`u32 as i32`，可为负） | 如 `0x80340000` → `-2143289344` | `drivers/opcua/src/lib.rs:789`、`:818`、`:832`、`:888`、`:918` |
| `focas2` | **Mesa 单点错误分类码**（非厂家码） | `1..5` | `drivers/focas2/src/lib.rs:1660-1688`、`:1504-1511`、`:1525` |
| test-driver / simulator | 未核实 | **待确认**（§12-4） | — |

### 4.2 完整映射表（判定 → 分类 → 展示文案 → 可解释性）

判据只有一条：**仓库内是否存在该码的显式定义**。有定义才允许一句话解释；否则只显示原值。

| 判定条件 | `codeClass` | 展示（码区） | 可解释为设备语义？ | 依据 |
|---|---|---|---|---|
| `quality_code` 缺失 且 `quality="GOOD"` | `NONE` | **不渲染码区** | — | §1.2-2 |
| `quality_code` 缺失 且 `quality="BAD"` | `UNKNOWN` | "未提供（异常形态，请报告）" | 否 | 兜底逻辑应恒写 `DEVICE_ERROR`（`snapshot.rs:318-324`），缺失即异常 |
| `= "DEVICE_ERROR"` | `MESA_MARKER` | 分组"Mesa 标记（不是设备返回码）"：`DEVICE_ERROR` — "驱动未提供数字码" | **否**（Core 本地标记） | `snapshot.rs:318-324` |
| `= "COMMUNICATION_LOST"` | `MESA_MARKER` | 同上分组：`COMMUNICATION_LOST` — "连接丢失，已覆盖该点此前的码"+ "此前的项级码已被覆盖，不可恢复" | **否**（Core 本地标记） | `snapshot.rs:399-424`（`:404-405`） |
| `driver=sinumerik-nck` 且 `code=3` | `DEVICE_S7_ITEM` | `0x03 (3)` — `仓库现有 S7 参考码表：访问拒绝类（可能关联，需结合协议层确认）` | **不直接解释根因**（仅参考类别） | `crates/s7-transport/src/error.rs:18-19`、`:88-91` |
| `driver=sinumerik-nck` 且 `code=4` | `DEVICE_S7_ITEM` | `0x04 (4)` — `仓库现有 S7 参考码表：上下文不支持类（可能关联，需结合协议层确认）` | **不直接解释根因** | `error.rs:16-17`、`:80-83` |
| `driver=sinumerik-nck` 且 `code=5` | `DEVICE_S7_ITEM` | `0x05 (5)` — `仓库现有 S7 参考码表：地址错误类（可能关联，需结合协议层确认）` | **不直接解释根因** | `error.rs:14-15`、`:84-87` |
| `driver=sinumerik-nck` 且 `code=10`（0x0A） | `DEVICE_S7_ITEM` | `0x0A (10)` — **无解释** | **否**（仓库无码表） | 仅 `tests/identity_browse.rs:378-385` 出现该值 |
| `driver=sinumerik-nck` 且其他 `0..255` | `DEVICE_S7_ITEM` | `0xNN (N)` — 无解释 | **否** | 仓库无定义 |
| `driver=opcua` 且任意数字 | `PROTOCOL_STATUS` | `0x80340000 (-2143289344)` — "OPC UA 状态码（位模式），无仓库码表" | **否** | `drivers/opcua/src/lib.rs:789` 等 |
| `driver=focas2` 且 `code=2` | `MESA_CLASS` | "Mesa 分类：不支持/未实现（EW_NOOPT 等）" | 否（**Mesa 分类，非厂家码**） | `focas2/src/lib.rs:1666-1667` |
| `driver=focas2` 且 `code=3` | `MESA_CLASS` | "Mesa 分类：参数类（EW_PARAM/RANGE/NUMBER/LENGTH）" | 否 | `:1668-1673` |
| `driver=focas2` 且 `code=4` | `MESA_CLASS` | "Mesa 分类：数据/属性类（EW_DATA/ATTRIB）" | 否 | `:1674-1675` |
| `driver=focas2` 且 `code=5` | `MESA_CLASS` | "Mesa 分类：连接类（SOCKET/HANDLE/BUSY/NODLL/CLOSED/TIMEOUT）" | 否 | `:1676-1684` |
| `driver=focas2` 且 `code=1` | `MESA_CLASS` | "Mesa 分类：其他/未知" | 否 | `:1685-1687` |
| 其他 driver / 无 `driverId` | `UNKNOWN` | 只显示原值 + "未知命名空间（不解释）" | **否** | 保守：无驱动信息即不解释 |

**唯一可称为"设备返回的码"的只有 `sinumerik-nck` 的 `0..254` 与 `opcua` 的 StatusCode 位模式**；其余全是 Mesa 标记或分类。

> **首版收窄口径（P1-3）**：`0x03`/`0x04`/`0x05` 的说明**只给类别名**，因为仓库码表声明的是 **CPU 侧**语义（`error.rs:14-19`），而 `quality_code` 走的是 **item 级**字节（`read_var.rs:190-231`），二者是否同义**尚未确认**（§12-2）。因此首版：
> - **保留原始码**（`0x03 (3)` 等）；
> - 标注 `仓库现有 S7 参考码表`；
> - **禁止**显示具体根因（见 §4.4 禁用清单）；
> - 未核实部分统一写 `可能关联，需结合协议层确认`。

### 4.3 数值空间冲突（必须用二元组）

`3` 在 `sinumerik-nck` 是 S7 参考码表的访问拒绝类，在 `focas2` 是"参数类错误"（`focas2/src/lib.rs:1668-1673`）。故：展示与统计的最小键是 `(driverId, code)`；UI 必须把码与其命名空间绑定显示；跨驱动聚合**不得**相加同名数字。

### 4.4 文案红线（禁止暗示因果）

允许：`原始码 0x05`、`来源：S7 item 返回码（设备返回）`、`仓库现有 S7 参考码表：地址错误类（可能关联，需结合协议层确认）`。
**禁用清单（逐字禁止出现在任何码区/导出文案）**：`CPU 保护等级`、`安全策略禁止`、`DB 不存在`、`非标准访问`、`优化块`、`越界`、`S7-1200/1500 未放行 PUT/GET`、`权限不足`、`因…而读取失败`、`设备说…`、`导致 BAD`。理由：这些是**具体根因**，仓库码表并未在 item 级确认（§12-1/§12-2），写出来会误导现场人员。
固定脚注（每块码区底部，逐字）：**"展示一个码 ≠ 确定根因；码表仅覆盖仓库内已定义的部分。"**（对齐 `quality-code-observability-investigation.md:137-146`）

## 5. 断线显示规则（逐条件）

### 5.1 断线改了什么（`snapshot.rs:399-424`）

`quality` 强制 `"BAD"`（`:404`）；`quality_code` 强制 `"COMMUNICATION_LOST"`（`:405`，**覆盖**此前的设备码/`DEVICE_ERROR`）；`value_origin` 中 `CURRENT`/`LAST_KNOWN`→`LAST_KNOWN`，`PLACEHOLDER` 保持（`:406-408`）；`source_timestamp_ns` 仅 `PLACEHOLDER` 置 `None`（`:409-411`）；**`timestamp_ns` 与 `value` 不变**（`:413-414`）。

### 5.2 新鲜度只看 `value_origin` + `timestamp_ns`，**不看 `state`**

`RUNNING` 只证明 `StartConnectionAck` 被接受（`endpoint-running-backoff-p1-review.md:37-45`；`nck-s7-job-0x8201-diagnosis-plan.md:645-653`），不得用于判断"这条值是否新鲜"。

### 5.3 逐条件展示表

**`value_origin` 表达"值的来源"，`quality` 表达"质量"——两者不可相互替代。`CURRENT` 只说明值由本次 attempt 产生，不说明采集成功**：`CURRENT + BAD` 是"本次尝试得到的最新一次失败结果"。

| `value_origin` | `quality` | `quality_code` | 采样徽标（文案/色） | 码区 | 时间列 | "不可恢复"提示 |
|---|---|---|---|---|---|---|
| `CURRENT` | `GOOD` | 缺失 | **`当前有效值`**（绿） | 不渲染 | `最后采样时间：…` | 无 |
| `CURRENT` | `BAD` | 数字码 | **`本次返回失败`**（红，**禁用绿色**） | 设备/协议分组 + 命名空间 + 参考类别（若有） | 同上 | 无 |
| `CURRENT` | `BAD` | `DEVICE_ERROR` | **`本次返回失败（未提供数字码）`**（红） | "Mesa 标记"分组 | 同上 | 无 |
| `CURRENT` | `UNCERTAIN` | 数字码 | **`本次返回不确定`**（橙） | 设备/协议分组 | 同上 | 无 |
| `CURRENT` | `BAD` | `COMMUNICATION_LOST` | 理论不可达（`mark_communication_lost` 会把 `CURRENT`→`LAST_KNOWN`，`snapshot.rs:406-408`）；若出现按 `本次返回失败` 渲染并标 `异常形态` | "Mesa 标记"分组 | 同上 | 无 |
| `LAST_KNOWN` | `BAD` | `COMMUNICATION_LOST` | **`旧值，未更新（断线前）`**（灰） | "Mesa 标记"分组 | `最后采样时间：…（非本次 attempt 产生）` | **显示** |
| `LAST_KNOWN` | `BAD` | 数字码 | `旧值，未更新（本次 attempt 未更新该点）`（灰） | 设备/协议分组 | 同上 | 无 |
| `PLACEHOLDER` | `BAD` | `DEVICE_ERROR` | **`无有效值`**（灰） | "Mesa 标记"分组 | `最后采样时间：—`（无有效采样） | 无 |
| `PLACEHOLDER` | `BAD` | `COMMUNICATION_LOST` | `无有效值（且已断线）`（灰） | "Mesa 标记"分组 | 同上 | 显示 |
| **缺失/未知** | 任意 | 任意 | `采样状态未知（服务端未提供 value_origin）`（灰）+ `异常形态` 标签 | 按码规则正常渲染 | `最后采样时间：…（非本次 attempt 产生）` | 无 |

### 5.4 红线

1. **不得**把 `LAST_KNOWN` 渲染成"当前值"；`CURRENT` 以外一律带"未更新/无有效值"字样。
2. **不得**把 `CURRENT + BAD` 渲染成绿色/"本次采样成功"——它只表示"本次 attempt 返回了这个失败结果"（P1-1）。
3. **不得**在 `COMMUNICATION_LOST` 行展示任何设备码解释（原码已被覆盖，展示即编造）。
4. **不得**用 `STALE`（`deviceModel.ts:496`、`:543-547`，30s 阈值）替代新鲜度：`STALE` 答"多久没更新"，`value_origin` 答"这条值是谁的"，`quality` 答"值是否可信"，三者并列显示、互不替代。

## 6. 原始快照导出（前端拼装，零后端）

### 6.1 决策

**前端生成**，文件由浏览器下载；**不落盘到服务端**。理由见 §2.3（服务端无更好一致性、且新增存储/权限面）。数据**不用页面缓存**：点击时重新拉 4 个既有 GET，避免把"SSE 断线后的 last-known"当成服务端快照导出。

### 6.2 生成流程（4 个 GET、2 波、失败即中止）

1. `taken_at_ns = (BigInt(Date.now()) * 1_000_000n).toString()`（**十进制字符串**；`Date.now()` 只有毫秒分辨率，故末 6 位恒为 0，这是**已知且必须标注**的精度上限）。
2. 并行：`GET /api/v1/endpoints` → `state_before`；`GET /api/v1/diagnostics` → `mesa_version`（**必需**；本机只读内存快照，失败即中止，见步骤 5）。同一份 `/endpoints` 响应同时建立**端点 → 设备**映射（`projectEndpointDevices`），供设备作用域过滤；只用读点**前**的清单，不额外发请求（`qualityExport.ts:367-369`、`:239-253`）。
3. `points_read_at_ns = (BigInt(Date.now()) * 1_000_000n).toString()`，然后 `GET /api/v1/points/latest`：先 `await r.text()` 保存**原文**，再 `JSON.parse` 得结构化副本。结构化副本随即按 §7.1 的**设备 + 连接**作用域过滤（`filterPointsInQualityScope`），**原始文本始终不裁剪**（`qualityExport.ts:379-389`）。
4. `GET /api/v1/endpoints` → `state_after`。
5. **四个请求全部为必需**：任一非 2xx、`points.points` 非数组、或响应结构不符预期 → **中止，不产出任何文件**（禁止半份证据）。
6. `request_status` / `state_consistency` / `freshness_summary` / `evidence_integrity` 按 §6.3 计算；**不得**由"前后状态相同"推导出"样本有效"（§6.6-3）。
7. `evidence_integrity.raw_sha256` = `crypto.subtle.digest("SHA-256", new TextEncoder().encode(rawText))`（对**原文 UTF-8 字节**计算；Node 22/浏览器均可用），`raw_utf8_bytes` 用同一份字节数组的 `length`。

### 6.3 逐字段 schema（`mesa.points.export.v1`）

> 时间字段一律 **十进制字符串**（`string`），不用 JSON number：Unix ns 约 `1.8e18` 远超 `Number.MAX_SAFE_INTEGER`（`9_007_199_254_740_991` ≈ `9.0e15`），number 会静默舍入（§6.6-1）。

| 字段 | 类型 | 必填 | 来源 / 规则 |
|---|---|---|---|
| `schema` | string | 是 | 常量 `"mesa.points.export.v1"` |
| `taken_at_ns` | **string** | 是 | 点击时刻 UTC ns 十进制字符串；毫秒分辨率（末 6 位为 `0`） |
| `mesa_version` | string | 是 | `GET /api/v1/diagnostics` 的 `version`（`core-api:1594`）；该请求为必需，失败则不产出文件，故本字段**不可为 null** |
| `scope` | object | 是 | 兼容扁平视图：`{ "device_id": string \| null, "endpoint_id": string \| null, "row_count": number }`；`null` = 该维度不筛选。**权威范围以 `analysis_scope` 为准**（含 `device_ids` 与文案），本字段只是同源标量视图（`qualityExport.ts:130-139`、`:443-447`） |
| `raw_scope` | object | 是 | **原始证据范围**：`{ "source": "GET /api/v1/points/latest", "includes": "ALL_ENDPOINTS", "row_count": n, "description": string }`。`row_count` = 原始响应的**全部行**，与 UI 作用域无关（可能含其他连接/设备）；单独打开 `-raw.json` 时同样带本字段（`qualityExport.ts:95-100`、`:403-409`、`:492-499`） |
| `analysis_scope` | object | 是 | **分析范围**：`{ "device_id": string \| null, "device_ids": string[], "endpoint_id": string \| null, "row_count": n, "description": string }`。`device_ids` = **实际覆盖**的设备集合（由筛选结果反推、去重排序；归属不可证明的点已被排除），可能比 `device_id` 更窄，所选设备下没有任何点时为 `[]`（`qualityExport.ts:111-120`、`:390-416`） |
| `history` | object | 是 | `{ "available": false, "reason": "Snapshot 仅驻留当前最新值，不含时间序列" }` |
| `sampling.evidence_integrity` | object | 是 | **唯一**的证据完整性字段：`{ "raw_preserved": "INLINE" \| "SEPARATE_FILE", "raw_utf8_bytes": number, "raw_sha256": string }`。**不得**再并列新增"完整性/校验和"类同义字段（§6.5） |
| `sampling.endpoint_state_before` | array | 是 | `GET /endpoints` 原始项的 `{id, driver_id, runtime{state,detail,revision,points,epoch}}`，**逐字保留** |
| `sampling.points_read_at_ns` | **string** | 是 | 第 3 步时刻（同上精度） |
| `sampling.endpoint_state_after` | array | 是 | 第 4 步，同结构 |
| `sampling.state_consistency` | string | 是 | `STATE_UNCHANGED` / `STATE_CHANGED_DURING_READ`（**仅描述前后状态**，不代表样本有效） |
| `sampling.request_status` | string | 是 | **仅一个值 `COMPLETE`**（所有请求成功且响应结构符合预期）。失败即不产出文件，故**不存在** `FAILED`/`INCOMPLETE` 等值 |
| `sampling.freshness_summary` | object | 是 | `{ "current": n, "last_known": n, "placeholder": n, "unknown": n }`（按 `value_origin` 统计，单位=点数；**统计对象是 `analysis_scope` 内的 `points[]`，不是原始全量**）。`unknown` = 未知/缺失 `value_origin` 的行数，**不得**并入 `placeholder`（否则把"来源不明"断言成"确认无值"）；四类之和 == `analysis_scope.row_count`（`qualityExport.ts:76-88`、`:264-276`、`:462`） |
| `points` | array | 是 | `GET /points/latest` 的 `points[]` 经 `JSON.parse` 后**再按 §7.1 作用域过滤**得到的**结构化副本**（保序；不重排/不改写行对象/不补默认值）；**纳秒数值可能已被舍入，不作为对账依据**（`qualityExport.ts:380-389`） |
| `points_raw_text` | string | 条件必填 | `/points/latest` 的**原始响应文本**（原始证据）。`sampling.evidence_integrity.raw_preserved="INLINE"` 时必填；`"SEPARATE_FILE"` 时在 `<base>-raw.json` 中 |
| `summary.by_quality` | object | 是 | `{GOOD, UNCERTAIN, BAD, UNKNOWN}` 计数；统计对象 = `analysis_scope` 内的 `points[]` |
| `summary.by_code` | array | 是 | `[{endpoint_id, driver_id, code, origin, count}]`，键 = `(endpoint_id, driver_id, code)`；统计对象同上（`qualityExport.ts:292-321`、`:465-468`） |
| `limitations` | array[string] | 是 | 固定 4 条（§6.4），**含"单次导出不是原子一致性快照"** |

> **`request_status=COMPLETE` 不代表数据新鲜、连接健康或设备正常。** 它只说明"四个请求都成功且结构符合预期"。数据是否新鲜只看 `freshness_summary`，连接是否健康看端点 `runtime.state`（且该状态本身也不等于设备已连通，见 §5.2）。

> **格式兼容（`mesa.points.export.v1` 的增量字段）**：本版新增 `raw_scope`、`analysis_scope`（含 `device_id` / `device_ids` / `endpoint_id` / `row_count`）、`sampling.freshness_summary.unknown`、`scope.device_id`，全部为 **additive**：不重命名、不改变既有字段语义、不删除既有字段（`qualityExport.ts:22-31`）。因此消费方必须：
> 1. **保留未知字段**——解析后重建文档时不得丢弃不认识的字段；
> 2. 做**显式兼容性检查**（探测 `schema` 与所需字段是否存在），不满足时给出可诊断错误，不得静默按旧结构解释；
> 3. **不得假定所有外部解析器都兼容**——严格 Schema 校验程序（`additionalProperties: false` 之类）是否接受这些新增字段**仍属未知**；本仓库不做该保证，也不宣称"所有外部解析器都兼容"。

### 6.4 示例（节选）

```json
{ "schema": "mesa.points.export.v1",
  "taken_at_ns": "1792460055540000000",
  "mesa_version": "0.0.0",
  "scope": { "device_id": "cnc-01", "endpoint_id": "lx-nck-tags-190-connection", "row_count": 238 },
  "raw_scope": {
    "source": "GET /api/v1/points/latest",
    "includes": "ALL_ENDPOINTS",
    "row_count": 240,
    "description": "原始响应包含全部端点（可能含其他连接/设备），未按设备或连接筛选；纳秒级对账以 points_raw_text 为权威" },
  "analysis_scope": {
    "device_id": "cnc-01",
    "device_ids": ["cnc-01"],
    "endpoint_id": "lx-nck-tags-190-connection",
    "row_count": 238,
    "description": "分析仅覆盖设备 cnc-01 的连接 lx-nck-tags-190-connection" },
  "history": { "available": false, "reason": "Snapshot 仅驻留当前最新值，不含时间序列" },
  "sampling": {
    "endpoint_state_before": [{ "id": "lx-nck-tags-190-connection", "driver_id": "sinumerik-nck",
      "runtime": { "state": "RUNNING", "detail": "", "revision": 16, "points": 238, "epoch": 123 } }],
    "points_read_at_ns": "1792460055540000000",
    "endpoint_state_after": [{ "…": "同结构" }],
    "state_consistency": "STATE_UNCHANGED",
    "request_status": "COMPLETE",
    "freshness_summary": { "current": 0, "last_known": 158, "placeholder": 80, "unknown": 0 },
    "evidence_integrity": { "raw_preserved": "INLINE", "raw_utf8_bytes": 486213, "raw_sha256": "…" } },
  "points": [{ "endpoint_id": "…", "point_id": 261, "point_key": "…", "key": "…",
    "type": "u32", "value": 5, "quality": "BAD", "quality_code": "COMMUNICATION_LOST",
    "timestamp_ns": 1792460055540482200, "value_origin": "LAST_KNOWN" }],
  "points_raw_text": "{\"count\":240,\"points\":[…] }",
  "summary": { "by_quality": { "GOOD": 0, "UNCERTAIN": 0, "BAD": 238, "UNKNOWN": 0 },
    "by_code": [{ "endpoint_id": "…", "driver_id": "sinumerik-nck",
      "code": "COMMUNICATION_LOST", "origin": "MESA_MARKER", "count": 238 }] },
  "limitations": ["仅当前快照，不含历史序列",
    "quality_code 语义依赖 driver_id，未知码不解释",
    "断线后的码为 COMMUNICATION_LOST 覆盖值，不可反推原始项级码",
    "单次导出不是原子一致性快照：行与端点状态来自独立读取，且 points 的纳秒数值可能被 JSON 舍入"] }
```

> 注 1：示例沿用本机 `t0` 断线场景的 238 行统计（238/238 `COMMUNICATION_LOST`）。其中 `device_id`/`device_ids` 与 `raw_scope.row_count = 240`（示意另有一个端点 2 行）用于展示**双重作用域**与"原始证据范围 ≠ 分析范围"，**非 t0 实测**；真实导出按实际响应计数。它同时演示 P0-2：`state_consistency="STATE_UNCHANGED"` 但 `freshness_summary.current = 0` —— **前后状态相同绝不等于"本次新鲜采样"**。
> 注 2：示例中 `points[].timestamp_ns` 仍是 **number**——它来自服务端原始 JSON，前端只能原样保留；再次 `JSON.stringify` 落盘时会写成**舍入后的值**，与 `points_raw_text` 内的原文可能相差 ≤128 ns。**权威值请读 `points_raw_text`**（§6.6-1）。
> 注 3：`freshness_summary` 现为**四类**计数，本场景 `current + last_known + placeholder + unknown = 238 == analysis_scope.row_count`。未知/缺失 `value_origin` 只进 `unknown`，**不得**抬高 `placeholder`（§6.3）。

### 6.5 上界与证据完整性（不静默降级）

- **请求次数**：每次点击 **4 个 GET**（2 波，**全部必需**）；不轮询、不重试；任一失败即中止且不产文件。
- **大小阈值按 UTF-8 字节数**：`new TextEncoder().encode(rawText).length`（**不是** `String.length`——后者是 UTF-16 码元数，非 ASCII 内容会低估）。
- **证据完整性只有一个字段 `sampling.evidence_integrity`**（下称 `evidence_integrity`；此前草案把"完整性状态"与"校验和"拆成两个并列字段，现统一为本字段的一个对象，避免重叠定义）：
  - `raw_preserved="INLINE"`（默认，原文 ≤ 5 MiB）：单文件，`points_raw_text` 内嵌。
  - `raw_preserved="SEPARATE_FILE"`（原文 > 5 MiB）：**下载两个文件** —— `<base>-raw.json`（原文 + `raw_scope` + `evidence_integrity` + `taken_at_ns`）与 `<base>-analysis.json`（结构化副本 + 同一 `evidence_integrity`）。UI 必须提示"已拆分为 2 个文件"（`qualityExport.ts:490-510`）。
  - `raw_utf8_bytes`：原文 UTF-8 字节数（触发 `SEPARATE_FILE` 的判据）；`raw_sha256`：原文 UTF-8 字节的 SHA-256（hex 小写），两文件模式下两边相同，用于配对校验。
- **禁止**：省略原文后仍宣称"完整原始证据"；`ANALYSIS_ONLY` / 任何"只留分析"的状态都不存在。
- **文件名（含设备维度）**：`mesa-points-export-<UTC yyyyMMdd-HHmmss>-<deviceId[__endpointId]|all>.json`；`state_consistency=STATE_CHANGED_DURING_READ` 追加 `-statechanged`；`SEPARATE_FILE` 再追加 `-raw` / `-analysis`（`qualityExport.ts:472-478`、`:504-505`）。
  - 作用域段 = `[deviceId, endpointId]` 中非 `null` 者按 `__` 连接；两者都为 `null` 时用 `all`。
  - 例：仅设备 `cnc-01` ⇒ `…-cnc-01.json`；设备 `cnc-01` + 连接 `lx-nck` ⇒ `…-cnc-01__lx-nck.json`；仅连接 ⇒ `…-lx-nck.json`；不筛选 ⇒ `…-all.json`。
  - **目的**：文件名是分享时最先看到的信息，设备筛选必须体现在名字里，否则"只含所选设备"的分析文件会被误当成整库快照（降低误分享风险，`qualityExport.ts:472-474`）。

### 6.6 精度与证据分级（P0-1 / P1-2）

1. **纳秒精度**：Unix ns ≈ `1.8e18`，`Number.MAX_SAFE_INTEGER = 9_007_199_254_740_991`（`9.0e15`）。在该量级 IEEE-754 double 的间隔为 **256 ns**，`JSON.parse` 后的 `timestamp_ns` 可能偏差 ≤128 ns；**`Date.now()*1e6` 还会额外引入整毫秒截断**。故：
   - 导出元数据时间戳用**十进制字符串**（`BigInt` 计算，无精度损失）；
   - `points[].timestamp_ns` 是 number，**只用于展示/排序**；
   - **纳秒级对账一律以 `points_raw_text`（+ `evidence_integrity.raw_sha256`）为权威**；
   - **不得**声称 `points[]` 与原文在纳秒数值上完全一致。
2. **证据分级**：
   - **原始证据** = `points_raw_text` + `evidence_integrity`（`raw_sha256` 字节级可校验、`raw_utf8_bytes` 可核大小、`raw_preserved` 说明存放位置）；
   - **结构化副本** = `points[]` + `summary`（供展示与统计，允许精度损失）。
3. **`STATE_UNCHANGED` 不等于"有效新鲜样本"**：前后状态相同只说明两次读到的 `runtime` 相同；中间可能发生过一次短暂重连（退避窗口内的 `RECONNECTING` 未被两次采样覆盖），且部分行本就可能是上一轮的 `LAST_KNOWN`。因此：
   - **数据来源分布**以 `sampling.freshness_summary`（`current` / `last_known` / `placeholder` / `unknown` 分布）判断；**质量**以逐点 `quality` 判断；**时间新鲜程度**还须结合采样时间与运行窗口。三者均不能仅由 `state_consistency` 或 `request_status` 推导；
   - `request_status=COMPLETE` 只说明"请求都成功"，与新鲜度、连接健康、设备正常**无关**；
   - `state_consistency` 只描述状态是否漂移，**不得**据此把整个导出标为"有效"；
   - `evidence_integrity` 只描述"原文是否完整保留且可校验"，同样不表达数据有效性；
   - `limitations` 必含"单次导出不是原子一致性快照"。

### 6.7 导出前范围提示（契约，不是建议）

`raw_scope` / `analysis_scope` 只存在于导出文件里**不足以**避免误分享：用户必须在**点击导出之前**就看见"原始证据是全量、分析才按范围过滤"。因此界面**必须**在导出按钮**正下方同一视觉区域**常显提示（`data-testid="export-scope-hint"`，`QualityCodeBreakdown.tsx:176-182`）。

文案要点（两行，逐字）：

```
原始响应包含全部端点，可能包括其他设备。
结构化分析仅按所选设备与连接过滤（当前：<范围>）。
```

- `<范围>` 必须与计数行、Popover 顶部**同源**（`qualityScopeText`，如 `CNC-01 · 全部连接`），不得写死"全部连接"、不得省略；
- 提示**常显**（Popover 展开即可见），不得折叠进帮助图标/Tooltip/仅文件内；
- 必须与导出按钮**同一容器**，保证"点导出前"就能看到（`QualityCodeBreakdown.test.tsx:97-106`）；
- 回归用例：`export_scope_hint_is_visible_next_to_export_button`、`export_scope_hint_shows_current_scope`、`导出前可见范围提示…`（§10.2.1）。

## 7. 统计口径

### 7.1 作用域：设备 + 连接双重（唯一真值）

**单一真值**：`apps/mesa-web/src/qualityScope.ts` 的 `makeQualityScope` / `pointInQualityScope` / `filterPointsInQualityScope`。计数行、按码分解 Popover、导出 `analysis_scope` **共用同一函数**，不再各算一遍（`GlobalDataPage.tsx:94-111`、`:289-296`；`DeviceLiveData.tsx:63-98`；`qualityExport.ts:357`、`:385-389`）。

| 设备筛选 | 连接筛选 | 统计与分析范围 |
|---|---|---|
| 全部设备 | 全部连接 | 所有设备、所有连接 |
| 828D-A | 全部连接 | **仅 828D-A 的所有连接** |
| 828D-A | NCK-1 | 仅 NCK-1 |
| 全部设备 | NCK-1 | 仅 NCK-1 |

规则（逐条对应实现，防止口径漂移）：

1. **设备选中 + 连接"全部连接" ⇒ 范围收敛到该设备，不是跨设备。** 旧标签 `全部连接（跨设备）` **已移除**，改为写实标注 `统计范围：CNC-01 · 全部连接`（`qualityScope.ts:96-99`；`GlobalDataPage.tsx:127-136`、`:298-300`；`DeviceLiveData.tsx:191-194`）。
2. **质量状态筛选（GOOD/BAD/STALE）与搜索关键字只影响表格，不进入基础统计范围。** 实现上 `filtered`（表格）与 `scopedPoints`（统计/分解/导出）是两条路径（`GlobalDataPage.tsx:108-111`、`:138-159`；`qualityScope.ts:7-9`）。否则用户搜索一个点后，会误以为整台设备只有一个点。Popover 顶部逐字显示 `统计范围：<范围>（不受质量状态/搜索筛选影响）`（`QualityCodeBreakdown.tsx:139-141`）。
3. **归属不可证明 ⇒ fail-closed。** 端点不在 `/api/v1/endpoints` 清单里时，设备筛选下**排除**该点（`qualityScope.ts:10-12`、`:64-76`）；"无法判断"绝不解释成"可能属于所选设备"（与表格 RC2 修1 同口径）。未选设备（全部设备）时保留，表格以归属列如实展示。
4. **UI 行自带 `deviceId` 时以行为准**，缺失才回落到端点映射（`qualityScope.ts:71-75`）；导出原始行没有 `deviceId`，由 `/endpoints` 的 `device_id` 补齐（零后端改动，`qualityExport.ts:239-253`）。
5. **归一化**：作用域只认 `deviceId` / `endpointId` 两个维度，`"ALL"`、空串、`undefined` 一律视为"不筛选"（`qualityScope.ts:44-56`）。
6. **文案同源**：四种组合的界面标注与导出 `analysis_scope.description` 由 `qualityScopeText` / `qualityScopeDescription` 统一生成（`qualityScope.ts:87-116`），不出现"跨设备"这种与真实范围不符的提示。

### 7.2 边界（当前快照 vs 历史）

| 问题 | D1 能否回答 | 说明 |
|---|---|---|
| 当前各码有多少点？ | **能** | 来自 `/points/latest`（或 SSE 全量），每点 1 个样本 |
| 某点最近 N 次的码分布？ | **不能（服务端无历史）** | `Snapshot` 只有 `latest: HashMap`（`snapshot.rs:155-176`），进程内存、不落盘（`:1-3`）；断线即把全部点码改写为同一字符串（`:399-424`） |
| 昨天/断线前的码？ | **不能** | 覆盖发生在写入时（`:404-405`），Core 不保留旧值 |

现成反例：`target/validation/t0-points-latest.raw.json` 238 行全 `BAD`/`COMMUNICATION_LOST`（`PLACEHOLDER` 80、`LAST_KNOWN` 158）。

### 7.3 计算位置与复杂度

- **位置**：浏览器内，输入是页面**已持有**的 `allPoints`（`useLivePointsSource.ts:107-131`）或设备页 `devicePoints`；统计前先按 §7.1 的**设备 + 连接**作用域过滤（`scopedPoints`），质量状态与搜索词不参与。
- **复杂度**：O(N)，N=点数；238 点 ≈ 238 次 Map 累加（微秒级）；50K 点 ≈ 50K 次（毫秒级），且**只在展开时**发生。
- **键**：`(endpoint_id, driver_id, code)`；`MESA_MARKER` 可另给跨端点合计。
- **禁止**：任何按行/按点发请求的聚合；任何在 SSE 每帧触发的全量聚合。

### 7.4 触发与缓存

| 触发 | 行为 |
|---|---|
| 计数行渲染 | 只算作用域内的 `counts`（`GlobalDataPage.tsx:113-125`、`DeviceLiveData.tsx:88-98`），**不**算按码分解 |
| **点击**计数行（**仅点击，不用悬停**） | 打开 Popover 时才 `useMemo` 计算分解；`useMemo` 依赖 `points` 数组引用（SSE 每帧最多换引用一次）。悬停不展开——避免无意间触发大规模统计 |
| Popover 关闭 | 结果随 memo 保留，不重算；不写任何 store |

### 7.5 限制语（Popover 底部固定文案）

"统计基于当前快照（每点 1 个样本）。Mesa 不保存质量码历史；断线会把该端点全部点的码覆盖为 `COMMUNICATION_LOST`，此前原因不可恢复。"

### 7.6 弹层可达性（实现约束，含强制回归）

**缺陷事实**：第三方弹层组件（antd `^6.6.2`，`apps/mesa-web/package.json:18`）的 Popover 在 `content` 不可渲染（`null`）时**永远打不开**：Tooltip 侧 `noTitle=true` 会强制 `setOpen(false)`，且**不再回调 `onOpenChange`**。原实现 `content={open ? <Content/> : null}` ⇒ 计数行点击无反应，「按码分解/导出」面板在浏览器中**根本无法打开**；既有测试只直接渲染 `QualityCodeBreakdownContent`、**未测触发器**，故未发现（`QualityCodeBreakdown.tsx:200-219`）。

**实现约束**：

1. `content` **必须始终传元素**，不得传 `null` / `false` / 条件表达式的结果；"只在首次展开时挂载"靠 rc-trigger 的懒挂载实现，而不是靠传 `null`（`QualityCodeBreakdown.tsx:205-219`）。
2. "只在点击时计算一次"的意图由懒挂载保证：未展开时内容组件不 mount（`useMemo` 不执行），展开后结果随 memo 保留（§7.4）。
3. 该约束对任何后续弹层（antd Tooltip/Dropdown 同源 rc-trigger）同样适用：条件渲染放在**内容组件内部**，不要放在 `content` 属性上。

**测试要求**：必须有**真实点击展开**的回归用例，**不能只渲染内容组件**。断言至少覆盖：

- 未展开时内容不存在（如 `queryByTestId("export-scope-hint") === null`）→ 点击触发器 → 内容可见（`QualityCodeBreakdown.test.tsx:81-95`）；
- 页面级"点击计数行 → 范围提示可见"（`GlobalDataPage.test.tsx:222-241`）。

## 8. 性能：热路径与请求上界

### 8.1 热路径零改动（可机器验证）

| 路径 | D1 是否触碰 | 证据 |
|---|---|---|
| Driver 采集（`drivers/**`） | **否** | D1 改动面仅 `apps/mesa-web`（§2.2） |
| `Snapshot::apply_batch` 写入（`snapshot.rs:293-342`） | **否** | 同上 |
| `mark_communication_lost`（`snapshot.rs:399-424`） | **否** | 同上 |
| REST/SSE 序列化（`core-api/src/lib.rs`） | **否** | 同上 |
| Wire / proto | **否** | 同上 |

验收判据：`git diff --stat` 中 `crates/**`、`drivers/**`、`proto/**`、`tests/driver-contract/**` 必须为空（§10-1）。

### 8.2 请求次数上界（238 点场景）

| 动作 | 请求数 | 频率 |
|---|---|---|
| 页面稳态 | 1 条 SSE 长连接 + `GET /endpoints` 每 10s 1 次（`DEVICE_ENDPOINTS_POLL_MS`，`useDeviceWorkspaceData.ts:106`） | 与 D1 前**完全一致** |
| 打开点详情 | **0** | 数据已在内存（SSE） |
| 展开按码统计 | **0** | 纯内存聚合（§7.3） |
| 点击导出 | **4**（2 波） | 用户动作触发，无轮询 |

**D1 不引入任何新的周期性请求。**

### 8.3 渲染策略

- 238 行沿用现有 `antd Table` + `pageSize:20`（`GlobalDataPage.tsx:217-226`），**不引入虚拟滚动**（当前行数无需；若未来 >1000 行再单独立项）。
- 详情抽屉**按需渲染**（点击才 `open`，`PointDetailDrawer.tsx:44-49`），关闭即不渲染。
- 码区在**有码或非 GOOD 无码**时渲染（`shouldShowCodeSection`，`qualityCode.ts:113-126`）；GOOD 且无码的行零额外 DOM。
- 行级 bailout 依赖 `reconcilePointSnapshots`（`deviceModel.ts:621-651`）；§2.2-3 的指纹修复会让"仅码变化"的行多一次重渲染——这是**必要**代价（否则看不到变化），且每次最多一行。

### 8.4 复杂度汇总

| 操作 | 复杂度 | 触发 |
|---|---|---|
| 指纹/合并 | O(N)/帧，N=点数（现状已如此） | SSE 帧（≤5 帧/s，`POINT_LIVE_FLUSH_MS`，`core-api:2821`） |
| 按码聚合 | O(N)，仅展开时 | 用户动作 |
| 导出 | O(N) 序列化 + 4 次 GET | 用户动作 |
| 抽屉 | O(1)（单行） | 点击 |

## 9. 前端交互逐条定义

| # | 组件/位置 | 触发/条件 | 展示内容与文案 |
|---|---|---|---|
| U1 | `PointDetailDrawer` 状态区之后（`:70-78` 后） | 抽屉打开 | 新增"采样"区：徽标按 **`(value_origin, quality)` 二维**取 §5.3 文案（`CURRENT+GOOD`→`当前有效值`绿；`CURRENT+BAD`→`本次返回失败`红；`LAST_KNOWN`→`旧值，未更新`灰；`PLACEHOLDER`→`无有效值`灰）+ `最后采样时间：<本地时间>`；非 `CURRENT` 追加 `（非本次 attempt 产生）`；`源时间：<本地时间>` 或 `—（协议未提供）` |
| U2 | 同上 | 有码 **或** 非 GOOD 无码 | 渲染"原因码"区：有码显示码与命名空间；非 GOOD 无码显示 `未提供（异常形态，请报告）`；**GOOD 且无码整块不渲染**（`qualityCode.ts:113-126`、`PointDetailDrawer.tsx:184-226`） |
| U3 | 原因码区 | `codeClass = MESA_MARKER` | 小标题 `Mesa 标记（不是设备返回码）`；正文 `<code>` + 说明；`COMMUNICATION_LOST` 追加 `此前的项级码已被覆盖，不可恢复` |
| U4 | 原因码区 | `codeClass ∈ {DEVICE_S7_ITEM, PROTOCOL_STATUS}` | 小标题 `设备/协议返回码`；正文 `0xNN (N)`（十进制码时）+ 命名空间标签（`S7 item` / `OPC UA StatusCode`）；nck 的 3/4/5 只给 `仓库现有 S7 参考码表：<类别>（可能关联，需结合协议层确认）`；无类别时加 `仓库无该码定义（待确认）`。**不得**出现 §4.4 禁用清单中的任何具体根因 |
| U4b | 原因码区 | `codeClass = MESA_CLASS` | 小标题 **`Mesa 分类（非厂家码）`**；正文 `0xNN (N)` + 命名空间标签 `Mesa 分类` + 分类文案。**不得**与 `设备/协议返回码` 同组（同组会让读者把 Mesa 本地分类当成设备给出的码，`qualityCode.ts:179-195`、`QualityCodeBreakdown.tsx:49-55`） |
| U5 | 原因码区 | `codeClass = UNKNOWN` | 小标题 **`未知命名空间（不解释）`**；只显示原值 + `不解释` |
| U6 | 原因码区底部 | 恒显示（U2 成立时） | 脚注 `展示一个码 ≠ 确定根因；码表仅覆盖仓库内已定义的部分。` |
| U7 | 行内状态列（`LiveCells.tsx:86-93` 后，可选） | `quality_code !== undefined` | Tag 后追加徽标：`nck`/`opcua`/`focas2`/`Mesa`；`title` = 原值；无码则不渲染 |
| U8 | 计数行（`DeviceLiveData.tsx:188-190`、`GlobalDataPage.tsx:289-296`） | **仅点击**（悬停不展开） | 打开 Popover：顶部一行 `统计范围：<范围>（不受质量状态/搜索筛选影响）`（`QualityCodeBreakdown.tsx:139-141`）；按 `Mesa 标记（不是设备返回码）` / `设备/协议返回码` / `Mesa 分类（非厂家码）` / `未知命名空间（不解释）` 分区，按 `(endpoint_id, driver_id, code)` 分组的码计数；底部 §7.5 限制语 |
| U9 | 同一 Popover 内 | 恒显示 | 导出按钮 `导出原始快照`；点击后禁用并显示 `采集中…`；完成触发下载（`raw_preserved="INLINE"` 单文件；`"SEPARATE_FILE"` 两个文件 + 提示 `已拆分为 2 个文件`）；失败弹 `导出失败：<HTTP 状态>（未生成文件）`。范围提示见 U9b（契约 §6.7） |
| U9b | 导出按钮**正下方**（`data-testid="export-scope-hint"`） | 恒显示（Popover 展开即见） | 两行逐字：`原始响应包含全部端点，可能包括其他设备。` / `结构化分析仅按所选设备与连接过滤（当前：<范围>）。`；`<范围>` 与计数行同源；不得折叠/省略（§6.7、`QualityCodeBreakdown.tsx:176-182`） |
| U10 | 导出文件名 | 完成时 | `mesa-points-export-<UTC yyyyMMdd-HHmmss>-<deviceId[__endpointId]\|all>.json`；`state_consistency=STATE_CHANGED_DURING_READ` 时追加 `-statechanged`；`raw_preserved="SEPARATE_FILE"` 追加 `-raw`/`-analysis`（§6.5、`qualityExport.ts:472-478`） |
| U11 | 任意码区 | 恒 | **不出现**任何因果措辞（§4.4 禁用词表）；不出现"设备说" |
| U12 | 采样区 | `value_origin=CURRENT` 且 `quality=BAD` | 徽标必须为红色 `本次返回失败`，**不得**使用绿色/成功语义（P1-1） |

## 10. 回归测试清单（用例名 + 断言点）

### 10.1 既有契约不变（必须全绿，且不改断言）

| 用例/文件 | 断言点 | 证据 |
|---|---|---|
| `data_semantics::disconnect_sets_bad_communication_lost` | 断线后 `quality_code == "COMMUNICATION_LOST"` 逐字 | `data_semantics.rs:426-455` |
| `data_semantics::disconnect_keeps_original_timestamp` | 断线**不改** `timestamp_ns`（§5.1 的既有守门） | `data_semantics.rs:392-422` |
| `data_semantics::bad_value_type_still_matches_descriptor` | BAD 仍携带匹配 typed value | `data_semantics.rs:103-123` |
| `management_api::nck_configure_rejects_identity_scope_at_start` | 失败端点不得出现在 `points/latest` | `management_api.rs:1165-1285`（断言 `:1278-1285`） |
| `management_api::definitions_rpc_opens_no_connection_and_has_no_side_effects` | `latest_before == latest_after` | `management_api.rs:3755-3777` |
| `point_live::point_live_empty_snapshot_ok` / `point_live_batch_then_delta_with_final_value` | SSE 帧形状与 delta 语义 | `point_live.rs:183-192`、`:195-209` |
| `protocol_negotiation::protocol_additive_fields_do_not_alter_existing_messages` | `PointValueMsg` 等既有 message 字段清单逐字（冻结表 `:287-324`，`PointValueMsg` 在 `:311`） | `protocol_negotiation.rs:537-644` |
| `reconnect_backoff` / `stop_lifecycle` | 退避序列 / 启停终态 | `scripts/contract_suites.py:47-51` |

### 10.2 新增前端用例（只加不改）

| # | 用例名 | 文件 | 断言点 |
|---|---|---|---|
| T1 | `classifyCode_device_codes_only_for_known_drivers` | `qualityCode.test.ts` | `("sinumerik-nck","5")→DEVICE_S7_ITEM`；`("opcua","-2143289344")→PROTOCOL_STATUS`；`("focas2","2")→MESA_CLASS`；`("","5")→UNKNOWN` |
| T2 | `classifyCode_never_explains_unknown_numbers` | 同上 | `("sinumerik-nck","10")` 与 `("sinumerik-nck","7")` 的 `codeText === null` |
| T3 | `mesa_markers_are_never_device_codes` | 同上 | `DEVICE_ERROR`/`COMMUNICATION_LOST` → `MESA_MARKER` 且 `codeText` 不含"设备" |
| T4a | `fingerprint_covers_quality_code_alone` | `deviceModel.test.ts`（扩展 `:754-769` 循环） | **仅**改 `quality_code`（其余字段全同）⇒ `changed === true` 且返回新引用。**D1 必要功能，非可选优化** |
| T4b | `fingerprint_covers_value_origin_alone` | 同上 | **仅**改 `value_origin`（其余字段全同）⇒ `changed === true` 且返回新引用 |
| T5 | `reconcile_updates_on_communication_lost_overwrite` | `deviceModel.test.ts` | `BAD/DEVICE_ERROR/PLACEHOLDER/T` → `BAD/COMMUNICATION_LOST/PLACEHOLDER/T` 必须换引用 |
| T6 | `drawer_marks_last_known_as_not_current` | `PointDetailDrawer.test.tsx`（新建） | 文本含 `旧值，未更新（断线前）`、`此前的项级码已被覆盖，不可恢复`；不含 `设备返回` |
| T6b | `drawer_current_bad_is_not_success` | 同上 | `CURRENT+BAD+code="5"` ⇒ 含 `本次返回失败`，**不含** `当前有效值`/`本次采样`；徽标色非绿 |
| T7 | `drawer_hides_code_section_for_good_rows` | 同上 | GOOD 且无码 ⇒ 无"原因码"标题 |
| T8 | `drawer_shows_reference_class_not_root_cause` | 同上 | `0x05 (5)` 与 `仓库现有 S7 参考码表：地址错误类（可能关联，需结合协议层确认）` 同时出现；**断言不含** `DB 不存在`/`CPU 保护等级`/`PUT/GET`；`0x0A` 出现时无任何类别文案 |
| T9 | `breakdown_groups_by_origin_and_endpoint` | `QualityCodeBreakdown.test.tsx`（新建） | 分区标题存在；同一数字在不同 driver 下**分开计数** |
| T10 | `export_builds_schema_with_sampling_context` | `qualityExport.test.ts`（新建） | 4 次 GET（顺序断言）；`schema/history/state_consistency/request_status/freshness_summary/evidence_integrity` 存在；`request_status==="COMPLETE"`；`scope` 含 `device_id`；`freshness_summary` 含 `unknown`；`taken_at_ns` 与 `points_read_at_ns` 为**十进制字符串**且 `/^\d+$/`；`points` 与 latest 响应的**解析结果**深度相等（**不**与原文比数值）；文件名 `…-all.json` |
| T11 | `export_aborts_on_http_error` | 同上 | **四个请求中任一** 500 ⇒ 抛错且**不产生**任何文件（`SEPARATE_FILE` 两文件也都不产出） |
| T12 | `export_separates_state_consistency_from_freshness` | 同上 | before≠after ⇒ `state_consistency==="STATE_CHANGED_DURING_READ"`，但**断言不存在** `sample_status`/`VALID`/`FAILED` 字段；该用例 `unknown=0`，断言 `current+last_known+placeholder` 之和 == `scope.row_count`（`unknown` 的单独计数口径见 §6.3） |
| T13 | `export_separates_raw_when_large` | 同上 | 构造 >5 MiB 原文 ⇒ `sampling.evidence_integrity.raw_preserved==="SEPARATE_FILE"` 且产出 2 个文件；两文件 `raw_sha256` 相同；**断言原文未被丢弃** |
| T14 | `export_threshold_uses_utf8_bytes` | 同上 | 含多字节字符（如中文）的原文按 `TextEncoder` 字节数判定，而非 `String.length`；`sampling.evidence_integrity.raw_utf8_bytes` 等于该字节数 |
| T15 | `export_timestamp_is_bigint_string` | 同上 | `taken_at_ns` 与 `points_read_at_ns` 用 `BigInt` 生成（末 6 位为 `0` 的毫秒对齐值），**不得**出现 `Date.now()*1e6` 的浮点结果 |

#### 10.2.1 第四轮（对齐已合并实现）新增用例（共 18 条）

> 来源：PR #127 复审修复提交 `c6c9919`；用例名逐字取自测试文件。计数口径：`it.each` 的 4 个组合各算 1 条，故「双设备多连接 · 组合…」记 4 条。合计 6（S1–S6）+ 3（E1–E3）+ 6（G3、G4、G5×4）+ 3（B1–B3）= **18 条**。

| # | 用例名 | 文件:行 | 断言点 |
|---|---|---|---|
| S1 | `scope_matrix_four_combinations` | `qualityScope.test.ts:39-59` | 四种组合的行集与端点数：全设备全连接 5 行 → 设备全连接 2 行 → 设备单连接 1 行 → 全设备单连接 1 行 |
| S2 | `device_selected_connection_all_is_device_only_not_cross_device` | `qualityScope.test.ts:61-69` | 设备选中 + `"ALL"` ⇒ `endpointId=null`；另一设备的端点一个都不进 |
| S3 | `unattributed_points_fail_closed_when_device_selected` | `qualityScope.test.ts:71-77` | 端点不在清单 ⇒ 设备筛选下排除；未选设备时保留（聚合本职） |
| S4 | `point_level_device_id_wins_over_endpoint_map` | `qualityScope.test.ts:79-88` | 行自带 `deviceId` 优先于端点映射（命中/不命中两个方向都断言） |
| S5 | `scope_labels_and_description_reflect_real_scope` | `qualityScope.test.ts:90-111` | `CNC-A · 全部连接`、`统计范围：…`；`description` 含设备 id 且**不含**"跨设备"；四组合文案齐全 |
| S6 | `status_and_search_are_not_part_of_scope` | `qualityScope.test.ts:113-122` | 带 `derived`/`displayKey` 的点全部保留：状态/搜索不是作用域输入 |
| E1 | `export_scope_device_and_connection_matrix` | `qualityExport.test.ts:254-314` | 四组合的 `analysis_scope`/`scope`/`points`/`freshness`/`by_quality`；`raw_scope.row_count` 恒为全部行、原文不被裁剪 |
| E2 | `export_device_scope_fail_closed_for_unattributed_points` | `qualityExport.test.ts:316-340` | `ghost` 端点被排除出分析，但仍在原始证据中（`raw_scope.row_count=2`） |
| E3 | `export_filename_reflects_device_scope` | `qualityExport.test.ts:342-360` | 仅设备 ⇒ `…-cnc-01.json`；设备+连接 ⇒ `…-cnc-01__lx-nck.json` |
| G3 | `质量状态筛选与搜索只影响表格，不改变基础统计范围` | `GlobalDataPage.test.tsx:185-220` | 搜索/状态筛选改变表格（`实时数据 · 1/4`）但计数与 `data-scope-label` 不变 |
| G4 | `导出前可见范围提示：原始响应含全部端点，分析按所选设备与连接过滤` | `GlobalDataPage.test.tsx:222-241` | 点击计数行 ⇒ `export-scope-hint` 可见，含"全部端点"/"按所选设备与连接"，与导出按钮同容器 |
| G5 | `双设备多连接 · 组合($device,$connection) ⇒ $label`（`it.each` ×4） | `GlobalDataPage.test.tsx:243-274` | 四组合的计数与 `data-scope-label` 写实；选设备时另一设备行不出现 |
| B1 | `popover_opens_on_click_and_shows_breakdown` | `QualityCodeBreakdown.test.tsx:81-95` | **触发器真实点击**：展开前内容不 mount，点击后提示与限制语可见（§7.6） |
| B2 | `export_scope_hint_is_visible_next_to_export_button` | `QualityCodeBreakdown.test.tsx:97-106` | 提示含"全部端点"/"按所选设备与连接"，且与导出按钮同一容器 |
| B3 | `export_scope_hint_shows_current_scope` | `QualityCodeBreakdown.test.tsx:108-118` | 提示含当前范围 `CNC-01 · 全部连接` |

> 说明：`GlobalDataPage.test.tsx` 的「计数行跟随设备 + 连接双重筛选」与「设备选中但连接 ALL：统计只含该设备的全部连接（不是跨设备）」是**既有用例的改写**（原名分别为「计数行跟随连接筛选（不跟随设备筛选）」「设备选中但连接 ALL 时明确标注全部连接（跨设备）」），不计入上表 18 条；改写本身即"旧口径 → 新口径"的回归守门。
>
> **上一实现轮（`a1090bb`）的用例已补录**（此前遗漏，本轮补齐）。它们与 §10.2 的 18 条同属 D1 验收范围：

| # | 用例 | 断言要点 |
|---|---|---|
| R1 | `export_marks_raw_scope_vs_analysis_scope` | `raw_scope` 覆盖全部端点、`analysis_scope` 仅所选设备与连接；两者同时写入导出文档 |
| R2 | `export_counts_unknown_value_origin_separately` | 未知/缺失 `value_origin` 单独计入 `unknown`，**不并入 `placeholder`**；四项之和 == `row_count` |
| R3 | `drawer_shows_missing_code_for_bad_rows` | **BAD 且无码**时码区可见并显示"未提供（异常形态，请报告）"（§9 U2） |
| R4 | `drawer_follows_live_updates_while_open` | **抽屉打开期间**收到实时更新：码变化随之更新；点消失显示"已从实时快照中消失"，**不再显示旧值** |
| R5 | `focas_mesa_class_is_not_device_return_group` | FOCAS2 分类码归入"Mesa 分类（非厂家码）"，**不与设备返回码同组** |
| R6 | `missing_code_is_visible_for_non_good_rows_only` | 码区可见性判据：有码恒显示、GOOD 无码隐藏、**非 GOOD 无码必须显示** |

### 10.3 "没有改变采集正确性"的证明

1. `git diff --stat` 中 `crates/**`、`drivers/**`、`proto/**`、`tests/driver-contract/**` 为空。
2. 同端点同状态下 D1 前后各抓一次 `points/latest`：`points[]` 的**字段集合**（键名）与 `quality_code` 字符串形态完全一致（`timestamp_ns`/`value` 自然变化；**比较在"解析后的键集合"层面进行，不主张纳秒数值相等**，§6.6-1）。
3. **未筛选导出**时，导出的 `points` 与同刻 `points/latest` 的**解析结果**深度相等（T10）——若有人改了码格式立刻红；**有作用域筛选**时 `points` 是过滤后的副本，全量未被裁剪由 `raw_scope.row_count` + `points_raw_text` 证明（E1）。**原文一致性**由 `evidence_integrity.raw_sha256` 单独证明。
4. 断线契约断言原样通过（§10.1 第 1 行）。
5. 唯一可信证据：`python scripts/write-contract-evidence.py` 产出的 `target/validation/contract.json`。**suite 数必须以注册表为准**：`scripts/contract_suites.py:13-52` 当前为 **30**（BASELINE 14 + EVENT 12 + LIFECYCLE 4），`AGENTS.md` 已同步为 30。

## 11. D1 明确不能解决什么（诚实边界）

### 11.1 80 个 BAD 点：仍**尚无有效的项级原因证据**

现有唯一 238 行产物是断线后抓的（`target/validation/t0-points-latest.raw.json`：238/238 `BAD`+`COMMUNICATION_LOST`，`PLACEHOLDER` 80、`LAST_KNOWN` 158），**没有一次"RUNNING 窗口 + 带数字码"的样本**。当前状态是"**尚无有效的项级原因证据**"，**不是**"没有数据就一定读取失败"。**仅新增 UI 不能恢复历史上已被 `COMMUNICATION_LOST` 覆盖的码**：覆盖发生在写入时（`snapshot.rs:404-405`），Core 不保留旧值——D1 只让**今后**的码可见，仍需后续受控采样（须遵守 `capture-points.ps1` 纪律）。旁证：本机今日抓取（`target/validation/qc-capture/20261020-093420-338-…`）是**同一 endpoint id 的另一轮配置**（`revision=16`、`points=4`、全 GOOD），不构成 238 点/80 BAD 的证据。

### 11.2 238 点请求数：**不得**推断每秒请求数

`max_read_items` 可配为 1（`drivers/sinumerik-nck/src/config.rs:42`；`:57` 取 `S7_MAX_ITEMS_PER_PDU`，`:380` 断言默认 19；`:187-192` 允许 1..=19；`client.rs:92` 按该值切块）。取 1 时每轮**最多**涉及 238 个**串行单点**读取——这是"每轮上限"，**不是**固定 QPS（轮间隔、超时、重连、退避都会改变实际速率，§16 已证明退避曾被恒定为 1s）。任何批量读取优化（调大 `max_read_items`/合并区间）**必须单独测试设备负载与 Job 拒绝**，不能从本设计推导。

### 11.3 历史元数据：**不制造伪历史**

原始 240 点配置未备份是**已知证据缺口**；**不得**通过猜测 `legacy resource_id`/`outputs` 重建。只保留可追溯的 rev15 配置；D1 不新增、不改写任何历史配置。

### 11.4 既有问题：前端纳秒时间戳的精度（**本轮不修**）

**现象（可确认的部分）**：现有前端用 JavaScript `Number` 解析 `/points/latest` 与 SSE 里的 `timestamp_ns`（Unix ns）。该量级约 `1.8e18`，**超出安全整数上限** `Number.MAX_SAFE_INTEGER = 9_007_199_254_740_991`（≈`9.0e15`），因此 **JS 侧无法保证纳秒级精确还原**；在 `1.8e18` 量级上 IEEE-754 双精度的可表示间隔为 **256 ns**。

**不得据此推断的部分**：**不能**由上述事实断言"当前界面显示的时间是错的"。界面若只是把时间戳转成毫秒再格式化（如 `new Date(ns/1e6)`），256 ns 量级的误差**通常不会造成肉眼可见的变化**。任何"现有 UI 排序/显示有实际错误"的结论都**未验证**，本轮不作此断言。

**需要重点检查的场景**（这些才是精度会真正显形的场合，**均未验证**）：

| 场景 | 风险 | 状态 |
|---|---|---|
| 纳秒级对账（把 UI/导出值与抓包/原文逐纳秒比对） | 必然不等 | **未验证**（导出侧已由 §6.6-1 处理：以原文为权威） |
| 严格相等比较（`a.timestamp_ns === b.timestamp_ns`、去重键、Map 键） | 本应相等的两值可能被判不等 | **未验证** |
| 排序（同批次内多点 `timestamp_ns` 相同，排序结果依赖低位） | 可能改变稳定次序 | **未验证** |
| 时间差计算（`t2 - t1` 得到毫秒/秒，用于年龄、超时、STALE 判定） | 误差 ≤256 ns，对 30s 级阈值无实质影响；对纳秒级阈值有影响 | **未验证**；现有阈值均为毫秒/秒级（`deviceModel.ts:496`） |

**本轮明确不改**：业务时间戳契约（UTC Unix ns，方案 §3.8）、后端 JSON 形状（`timestamp_ns` 仍是 i64 number）、现有 UI 日期组件与 `pointAgeMs`/`formatAge`/`derivePointStale` 的实现。上述检查项作为**独立技术债**记录，不并入 D1 交付范围；若后续要修，须单独立项（改契约或引入 BigInt 解析），并单独评估对导出与对账口径的影响。

### 11.5 其他明确不做

| 不做 | 原因 |
|---|---|
| 改 `Quality` 三态 / 加 `detail` | 冻结契约；D1 只读展示 |
| 改 `proto/driver.proto` 或 `sint32 quality_code` 类型 | 会破 `protocol_negotiation.rs:311` |
| 改 `mark_communication_lost` 覆盖行为 / 保留断线前原因 | 冻结语义（方案 §3.11）；后续专项 |
| 给 S7 补设备码、给驱动加 subscriber | 属 D2（本地失败原因保真） |
| Job/Transport 结构化分类 | 属 D3 |
| 重定义 `RUNNING` / 新增健康判据 | 属生命周期专项（`endpoint-running-backoff-p1-review.md:227-241`） |
| 解释 OPC UA / 未知 S7 码 / 0x0A | 仓库无码表，**待确认**（§12） |
| 历史存储 / 时序库 | 超出 D1，且新增存储面 |
| 自动判定根因、剔除坏点、改点表 | 明令禁止（上游 §9） |

## 12. 待确认（由谁、依据什么确认）

| # | 待确认 | 为什么定不了 | 由谁 / 依据什么确认 |
|---|---|---|---|
| 1 | NCK item 码 `0x0A` 及 `0..254` 未定义值的语义 | 仓库只有 0x03/0x04/0x05 定义（`error.rs:14-21`），0x0A 仅见测试夹具 | **驱动负责人** 依厂商 S7/NCK 文档；无文档则维持"不解释"（D1 默认行为，无需等待） |
| 2 | item 级与 header 级 0x03/0x04/0x05 是否逐字同义 | `error.rs:78-99` 只声明 CPU 侧语义 | **S7 传输层负责人** 依 S7Comm 规范；确认前文案必须带"仓库 S7 码表"限定词 |
| 3 | `focas2` 的 `1..5` 是否覆盖全部单点失败 | 分类是字符串匹配（`focas2/src/lib.rs:1664-1687`），未知落 1 | **FOCAS2 负责人** 依现场码分布；D1 只显示"Mesa 分类"，不依赖该结论 |
| 4 | test-driver / simulator 是否写 `quality_code` | 本轮未核实 | **实现者** 在 D1 开发首日静态核查 `tests/support/test-driver`；未写则分类恒 `UNKNOWN`（不解释），不影响交付 |
| 5 | 是否有仓外消费方依赖十进制字符串格式 | 仓库内无消费方，仓外未知 | **现场负责人** 确认；确认前 D1 不改格式（只加 `0xNN` 视图） |
| 6 | OPC UA 负值在 UI 上是否与"数字码"混淆 | 需真实 OPC UA 样本 | **前端负责人** 用真实端点截图评审；D1 已用命名空间标签区分 |
| 7 | 50K 点下 Popover 聚合与导出的可接受延迟 | 取决于部署规模 | **性能负责人** 在 50K 压测时测量管理面耗时；不达标再议（D1 不预设阈值） |
| 8 | 会话内"最近 N 次"是否补做 | 易被误读为历史 | **评审** 裁决；默认**不交付**（§7.2 已明确无历史） |
| 9 | 前端纳秒精度是否单独立项（§11.4） | 本轮只记录既有事实，**未验证**任何具体 UI 路径是否受影响 | **前端负责人 + 评审**：先按 §11.4 的 4 类场景（纳秒对账/严格相等/排序/时间差）做一次代码审计，再决定是否立项；在此之前**不得**把任何显示/排序定性为存在实际错误 |

## 附录 A：评审六项重点 → 章节对照

| 审查重点 | 本文件章节 | 一句话结论 |
|---|---|---|
| 数据契约 | §1、§3 | 复用 11 个现有字段，零新增；派生字段只在视图层，不复制状态。导出 schema 的 `raw_scope`/`analysis_scope`/`freshness_summary.unknown`/`scope.device_id` 是 **additive**，消费方须保留未知字段并显式兼容检查（§6.3） |
| 质量码解释 | §4 | 按 `(driver_id, code)` 完整映射 16 种情形；设备码仅 nck/opcua；首版只给"参考类别"，禁写具体根因 |
| 断线显示 | §5 | 9 种 `(origin, quality, code)` 组合逐条定义 + `value_origin` 缺失的异常形态；`CURRENT+BAD` 不得用绿色；旧值必带"未更新/无有效值" |
| 原始数据导出 | §6 | 逐字段 schema + 4 个**必需** GET；时间戳为十进制字符串；`evidence_integrity`（原文+sha256）为权威，`points[]` 为结构化副本；`raw_scope` 与 `analysis_scope` 分别标注，导出前必须显示范围提示（§6.7）；`request_status` 单值且显式免责 |
| 统计分析 | §7 | 仅当前快照（每点 1 样本）；无历史，限制语固定；**设备 + 连接双重作用域**（§7.1），状态/搜索只影响表格；Popover 仅点击触发，`content` 必须始终传元素（§7.6） |
| 回归测试 | §10 | 8 条既有契约不变 + **35 条**新增用例（T1–T15 共 17 条 + 第四轮 18 条，用例名+断言点）；suite 数以注册表 30 为准 |

## 附录 B：修订对照（B.1/B.2 相对 `80c180e`；B.3 相对已合并实现 `760942e`）

### B.1 第二轮（评审五项 + 三项建议 + 一项细节）

| 编号 | 改在哪一节 | 改成了什么 |
|---|---|---|
| **P0-1** 纳秒精度 | §6.2 步骤 1/3/7、§6.3、§6.4 注 2、§6.6-1、§10.2 T10/T15、§10.3-2/3 | `taken_at_ns`/`points_read_at_ns` 改**十进制字符串**（`BigInt`）；`points[]` 降级为"结构化副本"；`points_raw_text` + 校验和 为权威；明确**不得**声称两者纳秒一致 |
| **P0-2** 状态≠有效 | §6.2 步骤 6、§6.3、§6.4 注 1、§6.6-3、§9 U10、§10.2 T12 | 拆成 `state_consistency` / `request_status` / `freshness_summary` 三个采样字段 + 顶层 `limitations`（第三轮再补 `sampling.evidence_integrity`，凑齐四个）；删除旧的单一"有效/无效"标志；**不再有单一有效性判据** —— 来源分布看 `freshness_summary`、质量看逐点 `quality`、时间新鲜度需结合采样时间与运行窗口 |
| **P1-1** CURRENT+BAD | §5.3（表头与 9 行）、§5.4-2、§9 U1/U12、§10.2 T6/T6b | `CURRENT+GOOD`→`当前有效值`（绿）；`CURRENT+BAD`→`本次返回失败`（红，禁绿）；`LAST_KNOWN`→`旧值，未更新`；`PLACEHOLDER`→`无有效值` |
| **P1-2** 原始证据 vs 副本 | §6.3、§6.5、§6.6-2 | 原文 + 校验和 = 原始证据；`points[]` + `summary` = 结构化副本；5 MiB 阈值按 **UTF-8 字节**（`TextEncoder`）判定 |
| **P1-3** 收窄 NCK 解释 | §4.2（3 行改写 + 收窄口径块）、§4.3、§4.4（新增禁用清单）、§9 U4、§10.2 T8 | 只给 `仓库现有 S7 参考码表：<类别>（可能关联，需结合协议层确认）`；逐字禁用 `CPU 保护等级`/`DB 不存在`/`PUT/GET` 等具体根因 |
| **建议 1** suite 数 | §10.3-5、附录 A | 27 → **30**（`scripts/contract_suites.py:13-52`：BASELINE 14 + EVENT 12 + LIFECYCLE 4）；`AGENTS.md` 现已同步为 30 |
| **建议 2** Popover | §7.4、§9 U8 | 统一**仅点击**触发；悬停不展开 |
| **建议 3** 指纹用例 | §10.2 T4a/T4b | 拆成"仅改 `quality_code`"与"仅改 `value_origin`"两条独立用例，并标注为 **D1 必要功能** |
| **细节** >5 MB | §6.3、§6.5、§9 U9、§10.2 T13/T14 | 新增证据完整性状态（`INLINE`/`SEPARATE_FILE`）；超限时**下载两个文件**而非丢弃原文；禁止 `ANALYSIS_ONLY` |

### B.2 第三轮（设计冻结）

| 编号 | 改在哪一节 | 改成了什么 |
|---|---|---|
| **改名** | §6.2 步骤 6、§6.3、§6.4、§6.6-3、§9 U10、§10.2 T10/T12、附录 A/B | 四个字段统一为 `request_status` / `state_consistency` / `freshness_summary` / `evidence_integrity`；全文已无旧名残留 |
| **单值 + 显式免责** | §6.3（新增脚注）、§6.2 步骤 2/5、§9 U9 | `request_status` 只有 `COMPLETE` 一个值（失败不产文件，故无 `FAILED`/`INCOMPLETE`）；`diagnostics` 改为必需 ⇒ `mesa_version` 不再可空；新增"`request_status=COMPLETE` 不代表数据新鲜、连接健康或设备正常" |
| **证据完整性统一** | §6.3、§6.5、§6.6-2、§9 U9/U10、§10.2 T13/T14 | 完整性状态与校验和合并为**一个** `evidence_integrity` 对象（`raw_preserved` / `raw_utf8_bytes` / `raw_sha256`），不再有并列的同义字段 |
| **§11 新条目** | 新增 §11.4（原"其他明确不做"顺延为 §11.5） | 记录"前端纳秒精度"这一**既有问题**：只写可确认事实，明确不得推断现有界面显示错误，列出需重点检查的 4 类场景（均标未验证），并声明本轮不改时间戳契约/后端 JSON/UI 日期组件 |

### B.3 第四轮（对齐已合并实现，纯文档）

| 编号 | 改在哪一节 | 改成了什么 |
|---|---|---|
| **1** 导出 schema 增量字段 | §6.3（`scope` 改写 + 新增 `raw_scope`/`analysis_scope` 行 + `freshness_summary.unknown` + 兼容性块）、§6.2 步骤 2/3、§6.4、§6.5 | 记录四类 additive 字段；`points`/`summary`/`freshness_summary` 明确只统计 `analysis_scope` 内的行；要求消费方保留未知字段 + 显式兼容检查，**不宣称**所有外部解析器兼容（严格 Schema 校验是否兼容仍属未知） |
| **2** 双重作用域（替换旧口径） | §7.1（新增）、§7.2–§7.5（原 7.1–7.4 顺延）、§7.3 输入说明、§9 U8、附录 A、§0-12 | 设备×连接四组合表；质量状态/搜索只影响表格；归属不可证明 fail-closed；旧标签 `全部连接（跨设备）` 移除，改为写实 `统计范围：CNC-01 · 全部连接` |
| **3** 导出前范围提示 | §6.7（新增）、§9 U9/U9b | 从建议提升为**契约**：两行逐字文案、`<范围>` 与计数行同源、常显且与导出按钮同区域 |
| **4** 文件名含设备维度 | §6.5、§9 U10 | `mesa-points-export-<stamp>-<deviceId[__endpointId]\|all>…`；目的：降低误分享风险 |
| **5** Popover 可达性 | §7.6（新增）、§10.2.1 B1/G4 | `content` 必须始终传元素（传 `null` 永不打开）；必须有真实点击展开的回归，不能只渲染内容组件 |
| **6** 示例/测试/附录 | §6.4、§10.2 T10/T12、§10.2.1、附录 A/B | 示例补 `raw_scope`/`analysis_scope`/`scope.device_id`/`unknown`；登记 18 条新增用例（S1–S6、E1–E3、G3–G5、B1–B3）；说明 G1/G2 为旧用例改写 |
| **附** 展示语义（实现对齐） | §3.2、§5.3、§9 U2/U4/U4b/U5、§10.3-3 | `freshness` 补 `UNKNOWN` 与"采样状态未知"异常形态；非 GOOD 无码**必须**渲染 `未提供（异常形态，请报告）`；`MESA_CLASS`（focas2 的 1..5）单独分组 `Mesa 分类（非厂家码）`，不得并入 `设备/协议返回码`；`UNKNOWN` 标题补"（不解释）"；"`points` 与 latest 解析结果深度相等"限定为**未筛选导出** |

> 本轮基线：`main = 760942e8532f9990b7dd33dcc10acb40d3d201ac`（PR #127 已合并，D1 实现已进 main）。本轮**只改本文档**：未改任何代码/测试/配置；未启动 mesad/Vite/emulator；未连接设备；未运行测试或构建；未做 git 写操作。
