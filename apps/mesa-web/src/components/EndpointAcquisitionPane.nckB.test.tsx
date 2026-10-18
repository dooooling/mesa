// PR-C 第二批 B：Endpoint 采集窗格的端到端（组件级）门禁。
//
// 覆盖：目录分页 → scope 过滤 → 新增点位 → 保存计划 → PUT → 回读；
// 已有点位编辑不增殖旧 Binding；存量 identity 点位的**显式移除恢复路径**
// （删除不经准入门）；预览失败/身份不一致时禁用保存。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import {
  CANONICAL_RESOURCES_KIND,
  NCK_POINTS_KIND,
  type AcquisitionTaskShape,
  type NckPointSelection,
  type ResourceSelection,
} from "../deviceModel";
import type { NckCatalogDefinition, NckDefinitionsResponse } from "../nckCatalog";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-nck";

const RESOURCES = [
  {
    id: "variable",
    label: { default: "Variable" },
    parameters: { fields: [] },
    outputs: [],
    modes: ["poll"],
  },
];

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function catDefinition(
  variableId: string,
  overrides: Partial<NckCatalogDefinition> = {},
): NckCatalogDefinition {
  const revision = overrides.revision ?? 1;
  return {
    variable_id: variableId,
    revision,
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
    golden_ref: [{ entry: "nc_variable" }],
    digest: `digest-${variableId}-${revision}`,
    ...overrides,
  };
}

function catEnvelope(overrides: Partial<NckDefinitionsResponse> = {}): NckDefinitionsResponse {
  return {
    format_version: 1,
    family: "828d",
    revision: 1,
    driver_id: "sinumerik-nck",
    driver_version: "0.1.0",
    note: null,
    total: 0,
    limit: 100,
    next_cursor: null,
    definitions: [],
    ...overrides,
  };
}

interface MockState {
  tasks: AcquisitionTaskShape[];
  savedTasks: AcquisitionTaskShape[] | null;
  readBack: AcquisitionTaskShape[] | null;
  putBodies: AcquisitionTaskShape[][];
  previewFails: boolean;
  previewCalls: string[];
  /** 目录页：按 cursor 索引（"" = 首页）。为空表示不处理目录请求。 */
  catalogPages: Record<string, NckDefinitionsResponse>;
  catalogCalls: string[];
  /** Browse 根节点（为空表示不处理 browse 请求）。 */
  browseNodes: unknown[];
  /** 覆盖 Descriptor 的 resources（native Schema 测试用）。 */
  resources?: unknown[];
}

function mkState(overrides: Partial<MockState> = {}): MockState {
  return {
    tasks: [],
    savedTasks: null,
    readBack: null,
    putBodies: [],
    previewFails: false,
    previewCalls: [],
    catalogPages: {},
    catalogCalls: [],
    browseNodes: [],
    ...overrides,
  };
}

function previewKey(variableId: string, target: Record<string, unknown>): string {
  if (typeof target.number === "number" && typeof target.entry === "string") {
    return `${variableId}@drive_object=${String(target.number)},entry=${target.entry}`;
  }
  return `${variableId}@${Object.entries(target)
    .map(([k, v]) => `${k}=${String(v)}`)
    .join(",")}`;
}

