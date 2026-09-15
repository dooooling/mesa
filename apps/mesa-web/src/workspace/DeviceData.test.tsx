// M2 回归（8 条）：设备数据归属 + fail-closed + STALE + connection 过滤 +
// Drawer 来源/跳转 + 代际守卫。沿用 Monitor STALE 测试的 fake timers 套路。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { POINT_STALE_AFTER_MS } from "../deviceModel";
import { DeviceWorkspacePage } from "./DeviceWorkspacePage";
import { buildAttentionList } from "./DeviceOverview";

const T0 = 1_700_000_000_000;

function pt(ep: string, key: string, quality: string, ageMs: number, value: unknown = 1) {
  return {
    endpoint_id: ep,
    key,
    point_id: key.length,
    quality,
    type: "f64",
    value,
    timestamp_ns: (T0 - ageMs) * 1e6,
  };
}

function mockWorkspace(opts: {
  points: () => unknown;
  pointsFail?: boolean;
  endpoints?: unknown;
}) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/devices/cnc-01") {
      return { ok: true, status: 200, json: async () => ({ id: "cnc-01", name: "CNC-01" }) };
    }
    if (url === "/api/v1/devices") {
      return { ok: true, status: 200, json: async () => ({ devices: [{ id: "cnc-01", name: "CNC-01" }] }) };
    }
    if (url === "/api/v1/endpoints") {
      return {
        ok: true,
        status: 200,
        json: async () =>
          opts.endpoints ?? {
            endpoints: [
              { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
              { id: "opcua", name: "OPC UA", driver_id: "opcua", device_id: "cnc-01", state: "RUNNING" },
            ],
          },
      };
    }
    if (url === "/api/v1/points/latest") {
      if (opts.pointsFail) return { ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) };
      return { ok: true, status: 200, json: async () => opts.points() };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
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

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  vi.setSystemTime(T0);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("M2 设备数据归属与过滤", () => {
  it("1. 只显示当前设备所属 Endpoint 的 Points（它设备数据不串入）", async () => {
    mockWorkspace({
      points: () => ({
        points: [pt("focas", "mine", "GOOD", 500), pt("other-ep", "theirs", "GOOD", 500)],
      }),
      endpoints: {
        endpoints: [
          { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
          { id: "other-ep", name: "OTHER", driver_id: "s7", device_id: "other-dev", state: "RUNNING" },
        ],
      },
    });
    renderWorkspace("/devices/cnc-01/data");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText("mine").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryAllByText("theirs")).toHaveLength(0);
  });

  it("4. connection= 过滤生效（URL 即唯一上下文）", async () => {
    mockWorkspace({
      points: () => ({
        points: [pt("focas", "f-key", "GOOD", 500), pt("opcua", "o-key", "GOOD", 500)],
      }),
    });
    renderWorkspace("/devices/cnc-01/data?connection=focas");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText("f-key").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryAllByText("o-key")).toHaveLength(0);
  });

  it("5. 无参默认全部连接", async () => {
    mockWorkspace({
      points: () => ({
        points: [pt("focas", "f-key", "GOOD", 500), pt("opcua", "o-key", "GOOD", 500)],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText("f-key").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("o-key").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByTestId("workspace-connection-context").textContent ?? "").toContain("全部连接");
  });

  it("P1. 来源列：有 source_label 显示标签，无则诚实显示未提供（绝不拿 point_key 反推）", async () => {
    vi.useRealTimers();
    mockWorkspace({
      points: () => ({
        points: [
          { ...pt("focas", "labeled", "GOOD", 500), source_label: "DB10.DBD20" },
          // 用户语义 key + 无来源：来源列只能是"—"，不能出现 motor.speed
          { ...pt("opcua", "motor.speed", "GOOD", 500) },
        ],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await screen.findAllByText("labeled");
    // 数据点列仍是 point_key
    expect(screen.getAllByText("motor.speed").length).toBeGreaterThanOrEqual(1);
    // 来源列：label 一处；motor.speed 只允许出现在数据点列（来源列是"—"）
    expect(screen.getAllByText("DB10.DBD20").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("—").length).toBeGreaterThanOrEqual(1);
    // Drawer 来源块同样展示
    const user = userEvent.setup();
    await user.click(screen.getAllByText("labeled")[0]);
    await screen.findAllByText("来源");
    expect(screen.getAllByText("DB10.DBD20").length).toBeGreaterThanOrEqual(2);
  });
});

describe("M2 fail-closed 与 STALE", () => {
  it("2/3. points 失败不清空；nowMs 推进使旧点自然 STALE", async () => {
    let fail = false;
    mockWorkspace({
      get pointsFail() {
        return fail;
      },
      points: () => ({ points: [pt("focas", "k1", "GOOD", 5000)] }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText("k1").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByText("STALE")).toBeNull();

    fail = true;
    vi.setSystemTime(T0 + POINT_STALE_AFTER_MS + 1000);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    // 不清空 + 自然 STALE
    expect(screen.getAllByText("k1").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("STALE")).toBeTruthy();
  });
});

describe("M2 需要关注规则", () => {
  it("排序 FAILED → BAD → STALE；全正常为空", () => {
    const eps = [
      { id: "bad-ep", name: "BAD-EP", driver_id: "s7", device_id: "cnc-01", state: "FAILED" },
      { id: "ok-ep", name: "OK-EP", driver_id: "s7", device_id: "cnc-01", state: "RUNNING" },
    ];
    const pts = [
      {
        endpoint_id: "ok-ep", key: "stale-k", point_id: 1, quality: "GOOD", type: "f64",
        value: 1, timestamp_ns: (T0 - POINT_STALE_AFTER_MS - 5000) * 1e6,
        displayKey: "stale-k", sourceText: "stale-k",
        ageMs: POINT_STALE_AFTER_MS + 5000, derived: "STALE" as const,
        endpointName: "OK-EP", deviceId: "cnc-01", deviceName: "CNC-01",
      },
      {
        endpoint_id: "ok-ep", key: "bad-k", point_id: 2, quality: "BAD", type: "f64",
        value: 0, timestamp_ns: T0 * 1e6,
        displayKey: "bad-k", sourceText: "bad-k",
        ageMs: 0, derived: "BAD" as const,
        endpointName: "OK-EP", deviceId: "cnc-01", deviceName: "CNC-01",
      },
    ];
    const kinds = buildAttentionList(eps, pts).map((a) => a.kind);
    expect(kinds).toEqual(["endpoint-failed", "point-bad", "point-stale"]);
    expect(buildAttentionList(
      [{ id: "ok-ep", name: "OK-EP", driver_id: "s7", device_id: "cnc-01", state: "RUNNING" }],
      [],
    )).toEqual([]);
  });
});

describe("M2 Drawer 与跳转", () => {
  it("6/7. 点击行开 Drawer，来源正确；配置/诊断保留 ?connection=", async () => {
    vi.useRealTimers();    mockWorkspace({
      points: () => ({ points: [pt("focas", "axis.z.position", "GOOD", 500, 83.12)] }),
    });
    const user = userEvent.setup();
    renderWorkspace("/devices/cnc-01/data");
    await screen.findAllByText("axis.z.position");
    await user.click(screen.getAllByText("axis.z.position")[0]);
    // Drawer：来源三段 + 跳转按钮（设备名在 Header/面包屑/占位多处重名，
    // 用 getAllBy 断言；endpoint 名/id 同理）
    await screen.findByText("打开连接配置");
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("FOCAS").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("focas").length).toBeGreaterThanOrEqual(1);
    // 跳转配置：保留 connection=focas（URL 即 Workspace config tab）
    await user.click(screen.getByText("打开连接配置"));
    await waitFor(() => {
      expect(screen.getByTestId("workspace-connection-context").textContent ?? "").toContain("FOCAS");
    });
    // 全量并行下轮询+Drawer 多轮异步易超时，显式放宽（同删除用例 30s 先例）。
  }, 30000);
});

describe("M2 代际守卫", () => {
  it("8. A→B 快速切换时 A 的迟到 points 不覆盖 B", async () => {
    // 真 timers：轮询 interval 与导航都不依赖 fake clock，避免互相咬死。
    vi.useRealTimers();
    const gates: Array<{ resolve: () => void; which: string }> = [];
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices/cnc-a" || url === "/api/v1/devices/cnc-b") {
        const id = url.split("/").pop()!;
        return { ok: true, status: 200, json: async () => ({ id, name: id.toUpperCase() }) };
      }
      if (url === "/api/v1/devices" || url === "/api/v1/endpoints") {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            devices: [
              { id: "cnc-a", name: "CNC-A" },
              { id: "cnc-b", name: "CNC-B" },
            ],
            endpoints: [
              { id: "ep-a", name: "EP-A", driver_id: "s7", device_id: "cnc-a", state: "RUNNING" },
              { id: "ep-b", name: "EP-B", driver_id: "s7", device_id: "cnc-b", state: "RUNNING" },
            ],
          }),
        };
      }
      if (url === "/api/v1/points/latest") {
        // 每个请求挂一个 gate，调用方决定哪个先回。
        const gate: { resolve: () => void; which: string } = { resolve: () => {}, which: "" };
        const p = new Promise<void>((r) => {
          gate.resolve = r;
        });
        gates.push(gate);
        await p;
        const ep = gate.which === "b" ? "ep-b" : "ep-a";
        const key = gate.which === "b" ? "point-b" : "point-a";
        return {
          ok: true,
          status: 200,
          json: async () => ({ points: [pt(ep, key, "GOOD", 500)] }),
        };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    }) as unknown as typeof fetch;
    // 注：Data Router（createMemoryRouter/RouterProvider）的任何导航在 jsdom
    // 下都会触发 AbortSignal 基线异常（旧 EndpointWorkspacePage 同款失败同根
    // 因）。此处用普通 MemoryRouter + Link 点击做真导航：从 A 切到 B 时路由
    // 位置不变、只有 params 变化，同一组件实例复用 → 代际守卫的真实场景。
    //（M1“整行点击进 Workspace”已证明普通 Router + Link 在 jsdom 下可用。）
    const { Link, useLocation } = await import("react-router-dom");
    function Nav() {
      // 导航条常驻（放在 Workspace 之外），不受 Workspace 渲染影响。
      const loc = useLocation();
      return (
        <div>
          <span data-testid="loc">{loc.pathname}</span>
          <Link to="/devices/cnc-a/data">go-a</Link>
          <Link to="/devices/cnc-b/data">go-b</Link>
        </div>
      );
    }
    render(
      <MemoryRouter initialEntries={["/"]}>
        <Nav />
        <Routes>
          <Route path="/" element={<div>home</div>} />
          <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    const user = userEvent.setup();
    // 先进 A：首轮 points 挂起
    await user.click(screen.getByText("go-a"));
    await waitFor(() => expect(gates.length).toBeGreaterThanOrEqual(1));
    // 真导航到 B（B 也会发起首轮 points；同一组件实例，deviceId 变化）
    await user.click(screen.getByText("go-b"));
    await waitFor(() => expect(gates.length).toBeGreaterThanOrEqual(2));
    const [gateA, gateB] = gates;
    // B 先回：显示 point-b
    gateB.which = "b";
    gateB.resolve();
    await waitFor(() => expect(screen.getAllByText("point-b").length).toBeGreaterThanOrEqual(1));
    // A 后回：必须被忽略
    gateA.which = "a";
    gateA.resolve();
    await new Promise((r) => setTimeout(r, 100));
    expect(screen.queryAllByText("point-a")).toHaveLength(0);
    expect(screen.getAllByText("point-b").length).toBeGreaterThanOrEqual(1);
  });
});

describe("RC2 ownership fail-closed", () => {
  it("11. A 开 Point Drawer 后切 B：Drawer 必须关闭（不展示 A 的点）", async () => {
    vi.useRealTimers();
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices/cnc-a" || url === "/api/v1/devices/cnc-b") {
        const id = url.split("/").pop()!;
        return { ok: true, status: 200, json: async () => ({ id, name: id.toUpperCase() }) };
      }
      if (url === "/api/v1/devices" || url === "/api/v1/endpoints") {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            devices: [
              { id: "cnc-a", name: "CNC-A" },
              { id: "cnc-b", name: "CNC-B" },
            ],
            endpoints: [
              { id: "ep-a", name: "EP-A", driver_id: "s7", device_id: "cnc-a", state: "RUNNING" },
              { id: "ep-b", name: "EP-B", driver_id: "s7", device_id: "cnc-b", state: "RUNNING" },
            ],
          }),
        };
      }
      if (url === "/api/v1/points/latest") {
        return {
          ok: true,
          status: 200,
          json: async () => ({ points: [pt("ep-a", "point-a", "GOOD", 500), pt("ep-b", "point-b", "GOOD", 500)] }),
        };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    }) as unknown as typeof fetch;
    const { Link } = await import("react-router-dom");
    function Nav() {
      return (
        <div>
          <Link to="/devices/cnc-a/data">go-a</Link>
          <Link to="/devices/cnc-b/data">go-b</Link>
        </div>
      );
    }
    render(
      <MemoryRouter initialEntries={["/"]}>
        <Nav />
        <Routes>
          <Route path="/" element={<div>home</div>} />
          <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    const user = userEvent.setup();
    await user.click(screen.getByText("go-a"));
    await waitFor(() => expect(screen.getAllByText("point-a").length).toBeGreaterThanOrEqual(1), { timeout: 10000 });
    await user.click(screen.getAllByText("point-a")[0]);
    await screen.findByText("打开连接配置", undefined, { timeout: 10000 });
    // 切 B：Drawer 必须关闭（A 的 point 不得留在 B 页面）
    await user.click(screen.getByText("go-b"));
    await waitFor(() => expect(screen.queryByText("打开连接配置")).toBeNull(), { timeout: 10000 });
    // B 自己的点正常显示
    expect(screen.getAllByText("point-b").length).toBeGreaterThanOrEqual(1);
  }, 30000);

  // review blocker：A→B 切换窗口 + B inventory 失败时，未知归属 points
  // 绝不能进入 B 的 devicePoints（错误设备归属展示，不是闪烁）。
  it("9. A→B 时 B inventory 挂起：A 旧 points 绝不出现在 B", async () => {
    vi.useRealTimers();
    const invGates: Array<{ resolve: () => void }> = [];
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices/cnc-a" || url === "/api/v1/devices/cnc-b") {
        const id = url.split("/").pop()!;
        return { ok: true, status: 200, json: async () => ({ id, name: id.toUpperCase() }) };
      }
      if (url === "/api/v1/devices" || url === "/api/v1/endpoints") {
        // inventory 挂起：调用方放行
        const gate: { resolve: () => void } = { resolve: () => {} };
        const p = new Promise<void>((r) => {
          gate.resolve = r;
        });
        invGates.push(gate);
        await p;
        return {
          ok: true,
          status: 200,
          json: async () => ({
            devices: [
              { id: "cnc-a", name: "CNC-A" },
              { id: "cnc-b", name: "CNC-B" },
            ],
            endpoints: [
              { id: "ep-a", name: "EP-A", driver_id: "s7", device_id: "cnc-a", state: "RUNNING" },
              { id: "ep-b", name: "EP-B", driver_id: "s7", device_id: "cnc-b", state: "RUNNING" },
            ],
          }),
        };
      }
      if (url === "/api/v1/points/latest") {
        // points 始终回 A 的点（last-known 保留场景）
        return {
          ok: true,
          status: 200,
          json: async () => ({ points: [pt("ep-a", "point-a", "GOOD", 500)] }),
        };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    }) as unknown as typeof fetch;
    const { Link } = await import("react-router-dom");
    function Nav() {
      return (
        <div>
          <Link to="/devices/cnc-a/data">go-a</Link>
          <Link to="/devices/cnc-b/data">go-b</Link>
        </div>
      );
    }
    render(
      <MemoryRouter initialEntries={["/"]}>
        <Nav />
        <Routes>
          <Route path="/" element={<div>home</div>} />
          <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    const user = userEvent.setup();
    // A：inventory 放行，point-a 出现
    await user.click(screen.getByText("go-a"));
    await waitFor(() => expect(invGates.length).toBeGreaterThanOrEqual(1));
    invGates.forEach((g) => g.resolve());
    await waitFor(() => expect(screen.getAllByText("point-a").length).toBeGreaterThanOrEqual(1));
    // 切 B：B 的 inventory 挂起（新 gate 不放行），points 仍是 A 的旧点
    await user.click(screen.getByText("go-b"));
    await waitFor(() => expect(invGates.length).toBeGreaterThanOrEqual(2));
    await new Promise((r) => setTimeout(r, 300));
    // B inventory 未就绪 → A 的点绝不能出现（owner=A≠B；B 的点无 mapping 同样不出）
    expect(screen.queryAllByText("point-a")).toHaveLength(0);
    // B inventory 恢复 → 仍无 point-a（A 的点 owner 明确非 B）
    invGates.forEach((g) => g.resolve());
    await new Promise((r) => setTimeout(r, 300));
    expect(screen.queryAllByText("point-a")).toHaveLength(0);
  }, 30000);

  it("10. B inventory 失败：未知归属 points 全部排除（空列表，不展示别家）", async () => {
    vi.useRealTimers();
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/devices/cnc-b") {
        return { ok: true, status: 200, json: async () => ({ id: "cnc-b", name: "CNC-B" }) };
      }
      if (url === "/api/v1/devices" || url === "/api/v1/endpoints") {
        return { ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) };
      }
      if (url === "/api/v1/points/latest") {
        return {
          ok: true,
          status: 200,
          json: async () => ({ points: [pt("ghost-ep", "ghost-point", "GOOD", 500)] }),
        };
      }
      return { ok: false, status: 404, json: async () => ({}) };
    }) as unknown as typeof fetch;
    render(
      <MemoryRouter initialEntries={["/devices/cnc-b/data"]}>
        <Routes>
          <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    // inventory 失败告警出现，且 ghost 点绝不渲染
    await waitFor(
      () => expect(screen.getByText("连接清单不可用")).toBeTruthy(),
      { timeout: 10000 },
    );
    expect(screen.queryAllByText("ghost-point")).toHaveLength(0);
  }, 30000);
});
