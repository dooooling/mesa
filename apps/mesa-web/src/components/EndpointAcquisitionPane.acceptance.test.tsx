// 第 3 步验收：**238 点离线夹具上的完整操作路径**（评审稿 §5.2 的 11 项）。
//
// 全部为离线/前端验收：不启服务、不连真机、不改配置、不碰生产库。
// 判定一律可机械核实（计数、页码、滚动位置、请求序列、文案），不靠目视。
//
// 夹具分两份（§5.1）：本文件用 `buildAcquisition238()` 的**正常夹具**做「首屏安静」
// 类断言；「真实问题照常出现」的用例在同一个文件里用**显式追加的问题点位**构造，
// 不与正常断言混用（每个用例各自渲染）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { buildAcquisition238, semanticPointKey, type AcquisitionFixture238 } from "../test/acquisitionFixture238";
import { createRequestProbe, installProbedFetch } from "../test/requestProbe";
import { selectVariableViaTreeWith } from "../test/catalogTreePick";
import { pickObject, probeResponse } from "../test/objectCascadePick";
import type { AcquisitionTaskShape, NckPointSelection } from "../deviceModel";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-238";

/**
 * 保留任务：订阅型 + 四个**服务端缺省字段**齐全（`crates/core-types/src/lib.rs:442-450`
 * 的 serde default）。它不属于本次编辑范围，保存时必须**逐字**保留——这也是
 * 「服务端归一化豁免 ②」在本轮的实际形态（前端不省字段、不重排、不放宽比对）。
 */
const PRESERVED_TASK = {
  id: "keep-sub-task",
  schedule: {
    mode: "subscribe",
    publishing_interval_ms: 500,
    sampling_interval_ms: 250,
    queue_size: 10,
    discard_oldest: true,
  },
  binding: { kind: "custom.v0", config: { source: "kept" } },
} as AcquisitionTaskShape;

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function fail(status = 500, body: unknown = {}) {
  return { ok: false, status, json: async () => body };
}

interface MockState {
  fixture: AcquisitionFixture238;
  /** 依次取一个值（最后一个重复）；"fail" = GET /endpoints 失败。 */
  endpointStates: Array<string | "fail">;
  endpointCalls: number;
  tasks: AcquisitionTaskShape[];
  /** 回读桩："server" = 回显 PUT 写入值；"fail" = 500；数组 = 指定值。 */
  readBack: "server" | "fail" | AcquisitionTaskShape[];
  /** PUT 桩：true ⇒ 返回 400 + issues（保存失败用例）。 */
  putRejects: boolean;
  putBodies: AcquisitionTaskShape[][];
  stopCalls: number;
  startCalls: number;
}

function mkState(overrides: Partial<MockState> = {}): MockState {
  const fixture = buildAcquisition238();
  return {
    fixture,
    endpointStates: ["STOPPED"],
    endpointCalls: 0,
    tasks: [fixture.task, PRESERVED_TASK],
    readBack: "server",
    putRejects: false,
    putBodies: [],
    stopCalls: 0,
    startCalls: 0,
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
      const body = JSON.parse(String(init?.body)) as { parameters: Record<string, unknown> };
      const variableId = String(body.parameters.variable_id ?? "");
      const target = (body.parameters.target ?? {}) as Record<string, number>;
      return ok({
        point_key: semanticPointKey(variableId, target),
        identity_mode: "derived",
        scope: "collect",
        variable_id: variableId,
        definition_revision: body.parameters.definition_revision,
        definition_digest: "digest-238",
      });
    }
    if (url.startsWith("/api/v1/drivers/")) {
      return ok({
        resources: [state.fixture.resource],
        resource_selection_methods: ["manual", "browse"],
      });
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      if (state.putBodies.length > 0) {
        if (state.readBack === "fail") return fail();
        if (Array.isArray(state.readBack)) return ok({ tasks: state.readBack });
      }
      return ok({ tasks: state.tasks });
    }
    if (method === "PUT" && url.startsWith("/api/v1/tasks/")) {
      const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
      if (state.putRejects) {
        return fail(400, {
          valid: false,
          issues: [{ path: "tasks[0].binding.config.points[3].parameters.module", code: "OUT_OF_RANGE", message: "module 超出范围" }],
        });
      }
      state.putBodies.push(body.tasks);
      state.tasks = body.tasks;
      return ok({ ok: true });
    }
    if (method === "POST" && url === `/api/v1/endpoints/${EP}/browse`) {
      // 联动选择的按需探测：逐级候选来自驱动契约夹具（夹具只导出首页，后续页为空）。
      const browseBody = JSON.parse(String(init?.body ?? "{}")) as {
        parent?: string;
        cursor?: string;
      };
      if ((browseBody.cursor ?? "") !== "") return ok({ nodes: [], next_cursor: null });
      return probeResponse(browseBody.parent ?? "");
    }
    if (method === "POST" && url === `/api/v1/endpoints/${EP}/stop`) {
      state.stopCalls += 1;
      state.endpointStates = ["STOPPED"];
      state.endpointCalls = 0;
      return ok({ ok: true });
    }
    if (method === "POST" && url === `/api/v1/endpoints/${EP}/start`) {
      state.startCalls += 1;
      state.endpointStates = ["RUNNING"];
      state.endpointCalls = 0;
      return ok({ ok: true });
    }
    return fail(404);
  });
}

