// 通用「字段标签 + 值」摘要（纯前端，不新增 summary_template 契约）。
//
// 来源与边界：
// - 标签只能来自既有事实：Descriptor 字段 `label`、NCK 定义目录的 argument
//   `label`，或字段 key 本身；**不得**按地址/参数猜测业务含义（沿用
//   nckPointDisplay 的硬约束）；
// - 不做表达式/脚本/模板语言：只按「标签 值」逐项拼接，键序稳定；
// - 不 import React/AntD：摘要文案要能逐条单测钉死。
//
// 为什么要有它：NCK 点位行早就能拼出「通道 1 / 轴 2」，但非 NCK 点位行仍直接
// `JSON.stringify(parameters)`——用户看到的是原始 JSON。同一类信息（参数表）
// 在两处用了两种表达，因此把「标签 + 值」的派生提成通用能力，NCK 与通用资源
// 共用同一条实现。
import type { SchemaDescriptor } from "./types";

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * 单个值的展示文本（只做格式化，不做类型转换/截断）。
 *
 * 数组与对象展开成 `k=v / k=v`，而不是整串 JSON：摘要的用途是让人读懂参数，
 * 不是复刻 wire 载荷。真正的原始值仍在「技术详情」里以 JSON 展示。
 */
export function formatSummaryValue(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  if (Array.isArray(value)) return value.map(formatSummaryValue).join(", ");
  if (isPlainObject(value)) {
    return Object.entries(value)
      .map(([key, inner]) => `${key}=${formatSummaryValue(inner)}`)
      .join(" / ");
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export interface ParameterSummaryOptions {
  /** 字段 key → 展示名；返回 undefined/空串即退回 key 本身。 */
  labelOf?: (key: string) => string | undefined;
  /** 优先输出的 key 顺序（通常是 Schema 字段声明顺序），其余按参数自身键序补在后面。 */
  order?: readonly string[];
  /** 项间分隔符，默认 `" · "`。 */
  separator?: string;
  /**
   * 自定义某个顶层键的渲染（返回 undefined 表示走默认）。
   *
   * 存在的理由：个别字段的「标签 + 值」不是简单拼接（如 NCK 的 drive_object
   * 需要把 `{number, entry}` 渲染成「驱动对象 26（入口 nc_variable）」）。
   * 这是渲染钩子，不是模板语言：调用方给的是纯函数，不引入表达式。
   */
  formatEntry?: (key: string, value: unknown) => string | undefined;
}

/**
 * 把参数表派生为可读摘要。
 *
 * - 跳过 `undefined` / `null` / 空串（未设置项不占篇幅）；
 * - 嵌套对象展开一层（`标签（子键 值 / 子键 值）`），不整串 JSON；
 * - 没有任何可展示项时返回 `""`，由调用方决定回落（point_key / 地址）。
 */
export function summarizeParameters(
  parameters: Record<string, unknown> | null | undefined,
  options: ParameterSummaryOptions = {},
): string {
  const params = parameters ?? {};
  const labelOf = options.labelOf ?? (() => undefined);
  const separator = options.separator ?? " · ";
  const keys: string[] = [];
  for (const key of options.order ?? []) {
    if (Object.prototype.hasOwnProperty.call(params, key) && !keys.includes(key)) keys.push(key);
  }
  for (const key of Object.keys(params)) {
    if (!keys.includes(key)) keys.push(key);
  }

  const parts: string[] = [];
  for (const key of keys) {
    const value = params[key];
    if (value === undefined || value === null) continue;
    if (typeof value === "string" && value === "") continue;

    const custom = options.formatEntry?.(key, value);
    if (custom !== undefined) {
      if (custom !== "") parts.push(custom);
      continue;
    }

    const label = labelOf(key) || key;
    if (isPlainObject(value)) {
      const nested = Object.entries(value)
        .filter(([, inner]) => inner !== undefined && inner !== null && inner !== "")
        .map(([innerKey, inner]) => `${labelOf(innerKey) || innerKey} ${formatSummaryValue(inner)}`);
      if (nested.length) parts.push(`${label}（${nested.join(" / ")}）`);
      continue;
    }
    parts.push(`${label} ${formatSummaryValue(value)}`);
  }
  return parts.join(separator);
}

/** Schema 字段 label 的唯一取值入口（key → label；未声明即 undefined）。 */
export function schemaLabelResolver(
  schema: SchemaDescriptor | null | undefined,
): (key: string) => string | undefined {
  if (!schema) return () => undefined;
  const labels = new Map<string, string>();
  for (const field of schema.fields ?? []) {
    if (typeof field.label === "string" && field.label !== "") labels.set(field.key, field.label);
  }
  return (key) => labels.get(key);
}

/**
 * 点位摘要：有可读参数就用参数摘要，否则回落 point_key（再没有就空串）。
 *
 * 「无标签时回落 point_key / 地址」是 A4 的明确要求：宁可显示身份，
 * 也不显示 `{}` 或一段无法解释的 JSON。
 */
export function pointSummary(input: {
  point_key?: string | null;
  parameters?: Record<string, unknown> | null;
  schema?: SchemaDescriptor | null;
}): string {
  const text = summarizeParameters(input.parameters, {
    labelOf: schemaLabelResolver(input.schema),
    order: (input.schema?.fields ?? []).map((field) => field.key),
  });
  return text || (input.point_key ?? "");
}
