// PR27 Device-first 模型单测：锁定 Device≈Endpoint 分离语义。
// 行为与协议语义由后端 contract 保证，这里只断言 Web 不再发明旧假设。
import { describe, expect, it } from "vitest";
import {
  applyEndpointChange,
  buildEndpointCreatePayload,
  buildEndpointUpdatePayload,
  canDeleteDevice,
  cleanConnection,
  deviceCounts,
  groupEndpointsByDevice,
  isDriverChangeAttempt,
  isRunningState,
  isStrictlyRunning,
  formatAge,
  formatPointValue,
  pointAgeMs,
  pointRowFingerprint,
  reconcilePointSnapshots,
  pointsSnapshotSignature,
  derivePointStale,
  isTaskSnapshotReady,
  mergeAcquisitionTasks,
  splitAcquisitionTasks,
  resolveEndpointContexts,
  suggestEndpointId,
  type AcquisitionTaskShape,
  type LifecycleStepResult,
} from "./deviceModel";

describe("groupEndpointsByDevice", () => {
  it("同一 Device 的多个 Endpoint 归入一组（Device A 下 3 个连接）", () => {
    const groups = groupEndpointsByDevice([
      { id: "a-nck", driver_id: "sinumerik-nck", device_id: "device-a" },
      { id: "a-plc", driver_id: "s7", device_id: "device-a" },
      { id: "a-opc", driver_id: "opcua", device_id: "device-a" },
      { id: "b-sim", driver_id: "simulator", device_id: "device-b" },
    ]);
    expect(groups.get("device-a")?.map((e) => e.id)).toEqual(["a-nck", "a-plc", "a-opc"]);
    expect(groups.get("device-b")?.map((e) => e.id)).toEqual(["b-sim"]);
  });

  it("device_id 缺失的 Endpoint 归入未归属组，不并入任一 Device", () => {
    const groups = groupEndpointsByDevice([{ id: "orphan", driver_id: "s7" }]);
    expect(groups.get("")?.map((e) => e.id)).toEqual(["orphan"]);
  });
});

describe("deviceCounts", () => {
  it("Device 数量与 Endpoint 数量分别计数（2 Device / 4 Endpoint）", () => {
    const c = deviceCounts(
      [
        { id: "device-a", name: "Device A" },
        { id: "device-b", name: "Device B" },
      ],
      [
        { id: "a-nck", driver_id: "sinumerik-nck", device_id: "device-a" },
        { id: "a-plc", driver_id: "s7", device_id: "device-a" },
        { id: "a-opc", driver_id: "opcua", device_id: "device-a" },
        { id: "b-sim", driver_id: "simulator", device_id: "device-b" },
      ],
    );
    expect(c).toEqual({ deviceCount: 2, endpointCount: 4 });
  });
});

describe("endpoint id", () => {
  it("建议 ID 不复用 device id（不同命名空间）", () => {
    for (let i = 0; i < 20; i++) {
      const id = suggestEndpointId("s7");
      expect(id.startsWith("s7-")).toBe(true);
      expect(id).not.toBe("device-a");
    }
  });

  it("创建请求体 device_id 固定为当前 Device，调用方无覆盖手段", () => {
    const p = buildEndpointCreatePayload({
      deviceId: "device-a",
      driverId: "s7",
      name: "PLC",
      connection: { host: "10.0.0.1" },
      id: "a-plc",
    });
    expect(p.device_id).toBe("device-a");
    expect(p.driver_id).toBe("s7");
    expect(p.id).toBe("a-plc");
    expect(p.name).toBe("PLC");
  });

  it("未给 id 时自动生成，不塌成 device id", () => {
    const p = buildEndpointCreatePayload({
      deviceId: "device-a",
      driverId: "s7",
      name: "PLC",
      connection: {},
    });
    expect(p.id).not.toBe("device-a");
    expect(p.id.startsWith("s7-")).toBe(true);
  });
});

describe("update payload", () => {
  it("修改请求体不含 driver_id（创建后不可改由类型保证）", () => {
    const p = buildEndpointUpdatePayload({
      name: "PLC-new",
      deviceId: "device-a",
      connection: { host: "10.0.0.2" },
    });
    expect(p).not.toHaveProperty("driver_id");
    expect(p.device_id).toBe("device-a");
  });

  it("isDriverChangeAttempt 拦截修改体里的 driver_id", () => {
    expect(isDriverChangeAttempt({ name: "x", driver_id: "opcua" })).toBe(true);
    expect(isDriverChangeAttempt({ name: "x", device_id: "device-a" })).toBe(false);
  });
});

