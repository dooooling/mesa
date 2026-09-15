// M2 共享快照层：同一台设备的 Overview 与 LiveData 使用同一个数据来源。
// - points 1s 轮询：成功才替换快照；失败/坏形态保留 last-known，nowMs 独立
//   推进让旧点自然进入 STALE（绝不把错误包解释成 []）；
// - endpoints 低频刷新（默认 10s），失败保留 last-known 上下文：过滤依据
//   deviceId → endpointIds 绝不能因一次失败突然变空而把 points 过滤成空；
// - 代际守卫：deviceId 切换即新一代，旧请求的迟到响应一律丢弃（A 不覆盖 B）；
// - 不发明健康模型：只输出连接运行态 + 点位 GOOD/BAD/STALE 派生，“需要关注”
//   由调用方按 FAILED→BAD→STALE 规则组装。
import { useEffect, useMemo, useRef, useState } from "react";
import {
  POINT_STALE_AFTER_MS,
  pointAgeMs,
  resolveEndpointContexts,
  type Device,
} from "../deviceModel";

export interface WorkspacePoint {
  endpoint_id: string;
  key: string;
  point_key?: string;
  point_id: number;
  quality: string;
  type: string;
  value: unknown;
  timestamp_ns: number;
}

export interface WorkspaceEndpoint {
  id: string;
  name: string;
  driver_id: string;
  device_id?: string;
  state?: string;
}

export type PointStale = "GOOD" | "BAD" | "STALE" | "UNKNOWN";

export interface DevicePointView extends WorkspacePoint {
  /** 展示用点名（key 缺失回落 point_key，再缺失回落 point_id）。 */
  displayKey: string;
  /** 距 nowMs 的年龄（ms），非法时间戳为 null。 */
  ageMs: number | null;
  /** 派生状态：quality BAD 即 BAD；否则 age 超阈即 STALE；非法时间戳为 UNKNOWN。 */
  derived: PointStale;
  endpointName: string;
  deviceId: string;
  deviceName: string;
}

export interface DeviceWorkspaceData {
  device: Device | null;
  deviceNotFound: boolean;
  deviceError: string;
  endpoints: WorkspaceEndpoint[];
  endpointsReady: boolean;
  endpointsError: string;
  /** 归属当前设备的连接 id（顺序即展示顺序）。失败时保留 last-known，不变空。 */
  endpointIds: string[];
  /** 全局快照（未按设备过滤，调用方按需过滤；失败保留 last-known）。 */
  points: WorkspacePoint[];
  pointsError: boolean;
  nowMs: number;
  /** 当前设备的点位视图（含派生 STALE/归属名）。 */
  devicePoints: DevicePointView[];
  counts: { total: number; good: number; bad: number; stale: number; unknown: number };
  /** M6：配置变更后立即刷新 inventory（不等 10s 轮询）。代际守卫内，安全。 */
  reloadInventory: () => void;
}

export const DEVICE_POINTS_POLL_MS = 1000;
export const DEVICE_ENDPOINTS_POLL_MS = 10_000;

/** 派生点位状态：BAD 优先于 STALE；非法时间戳既不算 GOOD 也不算 STALE。 */
export function derivePointStale(quality: string, ageMs: number | null): PointStale {
  if ((quality ?? "").toUpperCase() === "BAD") return "BAD";
  if (ageMs === null) return "UNKNOWN";
  return ageMs > POINT_STALE_AFTER_MS ? "STALE" : "GOOD";
}

function toView(
  p: WorkspacePoint,
  nowMs: number,
  ctx: Map<string, { endpointName: string; deviceId: string; deviceName: string }>,
): DevicePointView {
  const c = ctx.get(p.endpoint_id);
  const ageMs = pointAgeMs(p.timestamp_ns, nowMs);
  return {
    ...p,
    displayKey: p.key ?? p.point_key ?? String(p.point_id),
    ageMs,
    derived: derivePointStale(p.quality, ageMs),
    endpointName: c?.endpointName ?? p.endpoint_id,
    deviceId: c?.deviceId ?? "",
    deviceName: c?.deviceName ?? "",
  };
}

async function fetchJson(url: string): Promise<unknown> {
  const r = await fetch(url);
  if (!r.ok) {
    // status 必须带上：调用方据此区分 404（设备不存在 view）与其它错误。
    // 直接抛裸 Error 会丢 status，导致 404 永远进不了 deviceNotFound。
    const err = new Error(`GET ${url} ${r.status}`) as Error & { status?: number };
    err.status = r.status;
    throw err;
  }
  return r.json();
}

