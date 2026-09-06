// PR8 Gate（Hook）：EventSource 生命周期 + mesa-event 解码 + 暂停/恢复游标。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { sseUrl, useEventStream } from "./useEventStream";
import { makeEvent } from "../test/fixtures";

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  onopen: ((e: unknown) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  closed = false;
  private listeners = new Map<string, ((e: { data: string }) => void)[]>();

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(name: string, fn: (e: { data: string }) => void) {
    const arr = this.listeners.get(name) ?? [];
    arr.push(fn);
    this.listeners.set(name, arr);
  }

  removeEventListener(name: string, fn: (e: { data: string }) => void) {
    this.listeners.set(name, (this.listeners.get(name) ?? []).filter((f) => f !== fn));
  }

  open() {
    this.onopen?.({});
  }

  fail() {
    this.onerror?.({});
  }

  emit(name: string, data: string) {
    for (const fn of this.listeners.get(name) ?? []) fn({ data });
  }

  listenerCount(name: string): number {
    return this.listeners.get(name)?.length ?? 0;
  }

  close() {
    this.closed = true;
  }
}

beforeEach(() => {
  MockEventSource.instances = [];
  vi.stubGlobal("EventSource", MockEventSource as unknown as typeof EventSource);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("sseUrl", () => {
  it("首连携带冻结高水位 H", () => {
    expect(sseUrl(7)).toBe("/api/v1/events/live?after_seq=7");
    expect(sseUrl(null)).toBe("/api/v1/events/live");
  });
});

describe("useEventStream", () => {
  it("监听 mesa-event（非 onmessage），解码后推进游标", () => {
    const onEvent = vi.fn();
    const { result } = renderHook(() => useEventStream({ afterSeq: 5, enabled: true, onEvent }));
    expect(MockEventSource.instances).toHaveLength(1);
    expect(MockEventSource.instances[0].url).toContain("after_seq=5");
    // 必须用 mesa-event 命名事件监听，而非默认 message
    expect(MockEventSource.instances[0].listenerCount("mesa-event")).toBe(1);

    act(() => MockEventSource.instances[0].open());
    expect(result.current.status).toBe("live");

    const frame = JSON.stringify(makeEvent(6));
    act(() => MockEventSource.instances[0].emit("mesa-event", frame));
    expect(onEvent).toHaveBeenCalledTimes(1);
    expect(result.current.lastSeq).toBe(6);

    // 其它事件名忽略
    act(() => MockEventSource.instances[0].emit("message", frame));
    expect(onEvent).toHaveBeenCalledTimes(1);
  });

  it("打开后失败进入 reconnecting；暂停关闭并保留游标，恢复后从游标重连", () => {
    const onEvent = vi.fn();
    const { result, rerender } = renderHook(({ enabled }: { enabled: boolean }) =>
      useEventStream({ afterSeq: 5, enabled, onEvent }),
    { initialProps: { enabled: true } });
    act(() => MockEventSource.instances[0].open());
    act(() => MockEventSource.instances[0].fail());
    expect(result.current.status).toBe("reconnecting");

    rerender({ enabled: false });
    expect(result.current.status).toBe("paused");
    expect(MockEventSource.instances[0].closed).toBe(true);

    rerender({ enabled: true });
    expect(MockEventSource.instances).toHaveLength(2);
    // 恢复后从保留游标重连（DB replay missed）
    expect(MockEventSource.instances[1].url).toContain("after_seq=5");
  });
});
