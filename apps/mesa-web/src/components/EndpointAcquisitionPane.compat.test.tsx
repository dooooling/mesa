// A3/A4/P1 采集窗格回归（非 NCK 通用资源路径）：
// - A3：已保存配置里带着「界面不认识的字段类型」⇒ 保存禁用且不发 PUT；
// - P1：驱动新增未知必填字段、旧配置未携带 ⇒ 完整 Schema 守卫仍拒绝（且在任何请求前）；
// - A4：点位行用「字段标签 + 值」摘要，替代原来的 JSON.stringify(parameters)。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { UNSUPPORTED_CONFIG_NOTICE } from "../descriptorCompatibility";
import type { ResourceDescriptor } from "../types";

const EP = "ep-generic";

function resource(overrides: Partial<ResourceDescriptor> = {}): ResourceDescriptor {
  return {
    id: "probe",
    label: { default: "Probe" },
    parameters: {
      fields: [
        { key: "unit", label: "寄存器", field_type: "integer", required: false, validation: {}, ui: {} },
        { key: "encoding", label: "编码", field_type: "string", required: false, validation: {}, ui: {} },
      ],
    },
    outputs: [
      { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
    ],
    modes: ["poll"],
    ...overrides,
  };
}

function taskWith(parameters: Record<string, unknown>) {
  return {
    tasks: [
      {
        id: "t1",
        schedule: { mode: "poll", interval_ms: 1000 },
        binding: {
          kind: "mesa.resources.v1",
          config: {
            selections: [
              {
                resource_id: "probe",
                parameters,
                outputs: [{ output: "value", point_key: "probe.value" }],
              },
            ],
          },
        },
      },
    ],
  };
}

function installFetch(
  resources: ResourceDescriptor[],
  tasks: unknown,
  options: { state?: "STOPPED" | "RUNNING" } = {},
) {
  const puts: Array<{ url: string; body: unknown }> = [];
  // P1 证据：记录**所有**请求，用于断言拒绝发生在 stop / PUT / start 之前。
  const calls: Array<{ url: string; method: string }> = [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    calls.push({ url, method });
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/descriptor")) {
      return { ok: true, status: 200, json: async () => ({ resources }) };
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      return { ok: true, status: 200, json: async () => tasks };
    }
    if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
      puts.push({ url, body: init?.body ? JSON.parse(String(init.body)) : undefined });
      return { ok: true, status: 200, json: async () => ({ ok: true }) };
    }
    if (url === "/api/v1/endpoints") {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          endpoints: [
            {
              id: EP,
              driver_id: "drv",
              runtime: { state: options.state ?? "STOPPED" },
            },
          ],
        }),
      };
    }
    if (url === `/api/v1/endpoints/${EP}/stop` || url === `/api/v1/endpoints/${EP}/start`) {
      return { ok: true, status: 200, json: async () => ({ ok: true }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  return { puts, calls };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("A3：未知字段类型阻止保存（只影响该配置）", () => {
  it("已保存配置携带未知类型 → 提示 + 技术详情 + 保存禁用，绕过 disabled 也不发 PUT", async () => {
    // 驱动升级后多出一个 Web 不认识的类型 "geometry"；存量配置里确实带着它。
    const resources = [
      resource({
        parameters: {
          fields: [
            { key: "unit", label: "寄存器", field_type: "integer", required: false, validation: {}, ui: {} },
            {
              key: "shape",
              label: "Shape",
              field_type: "geometry" as ResourceDescriptor["parameters"]["fields"][number]["field_type"],
              required: false,
              validation: {},
              ui: {},
            },
          ],
        },
      }),
    ];
    const srv = installFetch(resources, taskWith({ unit: 3, shape: { x: 1 } }));
    render(<EndpointAcquisitionPane endpointId={EP} driverId="drv" onChanged={() => {}} />);

    expect(await screen.findByTestId("acquisition-unsupported-fields")).toBeTruthy();
    // 字段名与类型只在「技术详情」里出现一次；并标注是存量配置已携带。
    expect(screen.getByText("probe · shape · geometry · 存量配置已携带")).toBeTruthy();

    const save = screen.getByTestId("apply-button") as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(invokeDisabledButton(save)).toBe(true);
    await waitFor(() => expect(srv.puts).toHaveLength(0));
  });

  it("P1：驱动新增未知必填字段、旧配置未携带 → 完整 Schema 守卫拒绝，且不发任何请求", async () => {
    // 驱动升级后新增 required 字段 "shape"（未知类型），存量配置里没有这个键。
    // 旧守卫（unsupportedFieldsInValue）返回空 ⇒ 会放行；新守卫必须按完整 Schema 拒绝。
    const resources = [
      resource({
        parameters: {
          fields: [
            { key: "unit", label: "寄存器", field_type: "integer", required: false, validation: {}, ui: {} },
            {
              key: "shape",
              label: "Shape",
              field_type: "geometry" as ResourceDescriptor["parameters"]["fields"][number]["field_type"],
              required: true,
              validation: {},
              ui: {},
            },
          ],
        },
      }),
    ];
    // RUNNING：若守卫失效，保存会真的发出 stop → PUT → start（便于证明"拒绝在任何请求之前"）。
    const srv = installFetch(resources, taskWith({ unit: 3 }), { state: "RUNNING" });
    render(<EndpointAcquisitionPane endpointId={EP} driverId="drv" onChanged={() => {}} />);

    expect(await screen.findByTestId("acquisition-unsupported-fields")).toBeTruthy();
    expect(
      screen.getByText("probe · shape · geometry · 配置未携带（驱动新增字段）"),
    ).toBeTruthy();

    const save = screen.getByTestId("apply-button") as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    // 记录点击前的请求数：加载阶段的 GET 不计入。
    const before = srv.calls.length;
    // 绕过 disabled，直接触发组件级 onClick：守卫若缺失，这里会真的发 stop/PUT/start。
    expect(invokeDisabledButton(save)).toBe(true);
    await act(async () => {});
    // 拒绝必须发生在任何 stop / PUT / start 之前（也不应有提交前的 preview）。
    expect(srv.calls.length).toBe(before);
    expect(srv.puts).toHaveLength(0);
    expect(srv.calls.some((c) => /\/stop$|\/start$/.test(c.url))).toBe(false);
    expect(srv.calls.some((c) => c.method === "PUT")).toBe(false);
    // 用户可见的拒绝原因来自 Schema 守卫本身（toast，不是"未通过身份核验"那类分支）。
    await waitFor(() => {
      const toast = document.querySelector(".ant-message");
      expect(toast?.textContent ?? "").toContain(UNSUPPORTED_CONFIG_NOTICE);
    });
  });

  it("未知类型只在**未使用**的其它资源里声明 → 不阻断本次保存（限制影响范围）", async () => {
    // 提交只涉及 probe（Schema 干净）；unused 资源声明了未知类型但没被提交。
    const unused = resource({
      id: "unused",
      parameters: {
        fields: [
          {
            key: "shape",
            label: "Shape",
            field_type: "geometry" as ResourceDescriptor["parameters"]["fields"][number]["field_type"],
            required: true,
            validation: {},
            ui: {},
          },
        ],
      },
    });
    const srv = installFetch([resource(), unused], taskWith({ unit: 3 }));
    render(<EndpointAcquisitionPane endpointId={EP} driverId="drv" onChanged={() => {}} />);

    await waitFor(() => expect(screen.getByTestId("apply-button")).toBeTruthy());
    expect(screen.queryByTestId("acquisition-unsupported-fields")).toBeNull();
    const save = screen.getByTestId("apply-button") as HTMLButtonElement;
    expect(save.disabled).toBe(false);
    expect(srv.puts).toHaveLength(0);
  });
});

describe("A4：通用摘要（字段标签 + 值）", () => {
  it("点位行显示 Descriptor 字段 label + 值，不再输出原始 JSON", async () => {
    installFetch([resource()], taskWith({ unit: 3, encoding: "BE" }));
    render(<EndpointAcquisitionPane endpointId={EP} driverId="drv" onChanged={() => {}} />);

    // 标签来自 Descriptor（寄存器 / 编码），顺序按 Schema 声明
    expect(await screen.findByText(/寄存器 3 · 编码 BE/)).toBeTruthy();
    // 旧实现是 JSON.stringify(parameters)
    expect(screen.queryByText(/\{"unit":3/)).toBeNull();
  });

  it("没有参数时不显示空 JSON，回落 point_key", async () => {
    installFetch([resource()], taskWith({}));
    render(<EndpointAcquisitionPane endpointId={EP} driverId="drv" onChanged={() => {}} />);
    await waitFor(() => expect(screen.getByText(/probe\.value/)).toBeTruthy());
    expect(screen.queryByText(/\{\}/)).toBeNull();
    expect(screen.queryByText(/undefined/)).toBeNull();
  });
});
