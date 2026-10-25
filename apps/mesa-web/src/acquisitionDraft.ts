// 采集草稿的「有效配置」指纹 + 已保存/待保存对照文案（纯函数，无 React/AntD）。
//
// 为什么需要指纹（P2-1）：保存成功后的「已保存」结论必须随**任何**配置变化失效。
// 旧实现只比较「周期 + point_key + resource_id」：同一原生点位只改 decode/count
// 时这些键完全不变，旧结论就会残留——用户改完参数仍看到「配置已保存」，把旧
// 结论误当成新配置的结论。
//
// 指纹口径 = 本次提交的完整有效配置：
//   - 周期；
//   - 每个点位的完整参数（不是 key 摘要）；
//   - 顺序（点位顺序也是配置的一部分）；
//   - 非 NCK 路径还包括 resource_id + outputs（point_key 由输出决定）。
// 序列化用稳定 JSON（对象 key 排序），避免「同内容不同 key 顺序」被误判成变更。
import type { NckPointSelection, ResourceSelection } from "./deviceModel";

export interface DraftConfig {
  /** 采集周期（ms）。 */
  intervalMs: number;
  /** 非 NCK 路径的待提交选择；NCK 路径不传。 */
  selections?: ResourceSelection[];
  /** NCK 路径的待提交点位；非 NCK 路径不传。 */
  nckPoints?: NckPointSelection[];
}

/**
 * 生成草稿指纹。只纳入当前路径真正会提交的那一侧，另一侧显式写 `null`，
 * 让「切到另一条保存路径」本身也成为一次可识别的变更。
 */
export function draftSignature(draft: DraftConfig): string {
  return stableJson({
    intervalMs: draft.intervalMs,
    selections: draft.selections ?? null,
    nckPoints: draft.nckPoints ?? null,
  });
}

/**
 * 已保存值 / 草稿值的对照文案（P2-2 方案 B）。
 *
 * `saved === null` 表示服务端尚无已保存值（无任务或快照未就绪）：此时只能声明
 * 「待保存」，绝不能把草稿冒充成当前生效配置。
 */
export function savedVsDraftText(saved: number | null, draft: number, unit: string): string {
  if (saved === null) return `${draft} ${unit}（待保存）`;
  if (saved === draft) return `${draft} ${unit}（已保存）`;
  return `${saved} ${unit}（已保存） → ${draft} ${unit}（待保存）`;
}

/** JSON 稳定序列化：对象 key 排序、数组保序、`undefined` 丢弃（对齐 JSON 语义）。 */
function stableJson(value: unknown): string {
  return JSON.stringify(canonicalize(value));
}

function canonicalize(value: unknown): unknown {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(canonicalize);
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(value as Record<string, unknown>).sort()) {
    const v = canonicalize((value as Record<string, unknown>)[key]);
    if (v !== undefined) out[key] = v;
  }
  return out;
}
