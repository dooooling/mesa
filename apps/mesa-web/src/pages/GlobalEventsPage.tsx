// M3.4 GlobalEvents：全部设备事件聚合（V2 风格）。
// - 数据源 useEventFeed（与旧 EventsView 同机制：冻结 H→历史→SSE→merge）；
// - 设备/连接两级下拉进 URL（?device=&connection=，级联+归一，与 /data 同规则）；
// - 其余过滤（Active/时间/高级）M3.5 与设备事件统一接入，此处先跑通聚合与跳转；
// - 表格/Drawer 共用；行内“打开设备”闭环回所属设备事件页。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Select, Space, Tag } from "antd";
import { useNavigate, useSearchParams } from "react-router-dom";
import { api } from "../api";
import type { StoredEvent } from "../types";
import { EMPTY_EVENT_FILTER_FORM, type ActiveFilter, type EventFilterForm } from "../events/filters";
import { useEventFeed } from "../events/useEventFeed";
import { EventTable } from "../components/EventTable";
import { EventDetailDrawer } from "../components/EventDetailDrawer";
import { EventDiagnostics } from "../components/EventDiagnostics";

export function GlobalEventsPage() {
  const nav = useNavigate();
  const [params, setParams] = useSearchParams();
  const [devices, setDevices] = useState<Array<{ id: string; name: string }>>([]);
  const [endpoints, setEndpoints] = useState<Array<{ id: string; device_id?: string }>>([]);
  const [active, setActive] = useState<ActiveFilter>("all");
  const [selected, setSelected] = useState<StoredEvent | null>(null);

  const deviceParam = params.get("device") ?? "ALL";
  const connectionParam = params.get("connection") ?? "ALL";

  useEffect(() => {
    api
      .listDevices()
      .then((j) => setDevices(((j as { devices?: Array<{ id: string; name: string }> }).devices ?? [])))
      .catch(() => {});
    api
      .listEndpoints()
      .then((j) =>
        setEndpoints(
          ((j as { endpoints?: Array<{ id: string; device_id?: string }> }).endpoints ?? []).map((e) => ({
            id: e.id,
            device_id: e.device_id,
          })),
        ),
      )
      .catch(() => {});
  }, []);

  const endpointOptions = useMemo(() => {
    const list = deviceParam === "ALL" ? endpoints : endpoints.filter((e) => (e.device_id ?? "") === deviceParam);
    return [{ value: "ALL", label: "全部连接" }, ...list.map((e) => ({ value: e.id, label: e.id }))];
  }, [endpoints, deviceParam]);

  // URL 归一（清单未就绪前不动；设备切换连接回 ALL；非法值/错配回 ALL）。
  const endpointsReady = devices.length > 0 || endpoints.length > 0;
  useEffect(() => {
    if (!endpointsReady) return;
    const deviceIds = new Set(devices.map((d) => d.id));
    const endpointIds = new Set(endpoints.map((e) => e.id));
    const next = new URLSearchParams(params);
    let changed = false;
    if (deviceParam !== "ALL" && !deviceIds.has(deviceParam)) {
      next.delete("device");
      changed = true;
    }
    if (connectionParam !== "ALL" && !endpointIds.has(connectionParam)) {
      next.delete("connection");
      changed = true;
    }
    if (!changed && deviceParam !== "ALL" && connectionParam !== "ALL") {
      const ep = endpoints.find((e) => e.id === connectionParam);
      if (ep && (ep.device_id ?? "") !== deviceParam) {
        next.delete("connection");
        changed = true;
      }
    }
    if (changed) setParams(next, { replace: true });
  }, [endpointsReady, devices, endpoints, deviceParam, connectionParam, params, setParams]);

  const setDevice = (v: string) => {
    const next = new URLSearchParams(params);
    if (v === "ALL") next.delete("device");
    else next.set("device", v);
    next.delete("connection");
    setParams(next);
  };
  const setConnection = (v: string) => {
    const next = new URLSearchParams(params);
    if (v === "ALL") next.delete("connection");
    else next.set("connection", v);
    setParams(next);
  };

  // 后端过滤：connection 有值即 endpoint_id；device 只做连接级联（后端无
  // device_id 过滤——全局页是单源 endpoint/global 查询，不存在设备分页问题；
  // 若 device 已选而 connection 为 ALL，后端查全局，前端按 device 归属过滤）。
  const form: EventFilterForm = useMemo(
    () => ({
      ...EMPTY_EVENT_FILTER_FORM,
      active,
      endpoint_id: connectionParam !== "ALL" ? connectionParam : undefined,
    }),
    [active, connectionParam],
  );
  const feed = useEventFeed(form);
  const { history, nextCursor, loading, loadingMore, unavailable, error, liveOn } = feed;

  // device 限定的前端归属过滤（后端无 device 过滤时）：endpoint→device 查表。
  const deviceOf = useMemo(() => {
    const m = new Map<string, string>();
    for (const e of endpoints) m.set(e.id, e.device_id ?? "");
    return m;
  }, [endpoints]);
  const visible = useMemo(() => {
    const onFilterChange = (_: EventFilterForm) => {};
    void onFilterChange;
    if (deviceParam === "ALL") return history;
    return history.filter((ev) => {
      const owner = deviceOf.get(ev.endpoint_id) ?? "";
      // 归属未知时 fail-closed 保留（清单缺失），已知且非所选才排除。
      if (!owner) return true;
      return owner === deviceParam;
    });
  }, [history, deviceParam, deviceOf]);

  const statusTag = useMemo(() => {
    if (!liveOn) return <Tag>PAUSED</Tag>;
    if (feed.streamStatus === "live") return <Tag color="green">LIVE ●</Tag>;
    if (feed.streamStatus === "reconnecting") return <Tag color="orange">RECONNECTING</Tag>;
    if (feed.streamStatus === "connecting") return <Tag color="blue">CONNECTING</Tag>;
    return <Tag>{feed.streamStatus.toUpperCase()}</Tag>;
  }, [liveOn, feed.streamStatus]);

  const openDeviceOf = (ev: StoredEvent) => {
    const deviceId = deviceOf.get(ev.endpoint_id) ?? "";
    if (!deviceId) return;
    nav(`/devices/${deviceId}/events?connection=${ev.endpoint_id}`);
  };

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <Card
        size="small"
        title={
          <Space>
            <span>事件</span>
            {statusTag}
          </Space>
        }
        extra={
          <Space wrap>
            <Select
              value={deviceParam}
              onChange={setDevice}
              style={{ width: 160 }}
              options={[{ value: "ALL", label: "全部设备" }, ...devices.map((d) => ({ value: d.id, label: d.name ?? d.id }))]}
            />
            <Select value={connectionParam} onChange={setConnection} style={{ width: 160 }} options={endpointOptions} />
            <Select
              value={active}
              onChange={setActive}
              style={{ width: 140 }}
              options={[
                { value: "all", label: "全部状态" },
                { value: "active", label: "Active" },
                { value: "inactive", label: "已清除" },
              ]}
            />
            <Button size="small" onClick={() => feed.setLiveOn(!liveOn)}>
              {liveOn ? "暂停实时" : "恢复实时"}
            </Button>
          </Space>
        }
      >
        {unavailable ? (
          <Alert type="error" showIcon message="Event service unavailable" description="EventStore 当前不可用。" />
        ) : null}
        {error ? <Alert type="error" showIcon message="加载失败" description={error} style={{ marginTop: unavailable ? 8 : 0 }} /> : null}
        <div style={{ marginTop: 8 }}>
          <EventDiagnostics stats={feed.stats} />
        </div>
        <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
          高级筛选（Category/Kind/Code/Condition/Severity/时间）在 M3.5 与设备事件统一接入。
        </div>
        <div style={{ marginTop: 8, display: "grid", gap: 12 }}>
          <EventTable events={visible} loading={loading} onSelect={setSelected} />
          <div style={{ display: "flex", justifyContent: "center" }}>
            <Button onClick={feed.loadOlder} loading={loadingMore} disabled={nextCursor === null || nextCursor === undefined}>
              {nextCursor === null || nextCursor === undefined ? "没有更多" : "加载更早"}
            </Button>
          </div>
        </div>
      </Card>
      <EventDetailDrawer
        event={selected}
        onClose={() => setSelected(null)}
        extra={
          selected ? (
            <Button size="small" type="link" disabled={!(deviceOf.get(selected.endpoint_id) ?? "")} onClick={() => openDeviceOf(selected)}>
              打开设备 →
            </Button>
          ) : null
        }
      />
    </div>
  );
}
