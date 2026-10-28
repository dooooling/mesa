// PR-C 第二批 B：I-2 只读目录 / 权威身份预览的**唯一**客户端。
//
// 为什么必须有这一层（而不是在组件里直接 fetch）：
// 1. I-2 的目录是**分页契约**：`next_cursor` 是不透明复合游标
//    `v1-<variable_id>-<revision>`（drivers/sinumerik-nck/src/definition_api.rs:298-347）。
//    客户端只允许**逐字回传**，不得解析或自行构造；否则一旦同一变量发布多个
//    revision，页边界会静默漏项。分页循环、不重不漏、`total` 恒定校验因此必须
//    收敛到一处，而不是散落在 UI 的事件回调里。
// 2. 错误信封是 `{error:{code,kind,message}}`，HTTP 状态**只由 kind 决定**
//    （crates/core-api/src/driver_query.rs:179-190，全仓唯一映射点）。
//    客户端据此分类，不按 `code` 反推状态（那等于在前端重建 Core 的码表）。
// 3. 目录是「目录」不是「可采集清单」：Driver 如实返回 36 collect + 6 identity
//    （definition_api.rs:18-22）。本客户端**不做 scope 过滤**——过滤发生在 UI
//    的 `collectableDefinitions`（nckCollectability.ts），提交模型还会再查一次。
//    如果把过滤做在传输层，目录的完整性就丢了（`total` 与条目数会对不上），
//    也无法向用户解释「为什么少了 6 条」。
//
// 本文件为纯客户端逻辑：不 import React/AntD，可被 vitest 直接驱动。

import { CatalogContractError, readCompleteCatalog, type CompleteCatalogResult } from "./catalogContract";

/** 目录条目里的一条来源声明（I-2 `source[]`）。 */
export interface NckCatalogSourceRef {
  kind?: string;
  reference?: string;
  note?: string;
}

/** 定义展示名（I-2 `label{zh,en}`）。 */
export interface NckCatalogLabel {
  zh?: string;
  en?: string;
}

/** 身份键声明（I-2 `identity_keys[]`）。 */
export interface NckCatalogIdentityKey {
  path?: string;
  required?: boolean;
}

/**
 * arguments 规格（I-2 `arguments[]`）。
 *
 * `value_type` 今天只有 `integer` / `enum`（definition.rs:102-105）；`enum`
 * 的候选集来自 `enum_options`。若为 `enum` 但**没有**候选集，前端无法渲染，
 * 表单层必须 fail-closed（见 nckPointModel.buildNckFormModel）。
 */
export interface NckCatalogArgumentSpec {
  key: string;
  label?: NckCatalogLabel | string;
  value_type: string;
  required?: boolean;
  min?: number | null;
  max?: number | null;
  enum_options?: string[] | null;
  default?: unknown;
}

/** 冻结 wire 向量引用（I-2 `golden_ref[]`）。 */
export interface NckCatalogGoldenRef {
  area?: string;
  block?: string;
  variable?: string;
  /**
   * 驱动入口名（如 `nc_variable` / `drive_object`）。
   *
   * 前端**只把它当数据**：`drive_object` target 的 `entry` 候选集从这里的
   * 去重值得到，而不是在前端硬编码 module 26/130 的口径。若某定义没有
   * `golden_ref[].entry`，表单对 `entry` 采取 fail-closed（无法确认候选）。
   */
  entry?: string;
}

/** I-2 目录条目：**14 字段白名单投影**（definition_api.rs:56-71）。 */
export interface NckCatalogDefinition {
  variable_id: string;
  revision: number;
  family: string;
  scope: string;
  semantics_status: string;
  label: NckCatalogLabel;
  source: NckCatalogSourceRef[];
  targets: string[];
  target_required: string[];
  arguments: NckCatalogArgumentSpec[];
  identity_template: string;
  identity_keys: NckCatalogIdentityKey[];
  golden_ref: NckCatalogGoldenRef[];
  digest: string;
}

/** I-2 目录响应：**10 字段分页信封**（definition_api.rs:260-282）。 */
export interface NckDefinitionsResponse {
  format_version: number;
  /** 请求指定则回显；未指定且只发布一个系列时报该系列；多系列为 null。 */
  family: string | null;
  revision: number;
  driver_id: string;
  driver_version: string;
  /** 仅「已声明但无定义」时非 null（与驱动故障可区分）。 */
  note: string | null;
  /** **过滤后、分页前**的总数；翻页恒定。 */
  total: number;
  limit: number;
  /** 无下一页为 null；否则必须逐字回传。 */
  next_cursor: string | null;
  definitions: NckCatalogDefinition[];
}

