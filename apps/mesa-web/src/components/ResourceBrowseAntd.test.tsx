// P0-2 Browse 回归：浏览树下钻 + 选用回填只依赖节点信封，不解释协议。
// PR #150：语义判定不再写在组件里，改由调用方注入 `BrowseLeafPolicy`；
// 本文件的语义用例通过 `createNckBrowseLeafPolicy` 注入，断言逐条保持不变。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ResourceBrowseAntd } from "./ResourceBrowseAntd";
import { createNckBrowseLeafPolicy } from "../nckBrowsePolicy";
import type { NckDefinitionLike } from "../nckPointModel";

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

function installNodes(nodes: unknown[]) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async () => ({
    ok: true,
    json: async () => ({ nodes, next_cursor: null }),
  }));
}

function semanticNode(args: {
  id?: string;
  scopePreview?: string | null;
  includeIdentity?: boolean;
  kind?: string;
} = {}) {
  const identity = {
    template: "channel.program.status@channel={channel}",
    keys: [{ path: "channel", required: true }],
    preview_point_key:
      args.scopePreview === undefined
        ? "channel.program.status@channel=i:1"
        : args.scopePreview,
  };
  return {
    id: args.id ?? "nck://presets/semantic",
    label: "通道程序状态",
    kind: args.kind ?? "nck_variable",
    has_children: false,
    binding_json: JSON.stringify({
      kind: args.kind ?? "nck_variable",
      parameters: {
        variable_id: "channel.program.status",
        definition_revision: 1,
        target: { channel: 1 },
        arguments: {},
      },
      ...(args.includeIdentity === false ? {} : { identity }),
    }),
  };
}

function collectDefinition(overrides: Partial<NckDefinitionLike> = {}): NckDefinitionLike {
  return {
    variable_id: "channel.program.status",
    revision: 1,
    family: "828d",
    scope: "collect",
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    ...overrides,
  };
}

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

