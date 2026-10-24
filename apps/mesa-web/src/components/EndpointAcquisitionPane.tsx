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
  isNckDriver,
  isRunningState,
  isTaskSnapshotReady,
  selectionsOf,
  splitAcquisitionTasks,
  type AcquisitionTaskShape,
  type LifecycleStepResult,
  type NckPointSelection,
  type ResourceSelection,
  type TaskSnapshotState,
} from "../deviceModel";
import {
  collectableDefinitions,
  evaluateNckSelectionAdd,
} from "../nckCollectability";
import { nckPointsOf, parseTaskBinding, selectEditableTask } from "../nckTaskBinding";
import { planEndpointTaskSave, samePoints } from "../endpointSavePlan";
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
 * 保存后回读任务快照（仅 NCK 使用）。
 *
 * 为什么需要：`PUT /tasks` 返回 2xx 只证明"服务端接受了请求"，不证明"编辑已
 * 持久化"。旧实现正是在这里把 no-op 当成功汇报。NCK 路径因此额外回读一次，
 * 以点位是否原样回来作为成功判据。
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
  const [existingTasks, setExistingTasks] = useState<AcquisitionTaskShape[]>([]);
  const [preservedTasks, setPreservedTasks] = useState<AcquisitionTaskShape[]>([]);
  // 现有任务无法安全编辑（多候选 / 结构非法）：保存必须禁用，绝不猜第一个。
  const [editBlock, setEditBlock] = useState<{ code: string; message: string } | null>(null);
  const [running, setRunning] = useState(false);
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

  useEffect(() => {
    // 切 Endpoint 时先复位：旧快照不得污染新窗格（fail-closed 默认禁用保存）。
    setSels([]);
    setNckPoints([]);
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
    setPointStatuses([]);
    setRemovedPoints([]);
    setEditingIndex(null);
    setFormVersion(0);
    setBrowseOpen(false);
    setBrowsePreset(null);
    setEndpointFamily(null);
    setEndpointFamilyState("loading");
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
          } else {
            setPreservedTasks(selected.preserved);
            preservedIds = selected.preserved.map((t) => t.id);
            if (selected.editable?.schedule?.mode === "poll") {
              setIntervalMs(selected.editable.schedule.interval_ms ?? 1000);
            }
            // 只读 `{points:[…]}`：绝不把 NCK 点位当成 ResourceSelection[]。
            setNckPoints(nckPointsOf(selected.editable));
          }
        } else {
          const { editable, preserved } = splitAcquisitionTasks(tasks);
          setPreservedTasks(preserved);
          preservedIds = preserved.map((t) => t.id);
          if (editable) {
            if (editable.schedule?.mode === "poll") setIntervalMs(editable.schedule.interval_ms ?? 1000);
            const s = selectionsOf(editable);
            if (s.length) setSels(s);
          }
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
    api.listEndpoints()
      .then((j) => {
        if (cancelled) return;
        const list = ((j as { endpoints?: Array<{ id: string; runtime?: { state?: string }; state?: string; connection?: unknown }> }).endpoints ?? []);
        const live = list.find((e) => e.id === endpointId);
        setRunning(isRunningState(live?.state ?? live?.runtime?.state));
        // family 的唯一权威来源 = Endpoint 有效连接配置（不是目录顶层回显）。
        const connection = live?.connection;
        const rawFamily =
          connection && typeof connection === "object" && !Array.isArray(connection)
            ? (connection as { family?: unknown }).family
            : undefined;
        setEndpointFamily(typeof rawFamily === "string" && rawFamily.trim() ? rawFamily.trim() : null);
        setEndpointFamilyState("ready");
      })
      .catch(() => {
        if (cancelled) return;
        setEndpointFamily(null);
        setEndpointFamilyState("error");
      });
    return () => { cancelled = true; };
  }, [endpointId, driverId]);

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
  const editSignature = `${intervalMs}|${nckPoints.map((p) => p.point_key).join(",")}|${sels
    .map((s) => s.resource_id)
    .join(",")}`;
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
    // 表单重建一次：避免上一条目的输入（含 Browse 预填）残留到下一次新增。
    setBrowsePreset(null);
    setFormVersion((v) => v + 1);
  };

  const replacePoint = (index: number, point: NckPointSelection) => {
    if (takenPointKeys(index).includes(point.point_key)) {
      return message.error("该 point_key 已被 Endpoint 内其它点位占用：请调整 Target/Arguments（不得自动改名/加后缀）");
    }
    setNckPoints((prev) => prev.map((p, i) => (i === index ? point : p)));
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

  const save = async (restart: boolean) => {
    // 快照门：pending / 失败 / 串 Endpoint 一律不可 PUT。
    if (!isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId)) {
      if (tasksLoadState === "error") return message.error("任务快照加载失败，禁止保存以防覆盖已有任务。请切换后重试。");
      return message.warning("任务快照加载中，禁止保存以防覆盖已有任务。请稍候。");
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
      // 提交前再核验一次：防止「加载时通过、提交时目录/定义已变」。
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
    // 唯一保存计划入口：按 Driver 分流 + 快照门 + 准入 + 冲突 + 变更判定。
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
      // 无实际变更：**不发 PUT、不报成功**——"保存接口 2xx"不等于"编辑已持久化"。
      if (plan.code === "NO_CHANGES") return message.info(plan.message);
      // 其余（歧义 / 结构非法 / 准入 / 冲突 / 快照 / 目录缺失）一律明确失败。
      return message.error(plan.message);
    }
    const tasks = plan.tasks;
    const preservedCount = plan.preservedCount;
    // 回读核对参数先固定下来：闭包内不能再依赖联合类型收窄。
    const nckWrite =
      plan.driverKind === "nck" ? { taskId: plan.taskId, points: plan.points } : null;
    const kept = (extra: string) =>
      preservedCount > 0 ? `点位已保存（另保留 ${preservedCount} 个任务）${extra}` : `点位已保存${extra}`;
    setSaving(true);
    // 重新发起保存：清掉上一次的 issues（成功后同样清掉）。
    setSaveIssues([]);
    try {
      // 统一生命周期：STOPPED 直接 apply（绝不自动 start）；RUNNING 经
      // executor 走 stop → apply →（可选）start，各步失败都有明确 outcome。
      const outcome = await applyEndpointChange({
        wasRunning: running,
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
            body: JSON.stringify({ tasks }),
          });
          const j = await r.json().catch(() => ({}));
          if (r.ok) {
            setSaveIssues([]);
            // NCK：2xx 之后必须回读核对，否则"保存成功"只是接口语义。
            if (nckWrite) {
              const back = await readBackTasks(endpointId).catch(() => null);
              if (!back) {
                return {
                  ok: false,
                  message: "保存响应成功，但回读任务失败：无法确认编辑已持久化（按失败处理，不报告成功）",
                };
              }
              const savedTask = back.find((t) => t.id === nckWrite.taskId);
              if (!savedTask || !samePoints(nckPointsOf(savedTask), nckWrite.points)) {
                return {
                  ok: false,
                  message: "保存响应成功，但回读不一致：编辑未持久化（不报告成功，请重试或检查服务端）",
                };
              }
            }
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
      if (outcome.kind === "applied-restarted") {
        // P0-2：完成态必须明确写出实际落点，而不是让用户从「正在启动…」猜。
        setLastApply("restarted");
        setRunning(true);
        message.success("配置已保存，采集已启动");
        onChanged();
      } else if (outcome.kind === "applied-stopped") {
        setLastApply("stopped");
        // 状态区/下一次按钮必须反映“保存后确实停止了”，不能沿用保存前的运行态。
        setRunning(false);
        message.success(
          running && !restart ? kept("，Endpoint 保持停止") : kept("，当前停止"),
        );
        onChanged();
      } else if (outcome.kind === "stop-failed") {
        setLastApply(null);
        message.error(`停止失败，已中止应用，未修改点位：${outcome.message}`);
      } else if (outcome.kind === "apply-failed-stopped") {
        setLastApply(null);
        setRunning(false);
        message.error(`应用失败，Endpoint 当前已停止：${outcome.message}`);
        onChanged();
      } else {
        setLastApply("restart-failed");
        setRunning(false);
        message.warning(`配置已保存，但采集启动失败：${outcome.message}`);
        onChanged();
      }
    } finally {
      setSaving(false);
    }
  };

  // P0-2：完成态文案只描述**已经发生**的事；未保存时只描述“保存后会怎样”，
  // 且与按钮/复选框的实际分支一一对应（不写死 Stop → PUT → Start）。
  const afterSaveText =
    lastApply === "restarted"
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
  const executionNote = !running
    ? `保存将直接写入配置（PUT /tasks/${endpointId}），不会启动采集；Endpoint 保持停止。`
    : restart
      ? `保存将执行：停止 → 写入配置（PUT /tasks/${endpointId}）→ 启动。`
      : `保存将执行：停止 → 写入配置（PUT /tasks/${endpointId}），保存后保持停止。`;

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
        <InputNumber min={10} max={60000} step={10} value={intervalMs} onChange={(v) => setIntervalMs(v ?? 1000)} addonAfter="ms" style={{ width: 180 }} />
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
          <b data-testid="acquisition-status-state">{running ? "运行中" : "已停止"}</b>
        </span>
        <span>
          当前采集周期：
          <b data-testid="acquisition-status-interval">{intervalMs} ms</b>
        </span>
        <span>
          当前已配置点位：
          <b data-testid="acquisition-status-points">{nck ? nckPoints.length : sels.length} 个</b>
        </span>
        <span>
          保存后状态：
          <b data-testid="acquisition-status-after">{afterSaveText}</b>
        </span>
      </div>
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
              return (
                <div
                  key={`${point.point_key}-${idx}`}
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
          <Button size="small" onClick={() => setSels([])} style={{ marginLeft: 8 }}>清空</Button>
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
          {sels.map((s, idx) => (
            <div key={idx} style={{ display: "flex", gap: 8, alignItems: "center", padding: 6, border: "1px solid #e0e0e0", borderRadius: 0 }}>
              <span style={{ flex: 1, fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11 }}>{s.resource_id} → {s.outputs.map((o) => o.point_key).join(", ")} <span style={{ color: "#525252" }}>{JSON.stringify(s.parameters)}</span></span>
              <Button size="small" danger onClick={() => setSels((p) => p.filter((_, i) => i !== idx))}>移除</Button>
            </div>
          ))}
        </div>
      )}
      <div>
        {/* P0-2：已停止时保存只写配置（onApply(false)），按钮不得写「保存并启动」；
            运行中的恢复运行由 restart 勾选决定，结果由 lastApply 明确回显。 */}
        <ApplyWithRestart
          running={running}
          applying={saving}
          canApply={
            isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId) &&
            (nck
              ? !!definitions &&
                !!endpointFamily &&
                !editBlock &&
                nckPoints.length > 0 &&
                admissionIssues.length === 0 &&
                verificationReady &&
                unverified.length === 0
              : sels.length > 0 && !!desc)
          }
          applyLabel="保存配置"
          stoppedLabel="保存配置"
          restart={restart}
          onRestartChange={setRestart}
          result={lastApply}
          onApply={save}
        />
      </div>
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
