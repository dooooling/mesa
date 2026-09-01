import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";

export function Dashboard() {
  const [diag, setDiag] = useState<Record<string, unknown> | null>(null);
  const [endpoints, setEndpoints] = useState<number>(0);
  const [drivers, setDrivers] = useState<number>(0);

  useEffect(() => {
    api.diagnostics().then((d: Record<string, unknown>) => setDiag(d)).catch(() => {});
    api.listEndpoints().then((j: { endpoints?: unknown[] }) => setEndpoints(j.endpoints?.length ?? 0)).catch(() => {});
    api.listDrivers().then((j: { drivers?: unknown[] }) => setDrivers(j.drivers?.length ?? 0)).catch(() => {});
  }, []);

  return (
    <div className="grid" style={{ gap: 16 }}>
      <div className="card">
        <div className="card-bd" style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 16 }}>
          <div>
            <div style={{ fontSize: 12, letterSpacing: ".08em", color: "var(--muted)", textTransform: "uppercase" }}>Mesa 统一采集平台</div>
            <h2 style={{ margin: "6px 0 8px", fontSize: 22, letterSpacing: "-.02em" }}>一切以 Descriptor 为准 · Core 不懂协议</h2>
            <p className="help" style={{ maxWidth: 720, lineHeight: 1.6 }}>通用 UI 仅渲染 Driver 自描述（Connection / Resource / Control / Discovery）；无 S7Form / FocasForm 分支，新增驱动零前端改动。全量快照替换，队列有界背压 Latest-Wins。</p>
            <div style={{ display: "flex", gap: 10, marginTop: 12, flexWrap: "wrap" }}>
              <Link to="/devices" className="btn">添加设备向导 →</Link>
              <Link to="/browse" className="btn btn-ghost">浏览点位</Link>
              <Link to="/control" className="btn btn-ghost">控制面</Link>
            </div>
          </div>
          <div className="grid grid-2" style={{ minWidth: 320 }}>
            <div className="card stat"><div className="stat-label">Drivers</div><div className="stat-value">{drivers || "—"}</div><div className="stat-sub">已发现（sim/s7/focas2/opcua）</div></div>
            <div className="card stat"><div className="stat-label">Endpoints</div><div className="stat-value">{endpoints}</div><div className="stat-sub">运行中任务</div></div>
          </div>
        </div>
      </div>

      <div className="grid grid-3">
        <div className="card stat"><div className="stat-label">吞吐预算</div><div className="stat-value">≥50K <span style={{ fontSize: 13, fontWeight: 600, color: "var(--muted)" }}>updates/s</span></div><div className="stat-sub">60min 稳定 · 背压有效</div></div>
        <div className="card stat"><div className="stat-label">IPC 时延</div><div className="stat-value">p95 ≤20ms <span style={{ fontSize: 12, color: "var(--muted)" }}>p99 ≤50ms</span></div><div className="stat-sub">单调时钟测量</div></div>
        <div className="card stat"><div className="stat-label">质量模型</div><div className="stat-value" style={{ fontSize: 16 }}>GOOD / UNCERTAIN / BAD</div><div className="stat-sub">typed BAD + COMM_LOST 注入</div></div>
      </div>

      <div className="grid grid-2">
        <div className="card">
          <div className="card-hd"><h3>诊断快照</h3><span className="kbd">/api/v1/diagnostics</span></div>
          <div className="card-bd scroll" style={{ maxHeight: 280 }}>
            <pre style={{ margin: 0, fontSize: 12, lineHeight: 1.6, whiteSpace: "pre-wrap", wordBreak: "break-all" }}>{diag ? JSON.stringify(diag, null, 2) : "加载中…"}</pre>
          </div>
        </div>
        <div className="card">
          <div className="card-hd"><h3>快速上手</h3><span className="badge">通用流程</span></div>
          <div className="card-bd" style={{ display: "grid", gap: 10 }}>
            {[
              { n: "1", t: "选择型号 Profile", d: "厂商/家族/型号一键带入连接默认值与 Presets" },
              { n: "2", t: "校验与探测", d: "validate-connection 结构校验 + probe 真实可达性（6s）" },
              { n: "3", t: "选择数据", d: "ResourcePicker 按 Descriptor 渲染参数与 outputs，生成 point_key" },
              { n: "4", t: "浏览/控制", d: "OPC UA 浏览分页 · 控制面可靠队列（需 --enable-control）" },
            ].map((s) => (
              <div key={s.n} style={{ display: "flex", gap: 12, alignItems: "flex-start" }}>
                <span className="brand-mark" style={{ width: 28, height: 28, borderRadius: 9, fontSize: 12 }}>{s.n}</span>
                <div><div style={{ fontWeight: 700, fontSize: 13 }}>{s.t}</div><div className="help">{s.d}</div></div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
