/**
 * 连接配置的**纯函数状态模型**（C2）。
 *
 * 设计边界（对应 V1.1 §8.3 / §4）：
 * - 本模块不依赖 React、不碰 DOM，只做「raw draft → canonical 请求快照」与
 *   状态转换，因此可以完整单测；组件只负责渲染与事件转发。
 * - **TSAP 等字段的权威来源是 raw 文本**：所有校验/探测/保存动作都从当前
 *   raw 值现场归一化，绝不依赖 blur/Enter 已提交的旧值（A05）。
 * - **旧结果必须失效**：任何有效参数变化都 `revision += 1` 并清空上一次的
 *   校验/探测结果，避免「改了参数还显示旧的绿色结果」（A03）。
 * - **错误显示必须先解析**：HTTP 状态 + 响应体 → 结构化 outcome，
 *   再由 `describeOutcome()` 产出字符串；绝不把错误对象直接交给 React
 *   渲染（A11 / A26）。
 */

// ---------------------------------------------------------------------------
// Descriptor 的结构化最小视图（与 core-types 的 FieldDescriptor 兼容）
// ---------------------------------------------------------------------------

export interface UiHints {
  group?: string | null;
  order?: number | null;
  advanced?: boolean | null;
  placeholder?: string | null;
}

export interface FieldValidation {
  min?: number | null;
  max?: number | null;
  enum_options?: string[] | null;
}

export interface FieldDescriptor {
  key: string;
  label?: string;
  field_type?: string;
  required?: boolean;
  default?: unknown;
  validation?: FieldValidation | null;
  ui?: UiHints | null;
}

export type ConnectionValues = Record<string, unknown>;

export interface Issue {
  /** 例如 `connection.local_tsap`，供前端定位到具体字段 */
  path: string;
  code: string;
  message: string;
}

// ---------------------------------------------------------------------------
// 数值/TSAP 文本解析
// ---------------------------------------------------------------------------

export type ParsedInt =
  | { ok: true; value: number }
  | { ok: false; code: string; message: string };

/** TSAP 字段（`local_tsap` / `remote_tsap` 等）允许十六进制写法。 */
export function isTsapKey(key: string): boolean {
  return /(^|_)tsap$/i.test(key);
}

/**
 * 解析用户输入的整数文本。
 *
 * - `allowHex=true`（TSAP）：接受 `1024` / `0x0400` / `0X0400` / `0100h` / `0100H`；
 * - `allowHex=false`（port / timeout_ms 等）：只接受十进制，**不接受**十六进制，
 *   避免把 `0x0100` 这类写法悄悄当成另一个数值；
 * - 非法输入**不返回回退值**——调用方必须把 issue 暴露给用户，
 *   绝不静默替换成上一次的合法值（V1.1 §4 TSAP 规则）。
 */
export function parseIntInput(
  raw: unknown,
  opts: { allowHex: boolean; min?: number; max?: number },
): ParsedInt {
  const { allowHex, min = 0, max = 65535 } = opts;
  if (raw === null || raw === undefined) {
    return { ok: false, code: "REQUIRED", message: "必填" };
  }
  if (typeof raw === "number") {
    if (!Number.isInteger(raw)) {
      return { ok: false, code: "NOT_INTEGER", message: "需为整数（不接受小数）" };
    }
    return rangeCheck(raw, min, max);
  }
  if (typeof raw !== "string") {
    return { ok: false, code: "NOT_INTEGER", message: "需为整数" };
  }
  const text = raw.trim();
  if (text === "") {
    return { ok: false, code: "REQUIRED", message: "必填" };
  }
  let hex: string | null = null;
  if (allowHex) {
    if (text.startsWith("0x") || text.startsWith("0X")) hex = text.slice(2);
    else if (/[hH]$/.test(text)) hex = text.slice(0, -1);
  }
  if (hex !== null) {
    if (hex === "" || hex.length > 4 || !/^[0-9a-fA-F]+$/.test(hex)) {
      return {
        ok: false,
        code: "BAD_HEX",
        message: "十六进制格式非法（如 0x0400 或 0400h，最多 4 位）",
      };
    }
    return rangeCheck(parseInt(hex, 16), min, max);
  }
  if (!/^[0-9]+$/.test(text)) {
    return {
      ok: false,
      code: "NOT_INTEGER",
      message: allowHex
        ? "需为十进制或十六进制整数（如 1024 或 0x0400）"
        : "需为十进制整数",
    };
  }
  return rangeCheck(parseInt(text, 10), min, max);
}

