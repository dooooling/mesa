// M4.2：列表页创建 Modal 已删除（创建收敛到 /devices/new AddDeviceFlow）。
// 创建语义由 addDevice/bootstrap.test.ts + AddDeviceFlow.test.tsx 覆盖，
// 本文件只保留删除回归 + 入口导航回归。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import App from "../App";
import { DevicesPage } from "./DevicesPage";

const calls: Array<{ method: string; url: string; body?: unknown }> = [];

function mockFetch() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    let body: unknown;
    try {
      body = init?.body ? JSON.parse(init.body as string) : undefined;
    } catch {
      body = undefined;
    }
    calls.push({ method, url, body });
    if (url === "/api/v1/devices" && method === "GET") {
      return { ok: true, json: async () => ({ devices: [] }) };
    }
    if (url === "/api/v1/endpoints" && method === "GET") {
      return { ok: true, json: async () => ({ endpoints: [] }) };
    }
    if (url === "/api/v1/devices" && method === "POST") {
      const b = body as { id: string; name: string };
      return { ok: true, status: 201, json: async () => ({ id: b.id, name: b.name }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

beforeEach(() => {
  calls.length = 0;
  mockFetch();
});

describe("DevicesPage 添加设备入口", () => {
  it("+ 添加设备进 /devices/new（列表页不再直建）", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={["/devices"]}>
        <App />
      </MemoryRouter>,
    );
    await screen.findByText("暂无设备，先新建设备或走新建向导");
    await user.click(screen.getByRole("button", { name: "+ 添加设备" }));
    // 进入 AddDeviceFlow（四步标题出现）
    await waitFor(() => {
      expect(screen.getByText("添加设备")).toBeTruthy();
    });
  });
});

describe("DevicesPage 删除设备", () => {
  it("先弹明确确认，确认后才发 DELETE", async () => {
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      calls.push({ method, url });
      if (url === "/api/v1/devices" && method === "GET") {
        return { ok: true, json: async () => ({ devices: [{ id: "device-a", name: "Device A" }] }) };
      }
      if (url === "/api/v1/endpoints" && method === "GET") {
        return { ok: true, json: async () => ({ endpoints: [] }) };
      }
      if (method === "DELETE") {
        return { ok: true, status: 200, json: async () => ({ deleted: "device-a" }) };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    });
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <DevicesPage />
      </MemoryRouter>,
    );
    await screen.findByText("Device A");

    await user.click(screen.getByRole("button", { name: /删\s?除/ }));
    // 确认框说明后果（antd confirm 标题在 header/body 各渲染一次，只断言出现）
    await waitFor(() => {
      expect(screen.getAllByText("删除设备 device-a？").length).toBeGreaterThanOrEqual(1);
    });
    // 未确认不请求
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    // 确认框确定按钮（antd 双汉字空格；行内删除按钮同名，取最后一个即确认框）
    const oks = screen.getAllByRole("button", { name: /删\s?除/ });
    await user.click(oks[oks.length - 1]);
    await waitFor(() => {
      expect(calls.some((c) => c.method === "DELETE" && c.url === "/api/v1/devices/device-a")).toBe(true);
    });
  }, 30000);
});
