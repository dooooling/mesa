// M3.3 设备事件源（方案 A）：同一设备下每 endpoint 独立历史分页，客户端
// 按 seq merge。禁止“全局第一页再前端过滤”（分页错误：它设备事件会把本
// 设备历史挤出第一页）。
// - 每路独立 next_cursor：加载更早时各路按自己游标取一页再 merge；
// - SSE 单连（服务端 live 无过滤参数）：onLive 按 endpoint 集合 + 其余过滤
//   客户端判定归属后再合入；切换 device/filter 时旧上下文事件绝不混入
//   （formRef 代际 + endpoint 集合校验）；
// - 高水位 H 复用全局 eventHead（SSE replay 以全局 H 建连，历史按各路取）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, isEventStoreUnavailable } from "../api";
import type { StoredEvent } from "../types";
import { EVENT_FIRST_PAGE_LIMIT, toEventFilter, type EventFilterForm } from "./filters";
import { mergeEvents } from "./model";
import { useEventStream } from "./useEventStream";
import { matchesLiveFilter } from "../pages/EventsView";

export interface DeviceEventFeed {
  history: StoredEvent[];
  /** 还有任一路可继续向前的游标时为 true（任一路 next_cursor 非 null 即可继续）。 */
  hasMore: boolean;
  booted: boolean;
  loading: boolean;
  loadingMore: boolean;
  unavailable: boolean;
  error: string | null;
  liveOn: boolean;
  setLiveOn: (v: boolean) => void;
  streamStatus: string;
  reload: () => void;
  loadOlder: () => void;
}

export interface DeviceEventQuery {
  /** 归属当前设备的 endpoint id（顺序稳定；空 = 设备无连接）。 */
  endpointIds: string[];
  /** 除 endpoint 外的其余后端过滤（category/kind/severity/active/时间）。 */
  filter: Omit<EventFilterForm, "endpoint_id">;
}

/**
 * 设备事件源。query 身份变化（device 切换/连接增删/过滤变化）即新一代：
 * 旧请求的迟到响应一律丢弃；SSE live 行按新 endpoint 集合重判归属。
 */
