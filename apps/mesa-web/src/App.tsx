import { Layout, Menu, theme } from "antd";
import { DashboardOutlined, ApiOutlined, EyeOutlined, BellOutlined, SettingOutlined } from "@ant-design/icons";
import { Navigate, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { Dashboard } from "./pages/Dashboard";
import { DevicesPage } from "./pages/DevicesPage";
import { DeviceDetailPage } from "./pages/DeviceDetailPage";
import { EndpointWorkspacePage } from "./pages/EndpointWorkspacePage";
import { OnboardingPage } from "./pages/OnboardingPage";
import { MonitorView } from "./pages/MonitorView";
import { EventsView } from "./pages/EventsView";
import { DeviceWorkspacePage } from "./workspace/DeviceWorkspacePage";
import { LegacyEndpointRedirect } from "./workspace/LegacyEndpointRedirect";
import { GlobalDataPage } from "./pages/GlobalDataPage";
import { GlobalEventsPage } from "./pages/GlobalEventsPage";
import { OverviewPage } from "./overview/OverviewPage";
import { SystemPage } from "./pages/V2Placeholders";

const { Header, Sider, Content } = Layout;

// M1 V2 导航：总览 / 设备 / 实时数据 / 事件 / 系统。Device 是唯一一级主体，
// Connection 退化为 Device Workspace 内的上下文（`?connection=`），不再有导航层。
// 旧入口保留：/monitor、/events、/devices/:id、/devices/:deviceId/endpoints/:endpointId
// 全部兼容（重定向/并存），M5 才删除。
const items = [
  { key: "/overview", icon: <DashboardOutlined />, label: "总览" },
  { key: "/devices", icon: <ApiOutlined />, label: "设备" },
  { key: "/data", icon: <EyeOutlined />, label: "实时数据" },
  { key: "/events", icon: <BellOutlined />, label: "事件" },
  { key: "/system", icon: <SettingOutlined />, label: "系统" },
];
// 旧菜单 key → 新菜单 key（Header 标题与侧栏高亮共用，避免旧路由无标题）。
const LEGACY_SELECTED: Array<{ prefix: string; key: string; label: string }> = [
  { prefix: "/onboarding", key: "/devices", label: "设备" },
  { prefix: "/monitor", key: "/data", label: "实时数据" },
];

export default function App() {
  const loc = useLocation();
  const nav = useNavigate();
  const { token } = theme.useToken();
  // 高亮归属：Workspace 严格嵌套在 Device 下（无顶层入口），高亮仍归属 /devices；
  // 旧路由映射到新菜单（/monitor→/data），根路径与未知路径回总览。
  const selected = loc.pathname === "/"
    ? "/overview"
    : (items.find((i) => i.key !== "/overview" && loc.pathname.startsWith(i.key))?.key
      ?? LEGACY_SELECTED.find((l) => loc.pathname === l.prefix || loc.pathname.startsWith(`${l.prefix}/`))?.key
      ?? (loc.pathname.startsWith("/devices/") ? "/devices" : "/overview"));
  const headerLabel = items.find((i) => i.key === selected)?.label
    ?? LEGACY_SELECTED.find((l) => l.key === selected)?.label ?? "Mesa";

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
          <span style={{ fontWeight: 400, fontSize: 16 }}>{headerLabel}</span>
        </Header>
        <Content style={{ margin: 16 }}>
          <Routes>
            {/* M1 V2 路由。旧路由并存（M5 删除）：/ 仍可用（重定向 /overview），
                /devices/:id 与 Endpoint 深层 URL 做兼容重定向。 */}
            <Route path="/" element={<Navigate to="/overview" replace />} />
            <Route path="/overview" element={<OverviewPage />} />
            <Route path="/devices" element={<DevicesPage />} />
            <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId" element={<Navigate to="overview" replace />} />
            <Route path="/devices/:deviceId/endpoints/:endpointId" element={<LegacyEndpointRedirect />} />
            <Route path="/data" element={<GlobalDataPage />} />
            <Route path="/events" element={<GlobalEventsPage />} />
            <Route path="/system" element={<SystemPage />} />
            {/* 旧入口（M5 删除）：DeviceDetail / Endpoint Workspace / Monitor /
                Dashboard / EventsView / Onboarding 暂时保留，但不再作为新入口。 */}
            <Route path="/dashboard-legacy" element={<Dashboard />} />
            <Route path="/devices/:id/legacy" element={<DeviceDetailPage />} />
            <Route path="/onboarding" element={<OnboardingPage />} />
            <Route path="/monitor" element={<MonitorView />} />
            <Route path="/events-legacy" element={<EventsView />} />
            <Route path="*" element={<Navigate to="/overview" replace />} />
          </Routes>
        </Content>
      </Layout>
    </Layout>
  );
}