function renderPane(state: MockState) {
  return render(
    <EndpointAcquisitionPane
      endpointId={EP}
      driverId={NCK_DRIVER}
      onChanged={() => {}}
      nckDefinitions={state.fixture.definitions}
    />,
  );
}

function pagination(): HTMLElement {
  return screen.getByTestId("point-table-pagination");
}

/** 等 238 点全部核验落定（不依赖绿色标签：按行上的 data-verify-status 判定）。 */
async function waitAllVerified(total = 238) {
  await waitFor(() => expect(pagination().textContent).toContain(String(total)));
  await waitFor(() =>
    expect(document.querySelectorAll('[data-verify-status="pending"]').length).toBe(0),
  );
}

async function gotoPage(page: number) {
  fireEvent.click(within(pagination()).getByTitle(String(page)));
  await waitFor(() =>
    expect(pagination().textContent).toContain(`第 ${page} / 10 页`),
  );
}

function rowWith(text: string): HTMLElement {
  const row = screen.getAllByTestId("nck-point-row").find((el) => (el.textContent ?? "").includes(text));
  if (!row) throw new Error(`找不到包含「${text}」的行`);
  return row;
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

describe("验收 1–3 + 6/7/9/10：238 点完整操作路径（首屏 → 搜索 → 分页 → 第 10 页编辑 → 保存）", () => {
  it("full_path_on_238_toasts_nothing_and_saves_full_snapshot", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane(state);
    await waitAllVerified(238);

    // ---------- 1) 首屏：安静、只有一页 25 行、三列表头 ----------
    expect(document.querySelectorAll(".ant-alert").length).toBe(0);
    expect(document.body.textContent).not.toContain("校验通过");
    // 教程/流程说明不在页面上（只留在抽屉）。
    expect(screen.queryByTestId("nck-collectable-count")).toBeNull();
    expect(screen.queryByTestId("nck-collectable-summary")).toBeNull();
    expect(screen.queryByTestId("nck-catalog-meta")).toBeNull();
    expect(document.body.textContent).not.toContain("选择变量 → 填写必要参数");
    expect(document.querySelectorAll(".ant-form-item-explain-error")).toHaveLength(0);
    expect(screen.queryByTestId("nck-form-issues")).toBeNull();
    expect(document.body.textContent).not.toContain("FORM_MODEL_INVALID");
    // 抽屉未开 ⇒ 变量选择入口（搜索框 + 分类树）不渲染。
    expect(screen.queryByTestId("catalog-tree")).toBeNull();
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25);
    expect(pagination().textContent).toContain("共 238 项");
    expect(pagination().textContent).toContain("第 1 / 10 页");
    expect(
      screen.getAllByRole("columnheader").map((el) => el.textContent),
    ).toEqual(["名称或标识", "配置摘要", "操作"]);
    // 无修改 ⇒ 保存区完全不渲染；无问题 ⇒ 没有「有问题」筛选项。
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(screen.queryByTestId("point-filter")).toBeNull();

    // ---------- 2) 搜索三路：名称 / 地址片段 / 标识片段 ----------
    // 从非首页开始，证明搜索会复位页码，而非仅在首页看起来正确。
    await gotoPage(2);
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "轴实际位置" } });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    expect(pagination().textContent).toContain("第 1 / 1 页");
    expect(within(screen.getAllByTestId("nck-point-row")[0]!).getByTestId("point-row-title").textContent).toBe(
      "轴实际位置",
    );

    fireEvent.change(screen.getByTestId("point-search"), {
      target: { value: "区域 C · 区域号 1 · 模块 122" },
    });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    expect(
      within(screen.getAllByTestId("nck-point-row")[0]!).getByTestId("point-row-summary").textContent,
    ).toContain("区域 C · 区域号 1 · 模块 122 · 列 1 · 行 1");

    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "native.C.1.122" } });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));

    // 配置内容也能搜：参数 JSON 里的 module 值（不是名称也不是标识）。
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "F64_LE" } });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));

    // 不存在的串 ⇒ 空态提示（不是空表）。
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "不存在的点位" } });
    expect((await screen.findByTestId("point-table-empty")).textContent).toContain(
      "没有匹配的点位，可改用标识搜索",
    );
    expect(screen.queryAllByTestId("nck-point-row")).toHaveLength(0);
    await user.click(screen.getByTestId("clear-point-search"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));

    // ---------- 3) 分页 25 / 50 / 100 ----------
    await user.click(screen.getByTestId("page-size-50"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(50));
    expect(pagination().textContent).toContain("第 1 / 5 页");
    await user.click(screen.getByTestId("page-size-100"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(100));
    expect(pagination().textContent).toContain("第 1 / 3 页");
    await user.click(screen.getByTestId("page-size-25"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));
    expect(pagination().textContent).toContain("第 1 / 10 页");

    // ---------- 4) 第 10 页编辑：位置不变 + 行标「已修改」 ----------
    await gotoPage(10);
    const scroller = screen.getByTestId("point-list-scroll");
    scroller.scrollTop = 240;
    await user.click(within(rowWith("轴实际位置")).getByTestId("point-row-edit"));
    // 编辑态：已选对象以「已选」标签呈现（没有可键入的输入框）；重新选择另一台
    // 实际对象后，通道号由驱动载荷自动改写（axis 一级保持不变）。
    expect(screen.getByTestId("nck-object-selected-channel").getAttribute("data-value")).toBe("1");
    expect(screen.getByTestId("nck-object-selected-axis").getAttribute("data-value")).toBe("1");
    fireEvent.click(screen.getByTestId("nck-object-reselect-channel"));
    await pickObject("channel", "通道2");
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        semanticPointKey("axis.position.actual_bcs", { axis: 1, channel: 2 }),
      ),
    );
    // 抽屉内的提交文案是「加入待保存列表 / 确认修改」，不是「保存点位」。
    expect(screen.getByTestId("nck-add-point").textContent).toContain("确认修改");
    await user.click(screen.getByTestId("nck-add-point"));

    await waitFor(() => expect(pagination().textContent).toContain("第 10 / 10 页"));
    expect(screen.getByTestId("point-list-scroll").scrollTop).toBe(240);
    expect(within(rowWith("轴实际位置")).getByTestId("point-row-status").textContent).toContain("已修改");
    expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（修改 1）");

    // ---------- 5) 保存（已停止 ⇒「保存配置」）：全量替换 + 回读一致 ----------
    expect(saveButton().textContent).toContain("保存配置");
    probe.reset();
    await user.click(saveButton());
    const result = await screen.findByTestId("apply-result");
    expect(result.textContent).toContain("配置已保存");

    // 请求顺序：已停止路径仅 PUT → 回读 GET（无 stop / start）。
    expect(probe.writeSequence()).toEqual(["PUT:tasks", "GET:tasks"]);
    expect(state.stopCalls).toBe(0);
    expect(state.startCalls).toBe(0);

    // 请求体仍是全量快照：238 个点位一个不少，未被编辑的点位逐字不变。
    expect(state.putBodies).toHaveLength(1);
    const sent = state.putBodies[0];
    const sentTask = sent.find((task) => task.id === "nck-task")!;
    const sentPoints = (sentTask.binding.config as { points: NckPointSelection[] }).points;
    expect(sentPoints).toHaveLength(238);
    expect(sentPoints.filter((point) => point.kind === "nck_native")).toHaveLength(235);
    expect(sentPoints.slice(0, 235)).toEqual(state.fixture.points.slice(0, 235));
    expect(sentPoints[sentPoints.length - 3]).toEqual({
      kind: "nck_variable",
      point_key: semanticPointKey("axis.position.actual_bcs", { axis: 1, channel: 2 }),
      parameters: {
        variable_id: "axis.position.actual_bcs",
        definition_revision: 1,
        target: { axis: 1, channel: 2 },
        arguments: {},
      },
    });
    // 保留任务逐字保留（含 subscribe 的四个缺省字段，顺序与值都不动）。
    expect(sent.find((task) => task.id === "keep-sub-task")).toEqual(PRESERVED_TASK);

    // 成功只出现一次：保存区一条结果 + 没有 toast。
    expect(screen.getAllByTestId("apply-result")).toHaveLength(1);
    expect(document.querySelectorAll(".ant-message-notice").length).toBe(0);
    // 回读一致后基线刷新、草稿标记清除：动作区收起（不再有保存/放弃按钮）。
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(screen.queryByTestId("discard-draft")).toBeNull();
    expect(screen.getByTestId("acquisition-status-interval").textContent).toContain("（已保存）");
    expect(within(rowWith("轴实际位置")).queryByTestId("point-row-status")).toBeNull();
  }, 60_000);
});


