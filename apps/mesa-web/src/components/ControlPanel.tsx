import { useEffect, useState } from "react";
import { api } from "../api";

type EndpointBrief = { id: string; driver_id: string };

export function ControlPanel() {
  const [endpoints, setEndpoints] = useState<EndpointBrief[]>([]);
  const [selected, setSelected] = useState("");
  const [descriptor, setDescriptor] = useState<{ controls?: { commands: Array<{ id: string; label?: unknown }> } } | null>(null);
  const [target, setTarget] = useState("sim.counter");
  const [value, setValue] = useState("42");
  const [cmd, setCmd] = useState("status");
  const [cmdInput, setCmdInput] = useState("{}");
  const [result, setResult] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    api.listEndpoints().then((j: { endpoints: Array<{ id: string; driver_id: string }> }) => {
      const eps = (j.endpoints || []).map((e) => ({ id: e.id, driver_id: e.driver_id }));
      setEndpoints(eps);
      if (eps[0]) setSelected(eps[0].id);
    }).catch(() => {});
  }, []);

  useEffect(() => {
    if (!selected) return;
    const ep = endpoints.find((e) => e.id === selected);
    if (!ep) return;
    api.getDescriptor(ep.driver_id).then((d: unknown) => setDescriptor(d as never)).catch(() => setDescriptor(null));
  }, [selected, endpoints]);

  const doWrite = async () => {
    setError(""); setResult("");
    let parsed: unknown = value;
    const num = Number(value);
    if (value.trim() !== "" && !Number.isNaN(num) && String(num) === value.trim()) parsed = num;
    else if (value === "true" || value === "false") parsed = value === "true";
    const r = await fetch(`/api/v1/endpoints/${selected}/write`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ target, value: parsed }) });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) setError(`${r.status} ${j.error?.code || ""} ${j.error?.message || JSON.stringify(j)}`);
    else setResult(JSON.stringify(j, null, 2));
  };

  const doCommand = async () => {
    setError(""); setResult("");
    let input: unknown = {};
    try { input = JSON.parse(cmdInput); } catch { input = {}; }
    const r = await fetch(`/api/v1/endpoints/${selected}/commands/${cmd}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input && typeof input === "object" && !Array.isArray(input) ? input : { input }) });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) setError(`${r.status} ${j.error?.code || ""} ${j.error?.message || JSON.stringify(j)}`);
    else setResult(JSON.stringify(j, null, 2));
  };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div className="card">
        <div className="card-bd" style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
          <span className="badge badge-warn">需 --enable-control</span>
          <span className="help">可靠 Control 队列（32 有界，禁 Latest-Wins）；未开启返回 503 CONTROL_DISABLED</span>
          <span style={{ marginLeft: "auto", display: "flex", gap: 8, alignItems: "center" }}>
            <span className="label">Endpoint</span>
            <select className="select" value={selected} onChange={(e) => setSelected(e.target.value)} style={{ minWidth: 260 }}>
              {endpoints.map((e) => <option key={e.id} value={e.id}>{e.id} ({e.driver_id})</option>)}
              {!endpoints.length && <option value="">暂无端点</option>}
            </select>
          </span>
        </div>
      </div>

      {descriptor?.controls?.commands?.length ? (
        <div className="card"><div className="card-bd"><span className="label">可用命令：</span> {descriptor.controls.commands.map((c) => <span key={c.id} className="kbd mono" style={{ marginRight: 8 }}>{c.id}</span>)}</div></div>
      ) : null}

      <div className="grid grid-2">
        <div className="card">
          <div className="card-hd"><h3>Write</h3><span className="kbd">POST /endpoints/:id/write</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 12 }}>
            <div><div className="label" style={{ marginBottom: 6 }}>target（point_key / binding）</div><input className="input mono" value={target} onChange={(e) => setTarget(e.target.value)} placeholder="sim.counter / ns=2;s=Demo.Static.Scalar.Int32" /></div>
            <div><div className="label" style={{ marginBottom: 6 }}>value</div><input className="input mono" value={value} onChange={(e) => setValue(e.target.value)} placeholder="42 / true / 字符串" /></div>
            <button className="btn" onClick={doWrite}>写入</button>
          </div>
        </div>

        <div className="card">
          <div className="card-hd"><h3>Command</h3><span className="kbd">POST /endpoints/:id/commands/:command</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 12 }}>
            <div><div className="label" style={{ marginBottom: 6 }}>command</div><input className="input mono" value={cmd} onChange={(e) => setCmd(e.target.value)} placeholder="status / reset" /></div>
            <div><div className="label" style={{ marginBottom: 6 }}>input_json</div><input className="input mono" value={cmdInput} onChange={(e) => setCmdInput(e.target.value)} placeholder='{}' /></div>
            <button className="btn" onClick={doCommand}>执行</button>
          </div>
        </div>
      </div>

      {error && <div className="card" style={{ borderColor: "rgba(239,68,68,.35)" }}><div className="card-bd"><pre style={{ margin: 0, color: "#fca5a5", whiteSpace: "pre-wrap" }}>{error}</pre></div></div>}
      {result && <div className="card"><div className="card-hd"><h3>结果</h3><span className="badge badge-ok">200 OK</span></div><div className="card-bd scroll"><pre className="mono" style={{ margin: 0, fontSize: 12, whiteSpace: "pre-wrap" }}>{result}</pre></div></div>}
    </div>
  );
}
