// NCK Browse 语义**载荷解析**单测（IPC 1.8 收窄后）。
//
// 历史（PR #150）：这些断言测的是"前端如何推断一个叶能不能选"（scope 白名单、
// family 比对、定义目录查表）。IPC 1.8 起这些判定**搬到驱动侧**（
// `drivers/sinumerik-nck/src/presets.rs` 的 `semantic_leaf`），因此本文件改为
// 测本模块**唯一剩下的职责**：把驱动给的不透明载荷解析成父层认识的类型，
// 形状不合契约即 fail-closed。
//
// 注意：这里刻意**不**再构造定义目录 ——「前端不再查目录」本身就是本轮要锁住的
// 性质（见 `payload_is_parsed_without_any_definition_catalog`）。
import { describe, expect, it } from "vitest";
import {
  NCK_PAYLOAD_CODES,
  createNckSemanticHandler,
  parseNckSemanticPayload,
  type NckBrowseLeafSelection,
} from "./nckBrowseSemantic";

/** 构造驱动侧会给出的语义载荷（与 presets.rs `semantic_payload` 同形）。 */
function payload(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
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
    ...overrides,
  };
}

describe("parseNckSemanticPayload：载荷形状解析（fail-closed）", () => {
  it("selectable_payload_carries_opaque_selection_without_identity_in_parameters (A1)", () => {
    const parsed = parseNckSemanticPayload(payload());
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.label).toBe("选用（语义）");
    const sel = parsed.selection as NckBrowseLeafSelection;
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

  it("leaf_without_identity_is_rejected (A16)", () => {
    const parsed = parseNckSemanticPayload(payload({ identity: undefined }));
    expect(parsed.ok).toBe(false);
    if (parsed.ok) return;
    expect(parsed.code).toBe(NCK_PAYLOAD_CODES.MISSING_IDENTITY);
    expect(parsed.message).toContain("identity");
  });

  it("malformed_identity_is_rejected", () => {
    for (const bad of ["not-an-object", 42, ["a"], null]) {
      const parsed = parseNckSemanticPayload(payload({ identity: bad }));
      expect(parsed.ok, `identity=${JSON.stringify(bad)} 必须被拒`).toBe(false);
    }
  });

  it("native_kind_is_never_offered_from_semantic_browse", () => {
    const parsed = parseNckSemanticPayload(payload({ kind: "nck_native" }));
    expect(parsed.ok).toBe(false);
    if (parsed.ok) return;
    expect(parsed.code).toBe(NCK_PAYLOAD_CODES.NATIVE_NOT_FROM_BROWSE);
    expect(parsed.message).toContain("高级原生配置");
  });

  it("unexpected_point_kind_is_rejected", () => {
    for (const kind of ["widget_lamp", "", undefined, 7]) {
      const parsed = parseNckSemanticPayload(payload({ kind }));
      expect(parsed.ok, `kind=${String(kind)} 必须被拒`).toBe(false);
      if (!parsed.ok) {
        expect(parsed.code).toBe(NCK_PAYLOAD_CODES.UNEXPECTED_POINT_KIND);
      }
    }
  });

  it("missing_variable_id_is_rejected", () => {
    const parsed = parseNckSemanticPayload(
      payload({ parameters: { definition_revision: 1 } }),
    );
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.code).toBe(NCK_PAYLOAD_CODES.MISSING_VARIABLE_ID);
  });

  it("missing_or_non_integer_revision_is_rejected", () => {
    for (const revision of [undefined, "1", 1.5, null]) {
      const parsed = parseNckSemanticPayload(
        payload({ parameters: { variable_id: "x", definition_revision: revision } }),
      );
      expect(parsed.ok, `revision=${String(revision)} 必须被拒`).toBe(false);
      if (!parsed.ok) expect(parsed.code).toBe(NCK_PAYLOAD_CODES.MISSING_REVISION);
    }
  });

  it("non_object_parameters_is_rejected", () => {
    for (const parameters of [undefined, "{}", [1, 2], null]) {
      const parsed = parseNckSemanticPayload(payload({ parameters }));
      expect(parsed.ok).toBe(false);
      if (!parsed.ok) expect(parsed.code).toBe(NCK_PAYLOAD_CODES.INVALID_PARAMETERS);
    }
  });

  it("null_preview_key_keeps_payload_usable_with_warning_semantics (A10)", () => {
    // preview 为 null 只影响**展示**（驱动会在诊断里给 WARNING）；载荷本身仍可
    // 进语义表单补全，因此解析必须成功。
    const parsed = parseNckSemanticPayload(
      payload({
        identity: {
          template: "t@{x}",
          keys: [{ path: "x", required: true }],
          preview_point_key: null,
        },
      }),
    );
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.selection.identity.preview_point_key).toBeNull();
  });

  it("explicit_resource_id_is_honoured", () => {
    const parsed = parseNckSemanticPayload(payload({ resource_id: "  variable  " }));
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.selection.resourceId).toBe("variable");
  });

  it("identity_keys_are_shape_filtered_not_trusted", () => {
    const parsed = parseNckSemanticPayload(
      payload({
        identity: {
          template: "t",
          keys: [{ path: "a", required: true }, "junk", null, { path: 7 }],
          preview_point_key: "k",
        },
      }),
    );
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.selection.identity.keys).toEqual([
      { path: "a", required: true },
      { path: undefined, required: undefined },
    ]);
  });

  it("payload_is_parsed_without_any_definition_catalog", () => {
    // 关键性质：解析器不持有目录、不查 scope、不比对 family —— 未知 variable_id
    // 也必须原样通过（准入结论由驱动在 wire 上给出）。若哪天又有人把目录查表
    // 塞回来，这条会红（A13 的判定点已搬走）。
    const parsed = parseNckSemanticPayload(
      payload({
        parameters: {
          variable_id: "not.in.any.catalog",
          definition_revision: 99,
          target: {},
          arguments: {},
        },
      }),
    );
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.selection.parameters.variable_id).toBe("not.in.any.catalog");
  });

  it("parse_is_pure_and_stateless", () => {
    const p = payload();
    const a = parseNckSemanticPayload(p);
    const b = parseNckSemanticPayload(p);
    expect(a).toEqual(b);
    // 不得改写入参（父层可能复用同一载荷做展示）。
    expect(p).toEqual(payload());
  });
});

describe("createNckSemanticHandler", () => {
  it("handler_declares_not_provided_reason_and_delegates", () => {
    const handler = createNckSemanticHandler();
    expect(handler.name).toBe("nck-semantic");
    expect(handler.notProvidedReason).toContain("未提供语义表单入口");
    expect(handler.selectLabel).toBe("选用（语义）");
    expect(handler.parse(payload()).ok).toBe(true);
  });

  it("handler_is_stateless_across_payloads", () => {
    const handler = createNckSemanticHandler();
    expect(handler.parse(payload({ kind: "nck_native" })).ok).toBe(false);
    expect(handler.parse(payload()).ok).toBe(true);
    expect(handler.parse(payload()).ok).toBe(true);
  });
});
