// 草稿差异 + 保存区状态机（纯函数）门禁。
//
// 重点钉死：
// - 行级标记与计数按**完整有效配置内容**判，不是「改过就标记」；
// - 新增后移除相互抵消；修改后恢复原值取消标记；删除后重加同配置计数归零；
// - 摘要把点位变更与周期修改**分开表述**；
// - 保存区：无差异且无待处理结果 ⇒ 隐藏；正在保存/待核对/恢复失败 ⇒ 即使没差异也保留入口。
import { describe, expect, it } from "vitest";
import {
  INTERRUPT_NOTE,
  RUN_STATE_UNKNOWN_NOTE,
  STOPPED_NOTE,
  changeSummaryText,
  diffDraft,
  draftContentKey,
  saveBarView,
  type DraftBaseline,
  type DraftRowRef,
  type SaveBarInput,
} from "./acquisitionDraftDiff";

function row(rowId: string, content: string): DraftRowRef {
  return { rowId, contentKey: content };
}

const baseline: DraftBaseline = {
  rows: [row("row-1", "a"), row("row-2", "b"), row("row-3", "c")],
  intervalMs: 1000,
};

describe("diffDraft：行级标记与计数", () => {
  it("no_change_means_no_changes", () => {
    const diff = diffDraft({ baseline, draft: baseline.rows, intervalMs: 1000 });
    expect(diff.hasChanges).toBe(false);
    expect(diff.pointChangeCount).toBe(0);
    expect(diff.rowMarkers).toEqual({});
    expect(diff.summary).toBe("");
  });

  it("counts_add_modify_remove_separately", () => {
    const diff = diffDraft({
      baseline,
      draft: [row("row-1", "a-changed"), row("row-2", "b"), row("row-new", "d")],
      intervalMs: 1000,
    });
    expect(diff.modified).toBe(1);
    expect(diff.added).toBe(1);
    expect(diff.removed).toBe(1);
    expect(diff.pointChangeCount).toBe(3);
    expect(diff.rowMarkers).toEqual({ "row-1": "modified", "row-new": "added" });
    expect(diff.summary).toBe("3 项点位变更（新增 1 · 修改 1 · 移除 1）");
  });

  it("reverting_a_value_clears_the_marker", () => {
    const changed = diffDraft({
      baseline,
      draft: [row("row-1", "a-changed"), row("row-2", "b"), row("row-3", "c")],
      intervalMs: 1000,
    });
    expect(changed.rowMarkers["row-1"]).toBe("modified");
    // 改回原值：标记必须消失（按内容比较，不是「改过就标记」）。
    const reverted = diffDraft({ baseline, draft: baseline.rows, intervalMs: 1000 });
    expect(reverted.rowMarkers["row-1"]).toBeUndefined();
    expect(reverted.hasChanges).toBe(false);
  });

  it("add_then_remove_cancels_out", () => {
    // 新增一行再加回去（草稿里不再有它）⇒ 不产生「移除」计数。
    const diff = diffDraft({ baseline, draft: baseline.rows, intervalMs: 1000 });
    expect(diff.added).toBe(0);
    expect(diff.removed).toBe(0);
    expect(diff.summary).toBe("");
  });

  it("delete_then_readd_same_config_counts_zero", () => {
    // 删除中间一行后又加回来（新 rowId，内容与位置一致）⇒ 配置没变，计数归零。
    const diff = diffDraft({
      baseline,
      draft: [row("row-1", "a"), row("row-9", "b"), row("row-3", "c")],
      intervalMs: 1000,
    });
    expect(diff.pointChangeCount).toBe(0);
    expect(diff.rowMarkers).toEqual({});
    expect(diff.hasChanges).toBe(false);
  });

  it("interval_change_is_reported_separately", () => {
    const diff = diffDraft({ baseline, draft: baseline.rows, intervalMs: 500 });
    expect(diff.intervalChanged).toBe(true);
    expect(diff.pointChangeCount).toBe(0);
    expect(diff.hasChanges).toBe(true);
    expect(diff.summary).toBe("周期另有修改：1000 → 500 ms");
    // 点位与周期同时变化：分开表述，周期不计入「N 项点位变更」。
    const both = diffDraft({
      baseline,
      draft: [row("row-1", "a"), row("row-2", "b"), row("row-new", "d")],
      intervalMs: 500,
    });
    expect(both.summary).toBe("2 项点位变更（新增 1 · 移除 1）；周期另有修改：1000 → 500 ms");
  });

  it("missing_baseline_never_invents_changes", () => {
    const diff = diffDraft({
      baseline: null,
      draft: [row("row-1", "a")],
      intervalMs: 500,
    });
    expect(diff.hasChanges).toBe(false);
    expect(diff.intervalChanged).toBe(false);
    expect(diff.summary).toBe("");
  });

  it("summary_of_partial_dimensions_omits_zero_entries", () => {
    const text = changeSummaryText({
      added: 0,
      modified: 2,
      removed: 0,
      pointChangeCount: 2,
      rowMarkers: {},
      intervalChanged: false,
      intervalFrom: 1000,
      intervalTo: 1000,
      hasChanges: true,
      summary: "",
    });
    expect(text).toBe("2 项点位变更（修改 2）");
    expect(text).not.toContain("新增 0");
    expect(text).not.toContain("移除 0");
  });

  it("content_key_is_order_insensitive_but_content_sensitive", () => {
    expect(draftContentKey({ a: 1, b: 2 })).toBe(draftContentKey({ b: 2, a: 1 }));
    expect(draftContentKey({ a: 1 })).not.toBe(draftContentKey({ a: 2 }));
    // 数组顺序是配置的一部分：不得归一化。
    expect(draftContentKey([1, 2])).not.toBe(draftContentKey([2, 1]));
  });
});

