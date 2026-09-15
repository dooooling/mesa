// M6 DeviceConfig：设备 Workspace「配置」tab。
// - 设备区：改名 + 删除设备（canDeleteDevice 预检，后端 RESTRICT 为最终裁决）；
// - 连接区：归属本设备的连接表（状态/启停/删除）+ 新增连接 Modal
//   （Descriptor-driven，校验/探测，buildEndpointCreatePayload 组装）；
// - 单连接设置区：当前 ?connection= 上下文的连接 → Connection/Acquisition/
//   EventTask 三个既有 Pane（key={endpointId} 强制 remount，pane 内代际守卫
//   双保险，切连接旧快照绝不污染新窗格）。
import { useEffect, useState } from "react";
import { Alert, Button, Card, Form, Input, Modal, Select, Space, Table, Tag, message } from "antd";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import {
  buildEndpointCreatePayload,
  canDeleteDevice,
  cleanConnection,
  isRunningState,
  suggestEndpointId,
  type Device,
} from "../deviceModel";
import { DescriptorFields, materializeSchemaDefaults } from "../components/DescriptorFields";
import { EndpointAcquisitionPane } from "../components/EndpointAcquisitionPane";
import { EndpointConnectionPane } from "../components/EndpointConnectionPane";
import { EventTaskEditor } from "../components/EventTaskEditor";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";