describe("cleanConnection", () => {
  it("丢弃空值但保留 false / 0", () => {
    expect(
      cleanConnection({ a: "", b: undefined, c: null, d: false, e: 0, f: "x" }),
    ).toEqual({ d: false, e: 0, f: "x" });
  });

  it("Secret marker 原样保留（未触碰时后端按 marker-preserve 保留旧值）", () => {
    expect(cleanConnection({ password: { secret_set: true }, host: "x" })).toEqual({
      password: { secret_set: true },
      host: "x",
    });
  });

  it("clear 标记原样保留（后端按 clear 语义删除 Secret）", () => {
    expect(cleanConnection({ password: { clear_secret: true }, host: "x" })).toEqual({
      password: { clear_secret: true },
      host: "x",
    });
  });
});

describe("canDeleteDevice", () => {
  it("仍有 Endpoint 时前端预检不通过（最终以后端 RESTRICT 为准）", () => {
    const r = canDeleteDevice("device-a", [
      { id: "a-plc", driver_id: "s7", device_id: "device-a" },
    ]);
    expect(r.ok).toBe(false);
    expect(r.reason).toContain("先删除关联 Endpoint");
  });

  it("无归属 Endpoint 时允许删除", () => {
    expect(canDeleteDevice("device-a", []).ok).toBe(true);
  });
});

describe("isRunningState", () => {  it("RUNNING/CONNECTING/RECONNECTING 视为在线", () => {
    expect(isRunningState("RUNNING")).toBe(true);
    expect(isRunningState("reconnecting")).toBe(true);
    expect(isRunningState("STOPPED")).toBe(false);
    expect(isRunningState(undefined)).toBe(false);
  });

  it("isStrictlyRunning 只认 RUNNING（过渡态不算在线）", () => {
    expect(isStrictlyRunning("RUNNING")).toBe(true);
    expect(isStrictlyRunning("running")).toBe(true);
    expect(isStrictlyRunning("CONNECTING")).toBe(false);
    expect(isStrictlyRunning("RECONNECTING")).toBe(false);
    expect(isStrictlyRunning("STOPPED")).toBe(false);
    expect(isStrictlyRunning(undefined)).toBe(false);
  });
});

describe("resolveEndpointContexts", () => {
  const devices = [
    { id: "device-a", name: "CNC-01" },
    { id: "device-b", name: "Simulator" },
  ];
  it("endpoint 经 device_id 反查设备名（Device → Endpoint → Point）", () => {
    const ctx = resolveEndpointContexts(
      [
        { id: "a-nck", name: "NCK", device_id: "device-a" },
        { id: "b-sim", device_id: "device-b" },
      ],
      devices,
    );
    expect(ctx.get("a-nck")).toEqual({
      endpointId: "a-nck",
      endpointName: "NCK",
      deviceId: "device-a",
      deviceName: "CNC-01",
    });
    // endpoint 名缺失回落 id
    expect(ctx.get("b-sim")?.endpointName).toBe("b-sim");
    expect(ctx.get("b-sim")?.deviceName).toBe("Simulator");
  });

  it("Device 缺失时回落显示 device_id，不编造归属", () => {
    const ctx = resolveEndpointContexts([{ id: "x", device_id: "gone" }], devices);
    expect(ctx.get("x")?.deviceName).toBe("gone");
  });

  it("device_id 缺失时设备显示占位", () => {
    const ctx = resolveEndpointContexts([{ id: "orphan" }], devices);
    expect(ctx.get("orphan")).toMatchObject({ deviceId: "", deviceName: "—" });
  });
});

const canon = (id: string, extra?: Partial<AcquisitionTaskShape>): AcquisitionTaskShape => ({
  id,
  schedule: { mode: "poll", interval_ms: 1000 },
  binding: {
    kind: "mesa.resources.v1",
    config: { selections: [{ resource_id: "r", parameters: {}, outputs: [] }] },
  },
  ...extra,
});

const other = (id: string): AcquisitionTaskShape => ({
  id,
  schedule: { mode: "poll", interval_ms: 500 },
  binding: { kind: "driver.native.v1", config: { op: "scan" } },
});

