// PR8 事件过滤器：完整镜像后端 EventsQuery（精确匹配语义），不自创模糊搜索。
import type { EventFilter } from "../types";

export const EVENT_FIRST_PAGE_LIMIT = 100;
export const EVENT_MAX_LIMIT = 500;

export type ActiveFilter = "all" | "active" | "inactive";

export interface EventFilterForm {
  endpoint_id?: string;
  category?: string;
  kind?: string;
  severity_min?: number;
  code?: string;
  condition_id?: string;
  active: ActiveFilter;
  from_ns?: number;
  to_ns?: number;
}

export const EMPTY_EVENT_FILTER_FORM: EventFilterForm = { active: "all" };

/** 表单态 → 后端 EventFilter（active 三态映射；空字符串一律丢弃）。 */
export function toEventFilter(form: EventFilterForm, page: { before_seq?: number | null; limit?: number }): EventFilter {
  const trim = (v: string | undefined): string | undefined => {
    if (v === undefined) return undefined;
    const t = v.trim();
    return t === "" ? undefined : t;
  };
  const out: EventFilter = { limit: page.limit ?? EVENT_FIRST_PAGE_LIMIT };
  const endpoint_id = trim(form.endpoint_id);
  const category = trim(form.category);
  const kind = trim(form.kind);
  const code = trim(form.code);
  const condition_id = trim(form.condition_id);
  if (endpoint_id) out.endpoint_id = endpoint_id;
  if (category) out.category = category;
  if (kind) out.kind = kind;
  if (code) out.code = code;
  if (condition_id) out.condition_id = condition_id;
  if (typeof form.severity_min === "number" && Number.isFinite(form.severity_min)) {
    out.severity_min = Math.max(0, Math.min(1000, Math.floor(form.severity_min)));
  }
  if (form.active === "active") out.active = true;
  else if (form.active === "inactive") out.active = false;
  if (typeof form.from_ns === "number" && Number.isFinite(form.from_ns)) out.from_ns = form.from_ns;
  if (typeof form.to_ns === "number" && Number.isFinite(form.to_ns)) out.to_ns = form.to_ns;
  if (page.before_seq !== undefined && page.before_seq !== null) out.before_seq = page.before_seq;
  return out;
}
