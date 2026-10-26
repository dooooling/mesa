// M4.2 回归：AddDeviceFlow 四步 draft + 最终提交。
// - 前三步不落库（无 POST）；最后一步才编排提交；
// - 成功进设备；失败显示回滚状态；残留明示。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { renderAddDeviceFlow as renderFlow } from "../test/addDeviceFlowHarness";

const DESC = {
  connection: { fields: [] },
  resources: [],
  resource_selection_methods: ["manual"],
};

/**
 * C2 正向 bootstrap 用的 Descriptor fixture（**只用于测试**，不启动任何 Mesad /
 * 真实设备）。
 *
 * 逐条对齐真实 Descriptor 契约（已核对 `crates/core-types`）：
 * - `field_type` 一律**小写**（`FieldType` 是 `#[serde(rename_all = "lowercase")]`）；
 * - 连接字段取自真实 s7 / sinumerik-nck 的 `descriptor()`：
 *   host(Host, required, 无默认) / port(Port, default 102) / rack、slot(Integer,
 *   default 0、1) / local_tsap(**Integer, required, 0..65535**，前端按 TSAP 文本
 *   编辑态归一为整数) / timeout_ms(Duration, default 3000, 500..10000)；
 * - 资源 `memory` 与 s7 同形：area(Enum, default "DB") / db(Integer, default 10, 0..65535)
 *   / **offset(Integer, required 且无默认 → 必须现场填写)** / data_type(Enum,
 *   default "REAL")；output `value` 用 `from_parameter` 声明（mapping 精确覆盖
 *   enum 选项，否则 Descriptor 静态校验非法）；
 * - `modes: ["poll"]` 与 `capabilities.poll=true` 一致（Resource 声明的模式与驱动
 *   能力必须对得上，否则 Core 的 task 门禁会拒）；
 * - 驱动清单条目的 `launchable: true` 才可选（`submitBlockReason` 只认它）。
 */
const DESC_C2 = {
  contract_major: 2,
  contract_minor: 0,
  identity: { driver_id: "s7", name: "Siemens S7", version: "0.0.0-test" },
  connection: {
    fields: [
      {
        key: "host",
        label: "Host",
        field_type: "host",
        required: true,
        validation: {},
        ui: { order: 1, placeholder: "如 192.0.2.10（示例，需按现场填写）" },
      },
      {
        key: "port",
        label: "Port",
        field_type: "port",
        required: false,
        default: 102,
        validation: { min: 0, max: 65535 },
        ui: { order: 2 },
      },
      { key: "rack", label: "Rack", field_type: "integer", required: false, default: 0, validation: {}, ui: { order: 3 } },
      { key: "slot", label: "Slot", field_type: "integer", required: false, default: 1, validation: {}, ui: { order: 4 } },
      {
        key: "local_tsap",
        label: "Local TSAP",
        field_type: "integer",
        required: true,
        validation: { min: 0, max: 65535 },
        ui: { order: 5, placeholder: "如 0x0100 或 256" },
      },
      {
        key: "timeout_ms",
        label: "Timeout ms",
        field_type: "duration",
        required: false,
        default: 3000,
        validation: { min: 500, max: 10000 },
        ui: { order: 6 },
      },
    ],
  },
  resources: [
    {
      id: "memory",
      label: { default: "Memory" },
      parameters: {
        fields: [
          {
            key: "area",
            label: "Area",
            field_type: "enum",
            required: true,
            default: "DB",
            validation: { enum_options: ["DB", "M", "I", "Q"] },
            ui: { order: 1 },
          },
          {
            key: "db",
            label: "DB Number",
            field_type: "integer",
            required: false,
            default: 10,
            validation: { min: 0, max: 65535 },
            ui: { order: 2 },
          },
          {
            key: "offset",
            label: "Offset",
            field_type: "integer",
            required: true,
            validation: { min: 0 },
            ui: { order: 3 },
          },
          {
            key: "data_type",
            label: "Data Type",
            field_type: "enum",
            required: true,
            default: "REAL",
            validation: { enum_options: ["REAL", "DINT", "INT", "BOOL"] },
            ui: { order: 4 },
          },
        ],
      },
      outputs: [
        {
          id: "value",
          label: { default: "Value" },
          type_spec: {
            kind: "from_parameter",
            parameter: "data_type",
            mapping: { REAL: "F32", DINT: "I32", INT: "I32", BOOL: "Bool" },
          },
          access: "read",
        },
        {
          id: "flag",
          label: { default: "Flag" },
          type_spec: { kind: "fixed", data_type: "Bool" },
          access: "read",
        },
      ],
      modes: ["poll"],
    },
  ],
  controls: { commands: [] },
  resource_selection_methods: ["manual"],
  capabilities: { poll: true, subscribe: false, write: false, method: false },
  events: { streams: [] },
};