function installFetch(state: MockState) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [
          {
            id: EP,
            driver_id: NCK_DRIVER,
            connection: { family: "828d" },
            runtime: { state: "STOPPED" },
          },
          {
            id: "ep-2",
            driver_id: NCK_DRIVER,
            connection: { family: "828d" },
            runtime: { state: "STOPPED" },
          },
          {
            id: "ep-s7",
            driver_id: "s7",
            connection: { family: "" },
            runtime: { state: "STOPPED" },
          },
        ],
      });
    }
    if (url === `/api/v1/endpoints/${EP}/browse` || url === "/api/v1/endpoints/ep-2/browse") {
      return ok({ nodes: state.browseNodes, next_cursor: null });
    }
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/definitions/identity-preview")) {
      const body = JSON.parse(String(init?.body)) as {
        parameters: Record<string, unknown>;
      };
      const variableId = String(body.parameters.variable_id ?? "");
      state.previewCalls.push(variableId);
      if (state.previewFails) {
        return {
          ok: false,
          status: 503,
          json: async () => ({
            error: {
              code: "DRIVER_UNAVAILABLE",
              kind: "ConnectionError",
              message: "驱动不可达（测试桩）",
            },
          }),
        };
      }
      return ok({
        point_key: previewKey(variableId, (body.parameters.target ?? {}) as Record<string, unknown>),
        identity_mode: "derived",
        scope: "collect",
        variable_id: variableId,
        definition_revision: body.parameters.definition_revision,
        definition_digest: "digest-test",
      });
    }
    if (url.startsWith("/api/v1/drivers/") && url.includes("/definitions")) {
      state.catalogCalls.push(url);
      const parsed = new URL(url, "http://localhost");
      const cursor = parsed.searchParams.get("cursor") ?? "";
      const page = state.catalogPages[cursor];
      if (!page) return { ok: false, status: 404, json: async () => ({}) };
      return ok(page);
    }
    if (url.startsWith("/api/v1/drivers/")) {
      return ok({ resources: state.resources ?? RESOURCES });
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      return ok({ tasks: state.savedTasks ?? state.tasks });
    }
    if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
      const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
      state.putBodies.push(body.tasks);
      state.savedTasks = state.readBack ?? body.tasks;
      return ok({ ok: true });
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function point(variableId: string, target: Record<string, number>): NckPointSelection {
  return {
    kind: "nck_variable",
    point_key: previewKey(variableId, target),
    parameters: { variable_id: variableId, definition_revision: 1, target, arguments: {} },
  };
}

function nckTask(id: string, points: NckPointSelection[]): AcquisitionTaskShape {
  return {
    id,
    schedule: { mode: "poll", interval_ms: 1000 },
    binding: { kind: NCK_POINTS_KIND, config: { points } },
  };
}

function saveButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: /保存并启动/ }) as HTMLButtonElement;
}

function rowOf(pointKey: string): HTMLElement {
  const row = screen
    .getAllByTestId("nck-point-row")
    .find((el) => (el.textContent ?? "").includes(pointKey));
  if (!row) throw new Error(`找不到点位行：${pointKey}`);
  return row;
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

describe("门禁 1/3：目录分页完整读取，仅 collect 进入采集界面", () => {
  it("catalog_pages_are_all_loaded_and_identity_never_offered", async () => {
    const state = mkState({
      catalogPages: {
        "": catEnvelope({
          total: 3,
          limit: 2,
          next_cursor: "v1-syn.collect.b-1",
          definitions: [catDefinition("syn.collect.a"), catDefinition("syn.collect.b")],
        }),
        "v1-syn.collect.b-1": catEnvelope({
          total: 3,
          limit: 2,
          next_cursor: null,
          definitions: [catDefinition("syn.identity.a", { scope: "identity" })],
        }),
      },
    });
    installFetch(state);
    render(<EndpointAcquisitionPane endpointId={EP} driverId={NCK_DRIVER} onChanged={() => {}} />);

    const summary = await screen.findByTestId("nck-collectable-summary");
    expect(summary.textContent).toContain("2 / 3");
    expect(screen.getByTestId("nck-catalog-meta").textContent).toContain("3 条 / 2 页");
    // 两页都读了：cursor 逐字回传。
    expect(state.catalogCalls).toHaveLength(2);
    expect(state.catalogCalls[1]).toContain("cursor=v1-syn.collect.b-1");
    const options = Array.from(
      (screen.getByTestId("nck-variable-select") as HTMLSelectElement).querySelectorAll("option"),
    ).map((o) => o.value);
    expect(options).toContain("syn.collect.a");
    expect(options).toContain("syn.collect.b");
    // identity 条目不得出现在可采集选择列表（不是禁用，而是不提供）。
    expect(options).not.toContain("syn.identity.a");
  });
});

describe("门禁 7：新增点位 → nck.points.v1 → PUT 后回读一致", () => {
  it("new_point_is_saved_and_read_back", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={{
          "syn.collect.a": {
            variable_id: "syn.collect.a",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
        }}
      />,
    );
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    const select = screen.getByTestId("nck-variable-select") as HTMLSelectElement;
    await user.selectOptions(select, "syn.collect.a");
    await user.type(screen.getByLabelText("target-channel"), "1");
    await screen.findByTestId("nck-point-key");
    await user.click(screen.getByTestId("nck-add-point"));
    await screen.findByTestId("nck-point-row");
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    await screen.findByText(/点位已保存/);
    expect(state.putBodies).toHaveLength(1);
    const sent = state.putBodies[0];
    expect(sent).toHaveLength(1);
    expect(sent[0].binding.kind).toBe(NCK_POINTS_KIND);
    const points = (sent[0].binding.config as { points: NckPointSelection[] }).points;
    expect(points).toHaveLength(1);
    expect(points[0]).toEqual({
      kind: "nck_variable",
      point_key: "syn.collect.a@channel=1",
      parameters: {
        variable_id: "syn.collect.a",
        definition_revision: 1,
        target: { channel: 1 },
        arguments: {},
      },
    });
    // 回读成功 → 才报成功（失败路径见门禁 6 与第二批 A 的 stale readback 用例）。
    expect(state.savedTasks).not.toBeNull();
  });
});

