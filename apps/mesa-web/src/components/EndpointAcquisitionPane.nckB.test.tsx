// 第二批 B + 第 2 步（页面交互）：Endpoint 采集窗格的端到端（组件级）门禁。
//
// 覆盖：目录分页 → scope 过滤 → 抽屉内新增点位 → 保存计划 → PUT → 回读；
// 已有点位编辑不增殖旧 Binding；存量 identity 点位的**显式移除恢复路径**
// （删除不经准入门）；预览失败/身份不一致时禁用保存。
//
// 第 2 步只改「入口位置与查询方式」，断言强度不放宽：
// - 变量选择/目录统计在**抽屉**里（默认页面不常驻表单）；
// - 保存区在无修改时**不渲染**（`apply-button` 取不到是预期）；
// - 逐行绿色「校验通过」标签删除，改用 `data-verify-status` 锚点；
// - 移除走行内「⋯ → 移除」。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { UNSUPPORTED_CONFIG_NOTICE } from "../descriptorCompatibility";
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

/** 驱动声明的选型入口（NCK 真实声明 manual + browse；import 后端 501，不得开放）。 */
const SELECTION_METHODS = ["manual", "browse"];

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
      return ok({
        resources: state.resources ?? RESOURCES,
        resource_selection_methods: SELECTION_METHODS,
      });
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

/** 保存区主按钮（无修改时保存区不渲染，取不到是**预期**）。 */
function saveButton(): HTMLButtonElement {
  return screen.getByTestId("apply-button") as HTMLButtonElement;
}

/** 等身份核验落定（绿色标签已删除，改看行上的 data-verify-status）。 */
async function waitVerified(count = 1) {
  await waitFor(() =>
    expect(document.querySelectorAll('[data-verify-status="verified"]').length).toBe(count),
  );
}

/** 打开添加抽屉并等待里面的选择入口就绪。 */
async function openAddDrawer(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByTestId("add-point-entry"));
  await screen.findByTestId("nck-variable-select");
}

/** 制造一次真实变更，让固定保存区出现。 */
async function changeInterval(value: number) {
  await waitFor(() =>
    expect(screen.getByTestId("acquisition-status-interval").textContent).toContain("（已保存）"),
  );
  fireEvent.change(screen.getByTestId("acquisition-interval-input"), {
    target: { value: String(value) },
  });
}

/**
 * 按点位键找到对应行：界面默认只展示「名称 + 摘要」，完整键在技术详情里，
 * 因此这里用 variable_id 段定位（同一 Endpoint 内唯一）。
 */
