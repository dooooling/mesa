// PR-C 第二批 B：动态表单 / family / 预览竞态 / 已有点位重新核验的纯模型测试。
//
// 这些断言不依赖 React，失败时能直接指向判定函数，而不是「某个按钮没变绿」。
import { describe, expect, it } from "vitest";
import {
  buildNckFormModel,
  buildNckPreviewParameters,
  checkNckFamily,
  emptyNckDraft,
  evaluateNckPointIdentity,
  nckDraftFromParameters,
  nckPreviewFingerprint,
  shouldAcceptNckPreview,
  validateNckForm,
  validateNckPreviewResponse,
  type NckDefinitionLike,
  type NckFormDraft,
} from "./nckPointModel";
import type { NckIdentityPreviewResponse } from "./nckCatalog";

function definition(overrides: Partial<NckDefinitionLike> = {}): NckDefinitionLike {
  return {
    variable_id: "syn.channel.status",
    revision: 1,
    family: "828d",
    scope: "collect",
    semantics_status: "verified",
    source: [{ kind: "manual" }],
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    identity_template: "syn.channel.status@channel={channel}",
    identity_keys: [{ path: "channel", required: true }],
    golden_ref: [{ entry: "nc_variable" }],
    ...overrides,
  };
}

function preview(overrides: Partial<NckIdentityPreviewResponse> = {}): NckIdentityPreviewResponse {
  return {
    point_key: "syn.channel.status@channel=1",
    identity_mode: "derived",
    scope: "collect",
    variable_id: "syn.channel.status",
    definition_revision: 1,
    definition_digest: "digest",
    ...overrides,
  };
}

function draftOf(target: Record<string, string>, args: Record<string, string> = {}): NckFormDraft {
  return { target, arguments: args };
}

describe("门禁 2：family 交叉校验（Endpoint × 定义 × 目录顶层）", () => {
  it("family_endpoint_and_definition_must_match_before_preview", () => {
    expect(
      checkNckFamily({
        endpointFamily: "828d",
        definitionFamily: "828d",
        catalogFamily: "828d",
      }),
    ).toEqual({ ok: true, family: "828d" });

    const missingEndpoint = checkNckFamily({
      endpointFamily: "",
      definitionFamily: "828d",
    });
    expect(missingEndpoint.ok).toBe(false);
    if (!missingEndpoint.ok) expect(missingEndpoint.code).toBe("FAMILY_ENDPOINT_MISSING");

    const missingDefinition = checkNckFamily({
      endpointFamily: "828d",
      definitionFamily: undefined,
    });
    expect(missingDefinition.ok).toBe(false);
    if (!missingDefinition.ok) expect(missingDefinition.code).toBe("FAMILY_DEFINITION_MISSING");

    const mismatch = checkNckFamily({
      endpointFamily: "828d",
      definitionFamily: "840d-sl",
    });
    expect(mismatch.ok).toBe(false);
    if (!mismatch.ok) {
      expect(mismatch.code).toBe("FAMILY_MISMATCH");
      // 必须给出两个具体值，便于用户定位是连接配置还是定义数据的问题。
      expect(mismatch.message).toContain("828d");
      expect(mismatch.message).toContain("840d-sl");
    }
  });

  it("catalog_top_level_family_null_is_not_a_rejection_reason", () => {
    expect(
      checkNckFamily({
        endpointFamily: "828d",
        definitionFamily: "828d",
        catalogFamily: null,
      }),
    ).toEqual({ ok: true, family: "828d" });
    expect(
      checkNckFamily({
        endpointFamily: "828d",
        definitionFamily: "828d",
        catalogFamily: undefined,
      }),
    ).toEqual({ ok: true, family: "828d" });
    // 非 null 且不一致才是异常（目录可能来自另一系列）。
    const mismatch = checkNckFamily({
      endpointFamily: "828d",
      definitionFamily: "828d",
      catalogFamily: "840d-sl",
    });
    expect(mismatch.ok).toBe(false);
    if (!mismatch.ok) expect(mismatch.code).toBe("FAMILY_CATALOG_MISMATCH");
  });
});

