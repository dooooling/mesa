// PR-C 第二批 B：NCK 语义点位的**纯模型**（无 React / AntD / 请求层）。
//
// 本文件只做三件事，且每件都必须可被单测直接钉死：
// 1. **动态表单模型**：Target 控件由定义的 `targets` / `target_required` 生成，
//    Arguments 由 `arguments[]` 生成。前端**不硬编码** NCK 原生地址规则，
//    也不由 `identity.keys` 反推合法集合（边界文档 §1.4.3 / A42）。
// 2. **输入校验**：非法 Target / Arguments 在预览之前就被拒绝，并给出具体字段。
// 3. **权威预览状态判定**：请求序号 + 输入指纹双判定（迟到响应整份丢弃）、
//    family 交叉校验、预览响应与定义坐标的一致性校验。
//
// 为什么把这几件事从组件里抽出来：竞态与 family 判定属于「语义正确性」，
// 放在 JSX 里只能靠组件测试间接覆盖；抽成纯函数后可以逐条断言，且不会被
// UI 状态机的偶然行为掩盖。

import {
  admitNckDefinition,
  type NckScopeCode,
  type NckVariableDefinitionLike,
} from "./nckCollectability";
import type { NckIdentityPreviewResponse } from "./nckCatalog";

/** 目录条目（宽松形态）。字段来源是 I-2 的 14 字段投影；缺失时一律 fail-closed。 */
export interface NckDefinitionLike extends NckVariableDefinitionLike {
  variable_id?: string;
  revision?: number;
  family?: string;
  label?: string | { zh?: string; en?: string };
  /**
   * 导航分类（`definition_api.rs:571-576` 的投影期附加字段）。
   *
   * 取 `unknown` 而不是联合类型：这是**后端数据**，前端只允许逐字采用或落"待分类"，
   * 不做联合类型归一化（否则非法取值会被 TypeScript 掩盖成"合法分类"）。
   */
  category?: unknown;
  /** 分类依据（本轮恒为 `"navigation_rule"`；同样按 unknown 读取，展示原文）。 */
  category_basis?: unknown;
  /**
   * 四块 sidecar 元数据（`unit` / `manual_location` / `index_rule` / `availability`）。
   *
   * 一律 `unknown`：形状与取值都是**后端数据**，容错与判据只在 `definitionMeta.ts`
   * 里写一次。`availability` 缺失/非法 ⇒ 门禁 fail-closed（不是默认放行）。
   */
  unit?: unknown;
  manual_location?: unknown;
  index_rule?: unknown;
  availability?: unknown;
  semantics_status?: unknown;
  source?: unknown;
  targets?: unknown;
  target_required?: unknown;
  arguments?: unknown;
  identity_template?: string;
  identity_keys?: unknown;
  golden_ref?: unknown;
  digest?: string;
}

/** 展示名：I-2 是 `{zh,en}`；测试/注入允许直接字符串。 */
export function nckDefinitionLabel(definition: NckDefinitionLike | null | undefined): string {
  if (!definition) return "";
  const label = definition.label;
  if (typeof label === "string") return label;
  if (label && typeof label === "object") return label.zh || label.en || "";
  return "";
}

// ---------------------------------------------------------------------------
// 1. 动态表单模型（Target / Arguments）
// ---------------------------------------------------------------------------

export interface NckTargetField {
  /** 驱动声明的 target 变体名（`channel` / `axis` / `drive_object` …）。 */
  kind: string;
  required: boolean;
  /** 该 Target 变体是否需要额外的 `entry` 子字段（形状事实，不是地址规则）。 */
  requiresEntry: boolean;
  /**
   * 目录是否**权威声明**了 `entry` 的完整合法候选集。
   *
   * 当前 I-2 目录的 `targets` / `target_required` 只给 Target 变体名，
   * **没有** `entry` 合法域字段（已核对 `definition_api.rs` 的 14 字段投影与
   * `r1.json` 的 `targets` 元数据）⇒ 恒为 `false`。前端据此拒绝把示例值
   * 当成闭集（见 `entryExamples`）。
   */
  entryDomainDeclared: boolean;
  /**
   * `drive_object.entry` 的**示例值**，只来自定义数据 `golden_ref[].entry`。
   *
   * Golden Reference 是验证样例，不是合法值全集；目录未声明候选集时，这些值
   * 只能作为输入提示（datalist），**不得**用于「不在列表即拒绝」的闭集校验。
   * 真正的合法性由服务端权威 Preview 裁决。
   */
  entryExamples: string[];
}

