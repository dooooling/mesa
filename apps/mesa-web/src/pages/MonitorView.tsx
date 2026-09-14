import { useEffect, useMemo, useState } from "react";
import { Card, Input, Select, Space, Table, Tag } from "antd";
import {
  POINT_STALE_AFTER_MS,
  formatAge,
  formatPointValue,
  pointAgeMs,
  resolveEndpointContexts,
} from "../deviceModel";

type Point = { endpoint_id: string; key: string; point_key?: string; point_id: number; quality: string; type: string; value: unknown; timestamp_ns: number };

// P0-3：Monitor 按 Device → Endpoint → Point 三级展示。“设备”列只放 Device
// 名，“连接”列放 Endpoint（名 + id）；过滤同时匹配设备名/连接名/连接 id/点位。
export function MonitorView() {
  const [points, setPoints] = useState<Point[]>([]);
  const [endpoints, setEndpoints] = useState<Array<{ id: string; name?: string; device_id?: string }>>([]);
  const [devices, setDevices] = useState<Array<{ id: string; name: string }>>([]);
  const [filter, setFilter] = useState("");
  const [quality, setQuality] = useState<string>("ALL");
  const [deviceFilter, setDeviceFilter] = useState<string>("ALL");
  const [endpointFilter, setEndpointFilter] = useState<string>("ALL");
  // 独立时钟（P1-1）：STALE 判定用的 now 必须每秒推进，不能只在 render 时
  // 取 Date.now()——points 轮询失败时无 setState、无 rerender，now 会冻结
  // 在最后一次成功时刻，age 永远停在 5s，正好在最需要 STALE 的故障场景失效。
  const [nowMs, setNowMs] = useState(() => Date.now());

  useEffect(() => {
    fetch("/api/v1/devices").then((r) => r.json()).then((j) => setDevices(j.devices ?? [])).catch(() => {});
    fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => {
      setEndpoints((j.endpoints ?? []).map((e: { id: string; name?: string; device_id?: string }) => ({
        id: e.id,
        name: e.name ?? e.id,
        device_id: e.device_id,
      })));
    }).catch(() => {});
    // points 快照 fail-closed：成功才替换；500/网络失败/坏形态一律保留
    // last-known points，nowMs 独立推进让它们自然进入 STALE（绝不能
    // 把错误包解释成 []，否则故障时旧点直接消失、无物可 STALE）。
    const tick = () => fetch("/api/v1/points/latest").then(async (r) => {
      if (!r.ok) throw new Error(`GET /points/latest ${r.status}`);
      return r.json();
    }).then((j) => {
      const pts = (j as { points?: unknown }).points;
      if (!Array.isArray(pts)) throw new Error("points 形态非法");
      setPoints(pts as Point[]);
    }).catch(() => {});
    tick();
    const id = window.setInterval(() => {
      // 时钟与拉取解耦：拉取失败也不阻止时钟推进（STALE 照常出现）。
      setNowMs(Date.now());
      tick();
    }, 1000);
    return () => window.clearInterval(id);
  }, []);

  const ctx = useMemo(() => resolveEndpointContexts(endpoints, devices), [endpoints, devices]);

  // 连接下拉随设备级联（设备切换时连接回到全部，避免无匹配死角）
  const endpointOptions = useMemo(() => {
    const list = deviceFilter === "ALL" ? endpoints : endpoints.filter((e) => (e.device_id ?? "") === deviceFilter);
    return [{ value: "ALL", label: "全部连接" }, ...list.map((e) => ({ value: e.id, label: e.name ?? e.id }))];
  }, [endpoints, deviceFilter]);

  const data = points.filter((p) => {
    const k = p.key ?? p.point_key ?? "";
    if (quality !== "ALL" && p.quality !== quality) return false;
    const c = ctx.get(p.endpoint_id);
    if (deviceFilter !== "ALL" && (c?.deviceId ?? "") !== deviceFilter) return false;
    if (endpointFilter !== "ALL" && p.endpoint_id !== endpointFilter) return false;
    if (!filter) return true;
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
        <Space wrap>
          <Select
            value={deviceFilter}
            onChange={(v) => { setDeviceFilter(v); setEndpointFilter("ALL"); }}
            style={{ width: 160 }}
            options={[{ value: "ALL", label: "全部设备" }, ...devices.map((d) => ({ value: d.id, label: d.name ?? d.id }))]}
          />
          <Select value={endpointFilter} onChange={setEndpointFilter} style={{ width: 160 }} options={endpointOptions} />
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
          { title: "值", render: (_: unknown, r: Point) => <span title={String(r.value ?? "")}>{formatPointValue(r.value)}</span> },
          { title: "类型", dataIndex: "type", render: (v: string) => <Tag>{v ?? "—"}</Tag> },
          { title: "质量", dataIndex: "quality", render: (v: string) => <Tag color={v === "GOOD" ? "green" : v === "BAD" ? "red" : "orange"}>{v}</Tag> },
          {
            title: "更新",
            render: (_: unknown, r: Point) => {
              const age = pointAgeMs(r.timestamp_ns, nowMs);
              const stale = age !== null && age > POINT_STALE_AFTER_MS;
              return (
                <span>
                  {formatAge(age)}{stale ? <Tag color="orange" style={{ marginLeft: 6 }}>STALE</Tag> : null}
                </span>
              );
            },
          },
          { title: "时间", render: (_: unknown, r: Point) => new Date(Number(r.timestamp_ns) / 1e6).toLocaleTimeString() },
        ]}
        locale={{ emptyText: "暂无数据 · 请在设备页点 点位 配置并启动" }}
      />
    </Card>
  );
}
