// M5.5 AddDeviceFlow 提交（原子接口版）。
// - 前三步仍只构建 draft，不落库；最后一步一次 POST /device-bootstrap；
// - 后端单事务落库 + start side effect + 补偿删除 + 幂等（Idempotency-Key
//   前端生成：同一 draft 重复提交视为重放，不建两台设备）；
// - M4.2 的四步前端编排已退役（后端原子化后不再需要客户端补偿），
//   draft 模型与报告语义保留，调用方（AddDeviceFlow）不动。
import type { NckPointSelection, ResourceSelection } from "../deviceModel";
import { assembleBinding, isNckDriver } from "../deviceModel";
import type { NckVariableDefinitionLike } from "../nckCollectability";
import { planEndpointTaskSave } from "../endpointSavePlan";

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
  /**
   * 非 NCK 的采集点选择；**允许为空**（C3-D：零采集点 = 仅保存设备与连接）。
   *
   * NCK 点位**不得**放进这里：`selections` 与 `nckPoints` 是两种不可互换的
   * Binding 结构，混用会被 `assembleBinding`/Driver `configure` 整体拒绝。
   */
  selections: ResourceSelection[];
  /**
   * NCK 语义/原生点位（PR-D）。**仅存在于向导本地草稿，不持久化**：
   * 提交时统一交给 `planEndpointTaskSave` 做 scope 准入、唯一性与任务装配，
   * 本字段不是第二条保存路径。
   */
  nckPoints?: NckPointSelection[];
  /**
   * NCK 定义目录（按 `variable_id` 索引），由数据步的完整目录索引得到。
   *
   * 必须传**全量**目录而不是只传 collect 条目：保存计划要能用 `identity`
   * 条目给出 `SCOPE_NOT_COLLECTABLE` 的准确原因；缺失则 fail-closed。
   */
  nckDefinitions?: Readonly<Record<string, NckVariableDefinitionLike>> | null;
  /**
   * 创建后启动。
   *
   * C3-D 起默认 **false**（离线保存优先）：只有用户在数据步**明确勾选**才发送
   * `start: true`。理由是「设备还没开机/采集点还没定」是常态，默认启动会把
   * 用户推到一个必然失败或空转的运行态。
   */
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

/**
 * draft 完整性：确认页「添加设备」按钮的启用条件。
 *
 * C3-D：**采集点可以为空**（离线保存：先建好设备与连接，采集点稍后补），因此不再
 * 要求 `selections.length > 0`。但设备与连接仍是硬前置——连接参数缺失或未通过
 * 描述符/Secret 校验时，本函数依旧返回 false，跳过采集点**不会**绕过这些门禁
 * （ConnectionSetupPanel 的前置门与 `commitConnection` 的守卫都在更前面）。
 */