export interface NckArgumentField {
  key: string;
  label: string;
  valueType: string;
  required: boolean;
  min?: number;
  max?: number;
  enumOptions: string[];
  /** `enum` 但没有候选集（D14 的 fail-closed 分支）。 */
  optionsError?: string;
}

export interface NckFormModelOk {
  ok: true;
  targetFields: NckTargetField[];
  argumentFields: NckArgumentField[];
}

export interface NckFormModelFail {
  ok: false;
  code: NckFormModelCode;
  message: string;
  targetFields: NckTargetField[];
  argumentFields: NckArgumentField[];
}

export type NckFormModelCode =
  | "DEFINITION_MISSING"
  | "TARGETS_METADATA_MISSING"
  | "TARGET_REQUIRED_INVALID"
  | "ARGUMENTS_METADATA_INVALID";

export type NckFormModel = NckFormModelOk | NckFormModelFail;

/**
 * `drive_object` 的 envelope 形状（`{number, entry}`）由驱动的 Target 类型决定。
 *
 * 这里只识别「哪个 target 变体需要 entry 子字段」这一**形状**事实；
 * entry 的**值**没有任何前端权威来源（目录未声明候选集），必须由用户输入并
 * 最终交给服务端 Preview 校验。
 */
function needsDriveEntry(kind: string): boolean {
  return kind === "drive_object";
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asStringArray(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  if (!value.every((v) => typeof v === "string" && v.trim() !== "")) return null;
  return value as string[];
}

/**
 * 从定义生成表单模型。
 *
 * 元数据完整性判据直接取边界文档 §1.4.4 第 1 行：
 * `targets` + `target_required` + `arguments[]` + `scope` 都在才可能允许提交；
 * 本函数负责前三项的形状，`scope` 由 `nckCollectability` 单独判定。
 * 只有 `identity.keys` 时，这里**不会**推断任何 Target 集合（A42）。
 */
export function buildNckFormModel(
  definition: NckDefinitionLike | null | undefined,
): NckFormModel {
  if (!definition || typeof definition !== "object") {
    return {
      ok: false,
      code: "DEFINITION_MISSING",
      message: "定义缺失：无法生成 Target/Arguments 表单",
      targetFields: [],
      argumentFields: [],
    };
  }
  const targets = asStringArray(definition.targets);
  const required = asStringArray(definition.target_required);
  if (!targets || !targets.length) {
    return {
      ok: false,
      code: "TARGETS_METADATA_MISSING",
      message:
        "定义缺少 targets（Target 合法集合）：不得由 identity.keys 反推，保持不可提交（A42）",
      targetFields: [],
      argumentFields: [],
    };
  }
  if (!required || required.some((kind) => !targets.includes(kind))) {
    return {
      ok: false,
      code: "TARGET_REQUIRED_INVALID",
      message: "定义的 target_required 缺失或不在 targets 内：无法确定必填 Target",
      targetFields: [],
      argumentFields: [],
    };
  }

  // drive_object 的 entry 候选：驱动投影的 golden_ref[].entry，去重保序。
  const goldenEntries: string[] = [];
  if (Array.isArray(definition.golden_ref)) {
    for (const ref of definition.golden_ref) {
      const entry = isPlainObject(ref) ? ref.entry : undefined;
      if (typeof entry === "string" && entry !== "" && !goldenEntries.includes(entry)) {
        goldenEntries.push(entry);
      }
    }
  }
  const targetFields: NckTargetField[] = targets.map((kind) => ({
    kind,
    required: required.includes(kind),
    requiresEntry: needsDriveEntry(kind),
    // 目录（14 字段投影）没有 entry 合法域字段 ⇒ 只能诚实地标记为“未声明”。
    // TODO(PR-C 后端补充授权)：等 Definition Catalog 增加权威 entry 候选集后，
    // 这里改为读取该字段；在此之前不得把 golden_ref 示例当成完整可选域。
    entryDomainDeclared: false,
    entryExamples: needsDriveEntry(kind) ? goldenEntries : [],
  }));

  if (definition.arguments !== undefined && !Array.isArray(definition.arguments)) {
    return {
      ok: false,
      code: "ARGUMENTS_METADATA_INVALID",
      message: "定义的 arguments 不是数组：无法生成参数控件",
      targetFields,
      argumentFields: [],
    };
  }
  const argumentFields: NckArgumentField[] = [];
  // 身份键若同时是 argument，即使规格写 required=false，驱动仍会在身份层
  // 报 MISSING_IDENTITY_KEY（definition.rs:1060-1074）——表单按「身份必需」
  // 处理，避免让用户走一条必然被服务端拒绝的路径。
  const identityRequired = new Set<string>();
  if (Array.isArray(definition.identity_keys)) {
    for (const key of definition.identity_keys) {
      if (
        isPlainObject(key) &&
        key.required === true &&
        typeof key.path === "string" &&
        key.path !== ""
      ) {
        identityRequired.add(key.path);
      }
    }
  }
  for (const [index, raw] of (definition.arguments ?? []).entries()) {
    if (!isPlainObject(raw)) {
      return {
        ok: false,
        code: "ARGUMENTS_METADATA_INVALID",
        message: `arguments[${index}] 不是对象：无法生成参数控件`,
        targetFields,
        argumentFields,
      };
    }
    const key = typeof raw.key === "string" ? raw.key : "";
    const valueType = typeof raw.value_type === "string" ? raw.value_type : "";
    if (!key || !valueType) {
      return {
        ok: false,
        code: "ARGUMENTS_METADATA_INVALID",
        message: `arguments[${index}] 缺少 key/value_type：无法生成参数控件`,
        targetFields,
        argumentFields,
      };
    }
    const options = asStringArray(raw.enum_options) ?? [];
    argumentFields.push({
      key,
      label: nckDefinitionLabel({ label: raw.label as NckDefinitionLike["label"] }) || key,
      valueType,
      required: raw.required === true || identityRequired.has(key),
      min: typeof raw.min === "number" ? raw.min : undefined,
      max: typeof raw.max === "number" ? raw.max : undefined,
      enumOptions: options,
      optionsError:
        valueType === "enum" && options.length === 0
          ? `argument \`${key}\` 是 enum 但没有候选集（enum_options）：无法渲染，保持不可提交`
          : undefined,
    });
  }

  return { ok: true, targetFields, argumentFields };
}

/** 表单草稿：全部按用户键入的原始文本保存（未 blur 的值不得丢失）。 */
export interface NckFormDraft {
  /** 简单 Target：`kind -> 文本`；drive_object：`kind` = number 文本，`kind.entry` = entry。 */
  target: Record<string, string>;
  /** `argument key -> 文本`。 */
  arguments: Record<string, string>;
}

export function emptyNckDraft(model: NckFormModel): NckFormDraft {
  const target: Record<string, string> = {};
  for (const field of model.targetFields) {
    target[field.kind] = "";
    if (field.requiresEntry) target[`${field.kind}.entry`] = "";
  }
  const args: Record<string, string> = {};
  for (const field of model.argumentFields) args[field.key] = "";
  return { target, arguments: args };
}

export type NckFormIssueCode =
  | "FORM_MODEL_INVALID"
  | "MISSING_TARGET"
  | "UNSUPPORTED_TARGET"
  | "INVALID_TARGET"
  | "MISSING_TARGET_ENTRY"
  | "MISSING_ARGUMENT"
  | "UNKNOWN_ARGUMENT"
  | "INVALID_ARGUMENT"
  | "ARGUMENT_OPTIONS_MISSING";

export interface NckFormIssue {
  code: NckFormIssueCode;
  field?: string;
  message: string;
}

export interface NckFormValidation {
  ok: boolean;
  issues: NckFormIssue[];
  /** 实际可提交的 Target 对象（`drive_object` 为 `{number, entry}`）。 */
  target: Record<string, unknown>;
  /** 实际可提交的 Arguments 对象。 */
  arguments: Record<string, unknown>;
}

const MAX_JSON_U32 = 4294967295;

function parseIntegerText(raw: string): number | null {
  const text = raw.trim();
  if (!/^-?\d+$/.test(text)) return null;
  const value = Number(text);
  return Number.isSafeInteger(value) ? value : null;
}

/**
 * 校验用户输入并构造 `target` / `arguments`。
 *
 * 判据全部来自定义元数据：
 * - 必填 Target 由 `target_required` 决定；提供了 `targets` 未声明的变体即拒绝；
 * - Integer argument 的 `min`/`max` 由定义给出；
 * - Enum argument 的候选集由 `enum_options` 给出（缺失即拒绝，不猜）。
 *
 * 前端额外只做两件通用判断：JSON 整数形态与 u32 上界（这是 IPC 契约的数值
 * 边界，不是 NCK 地址规则）。实例号的业务范围（例如 0..=255）**故意不**在前端
 * 硬编码：由权威预览拒绝，预览失败即不可提交。
 */
export function validateNckForm(
  definition: NckDefinitionLike | null | undefined,
  model: NckFormModel,
  draft: NckFormDraft,
): NckFormValidation {
  const issues: NckFormIssue[] = [];
  const target: Record<string, unknown> = {};
  const args: Record<string, unknown> = {};
  if (!model.ok) {
    issues.push({ code: "FORM_MODEL_INVALID", message: model.message });
    return { ok: false, issues, target, arguments: args };
  }
  if (!definition) {
    issues.push({ code: "FORM_MODEL_INVALID", message: "定义缺失" });
    return { ok: false, issues, target, arguments: args };
  }

  const allowedTargetKeys = new Set<string>();
  for (const field of model.targetFields) {
    allowedTargetKeys.add(field.kind);
    if (field.requiresEntry) allowedTargetKeys.add(`${field.kind}.entry`);
  }
  for (const key of Object.keys(draft.target)) {
    if (!allowedTargetKeys.has(key)) {
      issues.push({
        code: "UNSUPPORTED_TARGET",
        field: key,
        message: `Target 变体 \`${key}\` 未在该定义的 targets 中声明`,
      });
    }
  }

  for (const field of model.targetFields) {
    const raw = (draft.target[field.kind] ?? "").trim();
    if (raw === "") {
      if (field.required) {
        issues.push({
          code: "MISSING_TARGET",
          field: field.kind,
          message: `缺少必填 Target \`${field.kind}\``,
        });
      }
      continue;
    }
    const number = parseIntegerText(raw);
    if (number === null || number < 0 || number > MAX_JSON_U32) {
      issues.push({
        code: "INVALID_TARGET",
        field: field.kind,
        message: `Target \`${field.kind}\` 必须是 0..=${MAX_JSON_U32} 的整数，实际 "${raw}"`,
      });
      continue;
    }
    if (field.requiresEntry) {
      const entry = (draft.target[`${field.kind}.entry`] ?? "").trim();
      if (entry === "") {
        issues.push({
          code: "MISSING_TARGET_ENTRY",
          field: `${field.kind}.entry`,
          message: `缺少必填 \`${field.kind}.entry\`：入口身份不可由 number 反推`,
        });
        continue;
      }
      // 关键裁决：Golden 示例不是完整合法域。目录没有声明 entry 候选集时，
      // 前端**不得**把「不在 golden_ref 中」当作非法（那会把示例当闭集）。
      // 真正的合法性由服务端 Preview 裁决；预览失败即不可提交。
      target[field.kind] = { number, entry };
    } else {
      target[field.kind] = number;
    }
  }

  const declaredArgs = new Set(model.argumentFields.map((f) => f.key));
  for (const key of Object.keys(draft.arguments)) {
    if (!declaredArgs.has(key)) {
      issues.push({
        code: "UNKNOWN_ARGUMENT",
        field: key,
        message: `argument \`${key}\` 未在该定义的 arguments 中声明（拒绝多余键）`,
      });
    }
  }

  for (const field of model.argumentFields) {
    const raw = (draft.arguments[field.key] ?? "").trim();
    if (raw === "") {
      if (field.required) {
        issues.push({
          code: "MISSING_ARGUMENT",
          field: field.key,
          message: `缺少必填 argument \`${field.key}\``,
        });
      }
      continue;
    }
    if (field.valueType === "integer") {
      const number = parseIntegerText(raw);
      if (number === null) {
        issues.push({
          code: "INVALID_ARGUMENT",
          field: field.key,
          message: `argument \`${field.key}\` 必须是整数，实际 "${raw}"`,
        });
        continue;
      }
      if (field.min !== undefined && number < field.min) {
        issues.push({
          code: "INVALID_ARGUMENT",
          field: field.key,
          message: `argument \`${field.key}\` = ${number} 小于下界 ${field.min}`,
        });
        continue;
      }
      if (field.max !== undefined && number > field.max) {
        issues.push({
          code: "INVALID_ARGUMENT",
          field: field.key,
          message: `argument \`${field.key}\` = ${number} 超过上界 ${field.max}`,
        });
        continue;
      }
      args[field.key] = number;
      continue;
    }
    if (field.valueType === "enum") {
      if (field.optionsError) {
        issues.push({
          code: "ARGUMENT_OPTIONS_MISSING",
          field: field.key,
          message: field.optionsError,
        });
        continue;
      }
      if (!field.enumOptions.includes(raw)) {
        issues.push({
          code: "INVALID_ARGUMENT",
          field: field.key,
          message: `argument \`${field.key}\` 取值 "${raw}" 不在 [${field.enumOptions.join(", ")}] 内`,
        });
        continue;
      }
      args[field.key] = raw;
      continue;
    }
    // 未来新增的 value_type：前端无法校验 ⇒ 未知即拒。
    issues.push({
      code: "INVALID_ARGUMENT",
      field: field.key,
      message: `argument \`${field.key}\` 的 value_type "${field.valueType}" 未知：按不可提交处理`,
    });
  }

  return { ok: issues.length === 0, issues, target, arguments: args };
}

/** 从已有点位的 `parameters` 还原草稿（重新核验/编辑时使用，绝不丢字段）。 */
export function nckDraftFromParameters(
  model: NckFormModel,
  parameters: Record<string, unknown> | null | undefined,
): NckFormDraft {
  const draft = emptyNckDraft(model);
  if (!parameters || typeof parameters !== "object") return draft;
  const rawTarget = isPlainObject(parameters.target) ? parameters.target : null;
  if (rawTarget) {
    for (const [key, value] of Object.entries(rawTarget)) {
      if (isPlainObject(value)) {
        if (typeof value.number === "number") draft.target[key] = String(value.number);
        if (typeof value.entry === "string") draft.target[`${key}.entry`] = value.entry;
      } else if (typeof value === "number") {
        draft.target[key] = String(value);
      } else if (typeof value === "string") {
        draft.target[key] = value;
      }
    }
  }
  if (isPlainObject(parameters.arguments)) {
    for (const [key, value] of Object.entries(parameters.arguments)) {
      if (typeof value === "string") draft.arguments[key] = value;
      else if (typeof value === "number") draft.arguments[key] = String(value);
      else draft.arguments[key] = JSON.stringify(value);
    }
  }
  return draft;
}

// ---------------------------------------------------------------------------
// 2. family 交叉校验（Endpoint 连接配置 × 定义条目）
// ---------------------------------------------------------------------------

export type NckFamilyCode =
  | "FAMILY_ENDPOINT_MISSING"
  | "FAMILY_DEFINITION_MISSING"
  | "FAMILY_MISMATCH"
  | "FAMILY_CATALOG_MISMATCH";

export type NckFamilyCheck =
  | { ok: true; family: string }
  | { ok: false; code: NckFamilyCode; message: string };

/**
 * Endpoint 有效 `connection.family` 与定义条目 `family` 的交叉校验。
 *
 * 六种场景（边界文档 §2.1 裁决表）：
 * 1. 两者一致 ⇒ 允许（返回 family，供预览请求使用）；
 * 2. Endpoint family 缺失/空串 ⇒ fail-closed（提示补连接配置）；
 * 3. 两者不一致 ⇒ 拒绝，并在文案里给出**两个具体值**；
 * 4. 目录顶层 family=null ⇒ **不是拒绝理由**（该字段是「请求未指定且多系列」
 *    的回显语义，不代表 Endpoint 意图）；只要 1 成立就允许；
 * 5. 条目缺 family ⇒ fail-closed（无法交叉校验）；
 * 6. 预览响应与提交坐标不一致 ⇒ 由 `validateNckPreviewResponse` 拒绝。
 *
 * 为什么 family 取 Endpoint 连接配置而不是目录顶层：目录顶层是请求回显，
 * 与「这台机床想用哪个系列」无关；用连接配置才能把用户意图与定义数据分开校验。
 */
export function checkNckFamily(input: {
  endpointFamily: unknown;
  definitionFamily: unknown;
  catalogFamily?: unknown;
}): NckFamilyCheck {
  const endpoint = typeof input.endpointFamily === "string" ? input.endpointFamily.trim() : "";
  if (!endpoint) {
    return {
      ok: false,
      code: "FAMILY_ENDPOINT_MISSING",
      message: "Endpoint 连接配置缺少 family：无法确认系列，禁止提交（请先补齐连接配置）",
    };
  }
  const definition =
    typeof input.definitionFamily === "string" ? input.definitionFamily.trim() : "";
  if (!definition) {
    return {
      ok: false,
      code: "FAMILY_DEFINITION_MISSING",
      message: "定义条目缺少 family：无法与 Endpoint 交叉校验，禁止提交",
    };
  }
  if (endpoint !== definition) {
    return {
      ok: false,
      code: "FAMILY_MISMATCH",
      message: `Endpoint family="${endpoint}" 与定义 family="${definition}" 不一致：禁止提交`,
    };
  }
  // 第 4 条：顶层 null/缺失不构成拒绝理由；非 null 且与 Endpoint 不一致才是异常。
  const catalog = input.catalogFamily;
  if (typeof catalog === "string" && catalog !== "" && catalog !== endpoint) {
    return {
      ok: false,
      code: "FAMILY_CATALOG_MISMATCH",
      message: `目录顶层 family="${catalog}" 与 Endpoint family="${endpoint}" 不一致：目录可能来自另一系列，禁止提交`,
    };
  }
  return { ok: true, family: endpoint };
}

// ---------------------------------------------------------------------------
// 3. 权威预览：请求指纹 / 序号判定 / 响应校验
// ---------------------------------------------------------------------------

/** 权威预览的请求形态（与待提交 `PointSelection.parameters` 逐字同构）。 */
export interface NckPreviewParameters {
  variable_id: string;
  definition_revision: number;
  target: Record<string, unknown>;
  arguments: Record<string, unknown>;
}

export function buildNckPreviewParameters(args: {
  variableId: string;
  revision: number;
  target: Record<string, unknown>;
  arguments: Record<string, unknown>;
}): NckPreviewParameters {
  return {
    variable_id: args.variableId,
    definition_revision: args.revision,
    target: args.target,
    arguments: args.arguments,
  };
}

/**
 * 输入指纹：稳定 JSON（对象 key 排序）。
 *
 * 竞态判定不能只看请求序号：序号只能证明「这是最新一次请求」，若输入在请求
 * 飞行中又改回同一实例，指纹才描述「这次响应是否对应我现在看到的值」。
 * 两者都相等才采纳（A39）。
 */
export function nckPreviewFingerprint(parameters: NckPreviewParameters): string {
  return JSON.stringify(canonicalize(parameters));
}

function canonicalize(value: unknown): unknown {
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(canonicalize);
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(value as Record<string, unknown>).sort()) {
    const v = canonicalize((value as Record<string, unknown>)[key]);
    if (v !== undefined) out[key] = v;
  }
  return out;
}

