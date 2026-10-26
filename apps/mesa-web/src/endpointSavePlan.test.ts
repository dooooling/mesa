// PR-C 第二批 A 回归：保存计划按 Driver 分流（P0 数据安全）。
//
// 覆盖评审要求的关键回归：
// 1. 已有 NCK Task 后新增点位 ⇒ 更新**正确任务**、不追加旧 Binding；
// 2. 已有 NCK Task 后编辑点位 ⇒ ID/Schedule 保留、保存后可重新读取；
// 3. NCK 快照未就绪 ⇒ 禁止保存（不产出可 PUT 的任务集）；
// 4. Binding 不合法或存在歧义 ⇒ 拒绝保存、不覆盖原任务；
// 5. 无实际编辑变化 ⇒ 不报告成功（NO_CHANGES，UI 据此不发 PUT）；
// 6. 非 NCK 路径与旧 `mergeAcquisitionTasks` 逐字一致（零行为变化）；
// 7. 绕过 UI 注入 identity 点位 ⇒ 提交模型仍拒绝。
//
// fixture 全部为合成数据，不搬运 Driver 的真实定义（42 条真实数据由 Driver 侧测试锁定）。
import { describe, expect, it } from "vitest";
import {
  CANONICAL_RESOURCES_KIND,
  NCK_POINTS_KIND,
  mergeAcquisitionTasks,
  type AcquisitionTaskShape,
  type NckPointSelection,
  type ResourceSelection,
} from "./deviceModel";
import { nckPointsOf } from "./nckTaskBinding";
import {
  NO_CHANGES_MESSAGE,
  planEndpointTaskSave,
  samePoints,
  sameSubmittedConfig,
  type EndpointSavePlanInput,
} from "./endpointSavePlan";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-nck";

/** 定义目录 fixture（只用于 scope 准入判定）。 */
const DEFINITIONS = {
  "var.collect.01": { variable_id: "var.collect.01", scope: "collect" },
  "var.collect.02": { variable_id: "var.collect.02", scope: "collect" },
  "var.identity.01": { variable_id: "var.identity.01", scope: "identity" },
  "var.no.scope": { variable_id: "var.no.scope" },
} as const;

function point(
  variableId: string,
  target: Record<string, number>,
  pointKey?: string,
): NckPointSelection {
  return {
    kind: "nck_variable",
    point_key:
      pointKey ??
      `${variableId}@${Object.entries(target)
        .map(([k, v]) => `${k}=${v}`)
        .join(",")}`,
    parameters: { variable_id: variableId, definition_revision: 1, target, arguments: {} },
  };
}

function nckTask(
  id: string,
  points: NckPointSelection[],
  schedule: AcquisitionTaskShape["schedule"] = { mode: "poll", interval_ms: 1000 },
): AcquisitionTaskShape {
  return { id, schedule, binding: { kind: NCK_POINTS_KIND, config: { points } } };
}

