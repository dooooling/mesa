// M4.2 AddDeviceFlow：替换旧 Onboarding（路由 /devices/new，唯一入口“+ 添加设备”）。
// - 设备 → 连接 → 数据 → 确认四步，前三步只攒 draft，最后一步才编排提交；
// - 连接表单 Descriptor-driven（校验/探测复用 api）；数据步用 ResourcePickerAntd
//  （manual 选择；browse 需 endpoint 已存在，M4.2 暂只 manual——备注说明）；
// - 提交走 bootstrapDevice（create→endpoint→tasks→start，失败反向补偿，
//   补偿失败明示残留）；成功进 Device Workspace。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Checkbox, Form, Input, InputNumber, Select, Space, Steps, Tag, message } from "antd";
import { useNavigate } from "react-router-dom";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import {
  buildEndpointCreatePayload,
  cleanConnection,
  suggestEndpointId,
  type ResourceSelection,
} from "../deviceModel";
import { DescriptorFields, materializeSchemaDefaults } from "../components/DescriptorFields";
import { ResourcePickerAntd } from "../components/ResourcePickerAntd";
import { bootstrapDevice, isDraftComplete, newOperationKey, type AcquisitionDraft, type BootstrapReport, type ConnectionDraft, type DeviceDraft } from "./bootstrap";

const FALLBACK_DRIVERS = [
  { value: "simulator", label: "Simulator" },
  { value: "s7", label: "Siemens S7" },
  { value: "focas2", label: "FANUC FOCAS2" },
  { value: "opcua", label: "OPC UA" },
];