const FALLBACK_DRIVERS = [
  { value: "simulator", label: "Simulator" },
  { value: "s7", label: "Siemens S7" },
  { value: "focas2", label: "FANUC FOCAS2" },
  { value: "opcua", label: "OPC UA" },
  { value: "sinumerik-nck", label: "SINUMERIK NCK" },
];

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function DeviceConfig({
  deviceId,
  device,
  endpoints,
  effectiveEndpointId,
  onReload,
}: {
  deviceId: string;
  device: Device | null;
  endpoints: WorkspaceEndpoint[];
  /** 当前 ?connection= 上下文（配置类 tab 必单选；null = 无连接）。 */
  effectiveEndpointId: string | null;
  /** 变更后刷新 Workspace 快照（Header 连接数/状态实时跟上）。 */
  onReload: () => void;
}) {
  const nav = useNavigate();
  const active = endpoints.find((e) => e.id === effectiveEndpointId) ?? null;

  // 新增连接
  const [addOpen, setAddOpen] = useState(false);
  const [addForm] = Form.useForm();
  const [driverId, setDriverId] = useState("simulator");
  const [driverOptions, setDriverOptions] = useState(FALLBACK_DRIVERS);
  const [addDesc, setAddDesc] = useState<DriverDescriptor | null>(null);
  const [addConn, setAddConn] = useState<Record<string, unknown>>({});
  const [addIssues, setAddIssues] = useState<Array<{ path: string; message: string }>>([]);
  const [addProbe, setAddProbe] = useState<{ ok: boolean; msg: string } | null>(null);

  // 设备改名
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameForm] = Form.useForm();

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
    if (!addOpen) return;
    fetch(`/api/v1/drivers/${driverId}/descriptor`).then((r) => r.json()).then((d) => {
      setAddDesc(d);
      setAddConn((prev) => ({ ...materializeSchemaDefaults(d.connection), ...prev }));
      setAddProbe(null);
      setAddIssues([]);
    }).catch(() => setAddDesc(null));
  }, [driverId, addOpen]);

  const changed = () => {
    onReload();
  };

  const openAdd = () => {
    setDriverId("simulator");
    setAddConn({});
    addForm.resetFields();
    setAddOpen(true);
  };

  const doAdd = async () => {
    try {
      const v = (await addForm.validateFields()) as { name: string; id?: string; driver_id: string };
      const payload = buildEndpointCreatePayload({
        deviceId,
        driverId: v.driver_id,
        name: v.name,
        connection: addConn,
        id: v.id,
      });
      const r = await api.createEndpoint(payload);
      if (r.status !== 201 && r.status !== 200) {
        message.error(r.body?.error?.message ?? "创建连接失败");
        return;
      }
      message.success(`已添加连接 ${payload.id}（归属 ${deviceId}，未新增设备）`);
      setAddOpen(false);
      changed();
    } catch { /* antd 校验未通过 */ }
  };

  const act = async (id: string, a: "start" | "stop" | "delete") => {
    if (a === "delete") {
      await api.stopEndpoint(id).catch(() => {});
      await sleep(300);
      const r = await api.deleteEndpoint(id);
      if (r.status !== 200) {
        message.error(r.body?.error?.message ?? "删除失败");
        changed();
        return;
      }
      message.success("已删除连接，所属设备保留");
      changed();
      return;
    }
    const r = a === "stop" ? await api.stopEndpoint(id) : await api.startEndpoint(id);
    if (r.status !== 200) {
      message.error(r.body?.error?.message ?? (a === "stop" ? "停止失败" : "启动失败"));
      changed();
      return;
    }
    message.success(a === "stop" ? "已停止" : "已启动");
    changed();
  };

  const confirmDeleteEndpoint = (id: string, name?: string) => {
    Modal.confirm({
      title: `删除连接 ${name ?? id}？`,
      content: "连接删除后其采集/事件配置一并清除，不可恢复；所属设备保留。",
      okText: "删除",
      okType: "danger",
      cancelText: "取消",
      onOk: () => act(id, "delete"),
    });
  };

  const saveRename = async () => {
    try {
      const v = (await renameForm.validateFields()) as { name: string };
      await api.updateDevice(deviceId, { name: v.name.trim() });
      message.success("设备已改名");
      setRenameOpen(false);
      changed();
    } catch (e) {
      const err = e as { status?: number; message?: string };
      if (err?.status) message.error(err?.message ?? "改名失败");
    }
  };

  const removeDevice = () => {
    const pre = canDeleteDevice(deviceId, endpoints.map((e) => ({ id: e.id, driver_id: e.driver_id, device_id: deviceId })));
    if (!pre.ok) {
      message.warning(pre.reason);
      return;
    }
    Modal.confirm({
      title: `删除设备 ${deviceId}？`,
      content: "设备删除后不可恢复；其连接必须已清空（仍有连接时后端会拒绝）。",
      okText: "删除",
      okType: "danger",
      cancelText: "取消",
      onOk: async () => {
        const r = await api.deleteDevice(deviceId);
        if (r.status !== 200) {
          message.error(r.body?.error?.message ?? "删除失败");
          changed();
          return;
        }
        message.success("已删除设备");
        nav("/devices");
      },
    });
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Card
        size="small"
        title={`设备 · ${device?.name ?? deviceId}`}
        extra={
          <Space>
            <Button size="small" onClick={() => { renameForm.setFieldsValue({ name: device?.name ?? "" }); setRenameOpen(true); }}>改名</Button>
            <Button size="small" danger onClick={removeDevice}>删除设备</Button>
            <Button size="small" type="primary" onClick={openAdd}>新增连接</Button>
          </Space>
        }
      >
        <div style={{ fontSize: 12, color: "#525252" }}>下挂 {endpoints.length} 个连接 · 连接的新增/启停/删除收敛于此</div>
      </Card>

      <Card size="small" title="连接">
        <Table
          size="small"
          rowKey="id"
          dataSource={endpoints}
          columns={[
            { title: "名称", dataIndex: "name", render: (v: string) => v ?? "—" },
            { title: "ID", dataIndex: "id", render: (v: string) => <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>{v}</span> },
            { title: "驱动", dataIndex: "driver_id", render: (v: string) => <Tag>{v}</Tag> },
            {
              title: "状态",
              dataIndex: "state",
              render: (v: string) => <Tag color={isRunningState(v) ? "green" : (v ?? "").toUpperCase() === "FAILED" ? "red" : "default"}>{v ?? "—"}</Tag>,
            },
            {
              title: "操作",
              render: (_: unknown, r: WorkspaceEndpoint) => {
                const running = isRunningState(r.state);
                return (
                  <Space onClick={(e) => e.stopPropagation()}>
                    {!running
                      ? <Button size="small" type="primary" onClick={() => act(r.id, "start")}>启动</Button>
                      : <Button size="small" onClick={() => act(r.id, "stop")}>停止</Button>}
                    <Button size="small" danger onClick={() => confirmDeleteEndpoint(r.id, r.name)}>删除</Button>
                  </Space>
                );
              },
            },
          ]}
          locale={{ emptyText: "该设备下暂无连接，点“新增连接”添加第一个连接（不会新增设备）" }}
        />
      </Card>

      {active ? (
        <Card
          size="small"
          title={`连接设置 · ${active.name ?? active.id}`}
          extra={<Tag>{active.driver_id}</Tag>}
        >
          {/* key 强制 remount：切连接时旧窗格（含未保存草稿/快照门）整体丢弃，
              与 pane 内代际守卫双保险。 */}
          <div key={active.id} style={{ display: "grid", gap: 16 }}>
            <EndpointConnectionPane
              endpointId={active.id}
              deviceId={deviceId}
              driverId={active.driver_id}
              initialName={active.name ?? active.id}
              onChanged={changed}
            />
            <EndpointAcquisitionPane endpointId={active.id} driverId={active.driver_id} onChanged={changed} />
            <Card size="small" title="事件订阅">
              <EventTaskEditor fixedEndpointId={active.id} />
            </Card>
          </div>
        </Card>
      ) : (
        <Alert type="warning" showIcon message="该设备暂无连接" description="请先新增连接后再做连接设置。" />
      )}

      <Modal title={`新增连接 · 归属 ${deviceId}`} open={addOpen} onOk={doAdd} onCancel={() => setAddOpen(false)} okText="创建" destroyOnHidden width={640}>
        <Form form={addForm} layout="vertical" initialValues={{ driver_id: "simulator" }}>
          <Form.Item name="driver_id" label="驱动（创建后不可改）" rules={[{ required: true }]}>
            <Select options={driverOptions} onChange={(v) => { setDriverId(v); setAddConn({}); }} />
          </Form.Item>
          <Form.Item name="name" label="连接名称" rules={[{ required: true, message: "连接名称必填" }]}>
            <Input placeholder="如 PLC / NCK / OPC UA" />
          </Form.Item>
          <Form.Item name="id" label="连接 ID（可空自动生成，与设备 ID 独立）">
            <Input placeholder={suggestEndpointId(driverId)} style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }} />
          </Form.Item>
          {!addDesc ? <div style={{ color: "#525252", fontSize: 12 }}>加载连接参数…</div> : (
            <DescriptorFields schema={addDesc.connection} value={addConn} onChange={setAddConn} />
          )}
          <Space style={{ marginTop: 8 }}>
            <Button onClick={async () => {
              const r = await api.validateConnection(driverId, cleanConnection(addConn));
              if (r.status === 200) { setAddIssues([]); message.success("校验通过"); }
              else { setAddIssues(r.body.issues ?? []); message.error(r.body?.error?.message ?? "校验失败"); }
            }}>校验</Button>
            <Button onClick={async () => {
              const r = await api.probe(driverId, cleanConnection(addConn));
              const ok = !!r.body.reachable;
              setAddProbe(ok ? { ok: true, msg: "可达" } : { ok: false, msg: r.body.error ?? "不可达" });
              if (ok) message.success("探测可达");
              else message.error(r.body.error ?? "探测不可达");
            }}>探测</Button>
            {addProbe && <Tag color={addProbe.ok ? "green" : "red"}>{addProbe.msg}</Tag>}
          </Space>
          {!!addIssues.length && <Alert style={{ marginTop: 8 }} type="error" message={addIssues.map((i) => `${i.path}: ${i.message}`).join("； ")} />}
        </Form>
      </Modal>

      <Modal title={`设备改名 · ${deviceId}`} open={renameOpen} onOk={saveRename} onCancel={() => setRenameOpen(false)} okText="保存" destroyOnHidden>
        <Form form={renameForm} layout="vertical">
          <Form.Item name="name" label="设备名称" rules={[{ required: true, message: "设备名称必填" }]}>
            <Input />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
