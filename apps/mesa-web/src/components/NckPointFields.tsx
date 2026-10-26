// PR-C 第二批 B：NCK 语义点位表单（Target / Arguments 动态生成 + 权威预览状态机）。
//
// 这个组件只服务「语义点位（nck_variable）」；高级原生（nck_native）仍走既有
// Descriptor 12 字段入口，不在本批改语义。
//
// 三条不可妥协的规则：
// 1. **point_key 唯一来源是服务端 `identity-preview`**：Browse 的
//    `preview_point_key` 只作展示，前端模板替换不得作为可提交来源。
// 2. **输入一变，旧 Key 立即失效**：失效是渲染期判定（指纹不一致就不展示
//    Key、按钮直接 disabled），不等 effect / 不等响应。`AbortController` 只
//    用于省流量，不是竞态防线。
// 3. **迟到响应整份丢弃**：请求序号 + 输入指纹都匹配才采纳；被丢弃的响应
//    不得在任何文案/状态里留下痕迹。
import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Input, Space, Tag } from "antd";
import type { SchemaDescriptor } from "../types";
import type { NckPointSelection } from "../deviceModel";
import {
  collectableDefinitions,
  evaluateNckSelectionAdd,
} from "../nckCollectability";
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
  submitLabel?: string;
  onCancel?: () => void;
  onSubmit: (point: NckPointSelection) => void;
  preview?: NckPreviewFn;
  /**
   * 抽屉内「有未确认修改」的上报（关闭抽屉前提醒一次，§4.1）。
   * 只在表单自身知道的信息（选了变量、改了参数、切了模式）上派生，不引入新判据。
   */
  onDirtyChange?: (dirty: boolean) => void;
}

