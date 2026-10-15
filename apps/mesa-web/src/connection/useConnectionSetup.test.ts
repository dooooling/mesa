/**
 * useConnectionSetup 契约回归（C2）。
 *
 * 覆盖组件层不易构造的场景：
 * - 驱动清单失败 → 空清单 + 错误态 + 重试；缺 `launchable` 的驱动不可选；
 * - 切换驱动按新 Schema 重建参数，不残留旧驱动字段；
 * - 迟到的描述符响应（abort 未生效时）按代际丢弃，不覆盖当前驱动；
 * - Secret marker 存在时 probe/validate 不发请求；
 * - 结果按 (driverId, revision) 失效。
 */
import { describe, expect, it, vi, afterEach } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ConnectionValues, ProbeOutcome } from "./connectionSetupModel";
import { useConnectionSetup, type UseConnectionSetupOptions } from "./useConnectionSetup";

interface Call {
  url: string;
  method: string;
  body: unknown;
}

const NCK_FIELDS = [
  { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
  { key: "local_tsap", label: "Local TSAP", field_type: "integer", required: true, default: 1024, validation: { min: 0, max: 65535 }, ui: { order: 10, advanced: true } },
];

const S7_FIELDS = [
  { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
  { key: "rack", label: "Rack", field_type: "integer", required: false, default: 0, validation: {}, ui: { order: 2 } },
];

const OPCUA_FIELDS = [
  { key: "endpoint_url", label: "Endpoint URL", field_type: "url", required: true, validation: {}, ui: { order: 1 } },
  { key: "api_token", label: "API Token", field_type: "secret", required: false, validation: {}, ui: { order: 2 } },
];

/** 引用必须稳定：hook 把 initialValues 的引用变化当作「新快照」。 */
const OPCUA_INITIAL: ConnectionValues = {
  endpoint_url: "opc.tcp://10.0.0.2:4840",
  api_token: { secret_set: true },
};

interface ServerOpts {
  driversFail?: boolean;
  drivers?: Array<{ id: string; name: string; launchable?: boolean; reason?: string | null }>;
  descriptors?: Record<string, unknown>;
  /** 每个驱动的描述符响应闸门：resolve 前该请求一直挂起（用于迟到响应） */
  gates?: Record<string, Promise<void>>;
  probe?: () => { status: number; body: unknown };
}

function installFetch(opts: ServerOpts) {
  const calls: Call[] = [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    let body: unknown;
    try {
      body = init?.body ? JSON.parse(String(init.body)) : undefined;
    } catch {
      body = undefined;
    }
    calls.push({ url, method, body });
    if (url === "/api/v1/drivers") {
      if (opts.driversFail) {
        return { ok: false, status: 500, json: async () => ({ error: { code: "BOOM", message: "清单挂了" } }) };
      }
      return { ok: true, status: 200, json: async () => ({ drivers: opts.drivers ?? [] }) };
    }
    const desc = /^\/api\/v1\/drivers\/([^/]+)\/descriptor$/.exec(url);
    if (desc) {
      const id = desc[1];
      const gate = opts.gates?.[id];
      if (gate) await gate;
      const fields = opts.descriptors?.[id];
      if (!fields) return { ok: false, status: 404, json: async () => ({ error: { code: "NOT_FOUND", message: "no descriptor" } }) };
      return { ok: true, status: 200, json: async () => ({ connection: fields }) };
    }
    if (/\/probe$/.test(url)) {
      const r = opts.probe?.() ?? { status: 200, body: { reachable: true, device: { vendor: "Siemens" }, warnings: [] } };
      return { ok: r.status < 300, status: r.status, json: async () => r.body };
    }
    if (/\/validate-connection$/.test(url)) {
      return { ok: true, status: 200, json: async () => ({}) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  return { calls, opts };
}

function renderSetup(options: UseConnectionSetupOptions) {
  return renderHook(() => useConnectionSetup(options));
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("C2 Hook：驱动清单", () => {
  it("清单失败：清空清单 + error 态 + 可重试（不回退内置常量）", async () => {
    const srv = installFetch({ driversFail: true });
    const { result } = renderSetup({ active: true, driverSelectable: true, sessionKey: "s" });

    await waitFor(() => expect(result.current.driversStatus).toBe("error"));
    expect(result.current.drivers).toEqual([]);
    expect(result.current.driversError).toContain("清单挂了");
    expect(result.current.driverId).toBeNull();

    srv.opts.driversFail = false;
    srv.opts.drivers = [{ id: "s7", name: "S7", launchable: true }];
    srv.opts.descriptors = { s7: S7_FIELDS };
    act(() => result.current.reloadDrivers());
    await waitFor(() => expect(result.current.driversStatus).toBe("ready"));
    expect(result.current.driverId).toBe("s7");
  });

  it("只有 launchable=true 的驱动可选（缺字段视为不可启动）", async () => {
    installFetch({
      drivers: [
        { id: "ghost", name: "Ghost" },
        { id: "s7", name: "S7", launchable: true },
        { id: "dead", name: "Dead", launchable: false, reason: "二进制缺失" },
      ],
      descriptors: { s7: { fields: S7_FIELDS } },
    });
    const { result } = renderSetup({ active: true, driverSelectable: true, sessionKey: "s" });

    await waitFor(() => expect(result.current.driversStatus).toBe("ready"));
    expect(result.current.drivers.map((d) => [d.id, d.launchable])).toEqual([
      ["ghost", false],
      ["s7", true],
      ["dead", false],
    ]);
    expect(result.current.drivers.find((d) => d.id === "dead")?.reason).toBe("二进制缺失");
    // 默认选中第一个可启动驱动
    await waitFor(() => expect(result.current.driverId).toBe("s7"));
  });
});

describe("C2 Hook：驱动切换与迟到响应", () => {
  it("切换驱动按新 Schema 重建参数，不残留旧驱动字段", async () => {
    installFetch({
      drivers: [
        { id: "sinumerik-nck", name: "NCK", launchable: true },
        { id: "s7", name: "S7", launchable: true },
      ],
      descriptors: { "sinumerik-nck": { fields: NCK_FIELDS }, s7: { fields: S7_FIELDS } },
    });
    const { result } = renderSetup({ active: true, driverSelectable: true, sessionKey: "s" });

    await waitFor(() => expect(result.current.driverId).toBe("sinumerik-nck"));
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    act(() => result.current.setField("local_tsap", 1024));
    expect(result.current.raw.local_tsap).toBe(1024);

    act(() => result.current.selectDriver("s7"));
    // 切换瞬间旧驱动参数立刻清空（不给旧值任何可见窗口）
    expect(result.current.raw.local_tsap).toBeUndefined();

    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    expect(result.current.fields.map((f) => f.key)).toEqual(["host", "rack"]);
    // 只由新 Schema 的默认值物化，绝不与旧驱动参数合并
    expect(result.current.raw).toEqual({ rack: 0 });
  });

  it("迟到的描述符响应不得覆盖当前驱动（abort + 代际双保险）", async () => {
    let releaseNck: () => void = () => {};
    const nckGate = new Promise<void>((resolve) => {
      releaseNck = resolve;
    });
    installFetch({
      drivers: [
        { id: "sinumerik-nck", name: "NCK", launchable: true },
        { id: "s7", name: "S7", launchable: true },
      ],
      descriptors: { "sinumerik-nck": { fields: NCK_FIELDS }, s7: { fields: S7_FIELDS } },
      gates: { "sinumerik-nck": nckGate },
    });
    const { result } = renderSetup({ active: true, driverSelectable: true, sessionKey: "s" });

    await waitFor(() => expect(result.current.driverId).toBe("sinumerik-nck"));
    await waitFor(() => expect(result.current.descriptorStatus).toBe("loading"));

    act(() => result.current.selectDriver("s7"));
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    expect(result.current.fields.map((f) => f.key)).toEqual(["host", "rack"]);

    // 放行 NCK 的迟到响应：必须整份丢弃，字段与草稿保持 S7
    await act(async () => {
      releaseNck();
      await nckGate;
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(result.current.driverId).toBe("s7");
    expect(result.current.fields.map((f) => f.key)).toEqual(["host", "rack"]);
    expect(result.current.raw.local_tsap).toBeUndefined();
  });
});

describe("C2 Hook：Secret-safe 与结果失效", () => {
  it("存在未重新输入的 marker 时：不发探测请求，返回 secret_unavailable", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup({
      active: true,
      driverSelectable: false,
      sessionKey: "edit",
      initialDriverId: "opcua",
      initialValues: OPCUA_INITIAL,
    });

    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    await waitFor(() => expect(result.current.secretMarkers).toEqual(["api_token"]));
    expect(result.current.canTestConnection).toBe(false);

    // 用数组承接：避免 TS 把 let 变量在闭包赋值后仍收窄为 null
    const outcomes: Array<ProbeOutcome | null> = [];
    await act(async () => {
      outcomes.push(await result.current.probeConnection());
    });
    expect(outcomes[0]?.kind).toBe("secret_unavailable");
    expect(srv.calls.some((c) => c.url.endsWith("/probe"))).toBe(false);

    // 重新输入明文后解禁，且发出的是明文而不是 marker
    act(() => result.current.setField("api_token", "new-token"));
    expect(result.current.secretMarkers).toEqual([]);
    await act(async () => {
      await result.current.probeConnection();
    });
    const probeCall = srv.calls.find((c) => c.url.endsWith("/probe"));
    expect((probeCall?.body as { connection: ConnectionValues }).connection.api_token).toBe("new-token");
  });

  it("任一参数变化都让旧的校验/探测结果立即失效", async () => {
    installFetch({ drivers: [{ id: "sinumerik-nck", name: "NCK", launchable: true }], descriptors: { "sinumerik-nck": { fields: NCK_FIELDS } } });
    const { result } = renderSetup({
      active: true,
      driverSelectable: true,
      sessionKey: "s",
      initialDriverId: "sinumerik-nck",
    });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    act(() => result.current.setField("host", "10.0.0.5"));

    await act(async () => {
      await result.current.probeConnection();
    });
    expect(result.current.probe?.kind).toBe("reachable_identified");
    const revisionBefore = result.current.revision;

    act(() => result.current.setField("host", "10.0.0.9"));
    expect(result.current.revision).toBe(revisionBefore + 1);
    expect(result.current.probe).toBeNull();
    expect(result.current.validation).toBeNull();
  });

  it("snapshot() 从当前 raw 现场归一化（TSAP 文本 → 十进制）", async () => {
    installFetch({ drivers: [{ id: "sinumerik-nck", name: "NCK", launchable: true }], descriptors: { "sinumerik-nck": { fields: NCK_FIELDS } } });
    const { result } = renderSetup({
      active: true,
      driverSelectable: true,
      sessionKey: "s",
      initialDriverId: "sinumerik-nck",
    });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    act(() => result.current.setField("local_tsap", "0x0500"));
    act(() => result.current.setField("host", "10.0.0.5"));
    const snap = result.current.snapshot();
    expect(snap.issues).toEqual([]);
    expect(snap.connection.local_tsap).toBe(1280);

    // 非法输入不静默回退：产生 issue 且不写入 canonical
    act(() => result.current.setField("local_tsap", "0x"));
    const bad = result.current.snapshot();
    expect(bad.connection.local_tsap).toBeUndefined();
    expect(bad.issues.map((i) => i.path)).toContain("connection.local_tsap");
  });
});
