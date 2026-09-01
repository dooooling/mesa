// 设备管理：侧边系统菜单直连式（选驱动→填连接→设点位），无向导步骤
import { useEffect, useState } from "react";
import type { DriverDescriptor } from "../types";
import { SchemaForm } from "./SchemaForm";
import { ResourcePicker } from "./ResourcePicker";

const DRIVERS = [
  { id: "simulator", name: "Simulator" },
  { id: "s7", name: "Siemens S7" },
  { id: "focas2", name: "FANUC FOCAS2" },
  { id: "opcua", name: "OPC UA" },
];

export function DeviceManager() {
  const [driverId, setDriverId] = useState("simulator");
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [connection, setConnection] = useState<Record<string, unknown>>({});
  const [issues, setIssues] = useState<{ path: string; message: string }[]>([]);
  const [probe, setProbe] = useState<{ ok: boolean; msg: string } | null>(null);
  const [endpoints, setEndpoints] = useState<Array<{ id: string; driver_id: string; state?: string }>>([]);
  const [selections, setSelections] = useState<unknown[]>([]);
  const [msg, setMsg] = useState("");

  const loadDesc = (id: string) => {
    fetch(`/api/v1/drivers/${id}/descriptor`).then((r) => r.json()).then((d) => { setDesc(d); setConnection({}); setIssues([]); setProbe(null); }).catch(() => setDesc(null));
  };
  const loadEndpoints = () => {
    fetch("/api/v1/endpoints").then((r) => r.json()).then((j) => setEndpoints(j.endpoints ?? [])).catch(() => {});
  };

  useEffect(() => { loadDesc(driverId); loadEndpoints(); }, []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { loadDesc(driverId); }, [driverId]);

  const validate = async () => {
    const r = await fetch(`/api/v1/drivers/${driverId}/validate-connection`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ connection }) });
    const j = await r.json();
    if (r.ok) { setIssues([]); setMsg("校验通过"); } else { setIssues(j.issues ?? []); setMsg(j.error?.message ?? "校验失败"); }
  };
  const doProbe = async () => {
    const r = await fetch(`/api/v1/drivers/${driverId}/probe`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ connection }) });
    const j = await r.json();
    setProbe(j.reachable ? { ok: true, msg: "可达" } : { ok: false, msg: `不可达 ${j.error ?? ""}` });
  };

  const createEndpoint = async () => {
    const id = `${driverId}-${Date.now().toString(36)}`;
    const r = await fetch("/api/v1/endpoints", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id, device_id: id, driver_id: driverId, connection }) });
    const j = await r.json();
    if (!r.ok) { setMsg(`${r.status} ${j.error?.message ?? JSON.stringify(j)}`); return; }
    setMsg(`已创建 ${id}`);
    loadEndpoints();
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div className="grid grid-2">
        <div className="card">
          <div className="card-hd"><h3>设备</h3><span className="badge">{endpoints.length} 端点</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 8, maxHeight: 320, overflow: "auto" }}>
            {endpoints.map((e) => (
              <div key={e.id} style={{ display: "flex", gap: 8, alignItems: "center", padding: "8px 10px", border: "1px solid var(--border)", borderRadius: 10, background: "rgba(255,255,255,.03)" }}>
                <span className="mono" style={{ flex: 1, fontSize: 12 }}>{e.id}</span>
                <span className="badge mono">{e.driver_id}</span>
                {e.state && <span className={e.state === "running" ? "badge badge-ok" : "badge"}>{e.state}</span>}
              </div>
            ))}
            {!endpoints.length && <div className="help" style={{ textAlign: "center" }}>暂无设备</div>}
          </div>
        </div>

        <div className="card">
          <div className="card-hd"><h3>新建</h3><span className="help">选驱动 → 填连接 → 设点位</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 10 }}>
            <div>
              <div className="label" style={{ marginBottom: 6 }}>驱动</div>
              <select className="select" value={driverId} onChange={(e) => setDriverId(e.target.value)}>
                {DRIVERS.map((d) => <option key={d.id} value={d.id}>{d.name} — {d.id}</option>)}
              </select>
            </div>
            {desc && <SchemaForm schema={desc.connection} values={connection} onChange={setConnection} issues={issues} />}
            <div style={{ display: "flex", gap: 8 }}>
              <button className="btn btn-ghost btn-sm" onClick={validate}>校验</button>
              <button className="btn btn-ghost btn-sm" onClick={doProbe}>探测</button>
              {probe && <span className={probe.ok ? "badge badge-ok" : "badge badge-bad"}>{probe.msg}</span>}
            </div>
            <button className="btn" onClick={createEndpoint}>创建设备</button>
            {msg && <div className="help">{msg}</div>}
          </div>
        </div>
      </div>

      {desc && (
        <div className="card">
          <div className="card-hd"><h3>点位</h3><span className="help">按资源自渲染</span></div>
          <div className="card-bd">
            <ResourcePicker resources={desc.resources} onAdd={(s) => setSelections((p) => [...p, s])} />
            <div className="help" style={{ marginTop: 8 }}>已选 {selections.length} 项（落库需调用 /tasks，此处仅预览）</div>
          </div>
        </div>
      )}
    </div>
  );
}
