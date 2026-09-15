// M4.1 OverviewPage：回答“现在有什么需要我处理”。
// - 系统状态行（正常/部分异常/未知，诚实三态，不造 Health Score）；
// - 设备/连接/数据/事件四组计数（失败标 STALE/未知，不写 0）；
// - 需要关注（deriveAttentionItems 纯函数派生，全部直达设备对应 tab）。
import { useMemo } from "react";
import { Alert, Button, Card, Col, Row, Space, Statistic } from "antd";
import { useNavigate } from "react-router-dom";
import { deriveAttentionItems } from "./deriveAttentionItems";
import { useOverviewSnapshot } from "./useOverviewSnapshot";

function fmtCount(unknown: boolean, n: number): string {
  return unknown ? "未知" : String(n);
}

export function OverviewPage() {
  const nav = useNavigate();
  const snap = useOverviewSnapshot();

  const attention = useMemo(() => {
    const endpointOf = new Map(
      snap.endpoints.map((e) => [
        e.id,
        {
          deviceId: e.device_id ?? "",
          deviceName: snap.devices.find((d) => d.id === e.device_id)?.name ?? e.device_id ?? "",
        },
      ]),
    );
    return deriveAttentionItems({
      devices: snap.devices,
      endpoints: snap.endpoints.map((e) => ({ ...e, name: e.name ?? e.id })),
      points: snap.allPoints,
      activeEvents: snap.activeEvents,
      endpointOf,
    }).slice(0, 10);
  }, [snap]);

  // 诚实三态：loading（任一源未就绪）≠ 未知（就绪但失败）。fail-closed 的
  // ready 在失败后同样为 true（信息已完整），因此未知必须按 error 判定，
  // 不能按 !ready——否则失败会被判成“正常”。
  const loading = !snap.inventoryReady || !snap.eventsReady;
  const unknown = !!snap.inventoryError || !!snap.eventsError || snap.eventsUnavailable;
  const systemState = loading
    ? { text: "加载中", color: undefined as string | undefined }
    : unknown
      ? { text: "状态未知", color: undefined as string | undefined }
      : snap.failedCount > 0 || snap.pointsBad > 0
        ? { text: "需要关注", color: "#cf1322" }
        : { text: "正常", color: "#3f8600" };

  return (
    <div style={{ display: "grid", gap: 16 }}>
      {(snap.inventoryError || snap.pointsError || snap.eventsError) && (
        <Alert
          type="warning"
          showIcon
          message="部分数据更新失败"
          description={[
            snap.inventoryError ? `设备/连接清单：${snap.inventoryError}` : null,
            snap.pointsError ? "最新点位：显示为上次已知值" : null,
            snap.eventsError ? `活动事件：${snap.eventsError}` : null,
          ]
            .filter(Boolean)
            .join("；")}
        />
      )}
      {snap.eventsUnavailable && (
        <Alert type="error" showIcon message="Event service unavailable" description="EventStore 当前不可用；事件计数未知。" />
      )}

      <Card size="small" title="系统运行">
        <Space>
          <span style={{ width: 8, height: 8, borderRadius: "50%", background: systemState.color ?? "#8d8d8d", display: "inline-block" }} />
          <span>{systemState.text}</span>
        </Space>
      </Card>

      <Row gutter={[16, 16]}>
        <Col xs={12} lg={6}>
          <Card size="small">
            <Statistic title="设备" value={fmtCount(unknown, snap.deviceCount)} />
          </Card>
        </Col>
        <Col xs={12} lg={6}>
          <Card size="small">
            <Statistic
              title="连接（运行中/总数）"
              value={unknown ? "未知" : `${snap.runningCount} / ${snap.endpointTotal}`}
            />
          </Card>
        </Col>
        <Col xs={12} lg={6}>
          <Card size="small">
            <Statistic
              title="数据（GOOD/BAD/STALE）"
              value={snap.pointsError && snap.pointsTotal === 0 ? "STALE" : `${snap.pointsGood}/${snap.pointsBad}/${snap.pointsStale}`}
            />
          </Card>
        </Col>
        <Col xs={12} lg={6}>
          <Card size="small">
            <Statistic
              title="活动事件"
              value={snap.activeEventCount === null ? "未知" : snap.activeEventCount}
            />
          </Card>
        </Col>
      </Row>

      <Card
        size="small"
        title={`需要关注 · ${attention.length}`}
        extra={<Button type="link" size="small" onClick={() => nav("/devices")}>全部设备 →</Button>}
      >
        {loading ? (
          <div style={{ fontSize: 12, color: "#525252" }}>加载中…（清单/事件未就绪前不判空）</div>
        ) : !attention.length ? (
          <div style={{ fontSize: 12, color: "#525252" }}>暂无需要关注的问题</div>
        ) : (
          <div style={{ display: "grid", gap: 8 }}>
            {attention.map((a, i) => (
              <div
                key={`${a.kind}-${a.deviceId}-${a.endpointId ?? ""}-${a.title}-${i}`}
                style={{ display: "flex", gap: 8, alignItems: "baseline", fontSize: 13 }}
              >
                <span style={{ fontWeight: 600 }}>{a.deviceName}</span>
                <span>{a.title}</span>
                <span style={{ color: "#525252", fontSize: 12 }}>{a.detail}</span>
                <Button size="small" type="link" onClick={() => nav(a.target.href)}>
                  查看 →
                </Button>
              </div>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
