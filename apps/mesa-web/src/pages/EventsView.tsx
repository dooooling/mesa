// EventsView：全局事件观察面（历史 + SSE 无窗口合并 + 诊断）。
// 事件订阅配置属于 Endpoint 管理面，已移至 Endpoint Workspace「事件」页，
// 本页不再承载（contract 分开：观察 vs 配置）。
// M3.2：数据机制已抽到 useEventFeed（历史/SSE/代际/merge 原样复用），本页只剩
// UI 装配（过滤表单 + 表格 + Drawer + 诊断 + endpoint 下拉）。
import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Card, Space, Tag } from "antd";
import { api } from "../api";
import type { StoredEvent } from "../types";
import { EMPTY_EVENT_FILTER_FORM, type EventFilterForm } from "../events/filters";
import { useEventFeed } from "../events/useEventFeed";
import { EventFilters } from "../components/EventFilters";
import { EventTable } from "../components/EventTable";
import { EventDetailDrawer } from "../components/EventDetailDrawer";
import { EventDiagnostics } from "../components/EventDiagnostics";

/** SSE 实时事件的客户端过滤（服务端 live 无过滤参数；语义与后端 SQL 对齐：精确匹配 + active NULL 不参与）。 */
export function matchesLiveFilter(ev: StoredEvent, form: EventFilterForm): boolean {
  if (form.endpoint_id && form.endpoint_id.trim() !== "" && ev.endpoint_id !== form.endpoint_id.trim()) return false;
  if (form.category && form.category.trim() !== "" && ev.event.category !== form.category.trim()) return false;
  if (form.kind && form.kind.trim() !== "" && ev.event.kind !== form.kind.trim()) return false;
  if (form.code && form.code.trim() !== "" && (ev.event.code ?? "") !== form.code.trim()) return false;
  if (form.condition_id && form.condition_id.trim() !== "" && (ev.event.condition?.condition_id ?? "") !== form.condition_id.trim()) return false;
  if (typeof form.severity_min === "number" && ev.event.severity < form.severity_min) return false;
  if (form.active === "active" && ev.event.condition?.active !== true) return false;
  if (form.active === "inactive" && ev.event.condition?.active !== false) return false;
  if (typeof form.from_ns === "number" && ev.received_at_ns < form.from_ns) return false;
  if (typeof form.to_ns === "number" && ev.received_at_ns > form.to_ns) return false;
  return true;
}

export function EventsView() {
  const [form, setForm] = useState<EventFilterForm>(EMPTY_EVENT_FILTER_FORM);
  const [endpoints, setEndpoints] = useState<string[]>([]);
  const [selected, setSelected] = useState<StoredEvent | null>(null);
  // form 对象身份即 feed 的过滤身份：每次 onChange 产生新对象并 reload，
  // 与旧“setForm + reloadHistory”语义一致。
  const feed = useEventFeed(form);
  const { history, nextCursor, loading, loadingMore, unavailable, error, liveOn } = feed;

  // Endpoint 下拉（事件页独立加载，失败不阻塞事件主体）
  useEffect(() => {
    api
      .listEndpoints()
      .then((j) => {
        const eps = (j.endpoints ?? []) as { id: string }[];
        setEndpoints(eps.map((e) => e.id));
      })
      .catch(() => {});
  }, []);

  // M3.2：form 对象身份即过滤身份。旧语义“setForm + reloadHistory”收敛为
  // “setForm 即新 feed”：feed 内部首屏只跑一次挂载，过滤变化走 reload。
  // 注意 useEventFeed 的首屏 effect 依赖 []，form 变化不会重跑首屏，必须显式 reload。
  const onFilterChange = (next: EventFilterForm) => {
    setForm(next);
    feed.reload(next);
  };

  const statusTag = useMemo(() => {
    if (!liveOn) return <Tag>PAUSED</Tag>;
    if (feed.streamStatus === "live") return <Tag color="green">LIVE ●</Tag>;
    if (feed.streamStatus === "reconnecting") return <Tag color="orange">RECONNECTING</Tag>;
    if (feed.streamStatus === "connecting") return <Tag color="blue">CONNECTING</Tag>;
    return <Tag>{feed.streamStatus.toUpperCase()}</Tag>;
  }, [liveOn, feed.streamStatus]);

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
          <Space>
            <Button size="small" onClick={() => feed.setLiveOn(!liveOn)}>
              {liveOn ? "暂停实时" : "恢复实时"}
            </Button>
          </Space>
        }
      >
        {unavailable ? (
          <Alert type="error" showIcon message="Event service unavailable" description="EventStore 当前不可用；设备、监控、Data Plane 页面继续正常。事件任务可读取配置，但 Event-enabled Endpoint 无法启动。" />
        ) : null}
        {error ? <Alert type="error" showIcon message="加载失败" description={error} style={{ marginTop: unavailable ? 8 : 0 }} /> : null}
        <div style={{ marginTop: 8 }}>
          <EventDiagnostics stats={feed.stats} />
        </div>
        <div style={{ marginTop: 8, fontSize: 12, color: "#525252" }}>
          事件订阅配置请到“设备 → 连接 → 事件”页管理，本页只做全局观察。
        </div>
        <div style={{ marginTop: 8, display: "grid", gap: 12 }}>
          <EventFilters
            value={form}
            endpoints={endpoints}
            onChange={onFilterChange}
            onReset={() => onFilterChange(EMPTY_EVENT_FILTER_FORM)}
          />
          <EventTable events={history} loading={loading} onSelect={setSelected} />
          <div style={{ display: "flex", justifyContent: "center" }}>
            <Button onClick={feed.loadOlder} loading={loadingMore} disabled={nextCursor === null || nextCursor === undefined}>
              {nextCursor === null || nextCursor === undefined ? "没有更多" : "加载更早"}
            </Button>
          </div>
        </div>
      </Card>
      <EventDetailDrawer event={selected} onClose={() => setSelected(null)} />
    </div>
  );
}