/** I-2 权威身份预览响应：**6 字段**（definition_api.rs:649-660）。 */
export interface NckIdentityPreviewResponse {
  /** 成功态恒非空。 */
  point_key: string;
  identity_mode: string;
  /** 定义原始 scope（不是准入结论）；原生点位为 null。 */
  scope: string | null;
  /** 语义点位为定义坐标；原生点位为 null。 */
  variable_id: string | null;
  definition_revision: number | null;
  definition_digest: string | null;
}

/** 统一错误信封（driver_query.rs:217-230）。 */
export interface NckApiErrorEnvelope {
  error: {
    code: string;
    kind: string;
    message: string;
  };
}

/** 错误分类：只用于文案/重试策略，**不参与**「状态码由 kind 决定」的映射。 */
export type NckFailureClass =
  | "request"
  | "not_found"
  | "too_large"
  | "unsupported"
  | "unavailable"
  | "timeout"
  | "network"
  | "contract";

/**
 * 客户端侧统一错误对象。
 *
 * `status` 是服务端响应状态（网络失败为 0）；`kind` 是错误信封里的 ErrorKind。
 * 二者都保留：状态码由服务端按 kind 唯一映射得到（driver_query.rs:179-190），
 * 客户端**不**根据 code 重新推断状态，只负责原样展示。
 */
export class NckRequestError extends Error {
  status: number;
  code: string;
  kind: string;

  constructor(args: { status: number; code: string; kind: string; message: string }) {
    super(args.message);
    this.name = "NckRequestError";
    this.status = args.status;
    this.code = args.code;
    this.kind = args.kind;
  }
}

export function isNckRequestError(value: unknown): value is NckRequestError {
  return value instanceof NckRequestError;
}

/**
 * 错误分类（文案与重试策略用）。
 *
 * 判定顺序：**先 kind、后 status**。413 是唯一一个信封外的框架级改写
 * （driver_query.rs:196-211），因此单独按 status 识别；其余状态若与 kind 不一致，
 * 以 kind 为准并归为 contract（说明服务端映射被破坏，不能猜）。
 */
export function classifyNckError(status: number, kind: string | undefined): NckFailureClass {
  if (status === 0) return "network";
  if (status === 413) return "too_large";
  switch (kind) {
    case "ConfigurationError":
      return "request";
    case "AddressError":
      return "not_found";
    case "UnsupportedError":
      return "unsupported";
    case "TimeoutError":
      return "timeout";
    case "ConnectionError":
    case "DeviceError":
    case "ProtocolError":
    case "DecodeError":
    case "InternalError":
      return "unavailable";
    default:
      return "contract";
  }
}

/** 错误分类对应的中文短标签（只用于展示，不改变判定）。 */
export function describeNckFailure(cls: NckFailureClass): string {
  switch (cls) {
    case "request":
      return "请求或参数不合法";
    case "not_found":
      return "变量/定义不存在";
    case "too_large":
      return "请求体过大";
    case "unsupported":
      return "驱动不支持该能力";
    case "unavailable":
      return "驱动不可用或内部错误";
    case "timeout":
      return "请求超时";
    case "network":
      return "网络请求失败";
    case "contract":
      return "响应不符合契约";
  }
}

function parseErrorEnvelope(body: unknown): NckApiErrorEnvelope["error"] | null {
  if (!body || typeof body !== "object") return null;
  const error = (body as { error?: unknown }).error;
  if (!error || typeof error !== "object") return null;
  const { code, kind, message } = error as Record<string, unknown>;
  if (typeof code !== "string" || typeof kind !== "string" || typeof message !== "string") {
    return null;
  }
  return { code, kind, message };
}

async function readError(response: Response): Promise<NckRequestError> {
  const body = await response.json().catch(() => null);
  const envelope = parseErrorEnvelope(body);
  if (envelope) {
    return new NckRequestError({
      status: response.status,
      code: envelope.code,
      kind: envelope.kind,
      message: envelope.message,
    });
  }
  return new NckRequestError({
    status: response.status,
    code: `HTTP_${response.status}`,
    kind: "",
    message: `服务端返回 ${response.status}，且响应体不是标准错误信封`,
  });
}

function contractError(message: string): NckRequestError {
  return new NckRequestError({
    status: 200,
    code: "CATALOG_RESPONSE_INVALID",
    kind: "",
    message,
  });
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((v) => typeof v === "string");
}

