// PR-C P0-2：NCK 采集任务的编辑/提交纯模型（双真值收敛 + 编辑安全）。
//
// 背景（已核实）：旧模型 `splitAcquisitionTasks` / `selectionsOf` 只认
// `mesa.resources.v1`、只解析 `{selections}`，`mergeAcquisitionTasks` 固定写
// 旧 kind。于是在已有 `nck.points.v1` 任务的 NCK Endpoint 上编辑点位，会**再
// 追加一个旧 kind 任务** ⇒ NCK `configure` 整体拒绝，连原有正确任务一起挂。
//
// 本模块提供可单测的替代路径（不改 Core / Driver，也不改 UI 组件）：
// 1. `bindingKindForDriver` / `assembleBinding`（deviceModel）按 Endpoint
//    Driver 选 kind，NCK 恒 `nck.points.v1`，非 NCK 恒 `mesa.resources.v1`；
// 2. `parseTaskBinding` 严格区分 `{points}` 与 `{selections}` 两种结构：
//    NCK 只读 points，其它只读 selections，**不**把 points 当 ResourceSelection；
// 3. `mergeNckAcquisitionTasks` 更新已有 NCK 任务时保留任务 ID / 合理保留
//    Schedule / 其它任务逐字不变；
// 4. 歧义（多个候选）、结构非法、快照未就绪一律拒绝，绝不「取第一个」；
// 5. 提交前做 Endpoint-wide point_key 唯一性校验（Core 侧仍会再拦一次，
//    这里只是让错误在保存前暴露，并给出与 Core 相同口径的拒绝）。
//
// 纯函数文件：不 import React / AntD / 请求层。

import {
  CANONICAL_RESOURCES_KIND,
  NCK_POINTS_KIND,
  assembleBinding,
  bindingKindForDriver,
  isNckDriver,
  isTaskSnapshotReady,
  type AcquisitionTaskShape,
  type NckPointSelection,
  type ResourceSelection,
  type TaskScheduleShape,
  type TaskSnapshotState,
} from "./deviceModel";
import {
  validateNckPointSubmission,
  type NckVariableDefinitionLike,
} from "./nckCollectability";

// ---------------------------------------------------------------------------
// 1. Binding 解析（两种结构严格分开）
// ---------------------------------------------------------------------------

export type BindingParseCode =
  | "UNSUPPORTED_BINDING_KIND"
  | "INVALID_BINDING_STRUCTURE"
  | "INVALID_POINT_STRUCTURE";

export type TaskBindingParse =
  | { ok: true; kind: typeof CANONICAL_RESOURCES_KIND; selections: ResourceSelection[] }
  | { ok: true; kind: typeof NCK_POINTS_KIND; points: NckPointSelection[] }
  | { ok: false; code: BindingParseCode; message: string };

/**
 * 解析一个任务的 Binding。
 *
 * NCK：config 必须恰为 `{points: PointSelection[]}`（Core 的 `PointBinding`
 * 是 deny_unknown_fields，多一个键即整任务被拒）；每个点位必须恰为
 * `{kind, point_key, parameters}` 三字段。
 * 其它 canonical：config 必须含 `selections` 数组（旧契约）。
 *
 * 未知 kind 不猜测、不当成 canonical：返回 UNSUPPORTED，由调用方决定是保留
 * 还是拒绝；本模块的提交模型会把它当作无法证明唯一性的结构而拒绝覆盖。
 */
