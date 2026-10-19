// PR-C P0-2 回归测试：NCK 任务编辑安全 + 双真值收敛。
//
// 覆盖验收：A19（NCK 绝不产出 mesa.resources.v1）、A28（装配单真值）、
// A29（NCK kind 恒 nck.points.v1）、A36（Selection 冻结算法不回归）、
// A37（快照非 ready 禁止保存），以及评审 5 条要求对应的断言。
import { describe, expect, it } from "vitest";
import {
  CANONICAL_RESOURCES_KIND,
  NCK_POINTS_KIND,
  assembleBinding,
  bindingKindForDriver,
  mergeAcquisitionTasks,
  selectionsOf,
  type AcquisitionTaskShape,
  type NckPointSelection,
  type ResourceSelection,
} from "./deviceModel";
import {
  collectEndpointPointKeys,
  mergeNckAcquisitionTasks,
  nckPointsOf,
  parseTaskBinding,
  selectEditableTask,
  validateNckTaskSubmission,
  type NckTaskSubmissionInput,
} from "./nckTaskBinding";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-nck";

/** 定义目录 fixture：只用于 scope 准入，不用真实 Driver 数据。 */
const DEFINITIONS = {
  "var.collect.01": { variable_id: "var.collect.01", scope: "collect" },
  "var.collect.02": { variable_id: "var.collect.02", scope: "collect" },
  "var.identity.01": { variable_id: "var.identity.01", scope: "identity" },
} as const;

function point(variableId: string, target: Record<string, number>, pointKey?: string): NckPointSelection {
  return {
    kind: "nck_variable",
    point_key: pointKey ?? `${variableId}@${Object.entries(target).map(([k, v]) => `${k}=${v}`).join(",")}`,
    parameters: { variable_id: variableId, definition_revision: 1, target, arguments: {} },
  };
}

function nckTask(
  id: string,
  points: NckPointSelection[],
  schedule: AcquisitionTaskShape["schedule"] = { mode: "poll", interval_ms: 5000 },
): AcquisitionTaskShape {
  return { id, schedule, binding: { kind: NCK_POINTS_KIND, config: { points } } };
}

function canonicalTask(
  id: string,
  pointKeys: string[],
  schedule: AcquisitionTaskShape["schedule"] = { mode: "poll", interval_ms: 5000 },
): AcquisitionTaskShape {
  const selections: ResourceSelection[] = [
    {
      resource_id: "variable",
      parameters: {},
      outputs: pointKeys.map((point_key, i) => ({ output: `out-${i}`, point_key })),
    },
  ];
  return { id, schedule, binding: { kind: CANONICAL_RESOURCES_KIND, config: { selections } } };
}

function submission(overrides: Partial<NckTaskSubmissionInput> = {}): NckTaskSubmissionInput {
  return {
    tasks: [],
    driverId: NCK_DRIVER,
    points: [],
    definitions: DEFINITIONS,
    snapshotState: "ready",
    loadedEndpointId: EP,
    currentEndpointId: EP,
    intervalMs: 1000,
    ...overrides,
  };
}

