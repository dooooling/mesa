// 采集页 UX 回归（P0-2 / P1-1 / P1-2 / P2-1 / P2-2）。
//
// 这些断言刻意都落在**用户能看到的文案与按钮**上：按钮说“保存配置”就必须
// 不启动采集；底部说明必须随 Endpoint 状态/勾选变化；绿色标签必须只声明
// “配置格式校验通过”，不得暗示真实机床读取成功。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane, type NckDefinitionMap } from "./EndpointAcquisitionPane";
import { NCK_POINTS_KIND, type AcquisitionTaskShape, type NckPointSelection } from "../deviceModel";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-nck";

const DEFINITIONS: NckDefinitionMap = {
  "syn.collect.a": {
    variable_id: "syn.collect.a",
    revision: 1,
    family: "828d",
    scope: "collect",
    label: { zh: "主轴转速", en: "Spindle speed" },
    targets: ["channel"],
    target_required: ["channel"],
    arguments: [],
  },
};

const NATIVE_SCHEMA = [
  { key: "area", label: "Area", field_type: "enum", required: true, validation: { enum_options: ["A"] }, ui: {} },
  { key: "area_no", label: "Area No.", field_type: "integer", required: true, validation: { min: 0, max: 31 }, ui: {} },
  { key: "module", label: "Module", field_type: "integer", required: true, validation: { min: 0, max: 255 }, ui: {} },
  { key: "column", label: "Column", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
  { key: "line", label: "Line", field_type: "integer", required: true, validation: { min: 0, max: 65535 }, ui: {} },
];

const RESOURCES = [
  {
    id: "variable",
    label: { default: "Variable" },
    parameters: { fields: NATIVE_SCHEMA },
    outputs: [],
    modes: ["poll"],
  },
];

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
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

const NATIVE_POINT: NckPointSelection = {
  kind: "nck_native",
  point_key: "native.A.1.26.30110.1",
  parameters: { area: "A", area_no: 1, module: 26, column: 30110, line: 1 },
};

interface MockState {
  endpointState: "STOPPED" | "RUNNING";
  tasks: AcquisitionTaskShape[];
  savedTasks: AcquisitionTaskShape[] | null;
  putBodies: AcquisitionTaskShape[][];
  stopCalls: number;
  startCalls: number;
}

function mkState(overrides: Partial<MockState> = {}): MockState {
  return {
    endpointState: "STOPPED",
    tasks: [],
    savedTasks: null,
    putBodies: [],
    stopCalls: 0,
    startCalls: 0,
    ...overrides,
  };
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
            runtime: { state: state.endpointState },
          },
        ],
      });
    }
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/definitions/identity-preview")) {
      const body = JSON.parse(String(init?.body)) as {
        parameters: Record<string, unknown>;
      };
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
      return ok({ tasks: state.savedTasks ?? state.tasks });
    }
    if (url === `/api/v1/endpoints/${EP}/stop` && method === "POST") {
      state.stopCalls += 1;
      state.endpointState = "STOPPED";
      return ok({ ok: true });
    }
    if (url === `/api/v1/endpoints/${EP}/start` && method === "POST") {
      state.startCalls += 1;
      state.endpointState = "RUNNING";
      return ok({ ok: true });
    }
    if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
      const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
      state.putBodies.push(body.tasks);
      state.savedTasks = body.tasks;
      return ok({ ok: true });
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function nckTask(points: NckPointSelection[]): AcquisitionTaskShape {
  return {
    id: "nck-task",
    schedule: { mode: "poll", interval_ms: 1000 },
    binding: { kind: NCK_POINTS_KIND, config: { points } },
  };
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

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

describe("P0-2 / P2-2：已停止时的按钮、说明与状态区", () => {
  it("stopped_endpoint_never_claims_start_and_status_is_truthful", async () => {
    const state = mkState({ tasks: [nckTask([point("syn.collect.a", { channel: 1 })])] });
    installFetch(state);
    renderPane();

    const status = await screen.findByTestId("acquisition-status");
    expect(within(status).getByTestId("acquisition-status-state").textContent).toBe("已停止");
    expect(within(status).getByTestId("acquisition-status-interval").textContent).toBe("1000 ms");
    await waitFor(() =>
      expect(within(status).getByTestId("acquisition-status-points").textContent).toBe("1 个"),
    );
    expect(within(status).getByTestId("acquisition-status-after").textContent).toBe(
      "保存后保持停止",
    );

    const button = screen.getByTestId("apply-button");
    expect(button.textContent).toBe("保存配置");
    expect(screen.getByTestId("apply-stopped-note").textContent).toContain("保存后保持停止");
    const note = screen.getByTestId("acquisition-execution-note").textContent ?? "";
    expect(note).toContain("不会启动采集");
    expect(note).toContain("保持停止");
    // 不得再写死旧路径。
    expect(note).not.toContain("Stop → PUT");
    expect(note).not.toContain("→ Start");
  });

  it("saving_while_stopped_writes_config_and_reports_currently_stopped", async () => {
    const user = userEvent.setup();
    const state = mkState({ tasks: [nckTask([point("syn.collect.a", { channel: 1 })])] });
    installFetch(state);
    renderPane();
    await waitFor(() => expect(screen.getByTestId("apply-button").hasAttribute("disabled")).toBe(false));
    // 制造一次真实变更（周期 1000 → 2000），否则保存计划会按 NO_CHANGES 拒绝发 PUT。
    fireEvent.change(screen.getByRole("spinbutton"), { target: { value: "2000" } });
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("2000 ms"),
    );
    await user.click(screen.getByTestId("apply-button"));

    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存，当前停止");
    expect(state.putBodies).toHaveLength(1);
    // 已停止 ⇒ 绝不调用 start。
    expect(state.startCalls).toBe(0);
    expect(state.stopCalls).toBe(0);
  });
});

