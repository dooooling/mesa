// 变量目录树（按后端 `category` 分桶）—— **纯模型**，无 React / AntD / 请求层。
//
// 数据来源只有一个：`GET /api/v1/drivers/{id}/definitions` 的每条
// `definitions[].category`（drivers/sinumerik-nck/src/definition_api.rs:571-576
// 的投影期附加字段）。本模块**只读**它，不解释它、不推断它、更不按
// `variable_id` 前缀自行归类。
//
// 为什么"零厂商规则"是硬约束：
// 分类前缀与分类并非一一对应（`channel.r_parameter` 是 `channel.` 前缀却属系统变量，
// `channel.machine_axis_assignment` 是 `channel.` 前缀却属机床数据，
// 见 docs/reference/nck-catalog-category-navigation-rule.md §4）。任何前端前缀表
// 都会在这两处静默给出**看似合理**的错误分类，而且用户无从发现。所以通用前端
// 对分类只有一个动作：**逐字采用后端给的 `category`**；拿不到就落"待分类"。
//
// 另一个必须显式呈现的事实：`category_basis` 本轮恒为 `"navigation_rule"`。
// 它说明分类是**驱动侧为建树临时制定的导航规则**，不是手册确认的事实。
// UI 必须让用户/开发者一眼看出这一点（见 `describeCategoryBasis`），否则
// "按分类找到的变量"会被误读成"手册说它属于这一类"。

/** 后端分类词表（`CatalogCategory::as_str()`，与驱动侧一一对应，**不新增第六个**）。 */
export const CATALOG_CATEGORY = {
  CNC_VARIABLE: "cnc_variable",
  SYSTEM_VARIABLE: "system_variable",
  MACHINE_DATA: "machine_data",
  DRIVE_PARAMETER: "drive_parameter",
  /** 其他 / 待分类：后端**明确**的失败关闭结论。 */
  UNCLASSIFIED: "unclassified",
} as const;

export type CatalogCategoryCode =
  (typeof CATALOG_CATEGORY)[keyof typeof CATALOG_CATEGORY];

/** 分类依据：导航规则（本轮唯一取值，`CATEGORY_BASIS_NAVIGATION_RULE`）。 */
export const CATEGORY_BASIS_NAVIGATION_RULE = "navigation_rule";

/** 分类依据的**自证**标签：方案 B（正式定义修订）落地后才会变成 `"definition"`。 */
export const CATEGORY_BASIS_DEFINITION = "definition";

/** 导航树分组顺序（与驱动侧 `CatalogCategory::ALL` 同序；待分类排最后）。 */
export const CATEGORY_ORDER: readonly CatalogCategoryCode[] = [
  CATALOG_CATEGORY.CNC_VARIABLE,
  CATALOG_CATEGORY.SYSTEM_VARIABLE,
  CATALOG_CATEGORY.MACHINE_DATA,
  CATALOG_CATEGORY.DRIVE_PARAMETER,
  CATALOG_CATEGORY.UNCLASSIFIED,
];

/** 分组标题（纯展示；文案与后端码的对应关系写死在这里，不做推理）。 */
export function categoryLabel(code: CatalogCategoryCode): string {
  switch (code) {
    case CATALOG_CATEGORY.CNC_VARIABLE:
      return "CNC 变量";
    case CATALOG_CATEGORY.SYSTEM_VARIABLE:
      return "系统变量";
    case CATALOG_CATEGORY.MACHINE_DATA:
      return "机床数据";
    case CATALOG_CATEGORY.DRIVE_PARAMETER:
      return "驱动参数";
    case CATALOG_CATEGORY.UNCLASSIFIED:
      return "其他 / 待分类";
  }
}

/** 分类依据的展示形态（标签 + 说明 + 是否"导航规则"）。 */
export interface CategoryBasisView {
  /** 原始值（缺失为 null）。 */
  raw: string | null;
  /** 短标签（角标用）。 */
  label: string;
  /** 完整说明（tooltip / 详情区一行）。 */
  hint: string;
  /** 是否为"驱动侧导航规则"（即：不是手册事实）。 */
  navigationRule: boolean;
}