describe("门禁 8：已有点位编辑保留合法任务，不增殖旧 Binding", () => {
  it("editing_existing_task_keeps_other_tasks_and_never_adds_generic_binding", async () => {
    const user = userEvent.setup();
    const p1 = point("syn.collect.a", { channel: 1 });
    const selections: ResourceSelection[] = [
      {
        resource_id: "variable",
        parameters: {},
        outputs: [{ output: "value", point_key: "legacy.k1" }],
      },
    ];
    const otherTask: AcquisitionTaskShape = {
      id: "task-b",
      schedule: { mode: "subscribe" },
      binding: { kind: CANONICAL_RESOURCES_KIND, config: { selections } },
    };
    const state = mkState({ tasks: [nckTask("nck-task", [p1]), otherTask] });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={{
          "syn.collect.a": {
            variable_id: "syn.collect.a",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
          "syn.collect.b": {
            variable_id: "syn.collect.b",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
        }}
      />,
    );
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    expect(rowOf(p1.point_key)).toBeTruthy();

    // 在同一个 NCK 任务里新增第二个点位。
    await user.selectOptions(
      screen.getByTestId("nck-variable-select") as HTMLSelectElement,
      "syn.collect.b",
    );
    await user.type(screen.getByLabelText("target-channel"), "2");
    await screen.findByTestId("nck-point-key");
    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(2));
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
    await screen.findByText(/点位已保存/);

    expect(state.putBodies).toHaveLength(1);
    const sent = state.putBodies[0];
    // 任务数不增加（2 个），NCK 任务 ID 不变，另一个任务逐字保留。
    expect(sent).toHaveLength(2);
    const nckSent = sent.find((t) => t.id === "nck-task");
    const otherSent = sent.find((t) => t.id === "task-b");
    expect(nckSent?.binding.kind).toBe(NCK_POINTS_KIND);
    expect((nckSent?.binding.config as { points: NckPointSelection[] }).points).toHaveLength(2);
    expect(otherSent).toEqual(otherTask);
    // 不得因为编辑 NCK 任务而追加一个 mesa.resources.v1 任务。
    expect(sent.filter((t) => t.binding.kind === CANONICAL_RESOURCES_KIND)).toHaveLength(1);
  });

  it("editing_a_point_replaces_identity_after_server_preview_and_keeps_task_id", async () => {
    const user = userEvent.setup();
    const original = point("syn.collect.a", { channel: 1 });
    const state = mkState({ tasks: [nckTask("nck-task", [original])] });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={{
          "syn.collect.a": {
            variable_id: "syn.collect.a",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
        }}
      />,
    );
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    await user.click(within(rowOf(original.point_key)).getByRole("button", { name: /编\s*辑/ }));
    const input = screen.getByLabelText("target-channel") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "3");
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe("syn.collect.a@channel=3"),
    );
    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() =>
      expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1),
    );
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
    await screen.findByText(/点位已保存/);

    const sent = state.putBodies[0];
    expect(sent).toHaveLength(1);
    expect(sent[0].id).toBe("nck-task");
    const points = (sent[0].binding.config as { points: NckPointSelection[] }).points;
    expect(points).toHaveLength(1);
    expect(points[0].point_key).toBe("syn.collect.a@channel=3");
  });
});

