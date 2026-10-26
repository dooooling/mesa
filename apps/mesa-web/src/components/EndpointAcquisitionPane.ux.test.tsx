// 采集页 UX 回归（第 2 步：紧凑列表 / 抽屉 / 固定保存区）。
//
// 这些断言刻意都落在**用户能看到的文案与按钮**上：
// - 顶部只有一行运行摘要（不再是四段状态条）；
// - 正常行不再重复绿色「校验通过」标签，技术细节收进「⋯ → 技术详情」；
// - 保存区只在需要时出现，按钮文案描述真实动作（不再出现「停止并应用」）；
// - 添加/编辑走抽屉，关闭后回到原列表位置。
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
  /** 覆盖 Descriptor 的 resources（原生字段 Schema 测试用）。 */
  resources?: unknown[];
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
    if (url.startsWith("/api/v1/drivers/")) return ok({ resources: state.resources ?? RESOURCES });
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

/** 制造一次真实变更（周期 1000 → value），使固定保存区出现。 */
async function changeInterval(value: number) {
  await waitFor(() =>
    expect(screen.getByTestId("acquisition-status-interval").textContent).toContain("（已保存）"),
  );
  fireEvent.change(screen.getByTestId("acquisition-interval-input"), {
    target: { value: String(value) },
  });
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
    // P2-2 方案 B：未修改时单值标注「已保存」，草稿不会被误读成当前配置。
    expect(within(status).getByTestId("acquisition-status-interval").textContent).toBe(
      "1000 ms（已保存）",
    );
    await waitFor(() =>
      expect(within(status).getByTestId("acquisition-status-points").textContent).toBe(
        "1 个（已保存）",
      ),
    );
    // 第 2 步：四段状态条收敛为一行，「保存后状态」不再常驻。
    expect(within(status).queryByTestId("acquisition-status-after")).toBeNull();
    // 没有修改 ⇒ 固定保存区完全不渲染（不是禁用）。
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(screen.queryByTestId("apply-button")).toBeNull();

    // 产生真实变更后：按钮文案只承诺「保存配置」，并说明不会启动采集。
    await changeInterval(2000);
    const button = await screen.findByTestId("apply-button");
    expect(button.textContent).toBe("保存配置");
    expect(screen.getByTestId("draft-save-note").textContent).toContain("不会启动采集");
    // 不得再写死旧路径文案。
    expect(document.body.textContent).not.toContain("Stop → PUT");
  });

  it("saving_while_stopped_writes_config_and_reports_currently_stopped", async () => {
    const user = userEvent.setup();
    const state = mkState({ tasks: [nckTask([point("syn.collect.a", { channel: 1 })])] });
    installFetch(state);
    renderPane();
    // 无实际修改 ⇒ 保存区不出现（§4.7 隐藏态）。
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("1000 ms（已保存）"),
    );
    expect(screen.queryByTestId("apply-button")).toBeNull();
    // 制造一次真实变更（周期 1000 → 2000），否则保存计划会按 NO_CHANGES 拒绝发 PUT。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe(
        "1000 ms（已保存） → 2000 ms（待保存）",
      ),
    );
    await user.click(await screen.findByTestId("apply-button"));

    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存，当前停止");
    expect(state.putBodies).toHaveLength(1);
    // 已停止 ⇒ 绝不调用 start。
    expect(state.startCalls).toBe(0);
    expect(state.stopCalls).toBe(0);
  });
});

