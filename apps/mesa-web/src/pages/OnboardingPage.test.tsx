// PR27 Blocker 2 回归：“再建一个”必须同时复位 driverId state 与表单，
// 否则下拉显示 simulator、实际却按上一轮的 opcua 提交。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { OnboardingPage } from "./OnboardingPage";

const calls: Array<{ method: string; url: string; body?: unknown }> = [];

const emptyDesc = {
  contract_major: 2,
  contract_minor: 0,
  identity: { driver_id: "x", name: "x", version: "0.3.0" },
  connection: { fields: [] },
  resources: [],
  controls: { commands: [] },
  resource_selection_methods: [],
  capabilities: { poll: true, subscribe: false, write: false, method: false },
};

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
    if (url === "/api/v1/drivers") {
      return {
        ok: true,
        json: async () => ({
          drivers: [
            { id: "simulator", name: "Simulator", version: "0.3.0" },
            { id: "opcua", name: "OPC UA", version: "0.3.0" },
          ],
        }),
      };
    }
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/descriptor")) {
      return { ok: true, json: async () => emptyDesc };
    }
    if (url === "/api/v1/devices" && method === "POST") {
      const b = body as { id: string; name: string };
      return { ok: true, status: 201, json: async () => ({ id: b.id, name: b.name }) };
    }
    if (url === "/api/v1/endpoints" && method === "POST") {
      return { ok: true, status: 201, json: async () => ({ id: (body as { id: string }).id }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

beforeEach(() => {
  calls.length = 0;
  mockFetch();
});

describe("OnboardingPage 再建一个", () => {
  // 双轮全流程（driver 切换 + 两次提交）耗时较长，显式放宽超时
  it("第二轮下拉显示与实际提交的 driver 一致（均为默认 simulator）", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <OnboardingPage />
      </MemoryRouter>,
    );

    // 第一轮：建设备 → 选 opcua 建连接
    await user.type(screen.getByPlaceholderText("device-a"), "device-a");
    await user.click(screen.getByRole("button", { name: /下一步：添加连接/ }));
    const combo = await screen.findByRole("combobox");
    await user.click(combo);
    await user.click(await screen.findByText("OPC UA"));
    await screen.findByText("该流无参数");
    // 卡片标题带 ② 前缀，区别于顶部 Steps 条的同名步骤
    const step2 = screen.getByText(/② 添加首个连接/).closest(".ant-card") as HTMLElement;
    await user.type(within(step2).getByPlaceholderText(/连接名称|如 PLC/), "OPC UA");
    await user.click(screen.getByRole("button", { name: /完\s?成/ }));
    await screen.findByText(/两个独立对象已生成/);
    const first = calls.find((c) => c.method === "POST" && c.url === "/api/v1/endpoints");
    expect((first?.body as { driver_id: string }).driver_id).toBe("opcua");

    // 再建一个 → 第二轮
    await user.click(screen.getByRole("button", { name: "再建一个" }));
    await user.type(await screen.findByPlaceholderText("device-a"), "device-b");
    await user.click(screen.getByRole("button", { name: /下一步：添加连接/ }));

    // 下拉显示回到默认 simulator…（role=combobox 在 rc-select 里是内置
    // input，文本为空；断言卡片内渲染的选中项文本才是用户所见）
    await screen.findByRole("combobox");
    const step2card = screen.getByText(/② 添加首个连接/).closest(".ant-card") as HTMLElement;
    expect(within(step2card).getByText("Simulator")).toBeTruthy();
    // …且提交的 driver_id 也是 simulator（此前分叉为 opcua）
    await screen.findByText("该流无参数");
    const step2b = screen.getByText(/② 添加首个连接/).closest(".ant-card") as HTMLElement;
    await user.type(within(step2b).getByPlaceholderText(/连接名称|如 PLC/), "Sim");
    await user.click(screen.getByRole("button", { name: /完\s?成/ }));
    await waitFor(() => {
      const posts = calls.filter((c) => c.method === "POST" && c.url === "/api/v1/endpoints");
      expect(posts).toHaveLength(2);
      expect((posts[1].body as { driver_id: string }).driver_id).toBe("simulator");
    });
  }, 30000);
});
