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
// **树 id 与节点形状来自驱动，不来自本文件**：
// 上一版的 `nck://identity/channels` 是测试里凭空写出来的字符串，驱动根本没有
// 这棵树——测试全绿，真机走不通。现在 Browse 响应体逐字来自驱动产出的契约夹具
// `src/test/nckIdentityProbeContract.json`（由
// `drivers/sinumerik-nck/tests/identity_probe_trees.rs` 在回环脚手架上真实浏览
// 后生成，驱动一变夹具就对不上而红）。本文件只按**标签关键字**在夹具里找节点，
// 找不到就抛错——不再手写任何树 id。
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
/** 身份根下的四棵树里，本文件用到通道与主轴两棵（轴/对象树由驱动侧测试覆盖）。 */
const CHANNELS = idOf(nodeByLabel(IDENTITY_ROOT, "通道"));
const SPINDLES = idOf(nodeByLabel(IDENTITY_ROOT, "主轴"));

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
/** 模板页里第一条 `scope=identity` 的叶（驱动判为 NOT_SELECTABLE + 结构化诊断）。 */
const IDENTITY_SCOPE_LEAF = nodeByLabel(PRESETS_ROOT, "MDCA_CTRLOUT_MODULE_NR");
/** 同一页里的可采集叶（用于证明阻断是按节点的，不是按列表的）。 */
const COLLECTABLE_LEAF = nodeByLabel(PRESETS_ROOT, "opMode");

// ---------------------------------------------------------------------------
// 1. 目录：七条定义（四类各一条 + 主轴 + 两种"待分类"）
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

/** 主轴转速：需要 channel + spindle ⇒ 主轴节点只填 spindle，channel 由用户补（用例 2）。 */
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

const CATALOG = [AXIS_DEF, FEED_DEF, SPINDLE_DEF, DRIVE_DEF, MODE_DEF, DEAD_DEF, LEGACY_DEF];

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
      const parent = body.parent ?? "";
      // 响应体**逐字**来自驱动夹具：本文件不编造任何树 id 或节点形状。
      if (!(parent in CONTRACT.pages)) {
        // 与驱动真实行为一致：未知父节点是错误，不是空页（`UNKNOWN_PARENT`）。
        return fail(400, {
          error: { code: "UNKNOWN_PARENT", message: `未知的浏览父节点 \`${parent}\`` },
        });
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

/** 打开添加抽屉并等树渲染（抽屉是 lazy 的：未打开时内部组件不挂载）。 */
async function openDrawer(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("add-point-entry"));
  await screen.findByTestId("catalog-tree");
}

/** 下钻到某一棵树的实例页：根 → 身份根 → 目标树（每一步都是驱动真实节点）。 */
async function drillTo(
  user: ReturnType<typeof userEvent.setup>,
  treeId: string,
) {
  await user.click(screen.getByTestId("object-probe-open"));
  await screen.findByTestId(`object-probe-node-${IDENTITY_ROOT}`);
  await user.click(screen.getByTestId(`object-probe-expand-${IDENTITY_ROOT}`));
  await screen.findByTestId(`object-probe-node-${treeId}`);
  await user.click(screen.getByTestId(`object-probe-expand-${treeId}`));
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  message.destroy();
});

// ---------------------------------------------------------------------------
// 用例 1：树形选择 → 选通道实例 → 预览 → 添加 → 保存（顺序与载荷）
// ---------------------------------------------------------------------------

