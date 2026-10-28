// P0-2 Browse 回归：浏览树下钻 + 选用回填只依赖 wire 契约，不解释协议。
//
// IPC 1.8 后：可选性/载荷/阻断原因由**驱动**给出（`selection_mode` /
// `selection_payload_json` / `diagnostics`；见 browseModel.evaluateBrowseNode）。
// 组件只渲染状态与诊断、派发不透明载荷；语义层只注入 `BrowseSemanticHandler`。
//
// 本文件的用例分四组：
// ① 旧驱动（未声明选择契约）的通用资源叶行为逐字不变 —— 向后兼容回归；
// ② RESOURCE / SEMANTIC / NOT_SELECTABLE 三种驱动声明；
// ③ fail-closed：未知模式、载荷畸形、缺处理器、驱动拒绝、网络失败；
// ④ 解耦证明：虚构驱动的语义处理器同样生效（通用组件不认识任何真实厂商）。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ResourceBrowseAntd } from "./ResourceBrowseAntd";
import {
  createNckSemanticHandler,
  type NckBrowseLeafSelection,
} from "../nckBrowseSemantic";
import { BROWSE_SELECTION_MODE, type BrowseNode } from "../browseModel";
import type { BrowseSemanticHandler } from "../browseSemanticHandler";

const calls: Array<{ url: string; body: unknown }> = [];

// 旧驱动（IPC 1.7）形态：**没有** selection_mode / payload / diagnostics。
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

/** 驱动侧的 NCK 语义载荷（与 presets.rs 同形）。 */
const NCK_PAYLOAD = {
  kind: "nck_variable",
  parameters: {
    variable_id: "channel.program.status",
    definition_revision: 1,
    target: { channel: 1 },
    arguments: {},
  },
  identity: {
    template: "channel.program.status@channel={channel}",
    keys: [{ path: "channel", required: true }],
    preview_point_key: "channel.program.status@channel=i:1",
  },
};

/** 驱动声明可选的语义叶（mode=SEMANTIC + 载荷 + INFO 诊断）。 */
function semanticNode(args: {
  id?: string;
  payload?: unknown;
  diagnostics?: Array<{ code: string; message: string; severity?: number }>;
  mode?: number;
} = {}): Record<string, unknown> {
  return {
    id: args.id ?? "nck://presets/semantic",
    label: "通道程序状态",
    kind: "nck_variable",
    has_children: false,
    binding_json: JSON.stringify(NCK_PAYLOAD),
    selection_mode: args.mode ?? BROWSE_SELECTION_MODE.SEMANTIC,
    selection_payload_json: JSON.stringify(args.payload ?? NCK_PAYLOAD),
    diagnostics: args.diagnostics ?? [
      {
        code: "IDENTITY_PREVIEW",
        message: "Browse 预览：channel.program.status@channel=i:1（仅展示；提交前仍需服务端确认）",
        severity: 1,
      },
    ],
  };
}

describe("旧驱动（未声明选择契约）：通用资源叶行为不变", () => {
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

  it("unspecified_semantic_payload_is_blocked_not_guessed (fail-closed)", async () => {
    // 旧驱动 + 语义载荷：没有权威准入结论 ⇒ 不可选，且必须可见；
    // 绝不回落到通用资源回填（那会走旧 variable 入口）。
    installNodes([
      {
        id: "legacy.semantic",
        label: "旧语义叶",
        kind: "nck_variable",
        has_children: false,
        // 没有 selection_mode / payload / diagnostics
        binding_json: JSON.stringify(NCK_PAYLOAD),
      },
    ]);
    const onFill = vi.fn();
    const onSemanticSelect = vi.fn();
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={onFill}
        semanticHandler={createNckSemanticHandler()}
        onSemanticSelect={onSemanticSelect}
      />,
    );
    await screen.findByText("旧语义叶");
    const block = screen.getByTestId("browse-leaf-block").textContent ?? "";
    expect(block).toContain("BROWSE_SELECTION_UNSPECIFIED");
    expect(block).toContain("IPC < 1.8");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
    expect(onFill).not.toHaveBeenCalled();
    expect(onSemanticSelect).not.toHaveBeenCalled();
  });

  it("malformed_legacy_binding_is_visible_block", async () => {
    installNodes([
      { id: "bad", label: "坏载荷", kind: "node", has_children: false, binding_json: "{oops" },
    ]);
    render(<ResourceBrowseAntd endpointId="ep1" onFill={() => {}} />);
    await screen.findByText("坏载荷");
    expect(screen.getByTestId("browse-leaf-block").textContent).toContain(
      "BROWSE_PAYLOAD_MALFORMED",
    );
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
  });
});

