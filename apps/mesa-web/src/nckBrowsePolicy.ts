// Browse「语义叶」判定的 NCK 实现（PR-C 第三批语义集中到此文件）。
//
// 背景：通用 Browse 组件曾直接识别 `nck_variable` / `nck_native` / `scope` /
// `family`。现在这些**驱动专属语义**只剩在这里：通用组件通过
// `BrowseLeafPolicy` 注入本策略，自己不认识任何 nck 字面量。
//
// 判定口径（fail-closed，逐条与既有断言一致）：
// - 语义叶必须带 identity（A16），否则入口关闭；
// - `nck_native` 不得从语义 Browse 选用（应走高级原生配置）；
// - `scope=identity` / 定义缺失 / family 不一致 / 目录不可用 ⇒ 显示原因且不给
//   「选用」（A13）；
// - 旧 `variable` 原生信封是「语义载荷不可用」的降级形态（presets.rs:161-212），
//   只提示改用高级原生配置，**不得**回落到旧通用 resource 回填（A22）。
//
// 身份纪律：Browse 的 `preview_point_key` **只展示**，不作最终身份；提交前
// 仍必须由服务端 identity-preview / Configure 给出（服务端权威校验不变）。
import { parseBrowseEnvelope, type BrowseNode } from "./browseModel";
import type {
  BrowseLeafDisplay,
  BrowseLeafOwned,
  BrowseLeafPolicy,
  BrowseLeafVerdict,
} from "./browseLeafPolicy";
import { evaluateNckSelectionAdd } from "./nckCollectability";
import { checkNckFamily, type NckDefinitionLike } from "./nckPointModel";

/** Browse 的语义准入上下文（由 Picker / Endpoint 页注入）。 */
export interface NckSemanticBrowseConfig {
  /** 目录定义（只读）。缺失即语义叶一律不可选（fail-closed）。 */
  definitions?: readonly NckDefinitionLike[] | null;
  endpointFamily?: string | null;
  catalogFamily?: string | null;
}

/** NCK 身份块（来自 Driver 的语义 binding；本模块只读展示）。 */
export interface NckBrowseIdentity {
  template?: string;
  keys?: Array<{ path?: string; required?: boolean }>;
  /** Driver 的预览 Key；**仅初始展示**，不得作为提交来源。 */
  preview_point_key?: string | null;
}

/** 语义叶的选用载荷（交给父层语义表单；parameters 已被 Core 契约约束）。 */
export interface NckBrowseLeafSelection {
  /** 点位 kind（父层据此构造 PointSelection，不再自己硬编码字符串）。 */
  pointKind: NckBrowsePointKind;
  resourceId: string;
  /** Core `PointSelection.parameters` 的形状（恰 4 键；identity 不在其中）。 */
  parameters: Record<string, unknown>;
  /** 仅展示用的身份块，**不得**折进 parameters。 */
  identity: NckBrowseIdentity;
}

/** NCK 点位层 kind（与通用 resource_id 不同层级）。 */
export type NckBrowsePointKind = "nck_variable" | "nck_native";

function isNckPointKind(value: unknown): value is NckBrowsePointKind {
  return value === "nck_variable" || value === "nck_native";
}

function parseIdentity(value: unknown): NckBrowseIdentity | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const obj = value as Record<string, unknown>;
  const out: NckBrowseIdentity = {};
  if (typeof obj.template === "string") out.template = obj.template;
  if (Array.isArray(obj.keys)) {
    out.keys = obj.keys
      .filter((k): k is Record<string, unknown> => !!k && typeof k === "object" && !Array.isArray(k))
      .map((k) => ({
        path: typeof k.path === "string" ? k.path : undefined,
        required: typeof k.required === "boolean" ? k.required : undefined,
      }));
  }
  if (obj.preview_point_key === null) out.preview_point_key = null;
  else if (typeof obj.preview_point_key === "string") {
    out.preview_point_key = obj.preview_point_key;
  }
  return out;
}

/**
 * 旧 `variable` 原生信封：显式 resource_id 且**没有** kind。
 *
 * 在 NCK 上下文里这是「语义载荷不可用」的降级形态（presets.rs 语义化之前
 * 的信封），只能提示改用高级原生配置；不得把它当成通用资源回填（A22）。
 */
function isLegacyVariableEnvelope(envelope: Record<string, unknown>): boolean {
  return envelope.resource_id === "variable" && envelope.kind === undefined;
}

function findDefinition(
  definitions: readonly NckDefinitionLike[] | null | undefined,
  variableId: string,
  revision: number | null,
): NckDefinitionLike | null {
  if (!definitions) return null;
  for (const definition of definitions) {
    if (definition.variable_id !== variableId) continue;
    if (
      revision !== null &&
      typeof definition.revision === "number" &&
      definition.revision !== revision
    ) {
      continue;
    }
    return definition;
  }
  return null;
}

/** 不可选用的统一形状（`blockTestId` 沿用既有测试锚点）。 */
function blocked(
  reason: string,
  display?: BrowseLeafDisplay,
): BrowseLeafOwned<NckBrowseLeafSelection> {
  return {
    mode: "owned",
    selectable: false,
    reason,
    blockTestId: "browse-semantic-block",
    ...display,
  };
}

