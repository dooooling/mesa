// PR8 事件诊断：无需新后端 API；页面顶部轻量状态 + Diagnostics Drawer 全量计数。
import { useState } from "react";
import { Button, Descriptions, Drawer, Space, Tag } from "antd";
import type { EventStats } from "../types";
import { formatBytes } from "../events/format";

function integrityError(s: EventStats): boolean {
  return s.ingress_regressions_total > 0 || s.ingress_collisions_total > 0 || s.ingress_store_failures_total > 0;
}

export function EventDiagnostics({ stats }: { stats: EventStats | null }) {
  const [open, setOpen] = useState(false);
  if (!stats) return <div style={{ fontSize: 12, color: "#888" }}>诊断加载中…</div>;
  const bad = integrityError(stats);
  return (
    <div>
      <Space wrap>
        <span style={{ fontSize: 12, color: "#888" }}>
          Store {stats.stored_rows.toLocaleString()} events · {formatBytes(stats.stored_size_bytes)}
        </span>
        <span style={{ fontSize: 12, color: "#888" }}>Live {stats.live_clients} client{stats.live_clients === 1 ? "" : "s"}</span>
        {bad ? (
          <Tag color="red">
            Integrity ERROR · regressions {stats.ingress_regressions_total} · collisions {stats.ingress_collisions_total} ·
            store failures {stats.ingress_store_failures_total}
          </Tag>
        ) : (
          <Tag color="green">
            Integrity OK · gaps {stats.ingress_gaps_total} · regressions 0 · collisions 0 · store failures 0
          </Tag>
        )}
        <Button size="small" onClick={() => setOpen(true)}>
          诊断详情
        </Button>
      </Space>
      <Drawer title="Event Diagnostics" open={open} onClose={() => setOpen(false)} width={560}>
        <div style={{ display: "grid", gap: 16 }}>
          <section>
            <h4>SSE</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="sse_lagged_total">{stats.sse_lagged_total}</Descriptions.Item>
              <Descriptions.Item label="sse_replay_frames_total">{stats.sse_replay_frames_total}</Descriptions.Item>
              <Descriptions.Item label="sse_reconcile_total">{stats.sse_reconcile_total}</Descriptions.Item>
              <Descriptions.Item label="live_clients">{stats.live_clients}</Descriptions.Item>
            </Descriptions>
          </section>
          <section>
            <h4>Ingress</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="ingress_batches_total">{stats.ingress_batches_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_persisted_events_total">{stats.ingress_persisted_events_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_batch_duplicates_total">{stats.ingress_batch_duplicates_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_event_duplicates_total">{stats.ingress_event_duplicates_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_gaps_total">{stats.ingress_gaps_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_regressions_total">{stats.ingress_regressions_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_collisions_total">{stats.ingress_collisions_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_invalid_total">{stats.ingress_invalid_total}</Descriptions.Item>
              <Descriptions.Item label="ingress_store_failures_total">{stats.ingress_store_failures_total}</Descriptions.Item>
            </Descriptions>
          </section>
          <section>
            <h4>Store / Retention</h4>
            <Descriptions size="small" column={1} bordered>
              <Descriptions.Item label="stored_rows">{stats.stored_rows}</Descriptions.Item>
              <Descriptions.Item label="stored_size_bytes">{stats.stored_size_bytes}</Descriptions.Item>
              <Descriptions.Item label="retention_purged_total">{stats.retention_purged_total}</Descriptions.Item>
            </Descriptions>
          </section>
        </div>
      </Drawer>
    </div>
  );
}
