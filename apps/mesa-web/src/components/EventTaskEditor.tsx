// PR8 订阅配置：Descriptor-driven EventTask Editor。
// 流程 Endpoint → driver_id → DriverDescriptor.events.streams → 动态表单；
// 全文件禁止出现任何驱动业务判断（grep 锚点：无 driver 私有 kind 字面量）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Card, Input, InputNumber, Select, Space, Tag } from "antd";
import { DeleteOutlined, PlusOutlined } from "@ant-design/icons";
import { api } from "../api";
import type { DriverDescriptor, EventStreamDescriptor, EventTask, LocalizedText, TaskMode } from "../types";
import { DescriptorFields, materializeSchemaDefaults } from "./DescriptorFields";
import {
  GENERIC_EVENT_BINDING_KIND,
  buildGenericEventBinding,
  genericParametersOf,
  genericStreamIdOf,
  isGenericEventTask,
} from "../events/taskBinding";

interface EndpointOption {
  id: string;
  driver_id: string;
  running: boolean;
}

type DraftGeneric = {
  key: string;
  kind: "generic";
  id: string;
  streamId: string;
  mode: TaskMode;
  interval_ms: number | null;
  parameters: Record<string, unknown>;
};
type DraftLegacy = { key: string; kind: "legacy"; task: EventTask };
type Draft = DraftGeneric | DraftLegacy;

let draftSeq = 0;
const nextKey = () => `draft-${Date.now()}-${(draftSeq += 1)}`;

export function streamLabel(label: LocalizedText): string {
  return label?.["zh-CN"] ?? label?.default ?? "";
}

function streamById(streams: EventStreamDescriptor[], id: string): EventStreamDescriptor | undefined {
  return streams.find((s) => s.id === id);
}

function toDraft(task: EventTask, streams: EventStreamDescriptor[] = []): Draft {
  if (!isGenericEventTask(task)) return { key: nextKey(), kind: "legacy", task };
  const streamId = genericStreamIdOf(task) ?? "";
  // P1-4：落盘/服务端参数与 schema defaults 合并物化——UI 显示的 default 即实际值，
  // required+default 字段不再出现“显示 100 却禁保存”的不一致。
  const st = streamById(streams, streamId);
  const parameters = {
    ...materializeSchemaDefaults(st?.parameters ?? { fields: [] }),
    ...genericParametersOf(task),
  };
  return {
    key: nextKey(),
    kind: "generic",
    id: task.id,
    streamId,
    mode: task.mode,
    interval_ms: task.interval_ms ?? null,
    parameters,
  };
}

function validateDraft(d: DraftGeneric, streams: EventStreamDescriptor[]): string | null {
  if (d.id.trim() === "") return "Task ID 不能为空";
  const st = streamById(streams, d.streamId);
  if (!st) return `未知事件流 ${d.streamId || "(空)"}`;
  if (!st.modes.includes(d.mode)) return `该流不支持模式 ${d.mode}`;
  if (d.mode === "poll" && !(typeof d.interval_ms === "number" && d.interval_ms > 0)) return "Poll 模式必须提供正整数 interval_ms";
  for (const f of st.parameters.fields ?? []) {
    if (!f.required) continue;
    const v = d.parameters[f.key];
    if (v === undefined || v === null || v === "") return `参数 ${f.key} 为必填`;
  }
  return null;
}

