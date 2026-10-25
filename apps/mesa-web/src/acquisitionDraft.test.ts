// P2-1 / P2-2：采集草稿指纹与「已保存 / 待保存」文案（纯函数）。
//
// 重点锁两件事：
// 1. 指纹必须覆盖完整有效配置——同一原生点位只改 decode/count 时键不变，
//    但指纹必须变化（否则旧的「已保存」结论会残留）；
// 2. 状态区文案必须能区分「当前生效」与「尚未保存」，服务端无已保存值时
//    不得把草稿冒充成已生效配置。
import { describe, expect, it } from "vitest";
import { draftSignature, savedVsDraftText } from "./acquisitionDraft";
import type { NckPointSelection, ResourceSelection } from "./deviceModel";

function nativePoint(parameters: Record<string, unknown>): NckPointSelection {
  return {
    kind: "nck_native",
    point_key: "native.A.1.26.30110.1",
    parameters,
  };
}

const BASE_NATIVE = { area: "A", area_no: 1, module: 26, column: 30110, line: 1, decode: "U16_LE" };

function selection(parameters: Record<string, unknown>): ResourceSelection {
  return {
    resource_id: "register",
    parameters,
    outputs: [{ output: "value", point_key: "register.value" }],
  };
}

describe("draftSignature：完整有效配置指纹", () => {
  it("同一点位只改 decode ⇒ 指纹变化（旧实现只比 point_key 会漏）", () => {
    const before = draftSignature({
      intervalMs: 1000,
      nckPoints: [nativePoint({ ...BASE_NATIVE })],
    });
    const after = draftSignature({
      intervalMs: 1000,
      nckPoints: [nativePoint({ ...BASE_NATIVE, decode: "U16_BE" })],
    });
    expect(after).not.toBe(before);
  });

  it("同一点位只改 count ⇒ 指纹变化", () => {
    const before = draftSignature({ intervalMs: 1000, nckPoints: [nativePoint({ ...BASE_NATIVE })] });
    const after = draftSignature({
      intervalMs: 1000,
      nckPoints: [nativePoint({ ...BASE_NATIVE, count: 2 })],
    });
    expect(after).not.toBe(before);
  });

  it("周期变化 ⇒ 指纹变化", () => {
    expect(draftSignature({ intervalMs: 1000 })).not.toBe(draftSignature({ intervalMs: 2000 }));
  });

  it("点位顺序变化 ⇒ 指纹变化（顺序也是配置的一部分）", () => {
    const a = nativePoint({ ...BASE_NATIVE, line: 1 });
    const b = nativePoint({ ...BASE_NATIVE, line: 2 });
    expect(draftSignature({ intervalMs: 1000, nckPoints: [a, b] })).not.toBe(
      draftSignature({ intervalMs: 1000, nckPoints: [b, a] }),
    );
  });

  it("对象 key 顺序不同但内容相同 ⇒ 指纹相同（不制造假变更）", () => {
    const a = draftSignature({
      intervalMs: 1000,
      nckPoints: [nativePoint({ area: "A", area_no: 1, decode: "U16_LE" })],
    });
    const b = draftSignature({
      intervalMs: 1000,
      nckPoints: [nativePoint({ decode: "U16_LE", area_no: 1, area: "A" })],
    });
    expect(b).toBe(a);
  });

  it("非 NCK 路径覆盖 resource_id / parameters / outputs", () => {
    const base = draftSignature({ intervalMs: 1000, selections: [selection({ unit: 3 })] });
    expect(draftSignature({ intervalMs: 1000, selections: [selection({ unit: 4 })] })).not.toBe(base);
    const other = selection({ unit: 3 });
    other.outputs = [{ output: "value", point_key: "register.other" }];
    expect(draftSignature({ intervalMs: 1000, selections: [other] })).not.toBe(base);
    const otherResource = selection({ unit: 3 });
    otherResource.resource_id = "register2";
    expect(draftSignature({ intervalMs: 1000, selections: [otherResource] })).not.toBe(base);
  });

  it("切换保存路径（selection ↔ nckPoints）⇒ 指纹变化", () => {
    expect(draftSignature({ intervalMs: 1000, selections: [] })).not.toBe(
      draftSignature({ intervalMs: 1000, nckPoints: [] }),
    );
  });
});

describe("savedVsDraftText：已保存 / 待保存对照", () => {
  it("无已保存值（服务端尚无任务）⇒ 只能声明待保存", () => {
    expect(savedVsDraftText(null, 1000, "ms")).toBe("1000 ms（待保存）");
  });

  it("未修改 ⇒ 单值标注已保存", () => {
    expect(savedVsDraftText(1000, 1000, "ms")).toBe("1000 ms（已保存）");
    expect(savedVsDraftText(1, 1, "个")).toBe("1 个（已保存）");
  });

  it("已修改 ⇒ 同时显示已保存值与待保存值", () => {
    expect(savedVsDraftText(1000, 2000, "ms")).toBe("1000 ms（已保存） → 2000 ms（待保存）");
    expect(savedVsDraftText(2, 3, "个")).toBe("2 个（已保存） → 3 个（待保存）");
  });
});
