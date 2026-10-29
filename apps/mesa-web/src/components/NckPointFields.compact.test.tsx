// 「添加点位」抽屉的**紧凑双模式选择器**验收（组件级）。
//
// 用户判定：不要调试控制台式的目录浏览器/状态墙；普通模式第一屏只应看到
// 模式切换、变量选择、对象选择、添加按钮。本文件逐条钉住新形态：
//   1. 第一屏只有四个块；目录浏览器入口不在抽屉里；技术细节默认折叠；
//   2. 树形下拉默认收起：打开后先只有分组标题，展开后才见变量；
//   3. 变量行只显示名称 + 简短可用状态；hold 长文不在列表里，选中后才可见；
//   4. 对象级只显示当前变量真正需要的级；
//   5. 关键容器不声明横向滚动（宽度 100% / minWidth 0 / overflowX hidden）。
//
// 这里刻意不重复门禁/预览状态机的细节断言（那些在 `NckPointFields.test.tsx`
// 与流程级用例里），只验证**呈现层**的新约束。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { NckPointFields } from "./NckPointFields";
import type { NckDefinitionLike } from "../nckPointModel";
import type { SchemaDescriptor } from "../types";
import { CANDIDATE_AVAILABILITY } from "../test/availabilityFixture";
import {
  expandAllCatalogGroups,
  openVariablePicker,
  selectVariableViaTree,
} from "../test/catalogTreePick";

const NATIVE_SCHEMA: SchemaDescriptor = {
  fields: [
    {
      key: "area",
      label: "Area",
      field_type: "string",
      required: true,
      validation: {},
      ui: {},
    },
  ],
};

const HOLD_REASON = "驱动无真机读取证据：手册未确认该变量在 828D 上可读";

function definition(overrides: Partial<NckDefinitionLike> = {}): NckDefinitionLike {
  return {
    variable_id: "channel.feed.actual",
    revision: 1,
    family: "828d",
    scope: "collect",
    semantics_status: "verified",
    label: { zh: "进给速度", en: "Feed rate" },
    category: "cnc_variable",
    category_basis: "navigation_rule",
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    availability: CANDIDATE_AVAILABILITY,
    ...overrides,
  };
}

const HOLD_DEF = definition({
  variable_id: "channel.feed.commanded",
  label: { zh: "指令进给速度", en: "Commanded feed" },
  availability: { state: "hold", basis: ["no_device_read"], reason_zh: HOLD_REASON },
});

const SPINDLE_DEF = definition({
  variable_id: "spindle.speed.actual",
  label: { zh: "主轴转速", en: "Spindle speed" },
  targets: ["channel", "spindle"],
  target_required: ["channel", "spindle"],
  identity_keys: [
    { path: "channel", required: true },
    { path: "spindle", required: true },
  ],
});

