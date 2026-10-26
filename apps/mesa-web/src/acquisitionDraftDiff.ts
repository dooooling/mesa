// 草稿差异 + 保存区状态机（纯函数，无 React/AntD，无驱动判断）。
//
// 两层职责刻意分开：
// 1. `diffDraft`：把「基线（加载/上次保存成功的快照）」与「当前草稿」比成
//    行级标记与计数，用于**展示**（行内「新增/已修改」、保存区摘要）。
//    —— 这不是保存判据。保存是否真有必要由 `endpointSavePlan`（完整有效配置
//    逐字比较）决定，两者口径不同、用途不同，不得互相替代（§4.4.2）。
// 2. `saveBarView`：保存区「可见 / 文案 / 入口」的唯一状态机（§4.7）。
//
// 三条硬规则（评审补充）：
// - 无差异、且没有正在保存/待核对/恢复失败 ⇒ 保存区**完全不渲染**；
// - 正在保存 / 待核对 / 恢复采集失败 ⇒ 即使没有差异也保留处理入口；
// - 摘要必须把**点位变更**与**周期变更**分开表述，避免「N 项修改」把周期算进去。

import type { RowMarker } from "./acquisitionRowView";

/** 草稿行（只带差异判定所需的两个字段）。 */
export interface DraftRowRef {
  rowId: string;
  /** 有效配置的稳定序列化（同内容不同键序不得算变更）。 */
  contentKey: string;
}

export interface DraftBaseline {
  rows: DraftRowRef[];
  /** 服务端已生效的周期；`null` = 没有可比对的已保存值（不得宣称「改了周期」）。 */
  intervalMs: number | null;
}

export interface DraftDiff {
  added: number;
  modified: number;
  removed: number;
  /** 点位变更总数（新增 + 修改 + 移除）。 */
  pointChangeCount: number;
  /** 行标记（按 rowId）：只有真正变化的行才有。 */
  rowMarkers: Record<string, RowMarker>;
  intervalChanged: boolean;
  intervalFrom: number | null;
  intervalTo: number;
  /** 是否存在任何实际差异（点位或周期）。 */
  hasChanges: boolean;
  /** 可直接展示的摘要文案；无差异时为空串。 */
  summary: string;
}

export interface DraftDiffInput {
  baseline: DraftBaseline | null;
  draft: DraftRowRef[];
  intervalMs: number;
}

/**
 * 计算草稿差异。
 *
 * 关键判定（§4.8）：
 * - **修改后恢复原值 ⇒ 取消标记**：按内容逐字比较，不是「改过就标记」；
 * - **新增后移除 ⇒ 抵消**：被删掉的新增行不再出现在草稿里，差异自然归零；
 * - **删除后重新添加**：新 rowId（只用于界面定位），但只要**有效配置与顺序**
 *   与基线一致 ⇒ 计数归零、保存区收起（rowId 不参与差异判定）。
 *
 * 因此最后一步是「序列级复核」：内容序列一致且周期未变时，把行级计数清零。
 */
export function diffDraft(input: DraftDiffInput): DraftDiff {
  const baseline = input.baseline;
  const intervalFrom = baseline?.intervalMs ?? null;
  const intervalTo = input.intervalMs;
  const intervalChanged = intervalFrom !== null && intervalFrom !== intervalTo;

  if (!baseline) {
    // 基线未知（快照未就绪/加载失败）：**不**猜测差异。保存区由「待核对/失败」
    // 等其它条件驱动，绝不能靠猜出来的「有修改」把保存区点亮。
    return {
      added: 0,
      modified: 0,
      removed: 0,
      pointChangeCount: 0,
      rowMarkers: {},
      intervalChanged: false,
      intervalFrom,
      intervalTo,
      hasChanges: false,
      summary: "",
    };
  }

  const baselineByRow = new Map<string, string>();
  for (const row of baseline.rows) baselineByRow.set(row.rowId, row.contentKey);
  const draftRowIds = new Set(input.draft.map((row) => row.rowId));

  let added = 0;
  let modified = 0;
  const rowMarkers: Record<string, RowMarker> = {};
  for (const row of input.draft) {
    const before = baselineByRow.get(row.rowId);
    if (before === undefined) {
      added += 1;
      rowMarkers[row.rowId] = "added";
    } else if (before !== row.contentKey) {
      modified += 1;
      rowMarkers[row.rowId] = "modified";
    }
  }
  let removed = 0;
  for (const row of baseline.rows) {
    if (!draftRowIds.has(row.rowId)) removed += 1;
  }

  // 序列级复核：有效配置与顺序完全一致 ⇒ 视为没有点位变更（覆盖
  // 「删除后重新添加同一配置」这种 rowId 变了但配置没变的场景）。
  const sameSequence =
    baseline.rows.length === input.draft.length &&
    baseline.rows.every((row, i) => row.contentKey === input.draft[i]!.contentKey);

  const finalAdded = sameSequence ? 0 : added;
  const finalModified = sameSequence ? 0 : modified;
  const finalRemoved = sameSequence ? 0 : removed;
  const pointChangeCount = finalAdded + finalModified + finalRemoved;
  const diff: DraftDiff = {
    added: finalAdded,
    modified: finalModified,
    removed: finalRemoved,
    pointChangeCount,
    rowMarkers: sameSequence ? {} : rowMarkers,
    intervalChanged,
    intervalFrom,
    intervalTo,
    hasChanges: pointChangeCount > 0 || intervalChanged,
    summary: "",
  };
  return { ...diff, summary: changeSummaryText(diff) };
}

