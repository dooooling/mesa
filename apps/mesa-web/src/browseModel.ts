// Browse → Selection 通用映射（P0-2：Descriptor 声明 Browse 就必须有产品入口）。
//
// 后端 POST /api/v1/endpoints/{id}/browse 返回节点。wire 字段分两层：
//
// ① **IPC 1.7 及之前**的 7 个字段：
//    {id, label, kind, data_type, access, has_children, binding_json}
// ② **IPC 1.8 选择契约**（additive，proto/driver.proto `BrowseNode` 8/9/10）：
//    {selection_mode, selection_payload_json, diagnostics}
//    ——「能不能选、为什么不能选、载荷是什么」由**驱动**给出权威结论。
//
// 本模块只做**消费**，不做推断：
//   1. `parseBrowseEnvelope`：把 `binding_json` 解析成对象（旧通路）；
//   2. `selectableFromNode`：**未声明选择契约的旧驱动**的通用资源叶规则；
//   3. `evaluateBrowseNode`：把 wire 的选择模式 + 载荷 + 诊断翻译成
//      「可选（资源/语义）/ 阻断（可见原因）/ 无入口」三态。
//
// fail-closed 口径（缺一即为回归）：
// - 未知/未来枚举值 ⇒ 阻断（不得当成"未声明"）；
// - 声明了模式但载荷缺失/畸形 ⇒ 阻断（不得回退 `binding_json`）；
// - 旧驱动（未声明模式）遇到**语义载荷** ⇒ 阻断（没有权威准入结论就不许选）；
// - 模式声明为不可选 ⇒ 永不给入口，哪怕载荷看起来合法。
//
// 本模块不认识任何厂商/协议的 point kind，也不解析 identity 之类的语义块：
// 语义载荷原样交给调用方注入的 `BrowseSemanticHandler`（见 browseSemanticHandler.ts；
// NCK 实现见 nckBrowseSemantic.ts）。
export interface BrowseNode {
  id: string;
  label?: string;
  kind?: string;
  data_type?: string;
  access?: string;
  has_children?: boolean;
  binding_json?: string;
  /**
   * IPC 1.8 选择模式（数值，见 [`BROWSE_SELECTION_MODE`]）。
   * 缺席与 `0` 等价：旧驱动不填 ⇒ 走 IPC 1.7 的既有通用规则。
   */
  selection_mode?: number;
  /** IPC 1.8 不透明选择载荷（驱动私有 JSON；畸形必须阻断）。 */
  selection_payload_json?: string;
  /** IPC 1.8 结构化诊断（稳定码 + 文案 + 级别）。 */
  diagnostics?: readonly BrowseDiagnostic[];
}

/** 选择模式取值（与 `proto/driver.proto` 的 `BrowseSelectionMode` 逐字对应）。 */
export const BROWSE_SELECTION_MODE = {
  /** 未声明：旧驱动（IPC < 1.8）。**不是**「不可选」。 */
  UNSPECIFIED: 0,
  /** 通用资源叶：载荷是 `{resource_id, parameters}`。 */
  RESOURCE: 1,
  /** 语义载荷叶：载荷只对其驱动自己的语义层有意义。 */
  SEMANTIC: 2,
  /** 明确不可选（分支/诊断/被驱动准入拒绝）。 */
  NOT_SELECTABLE: 3,
} as const;

/** 选择模式（`unknown` = wire 给了本端不认识的枚举值）。 */
export type BrowseSelectionModeName =
  | "unspecified"
  | "resource"
  | "semantic"
  | "not_selectable"
  | "unknown";

/** 诊断级别取值（与 proto 的 `BrowseDiagnosticSeverity` 逐字对应）。 */
const DIAGNOSTIC_SEVERITY = { INFO: 1, WARNING: 2, ERROR: 3 } as const;

export interface BrowseDiagnostic {
  readonly code: string;
  readonly message: string;
  readonly severity?: number;
}

/** 可见阻断（红色）。`testId` 只用于定位，不参与判定。 */
export interface BrowseBlock {
  readonly code: string;
  readonly message: string;
  readonly testId: string;
}

