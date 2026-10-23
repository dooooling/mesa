# 828D 一期开机恢复策略（NCK 永不自动恢复）与升级迁移说明

- 状态：已实施（P0-C，产品策略）
- 适用范围：`driver_id = "sinumerik-nck"`（828D / 840D sl NCK 原生驱动）
- 明确**不是**通用生命周期语义：其他驱动（`s7` / `focas2` / `opcua` / `test-driver` 等）
  的开机恢复行为不变，后续是否单独收紧由独立决策确定。

## 1. 产品约束（P0-C）

**`sinumerik-nck` 端点开机一律不自动恢复**，无论：

- DB 的 `desired_running` 是否为 `true`；
- 停止意图围栏（`<db>.stop-fence.json` / `meta.stop_intent_unpersisted`）是否命中；
- 正向意图凭证（`<db>.intent-proof.json`）是否确认该端点；
- 该端点升级前是否正常运行、是否曾由门成功写入过 `desired_running=true`。

必须由现场**显式 `POST /api/v1/endpoints/{id}/start`** 才允许启动。

实现位点：

- 判定函数：`crates/core-api/src/recovery.rs`（`boot_auto_restore_allowed`，
  只做精确 `driver_id` 匹配）；
- 开机恢复循环：`apps/mesad/src/main.rs`，在 `desired_running=true` 之后、
  围栏/凭证判定之前直接 `continue`，并记录 `SKIP auto-restore` 告警。

为什么放在凭证判定之前：这是**产品策略**，不依赖"能否证明用户意图"。即使凭证完整
（升级后正常 Start 过），NCK 也不得被自动拉起。

## 2. 升级迁移行为（①，评审已接受）

G7 起开机恢复采用**默认不恢复 + 正向证明**：只有 `desired_running=true` 且
`<db>.intent-proof.json` 中确认过的端点才会自动恢复。升级前的旧库没有这份凭证。

因此升级后：

1. **首次重启**：升级前已在运行的端点（`desired_running=true`）因缺少正向凭证
   而**不会自动恢复**（fail-closed，宁可少恢复，不可误拉起机床端点）。
2. **现场处置**：对每个应运行的端点显式 `POST /start` 一次。Start 成功会写入
   正向凭证并清除停止意图围栏。
3. **后续重启**：非 NCK 端点恢复正常自动恢复；NCK 端点**任何重启都不自动恢复**
   （一期产品约束），每次开机后仍需显式 `POST /start`。

> 迁移不改写 DB 里的 `desired_running` 旧值：策略只阻止"自动拉起"，不替用户
> 修改意图；现场可通过 `GET /api/v1/endpoints` 看到 `desired_running=true` 但
> 运行态为 `STOPPED/UNKNOWN`，这是预期状态。

## 3. 现场通知要点（一句话）

**升级后首次重启不会自动恢复升级前在运行的端点，请重启后逐台显式
`POST /start` 确认；828D/NCK 端点任何重启都必须显式启动。**

## 4. 验证

- 单元：`crates/core-api/src/recovery.rs` 的 `recovery::tests::*`（NCK 拒绝 +
  其他驱动不受影响）。
- 进程级：`tests/driver-contract/tests/mesad_restart.rs`
  `nck_endpoint_is_never_auto_restored_at_boot`：同库构造
  `desired_running=true` + 无围栏 + 正向凭证已确认的 NCK 端点与 test-driver
  对照端点；重启后对照组 `RUNNING`，NCK 端点 `/state` 恒为 `NOT_FOUND`
  （从未被拉起），显式 `POST /start` 返回 200。

## 5. 非目标

- 不改变 `s7` / `focas2` / `opcua` 的开机恢复语义；
- 不改变围栏 / 正向凭证的既有判定；
- 不新增 schema、不新增 REST 字段、不改 wire/proto；
- 不把"NCK 不自动恢复"解释为通用生命周期规则。
