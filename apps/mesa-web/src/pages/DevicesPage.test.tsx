// PR27 Blocker 1 回归：新建设备时名称可空，只填 ID 也必须成功创建
//（name 缺省时用 id；此前 v.name.trim() 在 undefined 上抛异常后被空
// catch 当校验失败吞掉，點擊创建毫无反应）。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
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

describe("DevicesPage 新建设备", () => {
  it("只填 ID、名称留空时仍创建成功且 name=id", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <DevicesPage />
      </MemoryRouter>,
    );
    await screen.findByText("暂无设备，先新建设备或走新建向导");

    await user.click(screen.getByRole("button", { name: "新建设备" }));
    // payload 回归只验证提交值，不验证键盘逐字符输入：一次性 change
    // 避免 AntD Form/Modal 在 jsdom 下逐键重排把测试推过 5s 墙。
    fireEvent.change(screen.getByPlaceholderText("device-a"), {
      target: { value: "device-only" },
    });
    // 名称输入保持空白（placeholder“默认为 ID”）
    // antd 会在双汉字按钮文本中插入空格（“创 建”），用正则匹配
    await user.click(screen.getByRole("button", { name: /创\s?建/ }));

    await waitFor(() => {
      const post = calls.find((c) => c.method === "POST" && c.url === "/api/v1/devices");
      expect(post?.body).toEqual({ id: "device-only", name: "device-only" });
    });
  });

  it("填写名称时按填写值创建", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <DevicesPage />
      </MemoryRouter>,
    );
    await screen.findByText("暂无设备，先新建设备或走新建向导");

    await user.click(screen.getByRole("button", { name: "新建设备" }));
    fireEvent.change(screen.getByPlaceholderText("device-a"), {
      target: { value: "device-a" },
    });
    fireEvent.change(screen.getByPlaceholderText("默认为 ID"), {
      target: { value: "Device A" },
    });
    await user.click(screen.getByRole("button", { name: /创\s?建/ }));

    await waitFor(() => {
      const post = calls.find((c) => c.method === "POST" && c.url === "/api/v1/devices");
      expect(post?.body).toEqual({ id: "device-a", name: "Device A" });
    });
  });
});