export function AddDeviceFlow() {
  const nav = useNavigate();
  const [step, setStep] = useState(0);
  const [deviceForm] = Form.useForm();
  const [connForm] = Form.useForm();
  const [device, setDevice] = useState<DeviceDraft | null>(null);
  const [connection, setConnection] = useState<ConnectionDraft | null>(null);
  const [acquisition, setAcquisition] = useState<AcquisitionDraft | null>(null);

  // 连接步状态（Descriptor-driven，与 Onboarding/DeviceDetail 同源逻辑）
  const [driverId, setDriverId] = useState("simulator");
  const [driverOptions, setDriverOptions] = useState(FALLBACK_DRIVERS);
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [conn, setConn] = useState<Record<string, unknown>>({});
  const [connName, setConnName] = useState("");
  const [connId, setConnId] = useState("");
  const [probe, setProbe] = useState<{ ok: boolean; msg: string } | null>(null);
  const [issues, setIssues] = useState<Array<{ path: string; message: string }>>([]);

  // 数据步状态
  const [sels, setSels] = useState<ResourceSelection[]>([]);
  const [intervalMs, setIntervalMs] = useState(1000);
  const [startAfterCreate, setStartAfterCreate] = useState(true);

  // 提交状态
  const [submitting, setSubmitting] = useState(false);
  const [report, setReport] = useState<BootstrapReport | null>(null);
  // RC2 修3：operation key（per-operation 幂等）。进入确认页生成一次，
  // 同一 Flow 内重复提交（重试/双击）复用；reset（再建一个）即新操作新 key。
  const [operationKey, setOperationKey] = useState("");

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

  const draftComplete = useMemo(
    () => isDraftComplete({ device, connection, acquisition }),
    [device, connection, acquisition],
  );

  const commitDevice = async () => {
    try {
      const v = (await deviceForm.validateFields()) as { id: string; name?: string };
      const id = v.id.trim();
      setDevice({ deviceId: id, deviceName: v.name?.trim() || id });
      setStep(1);
    } catch { /* 校验未通过 */ }
  };

  const commitConnection = async () => {
    try {
      const v = (await connForm.validateFields()) as { name?: string; id?: string };
      const payload = buildEndpointCreatePayload({
        deviceId: device?.deviceId ?? "",
        driverId,
        name: (v.name ?? connName).trim() || "connection",
        connection: cleanConnection(conn),
        id: (v.id ?? connId).trim() || undefined,
      });
      setConnection({
        driverId,
        endpointId: payload.id,
        endpointName: payload.name,
        connection: payload.connection,
      });
      setConnId(payload.id);
      setStep(2);
    } catch { /* 校验未通过 */ }
  };

  const commitAcquisition = () => {
    if (!sels.length) {
      message.warning("请至少选择一个数据点");
      return;
    }
    setAcquisition({ intervalMs, selections: sels, startAfterCreate });
    setOperationKey(newOperationKey());
    setStep(3);
  };

  const submit = async () => {
    if (!device || !connection || !acquisition) return;
    setSubmitting(true);
    setReport(null);
    try {
      // M5.5：一次原子提交（后端单事务 + 幂等；前端不再分步编排/补偿）。
      // RC2 修3：复用本 Flow 的 operationKey（重试同 key → 重放，不双建）。
      const r = await bootstrapDevice(
        {
          deviceBootstrap: async (b) => {
            const res = await fetch("/api/v1/device-bootstrap", {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify(b),
            });
            const body = await res.json().catch(() => ({}));
            return { status: res.status, body };
          },
        },
        { device, connection, acquisition },
        { idempotencyKey: operationKey || undefined },
      );
      setReport(r);
      if (r.ok) {
        message.success(r.replayed ? `设备 ${r.deviceId} 已存在（幂等重放，未重复创建）` : `已添加设备 ${r.deviceId}`);
      } else if (r.residual) {
        message.error("创建失败，可能存在残留设备/连接，请到设备列表检查");
      } else {
        message.error(`${r.failedMessage ?? "创建失败"}（已自动回滚）`);
      }
    } finally {
      setSubmitting(false);
    }
  };

  const reset = () => {
    setStep(0);
    setDevice(null);
    setConnection(null);
    setAcquisition(null);
    setSels([]);
    setReport(null);
    setProbe(null);
    setIssues([]);
    // 新操作：旧 operationKey 作废（删除后重建相同配置必须真正执行，
    // 不能命中旧 replay）。
    setOperationKey("");
    deviceForm.resetFields();
    connForm.resetFields();
  };

  return (
    <div style={{ display: "grid", gap: 16, maxWidth: 760 }}>
      <Card size="small" title="添加设备">
        <Steps
          current={step}
          items={[{ title: "设备" }, { title: "连接" }, { title: "数据" }, { title: "确认" }]}
        />
        <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
          前三步只构建草稿，最后一步才真正创建；任一步失败自动回滚。
        </div>
      </Card>

      {step === 0 && (
        <Card size="small" title="① 设备">
          <Form form={deviceForm} layout="vertical">
            <Form.Item name="id" label="名称 *" rules={[{ required: true, message: "设备 ID 必填" }]}>
              <Input placeholder="CNC-01" style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }} />
            </Form.Item>
            <Form.Item name="name" label="显示名">
              <Input placeholder="默认为 ID" />
            </Form.Item>
          </Form>
          <Space>
            <Button type="primary" onClick={commitDevice}>下一步：连接 →</Button>
            <Button onClick={() => nav("/devices")}>返回列表</Button>
          </Space>
        </Card>
      )}

      {step === 1 && (
        <Card size="small" title="② 连接" extra={<Tag>{device?.deviceId}</Tag>}>
          <Form form={connForm} layout="vertical" initialValues={{ driver_id: driverId }}>
            <Form.Item name="driver_id" label="驱动（创建后不可改）" rules={[{ required: true }]}>
              <Select options={driverOptions} onChange={(v) => { setDriverId(v); setConn({}); }} />
            </Form.Item>
            <Form.Item name="name" label="连接名称">
              <Input placeholder="如 FOCAS / OPC UA" value={connName} onChange={(e) => setConnName(e.target.value)} />
            </Form.Item>
            <Form.Item name="id" label="连接 ID（可空自动生成）">
              <Input
                placeholder={suggestEndpointId(driverId)}
                value={connId}
                onChange={(e) => setConnId(e.target.value)}
                style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}
              />
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
                setProbe(ok ? { ok: true, msg: "连接成功" } : { ok: false, msg: r.body.error ?? "不可达" });
                if (ok) message.success("连接成功");
                else message.error(r.body.error ?? "探测不可达");
              }}>测试连接</Button>
              {probe && <Tag color={probe.ok ? "green" : "red"}>{probe.msg}</Tag>}
            </Space>
            {!!issues.length && <Alert style={{ marginTop: 8 }} type="error" message={issues.map((i) => `${i.path}: ${i.message}`).join("； ")} />}
          </Form>
          <Space style={{ marginTop: 16 }}>
            <Button onClick={() => setStep(0)}>上一步</Button>
            <Button type="primary" onClick={commitConnection}>下一步：数据 →</Button>
          </Space>
        </Card>
      )}

      {step === 2 && (
        <Card size="small" title="③ 数据">
          {!desc ? (
            <div style={{ fontSize: 12, color: "#525252" }}>驱动描述缺失，无法选择数据点。请返回上一步确认驱动。</div>
          ) : (
            <>
              <ResourcePickerAntd
                resources={(desc.resources ?? []) as never[]}
                existingSelections={sels as never}
                selectionMethods={(desc.resource_selection_methods ?? ["manual"]) as never}
                onAdd={(s) => {
                  setSels((cur) => [...cur, s as ResourceSelection]);
                  return true;
                }}
              />
              <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
                浏览设备（browse）需连接已存在，M4.2 暂只支持手动添加；已选 {sels.length} 项。
              </div>
              {sels.length > 0 && (
                <div style={{ marginTop: 8, display: "grid", gap: 4 }}>
                  {sels.map((s, i) => (
                    <div key={i} style={{ fontSize: 12, fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                      {s.resource_id}
                      <Button size="small" type="link" danger onClick={() => setSels((cur) => cur.filter((_, j) => j !== i))}>移除</Button>
                    </div>
                  ))}
                </div>
              )}
              <div style={{ marginTop: 12, display: "flex", gap: 12, alignItems: "center" }}>
                <span style={{ fontSize: 12 }}>采集周期</span>
                <InputNumber min={100} value={intervalMs} onChange={(v) => setIntervalMs(typeof v === "number" ? v : 1000)} />
                <span style={{ fontSize: 12 }}>ms</span>
                <Checkbox checked={startAfterCreate} onChange={(e) => setStartAfterCreate(e.target.checked)}>
                  创建后启动
                </Checkbox>
              </div>
            </>
          )}
          <Space style={{ marginTop: 16 }}>
            <Button onClick={() => setStep(1)}>上一步</Button>
            <Button type="primary" onClick={commitAcquisition}>下一步：确认 →</Button>
          </Space>
        </Card>
      )}

      {step === 3 && (
        <Card size="small" title="④ 确认 · 最终按钮才真正提交">
          <pre style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, background: "#f4f4f4", padding: 12 }}>
{`${device?.deviceId}（${device?.deviceName}）
└─ ${connection?.endpointName} [${connection?.driverId}]
   └─ ${acquisition?.selections.length} 个数据点 · ${acquisition?.intervalMs}ms${acquisition?.startAfterCreate ? " · 创建后启动" : ""}`}
          </pre>
          {!draftComplete && (
            <Alert type="warning" showIcon message="草稿不完整" description="请返回前面步骤补齐设备、连接与数据选择。" />
          )}
          {report && !report.ok && (
            <Alert
              type={report.residual ? "error" : "warning"}
              showIcon
              style={{ marginBottom: 12 }}
              message={report.residual ? "创建失败，可能存在残留" : "创建失败，已自动回滚"}
              description={[
                `失败步骤：${report.failedStep}（${report.failedMessage ?? ""}）`,
                report.compensated ? "后端已补偿删除未启动的设备/连接。" : null,
                report.residual ? "请到设备列表检查。" : null,
              ].filter(Boolean).join("；")}
            />
          )}
          <Space>
            <Button onClick={() => setStep(2)}>上一步</Button>
            <Button type="primary" loading={submitting} disabled={!draftComplete} onClick={submit}>
              添加设备
            </Button>
            {report?.ok && (
              <Button type="primary" onClick={() => nav(`/devices/${report.deviceId}/overview`)}>
                进入设备 →
              </Button>
            )}
            <Button onClick={reset}>再建一个</Button>
            <Button onClick={() => nav("/devices")}>返回列表</Button>
          </Space>
        </Card>
      )}
    </div>
  );
}
