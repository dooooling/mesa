import { Layout, Menu, theme } from "antd";
import { DashboardOutlined, ApiOutlined, PlusOutlined, EyeOutlined, BellOutlined } from "@ant-design/icons";
import { Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { Dashboard } from "./pages/Dashboard";
import { DevicesPage } from "./pages/DevicesPage";
import { DeviceDetailPage } from "./pages/DeviceDetailPage";
import { OnboardingPage } from "./pages/OnboardingPage";
import { MonitorView } from "./pages/MonitorView";
import { EventsView } from "./pages/EventsView";

const { Header, Sider, Content } = Layout;

// 导航以 Device 为管理入口：设备列表 / 设备详情 / 新建向导；
// Endpoint 是采集运行实体，只在设备详情内管理，不再有顶层入口。
const items = [
  { key: "/", icon: <DashboardOutlined />, label: "看板" },
  { key: "/devices", icon: <ApiOutlined />, label: "设备" },
  { key: "/onboarding", icon: <PlusOutlined />, label: "新建" },
  { key: "/monitor", icon: <EyeOutlined />, label: "监控" },
  { key: "/events", icon: <BellOutlined />, label: "事件" },
];

export default function App() {
  const loc = useLocation();
  const nav = useNavigate();
  const { token } = theme.useToken();
  // /devices/:id 高亮归属 /devices，保持 Device 入口心智
  const selected = loc.pathname.startsWith("/devices/") ? "/devices"
    : (items.find((i) => i.key !== "/" && loc.pathname.startsWith(i.key))?.key ?? "/");

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
            <Route path="/devices" element={<DevicesPage />} />
            <Route path="/devices/:id" element={<DeviceDetailPage />} />
            <Route path="/onboarding" element={<OnboardingPage />} />
            <Route path="/monitor" element={<MonitorView />} />
            <Route path="/events" element={<EventsView />} />
          </Routes>
        </Content>
      </Layout>
    </Layout>
  );
}
