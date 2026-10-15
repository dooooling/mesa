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
  /** 条件可见：与 `DescriptorFields` 的渲染判定同源（同一套语义，不得两处各写一份） */
  visible_if?: VisibilityCondition | null;
}

export interface VisibilityCondition {
  field: string;
  op: "eq" | "neq" | "in";
  value: unknown;
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
// 字段类型判定
//
// 事实来源：`crates/core-types/src/schema.rs` 的 `FieldType` 用
// `#[serde(rename_all = "lowercase")]`，因此 **wire 形态是小写**
// （`integer` / `port` / `boolean` / `secret` / `duration` …），
// `apps/mesa-web/src/types.ts` 与之对齐。
// 这里统一按小写比较并容忍历史 PascalCase 写法，避免「模型认不出真实
// Descriptor 的字段类型 → 跳过范围校验 / 把 Secret 当普通字符串」。
// ---------------------------------------------------------------------------

/**
 * 是否整数类字段（需要按 min/max 做范围校验）。
 *
 * `duration` **刻意包含在内**：见 [`isDurationField`] 的契约核查——具体 Driver
 * 的毫秒参数一律用 `as_u64()` 读取，小数必被拒。三者在此同语义，不再区分。
 */
export function isIntegerField(fieldType: string | undefined): boolean {
  const t = String(fieldType ?? "").toLowerCase();
  return t === "integer" || t === "port" || t === "duration";
}

/**
 * 是否 `duration` 字段（毫秒/时长）。
 *
 * **三方契约核查结论（本轮实测，以代码为准）**：
 * 1. Core Schema（`crates/core-types/src/schema.rs::field_type_matches`）：
 *    `Number | Duration => val.is_number()` → **允许小数**；内禀范围仅 `>= 0`，**无上界**。
 * 2. 具体 Driver Parser（`timeout_ms` 在 s7 / opcua / sinumerik-nck / focas2 中
 *    是唯一的 Duration 字段）：一律 `as_u64()`，**要求非负整数**，小数报
 *    `timeout_ms 必须为非负整数`（BAD_CONFIG）。
 * 3. 前端此前自相矛盾：`DescriptorFields` 用允许小数的 InputNumber 渲染，
 *    而 `normalizeConnection` 又按整数字段校验 → 用户能输入 5000.5，提交时才被拒。
 *
 * **取舍：以 Driver Parser 为准（duration 与 integer/port 同语义，要求整数）。**
 * 理由：Driver 是该值的最终消费者，放行小数必然在 OpenConnection/Probe 阶段失败；
 * 前端提前给出明确错误优于把后端 BAD_CONFIG 抛给用户（fail-closed）。同时渲染层
 * 改为整数输入，消除「界面允许、模型拒绝」的自相矛盾。
 *
 * NOTE: Core Schema 允许 `Duration` 为 JSON number 与 Driver 要求整数的不一致属
 * Rust 侧问题，本轮不修改 `crates/`、`drivers/`。若后端后续统一（例如 Duration
 * 也要求整数，或 Driver 改读 `as_f64`），前端只需放宽本判定与 `numericBounds`。
 */
export function isDurationField(fieldType: string | undefined): boolean {
  return String(fieldType ?? "").toLowerCase() === "duration";
}

/** 异步资源加载状态（驱动清单 / 描述符共用）。 */
export type LoadStatus = "idle" | "loading" | "ready" | "error";

/** 是否布尔字段。 */
export function isBooleanField(fieldType: string | undefined): boolean {
  return String(fieldType ?? "").toLowerCase() === "boolean";
}

/** 是否 Secret 字段（凭据）。 */
export function isSecretField(fieldType: string | undefined): boolean {
  return String(fieldType ?? "").toLowerCase() === "secret";
}

/**
 * Secret 的 **marker 形态**：后端持有旧凭据但不回显明文。
 *
 * `{secret_set:true}` = 已存有凭据；`{clear_secret:true}` = 已标记清除。
 * 两者都不是密码，**绝不能**当作连接参数发给 probe / validate-connection
 * （会把 marker 当口令去连设备，并把失败误报成「设备不可达」）。
 */
export function isSecretMarker(value: unknown): boolean {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const obj = value as Record<string, unknown>;
  return obj.secret_set === true || obj.clear_secret === true;
}

/**
 * 递归探测 marker 形状（对象/数组，深度受限）。
 *
 * 为什么按**形状**而不是按字段名/字段类型判断：凭据字段的命名会演进
 * （`password` / `api_token` / `credentials` …），靠白名单必然漏；只要值里
 * 出现 marker 形状，就说明它**不是可用的明文**，不能进 Probe 请求。
 */
export function containsSecretMarker(value: unknown, depth = 0): boolean {
  if (depth > 4) return false;
  if (isSecretMarker(value)) return true;
  if (Array.isArray(value)) {
    return value.some((item) => containsSecretMarker(item, depth + 1));
  }
  if (value !== null && typeof value === "object") {
    return Object.values(value as Record<string, unknown>).some((item) =>
      containsSecretMarker(item, depth + 1),
    );
  }
  return false;
}

/**
 * 不能进入 probe / validate-connection 请求的字段。
 *
 * 判定规则（形状优先，与字段名无关）：
 * 1. 值里（含嵌套）出现 marker 形状 → 命中；
 * 2. 声明为 Secret 类型的字段出现任何对象值 → 一律命中（对象不可能是密码，
 *    保守阻断优于把未知结构当明文发出去）。
 *
 * 注意：这些字段**可以保存**——marker 原样回传是「不更新凭据」的正确语义，
 * 限制只针对 Driver-bound 的 Probe/Validate（只接受明文）。编辑态想「用已保存
 * 凭据测试」必须走 Endpoint-bound 的 `connection-check`，那条路径用
 * [`unsupportedCheckMarkerKeys`] 判定（规范 marker 合法）。
 */
export function secretMarkerKeys(
  fields: FieldDescriptor[],
  raw: ConnectionValues,
): string[] {
  const declared = new Set(
    fields.filter((f) => isSecretField(f.field_type)).map((f) => f.key),
  );
  const keys: string[] = [];
  for (const [key, value] of Object.entries(raw)) {
    if (containsSecretMarker(value)) {
      keys.push(key);
      continue;
    }
    if (declared.has(key) && value !== null && typeof value === "object") {
      keys.push(key);
    }
  }
  return keys;
}

// ---------------------------------------------------------------------------
// C3-C：Endpoint-bound 检查的 Secret 语义（marker 白名单 + 明确清除）
// ---------------------------------------------------------------------------

/**
 * Secret 的**规范 marker**：只允许 `{secret_set:true}` 与 `{clear_secret:true}`
 * 这两种「单键、值为 true」的形状。
 *
 * 为什么必须严格（与 Core `classify_secret_value` 同口径）：端点检查会把 marker
 * 交给服务端物化，任何「长得像 marker 但不是」的值（`{secret_set:"yes"}`、
 * 多键对象、裸对象）都会被服务端 400 `INVALID_SECRET_MARKER`；前端先拦一次是为了
 * 把「字段写错了」与「设备连不上」区分开——后者才是探测结论。
 */
export function isCanonicalSecretMarker(value: unknown): boolean {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const obj = value as Record<string, unknown>;
  if (Object.keys(obj).length !== 1) return false;
  return obj.secret_set === true || obj.clear_secret === true;
}

/**
 * Endpoint-bound 检查**不能接受**的 marker 位置/形状（返回字段名，空数组即可发请求）。
 *
 * 与 [`secretMarkerKeys`] 的分工（这是 C3-C 的关键区别）：
 * - `secretMarkerKeys`：Driver-bound 的 `validate-connection` / `probe` 只接受明文，
 *   因此**任何** marker 都必须拦下（marker 当密码发出去既连不上、又把「凭据未回显」
 *   误报成设备不可达）；
 * - 本函数：Endpoint-bound 的 `connection-check` **正是**为「复用已保存凭据测试」
 *   而存在，规范 marker 是**合法**输入，只拦两类必然 400 的形态：
 *   ① 非 Secret 声明字段上的 marker 形状（位置红线）；
 *   ② Secret 声明字段上的非规范取值（形状红线：多键对象、裸对象、数组/数字/布尔）。
 *   空值（`null`/`undefined`/`""`）不算问题：清除意图由 `applySecretClears` 表达成
 *   `{"clear_secret":true}`，而不是靠「字段缺席」（缺席会被服务端读成「继承旧值」）。
 */
export function unsupportedCheckMarkerKeys(
  fields: FieldDescriptor[],
  raw: ConnectionValues,
): string[] {
  const declared = new Set(
    fields.filter((f) => isSecretField(f.field_type)).map((f) => f.key),
  );
  const keys: string[] = [];
  for (const [key, value] of Object.entries(raw)) {
    if (declared.has(key)) {
      if (typeof value === "string") continue; // 用户输入的明文
      if (isCanonicalSecretMarker(value)) continue; // 已保存凭据 / 明确清除
      if (isEmptyConnectionValue(value)) continue; // 空值 → 交给 applySecretClears
      keys.push(key);
      continue;
    }
    // 非 Secret 字段：任何 marker 形状都是「放错位置」，服务端必定 400
    if (containsSecretMarker(value)) keys.push(key);
  }
  return keys;
}

/**
 * `touched + 空 Secret` → `{"clear_secret":true}`（C3 §10.2/§10.3）。
 *
 * 为什么必须显式表达：C2 的归一化会**丢弃空值**（`null`/`undefined`/`""` 不进入
 * canonical），而服务端把「字段缺席」定义为「按 PUT 契约继承旧值」。于是「用户把
 * 密码清空」会表现为 key 缺席 → 服务端拿**旧密码**去检查/保存，正好命中契约
 * §2 明令禁止的「偷偷回退到旧密码」。
 *
 * 判定范围（严格限定，不得扩大）：
 * - **只**处理 Descriptor 声明为 Secret（`FieldType::Secret`）的字段；
 * - **只**在 `touched` 命中且当前值为空时转换；
 * - **未触碰**的空值（程序初始化 / 快照装载物化出来的空）不转换——与 C2 的
 *   `preserveHiddenFields` 同一口径；
 * - 非 Secret 字段的空值规则完全不变（走「整体替换 + `required` 裁决」）。
 *
 * 与 `required` 的关系：必填 Secret 被清空时 `normalizeConnection` 仍会报
 * `REQUIRED`（可见）或 `clearedHiddenRequiredIssues` 报 `REQUIRED_CLEARED`（隐藏），
 * 本地即阻断提交——「必填凭据不能靠清空绕过」。清除标记只是把意图如实表达给服务端，
 * 不会让必填缺失变成合法配置。
 *
 * 调用点：`useConnectionSetup.normalizeDraft()`，因此「测试/校验」与「正式保存」
 * **两个出口共用同一份语义**（只修一个出口会让「测试说不需要密码」与「保存后旧密码
 * 还在」自相矛盾）。
 */
export function applySecretClears(
  connection: ConnectionValues,
  fields: FieldDescriptor[],
  raw: ConnectionValues,
  touched: readonly string[],
): { connection: ConnectionValues; clearedKeys: string[] } {
  const touchedSet = new Set(touched);
  const out: ConnectionValues = { ...connection };
  const clearedKeys: string[] = [];
  for (const field of fields) {
    if (!isSecretField(field.field_type)) continue;
    if (!touchedSet.has(field.key)) continue;
    const hasRaw = Object.prototype.hasOwnProperty.call(raw, field.key);
    const value = hasRaw ? raw[field.key] : undefined;
    if (!isEmptyConnectionValue(value)) continue;
    out[field.key] = { clear_secret: true };
    clearedKeys.push(field.key);
  }
  return { connection: out, clearedKeys };
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

/**
 * 字段当前是否可见（`ui.visible_if`）。
 *
 * 为什么模型必须自己判一次：`normalizeConnection` 决定「提交什么」，
 * `missingRequiredAdvanced` 决定「是否阻断提交」。若不看可见性，一个
 * 条件不满足而**根本不会渲染**的必填字段会永远报「缺少必填」，让用户
 * 无法提交；同时它的默认值还会被塞进请求体。判定规则与
 * `components/DescriptorFields.tsx` 的 `isVisible` 保持一致。
 */
export function isFieldVisible(field: FieldDescriptor, raw: ConnectionValues): boolean {
  const cond = field.ui?.visible_if;
  if (!cond) return true;
  const current = raw[cond.field];
  if (cond.op === "eq") return current === cond.value;
  if (cond.op === "neq") return current !== cond.value;
  if (cond.op === "in") {
    return Array.isArray(cond.value) && (cond.value as unknown[]).includes(current);
  }
  return true;
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
    // 条件不可见的字段既不提交也不报必填（用户看不到它，无法对它负责）
    if (!isFieldVisible(field, raw)) continue;
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

    if (isBooleanField(field.field_type)) {
      if (typeof value !== "boolean") {
        issues.push({ path, code: "NOT_BOOLEAN", message: "需为布尔值" });
        continue;
      }
      connection[field.key] = value;
      continue;
    }

    if (isTsapKey(field.key) || isIntegerField(field.field_type)) {
      const parsed = parseIntInput(value, numericBounds(field));
      if (!parsed.ok) {
        issues.push({ path, code: parsed.code, message: parsed.message });
        continue;
      }
      connection[field.key] = parsed.value;
      continue;
    }

    if (isSecretField(field.field_type)) {
      // Secret 原样透传（marker 或明文由后端处理），前端不做类型改写。
      // 注意：marker 可以保存，但**不得**用于 probe/validate（见 secretMarkerKeys）。
      connection[field.key] = value;
      continue;
    }

    connection[field.key] = value;
  }

  return { connection, issues };
}

/**
 * 数值字段的解析边界（allowHex / min / max）。
 *
 * 为什么不能一律用 65535 兜底：`Duration` 在 Core Schema 里**没有 16 位上界**
 * （内禀范围只有 `>= 0`），用端口/整数的 65535 兜底会把合法时长误判为越界，
 * 形成「前端拒绝、后端接受」的矛盾。因此按字段类型分别取默认边界：
 * - TSAP：16 位协议字段，允许十六进制；
 * - Port / Integer：沿用 65535 兜底（既有行为，不改）；
 * - Duration：默认只受下界 0 约束，上界**只在 Descriptor 显式声明时**生效。
 */
function numericBounds(field: FieldDescriptor): {
  allowHex: boolean;
  min: number;
  max: number;
} {
  const validation = field.validation ?? {};
  const allowHex = isTsapKey(field.key);
  if (isDurationField(field.field_type) && !allowHex) {
    return {
      allowHex,
      min: validation.min ?? 0,
      max: validation.max ?? Number.POSITIVE_INFINITY,
    };
  }
  return {
    allowHex,
    min: validation.min ?? 0,
    max: validation.max ?? 65535,
  };
}

// ---------------------------------------------------------------------------
// 提交前置门（P0）：三个入口共用**唯一**判定
// ---------------------------------------------------------------------------

/**
 * 提交被挡下的原因；`null` 表示可提交。
 *
 * 为什么必须唯一：P0 要求「驱动清单失败 / 无可用驱动 / 描述符失败时，所有提交
 * 路径都必须失效」。若按钮的 `disabled` 与提交函数里的守卫各写一套判定，两者
 * 迟早漂移（回归出「禁用被绕过」）。这里让「按钮 disabled」与「提交函数首行守卫」
 * 读同一个纯函数，任何一侧改动都会同时影响另一侧。
 */
export type SubmitBlockReason =
  | "inactive"
  | "catalog-unavailable"
  | "no-usable-driver"
  | "no-driver"
  | "descriptor-unavailable";

export interface SubmitGateInput {
  /** 面板是否可见（弹窗关闭 / 非当前步骤 → 不发任何请求） */
  active: boolean;
  /**
   * 驱动是否允许用户切换。edit 模式为 false：既不需要驱动清单
   * （`driver_id` 由服务端快照给定），也不能用清单失败去否决编辑保存。
   */
  driverSelectable: boolean;
  driversStatus: LoadStatus;
  /** 清单中 `launchable === true` 的驱动 id（其余一律不可用） */
  usableDriverIds: string[];
  driverId: string | null;
  descriptorStatus: LoadStatus;
}

/**
 * 提交前置门判定。**不修改任何状态**，因此可完整单测。
 *
 * 判定顺序即「先决条件链」：面板可见 → 清单可用 → 驱动可用 → 描述符可用。
 * 任一步不满足都返回具体原因，调用方据此禁用按钮并（作为第二道防线）在提交
 * 函数首行直接 return。
 */
export function submitBlockReason(input: SubmitGateInput): SubmitBlockReason | null {
  if (!input.active) return "inactive";
  if (input.driverSelectable) {
    if (input.driversStatus !== "ready") return "catalog-unavailable";
    if (!input.driverId || !input.usableDriverIds.includes(input.driverId)) {
      return "no-usable-driver";
    }
  }
  if (!input.driverId) return "no-driver";
  if (input.descriptorStatus !== "ready") return "descriptor-unavailable";
  return null;
}

// ---------------------------------------------------------------------------
// 编辑态补齐：`visible_if` 隐藏字段不得丢失（P1）
// ---------------------------------------------------------------------------

/** 视为「空」的取值：与 `cleanConnection` 剔除的形态保持一致（undefined/null/""）。 */
export function isEmptyConnectionValue(value: unknown): boolean {
  return value === undefined || value === null || value === "";
}

/**
 * `preserveHiddenFields` 的上下文（P1-2：区分「用户清除」与「程序初始化」）。
 */
export interface PreserveHiddenOptions {
  /**
   * 用户本次会话**显式改动过**的 key（含清空，见 [`Draft.touched`]）。
   *
   * 为什么必须由外部显式给出而不是从值推断：`raw` 里的空值有两种来源——
   * ①用户在字段可见时把它清空（= 删除意图）；②程序初始化/按新 Schema 物化
   * 默认值时写进去的空值（**不是**删除意图）。只看 `raw[key] === ""` 无法区分
   * 二者，会把 ② 误判成删除并静默丢掉服务端旧值。
   */
  touched?: readonly string[];
  /**
   * 服务端快照（编辑态 `initialValues`）：用于给「未触碰却为空」的隐藏字段
   * 保住旧值。缺省表示没有更权威的旧值来源，此时保留 `raw` 原样。
   */
  fallback?: ConnectionValues | null;
}

/**
 * 编辑态提交补齐：把「**当前 Descriptor 内**、本次因 `visible_if` 不满足而未被
 * 渲染、因而未进入 canonical」的字段，按原值补回提交负载。
 *
 * 为什么必须补：后端 `update_endpoint()` 用请求里的 connection **整体替换**旧
 * JSON，只有 Secret 字段有「缺失即保留」语义。编辑态若省略这些字段，它们会真的
 * 从服务端消失（不是理论风险，是已核对后端源码的结论）。
 *
 * **边界（评审要求：不得无脑整体合并旧 connection）**
 * - **保留**：key 属于当前 Descriptor，且 `visible_if` 当前不满足，且 `raw` 里有
 *   非空值 → 原样保留（未触碰即服务端原值；用户显式改过则以用户值为准）。
 * - **保留（不误删）**：`raw` 里是空值但**用户从未触碰**该字段时，空值来自程序
 *   初始化（默认物化/装载），**不构成清除**：若 `opts.fallback`（服务端快照）里
 *   还有该字段的旧值，一律以旧值补回；否则保留 `raw` 的空值原样（由下游
 *   `cleanConnection` 决定是否剔除），**绝不**在此把它当作删除。
 * - **删除**：用户**显式清除**（`opts.touched` 命中且当前为空）→ 不补回，该 key
 *   缺席提交负载，下游 `cleanConnection` 后即从服务端删除。这正是「用户明确
 *   清除优先」的语义。
 * - **丢弃**：`raw` 中**不属于当前 Descriptor** 的 key（已过期/未知字段）。它们
 *   既不在 canonical 里，也不会被本函数补回；Core 的 `UNKNOWN_FIELD` 同样拒绝，
 *   无脑整体合并会把废弃配置重新写回。
 * - **丢弃**：`visible_if` 满足但未进入 canonical 的字段——那是「可见字段校验
 *   失败/必填缺失」，必须由校验阻断提交，绝不能用旧值掩盖。
 * - **丢弃**：`raw` 中不存在的 key——不替 Descriptor 发明默认值（也不从
 *   `fallback` 里凭空补回草稿没有的字段）。
 *
 * Secret 不受影响：Secret 字段的值同样取自 `raw`，marker（`{secret_set:true}` /
 * `{clear_secret:true}`）原样透传，既有 marker 保留语义不变。
 */
export function preserveHiddenFields(
  connection: ConnectionValues,
  raw: ConnectionValues,
  fields: FieldDescriptor[],
  opts: PreserveHiddenOptions = {},
): ConnectionValues {
  const out: ConnectionValues = { ...connection };
  const touched = new Set(opts.touched ?? []);
  const fallback = opts.fallback ?? null;
  for (const field of fields) {
    const key = field.key;
    // 已进入 canonical（可见且校验通过）→ 不动
    if (Object.prototype.hasOwnProperty.call(out, key)) continue;
    // 可见却缺席 → 是校验问题，必须由校验阻断，不得用旧值掩盖
    if (isFieldVisible(field, raw)) continue;
    // 快照/草稿里本来就没有这个 key → 不凭空造值
    if (!Object.prototype.hasOwnProperty.call(raw, key)) continue;
    const value = raw[key];

    if (touched.has(key)) {
      // 用户本次显式改动过：以用户输入为准。清空即删除意图，
      // **不得**用旧值/快照兜底（否则「用户明确清除优先」被推翻）。
      if (isEmptyConnectionValue(value)) continue;
      out[key] = value;
      continue;
    }

    if (!isEmptyConnectionValue(value)) {
      out[key] = value;
      continue;
    }

    // 未触碰却为空：程序初始化得到的空值 ≠ 用户清除，不得据此删除旧值。
    const hasFallback =
      fallback !== null && Object.prototype.hasOwnProperty.call(fallback, key);
    const fallbackValue = hasFallback ? fallback[key] : undefined;
    if (hasFallback && !isEmptyConnectionValue(fallbackValue)) {
      out[key] = fallbackValue;
      continue;
    }
    if (value !== undefined) out[key] = value;
  }
  return out;
}

/**
 * 「必填字段被用户显式清空，且当前因 `visible_if` 不满足而隐藏」→ 本地校验失败。
 *
 * 为什么必须拦而不是静默删除：Core 的 `validate_instance` **完全不看
 * `visible_if`**（见 `crates/core-types/src/schema.rs`），缺失的必填字段一律
 * `REQUIRED`（400）。若前端把用户清空的值从负载里剔除，用户看到的是「点了保存
 * 才莫名 400」，而清空这个动作本身从未被校验拒绝——属于静默删除。
 *
 * 判定只针对「用户显式清除」（`touched` 命中且当前为空）：
 * 程序初始化得到的空值不是清除，因此**不**在这里报错（那属于
 * `preserveHiddenFields` 的「不得误删」分支）。
 * 字段可见时由 `normalizeConnection` 报常规 `REQUIRED`，此处不重复报。
 */
export function clearedHiddenRequiredIssues(
  fields: FieldDescriptor[],
  raw: ConnectionValues,
  touched: readonly string[],
): Issue[] {
  const touchedSet = new Set(touched);
  const issues: Issue[] = [];
  for (const field of fields) {
    if (field.required !== true) continue;
    if (isFieldVisible(field, raw)) continue;
    if (!touchedSet.has(field.key)) continue;
    const hasRaw = Object.prototype.hasOwnProperty.call(raw, field.key);
    const value = hasRaw ? raw[field.key] : undefined;
    if (!isEmptyConnectionValue(value)) continue;
    issues.push({
      path: `connection.${field.key}`,
      code: "REQUIRED_CLEARED",
      message: `必填字段 ${field.label ?? field.key} 在可见时被清空；条件隐藏不免除必填（Core 校验不看 visible_if），已阻止提交，不会静默删除该字段`,
    });
  }
  return issues;
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
    .filter((f) => isFieldVisible(f, raw))
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
  /**
   * 用户本次会话**显式改动过**的 key（含把字段清空）。
   *
   * 存在的唯一理由（P1-2）：`raw` 里的空值有两种来源——用户清除，或程序初始化
   * （快照装载 / 按新 Schema 物化默认值）。只凭「值是空的」无法区分删除意图与
   * 「本来就没有值」，因此把「用户动过」这件事显式记录在状态里。程序侧写入
   * （`withDriverChange`、会话复位、快照装载）一律**不**登记为 touched。
   */
  touched: string[];
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
    draft: { raw: {}, revision: 0, touched: [] },
    driverId: null,
    lastValidatedRevision: null,
    probeRevision: null,
  };
}

