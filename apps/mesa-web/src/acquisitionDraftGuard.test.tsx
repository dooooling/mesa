// 草稿生命周期守卫（§7-7）：刷新用浏览器原生提醒；切换连接/离开页面先问一次。
import { describe, expect, it } from "vitest";
import { act, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DRAFT_LEAVE_MESSAGE, confirmDiscardDraft, useBeforeUnloadGuard } from "./acquisitionDraftGuard";

describe("useBeforeUnloadGuard：浏览器原生离开提醒", () => {
  it("registers_only_when_dirty", () => {
    const clean = renderHook(({ dirty }) => useBeforeUnloadGuard(dirty), {
      initialProps: { dirty: false },
    });
    const cleanEvent = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(cleanEvent);
    expect(cleanEvent.defaultPrevented).toBe(false);

    clean.rerender({ dirty: true });
    const dirtyEvent = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(dirtyEvent);
    // 有未保存修改 ⇒ 必须触发浏览器原生提醒，不静默丢弃。
    expect(dirtyEvent.defaultPrevented).toBe(true);

    // 修改保存完（dirty=false）后注销，正常刷新不被拦住。
    clean.rerender({ dirty: false });
    const afterSave = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(afterSave);
    expect(afterSave.defaultPrevented).toBe(false);
    clean.unmount();
    expect(DRAFT_LEAVE_MESSAGE).toContain("未保存");
  });
});

describe("confirmDiscardDraft：切换连接 / 离开页面先问一次", () => {
  it("no_dialog_when_clean", async () => {
    await expect(confirmDiscardDraft(false, "切换连接")).resolves.toBe(true);
    expect(screen.queryByText(/会丢弃当前未保存的采集配置修改/)).toBeNull();
  });

  it("asks_once_and_respects_both_answers", async () => {
    const user = userEvent.setup();
    // 取消：不离开
    const reject = confirmDiscardDraft(true, "切换连接");
    const titles = await screen.findAllByText(/切换连接会丢弃当前未保存的采集配置修改，确定继续？/);
    expect(titles.length).toBeGreaterThan(0);
    await user.click(screen.getByRole("button", { name: "继续编辑" }));
    await expect(reject).resolves.toBe(false);

    // 确认：放弃修改并继续
    const accept = confirmDiscardDraft(true, "离开采集页");
    await waitFor(() =>
      expect(
        screen.getAllByText(/离开采集页会丢弃当前未保存的采集配置修改/).length,
      ).toBeGreaterThan(0),
    );
    await act(async () => {
      await user.click(screen.getByRole("button", { name: "放弃修改并继续" }));
    });
    await expect(accept).resolves.toBe(true);
  });
});
