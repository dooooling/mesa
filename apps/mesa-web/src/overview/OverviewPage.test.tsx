// M4.1 回归：总览派生纯函数 + 页面诚实语义。
// - deriveAttentionItems 排序 FAILED→Active→BAD→STALE；归属未知不编造；
// - 页面：失败标未知/STALE 不写 0；需要关注直达设备对应 tab。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { POINT_STALE_AFTER_MS } from "../deviceModel";
import { deriveAttentionItems } from "./deriveAttentionItems";
import type { DevicePointView } from "../workspace/useDeviceWorkspaceData";
import type { StoredEvent } from "../types";
import App from "../App";

const T0 = 1_700_000_000_000;

function pt(ep: string, key: string, quality: string, ageMs: number): DevicePointView {
  const ts = (T0 - ageMs) * 1e6;
  const age = ageMs;
  return {
    endpoint_id: ep,
    key,
    point_id: key.length,
    quality,
    type: "f64",
    value: 1,
    timestamp_ns: ts,
    displayKey: key,
    // P1：测试构造同样带 sourceText（与 toView 同口径：无 label 即技术坐标）
    sourceText: key,
    ageMs: age,
    derived: quality === "BAD" ? "BAD" : age > POINT_STALE_AFTER_MS ? "STALE" : "GOOD",
    endpointName: ep.toUpperCase(),
    deviceId: ep === "ep-a" || ep === "ep-b" ? "cnc-01" : "",
    deviceName: ep === "ep-a" || ep === "ep-b" ? "CNC-01" : "",
  };
}

function activeEv(seq: number, endpointId: string): StoredEvent {
  return {
    seq,
    received_at_ns: seq * 1_000_000,
    endpoint_id: endpointId,
    event: {
      event_id: `e-${seq}`,
      category: "alarm",
      kind: "alarm.condition",
      severity: 800,
      message: `alarm-${seq}`,
      occurred_at_ns: seq * 1_000_000,
      published_at_ns: seq * 1_000_000,
      condition: { condition_id: "c1", active: true },
    },
  } as StoredEvent;
}

describe("M4.1 deriveAttentionItems", () => {
  it("排序 FAILED → Active → BAD → STALE", () => {
    const items = deriveAttentionItems({
      devices: [{ id: "cnc-01", name: "CNC-01" }],
      endpoints: [{ id: "ep-a", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "FAILED" }],
      points: [pt("ep-a", "bad-k", "BAD", 1000), pt("ep-a", "stale-k", "GOOD", POINT_STALE_AFTER_MS + 1000)],
      activeEvents: [activeEv(50, "ep-a")],
      endpointOf: new Map([["ep-a", { deviceId: "cnc-01", deviceName: "CNC-01" }]]),
    }).map((a) => a.kind);
    expect(items).toEqual(["endpoint-failed", "event-active", "point-bad", "point-stale"]);
  });

  it("归属未知不编造设备（跳过而非归入某设备）", () => {
    const items = deriveAttentionItems({
      devices: [{ id: "cnc-01", name: "CNC-01" }],
      endpoints: [],
      points: [pt("ghost-ep", "bad-k", "BAD", 1000)],
      activeEvents: [],
      endpointOf: new Map(),
    });
    expect(items).toEqual([]);
  });

  it("直达路由正确（诊断/事件/数据带 connection）", () => {
    const items = deriveAttentionItems({
      devices: [{ id: "cnc-01", name: "CNC-01" }],
      endpoints: [{ id: "ep-a", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "FAILED" }],
      points: [pt("ep-a", "bad-k", "BAD", 1000)],
      activeEvents: [activeEv(50, "ep-a")],
      endpointOf: new Map([["ep-a", { deviceId: "cnc-01", deviceName: "CNC-01" }]]),
    });
    const byKind = new Map(items.map((a) => [a.kind, a.target.href]));
    expect(byKind.get("endpoint-failed")).toBe("/devices/cnc-01/diagnostics?connection=ep-a");
    expect(byKind.get("event-active")).toBe("/devices/cnc-01/events");
    expect(byKind.get("point-bad")).toBe("/devices/cnc-01/data?connection=ep-a");
  });
});

describe("M4.1 OverviewPage 诚实语义", () => {
  beforeEach(() => {
    vi.useRealTimers();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function mockOverview(opts: { failInventory?: boolean; failPoints?: boolean; failEvents?: boolean }) {
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices") {
        if (opts.failInventory) return { ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) };
        return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
      }
      if (url === "/api/v1/endpoints") {
        if (opts.failInventory) return { ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) };
        return {
          ok: true,
          status: 200,
          json: async () => ({
            endpoints: [{ id: "ep-a", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "FAILED" }],
          }),
        };
      }
      if (url === "/api/v1/points/latest") {
        if (opts.failPoints) return { ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) };
        return { ok: true, status: 200, json: async () => ({ points: [] }) };
      }
      if (url.startsWith("/api/v1/events")) {
        if (opts.failEvents) return { ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) };
        return { ok: true, status: 200, json: async () => ({ events: [], next_cursor: null }) };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    });
  }

  it("全部失败时标未知/警告，不写 0 假装正常", async () => {
    mockOverview({ failInventory: true, failPoints: true, failEvents: true });
    render(
      <MemoryRouter initialEntries={["/overview"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("部分数据更新失败");
    // “状态未知”在系统运行卡片内；失败后多源 setState 有先后，用 waitFor 等收敛。
    await waitFor(() => {
      expect(screen.getAllByText("状态未知").length).toBeGreaterThanOrEqual(1);
    });
  });

  it("FAILED 连接出现在需要关注并可直达诊断", async () => {
    mockOverview({});
    render(
      <MemoryRouter initialEntries={["/overview"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("FOCAS");
    expect(screen.getByText(/连接失败/)).toBeTruthy();
  });
});
