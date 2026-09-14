// Endpoint 采集配置窗格（Workspace「采集」tab；逻辑由 DeviceDetail 点位
// Modal 原样迁移）。单任务编辑器：只编辑 canonical（mesa.resources.v1）
// 任务，其余任务原样保留（P0 数据安全）；保存 Stop → PUT → Start。
import { useEffect, useState } from "react";
import { Button, InputNumber, Tag, message } from "antd";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import {
  mergeAcquisitionTasks,
  selectionsOf,
  splitAcquisitionTasks,
  type AcquisitionTaskShape,
  type ResourceSelection,
} from "../deviceModel";
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
  const [intervalMs, setIntervalMs] = useState(1000);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    fetch(`/api/v1/drivers/${driverId}/descriptor`)
      .then((x) => x.json())
      .then((d) => setDesc(d))
      .catch(() => setDesc(null));
    fetch(`/api/v1/tasks?endpoint=${endpointId}`)
      .then((x) => x.json())
      .then((j) => {
        const tasks = (j.tasks ?? []) as AcquisitionTaskShape[];
        setExistingTasks(tasks);
        const { editable, preserved } = splitAcquisitionTasks(tasks);
        setPreservedTasks(preserved);
        if (editable) {
          if (editable.mode === "poll") setIntervalMs(editable.interval_ms ?? 1000);
          const s = selectionsOf(editable);
          if (s.length) setSels(s);
        }
        if (tasks.length) {
          message.info(
            preserved.length
              ? `已回显 canonical 任务，另有 ${preserved.length} 个任务将被保留（${preserved.map((t) => t.id).join("、")}）`
              : `已回显 ${tasks.length} 任务`,
          );
        }
      })
      .catch(() => {});
  }, [endpointId, driverId]);

  const save = async () => {
    if (!sels.length) return message.warning("请先加入点位");
    const tasks = mergeAcquisitionTasks(existingTasks, { interval_ms: intervalMs, selections: sels });
    const preservedCount = tasks.length - 1;
    setSaving(true);
    try {
      await api.stopEndpoint(endpointId).catch(() => {});
      const r = await fetch(`/api/v1/tasks/${endpointId}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ tasks }),
      });
      const j = await r.json().catch(() => ({}));
      if (!r.ok) {
        message.error(j.error?.message ?? "点位保存失败");
        return;
      }
      message.success(preservedCount > 0 ? `点位已保存（另保留 ${preservedCount} 个任务），正在启动…` : "点位已保存，正在启动…");
      await api.startEndpoint(endpointId);
      onChanged();
    } finally {
      setSaving(false);
    }
  };

  if (!desc) return <div style={{ color: "#525252" }}>加载资源…</div>;
  return (
    <div style={{ display: "grid", gap: 12, maxWidth: 860 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <span style={{ fontSize: 12 }}>采集周期</span>
        <InputNumber min={10} max={60000} step={10} value={intervalMs} onChange={(v) => setIntervalMs(v ?? 1000)} addonAfter="ms" style={{ width: 180 }} />
        <span style={{ fontSize: 12, color: "#525252" }}>10ms–60s，20ms已通过50K/s压测</span>
      </div>
      <ResourcePickerAntd
        resources={desc.resources}
        existingKeys={sels.flatMap((s) => s.outputs.map((o) => o.point_key))}
        selectionMethods={desc.resource_selection_methods}
        endpointId={endpointId}
        onAdd={(s) => {
          const keys = s.outputs.map((o) => o.point_key);
          const dup = sels.some((ex) => ex.outputs.some((o) => keys.includes(o.point_key)));
          if (dup) {
            message.warning(`point_key 重复：${keys.join(", ")} 已存在`);
            return false;
          }
          setSels((p) => [...p, s]);
          return true;
        }}
      />
      <div style={{ fontSize: 12, color: "#525252" }}>
        已选 {sels.length} 项 · {intervalMs}ms 轮询 · 保存将执行 Stop → PUT /tasks/{endpointId} → Start
        <Button size="small" onClick={() => setSels([])} style={{ marginLeft: 8 }}>清空</Button>
      </div>
      {preservedTasks.length > 0 && (
        <div style={{ fontSize: 12, color: "#525252" }}>
          以下任务不在此编辑、保存时原样保留：
          {preservedTasks.map((t) => (
            <Tag key={t.id} style={{ marginLeft: 6 }}>{t.id} · {t.binding.kind} · {t.mode}</Tag>
          ))}
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
        <Button type="primary" onClick={save} loading={saving}>保存并启动</Button>
      </div>
    </div>
  );
}
