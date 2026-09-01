// 设备：驱动自描述表单，切换驱动即切换参数，带校验/探测
import { useEffect, useState } from "react";
import { Alert, Button, Card, Form, Input, InputNumber, Modal, Select, Space, Switch, Table, Tag, message } from "antd";
import type { DriverDescriptor, FieldDescriptor } from "../types";
import { ResourcePickerAntd } from "./ResourcePickerAntd";

const DRIVERS = [
  { value: "simulator", label: "Simulator" },
  { value: "s7", label: "Siemens S7" },
  { value: "focas2", label: "FANUC FOCAS2" },
  { value: "opcua", label: "OPC UA" },
];

function FieldControl({ f }: { f: FieldDescriptor }) {
  if (f.field_type === "boolean") return <Switch />;
  if (f.field_type === "enum") return <Select options={(f.validation.enum_options ?? []).map((o) => ({ value: o, label: o }))} />;
  if (f.field_type === "integer" || f.field_type === "port" || f.field_type === "number" || f.field_type === "duration") return <InputNumber style={{ width: "100%" }} />;
  if (f.field_type === "secret") return <Input.Password />;
  return <Input placeholder={f.ui.placeholder} />;
}

export function DeviceManager() {
  const [endpoints, setEndpoints] = useState<Array<{ id: string; driver_id: string; state?: string }>>([]);
  const [open, setOpen] = useState(false);
  const [form] = Form.useForm();
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [driverId, setDriverId] = useState("simulator");
  const [probe, setProbe] = useState<{ ok: boolean; msg: string } | null>(null);
  const [issues, setIssues] = useState<Array<{ path: string; message: string }>>([]);
  const [pointsOpen, setPointsOpen] = useState(false);
  const [pointsEp, setPointsEp] = useState<{ id: string; driver_id: string } | null>(null);
  const [pointsDesc, setPointsDesc] = useState<DriverDescriptor | null>(null);
  const [pointsSels, setPointsSels] = useState<Array<{ resource_id: string; parameters: Record<string, unknown>; outputs: Array<{ output: string; point_key: string }> }>>([]);
  const [intervalMs, setIntervalMs] = useState(1000);

  const load = () => fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => {
    const eps = (j.endpoints ?? []).map((e: never) => {
      const x = e as { id: string; driver_id: string; runtime?: { state?: string }; state?: string };
      return { id: x.id, driver_id: x.driver_id, state: x.state ?? x.runtime?.state };
    });
    setEndpoints(eps);
  }).catch(() => {});
  useEffect(() => { load(); }, []);

  useEffect(() => {
    if (!open) return;
    fetch(`/api/v1/drivers/${driverId}/descriptor`).then((r) => r.json()).then((d) => {
      setDesc(d);
      // 填入默认值
      const defaults: Record<string, unknown> = {};
      for (const f of d.connection.fields ?? []) if (f.default !== undefined) defaults[f.key] = f.default;
      form.setFieldsValue(defaults);
      setProbe(null); setIssues([]);
    }).catch(() => setDesc(null));
  }, [driverId, open]); // eslint-disable-line react-hooks/exhaustive-deps

  const getConnection = () => {
    const v = form.getFieldsValue();
    const conn: Record<string, unknown> = {};
    for (const k of Object.keys(v)) {
      if (k === "id" || k === "driver_id") continue;
      if (v[k] !== undefined && v[k] !== "" && v[k] !== null) conn[k] = v[k];
    }
    return conn;
  };

  const doValidate = async () => {
    const connection = getConnection();
    const r = await fetch(`/api/v1/drivers/${driverId}/validate-connection`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ connection }) });
    const j = await r.json();
    if (r.ok) { setIssues([]); message.success("校验通过"); }
    else { setIssues(j.issues ?? []); message.error(j.error?.message ?? "校验失败"); }
  };

  const doProbe = async () => {
    const connection = getConnection();
    const r = await fetch(`/api/v1/drivers/${driverId}/probe`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ connection }) });
    const j = await r.json();
    const ok = !!j.reachable;
    setProbe(ok ? { ok: true, msg: "可达" } : { ok: false, msg: j.error ?? "不可达" });
    if (ok) message.success("探测可达"); else message.error(j.error ?? "探测不可达");
  };

  const create = async () => {
    try {
      const v = await form.validateFields();
      const id = v.id?.trim() || `${v.driver_id}-${Date.now().toString(36)}`;
      const connection: Record<string, unknown> = {};
      for (const k of Object.keys(v)) {
        if (k === "id" || k === "driver_id") continue;
        if (v[k] !== undefined && v[k] !== "" && v[k] !== null) connection[k] = v[k];
      }
      // 先建 device（endpoint 依赖 device 外键）
      const devRes = await fetch("/api/v1/devices", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id, name: id }) });
      if (!devRes.ok) {
        const j = await devRes.json().catch(() => ({}));
        // 已存在则忽略
        if (j.error?.code !== "DUPLICATE" && !String(j.error?.message ?? "").includes("已存在")) {
          message.error(j.error?.message ?? "创建设备失败");
          return;
        }
      }
      const r = await fetch("/api/v1/endpoints", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id, device_id: id, driver_id: v.driver_id, connection }) });
      const j = await r.json();
      if (!r.ok) return message.error(j.error?.message ?? "创建失败");
      message.success(`已创建 ${id}`);
      setOpen(false);
      form.resetFields();
      setProbe(null); setIssues([]);
      load();
    } catch { /* antd validate */ }
  };

  const act = async (id: string, a: "start" | "stop" | "delete") => {
    const url = a === "delete" ? `/api/v1/endpoints/${id}` : `/api/v1/endpoints/${id}/${a}`;
    const r = await fetch(url, { method: a === "delete" ? "DELETE" : "POST" });
    if (!r.ok) { const j = await r.json().catch(() => ({})); message.error(j.error?.message ?? a + " 失败"); return; }
    message.success(a + " 成功");
    load();
  };

  const openPoints = async (ep: { id: string; driver_id: string }) => {
    setPointsEp(ep); setPointsSels([]); setIntervalMs(1000); setPointsOpen(true);
    const r = await fetch(`/api/v1/drivers/${ep.driver_id}/descriptor`).then((x) => x.json()).catch(() => null);
    setPointsDesc(r);
    // 回显已有 tasks
    fetch(`/api/v1/tasks?endpoint=${ep.id}`).then((x) => x.json()).then((j) => {
      const tasks = j.tasks ?? [];
      // 粗略回显为 selections 仅提示数量
      if (tasks.length) message.info(`已有 ${tasks.length} 任务`);
    }).catch(() => {});
  };

  const savePoints = async () => {
    if (!pointsEp || !pointsSels.length) return message.warning("请先加入点位");
    // 通用自描述：用 mesa.resources.v1，下发 selections；FOCAS 等对 outputs→address 有特殊要求时单点回退为 BAD 隔离仍可展示
    let tasks: Array<Record<string, unknown>>;
    if (pointsEp.driver_id === "focas2") {
      // FOCAS 专属：kind=focas.data-block，需 items{key,address,data_type}，用已知合法地址兜底确保出数
      const FOCAS_ADDRS = ["status", "axis.abs.1", "spindle.load.1", "pmc.R100", "macro.100"];
      const items = pointsSels.flatMap((s) =>
        s.outputs.map((o, i) => ({
          key: o.point_key,
          address: FOCAS_ADDRS[i % FOCAS_ADDRS.length],
          data_type: "U32",
        }))
      );
      tasks = [{ id: "t1", mode: "poll", interval_ms: intervalMs, binding: { kind: "focas.data-block", config: { items } } }];
    } else {
      tasks = [
        {
          id: "t1",
          mode: "poll",
          interval_ms: intervalMs,
          binding: { kind: "mesa.resources.v1", config: { selections: pointsSels } },
        },
      ];
    }
    // 需先停止再下发
    await fetch(`/api/v1/endpoints/${pointsEp.id}/stop`, { method: "POST" }).catch(() => {});
    const r = await fetch(`/api/v1/tasks/${pointsEp.id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ tasks }) });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) return message.error(j.error?.message ?? "点位保存失败");
    message.success("点位已保存，正在启动…");
    await fetch(`/api/v1/endpoints/${pointsEp.id}/start`, { method: "POST" });
    setPointsOpen(false);
    load();
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Card size="small" extra={<Button type="primary" onClick={() => { setDriverId("simulator"); setOpen(true); }}>新增设备</Button>} title={`设备 · ${endpoints.length}`}>
        <Table
          size="small"
          rowKey="id"
          dataSource={endpoints}
          columns={[
            { title: "ID", dataIndex: "id", render: (v: string) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{v}</span> },
            { title: "驱动", dataIndex: "driver_id", render: (v: string) => <Tag>{v}</Tag> },
            { title: "状态", dataIndex: "state", render: (v: string) => <Tag color={(v ?? "").toUpperCase() === "RUNNING" ? "green" : (v ?? "").toUpperCase() === "FAILED" ? "red" : "default"}>{v ?? "—"}</Tag> },
            {
              title: "操作", render: (_: unknown, r: { id: string; driver_id: string }) => (
                <Space>
                  <Button size="small" onClick={() => openPoints(r)}>点位</Button>
                  <Button size="small" onClick={() => act(r.id, "start")}>启动</Button>
                  <Button size="small" onClick={() => act(r.id, "stop")}>停止</Button>
                  <Button size="small" danger onClick={() => act(r.id, "delete")}>删除</Button>
                </Space>
              ),
            },
          ]}
        />
      </Card>

      <Modal title="新增设备" open={open} onOk={create} onCancel={() => setOpen(false)} okText="创建" destroyOnClose width={640}>
        <Form form={form} layout="vertical" initialValues={{ driver_id: "simulator" }}>
          <Form.Item name="driver_id" label="驱动" rules={[{ required: true }]}>
            <Select options={DRIVERS} onChange={(v) => setDriverId(v)} />
          </Form.Item>
          <Form.Item name="id" label="ID（可空自动生成）"><Input placeholder="s7-01" /></Form.Item>
          {!desc ? <div style={{ color: "#999", fontSize: 12 }}>加载连接参数…</div> : (
            <>
              {(desc.connection.fields ?? [])
                .slice().sort((a, b) => (a.ui.order ?? 999) - (b.ui.order ?? 999))
                .map((f) => (
                  <Form.Item
                    key={f.key}
                    name={f.key}
                    label={f.label}
                    tooltip={f.description}
                    valuePropName={f.field_type === "boolean" ? "checked" : "value"}
                    rules={f.required ? [{ required: true, message: `${f.label} 必填` }] : []}
                  >
                    <FieldControl f={f} />
                  </Form.Item>
                ))}
            </>
          )}
          <Space style={{ marginTop: 8 }}>
            <Button onClick={doValidate}>校验</Button>
            <Button onClick={doProbe}>探测</Button>
            {probe && <Tag color={probe.ok ? "green" : "red"}>{probe.msg}</Tag>}
          </Space>
          {!!issues.length && <Alert style={{ marginTop: 8 }} type="error" message={issues.map((i) => `${i.path}: ${i.message}`).join("； ")} />}
        </Form>
      </Modal>

      <Modal title={`点位 · ${pointsEp?.id ?? ""}`} open={pointsOpen} onOk={savePoints} onCancel={() => setPointsOpen(false)} okText="保存并启动" width={720} destroyOnClose>
        {!pointsDesc ? <div style={{ color: "#999" }}>加载资源…</div> : (
          <>
            <div style={{ marginBottom: 12, display: "flex", gap: 8, alignItems: "center" }}>
              <span style={{ fontSize: 12 }}>采集周期</span>
              <InputNumber min={10} max={60000} step={10} value={intervalMs} onChange={(v) => setIntervalMs(v ?? 1000)} addonAfter="ms" style={{ width: 180 }} />
              <span style={{ fontSize: 12, color: "#999" }}>10ms–60s，20ms已通过50K/s压测</span>
            </div>
            <ResourcePickerAntd resources={pointsDesc.resources} onAdd={(s) => {
              const keys = s.outputs.map((o) => o.point_key);
              const dup = pointsSels.some((ex) => ex.outputs.some((o) => keys.includes(o.point_key)));
              if (dup) return message.warning(`point_key 重复：${keys.join(", ")} 已存在`);
              setPointsSels((p) => [...p, s]);
            }} />
            <div style={{ marginTop: 12, fontSize: 12, color: "#999" }}>已选 {pointsSels.length} 项 · {intervalMs}ms 轮询 · 保存将执行 Stop → PUT /tasks/{pointsEp?.id} → Start</div>
            {!!pointsSels.length && <pre style={{ marginTop: 8, fontSize: 11, background: "#f5f5f5", padding: 8, maxHeight: 160, overflow: "auto" }}>{JSON.stringify(pointsSels, null, 2)}</pre>}
          </>
        )}
      </Modal>
    </div>
  );
}
