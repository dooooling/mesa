// PR-D1 原始快照导出（前端拼装，零后端改动）。
//
// 为什么在浏览器内做：`/points/latest` 已由 SSE/HTTP 提供全量行，前端依次发
// 4 个**既有** GET 就能复用 `capture-points.ps1` 的采集纪律（时刻 → 状态 →
// 原始响应 → 事后状态）；服务端导出并不能获得更好的一致性（行与端点状态是
// 独立锁），却会新增写文件/大小限制/权限面（§2.3）。
//
// 证据分级（P0-2 / P1-2）：
// - **原始证据** = `points_raw_text`（原始响应文本）+ `evidence_integrity`
//   （UTF-8 字节数 + SHA-256）。纳秒级对账只认这两者；
// - **结构化副本** = `points[]` + `summary`，允许 `JSON.parse` 的精度损失。
//   Unix ns ≈1.8e18 超出 `Number.MAX_SAFE_INTEGER`（≈9.0e15），number 会静默
//   舍入（1.8e18 量级间隔 256 ns），因此**导出元数据时间戳一律十进制字符串**
//   （BigInt 计算），绝不使用 `Date.now() * 1e6` 的浮点结果。
//
// 范围（缺陷 3 / 复审修订 1）：原始证据始终是 `/points/latest` 的**全部端点**
// （可能含其他连接/设备），而 `points[]` / `summary` 只覆盖当前 UI 的**设备 +
// 连接**作用域（与计数行/Popover 共用 qualityScope 的同一真值）。两者必须在
// 导出对象里分别标注为 `raw_scope` / `analysis_scope`，防止用户误以为导出
// 只含自己选的连接。
//
// 格式兼容（`mesa.points.export.v1` 的**增量**字段，评审要求明确记录）：
// 本版本新增 `raw_scope`、`analysis_scope`（含 `device_id` / `device_ids`）、
// `freshness_summary.unknown`、`scope.device_id`，均为 **additive**：
// 不重命名、不改变既有字段语义、不删除既有字段。因此消费方必须：
//   1. **保留未知字段**——解析后重建文档时不得丢弃不认识的字段；
//   2. 做**显式兼容性检查**（探测 `schema`/字段是否存在），不满足时给出可诊断
//      错误，而不是静默按旧结构解释；
//   3. **不得假定所有外部解析器都兼容**——严格 Schema 校验程序（`additional
//      Properties: false` 之类）是否接受这些新增字段**仍属未知**，本仓库不做
//      该保证，也不宣称"所有外部解析器都兼容"。
//
// 请求上界：一次点击 **4 个 GET**（2 波），不轮询、不重试；任一失败即中止且
// **不产出任何文件**（禁止半份证据）。`request_status` 只有 `COMPLETE` 一个值，
// 失败时根本不存在文件，故没有 `FAILED` / `INCOMPLETE`。
import { classifyCode, type CodeClass } from "./qualityCode";
import {
  filterPointsInQualityScope,
  makeQualityScope,
  qualityScopeDescription,
  type QualityScope,
} from "./qualityScope";

export const EXPORT_SCHEMA = "mesa.points.export.v1";

/** 内联上限：原文 UTF-8 字节数 > 5 MiB 时拆分为两个文件（§6.5）。 */
export const RAW_INLINE_LIMIT_BYTES = 5 * 1024 * 1024;

/**
 * 固定 4 条限制（§6.4，逐字）。其中"单次导出不是原子一致性快照"是硬要求：
 * 行与端点状态来自独立读取，且 points 的纳秒数值可能被 JSON 舍入。
 */
export const EXPORT_LIMITATIONS: readonly string[] = [
  "仅当前快照，不含历史序列",
  "quality_code 语义依赖 driver_id，未知码不解释",
  "断线后的码为 COMMUNICATION_LOST 覆盖值，不可反推原始项级码",
  "单次导出不是原子一致性快照：行与端点状态来自独立读取，且 points 的纳秒数值可能被 JSON 舍入",
];

/** 证据完整性（唯一字段，不再并列"完整性状态/校验和"两个同义字段）。 */
export interface EvidenceIntegrity {
  raw_preserved: "INLINE" | "SEPARATE_FILE";
  raw_utf8_bytes: number;
  raw_sha256: string;
}

/** 端点状态采样：`{id, driver_id, runtime}` 逐字保留（不重排/不补默认值）。 */
export interface EndpointSamplingState {
  id: string;
  driver_id: string;
  runtime: unknown;
}

export type StateConsistency = "STATE_UNCHANGED" | "STATE_CHANGED_DURING_READ";

