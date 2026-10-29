// 第 2 步（页面交互）之后的采集窗格回归：把原有守卫断言迁到新界面上。
//
// 只改「查询方式」与「入口位置」，不放宽任何断言：
// - 目录统计与变量选择收进**抽屉**（默认页面不再有标签墙/统计）；
// - 保存区在无修改时**不渲染**，因此「无实际修改 ⇒ 不发 PUT」改为断言
//   「保存区不存在 + 无写请求」，仍由 `endpointSaveTaskSave` 的 NO_CHANGES 守卫兜底；
// - 逐行绿色「校验通过」标签删除，改用行上的 `data-verify-status` 锚点；
// - 移除走行内「⋯ → 移除」（先改草稿，可撤销）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import {
  EndpointAcquisitionPane,
  type NckDefinitionEntry,
  type NckDefinitionMap,
} from "./EndpointAcquisitionPane";
import {
  CANONICAL_RESOURCES_KIND,
  NCK_POINTS_KIND,
  mergeAcquisitionTasks,
  type AcquisitionTaskShape,
  type NckPointSelection,
  type ResourceSelection,
} from "../deviceModel";
import { catalogLeafIds } from "../test/catalogTreePick";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-nck";

// Descriptor 只需形态合法（NCK 路径不消费 resources；此处仅为复用既有桩形状）。
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

interface MockState {
  tasks: AcquisitionTaskShape[];
  putBodies: AcquisitionTaskShape[][];
  putOk: boolean;
  putErrorBody: unknown;
  tasksGetFails: boolean;
  /** 回读时返回的任务（null ⇒ 返回刚保存的任务，模拟正常持久化）。 */
  readBack: AcquisitionTaskShape[] | null;
  savedTasks: AcquisitionTaskShape[] | null;
  /** 预览是否失败（用于证明「失败不回落旧 Key」）。 */
  previewFails?: boolean;
  /** 已发生的预览请求（variable_id 序列，便于断言重新核验确实发生）。 */
  previewCalls?: string[];
}

/** 保存链路的 fetch 桩：区分初次快照 GET、PUT、目录预览与保存后的回读 GET。 */
function installFetch(state: MockState) {
  state.previewCalls ??= [];
  (globalThis as { fetch?: unknown }).fetch = vi.fn(
    async (url: string, init?: RequestInit) => {
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
              id: "ep-s7",
              driver_id: "s7",
              connection: { family: "" },
              runtime: { state: "STOPPED" },
            },
          ],
        });
      }
      if (url.startsWith("/api/v1/drivers/") && url.endsWith("/definitions/identity-preview")) {
        const body = JSON.parse(String(init?.body)) as {
          kind: string;
          parameters: Record<string, unknown>;
        };
        const variableId = String(body.parameters.variable_id ?? "");
        state.previewCalls!.push(variableId);
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
        // 与下方 point() 构造的 Key 逐字一致：channel / drive_object 两种形态。
        const target = (body.parameters.target ?? {}) as Record<string, unknown>;
        const key =
          typeof target.number === "number" && typeof target.entry === "string"
            ? `${variableId}@drive_object=${String(target.number)},entry=${target.entry}`
            : `${variableId}@${Object.entries(target)
                .map(([k, v]) => `${k}=${String(v)}`)
                .join(",")}`;
        return ok({
          point_key: key,
          identity_mode: "derived",
          scope: "collect",
          variable_id: variableId,
          definition_revision: body.parameters.definition_revision,
          definition_digest: "digest-test",
        });
      }
      if (url.startsWith("/api/v1/drivers/")) return ok({ resources: RESOURCES });
      if (url.startsWith("/api/v1/tasks?endpoint=")) {
        if (state.tasksGetFails) return { ok: false, status: 500, json: async () => ({}) };
        return ok({ tasks: state.savedTasks ?? state.tasks });
      }
      if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
        const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
        state.putBodies.push(body.tasks);
        if (!state.putOk) return { ok: false, status: 400, json: async () => state.putErrorBody };
        state.savedTasks = state.readBack ?? body.tasks;
        return ok({ ok: true });
      }
      return { ok: false, status: 404, json: async () => ({}) };
    },
  );
}

function mkState(overrides: Partial<MockState> = {}): MockState {
  return {
    tasks: [],
    putBodies: [],
    putOk: true,
    putErrorBody: {},
    tasksGetFails: false,
    readBack: null,
    savedTasks: null,
    ...overrides,
  };
}

