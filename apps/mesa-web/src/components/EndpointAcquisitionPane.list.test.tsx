// 列表草稿与错误处理：规模相关行为由 acceptance 中的 238 点链路覆盖，
// 此处用两个原生点位与三个语义点位验证交互，避免每个独立行为都核验完整目录。
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
  SEMANTIC_DEFINITIONS,
  semanticPointKey,
  type AcquisitionFixture238,
} from "../test/acquisitionFixture238";
import { selectVariableViaTreeWith } from "../test/catalogTreePick";

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
  const fixture = buildAcquisition238();
  const points = fixture.points.filter((point, index) => index < 2 || point.kind === "nck_variable");
  return {
    fixture: {
      ...fixture,
      points,
      task: { ...fixture.task, binding: { ...fixture.task.binding, config: { points } } },
    },
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
      nckDefinitions={SEMANTIC_DEFINITIONS}
    />,
  );
}

/** 等小夹具的全部点位核验落定（行锚点，不靠绿色标签）。 */
async function waitAllVerified(total: number) {
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

describe("验收 6/8：变更摘要措辞与「抵消 / 恢复原值」", () => {
  it("summary_separates_point_changes_from_interval_change", async () => {
    const user = userEvent.setup();
    const state = mkState();
    installFetch(state);
    renderPane();
    await waitAllVerified(state.fixture.points.length);

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
    await waitAllVerified(state.fixture.points.length);

    // 新增一个语义点位 → 计数「新增 1」，新行标记「新增」。
    await user.click(screen.getByTestId("add-point-entry"));
    await selectVariableViaTreeWith(user, "channel.feed.actual");
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
    await waitAllVerified(state.fixture.points.length);
    await user.click(screen.getByTestId("add-point-entry"));
    // 变量选择入口（语义点位）在抽屉里：唯一入口是搜索框 + 分类树。
    await screen.findByTestId("catalog-tree");
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
    await waitAllVerified(state.fixture.points.length);

    await user.click(screen.getByTestId("add-point-entry"));
    await selectVariableViaTreeWith(user, "channel.feed.actual");
    await user.type(screen.getByLabelText("target-channel"), "7");
    await screen.findByTestId("nck-point-key");

    // 关闭抽屉：先提醒一次（不静默丢弃）。
    await user.click(screen.getByTestId("drawer-cancel"));
    const title = await screen.findAllByText("放弃未确认的修改？");
    expect(title.length).toBeGreaterThan(0);
    // 「继续编辑」⇒ 抽屉仍然开着，输入还在。
    await user.click(screen.getByRole("button", { name: "继续编辑" }));
    expect(screen.getByTestId("catalog-tree")).toBeTruthy();

    // 「放弃修改」⇒ 抽屉关闭，且没有写入草稿（无点位变更、无写请求）。
    await user.click(screen.getByTestId("drawer-cancel"));
    await user.click(await screen.findByRole("button", { name: "放弃修改" }));
    await waitFor(() => expect(screen.queryByTestId("catalog-tree")).toBeNull());
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
        nckDefinitions={SEMANTIC_DEFINITIONS}
        onDraftStateChange={onDraftStateChange}
      />,
    );
    await waitAllVerified(state.fixture.points.length);
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
