// 布局回归：**jsdom 能机械判定**的部分与**必须真实浏览器**的部分分开写清楚。
//
// 用户指出：「布局问题也不是自动化必然测不到，而是当前测试缺少真实浏览器布局覆盖」。
// 因此这里分两类：
// 1. **可机械判定**（本文件断言）：
//    - 列表滚动容器声明的最大高度按视口换算后是否够高（`calc(100vh - 360px)` 逐项算）；
//    - 分页、保存区是否在**滚动容器之外**（DOM 层级，与滚动位置无关）；
//    - 滚动容器是唯一的 `overflow-y: auto` 区域，表格在其内部。
// 2. **必须真实浏览器**（本文件用一条显式断言记录该限制，不假装覆盖）：
//    jsdom 没有布局引擎，`getBoundingClientRect()` 恒为 0——真实溢出、遮挡、层叠、
//    字体度量一律测不到。若将来引入 Playwright/真实浏览器，第一条断言会失败，
//    提醒维护者把这里升级成真实布局断言（而不是继续默默"以为覆盖了"）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { NCK_POINTS_KIND, type AcquisitionTaskShape, type NckPointSelection } from "../deviceModel";
import { NATIVE_RESOURCE, nativePointKey } from "../test/acquisitionFixture238";

const NCK = "sinumerik-nck";
const EP = "ep-layout";

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function nativePoints(count = 30): NckPointSelection[] {
  return Array.from({ length: count }, (_, i) => {
    const parameters = { area: "C", area_no: 1, module: 300 + i, column: 1, line: 1 };
    return {
      kind: "nck_native" as const,
      point_key: nativePointKey({
        area: parameters.area,
        area_no: parameters.area_no,
        module: parameters.module,
        column: parameters.column,
        line: parameters.line,
      }),
      parameters,
    };
  });
}

function installFetch() {
  const task: AcquisitionTaskShape = {
    id: "nck-task",
    schedule: { mode: "poll", interval_ms: 1000 },
    binding: { kind: NCK_POINTS_KIND, config: { points: nativePoints() } },
  };
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string) => {
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [{ id: EP, driver_id: NCK, connection: { family: "828d" }, runtime: { state: "STOPPED" } }],
      });
    }
    if (url.startsWith("/api/v1/drivers/")) return ok({ resources: [NATIVE_RESOURCE] });
    if (url.startsWith("/api/v1/tasks?endpoint=")) return ok({ tasks: [task] });
    return { ok: false, status: 404, json: async () => ({}) };
  });
}

async function renderReady() {
  installFetch();
  render(<EndpointAcquisitionPane endpointId={EP} driverId={NCK} onChanged={() => {}} nckDefinitions={{}} />);
  await waitFor(() => expect(screen.getByTestId("point-table-pagination").textContent).toContain("共 30 项"));
  return screen.getByTestId("point-list-scroll");
}

/** 把声明的 `max-height` 按视口高度换算成 px（只认当前实现的两种形态）。 */
function declaredMaxHeightPx(decl: string, viewportHeight: number): number {
  const calc = /calc\(\s*(\d+(?:\.\d+)?)vh\s*-\s*(\d+(?:\.\d+)?)px\s*\)/.exec(decl);
  if (calc) return (Number(calc[1]) / 100) * viewportHeight - Number(calc[2]);
  const px = /^(\d+(?:\.\d+)?)px$/.exec(decl.trim());
  return px ? Number(px[1]) : Number.NaN;
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("列表高度与分页位置（声明层面，可机械判定）", () => {
  it("list_container_declares_enough_height_for_the_viewport", async () => {
    const list = await renderReady();
    const decl = list.style.maxHeight;
    expect(decl).toBe("calc(100vh - 360px)");
    // 1080p：720px（远高于「一屏能看到多行」的阈值 600）。
    expect(declaredMaxHeightPx(decl, 1080)).toBeGreaterThanOrEqual(600);
    // 720p 笔记本：360px，仍高于阈值 300（列表不会被压成一条缝）。
    expect(declaredMaxHeightPx(decl, 720)).toBeGreaterThanOrEqual(300);
    // 兜底下限：极矮视口仍有 240px 可用高度。
    expect(Number.parseFloat(list.style.minHeight)).toBeGreaterThanOrEqual(240);
  });

  it("pagination_and_save_bar_live_outside_the_scroll_container", async () => {
    const list = await renderReady();
    const pagination = screen.getByTestId("point-table-pagination");
    // DOM 层级：分页不是滚动容器的后代 ⇒ 列表滚动时分页不会被一起滚走。
    expect(list.contains(pagination)).toBe(false);
    expect(screen.getByTestId("point-table").contains(pagination)).toBe(false);
    // 顺序：表格在滚动容器内，分页在其后（同一父级）。
    expect(list.contains(screen.getByTestId("point-table"))).toBe(true);
    expect(
      list.compareDocumentPosition(pagination) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    // 滚动容器是**唯一**的滚动区：只有它声明 overflow-y: auto。
    expect(list.style.overflowY).toBe("auto");
    expect(screen.getByTestId("point-table").style.overflowY).toBe("");

    // 有修改时出现固定保存区，同样不在滚动容器内。
    const { fireEvent } = await import("@testing-library/react");
    fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
    const bar = await screen.findByTestId("draft-save-bar");
    expect(list.contains(bar)).toBe(false);
  });

  it("renders_one_page_of_rows_inside_the_scroll_container", async () => {
    const list = await renderReady();
    const rows = screen.getAllByTestId("nck-point-row");
    expect(rows).toHaveLength(25);
    for (const row of rows) expect(list.contains(row)).toBe(true);
  });
});

describe("必须真实浏览器才能判定的部分（此处显式记录限制，不假装覆盖）", () => {
  it("jsdom_has_no_layout_engine_so_real_overflow_cannot_be_asserted", async () => {
    const list = await renderReady();
    // jsdom 不做布局：高度恒为 0。下面这些「真实溢出/遮挡/层叠」问题**无法**在此判定，
    // 验收报告里按「需真实浏览器」单列：
    // - 480px 视口下抽屉内字段是否横向溢出；
    // - 滚动中间态保存区与分页的视觉重叠；
    // - 行内「⋯」菜单是否被容器裁切；
    // - 真实字号/触摸目标大小。
    expect(list.getBoundingClientRect().height).toBe(0);
    const row = screen.getAllByTestId("nck-point-row")[0]!;
    expect(row.getBoundingClientRect().height).toBe(0);
    expect(list.clientHeight).toBe(0);
  });
});