describe("驱动声明 RESOURCE：通用资源叶由载荷直转", () => {
  it("resource_payload_fills_form_without_binding_json", async () => {
    const user = userEvent.setup();
    const onFill = vi.fn();
    installNodes([
      {
        id: "nsu=urn:mesa:fake:1;i=1",
        label: "Counter",
        kind: "node",
        has_children: false,
        // 刻意不给 binding_json：RESOURCE 模式的真值是 payload
        selection_mode: BROWSE_SELECTION_MODE.RESOURCE,
        selection_payload_json: JSON.stringify({
          resource_id: "node",
          parameters: { node_id: "nsu=urn:mesa:fake:1;i=1", data_type: "UINT32" },
        }),
      },
    ]);
    render(<ResourceBrowseAntd endpointId="ep1" onFill={onFill} />);
    await screen.findByText("Counter");
    await user.click(screen.getByRole("button", { name: /选\s*用/ }));
    expect(onFill).toHaveBeenCalledWith({
      resource_id: "node",
      parameters: { node_id: "nsu=urn:mesa:fake:1;i=1", data_type: "UINT32" },
    });
  });

  it("resource_payload_without_id_or_parameters_is_blocked", async () => {
    for (const payload of [{ parameters: {} }, { resource_id: "node" }, { resource_id: "  " }]) {
      installNodes([
        {
          id: "broken",
          label: "残缺资源叶",
          has_children: false,
          selection_mode: BROWSE_SELECTION_MODE.RESOURCE,
          selection_payload_json: JSON.stringify(payload),
        },
      ]);
      const { unmount } = render(<ResourceBrowseAntd endpointId="ep1" onFill={() => {}} />);
      await screen.findByText("残缺资源叶");
      expect(screen.getByTestId("browse-leaf-block").textContent).toContain(
        "BROWSE_RESOURCE_PAYLOAD_INVALID",
      );
      expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
      unmount();
    }
  });

  it("missing_or_malformed_payload_never_falls_back_to_binding_json", async () => {
    for (const raw of ["", "  ", "{oops", "[1,2]", '"text"']) {
      installNodes([
        {
          id: "x",
          label: "无载荷",
          has_children: false,
          // binding_json 合法，但驱动声明了 RESOURCE 却没给合法载荷 ⇒ 阻断
          binding_json: JSON.stringify({ resource_id: "node", parameters: { node_id: "n" } }),
          selection_mode: BROWSE_SELECTION_MODE.RESOURCE,
          selection_payload_json: raw,
        },
      ]);
      const { unmount } = render(<ResourceBrowseAntd endpointId="ep1" onFill={() => {}} />);
      await screen.findByText("无载荷");
      const text = screen.getByTestId("browse-leaf-block").textContent ?? "";
      expect(
        text.includes("BROWSE_PAYLOAD_MISSING") || text.includes("BROWSE_PAYLOAD_MALFORMED"),
        `payload=${raw} 必须可见阻断，实际：${text}`,
      ).toBe(true);
      expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
      unmount();
    }
  });
});

