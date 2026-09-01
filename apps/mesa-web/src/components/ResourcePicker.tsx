// 通用 ResourcePicker：渲染 ResourceDescriptor.resources，不含协议分支
import { useState } from "react";
import type { ResourceDescriptor } from "../types";
import { SchemaForm } from "./SchemaForm";

export function ResourcePicker({
  resources,
  onAdd,
}: {
  resources: ResourceDescriptor[];
  onAdd: (selection: { resource_id: string; parameters: Record<string, unknown>; outputs: { output: string; point_key: string }[] }) => void;
}) {
  const [selected, setSelected] = useState<string>(resources[0]?.id ?? "");
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [outputs, setOutputs] = useState<{ output: string; point_key: string }[]>([]);

  const res = resources.find((r) => r.id === selected);
  if (!res) return <div>无可用资源</div>;

  return (
    <div style={{ border: "1px solid #ddd", padding: 12, margin: "12px 0" }}>
      <label>
        资源
        <select value={selected} onChange={(e) => { setSelected(e.target.value); setParams({}); setOutputs([]); }} style={{ marginLeft: 8 }}>
          {resources.map((r) => (
            <option key={r.id} value={r.id}>
              {r.label.default} ({r.id})
            </option>
          ))}
        </select>
      </label>

      {res.parameters.fields.length > 0 && (
        <div style={{ marginTop: 8 }}>
          <strong>参数</strong>
          <SchemaForm schema={res.parameters} values={params} onChange={setParams} />
        </div>
      )}

      <div style={{ marginTop: 8 }}>
        <strong>输出</strong>
        {res.outputs.map((o) => (
          <label key={o.id} style={{ display: "block", margin: "4px 0" }}>
            <input
              type="checkbox"
              checked={outputs.some((x) => x.output === o.id)}
              onChange={(e) => {
                if (e.target.checked) {
                  const key = `${res.id}.${o.id}`;
                  setOutputs([...outputs, { output: o.id, point_key: key }]);
                } else {
                  setOutputs(outputs.filter((x) => x.output !== o.id));
                }
              }}
            />{" "}
            {o.label.default} ({o.id}) [{o.data_type}]
          </label>
        ))}
        {outputs.map((o) => (
          <div key={o.output} style={{ marginLeft: 16 }}>
            point_key:{" "}
            <input value={o.point_key} onChange={(e) => setOutputs(outputs.map((x) => (x.output === o.output ? { ...x, point_key: e.target.value } : x)))} />
          </div>
        ))}
      </div>

      <button disabled={outputs.length === 0} onClick={() => onAdd({ resource_id: res.id, parameters: params, outputs })} style={{ marginTop: 8 }}>
        加入选择
      </button>
    </div>
  );
}
