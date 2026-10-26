// 238 点离线夹具的页面验收（§5.2 的 1–5、7、8、11 项）。
//
// 全部为**离线/前端**验收：不启动服务、不连真机、不改配置；判定全部可机械核实
// （计数、页码、滚动位置、请求次数、文案）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import {
  buildAcquisition238,
  semanticPointKey,
  type AcquisitionFixture238,
} from "../test/acquisitionFixture238";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-238";

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

interface MockState {
  fixture: AcquisitionFixture238;
  /** 任务快照请求是否失败（问题夹具用）。 */
  tasksGetFails?: boolean;
  /** Descriptor 声明的选型入口（默认 manual + browse；import 用另一条用例验证）。 */
  selectionMethods?: string[];
  putBodies: Array<{ tasks: unknown[] }>;
  /** 最近一次成功写入的任务集（回读桩用：PUT 之后应回显它）。 */
  lastPut?: unknown[] | null;
}

function mkState(overrides: Partial<MockState> = {}): MockState {
  return {
    fixture: buildAcquisition238(),
    selectionMethods: ["manual", "browse"],
    putBodies: [],
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
            runtime: { state: "STOPPED" },
          },
        ],
      });
    }
    if (url.startsWith("/api/v1/drivers/") && url.endsWith("/definitions/identity-preview")) {
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
        resource_selection_methods: state.selectionMethods ?? ["manual"],
      });
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      if (state.tasksGetFails) return { ok: false, status: 500, json: async () => ({}) };
      // 写入之后再读到的是刚写下的任务（回读一致）；否则读初始快照。
      return ok({ tasks: state.lastPut ?? [state.fixture.task] });
    }
    if (url.startsWith("/api/v1/tasks/") && method === "PUT") {
      const body = JSON.parse(String(init?.body)) as { tasks: unknown[] };
      state.putBodies.push(body);
      state.lastPut = body.tasks;
      return ok({ ok: true });
    }
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderPane() {
  return render(
    <EndpointAcquisitionPane
      endpointId={EP}
      driverId={NCK_DRIVER}
      onChanged={() => {}}
      nckDefinitions={buildAcquisition238().definitions}
    />,
  );
}

/** 等 238 点全部核验落定（行锚点，不靠绿色标签）。 */
async function waitAllVerified(total = 238) {
  // 只有当前页渲染在 DOM 里；用「总数」锚点在分页信息上，核验结果看行属性。
  await waitFor(() =>
    expect(screen.getByTestId("point-table-pagination").textContent).toContain(String(total)),
  );
  await waitFor(() =>
    expect(
      document.querySelectorAll('[data-verify-status="pending"]').length,
    ).toBe(0),
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

describe("验收 1：正常首屏安静（无标签墙 / 无教程 / 无逐行绿标 / 无无关警告）", () => {
  it("first_screen_is_quiet_and_only_renders_one_page", async () => {
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);

    // Alert 数 = 0（正常夹具：加载成功、配置完整、身份核验通过）。
    expect(document.querySelectorAll(".ant-alert").length).toBe(0);
    // 无逐行绿色「校验通过」标签、无标签墙、无教程/流程说明。
    expect(document.body.textContent).not.toContain("校验通过");
    expect(screen.queryByTestId("nck-collectable-summary")).toBeNull();
    expect(screen.queryByTestId("nck-catalog-meta")).toBeNull();
    expect(document.body.textContent).not.toContain("选择变量 → 填写必要参数");
    // 238 行不同时渲染：只渲染 25 行（分页 10 页）。
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25);
    const pagination = screen.getByTestId("point-table-pagination").textContent ?? "";
    expect(pagination).toContain("共 238 项");
    expect(pagination).toContain("第 1 / 10 页");
    // 无修改时保存区完全不渲染。
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    // 三列表头（名称或标识 / 配置摘要 / 操作）。
    const headers = screen.getAllByRole("columnheader").map((el) => el.textContent);
    expect(headers).toEqual(["名称或标识", "配置摘要", "操作"]);
  });

  it("page_size_can_switch_to_50_and_100", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);

    await user.click(screen.getByTestId("page-size-50"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(50));
    expect(screen.getByTestId("point-table-pagination").textContent).toContain("第 1 / 5 页");

    await user.click(screen.getByTestId("page-size-100"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(100));
    expect(screen.getByTestId("point-table-pagination").textContent).toContain("第 1 / 3 页");

    // 切回默认 25 项/页。
    await user.click(screen.getByTestId("page-size-25"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));
    expect(screen.getByTestId("point-table-pagination").textContent).toContain("第 1 / 10 页");
  });
});

