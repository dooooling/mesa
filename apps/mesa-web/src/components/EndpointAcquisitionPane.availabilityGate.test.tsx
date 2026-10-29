// 流程级验收：**sidecar 元数据消费 + 可用性门禁**（真实渲染 + 请求探针）。
//
// 为什么必须是流程级而不是纯模型单测：`definitionMeta.ts` 的判据（单位是否核实、
// 四态门禁、缺失即 fail-closed）在纯函数层已被单测钉死，但单测不能证明
// "界面上真的按这条判定走"——中间还隔着目录解析（`nckCatalog.parseDefinitionEntry`
// 的容错）、建树（`catalogTree`）、表单状态机（`NckPointFields` 的预览/提交双判定）
// 与草稿层。任一环节接线错位（例如树上写了 hold、表单却放行添加），纯模型单测全绿
// 而用户实际能采到不该采的变量。本文件因此只做一件事：用离线 mock 驱动真实组件，
// 断言**交互路径**、**可见文案**与**请求数**。
//
// 七条流程与验收表逐条对应：
//   1 hold 条目：可见 + 原因可见 + 无法添加（零 PUT）
//   2 reference_only：可见 + 不可作为普通点位添加（零 PUT）
//   3 candidate：可走流程 + 添加处有"未验证"标注（请求正常）
//   4 unit.unknown：显示"单位未核实"，不显示假单位
//   5 unit.verified：显示真实单位值
//   6 availability 缺失：fail-closed 阻断且原因可见
//   7 ready + 无对应对象：静态可用性与动态对象分开显示，缺参数明确阻断
//
// 参数来源：语义页**没有** Target 手填框，参数由"选中实际对象"时的驱动载荷自动填入
// （探测桩服务驱动契约夹具，见 `src/test/objectCascadePick.ts`）。
//
// 约束（与验收口径一致）：
// - 纯离线：不启服务、不连真机、不改配置、不碰生产库；全部请求由本文件 mock；
// - 判定可机械核实：DOM 文本、`data-*` 属性、按钮可用性、请求计数；
// - 通用组件零厂商判断：本文件只喂数据，不依赖任何前端厂商分支。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { createRequestProbe, installProbedFetch } from "../test/requestProbe";
import { NATIVE_RESOURCE } from "../test/acquisitionFixture238";
import { READY_AVAILABILITY } from "../test/availabilityFixture";
import { openCascade, pickObject, probeResponse } from "../test/objectCascadePick";
import { expandAllCatalogGroups } from "../test/catalogTreePick";

const EP = "ep-availability";
const DRIVER = "sinumerik-nck";
const FAMILY = "828d";

// ---------------------------------------------------------------------------
// 1. 目录：六条定义，覆盖四态 + 缺失 + 单位两种形态
// ---------------------------------------------------------------------------

/** 一条 I-2 目录定义的完整 14 字段形态（缺一个字段 `nckCatalog` 会整体拒绝整页）。 */
function definitionOf(overrides: Record<string, unknown>): Record<string, unknown> {
  return {
    revision: 1,
    family: FAMILY,
    scope: "collect",
    semantics_status: "confirmed",
    label: { zh: "", en: "" },
    source: [{ kind: "manual", reference: "可用性门禁流程测试合成条目" }],
    targets: [],
    target_required: [],
    arguments: [],
    identity_template: "",
    identity_keys: [],
    golden_ref: [],
    digest: "digest-synthetic",
    category: "cnc_variable",
    category_basis: "navigation_rule",
    ...overrides,
  };
}

/**
 * 需要 channel 实例的语义变量（这样"选对象 → 填参 → 预览"这条链在每条流程里都成立）。
 *
 * 变量 id 取自驱动夹具的 `identity.applies_to_variables`：候选归属由驱动声明决定，
 * 前端不按名字猜（见 `objectCascade.nodeBelongsToVariable`）。
 */
