// PR-C P0-1 回归测试：`scope` 准入的 6 条必备断言（A13 / A40 / A43）。
//
// 目录 fixture 用**合成 variable_id**，不抄 `definitions/828d/r1.json` 的真实
// 6 个 identity id——真实 42/36/6 已由 Driver 侧测试锁定；前端测试只验证
// 「判据 + 两处执行点」，把 Driver 数据搬进前端会违反设计 §1.6 的红线。
import { describe, expect, it } from "vitest";
import {
  admitNckDefinition,
  collectableDefinitions,
  evaluateNckSelectionAdd,
  isCollectableScope,
  validateNckPointSubmission,
  type NckVariableDefinitionLike,
} from "./nckCollectability";

/** 合成目录：42 条 = 36 collect + 6 identity（与 I-2 真实计数同构）。 */
function syntheticCatalog(): NckVariableDefinitionLike[] {
  const out: NckVariableDefinitionLike[] = [];
  for (let i = 1; i <= 36; i += 1) {
    out.push({ variable_id: `synthetic.collect.${String(i).padStart(2, "0")}`, scope: "collect" });
  }
  for (let i = 1; i <= 6; i += 1) {
    out.push({ variable_id: `synthetic.identity.${String(i).padStart(2, "0")}`, scope: "identity" });
  }
  return out;
}

const catalog = syntheticCatalog();
const identityDef = catalog.find((d) => d.scope === "identity")!;
const collectDef = catalog.find((d) => d.scope === "collect")!;

/** 一个格式完全合法的待提交点位（证明拒绝只来自 scope，而不是形状）。 */
function validSelection(definition: NckVariableDefinitionLike) {
  return {
    kind: "nck_variable",
    point_key: `${definition.variable_id}@channel=1`,
    parameters: {
      variable_id: definition.variable_id,
      definition_revision: 1,
      target: { channel: 1 },
      arguments: {},
    },
  };
}

