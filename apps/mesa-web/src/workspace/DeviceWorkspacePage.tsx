// M1 Device Workspace：V2 骨架。用户进入一台设备后不再离开本页。
// - Header 始终展示 Device 身份（名 + id + 连接数），上下文稳定；
// - Connection selector：观察类 tab 可选全部，配置类 tab 必须单连接，
//   选择经 connection.ts 解析并同步回 URL（`?connection=` 稳定，不漂移）；
// - 五个 tab 在 M1 只给真实框架（M2/M3 填内容），不复制旧页面。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Space, Tabs, Tag } from "antd";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { isRunningState } from "../deviceModel";
import { PointDetailDrawer } from "../components/PointDetailDrawer";
import { DeviceConfig } from "./DeviceConfig";
import { DeviceDiagnostics } from "./DeviceDiagnostics";
import { DeviceEvents } from "./DeviceEvents";
import { DeviceLiveData } from "./DeviceLiveData";
import { DeviceOverview } from "./DeviceOverview";
import {
  WORKSPACE_TABS,
  isSingleConnectionTab,
  isWorkspaceTab,
  resolveEffectiveConnection,
  type WorkspaceTab,
} from "./connection";
import {
  useDeviceWorkspaceData,
  type DevicePointView,
  type WorkspaceEndpoint,
} from "./useDeviceWorkspaceData";

const TAB_LABEL: Record<WorkspaceTab, string> = {
  overview: "概览",
  data: "实时数据",
  events: "事件",
  config: "配置",
  diagnostics: "诊断",
};

interface EndpointSummary {
  id: string;
  name?: string;
  driver_id: string;
  device_id?: string;
  state?: string;
}

const TAB_PARAM: Record<string, WorkspaceTab> = {
  overview: "overview",
  data: "data",
  events: "events",
  config: "config",
  diagnostics: "diagnostics",
};

