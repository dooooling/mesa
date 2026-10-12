// PR8 受控描述字段：值纯 controlled（无内部值状态），只理解 FieldDescriptor，
// 不理解 Driver / Protocol / EventStream ID。PR9 OPC UA 事件过滤参数直接复用。
// 唯一内部状态是高级折叠显隐（纯展示，不触碰值）。
import { useState } from "react";
import { Button, Checkbox, Input, InputNumber, Select } from "antd";
import type { FieldDescriptor, SchemaDescriptor } from "../types";
import { materializeSchemaDefaults } from "../resourceSelectionModel";

/**
 * TSAP 文本编辑态：本地 draft（输入 0x010 不格式化打断），失焦/回车时
 * 合法值规范化为 number 提交；非法保留 draft 标红，由 required/Core 门裁决。
 * number 回显 0xXXXX。
 */
function TsapField({
  field,
  value,
  disabled,
  onChange,
  label,
}: {
  field: FieldDescriptor;
  value: unknown;
  disabled?: boolean;
  onChange: (v: unknown) => void;
  label: React.ReactNode;
}) {
  const toDisplay = (v: unknown): string => {
    if (typeof v === "number") return `0x${v.toString(16).toUpperCase().padStart(4, "0")}`;
    if (typeof v === "string") return v;
    return "";
  };
  const [draft, setDraft] = useState<string | null>(null);
  const shown = draft ?? toDisplay(value);
  const parsed = parseTsapInput(shown);
  const invalid = typeof parsed === "string" && shown.trim() !== "";
  const commit = (s: string) => {
    setDraft(null);
    const p = parseTsapInput(s);
    // 合法→number；非法→原字符串（Core 门拒绝，不静默吞掉）。
    onChange(typeof p === "number" ? p : s);
  };
  return (
    <div style={{ display: "grid", gap: 4 }}>
      <span style={{ fontSize: 12 }}>{label}</span>
      <Input
        disabled={disabled}
        value={shown}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onPressEnter={(e) => commit((e.target as HTMLInputElement).value)}
        placeholder={field.ui.placeholder ?? "如 0x0100 或 256"}
        status={invalid ? "error" : undefined}
        style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}
      />
      {invalid ? (
        <span style={{ fontSize: 12, color: "#da1e28" }}>TSAP 格式非法（十进制 0~65535 或十六进制 0x0000~0xFFFF）</span>
      ) : null}
    </div>
  );
}

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
 * P1-4：re-export（实现已迁移至 resourceSelectionModel 纯函数文件，
 * 避免 deviceModel 经本组件反向耦合 React/AntD；调用方逐步迁移 import 源）。
 */
export { materializeSchemaDefaults } from "../resourceSelectionModel";

/**
 * TSAP 双格式解析（NCK local_tsap/remote_tsap）：十进制（256）或十六进制
 * （0x0100/0X0100/0100h/0100H，允许前后空白）→ 同一 number；
 * 非法（空串原样返回由 required 门裁决；格式错/越界返回原字符串，前端标红，
 * 后端 from_json 亦拒绝）。Core schema 只见 number。
 */