import {
  reconcileEditableSelection,
  applySelectionAdd,
  effectiveResourceParameters,
  resourceInstanceKey,
  missingRequiredParams,
} from "./resourceSelectionModel";
import {
  nextStaleDeadlineMs,
  reconcilePointDelta,
  POINT_STALE_AFTER_MS,
} from "./deviceModel";

describe("selection reconciliation（冻结算法）", () => {
  const schemaOf = (resourceId: string) => ({
    fields:
      resourceId === "dynamic"
        ? [{ key: "axis", default: 1 }]
        : [],
  });
  const sel = (
    resource_id: string,
    parameters: Record<string, unknown>,
    outputs: Array<{ output: string; point_key: string }>,
  ) => ({ resource_id, parameters, outputs });

  it("终审 #1：{} / {axis:1} / {axis:undefined} effective identity 一致", () => {
    const eff = (p: Record<string, unknown>) =>
      resourceInstanceKey("dynamic", effectiveResourceParameters(schemaOf("dynamic"), p));
    expect(eff({})).toBe(eff({ axis: 1 }));
    expect(eff({ axis: undefined })).toBe(eff({}));
    // 且三者判成同一 exact duplicate
    const d = reconcileEditableSelection({
      candidate: sel("dynamic", { axis: undefined }, [{ output: "feed", point_key: "k" }]),
      editable: [sel("dynamic", {}, [{ output: "feed", point_key: "k" }])],
      protectedSelections: [],
      schemaOf,
    });
    expect(d.kind).toBe("duplicate-editable");
  });

  it("effective parameters：缺省经 defaults 物化后相等（{} ≡ {axis:1}）", () => {
    const d1 = reconcileEditableSelection({
      candidate: sel("dynamic", {}, [{ output: "feed", point_key: "dynamic.feed" }]),
      editable: [sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "dynamic.feed" }])],
      protectedSelections: [],
      schemaOf,
    });
    expect(d1.kind).toBe("duplicate-editable");
  });

  it("exact output 在 editable 即 duplicate（拒绝，不 merge 不 append）", () => {
    const d = reconcileEditableSelection({
      candidate: sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "dynamic.feed" }]),
      editable: [sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "dynamic.feed" }])],
      protectedSelections: [],
      schemaOf,
    });
    expect(d.kind).toBe("duplicate-editable");
  });

  it("exact output 在 protected 即拒绝（已在其他任务采集）", () => {
    const d = reconcileEditableSelection({
      candidate: sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "x" }]),
      editable: [],
      protectedSelections: [
        sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "dynamic.feed" }]),
      ],
      schemaOf,
    });
    expect(d.kind).toBe("duplicate-protected");
  });

  it("同 ResourceInstance 新 output → merge 进 editable 该项（protected 不动）", () => {
    const editable = [sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "dynamic.feed" }])];
    const prot = [sel("dynamic", { axis: 1 }, [{ output: "position.absolute", point_key: "p" }])];
    const d = reconcileEditableSelection({
      candidate: sel("dynamic", { axis: 1 }, [{ output: "spindle.speed", point_key: "s" }]),
      editable,
      protectedSelections: prot,
      schemaOf,
    });
    expect(d).toMatchObject({ kind: "merge", mergedIndex: 0 });
    // 调用方按 merge 执行后 protected 深相等（回归：只读任务永不被改）
    expect(prot).toEqual([
      sel("dynamic", { axis: 1 }, [{ output: "position.absolute", point_key: "p" }]),
    ]);
  });

  it("无同组 → append；point_key 冲突 → 拒绝（不碰后端 400）", () => {
    const app = reconcileEditableSelection({
      candidate: sel("status", {}, [{ output: "value", point_key: "k1" }]),
      editable: [],
      protectedSelections: [],
      schemaOf,
    });
    expect(app.kind).toBe("append");
    const conflict = reconcileEditableSelection({
      candidate: sel("status", {}, [{ output: "value", point_key: "k1" }]),
      editable: [sel("other", {}, [{ output: "value", point_key: "k1" }])],
      protectedSelections: [],
      schemaOf,
    });
    expect(conflict).toMatchObject({ kind: "point-key-conflict", pointKeys: ["k1"] });
  });

  it("preserved task 全程参与检测但深相等不变（核心回归）", () => {
    const prot = [
      sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "dynamic.feed" }]),
    ];
    const before = JSON.parse(JSON.stringify(prot));
    const decisions = [
      reconcileEditableSelection({
        candidate: sel("dynamic", { axis: 1 }, [{ output: "feed", point_key: "other" }]),
        editable: [],
        protectedSelections: prot,
        schemaOf,
      }),
      reconcileEditableSelection({
        candidate: sel("dynamic", { axis: 2 }, [{ output: "feed", point_key: "dynamic.feed" }]),
        editable: [],
        protectedSelections: prot,
        schemaOf,
      }),
      reconcileEditableSelection({
        candidate: sel("status", {}, [{ output: "value", point_key: "new" }]),
        editable: [],
        protectedSelections: prot,
        schemaOf,
      }),
    ];
    expect(decisions.map((d) => d.kind)).toEqual([
      "duplicate-protected",
      "point-key-conflict",
      "append",
    ]);
    expect(prot).toEqual(before);
  });

  it("绝不做值语义转换（'1' ≠ 1，大小写敏感）", () => {
    const d = reconcileEditableSelection({
      candidate: sel("r", { v: "1" }, [{ output: "o", point_key: "k" }]),
      editable: [sel("r", { v: 1 }, [{ output: "o", point_key: "k" }])],
      protectedSelections: [],
      schemaOf: () => ({ fields: [] }),
    });
    // 参数 JSON 不同 → 不是 exact duplicate；point_key 相同 → key 冲突
    expect(d.kind).toBe("point-key-conflict");
  });
});

