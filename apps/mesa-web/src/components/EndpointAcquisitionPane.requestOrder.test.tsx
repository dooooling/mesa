// 第 3 步验收：**请求顺序**证据（§4.4 / §4.7 / §5.4「典型失败信号」）。
//
// 在每种状态下点保存，验证请求顺序及最终界面结果，避免为同一请求链另挂一次页面。
// 计数式断言（putCount === 1）证明不了顺序，因此这里逐条比对序列，并且专门验证
// 「守卫都在 stop/PUT/start 之前」——即被拒绝的保存连一个写请求都不该发出。
//
// 夹具刻意保持**小规模**（2–4 个点位）：顺序语义与点数无关，小夹具让失败信息可读；
// 238 点规模下的同一路径由 `EndpointAcquisitionPane.acceptance.test.tsx` 覆盖。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane, type NckDefinitionMap } from "./EndpointAcquisitionPane";
import { invokeDisabledButton } from "../test/bypassDisabled";
import { createRequestProbe, installProbedFetch } from "../test/requestProbe";
import { planEndpointTaskSave } from "../endpointSavePlan";
import {
  NCK_POINTS_KIND,
  type AcquisitionTaskShape,
  type NckPointSelection,
} from "../deviceModel";
import type { ResourceDescriptor } from "../types";
import { CANDIDATE_AVAILABILITY } from "../test/availabilityFixture";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-order";

const DEFINITIONS: NckDefinitionMap = {
  "syn.collect.a": {
    variable_id: "syn.collect.a",
    revision: 1,
    family: "828d",
    scope: "collect",
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
    availability: CANDIDATE_AVAILABILITY,
  },
};

const NATIVE_FIELDS: ResourceDescriptor["parameters"]["fields"] = [
  { key: "area", label: "Area", field_type: "string", required: true, validation: {}, ui: {} },
  { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: {}, ui: {} },
  { key: "module", label: "Module", field_type: "integer", required: true, validation: {}, ui: {} },
  { key: "column", label: "Column", field_type: "integer", required: true, validation: {}, ui: {} },
  { key: "line", label: "Line", field_type: "integer", required: true, validation: {}, ui: {} },
];

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function fail(status = 500, body: unknown = {}) {
  return { ok: false, status, json: async () => body };
}

function point(channel: number): NckPointSelection {
  return {
    kind: "nck_variable",
    point_key: `syn.collect.a@channel=${channel}`,
    parameters: { variable_id: "syn.collect.a", definition_revision: 1, target: { channel }, arguments: {} },
  };
}

function nckTask(points: NckPointSelection[], intervalMs = 1000): AcquisitionTaskShape {
  return {
    id: "nck-task",
    schedule: { mode: "poll", interval_ms: intervalMs },
    binding: { kind: NCK_POINTS_KIND, config: { points } },
  };
}

interface MockState {
  /** 依次取一个值（最后一个重复）；"fail" = 读取失败。 */
  endpointStates: Array<string | "fail">;
  endpointCalls: number;
  tasks: AcquisitionTaskShape[];
  /** 回读桩："server" = 回显 PUT 写入的任务；"fail" = 500；数组 = 指定值。 */
  readBack: "server" | "fail" | AcquisitionTaskShape[];
  putBodies: AcquisitionTaskShape[][];
  stopCalls: number;
  startCalls: number;
  startOk: boolean;
  /** 身份预览桩：true ⇒ 预览失败（模拟提交前复核发现身份失效）。 */
  previewFails: boolean;
  /** Descriptor 的资源 Schema（未知字段类型用例会替换它）。 */
  fields: ResourceDescriptor["parameters"]["fields"];
}

function mkState(overrides: Partial<MockState> = {}): MockState {
  return {
    endpointStates: ["STOPPED"],
    endpointCalls: 0,
    tasks: [nckTask([point(1)])],
    readBack: "server",
    putBodies: [],
    stopCalls: 0,
    startCalls: 0,
    startOk: true,
    previewFails: false,
    fields: NATIVE_FIELDS,
    ...overrides,
  };
}

