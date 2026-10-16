/**
 * ConnectionSetupPanel 交互回归（C2）。
 *
 * 与 `connectionSetupModel.test.ts` 的分工：模型单测证明纯函数语义，
 * 本文件证明**真实交互与真实请求负载**：
 * - TSAP 未失焦提交最新输入（用 fireEvent 触发动作，避免 userEvent.click
 *   的焦点编排掩盖「依赖 onBlur」的缺陷）；
 * - 非法 TSAP 阻断且不沿用上一次的合法值；
 * - 驱动清单失败时所有动作失效且不出现硬编码 fallback 驱动；
 * - 400/501/503/504/网络失败/不可达 各自独立文案，结构化错误对象不崩溃；
 * - 高级必填缺失自动展开并定位；
 * - 参数变化后旧结果立即失效；
 * - Secret marker（非 password 命名）不得进入探测请求。
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ConnectionValues } from "./connectionSetupModel";
import { ConnectionSetupPanel } from "./ConnectionSetupPanel";
import { invokeDisabledButton } from "../test/bypassDisabled";

interface Call {
  url: string;
  method: string;
  body: unknown;
}

type ProbeResponder = () => { status: number; body: unknown };

const NCK_FIELDS = [
  { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
  { key: "port", label: "Port", field_type: "port", required: false, default: 102, validation: { min: 1, max: 65535 }, ui: { order: 2 } },
  { key: "local_tsap", label: "Local TSAP", field_type: "integer", required: true, default: 1024, validation: { min: 0, max: 65535 }, ui: { order: 10, advanced: true } },
  { key: "remote_tsap", label: "Remote TSAP", field_type: "integer", required: true, default: 3332, validation: { min: 0, max: 65535 }, ui: { order: 11, advanced: true } },
];

/** 高级必填且**无默认值**：用于验证「自动展开并定位」。 */
const NCK_REQUIRED_ADVANCED = NCK_FIELDS.map((f) =>
  f.key === "local_tsap" || f.key === "remote_tsap"
    ? { ...f, default: undefined }
    : f,
);

const NCK_DRIVER = [{ id: "sinumerik-nck", name: "SINUMERIK NCK", launchable: true }];

interface ServerOpts {
  driversFail?: boolean;
  drivers?: Array<{ id: string; name: string; launchable?: boolean; reason?: string | null }>;
  descriptors?: Record<string, unknown>;
  probe?: ProbeResponder;
  probeThrows?: boolean;
  validate?: ProbeResponder;
  /** C3-C：Endpoint-bound `connection-check` 的响应 */
  connectionCheck?: ProbeResponder;
}

function installFetch(opts: ServerOpts) {
  const calls: Call[] = [];
  const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
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
        return {
          ok: false,
          status: 500,
          json: async () => ({ error: { code: "BOOM", message: "驱动清单不可用" } }),
        };
      }
      return { ok: true, status: 200, json: async () => ({ drivers: opts.drivers ?? [] }) };
    }
    const desc = /^\/api\/v1\/drivers\/([^/]+)\/descriptor$/.exec(url);
    if (desc) {
      const fields = opts.descriptors?.[desc[1]];
      if (!fields) {
        return { ok: false, status: 404, json: async () => ({ error: { code: "NOT_FOUND", message: "no descriptor" } }) };
      }
      return { ok: true, status: 200, json: async () => ({ connection: fields }) };
    }
    const probe = /^\/api\/v1\/drivers\/([^/]+)\/probe$/.exec(url);
    if (probe) {
      if (opts.probeThrows) throw new TypeError("Failed to fetch");
      const r = opts.probe?.() ?? {
        status: 200,
        body: { reachable: true, device: { vendor: "Siemens" }, warnings: [] },
      };
      return { ok: r.status < 300, status: r.status, json: async () => r.body };
    }
    // C3-C：Endpoint-bound 检查（编辑态）。模式由请求体的 `mode` 区分，
    // 默认回「校验通过」；探测用例传入自己的响应。
    const check = /^\/api\/v1\/endpoints\/([^/]+)\/connection-check$/.exec(url);
    if (check) {
      const r = opts.connectionCheck?.() ?? { status: 200, body: { valid: true, issues: [] } };
      return { ok: r.status < 300, status: r.status, json: async () => r.body };
    }
    const validate = /^\/api\/v1\/drivers\/([^/]+)\/validate-connection$/.exec(url);
    if (validate) {
      const r = opts.validate?.() ?? { status: 200, body: {} };
      return { ok: r.status < 300, status: r.status, json: async () => r.body };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  (globalThis as { fetch?: unknown }).fetch = fetchMock;
  return { calls, opts };
}

function renderPanel(
  overrides: Omit<Partial<React.ComponentProps<typeof ConnectionSetupPanel>>, "onSubmit"> = {},
) {
  const onSubmit = vi.fn();
  const utils = render(
    <ConnectionSetupPanel
      mode="add"
      submitLabel="下一步"
      onSubmit={onSubmit}
      {...overrides}
    />,
  );
  return { ...utils, onSubmit };
}

/** 通过字段锚点取输入框（DescriptorFields 的 idPrefix/data-field-key 锚点）。 */
function fieldInput(container: HTMLElement, key: string): HTMLInputElement {
  const el = container.querySelector(`[data-field-key="${key}"] input`);
  if (!el) throw new Error(`字段 ${key} 未渲染`);
  return el as HTMLInputElement;
}

/**
 * 按 label 文本就近取输入框——**不依赖 data-field-key 锚点**。
 *
 * 为什么需要这个取法：TSAP 的对照实验要能在「字段组件回退为旧实现」
 * （旧版不渲染锚点）时照样定位到同一个输入框，否则测试会因取不到元素
 * 而失败，掩盖真正的断言差异。
 */
function inputByLabel(container: HTMLElement, label: string): HTMLInputElement {
  for (const span of Array.from(container.querySelectorAll("span"))) {
    const text = (span.textContent ?? "").replace(/\s*\*\s*$/, "").trim();
    if (text !== label) continue;
    const input = (span.parentElement ?? container).querySelector("input");
    if (input) return input as HTMLInputElement;
  }
  throw new Error(`未找到 label 为 ${label} 的输入框`);
}

function probeCalls(calls: Call[]): Call[] {
  return calls.filter((c) => c.url.endsWith("/probe"));
}

/** 常用前置：等描述符就绪 → 填 host → 返回容器与请求记录。 */
async function readyPanel(opts: ServerOpts) {
  const srv = installFetch({
    drivers: NCK_DRIVER,
    descriptors: { "sinumerik-nck": { fields: NCK_FIELDS } },
    ...opts,
  });
  const view = renderPanel();
  await waitFor(() => expect(inputByLabel(view.container, "Host")).toBeTruthy());
  fireEvent.change(inputByLabel(view.container, "Host"), { target: { value: "10.0.0.5" } });
  return { ...view, ...srv };
}