describe("applySelectionAdd（父层共享实现，两调用方行为一致）", () => {
  // 终审指定回归：已有 register/value + 再加 register/raw
  // → sels.length === 1，outputs === [value, raw]（merge，不是 append 两项）。
  const regSchema = () => ({ fields: [] });
  const regSel = (
    params: Record<string, unknown>,
    outputs: Array<{ output: string; point_key: string }>,
  ) => ({ resource_id: "register", parameters: params, outputs });

  it("同实例新 output → merge 进同一 selection（sels 长度不变）", () => {
    const editable = [regSel({ unit: 3 }, [{ output: "value", point_key: "register.value" }])];
    const { next, message } = applySelectionAdd({
      candidate: regSel({ unit: 3 }, [{ output: "raw", point_key: "register.raw" }]),
      editable,
      protectedSelections: [],
      schemaOf: regSchema,
    });
    expect(message).toBeNull();
    expect(next).toHaveLength(1);
    expect(next[0].outputs.map((o) => o.output).sort()).toEqual(["raw", "value"]);
    // 输入数组不被原地修改（调用方 setSels(next) 语义）
    expect(editable[0].outputs).toHaveLength(1);
  });

  it("duplicate / conflict → 数组不变 + 返回中文原因", () => {
    const editable = [regSel({ unit: 3 }, [{ output: "value", point_key: "register.value" }])];
    const dup = applySelectionAdd({
      candidate: regSel({ unit: 3 }, [{ output: "value", point_key: "other" }]),
      editable,
      protectedSelections: [],
      schemaOf: regSchema,
    });
    expect(dup.next).toBe(editable);
    expect(dup.message).toMatch(/已在采集/);
    const conflict = applySelectionAdd({
      candidate: regSel({ unit: 9 }, [{ output: "raw", point_key: "register.value" }]),
      editable,
      protectedSelections: [],
      schemaOf: regSchema,
    });
    expect(conflict.next).toBe(editable);
    expect(conflict.message).toMatch(/point_key/);
  });
});