/**
 * 校验一页目录响应的形状（不做业务语义判断：family/revision/scope 留给调用方）。
 *
 * 为什么在客户端再校验一次：Core 只做「逐字透传」（driver_query.rs:112-117），
 * 驱动若返回半截结构，前端必须整体拒绝而不是拿 `undefined` 继续拼 UI——那会
 * 把「契约损坏」伪装成「目录为空」，正是 fail-closed 要避免的。
 */
export function parseDefinitionsResponse(value: unknown): NckDefinitionsResponse {
  if (!isPlainObject(value)) throw contractError("目录响应不是 JSON 对象");
  const required = [
    "format_version",
    "family",
    "revision",
    "driver_id",
    "driver_version",
    "note",
    "total",
    "limit",
    "next_cursor",
    "definitions",
  ] as const;
  for (const key of required) {
    if (!(key in value)) throw contractError(`目录响应缺少字段 \`${key}\``);
  }
  if (typeof value.format_version !== "number") throw contractError("format_version 必须是数字");
  if (value.family !== null && typeof value.family !== "string") {
    throw contractError("family 必须是字符串或 null");
  }
  if (typeof value.revision !== "number") throw contractError("revision 必须是数字");
  if (typeof value.driver_id !== "string") throw contractError("driver_id 必须是字符串");
  if (typeof value.driver_version !== "string") {
    throw contractError("driver_version 必须是字符串");
  }
  if (value.note !== null && typeof value.note !== "string") {
    throw contractError("note 必须是字符串或 null");
  }
  if (typeof value.total !== "number" || !Number.isInteger(value.total) || value.total < 0) {
    throw contractError("total 必须是非负整数");
  }
  if (typeof value.limit !== "number" || !Number.isInteger(value.limit) || value.limit <= 0) {
    throw contractError("limit 必须是正整数");
  }
  if (value.next_cursor !== null && typeof value.next_cursor !== "string") {
    throw contractError("next_cursor 必须是字符串或 null");
  }
  if (!Array.isArray(value.definitions)) throw contractError("definitions 必须是数组");
  const definitions = value.definitions.map((entry, index) =>
    parseDefinitionEntry(entry, index),
  );
  return {
    format_version: value.format_version,
    family: value.family as string | null,
    revision: value.revision,
    driver_id: value.driver_id,
    driver_version: value.driver_version,
    note: value.note as string | null,
    total: value.total,
    limit: value.limit,
    next_cursor: value.next_cursor as string | null,
    definitions,
  };
}

/** 14 字段白名单投影的形状校验（缺一个即整体拒绝，不补默认值）。 */
function parseDefinitionEntry(value: unknown, index: number): NckCatalogDefinition {
  if (!isPlainObject(value)) throw contractError(`definitions[${index}] 不是 JSON 对象`);
  const where = `definitions[${index}]`;
  const need = (key: string) => {
    if (!(key in value)) throw contractError(`${where} 缺少字段 \`${key}\``);
    return value[key];
  };
  const variableId = need("variable_id");
  if (typeof variableId !== "string" || variableId === "") {
    throw contractError(`${where}.variable_id 必须是非空字符串`);
  }
  const revision = need("revision");
  if (typeof revision !== "number" || !Number.isInteger(revision) || revision < 1) {
    throw contractError(`${where}.revision 必须是 >=1 的整数`);
  }
  const family = need("family");
  if (typeof family !== "string" || family === "") {
    throw contractError(`${where}.family 必须是非空字符串`);
  }
  const scope = need("scope");
  if (typeof scope !== "string") throw contractError(`${where}.scope 必须是字符串`);
  const status = need("semantics_status");
  if (typeof status !== "string") throw contractError(`${where}.semantics_status 必须是字符串`);
  const label = need("label");
  if (!isPlainObject(label)) throw contractError(`${where}.label 必须是对象`);
  const source = need("source");
  if (!Array.isArray(source)) throw contractError(`${where}.source 必须是数组`);
  const targets = need("targets");
  if (!isStringArray(targets)) throw contractError(`${where}.targets 必须是字符串数组`);
  const targetRequired = need("target_required");
  if (!isStringArray(targetRequired)) {
    throw contractError(`${where}.target_required 必须是字符串数组`);
  }
  const args = need("arguments");
  if (!Array.isArray(args)) throw contractError(`${where}.arguments 必须是数组`);
  for (const [i, spec] of args.entries()) {
    if (!isPlainObject(spec) || typeof spec.key !== "string" || typeof spec.value_type !== "string") {
      throw contractError(`${where}.arguments[${i}] 必须包含字符串 key/value_type`);
    }
  }
  const template = need("identity_template");
  if (typeof template !== "string") throw contractError(`${where}.identity_template 必须是字符串`);
  const identityKeys = need("identity_keys");
  if (!Array.isArray(identityKeys)) throw contractError(`${where}.identity_keys 必须是数组`);
  const goldenRef = need("golden_ref");
  if (!Array.isArray(goldenRef)) throw contractError(`${where}.golden_ref 必须是数组`);
  const digest = need("digest");
  if (typeof digest !== "string") throw contractError(`${where}.digest 必须是字符串`);
  return {
    variable_id: variableId,
    revision,
    family,
    scope,
    semantics_status: status,
    label: label as NckCatalogLabel,
    source: source as NckCatalogSourceRef[],
    targets,
    target_required: targetRequired,
    arguments: args as NckCatalogArgumentSpec[],
    identity_template: template,
    identity_keys: identityKeys as NckCatalogIdentityKey[],
    golden_ref: goldenRef as NckCatalogGoldenRef[],
    digest,
  };
}

