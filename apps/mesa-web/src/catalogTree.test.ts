// 分类树纯模型单测：分组**只读**后端 `category`，未知一律落待分类，绝不猜。
//
// 这些用例钉的是"通用前端零厂商规则"这条红线：任何一条失败都意味着有人在前端
// 重新发明了分类表（按前缀/名称归类），而那在真机上会静默给出错误分类。
import { describe, expect, it } from "vitest";
import {
  CATALOG_CATEGORY,
  CATEGORY_ORDER,
  buildCatalogTree,
  categoryLabel,
  classifyCatalogDefinition,
  describeCategoryBasis,
  evaluateCategoryGate,
  isCatalogCategoryCode,
  matchesCatalogQuery,
} from "./catalogTree";

/** 一条最小定义（只保留与建树相关的字段，其余由调用方补）。 */
function def(
  variableId: string,
  category: unknown,
  label: { zh?: string; en?: string } | string = { zh: variableId },
) {
  return { variable_id: variableId, category, category_basis: "navigation_rule", label };
}

describe("分组：只认后端 category（不猜、不按前缀）", () => {
  it("groups_follow_backend_category_verbatim", () => {
    const tree = buildCatalogTree([
      def("a.cnc", CATALOG_CATEGORY.CNC_VARIABLE),
      def("a.system", CATALOG_CATEGORY.SYSTEM_VARIABLE),
      def("a.machine", CATALOG_CATEGORY.MACHINE_DATA),
      def("a.drive", CATALOG_CATEGORY.DRIVE_PARAMETER),
    ]);
    expect(tree.groups.map((group) => group.code)).toEqual([...CATEGORY_ORDER]);
    expect(tree.groups.map((group) => group.items.length)).toEqual([1, 1, 1, 1, 0]);
    expect(tree.classifiedCount).toBe(4);
    expect(tree.total).toBe(4);
    // 分组标题固定为五项（含待分类），空分组也保留：用户要看见"这一类今天没有条目"。
    expect(tree.groups.map((group) => group.label)).toEqual([
      "CNC 变量",
      "系统变量",
      "机床数据",
      "驱动参数",
      "其他 / 待分类",
    ]);
  });

  it("missing_category_falls_into_unclassified_bucket", () => {
    const tree = buildCatalogTree([def("no.category", undefined)]);
    const unclassified = tree.groups.find(
      (group) => group.code === CATALOG_CATEGORY.UNCLASSIFIED,
    )!;
    expect(unclassified.items).toHaveLength(1);
    const item = unclassified.items[0]!;
    expect(item.variableId).toBe("no.category");
    expect(item.categoryKnown).toBe(false);
    // 缺字段 ≠ 驱动明确 unclassified：两者都能标注，但只有后者拒提交（见下文门禁）。
    expect(item.explicitlyUnclassified).toBe(false);
    // 绝不落进四类之一，也不计入"分类已确认"计数。
    expect(tree.groups.filter((g) => g.code !== CATALOG_CATEGORY.UNCLASSIFIED)
      .flatMap((g) => g.items)).toHaveLength(0);
    expect(tree.classifiedCount).toBe(0);
  });

  it("unknown_category_value_is_not_guessed", () => {
    // 大小写变体、拼写变体、非字符串一律不认——猜测只会制造假分类。
    for (const bogus of ["CNC_VARIABLE", "cnc-variable", "channel", 5, null, {}]) {
      const item = classifyCatalogDefinition(def(`x.${String(bogus)}`, bogus));
      expect(item.bucket).toBe(CATALOG_CATEGORY.UNCLASSIFIED);
      expect(item.categoryKnown).toBe(false);
    }
    expect(isCatalogCategoryCode("cnc_variable")).toBe(true);
    expect(isCatalogCategoryCode("CNC_VARIABLE")).toBe(false);
  });

  it("prefixes_do_not_determine_category", () => {
    // 反例的真实形态（docs/reference/nck-catalog-category-navigation-rule.md §4）：
    // 同为 `channel.` 前缀，后端一个是系统变量、一个是机床数据。前端必须逐字跟随，
    // 一旦按前缀归类，这两条必然有一条错。
    const tree = buildCatalogTree([
      def("channel.r_parameter", CATALOG_CATEGORY.SYSTEM_VARIABLE),
      def("channel.machine_axis_assignment", CATALOG_CATEGORY.MACHINE_DATA),
    ]);
    const system = tree.groups.find((g) => g.code === CATALOG_CATEGORY.SYSTEM_VARIABLE)!;
    const machine = tree.groups.find((g) => g.code === CATALOG_CATEGORY.MACHINE_DATA)!;
    expect(system.items.map((item) => item.variableId)).toEqual(["channel.r_parameter"]);
    expect(machine.items.map((item) => item.variableId)).toEqual([
      "channel.machine_axis_assignment",
    ]);
  });

  it("explicit_unclassified_is_marked_but_still_not_a_category", () => {
    const item = classifyCatalogDefinition(def("x.u", CATALOG_CATEGORY.UNCLASSIFIED));
    expect(item.bucket).toBe(CATALOG_CATEGORY.UNCLASSIFIED);
    expect(item.explicitlyUnclassified).toBe(true);
    expect(item.categoryKnown).toBe(false);
  });
});

