// PR8 Gate（组件）：历史首屏 + mesa-event SSE 帧出现 + history/SSE 同 seq 去重 + filter 切换重置。
import { describe, expect, it, vi, beforeEach, afterEach, type Mock } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { api } from "../api";
import { EventsView } from "./EventsView";
import { makeEvent } from "../test/fixtures";

vi.mock("../api", () => ({
  api: {
    listEndpoints: vi.fn(),
    eventHead: vi.fn(),
    listEvents: vi.fn(),
    eventStats: vi.fn(),
  },
  isEventStoreUnavailable: () => false,
}));

const mocked = api as unknown as {
  listEndpoints: Mock;
  eventHead: Mock;
  listEvents: Mock;
  eventStats: Mock;
};

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

  emit(name: string, data: string) {
    for (const fn of this.listeners.get(name) ?? []) fn({ data });
  }

  close() {
    this.closed = true;
  }
}

const frame = (seq: number) =>
  JSON.stringify(makeEvent(seq, { event: { message: `msg-${seq}` } }));

const historyPage = (seqs: number[]) => ({
  events: seqs.map((s) => makeEvent(s, { event: { message: `msg-${s}` } })),
  next_cursor: null,
});

const STATS = {
  sse_lagged_total: 0,
  sse_replay_frames_total: 0,
  sse_reconcile_total: 0,
  ingress_batches_total: 0,
  ingress_persisted_events_total: 0,
  ingress_batch_duplicates_total: 0,
  ingress_event_duplicates_total: 0,
  ingress_gaps_total: 0,
  ingress_regressions_total: 0,
  ingress_collisions_total: 0,
  ingress_invalid_total: 0,
  ingress_store_failures_total: 0,
  retention_purged_total: 0,
  live_clients: 1,
  stored_rows: 3,
  stored_size_bytes: 1024,
};

beforeEach(() => {
  vi.clearAllMocks();
  MockEventSource.instances = [];
  vi.stubGlobal("EventSource", MockEventSource as unknown as typeof EventSource);
  mocked.listEndpoints.mockResolvedValue({ endpoints: [] });
  mocked.eventHead.mockResolvedValue(5);
  mocked.listEvents.mockImplementation((filter: { before_seq?: number; category?: string }) => {
    if (filter.category) return Promise.resolve({ events: [], next_cursor: null });
    if (filter.before_seq) return Promise.resolve(historyPage([2, 1]));
    return Promise.resolve({ ...historyPage([5, 4, 3]), next_cursor: 2 });
  });
  mocked.eventStats.mockResolvedValue(STATS);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("EventsView", () => {
  it("历史首屏渲染 + SSE 用 after_seq 高水位建连", async () => {
    render(<EventsView />);
    expect(await screen.findByText("msg-5")).toBeTruthy();
    expect(screen.getByText("msg-3")).toBeTruthy();
    await waitFor(() => expect(MockEventSource.instances).toHaveLength(1));
    // 无窗口启动：SSE 携带冻结高水位 H
    expect(MockEventSource.instances[0].url).toContain("after_seq=5");
  });

  it("mesa-event 帧出现；与历史同 seq 只显示一次", async () => {
    render(<EventsView />);
    await screen.findByText("msg-5");
    await waitFor(() => expect(MockEventSource.instances).toHaveLength(1));
    const es = MockEventSource.instances[0];
    es.open();
    expect(await screen.findByText("LIVE ●")).toBeTruthy();
    // 重复帧（history 已有 seq=3）去重
    es.emit("mesa-event", frame(3));
    await waitFor(() => expect(screen.getAllByText("msg-3")).toHaveLength(1));
    // 新帧追加
    es.emit("mesa-event", frame(6));
    expect(await screen.findByText("msg-6")).toBeTruthy();
  });

  it("filter 切换清空旧历史并重载", async () => {
    const user = userEvent.setup();
    render(<EventsView />);
    await screen.findByText("msg-5");
    const input = screen.getByPlaceholderText("Category（精确匹配，如 alarm）");
    await user.type(input, "zz");
    await waitFor(() => expect(screen.queryByText("msg-5")).toBeNull());
    // 重载请求带上新 filter
    const calls = mocked.listEvents.mock.calls;
    const lastCall = calls[calls.length - 1]?.[0] as { category?: string };
    expect(lastCall.category).toContain("zz");
  });

  it("加载更早按 next_cursor 追加且不重复", async () => {
    const user = userEvent.setup();
    render(<EventsView />);
    await screen.findByText("msg-3");
    await user.click(screen.getByRole("button", { name: "加载更早" }));
    expect(await screen.findByText("msg-1")).toBeTruthy();
    // 旧页仍在且无重复（诊断区无事件文本，全屏断言即可）
    expect(screen.getAllByText("msg-3")).toHaveLength(1);
  });
});