function channelDef(
  variableId: string,
  label: string,
  extra: Record<string, unknown>,
): Record<string, unknown> {
  return definitionOf({
    variable_id: variableId,
    label: { zh: label, en: label },
    targets: ["channel"],
    target_required: ["channel"],
    identity_keys: [{ path: "channel", required: true }],
    identity_template: `${variableId}@channel=i:{channel}`,
    digest: `digest-${variableId}`,
    ...extra,
  });
}

/** `hold`：暂缓 + 手册位置 + 已核实单位（流程 1 同时验证"原因可见"与"位置可查"）。 */
const HOLD_DEF = channelDef("channel.feed.commanded", "进给速度（暂缓）", {
  availability: {
    state: "hold",
    basis: ["no_device_read", "manual_unverified"],
    reason_zh: "驱动无真机读取证据：手册未确认该变量在 828D 上可读",
  },
  manual_location: {
    document_number: "A5E53370586B AC",
    version: "03/2025",
    section: "5.3.16",
    page: 272,
    source_ref: "manual#5.3.16",
  },
  unit: { value: "mm/min, inch/min, user defined", source_ref: "manual#5.3.16", status: "verified" },
});

/** `reference_only`：只作展示，不可作为普通采集点添加。 */
const REFERENCE_DEF = channelDef("channel.program.block", "参考变量", {
  availability: {
    state: "reference_only",
    basis: ["identity_only"],
    reason_zh: "仅用于身份核对，不参与采集",
  },
});

/** `candidate`：可走流程，但添加处必须标注"未经真机读取验证"（流程 3/5）。 */
const CANDIDATE_DEF = channelDef("channel.feed.actual", "进给速度", {
  availability: { state: "candidate", basis: ["no_device_read"], reason_zh: "尚未在真机读取" },
  unit: { value: "mm/min, inch/min, user defined", source_ref: "manual#5.3.16", status: "verified" },
});

/** `unit.status = unknown`：必须显示"单位未核实"，不得显示假单位（流程 4）。 */
const UNIT_UNKNOWN_DEF = channelDef("channel.program.status", "程序状态", {
  availability: { state: "candidate", basis: ["no_device_read"], reason_zh: null },
  unit: { value: null, source_ref: null, status: "unknown" },
});

/** 完全没有 `availability` 字段：fail-closed 阻断且原因可见（流程 6）。 */
const NO_AVAILABILITY_DEF = channelDef("channel.program.name.main", "无可用性声明", {
  unit: { value: "mm", source_ref: "manual", status: "verified" },
});

/** `ready`：静态可用性已核实；本机是否真有对象由探测回答（流程 7）。 */
const READY_DEF = channelDef("channel.feed.override", "已核实变量", {
  availability: READY_AVAILABILITY,
});

const CATALOG = [
  HOLD_DEF,
  REFERENCE_DEF,
  CANDIDATE_DEF,
  UNIT_UNKNOWN_DEF,
  NO_AVAILABILITY_DEF,
  READY_DEF,
];

// ---------------------------------------------------------------------------
// 2. 请求桩：目录 / 预览 / 探测 / 任务快照
// ---------------------------------------------------------------------------

interface FlowState {
  tasks: unknown[];
  putBodies: unknown[][];
  previewLog: Array<Record<string, unknown>>;
  browseLog: string[];
  /** true ⇒ 探测一律返回空页（流程 7：探测成功但本机没有对象）。 */
  browseEmpty: boolean;
}

function mkState(overrides: Partial<FlowState> = {}): FlowState {
  return { tasks: [], putBodies: [], previewLog: [], browseLog: [], browseEmpty: false, ...overrides };
}

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function fail(status: number, body: unknown) {
  return { ok: false, status, json: async () => body };
}

/** 服务端身份预览的 Key 生成（**测试桩**，模拟服务端权威计算；前端不产生该值）。 */
function pointKeyOf(parameters: Record<string, unknown>): string {
  const variableId = String(parameters.variable_id ?? "");
  const target = (parameters.target ?? {}) as Record<string, unknown>;
  const parts = Object.keys(target)
    .sort()
    .map((key) => `${key}=i:${String(target[key])}`);
  return `${variableId}@${parts.join(",")}`;
}

