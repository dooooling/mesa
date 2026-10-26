// 单行提示位（通用组件，§2.8）：按节点出现，绝非常驻横幅。
//
// 用途边界：只放**需要动作**或**真实问题**的信息（如「已移除 1 项 ［撤销］」、
// 「3 个点位需要处理 ［仅看问题］」）。成功/正常/已保存不占位置。
import type { ReactNode } from "react";

export type NoticeLevel = "info" | "warning" | "error";

const COLORS: Record<NoticeLevel, string> = {
  info: "#525252",
  warning: "#8a6d3b",
  error: "#c62828",
};

export function NoticeSlot({
  level,
  text,
  detail,
  action,
  testId,
}: {
  level: NoticeLevel;
  text: string;
  /** 可展开的补充说明（原始错误码、原始配置等）。 */
  detail?: ReactNode;
  action?: ReactNode;
  testId?: string;
}) {
  return (
    <div
      data-testid={testId}
      data-level={level}
      style={{
        display: "flex",
        gap: 8,
        alignItems: "baseline",
        flexWrap: "wrap",
        fontSize: 12,
        color: COLORS[level],
        padding: "4px 0",
      }}
    >
      <span>{text}</span>
      {action}
      {detail ? (
        <details style={{ width: "100%" }}>
          <summary style={{ cursor: "pointer", fontSize: 11, color: "#525252" }}>技术详情</summary>
          <div style={{ marginTop: 4 }}>{detail}</div>
        </details>
      ) : null}
    </div>
  );
}