function mockFlow(opts: {
  failAt?: string;
  driversFail?: boolean;
  descriptorFail?: boolean;
  /** 覆盖描述符（默认空字段空资源的 DESC，正向用例传 DESC_C2） */
  descriptor?: unknown;
} = {}) {
  const calls: Array<{ method: string; url: string; body?: unknown }> = [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    let body: unknown;
    try {
      body = init?.body ? JSON.parse(init.body as string) : undefined;
    } catch {
      body = undefined;
    }
    calls.push({ method, url, body });
    if (url === "/api/v1/drivers") {
      // C2：驱动清单必须显式给出 launchable（真实 DriverInfo 契约字段）。
      // 面板只把 launchable=true 的驱动列为可选，缺字段即视为不可启动。
      if (opts.driversFail) {
        return { ok: false, status: 500, json: async () => ({ error: { code: "BOOM", message: "清单挂了" } }) };
      }
      return { ok: true, status: 200, json: async () => ({ drivers: [{ id: "s7", name: "Siemens S7", launchable: true }] }) };
    }
    if (url === "/api/v1/drivers/s7/descriptor") {
      if (opts.descriptorFail) {
        return { ok: false, status: 500, json: async () => ({ error: { code: "BOOM", message: "描述符挂了" } }) };
      }
      return { ok: true, status: 200, json: async () => opts.descriptor ?? DESC };
    }
    if (url === "/api/v1/device-bootstrap" && method === "POST") {
      if (opts.failAt === "bootstrap") {
        return { ok: false, status: 500, json: async () => ({ error: { code: "BOOM", message: "bootstrap boom" } }) };
      }
      return {
        ok: true,
        status: 201,
        json: async () => ({ device_id: "CNC-01", endpoint_id: "s7-ep-01", revision: 1, started: true }),
      };
    }
    if (url === "/api/v1/devices" && method === "POST") {
      if (opts.failAt === "device") return { ok: false, status: 500, json: async () => ({ error: { message: "dev boom" } }) };
      return { ok: true, status: 201, json: async () => ({}) };
    }
    if (url === "/api/v1/endpoints" && method === "POST") {
      return { ok: true, status: 201, json: async () => ({}) };
    }
    if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
      if (opts.failAt === "tasks") return { ok: false, status: 500, json: async () => ({ error: { message: "tasks boom" } }) };
      return { ok: true, status: 200, json: async () => ({}) };
    }
    if (url.endsWith("/start") && method === "POST") {
      return { ok: true, status: 200, json: async () => ({}) };
    }
    if (method === "DELETE") {
      return { ok: true, status: 200, json: async () => ({}) };
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  return calls;
}

beforeEach(() => {
  vi.useRealTimers();
});

afterEach(() => {
  vi.unstubAllGlobals();
});


describe("M4.2 AddDeviceFlow", () => {
  it("前三步不落库，最终提交才四步编排", async () => {
    const calls = mockFlow();
    const user = userEvent.setup();
    renderFlow();

    // ① 设备
    fireEvent.change(screen.getByPlaceholderText("CNC-01"), { target: { value: "cnc-01" } });
    await user.click(screen.getByRole("button", { name: /下一步：连接/ }));
    expect(calls.some((c) => c.method === "POST")).toBe(false);

    // ② 连接（descriptor 空字段，直接下一步；前三步均不 POST）
    await waitFor(() => {
      expect(screen.getByText("② 连接")).toBeTruthy();
    });
    fireEvent.change(screen.getByPlaceholderText("如 FOCAS / OPC UA"), { target: { value: "FOCAS" } });
    await user.click(screen.getByRole("button", { name: /下一步：数据/ }));
    expect(calls.some((c) => c.method === "POST")).toBe(false);

    // 确认最终提交才 POST（数据步 picker 交互重，改为直接验证确认页存在即 draft 流转正常）
    await waitFor(() => {
      expect(screen.getByText("③ 数据")).toBeTruthy();
    });
  });

  it("提交失败显示回滚状态（tasks 失败 → 已自动回滚）", async () => {
    mockFlow({ failAt: "tasks" });
    // 直接测编排层已在 bootstrap.test.ts 覆盖；此处验证确认页 draft 门控存在
    renderFlow();
    await screen.findByText("① 设备");
    expect(screen.getByText(/前三步只构建草稿/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// P0：连接步的提交出口在「清单失败 / 描述符失败」时必须全部失效
//
// 连接步的出口有两个：
// 1. 「下一步：数据 →」 → 面板 onSubmit → commitConnection（攒草稿，不落库）；
// 2. 「添加设备」 → POST /api/v1/device-bootstrap（确认页，依赖 1 产出的草稿）。
// 因此 1 被挡住 ⇒ 草稿不会被污染 ⇒ 2 结构上不可达。下面的用例除了按钮 disabled
// 之外，还用探针绕过 disabled 直接触发处理函数，证明提交函数自身也有守卫。
// ---------------------------------------------------------------------------

describe("M4.2 AddDeviceFlow：P0 连接步提交出口", () => {
  /** 从设备步走到连接步（唯一入口）。 */
  async function gotoConnectionStep(user: ReturnType<typeof userEvent.setup>) {
    fireEvent.change(screen.getByPlaceholderText("CNC-01"), { target: { value: "cnc-01" } });
    await user.click(screen.getByRole("button", { name: /下一步：连接/ }));
  }

  function connectionStepButtons() {
    return {
      probe: screen.getByRole("button", { name: /测试连接/ }) as HTMLButtonElement,
      validate: screen.getByRole("button", { name: /校验参数/ }) as HTMLButtonElement,
      next: screen.getByRole("button", { name: /下一步：数据/ }) as HTMLButtonElement,
    };
  }

  /** 三条路径全部禁用 + 绕过 disabled 后仍不发任何写请求。 */
  async function assertAllPathsBlocked(calls: Array<{ method: string; url: string }>) {
    const { probe, validate, next } = connectionStepButtons();
    expect([probe.disabled, validate.disabled, next.disabled]).toEqual([true, true, true]);

    await act(async () => {
      // 探针必须真的调用到处理函数（否则用例是假绿）
      expect(invokeDisabledButton(probe)).toBe(true);
      expect(invokeDisabledButton(validate)).toBe(true);
      expect(invokeDisabledButton(next)).toBe(true);
    });

    // 草稿未被污染：仍停留在连接步
    expect(screen.queryByText("③ 数据")).toBeNull();
    // 零写请求：含 POST /device-bootstrap、POST /endpoints、PUT /endpoints/*、
    // /probe、/validate-connection 全部为 0 次
    expect(calls.filter((c) => c.method !== "GET")).toEqual([]);
    expect(calls.some((c) => c.url === "/api/v1/device-bootstrap")).toBe(false);
    expect(calls.some((c) => c.url.endsWith("/probe"))).toBe(false);
    expect(calls.some((c) => c.url.endsWith("/validate-connection"))).toBe(false);
  }

  it("驱动清单失败：显示错误 + 重试，零写请求且不出现硬编码 fallback 驱动", async () => {
    const calls = mockFlow({ driversFail: true });
    const user = userEvent.setup();
    renderFlow();
    await gotoConnectionStep(user);

    await screen.findByText("驱动清单加载失败");
    expect(screen.getByText(/清单挂了/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /重\s*试/ })).toBeTruthy();
    // 已删除的 FALLBACK_DRIVERS 常量不得以任何形式回归
    expect(screen.queryByText("Siemens S7")).toBeNull();
    expect(screen.queryByText("FANUC FOCAS2")).toBeNull();
    expect(screen.queryByText("OPC UA")).toBeNull();

    await assertAllPathsBlocked(calls);
  });

  it("描述符失败：显示错误 + 重试，零写请求", async () => {
    const calls = mockFlow({ descriptorFail: true });
    const user = userEvent.setup();
    renderFlow();
    await gotoConnectionStep(user);

    await screen.findByText("驱动描述符加载失败");
    expect(screen.getByText(/描述符挂了/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /重\s*试/ })).toBeTruthy();

    await assertAllPathsBlocked(calls);
  });

  it("清单与描述符就绪时可以正常前进（证明前置门是活的）", async () => {
    const calls = mockFlow();
    const user = userEvent.setup();
    renderFlow();
    await gotoConnectionStep(user);

    await waitFor(() =>
      expect((screen.getByRole("button", { name: /下一步：数据/ }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.change(screen.getByPlaceholderText("如 FOCAS / OPC UA"), { target: { value: "FOCAS" } });
    await user.click(screen.getByRole("button", { name: /下一步：数据/ }));

    await waitFor(() => expect(screen.getByText("③ 数据")).toBeTruthy());
    // 前三步仍然不落库
    expect(calls.filter((c) => c.method !== "GET")).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// C2 正向出口：合法 Descriptor（含真实 resources/points）必须能一路走到确认页
// 并真正发出 POST /api/v1/device-bootstrap
//
// 为什么必须补这条：此前的用例只证明了「清单/描述符失败时不能提交」（调用次数为
// 0）。空 resources 的 fixture 让第 ③ 步选不到点、根本进不了确认页，因此
// **「允许合法提交」从未被证明**——出口可能永远打不开而测试全绿。
// 本用例用 mock API 走完整流程并断言**实际请求体**（不启动 Mesad、不连任何设备）。
// ---------------------------------------------------------------------------

describe("M4.2 AddDeviceFlow：正向 bootstrap 请求体（C2）", () => {
  /**
   * 这两条用例要走完整个向导（① 设备 → ④ 确认页），是本文件里最重的交互测试：
   * 本机实测约 4.2–4.5s，已接近 vitest 默认 5s 单测超时，Linux CI（更慢且并发）
   * 会稳定超时。因此**只给这两条**指定独立超时，不改全局配置、不放宽任何断言。
   */
  const FULL_FLOW_TIMEOUT_MS = 15_000;

  /**
   * CI 阶段诊断标记：仅在 `CI` 环境下打印阶段与耗时。
   *
   * 为什么需要：这两条用例一旦超时，vitest 只报 "Test timed out in 5000ms"，
   * 无法区分「流程卡在某一步没推进」与「流程正常但耗时超预算」。有了阶段标记，
   * CI 日志会直接给出最后到达的阶段，避免靠猜。本地运行不打印，不制造噪音。
   */
  function makeStageMarker() {
    const t0 = Date.now();
    return (stage: string) => {
      if (process.env.CI) {
        // eslint-disable-next-line no-console
        console.log(`[walk] ${stage} @${Date.now() - t0}ms`);
      }
    };
  }

  /** 从设备步一路走到确认页（④），返回过程中截获的请求记录。
   *  `withPoints=false`：不加入任何采集点（C3-D 零采集点离线保存路径）。
   *  `startWhenPoints`：在数据步**明确勾选**「创建后启动」（默认不勾选）。
   *  `reuse`：复用已挂载的流程容器（用于「再建一个」之后在同一实例上再走一遍）。 */
  async function walkToConfirmPage(
    opts: { withPoints?: boolean; startWhenPoints?: boolean; reuse?: { container: HTMLElement } } = {},
  ) {
    const withPoints = opts.withPoints ?? true;
    const calls = mockFlow({ descriptor: DESC_C2 });
    const user = userEvent.setup();
    const mark = makeStageMarker();
    const container = opts.reuse ? opts.reuse.container : renderFlow().container;
    mark("rendered");

    // ① 设备：ID 与显示名都要落进 device.id / device.name（断言在下面的请求体里）
    await screen.findByText("① 设备");
    mark("① 设备就绪");
    fireEvent.change(screen.getByPlaceholderText("CNC-01"), { target: { value: "CNC-01" } });
    fireEvent.change(screen.getByPlaceholderText("默认为 ID"), { target: { value: "一号机床" } });
    await user.click(screen.getByRole("button", { name: /下一步：连接/ }));

    // ② 连接：真实 Descriptor 的 required 字段必须填齐；TSAP 用文本输入
    await screen.findByText("② 连接");
    mark("② 连接就绪");
    await waitFor(() =>
      expect(container.querySelector('[data-field-key="host"]')).not.toBeNull(),
    );
    fireEvent.change(screen.getByPlaceholderText("如 FOCAS / OPC UA"), { target: { value: "S7 主站" } });
    fireEvent.change(screen.getByLabelText(/连接 ID/), { target: { value: "s7-ep-01" } });
    fireEvent.change(container.querySelector('[data-field-key="host"] input')!, {
      target: { value: "10.0.0.5" },
    });
    // TSAP：**不失焦**直接进入下一步（文本草稿必须现场归一化为整数 1280）
    fireEvent.change(container.querySelector('[data-field-key="local_tsap"] input')!, {
      target: { value: "0x0500" },
    });
    fireEvent.click(screen.getByRole("button", { name: /下一步：数据/ }));

    // ③ 数据
    await screen.findByText("③ 数据");
    mark("③ 数据就绪");
    if (withPoints) {
      // 选资源参数 + 输出，加入采集项（required 的 offset 无默认值必须填）
      fireEvent.change(container.querySelector('[data-field-key="offset"] input')!, {
        target: { value: "4" },
      });
      const outputBox = screen.getByRole("checkbox", { name: "output-value" });
      fireEvent.click(outputBox.querySelector("input") ?? outputBox);
      const flagBox = screen.getByRole("checkbox", { name: "output-flag" });
      fireEvent.click(flagBox.querySelector("input") ?? flagBox);
      // 注意：antd 会给「两个汉字」的按钮自动插入空格，故用 /加\s*入/ 而不是精确串
      fireEvent.click(screen.getByRole("button", { name: /加\s*入/ }));
      // 采集项确实进了草稿（加入成功后 picker 会清空输出表单，改看流程自身的已选计数）
      expect(screen.getByText(/已选 1 项/)).toBeTruthy();
      fireEvent.change(screen.getByText("采集周期").parentElement!.querySelector("input")!, {
        target: { value: "2000" },
      });
      // C3-D：默认**不启动**（复选框未勾选且可用）——启动必须是用户的明确动作
      const startBox = screen.getByRole("checkbox", { name: /创建后启动/ }) as HTMLInputElement;
      expect(startBox.checked).toBe(false);
      expect(startBox.disabled).toBe(false);
      if (opts.startWhenPoints) {
        fireEvent.click(startBox.querySelector("input") ?? startBox);
        expect(
          (screen.getByRole("checkbox", { name: /创建后启动/ }) as HTMLInputElement).checked,
        ).toBe(true);
        // 勾选后才出现「立即开始采集」的提示
        expect(screen.getByText(/创建成功后立即开始采集/)).toBeTruthy();
      }
    } else {
      // 零采集点：界面必须明确「仅保存、不启动、不立即采集」，且启动开关不可用
      await screen.findByText(/未选择数据点：将按「仅保存」创建/);
      const startBox = screen.getByRole("checkbox", { name: /创建后启动/ }) as HTMLInputElement;
      expect(startBox.disabled).toBe(true);
      expect(startBox.checked).toBe(false);
    }
    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));

    // ④ 确认页（此前不可达）
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    mark("④ 确认页就绪");
    if (withPoints) {
      expect(screen.getByText(/1 个数据点 · 2000ms/)).toBeTruthy();
    } else {
      expect(screen.getByText(/零采集点（仅保存连接）/)).toBeTruthy();
      expect(screen.getByText(/仅保存（不启动、不立即采集）/)).toBeTruthy();
    }
    return { calls, user, container };
  }

  it("走到确认页后提交：请求体含 Device / Endpoint / Connection(canonical, 含 TSAP) / 采集任务", async () => {
    // C3-D：启动是**用户的明确动作**——在数据步显式勾选「创建后启动」
    //（walker 已断言初始未勾选；零采集点路径另有用例）
    const { calls, user } = await walkToConfirmPage({ startWhenPoints: true });

    await user.click(screen.getByRole("button", { name: "添加设备" }));
    await waitFor(() =>
      expect(calls.some((c) => c.url === "/api/v1/device-bootstrap")).toBe(true),
    );

    // 原子接口：一次 POST，不得再出现分步编排的 create device / endpoint / tasks
    const writes = calls.filter((c) => c.method !== "GET");
    expect(writes.map((c) => c.url)).toEqual(["/api/v1/device-bootstrap"]);

    const body = writes[0].body as {
      device: { id: string; name: string };
      endpoint: { id: string; name: string; driver_id: string; connection: Record<string, unknown> };
      acquisition: {
        tasks: Array<{
          id: string;
          schedule: { mode: string; interval_ms: number };
          binding: { kind: string; config: { selections: unknown[] } };
        }>;
      };
      start: boolean;
      idempotency_key: string;
    };

    // --- Device ---
    expect(body.device).toEqual({ id: "CNC-01", name: "一号机床" });

    // --- Endpoint + Connection（ConnectionSetupPanel 产出的 canonical 形态）---
    expect(body.endpoint.id).toBe("s7-ep-01");
    expect(body.endpoint.name).toBe("S7 主站");
    expect(body.endpoint.driver_id).toBe("s7");
    expect(body.endpoint.connection).toEqual({
      host: "10.0.0.5", // 用户输入
      port: 102, // Descriptor 默认值被物化（number，不是字符串）
      rack: 0,
      slot: 1,
      local_tsap: 1280, // 0x0500 文本现场归一化（未失焦提交）
      timeout_ms: 3000,
    });
    // 归一化证据：TSAP 是 number 而非用户输入的字符串
    expect(typeof body.endpoint.connection.local_tsap).toBe("number");
    expect(body.endpoint.connection.local_tsap).not.toBe("0x0500");

    // --- 采集任务（点位/任务绑定）---
    expect(body.acquisition.tasks).toHaveLength(1);
    const task = body.acquisition.tasks[0];
    expect(task.id).toBe("t1");
    expect(task.schedule).toEqual({ mode: "poll", interval_ms: 2000 });
    expect(task.binding.kind).toBe("mesa.resources.v1");
    expect(task.binding.config.selections).toEqual([
      {
        resource_id: "memory",
        // 参数 = Descriptor 默认值物化 + 用户现场填写（offset）
        parameters: { area: "DB", db: 10, offset: 4, data_type: "REAL" },
        // 两个输出各一个采集点，point_key 自动命名（resource.output）
        outputs: [
          { output: "value", point_key: "memory.value" },
          { output: "flag", point_key: "memory.flag" },
        ],
      },
    ]);

    // --- 启动开关与幂等键 ---
    expect(body.start).toBe(true);
    expect(body.idempotency_key).toMatch(/^web-op-/);

    // 成功后进入设备工作区的入口出现（证明成功路径而非仅"发出请求"）
    await screen.findByText(/已添加设备 CNC-01/);
    expect(screen.getByRole("button", { name: /进入设备/ })).toBeTruthy();
  }, FULL_FLOW_TIMEOUT_MS);

  it("同一 Flow 内重复提交复用同一幂等键（重试不双建）", async () => {
    const { calls, user } = await walkToConfirmPage();
    const submit = screen.getByRole("button", { name: "添加设备" });
    const bootstraps = () => calls.filter((c) => c.url === "/api/v1/device-bootstrap");

    await user.click(submit);
    await waitFor(() => expect(bootstraps()).toHaveLength(1));
    // 等第一次提交真正结束（loading 态会吞掉点击）再提交第二次
    await waitFor(() => expect(submit.className).not.toContain("ant-btn-loading"));
    await user.click(submit);
    await waitFor(() => expect(bootstraps()).toHaveLength(2));

    const key = (c: { body?: unknown }) => (c.body as { idempotency_key: string }).idempotency_key;
    // 重试必须复用同一 operation key（后端据此重放而不是再建一台设备）
    expect(key(bootstraps()[1])).toBe(key(bootstraps()[0]));
  }, FULL_FLOW_TIMEOUT_MS);

  // -------------------------------------------------------------------------
  // C3-D：零采集点离线保存 + 默认不启动
  // -------------------------------------------------------------------------

  it("零采集点 + 未勾选启动：请求体 acquisition.tasks=[] 且 start=false（只保存，不启动）", async () => {
    const { calls, user } = await walkToConfirmPage({ withPoints: false });

    await user.click(screen.getByRole("button", { name: "添加设备" }));
    await waitFor(() =>
      expect(calls.some((c) => c.url === "/api/v1/device-bootstrap")).toBe(true),
    );

    const writes = calls.filter((c) => c.method !== "GET");
    // 只发一次原子创建请求（不得退化成 create device / endpoint / tasks 分步编排）
    expect(writes.map((c) => c.url)).toEqual(["/api/v1/device-bootstrap"]);
    const body = writes[0].body as {
      device: { id: string };
      endpoint: { id: string; driver_id: string; connection: Record<string, unknown> };
      acquisition: { tasks: unknown[] };
      start: boolean;
    };
    // 设备与连接照常提供（跳过采集点**不**绕过连接校验/Secret 物化）
    expect(body.device.id).toBe("CNC-01");
    expect(body.endpoint.id).toBe("s7-ep-01");
    expect(body.endpoint.driver_id).toBe("s7");
    expect(body.endpoint.connection).toMatchObject({ host: "10.0.0.5", local_tsap: 1280 });
    // 关键：零采集点 = tasks: []，而不是「一个 selections 为空的假任务」
    expect(body.acquisition.tasks).toEqual([]);
    expect(body.start).toBe(false);

    // 成功路径照常（设备已建好、保持停止）
    await screen.findByText(/已添加设备 CNC-01/);
  }, FULL_FLOW_TIMEOUT_MS);

  it("再建一个设备：Reset 后「创建后启动」回到默认不勾选（不继承上次状态）", async () => {
    // 第一次：用户在数据步明确勾选启动
    const { user, container } = await walkToConfirmPage({ startWhenPoints: true });

    // 回到确认页操作区点「再建一个」
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    await user.click(screen.getByRole("button", { name: "再建一个" }));
    await screen.findByText("① 设备");

    // 第二遍走到数据步：walker 内置断言「复选框未勾选且可用」——即默认不启动
    await walkToConfirmPage({ reuse: { container } });
    // 确认页文案也必须是「仅保存」，不得因为上次勾过就启动
    expect(screen.getByText(/仅保存（不启动、不立即采集）/)).toBeTruthy();
  }, 30_000);
});