export function parseTaskBinding(
  task: AcquisitionTaskShape | null | undefined,
): TaskBindingParse {
  if (!task || typeof task !== "object" || !task.binding || typeof task.binding !== "object") {
    return invalidStructure("任务缺少 binding");
  }
  const kind = task.binding.kind;
  if (kind === NCK_POINTS_KIND) {
    return parseNckPointsConfig(task.binding.config);
  }
  if (kind === CANONICAL_RESOURCES_KIND) {
    const config = task.binding.config;
    if (!isPlainObject(config)) {
      return invalidStructure(`任务 ${task.id} 的 canonical config 不是对象`);
    }
    const selections = (config as { selections?: unknown }).selections;
    if (!Array.isArray(selections)) {
      return invalidStructure(`任务 ${task.id} 的 canonical config 缺少 selections 数组`);
    }
    return {
      ok: true,
      kind: CANONICAL_RESOURCES_KIND,
      selections: selections as ResourceSelection[],
    };
  }
  return {
    ok: false,
    code: "UNSUPPORTED_BINDING_KIND",
    message: `不支持的 Binding Kind：${String(kind)}（Core 仅接受 ${CANONICAL_RESOURCES_KIND} / ${NCK_POINTS_KIND}）`,
  };
}

function parseNckPointsConfig(config: unknown): TaskBindingParse {
  if (!isPlainObject(config)) {
    return invalidStructure("nck.points.v1 的 config 不是对象");
  }
  const keys = Object.keys(config);
  if (keys.length !== 1 || keys[0] !== "points") {
    return invalidStructure(
      `nck.points.v1 的 config 必须恰为 {points:[...]}（实际键：${keys.join(",") || "空"}）`,
    );
  }
  const raw = (config as { points?: unknown }).points;
  if (!Array.isArray(raw)) {
    return invalidStructure("nck.points.v1 的 config.points 必须是数组");
  }
  const points: NckPointSelection[] = [];
  for (let i = 0; i < raw.length; i += 1) {
    const normalized = normalizePoint(raw[i]);
    if (!normalized.ok) {
      return {
        ok: false,
        code: "INVALID_POINT_STRUCTURE",
        message: `points[${i}] 结构非法：${normalized.message}`,
      };
    }
    points.push(normalized.point);
  }
  return { ok: true, kind: NCK_POINTS_KIND, points };
}

type NormalizedPoint =
  | { ok: true; point: NckPointSelection }
  | { ok: false; message: string };

/** 校验点位信封恰为 {kind, point_key, parameters} 三字段。 */
function normalizePoint(raw: unknown): NormalizedPoint {
  if (!isPlainObject(raw)) return { ok: false, message: "点位必须是 JSON 对象" };
  const keys = Object.keys(raw);
  const extra = keys.filter((k) => k !== "kind" && k !== "point_key" && k !== "parameters");
  if (extra.length) return { ok: false, message: `不允许字段 ${extra.join(",")}` };
  const kind = (raw as { kind?: unknown }).kind;
  if (typeof kind !== "string" || kind.trim() === "") {
    return { ok: false, message: "kind 必须是非空字符串" };
  }
  const pointKey = (raw as { point_key?: unknown }).point_key;
  if (typeof pointKey !== "string" || pointKey.trim() === "") {
    return { ok: false, message: "point_key 必须是非空字符串" };
  }
  const parameters = (raw as { parameters?: unknown }).parameters;
  if (!isPlainObject(parameters)) {
    return { ok: false, message: "parameters 必须是 JSON 对象" };
  }
  return {
    ok: true,
    point: { kind, point_key: pointKey, parameters: parameters as Record<string, unknown> },
  };
}

/** 读取 NCK 点位；非 NCK / 结构非法返回空数组（读取不抛异常）。 */
export function nckPointsOf(task: AcquisitionTaskShape | null | undefined): NckPointSelection[] {
  const parsed = parseTaskBinding(task);
  return parsed.ok && parsed.kind === NCK_POINTS_KIND ? parsed.points : [];
}

// ---------------------------------------------------------------------------
// 2. 可编辑任务选择（歧义拒绝）
// ---------------------------------------------------------------------------

export type EditableSelectionCode =
  | "MULTIPLE_EDITABLE_TASKS"
  | "INVALID_BINDING_STRUCTURE"
  | "UNSUPPORTED_BINDING_KIND";

