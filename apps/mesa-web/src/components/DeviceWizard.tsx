// 通用 DeviceWizard：Profile → Connection → Validate/Probe → ResourcePicker（V2.1 §10/§21，无 driverId 分支）
import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import { SchemaForm } from "./SchemaForm";
import { ResourcePicker } from "./ResourcePicker";

interface Profile {
  id: string;
  vendor: string;
  family: string;
  model: string;
  driver_id: string;
  connection_defaults: Record<string, unknown>;
  presets: { id: string; label: { default: string; "zh-CN"?: string }; selections: unknown[] }[];
}

export function DeviceWizard() {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [profileId, setProfileId] = useState("");
  const [driverId, setDriverId] = useState("");
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [connection, setConnection] = useState<Record<string, unknown>>({});
  const [issues, setIssues] = useState<{ path: string; message: string }[]>([]);
  const [probe, setProbe] = useState<{ ok: boolean; msg: string } | null>(null);
  const [valid, setValid] = useState<boolean | null>(null);
  const [selections, setSelections] = useState<unknown[]>([]);
  const [step, setStep] = useState(1);

  useEffect(() => { api.listProfiles().then((j) => setProfiles(j.profiles ?? [])).catch(() => {}); }, []);
  const selectedProfile = useMemo(() => profiles.find((p) => p.id === profileId) ?? null, [profiles, profileId]);

  useEffect(() => {
    const id = selectedProfile?.driver_id ?? driverId;
    if (!id) return;
    api.getDescriptor(id).then((d) => {
      setDesc(d);
      setConnection(selectedProfile?.connection_defaults ?? {});
      setIssues([]);
      setProbe(null);
      setValid(null);
      setStep(2);
    }).catch(() => setDesc(null));
  }, [selectedProfile, driverId]);

  const progress = step === 1 ? 22 : step === 2 ? 58 : 92;

  const validate = async () => {
    const id = selectedProfile?.driver_id ?? driverId;
    const r = await api.validateConnection(id, connection);
    if (r.status === 200) { setIssues([]); setValid(true); } else { setIssues(r.body.issues ?? []); setValid(false); }
  };
  const doProbe = async () => {
    const id = selectedProfile?.driver_id ?? driverId;
    const r = await api.probe(id, connection);
    setProbe(r.body.reachable ? { ok: true, msg: "可达 — 连接正常" } : { ok: false, msg: `不可达：${r.body.error ?? ""}` });
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div className="card">
        <div className="card-bd">
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span className="badge">通用向导</span>
            <span className="help">Profile 优先 · 一切以 Descriptor 为准</span>
            <span style={{ marginLeft: "auto" }} className="kbd">Step {step}/3</span>
          </div>
          <div className="stepper" style={{ marginTop: 12 }}><span className="step"><i style={{ width: `${progress}%` }} /></span></div>
          <div className="step-labels"><span>① 选择型号</span><span>② 连接参数</span><span>③ 数据选择</span></div>
        </div>
      </div>

      <div className="grid grid-2">
        <div className="card">
          <div className="card-hd"><h3>① 设备型号</h3><span className="badge">{profiles.length} profiles</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 12 }}>
            <div>
              <div className="label" style={{ marginBottom: 6 }}>Profile（厂商/型号）</div>
              <select className="select" value={profileId} onChange={(e) => { setProfileId(e.target.value); if (e.target.value) setDriverId(""); }}>
                <option value="">选择型号（或直接选驱动）</option>
                {profiles.map((p) => <option key={p.id} value={p.id}>{p.vendor} {p.family} {p.model} — {p.id}</option>)}
              </select>
            </div>
            {!profileId && (
              <div>
                <div className="label" style={{ marginBottom: 6 }}>驱动（无 Profile 时）</div>
                <select className="select" value={driverId} onChange={(e) => setDriverId(e.target.value)}>
                  <option value="">选择驱动</option>
                  <option value="simulator">Mesa Simulator — simulator</option>
                  <option value="s7">Siemens S7 — s7</option>
                  <option value="focas2">FANUC FOCAS2 — focas2</option>
                  <option value="opcua">OPC UA — opcua</option>
                </select>
              </div>
            )}
            {selectedProfile && (
              <div className="card" style={{ background: "rgba(34,211,238,.08)", borderColor: "rgba(34,211,238,.22)" }}>
                <div className="card-bd" style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                  <strong style={{ fontSize: 13 }}>推荐数据</strong>
                  {selectedProfile.presets.map((pr) => (
                    <button key={pr.id} className="btn btn-ghost btn-sm" onClick={() => setSelections((s) => [...s, ...pr.selections])}>
                      + {pr.label["zh-CN"] ?? pr.label.default}
                    </button>
                  ))}
                  <span className="help">一键加入</span>
                </div>
              </div>
            )}
          </div>
        </div>

        <div className="card">
          <div className="card-hd"><h3>状态</h3><span className={valid === null ? "badge" : valid ? "badge badge-ok" : "badge badge-bad"}>{valid === null ? "待校验" : valid ? "校验通过" : "校验失败"}</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 10 }}>
            <div style={{ display: "flex", gap: 8 }}>
              <span className="badge">已选 {selections.length} 项</span>
              <span className="badge mono">{selectedProfile?.driver_id ?? driverId ?? "—"}</span>
              {probe && <span className={probe.ok ? "badge badge-ok" : "badge badge-bad"}>{probe.msg}</span>}
            </div>
            <div className="card" style={{ background: "rgba(0,0,0,.18)" }}>
              <div className="card-bd scroll" style={{ maxHeight: 160 }}>
                <pre className="mono" style={{ margin: 0, fontSize: 12, whiteSpace: "pre-wrap" }}>{selections.length ? JSON.stringify(selections, null, 2) : "尚未选择数据，前往第 ③ 步"}</pre>
              </div>
            </div>
          </div>
        </div>
      </div>

      {desc && (
        <>
          <div className="card">
            <div className="card-hd">
              <h3>② 连接参数</h3>
              <span className="help">{selectedProfile ? `已填入 ${selectedProfile.id} 默认值` : desc.identity.name}</span>
            </div>
            <div className="card-bd">
              <SchemaForm schema={desc.connection} values={connection} onChange={setConnection} issues={issues} />
              <div style={{ display: "flex", gap: 10, marginTop: 12 }}>
                <button className="btn" onClick={validate}>校验</button>
                <button className="btn btn-ghost" onClick={doProbe}>探测（6s）</button>
                <button className="btn btn-ghost" onClick={() => setStep(3)}>下一步 → 数据选择</button>
              </div>
              {probe && <div className={probe.ok ? "badge badge-ok" : "badge badge-bad"} style={{ marginTop: 10, display: "inline-flex" }}>{probe.msg}</div>}
            </div>
          </div>

          <div className="card">
            <div className="card-hd"><h3>③ 数据选择</h3><span className="help">按 Descriptor 资源自渲染</span></div>
            <div className="card-bd">
              <ResourcePicker resources={desc.resources} onAdd={(s) => setSelections((prev) => [...prev, s])} />
            </div>
          </div>
        </>
      )}
    </div>
  );
}
