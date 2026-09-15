// M2 DeviceLiveData：设备内实时数据（从 MonitorView 拆出，产品结构重做）。
// - 设备内不显示“设备”列（用户已在 Device Workspace 内）；全局 /data 才需要；
// - 连接过滤复用 M1 的 connection 上下文，不另起 selection state；
// - 行点击打开 PointDetailDrawer，不跳页面。
import { useMemo, useState } from "react";
import { Button, Input, Select, Space, Table, Tag } from "antd";
import { formatAge, formatPointValue } from "../deviceModel";
import type { DevicePointView, WorkspaceEndpoint } from "./useDeviceWorkspaceData";

type StatusFilter = "ALL" | "GOOD" | "BAD" | "STALE";

export function DeviceLiveData(props: {
  endpoints: WorkspaceEndpoint[];
  /** M1 解析后的有效连接（null = 全部）。 */
  effectiveEndpointId: string | null;
  points: DevicePointView[];
  pointsError: boolean;
  onOpenPoint: (p: DevicePointView) => void;
  onSelectConnection: (endpointId: string | null) => void;
}) {
  const { endpoints, effectiveEndpointId, points, pointsError, onOpenPoint, onSelectConnection } = props;
  const [status, setStatus] = useState<StatusFilter>("ALL");
  const [search, setSearch] = useState("");

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return points.filter((p) => {
      if (effectiveEndpointId && p.endpoint_id !== effectiveEndpointId) return false;
      if (status !== "ALL" && p.derived !== status) return false;
      if (!q) return true;
      return p.displayKey.toLowerCase().includes(q) || p.endpoint_id.toLowerCase().includes(q);
    });
  }, [points, effectiveEndpointId, status, search]);

  const counts = useMemo(() => {
    // 计数不受状态/搜索过滤影响，只受连接上下文影响（与标题语义一致）。
    const scoped = effectiveEndpointId ? points.filter((p) => p.endpoint_id === effectiveEndpointId) : points;
    let good = 0;
    let bad = 0;
    let stale = 0;
    for (const p of scoped) {
      if (p.derived === "GOOD") good += 1;
      else if (p.derived === "BAD") bad += 1;
      else if (p.derived === "STALE") stale += 1;
    }
    return { total: scoped.length, good, bad, stale };
  }, [points, effectiveEndpointId]);

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <Space wrap>
          <Select
            value={effectiveEndpointId ?? "ALL"}
            onChange={(v) => onSelectConnection(v === "ALL" ? null : v)}
            style={{ width: 180 }}
            options={[
              { value: "ALL", label: "全部连接" },
              ...endpoints.map((e) => ({ value: e.id, label: `${e.name ?? e.id}` })),
            ]}
          />
          <Select
            value={status}
            onChange={setStatus}
            style={{ width: 140 }}
            options={[
              { value: "ALL", label: "全部状态" },
              { value: "GOOD", label: "GOOD" },
              { value: "BAD", label: "BAD" },
              { value: "STALE", label: "STALE" },
            ]}
          />
          <Input
            placeholder="搜索点位 / key"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            style={{ width: 200 }}
            allowClear
          />
        </Space>
        {pointsError ? <Tag color="orange">快照更新失败（显示上次已知值）</Tag> : null}
      </div>

      <div style={{ fontSize: 12, color: "#525252" }}>
        全部 {counts.total} · GOOD {counts.good} · BAD {counts.bad} · STALE {counts.stale}
      </div>

      <Table
        size="small"
        rowKey={(r) => `${(r as DevicePointView).endpoint_id}:${(r as DevicePointView).point_id}`}
        dataSource={filtered}
        pagination={{ pageSize: 20 }}
        onRow={(r) => ({ onClick: () => onOpenPoint(r as DevicePointView), style: { cursor: "pointer" } })}
        columns={[
          {
            title: "数据点",
            render: (_: unknown, r: DevicePointView) => (
              <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>
                {r.displayKey}
              </span>
            ),
          },
          {
            title: "当前值",
            render: (_: unknown, r: DevicePointView) => (
              <span title={String(r.value ?? "")}>{formatPointValue(r.value)}</span>
            ),
          },
          { title: "类型", dataIndex: "type", render: (v: string) => <Tag>{v ?? "—"}</Tag> },
          {
            title: "连接",
            render: (_: unknown, r: DevicePointView) => (
              <span style={{ fontSize: 12 }}>{r.endpointName}</span>
            ),
          },
          {
            title: "状态",
            render: (_: unknown, r: DevicePointView) => (
              <Tag color={r.derived === "GOOD" ? "green" : r.derived === "BAD" ? "red" : "orange"}>
                {r.derived}
              </Tag>
            ),
          },
          {
            title: "更新",
            render: (_: unknown, r: DevicePointView) => <span>{formatAge(r.ageMs)}</span>,
          },
          {
            // M7 可访问性：键盘路径（行 onClick 仅鼠标可达）。
            title: "操作",
            width: 80,
            render: (_: unknown, r: DevicePointView) => (
              <Button size="small" type="link" onClick={(e) => { e.stopPropagation(); onOpenPoint(r); }}>
                详情
              </Button>
            ),
          },
        ]}
        locale={{ emptyText: "暂无数据" }}
      />
    </div>
  );
}
