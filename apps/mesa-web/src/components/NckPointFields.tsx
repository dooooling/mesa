// 语义点位表单：**两个互斥入口**——默认「联动选择」，可选「高级原生配置」。
//
// 联动选择（普通用户，默认）：
//   ① 搜索或树形选择变量（`CatalogCategoryTree`，分类只读后端 `category`）
//   → ② 按变量需要的参数逐级选择**实际对象**（`NckObjectCascade`：展开下拉时才探测）
//   → ③ 参数由驱动返回的选择载荷**自动填入**（本表单没有任何 Target/Arguments 输入框）
//   → ④ 服务端 Preview 确认身份（唯一权威，前端不产生 point_key）
//   → ⑤ 加入待保存列表（只进草稿，不发写请求）。
// 高级原生配置（高级用户）：`NckNativeFields`，原样保留，不受联动选择影响。
//
// 为什么**彻底移除**手填 Target/Arguments：实例号（channel/axis/spindle/…）属于
// 驱动侧事实。前端一旦提供输入框，用户就能填出"格式合法但指向别的对象"的坐标，
// 而服务端 Preview 只校验坐标合法性，拦不住这类错误。参数因此只有一条产生路径：
// 探测 → 选对象 → 载荷翻译。拿不到所需参数时**明确提示并阻断**，不回退手填、
// 不填默认值（见 `objectCascade.evaluateCascadeReadiness`）。
//
// 分类只是**导航规则**：`category` 由驱动侧临时规则给出（`category_basis =
// navigation_rule`），本组件绝不据此推断可采集性，也不按前缀自行归类；
// 后端明确 `unclassified` 的条目按 fail-closed 拒绝提交（见 catalogTree.evaluateCategoryGate）。
//
// 三条不可妥协的规则：
// 1. **point_key 唯一来源是服务端 `identity-preview`**：Browse 的
//    `preview_point_key` 只作展示，前端模板替换不得作为可提交来源。
// 2. **输入一变，旧 Key 立即失效**：失效是渲染期判定（指纹不一致就不展示
//    Key、按钮直接 disabled），不等 effect / 不等响应。`AbortController` 只
//    用于省流量，不是竞态防线。
// 3. **迟到响应整份丢弃**：请求序号 + 输入指纹都匹配才采纳；被丢弃的响应
//    不得在任何文案/状态里留下痕迹。
// 4. **可用性门禁 fail-closed**：`availability`（sidecar 静态元数据）为
//    `hold` / `reference_only` / 缺失 / 取值非法时禁止添加，并把原因**可见**地
//    摆出来；`candidate` 允许走流程但添加处必须标注"未经真机读取验证"。
//    静态可用性（元数据）与动态对象（本机探测）**分开显示**：`ready` 不代表
//    当前机床存在该对象。判定全部收敛在 `definitionMeta.evaluateAvailabilityGate`。
// 5. **元数据分层**：单位、可用性状态、hold 原因、错误/阻断提示留在主视觉；
//    手册位置、分类依据、`availability.basis`、`index_rule`、静态/动态说明等
//    收进默认折叠的「技术详情」，不占主视觉。
import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Collapse, Input, Space, Tag, Tooltip } from "antd";
import type { SchemaDescriptor } from "../types";
import type { NckPointSelection } from "../deviceModel";
import {
  collectableDefinitions,
  evaluateNckSelectionAdd,
} from "../nckCollectability";
import { classifyCatalogDefinition, evaluateCategoryGate } from "../catalogTree";
import {
  AVAILABILITY_TAG_COLOR,
  STATIC_AVAILABILITY_NOTE,
  describeObjectDynamicState,
  evaluateAvailabilityGate,
  objectDynamicState,
  readDefinitionMeta,
} from "../definitionMeta";
import { CatalogCategoryTree } from "./CatalogCategoryTree";
import { NckObjectCascade } from "./NckObjectCascade";
import {
  objectProbeGate,
  resolveObjectNeeds,
  type ObjectFill,
  type ObjectProbeGate,
} from "../objectProbe";
import type { CascadeKey } from "../objectCascade";
import { cascadeKeysOf, isKeyResolved } from "../objectCascade";
import {
  isNckRequestError,
  previewNckPoint,
  type NckIdentityPreviewResponse,
} from "../nckCatalog";
import {
  buildNckFormModel,
  buildNckPreviewParameters,
  checkNckFamily,
  emptyNckDraft,
  nckDefinitionLabel,
  nckDraftFromParameters,
  nckPreviewFingerprint,
  shouldAcceptNckPreview,
  validateNckForm,
  validateNckPreviewResponse,
  type NckDefinitionLike,
  type NckFormDraft,
  type NckPreviewParameters,
} from "../nckPointModel";
import { NckNativeFields } from "./NckNativeFields";

/** 预览函数签名（可注入：单测用 deferred promise 精确控制返回顺序）。 */
export type NckPreviewFn = (
  request: {
    driverId: string;
    family: string;
    kind: string;
    parameters: Record<string, unknown>;
  },
  signal?: AbortSignal,
) => Promise<NckIdentityPreviewResponse>;

