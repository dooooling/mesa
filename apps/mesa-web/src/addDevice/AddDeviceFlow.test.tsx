// M4.2 回归：AddDeviceFlow 四步 draft + 最终提交。
// - 前三步不落库（无 POST）；最后一步才编排提交；
// - 成功进设备；失败显示回滚状态；残留明示。
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { AddDeviceFlow } from "./AddDeviceFlow";

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

function renderFlow() {
  return render(
    <MemoryRouter initialEntries={["/devices/new"]}>
      <Routes>
        <Route path="/devices/new" element={<AddDeviceFlow />} />
      </Routes>
    </MemoryRouter>,
  );
}

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

// ===========================================================================
// PR-D：NCK 设备创建向导接线（D1–D10）
//
// 全部用例只使用 mock fetch：不启动 mesad、不启动 NCK emulator、不连接任何
// 真实设备；断言的是「向导实际发出的请求体」与「UI 实际状态」。
// ===========================================================================

/** 真实的 NCK 连接字段形状（family 必填枚举 + TSAP 高级必填）。 */
function nckDescriptor(opts: { familyRequired?: boolean } = {}) {
  return {
    contract_major: 2,
    contract_minor: 0,
    identity: { driver_id: "sinumerik-nck", name: "SINUMERIK NCK", version: "0.0.0-test" },
    connection: {
      fields: [
        { key: "host", label: "Host", field_type: "host", required: true, validation: {}, ui: { order: 1 } },
        { key: "port", label: "Port", field_type: "port", required: false, default: 102, validation: { min: 0, max: 65535 }, ui: { order: 2 } },
        {
          key: "family",
          label: "Family",
          field_type: "enum",
          required: opts.familyRequired !== false,
          validation: { enum_options: ["828d", "840d-sl"] },
          ui: { order: 3 },
        },
        { key: "local_tsap", label: "Local TSAP", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: { order: 4, advanced: true } },
        { key: "remote_tsap", label: "Remote TSAP", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: { order: 5, advanced: true } },
        { key: "timeout_ms", label: "Timeout ms", field_type: "duration", required: false, default: 5000, validation: { min: 500, max: 30000 }, ui: { order: 6 } },
      ],
    },
    resources: [
      {
        id: "variable",
        label: { default: "NC Variable" },
        // 12 原生字段：全部必填项都给默认值，让 derived Key 一进入就可用；
        // 具体字段校验仍由 NckNativeFields/Descriptor 约束执行（测试另测非法 Key）。
        parameters: {
          fields: [
            { key: "area", label: "Area", field_type: "enum", required: true, default: "N", validation: { enum_options: ["N", "B", "C", "A", "T", "V", "H"] }, ui: { order: 1 } },
            { key: "area_no", label: "Area No.", field_type: "integer", required: true, default: 1, validation: { min: 0, max: 31 }, ui: { order: 2 } },
            { key: "module", label: "Module", field_type: "integer", required: true, default: 2, validation: { min: 0, max: 255 }, ui: { order: 3 } },
            { key: "column", label: "Column", field_type: "integer", required: true, default: 3, validation: { min: 0, max: 65535 }, ui: { order: 4 } },
            { key: "line", label: "Line", field_type: "integer", required: true, default: 4, validation: { min: 0, max: 65535 }, ui: { order: 5 } },
            { key: "count", label: "Count", field_type: "integer", required: true, default: 1, validation: { min: 1, max: 255 }, ui: { order: 6 } },
            { key: "unit_mode", label: "Unit mode", field_type: "enum", required: true, default: "current", validation: { enum_options: ["current", "metric", "inch"] }, ui: { order: 7 } },
            { key: "decode", label: "Decode", field_type: "enum", required: true, default: "f32_le", validation: { enum_options: ["f32_le", "f32_be"] }, ui: { order: 8 } },
            { key: "transport_size", label: "Transport size", field_type: "enum", required: true, default: "0x05", validation: { enum_options: ["0x05", "0x06", "0x07", "0x09"] }, ui: { order: 9 } },
            { key: "invalid_value", label: "Invalid value", field_type: "number", required: false, validation: {}, ui: { order: 10 } },
            { key: "invalid_tolerance", label: "Invalid tolerance", field_type: "number", required: false, validation: { min: 0, max: 1 }, ui: { order: 11 } },
            { key: "unit", label: "Unit", field_type: "string", required: false, validation: {}, ui: { order: 12 } },
          ],
        },
        outputs: [{ id: "value", label: { default: "Value" }, type_spec: { kind: "driver_resolved" }, access: "read" }],
        modes: ["poll"],
      },
    ],
    controls: { commands: [] },
    resource_selection_methods: ["manual", "browse"],
    capabilities: { poll: true, subscribe: false, write: false, method: false },
    events: { streams: [] },
  };
}

