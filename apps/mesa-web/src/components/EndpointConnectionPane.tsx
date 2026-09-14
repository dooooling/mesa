// Endpoint 连接编辑窗格（Workspace「连接」tab；逻辑由 DeviceDetail 编辑
// Modal 原样迁移）。driver_id 创建后不可改（请求体无该字段）；保存先停止，
// 成功后需手动启动（生命周期统一见后续项，本窗格保持原语义）。
import { useEffect, useState } from "react";
import { Alert, Input, Space, Tag, message } from "antd";
import { api } from "../api";
import type { DriverDescriptor } from "../types";
import {
  applyEndpointChange,
  buildEndpointUpdatePayload,
  cleanConnection,
  isRunningState,
  type LifecycleStepResult,
} from "../deviceModel";
import { ApplyWithRestart } from "./ApplyWithRestart";
import { DescriptorFields } from "./DescriptorFields";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function EndpointConnectionPane({
  endpointId,
  deviceId,
  driverId,
  initialName,
  onChanged,
}: {
  endpointId: string;
  deviceId: string;
  driverId: string;
  initialName: string;
  onChanged: () => void;
}) {
  const [desc, setDesc] = useState<DriverDescriptor | null>(null);
  const [conn, setConn] = useState<Record<string, unknown>>({});
  const [name, setName] = useState(initialName);
  const [running, setRunning] = useState(false);
  const [saving, setSaving] = useState(false);
  // 连接快照门（P0 fail-closed）：只有 conn + descriptor 双双归属当前
  // Endpoint 且加载完成才允许保存；切 Endpoint 先清旧值 + 代际守卫，
  // A 的迟到响应绝不能污染 B（否则 updateEndpoint(B, A-connection)）。
  const [connLoadedEpId, setConnLoadedEpId] = useState<string | null>(null);
  const [connLoading, setConnLoading] = useState(false);
  const [connError, setConnError] = useState("");

  useEffect(() => {
    // 切 Endpoint 先复位：旧 connection 不得残留成新窗格的可保存内容。
    setConn({});
    setName(initialName);
    setDesc(null);
    setConnLoadedEpId(null);
    setConnError("");
    setConnLoading(true);
    let cancelled = false;
    Promise.all([
      fetch(`/api/v1/endpoints/${endpointId}`)
        .then(async (x) => {
          if (!x.ok) throw new Error(`GET /endpoints/${endpointId} ${x.status}`);
          return x.json();
        })
        .catch((e) => ({ error: e as unknown })),
      fetch(`/api/v1/drivers/${driverId}/descriptor`)
        .then((x) => x.json())
        .catch(() => null),
    ]).then(([epRes, d]) => {
      if (cancelled) return;
      // 只接受带 connection schema 的 Descriptor：错误包不得 masquerade 成描述。
      if (d && (d as { connection?: unknown }).connection) setDesc(d);
      if (epRes && !(epRes as { error?: unknown }).error) {
        // 服务端可能脱敏 Secret 字段；未动字段原样回传由后端合并语义决定
        setConn(((epRes as { connection?: unknown }).connection as Record<string, unknown>) ?? {});
        setName(
          ((epRes as { name?: string }).name
            ?? (epRes as { id?: string }).id
            ?? endpointId) as string,
        );
        setConnLoadedEpId(endpointId);
        setConnLoading(false);
      } else {
        setConnLoading(false);
        setConnError("连接快照加载失败：服务端当前连接未知，已禁用保存（请切换后重试，不会用旧连接覆盖）。");
      }
    });
    api.listEndpoints()
      .then((j) => {
        const live = ((j as { endpoints?: Array<{ id: string; runtime?: { state?: string }; state?: string }> }).endpoints ?? []).find(
          (e) => e.id === endpointId,
        );
        if (!cancelled) setRunning(isRunningState(live?.state ?? live?.runtime?.state));
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [endpointId, driverId, initialName]);

  const connReady = connLoadedEpId === endpointId && !connLoading && !!desc;

  const save = async (restart: boolean) => {
    // 快照门：B 未 ready 前保存禁用；串 Endpoint / 加载失败一律不可写。
    if (!connReady) {
      if (connError) return message.error("连接快照加载失败，禁止保存以防用旧连接覆盖。请切换后重试。");
      return message.warning("连接快照加载中，禁止保存以防串改另一连接。请稍候。");
    }
    const cleaned = cleanConnection(conn);
    if (!Object.keys(cleaned).length) return message.warning("请填写连接参数");
    setSaving(true);
    try {
      // postJson 对 409/500 不 throw：每步显式检查 status，统一走 executor。
      const outcome = await applyEndpointChange({
        wasRunning: running,
        restart,
        stop: async (): Promise<LifecycleStepResult> => {
          const r = await api.stopEndpoint(endpointId).catch((e) => ({
            status: -1,
            body: { error: { message: e instanceof Error ? e.message : String(e) } },
          }));
          await sleep(300);
          return r.status === 200
            ? { ok: true }
            : { ok: false, message: (r.body as { error?: { message?: string } })?.error?.message ?? `停止失败（${r.status}），已中止应用` };
        },
        apply: async (): Promise<LifecycleStepResult> => {
          // driver_id 不可变：请求体无该字段（误带即后端 deny_unknown_fields 拒绝）
          const body = buildEndpointUpdatePayload({ name: name.trim() || endpointId, deviceId, connection: cleaned });
          try {
            await api.updateEndpoint(endpointId, body);
            return { ok: true };
          } catch (e) {
            return { ok: false, message: e instanceof Error ? e.message : String(e) };
          }
        },
        start: async (): Promise<LifecycleStepResult> => {
          const r = await api.startEndpoint(endpointId).catch((e) => ({
            status: -1,
            body: { error: { message: e instanceof Error ? e.message : String(e) } },
          }));
          return r.status === 200
            ? { ok: true }
            : { ok: false, message: (r.body as { error?: { message?: string } })?.error?.message ?? `恢复运行失败（${r.status}）` };
        },
      });
      if (outcome.kind === "applied-restarted") {
        message.success("修改成功，已恢复运行");
        onChanged();
      } else if (outcome.kind === "applied-stopped") {
        message.success(running && !restart ? "配置已保存，Endpoint 保持停止" : "修改成功（驱动未变，需手动启动）");
        onChanged();
      } else if (outcome.kind === "stop-failed") {
        message.error(`停止失败，已中止应用，未修改连接：${outcome.message}`);
      } else if (outcome.kind === "apply-failed-stopped") {
        message.error(`应用失败，Endpoint 当前已停止：${outcome.message}`);
        onChanged();
      } else {
        message.warning(`配置已保存，但恢复运行失败：${outcome.message}`);
        onChanged();
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <div style={{ display: "grid", gap: 12, maxWidth: 720 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <span style={{ fontSize: 12 }}>驱动</span>
        <Tag>{driverId}</Tag>
        <span style={{ fontSize: 12, color: "#525252" }}>创建后不可改</span>
      </div>
      <div>
        <div style={{ fontSize: 12, marginBottom: 4 }}>连接名称</div>
        <Input value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      {!desc && connLoading ? (
        <div style={{ color: "#525252" }}>加载中…</div>
      ) : null}
      {connError ? (
        <Alert type="error" message={connError} description="服务端当前连接未知，为防止串改已禁用保存。请切换后重试。" />
      ) : null}
      {connLoading && !connError ? (
        <div style={{ fontSize: 12, color: "#525252" }}>正在加载连接快照…（快照就绪前保存保持禁用，防止串改另一连接）</div>
      ) : null}
      {!desc && !connLoading ? (
        <Alert type="error" message="资源描述加载失败" description="描述缺失时无合法编辑依据，已禁用保存。请切换后重试。" />
      ) : null}
      {desc ? (
        <DescriptorFields schema={desc.connection} value={conn} onChange={setConn} />
      ) : null}
      <div style={{ fontSize: 12, color: "#525252" }}>需先停止再修改（停止是应用动作的一部分）</div>
      <Space>
        <ApplyWithRestart
          running={running}
          applying={saving}
          canApply={Object.keys(cleanConnection(conn)).length > 0 && connReady}
          applyLabel="保存"
          onApply={save}
        />
      </Space>
    </div>
  );
}