/**
 * 从「整份 raw 替换」里推导用户实际改动过的 key。
 *
 * 为什么需要：`DescriptorFields` 的受控语义是「浅合并后的完整对象」回调
 * （见 `useConnectionSetup.setRaw`），无法逐字段得知用户动的是哪一个；但按值
 * 比较可以精确得到**值真的变了**的 key（含被清空、被新增的 key），而重新选中
 * 同一个值不会产生 touched——这正是 [`Draft.touched`] 想要的语义。
 */
export function touchedKeys(
  prev: ConnectionValues,
  next: ConnectionValues,
): string[] {
  const out: string[] = [];
  for (const key of Object.keys(prev)) {
    if (prev[key] !== next[key]) out.push(key);
  }
  for (const key of Object.keys(next)) {
    if (Object.prototype.hasOwnProperty.call(prev, key)) continue;
    if (next[key] !== undefined) out.push(key);
  }
  return out;
}

/** 字段变更：revision+1，登记为「用户显式改动」，并让既有校验/探测结果失效。 */
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
      touched: state.draft.touched.includes(key)
        ? state.draft.touched
        : [...state.draft.touched, key],
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
 *
 * 注意 `touched` 一并清空：这里的值是**程序物化的默认值**，不是用户输入，
 * 因此不得登记为用户改动（否则「用户明确清除」会被凭空构造出来）。
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
    draft: { raw, revision: state.draft.revision + 1, touched: [] },
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
  | { kind: "network"; detail: string }
  /**
   * 未做探测：连接里存在**未重新输入明文**的 Secret marker。
   *
   * 这不是「设备不可达」，也不是「参数非法」，而是「本次测试被安全边界拦下」。
   * 单独成类是为了避免把安全边界伪装成网络/设备结论（V1.1 §9.4）。
   */
  | { kind: "secret_unavailable"; detail: string };