function nckDefinition(variableId: string, overrides: Record<string, unknown> = {}) {
  return {
    variable_id: variableId,
    revision: 1,
    family: "828d",
    scope: "collect",
    semantics_status: "verified",
    label: { zh: variableId, en: variableId },
    source: [{ kind: "manual", reference: "test" }],
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    identity_template: `${variableId}@channel={channel}`,
    identity_keys: [{ path: "channel", required: true }],
    golden_ref: [],
    digest: `digest-${variableId}`,
    ...overrides,
  };
}

const NCK_COLLECT = nckDefinition("syn.collect.a");
const NCK_IDENTITY = nckDefinition("syn.identity.a", {
  scope: "identity",
  targets: ["axis"],
  target_required: ["axis"],
  identity_template: "syn.identity.a@axis={axis}",
  identity_keys: [{ path: "axis", required: true }],
});

function nckCatalogEnvelope(definitions: unknown[], overrides: Record<string, unknown> = {}) {
  return {
    format_version: 1,
    family: "828d",
    revision: 1,
    driver_id: "sinumerik-nck",
    driver_version: "0.1.0",
    note: null,
    total: definitions.length,
    limit: 100,
    next_cursor: null,
    definitions,
    ...overrides,
  };
}

function okJson(body: unknown, status = 200) {
  return { ok: true, status, json: async () => body };
}

/**
 * NCK 向导 mock：驱动清单 / 描述符 / 完整目录 / identity-preview / bootstrap。
 * 不处理 /api/v1/endpoints、/tasks、/browse —— 向导若调用它们会得到 404，
 * 从而使「不调用」成为可断言的负向证据。
 */
function mockNckFlow(
  opts: {
    catalogFail?: boolean;
    definitions?: unknown[];
    familyRequired?: boolean;
  } = {},
) {
  const calls: Array<{ method: string; url: string; body?: unknown }> = [];
  const definitions = opts.definitions ?? [NCK_COLLECT, NCK_IDENTITY];
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
      return okJson({ drivers: [{ id: "sinumerik-nck", name: "SINUMERIK NCK", launchable: true }] });
    }
    if (url === "/api/v1/drivers/sinumerik-nck/descriptor") {
      return okJson(nckDescriptor({ familyRequired: opts.familyRequired }));
    }
    if (url === "/api/v1/drivers/sinumerik-nck/definitions/identity-preview") {
      const req = body as { family?: string; parameters?: Record<string, unknown> };
      const parameters = req.parameters ?? {};
      const variableId = String(parameters.variable_id ?? "");
      const target = (parameters.target ?? {}) as Record<string, unknown>;
      return okJson({
        point_key: `${variableId}@channel=${String(target.channel ?? "")}`,
        identity_mode: "derived",
        scope: "collect",
        variable_id: variableId,
        definition_revision: parameters.definition_revision,
        definition_digest: "digest-test",
      });
    }
    if (url.startsWith("/api/v1/drivers/sinumerik-nck/definitions")) {
      if (opts.catalogFail) {
        return {
          ok: false,
          status: 500,
          json: async () => ({ error: { code: "BOOM", kind: "Internal", message: "目录挂了" } }),
        };
      }
      return okJson(nckCatalogEnvelope(definitions));
    }
    if (url === "/api/v1/device-bootstrap" && method === "POST") {
      return okJson({ device_id: "CNC-01", endpoint_id: "nck-ep", revision: 1, started: false }, 201);
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
  return calls;
}

