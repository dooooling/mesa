// M2 DeviceOverview：设备视角的运行与数据状态（非数据库详情）。
// 诚实语义：连接运行态与点位 GOOD/BAD/STALE 分开展示，不合成“健康评分”。
// “需要关注”只从真实信息派生：FAILED endpoint → BAD point → STALE point。
import { Button } from "antd";
import { useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { formatAge, formatPointValue, isRunningState } from "../deviceModel";
import type { DevicePointView, WorkspaceEndpoint } from "./useDeviceWorkspaceData";

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section>
      <h3 style={{ fontSize: 13, fontWeight: 600, margin: "0 0 8px" }}>{title}</h3>
      <div style={{ borderTop: "1px solid #e0e0e0", paddingTop: 8 }}>{children}</div>
    </section>
  );
}

function isFailedState(state?: string): boolean {
  return (state ?? "").toUpperCase() === "FAILED";
}

export interface AttentionItem {
  kind: "endpoint-failed" | "point-bad" | "point-stale";
  title: string;
  detail: string;
  endpointId: string;
  endpointName: string;
  pointKey?: string;
  sortTs: number;
}

/** 需要关注（M2 克制版）：FAILED endpoint → BAD → STALE，同类按时间倒序。 */
export function buildAttentionList(
  endpoints: WorkspaceEndpoint[],
  points: DevicePointView[],
): AttentionItem[] {
  const out: AttentionItem[] = [];
  for (const e of endpoints) {
    if (isFailedState(e.state)) {
      out.push({
        kind: "endpoint-failed",
        title: e.name ?? e.id,
        detail: `连接失败 · ${e.state}`,
        endpointId: e.id,
        endpointName: e.name ?? e.id,
        sortTs: Number.POSITIVE_INFINITY,
      });
    }
  }
  for (const p of points) {
    if (p.derived === "BAD") {
      out.push({
        kind: "point-bad",
        title: p.displayKey,
        detail: `BAD · ${formatAge(p.ageMs)}`,
        endpointId: p.endpoint_id,
        endpointName: p.endpointName,
        pointKey: p.displayKey,
        sortTs: typeof p.timestamp_ns === "number" ? p.timestamp_ns : 0,
      });
    }
  }
  for (const p of points) {
    if (p.derived === "STALE") {
      out.push({
        kind: "point-stale",
        title: p.displayKey,
        detail: `STALE · ${formatAge(p.ageMs)}未更新`,
        endpointId: p.endpoint_id,
        endpointName: p.endpointName,
        pointKey: p.displayKey,
        sortTs: typeof p.timestamp_ns === "number" ? p.timestamp_ns : 0,
      });
    }
  }
  const rank = { "endpoint-failed": 0, "point-bad": 1, "point-stale": 2 } as const;
  return out.sort((a, b) => rank[a.kind] - rank[b.kind] || b.sortTs - a.sortTs);
}

export function DeviceOverview(props: {
  deviceId: string;
  deviceName: string;
  endpoints: WorkspaceEndpoint[];
  endpointIds: string[];
  points: DevicePointView[];
  counts: { total: number; good: number; bad: number; stale: number };
  onOpenPoint: (p: DevicePointView) => void;
}) {
  const { deviceId, deviceName, endpoints, points, counts, onOpenPoint } = props;
  const nav = useNavigate();
  const attention = useMemo(() => buildAttentionList(endpoints, points), [endpoints, points]);
  const recent = useMemo(
    () =>
      [...points]
        .sort((a, b) => (b.timestamp_ns || 0) - (a.timestamp_ns || 0))
        .slice(0, 5),
    [points],
  );

  return (
    <div style={{ display: "grid", gap: 24 }}>
      <div style={{ fontSize: 12, color: "#525252" }}>
        {endpoints.length} 个连接 · {counts.total} 个数据点
      </div>

      <Section title="运行状态">
        {!endpoints.length ? (
          <div style={{ fontSize: 12, color: "#525252" }}>该设备暂无连接。</div>
        ) : (
          <div style={{ display: "grid", gap: 8 }}>
            {endpoints.map((e) => (
              <div key={e.id} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
                <span
                  style={{
                    width: 8,
                    height: 8,
                    borderRadius: "50%",
                    background: isFailedState(e.state) ? "#da1e28" : isRunningState(e.state) ? "#24a148" : "#8d8d8d",
                  }}
                />
                <span style={{ fontWeight: 600 }}>{e.name ?? e.id}</span>
                <span style={{ color: "#525252", fontSize: 12 }}>{e.driver_id}</span>
                <span style={{ color: "#525252", fontSize: 12 }}>{e.state ?? "—"}</span>
                <Button
                  size="small"
                  type="link"
                  onClick={() => nav(`/devices/${deviceId}/diagnostics?connection=${e.id}`)}
                >
                  诊断 →
                </Button>
              </div>
            ))}
          </div>
        )}
      </Section>

      <Section title="数据状态">
        <div style={{ display: "flex", gap: 24, fontSize: 13 }}>
          <span>GOOD <strong>{counts.good}</strong></span>
          <span>BAD <strong>{counts.bad}</strong></span>
          <span>STALE <strong>{counts.stale}</strong></span>
        </div>
      </Section>

      <Section title="需要关注">
        {!attention.length ? (
          <div style={{ fontSize: 12, color: "#525252" }}>暂无需要关注的问题</div>
        ) : (
          <div style={{ display: "grid", gap: 8 }}>
            {attention.slice(0, 5).map((a, i) => (
              <div key={`${a.kind}-${a.endpointId}-${a.pointKey ?? ""}-${i}`} style={{ fontSize: 13 }}>
                <span style={{ fontWeight: 600 }}>{a.title}</span>
                <span style={{ color: "#525252", marginLeft: 8 }}>{a.detail}</span>
                <span style={{ color: "#8d8d8d", marginLeft: 8, fontSize: 12 }}>{a.endpointName}</span>
              </div>
            ))}
          </div>
        )}
      </Section>

      <Section title="最近数据">
        {!recent.length ? (
          <div style={{ fontSize: 12, color: "#525252" }}>暂无数据</div>
        ) : (
          <div style={{ display: "grid", gap: 4 }}>
            {recent.map((p) => (
              <div
                key={`${p.endpoint_id}:${p.point_id}`}
                onClick={() => onOpenPoint(p)}
                style={{ display: "flex", gap: 12, fontSize: 13, cursor: "pointer", padding: "4px 0" }}
              >
                <span style={{ flex: 1 }}>{p.displayKey}</span>
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {formatPointValue(p.value)}
                </span>
                <span style={{ color: "#525252" }}>{p.derived}</span>
              </div>
            ))}
            <div>
              <Button size="small" type="link" onClick={() => nav(`/devices/${deviceId}/data`)}>
                查看全部数据 →
              </Button>
            </div>
          </div>
        )}
        <span style={{ display: "none" }}>{deviceName}</span>
      </Section>
    </div>
  );
}
