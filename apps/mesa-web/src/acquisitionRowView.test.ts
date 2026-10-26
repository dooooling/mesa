// 点位行视图模型与查询（纯函数）门禁：搜索三段、按数据生成筛选项、必要标识。
import { describe, expect, it } from "vitest";
import {
  applyRowQuery,
  buildFilterOptions,
  countRowStates,
  ensureDistinguishable,
  matchesPointQuery,
  normalizeNeedle,
  shortPointKey,
  type PointRowView,
} from "./acquisitionRowView";

function row(overrides: Partial<PointRowView> = {}): PointRowView {
  return {
    rowId: "row-1",
    title: "轴实际位置",
    summary: "轴 1 · 通道 1",
    searchText: "轴实际位置\n轴 1 · 通道 1\naxis.position.actual_bcs@axis=i:1",
    detail: [],
    ...overrides,
  };
}

describe("搜索：标题、摘要、标识三段都命中", () => {
  it("matches_title_summary_and_identifier", () => {
    const target = row();
    expect(matchesPointQuery(target, normalizeNeedle("轴实际位置"))).toBe(true);
    expect(matchesPointQuery(target, normalizeNeedle("通道 1"))).toBe(true);
    // 标识（点键）也要能搜到——即使行内不展示完整点键。
    expect(matchesPointQuery(target, normalizeNeedle("axis.position"))).toBe(true);
    expect(matchesPointQuery(target, normalizeNeedle("NATIVE"))).toBe(false);
    // 大小写不敏感。
    expect(matchesPointQuery(target, normalizeNeedle("AXIS.POSITION"))).toBe(true);
  });

  it("empty_query_matches_everything", () => {
    expect(matchesPointQuery(row(), normalizeNeedle("   "))).toBe(true);
  });
});

describe("筛选：只有存在问题时才提供对应入口（不写死分类）", () => {
  it("only_all_option_when_no_issues_and_no_changes", () => {
    expect(buildFilterOptions({ issueCount: 0, modifiedCount: 0 })).toEqual([
      { value: "all", label: "全部" },
    ]);
  });

  it("adds_issue_and_modified_options_from_actual_data", () => {
    expect(buildFilterOptions({ issueCount: 3, modifiedCount: 2 })).toEqual([
      { value: "all", label: "全部" },
      { value: "issue", label: "有问题（3）" },
      { value: "modified", label: "已修改（2）" },
    ]);
  });

  it("counts_come_from_rows", () => {
    const rows = [
      row({ rowId: "a", issue: { text: "定义缺失" } }),
      row({ rowId: "b", marker: "modified" }),
      row({ rowId: "c", marker: "added", issue: { text: "校验失败" } }),
      row({ rowId: "d" }),
    ];
    expect(countRowStates(rows)).toEqual({ issueCount: 2, modifiedCount: 2 });
    // 「有问题」只看真实问题；「已修改」包含新增与修改。
    expect(applyRowQuery(rows, { filter: "issue" }).map((r) => r.rowId)).toEqual(["a", "c"]);
    expect(applyRowQuery(rows, { filter: "modified" }).map((r) => r.rowId)).toEqual(["b", "c"]);
  });

  it("query_and_filter_are_combined_without_reordering", () => {
    const rows = [
      row({ rowId: "a", title: "主轴转速", summary: "通道 1" }),
      row({ rowId: "b", title: "主轴负载", summary: "通道 1", marker: "modified" }),
      row({ rowId: "c", title: "进给速度", summary: "通道 1" }),
    ];
    expect(applyRowQuery(rows, { query: "主轴" }).map((r) => r.rowId)).toEqual(["a", "b"]);
    expect(applyRowQuery(rows, { query: "主轴", filter: "modified" }).map((r) => r.rowId)).toEqual([
      "b",
    ]);
  });
});

describe("必要标识：只有名称不足或重名时才补", () => {
  it("duplicated_title_and_summary_get_identifier", () => {
    const rows = ensureDistinguishable([
      row({ rowId: "a", title: "原生点位", summary: "区域 C · 区域号 1", identifierHint: "native.C.1" }),
      row({ rowId: "b", title: "原生点位", summary: "区域 C · 区域号 1", identifierHint: "native.C.2" }),
    ]);
    expect(rows[0]!.identifier).toBe("native.C.1");
    expect(rows[1]!.identifier).toBe("native.C.2");
  });

  it("distinguishable_rows_stay_clean", () => {
    const rows = ensureDistinguishable([
      row({ rowId: "a", title: "主轴转速", summary: "通道 1", identifierHint: "x@1" }),
      row({ rowId: "b", title: "进给速度", summary: "通道 1", identifierHint: "y@1" }),
    ]);
    expect(rows[0]!.identifier).toBeUndefined();
    expect(rows[1]!.identifier).toBeUndefined();
  });

  it("unnamed_row_always_gets_identifier", () => {
    const rows = ensureDistinguishable([row({ title: "   ", identifierHint: "k-1" })]);
    expect(rows[0]!.identifier).toBe("k-1");
  });

  it("short_point_key_keeps_the_tail(self)", () => {
    expect(shortPointKey("native.C.1.122.5.1")).toBe("5.1");
    expect(shortPointKey("single")).toBe("single");
    expect(shortPointKey("  ")).toBe("");
  });
});
