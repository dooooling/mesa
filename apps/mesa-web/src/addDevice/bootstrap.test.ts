// M4.2 bootstrap 编排单测：步骤顺序 + 反向补偿 + 残留明示。
import { describe, expect, it, vi } from "vitest";
import { bootstrapDevice, isDraftComplete, type BootstrapClient } from "./bootstrap";

function client(over: Partial<BootstrapClient> = {}): BootstrapClient & { calls: string[] } {
  const calls: string[] = [];
  const ok = { status: 201 as number, body: {} };
  return {
    calls,
    createDevice: vi.fn(async () => { calls.push("create-device"); return ok; }),
    createEndpoint: vi.fn(async () => { calls.push("create-endpoint"); return ok; }),
    putTasks: vi.fn(async () => { calls.push("put-tasks"); return ok; }),
    startEndpoint: vi.fn(async () => { calls.push("start-endpoint"); return ok; }),
    deleteEndpoint: vi.fn(async () => { calls.push("delete-endpoint"); return ok; }),
    deleteDevice: vi.fn(async () => { calls.push("delete-device"); return ok; }),
    ...over,
  };
}

const DRAFT = {
  device: { deviceId: "cnc-01", deviceName: "CNC-01" },
  connection: { driverId: "simulator", endpointId: "sim-ep", endpointName: "SIM", connection: {} },
  acquisition: { intervalMs: 1000, selections: [{ resource_id: "r1", parameters: {}, outputs: [] }], startAfterCreate: true },
};

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

describe("bootstrapDevice", () => {
  it("全成功：四步顺序执行，无补偿", async () => {
    const c = client();
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(true);
    expect(c.calls).toEqual(["create-device", "create-endpoint", "put-tasks", "start-endpoint"]);
    expect(r.compensated).toEqual([]);
    expect(r.residual).toBe(false);
  });

  it("create-endpoint 失败：补偿删除 device", async () => {
    const c = client({
      createEndpoint: vi.fn(async () => ({ status: 500, body: { error: { message: "bad conn" } } })),
    });
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("create-endpoint");
    expect(r.failedMessage).toBe("bad conn");
    expect(r.compensated).toEqual([{ action: "delete-device", ok: true, message: undefined }]);
    expect(r.residual).toBe(false);
  });

  it("start 失败：补偿删除 endpoint → device", async () => {
    const c = client({
      startEndpoint: vi.fn(async () => ({ status: 500, body: { error: { message: "start boom" } } })),
    });
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("start-endpoint");
    expect(r.compensated.map((x) => x.action)).toEqual(["delete-endpoint", "delete-device"]);
    expect(r.residual).toBe(false);
  });

  it("补偿也失败：residual 必须为 true（明示残留）", async () => {
    const c = client({
      putTasks: vi.fn(async () => ({ status: 500, body: {} })),
      deleteEndpoint: vi.fn(async () => ({ status: 500, body: { error: { message: "del-ep boom" } } })),
    });
    const r = await bootstrapDevice(c, DRAFT);
    expect(r.ok).toBe(false);
    expect(r.failedStep).toBe("put-tasks");
    expect(r.compensated[0]).toMatchObject({ action: "delete-endpoint", ok: false });
    // endpoint 删不掉时不再删 device（归属仍在，后端会拒绝）
    expect(r.compensated.map((x) => x.action)).toEqual(["delete-endpoint"]);
    expect(r.residual).toBe(true);
  });

  it("startAfterCreate=false 时跳过启动（保持 STOPPED）", async () => {
    const c = client();
    const r = await bootstrapDevice(c, { ...DRAFT, acquisition: { ...DRAFT.acquisition, startAfterCreate: false } });
    expect(r.ok).toBe(true);
    expect(c.calls).toEqual(["create-device", "create-endpoint", "put-tasks"]);
  });
});
