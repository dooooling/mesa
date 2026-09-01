// 设备：驱动自描述表单，切换驱动即切换参数
import { useEffect, useState } from "react";
import { Button, Card, Form, Input, InputNumber, Modal, Select, Space, Switch, Table, Tag, message } from "antd";
import type { DriverDescriptor, FieldDescriptor } from "../types";

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

  const load = () => fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => setEndpoints(j.endpoints ?? [])).catch(() => {});
  useEffect(() => { load(); }, []);

  // 弹框内驱动切换即拉取 Descriptor
  useEffect(() => {
    if (!open) return;
    fetch(`/api/v1/drivers/${driverId}/descriptor`).then((r) => r.json()).then(setDesc).catch(() => setDesc(null));
    // 保留已填的 id，清空连接字段
    const cur = form.getFieldsValue();
    form.resetFields();
    form.setFieldsValue({ driver_id: driverId, id: cur.id });
  }, [driverId, open]); // eslint-disable-line react-hooks/exhaustive-deps

  const create = async () => {
    try {
      const v = await form.validateFields();
      const id = v.id?.trim() || `${v.driver_id}-${Date.now().toString(36)}`;
      const connection: Record<string, unknown> = {};
      for (const k of Object.keys(v)) {
        if (k === "id" || k === "driver_id") continue;
        if (v[k] !== undefined && v[k] !== "") connection[k] = v[k];
      }
      const r = await fetch("/api/v1/endpoints", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id, device_id: id, driver_id: v.driver_id, connection }) });
      const j = await r.json();
      if (!r.ok) return message.error(j.error?.message ?? "创建失败");
      message.success(`已创建 ${id}`);
      setOpen(false);
      form.resetFields();
      load();
    } catch { /* validate */ }
  };

  const act = async (id: string, a: "start" | "stop" | "delete") => {
    const url = a === "delete" ? `/api/v1/endpoints/${id}` : `/api/v1/endpoints/${id}/${a}`;
    const r = await fetch(url, { method: a === "delete" ? "DELETE" : "POST" });
    if (!r.ok) { const j = await r.json().catch(() => ({})); message.error(j.error?.message ?? a + " 失败"); return; }
    message.success(a + " 成功");
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
            { title: "状态", dataIndex: "state", render: (v: string) => <Tag color={v === "running" ? "green" : "default"}>{v ?? "—"}</Tag> },
            {
              title: "操作", render: (_: unknown, r: { id: string; state?: string }) => (
                <Space>
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
        </Form>
      </Modal>
    </div>
  );
}
