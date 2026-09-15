# Mesa Frontend V2（Device-first）

V2 导航按用户任务组织，不按后端对象层级。`Device` 是前端唯一一级主体；
`Connection`（Endpoint）是 Device 的内部能力，不再成为导航层。

## 路由

| 路由 | 页面 | 说明 |
|---|---|---|
| `/` → `/overview` | — | 根重定向 |
| `/overview` | 总览 | 系统状态 + 需要关注（点击直达设备） |
| `/devices` | 设备列表 | 瘦列表：搜索 + 状态 + 连接数；唯一创建入口 `+ 添加设备` |
| `/devices/new` | 添加设备 | 四步 draft（设备→连接→数据→确认），最终一次原子提交 |
| `/devices/:deviceId/:tab` | Device Workspace | `overview`/`data`/`events`/`config`/`diagnostics` |
| `/devices/:deviceId` | — | 重定向到 `overview` |
| `/devices/:deviceId/endpoints/:endpointId` | — | 旧深链重定向（保留 `?connection=`） |
| `/data` | 全局实时数据 | 跨设备 Point 聚合（`?device=` `?connection=`） |
| `/events` | 全局事件 | 跨设备事件聚合（`?device=` `?connection=`） |
| `/system` | 系统 | 只展示可确认状态（version/uptime/AVAILABLE） |

## Device Workspace（M1 DoD）

> 从这一刻开始，用户进入一台设备后，不再离开 Device Workspace。

- Header 常驻设备身份；`?connection=` 是唯一的连接上下文（URL 即真相）；
- 观察类 tab（概览/数据/事件）可选“全部连接”，配置类 tab（配置/诊断）必须单连接；
- 任何 Point/事件行点击 → 同页 Drawer → 一步跳其 Connection 的配置或诊断。

## 状态语义（诚实三态）

- 连接运行态：`RUNNING` / `STOPPED` / `FAILED`（来自后端，不编造）；
- 数据态：`GOOD` / `BAD` / `STALE`（`BAD` 优先于 `STALE`；时间非法为 `UNKNOWN`）；
- 失败 ≠ 0，未知 ≠ 正常，`STALE` ≠ `BAD`；无 Health Score；
- fail-closed：快照失败保留 last-known，`nowMs` 独立推进使旧点自然 `STALE`。

## 添加设备（M4.2/M5 原子接口）

- 前三步只攒 draft；确认页 `POST /api/v1/device-bootstrap` 一次提交；
- 后端单事务落库（Device + Endpoint + Secrets + Tasks），Driver start 为
  side effect，失败补偿删除并明确报告；
- 幂等：`Idempotency-Key`（header 或 body）+ 请求指纹；同 key 同体直接
  重放上次结果（`replayed: true`），同 key 不同体 `409 IDEMPOTENCY_CONFLICT`；
- 前端幂等键由 draft 内容派生：同 draft 重复提交不建两台设备。

## 事件过滤（M5.3/M7）

- `GET /api/v1/events` 支持 `endpoint_ids`（CSV 并集）与 `device_id`
  （后端映射为 endpoint 集合，事件表不动）；
- 设备事件页用 `endpoint_ids` 单查询（代替逐 endpoint 分查）；
- 全局事件页 `?device=` 走后端 `device_id`；SSE live 仍是客户端归属判定
  （服务端 live 无过滤参数，语义与 SQL 对齐）；
- 文本过滤 400ms debounce；Select/状态/时间即时生效。

## 测试

- 前端：`pnpm vitest run`（forks/threads 池 2 并发；重交互用例显式超时）；
- 后端：`cargo test -p mesa-config-store -p mesa-event-store -p mesa-core-api`；
- 规范：`pnpm typecheck` + `pnpm lint`（前端），`cargo clippy`（后端）。

## R4 性能基线（隔离实例实测）

| 接口 | 规模 | 延迟 |
|---|---|---|
| `GET /devices` | 101 devices | 3–7ms |
| `GET /endpoints` | 501 endpoints | 热 25–30ms；首次 549ms（driver session 冷启动，一次性） |
| `GET /diagnostics` | — | 6ms |
| `GET /points/latest` | 2005 points | 56–61ms / 413KB |
| `GET /events?limit=100` | 10k rows（直写） | 14–21ms（含 `endpoint_ids`/`device_id`/`active` 过滤） |

结论：API 侧无瓶颈；前端表格分页（20/页）已覆盖渲染规模，
virtualization/memoization 暂不需要（按需引入，不提前复杂化）。

## V1 → V2 迁移

- 旧页面（Dashboard/DeviceDetail/EndpointWorkspace/Onboarding/Monitor/
  EventsView）已在 M5.6 删除；旧路由仅保留 endpoint 深链重定向；
- 旧创建链路（分步 `POST /devices` → `/endpoints` → `PUT /tasks` → `/start`）
  已被 `/device-bootstrap` 代替；前端四步编排已退役；
- 数据库迁移到 v6（`006_bootstrap_idempotency` 幂等键表），旧库自动升级，
  业务数据不丢失（迁移链单测覆盖 v2/v4 → 最新）。
