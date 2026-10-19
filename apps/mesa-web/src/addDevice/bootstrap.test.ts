// M5.5 原子提交单测：一次 POST + 结果映射 + 幂等键稳定。
// RC2 修3：幂等键语义改为 per-operation（调用方持有，重试复用；新 Flow 新 key）。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { bootstrapDevice, isDraftComplete, newOperationKey, type BootstrapClient } from "./bootstrap";
import { CANONICAL_RESOURCES_KIND, NCK_POINTS_KIND, mergeAcquisitionTasks } from "../deviceModel";
import type { EndpointSavePlanInput } from "../endpointSavePlan";

// PR-D：记录保存计划的入参，用来钉死「`tasks: [] + snapshotState: "ready"` 只允许
// 出现在 Bootstrap 原子创建分支」这一 P0 约束（行为断言之外的结构证据）。
const planCalls: EndpointSavePlanInput[] = [];
vi.mock("../endpointSavePlan", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../endpointSavePlan")>();
  return {
    ...actual,
    planEndpointTaskSave: (input: EndpointSavePlanInput) => {
      planCalls.push(input);
      return actual.planEndpointTaskSave(input);
    },
  };
});

beforeEach(() => {
  planCalls.length = 0;
});

const DRAFT = {
  device: { deviceId: "cnc-01", deviceName: "CNC-01" },
  connection: { driverId: "s7", endpointId: "s7-ep", endpointName: "S7", connection: {} },
  acquisition: { intervalMs: 1000, selections: [{ resource_id: "r1", parameters: {}, outputs: [] }], startAfterCreate: true },
};

function client(body: Record<string, unknown>, status = 201): BootstrapClient & { calls: unknown[] } {
  const calls: unknown[] = [];
  return {
    calls,
    deviceBootstrap: vi.fn(async (b) => {
      calls.push(b);
      return { status, body };
    }),
  };
}

describe("isDraftComplete", () => {
  it("设备与连接齐全才可提交；采集点可以为空（C3-D 离线保存）", () => {
    expect(isDraftComplete({ device: null, connection: null, acquisition: null })).toBe(false);
    expect(isDraftComplete({ ...DRAFT, device: { deviceId: "", deviceName: "" } } as never)).toBe(false);
    expect(isDraftComplete({ device: DRAFT.device, connection: null, acquisition: DRAFT.acquisition })).toBe(false);
    // C3-D：零采集点**不再**阻断提交（跳过采集点不得绕过设备/连接两道硬前置）
    expect(
      isDraftComplete({ device: DRAFT.device, connection: DRAFT.connection, acquisition: { ...DRAFT.acquisition, selections: [] } }),
    ).toBe(true);
    // 但没有 acquisition 草稿（数据步未提交）仍然不可提交
    expect(isDraftComplete({ device: DRAFT.device, connection: DRAFT.connection, acquisition: null })).toBe(false);
    expect(isDraftComplete(DRAFT)).toBe(true);
  });
});

describe("newOperationKey（per-operation 幂等）", () => {
  it("每次生成唯一（新 Flow 新 key，删后重建不再命中旧 replay）", () => {
    const k1 = newOperationKey();
    const k2 = newOperationKey();
    expect(k1.startsWith("web-op-")).toBe(true);
    expect(k2).not.toBe(k1);
  });

  it("同 Flow 重试复用 key（调用方传入 opts.idempotencyKey）", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "sim-ep", revision: 1, started: true });
    const key = newOperationKey();
    await bootstrapDevice(c, DRAFT, { idempotencyKey: key });
    await bootstrapDevice(c, DRAFT, { idempotencyKey: key });
    expect(c.calls).toHaveLength(2);
    const b0 = c.calls[0] as { idempotency_key?: unknown };
    const b1 = c.calls[1] as { idempotency_key?: unknown };
    expect(b0.idempotency_key).toBe(key);
    expect(b1.idempotency_key).toBe(key);
  });
});