export interface FreshnessSummary {
  current: number;
  last_known: number;
  placeholder: number;
  /**
   * 缺陷 4：未知/缺失 `value_origin` 单独计数。
   *
   * 旧实现把它并入 `placeholder`（"无有效值"），等于静默把"来源不明"
   * 断言成"确认无值"——统计失真。未知来源既不能证明来自本次采集，
   * 也不能证明是占位值，只能诚实标为未分类。
   */
  unknown: number;
}

/**
 * 原始证据范围（缺陷 3）：`GET /points/latest` 的**完整**响应，
 * 不按连接筛选，因此可能包含其他连接/设备的点。纳秒级对账只认
 * `points_raw_text` + `evidence_integrity`。
 */
export interface RawScope {
  source: "GET /api/v1/points/latest";
  includes: "ALL_ENDPOINTS";
  row_count: number;
  description: string;
}

/**
 * 分析范围（缺陷 3 / 复审修订 1）：`points[]` / `summary` 实际覆盖的
 * **设备 + 连接**作用域。与 {@link RawScope} 必须分别标注，防止用户以为
 * 导出只含所选连接。
 *
 * `device_ids` 是**实际覆盖**的设备集合（由筛选结果反推，去重排序）；它可能
 * 比 `device_id` 更窄（例如所选设备下没有任何点）。归属不可证明的点不会出现
 * 在结果里（fail-closed）。
 */
export interface AnalysisScope {
  /** 所选设备；`null` = 全部设备。 */
  device_id: string | null;
  /** 实际覆盖的设备 id 集合（排序；不含归属不可证明的端点）。 */
  device_ids: string[];
  /** 所选连接；`null` = 全部连接。 */
  endpoint_id: string | null;
  row_count: number;
  description: string;
}

export interface CodeSummaryEntry {
  endpoint_id: string;
  driver_id: string;
  code: string;
  origin: CodeClass;
  count: number;
}

export interface PointsExportDocument {
  schema: string;
  taken_at_ns: string;
  mesa_version: string;
  /**
   * 兼容字段（扁平视图）：与 `analysis_scope` 同源，但只保留
   * `device_id` / `endpoint_id` / `row_count` 三个标量。
   * **权威范围以 `analysis_scope` 为准**（含 `device_ids` 与文案）。
   */
  scope: { device_id: string | null; endpoint_id: string | null; row_count: number };
  /** 缺陷 3：原始证据范围（全部端点）与当前 UI 作用域无关。 */
  raw_scope: RawScope;
  /** 缺陷 3 / 复审修订 1：分析范围（设备 + 连接筛选后的结构化副本与统计）。 */
  analysis_scope: AnalysisScope;
  history: { available: false; reason: string };
  sampling: {
    evidence_integrity: EvidenceIntegrity;
    endpoint_state_before: EndpointSamplingState[];
    points_read_at_ns: string;
    endpoint_state_after: EndpointSamplingState[];
    state_consistency: StateConsistency;
    request_status: "COMPLETE";
    freshness_summary: FreshnessSummary;
  };
  points: Array<Record<string, unknown>>;
  /** 原始响应文本；`raw_preserved="INLINE"` 时必填（`SEPARATE_FILE` 在 -raw.json）。 */
  points_raw_text?: string;
  summary: {
    by_quality: { GOOD: number; UNCERTAIN: number; BAD: number; UNKNOWN: number };
    by_code: CodeSummaryEntry[];
  };
  limitations: string[];
}

export interface ExportFile {
  name: string;
  text: string;
}

export interface PointsExportResult {
  files: ExportFile[];
  document: PointsExportDocument;
  evidence: EvidenceIntegrity;
  /** 实际发出的 GET 次数（恒为 4；G4 请求上界的可断言依据）。 */
  requestCount: number;
}

/** 导出失败：带 HTTP 状态，UI 文案 `导出失败：<状态>（未生成文件）`。 */
export class ExportHttpError extends Error {
  status: number;
  constructor(status: number, url: string) {
    super(`GET ${url} ${status}`);
    this.name = "ExportHttpError";
    this.status = status;
  }
}

function nowMs(opts: { now?: () => number }): number {
  return opts.now ? opts.now() : Date.now();
}

/** Unix ns 十进制字符串：`BigInt` 计算，无浮点舍入（毫秒分辨率，末 6 位恒 0）。 */
export function utcNsString(ms: number): string {
  return (BigInt(Math.trunc(ms)) * 1_000_000n).toString();
}

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) throw new Error("当前环境不支持 crypto.subtle，无法计算原始证据校验和");
  // 对**原文 UTF-8 字节**计算（不是 String.length：后者是 UTF-16 码元数）。
  // 这里的断言只为满足 lib.dom 的 BufferSource 类型（Uint8Array<ArrayBufferLike>
  // 与 Uint8Array<ArrayBuffer> 的泛型差异），运行期字节不变。
  const digest = await subtle.digest("SHA-256", bytes as unknown as BufferSource);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

