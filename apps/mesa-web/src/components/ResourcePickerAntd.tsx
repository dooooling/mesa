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
//
// 本文件不再包含任何 NCK/语义分支（PR #150 解耦项）：语义选型是驱动专属
// 形态，由调用方显式渲染 `components/NckSemanticPicker.tsx`，而不是在这里
// 通过 `semantic` 开关切走整棵子树。
import { useEffect, useState } from "react";
import { Button, Card, Checkbox, Input, Select, Tabs, Tag, message } from "antd";
import type { ResourceDescriptor } from "../types";
import { localizedText } from "../localizedText";
import { materializeSchemaDefaults, missingRequiredParams } from "../resourceSelectionModel";
import {
  allKnownPointKeys,
  outputOwnership,
  reconcileEditableSelection,
  type ResourceSelectionLike,
} from "../resourceSelectionModel";
import { DescriptorFields } from "./DescriptorFields";
import { locateFieldAnchor } from "./fieldFocus";
import { draftContentKey } from "../acquisitionDraftDiff";
import { UNSUPPORTED_CONFIG_NOTICE, unsupportedFields } from "../descriptorCompatibility";
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

/** point_key 自动命名：`resource.output`，冲突时 `.2/.3/...` 递增。
 * 依据全集（editable + protected）：用户手改成已存在 key 也会被上游拦截。 */
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
  existingSelections,
  protectedSelections,
  onAdd,
  selectionMethods,
  endpointId,
  requireExplicitSelection,
  submitLabel,
  onDirtyChange,
  initialSelection,
  focusFieldKey,
}: {
  resources: ResourceDescriptor[];
  /**
   * Selection 层 reconciliation 输入（冻结算法，唯一真值）：
   * - existingSelections：editable 全量（唯一允许 merge/append 的集合）；
   * - protectedSelections：preserved tasks 展开（永远只读，只参与
   *   反显/exact 检测/point_key 占用检测）。
   * knownKeys 完全由 selections 推导，不再接受外部 key 列表
   * （终审 #2：删除 existingKeys 双真值/旧 fallback）。
   */
  existingSelections: ResourceSelectionLike[];
  protectedSelections?: ResourceSelectionLike[];
  /** 返回 true 表示父层已接收；成功后清空 outputs（保留 params） */
  onAdd?: (sel: ResourceSelectionInput) => boolean;
  /** Descriptor.resource_selection_methods 声明；缺省即 manual（老驱动兼容） */
  selectionMethods?: Array<"manual" | "browse" | "import">;
  /** Browse 模式必需：浏览作用的 endpoint */
  endpointId?: string;
  /**
   * 「先选资源、再填参数」形态（抽屉添加流程，§4.1-4）：
   * 未选择资源时只渲染**一个**搜索/选择入口，不展示空表单、禁用按钮与错误框的组合。
   * 缺省 false ⇒ 行为逐字保持旧 Picker（既有调用方零变化）。
   */
  requireExplicitSelection?: boolean;
  /** 主按钮文案（抽屉里是「加入待保存列表」，不是「保存点位」）。 */
  submitLabel?: string;
  /** 抽屉内「有未确认修改」的上报（关闭前提醒一次）。 */
  onDirtyChange?: (dirty: boolean) => void;
  /**
   * **就地编辑**已有点位（通用资源）：给定时表单按这份选择预填（资源 + 参数 + 输出），
   * 主按钮执行「替换这一行」而不是「加入」。
   *
   * 为什么要有：服务端字段级校验（`issues[].path = ...parameters.<field>`）按
   * `issueLocation.ts` 的核实主要出现在**通用资源**路径上；只有能就地编辑，用户才能
   * 在那条字段上直接改完再保存。
   */
  initialSelection?: ResourceSelectionInput | null;
  /**
   * 外部要求定位的字段 key（字段级 issues）：与 NCK 原生表单同一套动作
   * （`fieldFocus.ts`：展开高级区 → 高亮 → 有限次重试定位）。
   */
  focusFieldKey?: string | null;
}) {
  const methods: Array<"manual" | "browse" | "import"> =
    selectionMethods && selectionMethods.length ? selectionMethods : ["manual"];
  const [mode, setMode] = useState<"manual" | "browse">(methods.includes("manual") ? "manual" : "browse");
  // 就地编辑：预填「资源 + 参数 + 输出」，且不再要求用户先选资源。
  const editing = initialSelection ?? null;
  const initialResource = editing
    ? resources.find((r) => r.id === editing.resource_id)
    : requireExplicitSelection
      ? undefined
      : resources[0];
  const [rid, setRid] = useState(initialResource?.id ?? "");
  const [params, setParams] = useState<Record<string, unknown>>(() =>
    editing
      ? { ...materializeSchemaDefaults(initialResource?.parameters ?? { fields: [] }), ...editing.parameters }
      : initialResource
        ? materializeSchemaDefaults(initialResource.parameters)
        : {},
  );
  const [outputs, setOutputs] = useState<Array<{ output: string; point_key: string }>>(() =>
    editing ? editing.outputs.map((o) => ({ ...o })) : [],
  );
  const [fill, setFill] = useState<PickerExternalFill | null>(null);
  // 高级折叠：受控（字段级定位可能需要**先展开**再滚动，见下方 focus effect）。
  const [advancedOpen, setAdvancedOpen] = useState(false);

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

  // 抽屉「有未确认修改」的判定：选了资源、勾了输出即算（不引入新业务判据）。
  // 就地编辑时改为「与预填值比是否真的改过」：打开表单本身不算修改，否则关抽屉
  // 会无端弹「放弃未确认的修改？」。
  const dirty = editing
    ? draftContentKey({ resource_id: rid, parameters: params, outputs }) !==
      draftContentKey({
        resource_id: editing.resource_id,
        parameters: editing.parameters,
        outputs: editing.outputs,
      })
    : !!rid || outputs.length > 0;
  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  // 字段级定位（服务端 `issues[].path` 指到 `...parameters.<field>`）：
  // 与 NCK 原生表单同一套动作——高级字段在折叠态**不渲染**，必须先展开，
  // 再在下一轮渲染滚动到锚点并高亮（`DescriptorFields` 的 highlightKeys）。
  // 只认当前资源 Schema 里真实存在的字段；不存在就什么都不做（不猜、不硬塞高亮）。
  const focusField = focusFieldKey
    ? (resources.find((r) => r.id === rid)?.parameters.fields ?? []).find((f) => f.key === focusFieldKey)
    : undefined;
  const focusFieldName = focusField?.key ?? null;
  const focusFieldIsAdvanced = focusField?.ui?.advanced === true;
  useEffect(() => {
    if (!focusFieldName) return;
    if (focusFieldIsAdvanced && !advancedOpen) {
      setAdvancedOpen(true);
      return;
    }
    return locateFieldAnchor(`selection-field-${focusFieldName}`);
  }, [focusFieldName, focusFieldIsAdvanced, advancedOpen]);

  /** 切换资源：参数按新 Schema 物化默认值，输出选择清空（显示即保存值）。 */
  const switchResource = (v: string) => {
    setRid(v);
    const next = resources.find((r) => r.id === v);
    setParams(materializeSchemaDefaults(next?.parameters ?? { fields: [] }));
    setOutputs([]);
  };

  const res = resources.find((r) => r.id === rid);
  if (!resources.length) return <div style={{ color: "#525252" }}>无可用资源</div>;
  if (!res) {
    // 还没选资源：只给**一个**搜索/选择入口（§4.1-4）。参数表、输出表与主按钮
    // 都要等选中之后才出现，避免「空表单 + 禁用按钮 + 错误框」的组合。
    return (
      <div style={{ display: "grid", gap: 8 }} data-testid="resource-picker-idle">
        <div style={{ fontSize: 12, fontWeight: 600 }}>资源</div>
        <Select
          data-testid="resource-select"
          aria-label="搜索并选择资源"
          showSearch
          optionFilterProp="label"
          placeholder="搜索并选择资源"
          style={{ width: "100%" }}
          value={undefined}
          onChange={switchResource}
          options={resources.map((r) => ({ value: r.id, label: `${labelOf(r.label) ?? r.id} — ${r.id}` }))}
        />
      </div>
    );
  }

  // A3：未知字段类型 ⇒ 该资源参数**不可编辑、不可加入**。
  // 判定基于完整 Schema（含高级/隐藏字段），打开界面即生效；只影响当前资源，
  // 不阻止切换资源或使用其它连接。
  const unsupported = unsupportedFields(res.parameters);

  // required 前置（表单完整性预检，唯一允许的前端 required 判断）：
  // 缺席的必填字段 key 列表（`0`/`false` 视为在场，不得 truthy 判）；
  // 类型/enum/range/pattern 一律不判，交给 Core 唯一真值。
  const missingRequired = missingRequiredParams(
    { fields: res.parameters.fields ?? [] },
    params,
  );

  // Selection 层输入：reconciliation 用全对象（唯一真值）。
  const editableSels: ResourceSelectionLike[] = existingSelections;
  const protectedSels: ResourceSelectionLike[] = protectedSelections ?? [];
  const schemaOf = (resourceId: string) => {
    const found = resources.find((r) => r.id === resourceId);
    return { fields: (found?.parameters.fields ?? []).map((f) => ({ key: f.key, default: f.default })) };
  };
  // 全集 point_key（editable + protected）：自动命名与冲突提示的唯一依据，
  // 完全由 selections 推导（终审 #2：无外部 key 列表）。
  const knownKeys = allKnownPointKeys(editableSels, protectedSels);
  // 当前表单归属反显（同 resource+params 的已选 outputs 呈 disabled）。
  const ownership = outputOwnership(
    res.id,
    params,
    res.outputs.map((o) => o.id),
    editableSels,
    protectedSels,
    schemaOf,
  );

  const toggleOutput = (outputId: string, checked: boolean) => {
    if (checked) {
      const taken = new Set([...knownKeys, ...outputs.map((o) => o.point_key)]);
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
    // §4.1.1：导入必须**同时**满足「驱动声明」与「产品实现可用」；后端导入接口当前
    // 恒返回 501，因此这里**不渲染**该入口（不是置灰占位，也不因声明而放开）。
  ];

  function manualPane() {
    // 闭包内重新收窄（外层 early-return 的 narrowing 进不了嵌套函数）
    if (!res) return null;
    return (
      <div style={{ display: "grid", gap: 12 }}>
        <div>
          <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 6 }}>资源</div>
          <Select
            data-testid="resource-select"
            aria-label="搜索并选择资源"
            showSearch
            optionFilterProp="label"
            placeholder="搜索并选择资源"
            style={{ width: "100%" }}
            value={rid}
            onChange={switchResource}
            options={resources.map((r) => ({ value: r.id, label: `${labelOf(r.label) ?? r.id} — ${r.id}` }))}
          />
        </div>

        {!!res.parameters.fields.length && (
          <Card size="small" title="参数">
            <DescriptorFields
              schema={res.parameters}
              value={params}
              onChange={setParams}
              // 字段锚点前缀 + 高亮集合：字段级问题定位要能滚动到这里（`fieldFocus.ts`）。
              idPrefix="selection-field"
              advancedOpen={advancedOpen}
              onAdvancedOpenChange={setAdvancedOpen}
              highlightKeys={focusFieldName ? [focusFieldName] : []}
            />
          </Card>
        )}

        <Card size="small" title={`输出 · ${outputs.length}/${res.outputs.length}`}>
          <div style={{ display: "grid", gap: 8 }}>
            {res.outputs.map((o) => {
              const inForm = outputs.some((x) => x.output === o.id);
              // 反显：exact 签名已在 editable/protected 即 disabled（只读提示，
              // 不在此编辑；新增走 merge/拒绝路径，由加入时裁决）。
              const owned = ownership.get(o.id);
              const takenElsewhere = owned !== undefined && owned.state !== "free";
              return (
                <label key={o.id} style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <Checkbox
                    aria-label={`output-${o.id}`}
                    checked={inForm || takenElsewhere}
                    disabled={takenElsewhere && !inForm}
                    onChange={(e) => toggleOutput(o.id, e.target.checked)}
                  />
                  <span style={{ flex: 1 }}>
                    {labelOf(o.label) ?? o.id}{" "}
                    <Tag>{o.type_spec.kind === "fixed" ? o.type_spec.data_type : o.type_spec.kind === "from_parameter" ? `←${o.type_spec.parameter}` : "driver"}</Tag>
                    {o.access !== "read" ? <Tag color="orange">{o.access}</Tag> : null}
                    {owned?.state === "editable" ? <Tag color="blue">已采集</Tag> : null}
                    {owned?.state === "protected" ? <Tag color="purple">其他任务已采集</Tag> : null}
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
                status={knownKeys.has(o.point_key) ? "error" : undefined}
              />
            ))}
            {outputs.some((o) => knownKeys.has(o.point_key)) ? (
              <div style={{ fontSize: 12, color: "#da1e28" }}>
                point_key 已被其他采集项使用，加入将被拒绝（请改名）。
              </div>
            ) : null}
          </div>
        </Card>

        {missingRequired.length > 0 ? (
          <div data-testid="missing-required" style={{ fontSize: 12, color: "#da1e28" }}>
            缺少必填参数：{missingRequired.join(", ")}（补齐后可加入；类型/范围等由后端校验）
          </div>
        ) : null}

        <Button
          type="primary"
          disabled={!outputs.length || missingRequired.length > 0 || unsupported.length > 0}
          onClick={() => {
            // A3 提交门（与 disabled 同源，防程序化绕过）：未知字段类型的配置
            // 既不能编辑也不能提交，绝不发出一个界面没看懂的选择。
            if (unsupported.length > 0) {
              message.error(UNSUPPORTED_CONFIG_NOTICE);
              return;
            }
            // required 前置防御（即使绕过 disabled，onClick 再判一次）：
            // 缺席即拒绝，不发 onAdd；类型/enum/range/pattern 不判，交 Core。
            const missing = missingRequiredParams(
              { fields: res.parameters.fields ?? [] },
              params,
            );
            if (missing.length > 0) {
              message.error(`缺少必填参数：${missing.join(", ")}`);
              return;
            }
            // 冻结算法前置裁决（只读 protected，只改 editable；protected 永不动）。
            // Picker 只做即时 UX 预判，最终防御门在父层 onAdd（同一纯函数）。
            const decision = reconcileEditableSelection({
              candidate: { resource_id: res.id, parameters: params, outputs },
              editable: editableSels,
              protectedSelections: protectedSels,
              schemaOf,
            });
            if (decision.kind === "duplicate-editable") {
              message.warning(`已在采集中：${decision.outputs.join(", ")}，不会重复加入`);
              return;
            }
            if (decision.kind === "duplicate-protected") {
              message.warning(`已在其他任务采集：${decision.outputs.join(", ")}，不会重复加入`);
              return;
            }
            if (decision.kind === "point-key-conflict") {
              message.error(`point_key 已被其他采集项使用：${decision.pointKeys.join(", ")}（请改名）`);
              return;
            }
            // merge/append 的实际数组变更由父层 onAdd 执行（父层持有 sels）。
            if (onAdd?.({ resource_id: res.id, parameters: params, outputs })) setOutputs([]);
          }}
        >
          {submitLabel ?? "加入"}
        </Button>
      </div>
    );
  }

  // 纯手动声明时保持老布局零变化；一旦声明 browse 即进入 tab 形态
  //（单 browse 声明也走 Tabs，避免回落到不存在的手动页）。
  // 未实现的 import 不进入 tabs（见上方注释），因此不再作为分支条件。
  if (!methods.includes("browse")) return manualPane();
  return <Tabs activeKey={mode} onChange={(k) => setMode(k as "manual" | "browse")} items={tabs} />;
}

// 资源 / 输出标签的唯一取值入口（A1）：统一走 localizedText（优先 zh-CN，
// 回落 default）。返回空串即视为「没有标签」，由调用方退回 id。
function labelOf(label: ResourceDescriptor["label"] | null | undefined): string | undefined {
  const text = localizedText(label);
  return text === "" ? undefined : text;
}
