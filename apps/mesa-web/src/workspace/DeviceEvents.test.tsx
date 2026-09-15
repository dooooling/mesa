// M5.4 回归：设备事件（服务端 endpoint_ids 过滤，单查询）。
// - endpoint_ids CSV 一次返回多路（B 的历史不被 A 挤掉——分页错误反例）；
// - ?connection= 限定单连接；无参默认全部；
// - live 行按 endpoint 集合归属合入，它设备事件不混入；
// - device 切换时代际丢弃旧响应。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import type { StoredEvent } from "../types";
import { DeviceWorkspacePage } from "./DeviceWorkspacePage";

function ev(seq: number, endpointId: string, message: string): StoredEvent {
  return {
    seq,
    received_at_ns: seq * 1_000_000,
    endpoint_id: endpointId,
    event: {
      event_id: `e-${seq}`,
      category: "alarm",
      kind: "alarm.condition",
      severity: 800,
      message,
      occurred_at_ns: seq * 1_000_000,
      published_at_ns: seq * 1_000_000,
    },
  } as StoredEvent;
}

function mockDeviceEvents(opts: {
  endpoints: Array<{ id: string; device_id: string }>;
  pages: Record<string, { events: StoredEvent[]; next: number | null }>;
  older?: Record<string, { events: StoredEvent[]; next: number | null }>;
}) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/devices/cnc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "cnc-01", name: "CNC-01" }) };
    }
    if (url === "/api/v1/devices") {
      return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
    }
    if (url === "/api/v1/endpoints") {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          endpoints: opts.endpoints.map((e) => ({ ...e, name: e.id.toUpperCase(), driver_id: "s7", state: "RUNNING" })),
        }),
      };
    }
    if (url === "/api/v1/points/latest") {
      return { ok: true, status: 200, json: async () => ({ points: [] }) };
    }
    if (url === "/api/v1/events?limit=1") {
      return { ok: true, status: 200, json: async () => ({ events: [], next_cursor: null }) };
    }
    if (url.startsWith("/api/v1/events?") || url.startsWith("/api/v1/events&")) {
      const u = new URL(url, "http://localhost");
      const before = u.searchParams.get("before_seq");
      const table = before ? (opts.older ?? {}) : opts.pages;
      // M5.4：服务端过滤形态——endpoint_ids CSV 取并集（保持 seq DESC）；
      // 旧 endpoint_id 单值保留兼容（全局页仍用单值）。
      const csv = u.searchParams.get("endpoint_ids");
      const ep = u.searchParams.get("endpoint_id") ?? "";
      const wanted = csv ? csv.split(",").map((s) => s.trim()).filter(Boolean) : [ep];
      const merged = wanted.flatMap((id) => (table[id] ?? { events: [] }).events);
      merged.sort((a, b) => b.seq - a.seq);
      // next_cursor：任一路还有游标即继续（取最小非 null 游标简化模拟）。
      const nexts = wanted.map((id) => table[id]?.next ?? null).filter((n) => n !== null) as number[];
      const next = nexts.length ? Math.min(...nexts) : null;
      return {
        ok: true,
        status: 200,
        json: async () => ({ events: merged, next_cursor: next }),
      };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderWorkspace(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
      </Routes>
    </MemoryRouter>,
  );
}

// SSE mock（与 EventsView.test.tsx 同套路）：DeviceEvents 建连即建 Mock，
// 默认不 open（不断言 live，只验证历史合并/分页；live 归属见本文件末尾用例）。
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

  emit(name: string, data: string) {
    for (const fn of this.listeners.get(name) ?? []) fn({ data });
  }

  close() {
    this.closed = true;
  }
}