describe("分类依据：必须可见，且如实写成导航规则", () => {
  it("navigation_rule_basis_is_described_as_not_a_manual_fact", () => {
    const view = describeCategoryBasis("navigation_rule");
    expect(view.navigationRule).toBe(true);
    expect(view.label).toBe("导航规则");
    expect(view.hint).toContain("不是手册确认的分类");
  });

  it("unknown_or_missing_basis_is_weaker_than_a_fact", () => {
    for (const basis of [undefined, null, "", "guess"]) {
      const view = describeCategoryBasis(basis);
      expect(view.navigationRule).toBe(false);
      expect(view.label).toBe("依据未知");
    }
    expect(describeCategoryBasis("definition").label).toBe("定义事实");
  });

  it("every_definition_carries_its_basis_into_the_tree", () => {
    const tree = buildCatalogTree([def("a", CATALOG_CATEGORY.CNC_VARIABLE)]);
    const item = tree.groups[0]!.items[0]!;
    expect(item.basis.raw).toBe("navigation_rule");
    expect(item.basis.label).toBe("导航规则");
  });
});

describe("搜索：跨全部分类", () => {
  it("search_spans_all_categories", () => {
    const tree = buildCatalogTree(
      [
        def("axis.position.actual_bcs", CATALOG_CATEGORY.CNC_VARIABLE, { zh: "轴实际位置" }),
        def("axis.ctrlout.module_number", CATALOG_CATEGORY.MACHINE_DATA, {
          zh: "轴驱动输出模块号",
        }),
        def("drive.parameter.r0021", CATALOG_CATEGORY.DRIVE_PARAMETER, { zh: "驱动参数 r0021" }),
      ],
      { query: "轴" },
    );
    expect(tree.searching).toBe(true);
    expect(tree.matched).toBe(2);
    const hits = tree.groups.filter((group) => group.items.length > 0).map((group) => group.code);
    expect(hits).toEqual([CATALOG_CATEGORY.CNC_VARIABLE, CATALOG_CATEGORY.MACHINE_DATA]);
  });

  it("search_matches_variable_id_and_english_label", () => {
    const item = classifyCatalogDefinition(
      def("channel.feed.actual", CATALOG_CATEGORY.CNC_VARIABLE, {
        zh: "进给速度",
        en: "Feed rate",
      }),
    );
    expect(matchesCatalogQuery(item, "feed")).toBe(true);
    expect(matchesCatalogQuery(item, "进给")).toBe(true);
    expect(matchesCatalogQuery(item, "channel.feed")).toBe(true);
    expect(matchesCatalogQuery(item, "spindle")).toBe(false);
  });

  it("search_does_not_change_grouping", () => {
    const tree = buildCatalogTree(
      [def("a.cnc", CATALOG_CATEGORY.CNC_VARIABLE), def("b.drive", CATALOG_CATEGORY.DRIVE_PARAMETER)],
      { query: "cnc" },
    );
    // 分组结构恒为五项；搜索只过滤条目。
    expect(tree.groups).toHaveLength(5);
    expect(tree.total).toBe(2);
    expect(tree.matched).toBe(1);
  });
});

describe("分类门禁：明确 unclassified 不得作为可采集", () => {
  it("explicit_unclassified_is_refused", () => {
    const gate = evaluateCategoryGate(
      classifyCatalogDefinition(def("x.u", CATALOG_CATEGORY.UNCLASSIFIED)),
    );
    expect(gate.ok).toBe(false);
    if (!gate.ok) {
      expect(gate.code).toBe("CATEGORY_UNCLASSIFIED");
      expect(gate.message).toContain("unclassified");
    }
  });

  it("classified_items_pass_and_missing_category_is_only_annotated", () => {
    expect(
      evaluateCategoryGate(classifyCatalogDefinition(def("a", CATALOG_CATEGORY.CNC_VARIABLE))).ok,
    ).toBe(true);
    // 缺字段：只标注"分类未知"（不阻断）——缺字段不代表驱动判定它无法归类，
    // 而可采集性本来就由 scope 决定，分类只回答"在树的哪个分支"。
    const missing = classifyCatalogDefinition(def("b", undefined));
    expect(evaluateCategoryGate(missing).ok).toBe(true);
    expect(missing.categoryKnown).toBe(false);
  });

  it("no_selection_is_refused", () => {
    expect(evaluateCategoryGate(null).ok).toBe(false);
  });
});

describe("文案与词表", () => {
  it("labels_cover_every_code", () => {
    for (const code of CATEGORY_ORDER) {
      expect(categoryLabel(code)).not.toBe("");
    }
  });
});
