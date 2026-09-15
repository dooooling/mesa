// M3.1 Global Live Data：跨设备聚合搜索（设备限制去掉，其余与设备页同源）。
// - 共用 useLivePointsSource（快照/STALE/归属语义与 M2 同一套）；
// - 共用 PointDetailDrawer（只多展示来源 Device，不复制 Drawer）；
// - device/connection 进 URL（?device=&connection=），状态/搜索为页面内状态；
// - 行内“打开设备”闭环回 /devices/:id/data?connection=。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Input, Select, Space, Table, Tag } from "antd";
import { useNavigate, useSearchParams } from "react-router-dom";
import { formatAge, formatPointValue } from "../deviceModel";
import { PointDetailDrawer } from "../components/PointDetailDrawer";
import type { DevicePointView } from "../workspace/useDeviceWorkspaceData";
import { AgeCell, NameCell, SourceCell, StatusCell, ValueCell } from "../workspace/LiveCells";
import { useLivePointsSource } from "../workspace/useLivePointsSource";

type StatusFilter = "ALL" | "GOOD" | "BAD" | "STALE";

export function GlobalDataPage() {
  const nav = useNavigate();
  const [params, setParams] = useSearchParams();
  const src = useLivePointsSource();
  const [status, setStatus] = useState<StatusFilter>("ALL");
  const [search, setSearch] = useState("");
  const [openPoint, setOpenPoint] = useState<DevicePointView | null>(null);

  const deviceParam = params.get("device") ?? "ALL";
  const connectionParam = params.get("connection") ?? "ALL";

  // 连接下拉随设备级联：设备切换时连接回到全部，避免无匹配死角（与旧 Monitor 同规则）。
  const endpointOptions = useMemo(() => {
    const list =
      deviceParam === "ALL" ? src.endpoints : src.endpoints.filter((e) => (e.device_id ?? "") === deviceParam);
    return [{ value: "ALL", label: "全部连接" }, ...list.map((e) => ({ value: e.id, label: e.name ?? e.id }))];
  }, [src.endpoints, deviceParam]);

  // URL 合法性：设备/连接 id 不在清单里时视为 ALL（清单未就绪前不动 URL）。
  useEffect(() => {
    if (!src.endpointsReady) return;
    const deviceIds = new Set(src.devices.map((d) => d.id));
    const endpointIds = new Set(src.endpoints.map((e) => e.id));
    const next = new URLSearchParams(params);
    let changed = false;
    if (deviceParam !== "ALL" && !deviceIds.has(deviceParam)) {
      next.delete("device");
      changed = true;
    }
    if (connectionParam !== "ALL" && !endpointIds.has(connectionParam)) {
      next.delete("connection");
      changed = true;
    }
    // 连接不归属所选设备时同样回 ALL（避免死角）。
    if (!changed && deviceParam !== "ALL" && connectionParam !== "ALL") {
      const ep = src.endpoints.find((e) => e.id === connectionParam);
      if (ep && (ep.device_id ?? "") !== deviceParam) {
        next.delete("connection");
        changed = true;
      }
    }
    if (changed) setParams(next, { replace: true });
  }, [src.endpointsReady, src.devices, src.endpoints, deviceParam, connectionParam, params, setParams]);

  const setDevice = (v: string) => {
    const next = new URLSearchParams(params);
    if (v === "ALL") next.delete("device");
    else next.set("device", v);
    // 设备切换时连接回到全部
    next.delete("connection");
    setParams(next);
  };
  const setConnection = (v: string) => {
    const next = new URLSearchParams(params);
    if (v === "ALL") next.delete("connection");
    else next.set("connection", v);
    setParams(next);
  };

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return src.allPoints.filter((p) => {
      // RC2 修1：选了具体设备时归属必须可证明（p.deviceId === deviceParam），
      // 未知归属一律排除——“无法判断”绝不解释成“可能属于所选设备”。
      // device=ALL 是聚合本职，全显（含未知归属，归属列明示）。
      if (deviceParam !== "ALL" && p.deviceId !== deviceParam) return false;
      if (connectionParam !== "ALL" && p.endpoint_id !== connectionParam) return false;
      if (status !== "ALL" && p.derived !== status) return false;
      if (!q) return true;
      // P2：展示名与 point_key 都可搜（改名不丢稳定身份）。
      const pointKey = p.key ?? p.point_key ?? "";
      return (
        p.displayKey.toLowerCase().includes(q) ||
        pointKey.toLowerCase().includes(q) ||
        p.endpoint_id.toLowerCase().includes(q) ||
        p.deviceName.toLowerCase().includes(q) ||
        p.endpointName.toLowerCase().includes(q) ||
        (p.sourceText ?? "").toLowerCase().includes(q)
      );
    });
  }, [src.allPoints, deviceParam, connectionParam, status, search]);

  const drawerDeviceName =
    openPoint == null
      ? ""
      : (src.devices.find((d) => d.id === openPoint.deviceId)?.name ?? openPoint.deviceName ?? openPoint.deviceId);

  // 列定义 memo 化 + 固定布局（与设备页同口径，Layout shift 根治）。
  const columns = useMemo(
    () => [
      {
        title: "设备",
        width: 140,
        render: (_: unknown, r: DevicePointView) => (
          <span style={{ fontSize: 12, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", display: "block" }}>
            {r.deviceName || r.deviceId || "—"}
          </span>
        ),
      },
      {
        title: "连接",
        width: 140,
        render: (_: unknown, r: DevicePointView) => (
          <span style={{ fontSize: 12, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", display: "block" }}>{r.endpointName}</span>
        ),
      },
      {
        title: "数据点",
        width: 240,
        render: (_: unknown, r: DevicePointView) => (
          <NameCell
            displayKey={r.displayKey}
            pointKey={r.key ?? r.point_key ?? String(r.point_id)}
            displayName={r.display_name}
          />
        ),
      },
      {
        title: "来源",
        width: 220,
        render: (_: unknown, r: DevicePointView) => (
          <SourceCell sourceText={r.sourceText} sourceLabel={r.source_label} />
        ),
      },
      {
        title: "当前值",
        width: 180,
        render: (_: unknown, r: DevicePointView) => <ValueCell value={r.value} />,
      },
      { title: "类型", width: 90, dataIndex: "type", render: (v: string) => <Tag>{v ?? "—"}</Tag> },
      {
        title: "状态",
        width: 90,
        render: (_: unknown, r: DevicePointView) => <StatusCell derived={r.derived} />,
      },
      {
        title: "更新",
        width: 90,
        render: (_: unknown, r: DevicePointView) => <AgeCell timestampNs={r.timestamp_ns} />,
      },
      {
        title: "操作",
        width: 130,
        render: (_: unknown, r: DevicePointView) => (
          <Space onClick={(e) => e.stopPropagation()}>
            {/* M7 可访问性：详情是 Drawer 的键盘路径（行 onClick 仅鼠标可达）。 */}
            <Button size="small" type="link" onClick={() => setOpenPoint(r)}>
              详情
            </Button>
            <Button
              size="small"
              type="link"
              disabled={!r.deviceId}
              onClick={() =>
                nav(`/devices/${r.deviceId}/data${r.endpoint_id ? `?connection=${r.endpoint_id}` : ""}`)
              }
            >
              打开设备 →
            </Button>
          </Space>
        ),
      },
    ],
    [setOpenPoint, nav],
  );

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <Card
        size="small"
        title={`实时数据 · ${filtered.length}/${src.allPoints.length}`}
        extra={
          <Space wrap>
            <Select
              value={deviceParam}
              onChange={setDevice}
              style={{ width: 160 }}
              options={[{ value: "ALL", label: "全部设备" }, ...src.devices.map((d) => ({ value: d.id, label: d.name ?? d.id }))]}
            />
            <Select value={connectionParam} onChange={setConnection} style={{ width: 160 }} options={endpointOptions} />
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
            <Input placeholder="搜索设备/连接/点位" value={search} onChange={(e) => setSearch(e.target.value)} style={{ width: 200 }} allowClear />
          </Space>
        }
      >
        {src.endpointsError ? <Alert type="warning" showIcon message="连接清单不可用" description={src.endpointsError} style={{ marginBottom: 8 }} /> : null}
        {src.pointsError ? <Alert type="warning" showIcon message="快照更新失败（显示上次已知值）" style={{ marginBottom: 8 }} /> : null}
        <div style={{ fontSize: 12, color: "#525252", marginBottom: 8 }}>
          全部 {src.counts.total} · GOOD {src.counts.good} · BAD {src.counts.bad} · STALE {src.counts.stale}
        </div>
        <Table
          size="small"
          rowKey={(r) => `${(r as DevicePointView).endpoint_id}:${(r as DevicePointView).point_id}`}
          tableLayout="fixed"
          dataSource={filtered}
          pagination={{ pageSize: 20 }}
          onRow={(r) => ({ onClick: () => setOpenPoint(r as DevicePointView), style: { cursor: "pointer" } })}
          columns={columns}
          locale={{ emptyText: "暂无数据" }}
        />
      </Card>

      <PointDetailDrawer
        deviceId={openPoint?.deviceId ?? ""}
        deviceName={drawerDeviceName}
        point={openPoint}
        onClose={() => setOpenPoint(null)}
        onRenamed={(target, display_name) => {
          src.patchDisplayName(target.endpoint_id, target.point_key, display_name);
          setOpenPoint((prev) =>
            prev !== null &&
            prev.endpoint_id === target.endpoint_id &&
            (prev.key ?? prev.point_key) === target.point_key
              ? { ...prev, display_name: display_name ?? undefined }
              : prev,
          );
        }}
      />
    </div>
  );
}
