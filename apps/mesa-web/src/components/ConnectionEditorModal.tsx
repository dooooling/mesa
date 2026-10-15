// 连接编辑 Modal（C2：参数区改为共享 ConnectionSetupPanel）。
//
// 保留的既有安全能力（逐条不得回归）：
// 1. Endpoint 快照门（P0 fail-closed）：GET /endpoints/{id} 未成功返回前
//    保存一律禁用；切 Endpoint 先清旧值 + cancelled 守卫，A 的迟到响应
//    绝不能污染 B（否则 updateEndpoint(B, A-connection)）；
// 2. Secret marker 语义：服务端不回显明文（`{secret_set:true}`），marker
//    原样保留在草稿里并随 PUT 回传（后端据此保留历史凭据）；用户显式清除
//    走 `{clear_secret:true}`。面板只把 marker 排除在 probe/validate 之外，
//    **不**改变保存语义；
// 3. 生命周期顺序不变：STOPPED 直接 Apply；RUNNING 走 Stop → Apply →
//    （可选）Restart，Stop 失败绝不 Apply，Apply 失败保持 STOPPED，
//    Restart 失败明确提示。
// Prop 变化（切连接）即 remount 语义由父页 key 保证。
import { useEffect, useState } from "react";
import { Alert, Input, Modal, Tag, message } from "antd";
import { api } from "../api";
import {
  applyEndpointChange,
  buildEndpointUpdatePayload,
  cleanConnection,
  isRunningState,
  type LifecycleStepResult,
} from "../deviceModel";
import type { ConnectionValues } from "../connection/connectionSetupModel";
import { ConnectionSetupPanel } from "../connection/ConnectionSetupPanel";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function ConnectionEditorModal(props: {
  endpointId: string;
  deviceId: string;
  driverId: string;
  initialName: string;
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { endpointId, deviceId, driverId, initialName, open, onClose, onChanged } = props;
  const [conn, setConn] = useState<ConnectionValues | null>(null);
  const [name, setName] = useState(initialName);
  const [running, setRunning] = useState(false);
  const [saving, setSaving] = useState(false);
  // 连接快照门（P0 fail-closed）：只有快照归属当前 Endpoint 且加载完成才允许保存；
  // 切 Endpoint 先清旧值 + 代际守卫，A 的迟到响应绝不能污染 B。
  const [connLoadedEpId, setConnLoadedEpId] = useState<string | null>(null);
  const [connLoading, setConnLoading] = useState(false);
  const [connError, setConnError] = useState("");
  // 驱动描述符就绪门（P0）：描述符失败时字段集合不可信（visible_if / 类型 / 范围
  // 都无从判断），此时 PUT 出去的 connection 可能是残缺的，必须挡住。
  // 由共享面板的 onDescriptorChange 上报（成功给 descriptor、失败给 null）。
  const [descriptorReady, setDescriptorReady] = useState(false);

  useEffect(() => {
    if (!open) return;
    // 开窗即复位：旧 connection 不得残留成可保存内容。
    setConn(null);
    setName(initialName);
    setConnLoadedEpId(null);
    setConnError("");
    setDescriptorReady(false);
    setConnLoading(true);
    let cancelled = false;
    fetch(`/api/v1/endpoints/${endpointId}`)
      .then(async (x) => {
        if (!x.ok) throw new Error(`GET /endpoints/${endpointId} ${x.status}`);
        return x.json();
      })
      .then((ep) => {
        if (cancelled) return;
        // 服务端可能脱敏 Secret 字段；未动字段原样回传由后端合并语义决定
        setConn(((ep as { connection?: unknown }).connection as ConnectionValues) ?? {});
        setName(
          ((ep as { name?: string }).name ?? (ep as { id?: string }).id ?? endpointId) as string,
        );
        setConnLoadedEpId(endpointId);
        setConnLoading(false);
      })
      .catch(() => {
        if (cancelled) return;
        setConn(null);
        setConnLoading(false);
        setConnError("连接快照加载失败：服务端当前连接未知，已禁用保存（请关闭重试，不会用旧连接覆盖）。");
      });
    api
      .listEndpoints()
      .then((j) => {
        const live = (
          (j as { endpoints?: Array<{ id: string; runtime?: { state?: string }; state?: string }> })
            .endpoints ?? []
        ).find((e) => e.id === endpointId);
        if (!cancelled) setRunning(isRunningState(live?.state ?? live?.runtime?.state));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [open, endpointId, initialName]);

  const connReady = connLoadedEpId === endpointId && !connLoading && conn !== null;

  const save = async (restart: boolean, connection: ConnectionValues) => {
    // 快照门：未 ready 前保存禁用；串 Endpoint / 加载失败一律不可写。
    if (!connReady) {
      if (connError) return message.error("连接快照加载失败，禁止保存以防用旧连接覆盖。请关闭重试。");
      return message.warning("连接快照加载中，禁止保存以防串改另一连接。请稍候。");
    }
    // ---------------------------------------------------------------------
    // P0 提交守卫：驱动描述符未就绪（加载失败 / 尚未返回）时**本函数自身**也拒绝
    // 发写请求。面板在描述符不可用时不会回调到这里，这里再判一次是为了防止
    // 按钮 disabled 被回归移除、或将来新增程序化保存入口时直接发出
    // PUT /endpoints/{id}（Stop→Apply→Restart 全链都不得启动）。
    // ---------------------------------------------------------------------
    if (!descriptorReady) {
      return message.error(
        "驱动描述符未就绪，已阻止保存（字段集合不可信，不会用残缺连接覆盖服务端）。请重试加载描述符。",
      );
    }
    const cleaned = cleanConnection(connection);
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
    <Modal
      title={`编辑连接 · ${initialName}`}
      open={open}
      onCancel={onClose}
      footer={null}
      destroyOnHidden
      width={720}
    >
      <div style={{ display: "grid", gap: 12 }}>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <span style={{ fontSize: 12 }}>ID</span>
          <span style={{ fontFamily: "'IBM Plex Mono','JetBrains Mono',ui-monospace,monospace", fontSize: 12 }}>{endpointId}</span>
          <Tag>{driverId}</Tag>
        </div>
        <div>
          <div style={{ fontSize: 12, marginBottom: 4 }}>连接名称</div>
          <Input value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        {connError ? (
          <Alert
            type="error"
            message={connError}
            description="服务端当前连接未知，为防止串改已禁用保存。请关闭重试。"
          />
        ) : null}
        {connLoading && !connError ? (
          <div style={{ fontSize: 12, color: "#525252" }}>
            正在加载连接快照…（快照就绪前保存保持禁用，防止串改另一连接）
          </div>
        ) : null}
        <ConnectionSetupPanel
          mode="edit"
          active={open}
          sessionKey={`edit-connection:${endpointId}`}
          initialDriverId={driverId}
          initialValues={conn}
          running={running}
          applying={saving}
          submitLabel="保存"
          // 快照未就绪 / 加载失败：保存与校验/测试一律禁用（fail-closed）
          actionsDisabled={!connReady}
          onDescriptorChange={(desc) => setDescriptorReady(desc !== null)}
          onSubmit={async ({ connection, restart }) => {
            await save(restart, connection);
          }}
        />
        <div style={{ fontSize: 12, color: "#525252" }}>需先停止再修改（停止是应用动作的一部分）</div>
      </div>
    </Modal>
  );
}
