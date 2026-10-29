// 对象探测 / 填参依据 的纯模型单测。
//
// 重点钉三件事：
// 1. 需求与填参**只来自数据**（定义 targets / wire identity.keys / 驱动载荷），
//    前端不从名称、ID 前缀或标签推断；
// 2. 失败与空页严格区分（失败是异常，不是"没有对象"）；
// 3. 没有可靠地址映射时**拒绝填参**，绝不生成"貌似完整"的地址。
import { describe, expect, it, vi } from "vitest";
import type { BrowseNode } from "./browseModel";
import {
  buildObjectProbeRequest,
  canExpandNode,
  deriveObjectFill,
  fetchObjectProbePage,
  isObjectProbeError,
  objectNeedsFromIdentityKeys,
  objectNeedsOf,
  objectProbeGate,
  resolveObjectNeeds,
} from "./objectProbe";

const DEFINITION = {
  variable_id: "axis.position.actual_bcs",
  revision: 1,
  targets: ["axis", "channel"],
  target_required: ["axis", "channel"],
  identity_keys: [{ path: "axis", required: true }, { path: "channel", required: true }],
};

/** 语义叶节点（形状取自真机抓包：`selection_mode=2` + 载荷 + 诊断）。 */
function semanticNode(payload: Record<string, unknown>, overrides: Partial<BrowseNode> = {}): BrowseNode {
  return {
    id: "nck://identity/axis/1",
    label: "MX1 · 轴1",
    kind: "nck_variable",
    has_children: false,
    binding_json: "{}",
    selection_mode: 2,
    selection_payload_json: JSON.stringify(payload),
    diagnostics: [],
    ...overrides,
  };
}

function payload(args: {
  variable_id?: string;
  definition_revision?: number;
  target?: Record<string, unknown>;
  arguments?: Record<string, unknown>;
}) {
  return {
    kind: "nck_variable",
    parameters: {
      variable_id: args.variable_id ?? "axis.position.actual_bcs",
      definition_revision: args.definition_revision ?? 1,
      target: args.target ?? {},
      arguments: args.arguments ?? {},
    },
    identity: { template: "axis.position.actual_bcs@axis=i:{axis}", keys: [], preview_point_key: null },
  };
}

describe("对象需求：只读数据", () => {
  it("needs_come_from_definition_targets", () => {
    expect(objectNeedsOf(DEFINITION)).toEqual([
      { kind: "axis", required: true, source: "definition_targets" },
      { kind: "channel", required: true, source: "definition_targets" },
    ]);
    // 没有 targets ⇒ 不需要选对象 ⇒ UI 不出现探测入口（按需出现）。
    expect(objectNeedsOf({ targets: [], target_required: [] })).toEqual([]);
    expect(objectNeedsOf(null)).toEqual([]);
    // 定义把某个变体标成非必填时，required 跟随数据而不是硬编码。
    expect(
      objectNeedsOf({ targets: ["drive_object"], target_required: [] })[0],
    ).toEqual({ kind: "drive_object", required: false, source: "definition_targets" });
  });

  it("wire_identity_keys_win_over_definition_targets", () => {
    const needs = resolveObjectNeeds({
      definition: DEFINITION,
      identityKeys: [{ path: "mode_group", required: true }],
    });
    expect(needs[0]).toEqual({
      kind: "mode_group",
      required: true,
      source: "browse_identity_keys",
    });
    // 定义里多出来的变体仍然补足（两处都是数据，取并集）。
    expect(needs.map((need) => need.kind)).toEqual(["mode_group", "axis", "channel"]);
    expect(objectNeedsFromIdentityKeys([{ path: "" }, { path: "index" }])).toEqual([
      { kind: "index", required: false, source: "browse_identity_keys" },
    ]);
  });
});

