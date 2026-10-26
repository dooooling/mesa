// 抽屉容器（通用组件）门禁：唯一说明、保存中禁止关闭、页脚动作。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { DRAFT_NOTICE, PointDrawer } from "./PointDrawer";

function renderDrawer(overrides: Partial<Parameters<typeof PointDrawer>[0]> = {}) {
  const onClose = vi.fn();
  render(
    <PointDrawer open title="添加点位" onClose={onClose} footer={<button>取消</button>} {...overrides}>
      <div data-testid="drawer-body">内容</div>
    </PointDrawer>,
  );
  return { onClose };
}

describe("抽屉唯一说明（§4.3）", () => {
  it("shows_exactly_one_notice_line", () => {
    renderDrawer();
    expect(screen.getByTestId("drawer-draft-notice").textContent).toBe(
      "确认后加入页面草稿，保存配置后生效。",
    );
    expect(DRAFT_NOTICE).toBe("确认后加入页面草稿，保存配置后生效。");
    // 除这一句外不再有第二段流程说明。
    expect(screen.getAllByTestId("drawer-draft-notice")).toHaveLength(1);
  });

  it("read_only_drawer_can_hide_the_notice", () => {
    renderDrawer({ notice: null, title: "技术详情" });
    expect(screen.queryByTestId("drawer-draft-notice")).toBeNull();
    expect(screen.getByTestId("drawer-body")).toBeTruthy();
  });
});

describe("保存中禁止关闭抽屉（§4.7）", () => {
  it("busy_hides_close_controls_and_ignores_mask_click", () => {
    const { onClose } = renderDrawer({ busy: true });
    // 关闭按钮不渲染；遮罩点击也不会调用 onClose（避免「请求还在飞、抽屉已关」）。
    expect(document.querySelector(".ant-drawer-close")).toBeNull();
    const mask = document.querySelector(".ant-drawer-mask") as HTMLElement | null;
    if (mask) fireEvent.click(mask);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("close_button_calls_onClose_when_not_busy", () => {
    const { onClose } = renderDrawer();
    const close = document.querySelector(".ant-drawer-close") as HTMLElement | null;
    expect(close).not.toBeNull();
    fireEvent.click(close!);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
