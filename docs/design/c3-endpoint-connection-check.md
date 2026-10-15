# C3：离线保存与 Endpoint-bound Secret-safe 连接检查（V1 冻结）

- 状态：**接口与验收标准冻结**（本文档先冻结，再实施）
- 分支：`feat/connection-c3-endpoint-check`（自 C2 `f4b8a0b` 分出）
- 依赖：C1（Probe 分层预算、Secret 物化、`master.key` 保护）、C2（共享连接面板、统一草稿/revision 门禁、提交守卫）
- 非目标：不新增数据库表、不新增后台任务、不做凭据缓存、不建通用检查框架、不引入新的驱动通信协议

## 1. 目标（完整用户流程）

```
配置设备 → 保存离线连接 → 后续补充采集点 → 复用已保存凭据测试连接 → 确认后启动采集
```

C3 的完成目标**不是**"让测试按钮重新可点"，而是让上面这条流程真正可用：
设备未开机也能保存合法配置；编辑已有凭据的连接时不必反复输入密码即可测试；
并且**任何情况下都不把 Secret marker 当密码发出、不把 Secret 写进响应或日志**。

## 2. 功能边界（冻结）

| 功能 | C3 决定 |
|---|---|
| 设备未开机、网络不可达 | 允许保存合法配置，**不强制 Probe** |
| 新建设备暂无采集点 | 允许仅保存设备与连接，`tasks=[]` |
| 离线保存默认行为 | **不启动 Endpoint**，明确保存为 `STOPPED` |
| 编辑已有连接并保留密码 | 可使用**已保存 Secret** 测试当前草稿 |
| 用户重新输入密码 | 测试使用新明文，但**不自动保存** |
| 用户明确清除 Secret | **不允许**偷偷回退到旧密码 |
| 测试成功 | 仅代表**本次探测**结果，不代表已保存或已开始采集 |
| Mesa API 本身不可访问 | 不支持浏览器本地排队保存（**不在本轮范围**） |

离线保存**仍必须**满足 Descriptor 与 Schema 校验：不能因为设备离线就接受非法配置。

## 3. 新增 Endpoint-bound API（冻结）

```http
POST /api/v1/endpoints/{endpoint_id}/connection-check
Content-Type: application/json
```

```json
{
  "mode": "probe",
  "connection": {
    "endpoint_url": "opc.tcp://192.168.1.10:4840",
    "username": "operator",
    "password": { "secret_set": true },
    "timeout_ms": 5000
  }
}
```

- `mode` ∈ {`validate`, `probe`}：`validate` **仅校验不连接设备**；`probe` 复用临时探测。
- 响应对齐既有 Probe 状态分类（§6），**不重新定义**整套结果模型。

### 核心处理链（冻结）

1. 按 URL 中的 Endpoint ID 读取持久化记录；
2. 按**已保存**的 `driver_id` 取 Descriptor（**不接受请求体覆盖 driver_id**，否则等于绕过 Endpoint 归属）；
3. 在 Core 内存中解析 Secret 标记与用户草稿；
4. 统一 Schema 校验（与 C2 同一套 Descriptor/Schema 语义）；
5. `validate` → 仅校验，**不连接设备**；
6. `probe` → 复用 `MesaManager::probe()` 的**临时**探测机制；
7. 返回**脱敏**结果，**不写入任何配置**。

### 安全边界（冻结，违反即为缺陷）

- `{"secret_set": true}` 只允许作为 **Core API 的引用标记**；
  **绝不能原样进入 Driver 的 `OpenConnection` 或 Probe RPC**。
- 明文只允许在**服务端内存中短暂物化**，用后即弃。
- 不把 Secret 写入：HTTP 响应体、`tracing` 日志、错误文本。

## 4. Secret 四种语义（冻结）

| 草稿内容 | 服务端处理 |
|---|---|
| `{"secret_set": true}` | 读取**该 Endpoint、该字段**的已存 Secret |
| 用户输入新明文 | 本次测试使用新值，**不更新** SecretStore |
| `{"clear_secret": true}` | 本次测试中**移除**凭据；**不得继承旧值** |
| 字段缺失 | 按现有 PUT 契约继承旧值；无旧值则保持缺失 |

**必须保留 C2 的 `touched` 信息**：避免用户清空凭据后，因归一化省略了空字段，
服务端又误用原密码。**此差异必须有专门测试。**

## 5. 错误与副作用边界（冻结）

| 响应 | UI 含义 |
|---|---|
| 200，`reachable=true` | 协议可达；身份是否确认**单独显示** |
| 200，`reachable=false` | 探测完成，目标不可达 |
| 400 | 参数错误、Secret 引用无效、必填凭据缺失 |
| 404 | Endpoint 不存在 |
| 501 | Driver 不支持该探测 |
| 503 | Driver、SecretStore 或探测基础设施不可用 |
| 504 | 探测超时 |

- **Secret 解密失败、密钥丢失**等情况：**不得**显示成 CNC 不可达，**不得**自动切换匿名连接。
- 连接检查**不得**修改：`connection_json`、`endpoint_secrets`、`desired_running`、任务、运行态；
  **不得**停止或重启现有 Endpoint。
- 现有 `MesaManager::probe()` 已使用临时 Driver 进程，**直接复用**，不另建探测生命周期。

## 6. 验收矩阵（冻结）

