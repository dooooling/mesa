// M3.1 回归：全局 /data 与设备页共用 point 语义。
// - 跨设备聚合（设备/连接两列齐全）；
// - ?device=/?connection= URL 联动与级联（设备切换连接回 ALL；非法值归一）；
// - 共用 PointDetailDrawer（来源 Device/Connection/Endpoint 齐全，“打开设备”闭环）。
// 点值走 Point Live SSE 桩；inventory 仍 fetch。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { POINT_STALE_AFTER_MS } from "../deviceModel";
import App from "../App";
import { installPointLiveStub, pointLiveRow } from "../test/pointLiveStub";

const T0 = 1_700_000_000_000;

function pt(ep: string, key: string, quality: string, ageMs: number, value: unknown = 1) {
  return pointLiveRow(ep, key, quality, ageMs, value);
}

async function waitLiveSubscribed() {
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

function liveRows(rows: ReturnType<typeof pt>[]) {
  return rows.map((r) => ({ ...r }));
}

function mockGlobal() {
  return installPointLiveStub({
    points: liveRows([pt("focas", "spindle.speed", "GOOD", 500, 6000), pt("s7", "DB1.temp", "GOOD", 600, 36.5)]),
    devices: [
      { id: "cnc-01", name: "CNC-01" },
      { id: "plc-01", name: "PLC-01" },
    ],
    endpoints: [
      { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      { id: "s7", name: "S7", driver_id: "s7", device_id: "plc-01", state: "RUNNING" },
    ],
  });
}

/**
 * 两台设备、各两个连接（复审修订 1 的作用域矩阵夹具）。
 *
 * CNC-01：focas（BAD，派生 BAD）/ nck（GOOD，但夹具时间锚在过去 ⇒ STALE）
 * PLC-01：s7 / opcua（同为 STALE）
 * 计数基线：全部 4 · GOOD 0 · BAD 1 · STALE 3。
 */
function mockTwoDevices() {
  return installPointLiveStub({
    points: liveRows([
      pt("focas", "spindle.speed", "BAD", 500, 6000),
      pt("nck", "channel.1", "GOOD", 600, 1),
      pt("s7", "DB1.temp", "GOOD", 600, 36.5),
      pt("opcua", "motor.speed", "GOOD", 600, 1500),
    ]),
    devices: [
      { id: "cnc-01", name: "CNC-01" },
      { id: "plc-01", name: "PLC-01" },
    ],
    endpoints: [
      { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      { id: "nck", name: "NCK", driver_id: "sinumerik-nck", device_id: "cnc-01", state: "RUNNING" },
      { id: "s7", name: "S7", driver_id: "s7", device_id: "plc-01", state: "RUNNING" },
      { id: "opcua", name: "OPC UA", driver_id: "opcua", device_id: "plc-01", state: "RUNNING" },
    ],
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function renderApp(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

describe("M3.1 全局实时数据", () => {
  it("跨设备聚合：设备/连接/点位三列齐全", async () => {
    const stub = mockGlobal();
    renderApp("/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });
    expect(screen.getAllByText("spindle.speed").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("DB1.temp").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("PLC-01").length).toBeGreaterThanOrEqual(1);
  });

  it("?device= 过滤只留该设备点位", async () => {
    const stub = mockGlobal();
    renderApp("/data?device=plc-01");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("DB1.temp", undefined, { timeout: 30000 });
    expect(screen.getAllByText("DB1.temp").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryAllByText("spindle.speed")).toHaveLength(0);
  });

  it("STALE 语义与设备页一致（旧点 deadline 翻转）", async () => {
    const stub = installPointLiveStub({
      points: liveRows([pt("focas", "old-k", "GOOD", POINT_STALE_AFTER_MS + 5000)]),
      endpoints: [
        { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      ],
    });
    renderApp("/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("old-k", undefined, { timeout: 30000 });
    expect(screen.getAllByText("old-k").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("STALE")).toBeTruthy();
  });

  it("SSE 失败保留 last-known（不清空）", async () => {
    const stub = installPointLiveStub({
      points: liveRows([pt("focas", "edge-k", "GOOD", 29_000)]),
      endpoints: [
        { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      ],
    });
    renderApp("/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("edge-k", undefined, { timeout: 30000 });
    expect(screen.getAllByText("edge-k").length).toBeGreaterThanOrEqual(1);
    // 失败只翻 pointsError，last-known 保留；STALE 由 deadline 推进。
    act(() => stub.fail());
    await waitFor(() => expect(screen.getByText(/快照更新失败/)).toBeTruthy(), {
      timeout: 30000,
    });
    expect(screen.getAllByText("edge-k").length).toBeGreaterThanOrEqual(1);
  });

  it("计数行跟随设备 + 连接双重筛选", async () => {
    // 复审修订 1：计数行 / Popover / 导出分析使用**同一个设备 + 连接作用域**。
    // 设备=cnc-01 且连接=focas 时只能统计 focas 的点（旧实现用全局 counts ⇒ 2）。
    const stub = mockGlobal();
    renderApp("/data?device=cnc-01&connection=focas");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });
    // 夹具时间锚固定在过去 ⇒ 派生状态为 STALE；这里只断言**范围**（1 个点）。
    expect(screen.getByText(/全部 1 · GOOD 0 · BAD 0 · STALE 1/)).toBeTruthy();
    // 范围已收敛到该设备 + 该连接：标注必须写实，不得再出现"跨设备"。
    expect(screen.getByTestId("data-scope-label").textContent).toBe("统计范围：CNC-01 · FOCAS");
  });

  it("设备选中但连接 ALL：统计只含该设备的全部连接（不是跨设备）", async () => {
    // 复审修订 1 的核心口径：device=cnc-01、connection=ALL。
    // 旧实现统计范围 = 全部连接（跨设备）⇒ 4；新实现 = 该设备的 2 个连接 ⇒ 2。
    const stub = mockTwoDevices();
    renderApp("/data?device=cnc-01");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });
    // 旧文案"全部连接（跨设备）"已无意义：范围收敛到该设备，标注写实。
    const label = screen.getByTestId("data-scope-label");
    expect(label.textContent).toBe("统计范围：CNC-01 · 全部连接");
    expect(label.textContent).not.toContain("跨设备");
    // 统计只含 CNC-01 的 2 个点（BAD 1 + STALE 1），绝不是 4。
    expect(screen.getByText(/^全部 2 · GOOD 0 · BAD 1 · STALE 1$/)).toBeTruthy();
    // 另一台设备的点既不在表格里，也不在统计里。
    expect(screen.queryAllByText("DB1.temp")).toHaveLength(0);
    expect(screen.queryAllByText("motor.speed")).toHaveLength(0);
  });

  it("质量状态筛选与搜索只影响表格，不改变基础统计范围", async () => {
    // 边界 1：状态/搜索**不进入**统计作用域。否则用户搜一个点后会误以为
    // 整台设备只有一个点。
    const stub = mockTwoDevices();
    const user = userEvent.setup();
    renderApp("/data?device=cnc-01");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });

    const counts = () =>
      screen.getByText(/^全部 \d+ · GOOD \d+ · BAD \d+ · STALE \d+$/).textContent;
    expect(counts()).toBe("全部 2 · GOOD 0 · BAD 1 · STALE 1");

    // 搜索一个点：表格只剩 1 行，统计总数不变。
    await user.type(screen.getByPlaceholderText("搜索设备/连接/点位"), "spindle.speed");
    await waitFor(() => expect(screen.queryAllByText("channel.1")).toHaveLength(0), {
      timeout: 10000,
    });
    expect(screen.getByText(/实时数据 · 1\/4/)).toBeTruthy();
    expect(counts()).toBe("全部 2 · GOOD 0 · BAD 1 · STALE 1");
    // 统计范围标注也不随搜索变化。
    expect(screen.getByTestId("data-scope-label").textContent).toBe("统计范围：CNC-01 · 全部连接");

    // 状态筛选 BAD：表格只剩 BAD 行，统计仍不变。
    await user.clear(screen.getByPlaceholderText("搜索设备/连接/点位"));
    const triggers = document.querySelectorAll(".ant-select-content");
    expect(triggers.length).toBeGreaterThanOrEqual(3);
    await user.click(triggers[2] as HTMLElement); // 设备 / 连接 / 状态
    await user.click(await screen.findByTitle("BAD"));
    await waitFor(() => expect(screen.queryAllByText("channel.1")).toHaveLength(0), {
      timeout: 10000,
    });
    expect(counts()).toBe("全部 2 · GOOD 0 · BAD 1 · STALE 1");
    expect(screen.getAllByText("spindle.speed").length).toBeGreaterThanOrEqual(1);
  });

  it("导出前可见范围提示：原始响应含全部端点，分析按所选设备与连接过滤", async () => {
    // 复审修订 2：raw_scope / analysis_scope 不能只存在于文件里，
    // 用户点击导出前必须在同一视觉区域看到提示（避免误分享）。
    const stub = mockTwoDevices();
    renderApp("/data?device=cnc-01");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });

    // 点击计数行打开 Popover（antd Popover 的 trigger=click；用 fireEvent
    // 避免 user-event 的 pointer-events 检查在 jsdom 下空转）。
    act(() => {
      fireEvent.click(screen.getByText(/^全部 2 ·/));
    });
    const hint = await screen.findByTestId("export-scope-hint", undefined, { timeout: 10000 });
    expect(hint.textContent).toContain("全部端点");
    expect(hint.textContent).toContain("按所选设备与连接");
    // 提示与导出按钮在同一区域（不是藏在别处）。
    expect(hint.parentElement?.textContent).toContain("导出原始快照");
  }, 30000);

  it.each([
    // 复审修订 1 的口径表：设备 × 连接 ⇒ 统计范围。
    { device: null, connection: null, total: 4, good: 0, bad: 1, stale: 3, label: "统计范围：全部设备 · 全部连接" },
    { device: "cnc-01", connection: null, total: 2, good: 0, bad: 1, stale: 1, label: "统计范围：CNC-01 · 全部连接" },
    { device: "cnc-01", connection: "focas", total: 1, good: 0, bad: 1, stale: 0, label: "统计范围：CNC-01 · FOCAS" },
    { device: null, connection: "focas", total: 1, good: 0, bad: 1, stale: 0, label: "统计范围：全部设备 · FOCAS" },
  ])("双设备多连接 · 组合($device,$connection) ⇒ $label", async (c) => {
    const stub = mockTwoDevices();
    const qs = new URLSearchParams();
    if (c.device) qs.set("device", c.device);
    if (c.connection) qs.set("connection", c.connection);
    const q = qs.toString();
    renderApp(`/data${q ? `?${q}` : ""}`);
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });

    // 计数行 = 作用域内的点数（质量状态分布按夹具）。
    expect(
      screen.getByText(
        new RegExp(`^全部 ${c.total} · GOOD ${c.good} · BAD ${c.bad} · STALE ${c.stale}$`),
      ),
    ).toBeTruthy();
    // 范围标注必须与真实范围一致。
    expect(screen.getByTestId("data-scope-label").textContent).toBe(c.label);

    // 表格同样收敛到该范围：选了设备时，另一台设备的行绝不出现。
    if (c.device) {
      expect(screen.queryAllByText("DB1.temp")).toHaveLength(0);
      expect(screen.queryAllByText("motor.speed")).toHaveLength(0);
    }
  });

  it("抽屉打开期间收到 SSE 更新：内容随之变化（码变化 + 点消失）", async () => {
    // 缺陷 1（P1 合并阻塞）：Drawer 不得持有点击时的对象快照。
    // 打开期间 SSE 把该点码覆盖为 COMMUNICATION_LOST，抽屉必须立刻显示新码；
    // 点从 snapshot 消失后必须明确表现，不得继续显示旧值。
    vi.useRealTimers();
    const stub = installPointLiveStub({
      points: [
        pointLiveRow("focas", "spindle.speed", "BAD", 500, 6000, {
          quality_code: "5",
          value_origin: "CURRENT",
        }),
      ],
      devices: [{ id: "cnc-01", name: "CNC-01" }],
      endpoints: [
        { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      ],
    });
    const user = userEvent.setup();
    renderApp("/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });
    await user.click(screen.getAllByText("spindle.speed")[0]);
    await screen.findByText("0x05 (5)", undefined, { timeout: 10000 });

    // 断线覆盖：同一 point_id，码 5 → COMMUNICATION_LOST（delta 整行替换）。
    act(() =>
      stub.emitDelta([
        pointLiveRow("focas", "spindle.speed", "BAD", 500, 6000, {
          quality_code: "COMMUNICATION_LOST",
          value_origin: "LAST_KNOWN",
        }),
      ]),
    );
    await screen.findByText(/此前的项级码已被覆盖，不可恢复/, undefined, { timeout: 10000 });
    expect(screen.queryByText("0x05 (5)")).toBeNull();

    // snapshot 全量替换后该点消失：不得静默显示旧值。
    act(() => stub.emitSnapshot([]));
    await screen.findByText(/已从实时快照中消失/, undefined, { timeout: 10000 });
    expect(screen.queryByText(/旧值，未更新（断线前）/)).toBeNull();
  }, 30000);

  it("共用 Drawer：来源齐全，“打开设备”闭环回设备页", async () => {
    vi.useRealTimers();
    const stub = mockGlobal();
    const user = userEvent.setup();
    renderApp("/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    // 并行负载下表格渲染+轮询多轮，findBy 放宽（只放宽等待，不放宽断言）。
    // P1 后数据点列与来源列文本相同（回落），用 AllBy。
    await screen.findAllByText("spindle.speed", undefined, { timeout: 30000 });
    await user.click(screen.getAllByText("spindle.speed")[0]);
    await screen.findByText("采集设置", undefined, { timeout: 10000 });
    // 来源：设备 + 连接 + endpoint 三段
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("FOCAS").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("focas").length).toBeGreaterThanOrEqual(1);
    // Drawer 内“采集设置”闭环：进设备采集页（连接 query 保留 focas；
    // MemoryRouter 不写 window.location，用采集页内容断言）。
    // 表格行内“打开设备”与 Drawer 按钮重名，锚定 Drawer 的采集按钮。
    await user.click(screen.getByText("采集设置"));
    await screen.findByText("数据采集 · FOCAS", undefined, { timeout: 10000 });
  }, 30000);
});