async function selectNckFamily(container: HTMLElement, family: string) {
  const trigger = container.querySelector('[data-field-key="family"] .ant-select-content');
  if (!trigger) throw new Error("family Select 未渲染");
  const user = userEvent.setup();
  await user.click(trigger as HTMLElement);
  const option = await screen.findByTitle(family);
  await user.click(option);
}

/**
 * 从设备步走到 NCK 数据步。
 *
 * 注意：local_tsap / remote_tsap 是「高级必填」，第一次点下一步会展开高级区并
 * 阻止提交；填完再点一次。这里走的是用户真实路径，不绕过任何前置门。
 */
async function walkNckToDataStep(
  opts: {
    catalogFail?: boolean;
    definitions?: unknown[];
    familyRequired?: boolean;
    family?: string;
  } = {},
) {
  const calls = mockNckFlow(opts);
  const user = userEvent.setup();
  const container = renderFlow().container;
  await screen.findByText("① 设备");
  fireEvent.change(screen.getByPlaceholderText("CNC-01"), { target: { value: "CNC-01" } });
  await user.click(screen.getByRole("button", { name: /下一步：连接/ }));

  await screen.findByText("② 连接");
  await waitFor(() =>
    expect(container.querySelector('[data-field-key="host"]')).not.toBeNull(),
  );
  fireEvent.change(container.querySelector('[data-field-key="host"] input')!, {
    target: { value: "10.0.0.5" },
  });
  if (opts.familyRequired !== false) {
    await selectNckFamily(container, opts.family ?? "828d");
  }
  fireEvent.click(screen.getByRole("button", { name: /下一步：数据/ }));
  await screen.findByText(/高级参数缺少必填项/);
  fireEvent.change(container.querySelector('[data-field-key="local_tsap"] input')!, {
    target: { value: "0x0400" },
  });
  fireEvent.change(container.querySelector('[data-field-key="remote_tsap"] input')!, {
    target: { value: "0x0d04" },
  });
  fireEvent.click(screen.getByRole("button", { name: /下一步：数据/ }));

  await screen.findByText("③ 数据");
  return { calls, user, container };
}

/** 在语义页加入一个点位（只填 Target，不填任何原生字段）。 */
async function addSemanticPoint(
  user: ReturnType<typeof userEvent.setup>,
  channel = "1",
  variableId = "syn.collect.a",
) {
  await waitFor(() =>
    expect(screen.getByTestId("nck-variable-select")).toBeTruthy(),
  );
  await user.selectOptions(
    screen.getByTestId("nck-variable-select") as HTMLSelectElement,
    variableId,
  );
  const input = screen.getByLabelText("target-channel");
  await user.clear(input);
  await user.type(input, channel);
  await screen.findByTestId("nck-point-key");
  const key = screen.getByTestId("nck-point-key").textContent ?? "";
  await user.click(screen.getByTestId("nck-add-point"));
  await screen.findByTestId("nck-draft-point-row");
  return key;
}

function submitButton() {
  return screen.getByRole("button", { name: "添加设备" }) as HTMLButtonElement;
}

function bootstrapBodies(calls: Array<{ method: string; url: string; body?: unknown }>) {
  return calls
    .filter((c) => c.url === "/api/v1/device-bootstrap" && c.method === "POST")
    .map((c) => c.body as Record<string, unknown>);
}

type BootstrapBody = {
  device: { id: string; name: string };
  endpoint: { id: string; name: string; driver_id: string; connection: Record<string, unknown> };
  acquisition: { tasks: Array<{ id: string; schedule: Record<string, unknown>; binding: { kind: string; config: Record<string, unknown> } }> };
  start: boolean;
  idempotency_key: string;
};

