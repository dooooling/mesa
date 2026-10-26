// 采集页第 1 步（状态与保存流程）门禁 —— 第 2 步（页面交互）后的版本。
//
// 覆盖评审稿的六项交付里可直接观测的行为：
// 1. 运行态三态：加载失败 / 切换连接 ⇒ unknown，**不沿用**旧值、不当成「已停止」；
// 2. unknown ⇒ 禁止任何保存（含只写配置），绕过 disabled 也不发任何请求；
// 3. 保存前重新确认运行状态：不一致 ⇒ 中止、不发 stop/PUT/start，由用户再次点击；
// 4. 成功 = 回读与**完整提交配置**一致；回读不一致 ⇒「保存结果待核对」，
//    不报成功、不刷新基线、不自动重新 PUT；
// 5. 配置已保存但启动失败 ⇒ 草稿标记清除，只重试启动（不重复整套保存）；
// 6. 稳定 rowId：编辑/删除/新增后同一行的 data-row-id 不变，删除的号不复用。
//
// 第 2 步只改「入口位置与查询方式」：无修改时保存区**不渲染**（不是禁用），
// 添加/编辑走抽屉，移除走行内「⋯ → 移除」。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane, type NckDefinitionMap } from "./EndpointAcquisitionPane";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { NCK_POINTS_KIND, type AcquisitionTaskShape, type NckPointSelection } from "../deviceModel";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-flow";
const EP2 = "ep-flow-2";

const DEFINITIONS: NckDefinitionMap = {
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
};

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

function point(variableId: string, channel: number): NckPointSelection {
  return {
    kind: "nck_variable",
    point_key: `${variableId}@channel=${channel}`,
    parameters: { variable_id: variableId, definition_revision: 1, target: { channel }, arguments: {} },
  };
}

function nckTask(
  points: NckPointSelection[],
  intervalMs = 1000,
): AcquisitionTaskShape {
  return {
    id: "nck-task",
    schedule: { mode: "poll", interval_ms: intervalMs },
    binding: { kind: NCK_POINTS_KIND, config: { points } },
  };
}

interface MockState {
  /** 每次 GET /api/v1/endpoints 依次取一个值（最后一个重复）；"fail" = 请求失败。 */
  endpointStates: Array<string | "fail">;
  endpointCalls: number;
  tasks: AcquisitionTaskShape[];
  /** 回读返回值：null = 回显服务端当前任务；"fail" = 请求失败；数组 = 指定值。 */
  readBack: AcquisitionTaskShape[] | "fail" | null;
  putCount: number;
  putBodies: AcquisitionTaskShape[][];
  stopCalls: number;
  startCalls: number;
  startOk: boolean;
}

function mkState(overrides: Partial<MockState> = {}): MockState {
  return {
    endpointStates: ["STOPPED"],
    endpointCalls: 0,
    tasks: [],
    readBack: null,
    putCount: 0,
    putBodies: [],
    stopCalls: 0,
    startCalls: 0,
    startOk: true,
    ...overrides,
  };
}

