// PR-C 第二批 A：Endpoint 采集保存**计划**（按 Driver 分流，纯函数、无 React/AntD）。
//
// 为什么必须单独一层而不是把 EndpointAcquisitionPane 里的
// `mergeAcquisitionTasks(...)` 机械换成 `mergeNckAcquisitionTasks(...)`：
// 两者产出的 `binding.config` **结构不同**（`{selections}` vs `{points}`），
// 且任务 kind 由 Endpoint 的 Driver 决定，不是"名字可互换的两种写法"。
// 机械替换会在非 NCK Endpoint 上写出 `nck.points.v1`（驱动整体拒绝），
// 在 NCK Endpoint 上写出 `mesa.resources.v1`（`configure` 直接 UNSUPPORTED_BINDING）。
// 因此这里把四条路径统一成**一个决策点**：
//   ① 读取（读取在 UI 加载时按 kind 分流，本层不重复读取）；
//   ② 编辑（按 kind 选 merge 实现）；
//   ③ 冲突检查（NCK 侧 Endpoint-wide point_key 唯一性 + 歧义拒绝）；
//   ④ 保存响应（本层给出"是否真有变更"，让 UI 不可能在 no-op 时报告成功）。
//
// 数据安全口径（本批的核心）：
// - 快照未就绪 ⇒ 直接拒绝（不产出任何可 PUT 的任务集）；
// - NCK 目标集与当前快照**逐字相等** ⇒ 拒绝并给 `NO_CHANGES`：UI 必须**不报告成功**，
//   也不应发 PUT（"保存接口 2xx"不等于"编辑已持久化"，这是旧 no-op 安全网误报的根因）；
// - 非 NCK 路径逐字沿用 `mergeAcquisitionTasks`（唯一差异见 `DRIVER_KIND_MISMATCH`：
//   旧实现在"非 NCK Endpoint 上存在 NCK 任务"时会 no-op 然后误报成功，本层改为显式拒绝）。

import {
  NCK_POINTS_KIND,
  isNckDriver,
  isTaskSnapshotReady,
  mergeAcquisitionTasks,
  type AcquisitionTaskShape,
  type NckPointSelection,
  type ResourceSelection,
  type TaskSnapshotState,
} from "./deviceModel";
import type { NckVariableDefinitionLike } from "./nckCollectability";
import {
  mergeNckAcquisitionTasks,
  type NckTaskRejectionCode,
} from "./nckTaskBinding";

/** 保存计划被拒绝的原因码（含 NCK 模型自己的码，便于 UI 原样透出）。 */
export type EndpointSaveCode =
  | "SNAPSHOT_NOT_READY"
  | "EMPTY_SELECTION"
  | "EMPTY_POINTS"
  | "NO_CHANGES"
  | "DRIVER_KIND_MISMATCH"
  | "NCK_DEFINITIONS_MISSING"
  | NckTaskRejectionCode;

export interface EndpointSavePlanInput {
  /** Endpoint 的 Driver id（唯一决定任务 kind 的输入）。 */
  driverId: string;
  /** 已成功加载的服务端任务快照（不是 []，除非服务端确实没有任务）。 */
  tasks: AcquisitionTaskShape[];
  snapshotState: TaskSnapshotState;
  loadedEndpointId: string | null;
  currentEndpointId: string;
  /** 采集周期（仅 poll schedule 写入；其它 schedule 逐字保留）。 */
  intervalMs: number;
  /** 非 NCK 路径：Picker 产出的旧结构选择。 */
  selections?: ResourceSelection[];
  /** NCK 路径：待提交点位（来自快照 + 本批编辑，不自行推导身份）。 */
  nckPoints?: NckPointSelection[];
  /**
   * NCK 路径：定义目录（第二批 B 注入）。缺失即 fail-closed——
   * 没有 `scope` 就无法执行 P0-1 的准入，宁可禁用保存也不静默放行。
   */
  nckDefinitions?: Readonly<Record<string, NckVariableDefinitionLike>> | null;
}

export type EndpointSavePlan =
  | {
      ok: true;
      driverKind: "canonical";
      tasks: AcquisitionTaskShape[];
      preservedCount: number;
      /** 仅信息性：非 NCK 路径不做回读校验（行为与旧实现一致）。 */
      changed: boolean;
    }
  | {
      ok: true;
      driverKind: "nck";
      tasks: AcquisitionTaskShape[];
      preservedCount: number;
      changed: boolean;
      /** 被写入的任务 id（保存后回读核对用）。 */
      taskId: string;
      /** 归一化后的待提交点位（保存后回读逐字对比用）。 */
      points: NckPointSelection[];
    }
  | { ok: false; code: EndpointSaveCode; message: string };

/**
 * 生成保存计划。**不**发请求、**不**改入参；调用方拿到 `ok:true` 才可 PUT。
 *
 * 顺序刻意固定为「快照门 → Driver 分流 → 结构/准入/冲突 → 变更判定」：
 * 快照未就绪时任何唯一性/歧义判断都建立在不可信输入上，必须先拒绝。
 */
