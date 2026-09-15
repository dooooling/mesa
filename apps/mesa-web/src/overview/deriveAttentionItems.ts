// M4.1 总览派生层（纯函数）：把全局快照变成“需要关注”列表。
// 诚实语义：请求失败 != 0、未知 != 正常、STALE != BAD，不造 Health Score。
// 来源只取四种真实信息：FAILED endpoint → Active event → BAD point → STALE point。
import { formatAge } from "../deviceModel";
import type { StoredEvent } from "../types";
import type { DevicePointView, WorkspaceEndpoint } from "../workspace/useDeviceWorkspaceData";

export type AttentionKind = "endpoint-failed" | "event-active" | "point-bad" | "point-stale";

export interface AttentionTarget {
  deviceId: string;
  deviceName: string;
  /** 直达路由（设备概览/数据/事件/诊断，不跳中间层）。 */
  href: string;
}

export interface AttentionItem {
  kind: AttentionKind;
  title: string;
  detail: string;
  deviceId: string;
  deviceName: string;
  endpointId?: string;
  endpointName?: string;
  target: AttentionTarget;
  /** 排序键：kind 秩优先，同类按时间倒序（FAILED 无时间，置顶）。 */
  sortTs: number;
}

function isFailedState(state?: string): boolean {
  return (state ?? "").toUpperCase() === "FAILED";
}

function deviceHref(deviceId: string, tab: string, connection?: string): string {
  return `/devices/${deviceId}/${tab}${connection ? `?connection=${connection}` : ""}`;
}

/**
 * 派生需要关注列表。endpointOf：endpoint_id → {deviceId, deviceName} 归属表
 *（调用方由 inventory 构造；归属未知事件/点位不编造设备，直接跳过）。
 */
export function deriveAttentionItems(args: {
  devices: Array<{ id: string; name: string }>;
  endpoints: WorkspaceEndpoint[];
  points: DevicePointView[];
  activeEvents: StoredEvent[];
  endpointOf: Map<string, { deviceId: string; deviceName: string }>;
}): AttentionItem[] {
  const { devices, endpoints, points, activeEvents, endpointOf } = args;
  const deviceNameOf = new Map(devices.map((d) => [d.id, d.name ?? d.id]));
  const out: AttentionItem[] = [];

  for (const e of endpoints) {
    if (!isFailedState(e.state)) continue;
    const deviceId = e.device_id ?? "";
    if (!deviceId) continue;
    const deviceName = deviceNameOf.get(deviceId) ?? deviceId;
    out.push({
      kind: "endpoint-failed",
      title: e.name ?? e.id,
      detail: `连接失败 · ${e.state}`,
      deviceId,
      deviceName,
      endpointId: e.id,
      endpointName: e.name ?? e.id,
      target: {
        deviceId,
        deviceName,
        href: deviceHref(deviceId, "diagnostics", e.id),
      },
      sortTs: Number.POSITIVE_INFINITY,
    });
  }

  for (const ev of activeEvents) {
    const owner = endpointOf.get(ev.endpoint_id);
    if (!owner) continue;
    const sev = ev.event.severity;
    out.push({
      kind: "event-active",
      title: ev.event.message ?? ev.event.code ?? ev.event.kind,
      detail: `Active · Severity ${sev} · ${ev.endpoint_id}`,
      deviceId: owner.deviceId,
      deviceName: owner.deviceName,
      endpointId: ev.endpoint_id,
      target: {
        deviceId: owner.deviceId,
        deviceName: owner.deviceName,
        href: deviceHref(owner.deviceId, "events"),
      },
      sortTs: typeof ev.received_at_ns === "number" ? ev.received_at_ns : 0,
    });
  }

  for (const p of points) {
    if (p.derived !== "BAD") continue;
    const deviceId = p.deviceId;
    if (!deviceId) continue;
    out.push({
      kind: "point-bad",
      title: p.displayKey,
      detail: `BAD · ${formatAge(p.ageMs)}`,
      deviceId,
      deviceName: p.deviceName || deviceId,
      endpointId: p.endpoint_id,
      endpointName: p.endpointName,
      target: {
        deviceId,
        deviceName: p.deviceName || deviceId,
        href: deviceHref(deviceId, "data", p.endpoint_id),
      },
      sortTs: typeof p.timestamp_ns === "number" ? p.timestamp_ns : 0,
    });
  }

  for (const p of points) {
    if (p.derived !== "STALE") continue;
    const deviceId = p.deviceId;
    if (!deviceId) continue;
    out.push({
      kind: "point-stale",
      title: p.displayKey,
      detail: `STALE · ${formatAge(p.ageMs)}未更新`,
      deviceId,
      deviceName: p.deviceName || deviceId,
      endpointId: p.endpoint_id,
      endpointName: p.endpointName,
      target: {
        deviceId,
        deviceName: p.deviceName || deviceId,
        href: deviceHref(deviceId, "data", p.endpoint_id),
      },
      sortTs: typeof p.timestamp_ns === "number" ? p.timestamp_ns : 0,
    });
  }

  const rank: Record<AttentionKind, number> = {
    "endpoint-failed": 0,
    "event-active": 1,
    "point-bad": 2,
    "point-stale": 3,
  };
  return out.sort((a, b) => rank[a.kind] - rank[b.kind] || b.sortTs - a.sortTs);
}
