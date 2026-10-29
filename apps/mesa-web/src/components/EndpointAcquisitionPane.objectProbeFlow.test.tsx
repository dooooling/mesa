// 流程级验收：**真实渲染 + 请求探针**的「选变量 → 逐级选实际对象 → 自动填参 →
// 预览 → 添加 → 保存」。
//
// 为什么必须单独一个流程级文件（而不是继续加纯模型单测）：
// `catalogTree.ts`（分类建树）与 `objectCascade.ts`（联动选择/缺参数判定）各自已被
// 单测覆盖，但单测只能证明"函数返回正确"，不能证明"界面上真的按这条流程走"——
// 两模块接进采集抽屉后，中间还隔着表单校验、身份指纹失效、服务端预览、草稿差异与
// 保存门。任一环节接线错位（例如探测请求体写成 `parent_id`、填参后旧 point_key 没
// 失效、失败被渲染成空下拉），纯模型单测全绿而用户实际走不通。
//
// **树 id 与节点形状来自驱动，不来自本文件**：Browse 响应体逐字来自驱动产出的契约
// 夹具 `src/test/nckIdentityProbeContract.json`（由
// `drivers/sinumerik-nck/tests/identity_probe_trees.rs` 在回环脚手架上真实浏览后
// 生成，驱动一变夹具就对不上而红）。本文件只按**标签关键字**在夹具里找节点，
// 找不到就抛错——不再手写任何树 id。
//
// 形态已收敛：语义页**没有** Target/Arguments 输入框，参数只能来自驱动载荷；
// 拿不到所需参数时明确提示并阻断（不回退手填、不填默认值）。
//
// 约束（与验收口径一致）：
// - 纯离线：不启服务、不连真机、不改配置、不碰生产库；全部请求由本文件 mock；
// - 判定可机械核实：请求标签序列、请求体字段、DOM 文本、按钮可用性；
// - 通用组件零厂商判断：本文件只喂数据，不依赖任何前端厂商分支。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { message } from "antd";
import { EndpointAcquisitionPane } from "./EndpointAcquisitionPane";
import { createRequestProbe, installProbedFetch } from "../test/requestProbe";
import { NATIVE_RESOURCE } from "../test/acquisitionFixture238";
import type { AcquisitionTaskShape, NckPointSelection } from "../deviceModel";
import type { ResourceDescriptor } from "../types";
import contract from "../test/nckIdentityProbeContract.json";
import { CANDIDATE_AVAILABILITY } from "../test/availabilityFixture";
import { cascadeOptions, openCascade, pickObject } from "../test/objectCascadePick";
import { expandAllCatalogGroups, openVariablePicker } from "../test/catalogTreePick";

const EP = "ep-flow";
const DRIVER = "sinumerik-nck";

// ---------------------------------------------------------------------------
// 0. 驱动产出的 Browse 契约（树 id / 选择模式 / 载荷 / 诊断码全部来自驱动）
// ---------------------------------------------------------------------------

type ContractNode = Record<string, unknown>;
interface ContractPage {
  nodes: ContractNode[];
  next_cursor: string | null;
}

const CONTRACT = contract as unknown as {
  family: string;
  pages: Record<string, ContractPage>;
};

/** 某一 parent 的真实响应体；驱动没有这棵树时返回空页（与驱动行为一致：不伪造）。 */
function pageOf(parent: string): ContractPage {
  return CONTRACT.pages[parent] ?? { nodes: [], next_cursor: null };
}

const idOf = (node: ContractNode): string => String(node.id);
const labelOf = (node: ContractNode): string => String(node.label ?? "");

/** 在夹具里按标签关键字找节点：找不到就失败（说明驱动的树变了，必须重生成夹具）。 */
function nodeByLabel(parent: string, keyword: string): ContractNode {
  const node = pageOf(parent).nodes.find((candidate) => labelOf(candidate).includes(keyword));
  if (!node) {
    throw new Error(`驱动契约里没有「${keyword}」节点（parent=${parent}）：请重生成 nckIdentityProbeContract.json`);
  }
  return node;
}

