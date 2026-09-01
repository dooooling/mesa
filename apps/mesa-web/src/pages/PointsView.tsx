import { useEffect, useState } from "react";

export function PointsView() {
  const [live, setLive] = useState<{ points: Array<{ endpoint_id: string; point_key: string; point_id: number; quality: string; value: { type: string; value: unknown }; timestamp_ns: number }> } | null>(null);

  useEffect(() => {
    let alive = true;
    const tick = async () => { try { const j = await fetch("/api/v1/points/latest").then((r) => r.json()); if (alive) setLive(j); } catch { /* ignore */ } };
    tick();
    const id = window.setInterval(tick, 1000);
    return () => { alive = false; window.clearInterval(id); };
  }, []);

  return (
    <div className="card">
      <div className="card-hd"><h3>数据</h3><span className="badge">GET /points/latest · 1s</span></div>
      <div className="card-bd" style={{ overflow: "auto" }}>
        <table className="table">
          <thead><tr><th>Endpoint</th><th>Point key</th><th>Id</th><th>Quality</th><th>Type</th><th>Value</th><th>Time</th></tr></thead>
          <tbody>
            {(live?.points ?? []).map((p) => (
              <tr key={`${p.endpoint_id}:${p.point_id}`}>
                <td className="mono" style={{ fontSize: 11 }}>{p.endpoint_id}</td>
                <td className="mono">{p.point_key}</td>
                <td className="mono">{p.point_id}</td>
                <td><span className={p.quality === "GOOD" ? "badge badge-ok" : p.quality === "BAD" ? "badge badge-bad" : "badge badge-warn"}>{p.quality}</span></td>
                <td className="mono">{p.value.type}</td>
                <td className="mono">{String(p.value.value)}</td>
                <td className="mono" style={{ fontSize: 11 }}>{new Date(Number(p.timestamp_ns) / 1e6).toLocaleTimeString()}</td>
              </tr>
            ))}
            {!live?.points?.length && <tr><td colSpan={7} className="help" style={{ textAlign: "center", padding: 16 }}>暂无数据</td></tr>}
          </tbody>
        </table>
      </div>
    </div>
  );
}