describe("bootstrapDevice（原子接口）", () => {
  it("201：一次 POST，请求体含 device/endpoint/tasks/start/幂等键", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "s7-ep", revision: 1, started: true });
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(true);
    expect(r.failedStep).toBeNull();
    expect(r.replayed).toBe(false);
    expect(c.calls).toHaveLength(1);
    const b = c.calls[0] as Record<string, unknown>;
    expect(b).toMatchObject({
      device: { id: "cnc-01", name: "CNC-01" },
      endpoint: { id: "s7-ep", driver_id: "s7" },
      start: true,
    });
    expect(typeof (b as { idempotency_key?: unknown }).idempotency_key).toBe("string");
    const tasks = (b as { acquisition: { tasks: Array<{ id: string; binding: { kind: string } }> } }).acquisition.tasks;
    expect(tasks).toHaveLength(1);
    expect(tasks[0].binding.kind).toBe("mesa.resources.v1");
  });

  it("200 + replayed：幂等重放（未重复创建）", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "s7-ep", replayed: true }, 200);
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(true);
    expect(r.replayed).toBe(true);
  });

  it("START_FAILED：映射 start-endpoint + 后端已补偿", async () => {
    const c = client(
      { error: { code: "START_FAILED", message: "start boom" }, compensated: true },
      502,
    );
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("start-endpoint");
    expect(r.failedMessage).toBe("start boom");
    expect(r.compensated).toBe(true);
    expect(r.residual).toBe(false);
  });

  it("CONFLICT（device 已存在）：映射 create-device，无残留误报", async () => {
    const c = client({ error: { code: "CONFLICT", message: "device `cnc-01` 已存在" } }, 409);
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("create-device");
    expect(r.residual).toBe(false);
  });

  // -------------------------------------------------------------------------
  // C3-D：零采集点（仅保存）与「默认不启动」
  // -------------------------------------------------------------------------

  it("零采集点：acquisition.tasks 必须是空数组，且绝不生成空 selections 的假任务", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "s7-ep", revision: 1, started: false });
    const draft = {
      ...DRAFT,
      acquisition: { intervalMs: 1000, selections: [], startAfterCreate: false },
    };
    const r = await bootstrapDevice(c, draft);
    expect(r.ok).toBe(true);
    const b = c.calls[0] as { acquisition: { tasks: unknown[] }; start: unknown };
    // 后端契约：tasks=[] 表示「本次不建任务」（bootstrap.rs 的 !acq.tasks.is_empty() 分支）
    expect(b.acquisition.tasks).toEqual([]);
    expect(b.start).toBe(false);
  });

  it("零采集点 + 草稿里误置 start=true：仍然只发 start=false（不发自相矛盾的组合）", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "s7-ep", revision: 1, started: false });
    const draft = {
      ...DRAFT,
      acquisition: { intervalMs: 1000, selections: [], startAfterCreate: true },
    };
    await bootstrapDevice(c, draft);
    const b = c.calls[0] as { acquisition: { tasks: unknown[] }; start: unknown };
    expect(b.acquisition.tasks).toEqual([]);
    expect(b.start).toBe(false);
  });

  it("有采集点但未勾选启动：tasks 正常、start=false（默认不启动）", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "s7-ep", revision: 1, started: false });
    const draft = { ...DRAFT, acquisition: { ...DRAFT.acquisition, startAfterCreate: false } };
    await bootstrapDevice(c, draft);
    const b = c.calls[0] as { acquisition: { tasks: unknown[] }; start: unknown };
    expect(b.acquisition.tasks).toHaveLength(1);
    expect(b.start).toBe(false);
  });

  // -------------------------------------------------------------------------
  // P0-2：装配单真值（A28）+ NCK 绝不写旧 kind（A19/A29）
  // -------------------------------------------------------------------------

  it("A28：向导页与端点页的 binding 逐字相等（唯一装配入口）", async () => {
    const selections = [
      {
        resource_id: "r1",
        parameters: { address: 1 },
        outputs: [{ output: "value", point_key: "r1.value" }],
      },
    ];
    // 端点页路径（legacy canonical）：空任务集 + 编辑器周期/选择。
    const merged = mergeAcquisitionTasks([], { interval_ms: 1000, selections });
    const c = client({ device_id: "cnc-01", endpoint_id: "s7-ep", revision: 1, started: false });
    await bootstrapDevice(c, { ...DRAFT, acquisition: { ...DRAFT.acquisition, selections } });
    const b = c.calls[0] as { acquisition: { tasks: unknown[] } };
    expect(b.acquisition.tasks).toEqual(merged);
    expect((b.acquisition.tasks[0] as { binding: { kind: string } }).binding.kind).toBe(
      CANONICAL_RESOURCES_KIND,
    );
  });

  it("A19/A29：NCK 向导硬塞旧 selections ⇒ 装配拒绝、零请求、不写 mesa.resources.v1", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: true });
    const r = await bootstrapDevice(c, {
      ...DRAFT,
      connection: { ...DRAFT.connection, driverId: "sinumerik-nck", endpointId: "nck-ep" },
    });
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("put-tasks");
    expect(r.failedMessage).toContain(NCK_POINTS_KIND);
    expect(c.calls).toHaveLength(0); // 连 POST 都不发，绝不写无效任务
  });

  it("A29：NCK 零采集点仍发送 tasks=[]，不被误判成非法 binding", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: false });
    const r = await bootstrapDevice(c, {
      ...DRAFT,
      connection: { ...DRAFT.connection, driverId: "sinumerik-nck", endpointId: "nck-ep" },
      acquisition: { intervalMs: 1000, selections: [], startAfterCreate: true },
    });
    expect(r.ok).toBe(true);
    const b = c.calls[0] as { acquisition: { tasks: unknown[] }; start: unknown };
    expect(b.acquisition.tasks).toEqual([]);
    expect(b.start).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// PR-D：NCK 语义/原生点位 → planEndpointTaskSave → nck.points.v1
