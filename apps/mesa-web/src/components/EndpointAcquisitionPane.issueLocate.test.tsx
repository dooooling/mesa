// 验收第 5 项（§5.2-5）页面级回归：保存失败的 `issues[].path` 能否变成**可处理的位置**。
//
// 两类结果必须同时成立：
// 1. 字段级 path ⇒ 跳页 + 打开抽屉 + 高亮字段（可机械断言）；
// 2. 任务级 / 保留任务 / 无法识别的 path ⇒ 明确说「无法自动定位」，**不跳转**。
//
// 夹具刻意小（30 点 / 2 页）：本文件验证的是「path → 行」的换算与交互落点，
// 238 点规模的同一路径由 `EndpointAcquisitionPane.acceptance.test.tsx` 覆盖。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { NCK_POINTS_KIND, type AcquisitionTaskShape, type NckPointSelection } from "../deviceModel";
import { NATIVE_RESOURCE, nativePointKey } from "../test/acquisitionFixture238";

const NCK = "sinumerik-nck";
const EP = "ep-locate";
const PAGE2_ROW_INDEX = 27; // 每页 25 项 ⇒ 下标 27 落在第 2 页

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function fail(status: number, body: unknown) {
  return { ok: false, status, json: async () => body };
}

/** 30 个原生点位（每页 25 项 ⇒ 2 页），点位参数齐全、不制造额外问题。 */
function nativePoints(count = 30): NckPointSelection[] {
  return Array.from({ length: count }, (_, i) => {
    const parameters = {
      area: "C",
      area_no: 1,
      module: 200 + i,
      column: 1,
      line: 1,
      count: 1,
      unit_mode: "current",
      decode: "F64_LE",
      transport_size: "0x09",
    };
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

/** 保留任务（订阅型，不在本页编辑）：用于验证「保留任务的 path 不跳转」。 */
const PRESERVED: AcquisitionTaskShape = {
  id: "keep-sub-task",
  schedule: { mode: "subscribe", publishing_interval_ms: 500, sampling_interval_ms: 250, queue_size: 10, discard_oldest: true },
  binding: { kind: "custom.v0", config: { source: "kept" } },
};

interface Harness {
  tasks: AcquisitionTaskShape[];
  issues: Array<{ path: string; code: string; message: string }>;
}

function installFetch(h: Harness) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = (init?.method ?? "GET").toUpperCase();
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [{ id: EP, driver_id: NCK, connection: { family: "828d" }, runtime: { state: "STOPPED" } }],
      });
    }
    if (url.startsWith("/api/v1/drivers/")) return ok({ resources: [NATIVE_RESOURCE] });
    if (url.startsWith("/api/v1/tasks?endpoint=")) return ok({ tasks: h.tasks });
    if (method === "PUT" && url.startsWith("/api/v1/tasks/")) {
      return fail(400, { valid: false, issues: h.issues });
    }
    return fail(404, {});
  });
}

function harness(overrides: Partial<Harness> = {}): Harness {
  const points = nativePoints();
  const task: AcquisitionTaskShape = {
    id: "nck-task",
    schedule: { mode: "poll", interval_ms: 1000 },
    binding: { kind: NCK_POINTS_KIND, config: { points } },
  };
  return { tasks: [task, PRESERVED], issues: [], ...overrides };
}

/** 渲染并等到 30 点就绪（行已分页、点位都已核验落定）。 */
async function renderReady(h: Harness) {
  installFetch(h);
  const view = render(
    <EndpointAcquisitionPane endpointId={EP} driverId={NCK} onChanged={() => {}} nckDefinitions={{}} />,
  );
  await waitFor(() => expect(screen.getByTestId("point-table-pagination").textContent).toContain("共 30 项"));
  await waitFor(() =>
    expect(document.querySelectorAll('[data-verify-status="pending"]').length).toBe(0),
  );
  return view;
}

function pagination(): HTMLElement {
  return screen.getByTestId("point-table-pagination");
}

async function gotoPage(page: number) {
  fireEvent.click(within(pagination()).getByTitle(String(page)));
  await waitFor(() => expect(pagination().textContent).toContain(`第 ${page} / 2 页`));
}

