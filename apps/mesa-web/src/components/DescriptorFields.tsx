// PR8 受控描述字段：纯 controlled（无内部 local state），只理解 FieldDescriptor，
// 不理解 Driver / Protocol / EventStream ID。PR9 OPC UA 事件过滤参数直接复用。
import { Checkbox, Input, InputNumber, Select } from "antd";
import type { FieldDescriptor, SchemaDescriptor } from "../types";

function isVisible(field: FieldDescriptor, values: Record<string, unknown>): boolean {
  const cond = field.ui.visible_if;
  if (!cond) return true;
  const cur = values[cond.field];
  if (cond.op === "eq") return cur === cond.value;
  if (cond.op === "neq") return cur !== cond.value;
  if (cond.op === "in") return Array.isArray(cond.value) && (cond.value as unknown[]).includes(cur);
  return true;
}

/**
 * P1-4：将 schema 中带 `default` 的字段物化为参数初值。调用方在新建任务、
 * 切换流、加载服务端任务时统一使用，保证“UI 显示的 default 即实际保存值”，
 * required + default 字段不再出现显示有值却禁保存的不一致。
 */
export function materializeSchemaDefaults(schema: SchemaDescriptor): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of schema.fields ?? []) {
    if (f.default !== undefined) out[f.key] = f.default;
  }
  return out;
}

function FieldControl({
  field,
  value,
  disabled,
  onChange,
}: {
  field: FieldDescriptor;
  value: unknown;
  disabled?: boolean;
  onChange: (v: unknown) => void;
}) {
  const label = (
    <span>
      {field.label} {field.required && <span style={{ color: "#ff4d4f" }}>*</span>}
      {field.description ? <span style={{ color: "#888", fontWeight: 400 }}> — {field.description}</span> : null}
    </span>
  );
  if (field.field_type === "boolean") {
    // 三态：无真实值/default 时为 unset（indeterminate），不显示假 false；
    // 用户操作后才写入 boolean。required 无 default 的合法字段不得“看起来有值”。
    if (value === undefined && field.default === undefined) {
      return (
        <div style={{ display: "grid", gap: 4 }}>
          <span style={{ fontSize: 12 }}>{label}</span>
          <Select
            disabled={disabled}
            value={undefined}
            placeholder="未选择"
            onChange={(v) => onChange(v === "true")}
            options={[
              { value: "true", label: "true" },
              { value: "false", label: "false" },
            ]}
          />
        </div>
      );
    }
    return (
      <div>
        <Checkbox disabled={disabled} checked={Boolean(value ?? field.default ?? false)} onChange={(e) => onChange(e.target.checked)}>
          {label}
        </Checkbox>
      </div>
    );
  }
  if (field.field_type === "enum") {
    const opts = field.validation.enum_options ?? [];
    // 无真实值/default 时保持未选择（allowClear 可清回 unset）；禁止 opts[0]
    // 假默认值——Web 不替 Descriptor 发明语义，缺 required 由 Core 门禁裁决。
    return (
      <div style={{ display: "grid", gap: 4 }}>
        <span style={{ fontSize: 12 }}>{label}</span>
        <Select
          disabled={disabled}
          value={(value as string) ?? (field.default as string) ?? undefined}
          onChange={onChange}
          placeholder={field.ui.placeholder ?? "未选择"}
          allowClear={!field.required}
          options={opts.map((o) => ({ value: o, label: o }))}
        />
      </div>
    );
  }
  if (field.field_type === "secret") {
    return (
      <div style={{ display: "grid", gap: 4 }}>
        <span style={{ fontSize: 12 }}>{label}</span>
        <Input.Password disabled={disabled} value={(value as string) ?? ""} onChange={(e) => onChange(e.target.value)} placeholder={field.ui.placeholder} />
      </div>
    );
  }
  if (field.field_type === "integer" || field.field_type === "port") {
    const v = typeof value === "number" ? value : typeof field.default === "number" ? field.default : undefined;
    return (
      <div style={{ display: "grid", gap: 4 }}>
        <span style={{ fontSize: 12 }}>{label}</span>
        <InputNumber
          disabled={disabled}
          value={v}
          min={field.validation.min}
          max={field.validation.max}
          onChange={(n) => onChange(n)}
          placeholder={field.ui.placeholder}
          style={{ width: "100%" }}
        />
      </div>
    );
  }
  if (field.field_type === "number" || field.field_type === "duration") {
    const v = typeof value === "number" ? value : typeof field.default === "number" ? field.default : undefined;
    return (
      <div style={{ display: "grid", gap: 4 }}>
        <span style={{ fontSize: 12 }}>{label}</span>
        <InputNumber
          disabled={disabled}
          value={v}
          min={field.validation.min}
          max={field.validation.max}
          onChange={(n) => onChange(n)}
          placeholder={field.ui.placeholder}
          style={{ width: "100%" }}
        />
      </div>
    );
  }
  return (
    <div style={{ display: "grid", gap: 4 }}>
      <span style={{ fontSize: 12 }}>{label}</span>
      <Input
        disabled={disabled}
        value={String(value ?? field.default ?? "")}
        onChange={(e) => onChange(e.target.value)}
        placeholder={field.ui.placeholder}
        style={{ fontFamily: "monospace" }}
      />
    </div>
  );
}

export function DescriptorFields({
  schema,
  value,
  disabled,
  onChange,
}: {
  schema: SchemaDescriptor;
  value: Record<string, unknown>;
  disabled?: boolean;
  onChange: (next: Record<string, unknown>) => void;
}) {
  const fields = (schema.fields ?? []).filter((f) => isVisible(f, value));
  const sorted = [...fields].sort((a, b) => (a.ui.order ?? 999) - (b.ui.order ?? 999));
  if (sorted.length === 0) return <div style={{ color: "#888", fontSize: 12 }}>该流无参数</div>;
  const groups = new Map<string, FieldDescriptor[]>();
  for (const f of sorted) {
    const g = f.ui.group ?? "default";
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g)!.push(f);
  }
  return (
    <div style={{ display: "grid", gap: 12 }}>
      {[...groups.entries()].map(([g, fs]) => (
        <div key={g}>
          {g !== "default" ? <div style={{ fontSize: 12, color: "#888", marginBottom: 6 }}>{g}</div> : null}
          <div style={{ display: "grid", gap: 10, gridTemplateColumns: "repeat(auto-fit, minmax(240px, 1fr))" }}>
            {fs.map((f) => (
              <FieldControl
                key={f.key}
                field={f}
                value={value[f.key]}
                disabled={disabled}
                onChange={(v) => onChange({ ...value, [f.key]: v })}
              />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