function rowOf(pointKey: string): HTMLElement {
  const needle = pointKey.split("@")[0]!;
  const row = screen
    .getAllByTestId("nck-point-row")
    .find((el) => (el.textContent ?? "").includes(needle));
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
    const user = userEvent.setup();
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

    // 目录统计不在默认页面上（§2.5），打开抽屉后才是选择器的信息区。
    await waitFor(() => expect(screen.queryByTestId("nck-collectable-summary")).toBeNull());
    await openAddDrawer(user);
    const summary = await screen.findByTestId("nck-collectable-summary");
    expect(summary.textContent).toContain("2 个可配置变量");
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

describe("门禁 7：抽屉内新增点位 → nck.points.v1 → PUT 后回读一致", () => {
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
    // 默认页面不常驻添加表单（§2.1）。
    await waitFor(() => expect(screen.queryByTestId("add-point-entry")).toBeTruthy());
    expect(screen.queryByTestId("nck-variable-select")).toBeNull();
    // 无修改时保存区不渲染。
    expect(screen.queryByTestId("apply-button")).toBeNull();

    await openAddDrawer(user);
    // 抽屉里只有一句说明（§4.3）。
    expect(screen.getByTestId("drawer-draft-notice").textContent).toBe(
      "确认后加入页面草稿，保存配置后生效。",
    );
    // 未选资源时只显示一个选择入口：没有空身份、禁用按钮与错误框的组合。
    expect(screen.queryByTestId("nck-add-point")).toBeNull();
    const select = screen.getByTestId("nck-variable-select") as HTMLSelectElement;
    await user.selectOptions(select, "syn.collect.a");
    await user.type(screen.getByLabelText("target-channel"), "1");
    await screen.findByTestId("nck-point-key");
    // 按钮文案是「加入待保存列表」，不是「保存点位」（§4.1-7）。
    expect(screen.getByTestId("nck-add-point").textContent).toBe("加入待保存列表");
    await user.click(screen.getByTestId("nck-add-point"));
    await screen.findByTestId("nck-point-row");
    // 抽屉关闭（只更新草稿，不发任何写请求）。
    await waitFor(() => expect(screen.queryByTestId("nck-variable-select")).toBeNull());
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    // 回读一致后才报成功；成功提示只由保存区结果条展示一次（§4.6-4）。
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存");
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
    // 快照就绪（点位行已渲染）；无实际修改时保存区不渲染（§4.7）。
    await waitVerified(1);
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(rowOf(p1.point_key)).toBeTruthy();

    // 在同一个 NCK 任务里新增第二个点位（抽屉入口）。
    await openAddDrawer(user);
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
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存");

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
    await waitVerified(1);
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();

    // 点行「编辑」→ 抽屉立即打开并带入配置；确认按钮是「确认修改」。
    await user.click(within(rowOf(original.point_key)).getByTestId("point-row-edit"));
    await screen.findByTestId("nck-variable-select");
    expect(screen.getByTestId("nck-add-point").textContent).toBe("确认修改");
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
    // 「确认修改」只更新草稿：没有任何写请求，行标记「已修改」。
    expect(state.putBodies).toHaveLength(0);
    await waitFor(() =>
      expect(within(rowOf(original.point_key)).getByTestId("point-row-status").textContent).toContain(
        "已修改",
      ),
    );
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存");

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

    // 1) 必须展示「不能保存的具体原因」（真实问题照常出现，§3.3），且不可保存。
    const alert = await screen.findByTestId("nck-legacy-identity-alert");
    expect(alert.textContent).toContain("身份探针点位");
    expect(alert.textContent).toContain("请检查并手动移除这些点位");
    expect(alert.textContent).toContain("SCOPE_IDENTITY");
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(state.putBodies).toHaveLength(0);

    // 2) 不得自动删除：两个点位都还在。
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(2);
    expect(rowOf(identityPoint.point_key)).toBeTruthy();

    // 3) 显式移除：入口必须可用（删除本身不经过 scope 准入门）。
    const identityRow = rowOf(identityPoint.point_key);
    const previewsBeforeRemoval = state.previewCalls.length;
    await user.click(within(identityRow).getByTestId("point-row-more"));
    const removeItem = await screen.findByTestId("point-row-remove");
    expect((removeItem as HTMLButtonElement).disabled).toBe(false);
    await user.click(removeItem);

    // 4) 原始点位信息保留（技术详情里），便于确认改了什么；并可撤销。
    const removed = await screen.findByTestId("nck-removed-points");
    expect(removed.textContent).toContain(identityPoint.point_key);
    expect(removed.textContent).toContain('"channel":9');
    expect(removed.textContent).toContain("SCOPE_IDENTITY");
    expect(screen.getByTestId("undo-remove")).toBeTruthy();

    // 5) 移除后重新验证剩余点位（至少再次调用预览），然后才允许保存。
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1);
    await waitFor(() => expect(state.previewCalls.length).toBeGreaterThan(previewsBeforeRemoval));
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    // 6) 保存只包含剩余合法点位；identity 没有被静默改成 collect。
    await user.click(saveButton());
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存");
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
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
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
    await openAddDrawer(user);
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
    // 切换 Endpoint 必须关闭抽屉并重建表单：上一台的目标输入与已确认 Key 不得残留。
    await waitFor(() => expect(screen.queryByTestId("nck-variable-select")).toBeNull());
    await openAddDrawer(user);
    expect((screen.getByTestId("nck-variable-select") as HTMLSelectElement).value).toBe("");
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
    // 原生点位同样只靠行级锚点声明「配置格式校验通过」，界面上不再有绿色标签。
    await waitVerified(1);
    expect(document.body.textContent).not.toContain("校验通过");
    // 无实际修改 ⇒ 保存区不渲染；改一次周期后按钮仍可用（证明准入没问题）。
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "1000" } });
    await waitFor(() => expect(screen.queryByTestId("draft-save-bar")).toBeNull());
    expect(state.putBodies).toHaveLength(0);

    // native 点位也必须可进入编辑（高级原生页），而不是只能移除。
    await user.click(screen.getByTestId("point-row-edit"));
    expect(screen.getByTestId("nck-native-fields")).toBeTruthy();
    expect(screen.getByTestId("nck-native-key").textContent).toBe("native.C.1.21.1.396");
  });
});

