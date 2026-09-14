// M4.3 System：平台级状态页（只展示真实可确认的状态）。
// - Mesa Core：version + uptime（/diagnostics 可证明）；
// - Drivers：已发现列表（/drivers 可证明）→ AVAILABLE，不写 HEALTHY；
// - 存储/事件服务：eventStats 可用即 AVAILABLE，不可用即 UNAVAILABLE
//   （503 EVENT_STORE_UNAVAILABLE），失败原因明示；
// - 未知一律标未知，不写正常。
import { useEffect, useState } from "react";
import { Alert, Card, Descriptions, Table, Tag } from "antd";
import { api, isEventStoreUnavailable } from "../api";
import type { EventStats } from "../types";

interface Diagnostics {
  version?: string;
  uptime_secs?: number;
  drivers?: { count?: number };
  devices?: { stored?: number };
  endpoints?: { stored?: number; runtime?: number };
  certificates?: unknown;
}

function fmtUptime(secs?: number): string {
  if (typeof secs !== "number" || !Number.isFinite(secs)) return "—";
  const s = Math.floor(secs);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}小时${m}分`;
  if (m > 0) return `${m}分${s % 60}秒`;
  return `${s}秒`;
}

export function SystemPage() {
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [diagError, setDiagError] = useState("");
  const [drivers, setDrivers] = useState<Array<{ id: string; name?: string; version?: string }>>([]);
  const [driversError, setDriversError] = useState("");
  const [stats, setStats] = useState<EventStats | null>(null);
  const [statsError, setStatsError] = useState("");
  const [statsUnavailable, setStatsUnavailable] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .diagnostics()
      .then((d) => {
        if (!cancelled) {
          setDiag(d as Diagnostics);
          setDiagError("");
        }
      })
      .catch((e) => {
        if (!cancelled) setDiagError(e instanceof Error ? e.message : String(e));
      });
    api
      .listDrivers()
      .then((j) => {
        if (!cancelled) {
          setDrivers(((j as { drivers?: Array<{ id: string; name?: string; version?: string }> }).drivers ?? []));
          setDriversError("");
        }
      })
      .catch((e) => {
        if (!cancelled) setDriversError(e instanceof Error ? e.message : String(e));
      });
    api
      .eventStats()
      .then((s) => {
        if (!cancelled) {
          setStats(s);
          setStatsError("");
        }
      })
      .catch((e) => {
        if (cancelled) return;
        if (isEventStoreUnavailable(e)) setStatsUnavailable(true);
        else setStatsError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div style={{ display: "grid", gap: 16 }}>
      <Card size="small" title="Mesa Core">
        {diagError ? (
          <Alert type="error" showIcon message="核心诊断不可用" description={diagError} />
        ) : !diag ? (
          <div style={{ fontSize: 12, color: "#525252" }}>加载中…</div>
        ) : (
          <Descriptions size="small" column={1} bordered>
            <Descriptions.Item label="版本">{diag.version ?? "—"}</Descriptions.Item>
            <Descriptions.Item label="运行时长">{fmtUptime(diag.uptime_secs)}</Descriptions.Item>
            <Descriptions.Item label="设备（存储）">{diag.devices?.stored ?? "—"}</Descriptions.Item>
            <Descriptions.Item label="连接（存储/运行时）">
              {diag.endpoints?.stored ?? "—"} / {diag.endpoints?.runtime ?? "—"}
            </Descriptions.Item>
          </Descriptions>
        )}
      </Card>

      <Card size="small" title={`Drivers · ${drivers.length}`}>
        {driversError ? (
          <Alert type="error" showIcon message="驱动清单不可用" description={driversError} />
        ) : (
          <Table
            size="small"
            rowKey="id"
            pagination={false}
            dataSource={drivers}
            locale={{ emptyText: "暂无已发现驱动" }}
            columns={[
              { title: "驱动", dataIndex: "id", render: (v: string) => <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>{v}</span> },
              { title: "名称", dataIndex: "name", render: (v: string) => v ?? "—" },
              { title: "版本", dataIndex: "version", render: (v: string) => v ?? "—" },
              {
                // API 只能证明 driver 被发现：AVAILABLE，不写 HEALTHY。
                title: "状态",
                render: () => <Tag color="blue">AVAILABLE</Tag>,
              },
            ]}
          />
        )}
      </Card>

      <Card size="small" title="事件服务">
        {statsUnavailable ? (
          <Alert type="error" showIcon message="Event service unavailable" description="EventStore 当前不可用。" />
        ) : statsError ? (
          <Alert type="error" showIcon message="事件统计不可用" description={statsError} />
        ) : !stats ? (
          <div style={{ fontSize: 12, color: "#525252" }}>加载中…</div>
        ) : (
          <Descriptions size="small" column={1} bordered>
            <Descriptions.Item label="状态"><Tag color="green">AVAILABLE</Tag></Descriptions.Item>
            <Descriptions.Item label="已存事件">{(stats as { stored_rows?: number }).stored_rows ?? "—"}</Descriptions.Item>
            <Descriptions.Item label="实时客户端">{(stats as { live_clients?: number }).live_clients ?? "—"}</Descriptions.Item>
          </Descriptions>
        )}
      </Card>
    </div>
  );
}
