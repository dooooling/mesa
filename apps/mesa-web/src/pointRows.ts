// 采集点位行的稳定身份（§4.8，纯函数、无 React/AntD、无驱动特判）。
//
// 为什么行 key 不能用数组下标或 `point_key`：
// - 下标：删除/插入一行之后，同一个 key 指向的是**另一行**——编辑态、折叠态、
//   行内问题标记会串到别的点位上；分页/搜索改变可见集合时同样错位。
// - `point_key`：编辑会改 key（改 Target/参数即改身份）；非 NCK 同一资源多输出时
//   key 由输出决定。用它做 key，React 会把「同一行的编辑」当成「新行」，
//   于是行被重建、位置被顶走——正是本次要修的问题（§6-14）。
//
// `rowId` 的边界（§4.8）：
// - 页面层在**加载快照 / 新增行**时分配一次，之后不随内容变化；
// - 删除后不回收、不复用（新行永远拿新 id）；
// - **只用于界面定位**：不参与配置差异判定（差异只比有效配置与顺序）。
//   本模块的 `stableIdentityKey` 只服务「同一行换了对象后接续 rowId」，
//   与配置差异判定无关，不得被当成配置指纹。

/** 行身份：`rowId` 与它指向的数据项（数据项本身不携带任何身份语义）。 */
export interface PointRow<T> {
  rowId: string;
  item: T;
}

/** 单调递增的 rowId 分配器：只发号，不回收已释放的号。 */
export interface RowIdAllocator {
  next: () => string;
}

/**
 * 创建分配器。`prefix` 只用于调试可读性（如 `nck` / `sel`），不参与判定。
 *
 * 刻意不做「按内容哈希」：内容变化（编辑）不得改变 rowId。
 *
 * `startAt`：从既有 rowId 续号。用于「草稿副本在路由之外保存、重挂载后恢复」的
 * 场景（`draftStash.tsx`）：恢复出来的行沿用**原来的** rowId，新分配器必须
 * 从最大序号之后继续，否则新增行会撞上已恢复的 id（React key 冲突 / 编辑态串行）。
 */
export function createRowIdAllocator(prefix = "row", startAt = 0): RowIdAllocator {
  let seq = Number.isFinite(startAt) ? Math.max(0, Math.floor(startAt)) : 0;
  return {
    next: () => {
      seq += 1;
      return `${prefix}-${seq}`;
    },
  };
}

/**
 * 既有 rowId 集合里的最大序号（`<prefix>-<n>` 形态；其它形态忽略）。
 *
 * 与 `createRowIdAllocator(prefix, maxRowIdSeq(ids, prefix))` 配对使用，
 * 只服务「恢复草稿副本后继续发号」，不参与任何配置判定。
 */
export function maxRowIdSeq(ids: readonly string[], prefix = "row"): number {
  let max = 0;
  for (const id of ids) {
    if (typeof id !== "string" || !id.startsWith(`${prefix}-`)) continue;
    const tail = id.slice(prefix.length + 1);
    if (!/^[0-9]+$/.test(tail)) continue;
    const n = Number(tail);
    if (Number.isFinite(n) && n > max) max = n;
  }
  return max;
}

/** 首次加载：按顺序为新行分配 rowId（顺序即服务端/草稿顺序）。 */
export function assignRowIds(count: number, alloc: RowIdAllocator): string[] {
  return Array.from({ length: Math.max(0, count) }, () => alloc.next());
}

/**
 * 对齐既有 rowId：**同一对象引用**沿用旧 id；可选按 `identityOf` 给出的稳定
 * 身份键继续匹配（同一行被替换成新对象、但仍是同一行时用它，例如非 NCK 把
 * 新勾选的输出合并进已有选择时会产生新对象）；其余新对象分配新 id。
 *
 * 被删掉的 id 不会被复用（分配器只递增）。
 */
export function alignRowIds<T>(
  prevItems: readonly T[],
  prevIds: readonly string[],
  nextItems: readonly T[],
  alloc: RowIdAllocator,
  identityOf?: (item: T) => string,
): string[] {
  const byItem = new Map<T, string>();
  prevItems.forEach((item, i) => {
    const id = prevIds[i];
    // 重复引用（同一对象出现两次）只认第一次，第二份单独发新号，避免两行同 key。
    if (id !== undefined && !byItem.has(item)) byItem.set(item, id);
  });
  const byIdentity = new Map<string, string>();
  if (identityOf) {
    prevItems.forEach((item, i) => {
      const id = prevIds[i];
      if (id === undefined) return;
      const key = identityOf(item);
      if (!byIdentity.has(key)) byIdentity.set(key, id);
    });
  }
  const used = new Set<string>();
  return nextItems.map((item) => {
    const byRef = byItem.get(item);
    if (byRef !== undefined && !used.has(byRef)) {
      used.add(byRef);
      return byRef;
    }
    const key = identityOf?.(item);
    const byKey = key !== undefined ? byIdentity.get(key) : undefined;
    if (byKey !== undefined && !used.has(byKey)) {
      used.add(byKey);
      return byKey;
    }
    return alloc.next();
  });
}

/** 把并行数组组合成带身份的行（供展示层/测试使用）。 */
export function toRows<T>(items: readonly T[], ids: readonly string[]): PointRow<T>[] {
  return items.map((item, i) => ({ rowId: ids[i] ?? "", item }));
}

/**
 * 生成**身份键**用的稳定序列化（对象 key 排序、数组保序、`undefined` 丢弃）。
 *
 * 只用于 rowId 对齐（同一行换了对象、但仍是同一行），**不得**用于配置差异
 * 判定——差异判定只比有效配置与顺序（§4.8）。
 */
export function stableIdentityKey(value: unknown): string {
  return JSON.stringify(canonicalizeForIdentity(value));
}

function canonicalizeForIdentity(value: unknown): unknown {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(canonicalizeForIdentity);
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(value as Record<string, unknown>).sort()) {
    const v = canonicalizeForIdentity((value as Record<string, unknown>)[key]);
    if (v !== undefined) out[key] = v;
  }
  return out;
}

/**
 * 搜索/筛选：只改变可见集合，不重新分配 rowId（§4.8「排序、过滤只改变可见集合」）。
 */
export function filterRows<T>(
  rows: readonly PointRow<T>[],
  predicate: (row: PointRow<T>) => boolean,
): PointRow<T>[] {
  return rows.filter(predicate);
}

/**
 * 分页：只切片，不重新分配 rowId。页码越界时收敛到有效范围（不返回负页码）。
 *
 * 与 `paginateItems` 同一实现：行身份与分页无关，分页只需要「按顺序切片」。
 */
export function paginateRows<T>(
  rows: readonly PointRow<T>[],
  page: number,
  pageSize: number,
): { rows: PointRow<T>[]; page: number; pageCount: number; total: number } {
  return paginateItems(rows, page, pageSize);
}

/** 通用分页（任意元素数组；顺序即顺序，不做排序、不重排）。 */
export function paginateItems<T>(
  items: readonly T[],
  page: number,
  pageSize: number,
): { rows: T[]; page: number; pageCount: number; total: number } {
  const total = items.length;
  const size = Math.max(1, Math.floor(pageSize) || 1);
  const pageCount = Math.max(1, Math.ceil(total / size));
  const safePage = Math.min(Math.max(1, Math.floor(page) || 1), pageCount);
  const start = (safePage - 1) * size;
  return { rows: items.slice(start, start + size), page: safePage, pageCount, total };
}
