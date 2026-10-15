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
 * - **「用户清除」与「程序初始化得到的空值」必须区分**（P1-2）：草稿里显式记录
 *   用户改动过的 key（`Draft.touched`），只有「用户动过且当前为空」才算删除意图；
 *   程序物化/快照装载得到的空值一律不算，隐藏字段补齐时会用服务端快照兜底旧值。
 *   另：用户清空**必填**字段后再让条件隐藏它 → 本地校验失败（不再静默删除，
 *   Core 的 `validate_instance` 不看 `visible_if`，缺必填必定 400）。
 * - **两条检查通道，判据不同（C3-C）**：编辑态（`endpointId` 非 null）走 Endpoint-bound
 *   `connection-check`，规范 Secret marker 合法（服务端在内存中物化，明文不进响应与
 *   日志）；新建/新增走 Driver-bound `validate-connection` / `probe`，marker 一律拦下。
 *   同时 `touched + 空 Secret` 一律显式转成 `{"clear_secret":true}`（§10.2），
 *   因此「测试」与「保存」两个出口的清除语义同源、不会分叉。
 *
 * 本 Hook 只做状态与请求编排，不做任何协议语义判断（分类交给
 * `connectionSetupModel.classifyProbeOutcome`）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { DriverDescriptor, FieldDescriptor } from "../types";
import {
  applySecretClears,
  classifyProbeOutcome,
  clearedHiddenRequiredIssues,
  initialSetupState,
  isResultCurrent,
  networkFailureOutcome,
  normalizeConnection,
  preserveHiddenFields,
  safeText,
  secretMarkerKeys,
  secretUnavailableOutcome,
  submitBlockReason,
  touchedKeys,
  unsupportedCheckMarkerKeys,
  withDriverChange,
  withFieldChange,
  type ConnectionValues,
  type Issue,
  type LoadStatus,
  type NormalizeResult,
  type ProbeOutcome,
  type SetupState,
  type SubmitBlockReason,
  type SubmitGateInput,
} from "./connectionSetupModel";

/** 异步资源加载状态（驱动清单 / 描述符共用，定义在模型层以便纯函数判定复用）。 */
export type { LoadStatus };