export function planEndpointTaskSave(input: EndpointSavePlanInput): EndpointSavePlan {
  if (
    !isTaskSnapshotReady(input.snapshotState, input.loadedEndpointId, input.currentEndpointId)
  ) {
    return {
      ok: false,
      code: "SNAPSHOT_NOT_READY",
      message:
        "任务快照未就绪（加载中/失败/归属不符）：禁止保存，避免用空快照覆盖已有任务",
    };
  }
  return isNckDriver(input.driverId) ? planNckSave(input) : planCanonicalSave(input);
}

/**
 * 非 NCK 旧路径：逐字沿用 `mergeAcquisitionTasks`，不引入任何新的编辑语义
 * （否则就是"非 NCK 行为变化"）。这里只加了一道匹配性检查：
 * 若任务集里存在 `nck.points.v1`，说明 Driver 与任务集不匹配——旧实现会
 * no-op 并让 UI 误报"已保存"，本层改为显式拒绝。
 */
function planCanonicalSave(input: EndpointSavePlanInput): EndpointSavePlan {
  const foreignNckTasks = input.tasks.filter((task) => task?.binding?.kind === NCK_POINTS_KIND);
  if (foreignNckTasks.length) {
    return {
      ok: false,
      code: "DRIVER_KIND_MISMATCH",
      message: `Endpoint Driver（${input.driverId || "未知"}）不是 NCK，但任务集中存在 ${NCK_POINTS_KIND} 任务（${foreignNckTasks
        .map((t) => t.id)
        .join("、")}）：拒绝保存（旧实现会原样返回并误报成功）`,
    };
  }
  const selections = input.selections ?? [];
  if (!selections.length) {
    // 与旧文案逐字一致：非 NCK 的 UI 行为（提示层级、文案）不得变化。
    return { ok: false, code: "EMPTY_SELECTION", message: "请先加入点位" };
  }
  const tasks = mergeAcquisitionTasks(input.tasks, {
    interval_ms: input.intervalMs,
    selections,
  });
  return {
    ok: true,
    driverKind: "canonical",
    tasks,
    preservedCount: tasks.length - 1,
    changed: !sameTaskSet(input.tasks, tasks),
  };
}

/** NCK 路径：唯一编辑入口，全部校验（scope 准入 / 歧义 / 冲突）在模型内执行。 */
function planNckSave(input: EndpointSavePlanInput): EndpointSavePlan {
  const points = input.nckPoints ?? [];
  if (!points.length) {
    // 驱动不接受空 points；清空点位的正确做法是删除任务（本批不提供该动作）。
    return {
      ok: false,
      code: "EMPTY_POINTS",
      message: "点位列不能为空：请至少保留一个点位（空任务会被驱动拒绝）",
    };
  }
  if (!input.nckDefinitions) {
    return {
      ok: false,
      code: "NCK_DEFINITIONS_MISSING",
      message:
        "定义目录未注入/不可用：无法确认 scope，禁止提交（fail-closed；目录接线见第二批 B）",
    };
  }
  const merged = mergeNckAcquisitionTasks({
    tasks: input.tasks,
    driverId: input.driverId,
    points,
    definitions: input.nckDefinitions,
    snapshotState: input.snapshotState,
    loadedEndpointId: input.loadedEndpointId,
    currentEndpointId: input.currentEndpointId,
    intervalMs: input.intervalMs,
  });
  if (!merged.ok) {
    // 歧义 / 结构非法 / 准入失败 / 冲突：一律拒绝，**不**返回部分结果
    // （调用方不得拿旧 tasks 凑合，那正是"覆盖原任务"的事故路径）。
    return { ok: false, code: merged.code, message: merged.message };
  }
  if (sameTaskSet(input.tasks, merged.tasks)) {
    return {
      ok: false,
      code: "NO_CHANGES",
      message: "未检测到实际修改（目标任务集与当前快照逐字相等）：未发送保存请求",
    };
  }
  return {
    ok: true,
    driverKind: "nck",
    tasks: merged.tasks,
    preservedCount: merged.tasks.length - 1,
    changed: true,
    taskId: merged.taskId,
    points: merged.points,
  };
}

/**
 * 任务集逐字相等判定（顺序敏感：任务顺序也是配置的一部分）。
 * 用稳定 JSON（对象 key 排序）比较，避免"同内容不同 key 顺序"被误判成变更。
 */
export function sameTaskSet(
  a: AcquisitionTaskShape[],
  b: AcquisitionTaskShape[],
): boolean {
  if (a.length !== b.length) return false;
  return a.every((task, i) => stableJson(task) === stableJson(b[i]));
}

/**
 * 点位数组逐字相等判定（保存后回读核对用）。
 * 只比较**点位**而不是整任务：服务端可能对 schedule 做归一化，但点位是
 * 本次编辑的实体，"编辑是否持久化"只应由点位是否原样回来决定。
 */
export function samePoints(a: NckPointSelection[], b: NckPointSelection[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((point, i) => stableJson(point) === stableJson(b[i]));
}

/** JSON 稳定序列化：对象 key 排序、数组保序、`undefined` 丢弃（对齐 JSON 语义）。 */
function stableJson(value: unknown): string {
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