export function DeviceWorkspacePage() {
  const { deviceId = "", tab = "overview" } = useParams();
  const activeTab: WorkspaceTab = isWorkspaceTab(tab) ? tab : "overview";
  const nav = useNavigate();
  const [params, setParams] = useSearchParams();
  // M2：Overview 与 LiveData 共用同一快照源（单轮询/单 nowMs/单归属判定），
  // 不再各自 GET /endpoints + /points/latest。
  const data = useDeviceWorkspaceData(deviceId);
  const {
    device,
    deviceNotFound: notFound,
    deviceError,
    endpoints,
    endpointsReady: epReady,
    endpointsError: epError,
    endpointIds: deviceEndpointIds,
  } = data;
  const [openPoint, setOpenPoint] = useState<DevicePointView | null>(null);

  // RC2 修2：scope detail state 必须在 scope identity 变化时清空。
  // 同一 route element 在 /devices/A/data → /devices/B/data 时被复用，
  // 不清的话 A 的 Point Drawer 会开在 B 的页面上，而 Drawer 头已变成 B
  // 的设备信息 → 错误来源展示。
  useEffect(() => {
    setOpenPoint(null);
  }, [deviceId]);

  // M1 URL 稳定规则保持：清单未就绪前不动 URL，避免空清单误删合法参数。
  const endpointIds = useMemo(
    () => endpoints.filter((e) => (e.device_id ?? "") === deviceId).map((e) => e.id),
    [endpoints, deviceId],
  );
  const resolved = useMemo(
    () =>
      resolveEffectiveConnection({
        tab: activeTab,
        param: params.get("connection"),
        endpointIds,
      }),
    [activeTab, params, endpointIds],
  );

  // 选择稳定：最终选择同步回 URL（replace，不污染历史）。比较 normalizedParam
  // 与当前 param，避免 setSearchParams 循环。清单未就绪前不动 URL——
  // 空 endpointIds 下的回落会误删合法的 ?connection=（上下文漂移）。
  useEffect(() => {
    if (!epReady) return;
    const cur = params.get("connection");
    const want = resolved.normalizedParam;
    if (cur === want) return;
    const next = new URLSearchParams(params);
    if (want === null) next.delete("connection");
    else next.set("connection", want);
    setParams(next, { replace: true });
  }, [epReady, resolved.normalizedParam, params, setParams]);

  const selectConnection = (v: string | null) => {
    const next = new URLSearchParams(params);
    if (v === null) next.delete("connection");
    else next.set("connection", v);
    setParams(next);
  };

  const gotoTab = (t: string) => {
    const target: WorkspaceTab = TAB_PARAM[t] ?? "overview";
    // 跨 tab 保留 connection 参数，由目标 tab 的解析规则决定沿用/回落/归一。
    const qs = params.toString();
    nav(`/devices/${deviceId}/${target}${qs ? `?${qs}` : ""}`);
  };

  const singleTab = isSingleConnectionTab(activeTab);
  const effectiveId =
    resolved.effective.kind === "single" ? resolved.effective.endpointId : null;
  const activeEndpoint = endpoints.find((e) => e.id === effectiveId) ?? null;
  // Header 连接数只数归属当前设备的连接（endpoints 全量里可能含其它设备，
  // M1 只取当前设备时此处 endpointIds 即全量，保持一致即可）。
  const deviceEndpoints = useMemo(
    () => endpoints.filter((e) => (e.device_id ?? "") === deviceId),
    [endpoints, deviceId],
  );

  // 404 视图必须在全部 hook 之后 early return：notFound 由异步请求后置，
  // 提前 return 会让本次渲染的 hook 数少于上次（Rendered fewer hooks）。
  const notFoundView = notFound ? (
    <Card size="small" title="设备不存在">
      <p style={{ color: "#525252" }}>设备 `{deviceId}` 不存在，可能已被删除。</p>
      <Button type="primary" onClick={() => nav("/devices")}>返回设备列表</Button>
    </Card>
  ) : null;

  const renderTab = (t: WorkspaceTab) => {
    if (t === "overview") {
      return (
        <DeviceOverview
          deviceId={deviceId}
          deviceName={device?.name ?? deviceId}
          endpoints={deviceEndpoints}
          endpointIds={endpointIds}
          points={data.devicePoints}
          counts={data.counts}
          onOpenPoint={setOpenPoint}
        />
      );
    }
    if (t === "data") {
      return (
        <DeviceLiveData
          endpoints={deviceEndpoints}
          effectiveEndpointId={effectiveId}
          points={data.devicePoints}
          pointsError={data.pointsError}
          onOpenPoint={setOpenPoint}
          onSelectConnection={selectConnection}
        />
      );
    }
    if (t === "events") {
      return (
        <DeviceEvents
          deviceId={deviceId}
          deviceName={device?.name ?? deviceId}
          endpointIds={endpointIds}
          endpointNames={new Map(deviceEndpoints.map((e) => [e.id, e.name ?? e.id]))}
          effectiveEndpointId={effectiveId}
        />
      );
    }
    // M6：config/diagnostics 已实现（设备改名/连接增删改/单连接设置/诊断）。
    if (t === "config") {
      return (
        <DeviceConfig
          deviceId={deviceId}
          device={device}
          endpoints={deviceEndpoints}
          effectiveEndpointId={effectiveId}
          onReload={data.reloadInventory}
        />
      );
    }
    if (t === "diagnostics") {
      return (
        <DeviceDiagnostics
          endpoints={deviceEndpoints}
          effectiveEndpointId={effectiveId}
          points={data.devicePoints}
          counts={data.counts}
        />
      );
    }
    return (
      <WorkspaceTabPlaceholder
        tab={t}
        deviceId={deviceId}
        deviceName={device?.name ?? deviceId}
        endpoints={deviceEndpoints}
        effective={resolved.effective}
        activeEndpoint={activeEndpoint}
      />
    );
  };

  const tabItems = WORKSPACE_TABS.map((t) => ({
    key: t,
    label: TAB_LABEL[t],
    children: renderTab(t),
  }));

  // notFound 视图：hook 之后才分支返回，保证每次渲染 hook 数一致。
  if (notFoundView) return notFoundView;

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div style={{ fontSize: 12, color: "#525252" }}>
        <Link to="/devices">设备</Link> {" > "} {device?.name ?? deviceId}
      </div>

      <Card
        size="small"
        title={
          <Space>
            <span>{device?.name ?? deviceId}</span>
            <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, color: "#525252" }}>
              {deviceId}
            </span>
          </Space>
        }
      >
        <div style={{ fontSize: 12, color: "#525252" }}>
          <span data-testid="workspace-connection-count">{deviceEndpoints.length} 个连接</span>
          {epReady ? (
            resolved.effective.kind === "single" && activeEndpoint
              ? <span data-testid="workspace-connection-context"> · 当前上下文：{activeEndpoint.name ?? activeEndpoint.id}</span>
              : resolved.effective.kind === "all"
                ? <span data-testid="workspace-connection-context"> · 当前上下文：全部连接</span>
                : null
          ) : (
            <span style={{ marginLeft: 8 }}>连接加载中…</span>
          )}
        </div>
      </Card>

      {epError ? <Alert type="warning" showIcon message="连接清单不可用" description={epError} /> : null}

      <Card size="small" title="连接">
        <Space wrap>
          {/* 观察类 tab 才有“全部”；配置类 tab 必须单选（规则见 connection.ts）。 */}
          {!singleTab && (
            <Button
              size="small"
              type={resolved.effective.kind === "all" ? "primary" : "default"}
              onClick={() => selectConnection(null)}
            >
              全部
            </Button>
          )}
          {deviceEndpoints.map((e) => {
            const active = effectiveId === e.id;
            return (
              <Button
                key={e.id}
                size="small"
                type={active ? "primary" : "default"}
                onClick={() => selectConnection(e.id)}
              >
                {e.name ?? e.id}{" "}
                <Tag
                  color={isRunningState(e.state) ? "green" : "default"}
                  style={{ marginLeft: 4 }}
                >
                  ●
                </Tag>
              </Button>
            );
          })}
          <Button size="small" onClick={() => nav(`/devices/${deviceId}/config`)}>
            + 添加连接
          </Button>
        </Space>
        {!endpoints.length && !epError ? (
          <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
            该设备暂无连接。M2 起连接的新增/编辑收敛到「配置」页。
          </div>
        ) : null}
      </Card>

      <Card size="small">
        <Tabs activeKey={activeTab} onChange={gotoTab} items={tabItems} />
      </Card>

      {deviceError ? (
        <Alert type="error" showIcon message="设备加载失败" description={deviceError} />
      ) : null}

      <PointDetailDrawer
        deviceId={deviceId}
        deviceName={device?.name ?? deviceId}
        point={openPoint}
        onClose={() => setOpenPoint(null)}
        // P2 改名：同步快照（表格双行即时更新）+ 刷新 Drawer 内展示。
        onRenamed={(display_name) => {
          if (!openPoint) return;
          const key = openPoint.key ?? openPoint.point_key ?? "";
          data.patchDisplayName(openPoint.endpoint_id, String(key), display_name);
          setOpenPoint({ ...openPoint, display_name: display_name ?? undefined });
        }}
      />
    </div>
  );
}