describe("添加：抽屉 → 选中资源 → 填写 → 加入待保存列表 → 回原位置", () => {
  it("add_via_drawer_returns_to_same_page_and_scroll", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane(state);
    await waitAllVerified(238);

    await gotoPage(5);
    screen.getByTestId("point-list-scroll").scrollTop = 160;
    probe.reset();

    await user.click(screen.getByTestId("add-point-entry"));
    // 抽屉只保留唯一一句说明（§4.3）。
    expect(screen.getByTestId("drawer-draft-notice").textContent).toBe(
      "确认后加入页面草稿，保存配置后生效。",
    );
    await selectVariableViaTreeWith(user, "channel.feed.actual");
    // 参数由驱动载荷自动填入（通道 1 已被现有点位占用 ⇒ 选通道 2 形成新身份）。
    await pickObject("channel", "通道2");
    await screen.findByTestId("nck-point-key");
    expect(screen.getByTestId("nck-add-point").textContent).toContain("加入待保存列表");
    await user.click(screen.getByTestId("nck-add-point"));

    // 回到原位置：仍在第 5 页、滚动位置不变；新增只进草稿（零写请求）。
    await waitFor(() => expect(screen.queryByTestId("catalog-tree")).toBeNull());
    expect(pagination().textContent).toContain("第 5 / 10 页");
    expect(screen.getByTestId("point-list-scroll").scrollTop).toBe(160);
    expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）");
    expect(screen.getByTestId("acquisition-status-points").textContent).toContain("239");
    expect(probe.writeSequence()).toEqual([]);
  }, 60_000);
});