/** 迟到响应判定：序号与当前输入指纹**同时**匹配才可采纳。 */
export function shouldAcceptNckPreview(args: {
  responseSeq: number;
  latestSeq: number;
  responseFingerprint: string;
  currentFingerprint: string;
}): boolean {
  return (
    Number.isInteger(args.responseSeq) &&
    args.responseSeq === args.latestSeq &&
    args.responseFingerprint === args.currentFingerprint
  );
}

export type NckPreviewRejectCode =
  | "PREVIEW_POINT_KEY_MISSING"
  | "PREVIEW_COORDINATE_MISMATCH"
  | "PREVIEW_SCOPE_IDENTITY"
  | "PREVIEW_SCOPE_MISSING"
  | "PREVIEW_SCOPE_MISMATCH"
  | "PREVIEW_IDENTITY_MODE_INVALID";

export type NckPreviewCheck =
  | { ok: true; pointKey: string }
  | { ok: false; code: NckPreviewRejectCode; message: string };

/**
 * 校验预览响应是否可作为**当前输入**的权威身份。
 *
 * 说明（与本轮接口事实一致）：I-2 的预览响应是 6 字段，**不含 family**
 * （definition_api.rs:649-660）。因此「预览返回的 family 与提交 family 不一致」
 * 无法按字面检查；这里用等价的**坐标校验**落地：`variable_id` +
 * `definition_revision` 必须与请求完全一致，且 `scope` 必须是条目声明的
 * `collect`。任何不一致都拒绝，绝不静默采用返回值。
 */
