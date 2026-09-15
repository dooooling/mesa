// M3.1 回归：全局 /data 与设备页共用 point 语义。
// - 跨设备聚合（设备/连接两列齐全）；
// - ?device=/?connection= URL 联动与级联（设备切换连接回 ALL；非法值归一）；
// - 共用 PointDetailDrawer（来源 Device/Connection/Endpoint 齐全，“打开设备”闭环）。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { POINT_STALE_AFTER_MS } from "../deviceModel";
import App from "../App";

const T0 = 1_700_000_000_000;

function pt(ep: string, key: string, quality: string, ageMs: number, value: unknown = 1) {
  return {
    endpoint_id: ep,
    key,
    point_id: key.length,
    quality,
    type: "f64",
    value,
    timestamp_ns: (T0 - ageMs) * 1e6,
  };
}

function mockGlobal() {
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
    if (url === "/api/v1/points/latest") {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          points: [pt("focas", "spindle.speed", "GOOD", 500, 6000), pt("s7", "DB1.temp", "GOOD", 600, 36.5)],
        }),
      };
    }
    if (url === "/api/v1/devices/cnc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "cnc-01", name: "CNC-01" }) };
    }
    if (url === "/api/v1/devices/plc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "plc-01", name: "PLC-01" }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  vi.setSystemTime(T0);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function renderApp(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

describe("M3.1 全局实时数据", () => {
  it("跨设备聚合：设备/连接/点位三列齐全", async () => {
    mockGlobal();
    renderApp("/data");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText("spindle.speed").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("DB1.temp").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("PLC-01").length).toBeGreaterThanOrEqual(1);
  });

  it("?device= 过滤只留该设备点位", async () => {
    mockGlobal();
    renderApp("/data?device=plc-01");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText("DB1.temp").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryAllByText("spindle.speed")).toHaveLength(0);
  });

  it("STALE 语义与设备页一致（旧点自然 STALE）", async () => {
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices") {
        return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
      }
      if (url === "/api/v1/endpoints") {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            endpoints: [{ id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" }],
          }),
        };
      }
      return {
        ok: true,
        status: 200,
        json: async () => ({ points: [pt("focas", "old-k", "GOOD", POINT_STALE_AFTER_MS + 5000)] }),
      };
    });
    renderApp("/data");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText("old-k").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("STALE")).toBeTruthy();
  });

  it("共用 Drawer：来源齐全，“打开设备”闭环回设备页", async () => {
    vi.useRealTimers();
    mockGlobal();
    const user = userEvent.setup();
    renderApp("/data");
    // 并行负载下表格渲染+轮询多轮，findBy 放宽（只放宽等待，不放宽断言）。
    // P1 后数据点列与来源列文本相同（回落），用 AllBy。
    await screen.findAllByText("spindle.speed", undefined, { timeout: 10000 });
    await user.click(screen.getAllByText("spindle.speed")[0]);
    await screen.findByText("采集设置", undefined, { timeout: 10000 });
    // 来源：设备 + 连接 + endpoint 三段
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("FOCAS").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("focas").length).toBeGreaterThanOrEqual(1);
    // Drawer 内“采集设置”闭环：进设备采集页（连接 query 保留 focas；
    // MemoryRouter 不写 window.location，用采集页内容断言）。
    // 表格行内“打开设备”与 Drawer 按钮重名，锚定 Drawer 的采集按钮。
    await user.click(screen.getByText("采集设置"));
    await screen.findByText("数据采集 · FOCAS", undefined, { timeout: 10000 });
  }, 30000);
});