const SEMANTIC_BADGE = "语义点位";

const LEGACY_VARIABLE_NOTICE = {
  text: "语义载荷不可用（内嵌定义目录缺少该变量）：只能使用「高级原生配置」，不得走旧 variable 采集入口",
  testId: "browse-legacy-variable-block",
} as const;

/**
 * 判定一个 Browse 节点在 NCK 语义下的归属。
 *
 * 非语义叶（无 kind 载荷、且不是旧 variable 信封）返回 `{ mode: "generic" }`，
 * 通用资源叶规则逐字保持（A44）。
 */
export function evaluateNckBrowseLeaf(
  node: BrowseNode,
  config: NckSemanticBrowseConfig | null | undefined,
): BrowseLeafVerdict<NckBrowseLeafSelection> {
  const envelope = parseBrowseEnvelope(node);
  if (!envelope) return { mode: "generic" };
  if (!isNckPointKind(envelope.kind)) {
    // 旧 variable 原生信封：语义载荷不可用 ⇒ 阻断，且绝不回落通用回填（A22）。
    if (isLegacyVariableEnvelope(envelope)) {
      return {
        mode: "owned",
        selectable: false,
        notice: LEGACY_VARIABLE_NOTICE,
        blockTestId: "browse-semantic-block",
      };
    }
    return { mode: "generic" };
  }

  const kind = envelope.kind;
  const tags = kind === "nck_native" ? [kind] : [];
  const identity = parseIdentity(envelope.identity);
  const parameters =
    envelope.parameters && typeof envelope.parameters === "object" && !Array.isArray(envelope.parameters)
      ? (envelope.parameters as Record<string, unknown>)
      : null;

  // A16：无 identity（或形态非法）/ parameters 形态非法 ⇒ 不可选，入口关闭。
  if (!identity || !parameters) {
    return blocked("语义叶缺少 identity 或形态非法：不可选用（A16）", {
      badge: SEMANTIC_BADGE,
      tags,
    });
  }

  // 预览提示只做展示：null 表示该模板的 target/arguments 尚未补全（A10）。
  const preview: { text: string; tone: "muted" | "warn" } | undefined =
    identity.preview_point_key
      ? {
          text: `Browse 预览：${identity.preview_point_key}（仅展示；提交前仍需服务端确认）`,
          tone: "muted",
        }
      : kind === "nck_variable"
        ? {
            text: "Browse 预览：身份未确定（target/arguments 不完整）——可选用后到语义表单补全，提交前必须由服务端 Preview 确认",
            tone: "warn",
          }
        : undefined;

  const base = { badge: SEMANTIC_BADGE, tags, preview } as const;

  if (kind === "nck_native") {
    return blocked("原生点位请使用「高级原生配置」，不要从语义 Browse 加入", base);
  }

  const variableId = parameters.variable_id;
  const revision = parameters.definition_revision;
  if (typeof variableId !== "string" || variableId === "") {
    return blocked("语义叶缺少 variable_id", base);
  }
  if (typeof revision !== "number" || !Number.isInteger(revision)) {
    return blocked("语义叶缺少 definition_revision", base);
  }
  if (!config) {
    return blocked("未提供语义目录上下文：不可选用", base);
  }
  if (!config.definitions) {
    return blocked("定义目录不可用：无法确认 scope，禁止选用（fail-closed）", base);
  }
  const definition = findDefinition(config.definitions, variableId, revision);
  if (!definition) {
    return blocked(`定义目录中找不到 ${variableId}（rev ${revision}）：无法确认 scope`, base);
  }
  const admission = evaluateNckSelectionAdd(definition);
  if (!admission.ok) {
    return blocked(`${admission.code}：${admission.message}`, base);
  }
  const family = checkNckFamily({
    endpointFamily: config.endpointFamily,
    definitionFamily: definition.family,
    catalogFamily: config.catalogFamily,
  });
  if (!family.ok) {
    return blocked(`${family.code}：${family.message}`, base);
  }

  // 可选用：载荷交给父层语义表单。preview_point_key 只随载荷展示，
  // 父层必须把它置空并重新走服务端 Preview（这里不做任何身份承诺）。
  return {
    mode: "owned",
    selectable: true,
    selection: {
      pointKind: kind,
      resourceId:
        typeof envelope.resource_id === "string" && envelope.resource_id.trim()
          ? envelope.resource_id.trim()
          : kind,
      parameters,
      identity,
    },
    selectLabel: "选用（语义）",
    blockTestId: "browse-semantic-block",
    ...base,
  };
}

/**
 * 构造注入给通用 Browse 组件的策略对象。
 *
 * 调用方（NCK 页面）只需传目录与 family 上下文；通用组件不感知这些字段名。
 */
export function createNckBrowseLeafPolicy(
  config: NckSemanticBrowseConfig | null | undefined,
): BrowseLeafPolicy<NckBrowseLeafSelection> {
  return {
    name: "nck-semantic",
    missingHandlerReason: "上层未提供语义表单入口：不可选用",
    evaluate: (node) => evaluateNckBrowseLeaf(node, config),
  };
}
