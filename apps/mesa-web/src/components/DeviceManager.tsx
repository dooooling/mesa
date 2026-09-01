// 设备：简单表格 + 新增，无引导
import { useEffect, useState } from "react";
import { Button, Card, Form, Input, Modal, Select, Space, Table, Tag, message } from "antd";

const DRIVERS = [
  { value: "simulator", label: "Simulator" },
  { value: "s7", label: "Siemens S7" },
  { value: "focas2", label: "FANUC FOCAS2" },
  { value: "opcua", label: "OPC UA" },
];

export function DeviceManager() {
  const [endpoints, setEndpoints] = useState<Array<{ id: string; driver_id: string; state?: string }>>([]);
  const [open, setOpen] = useState(false);
  const [form] = Form.useForm();

  const load = () => fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => setEndpoints(j.endpoints ?? [])).catch(() => {});
  useEffect(() => { load(); }, []);

  const create = async () => {
    try {
      const v = await form.validateFields();
      const id = v.id?.trim() || `${v.driver_id}-${Date.now().toString(36)}`;
      const connection: Record<string, unknown> = {};
      if (v.host) connection.host = v.host;
      if (v.port) connection.port = Number(v.port);
      if (v.rack !== undefined) connection.rack = Number(v.rack);
      if (v.slot !== undefined) connection.slot = Number(v.slot);
      // 其余字段原样带入
      Object.keys(v).forEach((k) => { if (!["id", "driver_id", "host", "port", "rack", "slot"].includes(k) && v[k] !== undefined && v[k] !== "") connection[k] = v[k]; });
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
      <Card size="small" extra={<Button type="primary" onClick={() => setOpen(true)}>新增设备</Button>} title={`设备 · ${endpoints.length}`}>
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

      <Modal title="新增设备" open={open} onOk={create} onCancel={() => setOpen(false)} okText="创建" destroyOnClose>
        <Form form={form} layout="vertical" initialValues={{ driver_id: "simulator" }}>
          <Form.Item name="driver_id" label="驱动" rules={[{ required: true }]}><Select options={DRIVERS} /></Form.Item>
          <Form.Item name="id" label="ID（可空自动生成）"><Input placeholder="s7-01" /></Form.Item>
          <Form.Item name="host" label="Host"><Input placeholder="192.168.0.10" /></Form.Item>
          <Form.Item name="port" label="Port"><Input placeholder="102 / 8193 / 4840" /></Form.Item>
          <Form.Item name="rack" label="Rack"><Input placeholder="0" /></Form.Item>
          <Form.Item name="slot" label="Slot"><Input placeholder="1" /></Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