/** 合成目录：36 collect + 6 identity，条目 id 自造（不搬运 Driver 真实定义）。 */
function buildDefinitions(): NckDefinitionMap {
  const out: Record<string, NckDefinitionEntry> = {};
  for (let i = 1; i <= 36; i += 1) {
    out[`syn.collect.${i}`] = {
      variable_id: `syn.collect.${i}`,
      revision: 1,
      family: "828d",
      scope: "collect",
      targets: ["channel"],
      target_required: ["channel"],
      arguments: [],
      label: `Collect ${i}`,
    };
  }
  for (let i = 1; i <= 6; i += 1) {
    // 刻意不给 label：若它出现在 DOM 里，只会以 id 形式出现，便于断言"未渲染"。
    out[`syn.identity.${i}`] = {
      variable_id: `syn.identity.${i}`,
      revision: 1,
      family: "828d",
      scope: "identity",
      targets: ["axis"],
      target_required: ["axis"],
      arguments: [],
    };
  }
  return out;
}

function point(variableId: string, target: Record<string, number>): NckPointSelection {
  return {
    kind: "nck_variable",
    point_key: `${variableId}@${Object.entries(target)
      .map(([k, v]) => `${k}=${v}`)
      .join(",")}`,
    parameters: { variable_id: variableId, definition_revision: 1, target, arguments: {} },
  };
}

function nckTask(
  id: string,
  points: NckPointSelection[],
  schedule: AcquisitionTaskShape["schedule"] = { mode: "poll", interval_ms: 1000 },
): AcquisitionTaskShape {
  return { id, schedule, binding: { kind: NCK_POINTS_KIND, config: { points } } };
}

/** 保存区的主按钮（无修改时保存区不渲染，取不到是**预期**行为）。 */
function saveButton(): HTMLButtonElement {
  return screen.getByTestId("apply-button") as HTMLButtonElement;
}

/** 等身份核验落定：行上带 `data-verify-status="verified"`（绿色标签已删除）。 */
async function waitVerified(count = 1) {
  await waitFor(() =>
    expect(document.querySelectorAll('[data-verify-status="verified"]').length).toBe(count),
  );
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

function writeCalls(): string[] {
  const calls = (
    globalThis.fetch as unknown as { mock: { calls: Array<[string, RequestInit | undefined]> } }
  ).mock.calls;
  return calls
    .map(([url, init]) => `${init?.method ?? "GET"} ${String(url)}`)
    .filter((call) => call.startsWith("PUT ") || /POST .*\/(stop|start)$/.test(call));
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  // antd message 是全局单例：不销毁会跨用例残留，造成"上一条成功提示"污染断言。
  message.destroy();
});

describe("关键回归 6：UI 只提供 36 条 collect（scope 准入接线）", () => {
  it("nck_ui_offers_only_collectable_definitions", async () => {
    const user = userEvent.setup();
    const state = mkState({
      tasks: [nckTask("nck-task", [point("syn.collect.1", { channel: 1 })])],
    });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildDefinitions()}
      />,
    );
    // §2.5：目录统计不在默认页面上，收进抽屉（搜索即列表）。
    await waitFor(() => expect(screen.queryByTestId("nck-point-row")).toBeTruthy());
    expect(screen.queryByTestId("nck-collectable-summary")).toBeNull();
    await user.click(screen.getByTestId("add-point-entry"));
    const summary = await screen.findByTestId("nck-collectable-summary");
    expect(summary.textContent).toContain("36 个可配置变量");
    expect(screen.queryAllByTestId("nck-collectable-def")).toHaveLength(0);
    // 树上可见叶子 = 当前可提供的变量集合（原「下拉框 option 数」的等价口径）。
    expect(catalogLeafIds()).toHaveLength(36);
    // 6 条 identity 定义不得出现在可采集集合里（不是"显示为禁用"，而是根本不渲染）。
    for (let i = 1; i <= 6; i += 1) {
      expect(screen.queryByText(`syn.identity.${i}`)).toBeNull();
    }
    await user.click(screen.getByTestId("drawer-cancel"));

    // 准入通过 + 点位来自快照，但**无实际修改**时保存区不渲染（§4.4.2 / §4.7）；
    // 制造一次周期变更后必须变为可用。
    await waitVerified(1);
    expect(screen.queryByTestId("apply-button")).toBeNull();
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
  });
});

