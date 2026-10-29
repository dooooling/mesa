// 「选择实际对象」——对象探测与填参依据判定（纯模型 + 一个最小请求封装）。
//
// 两件事必须严格分开，混在一起就会产生"看起来完整但错误"的采集地址：
//
// ① **探测（probe）**：向已创建的 Endpoint 要"设备上实际有什么对象"。
//    走既有能力 `POST /api/v1/endpoints/{id}/browse`
//    （crates/core-api/src/lib.rs:910-969，请求字段 **`parent`**、
//    filter/cursor/limit；proto `BrowseRequest` 同形）。节点由驱动给出，
//    前端不解释、不拼地址。
// ② **填参（fill）**：把用户选中的那个节点的**驱动载荷**
//    （`selection_payload_json.parameters` 里的 `target` / `arguments`）
//    原样翻成表单草稿。载荷里没有的键，前端**不补、不猜、不默认**。
//
// 为什么填参只能来自驱动载荷：
// 语义点位的身份坐标（channel / axis / spindle / drive_object / mode_group 的实例号）
// 属于驱动侧事实。前端唯一可以引用的映射就是这条节点自带的不透明载荷——它由驱动
// 用同一份定义算出来（drivers/sinumerik-nck/src/presets.rs:379-440 的
// `semantic_payload` → `prefill_from_native`）。任何"从标签/ID/前缀推出实例号"
// 都会在真机上产生合法格式但指向别的对象的地址；这类错误服务端 Preview 也可能
// 通过（它只校验坐标合法性），因此必须在生成端就拒绝。
//
// 由此产生的三条纪律：
// 1. 需要的对象类型只来自**数据**：定义声明的 `targets` / `target_required`
//    （或 Browse 语义载荷的 `identity.keys[].path`），绝不从前缀/名称推断；
// 2. 节点不可选（`selection_mode = 3`）/ 载荷缺失 / 坐标不匹配 ⇒ **拒绝填参**
//    并给出稳定码，绝不生成"貌似完整"的地址；
// 3. 探测失败必须如实上抛（`ObjectProbeError`），不得退化成空列表。
import {
  evaluateBrowseNode,
  type BrowseNode,
  type BrowseSelectionModeName,
} from "./browseModel";

// ---------------------------------------------------------------------------
// 1. 需要选哪些实际对象（只读数据）
// ---------------------------------------------------------------------------

/** 一类需要选实例的对象。`kind` 原样来自定义/载荷，前端不做词表校验。 */
export interface ObjectNeed {
  kind: string;
  /** 定义是否把它标为必填（`target_required`）。 */
  required: boolean;
  /** 依据来源（写进 UI，让用户知道"凭什么要选"）。 */
  source: "definition_targets" | "browse_identity_keys";
}

/** 判定用的最小定义形态（与 `NckDefinitionLike` 兼容）。 */
export interface ObjectNeedDefinitionLike {
  targets?: unknown;
  target_required?: unknown;
}

function stringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string" && item !== "");
}

/**
 * 从定义声明推出"要选哪些实际对象"。
 *
 * 空数组 = 该变量不需要额外对象实例（例如只靠 arguments 定位的条目），
 * 此时 UI **不显示**"选择实际对象"入口（评审要求：按需出现）。
 */
export function objectNeedsOf(
  definition: ObjectNeedDefinitionLike | null | undefined,
): ObjectNeed[] {
  if (!definition) return [];
  const targets = stringArray(definition.targets);
  const required = stringArray(definition.target_required);
  return targets.map((kind) => ({
    kind,
    required: required.includes(kind),
    source: "definition_targets" as const,
  }));
}

/**
 * 从 Browse 语义载荷的 `identity.keys[].path` 读需求。
 *
 * 这是驱动在 wire 上直接给出的"这条定义还需要哪类实例"（真机实测：
 * `axis` / `channel` / `mode_group` / `index` / `position`），比定义投影更细，
 * 因此当两者都在时**以它为准**（见 `resolveObjectNeeds`）。
 */
