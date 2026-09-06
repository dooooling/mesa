// PR8 Gate（纯函数）：时间/严重度/通用值渲染语义。
import { describe, expect, it } from "vitest";
import { formatBytes, formatMesaValue, formatNsTime, formatSeverity } from "./format";

describe("formatNsTime", () => {
  it("occurred 缺失显示 em dash（绝不用 received_at 冒充）", () => {
    expect(formatNsTime(null)).toBe("—");
    expect(formatNsTime(undefined)).toBe("—");
  });

  it("三层时间各自独立格式化", () => {
    const a = formatNsTime(1_700_000_000_000_000_000);
    const b = formatNsTime(1_700_000_000_100_000_000);
    expect(a).not.toBe("—");
    expect(b).not.toBe("—");
    expect(a).not.toBe(b);
  });
});

describe("formatSeverity", () => {
  it("0 显示 unknown，不自创 warning/critical 分类", () => {
    expect(formatSeverity(0)).toBe("0 (unknown)");
    expect(formatSeverity(700)).toBe("700");
  });
});

describe("formatMesaValue", () => {
  it("通用渲染：字符串/数字/对象", () => {
    expect(formatMesaValue("x")).toBe("x");
    expect(formatMesaValue(7)).toBe("7");
    expect(formatMesaValue(null)).toBe("—");
    expect(formatMesaValue({ a: 1 })).toContain("a");
  });
});

describe("formatBytes", () => {
  it("存储规模可读", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(7_400_000)).toContain("MB");
  });
});