function installFetch(state: MockState) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    if (url === "/api/v1/endpoints") {
      const idx = Math.min(state.endpointCalls, state.endpointStates.length - 1);
      const value = state.endpointStates[idx];
      state.endpointCalls += 1;
      if (value === "fail") return { ok: false, status: 500, json: async () => ({}) };
      return ok({
        endpoints: [
          {
            id: EP,
            driver_id: NCK_DRIVER,
            connection: { family: "828d" },
            runtime: { state: value },
          },
          {
            id: EP2,
            driver_id: NCK_DRIVER,
            connection: { family: "828d" },
            runtime: { state: value },
          },
        ],
      });
    }
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/definitions/identity-preview")) {
      const body = JSON.parse(String(init?.body)) as { parameters: Record<string, unknown> };
      const variableId = String(body.parameters.variable_id ?? "");
      const target = (body.parameters.target ?? {}) as Record<string, unknown>;
      return ok({
        point_key: `${variableId}@${Object.entries(target)
          .map(([k, v]) => `${k}=${String(v)}`)
          .join(",")}`,
        identity_mode: "derived",
        scope: "collect",
        variable_id: variableId,
        definition_revision: body.parameters.definition_revision,
        definition_digest: "digest-test",
      });
    }
    if (url.startsWith("/api/v1/drivers/")) return ok({ resources: RESOURCES });
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      // 同一个 URL 承担「加载快照」与「保存后回读」两种语义：PUT 之后才按回读桩返回。
      if (state.putCount > 0) {
        if (state.readBack === "fail") return { ok: false, status: 500, json: async () => ({}) };
        if (Array.isArray(state.readBack)) return ok({ tasks: state.readBack });
      }
      return ok({ tasks: state.tasks });
    }
    if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
      const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
      state.putBodies.push(body.tasks);
      state.putCount += 1;
      state.tasks = body.tasks;
      return ok({ ok: true });
    }
    if (url === `/api/v1/endpoints/${EP}/stop` && method === "POST") {
      state.stopCalls += 1;
      state.endpointStates = ["STOPPED"];
      state.endpointCalls = 0;
      return ok({ ok: true });
    }
    if (url === `/api/v1/endpoints/${EP}/start` && method === "POST") {
      state.startCalls += 1;
      if (!state.startOk) {
        return { ok: false, status: 500, json: async () => ({ error: { message: "启动失败（测试桩）" } }) };
      }
      state.endpointStates = ["RUNNING"];
      state.endpointCalls = 0;
      return ok({ ok: true });
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderPane(endpointId = EP) {
  return render(
    <EndpointAcquisitionPane
      endpointId={endpointId}
      driverId={NCK_DRIVER}
      onChanged={() => {}}
      nckDefinitions={DEFINITIONS}
    />,
  );
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

/**
 * 改采集周期前先等任务快照就绪：加载完成会用服务端周期覆盖输入框，
 * 过早改动会被覆盖掉（也会让「无修改」判定失效）。
 */
async function changeInterval(value: number) {
  await waitFor(() =>
    expect(screen.getByTestId("acquisition-status-interval").textContent).toContain("（已保存）"),
  );
  fireEvent.change(screen.getByTestId("acquisition-interval-input"), {
    target: { value: String(value) },
  });
}

function fetchCalls(): Array<[string, RequestInit | undefined]> {
  return (globalThis.fetch as unknown as { mock: { calls: Array<[string, RequestInit | undefined]> } })
    .mock.calls;
}

function writeCalls(): string[] {
  return fetchCalls()
    .map(([url, init]) => `${init?.method ?? "GET"} ${String(url)}`)
    .filter((call) => call.startsWith("PUT ") || /POST .*\/(stop|start)$/.test(call));
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

describe("交付 1/2：运行态三态与「未知禁止保存」", () => {
  it("运行态读取失败 ⇒ unknown（不是已停止），绕过 disabled 也不发任何请求", async () => {
    const state = mkState({
      endpointStates: ["fail"],
      tasks: [nckTask([point("syn.collect.a", 1)])],
    });
    installFetch(state);
    renderPane();

    // 三态：读不到 ⇒ 状态未知，绝不推断成「已停止」。
    expect(await screen.findByTestId("acquisition-run-state-unknown")).toBeTruthy();
    expect(screen.getByTestId("acquisition-status-state").textContent).toBe("状态未知");
    expect(screen.getByTestId("refresh-run-state")).toBeTruthy();
    // 草稿仍可编辑（点位行照常渲染）；等任务快照就绪后再验证提交函数守卫。
    expect(screen.getByTestId("nck-point-row")).toBeTruthy();
    await changeInterval(2000);

    // 有修改 ⇒ 保存区出现，但按钮禁用且只提供「刷新状态」。
    const save = (await screen.findByTestId("apply-button")) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(screen.getByTestId("save-bar-refresh-state")).toBeTruthy();
    const before = fetchCalls().length;
    // 绕过 disabled：提交函数自身的守卫必须拦住，且**一个请求都不发**。
    expect(invokeDisabledButton(save)).toBe(true);
    await act(async () => {});
    expect(fetchCalls().length).toBe(before);
    expect(writeCalls()).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
    expect(state.stopCalls).toBe(0);
    expect(state.startCalls).toBe(0);
    // 提示必须说明「已禁用保存」，并保留刷新入口。
    await waitFor(() =>
      expect(document.querySelector(".ant-message")?.textContent ?? "").toContain(
        "运行状态未知",
      ),
    );
  });

  it("加载失败后不沿用旧状态：切到新连接时旧 RUNNING 不得变成「已停止」", async () => {
    const state = mkState({
      // 第一台读成功（RUNNING），第二台读取失败。
      endpointStates: ["RUNNING", "fail"],
      tasks: [nckTask([point("syn.collect.a", 1)])],
    });
    installFetch(state);
    const view = renderPane(EP);
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );

    view.rerender(
      <EndpointAcquisitionPane
        endpointId={EP2}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={DEFINITIONS}
      />,
    );
    // 关键：读取失败 ⇒ 未知；既不是上一台的「运行中」，也不是「已停止」。
    expect(await screen.findByTestId("acquisition-run-state-unknown")).toBeTruthy();
    expect(screen.getByTestId("acquisition-status-state").textContent).toBe("状态未知");
    // 切换连接后草稿清空 ⇒ 没有「待保存」态，保存区不渲染。
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(writeCalls()).toEqual([]);
  });

  it("刷新状态：重新读取成功后转为已停止（不再是未知）", async () => {
    const state = mkState({
      endpointStates: ["fail", "STOPPED"],
      tasks: [nckTask([point("syn.collect.a", 1)])],
    });
    installFetch(state);
    renderPane();
    // 先等第一次读取结束（提示出现），否则刷新点击可能抢在它之前发出第二次请求。
    expect(await screen.findByTestId("acquisition-run-state-unknown")).toBeTruthy();
    fireEvent.click(screen.getByTestId("refresh-run-state"));
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("已停止"),
    );
    expect(screen.queryByTestId("acquisition-run-state-unknown")).toBeNull();
  });

  it("Endpoint 不在清单 ⇒ unknown（旧实现会当成「已停止」）", async () => {
    // 清单里没有当前 Endpoint：旧 isRunningState(undefined) === false ⇒ 被当成已停止，
    // 于是保存会直接 PUT（运行中则 409），这正是 §6-2 的语义缺口。
    (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
      if (url === "/api/v1/endpoints") {
        return ok({ endpoints: [{ id: "ep-other", runtime: { state: "STOPPED" } }] });
      }
      if (url.startsWith("/api/v1/drivers/")) return ok({ resources: RESOURCES });
      if (url.startsWith("/api/v1/tasks?endpoint=")) {
        return ok({ tasks: [nckTask([point("syn.collect.a", 1)])] });
      }
      return { ok: false, status: 404, json: async () => ({}) };
    });
    renderPane(EP);
    expect(await screen.findByTestId("acquisition-run-state-unknown")).toBeTruthy();
    expect(screen.getByTestId("acquisition-status-state").textContent).toBe("状态未知");
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(writeCalls()).toEqual([]);
  });
});

