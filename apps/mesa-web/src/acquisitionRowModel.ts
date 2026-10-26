// 页面层的「点位行」装配（**允许**按驱动分流，因为这是页面层，不是通用组件）。
//
// 本模块的唯一职责：把两条数据路径（NCK 点位 / 通用资源选择）归一成
// `PointRowView`，供通用表格/搜索/抽屉使用。驱动判断到此为止——
// `PointTable` / `PointSearch` / `PointDrawer` / `DraftSaveBar` 里不会再出现它。
//
// 辨识口径（§2.6）：
// - 名称只取既有事实：定义目录 label（中文优先）→ 变量/资源 id → 通用名；
// - **不得按地址编造业务名**；
// - 原生点位没有可靠业务名 ⇒ 显示「原生点位」+ 带字段标签的地址摘要
//   （不能只给无说明的五段数字）；
// - 技术详情（完整点键、原始参数、定义版本、身份来源）只放进详情抽屉。
import { localizedText } from "./localizedText";
import { pointSummary } from "./pointSummary";
import { nckPointDisplay } from "./nckPointDisplay";
import type { NckPointSelection, ResourceSelection } from "./deviceModel";
import type { ResourceDescriptor } from "./types";
import type { NckDefinitionLike } from "./nckPointModel";
import {
  ensureDistinguishable,
  shortPointKey,
  type PointDetailEntry,
  type PointRowView,
} from "./acquisitionRowView";

/** 已有点位的配置格式校验结论（页面层判定结果，行内只用它出问题标记）。 */
export interface RowVerification {
  status: "verified" | "unverified" | "pending";
  code?: string;
  message?: string;
}

function detailEntries(entries: PointDetailEntry[]): PointDetailEntry[] {
  return entries;
}

/** NCK 点位行。 */
export function buildNckPointRows(input: {
  points: readonly NckPointSelection[];
  rowIds: readonly string[];
  verificationOf: (index: number) => RowVerification | undefined;
  definitionOf: (point: NckPointSelection) => NckDefinitionLike | null;
}): PointRowView[] {
  const rows = input.points.map((point, index) => {
    const definition = input.definitionOf(point);
    const display = nckPointDisplay(point, definition);
    const verification = input.verificationOf(index);
    const variableId =
      typeof point.parameters?.variable_id === "string" ? point.parameters.variable_id : "";
    const rawJson = JSON.stringify(point.parameters ?? {});
    const issue =
      verification?.status === "unverified"
        ? {
            // 面向用户：可处理原因；原始错误码收进技术详情（§3.3）。
            text: verification.message ?? "配置格式校验未通过",
            detail: verification.code,
          }
        : undefined;
    return {
      rowId: input.rowIds[index] ?? "",
      title: display.title,
      summary: display.address,
      identifierHint: shortPointKey(point.point_key),
      searchText: [display.title, display.address, point.point_key, variableId, rawJson].join("\n"),
      issue,
      detail: detailEntries([
        { label: "point_key", value: point.point_key, mono: true },
        { label: "点位类型", value: point.kind, mono: true },
        { label: "定义版本", value: display.revision.replace(/^定义版本：/, ""), mono: true },
        { label: "原始参数（JSON）", value: rawJson, mono: true },
        {
          label: "身份来源",
          value:
            point.kind === "nck_native"
              ? "原生字段（不适用目录 scope）"
              : "服务端 identity-preview 重算",
        },
        ...(verification?.code
          ? [{ label: "校验错误码", value: verification.code, mono: true }]
          : []),
      ]),
    } satisfies PointRowView;
  });
  return ensureDistinguishable(rows);
}

/** 通用资源选择行（一条选择可含多个输出）。 */
export function buildSelectionRows(input: {
  selections: readonly ResourceSelection[];
  rowIds: readonly string[];
  resources: readonly ResourceDescriptor[];
}): PointRowView[] {
  const rows = input.selections.map((sel, index) => {
    const resource = input.resources.find((item) => item.id === sel.resource_id);
    const title = localizedText(resource?.label) || sel.resource_id;
    const pointKeys = sel.outputs.map((output) => output.point_key);
    const summary = pointSummary({
      point_key: pointKeys.join(", "),
      parameters: sel.parameters,
      schema: resource?.parameters ?? null,
    });
    const rawJson = JSON.stringify(sel.parameters ?? {});
    return {
      rowId: input.rowIds[index] ?? "",
      title,
      summary: summary || pointKeys.join(", ") || sel.resource_id,
      identifierHint: pointKeys[0] ? shortPointKey(pointKeys[0]) : sel.resource_id,
      searchText: [title, summary, sel.resource_id, pointKeys.join(" "), rawJson].join("\n"),
      detail: detailEntries([
        { label: "资源", value: sel.resource_id, mono: true },
        { label: "point_key", value: pointKeys.length ? pointKeys.join(", ") : "（无输出）", mono: true },
        { label: "原始参数（JSON）", value: rawJson, mono: true },
        { label: "定义版本", value: "不适用（通用资源）" },
        { label: "身份来源", value: "输出命名（point_key 由所选输出决定）" },
      ]),
    } satisfies PointRowView;
  });
  return ensureDistinguishable(rows);
}
