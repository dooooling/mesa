// M4.2 AddDeviceFlow draft + 编排（纯逻辑层）。
// - 前三步只构建 draft，不落库；最后一步才真正提交；
// - 前端编排：createDevice → createEndpoint → putTasks → startEndpoint，
//   任一步失败反向补偿（deleteEndpoint → deleteDevice）；
// - 补偿也失败时明确报告残留，不假装已恢复（M5 原子化后此编排退役，
//   但 draft 模型与补偿语义保留为后端事务的对照）。
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

export interface BootstrapReport {
  ok: boolean;
  deviceId: string;
  endpointId: string;
  /** 走到哪一步失败（ok 时为 null）。 */
  failedStep: BootstrapStep | null;
  failedMessage?: string;
  /** 补偿动作记录（成功/失败逐条）。 */
  compensated: Array<{ action: "delete-endpoint" | "delete-device"; ok: boolean; message?: string }>;
  /** 补偿未完全成功时为 true：必须明确提示残留，不假装恢复。 */
  residual: boolean;
}

export interface BootstrapClient {
  createDevice: (body: { id: string; name: string }) => Promise<{ status: number; body?: { error?: { message?: string } } }>;
  createEndpoint: (body: {
    id: string;
    name: string;
    device_id: string;
    driver_id: string;
    connection: Record<string, unknown>;
  }) => Promise<{ status: number; body?: { error?: { message?: string } } }>;
  putTasks: (endpointId: string, tasks: unknown[]) => Promise<{ status: number; body?: { error?: { message?: string } } }>;
  startEndpoint: (endpointId: string) => Promise<{ status: number; body?: { error?: { message?: string } } }>;
  deleteEndpoint: (endpointId: string) => Promise<{ status: number; body?: { error?: { message?: string } } }>;
  deleteDevice: (deviceId: string) => Promise<{ status: number; body?: { error?: { message?: string } } }>;
}

function okStatus(s: number): boolean {
  return s === 200 || s === 201 || s === 204;
}

function msg(body: { error?: { message?: string } } | undefined, fallback: string): string {
  return body?.error?.message ?? fallback;
}

/**
 * 前端编排 bootstrap。connection/acquisition 的请求体构造沿用 deviceModel
 *（cleanConnection/mergeAcquisitionTasks），此处只做步骤编排与补偿。
 */
export async function bootstrapDevice(
  client: BootstrapClient,
  draft: { device: DeviceDraft; connection: ConnectionDraft; acquisition: AcquisitionDraft },
): Promise<BootstrapReport> {
  const { device, connection, acquisition } = draft;
  const compensated: BootstrapReport["compensated"] = [];

  // 1. createDevice
  const r1 = await client.createDevice({ id: device.deviceId, name: device.deviceName || device.deviceId });
  if (!okStatus(r1.status)) {
    return {
      ok: false, deviceId: device.deviceId, endpointId: connection.endpointId,
      failedStep: "create-device", failedMessage: msg(r1.body, "创建设备失败"),
      compensated, residual: false,
    };
  }

  // 2. createEndpoint（失败 → 补偿删除 device）
  const r2 = await client.createEndpoint({
    id: connection.endpointId,
    name: connection.endpointName || connection.endpointId,
    device_id: device.deviceId,
    driver_id: connection.driverId,
    connection: connection.connection,
  });
  if (!okStatus(r2.status)) {
    const d = await client.deleteDevice(device.deviceId);
    compensated.push({
      action: "delete-device",
      ok: okStatus(d.status),
      message: okStatus(d.status) ? undefined : msg(d.body, "回滚删除设备失败"),
    });
    return {
      ok: false, deviceId: device.deviceId, endpointId: connection.endpointId,
      failedStep: "create-endpoint", failedMessage: msg(r2.body, "创建连接失败"),
      compensated, residual: compensated.some((c) => !c.ok),
    };
  }

  // 3. putTasks（canonical 单任务；失败 → 补偿删除 endpoint → device）
  const tasks = [
    {
      id: "t1",
      mode: "poll",
      interval_ms: acquisition.intervalMs,
      binding: { kind: "mesa.resources.v1", config: { selections: acquisition.selections } },
    },
  ];
  const r3 = await client.putTasks(connection.endpointId, tasks);
  if (!okStatus(r3.status)) {
    const e = await client.deleteEndpoint(connection.endpointId);
    compensated.push({ action: "delete-endpoint", ok: okStatus(e.status), message: okStatus(e.status) ? undefined : msg(e.body, "回滚删除连接失败") });
    if (okStatus(e.status)) {
      const d = await client.deleteDevice(device.deviceId);
      compensated.push({ action: "delete-device", ok: okStatus(d.status), message: okStatus(d.status) ? undefined : msg(d.body, "回滚删除设备失败") });
    }
    return {
      ok: false, deviceId: device.deviceId, endpointId: connection.endpointId,
      failedStep: "put-tasks", failedMessage: msg(r3.body, "配置采集失败"),
      compensated, residual: compensated.some((c) => !c.ok),
    };
  }

  // 4. startEndpoint（可选；失败 → 同 put-tasks 补偿；残留必须明示）
  if (acquisition.startAfterCreate) {
    const r4 = await client.startEndpoint(connection.endpointId);
    if (!okStatus(r4.status)) {
      const e = await client.deleteEndpoint(connection.endpointId);
      compensated.push({ action: "delete-endpoint", ok: okStatus(e.status), message: okStatus(e.status) ? undefined : msg(e.body, "回滚删除连接失败") });
      if (okStatus(e.status)) {
        const d = await client.deleteDevice(device.deviceId);
        compensated.push({ action: "delete-device", ok: okStatus(d.status), message: okStatus(d.status) ? undefined : msg(d.body, "回滚删除设备失败") });
      }
      return {
        ok: false, deviceId: device.deviceId, endpointId: connection.endpointId,
        failedStep: "start-endpoint", failedMessage: msg(r4.body, "启动连接失败"),
        compensated, residual: compensated.some((c) => !c.ok),
      };
    }
  }

  return { ok: true, deviceId: device.deviceId, endpointId: connection.endpointId, failedStep: null, compensated, residual: false };
}
