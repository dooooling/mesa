// M2 PointDetailDrawer：点击数据行打开，不跳页面。
// - endpoint_id 只出现在“来源”下，不做主信息（Endpoint 是连接的后端映射）；
// - 只有 /points/latest 快照时不画假趋势；历史/趋势后续再加；
// - [打开连接配置]/[诊断] 进入同 Workspace 的对应 tab，保留 ?connection=。
import { Button, Descriptions, Drawer, Space, Tag } from "antd";
import { useNavigate } from "react-router-dom";
import { formatAge, formatPointValue } from "../deviceModel";
import type { DevicePointView } from "../workspace/useDeviceWorkspaceData";

function formatNsTime(ns: unknown): string {
  const n = typeof ns === "number" ? ns : Number(ns);
  if (!Number.isFinite(n) || n <= 0) return "—";
  try {
    return new Date(Math.floor(n / 1e6)).toLocaleString();
  } catch {
    return "—";
  }
}

export function PointDetailDrawer(props: {
  deviceId: string;
  deviceName: string;
  point: DevicePointView | null;
  onClose: () => void;
}) {
  const { deviceId, deviceName, point, onClose } = props;
  const nav = useNavigate();

  const gotoConfig = () => {
    if (!point) return;
    onClose();
    nav(`/devices/${deviceId}/config?connection=${point.endpoint_id}`);
  };
  const gotoDiagnostics = () => {
    if (!point) return;
    onClose();
    nav(`/devices/${deviceId}/diagnostics?connection=${point.endpoint_id}`);
  };

  return (
    <Drawer
      title={point ? point.displayKey : "数据点"}
      open={!!point}
      onClose={onClose}
      width={480}
    >
      {!point ? null : (
        <div style={{ display: "grid", gap: 16 }}>
          <div style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12, color: "#525252" }}>
            {point.key ?? point.point_key ?? ""}
          </div>

          <section>
            <h4>当前值</h4>
            <div style={{ fontSize: 20 }}>{formatPointValue(point.value)}</div>
          </section>

          <section>
            <h4>状态</h4>
            <Space>
              <Tag color={point.derived === "GOOD" ? "green" : point.derived === "BAD" ? "red" : "orange"}>
                {point.derived}
              </Tag>
              <span style={{ fontSize: 12, color: "#525252" }}>{formatAge(point.ageMs)}未更新</span>
            </Space>
          </section>

          <section>
            <h4>数据类型</h4>
            <Tag>{point.type ?? "—"}</Tag>
          </section>

          <section>
            <h4>来源</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="设备">{deviceName}</Descriptions.Item>
              <Descriptions.Item label="连接">{point.endpointName}</Descriptions.Item>
              <Descriptions.Item label="Endpoint">
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {point.endpoint_id}
                </span>
              </Descriptions.Item>
              <Descriptions.Item label="Point">
                <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace" }}>
                  {point.key ?? point.point_key ?? point.point_id}
                </span>
              </Descriptions.Item>
            </Descriptions>
          </section>

          <section>
            <h4>时间</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="采集时间">{formatNsTime(point.timestamp_ns)}</Descriptions.Item>
              <Descriptions.Item label="距现在">{formatAge(point.ageMs)}</Descriptions.Item>
            </Descriptions>
          </section>

          <Space>
            <Button type="primary" onClick={gotoConfig}>打开连接配置</Button>
            <Button onClick={gotoDiagnostics}>诊断</Button>
          </Space>
        </div>
      )}
    </Drawer>
  );
}
