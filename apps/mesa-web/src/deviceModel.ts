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

// ---------------------------------------------------------------------------
// Monitor 语义（P0-3）：Device → Endpoint → Point 三级，文案不得把 Endpoint
// 叫成“设备”。由 endpoint 归属 device_id 反查设备名；归属缺失时如实展示。
// ---------------------------------------------------------------------------

export interface EndpointContext {
  endpointId: string;
  endpointName: string;
  deviceId: string;
  deviceName: string;
}

/**
 * 按 endpoint id 建上下文索引。deviceName 取 Device.name；Device 缺失
 * （如已被删）时回落显示 device_id，endpoint 名缺失回落 id——不编造归属。
 */
export function resolveEndpointContexts(
  endpoints: Array<{ id: string; name?: string; device_id?: string }>,
  devices: Array<{ id: string; name: string }>,
): Map<string, EndpointContext> {
  const byId = new Map(devices.map((d) => [d.id, d.name] as const));
  const out = new Map<string, EndpointContext>();
  for (const ep of endpoints) {
    const deviceId = ep.device_id ?? "";
    out.set(ep.id, {
      endpointId: ep.id,
      endpointName: ep.name ?? ep.id,
      deviceId,
      deviceName: (deviceId && byId.get(deviceId)) || deviceId || "—",
    });
  }
  return out;
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

// ---------------------------------------------------------------------------
// Acquisition Task Set（P0：Web 单任务编辑器不得破坏多 Task）
//
// 背景：PUT /tasks/{endpoint} 是全量替换。旧 Web 只回显 tasks[0]、保存时
// 只发 [{id:"t1"}]，会把 task-b/task-c 等其它任务静默删掉。正确语义：
// Web 只编辑 canonical（mesa.resources.v1）任务，其余任务原样保留。
// ---------------------------------------------------------------------------

/** Web 可理解的 canonical 点位选择形态（与 ResourcePickerAntd 产出一致）。 */
export interface ResourceSelection {
  resource_id: string;
  parameters: Record<string, unknown>;
  outputs: Array<{ output: string; point_key: string }>;
}

export const CANONICAL_RESOURCES_KIND = "mesa.resources.v1";

/** 服务端任务形状（Web 只读 id/mode/interval/binding.kind，不解释其它 binding）。 */
export interface AcquisitionTaskShape {
  id: string;
  mode: string;
  interval_ms?: number | null;
  binding: {
    kind: string;
    config?: unknown;
  };
}

/**
 * 拆分任务集：第一个 canonical 任务归 Web 编辑，其余全部归保留。
 * 无 canonical 时 editable 为 null（保存即新增），preserved 为全集。
 */
export function splitAcquisitionTasks(tasks: AcquisitionTaskShape[]): {
  editable: AcquisitionTaskShape | null;
  preserved: AcquisitionTaskShape[];
} {
  const idx = tasks.findIndex((t) => t.binding.kind === CANONICAL_RESOURCES_KIND);
  if (idx < 0) return { editable: null, preserved: [...tasks] };
  return {
    editable: tasks[idx],
    preserved: tasks.filter((_, i) => i !== idx),
  };
}

/** 从 canonical 任务的 config 里提取已选（非 canonical 形态返回空数组）。 */
export function selectionsOf(task: AcquisitionTaskShape | null): ResourceSelection[] {
  if (!task || task.binding.kind !== CANONICAL_RESOURCES_KIND) return [];
  const config = task.binding.config as { selections?: ResourceSelection[] } | undefined;
  return config?.selections ?? [];
}

/**
 * 点位编辑器任务快照加载状态（P0 数据安全：fail-closed 门）。
 * - idle：尚未加载；loading：请求飞行中；ready：快照已就绪可安全合并；
 * - error：加载失败，此时服务端任务集未知，绝不能把 [] 当作“无任务”去 PUT。
 */
export type TaskSnapshotState = "idle" | "loading" | "ready" | "error";

/**
 * 快照就绪判定：只有快照为 ready 且归属当前 Endpoint 时才允许保存。
 * pending（idle/loading）与失败（error）一律不可 PUT，避免用空快照
 * mergeAcquisitionTasks([], ...) 覆盖掉服务端已有 task-a/b/c。
 */
export function isTaskSnapshotReady(
  state: TaskSnapshotState,
  loadedEndpointId: string | null,
  currentEndpointId: string,
): boolean {
  return state === "ready" && loadedEndpointId === currentEndpointId;
}

/**
 * 合并回写：用编辑结果更新 canonical 任务，其余任务逐字保留。
 * - 沿用已存在的 canonical id（不增殖 id；无则用占位 t1，冲突时 t1-2/t1-3…避让）；
 * - 沿用已存在的 mode（外来 subscribe 任务不得被编辑器默默翻成 poll）；
 *   poll 任务写编辑器周期，非 poll 任务保留原周期。
 */
export function mergeAcquisitionTasks(
  existing: AcquisitionTaskShape[],
  edited: { interval_ms: number; selections: ResourceSelection[] },
): AcquisitionTaskShape[] {
  const { editable, preserved } = splitAcquisitionTasks(existing);
  const taken = new Set(existing.map((t) => t.id));
  let id = editable?.id ?? "t1";
  if (!editable) {
    let n = 2;
    while (taken.has(id)) {
      id = `t1-${n}`;
      n += 1;
    }
  }
  const mode = editable?.mode ?? "poll";
  const canonical: AcquisitionTaskShape = {
    id,
    mode,
    interval_ms: mode === "poll" ? edited.interval_ms : (editable?.interval_ms ?? null),
    binding: { kind: CANONICAL_RESOURCES_KIND, config: { selections: edited.selections } },
  };
  return [...preserved, canonical];
}
