// M3.1 全局实时点数据源：与 useDeviceWorkspaceData 共用同一快照语义。
// - points 走 Point Live SSE：首帧全量 snapshot，后续 delta；
//   断线保留 last-known，STALE 由 deadline 推进；
// - endpoints 10s 刷新，失败保留 last-known；
// - 无 deviceId 限定：全局聚合，全量 points 全部成视图（含设备/连接归属名）。
// 设备页继续用 useDeviceWorkspaceData（内部复用本源 + 设备过滤），语义同源。
import { useEffect, useMemo, useRef, useState } from "react";
import { derivePointStale, pointAgeMs, resolveEndpointContexts, type Device } from "../deviceModel";
import {
  DEVICE_ENDPOINTS_POLL_MS,
  type DevicePointView,
  type WorkspaceEndpoint,
  type WorkspacePoint,
} from "./useDeviceWorkspaceData";
import { usePointLiveStream } from "./usePointLiveStream";

async function fetchJson(url: string): Promise<unknown> {
  const r = await fetch(url);
  if (!r.ok) {
    const err = new Error(`GET ${url} ${r.status}`) as Error & { status?: number };
    err.status = r.status;
    throw err;
  }
  return r.json();
}

export interface LivePointsSource {
  devices: Device[];
  endpoints: WorkspaceEndpoint[];
  endpointsReady: boolean;
  endpointsError: string;
  points: WorkspacePoint[];
  pointsError: boolean;
  /** 全量点位视图（含设备/连接归属名；派生 STALE 与 M2 同规则）。 */
  allPoints: DevicePointView[];
  counts: { total: number; good: number; bad: number; stale: number; unknown: number };
  /** P2：改名后本地即时更新（与设备页同语义）。 */
  patchDisplayName: (endpoint_id: string, point_key: string, display_name: string | null) => void;
}

export function useLivePointsSource(): LivePointsSource {
  const [devices, setDevices] = useState<Device[]>([]);
  const [endpoints, setEndpoints] = useState<WorkspaceEndpoint[]>([]);
  const [endpointsReady, setEndpointsReady] = useState(false);
  const [endpointsError, setEndpointsError] = useState("");
  // 点值唯一来源：Point Live Stream（1s REST 轮询已删除）。
  const live = usePointLiveStream();
  const points = live.points as WorkspacePoint[];
  const pointsError = live.pointsError;
  // 无 nowMs state：STALE 由 live hook 的 deadline 翻转推进。
  const gen = useRef(0);

  useEffect(() => {
    const id = ++gen.current;
    let cancelled = false;

    const loadInventory = () => {
      Promise.all([fetchJson("/api/v1/devices"), fetchJson("/api/v1/endpoints")])
        .then(([dj, ej]) => {
          if (cancelled || gen.current !== id) return;
          const ds = ((dj as { devices?: unknown }).devices ?? []) as Device[];
          const eps = ((ej as { endpoints?: unknown }).endpoints ?? []) as Array<
            WorkspaceEndpoint & { runtime?: { state?: string }; state?: string }
          >;
          if (!Array.isArray(ds) || !Array.isArray(eps)) throw new Error("inventory 形态非法");
          setDevices(ds);
          setEndpoints(
            eps.map((e) => ({
              id: e.id,
              name: e.name ?? e.id,
              driver_id: e.driver_id,
              device_id: e.device_id,
              state: e.state ?? e.runtime?.state,
            })),
          );
          setEndpointsError("");
          setEndpointsReady(true);
        })
        .catch(() => {
          if (cancelled || gen.current !== id) return;
          setEndpointsError("连接清单加载失败：显示为上次已知值。");
          setEndpointsReady(true);
        });
    };

    loadInventory();
    const timer = window.setInterval(() => {
      if (gen.current !== id) return;
      loadInventory();
    }, DEVICE_ENDPOINTS_POLL_MS);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const ctx = useMemo(
    () =>
      resolveEndpointContexts(
        endpoints.map((e) => ({ id: e.id, name: e.name, device_id: e.device_id })),
        devices,
      ),
    [endpoints, devices],
  );

  const allPoints = useMemo<DevicePointView[]>(
    () => {
      // STALE 由 live hook 的 deadline 翻转推进（无 1s tick、无网络请求）。
      void live.staleRevision;
      return points.map((p) => {
        const c = ctx.get(p.endpoint_id);
        const buildNow = Date.now();
        const ageMs = pointAgeMs(p.timestamp_ns, buildNow);
        return {
          ...p,
          // P2 Name：与设备页同口径（display_name 优先）。
          displayKey: p.display_name?.trim() ? p.display_name : (p.key ?? p.point_key ?? String(p.point_id)),
          // P1 Source：与设备页同口径（缺失为 None，不反推）。
          sourceText: p.source_label ?? null,
          ageMs,
          derived: derivePointStale(p.quality, ageMs),
          endpointName: c?.endpointName ?? p.endpoint_id,
          deviceId: c?.deviceId ?? "",
          deviceName: c?.deviceName ?? "",
        };
      });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [points, ctx, live.staleRevision],
  );

  const counts = useMemo(() => {
    let good = 0;
    let bad = 0;
    let stale = 0;
    let unknown = 0;
    for (const v of allPoints) {
      if (v.derived === "GOOD") good += 1;
      else if (v.derived === "BAD") bad += 1;
      else if (v.derived === "STALE") stale += 1;
      else unknown += 1;
    }
    return { total: allPoints.length, good, bad, stale, unknown };
  }, [allPoints]);

  const patchDisplayName = live.patchDisplayName;

  return { devices, endpoints, endpointsReady, endpointsError, points, pointsError, allPoints, counts, patchDisplayName };
}
