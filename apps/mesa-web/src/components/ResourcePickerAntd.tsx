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
// PR-C 第三批：当上层提供 `semantic` 配置（仅 NCK Endpoint 页/未来 PR-D 向导）
// 时，组件进入**语义模式**：
// - 不再渲染旧 `variable` 手动/原生字段入口（A22：该路径在 NCK 上必被驱动拒绝）；
// - 默认页是 NckPointFields（服务端 Preview 唯一权威，A21）；
// - Browse 语义叶经 nckSemantic 准入（scope/family/identity）后才可选用（A13/A16）。
// 未提供 `semantic` 时，行为逐字保持旧通用 Picker（非 NCK 零变化）。
import { useEffect, useState } from "react";
import { Alert, Button, Card, Checkbox, Input, Select, Tabs, Tag, message } from "antd";
import type { ResourceDescriptor, SchemaDescriptor } from "../types";
import { localizedText } from "../localizedText";
import { materializeSchemaDefaults, missingRequiredParams } from "../resourceSelectionModel";
import {
  allKnownPointKeys,
  outputOwnership,
  reconcileEditableSelection,
  type ResourceSelectionLike,
} from "../resourceSelectionModel";
import { DescriptorFields } from "./DescriptorFields";
import { UNSUPPORTED_CONFIG_NOTICE, unsupportedFields } from "../descriptorCompatibility";
import {
  ResourceBrowseAntd,
  type NckSemanticBrowseConfig,
} from "./ResourceBrowseAntd";
import { NckPointFields, type NckPreviewFn } from "./NckPointFields";
import type { BrowseSelection } from "../browseModel";
import type { NckPointSelection } from "../deviceModel";
import type { NckDefinitionLike } from "../nckPointModel";

/**
 * 语义模式配置（仅 NCK 调用方注入）。
 *
 * `definitions` 是语义表单的唯一元数据来源；缺失即 fail-closed，不退回旧
 * `variable` 原生字段路径。
 */