const defaultPreview: NckPreviewFn = (request, signal) =>
  previewNckPoint(request, fetch, signal);

type PreviewState =
  | { kind: "idle" }
  | { kind: "pending"; seq: number; fingerprint: string }
  | {
      kind: "ok";
      seq: number;
      fingerprint: string;
      response: NckIdentityPreviewResponse;
      pointKey: string;
    }
  | { kind: "error"; seq: number; fingerprint: string; code: string; message: string };

export interface NckPointFieldsProps {
  driverId: string;
  /** Endpoint 有效连接配置的 family（权威来源；顶层目录 family 不是）。 */
  endpointFamily?: string | null;
  /** 目录顶层 family（回显语义；null 不是拒绝理由）。 */
  catalogFamily?: string | null;
  /** 可采集定义列表（父层已按 scope 过滤；本组件仍会复核 scope）。 */
  definitions: NckDefinitionLike[];
  /** Endpoint 内已被占用的 point_key（跨任务、跨 kind）。 */
  takenPointKeys: string[];
  /** 编辑已有语义点位时的初始值（仍必须重新走服务端预览）。 */
  initialPoint?: NckPointSelection | null;
  /**
   * NCK Descriptor 的 `variable` 资源 Schema（12 原生字段）。
   * 提供时才显示「高级原生配置」页；缺失即不提供 native 入口（fail-closed）。
   */
  nativeSchema?: SchemaDescriptor | null;
  /**
   * 探测"实际对象"用的 Endpoint（缺省 ⇒ 探测入口不可用，且**不发任何请求**）。
   *
   * 为什么必须显式传入而不是从上下文取：探测会临时拉起驱动进程读真机，
   * 未连接端点时发起只会得到误导性失败（评审硬要求）。
   */
  endpointId?: string | null;
  /**
   * 探测入口可用性（由父层按"端点是否已创建 + 连接配置是否可读"算好）。
   * 缺省按"不可用"处理，并给出原因（不静默放开）。
   */
  probeGate?: ObjectProbeGate;
  /**
   * Browse 语义叶带来的 `identity.keys`（驱动在 wire 上直接声明"还需要哪类实例"）。
   * 有它时对象需求以它为准，定义 `targets` 只作补足——两者都是数据，前端不推断。
   */
  objectIdentityKeys?: unknown;
  submitLabel?: string;
  onCancel?: () => void;
  onSubmit: (point: NckPointSelection) => void;
  preview?: NckPreviewFn;
  /**
   * 抽屉内「有未确认修改」的上报（关闭抽屉前提醒一次，§4.1）。
   * 只在表单自身知道的信息（选了变量、改了参数、切了模式）上派生，不引入新判据。
   */
  onDirtyChange?: (dirty: boolean) => void;
  /**
   * 外部要求定位的字段 key（服务端 `issues[].path` 的字段级定位，§5.2-5）。
   * 原生页交给 `NckNativeFields` 的展开+定位机制；语义页目前没有可锚定的同源字段，
   * 因此不假装高亮（宁可只定位到行，也不指错字段）。
   */
  focusField?: string | null;
}