describe("missingRequiredParams（Web required preflight，表单完整性唯一判断）", () => {
  // 与 Core validate_instance REQUIRED 语义对齐：只看字段 key 缺不缺席，
  // 不看类型/enum/range/pattern；`0`/`false`/`""` 视为在场。
  const schema = () => ({
    fields: [
      { key: "axis", required: true },
      { key: "count", required: true },
      { key: "flag", required: true },
      { key: "optional", required: false },
    ],
  });

  it("required 缺席 → 逐个报出；选填缺席不报", () => {
    expect(missingRequiredParams(schema(), {})).toEqual(["axis", "count", "flag"]);
    expect(missingRequiredParams(schema(), { axis: 1 })).toEqual(["count", "flag"]);
  });

  it("required integer = 0 / boolean = false → 不算缺失（禁 truthy 判）", () => {
    expect(missingRequiredParams(schema(), { axis: 0, count: 0, flag: false })).toEqual([]);
  });

  it("undefined 算缺席（wire 等价 key 不存在）；null 在场（交 Core INVALID_TYPE）", () => {
    // null 是"存在但类型可能非法"，由 Core INVALID_TYPE 裁决，Web 不得报缺席。
    expect(missingRequiredParams(schema(), { axis: undefined, count: null, flag: false })).toEqual([
      "axis",
    ]);
  });

  it("补齐 required → 空（按钮可恢复加入）", () => {
    expect(missingRequiredParams(schema(), { axis: 1, count: 2, flag: true })).toEqual([]);
  });

  it("range/enum/type 错误不拦截（交给 Core issues 展示）", () => {
    // 越界/错类型照样算"在场"：前端不自作主张，Core 会报 OUT_OF_RANGE/INVALID_TYPE。
    expect(missingRequiredParams(schema(), { axis: "not-a-number", count: -999, flag: "x" })).toEqual(
      [],
    );
  });
});

describe("splitAcquisitionTasks", () => {
  it("首个 canonical 归编辑，其余归保留（task-a/b/c 场景）", () => {
    const { editable, preserved } = splitAcquisitionTasks([canon("task-a"), other("task-b"), other("task-c")]);
    expect(editable?.id).toBe("task-a");
    expect(preserved.map((t) => t.id)).toEqual(["task-b", "task-c"]);
  });

  it("无 canonical 时 editable 为空、全部保留", () => {
    const { editable, preserved } = splitAcquisitionTasks([other("task-b"), other("task-c")]);
    expect(editable).toBeNull();
    expect(preserved.map((t) => t.id)).toEqual(["task-b", "task-c"]);
  });

  it("多个 canonical 时只取第一个编辑，第二个保留", () => {
    const { editable, preserved } = splitAcquisitionTasks([canon("c1"), canon("c2")]);
    expect(editable?.id).toBe("c1");
    expect(preserved.map((t) => t.id)).toEqual(["c2"]);
  });
});

describe("mergeAcquisitionTasks", () => {
  const sel = [{ resource_id: "r2", parameters: {}, outputs: [] }];

  it("更新沿用原 canonical id，其它任务逐字保留（核心回归）", () => {
    const out = mergeAcquisitionTasks([canon("task-a"), other("task-b"), other("task-c")], {
      interval_ms: 2000,
      selections: sel,
    });
    expect(out.map((t) => t.id).sort()).toEqual(["task-a", "task-b", "task-c"]);
    const a = out.find((t) => t.id === "task-a")!;
    expect(a.schedule.interval_ms).toBe(2000);
    expect((a.binding.config as { selections: unknown }).selections).toEqual(sel);
    // 被保留任务逐字不动（含自定义 binding）
    expect(out.find((t) => t.id === "task-b")).toEqual(other("task-b"));
    expect(out.find((t) => t.id === "task-c")).toEqual(other("task-c"));
  });

  it("空集保存即新增 t1", () => {
    const out = mergeAcquisitionTasks([], { interval_ms: 1000, selections: sel });
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("t1");
    expect(out[0].binding.kind).toBe("mesa.resources.v1");
  });

  it("t1 被占用时避让为 t1-2", () => {
    const out = mergeAcquisitionTasks([other("t1")], { interval_ms: 1000, selections: sel });
    expect(out.map((t) => t.id).sort()).toEqual(["t1", "t1-2"]);
  });

  it("外来 subscribe canonical 不被默默翻成 poll，schedule 也保留", () => {
    const sub = canon("sub-1", { schedule: { mode: "subscribe" } });
    const out = mergeAcquisitionTasks([sub], { interval_ms: 1000, selections: sel });
    expect(out).toHaveLength(1);
    expect(out[0].schedule.mode).toBe("subscribe");
    expect(out[0].schedule.interval_ms).toBeUndefined();
  });
});

