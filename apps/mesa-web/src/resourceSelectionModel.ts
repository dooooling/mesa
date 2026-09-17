// ResourceSelection reconciliation（Selection 层契约级比较）。
//
// 冻结边界（协议接入不变量）：
// - 只比较 resource_id + effective(parameters) + output，不理解任何协议地址；
// - effective(parameters) = Descriptor defaults 物化 + 显式值覆盖，
//   去 undefined、key 稳定化、保持 JSON 类型；绝不做 "1"->1 / trim /
//   lowercase / 地址转换 / alias 解析；
// - 物理地址是否相同只有 Driver 知道，本模块只敢说"契约层完全相同"。
// - Core 现有正式契约（point_key / schema / resource / output / mode /
//   access 等）不变：本模块只防止当前编辑器产生契约级重复，不扩大 Core。
//
// 三个集合（用途严格区分）：
// - editableSelections：唯一允许 merge / append 的集合；
// - protectedSelections（preserved tasks 展开）：永远只读，只参与
//   已采集反显、exact duplicate 检测、point_key 占用检测；
// - visibleSelections = editable + protected：展示与检测用。
//
// 纯函数文件：不得 import React / AntD / 组件，只放 Selection 纯函数。

/** 与 ResourcePickerAntd 产出 / deviceModel.ResourceSelection 同构的最小形态。 */
export interface ResourceSelectionLike {
  resource_id: string;
  parameters: Record<string, unknown>;
  outputs: Array<{ output: string; point_key: string }>;
}

/** Descriptor parameters 的 schema 字段最小形态（仅 defaults 物化所需）。 */
export interface SchemaDefaultsSource {
  fields: Array<{ key: string; default?: unknown }>;
}

/** required 前置判断的最小字段形态（仅 key + required；无类型/range/enum）。
 * Core REQUIRED 语义 = 字段 key 缺席（`obj.get(&key)` 为 None 即报），
 * 与值 falsy 无关：`0` / `false` / `""` 都是“在场”，不得误判缺失。 */
export interface RequiredFieldSource {
  key: string;
  required?: boolean;
}

export interface RequiredParamsSource {
  fields: Array<RequiredFieldSource>;
}

/**
 * Web required preflight（表单完整性预检，唯一允许的前端 required 判断）：
 * 返回 required 但在 params 中**缺席**的字段 key 列表。
 * - 缺席 = `!(key in params)` 或值为 `undefined`/`null`（控件清空态）；
 * - `0` / `false` / `""` 视为在场（与 Core `obj.get` 语义对齐，不得用 truthy 判）；
 * - secret marker（`{secret_set:true}` / `{clear_secret:true}` 对象）视为在场；
 * - 只回答"缺不缺"，绝不判断类型/enum/range/pattern（那是 Core 唯一真值）。
 */
export function missingRequiredParams(
  schema: RequiredParamsSource,
  parameters: Record<string, unknown>,
): string[] {
  const params = parameters ?? {};
  const out: string[] = [];
  for (const f of schema.fields ?? []) {
    if (!f.required) continue;
    // hasOwn 优先（显式传入但 undefined 仍算缺席，与 removeUndefined 对齐）；
    // in 不可用——原型链 key 会误判在场。
    if (!Object.prototype.hasOwnProperty.call(params, f.key)) {
      out.push(f.key);
      continue;
    }
    const v = params[f.key];
    if (v === undefined || v === null) out.push(f.key);
  }
  return out;
}

/**
 * P1-4（自 DescriptorFields.tsx 迁移，语义逐字保留）：将 schema 中带
 * `default` 的字段物化为参数初值，保证"UI 显示的 default 即实际保存值"。
 */
export function materializeSchemaDefaults(
  schema: SchemaDefaultsSource,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of schema.fields ?? []) {
    if (f.default !== undefined) out[f.key] = f.default;
  }
  return out;
}

/** JSON 语义的稳定化：对象 key 排序，数组保序；undefined 丢弃。 */
function stableJson(value: unknown): unknown {  if (value === undefined) return undefined;
  if (value === null) return null;
  if (Array.isArray(value)) {
    const out: unknown[] = [];
    for (const v of value) {
      const s = stableJson(v);
      if (s !== undefined) out.push(s);
    }
    return out;
  }
  if (typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(value as Record<string, unknown>).sort()) {
      const s = stableJson((value as Record<string, unknown>)[k]);
      if (s !== undefined) out[k] = s;
    }
    return out;
  }
  return value;
}

/** 显式参数去 undefined：控件清空值不得覆盖 Descriptor default。
 * 必须先删再覆盖，否则 {axis:undefined} 会先盖掉 default 再被 stable 删掉，
 * 导致与 {} / {axis:1} 判成不同实例（终审 blocker #1）。 */
function removeUndefined(
  parameters: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(parameters ?? {})) {
    if (v !== undefined) out[k] = v;
  }
  return out;
}

