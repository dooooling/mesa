/**
 * 连接配置交互的**唯一草稿与请求代际**（C2）。
 *
 * 设计边界（对应 V1.1 §8.3 与验收 A01–A06、A26、A27）：
 * - 三个入口（新建流程 / 新增连接 / 编辑连接）复用同一个 Hook，草稿只有一份，
 *   不存在「每个入口各写一套 state」导致的行为漂移；
 * - **驱动清单失败即不可用**：不再回退到内置驱动常量，清单拉取失败时暴露
 *   错误 + 重试入口，且 `canSubmit` 为 false（A02）；
 * - **切换驱动必须作废旧请求**：AbortController + 代际计数双保险，迟到响应
 *   既不能覆盖当前草稿，也不能覆盖当前结果（A01）；
 * - **新参数只由新 Schema 的默认值物化**：切驱动走 `withDriverChange`，
 *   绝不与旧驱动参数合并；
 * - **结果按 (driverId, revision) 判定是否仍然有效**：任一字段（含 TSAP 草稿的
 *   每个字符）变化都让旧的校验/探测结果立即失效（A03/A05）；
 * - **提交前一律用 `normalizeConnection` 现场归一化**：TSAP 从 raw 文本解析，
 *   因此「未失焦就点保存」提交的也是最新输入；非法值产生 issue 并阻止提交，
 *   绝不静默回退为上一次的合法值。
 *
 * 本 Hook 只做状态与请求编排，不做任何协议语义判断（分类交给
 * `connectionSetupModel.classifyProbeOutcome`）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { DriverDescriptor, FieldDescriptor } from "../types";
import {
  classifyProbeOutcome,
  initialSetupState,
  isResultCurrent,
  networkFailureOutcome,
  normalizeConnection,
  safeText,
  secretMarkerKeys,
  secretUnavailableOutcome,
  withDriverChange,
  withFieldChange,
  type ConnectionValues,
  type Issue,
  type NormalizeResult,
  type ProbeOutcome,
  type SetupState,
} from "./connectionSetupModel";

/** 驱动清单条目：只把服务端真实返回的 `launchable` 当作可选依据。 */
export interface DriverOption {
  id: string;
  name: string;
  launchable: boolean;
  /** 不可启动的原因（服务端 `reason`），必须原样展示给用户 */
  reason: string | null;
}

export type LoadStatus = "idle" | "loading" | "ready" | "error";

/** 参数校验结果（本地归一化 或 服务端 validate-connection 的结论）。 */
export interface ValidationState {
  kind: "ok" | "invalid" | "error";
  /** 字段级 issue；已全部转成字符串，可直接渲染 */
  issues: Issue[];
  /** 非字段级的说明（服务端错误体/网络失败） */
  detail: string;
}

/** `validate()` 的返回：`ok` 同时要求本地归一化无 issue 且服务端返回 200。 */
export interface ValidateOutcome {
  normalize: NormalizeResult;
  ok: boolean;
}

export interface UseConnectionSetupOptions {
  /** 面板不可见（弹窗关闭 / 非当前步骤）时为 false：不发任何请求 */
  active: boolean;
  /**
   * 驱动是否允许用户切换。edit 模式为 false（`driver_id` 创建后不可变），
   * 此时既不需要驱动清单，也不能用 Schema 默认值重建草稿（服务端快照才是权威）。
   */
  driverSelectable: boolean;
  /**
   * 会话标识：变化即整体复位（弹窗重新打开、切换 Endpoint、流程重来）。
   * 用字符串而不是对象，避免引用变化引发无意义的复位。
   */
  sessionKey: string;
  /** 初始驱动；add/create-draft 可给 null，等清单就绪后取第一个可启动驱动 */
  initialDriverId?: string | null;
  /**
   * edit 模式的服务端连接快照。**调用方必须保持引用稳定**（放进 state），
   * 否则每次渲染都会被当成一次新的快照而重置草稿。
   */
  initialValues?: ConnectionValues | null;
  /** 描述符加载结果上报（新建流程的数据步需要 resources 目录） */
  onDescriptorChange?: (descriptor: DriverDescriptor | null) => void;
}

export interface ConnectionSetupController {
  drivers: DriverOption[];
  driversStatus: LoadStatus;
  driversError: string;
  reloadDrivers: () => void;

  driverId: string | null;
  selectDriver: (id: string) => void;

  fields: FieldDescriptor[];
  descriptorStatus: LoadStatus;
  descriptorError: string;
  reloadDescriptor: () => void;

