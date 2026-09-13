// 通用 API 客户端：仅依赖 Descriptor 契约，不含协议分支
import type { EventFilter, EventStats, EventTask, ListEventsResponse, StoredEvent } from "./types";

const BASE = "";

export interface ApiError extends Error {
  status: number;
  code?: string;
}

function toApiError(status: number, path: string, code?: string): ApiError {
  const e = new Error(`${status} ${path}`) as ApiError;
  e.status = status;
  e.code = code;
  return e;
}

async function getJson(path: string) {
  const r = await fetch(`${BASE}${path}`);
  if (!r.ok) {
    const j = await r.json().catch(() => ({}));
    const code = (j as { error?: { code?: string } })?.error?.code;
    throw toApiError(r.status, path, code);
  }
  return r.json();
}

function toQuery(filter: EventFilter): string {
  const p = new URLSearchParams();
  const put = (k: string, v: unknown) => {
    if (v === undefined || v === null || v === "") return;
    p.set(k, String(v));
  };
  put("endpoint_id", filter.endpoint_id);
  put("category", filter.category);
  put("kind", filter.kind);
  put("severity_min", filter.severity_min);
  put("code", filter.code);
  put("condition_id", filter.condition_id);
  if (filter.active !== undefined) p.set("active", String(filter.active));
  put("from_ns", filter.from_ns);
  put("to_ns", filter.to_ns);
  put("before_seq", filter.before_seq);
  put("after_seq", filter.after_seq);
  put("limit", filter.limit);
  const s = p.toString();
  return s ? `?${s}` : "";
}

async function postJson(path: string, body: unknown) {
  const r = await fetch(`${BASE}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const j = await r.json().catch(() => ({}));
  return { status: r.status, body: j };
}

async function putJson(path: string, body: unknown) {
  const r = await fetch(`${BASE}${path}`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) {
    const code = (j as { error?: { code?: string } })?.error?.code;
    throw toApiError(r.status, path, code);
  }
  return j;
}

export function isEventStoreUnavailable(e: unknown): boolean {
  const err = e as ApiError;
  return !!err && typeof err.status === "number" && err.status === 503 && err.code === "EVENT_STORE_UNAVAILABLE";
}

export const api = {
  listDrivers: () => getJson("/api/v1/drivers"),
  getDriver: (id: string) => getJson(`/api/v1/drivers/${id}`),
  getDescriptor: (id: string) => getJson(`/api/v1/drivers/${id}/descriptor`),
  validateConnection: (id: string, connection: unknown) =>
    postJson(`/api/v1/drivers/${id}/validate-connection`, { connection }),
  probe: (id: string, connection: unknown) =>
    postJson(`/api/v1/drivers/${id}/probe`, { connection }),
  listEndpoints: () => getJson("/api/v1/endpoints"),
  listDevices: () => getJson("/api/v1/devices"),
  getDevice: (id: string) => getJson(`/api/v1/devices/${id}`),
  createDevice: (body: { id: string; name: string }) =>
    postJson("/api/v1/devices", body),
  updateDevice: (id: string, body: { name: string }) =>
    putJson(`/api/v1/devices/${id}`, body),
  deleteDevice: async (id: string) => {
    const r = await fetch(`/api/v1/devices/${id}`, { method: "DELETE" });
    const j = await r.json().catch(() => ({}));
    return { status: r.status, body: j };
  },
  // Endpoint 创建：device_id 固定来自当前 Device；修改形状无 driver_id
  //（PR25 deny_unknown_fields，创建后不可变），类型层面即禁止传入。
  createEndpoint: (body: {
    id: string;
    name: string;
    device_id: string;
    driver_id: string;
    connection: Record<string, unknown>;
  }) => postJson("/api/v1/endpoints", body),
  updateEndpoint: (id: string, body: {
    name: string;
    device_id: string;
    connection: Record<string, unknown>;
  }) => putJson(`/api/v1/endpoints/${id}`, body),
  deleteEndpoint: async (id: string) => {
    const r = await fetch(`/api/v1/endpoints/${id}`, { method: "DELETE" });
    const j = await r.json().catch(() => ({}));
    return { status: r.status, body: j };
  },
  startEndpoint: (id: string) => postJson(`/api/v1/endpoints/${id}/start`, {}),
  stopEndpoint: (id: string) => postJson(`/api/v1/endpoints/${id}/stop`, {}),
  diagnostics: () => getJson("/api/v1/diagnostics"),
  endpointDiagnostics: (id: string) => getJson(`/api/v1/endpoints/${id}/diagnostics`),
  controlWrite: (endpointId: string, target: string, value: unknown, expected?: unknown) =>
    postJson(`/api/v1/endpoints/${endpointId}/write`, { target, value, expected_value: expected }),
  controlCommand: (endpointId: string, command: string, input: unknown) =>
    postJson(`/api/v1/endpoints/${endpointId}/commands/${command}`, input && typeof input === "object" ? input as Record<string, unknown> : { input }),
  // PR8 Event Plane：历史/SSE 引导/订阅配置/诊断（后端已固定 seq DESC 分页，Web 不自创算法）
  listEvents: (filter: EventFilter): Promise<ListEventsResponse> =>
    getJson(`/api/v1/events${toQuery(filter)}`),
  getEvent: (seq: number): Promise<StoredEvent> => getJson(`/api/v1/events/${seq}`),
  listEventTasks: (endpointId: string): Promise<{ endpoint_id: string; revision: number; event_tasks: EventTask[] }> =>
    getJson(`/api/v1/endpoints/${endpointId}/event-tasks`),
  // Web 只发 {event_tasks:[...]} 唯一形态（后端虽兼容裸 array，但不使用宽容语法）
  replaceEventTasks: (endpointId: string, tasks: EventTask[]) =>
    putJson(`/api/v1/endpoints/${endpointId}/event-tasks`, { event_tasks: tasks }),
  eventStats: (): Promise<EventStats> => getJson("/api/v1/events/stats"),
  // SSE 无窗口启动的关键：冻结全局高水位 H（number；空库为 0，因 Store seq 自 1 起，
  // SSE 永远携带 ?after_seq=H，杜绝 live-only 漏事件窗口）。
  eventHead: async (): Promise<number> => {
    const res = (await getJson("/api/v1/events?limit=1")) as ListEventsResponse;
    return res.events.length > 0 ? res.events[0].seq : 0;
  },
};