/**
 * effective(parameters)：Descriptor defaults 被显式值覆盖后的生效参数。
 * 顺序：先删 explicit 的 undefined → 再覆盖 defaults → 最后 stable。
 * 只做结构归一，不做任何值语义转换（类型保持原样）。
 */
export function effectiveResourceParameters(
  schema: SchemaDefaultsSource,
  parameters: Record<string, unknown>,
): Record<string, unknown> {
  const merged: Record<string, unknown> = {
    ...materializeSchemaDefaults(schema),
    ...removeUndefined(parameters),
  };
  return (stableJson(merged) ?? {}) as Record<string, unknown>;
}

/** ResourceInstance 签名：[resource_id, effective(parameters)] 结构编码。
 * 无歧义数组编码，不依赖分隔符（resource/output ID 含特殊字符仍安全）。 */
export function resourceInstanceKey(
  resourceId: string,
  effectiveParameters: Record<string, unknown>,
): string {
  return JSON.stringify([resourceId, stableJson(effectiveParameters) ?? {}]);
}

/** ExactSelectionOutput 签名：[ResourceInstance, output] 结构编码。 */
export function selectionOutputKey(instanceKey: string, output: string): string {
  return JSON.stringify([instanceKey, output]);
}

/** 候选 Selection 进入父层后的裁决结果（调用方据此 warning，不抛异常）。 */
export type ReconcileDecision =
  | { kind: "duplicate-editable"; outputs: string[] }
  | { kind: "duplicate-protected"; outputs: string[] }
  | { kind: "point-key-conflict"; pointKeys: string[] }
  | { kind: "merge"; mergedIndex: number; addedOutputs: string[] }
  | { kind: "append" };

/**
 * 候选 Selection 的 reconciliation（冻结算法）：
 * exact output 在 editable → duplicate 拒绝；
 * exact output 在 protected → 已在其他任务采集，拒绝；
 * point_key 在任意 task 已存在 → point_key 冲突，拒绝；
 * editable 存在同 ResourceInstance → 只把新 output merge 进该 editable 项；
 * 否则 append 新 selection。
 * 只返回"怎么做"，实际改 editable 数组由调用方执行；protected 永不被改。
 */
export function reconcileEditableSelection(args: {
  candidate: ResourceSelectionLike;
  editable: ResourceSelectionLike[];
  protectedSelections: ResourceSelectionLike[];
  schemaOf: (resourceId: string) => SchemaDefaultsSource;
}): ReconcileDecision {
  const schema = args.schemaOf(args.candidate.resource_id);
  const inst = resourceInstanceKey(
    args.candidate.resource_id,
    effectiveResourceParameters(schema, args.candidate.parameters),
  );
  const sigOf = (
    sel: ResourceSelectionLike,
    resourceId: string,
    params: Record<string, unknown>,
    output: string,
  ): string => {
    const s = args.schemaOf(resourceId);
    return selectionOutputKey(
      resourceInstanceKey(resourceId, effectiveResourceParameters(s, params)),
      output,
    );
  };

  // 全集签名（exact 比较用）
  const editableSigs = new Set<string>();
  for (const sel of args.editable) {
    for (const o of sel.outputs) {
      editableSigs.add(sigOf(sel, sel.resource_id, sel.parameters, o.output));
    }
  }
  const protectedSigs = new Set<string>();
  for (const sel of args.protectedSelections) {
    for (const o of sel.outputs) {
      protectedSigs.add(sigOf(sel, sel.resource_id, sel.parameters, o.output));
    }
  }

  const candSigs = args.candidate.outputs.map((o) =>
    sigOf(args.candidate, args.candidate.resource_id, args.candidate.parameters, o.output),
  );

  // 1) exact output 已存在于 editable
  const dupEditable = args.candidate.outputs
    .filter((_, i) => editableSigs.has(candSigs[i]))
    .map((o) => o.output);
  if (dupEditable.length) return { kind: "duplicate-editable", outputs: dupEditable };

  // 2) exact output 已存在于 protected（其他任务已采集）
  const dupProtected = args.candidate.outputs
    .filter((_, i) => protectedSigs.has(candSigs[i]))
    .map((o) => o.output);
  if (dupProtected.length) return { kind: "duplicate-protected", outputs: dupProtected };

  // 3) point_key 在任意 task 已存在（Endpoint-wide unique 的前端执行）
  const knownKeys = new Set<string>();
  for (const sel of [...args.editable, ...args.protectedSelections]) {
    for (const o of sel.outputs) knownKeys.add(o.point_key);
  }
  const keyConflicts = args.candidate.outputs
    .filter((o) => knownKeys.has(o.point_key))
    .map((o) => o.point_key);
  if (keyConflicts.length) return { kind: "point-key-conflict", pointKeys: keyConflicts };

  // 4) editable 存在同 ResourceInstance → merge（只改 editable 该项）
  const idx = args.editable.findIndex((sel) => {
    const s = args.schemaOf(sel.resource_id);
    return (
      sel.resource_id === args.candidate.resource_id &&
      resourceInstanceKey(sel.resource_id, effectiveResourceParameters(s, sel.parameters)) ===
        inst
    );
  });
  if (idx >= 0) {
    return {
      kind: "merge",
      mergedIndex: idx,
      addedOutputs: args.candidate.outputs.map((o) => o.output),
    };
  }

  // 5) append
  return { kind: "append" };
}

