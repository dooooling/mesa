import { useEffect, useMemo, useState } from "react";
import { Card, Col, Row, Statistic, Table, Tag } from "antd";

export function Dashboard() {
  const [endpoints, setEndpoints] = useState<Array<{ id: string; driver_id: string; state?: string }>>([]);
  const [points, setPoints] = useState<Array<{ endpoint_id: string; point_key: string; quality: string; value: { type: string; value: unknown } }>>([]);

  useEffect(() => {
    fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => setEndpoints(j.endpoints ?? [])).catch(() => {});
    const tick = () => fetch("/api/v1/points/latest").then((r) => r.json()).then((j) => setPoints(j.points ?? [])).catch(() => {});
    tick();
    const id = window.setInterval(tick, 2000);
    return () => window.clearInterval(id);
  }, []);

  const online = useMemo(() => endpoints.filter((e) => e.state === "running").length, [endpoints]);
  const bad = useMemo(() => points.filter((p) => p.quality === "BAD").length, [points]);

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Row gutter={[16, 16]}>
        <Col xs={12} lg={6}><Card><Statistic title="设备" value={endpoints.length} /></Card></Col>
        <Col xs={12} lg={6}><Card><Statistic title="在线" value={online} valueStyle={{ color: online ? "#3f8600" : "#cf1322" }} /></Card></Col>
        <Col xs={12} lg={6}><Card><Statistic title="点位" value={points.length} /></Card></Col>
        <Col xs={12} lg={6}><Card><Statistic title="异常" value={bad} valueStyle={{ color: bad ? "#cf1322" : undefined }} /></Card></Col>
      </Row>

      <Row gutter={[16, 16]}>
        <Col xs={24} lg={12}>
          <Card title="设备" size="small">
            <Table
              size="small"
              pagination={false}
              rowKey="id"
              dataSource={endpoints}
              columns={[
                { title: "设备", dataIndex: "id", render: (v: string) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{v}</span> },
                { title: "驱动", dataIndex: "driver_id", render: (v: string) => <Tag>{v}</Tag> },
                { title: "状态", dataIndex: "state", render: (v: string) => <Tag color={v === "running" ? "green" : v === "error" ? "red" : "default"}>{v ?? "—"}</Tag> },
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
              rowKey={(r) => `${(r as { endpoint_id: string }).endpoint_id}:${(r as { point_key: string }).point_key}`}
              dataSource={points.slice(0, 8) as never[]}
              columns={[
                { title: "点位", dataIndex: "point_key", render: (v: string) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{v}</span> },
                { title: "值", render: (_: unknown, r: never) => String((r as { value: { value: unknown } }).value.value) },
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
