// PR8 Gate（纯函数）：history + live 按 seq 去重，结果 seq DESC。
import { describe, expect, it } from "vitest";
import { EVENT_LIST_WINDOW, mergeEvents, nextPageCursor } from "./model";
import { makeEvent } from "../test/fixtures";

describe("mergeEvents", () => {
  it("history 与 SSE 同一 seq 只出现一次", () => {
    const history = [makeEvent(3), makeEvent(2), makeEvent(1)];
    const merged = mergeEvents(history, [makeEvent(2), makeEvent(4)]);
    expect(merged.map((e) => e.seq)).toEqual([4, 3, 2, 1]);
  });

  it("结果保持 seq DESC（不按 occurred_at 重排）", () => {
    const a = makeEvent(1, { event: { occurred_at_ns: 9_999 } });
    const b = makeEvent(2, { event: { occurred_at_ns: 1 } });
    expect(mergeEvents([a], [b]).map((e) => e.seq)).toEqual([2, 1]);
  });

  it("空增量返回原数组", () => {
    const cur = [makeEvent(1)];
    expect(mergeEvents(cur, [])).toBe(cur);
  });

  it("内存列表截断到 UI 窗口（历史事实仍在 DB）", () => {
    const cur = Array.from({ length: EVENT_LIST_WINDOW }, (_, i) => makeEvent(EVENT_LIST_WINDOW - i));
    const merged = mergeEvents(cur, [makeEvent(EVENT_LIST_WINDOW + 1)]);
    expect(merged).toHaveLength(EVENT_LIST_WINDOW);
    expect(merged[0].seq).toBe(EVENT_LIST_WINDOW + 1);
  });

  it("分页游标直接透传后端 next_cursor", () => {
    expect(nextPageCursor(42)).toBe(42);
    expect(nextPageCursor(null)).toBeNull();
  });
});
