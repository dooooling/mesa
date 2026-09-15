// M3.3 DeviceEvents：设备 Workspace「事件」tab。
// - Device = 当前设备自动限定（用户不选设备，只选连接/状态/时间等）；
// - 连接复用 M1 的 ?connection= 上下文：ALL 即全部连接，单选即该连接
//   （query.endpointIds 由 Workspace 传入，不在本组件另起 state）；
// - 表格/Drawer 共用（EventTable/EventDetailDrawer），endpoint 列保留
//   （多连接设备需要区分来源），不加设备列（已在设备内）。
// - M3.5：过滤栏升级为 EventFilterBar（常用 Active/时间 + 高级精确匹配，
//   文本 400ms debounce），与全局页同一组件。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Space, Tag } from "antd";
import type { StoredEvent } from "../types";
import { EMPTY_EVENT_FILTER_FORM, type EventFilterForm } from "../events/filters";
import { useDeviceEventFeed } from "../events/useDeviceEventFeed";
import { EventFilterBar } from "../components/EventFilterBar";
import { EventTable } from "../components/EventTable";
import { EventDetailDrawer } from "../components/EventDetailDrawer";

export function DeviceEvents(props: {
  /** scope 唯一身份（Drawer 清空用它，不用展示名——同名设备并存时名称不变）。 */
  deviceId: string;
  deviceName: string;
  endpointIds: string[];
  endpointNames: Map<string, string>;
  /** M1 解析后的有效连接（null = 全部）。 */
  effectiveEndpointId: string | null;
}) {
  const { deviceId, deviceName, endpointIds, endpointNames, effectiveEndpointId } = props;
  const [rest, setRest] = useState<EventFilterForm>(EMPTY_EVENT_FILTER_FORM);
  const [selected, setSelected] = useState<StoredEvent | null>(null);

  // RC2 修2：scope detail state 必须在 scope identity 变化时清空。
  // 用 deviceId（唯一身份），不用 deviceName（同名并存时 A→B 名称不变，
  // effect 不执行，A 的 Drawer 会留在 B 页面）。
  useEffect(() => {
    setSelected(null);
  }, [deviceId]);

  const scopedEndpointIds = useMemo(
    () => (effectiveEndpointId ? [effectiveEndpointId] : endpointIds),
    [effectiveEndpointId, endpointIds],
  );
  const feed = useDeviceEventFeed({ endpointIds: scopedEndpointIds, filter: rest });

  const statusTag = useMemo(() => {
    if (!feed.liveOn) return <Tag>PAUSED</Tag>;
    if (feed.streamStatus === "live") return <Tag color="green">LIVE ●</Tag>;
    if (feed.streamStatus === "reconnecting") return <Tag color="orange">RECONNECTING</Tag>;
    if (feed.streamStatus === "connecting") return <Tag color="blue">CONNECTING</Tag>;
    return <Tag>{feed.streamStatus.toUpperCase()}</Tag>;
  }, [feed.liveOn, feed.streamStatus]);

  void endpointNames;
  void deviceName;

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        {statusTag}
        <Button size="small" onClick={() => feed.setLiveOn(!feed.liveOn)}>
          {feed.liveOn ? "暂停实时" : "恢复实时"}
        </Button>
        <Button size="small" onClick={() => feed.reload()}>
          刷新
        </Button>
      </div>

      {feed.unavailable ? (
        <Alert
          type="error"
          showIcon
          message="Event service unavailable"
          description="EventStore 当前不可用。恢复后点重试重新建连（重建高水位，不会 replay 大量历史）。"
          action={
            // RC2 修4：unavailable 也有重试（完整 boot → 新 H → SSE 重建）。
            <Button size="small" onClick={() => feed.reload()}>
              重试
            </Button>
          }
        />
      ) : null}
      {feed.error ? (
        <Alert
          type="error"
          showIcon
          message="加载失败"
          description={feed.error}
          action={<Button size="small" onClick={() => feed.reload()}>重试</Button>}
        />
      ) : null}
      {!scopedEndpointIds.length && feed.booted ? (
        <Alert type="info" showIcon message="该设备暂无连接" description="事件随连接产生，先添加连接后再查看。" />
      ) : null}

      <EventFilterBar
        value={rest}
        onChange={setRest}
        onReset={() => setRest(EMPTY_EVENT_FILTER_FORM)}
      />
      <Space style={{ fontSize: 12, color: "#525252" }}>
        <span>Device = {deviceName}（自动限定）</span>
      </Space>

      <EventTable events={feed.history} loading={feed.loading} onSelect={setSelected} />
      <div style={{ display: "flex", justifyContent: "center" }}>
        <Button onClick={feed.loadOlder} loading={feed.loadingMore} disabled={!feed.hasMore}>
          {!feed.hasMore ? "没有更多" : "加载更早"}
        </Button>
      </div>
      <EventDetailDrawer event={selected} onClose={() => setSelected(null)} />
    </div>
  );
}
