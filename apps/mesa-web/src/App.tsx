import { Layout, Menu, theme } from "antd";
import { DashboardOutlined, ApiOutlined, EyeOutlined, BellOutlined } from "@ant-design/icons";
import { Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { Dashboard } from "./pages/Dashboard";
import { DevicesPage } from "./pages/DevicesPage";
import { DeviceDetailPage } from "./pages/DeviceDetailPage";
import { OnboardingPage } from "./pages/OnboardingPage";
import { MonitorView } from "./pages/MonitorView";
import { EventsView } from "./pages/EventsView";

const { Header, Sider, Content } = Layout;

// 导航以 Device 为管理入口：一级菜单只表达“管理什么”（domain），
// 不表达“执行什么动作”。创建行为收进设备上下文（DevicesPage 的
// “新建设备”/“新建向导”）；/onboarding 只是 workflow route，不占菜单。
// Endpoint 是采集运行实体，只在设备详情内管理，不再有顶层入口。
const items = [
  { key: "/", icon: <DashboardOutlined />, label: "看板" },
  { key: "/devices", icon: <ApiOutlined />, label: "设备" },
  { key: "/monitor", icon: <EyeOutlined />, label: "监控" },
  { key: "/events", icon: <BellOutlined />, label: "事件" },
];

export default function App() {
  const loc = useLocation();
  const nav = useNavigate();
  const { token } = theme.useToken();
  // /devices/:id 与 /onboarding 都归属设备管理：前者是详情，后者是
  // 设备创建流程（workflow route，非一级模块），Header 同显示“设备”
  const selected = (loc.pathname === "/onboarding" || loc.pathname.startsWith("/devices/")) ? "/devices"
    : (items.find((i) => i.key !== "/" && loc.pathname.startsWith(i.key))?.key ?? "/");

  return (
    <Layout style={{ minHeight: "100vh", background: token.colorBgLayout }}>
      {/* Carbon 应用壳：白底侧栏 + 右侧 hairline，选中态灰底 + 左侧蓝条（见 carbon.css） */}
      <Sider
        breakpoint="lg"
        collapsedWidth="64"
        theme="light"
        style={{ overflow: "auto", height: "100vh", position: "sticky", top: 0, left: 0, background: "#ffffff", borderRight: "1px solid #e0e0e0" }}
      >
        <div style={{ height: 56, display: "flex", alignItems: "center", gap: 10, padding: "0 16px", color: "#161616", fontWeight: 600, borderBottom: "1px solid #e0e0e0" }}>
          <span style={{ width: 28, height: 28, background: token.colorPrimary, color: "#ffffff", display: "grid", placeItems: "center", fontSize: 14, fontWeight: 600 }}>M</span>
          <span>Mesa</span>
        </div>
        <Menu theme="light" mode="inline" selectedKeys={[selected]} items={items} onClick={({ key }) => nav(key)} style={{ borderRight: "none" }} />
      </Sider>
      <Layout style={{ background: token.colorBgLayout }}>
        <Header style={{ padding: "0 16px", background: "#ffffff", borderBottom: "1px solid #e0e0e0", display: "flex", alignItems: "center" }}>
          <span style={{ fontWeight: 400, fontSize: 16 }}>{items.find((i) => i.key === selected)?.label ?? "Mesa"}</span>
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
