// SSE 实时事件的客户端过滤（服务端 live 无过滤参数；语义与后端 SQL 对齐：
// 精确匹配 + active NULL 不参与）。原属旧 EventsView，M5.6 删除旧页面时
// 搬迁至此（useEventFeed/useDeviceEventFeed 共用）。
import type { StoredEvent } from "../types";
import type { EventFilterForm } from "./filters";

export function matchesLiveFilter(ev: StoredEvent, form: EventFilterForm): boolean {
  if (form.endpoint_id && form.endpoint_id.trim() !== "" && ev.endpoint_id !== form.endpoint_id.trim()) return false;
  if (form.category && form.category.trim() !== "" && ev.event.category !== form.category.trim()) return false;
  if (form.kind && form.kind.trim() !== "" && ev.event.kind !== form.kind.trim()) return false;
  if (form.code && form.code.trim() !== "" && (ev.event.code ?? "") !== form.code.trim()) return false;
  if (form.condition_id && form.condition_id.trim() !== "" && (ev.event.condition?.condition_id ?? "") !== form.condition_id.trim()) return false;
  if (typeof form.severity_min === "number" && ev.event.severity < form.severity_min) return false;
  if (form.active === "active" && ev.event.condition?.active !== true) return false;
  if (form.active === "inactive" && ev.event.condition?.active !== false) return false;
  if (typeof form.from_ns === "number" && ev.received_at_ns < form.from_ns) return false;
  if (typeof form.to_ns === "number" && ev.received_at_ns > form.to_ns) return false;
  return true;
}
