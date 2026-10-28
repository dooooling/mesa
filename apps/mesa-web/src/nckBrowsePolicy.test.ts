// NCK Browse 叶策略单测（PR #150 解耦项）。
//
// 这些断言此前散落在通用组件/通用模型里；解耦后 NCK 语义集中到
// nckBrowsePolicy.ts，判定与文案必须逐条保留（A13/A16/A10/A22 不放宽）。
import { describe, expect, it } from "vitest";
import {
  createNckBrowseLeafPolicy,
  evaluateNckBrowseLeaf,
  type NckBrowseLeafSelection,
  type NckSemanticBrowseConfig,
} from "./nckBrowsePolicy";
import type { BrowseNode } from "./browseModel";
import type { NckDefinitionLike } from "./nckPointModel";

function definition(overrides: Partial<NckDefinitionLike> = {}): NckDefinitionLike {
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

function config(overrides: Partial<NckSemanticBrowseConfig> = {}): NckSemanticBrowseConfig {
  return {
    definitions: [definition()],
    endpointFamily: "828d",
    catalogFamily: null,
    ...overrides,
  };
}

function semanticNode(args: {
  kind?: string;
  includeIdentity?: boolean;
  preview?: string | null;
  parameters?: Record<string, unknown>;
} = {}): BrowseNode {
  const kind = args.kind ?? "nck_variable";
  return {
    id: "nck://presets/semantic",
    label: "通道程序状态",
    kind,
    has_children: false,
    binding_json: JSON.stringify({
      kind,
      parameters: args.parameters ?? {
        variable_id: "channel.program.status",
        definition_revision: 1,
        target: { channel: 1 },
        arguments: {},
      },
      ...(args.includeIdentity === false
        ? {}
        : {
            identity: {
              template: "channel.program.status@channel={channel}",
              keys: [{ path: "channel", required: true }],
              preview_point_key: args.preview === undefined ? "channel.program.status@channel=i:1" : args.preview,
            },
          }),
    }),
  };
}

describe("evaluateNckBrowseLeaf：语义叶路由与准入", () => {
  it("selectable_leaf_carries_opaque_payload_without_identity_in_parameters (A1)", () => {
    const verdict = evaluateNckBrowseLeaf(semanticNode(), config());
    expect(verdict.mode).toBe("owned");
    if (verdict.mode !== "owned") return;
    expect(verdict.selectable).toBe(true);
    expect(verdict.badge).toBe("语义点位");
    expect(verdict.selectLabel).toBe("选用（语义）");
    expect(verdict.preview?.text).toContain("Browse 预览：channel.program.status@channel=i:1");
    const sel = verdict.selection as NckBrowseLeafSelection;
    expect(sel.pointKind).toBe("nck_variable");
    expect(sel.resourceId).toBe("nck_variable");
    // identity 绝不折进 parameters（否则会被持久化，Core 整体拒绝）。
    expect(sel.parameters).toEqual({
      variable_id: "channel.program.status",
      definition_revision: 1,
      target: { channel: 1 },
      arguments: {},
    });
    expect(sel.parameters).not.toHaveProperty("identity");
    expect(sel.identity.preview_point_key).toBe("channel.program.status@channel=i:1");
  });

  it("leaf_without_identity_is_not_selectable (A16)", () => {
    for (const kind of ["nck_variable", "nck_native"]) {
      const verdict = evaluateNckBrowseLeaf(semanticNode({ kind, includeIdentity: false }), config());
      expect(verdict.mode).toBe("owned");
      if (verdict.mode !== "owned") return;
      expect(verdict.selectable).toBe(false);
      expect(verdict.reason).toContain("identity");
      expect(verdict.selection).toBeUndefined();
      expect(verdict.blockTestId).toBe("browse-semantic-block");
    }
  });

  it("malformed_identity_is_not_selectable", () => {
    const node: BrowseNode = {
      id: "x",
      kind: "nck_variable",
      binding_json: JSON.stringify({
        kind: "nck_variable",
        parameters: { variable_id: "v" },
        identity: "not-an-object",
      }),
    };
    const verdict = evaluateNckBrowseLeaf(node, config());
    expect(verdict.mode === "owned" && verdict.selectable).toBe(false);
  });

  it("null_preview_keeps_leaf_selectable_with_warning (A10)", () => {
    const verdict = evaluateNckBrowseLeaf(semanticNode({ preview: null }), config());
    if (verdict.mode !== "owned") throw new Error("expected owned");
    expect(verdict.selectable).toBe(true);
    expect(verdict.preview?.tone).toBe("warn");
    expect(verdict.preview?.text).toContain("身份未确定");
  });

  it("native_kind_is_never_selectable_from_semantic_browse", () => {
    const verdict = evaluateNckBrowseLeaf(semanticNode({ kind: "nck_native" }), config());
    if (verdict.mode !== "owned") throw new Error("expected owned");
    expect(verdict.selectable).toBe(false);
    expect(verdict.reason).toContain("高级原生配置");
    expect(verdict.tags).toEqual(["nck_native"]);
  });

  it("identity_scope_is_rejected_with_code (A13)", () => {
    const verdict = evaluateNckBrowseLeaf(
      semanticNode(),
      config({ definitions: [definition({ scope: "identity" })] }),
    );
    if (verdict.mode !== "owned") throw new Error("expected owned");
    expect(verdict.selectable).toBe(false);
    expect(verdict.reason).toContain("SCOPE_IDENTITY");
  });

  it("missing_definition_blocks_with_reason", () => {
    const verdict = evaluateNckBrowseLeaf(semanticNode(), config({ definitions: [] }));
    if (verdict.mode !== "owned") throw new Error("expected owned");
    expect(verdict.reason).toContain("找不到");
  });

  it("missing_definitions_catalog_fails_closed", () => {
    const verdict = evaluateNckBrowseLeaf(semanticNode(), config({ definitions: null }));
    if (verdict.mode !== "owned") throw new Error("expected owned");
    expect(verdict.selectable).toBe(false);
    expect(verdict.reason).toContain("定义目录不可用");
  });

  it("family_mismatch_blocks_with_code", () => {
    const verdict = evaluateNckBrowseLeaf(
      semanticNode(),
      config({ definitions: [definition({ family: "840d-sl" })] }),
    );
    if (verdict.mode !== "owned") throw new Error("expected owned");
    expect(verdict.reason).toContain("FAMILY_MISMATCH");
  });

  it("missing_variable_id_or_revision_blocks", () => {
    const noId = evaluateNckBrowseLeaf(semanticNode({ parameters: { definition_revision: 1 } }), config());
    if (noId.mode !== "owned") throw new Error("expected owned");
    expect(noId.reason).toContain("variable_id");

    const noRev = evaluateNckBrowseLeaf(semanticNode({ parameters: { variable_id: "x" } }), config());
    if (noRev.mode !== "owned") throw new Error("expected owned");
    expect(noRev.reason).toContain("definition_revision");
  });

  it("legacy_variable_envelope_is_blocked_not_generic (A22)", () => {
    const verdict = evaluateNckBrowseLeaf(
      {
        id: "legacy",
        kind: "variable",
        binding_json: JSON.stringify({
          resource_id: "variable",
          parameters: { area: "C", module: 21, column: 1, count: 1 },
        }),
      },
      config(),
    );
    if (verdict.mode !== "owned") throw new Error("expected owned");
    expect(verdict.selectable).toBe(false);
    expect(verdict.notice?.testId).toBe("browse-legacy-variable-block");
    expect(verdict.notice?.text).toContain("语义载荷不可用");
  });

  it("non_semantic_leaf_falls_through_to_generic (A44)", () => {
    expect(
      evaluateNckBrowseLeaf(
        {
          id: "g",
          kind: "register",
          binding_json: JSON.stringify({ resource_id: "register", parameters: { unit: 3 } }),
        },
        config(),
      ),
    ).toEqual({ mode: "generic" });
    // 分支节点（空信封）同样交回通用规则（通用规则会判为不可选）。
    expect(
      evaluateNckBrowseLeaf({ id: "b", kind: "group", has_children: true, binding_json: "{}" }, config()),
    ).toEqual({ mode: "generic" });
  });
});

describe("createNckBrowseLeafPolicy", () => {
  it("policy_declares_missing_handler_reason_and_delegates", () => {
    const policy = createNckBrowseLeafPolicy(config());
    expect(policy.name).toBe("nck-semantic");
    expect(policy.missingHandlerReason).toContain("未提供语义表单入口");
    const verdict = policy.evaluate(semanticNode());
    expect(verdict.mode).toBe("owned");
  });

  it("policy_is_stateless_across_nodes", () => {
    const policy = createNckBrowseLeafPolicy(config());
    expect(policy.evaluate(semanticNode({ kind: "nck_native" })).mode).toBe("owned");
    expect(policy.evaluate(semanticNode()).mode).toBe("owned");
  });
});