/**
 * 变更摘要文案（§2.3 / §4.8）。
 *
 * 形态：`3 项点位变更（新增 1 · 修改 1 · 移除 1）；周期另有修改：1000 → 500 ms`
 * - 点位与周期**分开表述**：周期不并入「N 项点位变更」的计数；
 * - 没有变化的维度不出现（不写「新增 0」）；
 * - 周期维度用「另有修改」措辞，避免被读成点位数量的一部分。
 */
export function changeSummaryText(diff: DraftDiff): string {
  const parts: string[] = [];
  if (diff.pointChangeCount > 0) {
    const breakdown: string[] = [];
    if (diff.added > 0) breakdown.push(`新增 ${diff.added}`);
    if (diff.modified > 0) breakdown.push(`修改 ${diff.modified}`);
    if (diff.removed > 0) breakdown.push(`移除 ${diff.removed}`);
    const suffix = breakdown.length ? `（${breakdown.join(" · ")}）` : "";
    parts.push(`${diff.pointChangeCount} 项点位变更${suffix}`);
  }
  if (diff.intervalChanged) {
    const from = diff.intervalFrom === null ? "未保存" : String(diff.intervalFrom);
    // 与文档示例同形：`周期 1000 → 500 ms`（单位只写一次，避免「1000 ms → 500 ms」的重复）。
    parts.push(`周期另有修改：${from} → ${diff.intervalTo} ms`);
  } else if (diff.pointChangeCount === 0 && diff.intervalFrom !== null) {
    // 无差异：摘要为空（保存区不渲染）。
    return "";
  }
  return parts.join("；");
}

// ---- 保存区状态机（§4.7） -------------------------------------------------

export type SaveBarMode =
  | "hidden"
  | "pending"
  | "saving"
  | "pending-verify"
  | "restart-failed"
  | "saved"
  | "run-state-unknown";

export interface SaveBarInput {
  hasChanges: boolean;
  saving: boolean;
  pendingVerify: boolean;
  restartFailed: boolean;
  /** 回读一致后的结果（只展示一次，随后收起）。 */
  savedResult: "stopped" | "restarted" | null;
  runState: "running" | "stopped" | "unknown";
  /** 用户选择：保存后是否恢复采集（运行中才有意义）。 */
  restart: boolean;
  /**
   * 页面层的「现在允许保存吗」（快照门 / 准入 / 冲突 / 完整 Schema / 无修改…
   * 全部判定都留在页面层）。本状态机只负责把它映射成按钮可用性。
   */
  canSave: boolean;
}

export interface SaveBarView {
  visible: boolean;
  mode: SaveBarMode;
  primaryLabel: string;
  primaryDisabled: boolean;
  /** 次级选项文案（切换「保存后保持停止 / 恢复采集」），已停止时为 null。 */
  secondaryLabel: string | null;
  /** 一行中断/范围说明（仅在需要的那一刻出现）。 */
  note: string | null;
  /** 持久说明（待核对 / 恢复采集失败）。 */
  notice: string | null;
  showDiscard: boolean;
  showRecheck: boolean;
  showRetryStart: boolean;
  showRefreshState: boolean;
  /** 成功结果文案（只用一次）。 */
  resultText: string | null;
}