export function validateNckPreviewResponse(args: {
  definition: NckDefinitionLike;
  expected: NckPreviewParameters;
  response: NckIdentityPreviewResponse;
}): NckPreviewCheck {
  const { expected, response, definition } = args;
  if (typeof response.point_key !== "string" || response.point_key.trim() === "") {
    return {
      ok: false,
      code: "PREVIEW_POINT_KEY_MISSING",
      message: "预览响应没有有效的 point_key：身份未确认，禁止提交",
    };
  }
  if (response.variable_id !== expected.variable_id) {
    return {
      ok: false,
      code: "PREVIEW_COORDINATE_MISMATCH",
      message: `预览响应的 variable_id=${String(
        response.variable_id,
      )} 与请求的 ${expected.variable_id} 不一致：拒绝采用该身份`,
    };
  }
  if (response.definition_revision !== expected.definition_revision) {
    return {
      ok: false,
      code: "PREVIEW_COORDINATE_MISMATCH",
      message: `预览响应的 definition_revision=${String(
        response.definition_revision,
      )} 与请求的 ${expected.definition_revision} 不一致：拒绝采用该身份`,
    };
  }
  // Preview 成功 ≠ 有采集资格：即使 Key 合法，scope=identity 仍必须拒绝。
  if (response.scope === "identity") {
    return {
      ok: false,
      code: "PREVIEW_SCOPE_IDENTITY",
      message: "预览返回 scope=identity：该变量只服务身份探针，不得作为采集点位（Key 合法也不能提交）",
    };
  }
  if (response.scope === null) {
    return {
      ok: false,
      code: "PREVIEW_SCOPE_MISSING",
      message: "预览响应缺少 scope：无法确认采集资格，禁止提交",
    };
  }
  if (response.scope !== definition.scope) {
    return {
      ok: false,
      code: "PREVIEW_SCOPE_MISMATCH",
      message: `预览返回 scope="${response.scope}" 与目录条目 scope="${String(
        definition.scope,
      )}" 不一致：拒绝采用该结论`,
    };
  }
  if (response.identity_mode !== "derived") {
    return {
      ok: false,
      code: "PREVIEW_IDENTITY_MODE_INVALID",
      message: `语义点位的 identity_mode 必须是 derived，实际 "${response.identity_mode}"`,
    };
  }
  return { ok: true, pointKey: response.point_key };
}