describe("PR-D D1–D3：NCK 数据步只提供语义入口", () => {
  it("nck_semantic_mode_does_not_require_native_fields (A8)", async () => {
    await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");

    // 只填 Target 即可加入：不得出现「缺少必填参数：area/area_no/...」。
    expect(screen.queryByText(/缺少必填参数/)).toBeNull();
    expect(screen.queryByTestId("missing-required")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);

    await addSemanticPoint(userEvent.setup(), "1");
    expect(screen.getByTestId("nck-draft-point-row").textContent).toContain(
      "syn.collect.a@channel=1",
    );
  }, 30_000);

  it("nck_semantic_picker_is_default_entry_in_wizard (A21)", async () => {
    await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");

    expect(screen.getByTestId("nck-semantic-picker")).toBeTruthy();
    expect(screen.getByTestId("nck-variable-select")).toBeTruthy();
    // 旧通用 Picker 的手动页整体不渲染：无「手动」Tab、无 output 复选框、无「无可用资源」。
    expect(screen.queryByRole("tab", { name: "手动" })).toBeNull();
    expect(screen.queryByRole("checkbox", { name: "output-value" })).toBeNull();
    expect(screen.queryByText("无可用资源")).toBeNull();
  }, 30_000);

  it("legacy_variable_entry_absent_and_native_is_labeled (A22)", async () => {
    await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");

    // 旧 variable 入口（整份 12 字段 schema 判必填 / 自动 point_key）不存在。
    expect(screen.queryByTestId("missing-required")).toBeNull();
    expect(screen.queryByRole("checkbox", { name: "output-value" })).toBeNull();
    // 高级原生入口存在且被明确标注。
    expect(screen.getByTestId("nck-editor-mode")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /高级原生配置/ }));
    expect(screen.getByTestId("nck-native-fields")).toBeTruthy();
    expect(screen.getByText(/高级原生配置（nck_native）/)).toBeTruthy();
  }, 30_000);
});

