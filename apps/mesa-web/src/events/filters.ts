// PR8 事件过滤器：完整镜像后端 EventsQuery（精确匹配语义），不自创模糊搜索。
import type { EventFilter } from "../types";

export const EVENT_FIRST_PAGE_LIMIT = 100;
export const EVENT_MAX_LIMIT = 500;

export type ActiveFilter = "all" | "active" | "inactive";

export interface EventFilterForm {
  endpoint_id?: string;
  /** M7：全局页 device 下拉进后端过滤（device_id → endpoint 集合，后端映射）。 */
  device_id?: string;
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

/** ns 时间戳 → datetime-local 输入值（本地时区，精确到分钟）。 */
export function nsToLocalInput(ns: number | undefined): string {
  if (ns === undefined) return "";
  const d = new Date(ns / 1e6);
  if (Number.isNaN(d.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** datetime-local 输入值 → ns 时间戳；空/非法返回 undefined（即不过滤）。 */
export function localInputToNs(v: string): number | undefined {
  if (!v) return undefined;
  const t = new Date(v).getTime();
  return Number.isFinite(t) ? Math.floor(t * 1e6) : undefined;
}

/** 表单态 → 后端 EventFilter（active 三态映射；空字符串一律丢弃）。 */
export function toEventFilter(form: EventFilterForm, page: { before_seq?: number | null; limit?: number }): EventFilter {
  const trim = (v: string | undefined): string | undefined => {
    if (v === undefined) return undefined;
    const t = v.trim();
    return t === "" ? undefined : t;
  };
  const out: EventFilter = { limit: page.limit ?? EVENT_FIRST_PAGE_LIMIT };
  const endpoint_id = trim(form.endpoint_id);
  const device_id = trim(form.device_id);
  const category = trim(form.category);
  const kind = trim(form.kind);
  const code = trim(form.code);
  const condition_id = trim(form.condition_id);
  if (endpoint_id) out.endpoint_id = endpoint_id;
  if (device_id) out.device_id = device_id;
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