/** 驱动清单条目：只把服务端真实返回的 `launchable` 当作可选依据。 */
export interface DriverOption {
  id: string;
  name: string;
  launchable: boolean;
  /** 不可启动的原因（服务端 `reason`），必须原样展示给用户 */
  reason: string | null;
}

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
  /**
   * 编辑态专用（`mode="edit"`）：提交快照里补齐「当前 Descriptor 内、因
   * `visible_if` 不满足而未渲染」的字段原值。
   *
   * 为什么只在编辑态开：后端 `update_endpoint()` 整体替换 connection，缺失即真丢；
   * 而新建/新增路径的 raw 只由 Descriptor 默认值物化，隐藏字段**不应**被提交
   * （用户看不到它，无法对它负责）。详见 `preserveHiddenFields` 的边界说明。
   */
  preserveHiddenValues?: boolean;
  /**
   * C3-C：本次会话绑定的**已存在 Endpoint**（编辑态由 `ConnectionEditorModal`
   * 给出）。
   *
   * 非 null 时，`validate()` / `probeConnection()` 改走 Endpoint-bound 的
   * `POST /api/v1/endpoints/{id}/connection-check`：
   * - 起草载荷里的 `{secret_set:true}` / `{clear_secret:true}` 由服务端在内存中
   *   物化/清除，因此**不再**拦截 marker（`secretMarkerKeys` 那道门只针对
   *   Driver-bound 路径）；
   * - `driver_id` 由服务端按已保存记录取，前端连传都不传（传了会被 400）。
   *
   * 为 null 时保持既有 Driver-bound 行为：新建流程与新增连接**必须**不传该值
   * （那时 Endpoint 还不存在，且 marker 绝不能进 Driver-bound Probe）。
   */
  endpointId?: string | null;
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
   * 提交前置门的判定结果（`null` = 可提交）。
   *
   * **唯一真值来源**：按钮的 `disabled` 与提交函数首行守卫必须都读它，
   * 不得各写一套判定（否则会漂移出「禁用被绕过」的回归）。判定逻辑见
   * `connectionSetupModel.submitBlockReason`（纯函数，可单测）。
   */
  blockReason: SubmitBlockReason | null;

  /**
   * 仍处于 marker 态、未重新输入明文的 Secret 字段。
   *
   * 含义随检查通道而变（见 [`ConnectionSetupController.checkTarget`]）：
   * - Driver-bound：非空即 `canTestConnection=false`——marker 不是密码，禁止发给
   *   `probe` / `validate-connection`；
   * - Endpoint-bound（`endpointId` 非 null）：规范 marker **可以**发（服务端物化），
   *   因此它只用于界面提示「本次测试将使用已保存的凭据」。
   */
  secretMarkers: string[];
  /** 本次会话的检查通道：Endpoint-bound 还是 Driver-bound（界面据此换文案） */
  checkTarget: "endpoint" | "driver";
  /**
   * 是否允许发起 校验/探测。
   *
   * - Driver-bound：存在未重新输入的 Secret marker 时禁止（保存不受影响）；
   * - Endpoint-bound：规范 marker 合法，只有「非 Secret 字段上的 marker」或
   *   「Secret 字段上的非规范取值」才禁止（服务端必定 400，本地先拦）。
   */
  canTestConnection: boolean;

  /** 现场归一化（不改状态）：提交前的 canonical 快照来源 */
  snapshot: () => NormalizeResult;
  /**
   * **提交用** canonical 快照。前置门未过时返回 `null`，调用方必须据此中止，
   * 不得改用 `snapshot()` 绕过（那是给 probe/validate 用的，且不做隐藏字段补齐）。
   */
  submitSnapshot: () => NormalizeResult | null;
  /** 只做本地归一化；有 issue 时写入结果区并返回 ok=false（不阻断式静默） */
  validateLocal: () => ValidateOutcome;
  /**
   * 本地归一化 + 服务端参数校验。
   * 编辑态（`endpointId` 非 null）走 Endpoint-bound `connection-check(mode=validate)`，
   * 其余入口走 Driver-bound `POST /drivers/{id}/validate-connection`。
   */
  validate: () => Promise<ValidateOutcome>;
  /**
   * 本地归一化 + 服务端探测，按 HTTP 状态与响应体分类。
   * 编辑态走 Endpoint-bound `connection-check(mode=probe)`（可携带已保存凭据 marker），
   * 其余入口走 Driver-bound `POST /drivers/{id}/probe`（只接受明文）。
   */
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

/**
 * Endpoint-bound 检查下 marker **位置/形状**非法的统一说明（本地拦下，不发请求）。
 *
 * 与 [`secretMarkerReason`] 的区别：那条是「Driver-bound 不接受任何 marker」，
 * 这条是「Endpoint-bound 只接受 Descriptor 声明为 Secret 的字段上的规范 marker」。
 * 两者都是安全边界，不可合并成一句含糊的「参数非法」。
 */