// ---------------------------------------------------------------------------
// 4. 已有点位的重新核验（不是「记录里有 Key 就有效」）
// ---------------------------------------------------------------------------

export type NckPointIdentityCode =
  | NckFamilyCode
  | NckPreviewRejectCode
  | NckScopeCode
  | "POINT_NOT_SEMANTIC"
  | "DEFINITION_MISSING"
  | "DEFINITION_REVISION_MISMATCH"
  | "PARAMETERS_INVALID"
  | "POINT_KEY_MISMATCH";

export type NckPointIdentityCheck =
  | { ok: true; pointKey: string; native: boolean }
  | { ok: false; code: NckPointIdentityCode; message: string };

/**
 * 重新核验一个已有点位。
 *
 * 关键口径（本轮边界 1）：**记录里有 `point_key` 不等于当前可提交**。必须同时
 * 证明：定义仍存在且 revision 命中、`scope=collect`、family 匹配、参数仍能通过
 * 定义校验、服务端预览在**当前目录**下重算出的 Key 与记录逐字相等。
 *
 * 任何一步不成立都返回具体原因；调用方必须允许用户**查看原始配置**，但不得
 * 把它作为有效点位继续提交。
 */
export function evaluateNckPointIdentity(args: {
  point: { kind: string; point_key: string; parameters: Record<string, unknown> };
  definition: NckDefinitionLike | null | undefined;
  endpointFamily: unknown;
  catalogFamily?: unknown;
  preview: NckIdentityPreviewResponse;
}): NckPointIdentityCheck {
  const { point, definition } = args;
  if (point.kind === "nck_native") {
    // 高级原生模式的身份由固定规则/用户显式给出，不适用目录 scope 与语义预览。
    // TODO(PR-C 后续)：若需要核验 native.derived 的 Key，应另走同一预览端点并
    // 逐字比对；本批只对语义点位做权威复核（与 A25/A26 的口径一致）。
    return { ok: true, pointKey: point.point_key, native: true };
  }
  if (point.kind !== "nck_variable") {
    return {
      ok: false,
      code: "POINT_NOT_SEMANTIC",
      message: `未知 Point Kind "${point.kind}"：无法确认定义与采集资格`,
    };
  }
  if (!definition) {
    return {
      ok: false,
      code: "DEFINITION_MISSING",
      message: "定义目录中找不到该变量：无法重新核验身份（原始配置仍可查看/移除）",
    };
  }
  const admission = admitNckDefinition(definition);
  if (!admission.collectable) {
    return {
      ok: false,
      code: admission.code,
      message: `${admission.message}（原始配置仍可查看/移除）`,
    };
  }
  const family = checkNckFamily({
    endpointFamily: args.endpointFamily,
    definitionFamily: definition.family,
    catalogFamily: args.catalogFamily,
  });
  if (!family.ok) return { ok: false, code: family.code, message: family.message };

  const variableId = point.parameters.variable_id;
  const revision = point.parameters.definition_revision;
  if (typeof variableId !== "string" || variableId === "") {
    return {
      ok: false,
      code: "PARAMETERS_INVALID",
      message: "点位 parameters 缺少 variable_id",
    };
  }
  if (typeof revision !== "number" || !Number.isInteger(revision)) {
    return {
      ok: false,
      code: "PARAMETERS_INVALID",
      message: "点位 parameters 缺少 definition_revision",
    };
  }
  // 参数层只允许这 4 个键（与驱动 SEMANTIC_PARAMETER_KEYS 同口径）：多一个
  // （例如把展示用的 `identity` 块折进来）就会被 Configure 整体拒绝，绝不能
  // 因为「Key 没变」就当成可提交。
  const extraKeys = Object.keys(point.parameters).filter(
    (key) =>
      key !== "variable_id" &&
      key !== "definition_revision" &&
      key !== "target" &&
      key !== "arguments",
  );
  if (extraKeys.length) {
    return {
      ok: false,
      code: "PARAMETERS_INVALID",
      message: `点位 parameters 含不允许的字段 ${extraKeys.join(",")}：驱动会整体拒绝`,
    };
  }
  if (definition.revision !== undefined && definition.revision !== revision) {
    return {
      ok: false,
      code: "DEFINITION_REVISION_MISMATCH",
      message: `点位记录的 revision=${revision} 与目录中该变量（revision=${definition.revision}）不一致：需重新配置`,
    };
  }
  const model = buildNckFormModel(definition);
  if (!model.ok) {
    return { ok: false, code: "PARAMETERS_INVALID", message: model.message };
  }
  const draft = nckDraftFromParameters(model, point.parameters);
  const validation = validateNckForm(definition, model, draft);
  if (!validation.ok) {
    return {
      ok: false,
      code: "PARAMETERS_INVALID",
      message: `点位参数不再满足定义约束：${validation.issues
        .map((i) => i.message)
        .join("；")}`,
    };
  }
  const expected = buildNckPreviewParameters({
    variableId,
    revision,
    target: validation.target,
    arguments: validation.arguments,
  });
  const preview = validateNckPreviewResponse({
    definition,
    expected,
    response: args.preview,
  });
  if (!preview.ok) return preview;
  if (preview.pointKey !== point.point_key) {
    // 三要素（持久化 Key / 服务端重算 Key / 变量+revision）都必须可见。
    return {
      ok: false,
      code: "POINT_KEY_MISMATCH",
      message: `记录中的 point_key="${point.point_key}" 与当前服务端重算 Key="${preview.pointKey}" 不一致（${variableId} rev ${revision}）：不得按原 Key 继续提交`,
    };
  }
  return { ok: true, pointKey: preview.pointKey, native: false };
}
