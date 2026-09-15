// M3.4 GlobalEvents：全部设备事件聚合（V2 风格）。
// - 数据源 useEventFeed（与旧 EventsView 同机制：冻结 H→历史→SSE→merge）；
// - 设备/连接两级下拉进 URL（?device=&connection=，级联+归一，与 /data 同规则）；
// - 其余过滤（Active/时间/高级）M3.5 与设备事件统一接入，此处先跑通聚合与跳转；
// - 表格/Drawer 共用；行内“打开设备”闭环回所属设备事件页。
import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Card, Select, Space, Tag } from "antd";
import { useNavigate, useSearchParams } from "react-router-dom";
import { api } from "../api";
import type { StoredEvent } from "../types";
import { EMPTY_EVENT_FILTER_FORM, type EventFilterForm } from "../events/filters";
import { useEventFeed } from "../events/useEventFeed";
import { EventFilterBar } from "../components/EventFilterBar";
import { EventTable } from "../components/EventTable";
import { EventDetailDrawer } from "../components/EventDetailDrawer";
import { EventDiagnostics } from "../components/EventDiagnostics";

export function GlobalEventsPage() {
  const nav = useNavigate();
  const [params, setParams] = useSearchParams();
  const [devices, setDevices] = useState<Array<{ id: string; name: string }>>([]);
  const [endpoints, setEndpoints] = useState<Array<{ id: string; device_id?: string }>>([]);
  // M3.5：除 endpoint 外的全部后端过滤（Active/时间/高级）收敛到一个 form，
  // 文本型经 EventFilterBar 400ms debounce 后才进 form（避免每键一次 reload）。
  const [rest, setRest] = useState<EventFilterForm>(EMPTY_EVENT_FILTER_FORM);
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

  // 后端过滤：connection 有值即 endpoint_id；device 有值即 device_id
  // （M5.3 后端映射为 endpoint 集合；M7 起全局页不再客户端归属过滤）。
  const form: EventFilterForm = useMemo(
    () => ({
      ...rest,
      endpoint_id: connectionParam !== "ALL" ? connectionParam : undefined,
      device_id: deviceParam !== "ALL" ? deviceParam : undefined,
    }),
    [rest, connectionParam, deviceParam],
  );
  // deviceOf 映射：SSE live 行归属判定 + Drawer“打开设备”跳转用。
  const deviceOf = useMemo(() => {
    const m = new Map<string, string>();
    for (const e of endpoints) m.set(e.id, e.device_id ?? "");
    return m;
  }, [endpoints]);
  const feed = useEventFeed(form, { deviceOf: (id) => deviceOf.get(id) });
  const { history, nextCursor, loading, loadingMore, unavailable, error, liveOn } = feed;

  // device/connection（URL）变化即 reload（useEventFeed 首屏只查一次，
  // 此前靠客户端 visible 过滤掩盖；M7 后端过滤后必须显式 reload）。
  // 首屏跳过（首屏查询由 hook 承担，此处只响应后续变化，避免双查）。
  const formKey = JSON.stringify(form);
  const firstFormKey = useRef(true);
  useEffect(() => {
    if (firstFormKey.current) {
      firstFormKey.current = false;
      return;
    }
    feed.reload(form);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [formKey]);

  // RC2 修5：reload owner 唯一——formKey effect 是唯一的 reload 发起者。
  // onRestChange 只改 rest（formKey 变化触发 effect reload）。之前两处都
  // reload，用户改一个 filter 发两次请求（debounce 后仍 double）。
  // form 变化即 reload（与旧 EventsView 的 onFilterChange 语义一致）。
  const onRestChange = (next: EventFilterForm) => {
    setRest(next);
  };

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
            <Button size="small" onClick={() => feed.setLiveOn(!liveOn)}>
              {liveOn ? "暂停实时" : "恢复实时"}
            </Button>
          </Space>
        }
      >
        {unavailable ? (
          <Alert
            type="error"
            showIcon
            message="Event service unavailable"
            description="EventStore 当前不可用。恢复后点重试重新建连（重建高水位，不会 replay 大量历史）。"
            action={
              // RC2 修4：unavailable 也有重试（完整 boot → 新 H → SSE 重建）。
              <Button size="small" onClick={() => feed.reload(form)}>
                重试
              </Button>
            }
          />
        ) : null}
        {error ? (
          <Alert
            type="error"
            showIcon
            message="加载失败"
            description={error}
            style={{ marginTop: unavailable ? 8 : 0 }}
            action={<Button size="small" onClick={() => feed.reload(form)}>重试</Button>}
          />
        ) : null}
        <div style={{ marginTop: 8 }}>
          <EventDiagnostics stats={feed.stats} />
        </div>
        <div style={{ marginTop: 8, display: "grid", gap: 12 }}>
          <EventFilterBar
            value={rest}
            onChange={onRestChange}
            onReset={() => onRestChange(EMPTY_EVENT_FILTER_FORM)}
            showConnection
            connectionValue={connectionParam}
            connectionOptions={endpointOptions}
            onConnectionChange={setConnection}
          />
          <EventTable events={history} loading={loading} onSelect={setSelected} />
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
