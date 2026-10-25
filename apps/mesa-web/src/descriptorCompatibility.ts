// Descriptor 字段类型兼容性判定（纯前端，**不新增任何契约**）。
//
// 背景：`DescriptorFields` 原先对未识别的 `field_type` 走「文本框兜底」，
// 于是一个新类型（或降级/错版驱动）会被渲染成可编辑字符串：界面看起来能改，
// 提交的值却与驱动语义无关，属于「假装能用」。
//
// 本模块只回答一个问题：**当前界面是否认识这个字段类型**。
// - 已知文本类类型（string/host/url/file/certificate_ref）共用一个文本框控件；
// - 其余已知类型各有专用控件（boolean/enum/secret/integer/number/port/duration）；
// - 其余一律视为「不支持」，由界面拒绝编辑与提交（后端仍有
//   `SchemaDescriptor::validate_instance` 独立校验，两件事互不替代）。
//
// 为什么判定要扫**完整 Schema**（不看 ui.advanced / visible_if）：
// 高级区默认折叠、visible_if 随值变化，若只检查「当前渲染出来的字段」，
// 用户会先看到一份完整可编辑的表单，展开/切值后才发现不支持——那正是本次
// 要消除的「延迟发现」。判定在打开界面时一次性完成。
import type { FieldDescriptor, SchemaDescriptor } from "./types";

/**
 * 文本类字段类型白名单：共用同一个文本输入框。
 *
 * 与 core-types `FieldType` 的 wire 形态一一对应；这里**只列文本类**，
 * 不把 integer/number 等塞进来——它们有独立控件，不能降级成文本。
 */
export const TEXT_FIELD_TYPES = [
  "string",
  "host",
  "url",
  "file",
  "certificate_ref",
] as const;

/** 有专用控件的非文本类型。 */
export const DEDICATED_FIELD_TYPES = [
  "boolean",
  "enum",
  "secret",
  "integer",
  "number",
  "port",
  "duration",
] as const;

const SUPPORTED = new Set<string>([...TEXT_FIELD_TYPES, ...DEDICATED_FIELD_TYPES]);

/** 文本类：共用文本框（`DescriptorFields` 的唯一文本渲染路径）。 */
export function isTextLikeFieldType(fieldType: unknown): boolean {
  return typeof fieldType === "string" && (TEXT_FIELD_TYPES as readonly string[]).includes(fieldType);
}

/** 已知类型（文本类或专用控件）；未知类型必须拒绝编辑。 */
export function isSupportedFieldType(fieldType: unknown): boolean {
  return typeof fieldType === "string" && SUPPORTED.has(fieldType);
}

/** 一条「界面不支持」的字段记录（技术详情用，不做逐行红框）。 */
export interface UnsupportedField {
  key: string;
  /** 字段展示名（Descriptor label；缺失时由调用方退回 key）。 */
  label: string;
  /** wire 上的原始类型名，**不做归一化/映射**（未知类型必须原样可见）。 */
  fieldType: string;
  /** 是否位于高级区（折叠不能掩盖不支持：只用于详情标注）。 */
  advanced: boolean;
}

/**
 * 扫描**完整 Schema**：所有字段（含高级区、含当前被 visible_if 隐藏的字段）。
 *
 * 返回值顺序即 Schema 声明顺序，便于技术详情稳定复现。
 */
export function unsupportedFields(
  schema: SchemaDescriptor | null | undefined,
): UnsupportedField[] {
  const fields = schema?.fields ?? [];
  return fields
    .filter((field) => !isSupportedFieldType(field?.field_type))
    .map((field) => ({
      key: String(field.key ?? ""),
      label: typeof field.label === "string" ? field.label : String(field.key ?? ""),
      // 未知类型可能是任意 JSON 值（甚至非字符串），原样展示才能定位驱动版本问题。
      fieldType: String((field as { field_type?: unknown }).field_type ?? ""),
      advanced: field.ui?.advanced === true,
    }));
}

/**
 * 只报告**该配置里真的带着**的不支持字段（按 key 存在性判定）。
 *
 * 为什么需要这个更窄的判定：对「已经保存过的配置」做提交前检查时，若因为
 * Descriptor 里声明了一个当前未使用的未知字段就封掉整个保存，会波及与问题
 * 无关的配置（违反「只阻止相关配置」）。这里要求 key 在 `value` 里真实存在，
 * 把影响范围限制在「确实携带未知字段」的那一条配置上。
 *
 * 注意：只判 key 是否存在（含 null），**不判断值是否合法**——类型合法性是
 * 后端 `validate_instance` 的职责。
 */
export function unsupportedFieldsInValue(
  schema: SchemaDescriptor | null | undefined,
  value: Record<string, unknown> | null | undefined,
): UnsupportedField[] {
  if (!value) return [];
  return unsupportedFields(schema).filter((field) =>
    Object.prototype.hasOwnProperty.call(value, field.key),
  );
}

/**
 * 用户可见的兼容性提示（唯一文案，所有入口复用，避免各写一套说法）。
 *
 * 出现时机：**打开相关编辑界面、发现不兼容时出现一次**（组件顶部一条 Alert），
 * 不是逐行红框；字段名与类型收在「技术详情」里。
 */
export const UNSUPPORTED_CONFIG_NOTICE =
  "当前界面不支持该配置中的部分字段，暂时无法编辑或保存。请更新界面或检查驱动版本。";

/** 技术详情行：`字段名 · 类型`（高级字段额外标注，便于对照驱动版本）。 */
export function unsupportedFieldDetails(fields: UnsupportedField[]): string[] {
  return fields.map((field) =>
    field.advanced ? `${field.key} · ${field.fieldType}（高级）` : `${field.key} · ${field.fieldType}`,
  );
}

/** 字段 key 集合（供渲染层按 key 选择只读视图）。 */
export function unsupportedFieldKeys(fields: UnsupportedField[]): Set<string> {
  return new Set(fields.map((field) => field.key));
}

/** 便于调用方在渲染前直接问一个 FieldDescriptor。 */
export function isUnsupportedField(field: FieldDescriptor): boolean {
  return !isSupportedFieldType(field?.field_type);
}