describe("P0-2 编辑全链：已有合法 NCK 任务 → 编辑 → 保存 → 重新读取", () => {
  it("保留任务 ID 与 poll Schedule，写入 points 后可逐字读回", () => {
    const existing = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const edited = [
      point("var.collect.01", { channel: 1 }),
      point("var.collect.02", { axis: 2, channel: 1 }),
    ];
    const result = mergeNckAcquisitionTasks(
      submission({ tasks: [existing], points: edited, intervalMs: 250 }),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    expect(result.tasks).toHaveLength(1);
    expect(result.taskId).toBe("nck-task");
    const saved = result.tasks[0];
    expect(saved.id).toBe("nck-task");
    expect(saved.schedule).toEqual({ mode: "poll", interval_ms: 250 });
    expect(saved.binding.kind).toBe(NCK_POINTS_KIND);

    // 重新读取（模拟下一次快照加载）：两种结构严格分流，points 原样回来。
    expect(nckPointsOf(saved)).toEqual(edited);
    expect(selectionsOf(saved)).toEqual([]); // NCK points 不得被读成 ResourceSelection
    const parsedAgain = parseTaskBinding(saved);
    expect(parsedAgain.ok).toBe(true);
    if (parsedAgain.ok) expect(parsedAgain.kind).toBe(NCK_POINTS_KIND);

    // 保存后的任务仍可再次作为唯一候选被编辑（不是一次性的）。
    const again = selectEditableTask({ tasks: result.tasks, driverId: NCK_DRIVER });
    expect(again.ok).toBe(true);
    if (again.ok) expect(again.editable?.id).toBe("nck-task");
  });

  it("非 poll Schedule（subscribe）逐字保留，不被编辑器翻成 poll", () => {
    const existing = nckTask(
      "nck-task",
      [point("var.collect.01", { channel: 1 })],
      { mode: "subscribe" },
    );
    const result = mergeNckAcquisitionTasks(
      submission({ tasks: [existing], points: [point("var.collect.01", { channel: 2 })] }),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.tasks[0].schedule).toEqual({ mode: "subscribe" });
  });
});

describe("P0-2 绝不额外生成旧 kind 任务（A19/A29）", () => {
  it("已有 NCK 任务时保存只产出 nck.points.v1，payload 中无 mesa.resources.v1", () => {
    const existing = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const result = mergeNckAcquisitionTasks(
      submission({ tasks: [existing], points: [point("var.collect.02", { axis: 1 })] }),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.tasks.map((t) => t.binding.kind)).toEqual([NCK_POINTS_KIND]);
    expect(JSON.stringify(result.tasks)).not.toContain(CANONICAL_RESOURCES_KIND);
    expect(result.tasks).toHaveLength(1); // 没有「再追加一个旧 kind 任务」
  });

  it("旧 merge 路径遇到 NCK 任务时**显式抛错**（第二批 A 已移除 no-op 安全网）", () => {
    const existing = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    // 旧实现"原样返回 existing"——调用方随后 PUT 成功并提示"点位已保存"，
    // 编辑其实没写进去（虚假保存成功）。现在改为抛错：用错入口必须立刻暴露。
    expect(() =>
      mergeAcquisitionTasks([existing], {
        interval_ms: 1000,
        selections: [{ resource_id: "variable", parameters: {}, outputs: [] }],
      }),
    ).toThrow(/nck\.points\.v1/);
  });

  it("kind 由 Endpoint Driver 决定；结构与 Driver 不匹配时装配直接拒绝", () => {
    expect(bindingKindForDriver(NCK_DRIVER)).toBe(NCK_POINTS_KIND);
    expect(bindingKindForDriver("s7")).toBe(CANONICAL_RESOURCES_KIND);
    expect(bindingKindForDriver("")).toBe(CANONICAL_RESOURCES_KIND);

    const nck = assembleBinding({
      driverId: NCK_DRIVER,
      content: { type: "nck-points", points: [point("var.collect.01", { channel: 1 })] },
    });
    expect(nck.ok).toBe(true);
    if (nck.ok) expect(nck.binding.kind).toBe(NCK_POINTS_KIND);

    // NCK + selections（旧结构）必须拒绝，而不是写一条必然被 configure 拒的任务。
    const mismatch = assembleBinding({
      driverId: NCK_DRIVER,
      content: { type: "resources", selections: [] },
    });
    expect(mismatch.ok).toBe(false);
    if (!mismatch.ok) expect(mismatch.code).toBe("DRIVER_BINDING_KIND_MISMATCH");

    // 非 NCK + points 同样拒绝（不得把 NCK 结构写给其它驱动）。
    const reverse = assembleBinding({
      driverId: "s7",
      content: { type: "nck-points", points: [] },
    });
    expect(reverse.ok).toBe(false);
  });
});

describe("P0-2 歧义/非法/快照未就绪：拒绝而不是猜", () => {
  it("多个可编辑候选（两个 NCK 任务）⇒ MULTIPLE_EDITABLE_TASKS", () => {
    const tasks = [
      nckTask("nck-a", [point("var.collect.01", { channel: 1 })]),
      nckTask("nck-b", [point("var.collect.02", { axis: 1 })]),
    ];
    const selected = selectEditableTask({ tasks, driverId: NCK_DRIVER });
    expect(selected.ok).toBe(false);
    if (!selected.ok) expect(selected.code).toBe("MULTIPLE_EDITABLE_TASKS");

    const result = validateNckTaskSubmission(
      submission({ tasks, points: [point("var.collect.01", { channel: 2 })] }),
    );
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.code).toBe("MULTIPLE_EDITABLE_TASKS");
  });

  it("Binding 结构非法（把 selections 塞进 nck kind / 点位缺字段）⇒ INVALID_BINDING_STRUCTURE", () => {
    const wrongShape: AcquisitionTaskShape = {
      id: "nck-bad",
      schedule: { mode: "poll", interval_ms: 1000 },
      binding: {
        kind: NCK_POINTS_KIND,
        // 旧结构冒充 NCK：不得被当成 ResourceSelection[] 放过。
        config: { selections: [{ resource_id: "variable", parameters: {}, outputs: [] }] },
      },
    };
    const parsed = parseTaskBinding(wrongShape);
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.code).toBe("INVALID_BINDING_STRUCTURE");

    const badPoint: AcquisitionTaskShape = {
      id: "nck-bad-point",
      schedule: { mode: "poll", interval_ms: 1000 },
      binding: {
        kind: NCK_POINTS_KIND,
        config: { points: [{ kind: "nck_variable", parameters: {} }] }, // 缺 point_key
      },
    };
    const result = validateNckTaskSubmission(
      submission({ tasks: [badPoint], points: [point("var.collect.01", { channel: 1 })] }),
    );
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.code).toBe("INVALID_BINDING_STRUCTURE");

    // 点位多一个字段：Core 的 deny_unknown_fields 会拒，前端提前拒绝。
    const extraField: AcquisitionTaskShape = {
      id: "nck-extra",
      schedule: { mode: "poll", interval_ms: 1000 },
      binding: {
        kind: NCK_POINTS_KIND,
        config: {
          points: [
            { kind: "nck_variable", point_key: "k", parameters: {}, identity: { template: "x" } },
          ],
        },
      },
    };
    const extraResult = parseTaskBinding(extraField);
    expect(extraResult.ok).toBe(false);
    if (!extraResult.ok) expect(extraResult.code).toBe("INVALID_POINT_STRUCTURE");
  });

  it("快照未就绪 / 归属不符 ⇒ SNAPSHOT_NOT_READY（不以空快照覆盖）", () => {
    for (const state of ["idle", "loading", "error"] as const) {
      const result = validateNckTaskSubmission(
        submission({ snapshotState: state, points: [point("var.collect.01", { channel: 1 })] }),
      );
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe("SNAPSHOT_NOT_READY");
    }
    const stale = validateNckTaskSubmission(
      submission({
        loadedEndpointId: "ep-other",
        points: [point("var.collect.01", { channel: 1 })],
      }),
    );
    expect(stale.ok).toBe(false);
    if (!stale.ok) expect(stale.code).toBe("SNAPSHOT_NOT_READY");
  });

  it("跨 kind 共存：编辑 NCK 任务不误伤 canonical 任务，顺序与内容逐字保留", () => {
    const canonical = canonicalTask("res-task", ["legacy.key.1"]);
    const nck = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const beforeCanonical = JSON.parse(JSON.stringify(canonical));
    const result = mergeNckAcquisitionTasks(
      submission({ tasks: [canonical, nck], points: [point("var.collect.02", { axis: 1 })] }),
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.tasks).toHaveLength(2);
    expect(result.tasks[0]).toEqual(beforeCanonical);
    expect(result.tasks[0]).toBe(canonical); // 引用也未变（调用方不必重建）
    expect(result.tasks[1].id).toBe("nck-task");
    expect(selectionsOf(result.tasks[0])).toHaveLength(1); // canonical 仍按旧结构读
  });
});

