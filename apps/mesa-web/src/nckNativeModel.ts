// PR-C 第三批：`nck_native` 高级模式的纯模型。
//
// 这里只实现**已冻结**的规则（A27 / A24–A26），不重实现 NCK 地址解析器：
// - `identity_mode="derived"` 的 Key 固定为
//   `native.<area>.<area_no>.<module>.<column>.<line>`（definition.rs:1163-1175）；
// - `identity_mode="user"` 的 Key 由用户显式给出：只校验 `native.` 命名空间与
//   通用语法（对齐 core-types point.rs:465-500），**绝不自动改名/追加后缀**；
// - 12 个原生字段来自 Descriptor 的 `variable` 资源；必需性/类型/范围按
//   SchemaDescriptor 通用校验，不引入协议常量。
import type { SchemaDescriptor } from "./types";
import { missingRequiredParams } from "./resourceSelectionModel";

export type NckNativeIdentityMode = "derived" | "user";

export interface NckNativeIssue {
  key?: string;
  code: string;
  message: string;
}

function asInteger(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) ? value : null;
}

/**
 * derived Key（A27）：恰为五个寻址字段，**不含** count/decode/transport_size/
 * invalid_*、unit。字段缺失或非整数即返回 null（调用方必须禁止提交）。
 */
export function nativeDerivedPointKey(
  parameters: Record<string, unknown> | null | undefined,
): string | null {
  if (!parameters) return null;
  const area = typeof parameters.area === "string" ? parameters.area : "";
  if (!/^[A-Z]+$/.test(area)) return null;
  const areaNo = asInteger(parameters.area_no);
  const module = asInteger(parameters.module);
  const column = asInteger(parameters.column);
  const line = asInteger(parameters.line);
  if (
    areaNo === null ||
    module === null ||
    column === null ||
    line === null ||
    areaNo < 0 ||
    module < 0 ||
    column < 0 ||
    line < 0
  ) {
    return null;
  }
  return `native.${area}.${areaNo}.${module}.${column}.${line}`;
}

export type NckNativeKeyCheck =
  | { ok: true; key: string }
  | { ok: false; code: "NATIVE_KEY_INVALID"; message: string };

/**
 * user Key 校验（A26）：`native.` + 字母 area + 四个无前导零十进制段；
 * 允许 Core 支持的末尾 `~<n>`（n>=2），但**不自动生成/不自动改名**（A25）。
 */
export function validateNativeUserPointKey(key: string): NckNativeKeyCheck {
  const raw = key;
  if (raw.trim() === "") {
    return { ok: false, code: "NATIVE_KEY_INVALID", message: "Native point_key 不能为空" };
  }
  if (raw !== raw.trim()) {
    return {
      ok: false,
      code: "NATIVE_KEY_INVALID",
      message: "Native point_key 不得包含首尾空白（身份必须保持原值）",
    };
  }
  if (new TextEncoder().encode(raw).length > 256) {
    return { ok: false, code: "NATIVE_KEY_INVALID", message: "Native point_key 超过 256 字节" };
  }
  if (!raw.startsWith("native.")) {
    return {
      ok: false,
      code: "NATIVE_KEY_INVALID",
      message: "user 模式 point_key 必须以 `native.` 开头（命名空间隔离）",
    };
  }
  let base = raw.slice("native.".length);
  // 末尾 ~<n> 后缀：只允许一次，n>=2（~1/~/~02 均非法）。
  const tilde = base.indexOf("~");
  if (tilde >= 0) {
    const suffix = base.slice(tilde + 1);
    base = base.slice(0, tilde);
    if (base.includes("~")) {
      return { ok: false, code: "NATIVE_KEY_INVALID", message: "`~` 只能作为末尾后缀出现一次" };
    }
    if (!/^[2-9]\d*$/.test(suffix)) {
      return {
        ok: false,
        code: "NATIVE_KEY_INVALID",
        message: "`~<n>` 后缀必须是不含前导零且 >=2 的整数",
      };
    }
  }
  const segments = base.split(".");
  if (segments.length !== 5) {
    return {
      ok: false,
      code: "NATIVE_KEY_INVALID",
      message: "Native key 必须恰为 native.<area>.<area_no>.<module>.<column>.<line>",
    };
  }
  const [area, ...numbers] = segments;
  if (!/^[A-Z]+$/.test(area)) {
    return { ok: false, code: "NATIVE_KEY_INVALID", message: "Native key 的 area 段必须是大写字母" };
  }
  for (const [idx, segment] of numbers.entries()) {
    if (!/^(0|[1-9]\d*)$/.test(segment)) {
      return {
        ok: false,
        code: "NATIVE_KEY_INVALID",
        message: `Native key 第 ${idx + 1} 个十进制段非法（禁止前导零/空段）`,
      };
    }
  }
  return { ok: true, key: raw };
}

