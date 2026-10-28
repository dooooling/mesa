// Browse → Selection 映射单测：锁定信封形态，不解释协议语义。
//
// PR #150 解耦后：通用层不认识任何 point kind。带 `kind` 的信封（语义载荷）
// 通用层一律不猜（返回 null），identity 之类的语义块由 `parseBrowseEnvelope`
// 原样透出给语义层（见 nckBrowseSemantic.test.ts）。
import { describe, expect, it } from "vitest";
import {
  buildBrowseRequest,
  declaresOpaquePointKind,
  parseBrowseEnvelope,
  selectableFromNode,
} from "./browseModel";
import { missingRequiredParams } from "./resourceSelectionModel";
import type { FieldDescriptor } from "./types";

describe("selectableFromNode", () => {
  it("semantic_payload_is_not_generically_selectable_and_envelope_is_opaque (A1)", () => {
    const node = {
      id: "nck://presets/828d.C.S.progStatus.nck_variable",
      kind: "nck_variable",
      has_children: false,
      binding_json: JSON.stringify({
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
      }),
    };
    // 通用层不解释 point kind：不产出 selection（也就不会给出「选用」）。
    expect(selectableFromNode(node)).toBeNull();
    const envelope = parseBrowseEnvelope(node)!;
    expect(declaresOpaquePointKind(envelope)).toBe(true);
    // 信封原样透出（策略层据此判定），parameters 仍是 Core 契约的 4 键。
    expect(envelope.identity).toEqual({
      template: "channel.program.status@channel={channel}",
      keys: [{ path: "channel", required: true }],
      preview_point_key: "channel.program.status@channel=i:1",
    });
    expect(envelope.parameters).toEqual({
      variable_id: "channel.program.status",
      definition_revision: 1,
      target: { channel: 1 },
      arguments: {},
    });
  });

  it("leaf_without_identity_is_not_generically_selectable (A16)", () => {
    for (const kind of ["nck_variable", "nck_native"]) {
      for (const envelope of [
        { parameters: { variable_id: "channel.program.status" } },
        { parameters: {}, identity: "not-an-object" },
      ]) {
        const node = {
          id: "x",
          kind,
          binding_json: JSON.stringify({ kind, ...envelope }),
        };
        // 无论 identity 是否合法，通用层都不得把它当资源叶（交给策略层判定）。
        expect(selectableFromNode(node)).toBeNull();
      }
    }
  });

  it("semantic_leaf_with_null_preview_keeps_null_in_envelope (A10)", () => {
    // A10 的“预览为空”不是“无 identity”：信封如实透出 null，由策略层显示原因。
    const envelope = parseBrowseEnvelope({
      id: "x",
      kind: "nck_variable",
      binding_json: JSON.stringify({
        kind: "nck_variable",
        parameters: { variable_id: "v" },
        identity: { template: "v@channel={channel}", keys: [], preview_point_key: null },
      }),
    })!;
    expect((envelope.identity as { preview_point_key?: unknown }).preview_point_key).toBeNull();
  });

  it("generic leaves keep the exact two-field shape (A44)", () => {
    const generic = selectableFromNode({
      id: "g",
      kind: "register",
      binding_json: JSON.stringify({ resource_id: "register", parameters: { unit: 3 } }),
    });
    expect(generic).toEqual({ resource_id: "register", parameters: { unit: 3 } });

    const bare = selectableFromNode({
      id: "n",
      kind: "node",
      binding_json: JSON.stringify({ node_id: "x", data_type: "STRING" }),
    });
    expect(bare).toEqual({ resource_id: "node", parameters: { node_id: "x", data_type: "STRING" } });
  });

  it("NCK 模板叶形态：显式 resource_id + 原生字段（不含实例/行）", () => {
    // 预填模板只带结构固定字段；area_no/line 必须由使用者确认。
    const sel = selectableFromNode({
      id: "nck://presets/828d.V.M.r0080.nc_variable",
      kind: "variable",
      has_children: false,
      binding_json: JSON.stringify({
        resource_id: "variable",
        parameters: {
          area: "V",
          module: 26,
          column: 80,
          count: 1,
          unit_mode: "current",
          decode: "F32_BE",
          transport_size: "0x07",
        },
      }),
    });
    expect(sel?.resource_id).toBe("variable");
    expect(sel?.parameters).toMatchObject({ area: "V", module: 26, column: 80 });
    expect(sel?.parameters).not.toHaveProperty("area_no");
    expect(sel?.parameters).not.toHaveProperty("line");
    expect(sel?.parameters).not.toHaveProperty("access_path");
  });

  it("OPC UA 形态：裸参数 + kind 回落为 resource", () => {
    const sel = selectableFromNode({
      id: "nsu=urn:mesa:fake:1;i=1",
      kind: "node",
      has_children: true,
      binding_json: JSON.stringify({ node_id: "nsu=urn:mesa:fake:1;i=1", data_type: "STRING" }),
    });
    expect(sel).toEqual({
      resource_id: "node",
      parameters: { node_id: "nsu=urn:mesa:fake:1;i=1", data_type: "STRING" },
    });
  });

  it("NCK 分支形态 binding_json='{}' 永远不可选（模板/身份目录）", () => {
    // 分支节点（nck://presets、nck://identity/*）binding_json 不是空串而是 "{}"；
    // 若回落到 kind 会把分支误判成可选资源（P0-2 回归）。
    for (const kind of ["folder", "diagnostic"]) {
      expect(
        selectableFromNode({ id: "nck://presets", kind, has_children: true, binding_json: "{}" }),
      ).toBeNull();
    }
    expect(
      selectableFromNode({
        id: "nck://identity",
        kind: "folder",
        has_children: true,
        binding_json: " { } ",
      }),
    ).toBeNull();
  });

  it("无 binding 的分支节点不可选用（只能下钻）", () => {
    expect(selectableFromNode({ id: "nck://presets", kind: "group", has_children: true })).toBeNull();
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: "  " })).toBeNull();
  });

  it("非法 JSON / 非对象 binding 不可用", () => {
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: "{oops" })).toBeNull();
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: "[1,2]" })).toBeNull();
    expect(selectableFromNode({ id: "x", kind: "node", binding_json: '"str"' })).toBeNull();
  });

  it("无 resource_id 又无 kind 时不可用", () => {
    expect(selectableFromNode({ id: "x", binding_json: JSON.stringify({ a: 1 }) })).toBeNull();
  });
});

