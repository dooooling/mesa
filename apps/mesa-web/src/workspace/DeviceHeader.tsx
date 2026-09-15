// 设备 Header：设备唯一 Workspace 的身份区。
// 只展示真实聚合事实（连接数/运行分布/点位数），不合成“健康评分”。
// 设备改名/删除走 DeviceEditorModal（编辑），不内联表单。
import { Button, Space } from "antd";
import { useState } from "react";
import { Link } from "react-router-dom";
import { isRunningState } from "../deviceModel";
import type { DevicePointView, WorkspaceEndpoint } from "./useDeviceWorkspaceData";
import { DeviceEditorModal } from "../components/DeviceEditorModal";

export function DeviceHeader(props: {
  deviceId: string;
  deviceName: string;
  endpoints: WorkspaceEndpoint[];
  points: DevicePointView[];
  /** 改名/删除后刷新（Header 名称实时跟上）。 */
  onChanged: () => void;
}) {
  const { deviceId, deviceName, endpoints, points, onChanged } = props;
  const [editOpen, setEditOpen] = useState(false);
  const running = endpoints.filter((e) => isRunningState(e.state)).length;
  const stopped = endpoints.length - running;

  return (
    <div>
      <div style={{ fontSize: 12, color: "#525252" }}>
        <Link to="/devices">设备</Link> {" > "} {deviceName}
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 12, marginTop: 4 }}>
        <span style={{ fontSize: 20, fontWeight: 600 }}>{deviceName}</span>
        <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, color: "#525252" }}>
          {deviceId}
        </span>
        <Space style={{ marginLeft: "auto" }}>
          <Button size="small" onClick={() => setEditOpen(true)}>编辑设备</Button>
        </Space>
      </div>
      <div style={{ fontSize: 12, color: "#525252", marginTop: 4 }}>
        {endpoints.length} 个连接
        {endpoints.length ? ` · ${running} RUNNING · ${stopped} STOPPED` : null}
        {` · ${points.length} 个数据点`}
      </div>
      <DeviceEditorModal
        deviceId={deviceId}
        deviceName={deviceName}
        endpoints={endpoints}
        open={editOpen}
        onClose={() => setEditOpen(false)}
        onChanged={() => { setEditOpen(false); onChanged(); }}
      />
    </div>
  );
}
