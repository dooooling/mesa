// Browse → Selection 通用映射（P0-2：Descriptor 声明 Browse 就必须有产品入口）。
//
// 后端 POST /api/v1/endpoints/{id}/browse 返回节点，wire 上**只有 7 个字段**：
// {id, label, kind, data_type, access, has_children, binding_json}
//（proto/driver.proto:340-349；core-api 逐字段透传，crates/core-api/src/lib.rs:905-918）。
// 注意：wire 上没有 `selectable` / `diagnostics` 之类的结构化准入字段。
//
// 因此本模块只做两件**通用**的事：
//   1. `parseBrowseEnvelope`：把 Driver 给的 binding_json 解析成对象；
//   2. `selectableFromNode`：**通用资源叶**的映射（显式 resource_id 口径；
//      裸参数口径回落到 node.kind）。
//
// 本模块不认识任何厂商/协议的 point kind，也不解析 identity 之类的语义块：
// 只要信封里出现 `kind` 字段，就说明这是「由驱动语义层解释的载荷」，通用层
// 一律不猜资源（返回 null），交由调用方注入的 `BrowseLeafPolicy` 决定
//（见 browseLeafPolicy.ts；NCK 实现见 nckBrowsePolicy.ts）。
// 这样「可选 / 可采集 / 属于什么类别」的判断不再写死在通用 Browse 里。
//
// 信封现实形态（只列形态，不做判断）：
// - 显式资源叶：{"resource_id": "variable", "parameters": {...}}
// - 裸参数叶（OPC UA 口径）：{"node_id": ..., "data_type": ...}（resource 即 node.kind）
// - 语义叶：{"kind": <驱动 point kind>, "parameters": {...}, ...语义附加块}
export interface BrowseNode {
  id: string;
  label?: string;
  kind?: string;
  data_type?: string;
  access?: string;
  has_children?: boolean;
  binding_json?: string;
}

/** 通用资源叶的选用结果（语义叶由 BrowseLeafPolicy 给出自己的载荷类型）。 */
export interface BrowseSelection {
  resource_id: string;
  parameters: Record<string, unknown>;
}

/**
 * 解析节点信封（binding_json）。
 *
 * 空串 / 空白 / 非法 JSON / 非对象一律返回 null：调用方据此关闭「选用」入口
 *（分支节点的 `"{}"` 是有效对象，由调用方再判断内容是否为空）。
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
