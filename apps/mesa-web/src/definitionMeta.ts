// 定义元数据 sidecar（`unit` / `manual_location` / `index_rule` / `availability`）
// 的**唯一**读取层：把后端投影出来的原始 JSON 收敛成"可展示 + 可判定"的视图。
//
// 为什么单独一层（而不是在组件里 `definition.unit?.value ?? ""`）：
// 1. **容错必须集中**：四块都是"附加信息"，字段缺失、类型漂移、取值超出词表都
//    不能崩，也不能被 `??` / `||` 悄悄补成某个看似合理的值（那就是"猜"）。
//    判定与兜底只允许写一次，否则树、表单、未来的新入口会各自兜出不同结论。
// 2. **静态可用性 ≠ 动态对象存在性**：`availability` 描述的是"这条定义在元数据层面
//    是否可用"，与"当前这台机床有没有该对象"是两件事（后者只能靠探测/真机读取）。
//    本模块只回答前者，绝不因为 `ready` 就宣告后者（见 `STATIC_AVAILABILITY_NOTE`）。
// 3. **fail-closed 是默认值**：`availability` 缺失或取值非法 ⇒ 一律 `unavailable`
//    （前端内部态，**不是**后端四态之一），门禁据此阻断且必须可见。
//
// 本文件为纯模型：不 import React/AntD/请求层，可被 vitest 直接驱动。
// 也**不**包含任何厂商判断：只认 sidecar 的字段名与白名单字面量。

/** 后端声明的四态（与 sidecar `availability.state` 一一对应）。 */
export const AVAILABILITY_STATE = {
  READY: "ready",
  CANDIDATE: "candidate",
  HOLD: "hold",
  REFERENCE_ONLY: "reference_only",
} as const;

/** 后端四态的联合类型。 */
export type DeclaredAvailabilityState =
  (typeof AVAILABILITY_STATE)[keyof typeof AVAILABILITY_STATE];

/**
 * 前端内部态：`availability` 缺失或形状/取值非法。
 *
 * 刻意**不**并入后端词表（不叫 `unknown`、也不复用四态之一）：
 * "驱动说它是 hold" 与 "驱动什么都没说" 在产品上是两件事，前者有原因可查，
 * 后者只能 fail-closed。混成一个值会让文案与排查都失去分辨力。
 */
export const AVAILABILITY_UNAVAILABLE = "unavailable";

export type AvailabilityViewState = DeclaredAvailabilityState | typeof AVAILABILITY_UNAVAILABLE;

/** `unavailable` 的两种成因（文案与排查口径不同，必须区分）。 */
export type AvailabilityInvalidCode = "AVAILABILITY_MISSING" | "AVAILABILITY_INVALID";

/** 可用性视图：展示文案 + 门禁判据，两者同源（不各算一次）。 */
export interface AvailabilityView {
  /** 四态之一，或前端内部态 `unavailable`。 */
  state: AvailabilityViewState;
  /** 原始 `state` 值（逐字保留；缺失为 null；非法时为原文）。 */
  rawState: string | null;
  /** 仅 `unavailable` 时非 null。 */
  invalidCode: AvailabilityInvalidCode | null;
  /** 驱动给出的依据码（只展示，不参与判定）。 */
  basis: string[];
  /** 驱动给出的中文原因（`hold` 必须展示；其它态可能为 null）。 */
  reasonZh: string | null;
  /** 完整标签（表单/详情用）。 */
  label: string;
  /** 短标签（树上的小角标用，不喧宾夺主）。 */
  shortLabel: string;
  /** 完整说明（tooltip / 详情行）。 */
  hint: string;
  /** 是否允许作为普通采集点添加（`ready` / `candidate` 才为真）。 */
  addable: boolean;
  /** 添加时必须标注"未经真机读取验证"（只有 `candidate`）。 */
  requiresUnverifiedNotice: boolean;
}

/** 单位视图。 */
export interface UnitView {
  /** 只有"驱动声明 verified 且确实给出取值"才是 `verified`。 */
  status: "verified" | "unknown";
  /** 原始 status 值（逐字；缺失 null）。 */
  rawStatus: string | null;
  /** 单位取值；`unknown` 时**恒为 null**（绝不补默认值、绝不展示空串）。 */
  value: string | null;
  sourceRef: string | null;
  /** 展示文本：真实单位值，或"单位未核实"。 */
  label: string;
  hint: string;
}

