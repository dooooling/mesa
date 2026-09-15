// M5.4 设备事件源（服务端过滤版）：M5.3 后端 `endpoint_ids` 一次查询
// 代替 M3.3 的 per-endpoint merge（N+1）。SSE live 仍是单连接无服务端
// 过滤参数，onLive 按 endpoint 集合客户端判定归属后再合入；切换
// device/filter 时旧上下文事件绝不混入（代际 + 集合校验）。
// 高水位 H 复用全局 eventHead（SSE replay 以全局 H 建连）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, isEventStoreUnavailable } from "../api";
import type { StoredEvent } from "../types";
import { EVENT_FIRST_PAGE_LIMIT, toEventFilter, type EventFilterForm } from "./filters";
import { mergeEvents } from "./model";
import { useEventStream } from "./useEventStream";
import { matchesLiveFilter } from "./liveFilter";

export interface DeviceEventFeed {
  history: StoredEvent[];
  /** 服务端还有更早页（next_cursor 非 null）时为 true。 */
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
  const [cursor, setCursor] = useState<number | null>(null);
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

  /** 单次服务端查询（endpoint_ids CSV；空集合直接空页，不发请求）。 */
  const fetchPage = useCallback(
    async (q: DeviceEventQuery, beforeSeq?: number | null) => {
      const ids = [...q.endpointIds].sort().join(",");
      // 空集合直接空页：绝不能把空字符串发给后端（toQuery 会丢弃空值，
      // 后端收不到参数即全表查询—— fail-open 事故）。
      if (!ids) return { events: [], next_cursor: null as number | null };
      return api.listEvents({
        ...toEventFilter({ ...q.filter }, { limit: EVENT_FIRST_PAGE_LIMIT }),
        endpoint_ids: ids,
        ...(beforeSeq !== undefined && beforeSeq !== null ? { before_seq: beforeSeq } : {}),
      });
    },
    [],
  );

  const boot = useCallback(() => {
    const id = ++gen.current;
    const q = queryRef.current;
    setLoading(true);
    setError(null);
    setHistory([]);
    setCursor(null);
    // RC2 修6：generation 更替自己负责复位 loadingMore（loadOlder 的 stale
    // 失败分支直接 return 会把 loadingMore 留在 true）。
    setLoadingMore(false);
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
          // RC2 修4：完整 boot 成功即恢复（unavailable 清零，SSE 重建）。
          setUnavailable(false);
          return;
        }
        const page = await fetchPage(q);
        if (gen.current !== id) return;
        const live = drainPendingLive(q);
        const merged = mergeEvents([...page.events].sort((a, b) => b.seq - a.seq), live);
        setHistory(merged);
        setCursor(page.next_cursor);
        setHighWater(h);
        setLoading(false);
        setBooted(true);
        // RC2 修4：完整 boot 成功即恢复（unavailable 清零，SSE 重建）。
        setUnavailable(false);
      } catch (e) {
        if (gen.current !== id) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
        setBooted(true);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fetchPage, matchesQuery]);

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
    if (cursor === null || cursor === undefined) return;
    setLoadingMore(true);
    fetchPage(q, cursor)
      .then((page) => {
        // RC2 修6：stale 响应绝不碰新 generation 状态。
        if (id !== gen.current) return;
        // 合入时再按当前 query 过一遍：防期间 endpoint 集合收缩导致的归属外行。
        const cur = queryRef.current;
        const rows = page.events.filter((ev) => matchesQuery(ev, cur));
        setHistory((prev) => mergeEvents(prev, rows));
        setCursor(page.next_cursor);
        setLoadingMore(false);
      })
      .catch((e) => {
        if (id !== gen.current) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoadingMore(false);
      });
  }, [cursor, loadingMore, fetchPage, matchesQuery]);

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

  const hasMore = useMemo(() => cursor !== null && cursor !== undefined, [cursor]);

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