describe("P0-1 scope 准入（A13/A40/A43）", () => {
  it("42 条目录只有 36 条 collect 成为候选；6 条 identity 不进入可采集列表", () => {
    expect(catalog).toHaveLength(42);
    const collectable = collectableDefinitions(catalog);
    expect(collectable).toHaveLength(36);
    expect(collectable.every((d) => isCollectableScope(d.scope))).toBe(true);

    const collectableIds = new Set(collectable.map((d) => d.variable_id));
    for (const d of catalog.filter((x) => x.scope === "identity")) {
      expect(collectableIds.has(d.variable_id)).toBe(false);
      expect(isCollectableScope(d.scope)).toBe(false);
    }
    // 判据逐条严格等价，防止未来把 `!== "identity"` 反写回来。
    for (const d of catalog) {
      expect(isCollectableScope(d.scope)).toBe(d.scope === "collect");
    }
  });

  it("强行选中 identity 定义：UI 加入门仍拒绝（不依赖列表是否过滤过）", () => {
    // 模拟「深链 / 被改写的组件状态」直接把 identity 定义塞进选中态。
    const gate = evaluateNckSelectionAdd(identityDef);
    expect(gate.ok).toBe(false);
    if (!gate.ok) {
      expect(gate.code).toBe("SCOPE_IDENTITY");
      expect(gate.message).toContain("identity");
    }
    // 同一入口对 collect 定义必须放行（证明门是活的，不是永久关闭）。
    expect(evaluateNckSelectionAdd(collectDef)).toEqual({ ok: true });
    // 定义整体缺失同样拒绝（不把「没有元数据」当默认可采集）。
    expect(evaluateNckSelectionAdd(null).ok).toBe(false);
  });

  it("绕过 UI 直接构造提交：提交模型拒绝，且不抛异常", () => {
    const selection = validSelection(identityDef);
    expect(() =>
      validateNckPointSubmission({ definition: identityDef, selection }),
    ).not.toThrow();
    const result = validateNckPointSubmission({ definition: identityDef, selection });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      // 前端自定义码，不冒充 Driver/Core 的错误码（设计 §4.3 建议名）。
      expect(result.code).toBe("SCOPE_NOT_COLLECTABLE");
    }

    // 正例：collect 定义 + 合法选择 ⇒ 放行，且信封恰为 3 字段。
    const ok = validateNckPointSubmission({ definition: collectDef, selection: validSelection(collectDef) });
    expect(ok.ok).toBe(true);
    if (ok.ok) {
      expect(Object.keys(ok.selection).sort()).toEqual(["kind", "parameters", "point_key"]);
    }
  });

  it("scope 缺失 / null / 大小写变体 / 未来值 / 非字符串：一律 fail-closed", () => {
    const cases: Array<{ def: NckVariableDefinitionLike; code: string }> = [
      { def: { variable_id: "synthetic.missing" }, code: "SCOPE_MISSING" },
      { def: { variable_id: "synthetic.undefined", scope: undefined }, code: "SCOPE_MISSING" },
      { def: { variable_id: "synthetic.null", scope: null }, code: "SCOPE_MISSING" },
      { def: { variable_id: "synthetic.upper", scope: "IDENTITY" }, code: "SCOPE_UNKNOWN" },
      { def: { variable_id: "synthetic.title", scope: "Collect" }, code: "SCOPE_UNKNOWN" },
      { def: { variable_id: "synthetic.future", scope: "future_value" }, code: "SCOPE_UNKNOWN" },
      { def: { variable_id: "synthetic.number", scope: 1 }, code: "SCOPE_UNKNOWN" },
      { def: { variable_id: "synthetic.bool", scope: true }, code: "SCOPE_UNKNOWN" },
    ];
    for (const { def, code } of cases) {
      expect(isCollectableScope(def.scope)).toBe(false);
      const admission = admitNckDefinition(def);
      expect(admission.collectable).toBe(false);
      if (!admission.collectable) expect(admission.code).toBe(code);
      const result = validateNckPointSubmission({ definition: def, selection: validSelection(def) });
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe(code);
    }
    // 列表过滤同样一条都不放行（不是「列表里过滤、提交时放行」）。
    expect(collectableDefinitions(cases.map((c) => c.def))).toEqual([]);
  });

  it("Preview 返回合法 Key：不能覆盖 scope 准入限制（也不得反写缺失 scope）", () => {
    // 情形 1：identity 定义 —— Preview 按真实实现同样会返回合法 Key。
    const preview = {
      point_key: `${identityDef.variable_id}@channel=1`,
      scope: "identity",
    };
    const identityResult = validateNckPointSubmission({
      definition: identityDef,
      selection: validSelection(identityDef),
      preview,
    });
    expect(identityResult.ok).toBe(false);
    if (!identityResult.ok) expect(identityResult.code).toBe("SCOPE_NOT_COLLECTABLE");

    // 情形 2：scope 缺失，Preview 甚至「声称」collect —— 仍按缺失拒绝。
    const missing: NckVariableDefinitionLike = { variable_id: "synthetic.missing" };
    const missingResult = validateNckPointSubmission({
      definition: missing,
      selection: validSelection(missing),
      preview: { point_key: "synthetic.missing@channel=1", scope: "collect" },
    });
    expect(missingResult.ok).toBe(false);
    if (!missingResult.ok) expect(missingResult.code).toBe("SCOPE_MISSING");

    // 情形 3：未知 scope + 合法 Key —— 仍是 UNKNOWN，不得默认放行。
    const unknown: NckVariableDefinitionLike = { variable_id: "synthetic.future", scope: "future_value" };
    const unknownResult = validateNckPointSubmission({
      definition: unknown,
      selection: validSelection(unknown),
      preview: { point_key: "synthetic.future@channel=1", scope: "future_value" },
    });
    expect(unknownResult.ok).toBe(false);
    if (!unknownResult.ok) expect(unknownResult.code).toBe("SCOPE_UNKNOWN");
  });
});
