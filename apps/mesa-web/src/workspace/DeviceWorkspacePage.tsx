// M1 Device Workspace：V2 骨架。用户进入一台设备后不再离开本页。
// - Header 始终展示 Device 身份（名 + id + 连接数），上下文稳定；
// - Connection selector：观察类 tab 可选全部，配置类 tab 必须单连接，
//   选择经 connection.ts 解析并同步回 URL（`?connection=` 稳定，不漂移）；
// - 五个 tab 在 M1 只给真实框架（M2/M3 填内容），不复制旧页面。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Radio, Space, Tabs, Tag, message } from "antd";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api } from "../api";
import { isRunningState, type Device } from "../deviceModel";
import {
  WORKSPACE_TABS,
  isSingleConnectionTab,
  isWorkspaceTab,
  resolveEffectiveConnection,
  type WorkspaceTab,
} from "./connection";

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
  const [device, setDevice] = useState<Device | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [endpoints, setEndpoints] = useState<EndpointSummary[]>([]);
  const [epError, setEpError] = useState("");
  // 连接清单就绪前不得碰 URL：endpointIds 为空时任何“回落/归一”判定都是
  // 基于不完整信息的误判（例如删掉合法的 ?connection=opcua），即上下文漂移。
  const [epReady, setEpReady] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setNotFound(false);
    setDevice(null);
    setEpReady(false);
    api
      .getDevice(deviceId)
      .then((d) => {
        if (!cancelled) setDevice(d as Device);
      })
      .catch((e: { status?: number }) => {
        if (cancelled) return;
        if (e?.status === 404) setNotFound(true);
        else message.error("加载设备失败");
      });
    api
      .listEndpoints()
      .then((j) => {
        if (cancelled) return;
        const eps = ((j as { endpoints?: Array<EndpointSummary & { runtime?: { state?: string } }> }).endpoints ?? [])
          .filter((e) => e.device_id === deviceId)
          .map((e) => ({
            id: e.id,
            name: e.name ?? e.id,
            driver_id: e.driver_id,
            device_id: e.device_id,
            state: e.state ?? e.runtime?.state,
          }));
        setEndpoints(eps);
        setEpError("");
        setEpReady(true);
      })
      .catch(() => {
        if (!cancelled) {
          setEndpoints([]);
          setEpError("连接清单加载失败：当前连接选择可能不完整，请稍后重试。");
          // 失败同样算“就绪”（信息已完整：确实拿不到清单）：此时 config 的
          // none 空状态与 URL 归一才是诚实的，不再等待。
          setEpReady(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [deviceId]);

  const endpointIds = useMemo(() => endpoints.map((e) => e.id), [endpoints]);
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

  if (notFound) {
    return (
      <Card size="small" title="设备不存在">
        <p style={{ color: "#525252" }}>设备 `{deviceId}` 不存在，可能已被删除。</p>
        <Button type="primary" onClick={() => nav("/devices")}>返回设备列表</Button>
      </Card>
    );
  }

  const singleTab = isSingleConnectionTab(activeTab);
  const effectiveId =
    resolved.effective.kind === "single" ? resolved.effective.endpointId : null;
  const activeEndpoint = endpoints.find((e) => e.id === effectiveId) ?? null;

  const tabItems = WORKSPACE_TABS.map((t) => ({
    key: t,
    label: TAB_LABEL[t],
    children: (
      <WorkspaceTabPlaceholder
        tab={t}
        deviceId={deviceId}
        deviceName={device?.name ?? deviceId}
        endpoints={endpoints}
        effective={resolved.effective}
        activeEndpoint={activeEndpoint}
      />
    ),
  }));

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
          <span data-testid="workspace-connection-count">{endpoints.length} 个连接</span>
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
          {endpoints.map((e) => {
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
    </div>
  );
}

/** M1 占位：五个 tab 的真实框架。M2 填 overview/data，M3 填 events，
 *  M2/M4 填 config（含新增连接），M2 填 diagnostics。 */
function WorkspaceTabPlaceholder(props: {
  tab: WorkspaceTab;
  deviceId: string;
  deviceName: string;
  endpoints: EndpointSummary[];
  effective: { kind: string; endpointId?: string };
  activeEndpoint: EndpointSummary | null;
}) {
  const { tab, deviceId, deviceName, endpoints, effective, activeEndpoint } = props;
  const scope =
    effective.kind === "single"
      ? `连接 ${activeEndpoint?.name ?? (effective as { endpointId: string }).endpointId}`
      : effective.kind === "none"
        ? "暂无连接"
        : `全部连接（${endpoints.length} 个）`;
  const body: Record<WorkspaceTab, string> = {
    overview: `M2 在此实现设备概览（运行状态/数据健康/需要关注/最近事件/最近数据）。`,
    data: `M2 在此实现设备实时数据（默认全部连接，可按连接过滤；点行打开 Drawer）。`,
    events: `M3 在此实现设备事件（Device = ${deviceName} 自动限定，详情进 Drawer）。`,
    config: `M2/M4 在此实现设备配置（设备改名 + 单连接设置/采集/事件订阅 + 新增连接）。`,
    diagnostics: `M2 在此实现设备诊断（单连接状态 + 采集健康 + 高级诊断折叠）。`,
  };
  return (
    <div style={{ display: "grid", gap: 8 }}>
      <div style={{ fontSize: 12, color: "#525252" }}>
        {deviceName} <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>{deviceId}</span>
        {" · "}{scope}
      </div>
      <Radio.Group value={tab} disabled style={{ display: "none" }} />
      <Alert type="info" showIcon message={TAB_LABEL[tab]} description={body[tab]} />
      {effective.kind === "none" && (tab === "config" || tab === "diagnostics") ? (
        <Alert type="warning" showIcon message="该设备暂无连接" description="请先添加连接后再配置或诊断。" />
      ) : null}
    </div>
  );
}
