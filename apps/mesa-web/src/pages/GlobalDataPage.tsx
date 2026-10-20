// M3.1 Global Live Data：跨设备聚合搜索（设备限制去掉，其余与设备页同源）。
// - 共用 useLivePointsSource（快照/STALE/归属语义与 M2 同一套）；
// - 共用 PointDetailDrawer（只多展示来源 Device，不复制 Drawer）；
// - device/connection 进 URL（?device=&connection=），状态/搜索为页面内状态；
// - 行内“打开设备”闭环回 /devices/:id/data?connection=。
import { useCallback, useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Input, Select, Space, Table, Tag } from "antd";
import { useNavigate, useSearchParams } from "react-router-dom";
import { formatAge, formatPointValue } from "../deviceModel";
import { PointDetailDrawer, pointIdentityOf, type PointIdentity } from "../components/PointDetailDrawer";
import { QualityCodeBreakdown } from "../components/QualityCodeBreakdown";
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
  // PR-D1 缺陷 1：只记定位标识；Drawer 渲染时从 src.allPoints（SSE 合并结果）
  // 查找，SSE 改码/删点即时反映，不持有点击时的行快照。
  const [openPoint, setOpenPoint] = useState<PointIdentity | null>(null);
  const openPointRow = useCallback(
    (p: DevicePointView) => setOpenPoint(pointIdentityOf(p)),
    [],
  );

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

  // PR-D1：按码分解/导出的作用域集合（连接筛选后）。memo 化以保住
  // Popover 内 useMemo 的依赖引用（§7.3：只在点击展开时算一次）。
  const breakdownPoints = useMemo(
    () =>
      connectionParam === "ALL"
        ? src.allPoints
        : src.allPoints.filter((p) => p.endpoint_id === connectionParam),
    [src.allPoints, connectionParam],
  );

  // 裁决 2：计数行与 Popover / 导出分析**共用同一个连接范围**（connectionParam）。
  // 旧实现计数行用 src.counts（全局、无视连接筛选），与 Popover/导出不一致：
  // 选单连接后计数仍显示全部连接的点，用户会以为筛选没生效。
  // 注意：设备筛选只作用于表格，不参与统计范围——当 connection=ALL 时统计
  // 会跨设备，必须由 scopeLabel 明示（见渲染处），不能让人误以为已按单连接过滤。
  const scopedCounts = useMemo(() => {
    let good = 0;
    let bad = 0;
    let stale = 0;
    let unknown = 0;
    for (const p of breakdownPoints) {
      if (p.derived === "GOOD") good += 1;
      else if (p.derived === "BAD") bad += 1;
      else if (p.derived === "STALE") stale += 1;
      else unknown += 1;
    }
    return { total: breakdownPoints.length, good, bad, stale, unknown };
  }, [breakdownPoints]);

  // 连接=ALL ⇒ 统计范围是全部连接（可能跨设备）；必须显式标注。
  const scopeLabel = connectionParam === "ALL" ? "全部连接（跨设备）" : null;

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

  // Drawer 的设备名从**连接映射**反查（不依赖已消失的行）；行还在时再回退
  // 到行上的 deviceName，保证点被删除后 Drawer 仍有可读来源。
  const openEndpoint =
    openPoint === null ? null : (src.endpoints.find((e) => e.id === openPoint.endpoint_id) ?? null);
  const drawerDeviceId = openEndpoint?.device_id ?? "";
  const drawerPoint =
    openPoint === null
      ? null
      : (src.allPoints.find(
          (p) => p.endpoint_id === openPoint.endpoint_id && p.point_id === openPoint.point_id,
        ) ?? null);
  const drawerDeviceName =
    (drawerDeviceId
      ? (src.devices.find((d) => d.id === drawerDeviceId)?.name ?? drawerDeviceId)
      : "") || drawerPoint?.deviceName || "";

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
            <Button size="small" type="link" onClick={() => openPointRow(r)}>
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
    [openPointRow, nav],
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
          {/* PR-D1 U8：计数行**仅点击**打开按码分解；导出作用域 = 当前连接筛选。 */}
          <QualityCodeBreakdown
            points={breakdownPoints}
            scopeEndpointId={connectionParam === "ALL" ? null : connectionParam}
          >
            全部 {scopedCounts.total} · GOOD {scopedCounts.good} · BAD {scopedCounts.bad} · STALE{" "}
            {scopedCounts.stale}
          </QualityCodeBreakdown>
          {/* 裁决 2 特殊情况：连接=ALL 时统计跨设备，必须显式标注。 */}
          {scopeLabel === null ? null : (
            <span data-testid="data-scope-label" style={{ marginLeft: 8 }}>
              {scopeLabel}
            </span>
          )}
        </div>
        <Table
          size="small"
          rowKey={(r) => `${(r as DevicePointView).endpoint_id}:${(r as DevicePointView).point_id}`}
          tableLayout="fixed"
          dataSource={filtered}
          pagination={{ pageSize: 20 }}
          onRow={(r) => ({ onClick: () => openPointRow(r as DevicePointView), style: { cursor: "pointer" } })}
          columns={columns}
          locale={{ emptyText: "暂无数据" }}
        />
      </Card>

      <PointDetailDrawer
        deviceId={drawerDeviceId}
        deviceName={drawerDeviceName}
        points={src.allPoints}
        selected={openPoint}
        onClose={() => setOpenPoint(null)}
        onRenamed={(target, display_name) => {
          src.patchDisplayName(target.endpoint_id, target.point_key, display_name);
        }}
      />
    </div>
  );
}
