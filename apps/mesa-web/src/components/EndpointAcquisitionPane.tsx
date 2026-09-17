// Endpoint 采集配置窗格（Workspace「采集」tab；逻辑由 DeviceDetail 点位
// Modal 原样迁移）。单任务编辑器：只编辑 canonical（mesa.resources.v1）
// 任务，其余任务原样保留（P0 数据安全）；保存 Stop → PUT → Start。
import { useEffect, useState } from "react";
import { Alert, Button, InputNumber, Tag, message } from "antd";
import { api } from "../api";
import type { DriverDescriptor, ValidationIssue } from "../types";
import {
  applyEndpointChange,
  isTaskSnapshotReady,
  mergeAcquisitionTasks,
  selectionsOf,
  splitAcquisitionTasks,
  isRunningState,
  type AcquisitionTaskShape,
  type LifecycleStepResult,
  type ResourceSelection,
  type TaskSnapshotState,
} from "../deviceModel";
import { applySelectionAdd } from "../resourceSelectionModel";
import { ApplyWithRestart } from "./ApplyWithRestart";
import { ResourcePickerAntd } from "./ResourcePickerAntd";

export function EndpointAcquisitionPane({
  endpointId,
  driverId,
  onChanged,
}: {
  endpointId: string;
  driverId: string;
  onChanged: () => void;
}) {
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [sels, setSels] = useState<ResourceSelection[]>([]);
  const [existingTasks, setExistingTasks] = useState<AcquisitionTaskShape[]>([]);
  const [preservedTasks, setPreservedTasks] = useState<AcquisitionTaskShape[]>([]);
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

  useEffect(() => {
    // 切 Endpoint 时先复位：旧快照不得污染新窗格（fail-closed 默认禁用保存）。
    setSels([]);
    setExistingTasks([]);
    setPreservedTasks([]);
    setIntervalMs(1000);
    setDesc(null);
    setTasksLoadState("loading");
    setTasksLoadedEpId(null);
    setTasksLoadError("");
    setSaveIssues([]);
    let cancelled = false;
    // Descriptor + Task 快照并行加载；二者都成功才开放保存。
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
        const { editable, preserved } = splitAcquisitionTasks(tasks);
        setPreservedTasks(preserved);
        if (editable) {
          if (editable.schedule?.mode === "poll") setIntervalMs(editable.schedule.interval_ms ?? 1000);
          const s = selectionsOf(editable);
          if (s.length) setSels(s);
        }
        // 快照就绪：只有此时保存才允许 PUT（空数组即服务端真的无任务）。
        setTasksLoadedEpId(endpointId);
        setTasksLoadState("ready");
        if (tasks.length) {
          message.info(
            preserved.length
              ? `已回显 canonical 任务，另有 ${preserved.length} 个任务将被保留（${preserved.map((t) => t.id).join("、")}）`
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
    // 描述门：Descriptor 未就绪同样不可 PUT（选型无合法依据，禁止凭空回写）。
    if (!desc) return message.error("资源描述加载失败，禁止保存。请切换后重试。");
    if (!sels.length) return message.warning("请先加入点位");
    const tasks = mergeAcquisitionTasks(existingTasks, { interval_ms: intervalMs, selections: sels });
    const preservedCount = tasks.length - 1;
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

  if (!desc && tasksLoadState === "loading") return <div style={{ color: "#525252" }}>加载资源…</div>;
  return (
    <div style={{ display: "grid", gap: 12, maxWidth: 860 }}>
      {tasksLoadState === "error" ? (
        <Alert type="error" message={tasksLoadError || "任务快照加载失败"} description="服务端已有任务未知，为防止覆盖已禁用保存。请切换后重试。" />
      ) : null}
      {tasksLoadState === "loading" ? <div style={{ fontSize: 12, color: "#525252" }}>正在加载任务快照…（快照就绪前保存保持禁用，防止覆盖已有任务）</div> : null}
      {!desc ? (
        <Alert type="error" message="资源描述加载失败" description="描述缺失时选型无合法依据，已禁用保存（不会覆盖已有任务）。请切换后重试。" />
      ) : (
      <>
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <span style={{ fontSize: 12 }}>采集周期</span>
        <InputNumber min={10} max={60000} step={10} value={intervalMs} onChange={(v) => setIntervalMs(v ?? 1000)} addonAfter="ms" style={{ width: 180 }} />
        <span style={{ fontSize: 12, color: "#525252" }}>10ms–60s，20ms已通过50K/s压测</span>
      </div>
      <ResourcePickerAntd
        resources={desc.resources}
        existingSelections={sels}
        protectedSelections={preservedTasks.flatMap((t) => selectionsOf(t))}
        selectionMethods={desc.resource_selection_methods}
        endpointId={endpointId}
        onAdd={(s) => {
          // 唯一防御门：与 AddDeviceFlow 同一共享实现（终审：两调用方行为一致，
          // protected 永不动；point_key 检查由 reconcile 统一执行）。
          const schemaOf = (resourceId: string) => {
            const found = desc.resources.find((r) => r.id === resourceId);
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
      </>)}
      <div style={{ fontSize: 12, color: "#525252" }}>
        已选 {sels.length} 项 · {intervalMs}ms 轮询 · 保存将执行 Stop → PUT /tasks/{endpointId} → Start
        <Button size="small" onClick={() => setSels([])} style={{ marginLeft: 8 }}>清空</Button>
      </div>
      {preservedTasks.length > 0 && (
        <div style={{ fontSize: 12, color: "#525252" }}>
          以下任务不在此编辑、保存时原样保留（只读，reconciliation 仅检测不修改）：
          {preservedTasks.map((t) => {
            const ss = selectionsOf(t);
            const keys = ss.flatMap((s) => s.outputs.map((o) => o.point_key));
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
      {!!sels.length && (
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
          canApply={sels.length > 0 && !!desc && isTaskSnapshotReady(tasksLoadState, tasksLoadedEpId, endpointId)}
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
