// 变量目录：**可搜索的紧凑树形下拉**（通用组件：不认识任何厂商、协议或变量名前缀）。
//
// 为什么从"常驻展开的树"改成下拉：
// - 36 条变量全部铺在抽屉里时，主流程（选变量 → 选对象 → 添加）会被几十行目录淹没；
// - 变量选择本身是"选一个"的动作，天然适合一个收起的选择器，而不是一面目录墙。
//
// 纪律：
// 1. 分类只读后端 `category`（四类 + 「其他 / 待分类」），本文件不写任何
//    `variable_id` 前缀判断；分类依据只作为分组标题上的小角标 + tooltip 呈现。
// 2. 默认**全部收起**：打开下拉后先只看到五个分组标题与计数；用户点分组才展开变量。
// 3. 搜索跨全部分类：命中即自动展开对应分组（搜索是显式动作，不是默认态）。
// 4. 叶子行只显示**名称 + 简短可用状态**；ID 放在 `title`/选中后的「变量详情」，
//    单位、手册位置、暂缓长文等细节不占列表（避免长文撑乱、右侧被裁切）。
//    判定与文案全部来自 `definitionMeta.ts`，本文件只排版——否则会出现
//    "树上写可用、表单说不可添加"的自相矛盾。
import { useEffect, useMemo, useRef, useState } from "react";
import { Input, Tag, Tooltip } from "antd";
import {
  CATALOG_CATEGORY,
  buildCatalogTree,
  catalogItemLabel,
  classifyCatalogDefinition,
  describeCategoryBasis,
  type CatalogCategoryCode,
  type CatalogTreeDefinitionLike,
  type CatalogTreeItem,
} from "../catalogTree";
import { AVAILABILITY_TAG_COLOR } from "../definitionMeta";

