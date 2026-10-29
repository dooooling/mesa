// 联动选择（级联下拉）的**判定层**：把"这个变量还需要哪些实例参数、设备上到底
// 有没有能提供这些参数的对象、拿不到时是什么原因"收敛成一套纯函数。
//
// 为什么单独一层（而不是写在组件里）：
// 1. **零手填**：语义点位的实例号（channel / axis / spindle / drive_object / index…）
//    只能来自驱动返回的选择载荷。前端一旦提供"输入框"，就必然出现"用户随手填一个
//    合法但指向别的对象的数字"——服务端 Preview 只校验坐标合法性，拦不住这类错误。
//    因此参数的产生路径被压缩成唯一一条：探测 → 用户选对象 → 载荷翻成草稿。
// 2. **拿不到就说拿不到**：探测不可用 / 探测失败 / 探测成功但没有候选，是三种
//    不同的事实，必须给出不同原因；其中任何一种都**不得**回退成手填表单，也
//    不得填默认值（默认值等于凭空编造身份坐标）。
// 3. **"探测失败" ≠ "设备没有"**：分支不可达时只能承认"探测不完整"，不能说
//    "设备上不存在该对象"——后者会把网络/驱动故障伪装成设备事实。
//
// 本模块只消费驱动数据：候选的归属（`parameters.variable_id` /
// `identity.applies_to_variables`）与提供的键（载荷 `target` / `arguments` /
// `identity.keys[].path`）全部来自 wire，前端不按前缀/标签推断任何厂商语义。

import { evaluateBrowseNode, type BrowseNode } from "./browseModel";
import {
  deriveObjectFill,
  fetchObjectProbePage,
  isObjectProbeError,
  type ObjectFill,
  type ObjectNeed,
  type ObjectProbeFetchLike,
  type ObjectProbeGate,
} from "./objectProbe";
import type { NckFormDraft, NckFormModel } from "./nckPointModel";

/** 判定用的最小定义形态（与 `NckDefinitionLike` 兼容）。 */
export interface CascadeDefinitionLike {
  variable_id?: string;
  revision?: number;
  targets?: unknown;
  target_required?: unknown;
  identity_keys?: unknown;
}

// ---------------------------------------------------------------------------
// 1. 需要由设备提供的参数键
// ---------------------------------------------------------------------------

/** 一级级联：一个"必须由设备给出取值"的参数键。 */
export interface CascadeKey {
  /** target 变体名或 argument key（逐字来自定义，前端不改写）。 */
  key: string;
  /** 展示标签（argument 用定义里的 label，target 用变体名）。 */
  label: string;
  source: "target" | "argument";
  /** `drive_object` 这类 `{number, entry}` 信封的子键：与 number 同属一级。 */
  subKey?: string;
}

/**
 * 从表单模型推出需要逐级选择的参数键。
 *
 * 只取**必填**项：可选 target/argument 不选也能形成合法身份，凭空多给一级
 * 下拉只会让用户以为"不选就不完整"。顺序沿用定义声明顺序（前端不重排，
 * 因为"先通道后主轴"这类顺序本身就是定义侧事实）。
 */
export function cascadeKeysOf(model: NckFormModel): CascadeKey[] {
  const keys: CascadeKey[] = [];
  for (const field of model.targetFields) {
    if (!field.required) continue;
    keys.push({
      key: field.kind,
      label: field.kind,
      source: "target",
      subKey: field.requiresEntry ? `${field.kind}.entry` : undefined,
    });
  }
  for (const field of model.argumentFields) {
    if (!field.required) continue;
    keys.push({ key: field.key, label: field.label, source: "argument" });
  }
  return keys;
}

/** 该键是否已在草稿里被填上（含 entry 子键）。 */
export function isKeyResolved(key: CascadeKey, draft: NckFormDraft): boolean {
  const value =
    key.source === "target"
      ? (draft.target[key.key] ?? "").trim()
      : (draft.arguments[key.key] ?? "").trim();
  if (value === "") return false;
  if (key.subKey) {
    return (draft.target[key.subKey] ?? "").trim() !== "";
  }
  return true;
}

// ---------------------------------------------------------------------------
// 2. 候选对象（全部来自驱动载荷）
// ---------------------------------------------------------------------------

/** 一个候选对象：能填哪些键、填什么、以及为什么不能用。 */
export interface CascadeOption {
  nodeId: string;
  label: string;
  kind: string;
  /** 载荷**声明**了哪些键（含无取值的键；用于解释"为什么不能用"）。 */
  declared: string[];
  /** 载荷确实给出取值的键（`ObjectFill.resolved`）。 */
  provides: string[];
  /** 可选用时的填参结果；不可用时为 null。 */
  fill: ObjectFill | null;
  /** 不可选用的原因（驱动诊断 / 载荷缺失 / 无地址映射…）。 */
  blocked: { code: string; message: string } | null;
}

