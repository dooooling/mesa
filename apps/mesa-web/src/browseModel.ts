// Browse → Selection 通用映射（P0-2：Descriptor 声明 Browse 就必须有产品入口）。
//
// 后端 POST /api/v1/endpoints/{id}/browse 返回节点：
// {id, label, kind, data_type, access, has_children, binding_json}。
// binding_json 是“Selection 片段”，三种现实形态：
// - 通用资源叶：{"resource_id": "variable", "parameters": {...}}（显式 resource）
// - OPC UA 叶：{"node_id": ..., "data_type": ...}（裸参数，resource 即 kind）
// - NCK 语义叶：{"kind":"nck_variable", "parameters":{...}, "identity":{...}}
//
// 本模块只理解信封形态，不理解任何协议语义。**identity 只作为可选元数据透出，
// 绝不折进 parameters**：parameters 会被持久化，identity 则不是点位参数
//（Core 的 PointSelection 是 deny_unknown_fields，混入即整体拒绝）。
//
// A16（无 identity 的语义叶不可选）：语义叶必须带 identity 对象，否则返回 null，
// 使 Browse 的“选用”入口自然关闭；非 NCK 叶完全不要求 identity（A44）。
export interface BrowseNode {
  id: string;
  label?: string;
  kind?: string;
  data_type?: string;
  access?: string;
  has_children?: boolean;
  binding_json?: string;
}

/** NCK 身份块（来自 Driver 的 semantic binding；本模块只读展示）。 */
export interface BrowseIdentity {
  template?: string;
  keys?: Array<{ path?: string; required?: boolean }>;
  /** Driver 的预览 Key；**仅初始展示**，不得作为提交来源。 */
  preview_point_key?: string | null;
}

/** NCK 点位层 kind（与通用 resource_id 不同层级）。 */
export type BrowsePointKind = "nck_variable" | "nck_native";

export interface BrowseSelection {
  resource_id: string;
  parameters: Record<string, unknown>;
  /** 仅 NCK 语义叶存在；非语义叶必须为 undefined（A44）。 */
  point_kind?: BrowsePointKind;
  /** 仅 NCK 语义叶存在；**不进入 parameters**。 */
  identity?: BrowseIdentity;
}

function isSemanticKind(value: unknown): value is BrowsePointKind {
  return value === "nck_variable" || value === "nck_native";
}

function parseIdentity(value: unknown): BrowseIdentity | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const obj = value as Record<string, unknown>;
  const out: BrowseIdentity = {};
  if (typeof obj.template === "string") out.template = obj.template;
  if (Array.isArray(obj.keys)) {
    out.keys = obj.keys
      .filter((k): k is Record<string, unknown> => !!k && typeof k === "object" && !Array.isArray(k))
      .map((k) => ({
        path: typeof k.path === "string" ? k.path : undefined,
        required: typeof k.required === "boolean" ? k.required : undefined,
      }));
  }
  if (obj.preview_point_key === null) out.preview_point_key = null;
  else if (typeof obj.preview_point_key === "string") {
    out.preview_point_key = obj.preview_point_key;
  }
  return out;
}

/** 节点是否可选用（有可用 binding）；否则只能下钻或停留。 */
export function selectableFromNode(node: BrowseNode): BrowseSelection | null {
  const raw = (node.binding_json ?? "").trim();
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const obj = parsed as Record<string, unknown>;
  // 空信封（如 NCK 非叶节点的 "{}"）永远不可选：无 resource 信息，
  // 若回落到 node.kind 会把 area/block/channel 等分支误判成可选资源。
  if (Object.keys(obj).length === 0) return null;

  // 语义叶：kind 是 Point Kind，不是通用 resource。
  if (isSemanticKind(obj.kind)) {
    const identity = parseIdentity(obj.identity);
    // A16：无 identity（或形态非法）的语义叶不可选，入口关闭。
    if (!identity) return null;
    if (!obj.parameters || typeof obj.parameters !== "object" || Array.isArray(obj.parameters)) {
      return null;
    }
    return {
      resource_id:
        typeof obj.resource_id === "string" && obj.resource_id.trim()
          ? obj.resource_id.trim()
          : obj.kind,
      parameters: obj.parameters as Record<string, unknown>,
      point_kind: obj.kind,
      identity,
    };
  }

  // 显式 resource_id 优先，否则回落到 node.kind（OPC UA 口径）。
  const resourceId = typeof obj.resource_id === "string" && obj.resource_id.trim()
    ? obj.resource_id.trim()
    : typeof node.kind === "string" && node.kind.trim()
      ? node.kind.trim()
      : "";
  if (!resourceId) return null;
  // 显式 parameters 优先，否则把信封其余字段整体当参数（OPC UA 口径）。
  let parameters: Record<string, unknown>;
  if (obj.parameters && typeof obj.parameters === "object" && !Array.isArray(obj.parameters)) {
    parameters = obj.parameters as Record<string, unknown>;
  } else {
    const { resource_id: _drop, ...rest } = obj;
    parameters = rest;
  }
  // 非语义叶绝不附加 identity/point_kind：既有调用方看到的对象形状逐字不变（A44）。
  return { resource_id: resourceId, parameters };
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
