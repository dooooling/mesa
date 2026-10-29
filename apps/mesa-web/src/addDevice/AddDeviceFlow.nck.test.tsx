import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { renderAddDeviceFlow as renderFlow } from "../test/addDeviceFlowHarness";
import { selectVariableViaTreeWith } from "../test/catalogTreePick";
import { CANDIDATE_AVAILABILITY } from "../test/availabilityFixture";

beforeEach(() => {
  vi.useRealTimers();
});

afterEach(() => {
  vi.unstubAllGlobals();
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
    availability: CANDIDATE_AVAILABILITY,
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

/**
 * 在向导的语义页选中变量后，**语义点位无法配置**（Endpoint 尚未创建 ⇒ 无法探测实际
 * 对象 ⇒ 缺参数且无法从设备探测获得）。这里断言的是"明确阻断 + 两条出路"，
 * 而不是"再给一套手填参数表单"。
 */
async function expectSemanticAddBlocked(
  user: ReturnType<typeof userEvent.setup>,
  variableId = "syn.collect.a",
) {
  await waitFor(() => expect(screen.getByTestId("catalog-tree")).toBeTruthy());
  await selectVariableViaTreeWith(user, variableId);
  const select = screen.getByTestId("nck-object-select-channel") as HTMLSelectElement;
  expect(select.disabled).toBe(true);
  expect(screen.getByTestId("nck-cascade-gate").textContent).toContain("需先创建并连接端点");
  // 没有手填参数表单（回退即违规）。
  expect(screen.queryByTestId(/^nck-target-/)).toBeNull();
  expect(screen.queryByTestId(/^nck-argument-/)).toBeNull();
  const missing = screen.getByTestId("nck-cascade-missing");
  expect(missing.textContent).toContain("缺少参数 channel");
  expect(missing.textContent).toContain("无法从设备探测获得");
  // 阻断文案必须给出**下一步**：先建端点→采集页配置；或切高级原生配置。
  const guidance = screen.getByTestId("nck-cascade-guidance").textContent ?? "";
  expect(guidance).toContain("先完成设备创建与连接");
  expect(guidance).toContain("「采集」页配置点位");
  expect(guidance).toContain("「高级原生配置」");
  // 不可加入（按钮禁用；程序化点击也不产生草稿点位）。
  const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
  expect(add.disabled).toBe(true);
  fireEvent.click(add);
  expect(screen.queryByTestId("nck-draft-point-row")).toBeNull();
  expect(screen.queryByTestId("nck-point-key")).toBeNull();
}

/** 在向导里通过「高级原生配置」加入一个 derived 原生点位（向导内唯一可用的配置入口）。 */
async function addNativePoint(user: ReturnType<typeof userEvent.setup>) {
  fireEvent.click(screen.getByRole("button", { name: /高级原生配置/ }));
  await screen.findByTestId("nck-native-fields");
  const key = screen.getByTestId("nck-native-key").textContent ?? "";
  await user.click(screen.getByTestId("nck-native-add-point"));
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
    const { user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");

    // 语义模式不要求原生字段：没有「缺少必填参数：area/area_no/...」这类提示。
    expect(screen.queryByText(/缺少必填参数/)).toBeNull();
    expect(screen.queryByTestId("missing-required")).toBeNull();
    // §4.1-4：未选择变量时只保留一个选择入口（没有提交按钮，也就没有可提交路径）。
    expect(screen.queryByTestId("nck-add-point")).toBeNull();

    // 选中变量后：语义点位需要探测实际对象，而向导内 Endpoint 尚未创建 ⇒
    // 明确阻断并给出下一步（不要求填原生字段，也不回退手填参数）。
    await expectSemanticAddBlocked(user);
  }, 30_000);

  it("nck_semantic_picker_is_default_entry_in_wizard (A21)", async () => {
    await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");

    expect(screen.getByTestId("nck-semantic-picker")).toBeTruthy();
    // 变量选择入口 = 搜索框 + 分类树（平铺下拉框已按评审要求移除）。
    expect(screen.getByTestId("nck-variable-search")).toBeTruthy();
    expect(screen.getByTestId("catalog-tree")).toBeTruthy();
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
    await selectVariableViaTreeWith(user, "syn.collect.a");
    await screen.findByTestId("nck-family-reject");
    expect(screen.getByTestId("nck-family-reject").textContent).toContain("FAMILY_MISMATCH");
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
    // family 不匹配时不应发出 identity-preview（请求 family 必须来自连接草稿）。
    expect(calls.some((c) => c.url.endsWith("/identity-preview"))).toBe(false);
  }, 30_000);

  it("wizard_sends_no_preview_because_there_is_no_endpoint_to_probe", async () => {
    // 联动选择必须先探测实际对象；向导里没有 Endpoint ⇒ 不发探测、不发预览，
    // 语义点位保持不可配置（family 权威性由端点采集页用例覆盖）。
    const { calls, user } = await walkNckToDataStep({ family: "828d" });
    await screen.findByTestId("nck-catalog-meta");
    await expectSemanticAddBlocked(user);
    expect(calls.some((c) => c.url.endsWith("/identity-preview"))).toBe(false);
    expect(calls.some((c) => c.url.includes("/browse"))).toBe(false);
  }, 30_000);

  it("family_missing_blocks_add_and_submit", async () => {
    // family 设为非必填以模拟「连接草稿缺少 family」；不得加入、不得提交。
    const { calls, user } = await walkNckToDataStep({ familyRequired: false });
    await screen.findByTestId("nck-catalog-meta");
    expect(screen.getByTestId("nck-family-missing")).toBeTruthy();

    await selectVariableViaTreeWith(user, "syn.collect.a");
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
    // 向导内 Endpoint 尚未创建 ⇒ 语义点位无法探测配置；初始点位改由**高级原生配置**
    // 产生（`NckNativeFields` 原样保留）。这里验证的是提交体形状与单写路径。
    const key = await addNativePoint(user);
    expect(key).toBe("native.N.1.2.3.4");
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
    // 请求体**恰为** `{points}`，且 point_key 逐字等于表单给出的身份（不自动改名）。
    expect(Object.keys(task.binding.config)).toEqual(["points"]);
    expect((task.binding.config.points as Array<{ point_key: string; kind: string }>)[0]).toEqual(
      expect.objectContaining({ kind: "nck_native", point_key: "native.N.1.2.3.4" }),
    );
    // 默认不启动（未勾选）。
    expect(body.start).toBe(false);
    expect(body.idempotency_key).toMatch(/^web-op-/);
    // 全链（mock）：201 → 成功提示 + 进入设备入口。
    await screen.findByText(/已添加设备 CNC-01/);
    expect(screen.getByRole("button", { name: /进入设备/ })).toBeTruthy();
    expect(bootstrapBodies(calls)).toHaveLength(1);
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
    const writes = calls.filter((c) => c.method !== "GET");
    // 唯一写路径：一次 POST /device-bootstrap；无 PUT /tasks、无 POST /endpoints。
    expect(writes.map((c) => `${c.method} ${c.url}`)).toEqual([
      "POST /api/v1/device-bootstrap",
    ]);
    expect(calls.some((c) => c.method === "PUT" && c.url.startsWith("/api/v1/tasks"))).toBe(false);
    expect(calls.some((c) => c.url === "/api/v1/endpoints" && c.method === "POST")).toBe(false);
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
  it("nck_remove_then_readd_requires_fresh_form_state", async () => {
    const { calls, user } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    const firstKey = await addNativePoint(user);
    expect(firstKey).toBe("native.N.1.2.3.4");
    expect(screen.getByTestId("nck-draft-point-row").textContent).toContain("native.N.1.2.3.4");

    // 显式移除：只改本地草稿，不发网络写请求。
    const writesBefore = calls.filter((c) => c.method !== "GET").length;
    await user.click(screen.getByTestId("nck-remove-point"));
    expect(screen.queryByTestId("nck-draft-point-row")).toBeNull();
    expect(calls.filter((c) => c.method !== "GET").length).toBe(writesBefore);
    // 删除后表单里没有旧身份残留（未选变量时连身份行都不渲染）。
    expect(screen.queryByTestId("nck-point-key-empty")).toBeNull();
    expect(screen.queryByTestId("nck-point-key")).toBeNull();

    // 重新加入必须重新走一遍表单（不复用旧状态）：Key 由当前字段重新生成。
    const secondKey = await addNativePoint(user);
    expect(secondKey).toBe("native.N.1.2.3.4");
    expect(screen.getByTestId("nck-draft-point-row").textContent).toContain(
      "native.N.1.2.3.4",
    );
  }, 30_000);

  it("connection_draft_change_clears_nck_draft_and_catalog", async () => {
    const { calls, user, container } = await walkNckToDataStep();
    await screen.findByTestId("nck-catalog-meta");
    await addNativePoint(user);
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
    expect(JSON.stringify(body)).not.toContain("native.N.1.2.3.4");
  }, 30_000);
});
