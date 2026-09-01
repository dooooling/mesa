import { useEffect, useState } from "react";
import { api } from "../api";

type EndpointBrief = { id: string; driver_id: string };

export function ControlPanel() {
  const [endpoints, setEndpoints] = useState<EndpointBrief[]>([]);
  const [selected, setSelected] = useState<string>("");
  const [descriptor, setDescriptor] = useState<{ controls?: { commands: Array<{ id: string; label: { zh?: string; en?: string } | string }> } } | null>(null);
  const [target, setTarget] = useState("sim.counter");
  const [value, setValue] = useState("42");
  const [cmd, setCmd] = useState("reset");
  const [cmdInput, setCmdInput] = useState("{}");
  const [result, setResult] = useState<string>("");
  const [error, setError] = useState<string>("");

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
    // 尝试按数字解析
    const num = Number(value);
    if (value.trim() !== "" && !Number.isNaN(num) && String(num) === value.trim()) parsed = num;
    else if (value === "true" || value === "false") parsed = value === "true";
    const r = await fetch(`/api/v1/endpoints/${selected}/write`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ target, value: parsed }),
    });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) setError(`${r.status} ${j.error?.code || ""} ${j.error?.message || JSON.stringify(j)}`);
    else setResult(JSON.stringify(j, null, 2));
  };

  const doCommand = async () => {
    setError(""); setResult("");
    let input: unknown = {};
    try { input = JSON.parse(cmdInput); } catch { input = {}; }
    const r = await fetch(`/api/v1/endpoints/${selected}/commands/${cmd}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(input && typeof input === "object" && !Array.isArray(input) ? input : { input }),
    });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) setError(`${r.status} ${j.error?.code || ""} ${j.error?.message || JSON.stringify(j)}`);
    else setResult(JSON.stringify(j, null, 2));
  };

  return (
    <div style={{ padding: 16 }}>
      <h2>控制面（需 --enable-control）</h2>
      <p style={{ color: "#666" }}>可靠 Control 队列，禁 Latest-Wins；默认关闭，未开启时返回 503 CONTROL_DISABLED。</p>
      <label>Endpoint: <select value={selected} onChange={(e) => setSelected(e.target.value)}>
        {endpoints.map((e) => <option key={e.id} value={e.id}>{e.id} ({e.driver_id})</option>)}
      </select></label>
      {descriptor?.controls?.commands?.length ? (
        <div style={{ marginTop: 8 }}>可用命令: {descriptor.controls.commands.map((c) => c.id).join(", ")}</div>
      ) : null}

      <fieldset style={{ marginTop: 16 }}>
        <legend>Write</legend>
        <label>target: <input value={target} onChange={(e) => setTarget(e.target.value)} style={{ width: 200 }} /></label>{" "}
        <label>value: <input value={value} onChange={(e) => setValue(e.target.value)} style={{ width: 120 }} /></label>{" "}
        <button onClick={doWrite}>写入</button>
      </fieldset>

      <fieldset style={{ marginTop: 16 }}>
        <legend>Command</legend>
        <label>command: <input value={cmd} onChange={(e) => setCmd(e.target.value)} style={{ width: 120 }} /></label>{" "}
        <label>input_json: <input value={cmdInput} onChange={(e) => setCmdInput(e.target.value)} style={{ width: 300 }} /></label>{" "}
        <button onClick={doCommand}>执行</button>
      </fieldset>

      {error && <pre style={{ color: "crimson", background: "#fee", padding: 8 }}>{error}</pre>}
      {result && <pre style={{ background: "#f6f6f6", padding: 8 }}>{result}</pre>}
    </div>
  );
}