function renderFields(definitions: NckDefinitionLike[]) {
  render(
    <NckPointFields
      driverId="sinumerik-nck"
      endpointFamily="828d"
      definitions={definitions}
      takenPointKeys={[]}
      endpointId="ep-compact"
      probeGate={{ available: true }}
      nativeSchema={NATIVE_SCHEMA}
      onSubmit={() => {}}
      preview={() => new Promise(() => {})}
    />,
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("紧凑选择器：第一屏只有四个块", () => {
  it("first_screen_has_mode_variable_object_and_add_only", () => {
    renderFields([definition(), HOLD_DEF, SPINDLE_DEF]);
    const root = screen.getByTestId("nck-point-fields-idle");

    // 模式切换在最上方；变量选择与添加按钮紧随其后。
    expect(root.firstElementChild?.getAttribute("data-testid")).toBe("nck-editor-mode");
    expect(screen.getByTestId("nck-variable-picker-trigger")).toBeTruthy();
    // 对象选择区在首屏有固定位置（选变量后才显示真正需要的级）。
    expect(screen.getByTestId("nck-object-placeholder").textContent).toContain("实际对象选择");
    const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(root.contains(add)).toBe(true);

    // 目录浏览器入口不在抽屉里；技术细节默认折叠（未选变量时不渲染）。
    expect(screen.queryByTestId("nck-browse-toggle")).toBeNull();
    expect(screen.queryByTestId("nck-tech-details")).toBeNull();
    // 树形下拉默认收起：面板/搜索框/叶子都不在首屏。
    expect(screen.queryByTestId("catalog-tree-panel")).toBeNull();
    expect(screen.queryByTestId("nck-variable-search")).toBeNull();
    expect(screen.queryAllByTestId(/^catalog-tree-leaf-/)).toHaveLength(0);
  });
});

describe("树形下拉：默认收起、可搜索、选中后显示详情", () => {
  it("dropdown_opens_with_group_titles_only_then_expands_to_variables", () => {
    renderFields([definition(), SPINDLE_DEF]);
    openVariablePicker();

    // 打开下拉后：五个分组标题都在，但没有任何变量叶子（默认全收起）。
    const toggles = screen.getAllByTestId(/^catalog-tree-toggle-/);
    expect(toggles).toHaveLength(5);
    expect(screen.queryAllByTestId(/^catalog-tree-leaf-/)).toHaveLength(0);
    for (const toggle of toggles) {
      expect(toggle.getAttribute("aria-expanded")).toBe("false");
    }
    expect(screen.getByTestId("catalog-tree-group-cnc_variable").getAttribute("data-expanded"))
      .toBe("false");

    // 点分组标题后才出现该组的变量。
    fireEvent.click(screen.getByTestId("catalog-tree-toggle-cnc_variable"));
    expect(screen.getByTestId("catalog-tree-group-cnc_variable").getAttribute("data-expanded"))
      .toBe("true");
    expect(screen.getByTestId("catalog-tree-leaf-channel.feed.actual")).toBeTruthy();
  });

  it("variable_row_shows_name_and_short_availability_only", () => {
    renderFields([HOLD_DEF]);
    openVariablePicker();
    expandAllCatalogGroups();
    const leaf = screen.getByTestId("catalog-tree-leaf-channel.feed.commanded");
    expect(leaf.textContent).toContain("指令进给速度");
    expect(leaf.textContent).toContain("暂缓");
    // ID / 单位 / 手册 / 暂缓长文都不在列表行里（按需在选中后查看）。
    expect(leaf.textContent).not.toContain("channel.feed.commanded");
    expect(leaf.textContent).not.toContain(HOLD_REASON);
    expect(screen.queryByTestId("catalog-tree-leaf-hold-reason-channel.feed.commanded")).toBeNull();
    expect(screen.queryByTestId("catalog-tree-leaf-unit-channel.feed.commanded")).toBeNull();
    expect(screen.queryByTestId("catalog-tree-leaf-manual-channel.feed.commanded")).toBeNull();
  });

  it("hold_reason_is_visible_only_after_selecting_the_variable", () => {
    renderFields([HOLD_DEF]);
    openVariablePicker();
    expandAllCatalogGroups();
    // 列表阶段：原因长文不可见。
    expect(screen.queryByTestId("nck-availability-hold-reason")).toBeNull();

    fireEvent.click(screen.getByTestId("catalog-tree-leaf-channel.feed.commanded"));
    // 选中后：原因可见、门禁阻断；下拉已收起（不继续占空间）。
    expect(screen.getByTestId("nck-availability-hold-reason").textContent).toContain(HOLD_REASON);
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByTestId("catalog-tree-panel")).toBeNull();
  });

  it("selected_variable_details_are_collapsed_until_opened", () => {
    renderFields([definition()]);
    selectVariableViaTree("channel.feed.actual");

    const details = screen.getByTestId("nck-variable-details") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    expect(details.textContent).toContain("channel.feed.actual");
    expect(details.querySelector('[data-testid="nck-unit"]')).toBeTruthy();
    // 技术详情同样是默认折叠的排查层。
    expect(screen.getByTestId("nck-tech-details")).toBeTruthy();
    expect(screen.queryByTestId("nck-tech-details-body")).toBeNull();
    fireEvent.click(screen.getByText(/技术详情/));
    expect(screen.getByTestId("nck-tech-details-body")).toBeTruthy();
  });
});

describe("对象选择：只显示当前变量需要的级", () => {
  it("channel_only_variable_renders_exactly_one_dropdown", () => {
    renderFields([definition(), SPINDLE_DEF]);
    selectVariableViaTree("channel.feed.actual");
    expect(screen.getAllByTestId(/^nck-object-select-/)).toHaveLength(1);
    expect(screen.getByTestId("nck-object-select-channel")).toBeTruthy();
    expect(screen.queryByTestId("nck-object-level-spindle")).toBeNull();
    expect(screen.queryByTestId("nck-object-level-axis")).toBeNull();
    expect(screen.getByTestId("nck-object-cascade").getAttribute("data-level-count")).toBe("1");
  });

  it("two_level_variable_renders_only_its_two_required_levels", () => {
    renderFields([definition(), SPINDLE_DEF]);
    selectVariableViaTree("spindle.speed.actual");
    expect(screen.getAllByTestId(/^nck-object-select-/)).toHaveLength(2);
    expect(screen.getByTestId("nck-object-level-channel")).toBeTruthy();
    expect(screen.getByTestId("nck-object-level-spindle")).toBeTruthy();
    expect(screen.queryByTestId("nck-object-level-axis")).toBeNull();
    expect(screen.getByTestId("nck-object-cascade").getAttribute("data-level-count")).toBe("2");
  });
});

describe("布局：关键容器不声明横向滚动", () => {
  it("no_horizontal_overflow_declarations_on_drawer_content", () => {
    renderFields([definition()]);
    selectVariableViaTree("channel.feed.actual");
    openVariablePicker();

    const root = screen.getByTestId("nck-point-fields");
    expect(root.style.width).toBe("100%");
    expect(root.style.minWidth).toBe("0");

    const picker = screen.getByTestId("nck-variable-picker");
    expect(picker.style.minWidth).toBe("0");
    expect(picker.style.width).toBe("100%");

    const panel = screen.getByTestId("catalog-tree-panel");
    expect(panel.style.overflowX).toBe("hidden");
    expect(panel.style.minWidth).toBe("0");

    const level = screen.getByTestId("nck-object-level-channel");
    expect(level.style.minWidth).toBe("0");
    const select = screen.getByTestId("nck-object-select-channel") as HTMLSelectElement;
    expect(select.style.maxWidth).toBe("100%");
    expect(select.style.minWidth).toBe("0");

    // 任何后代都不得声明 overflow-x: auto/scroll（jsdom 无布局引擎，只能断言声明）。
    const forced = Array.from(root.querySelectorAll<HTMLElement>("*")).filter((el) =>
      ["auto", "scroll"].includes(el.style.overflowX),
    );
    expect(forced).toHaveLength(0);
  });
});