//
// 这些用例只走 mock client，不启动 mesad、不连接任何设备；请求体逐字断言，
// 证明向导没有第二套任务装配/第二条 HTTP 保存路径。
// ---------------------------------------------------------------------------

const NCK_DEFINITIONS = {
  "syn.collect.a": {
    variable_id: "syn.collect.a",
    revision: 1,
    family: "828d",
    scope: "collect",
  },
  "syn.identity.a": {
    variable_id: "syn.identity.a",
    revision: 1,
    family: "828d",
    scope: "identity",
  },
};

const SEMANTIC_POINT = {
  kind: "nck_variable",
  point_key: "syn.collect.a@channel=1",
  parameters: {
    variable_id: "syn.collect.a",
    definition_revision: 1,
    target: { channel: 1 },
    arguments: {},
  },
};

function nckDraft(acquisition: Partial<{
  nckPoints: Array<{ kind: string; point_key: string; parameters: Record<string, unknown> }>;
  nckDefinitions: typeof NCK_DEFINITIONS | null;
  selections: typeof DRAFT.acquisition.selections;
  startAfterCreate: boolean;
  intervalMs: number;
}>) {
  return {
    device: DRAFT.device,
    connection: {
      driverId: "sinumerik-nck",
      endpointId: "nck-ep",
      endpointName: "NCK",
      connection: { host: "10.0.0.5", family: "828d" },
    },
    acquisition: {
      intervalMs: 2000,
      selections: [],
      nckPoints: [SEMANTIC_POINT],
      nckDefinitions: NCK_DEFINITIONS,
      startAfterCreate: true,
      ...acquisition,
    },
  };
}