export function NckPointFields({
  driverId,
  endpointFamily,
  catalogFamily,
  definitions,
  takenPointKeys,
  initialPoint,
  nativeSchema,
  submitLabel,
  onCancel,
  onSubmit,
  preview,
  onDirtyChange,
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
  // 已选变量必须始终留在下拉里：搜索词把它过滤掉时补回首项，避免
  // `<select value>` 找不到匹配项而视觉上变成“未选择”（状态其实还在）。
  const visibleDefinitions = useMemo(() => {
    if (!selectedId) return filteredDefinitions;
    if (filteredDefinitions.some((item) => String(item.variable_id) === selectedId)) {
      return filteredDefinitions;
    }
    const selected = selectableDefinitions.find(
      (item) => String(item.variable_id) === selectedId,
    );
    return selected ? [selected, ...filteredDefinitions] : filteredDefinitions;
  }, [filteredDefinitions, selectedId, selectableDefinitions]);
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

  const model = useMemo(() => buildNckFormModel(definition), [definition]);
  const admission = useMemo(() => evaluateNckSelectionAdd(definition), [definition]);
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
    admission.ok &&
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
  };

  const submit = () => {
    // 不信 UI 布尔值：提交时用**实际输入**重新跑一遍全部判定。
    if (!definition) return;
    const gate = evaluateNckSelectionAdd(definition);
    const family = checkNckFamily({
      endpointFamily,
      definitionFamily: definition.family,
      catalogFamily,
    });
    const currentModel = buildNckFormModel(definition);
    const currentValidation = validateNckForm(definition, currentModel, draft);
    const revision = typeof definition.revision === "number" ? definition.revision : null;
    if (
      !gate.ok ||
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

  const modeSwitch = nativeSchema ? (
    <div style={{ display: "flex", gap: 8, alignItems: "center" }} data-testid="nck-editor-mode">
      <Button
        size="small"
        type={editorMode === "semantic" ? "primary" : "default"}
        onClick={() => setEditorMode("semantic")}
      >
        语义点位
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

  /** 变量选择入口（搜索即列表；选中才展示定义信息）。 */
  const picker = (
    <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      <span style={{ fontSize: 12 }}>变量</span>
      <Input
        data-testid="nck-variable-search"
        aria-label="搜索变量"
        placeholder="搜索变量名称或 ID"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        allowClear
        style={{ width: 260 }}
      />
      <select
        data-testid="nck-variable-select"
        value={selectedId}
        onChange={(e) => handleSelect(e.target.value)}
        style={{ minWidth: 320, padding: "4px 6px" }}
      >
        <option value="">请选择可采集变量</option>
        {visibleDefinitions.map((d) => (
          <option key={String(d.variable_id)} value={String(d.variable_id)}>
            {nckDefinitionLabel(d) || String(d.variable_id)}（{String(d.variable_id)}）
          </option>
        ))}
      </select>
      {query.trim() ? (
        <span data-testid="nck-variable-search-count" style={{ fontSize: 12, color: "#525252" }}>
          {filteredDefinitions.length
            ? `匹配 ${filteredDefinitions.length} 个变量`
            : "没有匹配的变量（可改用变量 ID 搜索）"}
        </span>
      ) : null}
      {definition ? (
        <Space size={4}>
          <Tag>{String(definition.scope ?? "scope 缺失")}</Tag>
          <Tag>{String(definition.semantics_status ?? "状态未提供")}</Tag>
          {typeof definition.revision === "number" ? <Tag>rev {definition.revision}</Tag> : null}
        </Space>
      ) : null}
    </div>
  );

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
          请选择可采集变量后填写参数。
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

      {definition && model.ok ? (
        <div style={{ display: "grid", gap: 6 }}>
          {model.targetFields.some((field) => field.requiresEntry && !field.entryDomainDeclared) ? (
            <Alert
              type="warning"
              data-testid="nck-entry-contract-gap"
              message="drive_object.entry 候选集未由目录声明（契约缺口）"
              description={
                <span>
                  目录的 targets / target_required 只声明 Target 变体，未提供 entry 的合法候选集；
                  下面展示的 Golden 示例仅作输入提示，不代表完整可选域。合法性由服务端
                  Preview 裁决（失败即不可提交）。已登记为待补充的最小契约缺口。
                </span>
              }
            />
          ) : null}
          <div style={{ fontSize: 12, color: "#525252" }}>Target（由定义 targets / target_required 动态生成）</div>
          {model.targetFields.map((field) => (
            <div key={field.kind} style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <span style={{ width: 160, fontSize: 12 }}>
                {field.kind}
                {field.required ? <span style={{ color: "#c62828" }}> *</span> : null}
              </span>
              <Input
                aria-label={`target-${field.kind}`}
                data-testid={`nck-target-${field.kind}`}
                value={draft.target[field.kind] ?? ""}
                onChange={(e) =>
                  setDraft((prev) => ({
                    ...prev,
                    target: { ...prev.target, [field.kind]: e.target.value },
                  }))
                }
                style={{ width: 160 }}
                placeholder="实例号"
              />
              {field.requiresEntry ? (
                <>
                  <span style={{ fontSize: 12 }}>entry</span>
                  {/*
                    这里刻意用自由输入 + datalist，而不是 select：
                    目录没有声明 entry 合法域，把 Golden 示例渲染成闭集会让用户
                    误以为「列表即全集」。输入值最终由服务端 Preview 校验。
                  */}
                  <Input
                    list={`nck-entry-examples-${field.kind}`}
                    data-testid={`nck-target-${field.kind}-entry`}
                    aria-label={`target-${field.kind}-entry`}
                    value={draft.target[`${field.kind}.entry`] ?? ""}
                    onChange={(e) =>
                      setDraft((prev) => ({
                        ...prev,
                        target: { ...prev.target, [`${field.kind}.entry`]: e.target.value },
                      }))
                    }
                    style={{ width: 220 }}
                    placeholder={
                      field.entryExamples.length
                        ? `入口名（示例：${field.entryExamples.join(" / ")}）`
                        : "入口名（目录未提供示例）"
                    }
                  />
                  <datalist id={`nck-entry-examples-${field.kind}`}>
                    {field.entryExamples.map((entry) => (
                      <option key={entry} value={entry} />
                    ))}
                  </datalist>
                </>
              ) : null}
            </div>
          ))}

          <div style={{ fontSize: 12, color: "#525252" }}>
            Arguments（由定义 arguments[] 动态生成）
          </div>
          {model.argumentFields.length === 0 ? (
            <div style={{ fontSize: 12, color: "#525252" }}>该变量没有 arguments</div>
          ) : null}
          {model.argumentFields.map((field) => (
            <div key={field.key} style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <span style={{ width: 160, fontSize: 12 }}>
                {field.label}（{field.key}）
                {field.required ? <span style={{ color: "#c62828" }}> *</span> : null}
              </span>
              {field.valueType === "enum" ? (
                <select
                  data-testid={`nck-argument-${field.key}`}
                  aria-label={`argument-${field.key}`}
                  value={draft.arguments[field.key] ?? ""}
                  onChange={(e) =>
                    setDraft((prev) => ({
                      ...prev,
                      arguments: { ...prev.arguments, [field.key]: e.target.value },
                    }))
                  }
                  style={{ padding: "4px 6px" }}
                >
                  <option value="">请选择</option>
                  {field.enumOptions.map((option) => (
                    <option key={option} value={option}>
                      {option}
                    </option>
                  ))}
                </select>
              ) : (
                <Input
                  aria-label={`argument-${field.key}`}
                  data-testid={`nck-argument-${field.key}`}
                  value={draft.arguments[field.key] ?? ""}
                  onChange={(e) =>
                    setDraft((prev) => ({
                      ...prev,
                      arguments: { ...prev.arguments, [field.key]: e.target.value },
                    }))
                  }
                  style={{ width: 200 }}
                  placeholder={
                    field.min !== undefined || field.max !== undefined
                      ? `整数 ${field.min ?? ""}..${field.max ?? ""}`
                      : "整数"
                  }
                />
              )}
              {field.optionsError ? (
                <span style={{ fontSize: 11, color: "#c62828" }}>{field.optionsError}</span>
              ) : null}
            </div>
          ))}
        </div>
      ) : null}

      {issues.length ? (
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

      <div style={{ display: "flex", gap: 8 }}>
        <Button
          type="primary"
          data-testid="nck-add-point"
          disabled={!canSubmit}
          onClick={submit}
        >
          {submitLabel ?? "加入点位"}
        </Button>
        {onCancel ? (
          <Button size="small" onClick={onCancel}>
            取消编辑
          </Button>
        ) : null}
      </div>
    </div>
  );
}
