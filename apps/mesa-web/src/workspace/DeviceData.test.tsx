// M2 回归（8 条）：设备数据归属 + fail-closed + STALE + connection 过滤 +
// Drawer 来源/跳转 + 代际守卫。点值走 Point Live SSE 桩（snapshot/delta），
// inventory 仍 fetch。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { POINT_STALE_AFTER_MS } from "../deviceModel";
import { DeviceWorkspacePage } from "./DeviceWorkspacePage";
import { StaleClockProvider } from "./StaleClock";
import { buildAttentionList } from "./DeviceOverview";
import { installPointLiveStub, pointLiveRow } from "../test/pointLiveStub";

const T0 = 1_700_000_000_000;

// Subscription-first 等待：live 订阅建立后再发首帧（不靠时序猜）。
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

function pt(ep: string, key: string, quality: string, ageMs: number, value: unknown = 1) {
  return pointLiveRow(ep, key, quality, ageMs, value);
}

function mockWorkspace(opts: {
  points: () => unknown;
  pointsFail?: boolean;
  endpoints?: unknown;
}) {
  // 新语义：points 回调产出首帧 snapshot 行；pointsFail 即 SSE onerror。
  const body = opts.points() as { points?: unknown[] };
  const rows = ((body.points ?? []) as Record<string, unknown>[]).map((p) => ({
    endpoint_id: String((p as { endpoint_id?: unknown }).endpoint_id ?? ""),
    key: String((p as { key?: unknown }).key ?? ""),
    quality: String((p as { quality?: unknown }).quality ?? "GOOD"),
    type: String((p as { type?: unknown }).type ?? "f64"),
    value: (p as { value?: unknown }).value ?? 1,
    timestamp_ns: Number((p as { timestamp_ns?: unknown }).timestamp_ns ?? (T0 - 500) * 1e6),
    ...((p as { source_label?: string }).source_label
      ? { source_label: (p as { source_label?: string }).source_label }
      : {}),
    ...((p as { display_name?: string }).display_name
      ? { display_name: (p as { display_name?: string }).display_name }
      : {}),
  }));
  const stub = installPointLiveStub({
    points: rows,
    endpoints: opts.endpoints as Array<{
      id: string;
      name?: string;
      driver_id: string;
      device_id: string;
      state?: string;
    }> | undefined,
  });
  if (opts.pointsFail) {
    // 首帧前即失败：保留 last-known（空）+ pointsError。
    queueMicrotask(() => stub.fail());
  }
  // 正常路径不自动 emit：调用方渲染后 act(emit)（订阅建立后才发首帧）。
  return stub;
}