  raw: ConnectionValues;
  revision: number;
  setField: (key: string, value: unknown) => void;
  /** 整份替换 raw（DescriptorFields 的 onChange 语义：浅合并后的完整对象） */
  setRaw: (next: ConnectionValues) => void;

  validation: ValidationState | null;
  probe: ProbeOutcome | null;
  busy: "validate" | "probe" | null;

  /**
   * 仍处于 marker 态、未重新输入明文的 Secret 字段。
   * 非空时 `canTestConnection` 为 false：marker 不是密码，禁止发给
   * probe / validate-connection（C3 的 Endpoint-bound connection-check 尚未实现）。
   */
  secretMarkers: string[];
  /** 是否允许发起 校验/探测（Secret marker 存在时禁止；保存不受影响） */
  canTestConnection: boolean;

  /** 现场归一化（不改状态）：提交前的 canonical 快照来源 */
  snapshot: () => NormalizeResult;
  /** 只做本地归一化；有 issue 时写入结果区并返回 ok=false（不阻断式静默） */
  validateLocal: () => ValidateOutcome;
  /** 本地归一化 + 服务端 validate-connection */
  validate: () => Promise<ValidateOutcome>;
  /** 本地归一化 + 服务端 probe，按 HTTP 状态与响应体分类 */
  probeConnection: () => Promise<ProbeOutcome | null>;
}

/** 服务端 issue 形状不确定，统一降级为可渲染字符串，绝不把对象交给 React。 */
function toIssue(value: unknown, index: number): Issue {
  const obj = (value ?? {}) as Record<string, unknown>;
  return {
    path: safeText(obj.path) || `connection[${index}]`,
    code: safeText(obj.code) || "INVALID",
    message: safeText(obj.message) || safeText(value) || "参数非法",
  };
}

function errorText(error: unknown): string {
  if (error instanceof Error) return safeText(error.message);
  return safeText(error);
}

function isAbort(error: unknown): boolean {
  return (
    !!error &&
    typeof error === "object" &&
    (error as { name?: unknown }).name === "AbortError"
  );
}

/** 统一的 Secret-safe 说明文案，供校验结果与界面提示共用。 */
export function secretMarkerReason(key: string): string {
  return `字段 ${key} 仍是「已保存的凭据」标记（不是明文）；重新输入凭据后才能校验/测试。`;
}