describe("门禁 9：存量 identity 点位 → 显式移除 → 重验剩余 → 保存并回读", () => {
  it("identity_point_can_be_removed_without_passing_scope_gate_then_save_recovers", async () => {
    const user = userEvent.setup();
    const identityPoint = point("syn.identity.a", { channel: 9 });
    // identity 点位的参数形状按定义（axis），这里故意用 channel 也不影响：
    // 准入失败发生在参数校验之前，证明「删除不经准入门」不是靠参数碰巧合法。
    const collectPoint = point("syn.collect.a", { channel: 1 });
    const state = mkState({ tasks: [nckTask("nck-task", [identityPoint, collectPoint])] });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={{
          "syn.identity.a": {
            variable_id: "syn.identity.a",
            revision: 1,
            family: "828d",
            scope: "identity",
            targets: ["axis"],
            target_required: ["axis"],
            arguments: [],
          },
          "syn.collect.a": {
            variable_id: "syn.collect.a",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
        }}
      />,
    );

    // 1) 必须展示「不能保存的具体原因」，且不可保存。
    const alert = await screen.findByTestId("nck-legacy-identity-alert");
    expect(alert.textContent).toContain("身份探针点位");
    expect(alert.textContent).toContain("请检查并手动移除这些点位");
    expect(alert.textContent).toContain("SCOPE_IDENTITY");
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    expect(state.putBodies).toHaveLength(0);

    // 2) 不得自动删除：两个点位都还在。
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(2);
    expect(rowOf(identityPoint.point_key)).toBeTruthy();

    // 3) 显式移除：按钮必须可用（删除本身不经过 scope 准入门）。
    const identityRow = rowOf(identityPoint.point_key);
    const removeButton = within(identityRow).getByRole("button", { name: /移\s*除/ }) as HTMLButtonElement;
    expect(removeButton.disabled).toBe(false);
    const previewsBeforeRemoval = state.previewCalls.length;
    await user.click(removeButton);

    // 4) 原始点位信息保留，便于确认改了什么。
    const removed = await screen.findByTestId("nck-removed-points");
    expect(removed.textContent).toContain(identityPoint.point_key);
    expect(removed.textContent).toContain('"channel":9');
    expect(removed.textContent).toContain("SCOPE_IDENTITY");

    // 5) 移除后重新验证剩余点位（至少再次调用预览），然后才允许保存。
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1);
    await waitFor(() => expect(state.previewCalls.length).toBeGreaterThan(previewsBeforeRemoval));
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    // 6) 保存只包含剩余合法点位；identity 没有被静默改成 collect。
    await user.click(saveButton());
    await screen.findByText(/点位已保存/);
    expect(state.putBodies).toHaveLength(1);
    const sentPoints = (state.putBodies[0][0].binding.config as { points: NckPointSelection[] })
      .points;
    expect(sentPoints).toEqual([collectPoint]);
    expect(JSON.stringify(sentPoints)).not.toContain("syn.identity.a");
    // 回读确认（保存成功路径要求 readback 一致；桩默认回填刚保存的任务）。
    expect(state.savedTasks).not.toBeNull();
  });
});