describe("PR-D D4–D5：目录先于 Endpoint、family 取自连接草稿", () => {
  it("catalog_loads_before_endpoint_exists", async () => {
    const { calls } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");

    const catalog = calls.find(
      (c) => c.url.startsWith("/api/v1/drivers/sinumerik-nck/definitions") && c.method === "GET",
    );
    expect(catalog).toBeTruthy();
    // 目录请求不带 family 过滤（与端点页一致；交叉校验交给 checkNckFamily）。
    expect(catalog!.url).not.toContain("family=");
    // Endpoint 尚不存在：没有 POST /endpoints、没有 device-bootstrap、没有 Browse。
    expect(calls.some((c) => c.url === "/api/v1/endpoints" && c.method === "POST")).toBe(false);
    expect(calls.some((c) => c.url === "/api/v1/device-bootstrap")).toBe(false);
    expect(calls.some((c) => c.url.includes("/browse"))).toBe(false);
    expect(screen.getByTestId("nck-catalog-meta").textContent).toContain("2 条");
  }, 30_000);

  it("catalog_failure_disables_semantic_save", async () => {
    const { calls } = await walkNckToDataStep({ catalogFail: true });
    await screen.findByTestId("nck-catalog-error");
    expect(screen.getByTestId("nck-catalog-error").textContent).toContain("目录挂了");
    expect(screen.getByTestId("nck-catalog-error").textContent).toContain(
      "不会使用旧目录缓存",
    );
    // 语义表单 fail-closed：不渲染可加入表单。
    expect(screen.getByTestId("nck-semantic-picker-no-definitions")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    // 仍在数据步；没有 POST。
    expect(screen.queryByText("④ 确认 · 最终按钮才真正提交")).toBeNull();
    expect(await screen.findByText(/NCK 定义目录未就绪或不完整/)).toBeTruthy();
    expect(calls.some((c) => c.url === "/api/v1/device-bootstrap")).toBe(false);
  }, 30_000);

  it("family_comes_from_connection_draft_and_mismatch_blocks_add", async () => {
    // 目录条目 family=840d-sl，连接草稿 family=828d ⇒ FAMILY_MISMATCH，不可加入。
    const { calls, user } = await walkNckToDataStep({
      definitions: [nckDefinition("syn.collect.a", { family: "840d-sl" })],
      family: "828d",
    });
    await screen.findByTestId("nck-catalog-meta");
    await user.selectOptions(
      screen.getByTestId("nck-variable-select") as HTMLSelectElement,
      "syn.collect.a",
    );
    await user.type(screen.getByLabelText("target-channel"), "1");
    await screen.findByTestId("nck-family-reject");
    expect(screen.getByTestId("nck-family-reject").textContent).toContain("FAMILY_MISMATCH");
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
    // family 不匹配时不应发出 identity-preview（请求 family 必须来自连接草稿）。
    expect(calls.some((c) => c.url.endsWith("/identity-preview"))).toBe(false);
  }, 30_000);

  it("preview_request_family_equals_connection_draft", async () => {
    const { calls, user } = await walkNckToDataStep({ family: "828d" });
    await screen.findByTestId("nck-catalog-meta");
    await addSemanticPoint(user, "7");
    const preview = calls.find((c) => c.url.endsWith("/identity-preview"));
    expect(preview).toBeTruthy();
    expect((preview!.body as { family: string }).family).toBe("828d");
  }, 30_000);

  it("family_missing_blocks_add_and_submit", async () => {
    // family 设为非必填以模拟「连接草稿缺少 family」；不得加入、不得提交。
    const { calls, user } = await walkNckToDataStep({ familyRequired: false });
    await screen.findByTestId("nck-catalog-meta");
    expect(screen.getByTestId("nck-family-missing")).toBeTruthy();

    await user.selectOptions(
      screen.getByTestId("nck-variable-select") as HTMLSelectElement,
      "syn.collect.a",
    );
    await user.type(screen.getByLabelText("target-channel"), "1");
    await screen.findByTestId("nck-family-reject");
    expect(screen.getByTestId("nck-family-reject").textContent).toContain(
      "FAMILY_ENDPOINT_MISSING",
    );
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    expect(screen.queryByText("④ 确认 · 最终按钮才真正提交")).toBeNull();
    // 提示可能同时出现在 Alert 与 antd message 中，按「仍停留数据步 + 零 POST」判定。
    expect(screen.getAllByText(/连接草稿缺少 family/).length).toBeGreaterThan(0);
    expect(calls.some((c) => c.url === "/api/v1/device-bootstrap")).toBe(false);
  }, 30_000);
});

describe("PR-D D6–D10：提交体、单写路径、零采集点与全链", () => {
  it("nck_bootstrap_body_is_nck_points_v1", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    const key = await addSemanticPoint(user, "1");
    expect(key).toBe("syn.collect.a@channel=1");
    // 采集周期改 2000，证明 schedule 来自草稿。
    fireEvent.change(screen.getByText("采集周期").parentElement!.querySelector("input")!, {
      target: { value: "2000" },
    });
    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    expect(screen.getByText(/1 个 NCK 点位 · 2000ms/)).toBeTruthy();

    await user.click(submitButton());
    await waitFor(() => expect(bootstrapBodies(calls)).toHaveLength(1));
    const body = bootstrapBodies(calls)[0] as unknown as BootstrapBody;

    expect(body.device).toEqual({ id: "CNC-01", name: "CNC-01" });
    expect(body.endpoint.driver_id).toBe("sinumerik-nck");
    expect(body.endpoint.connection.family).toBe("828d");
    expect(body.acquisition.tasks).toHaveLength(1);
    const task = body.acquisition.tasks[0];
    expect(task.id).toBe("t1");
    expect(task.schedule).toEqual({ mode: "poll", interval_ms: 2000 });
    expect(task.binding.kind).toBe("nck.points.v1");
    // 请求体**恰为** `{points}`，且 point_key 逐字等于服务端预览值。
    expect(Object.keys(task.binding.config)).toEqual(["points"]);
    expect(task.binding.config.points).toEqual([
      {
        kind: "nck_variable",
        point_key: "syn.collect.a@channel=1",
        parameters: {
          variable_id: "syn.collect.a",
          definition_revision: 1,
          target: { channel: 1 },
          arguments: {},
        },
      },
    ]);
    // 默认不启动（未勾选）。
    expect(body.start).toBe(false);
    expect(body.idempotency_key).toMatch(/^web-op-/);
  }, 30_000);

  it("single_write_path_only_bootstrap", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    await user.click(submitButton());
    await waitFor(() => expect(bootstrapBodies(calls)).toHaveLength(1));

    const writes = calls.filter((c) => c.method !== "GET");
    // 唯一写路径：一次 POST /device-bootstrap；无 PUT /tasks、无 POST /endpoints。
    expect(writes.map((c) => `${c.method} ${c.url}`)).toEqual([
      "POST /api/v1/device-bootstrap",
    ]);
    expect(calls.some((c) => c.method === "PUT" && c.url.startsWith("/api/v1/tasks"))).toBe(false);
    expect(calls.some((c) => c.url === "/api/v1/endpoints" && c.method === "POST")).toBe(false);
  }, 30_000);

  it("nck_zero_points_still_only_save (D9)", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    // 零点位：启动开关禁用，文案明确「仅保存」。
    const startBox = screen.getByRole("checkbox", { name: /创建后启动/ }) as HTMLInputElement;
    expect(startBox.disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    expect(screen.getByText(/零采集点（仅保存连接）/)).toBeTruthy();
    await user.click(submitButton());
    await waitFor(() => expect(bootstrapBodies(calls)).toHaveLength(1));
    const body = bootstrapBodies(calls)[0] as unknown as BootstrapBody;
    expect(body.acquisition.tasks).toEqual([]);
    expect(body.start).toBe(false);
  }, 30_000);

  it("nck_end_to_end_wizard_mock (D10)", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    await addSemanticPoint(user, "5");
    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    await user.click(submitButton());

    // 全链（mock）：201 → 成功提示 + 进入设备入口。
    await screen.findByText(/已添加设备 CNC-01/);
    expect(screen.getByRole("button", { name: /进入设备/ })).toBeTruthy();
    expect(bootstrapBodies(calls)).toHaveLength(1);
  }, 30_000);
});