export interface CatalogCategoryTreeProps<T extends CatalogTreeDefinitionLike> {
  /** 待建树条目（父层已完成 scope 等准入过滤；本组件只负责分类导航）。 */
  definitions: readonly T[];
  /** 当前选中变量 ID（空串 = 未选）。 */
  selectedId: string;
  onSelect: (variableId: string) => void;
  /** 禁用（保存中等场景）；默认可用。 */
  disabled?: boolean;
  /** 未选中时的触发按钮文案。 */
  placeholder?: string;
  /** 搜索框 testid（默认 `catalog-tree-search`；父层可沿用既有契约）。 */
  searchTestId?: string;
  /** 搜索命中计数 testid（默认 `catalog-tree-search-count`）。 */
  searchCountTestId?: string;
  /** 触发按钮 testid（默认 `catalog-tree-trigger`）。 */
  triggerTestId?: string;
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
  selectedId,
  onSelect,
  disabled = false,
  placeholder = "选择变量",
  searchTestId = "catalog-tree-search",
  searchCountTestId = "catalog-tree-search-count",
  triggerTestId = "catalog-tree-trigger",
}: CatalogCategoryTreeProps<T>) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  // 折叠态按分组码记录：**默认全收起**（缺省即 false）。搜索时默认展开命中分组，
  // 但用户仍可手动收起（用 `!== false` 表达"搜索态默认展开"）。
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const rootRef = useRef<HTMLDivElement | null>(null);

  const tree = useMemo(() => buildCatalogTree(definitions, { query }), [definitions, query]);
  // 选中项从**未过滤**的定义集合里找：搜索词可能把已选项过滤掉，触发按钮仍要
  // 显示"当前选的是谁"（不能变成占位符，也不能误报未选择）。
  const selectedItem = useMemo(() => {
    const found = definitions.find((item) => (item.variable_id ?? "") === selectedId);
    return found ? classifyCatalogDefinition(found) : null;
  }, [definitions, selectedId]);
  // 搜索词可能把选中项过滤掉；触发按钮仍需显示"当前选的是谁"（不能变成占位符）。
  const selectedFallback = selectedId.trim();

  const groups = tree.searching
    ? tree.groups.filter((group) => group.items.length > 0)
    : tree.groups;

  const toggle = (code: CatalogCategoryCode) =>
    setExpanded((prev) => {
      const currently = tree.searching ? prev[code] !== false : prev[code] === true;
      return { ...prev, [code]: !currently };
    });

  // 点击外部关闭：下拉必须能"点到别处就收起"，否则会一直盖住对象选择区。
  useEffect(() => {
    if (!open) return;
    const onDocMouseDown = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocMouseDown);
    return () => document.removeEventListener("mousedown", onDocMouseDown);
  }, [open]);

  const choose = (variableId: string) => {
    onSelect(variableId);
    setOpen(false);
    // 选完清空搜索：下次打开看到完整分组，而不是上一次的过滤结果。
    setQuery("");
  };

  return (
    <div
      ref={rootRef}
      data-testid="catalog-tree"
      data-open={open ? "true" : "false"}
      onKeyDown={(event) => {
        if (event.key === "Escape") setOpen(false);
      }}
      style={{ position: "relative", display: "grid", gap: 4, minWidth: 0, width: "100%" }}
    >
      <button
        type="button"
        data-testid={triggerTestId}
        aria-haspopup="tree"
        aria-expanded={open}
        aria-label={selectedItem ? `采集变量：${selectedItem.label}` : placeholder}
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
        style={{
          width: "100%",
          minWidth: 0,
          display: "flex",
          alignItems: "center",
          gap: 6,
          padding: "3px 8px",
          border: "1px solid #d9d9d9",
          borderRadius: 4,
          background: disabled ? "#f5f5f5" : "#fff",
          cursor: disabled ? "not-allowed" : "pointer",
          textAlign: "left",
          fontSize: 12,
          color: "#262626",
        }}
      >
        <span
          style={{
            flex: "1 1 auto",
            minWidth: 0,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {selectedItem?.label ?? (selectedFallback || placeholder)}
        </span>
        {selectedItem ? (
          <Tag
            data-testid="catalog-tree-trigger-availability"
            data-availability={selectedItem.meta.availability.state}
            color={AVAILABILITY_TAG_COLOR[selectedItem.meta.availability.state]}
            style={{ marginInlineEnd: 0, flexShrink: 0, fontSize: 10, lineHeight: "16px" }}
          >
            {selectedItem.meta.availability.shortLabel}
          </Tag>
        ) : null}
        <span aria-hidden style={{ flexShrink: 0, color: "#8c8c8c" }}>
          {open ? "▴" : "▾"}
        </span>
      </button>

      {open ? (
        <div
          data-testid="catalog-tree-panel"
          style={{
            position: "absolute",
            zIndex: 20,
            left: 0,
            right: 0,
            top: "calc(100% + 4px)",
            display: "grid",
            gap: 6,
            padding: 8,
            background: "#fff",
            border: "1px solid #d9d9d9",
            borderRadius: 6,
            boxShadow: "0 6px 16px rgba(0, 0, 0, 0.12)",
            maxHeight: 360,
            overflowY: "auto",
            // 长变量名/角标一律换行或省略，**不产生横向滚动条**。
            overflowX: "hidden",
            minWidth: 0,
            boxSizing: "border-box",
          }}
        >
          <Input
            data-testid={searchTestId}
            aria-label="搜索变量"
            placeholder="搜索变量名称或 ID（跨全部分类）"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            allowClear
            size="small"
            style={{ width: "100%", minWidth: 0 }}
          />
          {tree.searching ? (
            <div
              data-testid="catalog-tree-search-summary"
              style={{ fontSize: 11, color: "#525252" }}
            >
              {tree.matched
                ? `跨分类匹配 ${tree.matched} 个变量（搜索不改变分类，只过滤条目）`
                : "没有匹配的变量（可改用变量 ID 搜索）"}
            </div>
          ) : null}
          {tree.searching ? (
            <div data-testid={searchCountTestId} style={{ fontSize: 11, color: "#525252" }}>
              {tree.matched ? `匹配 ${tree.matched} 个变量` : "没有匹配的变量（可改用变量 ID 搜索）"}
            </div>
          ) : null}

          <div style={{ display: "grid", gap: 2, minWidth: 0 }}>
            {groups.map((group) => {
              const isExpanded = tree.searching
                ? expanded[group.code] !== false
                : expanded[group.code] === true;
              const groupBases = basisTags(group.items);
              return (
                <div
                  key={group.code}
                  data-testid={`catalog-tree-group-${group.code}`}
                  data-expanded={isExpanded ? "true" : "false"}
                  data-category-known={
                    group.code === CATALOG_CATEGORY.UNCLASSIFIED ? "false" : "true"
                  }
                  style={{ minWidth: 0 }}
                >
                  <div style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
                    <button
                      type="button"
                      data-testid={`catalog-tree-toggle-${group.code}`}
                      aria-expanded={isExpanded}
                      title={groupBases.map((basis) => basis.hint).join(" / ") || undefined}
                      onClick={() => toggle(group.code)}
                      style={{
                        fontSize: 12,
                        background: "none",
                        border: "none",
                        padding: 0,
                        cursor: "pointer",
                        color: "#262626",
                        minWidth: 0,
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {isExpanded ? "▾" : "▸"} {group.label}
                    </button>
                    <span
                      data-testid={`catalog-tree-group-count-${group.code}`}
                      style={{ fontSize: 11, color: "#8c8c8c", flexShrink: 0 }}
                    >
                      {group.items.length}
                    </span>
                    {group.code === CATALOG_CATEGORY.UNCLASSIFIED ? (
                      <Tooltip title="这一类是「无法确认归属」的去处：驱动明确判定 unclassified 的条目不得作为可采集提交；category 字段缺失则只标注「分类未知」。">
                        <Tag
                          color="orange"
                          data-testid="catalog-tree-unclassified-hint"
                          style={{ fontSize: 10, lineHeight: "16px", marginInlineEnd: 0 }}
                        >
                          分类未知
                        </Tag>
                      </Tooltip>
                    ) : null}
                    {/*
                      分类依据只给「其他 / 待分类」一个可见角标：其余四类的
                      "导航规则"说明收进分组标题的 tooltip，避免每行都挂技术标签。
                    */}
                    {group.code === CATALOG_CATEGORY.UNCLASSIFIED && groupBases.length
                      ? groupBases.map((basis) => (
                          <Tooltip key={basis.label} title={basis.hint}>
                            <span
                              data-testid={`catalog-tree-basis-${group.code}`}
                              data-basis={basis.raw ?? ""}
                              style={{
                                fontSize: 10,
                                color: basis.navigationRule ? "#8c8c8c" : "#1677ff",
                                flexShrink: 0,
                                cursor: "help",
                              }}
                            >
                              {basis.label}
                            </span>
                          </Tooltip>
                        ))
                      : null}
                  </div>

                  {isExpanded ? (
                    <div style={{ display: "grid", gap: 1, paddingLeft: 12, minWidth: 0 }}>
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
                          onSelect={choose}
                        />
                      ))}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        </div>
      ) : null}
    </div>
  );
}

/**
 * 一个叶子：**只显示名称 + 简短可用状态**。
 *
 * 为什么不再把单位/手册/暂缓原因铺在行内：那些是"要查的时候才看"的细节，
 * 长文会把一行撑成多行并导致右侧裁切。ID 放进 `title` 与选中后的「变量详情」，
 * 单位/手册位置/暂缓原因在表单里按需展开——列表保持可扫描。
 */
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
  const { availability } = item.meta;
  return (
    <button
      type="button"
      data-testid={`catalog-tree-leaf-${item.variableId}`}
      data-category={item.bucket}
      data-category-known={item.categoryKnown ? "true" : "false"}
      data-explicitly-unclassified={item.explicitlyUnclassified ? "true" : "false"}
      data-selected={selected ? "true" : "false"}
      data-availability={availability.state}
      title={item.variableId}
      disabled={disabled || item.variableId === ""}
      onClick={() => onSelect(item.variableId)}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        flexWrap: "wrap",
        textAlign: "left",
        background: selected ? "#e6f4ff" : "none",
        border: "none",
        borderRadius: 2,
        padding: "2px 4px",
        cursor: disabled ? "not-allowed" : "pointer",
        fontSize: 12,
        color: "#262626",
        minWidth: 0,
        width: "100%",
        boxSizing: "border-box",
      }}
    >
      <span
        style={{
          flex: "1 1 120px",
          minWidth: 0,
          overflowWrap: "anywhere",
        }}
      >
        {catalogItemLabel(item.definition) || item.variableId}
      </span>
      <Tag
        data-testid={`catalog-tree-leaf-availability-${item.variableId}`}
        data-availability={availability.state}
        color={AVAILABILITY_TAG_COLOR[availability.state]}
        style={{ fontSize: 10, lineHeight: "16px", marginInlineEnd: 0, flexShrink: 0 }}
      >
        {availability.shortLabel}
      </Tag>
    </button>
  );
}
