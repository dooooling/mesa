// Device Workspace（扁平信息架构）：Device = 唯一 Workspace，
// Tab = 用户当前意图（概览/实时数据/连接/采集/事件/诊断）。
// 本页只负责：Device 加载 + Header + Tab 导航 + 当前页 + Point Drawer。
// Connection 选择各页局部管理（useConnectionQuery），无全局 Context。
import { useEffect, useState } from "react";
import { Alert, Button, Card } from "antd";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { PointDetailDrawer } from "../components/PointDetailDrawer";
import { DeviceAcquisition } from "./DeviceAcquisition";
import { DeviceConnections } from "./DeviceConnections";
import { DeviceDiagnostics } from "./DeviceDiagnostics";
import { DeviceEvents } from "./DeviceEvents";
import { DeviceHeader } from "./DeviceHeader";
import { DeviceLiveData } from "./DeviceLiveData";
import { DeviceOverview } from "./DeviceOverview";
import { DeviceTabNav } from "./DeviceTabNav";
import {
  useDeviceWorkspaceData,
  type DevicePointView,
} from "./useDeviceWorkspaceData";

export function DeviceWorkspacePage() {
  // 六条显式路由无 :tab 参数（见 App.tsx），tab 从 pathname 尾段派生。
  const { deviceId = "" } = useParams();
  const loc = useLocation();
  const segs = loc.pathname.split("/").filter(Boolean);
  const activeTab = segs[segs.length - 1] ?? "overview";
  const nav = useNavigate();
  // M2：Overview 与 LiveData 共用同一快照源（单轮询/单 nowMs/单归属判定），
  // 不再各自 GET /endpoints + /points/latest。
  const data = useDeviceWorkspaceData(deviceId);
  const {
    device,
    deviceNotFound: notFound,
    deviceError,
    deviceEndpoints,
    endpointsReady: epReady,
    endpointsError: epError,
  } = data;
  const [openPoint, setOpenPoint] = useState<DevicePointView | null>(null);

  // RC2 修2：scope detail state 必须在 scope identity 变化时清空。
  // 同一 route element 在 /devices/A/data → /devices/B/data 时被复用，
  // 不清的话 A 的 Point Drawer 会开在 B 的页面上，而 Drawer 头已变成 B
  // 的设备信息 → 错误来源展示。
  useEffect(() => {
    setOpenPoint(null);
  }, [deviceId]);

  // 404 视图必须在全部 hook 之后 early return：notFound 由异步请求后置，
  // 提前 return 会让本次渲染的 hook 数少于上次（Rendered fewer hooks）。
  const notFoundView = notFound ? (
    <Card size="small" title="设备不存在">
      <p style={{ color: "#525252" }}>设备 `{deviceId}` 不存在，可能已被删除。</p>
      <Button type="primary" onClick={() => nav("/devices")}>返回设备列表</Button>
    </Card>
  ) : null;

  const renderTab = () => {
    if (activeTab === "overview") {
      return (
        <DeviceOverview
          deviceId={deviceId}
          deviceName={device?.name ?? deviceId}
          endpoints={deviceEndpoints}
          endpointIds={deviceEndpoints.map((e) => e.id)}
          points={data.devicePoints}
          counts={data.counts}
          onOpenPoint={setOpenPoint}
        />
      );
    }
    if (activeTab === "data") {
      return (
        <DeviceLiveData
          endpoints={deviceEndpoints}
          endpointsReady={epReady}
          points={data.devicePoints}
          pointsError={data.pointsError}
          onOpenPoint={setOpenPoint}
        />
      );
    }
    if (activeTab === "connections") {
      return (
        <DeviceConnections
          deviceId={deviceId}
          endpoints={deviceEndpoints}
          onReload={data.reloadInventory}
        />
      );
    }
    if (activeTab === "acquisition") {
      return (
        <DeviceAcquisition
          endpoints={deviceEndpoints}
          endpointsReady={epReady}
          onReload={data.reloadInventory}
        />
      );
    }
    if (activeTab === "events") {
      return (
        <DeviceEvents
          deviceId={deviceId}
          deviceName={device?.name ?? deviceId}
          endpoints={deviceEndpoints}
          endpointsReady={epReady}
        />
      );
    }
    // diagnostics
    return (
      <DeviceDiagnostics
        endpoints={deviceEndpoints}
        endpointsReady={epReady}
        points={data.devicePoints}
        counts={data.counts}
      />
    );
  };

  // notFound 视图：hook 之后才分支返回，保证每次渲染 hook 数一致。
  if (notFoundView) return notFoundView;

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <DeviceHeader
        deviceId={deviceId}
        deviceName={device?.name ?? deviceId}
        endpoints={deviceEndpoints}
        onChanged={data.reloadInventory}
      />

      {epError ? <Alert type="warning" showIcon message="连接清单不可用" description={epError} /> : null}

      <DeviceTabNav deviceId={deviceId} active={activeTab} />

      {renderTab()}

      {deviceError ? (
        <Alert type="error" showIcon message="设备加载失败" description={deviceError} />
      ) : null}

      <PointDetailDrawer
        deviceId={deviceId}
        deviceName={device?.name ?? deviceId}
        point={openPoint}
        onClose={() => setOpenPoint(null)}
        // P2 改名：永远 patch 回请求时的 target；只有 Drawer 当前仍是
        // 同一点才刷新 openPoint（pending 期间切点不污染 B）。
        onRenamed={(target, display_name) => {
          data.patchDisplayName(target.endpoint_id, target.point_key, display_name);
          setOpenPoint((prev) =>
            prev !== null &&
            prev.endpoint_id === target.endpoint_id &&
            (prev.key ?? prev.point_key) === target.point_key
              ? { ...prev, display_name: display_name ?? undefined }
              : prev,
          );
        }}
      />
    </div>
  );
}