async function getText(
  fetchImpl: typeof fetch,
  url: string,
): Promise<{ text: string; json: unknown }> {
  const r = await fetchImpl(url);
  // 先取原文：即使非 2xx 也不猜测内容；失败路径不产出任何文件。
  const text = await r.text();
  if (!r.ok) throw new ExportHttpError(r.status, url);
  try {
    return { text, json: JSON.parse(text) as unknown };
  } catch {
    throw new Error(`GET ${url} 响应不是合法 JSON`);
  }
}

/** 端点清单 → 采样快照（只保留 id/driver_id/runtime，其余字段不入证据）。 */
function projectEndpointState(json: unknown): EndpointSamplingState[] {
  const list = (json as { endpoints?: unknown } | null)?.endpoints;
  if (!Array.isArray(list)) {
    throw new Error("GET /api/v1/endpoints 结构不符：缺少 endpoints 数组");
  }
  return list.map((raw) => {
    const ep = raw as { id?: unknown; driver_id?: unknown; runtime?: unknown };
    if (typeof ep.id !== "string" || typeof ep.driver_id !== "string") {
      throw new Error("GET /api/v1/endpoints 结构不符：端点缺 id/driver_id");
    }
    // runtime 逐字保留（不重建字段，避免把未知字段静默抹掉）。
    return { id: ep.id, driver_id: ep.driver_id, runtime: ep.runtime ?? null };
  });
}

/**
 * 端点 → 设备归属（`/endpoints` 响应已含 `device_id`，零后端改动）。
 * 缺失/非字符串一律记空串，交给作用域函数 fail-closed（不猜归属）。
 */
function projectEndpointDevices(json: unknown): Map<string, string> {
  const list = (json as { endpoints?: unknown } | null)?.endpoints;
  const out = new Map<string, string>();
  if (!Array.isArray(list)) return out;
  for (const raw of list) {
    const ep = raw as { id?: unknown; device_id?: unknown };
    if (typeof ep.id !== "string") continue;
    out.set(ep.id, typeof ep.device_id === "string" ? ep.device_id : "");
  }
  return out;
}

function readVersion(json: unknown): string {
  const version = (json as { version?: unknown } | null)?.version;
  // diagnostics 是必需请求；缺失 version 即结构不符 → 中止，不产出文件。
  if (typeof version !== "string" || version.length === 0) {
    throw new Error("GET /api/v1/diagnostics 结构不符：缺少 version");
  }
  return version;
}

function countFreshness(points: Array<Record<string, unknown>>): FreshnessSummary {
  const out: FreshnessSummary = { current: 0, last_known: 0, placeholder: 0, unknown: 0 };
  for (const p of points) {
    const origin = p.value_origin;
    if (origin === "CURRENT") out.current += 1;
    else if (origin === "LAST_KNOWN") out.last_known += 1;
    else if (origin === "PLACEHOLDER") out.placeholder += 1;
    // 缺陷 4：未知/缺失来源**不得**并入 placeholder（那会把"来源不明"
    // 静默断言成"确认无值"）。单独计为 unknown，保持可解释。
    else out.unknown += 1;
  }
  return out;
}

function countQuality(
  points: Array<Record<string, unknown>>,
): PointsExportDocument["summary"]["by_quality"] {
  const out = { GOOD: 0, UNCERTAIN: 0, BAD: 0, UNKNOWN: 0 };
  for (const p of points) {
    const q = typeof p.quality === "string" ? p.quality.toUpperCase() : "";
    if (q === "GOOD") out.GOOD += 1;
    else if (q === "UNCERTAIN") out.UNCERTAIN += 1;
    else if (q === "BAD") out.BAD += 1;
    else out.UNKNOWN += 1;
  }
  return out;
}

/**
 * 按码汇总。键 = `(endpoint_id, driver_id, code)`：跨驱动同名数字**不得**相加
 * （`3` 在 nck 与 focas2 语义不同），故 driver_id 必须进键。
 */
