// P0-2 Browse 回归：浏览树下钻 + 选用回填只依赖节点信封，不解释协议。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ResourceBrowseAntd } from "./ResourceBrowseAntd";

const calls: Array<{ url: string; body: unknown }> = [];

const BRANCH = { id: "nck://C", label: "通道 (C)", kind: "group", has_children: true };
const LEAF = {
  id: "nck://C/SEMA/axConf",
  label: "SEMA/axConf",
  kind: "variable",
  data_type: "F64",
  access: "read",
  has_children: false,
  binding_json: JSON.stringify({
    resource_id: "variable",
    parameters: { area: "C", block: "SEMA", variable: "axConf" },
  }),
};
const LEAF2 = {
  id: "nck://C/SEMA/axConf2",
  label: "SEMA/axConf2",
  kind: "variable",
  has_children: false,
  binding_json: JSON.stringify({
    resource_id: "variable",
    parameters: { area: "C", block: "SEMA", variable: "axConf2" },
  }),
};

function mockFetch() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const body = JSON.parse(init?.body as string) as { parent: string; cursor: string };
    calls.push({ url, body });
    if (body.parent === "") {
      return { ok: true, json: async () => ({ nodes: [BRANCH, LEAF], next_cursor: null }) };
    }
    if (body.parent === "nck://C" && !body.cursor) {
      return { ok: true, json: async () => ({ nodes: [LEAF2], next_cursor: "1" }) };
    }
    if (body.cursor === "1") {
      return { ok: true, json: async () => ({ nodes: [], next_cursor: null }) };
    }
    return { ok: true, json: async () => ({ nodes: [], next_cursor: null }) };
  });
}

beforeEach(() => {
  calls.length = 0;
  mockFetch();
});

describe("ResourceBrowseAntd", () => {
  it("根加载 + 分支进入 + 叶选用回填（binding 直转 selection）", async () => {
    const user = userEvent.setup();
    const onFill = vi.fn();
    render(<ResourceBrowseAntd endpointId="ep1" onFill={onFill} />);

    // 根节点：分支只有进入，叶只有选用
    await screen.findByText("通道 (C)");
    await screen.findByText("SEMA/axConf");
    const branchRow = screen.getByText("通道 (C)").closest("div") as HTMLElement;
    expect(branchRow.textContent).toMatch(/进\s*入/);
    expect(branchRow.textContent).not.toMatch(/选\s*用/);

    // 下钻分支：请求带 parent
    await user.click(screen.getByText(/进\s*入/));
    await waitFor(() => {
      expect(calls.some((c) => (c.body as { parent: string }).parent === "nck://C")).toBe(true);
    });
    await screen.findByText("SEMA/axConf2");

    // 回根
    await user.click(screen.getByText(/回\s*根/));
    await screen.findByText("SEMA/axConf");

    // 选用叶节点：binding_json → {resource_id, parameters}
    const leafRow = screen.getByText("SEMA/axConf").closest("div") as HTMLElement;
    const pickBtn = Array.from(leafRow.querySelectorAll("button")).find((b) => /选\s*用/.test(b.textContent ?? ""))!;
    await user.click(pickBtn);
    expect(onFill).toHaveBeenCalledTimes(1);
    expect(onFill.mock.calls[0][0]).toEqual({
      resource_id: "variable",
      parameters: { area: "C", block: "SEMA", variable: "axConf" },
    });
  });

  it("next_cursor 翻页追加", async () => {
    const user = userEvent.setup();
    render(<ResourceBrowseAntd endpointId="ep1" onFill={() => {}} />);
    await screen.findByText("通道 (C)");
    await user.click(screen.getByText(/进\s*入/));
    await screen.findByText("下一页");
    await user.click(screen.getByText("下一页"));
    await waitFor(() => {
      expect(calls.some((c) => (c.body as { cursor: string }).cursor === "1")).toBe(true);
    });
  });
});
