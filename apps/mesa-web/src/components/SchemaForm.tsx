// 通用 SchemaForm：仅理解 FieldDescriptor，不含 driverId 分支（V2.1 §21.1）
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

function FieldInput({
  field,
  value,
  onChange,
  error,
}: {
  field: FieldDescriptor;
  value: unknown;
  onChange: (v: unknown) => void;
  error?: string;
}) {
  const common = {
    id: field.key,
    placeholder: field.ui.placeholder,
    value: (value ?? field.default ?? "") as string | number,
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>) => {
      const v = e.target.value;
      if (field.field_type === "integer" || field.field_type === "port") onChange(Number(v));
      else if (field.field_type === "number" || field.field_type === "duration") onChange(Number(v));
      else if (field.field_type === "boolean") onChange((e.target as HTMLInputElement).checked);
      else onChange(v);
    },
  } as const;

  if (field.field_type === "boolean") {
    return (
      <label style={{ display: "block", margin: "8px 0" }}>
        <input type="checkbox" checked={Boolean(value ?? field.default ?? false)} onChange={(e) => onChange(e.target.checked)} /> {field.label} {field.required && "*"}
        {error && <span style={{ color: "red", marginLeft: 8 }}>{error}</span>}
      </label>
    );
  }
  if (field.field_type === "enum") {
    const opts = field.validation.enum_options ?? [];
    return (
      <label style={{ display: "block", margin: "8px 0" }}>
        {field.label} {field.required && "*"}
        <select value={String(value ?? field.default ?? opts[0] ?? "")} onChange={common.onChange} style={{ marginLeft: 8 }}>
          {opts.map((o) => (
            <option key={o} value={o}>
              {o}
            </option>
          ))}
        </select>
        {error && <span style={{ color: "red", marginLeft: 8 }}>{error}</span>}
      </label>
    );
  }
  if (field.field_type === "secret") {
    return (
      <label style={{ display: "block", margin: "8px 0" }}>
        {field.label} {field.required && "*"}
        <input type="password" {...common} value={String(value ?? "")} />
        {error && <span style={{ color: "red", marginLeft: 8 }}>{error}</span>}
      </label>
    );
  }
  return (
    <label style={{ display: "block", margin: "8px 0" }}>
      {field.label} {field.required && "*"}
      <input {...common} style={{ marginLeft: 8 }} />
      {error && <span style={{ color: "red", marginLeft: 8 }}>{error}</span>}
    </label>
  );
}

export function SchemaForm({
  schema,
  values,
  onChange,
  issues,
}: {
  schema: SchemaDescriptor;
  values: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
  issues?: { path: string; message: string }[];
}) {
  const [local, setLocal] = useState(values);
  const issueMap = useMemo(() => {
    const m = new Map<string, string>();
    for (const it of issues ?? []) m.set(it.path, it.message);
    return m;
  }, [issues]);

  const visibleFields = schema.fields.filter((f) => isVisible(f, local));

  // 按 ui.order 与 group 简单排序
  const sorted = [...visibleFields].sort((a, b) => (a.ui.order ?? 999) - (b.ui.order ?? 999));

  return (
    <div>
      {sorted.map((f) => (
        <FieldInput
          key={f.key}
          field={f}
          value={local[f.key]}
          error={issueMap.get(`connection.${f.key}`)}
          onChange={(v) => {
            const next = { ...local, [f.key]: v };
            setLocal(next);
            onChange(next);
          }}
        />
      ))}
    </div>
  );
}
