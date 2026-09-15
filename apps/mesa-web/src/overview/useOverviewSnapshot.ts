// M4.1 总览快照：全局 inventory + points（复用 useLivePointsSource 同源语义）
// + Active events 首屏（active=true 单页，不做 SSE/分页——总览只取“当前活动”信号）。
// 失败语义诚实：任一源失败即标记，不把失败当 0；STALE 由点源的 nowMs 自然派生。
import { useEffect, useMemo, useState } from "react";
import { api, isEventStoreUnavailable } from "../api";
import type { StoredEvent } from "../types";
import { EVENT_FIRST_PAGE_LIMIT, toEventFilter } from "../events/filters";
import { useLivePointsSource } from "../workspace/useLivePointsSource";

export interface OverviewSnapshot {
  devices: Array<{ id: string; name: string }>;
  deviceCount: number;
  endpoints: Array<{ id: string; name?: string; driver_id: string; device_id?: string; state?: string }>;
  endpointTotal: number;
  runningCount: number;
  failedCount: number;
  stoppedCount: number;
  inventoryError: string;
  inventoryReady: boolean;
  pointsTotal: number;
  pointsGood: number;
  pointsBad: number;
  pointsStale: number;
  pointsError: boolean;
  /** 全量点位视图（与计合同源，供需要关注派生；避免页面再开第二个轮询）。 */
  allPoints: import("../workspace/useDeviceWorkspaceData").DevicePointView[];
  activeEvents: StoredEvent[];
  activeEventCount: number | null;
  eventsError: string;
  eventsReady: boolean;
  eventsUnavailable: boolean;
}

export function useOverviewSnapshot(): OverviewSnapshot {
  const src = useLivePointsSource();
  const [activeEvents, setActiveEvents] = useState<StoredEvent[]>([]);
  const [activeEventCount, setActiveEventCount] = useState<number | null>(null);
  const [eventsError, setEventsError] = useState("");
  const [eventsReady, setEventsReady] = useState(false);
  const [eventsUnavailable, setEventsUnavailable] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // 总览只取当前 Active 首屏（排序 seq DESC 取前 N，不翻页——“需要关注”取信号而非全量）。
    api
      .listEvents(toEventFilter({ active: "active" }, { limit: EVENT_FIRST_PAGE_LIMIT }))
      .then((res) => {
        if (cancelled) return;
        setActiveEvents(res.events ?? []);
        setActiveEventCount((res.events ?? []).length);
        setEventsError("");
        setEventsReady(true);
      })
      .catch((e) => {
        if (cancelled) return;
        if (isEventStoreUnavailable(e)) setEventsUnavailable(true);
        else setEventsError(e instanceof Error ? e.message : String(e));
        // 失败即未知：count 保持 null（不写 0），列表保持空。
        setActiveEventCount(null);
        setEventsReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return useMemo(() => {
    const running = src.endpoints.filter((e) => (e.state ?? "").toUpperCase() === "RUNNING").length;
    const failed = src.endpoints.filter((e) => (e.state ?? "").toUpperCase() === "FAILED").length;
    const stopped = src.endpoints.filter(
      (e) => !["RUNNING", "CONNECTING", "RECONNECTING", "FAILED"].includes((e.state ?? "").toUpperCase()),
    ).length;
    return {
      devices: src.devices,
      deviceCount: src.devices.length,
      endpoints: src.endpoints,
      endpointTotal: src.endpoints.length,
      runningCount: running,
      failedCount: failed,
      stoppedCount: stopped,
      inventoryError: src.endpointsError,
      inventoryReady: src.endpointsReady,
      pointsTotal: src.counts.total,
      pointsGood: src.counts.good,
      pointsBad: src.counts.bad,
      pointsStale: src.counts.stale,
      pointsError: src.pointsError,
      allPoints: src.allPoints,
      activeEvents,
      activeEventCount,
      eventsError,
      eventsReady,
      eventsUnavailable,
    };
  }, [src, activeEvents, activeEventCount, eventsError, eventsReady, eventsUnavailable]);
}
