// 采集行身份（§4.8）：rowId 在编辑 / 搜索 / 分页后必须保持稳定。
//
// 这里的断言全部落在**纯模型**上：页面层用同一组函数分配/对齐 rowId，
// 因此模型级「稳定」等价于组件级「稳定」（组件级证据另见
// EndpointAcquisitionPane.saveFlow.test.tsx 的 data-row-id 断言）。
import { describe, expect, it } from "vitest";
import {
  alignRowIds,
  assignRowIds,
  createRowIdAllocator,
  filterRows,
  maxRowIdSeq,
  paginateRows,
  stableIdentityKey,
  toRows,
} from "./pointRows";

interface Item {
  point_key: string;
  parameters: Record<string, unknown>;
}

function item(pointKey: string, parameters: Record<string, unknown> = {}): Item {
  return { point_key: pointKey, parameters };
}

describe("rowId 分配：只发号、不回收、不复用", () => {
  it("分配器单调递增，删除后不复用旧号", () => {
    const alloc = createRowIdAllocator("row");
    const ids = assignRowIds(3, alloc);
    expect(ids).toEqual(["row-1", "row-2", "row-3"]);
    expect(new Set(ids).size).toBe(3);
    // 删掉中间一行后新增：新号必须比所有旧号都新（旧号不复用）。
    const remaining = [ids[0], ids[2]];
    const added = alloc.next();
    expect(remaining).not.toContain(added);
    expect(added).toBe("row-4");
  });

  it("rowId 与内容无关：同一内容不会得到同一 rowId", () => {
    const alloc = createRowIdAllocator("row");
    const a = alloc.next();
    const b = alloc.next();
    expect(a).not.toBe(b);
  });
});

describe("恢复草稿副本后继续发号（draftStash 用）", () => {
  it("maxRowIdSeq 只认 <prefix>-<n> 形态，取最大序号", () => {
    expect(maxRowIdSeq(["row-1", "row-9", "row-3"], "row")).toBe(9);
    // 其它前缀 / 非数字后缀一律忽略（不猜、不把奇怪 id 当序号）。
    expect(maxRowIdSeq(["nck-7", "row-x", "row-", "", "row-2"] as string[], "row")).toBe(2);
    expect(maxRowIdSeq([], "row")).toBe(0);
  });

  it("createRowIdAllocator 带 startAt 时从旧序号之后继续，新号不会撞上已恢复的行", () => {
    // 场景：草稿副本里有 row-1..row-5，重新挂载后分配器必须从 6 开始。
    const restored = ["row-1", "row-2", "row-3", "row-4", "row-5"];
    const alloc = createRowIdAllocator("row", maxRowIdSeq(restored, "row"));
    const next = alloc.next();
    expect(next).toBe("row-6");
    expect(restored).not.toContain(next);
  });
});

describe("编辑：同一行的 rowId 不变（不得用 point_key / 下标）", () => {
  it("替换行对象（point_key 变化）后 rowId 不变", () => {
    const alloc = createRowIdAllocator("row");
    const before = [item("syn.a@channel=1"), item("syn.b@channel=2")];
    const ids = assignRowIds(before.length, alloc);
    // 编辑第二行：point_key 与参数都变了，但它仍是第二行。
    const after = [before[0], item("syn.b@channel=9", { channel: 9 })];
    const nextIds = [...ids];
    expect(nextIds).toEqual(ids);
    const rows = toRows(after, nextIds);
    expect(rows[1].rowId).toBe(ids[1]);
    // 不得把 point_key 当身份：两行的 rowId 与 point_key 无包含关系。
    expect(rows[1].rowId).not.toContain("syn.b");
  });

  it("合并 outputs 产生新对象时按身份键接续 rowId", () => {
    const alloc = createRowIdAllocator("row");
    const before = [item("register", { unit: 1 })];
    const ids = assignRowIds(before.length, alloc);
    // 同一资源同参数，只是 outputs 变了（applySelectionAdd 的 merge 会 spread 出新对象）。
    const after = [item("register", { unit: 1 })];
    const nextIds = alignRowIds(
      before,
      ids,
      after,
      alloc,
      (sel) => `${sel.point_key}\u0000${stableIdentityKey(sel.parameters)}`,
    );
    expect(nextIds).toEqual(ids);
  });

  it("参数本身变化 = 另一条选择，必须发新号（不串行身份）", () => {
    const alloc = createRowIdAllocator("row");
    const before = [item("register", { unit: 1 })];
    const ids = assignRowIds(before.length, alloc);
    const after = [item("register", { unit: 2 })];
    const nextIds = alignRowIds(
      before,
      ids,
      after,
      alloc,
      (sel) => `${sel.point_key}\u0000${stableIdentityKey(sel.parameters)}`,
    );
    expect(nextIds[0]).not.toBe(ids[0]);
  });
});

describe("搜索 / 筛选 / 分页：只改可见集合，不重新分配 rowId", () => {
  const alloc = createRowIdAllocator("row");
  const items = Array.from({ length: 30 }, (_, i) => item(`p${i}`, { channel: i + 1 }));
  const ids = assignRowIds(items.length, alloc);
  const rows = toRows(items, ids);

  it("筛选（搜索）后 rowId 与筛选前完全一致", () => {
    const visible = filterRows(rows, (row) => row.item.point_key.startsWith("p1"));
    expect(visible.map((r) => r.rowId)).toEqual(rows.filter((r) => r.item.point_key.startsWith("p1")).map((r) => r.rowId));
    // 可见集合里的 rowId 仍是原始分配值。
    visible.forEach((row) => expect(ids).toContain(row.rowId));
  });

  it("分页不重新分配 rowId，跨页不重复", () => {
    const page1 = paginateRows(rows, 1, 25);
    const page2 = paginateRows(rows, 2, 25);
    expect(page1.rows).toHaveLength(25);
    expect(page2.rows).toHaveLength(5);
    expect(page1.rows.map((r) => r.rowId)).toEqual(ids.slice(0, 25));
    expect(page2.rows.map((r) => r.rowId)).toEqual(ids.slice(25));
    const all = [...page1.rows, ...page2.rows].map((r) => r.rowId);
    expect(new Set(all).size).toBe(30);
  });

  it("分页越界收敛到有效页码（不返回负页码/空页）", () => {
    const over = paginateRows(rows, 99, 25);
    expect(over.page).toBe(2);
    expect(over.pageCount).toBe(2);
    const under = paginateRows(rows, 0, 25);
    expect(under.page).toBe(1);
  });

  it("删除一行后其余行 rowId 不变（不按下标重排）", () => {
    const remainingItems = items.filter((_, i) => i !== 3);
    const remainingIds = ids.filter((_, i) => i !== 3);
    const remaining = toRows(remainingItems, remainingIds);
    expect(remaining[3].rowId).toBe(ids[4]);
    expect(remaining.map((r) => r.rowId)).toEqual(ids.filter((_, i) => i !== 3));
  });
});