export function objectNeedsFromIdentityKeys(keys: unknown): ObjectNeed[] {
  if (!Array.isArray(keys)) return [];
  const out: ObjectNeed[] = [];
  for (const key of keys) {
    if (!key || typeof key !== "object" || Array.isArray(key)) continue;
    const path = (key as { path?: unknown }).path;
    if (typeof path !== "string" || path.trim() === "") continue;
    const kind = path.trim();
    if (out.some((item) => item.kind === kind)) continue;
    out.push({
      kind,
      required: (key as { required?: unknown }).required === true,
      source: "browse_identity_keys",
    });
  }
  return out;
}

/**
 * 合并两个来源：wire 的 `identity.keys` 优先（更细），定义 `targets` 补足。
 *
 * 注意这不是"猜"：两者都是数据，合并只是取并集，`wire` 标记来源以便 UI 说明。
 */
export function resolveObjectNeeds(args: {
  definition: ObjectNeedDefinitionLike | null | undefined;
  identityKeys?: unknown;
}): ObjectNeed[] {
  const fromWire = objectNeedsFromIdentityKeys(args.identityKeys);
  if (fromWire.length) {
    const extra = objectNeedsOf(args.definition).filter(
      (need) => !fromWire.some((item) => item.kind === need.kind),
    );
    return [...fromWire, ...extra];
  }
  return objectNeedsOf(args.definition);
}

// ---------------------------------------------------------------------------
// 2. 探测入口的可用性（端点必须已创建且连接配置可读）
// ---------------------------------------------------------------------------

export type ObjectProbeGate =
  | { available: true }
  | { available: false; code: ObjectProbeGateCode; reason: string };

export type ObjectProbeGateCode = "ENDPOINT_MISSING" | "ENDPOINT_NOT_CONNECTED";

/**
 * 探测入口是否可用。
 *
 * **未连接端点时绝不发起在线探测**（评审硬要求）：探测会临时拉起驱动进程并读真机，
 * 在连接未就绪的情况下发起，只会得到一堆误导性失败。因此这里先判"端点是否已创建
 * 且连接配置可读"，不满足就只给原因、不给按钮动作。
 *
 * `endpointKnown` = 端点清单读取成功且能查到本端点；`connectionKnown` =
 * 该端点的有效连接配置可读（family 可读是既有判据，见 EndpointAcquisitionPane）。
 */
export function objectProbeGate(args: {
  endpointId?: string | null;
  endpointKnown: boolean;
  connectionKnown: boolean;
}): ObjectProbeGate {
  const id = typeof args.endpointId === "string" ? args.endpointId.trim() : "";
  if (!id) {
    return {
      available: false,
      code: "ENDPOINT_MISSING",
      reason: "需先创建并连接端点：当前页面没有可用的 Endpoint，不会发起在线探测",
    };
  }
  if (!args.endpointKnown) {
    return {
      available: false,
      code: "ENDPOINT_NOT_CONNECTED",
      reason:
        "需先创建并连接端点：无法确认该 Endpoint 是否存在（未读到端点清单），不会发起在线探测",
    };
  }
  if (!args.connectionKnown) {
    return {
      available: false,
      code: "ENDPOINT_NOT_CONNECTED",
      reason:
        "需先创建并连接端点：该 Endpoint 的连接配置不可读（缺少 family 等必要项），不会发起在线探测",
    };
  }
  return { available: true };
}

// ---------------------------------------------------------------------------
// 3. 探测请求（既有 `/endpoints/{id}/browse`，字段 parent/filter/cursor/limit）
// ---------------------------------------------------------------------------

export type ObjectProbeErrorCode =
  | "PROBE_REQUEST_FAILED"
  | "PROBE_NETWORK"
  | "PROBE_RESPONSE_INVALID";

/** 探测失败（**必须**原样上报给 UI，不得转成空列表）。 */
export class ObjectProbeError extends Error {
  code: ObjectProbeErrorCode;
  status: number;
  detail: string;

  constructor(args: {
    code: ObjectProbeErrorCode;
    status: number;
    message: string;
    detail?: string;
  }) {
    super(args.message);
    this.name = "ObjectProbeError";
    this.code = args.code;
    this.status = args.status;
    this.detail = args.detail ?? "";
  }
}

export function isObjectProbeError(value: unknown): value is ObjectProbeError {
  return value instanceof ObjectProbeError;
}