/** 展示性提示（信息/警告）：**不改变可选性**。 */
export interface BrowseNote {
  readonly code: string;
  readonly message: string;
  readonly tone: "muted" | "warn";
}

/** `evaluateBrowseNode` 的结果：可选性、载荷与可见阻断的唯一出口。 */
export interface BrowseLeafEvaluation {
  readonly mode: BrowseSelectionModeName;
  /** 通用资源叶的选中结果（RESOURCE 且载荷合法时非空）。 */
  readonly resource: BrowseSelection | null;
  /** 语义叶的不透明载荷（SEMANTIC 且载荷可解析时非空）。 */
  readonly semanticPayload: Record<string, unknown> | null;
  /** 必须渲染出来的阻断（含驱动给的 ERROR 诊断与本端的契约违规）。 */
  readonly blocks: readonly BrowseBlock[];
  /** 只渲染不判定的提示（驱动给的 INFO/WARNING 诊断）。 */
  readonly notes: readonly BrowseNote[];
}

/**
 * 本端（消费侧）契约违规码。
 *
 * 与**驱动侧**诊断码（如 `NCK_*` / `DEFINITION_SCOPE_NOT_COLLECTABLE`）分开命名：
 * 前者说明"wire 上的东西不合契约"，后者说明"驱动为什么不让选"。
 * 两者都只用于展示与定位，不构成放行依据。
 */
export const BROWSE_CONTRACT_CODES = {
  MODE_UNKNOWN: "BROWSE_SELECTION_MODE_UNKNOWN",
  PAYLOAD_MISSING: "BROWSE_PAYLOAD_MISSING",
  PAYLOAD_MALFORMED: "BROWSE_PAYLOAD_MALFORMED",
  RESOURCE_PAYLOAD_INVALID: "BROWSE_RESOURCE_PAYLOAD_INVALID",
  SELECTION_UNSPECIFIED: "BROWSE_SELECTION_UNSPECIFIED",
  SEMANTIC_HANDLER_MISSING: "BROWSE_SEMANTIC_HANDLER_MISSING",
} as const;

/** 通用资源叶的选用结果（语义叶由注入的 `BrowseSemanticHandler` 给出载荷类型）。 */
export interface BrowseSelection {
  resource_id: string;
  parameters: Record<string, unknown>;
}

/**
 * 解析节点信封（binding_json）。
 *
 * 空串 / 空白 / 非法 JSON / 非对象一律返回 null：调用方据此关闭「选用」入口
 *（分支节点的 `"{}"` 是有效对象，由调用方再判断内容是否为空）。
 *
 * 信封现实形态（只列形态，不做判断；新驱动改用 `selection_payload_json`）：
 * - 显式资源叶：{"resource_id": "variable", "parameters": {...}}
 * - 裸参数叶（OPC UA 口径）：{"node_id": ..., "data_type": ...}（resource 即 node.kind）
 * - 语义叶：{"kind": <驱动 point kind>, "parameters": {...}, ...语义附加块}
 */
export function parseBrowseEnvelope(node: BrowseNode): Record<string, unknown> | null {
  const raw = (node.binding_json ?? "").trim();
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  return parsed as Record<string, unknown>;
}

/**
 * 信封是否声明了「需要驱动语义层解释的 point kind 载荷」。
 *
 * 判据只有一个形态事实：`kind` 是非空字符串。这不解释 kind 的取值，也不推断
 * 它属于哪个驱动；通用层只需知道「这里有语义载荷，我不猜」。
 */
export function declaresOpaquePointKind(envelope: Record<string, unknown> | null): boolean {
  return !!envelope && typeof envelope.kind === "string" && envelope.kind.trim() !== "";
}