/** 中断说明统一文案：不承诺时长（§4.4.5，跨驱动无测量依据）。 */
export const INTERRUPT_NOTE = "保存会暂时中断当前连接的采集";
/** 已停止时的范围说明：只写配置，不自动启动。 */
export const STOPPED_NOTE = "保存只写入配置，不会启动采集";
/** 运行状态未知时禁止一切保存（含只写配置）。 */
export const RUN_STATE_UNKNOWN_NOTE = "运行状态未知，无法保存";

/**
 * 保存区视图状态机。
 *
 * 可见条件（评审补充规则 1/2）：
 * - 有草稿差异 ⇒ 可见（待保存）；
 * - 正在保存 / 结果待核对 / 恢复采集失败 ⇒ **即使没有差异也可见**，
 *   并保留对应处理入口（进度 / 重新核对 / 重试启动）；
 * - 以上都不成立 ⇒ 完全隐藏（不是禁用）。
 */
export function saveBarView(input: SaveBarInput): SaveBarView {
  const mode = resolveMode(input);
  const visible = mode !== "hidden";
  const primaryLabel = resolvePrimaryLabel(input, mode);
  const running = input.runState === "running";
  return {
    visible,
    mode,
    primaryLabel,
    primaryDisabled:
      !input.canSave ||
      mode === "saving" ||
      mode === "pending-verify" ||
      mode === "restart-failed" ||
      mode === "saved" ||
      mode === "run-state-unknown",
    secondaryLabel:
      mode === "pending" && running
        ? input.restart
          ? "保存后保持停止"
          : "保存后恢复采集"
        : null,
    note: resolveNote(input, mode),
    // 待处理结果的说明与入口**独立于**是否还有草稿差异：恢复采集失败后用户又改了
    // 东西，也要能看见「上次保存后采集未恢复 ［重试启动］」（§4.7 补充）。
    notice:
      input.pendingVerify
        ? "保存结果待核对"
        : input.restartFailed
          ? "配置已保存，但恢复采集失败"
          : null,
    showDiscard: mode === "pending" && input.hasChanges,
    showRecheck: input.pendingVerify,
    showRetryStart: input.restartFailed,
    showRefreshState: mode === "run-state-unknown",
    resultText:
      mode === "saved"
        ? input.savedResult === "restarted"
          ? "配置已保存，采集已启动"
          : input.savedResult === "stopped"
            ? "配置已保存，当前停止"
            : null
        : null,
  };
}

function resolveMode(input: SaveBarInput): SaveBarMode {
  // 优先级：进行中的动作 > 待核对 > 待保存（含未知） > 恢复失败 > 成功结果 > 隐藏。
  // 「待保存」排在「恢复失败」之前：产生新修改后主按钮必须回到可保存状态，
  // 而已保存但启动失败的处理入口由 notice/showRetryStart 独立保留。
  if (input.saving) return "saving";
  if (input.pendingVerify) return "pending-verify";
  if (input.hasChanges) return input.runState === "unknown" ? "run-state-unknown" : "pending";
  if (input.restartFailed) return "restart-failed";
  if (input.savedResult) return "saved";
  return "hidden";
}

function resolvePrimaryLabel(input: SaveBarInput, mode: SaveBarMode): string {
  if (mode === "saving") return "处理中…";
  if (mode === "pending-verify") return "保存配置";
  if (mode === "restart-failed") return "保存配置";
  if (input.runState === "unknown") return "保存配置";
  if (input.runState === "stopped") return "保存配置";
  // 运行中（含连接中/重连中过渡态，由页面层归入 running）：
  return input.restart ? "保存并恢复采集" : "保存并停止采集";
}

function resolveNote(input: SaveBarInput, mode: SaveBarMode): string | null {
  if (mode === "run-state-unknown") return RUN_STATE_UNKNOWN_NOTE;
  if (mode !== "pending") return null;
  return input.runState === "running" ? INTERRUPT_NOTE : STOPPED_NOTE;
}

// ---- 稳定序列化（差异判定专用） -------------------------------------------

/**
 * 草稿差异用的稳定序列化：对象 key 排序、数组保序、`undefined` 丢弃。
 *
 * NOTE：只服务**展示层**的差异与行标记；保存是否必要仍由完整任务集比较决定
 * （§4.4.2）。两者不要互相替代。
 */
export function draftContentKey(value: unknown): string {
  return JSON.stringify(canonicalize(value));
}

function canonicalize(value: unknown): unknown {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(canonicalize);
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(value as Record<string, unknown>).sort()) {
    const v = canonicalize((value as Record<string, unknown>)[key]);
    if (v !== undefined) out[key] = v;
  }
  return out;
}