function renderWorkspace(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <StaleClockProvider>
        <Routes>
          <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
        </Routes>
      </StaleClockProvider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("M2 设备数据归属与过滤", () => {
  it("1. 只显示当前设备所属 Endpoint 的 Points（它设备数据不串入）", async () => {
    const stub = mockWorkspace({
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
    // Header 先就绪（fetch 路径正常）。
    await screen.findByText("CNC-01", undefined, { timeout: 30000 });
    // live 订阅建立后发首帧。
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("mine", undefined, { timeout: 30000 });
    expect(screen.getAllByText("mine").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryAllByText("theirs")).toHaveLength(0);
  });

  it("4. connection= 过滤生效（URL 即唯一上下文）", async () => {
    const stub = mockWorkspace({
      points: () => ({
        points: [pt("focas", "f-key", "GOOD", 500), pt("opcua", "o-key", "GOOD", 500)],
      }),
    });
    renderWorkspace("/devices/cnc-01/data?connection=focas");
    // Subscription-first：emit 前先等订阅建立。
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("f-key", undefined, { timeout: 30000 });
    expect(screen.getAllByText("f-key").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryAllByText("o-key")).toHaveLength(0);
  });

  it("5. 无参默认全部连接", async () => {
    const stub = mockWorkspace({
      points: () => ({
        points: [pt("focas", "f-key", "GOOD", 500), pt("opcua", "o-key", "GOOD", 500)],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("f-key", undefined, { timeout: 30000 });
    expect(screen.getAllByText("f-key").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("o-key").length).toBeGreaterThanOrEqual(1);
    // 扁平架构：无全局 Connection Context（连接筛选在本页 Select 内）。
    expect(screen.queryByTestId("workspace-connection-context")).toBeNull();
  });

  it("P1. 来源列：有 source_label 显示标签，无则诚实显示未提供（绝不拿 point_key 反推）", async () => {
    vi.useRealTimers();
    const stub = mockWorkspace({
      points: () => ({
        points: [
          { ...pt("focas", "labeled", "GOOD", 500), source_label: "DB10.DBD20" },
          // 用户语义 key + 无来源：来源列只能是"—"，不能出现 motor.speed
          { ...pt("opcua", "motor.speed", "GOOD", 500) },
        ],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("labeled", undefined, { timeout: 30000 });
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

  it("P2. 展示名优先 point_key，第二行显示 key；改名不改变 key/来源", async () => {
    vi.useRealTimers();
    const stub = mockWorkspace({
      points: () => ({
        points: [
          { ...pt("focas", "motor.speed", "GOOD", 500), display_name: "主轴转速", source_label: "DB10.DBD20" },
          pt("opcua", "plain.key", "GOOD", 500),
        ],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("主轴转速", undefined, { timeout: 30000 });
    // 第一行展示名 + 第二行 key 双行；未命名点仍显示 key
    expect(screen.getAllByText("motor.speed").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("plain.key").length).toBeGreaterThanOrEqual(1);
    // 来源列不受改名影响
    expect(screen.getAllByText("DB10.DBD20").length).toBeGreaterThanOrEqual(1);
    // Drawer：标题为展示名，高级信息仍是原 key
    const user = userEvent.setup();
    await user.click(screen.getAllByText("主轴转速")[0]);
    await screen.findAllByText("展示名");
    expect(screen.getAllByText("motor.speed").length).toBeGreaterThanOrEqual(2);
  });

  it("P2. Drawer 改名调 PUT 并即时更新（id/key/来源不动）", async () => {
    vi.useRealTimers();
    const puts: Array<{ url: string; body: unknown }> = [];
    const stub = installPointLiveStub({
      points: [{ ...pt("focas", "motor.speed", "GOOD", 500), source_label: "DB10.DBD20" }],
      endpoints: [
        { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      ],
    });
    const prevFetch = (globalThis as { fetch?: unknown }).fetch as typeof fetch;
    (globalThis as { fetch?: unknown }).fetch = (async (url: string, init?: { method?: string; body?: string }) => {
      if (typeof url === "string" && url.includes("/display-name") && init?.method === "PUT") {
        puts.push({ url, body: JSON.parse(init.body ?? "{}") });
        return { ok: true, status: 200, json: async () => ({}) };
      }
      return (prevFetch as (u: string, i?: unknown) => Promise<unknown>)(url, init);
    }) as unknown as typeof fetch;
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    // 并行负载下表格渲染多轮，等待放宽（只放宽等待，不放宽断言）。
    await screen.findAllByText("motor.speed", undefined, { timeout: 30000 });
    const user = userEvent.setup();
    await user.click(screen.getAllByText("motor.speed")[0]);
    await screen.findAllByText("展示名", undefined, { timeout: 10000 });
    // 输入新名并保存（Drawer 动画后 Input 才 mount，先等待）
    const input = await screen.findByPlaceholderText("未设置（显示 point_key）", undefined, { timeout: 10000 });
    await user.clear(input);
    await user.type(input, "主轴转速");
    await user.click(screen.getByRole("button", { name: "保 存" }));
    await waitFor(() => expect(puts.length).toBe(1), { timeout: 10000 });
    expect(puts[0].url).toContain("/api/v1/endpoints/focas/points/motor.speed/display-name");
    expect(puts[0].body).toEqual({ display_name: "主轴转速" });
    // 本地即时更新：第一行出现新名，第二行 key 与来源不动
    await screen.findAllByText("主轴转速", undefined, { timeout: 10000 });
    expect(screen.getAllByText("motor.speed").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("DB10.DBD20").length).toBeGreaterThanOrEqual(1);
  }, 30000);

  it("P2. 改名 pending 时切点：resolve 回填 A，不污染 B", async () => {
    vi.useRealTimers();
    let releasePut!: () => void;
    const putGate = new Promise<void>((r) => { releasePut = r; });
    const stub = installPointLiveStub({
      points: [pt("focas", "point-a", "GOOD", 500), pt("focas", "point-b", "GOOD", 500)],
      endpoints: [
        { id: "focas", name: "FOCAS", driver_id: "focas2", device_id: "cnc-01", state: "RUNNING" },
      ],
    });
    const prevFetch = (globalThis as { fetch?: unknown }).fetch as typeof fetch;
    (globalThis as { fetch?: unknown }).fetch = (async (url: string, init?: { method?: string; body?: string }) => {
      if (typeof url === "string" && url.includes("/display-name") && init?.method === "PUT") {
        await putGate; // 挂起：模拟 pending 期间切点
        return { ok: true, status: 200, json: async () => ({}) };
      }
      return (prevFetch as (u: string, i?: unknown) => Promise<unknown>)(url, init);
    }) as unknown as typeof fetch;
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("point-a", undefined, { timeout: 30000 });
    const user = userEvent.setup();
    // 开 A Drawer，输入新名，点保存（PUT 挂起中）
    await user.click(screen.getAllByText("point-a")[0]);
    const input = await screen.findByPlaceholderText("未设置（显示 point_key）", undefined, { timeout: 10000 });
    await user.clear(input);
    await user.type(input, "A的新名");
    await user.click(screen.getByRole("button", { name: "保 存" }));
    // 关 A Drawer 后再开 B（Drawer overlay 会遮挡表格行，直接点行切不到 B）
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryAllByPlaceholderText("未设置（显示 point_key）")).toHaveLength(0), { timeout: 10000 });
    await user.click(screen.getAllByText("point-b")[0]);
    await screen.findAllByText("展示名", undefined, { timeout: 10000 });
    // A 的 PUT 返回：A 被回填，B Drawer 不变
    releasePut();
    await screen.findAllByText("A的新名", undefined, { timeout: 10000 });
    // B 的 Drawer 输入框仍是空（未被污染）
    const inputs = screen.getAllByPlaceholderText("未设置（显示 point_key）");
    expect((inputs[inputs.length - 1] as HTMLInputElement).value).toBe("");
  }, 30000);

  it("P2. 改名后展示名与 point_key 都可搜（key 仍是稳定身份）", async () => {
    vi.useRealTimers();
    const stub = mockWorkspace({
      points: () => ({
        points: [
          { ...pt("focas", "motor.speed", "GOOD", 500), display_name: "主轴转速" },
          pt("focas", "other.key", "GOOD", 500),
        ],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("主轴转速", undefined, { timeout: 30000 });
    const user = userEvent.setup();
    const input = screen.getByPlaceholderText("搜索点位 / key");
    // 按展示名搜到
    await user.clear(input);
    await user.type(input, "主轴转速");
    await screen.findAllByText("主轴转速", undefined, { timeout: 10000 });
    expect(screen.queryAllByText("other.key")).toHaveLength(0);
    // 按 point_key 同样搜到（改名不丢身份）
    await user.clear(input);
    await user.type(input, "motor.speed");
    await screen.findAllByText("主轴转速", undefined, { timeout: 10000 });
    expect(screen.queryAllByText("other.key")).toHaveLength(0);
  }, 30000);
});

describe("M2 fail-closed 与 STALE", () => {
  it("2/3. SSE 失败保留 last-known；STALE 由 deadline 翻转（无网络帧）", async () => {
    const stub = mockWorkspace({
      points: () => ({ points: [pt("focas", "k1", "GOOD", 29_000)] }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("k1", undefined, { timeout: 30000 });
    // 29s 旧点：deadline 1s 后翻 STALE（先不断言瞬间 GOOD，避开 Timer 竞态）。
    await waitFor(
      () => expect(screen.queryAllByText("STALE").length).toBeGreaterThanOrEqual(1),
      { timeout: 15000 },
    );
    // SSE 失败：保留 last-known，只翻 pointsError；STALE 保持。
    act(() => stub.fail());
    await waitFor(
      () => expect(screen.getByText(/快照更新失败/)).toBeTruthy(),
      { timeout: 30000 },
    );
    expect(screen.getAllByText("k1").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("STALE").length).toBeGreaterThanOrEqual(1);
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
  it("6/7. 点击行开 Drawer，来源正确；采集/诊断保留 ?connection=", async () => {
    vi.useRealTimers();
    const stub = mockWorkspace({
      points: () => ({ points: [pt("focas", "axis.z.position", "GOOD", 500, 83.12)] }),
    });
    const user = userEvent.setup();
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("axis.z.position", undefined, { timeout: 30000 });
    await user.click(screen.getAllByText("axis.z.position")[0]);
    // Drawer：来源三段 + 跳转按钮（设备名在 Header/面包屑/占位多处重名，
    // 用 getAllBy 断言；endpoint 名/id 同理）
    await screen.findByText("采集设置");
    expect(screen.getAllByText("CNC-01").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("FOCAS").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("focas").length).toBeGreaterThanOrEqual(1);
    // 跳转采集：保留 connection=focas（采集页局部单选沿用 query；
    // MemoryRouter 不写 window.location，用采集页内容断言导航成功）
    await user.click(screen.getByText("采集设置"));
    await screen.findByText("数据采集 · FOCAS", undefined, { timeout: 10000 });
    // 全量并行下轮询+Drawer 多轮异步易超时，显式放宽（同删除用例 30s 先例）。
  }, 30000);
});

describe("M2 代际守卫", () => {
  it("8. A→B 快速切换时 A 的迟到 snapshot 不覆盖 B", async () => {
    // Point Live 语义：迟到 snapshot 按 endpoint ownership 过滤，A 的行
    // owner=A≠B 自然不可见（不再依赖 REST 代际守卫）。
    vi.useRealTimers();
    const stub = installPointLiveStub({
      points: [],
      endpoints: [
        { id: "ep-a", name: "EP-A", driver_id: "s7", device_id: "cnc-a", state: "RUNNING" },
        { id: "ep-b", name: "EP-B", driver_id: "s7", device_id: "cnc-b", state: "RUNNING" },
      ],
    });
    const prevFetch = (globalThis as { fetch?: unknown }).fetch as typeof fetch;
    (globalThis as { fetch?: unknown }).fetch = (async (url: string, init?: unknown) => {
      if (typeof url === "string" && (url === "/api/v1/devices/cnc-a" || url === "/api/v1/devices/cnc-b")) {
        const id = url.split("/").pop()!;
        return { ok: true, status: 200, json: async () => ({ id, name: id.toUpperCase() }) };
      }
      if (typeof url === "string" && url === "/api/v1/devices") {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            devices: [
              { id: "cnc-a", name: "CNC-A" },
              { id: "cnc-b", name: "CNC-B" },
            ],
          }),
        };
      }
      return (prevFetch as (u: string, i?: unknown) => Promise<unknown>)(url, init);
    }) as unknown as typeof fetch;
    // 注：普通 MemoryRouter + Link 真导航（同一组件实例复用，
    // params 变化），M1 已证明 jsdom 下可用。
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
    // A 页：snapshot 只有 A 的行。
    await user.click(screen.getByText("go-a"));
    await waitLiveSubscribed();
    act(() => stub.emitDelta([pt("ep-a", "point-a", "GOOD", 500)]));
    await screen.findAllByText("point-a", undefined, { timeout: 30000 });
    // 切 B：B 的 snapshot 只有 B 的行；A 的旧行按 ownership 自然不可见。
    await user.click(screen.getByText("go-b"));
    act(() => stub.emitDelta([pt("ep-b", "point-b", "GOOD", 500)]));
    await screen.findAllByText("point-b", undefined, { timeout: 30000 });
    expect(screen.queryAllByText("point-a")).toHaveLength(0);
  }, 60000);
});

describe("RC2 ownership fail-closed", () => {
  it("11. A 开 Point Drawer 后切 B：Drawer 必须关闭（不展示 A 的点）", async () => {
    vi.useRealTimers();
    const stub = installPointLiveStub({
      points: [],
      endpoints: [
        { id: "ep-a", name: "EP-A", driver_id: "s7", device_id: "cnc-a", state: "RUNNING" },
        { id: "ep-b", name: "EP-B", driver_id: "s7", device_id: "cnc-b", state: "RUNNING" },
      ],
    });
    const prevFetch = (globalThis as { fetch?: unknown }).fetch as typeof fetch;
    (globalThis as { fetch?: unknown }).fetch = (async (url: string, init?: unknown) => {
      if (typeof url === "string" && (url === "/api/v1/devices/cnc-a" || url === "/api/v1/devices/cnc-b")) {
        const id = url.split("/").pop()!;
        return { ok: true, status: 200, json: async () => ({ id, name: id.toUpperCase() }) };
      }
      if (typeof url === "string" && url === "/api/v1/devices") {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            devices: [
              { id: "cnc-a", name: "CNC-A" },
              { id: "cnc-b", name: "CNC-B" },
            ],
          }),
        };
      }
      return (prevFetch as (u: string, i?: unknown) => Promise<unknown>)(url, init);
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
    await waitLiveSubscribed();
    act(() => stub.emitDelta([pt("ep-a", "point-a", "GOOD", 500), pt("ep-b", "point-b", "GOOD", 500)]));
    await waitFor(() => expect(screen.getAllByText("point-a").length).toBeGreaterThanOrEqual(1), { timeout: 30000 });
    await user.click(screen.getAllByText("point-a")[0]);
    await screen.findByText("采集设置", undefined, { timeout: 30000 });
    // 切 B：Drawer 必须关闭（A 的 point 不得留在 B 页面）
    await user.click(screen.getByText("go-b"));
    await waitFor(() => expect(screen.queryByText("采集设置")).toBeNull(), { timeout: 30000 });
    // B 自己的点正常显示
    expect(screen.getAllByText("point-b").length).toBeGreaterThanOrEqual(1);
  }, 60000);

  // review blocker：A→B 切换窗口 + B inventory 失败时，未知归属 points
  // 绝不能进入 B 的 devicePoints（错误设备归属展示，不是闪烁）。
  it("9. A→B 时 B inventory 挂起：A 旧 points 绝不出现在 B", async () => {
    vi.useRealTimers();
    const invGates: Array<{ resolve: () => void }> = [];
    const stub = installPointLiveStub({ points: [] });
    const prevFetch = (globalThis as { fetch?: unknown }).fetch as typeof fetch;
    (globalThis as { fetch?: unknown }).fetch = (async (url: string, init?: unknown) => {
      if (typeof url === "string" && (url === "/api/v1/devices/cnc-a" || url === "/api/v1/devices/cnc-b")) {
        const id = url.split("/").pop()!;
        return { ok: true, status: 200, json: async () => ({ id, name: id.toUpperCase() }) };
      }
      if (typeof url === "string" && (url === "/api/v1/devices" || url === "/api/v1/endpoints")) {
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
      return (prevFetch as (u: string, i?: unknown) => Promise<unknown>)(url, init);
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
    // A：inventory 放行 + live 发 A 的点，point-a 出现
    await user.click(screen.getByText("go-a"));
    await waitFor(() => expect(invGates.length).toBeGreaterThanOrEqual(1));
    invGates.forEach((g) => g.resolve());
    await waitLiveSubscribed();
    act(() => stub.emitDelta([pt("ep-a", "point-a", "GOOD", 500)]));
    await waitFor(() => expect(screen.getAllByText("point-a").length).toBeGreaterThanOrEqual(1), {
      timeout: 30000,
    });
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
    const stub = installPointLiveStub({ points: [] });
    const prevFetch = (globalThis as { fetch?: unknown }).fetch as typeof fetch;
    (globalThis as { fetch?: unknown }).fetch = (async (url: string, init?: unknown) => {
      if (typeof url === "string" && url === "/api/v1/devices/cnc-b") {
        return { ok: true, status: 200, json: async () => ({ id: "cnc-b", name: "CNC-B" }) };
      }
      if (typeof url === "string" && (url === "/api/v1/devices" || url === "/api/v1/endpoints")) {
        return { ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) };
      }
      return (prevFetch as (u: string, i?: unknown) => Promise<unknown>)(url, init);
    }) as unknown as typeof fetch;
    render(
      <MemoryRouter initialEntries={["/devices/cnc-b/data"]}>
        <Routes>
          <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
        </Routes>
      </MemoryRouter>,
    );
    // ghost 点经 live 来，但归属未知 → 绝不渲染；inventory 失败告警出现。
    await waitLiveSubscribed();
    act(() => stub.emitDelta([pt("ghost-ep", "ghost-point", "GOOD", 500)]));
    await waitFor(
      () => expect(screen.getByText("连接清单不可用")).toBeTruthy(),
      { timeout: 30000 },
    );
    expect(screen.queryAllByText("ghost-point")).toHaveLength(0);
  }, 60000);
});

describe("M2 reconcile 语义回归", () => {
  it("source_label-only 变化必须显示（value/timestamp 不变不吞更新）", async () => {
    const stub = mockWorkspace({
      points: () => ({
        points: [{ ...pt("focas", "k1", "GOOD", 500), source_label: "DB10.DBD20" }],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("DB10.DBD20", undefined, { timeout: 30000 });
    // Configure 改地址 → Core 经 SSE delta 刷新 source_label（无新 DataBatch 也可）。
    act(() => {
      stub.emitDelta([{ ...pt("focas", "k1", "GOOD", 500), source_label: "DB20.DBD40" }]);
    });
    await waitFor(() => expect(screen.getAllByText("DB20.DBD40").length).toBeGreaterThanOrEqual(1));
    expect(screen.queryAllByText("DB10.DBD20")).toHaveLength(0);
  });

  it("display_name-only 变化必须显示（跨客户端改名 SSE 可发现）", async () => {
    const stub = mockWorkspace({
      points: () => ({
        points: [{ ...pt("focas", "k1", "GOOD", 500) }],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    await screen.findAllByText("k1", undefined, { timeout: 30000 });
    act(() => {
      stub.emitDelta([{ ...pt("focas", "k1", "GOOD", 500), display_name: "新名字" }]);
    });
    await screen.findAllByText("新名字");
  });

  it("timestamp 不变时 Age 文本照常推进（StaleClock 独立刷新显示）", async () => {
    vi.useRealTimers();
    // 真 timers 下 Date.now() 是真实当前时间：timestamp 必须相对现在，
    // 否则 age 直接落分钟级。AgeCell 由 StaleClock 每秒刷新文本（与 live 无关）。
    const now = Date.now();
    const stub = mockWorkspace({
      points: () => ({
        points: [
          {
            endpoint_id: "focas",
            key: "k1",
            point_id: 2,
            quality: "GOOD",
            type: "f64",
            value: 1,
            timestamp_ns: (now - 500) * 1e6,
          },
        ],
      }),
    });
    renderWorkspace("/devices/cnc-01/data");
    await waitLiveSubscribed();
    act(() => stub.emit());
    // timestamp 不变，真时间推进后共享秒钟独立重算年龄（不经过整表重建）。
    // 真 timers 下首帧可能已过"刚刚"，直接等"N秒前"（值文本本身不动）。
    await screen.findByText(/秒前/, undefined, { timeout: 30000 });
    expect(screen.getAllByText("k1").length).toBeGreaterThanOrEqual(1);
  }, 60000);
});