// P1：Endpoint 层最终保存守卫（复用本文件的目录/预览夹具，不连真机、不新增 mock 套件）。
//
// 场景：驱动升级后 `variable` 资源新增一个界面不认识的**必填**字段，存量原生点位
// 尚未携带该键。旧守卫只查「值里有没有这个 key」⇒ 返回空 ⇒ 放行保存；新守卫必须
// 按**完整 Schema** 拒绝，并且拒绝要发生在任何 stop / PUT / start 之前。
describe("P1：Endpoint 层最终保存守卫（完整 Schema）", () => {
  it("unknown_required_native_field_blocks_save_before_any_request", async () => {
    const nativeSchema = [
      { key: "area", label: "Area", field_type: "enum", required: true, validation: { enum_options: ["C"] }, ui: {} },
      { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: { min: 0, max: 31 }, ui: {} },
      { key: "module", label: "Module", field_type: "integer", required: true, validation: { min: 0, max: 255 }, ui: {} },
      { key: "column", label: "Column", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
      { key: "line", label: "Line", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
      // 驱动新增：界面不认识的类型 + required，旧配置里没有这个键。
      { key: "axis_ref", label: "Axis ref", field_type: "axis_ref", required: true, validation: {}, ui: {} },
    ];
    const nativePoint: NckPointSelection = {
      kind: "nck_native",
      point_key: "native.C.1.21.1.396",
      parameters: { area: "C", area_no: 1, module: 21, column: 1, line: 396 },
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

    // 完整 Schema 守卫在界面上可见：未携带的未知必填字段也被点名。
    expect(await screen.findByTestId("acquisition-unsupported-fields")).toBeTruthy();
    expect(
      screen.getByText("variable · axis_ref · axis_ref · 配置未携带（驱动新增字段）"),
    ).toBeTruthy();

    // 制造一次真实变更（周期 1000 → 2000）：若守卫缺失，保存会真的发请求。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    const save = await screen.findByTestId("apply-button");
    expect((save as HTMLButtonElement).disabled).toBe(true);

    const fetchMock = globalThis.fetch as unknown as {
      mock: { calls: Array<[string, RequestInit | undefined]> };
    };
    const before = fetchMock.mock.calls.length;
    // 绕过 disabled 直接触发组件级 onClick（disabled 不是唯一防线）。
    expect(invokeDisabledButton(save as HTMLButtonElement)).toBe(true);
    await act(async () => {});

    // 拒绝发生在任何 stop / PUT / start 之前：连一个请求都没有发出。
    expect(fetchMock.mock.calls.length).toBe(before);
    expect(state.putBodies).toHaveLength(0);
    const urls = fetchMock.mock.calls.map(([url]) => String(url));
    expect(urls.some((url) => /\/stop$|\/start$/.test(url))).toBe(false);
    // 拒绝原因来自 Schema 守卫（不是「身份核验未通过」那条分支）。
    await waitFor(() =>
      expect(document.querySelector(".ant-message")?.textContent ?? "").toContain(
        UNSUPPORTED_CONFIG_NOTICE,
      ),
    );
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
    // 浏览入口在抽屉里，且必须由驱动声明（resource_selection_methods 含 browse）。
    await openAddDrawer(user);
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