export type EditableTaskSelection =
  | { ok: true; editable: AcquisitionTaskShape | null; preserved: AcquisitionTaskShape[] }
  | { ok: false; code: EditableSelectionCode; message: string };

/**
 * 按 Endpoint Driver 找到**唯一**可编辑任务。
 *
 * - 候选 = 与期望 kind 相同的任务；0 个 ⇒ 新建；1 个 ⇒ 编辑；
 * - ≥2 个 ⇒ MULTIPLE_EDITABLE_TASKS（**不**像旧 `splitAcquisitionTasks` 那样
 *   取第一个：在 NCK 上猜错任务会覆盖用户既有采集配置）；
 * - 候选自身结构非法 ⇒ INVALID_BINDING_STRUCTURE；
 * - 其它 kind 的任务全部原样保留（不误伤跨 kind 共存）。
 */
export function selectEditableTask(args: {
  tasks: AcquisitionTaskShape[];
  driverId: string | null | undefined;
}): EditableTaskSelection {
  const expectedKind = bindingKindForDriver(args.driverId);
  const candidates = args.tasks.filter((task) => task?.binding?.kind === expectedKind);
  if (candidates.length > 1) {
    return {
      ok: false,
      code: "MULTIPLE_EDITABLE_TASKS",
      message: `存在 ${candidates.length} 个可编辑的 ${expectedKind} 任务（${candidates
        .map((t) => t.id)
        .join("、")}）：拒绝覆盖，请先清理歧义`,
    };
  }
  const editable = candidates[0] ?? null;
  if (editable) {
    const parsed = parseTaskBinding(editable);
    if (!parsed.ok) {
      return {
        ok: false,
        code: parsed.code === "UNSUPPORTED_BINDING_KIND" ? "UNSUPPORTED_BINDING_KIND" : "INVALID_BINDING_STRUCTURE",
        message: `可编辑任务 ${editable.id} 的 Binding 结构非法：${parsed.message}`,
      };
    }
  }
  return {
    ok: true,
    editable,
    preserved: args.tasks.filter((task) => task !== editable),
  };
}

// ---------------------------------------------------------------------------
// 3. 提交前校验（含 scope 准入 + point_key 唯一性）
// ---------------------------------------------------------------------------

export type NckTaskRejectionCode =
  | "SNAPSHOT_NOT_READY"
  | "WRONG_DRIVER"
  | "EMPTY_POINTS"
  | "MULTIPLE_EDITABLE_TASKS"
  | "INVALID_BINDING_STRUCTURE"
  | "UNSUPPORTED_BINDING_KIND"
  | "POINT_KEY_CONFLICT"
  | "DEFINITION_MISSING"
  | "SCOPE_NOT_COLLECTABLE"
  | "SCOPE_IDENTITY"
  | "SCOPE_MISSING"
  | "SCOPE_UNKNOWN"
  | "INVALID_POINT_STRUCTURE"
  | "MISSING_POINT_KIND"
  | "MISSING_POINT_KEY"
  | "INVALID_PARAMETERS"
  | "INVALID_SELECTION"
  | "DRIVER_BINDING_KIND_MISMATCH";

export interface NckTaskSubmissionInput {
  /** 服务端任务快照（已加载成功的那一份）。 */
  tasks: AcquisitionTaskShape[];
  /** Endpoint 的 Driver id（来自 listEndpoints / Endpoint 详情）。 */
  driverId: string;
  /** 本次要提交的 NCK 点位（语义 + 高级原生）。 */
  points: NckPointSelection[];
  /**
   * 定义目录（按 variable_id 索引）。语义点位必须能查到定义，否则无法确认
   * scope ⇒ fail-closed。`nck_native` 点位不走 scope 准入（高级原生模式）。
   */
  definitions?: Readonly<Record<string, NckVariableDefinitionLike>> | null;
  /** 快照状态与归属（A37：非 ready / 串 Endpoint 一律拒绝）。 */
  snapshotState: TaskSnapshotState;
  loadedEndpointId: string | null;
  currentEndpointId: string;
  /** 编辑器周期（poll schedule 使用；非 poll schedule 原样保留）。 */
  intervalMs: number;
}