describe("buildBrowseRequest", () => {
  it("缺省 parent/filter/cursor/limit", () => {
    expect(buildBrowseRequest({})).toEqual({ parent: "", filter: "", cursor: "", limit: 50 });
  });
});

// V2.0 §4.3 Gate：不完整模板选用后**不得**绕过必填校验。
// 原生 variable 资源的实例/行等都是必填，模板只预填结构固定字段。
describe("不完整模板不绕过必填前置", () => {
  const nckFields: FieldDescriptor[] = [
    "area",
    "area_no",
    "module",
    "column",
    "line",
    "count",
    "unit_mode",
    "decode",
    "transport_size",
  ].map((key) => ({
    key,
    label: key,
    field_type: "integer",
    required: true,
    validation: {},
    ui: {},
  }));

  it("模板叶只带固定字段 → area_no/line 仍被 required 前置拦下", () => {
    const sel = selectableFromNode({
      id: "nck://presets/828d.V.M.r0080.nc_variable",
      kind: "variable",
      binding_json: JSON.stringify({
        resource_id: "variable",
        parameters: {
          area: "V",
          module: 26,
          column: 80,
          count: 1,
          unit_mode: "current",
          decode: "F32_BE",
          transport_size: "0x07",
        },
      }),
    })!;
    // 通用 required 前置（唯一允许的前端 required 判断）必须报出缺失项。
    expect(missingRequiredParams({ fields: nckFields }, sel.parameters)).toEqual([
      "area_no",
      "line",
    ]);
    // 使用者补齐后才放行。
    expect(
      missingRequiredParams({ fields: nckFields }, { ...sel.parameters, area_no: 6, line: 1 }),
    ).toEqual([]);
  });
});

// IPC 1.8：wire 选择契约 → 「可选 / 阻断 / 无入口」三态。
//
// 这一组锁的是**消费侧**规则（不涉及任何厂商语义）：
// - 驱动声明的模式是唯一可选性真值；
// - 未知模式 / 载荷畸形 / 旧驱动的语义载荷一律可见阻断；
// - 旧驱动（未声明）的通用资源浏览行为逐字不变。
import {
  BROWSE_CONTRACT_CODES,
  BROWSE_SELECTION_MODE,
  decodeBrowseSelectionMode,
  evaluateBrowseNode,
} from "./browseModel";

