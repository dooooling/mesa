// 通用 ResourcePicker：渲染 ResourceDescriptor.resources，不含协议分支
import { useState } from "react";
import type { ResourceDescriptor } from "../types";
import { SchemaForm } from "./SchemaForm";

export function ResourcePicker({ resources, onAdd }: { resources: ResourceDescriptor[]; onAdd: (selection: { resource_id: string; parameters: Record<string, unknown>; outputs: { output: string; point_key: string }[] }) => void; }) {
  const [selected, setSelected] = useState<string>(resources[0]?.id ?? "");
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [outputs, setOutputs] = useState<{ output: string; point_key: string }[]>([]);
  const res = resources.find((r) => r.id === selected);
  if (!res) return <div className="help">无可用资源</div>;

  return (
    <div className="grid" style={{ gap: 14 }}>
      <div style={{ display: "flex", gap: 10, flexWrap: "wrap", alignItems: "end" }}>
        <div style={{ minWidth: 320, flex: 1 }}>
          <div className="label" style={{ marginBottom: 6 }}>资源</div>
          <select className="select" value={selected} onChange={(e) => { setSelected(e.target.value); setParams({}); setOutputs([]); }}>
            {resources.map((r) => <option key={r.id} value={r.id}>{r.label["zh-CN"] ?? r.label.default} — {r.id}</option>)}
          </select>
          <div className="help" style={{ marginTop: 6 }}>{res.modes.join(" · ")} · {res.outputs.length} outputs</div>
        </div>
        <span className="badge mono">{res.id}</span>
      </div>

      {res.parameters.fields.length > 0 && (
        <SchemaForm schema={res.parameters} values={params} onChange={setParams} />
      )}

      <div className="card">
        <div className="card-hd"><h3>输出</h3><span className="badge">{outputs.length}/{res.outputs.length} 已选</span></div>
        <div className="card-bd" style={{ display: "grid", gap: 10 }}>
          {res.outputs.map((o) => {
            const checked = outputs.some((x) => x.output === o.id);
            return (
              <label key={o.id} style={{ display: "flex", gap: 10, alignItems: "center", padding: "10px 12px", border: "1px solid var(--border)", borderRadius: 10, background: checked ? "rgba(34,211,238,.10)" : "rgba(255,255,255,.03)" }}>
                <input type="checkbox" checked={checked} onChange={(e) => {
                  if (e.target.checked) setOutputs([...outputs, { output: o.id, point_key: `${res.id}.${o.id}` }]);
                  else setOutputs(outputs.filter((x) => x.output !== o.id));
                }} />
                <span style={{ flex: 1 }}>
                  <span className="label">{o.label["zh-CN"] ?? o.label.default}</span> <span className="kbd mono">{o.id}</span> <span className="help">[{o.data_type}{o.unit ? ` · ${o.unit}` : ""} · {o.access}]</span>
                </span>
                <span className="badge">{o.data_type}</span>
              </label>
            );
          })}
          {outputs.map((o) => (
            <div key={o.output} style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <span className="mono" style={{ fontSize: 12, color: "var(--muted)", minWidth: 90 }}>{o.output}</span>
              <input className="input mono" value={o.point_key} onChange={(e) => setOutputs(outputs.map((x) => x.output === o.output ? { ...x, point_key: e.target.value } : x))} placeholder="point_key" />
            </div>
          ))}
        </div>
      </div>

      <div style={{ display: "flex", justifyContent: "flex-end" }}>
        <button className="btn" disabled={outputs.length === 0} onClick={() => onAdd({ resource_id: res.id, parameters: params, outputs })}>加入选择</button>
      </div>
    </div>
  );
}