function installFetch(state: MockState, probe: ReturnType<typeof createRequestProbe>) {
  installProbedFetch(probe, async (url, init) => {
    const method = (init?.method ?? "GET").toUpperCase();
    if (url === "/api/v1/endpoints") {
      const idx = Math.min(state.endpointCalls, state.endpointStates.length - 1);
      const value = state.endpointStates[idx];
      state.endpointCalls += 1;
      if (value === "fail") return fail();
      return ok({
        endpoints: [
          { id: EP, driver_id: NCK_DRIVER, connection: { family: "828d" }, runtime: { state: value } },
        ],
      });
    }
    if (url.endsWith("/definitions/identity-preview")) {
      if (state.previewFails) {
        return fail(422, { error: { code: "IDENTITY_PREVIEW_FAILED", message: "预览失败（测试桩）" } });
      }
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
        definition_digest: "digest-order",
      });
    }
    if (url.startsWith("/api/v1/drivers/")) {
      return ok({
        resources: [
          {
            id: "variable",
            label: { default: "Variable" },
            parameters: { fields: state.fields },
            outputs: [],
            modes: ["poll"],
          },
        ],
        resource_selection_methods: ["manual", "browse"],
      });
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      // 同一个 URL 承担「加载快照」与「保存后回读」：有过 PUT 才按回读桩返回。
      if (state.putBodies.length > 0) {
        if (state.readBack === "fail") return fail();
        if (Array.isArray(state.readBack)) return ok({ tasks: state.readBack });
      }
      return ok({ tasks: state.tasks });
    }
    if (method === "PUT" && url.startsWith("/api/v1/tasks/")) {
      const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
      state.putBodies.push(body.tasks);
      state.tasks = body.tasks;
      return ok({ ok: true });
    }
    if (method === "POST" && url === `/api/v1/endpoints/${EP}/stop`) {
      state.stopCalls += 1;
      state.endpointStates = ["STOPPED"];
      state.endpointCalls = 0;
      return ok({ ok: true });
    }
    if (method === "POST" && url === `/api/v1/endpoints/${EP}/start`) {
      state.startCalls += 1;
      if (!state.startOk) return fail(500, { error: { message: "启动失败（测试桩）" } });
      state.endpointStates = ["RUNNING"];
      state.endpointCalls = 0;
      return ok({ ok: true });
    }
    return fail(404);
  });
}

function renderPane() {
  return render(
    <EndpointAcquisitionPane
      endpointId={EP}
      driverId={NCK_DRIVER}
      onChanged={() => {}}
      nckDefinitions={DEFINITIONS}
    />,
  );
}

/** 等任务快照就绪（状态区出现「（已保存）」才算基线建立）。 */
async function waitSnapshotReady() {
  await waitFor(() =>
    expect(screen.getByTestId("acquisition-status-interval").textContent).toContain("（已保存）"),
  );
}

/** 等身份核验落定（不依赖绿色标签：按行上的 data-verify-status 判定）。 */
async function waitVerified(count: number) {
  await waitFor(() =>
    expect(document.querySelectorAll('[data-verify-status="verified"]').length).toBe(count),
  );
}

async function changeInterval(value: number) {
  await waitSnapshotReady();
  fireEvent.change(screen.getByTestId("acquisition-interval-input"), {
    target: { value: String(value) },
  });
}

