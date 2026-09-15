// 设备 Header：设备唯一 Workspace 的身份区。
// 只展示真实聚合事实（连接数/逐状态计数），不合成“健康评分”，
// 不合并状态语义（FAILED≠STOPPED，RECONNECTING≠RUNNING）。
// 点位数不展示：devicePoints 是 latest 快照数，不是已配置总数，
// “不知道”不显示成精确总数。设备改名/删除走 DeviceEditorModal。
import { Button, Space } from "antd";
import { useState } from "react";
import { Link } from "react-router-dom";
import type { WorkspaceEndpoint } from "./useDeviceWorkspaceData";
import { DeviceEditorModal } from "../components/DeviceEditorModal";

export function DeviceHeader(props: {
  deviceId: string;
  deviceName: string;
  endpoints: WorkspaceEndpoint[];
  /** 改名/删除后刷新（Header 名称实时跟上）。 */
  onChanged: () => void;
}) {
  const { deviceId, deviceName, endpoints, onChanged } = props;
  const [editOpen, setEditOpen] = useState(false);
  // 真实状态计数：只渲染非零状态，绝不做 total-running=stopped 合并。
  const byState = new Map<string, number>();
  for (const e of endpoints) {
    const s = (e.state ?? "UNKNOWN").toUpperCase();
    byState.set(s, (byState.get(s) ?? 0) + 1);
  }
  const stateText = [...byState.entries()]
    .sort(([a], [b]) => (a < b ? -1 : 1))
    .map(([s, n]) => `${n} ${s}`)
    .join(" · ");

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
        {endpoints.length} 个连接{stateText ? ` · ${stateText}` : null}
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