export function useDeviceWorkspaceData(deviceId: string): DeviceWorkspaceData {
  const [device, setDevice] = useState<Device | null>(null);
  const [deviceNotFound, setDeviceNotFound] = useState(false);
  const [deviceError, setDeviceError] = useState("");
  const [endpoints, setEndpoints] = useState<WorkspaceEndpoint[]>([]);
  const [endpointsReady, setEndpointsReady] = useState(false);
  const [endpointsError, setEndpointsError] = useState("");
  const [devices, setDevices] = useState<Device[]>([]);
  const [points, setPoints] = useState<WorkspacePoint[]>([]);
  const [pointsError, setPointsError] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  // 路由代际：deviceId 切换即新一代；旧请求的迟到响应一律丢弃。
  const gen = useRef(0);
  // M6 reloadInventory 代际：手动刷新与轮询共享同一代际计数。
  const reloadRef = useRef<() => void>(() => {});

  useEffect(() => {
    const id = ++gen.current;
    setDevice(null);
    setDeviceNotFound(false);
    setDeviceError("");
    setEndpoints([]);
    setEndpointsReady(false);
    setEndpointsError("");
    // points 快照保留 last-known（跨设备切换不清零旧点，过滤后自然不可见；
    // 代际守卫保证旧设备的迟到点不会写入新一代）。
    let cancelled = false;

    fetchJson(`/api/v1/devices/${deviceId}`)
      .then((d) => {
        if (cancelled || gen.current !== id) return;
        setDevice(d as Device);
      })
      .catch((e: { status?: number } | Error) => {
        if (cancelled || gen.current !== id) return;
        const status = (e as { status?: number })?.status;
        // api.getDevice 抛 ApiError（带 status）；直接 fetch 的兜底按消息判断。
        if (status === 404) setDeviceNotFound(true);
        else setDeviceError(e instanceof Error ? e.message : String(e));
      });

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
          // fail-closed：保留 last-known endpoints/devices，只标记错误与就绪
          //（就绪指“信息已完整”，调用方据此决定是否动 URL/是否过滤）。
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
          // fail-closed：保留 last-known points，nowMs 独立推进使其自然 STALE。
          if (cancelled || gen.current !== id) return;
          setPointsError(true);
        });
    };

    loadInventory();
    loadPoints();
    reloadRef.current = loadInventory;
    let n = 0;
    const timer = window.setInterval(() => {
      if (gen.current !== id) return;
      n += 1;
      // 时钟与拉取解耦：拉取失败也不阻止时钟推进（STALE 照常出现）。
      setNowMs(Date.now());
      loadPoints();
      if (n % (DEVICE_ENDPOINTS_POLL_MS / DEVICE_POINTS_POLL_MS) === 0) loadInventory();
    }, DEVICE_POINTS_POLL_MS);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [deviceId]);

  const ctx = useMemo(
    () =>
      resolveEndpointContexts(
        endpoints.map((e) => ({ id: e.id, name: e.name, device_id: e.device_id })),
        devices.length ? devices : device ? [device] : [],
      ),
    [endpoints, devices, device],
  );

  // 当前设备的连接 id：endpoints 未就绪前返回 last-known（state 里旧值），
  // 绝不因一次失败变空；device 切换时 endpoints 已随代际清空，自然不可见。
  const endpointIds = useMemo(
    () => endpoints.filter((e) => (e.device_id ?? "") === deviceId).map((e) => e.id),
    [endpoints, deviceId],
  );
  const endpointIdSet = useMemo(() => new Set(endpointIds), [endpointIds]);

  const devicePoints = useMemo(() => {
    const views: DevicePointView[] = [];
    for (const p of points) {
      const c = ctx.get(p.endpoint_id);
      // 归属判定优先用 ctx（endpoints 清单），清单缺失该 endpoint 时回落：
      // 若快照里有 deviceId 明确归属其它设备则排除，否则保留（fail-closed，
      // 避免清单一次失败把全量点过滤成空）。
      const owner = c?.deviceId ?? "";
      if (owner && owner !== deviceId) continue;
      if (!owner && !endpointIdSet.has(p.endpoint_id)) {
        // endpoint 不在当前设备的 last-known 清单里：只有当清单已就绪且
        // 明确非空时才排除（此时“不在清单”即“不归属”）；清单未就绪/失败时保留。
        if (endpointsReady && endpointIds.length > 0) continue;
      }
      views.push(toView(p, nowMs, ctx));
    }
    return views;
  }, [points, ctx, deviceId, endpointIdSet, endpointsReady, endpointIds.length, nowMs]);

  const counts = useMemo(() => {
    let good = 0;
    let bad = 0;
    let stale = 0;
    let unknown = 0;
    for (const v of devicePoints) {
      if (v.derived === "GOOD") good += 1;
      else if (v.derived === "BAD") bad += 1;
      else if (v.derived === "STALE") stale += 1;
      else unknown += 1;
    }
    return { total: devicePoints.length, good, bad, stale, unknown };
  }, [devicePoints]);

  return {
    device,
    deviceNotFound,
    deviceError,
    endpoints,
    endpointsReady,
    endpointsError,
    endpointIds,
    points,
    pointsError,
    nowMs,
    devicePoints,
    counts,
    reloadInventory: () => reloadRef.current(),
  };
}
