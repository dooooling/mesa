// 流程级验收：**真实渲染 + 请求探针**的「选变量 → 选实际对象 → 预览 → 添加 → 保存」。
//
// 为什么必须单独一个流程级文件（而不是继续加纯模型单测）：
// `catalogTree.ts`（分类建树）与 `objectProbe.ts`（对象探测/填参）各自已被单测覆盖，
// 但单测只能证明"函数返回正确"，不能证明"界面上真的按这条流程走"——两模块接进
// 采集抽屉后，中间还隔着表单校验、身份指纹失效、服务端预览、草稿差异与保存门。
// 任一环节接线错位（例如探测请求体写成 `parent_id`、填参后旧 point_key 没失效、
// 失败被渲染成空列表），纯模型单测全绿而用户实际走不通。本文件因此只做一件事：
// 用离线 mock 驱动真实组件，断言**交互路径**与**请求顺序/载荷**。
//
// 约束（与验收口径一致）：
// - 纯离线：不启服务、不连真机、不改配置、不碰生产库；全部请求由本文件 mock；
// - 判定可机械核实：请求标签序列、请求体字段、DOM 文本、按钮可用性；
// - 通用组件零厂商判断：本文件只喂数据，不依赖任何前端厂商分支（见报告 §6）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { createRequestProbe, installProbedFetch } from "../test/requestProbe";
import { NATIVE_RESOURCE } from "../test/acquisitionFixture238";
import type { AcquisitionTaskShape, NckPointSelection } from "../deviceModel";
import type { ResourceDescriptor } from "../types";

const EP = "ep-flow";
const DRIVER = "sinumerik-nck";
const AXES = "nck://identity/axes";
const CHANNELS = "nck://identity/channels";
const OBJECTS = "nck://identity/objects";
const axisId = (n: number) => `${AXES}/${n}`;
const channelId = (n: number) => `${CHANNELS}/${n}`;
const objectId = (n: number) => `${OBJECTS}/${n}`;

// ---------------------------------------------------------------------------
// 1. 目录：六条定义（四类各一条 + 两种"待分类"）
// ---------------------------------------------------------------------------

/**
 * 一条 I-2 目录定义的完整 14 字段形态（缺一个字段 `nckCatalog` 会整体拒绝整页）。
 *
 * 分类字段刻意都带 `category_basis = navigation_rule`：真机本轮恒为导航规则，
 * 不是手册事实；树上必须能看见这个前提（见用例 3）。
 */
function definitionOf(overrides: Record<string, unknown>): Record<string, unknown> {
  return {
    revision: 1,
    family: "828d",
    scope: "collect",
    semantics_status: "confirmed",
    label: { zh: "", en: "" },
    source: [{ kind: "manual", reference: "流程级测试合成条目" }],
    targets: [],
    target_required: [],
    arguments: [],
    identity_template: "",
    identity_keys: [],
    golden_ref: [],
    digest: "digest-synthetic",
    category_basis: "navigation_rule",
    ...overrides,
  };
}

/** 轴/通道两个实例坐标的语义变量（探测轴列表后填 axis+channel）。 */
const AXIS_DEF = definitionOf({
  variable_id: "axis.position.actual_bcs",
  label: { zh: "轴实际位置", en: "Axis actual position" },
  category: "cnc_variable",
  targets: ["axis", "channel"],
  target_required: ["axis", "channel"],
  identity_keys: [
    { path: "axis", required: true },
    { path: "channel", required: true },
  ],
  identity_template: "axis.position.actual_bcs@axis=i:{axis},channel=i:{channel}",
  digest: "digest-axis",
});

/** 单通道语义变量：证明"需要哪些对象"完全来自定义数据（只需 channel）。 */
const FEED_DEF = definitionOf({
  variable_id: "channel.feed.actual",
  label: { zh: "进给速度", en: "Feed rate" },
  category: "system_variable",
  targets: ["channel"],
  target_required: ["channel"],
  identity_keys: [{ path: "channel", required: true }],
  identity_template: "channel.feed.actual@channel=i:{channel}",
  digest: "digest-feed",
});

/** 驱动参数（搜索"actual"跨分类命中的第二类）。 */
const DRIVE_DEF = definitionOf({
  variable_id: "drive.temperature",
  label: { zh: "驱动温度", en: "Drive temperature" },
  category: "drive_parameter",
  targets: ["channel"],
  target_required: ["channel"],
  identity_keys: [{ path: "channel", required: true }],
  identity_template: "drive.temperature@channel=i:{channel}",
  digest: "digest-drive",
});

