// PR8 EventsView：事件记录（历史 + SSE 无窗口合并）与订阅配置。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Card, Space, Tabs, Tag } from "antd";
import { api, isEventStoreUnavailable } from "../api";
import type { EventStats, StoredEvent } from "../types";
import { EMPTY_EVENT_FILTER_FORM, EVENT_FIRST_PAGE_LIMIT, toEventFilter, type EventFilterForm } from "../events/filters";
import { mergeEvents } from "../events/model";
import { useEventStream } from "../events/useEventStream";
import { EventFilters } from "../components/EventFilters";
import { EventTable } from "../components/EventTable";
import { EventDetailDrawer } from "../components/EventDetailDrawer";
import { EventDiagnostics } from "../components/EventDiagnostics";
import { EventTaskEditor } from "../components/EventTaskEditor";

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
  const [history, setHistory] = useState<StoredEvent[]>([]);
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [highWater, setHighWater] = useState<number | null>(null);
  const [booted, setBooted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [liveOn, setLiveOn] = useState(true);
  const [selected, setSelected] = useState<StoredEvent | null>(null);
  const [stats, setStats] = useState<EventStats | null>(null);
  const reqId = useRef(0);
  const formRef = useRef(form);
  formRef.current = form;

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

  // ① 冻结全局高水位 H → ② 当前 filter 历史页 → ③ ?after_seq=H 建 SSE（无窗口）
  useEffect(() => {
    const id = ++reqId.current;
    setLoading(true);
    setError(null);
    api
      .eventHead()
      .then((h) => {
        if (reqId.current !== id) return;
        setHighWater(h);
        setBooted(true);
        return api.listEvents(toEventFilter(formRef.current, { limit: EVENT_FIRST_PAGE_LIMIT })).then((res) => {
          if (reqId.current !== id) return;
          setHistory([...res.events].sort((a, b) => b.seq - a.seq));
          setNextCursor(res.next_cursor);
          setLoading(false);
        });
      })
      .catch((e) => {
        if (reqId.current !== id) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
        setBooted(true);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const reloadHistory = useCallback((next: EventFilterForm) => {
    const id = ++reqId.current;
    setLoading(true);
    setError(null);
    setNextCursor(null);
    api
      .listEvents(toEventFilter(next, { limit: EVENT_FIRST_PAGE_LIMIT }))
      .then((res) => {
        if (reqId.current !== id) return;
        setHistory([...res.events].sort((a, b) => b.seq - a.seq));
        setNextCursor(res.next_cursor);
        setLoading(false);
      })
      .catch((e) => {
        if (reqId.current !== id) return;
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      });
  }, []);

  const onFilterChange = useCallback(
    (next: EventFilterForm) => {
      setForm(next);
      reloadHistory(next);
    },
    [reloadHistory],
  );

  const loadOlder = useCallback(() => {
    if (nextCursor === null || nextCursor === undefined) return;
    setLoadingMore(true);
    api
      .listEvents(toEventFilter(formRef.current, { before_seq: nextCursor, limit: EVENT_FIRST_PAGE_LIMIT }))
      .then((res) => {
        setHistory((cur) => mergeEvents(cur, res.events));
        setNextCursor(res.next_cursor);
        setLoadingMore(false);
      })
      .catch((e) => {
        if (isEventStoreUnavailable(e)) setUnavailable(true);
        else setError(e instanceof Error ? e.message : String(e));
        setLoadingMore(false);
      });
  }, [nextCursor]);

  const onLive = useCallback((ev: StoredEvent) => {
    if (!matchesLiveFilter(ev, formRef.current)) return;
    setHistory((cur) => mergeEvents(cur, [ev]));
  }, []);

  // 诊断轮询（15s；失败静默，页面主体不受影响）
  useEffect(() => {
    let stop = false;
    const tick = () => {
      api
        .eventStats()
        .then((s) => {
          if (!stop) setStats(s);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, 15000);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, []);

  const stream = useEventStream({ afterSeq: highWater, enabled: booted && liveOn && !unavailable, onEvent: onLive });

  const statusTag = useMemo(() => {
    if (!liveOn) return <Tag>PAUSED</Tag>;
    if (stream.status === "live") return <Tag color="green">LIVE ●</Tag>;
    if (stream.status === "reconnecting") return <Tag color="orange">RECONNECTING</Tag>;
    if (stream.status === "connecting") return <Tag color="blue">CONNECTING</Tag>;
    return <Tag>{stream.status.toUpperCase()}</Tag>;
  }, [liveOn, stream.status]);

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
            <Button size="small" onClick={() => setLiveOn((v) => !v)}>
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
          <EventDiagnostics stats={stats} />
        </div>
        <Tabs
          defaultActiveKey="records"
          items={[
            {
              key: "records",
              label: "事件记录",
              children: (
                <div style={{ display: "grid", gap: 12 }}>
                  <EventFilters
                    value={form}
                    endpoints={endpoints}
                    onChange={onFilterChange}
                    onReset={() => onFilterChange(EMPTY_EVENT_FILTER_FORM)}
                  />
                  <EventTable events={history} loading={loading} onSelect={setSelected} />
                  <div style={{ display: "flex", justifyContent: "center" }}>
                    <Button onClick={loadOlder} loading={loadingMore} disabled={nextCursor === null || nextCursor === undefined}>
                      {nextCursor === null || nextCursor === undefined ? "没有更多" : "加载更早"}
                    </Button>
                  </div>
                </div>
              ),
            },
            {
              key: "tasks",
              label: "订阅配置",
              children: <EventTaskEditor />,
            },
          ]}
        />
      </Card>
      <EventDetailDrawer event={selected} onClose={() => setSelected(null)} />
    </div>
  );
}
