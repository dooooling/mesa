// M5.5 原子提交单测：一次 POST + 结果映射 + 幂等键稳定。
import { describe, expect, it, vi } from "vitest";
import { bootstrapDevice, draftIdempotencyKey, isDraftComplete, type BootstrapClient } from "./bootstrap";

const DRAFT = {
  device: { deviceId: "cnc-01", deviceName: "CNC-01" },
  connection: { driverId: "simulator", endpointId: "sim-ep", endpointName: "SIM", connection: {} },
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
  it("三步齐全才可提交", () => {
    expect(isDraftComplete({ device: null, connection: null, acquisition: null })).toBe(false);
    expect(isDraftComplete({ ...DRAFT, device: { deviceId: "", deviceName: "" } } as never)).toBe(false);
    expect(isDraftComplete({ device: DRAFT.device, connection: null, acquisition: DRAFT.acquisition })).toBe(false);
    expect(
      isDraftComplete({ device: DRAFT.device, connection: DRAFT.connection, acquisition: { ...DRAFT.acquisition, selections: [] } }),
    ).toBe(false);
    expect(isDraftComplete(DRAFT)).toBe(true);
  });
});

describe("draftIdempotencyKey", () => {
  it("同 draft 同 key（重放），改任一字段即换 key", () => {
    const k1 = draftIdempotencyKey(DRAFT);
    const k2 = draftIdempotencyKey(structuredClone(DRAFT));
    expect(k1).toBe(k2);
    expect(k1.startsWith("web-")).toBe(true);
    const k3 = draftIdempotencyKey({ ...DRAFT, acquisition: { ...DRAFT.acquisition, intervalMs: 500 } });
    expect(k3).not.toBe(k1);
  });
});

describe("bootstrapDevice（原子接口）", () => {
  it("201：一次 POST，请求体含 device/endpoint/tasks/start/幂等键", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "sim-ep", revision: 1, started: true });
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(true);
    expect(r.failedStep).toBeNull();
    expect(r.replayed).toBe(false);
    expect(c.calls).toHaveLength(1);
    const b = c.calls[0] as Record<string, unknown>;
    expect(b).toMatchObject({
      device: { id: "cnc-01", name: "CNC-01" },
      endpoint: { id: "sim-ep", driver_id: "simulator" },
      start: true,
    });
    expect(typeof (b as { idempotency_key?: unknown }).idempotency_key).toBe("string");
    const tasks = (b as { acquisition: { tasks: Array<{ id: string; binding: { kind: string } }> } }).acquisition.tasks;
    expect(tasks).toHaveLength(1);
    expect(tasks[0].binding.kind).toBe("mesa.resources.v1");
  });

  it("200 + replayed：幂等重放（未重复创建）", async () => {
    const c = client({ device_id: "cnc-01", endpoint_id: "sim-ep", replayed: true }, 200);
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
});