/** 不声明 targets 的条目：不出现"选择实际对象"入口（入口按需出现）。 */
const MODE_DEF = definitionOf({
  variable_id: "machine_data.mode",
  label: { zh: "运行方式", en: "Operation mode" },
  category: "machine_data",
  targets: [],
  target_required: [],
  digest: "digest-mode",
});

/** 驱动**明确** unclassified：可见、标注未知、但不得作为可采集提交。 */
const DEAD_DEF = definitionOf({
  variable_id: "dead.variable.probe",
  label: { zh: "未登记变量", en: "Unregistered variable" },
  category: "unclassified",
  targets: ["channel"],
  target_required: ["channel"],
  identity_template: "dead.variable.probe@channel=i:{channel}",
  digest: "digest-dead",
});

/** 旧目录（`category` 字段缺失）：落待分类，但只标注"分类未知"，不升级为四类之一。 */
const LEGACY_DEF = definitionOf({
  variable_id: "mystery.variable",
  label: { zh: "旧目录变量", en: "Legacy catalog variable" },
  targets: ["channel"],
  target_required: ["channel"],
  identity_template: "mystery.variable@channel=i:{channel}",
  digest: "digest-mystery",
});

const CATALOG = [AXIS_DEF, FEED_DEF, DRIVE_DEF, MODE_DEF, DEAD_DEF, LEGACY_DEF];

// ---------------------------------------------------------------------------
// 2. 真机形状的 Browse 节点（字段名/取值一律照抄 wire 事实）
// ---------------------------------------------------------------------------

/** 语义叶载荷：只有驱动给的对象坐标，前端不补任何键。 */
function payloadOf(target: Record<string, unknown>, keys: Array<{ path: string; required: boolean }>): string {
  return JSON.stringify({
    identity: { keys, template: "…@axis={axis},channel={channel}" },
    parameters: { target, arguments: {} },
  });
}

/**
 * 根节点：三个可下钻分组。
 *
 * `nck://identity/axes` 故意声明 `selection_mode = 3`（分支不可选）并带一条
 * severity=2 的诊断：证明"分支节点不会被误当成可选对象"。
 */
const ROOT_NODES = [
  {
    id: AXES,
    kind: "identity_group",
    label: "轴（8 个）",
    has_children: true,
    selection_mode: 3,
    diagnostics: [
      { code: "IDENTITY_GROUP_CONTAINER", message: "分组节点只用于下钻，本身不是可选对象", severity: 2 },
    ],
  },
  { id: CHANNELS, kind: "identity_group", label: "通道（8 个）", has_children: true, selection_mode: 0 },
  { id: OBJECTS, kind: "identity_group", label: "驱动对象（8 个候选）", has_children: true, selection_mode: 0 },
];

/**
 * 8 个轴：轴 6 是**未确认**形态（`has_children=false` + 无载荷 + label 含"未确认"），
 * 与真机 `MB1` 轴 6 一致；其余 7 轴带 `selection_mode=2` 的语义载荷。
 */
const AXIS_NODES = Array.from({ length: 8 }, (_, index) => {
  const n = index + 1;
  if (n === 6) {
    return {
      id: axisId(6),
      kind: "nck_variable",
      label: "轴 6 · MB1（未确认：未分配有效驱动输出）",
      has_children: false,
      selection_mode: 0,
    };
  }
  return {
    id: axisId(n),
    kind: "nck_variable",
    label: `轴 ${n} · MB1`,
    has_children: false,
    selection_mode: 2,
    selection_payload_json: payloadOf({ axis: n, channel: 1 }, [
      { path: "axis", required: true },
      { path: "channel", required: true },
    ]),
  };
});

const CHANNEL_NODES = Array.from({ length: 8 }, (_, index) => {
  const n = index + 1;
  return {
    id: channelId(n),
    kind: "nck_variable",
    label: `通道 ${n}`,
    has_children: false,
    selection_mode: 2,
    selection_payload_json: payloadOf({ channel: n }, [{ path: "channel", required: true }]),
  };
});

/**
 * 8 个候选驱动对象：第 3 个被驱动**明确**拒绝（`selection_mode=3` + severity=3 诊断）。
 * 诊断码与文案必须逐字可见——"不可选"三个字不构成解释。
 */