describe("isTaskSnapshotReady", () => {
  it("快照 pending（idle/loading）时不可 PUT（P0-1 回归：慢请求窗口）", () => {
    expect(isTaskSnapshotReady("idle", null, "ep-1")).toBe(false);
    expect(isTaskSnapshotReady("loading", null, "ep-1")).toBe(false);
  });

  it("快照失败（error）时不可 PUT（P0-1 回归：失败请求路径）", () => {
    expect(isTaskSnapshotReady("error", null, "ep-1")).toBe(false);
    // 即使有旧 loaded id，只要状态不是 ready 同样不可写
    expect(isTaskSnapshotReady("error", "ep-1", "ep-1")).toBe(false);
  });

  it("ready 但串 Endpoint 时不可 PUT（旧快照不得污染新编辑器）", () => {
    expect(isTaskSnapshotReady("ready", "ep-1", "ep-2")).toBe(false);
    expect(isTaskSnapshotReady("ready", null, "ep-1")).toBe(false);
  });

  it("ready + 同 Endpoint 时才可 PUT", () => {
    expect(isTaskSnapshotReady("ready", "ep-1", "ep-1")).toBe(true);
  });
});

describe("applyEndpointChange", () => {
  const ok = (): LifecycleStepResult => ({ ok: true });
  const fail = (message: string): LifecycleStepResult => ({ ok: false, message });

  it("STOPPED → 直接 apply，绝不 stop/start（保存后仍停止）", async () => {
    const calls: string[] = [];
    const out = await applyEndpointChange({
      wasRunning: false,
      restart: false,
      stop: async () => { calls.push("stop"); return ok(); },
      apply: async () => { calls.push("apply"); return ok(); },
      start: async () => { calls.push("start"); return ok(); },
    });
    expect(out).toEqual({ kind: "applied-stopped", restarted: false });
    expect(calls).toEqual(["apply"]);
  });

  it("RUNNING + restart → stop/apply/start 全走", async () => {
    const calls: string[] = [];
    const out = await applyEndpointChange({
      wasRunning: true,
      restart: true,
      stop: async () => { calls.push("stop"); return ok(); },
      apply: async () => { calls.push("apply"); return ok(); },
      start: async () => { calls.push("start"); return ok(); },
    });
    expect(out).toEqual({ kind: "applied-restarted", restarted: true });
    expect(calls).toEqual(["stop", "apply", "start"]);
  });

  it("RUNNING + !restart → stop/apply，不 start（保持停止）", async () => {
    const calls: string[] = [];
    const out = await applyEndpointChange({
      wasRunning: true,
      restart: false,
      stop: async () => { calls.push("stop"); return ok(); },
      apply: async () => { calls.push("apply"); return ok(); },
      start: async () => { calls.push("start"); return ok(); },
    });
    expect(out).toEqual({ kind: "applied-stopped", restarted: false });
    expect(calls).toEqual(["stop", "apply"]);
  });

  it("stop 500 → 中止，绝不 apply（更不 start）", async () => {
    const calls: string[] = [];
    const out = await applyEndpointChange({
      wasRunning: true,
      restart: true,
      stop: async () => { calls.push("stop"); return fail("停止失败（500）"); },
      apply: async () => { calls.push("apply"); return ok(); },
      start: async () => { calls.push("start"); return ok(); },
    });
    expect(out.kind).toBe("stop-failed");
    expect(calls).toEqual(["stop"]);
  });

  it("apply 失败 → 不 start，明确已停止", async () => {
    const calls: string[] = [];
    const out = await applyEndpointChange({
      wasRunning: true,
      restart: true,
      stop: async () => { calls.push("stop"); return ok(); },
      apply: async () => { calls.push("apply"); return fail("点位保存失败"); },
      start: async () => { calls.push("start"); return ok(); },
    });
    expect(out.kind).toBe("apply-failed-stopped");
    expect(calls).toEqual(["stop", "apply"]);
  });

  it("start 500 → applied-but-restart-failed（保存成功但恢复运行失败）", async () => {
    const out = await applyEndpointChange({
      wasRunning: true,
      restart: true,
      stop: async () => ok(),
      apply: async () => ok(),
      start: async () => fail("恢复运行失败（500）"),
    });
    expect(out.kind).toBe("applied-but-restart-failed");
  });
});

describe("formatPointValue", () => {
  it("标量直显、空值占位", () => {
    expect(formatPointValue(42)).toBe("42");
    expect(formatPointValue(true)).toBe("true");
    expect(formatPointValue("abc")).toBe("abc");
    expect(formatPointValue(null)).toBe("—");
    expect(formatPointValue(undefined)).toBe("—");
  });

  it("数组给长度 + 前 3 项摘要，不全量展开", () => {
    expect(formatPointValue([1, 2, 3])).toBe("[3] 1, 2, 3");
    expect(formatPointValue([1, 2, 3, 4, 5])).toBe("[5] 1, 2, 3, …");
    expect(formatPointValue([])).toBe("[0] ");
  });

  it("对象 JSON 化、超长截断", () => {
    expect(formatPointValue({ a: 1 })).toBe('{"a":1}');
    expect(formatPointValue("x".repeat(200)).endsWith("…")).toBe(true);
  });
});

