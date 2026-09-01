// 通用 DeviceWizard：Profile(厂商/型号) → Connection(带默认值) → Validate/Probe → Preset一键 → ResourcePicker → Review（V2.1 §21.2, §10）
// 仅依赖 Descriptor/Profile，不含 driverId 分支
import { useEffect, useState } from "react";
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
  const [probe, setProbe] = useState<string>("");
  const [selections, setSelections] = useState<unknown[]>([]);

  useEffect(() => {
    api.listProfiles().then((j) => setProfiles(j.profiles ?? [])).catch(() => {});
  }, []);

  const selectedProfile = profiles.find((p) => p.id === profileId) ?? null;

  useEffect(() => {
    const id = selectedProfile?.driver_id ?? driverId;
    if (!id) return;
    api.getDescriptor(id).then((d) => {
      setDesc(d);
      // 带入 Profile 的默认值
      setConnection(selectedProfile?.connection_defaults ?? {});
      setIssues([]);
    }).catch(() => setDesc(null));
  }, [selectedProfile, driverId]);

  const applyPreset = (presetId: string) => {
    const preset = selectedProfile?.presets.find((p) => p.id === presetId);
    if (!preset) return;
    setSelections([...selections, ...preset.selections]);
  };

  const validate = async () => {
    const id = selectedProfile?.driver_id ?? driverId;
    const r = await api.validateConnection(id, connection);
    if (r.status === 200) { setIssues([]); setProbe("校验通过"); }
    else { setIssues(r.body.issues ?? []); setProbe(`校验失败: ${r.body.error?.message ?? ""}`); }
  };

  const doProbe = async () => {
    const id = selectedProfile?.driver_id ?? driverId;
    const r = await api.probe(id, connection);
    setProbe(r.body.reachable ? "可达" : `不可达: ${r.body.error ?? ""}`);
  };

  return (
    <div style={{ maxWidth: 760, margin: "0 auto", padding: 16 }}>
      <h2>添加设备（通用向导）</h2>

      <label>
        设备型号（Profile）
        <select value={profileId} onChange={(e) => { setProfileId(e.target.value); if (e.target.value) setDriverId(""); }} style={{ marginLeft: 8 }}>
          <option value="">选择型号（或直接选驱动）</option>
          {profiles.map((p) => (
            <option key={p.id} value={p.id}>
              {p.vendor} {p.family} {p.model} ({p.id})
            </option>
          ))}
        </select>
      </label>

      {!profileId && (
        <label style={{ marginLeft: 16 }}>
          驱动
          <select value={driverId} onChange={(e) => setDriverId(e.target.value)} style={{ marginLeft: 8 }}>
            <option value="">选择驱动</option>
            <option value="simulator">Mesa Simulator (simulator)</option>
            <option value="s7">Siemens S7 (s7)</option>
            <option value="focas2">FANUC FOCAS2 (focas2)</option>
            <option value="opcua">OPC UA (opcua)</option>
          </select>
        </label>
      )}

      {selectedProfile && (
        <div style={{ marginTop: 8, background: "#f6f8fa", padding: 8 }}>
          <strong>推荐数据</strong>
          {selectedProfile.presets.map((preset) => (
            <button key={preset.id} onClick={() => applyPreset(preset.id)} style={{ marginLeft: 8 }}>
              添加 {preset.label["zh-CN"] ?? preset.label.default} ({preset.id})
            </button>
          ))}
        </div>
      )}

      {desc && (
        <>
          <h3>连接参数 {selectedProfile && `（${selectedProfile.id} 默认值已填入）`}</h3>
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