describe("门禁 4：Target/Arguments 由定义动态生成，非法值拒绝", () => {
  it("compound_target_generates_two_required_fields_from_definition", () => {
    const model = buildNckFormModel(
      definition({ targets: ["channel", "axis"], target_required: ["channel", "axis"] }),
    );
    expect(model.ok).toBe(true);
    expect(model.targetFields.map((f) => f.kind)).toEqual(["channel", "axis"]);
    expect(model.targetFields.every((f) => f.required)).toBe(true);
    // 只给 channel 不给 axis ⇒ 必须拒绝（不在前端硬编码「channel 必填」）。
    const validation = validateNckForm(
      definition({ targets: ["channel", "axis"], target_required: ["channel", "axis"] }),
      model,
      draftOf({ channel: "1", axis: "" }),
    );
    expect(validation.ok).toBe(false);
    expect(validation.issues.some((i) => i.code === "MISSING_TARGET" && i.field === "axis")).toBe(
      true,
    );
  });

  it("drive_object_entry_is_explicit_and_never_inferred_from_number", () => {
    const def = definition({
      targets: ["drive_object"],
      target_required: ["drive_object"],
      golden_ref: [{ entry: "nc_variable" }, { entry: "drive_object" }],
    });
    const model = buildNckFormModel(def);
    expect(model.ok).toBe(true);
    const field = model.targetFields[0];
    expect(field.kind).toBe("drive_object");
    // 候选集来自定义数据（golden_ref[].entry），不是前端写死的 26/130。
    expect(field.entryOptions).toEqual(["nc_variable", "drive_object"]);

    const missingEntry = validateNckForm(def, model, draftOf({ drive_object: "26", "drive_object.entry": "" }));
    expect(missingEntry.ok).toBe(false);
    expect(missingEntry.issues.some((i) => i.code === "MISSING_TARGET_ENTRY")).toBe(true);

    const badEntry = validateNckForm(
      def,
      model,
      draftOf({ drive_object: "26", "drive_object.entry": "guessed_from_number" }),
    );
    expect(badEntry.ok).toBe(false);
    expect(badEntry.issues.some((i) => i.code === "INVALID_TARGET_ENTRY")).toBe(true);

    const ok = validateNckForm(
      def,
      model,
      draftOf({ drive_object: "26", "drive_object.entry": "drive_object" }),
    );
    expect(ok.ok).toBe(true);
    expect(ok.target.drive_object).toEqual({ number: 26, entry: "drive_object" });
  });

  it("only_identity_keys_never_unlocks_submit (A42)", () => {
    const def: NckDefinitionLike = {
      variable_id: "syn.keys.only",
      scope: "collect",
      family: "828d",
      identity_keys: [{ path: "channel", required: true }],
    };
    const model = buildNckFormModel(def);
    expect(model.ok).toBe(false);
    if (!model.ok) expect(model.code).toBe("TARGETS_METADATA_MISSING");
    const validation = validateNckForm(def, model, draftOf({ channel: "1" }));
    expect(validation.ok).toBe(false);
    expect(validation.issues[0]?.code).toBe("FORM_MODEL_INVALID");
  });

  it("arguments_follow_definition_and_reject_invalid_values", () => {
    const def = definition({
      arguments: [
        {
          key: "index",
          label: { zh: "R 参数号", en: "index" },
          value_type: "integer",
          required: true,
          min: 0,
          max: 10,
        },
      ],
    });
    const model = buildNckFormModel(def);
    expect(model.ok).toBe(true);
    expect(model.argumentFields.map((f) => f.key)).toEqual(["index"]);
    expect(model.argumentFields[0].required).toBe(true);

    const missing = validateNckForm(def, model, draftOf({ channel: "1" }, {}));
    expect(missing.issues.some((i) => i.code === "MISSING_ARGUMENT")).toBe(true);

    const notInteger = validateNckForm(def, model, draftOf({ channel: "1" }, { index: "1.5" }));
    expect(notInteger.issues.some((i) => i.code === "INVALID_ARGUMENT")).toBe(true);

    const tooBig = validateNckForm(def, model, draftOf({ channel: "1" }, { index: "11" }));
    expect(tooBig.issues.some((i) => i.code === "INVALID_ARGUMENT")).toBe(true);

    const unknown = validateNckForm(def, model, draftOf({ channel: "1" }, { extra: "1" }));
    expect(unknown.issues.some((i) => i.code === "UNKNOWN_ARGUMENT")).toBe(true);

    const ok = validateNckForm(def, model, draftOf({ channel: "1" }, { index: "7" }));
    expect(ok.ok).toBe(true);
    expect(ok.arguments).toEqual({ index: 7 });

    // 身份键同时是 argument：即使规格 required=false，也按身份必需处理
    // （驱动在身份层会报 MISSING_IDENTITY_KEY，不能给出「看起来可提交」的表单）。
    const identityArg = definition({
      identity_keys: [{ path: "index", required: true }],
      arguments: [{ key: "index", value_type: "integer", required: false }],
    });
    const identityModel = buildNckFormModel(identityArg);
    expect(identityModel.argumentFields[0].required).toBe(true);
  });

  it("enum_argument_without_options_fails_closed", () => {
    const def = definition({
      arguments: [{ key: "mode", value_type: "enum", required: true }],
    });
    const model = buildNckFormModel(def);
    const validation = validateNckForm(def, model, draftOf({ channel: "1" }, { mode: "a" }));
    expect(validation.ok).toBe(false);
    expect(validation.issues.some((i) => i.code === "ARGUMENT_OPTIONS_MISSING")).toBe(true);
  });

  it("draft_from_parameters_keeps_nested_drive_object_and_revision", () => {
    const def = definition({
      targets: ["drive_object"],
      target_required: ["drive_object"],
      golden_ref: [{ entry: "nc_variable" }],
    });
    const model = buildNckFormModel(def);
    const draft = nckDraftFromParameters(model, {
      variable_id: "syn.channel.status",
      definition_revision: 1,
      target: { drive_object: { number: 5, entry: "nc_variable" } },
      arguments: {},
    });
    expect(draft.target.drive_object).toBe("5");
    expect(draft.target["drive_object.entry"]).toBe("nc_variable");
  });

  it("switching_variable_draft_is_rebuilt_empty (A30/A31 helper)", () => {
    const model = buildNckFormModel(definition());
    expect(emptyNckDraft(model)).toEqual({ target: { channel: "" }, arguments: {} });
  });
});