export type NckTaskSubmission =
  | {
      ok: true;
      editable: AcquisitionTaskShape | null;
      preserved: AcquisitionTaskShape[];
      task: AcquisitionTaskShape;
      points: NckPointSelection[];
    }
  | { ok: false; code: NckTaskRejectionCode; message: string };

/**
 * 提交前校验：只有全部通过才产出可 PUT 的任务。
 *
 * 顺序刻意把「快照门」放在最前：快照未就绪时服务端任务集未知，此时任何
 * 唯一性/歧义判断都建立在不可信输入上，必须直接拒绝（A37）。
 */
export function validateNckTaskSubmission(
  input: NckTaskSubmissionInput,
): NckTaskSubmission {
  if (
    !isTaskSnapshotReady(input.snapshotState, input.loadedEndpointId, input.currentEndpointId)
  ) {
    return {
      ok: false,
      code: "SNAPSHOT_NOT_READY",
      message: "任务快照未就绪（加载中/失败/归属不符）：禁止保存，避免用空快照覆盖已有任务",
    };
  }
  if (!isNckDriver(input.driverId)) {
    return {
      ok: false,
      code: "WRONG_DRIVER",
      message: `本模型只处理 NCK Endpoint（当前 driver=${input.driverId || "未知"}）`,
    };
  }
  if (!Array.isArray(input.points) || input.points.length === 0) {
    return { ok: false, code: "EMPTY_POINTS", message: "点位列不能为空（空任务会被驱动拒绝）" };
  }

  // 3.1 逐点做 scope 准入 + 信封归一（P0-1 的提交模型在这里被执行）。
  const normalized: NckPointSelection[] = [];
  for (let i = 0; i < input.points.length; i += 1) {
    const checked = validateSubmittedPoint(input.points[i], input.definitions);
    if (!checked.ok) {
      return {
        ok: false,
        code: checked.code,
        message: `points[${i}]：${checked.message}`,
      };
    }
    normalized.push(checked.point);
  }

  // 3.2 可编辑候选歧义 / 结构非法。
  const selection = selectEditableTask({ tasks: input.tasks, driverId: input.driverId });
  if (!selection.ok) {
    return { ok: false, code: selection.code, message: selection.message };
  }
  const { editable, preserved } = selection;

  // 3.2.1 既有任务（将被逐字保留）的结构也必须可解析：否则无法证明
  // point_key 唯一性，且 Core 很可能整体拒绝。未知 kind 不在此列——它可能是
  // 未来驱动，本层不猜；Core 的 kind 白名单是最终裁决（跨 kind 共存不误伤）。
  for (const task of preserved) {
    const parsed = parseTaskBinding(task);
    if (!parsed.ok && parsed.code !== "UNSUPPORTED_BINDING_KIND") {
      return {
        ok: false,
        code: "INVALID_BINDING_STRUCTURE",
        message: `既有任务 ${task.id} 的 Binding 结构非法：${parsed.message}`,
      };
    }
  }

  // 3.3 Endpoint-wide 唯一性：候选内部 + 与其它任务（含跨 kind）冲突。
  const duplicateInCandidate = duplicates(normalized.map((p) => p.point_key));
  if (duplicateInCandidate.length) {
    return {
      ok: false,
      code: "POINT_KEY_CONFLICT",
      message: `本次提交内 point_key 重复：${duplicateInCandidate.join("、")}`,
    };
  }
  const occupied = collectEndpointPointKeys(preserved);
  const duplicateInPreserved = duplicates(occupied.map((o) => o.point_key));
  if (duplicateInPreserved.length) {
    return {
      ok: false,
      code: "POINT_KEY_CONFLICT",
      message: `既有任务之间已存在重复 point_key：${duplicateInPreserved.join("、")}（Core 会整体拒绝）`,
    };
  }
  const occupiedKeys = new Set(occupied.map((o) => o.point_key));
  const conflicts = normalized.filter((p) => occupiedKeys.has(p.point_key)).map((p) => p.point_key);
  if (conflicts.length) {
    return {
      ok: false,
      code: "POINT_KEY_CONFLICT",
      message: `point_key 已被 Endpoint 内其它任务占用：${conflicts.join("、")}`,
    };
  }

  // 3.4 装配（kind 由 Driver 决定；这里不出现任何硬编码字面量）。
  const assembled = assembleBinding({
    driverId: input.driverId,
    content: { type: "nck-points", points: normalized },
  });
  if (!assembled.ok) {
    return { ok: false, code: "DRIVER_BINDING_KIND_MISMATCH", message: assembled.message };
  }

  const task = buildTask({
    editable,
    tasks: input.tasks,
    binding: assembled.binding,
    intervalMs: input.intervalMs,
  });
  return { ok: true, editable, preserved, task, points: normalized };
}