/** 节点是否可作为**通用资源叶**选用；语义载荷与无 binding 的节点返回 null。 */
export function selectableFromNode(node: BrowseNode): BrowseSelection | null {
  const obj = parseBrowseEnvelope(node);
  if (!obj) return null;
  // 空信封（分支节点常见 `"{}"`）永远不可选：无 resource 信息，
  // 若回落到 node.kind 会把 area/block/channel 等分支误判成可选资源。
  if (Object.keys(obj).length === 0) return null;
  // 声明了 point kind ⇒ 语义载荷：通用层不解释、不猜 resource（fail-closed）。
  if (declaresOpaquePointKind(obj)) return null;

  // 显式 resource_id 优先，否则回落到 node.kind（裸参数口径）。
  const resourceId = typeof obj.resource_id === "string" && obj.resource_id.trim()
    ? obj.resource_id.trim()
    : typeof node.kind === "string" && node.kind.trim()
      ? node.kind.trim()
      : "";
  if (!resourceId) return null;
  // 显式 parameters 优先，否则把信封其余字段整体当参数（裸参数口径）。
  let parameters: Record<string, unknown>;
  if (obj.parameters && typeof obj.parameters === "object" && !Array.isArray(obj.parameters)) {
    parameters = obj.parameters as Record<string, unknown>;
  } else {
    const { resource_id: _drop, ...rest } = obj;
    parameters = rest;
  }
  // 结果形状恒为两字段：附加语义块（identity 等）绝不混入 parameters
  //（parameters 会被持久化，Core 的 PointSelection 是 deny_unknown_fields）。
  return { resource_id: resourceId, parameters };
}

/**
 * 解码 wire 的选择模式。
 *
 * 缺席（旧驱动）与 `0` 等价；**未知数值不降级成 `unspecified`**——那样会让
 * 未来驱动的新语义被静默当成"旧协议"处理，正是 fail-open。
 */
export function decodeBrowseSelectionMode(node: BrowseNode): BrowseSelectionModeName {
  const raw = node.selection_mode;
  if (raw === undefined || raw === null) return "unspecified";
  switch (raw) {
    case BROWSE_SELECTION_MODE.UNSPECIFIED:
      return "unspecified";
    case BROWSE_SELECTION_MODE.RESOURCE:
      return "resource";
    case BROWSE_SELECTION_MODE.SEMANTIC:
      return "semantic";
    case BROWSE_SELECTION_MODE.NOT_SELECTABLE:
      return "not_selectable";
    default:
      return "unknown";
  }
}

/** 诊断级别 → 展示语义。未知级别按最保守（阻断样式）处理，但**不**授予权限。 */
function diagnosticTone(severity: number | undefined): "muted" | "warn" | "error" {
  switch (severity) {
    case DIAGNOSTIC_SEVERITY.INFO:
      return "muted";
    case DIAGNOSTIC_SEVERITY.WARNING:
      return "warn";
    default:
      // 含 ERROR、UNSPECIFIED(0) 与未来未知值：一律按错误展示。
      return "error";
  }
}

interface PayloadParse {
  ok: boolean;
  value: Record<string, unknown> | null;
  code: string;
  message: string;
}

/**
 * 解析 `selection_payload_json`。
 *
 * 空载荷与畸形载荷都算契约违规：驱动**声明了**选择模式就必须给出载荷，
 * 消费端不得回退 `binding_json`（那会绕过驱动的准入结论）。
 */
function parseSelectionPayload(raw: string | undefined): PayloadParse {
  const text = (raw ?? "").trim();
  if (!text) {
    return {
      ok: false,
      value: null,
      code: BROWSE_CONTRACT_CODES.PAYLOAD_MISSING,
      message: "驱动声明了选择模式但没有给出选择载荷：不可选用",
    };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return {
      ok: false,
      value: null,
      code: BROWSE_CONTRACT_CODES.PAYLOAD_MALFORMED,
      message: "驱动给出的选择载荷不是合法 JSON：不可选用",
    };
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return {
      ok: false,
      value: null,
      code: BROWSE_CONTRACT_CODES.PAYLOAD_MALFORMED,
      message: "驱动给出的选择载荷不是 JSON 对象：不可选用",
    };
  }
  return { ok: true, value: parsed as Record<string, unknown>, code: "", message: "" };
}

function block(code: string, message: string, testId = "browse-leaf-block"): BrowseBlock {
  return { code, message, testId };
}

/**
 * 把一个 Browse 节点翻译成「可选 / 阻断 / 无入口」。
 *
 * 这是本模块的**唯一判定入口**：通用组件只渲染它的结果，不再自己 if 任何
 * 厂商字面量。四种模式的语义见 [`BROWSE_SELECTION_MODE`]。
 *
 * 判定顺序（不可调换）：先看驱动声明的模式，再看载荷是否合契约，最后才谈
 * 「有什么入口」。任何一步不成立都**只**能产生阻断，不得降级成另一个模式。
 */