describe("门禁 5：竞态判定 = 请求序号 + 输入指纹双条件", () => {
  it("preview_fingerprint_is_stable_across_key_order", () => {
    const a = buildNckPreviewParameters({
      variableId: "v",
      revision: 1,
      target: { channel: 1, axis: 2 },
      arguments: {},
    });
    const b = buildNckPreviewParameters({
      variableId: "v",
      revision: 1,
      target: { axis: 2, channel: 1 },
      arguments: {},
    });
    expect(nckPreviewFingerprint(a)).toBe(nckPreviewFingerprint(b));
    expect(
      nckPreviewFingerprint(
        buildNckPreviewParameters({
          variableId: "v",
          revision: 1,
          target: { channel: 2, axis: 2 },
          arguments: {},
        }),
      ),
    ).not.toBe(nckPreviewFingerprint(a));
  });

  it("stale_preview_is_rejected_when_seq_or_fingerprint_differs", () => {
    const fp = "fp-current";
    expect(
      shouldAcceptNckPreview({
        responseSeq: 2,
        latestSeq: 2,
        responseFingerprint: fp,
        currentFingerprint: fp,
      }),
    ).toBe(true);
    // 迟到的 A（序号更小）
    expect(
      shouldAcceptNckPreview({
        responseSeq: 1,
        latestSeq: 2,
        responseFingerprint: fp,
        currentFingerprint: fp,
      }),
    ).toBe(false);
    // 序号相同但输入已变（指纹不同）
    expect(
      shouldAcceptNckPreview({
        responseSeq: 2,
        latestSeq: 2,
        responseFingerprint: "fp-old",
        currentFingerprint: fp,
      }),
    ).toBe(false);
  });
});

describe("门禁 3/6：预览成功 ≠ 有采集资格；失败不得回退旧 Key", () => {
  it("preview_success_never_overrides_scope_admission", () => {
    const def = definition({ scope: "identity" });
    const expected = buildNckPreviewParameters({
      variableId: "syn.channel.status",
      revision: 1,
      target: { channel: 1 },
      arguments: {},
    });
    const check = validateNckPreviewResponse({
      definition: def,
      expected,
      response: preview({ scope: "identity", point_key: "legal-looking-key" }),
    });
    expect(check.ok).toBe(false);
    if (!check.ok) expect(check.code).toBe("PREVIEW_SCOPE_IDENTITY");
  });

  it("preview_coordinate_and_identity_mode_mismatch_are_rejected", () => {
    const def = definition();
    const expected = buildNckPreviewParameters({
      variableId: "syn.channel.status",
      revision: 1,
      target: { channel: 1 },
      arguments: {},
    });
    const wrongVariable = validateNckPreviewResponse({
      definition: def,
      expected,
      response: preview({ variable_id: "other.variable" }),
    });
    expect(wrongVariable.ok).toBe(false);
    if (!wrongVariable.ok) expect(wrongVariable.code).toBe("PREVIEW_COORDINATE_MISMATCH");

    const wrongRevision = validateNckPreviewResponse({
      definition: def,
      expected,
      response: preview({ definition_revision: 2 }),
    });
    expect(wrongRevision.ok).toBe(false);
    if (!wrongRevision.ok) expect(wrongRevision.code).toBe("PREVIEW_COORDINATE_MISMATCH");

    const userMode = validateNckPreviewResponse({
      definition: def,
      expected,
      response: preview({ identity_mode: "user" }),
    });
    expect(userMode.ok).toBe(false);
    if (!userMode.ok) expect(userMode.code).toBe("PREVIEW_IDENTITY_MODE_INVALID");

    const ok = validateNckPreviewResponse({ definition: def, expected, response: preview() });
    expect(ok).toEqual({ ok: true, pointKey: "syn.channel.status@channel=1" });
  });
});

