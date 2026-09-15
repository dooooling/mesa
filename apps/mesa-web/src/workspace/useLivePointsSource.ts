// M3.1 全局实时点数据源：与 useDeviceWorkspaceData 共用同一快照语义。
// - points 1s 轮询成功才替换，失败保留 last-known + nowMs 独立推进自然 STALE；
// - endpoints 10s 刷新，失败保留 last-known；
// - 无 deviceId 限定：全局聚合，全量 points 全部成视图（含设备/连接归属名）。
// 设备页继续用 useDeviceWorkspaceData（内部复用本源 + 设备过滤），语义同源。
import { useEffect, useMemo, useRef, useState } from "react";
import { resolveEndpointContexts, type Device } from "../deviceModel";
import {
  derivePointStale,
  DEVICE_ENDPOINTS_POLL_MS,
  DEVICE_POINTS_POLL_MS,
  type DevicePointView,
  type WorkspaceEndpoint,
  type WorkspacePoint,
} from "./useDeviceWorkspaceData";

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
  nowMs: number;
  /** 全量点位视图（含设备/连接归属名；派生 STALE 与 M2 同规则）。 */
  allPoints: DevicePointView[];
  counts: { total: number; good: number; bad: number; stale: number; unknown: number };
}

export function useLivePointsSource(): LivePointsSource {
  const [devices, setDevices] = useState<Device[]>([]);
  const [endpoints, setEndpoints] = useState<WorkspaceEndpoint[]>([]);
  const [endpointsReady, setEndpointsReady] = useState(false);
  const [endpointsError, setEndpointsError] = useState("");
  const [points, setPoints] = useState<WorkspacePoint[]>([]);
  const [pointsError, setPointsError] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
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

    const loadPoints = () => {
      fetchJson("/api/v1/points/latest")
        .then((j) => {
          if (cancelled || gen.current !== id) return;
          const pts = (j as { points?: unknown }).points;
          if (!Array.isArray(pts)) throw new Error("points 形态非法");
          setPoints(pts as WorkspacePoint[]);
          setPointsError(false);
        })
        .catch(() => {
          if (cancelled || gen.current !== id) return;
          setPointsError(true);
        });
    };

    loadInventory();
    loadPoints();
    let n = 0;
    const timer = window.setInterval(() => {
      if (gen.current !== id) return;
      n += 1;
      setNowMs(Date.now());
      loadPoints();
      if (n % (DEVICE_ENDPOINTS_POLL_MS / DEVICE_POINTS_POLL_MS) === 0) loadInventory();
    }, DEVICE_POINTS_POLL_MS);

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
    () =>
      points.map((p) => {
        const c = ctx.get(p.endpoint_id);
        const ageMs =
          typeof p.timestamp_ns === "number" && Number.isFinite(p.timestamp_ns) && p.timestamp_ns > 0
            ? nowMs - Math.floor(Number(p.timestamp_ns) / 1e6)
            : null;
        return {
          ...p,
          displayKey: p.key ?? p.point_key ?? String(p.point_id),
          // P1 Source：source_label ?? 技术坐标（与设备页同口径）
          sourceText: p.source_label ?? p.key ?? p.point_key ?? String(p.point_id),
          ageMs,
          derived: derivePointStale(p.quality, ageMs),
          endpointName: c?.endpointName ?? p.endpoint_id,
          deviceId: c?.deviceId ?? "",
          deviceName: c?.deviceName ?? "",
        };
      }),
    [points, ctx, nowMs],
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

  return { devices, endpoints, endpointsReady, endpointsError, points, pointsError, nowMs, allPoints, counts };
}