describe("门禁 6：已有点位预览失败 ⇒ 不得按旧 Key 继续提交", () => {
  it("existing_point_with_failed_preview_blocks_save_with_reason", async () => {
    const p = point("syn.collect.a", { channel: 1 });
    const state = mkState({ tasks: [nckTask("nck-task", [p])], previewFails: true });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={{
          "syn.collect.a": {
            variable_id: "syn.collect.a",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
        }}
      />,
    );
    const alert = await screen.findByTestId("nck-legacy-identity-alert");
    expect(alert.textContent).toContain("DRIVER_UNAVAILABLE");
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    expect(state.putBodies).toHaveLength(0);
  });
});

describe("收口回归：切换 Endpoint 不得残留上一台的表单/Preview", () => {
  it("endpoint_switch_remounts_semantic_form_and_drops_previous_preview", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    const definitions = {
      "syn.collect.a": {
        variable_id: "syn.collect.a",
        revision: 1,
        family: "828d",
        scope: "collect",
        targets: ["channel"],
        target_required: ["channel"],
        arguments: [],
      },
    };
    const { rerender } = render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={definitions}
      />,
    );
    await screen.findByTestId("nck-collectable-summary");
    await user.selectOptions(
      screen.getByTestId("nck-variable-select") as HTMLSelectElement,
      "syn.collect.a",
    );
    await user.type(screen.getByLabelText("target-channel"), "1");
    await screen.findByTestId("nck-point-key");

    rerender(
      <EndpointAcquisitionPane
        endpointId="ep-2"
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={definitions}
      />,
    );
    // 表单以 endpointId 作为 key：切换 Endpoint 必须卸载重建，
    // 上一台的目标输入与已确认 Key 不得跨设备残留。
    await waitFor(() =>
      expect((screen.getByTestId("nck-variable-select") as HTMLSelectElement).value).toBe(""),
    );
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    // 未重新选择变量 ⇒ 不渲染任何 Target 输入（上一台的输入已随卸载丢弃）。
    expect(screen.queryByLabelText("target-channel")).toBeNull();
  });
});

