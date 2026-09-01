import { useEffect, useState } from "react";
import { api } from "../api";
import { ResourceBrowser } from "../components/ResourceBrowser";

export function BrowsePage() {
  const [endpoints, setEndpoints] = useState<{ id: string; driver_id: string }[]>([]);
  const [selected, setSelected] = useState("");

  useEffect(() => { api.listEndpoints().then((j) => setEndpoints(j.endpoints ?? [])).catch(() => {}); }, []);

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div className="card">
        <div className="card-bd" style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
          <span className="label">端点</span>
          <select className="select" value={selected} onChange={(e) => setSelected(e.target.value)} style={{ minWidth: 300 }}>
            <option value="">选择端点（OPC UA 分页 50 + cursor）</option>
            {endpoints.map((e) => <option key={e.id} value={e.id}>{e.id} — {e.driver_id}</option>)}
          </select>
          <span className="help">通用 Browse：分页 / cursor / 导入（当前 import 返回 501 预留）</span>
        </div>
      </div>
      {selected ? <ResourceBrowser endpointId={selected} /> : <div className="card"><div className="card-bd help" style={{ textAlign: "center" }}>请选择端点后浏览</div></div>}
    </div>
  );
}
