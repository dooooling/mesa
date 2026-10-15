// AddConnectionModal（C2）：共享面板接入后的提交出口回归。
//
// P0 重点：驱动清单失败 / 描述符失败时，**所有**提交路径都必须失效——
// 不只是按钮 disabled，提交函数自身也必须有前置守卫。这里的用例除了断言
// 按钮禁用与页面报错/重试入口之外，还用 bypassDisabled 探针直接调用按钮组件级
// onClick（绕过 React + AntD 对 disabled 的双重拦截），证明「禁用被绕过」时
// 依然不会发出 POST /api/v1/endpoints。
import { describe, expect, it, vi, beforeEach, afterEach, type Mock } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { api } from "../api";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { AddConnectionModal } from "./AddConnectionModal";

vi.mock("../api", () => ({
  api: {
    listDrivers: vi.fn(),
    getDescriptor: vi.fn(),
    validateConnection: vi.fn(),
    probe: vi.fn(),
    createEndpoint: vi.fn(),
  },
}));

const mocked = api as unknown as {
  listDrivers: Mock;
  getDescriptor: Mock;
  validateConnection: Mock;
  probe: Mock;
  createEndpoint: Mock;
};

const DESC = {
  connection: {
    fields: [
      { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
    ],
  },
};

const DRIVERS = [{ id: "s7", name: "S7", launchable: true }];

/** 记录所有 fetch 调用（方法 + URL），用于「零写请求」的全量兜底断言。 */
function spyFetch(): Array<{ method: string; url: string }> {
  const calls: Array<{ method: string; url: string }> = [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ method: init?.method ?? "GET", url });
    return { ok: false, status: 500, json: async () => ({}) };
  });
  return calls;
}

function renderModal() {
  return render(
    <AddConnectionModal deviceId="dev-1" open onClose={() => {}} onCreated={() => {}} />,
  );
}

function button(name: RegExp): HTMLButtonElement {
  return screen.getByRole("button", { name }) as HTMLButtonElement;
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("C2 AddConnectionModal：P0 提交出口", () => {
  it("驱动清单失败：显示错误 + 重试、三条路径禁用，绕过 disabled 也零写请求", async () => {
    mocked.listDrivers.mockRejectedValue(new Error("500 /api/v1/drivers: 清单挂了"));
    const fetchCalls = spyFetch();
    renderModal();

    await screen.findByText("驱动清单加载失败");
    expect(screen.getByText(/清单挂了/)).toBeTruthy();
    expect(button(/重\s*试/)).toBeTruthy();
    // 硬编码 fallback 驱动文案不得出现
    expect(screen.queryByText("Siemens S7")).toBeNull();
    expect(screen.queryByText("FANUC FOCAS2")).toBeNull();
    expect(screen.queryByText("OPC UA")).toBeNull();

    const probeBtn = button(/测试连接/);
    const validateBtn = button(/校验参数/);
    const submitBtn = button(/创\s*建/);
    expect([probeBtn.disabled, validateBtn.disabled, submitBtn.disabled]).toEqual([true, true, true]);

    await act(async () => {
      // 探针必须真的调用到处理函数（否则用例是假绿）
      expect(invokeDisabledButton(probeBtn)).toBe(true);
      expect(invokeDisabledButton(validateBtn)).toBe(true);
      expect(invokeDisabledButton(submitBtn)).toBe(true);
    });

    expect(mocked.createEndpoint).not.toHaveBeenCalled();
    expect(mocked.probe).not.toHaveBeenCalled();
    expect(mocked.validateConnection).not.toHaveBeenCalled();
    // 全量兜底：连一个非 GET 请求都没有
    expect(fetchCalls.filter((c) => c.method !== "GET")).toEqual([]);
  });

  it("描述符失败：显示错误 + 重试，绕过 disabled 也零写请求", async () => {
    mocked.listDrivers.mockResolvedValue({ drivers: DRIVERS });
    mocked.getDescriptor.mockRejectedValue(new Error("500 /api/v1/drivers/s7/descriptor: 描述符挂了"));
    const fetchCalls = spyFetch();
    renderModal();

    await screen.findByText("驱动描述符加载失败");
    expect(screen.getByText(/描述符挂了/)).toBeTruthy();
    expect(button(/重\s*试/)).toBeTruthy();

    // 关键：把名称填好（antd 表单校验会拦住空名称，那会掩盖真正要验证的守卫），
    // 这样一旦提交函数没有守卫，就真的会发出 POST /api/v1/endpoints。
    fireEvent.change(screen.getByPlaceholderText("如 PLC / NCK / OPC UA"), {
      target: { value: "PLC" },
    });

    const submitBtn = button(/创\s*建/);
    expect(submitBtn.disabled).toBe(true);
    await act(async () => {
      expect(invokeDisabledButton(submitBtn)).toBe(true);
    });

    expect(mocked.createEndpoint).not.toHaveBeenCalled();
    expect(fetchCalls.filter((c) => c.method !== "GET")).toEqual([]);
  });

  it("清单 + 描述符就绪时可以正常创建（证明前置门是活的，不是永久关闭）", async () => {
    mocked.listDrivers.mockResolvedValue({ drivers: DRIVERS });
    mocked.getDescriptor.mockResolvedValue(DESC);
    mocked.createEndpoint.mockResolvedValue({ status: 201, body: {} });
    renderModal();

    await waitFor(() => expect(button(/创\s*建/).disabled).toBe(false));
    // Modal 渲染在 portal 里，字段锚点要从 document 取
    const host = document.querySelector('[data-field-key="host"] input') as HTMLInputElement;
    fireEvent.change(host, { target: { value: "10.0.0.9" } });
    fireEvent.change(screen.getByPlaceholderText("如 PLC / NCK / OPC UA"), {
      target: { value: "PLC" },
    });

    fireEvent.click(button(/创\s*建/));
    await waitFor(() => expect(mocked.createEndpoint).toHaveBeenCalledTimes(1));
    const payload = mocked.createEndpoint.mock.calls[0][0] as {
      device_id: string;
      driver_id: string;
      name: string;
      connection: Record<string, unknown>;
    };
    expect(payload.device_id).toBe("dev-1");
    expect(payload.driver_id).toBe("s7");
    expect(payload.name).toBe("PLC");
    expect(payload.connection.host).toBe("10.0.0.9");
  });
});
