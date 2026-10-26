// 第 3 步验收：**窄屏**检查（§4.1-1 / §7-5 / §6-15）。
//
// jsdom 没有布局引擎，所以这里只能断言「组件声明了什么」——抽屉收敛到视口宽度、
// 三列表格在窄屏改成单列堆叠、保存区是文档流内的 sticky 而不是 fixed 覆盖层。
// 真实像素级遮挡（保存区是否压住分页、抽屉操作是否可点）**必须人工目视**，
// 已在验收报告 §4 里单列。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { PointDrawer } from "./acquisition/PointDrawer";
import { buildAcquisition238, semanticPointKey } from "../test/acquisitionFixture238";
import { createRequestProbe, installProbedFetch } from "../test/requestProbe";
import { fitDrawerWidth, isNarrowViewport, NARROW_VIEWPORT_MAX } from "../useViewportWidth";
import type { AcquisitionTaskShape } from "../deviceModel";

const NCK_DRIVER = "sinumerik-nck";
const EP = "ep-narrow";

function setViewport(width: number) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    writable: true,
    value: width,
  });
  fireEvent(window, new Event("resize"));
}

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function installFetch(probe: ReturnType<typeof createRequestProbe>, tasks: AcquisitionTaskShape[], state = "STOPPED") {
  installProbedFetch(probe, async (url, init) => {
    const method = (init?.method ?? "GET").toUpperCase();
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [
          { id: EP, driver_id: NCK_DRIVER, connection: { family: "828d" }, runtime: { state } },
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
        definition_digest: "digest-narrow",
      });
    }
    if (url.startsWith("/api/v1/drivers/")) {
      return ok({
        resources: [buildAcquisition238().resource],
        resource_selection_methods: ["manual", "browse"],
      });
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) return ok({ tasks });
    if (method === "PUT" && url.startsWith("/api/v1/tasks/")) {
      const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
      tasks.splice(0, tasks.length, ...body.tasks);
      return ok({ ok: true });
    }
    if (method === "POST" && url.endsWith("/stop")) return ok({ ok: true });
    if (method === "POST" && url.endsWith("/start")) return ok({ ok: true });
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

function renderPane() {
  const fixture = buildAcquisition238();
  return render(
    <EndpointAcquisitionPane
      endpointId={EP}
      driverId={NCK_DRIVER}
      onChanged={() => {}}
      nckDefinitions={fixture.definitions}
    />,
  );
}

const DEFAULT_VIEWPORT = window.innerWidth;

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
  setViewport(DEFAULT_VIEWPORT);
});

describe("窄屏规则（纯函数）：阈值与抽屉收敛", () => {
  it("narrow_threshold_and_drawer_fit", () => {
    expect(NARROW_VIEWPORT_MAX).toBe(768);
    expect(isNarrowViewport(480)).toBe(true);
    expect(isNarrowViewport(768)).toBe(true);
    expect(isNarrowViewport(1280)).toBe(false);
    // 宽度未知（SSR / 首帧）不猜：按宽屏处理。
    expect(isNarrowViewport(0)).toBe(false);
    // 桌面保持 640px；窄屏收敛到视口，绝不超出。
    expect(fitDrawerWidth(640, 1440)).toBe(640);
    expect(fitDrawerWidth(640, 640)).toBe(640);
    expect(fitDrawerWidth(640, 480)).toBe(480);
  });
});

describe("抽屉宽度：桌面 640px，窄屏自适应（无拖拽）", () => {
  function drawerWidth(): string {
    const wrapper = document.querySelector(".ant-drawer-content-wrapper") as HTMLElement | null;
    if (!wrapper) throw new Error("抽屉未渲染");
    return wrapper.style.width;
  }

  it("desktop_keeps_640_and_narrow_follows_viewport", async () => {
    setViewport(1440);
    const { unmount } = render(
      <PointDrawer open title="添加点位" onClose={() => {}}>
        <div data-testid="drawer-body">内容</div>
      </PointDrawer>,
    );
    await waitFor(() => expect(document.querySelector(".ant-drawer-content-wrapper")).not.toBeNull());
    expect(drawerWidth()).toBe("640px");
    // 没有拖拽把手（本期不做宽度调整）。
    expect(document.querySelector(".ant-drawer-resizable-handle")).toBeNull();
    unmount();

    setViewport(480);
    render(
      <PointDrawer open title="添加点位" onClose={() => {}}>
        <div data-testid="drawer-body">内容</div>
      </PointDrawer>,
    );
    await waitFor(() => expect(document.querySelector(".ant-drawer-content-wrapper")).not.toBeNull());
    expect(drawerWidth()).toBe("480px");
  });

  it("pane_drawer_also_adapts_on_narrow_viewport", async () => {
    setViewport(480);
    const probe = createRequestProbe();
    const fixture = buildAcquisition238();
    installFetch(probe, [fixture.task]);
    renderPane();
    const user = userEvent.setup();
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));
    await user.click(screen.getByTestId("add-point-entry"));
    await waitFor(() => expect(document.querySelector(".ant-drawer-content-wrapper")).not.toBeNull());
    expect((document.querySelector(".ant-drawer-content-wrapper") as HTMLElement).style.width).toBe(
      "480px",
    );
  }, 60_000);
});

