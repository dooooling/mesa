// PR-C 第二批 A：EndpointAcquisitionPane 的 NCK 接线回归（组件级 / UI 侧）。
//
// 覆盖评审要求的关键回归（UI 侧证据）：
// 3. 任务快照未就绪 ⇒ 保存禁用、不发 PUT；
// 4. 歧义（两个 NCK 任务）⇒ 禁止编辑、不发 PUT；
// 5. 无实际编辑变化 ⇒ 不发 PUT、**不显示"已保存"**；
// 6. 42 条定义（36 collect + 6 identity）⇒ UI 只提供 36 条 collect；
// 1/2. 移除点位 ⇒ 更新同一个 NCK 任务、保存后回读成功才报成功；
// 以及"保存 2xx 但回读不一致 ⇒ 不报成功"（接口成功 ≠ 编辑已持久化）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import {
  EndpointAcquisitionPane,
  type NckDefinitionEntry,
  type NckDefinitionMap,
} from "./EndpointAcquisitionPane";
import { invokeDisabledButton } from "../test/bypassDisabled";
import {
  CANONICAL_RESOURCES_KIND,
  NCK_POINTS_KIND,
  mergeAcquisitionTasks,
  type AcquisitionTaskShape,
  type NckPointSelection,
  type ResourceSelection,
} from "../deviceModel";

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

function saveButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: /保存/ }) as HTMLButtonElement;
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
    const summary = await screen.findByTestId("nck-collectable-summary");
    // P1-1：默认只显示数量文字，不再铺 36 个不可点击的 Tag。
    expect(summary.textContent).toContain("36 个可配置变量");
    expect(screen.queryAllByTestId("nck-collectable-def")).toHaveLength(0);
    const options = Array.from(
      (screen.getByTestId("nck-variable-select") as HTMLSelectElement).querySelectorAll("option"),
    ).map((o) => o.value);
    expect(options.filter(Boolean)).toHaveLength(36);
    // 6 条 identity 定义不得出现在可采集集合里（不是"显示为禁用"，而是根本不渲染）。
    for (let i = 1; i <= 6; i += 1) {
      expect(screen.queryByText(`syn.identity.${i}`)).toBeNull();
    }
    // 准入通过 + 点位来自快照 ⇒ 保存链路就绪。但**无实际修改**时按钮禁用
    // （所有驱动统一拦截，§4.4.2）：制造一次周期变更后必须变为可用。
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    fireEvent.change(screen.getByRole("spinbutton"), { target: { value: "2000" } });
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
    await screen.findByText(/任务快照加载失败/);
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    expect(state.putBodies).toHaveLength(0);
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
    await waitFor(() => expect(saveButton().disabled).toBe(true));
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
    // 等待身份核验落定（否则按钮禁用可能只是因为「正在核验」）。
    await screen.findAllByText("配置格式校验通过");
    // 无实际修改 ⇒ 按钮禁用（§4.4.2）。
    await waitFor(() => expect(saveButton().disabled).toBe(true));

    // 关键：按钮禁用只是体验，**提交函数也必须守卫**。
    // 绕过 disabled 直接触发组件级 onClick：若守卫缺失，这里会真的发 stop/PUT/start。
    const fetchMock = globalThis.fetch as unknown as {
      mock: { calls: Array<[string, RequestInit | undefined]> };
    };
    const before = fetchMock.mock.calls.length;
    expect(invokeDisabledButton(saveButton())).toBe(true);
    // 明确提示"未发送保存请求"，且**绝不**出现"已保存"。
    await screen.findByText(/未检测到实际修改/);
    expect(screen.queryByTestId("apply-result")).toBeNull();
    expect(state.putBodies).toHaveLength(0);
    // 无修改时连 stop / start / preview 都不该发（计划在核验之前就已拒绝）。
    expect(fetchMock.mock.calls.length).toBe(before);
    expect(
      fetchMock.mock.calls.some(([url]) => /\/stop$|\/start$/.test(String(url))),
    ).toBe(false);
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
    // 等待快照与身份核验就绪（无修改时按钮禁用，移除后才产生真实变更）。
    await screen.findAllByText("配置格式校验通过");
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    const removeButtons = screen.getAllByRole("button", { name: /移\s*除/ });
    expect(removeButtons).toHaveLength(2);
    await user.click(removeButtons[0]);
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
    await screen.findAllByText("配置格式校验通过");
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    await user.click(screen.getAllByRole("button", { name: /移\s*除/ })[0]);
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
    await waitFor(() => expect(saveButton().disabled).toBe(true));
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
    // 点位与快照一致 ⇒ 无实际修改，按钮禁用（不再像旧实现那样照发 PUT）。
    await waitFor(() => expect(saveButton().disabled).toBe(true));
    // 只改周期（点位完全相同）也必须被当成真实修改并发请求（§4.4.2 / §4.4.3）。
    fireEvent.change(screen.getByRole("spinbutton"), { target: { value: "2000" } });
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
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
    await waitFor(() => expect(saveButton().disabled).toBe(true));

    // 按钮禁用只是体验：绕过 disabled 直接触发提交函数，仍不得发出任何写请求。
    const fetchMock = globalThis.fetch as unknown as {
      mock: { calls: Array<[string, RequestInit | undefined]> };
    };
    const before = fetchMock.mock.calls.length;
    expect(invokeDisabledButton(saveButton())).toBe(true);
    await screen.findByText(/未检测到实际修改/);
    expect(state.putBodies).toHaveLength(0);
    expect(fetchMock.mock.calls.length).toBe(before);
    expect(
      fetchMock.mock.calls.some(([url]) => /\/stop$|\/start$/.test(String(url))),
    ).toBe(false);
    // 不得报成功。
    expect(screen.queryByTestId("apply-result")).toBeNull();
  });
});
