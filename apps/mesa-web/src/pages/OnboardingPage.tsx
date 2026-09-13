// PR27 Onboarding：Create Device → Add first Endpoint 两步向导。
// 提交后生成两个独立对象（Device 与 Endpoint 各自 id），绝不塌成
// “一个 Device 对应一个 Driver”。连接表单完全 Descriptor-driven。
import { useEffect, useState } from "react";
import { Alert, Button, Card, Form, Input, Select, Space, Steps, Tag, message } from "antd";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import { buildEndpointCreatePayload, cleanConnection } from "../deviceModel";
import { DescriptorFields, materializeSchemaDefaults } from "../components/DescriptorFields";

const FALLBACK_DRIVERS = [
  { value: "simulator", label: "Simulator" },
  { value: "s7", label: "Siemens S7" },
  { value: "focas2", label: "FANUC FOCAS2" },
  { value: "opcua", label: "OPC UA" },
  { value: "sinumerik-nck", label: "SINUMERIK NCK" },
];

// 驱动默认值的唯一真相来源：React state 与 Form initialValues 都取此处。
// “再建一个”必须同时复位两者，否则下拉显示与实际提交的 driver 会分叉
//（用户看到 Simulator，实际却按上次的 opcua 创建）。
const DEFAULT_DRIVER = "simulator";

export function OnboardingPage() {
  const nav = useNavigate();
  const [step, setStep] = useState(0);
  const [deviceForm] = Form.useForm();
  const [epForm] = Form.useForm();
  const [deviceId, setDeviceId] = useState("");
  const [deviceName, setDeviceName] = useState("");
  const [endpointId, setEndpointId] = useState("");
  const [driverId, setDriverId] = useState(DEFAULT_DRIVER);
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
    if (step !== 1) return;
    fetch(`/api/v1/drivers/${driverId}/descriptor`).then((r) => r.json()).then((d) => {
      setDesc(d);
      setConn((prev) => ({ ...materializeSchemaDefaults(d.connection), ...prev }));
      setProbe(null);
      setIssues([]);
    }).catch(() => setDesc(null));
  }, [driverId, step]);

  const submitDevice = async () => {
    try {
      const v = (await deviceForm.validateFields()) as { id: string; name?: string };
      const id = v.id.trim();
      const name = v.name?.trim() || id;
      const r = await api.createDevice({ id, name });
      if (r.status !== 201 && r.status !== 200) {
        message.error(r.body?.error?.message ?? "创建设备失败");
        return;
      }
      setDeviceId(id);
      setDeviceName(name);
      setStep(1);
    } catch { /* antd 校验未通过 */ }
  };

  // 向导全量复位（“再建一个”）：state 与表单必须同归 DEFAULT_DRIVER，
  // 外加描述/连接/诊断状态清零，不留上一轮的 Descriptor 或校验残留。
  const resetWizard = () => {
    setStep(0);
    setDeviceId("");
    setDeviceName("");
    setEndpointId("");
    setDriverId(DEFAULT_DRIVER);
    deviceForm.resetFields();
    epForm.resetFields();
    setDesc(null);
    setConn({});
    setProbe(null);
    setIssues([]);
  };

  const submitEndpoint = async () => {
    try {
      const v = (await epForm.validateFields()) as { id?: string; name: string };
      const payload = buildEndpointCreatePayload({
        deviceId,
        driverId,
        name: v.name,
        connection: conn,
        id: v.id,
      });
      const r = await api.createEndpoint(payload);
      if (r.status !== 201 && r.status !== 200) {
        // 设备已建好，只停留在本步允许重试，不回滚设备
        message.error(`${r.body?.error?.message ?? "创建连接失败"}（设备 ${deviceId} 已保留，可修正后重试）`);
        return;
      }
      setEndpointId(payload.id);
      setStep(2);
    } catch { /* antd 校验未通过 */ }
  };

  return (
    <div style={{ display: "grid", gap: 16, maxWidth: 760 }}>
      <Card size="small" title="新建向导 · 先建设备，再加连接">
        <Steps
          current={step}
          items={[{ title: "创建设备" }, { title: "添加首个连接" }, { title: "完成" }]}
        />
      </Card>

      {step === 0 && (
        <Card size="small" title="① 创建设备">
          <Form form={deviceForm} layout="vertical">
            <Form.Item name="id" label="设备 ID" rules={[{ required: true, message: "设备 ID 必填" }]}>
              <Input placeholder="device-a" style={{ fontFamily: "monospace" }} />
            </Form.Item>
            <Form.Item name="name" label="设备名称">
              <Input placeholder="默认为 ID" />
            </Form.Item>
          </Form>
          <Space>
            <Button type="primary" onClick={submitDevice}>下一步：添加连接 →</Button>
            <Button onClick={() => nav("/devices")}>返回列表</Button>
          </Space>
        </Card>
      )}

      {step === 1 && (
        <Card size="small" title={`② 添加首个连接 · 归属 ${deviceId}`} extra={<Tag>{deviceName}</Tag>}>
          <Form form={epForm} layout="vertical" initialValues={{ driver_id: DEFAULT_DRIVER }}>
            <Form.Item name="driver_id" label="驱动（创建后不可改）" rules={[{ required: true }]}>
              <Select options={driverOptions} onChange={(v) => { setDriverId(v); setConn({}); }} />
            </Form.Item>
            <Form.Item name="name" label="连接名称" rules={[{ required: true, message: "连接名称必填" }]}>
              <Input placeholder="如 PLC / NCK / OPC UA" />
            </Form.Item>
            <Form.Item name="id" label="连接 ID（可空自动生成，与设备 ID 独立）">
              <Input placeholder={`${driverId}-xxxxxx`} style={{ fontFamily: "monospace" }} />
            </Form.Item>
            {!desc ? <div style={{ color: "#999", fontSize: 12 }}>加载连接参数…</div> : (
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
          {/* 设备已持久化提交，不再伪装可退回未提交的 Step 1；
              需调整设备请到设备详情改名，连接失败本页直接重试。 */}
          <div style={{ fontSize: 12, color: "#999", marginTop: 16, marginBottom: 8 }}>
            设备 {deviceId} 已创建（已提交）。改名请到设备详情，连接失败可修正后重试。
          </div>
          <Space style={{ marginTop: 0 }}>
            <Button onClick={() => nav(`/devices/${deviceId}`)}>进入设备 →</Button>
            <Button type="primary" onClick={submitEndpoint}>完成</Button>
          </Space>
        </Card>
      )}

      {step === 2 && (
        <Card size="small" title="③ 完成 · 两个独立对象已生成">
          <pre style={{ fontFamily: "monospace", fontSize: 12, background: "rgba(0,0,0,.04)", padding: 12, borderRadius: 8 }}>
{`Device ${deviceId}（${deviceName}）
└─ Endpoint ${endpointId}
     └─ Driver ${driverId}`}
          </pre>
          <div style={{ fontSize: 12, color: "#999", marginBottom: 12 }}>
            Device 与 Endpoint id 各自独立；继续加连接请到设备详情（不新增设备）。
          </div>
          <Space>
            <Button type="primary" onClick={() => nav(`/devices/${deviceId}`)}>进入设备 →</Button>
            <Button onClick={resetWizard}>再建一个</Button>
            <Button onClick={() => nav("/devices")}>返回列表</Button>
          </Space>
        </Card>
      )}
    </div>
  );
}
