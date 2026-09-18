// Point Live 测试桩：fetch（devices/endpoints）+ EventSource（/points/live）。
// 各页面测试迁移到本桩：点值不再走 /points/latest 轮询；inventory 仍 fetch。
// EventSource 实例取全局 setup 桩（__PointLiveStubSource.instances），
// 本文件只负责 fetch 桩 + snapshot 编码 + emit 入口。
import { vi } from "vitest";

export interface PointLiveStubRow {
  endpoint_id: string;
  key: string;
  quality?: string;
  type?: string;
  value?: unknown;
  timestamp_ns?: number;
  source_label?: string;
  display_name?: string;
}

const T0 = 1_700_000_000_000;

// 测试时间锚：页面测试用 vi.setSystemTime(T0) 冻结 Date.now()，
// stub 行默认 age 500ms → GOOD（不 STALE），与旧 pt() 口径一致。
export function pointLiveNow(): number {
  return T0;
}

export function pointLiveRow(
  ep: string,
  key: string,
  quality = "GOOD",
  ageMs = 500,
  value: unknown = 1,
  extra?: Record<string, unknown>,
): PointLiveStubRow {
  return {
    endpoint_id: ep,
    key,
    quality,
    type: "f64",
    value,
    timestamp_ns: (T0 - ageMs) * 1e6,
    ...extra,
  };
}

