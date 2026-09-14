// P1-4 Workspace 回归：嵌套路由装配 + 五 tab + 面包屑归属 Device。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { EndpointWorkspacePage } from "./EndpointWorkspacePage";

const DESC = {
  contract_major: 2,
  contract_minor: 0,
  identity: { driver_id: "simulator", name: "Simulator", version: "0.3.0" },
  connection: { fields: [] },
  resources: [],
  controls: { commands: [] },
  resource_selection_methods: ["manual"],
  capabilities: { poll: true, subscribe: false, write: false, method: false },
  events: { streams: [] },
};

function mockFetch() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/devices/device-a") {
      return { ok: true, status: 200, json: async () => ({ id: "device-a", name: "CNC-01" }) };
    }
    if (url === "/api/v1/endpoints/a-nck") {
      return {
        ok: true,
        status: 200,
        json: async () => ({ id: "a-nck", name: "NCK", driver_id: "simulator", device_id: "device-a", connection: {} }),
      };
    }
    if (url === "/api/v1/endpoints") {
      return {
        ok: true,
        json: async () => ({
          endpoints: [{ id: "a-nck", name: "NCK", driver_id: "simulator", device_id: "device-a", runtime: { state: "STOPPED" } }],
        }),
      };
    }
    if (url === "/api/v1/drivers/simulator/descriptor") {
      return { ok: true, json: async () => DESC };
    }
    if (url.startsWith("/api/v1/tasks")) {
      return { ok: true, json: async () => ({ tasks: [] }) };
    }
    if (url.endsWith("/event-tasks")) {
      return { ok: true, json: async () => ({ endpoint_id: "a-nck", revision: 0, event_tasks: [] }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

beforeEach(() => {
  mockFetch();
});

describe("EndpointWorkspacePage", () => {
  it("嵌套路由渲染面包屑 + 身份 + 五 tab", async () => {
    render(
      <MemoryRouter initialEntries={["/devices/device-a/endpoints/a-nck"]}>
        <Routes>
          <Route path="/devices/:deviceId/endpoints/:endpointId" element={<EndpointWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    // 面包屑：设备 / CNC-01 / NCK（Endpoint 严格归属 Device；设备名在
    // 面包屑与概览各出现一次）
    await waitFor(() => {
      expect(screen.getAllByText("NCK").length).toBeGreaterThanOrEqual(2);
    });
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText("设备")).toBeTruthy();
    // 身份卡：名 + 驱动 + 状态（驱动在身份卡与概览各出现一次）
    expect(screen.getAllByText("simulator").length).toBeGreaterThanOrEqual(2);
    // 五 tab
    for (const t of ["概览", "连接", "采集", "事件", "诊断"]) {
      expect(screen.getByText(t)).toBeTruthy();
    }
    // 概览内容：采集任务计数行
    expect(await screen.findByText("采集任务")).toBeTruthy();
  });
});