export function isDraftComplete(d: AddDeviceDraft): boolean {
  if (!d.device || !d.device.deviceId.trim()) return false;
  if (!d.connection || !d.connection.driverId || !d.connection.endpointId.trim()) return false;
  if (!d.acquisition) return false;
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

/**
 * 幂等键语义（RC2 修3）：key 表示“这一次创建操作”，不是“这份配置内容”。
 * - 同一 Flow 内重复提交（超时重试/双击）→ 复用同一 key → 后端重放；
 * - 新 Flow（删除后重建/重新走一遍）→ 必须新 key → 后端真正执行。
 * key 由调用方（AddDeviceFlow）持有：确认页进入生成，重试复用，reset 更新。
 * 此处只提供生成器，绝不从 draft 内容派生（content-addressed 已退役：
 * 相同配置删除后重建会命中几个月前的 replay，进入不存在的设备）。
 */
export function newOperationKey(): string {
  const c = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (c?.randomUUID) return `web-op-${c.randomUUID()}`;
  return `web-op-${Date.now().toString(36)}-${Math.floor(Math.random() * 0xffffff).toString(16)}`;
}

/**
 * 原子提交：一次 POST /device-bootstrap。connection/acquisition 的请求体
 * 构造沿用 deviceModel（cleanConnection 由调用方完成，此处只组装）。
 *
 * C3-D 零采集点：`selections` 为空时发送 `acquisition.tasks = []`，
 * **绝不**生成「selections 为空」的假任务——那是一条真实任务，后端会把它入库，
 * 而空 selections 在驱动侧 `configure` 里必然被拒（`test-driver` 即
 * `INVALID_BINDING_CONFIG: selections 不能为空`），等于把创建失败推迟到启动时。
 * 后端对 `tasks: []` 的支持见 `crates/core-api/src/bootstrap.rs`（`if !acq.tasks.is_empty()`）。
 */
export async function bootstrapDevice(
  client: BootstrapClient,
  draft: { device: DeviceDraft; connection: ConnectionDraft; acquisition: AcquisitionDraft },
  opts?: { idempotencyKey?: string },
): Promise<BootstrapReport> {
  const { device, connection, acquisition } = draft;
  // 任务集只在这一处装配；`tasks=[]` 表示「本次不建任务」（零采集点）。
  let tasks: unknown[] = [];
  const nck = isNckDriver(connection.driverId);
  if (nck) {
    // NCK 的旧 `selections` 结构无法表达 `{points}`：保留原有 fail-closed
    // （A19/A29），确保绝不在 NCK 上写出 `mesa.resources.v1`。
    if (acquisition.selections.length > 0) {
      const assembled = assembleBinding({
        driverId: connection.driverId,
        content: { type: "resources", selections: acquisition.selections },
      });
      return {
        ok: false,
        deviceId: device.deviceId,
        endpointId: connection.endpointId,
        failedStep: "put-tasks",
        failedMessage: assembled.ok
          ? "NCK Endpoint 不允许写入 mesa.resources.v1 任务（必须使用 nck.points.v1）"
          : assembled.message,
        compensated: false,
        residual: false,
        replayed: false,
      };
    }
    const nckPoints = acquisition.nckPoints ?? [];
    if (nckPoints.length > 0) {
      // -------------------------------------------------------------------
      // P0 快照限定：`tasks: [] + snapshotState: "ready"` **只**允许出现在
      // Bootstrap 原子创建新 Endpoint 的分支——Endpoint 尚不存在，其服务端
      // 任务集可证为空，因此不存在「用空快照覆盖已有任务」的风险。
      // 已有 Endpoint 的编辑路径必须使用真实任务快照（见 EndpointAcquisitionPane
      // 的 tasksLoadState / tasksLoadedEpId），**不得**复制本段写法。
      // -------------------------------------------------------------------
      const plan = planEndpointTaskSave({
        driverId: connection.driverId,
        tasks: [],
        snapshotState: "ready",
        loadedEndpointId: connection.endpointId,
        currentEndpointId: connection.endpointId,
        intervalMs: acquisition.intervalMs,
        nckPoints,
        nckDefinitions: acquisition.nckDefinitions ?? null,
      });
      if (!plan.ok) {
        // 保存计划是唯一准入点（scope/结构/唯一性）；失败即零 POST。
        return {
          ok: false,
          deviceId: device.deviceId,
          endpointId: connection.endpointId,
          failedStep: "put-tasks",
          failedMessage: `${plan.code}：${plan.message}`,
          compensated: false,
          residual: false,
          replayed: false,
        };
      }
      // NCK 任务只来自保存计划：此处不重算身份、不改写任务、不出现 kind 字面量。
      tasks = plan.tasks;
    }
    // 零点位：保持 `tasks=[]`（D9/A29）。**不**调用保存计划——空点位列会被
    // 它以 EMPTY_POINTS 拒绝，而「零采集点」本身是合法的仅保存路径。
  } else if (acquisition.selections.length > 0) {
    const assembled = assembleBinding({
      driverId: connection.driverId,
      content: { type: "resources", selections: acquisition.selections },
    });
    if (!assembled.ok) {
      return {
        ok: false,
        deviceId: device.deviceId,
        endpointId: connection.endpointId,
        failedStep: "put-tasks",
        failedMessage: assembled.message,
        compensated: false,
        residual: false,
        replayed: false,
      };
    }
    tasks = [
      {
        id: "t1",
        schedule: { mode: "poll", interval_ms: acquisition.intervalMs },
        binding: assembled.binding,
      },
    ];
  }
  const res = await client.deviceBootstrap({
    device: { id: device.deviceId, name: device.deviceName || device.deviceId },
    endpoint: {
      id: connection.endpointId,
      name: connection.endpointName || connection.endpointId,
      driver_id: connection.driverId,
      connection: connection.connection,
    },
    acquisition: { tasks },
    // 零采集点时**必然**不发启动：没有采集点的运行态是空转，且会与「仅保存」的
    // 用户意图矛盾。这里再兜一道（UI 已强制 false），确保任何调用方都发不出
    // 「tasks=[] + start=true」这种自相矛盾的组合。
    start: tasks.length > 0 && acquisition.startAfterCreate,
    idempotency_key: opts?.idempotencyKey ?? newOperationKey(),
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
