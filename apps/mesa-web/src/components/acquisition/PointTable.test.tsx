// 通用点位表格（3 列 + 分页 + 空态 + 行内更多菜单）的组件门禁。
//
// 这些断言只依赖 `PointRowView`：组件内不存在任何驱动/协议判断（§2.8 可检验口径）。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PointTable } from "./PointTable";
import type { PointRowView } from "../../acquisitionRowView";

function row(overrides: Partial<PointRowView> = {}): PointRowView {
  return {
    rowId: "row-1",
    title: "轴实际位置",
    summary: "轴 1 · 通道 1",
    searchText: "轴实际位置",
    detail: [{ label: "point_key", value: "axis.position@axis=i:1", mono: true }],
    ...overrides,
  };
}

function renderTable(overrides: Partial<Parameters<typeof PointTable>[0]> = {}) {
  const actions = {
    onEdit: vi.fn(),
    onRemove: vi.fn(),
    onDetail: vi.fn(),
  };
  const onPageChange = vi.fn();
  const onPageSizeChange = vi.fn();
  render(
    <PointTable
      rows={[row(), row({ rowId: "row-2", title: "进给速度", summary: "通道 1" })]}
      total={2}
      page={1}
      pageSize={25}
      pageCount={1}
      pageSizeOptions={[25, 50, 100]}
      onPageChange={onPageChange}
      onPageSizeChange={onPageSizeChange}
      actions={actions}
      {...overrides}
    />,
  );
  return { actions, onPageChange, onPageSizeChange };
}

describe("三列布局与行级元素预算", () => {
  it("renders_three_columns_and_row_cells", () => {
    renderTable();
    const headers = screen.getAllByRole("columnheader").map((el) => el.textContent);
    expect(headers).toEqual(["名称或标识", "配置摘要", "操作"]);
    const rows = screen.getAllByRole("row");
    // 1 个表头行 + 2 个数据行
    expect(rows).toHaveLength(3);
    const first = screen.getAllByTestId("point-row")[0]!;
    expect(within(first).getByTestId("point-row-title").textContent).toBe("轴实际位置");
    expect(within(first).getByTestId("point-row-summary").textContent).toBe("轴 1 · 通道 1");
  });

  it("normal_row_has_no_status_marker", () => {
    renderTable();
    const first = screen.getAllByTestId("point-row")[0]!;
    expect(within(first).queryByTestId("point-row-status")).toBeNull();
    expect(first.getAttribute("data-marker")).toBe("none");
  });

  it("marker_and_issue_share_one_short_line", () => {
    renderTable({
      rows: [
        row({ marker: "modified", issue: { text: "定义缺失，无法核验身份" } }),
        row({ rowId: "row-2", marker: "added" }),
      ],
    });
    const rows = screen.getAllByTestId("point-row");
    // 已修改 + 有问题 ⇒ 一条短文案，不叠两个标签。
    expect(within(rows[0]!).getByTestId("point-row-status").textContent).toBe(
      "已修改 · 有问题：定义缺失，无法核验身份",
    );
    expect(within(rows[1]!).getByTestId("point-row-status").textContent).toBe("新增");
  });

  it("identifier_only_shown_when_present", () => {
    renderTable({
      rows: [
        row({ title: "原生点位", identifier: "native.C.1" }),
        row({ rowId: "row-2", title: "主轴转速" }),
      ],
    });
    const rows = screen.getAllByTestId("point-row");
    expect(within(rows[0]!).getByTestId("point-row-identifier").textContent).toBe("native.C.1");
    expect(within(rows[1]!).queryByTestId("point-row-identifier")).toBeNull();
  });
});

describe("行内操作：编辑 + 更多（技术详情 / 移除）", () => {
  it("edit_and_more_actions_dispatch_row_identity", async () => {
    const user = userEvent.setup();
    const { actions } = renderTable();
    const first = screen.getAllByTestId("point-row")[0]!;
    await user.click(within(first).getByTestId("point-row-edit"));
    expect(actions.onEdit).toHaveBeenCalledWith(expect.objectContaining({ rowId: "row-1" }));

    await user.click(within(first).getByTestId("point-row-more"));
    const menu = within(first).getByTestId("point-row-menu");
    await user.click(within(menu).getByTestId("point-row-detail"));
    expect(actions.onDetail).toHaveBeenCalledWith(expect.objectContaining({ rowId: "row-1" }));

    await user.click(within(first).getByTestId("point-row-more"));
    await user.click(within(first).getByTestId("point-row-remove"));
    expect(actions.onRemove).toHaveBeenCalledWith(expect.objectContaining({ rowId: "row-1" }));
  });

  it("edit_button_is_hidden_when_row_is_not_editable", () => {
    renderTable({ isEditable: () => false });
    expect(screen.queryAllByTestId("point-row-edit")).toHaveLength(0);
    // 不可编辑的行仍然可以移除（移除不经准入判定）。
    expect(screen.getAllByTestId("point-row-more")).toHaveLength(2);
  });

  it("copy_key_entry_only_when_handler_given", async () => {
    const user = userEvent.setup();
    const onCopyKey = vi.fn();
    renderTable({ actions: { onEdit: vi.fn(), onRemove: vi.fn(), onDetail: vi.fn(), onCopyKey } });
    const first = screen.getAllByTestId("point-row")[0]!;
    await user.click(within(first).getByTestId("point-row-more"));
    await user.click(within(first).getByTestId("point-row-copy-key"));
    expect(onCopyKey).toHaveBeenCalledWith(expect.objectContaining({ rowId: "row-1" }));
  });
});

describe("分页与空态", () => {
  it("page_size_buttons_and_page_text", async () => {
    const user = userEvent.setup();
    const { onPageSizeChange } = renderTable({ total: 238, pageCount: 10 });
    expect(screen.getByTestId("point-table-pagination").textContent).toContain("共 238 项");
    expect(screen.getByTestId("point-table-pagination").textContent).toContain("第 1 / 10 页");
    await user.click(screen.getByTestId("page-size-50"));
    expect(onPageSizeChange).toHaveBeenCalledWith(50);
    // 当前页不再触发重复切换。
    await user.click(screen.getByTestId("page-size-25"));
    expect(onPageSizeChange).toHaveBeenCalledTimes(1);
  });

  it("page_number_click_reports_page", async () => {
    const user = userEvent.setup();
    const { onPageChange } = renderTable({ total: 238, pageCount: 10 });
    await user.click(within(screen.getByTestId("point-table-pagination")).getByTitle("3"));
    expect(onPageChange).toHaveBeenCalledWith(3);
  });

  it("empty_state_replaces_the_table", () => {
    renderTable({ rows: [], total: 0, emptyState: <span>没有匹配的点位，可改用标识搜索</span> });
    expect(screen.getByTestId("point-table-empty").textContent).toContain("没有匹配的点位");
    expect(screen.queryByTestId("point-table")).toBeNull();
    expect(screen.queryAllByRole("columnheader")).toHaveLength(0);
  });

  it("row_data_attributes_are_forwarded", () => {
    renderTable({
      rows: [row()],
      rowData: () => ({ "data-verify-status": "verified" }),
    });
    expect(screen.getByTestId("point-row").getAttribute("data-verify-status")).toBe("verified");
  });
});