/** 已采集反显：当前表单 (resource+params) 下各 output 的归属。 */
export type OutputOwnership =
  | { state: "free" }
  | { state: "editable" }
  | { state: "protected" };

/**
 * 计算当前表单各 output 的归属（Picker 显示语义）：
 * exact 签名在 editable → editable（已采集，disabled）；
 * 在 protected → protected（其他任务已采集，disabled）；
 * 否则 free（可选）。
 */
export function outputOwnership(
  resourceId: string,
  parameters: Record<string, unknown>,
  outputs: string[],
  editable: ResourceSelectionLike[],
  protectedSelections: ResourceSelectionLike[],
  schemaOf: (resourceId: string) => SchemaDefaultsSource,
): Map<string, OutputOwnership> {
  const sigOf = (
    selRid: string,
    selParams: Record<string, unknown>,
    output: string,
  ): string => {
    const s = schemaOf(selRid);
    return selectionOutputKey(
      resourceInstanceKey(selRid, effectiveResourceParameters(s, selParams)),
      output,
    );
  };
  const editableSigs = new Set<string>();
  for (const sel of editable) {
    for (const o of sel.outputs) {
      editableSigs.add(sigOf(sel.resource_id, sel.parameters, o.output));
    }
  }
  const protectedSigs = new Set<string>();
  for (const sel of protectedSelections) {
    for (const o of sel.outputs) {
      protectedSigs.add(sigOf(sel.resource_id, sel.parameters, o.output));
    }
  }
  const out = new Map<string, OutputOwnership>();
  for (const o of outputs) {
    const sig = sigOf(resourceId, parameters, o);
    if (editableSigs.has(sig)) out.set(o, { state: "editable" });
    else if (protectedSigs.has(sig)) out.set(o, { state: "protected" });
    else out.set(o, { state: "free" });
  }
  return out;
}

/** 全集 point_key（editable + protected）：suggestPointKey 的唯一依据。 */
export function allKnownPointKeys(
  editable: ResourceSelectionLike[],
  protectedSelections: ResourceSelectionLike[],
): Set<string> {
  const out = new Set<string>();
  for (const sel of [...editable, ...protectedSelections]) {
    for (const o of sel.outputs) out.add(o.point_key);
  }
  return out;
}

/**
 * 父层 onAdd 共享实现（冻结行为，两个 Picker 调用方必须一致）：
 * 同一 reconcileEditableSelection 裁决 + 同一阵列变更语义。
 * - duplicate-editable / duplicate-protected / point-key-conflict →
 *   返回对应 message，数组不变；
 * - merge → 只改 editable[mergedIndex].outputs（并入新 output，protected 不动）；
 * - append → editable 追加 candidate。
 * AddDeviceFlow 以 protectedSelections=[] 调用（无 preserved task）。
 * 返回 null 表示成功（调用方直接 setSels(next)）；返回 string 为拒绝原因。
 */
export function applySelectionAdd(args: {
  candidate: ResourceSelectionLike;
  editable: ResourceSelectionLike[];
  protectedSelections: ResourceSelectionLike[];
  schemaOf: (resourceId: string) => SchemaDefaultsSource;
}): { next: ResourceSelectionLike[]; message: string | null } {
  const decision = reconcileEditableSelection({
    candidate: args.candidate,
    editable: args.editable,
    protectedSelections: args.protectedSelections,
    schemaOf: args.schemaOf,
  });
  switch (decision.kind) {
    case "duplicate-editable":
      return {
        next: args.editable,
        message: `已在采集中：${decision.outputs.join(", ")}，不会重复加入`,
      };
    case "duplicate-protected":
      return {
        next: args.editable,
        message: `已在其他任务采集：${decision.outputs.join(", ")}，不会重复加入`,
      };
    case "point-key-conflict":
      return {
        next: args.editable,
        message: `point_key 已被其他采集项使用：${decision.pointKeys.join(", ")}（请改名）`,
      };
    case "merge":
      return {
        next: args.editable.map((sel, i) =>
          i === decision.mergedIndex
            ? {
                ...sel,
                outputs: [
                  ...sel.outputs,
                  ...args.candidate.outputs.filter(
                    (o) => !sel.outputs.some((e) => e.output === o.output),
                  ),
                ],
              }
            : sel,
        ),
        message: null,
      };
    case "append":
      return { next: [...args.editable, args.candidate], message: null };
  }
}
