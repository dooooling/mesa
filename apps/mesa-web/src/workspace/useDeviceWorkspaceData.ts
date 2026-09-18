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
  derivePointStale,
  pointAgeMs,
  resolveEndpointContexts,
  type Device,
  type PointStale,
} from "../deviceModel";
import { usePointLiveStream } from "./usePointLiveStream";

export interface WorkspacePoint {
  endpoint_id: string;
  key: string;
  point_key?: string;
  point_id: number;
  quality: string;
  type: string;
  value: unknown;
  timestamp_ns: number;
  /** P1 来源标签（Driver 人类可读来源，如 S7 DB10.DBD20）；缺失即未支持。 */
  source_label?: string;
  /**
   * P2 用户展示名（registry 用户元数据，configure 不产生）。
   * 缺失/空即未设置，UI 回落 point_key；永远不改变 id/key/label。
   */
  display_name?: string;
}

export interface WorkspaceEndpoint {
  id: string;
  name: string;
  driver_id: string;
  device_id?: string;
  state?: string;
}


export interface DevicePointView extends WorkspacePoint {
  /**
   * P2 展示用点名：display_name ?? key ?? point_key ?? point_id。
   * display_name 是用户命名（可改名），key/point_key 是语义身份。
   */
  displayKey: string;
  /**
   * P1 来源展示（Name/Source 双字段口径）：
   * - 有 source_label 即来源标签；
   * - 缺失即 None（UI 诚实显示"未提供"，绝不拿 point_key 反推技术坐标——
   *   point_key 是语义身份，不是 provenance）。
   */
  sourceText: string | null;
  /**
   * 构建时刻的年龄快照（ms），非法时间戳为 null。
   * 实时“更新”文本不读它（AgeCell 从 timestamp_ns + 共享秒钟现算），
   * 它只供非 tick 场景（导出/调试）使用。
   */
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
  /**
   * 全局 inventory（未按设备过滤；仅供跨设备证明/聚合页使用）。
   * 设备页必须用 deviceEndpoints，绝不能直接消费它（跨设备泄漏）。
   */
  endpoints: WorkspaceEndpoint[];
  /**
   * 当前设备作用域的连接（device_id === deviceId，顺序即展示顺序）。
   * Workspace 层唯一真相；六个页面和 Header 只允许收这个。
   */
  deviceEndpoints: WorkspaceEndpoint[];
  endpointsReady: boolean;
  endpointsError: string;
  /** 归属当前设备的连接 id（顺序即展示顺序）。失败时保留 last-known，不变空。 */
  endpointIds: string[];
  /** 全局快照（未按设备过滤，调用方按需过滤；失败保留 last-known）。 */
  points: WorkspacePoint[];
  pointsError: boolean;
  /** 当前设备的点位视图（含派生 STALE/归属名）。 */
  devicePoints: DevicePointView[];
  counts: { total: number; good: number; bad: number; stale: number; unknown: number };
  /** M6：配置变更后立即刷新 inventory（不等 10s 轮询）。代际守卫内，安全。 */
  reloadInventory: () => void;
  /**
   * P2：改名后本地即时更新 points 快照（不等 1s 轮询）。
   * 只改 display_name 字段，不碰值/时间戳/派生状态。
   */
  patchDisplayName: (endpoint_id: string, point_key: string, display_name: string | null) => void;
}

export const DEVICE_ENDPOINTS_POLL_MS = 10_000;

// derivePointStale / PointStale 已收敛到 deviceModel（签名门共用同一派生）。