/** 手册位置视图（页码/章节只在详情或 tooltip 中出现，不占主视觉）。 */
export interface ManualLocationView {
  documentNumber: string | null;
  version: string | null;
  section: string | null;
  page: number | null;
  sourceRef: string | null;
  /** 单行位置文本；**所有字段都不可用**时为 null（不显示假位置）。 */
  label: string | null;
  hint: string;
}

/** 索引规则视图（`kind` 是数据，前端不做任何前缀/前缀归类推断）。 */
export interface IndexRuleView {
  kind: string | null;
  status: string | null;
  verified: boolean;
  label: string;
  hint: string;
}

/** 一条定义的 sidecar 四块视图。 */
export interface DefinitionMetaView {
  unit: UnitView;
  manualLocation: ManualLocationView;
  indexRule: IndexRuleView;
  availability: AvailabilityView;
}

/** 读取 sidecar 所需的最小形态（宽松：注入测试只给子集也成立）。 */
export interface DefinitionMetaLike {
  unit?: unknown;
  manual_location?: unknown;
  index_rule?: unknown;
  availability?: unknown;
}

/** 静态可用性说明：**不是**"当前机床有该对象"。 */
export const STATIC_AVAILABILITY_NOTE =
  "静态可用性来自定义元数据（sidecar），与当前机床是否存在该对象无关";

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 非空字符串或 null（空串按"未提供"处理：空串不是事实）。 */
function readText(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed;
}

/** 正整数或 null（页码 0 / 负数 / 小数 / 字符串都不认）。 */
function readPage(value: unknown): number | null {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 1) return null;
  return value;
}

/** 字符串数组（非字符串元素逐个丢弃，不整块崩）。 */
function readTextArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map((item) => readText(item)).filter((item): item is string => item !== null);
}

// ---------------------------------------------------------------------------
// 1. 单位
// ---------------------------------------------------------------------------

/**
 * 读取 `unit`。
 *
 * 判据只有一条：`status === "verified"` **且** `value` 是非空字符串才展示取值。
 * 其余情况（缺失、形状非法、status 未知、verified 但没给值）一律 `unknown`，
 * 展示"单位未核实"——空白或假单位都会让用户误以为"这个量没有单位/单位就是空"。
 */
export function readUnitMeta(raw: unknown): UnitView {
  if (!isPlainObject(raw)) {
    return {
      status: "unknown",
      rawStatus: null,
      value: null,
      sourceRef: null,
      label: "单位未核实",
      hint: "驱动未提供单位信息：按未核实处理（不补默认值、不猜单位）",
    };
  }
  const rawStatus = readText(raw.status);
  const value = readText(raw.value);
  const sourceRef = readText(raw.source_ref);
  if (rawStatus === "verified" && value !== null) {
    return {
      status: "verified",
      rawStatus,
      value,
      sourceRef,
      label: value,
      hint: `单位：${value}（驱动声明已核实${sourceRef ? ` · 依据：${sourceRef}` : ""}）`,
    };
  }
  if (rawStatus === "verified") {
    // 声明"已核实"却没有取值：这本身是契约缺口，绝不能渲染成空串或"已核实"。
    return {
      status: "unknown",
      rawStatus,
      value: null,
      sourceRef,
      label: "单位未核实",
      hint: "驱动声明单位已核实，但未给出取值：按未核实处理（不补默认值）",
    };
  }
  return {
    status: "unknown",
    rawStatus,
    value: null,
    sourceRef,
    label: "单位未核实",
    hint: rawStatus
      ? `单位未核实（驱动状态：${rawStatus}）`
      : "单位未核实（驱动未给出状态）",
  };
}

// ---------------------------------------------------------------------------
// 2. 手册位置
// ---------------------------------------------------------------------------