describe("交付 2（追加）：保存前重新确认运行状态", () => {
  it("复核发现状态变化 ⇒ 中止本次保存（不发 stop/PUT/start），再次点击才执行", async () => {
    const user = userEvent.setup();
    const state = mkState({
      // 加载时 RUNNING；保存复核时已变成 STOPPED（第三方操作）。
      endpointStates: ["RUNNING", "STOPPED"],
      tasks: [nckTask([point("syn.collect.a", 1)])],
    });
    installFetch(state);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    // 中止：没有任何写请求；界面以新状态为准。
    const outcome = await screen.findByTestId("save-outcome");
    expect(outcome.getAttribute("data-kind")).toBe("state-changed");
    expect(outcome.textContent).toContain("本次保存未执行");
    expect(writeCalls()).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("已停止"),
    );

    // 用户再次点击：这次状态一致（STOPPED），按已停止路径只写配置。
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
    await waitFor(() => expect(state.putBodies).toHaveLength(1));
    expect(state.stopCalls).toBe(0);
    expect(state.startCalls).toBe(0);
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存");
  });
});

describe("交付 3：稳定 rowId（编辑/删除/新增后同一行身份不变）", () => {
  it("编辑不改 rowId；删除不重排；新增不复用旧号", async () => {
    const user = userEvent.setup();
    const state = mkState({
      tasks: [nckTask([point("syn.collect.a", 1), point("syn.collect.b", 2)])],
    });
    installFetch(state);
    renderPane();
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(2));
    const rows = () => screen.getAllByTestId("nck-point-row");
    const idsOf = () => rows().map((row) => row.getAttribute("data-row-id"));
    const initial = idsOf();
    expect(initial.filter(Boolean)).toHaveLength(2);
    expect(new Set(initial).size).toBe(2);

    // 1) 编辑第二行：point_key 从 syn.collect.b@channel=2 变成 ...=9，但 rowId 不变。
    await user.click(within(rows()[1]!).getByTestId("point-row-edit"));
    const input = await screen.findByLabelText("target-channel");
    await user.clear(input);
    await user.type(input, "9");
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe("syn.collect.b@channel=9"),
    );
    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(2));
    expect(idsOf()).toEqual(initial);

    // 2) 删除第一行：剩下那行仍是它自己的 rowId（不按下标重排）。
    await user.click(within(rows()[0]!).getByTestId("point-row-more"));
    await user.click(await screen.findByTestId("point-row-remove"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    expect(idsOf()).toEqual([initial[1]]);

    // 3) 新增一行：发新号，且不复用被删除的 initial[0]。
    await user.click(screen.getByTestId("add-point-entry"));
    await user.selectOptions(
      (await screen.findByTestId("nck-variable-select")) as HTMLSelectElement,
      "syn.collect.a",
    );
    await user.type(screen.getByLabelText("target-channel"), "5");
    await screen.findByTestId("nck-point-key");
    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(2));
    const after = idsOf();
    expect(after[0]).toBe(initial[1]);
    expect(after[1]).not.toBe(initial[0]);
    expect(after[1]).not.toBe(initial[1]);
  }, 30_000);
});