describe("PR-C 第三批：语义叶准入与路由（A1/A13/A16/A44）", () => {
  const policy = (args: {
    definitions?: NckDefinitionLike[] | null;
    endpointFamily?: string | null;
    catalogFamily?: string | null;
  } = {}) =>
    createNckBrowseLeafPolicy({
      definitions: args.definitions === undefined ? [collectDefinition()] : args.definitions,
      endpointFamily: args.endpointFamily === undefined ? "828d" : args.endpointFamily,
      catalogFamily: args.catalogFamily ?? null,
    });

  it("semantic_leaf_routes_to_semantic_form_without_using_browse_key", async () => {
    const user = userEvent.setup();
    const onFill = vi.fn();
    const onLeafSelect = vi.fn();
    installNodes([semanticNode()]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={onFill}
        leafPolicy={policy()}
        onLeafSelect={onLeafSelect}
      />,
    );
    await screen.findByText("通道程序状态");
    expect(screen.getByText(/Browse 预览/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /选用（语义）/ }));
    expect(onLeafSelect).toHaveBeenCalledTimes(1);
    const sel = onLeafSelect.mock.calls[0][0] as {
      pointKind: string;
      parameters: Record<string, unknown>;
      identity?: { preview_point_key?: string | null };
    };
    expect(sel.pointKind).toBe("nck_variable");
    expect(sel.parameters).not.toHaveProperty("identity");
    expect(sel.identity?.preview_point_key).toContain("channel=i:1");
    // 语义叶绝不回落到通用 resource 回填（那会走旧变量入口）。
    expect(onFill).not.toHaveBeenCalled();
  });

  it("identity_scope_leaf_is_not_selectable_with_reason (A13)", async () => {
    installNodes([semanticNode()]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        leafPolicy={policy({ definitions: [collectDefinition({ scope: "identity" })] })}
        onLeafSelect={() => {}}
      />,
    );
    await screen.findByText("通道程序状态");
    expect(screen.getByTestId("browse-semantic-block").textContent).toContain("SCOPE_IDENTITY");
    expect(screen.queryByRole("button", { name: /选用（语义）/ })).toBeNull();
  });

  it("missing_definition_or_family_mismatch_blocks_semantic_select", async () => {
    installNodes([semanticNode()]);
    const { rerender } = render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        leafPolicy={policy({ definitions: [] })}
        onLeafSelect={() => {}}
      />,
    );
    await screen.findByText("通道程序状态");
    expect(screen.getByTestId("browse-semantic-block").textContent).toContain("找不到");

    installNodes([semanticNode()]);
    rerender(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        leafPolicy={policy({ definitions: [collectDefinition({ family: "840d-sl" })] })}
        onLeafSelect={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId("browse-semantic-block").textContent).toContain("FAMILY_MISMATCH"),
    );
    expect(screen.queryByRole("button", { name: /选用（语义）/ })).toBeNull();
  });

  it("leaf_without_identity_is_not_selectable (A16)", async () => {
    installNodes([semanticNode({ includeIdentity: false })]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        leafPolicy={policy()}
        onLeafSelect={() => {}}
      />,
    );
    await screen.findByText("通道程序状态");
    expect(screen.getByTestId("browse-semantic-block").textContent).toContain("identity");
    expect(screen.queryByRole("button", { name: /选用（语义）/ })).toBeNull();
  });

  it("null_browse_preview_still_opens_semantic_form_with_reason (A10)", async () => {
    const user = userEvent.setup();
    const onLeafSelect = vi.fn();
    installNodes([semanticNode({ scopePreview: null })]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        leafPolicy={policy()}
        onLeafSelect={onLeafSelect}
      />,
    );
    await screen.findByText("通道程序状态");
    expect(screen.getByText(/身份未确定/)).toBeTruthy();
    // preview 为 null 仅禁止“提交”，不禁止进入语义表单补全。
    await user.click(screen.getByRole("button", { name: /选用（语义）/ }));
    expect(onLeafSelect).toHaveBeenCalledTimes(1);
  });

  it("semantic_leaf_without_semantic_entry_is_closed_not_fallback", async () => {
    installNodes([semanticNode()]);
    const onFill = vi.fn();
    render(
      <ResourceBrowseAntd endpointId="ep1" onFill={onFill} leafPolicy={policy()} />,
    );
    await screen.findByText("通道程序状态");
    expect(screen.getByTestId("browse-semantic-block").textContent).toContain("未提供语义表单入口");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
    expect(onFill).not.toHaveBeenCalled();
  });

  it("legacy_variable_fallback_is_not_offered_in_semantic_context (A22)", async () => {
    installNodes([
      {
        id: "legacy",
        label: "旧变量叶",
        kind: "variable",
        has_children: false,
        binding_json: JSON.stringify({
          resource_id: "variable",
          parameters: { area: "C", module: 21, column: 1, count: 1 },
        }),
      },
    ]);
    const onFill = vi.fn();
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={onFill}
        leafPolicy={policy()}
        onLeafSelect={() => {}}
      />,
    );
    await screen.findByText("旧变量叶");
    expect(screen.getByTestId("browse-legacy-variable-block").textContent).toContain(
      "语义载荷不可用",
    );
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
    expect(onFill).not.toHaveBeenCalled();
  });

  it("semantic_payload_without_injected_policy_is_fail_closed", async () => {
    // 通用组件对语义载荷的唯一安全动作：不给入口、给出通用原因（不认识 NCK）。
    installNodes([semanticNode()]);
    const onFill = vi.fn();
    render(<ResourceBrowseAntd endpointId="ep1" onFill={onFill} />);
    await screen.findByText("通道程序状态");
    expect(screen.getByTestId("browse-leaf-block").textContent).toContain("驱动语义层解释");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
    expect(onFill).not.toHaveBeenCalled();
  });
});

