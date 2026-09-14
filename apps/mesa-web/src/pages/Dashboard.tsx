// Dashboard：诚实指标 + 状态不过期。
// - 设备 = /devices 数量；连接 = /endpoints 数量（PR27 已分离）；
// - 运行中 = 严格 RUNNING（过渡态 CONNECTING/RECONNECTING 不算在线）；
// - 最新点位 = /points/latest 当前条数（不是已配置数）；
// - BAD 质量 = 最新点位中 BAD 条数（不是健康模型，未来的 Healthy/Degraded
//   等 Health model 落地后再改名）；
// - 设备/连接每 10s 刷新一次（点位 2s），看板数字不许过期。
import { useEffect, useMemo, useState } from "react";
import { Card, Col, Row, Statistic, Table, Tag } from "antd";
import { useNavigate } from "react-router-dom";
import { isStrictlyRunning } from "../deviceModel";

type Point = { endpoint_id: string; key: string; point_key?: string; point_id: number; quality: string; type: string; value: unknown };

export function Dashboard() {
  const nav = useNavigate();
  const [devices, setDevices] = useState<Array<{ id: string; name: string }>>([]);
  const [endpoints, setEndpoints] = useState<Array<{ id: string; driver_id: string; state?: string }>>([]);
  const [points, setPoints] = useState<Point[]>([]);

  useEffect(() => {
    const loadInventory = () => {
      fetch("/api/v1/devices").then((r) => r.json()).then((j) => {
        setDevices(j.devices ?? []);
      }).catch(() => {});
      fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => {
        const eps = (j.endpoints ?? []).map((e: never) => {
          const x = e as { id: string; driver_id: string; runtime?: { state?: string }; state?: string };
          return { id: x.id, driver_id: x.driver_id, state: x.state ?? x.runtime?.state };
        });
        setEndpoints(eps);
      }).catch(() => {});
    };
    const loadPoints = () => fetch("/api/v1/points/latest").then((r) => r.json()).then((j) => setPoints(j.points ?? [])).catch(() => {});
    loadInventory();
    loadPoints();
    let n = 0;
    const id = window.setInterval(() => {
      n += 1;
      loadPoints();
      // 设备/连接低频刷新（每 5 轮 = 10s），避免看板状态过期
      if (n % 5 === 0) loadInventory();
    }, 2000);
    return () => window.clearInterval(id);
  }, []);

  const running = useMemo(() => endpoints.filter((e) => isStrictlyRunning(e.state)).length, [endpoints]);
  const bad = useMemo(() => points.filter((p) => p.quality === "BAD").length, [points]);

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Row gutter={[16, 16]}>
        <Col xs={12} lg={5}><Card><Statistic title="设备" value={devices.length} /></Card></Col>
        <Col xs={12} lg={5}><Card><Statistic title="连接" value={endpoints.length} /></Card></Col>
        <Col xs={12} lg={5}><Card><Statistic title="运行中" value={running} valueStyle={{ color: running ? "#3f8600" : "#cf1322" }} /></Card></Col>
        <Col xs={12} lg={5}><Card><Statistic title="最新点位" value={points.length} /></Card></Col>
        <Col xs={12} lg={4}><Card><Statistic title="BAD 质量" value={bad} valueStyle={{ color: bad ? "#cf1322" : undefined }} /></Card></Col>
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
