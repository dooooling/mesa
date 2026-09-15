// M2 DeviceLiveData：设备内实时数据（从 MonitorView 拆出，产品结构重做）。
// - 设备内不显示“设备”列（用户已在 Device Workspace 内）；全局 /data 才需要；
// - 连接筛选本页局部管理（useConnectionQuery allowAll + ConnectionSelect），
//   不再由父 Workspace 注入 Connection Context；
// - 行点击打开 PointDetailDrawer，不跳页面。
import { useMemo, useState } from "react";
import { Button, Input, Select, Space, Table, Tag } from "antd";
import { formatAge, formatPointValue } from "../deviceModel";
import type { DevicePointView, WorkspaceEndpoint } from "./useDeviceWorkspaceData";
import { ConnectionSelect } from "./ConnectionSelect";
import { PointNameCell } from "./PointNameCell";
import { useConnectionQuery } from "./useConnectionQuery";

type StatusFilter = "ALL" | "GOOD" | "BAD" | "STALE";

export function DeviceLiveData(props: {
  endpoints: WorkspaceEndpoint[];
  endpointsReady: boolean;
  points: DevicePointView[];
  pointsError: boolean;
  onOpenPoint: (p: DevicePointView) => void;
}) {
  const { endpoints, endpointsReady, points, pointsError, onOpenPoint } = props;
  const [status, setStatus] = useState<StatusFilter>("ALL");
  const [search, setSearch] = useState("");
  // 本页连接筛选（全部/单连接），URL 局部 ?connection=，不跨 Tab 同步。
  const endpointIds = useMemo(() => endpoints.map((e) => e.id), [endpoints]);
  const { selected: effectiveEndpointId, select: onSelectConnection } = useConnectionQuery({
    endpointIds,
    mode: "all",
    ready: endpointsReady,
  });

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return points.filter((p) => {
      if (effectiveEndpointId && p.endpoint_id !== effectiveEndpointId) return false;
      if (status !== "ALL" && p.derived !== status) return false;
      if (!q) return true;
      // P2：搜索同时匹配展示名与 point_key（改名后 key 仍是稳定可查身份）。
      const pointKey = p.key ?? p.point_key ?? "";
      return (
        p.displayKey.toLowerCase().includes(q) ||
        pointKey.toLowerCase().includes(q) ||
        p.endpoint_id.toLowerCase().includes(q) ||
        (p.sourceText ?? "").toLowerCase().includes(q)
      );
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
          <ConnectionSelect endpoints={endpoints} value={effectiveEndpointId} allowAll onChange={onSelectConnection} />
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
            // P2 双行：第一行展示名，第二行 point_key（仅命名时）。
            title: "数据点",
            render: (_: unknown, r: DevicePointView) => <PointNameCell point={r} />,
          },
          {
            // P1 来源列：有标签即标签；缺失显示"—"。
            // point_key 不再兼任来源。
            title: "来源",
            render: (_: unknown, r: DevicePointView) => (
              <span
                title={r.source_label ? `Driver 来源：${r.source_label}` : "Driver 未提供来源"}
                style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}
              >
                {r.sourceText ?? "—"}
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
