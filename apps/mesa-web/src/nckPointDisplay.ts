// NCK 已配置点位的**展示层纯函数**（P1-2：两层信息，技术细节折叠）。
//
// 为什么单独抽出来：
// 1. 「易读名称 / 地址」是文案判定，必须能逐条单测钉死，不能埋在 JSX 里靠组件
//    测试间接覆盖；
// 2. 有一条硬约束——**不得猜业务名称**。原生地址（area/area_no/module/column/
//    line）没有任何目录声明可供命名，只能显示「原生点位」；语义点位的名称只能
//    取定义目录的 label（或退回 variable_id），绝不允许按地址编造「通道进给倍率」
//    这类业务含义。
//
// 本模块不 import React/AntD，也不做任何身份/准入判定：它只负责把已经通过
// 校验的点位渲染成人能读的两行信息，判定仍在 nckPointModel / nckCollectability。
import { buildNckFormModel, nckDefinitionLabel, type NckDefinitionLike } from "./nckPointModel";

/** 展示所需的最小点位形态（与 `NckPointSelection` 结构兼容）。 */
export interface NckDisplayPoint {
  kind: string;
  point_key: string;
  parameters: Record<string, unknown>;
}

export interface NckPointDisplay {
  /** 易读名称；原生点位恒为「原生点位」（没有可靠业务名称就不编造）。 */
  title: string;
  /** 地址行：语义点位 `语义点位 · 通道 1 / 轴 2`；原生 `A / 1 / 26 / 30110 / 1`。 */
  address: string;
  /** 定义版本行（技术详情用）。 */
  revision: string;
}

/**
 * Target 变体的中文显示名。
 *
 * 这是**字段名翻译**，不是业务命名：键来自目录定义的 `targets`，值只是该键的
 * 中文说法。未收录的键原样展示（绝不猜一个业务含义贴上去）。
 */
const TARGET_LABELS: Record<string, string> = {
  channel: "通道",
  axis: "轴",
  drive_object: "驱动对象",
};

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function formatScalar(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

/** 原生地址五段（与 derived Key 的寻址字段同序，不重实现 Key 规则）。 */
const NATIVE_ADDRESS_KEYS = ["area", "area_no", "module", "column", "line"] as const;

function nativeAddress(parameters: Record<string, unknown>): string {
  const parts = NATIVE_ADDRESS_KEYS.map((key) => parameters[key])
    .filter((value) => value !== undefined && value !== null && value !== "")
    .map((value) => formatScalar(value));
  return parts.length ? parts.join(" / ") : "原生地址字段不完整";
}

/** 语义点位的地址：Target + Arguments，用定义里的 label 解释字段（不猜业务名）。 */
function semanticAddress(
  point: NckDisplayPoint,
  definition: NckDefinitionLike | null | undefined,
): string {
  const parameters = point.parameters ?? {};
  const target = isPlainObject(parameters.target) ? parameters.target : {};
  const args = isPlainObject(parameters.arguments) ? parameters.arguments : {};
  // argument 的显示名只能来自定义（`label` 或 key）；定义不可用时用原始 key。
  const model = definition ? buildNckFormModel(definition) : null;
  const argLabels = new Map<string, string>();
  if (model && model.ok) {
    for (const field of model.argumentFields) argLabels.set(field.key, field.label || field.key);
  }

  const parts: string[] = [];
  for (const [key, value] of Object.entries(target)) {
    const label = TARGET_LABELS[key] ?? key;
    if (isPlainObject(value)) {
      // drive_object 的 envelope：{number, entry}；entry 是入口身份，不是业务名。
      const number = value.number;
      const entry = typeof value.entry === "string" ? value.entry : "";
      parts.push(`${label} ${formatScalar(number)}${entry ? `（入口 ${entry}）` : ""}`);
    } else {
      parts.push(`${label} ${formatScalar(value)}`);
    }
  }
  for (const [key, value] of Object.entries(args)) {
    const label = argLabels.get(key) ?? key;
    parts.push(`${label} ${formatScalar(value)}`);
  }
  return parts.length ? `语义点位 · ${parts.join(" / ")}` : "语义点位";
}

/**
 * 生成点位行的两层信息（易读名称 / 地址 / 定义版本）。
 *
 * 名称来源严格限定：
 * - `nck_native` ⇒ 「原生点位」；
 * - `nck_variable` ⇒ 定义目录的 label（`{zh,en}` 或字符串），缺失则退回
 *   `variable_id`，再缺才用「语义点位」。
 * 绝不按地址/参数推测业务含义。
 */
export function nckPointDisplay(
  point: NckDisplayPoint,
  definition: NckDefinitionLike | null | undefined,
): NckPointDisplay {
  const parameters = point.parameters ?? {};
  if (point.kind === "nck_native") {
    return {
      title: "原生点位",
      address: nativeAddress(parameters),
      revision: "定义版本：不适用（高级原生点位）",
    };
  }
  const variableId = typeof parameters.variable_id === "string" ? parameters.variable_id : "";
  const label = nckDefinitionLabel(definition);
  const revision =
    typeof parameters.definition_revision === "number"
      ? `定义版本：rev ${parameters.definition_revision}`
      : "定义版本：未记录";
  return {
    title: label || variableId || "语义点位",
    address: semanticAddress(point, definition),
    revision,
  };
}
