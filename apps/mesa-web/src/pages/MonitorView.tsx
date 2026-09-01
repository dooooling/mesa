import { useEffect, useState } from "react";
import { Card, Input, Select, Space, Table, Tag } from "antd";

type Point = { endpoint_id: string; point_key: string; point_id: number; quality: string; value: { type: string; value: unknown }; timestamp_ns: number };

export function MonitorView() {
  const [points, setPoints] = useState<Point[]>([]);
  const [filter, setFilter] = useState("");
  const [quality, setQuality] = useState<string>("ALL");

  useEffect(() => {
    const tick = () => fetch("/api/v1/points/latest").then((r) => r.json()).then((j) => setPoints(j.points ?? [])).catch(() => {});
    tick();
    const id = window.setInterval(tick, 1000);
    return () => window.clearInterval(id);
  }, []);

  const data = points.filter((p) => {
    if (quality !== "ALL" && p.quality !== quality) return false;
    if (filter && !(p.point_key.includes(filter) || p.endpoint_id.includes(filter))) return false;
    return true;
  });

  return (
    <Card
      size="small"
      title={`监控 · ${data.length}/${points.length}`}
      extra={
        <Space>
          <Select value={quality} onChange={setQuality} style={{ width: 140 }} options={[{ value: "ALL", label: "全部质量" }, { value: "GOOD", label: "GOOD" }, { value: "BAD", label: "BAD" }, { value: "UNCERTAIN", label: "UNCERTAIN" }]} />
          <Input placeholder="过滤点位/设备" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ width: 180 }} allowClear />
        </Space>
      }
    >
      <Table
        size="small"
        rowKey={(r) => `${r.endpoint_id}:${r.point_id}`}
        dataSource={data}
        pagination={{ pageSize: 20 }}
        columns={[
          { title: "设备", dataIndex: "endpoint_id", render: (v: string) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{v}</span> },
          { title: "点位", dataIndex: "point_key", render: (v: string) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{v}</span> },
          { title: "值", render: (_: unknown, r: Point) => String(r.value.value) },
          { title: "类型", render: (_: unknown, r: Point) => <Tag>{r.value.type}</Tag> },
          { title: "质量", dataIndex: "quality", render: (v: string) => <Tag color={v === "GOOD" ? "green" : v === "BAD" ? "red" : "orange"}>{v}</Tag> },
          { title: "时间", render: (_: unknown, r: Point) => new Date(Number(r.timestamp_ns) / 1e6).toLocaleTimeString() },
        ]}
        locale={{ emptyText: "暂无数据 · 请在设备页点 点位 配置并启动" }}
      />
    </Card>
  );
}