describe("交付 4/5：完整配置回读与基线更新", () => {
  it("回读不一致（点位相同、周期没保存）⇒ 待核对、不报成功、不刷新基线、不重新 PUT", async () => {
    const user = userEvent.setup();
    const original = nckTask([point("syn.collect.a", 1)], 1000);
    const state = mkState({
      tasks: [original],
      // 服务端仍是旧周期：点位一模一样，只比点位会误报成功。
      readBack: [nckTask([point("syn.collect.a", 1)], 1000)],
    });
    installFetch(state);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("1000 ms（已保存）"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    const outcome = await screen.findByTestId("save-outcome");
    expect(outcome.getAttribute("data-kind")).toBe("pending-verify");
    expect(outcome.textContent).toContain("保存结果待核对");
    expect(outcome.textContent).toContain("回读不一致");
    // 不报成功。
    expect(screen.queryByTestId("apply-result")).toBeNull();
    // 只发了一次 PUT；不刷新基线（周期仍显示「已保存 1000 → 待保存 2000」）。
    expect(state.putBodies).toHaveLength(1);
    expect(screen.getByTestId("acquisition-status-interval").textContent).toBe(
      "1000 ms（已保存） → 2000 ms（待保存）",
    );
    // 待核对期间主操作只能是「重新核对」：主保存按钮不可用，不得直接重新 PUT。
    expect(saveButton().disabled).toBe(true);
    expect(screen.getByTestId("recheck-save")).toBeTruthy();
    expect(screen.queryByTestId("resave-after-pending")).toBeNull();

    // 再次核对仍不一致 ⇒ 仍不 PUT，此时才出现「重新保存」（用户主动触发）。
    await user.click(screen.getByTestId("recheck-save"));
    await waitFor(() => expect(screen.getByTestId("resave-after-pending")).toBeTruthy());
    expect(state.putBodies).toHaveLength(1);

    // 服务端最终写入后，重新核对一致 ⇒ 刷新基线并结束待核对（仍不重复 PUT）。
    state.readBack = null;
    await user.click(screen.getByTestId("recheck-save"));
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("2000 ms（已保存）"),
    );
    expect(screen.queryByTestId("save-outcome")).toBeNull();
    expect(state.putBodies).toHaveLength(1);
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存，当前停止");
  });

  it("回读请求失败 ⇒ 待核对（不是成功），且不重新 PUT", async () => {
    const user = userEvent.setup();
    const state = mkState({
      tasks: [nckTask([point("syn.collect.a", 1)])],
      readBack: "fail",
    });
    installFetch(state);
    renderPane();
    await waitVerified(1);
    // 无修改时保存区不渲染；产生修改后才出现按钮。
    expect(screen.queryByTestId("apply-button")).toBeNull();
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    const outcome = await screen.findByTestId("save-outcome");
    expect(outcome.getAttribute("data-kind")).toBe("pending-verify");
    expect(outcome.textContent).toContain("回读失败");
    expect(screen.queryByTestId("apply-result")).toBeNull();
    expect(state.putBodies).toHaveLength(1);
    // 待核对期间保存区必须仍然可见并保留入口（即使草稿已回到基线）。
    expect(screen.getByTestId("draft-save-bar")).toBeTruthy();
    expect(screen.getByTestId("draft-save-notice").textContent).toContain("保存结果待核对");
  });

  it("周期是唯一变更也必须发请求（点位相同）", async () => {
    const user = userEvent.setup();
    const p = point("syn.collect.a", 1);
    const state = mkState({ tasks: [nckTask([p], 1000)] });
    installFetch(state);
    renderPane();
    await waitVerified(1);
    expect(screen.queryByTestId("apply-button")).toBeNull();
    await changeInterval(500);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
    await waitFor(() => expect(state.putBodies).toHaveLength(1));
    const sent = state.putBodies[0][0];
    expect(sent.schedule).toEqual({ mode: "poll", interval_ms: 500 });
    // 点位逐字不变：证明差异判定包含周期，而不是只比点位。
    const sentPoints = (sent.binding.config as { points: NckPointSelection[] }).points;
    expect(sentPoints).toEqual([p]);
  });

  it("保存后基线整份刷新：无修改时保存区收起、不再发 PUT", async () => {
    const user = userEvent.setup();
    const state = mkState({ tasks: [nckTask([point("syn.collect.a", 1)], 1000)] });
    installFetch(state);
    renderPane();
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());
    await waitFor(() => expect(state.putBodies).toHaveLength(1));
    await screen.findByTestId("apply-result");

    // 基线已刷新为「周期 2000 + 同一点位」⇒ 无实际修改：保存动作收起，
    // 只剩一次简短结果行（不再有待保存摘要与按钮）。
    await waitFor(() => expect(screen.queryByTestId("apply-button")).toBeNull());
    expect(screen.queryByTestId("discard-draft")).toBeNull();
    expect(screen.queryByTestId("draft-save-summary")).toBeNull();
    expect(state.putBodies).toHaveLength(1);
  });
});