describe("关键回归 3/4：快照未就绪与任务歧义都不发 PUT", () => {
  it("snapshot_load_failure_disables_save", async () => {
    const state = mkState({
      tasksGetFails: true,
      tasks: [nckTask("nck-task", [point("syn.collect.1", { channel: 1 })])],
    });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildDefinitions()}
      />,
    );
    // 真实问题照常出现（§3.3）：可处理原因 + 重试入口。
    expect(await screen.findByTestId("tasks-load-error")).toBeTruthy();
    expect(screen.getByTestId("retry-load")).toBeTruthy();
    // 没有可信基线 ⇒ 连「待保存」都不成立，保存区不渲染（fail-closed）。
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(state.putBodies).toHaveLength(0);
    expect(writeCalls()).toEqual([]);
  });

  it("ambiguous_nck_tasks_block_editing", async () => {
    const state = mkState({
      tasks: [
        nckTask("nck-a", [point("syn.collect.1", { channel: 1 })]),
        nckTask("nck-b", [point("syn.collect.2", { channel: 1 })]),
      ],
    });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildDefinitions()}
      />,
    );
    await screen.findByText(/现有任务无法安全编辑/);
    expect(screen.getByText(/MULTIPLE_EDITABLE_TASKS/)).toBeTruthy();
    // 无法安全编辑 ⇒ 没有添加入口，也没有保存区。
    expect(screen.queryByTestId("add-point-entry")).toBeNull();
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(state.putBodies).toHaveLength(0);
  });
});

describe("关键回归 5：无实际编辑变化不发 PUT、不报成功", () => {
  it("no_change_save_sends_no_put_and_reports_no_success", async () => {
    const state = mkState({
      tasks: [nckTask("nck-task", [point("syn.collect.1", { channel: 1 })])],
    });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildDefinitions()}
      />,
    );
    await waitVerified(1);
    // 无实际修改 ⇒ 保存区**完全不渲染**（不是禁用；§4.7 隐藏态）。
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(screen.queryByTestId("apply-result")).toBeNull();

    // 改一次再改回去 ⇒ 草稿回到基线，保存区收起，全程零写请求。
    await changeInterval(2000);
    await waitFor(() => expect(screen.getByTestId("draft-save-bar")).toBeTruthy());
    await changeInterval(1000);
    await waitFor(() => expect(screen.queryByTestId("draft-save-bar")).toBeNull());
    expect(writeCalls()).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
    expect(screen.queryByTestId("apply-result")).toBeNull();
  });
});

describe("关键回归 1/2：移除点位 ⇒ 更新同一 NCK 任务 + 回读校验", () => {
  it("removing_point_updates_same_nck_task_after_readback", async () => {
    const user = userEvent.setup();
    const kept = point("syn.collect.2", { channel: 2 });
    const state = mkState({
      tasks: [nckTask("nck-task", [point("syn.collect.1", { channel: 1 }), kept])],
    });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildDefinitions()}
      />,
    );
    // 等快照与身份核验就绪（无修改时保存区不渲染；移除后才产生真实变更）。
    await waitVerified(2);
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    // 移除走「⋯ → 移除」：先改草稿（可撤销），不发请求。
    await user.click(screen.getAllByTestId("point-row-more")[0]!);
    await user.click(await screen.findByTestId("point-row-remove"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    // 回读一致（桩回填刚保存的任务）才算成功；成功提示只由保存区结果条展示一次。
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存");
    expect(state.putBodies).toHaveLength(1);
    const sent = state.putBodies[0];
    // 只写一个任务、kind 恒 nck.points.v1、ID 保留、points 只剩保留项。
    expect(sent).toHaveLength(1);
    expect(sent[0].id).toBe("nck-task");
    expect(sent[0].binding.kind).toBe(NCK_POINTS_KIND);
    const config = sent[0].binding.config as { points: NckPointSelection[] };
    expect(config.points).toEqual([kept]);
    expect(JSON.stringify(sent)).not.toContain("mesa.resources.v1");
  });

  it("successful_put_with_stale_readback_is_not_reported_as_success", async () => {
    const user = userEvent.setup();
    const original = [
      nckTask("nck-task", [point("syn.collect.1", { channel: 1 }), point("syn.collect.2", { channel: 2 })]),
    ];
    const state = mkState({ tasks: original, readBack: original });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildDefinitions()}
      />,
    );
    await waitVerified(2);
    await user.click(screen.getAllByTestId("point-row-more")[0]!);
    await user.click(await screen.findByTestId("point-row-remove"));
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    // PUT 成功但回读仍是旧内容 ⇒ 进入「保存结果待核对」，绝不报成功。
    const pending = await screen.findByTestId("save-outcome");
    expect(pending.textContent).toContain("保存结果待核对");
    expect(pending.textContent).toContain("回读不一致");
    expect(screen.queryByTestId("apply-result")).toBeNull();
    expect(state.putBodies).toHaveLength(1);
    // 待核对不得直接重新 PUT：只有「重新核对」入口。
    expect(screen.getByTestId("recheck-save")).toBeTruthy();
    expect(screen.queryByTestId("resave-after-pending")).toBeNull();
  });
});