export interface DeviceIdentity {
  vendor?: string | null;
  family?: string | null;
  model?: string | null;
  firmware?: string | null;
}

/**
 * 结构化 issue 的 `path` 前缀去重（P2）。
 *
 * 后端/本地 issue 的 `message` 有时**自带同一个 path 前缀**（如
 * `path: "connection.host"` + `message: "connection.host 缺少必填"`）；渲染层
 * 又会单独展示 path，拼起来就成了「connection.host：connection.host 缺少必填」。
 *
 * 去重规则（**向后兼容**，只在能确定是重复时才裁）：
 * - `message` 必须以**同一个** path 开头；
 * - 其后必须紧跟分隔符（空白 / 冒号 / 逗号 / 句号 / 分号 / 顿号 / 连字符）或直接结束，
 *   否则视为「只是恰好同前缀的另一个词」（如 `connection.hostname`）→ 原样保留。
 */
function stripDuplicatedPathPrefix(path: string, message: string): string {
  if (path === "" || message === "" || !message.startsWith(path)) return message;
  const rest = message.slice(path.length);
  if (rest === "") return rest;
  if (!/^[\s:：,，.。;；、-]/.test(rest)) return message;
  return rest.replace(/^[\s:：,，.。;；、-]+/, "");
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
    // 结构化错误体/字段 issue：取已知的 path/code/message，其余不渲染。
    // path 必须保留——它是「哪个字段出错」的唯一线索（丢了就只能靠后端原文猜）。
    const path = safeText(obj.path);
    const code = safeText(obj.code);
    const message = stripDuplicatedPathPrefix(path, safeText(obj.message));
    const parts = [path, code, message].filter((s) => s !== "");
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

/**
 * 把字段级 issue 渲染成一行可读文本：`path：message（code）`。
 *
 * 为什么要单独有这个函数（P2）：这是「path 与 message 分开渲染」的**唯一**位置，
 * 也正是重复前缀真正发生的地方——后端的 `message` 有时自带同一个 path 前缀
 * （如 `path: "connection.host"` + `message: "connection.host 缺少必填"`），
 * 直接拼接会渲染成「connection.host：connection.host 缺少必填」。
 * 这里复用 `stripDuplicatedPathPrefix` 做幂等去重，且**不改变**不含重复时的输出。
 */
export function describeIssue(issue: Issue): string {
  const path = safeText(issue.path);
  const code = safeText(issue.code);
  const message = stripDuplicatedPathPrefix(path, safeText(issue.message));
  const head = path !== "" && message !== "" ? `${path}：${message}` : path || message;
  return code !== "" ? `${head}（${code}）` : head;
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
 * 因 Secret marker 而**未发起**探测的结果。
 *
 * C2 复用既有的 `POST /drivers/{id}/probe`，它只接受明文连接参数；把
 * `{secret_set:true}` 当密码发出去既连不上、又会把「凭据未回显」误报成
 * 「设备不可达」。Endpoint-bound 的 Secret-safe `connection-check` 属 C3，
 * 因此这里显式表达「本场景暂不支持测试」。
 */
export function secretUnavailableOutcome(detail: string): ProbeOutcome {
  return { kind: "secret_unavailable", detail };
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
    case "secret_unavailable":
      return `未执行探测（已保存的凭据不会回显）：${outcome.detail}`;
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
    case "secret_unavailable":
      return "warning";
    case "not_found":
    case "unsupported":
    case "infrastructure":
    case "timeout":
    case "network":
      return "error";
  }
}