describe("P0-2 点位身份冲突：前端提交前唯一性校验", () => {
  it("候选 point_key 与其它任务（含旧 kind 任务）冲突 ⇒ POINT_KEY_CONFLICT", () => {
    const canonical = canonicalTask("res-task", ["var.collect.01@channel=1"]);
    const nck = nckTask("nck-task", [point("var.collect.01", { channel: 2 })]);
    const result = validateNckTaskSubmission(
      submission({
        tasks: [canonical, nck],
        points: [point("var.collect.01", { channel: 1 })], // 与 canonical 的 key 相同
      }),
    );
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.code).toBe("POINT_KEY_CONFLICT");
      expect(result.message).toContain("var.collect.01@channel=1");
    }
  });

  it("本次提交内部重复 point_key ⇒ POINT_KEY_CONFLICT（不静默去重）", () => {
    const p = point("var.collect.01", { channel: 1 });
    const result = validateNckTaskSubmission(submission({ tasks: [], points: [p, p] }));
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.code).toBe("POINT_KEY_CONFLICT");
  });

  it("唯一性枚举跨两种 kind（Core 的命名空间口径）", () => {
    const canonical = canonicalTask("res-task", ["legacy.key.1", "legacy.key.2"]);
    const nck = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const owners = collectEndpointPointKeys([canonical, nck]);
    expect(owners.map((o) => o.point_key).sort()).toEqual([
      "legacy.key.1",
      "legacy.key.2",
      "var.collect.01@channel=1",
    ]);
  });
});

