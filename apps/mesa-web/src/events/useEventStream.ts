// PR8 SSE Hook：职责只有 EventSource 生命周期 + JSON 解码 + 连接状态 + 全局游标 + 暂停/恢复。
// 后端是 DB-authority：首连 ?after_seq=H 做 server replay；断线重连浏览器自动带
// Last-Event-ID，服务端取 max(query.after_seq, Last-Event-ID)，URL 中的初始 H 不会拉回游标。
import { useCallback, useEffect, useRef, useState } from "react";
import type { StoredEvent } from "../types";

export type EventStreamStatus = "connecting" | "live" | "reconnecting" | "paused" | "closed";

const SSE_EVENT_NAME = "mesa-event";

export interface UseEventStreamOptions {
  /** 冻结高水位 H（空库为 0）；SSE 永远 ?after_seq=H，杜绝 live-only 窗口。 */
  afterSeq: number;
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

export function sseUrl(afterSeq: number): string {
  return `/api/v1/events/live?after_seq=${afterSeq}`;
}

export function useEventStream({ afterSeq, enabled, onEvent, onError }: UseEventStreamOptions): UseEventStreamResult {
  const [status, setStatus] = useState<EventStreamStatus>(enabled ? "connecting" : "paused");
  const [lastSeq, setLastSeq] = useState<number>(afterSeq);
  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  const seqRef = useRef<number>(afterSeq);
  const [paused, setPaused] = useState(!enabled);
  const prevPropRef = useRef<number>(afterSeq);

  useEffect(() => {
    setPaused(!enabled);
  }, [enabled]);

  // 调用方按 H → history → 建连顺序推进：冻结高水位到达、且本地尚未收到任何帧时跟进游标。
  // 已有 live 帧（seqRef 已前进）时绝不回拉。
  useEffect(() => {
    if (seqRef.current === prevPropRef.current && afterSeq !== prevPropRef.current) {
      seqRef.current = afterSeq;
      setLastSeq(afterSeq);
    }
    prevPropRef.current = afterSeq;
  }, [afterSeq]);

  useEffect(() => {
    if (paused) {
      setStatus("paused");
      return;
    }
    // Resume 从上次游标继续：DB replay missed，无需浏览器堆 pending。
    // 首连时 seqRef 即冻结高水位 H（调用方保证 history 完成之后才建连）。
    const start = seqRef.current;
    setStatus("connecting");
    const src = new EventSource(sseUrl(start));
    let opened = false;
    const handler = (e: MessageEvent) => {
      const ev = decodeFrame(e.data);
      if (!ev) return;
      // 服务端已保证 (cursor, high_water] + live 去重；客户端再按 seq 前进游标。
      if (ev.seq > seqRef.current) {
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
