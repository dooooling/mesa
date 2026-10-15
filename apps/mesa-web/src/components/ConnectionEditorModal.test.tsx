// ConnectionEditorModal 归属安全（由 EndpointConnectionPane.test 迁移，
// Pane→Modal 容器变更，安全约束不变）：
// A→B 切换时 B 未 ready 前保存必须 disabled；A 的迟到响应不得污染 B。
import { describe, expect, it, vi, beforeEach, type Mock } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { api } from "../api";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { ConnectionEditorModal } from "./ConnectionEditorModal";

vi.mock("../api", () => ({
  api: {
    listEndpoints: vi.fn(),
    getDescriptor: vi.fn(),
    probe: vi.fn(),
    validateConnection: vi.fn(),
    stopEndpoint: vi.fn(),
    updateEndpoint: vi.fn(),
    startEndpoint: vi.fn(),
  },
}));

const mocked = api as unknown as {
  listEndpoints: Mock;
  getDescriptor: Mock;
  probe: Mock;
  validateConnection: Mock;
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
    return { ok: false, status: 404, json: async () => ({}) };
  });
  mocked.listEndpoints.mockResolvedValue({ endpoints: [] });
  // C2：描述符由共享面板通过 api.getDescriptor 加载（不再直连 fetch）
  mocked.getDescriptor.mockResolvedValue(DESC);
  mocked.stopEndpoint.mockResolvedValue({ status: 200, body: {} });
  mocked.updateEndpoint.mockResolvedValue({});
  mocked.startEndpoint.mockResolvedValue({ status: 200, body: {} });
}

