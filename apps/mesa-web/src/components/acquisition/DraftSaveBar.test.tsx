// 固定保存区的组件门禁（可见性 / 文案 / 入口）。
//
// 三条评审补充规则在这里逐条钉死：
// 1. 无差异且无待处理结果 ⇒ **不渲染**；
// 2. 正在保存 / 待核对 / 恢复失败 ⇒ 即使没有差异也保留入口；
// 3. 是否真的能保存由页面层给的 `canSave` 决定（组件不推断）。
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DraftSaveBar } from "./DraftSaveBar";
import { saveBarView, type SaveBarInput } from "../../acquisitionDraftDiff";

function view(overrides: Partial<SaveBarInput> = {}) {
  return saveBarView({
    hasChanges: false,
    saving: false,
    pendingVerify: false,
    restartFailed: false,
    savedResult: null,
    runState: "stopped",
    restart: true,
    canSave: true,
    ...overrides,
  });
}

function renderBar(overrides: Partial<SaveBarInput> = {}, handlers: Record<string, () => void> = {}) {
  const handlers2 = {
    onSave: vi.fn(),
    onDiscard: vi.fn(),
    onRecheck: vi.fn(),
    onRetryStart: vi.fn(),
    onRefreshState: vi.fn(),
    onToggleRestart: vi.fn(),
    ...handlers,
  };
  render(<DraftSaveBar view={view(overrides)} summary={overrides.hasChanges ? "1 项点位变更（移除 1）" : ""} {...handlers2} />);
  return handlers2;
}

describe("规则 1：无差异且无待处理结果 ⇒ 不渲染", () => {
  it("renders_nothing_when_hidden", () => {
    renderBar();
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(screen.queryByTestId("apply-button")).toBeNull();
  });
});

describe("规则 2：正在保存 / 待核对 / 恢复失败 ⇒ 保留处理入口", () => {
  it("saving_shows_progress_without_diff", () => {
    renderBar({ saving: true, canSave: false });
    const bar = screen.getByTestId("draft-save-bar");
    expect(bar.getAttribute("data-mode")).toBe("saving");
    expect(screen.getByTestId("apply-button").textContent).toBe("处理中…");
    expect((screen.getByTestId("apply-button") as HTMLButtonElement).disabled).toBe(true);
  });

  it("pending_verify_keeps_recheck_entry_without_diff", async () => {
    const user = userEvent.setup();
    const handlers = renderBar({ pendingVerify: true, canSave: false });
    expect(screen.getByTestId("draft-save-notice").textContent).toContain("保存结果待核对");
    await user.click(screen.getByTestId("recheck-save"));
    expect(handlers.onRecheck).toHaveBeenCalled();
    // 没有摘要、没有「放弃修改」。
    expect(screen.queryByTestId("draft-save-summary")).toBeNull();
    expect(screen.queryByTestId("discard-draft")).toBeNull();
  });

  it("restart_failure_keeps_retry_entry_without_diff", async () => {
    const user = userEvent.setup();
    const handlers = renderBar({ restartFailed: true, canSave: false });
    expect(screen.getByTestId("draft-save-notice").textContent).toContain("恢复采集失败");
    await user.click(screen.getByTestId("retry-start"));
    expect(handlers.onRetryStart).toHaveBeenCalled();
  });
});

describe("待保存：摘要 + 中断说明 + 放弃修改 + 主按钮", () => {
  it("stopped_shows_save_config_and_no_interrupt_note", async () => {
    const user = userEvent.setup();
    const handlers = renderBar({ hasChanges: true });
    expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（移除 1）");
    expect(screen.getByTestId("apply-button").textContent).toBe("保存配置");
    expect(screen.getByTestId("draft-save-note").textContent).toContain("不会启动采集");
    await user.click(screen.getByTestId("discard-draft"));
    expect(handlers.onDiscard).toHaveBeenCalled();
    await user.click(screen.getByTestId("apply-button"));
    expect(handlers.onSave).toHaveBeenCalled();
  });

  it("running_shows_restart_default_and_interrupt_note", async () => {
    const user = userEvent.setup();
    const handlers = renderBar({ hasChanges: true, runState: "running" });
    expect(screen.getByTestId("apply-button").textContent).toBe("保存并恢复采集");
    expect(screen.getByTestId("draft-save-note").textContent).toBe(
      "保存会暂时中断当前连接的采集",
    );
    await user.click(screen.getByTestId("toggle-restart"));
    expect(handlers.onToggleRestart).toHaveBeenCalled();
  });

  it("run_state_unknown_offers_refresh_only", async () => {
    const user = userEvent.setup();
    const handlers = renderBar({ hasChanges: true, runState: "unknown" });
    expect(screen.getByTestId("apply-button").textContent).toBe("保存配置");
    expect((screen.getByTestId("apply-button") as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByTestId("save-bar-refresh-state"));
    expect(handlers.onRefreshState).toHaveBeenCalled();
  });
});

describe("已保存：只留一次结果，动作区收起", () => {
  it("saved_shows_result_once_and_hides_actions", () => {
    renderBar({ savedResult: "restarted" });
    expect(screen.getByTestId("apply-result").textContent).toContain("配置已保存，采集已启动");
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(screen.queryByTestId("discard-draft")).toBeNull();
    expect(screen.queryByTestId("draft-save-summary")).toBeNull();
  });
});
