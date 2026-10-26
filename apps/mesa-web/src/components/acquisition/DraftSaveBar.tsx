// 固定保存区（通用组件，§2.8 / §2.3 / §4.7）。
//
// 本组件只渲染页面层给出的 `SaveBarView`（可见性、文案、入口都已由纯函数决定），
// 不推断驱动、不推断运行态、不自行决定「能不能保存」——那是页面层的判定。
//
// 位置：粘在工作区底部（sticky），页面层在列表末尾预留高度，因此不会遮住分页、
// 侧栏或抽屉操作（抽屉层级更高）。没有修改且无待处理结果时整个组件**不渲染**。
import { Alert, Button } from "antd";
import type { SaveBarView } from "../../acquisitionDraftDiff";

export function DraftSaveBar({
  view,
  summary,
  resultDetail,
  noticeDetail,
  onSave,
  onDiscard,
  onRecheck,
  onRetryStart,
  onRefreshState,
  onToggleRestart,
  rechecking,
  retryBusy,
}: {
  view: SaveBarView;
  /** 变更摘要（无差异时为空串）。 */
  summary: string;
  /** 结果行的补充说明（如失败原因），可选。 */
  resultDetail?: string | null;
  /** 待处理结果的补充说明（如待核对的具体原因、启动失败原因）。 */
  noticeDetail?: string | null;
  onSave: () => void;
  onDiscard: () => void;
  /** 省略时不在保存区内渲染「重新核对」（由外部的持久结果块提供入口）。 */
  onRecheck?: () => void;
  onRetryStart: () => void;
  onRefreshState: () => void;
  /** 切换「保存后保持停止 / 保存后恢复采集」（次级选项，不是并列主按钮）。 */
  onToggleRestart: () => void;
  rechecking?: boolean;
  retryBusy?: boolean;
}) {
  if (!view.visible) return null;
  return (
    <div
      data-testid="draft-save-bar"
      data-mode={view.mode}
      style={{
        position: "sticky",
        bottom: 0,
        zIndex: 5,
        background: "#ffffff",
        borderTop: "1px solid #d0d0d0",
        padding: "10px 12px",
        display: "grid",
        gap: 8,
      }}
    >
      {summary ? (
        <div data-testid="draft-save-summary" style={{ fontSize: 13, color: "#161616" }}>
          {summary}
        </div>
      ) : null}
      {view.notice ? (
        <div data-testid="draft-save-notice" style={{ fontSize: 12, color: "#8a6d3b" }}>
          {view.notice}
          {noticeDetail ? <span style={{ color: "#525252" }}>：{noticeDetail}</span> : null}
        </div>
      ) : null}
      {view.note ? (
        <div data-testid="draft-save-note" style={{ fontSize: 12, color: "#525252" }}>
          {view.note}
        </div>
      ) : null}
      {/* 已保存：动作区收起，只留一次结果行（§4.7「已保存」→ 自动退出）。 */}
      {view.mode === "saved" ? null : (
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        {view.showDiscard ? (
          <Button size="small" data-testid="discard-draft" onClick={onDiscard}>
            放弃修改
          </Button>
        ) : null}
        <span style={{ marginLeft: "auto" }} />
        {view.showRefreshState ? (
          <Button size="small" data-testid="save-bar-refresh-state" onClick={onRefreshState}>
            刷新状态
          </Button>
        ) : null}
        {view.showRecheck && onRecheck ? (
          <Button size="small" data-testid="recheck-save" loading={rechecking} onClick={onRecheck}>
            重新核对
          </Button>
        ) : null}
        {view.showRetryStart ? (
          <Button size="small" data-testid="retry-start" loading={retryBusy} onClick={onRetryStart}>
            重试启动
          </Button>
        ) : null}
        <Button
          type="primary"
          data-testid="apply-button"
          loading={view.mode === "saving"}
          disabled={view.primaryDisabled}
          onClick={onSave}
        >
          {view.primaryLabel}
        </Button>
        {view.secondaryLabel ? (
          <Button size="small" type="link" data-testid="toggle-restart" onClick={onToggleRestart}>
            {view.secondaryLabel}
          </Button>
        ) : null}
        </div>
      )}
      {view.resultText ? (
        <Alert
          type="success"
          showIcon
          data-testid="apply-result"
          data-kind="success"
          message={view.resultText}
          description={resultDetail ?? undefined}
        />
      ) : null}
    </div>
  );
}