describe("PR-D：NCK 点位统一经 planEndpointTaskSave 装配", () => {
  it("nck_points_use_save_plan：请求体恰为 nck.points.v1 {points}，point_key 逐字等于预览值", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: true });
    const r = await bootstrapDevice(c, nckDraft({}));
    expect(r.ok).toBe(true);

    // 保存计划确实被调用，且入参是「Bootstrap 原子创建」专用的快照写法。
    expect(planCalls).toHaveLength(1);
    expect(planCalls[0]).toMatchObject({
      driverId: "sinumerik-nck",
      tasks: [],
      snapshotState: "ready",
      loadedEndpointId: "nck-ep",
      currentEndpointId: "nck-ep",
      intervalMs: 2000,
    });
    expect(planCalls[0].nckPoints).toEqual([SEMANTIC_POINT]);

    // 只有一次写请求；任务只来自 plan.tasks。
    expect(c.calls).toHaveLength(1);
    const b = c.calls[0] as {
      acquisition: { tasks: Array<Record<string, unknown>> };
      start: unknown;
    };
    expect(b.acquisition.tasks).toEqual([
      {
        id: "t1",
        schedule: { mode: "poll", interval_ms: 2000 },
        binding: { kind: NCK_POINTS_KIND, config: { points: [SEMANTIC_POINT] } },
      },
    ]);
    // 请求体**恰为** `{points}`，没有 selections 或其它字段混入。
    const binding = (b.acquisition.tasks[0] as { binding: { config: Record<string, unknown> } })
      .binding;
    expect(Object.keys(binding.config)).toEqual(["points"]);
    expect((binding.config.points as Array<{ point_key: string }>)[0].point_key).toBe(
      "syn.collect.a@channel=1",
    );
    expect(b.start).toBe(true);
  });

  it("nck_snapshot_ready_empty_tasks_is_bootstrap_only：非 NCK 路径不调用保存计划", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "s7-ep", revision: 1, started: false });
    await bootstrapDevice(c, DRAFT);
    // 非 NCK 仍走原有 assembleBinding；保存计划（及其空快照写法）不得被复用。
    expect(planCalls).toHaveLength(0);
  });

  it("identity_point_is_rejected_before_post：scope 无旁路，plan 拒绝且零 POST", async () => {
    const identityPoint = {
      kind: "nck_variable",
      point_key: "syn.identity.a@channel=9",
      parameters: {
        variable_id: "syn.identity.a",
        definition_revision: 1,
        target: { channel: 9 },
        arguments: {},
      },
    };
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: true });
    const r = await bootstrapDevice(c, nckDraft({ nckPoints: [identityPoint] }));
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("put-tasks");
    expect(r.failedMessage).toContain("SCOPE_NOT_COLLECTABLE");
    // 保存计划运行了（准入在模型层），但请求一个都不发。
    expect(planCalls).toHaveLength(1);
    expect(c.calls).toHaveLength(0);
  });

  it("nck_definitions_missing_fails_closed：目录不可用 ⇒ 拒绝提交且零 POST", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: true });
    const r = await bootstrapDevice(c, nckDraft({ nckDefinitions: null }));
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("put-tasks");
    expect(r.failedMessage).toContain("NCK_DEFINITIONS_MISSING");
    expect(c.calls).toHaveLength(0);
  });

  it("nck_duplicate_native_keys_rejected：原生点位冲突在 plan 层拒绝且零 POST", async () => {
    const nativePoint = {
      kind: "nck_native",
      point_key: "native.N.1.2.3.4",
      parameters: { area: "N", area_no: 1, module: 2, column: 3, line: 4 },
    };
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: true });
    const r = await bootstrapDevice(
      c,
      nckDraft({ nckPoints: [nativePoint, { ...nativePoint }] }),
    );
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("put-tasks");
    expect(r.failedMessage).toContain("POINT_KEY_CONFLICT");
    expect(c.calls).toHaveLength(0);
  });

  it("nck_zero_points_skips_save_plan：零点位不发 plan、tasks=[]、start=false（D9）", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: false });
    const r = await bootstrapDevice(c, nckDraft({ nckPoints: [], startAfterCreate: true }));
    expect(r.ok).toBe(true);
    expect(planCalls).toHaveLength(0);
    const b = c.calls[0] as { acquisition: { tasks: unknown[] }; start: unknown };
    expect(b.acquisition.tasks).toEqual([]);
    expect(b.start).toBe(false);
  });

  it("nck_legacy_selections_still_rejected：NCK + selections 仍 fail-closed（A19 不回退）", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "nck-ep", revision: 1, started: true });
    const r = await bootstrapDevice(
      c,
      nckDraft({ selections: DRAFT.acquisition.selections, nckPoints: [] }),
    );
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("put-tasks");
    expect(r.failedMessage).toContain(NCK_POINTS_KIND);
    expect(c.calls).toHaveLength(0);
  });
});
