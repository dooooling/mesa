// M6 DeviceDiagnostics：设备 Workspace「诊断」tab。
// - 单连接状态：endpoint diagnostics（运行态/驱动版本/连接状态/重连计数）；
// - 采集健康：该连接 tasks 快照（数量/revision）+ 其 points 的 GOOD/BAD/STALE；
// - 高级诊断折叠：diagnostics 原始 JSON（排障用，不做二次解释）。
// 全部只读，不触发任何变更；连接选择本页局部（单选，无连接即 Empty）。
// 切连接 key remount，迟到响应不污染新窗格。
import { useEffect, useMemo, useState } from "react";
import { Alert, Card, Collapse, Descriptions, Table, Tag } from "antd";
import { api } from "../api";
import { isRunningState } from "../deviceModel";
import { ConnectionSelect } from "./ConnectionSelect";
import { useConnectionQuery } from "./useConnectionQuery";
import type { DevicePointView, WorkspaceEndpoint } from "./useDeviceWorkspaceData";

interface EndpointDiagnostics {
  connection_state?: string;
  desired_running?: boolean;
  descriptor_state?: string;
  driver_id?: string;
  driver_version?: string;
  reconnect_attempt_total?: number;
  point_count?: number;
  last_connected_at_ns?: number | null;
  runtime?: { state?: string; detail?: string; revision?: number };
}

function fmtTime(ns?: number | null): string {
  if (typeof ns !== "number" || !Number.isFinite(ns) || ns <= 0) return "—";
  return new Date(ns / 1e6).toLocaleString();
}

export function DeviceDiagnostics({
  endpoints,
  endpointsReady,
  points,
}: {
  endpoints: WorkspaceEndpoint[];
  endpointsReady: boolean;
  points: DevicePointView[];
  counts?: { good: number; bad: number; stale: number };
}) {
  // 本页连接选择（单选）：合法 query 沿用，缺失/非法回第一个，无连接即 Empty。
  const endpointIds = useMemo(() => endpoints.map((e) => e.id), [endpoints]);
  const { selected: effectiveEndpointId, select: onSelectConnection } = useConnectionQuery({
    endpointIds,
    mode: "single",
    ready: endpointsReady,
  });
  const active = endpoints.find((e) => e.id === effectiveEndpointId) ?? null;
  const [diag, setDiag] = useState<EndpointDiagnostics | null>(null);
  const [diagError, setDiagError] = useState("");
  const [diagLoading, setDiagLoading] = useState(false);
  const [taskCount, setTaskCount] = useState<number | null>(null);

  useEffect(() => {
    if (!active) {
      setDiag(null);
      setDiagError("");
      setTaskCount(null);
      return;
    }
    let cancelled = false;
    setDiagLoading(true);
    setDiagError("");
    Promise.all([
      api.endpointDiagnostics(active.id).catch((e) => ({ __error: e instanceof Error ? e.message : String(e) })),
      fetch(`/api/v1/tasks?endpoint=${active.id}`)
        .then(async (x) => {
          if (!x.ok) throw new Error(`GET /tasks ${x.status}`);
          return x.json();
        })
        .catch((e) => ({ __error: e instanceof Error ? e.message : String(e) })),
    ]).then(([d, t]) => {
      if (cancelled) return;
      if ((d as { __error?: string }).__error) setDiagError((d as { __error: string }).__error);
      else setDiag(d as EndpointDiagnostics);
      const terr = (t as { __error?: string }).__error;
      if (!terr) setTaskCount(((t as { tasks?: unknown[] }).tasks ?? []).length);
      else setTaskCount(null);
      setDiagLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [active?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!active) {
    return <Alert type="warning" showIcon message="该设备暂无连接" description="请先到「连接」页新增连接后再诊断。" />;
  }

  const epPoints = points.filter((p) => p.endpoint_id === active.id);
  const good = epPoints.filter((p) => p.derived === "GOOD").length;
  const bad = epPoints.filter((p) => p.derived === "BAD").length;
  const stale = epPoints.filter((p) => p.derived === "STALE").length;

  return (
    <div key={active.id} style={{ display: "grid", gap: 16 }}>
      <div style={{ display: "flex", alignItems: "center" }}>
        <span style={{ marginLeft: "auto" }}>
          <ConnectionSelect endpoints={endpoints} value={active.id} allowAll={false} onChange={onSelectConnection} />
        </span>
      </div>
      <Card size="small" title={`连接状态 · ${active.name ?? active.id}`} extra={<Tag>{active.driver_id}</Tag>}>
        {diagLoading ? (
          <div style={{ fontSize: 12, color: "#525252" }}>加载诊断中…</div>
        ) : diagError ? (
          <Alert type="error" showIcon message="诊断不可用" description={diagError} />
        ) : (
          <Descriptions size="small" column={1} bordered>
            <Descriptions.Item label="运行态">
              <Tag color={isRunningState(diag?.runtime?.state ?? active.state) ? "green" : (diag?.runtime?.state ?? active.state ?? "").toUpperCase() === "FAILED" ? "red" : "default"}>
                {diag?.runtime?.state ?? active.state ?? "—"}
              </Tag>
            </Descriptions.Item>
            <Descriptions.Item label="期望态">{diag?.desired_running ? "运行" : "停止"}</Descriptions.Item>
            <Descriptions.Item label="连接">{diag?.connection_state ?? "—"}</Descriptions.Item>
            <Descriptions.Item label="驱动版本">{diag?.driver_version ?? "—"}</Descriptions.Item>
            <Descriptions.Item label="描述状态">{diag?.descriptor_state ?? "—"}</Descriptions.Item>
            <Descriptions.Item label="重连累计">{diag?.reconnect_attempt_total ?? "—"}</Descriptions.Item>
            <Descriptions.Item label="上次连通">{fmtTime(diag?.last_connected_at_ns)}</Descriptions.Item>
            {diag?.runtime?.detail ? <Descriptions.Item label="详情">{diag.runtime.detail}</Descriptions.Item> : null}
          </Descriptions>
        )}
      </Card>

      <Card size="small" title="采集健康">
        <Descriptions size="small" column={1} bordered>
          <Descriptions.Item label="任务数">{taskCount ?? "—"}</Descriptions.Item>
          <Descriptions.Item label="配置版本">{diag?.runtime?.revision ?? "—"}</Descriptions.Item>
          <Descriptions.Item label="点位 GOOD/BAD/STALE">
            {good} / {bad} / {stale}
          </Descriptions.Item>
        </Descriptions>
        {bad > 0 || stale > 0 ? (
          <Table
            size="small"
            style={{ marginTop: 8 }}
            rowKey={(r: DevicePointView) => r.displayKey}
            pagination={false}
            dataSource={epPoints.filter((p) => p.derived !== "GOOD").slice(0, 10)}
            columns={[
              { title: "点位", dataIndex: "displayKey" },
              {
                title: "状态",
                dataIndex: "derived",
                render: (v: string) => <Tag color={v === "BAD" ? "red" : "orange"}>{v}</Tag>,
              },
            ]}
            locale={{ emptyText: "—" }}
          />
        ) : null}
      </Card>

      <Collapse size="small" items={[
        {
          key: "raw",
          label: "高级诊断（原始 JSON）",
          children: (
            <pre style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, background: "#f4f4f4", padding: 12, overflow: "auto" }}>
              {JSON.stringify(diag ?? {}, null, 2)}
            </pre>
          ),
        },
      ]} />
    </div>
  );
}
