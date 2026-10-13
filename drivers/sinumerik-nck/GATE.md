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
    Rust 离线回放（请求 exact differential；S7WLDouble 下双方对同一 wire
    解出相同语义数据——单读 8 字节 exact，多读双 buffer exact）✅；
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

- 当前：experimental（828d随仓目录提供公共候选映射，common/840d-sl仍为空；
  候选可解析与只读验证，不等于通过硬件Gate；生产probe身份恒待确认）。
- 真机前恒为 ❌（Gate F 通过也不翻转）：`NCK_ANCHOR_PENDING` / TSAP confirmed /
  Catalog mapping / 840D supported。
- `supported` 需全部真机门通过后翻转。

## 软件控制器的驱动对象证据

模块130是显式候选入口，正式入口名及硬件适用性待确认。新增目录、UTF-16
对象名和整数响应长度修复可由独立软件控制器向量及测试验证；不改变真机门。
对象浏览通过MD30110/MD30130与p0199唯一名称关联，未启用轴输出的对象分开
展示。r0027/r0031仍可能被设备拒绝，不能以r0078/r0080成功替代其验收。

## 实体828D的有限只读验证（2026-10-13）

用户确认的实体828D通过模块26读取r0027/r0031；X/Z/C/A的有效驱动输出
分别由MD30110、MD30130、MD35000及p0199核对。选定240个点通过Mesa
连续3轮完整采集，包含原表170点、18个独立驱动参数及52个身份核对点。
原表20点保留：7项未定义资源、12项未匹配轴、1项无效主轴温度。
r0035接近-200的无效标记按BAD处理，不以ReadVar返回FF判为有效工程值。

这些结果只证明该目标与所选地址的只读行为；HMI逐项语义对照、全部动态
工况、multi-read、断线重连等完整硬件Gate未完成。模块130仍保留候选状态，
不据部分参数或对象名成功将整个目录标成supported。