/** 根页的两个入口（模板目录 + 身份核对）都由驱动给出。 */
const PRESETS_ROOT = idOf(nodeByLabel("", "常用预设模板"));
const IDENTITY_ROOT = idOf(nodeByLabel("", "轴与驱动对象"));
/** 身份根下的四棵树（轴/通道/主轴/驱动对象）都由驱动给出。 */
const AXES = idOf(nodeByLabel(IDENTITY_ROOT, "机床轴"));
const CHANNELS = idOf(nodeByLabel(IDENTITY_ROOT, "通道"));
const SPINDLES = idOf(nodeByLabel(IDENTITY_ROOT, "主轴"));
const OBJECTS = idOf(nodeByLabel(IDENTITY_ROOT, "驱动对象"));

/**
 * 一次联动探测的**完整分支序列**（广度优先，顺序由驱动树的节点顺序决定）。
 *
 * 为什么逐条钉住：探测必须"遍历目录找能提供参数的节点"，而不是猜某一棵固定子树；
 * 少扫一个分支就可能把"设备上有"显示成"没有"。夹具只导出了前端原流程下钻的页
 * （根/presets/identity/channels/spindles），未导出的 axes/objects 在这里如实返回
 * `UNKNOWN_PARENT`——扫描器把它记成"分支不可达"，绝不冒充"设备上没有"。
 */
const SCAN_ORDER = ["", PRESETS_ROOT, IDENTITY_ROOT, AXES, CHANNELS, SPINDLES, OBJECTS];

/**
 * 一次探测里**首次访问**的 parent 顺序。
 *
 * 分页会让同一个 parent 出现多次（夹具只导出首页，后续页为空），因此这里取
 * "首次出现"的顺序：它才是广度优先遍历的真实路径。
 */
function firstVisits(state: FlowState): string[] {
  const parents = state.browseLog.map((entry) => entry.parent);
  return parents.filter((parent, index) => parents.indexOf(parent) === index);
}

/** 通道/主轴实例节点：按驱动标签定位（实例号在标签里，id 也来自夹具）。 */
function channelNode(n: number): ContractNode {
  const node = pageOf(CHANNELS).nodes.find((candidate) => labelOf(candidate).startsWith(`通道${n}`));
  if (!node) throw new Error(`驱动契约里没有通道${n}`);
  return node;
}
function spindleNode(n: number): ContractNode {
  const node = pageOf(SPINDLES).nodes.find((candidate) => labelOf(candidate).startsWith(`主轴${n}`));
  if (!node) throw new Error(`驱动契约里没有主轴${n}`);
  return node;
}
/** 通道页里"无法判定"的诊断节点（驱动用稳定码标出来的）。 */
const CHANNEL_PENDING = pageOf(CHANNELS).nodes.find(
  (node) => node.kind === "diagnostic",
) as ContractNode;

// ---------------------------------------------------------------------------
// 1. 目录：八条定义（四类各若干 + 两种"待分类" + 模板目录条目）
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
    availability: CANDIDATE_AVAILABILITY,
    identity_template: "",
    identity_keys: [],
    golden_ref: [],
    digest: "digest-synthetic",
    category_basis: "navigation_rule",
    ...overrides,
  };
}

/** 轴/通道两个实例坐标的语义变量（只用于"入口按需出现"的对照）。 */
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

/** 单通道语义变量：只需要 channel ⇒ 通道树一个节点就能填满（用例 1）。 */
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

/** 主轴转速：需要 channel + spindle ⇒ 两级下拉各选一个（用例 2）。 */
const SPINDLE_DEF = definitionOf({
  variable_id: "spindle.speed.actual",
  label: { zh: "主轴实际转速", en: "Spindle actual speed" },
  category: "system_variable",
  targets: ["channel", "spindle"],
  target_required: ["channel", "spindle"],
  identity_keys: [
    { path: "channel", required: true },
    { path: "spindle", required: true },
  ],
  identity_template: "spindle.speed.actual@channel=i:{channel},spindle=i:{spindle}",
  digest: "digest-spindle",
});