/**
 * 描述 `category_basis`。
 *
 * 三种取值刻意分开：
 * - `navigation_rule` ⇒ 导航规则（不是手册事实，必须让用户看见）；
 * - `definition` ⇒ 定义事实（方案 B 之后才可能）；
 * - 缺失/未知 ⇒ "依据未知"，比"导航规则"更弱，同样不得当成事实。
 */
export function describeCategoryBasis(basis: unknown): CategoryBasisView {
  const raw = typeof basis === "string" && basis.trim() !== "" ? basis.trim() : null;
  if (raw === CATEGORY_BASIS_NAVIGATION_RULE) {
    return {
      raw,
      label: "导航规则",
      hint: "分类依据：导航规则（驱动侧为建树临时制定，不是手册确认的分类）",
      navigationRule: true,
    };
  }
  if (raw === CATEGORY_BASIS_DEFINITION) {
    return {
      raw,
      label: "定义事实",
      hint: "分类依据：定义修订里的正式字段（不是导航推断）",
      navigationRule: false,
    };
  }
  return {
    raw,
    label: "依据未知",
    hint: raw
      ? `分类依据：未知取值「${raw}」（不构成分类事实）`
      : "分类依据：未提供（不构成分类事实）",
    navigationRule: false,
  };
}

/** 目录条目里与建树相关的最小形态（宽松：允许注入测试只给子集）。 */
export interface CatalogTreeDefinitionLike {
  variable_id?: string;
  label?: unknown;
  category?: unknown;
  category_basis?: unknown;
}

/** 树里的一个可选项。 */
export interface CatalogTreeItem<T> {
  definition: T;
  variableId: string;
  /** 展示名（取自定义的 label；缺失时回落变量 ID）。 */
  label: string;
  /** 该条目落入的分组（缺失/非法/`unclassified` 一律落 `unclassified`）。 */
  bucket: CatalogCategoryCode;
  /** `category` 原始值（**未归一化**，便于详情区如实展示）。 */
  rawCategory: unknown;
  /**
   * 分类是否"已知"（落在四类之一）。
   *
   * `false` 有两种来源，UI 必须分别说清：
   * - 后端**明确**给出 `unclassified`（失败关闭结论，`explicitlyUnclassified`）；
   * - `category` 字段缺失或取值不在词表内（`explicitlyUnclassified === false`）。
   */
  categoryKnown: boolean;
  /** 后端明确说"无法归类"（唯一允许"不知道"的去处）。 */
  explicitlyUnclassified: boolean;
  /** 分类依据的可读形态（角标 + tooltip）。 */
  basis: CategoryBasisView;
}

/** 一个分组。 */
export interface CatalogTreeGroup<T> {
  code: CatalogCategoryCode;
  label: string;
  items: Array<CatalogTreeItem<T>>;
}

export interface CatalogTree<T> {
  groups: Array<CatalogTreeGroup<T>>;
  /** 参与建树的条目总数（= 各分组条目数之和）。 */
  total: number;
  /** 搜索命中的条目数（无搜索词时等于 total）。 */
  matched: number;
  /** 是否存在搜索词（决定分组是否强制展开）。 */
  searching: boolean;
  /** 四类（不含待分类）条目数；用于「分类已确认」的计数展示，**不含**待分类。 */
  classifiedCount: number;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 展示名：I-2 是 `{zh,en}`；注入测试允许直接字符串。 */
export function catalogItemLabel(definition: CatalogTreeDefinitionLike): string {
  const label = definition.label;
  if (typeof label === "string") return label;
  if (isPlainObject(label)) {
    const zh = typeof label.zh === "string" ? label.zh : "";
    const en = typeof label.en === "string" ? label.en : "";
    return zh || en;
  }
  return "";
}

/** 合法分类码判定（只认词表五项；其余一律不认——包括大小写变体）。 */
export function isCatalogCategoryCode(value: unknown): value is CatalogCategoryCode {
  return (
    typeof value === "string" &&
    (CATEGORY_ORDER as readonly string[]).includes(value)
  );
}

/**
 * 把一条定义归类到分组。
 *
 * 判定只有一条：**后端给了合法 `category` 就用它**；否则落 `unclassified`。
 * 这一层没有任何按 ID/前缀/label 的兜底——兜底就是"自行归类"，本轮明确禁止。
 */
export function classifyCatalogDefinition<T extends CatalogTreeDefinitionLike>(
  definition: T,
): CatalogTreeItem<T> {
  const variableId =
    typeof definition.variable_id === "string" ? definition.variable_id : "";
  const rawCategory = definition.category;
  const basis = describeCategoryBasis(definition.category_basis);
  const known = isCatalogCategoryCode(rawCategory);
  const bucket: CatalogCategoryCode = known
    ? (rawCategory as CatalogCategoryCode)
    : CATALOG_CATEGORY.UNCLASSIFIED;
  return {
    definition,
    variableId,
    label: catalogItemLabel(definition) || variableId,
    bucket,
    rawCategory,
    categoryKnown: known && bucket !== CATALOG_CATEGORY.UNCLASSIFIED,
    explicitlyUnclassified: rawCategory === CATALOG_CATEGORY.UNCLASSIFIED,
    basis,
  };
}

/** 搜索命中判定：变量 ID / 中文名 / 英文名，大小写不敏感，**跨全部分类**。 */
export function matchesCatalogQuery(
  item: CatalogTreeItem<CatalogTreeDefinitionLike>,
  query: string,
): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const variableId = item.variableId.toLowerCase();
  const zh = item.label.toLowerCase();
  const label = item.definition.label;
  const en =
    isPlainObject(label) && typeof label.en === "string" ? label.en.toLowerCase() : "";
  return variableId.includes(needle) || zh.includes(needle) || en.includes(needle);
}