describe("探测入口：端点必须已创建且连接可读", () => {
  it("gate_requires_endpoint_and_connection", () => {
    const noId = objectProbeGate({ endpointId: "", endpointKnown: true, connectionKnown: true });
    expect(noId.available).toBe(false);
    if (!noId.available) {
      expect(noId.code).toBe("ENDPOINT_MISSING");
      expect(noId.reason).toContain("需先创建并连接端点");
    }
    const unknownEndpoint = objectProbeGate({
      endpointId: "ep",
      endpointKnown: false,
      connectionKnown: true,
    });
    expect(unknownEndpoint.available).toBe(false);
    if (!unknownEndpoint.available) {
      expect(unknownEndpoint.code).toBe("ENDPOINT_NOT_CONNECTED");
      expect(unknownEndpoint.reason).toContain("不会发起在线探测");
    }
    const noConnection = objectProbeGate({
      endpointId: "ep",
      endpointKnown: true,
      connectionKnown: false,
    });
    expect(noConnection.available).toBe(false);
    expect(
      objectProbeGate({ endpointId: "ep", endpointKnown: true, connectionKnown: true }),
    ).toEqual({ available: true });
  });
});

describe("探测请求：字段与失败语义", () => {
  it("request_uses_parent_field", () => {
    // 真机实测：分页/下钻字段是 `parent`（写错会静默返回根节点）。
    expect(buildObjectProbeRequest({ parent: "nck://identity/axes" })).toEqual({
      parent: "nck://identity/axes",
      filter: "",
      cursor: "",
      limit: 50,
    });
  });

  it("page_request_posts_parent_and_returns_nodes", async () => {
    const calls: Array<{ url: string; body: unknown }> = [];
    const page = await fetchObjectProbePage(
      { endpointId: "ep-1", parent: "nck://identity/axes" },
      async (url, init) => {
        calls.push({ url, body: JSON.parse(String(init?.body)) });
        return {
          ok: true,
          status: 200,
          json: async () => ({
            nodes: [{ id: "nck://identity/axis/1", label: "MX1", has_children: true }],
            next_cursor: "9",
          }),
        };
      },
    );
    expect(calls[0]!.url).toBe("/api/v1/endpoints/ep-1/browse");
    expect((calls[0]!.body as { parent: string }).parent).toBe("nck://identity/axes");
    expect(page.nodes).toHaveLength(1);
    expect(page.nextCursor).toBe("9");
  });

  it("failure_is_an_error_not_an_empty_page", async () => {
    const error = await fetchObjectProbePage({ endpointId: "ep-1" }, async () => ({
      ok: false,
      status: 503,
      json: async () => ({ error: { code: "BROWSE_FAILED", message: "驱动不可用" } }),
    })).catch((thrown: unknown) => thrown);
    expect(isObjectProbeError(error)).toBe(true);
    if (!isObjectProbeError(error)) return;
    expect(error.code).toBe("PROBE_REQUEST_FAILED");
    expect(error.status).toBe(503);
    expect(error.message).toContain("BROWSE_FAILED");
    expect(error.message).toContain("驱动不可用");
  });

  it("network_failure_and_malformed_response_are_errors", async () => {
    const network = await fetchObjectProbePage({ endpointId: "ep-1" }, async () => {
      throw new Error("boom");
    }).catch((error: unknown) => error);
    expect(isObjectProbeError(network) && network.code).toBe("PROBE_NETWORK");

    const malformed = await fetchObjectProbePage({ endpointId: "ep-1" }, async () => ({
      ok: true,
      status: 200,
      json: async () => ({ nodes: "not-an-array" }),
    })).catch((error: unknown) => error);
    expect(isObjectProbeError(malformed) && malformed.code).toBe("PROBE_RESPONSE_INVALID");
  });

  it("empty_page_is_a_result_not_a_failure", async () => {
    const page = await fetchObjectProbePage({ endpointId: "ep-1" }, async () => ({
      ok: true,
      status: 200,
      json: async () => ({ nodes: [], next_cursor: null }),
    }));
    expect(page.nodes).toEqual([]);
    expect(page.nextCursor).toBeNull();
  });

  it("missing_endpoint_never_probes", async () => {
    const fetchImpl = vi.fn();
    await expect(
      fetchObjectProbePage({ endpointId: "" }, fetchImpl as never),
    ).rejects.toSatisfy((error: unknown) => isObjectProbeError(error));
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});

describe("填参：只来自驱动载荷", () => {
  it("fills_only_keys_present_in_payload", () => {
    const result = deriveObjectFill({
      node: semanticNode(payload({ target: { axis: 3, channel: 1 } })),
      definition: DEFINITION,
    });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.fill.target).toEqual({ axis: "3", channel: "1" });
    expect(result.fill.arguments).toEqual({});
    expect(result.fill.unresolved).toEqual([]);
    expect(result.fill.basis).toContain("parameters.target");
  });

  it("never_invents_missing_keys_and_reports_them", () => {
    const result = deriveObjectFill({
      node: semanticNode(payload({ target: { axis: 3 } })),
      definition: DEFINITION,
    });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    // channel 缺失 ⇒ 只填 axis，并且明确列为"仍需手动填写"（不填默认 1）。
    expect(result.fill.target).toEqual({ axis: "3" });
    expect(result.fill.unresolved).toEqual(["channel"]);
  });

  it("drive_object_envelope_keeps_number_and_entry", () => {
    const definition = {
      variable_id: "drive.object.name",
      revision: 1,
      targets: ["drive_object"],
      target_required: ["drive_object"],
    };
    const result = deriveObjectFill({
      node: semanticNode(
        payload({
          variable_id: "drive.object.name",
          target: { drive_object: { number: 4, entry: "drive_object" } },
        }),
      ),
      definition,
    });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.fill.target).toEqual({ drive_object: "4", "drive_object.entry": "drive_object" });
    expect(result.fill.unresolved).toEqual([]);
  });

  it("refuses_not_selectable_node", () => {
    const node = semanticNode(payload({ target: { axis: 3 } }), {
      selection_mode: 3,
      diagnostics: [
        { code: "DEFINITION_SCOPE_NOT_COLLECTABLE", message: "scope=identity", severity: 3 },
      ],
    });
    const result = deriveObjectFill({ node, definition: DEFINITION });
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.code).toBe("OBJECT_NODE_NOT_SELECTABLE");
    expect(result.message).toContain("DEFINITION_SCOPE_NOT_COLLECTABLE");
  });

  it("refuses_when_payload_has_no_address_mapping", () => {
    // 真机上"待确认 area_no/line"的模板就是这种形态：可选，但 target 为空。
    const result = deriveObjectFill({
      node: semanticNode(payload({ target: {} })),
      definition: DEFINITION,
    });
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.code).toBe("OBJECT_NO_ADDRESS_MAPPING");
    expect(result.message).toContain("不可自动填参");
  });

  it("refuses_payload_without_selection_block", () => {
    const node = semanticNode({}, { selection_payload_json: "" });
    const result = deriveObjectFill({ node, definition: DEFINITION });
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.code).toBe("OBJECT_PAYLOAD_MISSING");
  });

  it("refuses_object_belonging_to_another_variable_or_revision", () => {
    const other = deriveObjectFill({
      node: semanticNode(payload({ variable_id: "channel.feed.actual", target: { axis: 1 } })),
      definition: DEFINITION,
    });
    expect(!other.ok && other.code).toBe("OBJECT_VARIABLE_MISMATCH");

    const staleRevision = deriveObjectFill({
      node: semanticNode(payload({ definition_revision: 7, target: { axis: 1 } })),
      definition: DEFINITION,
    });
    expect(!staleRevision.ok && staleRevision.code).toBe("OBJECT_REVISION_MISMATCH");
  });

  it("refuses_undeclared_target_variant", () => {
    const result = deriveObjectFill({
      node: semanticNode(payload({ target: { spindle: 1 } })),
      definition: DEFINITION,
    });
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.code).toBe("OBJECT_TARGET_UNDECLARED");
  });

  it("arguments_are_filled_from_payload_only", () => {
    const definition = {
      variable_id: "channel.r_parameter",
      revision: 1,
      targets: ["channel"],
      target_required: ["channel"],
    };
    const result = deriveObjectFill({
      node: semanticNode(
        payload({
          variable_id: "channel.r_parameter",
          target: { channel: 1 },
          arguments: { index: 5 },
        }),
      ),
      definition,
    });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.fill.arguments).toEqual({ index: "5" });
  });
});

describe("节点展开：只看 wire 的 has_children", () => {
  it("unconfirmed_node_is_not_expandable", () => {
    // 真机轴 6：`has_children=false` + 标签写明"未确认对象"。
    const node: BrowseNode = {
      id: "nck://identity/axis/6",
      label: "MB1 · 轴6 → 未确认有效驱动输出 → 未确认对象",
      kind: "folder",
      has_children: false,
      selection_mode: 3,
    };
    expect(canExpandNode(node)).toBe(false);
    expect(canExpandNode({ ...node, has_children: true })).toBe(true);
    expect(canExpandNode({ ...node, has_children: undefined })).toBe(false);
  });
});
