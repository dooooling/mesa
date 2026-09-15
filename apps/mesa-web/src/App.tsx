import { Layout, Menu, theme } from "antd";
import { DashboardOutlined, ApiOutlined, EyeOutlined, BellOutlined, SettingOutlined } from "@ant-design/icons";
import { Navigate, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { DevicesPage } from "./pages/DevicesPage";
import { DeviceWorkspacePage } from "./workspace/DeviceWorkspacePage";
import { LegacyEndpointRedirect } from "./workspace/LegacyEndpointRedirect";
import { AddDeviceFlow } from "./addDevice/AddDeviceFlow";
import { GlobalDataPage } from "./pages/GlobalDataPage";
import { GlobalEventsPage } from "./pages/GlobalEventsPage";
import { OverviewPage } from "./overview/OverviewPage";
import { SystemPage } from "./system/SystemPage";

const { Header, Sider, Content } = Layout;

// M5.6 V2 导航（旧页面已删除）：总览 / 设备 / 实时数据 / 事件 / 系统。
// Device 是唯一一级主体，Connection 退化为 Device Workspace 内的上下文
//（`?connection=`），不再有导航层。外部旧书签仅保留 endpoint 深链重定向。
const items = [
  { key: "/overview", icon: <DashboardOutlined />, label: "总览" },
  { key: "/devices", icon: <ApiOutlined />, label: "设备" },
  { key: "/data", icon: <EyeOutlined />, label: "实时数据" },
  { key: "/events", icon: <BellOutlined />, label: "事件" },
  { key: "/system", icon: <SettingOutlined />, label: "系统" },
];

export default function App() {
  const loc = useLocation();
  const nav = useNavigate();
  const { token } = theme.useToken();
  // 高亮归属：Workspace 严格嵌套在 Device 下（无顶层入口），高亮仍归属 /devices；
  // 根路径与未知路径回总览。
  const selected = loc.pathname === "/"
    ? "/overview"
    : (items.find((i) => i.key !== "/overview" && loc.pathname.startsWith(i.key))?.key
      ?? (loc.pathname.startsWith("/devices/") ? "/devices" : "/overview"));
  const headerLabel = items.find((i) => i.key === selected)?.label ?? "Mesa";

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
            {/* M5.6 V2 路由（旧页面已删除）。外部旧书签仅保留 endpoint 深链重定向。 */}
            <Route path="/" element={<Navigate to="/overview" replace />} />
            <Route path="/overview" element={<OverviewPage />} />
            <Route path="/devices" element={<DevicesPage />} />
            <Route path="/devices/new" element={<AddDeviceFlow />} />
            <Route path="/devices/:deviceId/:tab" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId" element={<Navigate to="overview" replace />} />
            <Route path="/devices/:deviceId/endpoints/:endpointId" element={<LegacyEndpointRedirect />} />
            <Route path="/data" element={<GlobalDataPage />} />
            <Route path="/events" element={<GlobalEventsPage />} />
            <Route path="/system" element={<SystemPage />} />
            <Route path="*" element={<Navigate to="/overview" replace />} />
          </Routes>
        </Content>
      </Layout>
    </Layout>
  );
}
