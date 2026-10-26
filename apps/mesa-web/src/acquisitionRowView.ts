// 采集点位行的**视图模型 + 查询**（纯函数，无 React/AntD，无任何驱动判断）。
//
// 为什么单独一层（§2.8）：页面层保留「按驱动分流」的判断，但把结果归一成
// `PointRowView` 后交给通用组件（表格/搜索/分页/抽屉）。通用组件只认这个结构，
// 因此它们内部不可能出现驱动名、协议字段名或资源 ID 判断。
//
// 行的辨识口径（§2.6）：
// - `title` 只能来自既有事实（定义/资源 label，或通用名）；
// - `summary` 是「字段标签 + 值」摘要（地址摘要也走这条）；
// - `identifier` 只在**名称不足或同页重名**时才有值（允许展示必要标识）；
// - `searchText` 收集标题、摘要与标识（含完整点键），保证「用标识也能搜到」。
//
// 本模块不做身份/准入判定，也不决定某行是否可编辑：判定仍在页面层与既有模型里。

/** 行级标记：只标**需要动作**的两态；正常/成功/已保存不占视觉（§2.9）。 */
export type RowMarker = "added" | "modified";

/** 技术详情的一行（标签 + 值）。完整点键/原始 JSON/版本/来源都只放这里。 */
export interface PointDetailEntry {
  label: string;
  value: string;
  /** 等宽字体（点键、JSON）。 */
  mono?: boolean;
}

export interface PointRowView {
  /** 稳定行标识：加载/新增时分配一次，不随内容变化（§4.8）。 */
  rowId: string;
  /** 名称或通用名。 */
  title: string;
  /** 配置摘要（字段标签 + 值）。 */
  summary: string;
  /** 名称不足/重名时补充的必要标识（默认不渲染）。 */
  identifier?: string;
  /** 补充标识的候选值（通常是点键）；只在需要区分时才出现在界面上。 */
  identifierHint?: string;
  /** 搜索文本：标题 + 摘要 + 标识 + 完整点键（大小写不敏感包含匹配）。 */
  searchText: string;
  marker?: RowMarker;
  /** 真实问题（仅在确有问题时存在；正常行没有）。 */
  issue?: { text: string; detail?: string };
  /** 技术详情（主动展开才看得到）。 */
  detail: PointDetailEntry[];
}

/** 点位筛选：只有「全部」常驻；其余两项按实际数据出现（§2.7）。 */
export type PointFilter = "all" | "issue" | "modified";

export interface PointFilterOption {
  value: PointFilter;
  label: string;
}

/**
 * 生成筛选项：**从实际数据来**，不写死任何分类。
 *
 * 只有确实存在问题行/已修改行时才出现对应入口；没有问题时用户看不到这两个
 * 选项（默认页因此保持安静，§2.1）。
 */
export function buildFilterOptions(input: {
  issueCount: number;
  modifiedCount: number;
}): PointFilterOption[] {
  const options: PointFilterOption[] = [{ value: "all", label: "全部" }];
  if (input.issueCount > 0) {
    options.push({ value: "issue", label: `有问题（${input.issueCount}）` });
  }
  if (input.modifiedCount > 0) {
    options.push({ value: "modified", label: `已修改（${input.modifiedCount}）` });
  }
  return options;
}

/** 搜索词归一：去首尾空白 + 小写（包含匹配、大小写不敏感，§2.7）。 */
export function normalizeNeedle(query: string): string {
  return query.trim().toLowerCase();
}

/** 单行是否命中搜索词（标题、摘要、标识、点键）。 */
export function matchesPointQuery(row: PointRowView, needle: string): boolean {
  if (!needle) return true;
  const haystack = [
    row.title,
    row.summary,
    row.identifier ?? "",
    row.identifierHint ?? "",
    row.searchText,
  ]
    .join("\n")
    .toLowerCase();
  return haystack.includes(needle);
}

/** 筛选谓词：`all` 恒真；`issue`/`modified` 只看行级真实状态。 */
export function matchesPointFilter(row: PointRowView, filter: PointFilter): boolean {
  if (filter === "issue") return !!row.issue;
  if (filter === "modified") return row.marker === "added" || row.marker === "modified";
  return true;
}

/**
 * 搜索 + 筛选（只改变可见集合，**不重新分配** rowId，§4.8）。
 * 顺序保持原顺序：分页/筛选/搜索都不重排点位（顺序是配置的一部分）。
 */
export function applyRowQuery(
  rows: readonly PointRowView[],
  options: { query?: string; filter?: PointFilter } = {},
): PointRowView[] {
  const needle = normalizeNeedle(options.query ?? "");
  const filter = options.filter ?? "all";
  return rows.filter((row) => matchesPointQuery(row, needle) && matchesPointFilter(row, filter));
}

/** 行级问题/标记计数（筛选项与摘要都用它，口径唯一）。 */
export function countRowStates(rows: readonly PointRowView[]): {
  issueCount: number;
  modifiedCount: number;
} {
  let issueCount = 0;
  let modifiedCount = 0;
  for (const row of rows) {
    if (row.issue) issueCount += 1;
    if (row.marker) modifiedCount += 1;
  }
  return { issueCount, modifiedCount };
}

/**
 * 补齐「名称 + 摘要」无法区分的行（§2.6-7：任意两行必须能靠名称+摘要区分）。
 *
 * 判定口径刻意保守：只有**同页出现完全相同的名称+摘要**，或名称为空时才补标识。
 * 不给所有行都加标识——那会把「必要标识」变成常驻噪音（§2.9）。
 */
export function ensureDistinguishable(
  rows: readonly PointRowView[],
): PointRowView[] {
  const groups = new Map<string, number>();
  for (const row of rows) {
    const key = `${row.title}\u0000${row.summary}`;
    groups.set(key, (groups.get(key) ?? 0) + 1);
  }
  return rows.map((row) => {
    const key = `${row.title}\u0000${row.summary}`;
    const duplicated = (groups.get(key) ?? 0) > 1;
    const unnamed = row.title.trim() === "";
    if (!duplicated && !unnamed) return row;
    const identifier = row.identifier ?? row.identifierHint;
    return identifier ? { ...row, identifier } : row;
  });
}

/** 点键的短尾段（用作必要标识时的可读形态）。 */
export function shortPointKey(pointKey: string): string {
  const trimmed = pointKey.trim();
  if (!trimmed) return "";
  const parts = trimmed.split(/[./]/);
  return parts.length > 1 ? parts.slice(-2).join(".") : trimmed;
}
