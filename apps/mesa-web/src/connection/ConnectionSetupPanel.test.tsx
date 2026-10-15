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