export function useDeviceEventFeed(query: DeviceEventQuery): DeviceEventFeed {
  const [history, setHistory] = useState<StoredEvent[]>([]);
  const [cursors, setCursors] = useState<Record<string, number | null>>({});
  const [highWater, setHighWater] = useState<number>(0);
  const [booted, setBooted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [liveOn, setLiveOn] = useState(true);
  const gen = useRef(0);
  const pendingLiveRef = useRef<StoredEvent[]>([]);
  const queryRef = useRef(query);
  queryRef.current = query;

  const endpointSetKey = useMemo(() => [...query.endpointIds].sort().join(","), [query.endpointIds]);
  const filterKey = useMemo(() => JSON.stringify(query.filter), [query.filter]);

  const matchesQuery = useCallback((ev: StoredEvent, q: DeviceEventQuery): boolean => {
    if (!q.endpointIds.includes(ev.endpoint_id)) return false;
    return matchesLiveFilter(ev, { ...q.filter, endpoint_id: undefined });
  }, []);

  const drainPendingLive = (q: DeviceEventQuery): StoredEvent[] => {
    const live = pendingLiveRef.current.filter((ev) => matchesQuery(ev, q));
    pendingLiveRef.current = [];
    return live;
  };

  const boot = useCallback(() => {
    const id = ++gen.current;
    const q = queryRef.current;
    setLoading(true);
    setError(null);
    setHistory([]);
    setCursors({});
    (async () => {
      try {
        const h = await api.eventHead();
        if (gen.current !== id) return;
        // 无连接：空历史直接 booted（不查后端，不建无意义分页）。
        if (q.endpointIds.length === 0) {
          const live = drainPendingLive(q);
          setHistory(live);
          setHighWater(h);
          setLoading(false);
          setBooted(true);
          return;
        }
        const pages = await Promise.all(
          q.endpointIds.map(async (endpointId) => {
            const page = await api.listEvents(
              toEventFilter({ ...q.filter, endpoint_id: endpointId }, { limit: EVENT_FIRST_PAGE_LIMIT }),
            );
            return { endpointId, page };
          }),
        );
        if (gen.current !== id) return;
        const live = drainPendingLive(q);
        let merged: StoredEvent[] = [];
        const next: Record<string, number | null> = {};
        for (const { endpointId, page } of pages) {
          merged = mergeEvents(merged, [...page.events].sort((a, b) => b.seq - a.seq));
          next[endpointId] = page.next_cursor;
        }
        merged = mergeEvents(merged, live);
        setHistory(merged);
        setCursors(next);
        setHighWater(h);
        setLoading(false);
        setBooted(true);
      } catch (e) {
        if (gen.current !== id) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
        setBooted(true);
      }
    })();
  }, [matchesQuery]);

  // 首屏 + query 变化（device/连接/过滤）即新一代重查。
  useEffect(() => {
    boot();
  }, [boot, endpointSetKey, filterKey]);

  const reload = useCallback(() => {
    boot();
  }, [boot]);

  const loadOlder = useCallback(() => {
    if (loadingMore) return;
    const q = queryRef.current;
    const id = gen.current;
    // 还有游标的路才继续；全部 null 即没有更多。
    const active = q.endpointIds.filter((ep) => (cursors[ep] ?? null) !== null && cursors[ep] !== undefined);
    // cursors 缺 key（首屏失败/无连接）时不发请求。
    const known = q.endpointIds.filter((ep) => ep in cursors);
    if (known.length === 0 || active.length === 0) return;
    setLoadingMore(true);
    Promise.all(
      active.map(async (endpointId) => {
        const page = await api.listEvents(
          toEventFilter(
            { ...q.filter, endpoint_id: endpointId },
            { before_seq: cursors[endpointId] as number, limit: EVENT_FIRST_PAGE_LIMIT },
          ),
        );
        return { endpointId, page };
      }),
    )
      .then((pages) => {
        if (id !== gen.current) {
          setLoadingMore(false);
          return;
        }
        // 合入时再按当前 query 过一遍：期间 query 若变（代际已变直接丢弃，
        // 此处代际一致只防 endpoint 集合收缩导致的归属外行）。
        const cur = queryRef.current;
        let merged: StoredEvent[] | null = null;
        setHistory((prev) => {
          let next = prev;
          for (const { page } of pages) {
            const rows = page.events.filter((ev) => matchesQuery(ev, cur));
            next = mergeEvents(next, rows);
          }
          merged = next;
          return next;
        });
        void merged;
        setCursors((prev) => {
          const next = { ...prev };
          for (const { endpointId, page } of pages) next[endpointId] = page.next_cursor;
          return next;
        });
        setLoadingMore(false);
      })
      .catch((e) => {
        if (id !== gen.current) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoadingMore(false);
      });
  }, [cursors, loadingMore, matchesQuery]);

  const onLive = useCallback(
    (ev: StoredEvent) => {
      if (!matchesQuery(ev, queryRef.current)) return;
      pendingLiveRef.current.push(ev);
      if (pendingLiveRef.current.length > 1000) {
        pendingLiveRef.current.splice(0, pendingLiveRef.current.length - 1000);
      }
      setHistory((cur) => mergeEvents(cur, [ev]));
    },
    [matchesQuery],
  );

  const stream = useEventStream({ afterSeq: highWater, enabled: booted && liveOn && !unavailable, onEvent: onLive });

  const hasMore = useMemo(
    () => Object.values(cursors).some((c) => c !== null && c !== undefined),
    [cursors],
  );

  return useMemo(
    () => ({
      history,
      hasMore,
      booted,
      loading,
      loadingMore,
      unavailable,
      error,
      liveOn,
      setLiveOn,
      streamStatus: stream.status,
      reload,
      loadOlder,
    }),
    [history, hasMore, booted, loading, loadingMore, unavailable, error, liveOn, stream.status, reload, loadOlder],
  );
}