describe("筛选：只有确实有问题行时才出现「有问题（N）」", () => {
  it("issue_filter_and_summary_only_when_problem_rows_exist", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const fixture = buildAcquisition238();
    // 问题夹具（§5.1）：追加一个定义目录里不存在的语义点位 ⇒ 身份无法重新核验。
    const broken: NckPointSelection = {
      kind: "nck_variable",
      point_key: "missing.variable@channel=1",
      parameters: { variable_id: "missing.variable", definition_revision: 1, target: { channel: 1 }, arguments: {} },
    };
    const state = mkState({
      fixture,
      tasks: [
        {
          ...fixture.task,
          binding: {
            kind: fixture.task.binding.kind,
            config: { points: [...fixture.points, broken] },
          },
        },
        PRESERVED_TASK,
      ],
    });
    installFetch(state, probe);
    renderPane(state);
    await waitFor(() => expect(pagination().textContent).toContain("239"));
    // 只观察「加载完成之后」的请求（快照加载自带一次只读 GET /tasks）。
    probe.reset();

    // 一条摘要 + 可展开的「技术详情」（查看），问题不被隐藏（§3.3）。
    const alert = await screen.findByTestId("nck-legacy-identity-alert");
    expect(alert.textContent).toContain("1 个点位需要处理");
    expect(within(alert).getByText("技术详情")).toBeTruthy();
    // 问题点位在最后一页：用标识搜索定位（也顺带证明问题行可被搜到）。
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "missing.variable" } });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    // 行内只有一条合并短文案「有问题」，不是标签墙。
    expect(rowWith("missing.variable").getAttribute("data-issue")).toBe("true");
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "" } });

    // 筛选项出现（有问题 1），可切到「只看问题」。
    const filter = await screen.findByTestId("point-filter");
    await user.click(within(filter).getByRole("combobox"));
    await waitFor(() =>
      expect(document.querySelectorAll(".ant-select-item-option").length).toBeGreaterThan(0),
    );
    const options = Array.from(document.querySelectorAll(".ant-select-item-option")).map(
      (el) => el.textContent,
    );
    expect(options).toContain("有问题（1）");
    await user.click(
      Array.from(document.querySelectorAll(".ant-select-item-option")).find(
        (el) => el.textContent === "有问题（1）",
      )!,
    );
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    expect(rowWith("missing.variable")).toBeTruthy();

    // 有未通过核验的点位 ⇒ 保存保持禁用，且没有发出任何写请求。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "500" } });
    await waitFor(() => expect(screen.getByTestId("apply-button")).toBeTruthy());
    expect(saveButton().disabled).toBe(true);
    expect(probe.writeSequence()).toEqual([]);
  }, 60_000);
});

