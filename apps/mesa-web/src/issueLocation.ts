// 保存失败 `issues[].path` → 稳定 `rowId` 的**定位适配**（§5.2-5，纯函数、无 React/AntD）。
//
// 为什么要单独一层：后端 `path` 是**提交快照的 JSON 路径**（`tasks[i].binding.config.points[j]`
// 这类），而页面上的行身份是 `rowId`（`pointRows.ts`，§4.8）。两者之间只隔着一次
// 「本次提交的任务数组下标 + 任务内下标」的换算，因此不需要改后端契约——**存在多种
// path 形态 ≠ 必须先改契约**。
//
// 真实 path 形态（动手前逐个核实，不是猜的）：
// 1. 任务级（无法指向某个点位）：
//    - `tasks[i].binding.kind`：`crates/core-api/src/binding_gate.rs:60-67`（UNSUPPORTED_BINDING）
//    - `tasks[i].binding`：`crates/core-api/src/binding_gate.rs:97-102`（INVALID_BINDING_CONFIG）
//    - `tasks[i].selections`：`crates/core-types/src/resource.rs:262-268`（INVALID_STRUCTURE）
//    - `tasks[i].binding.config`：`crates/core-types/src/point.rs:818-845`（信封层结构错误）
// 2. 点位级（NCK，`nck.points.v1`）：`tasks[i].binding.config.points[j]` 及其子键
//    `kind` / `point_key` / `parameters`：`crates/core-types/src/point.rs:854-928`。
// 3. 点位级（通用，`mesa.resources.v1`）：`tasks[i].selections[j]` 及其子键
//    `resource_id` / `outputs` / `parameters`：`crates/core-types/src/resource.rs:270-359`。
// 4. 字段级：`...parameters.<field>`：`crates/core-types/src/schema.rs:293-301`
//    （`validate_instance` 用 `{root}.{field.key}` 拼路径，root 即上面两处 `.parameters`）。
//    注意：Core 对 NCK 点位 `parameters` **完全不透明**（`point.rs:915-928` 只判 JSON 对象），
//    因此字段级 path 在真实后端里只会出现在通用资源选择下；NCK 侧的字段级形态按
//    `...points[j].parameters.<field>` 同一规则处理（前端不做 kind 特判，见下）。
//
// 不可定位的三种情形（明确说明，**不猜目标**）：
// - 任务级 path；
// - 指向「保留任务」（不在本页编辑，用户改不了）；
// - 无法识别的 path（含下标越界、非 `tasks[...]` 前缀的空路径等）。

/** 无法自动定位的原因（只用于文案分层与断言，不含任何猜测目标）。 */
export type UnlocatableReason =
  /** 任务级：指向整个任务（binding/kind/结构），不指向某个点位。 */
  | "task"
  /** 保留任务：本次没有编辑它，页面上没有对应行。 */
  | "preserved"
  /** 无法识别：路径语法不认识、下标越界、或快照缺失。 */
  | "unrecognized";

/** 定位成功：已换算成稳定行身份（+ 可选字段）。 */
export interface IssueRowLocation {
  kind: "row";
  /** 提交数组中被编辑任务的下标（`tasks[t]` 的 t）。 */
  taskIndex: number;
  /** 任务内点位/选择下标（`points[j]` / `selections[j]` 的 j）。 */
  itemIndex: number;
  /** 该行的稳定身份（`pointRows.ts` 分配，只用于界面定位）。 */
  rowId: string;
  /** 字段级 path 的字段 key；行级 path 为 null。 */
  fieldKey: string | null;
}

/** 定位失败：保留详情，但**不得**猜一个目标跳过去。 */
export interface IssueUnlocatable {
  kind: "unlocatable";
  reason: UnlocatableReason;
  /** 技术详情里的简短说明（不用于正文）。 */
  detail: string;
}

export type IssueLocation = IssueRowLocation | IssueUnlocatable;

/**
 * 本次提交的行快照（**唯一**的 path→rowId 依据）。
 *
 * 为什么必须在提交时固化：`path` 里的下标是**提交那一刻**的数组下标；保存返回后
 * 草稿可能已被再次编辑（行增删），用实时列表换算会指错行。
 */
export interface SubmittedRowSnapshot {
  /** 被编辑任务在提交数组里的下标；其余下标都是保留任务。 */
  taskIndex: number;
  /** 提交任务数组的长度（用于区分「越界」与「保留任务」）。 */
  taskCount: number;
  /** 被编辑任务内每一行按提交顺序的稳定 rowId。 */
  rowIds: readonly string[];
}

const TASK_PREFIX = /^tasks\[(\d+)\]([\s\S]*)$/;
const POINTS_ITEM = /^\.binding\.config\.points\[(\d+)\]([\s\S]*)$/;
const SELECTIONS_ITEM = /^\.selections\[(\d+)\]([\s\S]*)$/;
const PARAMETERS_FIELD = /^\.parameters\.([^.[\]]+)([\s\S]*)$/;

