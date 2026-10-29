// 变量目录树（**通用**组件：不认识任何厂商、协议或变量名前缀）。
//
// 只做四件事：
// 1. 按后端 `category` 分五个分组（四类 + 「其他 / 待分类」）；
// 2. 搜索（跨所有分类，命中即展开）；
// 3. 展开 / 折叠 / 选择；
// 4. 把 `category_basis` 如实标出来（角标 + tooltip），让"这是导航规则，
//    不是手册事实"随时可查——但不喧宾夺主（只在分组标题上给一个小角标）。
//
// 分类判定全部在 `catalogTree.ts` 里（纯函数、可单测）；本文件不写任何
// `variable_id` 前缀判断，也不认识 `cnc_variable` 之外的世界。
import { useMemo, useState } from "react";
import { Tag, Tooltip } from "antd";
import {
  CATALOG_CATEGORY,
  buildCatalogTree,
  catalogItemLabel,
  describeCategoryBasis,
  type CatalogCategoryCode,
  type CatalogTreeDefinitionLike,
  type CatalogTreeItem,
} from "../catalogTree";

export interface CatalogCategoryTreeProps<T extends CatalogTreeDefinitionLike> {
  /** 待建树条目（父层已完成 scope 等准入过滤；本组件只负责分类导航）。 */
  definitions: readonly T[];
  /** 搜索词（与父层共用同一个输入框；空串即不过滤）。 */
  query?: string;
  /** 当前选中变量 ID（空串 = 未选）。 */
  selectedId: string;
  onSelect: (variableId: string) => void;
  /** 禁用（保存中等场景）；默认可用。 */
  disabled?: boolean;
}

/** 分组内分类依据的角标（同一分组内依据应一致；不一致时逐个展示，不合并猜测）。 */
function basisTags<T>(items: Array<CatalogTreeItem<T>>) {
  const views = new Map<string, ReturnType<typeof describeCategoryBasis>>();
  for (const item of items) {
    const key = item.basis.raw ?? "\u0000missing";
    if (!views.has(key)) views.set(key, item.basis);
  }
  return [...views.values()];
}

export function CatalogCategoryTree<T extends CatalogTreeDefinitionLike>({
  definitions,
  query = "",
  selectedId,
  onSelect,
  disabled = false,
}: CatalogCategoryTreeProps<T>) {
  const tree = useMemo(() => buildCatalogTree(definitions, { query }), [definitions, query]);
  // 折叠态按分组码记录；默认全部展开（42 条规模下"先看见全貌"更有用）。
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const toggle = (code: CatalogCategoryCode) =>
    setCollapsed((prev) => ({ ...prev, [code]: !prev[code] }));

  const groups = tree.searching
    ? tree.groups.filter((group) => group.items.length > 0)
    : tree.groups;

  return (
    <div data-testid="catalog-tree" style={{ display: "grid", gap: 4 }}>
      {tree.searching ? (
        <div data-testid="catalog-tree-search-summary" style={{ fontSize: 11, color: "#525252" }}>
          {tree.matched
            ? `跨分类匹配 ${tree.matched} 个变量（搜索不改变分类，只过滤条目）`
            : "没有匹配的变量（可改用变量 ID 搜索）"}
        </div>
      ) : null}

      {groups.map((group) => {
        const isCollapsed = collapsed[group.code] === true;
        return (
          <div
            key={group.code}
            data-testid={`catalog-tree-group-${group.code}`}
            data-expanded={isCollapsed ? "false" : "true"}
            data-category-known={group.code === CATALOG_CATEGORY.UNCLASSIFIED ? "false" : "true"}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <button
                type="button"
                data-testid={`catalog-tree-toggle-${group.code}`}
                aria-expanded={!isCollapsed}
                onClick={() => toggle(group.code)}
                style={{
                  fontSize: 12,
                  background: "none",
                  border: "none",
                  padding: 0,
                  cursor: "pointer",
                  color: "#262626",
                }}
              >
                {isCollapsed ? "▸" : "▾"} {group.label}
              </button>
              <span
                data-testid={`catalog-tree-group-count-${group.code}`}
                style={{ fontSize: 11, color: "#8c8c8c" }}
              >
                {group.items.length}
              </span>
              {group.code === CATALOG_CATEGORY.UNCLASSIFIED ? (
                <Tooltip title="这一类是「无法确认归属」的去处：驱动明确判定 unclassified 的条目不得作为可采集提交；category 字段缺失则只标注「分类未知」。">
                  <Tag color="orange" data-testid="catalog-tree-unclassified-hint">
                    分类未知
                  </Tag>
                </Tooltip>
              ) : null}
              {/* 分类依据：角标 + tooltip（不喧宾夺主，但随时可查）。 */}
              {group.items.length
                ? basisTags(group.items).map((basis) => (
                    <Tooltip key={basis.label} title={basis.hint}>
                      <Tag
                        data-testid={`catalog-tree-basis-${group.code}`}
                        data-basis={basis.raw ?? ""}
                        color={basis.navigationRule ? undefined : "blue"}
                        style={{ fontSize: 10, lineHeight: "16px", marginInlineEnd: 0 }}
                      >
                        {basis.label}
                      </Tag>
                    </Tooltip>
                  ))
                : null}
            </div>

            {isCollapsed ? null : (
              <div style={{ display: "grid", gap: 2, paddingLeft: 14 }}>
                {group.items.length === 0 ? (
                  <span
                    data-testid={`catalog-tree-empty-${group.code}`}
                    style={{ fontSize: 11, color: "#8c8c8c" }}
                  >
                    该分类当前没有条目
                  </span>
                ) : null}
                {group.items.map((item) => (
                  <ItemRow
                    key={item.variableId || item.label}
                    item={item}
                    selected={item.variableId === selectedId}
                    disabled={disabled}
                    onSelect={onSelect}
                  />
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function ItemRow<T extends CatalogTreeDefinitionLike>({
  item,
  selected,
  disabled,
  onSelect,
}: {
  item: CatalogTreeItem<T>;
  selected: boolean;
  disabled: boolean;
  onSelect: (variableId: string) => void;
}) {
  const unknown = !item.categoryKnown;
  return (
    <button
      type="button"
      data-testid={`catalog-tree-leaf-${item.variableId}`}
      data-category={item.bucket}
      data-category-known={item.categoryKnown ? "true" : "false"}
      data-selected={selected ? "true" : "false"}
      disabled={disabled || item.variableId === ""}
      onClick={() => onSelect(item.variableId)}
      style={{
        display: "flex",
        alignItems: "baseline",
        gap: 6,
        textAlign: "left",
        background: selected ? "#e6f4ff" : "none",
        border: "none",
        borderRadius: 2,
        padding: "2px 4px",
        cursor: disabled ? "not-allowed" : "pointer",
        fontSize: 12,
        color: "#262626",
        minWidth: 0,
      }}
    >
      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
        {catalogItemLabel(item.definition) || item.variableId}
      </span>
      <span style={{ fontSize: 10, color: "#8c8c8c", flexShrink: 0 }}>{item.variableId}</span>
      {unknown ? (
        <Tag
          color="orange"
          data-testid={`catalog-tree-leaf-badge-${item.variableId}`}
          style={{ fontSize: 10, lineHeight: "16px", marginInlineEnd: 0, flexShrink: 0 }}
        >
          {item.explicitlyUnclassified ? "分类未知（驱动 unclassified）" : "分类未知"}
        </Tag>
      ) : null}
    </button>
  );
}