function saveButton(): HTMLButtonElement {
  return screen.getByTestId("apply-button") as HTMLButtonElement;
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

describe("请求顺序 1：无修改 ⇒ 零 PUT / stop / start", () => {
  it("no_changes_sends_no_write_request", async () => {
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitVerified(1);
    await waitSnapshotReady();
    // 只观察「加载完成之后」的请求：快照加载本身有一次 GET:tasks（只读）。
    probe.reset();
    // 无修改时保存区**不渲染**（不是禁用）。
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(probe.writeSequence()).toEqual([]);

    // 改周期再改回原值（等价的「无实际修改」）：保存区收起，仍然零写请求。
    await changeInterval(500);
    await screen.findByTestId("draft-save-bar");
    await changeInterval(1000);
    await waitFor(() => expect(screen.queryByTestId("draft-save-bar")).toBeNull());
    expect(probe.writeSequence()).toEqual([]);
  });
});

describe("请求顺序 2：已停止保存 ⇒ 仅 PUT + 回读 GET", () => {
  it("stopped_save_is_put_then_readback_only", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitVerified(1);
    const originalPoints = (state.tasks[0].binding.config as { points: NckPointSelection[] }).points;
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    expect(saveButton().textContent).toBe("保存配置");
    expect(screen.getByTestId("draft-save-note").textContent).toContain("不会启动采集");
    expect(screen.getByTestId("acquisition-status-interval").textContent).toBe(
      "1000 ms（已保存） → 2000 ms（待保存）",
    );

    probe.reset();
    await user.click(saveButton());
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存，当前停止");
    expect(state.putBodies).toHaveLength(1);
    // 只改周期也必须提交，点位参数保持原样；成功后完整基线更新，动作区收起。
    expect(state.putBodies[0][0].schedule).toEqual({ mode: "poll", interval_ms: 2000 });
    expect((state.putBodies[0][0].binding.config as { points: NckPointSelection[] }).points).toEqual(
      originalPoints,
    );
    expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("2000 ms（已保存）");
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(screen.queryByTestId("discard-draft")).toBeNull();
    expect(screen.queryByTestId("draft-save-summary")).toBeNull();

    // 已停止路径：不 stop、不 start，只有「PUT → 回读 GET」。
    expect(probe.writeSequence()).toEqual(["PUT:tasks", "GET:tasks"]);
    expect(state.stopCalls).toBe(0);
    expect(state.startCalls).toBe(0);
    // 保存前复核用的是 GET /endpoints（只读），发生在 PUT 之前。
    expect(probe.indexOf("GET:endpoints")).toBeLessThan(probe.indexOf("PUT:tasks"));
  });
});

describe("请求顺序 3：运行中保存并恢复 ⇒ stop → PUT → 回读 GET → start", () => {
  it("running_save_with_restart_is_stop_put_get_start", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ endpointStates: ["RUNNING"] });
    installFetch(state, probe);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    probe.reset();
    await user.click(saveButton());
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存，采集已启动");
    expect(state.stopCalls).toBe(1);
    expect(state.startCalls).toBe(1);
    expect(state.putBodies).toHaveLength(1);
    expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("2000 ms（已保存）");

    expect(probe.writeSequence()).toEqual(["STOP", "PUT:tasks", "GET:tasks", "START"]);
    // 守卫顺序：状态复核（GET /endpoints）与提交前身份核验（PREVIEW）都早于 stop。
    expect(probe.indexOf("GET:endpoints")).toBeGreaterThanOrEqual(0);
    expect(probe.indexOf("GET:endpoints")).toBeLessThan(probe.indexOf("STOP"));
    expect(probe.indexOf("PREVIEW")).toBeLessThan(probe.indexOf("STOP"));
    expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中");
  });
});

describe("请求顺序 4：运行中保存并停止 ⇒ stop → PUT → 回读 GET（无 start）", () => {
  it("running_save_keep_stopped_omits_start", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ endpointStates: ["RUNNING"] });
    installFetch(state, probe);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(screen.queryByTestId("draft-save-note")).toBeNull();
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    expect(saveButton().textContent).toBe("保存并恢复采集");
    expect(screen.getByTestId("draft-save-note").textContent).toBe(
      "保存会暂时中断当前连接的采集",
    );
    expect(screen.getByTestId("draft-save-note").textContent).not.toMatch(/1[–-]2 ?秒/);
    expect(screen.getByTestId("toggle-restart").textContent).toBe("保存后保持停止");
    // 次级选项：切到「保存后保持停止」，主按钮文案随动作改写。
    probe.reset();
    await user.click(screen.getByTestId("toggle-restart"));
    expect(saveButton().textContent).toBe("保存并停止采集");
    expect(screen.getByTestId("toggle-restart").textContent).toBe("保存后恢复采集");
    expect(probe.writeSequence()).toEqual([]);

    probe.reset();
    await user.click(saveButton());
    await waitFor(() => expect(state.putBodies).toHaveLength(1));

    expect(probe.writeSequence()).toEqual(["STOP", "PUT:tasks", "GET:tasks"]);
    expect(state.startCalls).toBe(0);
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("已停止"),
    );
  });
});