function canonicalTask(
  id: string,
  pointKeys: string[],
  schedule: AcquisitionTaskShape["schedule"] = { mode: "poll", interval_ms: 1000 },
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

function base(overrides: Partial<EndpointSavePlanInput> = {}): EndpointSavePlanInput {
  return {
    driverId: NCK_DRIVER,
    tasks: [],
    snapshotState: "ready",
    loadedEndpointId: EP,
    currentEndpointId: EP,
    intervalMs: 1000,
    selections: [],
    nckPoints: [],
    nckDefinitions: DEFINITIONS,
    ...overrides,
  };
}

describe("关键回归 1：已有 NCK Task 后新增点位", () => {
  it("existing_nck_task_with_new_point_updates_same_task_without_legacy_binding", () => {
    const existing = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const plan = planEndpointTaskSave(
      base({
        tasks: [existing],
        nckPoints: [
          point("var.collect.01", { channel: 1 }),
          point("var.collect.02", { axis: 2, channel: 1 }),
        ],
      }),
    );
    expect(plan.ok).toBe(true);
    if (!plan.ok || plan.driverKind !== "nck") return;

    expect(plan.taskId).toBe("nck-task");
    expect(plan.tasks).toHaveLength(1);
    // 正确任务被更新，而不是"再加一个 mesa.resources.v1 任务"。
    expect(plan.tasks[0].id).toBe("nck-task");
    expect(plan.tasks[0].binding.kind).toBe(NCK_POINTS_KIND);
    expect(JSON.stringify(plan.tasks)).not.toContain(CANONICAL_RESOURCES_KIND);
    expect(plan.points).toHaveLength(2);
    expect(plan.preservedCount).toBe(0);
  });
});

describe("关键回归 2：已有 NCK Task 后编辑点位", () => {
  it("existing_nck_task_edit_keeps_id_and_schedule_and_is_rereadable", () => {
    const existing = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const edited = [point("var.collect.01", { channel: 7 })];
    const plan = planEndpointTaskSave(
      base({ tasks: [existing], nckPoints: edited, intervalMs: 250 }),
    );
    expect(plan.ok).toBe(true);
    if (!plan.ok || plan.driverKind !== "nck") return;

    const saved = plan.tasks[0];
    // ID 与 poll Schedule 保留（周期写编辑器值，任务不被重建）。
    expect(saved.id).toBe("nck-task");
    expect(saved.schedule).toEqual({ mode: "poll", interval_ms: 250 });
    // 保存后可逐字重新读取（下一次快照加载会走同一解析路径）。
    expect(nckPointsOf(saved)).toEqual(edited);
    expect(samePoints(nckPointsOf(saved), plan.points)).toBe(true);
    // 再存一次（内容不变）必须落到 NO_CHANGES：证明上次的结果就是当前真值。
    const again = planEndpointTaskSave(
      base({ tasks: plan.tasks, nckPoints: edited, intervalMs: 250 }),
    );
    expect(again.ok).toBe(false);
    if (!again.ok) expect(again.code).toBe("NO_CHANGES");
  });
});

describe("关键回归 3：快照未就绪", () => {
  it("nck_snapshot_not_ready_never_plans_save", () => {
    for (const state of ["idle", "loading", "error"] as const) {
      const plan = planEndpointTaskSave(
        base({
          snapshotState: state,
          tasks: [nckTask("nck-task", [point("var.collect.01", { channel: 1 })])],
          nckPoints: [point("var.collect.02", { axis: 1 })],
        }),
      );
      expect(plan.ok).toBe(false);
      if (!plan.ok) expect(plan.code).toBe("SNAPSHOT_NOT_READY");
    }
    // 串 Endpoint 的旧快照同样拒绝。
    const stale = planEndpointTaskSave(
      base({
        loadedEndpointId: "ep-other",
        tasks: [nckTask("nck-task", [point("var.collect.01", { channel: 1 })])],
        nckPoints: [point("var.collect.02", { axis: 1 })],
      }),
    );
    expect(stale.ok).toBe(false);
    if (!stale.ok) expect(stale.code).toBe("SNAPSHOT_NOT_READY");
  });
});

describe("关键回归 4：Binding 不合法 / 存在歧义 ⇒ 拒绝且不覆盖", () => {
  it("ambiguous_or_invalid_nck_binding_refuses_save_without_overwrite", () => {
    const tasks = [
      nckTask("nck-a", [point("var.collect.01", { channel: 1 })]),
      nckTask("nck-b", [point("var.collect.02", { axis: 1 })]),
    ];
    const ambiguous = planEndpointTaskSave(
      base({ tasks, nckPoints: [point("var.collect.01", { channel: 2 })] }),
    );
    expect(ambiguous.ok).toBe(false);
    // 拒绝时不返回任务集：调用方拿不到任何可 PUT 的载荷。
    if (!ambiguous.ok) expect(ambiguous.code).toBe("MULTIPLE_EDITABLE_TASKS");
    expect(Object.prototype.hasOwnProperty.call(ambiguous, "tasks")).toBe(false);

    const malformed: AcquisitionTaskShape = {
      id: "nck-bad",
      schedule: { mode: "poll", interval_ms: 1000 },
      binding: {
        // 旧结构冒充 NCK：不得被当成 ResourceSelection[] 放过。
        kind: NCK_POINTS_KIND,
        config: { selections: [{ resource_id: "variable", parameters: {}, outputs: [] }] },
      },
    };
    const invalid = planEndpointTaskSave(
      base({ tasks: [malformed], nckPoints: [point("var.collect.01", { channel: 1 })] }),
    );
    expect(invalid.ok).toBe(false);
    if (!invalid.ok) expect(invalid.code).toBe("INVALID_BINDING_STRUCTURE");
  });
});

describe("关键回归 5：无实际编辑变化（消除虚假保存成功）", () => {
  it("no_actual_change_never_reports_success", () => {
    const existing = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const plan = planEndpointTaskSave(
      base({ tasks: [existing], nckPoints: [point("var.collect.01", { channel: 1 })] }),
    );
    expect(plan.ok).toBe(false);
    if (plan.ok) return;
    expect(plan.code).toBe("NO_CHANGES");
    expect(plan.message).toContain("未发送保存请求");
    // 关键：没有任何成功载荷可被误当成"已保存"。
    expect(Object.prototype.hasOwnProperty.call(plan, "tasks")).toBe(false);
  });

  it("changing_only_the_interval_is_a_real_edit", () => {
    const existing = nckTask("nck-task", [point("var.collect.01", { channel: 1 })]);
    const plan = planEndpointTaskSave(
      base({ tasks: [existing], nckPoints: [point("var.collect.01", { channel: 1 })], intervalMs: 500 }),
    );
    expect(plan.ok).toBe(true);
    if (plan.ok && plan.driverKind === "nck") {
      expect(plan.tasks[0].schedule).toEqual({ mode: "poll", interval_ms: 500 });
    }
  });
});

describe("非 NCK：零行为变化（与旧 merge 逐字一致）", () => {
  it("non_nck_plan_is_identical_to_legacy_merge", () => {
    const existing = [
      canonicalTask("task-a", ["legacy.k1"]),
      { id: "task-b", schedule: { mode: "subscribe" }, binding: { kind: "custom.v0", config: {} } },
    ] as AcquisitionTaskShape[];
    const selections: ResourceSelection[] = [
      { resource_id: "register", parameters: {}, outputs: [{ output: "value", point_key: "legacy.k2" }] },
    ];
    const plan = planEndpointTaskSave(
      base({ driverId: "s7", tasks: existing, selections, nckPoints: undefined }),
    );
    expect(plan.ok).toBe(true);
    if (!plan.ok) return;
    const legacy = mergeAcquisitionTasks(existing, { interval_ms: 1000, selections });
    expect(plan.tasks).toEqual(legacy);
    expect(plan.driverKind).toBe("canonical");
    expect(plan.preservedCount).toBe(1);
    // 保留任务逐字不动（引用也未变）：旧实现的顺序是 [...preserved, canonical]，
    // 这里连顺序一起对齐，证明"非 NCK 零行为变化"。
    expect(plan.tasks[0]).toBe(existing[1]);
    expect(plan.tasks[1].id).toBe("task-a");
  });

  it("non_nck_empty_selection_keeps_legacy_message", () => {
    const plan = planEndpointTaskSave(base({ driverId: "s7", tasks: [], selections: [] }));
    expect(plan.ok).toBe(false);
    if (!plan.ok) {
      expect(plan.code).toBe("EMPTY_SELECTION");
      expect(plan.message).toBe("请先加入点位");
    }
  });

  it("non_nck_with_nck_task_is_explicitly_rejected_instead_of_fake_success", () => {
    // 旧实现：mergeAcquisitionTasks no-op → PUT 成功 → UI 报"点位已保存"（编辑没写进去）。
    const plan = planEndpointTaskSave(
      base({
        driverId: "s7",
        tasks: [nckTask("nck-task", [point("var.collect.01", { channel: 1 })])],
        selections: [{ resource_id: "r", parameters: {}, outputs: [{ output: "o", point_key: "k" }] }],
      }),
    );
    expect(plan.ok).toBe(false);
    if (!plan.ok) {
      expect(plan.code).toBe("DRIVER_KIND_MISMATCH");
      expect(plan.message).toContain(NCK_POINTS_KIND);
    }
  });

  it("non_nck_snapshot_gate_is_unchanged", () => {
    const plan = planEndpointTaskSave(
      base({ driverId: "s7", snapshotState: "error", selections: [] }),
    );
    expect(plan.ok).toBe(false);
    if (!plan.ok) expect(plan.code).toBe("SNAPSHOT_NOT_READY");
  });
});

describe("关键回归 7：绕过 UI 注入 identity 点位", () => {
  it("bypass_ui_identity_point_is_rejected_by_submit_model", () => {
    const identity = point("var.identity.01", { axis: 1 });
    // Key 看上去完全合法（身份约束不构成准入依据）。
    expect(identity.point_key).not.toBe("");
    const plan = planEndpointTaskSave(base({ tasks: [], nckPoints: [identity] }));
    expect(plan.ok).toBe(false);
    if (!plan.ok) expect(plan.code).toBe("SCOPE_NOT_COLLECTABLE");

    // 缺少 scope 的条目同样 fail-closed（不得反写成"非 identity 即放行"）。
    const noScope = planEndpointTaskSave(
      base({ tasks: [], nckPoints: [point("var.no.scope", { axis: 1 })] }),
    );
    expect(noScope.ok).toBe(false);
    if (!noScope.ok) expect(noScope.code).toBe("SCOPE_MISSING");

    // 目录里没有该变量 ⇒ 无法确认 scope ⇒ 拒绝。
    const unknown = planEndpointTaskSave(
      base({ tasks: [], nckPoints: [point("var.unknown.99", { axis: 1 })] }),
    );
    expect(unknown.ok).toBe(false);
    if (!unknown.ok) expect(unknown.code).toBe("DEFINITION_MISSING");
  });

  it("missing_definition_catalog_fails_closed", () => {
    const plan = planEndpointTaskSave(
      base({
        tasks: [],
        nckPoints: [point("var.collect.01", { channel: 1 })],
        nckDefinitions: null,
      }),
    );
    expect(plan.ok).toBe(false);
    if (!plan.ok) expect(plan.code).toBe("NCK_DEFINITIONS_MISSING");
  });
});

describe("点位身份冲突：与其它任务（含跨 kind）逐层拒绝", () => {
  it("nck_point_key_conflict_with_other_task_is_rejected", () => {
    const preserved = canonicalTask("res-task", ["var.collect.01@channel=1"]);
    const plan = planEndpointTaskSave(
      base({
        tasks: [preserved, nckTask("nck-task", [point("var.collect.02", { axis: 1 })])],
        nckPoints: [point("var.collect.01", { channel: 1 })],
      }),
    );
    expect(plan.ok).toBe(false);
    if (!plan.ok) {
      expect(plan.code).toBe("POINT_KEY_CONFLICT");
      expect(plan.message).toContain("var.collect.01@channel=1");
    }
  });
});

// ---------------------------------------------------------------------------
// 第 1 步（状态与保存流程）：无实际修改统一拦截 + 完整配置回读比对
// ---------------------------------------------------------------------------

describe("决定 2：无实际修改 —— 非 NCK 也必须强制拦截（不再只是信息）", () => {
  it("non_nck_no_change_is_rejected_with_NO_CHANGES", () => {
    const existing = canonicalTask("task-a", ["legacy.k1"]);
    const selections: ResourceSelection[] = [
      { resource_id: "variable", parameters: {}, outputs: [{ output: "out-0", point_key: "legacy.k1" }] },
    ];
    const plan = planEndpointTaskSave(
      base({ driverId: "s7", tasks: [existing], selections, nckPoints: undefined }),
    );
    // 旧实现：ok + changed:false（仅信息）⇒ save() 照发 PUT，把同一份配置整份重写。
    expect(plan.ok).toBe(false);
    if (plan.ok) return;
    expect(plan.code).toBe("NO_CHANGES");
    expect(plan.message).toBe(NO_CHANGES_MESSAGE);
    expect(plan.message).toContain("未发送保存请求");
    // 拒绝时不产出任何可 PUT 的载荷。
    expect(Object.prototype.hasOwnProperty.call(plan, "tasks")).toBe(false);
  });

  it("non_nck_interval_only_change_is_a_real_edit", () => {
    // 点位完全相同、只有采集周期变了：必须被视为真实修改（否则会漏保存周期）。
    const existing = canonicalTask("task-a", ["legacy.k1"]);
    const selections: ResourceSelection[] = [
      { resource_id: "variable", parameters: {}, outputs: [{ output: "out-0", point_key: "legacy.k1" }] },
    ];
    const plan = planEndpointTaskSave(
      base({
        driverId: "s7",
        tasks: [existing],
        selections,
        nckPoints: undefined,
        intervalMs: 500,
      }),
    );
    expect(plan.ok).toBe(true);
    if (!plan.ok) return;
    expect(plan.changed).toBe(true);
    const editable = plan.tasks.find((t) => t.id === "task-a");
    expect(editable?.schedule).toEqual({ mode: "poll", interval_ms: 500 });
  });

  it("task_array_order_is_not_a_change（服务端按 id 排序返回）", () => {
    // GET /tasks 按 task id 升序（config-store `ORDER BY id`）：加载顺序与
    // merge 产出的顺序不同，不得被判成「有修改」而多发一次 PUT。
    const a = canonicalTask("task-a", ["k1"]);
    const z = { id: "task-z", schedule: { mode: "subscribe" }, binding: { kind: "custom.v0", config: {} } } as AcquisitionTaskShape;
    const selections: ResourceSelection[] = [
      { resource_id: "variable", parameters: {}, outputs: [{ output: "out-0", point_key: "k1" }] },
    ];
    const plan = planEndpointTaskSave(
      base({ driverId: "s7", tasks: [a, z], selections, nckPoints: undefined }),
    );
    // 无修改：即便 merge 会把 canonical 放到末尾，也必须是 NO_CHANGES。
    expect(plan.ok).toBe(false);
    if (!plan.ok) expect(plan.code).toBe("NO_CHANGES");
  });
});

describe("决定 3：回读比对覆盖完整提交配置（周期 + 点位参数 + 保留任务）", () => {
  const submitted = [
    nckTask("nck-task", [point("var.collect.01", { channel: 1 }), point("var.collect.02", { axis: 2 })], {
      mode: "poll",
      interval_ms: 500,
    }),
    { id: "task-b", schedule: { mode: "subscribe" }, binding: { kind: "custom.v0", config: { x: 1 } } } as AcquisitionTaskShape,
  ];

  it("完全一致（任务数组顺序不同也算一致）", () => {
    // 服务端按 id 升序返回：task-b 在 nck-task 之前。
    expect(sameSubmittedConfig([submitted[1], submitted[0]], submitted)).toBe(true);
  });

  it("只比点位会漏掉的情形：点位相同、周期不同 ⇒ 不一致", () => {
    const readBack = [
      nckTask("nck-task", [point("var.collect.01", { channel: 1 }), point("var.collect.02", { axis: 2 })], {
        mode: "poll",
        interval_ms: 1000, // 周期没写进去
      }),
      submitted[1],
    ];
    // 点位本身完全一样（旧实现只比点位 ⇒ 会误报成功）。
    expect(samePoints(nckPointsOf(readBack[0]), nckPointsOf(submitted[0]))).toBe(true);
    expect(sameSubmittedConfig(readBack, submitted)).toBe(false);
  });

  it("点位参数变化 / 顺序变化 / 保留任务被改 ⇒ 不一致", () => {
    const changedParam = [
      nckTask("nck-task", [point("var.collect.01", { channel: 2 }), point("var.collect.02", { axis: 2 })], {
        mode: "poll",
        interval_ms: 500,
      }),
      submitted[1],
    ];
    expect(sameSubmittedConfig(changedParam, submitted)).toBe(false);

    const reordered = [
      nckTask("nck-task", [point("var.collect.02", { axis: 2 }), point("var.collect.01", { channel: 1 })], {
        mode: "poll",
        interval_ms: 500,
      }),
      submitted[1],
    ];
    expect(sameSubmittedConfig(reordered, submitted)).toBe(false);

    const preservedChanged = [
      submitted[0],
      { id: "task-b", schedule: { mode: "subscribe" }, binding: { kind: "custom.v0", config: { x: 2 } } } as AcquisitionTaskShape,
    ];
    expect(sameSubmittedConfig(preservedChanged, submitted)).toBe(false);
  });

  it("多一个 / 少一个任务 ⇒ 不一致", () => {
    expect(sameSubmittedConfig([submitted[0]], submitted)).toBe(false);
    expect(
      sameSubmittedConfig([...submitted, { id: "task-c", schedule: { mode: "poll", interval_ms: 1 }, binding: { kind: "custom.v0", config: {} } } as AcquisitionTaskShape], submitted),
    ).toBe(false);
  });
});
