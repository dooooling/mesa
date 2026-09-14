import { useEffect, useMemo, useState } from "react";
import { Card, Input, Select, Space, Table, Tag } from "antd";
import { resolveEndpointContexts } from "../deviceModel";

type Point = { endpoint_id: string; key: string; point_key?: string; point_id: number; quality: string; type: string; value: unknown; timestamp_ns: number };

// P0-3：Monitor 按 Device → Endpoint → Point 三级展示。“设备”列只放 Device
// 名，“连接”列放 Endpoint（名 + id）；过滤同时匹配设备名/连接名/连接 id/点位。
export function MonitorView() {
  const [points, setPoints] = useState<Point[]>([]);
  const [endpoints, setEndpoints] = useState<Array<{ id: string; name?: string; device_id?: string }>>([]);
  const [devices, setDevices] = useState<Array<{ id: string; name: string }>>([]);
  const [filter, setFilter] = useState("");
  const [quality, setQuality] = useState<string>("ALL");

  useEffect(() => {
    fetch("/api/v1/devices").then((r) => r.json()).then((j) => setDevices(j.devices ?? [])).catch(() => {});
    fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => {
      setEndpoints((j.endpoints ?? []).map((e: { id: string; name?: string; device_id?: string }) => ({
        id: e.id,
        name: e.name ?? e.id,
        device_id: e.device_id,
      })));
    }).catch(() => {});
    const tick = () => fetch("/api/v1/points/latest").then((r) => r.json()).then((j) => setPoints(j.points ?? [])).catch(() => {});
    tick();
    const id = window.setInterval(tick, 1000);
    return () => window.clearInterval(id);
  }, []);

  const ctx = useMemo(() => resolveEndpointContexts(endpoints, devices), [endpoints, devices]);

  const data = points.filter((p) => {
    const k = p.key ?? p.point_key ?? "";
    if (quality !== "ALL" && p.quality !== quality) return false;
    if (!filter) return true;
    const c = ctx.get(p.endpoint_id);
    return (
      k.includes(filter) ||
      p.endpoint_id.includes(filter) ||
      (c?.endpointName.includes(filter) ?? false) ||
      (c?.deviceName.includes(filter) ?? false)
    );
  });

  return (
    <Card
      size="small"
      title={`监控 · ${data.length}/${points.length}`}
      extra={
        <Space>
          <Select value={quality} onChange={setQuality} style={{ width: 140 }} options={[{ value: "ALL", label: "全部质量" }, { value: "GOOD", label: "GOOD" }, { value: "BAD", label: "BAD" }, { value: "UNCERTAIN", label: "UNCERTAIN" }]} />
          <Input placeholder="过滤设备/连接/点位" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ width: 180 }} allowClear />
        </Space>
      }
    >
      <Table
        size="small"
        rowKey={(r) => `${r.endpoint_id}:${r.point_id}`}
        dataSource={data}
        pagination={{ pageSize: 20 }}
        columns={[
          {
            title: "设备",
            render: (_: unknown, r: Point) => ctx.get(r.endpoint_id)?.deviceName ?? "—",
          },
          {
            title: "连接",
            render: (_: unknown, r: Point) => {
              const c = ctx.get(r.endpoint_id);
              return (
                <span>
                  <span style={{ fontSize: 12 }}>{c?.endpointName ?? r.endpoint_id}</span>
                  <span style={{ display: "block", fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11, color: "#525252" }}>
                    {r.endpoint_id}
                  </span>
                </span>
              );
            },
          },
          { title: "点位", render: (_: unknown, r: Point) => <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>{r.key ?? r.point_key ?? ""}</span> },
          { title: "值", render: (_: unknown, r: Point) => String(r.value ?? "") },
          { title: "类型", dataIndex: "type", render: (v: string) => <Tag>{v ?? "—"}</Tag> },
          { title: "质量", dataIndex: "quality", render: (v: string) => <Tag color={v === "GOOD" ? "green" : v === "BAD" ? "red" : "orange"}>{v}</Tag> },
          { title: "时间", render: (_: unknown, r: Point) => new Date(Number(r.timestamp_ns) / 1e6).toLocaleTimeString() },
        ]}
        locale={{ emptyText: "暂无数据 · 请在设备页点 点位 配置并启动" }}
      />
    </Card>
  );
}
