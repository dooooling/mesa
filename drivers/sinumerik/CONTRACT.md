# SINUMERIK 只读地基契约（PR11 — FROZEN 候选，待 PR12 真机确认）

> 主线：先把 SINUMERIK 做成可靠的只读设备，再真机验证，再接已冻结的 Event Plane，
> 最后才进 Control Plane。本文件冻结 PR11 的只读地基；PR12 真机结论回写 §8。
>
> 架构门禁：SINUMERIK 特异性止于 `drivers/sinumerik/*`，经公共
> `mesa-opcua-transport` 复用 Native OPC UA（不自建第二套 transport）。
> Core 只认识 Descriptor / ProbeReport / ResourceSelection / DataBatch，
> 无 `driver_id == "sinumerik"` 分支。

## 1. 范围（PR11 只做这些）

| 能力 | 形态 | 说明 |
|---|---|---|
| Descriptor | `sinumerik` | 连接 Schema（endpoint/安全/认证/超时）+ 资源 `node` + Poll/Subscribe + Browse |
| Probe | 事实采集 | 建连 → NamespaceArray → BuildInfo → Objects 浅浏览 → 断开；不建订阅、不写 |
| Browse | 管理面 | 单层浏览 + continuation 接力取全页 + canonical 身份输出 + 过滤/分页 |
| Configure | 校验 | 通用 `mesa.resources.v1`（resource `node`）+ legacy `sinumerik.node-group`/`sinumerik.subscription` |
| Poll | 只读 | `ResourceSelection → Read → UA value/status/timestamp → Mesa Value → DataBatch` |
| Subscribe | 只读 | 分裂生命周期（建订阅→建监控项→失败回滚→单项 BAD 合成初始事件→按序清理），Latest-Wins，不借 Event FIFO |
| 重连 | 自愈 | 每次 run 新鲜 NamespaceArray 换算；会话死亡 `READ_FAILED` 交 Manager 重建；teardown 有界 disconnect |

## 2. 稳定身份（P0，已冻结）

- Canonical 身份：`nsu=<namespace-uri>;<i|s|g|b>=<id>`（见 `src/canonical.rs`）。
- `configure` / `browse` 边界只接受 `nsu=`；`ns=<index>` 一律 `INVALID_ADDRESS` 拒绝
  （reason 指引去 browse 拿 canonical 身份）。`ns=` 索引重启后可能漂移，不接受。
- 运行期每次建会话后用新鲜 NamespaceArray 把 URI 换算回当前 index；
  同一资源 index 变化 → 同一 canonical → Core 的 point_id 不漂（测试：
  `browse_identity_stable_across_namespace_reshuffle`、
  `session_loss_fails_run_and_reconnect_resumes_same_point_id`）。
- URI 未出现在设备 NamespaceArray → `UNKNOWN_NAMESPACE` fail-closed，不带病运行。

## 3. Probe 规则（不过度推断）

- `vendor/model/firmware` 原样透传 BuildInfo 事实，不改写。
- `family = "SINUMERIK"` 仅当 ProductName 含 `sinumerik`（大小写不敏感）→ 置信度 `high`。
- 仅有 Siemens 痕迹（vendor/namespace 含 `siemens`）但无 `sinumerik` 字样 →
  family 保持 None + `SINUMERIK_UNCONFIRMED`（可能是 SIMATIC 等，绝不猜）。
- 全无 Siemens 痕迹 → 同样 `SINUMERIK_UNCONFIRMED`，按通用 OPC UA 设备处理
  （不拒绝采集；拒绝是 Core profile 匹配的事）。
- capabilities 只报实测的 `read`/`browse`（四态与通用驱动同规则）；
  `subscribe` 本次未建订阅，无资格断言，直接省略。

## 4. 值映射（与通用 OPC UA 驱动逐字同口径，见 `src/value.rs`）

- GOOD + 有效 typed 值 → CURRENT（更新 last_known）；UNCERTAIN + 有效值 →
  CURRENT/ Uncertain（不更新 last_known）；BAD/无值/类型不符 →
  LAST_KNOWN（有缓存）或 PLACEHOLDER（无缓存，source_timestamp=None）。
- 外来 Variant（Guid/StatusCode 等）→ `String(debug)` 显式回退（可见可调试）；
  期望类型非 String 时按 `BadTypeMismatch` 隔离为 BAD。绝不静默 skip/null/0/""。
- 数组保留 Typed Array（支持范围内）；SourceTimestamp 1601 ticks → Unix ns 精确保留。
- 非法 `data_type` 在 configure 期即拒绝，不拖到运行期。

## 5. 生命周期不变量（测试锁定）

- point_id 不漂（canonical 吸收 index 漂移）；
- 旧 session 不继续产数据（writer 按 epoch 丢弃；Stop 后不再 publish）；
- 新 session 不产生双 writer（supervisor 任一 Err 即 cancel 全体并 reap）；
- Stop 后不再产出（teardown 有界 disconnect，失败/超时仅诊断，不掩盖原始错误）；
- reconnect 不重复注册 subscription（失败回滚删订阅恰一次；shutdown 按序删项+删订阅可观测）；
- Data queue 不泄漏（Latest-Wins 有界；teardown 无残留映射）；
- 非法配置 fail closed（`BAD_CONFIG` / `INVALID_ADDRESS` / `UNKNOWN_NAMESPACE` /
  `EMPTY_PLAN` / `EVENT_NOT_SUPPORTED`）。

## 6. PR11 明确禁止（出现即 scope drift）

NC Start/Stop、Reset、Write variable、Program select/start、Mode switch、
Alarm ACK/Confirm、SINUMERIK EventRecord、EventStore/REST/Web 修改、
Control Plane API。成功定义：Mesa 可以稳定地看见并读取 SINUMERIK，
但绝对不能改变设备（`write`/`command` 沿用 SDK 默认 Unsupported；
`configure_events` 非空即 `EVENT_NOT_SUPPORTED`）。

## 7. Definition of Done（PR11）

- [x] Descriptor / Probe / Browse / Continuation / ResourceSelection …… PASS（单测 40）
- [x] Stable canonical resource identity / Stable point_id …… PASS（换算+重连测试）
- [x] Read scalar / typed values / Bad Status / Poll / Subscribe …… PASS
- [x] Reconnect / Stop / Session-loss / Invalid config fail closed …… PASS
- [x] Shared Data contract（descriptor/resource/discovery 合同门）…… PASS
- [x] No Core sinumerik branch / No Event / No Control …… PASS（本目录内，无 Core 改动）
- [x] 确定性 fixture（`src/fixture.rs`，冻结已验证语义，PR12 在此补回归）
- [ ] 真机验证（PR12，不在本 PR）

## 8. PR12 交接：待真机确认的假设清单

1. ProductName 是否含 `SINUMERIK` 字样（决定 §3 强证据规则是否成立；反例→修规则+补回归）。
2. Siemens 命名空间 URI 真实形态（fixture 用 `http://www.siemens.com/sinumerik` 占位假设）。
3. 实际 datatype / timestamps / BadStatus / subscription revised 值。
4. session timeout 行为与重连后 NamespaceArray 是否漂移。
5. 真机证据一律 machine-readable（`device/firmware/probe/browse/read/subscription/reconnect/result`），
   不落 password/private key/生产 IP/程序内容。