export class MockPointLiveSource {
  static instances: MockPointLiveSource[] = [];
  url: string;
  onopen: ((e: unknown) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  closed = false;
  private listeners = new Map<string, ((e: { data: string }) => void)[]>();

  constructor(url: string) {
    this.url = url;
    MockPointLiveSource.instances.push(this);
  }

  addEventListener(name: string, fn: (e: { data: string }) => void) {
    const arr = this.listeners.get(name) ?? [];
    arr.push(fn);
    this.listeners.set(name, arr);
  }

  removeEventListener(name: string, fn: (e: { data: string }) => void) {
    this.listeners.set(name, (this.listeners.get(name) ?? []).filter((f) => f !== fn));
  }

  emitSnapshot(rows: PointLiveStubRow[]) {
    const data = JSON.stringify({
      points: rows.map((r, i) => ({
        endpoint_id: r.endpoint_id,
        point_id: r.key.length + i * 1000,
        point_key: r.key,
        key: r.key,
        quality: r.quality ?? "GOOD",
        type: r.type ?? "f64",
        value: r.value ?? 1,
        timestamp_ns: r.timestamp_ns ?? (T0 - 500) * 1e6,
        value_origin: "CURRENT",
        ...(r.source_label ? { source_label: r.source_label } : {}),
        ...(r.display_name ? { display_name: r.display_name } : {}),
      })),
    });
    for (const fn of this.listeners.get("mesa-points-snapshot") ?? []) fn({ data });
  }

  emitDelta(rows: PointLiveStubRow[]) {
    const data = JSON.stringify({
      points: rows.map((r, i) => ({
        endpoint_id: r.endpoint_id,
        point_id: r.key.length + i * 1000,
        point_key: r.key,
        key: r.key,
        quality: r.quality ?? "GOOD",
        type: r.type ?? "f64",
        value: r.value ?? 1,
        timestamp_ns: r.timestamp_ns ?? (T0 - 500) * 1e6,
        value_origin: "CURRENT",
        ...(r.source_label ? { source_label: r.source_label } : {}),
        ...(r.display_name ? { display_name: r.display_name } : {}),
      })),
    });
    for (const fn of this.listeners.get("mesa-points-delta") ?? []) fn({ data });
  }

  fail() {
    this.onerror?.({});
  }

  close() {
    this.closed = true;
  }
}

type LiveStubInstance = {
  emitSnapshot(rows: PointLiveStubRow[]): void;
  emitDelta?(rows: PointLiveStubRow[]): void;
  fail?(): void;
  emit?(name: string, data: string): void;
};

function encodeRows(rows: PointLiveStubRow[]) {
  return rows.map((r, i) => ({
    endpoint_id: r.endpoint_id,
    point_id: r.key.length + i * 1000,
    point_key: r.key,
    key: r.key,
    quality: r.quality ?? "GOOD",
    type: r.type ?? "f64",
    value: r.value ?? 1,
    timestamp_ns: r.timestamp_ns ?? (T0 - 500) * 1e6,
    value_origin: "CURRENT",
    ...(r.source_label ? { source_label: r.source_label } : {}),
    ...(r.display_name ? { display_name: r.display_name } : {}),
  }));
}

function liveInstances(): LiveStubInstance[] {
  // setup 全局桩实例优先（usePointLiveStream 实际订阅的是它）。
  const fromSetup = (
    globalThis as unknown as { __PointLiveStubSource?: { instances?: LiveStubInstance[] } }
  ).__PointLiveStubSource?.instances;
  if (fromSetup) return fromSetup;
  return MockPointLiveSource.instances as unknown as LiveStubInstance[];
}

function liveInstanceCount(): number {
  return liveInstances().length;
}

/** 安装 fetch 桩；EventSource 用全局 setup 桩（自动实例化，测试只管 emit）。 */
export function installPointLiveStub(opts: {
  points: PointLiveStubRow[];
  devices?: Array<{ id: string; name: string }>;
  device?: { id: string; name: string };
  endpoints?: Array<{ id: string; name?: string; driver_id: string; device_id: string; state?: string }>;
}) {
  const registry = (
    globalThis as unknown as { __PointLiveStubSource?: { instances?: LiveStubInstance[] } }
  ).__PointLiveStubSource;
  if (registry?.instances) registry.instances = [];
  const deviceId = opts.device?.id ?? "cnc-01";
  const deviceName = opts.device?.name ?? "CNC-01";
  // 多设备页（全局 /data）：显式 devices 列表优先，否则单设备回退。
  const deviceList = opts.devices ?? [{ id: deviceId, name: deviceName }];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === `/api/v1/devices/${deviceId}`) {
      return {
        ok: true,
        status: 200,
        json: async () => ({ id: deviceId, name: deviceName }),
      };
    }
    if (url === "/api/v1/devices") {
      return {
        ok: true,
        status: 200,
        json: async () => ({ devices: deviceList }),
      };
    }
    if (url === "/api/v1/endpoints") {
      // 调用方可能传数组，也可能传已包好的 {endpoints:[...]}（旧 mock 口径）：
      // 归一化，避免 {endpoints:{endpoints:[...]}} 双层包装导致 inventory 非法。
      const raw = opts.endpoints as
        | Array<{ id: string; name?: string; driver_id: string; device_id: string; state?: string }>
        | { endpoints?: unknown }
        | undefined;
      const list = Array.isArray(raw)
        ? raw
        : Array.isArray((raw as { endpoints?: unknown } | undefined)?.endpoints)
          ? ((raw as { endpoints: unknown }).endpoints as Array<{
              id: string;
              name?: string;
              driver_id: string;
              device_id: string;
              state?: string;
            }>)
          : [
              { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
              { id: "opcua", name: "OPC UA", driver_id: "opcua", device_id: "cnc-01", state: "RUNNING" },
            ];
      return {
        ok: true,
        status: 200,
        json: async () => ({ endpoints: list }),
      };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  }) as unknown as typeof fetch;
  return {
    emit: () => {
      const inst = liveInstances()[0];
      const rows = encodeRows(opts.points);
      // setup 全局桩有通用 emit(name,data)：优先用它发已编码全行；
      // Mock 桩只有 emitSnapshot(raw)（内部自行编码），走回退。
      const generic = (inst as unknown as { emit?: (n: string, d: string) => void })?.emit;
      if (typeof generic === "function") {
        generic.call(inst, "mesa-points-snapshot", JSON.stringify({ points: rows }));
      } else {
        (inst as LiveStubInstance).emitSnapshot(opts.points);
      }
    },
    emitDelta: (rows: PointLiveStubRow[]) => {
      const inst = liveInstances()[0];
      const generic = (inst as unknown as { emit?: (n: string, d: string) => void })?.emit;
      if (typeof generic === "function") {
        generic.call(inst, "mesa-points-delta", JSON.stringify({ points: encodeRows(rows) }));
      } else if (inst?.emitDelta) {
        inst.emitDelta(rows);
      } else {
        inst?.emitSnapshot(rows);
      }
    },
    fail: () => {
      liveInstances()[0]?.fail?.();
    },
  };
}
