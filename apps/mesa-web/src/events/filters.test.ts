// PR8 Gate（纯函数）：表单过滤器 → 后端 query 参数（精确匹配语义）。
import { describe, expect, it } from "vitest";
import { EMPTY_EVENT_FILTER_FORM, toEventFilter } from "./filters";

describe("toEventFilter", () => {
  it("空表单只带默认 limit", () => {
    expect(toEventFilter(EMPTY_EVENT_FILTER_FORM, {})).toEqual({ limit: 100 });
  });

  it("精确匹配字段透传（去空白；空字符串丢弃）", () => {
    const out = toEventFilter(
      { ...EMPTY_EVENT_FILTER_FORM, endpoint_id: " sim-01 ", category: "alarm", kind: "", code: "  " },
      {},
    );
    expect(out.endpoint_id).toBe("sim-01");
    expect(out.category).toBe("alarm");
    expect(out.kind).toBeUndefined();
    expect(out.code).toBeUndefined();
  });

  it("severity 钳制到 0..1000", () => {
    expect(toEventFilter({ ...EMPTY_EVENT_FILTER_FORM, severity_min: 2000 }, {}).severity_min).toBe(1000);
    expect(toEventFilter({ ...EMPTY_EVENT_FILTER_FORM, severity_min: -5 }, {}).severity_min).toBe(0);
  });

  it("active 三态映射", () => {
    expect(toEventFilter({ ...EMPTY_EVENT_FILTER_FORM, active: "active" }, {}).active).toBe(true);
    expect(toEventFilter({ ...EMPTY_EVENT_FILTER_FORM, active: "inactive" }, {}).active).toBe(false);
    expect(toEventFilter({ ...EMPTY_EVENT_FILTER_FORM, active: "all" }, {}).active).toBeUndefined();
  });

  it("分页 before_seq 透传", () => {
    expect(toEventFilter(EMPTY_EVENT_FILTER_FORM, { before_seq: 77, limit: 50 })).toEqual({
      limit: 50,
      before_seq: 77,
    });
  });
});
