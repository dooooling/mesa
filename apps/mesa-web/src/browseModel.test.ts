// Browse → Selection 映射单测：锁定信封形态，不解释协议语义。
//
// PR #150 解耦后：通用层不认识任何 point kind。带 `kind` 的信封（语义载荷）
// 通用层一律不猜（返回 null），identity 之类的语义块由 `parseBrowseEnvelope`
// 原样透出给策略层（见 nckBrowsePolicy.test.ts）。
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