describe("驱动声明 SEMANTIC：载荷交给注入的语义处理器", () => {
  it("semantic_leaf_routes_to_semantic_form_without_using_browse_key", async () => {
    const user = userEvent.setup();
    const onFill = vi.fn();
    const onSemanticSelect = vi.fn();
    installNodes([semanticNode()]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={onFill}
        semanticHandler={createNckSemanticHandler()}
        onSemanticSelect={onSemanticSelect}
      />,
    );
    await screen.findByText("通道程序状态");
    // 驱动的 INFO 诊断只展示（不阻断）
    expect(screen.getByText(/Browse 预览/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /选用（语义）/ }));
    expect(onSemanticSelect).toHaveBeenCalledTimes(1);
    const sel = onSemanticSelect.mock.calls[0][0] as NckBrowseLeafSelection;
    expect(sel.pointKind).toBe("nck_variable");
    expect(sel.parameters).not.toHaveProperty("identity");
    expect(sel.identity.preview_point_key).toContain("channel=i:1");
    // 语义叶绝不回落到通用 resource 回填（那会走旧变量入口）。
    expect(onFill).not.toHaveBeenCalled();
  });

  it("warning_diagnostic_keeps_leaf_selectable (A10)", async () => {
    const user = userEvent.setup();
    const onSemanticSelect = vi.fn();
    installNodes([
      semanticNode({
        diagnostics: [
          {
            code: "IDENTITY_UNRESOLVED",
            message: "Browse 预览：身份未确定（target/arguments 不完整）",
            severity: 2,
          },
        ],
      }),
    ]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        semanticHandler={createNckSemanticHandler()}
        onSemanticSelect={onSemanticSelect}
      />,
    );
    await screen.findByText("通道程序状态");
    expect(screen.getByText(/身份未确定/)).toBeTruthy();
    // 警告只影响展示，不禁止进入语义表单补全。
    await user.click(screen.getByRole("button", { name: /选用（语义）/ }));
    expect(onSemanticSelect).toHaveBeenCalledTimes(1);
  });

  it("driver_not_selectable_with_error_diagnostic_blocks_visibly (A13)", async () => {
    installNodes([
      semanticNode({
        mode: BROWSE_SELECTION_MODE.NOT_SELECTABLE,
        diagnostics: [
          {
            code: "DEFINITION_SCOPE_NOT_COLLECTABLE",
            message: "定义 `channel.program.status` revision=1 的 scope=`identity` 不可采集",
            severity: 3,
          },
        ],
      }),
    ]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        semanticHandler={createNckSemanticHandler()}
        onSemanticSelect={() => {}}
      />,
    );
    await screen.findByText("通道程序状态");
    const block = screen.getByTestId("browse-selection-block").textContent ?? "";
    expect(block).toContain("DEFINITION_SCOPE_NOT_COLLECTABLE");
    expect(block).toContain("scope=");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
  });

  it("not_selectable_without_reason_renders_no_block (分支/诊断节点)", async () => {
    installNodes([
      {
        id: "nck://presets",
        label: "常用预设",
        kind: "folder",
        has_children: true,
        selection_mode: BROWSE_SELECTION_MODE.NOT_SELECTABLE,
        diagnostics: [],
      },
    ]);
    render(<ResourceBrowseAntd endpointId="ep1" onFill={() => {}} />);
    await screen.findByText("常用预设");
    expect(screen.queryByTestId("browse-selection-block")).toBeNull();
    expect(screen.queryByTestId("browse-leaf-block")).toBeNull();
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
    // 分支仍可下钻
    expect(screen.getByRole("button", { name: /进\s*入/ })).toBeTruthy();
  });

  it("semantic_payload_without_handler_is_closed_not_fallback", async () => {
    installNodes([semanticNode()]);
    const onFill = vi.fn();
    render(<ResourceBrowseAntd endpointId="ep1" onFill={onFill} />);
    await screen.findByText("通道程序状态");
    expect(screen.getByTestId("browse-leaf-block").textContent).toContain(
      "BROWSE_SEMANTIC_HANDLER_MISSING",
    );
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
    expect(onFill).not.toHaveBeenCalled();
  });

  it("semantic_leaf_without_semantic_entry_is_closed_with_handler_reason", async () => {
    installNodes([semanticNode()]);
    const onFill = vi.fn();
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={onFill}
        semanticHandler={createNckSemanticHandler()}
        // 刻意不提供 onSemanticSelect
      />,
    );
    await screen.findByText("通道程序状态");
    const block = screen.getByTestId("browse-leaf-block").textContent ?? "";
    expect(block).toContain("BROWSE_SEMANTIC_HANDLER_MISSING");
    expect(block).toContain("未提供语义表单入口");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
    expect(onFill).not.toHaveBeenCalled();
  });

  it("handler_rejection_blocks_with_handler_code", async () => {
    // 驱动乐观地声明 SEMANTIC，但载荷形状不合 NCK 契约（native）⇒ 处理器拒绝。
    installNodes([semanticNode({ payload: { ...NCK_PAYLOAD, kind: "nck_native" } })]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={() => {}}
        semanticHandler={createNckSemanticHandler()}
        onSemanticSelect={() => {}}
      />,
    );
    await screen.findByText("通道程序状态");
    const block = screen.getByTestId("browse-semantic-block").textContent ?? "";
    expect(block).toContain("NCK_PAYLOAD_NATIVE_NOT_FROM_BROWSE");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
  });

  it("unknown_selection_mode_is_blocked", async () => {
    installNodes([
      {
        id: "future",
        label: "未来驱动叶",
        has_children: false,
        selection_mode: 99,
        selection_payload_json: JSON.stringify({ anything: true }),
      },
    ]);
    render(<ResourceBrowseAntd endpointId="ep1" onFill={() => {}} />);
    await screen.findByText("未来驱动叶");
    const block = screen.getByTestId("browse-leaf-block").textContent ?? "";
    expect(block).toContain("BROWSE_SELECTION_MODE_UNKNOWN");
    expect(block).toContain("99");
    expect(screen.queryByRole("button", { name: /选\s*用/ })).toBeNull();
  });
});