/** 一页目录的请求参数。 */
export interface NckCatalogPageRequest {
  driverId: string;
  /** 目录查询的 family 过滤；缺省则不发送（与 §4.1 的「可省略」一致）。 */
  family?: string;
  /** 页大小；缺省由驱动取 100，超出 500 由驱动夹取。 */
  limit?: number;
  /** 上一页的 `next_cursor`，**逐字**回传。 */
  cursor?: string;
}

export type NckFetchLike = (
  input: string,
  init?: RequestInit,
) => Promise<Pick<Response, "ok" | "status" | "json">>;

/** 请求一页目录。 */
export async function fetchNckCatalogPage(
  request: NckCatalogPageRequest,
  fetchImpl: NckFetchLike = fetch,
): Promise<NckDefinitionsResponse> {
  const query = new URLSearchParams();
  if (request.family !== undefined) query.set("family", request.family);
  if (request.limit !== undefined) query.set("limit", String(request.limit));
  // 逐字回传：不解析、不重构、不补默认值。游标形态由驱动定义（v1-<id>-<rev>）。
  if (request.cursor !== undefined) query.set("cursor", request.cursor);
  const suffix = query.toString() ? `?${query.toString()}` : "";
  const path = `/api/v1/drivers/${encodeURIComponent(request.driverId)}/definitions${suffix}`;
  let response: Pick<Response, "ok" | "status" | "json">;
  try {
    response = await fetchImpl(path);
  } catch (e) {
    throw new NckRequestError({
      status: 0,
      code: "NETWORK",
      kind: "",
      message: `目录请求失败：${e instanceof Error ? e.message : String(e)}`,
    });
  }
  if (!response.ok) throw await readError(response as Response);
  const body = await response.json().catch(() => null);
  return parseDefinitionsResponse(body);
}

/** 全量目录（跨页合并后）。 */
export interface NckCatalog {
  driverId: string;
  /** 目录顶层 family（回显语义；多系列为 null）。**不是** Endpoint 的系列意图。 */
  family: string | null;
  revision: number;
  driverVersion: string;
  note: string | null;
  total: number;
  definitions: NckCatalogDefinition[];
  pages: number;
}

export interface NckCatalogLoadOptions {
  driverId: string;
  family?: string;
  limit?: number;
  /** 防御性上限：契约正常时页数 = ceil(total/limit)。 */
  maxPages?: number;
  fetchImpl?: NckFetchLike;
}

/**
 * 读取完整分页目录。通用分页不变量统一委托给 catalogContract；
 * NCK 适配器只负责路径、响应字段与 (variable_id, revision) 身份键。
 */
export async function fetchNckCatalog(options: NckCatalogLoadOptions): Promise<NckCatalog> {
  const fetchImpl = options.fetchImpl ?? fetch;
  let result: CompleteCatalogResult<NckDefinitionsResponse, NckCatalogDefinition>;
  try {
    result = await readCompleteCatalog({
      loadPage: (cursor) =>
        fetchNckCatalogPage(
          {
            driverId: options.driverId,
            family: options.family,
            limit: options.limit,
            cursor,
          },
          fetchImpl,
        ),
      getPage: (page) => ({
        total: page.total,
        nextCursor: page.next_cursor,
        entries: page.definitions,
      }),
      getEntryKey: (definition) => `${definition.variable_id}@${definition.revision}`,
      maxPages: options.maxPages,
    });
  } catch (error) {
    if (error instanceof CatalogContractError) {
      throw new NckRequestError({
        status: 200,
        code: error.code,
        kind: "",
        message: error.message,
      });
    }
    throw error;
  }

  const head = result.firstPage;
  return {
    driverId: head.driver_id,
    family: head.family,
    revision: head.revision,
    driverVersion: head.driver_version,
    note: head.note,
    total: head.total,
    definitions: result.entries,
    pages: result.pages,
  };
}

