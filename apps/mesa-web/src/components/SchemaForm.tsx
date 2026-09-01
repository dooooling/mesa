// 通用 SchemaForm：仅理解 FieldDescriptor，不含 driverId 分支
import { useMemo, useState } from "react";
import type { FieldDescriptor, SchemaDescriptor } from "../types";

function isVisible(field: FieldDescriptor, values: Record<string, unknown>) {
  const cond = field.ui.visible_if;
  if (!cond) return true;
  const cur = values[cond.field];
  if (cond.op === "eq") return cur === cond.value;
  if (cond.op === "neq") return cur !== cond.value;
  if (cond.op === "in") return Array.isArray(cond.value) && (cond.value as unknown[]).includes(cur);
  return true;
}

function FieldInput({ field, value, onChange, error }: { field: FieldDescriptor; value: unknown; onChange: (v: unknown) => void; error?: string; }) {
  const commonProps = {
    id: field.key,
    placeholder: field.ui.placeholder,
  } as const;

  if (field.field_type === "boolean") {
    return (
      <label style={{ display: "flex", gap: 10, alignItems: "center", padding: "10px 12px", border: "1px solid var(--border)", borderRadius: 10, background: "rgba(255,255,255,.04)" }}>
        <input type="checkbox" checked={Boolean(value ?? field.default ?? false)} onChange={(e) => onChange(e.target.checked)} />
        <span>
          <span className="label">{field.label} {field.required && <span style={{ color: "var(--accent)" }}>*</span>}</span>
          <span className="help" style={{ display: "block" }}>{field.description}</span>
        </span>
        {error && <span className="badge badge-bad" style={{ marginLeft: "auto" }}>{error}</span>}
      </label>
    );
  }
  if (field.field_type === "enum") {
    const opts = field.validation.enum_options ?? [];
    return (
      <div style={{ display: "grid", gap: 6 }}>
        <label className="label" htmlFor={field.key}>{field.label} {field.required && <span style={{ color: "var(--accent)" }}>*</span>} <span className="help">— {field.description ?? ""}</span></label>
        <select className="select" value={String(value ?? field.default ?? opts[0] ?? "")} onChange={(e) => onChange(e.target.value)} id={field.key}>
          {opts.map((o) => <option key={o} value={o}>{o}</option>)}
        </select>
        {error && <span className="badge badge-bad">{error}</span>}
      </div>
    );
  }
  if (field.field_type === "secret") {
    return (
      <div style={{ display: "grid", gap: 6 }}>
        <label className="label" htmlFor={field.key}>{field.label} {field.required && "*"}</label>
        <input className="input mono" type="password" value={String(value ?? "")} onChange={(e) => onChange(e.target.value)} id={field.key} placeholder={field.ui.placeholder} />
        {error && <span className="badge badge-bad">{error}</span>}
      </div>
    );
  }
  const inputType = field.field_type === "integer" || field.field_type === "port" || field.field_type === "number" || field.field_type === "duration" ? "number" : "text";
  return (
    <div style={{ display: "grid", gap: 6 }}>
      <label className="label" htmlFor={field.key}>{field.label} {field.required && <span style={{ color: "var(--accent)" }}>*</span>} <span className="help">— {field.description ?? ""}</span></label>
      <input
        className="input mono"
        type={inputType}
        value={String(value ?? field.default ?? "")}
        onChange={(e) => {
          const v = e.target.value;
          if (field.field_type === "integer" || field.field_type === "port") onChange(v === "" ? "" : Number(v));
          else if (field.field_type === "number" || field.field_type === "duration") onChange(v === "" ? "" : Number(v));
          else onChange(v);
        }}
        id={field.key} placeholder={field.ui.placeholder}
      />
      {error && <span className="badge badge-bad">{error}</span>}
    </div>
  );
}

export function SchemaForm({ schema, values, onChange, issues }: { schema: SchemaDescriptor; values: Record<string, unknown>; onChange: (next: Record<string, unknown>) => void; issues?: { path: string; message: string }[]; }) {
  const [local, setLocal] = useState(values);
  const issueMap = useMemo(() => { const m = new Map<string, string>(); for (const it of issues ?? []) m.set(it.path, it.message); return m; }, [issues]);
  const visible = schema.fields.filter((f) => isVisible(f, local));
  const sorted = [...visible].sort((a, b) => (a.ui.order ?? 999) - (b.ui.order ?? 999));

  // group 分区
  const groups = useMemo(() => {
    const map = new Map<string, FieldDescriptor[]>();
    for (const f of sorted) { const g = f.ui.group ?? "default"; if (!map.has(g)) map.set(g, []); map.get(g)!.push(f); }
    return [...map.entries()];
  }, [sorted]);

  return (
    <div style={{ display: "grid", gap: 16 }}>
      {groups.map(([g, fields]) => (
        <div key={g} className="card" style={{ background: "rgba(255,255,255,.03)" }}>
          <div className="card-hd"><h3 style={{ textTransform: "capitalize" }}>{g === "default" ? "参数" : g}</h3><span className="help">{fields.length} 字段</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 12, gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))" }}>
            {fields.map((f) => (
              <FieldInput
                key={f.key}
                field={f}
                value={local[f.key]}
                error={issueMap.get(`connection.${f.key}`)}
                onChange={(v) => { const next = { ...local, [f.key]: v }; setLocal(next); onChange(next); }}
              />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
