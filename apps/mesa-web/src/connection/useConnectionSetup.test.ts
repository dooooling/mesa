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
import {
  useConnectionSetup,
  type UseConnectionSetupOptions,
  type ValidateOutcome,
} from "./useConnectionSetup";

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
  /** C3-C：Endpoint-bound `connection-check` 的响应（默认 200 校验通过） */
  connectionCheck?: () => { status: number; body: unknown };
  /** C3-C：`connection-check` 响应闸门（用于「切 Endpoint 后迟到响应」） */
  checkGate?: Promise<void>;
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
    if (/\/connection-check$/.test(url)) {
      if (opts.checkGate) await opts.checkGate;
      const r = opts.connectionCheck?.() ?? { status: 200, body: { valid: true, issues: [] } };
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

// ---------------------------------------------------------------------------
// P0：Hook 级前置门 —— 清单/描述符不可用时 validate / probe / 提交快照全被挡住
// （按钮的 disabled 只是 UI 表达；这里是「提交函数自身」的守卫）
// ---------------------------------------------------------------------------

describe("C2 Hook：P0 前置门（不发任何请求）", () => {
  it("驱动清单失败：blockReason 明确，validate/probe/submitSnapshot 全部拒绝且零写请求", async () => {
    const srv = installFetch({ driversFail: true });
    const { result } = renderSetup({ active: true, driverSelectable: true, sessionKey: "s" });
    await waitFor(() => expect(result.current.driversStatus).toBe("error"));

    expect(result.current.blockReason).toBe("catalog-unavailable");
    const validations: ValidateOutcome[] = [];
    const probes: Array<ProbeOutcome | null> = [];
    await act(async () => {
      validations.push(await result.current.validate());
    });
    await act(async () => {
      probes.push(await result.current.probeConnection());
    });

    expect(validations[0].ok).toBe(false);
    // 明确表达「未发起探测」，不得伪装成设备不可达
    expect(probes[0]?.kind).toBe("infrastructure");
    expect(result.current.submitSnapshot()).toBeNull();
    // 除了清单 GET，一个请求都没发出去
    expect(srv.calls.filter((c) => c.method !== "GET")).toEqual([]);
    expect(srv.calls.some((c) => c.url.endsWith("/probe"))).toBe(false);
    expect(srv.calls.some((c) => c.url.endsWith("/validate-connection"))).toBe(false);
  });

  it("描述符失败：同样零请求，submitSnapshot 为 null", async () => {
    const srv = installFetch({
      drivers: [{ id: "s7", name: "S7", launchable: true }],
      descriptors: {},
    });
    const { result } = renderSetup({ active: true, driverSelectable: true, sessionKey: "s" });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("error"));

    expect(result.current.blockReason).toBe("descriptor-unavailable");
    await act(async () => {
      expect((await result.current.validate()).ok).toBe(false);
    });
    await act(async () => {
      expect((await result.current.probeConnection())?.kind).toBe("infrastructure");
    });
    expect(result.current.submitSnapshot()).toBeNull();
    expect(srv.calls.filter((c) => c.method !== "GET")).toEqual([]);
  });

  it("清单恢复后 submitSnapshot 才产出负载（门是活的，不是永久关闭）", async () => {
    const srv = installFetch({
      drivers: [{ id: "s7", name: "S7", launchable: true }],
      descriptors: { s7: { fields: S7_FIELDS } },
    });
    const { result } = renderSetup({ active: true, driverSelectable: true, sessionKey: "s" });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    expect(result.current.blockReason).toBeNull();
    act(() => result.current.setField("host", "10.0.0.9"));

    const snap = result.current.submitSnapshot();
    expect(snap).not.toBeNull();
    expect(snap?.connection.host).toBe("10.0.0.9");
    // 新建/新增路径不做隐藏字段补齐（preserveHiddenValues 默认 false）
    expect(srv.calls.some((c) => c.url.endsWith("/probe"))).toBe(false);
  });

  it("edit 模式（driverSelectable=false）不看清单，但描述符失败仍挡住提交快照", async () => {
    installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup({
      active: true,
      driverSelectable: false,
      sessionKey: "edit",
      initialDriverId: "opcua",
      initialValues: OPCUA_INITIAL,
    });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    expect(result.current.blockReason).toBeNull();

    installFetch({ descriptors: {} });
    act(() => result.current.reloadDescriptor());
    await waitFor(() => expect(result.current.descriptorStatus).toBe("error"));
    expect(result.current.blockReason).toBe("descriptor-unavailable");
    expect(result.current.submitSnapshot()).toBeNull();
  });

  it("submitSnapshot：编辑态补齐 visible_if 隐藏字段，并丢弃 Descriptor 之外的字段", async () => {
    const FIELDS = [
      { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
      { key: "use_native", label: "Use native", field_type: "boolean", required: false, default: false, validation: {}, ui: { order: 2 } },
      {
        key: "native_pdu",
        label: "PDU",
        field_type: "integer",
        required: false,
        validation: { min: 1, max: 65535 },
        ui: { order: 3, visible_if: { field: "use_native", op: "eq", value: true } },
      },
    ];
    installFetch({ descriptors: { opcua: { fields: FIELDS } } });
    const initial: ConnectionValues = {
      host: "10.0.0.2",
      use_native: false,
      native_pdu: 8192,
      legacy_field: "old",
    };
    const { result } = renderSetup({
      active: true,
      driverSelectable: false,
      sessionKey: "edit",
      initialDriverId: "opcua",
      initialValues: initial,
      preserveHiddenValues: true,
    });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    const snap = result.current.submitSnapshot();
    expect(snap?.connection.native_pdu).toBe(8192);
    expect("legacy_field" in (snap?.connection ?? {})).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// P1-2：隐藏字段补齐的上下文 —— 「用户清除」与「程序初始化为空」必须区分
//
// 场景构造：`driverSelectable=true` 时描述符到达会按新 Schema **程序物化**默认值
// （`withDriverChange`），因此草稿里可能出现「用户从未触碰、却是空值」的字段。
// 这与「用户在字段可见时明确清空」在草稿里长得一模一样，只能靠显式的用户改动
// 记录（`Draft.touched`）区分。
// ---------------------------------------------------------------------------

describe("C2 Hook：隐藏字段的清除判定（P1-2）", () => {
  const FIELDS = [
    { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
    { key: "use_native", label: "Use native", field_type: "boolean", required: false, default: false, validation: {}, ui: { order: 2 } },
    {
      key: "note",
      label: "备注",
      field_type: "string",
      required: false,
      // default "" 让「程序物化出空值」这一情形真实可达
      default: "",
      validation: {},
      ui: { order: 3, visible_if: { field: "use_native", op: "eq", value: true } },
    },
  ];

  function renderPreserve() {
    installFetch({
      drivers: [{ id: "s7", name: "S7", launchable: true }],
      descriptors: { s7: { fields: FIELDS } },
    });
    return renderSetup({
      active: true,
      driverSelectable: true,
      sessionKey: "s",
      initialDriverId: "s7",
      initialValues: { use_native: false, note: "服务端旧值" },
      preserveHiddenValues: true,
    });
  }

  it("③ 初始化得到的空值不是清除：用服务端快照旧值兜底，不得误删", async () => {
    const { result } = renderPreserve();
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    // 程序物化（默认值）写出的空串——用户从未触碰 note
    expect(result.current.raw.note).toBe("");

    const snap = result.current.submitSnapshot();
    expect(snap?.connection.note).toBe("服务端旧值");
  });

  it("② 用户在字段可见时明确清空、随后隐藏 → 明确清除优先，删除该字段", async () => {
    const { result } = renderPreserve();
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    act(() => result.current.setField("use_native", true)); // 字段变为可见
    act(() => result.current.setField("note", "临时输入")); // 用户输入
    act(() => result.current.setField("note", "")); // 可见时明确清空
    act(() => result.current.setField("use_native", false)); // 之后再让条件隐藏它

    const snap = result.current.submitSnapshot();
    // key 缺席 → 下游 cleanConnection 后即从服务端删除（不被快照旧值兜底）
    expect(snap && "note" in snap.connection).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// C3-C：Endpoint-bound 检查通道
//
// 纪律（两条通道判据不同，不可互相污染）：
// - `endpointId` 非空（编辑既有连接）→ `connection-check`，规范 marker 合法；
// - `endpointId` 为空（新建 / 新增）→ Driver-bound，marker 一律禁测。
// 另：`touched + 空 Secret` 必须在**两个出口**（校验/测试 与 保存）都表现为
// `{"clear_secret":true}`，否则「测试说不需要密码」与「保存后旧密码还在」会矛盾。
// ---------------------------------------------------------------------------

describe("C3-C Hook：Endpoint-bound 检查通道", () => {
  /** 编辑既有 Endpoint 的典型 options：驱动不可切、带服务端快照。 */
  function editOptions(endpointId: string | null): UseConnectionSetupOptions {
    return {
      active: true,
      driverSelectable: false,
      sessionKey: `edit-connection:${endpointId ?? "none"}`,
      initialDriverId: "opcua",
      initialValues: OPCUA_INITIAL,
      preserveHiddenValues: true,
      endpointId,
    };
  }

  it("endpointId 非空：probe/validate 走 connection-check，marker 原样发出，Driver-bound 零调用", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup(editOptions("e1"));

    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    await waitFor(() => expect(result.current.secretMarkers).toEqual(["api_token"]));
    // marker 仍被识别为「已保存凭据」，但在本通道**不再**禁测
    expect(result.current.checkTarget).toBe("endpoint");
    expect(result.current.canTestConnection).toBe(true);

    const outcomes: Array<ProbeOutcome | null> = [];
    await act(async () => {
      outcomes.push(await result.current.probeConnection());
    });
    const check = srv.calls.find((c) => c.url.endsWith("/connection-check"));
    expect(check?.url).toBe("/api/v1/endpoints/e1/connection-check");
    const body = check?.body as { mode: string; connection: ConnectionValues };
    expect(body.mode).toBe("probe");
    // marker 原样交给服务端物化（前端绝不自行把 marker 当密码，也不删掉它）
    expect(body.connection.api_token).toEqual({ secret_set: true });
    expect(body.connection.endpoint_url).toBe("opc.tcp://10.0.0.2:4840");

    await act(async () => {
      await result.current.validate();
    });
    const modes = srv.calls
      .filter((c) => c.url.endsWith("/connection-check"))
      .map((c) => (c.body as { mode: string }).mode);
    expect(modes).toEqual(["probe", "validate"]);

    // 用户重新输入明文 → 发出明文（marker 被替换），而不是 marker
    act(() => result.current.setField("api_token", "new-token"));
    await act(async () => {
      await result.current.probeConnection();
    });
    const checks = srv.calls.filter((c) => c.url.endsWith("/connection-check"));
    const plain = checks[checks.length - 1];
    expect((plain?.body as { connection: ConnectionValues }).connection.api_token).toBe("new-token");
    // Driver-bound 通道一步都没走
    expect(srv.calls.some((c) => c.url.endsWith("/probe"))).toBe(false);
    expect(srv.calls.some((c) => c.url.endsWith("/validate-connection"))).toBe(false);
  });

  it("无 endpointId（新建/新增）：marker 仍禁测，且零检查请求（既有边界不回归）", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup({ ...editOptions(null), endpointId: undefined });

    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    await waitFor(() => expect(result.current.secretMarkers).toEqual(["api_token"]));
    expect(result.current.checkTarget).toBe("driver");
    expect(result.current.canTestConnection).toBe(false);

    const outcomes: Array<ProbeOutcome | null> = [];
    await act(async () => {
      outcomes.push(await result.current.probeConnection());
    });
    expect(outcomes[0]?.kind).toBe("secret_unavailable");
    expect(srv.calls.filter((c) => c.method === "POST")).toEqual([]);
  });

  it("Endpoint-bound：非 Secret 字段上的 marker 仍被本地拦下（不发请求）", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup(editOptions("e1"));
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    // 把 marker 放到非 Secret 字段上（前端产物污染/字段声明写错的典型形态）
    act(() => result.current.setRaw({ endpoint_url: { secret_set: true } }));
    await waitFor(() => expect(result.current.canTestConnection).toBe(false));

    const outcomes: Array<ProbeOutcome | null> = [];
    await act(async () => {
      outcomes.push(await result.current.probeConnection());
    });
    expect(outcomes[0]?.kind).toBe("invalid");
    expect(outcomes[0] && "detail" in outcomes[0] ? outcomes[0].detail : "").toContain(
      "endpoint_url",
    );
    expect(srv.calls.some((c) => c.url.endsWith("/connection-check"))).toBe(false);
  });

  it("touched + 空 Secret：校验/测试载荷与保存载荷都带 {clear_secret:true}", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup(editOptions("e1"));
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    // 用户在凭据输入框里把内容删空（touched + 空值）→ 明确清除意图
    act(() => result.current.setRaw({ ...OPCUA_INITIAL, api_token: "" }));

    // ① 检查出口
    await act(async () => {
      await result.current.validate();
    });
    const check = srv.calls.find((c) => c.url.endsWith("/connection-check"));
    const checkBody = check?.body as { connection: ConnectionValues };
    expect(checkBody.connection.api_token).toEqual({ clear_secret: true });

    // ② 保存出口（同一份语义：不得一个清、一个继承）
    const submit = result.current.submitSnapshot();
    expect(submit?.connection.api_token).toEqual({ clear_secret: true });
  });

  it("未触碰的空 Secret 不是清除：保存载荷里不得凭空出现 clear 标记", async () => {
    installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup({
      ...editOptions("e1"),
      // 程序装载出来的空值（用户从未触碰）
      initialValues: { endpoint_url: "opc.tcp://10.0.0.2:4840", api_token: "" },
    });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    const submit = result.current.submitSnapshot();
    expect(submit && "api_token" in (submit.connection ?? {})).toBe(false);
  });

  it("检查载荷与保存载荷同源：visible_if 隐藏字段在探测里不回落成驱动默认值", async () => {
    const VIS_FIELDS = [
      { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
      { key: "use_native", label: "Use native", field_type: "boolean", required: false, validation: {}, ui: { order: 2 } },
      {
        key: "native_pdu",
        label: "PDU",
        field_type: "integer",
        required: false,
        validation: { min: 1, max: 65535 },
        ui: { order: 3, visible_if: { field: "use_native", op: "eq", value: true } },
      },
    ];
    const srv = installFetch({ descriptors: { opcua: { fields: VIS_FIELDS } } });
    const { result } = renderSetup({
      active: true,
      driverSelectable: false,
      sessionKey: "edit-connection:e1",
      initialDriverId: "opcua",
      initialValues: { host: "10.0.0.2", use_native: false, native_pdu: 8192 },
      preserveHiddenValues: true,
      endpointId: "e1",
    });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));
    expect(result.current.raw.native_pdu).toBe(8192);

    await act(async () => {
      await result.current.probeConnection();
    });
    const check = srv.calls.find((c) => c.url.endsWith("/connection-check"));
    const conn = (check?.body as { connection: ConnectionValues }).connection;
    // 隐藏字段带着快照原值进入检查载荷：探测用的连接与「保存后会写出的连接」一致
    expect(conn.native_pdu).toBe(8192);
    expect(conn.host).toBe("10.0.0.2");
  });

  it("必填 Secret 被清空：本地 REQUIRED 阻断，不发任何检查请求（清空不能绕过必填）", async () => {
    const REQ_FIELDS = [
      { key: "endpoint_url", label: "URL", field_type: "url", required: true, validation: {}, ui: { order: 1 } },
      { key: "password", label: "Password", field_type: "secret", required: true, validation: {}, ui: { order: 2 } },
    ];
    const srv = installFetch({ descriptors: { opcua: { fields: REQ_FIELDS } } });
    const { result } = renderSetup({
      active: true,
      driverSelectable: false,
      sessionKey: "edit-connection:e1",
      initialDriverId: "opcua",
      initialValues: { endpoint_url: "opc.tcp://10.0.0.2:4840", password: { secret_set: true } },
      preserveHiddenValues: true,
      endpointId: "e1",
    });
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    // 用户把必填凭据清空（touched + 空）→ 归一化报 REQUIRED，请求不得发出
    act(() => result.current.setRaw({ endpoint_url: "opc.tcp://10.0.0.2:4840", password: "" }));
    const outcomes: ValidateOutcome[] = [];
    await act(async () => {
      outcomes.push(await result.current.validate());
    });
    expect(outcomes[0].ok).toBe(false);
    expect(outcomes[0].normalize.issues.some((i) => i.code === "REQUIRED")).toBe(true);
    expect(srv.calls.some((c) => c.url.endsWith("/connection-check"))).toBe(false);
  });

  it("面板不可见（弹窗已关闭）时端点通道同样零请求", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: OPCUA_FIELDS } } });
    const { result } = renderSetup({ ...editOptions("e1"), active: false });
    // 不可见时连描述符都不拉（既有 fail-closed 语义），blockReason 直接是 inactive
    await waitFor(() => expect(result.current.blockReason).toBe("inactive"));

    const probes: Array<ProbeOutcome | null> = [];
    const validations: ValidateOutcome[] = [];
    await act(async () => {
      probes.push(await result.current.probeConnection());
      validations.push(await result.current.validate());
    });
    // 与提交同一道前置门：inactive 时既不发检查请求，也不伪装成设备不可达
    expect(validations[0].ok).toBe(false);
    expect(probes[0]?.kind).toBe("infrastructure");
    expect(srv.calls.filter((c) => c.method === "POST")).toEqual([]);
  });

  it("归属 Endpoint 变化后，旧 Endpoint 的迟到响应不得覆盖新会话结果", async () => {
    let release!: () => void;
    const gate = new Promise<void>((r) => {
      release = r;
    });
    const srv = installFetch({
      descriptors: { opcua: { fields: OPCUA_FIELDS } },
      checkGate: gate,
    });
    // options 可变：模拟父组件切换 Endpoint（sessionKey 一并变化 → 会话复位）
    let options: UseConnectionSetupOptions = editOptions("e1");
    const { result, rerender } = renderHook(() => useConnectionSetup(options));
    await waitFor(() => expect(result.current.descriptorStatus).toBe("ready"));

    const pending: Array<ProbeOutcome | null> = [];
    act(() => {
      pending.push(result.current.probeConnection() as unknown as ProbeOutcome | null);
    });
    // 切到 e2：draft/结果复位、在途请求中止
    await act(async () => {
      options = { ...editOptions("e2"), sessionKey: "edit-connection:e2" };
      rerender();
    });
    expect(srv.calls.some((c) => c.url === "/api/v1/endpoints/e1/connection-check")).toBe(true);

    // 放行 e1 的迟到响应：它既不能写进结果区，也不能被当成 e2 的结论
    await act(async () => {
      release();
      await Promise.resolve();
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(result.current.probe).toBeNull();
  });
});
