// Vitest 全局 setup：补齐 jsdom 缺失的浏览器 API（antd 渲染需要）。
// NOTE: 本工程未开 vitest globals，testing-library 的自动 cleanup 不会注册，
// 这里显式注册，保证每个测试后卸载组件，避免跨测试 DOM 残留造成多匹配。
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

afterEach(() => {
  cleanup();
});

if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

if (typeof window !== "undefined" && !window.ResizeObserver) {
  window.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof window.ResizeObserver;
}

// Point Live Stream 测试桩：jsdom 无 EventSource，全局默认给一个惰性桩
//（不自动发帧；各测试按需 emit snapshot/delta）。与 useEventStream.test.ts
// 的用例级 Mock 不冲突（用例 stubGlobal 会覆盖全局）。
class __PointLiveStubSource {
  static instances: __PointLiveStubSource[] = [];
  url: string;
  onopen: ((e: unknown) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  closed = false;
  private listeners = new Map<string, ((e: { data: string }) => void)[]>();

  constructor(url: string) {
    this.url = url;
    __PointLiveStubSource.instances.push(this);
  }

  addEventListener(name: string, fn: (e: { data: string }) => void) {
    const arr = this.listeners.get(name) ?? [];
    arr.push(fn);
    this.listeners.set(name, arr);
  }

  removeEventListener(name: string, fn: (e: { data: string }) => void) {
    this.listeners.set(name, (this.listeners.get(name) ?? []).filter((f) => f !== fn));
  }

  emit(name: string, data: string) {
    for (const fn of this.listeners.get(name) ?? []) fn({ data });
  }

  emitSnapshot(rows: Array<Record<string, unknown>>) {
    const data = JSON.stringify({ points: rows });
    for (const fn of this.listeners.get("mesa-points-snapshot") ?? []) fn({ data });
  }

  emitDelta(rows: Array<Record<string, unknown>>) {
    const data = JSON.stringify({ points: rows });
    for (const fn of this.listeners.get("mesa-points-delta") ?? []) fn({ data });
  }

  fail() {
    this.onerror?.({});
  }

  close() {
    this.closed = true;
  }
}

if (
  typeof window !== "undefined" &&
  (window as unknown as { EventSource?: unknown }).EventSource === undefined
) {
  (window as unknown as { EventSource: unknown }).EventSource = __PointLiveStubSource;
  (globalThis as unknown as { EventSource: unknown }).EventSource = __PointLiveStubSource;
  (globalThis as unknown as { __PointLiveStubSource: unknown }).__PointLiveStubSource =
    __PointLiveStubSource;
}
