// 定义元数据 sidecar 的**容错**与门禁单测。
//
// 为什么必须单独钉死容错：四块 sidecar 是"附加信息"，它们的失败模式是**静默**的——
// 少一个字段、类型漂移、取值超出词表，界面若不崩就会给出一个"看起来合理"的结论
// （空单位、默认可用），而那正是要禁止的"猜"。这里逐条钉住"非法 ⇒ 未知/不可用"，
// 让任何"补默认值"的改动立刻变红。
//
// 判据矩阵（与验收表一致）：
//   ready          ⇒ 可添加，无标注
//   candidate      ⇒ 可添加，必须标注"未经真机读取验证"
//   hold           ⇒ 禁止添加，原因必须可见
//   reference_only ⇒ 禁止添加（仅展示）
//   缺失/非法     ⇒ 禁止添加（fail-closed），且区分"缺失"与"非法"
import { describe, expect, it } from "vitest";
import {
  AVAILABILITY_STATE,
  AVAILABILITY_UNAVAILABLE,
  describeObjectDynamicState,
  evaluateAvailabilityGate,
  objectDynamicState,
  readAvailabilityMeta,
  readDefinitionMeta,
  readManualLocationMeta,
  readUnitMeta,
  summarizeAvailability,
} from "./definitionMeta";
import { parseAvailabilityMeta, parseDefinitionsResponse, parseUnitMeta } from "./nckCatalog";

describe("容错：非法/缺失一律收敛为“未知”，不崩、不猜", () => {
  it("definition_without_any_sidecar_is_all_unknown_and_never_available", () => {
    const meta = readDefinitionMeta(null);
    expect(meta.unit.status).toBe("unknown");
    expect(meta.unit.value).toBeNull();
    expect(meta.unit.label).toBe("单位未核实");
    expect(meta.manualLocation.label).toBeNull();
    expect(meta.indexRule.kind).toBeNull();
    expect(meta.indexRule.verified).toBe(false);
    // 缺失 ⇒ 前端内部态 unavailable（**不是**后端四态之一，也不是"默认放行"）。
    expect(meta.availability.state).toBe(AVAILABILITY_UNAVAILABLE);
    expect(meta.availability.invalidCode).toBe("AVAILABILITY_MISSING");
    expect(evaluateAvailabilityGate(meta.availability).ok).toBe(false);
  });

  it("malformed_unit_shapes_never_produce_a_unit_value", () => {
    const shapes: unknown[] = [
      "mm/min",
      42,
      ["mm"],
      null,
      { value: 5, status: "verified" },
      { value: "", status: "verified" },
      { value: "mm", status: 1 },
      { value: "mm" },
      {},
    ];
    for (const shape of shapes) {
      const unit = readUnitMeta(shape);
      expect(unit.status).toBe("unknown");
      expect(unit.value).toBeNull();
      expect(unit.label).toBe("单位未核实");
    }
  });

  it("verified_unit_without_value_is_treated_as_unknown_not_empty_string", () => {
    const unit = readUnitMeta({ value: null, source_ref: "manual", status: "verified" });
    expect(unit.status).toBe("unknown");
    expect(unit.value).toBeNull();
    expect(unit.label).toBe("单位未核实");
    expect(unit.hint).toContain("未给出取值");
    // 有值 + verified 才展示真实单位。
    expect(readUnitMeta({ value: "mm/min", status: "verified" }).label).toBe("mm/min");
  });

  it("manual_location_rejects_bad_pages_and_hides_when_nothing_usable", () => {
    // 页码只认 >=1 的整数：字符串 "272"、0、负数、小数都不认（不猜）。
    for (const page of ["272", 0, -3, 2.5, null]) {
      expect(readManualLocationMeta({ page }).page).toBeNull();
    }
    expect(readManualLocationMeta({ page: 272 }).page).toBe(272);
    // 一个可用字段都没有 ⇒ 不显示位置（不伪造"第 0 页"）。
    expect(readManualLocationMeta({}).label).toBeNull();
    expect(readManualLocationMeta("A5E53370586B").label).toBeNull();
    // 部分字段可用时只拼可用的部分。
    expect(readManualLocationMeta({ section: "5.3.16" }).label).toBe("§5.3.16");
    expect(
      readManualLocationMeta({
        document_number: "A5E53370586B AC",
        version: "03/2025",
        section: "5.3.16",
        page: 272,
      }).label,
    ).toBe("A5E53370586B AC 03/2025 · §5.3.16 · 第 272 页");
  });
});

