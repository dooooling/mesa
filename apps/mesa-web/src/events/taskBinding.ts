// PR8 EventTask 绑定唯一构造点：Web 只生成 mesa.events.v1。
// 本文件是全仓库 Web 侧唯一允许出现该 kind 字面量的地方（grep 锚点）。
import type { DriverBinding, EventTask } from "../types";

export const GENERIC_EVENT_BINDING_KIND = "mesa.events.v1";

/** 构造标准事件绑定：stream_id → EventStreamDescriptor.id，parameters → 其 parameters Schema 取值。 */
export function buildGenericEventBinding(
  streamId: string,
  parameters: Record<string, unknown> = {},
): DriverBinding {
  return {
    kind: GENERIC_EVENT_BINDING_KIND,
    config: {
      stream_id: streamId,
      parameters,
    },
  };
}

/** 是否为标准绑定；非标准一律视为 Private/Legacy（只读展示、可删、不可结构化编辑）。 */
export function isGenericEventTask(task: EventTask): boolean {
  return task.binding?.kind === GENERIC_EVENT_BINDING_KIND;
}

/** 从标准绑定中解析 stream_id（形态不对返回 undefined，调用方按 legacy 只读处理）。 */
export function genericStreamIdOf(task: EventTask): string | undefined {
  if (!isGenericEventTask(task)) return undefined;
  const cfg = task.binding.config as { stream_id?: unknown } | null | undefined;
  return typeof cfg?.stream_id === "string" ? cfg.stream_id : undefined;
}

/** 从标准绑定中解析 parameters（非对象一律按空对象处理，不抛异常）。 */
export function genericParametersOf(task: EventTask): Record<string, unknown> {
  if (!isGenericEventTask(task)) return {};
  const cfg = task.binding.config as { parameters?: unknown } | null | undefined;
  const p = cfg?.parameters;
  if (p && typeof p === "object" && !Array.isArray(p)) return p as Record<string, unknown>;
  return {};
}