/**
 * 建树。
 *
 * 分组恒为五项（含"其他 / 待分类"），空分组也保留——用户需要看到"这一类今天
 * 没有条目"，而不是让分组忽隐忽现。搜索词只过滤条目、不改变分组结构。
 */
export function buildCatalogTree<T extends CatalogTreeDefinitionLike>(
  definitions: readonly T[] | null | undefined,
  options: { query?: string } = {},
): CatalogTree<T> {
  const items = (definitions ?? []).map((definition) => classifyCatalogDefinition(definition));
  const query = options.query ?? "";
  const searching = query.trim() !== "";
  const matchedItems = items.filter((item) => matchesCatalogQuery(item, query));
  const groups: Array<CatalogTreeGroup<T>> = CATEGORY_ORDER.map((code) => ({
    code,
    label: categoryLabel(code),
    items: matchedItems.filter((item) => item.bucket === code),
  }));
  return {
    groups,
    total: items.length,
    matched: matchedItems.length,
    searching,
    classifiedCount: items.filter(
      (item) => item.categoryKnown && item.bucket !== CATALOG_CATEGORY.UNCLASSIFIED,
    ).length,
  };
}

/** 提交前的分类守卫结果（与 scope 准入**分开**，互不替代）。 */
export type CategoryGate =
  | { ok: true }
  | { ok: false; code: "CATEGORY_UNCLASSIFIED"; message: string };

/**
 * 分类守卫：后端**明确**说 `unclassified` 的条目不得被当成可采集。
 *
 * 为什么只拦"明确 unclassified"：
 * - `unclassified` 是驱动的失败关闭结论（未登记 ID / 证据锚点不成立 / 锚点漂移），
 *   此时连"这条定义属于哪一类对象"都无法确认，把它当采集点位就是把未知当已知；
 * - `category` 字段**缺失**（旧目录 / 非本字段的驱动）不代表"无法归类"，它只代表
 *   "驱动没有声明分类能力"。这类条目照常可用，但 UI 必须标注"分类未知"，且
 *   绝不把它算进四类之一（见 `classifyCatalogDefinition`）。
 *
 * 两层判定互不替代：可采集性仍由 `scope`（nckCollectability）决定，分类只回答
 * "它出现在导航树的哪个分支"。
 */
export function evaluateCategoryGate(
  item: CatalogTreeItem<CatalogTreeDefinitionLike> | null | undefined,
): CategoryGate {
  if (!item) {
    return {
      ok: false,
      code: "CATEGORY_UNCLASSIFIED",
      message: "分类未知：尚未选择变量，无法确认它在导航树中的归属",
    };
  }
  if (item.explicitlyUnclassified) {
    return {
      ok: false,
      code: "CATEGORY_UNCLASSIFIED",
      message:
        "分类未知（驱动侧失败关闭）：该条目被驱动归类为 unclassified，无法确认它属于哪一类对象，不得作为可采集点位提交",
    };
  }
  return { ok: true };
}