describe("UI 双执行：快照里的 identity 点位同样被拒绝", () => {
  it("identity_point_in_snapshot_blocks_save", async () => {
    const state = mkState({
      tasks: [nckTask("nck-task", [point("syn.identity.1", { axis: 1 })])],
    });
    installFetch(state);
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildDefinitions()}
      />,
    );
    await screen.findByTestId("nck-legacy-identity-alert");
    expect(screen.getAllByText(/当前任务包含不可采集的身份探针点位/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/SCOPE_IDENTITY/).length).toBeGreaterThan(0);
    // 保存路径被准入拦截：保存区不渲染、无写请求。
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(state.putBodies).toHaveLength(0);
  });
});

describe("非 NCK：零行为变化（UI 级证据）", () => {
  it("non_nck_put_body_is_identical_to_legacy_merge", async () => {
    const user = userEvent.setup();
    const selections: ResourceSelection[] = [
      { resource_id: "variable", parameters: {}, outputs: [{ output: "value", point_key: "legacy.k1" }] },
    ];
    const canonical: AcquisitionTaskShape = {
      id: "task-a",
      schedule: { mode: "poll", interval_ms: 1000 },
      binding: { kind: CANONICAL_RESOURCES_KIND, config: { selections } },
    };
    const other: AcquisitionTaskShape = {
      id: "task-b",
      schedule: { mode: "subscribe" },
      binding: { kind: "custom.v0", config: {} },
    };
    const state = mkState({ tasks: [canonical, other] });
    installFetch(state);
    render(<EndpointAcquisitionPane endpointId="ep-s7" driverId="s7" onChanged={() => {}} />);
    // 点位与快照一致 ⇒ 无实际修改，保存区不渲染（不再像旧实现那样照发 PUT）。
    await waitFor(() => expect(screen.getByTestId("sel-point-row")).toBeTruthy());
    expect(screen.queryByTestId("apply-button")).toBeNull();
    // 只改周期（点位完全相同）也必须被当成真实修改并发请求（§4.4.2 / §4.4.3）。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    const save = await screen.findByTestId("apply-button");
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false));
    await user.click(save);
    await waitFor(() => expect(state.putBodies).toHaveLength(1));
    // 逐字等于旧 merge 的结果：非 NCK 路径的编辑语义没有被本批改动。
    expect(state.putBodies[0]).toEqual(
      mergeAcquisitionTasks(state.tasks, { interval_ms: 2000, selections }),
    );
    expect(JSON.stringify(state.putBodies[0])).not.toContain(NCK_POINTS_KIND);
  });

  // 非 NCK 的「无实际修改」同样必须拦截：旧实现只有 NCK 有 NO_CHANGES，
  // 非 NCK 的 changed 仅作信息，于是「什么都没改」也会整份重写服务端配置。
  it("non_nck_no_change_sends_no_put_stop_or_start", async () => {
    const selections: ResourceSelection[] = [
      { resource_id: "variable", parameters: {}, outputs: [{ output: "value", point_key: "legacy.k1" }] },
    ];
    const canonical: AcquisitionTaskShape = {
      id: "task-a",
      schedule: { mode: "poll", interval_ms: 1000 },
      binding: { kind: CANONICAL_RESOURCES_KIND, config: { selections } },
    };
    const state = mkState({ tasks: [canonical] });
    installFetch(state);
    render(<EndpointAcquisitionPane endpointId="ep-s7" driverId="s7" onChanged={() => {}} />);
    await waitFor(() => expect(screen.getByTestId("sel-point-row")).toBeTruthy());
    // 无实际修改 ⇒ 没有可提交的入口（保存区不渲染），也没有任何写请求。
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(writeCalls()).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
    // 改一次再改回去：草稿回到基线后保存区同样收起（抵消规则）。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    await waitFor(() => expect(screen.getByTestId("draft-save-bar")).toBeTruthy());
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "1000" } });
    await waitFor(() => expect(screen.queryByTestId("draft-save-bar")).toBeNull());
    expect(state.putBodies).toHaveLength(0);
    expect(screen.queryByTestId("apply-result")).toBeNull();
  });
});
