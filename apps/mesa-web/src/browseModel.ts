// Browse → Selection 通用映射（P0-2：Descriptor 声明 Browse 就必须有产品入口）。
//
// 后端 POST /api/v1/endpoints/{id}/browse 返回节点：
// {id, label, kind, data_type, access, has_children, binding_json}。
// binding_json 是“ResourceSelection 片段”，两种现实形态：
// - NCK 叶：{"resource_id": "variable", "parameters": {...}}（显式 resource）
// - OPC UA 叶：{"node_id": ..., "data_type": ...}（裸参数，resource 即 kind）
// 本模块只理解这两种信封形态，不理解任何协议语义；无 binding 的分支节点
// 只可“进入”下钻，不可生成选择。非法 JSON / 空 binding / 空信封 "{}" 一律返回 null。
export interface BrowseNode {
  id: string;
  label?: string;
  kind?: string;
  data_type?: string;
  access?: string;
  has_children?: boolean;
  binding_json?: string;
}

export interface BrowseSelection {
  resource_id: string;
  parameters: Record<string, unknown>;
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