function renderModal(ep: string, name: string) {
  return render(
    <ConnectionEditorModal
      key={ep}
      endpointId={ep}
      deviceId="dev"
      driverId="drv"
      initialName={name}
      open
      onClose={() => {}}
      onChanged={() => {}}
    />,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("ConnectionEditorModal 归属安全", () => {
  it("B 未 ready 前保存 disabled（A→B 切换窗口不可写）", async () => {
    const resolvers: Record<string, (v: unknown) => void> = {};
    mockFetchConn(resolvers);
    const { unmount } = renderModal("ep-a", "A");
    // A 先回：保存可用
    await waitFor(() => expect(resolvers["ep-a"]).toBeDefined());
    await act(async () => {
      resolvers["ep-a"]({ id: "ep-a", name: "A", connection: { host: "10.0.0.1" } });
    });
    await waitFor(() => expect(screen.getByRole("button", { name: /保\s*存/ })).toBeTruthy());
    expect((screen.getByRole("button", { name: /保\s*存/ }) as HTMLButtonElement).disabled).toBe(false);
    // 切到 B（父页 key remount 语义）：B 的 GET 在途，快照复位后保存必须禁用
    unmount();
    renderModal("ep-b", "B");
    await waitFor(() => expect(resolvers["ep-b"]).toBeDefined());
    expect((screen.getByRole("button", { name: /保\s*存/ }) as HTMLButtonElement).disabled).toBe(true);
    // B 回来后恢复可用
    await act(async () => {
      resolvers["ep-b"]({ id: "ep-b", name: "B", connection: { host: "10.0.0.2" } });
    });
    await waitFor(() =>
      expect((screen.getByRole("button", { name: /保\s*存/ }) as HTMLButtonElement).disabled).toBe(false),
    );
  });

  it("A 的迟到响应不得污染 B（保存写 B 的连接）", async () => {
    const resolvers: Record<string, (v: unknown) => void> = {};
    mockFetchConn(resolvers);
    const { unmount } = renderModal("ep-a", "A");
    await waitFor(() => expect(resolvers["ep-a"]).toBeDefined());
    // 切到 B：A 的请求仍在途（旧 Modal 已卸载，但 fetch 仍会 resolve）
    unmount();
    renderModal("ep-b", "B");
    await waitFor(() => expect(resolvers["ep-b"]).toBeDefined());
    // B 先回（host=B）
    await act(async () => {
      resolvers["ep-b"]({ id: "ep-b", name: "B", connection: { host: "host-b" } });
    });
    await waitFor(() =>
      expect((screen.getByRole("button", { name: /保\s*存/ }) as HTMLButtonElement).disabled).toBe(false),
    );
    // A 后回（host=A）：旧 Modal 已卸载，cancelled 丢弃；B 输入框仍是 B 的值
    await act(async () => {
      resolvers["ep-a"]({ id: "ep-a", name: "A", connection: { host: "host-a" } });
    });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.getByDisplayValue("host-b")).toBeTruthy();
    expect(screen.queryByDisplayValue("host-a")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// C2：接入共享面板后必须原样保留的既有行为
//   - Secret marker 的保留语义（不回显明文、不被界面产物污染、随 PUT 回传）；
//   - 生命周期顺序与失败语义（Stop 失败不得 Apply / Apply 失败保持 STOPPED /
//     Restart 失败明确提示）；
//   - 含未重新输入凭据的连接禁止连接测试（Secret-safe 边界）。
// ---------------------------------------------------------------------------

const DESC_SECRET = {
  connection: {
    fields: [
      { key: "endpoint_url", label: "Endpoint URL", field_type: "url", required: true, validation: {}, ui: { order: 1 } },
      { key: "password", label: "密码", field_type: "secret", required: false, validation: {}, ui: { order: 2 } },
    ],
  },
};

function mockEdit(opts: {
  connection: Record<string, unknown>;
  descriptor?: unknown;
  state?: string;
}) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async () => ({
    ok: true,
    status: 200,
    json: async () => ({ id: "ep-a", name: "A", connection: opts.connection }),
  }));
  mocked.getDescriptor.mockResolvedValue(opts.descriptor ?? DESC);
  mocked.listEndpoints.mockResolvedValue({ endpoints: [{ id: "ep-a", state: opts.state ?? "STOPPED" }] });
  mocked.stopEndpoint.mockResolvedValue({ status: 200, body: {} });
  mocked.updateEndpoint.mockResolvedValue({});
  mocked.startEndpoint.mockResolvedValue({ status: 200, body: {} });
  mocked.probe.mockResolvedValue({ status: 200, body: { reachable: true } });
  mocked.validateConnection.mockResolvedValue({ status: 200, body: {} });
}

async function waitSaveEnabled() {
  await waitFor(() =>
    expect((screen.getByRole("button", { name: /保\s*存/ }) as HTMLButtonElement).disabled).toBe(false),
  );
}

describe("C2 ConnectionEditorModal：Secret marker 语义", () => {
  it("未触碰的凭据以 marker 原样随 PUT 回传（不被界面产物污染）", async () => {
    mockEdit({
      connection: { endpoint_url: "opc.tcp://10.0.0.2:4840", password: { secret_set: true } },
      descriptor: DESC_SECRET,
    });
    renderModal("ep-a", "A");
    await waitSaveEnabled();

    // 凭据输入框必须为空（不伪显示 [object Object] 或密码）
    const pwd = document.querySelector('input[type="password"]') as HTMLInputElement;
    expect(pwd.value).toBe("");
    expect(pwd.placeholder).toBe("已设置，留空保持不变");

    fireEvent.click(screen.getByRole("button", { name: /保\s*存/ }));
    await waitFor(() => expect(mocked.updateEndpoint).toHaveBeenCalledTimes(1));
    const body = mocked.updateEndpoint.mock.calls[0][1] as {
      connection: Record<string, unknown>;
      driver_id?: unknown;
    };
    expect(body.connection.password).toEqual({ secret_set: true });
    expect(body.connection.endpoint_url).toBe("opc.tcp://10.0.0.2:4840");
    // driver_id 创建后不可变：请求体不得携带
    expect(body.driver_id).toBeUndefined();
  });

  it("Secret-safe：存在未重新输入的 marker 时禁止连接测试，且不发探测请求", async () => {
    mockEdit({
      connection: { endpoint_url: "opc.tcp://10.0.0.2:4840", password: { secret_set: true } },
      descriptor: DESC_SECRET,
    });
    renderModal("ep-a", "A");
    await waitSaveEnabled();

    expect(await screen.findByText(/已保存的凭据不会回显/)).toBeTruthy();
    const probeBtn = screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement;
    const validateBtn = screen.getByRole("button", { name: /校验参数/ }) as HTMLButtonElement;
    expect(probeBtn.disabled).toBe(true);
    expect(validateBtn.disabled).toBe(true);
    expect(mocked.probe).not.toHaveBeenCalled();
    expect(mocked.validateConnection).not.toHaveBeenCalled();
  });
});

describe("C2 ConnectionEditorModal：生命周期顺序与失败语义", () => {
  it("RUNNING + 勾选重启：严格 Stop → Apply → Start", async () => {
    mockEdit({ connection: { host: "10.0.0.1" }, state: "RUNNING" });
    const order: string[] = [];
    mocked.stopEndpoint.mockImplementation(async () => {
      order.push("stop");
      return { status: 200, body: {} };
    });
    mocked.updateEndpoint.mockImplementation(async () => {
      order.push("apply");
      return {};
    });
    mocked.startEndpoint.mockImplementation(async () => {
      order.push("start");
      return { status: 200, body: {} };
    });
    renderModal("ep-a", "A");
    await screen.findByText(/Endpoint 正在运行/);
    await waitFor(() =>
      expect((screen.getByRole("button", { name: /停止并应用/ }) as HTMLButtonElement).disabled).toBe(false),
    );

    fireEvent.click(screen.getByRole("button", { name: /停止并应用/ }));
    await waitFor(() => expect(order).toEqual(["stop", "apply", "start"]));
    expect(screen.getByText(/修改成功，已恢复运行/)).toBeTruthy();
  });

  it("Stop 失败：中止应用，绝不 Apply / Start", async () => {
    mockEdit({ connection: { host: "10.0.0.1" }, state: "RUNNING" });
    mocked.stopEndpoint.mockResolvedValue({ status: 500, body: { error: { message: "stop boom" } } });
    renderModal("ep-a", "A");
    await screen.findByText(/Endpoint 正在运行/);
    await waitFor(() =>
      expect((screen.getByRole("button", { name: /停止并应用/ }) as HTMLButtonElement).disabled).toBe(false),
    );

    fireEvent.click(screen.getByRole("button", { name: /停止并应用/ }));
    await waitFor(() => expect(screen.getByText(/停止失败，已中止应用，未修改连接/)).toBeTruthy());
    await new Promise((r) => setTimeout(r, 400));
    expect(mocked.updateEndpoint).not.toHaveBeenCalled();
    expect(mocked.startEndpoint).not.toHaveBeenCalled();
  });

  it("Apply 失败：保持 STOPPED，不 Start", async () => {
    mockEdit({ connection: { host: "10.0.0.1" }, state: "RUNNING" });
    mocked.updateEndpoint.mockRejectedValue(new Error("apply boom"));
    renderModal("ep-a", "A");
    await screen.findByText(/Endpoint 正在运行/);
    await waitFor(() =>
      expect((screen.getByRole("button", { name: /停止并应用/ }) as HTMLButtonElement).disabled).toBe(false),
    );

    fireEvent.click(screen.getByRole("button", { name: /停止并应用/ }));
    await waitFor(() => expect(screen.getByText(/应用失败，Endpoint 当前已停止/)).toBeTruthy());
    expect(mocked.startEndpoint).not.toHaveBeenCalled();
  });

  it("Restart 失败：明确提示「配置已保存，但恢复运行失败」", async () => {
    mockEdit({ connection: { host: "10.0.0.1" }, state: "RUNNING" });
    mocked.startEndpoint.mockResolvedValue({ status: 500, body: { error: { message: "start boom" } } });
    renderModal("ep-a", "A");
    await screen.findByText(/Endpoint 正在运行/);
    await waitFor(() =>
      expect((screen.getByRole("button", { name: /停止并应用/ }) as HTMLButtonElement).disabled).toBe(false),
    );

    fireEvent.click(screen.getByRole("button", { name: /停止并应用/ }));
    await waitFor(() =>
      expect(screen.getByText(/配置已保存，但恢复运行失败：start boom/)).toBeTruthy(),
    );
  });
});

// ---------------------------------------------------------------------------
// P0：描述符失败时编辑保存的所有出口都必须失效
//
// 编辑保存的完整出口是「PUT /endpoints/{id} + Stop→Apply→（可选）Restart」，
// 描述符失败意味着字段集合不可信（visible_if / 类型 / 范围都无从判断），
// 此时写出去可能是残缺连接 —— 按钮 disabled 之外，提交函数自身也必须挡住。
// ---------------------------------------------------------------------------

describe("C2 ConnectionEditorModal：P0 描述符失败（提交出口全失效）", () => {
  /** 记录所有 fetch 调用（含方法），用于「零写请求」的全量兜底断言。 */
  function mockDescriptorFailure(state: string) {
    const fetchCalls: Array<{ method: string; url: string }> = [];
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
      fetchCalls.push({ method: init?.method ?? "GET", url });
      return {
        ok: true,
        status: 200,
        json: async () => ({ id: "ep-a", name: "A", connection: { host: "10.0.0.1" } }),
      };
    });
    mocked.getDescriptor.mockRejectedValue(
      new Error("500 /api/v1/drivers/drv/descriptor: 描述符挂了"),
    );
    mocked.listEndpoints.mockResolvedValue({ endpoints: [{ id: "ep-a", state }] });
    mocked.stopEndpoint.mockResolvedValue({ status: 200, body: {} });
    mocked.updateEndpoint.mockResolvedValue({});
    mocked.startEndpoint.mockResolvedValue({ status: 200, body: {} });
    mocked.probe.mockResolvedValue({ status: 200, body: { reachable: true } });
    mocked.validateConnection.mockResolvedValue({ status: 200, body: {} });
    return fetchCalls;
  }

  it("STOPPED：显示错误 + 重试、三条路径禁用，绕过 disabled 也不发 PUT / 不 probe", async () => {
    const fetchCalls = mockDescriptorFailure("STOPPED");
    renderModal("ep-a", "A");

    await screen.findByText("驱动描述符加载失败");
    expect(screen.getByText(/描述符挂了/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /重\s*试/ })).toBeTruthy();

    const save = screen.getByRole("button", { name: /保\s*存/ }) as HTMLButtonElement;
    const probeBtn = screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement;
    const validateBtn = screen.getByRole("button", { name: /校验参数/ }) as HTMLButtonElement;
    expect([save.disabled, probeBtn.disabled, validateBtn.disabled]).toEqual([true, true, true]);

    await act(async () => {
      // 探针必须真的调用到处理函数（否则用例是假绿）
      expect(invokeDisabledButton(save)).toBe(true);
      expect(invokeDisabledButton(probeBtn)).toBe(true);
      expect(invokeDisabledButton(validateBtn)).toBe(true);
    });

    expect(mocked.updateEndpoint).not.toHaveBeenCalled();
    expect(mocked.probe).not.toHaveBeenCalled();
    expect(mocked.validateConnection).not.toHaveBeenCalled();
    expect(fetchCalls.filter((c) => c.method !== "GET")).toEqual([]);
  });

  it("RUNNING：Stop → Apply → Restart 整条链都不得启动", async () => {
    const fetchCalls = mockDescriptorFailure("RUNNING");
    renderModal("ep-a", "A");

    await screen.findByText("驱动描述符加载失败");
    const apply = screen.getByRole("button", { name: /停止并应用/ }) as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    await act(async () => {
      expect(invokeDisabledButton(apply)).toBe(true);
    });

    expect(mocked.stopEndpoint).not.toHaveBeenCalled();
    expect(mocked.updateEndpoint).not.toHaveBeenCalled();
    expect(mocked.startEndpoint).not.toHaveBeenCalled();
    expect(fetchCalls.filter((c) => c.method !== "GET")).toEqual([]);
  });
});
