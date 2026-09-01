import { Link, Route, Routes } from "react-router-dom";
import { DeviceWizard } from "./components/DeviceWizard";
import { DiagnosticsPanel } from "./components/DiagnosticsPanel";
import { BrowsePage } from "./pages/Browse";

// 仅通用路由，无 FocasForm / S7Form 等协议特定组件（V2.1 §21.1）
export default function App() {
  return (
    <div>
      <nav style={{ padding: 12, borderBottom: "1px solid #ddd" }}>
        <Link to="/" style={{ marginRight: 12 }}>设备向导</Link>
        <Link to="/browse" style={{ marginRight: 12 }}>浏览</Link>
        <Link to="/diagnostics" style={{ marginRight: 12 }}>诊断</Link>
      </nav>
      <Routes>
        <Route path="/" element={<DeviceWizard />} />
        <Route path="/browse" element={<BrowsePage />} />
        <Route path="/diagnostics" element={<DiagnosticsPanel />} />
      </Routes>
    </div>
  );
}