describe("decodeBrowseSelectionMode", () => {
  it("maps_known_values_and_keeps_unknown_distinct", () => {
    expect(decodeBrowseSelectionMode({ id: "x" })).toBe("unspecified");
    expect(decodeBrowseSelectionMode({ id: "x", selection_mode: 0 })).toBe("unspecified");
    expect(decodeBrowseSelectionMode({ id: "x", selection_mode: 1 })).toBe("resource");
    expect(decodeBrowseSelectionMode({ id: "x", selection_mode: 2 })).toBe("semantic");
    expect(decodeBrowseSelectionMode({ id: "x", selection_mode: 3 })).toBe("not_selectable");
    // 未知值**不得**被当成 unspecified（那会把未来语义当旧协议放行）。
    expect(decodeBrowseSelectionMode({ id: "x", selection_mode: 4 })).toBe("unknown");
    expect(decodeBrowseSelectionMode({ id: "x", selection_mode: 99 })).toBe("unknown");
    expect(decodeBrowseSelectionMode({ id: "x", selection_mode: -1 })).toBe("unknown");
  });
});

describe("evaluateBrowseNode：驱动声明的三种模式", () => {
  it("resource_mode_yields_selection_from_payload", () => {
    const v = evaluateBrowseNode({
      id: "n1",
      selection_mode: BROWSE_SELECTION_MODE.RESOURCE,
      selection_payload_json: JSON.stringify({
        resource_id: "node",
        parameters: { node_id: "ns=1;i=1" },
      }),
    });
    expect(v.mode).toBe("resource");
    expect(v.resource).toEqual({ resource_id: "node", parameters: { node_id: "ns=1;i=1" } });
    expect(v.semanticPayload).toBeNull();
    expect(v.blocks).toHaveLength(0);
  });

  it("semantic_mode_exposes_opaque_payload_untouched", () => {
    const payload = { kind: "vendor_thing", nested: { a: 1 } };
    const v = evaluateBrowseNode({
      id: "n2",
      selection_mode: BROWSE_SELECTION_MODE.SEMANTIC,
      selection_payload_json: JSON.stringify(payload),
    });
    expect(v.semanticPayload).toEqual(payload);
    expect(v.resource).toBeNull();
  });

  it("not_selectable_never_yields_entry_even_with_valid_payload", () => {
    const v = evaluateBrowseNode({
      id: "n3",
      selection_mode: BROWSE_SELECTION_MODE.NOT_SELECTABLE,
      selection_payload_json: JSON.stringify({ resource_id: "node", parameters: {} }),
    });
    expect(v.resource).toBeNull();
    expect(v.semanticPayload).toBeNull();
    // 无理由的不可选 ⇒ 无阻断块（分支节点不该出现红色提示）。
    expect(v.blocks).toHaveLength(0);
  });

  it("driver_error_diagnostics_become_visible_blocks_and_others_notes", () => {
    const v = evaluateBrowseNode({
      id: "n4",
      selection_mode: BROWSE_SELECTION_MODE.NOT_SELECTABLE,
      diagnostics: [
        { code: "SOME_DRIVER_CODE", message: "驱动说不行", severity: 3 },
        { code: "INFO_CODE", message: "提示", severity: 1 },
        { code: "WARN_CODE", message: "注意", severity: 2 },
      ],
    });
    expect(v.blocks.map((b) => b.code)).toEqual(["SOME_DRIVER_CODE"]);
    expect(v.blocks[0].testId).toBe("browse-selection-block");
    expect(v.notes.map((n) => [n.code, n.tone])).toEqual([
      ["INFO_CODE", "muted"],
      ["WARN_CODE", "warn"],
    ]);
  });

  it("unknown_severity_is_treated_as_blocking_style_but_grants_nothing", () => {
    const v = evaluateBrowseNode({
      id: "n5",
      selection_mode: BROWSE_SELECTION_MODE.SEMANTIC,
      selection_payload_json: "{}",
      diagnostics: [{ code: "X", message: "未知级别", severity: 42 }],
    });
    expect(v.blocks.map((b) => b.code)).toEqual(["X"]);
    // 阻断样式只影响展示；载荷仍可交付（可选性由 mode 决定）。
    expect(v.semanticPayload).toEqual({});
  });
});

