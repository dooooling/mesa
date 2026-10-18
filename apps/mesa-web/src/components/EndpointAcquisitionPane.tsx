// Endpoint 采集配置窗格（Workspace「采集」tab；逻辑由 DeviceDetail 点位
// Modal 原样迁移）。保存路径**按 Endpoint 的 Driver 分流**（第二批 A）：
// - 非 NCK：编辑唯一 canonical（mesa.resources.v1）任务，其余任务原样保留；
// - NCK：编辑唯一 `nck.points.v1` 任务（`{points:[…]}`），其余任务原样保留。
// 两种 binding 的 `config` 结构不同，**不是**可互换的两种写法——混用会写出
// 驱动程序整体拒绝的配置（连原有正确任务一起启动失败），因此这里不按"名字"
// 判断，一律走 `endpointSavePlan` / `nckTaskBinding` 的 Driver 分流实现。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, InputNumber, Tag, message } from "antd";
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
  type NckVariableDefinitionLike,
} from "../nckCollectability";
import { nckPointsOf, parseTaskBinding, selectEditableTask } from "../nckTaskBinding";
import { planEndpointTaskSave, samePoints } from "../endpointSavePlan";
import { applySelectionAdd } from "../resourceSelectionModel";
import { ApplyWithRestart } from "./ApplyWithRestart";
import { ResourcePickerAntd } from "./ResourcePickerAntd";

/** 定义目录条目的最小形态（I-2 `VariableDefinition` 的子集 + 展示用 label）。 */
export interface NckDefinitionEntry extends NckVariableDefinitionLike {
  label?: string;
}

/**
 * NCK 定义目录（按 `variable_id` 索引）。
 *
 * 本批（A）**只消费不获取**：目录运输、分页、错误映射与 Preview 接线属第二批 B。
 * 缺失时 NCK 保存保持禁用（fail-closed）——没有 `scope` 就无法执行 P0-1 准入，
 * 绝不能退化成"目录不可用也能存"。
 *
 * TODO(PR-C 第二批 B)：由目录客户端（nckCatalog）加载后从这里注入，并补
 * `family`（取自 Endpoint 有效连接配置 `connection.family`，与条目 `definition.family`
 * 交叉校验）。在 B 完成前，生产环境的 NCK Endpoint 保存保持禁用——这是刻意的
 * fail-closed 选择，不是遗漏。
 */