/** 驱动自己报出的异常对象（无载荷、无法归属到任何参数键，但必须可见）。 */
export interface CascadeDiagnostic {
  nodeId: string;
  label: string;
  code: string;
  message: string;
  severity: number;
}

/** 一次探测里的一个分支（一个 browse parent）的结果。 */
export interface CascadeBranch {
  parent: string;
  ok: boolean;
  code: string;
  message: string;
}

/** 一次探测的完整结果。 */
export interface CascadeScan {
  options: CascadeOption[];
  diagnostics: CascadeDiagnostic[];
  branches: CascadeBranch[];
  /** 至少一个分支失败 ⇒ 探测**不完整**（不得据此断言"设备上没有"）。 */
  incomplete: boolean;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function textOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/**
 * 只用于**归类与解释**的载荷读取：不判可用性，因此对 `selection_mode=3`
 * 的节点同样生效（"不可选"的节点仍要能被解释成"为什么不可用"）。
 *
 * 优先 `selection_payload_json`；旧驱动没有它时回退 `binding_json` 信封。
 * 注意：这条路径**永不**授予可选性——可用性只由 `evaluateBrowseNode` 决定。
 */
function declarativePayloadOf(node: BrowseNode): Record<string, unknown> | null {
  const raw = (node.selection_payload_json ?? "").trim();
  if (raw !== "") {
    try {
      const parsed = JSON.parse(raw) as unknown;
      if (isPlainObject(parsed)) return parsed;
    } catch {
      return null;
    }
    return null;
  }
  const envelope = (node.binding_json ?? "").trim();
  if (envelope === "") return null;
  try {
    const parsed = JSON.parse(envelope) as unknown;
    return isPlainObject(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

/**
 * 节点是否属于当前变量。
 *
 * 判据全部是数据：
 * - 载荷声明了 `parameters.variable_id` ⇒ 必须与当前定义一致（明确属于别的变量就排除）；
 * - 否则看 `identity.applies_to_variables`（驱动声明的适用范围）⇒ 不包含当前变量就排除；
 * - 两者都没声明 ⇒ 视为通用实例提供者（例如"通道 1 这台设备上存在"），保留。
 *
 * 注意：这里**不**从节点 id / 标签前缀推断归属——那类推断在真机上会挑出
 * 属于另一个变量的实例，服务端 Preview 也拦不住。
 */
export function nodeBelongsToVariable(
  payload: Record<string, unknown> | null,
  variableId: string,
): boolean {
  if (!payload) return true;
  const parameters = isPlainObject(payload.parameters) ? payload.parameters : {};
  const declaredVariable = textOf(parameters.variable_id);
  if (declaredVariable !== "") return declaredVariable === variableId;
  const identity = isPlainObject(payload.identity) ? payload.identity : null;
  const applies = identity?.applies_to_variables;
  if (Array.isArray(applies) && applies.length) {
    return applies.some((item) => item === variableId);
  }
  return true;
}

/** 节点声明自己提供哪些键：载荷 target/arguments 的实际键 + `identity.keys[].path`。 */
export function declaredKeysOf(payload: Record<string, unknown> | null): string[] {
  if (!payload) return [];
  const out: string[] = [];
  const push = (key: string) => {
    if (key !== "" && !out.includes(key)) out.push(key);
  };
  const parameters = isPlainObject(payload.parameters) ? payload.parameters : {};
  if (isPlainObject(parameters.target)) {
    for (const [kind, value] of Object.entries(parameters.target)) {
      push(kind);
      if (isPlainObject(value) && typeof value.entry === "string" && value.entry !== "") {
        push(`${kind}.entry`);
      }
    }
  }
  if (isPlainObject(parameters.arguments)) {
    for (const key of Object.keys(parameters.arguments)) push(key);
  }
  const identity = isPlainObject(payload.identity) ? payload.identity : null;
  if (Array.isArray(identity?.keys)) {
    for (const key of identity.keys) {
      if (isPlainObject(key)) push(textOf(key.path));
    }
  }
  return out;
}

/**
 * 把一个探测节点翻译成候选（或"必须可见的诊断"）。
 *
 * 返回 `null` 表示"与本变量无关/与参数无关"，调用方不渲染它——**不是**把它当成
 * 不可用；只有真正声明了归属或键的节点才会进入列表。
 */
export function cascadeOptionFromNode(args: {
  node: BrowseNode;
  definition: CascadeDefinitionLike | null;
  needs: readonly ObjectNeed[];
}): { option: CascadeOption } | { diagnostic: CascadeDiagnostic } | null {
  const { node, definition, needs } = args;
  // 分支节点不是对象：它只负责下钻（由扫描器入队），不出现在候选里。
  if (node.has_children === true) return null;
  // 归类读声明式载荷（含 `selection_mode=3` 的节点）；可用性判定交给 deriveObjectFill。
  const payload = declarativePayloadOf(node);
  const variableId = textOf(definition?.variable_id);
  if (!nodeBelongsToVariable(payload, variableId)) return null;
  const declared = declaredKeysOf(payload);
  if (declared.length === 0) {
    // 没有载荷也没有键声明：驱动在这里报的是"异常/无法判定"类事实。
    // 它不能成为候选，但**必须可见**（"无法判定"不是"不存在"）。
    const first = (node.diagnostics ?? [])[0];
    if (!first) return null;
    return {
      diagnostic: {
        nodeId: node.id,
        label: node.label ?? node.id,
        code: textOf(first.code),
        message: textOf(first.message),
        severity: typeof first.severity === "number" ? first.severity : 0,
      },
    };
  }
  const fill = deriveObjectFill({ node, definition, needs });
  const base = {
    nodeId: node.id,
    label: node.label ?? node.id,
    kind: textOf(node.kind),
    declared,
  };
  if (!fill.ok) {
    return {
      option: {
        ...base,
        provides: [],
        fill: null,
        blocked: { code: fill.code, message: fill.message },
      },
    };
  }
  return {
    option: {
      ...base,
      provides: fill.fill.resolved,
      fill: fill.fill,
      blocked: null,
    },
  };
}

/** 扫描默认上限：真实设备树通常 3 层、十余页；上限只用于防止异常驱动无限下钻。 */
export const CASCADE_SCAN_LIMITS = { maxParents: 24, maxDepth: 3, maxPagesPerParent: 20 } as const;

export interface CascadeScanRequest {
  endpointId: string;
  definition: CascadeDefinitionLike | null;
  needs: readonly ObjectNeed[];
  fetchImpl?: ObjectProbeFetchLike;
  maxParents?: number;
  maxDepth?: number;
  /** 同一个 parent 的分页上限（异常驱动返回恒定游标时不得无限循环）。 */
  maxPagesPerParent?: number;
}

/**
 * early-stop 判据：所有**必填** key 都已有至少一个可选候选。
 *
 * 为什么可以停：每一级下拉只要求"存在一个能提供该 key 的候选"，后续分支再多
 * 也不会让已找到的候选失效；继续扫描只会把等待时间花在无关子树上。
 *
 * 为什么 `drive_object` 例外：它的身份是 `{number, entry}` 信封，entry 子键不在
 * `ObjectNeed` 里；提前停止可能漏掉"后面才有 entry 取值"的候选。这里保守地
 * 不提前停，等完整扫描后再由 `optionsForKey` 按 subKey 精确过滤。
 */
function allRequiredKeysCovered(
  needs: readonly ObjectNeed[],
  options: readonly CascadeOption[],
): boolean {
  const required = needs.filter((need) => need.required).map((need) => need.kind);
  if (required.length === 0) return false;
  if (required.includes("drive_object")) return false;
  return required.every((kind) =>
    options.some((option) => option.blocked === null && option.provides.includes(kind)),
  );
}

/**
 * 探测设备上的对象并整理成候选（广度优先遍历驱动的 browse 树）。
 *
 * 为什么是"遍历"而不是"猜某个固定分支"：驱动把对象按自己的树组织（通道树 /
 * 主轴树 / 轴树 / 驱动对象树…），前端不认识任何一棵。唯一可靠的做法是从根开始
 * 广度优先，把**每个**叶节点按它自己声明的归属与键归类；分支失败逐条记录，
 * 不让一棵子树拖垮整次探测，也不把失败当成"没有"。
 *
 * 该函数**不抛异常**：失败一律进入 `branches`，由调用方决定怎么展示。
 */
export async function scanObjectCandidates(
  request: CascadeScanRequest,
): Promise<CascadeScan> {
  const maxParents = request.maxParents ?? CASCADE_SCAN_LIMITS.maxParents;
  const maxDepth = request.maxDepth ?? CASCADE_SCAN_LIMITS.maxDepth;
  const maxPagesPerParent =
    request.maxPagesPerParent ?? CASCADE_SCAN_LIMITS.maxPagesPerParent;
  const options: CascadeOption[] = [];
  const diagnostics: CascadeDiagnostic[] = [];
  const branches: CascadeBranch[] = [];
  const visited = new Set<string>();
  const queue: Array<{ parent: string; depth: number }> = [{ parent: "", depth: 0 }];
  const seenNode = new Set<string>();

  scan: while (queue.length && visited.size < maxParents) {
    const current = queue.shift()!;
    if (visited.has(current.parent)) continue;
    visited.add(current.parent);
    let cursor = "";
    let pageIndex = 0;
    do {
      pageIndex += 1;
      let page: { nodes: BrowseNode[]; nextCursor: string | null };
      try {
        page = await fetchObjectProbePage(
          {
            endpointId: request.endpointId,
            parent: current.parent,
            cursor,
          },
          request.fetchImpl,
        );
      } catch (error) {
        branches.push({
          parent: current.parent,
          ok: false,
          code: isObjectProbeError(error) ? error.code : "PROBE_REQUEST_FAILED",
          message: error instanceof Error ? error.message : String(error),
        });
        break;
      }
      for (const node of page.nodes) {
        // 下钻只在有子节点时发生，且深度受限（防异常驱动递归）。
        if (node.has_children === true && current.depth + 1 <= maxDepth) {
          queue.push({ parent: node.id, depth: current.depth + 1 });
        }
        if (seenNode.has(node.id)) continue;
        seenNode.add(node.id);
        const mapped = cascadeOptionFromNode({
          node,
          definition: request.definition,
          needs: request.needs,
        });
        if (!mapped) continue;
        if ("diagnostic" in mapped) diagnostics.push(mapped.diagnostic);
        else options.push(mapped.option);
      }
      // 所有必填 key 都有候选 ⇒ 停止 BFS。**已经发生的失败分支/诊断仍保留在
      // `branches`/`diagnostics` 里**：提前停止只省掉"还没访问的分支"，绝不
      // 抹掉"已经发现的失败"。
      if (allRequiredKeysCovered(request.needs, options)) break scan;
      cursor = page.nextCursor ?? "";
    } while (cursor !== "" && pageIndex < maxPagesPerParent);
  }

  return {
    options,
    diagnostics,
    branches,
    incomplete: branches.some((branch) => !branch.ok),
  };
}

// ---------------------------------------------------------------------------
// 3. 某一级能看到什么
// ---------------------------------------------------------------------------

export interface CascadeLevelOptions {
  /** 可选用（载荷能给出取值）。 */
  selectable: CascadeOption[];
  /** 声明了该键但不可选用（诊断/载荷缺失/无地址映射）：必须可见。 */
  blocked: CascadeOption[];
}

/** 取某一级（某个参数键）的候选：`provides` 里确实有这个键才算。 */
export function optionsForKey(scan: CascadeScan, key: CascadeKey): CascadeLevelOptions {
  const match = (option: CascadeOption) =>
    option.provides.includes(key.key) &&
    (key.subKey ? option.provides.includes(key.subKey) : true);
  return {
    selectable: scan.options.filter((option) => option.blocked === null && match(option)),
    // 不可用候选的 `provides` 恒为空（没有可用取值），因此按"载荷是否声明了该键"归类：
    // 它们必须可见，用来解释"为什么这个对象不能用"。
    blocked: scan.options.filter(
      (option) => option.blocked !== null && option.declared.includes(key.key),
    ),
  };
}

// ---------------------------------------------------------------------------
// 4. 缺参数判定（唯一"为什么不能添加"的口径）
// ---------------------------------------------------------------------------

export type MissingReasonCode =
  /** 端点不可用（未创建/连接配置不可读）：一个请求都没发。 */
  | "probe_unavailable"
  /** 尚未探测（下拉还没展开过）。 */
  | "not_probed"
  /** 探测失败（整次或分支不可达）：结果不完整。 */
  | "probe_failed"
  /** 探测成功但没有对象能提供该参数。 */
  | "no_candidates"
  /** 有候选，只是还没选。 */
  | "not_selected";

export interface MissingParam {
  key: CascadeKey;
  reason: MissingReasonCode;
  /** 可读原因（含失败分支/不可用候选的解释）。 */
  detail: string;
}

export interface CascadeReadiness {
  ok: boolean;
  /** 尚未解析的必填参数（按定义顺序）。 */
  missing: MissingParam[];
  /** 真正"拿不到"的（`not_selected` 之外）：这些必须阻断并给出出路。 */
  unobtainable: MissingParam[];
}

/**
 * 判定"还缺哪些参数、为什么缺"。
 *
 * `scan === null` 表示还没探测过；`gate.available === false` 表示根本不能探测。
 * 两者都不是错误，但都**不得**放行添加：缺参数的草稿本来就过不了定义校验。
 */
export function evaluateCascadeReadiness(args: {
  keys: readonly CascadeKey[];
  draft: NckFormDraft;
  scan: CascadeScan | null;
  gate: ObjectProbeGate;
}): CascadeReadiness {
  const missing: MissingParam[] = [];
  for (const key of args.keys) {
    if (isKeyResolved(key, args.draft)) continue;
    missing.push({ key, ...describeMissing(key, args.scan, args.gate) });
  }
  return {
    ok: missing.length === 0,
    missing,
    // "尚未探测/尚未选择"不是"拿不到"：它只是流程还没走完，不该给出死路指引。
    unobtainable: missing.filter(
      (item) =>
        item.reason === "probe_unavailable" ||
        item.reason === "probe_failed" ||
        item.reason === "no_candidates",
    ),
  };
}

function describeMissing(
  key: CascadeKey,
  scan: CascadeScan | null,
  gate: ObjectProbeGate,
): { reason: MissingReasonCode; detail: string } {
  if (!gate.available) {
    return {
      reason: "probe_unavailable",
      detail: gate.reason,
    };
  }
  if (!scan) {
    return {
      reason: "not_probed",
      detail: `尚未探测实际对象：展开「${key.label}」下拉时会向设备查询`,
    };
  }
  const level = optionsForKey(scan, key);
  if (level.selectable.length > 0) {
    return {
      reason: "not_selected",
      detail: `设备探测到 ${level.selectable.length} 个可提供该参数的对象，请在下拉中选择`,
    };
  }
  const failed = scan.branches.filter((branch) => !branch.ok);
  const blockedText = level.blocked.length
    ? `；探测到的 ${level.blocked.length} 个对象不能提供该参数（${level.blocked
        .map((option) => option.blocked?.code ?? "")
        .filter((code) => code !== "")
        .join(" / ")}）`
    : "";
  if (failed.length) {
    return {
      reason: "probe_failed",
      detail:
        `探测未完成：${failed.length} 个分支不可达（${failed
          .map((branch) => `${branch.parent || "（根）"}：${branch.code}`)
          .join(" / ")}）` + blockedText,
    };
  }
  return {
    reason: "no_candidates",
    detail: `探测成功，但设备没有返回能提供该参数的对象${blockedText}`,
  };
}

/** 缺参数文案（两种口径必须**文案不同**：探测失败 ≠ 设备没有）。 */
export function describeMissingParam(item: MissingParam): string {
  const name = item.key.label === item.key.key ? item.key.key : `${item.key.label}（${item.key.key}）`;
  switch (item.reason) {
    case "probe_unavailable":
      return `缺少参数 ${name}，且无法从设备探测获得：${item.detail}`;
    case "probe_failed":
      return `缺少参数 ${name}，且无法从设备探测获得：${item.detail}（探测失败，不代表设备上没有该对象）`;
    case "no_candidates":
      return `缺少参数 ${name}，且无法从设备探测获得：${item.detail}`;
    case "not_probed":
      return `缺少参数 ${name}：${item.detail}`;
    case "not_selected":
      return `缺少参数 ${name}：${item.detail}`;
  }
}

/**
 * 阻断时的「下一步怎么做」（可操作指引）。
 *
 * 只说"缺参数"对用户是死路：必须给出两条真实可行的出路——
 * ① 到端点/采集页（那里端点已连接）逐级选择实际对象；
 * ② 需要直接填底层参数时切「高级原生配置」。
 */
export function cascadeGuidance(args: {
  gate: ObjectProbeGate;
  nativeAvailable: boolean;
}): string {
  const native = args.nativeAvailable
    ? "如需在创建阶段就直接配置底层参数，可切换到「高级原生配置」（nck_native）。"
    : "本页未提供「高级原生配置」入口：请在端点描述符可用后重试。";
  if (!args.gate.available) {
    return (
      "语义点位需要先连接设备才能探测实际对象：请先完成设备创建与连接，再到「采集」页配置点位" +
      "（那里端点已连接，可逐级选择实际对象）。" +
      native
    );
  }
  return (
    "请先确认端点连接与设备状态后重试（展开下拉会重新探测）；若设备确实不提供该对象，" +
    "该变量在当前机床无法可靠配置。" +
    native
  );
}