export function evaluateBrowseNode(node: BrowseNode): BrowseLeafEvaluation {
  const notes: BrowseNote[] = [];
  const blocks: BrowseBlock[] = [];
  // 驱动的结构化诊断：ERROR 进阻断，其余只展示。级别永远不授予权限。
  for (const d of node.diagnostics ?? []) {
    const code = typeof d?.code === "string" ? d.code : "";
    const message = typeof d?.message === "string" ? d.message : "";
    const tone = diagnosticTone(d?.severity);
    if (tone === "error") {
      blocks.push(block(code || "BROWSE_DIAGNOSTIC", message || "驱动未给出原因", "browse-selection-block"));
    } else {
      notes.push({ code, message, tone });
    }
  }

  const mode = decodeBrowseSelectionMode(node);
  const empty = (): BrowseLeafEvaluation => ({
    mode,
    resource: null,
    semanticPayload: null,
    blocks,
    notes,
  });

  if (mode === "unknown") {
    blocks.push(
      block(
        BROWSE_CONTRACT_CODES.MODE_UNKNOWN,
        `驱动声明的选择模式未知（selection_mode=${String(node.selection_mode)}）：不可选用`,
      ),
    );
    return empty();
  }

  if (mode === "not_selectable") {
    // 可选性已被驱动否定：载荷即使合法也不构成许可。
    return empty();
  }

  if (mode === "resource") {
    const payload = parseSelectionPayload(node.selection_payload_json);
    if (!payload.ok || !payload.value) {
      blocks.push(block(payload.code, payload.message));
      return empty();
    }
    const resourceId =
      typeof payload.value.resource_id === "string" ? payload.value.resource_id.trim() : "";
    const parameters = payload.value.parameters;
    if (!resourceId || !parameters || typeof parameters !== "object" || Array.isArray(parameters)) {
      blocks.push(
        block(
          BROWSE_CONTRACT_CODES.RESOURCE_PAYLOAD_INVALID,
          "资源载荷必须给出 resource_id 与 parameters 对象：不可选用（消费端不从节点字段推测）",
        ),
      );
      return empty();
    }
    return {
      mode,
      resource: { resource_id: resourceId, parameters: parameters as Record<string, unknown> },
      semanticPayload: null,
      blocks,
      notes,
    };
  }

  if (mode === "semantic") {
    const payload = parseSelectionPayload(node.selection_payload_json);
    if (!payload.ok || !payload.value) {
      blocks.push(block(payload.code, payload.message));
      return empty();
    }
    return {
      mode,
      resource: null,
      semanticPayload: payload.value,
      blocks,
      notes,
    };
  }

  // ---- unspecified：旧驱动（IPC < 1.8）------------------------------------
  // 普通资源浏览必须继续可用（逐字沿用 IPC 1.7 的通用规则）。
  const envelope = parseBrowseEnvelope(node);
  if (declaresOpaquePointKind(envelope)) {
    // 语义载荷 + 无权威准入结论 ⇒ 不可选用（fail-closed），且必须可见。
    blocks.push(
      block(
        BROWSE_CONTRACT_CODES.SELECTION_UNSPECIFIED,
        "驱动未声明选择模式（IPC < 1.8）：语义载荷没有权威准入结论，不可选用",
      ),
    );
    return empty();
  }
  const resource = selectableFromNode(node);
  if (resource) {
    return { mode, resource, semanticPayload: null, blocks, notes };
  }
  // 既无通用资源口径、也不是空信封 ⇒ 载荷畸形，必须可见（不得静默）。
  const rawBinding = (node.binding_json ?? "").trim();
  if (rawBinding && !envelope) {
    blocks.push(
      block(BROWSE_CONTRACT_CODES.PAYLOAD_MALFORMED, "节点载荷不是合法 JSON 对象：不可选用"),
    );
  }
  return empty();
}

/** Browse 请求体（与后端 BrowseReq 同形；limit 服务端钳制 1..1000）。 */
export function buildBrowseRequest(args: {
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