/** 读取 `manual_location`：页码/章节可查，但一个可用字段都没有时返回 null 文本。 */
export function readManualLocationMeta(raw: unknown): ManualLocationView {
  if (!isPlainObject(raw)) {
    return {
      documentNumber: null,
      version: null,
      section: null,
      page: null,
      sourceRef: null,
      label: null,
      hint: "驱动未提供手册位置",
    };
  }
  const documentNumber = readText(raw.document_number);
  const version = readText(raw.version);
  const section = readText(raw.section);
  const page = readPage(raw.page);
  const sourceRef = readText(raw.source_ref);
  const parts: string[] = [];
  if (documentNumber) parts.push(version ? `${documentNumber} ${version}` : documentNumber);
  if (section) parts.push(`§${section}`);
  if (page !== null) parts.push(`第 ${page} 页`);
  const label = parts.length ? parts.join(" · ") : null;
  return {
    documentNumber,
    version,
    section,
    page,
    sourceRef,
    label,
    hint: label
      ? `手册位置：${label}${sourceRef ? `（来源：${sourceRef}）` : ""}`
      : "驱动提供了 manual_location，但没有可用的文档号/章节/页码",
  };
}

// ---------------------------------------------------------------------------
// 3. 索引规则
// ---------------------------------------------------------------------------

/** 读取 `index_rule`：`kind` 逐字展示，`status === "verified"` 才算核实。 */
export function readIndexRuleMeta(raw: unknown): IndexRuleView {
  if (!isPlainObject(raw)) {
    return {
      kind: null,
      status: null,
      verified: false,
      label: "索引规则未声明",
      hint: "驱动未提供索引规则：不据此推断索引方式",
    };
  }
  const kind = readText(raw.kind);
  const status = readText(raw.status);
  const verified = status === "verified";
  return {
    kind,
    status,
    verified,
    label: kind ? `${kind}${verified ? "" : "（未核实）"}` : "索引规则未声明",
    hint: kind
      ? `索引规则：${kind}（${status ?? "状态未提供"}）${verified ? "" : " · 未核实"}`
      : "驱动提供了 index_rule，但未给出 kind",
  };
}

// ---------------------------------------------------------------------------
// 4. 可用性（门禁的唯一判据）
// ---------------------------------------------------------------------------

function availabilityHint(state: DeclaredAvailabilityState, reasonZh: string | null): string {
  const suffix = reasonZh ? ` · 驱动原因：${reasonZh}` : "";
  switch (state) {
    case AVAILABILITY_STATE.READY:
      return `静态可用性 ready：定义侧已核实可采集${suffix}。注意：${STATIC_AVAILABILITY_NOTE}`;
    case AVAILABILITY_STATE.CANDIDATE:
      return `静态可用性 candidate：可查看、可配置，但未经真机读取验证；添加时必须标注未验证${suffix}`;
    case AVAILABILITY_STATE.HOLD:
      return `静态可用性 hold：禁止自动生成采集配置、禁止添加${suffix}`;
    case AVAILABILITY_STATE.REFERENCE_ONLY:
      return `静态可用性 reference_only：只作展示，不可作为普通采集点添加${suffix}`;
  }
}

/**
 * 读取 `availability`。
 *
 * 三种失败都收敛到 `unavailable`（fail-closed），但成因分开记录：
 * - 字段缺失 / 不是对象 ⇒ `AVAILABILITY_MISSING`（驱动没声明）；
 * - 对象在、但 `state` 不是字符串 ⇒ `AVAILABILITY_MISSING`（等于没声明，形状非法）；
 * - `state` 是字符串但不在四态词表 ⇒ `AVAILABILITY_INVALID`（原文保留在 `rawState`）。
 */
