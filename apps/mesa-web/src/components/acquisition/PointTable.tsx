// 采集点位表格（通用组件，§2.8）：三列「名称或标识 · 配置摘要 · 操作」+ 分页。
//
// 边界（硬性）：本文件**不得**出现驱动名、协议字段名、资源 ID 判断，也不得 import
// 任何驱动/目录模型。它只认 `PointRowView`：行内放什么完全由页面层决定。
//
// 行级元素预算（§2.9）：
// - 名称 1 行；摘要 1 行（字段标签 + 值，已含地址摘要）；
// - 必要标识只在名称不足/重名时出现 1 段；
// - 状态标记最多 1 个（「已修改 · 有问题」合成**一条**短文案，不叠两个标签）；
// - 操作最多 3 个交互元素：编辑、更多（⋯）、必要时「查看已有项」；
// - 正常行不出现任何绿色/成功类标签（成功是默认态，不占视觉）。
import { useState } from "react";
import { Button, Pagination } from "antd";
import type { PointRowView } from "../../acquisitionRowView";
import { isNarrowViewport, useViewportWidth } from "../../useViewportWidth";

export interface PointTableActions {
  onEdit: (row: PointRowView) => void;
  /** 「更多」菜单：移除（先改草稿，可撤销）。
   */
  onRemove: (row: PointRowView) => void;
  /** 「更多」菜单：技术详情（完整点键 / 原始 JSON / 版本 / 身份来源）。 */
  onDetail: (row: PointRowView) => void;
  /** 「更多」菜单：复制标识（可选能力）。 */
  onCopyKey?: (row: PointRowView) => void;
}

const COLUMNS = "minmax(170px, 1fr) minmax(260px, 1.6fr) 132px";
// 窄屏：三列的最小宽度合计约 586px，480px 级视口会横向溢出到页面外。
// 这里不做「横向滚动」兜底（要点右侧操作还要先横拖，等于不可操作），而是把
// 同一份内容改成**单列堆叠**：名称/标识 → 摘要/状态 → 操作，信息一个不少。
const NARROW_COLUMNS = "minmax(0, 1fr)";