export interface PickerSemanticConfig extends NckSemanticBrowseConfig {
  driverId: string;
  definitions?: readonly NckDefinitionLike[] | null;
  /** Descriptor variable 资源 Schema；提供时才开放高级原生页。 */
  nativeSchema?: SchemaDescriptor | null;
  takenPointKeys?: string[];
  /** 语义点位加入回调；未提供时语义表单只读提示，不产生可提交点位。 */
  onAddPoint?: (point: NckPointSelection) => boolean;
  /** 测试注入 Preview；生产不传，走服务端 identity-preview。 */
  preview?: NckPreviewFn;
}


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
  semantic,
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
  /** NCK 语义模式（可选；未提供时行为逐字保持旧 Picker）。 */
  semantic?: PickerSemanticConfig | null;
}) {
  const methods: Array<"manual" | "browse" | "import"> =
    selectionMethods && selectionMethods.length ? selectionMethods : ["manual"];
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

  // 语义模式：不进入通用 resource/manual 分支，旧 `variable` 入口整体不渲染（A22）。
  if (semantic) {
    return (
      <SemanticResourcePicker
        methods={methods}
        endpointId={endpointId}
        semantic={semantic}
      />
    );
  }

  const res = resources.find((r) => r.id === rid);
  if (!res) return <div style={{ color: "#525252" }}>无可用资源</div>;

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

  const switchResource = (v: string) => {
    setRid(v);
    const next = resources.find((r) => r.id === v);
    setParams(materializeSchemaDefaults(next?.parameters ?? { fields: [] }));
    setOutputs([]);
  };

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

/**
 * NCK 语义 Picker（PR-C 第三批）。
 *
 * 为什么单独一个组件而不是在旧 manualPane 里加 if：
 * 旧 pane 的身份真值来自 `suggestPointKey(resource.output)` 与自由文本，
 * 与 NCK 的 `{kind,point_key,parameters}` 完全不是同一模型；把两者揉进一个
 * 表单会让“语义点位”再次出现第二套 Key 生成逻辑。这里直接复用
 * `NckPointFields`（服务端 Preview 唯一权威），旧 manual 入口在语义模式
 * 下**整体不渲染**（A22：不提供明知会失败的保存操作）。
 */
function SemanticResourcePicker({
  methods,
  endpointId,
  semantic,
}: {
  methods: Array<"manual" | "browse" | "import">;
  endpointId?: string;
  semantic: PickerSemanticConfig;
}) {
  const [mode, setMode] = useState<"semantic" | "browse">("semantic");
  const [preset, setPreset] = useState<NckPointSelection | null>(null);
  const [nonce, setNonce] = useState(0);
  const definitions = semantic.definitions ? [...semantic.definitions] : null;

  const semanticPane = !definitions ? (
    <div data-testid="nck-semantic-picker-no-definitions-body" style={{ fontSize: 12, color: "#525252" }}>
      定义目录不可用：不会渲染语义表单，也不会退回旧 variable 原生字段流程。
    </div>
  ) : semantic.onAddPoint ? (
    <NckPointFields
      key={`picker-semantic-${nonce}-${String(preset?.parameters.variable_id ?? "")}`}
      driverId={semantic.driverId}
      endpointFamily={semantic.endpointFamily}
      catalogFamily={semantic.catalogFamily}
      definitions={definitions ?? []}
      takenPointKeys={semantic.takenPointKeys ?? []}
      initialPoint={preset}
      nativeSchema={semantic.nativeSchema ?? null}
      preview={semantic.preview}
      onSubmit={(point) => {
        const accepted = semantic.onAddPoint?.(point) ?? false;
        if (accepted) {
          setPreset(null);
          setNonce((n) => n + 1);
        }
      }}
    />
  ) : (
    <div style={{ fontSize: 12, color: "#c62828" }}>
      上层未提供语义点位接收入口：禁止产生可提交点位。
    </div>
  );

  const browsePane = !endpointId ? (
    <div style={{ fontSize: 12, color: "#525252" }}>浏览需要已创建的 Endpoint。</div>
  ) : (
    <ResourceBrowseAntd
      endpointId={endpointId}
      onFill={() => {
        // 语义模式下不允许回填旧 resource 表单：非语义叶不应被当成 NCK 点位。
        message.warning("请选用语义点位（旧 variable 入口在 NCK 上已被禁用）");
      }}
      nckSemantic={{
        definitions: semantic.definitions,
        endpointFamily: semantic.endpointFamily,
        catalogFamily: semantic.catalogFamily,
      }}
      onSemanticSelect={
        semantic.onAddPoint
          ? (sel) => {
              setPreset({
                kind: "nck_variable",
                // Browse 的 preview 只是展示；这里置空，由 NckPointFields 的
                // 服务端 Preview 重新确认。
                point_key: "",
                parameters: sel.parameters,
              });
              setNonce((n) => n + 1);
              setMode("semantic");
              message.info("已把 Browse 叶带入语义表单：请等待服务端 Preview 确认身份");
            }
          : undefined
      }
    />
  );

  const tabs = [
    { key: "semantic", label: "语义点位", children: semanticPane },
    ...(methods.includes("browse")
      ? [{ key: "browse", label: "浏览（语义）", children: browsePane }]
      : []),
    ...(methods.includes("import")
      ? [
          {
            key: "import",
            label: "导入",
            disabled: true,
            children: (
              <div style={{ fontSize: 12, color: "#525252" }}>
                Import 尚未实现（后端 501）。声明保留，待实现后开放。
              </div>
            ),
          },
        ]
      : []),
  ];

  return (
    <div style={{ display: "grid", gap: 8 }}>
      <Alert
        type="info"
        data-testid="nck-semantic-picker"
        message="NCK 语义点位"
        description="旧 variable 原生字段入口在 NCK 上必然被驱动拒绝，已不再提供；请在此配置语义点位，身份以服务端 Preview 为准。"
      />
      {!definitions ? (
        <Alert
          type="warning"
          data-testid="nck-semantic-picker-no-definitions"
          message="定义目录不可用：语义点位保持禁用（fail-closed）"
          description="没有目录就无法确认 scope/Target/Arguments；不会退回旧原生字段流程。"
        />
      ) : null}
      <Tabs activeKey={mode} onChange={(k) => setMode(k as "semantic" | "browse")} items={tabs} />
    </div>
  );
}

// 资源 / 输出标签的唯一取值入口（A1）：统一走 localizedText（优先 zh-CN，
// 回落 default）。返回空串即视为「没有标签」，由调用方退回 id。
function labelOf(label: ResourceDescriptor["label"] | null | undefined): string | undefined {
  const text = localizedText(label);
  return text === "" ? undefined : text;
}
