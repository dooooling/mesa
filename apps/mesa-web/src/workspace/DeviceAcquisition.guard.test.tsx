// 采集页外壳的草稿守卫（§4.7 / §7-7）：切换连接前提示、保存中禁止切换、刷新提醒。
//
// 这里只测外壳的「是否拦/是否问」，不重复采集窗格内部的行为（见窗格测试）。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { DeviceAcquisition } from "./DeviceAcquisition";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";

const EP1 = "ep-guard-1";
const EP2 = "ep-guard-2";

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

const ENDPOINTS: WorkspaceEndpoint[] = [
  { id: EP1, name: "采集连接 A", driver_id: "sinumerik-nck" },
  { id: EP2, name: "采集连接 B", driver_id: "sinumerik-nck" },
];

const DEFINITIONS = {
  "syn.collect.a": {
    variable_id: "syn.collect.a",
    revision: 1,
    family: "828d",
    scope: "collect" as const,
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
  },
};

function installFetch() {
  const puts: unknown[] = [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [
          { id: EP1, driver_id: "sinumerik-nck", connection: { family: "828d" }, runtime: { state: "STOPPED" } },
          { id: EP2, driver_id: "sinumerik-nck", connection: { family: "828d" }, runtime: { state: "STOPPED" } },
        ],
      });
    }
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/definitions/identity-preview")) {
      const body = JSON.parse(String(init?.body)) as { parameters: Record<string, unknown> };
      const variableId = String(body.parameters.variable_id ?? "");
      const target = (body.parameters.target ?? {}) as Record<string, number>;
      return ok({
        point_key: `${variableId}@channel=${String(target.channel ?? 1)}`,
        identity_mode: "derived",
        scope: "collect",
        variable_id: variableId,
        definition_revision: body.parameters.definition_revision,
        definition_digest: "digest-guard",
      });
    }
    if (url.startsWith("/api/v1/drivers/")) return ok({ resources: [] });
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      // 已有一个 poll 任务：有了「已保存周期」才有可比对的基线（否则周期改动不算变更）。
      return ok({
        tasks: [
          {
            id: "nck-task",
            schedule: { mode: "poll", interval_ms: 1000 },
            binding: {
              kind: "nck.points.v1",
              config: {
                points: [
                  {
                    kind: "nck_variable",
                    point_key: "syn.collect.a@channel=1",
                    parameters: {
                      variable_id: "syn.collect.a",
                      definition_revision: 1,
                      target: { channel: 1 },
                      arguments: {},
                    },
                  },
                ],
              },
            },
          },
        ],
      });
    }
    if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
      puts.push(JSON.parse(String(init?.body)));
      return ok({ ok: true });
    }
    // 事件订阅等其它请求：返回空清单（本用例不关心）。
    if (url.includes("/api/v1/event")) return ok({ streams: [], tasks: [] });
    return { ok: false, status: 404, json: async () => ({}) };
  });
  return { puts };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("切换连接前提示未保存修改", () => {
  it("asks_before_switching_when_draft_is_dirty", async () => {
    const user = userEvent.setup();
    installFetch();
    render(
      <MemoryRouter>
        <DeviceAcquisition endpoints={ENDPOINTS} endpointsReady onReload={() => {}} />
      </MemoryRouter>,
    );
    // 等采集窗格就绪（周期输入出现）。
    await screen.findByTestId("acquisition-interval-input");
    // 无修改时也能正常切换（不打扰）。
    expect(screen.queryByText(/切换连接会丢弃当前未保存的采集配置修改/)).toBeNull();

    // 制造一次真实变更。
    const interval = screen.getByTestId("acquisition-interval-input");
    await user.clear(interval);
    await user.type(interval, "2000");
    await screen.findByTestId("draft-save-bar");

    // 切换连接：先问一次；「继续编辑」后仍停在本连接。
    await user.click(screen.getByTestId("connection-select"));
    await user.click(await screen.findByTitle("采集连接 B"));
    const titles = await screen.findAllByText(/切换连接会丢弃当前未保存的采集配置修改，确定继续？/);
    expect(titles.length).toBeGreaterThan(0);
    await user.click(screen.getByRole("button", { name: "继续编辑" }));
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-interval-input")).toBeTruthy(),
    );
  }, 30_000);
});

describe("刷新页面时的原生离开提醒", () => {
  it("registers_beforeunload_only_when_dirty", async () => {
    const user = userEvent.setup();
    installFetch();
    render(
      <MemoryRouter>
        <DeviceAcquisition endpoints={ENDPOINTS} endpointsReady onReload={() => {}} />
      </MemoryRouter>,
    );
    const interval = await screen.findByTestId("acquisition-interval-input");

    const cleanEvent = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(cleanEvent);
    expect(cleanEvent.defaultPrevented).toBe(false);

    await user.clear(interval);
    await user.type(interval, "2000");
    await screen.findByTestId("draft-save-bar");

    const dirtyEvent = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(dirtyEvent);
    // 有未保存修改 ⇒ 浏览器原生提醒（不静默丢弃）。
    expect(dirtyEvent.defaultPrevented).toBe(true);
  }, 30_000);
});



