// 「事件订阅」区域显示判据（§2.5 第 8 条 + §7-8 定案）。
//
// 用户实测问题：不支持事件的驱动也在采集页底部常驻一块「该驱动未声明事件流」的卡片。
// 定稿规则：
// - 判据是 `descriptor.events` 目录是否为空（**不是** driver_id）；
// - 不支持 ⇒ 整块不渲染（连「未声明」文案都不出现）；
// - 支持 ⇒ 保留独立折叠区域（默认收起）及其独立保存范围。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { DeviceAcquisition } from "./DeviceAcquisition";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";

const EP = "ep-events";
const DRIVER = "s7";

const ENDPOINTS: WorkspaceEndpoint[] = [{ id: EP, name: "采集连接 A", driver_id: DRIVER }];

const REGISTER_RESOURCE = {
  id: "register",
  label: { default: "Register" },
  parameters: { fields: [] },
  outputs: [
    { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
  ],
  modes: ["poll"],
};

/** 声明了一条事件流的描述（判据只看这里，不看 driver_id）。 */
function descriptorWithEvents(streams: unknown[]) {
  return {
    contract_major: 2,
    contract_minor: 1,
    identity: { driver_id: DRIVER, name: "S7", version: "0.0.1" },
    connection: { fields: [] },
    resources: [REGISTER_RESOURCE],
    controls: { commands: [] },
    resource_selection_methods: ["manual"],
    capabilities: { poll: true, subscribe: true, write: false, method: false, events: streams.length > 0 },
    events: streams.length > 0 ? { streams } : undefined,
  };
}

const STREAM = {
  id: "alarms",
  label: { default: "Alarms" },
  modes: ["subscribe"],
  parameters: { fields: [] },
  fields: [],
};

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function installFetch(descriptor: unknown) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = (init?.method ?? "GET").toUpperCase();
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [
          { id: EP, driver_id: DRIVER, connection: {}, runtime: { state: "STOPPED" }, device_id: "cnc-01" },
        ],
      });
    }
    if (url.startsWith("/api/v1/drivers/")) return ok(descriptor);
    if (url.startsWith("/api/v1/tasks?endpoint=")) return ok({ tasks: [] });
    if (url.endsWith("/event-tasks") && method === "GET") {
      return ok({ endpoint_id: EP, revision: 1, event_tasks: [] });
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderPage() {
  return render(
    <MemoryRouter>
      <DeviceAcquisition endpoints={ENDPOINTS} endpointsReady onReload={() => {}} />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("事件订阅区域的显示判据 = descriptor.events 目录", () => {
  it("events 目录为空 ⇒ 事件区域整块不存在", async () => {
    installFetch(descriptorWithEvents([]));
    renderPage();
    // 等采集窗格就绪（说明描述已经读到了，判据有结论）。
    await screen.findByTestId("acquisition-interval-input");
    await waitFor(() => expect(screen.queryByTestId("event-subscription-region")).toBeNull());
    // 连「未声明事件流」这类常驻说明也不得出现。
    expect(screen.queryByText(/未声明事件流/)).toBeNull();
    expect(screen.queryByText("事件订阅")).toBeNull();
  }, 60_000);

  it("events 目录非空 ⇒ 独立折叠区域存在且默认收起", async () => {
    const user = userEvent.setup();
    installFetch(descriptorWithEvents([STREAM]));
    renderPage();
    const region = await screen.findByTestId("event-subscription-region");
    expect(region.textContent ?? "").toContain("事件订阅");
    // 默认收起：折叠头 aria-expanded=false，且编辑器内容尚未渲染（不占默认页面空间）。
    const header = region.querySelector(".ant-collapse-header")!;
    expect(header.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("button", { name: /添加订阅/ })).toBeNull();

    // 展开后编辑器出现，保存范围仍在这一块内部（有自己的保存按钮）。
    await user.click(header);
    await waitFor(() => expect(header.getAttribute("aria-expanded")).toBe("true"));
    expect(await screen.findByRole("button", { name: /添加订阅/ })).toBeTruthy();
    expect(region.textContent ?? "").toContain("保存订阅");
  }, 60_000);
});