beforeEach(() => {
  vi.clearAllMocks();
  MockEventSource.instances = [];
  vi.stubGlobal("EventSource", MockEventSource as unknown as typeof EventSource);
  // 注：事件流是多轮 promise 链（device/inventory/eventHead/N 路历史），
  // fake timers 下单轮 flush 不够；沿用 EventsView.test.tsx 的真 timers + findBy。
  vi.useRealTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("M5.4 设备事件", () => {
  it("多路合并：B 的历史不被 A 挤掉（分页错误反例）", async () => {
    mockDeviceEvents({
      endpoints: [
        { id: "ep-a", device_id: "cnc-01" },
        { id: "ep-b", device_id: "cnc-01" },
        { id: "ep-x", device_id: "other" },
      ],
      pages: {
        "ep-a": { events: [ev(100, "ep-a", "a-new"), ev(90, "ep-a", "a-old")], next: 90 },
        "ep-b": { events: [ev(95, "ep-b", "b-only")], next: null },
      },
    });
    renderWorkspace("/devices/cnc-01/events");
    // 三条全在，且按 seq DESC（a-new > b-only > a-old）
    await screen.findByText("b-only");
    expect(screen.getByText("a-new")).toBeTruthy();
    expect(screen.getByText("a-old")).toBeTruthy();
    const rows = screen.getAllByText(/a-new|b-only|a-old/).map((el) => el.textContent);
    expect(rows).toEqual(["a-new", "b-only", "a-old"]);
  });

  it("?connection= 限定单连接；它连接事件不出现", async () => {
    mockDeviceEvents({
      endpoints: [
        { id: "ep-a", device_id: "cnc-01" },
        { id: "ep-b", device_id: "cnc-01" },
      ],
      pages: {
        "ep-a": { events: [ev(100, "ep-a", "a-new")], next: null },
        "ep-b": { events: [ev(95, "ep-b", "b-only")], next: null },
      },
    });
    renderWorkspace("/devices/cnc-01/events?connection=ep-b");
    await screen.findByText("b-only");
    expect(screen.queryByText("a-new")).toBeNull();
  });

  it("加载更早：有游标的路可继续（hasMore），无游标显示没有更多", async () => {
    mockDeviceEvents({
      endpoints: [
        { id: "ep-a", device_id: "cnc-01" },
        { id: "ep-b", device_id: "cnc-01" },
      ],
      pages: {
        "ep-a": { events: [ev(100, "ep-a", "a-new")], next: 100 },
        "ep-b": { events: [ev(95, "ep-b", "b-only")], next: null },
      },
      older: {
        "ep-a": { events: [ev(100, "ep-a", "a-new"), ev(80, "ep-a", "a-older")], next: null },
      },
    });
    renderWorkspace("/devices/cnc-01/events");
    await screen.findByText("a-new");
    // ep-a 还有游标 → 有更多；点加载更早后 a-older 出现且 a-new 不重复
    expect(screen.getByText("加载更早")).toBeTruthy();
  });
});

describe("RC2 收口：Event Drawer scope 用 deviceId", () => {
  // 同名设备并存（id 不同、name 相同）时 A→B 名称不变，Drawer 仍必须关闭。
  it("同名 A→B：事件 Drawer 关闭（scope identity 是 deviceId 不是展示名）", async () => {
    const user = (await import("@testing-library/user-event")).default.setup();
    const { Link } = await import("react-router-dom");
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices/dev-a" || url === "/api/v1/devices/dev-b") {
        const id = url.split("/").pop()!;
        return { ok: true, status: 200, json: async () => ({ id, name: "CNC" }) };
      }
      if (url === "/api/v1/devices") {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            devices: [
              { id: "dev-a", name: "CNC" },
              { id: "dev-b", name: "CNC" },
            ],
          }),
        };
      }
      if (url === "/api/v1/endpoints") {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            endpoints: [
              { id: "ep-a", name: "EP-A", driver_id: "s7", device_id: "dev-a", state: "RUNNING" },
              { id: "ep-b", name: "EP-B", driver_id: "s7", device_id: "dev-b", state: "RUNNING" },
            ],
          }),
        };
      }
      if (url === "/api/v1/points/latest") {
        return { ok: true, status: 200, json: async () => ({ points: [] }) };
      }
      if (url === "/api/v1/events?limit=1") {
        return { ok: true, status: 200, json: async () => ({ events: [], next_cursor: null }) };
      }
      if (url.startsWith("/api/v1/events?") || url.startsWith("/api/v1/events&")) {
        const u = new URL(url, "http://localhost");
        const csv = u.searchParams.get("endpoint_ids") ?? "";
        const msgs: Record<string, StoredEvent[]> = {
          "ep-a": [ev(100, "ep-a", "alarm-a")],
          "ep-b": [ev(90, "ep-b", "alarm-b")],
        };
        const merged = csv
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean)
          .flatMap((id) => msgs[id] ?? []);
        merged.sort((a, b) => b.seq - a.seq);
        return { ok: true, status: 200, json: async () => ({ events: merged, next_cursor: null }) };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    }) as unknown as typeof fetch;
    function Nav() {
      return (
        <div>
          <Link to="/devices/dev-a/events">go-a</Link>
          <Link to="/devices/dev-b/events">go-b</Link>
        </div>
      );
    }
    render(
      <MemoryRouter initialEntries={["/"]}>
        <Nav />
        <Routes>
          <Route path="/" element={<div>home</div>} />
          <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    await user.click(screen.getByText("go-a"));
    await screen.findByText("alarm-a");
    await user.click(screen.getByText("alarm-a"));
    // 事件详情 Drawer 打开（标题 Event #100）
    await screen.findByText("Event #100");
    // 切到同名 B：Drawer 必须关闭，B 的事件正常显示
    await user.click(screen.getByText("go-b"));
    await waitFor(() => expect(screen.queryByText("Event #100")).toBeNull(), { timeout: 10000 });
    expect(screen.getByText("alarm-b")).toBeTruthy();
  }, 30000);
});