export function PointTable({
  rows,
  total,
  page,
  pageSize,
  pageCount,
  pageSizeOptions,
  onPageChange,
  onPageSizeChange,
  actions,
  isEditable,
  rowTestId = "point-row",
  rowData,
  emptyState,
}: {
  rows: PointRowView[];
  total: number;
  page: number;
  pageSize: number;
  pageCount: number;
  pageSizeOptions: number[];
  onPageChange: (page: number) => void;
  onPageSizeChange: (size: number) => void;
  actions: PointTableActions;
  /** 行是否可编辑（可编辑性判定不属于本组件，由页面层给出）。 */
  isEditable?: (row: PointRowView) => boolean;
  /** 行节点的 testid（页面层提供的查询锚点）。 */
  rowTestId?: string;
  /** 附加到行节点的 data-* 属性（如配置格式校验状态）。 */
  rowData?: (row: PointRowView) => Record<string, string>;
  emptyState?: React.ReactNode;
}) {
  // 「更多」当前展开的行（同一时刻只开一个，避免菜单叠在别处）。
  const [moreRowId, setMoreRowId] = useState<string | null>(null);
  // 窄屏判定只影响布局（列数），不影响任何数据与操作（§5.2 窄屏验收）。
  const narrow = isNarrowViewport(useViewportWidth());
  const columns = narrow ? NARROW_COLUMNS : COLUMNS;

  if (rows.length === 0) {
    return <div data-testid="point-table-empty">{emptyState}</div>;
  }

  return (
    <div data-testid="point-table" data-layout={narrow ? "stacked" : "columns"}>
      {/* 窄屏不渲染三列表头：三列已堆叠，表头会变成三个各占一行的空标签。
          行内每块仍带 aria-label，读屏与自动化都能认出这一段是什么。 */}
      {narrow ? null : (
        <div
          role="row"
          style={{
            display: "grid",
            gridTemplateColumns: columns,
            gap: 12,
            padding: "6px 8px",
            fontSize: 12,
            color: "#525252",
            borderBottom: "1px solid #e0e0e0",
          }}
        >
          <span role="columnheader">名称或标识</span>
          <span role="columnheader">配置摘要</span>
          <span role="columnheader">操作</span>
        </div>
      )}
      {rows.map((row) => {
        const editable = isEditable ? isEditable(row) : true;
        const markerText =
          row.marker === "added" ? "新增" : row.marker === "modified" ? "已修改" : null;
        // §2.9：标记与问题合成一条短文案，不叠成两个标签。
        const statusText = [markerText, row.issue ? "有问题" : null]
          .filter(Boolean)
          .join(" · ");
        return (
          <div
            key={row.rowId}
            role="row"
            data-row-id={row.rowId}
            data-testid={rowTestId}
            data-marker={row.marker ?? "none"}
            data-issue={row.issue ? "true" : "false"}
            {...(rowData ? rowData(row) : {})}
            style={{
              display: "grid",
              gridTemplateColumns: columns,
              gap: 12,
              alignItems: "start",
              padding: "8px",
              borderBottom: "1px solid #f0f0f0",
              position: "relative",
              // 窄屏（单列）时显式允许收缩：默认 `min-width: auto` 会让长点键/长摘要
              // 撑破容器，反而造成横向溢出（正是要修的问题）。
              minWidth: 0,
            }}
          >
            <div style={{ display: "grid", gap: 2, minWidth: 0 }} aria-label="名称或标识">
              <span data-testid="point-row-title" style={{ fontSize: 13, fontWeight: 600 }}>
                {row.title}
              </span>
              {row.identifier ? (
                <span
                  data-testid="point-row-identifier"
                  style={{
                    fontSize: 11,
                    color: "#525252",
                    fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace",
                    overflowWrap: "anywhere",
                  }}
                >
                  {row.identifier}
                </span>
              ) : null}
            </div>
            <div style={{ display: "grid", gap: 2, minWidth: 0 }} aria-label="配置摘要">
              <span
                data-testid="point-row-summary"
                style={{ fontSize: 12, color: "#262626", overflowWrap: "anywhere" }}
              >
                {row.summary}
              </span>
              {statusText ? (
                <span
                  data-testid="point-row-status"
                  style={{
                    fontSize: 11,
                    color: row.issue ? "#c62828" : "#8a6d3b",
                  }}
                >
                  {statusText}
                  {row.issue ? `：${row.issue.text}` : ""}
                </span>
              ) : null}
            </div>
            <div
              style={{ display: "flex", gap: 6, alignItems: "start", flexWrap: "wrap", minWidth: 0 }}
              aria-label="操作"
            >
              {editable ? (
                <Button size="small" data-testid="point-row-edit" onClick={() => actions.onEdit(row)}>
                  编辑
                </Button>
              ) : null}
              <Button
                size="small"
                type="text"
                aria-haspopup="menu"
                aria-expanded={moreRowId === row.rowId}
                aria-label="更多操作"
                data-testid="point-row-more"
                onClick={() => setMoreRowId((cur) => (cur === row.rowId ? null : row.rowId))}
              >
                ⋯
              </Button>
              {moreRowId === row.rowId ? (
                <div
                  role="menu"
                  data-testid="point-row-menu"
                  style={{
                    position: "absolute",
                    right: 8,
                    top: 36,
                    // 必须高于固定保存区（`zIndex: 5`）：两者同层时，后渲染的保存区会
                    // 盖住最后一行「更多」菜单的下半部分，导致「移除/技术详情」点不到。
                    zIndex: 20,
                    background: "#ffffff",
                    border: "1px solid #e0e0e0",
                    boxShadow: "0 2px 8px rgba(0,0,0,0.12)",
                    display: "grid",
                    minWidth: 128,
                  }}
                >
                  <Button
                    type="text"
                    size="small"
                    role="menuitem"
                    data-testid="point-row-detail"
                    onClick={() => {
                      setMoreRowId(null);
                      actions.onDetail(row);
                    }}
                  >
                    技术详情
                  </Button>
                  {actions.onCopyKey ? (
                    <Button
                      type="text"
                      size="small"
                      role="menuitem"
                      data-testid="point-row-copy-key"
                      onClick={() => {
                        setMoreRowId(null);
                        actions.onCopyKey?.(row);
                      }}
                    >
                      复制标识
                    </Button>
                  ) : null}
                  <Button
                    type="text"
                    size="small"
                    danger
                    role="menuitem"
                    data-testid="point-row-remove"
                    onClick={() => {
                      setMoreRowId(null);
                      actions.onRemove(row);
                    }}
                  >
                    移除
                  </Button>
                </div>
              ) : null}
            </div>
          </div>
        );
      })}
      <div
        data-testid="point-table-pagination"
        style={{ display: "flex", alignItems: "center", gap: 12, padding: "8px 0", flexWrap: "wrap" }}
      >
        <span style={{ fontSize: 12, color: "#525252" }}>共 {total} 项</span>
        {/* 每页条数用显式按钮而不是 locale 依赖的下拉：文案固定中文、可机械点击。 */}
        <span style={{ fontSize: 12, color: "#525252" }}>每页</span>
        <span style={{ display: "inline-flex", gap: 4 }}>
          {pageSizeOptions.map((size) => (
            <Button
              key={size}
              size="small"
              type={size === pageSize ? "primary" : "default"}
              data-testid={`page-size-${size}`}
              onClick={() => {
                if (size !== pageSize) onPageSizeChange(size);
              }}
            >
              {size}
            </Button>
          ))}
        </span>
        <span style={{ fontSize: 12, color: "#525252" }}>项</span>
        <Pagination
          size="small"
          current={page}
          pageSize={pageSize}
          total={total}
          showSizeChanger={false}
          showQuickJumper={false}
          onChange={(nextPage) => onPageChange(nextPage)}
        />
        <span style={{ fontSize: 11, color: "#8c8c8c" }}>
          第 {page} / {pageCount} 页
        </span>
      </div>
    </div>
  );
}