describe("P0-2：运行中的按钮/勾选/底部说明随动作变化", () => {
  it("running_endpoint_shows_stop_and_apply_with_real_path", async () => {
    const user = userEvent.setup();
    const state = mkState({
      endpointState: "RUNNING",
      tasks: [nckTask([point("syn.collect.a", { channel: 1 })])],
    });
    installFetch(state);
    renderPane();

    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    const button = screen.getByTestId("apply-button");
    expect(button.textContent).toBe("停止并应用");
    const checkbox = screen.getByRole("checkbox") as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    expect(screen.getByTestId("acquisition-execution-note").textContent).toContain(
      `停止 → 写入配置（PUT /tasks/${EP}）→ 启动`,
    );
    expect(screen.getByTestId("acquisition-status-after").textContent).toBe(
      "停止 → 写入配置 → 启动",
    );

    await user.click(checkbox);
    expect(screen.getByTestId("acquisition-execution-note").textContent).toContain(
      "保存后保持停止",
    );
    expect(screen.getByTestId("acquisition-status-after").textContent).toBe(
      "停止 → 写入配置，保存后保持停止",
    );
  });

  it("running_save_with_restart_reports_acquisition_started", async () => {
    const user = userEvent.setup();
    const state = mkState({
      endpointState: "RUNNING",
      tasks: [nckTask([point("syn.collect.a", { channel: 1 })])],
    });
    installFetch(state);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-state").textContent).toBe("运行中"),
    );
    // 同样制造一次真实变更，确保保存计划不会因 NO_CHANGES 提前返回。
    fireEvent.change(screen.getByRole("spinbutton"), { target: { value: "2000" } });
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("2000 ms"),
    );
    await user.click(screen.getByTestId("apply-button"));

    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存，采集已启动");
    expect(state.stopCalls).toBe(1);
    expect(state.startCalls).toBe(1);
    expect(state.putBodies).toHaveLength(1);
  });
});

describe("P2-1：压测信息只出现在帮助提示里", () => {
  it("stress_test_copy_is_not_inline_and_tooltip_has_disclaimer", async () => {
    const user = userEvent.setup();
    const state = mkState({ tasks: [nckTask([point("syn.collect.a", { channel: 1 })])] });
    installFetch(state);
    renderPane();
    await screen.findByTestId("interval-note");
    expect(screen.getByTestId("interval-note").textContent).toContain(
      "实际采集表现以运行结果为准",
    );
    // 输入框旁边不再直接摆压测数字。
    expect(document.body.textContent).not.toContain("50K/s");

    await user.hover(screen.getByTestId("interval-help"));
    const tip = await screen.findByText(/软件压力测试结果不代表真实机床的可用采集周期/);
    expect(tip.textContent).toContain("50K/s");
  });
});

describe("P1-2：点位行两层信息 + 技术详情折叠 + 状态文案", () => {
  it("semantic_point_shows_name_address_and_config_only_status", async () => {
    const state = mkState({
      tasks: [nckTask([point("syn.collect.a", { channel: 1 })])],
    });
    installFetch(state);
    renderPane();

    const row = await screen.findByTestId("nck-point-row");
    await waitFor(() =>
      expect(within(row).getByTestId("nck-point-status").textContent).toContain(
        "配置格式校验通过",
      ),
    );
    expect(within(row).getByTestId("nck-point-name").textContent).toBe("主轴转速");
    expect(within(row).getByTestId("nck-point-address").textContent).toBe("语义点位 · 通道 1");
    // 绿色标签只声明配置格式，不得让用户以为真实机床读取成功。
    const status = within(row).getByTestId("nck-point-status").textContent ?? "";
    expect(status).not.toContain("采集正常");
    expect(status).not.toContain("已读取");
    expect(screen.getByTestId("nck-point-list-hint").textContent).toContain(
      "不代表机床已成功读取数据",
    );
    // 技术细节默认折叠，但 point_key / 完整 JSON / 定义版本都在。
    const tech = within(row).getByTestId("nck-point-technical") as HTMLDetailsElement;
    expect(tech.open).toBe(false);
    expect(tech.textContent).toContain("point_key：syn.collect.a@channel=1");
    expect(tech.textContent).toContain('"channel":1');
    expect(tech.textContent).toContain("定义版本：rev 1");
  });

  it("native_point_is_named_generically_and_shows_structured_address", async () => {
    const state = mkState({ tasks: [nckTask([NATIVE_POINT])] });
    installFetch(state);
    renderPane();

    const row = await screen.findByTestId("nck-point-row");
    await waitFor(() =>
      expect(within(row).getByTestId("nck-point-status").textContent).toContain(
        "配置格式校验通过（原生）",
      ),
    );
    // 硬约束：没有可靠业务名称 ⇒ 只能叫「原生点位」。
    expect(within(row).getByTestId("nck-point-name").textContent).toBe("原生点位");
    expect(within(row).getByTestId("nck-point-address").textContent).toBe("A / 1 / 26 / 30110 / 1");
    expect(row.textContent).not.toMatch(/进给|倍率|主轴转速/);
  });
});