describe("验收 2：238 点搜索直达（名称 / 地址片段 / 标识片段）", () => {
  it("search_by_name_address_and_identifier_lands_on_page_one", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);

    // 先翻到第 2 页，证明搜索会把结果带回第 1 页。
    await user.click(within(screen.getByTestId("point-table-pagination")).getByTitle("2"));
    await waitFor(() =>
      expect(screen.getByTestId("point-table-pagination").textContent).toContain("第 2 / 10 页"),
    );

    // 名称：轴实际位置
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "轴实际位置" } });
    await waitFor(() => expect(screen.getByTestId("point-table-pagination").textContent).toContain("第 1 / 1 页"));
    let rows = screen.getAllByTestId("nck-point-row");
    expect(rows).toHaveLength(1);
    expect(within(rows[0]!).getByTestId("point-row-title").textContent).toBe("轴实际位置");

    // 地址片段：区域 C · 区域号 1 · 模块 122（原生点位的带标签地址）
    fireEvent.change(screen.getByTestId("point-search"), {
      target: { value: "区域 C · 区域号 1 · 模块 122" },
    });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    rows = screen.getAllByTestId("nck-point-row");
    expect(within(rows[0]!).getByTestId("point-row-summary").textContent).toContain(
      "区域 C · 区域号 1 · 模块 122 · 列 1 · 行 1",
    );

    // 标识片段：native.C.1.122（完整点键只在技术详情，但必须可搜）
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "native.C.1.122" } });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));

    // 不存在串 ⇒ 空态提示（不是一张空表）。
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "不存在的点位" } });
    const empty = await screen.findByTestId("point-table-empty");
    expect(empty.textContent).toContain("没有匹配的点位，可改用标识搜索");
    expect(screen.queryAllByTestId("nck-point-row")).toHaveLength(0);
    // 清除搜索回到完整列表。
    await user.click(screen.getByTestId("clear-point-search"));
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));
  });
});

describe("验收 3：第 10 页编辑后位置不变", () => {
  it("editing_on_last_page_keeps_page_and_scroll", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);

    const pagination = screen.getByTestId("point-table-pagination");
    await user.click(within(pagination).getByTitle("10"));
    await waitFor(() => expect(pagination.textContent).toContain("第 10 / 10 页"));

    // 记录滚动位置（列表区自带滚动容器，位置必须保持）。
    const scroller = screen.getByTestId("point-list-scroll");
    scroller.scrollTop = 240;

    const targetRow = screen
      .getAllByTestId("nck-point-row")
      .find((row) => (row.textContent ?? "").includes("轴实际位置"))!;
    await user.click(within(targetRow).getByTestId("point-row-edit"));
    const input = await screen.findByLabelText("target-channel");
    await user.clear(input);
    await user.type(input, "3");
    await waitFor(() =>
      expect(screen.getByTestId("nck-point-key").textContent).toBe(
        semanticPointKey("axis.position.actual_bcs", { axis: 1, channel: 3 }),
      ),
    );
    await user.click(screen.getByTestId("nck-add-point"));

    // 仍停在第 10 页，滚动位置一致（不需要回到顶部）。
    await waitFor(() => expect(pagination.textContent).toContain("第 10 / 10 页"));
    expect(screen.getByTestId("point-list-scroll").scrollTop).toBe(240);
    // 该行标记「已修改」，保存区出现。
    const marked = screen
      .getAllByTestId("nck-point-row")
      .find((row) => (row.textContent ?? "").includes("轴实际位置"))!;
    expect(within(marked).getByTestId("point-row-status").textContent).toContain("已修改");
    expect(screen.getByTestId("draft-save-bar")).toBeTruthy();
  });
});

