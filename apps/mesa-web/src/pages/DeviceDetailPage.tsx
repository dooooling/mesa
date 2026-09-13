// PR27 Device-first：Device detail 主页面。下挂该 Device 的 Endpoint cards，
// Start / Stop / Edit connection / Configure resources 全部作用于 Endpoint。
// device_id 全页固定来自路由；driver_id 创建后不可改（修改请求无该字段）。
import { useEffect, useState } from "react";
import { Alert, Button, Card, Col, Form, Input, InputNumber, Modal, Row, Select, Space, Tag, message } from "antd";
import { useNavigate, useParams } from "react-router-dom";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import {
  buildEndpointCreatePayload,
  buildEndpointUpdatePayload,
  canDeleteDevice,
  cleanConnection,
  isRunningState,
  type Device,
} from "../deviceModel";
import { DescriptorFields, materializeSchemaDefaults } from "../components/DescriptorFields";
import { ResourcePickerAntd } from "../components/ResourcePickerAntd";

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
  connection?: Record<string, unknown>;
}

type Selection = {
  resource_id: string;
  parameters: Record<string, unknown>;
  outputs: Array<{ output: string; point_key: string }>;
};

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

  // 编辑连接
  const [editOpen, setEditOpen] = useState(false);
  const [editEp, setEditEp] = useState<Endpoint | null>(null);
  const [editDesc, setEditDesc] = useState<DriverDescriptor | null>(null);
  const [editConn, setEditConn] = useState<Record<string, unknown>>({});
  const [editName, setEditName] = useState("");

  // 设备改名
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameForm] = Form.useForm();

  // 点位配置
  const [pointsOpen, setPointsOpen] = useState(false);
  const [pointsEp, setPointsEp] = useState<Endpoint | null>(null);
  const [pointsDesc, setPointsDesc] = useState<DriverDescriptor | null>(null);
  const [pointsSels, setPointsSels] = useState<Selection[]>([]);
  const [intervalMs, setIntervalMs] = useState(1000);

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

  const openEdit = async (ep: Endpoint) => {
    const r = (await fetch(`/api/v1/endpoints/${ep.id}`).then((x) => x.json()).catch(() => null)) as
      | (Endpoint & { connection?: Record<string, unknown> })
      | null;
    if (!r) return message.error("获取连接失败");
    const d = await fetch(`/api/v1/drivers/${r.driver_id ?? ep.driver_id}/descriptor`)
      .then((x) => x.json()).catch(() => null);
    setEditDesc(d);
    // 服务端可能脱敏 Secret 字段；编辑页只改用户填写项，未动字段原样回传由后端合并语义决定
    setEditConn((r.connection as Record<string, unknown>) ?? {});
    setEditEp({ id: r.id ?? ep.id, name: r.name ?? ep.id, driver_id: r.driver_id ?? ep.driver_id, device_id: r.device_id });
    setEditName(r.name ?? r.id ?? ep.id);
    setEditOpen(true);
  };

  const saveEdit = async () => {
    if (!editEp) return;
    const cleaned = cleanConnection(editConn);
    if (!Object.keys(cleaned).length) return message.warning("请填写连接参数");
    await api.stopEndpoint(editEp.id).catch(() => {});
    await sleep(300);
    // driver_id 不可变：请求体无该字段（误带即后端 deny_unknown_fields 拒绝）
    const body = buildEndpointUpdatePayload({ name: editName || editEp.id, deviceId, connection: cleaned });
    try {
      await api.updateEndpoint(editEp.id, body);
    } catch (e) {
      const err = e as { message?: string };
      message.error(err?.message ?? "修改失败");
      return;
    }
    message.success("修改成功（驱动未变，需手动启动）");
    setEditOpen(false);
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

  const openPoints = async (ep: Endpoint) => {
    setPointsEp(ep);
    setPointsSels([]);
    setIntervalMs(1000);
    setPointsOpen(true);
    const r = await fetch(`/api/v1/drivers/${ep.driver_id}/descriptor`).then((x) => x.json()).catch(() => null);
    setPointsDesc(r);
    fetch(`/api/v1/tasks?endpoint=${ep.id}`).then((x) => x.json()).then((j) => {
      const tasks: Array<{ interval_ms?: number; binding: { kind: string; config: { selections?: Selection[] } } }> = j.tasks ?? [];
      if (tasks.length) {
        const first = tasks[0];
        if (first) setIntervalMs(first.interval_ms ?? 1000);
        // 回显只理解 mesa.resources.v1 canonical 形态
        if (first?.binding.kind === "mesa.resources.v1") {
          const sels = first.binding.config?.selections;
          if (sels?.length) setPointsSels(sels);
        }
        message.info(`已回显 ${tasks.length} 任务`);
      }
    }).catch(() => {});
  };

  const savePoints = async () => {
    if (!pointsEp || !pointsSels.length) return message.warning("请先加入点位");
    // 只发 mesa.resources.v1 canonical 形态，合法性由 Core 门禁裁决
    const tasks = [
      {
        id: "t1",
        mode: "poll",
        interval_ms: intervalMs,
        binding: { kind: "mesa.resources.v1", config: { selections: pointsSels } },
      },
    ];
    await api.stopEndpoint(pointsEp.id).catch(() => {});
    const r = await fetch(`/api/v1/tasks/${pointsEp.id}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ tasks }),
    });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) return message.error(j.error?.message ?? "点位保存失败");
    message.success("点位已保存，正在启动…");
    await api.startEndpoint(pointsEp.id);
    setPointsOpen(false);
    load();
  };

  if (notFound) {
    return (
      <Card size="small" title="设备不存在">
        <p style={{ color: "#999" }}>设备 `{deviceId}` 不存在，可能已被删除。</p>
        <Button type="primary" onClick={() => nav("/devices")}>返回设备列表</Button>
      </Card>
    );
  }

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Card
        size="small"
        title={<span>设备 · {device?.name ?? deviceId} <span style={{ fontFamily: "monospace", fontSize: 12, color: "#999" }}>{deviceId}</span></span>}
        extra={
          <Space>
            <Button size="small" onClick={() => nav("/devices")}>返回</Button>
            <Button size="small" onClick={openRename}>改名</Button>
            <Button size="small" danger onClick={removeDevice}>删除设备</Button>
            <Button size="small" type="primary" onClick={openAdd}>新增连接</Button>
          </Space>
        }
      >
        <div style={{ fontSize: 12, color: "#999" }}>下挂 {endpoints.length} 个连接 · 所有操作均作用于 Endpoint，设备本身不直接采集</div>
      </Card>

      <Row gutter={[16, 16]}>
        {endpoints.map((ep) => {
          const running = isRunningState(ep.state);
          return (
            <Col key={ep.id} xs={24} lg={12} xl={8}>
              <Card
                size="small"
                title={<span style={{ fontFamily: "monospace", fontSize: 13 }}>{ep.name ?? ep.id}</span>}
                extra={<Tag color={running ? "green" : (ep.state ?? "").toUpperCase() === "FAILED" ? "red" : "default"}>{ep.state ?? "—"}</Tag>}
              >
                <div style={{ display: "grid", gap: 8 }}>
                  <div style={{ fontSize: 12, color: "#999", fontFamily: "monospace" }}>{ep.id}</div>
                  <div><Tag>{ep.driver_id}</Tag></div>
                  <Space wrap>
                    <Button size="small" onClick={() => openEdit(ep)}>编辑连接</Button>
                    <Button size="small" onClick={() => openPoints(ep)}>配置点位</Button>
                    {!running
                      ? <Button size="small" type="primary" onClick={() => act(ep.id, "start")}>启动</Button>
                      : <Button size="small" onClick={() => act(ep.id, "stop")}>停止</Button>}
                    <Button size="small" danger onClick={() => act(ep.id, "delete")}>删除</Button>
                  </Space>
                </div>
              </Card>
            </Col>
          );
        })}
      </Row>
      {!endpoints.length && (
        <Card size="small"><div style={{ color: "#999", fontSize: 12 }}>该设备下暂无连接，点“新增连接”添加第一个 Endpoint（不会新增设备）。</div></Card>
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
            <Input placeholder={`${driverId}-xxxxxx`} style={{ fontFamily: "monospace" }} />
          </Form.Item>
          {!addDesc ? <div style={{ color: "#999", fontSize: 12 }}>加载连接参数…</div> : (
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

      <Modal title={`编辑连接 · ${editEp?.id ?? ""}`} open={editOpen} onOk={saveEdit} onCancel={() => setEditOpen(false)} okText="保存" width={640} destroyOnHidden={false} forceRender>
        <div style={{ marginBottom: 12, display: "flex", gap: 8, alignItems: "center" }}>
          <span style={{ fontSize: 12 }}>驱动</span>
          <Tag>{editEp?.driver_id}</Tag>
          <span style={{ fontSize: 12, color: "#999" }}>创建后不可改</span>
        </div>
        <div style={{ marginBottom: 12 }}>
          <div style={{ fontSize: 12, marginBottom: 4 }}>连接名称</div>
          <Input value={editName} onChange={(e) => setEditName(e.target.value)} />
        </div>
        {!editDesc ? <div style={{ color: "#999" }}>加载中…</div> : (
          <DescriptorFields schema={editDesc.connection} value={editConn} onChange={setEditConn} />
        )}
        <div style={{ marginTop: 8, fontSize: 12, color: "#999" }}>需先停止再修改（已自动停止），保存后需手动启动</div>
      </Modal>

      <Modal title={`设备改名 · ${deviceId}`} open={renameOpen} onOk={saveRename} onCancel={() => setRenameOpen(false)} okText="保存" destroyOnHidden>
        <Form form={renameForm} layout="vertical">
          <Form.Item name="name" label="设备名称" rules={[{ required: true, message: "设备名称必填" }]}>
            <Input />
          </Form.Item>
        </Form>
      </Modal>

      <Modal title={`点位 · ${pointsEp?.id ?? ""}`} open={pointsOpen} onOk={savePoints} onCancel={() => setPointsOpen(false)} okText="保存并启动" width={720} destroyOnHidden>
        {!pointsDesc ? <div style={{ color: "#999" }}>加载资源…</div> : (
          <>
            <div style={{ marginBottom: 12, display: "flex", gap: 8, alignItems: "center" }}>
              <span style={{ fontSize: 12 }}>采集周期</span>
              <InputNumber min={10} max={60000} step={10} value={intervalMs} onChange={(v) => setIntervalMs(v ?? 1000)} addonAfter="ms" style={{ width: 180 }} />
              <span style={{ fontSize: 12, color: "#999" }}>10ms–60s，20ms已通过50K/s压测</span>
            </div>
            <ResourcePickerAntd
              resources={pointsDesc.resources}
              existingKeys={pointsSels.flatMap((s) => s.outputs.map((o) => o.point_key))}
              onAdd={(s) => {
                const keys = s.outputs.map((o) => o.point_key);
                const dup = pointsSels.some((ex) => ex.outputs.some((o) => keys.includes(o.point_key)));
                if (dup) {
                  message.warning(`point_key 重复：${keys.join(", ")} 已存在`);
                  return false;
                }
                setPointsSels((p) => [...p, s]);
                return true;
              }}
            />
            <div style={{ marginTop: 12, fontSize: 12, color: "#999" }}>已选 {pointsSels.length} 项 · {intervalMs}ms 轮询 · 保存将执行 Stop → PUT /tasks/{pointsEp?.id} → Start <Button size="small" onClick={() => setPointsSels([])} style={{ marginLeft: 8 }}>清空</Button></div>
            {!!pointsSels.length && (
              <div style={{ marginTop: 8, display: "grid", gap: 6 }}>
                {pointsSels.map((s, idx) => (
                  <div key={idx} style={{ display: "flex", gap: 8, alignItems: "center", padding: 6, border: "1px solid #eee", borderRadius: 6 }}>
                    <span style={{ flex: 1, fontFamily: "monospace", fontSize: 11 }}>{s.resource_id} → {s.outputs.map((o) => o.point_key).join(", ")} <span style={{ color: "#999" }}>{JSON.stringify(s.parameters)}</span></span>
                    <Button size="small" danger onClick={() => setPointsSels((p) => p.filter((_, i) => i !== idx))}>移除</Button>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </Modal>
    </div>
  );
}
