// M1 Connection Context 纯函数测试：选择稳定规则。
// - 观察 tab 默认全部；`all` 归一化为无参；
// - 配置 tab 优先 URL，非法才回落第一个，并写回 URL；
// - 无连接时为 none，不编造。
import { describe, expect, it } from "vitest";
import {
  isSingleConnectionTab,
  isWorkspaceTab,
  resolveEffectiveConnection,
} from "./connection";

describe("workspace connection context", () => {
  it("观察 tab 默认全部连接", () => {
    for (const tab of ["overview", "data", "events"] as const) {
      expect(resolveEffectiveConnection({ tab, param: null, endpointIds: ["a", "b"] })).toEqual({
        effective: { kind: "all" },
        normalizedParam: null,
      });
    }
    expect(isSingleConnectionTab("data")).toBe(false);
  });

  it("观察 tab 的 all 归一化为无参（避免两种全部漂移）", () => {
    expect(resolveEffectiveConnection({ tab: "data", param: "all", endpointIds: ["a"] })).toEqual({
      effective: { kind: "all" },
      normalizedParam: null,
    });
  });

  it("观察 tab 合法 id 进入单连接上下文，非法回落全部", () => {
    expect(resolveEffectiveConnection({ tab: "data", param: "a", endpointIds: ["a", "b"] })).toEqual({
      effective: { kind: "single", endpointId: "a" },
      normalizedParam: "a",
    });
    expect(resolveEffectiveConnection({ tab: "data", param: "ghost", endpointIds: ["a"] })).toEqual({
      effective: { kind: "all" },
      normalizedParam: null,
    });
  });

  it("配置 tab 优先沿用 URL 的合法连接", () => {
    expect(resolveEffectiveConnection({ tab: "config", param: "b", endpointIds: ["a", "b"] })).toEqual({
      effective: { kind: "single", endpointId: "b" },
      normalizedParam: "b",
    });
    expect(isSingleConnectionTab("diagnostics")).toBe(true);
  });

  it("配置 tab 无参/非法时回落第一个连接并同步回 URL", () => {
    expect(resolveEffectiveConnection({ tab: "config", param: null, endpointIds: ["a", "b"] })).toEqual({
      effective: { kind: "single", endpointId: "a" },
      normalizedParam: "a",
    });
    expect(resolveEffectiveConnection({ tab: "diagnostics", param: "ghost", endpointIds: ["a"] })).toEqual({
      effective: { kind: "single", endpointId: "a" },
      normalizedParam: "a",
    });
  });

  it("无连接时为 none（不编造默认）", () => {
    expect(resolveEffectiveConnection({ tab: "config", param: null, endpointIds: [] }).effective).toEqual({
      kind: "none",
    });
    expect(resolveEffectiveConnection({ tab: "data", param: null, endpointIds: [] }).effective).toEqual({
      kind: "all",
    });
  });

  it("tab 守卫识别非法 tab", () => {
    expect(isWorkspaceTab("overview")).toBe(true);
    expect(isWorkspaceTab("bogus")).toBe(false);
  });
});