describe("验收 4：未操作时不报字段级错误", () => {
  it("no_field_errors_when_nothing_is_touched", async () => {
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);
    // 抽屉未打开时页面没有任何字段级错误。
    expect(document.querySelectorAll(".ant-form-item-explain-error").length).toBe(0);
    expect(document.querySelectorAll('[data-testid="nck-form-issues"]').length).toBe(0);
    expect(document.body.textContent).not.toContain("FORM_MODEL_INVALID");
    // 也未打开添加/编辑抽屉。
    expect(screen.queryByTestId("nck-variable-select")).toBeNull();
  });
});

describe("验收 6/8：变更摘要措辞与「抵消 / 恢复原值」", () => {
  it("summary_separates_point_changes_from_interval_change", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);

    // 改周期 + 移除一个点位 ⇒ 「1 项点位变更（移除 1）；周期另有修改：1000 → 500 ms」。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "500" } });
    await user.click(screen.getAllByTestId("point-row-more")[0]!);
    await user.click(await screen.findByTestId("point-row-remove"));
    const summary = await screen.findByTestId("draft-save-summary");
    expect(summary.textContent).toBe("1 项点位变更（移除 1）；周期另有修改：1000 → 500 ms");
    // 措辞不允许把周期算进点位计数。
    expect(summary.textContent).not.toContain("2 项");

    // 移除后可撤销：撤销 ⇒ 只剩周期修改（点位变更计数消失）。
    await user.click(screen.getByTestId("undo-remove"));
    await waitFor(() =>
      expect(screen.getByTestId("draft-save-summary").textContent).toBe(
        "周期另有修改：1000 → 500 ms",
      ),
    );

    // 周期改回原值 ⇒ 无任何差异，保存区收起（恢复原值取消标记）。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "1000" } });
    await waitFor(() => expect(screen.queryByTestId("draft-save-bar")).toBeNull());
  });

  it("add_then_remove_cancels_out", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);

    // 新增一个语义点位 → 计数「新增 1」，新行标记「新增」。
    await user.click(screen.getByTestId("add-point-entry"));
    await user.selectOptions(
      (await screen.findByTestId("nck-variable-select")) as HTMLSelectElement,
      "channel.feed.actual",
    );
    await user.type(screen.getByLabelText("target-channel"), "4");
    await screen.findByTestId("nck-point-key");
    await user.click(screen.getByTestId("nck-add-point"));
    // 新行落在最后一页：直接看摘要即可（不依赖页码）。
    await waitFor(() =>
      expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）"),
    );

    // 新增后移除 ⇒ 抵消：摘要与保存区一起消失，也不产生「移除 1」。
    // 用新行自己的摘要（通道 4）定位，避免与夹具里既有的同变量点位混淆。
    fireEvent.change(screen.getByTestId("point-search"), { target: { value: "通道 4" } });
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1));
    await user.click(screen.getByTestId("point-row-more"));
    await user.click(await screen.findByTestId("point-row-remove"));
    await waitFor(() => expect(screen.queryByTestId("draft-save-bar")).toBeNull());
    // 全程没有发出任何写请求（只改草稿）。
    expect(state.putBodies).toHaveLength(0);
  });
});

