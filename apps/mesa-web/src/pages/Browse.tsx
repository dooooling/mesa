import { useEffect, useState } from "react";
import { api } from "../api";
import { ResourceBrowser } from "../components/ResourceBrowser";

export function BrowsePage() {
  const [endpoints, setEndpoints] = useState<{ id: string; driver_id: string }[]>([]);
  const [selected, setSelected] = useState("");

  useEffect(() => {
    api.listEndpoints().then((j) => setEndpoints(j.endpoints ?? [])).catch(() => {});
  }, []);

  return (
    <div style={{ padding: 16 }}>
      <h2>浏览/导入</h2>
      <label>
        端点
        <select value={selected} onChange={(e) => setSelected(e.target.value)} style={{ marginLeft: 8 }}>
          <option value="">选择端点</option>
          {endpoints.map((e) => (
            <option key={e.id} value={e.id}>
              {e.id} ({e.driver_id})
            </option>
          ))}
        </select>
      </label>
      {selected && <ResourceBrowser endpointId={selected} />}
    </div>
  );
}