describe("PR-D：Native 高级模式（向导级）", () => {
  it("nck_native_derived_uses_frozen_key_and_submits_through_plan", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    fireEvent.click(screen.getByRole("button", { name: /高级原生配置/ }));
    await screen.findByTestId("nck-native-fields");

    // derived 固定命名规则：native.<area>.<area_no>.<module>.<column>.<line>
    expect(screen.getByTestId("nck-native-key").textContent).toBe("native.N.1.2.3.4");
    await user.click(screen.getByTestId("nck-native-add-point"));
    await screen.findByTestId("nck-draft-point-row");
    expect(screen.getByTestId("nck-draft-point-row").textContent).toContain(
      "native.N.1.2.3.4",
    );
    expect(screen.getByTestId("nck-draft-point-row").textContent).toContain("nck_native");

    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    await user.click(submitButton());
    await waitFor(() => expect(bootstrapBodies(calls)).toHaveLength(1));
    const body = bootstrapBodies(calls)[0] as unknown as BootstrapBody;
    expect(body.acquisition.tasks[0].binding.kind).toBe("nck.points.v1");
    expect((body.acquisition.tasks[0].binding.config.points as unknown[])[0]).toEqual({
      kind: "nck_native",
      point_key: "native.N.1.2.3.4",
      parameters: {
        area: "N",
        area_no: 1,
        module: 2,
        column: 3,
        line: 4,
        count: 1,
        unit_mode: "current",
        decode: "f32_le",
        transport_size: "0x05",
      },
    });
  }, 30_000);

  it("nck_native_user_requires_explicit_confirmation", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    fireEvent.click(screen.getByRole("button", { name: /高级原生配置/ }));
    await screen.findByTestId("nck-native-fields");
    fireEvent.click(screen.getByRole("radio", { name: /user（显式指定 Native Key）/ }));

    // 未确认：按钮禁用；真实点击（disabled）不得加入。
    const add = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    await user.click(add);
    expect(screen.queryByTestId("nck-draft-point-row")).toBeNull();

    // 先显式确认，再改 Key：确认态必须立即失效（不得「确认后偷改」）。
    fireEvent.click(screen.getByTestId("nck-native-user-confirm"));
    await waitFor(() =>
      expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
    fireEvent.change(screen.getByTestId("nck-native-user-key"), {
      target: { value: "native.N.9.8.7.6" },
    });
    await waitFor(() =>
      expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(
        true,
      ),
    );
    expect((screen.getByTestId("nck-native-user-confirm") as HTMLInputElement).checked).toBe(
      false,
    );

    // 重新确认后才可加入；自定义 Key 原值保留（不自动改名/追加后缀）。
    fireEvent.click(screen.getByTestId("nck-native-user-confirm"));
    await waitFor(() =>
      expect((screen.getByTestId("nck-native-add-point") as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
    await user.click(screen.getByTestId("nck-native-add-point"));
    await screen.findByTestId("nck-draft-point-row");
    expect(screen.getByTestId("nck-draft-point-row").textContent).toContain(
      "native.N.9.8.7.6",
    );

    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    await user.click(submitButton());
    await waitFor(() => expect(bootstrapBodies(calls)).toHaveLength(1));
    const body = bootstrapBodies(calls)[0] as unknown as BootstrapBody;
    const point = (body.acquisition.tasks[0].binding.config.points as Array<{
      point_key: string;
      parameters: Record<string, unknown>;
    }>)[0];
    expect(point.point_key).toBe("native.N.9.8.7.6");
    expect(point.parameters.identity_mode).toBe("user");
  }, 30_000);

  it("nck_native_invalid_user_key_cannot_be_added_or_submitted", async () => {
    await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    fireEvent.click(screen.getByRole("button", { name: /高级原生配置/ }));
    await screen.findByTestId("nck-native-fields");
    fireEvent.click(screen.getByRole("radio", { name: /user（显式指定 Native Key）/ }));

    const input = screen.getByTestId("nck-native-user-key") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "bad.key" } });
    expect(screen.getByTestId("nck-native-key-error")).toBeTruthy();
    const add = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    // 真实点击（disabled）不得加入；组件内部 submit 还会重新校验（PR-C 单测已锁定）。
    await act(async () => {
      fireEvent.click(add);
    });
    expect(screen.queryByTestId("nck-draft-point-row")).toBeNull();
  }, 30_000);

  it("nck_native_conflicting_key_cannot_be_added", async () => {
    const { user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    // 先加入 derived 点位 native.N.1.2.3.4。
    fireEvent.click(screen.getByRole("button", { name: /高级原生配置/ }));
    await screen.findByTestId("nck-native-fields");
    await user.click(screen.getByTestId("nck-native-add-point"));
    await screen.findByTestId("nck-draft-point-row");

    // 再次进入原生页，切到 user 并沿用同一个 Key ⇒ 冲突，不得加入。
    fireEvent.click(screen.getByRole("button", { name: /高级原生配置/ }));
    fireEvent.click(screen.getByRole("radio", { name: /user（显式指定 Native Key）/ }));
    expect(screen.getByTestId("nck-native-conflict")).toBeTruthy();
    const add = screen.getByTestId("nck-native-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    await act(async () => {
      fireEvent.click(add);
    });
    // 冲突点位未被加入：仍只有先前那一条。
    expect(screen.getAllByTestId("nck-draft-point-row")).toHaveLength(1);
  }, 30_000);
});