export function invalidCheckMarkerReason(keys: readonly string[]): string {
  return `字段 ${keys.join("、")} 的凭据标记位置或形状非法：只有 Descriptor 声明的 Secret 字段可以携带 {secret_set:true} / {clear_secret:true}。已阻止校验/测试，未发出任何请求。`;
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
    preserveHiddenValues = false,
    endpointId = null,
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

  // C3-C：检查通道的归属 Endpoint 也必须走 ref（与 stateRef 同理）。迟到响应除了
  // 要比对 (driverId, revision)，还要比对 **Endpoint 身份**：同一驱动、同一 revision
  // 在不同 Endpoint 上完全可能出现（用户切 Endpoint 后草稿按新快照重新装载）。
  const endpointIdRef = useRef<string | null>(endpointId);
  endpointIdRef.current = endpointId;

  // 服务端快照（编辑态 initialValues）：隐藏字段提交补齐时的**旧值来源**。
  // 为什么要单独留一份引用：草稿 raw 可能被程序按新 Schema 重新物化（`withDriverChange`）
  // 而丢掉快照里的旧值；此时只有快照能证明「这个字段原本有值」，
  // 从而区分「用户清空」与「程序初始化得到的空值」（见 preserveHiddenFields）。
  const snapshotRef = useRef<ConnectionValues | null>(null);

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
        ? { raw: { ...initialValues }, revision: base.draft.revision, touched: [] }
        : base.draft,
    });
    // initialValues / initialDriverId 只在会话建立时取初值；后续变化由各自
    // 的 effect 处理，故此处刻意不进入依赖数组。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionKey, commitState]);

  // edit 模式的服务端快照到达（引用变化即视为新快照）
  useEffect(() => {
    snapshotRef.current = initialValues ? { ...initialValues } : null;
    if (!initialValues) return;
    setValidation(null);
    setProbe(null);
    updateState((current) => ({
      ...current,
      // 快照装载是**程序写入**，不得登记为用户改动（touched 清空）
      draft: {
        raw: { ...initialValues },
        revision: current.draft.revision + 1,
        touched: [],
      },
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
      // touched 一并清空：旧驱动的「用户改动」不得带到新驱动上当作清除依据。
      commitState({
        draft: { raw: {}, revision: stateRef.current.draft.revision + 1, touched: [] },
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
      // 整份替换（DescriptorFields 的 onChange 语义）：按值差异登记用户真正
      // 改动过的 key（含把字段清空），供「用户清除 vs 程序初始化为空」判定使用。
      const changed = touchedKeys(current.draft.raw, next);
      const touched =
        changed.length === 0
          ? current.draft.touched
          : Array.from(new Set([...current.draft.touched, ...changed]));
      commitState({
        ...current,
        draft: { raw: { ...next }, revision: current.draft.revision + 1, touched },
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

  // C3-C：检查通道。Endpoint 已存在（编辑态）→ 走 Endpoint-bound 检查，
  // 规范 marker 合法（服务端在内存中物化）；否则维持 Driver-bound 的「marker 即禁测」。
  const checkTarget: "endpoint" | "driver" = endpointId ? "endpoint" : "driver";

  /**
   * Endpoint-bound 路径下**真正**不能发的字段：非 Secret 字段上的 marker 形状，
   * 或 Secret 字段上的非规范取值（服务端一律 400）。空数组 = 可以发。
   */
  const endpointMarkerIssues = useMemo(
    () => (checkTarget === "endpoint" ? unsupportedCheckMarkerKeys(fields, state.draft.raw) : []),
    [checkTarget, fields, state.draft.raw],
  );

  const canTestConnection =
    checkTarget === "endpoint" ? endpointMarkerIssues.length === 0 : secretMarkers.length === 0;

  // -------------------------------------------------------------------------
  // 提交前置门（P0）：按钮 disabled 与提交函数守卫共用同一判定
  // -------------------------------------------------------------------------
  const usableDriverIds = useMemo(
    () => drivers.filter((d) => d.launchable).map((d) => d.id),
    [drivers],
  );
  // 动作路径必须读**最新**的可用驱动集合（与 stateRef 同理：异步回调里渲染期
  // 闭包可能已过期），故同时保留一份 ref。
  const usableDriverIdsRef = useRef<string[]>(usableDriverIds);
  usableDriverIdsRef.current = usableDriverIds;

  const blockReason = submitBlockReason({
    active,
    driverSelectable,
    driversStatus,
    usableDriverIds,
    driverId: state.driverId,
    descriptorStatus,
  });

  /**
   * 现场归一化 + 「必填被显式清除」的本地校验（P1-3）。
   *
   * 为什么把「清除必填」并入归一化：可见字段被清空时 `normalizeConnection` 已报
   * REQUIRED；但用户在**可见时**清空、随后条件变化让该字段隐藏时，归一化会因
   * 「不可见即不报必填」而放行，隐藏字段补齐又会把空值剔除 —— 结果是静默删除，
   * 而 Core 的 `validate_instance` 不看 visible_if，必定回 400。这里补上该缺口，
   * 让校验失败发生在本地、并明确指向字段。
   */
  const normalizeDraft = useCallback((): NormalizeResult => {
    const { raw, touched } = stateRef.current.draft;
    const currentFields = fieldsRef.current;
    const result = normalizeConnection(raw, currentFields);
    const cleared = clearedHiddenRequiredIssues(currentFields, raw, touched);
    // C3-C §10.2/§10.3：`touched + 空 Secret` 显式转成 `{"clear_secret":true}`。
    //
    // 放在这里（而不是各调用点）是刻意的：`snapshot()`（校验/测试载荷）与
    // `submitSnapshot()`（正式保存载荷）**都**从这里出发，因此「测试」与「保存」
    // 两个出口的清除语义不可能分叉——只修一个出口会让「测试说不需要密码」与
    // 「保存后旧密码还在」自相矛盾。
    const withClears = applySecretClears(result.connection, currentFields, raw, touched);
    if (cleared.length === 0 && withClears.clearedKeys.length === 0) return result;
    return {
      connection: withClears.connection,
      issues: [...result.issues, ...cleared],
    };
  }, []);

  const snapshot = useCallback(() => normalizeDraft(), [normalizeDraft]);

  /**
   * **检查用**载荷（校验 / 测试）：Endpoint-bound 的编辑态与保存载荷同源。
   *
   * 为什么编辑态要额外补齐 `visible_if` 隐藏字段：检查与保存如果对同一份草稿
   * 给出不同的连接内容，探测结论就不能代表「保存后能否连通」——隐藏字段（如
   * `native_pdu`）在探测里回落到驱动默认值，用户会看到与真实配置不符的结论。
   * 因此只在 Endpoint-bound（编辑既有连接）时套用与 `submitSnapshot` 相同的
   * `preserveHiddenFields`；Driver-bound（新建 / 新增）**不**改变既有载荷语义。
   *
   * `preserveHiddenFields` 的边界不变：只补「当前 Descriptor 内、因 `visible_if`
   * 不满足而未进入 canonical」的字段，可见却校验失败的字段仍由校验阻断，绝不用
   * 旧值掩盖。
   */
  const checkPayload = useCallback((): NormalizeResult => {
    const result = normalizeDraft();
    if (!endpointIdRef.current || !preserveHiddenValues) return result;
    const { raw, touched } = stateRef.current.draft;
    return {
      issues: result.issues,
      connection: preserveHiddenFields(result.connection, raw, fieldsRef.current, {
        touched,
        fallback: snapshotRef.current,
      }),
    };
  }, [normalizeDraft, preserveHiddenValues]);

  /**
   * 现场构造前置门输入。
   *
   * 动作路径必须读 **ref** 里的最新 driverId / 可用驱动集合，而不是渲染期闭包，
   * 否则「禁用被绕过」的调用会拿到过期判定。
   */
  const gateInput = useCallback(
    (): SubmitGateInput => ({
      active,
      driverSelectable,
      driversStatus,
      usableDriverIds: usableDriverIdsRef.current,
      driverId: stateRef.current.driverId,
      descriptorStatus,
    }),
    [active, driverSelectable, driversStatus, descriptorStatus],
  );

  /**
   * 提交用快照：先过前置门（未过直接返回 `null`，不发也不构造），再现场归一化；
   * 编辑态额外补齐 `visible_if` 隐藏字段的原值（`preserveHiddenFields`）。
   *
   * 门禁放在这里而不是只放在组件里，是为了让「提交函数本身」拥有守卫：即便
   * 按钮的 `disabled` 被回归移除、或将来新增程序化提交入口，也拿不到可提交的负载。
   *
   * 隐藏字段补齐的上下文（P1-2）：
   * - `touched`：用户显式改动过的 key → 空值即「明确清除」，删除该字段；
   * - `fallback`：服务端快照 → 未触碰的空值不算清除，用旧值兜底，绝不误删。
   */
  const submitSnapshot = useCallback((): NormalizeResult | null => {
    if (submitBlockReason(gateInput()) !== null) return null;
    const result = normalizeDraft();
    if (!preserveHiddenValues) return result;
    const { raw, touched } = stateRef.current.draft;
    return {
      issues: result.issues,
      connection: preserveHiddenFields(result.connection, raw, fieldsRef.current, {
        touched,
        fallback: snapshotRef.current,
      }),
    };
  }, [gateInput, preserveHiddenValues, normalizeDraft]);

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
    const targetEndpoint = endpointIdRef.current;
    const revision = current.draft.revision;
    const result = checkPayload();
    // ---------------------------------------------------------------------
    // P0 守卫：驱动清单 / 可用驱动 / 描述符任一不可用即**不发请求**。
    // 与提交路径共用同一判定（submitBlockReason）：按钮的 disabled 只是 UI 表达，
    // 一旦被回归移除，没有这道守卫就会把请求发到一个字段集合不可信的驱动上。
    // ---------------------------------------------------------------------
    if (submitBlockReason(gateInput()) !== null) {
      setValidation({
        kind: "error",
        issues: [],
        detail: "驱动清单或连接参数描述未就绪，未发起校验请求。",
      });
      return { normalize: result, ok: false };
    }
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
    // Secret-safe 门（两条通道的判据不同，见各自函数的文档）：
    // - Endpoint-bound：规范 marker 合法（服务端物化），只拦位置/形状非法的 marker；
    // - Driver-bound：任何 marker 都不能当密码发出去，本地明确拒绝，绝不伪装成
    //   「参数非法」或「设备不可达」。
    if (targetEndpoint) {
      const bad = unsupportedCheckMarkerKeys(fieldsRef.current, current.draft.raw);
      if (bad.length > 0) {
        setValidation({
          kind: "error",
          issues: bad.map((key) => ({
            path: `connection.${key}`,
            code: "INVALID_SECRET_MARKER",
            message: `字段 ${key} 的凭据标记位置或形状非法`,
          })),
          detail: invalidCheckMarkerReason(bad),
        });
        return { normalize: result, ok: false };
      }
    } else {
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
    }
    actionAbort.current?.abort();
    const ac = new AbortController();
    actionAbort.current = ac;
    setBusy("validate");
    try {
      const r = targetEndpoint
        ? await api.connectionCheck(targetEndpoint, "validate", result.connection, ac.signal)
        : await api.validateConnection(targetDriver, result.connection, ac.signal);
      if (ac.signal.aborted) return { normalize: result, ok: false };
      // 迟到响应：草稿已变、驱动已换**或归属 Endpoint 已变**则整份丢弃。
      if (endpointIdRef.current !== targetEndpoint) return { normalize: result, ok: false };
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
  }, [checkPayload, gateInput]);

  const probeConnection = useCallback(async (): Promise<ProbeOutcome | null> => {
    const current = stateRef.current;
    const targetDriver = current.driverId;
    const targetEndpoint = endpointIdRef.current;
    const revision = current.draft.revision;
    const result = checkPayload();
    // ---------------------------------------------------------------------
    // P0 守卫：与 validate / 提交同一判定。驱动清单或描述符不可用时**不发探测**：
    // 此时字段集合不可信，探出来的结论只会误导用户（按钮 disabled 之外的防线）。
    // ---------------------------------------------------------------------
    if (submitBlockReason(gateInput()) !== null) {
      const blocked = classifyProbeOutcome(503, {
        error: {
          code: "SETUP_NOT_READY",
          message: "驱动清单或连接参数描述未就绪，未发起探测",
        },
      });
      setProbe(blocked);
      return blocked;
    }
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
    // Secret-safe 门：与 validate 同一判据（Endpoint-bound 允许规范 marker）。
    // 这里刻意**不发请求**并返回专用 outcome：既不伪装成「设备不可达」，
    // 也不伪装成「驱动不支持探测」。
    if (targetEndpoint) {
      const bad = unsupportedCheckMarkerKeys(fieldsRef.current, current.draft.raw);
      if (bad.length > 0) {
        // 复用统一分类（400 → invalid），不另造一套文案（A26：不把对象交给 React）
        const outcome = classifyProbeOutcome(400, {
          error: { code: "INVALID_SECRET_MARKER", message: invalidCheckMarkerReason(bad) },
        });
        setProbe(outcome);
        return outcome;
      }
    } else {
      const markers = secretMarkerKeys(fieldsRef.current, current.draft.raw);
      if (markers.length > 0) {
        const outcome = secretUnavailableOutcome(
          `连接中的凭据仍是「已保存」标记（${markers.join("、")}）；已保存的凭据不会回显，无法作为密码发送。请重新输入凭据后再测试，或直接保存（凭据会原样保留）。`,
        );
        setProbe(outcome);
        return outcome;
      }
    }
    actionAbort.current?.abort();
    const ac = new AbortController();
    actionAbort.current = ac;
    setBusy("probe");
    try {
      const r = targetEndpoint
        ? await api.connectionCheck(targetEndpoint, "probe", result.connection, ac.signal)
        : await api.probe(targetDriver, result.connection, ac.signal);
      if (ac.signal.aborted) return null;
      if (endpointIdRef.current !== targetEndpoint) return null;
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
  }, [checkPayload, gateInput]);

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
    blockReason,
    secretMarkers,
    checkTarget,
    canTestConnection,
    snapshot,
    submitSnapshot,
    validateLocal,
    validate,
    probeConnection,
  };
}