function countByCode(
  points: Array<Record<string, unknown>>,
  driverByEndpoint: Map<string, string>,
): CodeSummaryEntry[] {
  const acc = new Map<string, CodeSummaryEntry>();
  for (const p of points) {
    const code = p.quality_code;
    if (typeof code !== "string") continue;
    const endpointId = typeof p.endpoint_id === "string" ? p.endpoint_id : "";
    const driverId = driverByEndpoint.get(endpointId) ?? "";
    const key = `${endpointId}\u0000${driverId}\u0000${code}`;
    const existing = acc.get(key);
    if (existing) {
      existing.count += 1;
      continue;
    }
    acc.set(key, {
      endpoint_id: endpointId,
      driver_id: driverId,
      code,
      origin: classifyCode(driverId, code),
      count: 1,
    });
  }
  return [...acc.values()];
}

function utcStamp(ns: string): string {
  const ms = Number(BigInt(ns) / 1_000_000n);
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}` +
    `-${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}`
  );
}

export interface BuildPointsExportOptions {
  /**
   * 统计/分析作用域（**设备 + 连接**）；缺省 = 全部设备与全部连接。
   *
   * 与页面计数行/Popover 共用 {@link filterPointsInQualityScope} 同一真值：
   * 设备选中 + 连接 ALL 时只导出该设备的全部连接，**不是**跨设备。
   */
  scope?: Partial<QualityScope> | null;
  /** 注入时钟（测试用；生产走 `Date.now()`）。 */
  now?: () => number;
  /** 注入 fetch（测试用；生产走全局 fetch）。 */
  fetchImpl?: typeof fetch;
}

/**
 * 生成导出文档与文件（纯内存；下载由调用方执行）。
 *
 * 4 个 GET 顺序（T10 断言）：`/endpoints` → `/diagnostics` → `/points/latest`
 * → `/endpoints`。第一波两个请求并行（状态前 + 版本），其余严格串行。
 */
export async function buildPointsExport(
  options: BuildPointsExportOptions = {},
): Promise<PointsExportResult> {
  const fetchImpl = options.fetchImpl ?? fetch;
  const scope = makeQualityScope(options.scope?.deviceId, options.scope?.endpointId);

  // 步骤 1：点击时刻（十进制字符串，BigInt 计算）。
  const takenAtNs = utcNsString(nowMs(options));

  // 步骤 2：状态前 + 版本（并行；两者都是必需请求）。
  const [beforeRes, diagRes] = await Promise.all([
    getText(fetchImpl, "/api/v1/endpoints"),
    getText(fetchImpl, "/api/v1/diagnostics"),
  ]);
  const stateBefore = projectEndpointState(beforeRes.json);
  // 设备归属映射：作用域过滤只用**读点前**的清单（与状态前快照同一时刻）。
  const deviceByEndpoint = projectEndpointDevices(beforeRes.json);
  const mesaVersion = readVersion(diagRes.json);

  // 步骤 3：读点前时刻 + 原始响应（先原文，再结构化副本）。
  const pointsReadAtNs = utcNsString(nowMs(options));
  const latestRes = await getText(fetchImpl, "/api/v1/points/latest");
  const parsedLatest = latestRes.json as { points?: unknown } | null;
  if (!Array.isArray(parsedLatest?.points)) {
    throw new Error("GET /api/v1/points/latest 结构不符：缺少 points 数组");
  }
  const allPoints = parsedLatest.points as Array<Record<string, unknown>>;
  // 作用域过滤只影响"结构化副本"；原始响应文本始终完整保留为权威证据。
  // 设备 + 连接双重作用域：与计数行/Popover 调用**同一个**过滤函数。
  // 断言只为让 `Record<string, unknown>` 行满足 ScopedPoint 的静态形状；
  // 运行期仍按 fail-closed 处理（非字符串 endpoint_id 不会被误判为匹配）。
  const scopedAllPoints = allPoints as Array<Record<string, unknown> & { endpoint_id: string }>;
  const points = filterPointsInQualityScope(
    scopedAllPoints,
    scope,
    (endpointId) => deviceByEndpoint.get(endpointId) ?? "",
  );
  // 实际覆盖的设备（由筛选结果反推；归属不可证明的点已被排除）。
  const deviceIds = [
    ...new Set(
      points.map((p) => {
        const endpointId = typeof p.endpoint_id === "string" ? p.endpoint_id : "";
        return deviceByEndpoint.get(endpointId) ?? "";
      }),
    ),
  ]
    .filter((id) => id.length > 0)
    .sort();

  // 缺陷 3：证据范围与分析范围必须分开标注（两者可能不同）。
  const rawScope: RawScope = {
    source: "GET /api/v1/points/latest",
    includes: "ALL_ENDPOINTS",
    row_count: allPoints.length,
    description:
      "原始响应包含全部端点（可能含其他连接/设备），未按设备或连接筛选；纳秒级对账以 points_raw_text 为权威",
  };
  const analysisScope: AnalysisScope = {
    device_id: scope.deviceId,
    device_ids: deviceIds,
    endpoint_id: scope.endpointId,
    row_count: points.length,
    description: qualityScopeDescription(scope),
  };

  // 步骤 4：状态后。
  const afterRes = await getText(fetchImpl, "/api/v1/endpoints");
  const stateAfter = projectEndpointState(afterRes.json);

  // 步骤 5-6：四个字段分离（状态一致性 / 请求完整性 / 新鲜度分布 / 证据完整性）。
  const stateConsistency: StateConsistency =
    JSON.stringify(stateBefore) === JSON.stringify(stateAfter)
      ? "STATE_UNCHANGED"
      : "STATE_CHANGED_DURING_READ";

  // 步骤 7：证据完整性（对原文 UTF-8 字节计算，不是 String.length）。
  const rawBytes = new TextEncoder().encode(latestRes.text);
  const rawSha256 = await sha256Hex(rawBytes);
  const evidence: EvidenceIntegrity = {
    raw_preserved: rawBytes.length > RAW_INLINE_LIMIT_BYTES ? "SEPARATE_FILE" : "INLINE",
    raw_utf8_bytes: rawBytes.length,
    raw_sha256: rawSha256,
  };

  const driverByEndpoint = new Map(stateBefore.map((e) => [e.id, e.driver_id] as const));

  const document: PointsExportDocument = {
    schema: EXPORT_SCHEMA,
    taken_at_ns: takenAtNs,
    mesa_version: mesaVersion,
    scope: {
      device_id: scope.deviceId,
      endpoint_id: scope.endpointId,
      row_count: points.length,
    },
    raw_scope: rawScope,
    analysis_scope: analysisScope,
    history: {
      available: false,
      reason: "Snapshot 仅驻留当前最新值，不含时间序列",
    },
    sampling: {
      evidence_integrity: evidence,
      endpoint_state_before: stateBefore,
      points_read_at_ns: pointsReadAtNs,
      endpoint_state_after: stateAfter,
      state_consistency: stateConsistency,
      // 单值：失败路径根本不产文件，故不存在 FAILED/INCOMPLETE。
      request_status: "COMPLETE",
      freshness_summary: countFreshness(points),
    },
    points,
    summary: {
      by_quality: countQuality(points),
      by_code: countByCode(points, driverByEndpoint),
    },
    limitations: [...EXPORT_LIMITATIONS],
  };

  const stamp = utcStamp(takenAtNs);
  // 文件名是分享时最先看到的东西：设备筛选必须体现在名字里，
  // 否则"只含所选设备"的分析文件会被误当成整库快照。
  const scopeName =
    [scope.deviceId, scope.endpointId].filter((v): v is string => v !== null).join("__") || "all";
  const stateSuffix = stateConsistency === "STATE_CHANGED_DURING_READ" ? "-statechanged" : "";
  const base = `mesa-points-export-${stamp}-${scopeName}${stateSuffix}`;

  if (evidence.raw_preserved === "INLINE") {
    document.points_raw_text = latestRes.text;
    return {
      files: [{ name: `${base}.json`, text: JSON.stringify(document, null, 2) }],
      document,
      evidence,
      requestCount: 4,
    };
  }

  // SEPARATE_FILE：原文与结构化副本分成两个文件，两边带同一 evidence_integrity
  // 用于配对校验（禁止"省略原文后仍宣称完整原始证据"）。
  const rawFile = {
    schema: EXPORT_SCHEMA,
    taken_at_ns: takenAtNs,
    // 原始证据文件也标注范围：单独打开它时同样不应误以为只含所选连接。
    raw_scope: rawScope,
    evidence_integrity: evidence,
    points_raw_text: latestRes.text,
  };
  const analysisFile: PointsExportDocument = { ...document };
  delete analysisFile.points_raw_text;
  return {
    files: [
      { name: `${base}-raw.json`, text: JSON.stringify(rawFile, null, 2) },
      { name: `${base}-analysis.json`, text: JSON.stringify(analysisFile, null, 2) },
    ],
    document: analysisFile,
    evidence,
    requestCount: 4,
  };
}

/**
 * 触发浏览器下载（文件由调用方在成功后传入；失败路径不调用）。
 * 单个文件即 INLINE；`SEPARATE_FILE` 传两个文件（UI 需提示"已拆分为 2 个文件"）。
 */
export function downloadExportFiles(files: ExportFile[]): void {
  for (const file of files) {
    const blob = new Blob([file.text], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = file.name;
    a.rel = "noopener";
    document.body.appendChild(a);
    a.click();
    a.remove();
    // 立刻 revoke 会让部分浏览器取消下载，推迟到下一轮事件循环。
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }
}