describe("流程 1：展开树 → 选中变量 → 出现对象入口 → 探测通道 → 选通道 → 预览 → 添加", () => {
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
      screen.getByTestId("catalog-tree-group-system_variable").getAttribute("data-expanded"),
    ).toBe("true");
    await user.click(screen.getByTestId("catalog-tree-toggle-system_variable"));
    expect(
      screen.getByTestId("catalog-tree-group-system_variable").getAttribute("data-expanded"),
    ).toBe("false");
    expect(screen.queryByTestId("catalog-tree-leaf-channel.feed.actual")).toBeNull();
    await user.click(screen.getByTestId("catalog-tree-toggle-system_variable"));
    await screen.findByTestId("catalog-tree-leaf-channel.feed.actual");

    // ---- 选中变量：对象需求**来自定义数据**（targets/target_required）----
    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    expect((await screen.findByTestId("object-probe-needs")).textContent).toBe(
      "需要选择：channel*",
    );
    // 选变量本身不探测、不预览（避免每个变量都拉起驱动进程读真机）。
    expect(probe.count("BROWSE")).toBe(0);
    expect(probe.count("PREVIEW")).toBe(0);

    // ---- 探测：点击才发请求；根页 parent 为空 ----
    await user.click(screen.getByTestId("object-probe-open"));
    const identityNode = await screen.findByTestId(`object-probe-node-${IDENTITY_ROOT}`);
    // 分支节点声明 selection_mode=3：不可选（驱动给的可选性，前端不推断）。
    expect(identityNode.getAttribute("data-mode")).toBe("not_selectable");
    expect(within(identityNode).queryByTestId(`object-probe-use-${IDENTITY_ROOT}`)).toBeNull();

    // ---- 下钻：请求体字段名必须是 `parent`（写错服务端会报 UNKNOWN_PARENT）----
    await user.click(screen.getByTestId(`object-probe-expand-${IDENTITY_ROOT}`));
    await screen.findByTestId(`object-probe-node-${CHANNELS}`);
    await user.click(screen.getByTestId(`object-probe-expand-${CHANNELS}`));

    // ---- 通道页：可选实例 + 驱动标出的"无法判定"诊断 ----
    const channel2 = await screen.findByTestId(`object-probe-node-${idOf(channelNode(2))}`);
    expect(channel2.getAttribute("data-mode")).toBe("semantic");
    const pending = screen.getByTestId(`object-probe-node-${idOf(CHANNEL_PENDING)}`);
    expect(pending.getAttribute("data-mode")).toBe("not_selectable");
    expect(
      within(pending).getByTestId("object-probe-diagnostic-IDENTITY_PENDING_REVIEW"),
    ).toBeTruthy();

    // ---- 选用实际对象：填参只取驱动载荷里的坐标 ----
    await user.click(screen.getByTestId(`object-probe-use-${idOf(channelNode(2))}`));
    expect((screen.getByLabelText("target-channel") as HTMLInputElement).value).toBe("2");
    expect(screen.getByTestId("object-probe-applied").textContent).toContain("channel=2");
    expect(screen.getByTestId("nck-fill-notice").textContent).toContain(
      "参数已按所选实际对象填入",
    );

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

    // ---- 请求顺序与载荷：/definitions → /browse(根) → /browse(身份根) → /browse(通道) → preview ----
    expect(state.browseLog.map((entry) => entry.parent)).toEqual([
      "",
      IDENTITY_ROOT,
      CHANNELS,
    ]);
    expect(state.browseLog.every((entry) => entry.limit === 50)).toBe(true);
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
// 用例 2：搜索跨分类命中 → 主轴树（驱动派生的主轴）→ 补 channel → 预览 → 添加
// ---------------------------------------------------------------------------

describe("流程 2：搜索跨分类命中后选中，用主轴树填 spindle，再补 channel", () => {
  it("search_cross_category_then_spindle_tree_fills_spindle_and_leaves_channel_unresolved", async () => {
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
    expect((await screen.findByTestId("object-probe-needs")).textContent).toBe(
      "需要选择：channel* / spindle*",
    );

    // ---- 主轴树：主轴只能来自驱动读数（夹具里主轴1 由轴2/轴4 指向）----
    await drillTo(user, SPINDLES);
    const spindle1 = await screen.findByTestId(`object-probe-node-${idOf(spindleNode(1))}`);
    expect(spindle1.getAttribute("data-mode")).toBe("semantic");
    // 去重后的来源轴必须可见（用户要能看出"主轴1 是哪根轴给的"）。
    expect(spindle1.textContent).toContain("轴2");
    expect(spindle1.textContent).toContain("轴4");

    await user.click(screen.getByTestId(`object-probe-use-${idOf(spindleNode(1))}`));
    expect((screen.getByLabelText("target-spindle") as HTMLInputElement).value).toBe("1");
    // 主轴节点只提供 spindle：channel 必须如实列为"仍需手动填写"（不填默认值）。
    expect(screen.getByTestId("object-probe-applied").textContent).toContain(
      "仍需手动填写：channel",
    );
    // 缺必填 Target ⇒ 不可提交（fail-closed，不是"先加上再说"）。
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(true);

    // 手动补 channel 后才由服务端算出身份。
    await user.type(screen.getByLabelText("target-channel"), "1");
    const key = await screen.findByTestId("nck-point-key");
    expect(key.textContent).toBe("spindle.speed.actual@channel=i:1,spindle=i:1");
    expect(screen.getByTestId("nck-add-point").hasAttribute("disabled")).toBe(false);

    await user.click(screen.getByTestId("nck-add-point"));
    await waitFor(() =>
      expect(screen.getByTestId("draft-save-summary").textContent).toBe("1 项点位变更（新增 1）"),
    );
    // 探测只走"根 → 身份根 → 主轴"三次，与树形入口完全同一条路径。
    expect(state.browseLog.map((entry) => entry.parent)).toEqual(["", IDENTITY_ROOT, SPINDLES]);
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

    // 四类计数之和 = 5：两条待分类**不**被算进四类之一。
    const fourCounts = ["cnc_variable", "system_variable", "machine_data", "drive_parameter"].map(
      (code) => Number(screen.getByTestId(`catalog-tree-group-count-${code}`).textContent),
    );
    expect(fourCounts.reduce((a, b) => a + b, 0)).toBe(5);
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
// 用例 4：selection_mode=3 的节点（驱动准入拒绝，码与文案都必须可见）
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

    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    await screen.findByTestId("object-probe-needs");
    // 模板页也是驱动给出的树：`scope=identity` 的叶由驱动判为不可选。
    await user.click(screen.getByTestId("object-probe-open"));
    await user.click(await screen.findByTestId(`object-probe-expand-${PRESETS_ROOT}`));

    const blocked = await screen.findByTestId(
      `object-probe-node-${idOf(IDENTITY_SCOPE_LEAF)}`,
    );
    expect(blocked.getAttribute("data-mode")).toBe("not_selectable");
    // 诊断码 + 文案逐条展示（不是笼统的"不可选"）。
    const diagnostic = within(blocked).getByTestId(
      "object-probe-diagnostic-DEFINITION_SCOPE_NOT_COLLECTABLE",
    );
    expect(diagnostic.textContent).toContain("DEFINITION_SCOPE_NOT_COLLECTABLE");
    expect(diagnostic.textContent).toContain("scope=`identity`");
    // severity=3 ⇒ 同时进入"可见阻断"，标题带码、正文带原因。
    const block = within(blocked).getByTestId(
      "object-probe-block-DEFINITION_SCOPE_NOT_COLLECTABLE",
    );
    expect(block.textContent).toContain("不可选用（DEFINITION_SCOPE_NOT_COLLECTABLE）");
    expect(block.textContent).toContain("不可采集");
    // 没有"用这个对象"入口，也不会产生任何填参结论。
    expect(within(blocked).queryByTestId(`object-probe-use-${idOf(IDENTITY_SCOPE_LEAF)}`)).toBeNull();
    expect(screen.queryByTestId("object-probe-applied")).toBeNull();
    expect(screen.queryByTestId("object-probe-fill-error")).toBeNull();
    // 同列表里可采集的叶照常可选（阻断是按节点，不是按列表）。
    expect(screen.getByTestId(`object-probe-use-${idOf(COLLECTABLE_LEAF)}`)).toBeTruthy();
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
// 用例 7：驱动"无法判定"的实例（只有诊断，不可展开、不可选）
// ---------------------------------------------------------------------------

describe("流程 7：驱动无法判定的实例只显示诊断，不可展开、不可选，也不伪造子节点", () => {
  it("undecidable_instance_is_diagnosed_not_expandable_and_not_selectable", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    await drillTo(user, CHANNELS);

    const pending = await screen.findByTestId(`object-probe-node-${idOf(CHANNEL_PENDING)}`);
    // 诊断码与文案原样可见（"无法判定"不是"不存在"，必须能区分）。
    expect(pending.getAttribute("data-mode")).toBe("not_selectable");
    const diagnostic = within(pending).getByTestId(
      "object-probe-diagnostic-IDENTITY_PENDING_REVIEW",
    );
    expect(diagnostic.textContent).toContain("IDENTITY_PENDING_REVIEW");
    expect(diagnostic.textContent).toContain("无法判定");
    // has_children=false ⇒ 文案是"无子节点"，没有"进入"入口。
    expect(
      within(pending).getByTestId(`object-probe-nochildren-${idOf(CHANNEL_PENDING)}`),
    ).toBeTruthy();
    expect(
      within(pending).queryByTestId(`object-probe-expand-${idOf(CHANNEL_PENDING)}`),
    ).toBeNull();
    // 没有驱动载荷 ⇒ 也没有"用这个对象"入口（不可被当作可选实例）。
    expect(
      within(pending).queryByTestId(`object-probe-use-${idOf(CHANNEL_PENDING)}`),
    ).toBeNull();

    // 点它既不下钻也不选用：探测请求仍然只有"根 → 身份根 → 通道"三次。
    fireEvent.click(within(pending).getByTestId(`object-probe-nochildren-${idOf(CHANNEL_PENDING)}`));
    expect(state.browseLog.map((entry) => entry.parent)).toEqual(["", IDENTITY_ROOT, CHANNELS]);
    expect(screen.queryByTestId("object-probe-applied")).toBeNull();
    // 同页其余通道照常可用（无法判定只影响它自己）。
    expect(
      screen.getByTestId(`object-probe-use-${idOf(channelNode(1))}`),
    ).toBeTruthy();
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

// ---------------------------------------------------------------------------
// 用例 9：通道树的四类结果在界面上必须可区分（有效 / 被拒 / 无法判定）
// ---------------------------------------------------------------------------

describe("流程 9：通道树只显示驱动确认的通道，被拒实例不出现、无法判定有诊断", () => {
  it("channel_tree_shows_only_confirmed_channels_and_marks_undecidable_ones", async () => {
    const user = userEvent.setup();
    const probe = createRequestProbe();
    const state = mkState();
    installFetch(state, probe);
    renderPane();
    await waitPaneReady();
    await openDrawer(user);

    await user.click(screen.getByTestId("catalog-tree-leaf-channel.feed.actual"));
    await drillTo(user, CHANNELS);
    await screen.findByTestId(`object-probe-node-${idOf(channelNode(1))}`);

    // 通道页一共 3 项：两个有效通道 + 一条"无法判定"诊断。
    // 被设备拒绝的实例（夹具里通道 4 及以后）**一个节点都不该出现**——
    // 否则界面会把"设备说没有"显示成"这里有个通道"。
    const nodes = screen.getAllByTestId(/^object-probe-node-/);
    expect(nodes).toHaveLength(pageOf(CHANNELS).nodes.length);
    expect(nodes.map((node) => node.getAttribute("data-mode"))).toEqual([
      "semantic",
      "semantic",
      "not_selectable",
    ]);
    expect(screen.queryByText(/^通道4/)).toBeNull();

    // 有效通道：可选用，标签写明读数依据。
    const channel1 = screen.getByTestId(`object-probe-node-${idOf(channelNode(1))}`);
    expect(channel1.textContent).toContain("位置1→机床轴1");
    expect(screen.getByTestId(`object-probe-use-${idOf(channelNode(1))}`)).toBeTruthy();

    // 无法判定：诊断可见，且没有"用这个对象"入口。
    const pending = screen.getByTestId(`object-probe-node-${idOf(CHANNEL_PENDING)}`);
    expect(
      within(pending).getByTestId("object-probe-diagnostic-IDENTITY_PENDING_REVIEW").textContent,
    ).toContain("无法判定");
    expect(
      within(pending).queryByTestId(`object-probe-use-${idOf(CHANNEL_PENDING)}`),
    ).toBeNull();

    // 选用有效通道后填参来自驱动载荷（通道号不来自标签解析）。
    await user.click(screen.getByTestId(`object-probe-use-${idOf(channelNode(1))}`));
    expect((screen.getByLabelText("target-channel") as HTMLInputElement).value).toBe("1");
    expect((await screen.findByTestId("nck-point-key")).textContent).toBe(
      "channel.feed.actual@channel=i:1",
    );
    expect(probe.count("PUT:tasks")).toBe(0);
  }, 60_000);
});
