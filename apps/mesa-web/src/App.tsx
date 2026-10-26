import { Layout, Menu, theme } from "antd";
import { DashboardOutlined, ApiOutlined, EyeOutlined, BellOutlined, SettingOutlined } from "@ant-design/icons";
import { Navigate, Route, Routes, useLocation } from "react-router-dom";
import { DevicesPage } from "./pages/DevicesPage";
import { DeviceWorkspacePage } from "./workspace/DeviceWorkspacePage";
import { AddDeviceFlow } from "./addDevice/AddDeviceFlow";
import { GlobalDataPage } from "./pages/GlobalDataPage";
import { GlobalEventsPage } from "./pages/GlobalEventsPage";
import { NotFoundPage } from "./pages/NotFoundPage";
import { OverviewPage } from "./overview/OverviewPage";
import { StaleClockProvider } from "./workspace/StaleClock";
import { SystemPage } from "./system/SystemPage";
import { DraftGuardProvider, useGuardedNavigate } from "./workspace/draftNavGuard";

const { Header, Sider, Content } = Layout;

// M5.6 V2 导航（旧页面已删除）：总览 / 设备 / 实时数据 / 事件 / 系统。
// Device 是唯一一级主体，Connection 是各设备页内的局部筛选
//（`?connection=` 各页独立），不再有导航层，不保留旧书签兼容。
const items = [
  { key: "/overview", icon: <DashboardOutlined />, label: "总览" },
  { key: "/devices", icon: <ApiOutlined />, label: "设备" },
  { key: "/data", icon: <EyeOutlined />, label: "实时数据" },
  { key: "/events", icon: <BellOutlined />, label: "事件" },
  { key: "/system", icon: <SettingOutlined />, label: "系统" },
];

export default function App() {
  // 守卫状态必须在外壳层（侧栏与面包屑都不在设备页里）：Provider 包住整个应用壳，
  // 由 AppShell 消费（`useGuardedNavigate` 必须在 Provider 内部调用）。
  return (
    <DraftGuardProvider>
      <AppShell />
    </DraftGuardProvider>
  );
}

function AppShell() {
  const loc = useLocation();
  const { token } = theme.useToken();
  // 应用内离开守卫：侧栏点击必须与设备 Tab/面包屑/浏览器前进后退共用同一判定
  //（有未保存草稿时先确认，取消则该次导航不发生，§7-7）。
  const guardedNavigate = useGuardedNavigate();
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
        <Menu theme="light" mode="inline" selectedKeys={[selected]} items={items} onClick={({ key }) => guardedNavigate(key)} style={{ borderRight: "none" }} />
      </Sider>
      <Layout style={{ background: token.colorBgLayout }}>
        <Header style={{ padding: "0 16px", background: "#ffffff", borderBottom: "1px solid #e0e0e0", display: "flex", alignItems: "center" }}>
          <span style={{ fontWeight: 400, fontSize: 16 }}>{headerLabel}</span>
        </Header>
        <Content style={{ margin: 16 }}>
          {/* 共享秒钟：实时表 AgeCell 的唯一时间源（单 interval，多 AgeCell 订阅）。 */}
          <StaleClockProvider>
          <Routes>
            {/* 扁平设备详情：只存在六个 Device Tab（非法 tab 无路由，自然落 NotFound）。
                config/legacy/endpoints 旧路由已删除，旧 URL 诚实 404。 */}
            <Route path="/" element={<Navigate to="/overview" replace />} />
            <Route path="/overview" element={<OverviewPage />} />
            <Route path="/devices" element={<DevicesPage />} />
            <Route path="/devices/new" element={<AddDeviceFlow />} />
            <Route path="/devices/:deviceId/overview" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId/data" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId/connections" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId/acquisition" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId/events" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId/diagnostics" element={<DeviceWorkspacePage />} />
            <Route path="/devices/:deviceId" element={<Navigate to="overview" replace />} />
            <Route path="/data" element={<GlobalDataPage />} />
            <Route path="/events" element={<GlobalEventsPage />} />
            <Route path="/system" element={<SystemPage />} />
            <Route path="*" element={<NotFoundPage />} />
          </Routes>
          </StaleClockProvider>
        </Content>
      </Layout>
    </Layout>
  );
}
