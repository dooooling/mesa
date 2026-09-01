// 直观向导：选驱动 → 连设备 → 设点位 → 看数据（V2.1 无 driverId 分支，一切以 Descriptor 为准）
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

const DRIVERS = [
  { id: "simulator", name: "Simulator", sub: "本地仿真 · 零硬件", icon: "◐" },
  { id: "s7", name: "Siemens S7", sub: "S7-1200/1500 · DB/M/I/Q", icon: "⬢" },
  { id: "focas2", name: "FANUC FOCAS2", sub: "0i-F · PMC/动态/宏变量", icon: "⬣" },
  { id: "opcua", name: "OPC UA", sub: "订阅/轮询 · 证书安全", icon: "⬔" },
];

export function DeviceWizard() {
  const [step, setStep] = useState<1 | 2 | 3 | 4>(1);
  const [driverId, setDriverId] = useState("");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [profileId, setProfileId] = useState("");
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [connection, setConnection] = useState<Record<string, unknown>>({});
  const [issues, setIssues] = useState<{ path: string; message: string }[]>([]);
  const [probe, setProbe] = useState<{ ok: boolean; msg: string } | null>(null);
  const [valid, setValid] = useState<boolean | null>(null);
  const [selections, setSelections] = useState<unknown[]>([]);
  const [live, setLive] = useState<{ points: Array<{ endpoint_id: string; point_key: string; point_id: number; value: { type: string; value: unknown }; quality: string; timestamp_ns: number }> } | null>(null);

  useEffect(() => { api.listProfiles().then((j) => setProfiles(j.profiles ?? [])).catch(() => {}); }, []);

  const selectedProfile = useMemo(() => profiles.find((p) => p.id === profileId) ?? null, [profiles, profileId]);
  const driverProfiles = useMemo(() => profiles.filter((p) => p.driver_id === driverId), [profiles, driverId]);

  // 选驱动后拉取 Descriptor 并填入 Profile 默认值
  useEffect(() => {
    if (!driverId) return;
    api.getDescriptor(driverId).then((d) => {
      setDesc(d);
      setConnection(selectedProfile?.connection_defaults ?? {});
      setIssues([]); setProbe(null); setValid(null);
    }).catch(() => setDesc(null));
    // 切换驱动时重置 profile 选择
    if (selectedProfile && selectedProfile.driver_id !== driverId) setProfileId("");
  }, [driverId]); // eslint-disable-line react-hooks/exhaustive-deps

  // 切换 Profile 时带入默认值
  useEffect(() => {
    if (selectedProfile && driverId === selectedProfile.driver_id) {
      setConnection(selectedProfile.connection_defaults ?? {});
    }
  }, [selectedProfile, driverId]);

  // 轮询最新值
  useEffect(() => {
    if (step !== 4) return;
    let alive = true;
    const tick = async () => {
      try { const j = await fetch("/api/v1/points/latest").then((r) => r.json()); if (alive) setLive(j); } catch { /* ignore */ }
    };
    tick();
    const id = window.setInterval(tick, 1000);
    return () => { alive = false; window.clearInterval(id); };
  }, [step]);

  const validate = async () => {
    const r = await api.validateConnection(driverId, connection);
    if (r.status === 200) { setIssues([]); setValid(true); } else { setIssues(r.body.issues ?? []); setValid(false); }
  };
  const doProbe = async () => {
    const r = await api.probe(driverId, connection);
    setProbe(r.body.reachable ? { ok: true, msg: "可达 — 连接正常" } : { ok: false, msg: `不可达：${r.body.error ?? ""}` });
  };

  const canNext2 = !!driverId;
  const canNext3 = valid !== false && (probe?.ok || valid === true || driverId === "simulator"); // 仿真无需探测
  const steps = [
    { n: 1, t: "选驱动" },
    { n: 2, t: "连设备" },
    { n: 3, t: "设点位" },
    { n: 4, t: "看数据" },
  ] as const;

  return (
    <div style={{ display: "grid", gap: 16 }}>
      {/* 顶部步骤条 */}
      <div className="card">
        <div className="card-bd">
          <div style={{ display: "flex", gap: 10, overflow: "auto" }}>
            {steps.map((s) => (
              <button
                key={s.n}
                onClick={() => { if (s.n <= step) setStep(s.n as never); if (s.n === 2 && !driverId) return; if (s.n === 3 && !desc) return; }}
                style={{
                  flex: 1, minWidth: 120, padding: "10px 12px", borderRadius: 10,
                  border: step === s.n ? "1px solid rgba(34,211,238,.45)" : "1px solid var(--border)",
                  background: step === s.n ? "rgba(34,211,238,.14)" : step > s.n ? "rgba(34,197,94,.10)" : "rgba(255,255,255,.04)",
                  color: step === s.n ? "#e6faff" : "var(--text)", fontWeight: 700, cursor: "pointer"
                }}
              >
                <span style={{ fontSize: 11, color: "var(--muted)" }}>STEP {s.n}</span>
                <span style={{ display: "block", fontSize: 13 }}>{(step > s.n ? "✓ " : "") + s.t}</span>
              </button>
            ))}
          </div>
          <div className="stepper" style={{ marginTop: 12 }}><span className="step"><i style={{ width: `${(step / 4) * 100}%` }} /></span></div>
          <div className="help">线性流程：选好驱动 → 连接设备 → 设置点位 → 立即看数</div>
        </div>
      </div>

      {/* Step 1 选驱动 */}
      {step === 1 && (
        <div className="card">
          <div className="card-hd"><h3>① 选择驱动</h3><span className="help">按设备类型选择，一切能力由驱动自描述</span></div>
          <div className="card-bd grid grid-2">
            {DRIVERS.map((d) => (
              <button
                key={d.id}
                onClick={() => setDriverId(d.id)}
                style={{
                  textAlign: "left", padding: 14, borderRadius: 12,
                  border: driverId === d.id ? "1px solid rgba(34,211,238,.55)" : "1px solid var(--border)",
                  background: driverId === d.id ? "rgba(34,211,238,.12)" : "rgba(255,255,255,.04)",
                  boxShadow: driverId === d.id ? "0 0 0 3px rgba(34,211,238,.16)" : "none", cursor: "pointer"
                }}
              >
                <div style={{ display: "flex", gap: 12, alignItems: "center" }}>
                  <span style={{ width: 36, height: 36, borderRadius: 10, display: "grid", placeItems: "center", background: "linear-gradient(135deg,var(--accent),var(--accent2))", color: "#0b1220", fontWeight: 800 }}>{d.icon}</span>
                  <div><div style={{ fontWeight: 800 }}>{d.name}</div><div className="help">{d.sub}</div></div>
                  {driverId === d.id && <span className="badge badge-ok" style={{ marginLeft: "auto" }}>已选</span>}
                </div>
                <div className="help" style={{ marginTop: 8 }}>driver_id: <span className="mono">{d.id}</span></div>
              </button>
            ))}
          </div>
          {driverId && driverProfiles.length > 0 && (
            <div className="card-bd" style={{ paddingTop: 0 }}>
              <div className="label" style={{ marginBottom: 6 }}>可选型号（Profile，可跳过）</div>
              <select className="select" value={profileId} onChange={(e) => setProfileId(e.target.value)}>
                <option value="">不选型号，直接用驱动默认</option>
                {driverProfiles.map((p) => <option key={p.id} value={p.id}>{p.vendor} {p.family} {p.model} — {p.id}</option>)}
              </select>
              {selectedProfile && (
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginTop: 10 }}>
                  <span className="help">快捷预设：</span>
                  {selectedProfile.presets.map((pr) => (
                    <button key={pr.id} className="btn btn-ghost btn-sm" onClick={() => setSelections((s) => [...s, ...pr.selections])}>+ {pr.label["zh-CN"] ?? pr.label.default}</button>
                  ))}
                </div>
              )}
            </div>
          )}
          <div className="card-bd" style={{ display: "flex", justifyContent: "flex-end" }}>
            <button className="btn" disabled={!canNext2} onClick={() => setStep(2)}>下一步：连接设备 →</button>
          </div>
        </div>
      )}

      {/* Step 2 连设备 */}
      {step === 2 && (
        <div className="card">
          <div className="card-hd">
            <h3>② 连接设备</h3>
            <span className="badge mono">{driverId}</span>
          </div>
          <div className="card-bd">
            {!desc ? <div className="help">加载连接表单…</div> : (
              <>
                {selectedProfile && <div className="badge badge-ok" style={{ marginBottom: 10 }}>已填入 {selectedProfile.id} 默认连接</div>}
                <SchemaForm schema={desc.connection} values={connection} onChange={setConnection} issues={issues} />
                <div style={{ display: "flex", gap: 10, marginTop: 12, flexWrap: "wrap" }}>
                  <button className="btn" onClick={validate}>校验</button>
                  <button className="btn btn-ghost" onClick={doProbe}>探测（6s）</button>
                  {probe && <span className={probe.ok ? "badge badge-ok" : "badge badge-bad"}>{probe.msg}</span>}
                  {valid !== null && <span className={valid ? "badge badge-ok" : "badge badge-bad"}>{valid ? "校验通过" : "校验失败"}</span>}
                </div>
              </>
            )}
          </div>
          <div className="card-bd" style={{ display: "flex", justifyContent: "space-between" }}>
            <button className="btn btn-ghost" onClick={() => setStep(1)}>← 返回</button>
            <button className="btn" disabled={!canNext3} onClick={() => setStep(3)}>下一步：设置点位 →</button>
          </div>
        </div>
      )}

      {/* Step 3 设点位 */}
      {step === 3 && (
        <div className="card">
          <div className="card-hd"><h3>③ 设置点位</h3><span className="help">按驱动资源自渲染，生成 point_key</span></div>
          <div className="card-bd">
            {!desc ? <div className="help">请先完成连接</div> : (
              <>
                <ResourcePicker resources={desc.resources} onAdd={(s) => setSelections((prev) => [...prev, s])} />
                <div className="card" style={{ marginTop: 12, background: "rgba(0,0,0,.18)" }}>
                  <div className="card-hd"><h3>已选</h3><span className="badge">{selections.length} 项</span></div>
                  <div className="card-bd scroll" style={{ maxHeight: 180 }}>
                    <pre className="mono" style={{ margin: 0, fontSize: 12, whiteSpace: "pre-wrap" }}>{selections.length ? JSON.stringify(selections, null, 2) : "尚未添加点位，勾选输出后点“加入选择”"}</pre>
                  </div>
                </div>
              </>
            )}
          </div>
          <div className="card-bd" style={{ display: "flex", justifyContent: "space-between" }}>
            <button className="btn btn-ghost" onClick={() => setStep(2)}>← 返回</button>
            <button className="btn" disabled={selections.length === 0} onClick={() => setStep(4)}>完成 → 看数据</button>
          </div>
        </div>
      )}

      {/* Step 4 看数据 */}
      {step === 4 && (
        <div className="grid" style={{ gap: 14 }}>
          <div className="card">
            <div className="card-hd"><h3>④ 实时数据</h3><span className="badge badge-ok">自动刷新 1s · GET /points/latest</span></div>
            <div className="card-bd" style={{ overflow: "auto" }}>
              <table className="table">
                <thead><tr><th>Endpoint</th><th>Point key</th><th>Id</th><th>Quality</th><th>Type</th><th>Value</th><th>Time</th></tr></thead>
                <tbody>
                  {(live?.points ?? []).slice(0, 50).map((p) => (
                    <tr key={`${p.endpoint_id}:${p.point_id}`}>
                      <td className="mono" style={{ fontSize: 11 }}>{p.endpoint_id}</td>
                      <td className="mono">{p.point_key}</td>
                      <td className="mono">{p.point_id}</td>
                      <td><span className={p.quality === "GOOD" ? "badge badge-ok" : p.quality === "BAD" ? "badge badge-bad" : "badge badge-warn"}>{p.quality}</span></td>
                      <td className="mono">{p.value.type}</td>
                      <td className="mono" style={{ maxWidth: 200, overflow: "hidden", textOverflow: "ellipsis" }}>{String(p.value.value)}</td>
                      <td className="mono" style={{ fontSize: 11 }}>{new Date(Number(p.timestamp_ns) / 1e6).toLocaleTimeString()}</td>
                    </tr>
                  ))}
                  {!live?.points?.length && <tr><td colSpan={7} className="help" style={{ textAlign: "center", padding: 16 }}>暂无数据 · 请通过后端创建 endpoint 并 start（当前向导仅前端选型，落库由 /api/v1/endpoints 完成）</td></tr>}
                </tbody>
              </table>
            </div>
            <div className="card-bd" style={{ display: "flex", gap: 10 }}>
              <button className="btn btn-ghost" onClick={() => setStep(1)}>再建一个</button>
              <a className="btn btn-ghost" href="/diagnostics">去诊断 →</a>
            </div>
          </div>
          <div className="card">
            <div className="card-hd"><h3>已选配置</h3><span className="kbd">{driverId} · {selections.length} 点</span></div>
            <div className="card-bd scroll" style={{ maxHeight: 160 }}>
              <pre className="mono" style={{ margin: 0, fontSize: 12, whiteSpace: "pre-wrap" }}>{JSON.stringify({ driver_id: driverId, connection, selections }, null, 2)}</pre>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