describe("可用性门禁矩阵：四态 + 缺失 + 非法（fail-closed）", () => {
  it("four_declared_states_map_to_exactly_one_gate_result_each", () => {
    const ready = evaluateAvailabilityGate(readAvailabilityMeta({ state: "ready" }));
    expect(ready).toEqual({
      ok: true,
      state: AVAILABILITY_STATE.READY,
      requiresUnverifiedNotice: false,
      notice: null,
    });

    const candidate = evaluateAvailabilityGate(readAvailabilityMeta({ state: "candidate" }));
    expect(candidate.ok).toBe(true);
    expect(candidate.ok && candidate.requiresUnverifiedNotice).toBe(true);
    expect(candidate.ok && candidate.notice).toContain("未经真机读取验证");

    const hold = evaluateAvailabilityGate(
      readAvailabilityMeta({ state: "hold", reason_zh: "无真机读取证据" }),
    );
    expect(hold.ok).toBe(false);
    expect(hold.ok === false && hold.code).toBe("AVAILABILITY_HOLD");
    // 原因必须进文案（不能只剩一个码）。
    expect(hold.ok === false && hold.message).toContain("无真机读取证据");

    const reference = evaluateAvailabilityGate(readAvailabilityMeta({ state: "reference_only" }));
    expect(reference.ok).toBe(false);
    expect(reference.ok === false && reference.code).toBe("AVAILABILITY_REFERENCE_ONLY");
    expect(reference.ok === false && reference.message).toContain("仅作展示");
  });

  it("hold_without_reason_says_so_instead_of_inventing_one", () => {
    const gate = evaluateAvailabilityGate(readAvailabilityMeta({ state: "hold" }));
    expect(gate.ok).toBe(false);
    expect(gate.ok === false && gate.message).toContain("驱动未给出原因");
  });

  it("missing_or_invalid_availability_is_blocked_and_reported_distinctly", () => {
    const missing = readAvailabilityMeta(undefined);
    expect(missing.invalidCode).toBe("AVAILABILITY_MISSING");
    const missingGate = evaluateAvailabilityGate(missing);
    expect(missingGate.ok).toBe(false);
    expect(missingGate.ok === false && missingGate.code).toBe("AVAILABILITY_MISSING");

    // state 不是字符串 ⇒ 等同于没声明（形状非法）。
    expect(readAvailabilityMeta({ state: 7 }).invalidCode).toBe("AVAILABILITY_MISSING");

    // state 是字符串但不在四态词表 ⇒ 非法，且原文保留（便于排查）。
    const invalid = readAvailabilityMeta({ state: "maybe" });
    expect(invalid.state).toBe(AVAILABILITY_UNAVAILABLE);
    expect(invalid.invalidCode).toBe("AVAILABILITY_INVALID");
    expect(invalid.rawState).toBe("maybe");
    const invalidGate = evaluateAvailabilityGate(invalid);
    expect(invalidGate.ok).toBe(false);
    expect(invalidGate.ok === false && invalidGate.message).toContain("maybe");
    expect(invalidGate.ok === false && invalidGate.message).toContain("fail-closed");
  });

  it("static_availability_is_not_dynamic_object_state", () => {
    // ready 只是静态结论；本机对象是否确认是另一条独立判据。
    expect(objectDynamicState({ needCount: 0, filledFromDevice: false })).toBe("not_required");
    expect(objectDynamicState({ needCount: 2, filledFromDevice: false })).toBe("unconfirmed");
    expect(objectDynamicState({ needCount: 2, filledFromDevice: true })).toBe("confirmed");
    expect(describeObjectDynamicState("unconfirmed")).toContain("不代表本机存在该对象");
  });
});

