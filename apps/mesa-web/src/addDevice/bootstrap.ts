// M5.5 AddDeviceFlow 提交（原子接口版）。
// - 前三步仍只构建 draft，不落库；最后一步一次 POST /device-bootstrap；
// - 后端单事务落库 + start side effect + 补偿删除 + 幂等（Idempotency-Key
//   前端生成：同一 draft 重复提交视为重放，不建两台设备）；
// - M4.2 的四步前端编排已退役（后端原子化后不再需要客户端补偿），
//   draft 模型与报告语义保留，调用方（AddDeviceFlow）不动。
import type { ResourceSelection } from "../deviceModel";

export interface DeviceDraft {
  deviceId: string;
  deviceName: string;
}

export interface ConnectionDraft {
  driverId: string;
  endpointId: string;
  endpointName: string;
  connection: Record<string, unknown>;
}

export interface AcquisitionDraft {
  intervalMs: number;
  selections: ResourceSelection[];
  /** 创建后启动（默认 true；关闭则建完保持 STOPPED）。 */
  startAfterCreate: boolean;
}

export interface AddDeviceDraft {
  device: DeviceDraft | null;
  connection: ConnectionDraft | null;
  acquisition: AcquisitionDraft | null;
}

export const EMPTY_ADD_DEVICE_DRAFT: AddDeviceDraft = {
  device: null,
  connection: null,
  acquisition: null,
};

/** draft 完整性：确认页“添加设备”按钮的启用条件。 */
export function isDraftComplete(d: AddDeviceDraft): boolean {
  if (!d.device || !d.device.deviceId.trim()) return false;
  if (!d.connection || !d.connection.driverId || !d.connection.endpointId.trim()) return false;
  if (!d.acquisition || d.acquisition.selections.length === 0) return false;
  return true;
}

export type BootstrapStep = "create-device" | "create-endpoint" | "put-tasks" | "start-endpoint";

/**
 * 提交报告（M5.5：后端原子接口的结果映射）。
 * - ok：设备已建好（started 反映是否已启动）；
 * - failedStep：后端精确失败点（TRANSACTION 前失败映射为对应步骤，
 *   start side effect 失败为 start-endpoint + compensated=true）；
 * - residual：后端补偿删除失败（极罕见，需人工到设备列表检查）。
 */
export interface BootstrapReport {
  ok: boolean;
  deviceId: string;
  endpointId: string;
  /** 走到哪一步失败（ok 时为 null）。 */
  failedStep: BootstrapStep | "unknown" | null;
  failedMessage?: string;
  /** 后端已补偿删除（start 失败路径）。 */
  compensated: boolean;
  /** 补偿未完全成功时为 true：必须明确提示残留，不假装恢复。 */
  residual: boolean;
  /** 幂等重放（同 draft 重复提交，后端直接返回上次结果）。 */
  replayed: boolean;
}

export interface BootstrapRequest {
  device: { id: string; name: string };
  endpoint: { id: string; name: string; driver_id: string; connection: Record<string, unknown> };
  acquisition: { tasks: unknown[] };
  start: boolean;
  idempotency_key: string;
}

export interface BootstrapClient {
  deviceBootstrap: (
    body: BootstrapRequest,
  ) => Promise<{ status: number; body: Record<string, unknown> }>;
}

/** 幂等键：draft 内容哈希（同 draft 重复提交 → 同 key → 后端重放）。 */
export function draftIdempotencyKey(draft: {
  device: DeviceDraft;
  connection: ConnectionDraft;
  acquisition: AcquisitionDraft;
}): string {
  const s = JSON.stringify({
    d: [draft.device.deviceId, draft.device.deviceName],
    c: [draft.connection.driverId, draft.connection.endpointId, draft.connection.endpointName, draft.connection.connection],
    a: [draft.acquisition.intervalMs, draft.acquisition.selections, draft.acquisition.startAfterCreate],
  });
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (let i = 0; i < s.length; i++) {
    h1 = Math.imul(h1 ^ s.charCodeAt(i), 0x01000193) >>> 0;
    h2 = Math.imul(h2 + s.charCodeAt(i), 0x85ebca6b) >>> 0;
  }
  return `web-${h1.toString(16)}${h2.toString(16)}`;
}

/**
 * 原子提交：一次 POST /device-bootstrap。connection/acquisition 的请求体
 * 构造沿用 deviceModel（cleanConnection 由调用方完成，此处只组装）。
 */
export async function bootstrapDevice(
  client: BootstrapClient,
  draft: { device: DeviceDraft; connection: ConnectionDraft; acquisition: AcquisitionDraft },
): Promise<BootstrapReport> {
  const { device, connection, acquisition } = draft;
  const res = await client.deviceBootstrap({
    device: { id: device.deviceId, name: device.deviceName || device.deviceId },
    endpoint: {
      id: connection.endpointId,
      name: connection.endpointName || connection.endpointId,
      driver_id: connection.driverId,
      connection: connection.connection,
    },
    acquisition: {
      tasks: [
        {
          id: "t1",
          mode: "poll",
          interval_ms: acquisition.intervalMs,
          binding: { kind: "mesa.resources.v1", config: { selections: acquisition.selections } },
        },
      ],
    },
    start: acquisition.startAfterCreate,
    idempotency_key: draftIdempotencyKey(draft),
  });
  const b = res.body ?? {};
  const err = (b as { error?: { code?: string; message?: string } }).error;

  if (res.status === 200 || res.status === 201) {
    return {
      ok: true,
      deviceId: String((b as { device_id?: unknown }).device_id ?? device.deviceId),
      endpointId: String((b as { endpoint_id?: unknown }).endpoint_id ?? connection.endpointId),
      failedStep: null,
      compensated: false,
      residual: false,
      replayed: (b as { replayed?: boolean }).replayed === true,
    };
  }

  // 失败映射：后端 code → 前端步骤（调用方展示用，不做重试决策）。
  const code = err?.code ?? "";
  const message = err?.message ?? `创建失败（${res.status})`;
  let failedStep: BootstrapReport["failedStep"] = "unknown";
  if (code === "START_FAILED") failedStep = "start-endpoint";
  else if (/device/i.test(code) || /device/i.test(message)) failedStep = "create-device";
  else if (/endpoint/i.test(code) || /connection/i.test(message)) failedStep = "create-endpoint";
  else if (/task|acquisition|binding|valid/i.test(code)) failedStep = "put-tasks";
  const compensated = (b as { compensated?: boolean }).compensated === true;
  // start 失败后端已补偿；其它失败是事务回滚（无残留）。
  // residual：仅当后端明确说补偿失败——当前后端 code 未区分，保守按
  // compensated=false 的 start 失败视为需人工检查。
  const residual = failedStep === "start-endpoint" && !compensated;
  return {
    ok: false,
    deviceId: device.deviceId,
    endpointId: connection.endpointId,
    failedStep,
    failedMessage: message,
    compensated,
    residual,
    replayed: false,
  };
}
