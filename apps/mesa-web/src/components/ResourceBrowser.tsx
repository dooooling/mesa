// 通用 ResourceBrowser（V2.1 §20）：分页浏览，不含协议分支
import { useState } from "react";

export function ResourceBrowser({ endpointId }: { endpointId: string }) {
  const [parent, setParent] = useState("");
  const [filter, setFilter] = useState("");
  const [nodes, setNodes] = useState<{ id: string; label: string; has_children: boolean; binding_json: string }[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);

  const load = async (cur: string | null) => {
    const r = await fetch(`/api/v1/endpoints/${endpointId}/browse`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ parent, filter, cursor: cur ?? "", limit: 20 }),
    });
    const j = await r.json();
    if (r.ok) { setNodes(j.nodes ?? []); setNext(j.next_cursor ?? null); setCursor(cur); }
    else { setNodes([]); setNext(null); }
  };

  return (
    <div className="card">
      <div className="card-hd"><h3>浏览</h3><span className="mono help">{endpointId}</span></div>
      <div className="card-bd" style={{ display: "grid", gap: 12 }}>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <input className="input mono" value={parent} onChange={(e) => setParent(e.target.value)} placeholder="parent  e.g. ns=0;i=85" style={{ flex: 1, minWidth: 180 }} />
          <input className="input" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="过滤" style={{ width: 160 }} />
          <button className="btn btn-sm" onClick={() => load(null)}>加载</button>
        </div>
        <div style={{ display: "grid", gap: 8 }}>
          {nodes.map((n) => (
            <div key={n.id} style={{ display: "flex", gap: 10, alignItems: "center", padding: "10px 12px", border: "1px solid var(--border)", borderRadius: 10, background: "rgba(255,255,255,.03)" }}>
              <span className="mono" style={{ flex: 1, fontSize: 12 }}>{n.label} <span className="help">({n.id})</span></span>
              {n.has_children && <span className="badge">has_children</span>}
              <button className="btn btn-ghost btn-sm" onClick={() => setParent(n.id)}>进入</button>
            </div>
          ))}
          {!nodes.length && <div className="help" style={{ textAlign: "center", padding: 12 }}>点击「加载」浏览点位</div>}
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          {next && <button className="btn btn-ghost btn-sm" onClick={() => load(next)}>下一页</button>}
          {cursor && <button className="btn btn-ghost btn-sm" onClick={() => load(null)}>重置</button>}
        </div>
      </div>
    </div>
  );
}
