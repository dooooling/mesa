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

describe("isRunningState", () => {
  it("RUNNING/CONNECTING/RECONNECTING 视为在线", () => {
    expect(isRunningState("RUNNING")).toBe(true);
    expect(isRunningState("reconnecting")).toBe(true);
    expect(isRunningState("STOPPED")).toBe(false);
    expect(isRunningState(undefined)).toBe(false);
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
  mode: "poll",
  interval_ms: 1000,
  binding: {
    kind: "mesa.resources.v1",
    config: { selections: [{ resource_id: "r", parameters: {}, outputs: [] }] },
  },
  ...extra,
});

const other = (id: string): AcquisitionTaskShape => ({
  id,
  mode: "poll",
  interval_ms: 500,
  binding: { kind: "driver.native.v1", config: { op: "scan" } },
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
    expect(a.interval_ms).toBe(2000);
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

  it("外来 subscribe canonical 不被默默翻成 poll，周期也保留", () => {
    const sub = canon("sub-1", { mode: "subscribe", interval_ms: null });
    const out = mergeAcquisitionTasks([sub], { interval_ms: 1000, selections: sel });
    expect(out).toHaveLength(1);
    expect(out[0].mode).toBe("subscribe");
    expect(out[0].interval_ms).toBeNull();
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
