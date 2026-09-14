// PR30 Gate（P0）：Connection Pane 跨 Endpoint 归属安全。
// A→B 切换时 B 未 ready 前保存必须 disabled；A 的迟到响应不得污染 B。
import { describe, expect, it, vi, beforeEach, type Mock } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { api } from "../api";
import { EndpointConnectionPane } from "./EndpointConnectionPane";

vi.mock("../api", () => ({
  api: {
    listEndpoints: vi.fn(),
    stopEndpoint: vi.fn(),
    updateEndpoint: vi.fn(),
    startEndpoint: vi.fn(),
  },
}));

const mocked = api as unknown as {
  listEndpoints: Mock;
  stopEndpoint: Mock;
  updateEndpoint: Mock;
  startEndpoint: Mock;
};

const DESC = {
  connection: { fields: [{ key: "host", label: "Host", field_type: "string", required: true, validation: {}, ui: {} }] },
};

function mockFetchConn(resolvers: Record<string, (v: unknown) => void>) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url.startsWith("/api/v1/endpoints/")) {
      const id = url.split("/").pop()!;
      // 连接 GET 挂起，由测试按序放行（迟到响应可控）
      const body = await new Promise<unknown>((r) => { resolvers[id] = r; });
      return { ok: true, status: 200, json: async () => body };
    }
    if (url.startsWith("/api/v1/drivers/")) {
      return { ok: true, status: 200, json: async () => DESC };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  mocked.listEndpoints.mockResolvedValue({ endpoints: [] });
  mocked.stopEndpoint.mockResolvedValue({ status: 200, body: {} });
  mocked.updateEndpoint.mockResolvedValue({});
  mocked.startEndpoint.mockResolvedValue({ status: 200, body: {} });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("EndpointConnectionPane 归属安全", () => {
  it("B 未 ready 前保存 disabled（A→B 切换窗口不可写）", async () => {
    const resolvers: Record<string, (v: unknown) => void> = {};
    mockFetchConn(resolvers);
    const { rerender } = render(
      <EndpointConnectionPane endpointId="ep-a" deviceId="dev" driverId="drv" initialName="A" onChanged={() => {}} />,
    );
    // A 先回：保存可用
    await waitFor(() => expect(resolvers["ep-a"]).toBeDefined());
    await act(async () => {
      resolvers["ep-a"]({ id: "ep-a", name: "A", connection: { host: "10.0.0.1" } });
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeTruthy());
    expect((screen.getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(false);
    // 切到 B：B 的 GET 在途，快照复位后保存必须禁用
    rerender(
      <EndpointConnectionPane endpointId="ep-b" deviceId="dev" driverId="drv" initialName="B" onChanged={() => {}} />,
    );
    await waitFor(() => expect(resolvers["ep-b"]).toBeDefined());
    expect((screen.getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(true);
    // B 回来后恢复可用
    await act(async () => {
      resolvers["ep-b"]({ id: "ep-b", name: "B", connection: { host: "10.0.0.2" } });
    });
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(false),
    );
  });

  it("A 的迟到响应不得污染 B（保存写 B 的连接）", async () => {
    const resolvers: Record<string, (v: unknown) => void> = {};
    mockFetchConn(resolvers);
    const { rerender } = render(
      <EndpointConnectionPane endpointId="ep-a" deviceId="dev" driverId="drv" initialName="A" onChanged={() => {}} />,
    );
    await waitFor(() => expect(resolvers["ep-a"]).toBeDefined());
    rerender(
      <EndpointConnectionPane endpointId="ep-b" deviceId="dev" driverId="drv" initialName="B" onChanged={() => {}} />,
    );
    await waitFor(() => expect(resolvers["ep-b"]).toBeDefined());
    // B 先回（host=B）
    await act(async () => {
      resolvers["ep-b"]({ id: "ep-b", name: "B", connection: { host: "host-b" } });
    });
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(false),
    );
    // A 后回（host=A）：必须被丢弃，输入框仍是 B 的值
    await act(async () => {
      resolvers["ep-a"]({ id: "ep-a", name: "A", connection: { host: "host-a" } });
    });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.getByDisplayValue("host-b")).toBeTruthy();
    expect(screen.queryByDisplayValue("host-a")).toBeNull();
  });
});