describe("已有点位重新核验：记录里的 point_key 不是当前可提交身份", () => {
  const point = {
    kind: "nck_variable",
    point_key: "syn.channel.status@channel=1",
    parameters: {
      variable_id: "syn.channel.status",
      definition_revision: 1,
      target: { channel: 1 },
      arguments: {},
    },
  };

  it("verified_only_when_preview_recomputes_the_same_key", () => {
    const check = evaluateNckPointIdentity({
      point,
      definition: definition(),
      endpointFamily: "828d",
      catalogFamily: null,
      preview: preview(),
    });
    expect(check.ok).toBe(true);
  });

  it("stored_key_that_no_longer_matches_preview_is_unverified (three elements)", () => {
    const check = evaluateNckPointIdentity({
      point,
      definition: definition(),
      endpointFamily: "828d",
      preview: preview({ point_key: "syn.channel.status@channel=2" }),
    });
    expect(check.ok).toBe(false);
    if (!check.ok) {
      expect(check.code).toBe("POINT_KEY_MISMATCH");
      expect(check.message).toContain("syn.channel.status@channel=1");
      expect(check.message).toContain("syn.channel.status@channel=2");
      expect(check.message).toContain("syn.channel.status");
      expect(check.message).toContain("rev 1");
    }
  });

  it("identity_definition_is_unverified_even_if_preview_returns_a_key", () => {
    const check = evaluateNckPointIdentity({
      point,
      definition: definition({ scope: "identity" }),
      endpointFamily: "828d",
      preview: preview({ scope: "identity" }),
    });
    expect(check.ok).toBe(false);
    if (!check.ok) expect(check.code).toBe("SCOPE_IDENTITY");
  });

  it("missing_definition_and_parameter_drift_are_unverified", () => {
    const missing = evaluateNckPointIdentity({
      point,
      definition: null,
      endpointFamily: "828d",
      preview: preview(),
    });
    expect(missing.ok).toBe(false);
    if (!missing.ok) expect(missing.code).toBe("DEFINITION_MISSING");

    const drifted = {
      ...point,
      parameters: { ...point.parameters, target: { channel: 1, axis: 2 } },
    };
    const drift = evaluateNckPointIdentity({
      point: drifted,
      definition: definition(),
      endpointFamily: "828d",
      preview: preview(),
    });
    expect(drift.ok).toBe(false);
    if (!drift.ok) expect(drift.code).toBe("PARAMETERS_INVALID");

    const revisionDrift = evaluateNckPointIdentity({
      point: { ...point, parameters: { ...point.parameters, definition_revision: 2 } },
      definition: definition(),
      endpointFamily: "828d",
      preview: preview({ definition_revision: 2 }),
    });
    expect(revisionDrift.ok).toBe(false);
    if (!revisionDrift.ok) expect(revisionDrift.code).toBe("DEFINITION_REVISION_MISMATCH");

    // 展示用的 identity 块若被折进 parameters，驱动会整体拒绝 ⇒ 同样不可提交。
    const leakedIdentity = evaluateNckPointIdentity({
      point: {
        ...point,
        parameters: { ...point.parameters, identity: { preview_point_key: "x" } },
      },
      definition: definition(),
      endpointFamily: "828d",
      preview: preview(),
    });
    expect(leakedIdentity.ok).toBe(false);
    if (!leakedIdentity.ok) expect(leakedIdentity.code).toBe("PARAMETERS_INVALID");
  });
});