describe("错误恢复：加载 /endpoints 失败 ⇒ 状态未知但草稿仍可编辑", () => {
  it("endpoint_list_failure_keeps_draft_editable_and_sends_no_write", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ endpointStates: ["fail"] });
    installFetch(state, probe);
    renderPane(state);
    await waitAllVerified(238);
    // 只观察「加载完成之后」的请求（快照加载自带一次只读 GET /tasks）。
    probe.reset();
    expect(await screen.findByTestId("acquisition-run-state-unknown")).toBeTruthy();
    expect(screen.getByTestId("acquisition-status-state").textContent).toBe("状态未知");
    expect(screen.getByTestId("refresh-run-state")).toBeTruthy();
    // 238 个点位照常可看、草稿照常可编辑：周期能改，抽屉能开、能输入。
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25);
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "500" } });
    await waitFor(() =>
      expect(screen.getByTestId("draft-save-summary").textContent).toBe(
        "周期另有修改：1000 → 500 ms",
      ),
    );
    await user.click(screen.getByTestId("add-point-entry"));
    await selectVariableViaTreeWith(user, "channel.program.status");
    // 草稿仍可编辑：联动下拉照常渲染（不再是手填框），但端点连接配置读不到 ⇒
    // 探测闸门关闭、原因可见（fail-closed，不是静默失败）。
    expect(screen.getByTestId("nck-object-cascade")).toBeTruthy();
    const select = screen.getByTestId("nck-object-select-channel") as HTMLSelectElement;
    expect(select.disabled).toBe(true);
    expect(screen.getByTestId("nck-cascade-gate").textContent).toContain("需先创建并连接端点");
    // 身份核验需要的 family 读不到 ⇒ 展示可处理原因。
    expect(await screen.findByTestId("nck-family-reject")).toBeTruthy();

    // 保存禁用（含只写配置），并且零写请求。
    expect(saveButton().disabled).toBe(true);
    expect(probe.writeSequence()).toEqual([]);
    expect(state.putBodies).toHaveLength(0);
  }, 60_000);
});