function toView(
  p: WorkspacePoint,
  buildNow: number,
  ctx: Map<string, { endpointName: string; deviceId: string; deviceName: string }>,
): DevicePointView {
  const c = ctx.get(p.endpoint_id);
  const ageMs = pointAgeMs(p.timestamp_ns, buildNow);
  return {
    ...p,
    // P2 Name：display_name ?? key（point_key） ?? point_id。
    // display_name 为空串视为未设置（后端拒绝空串入库，此处防御性处理）。
    displayKey: p.display_name?.trim() ? p.display_name : (p.key ?? p.point_key ?? String(p.point_id)),
    // P1 Source：有 label 即标签；缺失为 None（UI 显示"未提供"，
    // 不拿 point_key 反推——key 是"它是什么"，不是"它从哪里来"）。
    sourceText: p.source_label ?? null,
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
  // 点值唯一来源：Point Live Stream（SSE state stream；1s REST 轮询已删除）。
  // inventory 仍 10s 轮询（本次只解决 point values）。
  const live = usePointLiveStream();
  const points = live.points as WorkspacePoint[];
  const pointsError = live.pointsError;
  // 无 nowMs state：interval 只拉取，不 tick 整页。年龄由 AgeCell 独立消费共享秒钟。
  // 路由代际：deviceId 切换即新一代；旧请求的迟到响应一律丢弃。
  const gen = useRef(0);
  // M6 reloadInventory 代际：手动刷新与轮询共享同一代际计数。
  const reloadRef = useRef<() => void>(() => {});

  useEffect(() => {
    const id = ++gen.current;
    setDevice(null);
    setDeviceNotFound(false);
    setDeviceError("");
    // RC2 修1：切 device 不再清空 endpoints/devices——/endpoints 与 /devices
    // 是全局 inventory，last-known 可继续为 ownership mapping 提供证明；
    // 按 deviceId 过滤的派生值（endpointIds/devicePoints）自然因 deviceId
    // 变化而失效，不会串台。清空它们反而制造“无法证明归属”的窗口。
    // 但 endpointsReady 必须复位（它是“当前 device inventory 已加载”的标志，
    // 调用方据此决定是否动 URL；数据保留，标志复位）。
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

    loadInventory();
    reloadRef.current = loadInventory;
    const timer = window.setInterval(() => {
      if (gen.current !== id) return;
      loadInventory();
    }, DEVICE_ENDPOINTS_POLL_MS);

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

  // 当前设备的连接：按 device_id 过滤全局 last-known inventory。
  // 切 device 瞬间旧清单仍在，但 deviceId 已变，过滤结果自然为空（不串台）；
  // 新 inventory 就绪后恢复。Workspace 层唯一设备作用域真相——页面只收这个。
  const deviceEndpoints = useMemo(
    () => endpoints.filter((e) => (e.device_id ?? "") === deviceId),
    [endpoints, deviceId],
  );
  const endpointIds = useMemo(
    () => deviceEndpoints.map((e) => e.id),
    [deviceEndpoints],
  );

  // STALE 由 live hook 的 deadline 翻转（staleRevision 变化即重算 derived）；
  // 无网络帧时同样翻转（不再依赖轮询节拍）。
  const devicePoints = useMemo(() => {
    void live.staleRevision;
    const buildNow = Date.now();
    const views: DevicePointView[] = [];
    for (const p of points) {
      // RC2 修1：ownership 真正 fail-closed。能证明 endpoint 归属当前 device
      //（ctx mapping 的 deviceId === deviceId）才显示；mapping 缺失（未知）
      // 一律排除——“无法判断”绝不解释成“可能属于当前设备”。
      // 切 device 窗口：旧 inventory 仍在，A 的 points owner=A≠B 被排除；
      // B 的新 points 在旧清单无 mapping，同样被排除；等 B inventory 就绪
      // 后 mapping 完整才显示。inventory 失败时 owner 恒未知 → 空列表
      //（配 empty 态，不展示别家数据）。
      const owner = ctx.get(p.endpoint_id)?.deviceId ?? "";
      if (owner !== deviceId) continue;
      views.push(toView(p, buildNow, ctx));
    }
    return views;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [points, ctx, deviceId, live.staleRevision]);

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
    deviceEndpoints,
    endpointsReady,
    endpointsError,
    endpointIds,
    points,
    pointsError,
    devicePoints,
    counts,
    reloadInventory: () => reloadRef.current(),
    patchDisplayName: live.patchDisplayName,
  };
}