/** 12 原生字段（不含 `identity_mode`）：用于 derived 预览/持久化参数。 */
export function nativeAddressParameters(
  values: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(values)) {
    if (key === "identity_mode") continue;
    if (value === undefined) continue;
    out[key] = value;
  }
  return out;
}

/**
 * 组装 native 点位信封。
 *
 * - derived：Key 由固定规则生成；参数只含 12 原生字段（不写 identity_mode）；
 * - user：Key 原样保留（不追加 `~<n>`、不改名）；参数追加
 *   `identity_mode:"user"` 供 Driver 识别。
 */
export function buildNativePointSelection(args: {
  values: Record<string, unknown>;
  identityMode: NckNativeIdentityMode;
  userKey?: string;
}):
  | { ok: true; point: { kind: "nck_native"; point_key: string; parameters: Record<string, unknown> } }
  | { ok: false; code: string; message: string } {
  const parameters = nativeAddressParameters(args.values);
  if (args.identityMode === "derived") {
    const key = nativeDerivedPointKey(parameters);
    if (!key) {
      return {
        ok: false,
        code: "NATIVE_DERIVED_KEY_UNAVAILABLE",
        message: "无法由原生字段生成 derived Key（area/area_no/module/column/line 必须完整且为整数）",
      };
    }
    return { ok: true, point: { kind: "nck_native", point_key: key, parameters } };
  }
  const checked = validateNativeUserPointKey(args.userKey ?? "");
  if (!checked.ok) return { ok: false, code: checked.code, message: checked.message };
  return {
    ok: true,
    point: {
      kind: "nck_native",
      // A25：身份保持用户原值，绝不自动改名/追加后缀。
      point_key: checked.key,
      parameters: { ...parameters, identity_mode: "user" },
    },
  };
}

/**
 * 通用 Schema 校验（非协议语义）：必填、integer/number 类型、min/max、enum。
 * 前端只做 Descriptor 已声明的约束，不解释 NCK 地址规则。
 */
export function validateNativeFields(
  schema: SchemaDescriptor,
  values: Record<string, unknown>,
): NckNativeIssue[] {
  const issues: NckNativeIssue[] = [];
  const missing = missingRequiredParams(schema, values);
  for (const key of missing) {
    issues.push({ key, code: "MISSING_NATIVE_FIELD", message: `缺少必填原生字段 \`${key}\`` });
  }
  for (const field of schema.fields) {
    const value = values[field.key];
    if (value === undefined || value === null) continue;
    switch (field.field_type) {
      case "integer":
        if (typeof value !== "number" || !Number.isInteger(value)) {
          issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 必须是整数` });
        } else {
          if (field.validation.min !== undefined && value < field.validation.min) {
            issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 小于下界` });
          }
          if (field.validation.max !== undefined && value > field.validation.max) {
            issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 超过上界` });
          }
        }
        break;
      case "number":
        if (typeof value !== "number" || !Number.isFinite(value)) {
          issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 必须是数字` });
        } else {
          if (field.validation.min !== undefined && value < field.validation.min) {
            issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 小于下界` });
          }
          if (field.validation.max !== undefined && value > field.validation.max) {
            issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 超过上界` });
          }
        }
        break;
      case "enum":
        if (typeof value !== "string" || !(field.validation.enum_options ?? []).includes(value)) {
          issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 不在候选集内` });
        }
        break;
      case "string":
        if (typeof value !== "string") {
          issues.push({ key: field.key, code: "INVALID_NATIVE_FIELD", message: `\`${field.key}\` 必须是字符串` });
        }
        break;
      default:
        break;
    }
  }
  return issues;
}

/** 校验已持久化 native 点位（重开编辑器/重新核验用），不做自动修复。 */
export function validateNativePoint(point: {
  point_key: string;
  parameters: Record<string, unknown>;
}): { ok: true } | { ok: false; code: string; message: string } {
  const mode = point.parameters.identity_mode;
  if (mode === "user") {
    const checked = validateNativeUserPointKey(point.point_key);
    return checked.ok ? { ok: true } : { ok: false, code: checked.code, message: checked.message };
  }
  if (mode !== undefined && mode !== "derived") {
    return {
      ok: false,
      code: "NATIVE_IDENTITY_MODE_INVALID",
      message: `未知 identity_mode=${JSON.stringify(mode)}（仅 derived/user）`,
    };
  }
  const expected = nativeDerivedPointKey(nativeAddressParameters(point.parameters));
  if (!expected) {
    return {
      ok: false,
      code: "NATIVE_DERIVED_KEY_UNAVAILABLE",
      message: "原生字段不完整，无法重算 derived Key",
    };
  }
  if (expected !== point.point_key) {
    return {
      ok: false,
      code: "NATIVE_KEY_MISMATCH",
      message: `记录 point_key="${point.point_key}" 与固定规则重算 "${expected}" 不一致`,
    };
  }
  return { ok: true };
}