/** 任务级前缀（命中即「指向整个任务」，不再往点位里找）。 */
const TASK_LEVEL_PREFIXES = [
  ".binding.kind",
  ".binding",
  ".schedule",
  ".id",
  ".mode",
  ".selections",
];

function unlocatable(reason: UnlocatableReason, detail: string): IssueUnlocatable {
  return { kind: "unlocatable", reason, detail };
}

/**
 * 把一条 `issue.path` 映射到稳定行身份。
 *
 * `snapshot` 为 `null`（没有提交快照）时一律不可定位——宁可让用户看原始 path，
 * 也不能拿实时列表的下标去猜一行。
 */
export function locateIssuePath(
  path: string,
  snapshot: SubmittedRowSnapshot | null,
): IssueLocation {
  if (!snapshot) {
    return unlocatable("unrecognized", "缺少本次提交的快照：无法把下标换算成行身份");
  }
  const text = typeof path === "string" ? path : "";
  const taskMatch = TASK_PREFIX.exec(text);
  if (!taskMatch) {
    return unlocatable(
      "unrecognized",
      `路径 \`${text || "(空)"}\` 不是 \`tasks[i]...\` 形态（例如连接级 path），无法对应到点位`,
    );
  }
  const taskIndex = Number(taskMatch[1]);
  const rest = taskMatch[2] ?? "";
  if (taskIndex !== snapshot.taskIndex) {
    if (taskIndex < snapshot.taskCount) {
      return unlocatable(
        "preserved",
        `路径指向保留任务 \`tasks[${taskIndex}]\`（不在本页编辑，保存时原样保留）`,
      );
    }
    return unlocatable(
      "unrecognized",
      `路径里的任务下标 ${taskIndex} 超出本次提交范围（共 ${snapshot.taskCount} 个任务）`,
    );
  }

  // 点位/选择下标：两种 binding kind 各一种写法。
  const item = POINTS_ITEM.exec(rest) ?? SELECTIONS_ITEM.exec(rest);
  if (!item) {
    // 任务级或子路径不认识：都明确说「无法自动定位」，绝不挑一行跳过去。
    if (TASK_LEVEL_PREFIXES.some((prefix) => rest === prefix || rest.startsWith(`${prefix}.`) || rest.startsWith(`${prefix}[`))) {
      return unlocatable(
        "task",
        `路径 \`${text}\` 指向整个任务（binding/schedule/结构校验），不指向某个点位`,
      );
    }
    return unlocatable(
      "unrecognized",
      `路径 \`${text}\` 的任务内写法不认识（既不是 points[j] 也不是 selections[j]）`,
    );
  }
  const itemIndex = Number(item[1]);
  const tail = item[2] ?? "";
  if (itemIndex >= snapshot.rowIds.length) {
    return unlocatable(
      "unrecognized",
      `路径里的点位下标 ${itemIndex} 超出本次提交行数（共 ${snapshot.rowIds.length} 行）`,
    );
  }
  const rowId = snapshot.rowIds[itemIndex] ?? "";
  if (!rowId) {
    return unlocatable("unrecognized", `下标 ${itemIndex} 在本次提交快照里没有对应的行身份`);
  }
  return {
    kind: "row",
    taskIndex,
    itemIndex,
    rowId,
    fieldKey: fieldKeyOf(tail),
  };
}

/**
 * 子路径 → 字段 key。
 *
 * 只有 `...parameters.<field>` 才算字段级；`kind` / `point_key` / `parameters` /
 * `outputs` 这些子键属于**行本身**（行级），返回 null：定位到行，不假装能高亮某个表单字段。
 */
function fieldKeyOf(tail: string): string | null {
  const match = PARAMETERS_FIELD.exec(tail);
  if (!match) return null;
  const key = match[1];
  return key ? key : null;
}

/** 不可定位原因 → 面向用户的说明（正文用，不含原始 path）。 */
export function describeUnlocatable(reason: UnlocatableReason): string {
  if (reason === "task") return "该问题定位到整个任务，无法对应到某个点位";
  if (reason === "preserved") return "该问题属于保留任务（不在此页编辑），无法定位到点位";
  return "该问题的路径无法识别，无法定位到点位";
}

/** 在问题列表里挑**第一条可定位**的问题（其余留给技术详情与用户排查）。 */
export function firstLocatableIssue<T extends { path: string }>(
  issues: readonly T[],
  snapshot: SubmittedRowSnapshot | null,
): { issue: T; location: IssueRowLocation } | null {
  for (const issue of issues) {
    const location = locateIssuePath(issue.path, snapshot);
    if (location.kind === "row") return { issue, location };
  }
  return null;
}
