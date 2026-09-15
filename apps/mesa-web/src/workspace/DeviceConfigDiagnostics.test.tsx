// M6 回归：Workspace config/diagnostics tab。
// - config：设备改名/删除/新增按钮 + 连接表（启停/删除）+ 当前连接三 Pane；
// - diagnostics：连接状态 + 采集健康 + 高级诊断折叠；
// - 无连接：两 tab 均明确提示新增连接，不渲染空 Pane。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { DeviceWorkspacePage } from "./DeviceWorkspacePage";

// 并行 worker 负载下多轮串行 fetch（devices→endpoints→diag/tasks）易超
// 默认 1s findBy；统一放宽（只放宽等待，不放宽断言）。
const FIND = { timeout: 10000 };

function mockWorkspace(opts: {
  endpoints: Array<{ id: string; name: string; driver_id: string; device_id: string; state: string }>;
  diag?: Record<string, unknown>;
  tasks?: unknown[];
}) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/devices/cnc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "cnc-01", name: "CNC-01" }) };
    }
    if (url === "/api/v1/devices") {
      return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
    }
    if (url === "/api/v1/endpoints") {
      return { ok: true, status: 200, json: async () => ({ endpoints: opts.endpoints }) };
    }
    if (url === "/api/v1/points/latest") {
      return { ok: true, status: 200, json: async () => ({ points: [] }) };
    }
    if (url.startsWith("/api/v1/endpoints/") && url.endsWith("/diagnostics")) {
      return { ok: true, status: 200, json: async () => opts.diag ?? {} };
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      return { ok: true, status: 200, json: async () => ({ tasks: opts.tasks ?? [] }) };
    }
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/descriptor")) {
      return { ok: true, status: 200, json: async () => ({ connection: { fields: [] }, resources: [] }) };
    }
    if (url === "/api/v1/drivers") {
      return { ok: true, status: 200, json: async () => ({ drivers: [] }) };
    }
    if (url.startsWith("/api/v1/endpoints/") && !url.includes("/diagnostics")) {
      const id = url.split("/").pop() ?? "";
      const ep = opts.endpoints.find((e) => e.id === id);
      if (!ep) return { ok: false, status: 404, json: async () => ({}) };
      return { ok: true, status: 200, json: async () => ({ ...ep, connection: {}, desired_running: false }) };
    }
    if (url === "/api/v1/events?limit=1") {
      return { ok: true, status: 200, json: async () => ({ events: [], next_cursor: null }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderTab(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.useRealTimers();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const EPS = [
  { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
];

describe("M6 DeviceConfig", () => {
  it("config tab：设备操作 + 连接表 + 当前连接设置区", async () => {
    mockWorkspace({ endpoints: EPS });
    renderTab("/devices/cnc-01/config?connection=focas");
    // 设备区（antd 双汉字按钮插空格，用正则；下挂计数确认设备区已渲染）
    await screen.findByText(/下挂 1 个连接/, undefined, FIND);
    expect(screen.getByRole("button", { name: /改\s?名/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /删除设备/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /新增连接/ })).toBeTruthy();
    // 连接表有 FOCAS 行 + 启停操作（RUNNING → 停止按钮；设置区 Pane 内
    // 可能有第二个停止按钮，用 All 断言，加载时序不影响）。
    expect(screen.getAllByText("FOCAS").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByRole("button", { name: /停\s?止/ }).length).toBeGreaterThanOrEqual(1);
    // 单连接设置区（三 Pane 标题出现）
    await screen.findByText(/连接设置/, undefined, FIND);
  });

  it("无连接：明确提示新增，不渲染空 Pane", async () => {
    mockWorkspace({ endpoints: [] });
    renderTab("/devices/cnc-01/config");
    await screen.findByText("该设备暂无连接", undefined, FIND);
  });
});

describe("M6 DeviceDiagnostics", () => {
  it("diagnostics tab：连接状态 + 采集健康 + 高级诊断", async () => {
    mockWorkspace({
      endpoints: EPS,
      diag: {
        connection_state: "RUNNING",
        desired_running: true,
        driver_version: "0.3.0",
        reconnect_attempt_total: 2,
        runtime: { state: "RUNNING", revision: 3 },
      },
      tasks: [{ id: "t1" }],
    });
    renderTab("/devices/cnc-01/diagnostics?connection=focas");
    await screen.findByText(/连接状态/, undefined, FIND);
    // 诊断数据是第二轮 fetch（endpoints 就绪 → diag/tasks），同步断言竞态；
    // 等版本号出现再断言其余（只等一次，后续同批渲染已完成）。
    await screen.findByText("0.3.0", undefined, FIND);
    expect(screen.getByText("高级诊断（原始 JSON）")).toBeTruthy();
  });

  it("无连接：明确提示新增", async () => {
    mockWorkspace({ endpoints: [] });
    renderTab("/devices/cnc-01/diagnostics");
    await screen.findByText("该设备暂无连接", undefined, FIND);
  });
});