export function EventTaskEditor() {
  const [endpoints, setEndpoints] = useState<EndpointOption[]>([]);
  const [selectedId, setSelectedId] = useState<string | undefined>(undefined);
  const [descriptor, setDescriptor] = useState<DriverDescriptor | null>(null);
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  // P0 归属门：drafts/descriptor 到底属于哪个 Endpoint。切换即失效旧状态；
  // 加载失败绝不重新暴露上一台设备的 editor；保存仅当归属与选中一致才允许。
  const [loadedEndpointId, setLoadedEndpointId] = useState<string | null>(null);
  // P0-2：Endpoint 切换竞态——旧请求返回绝不能覆盖新 Endpoint 的 descriptor/drafts，
  // 否则可能把 A 的事件配置写进 B。
  const loadGen = useRef(0);

  // Endpoint 列表（含运行态；running → 只读）
  const refreshEndpoints = useCallback(async () => {
    const j = await api.listEndpoints();
    const list = ((j.endpoints ?? []) as { id: string; driver_id: string; runtime?: { state?: string } }[]).map(
      (e) => ({ id: e.id, driver_id: e.driver_id, running: !!e.runtime && e.runtime.state !== "STOPPED" }),
    );
    setEndpoints(list);
    if (!selectedId && list.length > 0) setSelectedId(list[0].id);
    return list;
  }, [selectedId]);

  useEffect(() => {
    refreshEndpoints().catch((e) => setError(e instanceof Error ? e.message : String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selected = useMemo(() => endpoints.find((e) => e.id === selectedId), [endpoints, selectedId]);
  const running = !!selected?.running;
  const streams = useMemo(() => descriptor?.events?.streams ?? [], [descriptor]);

  // 选中 Endpoint → Descriptor + 现有 EventTask（代际守卫：stale 响应直接丢弃）
  useEffect(() => {
    if (!selected) return;
    const endpointId = selected.id;
    const driverId = selected.driver_id;
    const id = ++loadGen.current;
    setLoading(true);
    setError(null);
    setSaved(false);
    // 切换即失效旧归属：失败/竞态下不残留上一台设备的可编辑内容
    setLoadedEndpointId(null);
    setDescriptor(null);
    setDrafts([]);
    Promise.all([api.getDescriptor(driverId), api.listEventTasks(endpointId)])
      .then(([desc, tasks]) => {
        if (id !== loadGen.current) return;
        const streams = (desc as DriverDescriptor).events?.streams ?? [];
        setDescriptor(desc as DriverDescriptor);
        setDrafts(((tasks.event_tasks ?? []) as EventTask[]).map((t) => toDraft(t, streams)));
        setLoadedEndpointId(endpointId);
        setLoading(false);
      })
      .catch((e) => {
        if (id !== loadGen.current) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      });
  }, [selected?.id, selected?.driver_id]);

  const patchGeneric = (key: string, patch: Partial<DraftGeneric>) =>
    setDrafts((cur) => cur.map((d) => (d.key === key && d.kind === "generic" ? { ...d, ...patch } : d)));

  const addTask = () => {
    const first = streams[0];
    if (!first) return;
    const mode = first.modes[0] ?? "subscribe";
    setDrafts((cur) => [
      ...cur,
      {
        key: nextKey(),
        kind: "generic",
        id: `event-${cur.length + 1}`,
        streamId: first.id,
        mode,
        interval_ms: mode === "poll" ? 1000 : null,
        parameters: materializeSchemaDefaults(first.parameters),
      },
    ]);
  };

  const removeDraft = (key: string) => setDrafts((cur) => cur.filter((d) => d.key !== key));

  const problems = useMemo(() => {
    const ids = new Map<string, number>();
    for (const d of drafts) {
      if (d.kind !== "generic") continue;
      ids.set(d.id.trim(), (ids.get(d.id.trim()) ?? 0) + 1);
    }
    return drafts.map((d) => {
      if (d.kind !== "generic") return null;
      if (d.id.trim() !== "" && (ids.get(d.id.trim()) ?? 0) > 1) return "Task ID 重复";
      return validateDraft(d, streams);
    });
  }, [drafts, streams]);

  const hasProblem = problems.some((p) => p !== null);

  const save = async () => {
    // 归属门：只有当前选中 Endpoint 成功加载出的配置才允许保存
    if (!selected || loadedEndpointId !== selected.id || running || saving || hasProblem) return;
    setSaving(true);
    setError(null);
    setSaved(false);
    const tasks: EventTask[] = drafts.map((d) => {
      if (d.kind === "legacy") return d.task;
      const mode = d.mode;
      return {
        id: d.id.trim(),
        mode,
        interval_ms: mode === "poll" ? d.interval_ms : null,
        binding: buildGenericEventBinding(d.streamId, d.parameters),
      };
    });
    try {
      await api.replaceEventTasks(selected.id, tasks);
      setSaved(true);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      const status = (e as { status?: number }).status;
      // 运行中竞态 409：显示服务端错误并刷新运行态，保留用户表单不丢
      if (status === 409) {
        setError(`保存冲突（409）：Endpoint 可能已启动。${msg}`);
        refreshEndpoints().catch(() => {});
      } else {
        setError(msg);
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <Space>
        <span>Endpoint</span>
        <Select
          value={selectedId}
          onChange={setSelectedId}
          style={{ minWidth: 240 }}
          placeholder="选择 Endpoint"
          options={endpoints.map((e) => ({ value: e.id, label: `${e.id}${e.running ? "（运行中）" : ""}` }))}
        />
        {selected ? <Tag>{selected.driver_id}</Tag> : null}
        {running ? <Tag color="orange">运行中 · 只读</Tag> : <Tag color="green">已停止 · 可编辑</Tag>}
      </Space>

      {running ? (
        <Alert type="warning" showIcon message="事件任务只能在 Endpoint 停止状态修改" description="停止设备必须是用户显式动作；本页不会自动 Stop。" />
      ) : null}
      {error ? <Alert type="error" showIcon message="订阅配置失败" description={error} /> : null}
      {saved ? <Alert type="success" showIcon message="已保存" description="EventTask 全量快照已替换。" /> : null}

      {loading ? (
        <Card size="small" loading />
      ) : !selected ? (
        <Alert type="info" showIcon message="请选择 Endpoint" description="选择后加载其事件订阅配置。" />
      ) : !descriptor ? (
        <Alert
          type="info"
          showIcon
          message="订阅配置不可用"
          description="该 Endpoint 的描述与任务尚未成功加载（见上方错误）；不会显示其他设备的旧配置，保存已禁用。"
        />
      ) : streams.length === 0 ? (
        <Alert type="info" showIcon message="该驱动未声明事件流" description="descriptor.events 为空，无可配置的事件订阅。" />
      ) : (
        <div style={{ display: "grid", gap: 12 }}>
          {drafts.map((d, i) => {
            if (d.kind === "legacy") {
              return (
                <Card
                  key={d.key}
                  size="small"
                  title={
                    <Space>
                      <span>{d.task.id}</span>
                      <Tag color="default">Legacy / Private Binding</Tag>
                    </Space>
                  }
                  extra={
                    <Button size="small" danger icon={<DeleteOutlined />} disabled={running} onClick={() => removeDraft(d.key)}>
                      删除
                    </Button>
                  }
                >
                  <div style={{ fontSize: 12, color: "#888" }}>kind: <span style={{ fontFamily: "monospace" }}>{d.task.binding.kind}</span></div>
                  <pre style={{ fontSize: 12, background: "rgba(0,0,0,.04)", padding: 8, borderRadius: 6, overflow: "auto" }}>
                    {JSON.stringify(d.task.binding.config, null, 2)}
                  </pre>
                  <div style={{ fontSize: 12, color: "#888" }}>私有绑定只读展示、可删除，不可结构化编辑。</div>
                </Card>
              );
            }
            const st = streamById(streams, d.streamId);
            const problem = problems[i];
            return (
              <Card
                key={d.key}
                size="small"
                title={<span>任务 · {d.id || "(未命名)"}</span>}
                extra={
                  <Button size="small" danger icon={<DeleteOutlined />} disabled={running} onClick={() => removeDraft(d.key)}>
                    删除
                  </Button>
                }
              >
                <div style={{ display: "grid", gap: 10 }}>
                  <Space wrap>
                    <span>Task ID</span>
                    <Input
                      value={d.id}
                      disabled={running}
                      onChange={(e) => patchGeneric(d.key, { id: e.target.value })}
                      style={{ width: 200, fontFamily: "monospace" }}
                    />
                    <span>Event Stream</span>
                    <Select
                      value={d.streamId}
                      disabled={running}
                      onChange={(v) => {
                        const ns = streamById(streams, v);
                        const nm = ns?.modes[0] ?? "subscribe";
                        patchGeneric(d.key, {
                          streamId: v,
                          mode: nm,
                          interval_ms: nm === "poll" ? d.interval_ms ?? 1000 : null,
                          parameters: materializeSchemaDefaults(ns?.parameters ?? { fields: [] }),
                        });
                      }}
                      style={{ minWidth: 260 }}
                      options={streams.map((s) => ({ value: s.id, label: `${streamLabel(s.label)} (${s.id})` }))}
                    />
                    <span>Mode</span>
                    <Select
                      value={d.mode}
                      disabled={running}
                      onChange={(v: TaskMode) =>
                        patchGeneric(d.key, { mode: v, interval_ms: v === "poll" ? d.interval_ms ?? 1000 : null })
                      }
                      style={{ width: 130 }}
                      options={(st?.modes ?? []).map((m) => ({ value: m, label: m }))}
                    />
                    {d.mode === "poll" ? (
                      <>
                        <span>Interval ms</span>
                        <InputNumber
                          min={1}
                          value={d.interval_ms ?? undefined}
                          disabled={running}
                          onChange={(v) => patchGeneric(d.key, { interval_ms: typeof v === "number" ? v : null })}
                        />
                      </>
                    ) : null}
                  </Space>
                  {st ? (
                    <>
                      <DescriptorFields
                        schema={st.parameters}
                        value={d.parameters}
                        disabled={running}
                        onChange={(next) => patchGeneric(d.key, { parameters: next })}
                      />
                      {st.fields.length > 0 ? (
                        <div style={{ fontSize: 12, color: "#888" }}>
                          该流可能产生的字段：{st.fields.map((f) => f.key).join(" · ")}
                        </div>
                      ) : null}
                    </>
                  ) : null}
                  {problem ? <Alert type="error" showIcon message={problem} /> : null}
                </div>
              </Card>
            );
          })}
          <div>
            <Button icon={<PlusOutlined />} onClick={addTask} disabled={running || streams.length === 0}>
              添加订阅
            </Button>
            <span style={{ marginLeft: 12, fontSize: 12, color: "#888" }}>只生成 {GENERIC_EVENT_BINDING_KIND}</span>
          </div>
          <div>
            <Button
              type="primary"
              onClick={save}
              loading={saving}
              disabled={running || loading || loadedEndpointId !== selected?.id || hasProblem || !selected}
            >
              保存订阅
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
