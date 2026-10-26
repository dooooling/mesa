// Endpoint 采集配置窗格（Workspace「采集」tab）。
//
// 保存路径**按 Endpoint 的 Driver 分流**（第二批 A），第二批 B 在此之上接通：
//   I-2 完整分页目录 → scope/ family 准入 → 动态表单 → 服务端权威 Preview
//   → 新增/编辑 → planEndpointTaskSave（唯一保存入口）→ PUT → 回读确认。
//
// 两条数据安全红线：
// - 非 NCK 路径逐字沿用旧行为（`mergeAcquisitionTasks` + 原生命周期）；
// - NCK 路径不得因为「记录里已有 point_key」就认定身份可提交：加载后必须
//   重新核验定义、参数与**服务端重算的 Key**，核验失败只能查看原始配置或
//   显式移除，不能继续保存。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, InputNumber, Tag, Tooltip, message } from "antd";
import { QuestionCircleOutlined } from "@ant-design/icons";
import { api } from "../api";
import type { DriverDescriptor, ValidationIssue } from "../types";
import {
  NCK_POINTS_KIND,
  applyEndpointChange,
  deriveRunState,
  isNckDriver,
  isTaskSnapshotReady,
  selectionsOf,
  splitAcquisitionTasks,
  type AcquisitionTaskShape,
  type LifecycleStepResult,
  type NckPointSelection,
  type ResourceSelection,
  type RunState,
  type TaskSnapshotState,
} from "../deviceModel";
import {
  collectableDefinitions,
  evaluateNckSelectionAdd,
} from "../nckCollectability";
import { nckPointsOf, parseTaskBinding, selectEditableTask } from "../nckTaskBinding";
import {
  endpointSaveChangeState,
  planEndpointTaskSave,
  sameSubmittedConfig,
} from "../endpointSavePlan";
import {
  alignRowIds,
  assignRowIds,
  createRowIdAllocator,
  stableIdentityKey,
} from "../pointRows";
import {
  classifyNckError,
  describeNckFailure,
  fetchNckCatalog,
  indexNckCatalogDefinitions,
  isNckRequestError,
  previewNckPoint,
  type NckCatalog,
  type NckDefinitionIndex,
} from "../nckCatalog";
import {
  checkNckFamily,
  evaluateNckPointIdentity,
  type NckDefinitionLike,
} from "../nckPointModel";
import {
  nativeAddressParameters,
  validateNativeFields,
  validateNativePoint,
} from "../nckNativeModel";
import { applySelectionAdd } from "../resourceSelectionModel";
import { nckPointDisplay } from "../nckPointDisplay";
import { pointSummary } from "../pointSummary";
import {
  UNSUPPORTED_CONFIG_NOTICE,
  unsupportedFieldsInResources,
  type SubmittedResourceSchema,
  type UnsupportedFieldInResource,
} from "../descriptorCompatibility";
import { draftSignature, savedVsDraftText } from "../acquisitionDraft";
import { ApplyWithRestart, type ApplyResult } from "./ApplyWithRestart";
import { NckPointFields } from "./NckPointFields";
import { ResourceBrowseAntd } from "./ResourceBrowseAntd";
import { ResourcePickerAntd } from "./ResourcePickerAntd";

/** 定义目录条目（I-2 `VariableDefinition` 的宽松形态，注入/测试可只给子集）。 */
export type NckDefinitionEntry = NckDefinitionLike;

/**
 * NCK 定义目录（按 `variable_id` 索引）。
 *
 * 生产路径由本组件调用 `nckCatalog.fetchNckCatalog` 获取完整分页目录；
 * 该 prop 保留为**可注入测试缝**（第二批 A 的既有形态），注入时不再发目录请求。
 * 缺失且目录加载失败 ⇒ NCK 保存保持禁用（fail-closed）。
 */
export type NckDefinitionMap = Readonly<Record<string, NckDefinitionEntry>>;

/** 单个点位的身份核验状态（"checking" 只在渲染层出现，不进入提交）。 */
interface NckPointVerification {
  point: NckPointSelection;
  status: "verified" | "unverified";
  code?: string;
  message?: string;
  /** 高级原生点位不适用语义 scope 准入（身份由用户/固定规则给出）。 */
  native?: boolean;
}

/** 用户显式移除的点位：**保留原始配置**，便于确认具体改了什么。 */
interface RemovedNckPoint {
  point: NckPointSelection;
  reason: string;
}

/** 取任务内已知 point_key：两种 kind 都读，未知 kind / 结构非法返回空。 */
function pointKeysOfTask(task: AcquisitionTaskShape): string[] {
  const parsed = parseTaskBinding(task);
  if (parsed.ok && parsed.kind === NCK_POINTS_KIND) {
    return parsed.points.map((point) => point.point_key);
  }
  return selectionsOf(task).flatMap((s) => s.outputs.map((o) => o.point_key));
}

/**
 * 服务端快照里的采集周期（仅 poll 任务有意义）。
 *
 * 缺失/非 poll ⇒ `null`：状态区必须显示「没有已保存的周期」，不得用前端默认值
 * 冒充服务端已生效值。
 */
function snapshotIntervalMs(task: AcquisitionTaskShape | null | undefined): number | null {
  if (!task || task.schedule?.mode !== "poll") return null;
  return typeof task.schedule.interval_ms === "number" ? task.schedule.interval_ms : null;
}

/**
 * 服务端已生效配置的展示快照（P2-2）。
 *
 * 存在意义：状态区必须同时说清「当前生效」与「尚未保存」。只读编辑状态会把
 * 草稿显示成当前配置，用户会以为改动已经生效。
 *
 * `tasks` 是**完整任务基线**（周期 + 每个点位的完整参数 + 保留任务），
 * 保存后回读一致时整份替换；展示用的 `intervalMs` / `pointCount` 只是它的投影。
 */
interface SavedBaseline {
  /** 服务端当前是否存在可编辑的采集任务；false 时没有「已保存」值可对照。 */
  hasTask: boolean;
  intervalMs: number | null;
  /** 已生效点位数（口径与状态区一致：NCK = 点位，非 NCK = 资源选择条数）。 */
  pointCount: number;
  /** 完整任务基线（回读一致后的真值；无任务时为空数组）。 */
  tasks: AcquisitionTaskShape[];
}

/** 保存后回读一致时写入的完整基线（周期 + 点位完整参数 + 保留任务）。 */
interface SaveBaseline {
  tasks: AcquisitionTaskShape[];
  intervalMs: number;
  pointCount: number;
}

/**
 * 「保存结果待核对」的待确认提交（§4.4.3）。
 *
 * 为什么必须记住这些值：PUT 已返回 2xx 但回读失败/不一致时，页面既不能说成功、
 * 也不能刷新基线，更不能直接重新 PUT。用户点「重新核对」时只能拿**同一份提交值**
 * 再比一次；核对一致后还要接着执行用户原本选择的恢复采集（原保存可能停在写入后、
 * 启动前），因此 `restart` / `wasRunning` 必须一起留存。
 */
interface PendingVerification {
  baseline: SaveBaseline;
  /** 用户当时选择的「保存后是否恢复采集」。 */
  restart: boolean;
  /** 本次保存前 Endpoint 是否在运行（决定核对一致后是否需要 start）。 */
  wasRunning: boolean;
  /** 回读失败（请求本身失败）还是回读与提交值不一致。 */
  reason: "read-failed" | "mismatch";
}

/** 保存结果（持久展示，不用一次性 toast；§4.6-4）。 */
type SaveOutcome =
  | { kind: "failed"; message: string }
  | { kind: "state-changed"; message: string }
  | { kind: "pending-verify"; message: string; canResave?: boolean };

/** Endpoint 清单条目（只取本窗格需要的字段，避免把清单形状硬编码进业务判断）。 */
interface EndpointListEntry {
  id: string;
  state?: string;
  runtime?: { state?: string };
  connection?: unknown;
}

/** 运行态未知时禁止保存的统一文案（含「只写配置」，§4.4.1）。 */
const RUN_STATE_UNKNOWN_NOTICE =
  "运行状态未知，已禁用保存（含只写配置）：请先刷新状态，草稿仍可编辑";

/** 回读无法确认时的持久提示（§4.4.3）。 */
const PENDING_VERIFY_NOTICE = "保存结果待核对";

/**
 * 保存后回读任务快照（**所有驱动**的成功判据，§4.4.3）。
 *
 * 为什么需要：`PUT /tasks` 返回 2xx 只证明"服务端接受了请求"，不证明"编辑已
 * 持久化"。旧实现只对 NCK 回读，非 NCK 把 2xx 直接当成功——本步统一为：
 * 回读值与**本次完整提交配置**一致才算成功，否则进入「保存结果待核对」。
 */
async function readBackTasks(endpointId: string): Promise<AcquisitionTaskShape[] | null> {
  const r = await fetch(`/api/v1/tasks?endpoint=${endpointId}`);
  if (!r.ok) return null;
  const j = (await r.json().catch(() => null)) as { tasks?: AcquisitionTaskShape[] } | null;
  return Array.isArray(j?.tasks) ? j.tasks : null;
}

