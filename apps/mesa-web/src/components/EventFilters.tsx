// PR8 事件过滤栏：完整使用 PR7 过滤能力；category/kind/code/condition_id 均为精确匹配。
// 时间范围对应 received_at_ns（接收时间），明确不叫“发生时间”。
import { Button, Col, Input, InputNumber, Row, Select } from "antd";
import type { ActiveFilter, EventFilterForm } from "../events/filters";

/** ns 时间戳 → datetime-local 输入值（本地时区，精确到分钟）。 */
export function nsToLocalInput(ns: number | undefined): string {
  if (ns === undefined) return "";
  const d = new Date(ns / 1e6);
  if (Number.isNaN(d.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** datetime-local 输入值 → ns 时间戳；空/非法返回 undefined（即不过滤）。 */
export function localInputToNs(v: string): number | undefined {
  if (!v) return undefined;
  const t = new Date(v).getTime();
  return Number.isFinite(t) ? Math.floor(t * 1e6) : undefined;
}

export function EventFilters({
  value,
  endpoints,
  onChange,
  onReset,
}: {
  value: EventFilterForm;
  endpoints: string[];
  onChange: (next: EventFilterForm) => void;
  onReset: () => void;
}) {
  const set = (patch: Partial<EventFilterForm>) => onChange({ ...value, ...patch });
  return (
    <div style={{ display: "grid", gap: 8 }}>
      <Row gutter={8}>
        <Col span={6}>
          <Select
            allowClear
            placeholder="Endpoint（精确）"
            value={value.endpoint_id ?? undefined}
            onChange={(v) => set({ endpoint_id: v })}
            options={endpoints.map((e) => ({ value: e, label: e }))}
            style={{ width: "100%" }}
          />
        </Col>
        <Col span={6}>
          <Input
            allowClear
            placeholder="Category（精确匹配，如 alarm）"
            value={value.category ?? ""}
            onChange={(e) => set({ category: e.target.value })}
          />
        </Col>
        <Col span={6}>
          <Input
            allowClear
            placeholder="Kind（精确匹配，如 alarm.condition）"
            value={value.kind ?? ""}
            onChange={(e) => set({ kind: e.target.value })}
          />
        </Col>
        <Col span={6}>
          <InputNumber
            min={0}
            max={1000}
            placeholder="Severity ≥（0..1000）"
            value={value.severity_min}
            onChange={(v) => set({ severity_min: typeof v === "number" ? v : undefined })}
            style={{ width: "100%" }}
          />
        </Col>
      </Row>
      <Row gutter={8}>
        <Col span={6}>
          <Input
            allowClear
            placeholder="Code（精确匹配）"
            value={value.code ?? ""}
            onChange={(e) => set({ code: e.target.value })}
          />
        </Col>
        <Col span={6}>
          <Input
            allowClear
            placeholder="Condition ID（精确匹配）"
            value={value.condition_id ?? ""}
            onChange={(e) => set({ condition_id: e.target.value })}
          />
        </Col>
        <Col span={6}>
          <Select
            value={value.active}
            onChange={(v: ActiveFilter) => set({ active: v })}
            style={{ width: "100%" }}
            options={[
              { value: "all", label: "全部（Active 不过滤）" },
              { value: "active", label: "Active" },
              { value: "inactive", label: "Inactive" },
            ]}
          />
        </Col>
        <Col span={6} style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <Button onClick={onReset}>重置</Button>
        </Col>
      </Row>
      <Row gutter={8}>
        <Col span={6}>
          <Input
            type="datetime-local"
            value={nsToLocalInput(value.from_ns)}
            onChange={(e) => set({ from_ns: localInputToNs(e.target.value) })}
            placeholder="接收时间起"
            title="接收时间起（received_at_ns）"
            style={{ width: "100%" }}
          />
        </Col>
        <Col span={6}>
          <Input
            type="datetime-local"
            value={nsToLocalInput(value.to_ns)}
            onChange={(e) => set({ to_ns: localInputToNs(e.target.value) })}
            placeholder="接收时间止"
            title="接收时间止（received_at_ns）"
            style={{ width: "100%" }}
          />
        </Col>
      </Row>
      <div style={{ color: "#888", fontSize: 12 }}>
        时间过滤对应 received_at_ns（接收时间），非设备发生时间；后端按 seq DESC 分页，默认 100、最大 500。
      </div>
    </div>
  );
}