export type NckDefinitionMap = Readonly<Record<string, NckDefinitionEntry>>;

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
  /** NCK 定义目录（第二批 B 注入；缺失即禁用 NCK 保存）。 */
  nckDefinitions?: NckDefinitionMap | null;
}) {
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [sels, setSels] = useState<ResourceSelection[]>([]);
  // NCK 待提交点位（来自快照 + 本批编辑；本批不自行推导 point_key）。
  const [nckPoints, setNckPoints] = useState<NckPointSelection[]>([]);
  const [existingTasks, setExistingTasks] = useState<AcquisitionTaskShape[]>([]);
  const [preservedTasks, setPreservedTasks] = useState<AcquisitionTaskShape[]>([]);
  // 现有任务无法安全编辑（多候选 / 结构非法）：保存必须禁用，绝不猜第一个。
  const [editBlock, setEditBlock] = useState<{ code: string; message: string } | null>(null);
  const [running, setRunning] = useState(false);
  const [intervalMs, setIntervalMs] = useState(1000);
  const [saving, setSaving] = useState(false);
  // 任务快照门（P0 fail-closed，与 DeviceDetail 点位 Modal 同门）：
  // 只有 ready + 归属当前 Endpoint 才允许 PUT；loading/error 一律禁用保存，
  // 绝不能把 [] 当作“服务端没有任务”去覆盖 task-a/b/c。
  const [tasksLoadState, setTasksLoadState] = useState<TaskSnapshotState>("idle");
  const [tasksLoadedEpId, setTasksLoadedEpId] = useState<string | null>(null);
  const [tasksLoadError, setTasksLoadError] = useState("");
  // 保存失败的后端 issues 原样展示（`{valid:false, issues:[...]}`）：
  // 不翻译、不重新判定；成功保存 / 重新发起保存时清掉旧值。
  const [saveIssues, setSaveIssues] = useState<ValidationIssue[]>([]);

  const nck = isNckDriver(driverId);
  const definitions = nckDefinitions ?? null;
  const definitionList = useMemo<NckDefinitionEntry[]>(
    () => (definitions ? Object.values(definitions) : []),
    [definitions],
  );
  // UI 执行点①（P0-1）：只有 `scope === "collect"` 的定义才进入可采集集合。
  // identity / 缺失 / 未知 scope 一律不出现（fail-closed，不做 `!== "identity"` 反写）。
  const collectableDefs = useMemo(
    () => collectableDefinitions(definitionList),
    [definitionList],
  );
  // UI 执行点②（P0-1 双执行）：对**将要提交的点位**再查一次准入，不信任列表过滤
  // 或选中态；提交模型（endpointSavePlan → validateNckTaskSubmission）还会再查一次。
  const admissionIssues = useMemo(() => {
    if (!nck) return [] as Array<{ pointKey: string; code: string; message: string }>;
    return nckPoints.flatMap((point) => {
      // 原生点位（高级模式）不适用目录 scope：身份由用户/固定规则给出。
      if (point.kind !== "nck_variable") return [];
      const variableId = point.parameters?.variable_id;
      const definition =
        typeof variableId === "string" && definitions ? definitions[variableId] : undefined;
      const gate = evaluateNckSelectionAdd(definition);
      return gate.ok
        ? []
        : [{ pointKey: point.point_key, code: gate.code, message: gate.message }];
    });
  }, [nck, nckPoints, definitions]);

  useEffect(() => {
    // 切 Endpoint 时先复位：旧快照不得污染新窗格（fail-closed 默认禁用保存）。
    setSels([]);
    setNckPoints([]);
    setEditBlock(null);
    setExistingTasks([]);
    setPreservedTasks([]);
    setIntervalMs(1000);
    setDesc(null);
    setTasksLoadState("loading");
    setTasksLoadedEpId(null);
    setTasksLoadError("");
    setSaveIssues([]);
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
        const live = ((j as { endpoints?: Array<{ id: string; runtime?: { state?: string }; state?: string }> }).endpoints ?? []).find(
          (e) => e.id === endpointId,
        );
        if (!cancelled) setRunning(isRunningState(live?.state ?? live?.runtime?.state));
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [endpointId, driverId]);

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
        message.success(kept("，正在启动…"));
        onChanged();
      } else if (outcome.kind === "applied-stopped") {
        message.success(running && !restart ? kept("，Endpoint 保持停止") : kept(""));
        onChanged();
      } else if (outcome.kind === "stop-failed") {
        message.error(`停止失败，已中止应用，未修改点位：${outcome.message}`);
      } else if (outcome.kind === "apply-failed-stopped") {
        message.error(`应用失败，Endpoint 当前已停止：${outcome.message}`);
        onChanged();
      } else {
        message.warning(`配置已保存，但恢复运行失败：${outcome.message}`);
        onChanged();
      }
    } finally {
      setSaving(false);
    }
  };

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
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <span style={{ fontSize: 12 }}>采集周期</span>
        <InputNumber min={10} max={60000} step={10} value={intervalMs} onChange={(v) => setIntervalMs(v ?? 1000)} addonAfter="ms" style={{ width: 180 }} />
        <span style={{ fontSize: 12, color: "#525252" }}>10ms–60s，20ms已通过50K/s压测</span>
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
        {/* TODO(PR-C 第二批 B)：目录注入（nckCatalog）+ 语义点位表单 + Preview 状态机。
            本批只做"可采集范围 + 已有点位"两件事，避免在无法确认 scope 时放行。 */}
        {!definitions ? (
          <Alert
            type="warning"
            message="定义目录不可用：NCK 保存已禁用（fail-closed）"
            description="没有定义目录就无法确认哪些变量可采集（scope 准入），因此保存保持禁用。目录加载与语义点位表单在后续版本接入；在可用前不会出现“保存成功但编辑没生效”。"
          />
        ) : (
          <div style={{ fontSize: 12, color: "#525252" }} data-testid="nck-collectable-summary">
            可采集变量 {collectableDefs.length} / {definitionList.length}（其余为身份探针或缺少
            scope，不可采集）
            <div style={{ marginTop: 6, display: "flex", flexWrap: "wrap", gap: 6 }}>
              {collectableDefs.map((definition) => (
                <Tag key={String(definition.variable_id)} data-testid="nck-collectable-def">
                  {definition.label ?? String(definition.variable_id)}
                </Tag>
              ))}
            </div>
            <div style={{ marginTop: 6 }}>
              当前版本不在此新增语义点位（选择变量 + Target/Arguments + 服务端预览的表单尚未
              提供）；列表只说明可采集范围，保存使用已有点位。
            </div>
          </div>
        )}
        {admissionIssues.length > 0 ? (
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
          <div style={{ display: "grid", gap: 6 }}>
            {nckPoints.map((point, idx) => (
              <div key={`${point.point_key}-${idx}`} style={{ display: "flex", gap: 8, alignItems: "center", padding: 6, border: "1px solid #e0e0e0", borderRadius: 0 }}>
                <span style={{ flex: 1, fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 11 }}>
                  {point.kind} · {point.point_key}{" "}
                  <span style={{ color: "#525252" }}>{JSON.stringify(point.parameters)}</span>
                </span>
                <Button
                  size="small"
                  danger
                  onClick={() => setNckPoints((prev) => prev.filter((_, i) => i !== idx))}
                >
                  移除
                </Button>
              </div>
            ))}
          </div>
        ) : null}
      </div>
      )}
      </>)}
      <div style={{ fontSize: 12, color: "#525252" }}>
        已选 {nck ? nckPoints.length : sels.length} 项 · {intervalMs}ms 轮询 · 保存将执行 Stop → PUT /tasks/{endpointId} → Start
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
        <ApplyWithRestart
          running={running}
          applying={saving}
          canApply={
            isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId) &&
            (nck
              ? !!definitions &&
                !editBlock &&
                nckPoints.length > 0 &&
                admissionIssues.length === 0
              : sels.length > 0 && !!desc)
          }
          applyLabel="保存并启动"
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
