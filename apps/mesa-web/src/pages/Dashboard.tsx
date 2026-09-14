// Dashboard：诚实指标 + 状态不过期。
// - 设备 = /devices 数量；连接 = /endpoints 数量（PR27 已分离）；
// - 运行中 = 严格 RUNNING（过渡态 CONNECTING/RECONNECTING 不算在线）；
// - 最新点位 = /points/latest 当前条数（不是已配置数）；
// - BAD 质量 = 最新点位中 BAD 条数（不是健康模型，未来的 Healthy/Degraded
//   等 Health model 落地后再改名）；
// - 设备/连接每 10s 刷新一次（点位 2s），看板数字不许过期。
import { useEffect, useMemo, useState } from "react";
import { Alert, Card, Col, Row, Statistic, Table, Tag } from "antd";
import { useNavigate } from "react-router-dom";
import { isStrictlyRunning } from "../deviceModel";

type Point = { endpoint_id: string; key: string; point_key?: string; point_id: number; quality: string; type: string; value: unknown };

/** 数据新鲜度（P1-2）：2xx + 合法体才 ready；非 2xx / 网络失败一律 error，
 * 绝不能把错误包解释成 []（伪装成“系统真实为零”），也不能把旧值当当前值。 */
type Freshness = "loading" | "ready" | "error";

export function Dashboard() {
  const nav = useNavigate();
  const [devices, setDevices] = useState<Array<{ id: string; name: string }>>([]);
  const [endpoints, setEndpoints] = useState<Array<{ id: string; driver_id: string; state?: string }>>([]);
  const [points, setPoints] = useState<Point[]>([]);
  // inventory（设备/连接）与 points（点位）各自独立的新鲜度：失败即 error，
  // UI 明确显示“数据不可用”，不拿 [] 伪装成 0，也不拿旧值当当前值。
  const [inventoryState, setInventoryState] = useState<Freshness>("loading");
  const [pointsState, setPointsState] = useState<Freshness>("loading");

  useEffect(() => {
    let cancelled = false;
    const loadInventory = () => {
      // 两请求各自守卫：任一失败即 inventory error（不把错误包解释成 []）。
      Promise.all([
        fetch("/api/v1/devices").then(async (r) => {
          if (!r.ok) throw new Error(`GET /devices ${r.status}`);
          return r.json();
        }),
        fetch("/api/v1/endpoints").then(async (r) => {
          if (!r.ok) throw new Error(`GET /endpoints ${r.status}`);
          return r.json();
        }),
      ]).then(([dj, ej]) => {
        if (cancelled) return;
        const devs = (dj as { devices?: unknown }).devices;
        const eps = (ej as { endpoints?: unknown }).endpoints;
        // 形态守卫：错误包（如 {error:...}）无 devices/endpoints 数组即非法
        if (!Array.isArray(devs) || !Array.isArray(eps)) throw new Error("inventory 形态非法");
        setDevices(devs as Array<{ id: string; name: string }>);
        setEndpoints(
          (eps as Array<{ id: string; driver_id: string; runtime?: { state?: string }; state?: string }>).map((e) => ({
            id: e.id,
            driver_id: e.driver_id,
            state: e.state ?? e.runtime?.state,
          })),
        );
        setInventoryState("ready");
      }).catch(() => {
        if (!cancelled) setInventoryState("error");
      });
    };
    const loadPoints = () => fetch("/api/v1/points/latest").then(async (r) => {
      if (!r.ok) throw new Error(`GET /points/latest ${r.status}`);
      return r.json();
    }).then((j) => {
      if (cancelled) return;
      const pts = (j as { points?: unknown }).points;
      if (!Array.isArray(pts)) throw new Error("points 形态非法");
      setPoints(pts as Point[]);
      setPointsState("ready");
    }).catch(() => {
      if (!cancelled) setPointsState("error");
    });
    loadInventory();
    loadPoints();
    let n = 0;
    const id = window.setInterval(() => {
      n += 1;
      loadPoints();
      // 设备/连接低频刷新（每 5 轮 = 10s），避免看板状态过期
      if (n % 5 === 0) loadInventory();
    }, 2000);
    return () => { cancelled = true; window.clearInterval(id); };
  }, []);

  const running = useMemo(() => endpoints.filter((e) => isStrictlyRunning(e.state)).length, [endpoints]);
  const bad = useMemo(() => points.filter((p) => p.quality === "BAD").length, [points]);
  const inventoryStale = inventoryState === "error";
  const pointsStale = pointsState === "error";

  return (
    <div style={{ display: "grid", gap: 16 }}>
      {inventoryStale || pointsStale ? (
        <Alert
          type="warning"
          showIcon
          message="部分数据更新失败"
          description={[
            inventoryStale ? "设备/连接清单不可用（显示为上次已知值，已标 STALE）" : null,
            pointsStale ? "最新点位不可用（显示为上次已知值，已标 STALE）" : null,
          ].filter(Boolean).join("；")}
        />
      ) : null}
      <Row gutter={[16, 16]}>
        <Col xs={12} lg={5}><Card><Statistic title="设备" value={inventoryStale ? "STALE" : devices.length} /></Card></Col>
        <Col xs={12} lg={5}><Card><Statistic title="连接" value={inventoryStale ? "STALE" : endpoints.length} /></Card></Col>
        <Col xs={12} lg={5}><Card><Statistic title="运行中" value={inventoryStale ? "STALE" : running} valueStyle={{ color: running ? "#3f8600" : "#cf1322" }} /></Card></Col>
        <Col xs={12} lg={5}><Card><Statistic title="最新点位" value={pointsStale ? "STALE" : points.length} /></Card></Col>
        <Col xs={12} lg={4}><Card><Statistic title="BAD 质量" value={pointsStale ? "STALE" : bad} valueStyle={{ color: bad ? "#cf1322" : undefined }} /></Card></Col>
      </Row>

      <Row gutter={[16, 16]}>
        <Col xs={24} lg={12}>
          <Card title="设备" size="small" extra={<a onClick={() => nav("/devices")}>管理 →</a>}>
            <Table
              size="small"
              pagination={false}
              rowKey="id"
              dataSource={devices}
              onRow={(r) => ({ onClick: () => nav(`/devices/${r.id}`), style: { cursor: "pointer" } })}
              columns={[
                { title: "设备", dataIndex: "id", render: (v: string) => <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>{v}</span> },
                { title: "名称", dataIndex: "name", render: (v: string) => v ?? "—" },
              ]}
              locale={{ emptyText: "暂无设备" }}
            />
          </Card>
        </Col>
        <Col xs={24} lg={12}>
          <Card title="最新数据" size="small">
            <Table
              size="small"
              pagination={false}
              rowKey={(r) => `${(r as Point).endpoint_id}:${(r as Point).point_id}`}
              dataSource={points.slice(0, 8) as never[]}
              columns={[
                { title: "点位", render: (_: unknown, r: Point) => <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>{r.key ?? r.point_key ?? ""}</span> },
                { title: "值", render: (_: unknown, r: Point) => String(r.value ?? "") },
                { title: "质量", dataIndex: "quality", render: (v: string) => <Tag color={v === "GOOD" ? "green" : v === "BAD" ? "red" : "orange"}>{v}</Tag> },
              ]}
              locale={{ emptyText: "暂无数据" }}
            />
          </Card>
        </Col>
      </Row>
    </div>
  );
}
