// PR8 事件详情抽屉：5 块展示；acknowledged/confirmed 只是状态展示，绝无 [Acknowledge] 按钮。
import { Descriptions, Drawer, Table, Tag } from "antd";
import type { StoredEvent } from "../types";
import { formatMesaValue, formatNsTime, formatSeverity } from "../events/format";

export function EventDetailDrawer({ event, onClose }: { event: StoredEvent | null; onClose: () => void }) {
  return (
    <Drawer title={event ? `Event #${event.seq}` : "Event"} open={!!event} onClose={onClose} width={560}>
      {!event ? null : (
        <div style={{ display: "grid", gap: 16 }}>
          <section>
            <h4>Identity</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="Store seq">{event.seq}</Descriptions.Item>
              <Descriptions.Item label="event_id"><span style={{ fontFamily: "monospace" }}>{event.event.event_id}</span></Descriptions.Item>
              <Descriptions.Item label="endpoint_id"><span style={{ fontFamily: "monospace" }}>{event.endpoint_id}</span></Descriptions.Item>
              <Descriptions.Item label="category">{event.event.category}</Descriptions.Item>
              <Descriptions.Item label="kind"><span style={{ fontFamily: "monospace" }}>{event.event.kind}</span></Descriptions.Item>
            </Descriptions>
          </section>
          <section>
            <h4>Source / Message</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="source">{event.event.source || "—"}</Descriptions.Item>
              <Descriptions.Item label="severity">{formatSeverity(event.event.severity)}</Descriptions.Item>
              <Descriptions.Item label="code">{event.event.code ?? "—"}</Descriptions.Item>
              <Descriptions.Item label="message">{event.event.message ?? "—"}</Descriptions.Item>
              <Descriptions.Item label="message_locale">{event.event.message_locale ?? "—"}</Descriptions.Item>
            </Descriptions>
          </section>
          <section>
            <h4>Time（occurred / published / received 明确分开）</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="occurred_at">{formatNsTime(event.event.occurred_at_ns)}</Descriptions.Item>
              <Descriptions.Item label="published_at">{formatNsTime(event.event.published_at_ns)}</Descriptions.Item>
              <Descriptions.Item label="received_at">{formatNsTime(event.received_at_ns)}</Descriptions.Item>
            </Descriptions>
          </section>
          {event.event.condition ? (
            <section>
              <h4>Condition</h4>
              <Descriptions size="small" column={1} bordered>
                <Descriptions.Item label="condition_id"><span style={{ fontFamily: "monospace" }}>{event.event.condition.condition_id}</span></Descriptions.Item>
                <Descriptions.Item label="transition"><Tag>{event.event.condition.transition}</Tag></Descriptions.Item>
                <Descriptions.Item label="active">{String(event.event.condition.active ?? "—")}</Descriptions.Item>
                <Descriptions.Item label="acknowledged">{String(event.event.condition.acknowledged ?? "—")}</Descriptions.Item>
                <Descriptions.Item label="confirmed">{String(event.event.condition.confirmed ?? "—")}</Descriptions.Item>
                <Descriptions.Item label="retain">{String(event.event.condition.retain ?? "—")}</Descriptions.Item>
              </Descriptions>
            </section>
          ) : null}
          <section>
            <h4>Metadata</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="correlation_id"><span style={{ fontFamily: "monospace" }}>{event.event.correlation_id ?? "—"}</span></Descriptions.Item>
              <Descriptions.Item label="connection_handle">{event.event.connection_handle}</Descriptions.Item>
              <Descriptions.Item label="stream_epoch">{event.stream_epoch}</Descriptions.Item>
              <Descriptions.Item label="batch_sequence">{event.batch_sequence}</Descriptions.Item>
            </Descriptions>
            <h4 style={{ marginTop: 12 }}>attributes（通用展示，不解释协议字段）</h4>
            <Table
              size="small"
              pagination={false}
              rowKey={(r) => r.k}
              locale={{ emptyText: "无 attributes" }}
              dataSource={Object.entries(event.event.attributes ?? {}).map(([k, v]) => ({ k, v: formatMesaValue(v) }))}
              columns={[
                { title: "key", dataIndex: "k", width: 180, render: (v: string) => <span style={{ fontFamily: "monospace" }}>{v}</span> },
                { title: "value", dataIndex: "v", render: (v: string) => <span style={{ fontFamily: "monospace" }}>{v}</span> },
              ]}
            />
          </section>
        </div>
      )}
    </Drawer>
  );
}