function installFetch(state: FlowState, probe: ReturnType<typeof createRequestProbe>) {
  return installProbedFetch(probe, async (url, init) => {
    const method = (init?.method ?? "GET").toUpperCase();
    if (url === "/api/v1/endpoints") {
      return ok({
        endpoints: [
          {
            id: EP,
            driver_id: DRIVER,
            connection: { family: FAMILY },
            runtime: { state: "STOPPED" },
          },
        ],
      });
    }
    // 预览与目录同前缀：先分流预览。
    if (url.endsWith("/definitions/identity-preview")) {
      const body = JSON.parse(String(init?.body)) as {
        parameters: Record<string, unknown>;
      };
      state.previewLog.push(body as unknown as Record<string, unknown>);
      return ok({
        point_key: pointKeyOf(body.parameters),
        identity_mode: "derived",
        scope: "collect",
        variable_id: body.parameters.variable_id,
        definition_revision: body.parameters.definition_revision,
        definition_digest: `digest-${String(body.parameters.variable_id)}`,
      });
    }
    if (method === "GET" && url.includes("/definitions")) {
      return ok({
        format_version: 1,
        family: FAMILY,
        revision: 1,
        driver_id: DRIVER,
        driver_version: "0.0.0-availability-test",
        note: null,
        total: CATALOG.length,
        limit: 100,
        next_cursor: null,
        definitions: CATALOG,
      });
    }
    if (url.endsWith("/descriptor")) {
      return ok({ resources: [NATIVE_RESOURCE], resource_selection_methods: [] });
    }
    if (method === "POST" && url === `/api/v1/endpoints/${EP}/browse`) {
      const body = JSON.parse(String(init?.body ?? "{}")) as { parent?: string; cursor?: string };
      state.browseLog.push(body.parent ?? "");
      // 流程 7：驱动如实返回"该位置没有对象"（空页 ≠ 失败）。
      if (state.browseEmpty) return ok({ nodes: [], next_cursor: null });
      // 其余流程用驱动契约夹具的真实通道树（夹具只导出首页，后续页为空）。
      if ((body.cursor ?? "") !== "") return ok({ nodes: [], next_cursor: null });
      return probeResponse(body.parent ?? "");
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      return ok({ tasks: state.tasks });
    }
    if (method === "PUT" && url.startsWith("/api/v1/tasks/")) {
      const body = JSON.parse(String(init?.body)) as { tasks: unknown[] };
      state.putBodies.push(body.tasks);
      state.tasks = body.tasks;
      return ok({ ok: true });
    }
    return fail(404, {});
  });
}

function renderPane() {
  return render(
    <EndpointAcquisitionPane endpointId={EP} driverId={DRIVER} onChanged={() => {}} />,
  );
}

async function waitPaneReady() {
  await screen.findByTestId("add-point-entry", undefined, { timeout: 10_000 });
}

async function openDrawer(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("add-point-entry"));
  await screen.findByTestId("catalog-tree");
  // 变量下拉默认收起：打开面板并展开分组，才能看到变量行。
  await user.click(screen.getByTestId("nck-variable-picker-trigger"));
  await screen.findByTestId("catalog-tree-panel");
  expandAllCatalogGroups();
}