function rangeCheck(value: number, min: number, max: number): ParsedInt {
  if (Number.isNaN(value)) {
    return { ok: false, code: "NOT_INTEGER", message: "需为整数" };
  }
  if (value < min || value > max) {
    return {
      ok: false,
      code: "OUT_OF_RANGE",
      message: `需在 ${min}..=${max} 之间`,
    };
  }
  return { ok: true, value };
}

// ---------------------------------------------------------------------------
// raw draft → canonical 请求快照
// ---------------------------------------------------------------------------

export interface NormalizeResult {
  /** 可直接提交给 REST 的 connection 对象（TSAP 已归一为 number） */
  connection: ConnectionValues;
  issues: Issue[];
}

/**
 * 从**当前 raw 值**归一化出 canonical connection。
 *
 * 关键点：TSAP 从 raw 文本现场解析，因此「用户刚敲完还没失焦就点测试/保存」
 * 也会提交最新值（A05）。非法值产生 issue 且**不写入** canonical 对象，
 * 由调用方据此阻止提交。
 */
export function normalizeConnection(
  raw: ConnectionValues,
  fields: FieldDescriptor[],
): NormalizeResult {
  const connection: ConnectionValues = {};
  const issues: Issue[] = [];

  for (const field of fields) {
    const path = `connection.${field.key}`;
    const hasRaw = Object.prototype.hasOwnProperty.call(raw, field.key);
    const value = hasRaw ? raw[field.key] : field.default;

    if (value === null || value === undefined || value === "") {
      if (field.required) {
        // 没有 default 且未填 → 必须报必填，绝不自动填充
        issues.push({ path, code: "REQUIRED", message: `缺少必填字段 ${field.label ?? field.key}` });
      }
      continue;
    }

    const validation = field.validation ?? {};
    const enumOptions = validation.enum_options ?? null;

    if (enumOptions && enumOptions.length > 0) {
      if (typeof value !== "string" || !enumOptions.includes(value)) {
        issues.push({
          path,
          code: "BAD_ENUM",
          message: `取值非法，需为 ${enumOptions.join(" / ")} 之一`,
        });
        continue;
      }
      connection[field.key] = value;
      continue;
    }

    if (field.field_type === "Boolean") {
      if (typeof value !== "boolean") {
        issues.push({ path, code: "NOT_BOOLEAN", message: "需为布尔值" });
        continue;
      }
      connection[field.key] = value;
      continue;
    }

    if (isTsapKey(field.key) || isIntegerField(field.field_type)) {
      const parsed = parseIntInput(value, {
        allowHex: isTsapKey(field.key),
        min: validation.min ?? 0,
        max: validation.max ?? 65535,
      });
      if (!parsed.ok) {
        issues.push({ path, code: parsed.code, message: parsed.message });
        continue;
      }
      connection[field.key] = parsed.value;
      continue;
    }

    if (field.field_type === "Secret") {
      // Secret 原样透传（marker 或明文由后端处理），前端不做类型改写
      connection[field.key] = value;
      continue;
    }

    connection[field.key] = value;
  }

  return { connection, issues };
}

function isIntegerField(fieldType: string | undefined): boolean {
  return fieldType === "Integer" || fieldType === "Port" || fieldType === "Duration";
}

// ---------------------------------------------------------------------------
// 字段分组（基础 / 高级）与「隐藏必填」定位
// ---------------------------------------------------------------------------

export interface GroupedFields {
  basic: FieldDescriptor[];
  advanced: FieldDescriptor[];
}

function byOrderThenLabel(a: FieldDescriptor, b: FieldDescriptor): number {
  const oa = a.ui?.order ?? 0;
  const ob = b.ui?.order ?? 0;
  if (oa !== ob) return oa - ob;
  return (a.label ?? a.key).localeCompare(b.label ?? b.key);
}

/** 按 Descriptor 的 `ui.advanced` / `ui.order` 分组；不在组件里硬编码字段模板。 */
export function groupFields(fields: FieldDescriptor[]): GroupedFields {
  const basic: FieldDescriptor[] = [];
  const advanced: FieldDescriptor[] = [];
  for (const f of fields) {
    if (f.ui?.advanced === true) advanced.push(f);
    else basic.push(f);
  }
  basic.sort(byOrderThenLabel);
  advanced.sort(byOrderThenLabel);
  return { basic, advanced };
}