describe("evaluateBrowseNode：fail-closed 契约违规", () => {
  it("unknown_mode_blocks_with_code_and_value", () => {
    const v = evaluateBrowseNode({ id: "x", selection_mode: 77 });
    expect(v.blocks[0].code).toBe(BROWSE_CONTRACT_CODES.MODE_UNKNOWN);
    expect(v.blocks[0].message).toContain("77");
    expect(v.resource).toBeNull();
    expect(v.semanticPayload).toBeNull();
  });

  it("declared_mode_without_payload_is_blocked", () => {
    for (const mode of [BROWSE_SELECTION_MODE.RESOURCE, BROWSE_SELECTION_MODE.SEMANTIC]) {
      const v = evaluateBrowseNode({ id: "x", selection_mode: mode });
      expect(v.blocks[0].code).toBe(BROWSE_CONTRACT_CODES.PAYLOAD_MISSING);
    }
  });

  it("malformed_payload_is_blocked_and_never_falls_back", () => {
    for (const raw of ["{oops", "[1]", '"s"', "42"]) {
      const v = evaluateBrowseNode({
        id: "x",
        selection_mode: BROWSE_SELECTION_MODE.SEMANTIC,
        selection_payload_json: raw,
        // binding_json 合法也不得被当作回退来源
        binding_json: JSON.stringify({ kind: "vendor", parameters: {} }),
      });
      expect(v.blocks[0].code).toBe(BROWSE_CONTRACT_CODES.PAYLOAD_MALFORMED);
      expect(v.semanticPayload).toBeNull();
    }
  });

  it("resource_payload_must_carry_resource_id_and_parameters", () => {
    const bad = [
      { parameters: {} },
      { resource_id: "node" },
      { resource_id: "  ", parameters: {} },
      { resource_id: "node", parameters: [] },
    ];
    for (const p of bad) {
      const v = evaluateBrowseNode({
        id: "x",
        selection_mode: BROWSE_SELECTION_MODE.RESOURCE,
        selection_payload_json: JSON.stringify(p),
        kind: "node", // 即便节点自身 kind 像资源，也不得据此推测
      });
      expect(v.blocks[0].code).toBe(BROWSE_CONTRACT_CODES.RESOURCE_PAYLOAD_INVALID);
      expect(v.resource).toBeNull();
    }
  });

  it("legacy_semantic_payload_without_authority_is_blocked", () => {
    const v = evaluateBrowseNode({
      id: "x",
      kind: "vendor_point",
      binding_json: JSON.stringify({ kind: "vendor_point", parameters: { a: 1 } }),
    });
    expect(v.blocks[0].code).toBe(BROWSE_CONTRACT_CODES.SELECTION_UNSPECIFIED);
    expect(v.resource).toBeNull();
    expect(v.semanticPayload).toBeNull();
  });

  it("legacy_generic_resource_leaf_still_works (向后兼容)", () => {
    const v = evaluateBrowseNode({
      id: "nsu=x;i=1",
      kind: "node",
      binding_json: JSON.stringify({ node_id: "nsu=x;i=1", data_type: "STRING" }),
    });
    expect(v.mode).toBe("unspecified");
    expect(v.resource).toEqual({
      resource_id: "node",
      parameters: { node_id: "nsu=x;i=1", data_type: "STRING" },
    });
    expect(v.blocks).toHaveLength(0);
  });

  it("legacy_empty_envelope_is_silent_none (分支节点)", () => {
    const v = evaluateBrowseNode({ id: "b", kind: "group", has_children: true, binding_json: "{}" });
    expect(v.resource).toBeNull();
    expect(v.blocks).toHaveLength(0);
  });

  it("legacy_malformed_binding_is_visible_block", () => {
    const v = evaluateBrowseNode({ id: "b", kind: "node", binding_json: "{oops" });
    expect(v.blocks[0].code).toBe(BROWSE_CONTRACT_CODES.PAYLOAD_MALFORMED);
  });
});