export function useConnectionSetup(
  options: UseConnectionSetupOptions,
): ConnectionSetupController {
  const {
    active,
    driverSelectable,
    sessionKey,
    initialDriverId,
    initialValues,
    onDescriptorChange,
  } = options;

  const [drivers, setDrivers] = useState<DriverOption[]>([]);
  const [driversStatus, setDriversStatus] = useState<LoadStatus>("idle");
  const [driversError, setDriversError] = useState("");
  const [driversNonce, setDriversNonce] = useState(0);

  const [fields, setFields] = useState<FieldDescriptor[]>([]);
  const [descriptorStatus, setDescriptorStatus] = useState<LoadStatus>("idle");
  const [descriptorError, setDescriptorError] = useState("");
  const [descriptorNonce, setDescriptorNonce] = useState(0);

  const [state, setState] = useState<SetupState>(() => initialSetupState());
  const [validation, setValidation] = useState<ValidationState | null>(null);
  const [probe, setProbe] = useState<ProbeOutcome | null>(null);
  const [busy, setBusy] = useState<"validate" | "probe" | null>(null);

  // ref 是动作路径的**唯一真值来源**：异步回调必须在响应到达时读到最新草稿，
  // 而 React 的 state 在同一 tick 内不保证已提交，故所有变更先写 ref 再写 state。
  const stateRef = useRef<SetupState>(state);
  const fieldsRef = useRef<FieldDescriptor[]>(fields);
  fieldsRef.current = fields;

  const commitState = useCallback((next: SetupState) => {
    stateRef.current = next;
    setState(next);
  }, []);

  const updateState = useCallback(
    (fn: (current: SetupState) => SetupState) => {
      commitState(fn(stateRef.current));
    },
    [commitState],
  );

  // 每个请求通道各自持有 AbortController：新请求/会话复位即中止旧请求。
  const catalogAbort = useRef<AbortController | null>(null);
  const descriptorAbort = useRef<AbortController | null>(null);
  const actionAbort = useRef<AbortController | null>(null);
  /** 描述符请求代际：即便 abort 未能阻止 then 回调，也据代际丢弃迟到响应 */
  const descriptorGen = useRef(0);

  // onDescriptorChange 放进 ref：调用方常常传内联箭头函数，不能进 effect 依赖，
  // 否则每次渲染都会重发描述符请求。
  const onDescriptorRef = useRef(options.onDescriptorChange);
  onDescriptorRef.current = options.onDescriptorChange;

  // -------------------------------------------------------------------------
  // 会话复位：切换会话必须把草稿、结果、在途请求全部作废
  // -------------------------------------------------------------------------
  useEffect(() => {
    catalogAbort.current?.abort();
    descriptorAbort.current?.abort();
    actionAbort.current?.abort();
    descriptorGen.current += 1;
    setValidation(null);
    setProbe(null);
    setBusy(null);
    setFields([]);
    setDescriptorStatus("idle");
    setDescriptorError("");
    const base = initialSetupState();
    commitState({
      ...base,
      driverId: initialDriverId ?? null,
      draft: initialValues
        ? { raw: { ...initialValues }, revision: base.draft.revision }
        : base.draft,
    });
    // initialValues / initialDriverId 只在会话建立时取初值；后续变化由各自
    // 的 effect 处理，故此处刻意不进入依赖数组。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionKey, commitState]);

  // edit 模式的服务端快照到达（引用变化即视为新快照）
  useEffect(() => {
    if (!initialValues) return;
    setValidation(null);
    setProbe(null);
    updateState((current) => ({
      ...current,
      draft: { raw: { ...initialValues }, revision: current.draft.revision + 1 },
      lastValidatedRevision: null,
      probeRevision: null,
    }));
  }, [initialValues, updateState]);

  // -------------------------------------------------------------------------
  // 驱动清单：只认服务端返回且 launchable=true 的驱动
  // -------------------------------------------------------------------------
  useEffect(() => {
    if (!active || !driverSelectable) return;
    const ac = new AbortController();
    catalogAbort.current?.abort();
    catalogAbort.current = ac;
    setDriversStatus("loading");
    setDriversError("");
    api
      .listDrivers(ac.signal)
      .then((json) => {
        if (ac.signal.aborted) return;
        const list = (json as { drivers?: Array<Record<string, unknown>> }).drivers ?? [];
        const parsed: DriverOption[] = list.map((d) => ({
          id: safeText(d.id),
          name: safeText(d.name) || safeText(d.id),
          // 严格相等：字段缺失（老后端/错误响应）一律视为不可启动，
          // 绝不因为「没写 launchable」就当它能用。
          launchable: d.launchable === true,
          reason: d.reason === null || d.reason === undefined ? null : safeText(d.reason),
        }));
        setDrivers(parsed);
        setDriversStatus("ready");
      })
      .catch((error) => {
        if (ac.signal.aborted || isAbort(error)) return;
        // 关键：失败时清空清单而不是保留旧值/回退常量，避免把不可用驱动当可用。
        setDrivers([]);
        setDriversStatus("error");
        setDriversError(errorText(error) || "驱动清单加载失败");
      });
    return () => ac.abort();
  }, [active, driverSelectable, driversNonce]);

  // 清单就绪后确定默认驱动：当前选择失效（或未选）时取第一个可启动驱动
  useEffect(() => {
    if (!driverSelectable || driversStatus !== "ready") return;
    const usable = drivers.filter((d) => d.launchable);
    updateState((current) => {
      if (current.driverId && usable.some((d) => d.id === current.driverId)) return current;
      return { ...current, driverId: usable[0]?.id ?? null };
    });
  }, [drivers, driversStatus, driverSelectable, updateState]);

  // -------------------------------------------------------------------------
  // 描述符：AbortController + 代际；成功即按新 Schema 重建参数
  // -------------------------------------------------------------------------
  const driverId = state.driverId;
  useEffect(() => {
    if (!active || !driverId) {
      setFields([]);
      setDescriptorStatus("idle");
      return;
    }
    const ac = new AbortController();
    descriptorAbort.current?.abort();
    descriptorAbort.current = ac;
    const generation = descriptorGen.current + 1;
    descriptorGen.current = generation;
    setDescriptorStatus("loading");
    setDescriptorError("");
    api
      .getDescriptor(driverId, ac.signal)
      .then((json) => {
        // 双保险：abort 已置位 或 代际已前进 → 迟到响应必须整份丢弃
        if (ac.signal.aborted || generation !== descriptorGen.current) return;
        const desc = json as DriverDescriptor | null;
        const schemaFields = desc?.connection?.fields;
        if (!desc || !Array.isArray(schemaFields)) {
          throw new Error("驱动描述符缺少 connection schema");
        }
        setFields(schemaFields);
        setDescriptorStatus("ready");
        onDescriptorRef.current?.(desc);
        setValidation(null);
        setProbe(null);
        updateState((current) => {
          if (current.driverId !== driverId) return current;
          // add/create-draft：参数**只**由新 Schema 的默认值物化（不合并旧驱动参数）；
          // edit：服务端快照是权威，只补齐 driverId，绝不用默认值覆盖已存配置。
          if (!driverSelectable) {
            return { ...current, driverId, lastValidatedRevision: null, probeRevision: null };
          }
          return withDriverChange(current, driverId, schemaFields);
        });
      })
      .catch((error) => {
        if (ac.signal.aborted || isAbort(error) || generation !== descriptorGen.current) return;
        setFields([]);
        setDescriptorStatus("error");
        setDescriptorError(errorText(error) || "驱动描述符加载失败");
        onDescriptorRef.current?.(null);
      });
    return () => ac.abort();
  }, [active, driverId, descriptorNonce, driverSelectable, updateState]);

  // -------------------------------------------------------------------------
  // 动作：字段变更 / 选择驱动
  // -------------------------------------------------------------------------
  const clearResults = useCallback(() => {
    setValidation(null);
    setProbe(null);
  }, []);

  const selectDriver = useCallback(
    (next: string) => {
      if (next === stateRef.current.driverId) return;
      clearResults();
      descriptorAbort.current?.abort();
      actionAbort.current?.abort();
      // 先切 driverId（清空旧驱动的 raw，避免旧参数在新驱动下短暂可见）；
      // 新 Schema 到达后 withDriverChange 会按新默认值重新物化。
      commitState({
        draft: { raw: {}, revision: stateRef.current.draft.revision + 1 },
        driverId: next,
        lastValidatedRevision: null,
        probeRevision: null,
      });
    },
    [clearResults, commitState],
  );

  const setField = useCallback(
    (key: string, value: unknown) => {
      // 任一有效参数变化（含 TSAP 草稿的每个字符）：revision+1 且旧结果立即失效
      clearResults();
      commitState(withFieldChange(stateRef.current, key, value));
    },
    [clearResults, commitState],
  );

  const setRaw = useCallback(
    (next: ConnectionValues) => {
      clearResults();
      const current = stateRef.current;
      commitState({
        ...current,
        draft: { raw: { ...next }, revision: current.draft.revision + 1 },
        lastValidatedRevision: null,
        probeRevision: null,
      });
    },
    [clearResults, commitState],
  );

  const reloadDrivers = useCallback(() => setDriversNonce((n) => n + 1), []);
  const reloadDescriptor = useCallback(() => setDescriptorNonce((n) => n + 1), []);

  // Secret-safe 边界：只统计「仍是 marker」的 Secret 字段。用户重新输入明文后
  // 该 key 立即从列表消失，测试随之解禁（新建/新增路径本就没有 marker）。
  const secretMarkers = useMemo(
    () => secretMarkerKeys(fields, state.draft.raw),
    [fields, state.draft.raw],
  );
  const canTestConnection = secretMarkers.length === 0;

  const snapshot = useCallback(
    () => normalizeConnection(stateRef.current.draft.raw, fieldsRef.current),
    [],
  );

  const validateLocal = useCallback((): ValidateOutcome => {
    const result = snapshot();
    if (result.issues.length > 0) {
      setValidation({
        kind: "invalid",
        issues: result.issues,
        detail: "本地参数归一化未通过，已阻止提交",
      });
      return { normalize: result, ok: false };
    }
    return { normalize: result, ok: true };
  }, [snapshot]);

  const validate = useCallback(async (): Promise<ValidateOutcome> => {
    const current = stateRef.current;
    const targetDriver = current.driverId;
    const revision = current.draft.revision;
    const result = snapshot();
    if (result.issues.length > 0) {
      setValidation({
        kind: "invalid",
        issues: result.issues,
        detail: "本地参数归一化未通过，未发起请求",
      });
      return { normalize: result, ok: false };
    }
    if (!targetDriver) {
      setValidation({ kind: "error", issues: [], detail: "尚未选择驱动" });
      return { normalize: result, ok: false };
    }
    // Secret-safe：marker 不是密码。宁可明确拒绝，也不把 marker 发出去让后端
    // 把「凭据未回显」误判成连接失败（C3 的 Endpoint-bound check 才解决该场景）。
    const markers = secretMarkerKeys(fieldsRef.current, current.draft.raw);
    if (markers.length > 0) {
      setValidation({
        kind: "error",
        issues: markers.map((key) => ({
          path: `connection.${key}`,
          code: "SECRET_MARKER_NOT_TESTABLE",
          message: secretMarkerReason(key),
        })),
        detail: "已保存的凭据不会回显，本场景暂不支持校验/测试；未发出任何请求。",
      });
      return { normalize: result, ok: false };
    }
    actionAbort.current?.abort();
    const ac = new AbortController();
    actionAbort.current = ac;
    setBusy("validate");
    try {
      const r = await api.validateConnection(targetDriver, result.connection, ac.signal);
      if (ac.signal.aborted) return { normalize: result, ok: false };
      // 迟到响应：草稿已变（或驱动已换）则整份丢弃，不污染当前结果
      if (!isResultCurrent(stateRef.current, { driverId: targetDriver, revision })) {
        return { normalize: result, ok: false };
      }
      if (r.status === 200) {
        setValidation({ kind: "ok", issues: [], detail: "" });
        return { normalize: result, ok: true };
      }
      const body = (r.body ?? {}) as { issues?: unknown; error?: unknown };
      const issues = Array.isArray(body.issues)
        ? body.issues.map((v, i) => toIssue(v, i))
        : [];
      setValidation({
        kind: r.status === 400 ? "invalid" : "error",
        issues,
        detail: safeText(body.error) || `校验失败（HTTP ${r.status}）`,
      });
      return { normalize: result, ok: false };
    } catch (error) {
      if (ac.signal.aborted || isAbort(error)) return { normalize: result, ok: false };
      setValidation({ kind: "error", issues: [], detail: errorText(error) || "校验请求失败" });
      return { normalize: result, ok: false };
    } finally {
      if (!ac.signal.aborted) setBusy(null);
    }
  }, [snapshot]);

  const probeConnection = useCallback(async (): Promise<ProbeOutcome | null> => {
    const current = stateRef.current;
    const targetDriver = current.driverId;
    const revision = current.draft.revision;
    const result = snapshot();
    if (result.issues.length > 0) {
      // 参数非法时不发请求，但仍然产出结构化的 invalid 结果：
      // 复用 classifyProbeOutcome 而不是另造一套文案（A26：不把对象交给 React）。
      const outcome = classifyProbeOutcome(400, {
        error: { code: "CLIENT_VALIDATION_FAILED", message: "本地参数归一化未通过，未发起探测" },
        issues: result.issues.map((i) => `${i.path}: ${i.message}`),
      });
      setProbe(outcome);
      return outcome;
    }
    if (!targetDriver) return null;
    // Secret-safe：与 validate 同一道门。这里刻意**不发请求**并返回专用 outcome，
    // 既不伪装成「设备不可达」，也不伪装成「驱动不支持探测」。
    const markers = secretMarkerKeys(fieldsRef.current, current.draft.raw);
    if (markers.length > 0) {
      const outcome = secretUnavailableOutcome(
        `连接中的凭据仍是「已保存」标记（${markers.join("、")}）；已保存的凭据不会回显，无法作为密码发送。请重新输入凭据后再测试，或直接保存（凭据会原样保留）。`,
      );
      setProbe(outcome);
      return outcome;
    }
    actionAbort.current?.abort();
    const ac = new AbortController();
    actionAbort.current = ac;
    setBusy("probe");
    try {
      const r = await api.probe(targetDriver, result.connection, ac.signal);
      if (ac.signal.aborted) return null;
      if (!isResultCurrent(stateRef.current, { driverId: targetDriver, revision })) return null;
      const outcome = classifyProbeOutcome(r.status, r.body);
      setProbe(outcome);
      return outcome;
    } catch (error) {
      if (ac.signal.aborted || isAbort(error)) return null;
      // 浏览器/网络层失败与「设备不可达」是两件事，必须分开表达
      const outcome = networkFailureOutcome(errorText(error) || "Mesa API 不可达");
      setProbe(outcome);
      return outcome;
    } finally {
      if (!ac.signal.aborted) setBusy(null);
    }
  }, [snapshot]);

  return {
    drivers,
    driversStatus,
    driversError,
    reloadDrivers,
    driverId,
    selectDriver,
    fields,
    descriptorStatus,
    descriptorError,
    reloadDescriptor,
    raw: state.draft.raw,
    revision: state.draft.revision,
    setField,
    setRaw,
    validation,
    probe,
    busy,
    secretMarkers,
    canTestConnection,
    snapshot,
    validateLocal,
    validate,
    probeConnection,
  };
}