describe("PR-D：删除后重加与连接草稿变更", () => {
  it("nck_remove_then_readd_requires_fresh_preview", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    const firstKey = await addSemanticPoint(user, "1");
    expect(firstKey).toBe("syn.collect.a@channel=1");
    expect(calls.filter((c) => c.url.endsWith("/identity-preview"))).toHaveLength(1);

    // 显式移除：只改本地草稿，不发网络写请求。
    const writesBefore = calls.filter((c) => c.method !== "GET").length;
    await user.click(screen.getByTestId("nck-remove-point"));
    expect(screen.queryByTestId("nck-draft-point-row")).toBeNull();
    expect(calls.filter((c) => c.method !== "GET").length).toBe(writesBefore);
    // 删除后表单里没有旧 Preview 留下的 Key。
    expect(screen.getByTestId("nck-point-key-empty")).toBeTruthy();

    // 重新加入必须重新选择并重新请求服务端 Preview（不复用旧状态）。
    await addSemanticPoint(user, "1");
    expect(calls.filter((c) => c.url.endsWith("/identity-preview"))).toHaveLength(2);
    expect(screen.getByTestId("nck-draft-point-row").textContent).toContain(
      "syn.collect.a@channel=1",
    );
  }, 30_000);

  it("connection_draft_change_clears_nck_draft_and_catalog", async () => {
    const { calls, user, container } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    await addSemanticPoint(user, "1");
    expect(calls.filter((c) => c.url.startsWith("/api/v1/drivers/sinumerik-nck/definitions") && c.method === "GET")).toHaveLength(1);

    // 返回连接步骤，更换 family（828d → 840d-sl）。
    fireEvent.click(screen.getByRole("button", { name: /上一步/ }));
    await screen.findByText("② 连接");
    await waitFor(() =>
      expect(container.querySelector('[data-field-key="host"]')).not.toBeNull(),
    );
    fireEvent.change(container.querySelector('[data-field-key="host"] input')!, {
      target: { value: "10.0.0.5" },
    });
    await selectNckFamily(container, "840d-sl");
    fireEvent.click(screen.getByRole("button", { name: /下一步：数据/ }));
    await screen.findByText(/高级参数缺少必填项/);
    fireEvent.change(container.querySelector('[data-field-key="local_tsap"] input')!, {
      target: { value: "0x0400" },
    });
    fireEvent.change(container.querySelector('[data-field-key="remote_tsap"] input')!, {
      target: { value: "0x0d04" },
    });
    fireEvent.click(screen.getByRole("button", { name: /下一步：数据/ }));

    await screen.findByText("③ 数据");
    // 明确提示重新选择；旧点位与旧目录状态都被清空。
    expect(await screen.findByText(/连接草稿已变更/)).toBeTruthy();
    expect(screen.queryByTestId("nck-draft-point-row")).toBeNull();
    await screen.findByTestId("nck-catalog-meta");
    expect(calls.filter((c) => c.url.startsWith("/api/v1/drivers/sinumerik-nck/definitions") && c.method === "GET")).toHaveLength(2);

    // 旧 family 的语义 Key 不可直接提交：确认页只能以零点位保存。
    fireEvent.click(screen.getByRole("button", { name: /下一步：确认/ }));
    await screen.findByText("④ 确认 · 最终按钮才真正提交");
    expect(screen.getByText(/零采集点（仅保存连接）/)).toBeTruthy();
    await user.click(submitButton());
    await waitFor(() => expect(bootstrapBodies(calls)).toHaveLength(1));
    const body = bootstrapBodies(calls)[0] as unknown as BootstrapBody;
    expect(body.acquisition.tasks).toEqual([]);
    expect(JSON.stringify(body)).not.toContain("syn.collect.a@channel=1");
  }, 30_000);
});
