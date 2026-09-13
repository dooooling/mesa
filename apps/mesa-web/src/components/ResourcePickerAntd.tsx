// PR26 Generic Resource UI + P0-2 Browse：纯 Descriptor 驱动的资源选型器。
// 不理解任何 Driver / Protocol：无 area/data_type fallback、无 toAddr、
// 无 FOCAS_ADDRS、无 driver-specific 分支。参数渲染复用 DescriptorFields
//（typed JSON 值 + visible_if + group/order），默认值经
// materializeSchemaDefaults 物化，保证“显示即保存值”。
//
// 选型模式完全取自 Descriptor.resource_selection_methods 声明：
// - manual：资源下拉 + 参数表单 + 输出勾选 + 加入（基线能力）
// - browse：endpoint 浏览树下钻，选用节点即回填手动表单（resource +
//   parameters 来自节点 binding，输出仍由用户勾选；映射规则见 browseModel）
// - import：仅当声明时展示禁用占位（后端 501，未实现不伪装入口）
import { useEffect, useState } from "react";
import { Button, Card, Checkbox, Input, Select, Tabs, Tag, message } from "antd";
import type { ResourceDescriptor } from "../types";
import { DescriptorFields, materializeSchemaDefaults } from "./DescriptorFields";
import { ResourceBrowseAntd } from "./ResourceBrowseAntd";
import type { BrowseSelection } from "../browseModel";

export interface ResourceSelectionInput {
  resource_id: string;
  parameters: Record<string, unknown>;
  outputs: Array<{ output: string; point_key: string }>;
}

/** 外部回填（Browse 选用）：resource + parameters，nonce 触发应用 */
export interface PickerExternalFill {
  resource_id: string;
  parameters: Record<string, unknown>;
  nonce: number;
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
  selectionMethods,
  endpointId,
}: {
  resources: ResourceDescriptor[];
  /** 已存在的 point_key（含当前已选）：用于自动命名去重，最终唯一性由 Core 门禁负责 */
  existingKeys: string[];
  /** 返回 true 表示父层已接收；成功后清空 outputs（保留 params），
   * 再次勾选同 output 自然生成 .2/.3，连续加入形成闭环 */
  onAdd: (sel: ResourceSelectionInput) => boolean;
  /** Descriptor.resource_selection_methods 声明；缺省即 manual（老驱动兼容） */
  selectionMethods?: Array<"manual" | "browse" | "import">;
  /** Browse 模式必需：浏览作用的 endpoint */
  endpointId?: string;
}) {
  const methods = selectionMethods && selectionMethods.length ? selectionMethods : ["manual"];
  const [mode, setMode] = useState<"manual" | "browse">(methods.includes("manual") ? "manual" : "browse");
  const [rid, setRid] = useState(resources[0]?.id ?? "");
  const [params, setParams] = useState<Record<string, unknown>>(() =>
    materializeSchemaDefaults(resources[0]?.parameters ?? { fields: [] }),
  );
  const [outputs, setOutputs] = useState<Array<{ output: string; point_key: string }>>([]);
  const [fill, setFill] = useState<PickerExternalFill | null>(null);

  // Browse 选用回填：resource 必须存在于 Descriptor（Core 门禁同样要求），
  // 参数以节点 binding 为准、缺失项用 schema 默认补齐显示（显示即保存）。
  // 消费后清零，避免 resources 引用变化时重复应用把用户拽回手动页。
  useEffect(() => {
    if (!fill) return;
    const target = resources.find((r) => r.id === fill.resource_id);
    setFill(null);
    if (!target) {
      message.warning(`浏览节点指向未知资源 ${fill.resource_id}，无法回填`);
      return;
    }
    setRid(target.id);
    setParams({ ...materializeSchemaDefaults(target.parameters), ...fill.parameters });
    setOutputs([]);
    setMode("manual");
  }, [fill, resources]);

  const res = resources.find((r) => r.id === rid);
  if (!res) return <div style={{ color: "#525252" }}>无可用资源</div>;

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

  const applyBrowseFill = (sel: BrowseSelection) => {
    setFill({ resource_id: sel.resource_id, parameters: sel.parameters, nonce: Date.now() });
  };

  const tabs = [
    ...(methods.includes("manual")
      ? [{ key: "manual", label: "手动", children: manualPane() }]
      : []),
    ...(methods.includes("browse")
      ? [{
        key: "browse",
        label: "浏览",
        disabled: !endpointId,
        children: endpointId ? (
          <ResourceBrowseAntd endpointId={endpointId} onFill={applyBrowseFill} />
        ) : (
          <div style={{ fontSize: 12, color: "#525252" }}>浏览需要已创建的 Endpoint。</div>
        ),
      }]
      : []),
    ...(methods.includes("import")
      ? [{ key: "import", label: "导入", disabled: true, children: <div style={{ fontSize: 12, color: "#525252" }}>Import 尚未实现（后端 501）。声明保留，待实现后开放。</div> }]
      : []),
  ];

  function manualPane() {
    // 闭包内重新收窄（外层 early-return 的 narrowing 进不了嵌套函数）
    if (!res) return null;
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
                prefix={<span style={{ fontSize: 11, color: "#525252" }}>{o.output}</span>}
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

  // 纯手动声明时保持老布局零变化；一旦声明 browse/import 即进入 tab 形态
  //（单 browse 声明也走 Tabs，避免回落到不存在的手动页）。
  if (!methods.includes("browse") && !methods.includes("import")) return manualPane();
  return <Tabs activeKey={mode} onChange={(k) => setMode(k as "manual" | "browse")} items={tabs} />;
}

function labelOf(label: unknown): string | undefined {
  if (typeof label === "string") return label;
  if (label && typeof label === "object") {
    const d = (label as { default?: unknown }).default;
    return typeof d === "string" ? d : undefined;
  }
  return undefined;
}
