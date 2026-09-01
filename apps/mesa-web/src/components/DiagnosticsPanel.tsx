// 通用 DiagnosticsPanel（V2.1 §9）
import { useEffect, useState } from "react";
import { api } from "../api";

export function DiagnosticsPanel() {
  const [data, setData] = useState<Record<string, unknown> | null>(null);

  useEffect(() => { api.diagnostics().then((d: Record<string, unknown>) => setData(d)).catch(() => {}); }, []);

  const diag = data as unknown as { endpoints?: Array<{ id: string; driver_id: string; state?: string; points?: number; last_seen_ns?: number }> } | null;

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div className="grid grid-3">
        <div className="card stat"><div className="stat-label">端点</div><div className="stat-value">{diag?.endpoints?.length ?? "—"}</div><div className="stat-sub">/api/v1/endpoints</div></div>
        <div className="card stat"><div className="stat-label">采集</div><div className="stat-value" style={{ fontSize: 16 }}>DataPlane 有界</div><div className="stat-sub">Latest-Wins 背压</div></div>
        <div className="card stat"><div className="stat-label">控制</div><div className="stat-value" style={{ fontSize: 16 }}>Control 32 可靠</div><div className="stat-sub">禁合并 · 审计留痕</div></div>
      </div>

      <div className="card">
        <div className="card-hd"><h3>端点状态</h3><span className="kbd">GET /api/v1/diagnostics</span></div>
        <div className="card-bd" style={{ overflow: "auto" }}>
          <table className="table">
            <thead><tr><th>Endpoint</th><th>Driver</th><th>State</th><th>Points</th><th>Last seen</th></tr></thead>
            <tbody>
              {(diag?.endpoints ?? []).map((e) => (
                <tr key={e.id}>
                  <td className="mono">{e.id}</td><td className="mono">{e.driver_id}</td>
                  <td><span className={e.state === "running" ? "badge badge-ok" : "badge"}>{e.state ?? "—"}</span></td>
                  <td>{e.points ?? "—"}</td><td className="mono" style={{ fontSize: 11 }}>{e.last_seen_ns ?? "—"}</td>
                </tr>
              ))}
              {!diag?.endpoints?.length && <tr><td colSpan={5} className="help" style={{ padding: 16, textAlign: "center" }}>暂无端点 · 去「设备向导」创建</td></tr>}
            </tbody>
          </table>
        </div>
      </div>

      <div className="card">
        <div className="card-hd"><h3>原始 JSON</h3><span className="help">含 quality / audit / events</span></div>
        <div className="card-bd scroll" style={{ maxHeight: 380 }}>
          <pre className="mono" style={{ margin: 0, fontSize: 12, whiteSpace: "pre-wrap" }}>{data ? JSON.stringify(data, null, 2) : "加载中…"}</pre>
        </div>
      </div>
    </div>
  );
}
