// 采集一级页：定义什么数据进入 Mesa（周期数据 + 事件数据）。
// 两个 Section（数据采集 / 事件订阅），顶部 ConnectionSelect（单选）。
// EndpointAcquisitionPane 与 EventTaskEditor 为成熟组件，原样保留，
// 只是位置从 DeviceConfig 移到这里。
import { Alert, Card } from "antd";
import { useMemo } from "react";
import { EndpointAcquisitionPane } from "../components/EndpointAcquisitionPane";
import { EventTaskEditor } from "../components/EventTaskEditor";
import { ConnectionSelect } from "./ConnectionSelect";
import { useConnectionQuery } from "./useConnectionQuery";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";

export function DeviceAcquisition(props: {
  endpoints: WorkspaceEndpoint[];
  endpointsReady: boolean;
  /** 变更后刷新 inventory。 */
  onReload: () => void;
}) {
  const { endpoints, endpointsReady, onReload } = props;
  const endpointIds = useMemo(() => endpoints.map((e) => e.id), [endpoints]);
  const { selected, select } = useConnectionQuery({ endpointIds, mode: "single", ready: endpointsReady });
  const active = endpoints.find((e) => e.id === selected) ?? null;

  if (!endpoints.length) {
    return <Alert type="warning" showIcon message="该设备暂无连接" description="请先到「连接」页新增连接后再配置采集。" />;
  }

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <h3 style={{ fontSize: 14, fontWeight: 600, margin: 0 }}>采集</h3>
        <span style={{ marginLeft: "auto" }}>
          <ConnectionSelect endpoints={endpoints} value={selected} allowAll={false} onChange={select} />
        </span>
      </div>
      {active ? (
        <div key={active.id} style={{ display: "grid", gap: 16 }}>
          <Card size="small" title={`数据采集 · ${active.name ?? active.id}`}>
            <EndpointAcquisitionPane endpointId={active.id} driverId={active.driver_id} onChanged={onReload} />
          </Card>
          <Card size="small" title="事件订阅">
            <EventTaskEditor fixedEndpointId={active.id} />
          </Card>
        </div>
      ) : null}
    </div>
  );
}