describe("验收 8：保存失败保留修改（草稿计数、保存区、行标记都不清）", () => {
  it("save_failure_keeps_draft_and_row_marker", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ putRejects: true });
    installFetch(state, probe);
    renderPane(state);
    await waitAllVerified(238);

    await gotoPage(10);
    await user.click(within(rowWith("轴实际位置")).getByTestId("point-row-edit"));
    // 编辑：重新选择通道（参数由驱动载荷改写），形成一次真实的「修改」变更。
    fireEvent.click(screen.getByTestId("nck-object-reselect-channel"));
    await pickObject("channel", "通道2");
    await waitFor(() => expect(screen.getByTestId("nck-point-key")).toBeTruthy());
    await user.click(screen.getByTestId("nck-add-point"));
    expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（修改 1）");

    probe.reset();
    await user.click(saveButton());
    const outcome = await screen.findByTestId("save-outcome");
    expect(outcome.getAttribute("data-kind")).toBe("failed");
    expect(outcome.textContent).toContain("写入失败");
    // 后端 issues 原样保留（path / code），但原始值只在**技术详情**里（正文只写可处理说明）。
    const details = await screen.findByTestId("save-issues-details");
    expect(details.textContent).toContain("tasks[0].binding.config.points[3].parameters.module");
    expect(details.textContent).toContain("OUT_OF_RANGE");
    expect(screen.getByTestId("save-issues-lead").textContent ?? "").not.toContain("tasks[0]");

    // §5.2-5：字段级 path ⇒ 自动跳到该行所在页 + 打开抽屉 + 高亮该字段。
    await waitFor(() => expect(pagination().textContent).toContain("第 1 / 10 页"));
    const locatedField = await waitFor(() => {
      const el = document.getElementById("native-field-module");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(locatedField.getAttribute("style") ?? "").toContain("outline");
    expect(
      screen.getAllByTestId("nck-point-row").find((el) => el.getAttribute("data-issue-located") === "true"),
    ).toBeTruthy();

    // 草稿不丢：保存区仍在、计数不变。
    expect(screen.getByTestId("draft-save-bar")).toBeTruthy();
    expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（修改 1）");
    // 未成功 ⇒ 不出现成功结果条。
    expect(screen.queryByTestId("apply-result")).toBeNull();
    expect(probe.writeSequence()).toEqual(["PUT:tasks"]);

    // 定位打开的抽屉里未确认的修改需要一次确认才能关；关掉后回到第 10 页，
    // 行标记必须还在（草稿没有因为保存失败被清掉）。
    await user.click(await screen.findByTestId("drawer-cancel"));
    // 确认框里的「放弃修改」（保存区也有同名按钮，必须限定在 modal 内）。
    const confirm = await waitFor(() => {
      const el = document.querySelector(".ant-modal-confirm");
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    await user.click(within(confirm).getByRole("button", { name: "放弃修改" }));
    await gotoPage(10);
    expect(within(rowWith("轴实际位置")).getByTestId("point-row-status").textContent).toContain("已修改");
  }, 60_000);
});

describe("错误恢复：必填缺失 ⇒ 自动展开高级区并定位到该字段", () => {
  it("missing_required_advanced_field_expands_and_scrolls_to_anchor", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    // 高级区里的必填字段（无默认值）→ 必须自动展开 + 高亮 + 定位。
    const state = mkState({
      tasks: [],
      fixture: {
        ...buildAcquisition238(),
        resource: {
          id: "variable",
          label: { default: "NC Variable" },
          parameters: {
            fields: [
              { key: "area", label: "Area", field_type: "string", required: true, validation: {}, ui: {} },
              {
                key: "transport_size",
                label: "Transport",
                field_type: "string",
                required: true,
                validation: {},
                ui: { advanced: true },
              },
            ],
          },
          outputs: [],
          modes: ["poll"],
        },
      },
    });
    const scrolled: string[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function scrollIntoViewStub(this: Element) {
      scrolled.push((this as HTMLElement).id);
    } as Element["scrollIntoView"];

    try {
      installFetch(state, probe);
      renderPane(state);
      // 空点位 → 空态（没有分页信息可等）。
      expect(await screen.findByTestId("point-table-empty")).toBeTruthy();
      await user.click(screen.getByTestId("add-point-entry"));
      await user.click(await screen.findByRole("button", { name: "高级原生配置" }));

      // 自动展开：缺失的必填高级字段已经渲染出来（折叠态下根本不存在）。
      const anchor = await waitFor(() => {
        const el = document.getElementById("native-field-transport_size");
        expect(el).not.toBeNull();
        return el!;
      });
      // 高亮：字段外框标出（不新增标签）。
      expect(anchor.getAttribute("style") ?? "").toContain("outline");
      // 定位：展开后的下一轮渲染会把它滚到视口中间。
      await waitFor(() => expect(scrolled).toContain("native-field-transport_size"));
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  }, 60_000);
});
