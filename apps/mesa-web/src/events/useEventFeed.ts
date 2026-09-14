// M3.2 事件数据层：把 EventsView 里的历史/SSE/代际/pending-live/merge 机制
// 原样抽成可复用 hook（不重写语义，只解耦 UI）。
// - 单源 useEventFeed：一个后端过滤（endpoint_id 或全局）对应一次“冻结 H →
//   历史首屏 → SSE 建连 → live 合并 → 加载更早”的完整生命周期；
// - 设备页用多源合并（M3.3，方案 A：每 endpoint 一源，客户端按 seq merge）；
// - 全局页直接单源（M3.4，与旧 EventsView 同过滤）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, isEventStoreUnavailable } from "../api";
import type { EventStats, StoredEvent } from "../types";
import { EVENT_FIRST_PAGE_LIMIT, toEventFilter, type EventFilterForm } from "./filters";
import { mergeEvents } from "./model";
import { useEventStream } from "./useEventStream";
import { matchesLiveFilter } from "../pages/EventsView";

export interface EventFeed {
  history: StoredEvent[];
  nextCursor: number | null;
  booted: boolean;
  loading: boolean;
  loadingMore: boolean;
  unavailable: boolean;
  error: string | null;
  liveOn: boolean;
  setLiveOn: (v: boolean) => void;
  streamStatus: string;
  stats: EventStats | null;
  reload: (next: EventFilterForm) => void;
  loadOlder: () => void;
}

/**
 * 单事件源。form 为后端过滤（调用方保证对象身份稳定，否则每次渲染都 reload）。
 * 代际/pending-live/merge/SSE 冻结顺序与旧 EventsView 完全一致。
 */
export function useEventFeed(form: EventFilterForm, opts?: { statsPoll?: boolean }): EventFeed {
  const [history, setHistory] = useState<StoredEvent[]>([]);
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [highWater, setHighWater] = useState<number>(0);
  const [booted, setBooted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [liveOn, setLiveOn] = useState(true);
  const [stats, setStats] = useState<EventStats | null>(null);
  const histGen = useRef(0);
  const pendingLiveRef = useRef<StoredEvent[]>([]);
  const formRef = useRef(form);
  formRef.current = form;

  const drainPendingLive = (next: EventFilterForm): StoredEvent[] => {
    const live = pendingLiveRef.current.filter((ev) => matchesLiveFilter(ev, next));
    pendingLiveRef.current = [];
    return live;
  };

  // 首屏：冻结 H → 历史完成 → 最后 setHighWater + setBooted（SSE 在历史后建连）。
  useEffect(() => {
    const id = ++histGen.current;
    setLoading(true);
    setError(null);
    (async () => {
      try {
        const h = await api.eventHead();
        const page = await api.listEvents(toEventFilter(formRef.current, { limit: EVENT_FIRST_PAGE_LIMIT }));
        if (histGen.current !== id) return;
        const live = drainPendingLive(formRef.current);
        setHistory(mergeEvents([...page.events].sort((a, b) => b.seq - a.seq), live));
        setNextCursor(page.next_cursor);
        setHighWater(h);
        setLoading(false);
        setBooted(true);
      } catch (e) {
        if (histGen.current !== id) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
        setBooted(true);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const reload = useCallback((next: EventFilterForm) => {
    const id = ++histGen.current;
    setLoading(true);
    setError(null);
    setNextCursor(null);
    api
      .listEvents(toEventFilter(next, { limit: EVENT_FIRST_PAGE_LIMIT }))
      .then((res) => {
        if (histGen.current !== id) return;
        const live = drainPendingLive(next);
        setHistory(mergeEvents([...res.events].sort((a, b) => b.seq - a.seq), live));
        setNextCursor(res.next_cursor);
        setLoading(false);
      })
      .catch((e) => {
        if (histGen.current !== id) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      });
  }, []);

  const loadOlder = useCallback(() => {
    if (nextCursor === null || nextCursor === undefined || loadingMore) return;
    // additive 请求不推进代际；若其间发生过 reload（代际或 filter 已变）则丢弃，
    // 旧 filter 的 older 行绝不混进新页面。
    const id = histGen.current;
    const snapshot = formRef.current;
    setLoadingMore(true);
    api
      .listEvents(toEventFilter(snapshot, { before_seq: nextCursor, limit: EVENT_FIRST_PAGE_LIMIT }))
      .then((res) => {
        if (id !== histGen.current || formRef.current !== snapshot) {
          setLoadingMore(false);
          return;
        }
        setHistory((cur) => mergeEvents(cur, res.events));
        setNextCursor(res.next_cursor);
        setLoadingMore(false);
      })
      .catch((e) => {
        // stale 失败同样丢弃：旧 filter 的错误绝不显示在新页面，也不碰 loadingMore 之外的状态
        if (id !== histGen.current || formRef.current !== snapshot) {
          setLoadingMore(false);
          return;
        }
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoadingMore(false);
      });
  }, [nextCursor, loadingMore]);

  const onLive = useCallback((ev: StoredEvent) => {
    if (!matchesLiveFilter(ev, formRef.current)) return;
    pendingLiveRef.current.push(ev);
    if (pendingLiveRef.current.length > 1000) {
      pendingLiveRef.current.splice(0, pendingLiveRef.current.length - 1000);
    }
    setHistory((cur) => mergeEvents(cur, [ev]));
  }, []);

  // 诊断轮询（15s；失败静默）。设备页多源时由调用方关闭（避免 N 倍轮询）。
  useEffect(() => {
    if (opts?.statsPoll === false) return;
    let stop = false;
    const tick = () => {
      api
        .eventStats()
        .then((s) => {
          if (!stop) setStats(s);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, 15000);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [opts?.statsPoll]);

  const stream = useEventStream({ afterSeq: highWater, enabled: booted && liveOn && !unavailable, onEvent: onLive });

  return useMemo(
    () => ({
      history,
      nextCursor,
      booted,
      loading,
      loadingMore,
      unavailable,
      error,
      liveOn,
      setLiveOn,
      streamStatus: stream.status,
      stats,
      reload,
      loadOlder,
    }),
    [history, nextCursor, booted, loading, loadingMore, unavailable, error, liveOn, stream.status, stats, reload, loadOlder],
  );
}