/** 展开「技术详情」（默认折叠：手册位置 / 索引规则 / 静态-动态说明都在里面）。 */
async function expandTechDetails() {
  fireEvent.click(screen.getByText(/技术详情/));
  await screen.findByTestId("nck-tech-details-body");
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

// ---------------------------------------------------------------------------
// 流程 1：hold —— 树中可见、原因可见、可预览但**无法添加**
// ---------------------------------------------------------------------------

describe("流程 1：hold 条目可见、显示 reason_zh、且无法添加", () => {
  it("hold_is_visible_with_reason_and_cannot_be_added", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // ---- 树上：可见 + 简短可用状态；**不铺暂缓长文/单位/手册**（避免撑乱列表）----
    const leaf = screen.getByTestId("catalog-tree-leaf-channel.feed.commanded");
    expect(leaf.getAttribute("data-availability")).toBe("hold");
    const tag = screen.getByTestId("catalog-tree-leaf-availability-channel.feed.commanded");
    expect(tag.getAttribute("data-availability")).toBe("hold");
    expect(tag.textContent).toBe("暂缓");
    expect(leaf.textContent).not.toContain("驱动无真机读取证据");
    expect(screen.queryByTestId("catalog-tree-leaf-hold-reason-channel.feed.commanded")).toBeNull();
    expect(screen.queryByTestId("catalog-tree-leaf-manual-channel.feed.commanded")).toBeNull();
    expect(screen.queryByTestId("catalog-tree-leaf-unit-channel.feed.commanded")).toBeNull();

    // ---- 选中：表单里原因才可见（按需），门禁阻断 ----
    await user.click(leaf);
    expect((await screen.findByTestId("nck-availability")).getAttribute("data-state")).toBe(
      "hold",
    );
    expect(screen.getByTestId("nck-availability-hold-reason").textContent).toContain(
      "驱动无真机读取证据：手册未确认该变量在 828D 上可读",
    );
    const block = screen.getByTestId("nck-availability-block");
    expect(block.getAttribute("data-code")).toBe("AVAILABILITY_HOLD");
    expect(block.textContent).toContain("AVAILABILITY_HOLD");
    expect(block.textContent).toContain("驱动无真机读取证据");
    // 手册位置进「技术详情」：默认折叠不渲染，展开后可查（不占主视觉）。
    expect(screen.queryByTestId("nck-manual-location")).toBeNull();
    await expandTechDetails();
    expect(screen.getByTestId("nck-manual-location").getAttribute("data-page")).toBe("272");

    // ---- 查看与配置仍可用（参数由驱动载荷自动填入），但**添加按钮不可用** ----
    await pickObject("channel", "通道1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "channel.feed.commanded@channel=i:1",
    );
    const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    // 程序化点击也不得越过门禁（不信任 UI 布尔值）。
    fireEvent.click(add);
    await waitFor(() => expect(screen.getByTestId("catalog-tree")).toBeTruthy());
    expect(screen.queryByTestId("draft-save-summary")).toBeNull();
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 流程 2：reference_only —— 可见，但不可作为普通采集点添加
// ---------------------------------------------------------------------------

describe("流程 2：reference_only 条目可见但不可作为普通点位添加", () => {
  it("reference_only_is_visible_and_blocked_from_adding", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    const leaf = screen.getByTestId("catalog-tree-leaf-channel.program.block");
    expect(leaf.getAttribute("data-availability")).toBe("reference_only");
    expect(
      screen.getByTestId("catalog-tree-leaf-availability-channel.program.block").textContent,
    ).toBe("仅参考");

    await user.click(leaf);
    expect((await screen.findByTestId("nck-availability")).getAttribute("data-state")).toBe(
      "reference_only",
    );
    const block = screen.getByTestId("nck-availability-block");
    expect(block.getAttribute("data-code")).toBe("AVAILABILITY_REFERENCE_ONLY");
    expect(block.textContent).toContain("仅作展示");
    expect(block.textContent).toContain("仅用于身份核对");

    // 预览照常可用（只读展示），但添加被阻断且零写请求。
    await pickObject("channel", "通道1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "channel.program.block@channel=i:1",
    );
    const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    fireEvent.click(add);
    await waitFor(() => expect(screen.getByTestId("catalog-tree")).toBeTruthy());
    expect(screen.queryByTestId("draft-save-summary")).toBeNull();
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 流程 3：candidate —— 可走流程，但添加处必须标注"未经真机读取验证"
// ---------------------------------------------------------------------------

describe("流程 3：candidate 可走流程，但添加处标注未验证", () => {
  it("candidate_can_be_added_but_is_labelled_unverified", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    const leaf = screen.getByTestId("catalog-tree-leaf-channel.feed.actual");
    expect(leaf.getAttribute("data-availability")).toBe("candidate");
    expect(
      screen.getByTestId("catalog-tree-leaf-availability-channel.feed.actual").textContent,
    ).toBe("候选");

    await user.click(leaf);
    // 静态结论 + 未验证标注（两处：状态行 + 添加按钮旁）。
    expect((await screen.findByTestId("nck-availability")).getAttribute("data-state")).toBe(
      "candidate",
    );
    expect(screen.getByTestId("nck-availability-candidate-notice").textContent).toContain(
      "未经真机读取验证",
    );
    const addNotice = screen.getByTestId("nck-add-unverified-notice");
    expect(addNotice.textContent).toContain("未经真机读取验证");
    // candidate 不产生阻断告警（允许走流程 ≠ 伪装成已验证）。
    expect(screen.queryByTestId("nck-availability-block")).toBeNull();

    await pickObject("channel", "通道1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "channel.feed.actual@channel=i:1",
    );
    const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(false);
    await user.click(add);

    // 请求路径正常：先预览，添加只进草稿（零 PUT）。
    await waitFor(() =>
      expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）"),
    );
    expect(probe.count("PREVIEW")).toBeGreaterThanOrEqual(1);
    expect(state.previewLog[0]).toEqual({
      family: FAMILY,
      kind: "nck_variable",
      parameters: {
        variable_id: "channel.feed.actual",
        definition_revision: 1,
        target: { channel: 1 },
        arguments: {},
      },
    });
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 流程 4：unit.unknown —— 显示"单位未核实"，不显示假单位
// ---------------------------------------------------------------------------

describe("流程 4：unit 未知时显示“单位未核实”，不显示假单位", () => {
  it("unknown_unit_shows_explicit_text_and_never_a_fake_unit", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    installFetch(mkState(), probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 变量行不显示单位细节（只显示名称 + 简短可用状态）。
    const leaf = screen.getByTestId("catalog-tree-leaf-channel.program.status");
    expect(leaf.textContent).not.toContain("单位");
    expect(screen.queryByTestId("catalog-tree-leaf-unit-channel.program.status")).toBeNull();

    await user.click(leaf);
    const details = screen.getByTestId("nck-variable-details") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    const unitTag = await screen.findByTestId("nck-unit");
    expect(unitTag.getAttribute("data-unit-status")).toBe("unknown");
    expect(unitTag.textContent).toBe("单位未核实");
    // 任何"看起来像单位"的取值都不得出现在**这条定义的元数据行里**（不补默认值、不猜）。
    // 断言范围限定在该行：树上其它条目（单位已核实的那条）照常显示真实单位，
    // 那是它自己的 sidecar 结论，与本条无关。
    const metaRow = within(screen.getByTestId("nck-definition-meta-row"));
    expect(metaRow.queryByText(/mm\/min/)).toBeNull();
    expect(metaRow.queryByText(/\bmm\b/)).toBeNull();
    // 该行里只有一个单位标记，且它就是"单位未核实"。
    expect(metaRow.getAllByTestId("nck-unit")).toHaveLength(1);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 流程 5：unit.verified —— 显示真实单位值
// ---------------------------------------------------------------------------

describe("流程 5：unit 已核实时显示真实单位值", () => {
  it("verified_unit_shows_the_real_value_in_tree_and_form", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    installFetch(mkState(), probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 变量行只显示名称 + 简短可用状态，不显示单位取值（避免长单位撑乱行）。
    const leaf = screen.getByTestId("catalog-tree-leaf-channel.feed.actual");
    expect(leaf.textContent).not.toContain("mm/min");
    expect(screen.queryByTestId("catalog-tree-leaf-unit-channel.feed.actual")).toBeNull();

    await user.click(leaf);
    const unitTag = await screen.findByTestId("nck-unit");
    expect(unitTag.getAttribute("data-unit-status")).toBe("verified");
    expect(unitTag.textContent).toBe("mm/min, inch/min, user defined");
    // 选中后的「变量详情」与门禁读的是同一份 sidecar 结论（不会一处核实、一处未核实）。
    expect(screen.getByTestId("nck-variable-details").textContent).toContain(
      "mm/min, inch/min, user defined",
    );
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 流程 6：availability 缺失 —— fail-closed 阻断且原因可见
// ---------------------------------------------------------------------------

describe("流程 6：availability 缺失时 fail-closed 阻断并可见", () => {
  it("missing_availability_blocks_with_visible_reason", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    const leaf = screen.getByTestId("catalog-tree-leaf-channel.program.name.main");
    expect(leaf.getAttribute("data-availability")).toBe("unavailable");
    expect(
      screen.getByTestId("catalog-tree-leaf-availability-channel.program.name.main").textContent,
    ).toBe("可用性未知");

    await user.click(leaf);
    expect((await screen.findByTestId("nck-availability")).getAttribute("data-state")).toBe(
      "unavailable",
    );
    const block = screen.getByTestId("nck-availability-block");
    expect(block.getAttribute("data-code")).toBe("AVAILABILITY_MISSING");
    // 原因必须可见：缺失不等于"暂时可用"。
    expect(block.textContent).toContain("AVAILABILITY_MISSING");
    expect(block.textContent).toContain("fail-closed");

    // 参数由载荷填全 + 服务端预览成功也不放行（不得默认放行）。
    await pickObject("channel", "通道1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "channel.program.name.main@channel=i:1",
    );
    const add = screen.getByTestId("nck-add-point") as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    fireEvent.click(add);
    await waitFor(() => expect(screen.getByTestId("catalog-tree")).toBeTruthy());
    expect(screen.queryByTestId("draft-save-summary")).toBeNull();
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 流程 7：ready + 本机无对应对象 —— 静态可用性与动态对象分开显示
// ---------------------------------------------------------------------------

describe("流程 7：ready 但本机没有对应对象时不给虚假可用配置", () => {
  it("ready_static_state_and_dynamic_object_state_are_shown_separately", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ browseEmpty: true });
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    const leaf = screen.getByTestId("catalog-tree-leaf-channel.feed.override");
    expect(leaf.getAttribute("data-availability")).toBe("ready");
    expect(
      screen.getByTestId("catalog-tree-leaf-availability-channel.feed.override").textContent,
    ).toBe("可用");

    await user.click(leaf);

    // 第一处：静态可用性（sidecar 结论）—— ready，留在主视觉。
    const staticRow = await screen.findByTestId("nck-availability");
    expect(staticRow.getAttribute("data-state")).toBe("ready");
    expect(staticRow.textContent).toContain("可用（ready）");

    // 第二处：静态说明与动态对象状态 —— 收在「技术详情」里（默认折叠）。
    expect(screen.queryByTestId("nck-availability-static-note")).toBeNull();
    expect(screen.queryByTestId("nck-object-dynamic-state")).toBeNull();
    await expandTechDetails();
    expect(screen.getByTestId("nck-availability-static-note").textContent).toContain(
      "与当前机床是否存在该对象无关",
    );
    const dynamicRow = screen.getByTestId("nck-object-dynamic-state");
    expect(dynamicRow.getAttribute("data-object-state")).toBe("unconfirmed");
    expect(dynamicRow.textContent).toContain("静态可用性不代表本机存在该对象");

    // 探测：驱动如实返回空页 ⇒ "探测成功但没有对象"，不是"失败"也不是"静默空"。
    await openCascade("channel");
    const missing = await screen.findByTestId("nck-cascade-missing");
    expect(missing.getAttribute("data-reason")).toContain("no_candidates");
    expect(missing.textContent).toContain("探测成功，但设备没有返回");
    expect(screen.queryByTestId("nck-fill-notice")).toBeNull();
    expect(screen.getByTestId("nck-object-dynamic-state").getAttribute("data-object-state")).toBe(
      "unconfirmed",
    );

    // 没有对象 ⇒ 没有可用配置：目标仍为空、无身份、按钮禁用（不给出虚假可用配置）。
    expect(screen.getByTestId("nck-object-select-channel")).toBeTruthy();
    expect(screen.queryByTestId("nck-object-selected-channel")).toBeNull();
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect((screen.getByTestId("nck-add-point") as HTMLButtonElement).disabled).toBe(true);
    // 根页就是空页 ⇒ 没有可下钻的分支，探测只发一次。
    expect(state.browseLog).toEqual([""]);
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});