export function parseTsapInput(s: string): number | string {
  const t = s.trim();
  if (t === "") return s;
  if (/^[0-9]+$/.test(t)) {
    const n = Number(t);
    return Number.isInteger(n) && n >= 0 && n <= 65535 ? n : s;
  }
  const hex =
    t.startsWith("0x") || t.startsWith("0X")
      ? t.slice(2)
      : /[hH]$/.test(t)
        ? t.slice(0, -1)
        : null;
  if (hex !== null && /^[0-9a-fA-F]{1,4}$/.test(hex)) {
    return parseInt(hex, 16);
  }
  return s;
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
      {field.description ? <span style={{ color: "#525252", fontWeight: 400 }}> — {field.description}</span> : null}
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
    // Secret 已设置 marker（如 {secret_set:true}）是“后端持有旧值”的凭证，
    // 不是明文：禁止把它当字符串喂给 <input>（否则显示成 [object Object]，
    // 用户一编辑还会把显示产物写回成新密码）。显示空值 + 保持不变占位；
    // state 里保留 marker 原样回传，后端按 marker-preserve 保留历史 Secret；
    // 用户键入新值后 marker 即被明文替换，后端更新 Secret。
    const secretSet =
      value !== null &&
      typeof value === "object" &&
      (value as { secret_set?: unknown }).secret_set === true;
    // 显式清除标记（后端 clear 语义：删除已存 Secret；缺失/留空只是保留旧值，
    // 没有删除入口——这就是清除按钮存在的理由）。
    const clearMarked =
      value !== null &&
      typeof value === "object" &&
      (value as { clear_secret?: unknown }).clear_secret === true;
    const displayValue = typeof value === "string" ? value : "";
    return (
      <div style={{ display: "grid", gap: 4 }}>
        <span style={{ fontSize: 12 }}>{label}</span>
        <Input.Password
          disabled={disabled || clearMarked}
          value={displayValue}
          onChange={(e) => onChange(e.target.value)}
          placeholder={secretSet ? "已设置，留空保持不变" : field.ui.placeholder}
        />
        {secretSet && !disabled ? (
          <Button size="small" danger onClick={() => onChange({ clear_secret: true })}>
            清除凭据
          </Button>
        ) : null}
        {clearMarked && !disabled ? (
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <span style={{ fontSize: 12, color: "#da1e28" }}>已标记清除，保存后生效</span>
            <Button size="small" onClick={() => onChange({ secret_set: true })}>
              撤销
            </Button>
          </div>
        ) : null}
      </div>
    );
  }
  // TSAP 双格式（NCK local_tsap/remote_tsap，Integer 契约保持）：
  // 文本编辑态（useState 本地 draft，失焦/回车才规范化提交，不打断输入
  // 如 0x010 的连续键入）；合法 hex/十进制→number，非法保留 draft 标红
  // 由 required/Core 门裁决；number 回显 0xXXXX。
  if (field.key === "local_tsap" || field.key === "remote_tsap") {
    return <TsapField field={field} value={value} disabled={disabled} onChange={onChange} label={label} />;
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
        style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}
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
  // 高级字段折叠（ui.advanced）：通用行为，非 NCK-specific。默认只渲染
  // 基础字段；高级参数收进折叠区，点开才渲染（渲染前不参与 visible 计算之外的任何逻辑）。
  const [showAdvanced, setShowAdvanced] = useState(false);
  const fields = (schema.fields ?? []).filter((f) => isVisible(f, value));
  const sorted = [...fields].sort((a, b) => (a.ui.order ?? 999) - (b.ui.order ?? 999));
  if (sorted.length === 0) return <div style={{ color: "#525252", fontSize: 12 }}>该流无参数</div>;
  const base = sorted.filter((f) => f.ui.advanced !== true);
  const advanced = sorted.filter((f) => f.ui.advanced === true);
  const renderGroups = (list: FieldDescriptor[]) => {
    const groups = new Map<string, FieldDescriptor[]>();
    for (const f of list) {
      const g = f.ui.group ?? "default";
      if (!groups.has(g)) groups.set(g, []);
      groups.get(g)!.push(f);
    }
    return [...groups.entries()].map(([g, fs]) => (
      <div key={g}>
        {g !== "default" ? <div style={{ fontSize: 12, color: "#525252", marginBottom: 6 }}>{g}</div> : null}
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
    ));
  };
  return (
    <div style={{ display: "grid", gap: 12 }}>
      {renderGroups(base)}
      {advanced.length > 0 ? (
        <div>
          <Button size="small" onClick={() => setShowAdvanced((v) => !v)}>
            {showAdvanced ? "▾ 收起高级" : `▸ 高级（${advanced.length}）`}
          </Button>
          {showAdvanced ? <div style={{ marginTop: 8 }}>{renderGroups(advanced)}</div> : null}
        </div>
      ) : null}
    </div>
  );
}
