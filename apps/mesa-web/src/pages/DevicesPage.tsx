// PR27 Device-first：Device 列表页。只展示 Device（GET /devices），
// 数量只数 Device；Endpoint 数量另算（/endpoints），仅作为每行的
// “连接数”附属信息展示，绝不混入 Device 计数。行点击进入 Device detail。
import { useEffect, useState } from "react";
import { Button, Card, Form, Input, Modal, Space, Table, Tag, message } from "antd";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import { deviceCounts, groupEndpointsByDevice, type Device } from "../deviceModel";

export function DevicesPage() {
  const nav = useNavigate();
  const [devices, setDevices] = useState<Device[]>([]);
  const [epCountByDevice, setEpCountByDevice] = useState<Map<string, number>>(new Map());
  const [endpointTotal, setEndpointTotal] = useState(0);
  const [open, setOpen] = useState(false);
  const [form] = Form.useForm();

  const load = async () => {
    try {
      const dj = (await api.listDevices()) as { devices?: Device[] };
      const ds = dj.devices ?? [];
      setDevices(ds);
      const ej = (await api.listEndpoints()) as { endpoints?: Array<{ id: string; device_id?: string }> };
      const eps = ej.endpoints ?? [];
      const groups = groupEndpointsByDevice(
        eps.map((e) => ({ id: e.id, driver_id: "", device_id: e.device_id })),
      );
      const m = new Map<string, number>();
      for (const [k, v] of groups) if (k) m.set(k, v.length);
      setEpCountByDevice(m);
      setEndpointTotal(eps.length);
    } catch {
      message.error("加载设备列表失败");
    }
  };
  useEffect(() => { load(); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const { deviceCount } = deviceCounts(devices, []);

  const create = async () => {
    try {
      const v = (await form.validateFields()) as { id: string; name: string };
      const id = v.id.trim();
      const name = v.name.trim() || id;
      const r = await api.createDevice({ id, name });
      if (r.status !== 201 && r.status !== 200) {
        message.error(r.body?.error?.message ?? "创建设备失败");
        return;
      }
      message.success(`已创建设备 ${id}`);
      setOpen(false);
      form.resetFields();
      load();
    } catch { /* antd 校验未通过 */ }
  };

  const remove = async (id: string) => {
    const n = epCountByDevice.get(id) ?? 0;
    if (n > 0) {
      message.warning(`该设备下仍有 ${n} 个连接，请先到设备详情删除关联 Endpoint`);
      return;
    }
    const r = await api.deleteDevice(id);
    if (r.status !== 200) {
      // 后端 RESTRICT 为最终裁决（如竞态下仍有 Endpoint），直接透传服务端文案
      message.error(r.body?.error?.message ?? "删除失败");
      load();
      return;
    }
    message.success("已删除设备");
    load();
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Card
        size="small"
        title={`设备 · ${deviceCount}`}
        extra={
          <Space>
            <Button onClick={() => nav("/onboarding")}>新建向导</Button>
            <Button type="primary" onClick={() => setOpen(true)}>新建设备</Button>
          </Space>
        }
      >
        <Table
          size="small"
          rowKey="id"
          dataSource={devices}
          onRow={(r) => ({ onClick: () => nav(`/devices/${r.id}`), style: { cursor: "pointer" } })}
          columns={[
            { title: "ID", dataIndex: "id", render: (v: string) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{v}</span> },
            { title: "名称", dataIndex: "name", render: (v: string) => v ?? "—" },
            {
              title: "连接",
              dataIndex: "id",
              render: (id: string) => {
                const n = epCountByDevice.get(id) ?? 0;
                return n > 0 ? <Tag color="blue">{n} 个连接</Tag> : <Tag>无连接</Tag>;
              },
            },
            {
              title: "操作",
              render: (_: unknown, r: Device) => (
                <Space onClick={(e) => e.stopPropagation()}>
                  <Button size="small" onClick={() => nav(`/devices/${r.id}`)}>进入</Button>
                  <Button size="small" danger onClick={() => remove(r.id)}>删除</Button>
                </Space>
              ),
            },
          ]}
          locale={{ emptyText: "暂无设备，先新建设备或走新建向导" }}
        />
        <div style={{ marginTop: 8, fontSize: 12, color: "#999" }}>
          设备 {deviceCount} 个 · 连接 {endpointTotal} 个（连接归属设备，不计入设备数）
        </div>
      </Card>

      <Modal title="新建设备" open={open} onOk={create} onCancel={() => setOpen(false)} okText="创建" destroyOnHidden>
        <Form form={form} layout="vertical">
          <Form.Item name="id" label="设备 ID" rules={[{ required: true, message: "设备 ID 必填" }]}>
            <Input placeholder="device-a" style={{ fontFamily: "monospace" }} />
          </Form.Item>
          <Form.Item name="name" label="设备名称">
            <Input placeholder="默认为 ID" />
          </Form.Item>
          <div style={{ fontSize: 12, color: "#999" }}>只创建 Device；连接在设备详情页按需添加，可一对多。</div>
        </Form>
      </Modal>
    </div>
  );
}
