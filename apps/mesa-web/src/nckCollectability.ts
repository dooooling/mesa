// PR-C P0-1：`scope` 准入（可采集性）纯模型。
//
// 背景（I-2 已落地后核实）：定义目录如实返回 42 条定义（36 条 collect +
// 6 条 identity），但 Driver 的 Browse / `canonical_identity` / 权威 Preview
// **都不校验 `scope`**，I-2 也把「是否可采集」明确划给前端判定。因此：
// - 不能因为「Browse 树里出现了它」或「Preview 能算出合法 Key」就放行；
// - 也不能因为前端 UI 里没展示它，就认为提交路径安全——绕过 UI 直接构造
//   提交（脚本、旧组件、未来的新入口）必须同样被拒。
//
// 本模块只做纯判定，不 import React / AntD / 任何请求层，便于被 UI、提交模型
// 和单测共同调用。两处执行点（缺一不可）：
//   ① UI 层：collectableDefinitions（列表过滤）+ evaluateNckSelectionAdd
//      （选择/加入时拒绝）；UI 接线在后续批次，本批只交付可单测的纯函数。
//   ② 提交模型层：validateNckPointSubmission（不信任 UI 传入的 collectable
//      布尔标志，只重新读原始 definition.scope）。
//
// TODO(PR-C 后续批次)：把上面两个 UI 纯函数接到 NckPointFields /
// ResourcePickerAntd 的“加入”出口。本批受“只做纯模型、不动 UI 组件”约束，
// 因此这里只提供执行点，不接线；一旦接线，UI 与提交模型必须同时生效。
//
// 判据唯一且严格：`scope === "collect"` 才为真。`identity`、缺失、null、
// 大小写/拼写变体、未来新增值一律 false（fail-closed）。**禁止**写成
// `scope !== "identity"`——那会把未知值默认放行，正是 A40 要禁止的反写。

/** 目录条目里与准入相关的最小形态（I-2 `VariableDefinition` 的子集）。 */
export interface NckVariableDefinitionLike {
  variable_id?: string;
  /** 原始 scope，**不得**在读取时归一化/补默认值；缺失即缺失。 */
  scope?: unknown;
}

/** 准入失败码：区分「未提供」「明确 identity」「未知值」，便于 UI 给准确文案。 */
export type NckScopeCode = "SCOPE_MISSING" | "SCOPE_IDENTITY" | "SCOPE_UNKNOWN";

export type NckScopeAdmission =
  | { collectable: true; scope: "collect" }
  | { collectable: false; code: NckScopeCode; scope: unknown; message: string };

/**
 * 唯一准入判据：只认白名单字面量 `"collect"`。
 *
 * 为什么用正向白名单而不是排除 `"identity"`：I-2 的 scope 是 Driver 定义数据，
 * 未来可能新增值（例如 `"probe"`）。前端对未知语义只有一个安全动作——拒绝；
 * 反写会让新值静默变成「可采集」，而 Core/Driver 不会替我们拦（它们不看 scope）。
 */
export function isCollectableScope(scope: unknown): scope is "collect" {
  return scope === "collect";
}

/**
 * 判定一个原始 scope 值。`undefined` / `null` / 键缺失都归为 MISSING；
 * `"identity"` 归为 IDENTITY；其余任何值（含大小写变体、数字、对象）归为
 * UNKNOWN。三者都不可采集，绝不返回「默认放行」。
 */
export function admitNckScope(scope: unknown): NckScopeAdmission {
  if (scope === "collect") return { collectable: true, scope: "collect" };
  if (scope === "identity") {
    return {
      collectable: false,
      code: "SCOPE_IDENTITY",
      scope,
      message: "该变量只服务身份探针（scope=identity），不得作为采集点位",
    };
  }
  if (scope === undefined || scope === null) {
    return {
      collectable: false,
      code: "SCOPE_MISSING",
      scope,
      // 与设计 §4.1 / A40 的文案口径一致：缺失不是「暂时可用」，而是不可确认。
      message: "适用范围未提供（需定义目录接口）：无法确认该变量是否可采集",
    };
  }
  return {
    collectable: false,
    code: "SCOPE_UNKNOWN",
    scope,
    message: `适用范围未知（scope=${describeScope(scope)}）：按不可采集处理`,
  };
}

/** 对目录条目做准入判定；条目本身缺失时同样 fail-closed（不当作可采集）。 */
export function admitNckDefinition(
  definition: NckVariableDefinitionLike | null | undefined,
): NckScopeAdmission {
  if (!definition || typeof definition !== "object") {
    return {
      collectable: false,
      code: "SCOPE_MISSING",
      scope: undefined,
      message: "定义缺失：无法确认适用范围，按不可采集处理",
    };
  }
  // 只读原始 scope，绝不把 `collectable: true` 之类的派生标志写回/采信。
  return admitNckScope(definition.scope);
}

/**
 * UI 执行点①：从目录中筛出**可采集列表**。
 *
 * 返回新数组（不修改入参）；identity / 缺失 / 未知 scope 的条目不会出现。
 * 42 条目录 ⇒ 36 条（真实数据由 Driver 侧测试锁定；前端测试使用合成条目）。
 */
