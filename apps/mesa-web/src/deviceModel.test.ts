// PR27 Device-first 模型单测：锁定 Device≈Endpoint 分离语义。
// 行为与协议语义由后端 contract 保证，这里只断言 Web 不再发明旧假设。
import { describe, expect, it } from "vitest";
import {
  buildEndpointCreatePayload,
  buildEndpointUpdatePayload,
  canDeleteDevice,
  cleanConnection,
  deviceCounts,
  groupEndpointsByDevice,
  isDriverChangeAttempt,
  isRunningState,
  suggestEndpointId,
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