describe("交付 6：配置已保存但启动失败 ⇒ 只重试启动", () => {
  it("启动失败后重试只调用 start（不 stop、不重复 PUT）", async () => {
    const user = userEvent.setup();
    const state = mkState({
      endpointStates: ["RUNNING"],
      tasks: [nckTask([point("syn.collect.a", 1)])],
      startOk: false,
    });
    installFetch(state);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    await user.click(saveButton());

    // 回读一致 ⇒ 基线已刷新（草稿标记清除），但启动失败：保存区转为结果态。
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("2000 ms（已保存）"),
    );
    const notice = await screen.findByTestId("draft-save-notice");
    expect(notice.textContent).toContain("配置已保存，但恢复采集失败");
    expect(screen.getByTestId("retry-start")).toBeTruthy();
    expect(state.stopCalls).toBe(1);
    expect(state.putBodies).toHaveLength(1);
    expect(state.startCalls).toBe(1);

    // 重试启动：只调用 start。
    state.startOk = true;
    await user.click(screen.getByTestId("retry-start"));
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    expect(state.stopCalls).toBe(1);
    expect(state.putBodies).toHaveLength(1);
    expect(state.startCalls).toBe(2);
    // 结果态收起：不再有重试入口与保存按钮，只留一次成功结果。
    await waitFor(() => expect(screen.queryByTestId("retry-start")).toBeNull());
    expect(screen.queryByTestId("apply-button")).toBeNull();
  });
});

