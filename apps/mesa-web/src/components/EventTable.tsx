// PR8 事件表：固定基础列；顺序权威是 DB seq DESC（不用 occurred_at 重排）。
import { Table, Tag } from "antd";
import type { StoredEvent } from "../types";
import { formatNsTime, formatSeverity } from "../events/format";

export function EventTable({
  events,
  loading,
  onSelect,
}: {
  events: StoredEvent[];
  loading: boolean;
  onSelect: (ev: StoredEvent) => void;
}) {
  return (
    <Table
      size="small"
      rowKey={(r) => r.seq}
      dataSource={events}
      loading={loading}
      pagination={false}
      onRow={(r) => ({ onClick: () => onSelect(r), style: { cursor: "pointer" } })}
      locale={{ emptyText: "暂无事件 · 请配置 EventTask 并启动 Endpoint" }}
      columns={[
        { title: "Seq", dataIndex: "seq", width: 90, render: (v: number) => <span style={{ fontFamily: "monospace" }}>{v}</span> },
        {
          title: "发生时间",
          width: 190,
          render: (_: unknown, r: StoredEvent) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{formatNsTime(r.event.occurred_at_ns)}</span>,
        },
        {
          title: "接收时间",
          width: 190,
          render: (_: unknown, r: StoredEvent) => <span style={{ fontFamily: "monospace", fontSize: 12 }}>{formatNsTime(r.received_at_ns)}</span>,
        },
        {
          title: "Severity",
          width: 130,
          // P1-2：只显示 0..1000 原值（0 = unknown），颜色也不做 warning/critical 分类
          render: (_: unknown, r: StoredEvent) => <Tag>{formatSeverity(r.event.severity)}</Tag>,
        },
        { title: "Category", dataIndex: ["event", "category"], width: 110 },
        { title: "Kind", dataIndex: ["event", "kind"], width: 160 },
        { title: "Source", dataIndex: ["event", "source"], width: 130 },
        { title: "Code", width: 110, render: (_: unknown, r: StoredEvent) => r.event.code ?? "—" },
        {
          title: "Message",
          ellipsis: true,
          render: (_: unknown, r: StoredEvent) => r.event.message ?? "—",
        },
        {
          title: "Transition",
          width: 130,
          render: (_: unknown, r: StoredEvent) =>
            r.event.condition ? (
              <Tag color={r.event.condition.transition === "raised" ? "red" : r.event.condition.transition === "cleared" ? "green" : "blue"}>
                {r.event.condition.transition}
              </Tag>
            ) : (
              "—"
            ),
        },
      ]}
    />
  );
}
