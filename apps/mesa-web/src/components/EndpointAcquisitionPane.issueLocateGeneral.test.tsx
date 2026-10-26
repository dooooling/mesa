// 验收第 5 项（§5.2-5）的**通用资源主路径**回归：字段错误必须落到可编辑字段上。
//
// 为什么要单列这条：`issueLocation.ts:18-22` 已核实——Core 对 NCK 点位的 `parameters`
// 完全不透明，**真实后端的字段级校验主要出现在通用资源（`mesa.resources.v1`）路径**。
// 而旧实现（`EndpointAcquisitionPane.tsx:1645` 附近）只有「NCK + 有 fieldKey」才打开
// 编辑抽屉，通用资源的字段错误一律落到**只读技术详情**：只显示字段名，没有对应控件、
// 没有高亮、也没法就地改。因此通用资源路径必须补齐实现与断言。
//
// 本文件断言（不只看"抽屉开了"）：
// 1. 字段级 path ⇒ **跳页**（第 2 页）+ 打开**编辑表单**（`resource-picker` 编辑态，
//    不是只读技术详情）；
// 2. 该字段在表单里被**高亮**（`outline`）并**滚动到位**（`scrollIntoView`）；
// 3. 字段 key 不在资源 Schema 里 ⇒ **不假装高亮**：打开只读技术详情并明确说明
//    「无法就地编辑」；
// 4. 行级 / 任务级 / 保留任务 / 无法识别 ⇒ 保持既有行为（不定位、不跳转）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import type { AcquisitionTaskShape } from "../deviceModel";

const EP = "ep-generic";
const DRIVER = "s7";
const PAGE2_ROW_INDEX = 27; // 每页 25 项 ⇒ 下标 27 落在第 2 页
// 提交顺序是 `[...preserved, canonical]`（`deviceModel.mergeAcquisitionTasks`）：
// 有一个保留任务时，被编辑的 canonical 任务在 `tasks[1]`，`tasks[0]` 是保留任务。
const EDITABLE_TASK_INDEX = 1;
const PRESERVED_TASK_INDEX = 0;

/** 通用资源：带 3 个参数（一个普通字段、一个高级字段、一个数字字段）。 */
const REGISTER_RESOURCE = {
  id: "register",
  label: { default: "Register" },
  parameters: {
    fields: [
      { key: "axis", label: "轴", field_type: "string", required: false, validation: {}, ui: { order: 1 } },
      { key: "count", label: "数量", field_type: "integer", required: false, validation: {}, ui: { order: 2 } },
      { key: "decode", label: "解码", field_type: "string", required: false, validation: {}, ui: { order: 3, advanced: true } },
    ],
  },
  outputs: [
    { id: "value", label: { default: "Value" }, type_spec: { kind: "fixed", data_type: "F64" }, access: "read" },
  ],
  modes: ["poll"],
};

/** 30 条选择（每页 25 项 ⇒ 2 页）：用来验证「跳页」真的跳到目标行所在页。 */
function selections(count = 30) {
  return Array.from({ length: count }, (_, i) => ({
    resource_id: "register",
    parameters: { axis: "X", count: i },
    outputs: [{ output: "value", point_key: `register.${i}.value` }],
  }));
}

/** 保留任务（不在本页编辑）：验证「保留任务的 path 不跳转」。 */
const PRESERVED: AcquisitionTaskShape = {
  id: "keep-sub-task",
  schedule: { mode: "subscribe", publishing_interval_ms: 500, sampling_interval_ms: 250, queue_size: 10, discard_oldest: true },
  binding: { kind: "custom.v0", config: { source: "kept" } },
};

interface Harness {
  tasks: AcquisitionTaskShape[];
  issues: Array<{ path: string; code: string; message: string }>;
}

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function fail(status: number, body: unknown) {
  return { ok: false, status, json: async () => body };
}

function harness(overrides: Partial<Harness> = {}): Harness {
  const task: AcquisitionTaskShape = {
    id: "generic-task",
    schedule: { mode: "poll", interval_ms: 1000 },
    binding: { kind: "mesa.resources.v1", config: { selections: selections() } },
  };
  return { tasks: [task, PRESERVED], issues: [], ...overrides };
}

function installFetch(h: Harness) {
  (globalThis as { fetch?: unknown }).fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const method = (init?.method ?? "GET").toUpperCase();
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [{ id: EP, driver_id: DRIVER, connection: { family: "s7" }, runtime: { state: "STOPPED" } }],
      });
    }
    if (url.startsWith("/api/v1/drivers/")) return ok({ resources: [REGISTER_RESOURCE] });
    if (url.startsWith("/api/v1/tasks?endpoint=")) return ok({ tasks: h.tasks });
    if (method === "PUT" && url.startsWith("/api/v1/tasks/")) return fail(400, { valid: false, issues: h.issues });
    return fail(404, {});
  });
}