beforeEach(() => {
  vi.useRealTimers();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("C2 面板：TSAP 未失焦提交最新输入（A05）", () => {
  it("测试连接：焦点仍在输入框内就点按钮，请求负载是 0x0500 归一化后的 1280", async () => {
    const srv = await readyPanel({});
    const user = userEvent.setup();
    await user.click(screen.getByText(/高级（2）/));

    const tsap = inputByLabel(srv.container, "Local TSAP");
    tsap.focus();
    await user.clear(tsap);
    await user.type(tsap, "0x0500");

    // 关键前提：此刻焦点仍在输入框内，值只存在于 raw 草稿，尚未通过 blur 提交
    expect(document.activeElement).toBe(tsap);

    // 关键手法：用 fireEvent 触发点击（不做 userEvent 的焦点编排），
    // 因此本次动作**不可能**依赖 onBlur 提交旧值。
    fireEvent.click(screen.getByRole("button", { name: /测试连接/ }));

    await waitFor(() => expect(probeCalls(srv.calls).length).toBe(1));
    const body = probeCalls(srv.calls)[0].body as { connection: ConnectionValues };
    expect(body.connection.local_tsap).toBe(1280);
    // 不得回落到 Descriptor 默认值 1024
    expect(body.connection.local_tsap).not.toBe(1024);
  });

  it("保存（下一步）：同样在未失焦时提交最新 TSAP 值", async () => {
    const srv = await readyPanel({});
    const user = userEvent.setup();
    await user.click(screen.getByText(/高级（2）/));

    const tsap = inputByLabel(srv.container, "Local TSAP");
    tsap.focus();
    await user.clear(tsap);
    await user.type(tsap, "0x0500");
    expect(document.activeElement).toBe(tsap);

    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));

    await waitFor(() => expect(srv.onSubmit).toHaveBeenCalledTimes(1));
    const payload = srv.onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    expect(payload.connection.local_tsap).toBe(1280);
  });
});

describe("C2 面板：非法 TSAP 阻断（A05）", () => {
  it("先有已提交的合法值，再改成非法值：不发请求，也不沿用旧值", async () => {
    const srv = await readyPanel({});
    const user = userEvent.setup();
    await user.click(screen.getByText(/高级（2）/));
    const tsap = inputByLabel(srv.container, "Local TSAP");

    // 第一步：走 blur 提交一个合法值 0x0400（=1024）
    tsap.focus();
    fireEvent.change(tsap, { target: { value: "0x0400" } });
    fireEvent.blur(tsap);
    expect(srv.onSubmit).not.toHaveBeenCalled();

    // 第二步：改成非法值且**不失焦**
    tsap.focus();
    fireEvent.change(tsap, { target: { value: "0x" } });
    expect(document.activeElement).toBe(tsap);

    fireEvent.click(screen.getByRole("button", { name: /测试连接/ }));
    await screen.findByText(/未发起探测/);

    // 未发出探测请求，也没有任何请求体携带旧的 1024
    expect(probeCalls(srv.calls).length).toBe(0);
    expect(JSON.stringify(srv.calls)).not.toContain("1024");
    // 明确提示到具体字段与原因
    expect(screen.getByText(/connection\.local_tsap/)).toBeTruthy();
    expect(screen.getByText(/十六进制格式非法/)).toBeTruthy();
  });

  it("非法 TSAP 时保存路径同样被阻断", async () => {
    const srv = await readyPanel({});
    const user = userEvent.setup();
    await user.click(screen.getByText(/高级（2）/));
    const tsap = inputByLabel(srv.container, "Local TSAP");
    tsap.focus();
    fireEvent.change(tsap, { target: { value: "0xZZ" } });

    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await screen.findByText(/本地参数归一化未通过/);
    expect(srv.onSubmit).not.toHaveBeenCalled();
  });
});

describe("C2 面板：驱动清单失败（A02）", () => {
  it("显示错误 + 重试，所有动作禁用，且不出现硬编码 fallback 驱动", async () => {
    const srv = installFetch({ driversFail: true });
    renderPanel();

    await screen.findByText("驱动清单加载失败");
    expect(screen.getByText(/驱动清单不可用/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /重\s*试/ })).toBeTruthy();
    // 校验 / 测试 / 提交三条路径全部失效
    expect((screen.getByRole("button", { name: /校验参数/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /下一步/ }) as HTMLButtonElement).disabled).toBe(true);
    // 硬编码 fallback 驱动（已从代码中删除）不得出现
    expect(screen.queryByText("Siemens S7")).toBeNull();
    expect(screen.queryByText("FANUC FOCAS2")).toBeNull();
    expect(screen.queryByText("OPC UA")).toBeNull();

    // 重试成功后才恢复可用
    srv.opts.driversFail = false;
    srv.opts.drivers = NCK_DRIVER;
    srv.opts.descriptors = { "sinumerik-nck": { fields: NCK_FIELDS } };
    fireEvent.click(screen.getByRole("button", { name: /重\s*试/ }));
    await waitFor(() =>
      expect((screen.getByRole("button", { name: /下一步/ }) as HTMLButtonElement).disabled).toBe(false),
    );
  });
});

// ---------------------------------------------------------------------------
// P0：提交函数自身的前置守卫
//
// 为什么需要「绕过 disabled」的探针：React + AntD 双重拦截禁用按钮的 click，
// 普通交互测试无法触发被禁用的提交函数，守卫是否存在就成了不可验证的假绿。
// 探针直接调用按钮组件级 onClick，精确模拟「disabled 被回归移除」的场景。
// ---------------------------------------------------------------------------

