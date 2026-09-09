# SINUMERIK NCK Gate（进入 supported 的条件）

## 代码门

- Gate B（空壳）：descriptor 合法、`sinumerik-nck` 可被发现、旧 `sinumerik`
  零残留（discovery/contract/frontend）；运行期方法全部 `NOT_IMPLEMENTED`。
  ✅ 已过（Commit B）。
- Gate C（codec）：`0x82/83/84` exact-bytes golden + 回环 fixture
 （单读/多读/部分错/错基数/畸形/断开/PDU 边界）。
  ✅ 已过（Commit C）。
- Gate D（数据面）：configure/PointMap/Poll/DataBatch/LastKnown/Stop +
  driver contract + session-loss fail attempt + PDU 分片回环。
  ✅ 已过（Commit D；评审 P1-1/P1-2/P1-4/P2-1 修复后 exact gates 重验通过，
  含 JoinSet 双 task 回归 + transport/length 逐项校验 + 单 item 超 PDU 拒绝）。
- Gate E（topology+browse）：probe（会话可达 + 身份诚实待确认）/
  topology 类型 / Catalog 虚拟浏览树（binding 回环可用）/ 管理面 E2E。
  ✅ 已过（Commit E）。probe anchor 与 topology 实例回填待真机 PR7。
- Gate F（外部无真机验证，ADR 0002）：真机到手前可证明项的上限。
  - F1 source provenance pinned（`docs/evidence/nck-external-sources.lock.json`，
    三源 commit 锁定）。✅
  - F2 Wireshark assertions pinned（核对矩阵 #1–#6 入库；`wire_data_len`
    与主干解码数学一致）。✅
  - F3 Sharp7 reference vectors / interoperability：pinned vectors 入库 +
    Rust 离线回放（单读/多读 exact differential 为独立 wire evidence；
    响应为 compatibility evidence）✅；
    live 互操作（setup/单读/多读 rc=0）本地实证 ✅；
    非零 Area 差分与 `<<4` 仲裁仍 open ❌（Area=0 刻意规避）。
  - F4 Softing semantic evidence pinned（AWL 字段顺序 + 例证值入库）。✅
  - F5 standalone emulator process E2E（9 场景独立进程服务 + 握手验证）。✅
  - F6 disagreements explicitly unresolved（`<<4`/`0x06` 保持 open，
    不得静默修掉）。✅（保持 open 即通过项）

## 真机门（840D sl，PR7）

connect / probe / channel+axis topology / 轴实际值与速度 / R 参数 /
程序与通道变量 / multi-read / 断线重连，逐代表变量形成
`官方定义 + Mesa 参数 + wire bytes + 设备返回 + HMI 对照` 证据行，
最小必要包做成回归 fixture。828D 需独立 Gate，不得由 840D 推断。

## 状态

- 当前：experimental（V1 地基评审修复完成；随仓 catalog 为空，
  真机回填前生产 probe 身份恒待确认）。
- 真机前恒为 ❌（Gate F 通过也不翻转）：`NCK_ANCHOR_PENDING` / TSAP confirmed /
  Catalog mapping / 840D supported。
- `supported` 需全部真机门通过后翻转。