/** 驱动参数（搜索"actual"跨分类命中的第三类）。 */
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

/** 不声明 targets 的条目：不出现级联下拉（入口按需出现）。 */
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
  variable_id: "channel.feed.override",
  label: { zh: "未登记变量", en: "Unregistered variable" },
  category: "unclassified",
  targets: ["channel"],
  target_required: ["channel"],
  identity_keys: [{ path: "channel", required: true }],
  identity_template: "channel.feed.override@channel=i:{channel}",
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

/**
 * 模板目录里的条目（`axis.ctrlout.module_number`）：驱动把它判为
 * `scope=identity`（不可采集）并给出结构化诊断。它的载荷**没有实例号**，
 * 因此只能作为"不可用候选"出现在 axis 那一级——可见、带码，但选不了。
 */
const CTRLOUT_DEF = definitionOf({
  variable_id: "axis.ctrlout.module_number",
  label: { zh: "控制输出模块号", en: "Ctrlout module number" },
  category: "cnc_variable",
  targets: ["axis"],
  target_required: ["axis"],
  identity_keys: [{ path: "axis", required: true }],
  identity_template: "axis.ctrlout.module_number@axis=i:{axis}",
  digest: "digest-ctrlout",
});

const CATALOG = [
  AXIS_DEF,
  FEED_DEF,
  SPINDLE_DEF,
  DRIVE_DEF,
  MODE_DEF,
  DEAD_DEF,
  LEGACY_DEF,
  CTRLOUT_DEF,
];

// ---------------------------------------------------------------------------
// 2. 请求桩：目录 / 预览 / 探测 / 任务快照 / 保存
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
  /** true ⇒ 探测一律返回空页（"探测成功但没有对象"用例）。 */
  browseEmpty: boolean;
}