/** 制造一次真实变更（周期 1000 → 2000），让保存按钮可用。 */
async function makeDirtyAndSave(user: ReturnType<typeof userEvent.setup>) {
  fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
  const save = (await screen.findByTestId("apply-button")) as HTMLButtonElement;
  await waitFor(() => expect(save.disabled).toBe(false));
  await user.click(save);
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

describe("字段级 path ⇒ 跳页 + 打开抽屉 + 字段高亮", () => {
  it("locates_field_level_issue_to_row_page_and_highlights_field", async () => {
    const user = userEvent.setup();
    const scrolled: string[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function scrollIntoViewStub(this: Element) {
      scrolled.push((this as HTMLElement).id);
    } as Element["scrollIntoView"];

    try {
      const h = harness({
        issues: [
          {
            path: `tasks[0].binding.config.points[${PAGE2_ROW_INDEX}].parameters.module`,
            code: "OUT_OF_RANGE",
            message: "module 超出范围",
          },
        ],
      });
      await renderReady(h);
      // 初始在第 1 页。
      expect(pagination().textContent).toContain("第 1 / 2 页");

      await makeDirtyAndSave(user);

      // 1) 跳到该行所在页（不是第 1 页）。
      await waitFor(() => expect(pagination().textContent).toContain("第 2 / 2 页"));
      // 2) 打开抽屉（原生点位直接在原生页打开）。
      expect(await screen.findByTestId("nck-native-fields")).toBeTruthy();
      // 3) 高亮该字段 + 定位（复用 NckNativeFields 的展开+定位机制）。
      const anchor = await waitFor(() => {
        const el = document.getElementById("native-field-module");
        expect(el).not.toBeNull();
        return el!;
      });
      expect(anchor.getAttribute("style") ?? "").toContain("outline");
      await waitFor(() => expect(scrolled).toContain("native-field-module"));
      // 4) 被定位的行有锚点标记（只有这一行）。
      const located = screen
        .getAllByTestId("nck-point-row")
        .filter((el) => el.getAttribute("data-issue-located") === "true");
      expect(located).toHaveLength(1);

      // 原始 path / code 只在技术详情里；正文只写可处理说明。
      const lead = screen.getByTestId("save-issues-lead");
      expect(lead.textContent ?? "").toContain("点此查看第一个");
      expect(lead.textContent ?? "").not.toContain("tasks[");
      const details = screen.getByTestId("save-issues-details");
      expect(details.textContent ?? "").toContain("tasks[0].binding.config.points[27].parameters.module");
      expect(details.textContent ?? "").toContain("OUT_OF_RANGE");
      // 保存区结果文案同样不含原始 path（正文与详情分离）。
      expect(screen.getByTestId("save-outcome").textContent ?? "").not.toContain("tasks[");
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  }, 60_000);
});

describe("不可定位 ⇒ 明确说明且不跳转", () => {
  it("task_level_path_is_reported_as_unlocatable_without_navigation", async () => {
    const user = userEvent.setup();
    await renderReady(
      harness({ issues: [{ path: "tasks[0].binding.kind", code: "UNSUPPORTED_BINDING", message: "kind 不被支持" }] }),
    );
    // 先停在第 2 页：如果实现「猜一行跳过去」，页码就会变。
    await gotoPage(2);
    await makeDirtyAndSave(user);

    const lead = await screen.findByTestId("save-issues-lead");
    expect(lead.textContent ?? "").toContain("无法自动定位");
    expect(lead.textContent ?? "").toContain("整个任务");
    // 不跳转：页码不动、抽屉不开、没有行被标记。
    expect(pagination().textContent).toContain("第 2 / 2 页");
    expect(screen.queryByTestId("nck-native-fields")).toBeNull();
    expect(screen.queryByTestId("locate-first-issue")).toBeNull();
    expect(
      screen.getAllByTestId("nck-point-row").filter((el) => el.getAttribute("data-issue-located") === "true"),
    ).toHaveLength(0);
    // 详情仍在（可排查）。
    expect(screen.getByTestId("save-issues-details").textContent ?? "").toContain("UNSUPPORTED_BINDING");
  }, 60_000);

  it("preserved_task_path_is_reported_as_unlocatable_without_navigation", async () => {
    const user = userEvent.setup();
    await renderReady(
      harness({
        issues: [{ path: "tasks[1].binding.config.points[0]", code: "INVALID_STRUCTURE", message: "保留任务结构错" }],
      }),
    );
    await gotoPage(2);
    await makeDirtyAndSave(user);

    const lead = await screen.findByTestId("save-issues-lead");
    expect(lead.textContent ?? "").toContain("无法自动定位");
    expect(lead.textContent ?? "").toContain("保留任务");
    expect(pagination().textContent).toContain("第 2 / 2 页");
    expect(screen.queryByTestId("nck-native-fields")).toBeNull();
  }, 60_000);

  it("unrecognized_path_is_reported_as_unlocatable_without_navigation", async () => {
    const user = userEvent.setup();
    await renderReady(
      // 下标越界（本次提交只有 30 个点位）：不得「按最近的行动」猜一个。
      harness({
        issues: [{ path: "tasks[0].binding.config.points[999].parameters.module", code: "OUT_OF_RANGE", message: "越界" }],
      }),
    );
    await makeDirtyAndSave(user);

    const lead = await screen.findByTestId("save-issues-lead");
    expect(lead.textContent ?? "").toContain("无法自动定位");
    expect(pagination().textContent).toContain("第 1 / 2 页");
    expect(screen.queryByTestId("nck-native-fields")).toBeNull();
  }, 60_000);
});