const OBJECT_NODES = Array.from({ length: 8 }, (_, index) => {
  const n = index + 1;
  if (n === 3) {
    return {
      id: objectId(3),
      kind: "nck_drive_object",
      label: "驱动对象 3",
      has_children: false,
      selection_mode: 3,
      diagnostics: [
        {
          code: "DEFINITION_SCOPE_NOT_COLLECTABLE",
          message: "该驱动对象只服务身份探针（scope=identity），不得作为采集点位",
          severity: 3,
        },
      ],
    };
  }
  return {
    id: objectId(n),
    kind: "nck_drive_object",
    label: `驱动对象 ${n}`,
    has_children: false,
    selection_mode: 2,
    selection_payload_json: payloadOf({ channel: 1 }, [{ path: "channel", required: true }]),
  };
});

function browseNodesOf(parent: string): Array<Record<string, unknown>> {
  if (parent === "") return ROOT_NODES;
  if (parent === AXES) return AXIS_NODES;
  if (parent === CHANNELS) return CHANNEL_NODES;
  if (parent === OBJECTS) return OBJECT_NODES;
  return [];
}

// ---------------------------------------------------------------------------
// 3. 请求桩：目录 / 预览 / 探测 / 任务快照 / 保存
// ---------------------------------------------------------------------------

/**
 * 服务端身份预览的 Key 生成（**测试桩**，模拟服务端权威计算）。
 *
 * 关键点：前端**不产生**这个值，它只回显；用例据此证明"提交的 point_key 来自
 * 服务端预览"而不是前端拼出来的。
 */
function pointKeyOf(parameters: Record<string, unknown>): string {
  const variableId = String(parameters.variable_id ?? "");
  const target = (parameters.target ?? {}) as Record<string, unknown>;
  const parts = Object.keys(target)
    .sort()
    .map((key) => {
      const value = target[key];
      if (typeof value === "number") return `${key}=i:${value}`;
      if (value && typeof value === "object") {
        const envelope = value as { number?: unknown; entry?: unknown };
        return `${key}=o:${String(envelope.number)}:${String(envelope.entry)}`;
      }
      return `${key}=${String(value)}`;
    });
  return `${variableId}@${parts.join(",")}`;
}

/** 原生 12 字段里 transport_size 标为 `ui.advanced`（第二层折叠，见用例 8）。 */
const NATIVE_SCHEMA: ResourceDescriptor = {
  ...NATIVE_RESOURCE,
  parameters: {
    fields: (NATIVE_RESOURCE.parameters.fields ?? []).map((field) =>
      field.key === "transport_size"
        ? { ...field, required: false, ui: { ...(field.ui ?? {}), advanced: true } }
        : field,
    ),
  },
};

interface FlowState {
  /** Endpoint 的有效连接配置（`{}` ⇒ family 不可读 ⇒ 探测入口不可用）。 */
  connection: Record<string, unknown>;
  tasks: AcquisitionTaskShape[];
  putBodies: AcquisitionTaskShape[][];
  /** 探测请求体（**逐字保留**，用于钉住 `parent` 字段名）。 */
  browseLog: Array<{ parent: string; filter: string; cursor: string; limit: number }>;
  /** 预览请求体（`{family, kind, parameters}`）。 */
  previewLog: Array<Record<string, unknown>>;
  /** true ⇒ 探测返回 400 + 错误信封（失败用例）。 */
  browseFails: boolean;
}

function mkState(overrides: Partial<FlowState> = {}): FlowState {
  return {
    connection: { family: "828d" },
    tasks: [],
    putBodies: [],
    browseLog: [],
    previewLog: [],
    browseFails: false,
    ...overrides,
  };
}

function ok(body: unknown) {
  return { ok: true, status: 200, json: async () => body };
}