function mkState(overrides: Partial<FlowState> = {}): FlowState {
  return {
    connection: { family: "828d" },
    tasks: [],
    putBodies: [],
    browseLog: [],
    previewLog: [],
    browseFails: false,
    browseEmpty: false,
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
        // 刻意不声明 browse：本文件验证的是「联动选择」探测，不是旧的资源浏览树。
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
      if (state.browseEmpty) {
        return ok({ nodes: [], next_cursor: null });
      }
      const parent = body.parent ?? "";
      // 响应体**逐字**来自驱动夹具：本文件不编造任何树 id 或节点形状。
      if (!(parent in CONTRACT.pages)) {
        // 与驱动真实行为一致：未知父节点是错误，不是空页（`UNKNOWN_PARENT`）。
        return fail(400, {
          error: { code: "UNKNOWN_PARENT", message: `未知的浏览父节点 \`${parent}\`` },
        });
      }
      // 夹具只导出每个 parent 的**首页**（`next_cursor` 保留驱动的原值）。后续页在
      // 真实设备上由驱动分页返回，这里如实给空页——绝不复读首页（那会让"分页到底"
      // 变成死循环，掩盖真正的分页处理）。
      if ((body.cursor ?? "") !== "") {
        return ok({ nodes: [], next_cursor: null });
      }
      return ok(pageOf(parent));
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

/** 打开添加抽屉，展开变量下拉并展开全部分组（抽屉是 lazy 的：未打开时内部组件不挂载）。 */
async function openDrawer(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("add-point-entry"));
  await screen.findByTestId("catalog-tree");
  await user.click(screen.getByTestId("nck-variable-picker-trigger"));
  await screen.findByTestId("catalog-tree-panel");
  expandAllCatalogGroups();
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

// ---------------------------------------------------------------------------
// 用例 1：树形选择 → 级联选通道实例 → 自动填参 → 预览 → 添加 → 保存
// ---------------------------------------------------------------------------

describe("流程 1：选中变量 → 出现级联下拉 → 展开才探测 → 选通道 → 自动填参 → 预览 → 添加", () => {
  it("tree_select_cascade_probe_preview_add_then_save_keeps_request_order_and_payload", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();

    // 目录必须来自 I-2 分页接口（一次取全，无第二页）。
    expect(probe.count("GET:definitions")).toBe(1);

    await openDrawer(user);
    // 未选变量：只有选择入口，**没有**级联下拉（按需出现）。
    expect(screen.queryByTestId("nck-object-cascade")).toBeNull();

    // ---- 展开树：先折叠再展开（证明分组可开合，且条目在分组内）----
    expect(
      screen.getByTestId("catalog-tree-group-system_variable").getAttribute("data-expanded"),
    ).toBe("true");
    await user.click(screen.getByTestId("catalog-tree-toggle-system_variable"));
    expect(
      screen.getByTestId("catalog-tree-group-system_variable").getAttribute("data-expanded"),
    ).toBe("false");
    expect(screen.queryByTestId("catalog-tree-leaf-channel.feed.actual")).toBeNull();
    await user.click(screen.getByTestId("catalog-tree-toggle-system_variable"));
    await screen.findByTestId("catalog-tree-leaf-channel.feed.actual");

    // ---- 选中变量：需要的参数**来自定义数据**（targets/target_required）----
    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    expect(await screen.findByTestId("nck-object-level-channel")).toBeTruthy();
    // 语义页没有任何 Target/Arguments 手填框（参数只能来自驱动载荷）。
    expect(screen.queryByTestId(/^nck-target-/)).toBeNull();
    expect(screen.queryByTestId(/^nck-argument-/)).toBeNull();
    // 选变量本身不探测、不预览（避免每个变量都拉起驱动进程读真机）。
    expect(probe.count("BROWSE")).toBe(0);
    expect(probe.count("PREVIEW")).toBe(0);

    // ---- 展开下拉才探测；选通道2 ⇒ 参数由载荷自动填入 ----
    await pickObject("channel", "通道2");
    expect(screen.getByTestId("nck-object-selected-channel").getAttribute("data-value")).toBe("2");
    expect(screen.getByTestId("nck-fill-notice").textContent).toContain(
      "参数已按所选实际对象填入",
    );
    expect(screen.getByTestId("nck-fill-notice").textContent).toContain("驱动返回的选择载荷");

    // ---- 服务端预览确认身份（前端不产生 point_key）----
    const key = await screen.findByTestId("nck-point-key");
    expect(key.textContent).toBe("channel.feed.actual@channel=i:2");
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(false);

    // ---- 添加：只进草稿，零写请求 ----
    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() => expect(screen.queryByTestId("catalog-tree")).toBeNull());
    expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）");
    expect(screen.getAllByTestId("nck-point-row")).toHaveLength(1);
    expect(probe.count("PUT:tasks")).toBe(0);

    // ---- 探测：广度优先遍历驱动的目录分支，逐条记录 parent ----
    expect(firstVisits(state)).toEqual(SCAN_ORDER);
    expect(state.browseLog.every((entry) => entry.limit === 50)).toBe(true);
    // 预览载荷逐字来自"选中对象的驱动载荷"：target 是自动来的，不是手填的。
    expect(state.previewLog[0]).toEqual({
      family: "828d",
      kind: "nck_variable",
      parameters: {
        variable_id: "channel.feed.actual",
        definition_revision: 1,
        target: { channel: 2 },
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
        point_key: "channel.feed.actual@channel=i:2",
        parameters: {
          variable_id: "channel.feed.actual",
          definition_revision: 1,
          target: { channel: 2 },
          arguments: {},
        },
      },
    ]);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 2：搜索跨分类命中 → 两级下拉（channel + spindle）→ 预览 → 添加
// ---------------------------------------------------------------------------

describe("流程 2：搜索跨分类命中后选中，channel 与 spindle 各选一级（不再手填）", () => {
  it("search_cross_category_then_two_cascade_levels_fill_both_keys", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 搜索词 "actual" 同时命中三个分类里的条目。
    fireEvent.change(screen.getByTestId("nck-variable-search"), {
      target: { value: "actual" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("catalog-tree-search-summary").textContent).toContain(
        "跨分类匹配 3 个变量（搜索不改变分类，只过滤条目）",
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
    expect(
      within(screen.getByTestId("catalog-tree-group-system_variable")).getByTestId(
        "catalog-tree-leaf-spindle.speed.actual",
      ),
    ).toBeTruthy();
    // 没有命中的分组在搜索时隐藏（分组结构本身不重建）。
    expect(screen.queryByTestId("catalog-tree-group-drive_parameter")).toBeNull();

    // 从"另一个分类"里选中主轴变量：需求同时来自定义数据（channel + spindle）。
    await user.click(screen.getByTestId("catalog-tree-leaf-spindle.speed.actual"));
    expect(await screen.findByTestId("nck-object-level-channel")).toBeTruthy();
    expect(screen.getByTestId("nck-object-level-spindle")).toBeTruthy();
    // 缺一级 ⇒ 不可提交（fail-closed，不是"先加上再说"）。
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(true);

    // ---- 第一级：通道（主轴1 由轴2/轴4 指向，但通道必须单独选）----
    await pickObject("channel", "通道1");
    expect(screen.getByTestId("nck-object-selected-channel").getAttribute("data-value")).toBe("1");
    // 只填了一级 ⇒ 仍不可提交。
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(true);

    // ---- 第二级：主轴（只能来自驱动读数）----
    const spindle1 = spindleNode(1);
    expect(labelOf(spindle1)).toContain("轴2");
    expect(labelOf(spindle1)).toContain("轴4");
    await pickObject("spindle", "主轴1");
    expect(screen.getByTestId("nck-object-selected-spindle").getAttribute("data-value")).toBe("1");

    const key = await screen.findByTestId("nck-point-key");
    expect(key.textContent).toBe("spindle.speed.actual@channel=i:1,spindle=i:1");
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(false);

    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() =>
      expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）"),
    );
    // 两级都来自同一次探测（探测只发生一次，不因选第二级重复拉驱动）。
    expect(firstVisits(state)).toEqual(SCAN_ORDER);
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
    const dead = screen.getByTestId("catalog-tree-leaf-channel.feed.override");
    expect(dead.getAttribute("data-category")).toBe("unclassified");
    expect(dead.getAttribute("data-category-known")).toBe("false");
    // 明确 unclassified 与"category 缺失"必须可区分（不合并成一个"未知"）。
    expect(dead.getAttribute("data-explicitly-unclassified")).toBe("true");
    // 旧目录（category 缺失）同样落待分类，但**不是**驱动明确的 unclassified。
    const legacy = screen.getByTestId("catalog-tree-leaf-mystery.variable");
    expect(legacy.getAttribute("data-category")).toBe("unclassified");
    expect(legacy.getAttribute("data-category-known")).toBe("false");
    expect(legacy.getAttribute("data-explicitly-unclassified")).toBe("false");
    // 分组标题明确标注"分类未知"（列表行只留名称 + 可用状态，不再逐行挂长角标）。
    expect(screen.getByTestId("catalog-tree-unclassified-hint").textContent).toContain("分类未知");
    // 分类依据如实写"导航规则"（不是手册事实）。
    expect(
      screen.getByTestId("catalog-tree-basis-unclassified").textContent,
    ).toBe("导航规则");

    // 四类计数之和 = 6：两条待分类**不**被算进四类之一。
    const fourCounts = ["cnc_variable", "system_variable", "machine_data", "drive_parameter"].map(
      (code) => Number(screen.getByTestId(`catalog-tree-group-count-${code}`).textContent),
    );
    expect(fourCounts.reduce((a, b) => a + b, 0)).toBe(6);
    expect(screen.getByTestId("catalog-tree-group-count-unclassified").textContent).toBe("2");

    // 选中驱动明确 unclassified 的条目：可见原因，但**不可提交**。
    await user.click(dead);
    expect((await screen.findByTestId("nck-category-unknown")).textContent).toContain(
      "CATEGORY_UNCLASSIFIED",
    );
    // 即使参数由载荷填全、服务端预览照样能算出 Key（预览不判分类），
    // 分类守卫与 scope 准入是两道独立判定，提交按钮保持禁用。
    await pickObject("channel", "通道1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "channel.feed.override@channel=i:1",
    );
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(true);
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 4：驱动判为不可选的节点（码与文案都必须可见）
// ---------------------------------------------------------------------------

describe("流程 4：驱动判为不可选的对象必须可见并带诊断 code+message", () => {
  it("not_selectable_object_is_listed_disabled_with_its_diagnostic_code", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-axis.ctrlout.module_number"));
    await screen.findByTestId("nck-object-level-axis");
    await openCascade("axis");
    // 驱动把模板目录里的这个叶判为 scope=identity：它**可见**（disabled 选项），
    // 但不能选用——码与文案逐字露出，不是笼统的"没有对象"。
    await waitFor(() => expect(cascadeOptions("axis")).toHaveLength(0));
    const select = screen.getByTestId("nck-object-select-axis") as HTMLSelectElement;
    const blocked = Array.from(select.querySelectorAll("option")).filter(
      (option) => option.disabled,
    );
    expect(blocked.length).toBeGreaterThan(0);
    const text = blocked.map((option) => option.textContent ?? "").join(" | ");
    expect(text).toContain("DEFINITION_SCOPE_NOT_COLLECTABLE");
    expect(text).toContain("不可采集");
    // 没有任何填参结论、没有 Key、不可添加。
    expect(screen.queryByTestId("nck-fill-notice")).toBeNull();
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(true);
    // 缺参数原因必须如实（不是"设备没有"就是"探测失败"，两者都在文案里可区分）。
    const missing = await screen.findByTestId("nck-cascade-missing");
    expect(missing.textContent).toContain("缺少参数 axis");
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

    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    await openCascade("channel");

    const error = await screen.findByTestId("nck-cascade-error");
    expect(error.getAttribute("data-code")).toBe("PROBE_REQUEST_FAILED");
    // 稳定码 + 服务端错误码 + 原文一起露出（可排查，不是"没有对象"）。
    expect(error.textContent).toContain("PROBE_REQUEST_FAILED");
    expect(error.textContent).toContain("CONNECTION_LOST");
    expect(error.textContent).toContain("驱动进程已退出");
    expect(error.textContent).toContain("不会用空下拉冒充结果");
    // 缺参数口径必须是"探测失败"，不得说成"设备没有"。
    const missing = await screen.findByTestId("nck-cascade-missing");
    expect(missing.getAttribute("data-reason")).toContain("probe_failed");
    expect(missing.textContent).toContain("探测失败，不代表设备上没有该对象");
    // 下拉本身也不是"静默空"：占位文案写明探测失败。
    const select = screen.getByTestId("nck-object-select-channel") as HTMLSelectElement;
    expect(select.querySelector("option")?.textContent).toContain("探测失败");
    expect(cascadeOptions("channel")).toHaveLength(0);
    // 探测确实只发了一次根请求（根就失败，不继续下钻、不自动重试）。
    expect(probe.count("BROWSE")).toBe(1);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 6：端点未连接 ⇒ 探测入口不可用且零请求
// ---------------------------------------------------------------------------

describe("流程 6：端点未连接时联动选择不可用且零请求", () => {
  it("unconnected_endpoint_disables_cascade_with_reason_and_sends_nothing", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    // 连接配置可读但没有 family ⇒ 端点不可用于在线探测。
    const state = mkState({ connection: {} });
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    const gate = await screen.findByTestId("nck-cascade-gate");
    expect(gate.textContent).toContain("需先创建并连接端点");
    expect(gate.textContent).toContain("连接配置不可读");
    expect(gate.textContent).toContain("不会发起在线探测");

    // 下拉禁用；即使被程序化聚焦也不发请求。
    const select = screen.getByTestId("nck-object-select-axis") as HTMLSelectElement;
    expect(select.disabled).toBe(true);
    fireEvent.focus(select);
    await waitFor(() => expect(select.disabled).toBe(true));
    expect(probe.count("BROWSE")).toBe(0);
    // 阻断 + 两条出路（先建端点 → 采集页；或切高级原生配置）。
    const missing = screen.getByTestId("nck-cascade-missing");
    expect(missing.getAttribute("data-reason")).toContain("probe_unavailable");
    const guidance = screen.getByTestId("nck-cascade-guidance").textContent ?? "";
    expect(guidance).toContain("先完成设备创建与连接");
    expect(guidance).toContain("「采集」页配置点位");
    expect(guidance).toContain("「高级原生配置」");
    // 端点 family 缺失本身另有独立告警（探测闸门不是唯一提示）。
    expect(screen.getByTestId("nck-endpoint-family-missing")).toBeTruthy();
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 7：驱动"无法判定"的实例（只有诊断，不可展开、不可选）
// ---------------------------------------------------------------------------

describe("流程 7：驱动无法判定的实例只显示诊断，不可选，也不伪造候选", () => {
  it("undecidable_instance_is_diagnosed_not_selectable_and_not_offered", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    await openCascade("channel");
    await waitFor(() => expect(cascadeOptions("channel")).toHaveLength(2));

    // 诊断码与文案原样可见（"无法判定"不是"不存在"，必须能区分）。
    const diagnostic = await screen.findByTestId(
      "nck-cascade-diagnostic-IDENTITY_PENDING_REVIEW",
    );
    expect(diagnostic.textContent).toContain("IDENTITY_PENDING_REVIEW");
    expect(diagnostic.textContent).toContain("无法判定");
    expect(diagnostic.textContent).toContain(labelOf(CHANNEL_PENDING));
    // 它不在候选里（没有载荷 ⇒ 不能成为可选项，也不伪造实例号）。
    const values = cascadeOptions("channel").map((option) => option.value);
    expect(values).not.toContain(idOf(CHANNEL_PENDING));
    // 同页其余通道照常可用（无法判定只影响它自己）。
    expect(values).toContain(idOf(channelNode(1)));
    expect(screen.queryByTestId("nck-fill-notice")).toBeNull();
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 8：高级原生配置默认不可见（两个入口互斥）
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

    // 第一层：编辑器默认停在「联动选择」页，原生页整体折叠（不改数据、只改默认视图）。
    const mode = await screen.findByTestId("nck-editor-mode");
    expect(mode.getAttribute("data-native-collapsed")).toBe("true");
    expect(mode.getAttribute("data-editor-mode")).toBe("semantic");
    expect(screen.queryByTestId("nck-native-fields")).toBeNull();
    expect(document.getElementById("native-field-module")).toBeNull();
    expect(document.getElementById("native-field-column")).toBeNull();
    expect(document.getElementById("native-field-line")).toBeNull();
    expect(screen.queryByTestId("nck-native-add-point")).toBeNull();

    // 主动展开后才渲染原生 12 字段，且联动表单整体消失（任何时刻只有一套表单）。
    await user.click(screen.getByRole("button", { name: "高级原生配置" }));
    await waitFor(() => expect(document.getElementById("native-field-module")).not.toBeNull());
    expect(screen.getByTestId("nck-editor-mode").getAttribute("data-native-collapsed")).toBe(
      "false",
    );
    expect(document.getElementById("native-field-column")).not.toBeNull();
    expect(document.getElementById("native-field-line")).not.toBeNull();
    expect(screen.queryByTestId("nck-object-cascade")).toBeNull();
    expect(screen.queryByTestId("catalog-tree")).toBeNull();

    // 第二层：`ui.advanced` 标记的字段仍收在"高级（N）"折叠区里。
    expect(document.getElementById("native-field-transport_size")).toBeNull();
    await user.click(screen.getByRole("button", { name: /高级（1）/ }));
    await waitFor(() =>
      expect(document.getElementById("native-field-transport_size")).not.toBeNull(),
    );

    // 回到联动选择 ⇒ 原生参数再次整体消失（不是"隐藏某个标签"）。
    await user.click(screen.getByRole("button", { name: "联动选择" }));
    await waitFor(() => expect(screen.queryByTestId("nck-native-fields")).toBeNull());
    expect(document.getElementById("native-field-module")).toBeNull();
    // 整个过程零写请求（只是视图切换）。
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 补充：入口按需出现（不声明 targets 的条目没有级联下拉）
// ---------------------------------------------------------------------------

describe("补充流程：只有需要对象实例的变量才出现级联下拉", () => {
  it("cascade_appears_only_when_definition_declares_object_needs", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    // 该条目没有 targets ⇒ 定义元数据不足（不可提交），也没有级联下拉。
    await user.click(screen.getByTestId("catalog-tree-leaf-machine_data.mode"));
    expect(await screen.findByTestId("nck-model-reject")).toBeTruthy();
    expect(screen.queryByTestId("nck-object-cascade")).toBeNull();
    expect(probe.count("BROWSE")).toBe(0);

    // 换成声明了 targets 的条目 ⇒ 下拉才出现（选择后下拉已收起，先重新打开）。
    openVariablePicker();
    await user.click(screen.getByTestId("catalog-tree-leaf-axis.position.actual_bcs"));
    expect(await screen.findByTestId("nck-object-cascade")).toBeTruthy();
    // 但依然"不展开不探测"。
    expect(probe.count("BROWSE")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 9：通道树的三类结果在界面上必须可区分（有效 / 无法判定 / 不存在）
// ---------------------------------------------------------------------------

describe("流程 9：通道候选只显示驱动确认的通道，被拒实例不出现、无法判定有诊断", () => {
  it("channel_candidates_show_only_confirmed_channels_and_mark_undecidable_ones", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    await openCascade("channel");
    await waitFor(() => expect(cascadeOptions("channel")).toHaveLength(2));

    // 被设备拒绝的实例（夹具里通道 4 及以后）**一个节点都不该出现**——
    // 否则界面会把"设备说没有"显示成"这里有个通道"。
    expect(screen.queryByText(/^通道4/)).toBeNull();
    // 有效通道：可选，标签写明读数依据。
    expect(labelOf(channelNode(1))).toContain("位置1→机床轴1");
    // 无法判定：诊断可见，且不在候选里。
    const pending = screen.getByTestId("nck-cascade-diagnostic-IDENTITY_PENDING_REVIEW");
    expect(pending.textContent).toContain("无法判定");
    expect(cascadeOptions("channel").map((option) => option.value)).not.toContain(
      idOf(CHANNEL_PENDING),
    );

    // 选用有效通道后填参来自驱动载荷（通道号不来自标签解析）。
    await pickObject("channel", "通道1");
    expect(screen.getByTestId("nck-object-selected-channel").getAttribute("data-value")).toBe("1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "channel.feed.actual@channel=i:1",
    );
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});

// ---------------------------------------------------------------------------
// 用例 10：探测成功但设备没有对象 ⇒ "设备没有"口径 + 阻断（零写入）
// ---------------------------------------------------------------------------

describe("流程 10：探测成功但没有对象时必须说清「设备没有」并阻断", () => {
  it("successful_but_empty_probe_blocks_with_no_candidates_wording", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState({ browseEmpty: true });
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    await openCascade("channel");
    const missing = await screen.findByTestId("nck-cascade-missing");
    expect(missing.getAttribute("data-reason")).toContain("no_candidates");
    expect(missing.textContent).toContain("探测成功，但设备没有返回");
    expect(missing.textContent).not.toContain("探测失败");
    // 没有对象 ⇒ 没有可用配置：无填参、无身份、按钮禁用（零写请求）。
    expect(screen.queryByTestId("nck-fill-notice")).toBeNull();
    expect(screen.queryByTestId("nck-point-key")).toBeNull();
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(true);
    expect(probe.count("PUT:tasks")).toBe(0);
    // 根页就没有对象 ⇒ 没有可下钻的分支，探测只发一次（不空转）。
    expect(probe.count("BROWSE")).toBe(1);
  }, 60_000);
});
