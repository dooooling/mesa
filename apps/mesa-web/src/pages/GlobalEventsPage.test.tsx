// M3.4 回归：全局 /events 聚合。
// - 跨设备聚合（多 endpoint 历史同屏）；
// - ?device=/?connection= 级联与归一；
// - Drawer“打开设备”闭环回所属设备事件页（保留 ?connection=）。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import type { StoredEvent } from "../types";
import App from "../App";

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

  close() {
    this.closed = true;
  }
}

function mockGlobalEvents() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/devices") {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          devices: [
            { id: "cnc-01", name: "CNC-01" },
            { id: "plc-01", name: "PLC-01" },
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
            { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
            { id: "s7", name: "S7", driver_id: "s7", device_id: "plc-01", state: "RUNNING" },
          ],
        }),
      };
    }
    if (url === "/api/v1/devices/cnc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "cnc-01", name: "CNC-01" }) };
    }
    if (url === "/api/v1/points/latest") {
      return { ok: true, status: 200, json: async () => ({ points: [] }) };
    }
    if (url === "/api/v1/events?limit=1") {
      return { ok: true, status: 200, json: async () => ({ events: [], next_cursor: null }) };
    }
    if (url.startsWith("/api/v1/events?") || url.startsWith("/api/v1/events&")) {
      const u = new URL(url, "http://localhost");
      const ep = u.searchParams.get("endpoint_id");
      const dev = u.searchParams.get("device_id");
      const table: Record<string, StoredEvent[]> = {
        focas: [ev(100, "focas", "cnc-alarm")],
        s7: [ev(90, "s7", "plc-alarm")],
        "": [ev(100, "focas", "cnc-alarm"), ev(90, "s7", "plc-alarm")],
      };
      // M7：device_id 走后端映射（mock 模拟后端行为：plc-01 → s7 事件）。
      const devTable: Record<string, StoredEvent[]> = {
        "plc-01": [ev(90, "s7", "plc-alarm")],
        "cnc-01": [ev(100, "focas", "cnc-alarm")],
      };
      const events = dev ? (devTable[dev] ?? []) : (table[ep ?? ""] ?? []);
      return { ok: true, status: 200, json: async () => ({ events, next_cursor: null }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  MockEventSource.instances = [];
  vi.stubGlobal("EventSource", MockEventSource as unknown as typeof EventSource);
  vi.useRealTimers();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("M7 全局事件（device 服务端过滤）", () => {
  it("跨设备聚合：两设备事件同屏", async () => {
    mockGlobalEvents();
    render(
      <MemoryRouter initialEntries={["/events"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("cnc-alarm");
    expect(screen.getByText("plc-alarm")).toBeTruthy();
  });

  it("?device= 过滤只留该设备事件", async () => {
    mockGlobalEvents();
    render(
      <MemoryRouter initialEntries={["/events?device=plc-01"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("plc-alarm");
    expect(screen.queryByText("cnc-alarm")).toBeNull();
  });

  it("Drawer 打开设备闭环回所属设备事件页", async () => {
    mockGlobalEvents();
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={["/events"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("cnc-alarm");
    await user.click(screen.getByText("cnc-alarm"));
    await screen.findByText("打开设备 →");
    await user.click(screen.getByText("打开设备 →"));
    // 进入设备事件页（设备身份 + 连接上下文 FOCAS）
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context").textContent ?? "").toContain("FOCAS");
    });
    // 并行 worker 下跨页导航 + 多轮事件链较重，放宽超时（单跑 7s 内稳定）。
  }, 30000);
});
