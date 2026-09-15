// 扁平设备详情回归：六个一级 Tab、无全局 Connection Context、
// 旧路由诚实 404、Header 聚合事实、各页连接筛选局部化。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import App from "../App";

const T0 = 1_700_000_000_000;

function pt(ep: string, key: string) {
  return {
    endpoint_id: ep,
    key,
    point_id: key.length,
    quality: "GOOD",
    type: "f64",
    value: 1,
    timestamp_ns: (T0 - 500) * 1e6,
  };
}

const EPS = [
  { id: "plc", name: "PLC-01", driver_id: "s7", device_id: "cnc-01", state: "RUNNING" },
  { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "STOPPED" },
];

function mockAll() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/devices/cnc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "cnc-01", name: "CNC-01" }) };
    }
    if (url === "/api/v1/devices") {
      return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
    }
    if (url === "/api/v1/endpoints") {
      return { ok: true, status: 200, json: async () => ({ endpoints: EPS }) };
    }
    if (url === "/api/v1/points/latest") {
      return { ok: true, status: 200, json: async () => ({ points: [pt("plc", "a"), pt("focas", "b")] }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  }) as unknown as typeof fetch;
}

function renderApp(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
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

describe("扁平设备详情", () => {
  it("Header 展示真实聚合 + 六个一级 Tab，无全局连接上下文", async () => {
    mockAll();
    renderApp("/devices/cnc-01/overview");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    // Header：名 + id + 聚合事实（无合成健康评分；名在面包屑/Header 多处，用 AllBy）
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("2 个连接 · 1 RUNNING · 1 STOPPED · 2 个数据点")).toBeTruthy();
    expect(screen.getByRole("button", { name: "编辑设备" })).toBeTruthy();
    // 六个 Tab（侧边栏“实时数据”重名，锚定设备 Tab 导航）
    const nav = screen.getByTestId("device-tab-nav");
    for (const label of ["概览", "实时数据", "连接", "采集", "事件", "诊断"]) {
      expect(nav.textContent ?? "").toContain(label);
    }
    // 无全局 Connection Context
    expect(screen.queryByTestId("workspace-connection-context")).toBeNull();
    expect(screen.queryByText("配置")).toBeNull();
  });

  it("旧路由不再兼容：config 回 overview（非法 tab），深链进 404", async () => {
    mockAll();
    // /config 被 :tab 吞掉 → 非法 tab 回 overview（页内 Tab 导航仍在）
    {
      const { unmount } = renderApp("/devices/cnc-01/config");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.getByText("概览")).toBeTruthy();
      expect(screen.queryByTestId("workspace-connection-context")).toBeNull();
      unmount();
    }
    // 三段深链无路由 → 诚实 404（AntD Result 文案可能拆元素，用正则。
    // 注：/legacy 是两段，被 :tab 吞掉按非法 tab 回 overview，不进 404。）
    {
      const { unmount } = renderApp("/devices/cnc-01/endpoints/plc");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.getByText(/页面不存在/)).toBeTruthy();
      unmount();
    }
  });

  it("未知路由进 NotFound（不再回总览）", async () => {
    vi.useRealTimers();
    mockAll();
    renderApp("/no-such-page");
    expect(await screen.findByText(/页面不存在/, undefined, { timeout: 10000 })).toBeTruthy();
  });

  it("Tab 切换不保留 ?connection=（各页筛选局部）", async () => {
    vi.useRealTimers();
    mockAll();
    const user = userEvent.setup();
    renderApp("/devices/cnc-01/data?connection=plc");
    await screen.findAllByText("a", undefined, { timeout: 10000 });
    // 切到概览：Link 直达无参（MemoryRouter 下用页面内容断言，不读 window.location）
    await user.click(screen.getByText("概览"));
    await screen.findByText("需要关注", undefined, { timeout: 10000 });
    // 概览页无连接筛选残留
    expect(screen.queryByTestId("workspace-connection-context")).toBeNull();
  }, 30000);

  it("连接页只管 Endpoint（增删改查/启停），无采集与事件", async () => {
    mockAll();
    renderApp("/devices/cnc-01/connections");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText("PLC-01")).toBeTruthy();
    expect(screen.getByText("FOCAS")).toBeTruthy();
    expect(screen.getByRole("button", { name: "+ 新增连接" })).toBeTruthy();
    // 禁止项不在本页
    expect(screen.queryByText("数据采集")).toBeNull();
    expect(screen.queryByText("事件订阅")).toBeNull();
    expect(screen.queryByText("高级诊断")).toBeNull();
  });

  it("采集页含数据采集 + 事件订阅两 Section", async () => {
    mockAll();
    renderApp("/devices/cnc-01/acquisition");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText("数据采集 · PLC-01")).toBeTruthy();
    expect(screen.getByText("事件订阅")).toBeTruthy();
  });
});