function fail(status: number, body: unknown) {
  return { ok: false, status, json: async () => body };
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
            connection: state.connection,
            runtime: { state: "STOPPED" },
          },
        ],
      });
    }
    // 预览必须先于"目录"判定：两者同前缀，且预览 URL 也含 /definitions。
    if (url.endsWith("/definitions/identity-preview")) {
      const body = JSON.parse(String(init?.body)) as {
        family: string;
        kind: string;
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
        family: "828d",
        revision: 1,
        driver_id: DRIVER,
        driver_version: "0.0.0-flow-test",
        note: null,
        total: CATALOG.length,
        limit: 100,
        next_cursor: null,
        definitions: CATALOG,
      });
    }
    if (url.endsWith("/descriptor")) {
      return ok({
        resources: [NATIVE_SCHEMA],
        // 刻意不声明 browse：本文件验证的是「选择实际对象」探测，不是旧的资源浏览树。
        resource_selection_methods: [],
      });
    }
    if (method === "POST" && url === `/api/v1/endpoints/${EP}/browse`) {
      const body = JSON.parse(String(init?.body ?? "{}")) as {
        parent?: string;
        filter?: string;
        cursor?: string;
        limit?: number;
      };
      state.browseLog.push({
        parent: body.parent ?? "",
        filter: body.filter ?? "",
        cursor: body.cursor ?? "",
        limit: body.limit ?? 0,
      });
      if (state.browseFails) {
        // 驱动不可用（真实信封形状：`{error:{code,kind,message}}`）。
        return fail(400, {
          error: {
            code: "CONNECTION_LOST",
            kind: "ConnectionError",
            message: "驱动进程已退出：无法读取设备对象",
          },
        });
      }
      return ok({ nodes: browseNodesOf(body.parent ?? ""), next_cursor: null });
    }
    if (url.startsWith("/api/v1/tasks?endpoint=")) {
      return ok({ tasks: state.tasks });
    }
    if (method === "PUT" && url.startsWith("/api/v1/tasks/")) {
      const body = JSON.parse(String(init?.body)) as { tasks: AcquisitionTaskShape[] };
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

/** 等页面就绪：目录加载成功后才会出现「+ 添加点位」（NCK 的可编辑前提）。 */
async function waitPaneReady() {
  await screen.findByTestId("add-point-entry", undefined, { timeout: 10_000 });
}

/** 打开添加抽屉并等树渲染（抽屉是 lazy 的：未打开时内部组件不挂载）。 */
async function openDrawer(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("add-point-entry"));
  await screen.findByTestId("catalog-tree");
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

// ---------------------------------------------------------------------------
// 用例 1：树形选择 → 选实际对象 → 预览 → 添加 → 保存（顺序与载荷）
// ---------------------------------------------------------------------------

describe("流程 1：展开树 → 选中变量 → 出现对象入口 → 探测 → 选对象 → 预览 → 添加", () => {
  it("tree_select_probe_preview_add_then_save_keeps_request_order_and_payload", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();

    // 目录必须来自 I-2 分页接口（一次取全，无第二页）。
    expect(probe.count("GET:definitions")).toBe(1);

    await openDrawer(user);
    // 未选变量：只有选择入口，**没有**"选择实际对象"（按需出现）。
    expect(screen.queryByTestId("object-probe")).toBeNull();

    // ---- 展开树：先折叠再展开（证明分组可开合，且条目在分组内）----
    expect(
      screen.getByTestId("catalog-tree-group-cnc_variable").getAttribute("data-expanded"),
    ).toBe("true");
    await user.click(screen.getByTestId("catalog-tree-toggle-cnc_variable"));
    expect(
      screen.getByTestId("catalog-tree-group-cnc_variable").getAttribute("data-expanded"),
    ).toBe("false");
    expect(screen.queryByTestId("catalog-tree-leaf-axis.position.actual_bcs")).toBeNull();
    await user.click(screen.getByTestId("catalog-tree-toggle-cnc_variable"));
    await screen.findByTestId("catalog-tree-leaf-axis.position.actual_bcs");

    // ---- 选中变量：对象需求**来自定义数据**（targets/target_required）----
    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    expect((await screen.findByTestId("object-probe-needs")).textContent).toBe(
      "需要选择：axis* / channel*",
    );
    // 选变量本身不探测、不预览（避免每个变量都拉起驱动进程读真机）。
    expect(probe.count("BROWSE")).toBe(0);
    expect(probe.count("PREVIEW")).toBe(0);

    // ---- 探测：点击才发请求；根页 parent 为空 ----
    await user.click(screen.getByTestId("object-probe-open"));
    const axesNode = await screen.findByTestId(`object-probe-node-${AXES}`);
    // 分组节点声明 selection_mode=3：不可选，且原因可见。
    expect(axesNode.getAttribute("data-mode")).toBe("not_selectable");
    expect(within(axesNode).getByTestId("object-probe-diagnostic-IDENTITY_GROUP_CONTAINER")).toBeTruthy();
    expect(within(axesNode).queryByTestId(`object-probe-use-${AXES}`)).toBeNull();

    // ---- 下钻：请求体字段名必须是 `parent`（写错服务端会静默回到根）----
    await user.click(screen.getByTestId(`object-probe-expand-${AXES}`));
    await screen.findByTestId(`object-probe-node-${axisId(2)}`);

    // ---- 选用实际对象：填参只取驱动载荷里的坐标 ----
    await user.click(screen.getByTestId(`object-probe-use-${axisId(2)}`));
    expect((screen.getByLabelText("target-axis") as HTMLInputElement).value).toBe("2");
    expect((screen.getByLabelText("target-channel") as HTMLInputElement).value).toBe("1");
    expect(screen.getByTestId("object-probe-applied").textContent).toContain("axis=2");
    expect(screen.getByTestId("nck-fill-notice").textContent).toContain(
      "参数已按所选实际对象填入",
    );

    // ---- 服务端预览确认身份（前端不产生 point_key）----
    const key = await screen.findByTestId("nck-point-key");
    expect(key.textContent).toBe("axis.position.actual_bcs@axis=i:2,channel=i:1");
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(false);

    // ---- 添加：只进草稿，零写请求 ----
    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() => expect(screen.queryByTestId("catalog-tree")).toBeNull());
    expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）");
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1);
    expect(probe.count("PUT:tasks")).toBe(0);

    // ---- 请求顺序与载荷：/definitions → /browse(parent) → preview → put ----
    expect(state.browseLog).toEqual([
      { parent: "", filter: "", cursor: "", limit: 50 },
      { parent: AXES, filter: "", cursor: "", limit: 50 },
    ]);
    expect(state.previewLog[0]).toEqual({
      family: "828d",
      kind: "nck_variable",
      parameters: {
        variable_id: "axis.position.actual_bcs",
        definition_revision: 1,
        target: { axis: 2, channel: 1 },
        arguments: {},
      },
    });
    expect(probe.indexOf("GET:definitions")).toBeGreaterThanOrEqual(0);
    expect(probe.indexOf("GET:definitions")).toBeLessThan(probe.indexOf("BROWSE"));
    expect(probe.indexOf("BROWSE")).toBeLessThan(probe.indexOf("PREVIEW"));

    // ---- 保存：已停止 ⇒ PUT → 回读（无 stop / start）----
    await waitFor(() =>
      expect(screen.getByTestId("apply-button").hasAttribute("disabled")).toBe(false),
    );
    // 只观察"点击保存之后"的写路径（挂载时的快照读取 GET /tasks 不在本断言范围内）。
    probe.reset();
    await user.click(screen.getByTestId("apply-button"));
    await screen.findByTestId("apply-result");
    expect(probe.writeSequence()).toEqual(["PUT:tasks", "GET:tasks"]);
    expect(probe.indexOf("PREVIEW")).toBeLessThan(probe.indexOf("PUT:tasks"));

    // 提交载荷 = 服务端预览给出的身份（逐字），前端没有自己拼地址。
    expect(state.putBodies).toHaveLength(1);
    const sent = state.putBodies[0]!;
    expect(sent).toHaveLength(1);
    expect(sent[0]!.id).toBe("t1");
    expect(sent[0]!.schedule).toEqual({ mode: "poll", interval_ms: 1000 });
    expect((sent[0]!.binding.config as { points: NckPointSelection[] }).points).toEqual([
      {
        kind: "nck_variable",
        point_key: "axis.position.actual_bcs@axis=i:2,channel=i:1",
        parameters: {
          variable_id: "axis.position.actual_bcs",
          definition_revision: 1,
          target: { axis: 2, channel: 1 },
          arguments: {},
        },
      },
    ]);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 2：搜索跨分类命中 → 选中 → 同一流程走通
// ---------------------------------------------------------------------------

describe("流程 2：搜索跨分类命中后选中，仍走同一条流程", () => {
  it("search_cross_category_keeps_grouping_and_completes_same_flow", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 搜索词 "actual" 同时命中 cnc_variable 与 system_variable 两条不同分类的条目。
    fireEvent.change(screen.getByTestId("nck-variable-search"), {
      target: { value: "actual" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("catalog-tree-search-summary").textContent).toContain(
        "跨分类匹配 2 个变量（搜索不改变分类，只过滤条目）",
      ),
    );
    // 命中项**仍在各自原分组里**（搜索只过滤条目，不重新归类）。
    expect(
      within(screen.getByTestId("catalog-tree-group-cnc_variable")).getByTestId(
        "catalog-tree-leaf-axis.position.actual_bcs",
      ),
    ).toBeTruthy();
    expect(
      within(screen.getByTestId("catalog-tree-group-system_variable")).getByTestId(
        "catalog-tree-leaf-channel.feed.actual",
      ),
    ).toBeTruthy();
    // 没有命中的分组在搜索时隐藏（分组结构本身不重建）。
    expect(screen.queryByTestId("catalog-tree-group-drive_parameter")).toBeNull();

    // 从"另一个分类"里选中，继续同一条流程。
    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    expect((await screen.findByTestId("object-probe-needs")).textContent).toBe(
      "需要选择：channel*",
    );

    await user.click(screen.getByTestId("object-probe-open"));
    await screen.findByTestId(`object-probe-node-${CHANNELS}`);
    await user.click(screen.getByTestId(`object-probe-expand-${CHANNELS}`));
    await user.click(await screen.findByTestId(`object-probe-use-${channelId(2)}`));
    expect((screen.getByLabelText("target-channel") as HTMLInputElement).value).toBe("2");

    const key = await screen.findByTestId("nck-point-key");
    expect(key.textContent).toBe("channel.feed.actual@channel=i:2");
    await user.click(screen.getByTestId("nck-add-point"));

    await waitFor(() =>
      expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）"),
    );
    // 探测仍然只走两次（根 → 通道组），与树形入口完全同一条路径。
    expect(state.browseLog.map((entry) => entry.parent)).toEqual(["", CHANNELS]);
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 3：待分类条目
// ---------------------------------------------------------------------------

describe("流程 3：待分类条目可见、标注未知、不得被当作四类之一", () => {
  it("unclassified_is_visible_labelled_and_blocked_from_submission", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 驱动明确 unclassified：落"其他 / 待分类"，逐条标注未知。
    const dead = screen.getByTestId("catalog-tree-leaf-dead.variable.probe");
    expect(dead.getAttribute("data-category")).toBe("unclassified");
    expect(dead.getAttribute("data-category-known")).toBe("false");
    expect(
      screen.getByTestId("catalog-tree-leaf-badge-dead.variable.probe").textContent,
    ).toBe("分类未知（驱动 unclassified）");
    // 旧目录（category 缺失）同样落待分类，但只说"分类未知"（不冒充失败关闭结论）。
    const legacy = screen.getByTestId("catalog-tree-leaf-mystery.variable");
    expect(legacy.getAttribute("data-category")).toBe("unclassified");
    expect(legacy.getAttribute("data-category-known")).toBe("false");
    expect(
      screen.getByTestId("catalog-tree-leaf-badge-mystery.variable").textContent,
    ).toBe("分类未知");
    // 分类依据如实写"导航规则"（不是手册事实）。
    expect(
      screen.getByTestId("catalog-tree-basis-unclassified").textContent,
    ).toBe("导航规则");

    // 四类计数之和 = 4：两条待分类**不**被算进四类之一。
    const fourCounts = ["cnc_variable", "system_variable", "machine_data", "drive_parameter"].map(
      (code) => Number(screen.getByTestId(`catalog-tree-group-count-${code}`).textContent),
    );
    expect(fourCounts.reduce((a, b) => a + b, 0)).toBe(4);
    expect(screen.getByTestId("catalog-tree-group-count-unclassified").textContent).toBe("2");

    // 选中驱动明确 unclassified 的条目：可见原因，但**不可提交**。
    await user.click(dead);
    expect((await screen.findByTestId("nck-category-unknown")).textContent).toContain(
      "CATEGORY_UNCLASSIFIED",
    );
    // 即使参数填全也不放行：服务端预览照样能算出 Key（预览不判分类），
    // 但分类守卫与 scope 准入是两道独立判定，提交按钮保持禁用。
    await user.type(screen.getByLabelText("target-channel"), "1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "dead.variable.probe@channel=i:1",
    );
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(true);
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 4：selection_mode=3 的节点
// ---------------------------------------------------------------------------

describe("流程 4：selection_mode=3 节点必须阻断并显示诊断 code+message", () => {
  it("not_selectable_node_shows_diagnostic_code_and_message", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    await screen.findByTestId("object-probe-needs");
    await user.click(screen.getByTestId("object-probe-open"));
    await user.click(await screen.findByTestId(`object-probe-expand-${OBJECTS}`));

    const blocked = await screen.findByTestId(`object-probe-node-${objectId(3)}`);
    expect(blocked.getAttribute("data-mode")).toBe("not_selectable");
    // 诊断码 + 文案逐条展示（不是笼统的"不可选"）。
    const diagnostic = within(blocked).getByTestId(
      "object-probe-diagnostic-DEFINITION_SCOPE_NOT_COLLECTABLE",
    );
    expect(diagnostic.textContent).toContain("DEFINITION_SCOPE_NOT_COLLECTABLE");
    expect(diagnostic.textContent).toContain("该驱动对象只服务身份探针（scope=identity）");
    // severity=3 ⇒ 同时进入"可见阻断"，标题带码、正文带原因。
    const block = within(blocked).getByTestId(
      "object-probe-block-DEFINITION_SCOPE_NOT_COLLECTABLE",
    );
    expect(block.textContent).toContain("不可选用（DEFINITION_SCOPE_NOT_COLLECTABLE）");
    expect(block.textContent).toContain("不得作为采集点位");
    // 没有"用这个对象"入口，也不会产生任何填参结论。
    expect(within(blocked).queryByTestId(`object-probe-use-${objectId(3)}`)).toBeNull();
    expect(screen.queryByTestId("object-probe-applied")).toBeNull();
    expect(screen.queryByTestId("object-probe-fill-error")).toBeNull();
    // 同列表里其它候选对象照常可选（阻断是按节点，不是按列表）。
    expect(screen.getByTestId(`object-probe-use-${objectId(4)}`)).toBeTruthy();
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 5：探测失败 ≠ 空列表
// ---------------------------------------------------------------------------

describe("流程 5：探测失败必须显示失败与原因，不显示空列表", () => {
  it("probe_failure_is_shown_and_never_rendered_as_empty", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ browseFails: true });
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    await user.click(await screen.findByTestId("object-probe-open"));

    const error = await screen.findByTestId("object-probe-error");
    expect(error.getAttribute("data-code")).toBe("PROBE_REQUEST_FAILED");
    // 稳定码 + 服务端错误码 + 原文一起露出（可排查，不是"没有对象"）。
    expect(error.textContent).toContain("PROBE_REQUEST_FAILED");
    expect(error.textContent).toContain("CONNECTION_LOST");
    expect(error.textContent).toContain("驱动进程已退出");
    // 失败时**不渲染空态**，并额外给一条"不会用空列表冒充结果"的告警。
    expect(screen.queryByTestId("object-probe-empty")).toBeNull();
    expect(screen.getByTestId("object-probe-error-alert").textContent).toContain(
      "不会用空列表冒充结果",
    );
    // 探测确实只发了一次（失败不自动重试成"多次探测"）。
    expect(probe.count("BROWSE")).toBe(1);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 6：端点未连接 ⇒ 探测入口不可用且零请求
// ---------------------------------------------------------------------------

describe("流程 6：端点未连接时探测入口不可用且零请求", () => {
  it("unconnected_endpoint_disables_probe_with_reason_and_sends_nothing", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    // 连接配置可读但没有 family ⇒ 端点不可用于在线探测。
    const state = mkState({ connection: {} });
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    const gate = await screen.findByTestId("object-probe-gate");
    expect(gate.textContent).toContain("需先创建并连接端点");
    expect(gate.textContent).toContain("连接配置不可读");
    expect(gate.textContent).toContain("不会发起在线探测");

    // 入口禁用；即使被程序化点击也不发请求、不展开面板。
    const openButton = screen.getByTestId("object-probe-open") as HTMLButtonElement;
    expect(openButton.disabled).toBe(true);
    fireEvent.click(openButton);
    await waitFor(() => expect(openButton.disabled).toBe(true));
    expect(probe.count("BROWSE")).toBe(0);
    expect(screen.queryByTestId("object-probe-panel")).toBeNull();
    // 端点 family 缺失本身另有独立告警（探测闸门不是唯一提示）。
    expect(screen.getByTestId("nck-endpoint-family-missing")).toBeTruthy();
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 7：未确认对象（真机轴 6 `MB1` 形态）
// ---------------------------------------------------------------------------

describe("流程 7：未确认对象不可展开、不可选，也不伪造子节点", () => {
  it("unconfirmed_node_is_not_expandable_not_selectable_and_sends_no_child_request", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    await user.click(await screen.findByTestId("object-probe-open"));
    await user.click(await screen.findByTestId(`object-probe-expand-${AXES}`));

    const mb1 = await screen.findByTestId(`object-probe-node-${axisId(6)}`);
    expect(mb1.textContent).toContain("未确认");
    // has_children=false ⇒ 文案是"无子节点"，没有"进入"入口。
    expect(within(mb1).getByTestId(`object-probe-nochildren-${axisId(6)}`)).toBeTruthy();
    expect(within(mb1).queryByTestId(`object-probe-expand-${axisId(6)}`)).toBeNull();
    // 没有驱动载荷 ⇒ 也没有"用这个对象"入口（不可被当作可选实例）。
    expect(within(mb1).queryByTestId(`object-probe-use-${axisId(6)}`)).toBeNull();

    // 点它既不下钻也不选用：探测请求仍然只有"根 + 轴列表"两次。
    fireEvent.click(within(mb1).getByTestId(`object-probe-nochildren-${axisId(6)}`));
    expect(state.browseLog.map((entry) => entry.parent)).toEqual(["", AXES]);
    expect(screen.queryByTestId("object-probe-applied")).toBeNull();
    // 同组其余 7 个轴照常可用（未确认只影响它自己）。
    expect(screen.getByTestId(`object-probe-use-${axisId(1)}`)).toBeTruthy();
    expect(screen.getByTestId(`object-probe-use-${axisId(7)}`)).toBeTruthy();
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 8：高级原生参数默认隐藏
// ---------------------------------------------------------------------------

describe("流程 8：高级原生参数（module/column/line…）默认不可见", () => {
  it("native_fields_are_collapsed_by_default_and_appear_only_after_expansion", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 第一层：编辑器默认停在"语义点位"页，原生页整体折叠（不改数据、只改默认视图）。
    const mode = await screen.findByTestId("nck-editor-mode");
    expect(mode.getAttribute("data-native-collapsed")).toBe("true");
    expect(screen.queryByTestId("nck-native-fields")).toBeNull();
    expect(document.getElementById("native-field-module")).toBeNull();
    expect(document.getElementById("native-field-column")).toBeNull();
    expect(document.getElementById("native-field-line")).toBeNull();
    expect(screen.queryByTestId("nck-native-add-point")).toBeNull();

    // 主动展开后才渲染原生 12 字段。
    await user.click(screen.getByRole("button", { name: "高级原生配置" }));
    await waitFor(() => expect(document.getElementById("native-field-module")).not.toBeNull());
    expect(screen.getByTestId("nck-editor-mode").getAttribute("data-native-collapsed")).toBe(
      "false",
    );
    expect(document.getElementById("native-field-column")).not.toBeNull();
    expect(document.getElementById("native-field-line")).not.toBeNull();

    // 第二层：`ui.advanced` 标记的字段仍收在"高级（N）"折叠区里。
    expect(document.getElementById("native-field-transport_size")).toBeNull();
    await user.click(screen.getByRole("button", { name: /高级（1）/ }));
    await waitFor(() =>
      expect(document.getElementById("native-field-transport_size")).not.toBeNull(),
    );

    // 回到语义页 ⇒ 原生参数再次整体消失（不是"隐藏某个标签"）。
    await user.click(screen.getByRole("button", { name: "语义点位" }));
    await waitFor(() => expect(screen.queryByTestId("nck-native-fields")).toBeNull());
    expect(document.getElementById("native-field-module")).toBeNull();
    // 整个过程零写请求（只是视图切换）。
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 补充：入口按需出现（不声明 targets 的条目没有"选择实际对象"）
// ---------------------------------------------------------------------------

describe("补充流程：只有需要对象实例的变量才出现「选择实际对象」入口", () => {
  it("entry_appears_only_when_definition_declares_object_needs", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 该条目没有 targets ⇒ 定义元数据不足（不可提交），也没有对象入口。
    await user.click(screen.getByTestId("catalog-tree-leaf-machine_data.mode"));
    expect(await screen.findByTestId("nck-model-reject")).toBeTruthy();
    expect(screen.queryByTestId("object-probe")).toBeNull();
    expect(probe.count("BROWSE")).toBe(0);

    // 换成声明了 targets 的条目 ⇒ 入口才出现。
    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    expect(await screen.findByTestId("object-probe")).toBeTruthy();
    // 但依然"不点击不探测"。
    expect(probe.count("BROWSE")).toBe(0);
  }, 60_000);
});