/**
 * 找出「高级区里仍缺失的必填字段」——用于在点测试/保存时**自动展开并定位**，
 * 而不是静默落到后端 400（A04）。
 */
export function missingRequiredAdvanced(
  fields: FieldDescriptor[],
  raw: ConnectionValues,
): string[] {
  return fields
    .filter((f) => f.ui?.advanced === true && f.required === true)
    .filter((f) => {
      const hasRaw = Object.prototype.hasOwnProperty.call(raw, f.key);
      const value = hasRaw ? raw[f.key] : f.default;
      return value === null || value === undefined || value === "";
    })
    .map((f) => f.key);
}

// ---------------------------------------------------------------------------
// 草稿状态与代际（revision）
// ---------------------------------------------------------------------------

export interface Draft {
  /** 原始输入值（TSAP 可能是用户正在输入的字符串） */
  raw: ConnectionValues;
  /** 每次有效改动 +1；用于丢弃迟到的异步结果 */
  revision: number;
}

export interface SetupState {
  draft: Draft;
  driverId: string | null;
  /** 上一次校验结果；参数一变即置空（不得复用旧结果） */
  lastValidatedRevision: number | null;
  /** 上一次探测结果所属的 revision */
  probeRevision: number | null;
}

export function initialSetupState(): SetupState {
  return {
    draft: { raw: {}, revision: 0 },
    driverId: null,
    lastValidatedRevision: null,
    probeRevision: null,
  };
}

/** 字段变更：revision+1，并让既有校验/探测结果失效。 */
export function withFieldChange(
  state: SetupState,
  key: string,
  value: unknown,
): SetupState {
  return {
    ...state,
    draft: {
      raw: { ...state.draft.raw, [key]: value },
      revision: state.draft.revision + 1,
    },
    lastValidatedRevision: null,
    probeRevision: null,
  };
}

/**
 * 切换驱动：**按新 Schema 重建参数**，不保留上一个驱动的字段值。
 *
 * 为什么必须重建而不是合并：不同驱动的字段名/类型/默认值都不同，合并会
 * 产生「新驱动上残留旧驱动参数」甚至把旧值当新驱动的有效配置提交
 * （V1.1 §8.3 与 A01/A02）。
 */
export function withDriverChange(
  state: SetupState,
  driverId: string,
  fields: FieldDescriptor[],
): SetupState {
  const raw: ConnectionValues = {};
  for (const f of fields) {
    if (f.default !== undefined && f.default !== null) raw[f.key] = f.default;
  }
  return {
    draft: { raw, revision: state.draft.revision + 1 },
    driverId,
    lastValidatedRevision: null,
    probeRevision: null,
  };
}

/** 结果是否仍然对应当前草稿（迟到响应必须据此丢弃）。 */
export function isResultCurrent(
  state: SetupState,
  result: { driverId: string; revision: number },
): boolean {
  return (
    state.driverId === result.driverId && state.draft.revision === result.revision
  );
}

// ---------------------------------------------------------------------------
// 探测结果分类（HTTP 状态 + 响应体 → 结构化 outcome）
// ---------------------------------------------------------------------------

export type ProbeOutcome =
  | { kind: "reachable_identified"; device: DeviceIdentity; warnings: string[] }
  | { kind: "reachable_unidentified"; warnings: string[] }
  | { kind: "unreachable"; detail: string; warnings: string[] }
  | { kind: "invalid"; detail: string; issues: string[] }
  | { kind: "not_found"; detail: string }
  | { kind: "unsupported"; detail: string }
  | { kind: "infrastructure"; detail: string }
  | { kind: "timeout"; detail: string }
  | { kind: "network"; detail: string };

export interface DeviceIdentity {
  vendor?: string | null;
  family?: string | null;
  model?: string | null;
  firmware?: string | null;
}

/** 把任意后端字段安全地转成字符串——**绝不**返回对象给渲染层。 */
export function safeText(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) {
    return value.map(safeText).filter((s) => s !== "").join("；");
  }
  if (typeof value === "object") {
    const obj = value as Record<string, unknown>;
    // 结构化错误体：只取已知的 code/message，其余不渲染
    const code = safeText(obj.code);
    const message = safeText(obj.message);
    const parts = [code, message].filter((s) => s !== "");
    if (parts.length > 0) return parts.join(": ");
    try {
      return JSON.stringify(obj);
    } catch {
      return "[无法显示的错误详情]";
    }
  }
  return String(value);
}

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map(safeText).filter((s) => s !== "");
}

