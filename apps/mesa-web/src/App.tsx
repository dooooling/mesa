import { Layout, Menu, theme } from "antd";
import { DashboardOutlined, ApiOutlined, DatabaseOutlined, EyeOutlined } from "@ant-design/icons";
import { Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { Dashboard } from "./pages/Dashboard";
import { DeviceManager } from "./components/DeviceManager";
import { MonitorView } from "./pages/MonitorView";

const { Header, Sider, Content } = Layout;

const items = [
  { key: "/", icon: <DashboardOutlined />, label: "看板" },
  { key: "/devices", icon: <ApiOutlined />, label: "设备" },
  { key: "/monitor", icon: <EyeOutlined />, label: "监控" },
  { key: "/data", icon: <DatabaseOutlined />, label: "数据" },
];

export default function App() {
  const loc = useLocation();
  const nav = useNavigate();
  const { token } = theme.useToken();
  const selected = items.find((i) => i.key !== "/" && loc.pathname.startsWith(i.key))?.key ?? "/";

  return (
    <Layout style={{ minHeight: "100vh" }}>
      <Sider breakpoint="lg" collapsedWidth="64" style={{ overflow: "auto", height: "100vh", position: "sticky", top: 0, left: 0 }}>
        <div style={{ height: 56, display: "flex", alignItems: "center", gap: 10, padding: "0 16px", color: "#fff", fontWeight: 700 }}>
          <span style={{ width: 28, height: 28, borderRadius: 8, background: token.colorPrimary, display: "grid", placeItems: "center", fontSize: 14 }}>M</span>
          <span>Mesa</span>
        </div>
        <Menu theme="dark" mode="inline" selectedKeys={[selected]} items={items} onClick={({ key }) => nav(key)} />
      </Sider>
      <Layout>
        <Header style={{ padding: "0 16px", background: token.colorBgContainer, borderBottom: `1px solid ${token.colorBorderSecondary}`, display: "flex", alignItems: "center" }}>
          <span style={{ fontWeight: 600 }}>{items.find((i) => i.key === selected)?.label ?? "Mesa"}</span>
        </Header>
        <Content style={{ margin: 16 }}>
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/devices" element={<DeviceManager />} />
            <Route path="/monitor" element={<MonitorView />} />
            <Route path="/data" element={<MonitorView />} />
          </Routes>
        </Content>
      </Layout>
    </Layout>
  );
}