// 解耦验证：通用组件对**虚构**驱动语义同样生效（不认识任何真实厂商字面量）。
describe("Browse 叶策略 seam（不含任何厂商语义）", () => {
  it("fake_vendor_policy_drives_badges_reasons_and_payload", async () => {
    const user = userEvent.setup();
    const onFill = vi.fn();
    const onLeafSelect = vi.fn();
    installNodes([
      {
        id: "widget://lamp",
        label: "指示灯",
        kind: "widget",
        has_children: false,
        binding_json: JSON.stringify({
          kind: "widget_lamp",
          parameters: { lamp: 3 },
          identity: { preview_point_key: "lamp@3" },
        }),
      },
      {
        id: "widget://blocked",
        label: "被锁定的部件",
        kind: "widget",
        has_children: false,
        binding_json: JSON.stringify({
          kind: "widget_lamp",
          parameters: { lamp: 9 },
          identity: { preview_point_key: "lamp@9" },
        }),
      },
    ]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={onFill}
        leafPolicy={{
          name: "widget",
          missingHandlerReason: "缺少部件表单入口：不可选用",
          evaluate: (node) => {
            const payload = node.binding_json ? JSON.parse(node.binding_json) : null;
            if (payload?.kind !== "widget_lamp") return { mode: "generic" };
            const blocked = payload.parameters?.lamp === 9;
            return {
              mode: "owned",
              selectable: !blocked,
              reason: blocked ? "WIDGET_LOCKED：该部件被锁定" : undefined,
              selection: blocked ? undefined : { lamp: payload.parameters.lamp },
              selectLabel: "选用（部件）",
              badge: "部件点位",
              tags: ["widget_lamp"],
              preview: { text: `预览：${String(payload.identity.preview_point_key)}`, tone: "muted" },
            };
          },
        }}
        onLeafSelect={onLeafSelect}
      />,
    );
    await screen.findByText("指示灯");
    expect(screen.getByText("预览：lamp@3")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /选用（部件）/ }));
    expect(onLeafSelect).toHaveBeenCalledWith({ lamp: 3 });
    // 被锁定的叶：只显示原因，没有入口，也绝不回落通用回填。
    expect(screen.getByTestId("browse-leaf-block").textContent).toContain("WIDGET_LOCKED");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).not.toBeNull();
    expect(screen.getAllByRole("button", { name: /选用（部件）/ })).toHaveLength(1);
    expect(onFill).not.toHaveBeenCalled();
  });
});

// A15：Browse 503 / 网络异常 / 响应不可解析时，不得出现可操作的非法语义叶。
function installBrowseFailure(kind: "http503" | "network" | "unparseable") {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async () => {
    if (kind === "http503") {
      return {
        ok: false,
        status: 503,
        json: async () => ({ error: { code: "DRIVER_UNAVAILABLE", message: "驱动不可达" } }),
      };
    }
    if (kind === "network") throw new Error("network down");
    return {
      ok: true,
      json: async () => {
        throw new Error("invalid json");
      },
    };
  });
}

describe("PR-C A15：Browse 网络失败不得产生可选语义叶", () => {
  for (const kind of ["http503", "network", "unparseable"] as const) {
    it(`browse_${kind}_failure_never_offers_semantic_leaf`, async () => {
      installBrowseFailure(kind);
      const onFill = vi.fn();
      const onLeafSelect = vi.fn();
      render(
        <ResourceBrowseAntd
          endpointId="ep1"
          onFill={onFill}
          leafPolicy={createNckBrowseLeafPolicy({
            definitions: [collectDefinition()],
            endpointFamily: "828d",
          })}
          onLeafSelect={onLeafSelect}
        />,
      );
      if (kind === "unparseable") {
        // 响应体不可解析 ⇒ 当作空页处理（不猜节点），仍不得出现任何“选用”。
        await screen.findByText(/空（未知 parent 返回空页/);
      } else {
        await screen.findByText(/浏览不可用/);
      }
      expect(screen.queryByRole("button", { name: /选用（语义）/ })).toBeNull();
      expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
      expect(onFill).not.toHaveBeenCalled();
      expect(onLeafSelect).not.toHaveBeenCalled();
    });
  }
});

