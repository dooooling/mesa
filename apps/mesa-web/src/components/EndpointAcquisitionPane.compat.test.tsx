// A3/A4 采集窗格回归（非 NCK 通用资源路径）：
// - A3：已保存配置里带着「界面不认识的字段类型」⇒ 保存禁用且不发 PUT；
// - A4：点位行用「字段标签 + 值」摘要，替代原来的 JSON.stringify(parameters)。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { invokeDisabledButton } from "../test/bypassDisabled";
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

function installFetch(resources: ResourceDescriptor[], tasks: unknown) {
  const puts: Array<{ url: string; body: unknown }> = [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
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
        json: async () => ({ endpoints: [{ id: EP, driver_id: "drv", runtime: { state: "STOPPED" } }] }),
      };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  return { puts };
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
    // 字段名与类型只在「技术详情」里出现一次
    expect(screen.getByText("probe.value · shape · geometry")).toBeTruthy();

    const save = screen.getByTestId("apply-button") as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(invokeDisabledButton(save)).toBe(true);
    await waitFor(() => expect(srv.puts).toHaveLength(0));
  });

  it("未知类型只在 Descriptor 里声明、配置未携带 → 不阻断保存（限制影响范围）", async () => {
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
    const srv = installFetch(resources, taskWith({ unit: 3 }));
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