describe("请求顺序 5：回读不一致 ⇒ stop → PUT → 回读 GET，且不得出现第二个 PUT", () => {
  it("mismatched_readback_never_repeats_put", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    // 服务端仍是旧周期（点位一模一样）：只比点位会误报成功。
    const state = mkState({ endpointStates: ["RUNNING"], readBack: [nckTask([point(1)], 1000)] });
    installFetch(state, probe);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    probe.reset();
    await user.click(saveButton());
    const outcome = await screen.findByTestId("save-outcome");
    expect(outcome.getAttribute("data-kind")).toBe("pending-verify");
    // 不一致 ⇒ 不启动（回读一致才谈得上恢复采集）。
    expect(probe.writeSequence()).toEqual(["STOP", "PUT:tasks", "GET:tasks"]);
    expect(state.startCalls).toBe(0);
    // 不报成功。
    expect(screen.queryByTestId("apply-result")).toBeNull();

    // 重新核对：只再次 GET 比对，**不得**重新 PUT。
    probe.reset();
    await user.click(screen.getByTestId("recheck-save"));
    await waitFor(() => expect(screen.getByTestId("resave-after-pending")).toBeTruthy());
    expect(probe.calls.filter((call) => call === "PUT:tasks")).toHaveLength(0);
    expect(probe.writeSequence()).toEqual(["GET:tasks"]);
  });
});

describe("请求顺序 6：启动失败后重试 ⇒ 仅 start（无 stop、无 PUT）", () => {
  it("retry_start_sends_start_only", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ endpointStates: ["RUNNING"], startOk: false });
    installFetch(state, probe);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    probe.reset();
    await user.click(saveButton());
    await waitFor(() => expect(screen.getByTestId("retry-start")).toBeTruthy());
    expect(probe.writeSequence()).toEqual(["STOP", "PUT:tasks", "GET:tasks", "START"]);
    expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("2000 ms（已保存）");
    expect(screen.getByTestId("draft-save-notice").textContent).toContain(
      "配置已保存，但恢复采集失败",
    );
    expect(state.startCalls).toBe(1);

    // 重试启动：增量序列里只有 START。
    state.startOk = true;
    probe.reset();
    await user.click(screen.getByTestId("retry-start"));
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    expect(probe.calls.filter((call) => call !== "GET:endpoints")).toEqual(["START"]);
    expect(state.stopCalls).toBe(1);
    expect(state.putBodies).toHaveLength(1);
    expect(state.startCalls).toBe(2);
    await waitFor(() => expect(screen.queryByTestId("retry-start")).toBeNull());
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(screen.getByTestId("apply-result").textContent).toContain("配置已保存，采集已启动");
  });
});

describe("请求顺序 7：运行状态未知 ⇒ 零请求（含只读复核）", () => {
  it("unknown_state_sends_zero_requests_even_if_disabled_is_bypassed", async () => {
    const probe = createRequestProbe();
    const state = mkState({ endpointStates: ["fail"] });
    installFetch(state, probe);
    renderPane();
    expect(await screen.findByTestId("acquisition-run-state-unknown")).toBeTruthy();
    expect(screen.getByTestId("acquisition-status-state").textContent).toBe("状态未知");
    expect(screen.getByTestId("refresh-run-state")).toBeTruthy();
    expect(screen.getByTestId("nck-point-row")).toBeTruthy();
    await changeInterval(2000);
    const save = (await screen.findByTestId("apply-button")) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(screen.getByTestId("save-bar-refresh-state")).toBeTruthy();

    probe.reset();
    expect(invokeDisabledButton(save)).toBe(true);
    await act(async () => {});
    // 「零请求」是字面意思：连 GET /endpoints 复核与身份预览都不发。
    expect(probe.calls).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
    expect(state.stopCalls).toBe(0);
    expect(state.startCalls).toBe(0);
    await waitFor(() =>
      expect(document.querySelector(".ant-message")?.textContent ?? "").toContain("运行状态未知"),
    );
  });
});

describe("请求顺序 8：保存前复核发现状态变化 ⇒ 零 stop/PUT/start", () => {
  it("state_changed_during_recheck_aborts_before_any_write", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    // 加载时 RUNNING；点击保存时的复核读到 STOPPED（第三方已停）。
    const state = mkState({ endpointStates: ["RUNNING", "STOPPED"] });
    installFetch(state, probe);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    probe.reset();
    await user.click(saveButton());
    const outcome = await screen.findByTestId("save-outcome");
    expect(outcome.getAttribute("data-kind")).toBe("state-changed");
    expect(probe.writeSequence()).toEqual([]);
    // 复核失败后连身份预览都不发：复核在提交前核验之前。
    expect(probe.calls.filter((call) => call === "PREVIEW")).toHaveLength(0);
    expect(state.putBodies).toHaveLength(0);
    expect(outcome.textContent).toContain("本次保存未执行");
    await waitFor(() => expect(screen.getByTestId("acquisition-status-state").textContent).toBe("已停止"));

    // 状态变化后不自动重试；用户再次保存才按新的已停止路径提交。
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    probe.reset();
    await user.click(saveButton());
    expect((await screen.findByTestId("apply-result")).textContent).toContain("配置已保存");
    expect(probe.writeSequence()).toEqual(["PUT:tasks", "GET:tasks"]);
    expect(state.putBodies).toHaveLength(1);
    expect(state.stopCalls).toBe(0);
    expect(state.startCalls).toBe(0);
  });
});

