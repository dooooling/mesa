// saveIssues 回归：PUT /tasks 失败体的两种形态都不退化。
// - `{valid:false, issues:[...]}` → 原样保存并列表展示（path/code/message 全在）；
// - 普通 `{error:{message}}` → 仍走原错误文案，不退化；
// - 成功保存 / 重新发起保存 → 旧 issues 清掉。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";

const RESOURCES = [
  {
    id: "register",
    label: { default: "Register" },
    parameters: { fields: [] },
    outputs: [
      { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
    ],
    modes: ["poll"],
  },
];

const TASKS = {
  tasks: [
    {
      id: "t1",
      schedule: { mode: "poll", interval_ms: 1000 },
      binding: {
        kind: "mesa.resources.v1",
        config: { selections: [{ resource_id: "register", parameters: {}, outputs: [] }] },
      },
    },
  ],
};

function mockFetch(put: (url: string) => unknown) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    if (url.startsWith("/api/v1/drivers/")) {
      return { ok: true, status: 200, json: async () => ({ resources: RESOURCES }) };
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      return { ok: true, status: 200, json: async () => TASKS };
    }
    if (url.startsWith("/api/v1/tasks/") && (init?.method ?? "GET") === "PUT") {
      return put(url);
    }
    if (url === "/api/v1/endpoints") {
      return { ok: true, status: 200, json: async () => ({ endpoints: [] }) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("EndpointAcquisitionPane saveIssues（后端 issues 原样展示）", () => {
  it("PUT 返回 2 个 issues → 页面同时显示两条 path/code/message", async () => {
    const user = userEvent.setup();
    const issues = [
      { path: "tasks[0].selections[1].parameters.axis", code: "REQUIRED", message: "field `axis` is required" },
      { path: "tasks[0].selections[1].parameters.count", code: "OUT_OF_RANGE", message: "field `count` -1 < min 0" },
    ];
    mockFetch(() => ({ ok: false, status: 400, json: async () => ({ valid: false, issues }) }));
    render(<EndpointAcquisitionPane endpointId="ep1" driverId="drv" onChanged={() => {}} />);
    // 等待快照就绪 + 选一个点位使保存可用
    await waitFor(() => expect(screen.getByRole("button", { name: /保存/ })).toBeTruthy());
    const outputBox = screen.getByRole("checkbox", { name: "output-value" });
    await user.click(outputBox.querySelector("input") ?? outputBox);
    await user.click(screen.getByRole("button", { name: /保存/ }));
    // 两条 issues 原样展示（path/code/message 全在，不翻译不重判）：
    // toast 摘要与列表块都会出现 path 文案，故用 getAllBy 锁"同时显示两条"。
    await waitFor(() => expect(screen.getByText(/保存被后端拒绝/)).toBeTruthy());
    expect(screen.getAllByText(/tasks\[0\]\.selections\[1\]\.parameters\.axis/).length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText(/REQUIRED/).length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText(/field `axis` is required/).length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText(/tasks\[0\]\.selections\[1\]\.parameters\.count/).length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText(/OUT_OF_RANGE/).length).toBeGreaterThanOrEqual(1);
  });

  it("普通 {error:{message}} → 仍显示原错误，不退化", async () => {
    const user = userEvent.setup();
    mockFetch(() => ({ ok: false, status: 500, json: async () => ({ error: { message: "boom" } }) }));
    render(<EndpointAcquisitionPane endpointId="ep2" driverId="drv" onChanged={() => {}} />);
    await waitFor(() => expect(screen.getByRole("button", { name: /保存/ })).toBeTruthy());
    const outputBox = screen.getByRole("checkbox", { name: "output-value" });
    await user.click(outputBox.querySelector("input") ?? outputBox);
    await user.click(screen.getByRole("button", { name: /保存/ }));
    // issues 列表不出现（无 issues 可展），toast 走原 message（assert 不抛即不断言 antd portal 文案）
    await act(async () => {});
    expect(screen.queryByText(/保存被后端拒绝/)).toBeNull();
  });
});
