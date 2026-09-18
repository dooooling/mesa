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
 * 严格运行中（仅 RUNNING）。Dashboard“运行中”计数用此口径：
 * CONNECTING / RECONNECTING 是过渡态，不得计入在线；过渡态在启停按钮
 * 侧仍视为忙（isRunningState），避免重复启动。
 */
export function isStrictlyRunning(state?: string): boolean {
  return (state ?? "").toUpperCase() === "RUNNING";
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

/** 服务端任务形状（Foundation-2 单真值：schedule；binding 仅 mesa.resources.v1）。 */
export interface TaskScheduleShape {
  mode: string;
  interval_ms?: number | null;
  publishing_interval_ms?: number | null;
  sampling_interval_ms?: number | null;
  queue_size?: number | null;
  discard_oldest?: boolean | null;
}

export interface AcquisitionTaskShape {
  id: string;
  schedule: TaskScheduleShape;
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
 * - 沿用已存在的 schedule（外来 subscribe 任务不得被编辑器默默翻成 poll）；
 *   poll 任务写编辑器周期，非 poll 任务保留原 schedule。
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
  const schedule = editable?.schedule ?? { mode: "poll" };
  const canonical: AcquisitionTaskShape = {
    id,
    schedule:
      schedule.mode === "poll"
        ? { ...schedule, mode: "poll", interval_ms: edited.interval_ms }
        : schedule,
    binding: { kind: CANONICAL_RESOURCES_KIND, config: { selections: edited.selections } },
  };
  return [...preserved, canonical];
}

// ---------------------------------------------------------------------------
// 统一生命周期状态机（P1：Connection / Acquisition / Event 三处同构）。
//
// 产品语义（PR30 gate，ApplyWithRestart 按钮行为即此语义）：
// - STOPPED：直接 apply，绝不 start（保存后仍 STOPPED）；
// - RUNNING + restart：stop → apply → start；
// - RUNNING + !restart：stop → apply，结束后保持停止。
// 错误语义（fail-closed，postJson 对 409/500 不 throw，只返回 status，
// 调用方必须显式检查，绝不能“stop 500 照样 apply”或“start 500 报成功”）：
// - stop 失败 → 中止，绝不 apply；
// - apply 失败 → 不 start，明确“已停止”；
// - start 失败 → 明确“配置已保存，但恢复运行失败”。
// 本模块为纯状态机：实际 stop/apply/start 由调用方注入，返回 outcome
// 由调用方映射为 message；三处 Pane 不得各写一套生命周期分支。
// ---------------------------------------------------------------------------

/** stop/apply/start 注入动作的返回：一律显式 ok，禁止靠 throw 隐式表达。 */
export interface LifecycleStepResult {
  ok: boolean;
  message?: string;
}

/** applyEndpointChange 的确定性结果（调用方据此แสดง message，无歧义分支）。 */
export type LifecycleOutcome =
  | { kind: "applied-stopped"; restarted: false }
  | { kind: "applied-restarted"; restarted: true }
  | { kind: "stop-failed"; message: string }
  | { kind: "apply-failed-stopped"; message: string }
  | { kind: "applied-but-restart-failed"; message: string };

/**
 * 统一生命周期执行器：STOPPED 直接 apply；RUNNING 先 stop（失败即中止），
 * apply 后按 restart 决定是否 start。各步失败都有明确 outcome，调用方
 * 只负责把 outcome 翻译成中文 message，不得自行解释 wasRunning。
 */
export async function applyEndpointChange(args: {
  wasRunning: boolean;
  restart: boolean;
  stop: () => Promise<LifecycleStepResult>;
  apply: () => Promise<LifecycleStepResult>;
  start: () => Promise<LifecycleStepResult>;
}): Promise<LifecycleOutcome> {
  if (!args.wasRunning) {
    const a = await args.apply();
    if (!a.ok) return { kind: "apply-failed-stopped", message: a.message ?? "应用失败" };
    return { kind: "applied-stopped", restarted: false };
  }
  const s = await args.stop();
  if (!s.ok) return { kind: "stop-failed", message: s.message ?? "停止失败，已中止应用" };
  const a = await args.apply();
  if (!a.ok) return { kind: "apply-failed-stopped", message: a.message ?? "应用失败，Endpoint 当前已停止" };
  if (!args.restart) return { kind: "applied-stopped", restarted: false };
  const r = await args.start();
  if (!r.ok) return { kind: "applied-but-restart-failed", message: r.message ?? "配置已保存，但恢复运行失败" };
  return { kind: "applied-restarted", restarted: true };
}

// ---------------------------------------------------------------------------
// Monitor 展示（P1-10）：类型感知的值渲染 + 更新年龄 + 过期指示。
// 全量拉取是当前后端的唯一形态（/points/latest 无服务端过滤）；这里先做
// 好客户端展示层（设备/连接下拉、数组摘要、STALE），服务端分页/订阅是后话.
// ---------------------------------------------------------------------------

/** 超过该年龄未更新即标 STALE（快照停更通常意味着采集已停）。 */
export const POINT_STALE_AFTER_MS = 30_000;

/**
 * 类型感知的值渲染：标量直显；数组给长度 + 前 3 项摘要（不把几千点
 * 全打出来压垮表格）；对象 JSON 截断；超长截断 160 字符。
 */
export function formatPointValue(value: unknown, maxLen = 160): string {
  let s: string;
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") {
    s = value;
  } else if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    s = String(value);
  } else if (Array.isArray(value)) {
    const head = value.slice(0, 3).map((v) => (typeof v === "object" ? JSON.stringify(v) : String(v)));
    s = `[${value.length}] ${head.join(", ")}${value.length > 3 ? ", …" : ""}`;
  } else if (typeof value === "object") {
    try {
      s = JSON.stringify(value);
    } catch {
      s = String(value);
    }
  } else {
    s = String(value);
  }
  return s.length > maxLen ? `${s.slice(0, maxLen)}…` : s;
}

