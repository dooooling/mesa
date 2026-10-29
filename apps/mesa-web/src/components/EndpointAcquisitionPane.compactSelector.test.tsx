// 「添加点位」抽屉的页面级验收：统计口径 + 无独立浏览入口 + 无横向滚动声明。
//
// 为什么放在 Endpoint 页而不是组件级：统计文案由页面层根据**完整目录**计算，
// 组件级 `NckPointFields` 只拿到可采集子集，看不到 hold/reference_only 的全貌。
// 这里用离线 mock 驱动真实组件，不启服务、不连真机。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import type { NckDefinitionMap } from "./EndpointAcquisitionPane";
import type { NckDefinitionLike } from "../nckPointModel";
import { NATIVE_RESOURCE } from "../test/acquisitionFixture238";
import { CANDIDATE_AVAILABILITY, READY_AVAILABILITY } from "../test/availabilityFixture";
import type { AcquisitionTaskShape } from "../deviceModel";

const EP = "ep-compact";
const DRIVER = "sinumerik-nck";

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function definition(
  variableId: string,
  label: string,
  availability: unknown,
): NckDefinitionLike {
  return {
    variable_id: variableId,
    revision: 1,
    family: "828d",
    scope: "collect",
    semantics_status: "confirmed",
    label: { zh: label, en: label },
    targets: [],
    target_required: [],
    arguments: [],
    identity_template: "",
    identity_keys: [],
    golden_ref: [],
    digest: `digest-${variableId}`,
    category: "cnc_variable",
    category_basis: "navigation_rule",
    availability,
  };
}

const DEFINITIONS: NckDefinitionMap = {
  "demo.ready": definition("demo.ready", "已核实变量", READY_AVAILABILITY),
  "demo.candidate": definition("demo.candidate", "候选变量", CANDIDATE_AVAILABILITY),
  "demo.hold": definition("demo.hold", "暂缓变量", {
    state: "hold",
    basis: ["no_device_read"],
    reason_zh: "驱动无真机读取证据",
  }),
  "demo.reference": definition("demo.reference", "仅参考变量", {
    state: "reference_only",
    basis: ["identity_only"],
    reason_zh: null,
  }),
  "demo.missing": definition("demo.missing", "可用性缺失变量", undefined),
};

const TASK: AcquisitionTaskShape = {
  id: "nck-task",
  schedule: { mode: "poll", interval_ms: 1000 },
  binding: { kind: "nck.points.v1", config: { points: [] } },
};

function installFetch() {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [
          {
            id: EP,
            driver_id: DRIVER,
            connection: { family: "828d" },
            runtime: { state: "STOPPED" },
          },
        ],
      });
    }
    if (url.endsWith("/descriptor")) {
      return ok({ resources: [NATIVE_RESOURCE], resource_selection_methods: ["manual", "browse"] });
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) return ok({ tasks: [TASK] });
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderPane() {
  return render(
    <EndpointAcquisitionPane
      endpointId={EP}
      driverId={DRIVER}
      onChanged={() => {}}
      nckDefinitions={DEFINITIONS}
    />,
  );
}

async function openDrawer(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByTestId("add-point-entry"));
  await screen.findByTestId("catalog-tree");
}

beforeEach(() => {
  vi.clearAllMocks();
  installFetch();
});

afterEach(() => {
  message.destroy();
});

describe("统计口径：可添加与暂缓分开，hold 不计入可配置", () => {
  it("counts_addable_and_hold_separately_with_consistent_total", async () => {
    const user = userEvent.setup();
    renderPane();
    await openDrawer(user);

    const count = await screen.findByTestId("nck-collectable-count");
    // 五条 collect 定义：ready 1 + candidate 1 可添加；hold 1；reference_only 1；缺失 1。
    expect(count.textContent).toContain(
      "可添加 2 个 · 暂缓 1 个 · 仅参考 1 个 · 可用性未知 1 个",
    );
    expect(count.getAttribute("data-addable")).toBe("2");
    expect(count.getAttribute("data-hold")).toBe("1");
    expect(count.getAttribute("data-reference-only")).toBe("1");
    expect(count.getAttribute("data-unavailable")).toBe("1");
    expect(count.getAttribute("data-total")).toBe("5");
    // 可添加 + 暂缓 + 仅参考 + 未知 = 总数：统计没有漏项/重复。
    expect(2 + 1 + 1 + 1).toBe(5);
    // 明确不得再把 hold 算进"可配置"。
    expect(count.textContent).not.toContain("5 个可配置变量");
    expect(count.textContent).not.toContain("4 个可配置变量");
  });
});

describe("抽屉首屏：无独立浏览入口，技术细节默认折叠", () => {
  it("drawer_has_no_browse_entry_and_no_technical_wall", async () => {
    const user = userEvent.setup();
    renderPane();
    await openDrawer(user);

    // 驱动声明了 browse，但抽屉里不再出现独立浏览操作入口。
    expect(screen.queryByTestId("nck-browse-toggle")).toBeNull();
    expect(screen.queryByText("从浏览树选择语义变量")).toBeNull();
    // 变量选择是默认收起的树形下拉。
    expect(screen.getByTestId("nck-variable-picker-trigger")).toBeTruthy();
    expect(screen.queryByTestId("catalog-tree-panel")).toBeNull();
    expect(screen.queryByTestId("nck-variable-search")).toBeNull();
    // 未选变量：没有技术详情/对象级联/状态墙；添加按钮在场但禁用。
    expect(screen.queryByTestId("nck-tech-details")).toBeNull();
    expect(screen.queryByTestId("nck-object-cascade")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("布局：抽屉内容不声明横向滚动", () => {
  it("drawer_body_and_form_root_do_not_force_horizontal_scroll", async () => {
    const user = userEvent.setup();
    renderPane();
    await openDrawer(user);

    const body = document.querySelector(".ant-drawer-body") as HTMLElement | null;
    expect(body).not.toBeNull();
    expect(body!.style.overflowX).toBe("hidden");

    const root = screen.getByTestId("nck-point-fields-idle");
    expect(root.style.width).toBe("100%");
    expect(root.style.minWidth).toBe("0");

    // 抽屉内任何元素都不得声明 overflow-x: auto/scroll。
    const forced = Array.from(body!.querySelectorAll<HTMLElement>("*")).filter((el) =>
      ["auto", "scroll"].includes(el.style.overflowX),
    );
    expect(forced).toHaveLength(0);
  });
});
