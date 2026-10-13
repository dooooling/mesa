# 本机历史遗留 NCK 配置迁移证据（2026-10-13）

按《NCK 原生寻址重构 V2.0》§7.4「破坏性切换」执行并留档。
本文只记录**事实与命令结果**，不含任何凭据。

## 背景

本机 `mesa.db` 存有目录寻址时代的 NCK 端点：

| 项 | 值 |
|---|---|
| endpoint id | `lx-nck-tags-190-connection` |
| driver | `sinumerik-nck` |
| family / host / port | `828d` / `192.168.15.60` / `102` |
| local_tsap / remote_tsap | `1024` / `3332` |
| point_registry | 260 行（其中 20 行 `deleted=1`） |
| 升级前 desired_running | `true` |

## 1. 备份（升级前）

REST 之外的只读导出，落到 gitignore 目录，避免机床网络信息进公开仓库：

```text
target/nck-legacy-backup/legacy-nck-config.json   154272 bytes
  含 endpoints(3) / tasks(3) / point_registry(305) / devices
```

## 2. 升级后旧配置被明确拒绝（设计行为）

用新驱动二进制启动本机 `mesad`，查询端点状态：

```json
{
  "id": "lx-nck-tags-190-connection",
  "driver_id": "sinumerik-nck",
  "desired_running": true,
  "runtime": {
    "detail": "AddressError/INVALID_VARIABLE: point `程序状态`: 未知字段 `block`（新 Resource 不接受旧 block/variable/access_path 等字段，也不做自动迁移）",
    "points": 240,
    "state": "RECONNECTING"
  }
}
```

结论：旧 Task JSON **不会**被静默接受、**不会**被隐式推导为新地址，
也不会出现「保存正常但运行读了另一物理地址」。这与 Gate G5 的软件断言一致，
且是在真实本机部署上复现的。

## 3. 停用旧端点（按 §7.4）

```text
POST /api/v1/endpoints/lx-nck-tags-190-connection/stop
→ {"stopped":"lx-nck-tags-190-connection","was_running":true}
```

停用后复核：

```text
focas2-mtiog06g              driver=focas2         desired=False  state=-
lx-nck-tags-190-connection   driver=sinumerik-nck  desired=False  state=STOPPED
s7-mtiql34o                  driver=s7             desired=False  state=-
```

其余端点未受影响；`mesad` 已停止（恢复验证前状态：验证开始前它并未运行）。

## 4. 未做与需人工完成的部分

- **未直接改 SQLite**：遵守 `AGENTS.md`「CLI 仅作为 REST API 客户端，
  禁止直接读写 SQLite」。所有变更经 REST 完成。
- **未重建 240 点**：重建需要人工确认每个地址的 `area_no` 与 `line`，
  且 20 个原表点位的语义仍为 HOLD（见
  `nck-remaining-20-reconciliation.md`）。自动重建会把不确定写成默认值。
  重建路径见 `../drivers/sinumerik-nck/CONTRACT.md` §升级与迁移。
- 旧端点的配置行仍留在 `mesa.db`（`desired_running=false`），
  按方案不提供自动迁移；由使用者按新结构重建或删除。