// 解耦验证：通用组件对**虚构**驱动语义同样生效（不认识任何真实厂商字面量）。
describe("Browse 语义处理器缝（不含任何厂商语义）", () => {
  it("fake_vendor_handler_drives_labels_and_payload", async () => {
    const user = userEvent.setup();
    const onFill = vi.fn();
    const onSemanticSelect = vi.fn();
    const widgetHandler: BrowseSemanticHandler<{ lamp: number }> = {
      name: "widget",
      notProvidedReason: "缺少部件表单入口：不可选用",
      selectLabel: "选用（部件）",
      parse: (payload) =>
        payload.kind === "widget_lamp"
          ? { ok: true, selection: { lamp: Number(payload.lamp) } }
          : { ok: false, code: "WIDGET_UNEXPECTED_KIND", message: "不是部件载荷" },
    };
    const node = (id: string, label: string, lamp: number): BrowseNode => ({
      id,
      label,
      has_children: false,
      selection_mode: BROWSE_SELECTION_MODE.SEMANTIC,
      selection_payload_json: JSON.stringify({ kind: "widget_lamp", lamp }),
    });
    installNodes([node("widget://lamp", "指示灯", 3), node("widget://locked", "被锁定的部件", 9)]);
    render(
      <ResourceBrowseAntd
        endpointId="ep1"
        onFill={onFill}
        semanticHandler={widgetHandler}
        onSemanticSelect={onSemanticSelect}
      />,
    );
    await screen.findByText("指示灯");
    const lampRow = screen.getByText("指示灯").closest("div") as HTMLElement;
    await user.click(within(lampRow).getByRole("button", { name: /选用（部件）/ }));
    expect(onSemanticSelect).toHaveBeenCalledWith({ lamp: 3 });
    expect(onFill).not.toHaveBeenCalled();
    // 两个节点都给了入口文案按钮；具体业务"锁定"由处理器/驱动表达，
    // 通用组件不解释 lamp===9 这种厂商语义（这里两枚按钮即证明它不管）。
    expect(screen.getAllByRole("button", { name: /选用（部件）/ })).toHaveLength(2);
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
      const onSemanticSelect = vi.fn();
      render(
        <ResourceBrowseAntd
          endpointId="ep1"
          onFill={onFill}
          semanticHandler={createNckSemanticHandler()}
          onSemanticSelect={onSemanticSelect}
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
      expect(onSemanticSelect).not.toHaveBeenCalled();
    });
  }
});