describe("目录解析容错：sidecar 形状非法不拖垮整页", () => {
  function entry(overrides: Record<string, unknown>): Record<string, unknown> {
    return {
      variable_id: "syn.meta.a",
      revision: 1,
      family: "828d",
      scope: "collect",
      semantics_status: "confirmed",
      label: { zh: "合成", en: "synthetic" },
      source: [],
      targets: [],
      target_required: [],
      arguments: [],
      identity_template: "",
      identity_keys: [],
      golden_ref: [],
      digest: "digest",
      ...overrides,
    };
  }

  function envelope(definitions: unknown[]) {
    return {
      format_version: 1,
      family: "828d",
      revision: 1,
      driver_id: "sinumerik-nck",
      driver_version: "0.1.0",
      note: null,
      total: definitions.length,
      limit: 100,
      next_cursor: null,
      definitions,
    };
  }

  it("valid_sidecar_is_kept_verbatim", () => {
    const parsed = parseDefinitionsResponse(
      envelope([
        entry({
          unit: { value: "mm/min", source_ref: "manual#1", status: "verified" },
          manual_location: {
            document_number: "A5E53370586B AC",
            version: "03/2025",
            section: "5.3.16",
            page: 272,
            source_ref: "manual#1",
          },
          index_rule: { kind: "channel", status: "verified", source_ref: "manual#1" },
          availability: { state: "hold", basis: ["no_device_read"], reason_zh: "无真机读取" },
        }),
      ]),
    );
    const def = parsed.definitions[0]!;
    expect(def.unit).toEqual({ value: "mm/min", source_ref: "manual#1", status: "verified" });
    expect(def.manual_location?.page).toBe(272);
    expect(def.index_rule?.kind).toBe("channel");
    expect(def.availability).toEqual({
      state: "hold",
      basis: ["no_device_read"],
      reason_zh: "无真机读取",
    });
  });

  it("malformed_sidecar_is_dropped_but_the_page_still_parses", () => {
    const parsed = parseDefinitionsResponse(
      envelope([
        entry({
          unit: "mm/min",
          manual_location: 42,
          index_rule: ["channel"],
          availability: "ready",
        }),
      ]),
    );
    const def = parsed.definitions[0]!;
    // 形状非法 ⇒ 字段不出现（等价"未知/不可用"），但**不**整页拒绝。
    expect(def.unit).toBeUndefined();
    expect(def.manual_location).toBeUndefined();
    expect(def.index_rule).toBeUndefined();
    expect(def.availability).toBeUndefined();
    expect(parseUnitMeta("mm/min")).toBeUndefined();
    expect(parseAvailabilityMeta("ready")).toBeUndefined();
  });

  it("summarize_counts_addable_and_hold_separately", () => {
    // 口径：addable = ready + candidate；hold/reference_only/缺失都不得混进"可添加"。
    const counts = summarizeAvailability([
      { availability: { state: "ready", basis: [], reason_zh: null } },
      { availability: { state: "candidate", basis: [], reason_zh: null } },
      { availability: { state: "hold", basis: [], reason_zh: "暂缓原因" } },
      { availability: { state: "reference_only", basis: [], reason_zh: null } },
      { availability: undefined },
    ]);
    expect(counts).toEqual({
      total: 5,
      addable: 2,
      ready: 1,
      candidate: 1,
      hold: 1,
      referenceOnly: 1,
      unavailable: 1,
    });
    expect(counts.addable + counts.hold + counts.referenceOnly + counts.unavailable).toBe(
      counts.total,
    );
    // hold 明确**不**计入可添加。
    expect(counts.addable).not.toBe(counts.total - counts.hold);
  });

  it("summarize_treats_invalid_availability_as_unavailable_not_addable", () => {
    const counts = summarizeAvailability([
      { availability: { state: "pending_review", basis: [], reason_zh: null } },
      { availability: { state: 1 } },
      {},
    ]);
    expect(counts.addable).toBe(0);
    expect(counts.unavailable).toBe(3);
    expect(counts.hold).toBe(0);
  });

  it("unknown_availability_state_is_preserved_so_the_gate_can_report_it", () => {
    const parsed = parseDefinitionsResponse(
      envelope([entry({ availability: { state: "pending_review", basis: [], reason_zh: null } })]),
    );
    // 解析层不吞掉非法取值：门禁要能逐字报出"取值非法"。
    expect(parsed.definitions[0]!.availability?.state).toBe("pending_review");
    const gate = evaluateAvailabilityGate(readDefinitionMeta(parsed.definitions[0]).availability);
    expect(gate.ok).toBe(false);
    expect(gate.ok === false && gate.code).toBe("AVAILABILITY_INVALID");
    expect(gate.ok === false && gate.message).toContain("pending_review");
  });
});
