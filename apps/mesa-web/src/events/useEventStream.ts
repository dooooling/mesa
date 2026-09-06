// PR8 SSE Hook：职责只有 EventSource 生命周期 + JSON 解码 + 连接状态 + 全局游标 + 暂停/恢复。
// 后端是 DB-authority：首连 ?after_seq=H 做 server replay；断线重连浏览器自动带
// Last-Event-ID，服务端取 max(query.after_seq, Last-Event-ID)，URL 中的初始 H 不会拉回游标。
import { useCallback, useEffect, useRef, useState } from "react";
import type { StoredEvent } from "../types";

export type EventStreamStatus = "connecting" | "live" | "reconnecting" | "paused" | "closed";

const SSE_EVENT_NAME = "mesa-event";

export interface UseEventStreamOptions {
  /** 初始高水位 H（GET /events?limit=1 冻结）；null 表示尚无事件，照常建连。 */
  afterSeq: number | null;
  /** 是否建连（暂停时传 false，Hook 会 close 并保留游标）。 */
  enabled: boolean;
  onEvent: (ev: StoredEvent) => void;
  onError?: (e: unknown) => void;
}

export interface UseEventStreamResult {
  status: EventStreamStatus;
  lastSeq: number | null;
  pause: () => void;
  resume: () => void;
}

function decodeFrame(data: string): StoredEvent | null {
  try {
    const v = JSON.parse(data) as StoredEvent;
    if (typeof v?.seq !== "number") return null;
    return v;
  } catch {
    return null;
  }
}

export function sseUrl(afterSeq: number | null): string {
  return afterSeq === null || afterSeq === undefined
    ? "/api/v1/events/live"
    : `/api/v1/events/live?after_seq=${afterSeq}`;
}

export function useEventStream({ afterSeq, enabled, onEvent, onError }: UseEventStreamOptions): UseEventStreamResult {
  const [status, setStatus] = useState<EventStreamStatus>(enabled ? "connecting" : "paused");
  const [lastSeq, setLastSeq] = useState<number | null>(afterSeq);
  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  const seqRef = useRef<number | null>(afterSeq);
  const [paused, setPaused] = useState(!enabled);

  useEffect(() => {
    setPaused(!enabled);
  }, [enabled]);

  useEffect(() => {
    if (paused) {
      setStatus("paused");
      return;
    }
    // Resume 从上次游标继续：DB replay missed，无需浏览器堆 pending。
    const start = seqRef.current;
    setStatus("connecting");
    const src = new EventSource(sseUrl(start));
    let opened = false;
    const handler = (e: MessageEvent) => {
      const ev = decodeFrame(e.data);
      if (!ev) return;
      // 服务端已保证 (cursor, high_water] + live 去重；客户端再按 seq 前进游标。
      if (seqRef.current === null || ev.seq > seqRef.current) {
        seqRef.current = ev.seq;
        setLastSeq(ev.seq);
      }
      onEventRef.current(ev);
    };
    const onOpen = () => {
      opened = true;
      setStatus("live");
    };
    const onErr = () => {
      // EventSource 断线会自动重连（带 Last-Event-ID）；此处只翻状态，不手动建连。
      if (!opened) setStatus("connecting");
      else setStatus("reconnecting");
      onErrorRef.current?.(new Error("sse error"));
    };
    src.addEventListener(SSE_EVENT_NAME, handler as EventListener);
    src.onopen = onOpen;
    src.onerror = onErr;
    return () => {
      src.removeEventListener(SSE_EVENT_NAME, handler as EventListener);
      src.close();
      setStatus((s) => (s === "live" || s === "reconnecting" || s === "connecting" ? "closed" : s));
    };
  }, [paused, afterSeq]);

  const pause = useCallback(() => setPaused(true), []);
  const resume = useCallback(() => setPaused(false), []);

  return { status, lastSeq, pause, resume };
}
