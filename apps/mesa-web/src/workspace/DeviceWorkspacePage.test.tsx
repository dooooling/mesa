// M1 回归：新路由 + Workspace 选择器 + 兼容重定向。
// - / → /overview；/devices/:deviceId → /devices/:deviceId/overview；
// - 老 Endpoint 深层 URL 重定向到 Workspace 并保留 connection 上下文；
// - Workspace 内：设备身份常驻；观察 tab 默认全部；config 无参回落第一个
//   并同步回 URL；URL 合法值优先沿用。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import App from "../App";
import { DeviceWorkspacePage } from "./DeviceWorkspacePage";

function mockFetch(handler: (url: string) => unknown) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => handler(url));
}

const DEV = (id = "cnc-01") => ({
  ok: true,
  status: 200,
  json: async () => ({ id, name: "CNC-01" }),
});
const EPS = () => ({
  ok: true,
  status: 200,
  json: async () => ({
    endpoints: [
      { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      { id: "opcua", name: "OPC UA", driver_id: "opcua", device_id: "cnc-01", state: "STOPPED" },
    ],
  }),
});

beforeEach(() => {
  vi.clearAllMocks();
});

function workspaceHandler(url: string) {
  if (url === "/api/v1/devices/cnc-01") return DEV();
  // M2 共享快照层新增：inventory 全量（devices + endpoints）与 points 快照。
  if (url === "/api/v1/devices") {
    return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
  }
  if (url === "/api/v1/endpoints") return EPS();
  if (url === "/api/v1/points/latest") {
    return { ok: true, status: 200, json: async () => ({ points: [] }) };
  }
  return { ok: false, status: 404, json: async () => ({}) };
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

describe("M1 Device Workspace", () => {
  it("设备身份常驻 + 观察 tab 默认全部连接", async () => {
    mockFetch(workspaceHandler);
    renderWorkspace("/devices/cnc-01/data");
    // Header 身份（设备名至少出现一次）+ 上下文行 testid 断言（文本被多
    // 元素拆分，直接按正则匹配会误报，必须锚定 testid）
    await waitFor(() => {
      expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    });
    expect(screen.getByTestId("workspace-connection-context")?.textContent ?? "").toContain("当前上下文：全部连接");
    // 五 tab 全部存在（占位 Alert 标题与 tab 重名，用 getAllBy 断言至少出现）
    for (const t of ["概览", "实时数据", "事件", "配置", "诊断"]) {
      expect(screen.getAllByText(t).length).toBeGreaterThanOrEqual(1);
    }
  });

  it("config 无参回落第一个连接并同步回 URL（选择稳定）", async () => {
    mockFetch(workspaceHandler);
    const user = userEvent.setup();
    renderWorkspace("/devices/cnc-01/config");
    // 回落到 focas：上下文行显示 FOCAS
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context")?.textContent ?? "").toContain("当前上下文：FOCAS");
    });
    // 切到 OPC UA：按钮选中态跟随
    await user.click(screen.getByRole("button", { name: /OPC UA/ }));
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context")?.textContent ?? "").toContain("当前上下文：OPC UA");
    });
  });

  it("config 优先沿用 URL 的合法连接", async () => {
    mockFetch(workspaceHandler);
    renderWorkspace("/devices/cnc-01/config?connection=opcua");
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context")?.textContent ?? "").toContain("当前上下文：OPC UA");
    });
  });

  it("config 的非法 connection 回落第一个，不漂移", async () => {
    mockFetch(workspaceHandler);
    renderWorkspace("/devices/cnc-01/config?connection=ghost");
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context")?.textContent ?? "").toContain("当前上下文：FOCAS");
    });
  });

  it("设备不存在显示 404 view", async () => {
    mockFetch(() => ({ ok: false, status: 404, json: async () => ({}) }));
    renderWorkspace("/devices/ghost/overview");
    expect(await screen.findByText("设备不存在")).toBeTruthy();
  });
});

describe("M1 路由与兼容重定向", () => {
  function appHandler(url: string) {
    if (url === "/api/v1/devices/cnc-01") return DEV();
    if (url === "/api/v1/devices") {
      return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
    }
    if (url === "/api/v1/endpoints") return EPS();
    // M2 共享快照层新增 points 轮询；兼容测试同样 mock，避免 pend 住。
    if (url === "/api/v1/points/latest") {
      return { ok: true, status: 200, json: async () => ({ points: [] }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  }

  it("/ 重定向到 /overview（一级导航完整）", async () => {
    mockFetch(appHandler);
    render(
      <MemoryRouter initialEntries={["/"]}>
        <App />
      </MemoryRouter>,
    );
    // 重定向成功：OverviewPage 内容出现（系统运行/需要关注卡片；标题含计数故用正则）。
    await screen.findByText(/需要关注/);
    expect(screen.getByText("系统运行")).toBeTruthy();
    for (const label of ["总览", "设备", "实时数据", "事件", "系统"]) {
      expect(screen.getAllByText(label).length).toBeGreaterThanOrEqual(1);
    }
  });

  it("老 Endpoint 深层 URL 重定向到 Workspace 并保留 connection", async () => {
    mockFetch(appHandler);
    render(
      <MemoryRouter initialEntries={["/devices/cnc-01/endpoints/focas"]}>
        <App />
      </MemoryRouter>,
    );
    // Workspace 概览出现，且上下文为 FOCAS（connection 保留）
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context")?.textContent ?? "").toContain("当前上下文：FOCAS");
    });
  });

  it("设备列表无进入按钮、整行点击进 Workspace", async () => {
    mockFetch(appHandler);
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={["/devices"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("CNC-01");
    // “进入”按钮已删除
    expect(screen.queryByRole("button", { name: "进入" })).toBeNull();
    // 整行点击进入 Workspace（出现连接上下文 testid）
    await user.click(screen.getByText("CNC-01"));
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context")).toBeTruthy();
    });
  });
});
