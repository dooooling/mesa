import { NavLink, Route, Routes, useLocation } from "react-router-dom";
import { ControlPanel } from "./components/ControlPanel";
import { DiagnosticsPanel } from "./components/DiagnosticsPanel";
import { DeviceManager } from "./components/DeviceManager";
import { PointsView } from "./pages/PointsView";
import { BrowsePage } from "./pages/Browse";
import { Dashboard } from "./pages/Dashboard";

// 侧边系统菜单（无向导，纯系统 CRUD）
const nav = [
  { to: "/", label: "总览", icon: "◈" },
  { to: "/devices", label: "设备" },
  { to: "/points", label: "数据" },
  { to: "/browse", label: "浏览" },
  { to: "/control", label: "控制" },
  { to: "/diagnostics", label: "诊断" },
];

const titles: Record<string, string> = {
  "/": "总览",
  "/devices": "设备",
  "/points": "数据",
  "/browse": "浏览",
  "/control": "控制",
  "/diagnostics": "诊断",
};

export default function App() {
  const loc = useLocation();
  const title = titles[loc.pathname] ?? titles["/" + loc.pathname.split("/")[1]] ?? "Mesa";

  return (
    <div className="mesa-shell">
      <aside className="mesa-sidebar">
        <div className="brand">
          <div className="brand-mark">M</div>
          <div>
            <h1>Mesa</h1>
            <p>Industrial Data Platform</p>
          </div>
          <span className="pill" style={{ marginLeft: "auto" }}>V2.1</span>
        </div>
        <nav className="nav">
          <div className="nav-group">系统菜单</div>
          {nav.map((n) => (
            <NavLink key={n.to} to={n.to} end={n.to === "/"} className={({ isActive }) => `nav-item ${isActive ? "active" : ""}`}>
              <span className="nav-dot" />
              <span style={{ flex: 1 }}>{n.label}</span>
              {n.icon && <span style={{ fontSize: 12, opacity: .7 }}>{n.icon}</span>}
            </NavLink>
          ))}
        </nav>
        <div className="sidebar-foot">
          <span className="badge badge-ok">● 在线</span>
          <span className="help">Core 不懂协议</span>
        </div>
      </aside>

      <div className="mesa-main">
        <header className="mesa-topbar">
          <strong>{title}</strong>
          <span className="kbd mono" style={{ marginLeft: 10 }}>{loc.pathname}</span>
          <span style={{ marginLeft: "auto" }} className="badge">只读 V1 · loopback</span>
        </header>
        <main className="mesa-content">
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/devices" element={<DeviceManager />} />
            <Route path="/points" element={<PointsView />} />
            <Route path="/browse" element={<BrowsePage />} />
            <Route path="/control" element={<ControlPanel />} />
            <Route path="/diagnostics" element={<DiagnosticsPanel />} />
          </Routes>
        </main>
      </div>
    </div>
  );
}