describe("saveBarView：保存区可见性与文案状态机", () => {
  const base: SaveBarInput = {
    hasChanges: false,
    saving: false,
    pendingVerify: false,
    restartFailed: false,
    savedResult: null,
    runState: "stopped",
    restart: true,
    canSave: true,
  };

  it("hidden_when_nothing_to_do", () => {
    const view = saveBarView(base);
    expect(view.visible).toBe(false);
    expect(view.mode).toBe("hidden");
    expect(view.primaryLabel).toBe("保存配置");
  });

  it("stopped_pending_says_save_config_and_never_starts", () => {
    const view = saveBarView({ ...base, hasChanges: true });
    expect(view.visible).toBe(true);
    expect(view.mode).toBe("pending");
    expect(view.primaryLabel).toBe("保存配置");
    expect(view.note).toBe(STOPPED_NOTE);
    expect(view.showDiscard).toBe(true);
    expect(view.secondaryLabel).toBeNull();
  });

  it("running_pending_offers_restart_with_interrupt_note", () => {
    const view = saveBarView({ ...base, hasChanges: true, runState: "running" });
    expect(view.primaryLabel).toBe("保存并恢复采集");
    expect(view.note).toBe(INTERRUPT_NOTE);
    expect(view.secondaryLabel).toBe("保存后保持停止");
    // 选择保持停止后主按钮文案随动作改变（不是并列主按钮）。
    const keepStopped = saveBarView({ ...base, hasChanges: true, runState: "running", restart: false });
    expect(keepStopped.primaryLabel).toBe("保存并停止采集");
    expect(keepStopped.secondaryLabel).toBe("保存后恢复采集");
  });

  it("unknown_run_state_disables_save", () => {
    const view = saveBarView({ ...base, hasChanges: true, runState: "unknown" });
    expect(view.mode).toBe("run-state-unknown");
    expect(view.primaryDisabled).toBe(true);
    expect(view.note).toBe(RUN_STATE_UNKNOWN_NOTE);
    expect(view.showRefreshState).toBe(true);
  });

  it("saving_stays_visible_without_any_draft_diff", () => {
    const view = saveBarView({ ...base, saving: true, canSave: false });
    expect(view.visible).toBe(true);
    expect(view.mode).toBe("saving");
    expect(view.primaryLabel).toBe("处理中…");
    expect(view.primaryDisabled).toBe(true);
  });

  it("pending_verify_stays_visible_without_any_draft_diff", () => {
    const view = saveBarView({ ...base, pendingVerify: true, canSave: false });
    expect(view.visible).toBe(true);
    expect(view.mode).toBe("pending-verify");
    expect(view.notice).toBe("保存结果待核对");
    expect(view.showRecheck).toBe(true);
    expect(view.primaryDisabled).toBe(true);
    expect(view.showDiscard).toBe(false);
  });

  it("restart_failure_keeps_retry_entry_even_after_new_changes", () => {
    const failed = saveBarView({ ...base, restartFailed: true, canSave: false });
    expect(failed.visible).toBe(true);
    expect(failed.mode).toBe("restart-failed");
    expect(failed.notice).toBe("配置已保存，但恢复采集失败");
    expect(failed.showRetryStart).toBe(true);
    expect(failed.primaryDisabled).toBe(true);

    // 又产生新修改：主按钮回到可保存状态，但「重试启动」入口仍然保留。
    const withNewChanges = saveBarView({ ...base, restartFailed: true, hasChanges: true });
    expect(withNewChanges.mode).toBe("pending");
    expect(withNewChanges.primaryDisabled).toBe(false);
    expect(withNewChanges.showRetryStart).toBe(true);
    expect(withNewChanges.notice).toBe("配置已保存，但恢复采集失败");
  });

  it("saved_result_is_shown_once_without_actions", () => {
    const view = saveBarView({ ...base, savedResult: "restarted" });
    expect(view.mode).toBe("saved");
    expect(view.resultText).toBe("配置已保存，采集已启动");
    expect(view.showDiscard).toBe(false);
    const stopped = saveBarView({ ...base, savedResult: "stopped" });
    expect(stopped.resultText).toBe("配置已保存，当前停止");
  });

  it("canSave_false_disables_the_primary_button", () => {
    const view = saveBarView({ ...base, hasChanges: true, canSave: false });
    expect(view.primaryDisabled).toBe(true);
  });
});