export interface ObjectProbePage {
  nodes: BrowseNode[];
  nextCursor: string | null;
}

export type ObjectProbeFetchLike = (
  input: string,
  init?: RequestInit,
) => Promise<Pick<Response, "ok" | "status" | "json">>;

/** 探测请求体（`parent` 是下钻字段；写错会被服务端当成根，静默返回根节点）。 */
export function buildObjectProbeRequest(args: {
  parent?: string;
  filter?: string;
  cursor?: string;
  limit?: number;
}): { parent: string; filter: string; cursor: string; limit: number } {
  return {
    parent: args.parent ?? "",
    filter: args.filter ?? "",
    cursor: args.cursor ?? "",
    limit: args.limit ?? 50,
  };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 节点形状的宽松校验：不合格就整体拒绝（不拿 `undefined` 继续拼 UI）。 */
function parseProbeNodes(value: unknown): BrowseNode[] {
  if (!Array.isArray(value)) {
    throw new ObjectProbeError({
      code: "PROBE_RESPONSE_INVALID",
      status: 200,
      message: "探测响应缺少 nodes 数组：无法确认发现了哪些对象",
    });
  }
  return value.map((node, index) => {
    if (!isPlainObject(node)) {
      throw new ObjectProbeError({
        code: "PROBE_RESPONSE_INVALID",
        status: 200,
        message: `探测响应 nodes[${index}] 不是对象：无法确认发现了哪些对象`,
      });
    }
    const id = node.id;
    if (typeof id !== "string" || id === "") {
      throw new ObjectProbeError({
        code: "PROBE_RESPONSE_INVALID",
        status: 200,
        message: `探测响应 nodes[${index}] 缺少 id：无法下钻或选用`,
      });
    }
    return node as unknown as BrowseNode;
  });
}

/**
 * 读取一页实际对象。
 *
 * 失败一律抛 `ObjectProbeError`（含服务端错误码与原文），调用方据此展示失败；
 * 空页是**正常结果**（驱动如实返回 0 项），两者在 UI 上必须可区分。
 */
export async function fetchObjectProbePage(
  request: {
    endpointId: string;
    parent?: string;
    filter?: string;
    cursor?: string;
    limit?: number;
  },
  fetchImpl: ObjectProbeFetchLike = fetch,
): Promise<ObjectProbePage> {
  const endpointId = typeof request.endpointId === "string" ? request.endpointId.trim() : "";
  if (!endpointId) {
    throw new ObjectProbeError({
      code: "PROBE_REQUEST_FAILED",
      status: 0,
      message: "缺少 Endpoint：不能在未连接端点时发起在线探测",
    });
  }
  const body = buildObjectProbeRequest(request);
  let response: Pick<Response, "ok" | "status" | "json">;
  try {
    response = await fetchImpl(`/api/v1/endpoints/${encodeURIComponent(endpointId)}/browse`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch (error) {
    throw new ObjectProbeError({
      code: "PROBE_NETWORK",
      status: 0,
      message: `探测请求失败：${error instanceof Error ? error.message : String(error)}`,
    });
  }
  const payload = (await response.json().catch(() => null)) as unknown;
  if (!response.ok) {
    const envelope = isPlainObject(payload) ? payload.error : undefined;
    const code = isPlainObject(envelope) && typeof envelope.code === "string" ? envelope.code : "";
    const message =
      isPlainObject(envelope) && typeof envelope.message === "string" ? envelope.message : "";
    throw new ObjectProbeError({
      code: "PROBE_REQUEST_FAILED",
      status: response.status,
      message: `探测失败（HTTP ${response.status}${code ? ` · ${code}` : ""}）${
        message ? `：${message}` : ""
      }`,
      detail: code,
    });
  }
  if (!isPlainObject(payload)) {
    throw new ObjectProbeError({
      code: "PROBE_RESPONSE_INVALID",
      status: response.status,
      message: "探测响应不是 JSON 对象：无法确认发现了哪些对象",
    });
  }
  const nextCursor =
    typeof payload.next_cursor === "string" && payload.next_cursor !== ""
      ? payload.next_cursor
      : null;
  return { nodes: parseProbeNodes(payload.nodes), nextCursor };
}

// ---------------------------------------------------------------------------
// 4. 节点 → 填参依据（**唯一**允许的映射来源：驱动载荷）
// ---------------------------------------------------------------------------

export type ObjectFillCode =
  | "OBJECT_NODE_NOT_SELECTABLE"
  | "OBJECT_PAYLOAD_MISSING"
  | "OBJECT_PAYLOAD_INVALID"
  | "OBJECT_VARIABLE_MISMATCH"
  | "OBJECT_REVISION_MISMATCH"
  | "OBJECT_TARGET_UNDECLARED"
  | "OBJECT_NO_ADDRESS_MAPPING";

/** 填参结果：`draft` 直接是表单草稿（字符串），`basis` 是给用户看的依据。 */
export interface ObjectFill {
  variableId: string;
  /** 表单 target 草稿（`kind -> 文本`；drive_object 另带 `<kind>.entry`）。 */
  target: Record<string, string>;
  /** 表单 arguments 草稿。 */
  arguments: Record<string, string>;
  /** 已解析的身份键（来自驱动载荷）。 */
  resolved: string[];
  /** 仍未解析的必填身份键（必须由用户补齐，前端不填默认值）。 */
  unresolved: string[];
  /** 依据说明（原样展示，避免"猜出来的地址"被当成权威）。 */
  basis: string;
}

export type ObjectFillResult =
  | { ok: true; fill: ObjectFill }
  | { ok: false; code: ObjectFillCode; message: string; unresolved: string[] };

function refuse(
  code: ObjectFillCode,
  message: string,
  unresolved: string[] = [],
): ObjectFillResult {
  return { ok: false, code, message, unresolved };
}

function numberText(value: unknown): string | null {
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  return null;
}

/**
 * 从节点推出填参草稿。
 *
 * 判定顺序（不可调换）：可选性 → 载荷形状 → 变量/rev 坐标 → target/arguments 归属。
 * 任何一步不成立都返回稳定码，**不写任何地址**。
 */
export function deriveObjectFill(args: {
  node: BrowseNode;
  definition: {
    variable_id?: string;
    revision?: number;
    targets?: unknown;
    target_required?: unknown;
    identity_keys?: unknown;
  } | null;
  /** 需要的对象类型（来自 `resolveObjectNeeds`）；只用于计算 unresolved。 */
  needs?: readonly ObjectNeed[];
}): ObjectFillResult {
  const { node, definition } = args;
  if (!definition) {
    return refuse("OBJECT_PAYLOAD_INVALID", "未选择变量：无法判断该对象要填哪些参数");
  }
  const evaluation = evaluateBrowseNode(node);
  if (evaluation.mode === "not_selectable" || evaluation.mode === "unknown") {
    const block = evaluation.blocks[0];
    return refuse(
      "OBJECT_NODE_NOT_SELECTABLE",
      `该节点不可选（${block?.code ?? modeLabel(evaluation.mode)}）：${
        block?.message ?? "驱动未声明它可作为对象"
      }`,
    );
  }
  if (!evaluation.semanticPayload) {
    return refuse(
      "OBJECT_PAYLOAD_MISSING",
      "该节点没有给出语义载荷（selection_payload_json）：无法确认它对应哪个实际对象，拒绝填参",
    );
  }
  const parameters = evaluation.semanticPayload.parameters;
  if (!isPlainObject(parameters)) {
    return refuse(
      "OBJECT_PAYLOAD_INVALID",
      "该节点的语义载荷缺少 parameters 对象：拒绝填参（不生成猜测地址）",
    );
  }
  const expectedVariable =
    typeof definition.variable_id === "string" ? definition.variable_id : "";
  const payloadVariable =
    typeof parameters.variable_id === "string" ? parameters.variable_id : "";
  if (expectedVariable && payloadVariable && expectedVariable !== payloadVariable) {
    return refuse(
      "OBJECT_VARIABLE_MISMATCH",
      `所选对象属于另一变量（${payloadVariable}），与当前选择的 ${expectedVariable} 不一致：不填参`,
    );
  }
  const expectedRevision =
    typeof definition.revision === "number" ? definition.revision : null;
  const payloadRevision =
    typeof parameters.definition_revision === "number" ? parameters.definition_revision : null;
  if (
    expectedRevision !== null &&
    payloadRevision !== null &&
    expectedRevision !== payloadRevision
  ) {
    return refuse(
      "OBJECT_REVISION_MISMATCH",
      `所选对象的定义版本（rev ${payloadRevision}）与当前定义（rev ${expectedRevision}）不一致：不填参`,
    );
  }

  const allowedTargets = stringArray(definition.targets);
  const rawTarget = isPlainObject(parameters.target) ? parameters.target : {};
  const rawArguments = isPlainObject(parameters.arguments) ? parameters.arguments : {};

  const target: Record<string, string> = {};
  const resolved: string[] = [];
  for (const [kind, value] of Object.entries(rawTarget)) {
    // 载荷里出现定义没声明的 Target 变体 ⇒ 拒绝（不静默丢弃、不猜测归属）。
    if (allowedTargets.length && !allowedTargets.includes(kind)) {
      return refuse(
        "OBJECT_TARGET_UNDECLARED",
        `驱动载荷给出的 Target \`${kind}\` 未在该定义声明：拒绝填参（避免写入未声明的身份坐标）`,
      );
    }
    if (isPlainObject(value)) {
      // drive_object 这类 {number, entry} 信封：两个值都来自载荷。
      const number = numberText(value.number);
      const entry = typeof value.entry === "string" ? value.entry : "";
      if (number !== null) {
        target[kind] = number;
        resolved.push(kind);
      }
      if (entry !== "") target[`${kind}.entry`] = entry;
      if (number !== null && entry !== "") resolved.push(`${kind}.entry`);
      continue;
    }
    const text = numberText(value);
    if (text !== null) {
      target[kind] = text;
      resolved.push(kind);
    }
  }

  const argumentDraft: Record<string, string> = {};
  for (const [key, value] of Object.entries(rawArguments)) {
    const text =
      typeof value === "string" ? value : numberText(value);
    if (text !== null && text !== "") {
      argumentDraft[key] = text;
      resolved.push(key);
    }
  }

  if (Object.keys(target).length === 0 && Object.keys(argumentDraft).length === 0) {
    return refuse(
      "OBJECT_NO_ADDRESS_MAPPING",
      "该对象没有提供可用的地址映射（载荷 target/arguments 为空）：不可自动填参，请手动确认实例号",
    );
  }

  // 未解析的必填身份键：驱动给的需求清单 + 定义声明的必填 Target 的并集。
  const unresolvedSet = new Set<string>();
  const needs = args.needs ?? objectNeedsOf(definition);
  for (const need of needs) {
    if (!need.required) continue;
    if (!(need.kind in target)) unresolvedSet.add(need.kind);
  }
  const requiredTargets = stringArray(definition.target_required);
  for (const kind of requiredTargets) {
    if (!(kind in target)) unresolvedSet.add(kind);
  }

  return {
    ok: true,
    fill: {
      variableId: payloadVariable || expectedVariable,
      target,
      arguments: argumentDraft,
      resolved,
      unresolved: [...unresolvedSet],
      basis: `依据：驱动返回的选择载荷 parameters.target/arguments（对象节点 ${node.id}` +
        `${resolved.length ? `，已解析 ${resolved.join("/")}` : ""}）`,
    },
  };
}

/** 模式名 → 中文短标签（只用于文案）。 */
export function modeLabel(mode: BrowseSelectionModeName): string {
  switch (mode) {
    case "semantic":
      return "语义可选";
    case "resource":
      return "资源可选";
    case "not_selectable":
      return "不可选";
    case "unspecified":
      return "未声明";
    default:
      return "未知模式";
  }
}

/**
 * 节点能否下钻。
 *
 * 只看 wire 的 `has_children`：驱动说没有子节点就不展开。真机上的
 * "未确认对象"（例如轴 6 未分配有效驱动输出）正是 `has_children=false`，
 * 前端必须如实显示、不展开、也不当成可选项。
 */
export function canExpandNode(node: BrowseNode): boolean {
  return node.has_children === true;
}
