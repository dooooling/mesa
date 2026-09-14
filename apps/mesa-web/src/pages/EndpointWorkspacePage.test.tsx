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

  it("P0：A→B 路由切换时 A 的迟到响应不得覆盖 B", async () => {
    const resolvers: Record<string, () => void> = {};
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices/device-a") {
        return { ok: true, status: 200, json: async () => ({ id: "device-a", name: "CNC-01" }) };
      }
      if (url === "/api/v1/endpoints/a-nck" || url === "/api/v1/endpoints/b-sim") {
        const id = url.split("/").pop()!;
        // 两路由的 Endpoint GET 都挂起，由测试按序放行
        await new Promise<void>((r) => { resolvers[id] = r; });
        return {
          ok: true,
          status: 200,
          json: async () => ({
            id,
            name: id === "a-nck" ? "NCK" : "SIM",
            driver_id: "simulator",
            device_id: "device-a",
            connection: {},
          }),
        };
      }
      if (url === "/api/v1/endpoints") {
        return {
          ok: true,
          json: async () => ({
            endpoints: [
              { id: "a-nck", name: "NCK", driver_id: "simulator", device_id: "device-a", runtime: { state: "STOPPED" } },
              { id: "b-sim", name: "SIM", driver_id: "simulator", device_id: "device-a", runtime: { state: "STOPPED" } },
            ],
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
        return { ok: true, json: async () => ({ endpoint_id: "x", revision: 0, event_tasks: [] }) };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    });
    // 同一 Workspace 组件 A→B 复用（路由切换不 remount）
    const { rerender } = render(
      <MemoryRouter initialEntries={["/devices/device-a/endpoints/a-nck"]}>
        <Routes>
          <Route path="/devices/:deviceId/endpoints/:endpointId" element={<EndpointWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    await waitFor(() => expect(resolvers["a-nck"]).toBeDefined());
    rerender(
      <MemoryRouter initialEntries={["/devices/device-a/endpoints/b-sim"]}>
        <Routes>
          <Route path="/devices/:deviceId/endpoints/:endpointId" element={<EndpointWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    await waitFor(() => expect(resolvers["b-sim"]).toBeDefined());
    // B 先回：显示 SIM
    resolvers["b-sim"]();
    await waitFor(() => expect(screen.getAllByText("SIM").length).toBeGreaterThanOrEqual(1));
    // A 后回：必须被忽略，NCK 不得出现
    resolvers["a-nck"]();
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByText("NCK")).toBeNull();
    expect(screen.getAllByText("SIM").length).toBeGreaterThanOrEqual(1);
  });

  it("P0：跨设备归属不一致时不渲染任何操作与配置 Tab", async () => {
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices/device-a") {
        return { ok: true, status: 200, json: async () => ({ id: "device-a", name: "CNC-01" }) };
      }
      if (url === "/api/v1/endpoints/a-nck") {
        return {
          ok: true,
          status: 200,
          // Endpoint 真实归属 device-b，与路由 device-a 不一致
          json: async () => ({ id: "a-nck", name: "NCK", driver_id: "simulator", device_id: "device-b", connection: {} }),
        };
      }
      if (url === "/api/v1/endpoints") {
        return {
          ok: true,
          json: async () => ({
            endpoints: [{ id: "a-nck", name: "NCK", driver_id: "simulator", device_id: "device-b", runtime: { state: "STOPPED" } }],
          }),
        };
      }
      if (url.startsWith("/api/v1/tasks")) {
        return { ok: true, json: async () => ({ tasks: [] }) };
      }
      if (url.endsWith("/event-tasks")) {
        return { ok: true, json: async () => ({ endpoint_id: "a-nck", revision: 0, event_tasks: [] }) };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    });
    render(
      <MemoryRouter initialEntries={["/devices/device-a/endpoints/a-nck"]}>
        <Routes>
          <Route path="/devices/:deviceId/endpoints/:endpointId" element={<EndpointWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    // 只显示归属错误 + 前往正确路径
    expect(await screen.findByText("归属不一致")).toBeTruthy();
    expect(screen.getByText(/前往正确位置/)).toBeTruthy();
    // 操作按钮与配置 Tab 全部不得出现
    expect(screen.queryByRole("button", { name: "启动" })).toBeNull();
    expect(screen.queryByRole("button", { name: "停止" })).toBeNull();
    expect(screen.queryByRole("button", { name: "删除" })).toBeNull();
    for (const t of ["连接", "采集", "事件", "诊断"]) {
      expect(screen.queryByText(t)).toBeNull();
    }
  });
});
