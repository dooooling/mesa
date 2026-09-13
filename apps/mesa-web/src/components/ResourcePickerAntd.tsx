// PR26 Generic Resource UI：纯 Descriptor 驱动的资源选型器。
// 不理解任何 Driver / Protocol：无 area/data_type fallback、无 toAddr、
// 无 FOCAS_ADDRS、无 driver-specific 分支。参数渲染复用 DescriptorFields
//（typed JSON 值 + visible_if + group/order），默认值经
// materializeSchemaDefaults 物化，保证“显示即保存值”。
import { useState } from "react";
import { Button, Card, Checkbox, Input, Select, Tag } from "antd";
import type { ResourceDescriptor } from "../types";
import { DescriptorFields, materializeSchemaDefaults } from "./DescriptorFields";

export interface ResourceSelectionInput {
  resource_id: string;
  parameters: Record<string, unknown>;
  outputs: Array<{ output: string; point_key: string }>;
}

/** point_key 自动命名：`resource.output`，冲突时 `.2/.3/...` 递增。 */
export function suggestPointKey(
  resourceId: string,
  outputId: string,
  taken: Set<string>,
): string {
  const base = `${resourceId}.${outputId}`;
  if (!taken.has(base)) return base;
  let n = 2;
  while (taken.has(`${base}.${n}`)) n += 1;
  return `${base}.${n}`;
}

export function ResourcePickerAntd({
  resources,
  existingKeys,
  onAdd,
}: {
  resources: ResourceDescriptor[];
  /** 已存在的 point_key（含当前已选）：用于自动命名去重，最终唯一性由 Core 门禁负责 */
  existingKeys: string[];
  /** 返回 true 表示父层已接收；成功后清空 outputs（保留 params），
   * 再次勾选同 output 自然生成 .2/.3，连续加入形成闭环 */
  onAdd: (sel: ResourceSelectionInput) => boolean;
}) {
  const [rid, setRid] = useState(resources[0]?.id ?? "");
  const [params, setParams] = useState<Record<string, unknown>>(() =>
    materializeSchemaDefaults(resources[0]?.parameters ?? { fields: [] }),
  );
  const [outputs, setOutputs] = useState<Array<{ output: string; point_key: string }>>([]);

  const res = resources.find((r) => r.id === rid);
  if (!res) return <div style={{ color: "#999" }}>无可用资源</div>;

  const switchResource = (v: string) => {
    setRid(v);
    const next = resources.find((r) => r.id === v);
    setParams(materializeSchemaDefaults(next?.parameters ?? { fields: [] }));
    setOutputs([]);
  };

  const toggleOutput = (outputId: string, checked: boolean) => {
    if (checked) {
      const taken = new Set([...existingKeys, ...outputs.map((o) => o.point_key)]);
      setOutputs([...outputs, { output: outputId, point_key: suggestPointKey(res.id, outputId, taken) }]);
    } else {
      setOutputs(outputs.filter((x) => x.output !== outputId));
    }
  };

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <div>
        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>资源</div>
        <Select
          style={{ width: "100%" }}
          value={rid}
          onChange={switchResource}
          options={resources.map((r) => ({ value: r.id, label: `${labelOf(r.label) ?? r.id} — ${r.id}` }))}
        />
      </div>

      {!!res.parameters.fields.length && (
        <Card size="small" title="参数">
          <DescriptorFields schema={res.parameters} value={params} onChange={setParams} />
        </Card>
      )}

      <Card size="small" title={`输出 · ${outputs.length}/${res.outputs.length}`}>
        <div style={{ display: "grid", gap: 8 }}>
          {res.outputs.map((o) => {
            const checked = outputs.some((x) => x.output === o.id);
            return (
              <label key={o.id} style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <Checkbox
                  aria-label={`output-${o.id}`}
                  checked={checked}
                  onChange={(e) => toggleOutput(o.id, e.target.checked)}
                />
                <span style={{ flex: 1 }}>
                  {labelOf(o.label) ?? o.id}{" "}
                  <Tag>{o.type_spec.kind === "fixed" ? o.type_spec.data_type : o.type_spec.kind === "from_parameter" ? `←${o.type_spec.parameter}` : "driver"}</Tag>
                  {o.access !== "read" ? <Tag color="orange">{o.access}</Tag> : null}
                </span>
              </label>
            );
          })}
          {outputs.map((o) => (
            <Input
              key={o.output}
              value={o.point_key}
              onChange={(e) => setOutputs(outputs.map((x) => (x.output === o.output ? { ...x, point_key: e.target.value } : x)))}
              prefix={<span style={{ fontSize: 11, color: "#999" }}>{o.output}</span>}
            />
          ))}
        </div>
      </Card>

      <Button
        type="primary"
        disabled={!outputs.length}
        onClick={() => {
          if (onAdd({ resource_id: res.id, parameters: params, outputs })) setOutputs([]);
        }}
      >
        加入
      </Button>
    </div>
  );
}

function labelOf(label: unknown): string | undefined {
  if (typeof label === "string") return label;
  if (label && typeof label === "object") {
    const d = (label as { default?: unknown }).default;
    return typeof d === "string" ? d : undefined;
  }
  return undefined;
}