| 编号 | 必须通过的场景 |
|---|---|
| C3-01 | 设备不可达时可以仅保存合法配置，**零 Probe、零 Start** |
| C3-02 | 新建无采集点设备，`tasks=[]`、`start=false`，创建成功且保持 `STOPPED` |
| C3-03 | 编辑已有 OPC UA Secret，未重新输入密码仍能通过 Endpoint-bound 路径测试 |
| C3-04 | 实际送到临时 Driver 的是**正确明文**，不是 marker |
| C3-05 | 新输入密码仅用于本次检查，保存前 SecretStore 内容**不变** |
| C3-06 | 明确清除密码后不再使用旧值；缺失或损坏的 Secret **必须明确失败** |
| C3-07 | 两个 Endpoint 使用同一驱动时，**不能跨 Endpoint 读取凭据** |
| C3-08 | 校验不产生设备网络连接；Probe 不改变已有 Endpoint 生命周期 |
| C3-09 | 切换 Endpoint、修改参数或关闭弹窗后，**迟到结果不得覆盖**当前状态 |
| C3-10 | HTTP 分层、超时及缺少身份信息的展示语义**不回归** |
| C3-11 | C2 的 TSAP、隐藏字段、提交守卫及既有 347 项测试**零回归** |
| C3-12 | 全部 Required CI 通过，并有**服务端脱敏与无副作用**测试证据 |

## 7. 开发顺序（冻结）

| 阶段 | 内容 |
|---|---|
| C3-A | **后端契约**：实现 Endpoint-bound `connection-check`、Secret 内存物化与纯校验；先覆盖安全与错误语义 |
| C3-B | **临时 Probe 接入**：复用 `MesaManager::probe()`，验证凭据归属、超时、进程回收、对持久化状态零影响 |
| C3-C | **前端接入**：`ConnectionSetupPanel` 编辑态改走 Endpoint-bound 检查；新增与新建仍走原 Driver-bound 路径；保留 C2 统一草稿与 revision 门禁 |
| C3-D | **离线保存**：放通零采集点创建；提供明确的"仅保存，不启动"操作；把自动启动改为用户**明确选择** |
| C3-E | **综合验收**：Rust 契约测试、前端交互、API E2E、类型检查、Lint、全量测试与远程 CI；**全部不依赖实体机床** |

## 8. 已核实的事实基线（实施前核对过，避免建立在假设上）

- **路由注册点**：`crates/core-api/src/lib.rs:3149-3176`（`/api/v1/drivers*`、`/api/v1/endpoints/{id}/state`、`/browse` 等）；
  新增 `/api/v1/endpoints/{id}/connection-check` 应注册在同一处。
- **Secret 表按 Endpoint 维度**：`endpoint_secrets(endpoint_id, field_path, ...)`
  （`crates/config-store/src/lib.rs`：写入 1056/1100/1281/1854，删除 1108，读取 `get_secret` 1869）。
  ⇒ **C3-07 的跨 Endpoint 隔离有 schema 层面的支撑**，实现时只需保证查询始终带 `endpoint_id`。
- **可复用的边界（重要，勿误用）**：C3 **不得**直接调用
  `crates/core-api/src/runtime.rs` 的 `build_runtime_endpoint()`。
  该函数读取的是**已持久化**的完整 Endpoint 配置，并施加 **Data Tasks / Event Tasks 门禁**，
  而 C3 检查的对象是**尚未保存的连接草稿**。直接调用会导致两个错误：
  1. 测试的是**旧持久化参数**，而不是用户刚修改的草稿；
  2. 已保存任务的合法性会**干扰**本来只需要检查连接的操作。

  正确做法：复用 C1 的 **Secret 物化底层能力**（`runtime.rs` 中
  `materialize_connection()` 一类的原语，在坏 JSON 上 fail-closed），
  必要时在 Core 内提取一个**小型共享辅助函数**，使其能处理**内存中的草稿配置**。
  不得为此新建 SecretStore、通用 Resolver 框架或第二套连接模型。
- **bootstrap 已支持离线保存**：
  - `crates/core-api/src/bootstrap.rs:29` `start: Option<bool>`；
  - `:221` `if !acq.tasks.is_empty()` ⇒ **`tasks=[]` 可用**；
  - `:298` `desired_running: body.start.unwrap_or(true)` ⇒ 当前**默认 `true`**。

### 8.1 需要确认的一处默认值（影响 C3-D）

`body.start.unwrap_or(true)` 意味着**省略 `start` 即启动**。C3 要求"离线保存默认不启动"，
有两种做法：

- **(A) 前端始终显式传值**（推荐）：新建设备的"创建后启动"复选框**默认不勾选**，前端恒发送
  `start: true|false`；后端 `unwrap_or(true)` 保持不变。
  优点：不改变既有调用方的语义（向后兼容），默认值的改变是**产品行为**而不是**接口语义**。
- **(B) 后端默认翻转为 `false`**：任何省略 `start` 的既有调用方行为会静默改变。

C3-D 采用 **(A)**；如需 (B) 请明确指示。（本文档按 (A) 冻结。）

## 9. 与 C1/C2 的关系

- **C1**：Probe 分层预算（open/query/close）、`master.key` 保护与并发防覆盖、
  `PROBE_RPC_TIMEOUT`。C3 直接复用，**不重新实现**驱动通信与 Secret 存储。
- **C2**：`ConnectionSetupPanel` / `useConnectionSetup` / `connectionSetupModel`
  （统一草稿、revision 代际、`submitBlockReason` 提交守卫、`touched` 语义、`safeText`/`describeIssue` 脱敏渲染）。
  C3-C 只把**编辑态的检查入口**换到 Endpoint-bound 路径，其余保持不变。
- **C2 的"编辑态含 marker 即禁用测试"限制在本轮解除**——但**仅在** Endpoint-bound
  路径真正可用（C3-A/C3-B 通过验收）之后解除；在此之前保持禁用，不得显示虚假成功。
