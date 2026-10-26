// `issueLocation` 单测：按**后端实际 path 形态**（见 issueLocation.ts 顶部引用清单）
// 逐个断言映射结果，并锁死「不可定位 ⇒ 明确说明、不猜一行」。
import { describe, expect, it } from "vitest";
import {
  describeUnlocatable,
  firstLocatableIssue,
  locateIssuePath,
  type SubmittedRowSnapshot,
} from "./issueLocation";

// 提交快照：下标 0 是本次编辑的任务（238 点里只有 3 行也同理：下标与 rowId 一一对应），
// 下标 1 是保留任务。
const SNAPSHOT: SubmittedRowSnapshot = {
  taskIndex: 0,
  taskCount: 2,
  rowIds: ["row-1", "row-7", "row-9"],
};

describe("字段级 path ⇒ 行 + 字段", () => {
  it("NCK 点位 parameters.<field>（schema.rs:293-301 的 {root}.{field}）", () => {
    const loc = locateIssuePath("tasks[0].binding.config.points[2].parameters.module", SNAPSHOT);
    expect(loc).toEqual({
      kind: "row",
      taskIndex: 0,
      itemIndex: 2,
      rowId: "row-9",
      fieldKey: "module",
    });
  });

  it("通用选择 parameters.<field>（resource.rs:318-322 的 {base}.parameters）", () => {
    const loc = locateIssuePath("tasks[0].selections[1].parameters.axis", SNAPSHOT);
    expect(loc).toEqual({
      kind: "row",
      taskIndex: 0,
      itemIndex: 1,
      rowId: "row-7",
      fieldKey: "axis",
    });
  });
});

describe("点位级 path ⇒ 行（不带字段）", () => {
  it("points[j] 本身（point.rs:855 的 {root}.points[i]）", () => {
    const loc = locateIssuePath("tasks[0].binding.config.points[0]", SNAPSHOT);
    expect(loc).toMatchObject({ kind: "row", rowId: "row-1", fieldKey: null });
  });

  it("points[j].parameters（parameters 缺失/类型错：point.rs:915-928）", () => {
    const loc = locateIssuePath("tasks[0].binding.config.points[1].parameters", SNAPSHOT);
    expect(loc).toMatchObject({ kind: "row", rowId: "row-7", fieldKey: null });
  });

  it("points[j].point_key（point.rs:889-907）", () => {
    const loc = locateIssuePath("tasks[0].binding.config.points[1].point_key", SNAPSHOT);
    expect(loc).toMatchObject({ kind: "row", rowId: "row-7", fieldKey: null });
  });

  it("selections[j].outputs（resource.rs:329-357）", () => {
    const loc = locateIssuePath("tasks[0].selections[2].outputs", SNAPSHOT);
    expect(loc).toMatchObject({ kind: "row", rowId: "row-9", fieldKey: null });
  });
});

describe("不可定位 ⇒ 明确说明，不猜目标", () => {
  it("任务级：binding.kind / binding / binding.config / selections", () => {
    for (const path of [
      "tasks[0].binding.kind",
      "tasks[0].binding",
      "tasks[0].binding.config",
      "tasks[0].selections",
      "tasks[0].schedule.interval_ms",
    ]) {
      const loc = locateIssuePath(path, SNAPSHOT);
      expect(loc.kind).toBe("unlocatable");
      expect(loc.kind === "unlocatable" && loc.reason).toBe("task");
    }
  });

  it("保留任务：下标不是本次提交的编辑任务", () => {
    const loc = locateIssuePath("tasks[1].binding.config.points[0]", SNAPSHOT);
    expect(loc.kind === "unlocatable" && loc.reason).toBe("preserved");
    expect(loc.kind === "unlocatable" && loc.detail).toContain("保留任务");
  });

  it("无法识别：非 tasks 前缀 / 空 / 下标越界 / 任务下标越界", () => {
    // 连接级 issues（connection.validate_instance("connection", …)，core-api/src/lib.rs:712）
    for (const path of [
      "connection.host",
      "",
      "acquisition.tasks[0].binding.kind",
      "tasks[0].binding.config.points[9]",
      "tasks[0].binding.config.points[3]",
      "tasks[0].selections[9].parameters.axis",
      "tasks[5].binding.kind",
    ]) {
      const loc = locateIssuePath(path, SNAPSHOT);
      expect(loc.kind).toBe("unlocatable");
      expect(loc.kind === "unlocatable" && loc.reason).toBe("unrecognized");
    }
  });

  it("没有提交快照 ⇒ 一律不可定位（不拿实时列表下标去猜）", () => {
    const loc = locateIssuePath("tasks[0].binding.config.points[0]", null);
    expect(loc.kind === "unlocatable" && loc.reason).toBe("unrecognized");
    expect(loc.kind === "unlocatable" && loc.detail).toContain("快照");
  });
});

describe("辅助函数", () => {
  it("describeUnlocatable 三类原因各有中文说明，且不含原始 path", () => {
    const texts = [describeUnlocatable("task"), describeUnlocatable("preserved"), describeUnlocatable("unrecognized")];
    for (const text of texts) {
      expect(text).toContain("无法");
      expect(text).not.toContain("tasks[");
    }
    expect(new Set(texts).size).toBe(3);
  });

  it("firstLocatableIssue 跳过不可定位项，返回第一条可定位的", () => {
    const found = firstLocatableIssue(
      [
        { path: "tasks[0].binding.kind" },
        { path: "connection.host" },
        { path: "tasks[0].binding.config.points[2].parameters.column" },
      ],
      SNAPSHOT,
    );
    expect(found?.issue.path).toBe("tasks[0].binding.config.points[2].parameters.column");
    expect(found?.location.rowId).toBe("row-9");
    expect(found?.location.fieldKey).toBe("column");
  });

  it("firstLocatableIssue 全不可定位 ⇒ null（不猜）", () => {
    expect(firstLocatableIssue([{ path: "tasks[1].binding" }], SNAPSHOT)).toBeNull();
    expect(firstLocatableIssue([], SNAPSHOT)).toBeNull();
  });
});