export function readAvailabilityMeta(raw: unknown): AvailabilityView {
  const unavailable = (
    code: AvailabilityInvalidCode,
    rawState: string | null,
    hint: string,
  ): AvailabilityView => ({
    state: AVAILABILITY_UNAVAILABLE,
    rawState,
    invalidCode: code,
    basis: [],
    reasonZh: null,
    label: "可用性未知",
    shortLabel: "可用性未知",
    hint,
    addable: false,
    requiresUnverifiedNotice: false,
  });

  if (!isPlainObject(raw)) {
    return unavailable(
      "AVAILABILITY_MISSING",
      null,
      "驱动未提供 availability：无法确认该变量是否可用，按 fail-closed 禁止添加",
    );
  }
  const rawState = readText(raw.state);
  if (rawState === null) {
    return unavailable(
      "AVAILABILITY_MISSING",
      null,
      "availability 缺少可用的 state：无法确认该变量是否可用，按 fail-closed 禁止添加",
    );
  }
  if (
    rawState !== AVAILABILITY_STATE.READY &&
    rawState !== AVAILABILITY_STATE.CANDIDATE &&
    rawState !== AVAILABILITY_STATE.HOLD &&
    rawState !== AVAILABILITY_STATE.REFERENCE_ONLY
  ) {
    return unavailable(
      "AVAILABILITY_INVALID",
      rawState,
      `availability.state 取值非法（${rawState}）：不在 ready/candidate/hold/reference_only 内，按 fail-closed 禁止添加`,
    );
  }
  const state = rawState as DeclaredAvailabilityState;
  const reasonZh = readText(raw.reason_zh);
  const basis = readTextArray(raw.basis);
  return {
    state,
    rawState,
    invalidCode: null,
    basis,
    reasonZh,
    label: availabilityLabel(state),
    shortLabel: availabilityShortLabel(state),
    hint: availabilityHint(state, reasonZh),
    addable: state === AVAILABILITY_STATE.READY || state === AVAILABILITY_STATE.CANDIDATE,
    requiresUnverifiedNotice: state === AVAILABILITY_STATE.CANDIDATE,
  };
}

/** 完整标签（表单/详情）。 */
export function availabilityLabel(state: DeclaredAvailabilityState): string {
  switch (state) {
    case AVAILABILITY_STATE.READY:
      return "可用（ready）";
    case AVAILABILITY_STATE.CANDIDATE:
      return "候选（candidate，未经真机验证）";
    case AVAILABILITY_STATE.HOLD:
      return "暂缓（hold）";
    case AVAILABILITY_STATE.REFERENCE_ONLY:
      return "仅参考（reference_only）";
  }
}

/** 短标签（树上角标；四态各有不同文案 + 颜色，见 CatalogCategoryTree）。 */
export function availabilityShortLabel(state: AvailabilityViewState): string {
  switch (state) {
    case AVAILABILITY_STATE.READY:
      return "可用";
    case AVAILABILITY_STATE.CANDIDATE:
      return "候选";
    case AVAILABILITY_STATE.HOLD:
      return "暂缓";
    case AVAILABILITY_STATE.REFERENCE_ONLY:
      return "仅参考";
    case AVAILABILITY_UNAVAILABLE:
      return "可用性未知";
  }
}

/**
 * 可用性的视觉标记（AntD Tag 颜色名）。
 *
 * 纯展示映射：**不参与**任何判定（判据只认 `state` 与门禁结果），放在这里只为让
 * 树上与表单用同一套颜色，避免"同一个 hold 在两处颜色不同"。
 */
export const AVAILABILITY_TAG_COLOR: Record<AvailabilityViewState, string> = {
  [AVAILABILITY_STATE.READY]: "green",
  [AVAILABILITY_STATE.CANDIDATE]: "gold",
  [AVAILABILITY_STATE.HOLD]: "red",
  [AVAILABILITY_STATE.REFERENCE_ONLY]: "blue",
  [AVAILABILITY_UNAVAILABLE]: "orange",
};

// ---------------------------------------------------------------------------
// 5. 门禁（fail-closed）
// ---------------------------------------------------------------------------

export type AvailabilityGateCode =
  | AvailabilityInvalidCode
  | "AVAILABILITY_HOLD"
  | "AVAILABILITY_REFERENCE_ONLY";

export type AvailabilityGate =
  | {
      ok: true;
      state: typeof AVAILABILITY_STATE.READY | typeof AVAILABILITY_STATE.CANDIDATE;
      /** 添加时必须标注"未经真机读取验证"。 */
      requiresUnverifiedNotice: boolean;
      /** 添加处展示的标注文案（`ready` 时为 null）。 */
      notice: string | null;
    }
  | { ok: false; code: AvailabilityGateCode; message: string; reasonZh: string | null };