/** timestamp_ns 距 now 的年龄（ms）；非法时间戳返回 null（不瞎标）。 */
export function pointAgeMs(timestampNs: unknown, nowMs: number): number | null {
  const n = typeof timestampNs === "number" ? timestampNs : Number(timestampNs);
  if (!Number.isFinite(n) || n <= 0) return null;
  return nowMs - Math.floor(n / 1e6);
}

/** 年龄文案：刚刚 / Ns前 / N分钟前；null → 占位。 */
export function formatAge(ageMs: number | null): string {
  if (ageMs === null || ageMs < 0) return "—";
  if (ageMs < 1000) return "刚刚";
  const s = Math.floor(ageMs / 1000);
  if (s < 60) return `${s}秒前`;
  return `${Math.floor(s / 60)}分钟前`;
}

/** 派生点位状态：BAD 优先于 STALE；非法时间戳既不算 GOOD 也不算 STALE。 */
export type PointStale = "GOOD" | "BAD" | "STALE" | "UNKNOWN";

export function derivePointStale(quality: string, ageMs: number | null): PointStale {
  if ((quality ?? "").toUpperCase() === "BAD") return "BAD";
  if (ageMs === null) return "UNKNOWN";
  return ageMs > POINT_STALE_AFTER_MS ? "STALE" : "GOOD";
}
// ---------------------------------------------------------------------------
// 实时渲染稳定化：row 级 reconciliation（替代整快照签名门）。
// 签名门只比较 value/quality/timestamp 会吞掉 metadata 变化
//（source_label/display_name/key/type 独立刷新时 UI 永久不更新）。
// 此处按 endpoint_id:point_id 逐行全字段比较：不变行返回旧对象引用
//（React memo/行级 bailout 自然成立），变行/新行返回新对象。
// derived 用 reconcile 时刻计算，STALE 翻转对齐轮询节拍（语义无损）。
// ---------------------------------------------------------------------------

export interface PointSnapshotLike {
  endpoint_id: string;
  key?: string;
  point_key?: string;
  point_id: number;
  quality: string;
  value: unknown;
  timestamp_ns: number;
  type?: string;
  source_label?: string;
  display_name?: string;
}

/** 稳定行键（与后端 latest_all 的 endpoint_id+point_id 口径一致）。 */
export function pointRowKey(p: PointSnapshotLike): string {
  return `${p.endpoint_id}:${p.point_id}`;
}

function snapshotValueKey(v: unknown): string {
  if (v === null || v === undefined) return "vnull";
  if (typeof v === "number" || typeof v === "boolean" || typeof v === "string") {
    return `${typeof v}:${String(v)}`;
  }
  try {
    return `json:${JSON.stringify(v)}`;
  } catch {
    return "opaque";
  }
}

/**
 * 行级字段指纹：metadata 全字段 + value/quality/timestamp（与 now 无关）。
 * 少任何一个都会吞更新（P1 source_label 可独立刷新，P2 display_name 可跨客户端改名）。
 */
export function pointFieldFingerprint(p: PointSnapshotLike): string {
  return [
    pointRowKey(p),
    p.key ?? "",
    p.point_key ?? "",
    p.type ?? "",
    (p.quality ?? "").toUpperCase(),
    snapshotValueKey(p.value),
    String(p.timestamp_ns ?? 0),
    p.source_label ?? "",
    p.display_name ?? "",
  ].join("|");
}

/**
 * 行级 UI 可观察指纹 = 字段指纹 + derived(now)。
 */