async function renderReady(h: Harness) {
  installFetch(h);
  const view = render(<EndpointAcquisitionPane endpointId={EP} driverId={DRIVER} onChanged={() => {}} />);
  await waitFor(() => expect(screen.getByTestId("point-table-pagination").textContent).toContain("共 30 项"));
  return view;
}

function pagination(): HTMLElement {
  return screen.getByTestId("point-table-pagination");
}

/** 制造一次真实变更（周期 1000 → 2000），让保存按钮可用。 */
async function makeDirtyAndSave(user: ReturnType<typeof userEvent.setup>) {
  fireEvent.change(screen.getByTestId("acquisition-interval-input"), { target: { value: "2000" } });
  const save = (await screen.findByTestId("apply-button")) as HTMLButtonElement;
  await waitFor(() => expect(save.disabled).toBe(false));
  await user.click(save);
}

/**
 * jsdom 没有实现 `Element.prototype.scrollIntoView`，这里装上记录桩，
 * 用来断言"确实滚动到了该字段的锚点"（不是只加了高亮样式）。
 */
const originalScrollIntoView = Element.prototype.scrollIntoView;
const scrolled: string[] = [];
function installScrollStub() {
  scrolled.length = 0;
  Element.prototype.scrollIntoView = function scrollIntoViewStub(this: Element) {
    scrolled.push((this as HTMLElement).id);
  } as Element["scrollIntoView"];
}

beforeEach(() => {
  vi.clearAllMocks();
  installScrollStub();
});

afterEach(() => {
  Element.prototype.scrollIntoView = originalScrollIntoView;
  message.destroy();
});

