// PR27 Device-first：Device≈Endpoint 分离的纯模型逻辑。
//
// 设计意图：Web 层最后残留的错误模型是“新增设备同时创建同 ID 的
// Device + Endpoint”（device_id = endpoint_id = id）。本模块把正确
// 模型收敛为可单测的纯函数：Device 与 Endpoint 是两个独立对象，
// Endpoint 经 device_id 归属 Device；页面组件只负责渲染与请求。
// 本模块不理解任何协议语义，不触及 Descriptor / Core / Driver contract。
export interface Device {
  id: string;
  name: string;
}

export interface EndpointSummary {
  id: string;
  name?: string;
  driver_id: string;
  device_id?: string;
  state?: string;
}

/** 运行态是否视为在线（RUNNING / CONNECTING / RECONNECTING）。 */
export function isRunningState(state?: string): boolean {
  return ["RUNNING", "CONNECTING", "RECONNECTING"].includes((state ?? "").toUpperCase());
}

/**
 * 按 device_id 归组 Endpoint。device_id 缺失的归入 "" 组，
 * 由页面显式渲染为“未归属”，不得默默并入任一 Device。
 */
export function groupEndpointsByDevice(
  endpoints: EndpointSummary[],
): Map<string, EndpointSummary[]> {
  const groups = new Map<string, EndpointSummary[]>();
  for (const ep of endpoints) {
    const key = ep.device_id ?? "";
    const list = groups.get(key);
    if (list) list.push(ep);
    else groups.set(key, [ep]);
  }
  return groups;
}

/**
 * 计数分离：Device 数量只数 /devices，Endpoint 数量只数 /endpoints。
 * PR27 验收：2 个 Device + 4 个 Endpoint 必须显示为 2 / 4，不得混算。
 */
export function deviceCounts(
  devices: Device[],
  endpoints: EndpointSummary[],
): { deviceCount: number; endpointCount: number } {
  return { deviceCount: devices.length, endpointCount: endpoints.length };
}

/**
 * Endpoint ID 建议值：`{driver_id}-{随机}`。调用方不得把 device id
 * 透传进来复用——Device 与 Endpoint 的 id 命名空间各自独立。
 */
export function suggestEndpointId(driverId: string): string {
  const rand = Math.random().toString(36).slice(2, 8) || "x";
  return `${driverId || "endpoint"}-${rand}`;
}

/**
 * 清洗连接表单值：丢弃 undefined / null / 空字符串（选填字段未填
 * 不得发空串干扰后端校验）；false / 0 为合法值，必须保留。
 */
export function cleanConnection(
  values: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(values)) {
    if (v === undefined || v === null || v === "") continue;
    out[k] = v;
  }
  return out;
}

export interface EndpointCreatePayload {
  id: string;
  name: string;
  device_id: string;
  driver_id: string;
  connection: Record<string, unknown>;
}

/**
 * 构造创建 Endpoint 的请求体。device_id 只有唯一下发入口（参数），
 * 调用方没有覆盖手段：Add Connection 必须作用于当前 Device。
 * driver_id 仅在创建时写入；修改走 UpdateEndpointBody（无该字段）。
 */
export function buildEndpointCreatePayload(args: {
  deviceId: string;
  driverId: string;
  name: string;
  connection: Record<string, unknown>;
  id?: string;
}): EndpointCreatePayload {
  const name = args.name.trim();
  const id = args.id?.trim() || suggestEndpointId(args.driverId);
  return {
    id,
    name: name || id,
    device_id: args.deviceId,
    driver_id: args.driverId,
    connection: cleanConnection(args.connection),
  };
}

/** 修改 Endpoint 的请求体（PR25 形状：driver_id 创建后不可变，无该字段）。 */
export interface UpdateEndpointBody {
  name: string;
  device_id: string;
  connection: Record<string, unknown>;
}

export function buildEndpointUpdatePayload(args: {
  name: string;
  deviceId: string;
  connection: Record<string, unknown>;
}): UpdateEndpointBody {
  return {
    name: args.name.trim(),
    device_id: args.deviceId,
    connection: cleanConnection(args.connection),
  };
}

/**
 * 客户端护栏：修改请求体中一旦出现 driver_id 即视为改 driver 企图，
 * 直接拦截（后端以 deny_unknown_fields 拒绝为准，这里只做提前提示）。
 */
export function isDriverChangeAttempt(body: Record<string, unknown>): boolean {
  return "driver_id" in body;
}

/**
 * 删除 Device 的前端预检：仍有归属 Endpoint 时提示先清 Endpoint。
 * 最终裁决以后端 RESTRICT（Conflict）为准，本函数只做提前提示，
 * 不得替代后端的拒绝逻辑。
 */
export function canDeleteDevice(
  deviceId: string,
  endpoints: EndpointSummary[],
): { ok: boolean; reason?: string } {
  const n = endpoints.filter((e) => e.device_id === deviceId).length;
  if (n > 0) {
    return { ok: false, reason: `该设备下仍有 ${n} 个连接，请先删除关联 Endpoint` };
  }
  return { ok: true };
}