export function EndpointAcquisitionPane({
  endpointId,
  driverId,
  onChanged,
  nckDefinitions,
}: {
  endpointId: string;
  driverId: string;
  onChanged: () => void;
  /** NCK 定义目录注入（测试缝；缺省时由组件从 I-2 目录加载）。 */
  nckDefinitions?: NckDefinitionMap | null;
}) {
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [sels, setSels] = useState<ResourceSelection[]>([]);
  // NCK 待提交点位（来自快照 + 本批新增/编辑；身份一律来自服务端预览）。
  const [nckPoints, setNckPoints] = useState<NckPointSelection[]>([]);
  // 行身份（§4.8）：与 sels / nckPoints **按下标对齐**的稳定 rowId。
  // 只用于界面定位（React key / data-row-id），不参与配置差异判定。
  const [selRowIds, setSelRowIds] = useState<string[]>([]);
  const [nckRowIds, setNckRowIds] = useState<string[]>([]);
  // 分配器只发号不回收：删除后的 id 不复用，避免「删掉的行」与「新行」撞 key。
  const rowIdAllocator = useRef(createRowIdAllocator("row"));
  const [existingTasks, setExistingTasks] = useState<AcquisitionTaskShape[]>([]);
  const [preservedTasks, setPreservedTasks] = useState<AcquisitionTaskShape[]>([]);
  // 现有任务无法安全编辑（多候选 / 结构非法）：保存必须禁用，绝不猜第一个。
  const [editBlock, setEditBlock] = useState<{ code: string; message: string } | null>(null);
  // 运行态**三态**（§4.4.1）：unknown 一律禁止保存（含只写配置）。
  const [runState, setRunState] = useState<RunState>("unknown");
  // 是否已完成一次运行态读取：只用于「未知」提示的展示时机——首次加载期间
  // 不应把「还不知道」渲染成一条警告（§3.3 首屏安静），但保存侧仍按 unknown 禁用。
  const [runStateSettled, setRunStateSettled] = useState(false);
  const [intervalMs, setIntervalMs] = useState(1000);
  const [saving, setSaving] = useState(false);
  // P0-2：运行中是否在应用后恢复运行。提升到父层是为了让底部执行说明与
  // 状态区能显示**真实**路径（勾选状态不再只活在 ApplyWithRestart 内部）。
  const [restart, setRestart] = useState(true);
  // 上一次保存的真实落点（由 LifecycleOutcome 映射；不猜、不提前宣告成功）。
  const [lastApply, setLastApply] = useState<ApplyResult | null>(null);
  // 任务快照门（P0 fail-closed，与 DeviceDetail 点位 Modal 同门）：
  // 只有 ready + 归属当前 Endpoint 才允许 PUT；loading/error 一律禁用保存，
  // 绝不能把 [] 当作“服务端没有任务”去覆盖 task-a/b/c。
  const [tasksLoadState, setTasksLoadState] = useState<TaskSnapshotState>("idle");
  const [tasksLoadedEpId, setTasksLoadedEpId] = useState<string | null>(null);
  const [tasksLoadError, setTasksLoadError] = useState("");
  // 保存失败的后端 issues 原样展示（`{valid:false, issues:[...]}`）。
  const [saveIssues, setSaveIssues] = useState<ValidationIssue[]>([]);
  // P2-2：服务端已生效配置的展示快照（null = 快照未就绪/不可用）。
  // 状态区据此区分「已保存」与「待保存」，不再让草稿冒充当前配置。
  const [savedBaseline, setSavedBaseline] = useState<SavedBaseline | null>(null);
  // 回读无法确认（PUT 已发出但结果未知）：不报成功、不刷新基线、不自动重发 PUT。
  const [pendingVerify, setPendingVerify] = useState<PendingVerification | null>(null);
  const [rechecking, setRechecking] = useState(false);
  // 持久保存结果（失败/状态变化/待核对）；成功由 ApplyWithRestart 的结果条展示一次。
  const [saveOutcome, setSaveOutcome] = useState<SaveOutcome | null>(null);
  // 「配置已保存但启动失败」：草稿标记已清除，但处理入口必须保留（可跨新修改存活）。
  const [restartFailed, setRestartFailed] = useState<string | null>(null);

  // 过渡态（CONNECTING / RECONNECTING）按「运行中」路径处理（先 stop 再写入）。
  const running = runState === "running";

  // ---- 第二批 B 新增状态 -------------------------------------------------
  // Endpoint 有效连接配置的 family（权威来源；目录顶层 family 只是回显）。
  const [endpointFamily, setEndpointFamily] = useState<string | null>(null);
  const [endpointFamilyState, setEndpointFamilyState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  // I-2 目录（组件自行加载；注入 `nckDefinitions` 时跳过）。
  const [catalog, setCatalog] = useState<NckCatalog | null>(null);
  const [catalogState, setCatalogState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [catalogError, setCatalogError] = useState("");
  // 已有点位的重新核验结果（不是「有 point_key 就有效」）。
  const [pointStatuses, setPointStatuses] = useState<NckPointVerification[]>([]);
  const [verifying, setVerifying] = useState(false);
  // 用户显式移除的点位及原因（原始配置保留展示）。
  const [removedPoints, setRemovedPoints] = useState<RemovedNckPoint[]>([]);
  // 编辑态：null = 新增；否则为 nckPoints 下标。
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [formVersion, setFormVersion] = useState(0);
  // Browse → 语义表单的预填（A1/A16 的入口）。preview_point_key 不参与提交。
  const [browseOpen, setBrowseOpen] = useState(false);
  const [browsePreset, setBrowsePreset] = useState<NckPointSelection | null>(null);

  const nck = isNckDriver(driverId);
  const catalogIndex = useMemo<NckDefinitionIndex | null>(
    () => (catalog ? indexNckCatalogDefinitions(catalog.definitions) : null),
    [catalog],
  );
  const definitions = useMemo<NckDefinitionMap | null>(() => {
    if (nckDefinitions) return nckDefinitions;
    return catalogIndex ? catalogIndex.byVariableId : null;
  }, [nckDefinitions, catalogIndex]);
  const catalogFamily = catalog?.family ?? null;
  const definitionList = useMemo<NckDefinitionEntry[]>(
    () => (definitions ? Object.values(definitions) : []),
    [definitions],
  );
  // UI 执行点①（P0-1）：只有 `scope === "collect"` 的定义才进入可采集集合。
  const collectableDefs = useMemo(
    () => collectableDefinitions(definitionList),
    [definitionList],
  );
  // native 12 字段 Schema 的唯一来源：Descriptor 的 variable 资源（不硬编码）。
  const nativeSchema = useMemo(() => {
    if (!nck || !desc) return null;
    return desc.resources.find((r) => r.id === "variable")?.parameters ?? null;
  }, [nck, desc]);

  /** 按 `(variable_id, revision)` 精确查定义（多 revision 目录不得串用）。 */
  const lookupDefinition = useCallback(
    (variableId: string, revision: number | null): NckDefinitionLike | null => {
      if (!definitions) return null;
      if (catalogIndex && revision !== null) {
        const exact = catalogIndex.byRevision[`${variableId}@${revision}`];
        if (exact) return exact;
      }
      const entry = definitions[variableId];
      if (!entry) return null;
      if (typeof entry.revision === "number" && revision !== null && entry.revision !== revision) {
        return null;
      }
      return entry;
    },
    [definitions, catalogIndex],
  );

  /**
   * 重新核验一个已有点位（语义点位）。
   *
   * 顺序刻意是「定义 → scope → family → 预览 → Key 逐字比对」：任何一步失败都
   * 给出具体原因，但都**不**自动删除、不静默转换、也不放行其余点位。
   */
  const verifyOne = useCallback(
    async (point: NckPointSelection): Promise<NckPointVerification> => {
      if (!nck) return { point, status: "verified" };
      if (point.kind === "nck_native") {
        // 高级原生：不适用目录 scope，但身份规则仍必须复核：
        // derived 用固定规则重算，user 只校验 native. 语法；字段再按 Descriptor
        // Schema 校验。任一失败都不得作为有效点位继续提交。
        if (!nativeSchema) {
          return {
            point,
            status: "unverified",
            code: "PARAMETERS_INVALID",
            message: "Descriptor 的 variable 资源不可用：无法校验原生字段（fail-closed）",
          };
        }
        const nativeCheck = validateNativePoint(point);
        if (!nativeCheck.ok) {
          return {
            point,
            status: "unverified",
            code: nativeCheck.code,
            message: `${nativeCheck.message}（原始配置可查看/移除）`,
          };
        }
        if (nativeSchema) {
          const issues = validateNativeFields(
            nativeSchema,
            nativeAddressParameters(point.parameters),
          );
          if (issues.length) {
            return {
              point,
              status: "unverified",
              code: "PARAMETERS_INVALID",
              message: `原生字段不满足 Descriptor 约束：${issues
                .map((i) => i.message)
                .join("；")}`,
            };
          }
        }
        return { point, status: "verified", native: true };
      }
      if (point.kind !== "nck_variable") {
        return {
          point,
          status: "unverified",
          code: "PARAMETERS_INVALID",
          message: `未知 Point Kind "${point.kind}"：无法确认身份规则`,
        };
      }
      const variableId =
        typeof point.parameters.variable_id === "string" ? point.parameters.variable_id : "";
      const revision =
        typeof point.parameters.definition_revision === "number"
          ? point.parameters.definition_revision
          : null;
      if (!variableId || revision === null) {
        return {
          point,
          status: "unverified",
          code: "PARAMETERS_INVALID",
          message: "点位 parameters 缺少 variable_id / definition_revision，无法重新核验",
        };
      }
      const definition = lookupDefinition(variableId, revision);
      if (!definition) {
        return {
          point,
          status: "unverified",
          code: "DEFINITION_MISSING",
          message: `定义目录中找不到 ${variableId}（rev ${revision}）：无法重新核验身份（原始配置可查看/移除）`,
        };
      }
      const admission = evaluateNckSelectionAdd(definition);
      if (!admission.ok) {
        return {
          point,
          status: "unverified",
          code: admission.code,
          message: `${admission.message}（原始配置可查看/移除）`,
        };
      }
      const familyCheck = checkNckFamily({
        endpointFamily,
        definitionFamily: definition.family,
        catalogFamily,
      });
      if (!familyCheck.ok) {
        return { point, status: "unverified", code: familyCheck.code, message: familyCheck.message };
      }
      try {
        const preview = await previewNckPoint({
          driverId,
          family: familyCheck.family,
          kind: "nck_variable",
          parameters: point.parameters,
        });
        const check = evaluateNckPointIdentity({
          point,
          definition,
          endpointFamily,
          catalogFamily,
          preview,
        });
        return check.ok
          ? { point, status: "verified" }
          : { point, status: "unverified", code: check.code, message: check.message };
      } catch (e) {
        const code = isNckRequestError(e) ? e.code : "PREVIEW_FAILED";
        const detail = e instanceof Error ? e.message : String(e);
        return {
          point,
          status: "unverified",
          code,
          message: `服务端身份预览失败（${code}）：${detail}`,
        };
      }
    },
    [nck, lookupDefinition, endpointFamily, catalogFamily, driverId, nativeSchema],
  );

  const verifyPoints = useCallback(
    async (points: NckPointSelection[]): Promise<NckPointVerification[]> => {
      const out: NckPointVerification[] = [];
      for (const point of points) out.push(await verifyOne(point));
      return out;
    },
    [verifyOne],
  );

  /** 核验结果是否对应当前点位数组（对象引用 + 顺序同时匹配，避免错位复用）。 */
  const statusesCurrent =
    pointStatuses.length === nckPoints.length &&
    pointStatuses.every((status, i) => status.point === nckPoints[i]);
  const unverified = statusesCurrent
    ? pointStatuses.filter((status) => status.status === "unverified")
    : [];
  const verificationReady = statusesCurrent && !verifying;

  // ---- 目录加载（I-2 完整分页） -----------------------------------------
  useEffect(() => {
    if (!nck || nckDefinitions) {
      setCatalog(null);
      setCatalogState("idle");
      setCatalogError("");
      return;
    }
    let cancelled = false;
    setCatalogState("loading");
    setCatalogError("");
    setCatalog(null);
    fetchNckCatalog({ driverId })
      .then((loaded) => {
        if (cancelled) return;
        setCatalog(loaded);
        setCatalogState("ready");
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setCatalog(null);
        setCatalogState("error");
        setCatalogError(
          isNckRequestError(e)
            ? `${e.code}（HTTP ${e.status}，${describeNckFailure(
                classifyNckError(e.status, e.kind),
              )}）：${e.message}`
            : e instanceof Error
              ? e.message
              : String(e),
        );
      });
    return () => {
      cancelled = true;
    };
  }, [nck, driverId, nckDefinitions]);

  // ---- 运行态三态（§4.4.1 / §6-2） ---------------------------------------
  //
  // 与 family 共用同一次 `GET /api/v1/endpoints`（family 的唯一权威来源也是它）。
  // 关键点：**每次读取前先把状态复位为 unknown**——读取失败、Endpoint 不在清单、
  // 未知取值都不得沿用上一次的值（旧实现 catch 里不复位，于是切换连接后仍显示
  // 上一台的运行态，保存路径也跟着错）。
  const runStateToken = useRef(0);

  /**
   * 重新读取运行态 + Endpoint family。
   *
   * 返回本次读到的三态（`null` = 已被更新的读取取代，调用方应放弃后续动作）。
   * `unknown` 是**正常返回值**，不是异常：它表示「无法确认」，调用方必须禁止保存。
   *
   * `resetFirst`：读取前是否先把状态复位为 unknown。
   * - 刷新/加载路径必须复位（fail-closed：读不到就不得沿用旧值）；
   * - **保存前复核**不复位：复核期间界面不应闪成「状态未知」，且此时尚未发出任何
   *   写请求；读取失败仍会在 catch 里落成 unknown。
   */
  const loadRunState = useCallback(
    async (opts: { resetFirst?: boolean } = {}): Promise<RunState | null> => {
      const token = ++runStateToken.current;
      if (opts.resetFirst !== false) setRunState("unknown");
      try {
        const j = (await api.listEndpoints()) as { endpoints?: EndpointListEntry[] };
        if (token !== runStateToken.current) return null;
        const list = j?.endpoints ?? [];
        const live = list.find((e) => e.id === endpointId);
        const next = deriveRunState(live?.state ?? live?.runtime?.state);
        setRunState(next);
        setRunStateSettled(true);
        // family 的唯一权威来源 = Endpoint 有效连接配置（不是目录顶层回显）。
      const connection = live?.connection;
      const rawFamily =
        connection && typeof connection === "object" && !Array.isArray(connection)
          ? (connection as { family?: unknown }).family
          : undefined;
      setEndpointFamily(
        typeof rawFamily === "string" && rawFamily.trim() ? rawFamily.trim() : null,
      );
      setEndpointFamilyState("ready");
      return next;
    } catch {
      if (token !== runStateToken.current) return null;
      // 读取失败 ⇒ 运行态未知（不是「已停止」）；family 同样不可信。
      setRunState("unknown");
      setRunStateSettled(true);
      setEndpointFamily(null);
      setEndpointFamilyState("error");
      return "unknown";
    }
  }, [endpointId]);

  useEffect(() => {
    // 切 Endpoint 时先复位：旧快照不得污染新窗格（fail-closed 默认禁用保存）。
    setSels([]);
    setSelRowIds([]);
    setNckPoints([]);
    setNckRowIds([]);
    setEditBlock(null);
    setExistingTasks([]);
    setPreservedTasks([]);
    setIntervalMs(1000);
    setRestart(true);
    setLastApply(null);
    setDesc(null);
    setTasksLoadState("loading");
    setTasksLoadedEpId(null);
    setTasksLoadError("");
    setSaveIssues([]);
    setSavedBaseline(null);
    setPointStatuses([]);
    setRemovedPoints([]);
    setEditingIndex(null);
    setFormVersion(0);
    setBrowseOpen(false);
    setBrowsePreset(null);
    setEndpointFamily(null);
    setEndpointFamilyState("loading");
    // 运行态必须立即复位为未知：切换连接后不得沿用上一台的值（§4.4.1）。
    setRunState("unknown");
    setRunStateSettled(false);
    // 上一台的待核对/失败/启动失败结论同样不得带过来。
    setPendingVerify(null);
    setSaveOutcome(null);
    setRestartFailed(null);
    let cancelled = false;
    // Descriptor + Task 快照并行加载；二者都成功才开放保存（NCK 只需要任务快照，
    // 因为它的点位来自定义目录/快照而不是 Descriptor 的 resources）。
    Promise.all([
      fetch(`/api/v1/drivers/${driverId}/descriptor`)
        .then((x) => x.json())
        .catch(() => null),
      fetch(`/api/v1/tasks?endpoint=${endpointId}`)
        .then(async (x) => {
          if (!x.ok) throw new Error(`GET /tasks ${x.status}`);
          return (await x.json()) as { tasks?: AcquisitionTaskShape[] };
        })
        .catch((e) => ({ error: e as unknown })),
    ]).then(([d, tasksRes]) => {
      if (cancelled) return;
      // 只接受形态合法的 Descriptor：错误包不得 masquerade 成描述。
      if (d && Array.isArray((d as { resources?: unknown }).resources)) setDesc(d);
      if (tasksRes && !(tasksRes as { error?: unknown }).error) {
        const tasks = ((tasksRes as { tasks?: AcquisitionTaskShape[] }).tasks ?? []) as AcquisitionTaskShape[];
        setExistingTasks(tasks);
        // 提示文案用的保留任务 id（两种 kind 各自的分支里填写；不改旧文案口径）。
        let preservedIds: string[] = [];
        if (isNckDriver(driverId)) {
          // NCK：按 kind 选**唯一**可编辑任务；歧义/结构非法 ⇒ 拒绝编辑（不取第一个）。
          const selected = selectEditableTask({ tasks, driverId });
          if (!selected.ok) {
            setEditBlock({ code: selected.code, message: selected.message });
            setPreservedTasks([]);
            setNckPoints([]);
            setNckRowIds([]);
            // 现有任务无法安全编辑 ⇒ 没有可信的「已生效」值可对照。
            setSavedBaseline({ hasTask: false, intervalMs: null, pointCount: 0, tasks: [] });
          } else {
            setPreservedTasks(selected.preserved);
            preservedIds = selected.preserved.map((t) => t.id);
            if (selected.editable?.schedule?.mode === "poll") {
              setIntervalMs(selected.editable.schedule.interval_ms ?? 1000);
            }
            // 只读 `{points:[…]}`：绝不把 NCK 点位当成 ResourceSelection[]。
            const loadedPoints = nckPointsOf(selected.editable);
            setNckPoints(loadedPoints);
            // 行身份在**加载快照**时分配一次，之后编辑/搜索/分页都不再重发（§4.8）。
            setNckRowIds(assignRowIds(loadedPoints.length, rowIdAllocator.current));
            // P2-2 基线：快照里已生效的完整任务集（保存回读一致后才整份替换）。
            setSavedBaseline({
              hasTask: !!selected.editable,
              intervalMs: snapshotIntervalMs(selected.editable),
              pointCount: loadedPoints.length,
              tasks,
            });
          }
        } else {
          const { editable, preserved } = splitAcquisitionTasks(tasks);
          setPreservedTasks(preserved);
          preservedIds = preserved.map((t) => t.id);
          const loadedSelections = editable ? selectionsOf(editable) : [];
          if (editable?.schedule?.mode === "poll") {
            setIntervalMs(editable.schedule.interval_ms ?? 1000);
          }
          if (loadedSelections.length) {
            setSels(loadedSelections);
            setSelRowIds(assignRowIds(loadedSelections.length, rowIdAllocator.current));
          }
          // P2-2 基线：快照里已生效的完整任务集（保存回读一致后才整份替换）。
          setSavedBaseline({
            hasTask: !!editable,
            intervalMs: snapshotIntervalMs(editable),
            pointCount: loadedSelections.length,
            tasks,
          });
        }
        // 快照就绪：只有此时保存才允许 PUT（空数组即服务端真的无任务）。
        setTasksLoadedEpId(endpointId);
        setTasksLoadState("ready");
        if (tasks.length) {
          const editableLabel = isNckDriver(driverId) ? "NCK 任务" : "canonical 任务";
          message.info(
            preservedIds.length
              ? `已回显 ${editableLabel}，另有 ${preservedIds.length} 个任务将被保留（${preservedIds.join("、")}）`
              : `已回显 ${tasks.length} 任务`,
          );
        }
      } else {
        // Task 快照失败：fail-closed——显示错误并禁用保存，不拿 [] 去覆盖服务端。
        setTasksLoadState("error");
        setTasksLoadError("任务快照加载失败：服务端任务集未知，已禁用保存（请切换后重试，不会覆盖已有任务）。");
      }
    });
    // 运行态与 family 走同一次 GET（loadRunState 内部复位为 unknown，
    // 读取失败/清单缺失都不会沿用上一台的值）。
    void loadRunState();
    return () => {
      cancelled = true;
      // 让在途的运行态读取作废：切换连接后它的结果不得回写到新窗格。
      runStateToken.current += 1;
    };
  }, [endpointId, driverId, loadRunState]);

  // ---- 已有点位重新核验（加载后 & 点位变化后） ---------------------------
  const verifyToken = useRef(0);
  useEffect(() => {
    if (!nck || !definitions) {
      setVerifying(false);
      return;
    }
    const token = ++verifyToken.current;
    let cancelled = false;
    const snapshot = nckPoints;
    setVerifying(true);
    void verifyPoints(snapshot).then((statuses) => {
      if (cancelled || token !== verifyToken.current) return;
      setPointStatuses(statuses);
      setVerifying(false);
    });
    return () => {
      cancelled = true;
    };
  }, [nck, definitions, nckPoints, verifyPoints]);

  // 编辑一变，上一次的「已保存」结论立即失效：否则用户改完参数仍看到
  // “配置已保存，当前停止”，会把旧结论误当成新配置的结论。
  //
  // P2-1：指纹必须是**完整有效配置**（周期 + 每个点位的完整参数 + 顺序），
  // 不能只比 point_key / resource_id——同一原生点位只改 decode/count 时键不变，
  // 旧的「已保存」会残留。
  const editSignature = useMemo(
    () =>
      draftSignature({
        intervalMs,
        selections: nck ? undefined : sels,
        nckPoints: nck ? nckPoints : undefined,
      }),
    [intervalMs, nck, sels, nckPoints],
  );
  useEffect(() => {
    setLastApply(null);
  }, [editSignature]);

  /** 所有已占用 key：保留任务 + 当前点位（编辑时排除被编辑项自身）。 */
  const takenPointKeys = (skipIndex: number | null): string[] => {
    const keys = preservedTasks.flatMap(pointKeysOfTask);
    nckPoints.forEach((point, i) => {
      if (skipIndex !== null && i === skipIndex) return;
      keys.push(point.point_key);
    });
    return keys;
  };

  const addPoint = (point: NckPointSelection) => {
    if (takenPointKeys(null).includes(point.point_key)) {
      return message.error("该 point_key 已被 Endpoint 内其它点位占用：请调整 Target/Arguments（不得自动改名/加后缀）");
    }
    setNckPoints((prev) => [...prev, point]);
    // 新行分配**新** rowId（删除过的 id 不复用）；既有行身份不变（§4.8）。
    setNckRowIds((prev) => [...prev, rowIdAllocator.current.next()]);
    // 表单重建一次：避免上一条目的输入（含 Browse 预填）残留到下一次新增。
    setBrowsePreset(null);
    setFormVersion((v) => v + 1);
  };

  const replacePoint = (index: number, point: NckPointSelection) => {
    if (takenPointKeys(index).includes(point.point_key)) {
      return message.error("该 point_key 已被 Endpoint 内其它点位占用：请调整 Target/Arguments（不得自动改名/加后缀）");
    }
    setNckPoints((prev) => prev.map((p, i) => (i === index ? point : p)));
    // 编辑**不**改 rowId：同一行换了 point_key/参数，仍然是同一行（§4.8）。
    setEditingIndex(null);
    setFormVersion((v) => v + 1);
  };

  /**
   * 显式移除点位（存量 identity / 核验失败点位的**唯一恢复路径**）。
   *
   * 关键：本函数**刻意不经过** scope 准入门、不查 family、不要求 Preview。
   * 准入只约束「作为有效点位提交」，不约束「从既有载荷中移除」；若删除也要先
   * 把非法点位「变成合法」，就会形成死锁——那正是本批要避免的缺陷。
   *
   * 移除前先把**原始配置**记下来，用户可确认自己到底改了什么。
   */
  const removePoint = (index: number) => {
    const point = nckPoints[index];
    if (!point) return;
    const status = pointStatuses.find((s) => s.point === point);
    const reason =
      status?.status === "unverified"
        ? `${status.code ?? "UNVERIFIED"}：${status.message ?? "身份核验未通过"}`
        : "用户显式移除（原始配置已保留）";
    setRemovedPoints((prev) => [{ point, reason }, ...prev]);
    setNckPoints((prev) => prev.filter((_, i) => i !== index));
    // 移除只删掉**这一行**的 rowId：其余行的身份与顺序不变（不重新发号）。
    setNckRowIds((prev) => prev.filter((_, i) => i !== index));
    setEditingIndex((current) => {
      if (current === null) return null;
      if (current === index) return null;
      return current > index ? current - 1 : current;
    });
  };

  /**
   * UI 执行点②（P0-1 双执行）：对**将要提交的点位**再查一次准入，不信任列表过滤
   * 或选中态；提交模型（endpointSavePlan → validateNckTaskSubmission）还会再查一次。
   * 注意：本判定只约束「作为有效点位提交」，**不**约束移除（见 removePoint）。
   */
  const admissionIssues = useMemo(() => {
    if (!nck) return [] as Array<{ pointKey: string; code: string; message: string }>;
    return nckPoints.flatMap((point) => {
      // 原生点位（高级模式）不适用目录 scope：身份由用户/固定规则给出。
      if (point.kind !== "nck_variable") return [];
      const variableId = point.parameters?.variable_id;
      const revision =
        typeof point.parameters?.definition_revision === "number"
          ? point.parameters.definition_revision
          : null;
      const definition =
        typeof variableId === "string" ? lookupDefinition(variableId, revision) : null;
      const gate = evaluateNckSelectionAdd(definition);
      return gate.ok
        ? []
        : [{ pointKey: point.point_key, code: gate.code, message: gate.message }];
    });
  }, [nck, nckPoints, lookupDefinition]);

  /**
   * P1（最终保存守卫的唯一依据）：**本次提交涉及资源**的完整 Schema。
   *
   * 为什么不能只查「值里已有的键」（旧实现）：驱动升级后新增一个界面不认识的
   * **必填**字段时，旧配置尚未携带该键——编辑表单因为扫完整 Schema 已被禁用，
   * 只查 key 存在性的守卫却会放行，形成「表单说不行、保存却通过」的缺口。
   *
   * 影响范围仍然受限（不检查未使用的其它资源）：
   * - 非 NCK：只取 `sels` 里真实出现的 `resource_id` 对应的 Schema；
   * - NCK：语义点位参数来自定义目录，不经过 Descriptor Schema；只有本次提交里
   *   含原生点位时才涉及 `variable` 资源。
   */
  const submittedResources = useMemo<SubmittedResourceSchema[]>(() => {
    if (!desc) return [];
    if (nck) {
      if (!nativeSchema) return [];
      const nativeValues = nckPoints
        .filter((point) => point.kind === "nck_native")
        .map((point) => point.parameters);
      return nativeValues.length
        ? [{ resourceId: "variable", schema: nativeSchema, values: nativeValues }]
        : [];
    }
    const byResource = new Map<string, Array<Record<string, unknown>>>();
    for (const sel of sels) {
      const values = byResource.get(sel.resource_id) ?? [];
      values.push(sel.parameters ?? {});
      byResource.set(sel.resource_id, values);
    }
    return Array.from(byResource.entries()).flatMap(([resourceId, values]) => {
      const schema = (desc.resources ?? []).find((r) => r.id === resourceId)?.parameters;
      return schema ? [{ resourceId, schema, values }] : [];
    });
  }, [desc, nck, nativeSchema, nckPoints, sels]);

  /**
   * A3/P1：本次提交涉及资源里**界面不认识**的字段类型（按完整 Schema 判定）。
   *
   * 同时用 `unsupportedFieldsInValue` 标注该字段是否已被存量配置携带：
   * - 已携带 ⇒ 这条配置原本就带着未知字段（老问题）；
   * - 未携带 ⇒ 驱动新增/升级引入，旧配置根本没机会填（本次修复的缺口）。
   *
   * 影响范围：仅当前 Endpoint 的保存按钮；不停已运行的采集，也不阻断其它连接。
   */
  const incompatible = useMemo<UnsupportedFieldInResource[]>(
    () => unsupportedFieldsInResources(submittedResources),
    [submittedResources],
  );
  const hasIncompatible = incompatible.length > 0;

  /**
   * 回读一致后写入**完整任务基线**（§4.4.3 步骤 2）。
   *
   * 为什么连 `existingTasks` 一起换：它才是下一次「无实际修改判定」的对照物。
   * 只更新展示计数/点位列表，会让刚保存完的页面再点一次保存又发一遍 PUT
   * （判据仍拿旧快照比较）。基线必须同时覆盖周期、每个点位完整参数、保留任务。
   */
  const applyVerifiedBaseline = useCallback((baseline: SaveBaseline) => {
    setExistingTasks(baseline.tasks);
    setSavedBaseline({
      hasTask: true,
      intervalMs: baseline.intervalMs,
      pointCount: baseline.pointCount,
      tasks: baseline.tasks,
    });
  }, []);

  /** 只执行「启动」步骤（待核对核对一致后的恢复采集、启动失败后的重试共用）。 */
  const startOnly = async (): Promise<void> => {
    const r = await api.startEndpoint(endpointId).catch((e) => ({
      status: -1,
      body: { error: { message: e instanceof Error ? e.message : String(e) } },
    }));
    if (r.status === 200) {
      setRunState("running");
      setLastApply("restarted");
      setRestartFailed(null);
      setSaveOutcome(null);
    } else {
      const msg =
        (r.body as { error?: { message?: string } })?.error?.message ?? `恢复采集失败（${r.status}）`;
      setRunState("stopped");
      setLastApply("restart-failed");
      setRestartFailed(msg);
    }
    onChanged();
  };

  /**
   * 启动失败后的**重试启动**：只调用启动步骤，不 stop、不 PUT（§4.5）。
   *
   * 配置在回读一致时已经确认保存（基线已刷新），重复整套保存只会把已生效的
   * 配置再覆盖一次，并多出一次不必要的采集中断。
   */
  const retryStart = async () => {
    if (saving || rechecking) return;
    setSaving(true);
    try {
      await startOnly();
    } finally {
      setSaving(false);
    }
  };

  /**
   * 「保存结果待核对」→ 重新核对（§4.4.3）：只再次 GET 比对，**绝不**重新 PUT。
   * - 一致 ⇒ 刷新完整基线，并继续执行用户原本选择的恢复采集；
   * - 不一致 ⇒ 仍为待核对，此时才允许用户主动「重新保存」（重新走完整保存前检查）。
   */
  const recheckPending = async () => {
    const pending = pendingVerify;
    if (!pending || saving || rechecking) return;
    setRechecking(true);
    try {
      const back = await readBackTasks(endpointId).catch(() => null);
      if (back === null) {
        setSaveOutcome({
          kind: "pending-verify",
          message: `${PENDING_VERIFY_NOTICE}：回读失败，仍无法确认配置是否已持久化（未刷新基线，未重复写入）`,
        });
        return;
      }
      if (!sameSubmittedConfig(back, pending.baseline.tasks)) {
        setSaveOutcome({
          kind: "pending-verify",
          message: `${PENDING_VERIFY_NOTICE}：回读不一致（服务端当前配置与本次提交不同；未刷新基线，未重复写入）。可重新核对，或主动重新保存`,
          canResave: true,
        });
        return;
      }
      applyVerifiedBaseline(pending.baseline);
      setPendingVerify(null);
      setSaveOutcome(null);
      setSaveIssues([]);
      // 原保存可能停在「写入后、启动前」：核对一致后继续用户选择的恢复采集。
      if (pending.wasRunning && pending.restart) {
        await startOnly();
      } else {
        setRunState("stopped");
        setLastApply("stopped");
        onChanged();
      }
    } finally {
      setRechecking(false);
    }
  };

  const save = async (restart: boolean, opts: { fromPendingResave?: boolean } = {}) => {
    // 保存中不允许并发发起第二次（复核步骤本身也要时间）。
    if (saving) return;
    // 「保存结果待核对」期间**不得直接重新 PUT**：主操作只能是「重新核对」。
    // 唯一例外是用户在核对确认「未写入」后主动点「重新保存」（显式重走完整保存前检查）。
    if (pendingVerify && !opts.fromPendingResave) {
      return message.warning(
        "保存结果待核对：请先「重新核对」确认服务端当前配置，再决定是否重新保存（不会自动重复写入）",
      );
    }
    // 运行态未知 ⇒ 禁止**任何形式**的保存，包括「只写配置」（§4.4.1 决定 1）。
    // 这是最外层的门：位置刻意放在快照门与 NCK 提交前复核之前——未知状态下
    // 连身份预览都不发，更不会有 stop / PUT / start。草稿仍可编辑。
    if (runState === "unknown") {
      return message.error(RUN_STATE_UNKNOWN_NOTICE);
    }
    // 快照门：pending / 失败 / 串 Endpoint 一律不可 PUT。
    if (!isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId)) {
      if (tasksLoadState === "error") return message.error("任务快照加载失败，禁止保存以防覆盖已有任务。请切换后重试。");
      return message.warning("任务快照加载中，禁止保存以防覆盖已有任务。请稍候。");
    }
    // A3/P1 提交门：本次提交涉及资源的**完整 Schema** 里存在界面不认识的字段
    // 类型时，拒绝整份保存。必须放在任何 stop / PUT / start（以及提交前的
    // identity-preview）之前——拒绝时不得发出任何请求。
    // 只影响当前 Endpoint 的配置；不停采集、不阻断其它连接。
    if (hasIncompatible) {
      return message.error(UNSUPPORTED_CONFIG_NOTICE);
    }
    // 描述门（仅非 NCK）：旧路径的选型来自 Descriptor，缺失时不得凭空回写。
    if (!nck && !desc) return message.error("资源描述加载失败，禁止保存。请切换后重试。");
    // NCK：现有任务不可安全编辑时，连 PUT 都不发（否则会覆盖正确配置）。
    if (editBlock) return message.error(`现有任务无法安全编辑，已禁止保存：${editBlock.message}`);
    // UI 层准入（提交模型还会再查一次）：存在不可采集点位时不允许发起保存。
    if (admissionIssues.length) {
      return message.error(
        `存在不可采集点位（scope 准入）：${admissionIssues
          .map((i) => `${i.pointKey}（${i.code}）`)
          .join("、")}`,
      );
    }
    if (nck) {
      if (!definitions) {
        return message.error("定义目录不可用：无法确认 scope/family，禁止提交（fail-closed）");
      }
      if (!endpointFamily) {
        return message.error("Endpoint 连接配置缺少 family：无法确认系列，禁止提交（请先补齐连接配置）");
      }
      if (verifying || !verificationReady) {
        return message.warning("正在重新核验已有点位身份，请稍候（核验完成前不会发送保存请求）");
      }
      if (unverified.length) {
        return message.error(
          `存在未通过身份核验的点位（${unverified.length} 项）：请先手动移除这些点位并重新验证剩余点位后再保存`,
        );
      }
    }
    // 唯一保存计划入口：按 Driver 分流 + 快照门 + 准入 + 冲突 + 变更判定。
    // 刻意排在「提交前重新核验」（会发 identity-preview 请求）之前：
    // 无实际修改时连预览请求都不该发（§4.4.2）。
    const plan = planEndpointTaskSave({
      driverId,
      tasks: existingTasks,
      snapshotState: tasksLoadState,
      loadedEndpointId: tasksLoadedEpId,
      currentEndpointId: endpointId,
      intervalMs,
      selections: sels,
      nckPoints,
      nckDefinitions: definitions,
    });
    if (!plan.ok) {
      // 非 NCK 空选择：保持旧提示层级（warning）与文案。
      if (plan.code === "EMPTY_SELECTION") return message.warning(plan.message);
      // 无实际修改（**所有驱动**统一拦截）：不发 PUT、不发 stop、不发 start，
      // 提示为 info 而不是成功——"保存接口 2xx"不等于"编辑已持久化"（§4.4.2）。
      if (plan.code === "NO_CHANGES") return message.info(plan.message);
      // 其余（歧义 / 结构非法 / 准入 / 冲突 / 快照 / 目录缺失）一律明确失败。
      return message.error(plan.message);
    }
    const submittedTasks = plan.tasks;
    // ---- 保存前重新确认运行状态（§4.4.1）--------------------------------
    // 打开页面时读到的状态只用于渲染；用户可能已经编辑很久。这里在**任何**
    // stop / PUT / start 之前重新读一次：不一致即中止，由用户再次点击保存
    // （不额外弹确认框）。注意：这只降低「用过期状态决策」的概率，
    // **不提供并发原子保证**（§4.4.4，后端没有条件写/版本号）。
    // 复核期间保持界面现有状态（不闪成「状态未知」）；读取失败仍会落成 unknown。
    const confirmed = await loadRunState({ resetFirst: false });
    if (confirmed === null) return; // 页面已切换，放弃本次保存
    if (confirmed === "unknown" || confirmed !== runState) {
      setLastApply(null);
      setSaveOutcome({
        kind: "state-changed",
        message:
          confirmed === "unknown"
            ? "运行状态已变化或无法确认，本次保存未执行（未发出任何停止/写入/启动请求）。请刷新状态后再次点击保存"
            : "运行状态已变化，本次保存未执行（未发出任何停止/写入/启动请求）。请确认状态后再次点击保存",
      });
      return;
    }
    const wasRunning = confirmed === "running";
    // 提交前再核验一次（NCK）：防止「加载时通过、提交时目录/定义已变」。
    // 放在保存计划与状态复核之后：无实际修改、状态已变化时都不会因为一次
    // 无意义的保存触发 identity-preview 请求。这一步仍早于任何 stop / PUT / start。
    if (nck) {
      const fresh = await verifyPoints(nckPoints);
      setPointStatuses(fresh);
      const stillBad = fresh.filter((s) => s.status === "unverified");
      if (stillBad.length) {
        return message.error(
          `保存前重新核验失败（${stillBad.length} 项）：${stillBad
            .map((s) => `${s.point.point_key}（${s.code}）`)
            .join("、")}；请先移除后再保存`,
        );
      }
    }
    // 本次提交的**完整基线**：周期 + 每个点位的完整参数 + 保留任务（= 整份任务集）。
    // 回读一致后整份写入，作为下一次「无实际修改判定」的对照物。
    const baseline: SaveBaseline = {
      tasks: submittedTasks,
      intervalMs,
      pointCount: nck ? nckPoints.length : sels.length,
    };
    setSaving(true);
    // 重新发起保存：清掉上一次的 issues（成功后同样清掉）。
    setSaveIssues([]);
    setSaveOutcome(null);
    setPendingVerify(null);
    // apply 步骤里的「待核对」判定必须传出来：applyEndpointChange 只表达 ok/failed，
    // 用局部槽位 + 读取函数，避免 TS 把闭包内赋值当成不可达。
    let pendingOutcome: PendingVerification | null = null;
    const takePending = () => pendingOutcome;
    try {
      // 统一生命周期：STOPPED 直接 apply（绝不自动 start）；RUNNING 经
      // executor 走 stop → apply →（可选）start，各步失败都有明确 outcome。
      const outcome = await applyEndpointChange({
        wasRunning,
        restart,
        stop: async (): Promise<LifecycleStepResult> => {
          const r = await api.stopEndpoint(endpointId).catch((e) => ({
            status: -1,
            body: { error: { message: e instanceof Error ? e.message : String(e) } },
          }));
          return r.status === 200
            ? { ok: true }
            : { ok: false, message: (r.body as { error?: { message?: string } })?.error?.message ?? `停止失败（${r.status}），已中止应用` };
        },
        apply: async (): Promise<LifecycleStepResult> => {
          const r = await fetch(`/api/v1/tasks/${endpointId}`, {
            method: "PUT",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ tasks: submittedTasks }),
          });
          const j = await r.json().catch(() => ({}));
          if (r.ok) {
            setSaveIssues([]);
            // **所有驱动**统一回读：成功判据是「回读值与本次完整提交配置一致」，
            // 不是 PUT 2xx（§4.4.3）。回读失败或不一致一律进入「保存结果待核对」，
            // 此时不报成功、不刷新基线、不启动、也不自动重发 PUT。
            const back = await readBackTasks(endpointId).catch(() => null);
            if (back === null) {
              pendingOutcome = { baseline, restart, wasRunning, reason: "read-failed" };
              return { ok: false, message: `${PENDING_VERIFY_NOTICE}：回读失败` };
            }
            if (!sameSubmittedConfig(back, submittedTasks)) {
              pendingOutcome = { baseline, restart, wasRunning, reason: "mismatch" };
              return { ok: false, message: `${PENDING_VERIFY_NOTICE}：回读不一致` };
            }
            // 一致后才刷新**完整基线**；PUT 2xx 时绝不刷新（否则状态区会虚报已生效）。
            applyVerifiedBaseline(baseline);
            return { ok: true };
          }
          // 后端 issues 原样保存并展示（`{valid:false, issues}`）；
          // 普通 `{error:{message}}` / HTTP 异常仍走原 fallback，不退化。
          const issues = (j as { issues?: ValidationIssue }) as { issues?: unknown };
          if (Array.isArray(issues.issues)) {
            const list = issues.issues as ValidationIssue[];
            setSaveIssues(list);
            const summary =
              list.length > 0
                ? `点位保存失败（${list.length} 项）：${list
                    .map((i) => `${i.path} · ${i.code}`)
                    .join("；")}`
                : "点位保存失败";
            return { ok: false, message: summary };
          }
          return { ok: false, message: (j as { error?: { message?: string } })?.error?.message ?? "点位保存失败" };
        },
        start: async (): Promise<LifecycleStepResult> => {
          const r = await api.startEndpoint(endpointId).catch((e) => ({
            status: -1,
            body: { error: { message: e instanceof Error ? e.message : String(e) } },
          }));
          return r.status === 200
            ? { ok: true }
            : { ok: false, message: (r.body as { error?: { message?: string } })?.error?.message ?? `恢复运行失败（${r.status}）` };
        },
      });
      const pending = takePending();
      if (pending) {
        setPendingVerify(pending);
        setLastApply(null);
        setSaveOutcome({
          kind: "pending-verify",
          message:
            pending.reason === "read-failed"
              ? `${PENDING_VERIFY_NOTICE}：回读失败，无法确认配置是否已持久化（未刷新基线，未重复写入）`
              : `${PENDING_VERIFY_NOTICE}：回读不一致（回读值与本次提交配置不同；未刷新基线，未重复写入）`,
        });
        // stop 已成功 ⇒ 连接确实已停；写入结果未知只影响「待核对」，不影响运行态。
        if (wasRunning) setRunState("stopped");
        return;
      }
      if (outcome.kind === "applied-restarted") {
        // P0-2：完成态必须明确写出实际落点，而不是让用户从「正在启动…」猜。
        // 成功提示只由保存区结果条展示一次（不再叠加 toast，§4.6-4）。
        setLastApply("restarted");
        setRunState("running");
        setRestartFailed(null);
        onChanged();
      } else if (outcome.kind === "applied-stopped") {
        setLastApply("stopped");
        // 状态区/下一次按钮必须反映“保存后确实停止了”，不能沿用保存前的运行态。
        setRunState("stopped");
        setRestartFailed(null);
        onChanged();
      } else if (outcome.kind === "stop-failed") {
        setLastApply(null);
        setSaveOutcome({
          kind: "failed",
          message: `未能停止采集，已取消本次保存，未修改配置：${outcome.message}`,
        });
      } else if (outcome.kind === "apply-failed-stopped") {
        setLastApply(null);
        setRunState("stopped");
        setSaveOutcome({
          kind: "failed",
          message: `写入失败，Endpoint 当前已停止：${outcome.message}`,
        });
        onChanged();
      } else {
        // 回读已一致（基线已刷新、草稿标记已清除）但启动失败：
        // 只保留「重试启动」入口，重试只调用启动步骤，不重复整套保存（§4.5）。
        setLastApply("restart-failed");
        setRunState("stopped");
        setRestartFailed(outcome.message);
        onChanged();
      }
    } finally {
      setSaving(false);
    }
  };

  // P0-2：完成态文案只描述**已经发生**的事；未保存时只描述“保存后会怎样”，
  // 且与按钮/复选框的实际分支一一对应（不写死 Stop → PUT → Start）。
  // 运行态未知时不得描述任何保存路径（不知道会不会先 stop）。
  const afterSaveText =
    runState === "unknown"
      ? "状态未知（无法判断保存路径）"
      : lastApply === "restarted"
        ? "配置已保存，采集已启动"
        : lastApply === "stopped"
          ? "配置已保存，当前停止"
          : lastApply === "restart-failed"
            ? "配置已保存，但采集启动失败"
            : !running
              ? "保存后保持停止"
              : restart
                ? "停止 → 写入配置 → 启动"
                : "停止 → 写入配置，保存后保持停止";
  const executionNote =
    runState === "unknown"
      ? "运行状态未知：保存已禁用（不会发出停止/写入/启动请求）。请先刷新状态。"
      : !running
        ? `保存将直接写入配置（PUT /tasks/${endpointId}），不会启动采集；Endpoint 保持停止。`
        : restart
          ? `保存将执行：停止 → 写入配置（PUT /tasks/${endpointId}）→ 启动。`
          : `保存将执行：停止 → 写入配置（PUT /tasks/${endpointId}），保存后保持停止。`;

  /**
   * 按钮可用性与提交函数**共用同一判定**（§4.4.2）：直接调用唯一决策点
   * `planEndpointTaskSave`，不另写一套比较逻辑。
   * - `changed`：可保存；
   * - `unchanged`：无实际修改（按钮禁用；提交函数仍会再拦一次并给 info 提示）；
   * - `unknown`：被其它门拒绝（快照/准入/冲突等）。
   */
  const saveChangeState = useMemo(
    () =>
      endpointSaveChangeState({
        driverId,
        tasks: existingTasks,
        snapshotState: tasksLoadState,
        loadedEndpointId: tasksLoadedEpId,
        currentEndpointId: endpointId,
        intervalMs,
        selections: sels,
        nckPoints,
        nckDefinitions: definitions,
      }),
    [
      driverId,
      existingTasks,
      tasksLoadState,
      tasksLoadedEpId,
      endpointId,
      intervalMs,
      sels,
      nckPoints,
      definitions,
    ],
  );

  const descOk = nck || !!desc;
  if (!descOk && tasksLoadState === "loading") return <div style={{ color: "#525252" }}>加载资源…</div>;
  return (
    <div style={{ display: "grid", gap: 12, maxWidth: 860 }}>
      {tasksLoadState === "error" ? (
        <Alert type="error" message={tasksLoadError || "任务快照加载失败"} description="服务端已有任务未知，为防止覆盖已禁用保存。请切换后重试。" />
      ) : null}
      {tasksLoadState === "loading" ? <div style={{ fontSize: 12, color: "#525252" }}>正在加载任务快照…（快照就绪前保存保持禁用，防止覆盖已有任务）</div> : null}
      {editBlock ? (
        <Alert
          type="error"
          message="现有任务无法安全编辑，已禁用保存"
          description={`${editBlock.code}：${editBlock.message}`}
        />
      ) : null}
      {!descOk ? (
        <Alert type="error" message="资源描述加载失败" description="描述缺失时选型无合法依据，已禁用保存（不会覆盖已有任务）。请切换后重试。" />
      ) : (
      <>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12 }}>采集周期</span>
        <InputNumber min={10} max={60000} step={10} value={intervalMs} onChange={(v) => setIntervalMs(v ?? 1000)} addonAfter="ms" data-testid="acquisition-interval-input" style={{ width: 180 }} />
        <span style={{ fontSize: 12, color: "#525252" }}>10ms–60s（配置值）</span>
        {/*
          P2-1：压测数字只能出现在帮助提示里，且必须同时声明“软件压测 ≠ 机床能力”。
          把 20ms / 50K/s 直接摆在输入框旁，用户会把它读成“这台机床能 20ms 采”。
        */}
        <Tooltip
          title={
            <span>
              20ms 是软件压力测试中达到 50K/s 的条件；软件压力测试结果不代表真实机床的可用采集周期。
              实际采集表现以运行结果为准。
            </span>
          }
        >
          <QuestionCircleOutlined
            aria-label="采集周期说明"
            data-testid="interval-help"
            style={{ color: "#8c8c8c", cursor: "help" }}
          />
        </Tooltip>
      </div>
      <div style={{ fontSize: 11, color: "#8c8c8c" }} data-testid="interval-note">
        周期为配置值，实际采集表现以运行结果为准。
      </div>

      {/* P2-2：复用现有 Endpoint 状态与点位数据，不新增后端接口。 */}
      <div
        data-testid="acquisition-status"
        style={{
          display: "flex",
          gap: 16,
          flexWrap: "wrap",
          fontSize: 12,
          color: "#525252",
          border: "1px solid #e0e0e0",
          padding: "6px 8px",
        }}
      >
        <span>
          采集状态：
          <b data-testid="acquisition-status-state">
            {runState === "running" ? "运行中" : runState === "stopped" ? "已停止" : "状态未知"}
          </b>
        </span>
        <span>
          采集周期：
          {/* P2-2 方案 B：同时给出「已保存」与「待保存」，草稿不再冒充当前配置。 */}
          <b data-testid="acquisition-status-interval">
            {savedVsDraftText(
              savedBaseline?.hasTask ? savedBaseline.intervalMs : null,
              intervalMs,
              "ms",
            )}
          </b>
        </span>
        <span>
          已配置点位：
          <b data-testid="acquisition-status-points">
            {savedVsDraftText(
              savedBaseline?.hasTask ? savedBaseline.pointCount : null,
              nck ? nckPoints.length : sels.length,
              "个",
            )}
          </b>
        </span>
        <span>
          保存后状态：
          <b data-testid="acquisition-status-after">{afterSaveText}</b>
        </span>
      </div>
      {/*
        运行态未知（加载失败 / 切换连接 / 刷新失败 / Endpoint 不在清单）：
        禁止一切保存（含「只写配置」），只提供刷新入口；草稿仍可编辑（§4.4.1）。
      */}
      {runStateSettled && runState === "unknown" ? (
        <Alert
          type="warning"
          showIcon
          data-testid="acquisition-run-state-unknown"
          message="运行状态未知，已禁用保存"
          description="读取 Endpoint 运行状态失败，或该 Endpoint 不在清单中。为避免在未知状态下停止/写入/启动采集，保存已禁用（含只写配置）；草稿仍可编辑。"
          action={
            <Button
              size="small"
              data-testid="refresh-run-state"
              loading={saving}
              onClick={() => void loadRunState()}
            >
              刷新状态
            </Button>
          }
        />
      ) : null}
      {!nck ? (
      <ResourcePickerAntd
        resources={desc?.resources ?? []}
        existingSelections={sels}
        protectedSelections={preservedTasks.flatMap((t) => selectionsOf(t))}
        selectionMethods={desc?.resource_selection_methods ?? []}
        endpointId={endpointId}
        onAdd={(s) => {
          // 唯一防御门：与 AddDeviceFlow 同一共享实现（终审：两调用方行为一致，
          // protected 永不动；point_key 检查由 reconcile 统一执行）。
          const schemaOf = (resourceId: string) => {
            const found = (desc?.resources ?? []).find((r) => r.id === resourceId);
            return {
              fields: (found?.parameters.fields ?? []).map((f) => ({ key: f.key, default: f.default })),
            };
          };
          const { next, message: msg } = applySelectionAdd({
            candidate: s,
            editable: sels,
            protectedSelections: preservedTasks.flatMap((t) => selectionsOf(t)),
            schemaOf,
          });
          if (msg) {
            if (msg.startsWith("point_key")) message.error(msg);
            else message.warning(msg);
            return false;
          }
          setSels(next);
          // 已有的选择项按对象引用/身份键沿用 rowId；新增项发新号（不重排既有行）。
          // 身份键 = resource_id + 参数（同一资源同参数视为同一行；合并 outputs
          // 会产出新对象，靠身份键把 rowId 接续过去）。仅用于界面定位。
          setSelRowIds((prev) =>
            alignRowIds(
              sels,
              prev,
              next,
              rowIdAllocator.current,
              (sel) => `${sel.resource_id}\u0000${stableIdentityKey(sel.parameters)}`,
            ),
          );
          return true;
        }}
      />
      ) : (
      <div style={{ display: "grid", gap: 8 }}>
        {catalogState === "loading" ? (
          <div style={{ fontSize: 12, color: "#525252" }} data-testid="nck-catalog-loading">
            正在读取定义目录（完整分页读取，不遗漏、不重复）…
          </div>
        ) : null}
        {catalogState === "error" ? (
          <Alert
            type="error"
            data-testid="nck-catalog-error"
            message="定义目录加载失败：NCK 保存已禁用（fail-closed）"
            description={`${catalogError}。没有权威目录就无法确认 scope/family，也不会退回「无 scope 也能存」。`}
          />
        ) : null}
        {!definitions ? (
          <Alert
            type="warning"
            message="定义目录不可用：NCK 保存已禁用（fail-closed）"
            description="没有定义目录就无法确认哪些变量可采集（scope 准入），因此保存保持禁用；在可用前不会出现“保存成功但编辑没生效”。"
          />
        ) : (
          <div style={{ fontSize: 12, color: "#525252" }} data-testid="nck-collectable-summary">
            {/* P1-1：默认只显示数量；36 个标签墙已移除（标签不可点击，纯占位）。 */}
            <div style={{ fontSize: 13, color: "#262626" }} data-testid="nck-collectable-count">
              {collectableDefs.length} 个可配置变量
            </div>
            <details data-testid="nck-catalog-details" style={{ marginTop: 4 }}>
              <summary style={{ cursor: "pointer", fontSize: 11 }}>目录详情</summary>
              <div style={{ marginTop: 4, fontSize: 11 }} data-testid="nck-catalog-meta">
                共 {definitionList.length} 条定义（其余为身份探针或缺少 scope，不可采集）
                {" · "}目录：
                {catalog ? `${catalog.total} 条 / ${catalog.pages} 页 / rev ${catalog.revision}` : "注入目录"}
                {" · "}Endpoint family：
                {endpointFamilyState === "loading"
                  ? "读取中"
                  : endpointFamilyState === "error"
                    ? "读取失败"
                    : endpointFamily ?? "缺失（禁止提交）"}
              </div>
            </details>
          </div>
        )}

        {definitions && endpointFamilyState === "ready" && !endpointFamily ? (
          <Alert
            type="error"
            data-testid="nck-endpoint-family-missing"
            message="Endpoint 连接配置缺少 family：禁止新增/保存语义点位"
            description="family 必须以 Endpoint 有效连接配置为准（目录顶层 family 只是回显，不是提交依据）。请先补齐连接配置。"
          />
        ) : null}

        {definitions && !editBlock ? (
          <div style={{ display: "grid", gap: 8 }}>
            <div>
              {/* Browse 是辅助入口：用 link 样式避免与主选择框争抢注意力。 */}
              <Button
                size="small"
                type="link"
                data-testid="nck-browse-toggle"
                onClick={() => setBrowseOpen((v) => !v)}
              >
                {browseOpen ? "收起 Browse" : "从 Browse 选择语义变量"}
              </Button>
            </div>
            {browseOpen ? (
              <ResourceBrowseAntd
                endpointId={endpointId}
                onFill={() => message.warning("请选用语义点位（旧 variable 入口在 NCK 上已被禁用）")}
                nckSemantic={{
                  definitions: definitionList,
                  endpointFamily,
                  catalogFamily,
                }}
                onSemanticSelect={(sel) => {
                  // Browse 的 preview_point_key 只作展示：预填参数后，NckPointFields
                  // 会重新调用服务端 identity-preview 确认身份。
                  setBrowsePreset({
                    kind: "nck_variable",
                    point_key: "",
                    parameters: sel.parameters,
                  });
                  setEditingIndex(null);
                  setFormVersion((v) => v + 1);
                  message.info("已把 Browse 叶带入语义表单：请等待服务端 Preview 确认身份");
                }}
              />
            ) : null}
          </div>
        ) : null}

        {definitions && !editBlock ? (
          <div style={{ borderTop: "1px solid #e0e0e0", paddingTop: 8 }}>
            {editingIndex !== null && nckPoints[editingIndex] ? (
              <NckPointFields
                key={`edit-${endpointId}-${editingIndex}-${nckPoints[editingIndex].point_key}-${formVersion}`}
                driverId={driverId}
                endpointFamily={endpointFamily}
                catalogFamily={catalogFamily}
                definitions={collectableDefs}
                takenPointKeys={takenPointKeys(editingIndex)}
                initialPoint={nckPoints[editingIndex]}
                nativeSchema={nativeSchema}
                submitLabel="保存点位"
                onCancel={() => setEditingIndex(null)}
                onSubmit={(point) => replacePoint(editingIndex, point)}
              />
            ) : (
              <NckPointFields
                key={`add-${endpointId}-${formVersion}`}
                driverId={driverId}
                endpointFamily={endpointFamily}
                catalogFamily={catalogFamily}
                definitions={collectableDefs}
                takenPointKeys={takenPointKeys(null)}
                initialPoint={browsePreset}
                nativeSchema={nativeSchema}
                onSubmit={addPoint}
              />
            )}
          </div>
        ) : null}

        {removedPoints.length > 0 ? (
          <Alert
            type="info"
            data-testid="nck-removed-points"
            message={`已手动移除 ${removedPoints.length} 个点位（保存不会包含它们）`}
            description={
              <ul style={{ margin: 0, paddingLeft: 18 }}>
                {removedPoints.map((removed, idx) => (
                  <li
                    key={`${removed.point.point_key}-${idx}`}
                    style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 11 }}
                  >
                    {removed.point.kind} · {removed.point.point_key} · 原因：{removed.reason}
                    <div style={{ color: "#525252" }}>
                      原始配置：{JSON.stringify(removed.point.parameters)}
                    </div>
                  </li>
                ))}
              </ul>
            }
          />
        ) : null}

        {unverified.length > 0 ? (
          <Alert
            type="error"
            data-testid="nck-legacy-identity-alert"
            message="当前任务包含不可采集的身份探针点位"
            description={
              <div>
                <div>
                  当前任务包含不可采集的身份探针点位。请检查并手动移除这些点位，重新验证剩余点位后再保存。
                </div>
                <ul style={{ margin: "6px 0 0", paddingLeft: 18 }}>
                  {unverified.map((status, idx) => (
                    <li
                      key={`${status.point.point_key}-${idx}`}
                      style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 11 }}
                    >
                      {status.point.point_key} · {status.code} · {status.message}
                    </li>
                  ))}
                </ul>
                <div style={{ marginTop: 4 }}>
                  这些点位不会被自动删除，也不会被自动转成 collect；只能由你显式移除。
                </div>
              </div>
            }
          />
        ) : null}

        {admissionIssues.length > 0 && unverified.length === 0 ? (
          <Alert
            type="error"
            message={`存在不可采集点位（${admissionIssues.length} 项，scope 准入拒绝）`}
            description={
              <ul style={{ margin: 0, paddingLeft: 18 }}>
                {admissionIssues.map((issue, idx) => (
                  <li key={idx} style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 11 }}>
                    {issue.pointKey} · {issue.code} · {issue.message}
                  </li>
                ))}
              </ul>
            }
          />
        ) : null}

        {nckPoints.length > 0 ? (
          <div style={{ display: "grid", gap: 6 }} data-testid="nck-point-list">
            {/* P1-2 硬约束：绿色标签只代表**配置格式**校验，不能让用户误以为
                真实机床已经读取成功。 */}
            <div style={{ fontSize: 11, color: "#8c8c8c" }} data-testid="nck-point-list-hint">
              以下为配置格式校验结果，不代表机床已成功读取数据；真实采集情况请到「数据」页查看。
            </div>
            {nckPoints.map((point, idx) => {
              const status = statusesCurrent ? pointStatuses[idx] : undefined;
              const variableId =
                typeof point.parameters?.variable_id === "string"
                  ? point.parameters.variable_id
                  : "";
              const revision =
                typeof point.parameters?.definition_revision === "number"
                  ? point.parameters.definition_revision
                  : null;
              const exactDefinition =
                variableId && revision !== null
                  ? lookupDefinition(variableId, revision)
                  : null;
              // 只允许编辑「目录里仍是最新 revision」的点位：若该变量已发布新
              // revision，按旧 revision 打开编辑器会静默把身份迁到新 revision，
              // 违反「重新打开编辑器不得静默改变身份」。此时只能移除后重配。
              const latestDefinition = variableId ? definitions?.[variableId] : undefined;
              const revisionIsCurrent =
                !!exactDefinition &&
                (!latestDefinition ||
                  typeof exactDefinition.revision !== "number" ||
                  latestDefinition.revision === exactDefinition.revision);
              // native 编辑只要求 Descriptor Schema 可用（不依赖语义目录）；
              // 语义点位仍要求定义仍是最新 revision 且 scope=collect。
              const editableGate =
                point.kind === "nck_native"
                  ? !!nativeSchema
                  : exactDefinition && revisionIsCurrent
                    ? evaluateNckSelectionAdd(exactDefinition).ok
                    : false;
              // P1-2：两层信息。名称只取定义 label（原生点位固定「原生点位」），
              // 地址由参数结构化拼出；point_key / 完整 JSON / 版本收进折叠区。
              const display = nckPointDisplay(point, exactDefinition);
              // 行 key 用稳定 rowId：编辑改 point_key 时行不重建、位置不被顶走（§4.8）。
              const rowId = nckRowIds[idx] ?? `nck-row-missing-${idx}`;
              return (
                <div
                  key={rowId}
                  data-row-id={rowId}
                  data-testid="nck-point-row"
                  style={{ display: "grid", gap: 6, padding: 8, border: "1px solid #e0e0e0", borderRadius: 0 }}
                >
                  <div style={{ display: "flex", gap: 8, alignItems: "flex-start" }}>
                    <div style={{ flex: 1, display: "grid", gap: 2 }}>
                      <div data-testid="nck-point-name" style={{ fontSize: 13, fontWeight: 600 }}>
                        {display.title}
                      </div>
                      <div data-testid="nck-point-address" style={{ fontSize: 12, color: "#525252" }}>
                        {display.address}
                      </div>
                      <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
                        {!status ? <Tag>正在校验配置格式…</Tag> : null}
                        {status?.status === "verified" ? (
                          <Tag color="green" data-testid="nck-point-status">
                            {status.native ? "配置格式校验通过（原生）" : "配置格式校验通过"}
                          </Tag>
                        ) : null}
                        {status?.status === "unverified" ? (
                          <>
                            <Tag color="red" data-testid="nck-point-status">
                              配置格式校验未通过：{status.code}
                            </Tag>
                            <span
                              data-testid="nck-point-status-detail"
                              style={{ fontSize: 11, color: "#c62828" }}
                            >
                              {status.message}
                            </span>
                          </>
                        ) : null}
                      </div>
                    </div>
                    <div style={{ display: "flex", gap: 6 }}>
                      {editableGate ? (
                        <Button
                          size="small"
                          onClick={() => {
                            setEditingIndex(idx);
                            setFormVersion((v) => v + 1);
                          }}
                        >
                          编辑
                        </Button>
                      ) : null}
                      {/*
                        删除按钮**不经过** scope/family/预览准入门（见 removePoint 注释）。
                        无论点位是否合法都必须可移除，否则存量 identity 点位会形成死锁。
                      */}
                      <Button size="small" danger onClick={() => removePoint(idx)}>
                        移除
                      </Button>
                    </div>
                  </div>
                  <details data-testid="nck-point-technical">
                    <summary style={{ cursor: "pointer", fontSize: 11, color: "#525252" }}>
                      技术详情
                    </summary>
                    <div
                      style={{
                        marginTop: 4,
                        display: "grid",
                        gap: 2,
                        fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace",
                        fontSize: 11,
                        color: "#525252",
                      }}
                    >
                      <div>point_key：{point.point_key}</div>
                      <div>kind：{point.kind}</div>
                      <div>{display.revision}</div>
                      <div>参数：{JSON.stringify(point.parameters)}</div>
                      {point.kind === "nck_native" ? (
                        <div>校验方式：原生字段（不适用目录 scope）</div>
                      ) : (
                        <div>身份来源：服务端 identity-preview 重算</div>
                      )}
                    </div>
                  </details>
                </div>
              );
            })}
          </div>
        ) : null}
      </div>
      )}
      </>)}
      <div style={{ fontSize: 12, color: "#525252" }} data-testid="acquisition-execution-note">
        已选 {nck ? nckPoints.length : sels.length} 项 · {intervalMs}ms 轮询 · {executionNote}
        {!nck ? (
          <Button
            size="small"
            onClick={() => {
              setSels([]);
              setSelRowIds([]);
            }}
            style={{ marginLeft: 8 }}
          >
            清空
          </Button>
        ) : null}
      </div>
      {preservedTasks.length > 0 && (
        <div style={{ fontSize: 12, color: "#525252" }}>
          以下任务不在此编辑、保存时原样保留（只读，reconciliation 仅检测不修改）：
          {preservedTasks.map((t) => {
            const keys = pointKeysOfTask(t);
            return (
              <div key={t.id} style={{ marginTop: 4 }}>
                <Tag>{t.id} · {t.binding.kind} · {t.schedule?.mode}</Tag>
                <span style={{ marginLeft: 6, fontFamily: "'IBM Plex Mono',monospace", fontSize: 11 }}>
                  {keys.length ? keys.join(", ") : "（空任务）"}
                </span>
              </div>
            );
          })}
        </div>
      )}
      {!nck && !!sels.length && (
        <div style={{ display: "grid", gap: 6 }}>
          {sels.map((s, idx) => {
            // A4：通用「字段标签 + 值」摘要（标签来自 Descriptor 字段 label）；
            // 无可读参数时 pointSummary 回落 point_key——此处不再重复渲染
            // 一遍与左侧相同的 key，避免 `register.value register.value`。
            const schema = (desc?.resources ?? []).find((r) => r.id === s.resource_id)?.parameters;
            const pointKeys = s.outputs.map((o) => o.point_key).join(", ");
            const summary = pointSummary({
              point_key: pointKeys,
              parameters: s.parameters,
              schema,
            });
            // 行 key 用稳定 rowId（不能用下标：移除一行后同一 key 会指向别的行）。
            const rowId = selRowIds[idx] ?? `sel-row-missing-${idx}`;
            return (
              <div
                key={rowId}
                data-row-id={rowId}
                data-testid="sel-point-row"
                style={{ display: "flex", gap: 8, alignItems: "center", padding: 6, border: "1px solid #e0e0e0", borderRadius: 0 }}
              >
                <span style={{ flex: 1, fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11 }}>{s.resource_id} → {pointKeys} {summary && summary !== pointKeys ? <span style={{ color: "#525252" }}>{summary}</span> : null}</span>
                <Button
                  size="small"
                  danger
                  onClick={() => {
                    // 移除只删掉这一行的 rowId，其余行身份不变（§4.8）。
                    setSels((p) => p.filter((_, i) => i !== idx));
                    setSelRowIds((p) => p.filter((_, i) => i !== idx));
                  }}
                >
                  移除
                </Button>
              </div>
            );
          })}
        </div>
      )}
      {hasIncompatible ? (
        // A3/P1：提示只出现一次（不逐行红框），字段名/类型收在「技术详情」。
        // 明确区分「存量配置已携带」与「驱动新增、配置未携带」——后者正是
        // 「表单已禁用但旧守卫会放行」的缺口来源。
        // 只禁用本 Endpoint 的保存；不停已运行的采集，也不影响其它连接。
        <Alert
          type="warning"
          showIcon
          data-testid="acquisition-unsupported-fields"
          message={UNSUPPORTED_CONFIG_NOTICE}
          description={
            <details data-testid="acquisition-unsupported-details">
              <summary style={{ cursor: "pointer", fontSize: 12 }}>技术详情</summary>
              <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
                {incompatible.map((item, idx) => (
                  <li
                    key={`${item.resourceId}-${item.key}-${idx}`}
                    style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11 }}
                  >
                    {item.resourceId} · {item.key} · {item.fieldType} ·{" "}
                    {item.carried ? "存量配置已携带" : "配置未携带（驱动新增字段）"}
                  </li>
                ))}
              </ul>
            </details>
          }
        />
      ) : null}
      <div>
        {/*
          运行态未知：保存区不提供任何保存动作，只给「刷新状态」。
          仍渲染一个 disabled 的保存按钮（文案与已知状态一致），因为**按钮禁用
          只是体验**：真正的防线在 save() 里的 unknown 守卫（§4.4.1）。
        */}
        {runState === "unknown" ? (
          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <Button
              type="primary"
              data-testid="apply-button"
              disabled
              onClick={() => void save(restart)}
            >
              保存配置
            </Button>
            <span data-testid="apply-stopped-note" style={{ fontSize: 12, color: "#525252" }}>
              {runStateSettled ? "运行状态未知，无法保存" : "正在读取运行状态…"}
            </span>
          </div>
        ) : (
          /* P0-2：已停止时保存只写配置（onApply(false)），按钮不得写「保存并启动」；
             运行中的恢复运行由 restart 勾选决定，结果由 lastApply 明确回显。 */
          <ApplyWithRestart
            running={running}
            applying={saving}
            canApply={
              isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId) &&
              !hasIncompatible &&
              (nck
                ? !!definitions &&
                  !!endpointFamily &&
                  !editBlock &&
                  nckPoints.length > 0 &&
                  admissionIssues.length === 0 &&
                  verificationReady &&
                  unverified.length === 0
                : sels.length > 0 && !!desc) &&
              // 无实际修改 ⇒ 按钮不可用（所有驱动同一判定，§4.4.2）。
              saveChangeState === "changed" &&
              // 待核对期间不允许再走主保存动作（只能「重新核对」）。
              !pendingVerify
            }
            applyLabel="保存配置"
            stoppedLabel="保存配置"
            restart={restart}
            onRestartChange={setRestart}
            result={lastApply}
            onApply={save}
          />
        )}
      </div>
      {/*
        「保存结果待核对」（§4.4.3）：回读失败或回读不一致。
        持久展示，不报成功、不刷新基线；主操作只能是「重新核对」，
        不允许直接重新 PUT。核对确认不一致后才出现「重新保存」。
      */}
      {saveOutcome ? (
        <Alert
          type={saveOutcome.kind === "failed" ? "error" : "warning"}
          showIcon
          data-testid="save-outcome"
          data-kind={saveOutcome.kind}
          message={
            saveOutcome.kind === "pending-verify"
              ? PENDING_VERIFY_NOTICE
              : saveOutcome.kind === "state-changed"
                ? "运行状态已变化或无法确认"
                : "保存未完成"
          }
          description={
            <div style={{ display: "grid", gap: 8 }}>
              <div>{saveOutcome.message}</div>
              {saveOutcome.kind === "pending-verify" ? (
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                  <Button
                    size="small"
                    data-testid="recheck-save"
                    loading={rechecking}
                    onClick={() => void recheckPending()}
                  >
                    重新核对
                  </Button>
                  {saveOutcome.canResave && pendingVerify ? (
                    <Button
                      size="small"
                      data-testid="resave-after-pending"
                      disabled={saving || runState === "unknown"}
                      onClick={() =>
                        void save(pendingVerify.restart, { fromPendingResave: true })
                      }
                    >
                      重新保存
                    </Button>
                  ) : null}
                </div>
              ) : null}
              {saveOutcome.kind === "state-changed" ? (
                <div>
                  <Button
                    size="small"
                    data-testid="refresh-run-state-after-abort"
                    onClick={() => void loadRunState()}
                  >
                    刷新状态
                  </Button>
                </div>
              ) : null}
            </div>
          }
        />
      ) : null}
      {/*
        「配置已保存但启动失败」（§4.5）：草稿标记已清除（基线已刷新），
        但处理入口必须保留；重试**只调用启动步骤**，不 stop、不 PUT。
        该入口不随新修改消失（§4.7：产生新修改时仍保留「上次保存后采集未恢复」）。
      */}
      {restartFailed ? (
        <div
          data-testid="restart-failed"
          style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}
        >
          <span style={{ fontSize: 12, color: "#8a6d3b" }}>
            配置已保存，但恢复采集失败：{restartFailed}
          </span>
          <Button
            size="small"
            data-testid="retry-start"
            loading={saving}
            disabled={saving || rechecking || runState === "unknown"}
            onClick={() => void retryStart()}
          >
            重试启动
          </Button>
        </div>
      ) : null}
      {saveIssues.length > 0 ? (
        <Alert
          type="error"
          message={`保存被后端拒绝（${saveIssues.length} 项，原样展示）`}
          description={
            <ul style={{ margin: 0, paddingLeft: 18 }}>
              {saveIssues.map((i, idx) => (
                <li key={idx} style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11 }}>
                  {i.path} · {i.code} · {i.message}
                </li>
              ))}
            </ul>
          }
        />
      ) : null}
    </div>
  );
}
