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
  ✅ 已过（Commit D）。
- Gate E（topology+browse）：probe（会话可达 + 身份诚实待确认）/
  topology 类型 / Catalog 虚拟浏览树（binding 回环可用）/ 管理面 E2E。
  ✅ 已过（Commit E）。probe anchor 与 topology 实例回填待真机 PR7。

## 真机门（840D sl，PR7）

connect / probe / channel+axis topology / 轴实际值与速度 / R 参数 /
程序与通道变量 / multi-read / 断线重连，逐代表变量形成
`官方定义 + Mesa 参数 + wire bytes + 设备返回 + HMI 对照` 证据行，
最小必要包做成回归 fixture。828D 需独立 Gate，不得由 840D 推断。

## 状态

- 当前：experimental（V1 地基完成：codec + 数据面 + probe/browse；
  随仓 catalog 为空，真机回填前生产 probe 身份恒待确认）。
- `supported` 需全部真机门通过后翻转。
