// Device detail（P1-4 瘦身后）：只负责 Device 身份（改名/删除）与连接列表
//（新增/启停/删除）+ 进入 Endpoint Workspace。连接的编辑/采集/事件/诊断
// 已全部收敛到 EndpointWorkspacePage 的五个 tab，不再堆 Modal。
import { useEffect, useState } from "react";
import { Alert, Button, Card, Form, Input, Modal, Select, Space, Table, Tag, message } from "antd";
import { useNavigate, useParams } from "react-router-dom";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import {
  buildEndpointCreatePayload,
  canDeleteDevice,
  cleanConnection,
  isRunningState,
  type Device,
} from "../deviceModel";
import { DescriptorFields, materializeSchemaDefaults } from "../components/DescriptorFields";

// 后端 discovery 不可用时的兜底（正常情况下拉来自 /api/v1/drivers，
// 新驱动无需改前端即出现）。
const FALLBACK_DRIVERS = [
  { value: "simulator", label: "Simulator" },
  { value: "s7", label: "Siemens S7" },
  { value: "focas2", label: "FANUC FOCAS2" },
  { value: "opcua", label: "OPC UA" },
  { value: "sinumerik-nck", label: "SINUMERIK NCK" },
];

interface Endpoint {
  id: string;
  name?: string;
  driver_id: string;
  device_id?: string;
  state?: string;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function DeviceDetailPage() {
  const { id: deviceId = "" } = useParams();
  const nav = useNavigate();
  const [device, setDevice] = useState<Device | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [endpoints, setEndpoints] = useState<Endpoint[]>([]);

  // 新建连接
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

  const load = async () => {
    try {
      const d = (await api.getDevice(deviceId)) as Device;
      setDevice(d);
      // 同一路由实例从不存在 ID 切到存在 ID 时，清掉旧的 404 view
      setNotFound(false);
    } catch (e) {
      const err = e as { status?: number };
      if (err?.status === 404) setNotFound(true);
      else message.error("加载设备失败");
      return;
    }
    try {
      const j = (await api.listEndpoints()) as { endpoints?: Array<Endpoint & { runtime?: { state?: string } }> };
      const eps = (j.endpoints ?? [])
        .filter((e) => e.device_id === deviceId)
        .map((e) => ({
          id: e.id,
          name: e.name ?? e.id,
          driver_id: e.driver_id,
          device_id: e.device_id,
          state: e.state ?? e.runtime?.state,
        }));
      setEndpoints(eps);
    } catch {
      message.error("加载连接列表失败");
    }
  };
  useEffect(() => { load(); }, [deviceId]); // eslint-disable-line react-hooks/exhaustive-deps

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

  // 新建连接弹窗打开 / 驱动切换时拉取 Descriptor 并物化默认值（显示即保存）
  useEffect(() => {
    if (!addOpen) return;
    fetch(`/api/v1/drivers/${driverId}/descriptor`).then((r) => r.json()).then((d) => {
      setAddDesc(d);
      setAddConn((prev) => ({ ...materializeSchemaDefaults(d.connection), ...prev }));
      setAddProbe(null);
      setAddIssues([]);
    }).catch(() => setAddDesc(null));
  }, [driverId, addOpen]);

  const openAdd = () => {
    setDriverId("simulator");
    setAddConn({});
    addForm.resetFields();
    setAddOpen(true);
  };

  const doAddValidate = async () => {
    const r = await api.validateConnection(driverId, cleanConnection(addConn));
    if (r.status === 200) {
      setAddIssues([]);
      message.success("校验通过");
    } else {
      setAddIssues(r.body.issues ?? []);
      message.error(r.body?.error?.message ?? "校验失败");
    }
  };

  const doAddProbe = async () => {
    const r = await api.probe(driverId, cleanConnection(addConn));
    const ok = !!r.body.reachable;
    setAddProbe(ok ? { ok: true, msg: "可达" } : { ok: false, msg: r.body.error ?? "不可达" });
    if (ok) message.success("探测可达");
    else message.error(r.body.error ?? "探测不可达");
  };

  const doAdd = async () => {
    try {
      const v = (await addForm.validateFields()) as { id?: string; name: string };
      const payload = buildEndpointCreatePayload({
        deviceId,
        driverId,
        name: v.name,
        connection: addConn,
        id: v.id,
      });
      const r = await api.createEndpoint(payload);
      if (r.status !== 201 && r.status !== 200) {
        message.error(r.body?.error?.message ?? "创建连接失败");
        return;
      }
      message.success(`已在 ${deviceId} 下创建连接 ${payload.id}（未新增设备）`);
      setAddOpen(false);
      load();
    } catch { /* antd 校验未通过 */ }
  };

  const act = async (id: string, a: "start" | "stop" | "delete") => {
    if (a === "delete") {
      await api.stopEndpoint(id).catch(() => {});
      await sleep(300);
      const r = await api.deleteEndpoint(id);
      if (r.status !== 200) {
        message.error(r.body?.error?.message ?? "删除失败");
        load();
        return;
      }
      // 删除 Endpoint 后 Device 仍存在（由 load 刷新连接列表佐证）
      message.success("已删除连接，所属设备保留");
      load();
      return;
    }
    if (a === "stop") {
      const r = await api.stopEndpoint(id);
      if (r.status !== 200) {
        message.error(r.body?.error?.message ?? "停止失败");
        load();
        return;
      }
      message.success("已停止");
      load();
      return;
    }
    const r = await api.startEndpoint(id);
    if (r.status !== 200) {
      message.error(r.body?.error?.message ?? "启动失败");
      load();
      return;
    }
    message.success("已启动");
    load();
  };

  const openRename = () => {
    renameForm.setFieldsValue({ name: device?.name ?? "" });
    setRenameOpen(true);
  };

  const saveRename = async () => {
    try {
      const v = (await renameForm.validateFields()) as { name: string };
      await api.updateDevice(deviceId, { name: v.name.trim() });
      message.success("设备已改名");
      setRenameOpen(false);
      load();
    } catch (e) {
      const err = e as { status?: number; message?: string };
      // antd 校验失败时没有 status，直接吞掉；服务端错误才提示
      if (err?.status) message.error(err?.message ?? "改名失败");
    }
  };

  const removeDevice = async () => {
    const pre = canDeleteDevice(deviceId, endpoints.map((e) => ({ ...e, device_id: deviceId })));
    if (!pre.ok) {
      message.warning(pre.reason);
      return;
    }
    const r = await api.deleteDevice(deviceId);
    if (r.status !== 200) {
      message.error(r.body?.error?.message ?? "删除失败");
      load();
      return;
    }
    message.success("已删除设备");
    nav("/devices");
  };

  if (notFound) {
    return (
      <Card size="small" title="设备不存在">
        <p style={{ color: "#525252" }}>设备 `{deviceId}` 不存在，可能已被删除。</p>
        <Button type="primary" onClick={() => nav("/devices")}>返回设备列表</Button>
      </Card>
    );
  }

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Card
        size="small"
        title={<span>设备 · {device?.name ?? deviceId} <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, color: "#525252" }}>{deviceId}</span></span>}
        extra={
          <Space>
            <Button size="small" onClick={() => nav("/devices")}>返回</Button>
            <Button size="small" onClick={openRename}>改名</Button>
            <Button size="small" danger onClick={removeDevice}>删除设备</Button>
            <Button size="small" type="primary" onClick={openAdd}>新增连接</Button>
          </Space>
        }
      >
        <div style={{ fontSize: 12, color: "#525252" }}>下挂 {endpoints.length} 个连接 · 点行进入 Endpoint 工作空间</div>
      </Card>

      <Card size="small" title="连接">
        <Table
          size="small"
          rowKey="id"
          dataSource={endpoints}
          onRow={(r) => ({ onClick: () => nav(`/devices/${deviceId}/endpoints/${r.id}`), style: { cursor: "pointer" } })}
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
              render: (_: unknown, r: Endpoint) => {
                const running = isRunningState(r.state);
                return (
                  <Space onClick={(e) => e.stopPropagation()}>
                    <Button size="small" onClick={() => nav(`/devices/${deviceId}/endpoints/${r.id}`)}>进入</Button>
                    {!running
                      ? <Button size="small" type="primary" onClick={() => act(r.id, "start")}>启动</Button>
                      : <Button size="small" onClick={() => act(r.id, "stop")}>停止</Button>}
                    <Button size="small" danger onClick={() => act(r.id, "delete")}>删除</Button>
                  </Space>
                );
              },
            },
          ]}
          locale={{ emptyText: "该设备下暂无连接，点“新增连接”添加第一个 Endpoint（不会新增设备）" }}
        />
      </Card>

      <Modal title={`新增连接 · 归属 ${deviceId}`} open={addOpen} onOk={doAdd} onCancel={() => setAddOpen(false)} okText="创建" destroyOnHidden width={640}>
        <Form form={addForm} layout="vertical" initialValues={{ driver_id: "simulator" }}>
          <Form.Item name="driver_id" label="驱动（创建后不可改）" rules={[{ required: true }]}>
            <Select options={driverOptions} onChange={(v) => { setDriverId(v); setAddConn({}); }} />
          </Form.Item>
          <Form.Item name="name" label="连接名称" rules={[{ required: true, message: "连接名称必填" }]}>
            <Input placeholder="如 PLC / NCK / OPC UA" />
          </Form.Item>
          <Form.Item name="id" label="连接 ID（可空自动生成，与设备 ID 独立）">
            <Input placeholder={`${driverId}-xxxxxx`} style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }} />
          </Form.Item>
          {!addDesc ? <div style={{ color: "#525252", fontSize: 12 }}>加载连接参数…</div> : (
            <DescriptorFields schema={addDesc.connection} value={addConn} onChange={setAddConn} />
          )}
          <Space style={{ marginTop: 8 }}>
            <Button onClick={doAddValidate}>校验</Button>
            <Button onClick={doAddProbe}>探测</Button>
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