/**
 * 可用性门禁：**唯一**决定"能不能作为普通采集点添加"的函数。
 *
 * 与 `scope` 准入（nckCollectability）和分类守卫（catalogTree）互不替代：
 * 三条判定各回答一个问题，任一条失败都不得添加。
 */
export function evaluateAvailabilityGate(view: AvailabilityView): AvailabilityGate {
  switch (view.state) {
    case AVAILABILITY_STATE.READY:
      return { ok: true, state: AVAILABILITY_STATE.READY, requiresUnverifiedNotice: false, notice: null };
    case AVAILABILITY_STATE.CANDIDATE:
      return {
        ok: true,
        state: AVAILABILITY_STATE.CANDIDATE,
        requiresUnverifiedNotice: true,
        notice: "未经真机读取验证（candidate）：该变量尚未在本机读取确认",
      };
    case AVAILABILITY_STATE.HOLD:
      return {
        ok: false,
        code: "AVAILABILITY_HOLD",
        message: `不可添加：可用性为 hold（暂缓）${
          view.reasonZh ? ` · 原因：${view.reasonZh}` : " · 驱动未给出原因"
        }`,
        reasonZh: view.reasonZh,
      };
    case AVAILABILITY_STATE.REFERENCE_ONLY:
      return {
        ok: false,
        code: "AVAILABILITY_REFERENCE_ONLY",
        message: `仅作展示（reference_only）：不可作为普通采集点添加${
          view.reasonZh ? ` · 原因：${view.reasonZh}` : ""
        }`,
        reasonZh: view.reasonZh,
      };
    case AVAILABILITY_UNAVAILABLE:
      return {
        ok: false,
        code: view.invalidCode ?? "AVAILABILITY_MISSING",
        message: view.hint,
        reasonZh: null,
      };
  }
}

// ---------------------------------------------------------------------------
// 6. 汇总
// ---------------------------------------------------------------------------

/**
 * 一次性读出四块视图。
 *
 * `definition` 为 null/undefined 时四块全部按"未声明"处理（**不抛异常**）：
 * 未选择变量是正常状态，不该让表单崩溃，也不该被当成"可用"。
 */
export function readDefinitionMeta(
  definition: DefinitionMetaLike | null | undefined,
): DefinitionMetaView {
  const source: DefinitionMetaLike = definition ?? {};
  return {
    unit: readUnitMeta(source.unit),
    manualLocation: readManualLocationMeta(source.manual_location),
    indexRule: readIndexRuleMeta(source.index_rule),
    availability: readAvailabilityMeta(source.availability),
  };
}

/**
 * 静态可用性 × 动态对象探测的**分离展示**判据。
 *
 * `ready` 只说明"元数据层面可用"，`state` 说明"当前机床是否已确认存在该对象"：
 * - `not_required`：定义不声明对象实例需求，静态信息即全部；
 * - `confirmed`：已按本机探测到的实际对象填参（动态证据）；
 * - `unconfirmed`：需要对象但尚未探测/未确认 —— 必须与静态 `ready` 分开显示。
 */
export type ObjectDynamicState = "not_required" | "confirmed" | "unconfirmed";

export function objectDynamicState(args: {
  /** 定义声明的对象需求数量（`resolveObjectNeeds` 的结果长度）。 */
  needCount: number;
  /** 是否已按本机实际对象填过参。 */
  filledFromDevice: boolean;
}): ObjectDynamicState {
  if (args.filledFromDevice) return "confirmed";
  return args.needCount > 0 ? "unconfirmed" : "not_required";
}

/** 动态对象状态的展示文案（与静态可用性分开呈现，避免"ready=本机有"的误读）。 */
export function describeObjectDynamicState(state: ObjectDynamicState): string {
  switch (state) {
    case "not_required":
      return "本定义不声明对象实例：静态可用性即全部信息";
    case "confirmed":
      return "已按本机探测到的实际对象填参（动态证据）";
    case "unconfirmed":
      return "当前机床对象未探测确认：静态可用性不代表本机存在该对象";
  }
}
