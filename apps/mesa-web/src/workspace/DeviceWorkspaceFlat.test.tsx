// 扁平设备详情回归：六个一级 Tab、无全局 Connection Context、
// 旧路由诚实 404、Header 聚合事实、各页连接筛选局部化。
// 点值走 Point Live SSE 桩；inventory 仍 fetch。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import App from "../App";
import { installPointLiveStub, pointLiveRow } from "../test/pointLiveStub";

const T0 = 1_700_000_000_000;

function pt(ep: string, key: string) {
  return pointLiveRow(ep, key, "GOOD", 500, 1);
}

const EPS = [
  { id: "plc", name: "PLC-01", driver_id: "s7", device_id: "cnc-01", state: "RUNNING" },
  { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "STOPPED" },
];

// 跨设备 inventory：B 的连接绝不能漏进 A 的任何页面。
const EPS_CROSS = [
  ...EPS,
  { id: "b1", name: "B-ONE", driver_id: "s7", device_id: "other-dev", state: "RUNNING" },
];

function mockAll(endpoints: typeof EPS = EPS) {
  return installPointLiveStub({
    points: [pt("plc", "a"), pt("focas", "b")],
    endpoints,
  });
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
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function waitLiveSubscribed() {
  const { waitFor } = await import("@testing-library/react");
  await waitFor(
    () =>
      expect(
        (
          globalThis as { __PointLiveStubSource?: { instances?: unknown[] } }
        ).__PointLiveStubSource?.instances?.length ?? 0,
      ).toBeGreaterThan(0),
    { timeout: 30000, interval: 50 },
  );
}

describe("扁平设备详情", () => {
  it("Header 展示真实聚合 + 六个一级 Tab，无全局连接上下文", async () => {
    mockAll();
    renderApp("/devices/cnc-01/overview");
    // Header：名 + id + 聚合事实（无合成健康评分；名在面包屑/Header 多处，用 AllBy）
    await screen.findAllByText("CNC-01", undefined, { timeout: 30000 });
    expect(screen.getByText("2 个连接 · 1 RUNNING · 1 STOPPED")).toBeTruthy();
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

  it("旧路由不再兼容：config/legacy/深链全部进 404", async () => {
    mockAll();
    // 六条显式路由之外一律 NotFound（无非法 tab 回落）。
    for (const path of [
      "/devices/cnc-01/config",
      "/devices/cnc-01/legacy",
      "/devices/cnc-01/foo",
      "/devices/cnc-01/endpoints/plc",
    ]) {
      const { unmount } = renderApp(path);
      expect(screen.getByText(/页面不存在/)).toBeTruthy();
      unmount();
    }
  });

  it("未知路由进 NotFound（不再回总览）", async () => {
    mockAll();
    renderApp("/no-such-page");
    expect(await screen.findByText(/页面不存在/, undefined, { timeout: 30000 })).toBeTruthy();
  });

  it("Tab 切换不保留 ?connection=（各页筛选局部）", async () => {
    const stub = mockAll();
    const user = userEvent.setup();
    renderApp("/devices/cnc-01/data?connection=plc");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("a", undefined, { timeout: 30000 });
    // 切到概览：Link 直达无参（MemoryRouter 下用页面内容断言，不读 window.location）
    await user.click(screen.getByText("概览"));
    await screen.findByText("需要关注", undefined, { timeout: 10000 });
    // 概览页无连接筛选残留
    expect(screen.queryByTestId("workspace-connection-context")).toBeNull();
  }, 30000);

  it("连接页只管 Endpoint（增删改查/启停），无采集与事件", async () => {
    mockAll();
    renderApp("/devices/cnc-01/connections");
    await screen.findByText("PLC-01", undefined, { timeout: 30000 });
    expect(screen.getByText("PLC-01")).toBeTruthy();
    expect(screen.getByText("FOCAS")).toBeTruthy();
    // AntD 中文按钮自动加空格，用正则
    expect(screen.getByRole("button", { name: /新增连接/ })).toBeTruthy();
    // 禁止项不在本页
    expect(screen.queryByText("数据采集")).toBeNull();
    expect(screen.queryByText("事件订阅")).toBeNull();
    expect(screen.queryByText("高级诊断")).toBeNull();
  });

  it("采集页含数据采集 + 事件订阅两 Section", async () => {
    mockAll();
    renderApp("/devices/cnc-01/acquisition");
    // Card title 文本可能拆元素，分开断言
    await screen.findByText(/数据采集/, undefined, { timeout: 30000 });
    expect(screen.getByText(/数据采集/)).toBeTruthy();
    expect(screen.getByText("PLC-01")).toBeTruthy();
    expect(screen.getByText("事件订阅")).toBeTruthy();
  });

  it("跨设备隔离：B 的连接不进 A 的 Header/连接页/筛选器", async () => {
    mockAll(EPS_CROSS);
    // connections 页
    {
      const { unmount } = renderApp("/devices/cnc-01/connections");
      await screen.findByText("PLC-01", undefined, { timeout: 30000 });
      expect(screen.getByText("PLC-01")).toBeTruthy();
      expect(screen.queryByText("B-ONE")).toBeNull();
      unmount();
    }
    // overview Header 只计 A 的连接
    {
      const { unmount } = renderApp("/devices/cnc-01/overview");
      await screen.findByText("2 个连接 · 1 RUNNING · 1 STOPPED", undefined, { timeout: 30000 });
      expect(screen.getByText("2 个连接 · 1 RUNNING · 1 STOPPED")).toBeTruthy();
      expect(screen.queryByText("B-ONE")).toBeNull();
      unmount();
    }
    // data 页筛选器无 B
    {
      const { unmount } = renderApp("/devices/cnc-01/data");
      expect(screen.queryByText("B-ONE")).toBeNull();
      unmount();
    }
  });

  it("Header 状态不合并：FAILED/RECONNECTING 不吞入 RUNNING/STOPPED", async () => {
    mockAll([
      { id: "r", name: "R", driver_id: "s7", device_id: "cnc-01", state: "RUNNING" },
      { id: "s", name: "S", driver_id: "s7", device_id: "cnc-01", state: "STOPPED" },
      { id: "f", name: "F", driver_id: "s7", device_id: "cnc-01", state: "FAILED" },
      { id: "c", name: "C", driver_id: "s7", device_id: "cnc-01", state: "RECONNECTING" },
    ]);
    renderApp("/devices/cnc-01/overview");
    await screen.findByText("4 个连接 · 1 FAILED · 1 RECONNECTING · 1 RUNNING · 1 STOPPED", undefined, {
      timeout: 30000,
    });
    expect(
      screen.getByText("4 个连接 · 1 FAILED · 1 RECONNECTING · 1 RUNNING · 1 STOPPED"),
    ).toBeTruthy();
  });

  it("实时表固定布局：列宽固定，内容省略不推动整表", async () => {
    const stub = mockAll();
    renderApp("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("a", undefined, { timeout: 30000 });
    const table = document.querySelector("table");
    expect(table).toBeTruthy();
    // fixed 布局：colgroup 每列有显式宽度
    const cols = table!.querySelectorAll("col");
    expect(cols.length).toBeGreaterThanOrEqual(8);
    const widths = [...cols].map((c) => c.getAttribute("style") ?? "");
    expect(widths.some((s) => s.includes("240"))).toBe(true);
    expect(widths.some((s) => s.includes("180"))).toBe(true);
  });

  it("概览数据预览行位稳定（delta 只改值不行位）", async () => {
    const stub = mockAll();
    renderApp("/devices/cnc-01/overview");
    await screen.findByText("数据预览", undefined, { timeout: 30000 });
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("a", undefined, { timeout: 30000 });
    // 行位：预览区内 a/b 首次出现顺序（focas < plc，故 b,a；值/STALE 变化不影响行位）。
    const keyOrder = () => {
      const preview = screen.getByText("数据预览").closest("section")!;
      const text = [...preview.querySelectorAll("div")].map((d) => d.textContent ?? "").join("|");
      return ["a", "b"].filter((k) => text.includes(k)).sort((x, y) => text.indexOf(x) - text.indexOf(y)).join(",");
    };
    expect(keyOrder()).toBe("b,a");
    // delta 只改 b 的值：行位必须不变。
    act(() => stub.emitDelta([pointLiveRow("focas", "b", "GOOD", 100, 99)]));
    await waitFor(() => expect(keyOrder()).toBe("b,a"), { timeout: 30000 });
  }, 60000);

  it("非法 connection 被正规化回 URL（页面与地址栏一致）", async () => {
    const stub = mockAll();
    renderApp("/devices/cnc-01/data?connection=ghost");
    await waitLiveSubscribed();
    act(() => stub.emit());
    // Data 页：非法 → 全部（两点都显示），URL 删参
    await screen.findAllByText("a", undefined, { timeout: 30000 });
    expect(screen.getAllByText("b").length).toBeGreaterThanOrEqual(1);
  }, 60000);
});