describe("通用资源字段级 path ⇒ 跳页 + 打开编辑表单 + 字段高亮/滚动", () => {
  it("generic_field_level_issue_opens_the_edit_form_and_highlights_the_field", async () => {
    const user = userEvent.setup();

    await renderReady(
      harness({
        issues: [
          {
            path: `tasks[${EDITABLE_TASK_INDEX}].selections[${PAGE2_ROW_INDEX}].parameters.count`,
            code: "OUT_OF_RANGE",
            message: "count 超出范围",
          },
        ],
      }),
    );
    expect(pagination().textContent).toContain("第 1 / 2 页");

    await makeDirtyAndSave(user);

    // 1) 跳到该行所在页。
    await waitFor(() => expect(pagination().textContent).toContain("第 2 / 2 页"));
    // 2) 打开的是**编辑表单**（就地编辑这一条选择），不是只读技术详情。
    const drawer = await waitFor(() => {
      const el = screen.getByTestId("point-drawer");
      expect(el).toBeTruthy();
      return el;
    });
    expect(drawer.textContent ?? "").toContain("编辑点位");
    expect(screen.getByTestId("resource-select")).toBeTruthy();
    expect(screen.queryByTestId("point-detail-drawer")).toBeNull();
    // 表单按该行预填：资源是该行的资源，参数是该行的参数（不是空表单）。
    expect(drawer.textContent ?? "").toContain("register");
    const countInput = document.querySelector<HTMLInputElement>("#selection-field-count input");
    expect(countInput?.value).toBe(String(PAGE2_ROW_INDEX));
    // 3) 字段高亮 + 滚动到位（复用 fieldFocus 的定位机制）。
    const anchor = await waitFor(() => {
      const el = document.getElementById("selection-field-count");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(anchor.getAttribute("style") ?? "").toContain("outline");
    await waitFor(() => expect(scrolled).toContain("selection-field-count"));
    // 4) 被定位的行有锚点标记（只有这一行）。
    const located = screen
      .getAllByTestId("sel-point-row")
      .filter((el) => el.getAttribute("data-issue-located") === "true");
    expect(located).toHaveLength(1);
    // 正文只写可处理说明，原始 path / code 只在技术详情里。
    const lead = screen.getByTestId("save-issues-lead");
    expect(lead.textContent ?? "").toContain("点此查看第一个");
    expect(lead.textContent ?? "").toContain("高亮字段");
    expect(lead.textContent ?? "").not.toContain("tasks[");
  }, 60_000);

  it("generic_advanced_field_is_expanded_before_highlighting", async () => {
    const user = userEvent.setup();

    await renderReady(
      harness({
        issues: [
          {
            path: `tasks[${EDITABLE_TASK_INDEX}].selections[0].parameters.decode`,
            code: "INVALID_VALUE",
            message: "decode 取值非法",
          },
        ],
      }),
    );
    await makeDirtyAndSave(user);

    // 高级字段默认折叠：必须先展开，锚点才存在（与 NCK 原生表单同一套顺序）。
    const anchor = await waitFor(() => {
      const el = document.getElementById("selection-field-decode");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(anchor.getAttribute("style") ?? "").toContain("outline");
  }, 60_000);

  it("generic_field_missing_from_schema_opens_read_only_detail_and_says_so", async () => {
    const user = userEvent.setup();

    await renderReady(
      harness({
        issues: [
          {
            path: `tasks[${EDITABLE_TASK_INDEX}].selections[0].parameters.not_in_schema`,
            code: "UNKNOWN_FIELD",
            message: "后端报了界面不认识的字段",
          },
        ],
      }),
    );
    await makeDirtyAndSave(user);

    // 字段不在资源 Schema 里 ⇒ 没有对应控件：打开只读技术详情，并**明确说明**不可就地编辑。
    const detail = await waitFor(() => {
      const el = screen.getByTestId("point-detail");
      expect(el).toBeTruthy();
      return el;
    });
    const note = within(detail).getByTestId("point-detail-issue");
    expect(note.getAttribute("data-located-field")).toBe("not_in_schema");
    expect(note.getAttribute("data-in-place-editable")).toBe("false");
    expect(note.textContent ?? "").toContain("无法就地编辑");
    // 不假装高亮：表单没开、也没有任何字段锚点被滚动。
    expect(screen.queryByTestId("resource-select")).toBeNull();
    expect(scrolled.filter((id) => id.startsWith("selection-field-"))).toHaveLength(0);
    // 正文也不撒谎：说"打开技术详情"，不说"高亮字段"。
    const lead = screen.getByTestId("save-issues-lead");
    expect(lead.textContent ?? "").toContain("无法就地编辑");
  }, 60_000);
});

describe("通用资源：不可定位的 path 保持原有行为（不定位、不跳转）", () => {
  it("task_level_path_is_still_unlocatable", async () => {
    const user = userEvent.setup();
    await renderReady(
      harness({ issues: [{ path: `tasks[${EDITABLE_TASK_INDEX}].binding.kind`, code: "UNSUPPORTED_BINDING", message: "kind 不被支持" }] }),
    );
    await makeDirtyAndSave(user);

    const lead = await screen.findByTestId("save-issues-lead");
    expect(lead.textContent ?? "").toContain("无法自动定位");
    expect(screen.queryByTestId("locate-first-issue")).toBeNull();
    // 不跳页（仍在第 1 页）、不开抽屉。
    expect(pagination().textContent).toContain("第 1 / 2 页");
    expect(screen.queryByTestId("resource-select")).toBeNull();
    expect(
      screen.getAllByTestId("sel-point-row").filter((el) => el.getAttribute("data-issue-located") === "true"),
    ).toHaveLength(0);
  }, 60_000);

  it("row_level_path_opens_read_only_detail_not_the_edit_form", async () => {
    const user = userEvent.setup();
    await renderReady(
      harness({
        issues: [{ path: `tasks[${EDITABLE_TASK_INDEX}].selections[0].resource_id`, code: "INVALID_RESOURCE", message: "资源不存在" }],
      }),
    );
    await makeDirtyAndSave(user);

    // 行级（无字段 key）：打开技术详情，不摊开可编辑表单。
    await waitFor(() => expect(screen.getByTestId("point-detail")).toBeTruthy());
    expect(screen.queryByTestId("resource-select")).toBeNull();
    const note = screen.getByTestId("point-detail-issue");
    expect(note.getAttribute("data-located-field")).toBe("");
  }, 60_000);

  it("preserved_task_path_is_still_unlocatable", async () => {
    const user = userEvent.setup();
    await renderReady(
      harness({
        issues: [
          { path: `tasks[${PRESERVED_TASK_INDEX}].binding.config.points[0]`, code: "INVALID_STRUCTURE", message: "保留任务结构错" },
        ],
      }),
    );
    await makeDirtyAndSave(user);

    const lead = await screen.findByTestId("save-issues-lead");
    expect(lead.textContent ?? "").toContain("无法自动定位");
    // 保留任务不构成跳转理由：页码不动、抽屉不开、行不标记。
    expect(pagination().textContent).toContain("第 1 / 2 页");
    expect(screen.queryByTestId("resource-select")).toBeNull();
    expect(screen.queryByTestId("point-detail")).toBeNull();
  }, 60_000);
});
