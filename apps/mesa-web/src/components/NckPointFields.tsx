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
import type { NckPointSelection } from "../deviceModel";
import { evaluateNckSelectionAdd } from "../nckCollectability";
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
  submitLabel?: string;
  onCancel?: () => void;
  onSubmit: (point: NckPointSelection) => void;
  preview?: NckPreviewFn;
}

export function NckPointFields({
  driverId,
  endpointFamily,
  catalogFamily,
  definitions,
  takenPointKeys,
  initialPoint,
  submitLabel,
  onCancel,
  onSubmit,
  preview,
}: NckPointFieldsProps) {
  const previewFn = preview ?? defaultPreview;
  const initialVariableId =
    initialPoint && typeof initialPoint.parameters?.variable_id === "string"
      ? initialPoint.parameters.variable_id
      : "";
  const [selectedId, setSelectedId] = useState(initialVariableId);
  const definition = useMemo(
    () => definitions.find((d) => d.variable_id === selectedId) ?? null,
    [definitions, selectedId],
  );
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

  const issues = validation.issues;
  return (
    <div style={{ display: "grid", gap: 8 }} data-testid="nck-point-fields">
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span style={{ fontSize: 12 }}>变量</span>
        <select
          data-testid="nck-variable-select"
          value={selectedId}
          onChange={(e) => handleSelect(e.target.value)}
          style={{ minWidth: 320, padding: "4px 6px" }}
        >
          <option value="">请选择可采集变量</option>
          {definitions.map((d) => (
            <option key={String(d.variable_id)} value={String(d.variable_id)}>
              {nckDefinitionLabel(d) || String(d.variable_id)}（{String(d.variable_id)}）
            </option>
          ))}
        </select>
        {definition ? (
          <Space size={4}>
            <Tag>{String(definition.scope ?? "scope 缺失")}</Tag>
            <Tag>{String(definition.semantics_status ?? "状态未提供")}</Tag>
            {typeof definition.revision === "number" ? <Tag>rev {definition.revision}</Tag> : null}
          </Space>
        ) : null}
      </div>

      {definition ? (
        <div style={{ fontSize: 11, color: "#525252" }} data-testid="nck-definition-meta">
          来源：
          {Array.isArray(definition.source) && definition.source.length
            ? definition.source
                .map((s) =>
                  typeof s === "object" && s
                    ? String((s as { kind?: unknown }).kind ?? "unknown")
                    : "unknown",
                )
                .join(" / ")
            : "未提供"}
          {definition.identity_template ? ` · 身份模板：${definition.identity_template}` : ""}
        </div>
      ) : null}

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
