// 通用 DeviceWizard：Vendor/Profile → Connection → Validate → Probe → Resource → Review → Save（V2.1 §21.2）
// 仅依赖 Descriptor，不含 driverId 分支
import { useEffect, useState } from "react";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import { SchemaForm } from "./SchemaForm";
import { ResourcePicker } from "./ResourcePicker";

export function DeviceWizard() {
  const [drivers, setDrivers] = useState<{ id: string; name: string }[]>([]);
  const [driverId, setDriverId] = useState("");
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [connection, setConnection] = useState<Record<string, unknown>>({});
  const [issues, setIssues] = useState<{ path: string; message: string }[]>([]);
  const [probe, setProbe] = useState<string>("");
  const [selections, setSelections] = useState<unknown[]>([]);

  useEffect(() => {
    api.listDrivers().then((j) => setDrivers(j.drivers ?? j ?? [])).catch(() => {});
  }, []);

  useEffect(() => {
    if (!driverId) return;
    api.getDescriptor(driverId).then((d) => { setDesc(d); setConnection({}); setIssues([]); }).catch(() => setDesc(null));
  }, [driverId]);

  const validate = async () => {
    const r = await api.validateConnection(driverId, connection);
    if (r.status === 200) { setIssues([]); setProbe("校验通过"); }
    else { setIssues(r.body.issues ?? []); setProbe(`校验失败: ${r.body.error?.message ?? ""}`); }
  };

  const doProbe = async () => {
    const r = await api.probe(driverId, connection);
    setProbe(r.body.reachable ? "可达" : `不可达: ${r.body.error ?? ""}`);
  };

  return (
    <div style={{ maxWidth: 720, margin: "0 auto", padding: 16 }}>
      <h2>添加设备（通用向导）</h2>

      <label>
        驱动
        <select value={driverId} onChange={(e) => setDriverId(e.target.value)} style={{ marginLeft: 8 }}>
          <option value="">选择驱动</option>
          {drivers.map((d) => (
            <option key={d.id} value={d.id}>
              {d.name} ({d.id})
            </option>
          ))}
        </select>
      </label>

      {desc && (
        <>
          <h3>连接参数</h3>
          <SchemaForm schema={desc.connection} values={connection} onChange={setConnection} issues={issues} />
          <button onClick={validate} style={{ marginRight: 8 }}>校验</button>
          <button onClick={doProbe}>探测</button>
          {probe && <div style={{ marginTop: 8 }}>{probe}</div>}

          <h3>数据选择</h3>
          <ResourcePicker resources={desc.resources} onAdd={(s) => setSelections([...selections, s])} />
          <pre>{JSON.stringify(selections, null, 2)}</pre>
          <div>已选 {selections.length} 项</div>
        </>
      )}
    </div>
  );
}