function errorDetail(body: unknown): string {
  if (body && typeof body === "object") {
    const err = (body as Record<string, unknown>).error;
    if (err !== undefined) return safeText(err);
  }
  return safeText(body);
}

/**
 * HTTP 状态 + 响应体 → 结构化 outcome。
 *
 * 语义纪律（V1.1 §8.2）：`reachable` 只是「该驱动的探测成功」，
 * 不表示设备身份已确认；501 是「驱动不支持探测」而不是网络故障；
 * 503 是基础设施失败，**不得**显示成 CNC 不可达。
 */
export function classifyProbeOutcome(
  status: number,
  body: unknown,
): ProbeOutcome {
  const b = (body ?? {}) as Record<string, unknown>;
  switch (status) {
    case 200: {
      const warnings = stringList(b.warnings);
      if (b.reachable === true) {
        const device = (b.device ?? {}) as Record<string, unknown>;
        const identity: DeviceIdentity = {
          vendor: (device.vendor ?? null) as string | null,
          family: (device.family ?? null) as string | null,
          model: (device.model ?? null) as string | null,
          firmware: (device.firmware ?? null) as string | null,
        };
        const identified =
          identity.vendor !== null ||
          identity.family !== null ||
          identity.model !== null;
        return identified
          ? { kind: "reachable_identified", device: identity, warnings }
          : { kind: "reachable_unidentified", warnings };
      }
      return {
        kind: "unreachable",
        detail: errorDetail(body) || "探测完成，但目标设备不可达",
        warnings,
      };
    }
    case 400:
      return {
        kind: "invalid",
        detail: errorDetail(body) || "连接参数无效",
        issues: stringList(b.issues),
      };
    case 404:
      return { kind: "not_found", detail: errorDetail(body) || "驱动不存在" };
    case 501:
      return {
        kind: "unsupported",
        detail: errorDetail(body) || "该驱动不支持此探测方式",
      };
    case 503:
      return {
        kind: "infrastructure",
        detail: errorDetail(body) || "驱动或探测服务不可用",
      };
    case 504:
      return { kind: "timeout", detail: errorDetail(body) || "探测请求超时" };
    default:
      return {
        kind: "network",
        detail: errorDetail(body) || `未预期的响应（HTTP ${status}）`,
      };
  }
}

/** 浏览器网络层失败（fetch reject / 无响应）——与设备不可达区分。 */
export function networkFailureOutcome(reason: unknown): ProbeOutcome {
  return {
    kind: "network",
    detail: safeText(reason) || "Mesa API 不可用或浏览器网络异常",
  };
}

/**
 * outcome → 可安全渲染的字符串。
 *
 * 纪律：**不得**根据单个 `reachable` 布尔值宣称"连接成功"；也不得把
 * 400/501/503/504 统一说成 CNC 不可达（V1.1 §8.2「严格禁止」）。
 */
export function describeOutcome(outcome: ProbeOutcome): string {
  switch (outcome.kind) {
    case "reachable_identified":
      return "协议通信可达；设备返回身份信息";
    case "reachable_unidentified":
      return "协议通信可达；设备身份未确认";
    case "unreachable":
      return `探测完成，但目标设备不可达：${outcome.detail}`;
    case "invalid":
      return `连接参数无效：${outcome.detail}`;
    case "not_found":
      return `驱动不存在：${outcome.detail}`;
    case "unsupported":
      return `该驱动不支持此探测方式：${outcome.detail}`;
    case "infrastructure":
      return `驱动或探测服务不可用：${outcome.detail}`;
    case "timeout":
      return `探测超时：${outcome.detail}`;
    case "network":
      return `无法访问 Mesa API：${outcome.detail}`;
  }
}

/** 供 UI 决定色调：只有"可达"类才是中性/正向，其余一律不得显示为绿色成功。 */
export function outcomeTone(outcome: ProbeOutcome): "neutral" | "positive" | "warning" | "error" {
  switch (outcome.kind) {
    case "reachable_identified":
      return "positive";
    case "reachable_unidentified":
      return "neutral";
    case "unreachable":
    case "invalid":
      return "warning";
    case "not_found":
    case "unsupported":
    case "infrastructure":
    case "timeout":
    case "network":
      return "error";
  }
}
