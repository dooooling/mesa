// 通用 ResourceBrowser（V2.1 §20）：分页浏览，不含协议分支
import { useState } from "react";

export function ResourceBrowser({ endpointId }: { endpointId: string }) {
  const [parent, setParent] = useState("");
  const [filter, setFilter] = useState("");
  const [nodes, setNodes] = useState<{ id: string; label: string; has_children: boolean; binding_json: string }[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [next, setNext] = useState<string | null>(null);

  const load = async (cur: string | null) => {
    const r = await fetch(`/api/v1/endpoints/${endpointId}/browse`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ parent, filter, cursor: cur ?? "", limit: 20 }),
    });
    const j = await r.json();
    if (r.ok) {
      setNodes(j.nodes ?? []);
      setNext(j.next_cursor ?? null);
      setCursor(cur);
    } else {
      setNodes([]);
      setNext(null);
    }
  };

  return (
    <div style={{ border: "1px solid #ddd", padding: 12, margin: "12px 0" }}>
      <h4>浏览</h4>
      <div>
        parent: <input value={parent} onChange={(e) => setParent(e.target.value)} placeholder="ns=0;i=85" style={{ marginRight: 8 }} />
        filter: <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="过滤" style={{ marginRight: 8 }} />
        <button onClick={() => load(null)}>加载</button>
      </div>
      <ul>
        {nodes.map((n) => (
          <li key={n.id}>
            {n.label} ({n.id}) {n.has_children ? "[+]" : ""}{" "}
            <button onClick={() => setParent(n.id)}>进入</button>
          </li>
        ))}
      </ul>
      {next && <button onClick={() => load(next)}>下一页</button>}
      {cursor && <button onClick={() => load(null)}>重置</button>}
    </div>
  );
}
