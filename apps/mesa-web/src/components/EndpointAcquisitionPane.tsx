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
import { Alert, Button, InputNumber, Modal, Tooltip, message } from "antd";
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
  maxRowIdSeq,
  paginateItems,
  stableIdentityKey,
} from "../pointRows";
import { draftStashKey, useDraftStashOrDefault } from "../draftStash";
import {
  buildFilterOptions,
  countRowStates,
  applyRowQuery,
  type PointFilter,
  type PointRowView,
} from "../acquisitionRowView";
import {
  buildNckPointRows,
  buildSelectionRows,
  type RowVerification,
} from "../acquisitionRowModel";
import {
  diffDraft,
  draftContentKey,
  saveBarView,
  type DraftBaseline,
  type DraftRowRef,
} from "../acquisitionDraftDiff";
import { NoticeSlot } from "./acquisition/NoticeSlot";
import { PointDrawer } from "./acquisition/PointDrawer";
import { PointSearch } from "./acquisition/PointSearch";
import { PointTable, PointPagination } from "./acquisition/PointTable";
import { DraftSaveBar } from "./acquisition/DraftSaveBar";
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
import { createNckSemanticHandler } from "../nckBrowseSemantic";
import {
  nativeAddressParameters,
  validateNativeFields,
  validateNativePoint,
} from "../nckNativeModel";
import { applySelectionAdd } from "../resourceSelectionModel";
import { objectProbeGate } from "../objectProbe";
import type { ApplyResult } from "./ApplyWithRestart";
import { NckPointFields } from "./NckPointFields";
import { ResourceBrowseAntd } from "./ResourceBrowseAntd";
import { ResourcePickerAntd } from "./ResourcePickerAntd";
import {
  UNSUPPORTED_CONFIG_NOTICE,
  unsupportedFields,
  unsupportedFieldsInResources,
  type SubmittedResourceSchema,
  type UnsupportedFieldInResource,
} from "../descriptorCompatibility";
import { draftSignature, savedVsDraftText } from "../acquisitionDraft";
import {
  describeUnlocatable,
  firstLocatableIssue,
  locateIssuePath,
  type IssueLocation,
  type SubmittedRowSnapshot,
} from "../issueLocation";

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

/** 用户从草稿里移除的一行：**原始配置保留**，便于确认改了什么，且可撤销。 */
interface RemovedDraftEntry {
  rowId: string;
  /** 原下标：撤销时放回原位（顺序也是配置的一部分）。 */
  index: number;
  reason: string;
  point?: NckPointSelection;
  selection?: ResourceSelection;
}

/**
 * 草稿副本（`draftStash`，挂在**路由之外**）。
 *
 * 为什么要存这一份（§7-7 第 5 步）：声明式路由（`BrowserRouter` + `Routes`）下，
 * 浏览器后退触发 `popstate` 时路由已经变了，采集页会短暂卸载——组件内 `useState`
 * 的草稿会跟着消失。副本让「短暂的路由变化不可能销毁草稿」：卸载前写入，
 * 重新挂载并加载完服务端快照后取回，恢复成**未保存草稿**（基线仍是服务端快照，
 * 因此差异判定会照常把它算成"有未保存修改"）。
 *
 * 只放**用户可编辑的草稿内容**：服务端快照（任务/描述/运行态/基线）一律重新加载，
 * 不复制、不伪造。
 */
