import { NavLink, Route, Routes, useLocation } from "react-router-dom";
import { ControlPanel } from "./components/ControlPanel";
import { DeviceWizard } from "./components/DeviceWizard";
import { DiagnosticsPanel } from "./components/DiagnosticsPanel";
import { BrowsePage } from "./pages/Browse";
import { Dashboard } from "./pages/Dashboard";

// 工业控制台：侧边栏+顶栏，保持通用（无 driverId 分支）
const nav = [
  { to: "/", label: "总览", desc: "Overview", end: true },
  { to: "/devices", label: "设备向导", desc: "Add device" },
  { to: "/browse", label: "浏览", desc: "Browse & Import" },
  { to: "/control", label: "控制", desc: "Control plane" },
  { to: "/diagnostics", label: "诊断", desc: "Diagnostics" },
];

export default function App() {
  const loc = useLocation();
  const title =
    loc.pathname === "/" ? "总览" :
    loc.pathname.startsWith("/devices") ? "添加设备" :
    loc.pathname.startsWith("/browse") ? "浏览 / 导入" :
    loc.pathname.startsWith("/control") ? "控制面" : "诊断";

  return (
    <div className="mesa-shell">
      <aside className="mesa-sidebar">
        <div className="brand">
          <div className="brand-mark">M</div>
          <div>
            <h1>Mesa</h1>
            <p>Industrial Data Platform</p>
          </div>
          <span className="pill" style={{ marginLeft: "auto" }}>MVP</span>
        </div>
        <nav className="nav">
          <div className="nav-group">Workspace</div>
          {nav.map((n) => (
            <NavLink key={n.to} to={n.to} end={n.end as never} className={({ isActive }) => `nav-item ${isActive ? "active" : ""}`}>
              <span className="nav-dot" />
              <span style={{ flex: 1 }}>{n.label}</span>
              <span style={{ fontSize: 11, color: "var(--muted)" }}>{n.desc}</span>
            </NavLink>
          ))}
          <div className="nav-group" style={{ marginTop: 8 }}>Status</div>
          <div className="card" style={{ padding: 12 }}>
            <div style={{ fontSize: 12, color: "var(--muted)" }}>采集状态</div>
            <div style={{ display: "flex", gap: 8, marginTop: 8, flexWrap: "wrap" }}>
              <span className="badge badge-ok">Core 正常</span>
              <span className="badge">IPC p95 ≤20ms</span>
            </div>
            <div className="help" style={{ marginTop: 8 }}>REST 仅 loopback · 全量快照替换</div>
          </div>
        </nav>
        <div className="sidebar-foot">
          <span className="kbd">V2.1</span>
          <span>Driver 自描述 · 通用 UI</span>
        </div>
      </aside>

      <div className="mesa-main">
        <header className="mesa-topbar">
          <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
            <strong style={{ letterSpacing: ".02em" }}>{title}</strong>
            <span className="kbd">{loc.pathname}</span>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span className="badge">只读 V1</span>
            <span className="badge badge-ok">● 在线</span>
          </div>
        </header>
        <main className="mesa-content">
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/devices" element={<DeviceWizard />} />
            <Route path="/browse" element={<BrowsePage />} />
            <Route path="/control" element={<ControlPanel />} />
            <Route path="/diagnostics" element={<DiagnosticsPanel />} />
          </Routes>
        </main>
      </div>
    </div>
  );
}
