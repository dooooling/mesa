import { useState } from "react";
import { Button, Card, Checkbox, Input, Select, Tag } from "antd";
import type { ResourceDescriptor } from "../types";

export function ResourcePickerAntd({ resources, onAdd }: { resources: ResourceDescriptor[]; onAdd: (sel: { resource_id: string; parameters: Record<string, unknown>; outputs: Array<{ output: string; point_key: string }> }) => void }) {
  const [rid, setRid] = useState(resources[0]?.id ?? "");
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [outputs, setOutputs] = useState<Array<{ output: string; point_key: string }>>([]);

  const res = resources.find((r) => r.id === rid);
  if (!res) return <div style={{ color: "#999" }}>无可用资源</div>;

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <div>
        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>资源</div>
        <Select style={{ width: "100%" }} value={rid} onChange={(v) => { setRid(v); setParams({}); setOutputs([]); }} options={resources.map((r) => ({ value: r.id, label: `${(r.label as unknown as { default: string })?.default ?? r.id} — ${r.id}` }))} />
      </div>

      {!!res.parameters.fields.length && (
        <Card size="small" title="参数">
          {res.parameters.fields.map((f) => {
            const isEnum = f.field_type === "enum";
            const opts = f.validation.enum_options?.length ? f.validation.enum_options : f.key === "area" ? ["DB","M","I","Q"] : f.key === "data_type" ? ["REAL","INT","WORD","DWORD","DINT","BOOL","BYTE","STRING","WSTRING"] : [];
            return (
              <div key={f.key} style={{ marginBottom: 8 }}>
                <div style={{ fontSize: 12 }}>{f.label}{f.required ? " *" : ""}</div>
                {isEnum ? (
                  <Select style={{ width: "100%" }} value={String(params[f.key] ?? f.default ?? opts[0] ?? "")} onChange={(v) => setParams({ ...params, [f.key]: v })} options={opts.map((o) => ({ value: o, label: o }))} />
                ) : (
                  <Input value={String(params[f.key] ?? f.default ?? "")} placeholder={f.ui.placeholder} onChange={(e) => setParams({ ...params, [f.key]: e.target.value })} />
                )}
              </div>
            );
          })}
        </Card>
      )}

      <Card size="small" title={`输出 · ${outputs.length}/${res.outputs.length}`}>
        <div style={{ display: "grid", gap: 8 }}>
          {res.outputs.map((o) => {
            const checked = outputs.some((x) => x.output === o.id);
            return (
              <label key={o.id} style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <Checkbox checked={checked} onChange={(e) => {
                  if (e.target.checked) setOutputs([...outputs, { output: o.id, point_key: `${res.id}.${o.id}` }]);
                  else setOutputs(outputs.filter((x) => x.output !== o.id));
                }} />
                <span style={{ flex: 1 }}>{(o.label as unknown as { default: string })?.default ?? o.id} <Tag>{o.data_type}</Tag></span>
              </label>
            );
          })}
          {outputs.map((o) => (
            <Input key={o.output} value={o.point_key} onChange={(e) => setOutputs(outputs.map((x) => x.output === o.output ? { ...x, point_key: e.target.value } : x))} prefix={<span style={{ fontSize: 11, color: "#999" }}>{o.output}</span>} />
          ))}
        </div>
      </Card>

      <Button type="primary" disabled={!outputs.length} onClick={() => onAdd({ resource_id: res.id, parameters: params, outputs })}>加入</Button>
    </div>
  );
}