export function NckPointFields({
  driverId,
  endpointFamily,
  catalogFamily,
  definitions,
  takenPointKeys,
  initialPoint,
  nativeSchema,
  endpointId,
  probeGate,
  objectIdentityKeys,
  submitLabel,
  onCancel,
  onSubmit,
  preview,
  onDirtyChange,
  focusField,
}: NckPointFieldsProps) {
  const previewFn = preview ?? defaultPreview;
  // native 编辑态：已有 native 点位必须直接在高级原生页打开（身份规则不同）。
  const [editorMode, setEditorMode] = useState<"semantic" | "native">(
    initialPoint?.kind === "nck_native" ? "native" : "semantic",
  );
  const initialVariableId =
    initialPoint && typeof initialPoint.parameters?.variable_id === "string"
      ? initialPoint.parameters.variable_id
      : "";
  const [selectedId, setSelectedId] = useState(initialVariableId);
  // P1-1：36 个变量只用一个搜索框过滤（不新建分类体系）。支持中文名称与 ID。
  const [query, setQuery] = useState("");
  const definition = useMemo(
    () => definitions.find((d) => d.variable_id === selectedId) ?? null,
    [definitions, selectedId],
  );
  // 即使父层传入全量目录，组件层也只**提供** collect 条目（A13 双执行）；
  // definition 查找仍用全量，以便身份复核能显示具体 scope 原因。
  const selectableDefinitions = useMemo(
    () => collectableDefinitions(definitions),
    [definitions],
  );
  const filteredDefinitions = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return selectableDefinitions;
    return selectableDefinitions.filter((item) => {
      const variableId = String(item.variable_id ?? "").toLowerCase();
      const label = nckDefinitionLabel(item).toLowerCase();
      const en =
        item.label && typeof item.label === "object"
          ? String(item.label.en ?? "").toLowerCase()
          : "";
      return variableId.includes(needle) || label.includes(needle) || en.includes(needle);
    });
  }, [selectableDefinitions, query]);
  const initialModel = useMemo(
    () =>
      buildNckFormModel(
        definitions.find((d) => d.variable_id === initialVariableId) ?? null,
      ),
    // 初始模型只在挂载时用于回填；后续切换变量由 handleSelect 重建草稿。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  const [draft, setDraft] = useState<NckFormDraft>(() =>
    initialPoint ? nckDraftFromParameters(initialModel, initialPoint.parameters) : emptyNckDraft(initialModel),
  );
  const [previewState, setPreviewState] = useState<PreviewState>({ kind: "idle" });
  // 上一次「按实际对象自动填参」的依据（可见、可核对；null = 本次没填过）。
  // 只用于展示：填进去的值仍要过定义校验与服务端 Preview，前端不产生身份。
  const [fillNotice, setFillNotice] = useState<{ basis: string; unresolved: string[] } | null>(
    null,
  );

  const model = useMemo(() => buildNckFormModel(definition), [definition]);
  // 需要逐级选择（由设备提供取值）的必填参数键；为空 ⇒ 该变量不需要对象实例。
  const cascadeKeys = useMemo(() => cascadeKeysOf(model), [model]);
  // 还有没填的必填键 ⇒ 此时"定义约束未通过"只是流程没走完，不重复报错。
  const cascadePending = useMemo(
    () => cascadeKeys.some((key) => !isKeyResolved(key, draft)),
    [cascadeKeys, draft],
  );
  const admission = useMemo(() => evaluateNckSelectionAdd(definition), [definition]);
  // 分类导航（只读后端字段）：未选中 / 缺 category / 明确 unclassified 都落"待分类"，
  // 其中**后端明确 unclassified** 的条目按 fail-closed 拒绝提交。
  const categoryItem = useMemo(
    () => (definition ? classifyCatalogDefinition(definition) : null),
    [definition],
  );
  const categoryGate = useMemo(() => evaluateCategoryGate(categoryItem), [categoryItem]);
  // sidecar 元数据（单位 / 手册位置 / 索引规则 / 可用性）：树上与表单读同一份结论。
  const meta = useMemo(() => readDefinitionMeta(definition), [definition]);
  // 可用性门禁（**fail-closed**）：`availability` 缺失或取值非法一律阻断，
  // 绝不"默认放行"——那是把"驱动没说"当成"驱动说可以"。
  const availabilityGate = useMemo(() => evaluateAvailabilityGate(meta.availability), [meta]);
  // 需要的对象类型：wire 的 identity.keys 优先，定义 targets/target_required 补足。
  const objectNeeds = useMemo(
    () => resolveObjectNeeds({ definition, identityKeys: objectIdentityKeys }),
    [definition, objectIdentityKeys],
  );
  // 静态可用性（sidecar）× 动态对象（本机探测）分开呈现：
  // `ready` 只说明元数据层面可用，**不代表**当前机床存在该对象。
  const dynamicState = objectDynamicState({
    needCount: objectNeeds.length,
    filledFromDevice: fillNotice !== null,
  });
  const resolvedProbeGate = useMemo<ObjectProbeGate>(
    () =>
      probeGate ??
      objectProbeGate({ endpointId: endpointId ?? "", endpointKnown: false, connectionKnown: false }),
    [probeGate, endpointId],
  );
  const familyCheck = useMemo(
    () =>
      definition
        ? checkNckFamily({
            endpointFamily,
            definitionFamily: definition.family,
            catalogFamily,
          })
        : ({ ok: false, code: "FAMILY_ENDPOINT_MISSING", message: "请先选择变量" } as const),
    [definition, endpointFamily, catalogFamily],
  );
  const validation = useMemo(
    () => validateNckForm(definition, model, draft),
    [definition, model, draft],
  );
  const parameters = useMemo<NckPreviewParameters | null>(() => {
    if (!definition || !model.ok || !validation.ok) return null;
    const variableId = String(definition.variable_id ?? "");
    // revision 是定义坐标的一部分：缺失即无法构造权威预览请求（不得猜 1）。
    if (!variableId || typeof definition.revision !== "number") return null;
    return buildNckPreviewParameters({
      variableId,
      revision: definition.revision,
      target: validation.target,
      arguments: validation.arguments,
    });
  }, [definition, model.ok, validation]);
  const previewFamily = familyCheck.ok ? familyCheck.family : "";
  const parametersFingerprint = parameters ? nckPreviewFingerprint(parameters) : "";
  // 渲染期判定用的完整指纹 = family + 输入指纹：family 变了同样必须让旧 Key 失效。
  const fingerprint = parametersFingerprint ? `${previewFamily}|${parametersFingerprint}` : "";
  const readyToPreview = !!definition && admission.ok && familyCheck.ok && !!parameters;

  // effect 内读取最新值（避免把对象放进依赖导致每次渲染都重发请求）
  const latest = useRef({ definition, parameters, familyCheck, previewFn, driverId });
  latest.current = { definition, parameters, familyCheck, previewFn, driverId };
  const latestSeq = useRef(0);
  const currentFingerprint = useRef("");
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    const seq = ++latestSeq.current;
    currentFingerprint.current = fingerprint;
    abortRef.current?.abort();
    abortRef.current = null;
    if (!readyToPreview || !parameters) {
      setPreviewState({ kind: "idle" });
      return;
    }
    const controller = new AbortController();
    abortRef.current = controller;
    setPreviewState({ kind: "pending", seq, fingerprint });
    let cancelled = false;
    const current = latest.current;
    if (!current.familyCheck.ok) return;
    void current
      .previewFn(
        {
          driverId: current.driverId,
          family: current.familyCheck.family,
          kind: "nck_variable",
          parameters: current.parameters as NckPreviewParameters as unknown as Record<
            string,
            unknown
          >,
        },
        controller.signal,
      )
      .then((response) => {
        if (cancelled) return;
        // 竞态双判定：序号与指纹都匹配才采纳；否则整份丢弃（不改 Key/不报成功）。
        if (
          !shouldAcceptNckPreview({
            responseSeq: seq,
            latestSeq: latestSeq.current,
            responseFingerprint: fingerprint,
            currentFingerprint: currentFingerprint.current,
          })
        ) {
          return;
        }
        const currentDef = latest.current.definition;
        const expected = latest.current.parameters;
        if (!currentDef || !expected) return;
        const check = validateNckPreviewResponse({
          definition: currentDef,
          expected,
          response,
        });
        if (!check.ok) {
          setPreviewState({
            kind: "error",
            seq,
            fingerprint,
            code: check.code,
            message: check.message,
          });
          return;
        }
        setPreviewState({ kind: "ok", seq, fingerprint, response, pointKey: check.pointKey });
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        if (
          !shouldAcceptNckPreview({
            responseSeq: seq,
            latestSeq: latestSeq.current,
            responseFingerprint: fingerprint,
            currentFingerprint: currentFingerprint.current,
          })
        ) {
          return;
        }
        const code = isNckRequestError(error) ? error.code : "PREVIEW_FAILED";
        const message = error instanceof Error ? error.message : String(error);
        setPreviewState({ kind: "error", seq, fingerprint, code, message });
      });
    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [fingerprint, readyToPreview, previewFamily, driverId]);

  // 渲染期判定：Key 只在与**当前输入指纹**一致时可见。输入一变，旧 Key 立刻消失。
  const currentKey =
    previewState.kind === "ok" && previewState.fingerprint === fingerprint
      ? previewState.pointKey
      : null;
  const keyTaken = !!currentKey && takenPointKeys.includes(currentKey);
  const canSubmit =
    !!definition &&
    categoryGate.ok &&
    admission.ok &&
    availabilityGate.ok &&
    familyCheck.ok &&
    model.ok &&
    validation.ok &&
    !!currentKey &&
    !keyTaken;
  const hasSelection = selectedId.trim() !== "";
  // P0-1：未选择变量 ≠ 定义异常。前者只提示「请选择可采集变量」，后者（已选
  // 但目录里找不到定义）仍必须报错并保持不可提交——真正的异常不得被吞掉。
  const definitionMissing = hasSelection && !definition;

  const handleSelect = (value: string) => {
    setSelectedId(value);
    const nextDefinition = definitions.find((d) => d.variable_id === value) ?? null;
    // 切换变量必须清空不兼容的 Target/Arguments（A30/A31），绝不残留旧值。
    setDraft(emptyNckDraft(buildNckFormModel(nextDefinition)));
    setPreviewState({ kind: "idle" });
    setFillNotice(null);
  };

  /**
   * 接受「实际对象」带来的填参结果。
   *
   * 只覆盖载荷里**确实给出**的键：其余字段保持用户已输入/留空状态，绝不补默认值
   * （缺项由定义校验拦住，再由服务端 Preview 复核）。填参后旧身份立即失效——
   * `setPreviewState(idle)` 让上一份 point_key 不再可提交。
   */
  const applyObjectFill = (fill: ObjectFill) => {
    setDraft((prev) => ({
      target: { ...prev.target, ...fill.target },
      arguments: { ...prev.arguments, ...fill.arguments },
    }));
    setPreviewState({ kind: "idle" });
    setFillNotice({ basis: fill.basis, unresolved: fill.unresolved });
  };

  /**
   * 重新选择某一级：清掉该键（含 `drive_object.entry` 子键）后旧身份立即失效。
   *
   * 只清这一个键：其它级的选择仍是用户已确认的事实，不能因为重选一级就全部丢弃。
   */
  const clearObjectKey = (key: CascadeKey) => {
    setDraft((prev) => {
      const target = { ...prev.target };
      const args = { ...prev.arguments };
      if (key.source === "target") {
        delete target[key.key];
        if (key.subKey) delete target[key.subKey];
        target[key.key] = "";
        if (key.subKey) target[key.subKey] = "";
      } else {
        args[key.key] = "";
      }
      return { target, arguments: args };
    });
    setPreviewState({ kind: "idle" });
    setFillNotice(null);
  };

  const submit = () => {
    // 不信 UI 布尔值：提交时用**实际输入**重新跑一遍全部判定。
    if (!definition) return;
    const gate = evaluateNckSelectionAdd(definition);
    // 分类守卫同为"提交时重跑"：待分类（明确 unclassified）不得作为可采集点位。
    const category = evaluateCategoryGate(classifyCatalogDefinition(definition));
    // 可用性门禁同样"提交时重跑"：不信任渲染期的按钮可用状态
    // （深链、旧缓存、被改写的组件状态都可能绕过 UI）。
    const availability = evaluateAvailabilityGate(readDefinitionMeta(definition).availability);
    const family = checkNckFamily({
      endpointFamily,
      definitionFamily: definition.family,
      catalogFamily,
    });
    const currentModel = buildNckFormModel(definition);
    const currentValidation = validateNckForm(definition, currentModel, draft);
    const revision = typeof definition.revision === "number" ? definition.revision : null;
    if (
      !category.ok ||
      !gate.ok ||
      !availability.ok ||
      !family.ok ||
      !currentModel.ok ||
      !currentValidation.ok ||
      revision === null ||
      previewState.kind !== "ok" ||
      previewState.fingerprint !==
        `${previewFamily}|${nckPreviewFingerprint(
          buildNckPreviewParameters({
            variableId: String(definition.variable_id ?? ""),
            revision,
            target: currentValidation.target,
            arguments: currentValidation.arguments,
          }),
        )}`
    ) {
      return;
    }
    onSubmit({
      kind: "nck_variable",
      point_key: previewState.pointKey,
      parameters: {
        variable_id: String(definition.variable_id ?? ""),
        definition_revision: revision,
        target: currentValidation.target,
        arguments: currentValidation.arguments,
      },
    });
  };

  // P0-1：未选择变量时 `validateNckForm(null, …)` 会产出 FORM_MODEL_INVALID，
  // 但那是“还没选”而不是“定义坏了”。只有真正选中定义后才展示输入校验问题。
  const issues = definition ? validation.issues : [];

  // 两个入口**互斥**：默认停在「联动选择」（普通用户），「高级原生配置」只有用户
  // 主动切换才渲染，且切换后联动表单整体不渲染（任何时刻只有一套表单在场）。
  const modeSwitch = nativeSchema ? (
    <div
      data-testid="nck-editor-mode"
      data-editor-mode={editorMode}
      data-native-collapsed={editorMode === "native" ? "false" : "true"}
      style={{
        display: "flex",
        gap: 8,
        alignItems: "center",
        flexWrap: "wrap",
        borderTop: "1px dashed #e0e0e0",
        paddingTop: 6,
      }}
    >
      <span style={{ fontSize: 11, color: "#8c8c8c" }}>配置入口：</span>
      <Button
        size="small"
        type={editorMode === "semantic" ? "primary" : "default"}
        onClick={() => setEditorMode("semantic")}
      >
        联动选择
      </Button>
      <span style={{ fontSize: 11, color: "#8c8c8c" }}>推荐日常使用</span>
      <Button
        size="small"
        type={editorMode === "native" ? "primary" : "default"}
        onClick={() => setEditorMode("native")}
      >
        高级原生配置
      </Button>
    </div>
  ) : null;

  // 抽屉「有未确认修改」的判定：只取表单自己知道的事实（选了变量 / 填了参数 /
  // 切到原生页），不引入新的业务判据。关闭抽屉前据此提醒一次。
  const hasDraftInput =
    hasSelection ||
    Object.keys(draft.target).length > 0 ||
    Object.keys(draft.arguments).length > 0 ||
    editorMode === "native";
  useEffect(() => {
    onDirtyChange?.(hasDraftInput);
  }, [hasDraftInput, onDirtyChange]);

  /**
   * 变量选择入口：搜索（跨分类）+ 分类树。
   *
   * 这里刻意**只有一条**选择路径（评审要求）：搜索框过滤树中的条目，用户在树上
   * 选中变量。历史上还并列过一个平铺的 `<select>`，它与树是同一批条目的两套入口，
   * 只会让用户面对重复选择面，已移除；`selectedId`/`handleSelect` 仍由树驱动，
   * 选中后的定义信息、分类标签、探测、预览与添加流程保持不变。
   */
  const picker = (
    <div style={{ display: "grid", gap: 6 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12 }}>变量</span>
        <Input
          data-testid="nck-variable-search"
          aria-label="搜索变量"
          placeholder="搜索变量名称或 ID（跨全部分类）"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          allowClear
          style={{ width: 260 }}
        />
        {query.trim() ? (
          <span data-testid="nck-variable-search-count" style={{ fontSize: 12, color: "#525252" }}>
            {filteredDefinitions.length
              ? `匹配 ${filteredDefinitions.length} 个变量`
              : "没有匹配的变量（可改用变量 ID 搜索）"}
          </span>
        ) : null}
      </div>

      {/* 分类树：分类**只读**后端 `category`；搜索跨全部五个分组。 */}
      <CatalogCategoryTree
        definitions={selectableDefinitions}
        query={query}
        selectedId={selectedId}
        onSelect={handleSelect}
      />

      {definition ? (
        <div
          data-testid="nck-definition-meta-row"
          style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}
        >
          <Space size={4} data-testid="nck-definition-tags">
            <Tag>{String(definition.scope ?? "scope 缺失")}</Tag>
            <Tag>{String(definition.semantics_status ?? "状态未提供")}</Tag>
            {typeof definition.revision === "number" ? <Tag>rev {definition.revision}</Tag> : null}
          </Space>
          {/*
            单位：`verified` 才显示真实取值，其余明确写"单位未核实"——
            空白会被读成"无单位"，假单位比空白更糟（见 definitionMeta.readUnitMeta）。
            单位留在主视觉：它直接影响"这个读数怎么理解"。
          */}
          <Tooltip title={meta.unit.hint}>
            <Tag data-testid="nck-unit" data-unit-status={meta.unit.status}>
              {meta.unit.label}
            </Tag>
          </Tooltip>
        </div>
      ) : null}
    </div>
  );

  /**
   * 「技术详情」：默认折叠，承载**排查用**而不是填写用的元数据。
   *
   * 为什么分层：手册位置、分类依据、`availability.basis`、`index_rule`、静态/动态
   * 说明都是"要查的时候才看"的信息；把它们和输入区混排会让主流程（选变量 → 选对象
   * → 添加）被十几行标签淹没。折叠**不影响判定**：门禁与预览用的仍是同一份数据，
   * 这里只是把它们挪到可展开的一层。
   */
  const techDetails = definition ? (
    <Collapse
      size="small"
      data-testid="nck-tech-details"
      items={[
        {
          key: "tech",
          label: <span style={{ fontSize: 12 }}>技术详情（手册位置 / 分类依据 / 索引规则 / 可用性依据）</span>,
          children: (
            <div
              data-testid="nck-tech-details-body"
              style={{ display: "grid", gap: 4, fontSize: 11, color: "#525252" }}
            >
              <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
                {/* 索引规则：逐字展示驱动声明，未核实如实标注（不推断索引方式）。 */}
                <Tooltip title={meta.indexRule.hint}>
                  <Tag data-testid="nck-index-rule" data-index-status={meta.indexRule.status ?? ""}>
                    索引：{meta.indexRule.label}
                  </Tag>
                </Tooltip>
                {/* 手册位置（页码/章节）：tooltip + data 属性可查。 */}
                {meta.manualLocation.label ? (
                  <Tooltip title={meta.manualLocation.hint}>
                    <Tag
                      data-testid="nck-manual-location"
                      data-page={meta.manualLocation.page ?? ""}
                      data-section={meta.manualLocation.section ?? ""}
                      data-document={meta.manualLocation.documentNumber ?? ""}
                      style={{ fontSize: 10 }}
                    >
                      手册：{meta.manualLocation.label}
                    </Tag>
                  </Tooltip>
                ) : null}
                {/* 分类与分类依据：一行说清"这是导航规则，不是手册事实"。 */}
                {categoryItem ? (
                  <>
                    <Tag data-testid="nck-category" data-category={categoryItem.bucket}>
                      {categoryItem.categoryKnown
                        ? `分类：${categoryItem.bucket}`
                        : `分类未知（${categoryItem.bucket}）`}
                    </Tag>
                    <Tooltip title={categoryItem.basis.hint}>
                      <Tag
                        data-testid="nck-category-basis"
                        data-basis={categoryItem.basis.raw ?? ""}
                        style={{ fontSize: 10 }}
                      >
                        分类依据：{categoryItem.basis.label}
                      </Tag>
                    </Tooltip>
                  </>
                ) : null}
              </div>
              {meta.availability.basis.length ? (
                <span data-testid="nck-availability-basis">
                  可用性依据：{meta.availability.basis.join(" / ")}
                </span>
              ) : null}
              <span data-testid="nck-availability-static-note">{STATIC_AVAILABILITY_NOTE}</span>
              {/*
                静态可用性 × 动态对象的**分离显示**：这一行只讲"本机对象确认到什么程度"，
                与主视觉的 `nck-availability`（元数据结论）各自独立、互不替代。
              */}
              <span data-testid="nck-object-dynamic-state" data-object-state={dynamicState}>
                本机对象（动态）：{describeObjectDynamicState(dynamicState)}
              </span>
              {/*
                契约缺口：目录未声明 `drive_object.entry` 的合法候选集。现在 entry
                只能来自驱动载荷（不再手填），因此这条只在"载荷缺失"时作为排查线索出现。
              */}
              {model.ok &&
              model.targetFields.some((field) => field.requiresEntry && !field.entryDomainDeclared) ? (
                <span data-testid="nck-entry-contract-gap">
                  契约缺口：目录只声明 Target 变体，未提供 entry 合法候选集；entry 只能取自驱动载荷
                  （{model.targetFields
                    .filter((field) => field.requiresEntry)
                    .flatMap((field) => field.entryExamples)
                    .join(" / ") || "无示例"}）。
                </span>
              ) : null}
            </div>
          ),
        },
      ]}
    />
  ) : null;

  if (editorMode === "native") {
    return (
      <div style={{ display: "grid", gap: 8 }}>
        {modeSwitch}
        {nativeSchema ? (
          <NckNativeFields
            key={`native-${initialPoint?.point_key ?? "new"}`}
            schema={nativeSchema}
            takenPointKeys={takenPointKeys}
            initialPoint={initialPoint?.kind === "nck_native" ? initialPoint : null}
            submitLabel={submitLabel ?? "加入原生点位"}
            onSubmit={onSubmit}
            focusFieldKey={focusField ?? null}
          />
        ) : (
          <Alert
            type="error"
            data-testid="nck-native-schema-missing"
            message="原生字段 Schema 不可用：高级原生配置已禁用"
            description="Descriptor 的 variable 资源缺失时无法渲染 12 原生字段，保持不可提交（fail-closed）。"
          />
        )}
      </div>
    );
  }

  // 还没选变量：只给**一个选择入口**——不出现空身份、禁用按钮与错误框的组合（§4.1-4）。
  if (!hasSelection) {
    return (
      <div style={{ display: "grid", gap: 8 }} data-testid="nck-point-fields-idle">
        {modeSwitch}
        {picker}
        <div data-testid="nck-select-hint" style={{ fontSize: 12, color: "#525252" }}>
          请选择可采集变量后逐级选择实际对象（参数由驱动载荷自动填入，无需手填）。
        </div>
      </div>
    );
  }

  return (
    <div style={{ display: "grid", gap: 8 }} data-testid="nck-point-fields">
      {modeSwitch}
      {picker}

      {definitionMissing ? (
        <Alert
          type="error"
          data-testid="nck-definition-missing"
          message="定义缺失：无法生成 Target/Arguments 表单"
          description={`已选择变量 "${selectedId}"，但当前目录中找不到它的定义（可能已被移除，或不在可采集范围内）。真正的定义异常仍会拦截保存；请重新选择变量，若整个目录加载失败请先恢复目录。`}
        />
      ) : null}

      {/*
        定义的「来源 / 身份模板 / 接口地址」不再常驻表单：按 §2.5 收进技术详情
        （点位行「⋯ → 技术详情」），表单里只保留与填写直接相关的信息（scope/rev 标签）。
      */}

      {definition && !admission.ok ? (
        <Alert
          type="error"
          data-testid="nck-scope-reject"
          message={`不可采集：${admission.code}`}
          description={admission.message}
        />
      ) : null}
      {definition && !familyCheck.ok ? (
        <Alert
          type="error"
          data-testid="nck-family-reject"
          message={`family 校验失败：${familyCheck.code}`}
          description={familyCheck.message}
        />
      ) : null}
      {definition && !model.ok ? (
        <Alert
          type="error"
          data-testid="nck-model-reject"
          message={`定义元数据不足：${model.code}`}
          description={model.message}
        />
      ) : null}
      {/*
        分类守卫（fail-closed）：后端**明确** unclassified 的条目无法确认归属，
        不得被当成可采集；category 缺失只标注"分类未知"，不静默升级为某一类。
      */}
      {definition && !categoryGate.ok ? (
        <Alert
          type="error"
          data-testid="nck-category-unknown"
          message={`待分类：${categoryGate.code}`}
          description={categoryGate.message}
        />
      ) : null}

      {/*
        可用性（sidecar `availability`）：**静态**元数据结论，与"本机是否有该对象"
        分开显示（后者在技术详情里）。四态都可见；`hold` 的原因必须直接读得到，
        不藏在 tooltip 里；`basis` 这类排查信息收进技术详情。
      */}
      {definition ? (
        <div
          data-testid="nck-availability"
          data-state={meta.availability.state}
          style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}
        >
          <Tag
            data-testid="nck-availability-tag"
            data-availability={meta.availability.state}
            color={AVAILABILITY_TAG_COLOR[meta.availability.state]}
            style={{ marginInlineEnd: 0 }}
          >
            {meta.availability.label}
          </Tag>
          {meta.availability.state === "hold" ? (
            <span
              data-testid="nck-availability-hold-reason"
              style={{ fontSize: 11, color: "#c62828" }}
            >
              暂缓原因：{meta.availability.reasonZh ?? "驱动未给出原因"}
            </span>
          ) : null}
          {meta.availability.state === "candidate" ? (
            <span
              data-testid="nck-availability-candidate-notice"
              style={{ fontSize: 11, color: "#8c6000" }}
            >
              未经真机读取验证（可配置，但不得当作已验证）
            </span>
          ) : null}
        </div>
      ) : null}

      {/*
        可用性门禁（fail-closed）：`hold` / `reference_only` / 缺失 / 非法取值
        一律禁止添加，并把原因**可见**地摆出来（不静默禁用按钮）。
      */}
      {definition && !availabilityGate.ok ? (
        <Alert
          type="error"
          showIcon
          data-testid="nck-availability-block"
          data-code={availabilityGate.code}
          message={`不可添加：${availabilityGate.code}`}
          description={
            <span>
              {availabilityGate.message}
              {availabilityGate.reasonZh ? (
                <>
                  <br />
                  驱动给出的原因（reason_zh）：{availabilityGate.reasonZh}
                </>
              ) : null}
              <br />
              {STATIC_AVAILABILITY_NOTE}。本判定来自定义元数据，不会因探测到本机对象而改变。
            </span>
          }
        />
      ) : null}

      {/*
        统一流程第 ②③步：**级联下拉**。只有定义声明了必填参数（targets /
        target_required / identity.keys / arguments）时才出现；下拉展开时才探测设备。
        参数由驱动载荷自动填入——这里**没有任何手填输入框**（见文件头"为什么彻底移除"）。
      */}
      {definition && model.ok && cascadeKeys.length ? (
        <NckObjectCascade
          endpointId={endpointId ?? ""}
          gate={resolvedProbeGate}
          definition={definition}
          model={model}
          needs={objectNeeds}
          draft={draft}
          nativeAvailable={!!nativeSchema}
          onFill={applyObjectFill}
          onClear={clearObjectKey}
        />
      ) : null}
      {definition && model.ok && !cascadeKeys.length ? (
        <div data-testid="nck-cascade-not-required" style={{ fontSize: 11, color: "#525252" }}>
          该变量不需要额外对象实例：身份完全由定义决定，无需选择实际对象。
        </div>
      ) : null}
      {fillNotice ? (
        <div data-testid="nck-fill-notice" style={{ fontSize: 11, color: "#525252" }}>
          参数已按所选实际对象填入：{fillNotice.basis}
          {fillNotice.unresolved.length
            ? ` · 仍需逐级选择：${fillNotice.unresolved.join(" / ")}（前端不填默认值）`
            : ""}
        </div>
      ) : null}

      {techDetails}

      {/*
        这里**不再有** Target / Arguments 输入区。
        实例号只能来自"选中实际对象"这一步（`NckObjectCascade` → 驱动载荷）；
        拿不到所需参数时由 `nck-cascade-missing` 明确提示并阻断添加，
        不回退成手填表单、不填默认值。
      */}
      {issues.length && !cascadePending ? (
        <Alert
          type="warning"
          data-testid="nck-form-issues"
          message={`输入未通过定义约束（${issues.length} 项）`}
          description={
            <ul style={{ margin: 0, paddingLeft: 18 }}>
              {issues.map((issue, idx) => (
                <li key={idx} style={{ fontSize: 11 }}>
                  {issue.code}
                  {issue.field ? ` · ${issue.field}` : ""} · {issue.message}
                </li>
              ))}
            </ul>
          }
        />
      ) : null}

      {/* 未选择变量时：提交按钮保持禁用、身份区只显示占位，但**不展示任何
          校验错误**（P0-1）；真正的定义异常仍由上方 definition-missing 拦截。 */}
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12 }}>point_key</span>
        {currentKey ? (
          <code
            data-testid="nck-point-key"
            style={{ fontFamily: "'IBM Plex Mono',monospace", fontSize: 12 }}
          >
            {currentKey}
          </code>
        ) : (
          <span data-testid="nck-point-key-empty" style={{ fontSize: 12, color: "#8c8c8c" }}>
            （尚无服务端确认的身份）
          </span>
        )}
        {previewState.kind === "pending" ? (
          <span data-testid="nck-preview-status" style={{ fontSize: 12, color: "#8c8c8c" }}>
            正在向服务端校验身份…
          </span>
        ) : null}
        {currentKey ? (
          <span data-testid="nck-preview-ok" style={{ fontSize: 12, color: "#2e7d32" }}>
            已由服务端校验（只读，不可手改）
          </span>
        ) : null}
        {previewState.kind === "error" && previewState.fingerprint === fingerprint ? (
          <span data-testid="nck-preview-error" style={{ fontSize: 12, color: "#c62828" }}>
            预览失败：{previewState.code} · {previewState.message}
          </span>
        ) : null}
        {keyTaken ? (
          <span data-testid="nck-key-conflict" style={{ fontSize: 12, color: "#c62828" }}>
            该 point_key 已被 Endpoint 内其它点位占用：请调整 Target/Arguments（不得自动改名/加后缀）
          </span>
        ) : null}
      </div>

      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <Button
          type="primary"
          data-testid="nck-add-point"
          disabled={!canSubmit}
          onClick={submit}
        >
          {submitLabel ?? "加入点位"}
        </Button>
        {/*
          `candidate` 允许走流程，但**添加处必须标注**"未经真机读取验证"：
          允许 ≠ 已验证，标注放在按钮旁边，避免"加进去就当成已核实"。
        */}
        {availabilityGate.ok && availabilityGate.requiresUnverifiedNotice ? (
          <Tag
            data-testid="nck-add-unverified-notice"
            color="gold"
            style={{ marginInlineEnd: 0 }}
          >
            {availabilityGate.notice}
          </Tag>
        ) : null}
        {onCancel ? (
          <Button size="small" onClick={onCancel}>
            取消编辑
          </Button>
        ) : null}
      </div>
    </div>
  );
}