describe("pointAgeMs/formatAge", () => {
  it("ns 转年龄；非法返回 null", () => {
    expect(pointAgeMs(3_000_000_000, 5000)).toBe(2000);
    expect(pointAgeMs(undefined, 5000)).toBeNull();
    expect(pointAgeMs(-5, 5000)).toBeNull();
  });

  it("文案分级", () => {
    expect(formatAge(null)).toBe("—");
    expect(formatAge(500)).toBe("刚刚");
    expect(formatAge(3000)).toBe("3秒前");
    expect(formatAge(125000)).toBe("2分钟前");
  });
});

describe("pointsSnapshotSignature", () => {
  const T0 = 1_700_000_000_000;
  const mk = (key: string, value: unknown, ageMs: number, quality = "GOOD") => ({
    endpoint_id: "ep",
    key,
    point_id: key.length,
    quality,
    value,
    timestamp_ns: (T0 - ageMs) * 1e6,
  });

  it("值/质量/时间戳全不变则签名不变（跳过重渲染）", () => {
    const a = [mk("k1", 1.5, 500), mk("k2", "x", 600)];
    expect(pointsSnapshotSignature(a, T0)).toBe(pointsSnapshotSignature(a, T0));
  });

  it("值变化则签名变化", () => {
    const a = [mk("k1", 1.5, 500)];
    const b = [mk("k1", 1.6, 500)];
    expect(pointsSnapshotSignature(a, T0)).not.toBe(pointsSnapshotSignature(b, T0));
  });

  it("STALE 翻转属于签名（阈值 30s，语义无损）", () => {
    const pts = [mk("k1", 1, 29_000)];
    const s1 = pointsSnapshotSignature(pts, T0);
    expect(derivePointStale("GOOD", 29_000)).toBe("GOOD");
    // 时间推进越过阈值：签名必须变化（翻转被捕获）
    const s2 = pointsSnapshotSignature(pts, T0 + 2000);
    expect(s2).not.toBe(s1);
    expect(derivePointStale("GOOD", 31_000)).toBe("STALE");
  });

  it("顺序无关（后端已稳定排序，防御性处理）", () => {
    const a = [mk("k1", 1, 500), mk("k2", 2, 500)];
    const b = [mk("k2", 2, 500), mk("k1", 1, 500)];
    expect(pointsSnapshotSignature(a, T0)).toBe(pointsSnapshotSignature(b, T0));
  });
});

