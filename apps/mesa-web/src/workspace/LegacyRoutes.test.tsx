// R1.4 回归：旧入口迁移。所有已删除的 V1 路由必须 redirect 到 /overview
//（明确行为），endpoint 深链保留兼容重定向（进 Workspace 并保留 connection）。
// 不存在半死不活的旧页面（旧组件已删除，路由表无残留）。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import App from "../App";

function mockApp() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/drivers") {
      return { ok: true, status: 200, json: async () => ({ drivers: [] }) };
    }
    if (url === "/api/v1/devices") {
      return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
    }
    if (url === "/api/v1/devices/cnc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "cnc-01", name: "CNC-01" }) };
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
    if (url === "/api/v1/points/latest") {
      return { ok: true, status: 200, json: async () => ({ points: [] }) };
    }
    if (url === "/api/v1/events?limit=1") {
      return { ok: true, status: 200, json: async () => ({ events: [], next_cursor: null }) };
    }
    if (url === "/api/v1/events/stats") {
      return { ok: true, status: 200, json: async () => ({ stored_rows: 0, live_clients: 0 }) };
    }
    if (url === "/api/v1/diagnostics") {
      return { ok: true, status: 200, json: async () => ({ version: "x" }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.useRealTimers();
  mockApp();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("R1.4 旧入口迁移", () => {
  // 已删除的 V1 路由 → * → /overview（Overview 内容出现）。
  const dead = [
    "/monitor",
    "/onboarding",
    "/events-legacy",
    "/dashboard-legacy",
    "/devices/cnc-01/legacy",
    "/nope",
  ];
  for (const path of dead) {
    it(`${path} → /overview`, async () => {
      renderAt(path);
      // Overview 标志内容（系统运行卡片）；正则防标题计数拆分。
      await screen.findByText("系统运行", undefined, { timeout: 10000 });
    });
  }

  it("旧 endpoint 深链 → Workspace 并保留 connection", async () => {
    renderAt("/devices/cnc-01/endpoints/focas");
    await screen.findByTestId("workspace-connection-context", undefined, { timeout: 10000 });
    expect(screen.getByTestId("workspace-connection-context")?.textContent ?? "").toContain("FOCAS");
  });

  it("/devices/:deviceId → overview tab（设备内，不离开 Workspace）", async () => {
    renderAt("/devices/cnc-01");
    await screen.findByTestId("workspace-connection-context", undefined, { timeout: 10000 });
  });
});