type CheckedPoint =
  | { ok: true; point: NckPointSelection }
  | { ok: false; code: NckTaskRejectionCode; message: string };

/**
 * 单点提交校验：语义点位重新查 scope（不信 UI），原生点位只校验信封。
 *
 * 为什么未知 Point Kind 也拒绝：Core 不建 Point Kind 白名单，但前端无法判断
 * 一个未知 kind 是否需要 scope 准入；无法确认即拒绝（fail-closed）。
 */
function validateSubmittedPoint(
  point: NckPointSelection,
  definitions: Readonly<Record<string, NckVariableDefinitionLike>> | null | undefined,
): CheckedPoint {
  const normalized = normalizePoint(point);
  if (!normalized.ok) {
    return { ok: false, code: "INVALID_POINT_STRUCTURE", message: normalized.message };
  }
  const candidate = normalized.point;
  if (candidate.kind === "nck_native") {
    // 高级原生模式：身份由固定规则/用户显式给出，不适用目录 scope。
    return { ok: true, point: candidate };
  }
  if (candidate.kind !== "nck_variable") {
    return {
      ok: false,
      code: "INVALID_POINT_STRUCTURE",
      message: `未知 Point Kind "${candidate.kind}"：无法确认是否需要 scope 准入`,
    };
  }
  const variableId = candidate.parameters.variable_id;
  if (typeof variableId !== "string" || variableId.trim() === "") {
    return {
      ok: false,
      code: "DEFINITION_MISSING",
      message: "语义点位缺少 parameters.variable_id，无法查定义目录",
    };
  }
  const definition = definitions?.[variableId];
  if (!definition) {
    return {
      ok: false,
      code: "DEFINITION_MISSING",
      message: `定义目录中找不到变量 ${variableId}：无法确认 scope，禁止提交`,
    };
  }
  const result = validateNckPointSubmission({ definition, selection: candidate });
  if (!result.ok) {
    return {
      ok: false,
      code: result.code,
      message: result.message,
    };
  }
  return { ok: true, point: result.selection };
}

/** 任务 ID / Schedule 保留规则（与既有 canonical 编辑器同构）。 */
function buildTask(args: {
  editable: AcquisitionTaskShape | null;
  tasks: AcquisitionTaskShape[];
  binding: { kind: string; config: Record<string, unknown> };
  intervalMs: number;
}): AcquisitionTaskShape {
  const taken = new Set(args.tasks.map((task) => task.id));
  let id = args.editable?.id ?? "t1";
  if (!args.editable) {
    let n = 2;
    while (taken.has(id)) {
      id = `t1-${n}`;
      n += 1;
    }
  }
  const base: TaskScheduleShape = args.editable?.schedule ?? { mode: "poll" };
  // poll 任务写编辑器周期；subscribe 等外来 schedule 逐字保留（不得被悄悄改写）。
  const schedule =
    base.mode === "poll" ? { ...base, mode: "poll", interval_ms: args.intervalMs } : base;
  return { id, schedule, binding: args.binding };
}