describe("C2 面板：P0 提交守卫（禁用被绕过也不发写请求）", () => {
  it("驱动清单失败：绕过 disabled 触发「下一步」，onSubmit 仍不被调用", async () => {
    installFetch({ driversFail: true });
    const view = renderPanel();
    await screen.findByText("驱动清单加载失败");

    const submit = screen.getByRole("button", { name: /下一步/ }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    await act(async () => {
      // 探针必须真的调用到处理函数，否则本用例是假绿
      expect(invokeDisabledButton(submit)).toBe(true);
    });
    expect(view.onSubmit).not.toHaveBeenCalled();
  });

  it("描述符失败：绕过 disabled 触发「测试连接」与「下一步」，零请求", async () => {
    const srv = installFetch({ drivers: NCK_DRIVER, descriptors: {} });
    const view = renderPanel();
    await screen.findByText("驱动描述符加载失败");
    expect(screen.getByRole("button", { name: /重\s*试/ })).toBeTruthy();

    const probeBtn = screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement;
    const validateBtn = screen.getByRole("button", { name: /校验参数/ }) as HTMLButtonElement;
    const submitBtn = screen.getByRole("button", { name: /下一步/ }) as HTMLButtonElement;
    expect([probeBtn.disabled, validateBtn.disabled, submitBtn.disabled]).toEqual([true, true, true]);

    await act(async () => {
      expect(invokeDisabledButton(probeBtn)).toBe(true);
      expect(invokeDisabledButton(validateBtn)).toBe(true);
      expect(invokeDisabledButton(submitBtn)).toBe(true);
    });

    expect(view.onSubmit).not.toHaveBeenCalled();
    expect(probeCalls(srv.calls).length).toBe(0);
    expect(srv.calls.some((c) => c.url.endsWith("/validate-connection"))).toBe(false);
    // 任何写请求都不得发出（面板自身不发写请求，这里做全量兜底断言）
    expect(srv.calls.some((c) => c.method !== "GET")).toBe(false);
  });
});

describe("C2 面板：探测结果分层（A08/A09/A10/A11/A26）", () => {
  async function probeAndReadTitle(opts: ServerOpts): Promise<string> {
    const srv = await readyPanel(opts);
    fireEvent.click(screen.getByRole("button", { name: /测试连接/ }));
    const title = await screen.findByText(/协议探测：/);
    await waitFor(() => expect(probeCalls(srv.calls).length).toBe(1));
    return title.textContent ?? "";
  }

  it("501：是「驱动不支持此探测方式」，不得说成设备不可达", async () => {
    const title = await probeAndReadTitle({
      probe: () => ({ status: 501, body: { error: { code: "UNSUPPORTED", message: "probe not implemented" } } }),
    });
    expect(title).toContain("不支持此探测方式");
    expect(title).toContain("probe not implemented");
    expect(title).not.toContain("不可达");
    expect(title).not.toContain("连接成功");
  });

  it("503：是 Mesa/驱动侧基础设施失败，不得说成 CNC 不可达", async () => {
    const title = await probeAndReadTitle({
      probe: () => ({ status: 503, body: { error: { code: "DRIVER_UNAVAILABLE", message: "driver process down" } } }),
    });
    expect(title).toContain("驱动或探测服务不可用");
    expect(title).not.toContain("不可达");
  });

  it("504：是探测超时", async () => {
    const title = await probeAndReadTitle({ probe: () => ({ status: 504, body: {} }) });
    expect(title).toContain("探测超时");
    expect(title).not.toContain("不可达");
  });

  it("400：是连接参数无效，并逐条列出字段问题", async () => {
    const title = await probeAndReadTitle({
      probe: () => ({
        status: 400,
        body: {
          error: { code: "BAD_CONFIG", message: "field `module` is required" },
          issues: [{ path: "connection.module", code: "REQUIRED", message: "缺少 module" }],
        },
      }),
    });
    expect(title).toContain("连接参数无效");
    expect(title).toContain("BAD_CONFIG");
    expect(title).toContain("field `module` is required");
    expect(title).not.toContain("不可达");
    // 结构化错误对象渲染成文本（含字段定位 path），不得出现 [object Object]
    expect(document.body.textContent ?? "").not.toContain("[object Object]");
    expect(screen.getByText(/connection\.module: REQUIRED: 缺少 module/)).toBeTruthy();
  });

  it("网络层失败：是 Mesa API 不可达，不是设备不可达", async () => {
    const title = await probeAndReadTitle({ probeThrows: true });
    expect(title).toContain("无法访问 Mesa API");
    expect(title).not.toContain("目标设备不可达");
  });
  it("200 + reachable=false：才是设备不可达", async () => {
    const title = await probeAndReadTitle({
      probe: () => ({ status: 200, body: { reachable: false, error: "no route to host" } }),
    });
    expect(title).toContain("目标设备不可达");
    expect(title).not.toContain("连接成功");
  });

  it("200 + reachable=true 但身份缺失：只说协议可达、身份未确认", async () => {
    const title = await probeAndReadTitle({
      probe: () => ({
        status: 200,
        body: {
          reachable: true,
          device: { vendor: null, family: null, model: null, firmware: null },
          warnings: [{ code: "NCK_ANCHOR_PENDING", message: "身份未确认" }],
        },
      }),
    });
    expect(title).toContain("协议通信可达");
    expect(title).toContain("身份未确认");
    expect(title).not.toContain("已识别");
    expect(screen.getByText(/NCK_ANCHOR_PENDING: 身份未确认/)).toBeTruthy();
  });

  it("200 + 有身份：展示身份信息", async () => {
    const title = await probeAndReadTitle({
      probe: () => ({ status: 200, body: { reachable: true, device: { vendor: "Siemens", family: "828d" }, warnings: [] } }),
    });
    expect(title).toContain("设备返回身份信息");
    expect(screen.getByText(/Siemens/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// P2：字段级 issue 的 path 不得重复渲染
// （后端 message 有时自带同一个 path 前缀，而渲染层又会单独展示 path）
// ---------------------------------------------------------------------------

describe("C2 面板：issue path 前缀去重（P2）", () => {
  it("服务端 message 已含 connection. 前缀时不出现重复前缀", async () => {
    await readyPanel({
      validate: () => ({
        status: 400,
        body: {
          error: { code: "BAD_CONFIG", message: "连接参数非法" },
          issues: [
            {
              path: "connection.host",
              code: "REQUIRED",
              message: "connection.host 缺少必填",
            },
          ],
        },
      }),
    });

    fireEvent.click(screen.getByRole("button", { name: /校验参数/ }));
    await screen.findByText(/参数校验未通过/);

    expect(screen.getByText("connection.host：缺少必填（REQUIRED）")).toBeTruthy();
    expect(document.body.textContent ?? "").not.toContain("connection.host：connection.host");
  });
});

describe("C2 面板：高级必填自动展开与结果失效", () => {
  it("A04：高级必填缺失时自动展开并定位到缺失字段", async () => {
    const srv = installFetch({
      drivers: NCK_DRIVER,
      descriptors: { "sinumerik-nck": { fields: NCK_REQUIRED_ADVANCED } },
    });
    const view = renderPanel();
    await waitFor(() => expect(inputByLabel(view.container, "Host")).toBeTruthy());
    // 初始收起：高级字段不渲染
    expect(view.container.querySelector('[data-field-key="local_tsap"]')).toBeNull();

    fireEvent.change(inputByLabel(view.container, "Host"), { target: { value: "10.0.0.5" } });
    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));

    await screen.findByText(/高级参数缺少必填项/);
    expect(view.container.querySelector('[data-field-key="local_tsap"]')).not.toBeNull();
    // 高亮定位（outline 标记）
    const anchor = view.container.querySelector('[data-field-key="local_tsap"]') as HTMLElement;
    expect(anchor.style.outline).toContain("2px");
    expect(view.onSubmit).not.toHaveBeenCalled();
    expect(srv.calls.some((c) => c.url.endsWith("/probe"))).toBe(false);
  });

  it("A03：参数变化后旧的绿色探测结果立即失效", async () => {
    const srv = await readyPanel({
      probe: () => ({ status: 200, body: { reachable: true, device: { vendor: "Siemens" }, warnings: [] } }),
    });
    fireEvent.click(screen.getByRole("button", { name: /测试连接/ }));
    await screen.findByText(/协议探测：/);
    expect(srv.calls.filter((c) => c.url.endsWith("/probe")).length).toBe(1);

    // 改动任一连接参数 → 结果必须立刻消失，不得继续显示旧的成功结论
    fireEvent.change(fieldInput(srv.container, "host"), { target: { value: "10.0.0.9" } });
    await waitFor(() => expect(screen.queryByText(/协议探测：/)).toBeNull());
  });
});

describe("C2 面板：Secret-safe 边界（编辑态）", () => {
  const SECRET_FIELDS = [
    { key: "endpoint_url", label: "Endpoint URL", field_type: "url", required: true, validation: {}, ui: { order: 1 } },
    { key: "api_token", label: "API Token", field_type: "secret", required: false, validation: {}, ui: { order: 2 } },
  ];

  it("非 password 命名的凭据仍是 marker：禁止测试、不发出含 marker 的请求，保存仍保留 marker", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: SECRET_FIELDS } } });
    const onSubmit = vi.fn();
    render(
      <ConnectionSetupPanel
        mode="edit"
        driverSelectable={false}
        initialDriverId="opcua"
        initialValues={{ endpoint_url: "opc.tcp://10.0.0.2:4840", api_token: { secret_set: true } }}
        onSubmit={onSubmit}
      />,
    );

    await screen.findByText(/已保存的凭据不会回显/);
    expect((screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /校验参数/ }) as HTMLButtonElement).disabled).toBe(true);
    // 从未发出把 marker 当密码的探测/校验请求
    expect(probeCalls(srv.calls).length).toBe(0);
    expect(srv.calls.some((c) => c.url.endsWith("/validate-connection"))).toBe(false);
    expect(JSON.stringify(srv.calls)).not.toContain("secret_set");

    // 保存路径不受限：marker 原样回传（后端据此保留历史凭据）
    fireEvent.click(screen.getByRole("button", { name: /保\s*存/ }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const payload = onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    expect(payload.connection.api_token).toEqual({ secret_set: true });
  });

  it("重新输入明文凭据后测试解禁（并发出明文，而非 marker）", async () => {
    const srv = installFetch({ descriptors: { opcua: { fields: SECRET_FIELDS } } });
    render(
      <ConnectionSetupPanel
        mode="edit"
        driverSelectable={false}
        initialDriverId="opcua"
        initialValues={{ endpoint_url: "opc.tcp://10.0.0.2:4840", api_token: { secret_set: true } }}
        onSubmit={vi.fn()}
      />,
    );
    await screen.findByText(/已保存的凭据不会回显/);
    const token = document.querySelector('input[type="password"]') as HTMLInputElement;
    fireEvent.change(token, { target: { value: "new-token" } });

    await waitFor(() =>
      expect((screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(screen.getByRole("button", { name: /测试连接/ }));
    await waitFor(() => expect(probeCalls(srv.calls).length).toBe(1));
    const body = probeCalls(srv.calls)[0].body as { connection: ConnectionValues };
    expect(body.connection.api_token).toBe("new-token");
  });
});

// ---------------------------------------------------------------------------
// C3-C：编辑态接通 Endpoint-bound 检查
//
// 纪律：只有「编辑既有连接」（mode=edit + endpointId）才解除「含 marker 即禁测」；
// 新建/新增必须保持 Driver-bound 边界（marker 绝不进 /probe）。
// ---------------------------------------------------------------------------

describe("C3-C 面板：编辑态 Endpoint-bound 检查（已保存凭据可用）", () => {
  const SECRET_FIELDS = [
    { key: "endpoint_url", label: "Endpoint URL", field_type: "url", required: true, validation: {}, ui: { order: 1 } },
    { key: "api_token", label: "API Token", field_type: "secret", required: false, validation: {}, ui: { order: 2 } },
  ];
  const SAVED: ConnectionValues = {
    endpoint_url: "opc.tcp://10.0.0.2:4840",
    api_token: { secret_set: true },
  };

  function checkCalls(calls: Call[]): Call[] {
    return calls.filter((c) => c.url.endsWith("/connection-check"));
  }

  function renderEditEndpoint(
    opts: ServerOpts = {},
    overrides: Partial<React.ComponentProps<typeof ConnectionSetupPanel>> = {},
  ) {
    const srv = installFetch({
      drivers: [{ id: "opcua", name: "OPC UA", launchable: true }],
      descriptors: { opcua: { fields: SECRET_FIELDS } },
      ...opts,
    });
    const onSubmit = vi.fn();
    const utils = render(
      <ConnectionSetupPanel
        mode="edit"
        driverSelectable={false}
        initialDriverId="opcua"
        endpointId="ep-1"
        initialValues={SAVED}
        onSubmit={onSubmit}
        {...overrides}
      />,
    );
    return { ...utils, srv, onSubmit };
  }

  it("marker 解禁：校验/测试可用，请求发往 /endpoints/ep-1/connection-check 且 marker 原样", async () => {
    const { srv } = renderEditEndpoint({
      connectionCheck: () => ({
        status: 200,
        body: { reachable: true, device: { vendor: "Mesa" }, warnings: [] },
      }),
    });

    // 文案从「暂不支持」换成「使用已保存凭据」，且不再禁用
    await screen.findByText(/将使用已保存的凭据/);
    expect(screen.queryByText(/本场景暂不支持校验\/测试/)).toBeNull();
    const probeBtn = screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement;
    const validateBtn = screen.getByRole("button", { name: /校验参数/ }) as HTMLButtonElement;
    expect(probeBtn.disabled).toBe(false);
    expect(validateBtn.disabled).toBe(false);

    fireEvent.click(probeBtn);
    await waitFor(() => expect(checkCalls(srv.calls).length).toBe(1));
    const body = checkCalls(srv.calls)[0].body as { mode: string; connection: ConnectionValues };
    expect(body.mode).toBe("probe");
    // marker 由服务端按该 Endpoint 物化：前端既不发送明文，也不把 marker 抹掉
    expect(body.connection.api_token).toEqual({ secret_set: true });

    fireEvent.click(validateBtn);
    await waitFor(() => expect(checkCalls(srv.calls).length).toBe(2));
    expect((checkCalls(srv.calls)[1].body as { mode: string }).mode).toBe("validate");

    // Driver-bound 通道零调用（marker 绝不进 /probe、/validate-connection）
    expect(probeCalls(srv.calls).length).toBe(0);
    expect(srv.calls.some((c) => c.url.endsWith("/validate-connection"))).toBe(false);
  });

  it("两出口同口径：清空凭据 → 检查请求与保存负载都带 {clear_secret:true}", async () => {
    const { srv, onSubmit } = renderEditEndpoint();
    await screen.findByText(/将使用已保存的凭据/);

    // 用户在凭据输入框里输入后又删空（touched + 空值）——若只丢 key，服务端会拿旧密码。
    // 注意：marker 态输入框显示空值，直接 change 成 "" 会被 React 视为「值没变」而不触发
    // onChange，因此按真实用户动作「先输入、再删空」构造。
    const token = document.querySelector('input[type="password"]') as HTMLInputElement;
    fireEvent.change(token, { target: { value: "temp" } });
    fireEvent.change(token, { target: { value: "" } });

    fireEvent.click(screen.getByRole("button", { name: /测试连接/ }));
    await waitFor(() => expect(checkCalls(srv.calls).length).toBe(1));
    const checkBody = checkCalls(srv.calls)[0].body as { connection: ConnectionValues };
    expect(checkBody.connection.api_token).toEqual({ clear_secret: true });

    fireEvent.click(screen.getByRole("button", { name: /保\s*存/ }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const payload = onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    expect(payload.connection.api_token).toEqual({ clear_secret: true });
    // 其余字段不受影响（清除只针对被显式清空的 Secret）
    expect(payload.connection.endpoint_url).toBe("opc.tcp://10.0.0.2:4840");
  });

  it("面板层守卫：非 edit 模式即使被传了 endpointId 也仍走 Driver-bound（marker 禁测）", async () => {
    // 用 create-draft + 快照权威（driverSelectable=false）构造：草稿里保留 marker，
    // 只有面板层「mode === \"edit\" 才认 endpointId」这一道守卫能决定走哪条通道。
    const srv = installFetch({
      descriptors: { opcua: { fields: SECRET_FIELDS } },
    });
    render(
      <ConnectionSetupPanel
        mode="create-draft"
        driverSelectable={false}
        initialDriverId="opcua"
        endpointId="ep-1"
        initialValues={SAVED}
        onSubmit={vi.fn()}
      />,
    );

    // 走回 Driver-bound 的阻断文案与禁用态（marker 不是密码）
    await screen.findByText(/已保存的凭据不会回显/);
    expect((screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(checkCalls(srv.calls).length).toBe(0);
    expect(probeCalls(srv.calls).length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// P1：编辑态 visible_if 隐藏字段不得丢失
//
// 后端 update_endpoint() 用请求中的 connection **整体替换**旧 JSON（只有 Secret
// 有「缺失即保留」语义），因此编辑态省略隐藏字段 = 该字段真的丢失。
// ---------------------------------------------------------------------------

describe("C2 面板：编辑态 visible_if 隐藏字段保留（P1）", () => {
  const VIS_FIELDS = [
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
    { key: "api_token", label: "API Token", field_type: "secret", required: false, validation: {}, ui: { order: 4 } },
  ];

  function renderEdit(initialValues: ConnectionValues) {
    installFetch({ descriptors: { opcua: { fields: VIS_FIELDS } } });
    const onSubmit = vi.fn();
    render(
      <ConnectionSetupPanel
        mode="edit"
        driverSelectable={false}
        initialDriverId="opcua"
        initialValues={initialValues}
        onSubmit={onSubmit}
      />,
    );
    return { onSubmit };
  }

  it("正向：visible_if 不满足的字段以快照原值进入保存负载", async () => {
    const { onSubmit } = renderEdit({
      host: "10.0.0.2",
      use_native: false,
      native_pdu: 8192,
      api_token: { secret_set: true },
    });
    await screen.findByDisplayValue("10.0.0.2");
    // 隐藏字段不渲染（用户看不到它，但它的值必须原样保留）
    expect(document.querySelector('[data-field-key="native_pdu"]')).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /保\s*存/ }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const payload = onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    expect(payload.connection.native_pdu).toBe(8192);
    expect(payload.connection.host).toBe("10.0.0.2");
    // Secret 的 marker 保留语义不受本改动影响
    expect(payload.connection.api_token).toEqual({ secret_set: true });
  });

  it("负例：旧 connection 中不属于当前 Descriptor 的字段不得出现在保存负载里", async () => {
    const { onSubmit } = renderEdit({
      host: "10.0.0.2",
      use_native: false,
      native_pdu: 8192,
      legacy_field: "old",
      gone_object: { a: 1 },
    });
    await screen.findByDisplayValue("10.0.0.2");

    fireEvent.click(screen.getByRole("button", { name: /保\s*存/ }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const payload = onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    // 证明不是无脑整体合并旧 connection
    expect("legacy_field" in payload.connection).toBe(false);
    expect("gone_object" in payload.connection).toBe(false);
    expect(Object.keys(payload.connection).sort()).toEqual(["host", "native_pdu", "use_native"]);
  });
});

// ---------------------------------------------------------------------------
// P1-2：「用户明确清除」与「自动隐藏」的语义区分（已定案的产品语义）
//
// | 情形 | 保存行为 |
// | 条件变化导致隐藏、用户没清除 | 保留原值 |
// | 可见时明确清空、随后隐藏 | 删除原值 |
// | 初始化为空但未修改 | 不得误删 |
// | 必填被清除 | 校验失败，不得静默删除 |
//
// 关键：**程序初始化得到的空字符串 ≠ 用户明确清除**。草稿里的空值两种来源在
// 一起比对时长得一样，因此这里全部用真实交互（改条件、清空输入框）验证负载。
// ---------------------------------------------------------------------------

describe("C2 面板：用户清除 vs 自动隐藏（P1-2）", () => {
  const OPTIONAL_FIELDS = [
    { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
    {
      key: "use_pdu",
      label: "启用 PDU 协商",
      field_type: "boolean",
      required: false,
      default: true,
      validation: {},
      ui: { order: 2 },
    },
    {
      key: "pdu_size",
      label: "PDU 长度",
      field_type: "integer",
      required: false,
      validation: { min: 240, max: 960 },
      ui: { order: 3, visible_if: { field: "use_pdu", op: "eq", value: true } },
    },
    {
      key: "note",
      label: "备注",
      field_type: "string",
      required: false,
      validation: {},
      ui: { order: 4, visible_if: { field: "use_pdu", op: "eq", value: true } },
    },
  ];

  /** 必填的 `note`：用于验证「必填被清除不得静默删除」。 */
  const REQUIRED_NOTE_FIELDS = OPTIONAL_FIELDS.map((f) =>
    f.key === "note" ? { ...f, required: true } : f,
  );

  const SNAPSHOT: ConnectionValues = {
    host: "10.0.0.2",
    use_pdu: true,
    pdu_size: 512,
    note: "keep-me",
  };

  function renderEdit(fields: unknown[], initialValues: ConnectionValues) {
    installFetch({ descriptors: { s7: { fields } } });
    const onSubmit = vi.fn();
    const view = render(
      <ConnectionSetupPanel
        mode="edit"
        driverSelectable={false}
        initialDriverId="s7"
        initialValues={initialValues}
        onSubmit={onSubmit}
      />,
    );
    return { ...view, onSubmit };
  }

  const anchor = (c: HTMLElement, key: string) => c.querySelector(`[data-field-key="${key}"]`);
  const checkboxOf = (c: HTMLElement, key: string) =>
    c.querySelector(`[data-field-key="${key}"] input[type="checkbox"]`) as HTMLInputElement;
  const inputOf = (c: HTMLElement, key: string) =>
    c.querySelector(`[data-field-key="${key}"] input`) as HTMLInputElement;
  const save = () => screen.getByRole("button", { name: /保\s*存/ });

  it("① 字段仅因条件变化而隐藏、用户未清除 → 保存负载保留原值", async () => {
    const { container, onSubmit } = renderEdit(OPTIONAL_FIELDS, { ...SNAPSHOT });
    await screen.findByDisplayValue("10.0.0.2");
    // 前提：改动条件前字段确实可见（否则本用例证明不了「自动隐藏」）
    expect(anchor(container, "pdu_size")).not.toBeNull();

    fireEvent.click(checkboxOf(container, "use_pdu")); // 仅改条件，未清除任何字段
    expect(anchor(container, "pdu_size")).toBeNull(); // 字段被条件隐藏

    fireEvent.click(save());
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const payload = onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    expect(payload.connection.pdu_size).toBe(512);
    expect(payload.connection.use_pdu).toBe(false);
    expect(payload.connection.note).toBe("keep-me");
  });

  it("② 用户在字段可见时明确清空、随后字段隐藏 → 保存负载删除该字段", async () => {
    const { container, onSubmit } = renderEdit(OPTIONAL_FIELDS, { ...SNAPSHOT });
    await screen.findByDisplayValue("keep-me");

    const note = inputOf(container, "note");
    fireEvent.change(note, { target: { value: "" } }); // 可见时明确清空
    expect(note.value).toBe("");
    fireEvent.click(checkboxOf(container, "use_pdu")); // 之后字段才隐藏
    expect(anchor(container, "note")).toBeNull();

    fireEvent.click(save());
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const payload = onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    // 明确清除优先：字段从负载中消失（后端整体替换 ⇒ 服务端删除该值）
    expect("note" in payload.connection).toBe(false);
    // 未被清除的另一个隐藏字段不受牵连
    expect(payload.connection.pdu_size).toBe(512);
  });

  it("④a 必填字段（可见）被清空 → 校验失败，不提交、不静默删除", async () => {
    const { container, onSubmit } = renderEdit(OPTIONAL_FIELDS, { ...SNAPSHOT });
    await screen.findByDisplayValue("10.0.0.2");

    fireEvent.change(inputOf(container, "host"), { target: { value: "" } });
    fireEvent.click(save());

    await screen.findByText("参数校验未通过");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByText(/connection\.host/)).toBeTruthy();
  });

  it("④b 必填字段在可见时被清空、随后条件隐藏 → 校验失败（REQUIRED_CLEARED），不静默删除", async () => {
    const { container, onSubmit } = renderEdit(REQUIRED_NOTE_FIELDS, { ...SNAPSHOT });
    await screen.findByDisplayValue("keep-me");

    fireEvent.change(inputOf(container, "note"), { target: { value: "" } }); // 可见时清空
    fireEvent.click(checkboxOf(container, "use_pdu")); // 再隐藏
    expect(anchor(container, "note")).toBeNull();

    fireEvent.click(save());
    await screen.findByText("参数校验未通过");
    // 既没有发出保存负载，也没有把「用户清空必填」静默变成删除
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByText(/connection\.note/)).toBeTruthy();
    expect(screen.getByText(/REQUIRED_CLEARED/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// 实测示例预设接入（本轮）：`connectionProfiles.selectablePresets` → 面板
//
// 产品约定（不得偏离）：
// - 单台 828D 的实测 TSAP（0x0400/0x0D04）只是**可选示例**，不得自动预填；
// - 只有用户主动点击「应用实测示例」才写入，且必须走既有草稿机制；
// - 来源（provenance）与限制（caveat）必须对用户可见；
// - TSAP 归入高级连接参数，但必填与校验规则不变。
// ---------------------------------------------------------------------------

/**
 * 与真实 NCK Descriptor 对齐的字段集：TSAP **必填、无默认值、advanced=true**。
 *
 * 刻意不复用上面的 `NCK_FIELDS`——那份夹具带 `default: 1024/3332`，
 * 而「没有静默默认」正是本组用例要证明的结论，夹具里出现默认值会让结论失去意义。
 */
const NCK_PRESET_FIELDS = [
  { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
  { key: "port", label: "Port", field_type: "port", required: false, default: 102, validation: { min: 1, max: 65535 }, ui: { order: 2 } },
  { key: "family", label: "Family", field_type: "enum", required: true, validation: { enum_options: ["828d", "840d-sl"] }, ui: { order: 3 } },
  { key: "local_tsap", label: "Local TSAP", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: { order: 10, advanced: true, placeholder: "如 0x0100 或 256" } },
  { key: "remote_tsap", label: "Remote TSAP", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: { order: 11, advanced: true, placeholder: "如 0x0102 或 258" } },
];

/**
 * 在 AntD Select 上真实选择一个选项（点开触发器 → 点选项）。
 *
 * AntD v6 的触发器类名是 `.ant-select-content`（v5 为 `.ant-select-selector`），
 * 选项仍是 `.ant-select-item-option`（带 `title`）。
 */
async function selectEnumOption(container: HTMLElement, fieldKey: string, optionText: string) {
  const trigger = container.querySelector(`[data-field-key="${fieldKey}"] .ant-select-content`);
  if (!trigger) throw new Error(`字段 ${fieldKey} 的 Select 未渲染`);
  const user = userEvent.setup();
  await user.click(trigger as HTMLElement);
  const option = await screen.findByTitle(optionText);
  await user.click(option);
}

/** 预设用例前置：NCK 描述符就绪 → 填 host（不碰 TSAP / family）。 */
async function readyPresetPanel(opts: ServerOpts = {}) {
  const srv = installFetch({
    drivers: NCK_DRIVER,
    descriptors: { "sinumerik-nck": { fields: NCK_PRESET_FIELDS } },
    ...opts,
  });
  const view = renderPanel();
  await waitFor(() => expect(inputByLabel(view.container, "Host")).toBeTruthy());
  fireEvent.change(inputByLabel(view.container, "Host"), { target: { value: "10.0.0.5" } });
  return { ...view, ...srv };
}

const applyPresetButton = () => screen.getByRole("button", { name: /应用实测示例/ });

describe("C2 面板：实测示例预设（不静默预填 / 显式应用 / 来源可见）", () => {
  it("选择 NCK + 828d 后仍不预填：TSAP 输入框为空，草稿里没有 TSAP 值", async () => {
    const srv = await readyPresetPanel();
    await selectEnumOption(srv.container, "family", "828d");

    // 预设被识别（卡片出现），但值只出现在卡片文案里，不进草稿
    await screen.findByText("828D 实测连接示例");

    fireEvent.click(screen.getByText(/高级（2）/));
    expect(inputByLabel(srv.container, "Local TSAP").value).toBe("");
    expect(inputByLabel(srv.container, "Remote TSAP").value).toBe("");

    // 草稿里也没有 TSAP：必填缺失拦住提交（若有静默默认，这里就会提交成功）
    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await screen.findByText(/高级参数缺少必填项/);
    expect(srv.onSubmit).not.toHaveBeenCalled();
    expect(JSON.stringify(srv.calls)).not.toContain("1024");
    expect(JSON.stringify(srv.calls)).not.toContain("3332");
  });

  it("显式点击「应用实测示例」后：草稿与提交载荷是 local_tsap=1024 / remote_tsap=3332", async () => {
    const srv = await readyPresetPanel();
    await selectEnumOption(srv.container, "family", "828d");
    await screen.findByText("828D 实测连接示例");

    fireEvent.click(applyPresetButton());

    // 应用即展开高级区并按 number 回显为十六进制
    await waitFor(() => expect(inputByLabel(srv.container, "Local TSAP").value).toBe("0x0400"));
    expect(inputByLabel(srv.container, "Remote TSAP").value).toBe("0x0D04");

    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await waitFor(() => expect(srv.onSubmit).toHaveBeenCalledTimes(1));
    const payload = srv.onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    // 断言真实载荷值，而不是只看界面文案
    expect(payload.connection.local_tsap).toBe(1024);
    expect(payload.connection.remote_tsap).toBe(3332);
  });

  it("应用后仍可自由修改：以用户最终值为准，未改动的字段保持示例值", async () => {
    const srv = await readyPresetPanel();
    await selectEnumOption(srv.container, "family", "828d");
    await screen.findByText("828D 实测连接示例");
    fireEvent.click(applyPresetButton());
    await waitFor(() => expect(inputByLabel(srv.container, "Local TSAP").value).toBe("0x0400"));

    const tsap = inputByLabel(srv.container, "Local TSAP");
    fireEvent.change(tsap, { target: { value: "0x0500" } });
    fireEvent.blur(tsap);

    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await waitFor(() => expect(srv.onSubmit).toHaveBeenCalledTimes(1));
    const payload = srv.onSubmit.mock.calls[0][0] as { connection: ConnectionValues };
    expect(payload.connection.local_tsap).toBe(1280);
    expect(payload.connection.remote_tsap).toBe(3332);
  });

  it("TSAP 归入高级参数：默认折叠不可见，展开后才渲染（必填与范围不变）", async () => {
    const srv = await readyPresetPanel();
    await selectEnumOption(srv.container, "family", "828d");

    // 默认折叠：TSAP 不在 DOM 里
    expect(srv.container.querySelector('[data-field-key="local_tsap"]')).toBeNull();
    expect(srv.container.querySelector('[data-field-key="remote_tsap"]')).toBeNull();

    fireEvent.click(screen.getByText(/高级（2）/));
    expect(srv.container.querySelector('[data-field-key="local_tsap"]')).not.toBeNull();
    expect(srv.container.querySelector('[data-field-key="remote_tsap"]')).not.toBeNull();
    // 必填标记与格式提示仍在（归入高级区不等于放宽要求）
    expect(inputByLabel(srv.container, "Local TSAP").placeholder).toBe("如 0x0100 或 256");
  });

  it("来源与限制可见：provenance / caveat 原文渲染 + 单台标记 + 十六进制与十进制并列", async () => {
    const srv = await readyPresetPanel();
    await selectEnumOption(srv.container, "family", "828d");
    await screen.findByText("828D 实测连接示例");

    // 证据等级标记
    expect(screen.getByText("单台设备验证")).toBeTruthy();
    // 来源：来自 connectionProfiles 的原文（不另写文案）
    expect(screen.getByText(/nck-local-migration-2026-10-13\.md/)).toBeTruthy();
    // 限制：单台设备的实测示例，不是所有 828D 的通用默认
    expect(screen.getByText(/单台设备的实测示例/)).toBeTruthy();
    expect(screen.getByText(/不是所有 828D 的通用默认/)).toBeTruthy();
    // 十进制与十六进制同时给出
    expect(srv.container.querySelector('[data-preset-value="local_tsap"]')?.textContent).toBe(
      "Local TSAP 0x0400 (1024)",
    );
    expect(srv.container.querySelector('[data-preset-value="remote_tsap"]')?.textContent).toBe(
      "Remote TSAP 0x0D04 (3332)",
    );
  });

  it("换到无预设的系列（840d-sl）后不再展示示例，且不自动改动草稿", async () => {
    const srv = await readyPresetPanel();
    await selectEnumOption(srv.container, "family", "828d");
    await screen.findByText("828D 实测连接示例");

    await selectEnumOption(srv.container, "family", "840d-sl");
    await waitFor(() => expect(screen.queryByText("828D 实测连接示例")).toBeNull());
    expect(screen.queryByRole("button", { name: /应用实测示例/ })).toBeNull();
    // 从未自动写入过任何 TSAP（应用按钮一次都没点过）
    expect(JSON.stringify(srv.calls)).not.toContain("1024");
  });
});

// ---------------------------------------------------------------------------
// 跨系列预设遗留（评审要求）：切换系列只清理**仍由预设控制**的 TSAP 值
//
// 缺陷路径：选 828d → 应用实测示例（0x0400/0x0D04）→ 把系列改成 840d-sl；
// 828D 卡片消失，但两个 TSAP 仍留在草稿里，用户可能把 828D 的实测值用在
// 840D sl 上——与「单台实测值不得被当作其它设备的默认值」的约定冲突。
//
// 修复规则（不得偏离）：
// - 当前值仍等于预设写入的值（用户没改过）→ 清除，必填校验重新接管；
// - 当前值已被用户手工改过 → **不得删除**，改为要求用户确认适用性；
// - 切回 828d **不**自动重新填充，写值只有「应用实测示例」一条用户路径。
// ---------------------------------------------------------------------------

/** 应用 828D 实测示例，并等到两个 TSAP 真实回填（应用后高级区自动展开）。 */
async function apply828dExample(srv: { container: HTMLElement }) {
  await selectEnumOption(srv.container, "family", "828d");
  await screen.findByText("828D 实测连接示例");
  fireEvent.click(applyPresetButton());
  await waitFor(() => expect(inputByLabel(srv.container, "Local TSAP").value).toBe("0x0400"));
  expect(inputByLabel(srv.container, "Remote TSAP").value).toBe("0x0D04");
}

describe("C2 面板：跨系列预设遗留（只清理未被用户改过的预设值）", () => {
  it("① 828d 应用示例后切到 840d-sl：未改动的预设 TSAP 被自动清除，必填校验重新生效", async () => {
    const srv = await readyPresetPanel();
    await apply828dExample(srv);

    await selectEnumOption(srv.container, "family", "840d-sl");

    // 未改动的预设值回到未设置：输入框为空，示例卡片消失
    await waitFor(() => expect(inputByLabel(srv.container, "Local TSAP").value).toBe(""));
    expect(inputByLabel(srv.container, "Remote TSAP").value).toBe("");
    expect(screen.queryByText("828D 实测连接示例")).toBeNull();

    // 必填校验重新接管：提交被本地校验拦下，请求里没有 1024/3332
    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await screen.findByText(/高级参数缺少必填项/);
    expect(srv.onSubmit).not.toHaveBeenCalled();
    expect(JSON.stringify(srv.calls)).not.toContain("1024");
    expect(JSON.stringify(srv.calls)).not.toContain("3332");
  });

  it("② 手工改过的 TSAP 不因切换系列被误删，且出现「请确认适用性」提示", async () => {
    const srv = await readyPresetPanel();
    await apply828dExample(srv);

    // 用户手工把 Local TSAP 改成 0x0500（1280）——这是用户输入，不得删除
    const tsap = inputByLabel(srv.container, "Local TSAP");
    fireEvent.change(tsap, { target: { value: "0x0500" } });
    fireEvent.blur(tsap);
    await waitFor(() => expect(inputByLabel(srv.container, "Local TSAP").value).toBe("0x0500"));

    await selectEnumOption(srv.container, "family", "840d-sl");

    // 未被改动的 Remote TSAP 仍被清除；被改过的 Local TSAP 保留
    await waitFor(() => expect(inputByLabel(srv.container, "Remote TSAP").value).toBe(""));
    expect(inputByLabel(srv.container, "Local TSAP").value).toBe("0x0500");

    // 要求确认适用性：提示可见、指名来源系列与字段
    const notice = srv.container.querySelector("[data-stale-example]");
    expect(notice).not.toBeNull();
    expect(notice?.getAttribute("data-stale-example")).toBe("local_tsap");
    expect(notice?.textContent).toContain("示例值可能不适用于当前系列");
    expect(notice?.textContent).toContain("Local TSAP");
    expect(notice?.textContent).toContain("来自 828D 的实测示例");
    expect(notice?.textContent).toContain("请确认该值适用于当前控制器");
  });

  it("③ 切回 828d 不自动重新填充：草稿仍无 TSAP，必须用户再次主动应用", async () => {
    const srv = await readyPresetPanel();
    await apply828dExample(srv);
    await selectEnumOption(srv.container, "family", "840d-sl");
    await waitFor(() => expect(inputByLabel(srv.container, "Local TSAP").value).toBe(""));

    // 切回有预设的系列：卡片回来了，但值不会自动写回草稿
    await selectEnumOption(srv.container, "family", "828d");
    await screen.findByText("828D 实测连接示例");
    // 有预设的系列不再显示「暂无实机验证预设」说明（两者互斥）
    expect(screen.queryByText(/暂无经过 Mesa 实机验证的 TSAP 预设/)).toBeNull();
    expect(inputByLabel(srv.container, "Local TSAP").value).toBe("");
    expect(inputByLabel(srv.container, "Remote TSAP").value).toBe("");

    fireEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await screen.findByText(/高级参数缺少必填项/);
    expect(srv.onSubmit).not.toHaveBeenCalled();
    expect(JSON.stringify(srv.calls)).not.toContain("1024");

    // 只有再次点击「应用实测示例」才写回
    fireEvent.click(applyPresetButton());
    await waitFor(() => expect(inputByLabel(srv.container, "Local TSAP").value).toBe("0x0400"));
    expect(inputByLabel(srv.container, "Remote TSAP").value).toBe("0x0D04");
  });

  it("④ 840d-sl：显示「暂无实机验证预设」说明，且该系列没有任何可点击预设", async () => {
    const srv = await readyPresetPanel();
    // 还没选系列时不给结论：说明文案不出现
    expect(srv.container.querySelector("[data-preset-note]")).toBeNull();

    await selectEnumOption(srv.container, "family", "840d-sl");

    // 说明文案（评审给定原文）必须可见
    await screen.findByText(
      /840D sl 暂无经过 Mesa 实机验证的 TSAP 预设，请依据实际控制器配置填写/,
    );
    expect(srv.container.querySelector('[data-preset-note="840d-sl"]')).not.toBeNull();
    // 没有任何可点击预设：既没有「应用实测示例」按钮，也没有预设卡片
    expect(screen.queryByRole("button", { name: /应用实测示例/ })).toBeNull();
    expect(srv.container.querySelector("[data-preset-id]")).toBeNull();

    // 也不自动填充：展开高级区后两个 TSAP 仍为空
    fireEvent.click(screen.getByText(/高级（2）/));
    expect(inputByLabel(srv.container, "Local TSAP").value).toBe("");
    expect(inputByLabel(srv.container, "Remote TSAP").value).toBe("");
  });
});
