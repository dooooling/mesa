// Browse「语义载荷」的 NCK 实现：**只做载荷形状解析**，不做准入判定。
//
// 背景（IPC 1.8）：可选性、载荷与阻断原因已由**驱动**在 wire 上给出
//（`BrowseNode.selection_mode` / `selection_payload_json` / `diagnostics`），
// 通用 Browse 组件据此渲染状态与诊断（见 browseModel.evaluateBrowseNode）。
// 因此本模块不再「推断某个叶能不能选」——那套规则（scope 白名单、family
// 比对、定义目录查表）已经搬到驱动侧（`drivers/sinumerik-nck/src/presets.rs`
// 的 `semantic_leaf` + `browse_selection.rs`），与 Configure 的准入判定同源。
//
// 本模块剩下的唯一职责：把驱动给的不透明语义载荷翻译成父层（NckPointFields /
// NckSemanticPicker）认识的 `NckBrowseLeafSelection`。形状不合契约 ⇒
// `ok:false` + 稳定码（fail-closed，绝不把畸形载荷塞进表单）。
//
// 仍然**不做**的事：
// - 不把 `nck_native` 放进语义 Browse（原生地址走「高级原生配置」）；
// - 不承诺身份：`preview_point_key` 只是展示，父层必须置空并重新走服务端
//   Preview（身份唯一权威在服务端）。
import type { BrowseSemanticHandler, BrowseSemanticParse } from "./browseSemanticHandler";

/** NCK 身份块（来自 Driver 的语义载荷；本模块只读展示）。 */
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

/**
 * 载荷形状违规码（**消费侧**码）。
 *
 * 与驱动侧诊断码不同：驱动码说明「驱动为什么不让选」，这里的码说明
 * 「驱动给的载荷不合本端契约」。两者都只用于展示，不构成放行依据。
 */
export const NCK_PAYLOAD_CODES = {
  UNEXPECTED_POINT_KIND: "NCK_PAYLOAD_UNEXPECTED_POINT_KIND",
  NATIVE_NOT_FROM_BROWSE: "NCK_PAYLOAD_NATIVE_NOT_FROM_BROWSE",
  MISSING_IDENTITY: "NCK_PAYLOAD_MISSING_IDENTITY",
  INVALID_PARAMETERS: "NCK_PAYLOAD_INVALID_PARAMETERS",
  MISSING_VARIABLE_ID: "NCK_PAYLOAD_MISSING_VARIABLE_ID",
  MISSING_REVISION: "NCK_PAYLOAD_MISSING_REVISION",
} as const;

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

function reject(code: string, message: string): BrowseSemanticParse<NckBrowseLeafSelection> {
  return { ok: false, code, message };
}

/**
 * 把驱动的语义载荷解析成 `NckBrowseLeafSelection`。
 *
 * 逐条 fail-closed（与既有 A16/A22 口径一致）：
 * - `kind` 必须是 NCK 点位 kind，`nck_native` 一律拒绝（应走高级原生配置）；
 * - 必须带 identity（A16）：缺它就无法核对身份，入口关闭；
 * - `parameters` 必须是对象，且含非空 `variable_id` 与整数 `definition_revision`；
 * - identity 绝不折进 `parameters`（否则会被持久化，Core 整体拒绝）。
 */
export function parseNckSemanticPayload(
  payload: Record<string, unknown>,
): BrowseSemanticParse<NckBrowseLeafSelection> {
  const kind = payload.kind;
  if (!isNckPointKind(kind)) {
    return reject(
      NCK_PAYLOAD_CODES.UNEXPECTED_POINT_KIND,
      "语义载荷的 point kind 不是 NCK 点位类型：不可选用",
    );
  }
  if (kind === "nck_native") {
    return reject(
      NCK_PAYLOAD_CODES.NATIVE_NOT_FROM_BROWSE,
      "原生点位请使用「高级原生配置」，不要从语义 Browse 加入",
    );
  }

  const identity = parseIdentity(payload.identity);
  const rawParameters = payload.parameters;
  if (!identity) {
    // A16：无 identity / 形态非法 ⇒ 入口关闭。
    return reject(
      NCK_PAYLOAD_CODES.MISSING_IDENTITY,
      "语义叶缺少 identity 或形态非法：不可选用（A16）",
    );
  }
  if (!rawParameters || typeof rawParameters !== "object" || Array.isArray(rawParameters)) {
    return reject(
      NCK_PAYLOAD_CODES.INVALID_PARAMETERS,
      "语义叶的 parameters 不是对象：不可选用",
    );
  }
  const parameters = rawParameters as Record<string, unknown>;

  const variableId = parameters.variable_id;
  if (typeof variableId !== "string" || variableId === "") {
    return reject(NCK_PAYLOAD_CODES.MISSING_VARIABLE_ID, "语义叶缺少 variable_id");
  }
  const revision = parameters.definition_revision;
  if (typeof revision !== "number" || !Number.isInteger(revision)) {
    return reject(NCK_PAYLOAD_CODES.MISSING_REVISION, "语义叶缺少 definition_revision");
  }

  return {
    ok: true,
    label: "选用（语义）",
    selection: {
      pointKind: kind,
      resourceId:
        typeof payload.resource_id === "string" && payload.resource_id.trim()
          ? payload.resource_id.trim()
          : kind,
      parameters,
      identity,
    },
  };
}

/**
 * 构造注入给通用 Browse 组件的语义处理器。
 *
 * 无参数：准入判定在驱动侧，前端不再需要目录/family 上下文——这正是本轮
 * 「消除前端推断」的落点。父层仍需自己的目录数据来做**表单**（必填项、可选
 * 变量列表），但那与"能不能选这个叶"无关。
 */
export function createNckSemanticHandler(): BrowseSemanticHandler<NckBrowseLeafSelection> {
  return {
    name: "nck-semantic",
    notProvidedReason: "上层未提供语义表单入口：不可选用",
    selectLabel: "选用（语义）",
    parse: parseNckSemanticPayload,
  };
}