/** 兜底占位（理论不可达：五个 tab 均已实现；未知 tab 回 overview）。 */
function WorkspaceTabPlaceholder(props: {
  tab: WorkspaceTab;
  deviceId: string;
  deviceName: string;
  endpoints: WorkspaceEndpoint[];
  effective: { kind: string; endpointId?: string };
  activeEndpoint: WorkspaceEndpoint | null;
}) {
  const { tab, deviceId, deviceName, endpoints, effective, activeEndpoint } = props;
  const scope =
    effective.kind === "single"
      ? `连接 ${activeEndpoint?.name ?? (effective as { endpointId: string }).endpointId}`
      : effective.kind === "none"
        ? "暂无连接"
        : `全部连接（${endpoints.length} 个）`;
  const body: Record<WorkspaceTab, string> = {
    overview: `不应出现：overview 已实现。`,
    data: `不应出现：data 已实现。`,
    events: `不应出现：events 已实现。`,
    config: `不应出现：config 已在 M6 实现。`,
    diagnostics: `不应出现：diagnostics 已在 M6 实现。`,
  };
  return (
    <div style={{ display: "grid", gap: 8 }}>
      <div style={{ fontSize: 12, color: "#525252" }}>
        {deviceName} <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>{deviceId}</span>
        {" · "}{scope}
      </div>
      <Alert type="info" showIcon message={TAB_LABEL[tab]} description={body[tab]} />
      {effective.kind === "none" && (tab === "config" || tab === "diagnostics") ? (
        <Alert type="warning" showIcon message="该设备暂无连接" description="请先添加连接后再配置或诊断。" />
      ) : null}
    </div>
  );
}