describe("PR-C 第三批：Endpoint 页 native 存量点位复核（A24–A27）", () => {
  it("existing_derived_native_point_is_verified_and_noop_save_sends_no_put", async () => {
    const user = userEvent.setup();
    const nativeSchema = [
      { key: "area", label: "Area", field_type: "enum", required: true, validation: { enum_options: ["C"] }, ui: {} },
      { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: { min: 0, max: 31 }, ui: {} },
      { key: "module", label: "Module", field_type: "integer", required: true, validation: { min: 0, max: 255 }, ui: {} },
      { key: "column", label: "Column", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
      { key: "line", label: "Line", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
      { key: "count", label: "Count", field_type: "integer", required: true, default: 1, validation: { min: 1, max: 255 }, ui: {} },
      { key: "unit_mode", label: "Unit mode", field_type: "enum", required: true, default: "current", validation: { enum_options: ["current"] }, ui: {} },
      { key: "decode", label: "Decode", field_type: "enum", required: true, validation: { enum_options: ["U16_LE"] }, ui: {} },
      { key: "transport_size", label: "Transport", field_type: "enum", required: true, validation: { enum_options: ["0x05"] }, ui: {} },
    ];
    const nativePoint: NckPointSelection = {
      kind: "nck_native",
      point_key: "native.C.1.21.1.396",
      parameters: {
        area: "C",
        area_no: 1,
        module: 21,
        column: 1,
        line: 396,
        count: 1,
        unit_mode: "current",
        decode: "U16_LE",
        transport_size: "0x05",
      },
    };
    const state = mkState({
      tasks: [nckTask("nck-native-task", [nativePoint])],
      resources: [
        {
          id: "variable",
          label: { default: "NC Variable" },
          parameters: { fields: nativeSchema },
          outputs: [],
          modes: ["poll"],
        },
      ],
    });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={{
          "syn.collect.a": {
            variable_id: "syn.collect.a",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
        }}
      />,
    );
    await screen.findByText(/高级原生（不适用目录 scope）/);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
    await screen.findByText(/未检测到实际修改/);
    expect(state.putBodies).toHaveLength(0);

    // native 点位也必须可进入编辑（高级原生页），而不是只能移除。
    await user.click(
      within(screen.getByTestId("nck-point-row")).getByRole("button", { name: /编\s*辑/ }),
    );
    expect(screen.getByTestId("nck-native-fields")).toBeTruthy();
    expect(screen.getByTestId("nck-native-key").textContent).toBe("native.C.1.21.1.396");
  });
});

describe("PR-C 第三批：Endpoint 页 Browse → 语义表单（A1/A13/A16）", () => {
  it("semantic_browse_prefills_form_and_identity_leaf_is_blocked", async () => {
    const user = userEvent.setup();
    const collectNode = {
      id: "browse.collect",
      label: "Browse 程序状态",
      kind: "nck_variable",
      has_children: false,
      binding_json: JSON.stringify({
        kind: "nck_variable",
        parameters: {
          variable_id: "syn.collect.a",
          definition_revision: 1,
          target: { channel: 1 },
          arguments: {},
        },
        identity: {
          template: "syn.collect.a@channel={channel}",
          keys: [{ path: "channel", required: true }],
          preview_point_key: "syn.collect.a@channel=1",
        },
      }),
    };
    const identityNode = {
      id: "browse.identity",
      label: "Browse 身份探针",
      kind: "nck_variable",
      has_children: false,
      binding_json: JSON.stringify({
        kind: "nck_variable",
        parameters: {
          variable_id: "syn.identity.a",
          definition_revision: 1,
          target: { axis: 1 },
          arguments: {},
        },
        identity: {
          template: "syn.identity.a@axis={axis}",
          keys: [{ path: "axis", required: true }],
          preview_point_key: "syn.identity.a@axis=1",
        },
      }),
    };
    const state = mkState({ browseNodes: [collectNode, identityNode] });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={{
          "syn.collect.a": {
            variable_id: "syn.collect.a",
            revision: 1,
            family: "828d",
            scope: "collect",
            targets: ["channel"],
            target_required: ["channel"],
            arguments: [],
          },
          "syn.identity.a": {
            variable_id: "syn.identity.a",
            revision: 1,
            family: "828d",
            scope: "identity",
            targets: ["axis"],
            target_required: ["axis"],
            arguments: [],
          },
        }}
      />,
    );
    await screen.findByTestId("nck-collectable-summary");
    await user.click(screen.getByTestId("nck-browse-toggle"));
    await screen.findByText("Browse 程序状态");

    // A13：identity 叶必须有具体不可选原因，且没有“选用（语义）”。
    const identityRow = screen.getByText("Browse 身份探针").closest("div") as HTMLElement;
    expect(within(identityRow).getByTestId("browse-semantic-block").textContent).toContain(
      "SCOPE_IDENTITY",
    );
    expect(within(identityRow).queryByRole("button", { name: /选用（语义）/ })).toBeNull();

    // A1/A16：collect 叶可进入语义表单；Browse 预览只展示，提交前重新 Preview。
    const collectRow = screen.getByText("Browse 程序状态").closest("div") as HTMLElement;
    await user.click(within(collectRow).getByRole("button", { name: /选用（语义）/ }));
    await waitFor(() =>
      expect((screen.getByTestId("nck-variable-select") as HTMLSelectElement).value).toBe(
        "syn.collect.a",
      ),
    );
    expect((screen.getByLabelText("target-channel") as HTMLInputElement).value).toBe("1");
    await screen.findByTestId("nck-point-key");
    expect(screen.getByTestId("nck-point-key").textContent).toBe("syn.collect.a@channel=1");
    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
  });
});