describe("三列表格：窄屏改单列堆叠（不横向溢出到不可操作）", () => {
  it("stacked_layout_on_narrow_and_columns_on_desktop", async () => {
    setViewport(480);
    const probe = createRequestProbe();
    const fixture = buildAcquisition238();
    installFetch(probe, [fixture.task]);
    const view = renderPane();
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));

    const table = screen.getByTestId("point-table");
    expect(table.getAttribute("data-layout")).toBe("stacked");
    // 窄屏不渲染三列表头（避免三个各占一行的空标签）。
    expect(screen.queryAllByRole("columnheader")).toHaveLength(0);
    // 每行改成单列：名称/标识、摘要、操作三段仍然都在，操作可点。
    const row = screen.getAllByTestId("nck-point-row")[0]!;
    expect((row as HTMLElement).style.gridTemplateColumns).toBe("minmax(0, 1fr)");
    expect(within(row).getByTestId("point-row-title")).toBeTruthy();
    expect(within(row).getByTestId("point-row-summary")).toBeTruthy();
    expect(within(row).getByTestId("point-row-edit")).toBeTruthy();
    expect(within(row).getByTestId("point-row-more")).toBeTruthy();
    // 三个内容块都带可读标签（表头不在了，读屏仍能分辨）。
    expect(within(row).getByLabelText("名称或标识")).toBeTruthy();
    expect(within(row).getByLabelText("配置摘要")).toBeTruthy();
    expect(within(row).getByLabelText("操作")).toBeTruthy();

    // 视口变宽：立刻回到三列（同一次挂载，靠 resize 事件切换）。
    setViewport(1280);
    await waitFor(() => expect(screen.getByTestId("point-table").getAttribute("data-layout")).toBe("columns"));
    expect(
      screen.getAllByRole("columnheader").map((el) => el.textContent),
    ).toEqual(["名称或标识", "配置摘要", "操作"]);
    expect((screen.getAllByTestId("nck-point-row")[0] as HTMLElement).style.gridTemplateColumns).toContain(
      "132px",
    );
    view.unmount();
  }, 60_000);
});

describe("保存区：固定在工作区底部且不遮挡分页（结构与层级）", () => {
  it("save_bar_is_in_flow_sticky_below_the_list_and_below_drawers", async () => {
    setViewport(480);
    const probe = createRequestProbe();
    const fixture = buildAcquisition238();
    installFetch(probe, [fixture.task]);
    renderPane();
    await waitFor(() => expect(screen.getAllByTestId("nck-point-row")).toHaveLength(25));

    // 造出「待保存」态。
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "500" } });
    const bar = await screen.findByTestId("draft-save-bar");
    const style = bar.style;
    // 固定在底部，但**在文档流内**（sticky 而非 fixed）：因此末尾内容不会被永久盖住，
    // 且页面滚到底时保存区自然落到分页之下。
    expect(style.position).toBe("sticky");
    expect(style.bottom).toBe("0px");
    expect(style.position).not.toBe("fixed");
    // 不透明背景：滚动时不会出现「文字叠文字」。
    expect(style.background).not.toBe("");
    // 层级低于抽屉（抽屉 zIndex 1000+），抽屉内的操作不会被保存区压住。
    const barZ = Number(style.zIndex || 0);
    expect(barZ).toBeGreaterThan(0);
    expect(barZ).toBeLessThan(1000);
    // DOM 顺序：分页区域在保存区**之前**，所以保存区不会盖住分页这一块。
    const table = screen.getByTestId("point-table");
    const relation = table.compareDocumentPosition(bar);
    expect(relation & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // 行内「更多」菜单层级高于保存区（否则最后一行的菜单会被盖住）。
    const firstRow = screen.getAllByTestId("nck-point-row")[0]!;
    await userEvent.setup().click(within(firstRow).getByTestId("point-row-more"));
    const menu = await screen.findByTestId("point-row-menu");
    expect(Number(menu.style.zIndex)).toBeGreaterThan(barZ);
  }, 60_000);
});