export function pointRowFingerprint(p: PointSnapshotLike, nowMs: number): string {
  const age = pointAgeMs(p.timestamp_ns, nowMs);
  return `${pointFieldFingerprint(p)}|${derivePointStale(p.quality, age)}`;
}

/**
 * 逐行合并：不变行保留旧引用（调用方 memo/Table 行级 bailout），
 * 变行/新行取新对象。返回合并后数组与是否发生变化。
 *
 * 字段比较与 now 无关；derived 跨时刻比较（prevNow → now）：
 * timestamp 不变但时间推进越过 STALE 阈值时同样视为变化
 * （翻转时机对齐轮询节拍，语义无损）。
 */
export function reconcilePointSnapshots<T extends PointSnapshotLike>(
  prev: T[],
  next: T[],
  nowMs: number,
  prevNowMs?: number,
): { points: T[]; changed: boolean } {
  const prevByKey = new Map<string, T>();
  for (const p of prev) prevByKey.set(pointRowKey(p), p);
  const atPrev = prevNowMs ?? nowMs;
  let changed = prev.length !== next.length;
  const out = new Array<T>(next.length);
  for (let i = 0; i < next.length; i++) {
    const n = next[i];
    const o = prevByKey.get(pointRowKey(n));
    if (o === undefined) {
      out[i] = n;
      changed = true;
      continue;
    }
    const sameFields = pointFieldFingerprint(o) === pointFieldFingerprint(n);
    const oldDerived = derivePointStale(o.quality, pointAgeMs(o.timestamp_ns, atPrev));
    const newDerived = derivePointStale(n.quality, pointAgeMs(n.timestamp_ns, nowMs));
    if (sameFields && oldDerived === newDerived) {
      out[i] = o;
    } else {
      out[i] = n;
      changed = true;
    }
  }
  return { points: out, changed };
}

/** 兼容保留：稳定排序键。 */
export function pointSortKey(p: PointSnapshotLike): string {
  return pointRowKey(p);
}

/** 兼容保留：整快照签名（测试/诊断用；生产 gating 走 reconcile）。 */
export function pointsSnapshotSignature(points: PointSnapshotLike[], nowMs: number): string {
  return points.map((p) => pointRowFingerprint(p, nowMs)).sort().join(";");
}

// ---------------------------------------------------------------------------
// Point Live delta 合并（STATE STREAM 客户端侧）：delta 行是完整最新行，
// 不是 patch——已有点直接整行替换（不留脏字段），新点插入；
// delta 未提及的行完全不动；删除只由 snapshot/resync 表达。
// ---------------------------------------------------------------------------

/**
 * delta 合并：按 endpoint_id:point_id 行 identity，已有即整行替换、
 * 未有即插入；输入顺序不保证输出顺序（调用方按需排序）。
 * 不变行保留旧引用（与 snapshot reconcile 同 bailout 语义）。
 */
export function reconcilePointDelta<T extends PointSnapshotLike>(
  prev: T[],
  delta: T[],
): { points: T[]; changed: boolean } {
  if (!delta.length) return { points: prev, changed: false };
  const prevByKey = new Map<string, T>();
  for (const p of prev) prevByKey.set(pointRowKey(p), p);
  let changed = false;
  const out = [...prev];
  const idxByKey = new Map<string, number>();
  out.forEach((p, i) => idxByKey.set(pointRowKey(p), i));
  for (const d of delta) {
    const k = pointRowKey(d);
    const idx = idxByKey.get(k);
    if (idx === undefined) {
      idxByKey.set(k, out.length);
      out.push(d);
      changed = true;
      continue;
    }
    // 整行替换（delta 即完整 current entry，不是 patch）。
    if (pointFieldFingerprint(out[idx]) !== pointFieldFingerprint(d)) {
      out[idx] = d;
      changed = true;
    }
  }
  return { points: out, changed };
}

/**
 * STALE 最近 deadline（Point Live 无网络帧时的翻转调度）：
 * 返回距 nowMs 最近的 GOOD→STALE 翻转剩余 ms；无待翻转即 null。
 * BAD/UNKNOWN 不参与（BAD 永不 STALE，UNKNOWN 无合法时间戳）。
 */
export function nextStaleDeadlineMs<T extends PointSnapshotLike>(
  points: T[],
  nowMs: number,
): number | null {
  let best: number | null = null;
  for (const p of points) {
    if ((p.quality ?? "").toUpperCase() === "BAD") continue;
    const age = pointAgeMs(p.timestamp_ns, nowMs);
    if (age === null || age < 0) continue;
    const remain = POINT_STALE_AFTER_MS - age;
    if (remain <= 0) continue; // 已 STALE：应立即重算而非等待
    if (best === null || remain < best) best = remain;
  }
  return best;
}