export function collectableDefinitions<T extends NckVariableDefinitionLike>(
  definitions: readonly T[] | null | undefined,
): T[] {
  return (definitions ?? []).filter((definition) => admitNckDefinition(definition).collectable);
}

export type NckSelectionAddGate =
  | { ok: true }
  | { ok: false; code: NckScopeCode; message: string };

/**
 * UI 执行点②：选择/加入时再查一次 scope。
 *
 * 为什么 UI 列表过滤之后还要查：过滤只保证「正常渲染路径」不出现 identity
 * 条目；深链、旧缓存、测试桩或被改写的组件状态都可能把 identity 定义塞进
 * 选中态。准入判定必须基于**定义本身**，而不是「当前列表里有没有它」。
 */
export function evaluateNckSelectionAdd(
  definition: NckVariableDefinitionLike | null | undefined,
): NckSelectionAddGate {
  const admission = admitNckDefinition(definition);
  return admission.collectable
    ? { ok: true }
    : { ok: false, code: admission.code, message: admission.message };
}

// ---------------------------------------------------------------------------
// 提交模型层（P0-1 的核心）
// ---------------------------------------------------------------------------

/** 与 Core `PointSelection` 同形的点位信封（恰 3 字段）。 */
export interface NckSelectionDraft {
  kind: string;
  point_key: string;
  parameters: Record<string, unknown>;
}

export type NckPointSubmitCode =
  | NckScopeCode
  | "SCOPE_NOT_COLLECTABLE"
  | "MISSING_POINT_KIND"
  | "MISSING_POINT_KEY"
  | "INVALID_PARAMETERS"
  | "INVALID_SELECTION";

export type NckPointSubmitResult =
  | { ok: true; selection: NckSelectionDraft }
  | { ok: false; code: NckPointSubmitCode; message: string };

export interface NckPointSubmissionInput {
  /** 权威定义（来自 I-2 目录）；只读其原始 scope。 */
  definition: NckVariableDefinitionLike | null | undefined;
  /** 待提交选择；null/undefined 表示身份尚未确认，必须拒绝。 */
  selection: Partial<NckSelectionDraft> | null | undefined;
  /**
   * 服务端 Preview 结果。**故意不参与任何判定**。
   *
   * 理由（A43/第 6 条断言）：`compiler.rs` 的编译链没有 scope 分支，对
   * `scope=identity` 的变量同样会返回合法 Key。若让 preview 结果影响准入，
   * 就等于用「Key 合法」覆盖产品语义。此处保留该字段只为在类型上明确它存在、
   * 且被显式忽略——不要删掉后让调用方以为「没传就不影响」。
   */
  preview?: { point_key?: unknown; scope?: unknown } | null;
}

/**
 * 提交前校验（可被单测直接调用，不依赖任何 UI 状态）。
 *
 * 返回结构化结果而**不抛异常**：调用方需要把 code 映射成 UI 文案；抛异常会
 * 让「拒绝」变成「崩溃」，两者在验收上不是一回事。
 *
 * 校验顺序：先 scope 准入，再信封形状。这样即使调用方构造了一个「Key 合法、
 * 参数完整」的 identity 点位，也先在准入处被拒，不会出现「格式过了所以能提交」
 * 的错觉。
 */
export function validateNckPointSubmission(
  input: NckPointSubmissionInput,
): NckPointSubmitResult {
  // 注意：这里只读 definition.scope；input.preview 被有意忽略（见类型注释）。
  const admission = admitNckDefinition(input.definition);
  if (!admission.collectable) {
    const code: NckPointSubmitCode =
      admission.code === "SCOPE_IDENTITY" ? "SCOPE_NOT_COLLECTABLE" : admission.code;
    return { ok: false, code, message: admission.message };
  }

  const selection = input.selection;
  if (!selection || typeof selection !== "object") {
    return { ok: false, code: "INVALID_SELECTION", message: "待提交点位缺失（身份尚未确认）" };
  }
  const kind = typeof selection.kind === "string" ? selection.kind : "";
  if (kind.trim() === "") {
    return { ok: false, code: "MISSING_POINT_KIND", message: "点位缺少 Point Kind" };
  }
  const pointKey = typeof selection.point_key === "string" ? selection.point_key : "";
  if (pointKey.trim() === "") {
    return { ok: false, code: "MISSING_POINT_KEY", message: "身份未确认：point_key 为空，禁止提交" };
  }
  const parameters = selection.parameters;
  if (!parameters || typeof parameters !== "object" || Array.isArray(parameters)) {
    return { ok: false, code: "INVALID_PARAMETERS", message: "parameters 必须是 JSON 对象" };
  }

  // 显式重建恰 3 字段信封：调用方多传的字段（如 identity 块）不得进入持久化
  // 载荷——Core 的 `PointSelection` 是 deny_unknown_fields，多一个键即整体拒绝。
  return {
    ok: true,
    selection: {
      kind,
      point_key: pointKey,
      parameters: parameters as Record<string, unknown>,
    },
  };
}

/** scope 的可读描述（只用于文案，不参与判定）。 */
function describeScope(scope: unknown): string {
  if (typeof scope === "string") return `"${scope}"`;
  try {
    return JSON.stringify(scope) ?? String(scope);
  } catch {
    return String(scope);
  }
}
