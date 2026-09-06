// PR8 事件纯模型：history + live 按 Store seq 去重合并（不按 event_id）。
// EventStore 已完成 occurrence 去重；UI 只处理同一 Store row 被两边同时看见的情况。
import type { StoredEvent } from "../types";

/// UI 内存窗口上限（仅 UI window，历史事实仍在 DB）。
export const EVENT_LIST_WINDOW = 2000;

/** 合并历史与实时事件：按 seq 去重，结果 seq DESC 并截断到窗口上限。 */
export function mergeEvents(current: StoredEvent[], incoming: StoredEvent[]): StoredEvent[] {
  if (incoming.length === 0) return current;
  const seen = new Set<number>();
  for (const e of current) seen.add(e.seq);
  const merged = [...current];
  for (const e of incoming) {
    if (!seen.has(e.seq)) {
      seen.add(e.seq);
      merged.push(e);
    }
  }
  merged.sort((a, b) => b.seq - a.seq);
  return merged.length > EVENT_LIST_WINDOW ? merged.slice(0, EVENT_LIST_WINDOW) : merged;
}

/** 从一页历史响应中取出下一页游标（后端 next_cursor 直接用于 before_seq）。 */
export function nextPageCursor(nextCursor: number | null): number | null {
  return nextCursor;
}
