// PR8 事件格式化：时间三层明确分离；occurred 缺失绝不冒充；severity 不自创分类。
// UTC Unix ns → 本地可读；Mesa attributes 做通用值渲染，不解释协议字段。

/** ns 时间戳 → 本地字符串；null/undefined 显示 em dash（禁止用 received_at 冒充）。 */
export function formatNsTime(ns: number | null | undefined): string {
  if (ns === null || ns === undefined) return "—";
  const ms = ns / 1_000_000;
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "—";
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  const msPart = String(Math.floor(ms % 1000)).padStart(3, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${msPart}`;
}

/** Severity V1：只显示 0..1000 原值，0 = unknown；不映射 warning/critical。 */
export function formatSeverity(sev: number): string {
  if (!Number.isFinite(sev)) return "—";
  if (sev === 0) return "0 (unknown)";
  return String(sev);
}

/** 通用 Mesa Value 渲染：attributes 已是 JSON 值，此处只做可读字符串化。 */
export function formatMesaValue(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "string") return v === "" ? "(empty)" : v;
  if (typeof v === "number" || typeof v === "boolean" || typeof v === "bigint") return String(v);
  try {
    const s = JSON.stringify(v);
    if (s.length > 500) return `${s.slice(0, 500)}…`;
    return s;
  } catch {
    return String(v);
  }
}

/** 字节数 → 可读（诊断 Store 规模用）。 */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v.toFixed(1)} ${units[u]}`;
}