describe("P0-2 提交模型执行 P0-1 准入（绕过 UI 也拒绝）", () => {
  it("identity 定义 ⇒ SCOPE_NOT_COLLECTABLE；定义缺失 ⇒ DEFINITION_MISSING", () => {
    const identity = validateNckTaskSubmission(
      submission({ points: [point("var.identity.01", { axis: 1 })] }),
    );
    expect(identity.ok).toBe(false);
    if (!identity.ok) expect(identity.code).toBe("SCOPE_NOT_COLLECTABLE");

    const missing = validateNckTaskSubmission(
      submission({ points: [point("var.unknown.99", { axis: 1 })] }),
    );
    expect(missing.ok).toBe(false);
    if (!missing.ok) expect(missing.code).toBe("DEFINITION_MISSING");

    // 证明「合法 Key + 完整参数」不足以放行 identity。
    expect(point("var.identity.01", { axis: 1 }).point_key).toBe("var.identity.01@axis=1");
    const allowed = validateNckTaskSubmission(
      submission({ points: [point("var.collect.01", { channel: 1 })] }),
    );
    expect(allowed.ok).toBe(true);
  });

  it("nck_native 点位不适用 scope 准入，但仍校验信封", () => {
    const native: NckPointSelection = {
      kind: "nck_native",
      point_key: "native.channel.1.nck.variable.0",
      parameters: { area: "channel", area_no: 1, module: 1, column: 0, line: 0 },
    };
    const ok = validateNckTaskSubmission(submission({ points: [native] }));
    expect(ok.ok).toBe(true);

    const bad = validateNckTaskSubmission(
      submission({ points: [{ ...native, point_key: "" }] }),
    );
    expect(bad.ok).toBe(false);
    if (!bad.ok) expect(bad.code).toBe("INVALID_POINT_STRUCTURE");
  });
});