/**
 * 按 `(variable_id, revision)` 建索引。
 *
 * 为什么需要两个索引：目录允许同一变量发布多个 revision（definition_api.rs:212-216）。
 * 表单按变量选择时用「最新 revision」，而**重新核验已有点位**必须按点位里记录的
 * revision 精确命中——拿最新 revision 去核验旧点位会得出错误结论。
 */
export interface NckDefinitionIndex {
  byVariableId: Record<string, NckCatalogDefinition>;
  byRevision: Record<string, NckCatalogDefinition>;
}

export function indexNckCatalogDefinitions(
  definitions: readonly NckCatalogDefinition[],
): NckDefinitionIndex {
  const byVariableId: Record<string, NckCatalogDefinition> = {};
  const byRevision: Record<string, NckCatalogDefinition> = {};
  for (const definition of definitions) {
    byRevision[`${definition.variable_id}@${definition.revision}`] = definition;
    const existing = byVariableId[definition.variable_id];
    if (!existing || definition.revision > existing.revision) {
      byVariableId[definition.variable_id] = definition;
    }
  }
  return { byVariableId, byRevision };
}

/** 权威身份预览请求。 */
export interface NckIdentityPreviewRequest {
  driverId: string;
  /** 必填、无缺省（definition_api.rs:598-605）。 */
  family: string;
  /** `nck_variable` / `nck_native`。 */
  kind: string;
  /** 与将来持久化的 `PointSelection.parameters` **逐字同构**。 */
  parameters: Record<string, unknown>;
}

/**
 * 调服务端权威身份预览（`POST .../definitions/identity-preview`）。
 *
 * 这是语义点位 `point_key` 的**唯一**来源：Browse 的 `preview_point_key`
 * 只作展示，前端模板替换不得作为可提交来源（start-review §2.3 / D11）。
 */
export async function previewNckPoint(
  request: NckIdentityPreviewRequest,
  fetchImpl: NckFetchLike = fetch,
  signal?: AbortSignal,
): Promise<NckIdentityPreviewResponse> {
  const path = `/api/v1/drivers/${encodeURIComponent(request.driverId)}/definitions/identity-preview`;
  let response: Pick<Response, "ok" | "status" | "json">;
  try {
    response = await fetchImpl(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      // 顶层恰 3 键：family / kind / parameters（driver_query.rs:145-165）。
      body: JSON.stringify({
        family: request.family,
        kind: request.kind,
        parameters: request.parameters,
      }),
      ...(signal ? { signal } : {}),
    });
  } catch (e) {
    // AbortError 也走这里：调用方按「本次请求作废」处理，不会当作成功。
    throw new NckRequestError({
      status: 0,
      code: "NETWORK",
      kind: "",
      message: `身份预览请求失败：${e instanceof Error ? e.message : String(e)}`,
    });
  }
  if (!response.ok) throw await readError(response as Response);
  const body = await response.json().catch(() => null);
  return parseIdentityPreviewResponse(body);
}

/** 预览响应的 6 字段形状校验。 */
export function parseIdentityPreviewResponse(value: unknown): NckIdentityPreviewResponse {
  if (!isPlainObject(value)) throw contractError("身份预览响应不是 JSON 对象");
  const { point_key, identity_mode, scope, variable_id, definition_revision, definition_digest } =
    value;
  if (typeof point_key !== "string" || point_key.trim() === "") {
    throw contractError("身份预览响应的 point_key 必须是非空字符串");
  }
  if (typeof identity_mode !== "string" || identity_mode === "") {
    throw contractError("身份预览响应的 identity_mode 必须是非空字符串");
  }
  if (scope !== null && typeof scope !== "string") {
    throw contractError("身份预览响应的 scope 必须是字符串或 null");
  }
  if (variable_id !== null && typeof variable_id !== "string") {
    throw contractError("身份预览响应的 variable_id 必须是字符串或 null");
  }
  if (definition_revision !== null && typeof definition_revision !== "number") {
    throw contractError("身份预览响应的 definition_revision 必须是数字或 null");
  }
  if (definition_digest !== null && typeof definition_digest !== "string") {
    throw contractError("身份预览响应的 definition_digest 必须是字符串或 null");
  }
  return {
    point_key,
    identity_mode,
    scope: scope as string | null,
    variable_id: variable_id as string | null,
    definition_revision: definition_revision as number | null,
    definition_digest: definition_digest as string | null,
  };
}
