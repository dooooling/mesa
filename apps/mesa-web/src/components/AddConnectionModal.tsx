// 新增连接 Modal（逻辑由 DeviceConfig 原样迁移）：
// 选择 Driver → Descriptor → 名称/ID → 连接参数 → 校验/探测 → 创建。
// 创建成功后 onCreated（父页刷新 inventory）。
import { useEffect, useState } from "react";
import { Alert, Button, Form, Input, Modal, Select, Space, Tag, message } from "antd";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import {
  buildEndpointCreatePayload,
  cleanConnection,
  suggestEndpointId,
} from "../deviceModel";
import { DescriptorFields, materializeSchemaDefaults } from "./DescriptorFields";

const FALLBACK_DRIVERS = [
  { value: "simulator", label: "Simulator" },
  { value: "s7", label: "Siemens S7" },
  { value: "focas2", label: "FANUC FOCAS2" },
  { value: "opcua", label: "OPC UA" },
  { value: "sinumerik-nck", label: "SINUMERIK NCK" },
];

export function AddConnectionModal(props: {
  deviceId: string;
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
}) {
  const { deviceId, open, onClose, onCreated } = props;
  const [form] = Form.useForm();
  const [driverId, setDriverId] = useState("simulator");
  const [driverOptions, setDriverOptions] = useState(FALLBACK_DRIVERS);
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [conn, setConn] = useState<Record<string, unknown>>({});
  const [issues, setIssues] = useState<Array<{ path: string; message: string }>>([]);
  const [probe, setProbe] = useState<{ ok: boolean; msg: string } | null>(null);

  useEffect(() => {
    api.listDrivers().then((j) => {
      const ds = ((j as { drivers?: Array<{ id: string; name: string }> }).drivers ?? [])
        .map((d) => ({ value: d.id, label: d.name }));
      if (ds.length) {
        setDriverOptions(ds);
        setDriverId((cur) => (ds.some((x) => x.value === cur) ? cur : ds[0].value));
      }
    }).catch(() => {});
  }, []);

  useEffect(() => {
    if (!open) return;
    fetch(`/api/v1/drivers/${driverId}/descriptor`).then((r) => r.json()).then((d) => {
      setDesc(d);
      setConn((prev) => ({ ...materializeSchemaDefaults(d.connection), ...prev }));
      setProbe(null);
      setIssues([]);
    }).catch(() => setDesc(null));
  }, [driverId, open]);

  const openFresh = () => {
    setDriverId("simulator");
    setConn({});
    form.resetFields();
  };

  // 父页打开时重置（Modal 复用实例，切设备不清旧草稿不可接受）。
  useEffect(() => {
    if (open) openFresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open ]);

  const doAdd = async () => {
    try {
      const v = (await form.validateFields()) as { name: string; id?: string; driver_id: string };
      const payload = buildEndpointCreatePayload({
        deviceId,
        driverId: v.driver_id,
        name: v.name,
        connection: conn,
        id: v.id,
      });
      const r = await api.createEndpoint(payload);
      if (r.status !== 201 && r.status !== 200) {
        message.error(r.body?.error?.message ?? "创建连接失败");
        return;
      }
      message.success(`已添加连接 ${payload.id}（归属 ${deviceId}，未新增设备）`);
      onCreated();
    } catch { /* antd 校验未通过 */ }
  };

  return (
    <Modal title={`新增连接 · 归属 ${deviceId}`} open={open} onOk={doAdd} onCancel={onClose} okText="创建" destroyOnHidden width={640}>
      <Form form={form} layout="vertical" initialValues={{ driver_id: "simulator" }}>
        <Form.Item name="driver_id" label="驱动（创建后不可改）" rules={[{ required: true }]}>
          <Select options={driverOptions} onChange={(v) => { setDriverId(v); setConn({}); }} />
        </Form.Item>
        <Form.Item name="name" label="连接名称" rules={[{ required: true, message: "连接名称必填" }]}>
          <Input placeholder="如 PLC / NCK / OPC UA" />
        </Form.Item>
        <Form.Item name="id" label="连接 ID（可空自动生成，与设备 ID 独立）">
          <Input placeholder={suggestEndpointId(driverId)} style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }} />
        </Form.Item>
        {!desc ? <div style={{ color: "#525252", fontSize: 12 }}>加载连接参数…</div> : (
          <DescriptorFields schema={desc.connection} value={conn} onChange={setConn} />
        )}
        <Space style={{ marginTop: 8 }}>
          <Button onClick={async () => {
            const r = await api.validateConnection(driverId, cleanConnection(conn));
            if (r.status === 200) { setIssues([]); message.success("校验通过"); }
            else { setIssues(r.body.issues ?? []); message.error(r.body?.error?.message ?? "校验失败"); }
          }}>校验</Button>
          <Button onClick={async () => {
            const r = await api.probe(driverId, cleanConnection(conn));
            const ok = !!r.body.reachable;
            setProbe(ok ? { ok: true, msg: "可达" } : { ok: false, msg: r.body.error ?? "不可达" });
            if (ok) message.success("探测可达");
            else message.error(r.body.error ?? "探测不可达");
          }}>探测</Button>
          {probe && <Tag color={probe.ok ? "green" : "red"}>{probe.msg}</Tag>}
        </Space>
        {!!issues.length && <Alert style={{ marginTop: 8 }} type="error" message={issues.map((i) => `${i.path}: ${i.message}`).join("； ")} />}
      </Form>
    </Modal>
  );
}