describe("验收 10：未实现入口不出现（声明了 import 也不渲染）", () => {
  it("declared_import_entry_is_hidden_not_disabled", async () => {
    const user = userEvent.setup();
    const state = mkState({ selectionMethods: ["manual", "browse", "import"] });
    installFetch(state);
    renderPane();
    await waitAllVerified(238);
    await user.click(screen.getByTestId("add-point-entry"));
    // 变量选择入口（语义点位）在抽屉里；导入不出现（不是置灰占位）。
    await screen.findByTestId("nck-variable-select");
    expect(screen.queryByText("导入")).toBeNull();
    expect(screen.queryByRole("tab", { name: "导入" })).toBeNull();
    // 浏览入口只由「驱动声明」+「产品实现」共同决定（NCK 已实现 browse）。
    expect(screen.getByTestId("nck-browse-toggle")).toBeTruthy();
  });
});

describe("验收 11：真实问题不被隐藏（加载失败仍显示可处理原因）", () => {
  it("load_failure_shows_reason_and_retry", async () => {
    const state = mkState({ tasksGetFails: true });
    installFetch(state);
    renderPane();
    const alert = await screen.findByTestId("tasks-load-error");
    expect(alert.textContent).toContain("配置加载失败，暂时无法编辑");
    expect(alert.textContent).toContain("任务快照加载失败");
    expect(screen.getByTestId("retry-load")).toBeTruthy();
    // 没有可信快照 ⇒ 没有保存路径。
    expect(screen.queryByTestId("apply-button")).toBeNull();
    expect(state.putBodies).toHaveLength(0);
  });
});

describe("抽屉：未确认修改关闭时提醒一次（草稿生命周期）", () => {
  it("closing_with_unconfirmed_changes_asks_once", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(238);

    await user.click(screen.getByTestId("add-point-entry"));
    await user.selectOptions(
      (await screen.findByTestId("nck-variable-select")) as HTMLSelectElement,
      "channel.feed.actual",
    );
    await user.type(screen.getByLabelText("target-channel"), "7");
    await screen.findByTestId("nck-point-key");

    // 关闭抽屉：先提醒一次（不静默丢弃）。
    await user.click(screen.getByTestId("drawer-cancel"));
    const title = await screen.findAllByText("放弃未确认的修改？");
    expect(title.length).toBeGreaterThan(0);
    // 「继续编辑」⇒ 抽屉仍然开着，输入还在。
    await user.click(screen.getByRole("button", { name: "继续编辑" }));
    expect(screen.getByTestId("nck-variable-select")).toBeTruthy();

    // 「放弃修改」⇒ 抽屉关闭，且没有写入草稿（无点位变更、无写请求）。
    await user.click(screen.getByTestId("drawer-cancel"));
    await user.click(await screen.findByRole("button", { name: "放弃修改" }));
    await waitFor(() => expect(screen.queryByTestId("nck-variable-select")).toBeNull());
    expect(screen.queryByTestId("draft-save-bar")).toBeNull();
    expect(state.putBodies).toHaveLength(0);
  }, 30_000);
});

describe("草稿状态上报（外壳层切换连接/离开页面的依据）", () => {
  it("reports_dirty_and_saving_to_shell", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    const onDraftStateChange = vi.fn();
    render(
      <EndpointAcquisitionPane
        endpointId={EP}
        driverId={NCK_DRIVER}
        onChanged={() => {}}
        nckDefinitions={buildAcquisition238().definitions}
        onDraftStateChange={onDraftStateChange}
      />,
    );
    await waitAllVerified(238);
    // 初始：没有未保存修改、没有保存中。
    expect(onDraftStateChange).toHaveBeenLastCalledWith({ dirty: false, saving: false });

    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "500" } });
    await waitFor(() =>
      expect(onDraftStateChange).toHaveBeenLastCalledWith({ dirty: true, saving: false }),
    );

    await user.click(await screen.findByTestId("apply-button"));
    await waitFor(() => expect(state.putBodies).toHaveLength(1));
    // 保存完成后回到「无未保存修改」。
    await waitFor(() =>
      expect(onDraftStateChange).toHaveBeenLastCalledWith({ dirty: false, saving: false }),
    );
  });
});