// ---------------------------------------------------------------------------
// 4. 合并回写
// ---------------------------------------------------------------------------

export type NckTaskMergeResult =
  | {
      ok: true;
      tasks: AcquisitionTaskShape[];
      taskId: string;
      points: NckPointSelection[];
      preserved: AcquisitionTaskShape[];
    }
  | { ok: false; code: NckTaskRejectionCode; message: string };

/**
 * 更新已有 NCK 任务（或新建一个）：
 * - 候选唯一时**原位替换**该任务（ID / 非 poll Schedule / 位置不变）；
 * - 其它任务（含其它 kind）逐字保留、顺序不变；
 * - 任何校验失败都返回错误，**不返回部分结果**（调用方不得拿旧 tasks 凑合）。
 */
export function mergeNckAcquisitionTasks(
  input: NckTaskSubmissionInput,
): NckTaskMergeResult {
  const submission = validateNckTaskSubmission(input);
  if (!submission.ok) return submission;
  const { editable, task, points, preserved } = submission;
  const tasks = editable
    ? input.tasks.map((existing) => (existing === editable ? task : existing))
    : [...input.tasks, task];
  return { ok: true, tasks, taskId: task.id, points, preserved };
}

// ---------------------------------------------------------------------------
// 5. point_key 收集（唯一性校验用）
// ---------------------------------------------------------------------------

export interface PointKeyOwner {
  point_key: string;
  task_id: string;
  /** points 数组下标或 selections.outputs 下标，便于错误定位。 */
  index: number;
  source: typeof NCK_POINTS_KIND | typeof CANONICAL_RESOURCES_KIND;
}

/**
 * 枚举 Endpoint 内所有已知 point_key（两种 kind 都读，结构非法/未知 kind
 * 直接跳过——它们会由 `parseTaskBinding` 的严格校验在别处拒绝）。
 *
 * 旧 canonical 任务的 point_key 在 `selections[].outputs[].point_key`；
 * NCK 任务在 `points[].point_key`。两者都参与唯一性，因为 Core 的
 * `ensure_endpoint_point_keys_unique` 也是跨 Task、跨 kind 的统一命名空间。
 */
export function collectEndpointPointKeys(
  tasks: AcquisitionTaskShape[],
): PointKeyOwner[] {
  const out: PointKeyOwner[] = [];
  for (const task of tasks) {
    const parsed = parseTaskBinding(task);
    if (!parsed.ok) continue;
    if (parsed.kind === NCK_POINTS_KIND) {
      parsed.points.forEach((point, index) => {
        out.push({
          point_key: point.point_key,
          task_id: task.id,
          index,
          source: NCK_POINTS_KIND,
        });
      });
      continue;
    }
    parsed.selections.forEach((selection, si) => {
      const outputs = (selection as { outputs?: unknown }).outputs;
      if (!Array.isArray(outputs)) return;
      outputs.forEach((output, oi) => {
        const key = (output as { point_key?: unknown })?.point_key;
        if (typeof key === "string" && key.trim() !== "") {
          out.push({
            point_key: key,
            task_id: task.id,
            index: si * 1000 + oi,
            source: CANONICAL_RESOURCES_KIND,
          });
        }
      });
    });
  }
  return out;
}

function duplicates(values: string[]): string[] {
  const seen = new Set<string>();
  const dup = new Set<string>();
  for (const value of values) {
    if (seen.has(value)) dup.add(value);
    else seen.add(value);
  }
  return [...dup];
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function invalidStructure(message: string): TaskBindingParse {
  return { ok: false, code: "INVALID_BINDING_STRUCTURE", message };
}