describe("守卫顺序：Schema / 快照 / 未知状态都在 stop/PUT/start 之前", () => {
  it("schema_guard_rejects_before_any_request", async () => {
    const probe = createRequestProbe();
    // 原生字段 Schema 里出现界面不认识的类型（驱动升级）：整份保存必须被拒绝。
    const state = mkState({
      fields: [
        ...NATIVE_FIELDS,
        // `matrix` 不在界面支持清单里：故意用未知类型验证守卫（契约上不应出现，
        // 但驱动升级/降级时必须 fail-closed，因此这里显式绕过类型检查构造它）。
        {
          key: "waveform",
          label: "Waveform",
          field_type: "matrix",
          required: false,
          validation: {},
          ui: {},
        } as unknown as ResourceDescriptor["parameters"]["fields"][number],
      ],
      tasks: [
        nckTask([
          {
            kind: "nck_native",
            point_key: "native.C.1.122.1.1",
            parameters: { area: "C", area_no: 1, module: 122, column: 1, line: 1 },
          },
        ]),
      ],
    });
    installFetch(state, probe);
    renderPane();
    expect(await screen.findByTestId("acquisition-unsupported-fields")).toBeTruthy();
    await changeInterval(2000);
    const save = (await screen.findByTestId("apply-button")) as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    probe.reset();
    expect(invokeDisabledButton(save)).toBe(true);
    await act(async () => {});
    expect(probe.calls).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
  });

  it("snapshot_gate_rejects_in_the_plan_before_any_request", () => {
    // 快照未就绪 ⇒ 计划层直接拒绝（页面层据此连保存按钮都不给）。
    const plan = planEndpointTaskSave({
      driverId: NCK_DRIVER,
      tasks: [],
      snapshotState: "error",
      loadedEndpointId: EP,
      currentEndpointId: EP,
      intervalMs: 1000,
      nckPoints: [point(1)],
      nckDefinitions: DEFINITIONS,
    });
    expect(plan.ok).toBe(false);
    if (!plan.ok) expect(plan.code).toBe("SNAPSHOT_NOT_READY");
    // 拒绝时不产出任何可 PUT 的任务集。
    expect(Object.prototype.hasOwnProperty.call(plan, "tasks")).toBe(false);
  });

  it("pre_submit_identity_recheck_fails_closed_before_stop", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ endpointStates: ["RUNNING"] });
    installFetch(state, probe);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await waitVerified(1);
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));

    // 提交前复核失败：身份预览先发生，且失败后没有任何写请求。
    state.previewFails = true;
    probe.reset();
    await user.click(saveButton());
    await waitFor(() =>
      expect(document.querySelector(".ant-message")?.textContent ?? "").toContain(
        "保存前重新核验失败",
      ),
    );
    expect(probe.calls.filter((call) => call === "PREVIEW").length).toBeGreaterThan(0);
    expect(probe.writeSequence()).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
    expect(state.stopCalls).toBe(0);
  });
});

describe("过渡态（连接中/重连中）按运行中路径处理", () => {
  it("connecting_state_uses_stop_put_get_start", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ endpointStates: ["CONNECTING"] });
    installFetch(state, probe);
    renderPane();
    // 过渡态不得显示成「已停止」（§2.4）。
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    await changeInterval(2000);
    await waitFor(() => expect(saveButton().disabled).toBe(false));
    expect(saveButton().textContent).toContain("保存并恢复采集");

    probe.reset();
    await user.click(saveButton());
    await waitFor(() => expect(state.startCalls).toBe(1));
    expect(probe.writeSequence()).toEqual(["STOP", "PUT:tasks", "GET:tasks", "START"]);
  });
});