describe("reconcilePointSnapshots", () => {
  const T0 = 1_700_000_000_000;
  const mk = (over: Record<string, unknown> = {}) => ({
    endpoint_id: "ep",
    key: "k1",
    point_id: 1,
    quality: "GOOD",
    type: "f64",
    value: 100,
    timestamp_ns: (T0 - 500) * 1e6,
    source_label: "DB10.DBD20",
    ...over,
  });

  it("全同则保旧引用（下游 memo/行级 bailout 的真正保证）", () => {
    const prev = [mk()];
    const next = [mk()];
    const { points, changed } = reconcilePointSnapshots(prev, next, T0);
    expect(changed).toBe(false);
    expect(points[0]).toBe(prev[0]);
  });

  it("source_label-only 变化必须更新（P1 metadata 可独立刷新）", () => {
    const prev = [mk({ source_label: "DB10.DBD20" })];
    const next = [mk({ source_label: "DB20.DBD40" })];
    const { points, changed } = reconcilePointSnapshots(prev, next, T0);
    expect(changed).toBe(true);
    expect(points[0]).toBe(next[0]);
    expect((points[0] as { source_label: string }).source_label).toBe("DB20.DBD40");
  });

  it("display_name-only 变化必须更新（可跨客户端改名）", () => {
    const prev = [mk({ display_name: undefined })];
    const next = [mk({ display_name: "新名字" })];
    const { changed } = reconcilePointSnapshots(prev, next, T0);
    expect(changed).toBe(true);
  });

  it("key/type-only 变化必须更新", () => {
    expect(reconcilePointSnapshots([mk()], [mk({ type: "i32" })], T0).changed).toBe(true);
    expect(reconcilePointSnapshots([mk()], [mk({ key: "k2" })], T0).changed).toBe(true);
  });

  it("STALE 翻转被捕获（derived 跨时刻比较）", () => {
    const prev = [mk({ timestamp_ns: (T0 - 29_000) * 1e6 })];
    const next = [mk({ timestamp_ns: (T0 - 29_000) * 1e6 })];
    // 上轮 GOOD（29s），本轮 STALE（31s）：字段全同但跨阈值，必须更新
    const { changed } = reconcilePointSnapshots(prev, next, T0 + 2000, T0);
    expect(changed).toBe(true);
    // 同一时刻内比较则不变（无跨阈值）
    const same = reconcilePointSnapshots(prev, next, T0 + 2000, T0 + 2000);
    expect(same.changed).toBe(false);
  });

  it("增删行改变长度即 changed", () => {
    expect(reconcilePointSnapshots([mk()], [], T0).changed).toBe(true);
    expect(reconcilePointSnapshots([], [mk()], T0).changed).toBe(true);
  });

  it("指纹覆盖全部 UI 可观察字段", () => {
    const base = mk();
    for (const over of [
      { key: "k2" },
      { point_key: "pk" },
      { type: "i32" },
      { quality: "BAD" },
      { value: 101 },
      { timestamp_ns: T0 * 1e6 },
      { source_label: "X" },
      { display_name: "N" },
    ]) {
      expect(pointRowFingerprint(base, T0)).not.toBe(
        pointRowFingerprint({ ...base, ...over }, T0),
      );
    }
  });
});

describe("reconcilePointDelta（Point Live delta 整行合并）", () => {
  const T = 1_700_000_000_000;
  const row = (pid: number, value: unknown, extra?: Record<string, unknown>) => ({
    endpoint_id: "ep",
    point_id: pid,
    point_key: `k${pid}`,
    key: `k${pid}`,
    quality: "GOOD",
    value,
    timestamp_ns: T * 1e6,
    ...extra,
  });

  it("空 delta 不变（保引用）", () => {
    const prev = [row(1, 1)];
    const { points, changed } = reconcilePointDelta(prev, []);
    expect(changed).toBe(false);
    expect(points).toBe(prev);
  });

  it("已有整行替换（非 patch，不留脏字段）", () => {
    const prev = [row(1, 1, { source_label: "A" })];
    const next = row(1, 2, { source_label: "B" });
    const { points, changed } = reconcilePointDelta(prev, [next]);
    expect(changed).toBe(true);
    expect(points).toHaveLength(1);
    expect(points[0]).toEqual(next);
  });

  it("未变化行保引用，新点插入，未提及行不动", () => {
    const prev = [row(1, 1), row(2, 2)];
    const { points, changed } = reconcilePointDelta(prev, [row(1, 1), row(3, 3)]);
    expect(changed).toBe(true);
    expect(points).toHaveLength(3);
    expect(points.find((p) => p.point_id === 2)).toBe(prev[1]);
  });

  it("delta 不删除行（删除只由 snapshot 表达）", () => {
    const prev = [row(1, 1), row(2, 2)];
    const { points } = reconcilePointDelta(prev, [row(1, 9)]);
    expect(points).toHaveLength(2);
  });
});

describe("nextStaleDeadlineMs（Point Live STALE 翻转调度）", () => {
  const T = 1_700_000_000_000;
  const row = (ageMs: number, quality = "GOOD") => ({
    endpoint_id: "ep",
    point_id: 1,
    quality,
    value: 1,
    timestamp_ns: (T - ageMs) * 1e6,
  });

  it("最近翻转取最小剩余；BAD 不参与；空/全 BAD 即 null", () => {
    // A 剩 8s，B 剩 15s → 8s。
    expect(nextStaleDeadlineMs([row(22_000), { ...row(15_000), point_id: 2 }], T)).toBe(
      POINT_STALE_AFTER_MS - 22_000,
    );
    // BAD 永不 STALE。
    expect(nextStaleDeadlineMs([row(100_000, "BAD")], T)).toBeNull();
    expect(nextStaleDeadlineMs([], T)).toBeNull();
  });

  it("已越界不等待（调用方立即重算收敛）", () => {
    expect(nextStaleDeadlineMs([row(31_000)], T)).toBeNull();
  });
});