describe("P0-2：运行中的按钮/次级选项/中断说明随动作变化", () => {
  it("running_endpoint_offers_restart_by_default_and_secondary_keep_stopped", async () => {
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
    // 无修改 ⇒ 保存区不渲染（中断说明也不出现）。
    expect(screen.queryByTestId("draft-save-note")).toBeNull();

    await changeInterval(2000);
    const button = await screen.findByTestId("apply-button");
    // 默认动作是「保存并恢复采集」（不是旧文案「停止并应用」）。
    expect(button.textContent).toBe("保存并恢复采集");
    // 中断说明只在「有修改 + 运行中」时出现，且不承诺时长。
    const note = screen.getByTestId("draft-save-note").textContent ?? "";
    expect(note).toBe("保存会暂时中断当前连接的采集");
    expect(note).not.toMatch(/1[–-]2 ?秒/);

    // 次级选项：切到「保存后保持停止」后主按钮文案随之改变。
    expect(screen.getByTestId("toggle-restart").textContent).toBe("保存后保持停止");
    await user.click(screen.getByTestId("toggle-restart"));
    await waitFor(() =>
      expect(screen.getByTestId("apply-button").textContent).toBe("保存并停止采集"),
    );
    expect(screen.getByTestId("toggle-restart").textContent).toBe("保存后恢复采集");
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
    await changeInterval(2000);
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe(
        "1000 ms（已保存） → 2000 ms（待保存）",
      ),
    );
    await user.click(await screen.findByTestId("apply-button"));

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

describe("P1-2 / 第 2 步：点位行三层信息 + 技术详情抽屉，不再有逐行绿标", () => {
  it("semantic_point_shows_name_and_labeled_summary_without_green_tag", async () => {
    const state = mkState({
      tasks: [nckTask([point("syn.collect.a", { channel: 1 })])],
    });
    installFetch(state);
    renderPane();

    const row = await screen.findByTestId("nck-point-row");
    await waitFor(() => expect(row.getAttribute("data-verify-status")).toBe("verified"));
    expect(within(row).getByTestId("point-row-title").textContent).toBe("主轴转速");
    // 摘要 = 字段标签 + 值（不再带「语义点位 · 」前缀）。
    expect(within(row).getByTestId("point-row-summary").textContent).toBe("通道 1");
    // §6-7：正常行不出现绿色/成功类标签，也不出现任何「校验通过」文案。
    expect(within(row).queryByTestId("point-row-status")).toBeNull();
    expect(document.body.textContent).not.toContain("校验通过");

    // 技术细节收进「⋯ → 技术详情」抽屉（主动展开才看得到）。
    const user = userEvent.setup();
    await user.click(within(row).getByTestId("point-row-more"));
    await user.click(await screen.findByTestId("point-row-detail"));
    const detail = await screen.findByTestId("point-detail");
    expect(detail.textContent).toContain("point_key：syn.collect.a@channel=1");
    expect(detail.textContent).toContain('"channel":1');
    expect(detail.textContent).toContain("定义版本：rev 1");
    expect(detail.textContent).toContain("服务端 identity-preview 重算");
  });

  it("native_point_is_named_generically_and_shows_labeled_address", async () => {
    const state = mkState({ tasks: [nckTask([NATIVE_POINT])] });
    installFetch(state);
    renderPane();

    const row = await screen.findByTestId("nck-point-row");
    await waitFor(() => expect(row.getAttribute("data-verify-status")).toBe("verified"));
    // 硬约束：没有可靠业务名称 ⇒ 只能叫「原生点位」，不得按地址编造业务名。
    expect(within(row).getByTestId("point-row-title").textContent).toBe("原生点位");
    // §2.6-4：地址必须是「字段标签 + 值」，不能是无说明的五段数字。
    expect(within(row).getByTestId("point-row-summary").textContent).toBe(
      "区域 A · 区域号 1 · 模块 26 · 列 30110 · 行 1",
    );
    expect(row.textContent).not.toMatch(/进给|倍率|主轴转速/);
  });
});

// P2-1/P2-2 用：在共享 NATIVE_SCHEMA 上追加一个可选 decode（枚举），
// 用于验证「同一点位只改 decode」也必须清除已保存结论。
const DECODE_SCHEMA = [
  ...NATIVE_SCHEMA,
  {
    key: "decode",
    label: "Decode",
    field_type: "enum",
    required: false,
    default: "U16_LE",
    validation: { enum_options: ["U16_LE", "U16_BE"] },
    ui: {},
  },
];

function nativePointWithDecode(decode: string): NckPointSelection {
  return {
    kind: "nck_native",
    point_key: "native.A.1.26.30110.1",
    parameters: { area: "A", area_no: 1, module: 26, column: 30110, line: 1, decode },
  };
}

function nativeResources() {
  return [
    {
      id: "variable",
      label: { default: "Variable" },
      parameters: { fields: DECODE_SCHEMA },
      outputs: [],
      modes: ["poll"],
    },
  ];
}

describe("P2-2（状态区）：区分「已保存」与「待保存」", () => {
  it("未保存的周期改动只出现在「待保存」一侧，已保存值保持不变", async () => {
    const state = mkState({ tasks: [nckTask([point("syn.collect.a", { channel: 1 })])] });
    installFetch(state);
    renderPane();
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe(
        "1000 ms（已保存）",
      ),
    );

    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe(
        "1000 ms（已保存） → 2000 ms（待保存）",
      ),
    );
    // 尚未保存：不得出现「已保存」结论，服务端也没收到 PUT。
    expect(screen.queryByTestId("apply-result")).toBeNull();
    expect(state.putBodies).toHaveLength(0);
    // 点位没改 ⇒ 仍是已保存值（草稿不会冒充当前配置）。
    expect(screen.getByTestId("acquisition-status-points").textContent).toBe("1 个（已保存）");
  });
});

describe("P2-1（已保存状态）：改同一点位参数后「已保存」必须清除", () => {
  it("保存成功 → 编辑同一点位的 decode → 旧「已保存」提示消失", async () => {
    const user = userEvent.setup();
    const state = mkState({
      tasks: [nckTask([nativePointWithDecode("U16_LE")])],
      resources: nativeResources(),
    });
    installFetch(state);
    renderPane();

    // 先制造一次可保存的变更（周期 1000 → 2000）并保存成功。
    await waitFor(() =>
      expect(screen.getByTestId("acquisition-status-interval").textContent).toBe("1000 ms（已保存）"),
    );
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    await user.click(await screen.findByTestId("apply-button"));
    expect(await screen.findByTestId("apply-result")).toBeTruthy();

    // 编辑同一原生点位，只改 decode：point_key 与所有「键」都不变。
    // 编辑走抽屉（点行「编辑」→ 抽屉立即打开并带入配置）。
    await user.click(within(screen.getByTestId("nck-point-row")).getByTestId("point-row-edit"));
    const nativeForm = await screen.findByTestId("nck-native-fields");
    // 原生表单里的下拉只有两个枚举字段：area、decode（decode 是第 2 个）；
    // rc-select 在 mousedown 时展开候选。
    const decodeCombo = within(nativeForm).getAllByRole("combobox")[1]!;
    fireEvent.mouseDown(decodeCombo);
    const candidates = await screen.findAllByText("U16_BE");
    const option =
      candidates.find((el) => el.classList.contains("ant-select-item-option-content")) ??
      candidates[candidates.length - 1]!;
    await user.click(option);
    await user.click(within(nativeForm).getByTestId("nck-native-add-point"));

    // 参数已变 ⇒ 旧的「已保存」结论必须立即失效（重新出现未保存状态）。
    await waitFor(() => expect(screen.queryByTestId("apply-result")).toBeNull());
    // 该行标记「已修改」（唯一逐行标记），保存区重新出现。
    await waitFor(() =>
      expect(within(screen.getByTestId("nck-point-row")).getByTestId("point-row-status").textContent).toContain(
        "已修改",
      ),
    );
    expect(screen.getByTestId("draft-save-bar")).toBeTruthy();
  });
});
