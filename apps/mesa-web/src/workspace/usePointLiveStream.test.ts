// Point Live Stream Hook 回归（STATE STREAM，不是 EVENT STREAM）。
// 锁：snapshot 全量替换 / delta 整行合并 / delta 不删行 / malformed 保留
// last-known / onerror 保留 last-known / unmount 关闭 / 无 id-seq-replay。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { usePointLiveStream } from "./usePointLiveStream";

class MockPointEventSource {
  static instances: MockPointEventSource[] = [];
  url: string;
  onopen: ((e: unknown) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  closed = false;
  private listeners = new Map<string, ((e: { data: string }) => void)[]>();

  constructor(url: string) {
    this.url = url;
    MockPointEventSource.instances.push(this);
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

  listenerCount(name: string): number {
    return this.listeners.get(name)?.length ?? 0;
  }

  fail() {
    this.onerror?.({});
  }

  close() {
    this.closed = true;
  }
}

const T0 = 1_700_000_000_000;

function row(ep: string, pid: number, value: unknown, extra?: Record<string, unknown>) {
  return {
    endpoint_id: ep,
    point_id: pid,
    point_key: `k${pid}`,
    key: `k${pid}`,
    quality: "GOOD",
    type: "f64",
    value,
    timestamp_ns: T0 * 1e6,
    value_origin: "CURRENT",
    ...extra,
  };
}

function snapshotFrame(rows: unknown[]) {
  return JSON.stringify({ points: rows });
}

beforeEach(() => {
  MockPointEventSource.instances = [];
  vi.stubGlobal("EventSource", MockPointEventSource as unknown as typeof EventSource);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("usePointLiveStream", () => {
  it("建连 /points/live，监听 snapshot+delta 命名事件", () => {
    renderHook(() => usePointLiveStream());
    expect(MockPointEventSource.instances).toHaveLength(1);
    expect(MockPointEventSource.instances[0].url).toBe("/api/v1/points/live");
    expect(MockPointEventSource.instances[0].listenerCount("mesa-points-snapshot")).toBe(1);
    expect(MockPointEventSource.instances[0].listenerCount("mesa-points-delta")).toBe(1);
  });

  it("snapshot 全量替换（含删除收敛）", async () => {
    const { result } = renderHook(() => usePointLiveStream());
    await act(async () => {
      MockPointEventSource.instances[0].emit(
        "mesa-points-snapshot",
        snapshotFrame([row("ep", 1, 1), row("ep", 2, 2)]),
      );
    });
    expect(result.current.points).toHaveLength(2);
    // 删除收敛：新 snapshot 无 point 2，旧行消失。
    await act(async () => {
      MockPointEventSource.instances[0].emit("mesa-points-snapshot", snapshotFrame([row("ep", 1, 1)]));
    });
    expect(result.current.points).toHaveLength(1);
    expect(result.current.points[0].point_id).toBe(1);
  });

  it("delta 整行合并，未提及行不动，不处理删除", () => {
    const { result } = renderHook(() => usePointLiveStream());
    act(() =>
      MockPointEventSource.instances[0].emit(
        "mesa-points-snapshot",
        snapshotFrame([row("ep", 1, 1), row("ep", 2, 2)]),
      ),
    );
    const before = result.current.points;
    act(() =>
      MockPointEventSource.instances[0].emit("mesa-points-delta", snapshotFrame([row("ep", 1, 99)])),
    );
    expect(result.current.points).toHaveLength(2);
    expect(result.current.points.find((p) => p.point_id === 1)?.value).toBe(99);
    expect(result.current.points.find((p) => p.point_id === 2)?.value).toBe(2);
    // 未变化行保引用（行级 bailout）。
    const kept = result.current.points.find((p) => p.point_id === 2);
    expect(kept).toBe(before.find((p) => p.point_id === 2));
    // 新点插入。
    act(() =>
      MockPointEventSource.instances[0].emit("mesa-points-delta", snapshotFrame([row("ep", 3, 3)])),
    );
    expect(result.current.points).toHaveLength(3);
  });

  it("malformed 帧保留 last-known + 翻错误态，下一合法帧恢复", () => {
    const { result } = renderHook(() => usePointLiveStream());
    act(() =>
      MockPointEventSource.instances[0].emit(
        "mesa-points-snapshot",
        snapshotFrame([row("ep", 1, 1)]),
      ),
    );
    expect(result.current.points).toHaveLength(1);
    // malformed snapshot：保留 + pointsError=true（坏输入不伪装正常）。
    act(() => MockPointEventSource.instances[0].emit("mesa-points-snapshot", "not-json"));
    expect(result.current.points).toHaveLength(1);
    expect(result.current.pointsError).toBe(true);
    act(() => MockPointEventSource.instances[0].emit("mesa-points-delta", JSON.stringify({})));
    expect(result.current.points).toHaveLength(1);
    expect(result.current.pointsError).toBe(true);
    // 下一合法帧：正常合并 + 错误态恢复。
    act(() =>
      MockPointEventSource.instances[0].emit("mesa-points-delta", snapshotFrame([row("ep", 1, 2)])),
    );
    expect(result.current.points.find((p) => p.point_id === 1)?.value).toBe(2);
    expect(result.current.pointsError).toBe(false);
  });

  it("onerror 保留 last-known 只翻错态", () => {
    const { result } = renderHook(() => usePointLiveStream());
    act(() =>
      MockPointEventSource.instances[0].emit(
        "mesa-points-snapshot",
        snapshotFrame([row("ep", 1, 1)]),
      ),
    );
    expect(result.current.points).toHaveLength(1);
    expect(result.current.pointsError).toBe(false);
    act(() => MockPointEventSource.instances[0].fail());
    expect(result.current.pointsError).toBe(true);
    expect(result.current.points).toHaveLength(1);
  });

  it("unmount 关闭连接", () => {
    const { unmount } = renderHook(() => usePointLiveStream());
    unmount();
    expect(MockPointEventSource.instances[0].closed).toBe(true);
  });
});