interface StashedDraft {
  intervalMs: number;
  sels: ResourceSelection[];
  selRowIds: string[];
  nckPoints: NckPointSelection[];
  nckRowIds: string[];
  removedEntries: RemovedDraftEntry[];
  /** 「保存后是否恢复采集」的选择同属草稿（用户改过就不该被清掉）。 */
  restart: boolean;
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
 * 草稿行内容键（只用于**差异展示**：行标记与变更摘要）。
 *
 * NCK 点位用 `kind + 完整参数`：`point_key` 由参数派生，进不进键都不改变判定。
 * 通用资源选择用 `resource_id + 完整参数 + 输出`（顺序保序：顺序也是配置）。
 */
function pointContentKey(point: NckPointSelection): string {
  return draftContentKey({ kind: point.kind, parameters: point.parameters });
}

function selectionContentKey(selection: ResourceSelection): string {
  return draftContentKey({
    resource_id: selection.resource_id,
    parameters: selection.parameters,
    outputs: selection.outputs,
  });
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
  onDraftStateChange,
}: {
  endpointId: string;
  driverId: string;
  onChanged: () => void;
  /** NCK 定义目录注入（测试缝；缺省时由组件从 I-2 目录加载）。 */
  nckDefinitions?: NckDefinitionMap | null;
  /**
   * 草稿状态上报（外壳层用）：`dirty` = 有未保存修改，`saving` = 正在保存。
   * 外壳层据此禁用连接切换 / 提示离开页面（§4.7 / §7-7）；本组件不自己拦路由。
   */
  onDraftStateChange?: (state: { dirty: boolean; saving: boolean }) => void;
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
  // 本次提交的行快照：`issue.path` 的下标只能按**提交那一刻**的数组换算成稳定
  // rowId（§5.2-5）。只用于界面定位，不参与任何配置差异判定（§4.8）。
  const [submittedRowSnapshot, setSubmittedRowSnapshot] = useState<SubmittedRowSnapshot | null>(null);
  // 当前被定位的问题行（行高亮 + 抽屉内字段定位）；null = 没有生效的定位。
  // `inPlaceEditable`：该字段能否在**编辑表单里**就地改（决定"打开编辑表单并高亮"
  // 还是"打开只读技术详情并说明不可就地编辑"——不假装高亮）。
  const [locatedIssue, setLocatedIssue] = useState<
    { rowId: string; fieldKey: string | null; inPlaceEditable: boolean } | null
  >(null);
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
  // 用户显式移除的草稿行（原始配置保留 + 可撤销，§7-9）。
  const [removedEntries, setRemovedEntries] = useState<RemovedDraftEntry[]>([]);
  // 编辑态：null = 新增；否则为 nckPoints 下标。
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  // 通用资源的编辑态：null = 新增；否则为 sels 下标（就地编辑同一条选择）。
  // 与 `editingIndex` 分开：两种载荷结构不同（`{kind,point_key,parameters}` vs
  // `{resource_id,parameters,outputs}`），混用下标会让表单读错数据。
  const [editingSelIndex, setEditingSelIndex] = useState<number | null>(null);
  const [formVersion, setFormVersion] = useState(0);
  // Browse → 语义表单的预填（A1/A16 的入口）。preview_point_key 不参与提交。
  const [browseOpen, setBrowseOpen] = useState(false);
  const [browsePreset, setBrowsePreset] = useState<NckPointSelection | null>(null);
  // Browse 语义叶带来的 `identity.keys`（驱动在 wire 上声明"还需要哪类实例"）。
  // 只作为「选择实际对象」的需求来源之一（与定义 targets 取并集，均为数据）。
  const [browseIdentityKeys, setBrowseIdentityKeys] = useState<unknown>(null);

  /**
   * 「选择实际对象」探测入口的可用性。
   *
   * 判据：端点存在（清单可读）+ 连接配置可读（family 可读）。任一不成立都不允许
   * 发起在线探测——探测会临时拉起驱动进程读真机，连接未就绪时发起只会得到误导性
   * 失败（评审硬要求：未连接端点不得在线探测，须明确提示"需先创建并连接端点"）。
   */
  const probeGate = useMemo(
    () =>
      objectProbeGate({
        endpointId,
        endpointKnown: endpointFamilyState === "ready",
        connectionKnown: !!endpointFamily,
      }),
    [endpointId, endpointFamilyState, endpointFamily],
  );

  // ---- 紧凑列表：搜索 / 筛选 / 分页（§2.7） ------------------------------
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(25);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<PointFilter>("all");
  // ---- 抽屉式添加/编辑（§4.1/§4.2） --------------------------------------
  const [drawerOpen, setDrawerOpen] = useState(false);
  // 抽屉内是否有「尚未确认」的修改：关闭时提醒一次（不静默丢弃）。
  const [drawerDirty, setDrawerDirty] = useState(false);
  // 技术详情抽屉（完整点键 / 原始 JSON / 版本 / 身份来源，主动展开才看得到）。
  const [detailRow, setDetailRow] = useState<PointRowView | null>(null);
  // 草稿基线：差异**展示**的对照物（行标记 / 变更摘要）。保存判据不依赖它。
  const [draftBaseline, setDraftBaseline] = useState<DraftBaseline | null>(null);
  // 「放弃修改」= 整份回到服务端已保存值：递增该计数触发一次重新加载。
  const [reloadNonce, setReloadNonce] = useState(0);
  // 列表滚动位置（抽屉打开期间保持，关闭后恢复，§2.7「位置保持」）。
  const listScrollRef = useRef<HTMLDivElement | null>(null);
  const restoreScrollTop = useRef<number | null>(null);

  const nck = isNckDriver(driverId);
  // 草稿副本（路由之外的暂存，§7-7 第 5 步）：无 Provider 时退化为空实现，
  // 直接渲染本组件的既有测试行为零变化。
  const draftStash = useDraftStashOrDefault();
  const stashKey = draftStashKey(endpointId);
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

  /**
   * 当前草稿的「行标识 + 内容键」序列（差异展示的唯一输入）。
   *
   * 用 useCallback 而不是在 render 里就地算：保存/核对成功要把同一份序列写成
   * 新基线，两处必须用**同一个**口径，否则刚保存完的行会残留「已修改」标记。
   */
  const draftRowRefs = useCallback(
    (): DraftRowRef[] =>
      nck
        ? nckPoints.map((point, index) => ({
            rowId: nckRowIds[index] ?? "",
            contentKey: pointContentKey(point),
          }))
        : sels.map((selection, index) => ({
            rowId: selRowIds[index] ?? "",
            contentKey: selectionContentKey(selection),
          })),
    [nck, nckPoints, nckRowIds, sels, selRowIds],
  );

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
    setRemovedEntries([]);
    setEditingIndex(null);
    setEditingSelIndex(null);
    setFormVersion(0);
    setBrowseOpen(false);
    setBrowsePreset(null);
    setBrowseIdentityKeys(null);
    // 列表/搜索/分页与抽屉：切换连接后回到第 1 页、清空搜索与筛选。
    setPage(1);
    setPageSize(25);
    setQuery("");
    setFilter("all");
    setDrawerOpen(false);
    setDrawerDirty(false);
    setDetailRow(null);
    setDraftBaseline(null);
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
            const nextRowIds = assignRowIds(loadedPoints.length, rowIdAllocator.current);
            setNckRowIds(nextRowIds);
            // 草稿基线（差异展示用）：行标识 + 内容快照 + 已保存周期。
            setDraftBaseline({
              rows: loadedPoints.map((point, index) => ({
                rowId: nextRowIds[index]!,
                contentKey: pointContentKey(point),
              })),
              intervalMs: snapshotIntervalMs(selected.editable),
            });
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
          let nextRowIds: string[] = [];
          if (loadedSelections.length) {
            setSels(loadedSelections);
            nextRowIds = assignRowIds(loadedSelections.length, rowIdAllocator.current);
            setSelRowIds(nextRowIds);
          }
          setDraftBaseline({
            rows: loadedSelections.map((sel, index) => ({
              rowId: nextRowIds[index] ?? "",
              contentKey: selectionContentKey(sel),
            })),
            intervalMs: snapshotIntervalMs(editable),
          });
          // P2-2 基线：快照里已生效的完整任务集（保存回读一致后才整份替换）。
          setSavedBaseline({
            hasTask: !!editable,
            intervalMs: snapshotIntervalMs(editable),
            pointCount: loadedSelections.length,
            tasks,
          });
        }
        // 草稿副本恢复（§7-7 第 5 步）：离开守卫下浏览器后退可能让本组件短暂卸载，
        // 草稿内容在 `draftStash`（路由之外）里。这里在**服务端快照与基线就绪之后**
        // 才套用副本，因此：基线仍是服务端真值，草稿仍是"未保存"（差异判定照常把它
        // 算成有修改），保存路径与平时完全一致。
        // 键里含 (driverId, endpointId)：换连接不会串用别人的草稿。
        const stashed = draftStash.take(stashKey) as StashedDraft | null;
        if (stashed) {
          setIntervalMs(stashed.intervalMs);
          setRestart(stashed.restart);
          setRemovedEntries(stashed.removedEntries);
          if (isNckDriver(driverId)) {
            setNckPoints(stashed.nckPoints);
            setNckRowIds(stashed.nckRowIds);
            // 恢复出来的行沿用**原来的** rowId；分配器续号到最大序号之后，
            // 避免之后新增的行撞上已恢复的 id。
            rowIdAllocator.current = createRowIdAllocator("row", maxRowIdSeq(stashed.nckRowIds));
          } else {
            setSels(stashed.sels);
            setSelRowIds(stashed.selRowIds);
            rowIdAllocator.current = createRowIdAllocator("row", maxRowIdSeq(stashed.selRowIds));
          }
        }
        // 快照就绪：只有此时保存才允许 PUT（空数组即服务端真的无任务）。
        setTasksLoadedEpId(endpointId);
        setTasksLoadState("ready");
        // §2.5：删除「加载即弹的提示」；有保留任务时改由列表下方一行静态说明承担
        // （`preserved-tasks-note`），不再打断用户。
        void preservedIds;
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
  }, [endpointId, driverId, loadRunState, reloadNonce, draftStash, stashKey]);

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
    // 只更新页面草稿：不发任何写请求；抽屉关闭后回到原列表位置（§4.1-8）。
    closeDrawer();
  };

  const replacePoint = (index: number, point: NckPointSelection) => {
    if (takenPointKeys(index).includes(point.point_key)) {
      return message.error("该 point_key 已被 Endpoint 内其它点位占用：请调整 Target/Arguments（不得自动改名/加后缀）");
    }
    setNckPoints((prev) => prev.map((p, i) => (i === index ? point : p)));
    // 编辑**不**改 rowId：同一行换了 point_key/参数，仍然是同一行（§4.8）。
    setEditingIndex(null);
    setFormVersion((v) => v + 1);
    closeDrawer();
  };

  /**
   * 从草稿移除一行（**先改草稿、支持撤销**，§7-9）。
   *
   * 关键：本函数**刻意不经过** scope 准入门、不查 family、不要求 Preview。
   * 准入只约束「作为有效点位提交」，不约束「从既有载荷中移除」；若删除也要先
   * 把非法点位「变成合法」，就会形成死锁——那正是本批要避免的缺陷。
   *
   * 移除前把**原始配置**与原因记下来：一是让用户确认到底改了什么，二是支持撤销
   * （撤销把同一行放回**原下标**，顺序也是配置的一部分）。
   */
  const removePoint = (index: number) => {
    const point = nckPoints[index];
    if (!point) return;
    const status = pointStatuses.find((s) => s.point === point);
    const reason =
      status?.status === "unverified"
        ? `${status.code ?? "UNVERIFIED"}：${status.message ?? "身份核验未通过"}`
        : "用户显式移除（原始配置已保留）";
    setRemovedEntries((prev) => [
      { rowId: nckRowIds[index] ?? "", index, reason, point },
      ...prev,
    ]);
    setNckPoints((prev) => prev.filter((_, i) => i !== index));
    // 移除只删掉**这一行**的 rowId：其余行的身份与顺序不变（不重新发号）。
    setNckRowIds((prev) => prev.filter((_, i) => i !== index));
    setEditingIndex((current) => {
      if (current === null) return null;
      if (current === index) return null;
      return current > index ? current - 1 : current;
    });
  };

  /** 从草稿移除一条资源选择（同样先改草稿、可撤销）。 */
  const removeSelection = (index: number) => {
    const selection = sels[index];
    if (!selection) return;
    setRemovedEntries((prev) => [
      { rowId: selRowIds[index] ?? "", index, reason: "用户显式移除（原始配置已保留）", selection },
      ...prev,
    ]);
    setSels((prev) => prev.filter((_, i) => i !== index));
    setSelRowIds((prev) => prev.filter((_, i) => i !== index));
  };

  /**
   * 撤销一次移除：把行放回**原下标**并复用**它自己的** rowId。
   *
   * 复用旧 id 是安全的：该 id 属于这一行本身（不是「回收给新行」），因此
   * 「删除后重新添加要发新号」的规则不受影响（新行永远拿新号）。
   */
  const undoRemove = (rowId: string) => {
    const entry = removedEntries.find((item) => item.rowId === rowId);
    if (!entry) return;
    setRemovedEntries((prev) => prev.filter((item) => item.rowId !== rowId));
    if (nck && entry.point) {
      const insertAt = Math.min(Math.max(0, entry.index), nckPoints.length);
      setNckPoints((prev) => {
        const next = [...prev];
        next.splice(insertAt, 0, entry.point!);
        return next;
      });
      setNckRowIds((prev) => {
        const next = [...prev];
        next.splice(insertAt, 0, entry.rowId);
        return next;
      });
      return;
    }
    if (!nck && entry.selection) {
      const insertAt = Math.min(Math.max(0, entry.index), sels.length);
      setSels((prev) => {
        const next = [...prev];
        next.splice(insertAt, 0, entry.selection!);
        return next;
      });
      setSelRowIds((prev) => {
        const next = [...prev];
        next.splice(insertAt, 0, entry.rowId);
        return next;
      });
    }
  };

  // ---- 抽屉开合（§4.1 / §4.2） -------------------------------------------
  const openAddDrawer = () => {
    // 记录列表滚动位置：关闭后回到同一位置（不把用户顶回顶部）。
    restoreScrollTop.current = listScrollRef.current?.scrollTop ?? 0;
    // 用户主动开抽屉 ⇒ 上一次「问题定位」的高亮不再适用（不把旧定位带进新编辑）。
    setLocatedIssue(null);
    setEditingIndex(null);
    setEditingSelIndex(null);
    setBrowsePreset(null);
    setBrowseIdentityKeys(null);
    setFormVersion((v) => v + 1);
    setDrawerDirty(false);
    setDrawerOpen(true);
  };

  const openEditDrawer = (rowId: string) => {
    const index = nckRowIds.indexOf(rowId);
    if (index < 0) return;
    restoreScrollTop.current = listScrollRef.current?.scrollTop ?? 0;
    setLocatedIssue(null);
    setEditingIndex(index);
    setEditingSelIndex(null);
    setBrowsePreset(null);
    setFormVersion((v) => v + 1);
    setDrawerDirty(false);
    setDrawerOpen(true);
  };

  /**
   * 通用资源：就地编辑某一行（用户主动点「编辑」，或字段级问题定位时由
   * `locateIssueRow` 打开，见那里的 `setEditingSelIndex`）。
   *
   * 服务端字段级校验（`issues[].path = ...parameters.<field>`）按 `issueLocation.ts`
   * 的核实主要出现在这条路径上，因此必须有可编辑入口——否则字段名只能看不能改。
   */
  const openEditSelectionDrawer = (rowId: string) => {
    const index = selRowIds.indexOf(rowId);
    if (index < 0) return;
    restoreScrollTop.current = listScrollRef.current?.scrollTop ?? 0;
    setLocatedIssue(null);
    setEditingIndex(null);
    setEditingSelIndex(index);
    setBrowsePreset(null);
    setFormVersion((v) => v + 1);
    setDrawerDirty(false);
    setDrawerOpen(true);
  };

  /**
   * 替换一条资源选择（就地编辑的提交动作）。
   *
   * 与 `replacePoint` 同一条规则：**编辑不改 rowId**——同一行换了参数仍是同一行
   * （§4.8），否则行标记、行内编辑态会在保存前乱跳。
   */
  const replaceSelection = (index: number, selection: ResourceSelection): void => {
    setSels((prev) => prev.map((sel, i) => (i === index ? selection : sel)));
    setEditingSelIndex(null);
    setFormVersion((v) => v + 1);
    closeDrawer();
  };

  const closeDrawer = () => {
    setDrawerOpen(false);
    setEditingIndex(null);
    setEditingSelIndex(null);
    setDrawerDirty(false);
  };

  /**
   * 请求关闭抽屉：抽屉内**未确认的修改**必须先提醒一次（§4.1 补充）。
   * 保存中禁止关闭（§4.7）——按钮已被 `busy` 禁用挡住，这里再兜一层。
   */
  const requestCloseDrawer = () => {
    if (saving) return;
    if (!drawerDirty) {
      closeDrawer();
      return;
    }
    Modal.confirm({
      title: "放弃未确认的修改？",
      content: "抽屉里的修改还没有加入页面草稿，关闭后不会保留。",
      okText: "放弃修改",
      cancelText: "继续编辑",
      onOk: () => closeDrawer(),
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

  /**
   * 回读一致后把**草稿基线**整份替换（= 刚刚确认已生效的值）。
   *
   * 只在回读一致后调用：行标记「已修改/新增」、变更摘要与保存区都随之清零收起。
   * PUT 2xx 时就重置会让界面虚报「已保存」（§4.4.3）。
   */
  const rebaseDraftBaseline = useCallback((rows: DraftRowRef[], savedIntervalMs: number) => {
    setDraftBaseline({ rows, intervalMs: savedIntervalMs });
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
      rebaseDraftBaseline(draftRowRefs(), pending.baseline.intervalMs);
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
    // 提交快照：把「任务下标 + 任务内下标」与稳定 rowId 对齐，供保存失败后按
    // `issue.path` 定位（§5.2-5）。两次 merge 都是「保留任务 + 被编辑任务」的追加
    // 顺序（`deviceModel.ts:460`、`nckTaskBinding.ts:487`），NCK 直接用计划给出的
    // taskId 回查下标；下标对不齐时宁可不给快照（不猜行）。
    const editedTaskIndex =
      plan.driverKind === "nck"
        ? submittedTasks.findIndex((task) => task.id === plan.taskId)
        : submittedTasks.length - 1;
    const submittedRowIds = nck ? [...nckRowIds] : [...selRowIds];
    const rowIdsAligned =
      plan.driverKind !== "nck" || plan.points.length === submittedRowIds.length;
    const rowSnapshot: SubmittedRowSnapshot | null =
      editedTaskIndex >= 0 && rowIdsAligned
        ? { taskIndex: editedTaskIndex, taskCount: submittedTasks.length, rowIds: submittedRowIds }
        : null;
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
    setSubmittedRowSnapshot(rowSnapshot);
    setLocatedIssue(null);
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
            // 草稿基线同步替换：行标记清除、摘要与保存区收起（成功只报一次）。
            rebaseDraftBaseline(draftRowRefs(), intervalMs);
            return { ok: true };
          }
          // 后端 issues 原样保存并展示（`{valid:false, issues}`）；
          // 普通 `{error:{message}}` / HTTP 异常仍走原 fallback，不退化。
          const issues = (j as { issues?: ValidationIssue }) as { issues?: unknown };
          if (Array.isArray(issues.issues)) {
            const list = issues.issues as ValidationIssue[];
            setSaveIssues(list);
            // 定位：只对**能按提交快照换算成行**的问题跳转（§5.2-5）；
            // 任务级/保留任务/无法识别的 path 一律不跳，留给「技术详情」。
            const hit = firstLocatableIssue(list, rowSnapshot);
            if (hit) locateIssueRow(hit.issue, rowSnapshot);
            // 面向用户的一句话（**不含原始 path/code**）：原始值只在技术详情里。
            const summary =
              list.length > 0
                ? hit
                  ? `点位保存失败（${list.length} 项）：已跳到第一个可定位的问题，其余见「技术详情」`
                  : `点位保存失败（${list.length} 项）：该问题无法自动定位，详见「技术详情」的原始路径`
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

  // ---- 点位行视图（页面层：唯一允许按驱动分流的地方，§2.8） --------------
  const definitionOf = useCallback(
    (point: NckPointSelection): NckDefinitionLike | null => {
      const variableId =
        typeof point.parameters?.variable_id === "string" ? point.parameters.variable_id : "";
      const revision =
        typeof point.parameters?.definition_revision === "number"
          ? point.parameters.definition_revision
          : null;
      return variableId ? lookupDefinition(variableId, revision) : null;
    },
    [lookupDefinition],
  );

  const rowViews = useMemo<PointRowView[]>(() => {
    if (nck) {
      return buildNckPointRows({
        points: nckPoints,
        rowIds: nckRowIds,
        verificationOf: (index): RowVerification | undefined => {
          if (!statusesCurrent) return { status: "pending" };
          const status = pointStatuses[index];
          return status
            ? { status: status.status, code: status.code, message: status.message }
            : undefined;
        },
        definitionOf,
      });
    }
    return buildSelectionRows({
      selections: sels,
      rowIds: selRowIds,
      resources: desc?.resources ?? [],
    });
  }, [nck, nckPoints, nckRowIds, statusesCurrent, pointStatuses, definitionOf, sels, selRowIds, desc]);

  /**
   * 通用资源某一行能否**就地编辑**（不看 `driver_id`、不看资源 id 特判）：
   *
   * 判据只有两条，完全来自 Descriptor：
   * 1. 该选择指向的资源在 `desc.resources` 里存在（不存在就没有表单可渲染）；
   * 2. 该资源参数 Schema 里没有界面不认识的字段类型（A3：整份只读，不假装能编辑）。
   */
  const selectionRowEditable = useCallback(
    (selection: ResourceSelection | undefined): boolean => {
      if (!selection) return false;
      const resource = (desc?.resources ?? []).find((r) => r.id === selection.resource_id);
      if (!resource) return false;
      return unsupportedFields(resource.parameters).length === 0;
    },
    [desc],
  );

  /**
   * 字段级问题能否落在**可编辑控件**上：只按通用 Schema 的字段 key 判定。
   *
   * 字段 key 必须真的存在于该行资源的参数 Schema 里——不存在就说明界面没有对应控件
   * （或后端报的是前端不认识的字段），此时**不假装高亮**，退回只读技术详情并说明。
   */
  const selectionFieldEditable = useCallback(
    (rowId: string, fieldKey: string | null): boolean => {
      if (!fieldKey) return false;
      const index = selRowIds.indexOf(rowId);
      const selection = index >= 0 ? sels[index] : undefined;
      if (!selection || !selectionRowEditable(selection)) return false;
      const resource = (desc?.resources ?? []).find((r) => r.id === selection.resource_id);
      return (resource?.parameters.fields ?? []).some((field) => field.key === fieldKey);
    },
    [selRowIds, sels, desc, selectionRowEditable],
  );

  /**
   * 定位落点判定（**只判定、不执行**）：字段级问题能不能在编辑表单里就地改？
   *
   * NCK 保持既有行为（字段级 ⇒ 打开语义/原生表单）；通用资源按上面的 Schema 判定。
   */
  const canLocateFieldInPlace = useCallback(
    (rowId: string, fieldKey: string | null): boolean => {
      if (!fieldKey) return false;
      if (nck) return true;
      return selectionFieldEditable(rowId, fieldKey);
    },
    [nck, selectionFieldEditable],
  );

  /** 每行的「可编辑性」判定（原样保留既有准入口径，不下沉到通用组件）。 */
  const editableRowIds = useMemo(() => {
    const ids = new Set<string>();
    if (!nck) {
      // 通用资源：资源 Schema 全部字段类型都认识 ⇒ 可以就地编辑这一行。
      sels.forEach((selection, index) => {
        const rowId = selRowIds[index];
        if (rowId && selectionRowEditable(selection)) ids.add(rowId);
      });
      return ids;
    }
    nckPoints.forEach((point, index) => {
      const rowId = nckRowIds[index];
      if (!rowId) return;
      const variableId =
        typeof point.parameters?.variable_id === "string" ? point.parameters.variable_id : "";
      const revision =
        typeof point.parameters?.definition_revision === "number"
          ? point.parameters.definition_revision
          : null;
      const exactDefinition =
        variableId && revision !== null ? lookupDefinition(variableId, revision) : null;
      const latestDefinition = variableId ? definitions?.[variableId] : undefined;
      const revisionIsCurrent =
        !!exactDefinition &&
        (!latestDefinition ||
          typeof exactDefinition.revision !== "number" ||
          latestDefinition.revision === exactDefinition.revision);
      const gate =
        point.kind === "nck_native"
          ? !!nativeSchema
          : exactDefinition && revisionIsCurrent
            ? evaluateNckSelectionAdd(exactDefinition).ok
            : false;
      if (gate) ids.add(rowId);
    });
    return ids;
  }, [nck, nckPoints, nckRowIds, lookupDefinition, definitions, nativeSchema, sels, selRowIds, selectionRowEditable]);

  /**
   * 把一条保存失败的问题定位到「行（+字段）」并执行三个动作：跳页、打开抽屉、高亮。
   *
   * 三条硬约束（§5.2-5）：
   * 1. **只按提交快照换算**（`issueLocation.ts`），不拿实时列表下标去猜；
   * 2. 换算不出行就**什么都不做**（不挑一行跳过去），由界面明确说「无法自动定位」；
   * 3. 字段级问题打开**编辑表单**（那里能就地改并高亮字段），行级问题打开该行的
   *    「技术详情」，避免在用户没准备编辑时把可编辑表单摊开。
   *
   * 第 3 条的补充（本轮修复）：字段级问题**不区分驱动**——通用资源路径才是真实后端
   * 字段级校验的主路径（见 `issueLocation.ts:18-22`）。能否就地编辑只由该行资源
   * Schema 决定（字段 key 是否存在、字段类型是否都认识），不看 `driver_id`/资源 id。
   * 不可就地编辑时**不假装高亮**：打开只读技术详情，并明确写出原因。
   */
  function locateIssueRow(
    issue: ValidationIssue,
    snapshot: SubmittedRowSnapshot | null,
  ): IssueLocation {
    const location = locateIssuePath(issue.path, snapshot);
    if (location.kind !== "row") return location;
    const ids = nck ? nckRowIds : selRowIds;
    const index = ids.indexOf(location.rowId);
    if (index < 0) {
      // 行身份还在快照里、但已不在当前草稿（用户又改过草稿）：同样不猜。
      return { kind: "unlocatable", reason: "unrecognized", detail: "目标行已不在当前草稿中" };
    }
    // 目标行必须可见：先清掉搜索与筛选（否则跳到那一页也可能被过滤掉），
    // 再按「行在全部行里的下标」算页码——rowId 与页码都只由下标推导，不改行身份。
    setQuery("");
    setFilter("all");
    setPage(Math.floor(index / Math.max(1, pageSize)) + 1);
    const editableInPlace = canLocateFieldInPlace(location.rowId, location.fieldKey);
    setLocatedIssue({
      rowId: location.rowId,
      fieldKey: location.fieldKey,
      inPlaceEditable: editableInPlace,
    });
    if (editableInPlace && location.fieldKey) {
      restoreScrollTop.current = listScrollRef.current?.scrollTop ?? 0;
      if (nck) setEditingIndex(index);
      else setEditingSelIndex(index);
      setBrowsePreset(null);
      setFormVersion((v) => v + 1);
      setDrawerDirty(false);
      setDrawerOpen(true);
    } else {
      const row = rowViews.find((item) => item.rowId === location.rowId);
      if (row) setDetailRow(row);
    }
    return location;
  }

  // ---- 草稿差异与保存区（§4.7 / §4.8） -----------------------------------
  const draftRows = useMemo(() => draftRowRefs(), [draftRowRefs]);
  const diff = useMemo(
    () => diffDraft({ baseline: draftBaseline, draft: draftRows, intervalMs }),
    [draftBaseline, draftRows, intervalMs],
  );
  const markedRows = useMemo(
    () =>
      rowViews.map((row) => {
        const marker = diff.rowMarkers[row.rowId];
        return marker ? { ...row, marker } : row;
      }),
    [rowViews, diff],
  );
  const rowCounts = countRowStates(markedRows);
  const filterOptions = buildFilterOptions(rowCounts);
  const visibleRows = applyRowQuery(markedRows, { query, filter });
  const paged = paginateItems(visibleRows, page, pageSize);

  // 页码越界（筛选/每页条数变化、行被移除）时收敛到有效页；不改 rowId。
  useEffect(() => {
    if (paged.page !== page) setPage(paged.page);
  }, [paged.page, page]);

  /**
   * 把当前草稿写进**路由之外**的副本（§7-7 第 5 步）。
   *
   * 时机很关键：只在服务端快照就绪后写。加载前的空态（`sels=[]` / `intervalMs=1000`）
   * 绝不允许覆盖上一份副本，否则「后退时短暂卸载」期间写一次空态就把草稿抹掉了。
   * 没有未保存修改时删掉副本：不留无意义的残留，也避免下次进页面"复活"旧内容。
   */
  useEffect(() => {
    if (!isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId)) return;
    if (!diff.hasChanges) {
      draftStash.drop(stashKey);
      return;
    }
    draftStash.put(stashKey, {
      intervalMs,
      sels,
      selRowIds,
      nckPoints,
      nckRowIds,
      removedEntries,
      restart,
    } satisfies StashedDraft);
  }, [
    draftStash,
    stashKey,
    tasksLoadState,
    tasksLoadedEpId,
    endpointId,
    diff.hasChanges,
    intervalMs,
    sels,
    selRowIds,
    nckPoints,
    nckRowIds,
    removedEntries,
    restart,
  ]);

  // 抽屉关闭后回到原列表位置（§2.7「位置保持」，不把用户顶回顶部）。
  useEffect(() => {
    if (drawerOpen) return;
    const el = listScrollRef.current;
    if (el && restoreScrollTop.current !== null) el.scrollTop = restoreScrollTop.current;
  }, [drawerOpen]);

  // 保存结果只出现一次，随后自动收起（§4.7「已保存」→ 自动退出）。
  useEffect(() => {
    if (!lastApply || lastApply === "restart-failed" || diff.hasChanges) return;
    const timer = setTimeout(() => setLastApply(null), 5000);
    return () => clearTimeout(timer);
  }, [lastApply, diff.hasChanges]);

  const saveBar = saveBarView({
    hasChanges: diff.hasChanges,
    saving,
    pendingVerify: !!pendingVerify,
    restartFailed: !!restartFailed,
    savedResult:
      lastApply === "restarted" ? "restarted" : lastApply === "stopped" ? "stopped" : null,
    runState,
    restart,
    // 与提交函数**共用同一判定**（§4.4.2）：按钮可用性只是体验，真正的防线在 save()。
    canSave:
      isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId) &&
      !hasIncompatible &&
      !editBlock &&
      (nck
        ? !!definitions &&
          !!endpointFamily &&
          nckPoints.length > 0 &&
          admissionIssues.length === 0 &&
          verificationReady &&
          unverified.length === 0
        : sels.length > 0 && !!desc) &&
      saveChangeState === "changed" &&
      !pendingVerify,
  });

  /**
   * 保存失败问题的**分层展示**（需求 3）：
   * - 正文只回答「有几个、能不能点过去」；
   * - 原始 `path` / `code` 只在技术详情里。
   *
   * 可定位 = 能按提交快照换算成行；换算不出来就明确说「无法自动定位」，不猜目标。
   */
  const locatableIssue = firstLocatableIssue(saveIssues, submittedRowSnapshot);
  const firstUnlocatable = saveIssues
    .map((issue) => locateIssuePath(issue.path, submittedRowSnapshot))
    .find(
      (loc): loc is Extract<IssueLocation, { kind: "unlocatable" }> =>
        loc.kind === "unlocatable",
    );
  // 正文只承诺**做得到**的事：字段级且能就地编辑才说"高亮字段"；
  // 字段级但不可就地编辑时说"打开技术详情"，不假装高亮了某个控件。
  const locatableFieldInPlace = locatableIssue
    ? canLocateFieldInPlace(locatableIssue.location.rowId, locatableIssue.location.fieldKey)
    : false;
  const locateLeadAction = !locatableIssue
    ? null
    : locatableIssue.location.fieldKey
      ? locatableFieldInPlace
        ? "并高亮字段"
        : "并打开技术详情（该字段无法就地编辑）"
      : "并高亮该行";

  /** 放弃修改：整份回到服务端已保存值（不写服务端，只重读）。 */
  const discardDraft = () => {
    if (saving || rechecking) return;
    setReloadNonce((v) => v + 1);
  };

  // 把「有未保存修改 / 正在保存」上报给外壳层：切换连接与离开页面的提示由外壳做
  // （路由与连接选择不在本组件的职责内，见 §4.7 / §7-7）。
  useEffect(() => {
    onDraftStateChange?.({ dirty: diff.hasChanges, saving });
  }, [onDraftStateChange, diff.hasChanges, saving]);

  const copyPointKey = (row: PointRowView) => {
    const key = row.detail.find((entry) => entry.label === "point_key")?.value ?? "";
    const clipboard = (globalThis as { navigator?: Navigator }).navigator?.clipboard;
    if (clipboard?.writeText) {
      void clipboard.writeText(key).then(
        () => message.success("已复制标识"),
        () => message.warning("复制失败：请从技术详情手动复制"),
      );
      return;
    }
    message.warning("当前环境不支持剪贴板：请从技术详情手动复制");
  };

  const canAdd = !editBlock && (nck ? !!definitions : !!desc && (desc.resources ?? []).length > 0);
  const descOk = nck || !!desc;
  if (!descOk && tasksLoadState === "loading") return <div style={{ color: "#525252" }}>加载资源…</div>;

  const stateText =
    runState === "running" ? "运行中" : runState === "stopped" ? "已停止" : "状态未知";
  const draftPointCount = nck ? nckPoints.length : sels.length;

  // 不再限制面板最大宽度。原先写死 maxWidth 980 是按「表单可读宽度」设的，
  // 但采集页的主体是点位表格 —— 宽屏下整块被压在 980px 内、右侧留大片空白，
  // 表格三列也挤在一起。改为占满可用宽度，由表格自身的三列 grid 分配空间。
  return (
    <div style={{ display: "grid", gap: 12, minWidth: 0 }}>
      {/* 真实问题照常出现（§3.3）：加载失败/配置损坏/未知状态都不得被「首屏安静」掩盖。 */}
      {tasksLoadState === "error" ? (
        <Alert
          type="error"
          showIcon
          data-testid="tasks-load-error"
          message="配置加载失败，暂时无法编辑"
          description={
            <div style={{ display: "grid", gap: 6 }}>
              <span>{tasksLoadError || "任务快照加载失败"}</span>
              <span>
                <Button size="small" data-testid="retry-load" onClick={() => setReloadNonce((v) => v + 1)}>
                  重试
                </Button>
              </span>
            </div>
          }
        />
      ) : null}
      {tasksLoadState === "loading" ? (
        <div style={{ fontSize: 12, color: "#525252" }}>正在加载任务快照…（快照就绪前不会保存）</div>
      ) : null}
      {editBlock ? (
        <Alert
          type="error"
          showIcon
          message="现有任务无法安全编辑，已禁用保存"
          description={`${editBlock.code}：${editBlock.message}`}
        />
      ) : null}
      {!descOk ? (
        <Alert
          type="error"
          showIcon
          message="资源描述加载失败"
          description="描述缺失时选型无合法依据，已禁用保存（不会覆盖已有任务）。请切换后重试。"
        />
      ) : null}

      {/* 一行运行摘要（§2.2：四段状态条收敛为一行）。 */}
      <div
        data-testid="acquisition-status"
        style={{
          display: "flex",
          gap: 12,
          flexWrap: "wrap",
          fontSize: 12,
          color: "#525252",
          alignItems: "baseline",
        }}
      >
        <span>
          采集状态：<b data-testid="acquisition-status-state">{stateText}</b>
        </span>
        <span>·</span>
        <span>
          点位：
          <b data-testid="acquisition-status-points">
            {savedVsDraftText(
              savedBaseline?.hasTask ? savedBaseline.pointCount : null,
              draftPointCount,
              "个",
            )}
          </b>
        </span>
        <span>·</span>
        <span>
          周期：
          <b data-testid="acquisition-status-interval">
            {savedVsDraftText(
              savedBaseline?.hasTask ? savedBaseline.intervalMs : null,
              intervalMs,
              "ms",
            )}
          </b>
        </span>
      </div>

      {/* 运行状态未知：禁止一切保存（含只写配置），只给刷新入口；草稿仍可编辑。 */}
      {runStateSettled && runState === "unknown" ? (
        <Alert
          type="warning"
          showIcon
          data-testid="acquisition-run-state-unknown"
          message="运行状态未知，已禁用保存"
          description="读取 Endpoint 运行状态失败，或该 Endpoint 不在清单中。为避免在未知状态下停止/写入/启动采集，保存已禁用（含只写配置）；草稿仍可编辑。"
          action={
            <Button size="small" data-testid="refresh-run-state" loading={saving} onClick={() => void loadRunState()}>
              刷新状态
            </Button>
          }
        />
      ) : null}

      {/* 已有配置存在真实问题：一条摘要 + 可处理原因；原始错误码收进技术详情。 */}
      {unverified.length > 0 ? (
        <Alert
          type="error"
          showIcon
          data-testid="nck-legacy-identity-alert"
          message={`${unverified.length} 个点位需要处理：当前任务包含不可采集的身份探针点位`}
          description={
            <div>
              <div>
                请检查并手动移除这些点位，重新验证剩余点位后再保存。这些点位不会被自动删除，也不会被自动转成 collect；只能由你显式移除。
              </div>
              <details style={{ marginTop: 4 }}>
                <summary style={{ cursor: "pointer", fontSize: 11 }}>技术详情</summary>
                <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
                  {unverified.map((status, idx) => (
                    <li
                      key={`${status.point.point_key}-${idx}`}
                      style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 11 }}
                    >
                      {status.point.point_key} · {status.code} · {status.message}
                    </li>
                  ))}
                </ul>
              </details>
            </div>
          }
        />
      ) : null}
      {admissionIssues.length > 0 && unverified.length === 0 ? (
        <Alert
          type="error"
          showIcon
          data-testid="nck-admission-blocked"
          message={`存在不可采集点位（${admissionIssues.length} 项，scope 准入拒绝）`}
          description={
            <details>
              <summary style={{ cursor: "pointer", fontSize: 11 }}>技术详情</summary>
              <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
                {admissionIssues.map((issue, idx) => (
                  <li key={idx} style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 11 }}>
                    {issue.pointKey} · {issue.code} · {issue.message}
                  </li>
                ))}
              </ul>
            </details>
          }
        />
      ) : null}

      {/* 采集周期：一个输入框 + 单位（§2.2）。压测数字只在帮助提示里出现。 */}
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12 }}>采集周期</span>
        <InputNumber
          min={10}
          max={60000}
          step={10}
          value={intervalMs}
          onChange={(v) => setIntervalMs(v ?? 1000)}
          addonAfter="ms"
          data-testid="acquisition-interval-input"
          style={{ width: 180 }}
        />
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
        <span style={{ fontSize: 11, color: "#8c8c8c" }} data-testid="interval-note">
          周期为配置值，实际采集表现以运行结果为准。
        </span>
      </div>

      {descOk ? (
        <>
          {/* 列表标题 + 唯一添加入口（默认页面不常驻表单，§2.2）。 */}
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>已配置点位（{draftPointCount}）</span>
            <span style={{ marginLeft: "auto" }}>
              {canAdd ? (
                <Button
                  size="small"
                  type="primary"
                  ghost
                  data-testid="add-point-entry"
                  // 保存中不再开放新的编辑动作（避免「请求还在飞、草稿又变」）。
                  disabled={saving}
                  onClick={openAddDrawer}
                >
                  + 添加点位
                </Button>
              ) : null}
            </span>
          </div>

          <PointSearch
            query={query}
            placeholder="搜索名称、标识或配置内容……"
            onChange={(next) => {
              setQuery(next);
              setPage(1);
            }}
            filters={filterOptions}
            filter={filter}
            onFilterChange={(next) => {
              setFilter(next);
              setPage(1);
            }}
          />

          {/*
          列表滚动区按视口高度自适应，不再写死固定高度。
          写死 520px 时在 1080p 屏幕上列表只占约一半，下方留大片空白；而固定过高
          又会在小屏把分页挤到视口外。用 calc(100vh - 占位) 让列表吃掉剩余高度，
          并保留 minHeight 以免极窄视口下列表被压成一条缝。
          占位约为：页头 + 连接行 + 周期行 + 列表标题 + 搜索行 + 分页行 + 保存区。
        */}
        <div
          ref={listScrollRef}
          data-testid="point-list-scroll"
          style={{
            overflowY: "auto",
            maxHeight: "calc(100vh - 360px)",
            minHeight: 240,
          }}
        >
            <PointTable
              rows={paged.rows}
              total={paged.total}
              page={paged.page}
              pageSize={pageSize}
              pageCount={paged.pageCount}
              pageSizeOptions={[25, 50, 100]}
              onPageChange={setPage}
              onPageSizeChange={(size) => {
                setPageSize(size);
                setPage(1);
              }}
              rowTestId={nck ? "nck-point-row" : "sel-point-row"}
              rowData={(row) => {
                const status = nck ? pointStatuses[nckPoints.findIndex((_, i) => nckRowIds[i] === row.rowId)] : undefined;
                return {
                  "data-verify-status": status ? status.status : "none",
                  // 被保存失败的问题定位到的行：只做锚点标记，不加视觉标签（§2.9 元素预算）。
                  "data-issue-located": locatedIssue?.rowId === row.rowId ? "true" : "false",
                };
              }}
              isEditable={(row) => editableRowIds.has(row.rowId)}
              actions={{
                // 编辑入口按载荷类型分流（不按驱动名判断）：
                // NCK 点位 → `nckRowIds`；通用资源选择 → `selRowIds`。
                onEdit: (row) => {
                  if (nck) openEditDrawer(row.rowId);
                  else openEditSelectionDrawer(row.rowId);
                },
                onRemove: (row) => {
                  if (nck) {
                    const index = nckRowIds.indexOf(row.rowId);
                    if (index >= 0) removePoint(index);
                  } else {
                    const index = selRowIds.indexOf(row.rowId);
                    if (index >= 0) removeSelection(index);
                  }
                },
                onDetail: (row) => setDetailRow(row),
                onCopyKey: copyPointKey,
              }}
              emptyState={
                query || filter !== "all" ? (
                  <div style={{ padding: 16, fontSize: 12, color: "#525252" }}>
                    没有匹配的点位，可改用标识搜索。
                    <Button
                      size="small"
                      type="link"
                      data-testid="clear-point-search"
                      onClick={() => {
                        setQuery("");
                        setFilter("all");
                        setPage(1);
                      }}
                    >
                      清除搜索
                    </Button>
                  </div>
                ) : (
                  <div style={{ padding: 16, fontSize: 12, color: "#525252" }}>
                    还没有配置点位{canAdd ? "：点「+ 添加点位」开始配置。" : "。"}
                  </div>
                )
              }
              showPagination={false}
            />
          </div>
          {/* 分页固定在滚动区之外：列表滚动时分页常驻可见，不会跟着行一起滚走。 */}
          <PointPagination
            total={paged.total}
            page={paged.page}
            pageSize={pageSize}
            pageCount={paged.pageCount}
            pageSizeOptions={[25, 50, 100]}
            onPageChange={setPage}
            onPageSizeChange={(size) => {
              setPageSize(size);
              setPage(1);
            }}
          />

          {/* 移除先改草稿，可撤销（§7-9）：只有确实移除过才出现，且带处理动作。 */}
          {removedEntries.length > 0 ? (
            <NoticeSlot
              level="info"
              testId="nck-removed-points"
              text={`已从草稿移除 ${removedEntries.length} 项（保存不会包含它们）`}
              action={
                <Button
                  size="small"
                  type="link"
                  data-testid="undo-remove"
                  onClick={() => undoRemove(removedEntries[0]!.rowId)}
                >
                  撤销
                </Button>
              }
              detail={
                <ul style={{ margin: 0, paddingLeft: 18 }}>
                  {removedEntries.map((entry) => (
                    <li
                      key={entry.rowId}
                      style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 11, color: "#525252" }}
                    >
                      {entry.point?.point_key ??
                        entry.selection?.outputs.map((output) => output.point_key).join(", ") ??
                        entry.rowId}
                      {" · "}
                      {entry.reason}
                      <div>
                        原始配置：
                        {JSON.stringify(entry.point?.parameters ?? entry.selection?.parameters ?? {})}
                      </div>
                    </li>
                  ))}
                </ul>
              }
            />
          ) : null}

          {/* 保留任务：一行静态说明（替代旧的「已回显 N 任务」加载即弹提示，§2.5）。 */}
          {preservedTasks.length > 0 ? (
            <div data-testid="preserved-tasks-note" style={{ fontSize: 12, color: "#525252" }}>
              另有 {preservedTasks.length} 个任务不在此编辑，保存时原样保留：
              {preservedTasks.map((task) => task.id).join("、")}
            </div>
          ) : null}
        </>
      ) : null}

      {hasIncompatible ? (
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
                    style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 11 }}
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

      {/* 保存失败 / 状态变化 / 待核对：持久展示 + 可点击的下一步（§4.6-4）。 */}
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
                      onClick={() => void save(pendingVerify.restart, { fromPendingResave: true })}
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

      {saveIssues.length > 0 ? (
        <Alert
          type="error"
          message={`保存被后端拒绝：${saveIssues.length} 个问题需要处理`}
          data-testid="save-issues"
          description={
            <div style={{ display: "grid", gap: 8 }}>
              {/* 正文：只说「能不能处理、点哪里」，**不含**原始 path / code（§3.3 分层）。 */}
              <div data-testid="save-issues-lead">
                {locatableIssue
                  ? `${saveIssues.length} 个问题需要处理，点此查看第一个（跳到该行所在页${locateLeadAction ?? ""}）。`
                  : `${saveIssues.length} 个问题需要处理；该问题无法自动定位（${describeUnlocatable(
                      firstUnlocatable?.reason ?? "unrecognized",
                    )}），请按技术详情里的原始路径排查。`}
              </div>
              {locatableIssue ? (
                <div>
                  <Button
                    size="small"
                    data-testid="locate-first-issue"
                    onClick={() => locateIssueRow(locatableIssue.issue, submittedRowSnapshot)}
                  >
                    点此查看第一个
                  </Button>
                </div>
              ) : null}
              {/* 技术详情：原始 path / code / message 原样保留（可排查、可断言）。 */}
              <details data-testid="save-issues-details">
                <summary style={{ cursor: "pointer", fontSize: 12 }}>技术详情（原始 path / code）</summary>
                <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
                  {saveIssues.map((issue, idx) => (
                    <li
                      key={idx}
                      data-testid="issue-raw"
                      style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11 }}
                    >
                      {issue.path} · {issue.code} · {issue.message}
                    </li>
                  ))}
                </ul>
              </details>
            </div>
          }
        />
      ) : null}

      {/* 固定保存区（§2.3/§4.7）：没有修改且无待处理结果时**整个不渲染**。 */}
      <DraftSaveBar
        view={saveBar}
        summary={diff.summary}
        noticeDetail={restartFailed ?? null}
        onSave={() => void save(running ? restart : false)}
        onDiscard={discardDraft}
        onRetryStart={() => void retryStart()}
        onRefreshState={() => void loadRunState()}
        onToggleRestart={() => setRestart((value) => !value)}
        retryBusy={saving}
      />

      {/* 添加/编辑抽屉（§4.1/§4.2）：默认页面不常驻表单。 */}
      <PointDrawer
        open={drawerOpen}
        title={editingIndex !== null || editingSelIndex !== null ? "编辑点位" : "添加点位"}
        busy={saving}
        onClose={requestCloseDrawer}
        footer={
          <Button data-testid="drawer-cancel" disabled={saving} onClick={requestCloseDrawer}>
            取消
          </Button>
        }
      >
        {nck ? (
          <div style={{ display: "grid", gap: 12 }}>
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
                data-testid="nck-definitions-missing"
                message="定义目录不可用：NCK 保存已禁用（fail-closed）"
                description="没有定义目录就无法确认哪些变量可采集（scope 准入），因此保存保持禁用；在可用前不会出现“保存成功但编辑没生效”。"
              />
            ) : (
              <div style={{ fontSize: 12, color: "#525252" }} data-testid="nck-collectable-summary">
                {/* 目录统计不在默认页面上：收进抽屉（搜索即列表，选中才展示定义信息）。 */}
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

            {/* Browse 入口：只在驱动声明了 browse 时出现（不新增特判，§4.1-2）。 */}
            {definitions && (desc?.resource_selection_methods ?? []).includes("browse") ? (
              <div style={{ display: "grid", gap: 8 }}>
                <div>
                  <Button
                    size="small"
                    type="link"
                    data-testid="nck-browse-toggle"
                    onClick={() => setBrowseOpen((v) => !v)}
                  >
                    {browseOpen ? "收起浏览" : "从浏览树选择语义变量"}
                  </Button>
                </div>
                {browseOpen ? (
                  <ResourceBrowseAntd
                    endpointId={endpointId}
                    onFill={() => message.warning("请选用语义点位（旧资源入口在 NCK 上已被禁用）")}
                    // 语义载荷解析由 NCK 处理器注入；可选性/阻断原因由驱动给出（IPC 1.8）。
                    semanticHandler={createNckSemanticHandler()}
                    onSemanticSelect={(sel) => {
                      setBrowsePreset({
                        kind: sel.pointKind,
                        point_key: "",
                        parameters: sel.parameters,
                      });
                      // 只取 wire 的 identity.keys（"还需要哪类实例"）；preview_point_key
                      // 仅展示，不进提交载荷。
                      setBrowseIdentityKeys(sel.identity?.keys ?? null);
                      setEditingIndex(null);
                      setFormVersion((v) => v + 1);
                      message.info("已把浏览叶带入语义表单：请等待服务端预览确认身份");
                    }}
                  />
                ) : null}
              </div>
            ) : null}

            {definitions ? (
              editingIndex !== null && nckPoints[editingIndex] ? (
                <NckPointFields
                  key={`edit-${endpointId}-${editingIndex}-${nckPoints[editingIndex].point_key}-${formVersion}`}
                  driverId={driverId}
                  endpointFamily={endpointFamily}
                  catalogFamily={catalogFamily}
                  definitions={collectableDefs}
                  takenPointKeys={takenPointKeys(editingIndex)}
                  initialPoint={nckPoints[editingIndex]}
                  nativeSchema={nativeSchema}
                  endpointId={endpointId}
                  probeGate={probeGate}
                  submitLabel="确认修改"
                  onDirtyChange={setDrawerDirty}
                  onCancel={requestCloseDrawer}
                  onSubmit={(point) => replacePoint(editingIndex, point)}
                  // 字段级定位：只有「被定位的就是正在编辑的这一行」才把字段交给表单高亮。
                  focusField={
                    locatedIssue && nckRowIds[editingIndex] === locatedIssue.rowId
                      ? locatedIssue.fieldKey
                      : null
                  }
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
                  // 「选择实际对象」：只有已创建且连接可读的端点才允许按需探测。
                  endpointId={endpointId}
                  probeGate={probeGate}
                  objectIdentityKeys={browsePreset ? browseIdentityKeys : null}
                  submitLabel="加入待保存列表"
                  onDirtyChange={setDrawerDirty}
                  onSubmit={addPoint}
                />
              )
            ) : null}
          </div>
        ) : (
          <ResourcePickerAntd
            key={`sel-${endpointId}-${editingSelIndex ?? "add"}-${formVersion}`}
            resources={desc?.resources ?? []}
            // 就地编辑：把**被编辑的这一行**从 reconciliation 的 editable 集合里排除，
            // 否则提交自己会被判成「与已有项重复」；protected 永远不动。
            existingSelections={
              editingSelIndex !== null ? sels.filter((_, i) => i !== editingSelIndex) : sels
            }
            protectedSelections={preservedTasks.flatMap((t) => selectionsOf(t))}
            selectionMethods={desc?.resource_selection_methods ?? []}
            endpointId={endpointId}
            requireExplicitSelection
            initialSelection={
              editingSelIndex !== null && sels[editingSelIndex]
                ? {
                    resource_id: sels[editingSelIndex]!.resource_id,
                    parameters: sels[editingSelIndex]!.parameters,
                    outputs: sels[editingSelIndex]!.outputs,
                  }
                : null
            }
            // 字段级定位：只有「被定位的就是正在编辑的这一行」才把字段交给表单高亮。
            focusFieldKey={
              editingSelIndex !== null &&
              locatedIssue &&
              selRowIds[editingSelIndex] === locatedIssue.rowId
                ? locatedIssue.fieldKey
                : null
            }
            submitLabel={editingSelIndex !== null ? "确认修改" : "加入待保存列表"}
            onDirtyChange={setDrawerDirty}
            onAdd={(selection) => {
              // 就地编辑：替换这一行（rowId 不变，§4.8），不做 merge/append。
              if (editingSelIndex !== null) {
                replaceSelection(editingSelIndex, {
                  resource_id: selection.resource_id,
                  parameters: selection.parameters,
                  outputs: selection.outputs,
                });
                return true;
              }
              // 唯一防御门：与 AddDeviceFlow 同一共享实现（protected 永不动）。
              const schemaOf = (resourceId: string) => {
                const found = (desc?.resources ?? []).find((r) => r.id === resourceId);
                return {
                  fields: (found?.parameters.fields ?? []).map((f) => ({ key: f.key, default: f.default })),
                };
              };
              const { next, message: msg } = applySelectionAdd({
                candidate: selection,
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
              // 既有行按对象引用/身份键沿用 rowId；新增项发新号（不重排既有行）。
              setSelRowIds((prev) =>
                alignRowIds(
                  sels,
                  prev,
                  next,
                  rowIdAllocator.current,
                  (sel) => `${sel.resource_id}\u0000${stableIdentityKey(sel.parameters)}`,
                ),
              );
              closeDrawer();
              return true;
            }}
          />
        )}
      </PointDrawer>

      {/* 技术详情抽屉：完整点键 / 原始参数 / 定义版本 / 身份来源（主动展开才看）。 */}
      <PointDrawer
        open={!!detailRow}
        title="技术详情"
        notice={null}
        testId="point-detail-drawer"
        onClose={() => setDetailRow(null)}
        footer={
          <Button data-testid="detail-close" onClick={() => setDetailRow(null)}>
            关闭
          </Button>
        }
      >
        <div
          data-testid="point-detail"
          style={{
            display: "grid",
            gap: 6,
            fontSize: 12,
            fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace",
          }}
        >
          {/* 由保存失败的问题定位打开时：把「这一行为什么被定位」写在抽屉顶部。 */}
          {locatedIssue && detailRow?.rowId === locatedIssue.rowId ? (
            <div
              data-testid="point-detail-issue"
              data-located-field={locatedIssue.fieldKey ?? ""}
              data-in-place-editable={locatedIssue.inPlaceEditable ? "true" : "false"}
              style={{ color: "#c62828" }}
            >
              该行存在服务端校验问题
              {locatedIssue.fieldKey ? `（字段：${locatedIssue.fieldKey}）` : ""}
              {/* 字段级但**不能**就地编辑：如实说明，不假装高亮了某个控件。 */}
              {locatedIssue.fieldKey && !locatedIssue.inPlaceEditable
                ? " · 该字段无法就地编辑（资源参数 Schema 里没有这个字段，或含界面不支持的类型），请按下面的原始配置排查"
                : ""}
            </div>
          ) : null}
          {detailRow
            ? detailRow.detail.map((entry) => (
                <div key={entry.label} style={{ overflowWrap: "anywhere" }}>
                  <span style={{ color: "#525252" }}>{entry.label}：</span>
                  {entry.value}
                </div>
              ))
            : null}
        </div>
      </PointDrawer>
    </div>
  );
}
