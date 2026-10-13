# SINUMERIK NCK 原生寻址重构发布记录（2026-10-13）

- 依据：`docs/adr/0004-nck-native-addressing.md`（决策与取舍）
- 驱动：`sinumerik-nck`；**状态仍为 experimental**（不因本次改造提升）
- 冻结标签 `nck-v0.5.0-freeze-20261013` → `80a8c81`：**未移动**（本次不触碰）
- 性质：**破坏性**——旧 NCK 配置不再兼容，且不提供自动迁移

## 交付物

| 交付物 | 位置 |
|---|---|
| 原生地址 Resource（Descriptor/解析器/唯一 Configure-Run 管道） | `src/native.rs`、`src/lib.rs` |
| 内建身份探针 + 有界 Browse 回归 | `src/identity_spec.rs`、`src/identity.rs` |
| 可选预填模板与来源清单 | `presets/828d.json`、`presets/PROVENANCE.md` |
| **可直接复制的工程配置示例**（R 参数/普通 NC 变量/SINAMICS 只读参数/`p0199`） | `presets/EXAMPLES.md`（数据源 `presets/examples-828d.json`，由 `tests/examples_828d.rs` 逐条复核） |
| 契约与门禁 | `CONTRACT.md`、`GATE.md`（Gate G / G-C） |
| 决策记录 | `docs/adr/0004-nck-native-addressing.md` |
| 真实硬件与迁移证据 | `docs/evidence/nck-live-native-probe.txt`、`nck-local-migration-2026-10-13.md`、`nck-rest-e2e.txt` |
| 独立验收入口 | `tests/legacy_equivalence.rs`、`scripts/nck-native-rest-e2e.py` |

## 变更摘要

1. **Catalog 退出采集准入**：删 `catalog.rs`、`browse.rs`、`topology.rs`、
   `catalog/*.json`、`codec::resolve`、`NckVariableRef`、`NckAccessPath`、
   `NckShape`、`CatalogRegistry`；`family` 降为设备档案（不是读取许可）。
2. **唯一原生地址模型**：`area/area_no/module/column/line/count/unit_mode/
   decode/transport_size` + 可选 `invalid_value/invalid_tolerance/unit`；
   `decode` 15 项规范词；`transport_size` 枚举 `0x05/0x06/0x07/0x09`。
3. **身份能力内建**：`IdentityProbeSpec`（8 个 828D 探针），不查目录、不依赖模板。
4. **配置知识转可选模板**：`presets/828d.json` 62 个模板（42 定义 ×
   NC 变量/驱动对象两条入口）；只预填结构固定字段，`area_no`/`line` 留给使用者。
5. **Browse 两类入口**：`nck://presets`（预填）与 `nck://identity`（只读核对）。
6. **无效工程值显式配置**：覆盖数值标量/数组/整型；未配置则不做工程语义判定。
7. **transport 语义按实测更正**：NCK 响应为 `0x05/0x06/0x07/0x09` + **字节**长度；
   早期脚手架用的 `0x04`+位长度（PLC S7ANY 习惯）已更正。

## 合并内容

| PR | 提交 | 内容 |
|---|---|---|
| #96 PR-A | `666bf6f` | 原生地址内核、内建身份探针、严格解析与单测（不切换 Schema） |
| #97 PR-B | `1a9c414` | 单资源切换、Catalog 链路删除、Preset/Identity Browse、文档改写 |
| #98 PR-C | `35724af` | 旧→新等价门、REST/Core 端到端回归、ADR 0004、来源清单、迁移步骤 |
| #99 | `586da98` | 契约门稳定性：staged 驱动二进制的有界重试（独立于 NCK） |

## 已完成的验证

- **契约门**（`scripts/write-contract-evidence.py`）：28 组全部通过。
  含 #99 后为 187 项（新增 1 条重试语义单测）；#99 之前为 186 项。
- **驱动与传输**：`cargo test -p mesa-driver-sinumerik-nck -p mesa-s7-transport` 全通过；
  `mesa-nck-emulator` 13 项（含 9 场景独立进程 E2E）。
- **旧→新寻址等价门**：`tests/legacy_equivalence.rs` 用 `main@78e23ec` 删除前冻结的
  62 条地址字节逐条断言原生路径复现。**等价只覆盖地址，不覆盖工程语义。**
- **REST/Core 端到端**：`scripts/nck-native-rest-e2e.py`（9 项断言全过）
  - 手写原生字段：保存→启动→GOOD，`source_label=nck://read/C/1/18/42/1/1?…`
  - 模板路径：Browse 根 = `nck://presets` + `nck://identity`；取模板叶（只含固定字段、
    标注 `candidate` 与 `待确认：area_no/line`）→ 补实例/行 → 保存 → GOOD
  - 旧目录字段：**保存时**被 Core Schema 以 `REQUIRED module/column/decode` 拒绝
- **实体机只读**：`docs/evidence/nck-live-native-probe.txt`（实体 828D，内建探针
  取值与旧目录路径一致；`V/6` module 26 仍被 `0x8201` 拒绝）。
- **Web**：`pnpm test` 230 项（含“不完整模板不绕过 required 前置”）、lint、build 通过。
- **CI**：#96/#97/#98/#99 在各自 HEAD 上全绿（含 stress / perf / web / 三平台）。

## 本机历史遗留迁移

见 `docs/evidence/nck-local-migration-2026-10-13.md`：备份 → 升级后旧端点被明确拒绝
（`未知字段 block`）→ 经 REST 停用（`desired_running=false`、`state=STOPPED`）。
240 点**未自动重建**：重建需人工确认 `area_no`/`line`，且 20 点语义仍为 HOLD。

## 待办（GC6，依赖合并）

- ⬜ 在四个 PR 合并后的最终 SHA 上重跑契约证据与 CI，并把该 SHA 补入本文件。
- ⬜ 未完成或仍 HOLD 的项不得在本记录中升格为已验证：
  `NCK_ANCHOR_PENDING`、TSAP confirmed、地址语义已确认、840D supported、
  剩余 20 点对账（7 项未定义资源 / 12 项轴归属未证明 / 1 项无效主轴温度）。

## 未决：驱动功能版本号

方案 §7.5 建议本次功能版本升为 `0.6.0`，但同时注明“依实际发布流程决定”。
本记录**未擅自升版**：`Cargo.toml` / `driver.toml` 仍为 `0.5.0`，
`protocol_major/minor` 也未改（它们只代表 Mesa IPC 协议兼容性，
不因资源参数破坏性变化而上调）。是否升 `0.6.0` 请按发布流程决定。
